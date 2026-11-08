//! Our own copy / move engine: many files at once with `CopyFile2` (attributes, times and
//! alternate data streams come along), instead of the shell engine's one-file-at-a-time
//! pipeline. Names that already exist at the destination are ours to handle too (like Files):
//! folders merge, and every clashing file is put to the user once, in one list (replace, skip
//! or keep both).
//!
//! * Moves on one volume are renames (instant); onto another volume a copy, after which each
//!   source file is deleted only once its copy checks out.
//! * "Replace" copies to a temporary name first and swaps it in at the end, so a failed or
//!   cancelled copy never costs the file that was there.
//! * Small files are spread over folders: threads writing into the same folder wait on its
//!   lock (20 000 files: ~3 400/s in folder order, ~6 500/s spread).
//! * Pause, a speed limit and byte-accurate progress run in CopyFile2's progress callback.
//!
//! Left to the shell engine, so behavior stays Explorer's where it matters: items containing
//! links / junctions, a folder into itself, paths that aren't plain file system paths, and
//! files that fail here (access denied, in use, …) — retried through the shell at the end,
//! which can ask for elevation.
//!
//! Blocking: call from a worker thread.

use std::cell::Cell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime};

use windows::Win32::Storage::FileSystem::{
    COPY_FILE_FAIL_IF_EXISTS, COPY_FILE_NO_BUFFERING, COPYFILE2_CALLBACK_CHUNK_FINISHED, COPYFILE2_EXTENDED_PARAMETERS, COPYFILE2_MESSAGE,
    COPYFILE2_MESSAGE_ACTION, COPYFILE2_PROGRESS_CANCEL, COPYFILE2_PROGRESS_CONTINUE, CopyFile2, FILE_ATTRIBUTE_READONLY, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, GetFileAttributesW, MOVE_FILE_FLAGS, MOVEFILE_REPLACE_EXISTING, MoveFileExW, SetFileAttributesW,
};
use windows::core::PCWSTR;

use crate::fileops::{Tracker, Transfer};
use crate::wide::long_path;

/// Files above this size are copied unbuffered (no point filling the cache with them).
const UNBUFFERED: u64 = 256 << 20;
/// Files above this size are started first (they set the pace; small ones fill the gaps).
const BIG: u64 = 8 << 20;
/// A rename counts like this many bytes of progress.
const RENAME_WEIGHT: u64 = 64 << 10;

/// How copies are checked after writing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Verify {
    #[default]
    Off,
    /// Same size and modification time.
    Quick,
    /// Same bytes, read back from the disk (not from the cache).
    Full,
}

/// A name that already exists at the destination.
#[derive(Clone, Debug)]
pub struct Conflict {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub src_dir: bool,
    pub dst_dir: bool,
    pub src_size: u64,
    pub dst_size: u64,
    pub src_time: Option<SystemTime>,
    pub dst_time: Option<SystemTime>,
}

impl Conflict {
    /// A file against a folder (or the other way round): it can't replace, only skip or keep both.
    pub fn mismatched(&self) -> bool {
        self.src_dir != self.dst_dir
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resolve {
    Replace,
    Skip,
    KeepBoth,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Op {
    Copy,
    Rename,
}

/// One file to copy, or one item to rename into place.
#[derive(Clone, Debug)]
struct Job {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
    op: Op,
    /// The destination exists and is to be replaced.
    replace: bool,
}

/// What a transfer will do, worked out before anything is written.
#[derive(Default)]
struct Plan {
    /// Folders to create (copies), parents first.
    dirs: Vec<PathBuf>,
    jobs: Vec<Job>,
    conflicts: Vec<(Conflict, Job)>,
    /// (source, destination) of every item this transfer creates, for undo.
    undo: Vec<(PathBuf, PathBuf)>,
    /// (source folder, its new copy): attributes are copied once the contents are in.
    new_dirs: Vec<(PathBuf, PathBuf)>,
    /// Source folders whose contents are taken file by file (moves remove them when empty).
    src_dirs: Vec<PathBuf>,
    /// Top-level sources for the shell engine.
    whole: Vec<PathBuf>,
    /// Destination paths this plan writes (lowercase), so "keep both" picks a free name.
    taken: HashSet<String>,
}

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

fn is_reparse(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

/// A link or junction somewhere in the tree: the shell copes with those.
struct HasLink;

impl Plan {
    /// Plans one top-level source; on a link inside it, nothing of it is kept and it goes to
    /// the shell whole.
    fn top(&mut self, src: &Path, dest_dir: &Path, rename: bool) {
        let Some(name) = src.file_name() else {
            self.whole.push(src.to_path_buf());
            return;
        };
        let dst = dest_dir.join(name);
        let meta = match std::fs::symlink_metadata(src) {
            Ok(m) if !is_reparse(&m) => m,
            _ => {
                self.whole.push(src.to_path_buf());
                return;
            }
        };
        // a folder into itself or below itself: the shell refuses it with its own message
        if meta.is_dir() && inside(dest_dir, src) {
            self.whole.push(src.to_path_buf());
            return;
        }
        let mut sub = Plan { taken: std::mem::take(&mut self.taken), ..Default::default() };
        match sub.visit(src, &dst, &meta, rename) {
            Ok(()) => {
                self.taken = std::mem::take(&mut sub.taken);
                self.dirs.extend(sub.dirs);
                self.jobs.extend(sub.jobs);
                self.conflicts.extend(sub.conflicts);
                self.undo.extend(sub.undo);
                self.new_dirs.extend(sub.new_dirs);
                self.src_dirs.extend(sub.src_dirs);
            }
            Err(HasLink) => {
                // drop what it reserved
                let mut taken = std::mem::take(&mut sub.taken);
                for j in sub.jobs.iter().chain(sub.conflicts.iter().map(|c| &c.1)) {
                    taken.remove(&key(&j.dst));
                }
                for d in &sub.dirs {
                    taken.remove(&key(d));
                }
                self.taken = taken;
                self.whole.push(src.to_path_buf());
            }
        }
    }

    fn visit(&mut self, src: &Path, dst: &Path, meta: &std::fs::Metadata, rename: bool) -> Result<(), HasLink> {
        // already planned by another source of this transfer (same name from two folders)
        if self.taken.contains(&key(dst)) {
            let free = self.free_name(dst, meta.is_dir());
            return self.visit(src, &free, meta, rename);
        }
        let there = std::fs::symlink_metadata(dst).ok();
        match there {
            None if rename => {
                self.taken.insert(key(dst));
                self.jobs.push(Job { src: src.into(), dst: dst.into(), size: RENAME_WEIGHT, op: Op::Rename, replace: false });
                self.undo.push((src.into(), dst.into()));
            }
            None if meta.is_dir() => {
                self.new_tree(src, dst)?;
                self.undo.push((src.into(), dst.into()));
            }
            None => {
                self.taken.insert(key(dst));
                self.jobs.push(Job { src: src.into(), dst: dst.into(), size: meta.len(), op: Op::Copy, replace: false });
                self.undo.push((src.into(), dst.into()));
            }
            // folder onto folder: merge, item by item
            Some(d) if meta.is_dir() && d.is_dir() && !is_reparse(&d) => {
                self.src_dirs.push(src.into());
                let rd = std::fs::read_dir(src).map_err(|_| HasLink)?;
                for e in rd.flatten() {
                    let m = std::fs::symlink_metadata(e.path()).map_err(|_| HasLink)?;
                    if is_reparse(&m) {
                        return Err(HasLink);
                    }
                    self.visit(&e.path(), &dst.join(e.file_name()), &m, rename)?;
                }
            }
            Some(d) => {
                use std::os::windows::fs::MetadataExt;
                let c = Conflict {
                    src: src.into(),
                    dst: dst.into(),
                    src_dir: meta.is_dir(),
                    dst_dir: d.is_dir(),
                    src_size: if meta.is_dir() { 0 } else { meta.len() },
                    dst_size: if d.is_dir() { 0 } else { d.file_size() },
                    src_time: meta.modified().ok(),
                    dst_time: d.modified().ok(),
                };
                if meta.is_dir() {
                    // walked now, so a link inside is found before anything is asked
                    check_tree(src)?;
                }
                let size = if rename { RENAME_WEIGHT } else { meta.len() };
                self.conflicts.push((c, Job { src: src.into(), dst: dst.into(), size, op: if rename { Op::Rename } else { Op::Copy }, replace: false }));
            }
        }
        Ok(())
    }

    /// A whole new folder: its folders and files (copies).
    fn new_tree(&mut self, src: &Path, dst: &Path) -> Result<(), HasLink> {
        self.taken.insert(key(dst));
        self.dirs.push(dst.into());
        self.new_dirs.push((src.into(), dst.into()));
        let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
        while let Some((from, to)) = stack.pop() {
            let rd = std::fs::read_dir(&from).map_err(|_| HasLink)?;
            for e in rd.flatten() {
                let m = e.metadata().map_err(|_| HasLink)?;
                if is_reparse(&m) {
                    return Err(HasLink);
                }
                let (a, b) = (e.path(), to.join(e.file_name()));
                if m.is_dir() {
                    self.dirs.push(b.clone());
                    self.new_dirs.push((a.clone(), b.clone()));
                    stack.push((a, b));
                } else {
                    self.jobs.push(Job { src: a, dst: b, size: m.len(), op: Op::Copy, replace: false });
                }
            }
        }
        Ok(())
    }

    /// Applies the user's answers to the conflicts.
    fn resolve(&mut self, answers: &[Resolve]) {
        let conflicts = std::mem::take(&mut self.conflicts);
        for ((c, mut job), r) in conflicts.into_iter().zip(answers.iter().copied()) {
            let r = if r == Resolve::Replace && c.mismatched() { Resolve::KeepBoth } else { r };
            match r {
                Resolve::Skip => {}
                Resolve::Replace => {
                    job.replace = true;
                    self.undo.push((job.src.clone(), job.dst.clone()));
                    self.jobs.push(job);
                }
                Resolve::KeepBoth => {
                    let dst = self.free_name(&c.dst, c.src_dir);
                    if job.op == Op::Rename || !c.src_dir {
                        self.taken.insert(key(&dst));
                        self.undo.push((job.src.clone(), dst.clone()));
                        self.jobs.push(Job { dst, ..job });
                    } else if self.new_tree(&c.src, &dst).is_ok() {
                        self.undo.push((c.src.clone(), dst));
                    }
                }
            }
        }
    }

    /// "name (2).ext", "name (3).ext", … : the first that neither exists nor is planned.
    fn free_name(&self, dst: &Path, is_dir: bool) -> PathBuf {
        let dir = dst.parent().unwrap_or(Path::new(""));
        let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 && !is_dir => (name[..i].to_string(), name[i..].to_string()),
            _ => (name.clone(), String::new()),
        };
        (2..)
            .map(|n| dir.join(format!("{stem} ({n}){ext}")))
            .find(|p| !self.taken.contains(&key(p)) && std::fs::symlink_metadata(p).is_err())
            .expect("a free name")
    }
}

/// Errors if there is a link or junction anywhere under `dir`.
fn check_tree(dir: &Path) -> Result<(), HasLink> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|_| HasLink)?.flatten() {
            let m = e.metadata().map_err(|_| HasLink)?;
            if is_reparse(&m) {
                return Err(HasLink);
            }
            if m.is_dir() {
                stack.push(e.path());
            }
        }
    }
    Ok(())
}

/// Plans `sources` into `dest_dir`: what goes where, and what clashes.
fn plan(sources: &[PathBuf], dest_dir: &Path, rename: bool) -> Plan {
    let mut p = Plan::default();
    for s in sources {
        p.top(s, dest_dir, rename);
    }
    p
}

/// Big files first, then the rest taken from each folder in turn.
fn order(jobs: Vec<Job>) -> Vec<Job> {
    let (mut big, small): (Vec<Job>, Vec<Job>) = jobs.into_iter().partition(|j| j.size >= BIG);
    big.sort_by_key(|j| std::cmp::Reverse(j.size));
    let mut folders: Vec<std::collections::VecDeque<Job>> = Vec::new();
    let mut at: std::collections::HashMap<PathBuf, usize> = Default::default();
    for j in small {
        let parent = j.dst.parent().map(Path::to_path_buf).unwrap_or_default();
        let ix = *at.entry(parent).or_insert_with(|| {
            folders.push(Default::default());
            folders.len() - 1
        });
        folders[ix].push_back(j);
    }
    let mut out = big;
    loop {
        let before = out.len();
        for q in folders.iter_mut() {
            if let Some(j) = q.pop_front() {
                out.push(j);
            }
        }
        if out.len() == before {
            break;
        }
    }
    out
}

/// Progress, pause, speed limit and cancel, shared by the copy threads.
struct Run<'a> {
    tracker: &'a Tracker,
    total: u64,
    done: AtomicU64,
    /// Turned non-zero to stop CopyFile2 (it removes the partial file).
    cancel: AtomicI32,
    /// Speed limit clock: (since, bytes counted since then).
    clock: Mutex<(Instant, u64)>,
    last_report: Mutex<Instant>,
}

impl Run<'_> {
    fn cancelled(&self) -> bool {
        if self.tracker.cancel.load(Ordering::Relaxed) {
            self.cancel.store(1, Ordering::Relaxed);
        }
        self.cancel.load(Ordering::Relaxed) != 0
    }

    /// Blocks while paused; the paused time doesn't count against the speed limit.
    fn wait_if_paused(&self) {
        if !self.tracker.paused.load(Ordering::Relaxed) {
            return;
        }
        let from = Instant::now();
        while self.tracker.paused.load(Ordering::Relaxed) && !self.cancelled() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut c = self.clock.lock().unwrap();
        c.0 += from.elapsed();
    }

    fn add(&self, bytes: u64, name: Option<&Path>) {
        self.done.fetch_add(bytes, Ordering::Relaxed);
        let limit = self.tracker.limit.load(Ordering::Relaxed);
        if limit > 0 {
            let wait = {
                let mut c = self.clock.lock().unwrap();
                c.1 += bytes;
                let due = Duration::from_secs_f64(c.1 as f64 / limit as f64);
                due.saturating_sub(c.0.elapsed())
            };
            if !wait.is_zero() {
                std::thread::sleep(wait.min(Duration::from_millis(500)));
            }
        }
        let mut last = self.last_report.lock().unwrap();
        if last.elapsed() >= Duration::from_millis(50) {
            *last = Instant::now();
            drop(last);
            self.report(name);
        }
    }

    fn report(&self, name: Option<&Path>) {
        let d = self.done.load(Ordering::Relaxed).min(self.total);
        let permille = (d as u128 * 1000 / self.total.max(1) as u128) as u32;
        (self.tracker.on_progress)(1000, permille, name.and_then(|n| n.file_name()).map(|n| n.to_string_lossy().into_owned()));
    }
}

/// CopyFile2's per-file callback context.
struct FileCtx<'a> {
    run: &'a Run<'a>,
    name: &'a Path,
    sent: Cell<u64>,
}

unsafe extern "system" fn on_chunk(msg: *const COPYFILE2_MESSAGE, ctx: *const core::ffi::c_void) -> COPYFILE2_MESSAGE_ACTION {
    // SAFETY: CopyFile2 calls back on the copying thread while `ctx` (a FileCtx on that
    // thread's stack) is alive.
    let (msg, ctx) = unsafe { (&*msg, &*(ctx as *const FileCtx)) };
    if msg.Type == COPYFILE2_CALLBACK_CHUNK_FINISHED {
        let total = unsafe { msg.Info.ChunkFinished.uliTotalBytesTransferred };
        let delta = total.saturating_sub(ctx.sent.replace(total));
        ctx.run.add(delta, Some(ctx.name));
    }
    ctx.run.wait_if_paused();
    if ctx.run.cancelled() { COPYFILE2_PROGRESS_CANCEL } else { COPYFILE2_PROGRESS_CONTINUE }
}

fn copy_file(src: &Path, dst: &Path, size: u64, run: &Run) -> windows::core::Result<()> {
    let (s, d) = (long_path(src), long_path(dst));
    let mut flags = COPY_FILE_FAIL_IF_EXISTS.0;
    if size >= UNBUFFERED {
        flags |= COPY_FILE_NO_BUFFERING.0;
    }
    let ctx = FileCtx { run, name: src, sent: Cell::new(0) };
    let params = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: std::mem::size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: windows::Win32::Storage::FileSystem::COPYFILE_FLAGS(flags),
        pfCancel: run.cancel.as_ptr() as *mut windows_core::BOOL,
        pProgressRoutine: Some(on_chunk),
        pvCallbackContext: &ctx as *const FileCtx as *mut core::ffi::c_void,
    };
    let r = unsafe { CopyFile2(PCWSTR(s.as_ptr()), PCWSTR(d.as_ptr()), Some(&params)) };
    // whatever the callback didn't count (an empty file reports no chunk)
    run.add(size.max(1).saturating_sub(ctx.sent.get()), None);
    r
}

/// Moves `from` over `to` (replacing it when asked), clearing a read-only flag in the way.
fn move_into_place(from: &Path, to: &Path, replace: bool) -> windows::core::Result<()> {
    let (f, t) = (long_path(from), long_path(to));
    let flags = if replace { MOVEFILE_REPLACE_EXISTING } else { MOVE_FILE_FLAGS(0) };
    let r = unsafe { MoveFileExW(PCWSTR(f.as_ptr()), PCWSTR(t.as_ptr()), flags) };
    if r.is_err() && replace {
        unsafe {
            let a = GetFileAttributesW(PCWSTR(t.as_ptr()));
            if a != u32::MAX && a & FILE_ATTRIBUTE_READONLY.0 != 0 {
                let _ = SetFileAttributesW(PCWSTR(t.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a & !FILE_ATTRIBUTE_READONLY.0));
                return MoveFileExW(PCWSTR(f.as_ptr()), PCWSTR(t.as_ptr()), flags);
            }
        }
    }
    r
}

/// A temporary name next to `dst` for a copy that replaces it.
fn part_name(dst: &Path) -> PathBuf {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dst.with_file_name(format!(".{name}.{}.rfpart", std::process::id()))
}

/// One job; `Err` = leave it to the shell.
fn run_job(j: &Job, run: &Run) -> windows::core::Result<()> {
    match j.op {
        Op::Rename => {
            let r = move_into_place(&j.src, &j.dst, j.replace);
            run.add(j.size, Some(&j.src));
            r
        }
        Op::Copy if j.replace => {
            let tmp = part_name(&j.dst);
            let _ = std::fs::remove_file(&tmp);
            copy_file(&j.src, &tmp, j.size, run)?;
            move_into_place(&tmp, &j.dst, true).inspect_err(|_| {
                let _ = std::fs::remove_file(&tmp);
            })
        }
        Op::Copy => copy_file(&j.src, &j.dst, j.size, run),
    }
}

/// What a transfer did here: undo pairs, files copied (source, copy), and what is left for
/// the shell.
#[derive(Default)]
pub struct Outcome {
    pub undo: Vec<(PathBuf, PathBuf)>,
    /// Single files that failed here: (source, destination folder).
    pub failed: Vec<(PathBuf, PathBuf)>,
    /// Top-level sources the shell should transfer whole.
    pub whole: Vec<PathBuf>,
    copied: Vec<(PathBuf, PathBuf)>,
    src_dirs: Vec<PathBuf>,
    cancelled: bool,
}

/// Plans, asks about conflicts, and runs a transfer with our engine (`rename` = a move on one
/// volume). Everything it can't do is in the returned leftovers.
fn execute(sources: &[PathBuf], dest_dir: &Path, tracker: &Tracker, threads: usize, rename: bool) -> Outcome {
    let mut p = plan(sources, dest_dir, rename);
    if !p.conflicts.is_empty() {
        let list: Vec<Conflict> = p.conflicts.iter().map(|c| c.0.clone()).collect();
        let answers = match &tracker.ask {
            Some(ask) => ask(&list),
            None => Some(vec![Resolve::KeepBoth; list.len()]),
        };
        match answers {
            Some(a) => p.resolve(&a),
            None => return Outcome { cancelled: true, ..Default::default() },
        }
    }
    for d in &p.dirs {
        if let Err(e) = std::fs::create_dir(d) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                log::warn!("fastcopy: mkdir {}: {e}", d.display());
            }
        }
    }
    let jobs = order(std::mem::take(&mut p.jobs));
    let run = Run {
        tracker,
        total: jobs.iter().map(|j| j.size.max(1)).sum::<u64>().max(1),
        done: AtomicU64::new(0),
        cancel: AtomicI32::new(0),
        clock: Mutex::new((Instant::now(), 0)),
        last_report: Mutex::new(Instant::now() - Duration::from_secs(1)),
    };
    let next = AtomicUsize::new(0);
    let failed = Mutex::new(Vec::new());
    let done = Mutex::new(Vec::new());
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, jobs.len().max(1)) {
            s.spawn(|| {
                loop {
                    run.wait_if_paused();
                    if run.cancelled() {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(j) = jobs.get(i) else { return };
                    match run_job(j, &run) {
                        Ok(()) => done.lock().unwrap().push(i),
                        Err(e) => {
                            if !run.cancelled() {
                                log::info!("fastcopy: {} → shell ({e})", j.src.display());
                                failed.lock().unwrap().push(i);
                            }
                        }
                    }
                }
            });
        }
    });
    run.report(None);
    let cancelled = run.cancelled();
    // folder attributes (hidden, read-only, …) once their contents are in
    for (src, dst) in &p.new_dirs {
        if let Ok(m) = std::fs::metadata(src) {
            use std::os::windows::fs::MetadataExt;
            let w = long_path(dst);
            unsafe {
                let _ = SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(m.file_attributes()));
            }
        }
    }
    let failed: Vec<(PathBuf, PathBuf)> = failed
        .into_inner()
        .unwrap()
        .into_iter()
        .filter_map(|i| jobs[i].dst.parent().map(|d| (jobs[i].src.clone(), d.to_path_buf())))
        .collect();
    let copied = done.into_inner().unwrap().into_iter().filter(|&i| jobs[i].op == Op::Copy).map(|i| (jobs[i].src.clone(), jobs[i].dst.clone())).collect();
    Outcome { undo: p.undo, failed, whole: p.whole, copied, src_dirs: p.src_dirs, cancelled }
}

/// Copies `sources` into `dest_dir` in parallel (conflicts as `tracker.ask` says; without it
/// both are kept). Returns the undo pairs and the leftovers for the shell.
pub fn copy(sources: &[PathBuf], dest_dir: &Path, tracker: &Tracker, threads: usize) -> Outcome {
    execute(sources, dest_dir, tracker, threads, false)
}

/// Copy or move with our engine where it can, the shell engine for the rest. Returns like
/// `fileops::transfer_tracked`; undo pairs land in `tracker.results`. A verification failure
/// is an `Err` (the copies stay).
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    // pasting where the items already are ("x - Copy"), and anything not on a plain file
    // system path: the shell's way
    let same_dir = sources.iter().all(|s| s.parent() == Some(dest_dir));
    if same_dir || !local_fs(dest_dir) || !sources.iter().all(|s| local_fs(s)) {
        return crate::fileops::transfer_tracked(kind, sources, dest_dir, tracker);
    }
    let (here, there): (Vec<PathBuf>, Vec<PathBuf>) = sources.iter().cloned().partition(|s| kind == Transfer::Move && same_volume(s, dest_dir));
    let mut ok = true;
    if !here.is_empty() {
        let out = execute(&here, dest_dir, &tracker, 4, true);
        ok &= finish(Transfer::Move, &here, dest_dir, &tracker, out)?;
    }
    if !there.is_empty() && !tracker.cancel.load(Ordering::Relaxed) {
        let threads = threads_for(&there, dest_dir);
        let out = execute(&there, dest_dir, &tracker, threads, false);
        ok &= finish(kind, &there, dest_dir, &tracker, out)?;
    }
    for s in sources {
        if let Some(p) = s.parent() {
            crate::shell::notify_updated(p);
        }
    }
    crate::shell::notify_updated(dest_dir);
    Ok(ok && !tracker.cancel.load(Ordering::Relaxed))
}

/// After our engine: the shell for the leftovers, verification, and for moves the removal of
/// what was moved.
fn finish(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: &Arc<Tracker>, out: Outcome) -> anyhow::Result<bool> {
    tracker.results.lock().unwrap().extend(out.undo.iter().cloned());
    if out.cancelled || tracker.cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let mut ok = true;
    if !out.whole.is_empty() {
        ok &= crate::fileops::transfer_tracked(kind, &out.whole, dest_dir, tracker.clone())?;
    }
    // failed single files: one shell call per destination folder
    let mut by_dir: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for (f, d) in &out.failed {
        match by_dir.iter_mut().find(|(x, _)| x == d) {
            Some((_, v)) => v.push(f.clone()),
            None => by_dir.push((d.clone(), vec![f.clone()])),
        }
    }
    for (d, fs) in by_dir {
        // the shell reports its own results; those files' undo is covered by their folder
        let t = Arc::new(Tracker::new(Box::new(|_, _, _| {}), tracker.cancel.clone()));
        ok &= crate::fileops::transfer_tracked(kind, &fs, &d, t)?;
    }
    // checks: asked for, or a move about to delete its sources
    let mode = match (tracker.verify_mode(), kind) {
        (Verify::Off, Transfer::Move) => Verify::Quick,
        (m, _) => m,
    };
    let bad = check(&out.copied, mode, tracker);
    if tracker.cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    if kind == Transfer::Move {
        // only sources whose copy checked out go; skipped and failed ones stay
        let bad_set: HashSet<&PathBuf> = bad.iter().collect();
        let files: Vec<PathBuf> = out.copied.iter().filter(|(s, _)| !bad_set.contains(s)).map(|(s, _)| s.clone()).collect();
        // folders emptied by the move: merged ones, and whole ones copied across volumes
        let mut dirs: Vec<PathBuf> = out.src_dirs.clone();
        for (s, _) in &out.undo {
            if s.is_dir() && sources.iter().any(|t| s.starts_with(t)) {
                walk_dirs(s, &mut dirs);
            }
        }
        dirs.sort_by_key(|d| d.components().count());
        dirs.dedup();
        if !crate::fastdelete::remove_verified(&files, &dirs) {
            log::info!("fastcopy: some sources stay (skipped, failed or changed meanwhile)");
        }
    }
    if !bad.is_empty() {
        let names: Vec<String> = bad.iter().take(5).filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
        anyhow::bail!("{}: {} ({})", NOT_VERIFIED, bad.len(), names.join(", "));
    }
    Ok(ok)
}

/// Error text for copies that didn't verify (translated by the app).
pub const NOT_VERIFIED: &str = "Doğrulanamayan dosyalar";
/// Shown while copies are read back.
pub const VERIFYING: &str = "Doğrulanıyor…";

/// Every folder under `dir` (and `dir`).
fn walk_dirs(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            for e in rd.flatten() {
                if e.file_type().is_ok_and(|t| t.is_dir()) {
                    stack.push(e.path());
                }
            }
        }
        out.push(d);
    }
}

/// Sources whose copy doesn't match.
fn check(pairs: &[(PathBuf, PathBuf)], mode: Verify, tracker: &Tracker) -> Vec<PathBuf> {
    if mode == Verify::Off || pairs.is_empty() {
        return Vec::new();
    }
    if mode == Verify::Full {
        (tracker.on_progress)(1000, 0, Some(VERIFYING.to_string()));
    }
    let next = AtomicUsize::new(0);
    let bad = Mutex::new(Vec::new());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 8);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                loop {
                    if tracker.cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((a, b)) = pairs.get(i) else { return };
                    let same = match mode {
                        Verify::Quick => same_meta(a, b),
                        _ => same_meta(a, b) && same_bytes(a, b).unwrap_or(false),
                    };
                    if !same {
                        log::warn!("fastcopy: {} does not match {}", b.display(), a.display());
                        bad.lock().unwrap().push(a.clone());
                    }
                    if mode == Verify::Full && i % 16 == 0 {
                        (tracker.on_progress)(1000, (i * 1000 / pairs.len()) as u32, None);
                    }
                }
            });
        }
    });
    bad.into_inner().unwrap()
}

/// Same size and modification time (CopyFile2 keeps it; 2 s of slack for FAT/exFAT sticks).
fn same_meta(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if ma.len() != mb.len() {
        return false;
    }
    match (ma.modified(), mb.modified()) {
        (Ok(x), Ok(y)) => x.duration_since(y).or_else(|_| y.duration_since(x)).is_ok_and(|d| d <= Duration::from_secs(2)),
        _ => false,
    }
}

/// Same bytes; the copy is read past the cache (what the disk holds, not what was just
/// written to memory).
fn same_bytes(a: &Path, b: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    const CHUNK: usize = 1 << 20;
    const FILE_FLAG_NO_BUFFERING: u32 = 0x2000_0000;
    const FILE_FLAG_SEQUENTIAL_SCAN: u32 = 0x0800_0000;
    struct Aligned(*mut u8, std::alloc::Layout);
    impl Drop for Aligned {
        fn drop(&mut self) {
            unsafe { std::alloc::dealloc(self.0, self.1) }
        }
    }
    let mut fa = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_SEQUENTIAL_SCAN).open(a)?;
    let mut fb = std::fs::OpenOptions::new().read(true).custom_flags(FILE_FLAG_NO_BUFFERING).open(b)?;
    // unbuffered reads need a sector-aligned buffer
    let layout = std::alloc::Layout::from_size_align(CHUNK, 4096).expect("layout");
    let buf = Aligned(unsafe { std::alloc::alloc(layout) }, layout);
    if buf.0.is_null() {
        return Ok(false);
    }
    let bb = unsafe { std::slice::from_raw_parts_mut(buf.0, CHUNK) };
    let mut ba = vec![0u8; CHUNK];
    loop {
        let n = fb.read(bb)?;
        let mut m = 0;
        while m < n {
            let k = fa.read(&mut ba[m..n])?;
            if k == 0 {
                return Ok(false);
            }
            m += k;
        }
        if ba[..n] != bb[..n] {
            return Ok(false);
        }
        if n < CHUNK {
            // the copy ended: the source must too
            return Ok(fa.read(&mut ba[..1])? == 0);
        }
    }
}

/// `inner` is `outer` or inside it, comparing like Windows does (case-insensitive, either
/// separator).
fn inside(inner: &Path, outer: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    let (i, o) = (norm(inner), norm(outer));
    i == o || i.strip_prefix(&o).is_some_and(|rest| rest.starts_with('\\'))
}

/// Same drive letter or same `\\server\share`: a move there is a rename.
fn same_volume(a: &Path, b: &Path) -> bool {
    use std::path::Component;
    let root = |p: &Path| match p.components().next() {
        Some(Component::Prefix(x)) => Some(x.as_os_str().to_string_lossy().to_lowercase()),
        _ => None,
    };
    root(a).is_some() && root(a) == root(b)
}

/// Parallel copies pay off on SSDs; a spinning disk on either side only seeks more with
/// many at once, and network shares and USB sticks like a moderate number.
fn threads_for(sources: &[PathBuf], dest: &Path) -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let paths = sources.iter().map(PathBuf::as_path).chain(std::iter::once(dest));
    let mut slow = false;
    for p in paths {
        match crate::drives::seek_penalty(p) {
            Some(true) => return 2,
            Some(false) => {}
            None => slow = true,
        }
        slow |= crate::drives::is_removable(p);
    }
    if slow { 4 } else { cores.clamp(4, 16) }
}

/// Whether `p` is on a local or network file system path we can copy into directly (not a
/// shell namespace, library, phone, …).
fn local_fs(p: &Path) -> bool {
    let s = p.to_string_lossy();
    (s.len() >= 3 && s.as_bytes()[1] == b':') || s.starts_with(r"\\") && !s.starts_with(r"\\?\")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn tracker() -> Tracker {
        Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false)))
    }

    fn answering(r: Resolve) -> Tracker {
        let mut t = tracker();
        t.ask = Some(Box::new(move |c| Some(vec![r; c.len()])));
        t
    }

    fn base(name: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("rf-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(&b).unwrap();
        b
    }

    fn tree(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
        std::fs::create_dir_all(root.join("a/b/c")).unwrap();
        std::fs::create_dir_all(root.join("boş klasör")).unwrap();
        std::fs::write(root.join("kök.txt"), b"merhaba").unwrap();
        std::fs::write(root.join("a/boş.dat"), b"").unwrap();
        let big: Vec<u8> = (0..3_000_000u32).map(|i| (i * 31 % 251) as u8).collect();
        std::fs::write(root.join("a/b/büyük.bin"), &big).unwrap();
        for i in 0..300 {
            std::fs::write(root.join(format!("a/b/c/f{i}.txt")), format!("dosya {i}").repeat(i + 1)).unwrap();
        }
    }

    fn same(a: &Path, b: &Path) {
        let ma = std::fs::metadata(a).unwrap();
        let mb = std::fs::metadata(b).unwrap();
        assert_eq!(ma.is_dir(), mb.is_dir(), "{}", b.display());
        if ma.is_dir() {
            let mut na: Vec<_> = std::fs::read_dir(a).unwrap().map(|e| e.unwrap().file_name()).collect();
            let mut nb: Vec<_> = std::fs::read_dir(b).unwrap().map(|e| e.unwrap().file_name()).collect();
            na.sort();
            nb.sort();
            assert_eq!(na, nb, "{}", b.display());
            for n in na {
                same(&a.join(&n), &b.join(&n));
            }
        } else {
            assert_eq!(std::fs::read(a).unwrap(), std::fs::read(b).unwrap(), "{}", b.display());
            assert_eq!(ma.modified().unwrap(), mb.modified().unwrap(), "{}", b.display());
        }
    }

    fn same_bytes_tree(a: &Path, b: &Path) {
        let mut stack = vec![(a.to_path_buf(), b.to_path_buf())];
        while let Some((x, y)) = stack.pop() {
            for e in std::fs::read_dir(&x).unwrap() {
                let e = e.unwrap();
                let (p, q) = (e.path(), y.join(e.file_name()));
                if p.is_dir() {
                    stack.push((p, q));
                } else {
                    assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(&q).unwrap(), "{}", q.display());
                }
            }
        }
    }

    #[test]
    fn copies_a_tree_exactly() {
        let base = base("fastcopy");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        tree(&src);
        std::fs::create_dir_all(&dst).unwrap();
        let t = tracker();
        let out = copy(&[src.clone(), src.join("kök.txt")], &dst, &t, 8);
        assert!(out.whole.is_empty() && out.failed.is_empty());
        assert_eq!(out.undo.len(), 2);
        same(&src, &dst.join("kaynak"));
        same(&src.join("kök.txt"), &dst.join("kök.txt"));
        // again into the same place, nobody asked: the folder merges, every file gets a twin
        let out = copy(&[src.clone()], &dst, &t, 8);
        assert!(out.whole.is_empty());
        assert!(dst.join("kaynak/kök (2).txt").exists());
        assert!(dst.join("kaynak/a/b/c/f7 (2).txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn conflicts_replace_skip_keep_both() {
        let base = base("conflicts");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        std::fs::create_dir_all(src.join("alt")).unwrap();
        std::fs::create_dir_all(dst.join("kaynak/alt")).unwrap();
        std::fs::write(src.join("alt/aynı.txt"), b"yeni").unwrap();
        std::fs::write(src.join("alt/yeni.txt"), b"sadece kaynakta").unwrap();
        std::fs::write(dst.join("kaynak/alt/aynı.txt"), b"eski").unwrap();
        std::fs::write(dst.join("kaynak/alt/sadece hedefte.txt"), "kalır".as_bytes()).unwrap();
        let asked = Arc::new(Mutex::new(Vec::new()));
        // skip: the old file stays, the new one arrives, the merge keeps the rest
        let mut t = tracker();
        let a2 = asked.clone();
        t.ask = Some(Box::new(move |c| {
            a2.lock().unwrap().extend(c.iter().map(|c| c.dst.clone()));
            Some(vec![Resolve::Skip; c.len()])
        }));
        transfer(Transfer::Copy, &[src.clone()], &dst, Arc::new(t)).unwrap();
        assert_eq!(*asked.lock().unwrap(), vec![dst.join("kaynak/alt/aynı.txt")], "only the clashing file is asked");
        assert_eq!(std::fs::read(dst.join("kaynak/alt/aynı.txt")).unwrap(), b"eski");
        assert_eq!(std::fs::read(dst.join("kaynak/alt/yeni.txt")).unwrap(), b"sadece kaynakta");
        assert!(dst.join("kaynak/alt/sadece hedefte.txt").exists());
        // keep both
        transfer(Transfer::Copy, &[src.clone()], &dst, Arc::new(answering(Resolve::KeepBoth))).unwrap();
        assert_eq!(std::fs::read(dst.join("kaynak/alt/aynı (2).txt")).unwrap(), b"yeni");
        assert_eq!(std::fs::read(dst.join("kaynak/alt/aynı.txt")).unwrap(), b"eski");
        // replace (a read-only target included), no temporary file left behind
        let target = dst.join("kaynak/alt/aynı.txt");
        let mut perm = std::fs::metadata(&target).unwrap().permissions();
        perm.set_readonly(true);
        std::fs::set_permissions(&target, perm).unwrap();
        transfer(Transfer::Copy, &[src.clone()], &dst, Arc::new(answering(Resolve::Replace))).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"yeni");
        let leftovers = std::fs::read_dir(dst.join("kaynak/alt")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".rfpart")).count();
        assert_eq!(leftovers, 0);
        // cancel at the question: nothing written
        let mut t = tracker();
        t.ask = Some(Box::new(|_| None));
        std::fs::write(src.join("alt/başka.txt"), b"x").unwrap();
        assert!(!transfer(Transfer::Copy, &[src.clone()], &dst, Arc::new(t)).unwrap());
        assert!(!dst.join("kaynak/alt/başka.txt").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn keep_both_never_takes_a_planned_name() {
        let base = base("freename");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&dst).unwrap();
        // "a (2).txt" is copied in this same run; the clashing "a.txt" must become "a (3).txt"
        std::fs::write(src.join("a.txt"), b"yeni a").unwrap();
        std::fs::write(src.join("a (2).txt"), b"kaynaktaki a (2)").unwrap();
        std::fs::write(dst.join("a.txt"), b"eski a").unwrap();
        transfer(Transfer::Copy, &[src.join("a.txt"), src.join("a (2).txt")], &dst, Arc::new(answering(Resolve::KeepBoth))).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"eski a");
        assert_eq!(std::fs::read(dst.join("a (2).txt")).unwrap(), b"kaynaktaki a (2)");
        assert_eq!(std::fs::read(dst.join("a (3).txt")).unwrap(), b"yeni a");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn moves_on_one_volume_rename_and_merge() {
        let base = base("rename");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        tree(&src);
        std::fs::create_dir_all(dst.join("kaynak/a")).unwrap();
        std::fs::write(dst.join("kaynak/kök.txt"), b"eski").unwrap();
        std::fs::write(dst.join("kaynak/a/hedefte.txt"), "kalır".as_bytes()).unwrap();
        let t = Arc::new(answering(Resolve::Skip));
        assert!(transfer(Transfer::Move, &[src.clone()], &dst, t.clone()).unwrap());
        // merged: the clash skipped (stays in the source), the rest moved
        assert_eq!(std::fs::read(dst.join("kaynak/kök.txt")).unwrap(), b"eski");
        assert_eq!(std::fs::read(src.join("kök.txt")).unwrap(), b"merhaba");
        assert!(dst.join("kaynak/a/b/c/f299.txt").exists());
        assert!(dst.join("kaynak/a/hedefte.txt").exists());
        assert!(!src.join("a").exists(), "emptied folders go");
        assert!(src.exists(), "the source keeps the skipped file");
        // undo pairs name what was moved, never the merged folder itself
        let res = t.results.lock().unwrap().clone();
        assert!(!res.iter().any(|(_, d)| *d == dst.join("kaynak")), "{res:?}");
        // a plain rename of a whole folder
        let lone = base.join("tek");
        std::fs::create_dir_all(lone.join("iç")).unwrap();
        assert!(transfer(Transfer::Move, &[lone.clone()], &dst, Arc::new(tracker())).unwrap());
        assert!(!lone.exists() && dst.join("tek/iç").is_dir());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn move_deletes_sources_only_after_everything_arrived() {
        let base = base("fastmove");
        let (src, keep, dst) = (base.join("kaynak"), base.join("kopya-referans"), base.join("hedef"));
        tree(&src);
        tree(&keep);
        std::fs::create_dir_all(&dst).unwrap();
        let lone = base.join("tek dosya.txt");
        std::fs::write(&lone, b"tek").unwrap();
        // the cross-volume path, exercised on one volume
        let t = Arc::new(tracker());
        let out = execute(&[src.clone(), lone.clone()], &dst, &t, 8, false);
        assert!(finish(Transfer::Move, &[src.clone(), lone.clone()], &dst, &t, out).unwrap());
        assert!(!src.exists() && !lone.exists(), "sources removed");
        same_bytes_tree(&keep, &dst.join("kaynak"));
        assert_eq!(std::fs::read(dst.join("tek dosya.txt")).unwrap(), b"tek");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn verify_catches_a_changed_copy() {
        let base = base("verify");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        tree(&src);
        std::fs::create_dir_all(&dst).unwrap();
        let t = tracker();
        t.set_verify(Verify::Full);
        assert!(transfer(Transfer::Copy, &[src.clone()], &dst, Arc::new(t)).unwrap(), "a clean copy verifies");
        let pairs = vec![(src.join("a/b/büyük.bin"), dst.join("kaynak/a/b/büyük.bin"))];
        assert!(check(&pairs, Verify::Full, &tracker()).is_empty());
        // flip one byte in the middle, keep size and time
        let f = dst.join("kaynak/a/b/büyük.bin");
        let time = std::fs::metadata(&f).unwrap().modified().unwrap();
        let mut bytes = std::fs::read(&f).unwrap();
        bytes[1_500_000] ^= 0xFF;
        std::fs::write(&f, &bytes).unwrap();
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(time).unwrap();
        assert!(check(&pairs, Verify::Quick, &tracker()).is_empty(), "size and time still match");
        assert_eq!(check(&pairs, Verify::Full, &tracker()), vec![src.join("a/b/büyük.bin")]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn pause_holds_and_speed_limit_slows() {
        let base = base("pause");
        let src = base.join("kaynak");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(base.join("hedef")).unwrap();
        std::fs::write(src.join("2mb.bin"), vec![1u8; 2 << 20]).unwrap();
        // 4 MB/s limit: 2 MB take ~0.5 s
        let t = tracker();
        t.limit.store(4 << 20, Ordering::Relaxed);
        let start = Instant::now();
        copy(&[src.join("2mb.bin")], &base.join("hedef"), &t, 2);
        assert!(start.elapsed() >= Duration::from_millis(350), "{:?}", start.elapsed());
        // paused: nothing happens until resumed
        let t = Arc::new(tracker());
        t.paused.store(true, Ordering::Relaxed);
        let t2 = t.clone();
        let (s2, d2) = (src.clone(), base.join("hedef2"));
        std::fs::create_dir_all(&d2).unwrap();
        let h = std::thread::spawn(move || copy(&[s2.join("2mb.bin")], &d2, &t2, 2).undo.len());
        std::thread::sleep(Duration::from_millis(400));
        assert!(!base.join("hedef2/2mb.bin").exists(), "nothing copied while paused");
        t.paused.store(false, Ordering::Relaxed);
        assert_eq!(h.join().unwrap(), 1);
        assert!(base.join("hedef2/2mb.bin").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn order_spreads_small_files_over_folders() {
        let j = |d: &str, f: &str, size: u64| Job { src: PathBuf::from(format!("s/{d}/{f}")), dst: PathBuf::from(format!("d/{d}/{f}")), size, op: Op::Copy, replace: false };
        let jobs = vec![j("a", "1", 10), j("a", "2", 10), j("a", "3", 10), j("b", "1", 10), j("b", "2", 10), j("c", "big", 100 << 20)];
        let order: Vec<String> = order(jobs).iter().map(|j| j.dst.to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(order, ["d/c/big", "d/a/1", "d/b/1", "d/a/2", "d/b/2", "d/a/3"]);
    }

    #[test]
    fn inside_is_case_insensitive() {
        assert!(inside(Path::new(r"c:\a\alt"), Path::new(r"C:\A")));
        assert!(inside(Path::new(r"C:\A"), Path::new(r"c:/a/")));
        assert!(!inside(Path::new(r"C:\ab"), Path::new(r"C:\a")));
    }

    #[test]
    fn volumes_and_self_nesting() {
        assert!(same_volume(Path::new(r"C:\a"), Path::new(r"c:\b\c")));
        assert!(!same_volume(Path::new(r"C:\a"), Path::new(r"D:\a")));
        assert!(same_volume(Path::new(r"\\nas\paylaşım\a"), Path::new(r"\\NAS\paylaşım\b")));
        assert!(!same_volume(Path::new(r"\\nas\a\x"), Path::new(r"\\nas\b\x")));
        // a folder into its own subfolder is left to the shell (which refuses it)
        let base = base("fastnest");
        std::fs::create_dir_all(base.join("a/alt")).unwrap();
        let t = tracker();
        let out = copy(&[base.join("a")], &base.join("a/alt"), &t, 2);
        assert!(out.undo.is_empty() && out.whole == vec![base.join("a")]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_without_partial_files() {
        let base = base("fastcopy-c");
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        tree(&src);
        std::fs::create_dir_all(&dst).unwrap();
        let t = tracker();
        t.cancel.store(true, Ordering::Relaxed);
        let _ = copy(&[src.clone()], &dst, &t, 4);
        // whatever exists is complete: no file differs from its source
        let mut stack = vec![dst.join("kaynak")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p.strip_prefix(dst.join("kaynak")).unwrap();
                    assert_eq!(std::fs::read(&p).unwrap(), std::fs::read(src.join(rel)).unwrap());
                }
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }
}
