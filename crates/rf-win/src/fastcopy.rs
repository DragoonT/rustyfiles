//! Our own parallel copy engine: many files at once with `CopyFile2` (attributes, times and
//! alternate data streams come along), instead of the shell engine's one-file-at-a-time
//! pipeline. On this machine the shell engine needs ~200 s for 20 000 small files, a
//! parallel copy ~10 s.
//!
//! It only takes the plain cases and hands everything else to the shell engine, so behavior
//! stays Explorer's where it matters:
//! * a top-level item whose name already exists at the destination (the shell's conflict
//!   dialog), or that contains links / junctions, goes to the shell;
//! * files that fail here (access denied, in use, …) are retried through the shell at the
//!   end, which can ask for elevation or retry.
//!
//! Blocking: call from a worker thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use windows::Win32::Storage::FileSystem::{
    COPY_FILE_FAIL_IF_EXISTS, COPY_FILE_NO_BUFFERING, COPYFILE2_EXTENDED_PARAMETERS, CopyFile2, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW,
};
use windows::core::PCWSTR;

use crate::fileops::{Tracker, Transfer};
use crate::wide::long_path;

/// Files above this size are copied unbuffered (no point filling the cache with them).
const UNBUFFERED: u64 = 256 << 20;

/// One file or folder to create at the destination.
struct Job {
    src: PathBuf,
    dst: PathBuf,
    size: u64,
}

/// What the parallel pass couldn't do, for the shell engine.
#[derive(Default)]
pub struct Leftovers {
    /// Top-level sources (and their destination folder) the shell should copy whole.
    pub whole: Vec<PathBuf>,
    /// Single files that failed: (source, destination folder).
    pub files: Vec<(PathBuf, PathBuf)>,
}

/// Plans a copy: which top-level items the fast engine takes, and its folders and files.
/// `None` when it should all go to the shell (nothing plain to do).
fn plan(sources: &[PathBuf], dest_dir: &Path, left: &mut Leftovers) -> (Vec<(PathBuf, PathBuf)>, Vec<PathBuf>, Vec<Job>) {
    let mut tops = Vec::new();
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    'top: for s in sources {
        let Some(name) = s.file_name() else {
            left.whole.push(s.clone());
            continue;
        };
        let dst = dest_dir.join(name);
        // a name already there, or a folder into itself: the shell decides / refuses
        if std::fs::symlink_metadata(&dst).is_ok() || s.parent().is_some_and(|p| inside(p, dest_dir) && inside(dest_dir, p)) || inside(dest_dir, s) {
            left.whole.push(s.clone());
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(s) else {
            left.whole.push(s.clone());
            continue;
        };
        if is_reparse(&meta) {
            left.whole.push(s.clone());
            continue;
        }
        if meta.is_file() {
            files.push(Job { src: s.clone(), dst: dst.clone(), size: meta.len() });
            tops.push((s.clone(), dst));
            continue;
        }
        // a folder: walk it; links or junctions inside → the whole item goes to the shell
        let (mut d, mut f) = (vec![dst.clone()], Vec::new());
        let mut stack = vec![(s.clone(), dst.clone())];
        while let Some((from, to)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&from) else {
                left.whole.push(s.clone());
                continue 'top;
            };
            for e in rd.flatten() {
                let Ok(m) = e.metadata() else {
                    left.whole.push(s.clone());
                    continue 'top;
                };
                if is_reparse(&m) {
                    left.whole.push(s.clone());
                    continue 'top;
                }
                let (a, b) = (e.path(), to.join(e.file_name()));
                if m.is_dir() {
                    d.push(b.clone());
                    stack.push((a, b));
                } else {
                    f.push(Job { src: a, dst: b, size: m.len() });
                }
            }
        }
        dirs.extend(d);
        files.extend(f);
        tops.push((s.clone(), dst));
    }
    (tops, dirs, files)
}

fn is_reparse(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
}

/// Copies `sources` into `dest_dir` in parallel. Returns the (source, copy) pairs done here
/// (for undo) and what is left for the shell engine.
pub fn copy(sources: &[PathBuf], dest_dir: &Path, tracker: &Tracker, threads: usize) -> (Vec<(PathBuf, PathBuf)>, Leftovers) {
    let mut left = Leftovers::default();
    let (tops, dirs, mut files) = plan(sources, dest_dir, &mut left);
    // folders first (in walk order, parents before children)
    for d in &dirs {
        if let Err(e) = std::fs::create_dir(d) {
            if e.kind() != std::io::ErrorKind::AlreadyExists {
                // can't even make the folder: let the shell try the whole item
                log::warn!("fastcopy: mkdir {}: {e}", d.display());
            }
        }
    }
    // big files first: they set the pace, small ones fill the gaps
    files.sort_by_key(|j| std::cmp::Reverse(j.size));
    let total: u64 = files.iter().map(|j| j.size.max(1)).sum::<u64>().max(1);
    let done = AtomicU64::new(0);
    let next = AtomicUsize::new(0);
    let failed = std::sync::Mutex::new(Vec::new());
    let cancel_flag = std::sync::atomic::AtomicI32::new(0);
    let report = |name: Option<String>| {
        let d = done.load(Ordering::Relaxed).min(total);
        (tracker.on_progress)(1000, (d * 1000 / total) as u32, name);
    };
    std::thread::scope(|s| {
        for _ in 0..threads.clamp(1, files.len().max(1)) {
            s.spawn(|| {
                loop {
                    if tracker.cancel.load(Ordering::Relaxed) {
                        cancel_flag.store(1, Ordering::Relaxed);
                        return;
                    }
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(j) = files.get(i) else { return };
                    match copy_one(j, &cancel_flag) {
                        Ok(()) => {}
                        Err(e) => {
                            if !tracker.cancel.load(Ordering::Relaxed) {
                                log::info!("fastcopy: {} → shell ({e})", j.src.display());
                                failed.lock().unwrap().push(j.src.clone());
                            }
                        }
                    }
                    done.fetch_add(j.size.max(1), Ordering::Relaxed);
                    if i % 64 == 0 || j.size > 1 << 20 {
                        report(j.src.file_name().map(|n| n.to_string_lossy().into_owned()));
                    }
                }
            });
        }
    });
    report(None);
    // folder attributes (hidden, read-only, …) once their contents are in
    for (src, dst) in walk_dir_pairs(&tops) {
        if let Ok(m) = std::fs::metadata(&src) {
            use std::os::windows::fs::MetadataExt;
            let w = long_path(&dst);
            unsafe {
                let _ = SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(m.file_attributes()));
            }
        }
    }
    let failed = failed.into_inner().unwrap();
    for f in failed {
        // its destination folder: the same relative place under the top-level copy
        if let Some((top_src, top_dst)) = tops.iter().find(|(t, _)| f.starts_with(t)) {
            let rel = f.strip_prefix(top_src).unwrap_or(Path::new(""));
            let dst = top_dst.join(rel);
            if let Some(parent) = dst.parent() {
                left.files.push((f.clone(), parent.to_path_buf()));
            }
        }
    }
    (tops, left)
}

/// (source folder, copy) for every folder of the copied top-level items.
fn walk_dir_pairs(tops: &[(PathBuf, PathBuf)]) -> Vec<(PathBuf, PathBuf)> {
    let mut out = Vec::new();
    for (s, d) in tops {
        if !s.is_dir() {
            continue;
        }
        let mut stack = vec![(s.clone(), d.clone())];
        while let Some((a, b)) = stack.pop() {
            if let Ok(rd) = std::fs::read_dir(&a) {
                for e in rd.flatten() {
                    if e.file_type().is_ok_and(|t| t.is_dir()) {
                        stack.push((e.path(), b.join(e.file_name())));
                    }
                }
            }
            out.push((a, b));
        }
    }
    out
}

fn copy_one(j: &Job, cancel: &std::sync::atomic::AtomicI32) -> windows::core::Result<()> {
    let (src, dst) = (long_path(&j.src), long_path(&j.dst));
    let mut flags = COPY_FILE_FAIL_IF_EXISTS.0;
    if j.size >= UNBUFFERED {
        flags |= COPY_FILE_NO_BUFFERING.0;
    }
    let params = COPYFILE2_EXTENDED_PARAMETERS {
        dwSize: std::mem::size_of::<COPYFILE2_EXTENDED_PARAMETERS>() as u32,
        dwCopyFlags: windows::Win32::Storage::FileSystem::COPYFILE_FLAGS(flags),
        // CopyFile2 polls this and stops (removing the partial file) when it turns non-zero
        pfCancel: cancel.as_ptr() as *mut windows_core::BOOL,
        pProgressRoutine: None,
        pvCallbackContext: std::ptr::null_mut(),
    };
    unsafe { CopyFile2(PCWSTR(src.as_ptr()), PCWSTR(dst.as_ptr()), Some(&params)) }
}

/// Copy or move with the fast engine where it can, the shell engine for the rest. Returns
/// like `fileops::transfer_tracked`; undo pairs land in `tracker.results`.
///
/// A move within one volume is a rename: the shell does that instantly. A move to another
/// volume is a parallel copy, then each source item is deleted only once every file left in
/// it has a same-size counterpart at the destination.
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let renames = kind == Transfer::Move && sources.iter().any(|s| same_volume(s, dest_dir));
    if renames || !local_fs(dest_dir) || !sources.iter().all(|s| local_fs(s)) {
        return crate::fileops::transfer_tracked(kind, sources, dest_dir, tracker);
    }
    fast_transfer(kind, sources, dest_dir, tracker)
}

fn fast_transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let threads = threads_for(sources, dest_dir);
    let (pairs, left) = copy(sources, dest_dir, &tracker, threads);
    tracker.results.lock().unwrap().extend(pairs);
    if tracker.cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    let mut ok = true;
    if !left.whole.is_empty() {
        ok &= crate::fileops::transfer_tracked(kind, &left.whole, dest_dir, tracker.clone())?;
    }
    // failed single files: one shell call per destination folder
    let mut by_dir: Vec<(PathBuf, Vec<PathBuf>)> = Vec::new();
    for (f, d) in left.files {
        match by_dir.iter_mut().find(|(x, _)| *x == d) {
            Some((_, v)) => v.push(f),
            None => by_dir.push((d, vec![f])),
        }
    }
    for (d, fs) in by_dir {
        // the shell reports its own results; those files' undo is covered by their folder
        let t = Arc::new(Tracker::new(Box::new(|_, _, _| {}), tracker.cancel.clone()));
        ok &= crate::fileops::transfer_tracked(kind, &fs, &d, t)?;
    }
    if kind == Transfer::Move && !tracker.cancel.load(Ordering::Relaxed) {
        // the shell moved what failed here; whatever is left in a source was copied by us
        let pairs = tracker.results.lock().unwrap().clone();
        for (src, dst) in pairs.iter().filter(|(s, _)| sources.contains(s) && !left.whole.contains(s)) {
            if let Some((files, dirs)) = verified(src, dst) {
                // only what was checked goes: a file added to the source meanwhile stays
                ok &= crate::fastdelete::remove_verified(&files, &dirs);
            } else {
                log::warn!("fastcopy: {} kept: not everything arrived at {}", src.display(), dst.display());
                ok = false;
            }
        }
        for s in sources {
            if let Some(p) = s.parent() {
                crate::shell::notify_updated(p);
            }
        }
    }
    crate::shell::notify_updated(dest_dir);
    Ok(ok)
}

/// Every file under `src` has a counterpart under `dst` with the same size and modification
/// time (CopyFile2 keeps it; 2 s of slack for FAT/exFAT sticks): the source files and folders
/// (parents first) that may then be removed. `None` when anything is missing or differs, e.g.
/// a source file changed after it was copied.
fn verified(src: &Path, dst: &Path) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    let same_time = |a: &std::fs::Metadata, b: &std::fs::Metadata| match (a.modified(), b.modified()) {
        (Ok(x), Ok(y)) => x.duration_since(y).or_else(|_| y.duration_since(x)).is_ok_and(|d| d <= std::time::Duration::from_secs(2)),
        _ => false,
    };
    let (mut files, mut dirs) = (Vec::new(), Vec::new());
    let mut stack = vec![(src.to_path_buf(), dst.to_path_buf())];
    while let Some((a, b)) = stack.pop() {
        let ma = std::fs::symlink_metadata(&a).ok()?;
        if is_reparse(&ma) {
            return None; // links are the shell's business
        }
        if !ma.is_dir() {
            let mb = std::fs::symlink_metadata(&b).ok()?;
            if mb.is_dir() || mb.len() != ma.len() || !same_time(&ma, &mb) {
                return None;
            }
            files.push(a);
            continue;
        }
        if !std::fs::symlink_metadata(&b).ok()?.is_dir() {
            return None;
        }
        for e in std::fs::read_dir(&a).ok()? {
            let e = e.ok()?;
            stack.push((e.path(), b.join(e.file_name())));
        }
        dirs.push(a);
    }
    Some((files, dirs))
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
/// many at once, and network shares like a moderate number.
fn threads_for(sources: &[PathBuf], dest: &Path) -> usize {
    let cores = std::thread::available_parallelism().map_or(4, |n| n.get());
    let paths = sources.iter().map(PathBuf::as_path).chain(std::iter::once(dest));
    let mut network = false;
    for p in paths {
        match crate::drives::seek_penalty(p) {
            Some(true) => return 2,
            Some(false) => {}
            None => network = true,
        }
    }
    if network { cores.clamp(2, 8) } else { cores.clamp(2, 16) }
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

    #[test]
    fn copies_a_tree_exactly() {
        let base = std::env::temp_dir().join(format!("rf-fastcopy-{}", std::process::id()));
        let src = base.join("kaynak");
        let dst = base.join("hedef");
        tree(&src);
        let _ = std::fs::remove_dir_all(&dst);
        std::fs::create_dir_all(&dst).unwrap();
        let t = tracker();
        let (pairs, left) = copy(&[src.clone(), src.join("kök.txt")], &dst, &t, 8);
        assert!(left.whole.is_empty() && left.files.is_empty());
        assert_eq!(pairs.len(), 2);
        same(&src, &dst.join("kaynak"));
        same(&src.join("kök.txt"), &dst.join("kök.txt"));
        // a second copy into the same place: every name conflicts → all left to the shell
        let (pairs, left) = copy(&[src.clone()], &dst, &t, 8);
        assert!(pairs.is_empty());
        assert_eq!(left.whole, vec![src.clone()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn move_deletes_sources_only_after_everything_arrived() {
        let base = std::env::temp_dir().join(format!("rf-fastmove-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, keep, dst) = (base.join("kaynak"), base.join("kopya-referans"), base.join("hedef"));
        tree(&src);
        tree(&keep);
        std::fs::create_dir_all(&dst).unwrap();
        let lone = base.join("tek dosya.txt");
        std::fs::write(&lone, b"tek").unwrap();
        // the cross-volume path (fast_transfer), exercised on one volume
        let ok = fast_transfer(Transfer::Move, &[src.clone(), lone.clone()], &dst, Arc::new(tracker())).unwrap();
        assert!(ok);
        assert!(!src.exists() && !lone.exists(), "sources removed");
        // same bytes as an untouched twin (times differ: the twin was written later)
        let mut stack = vec![(keep.clone(), dst.join("kaynak"))];
        while let Some((a, b)) = stack.pop() {
            for e in std::fs::read_dir(&a).unwrap() {
                let e = e.unwrap();
                let (x, y) = (e.path(), b.join(e.file_name()));
                if x.is_dir() {
                    stack.push((x, y));
                } else {
                    assert_eq!(std::fs::read(&x).unwrap(), std::fs::read(&y).unwrap(), "{}", y.display());
                }
            }
        }
        assert_eq!(std::fs::read(dst.join("tek dosya.txt")).unwrap(), b"tek");
        // something missing at the destination: the source must stay
        tree(&src);
        let half = base.join("yarım");
        std::fs::create_dir_all(&half).unwrap();
        assert!(verified(&src, &half.join("kaynak")).is_none());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn move_never_loses_late_changes() {
        let base = std::env::temp_dir().join(format!("rf-fastmove2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, dst) = (base.join("kaynak"), base.join("hedef"));
        tree(&src);
        std::fs::create_dir_all(&dst).unwrap();
        let t = tracker();
        let (pairs, _) = copy(&[src.clone()], &dst, &t, 4);
        assert_eq!(pairs.len(), 1);
        let copy_root = dst.join("kaynak");
        // verified right after the copy
        let (files, dirs) = verified(&src, &copy_root).expect("copy matches");
        assert!(files.len() > 300 && dirs.len() >= 4);
        // a source file rewritten after copying (same size, later time): not verified
        let f = src.join("kök.txt");
        std::fs::write(&f, b"MERHABA").unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(later).unwrap();
        assert!(verified(&src, &copy_root).is_none());
        // a file added after verification: removing the verified list leaves it (and its folder)
        let added = src.join("a/b/c/sonradan.txt");
        std::fs::write(&added, b"yeni").unwrap();
        assert!(!crate::fastdelete::remove_verified(&files, &dirs));
        assert_eq!(std::fs::read(&added).unwrap(), b"yeni");
        assert!(!src.join("a/b/c/f1.txt").exists(), "verified files are gone");
        let _ = std::fs::remove_dir_all(&base);
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
        let base = std::env::temp_dir().join(format!("rf-fastnest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("a/alt")).unwrap();
        let t = tracker();
        let (pairs, left) = copy(&[base.join("a")], &base.join("a/alt"), &t, 2);
        assert!(pairs.is_empty() && left.whole == vec![base.join("a")]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn cancel_stops_without_partial_files() {
        let base = std::env::temp_dir().join(format!("rf-fastcopy-c-{}", std::process::id()));
        let src = base.join("kaynak");
        let dst = base.join("hedef");
        tree(&src);
        let _ = std::fs::remove_dir_all(&dst);
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
