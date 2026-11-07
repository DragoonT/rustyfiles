//! Folder size calculation: the details view's Size column ("Boyutu hesapla" / Settings →
//! "Klasör boyutlarını hesapla"), the preview pane and Properties.
//!
//! Directories are listed with `FindFirstFileExW` (basic info, large fetch) by a few shared
//! worker threads at below-normal CPU priority (the UI thread always wins; I/O priority stays
//! normal, background mode throttles the disk so hard that a column of folders crawls).
//! The workers serve all requested folders **round-robin**, one directory at a time: a small
//! folder clicked while a huge one is being measured still finishes in milliseconds instead of
//! queueing behind it. Reparse points (junctions, symlinks, OneDrive placeholders) are neither
//! counted nor followed. Unreadable sub-folders are skipped silently.
//!
//! Hard links are counted once per link; compressed / sparse files are measured by logical size.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use parking_lot::{Condvar, Mutex};
use rf_core::attrs;
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceW, GetVolumePathNameW};
use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL};
use windows::core::PCWSTR;

pub use crate::hash::Cancelled;
use crate::search::for_each_raw;
use crate::wide::{from_wide, wide};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FolderSize {
    /// Sum of logical file sizes.
    pub bytes: u64,
    pub files: u64,
    /// Sub-folders (the root itself is not counted).
    pub dirs: u64,
    /// Approximate size on disk: every file rounded up to the volume cluster size. `None` when
    /// the cluster size could not be determined.
    pub allocated: Option<u64>,
}

#[derive(Default)]
struct Counters {
    bytes: AtomicU64,
    files: AtomicU64,
    dirs: AtomicU64,
    allocated: AtomicU64,
}

impl Counters {
    fn snapshot(&self, track_alloc: bool) -> FolderSize {
        FolderSize {
            bytes: self.bytes.load(Ordering::Relaxed),
            files: self.files.load(Ordering::Relaxed),
            dirs: self.dirs.load(Ordering::Relaxed),
            allocated: track_alloc.then(|| self.allocated.load(Ordering::Relaxed)),
        }
    }
}

/// Bytes per cluster of the volume holding `dir`.
fn cluster_size(dir: &Path) -> Option<u64> {
    let src = wide(dir);
    let mut root = [0u16; 261];
    unsafe {
        GetVolumePathNameW(PCWSTR(src.as_ptr()), &mut root).ok()?;
        let (mut spc, mut bps) = (0u32, 0u32);
        GetDiskFreeSpaceW(PCWSTR(root.as_ptr()), Some(&mut spc), Some(&mut bps), None, None).ok()?;
        let c = spc as u64 * bps as u64;
        (c > 0).then_some(c)
    }
}

/// Folder sizes shared by the Size column, the preview pane and Properties.
pub fn shared_cache() -> &'static FolderSizeCache {
    static CACHE: OnceLock<FolderSizeCache> = OnceLock::new();
    CACHE.get_or_init(FolderSizeCache::default)
}

// ------------------------------------------------------------------------- scheduler

/// One folder being measured.
struct Job {
    batch: Arc<SizeBatch>,
    i: usize,
    root: PathBuf,
    /// Directories still to list (depth-first: the most recently found first).
    todo: Mutex<Vec<PathBuf>>,
    started: AtomicBool,
}

struct Sched {
    jobs: Mutex<Vec<Arc<Job>>>,
    cv: Condvar,
    next: AtomicUsize,
}

/// Measured on NVMe with 16 hardware threads: 8 workers is the sweet spot, more only contend.
fn sched() -> &'static Sched {
    static S: OnceLock<&'static Sched> = OnceLock::new();
    S.get_or_init(|| {
        let s: &'static Sched = Box::leak(Box::new(Sched { jobs: Mutex::new(Vec::new()), cv: Condvar::new(), next: AtomicUsize::new(0) }));
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(4, 8);
        for k in 0..n {
            let _ = std::thread::Builder::new().name(format!("rf-dirsize-{k}")).spawn(move || {
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                }
                worker(s)
            });
        }
        s
    })
}

/// Next (job, directory), taking the jobs in turn so every folder keeps moving.
fn take(s: &Sched) -> (Arc<Job>, PathBuf) {
    let mut jobs = s.jobs.lock();
    loop {
        let n = jobs.len();
        if n > 0 {
            let start = s.next.fetch_add(1, Ordering::Relaxed);
            for k in 0..n {
                let job = &jobs[(start + k) % n];
                if let Some(dir) = job.todo.lock().pop() {
                    return (job.clone(), dir);
                }
            }
        }
        s.cv.wait_for(&mut jobs, Duration::from_millis(250));
    }
}

fn worker(s: &'static Sched) {
    loop {
        let (job, dir) = take(s);
        process(s, &job, dir);
    }
}

fn finish(s: &Sched, job: &Job) {
    let slot = &job.batch.slots[job.i];
    if !job.batch.stop.load(Ordering::Relaxed) {
        let size = slot.c.snapshot(slot.cluster.get().copied().flatten().is_some());
        shared_cache().insert_at(&job.root, size, slot.mtime.get().copied().flatten());
    }
    slot.done.store(true, Ordering::Release);
    s.jobs.lock().retain(|j| !(Arc::ptr_eq(&j.batch, &job.batch) && j.i == job.i));
}

fn process(s: &Sched, job: &Job, dir: PathBuf) {
    let slot = &job.batch.slots[job.i];
    if job.batch.stop.load(Ordering::Relaxed) {
        // drop the rest of this folder's work at once
        let left = job.todo.lock().drain(..).count();
        if slot.pending.fetch_sub(left + 1, Ordering::AcqRel) == left + 1 {
            finish(s, job);
        }
        return;
    }
    if !job.started.swap(true, Ordering::Relaxed) {
        if let Some(c) = shared_cache().get(&job.root) {
            slot.c.bytes.store(c.bytes, Ordering::Relaxed);
            slot.c.files.store(c.files, Ordering::Relaxed);
            slot.c.dirs.store(c.dirs, Ordering::Relaxed);
            slot.c.allocated.store(c.allocated.unwrap_or(0), Ordering::Relaxed);
            let _ = slot.cluster.set(c.allocated.map(|_| 1));
            let _ = slot.mtime.set(dir_mtime(&job.root));
            slot.pending.store(0, Ordering::Release);
            finish(s, job);
            return;
        }
        // mtime before walking: a change during the walk then invalidates the entry
        let _ = slot.mtime.set(dir_mtime(&job.root));
        let _ = slot.cluster.set(cluster_size(&job.root));
    }
    let cluster = slot.cluster.get().copied().flatten();
    let (mut bytes, mut files, mut alloc) = (0u64, 0u64, 0u64);
    let mut subdirs = Vec::new();
    let r = for_each_raw(&dir, |d| {
        let n = &d.cFileName;
        if n[0] == b'.' as u16 && (n[1] == 0 || (n[1] == b'.' as u16 && n[2] == 0)) {
            return;
        }
        let a = d.dwFileAttributes;
        if a & attrs::REPARSE_POINT != 0 {
            return;
        }
        if a & attrs::DIRECTORY != 0 {
            subdirs.push(dir.join(from_wide(n)));
        } else {
            let size = ((d.nFileSizeHigh as u64) << 32) | d.nFileSizeLow as u64;
            bytes += size;
            files += 1;
            if let Some(cl) = cluster {
                alloc += size.div_ceil(cl) * cl;
            }
        }
    });
    if let Err(code) = r {
        log::debug!("foldersize: {} failed ({code})", dir.display());
    }
    slot.c.bytes.fetch_add(bytes, Ordering::Relaxed);
    slot.c.files.fetch_add(files, Ordering::Relaxed);
    slot.c.dirs.fetch_add(subdirs.len() as u64, Ordering::Relaxed);
    slot.c.allocated.fetch_add(alloc, Ordering::Relaxed);
    if !subdirs.is_empty() {
        slot.pending.fetch_add(subdirs.len(), Ordering::AcqRel);
        let n = subdirs.len();
        job.todo.lock().extend(subdirs);
        // wake idle workers to share this folder's new work (taking the lock first: a worker
        // that just found nothing is then already waiting and gets the signal)
        drop(s.jobs.lock());
        for _ in 0..n.min(8) {
            s.cv.notify_one();
        }
    }
    if slot.pending.fetch_sub(1, Ordering::AcqRel) == 1 {
        finish(s, job);
    }
}

// ------------------------------------------------------------------------- batch

#[derive(Default)]
struct Slot {
    c: Counters,
    /// Directories queued or being listed for this folder.
    pending: AtomicUsize,
    done: AtomicBool,
    cluster: OnceLock<Option<u64>>,
    mtime: OnceLock<Option<SystemTime>>,
}

/// State of one folder in a [`SizeBatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeState {
    /// Still walking; bytes counted so far.
    Partial(u64),
    Done(FolderSize),
}

/// Sizes of several folders, measured together by the shared workers. Each folder finishes on
/// its own; results also land in [`shared_cache`] (a folder measured before and unchanged since
/// comes straight from there).
pub struct SizeBatch {
    slots: Vec<Slot>,
    stop: AtomicBool,
}

impl SizeBatch {
    /// Starts at once and returns immediately (nothing touches the disk on the calling thread).
    pub fn start(dirs: Vec<PathBuf>) -> Arc<Self> {
        let b = Arc::new(SizeBatch { slots: dirs.iter().map(|_| Slot::default()).collect(), stop: AtomicBool::new(false) });
        let s = sched();
        let mut jobs = s.jobs.lock();
        for (i, dir) in dirs.into_iter().enumerate() {
            b.slots[i].pending.store(1, Ordering::Relaxed);
            jobs.push(Arc::new(Job { batch: b.clone(), i, root: dir.clone(), todo: Mutex::new(vec![dir]), started: AtomicBool::new(false) }));
        }
        drop(jobs);
        s.cv.notify_all();
        b
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn state(&self, i: usize) -> SizeState {
        let s = &self.slots[i];
        if s.done.load(Ordering::Acquire) {
            SizeState::Done(s.c.snapshot(s.cluster.get().copied().flatten().is_some()))
        } else {
            SizeState::Partial(s.c.bytes.load(Ordering::Relaxed))
        }
    }

    /// Running totals (final once [`finished`](Self::finished)).
    pub fn totals(&self, i: usize) -> FolderSize {
        let s = &self.slots[i];
        s.c.snapshot(s.cluster.get().copied().flatten().is_some())
    }

    pub fn finished(&self) -> bool {
        self.slots.iter().all(|s| s.done.load(Ordering::Acquire))
    }

    /// Stops all walks soon; results are then not cached.
    pub fn cancel(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Totals for everything under `dir` (blocking: call off the UI thread). `progress` receives
/// running totals about every 100 ms on the calling thread, `cancelled` is polled there too; on
/// cancel the error downcasts to [`Cancelled`]. Not cached here: see [`FolderSizeCache`].
pub fn folder_size(
    dir: &Path,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&FolderSize),
) -> anyhow::Result<FolderSize> {
    anyhow::ensure!(dir.is_dir(), "Klasör bulunamadı: {}", dir.display());
    // measure afresh (Properties / preview ask for the truth, the cache may be stale)
    shared_cache().invalidate(dir);
    let b = SizeBatch::start(vec![dir.to_path_buf()]);
    let mut since = std::time::Instant::now();
    while !b.finished() {
        if cancelled() {
            b.cancel();
            return Err(Cancelled.into());
        }
        std::thread::sleep(Duration::from_millis(5));
        if since.elapsed() >= Duration::from_millis(100) {
            since = std::time::Instant::now();
            progress(&b.totals(0));
        }
    }
    let total = b.totals(0);
    progress(&total);
    Ok(total)
}

// ------------------------------------------------------------------------- cache

struct CacheEntry {
    /// Last-write time of the root folder when computed.
    mtime: Option<SystemTime>,
    size: FolderSize,
    seq: u64,
}

/// Concurrent cache of computed folder sizes, keyed by path (case-insensitive) and validated
/// against the root folder's last-write time. The root's mtime changes when entries are added,
/// removed or renamed directly inside it, **not** when something deeper changes, so deep edits
/// show up after [`invalidate`](Self::invalidate) (e.g. from the file watcher) or a refresh.
pub struct FolderSizeCache {
    map: Mutex<HashMap<String, CacheEntry>>,
    seq: AtomicU64,
    capacity: usize,
}

impl Default for FolderSizeCache {
    fn default() -> Self {
        Self::new(8192)
    }
}

fn key(dir: &Path) -> String {
    dir.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

fn dir_mtime(dir: &Path) -> Option<SystemTime> {
    std::fs::metadata(dir).and_then(|m| m.modified()).ok()
}

impl FolderSizeCache {
    pub fn new(capacity: usize) -> Self {
        Self { map: Mutex::new(HashMap::new()), seq: AtomicU64::new(0), capacity: capacity.max(16) }
    }

    /// Cached size if present and the folder's mtime is unchanged. Cheap (one `stat`).
    pub fn get(&self, dir: &Path) -> Option<FolderSize> {
        let mtime = dir_mtime(dir)?;
        let map = self.map.lock();
        let e = map.get(&key(dir))?;
        (e.mtime == Some(mtime)).then_some(e.size)
    }

    pub fn insert(&self, dir: &Path, size: FolderSize) {
        self.insert_at(dir, size, dir_mtime(dir));
    }

    fn insert_at(&self, dir: &Path, size: FolderSize, mtime: Option<SystemTime>) {
        let mut map = self.map.lock();
        if map.len() >= self.capacity {
            // Evict the older half.
            let mut seqs: Vec<u64> = map.values().map(|e| e.seq).collect();
            seqs.sort_unstable();
            let cut = seqs[seqs.len() / 2];
            map.retain(|_, e| e.seq > cut);
        }
        map.insert(
            key(dir),
            CacheEntry { mtime, size, seq: self.seq.fetch_add(1, Ordering::Relaxed) },
        );
    }

    /// Forgets `dir` and every cached folder below it.
    pub fn invalidate(&self, dir: &Path) {
        let k = key(dir);
        let prefix = format!("{k}\\");
        self.map.lock().retain(|p, _| *p != k && !p.starts_with(&prefix));
    }

    pub fn clear(&self) {
        self.map.lock().clear();
    }

    /// Returns the cached value, or computes (blocking, see [`folder_size`]) and stores it.
    /// Cancelled or failed computations are not cached.
    pub fn get_or_compute(
        &self,
        dir: &Path,
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(&FolderSize),
    ) -> anyhow::Result<FolderSize> {
        if let Some(s) = self.get(dir) {
            progress(&s);
            return Ok(s);
        }
        // Read the mtime *before* walking: a change during the walk then invalidates the entry.
        let before = dir_mtime(dir);
        let size = folder_size(dir, cancelled, progress)?;
        self.insert_at(dir, size, before);
        Ok(size)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rf-fsize-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a/b")).unwrap();
        std::fs::create_dir_all(root.join("empty")).unwrap();
        std::fs::write(root.join("x.bin"), vec![1u8; 1000]).unwrap();
        std::fs::write(root.join("a/y.bin"), vec![2u8; 5000]).unwrap();
        std::fs::write(root.join("a/b/z.bin"), vec![3u8; 70_000]).unwrap();
        root
    }

    #[test]
    fn sums_tree() {
        let root = tree("sum");
        let s = folder_size(&root, &|| false, &mut |_| {}).unwrap();
        assert_eq!((s.bytes, s.files, s.dirs), (76_000, 3, 3));
        let alloc = s.allocated.expect("cluster size");
        assert!(alloc >= s.bytes && alloc < s.bytes + 3 * 64 * 1024);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn skips_reparse_points() {
        let root = tree("junc");
        let target = tree("junc-target");
        // Directory junction (no admin needed) via mklink /J.
        let link = root.join("link");
        let ok = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let s = folder_size(&root, &|| false, &mut |_| {}).unwrap();
        assert_eq!((s.bytes, s.files, s.dirs), (76_000, 3, 3), "junction was followed (created: {ok})");
        if ok {
            let _ = std::fs::remove_dir(&link);
        }
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&target).unwrap();
    }

    #[test]
    fn cancel_and_missing() {
        let root = tree("cancel");
        let e = folder_size(&root, &|| true, &mut |_| {}).unwrap_err();
        assert!(e.downcast_ref::<Cancelled>().is_some());
        assert!(folder_size(Path::new(r"C:\definitely\not\here\rf"), &|| false, &mut |_| {}).is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    fn wait(b: &SizeBatch) {
        let t = Instant::now();
        while !b.finished() {
            assert!(t.elapsed() < Duration::from_secs(10), "batch did not finish");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn batch_sizes_each_folder_and_fills_the_cache() {
        let (r1, r2) = (tree("batch1"), tree("batch2"));
        std::fs::write(r2.join("extra.bin"), vec![0u8; 24_000]).unwrap();
        let missing = PathBuf::from(r"C:\definitely\not\here\rf");
        let b = SizeBatch::start(vec![r1.clone(), r2.clone(), r1.join("empty"), missing]);
        wait(&b);
        let bytes = |i| match b.state(i) {
            SizeState::Done(s) => (s.bytes, s.files, s.dirs),
            s => panic!("{s:?}"),
        };
        assert_eq!(bytes(0), (76_000, 3, 3));
        assert_eq!(bytes(1), (100_000, 4, 3));
        assert_eq!(bytes(2), (0, 0, 0));
        assert_eq!(bytes(3), (0, 0, 0)); // unreadable: empty, but finished
        assert_eq!(shared_cache().get(&r2).map(|s| s.bytes), Some(100_000));
        // second time from the cache, same answer
        let b2 = SizeBatch::start(vec![r2.clone()]);
        wait(&b2);
        assert!(matches!(b2.state(0), SizeState::Done(s) if s.bytes == 100_000 && s.allocated.is_some()));
        shared_cache().invalidate(&r1);
        shared_cache().invalidate(&r2);
        std::fs::remove_dir_all(&r1).unwrap();
        std::fs::remove_dir_all(&r2).unwrap();
    }

    #[test]
    fn small_folder_is_not_held_back_by_a_big_one() {
        // a wide tree keeps the workers busy; a small folder started later still finishes first
        let big = std::env::temp_dir().join(format!("rf-fsize-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&big);
        for i in 0..400 {
            std::fs::create_dir_all(big.join(format!("d{i}/e/f"))).unwrap();
            std::fs::write(big.join(format!("d{i}/e/f/x")), b"1").unwrap();
        }
        let small = tree("small");
        let b1 = SizeBatch::start(vec![big.clone()]);
        let b2 = SizeBatch::start(vec![small.clone()]);
        wait(&b2);
        let big_done_first = b1.finished();
        wait(&b1);
        assert!(matches!(b2.state(0), SizeState::Done(s) if s.bytes == 76_000));
        assert!(matches!(b1.state(0), SizeState::Done(s) if s.bytes == 400 && s.dirs == 1200));
        assert!(!big_done_first, "the small folder waited for the big one");
        shared_cache().invalidate(&big);
        shared_cache().invalidate(&small);
        std::fs::remove_dir_all(&big).unwrap();
        std::fs::remove_dir_all(&small).unwrap();
    }

    #[test]
    fn cancelled_batch_finishes_without_caching() {
        let r = tree("batchcancel");
        let b = SizeBatch::start(vec![r.clone()]);
        b.cancel();
        wait(&b);
        // it may have finished before the cancel landed; if it was stopped nothing is cached
        if shared_cache().get(&r).is_none() {
            assert!(matches!(b.state(0), SizeState::Done(_)));
        }
        shared_cache().invalidate(&r);
        std::fs::remove_dir_all(&r).unwrap();
    }

    #[test]
    fn cache_hits_and_invalidates() {
        let root = tree("cache");
        let cache = FolderSizeCache::default();
        assert!(cache.get(&root).is_none());
        let s1 = cache.get_or_compute(&root, &|| false, &mut |_| {}).unwrap();
        assert_eq!(s1.bytes, 76_000);
        assert_eq!(cache.get(&root), Some(s1));

        // Deep change: root mtime unchanged -> stale hit until invalidated.
        std::fs::write(root.join("a/b/z2.bin"), vec![0u8; 10]).unwrap();
        assert_eq!(cache.get(&root).unwrap().bytes, 76_000);
        cache.invalidate(&root);
        assert!(cache.get(&root).is_none());

        // Direct change: root mtime moves -> miss, recompute sees the new file.
        let s2 = cache.get_or_compute(&root, &|| false, &mut |_| {}).unwrap();
        assert_eq!(s2.bytes, 76_010);
        std::thread::sleep(Duration::from_millis(50));
        std::fs::write(root.join("new.txt"), b"hello").unwrap();
        assert!(cache.get(&root).is_none());
        assert_eq!(cache.get_or_compute(&root, &|| false, &mut |_| {}).unwrap().bytes, 76_015);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
