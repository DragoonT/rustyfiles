//! File operations through the Windows shell copy engine (IFileOperation): same behavior as
//! Explorer (recycle bin, undo record, conflict dialogs, elevation prompts when needed).
//! Blocking — call from a worker thread.
//!
//! The engine is the shell's: with no shell window running it never finishes (seen without Explorer: the engine's own progress window
//! sat on the screen for ever). So every operation first asks `engine_guard::plan`: the engine when a shell window exists (run on a
//! thread of its own with a time limit for its start), a plain file call where one does the same thing (rename, permanent delete), and
//! a true refusal ([`NoShellEngine`]) for what only the shell can do (the Recycle Bin, names the shell makes). See `engine_guard.rs`.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NOCONFIRMMKDIR, FOF_RENAMEONCOLLISION, FOFX_ADDUNDORECORD,
    FOFX_RECYCLEONDELETE, FileOperation, IFileOperation, IShellItem, SHCreateItemFromParsingName,
};
use windows::core::PCWSTR;

use crate::engine_guard::{self as guard, Life, NoShellEngine, Op, Plan};
use crate::shell::ensure_com;
use crate::wide::wide;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    Copy,
    Move,
}

fn item(p: &Path) -> anyhow::Result<IShellItem> {
    let w = wide(p);
    Ok(unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None)? })
}

fn new_op(flags: u32) -> anyhow::Result<IFileOperation> {
    ensure_com();
    let op: IFileOperation = unsafe { CoCreateInstance(&FileOperation, None, CLSCTX_ALL)? };
    unsafe { op.SetOperationFlags(FILEOPERATION_FLAGS(flags))? };
    Ok(op)
}

/// A tracker nobody listens to and nobody cancels (the untracked operations still attach the progress sink: it is how the supervisor
/// learns that the engine started).
fn quiet_tracker() -> Arc<Tracker> {
    Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))))
}

/// Runs an engine job under supervision (a thread of its own, a limit for its start); the job is given the flag the sink raises.
fn engine<T: Send + 'static>(op: Op, job: impl FnOnce(Arc<Life>) -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    let life = Life::new();
    let seen = life.clone();
    guard::supervise(op, life, move || job(seen))
}

/// The answer for an operation that only the shell can do: the engine, or a refusal that says why.
fn needs_shell(op: Op) -> anyhow::Result<()> {
    match guard::plan(op, guard::shell_present()) {
        Plan::Refuse => Err(NoShellEngine { op }.into()),
        _ => Ok(()),
    }
}

/// Copies `sources` into `dest_dir` with the shell's engine and the given FOF_* flags (no undo
/// record unless the flags ask for one).
pub fn shell_copy(sources: &[PathBuf], dest_dir: &Path, flags: u32) -> anyhow::Result<()> {
    needs_shell(Op::Copy)?;
    let (sources, dest_dir) = (sources.to_vec(), dest_dir.to_path_buf());
    engine(Op::Copy, move |life| {
        let op = new_op(flags)?;
        let dest = item(&dest_dir)?;
        for s in &sources {
            let it = item(s)?;
            unsafe { op.CopyItem(&it, &dest, PCWSTR::null(), None)? };
        }
        anyhow::ensure!(perform_tracked(&op, quiet_tracker(), life)?, "the copy was aborted");
        Ok(())
    })
}

/// Copy or move into `dest_dir`. Pasting into the same folder makes "x - Kopya" names.
pub fn transfer(kind: Transfer, sources: &[PathBuf], dest_dir: &Path) -> anyhow::Result<bool> {
    let same_dir = sources.iter().all(|s| s.parent() == Some(dest_dir));
    if kind == Transfer::Move && same_dir {
        return Ok(true); // moving onto itself: no-op
    }
    let mut flags = FOF_ALLOWUNDO.0 as u32 | FOF_NOCONFIRMMKDIR.0 as u32 | FOFX_ADDUNDORECORD.0;
    if same_dir {
        flags |= FOF_RENAMEONCOLLISION.0 as u32;
    }
    transfer_through_engine(kind, sources, dest_dir, flags, quiet_tracker())
}

/// The engine part of every transfer: `flags` as the caller wants them, `tracker` the one the sink reports to.
fn transfer_through_engine(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, flags: u32, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let op_kind = if kind == Transfer::Copy { Op::Copy } else { Op::Move };
    needs_shell(op_kind)?;
    let (sources, dest_dir) = (sources.to_vec(), dest_dir.to_path_buf());
    engine(op_kind, move |life| {
        let op = new_op(flags)?;
        let dest = item(&dest_dir)?;
        for s in &sources {
            let it = item(s)?;
            unsafe {
                match kind {
                    Transfer::Copy => op.CopyItem(&it, &dest, PCWSTR::null(), None)?,
                    Transfer::Move => op.MoveItem(&it, &dest, PCWSTR::null(), None)?,
                }
            }
        }
        perform_tracked(&op, tracker, life)
    })
}

/// Delete to the recycle bin (`permanent = false`) or permanently (Windows asks for confirmation).
pub fn delete(paths: &[PathBuf], permanent: bool) -> anyhow::Result<bool> {
    let flags = if permanent { 0 } else { FOF_ALLOWUNDO.0 as u32 | FOFX_RECYCLEONDELETE.0 | FOFX_ADDUNDORECORD.0 };
    delete_through_engine(paths, permanent, flags, quiet_tracker())
}

/// The engine part of every delete. With no shell: a permanent delete is our own parallel delete alone (what it cannot remove is an
/// error that says so), the Recycle Bin is refused.
fn delete_through_engine(paths: &[PathBuf], permanent: bool, flags: u32, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let op_kind = if permanent { Op::PermanentDelete } else { Op::RecycleDelete };
    if guard::plan(op_kind, guard::shell_present()) == Plan::Own {
        let threads = crate::fastdelete::threads_for(paths);
        let done = crate::fastdelete::delete_without_shell(paths, tracker.clone(), threads)?;
        if !done && !tracker.cancel.load(Ordering::Relaxed) {
            // (something may well have been deleted already: this is not the "nothing was changed" refusal)
            return Err(guard::PartlyDone { op: Op::PermanentDelete, left: None }.into());
        }
        return Ok(done);
    }
    needs_shell(op_kind)?;
    let paths = paths.to_vec();
    engine(op_kind, move |life| {
        let op = new_op(flags)?;
        for p in &paths {
            unsafe { op.DeleteItem(&item(p)?, None)? };
        }
        perform_tracked(&op, tracker, life)
    })
}

pub fn rename(path: &Path, new_name: &str) -> anyhow::Result<bool> {
    if guard::plan(Op::Rename, guard::shell_present()) == Plan::Own {
        // no shell: the same rename as a plain file call, failing instead of replacing; the app's own undo list records it as before
        let to = path.with_file_name(new_name);
        log::info!("rename: no shell window, renaming directly");
        rename_no_replace(path, &to)?;
        return Ok(true);
    }
    let (path, new_name) = (path.to_path_buf(), new_name.to_string());
    engine(Op::Rename, move |life| {
        let op = new_op(FOF_ALLOWUNDO.0 as u32 | FOFX_ADDUNDORECORD.0)?;
        let w = wide(&new_name);
        unsafe { op.RenameItem(&item(&path)?, PCWSTR(w.as_ptr()), None)? };
        perform_tracked(&op, quiet_tracker(), life)
    })
}

/// Plain rename that fails instead of silently replacing an existing target
/// (`std::fs::rename` replaces on Windows).
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use windows::Win32::Storage::FileSystem::{MOVE_FILE_FLAGS, MoveFileExW};
    let (a, b) = (crate::wide::long_path(from), crate::wide::long_path(to));
    unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), MOVE_FILE_FLAGS(0)) }.map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

/// `name`, `name (2)`, … (the caller passes the name in the UI language) — returns the created path.
pub fn new_folder(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let p = unique_name(dir, name, "");
    std::fs::create_dir(&p)?;
    Ok(p)
}

pub fn new_text_file(dir: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let p = unique_name(dir, name, ".txt");
    std::fs::OpenOptions::new().write(true).create_new(true).open(&p)?;
    Ok(p)
}

pub fn unique_name(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let first = dir.join(format!("{stem}{ext}"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}


// ---------------------------------------------------------------------------------------------
// Tracked operations (in-app Status Center): progress callback + cancellation, no shell
// progress window (conflict / elevation dialogs still come from Windows).
// ---------------------------------------------------------------------------------------------

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::E_ABORT;
use windows::Win32::UI::Shell::{FOF_SILENT, IFileOperationProgressSink, IFileOperationProgressSink_Impl, SIGDN_NORMALDISPLAY};
use windows_core::{HRESULT, Ref, implement};

/// Bytes done and bytes in all, for an operation whose engine knows them (our copy engine,
/// the archive code). `on_progress` only carries abstract units, which is enough for a
/// percentage but not for a speed in MB/s; the interface reads this when a progress report
/// arrives. Both stay 0 for the shell's engine and for operations counted in items.
#[derive(Default)]
pub struct ByteProgress {
    done: std::sync::atomic::AtomicU64,
    total: std::sync::atomic::AtomicU64,
}

impl ByteProgress {
    pub fn set(&self, done: u64, total: u64) {
        // total first: a reader that sees the new `done` never pairs it with an older, smaller total
        self.total.store(total, std::sync::atomic::Ordering::Relaxed);
        self.done.store(done.min(total), std::sync::atomic::Ordering::Relaxed);
    }

    /// `(done, total)`; `total == 0` means the engine does not count bytes.
    pub fn get(&self) -> (u64, u64) {
        let done = self.done.load(std::sync::atomic::Ordering::Relaxed);
        let total = self.total.load(std::sync::atomic::Ordering::Relaxed);
        (done.min(total), total)
    }
}

/// Progress reporting for one operation. `on_progress(total, done, current_item)`;
/// `total`/`done` are the shell's abstract work units (use done/total as a fraction).
pub struct Tracker {
    /// Bytes, when the engine counts them (see [`ByteProgress`]). Shared so the interface can
    /// keep a handle while the worker owns the tracker.
    pub bytes: Arc<ByteProgress>,
    pub on_progress: Box<dyn Fn(u32, u32, Option<String>) + Send + Sync>,
    pub cancel: Arc<AtomicBool>,
    /// (source, result) for every item the shell finished: the new copy/moved location, or the
    /// item's path inside the Recycle Bin for deletes. Used to build undo records.
    pub results: std::sync::Mutex<Vec<(PathBuf, PathBuf)>>,
    /// Our engine waits while this is set (the shell engine can't pause).
    pub paused: Arc<AtomicBool>,
    /// Copy speed limit in bytes per second; 0 = none (our engine only).
    pub limit: std::sync::atomic::AtomicU64,
    /// Check copies after writing them (our engine only); see [`Tracker::set_verify`].
    pub verify: std::sync::atomic::AtomicU8,
    /// Asked (on the worker thread, blocking) what to do with names that already exist at the
    /// destination; `None` cancels the operation. Without it, both are kept.
    #[allow(clippy::type_complexity)]
    pub ask: Option<Box<dyn Fn(&[crate::fastcopy::Conflict]) -> Option<Vec<crate::fastcopy::Resolve>> + Send + Sync>>,
}

impl Tracker {
    pub fn set_verify(&self, v: crate::fastcopy::Verify) {
        self.verify.store(v as u8, Ordering::Relaxed);
    }

    pub fn verify_mode(&self) -> crate::fastcopy::Verify {
        match self.verify.load(Ordering::Relaxed) {
            1 => crate::fastcopy::Verify::Quick,
            2 => crate::fastcopy::Verify::Full,
            _ => crate::fastcopy::Verify::Off,
        }
    }

    pub fn new(on_progress: Box<dyn Fn(u32, u32, Option<String>) + Send + Sync>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            bytes: Default::default(),
            on_progress,
            cancel,
            results: Default::default(),
            paused: Default::default(),
            limit: Default::default(),
            verify: Default::default(),
            ask: None,
        }
    }
}

fn fs_path(item: Ref<IShellItem>) -> Option<PathBuf> {
    let it = item.as_ref()?;
    unsafe {
        let p = it.GetDisplayName(windows::Win32::UI::Shell::SIGDN_FILESYSPATH).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

#[implement(IFileOperationProgressSink)]
struct Sink {
    tracker: Arc<Tracker>,
    last: std::sync::Mutex<(u32, u32)>,
    /// Raised at the engine's first sign of life (see `engine_guard::supervise`).
    life: Arc<Life>,
}

impl Sink {
    fn check(&self) -> windows_core::Result<()> {
        self.life.signal();
        if self.tracker.cancel.load(Ordering::Relaxed) { Err(E_ABORT.into()) } else { Ok(()) }
    }
    fn record(&self, hr: HRESULT, src: Ref<IShellItem>, created: Ref<IShellItem>) {
        if hr.is_ok() {
            if let (Some(a), Some(b)) = (fs_path(src), fs_path(created)) {
                self.tracker.results.lock().unwrap().push((a, b));
            }
        }
    }
    fn item(&self, item: Ref<IShellItem>) {
        if let Some(it) = item.as_ref() {
            if let Ok(name) = unsafe { it.GetDisplayName(SIGDN_NORMALDISPLAY) } {
                let s = unsafe { name.to_string().unwrap_or_default() };
                unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(name.0 as *const _)) };
                let (t, d) = *self.last.lock().unwrap();
                (self.tracker.on_progress)(t, d, Some(s));
            }
        }
    }
}

#[allow(non_snake_case)]
impl IFileOperationProgressSink_Impl for Sink_Impl {
    fn StartOperations(&self) -> windows_core::Result<()> { self.life.signal(); Ok(()) }
    fn FinishOperations(&self, _hr: HRESULT) -> windows_core::Result<()> { Ok(()) }
    fn PreRenameItem(&self, _f: u32, item: Ref<IShellItem>, _n: &PCWSTR) -> windows_core::Result<()> { self.item(item); self.check() }
    fn PostRenameItem(&self, _f: u32, _i: Ref<IShellItem>, _n: &PCWSTR, _hr: HRESULT, _c: Ref<IShellItem>) -> windows_core::Result<()> { self.check() }
    fn PreMoveItem(&self, _f: u32, item: Ref<IShellItem>, _d: Ref<IShellItem>, _n: &PCWSTR) -> windows_core::Result<()> { self.item(item); self.check() }
    fn PostMoveItem(&self, _f: u32, i: Ref<IShellItem>, _d: Ref<IShellItem>, _n: &PCWSTR, hr: HRESULT, c: Ref<IShellItem>) -> windows_core::Result<()> { self.record(hr, i, c); self.check() }
    fn PreCopyItem(&self, _f: u32, item: Ref<IShellItem>, _d: Ref<IShellItem>, _n: &PCWSTR) -> windows_core::Result<()> { self.item(item); self.check() }
    fn PostCopyItem(&self, _f: u32, i: Ref<IShellItem>, _d: Ref<IShellItem>, _n: &PCWSTR, hr: HRESULT, c: Ref<IShellItem>) -> windows_core::Result<()> { self.record(hr, i, c); self.check() }
    fn PreDeleteItem(&self, _f: u32, item: Ref<IShellItem>) -> windows_core::Result<()> { self.item(item); self.check() }
    fn PostDeleteItem(&self, _f: u32, i: Ref<IShellItem>, hr: HRESULT, c: Ref<IShellItem>) -> windows_core::Result<()> { self.record(hr, i, c); self.check() }
    fn PreNewItem(&self, _f: u32, _d: Ref<IShellItem>, _n: &PCWSTR) -> windows_core::Result<()> { self.check() }
    fn PostNewItem(&self, _f: u32, _d: Ref<IShellItem>, _n: &PCWSTR, _t: &PCWSTR, _a: u32, _hr: HRESULT, _c: Ref<IShellItem>) -> windows_core::Result<()> { self.check() }
    fn UpdateProgress(&self, total: u32, done: u32) -> windows_core::Result<()> {
        *self.last.lock().unwrap() = (total, done);
        (self.tracker.on_progress)(total, done, None);
        self.check()
    }
    fn ResetTimer(&self) -> windows_core::Result<()> { Ok(()) }
    fn PauseTimer(&self) -> windows_core::Result<()> { Ok(()) }
    fn ResumeTimer(&self) -> windows_core::Result<()> { Ok(()) }
}

/// Performs the operation with the progress sink attached. Ok(true) when everything completed, Ok(false) when the user cancelled part of it.
fn perform_tracked(op: &IFileOperation, tracker: Arc<Tracker>, life: Arc<Life>) -> anyhow::Result<bool> {
    let sink: IFileOperationProgressSink = Sink { tracker: tracker.clone(), last: std::sync::Mutex::new((0, 0)), life }.into();
    let cookie = unsafe { op.Advise(&sink)? };
    let r = (|| -> anyhow::Result<bool> {
        unsafe {
            op.PerformOperations()?;
            Ok(!op.GetAnyOperationsAborted()?.as_bool())
        }
    })();
    unsafe {
        let _ = op.Unadvise(cookie);
    }
    if tracker.cancel.load(Ordering::Relaxed) {
        return Ok(false);
    }
    r
}

/// `transfer` with in-app progress (no shell progress window).
pub fn transfer_tracked(kind: Transfer, sources: &[PathBuf], dest_dir: &Path, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let same_dir = sources.iter().all(|s| s.parent() == Some(dest_dir));
    if kind == Transfer::Move && same_dir {
        return Ok(true);
    }
    let mut flags = FOF_ALLOWUNDO.0 as u32 | FOF_NOCONFIRMMKDIR.0 as u32 | FOFX_ADDUNDORECORD.0 | FOF_SILENT.0 as u32;
    if same_dir {
        flags |= FOF_RENAMEONCOLLISION.0 as u32;
    }
    transfer_through_engine(kind, sources, dest_dir, flags, tracker)
}

/// `delete` with in-app progress.
pub fn delete_tracked(paths: &[PathBuf], permanent: bool, tracker: Arc<Tracker>) -> anyhow::Result<bool> {
    let mut flags = if permanent { 0 } else { FOF_ALLOWUNDO.0 as u32 | FOFX_RECYCLEONDELETE.0 | FOFX_ADDUNDORECORD.0 };
    flags |= FOF_SILENT.0 as u32;
    delete_through_engine(paths, permanent, flags, tracker)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unique() {
        // the shell's engine never finishes without a shell window (seen without Explorer): say so and return, so the suite ends
        if guard::not_run_without_shell("fileops::tests::unique") {
            return;
        }
        let d = std::env::temp_dir().join(format!("rf-uniq-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = new_folder(&d, "Yeni klasör").unwrap();
        let b = new_folder(&d, "Yeni klasör").unwrap();
        assert!(a.ends_with("Yeni klasör"));
        assert!(b.ends_with("Yeni klasör (2)"));
        let f = new_text_file(&d, "Yeni Metin Belgesi").unwrap();
        assert!(rename(&f, "x.txt").unwrap());
        assert!(d.join("x.txt").exists());
        assert!(transfer(Transfer::Copy, &[d.join("x.txt")], &d).unwrap());
        let n = std::fs::read_dir(&d).unwrap().count();
        assert_eq!(n, 4); // 2 folders, x.txt, x - Kopya.txt
        // tracked copy reports progress
        let calls = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c2 = calls.clone();
        let t = Arc::new(Tracker::new(
            Box::new(move |_, _, _| {
                c2.fetch_add(1, Ordering::Relaxed);
            }),
            Arc::new(AtomicBool::new(false)),
        ));
        std::fs::create_dir_all(d.join("dst")).unwrap();
        assert!(transfer_tracked(Transfer::Copy, &[d.join("x.txt")], &d.join("dst"), t.clone()).unwrap());
        assert!(d.join("dst").join("x.txt").exists());
        assert!(calls.load(Ordering::Relaxed) > 0);
        let res = t.results.lock().unwrap().clone();
        assert_eq!(res.len(), 1);
        assert!(res[0].1.ends_with("dst\\x.txt"), "{res:?}");
        std::fs::remove_dir_all(&d).unwrap();
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-noshell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn without_a_shell_a_rename_is_a_plain_rename_that_does_not_replace() {
        let d = scratch("rename");
        std::fs::write(d.join("a.txt"), b"a").unwrap();
        std::fs::write(d.join("b.txt"), b"b").unwrap();
        guard::with_no_shell(|| {
            assert!(rename(&d.join("a.txt"), "c.txt").unwrap());
            assert!(!d.join("a.txt").exists() && d.join("c.txt").exists());
            // an existing target is an error and nothing is replaced
            assert!(rename(&d.join("c.txt"), "b.txt").is_err());
            assert_eq!(std::fs::read(d.join("b.txt")).unwrap(), b"b");
            assert!(d.join("c.txt").exists());
        });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_a_shell_the_recycle_bin_and_the_shells_copy_are_refused_and_nothing_is_touched() {
        let d = scratch("refuse");
        let f = d.join("keep.txt");
        std::fs::write(&f, b"x").unwrap();
        std::fs::create_dir_all(d.join("dst")).unwrap();
        guard::with_no_shell(|| {
            let t = Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))));
            let e = delete_tracked(std::slice::from_ref(&f), false, t.clone()).expect_err("the bin needs the shell");
            assert_eq!(e.downcast_ref::<NoShellEngine>().map(|n| n.op), Some(Op::RecycleDelete), "{e:#}");
            let e = delete(std::slice::from_ref(&f), false).expect_err("same, untracked");
            assert!(e.downcast_ref::<NoShellEngine>().is_some());
            let e = transfer_tracked(Transfer::Copy, std::slice::from_ref(&f), &d.join("dst"), t.clone()).expect_err("the engine copy needs the shell");
            assert_eq!(e.downcast_ref::<NoShellEngine>().map(|n| n.op), Some(Op::Copy));
            let e = transfer(Transfer::Move, std::slice::from_ref(&f), &d.join("dst")).expect_err("and so does a move");
            assert_eq!(e.downcast_ref::<NoShellEngine>().map(|n| n.op), Some(Op::Move));
            let e = shell_copy(std::slice::from_ref(&f), &d.join("dst"), 0).expect_err("and the plain shell copy");
            assert!(e.downcast_ref::<NoShellEngine>().is_some());
        });
        assert!(f.exists(), "a refused operation leaves the file where it is");
        assert!(std::fs::read_dir(d.join("dst")).unwrap().next().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn without_a_shell_a_permanent_delete_is_our_own_and_a_missing_item_is_done() {
        let d = scratch("permanent");
        std::fs::create_dir_all(d.join("tree/sub")).unwrap();
        std::fs::write(d.join("tree/sub/f.txt"), b"1").unwrap();
        std::fs::write(d.join("lone.txt"), b"2").unwrap();
        let t = Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))));
        guard::with_no_shell(|| {
            let gone = d.join("never-existed.txt");
            assert!(delete_tracked(&[d.join("tree"), d.join("lone.txt"), gone], true, t.clone()).unwrap());
        });
        assert!(!d.join("tree").exists() && !d.join("lone.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
