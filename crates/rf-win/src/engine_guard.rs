//! The shell's file-operation engine (`IFileOperation`) and what the app does when no shell is running.
//!
//! WHAT WAS SEEN (the user's second trial, without Explorer): the rf-win tests that go through the engine (`fileops::tests::unique` and
//! its neighbours, `shell_window::tests`) never finished. Each sat in the engine's own progress window ("Yeniden adlandırmaya
//! hazırlanıyor", "0 öğe bulundu...") on the user's screen until the test process was killed. The rename was given
//! `FOF_ALLOWUNDO | FOFX_ADDUNDORECORD` and no "no UI" flag, so the engine opens its own progress window and its undo record, and both are
//! the shell's: with no shell window the window is never answered and the operation never ends. (Documentation: `IFileOperation` is the
//! shell's copy engine; `SetOperationFlags` says `FOF_NO_UI`/`FOF_SILENT` suppress the dialogs, but nothing in it promises that the
//! engine works without a shell, and the trial showed it does not.)
//!
//! WHAT THE APP DOES NOW. One decision per kind of operation ([`plan`], pure, tested), taken on the CALLING thread before anything is
//! created:
//!
//! | operation | shell window there | no shell window |
//! |---|---|---|
//! | rename | the engine (undo record for Explorer's Ctrl+Z) | `MoveFileEx` without replace: the same rename, our own undo list (`undo.rs`) still records it |
//! | permanent delete | the engine, after our own parallel delete | our own parallel delete (`fastdelete`) alone; what it cannot remove (locked, read-only that cannot be cleared, access denied) is reported, not handed to the shell |
//! | delete to the Recycle Bin | the engine | REFUSED with a true message. There is no documented way to put a file into the bin without the shell: the bin is the shell's own folder (`$Recycle.Bin`, the `$I`/`$R` pair with its own format), and writing those by hand could leave the user's bin in a state Explorer does not read. The user is told, and "delete permanently" stays available |
//! | copy / move through the engine (paste in the same folder, names the shell must make, virtual folders, the leftovers of our own engine) | the engine | REFUSED with a true message. Our own copy engine (`fastcopy`) does the ordinary copies and moves by itself and does not come here |
//! | restore from / delete in the Recycle Bin (undo of a delete) | the bin's own verbs | REFUSED with a true message |
//! | new folder, new text file, bulk rename | plain file calls | unchanged (they never used the engine) |
//!
//! A TIME LIMIT on every call that does go to the engine ([`supervise`]): the call runs on a thread of its own and the caller waits for
//! a first sign of life from it (the engine tells the progress sink when it starts and when it begins an item). If nothing comes within
//! [`START_LIMIT`] the caller gets [`EngineStalled`] (a true message: the engine did not start, it may still finish, look at its window).
//! Once it has started there is no limit: a copy of a hundred gigabytes is legitimately long, the Status Center can cancel it, and a
//! conflict or elevation prompt waits for the user. A stalled engine's thread is left behind (a COM call cannot be cut short); it is the
//! lesser evil to a frozen interface.
//!
//! TESTS that need the engine do not run without a shell window: [`not_run_without_shell`] says why and the test returns, so a run of
//! the suite always ends. [`with_no_shell`] makes the calling thread believe there is no shell window, so that the no-shell paths run for
//! real (on files the test creates in a temp folder) while Explorer is running.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The kinds of operation the shell engine does for the app.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Rename,
    PermanentDelete,
    RecycleDelete,
    Copy,
    Move,
    /// Restoring items from the Recycle Bin (also what undoing a delete does).
    RestoreFromBin,
    /// Deleting items inside the Recycle Bin for good.
    DeleteFromBin,
}

impl Op {
    /// For the English message.
    pub fn what(self) -> &'static str {
        match self {
            Op::Rename => "Renaming",
            Op::PermanentDelete => "Deleting some of these items",
            Op::RecycleDelete => "Moving to the Recycle Bin",
            Op::Copy => "This copy",
            Op::Move => "This move",
            Op::RestoreFromBin => "Restoring from the Recycle Bin",
            Op::DeleteFromBin => "Deleting in the Recycle Bin",
        }
    }
}

/// What to do with an operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The shell engine (a shell window exists).
    Shell,
    /// The app's own way (see the module).
    Own,
    /// Say plainly that it cannot be done now.
    Refuse,
}

/// The decision, pure.
pub fn plan(op: Op, shell_present: bool) -> Plan {
    if shell_present {
        return Plan::Shell;
    }
    match op {
        Op::Rename | Op::PermanentDelete => Plan::Own,
        Op::RecycleDelete | Op::Copy | Op::Move | Op::RestoreFromBin | Op::DeleteFromBin => Plan::Refuse,
    }
}

/// The operation needs the Windows shell and no shell is running. Nothing was changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoShellEngine {
    pub op: Op,
}

impl std::fmt::Display for NoShellEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} needs the Windows shell (Explorer), and no shell is running. Nothing was changed.", self.op.what())
    }
}

impl std::error::Error for NoShellEngine {}

/// The app's own engine did part of an operation and the rest needs the shell (which is not running), or could not be done. Unlike
/// [`NoShellEngine`] this is raised AFTER something was changed, so it never says "nothing was changed": the copies our engine made stay,
/// a move's verified sources are gone, and the user is told how much is left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartlyDone {
    pub op: Op,
    /// How many top-level items are left (`None`: not counted).
    pub left: Option<usize>,
}

impl std::fmt::Display for PartlyDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let items = match self.left {
            Some(1) => "1 item".to_string(),
            Some(n) => format!("{n} items"),
            None => "Some items".to_string(),
        };
        match self.op {
            Op::Copy | Op::Move => {
                let verb = if self.op == Op::Copy { "copied" } else { "moved" };
                write!(f, "{items} could not be {verb} because that needs the Windows shell (Explorer), and no shell is running. Everything else was done.")
            }
            _ => write!(f, "{items} could not be deleted (locked, read-only or access denied); the rest was deleted. No shell is running to show details."),
        }
    }
}

impl std::error::Error for PartlyDone {}

/// The shell engine did not start working within the limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineStalled {
    pub op: Op,
    pub waited: Duration,
}

impl std::fmt::Display for EngineStalled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "The Windows file engine did not start within {} s ({}). Explorer may have closed or hung; the operation may still finish, look at the engine's own window.",
            self.waited.as_secs(),
            self.op.what()
        )
    }
}

impl std::error::Error for EngineStalled {}

thread_local! {
    /// Tests: what this thread believes about the shell window (`None`: ask the system).
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether a shell window exists right now (`GetShellWindow`, one read, nothing is messaged).
pub fn shell_present() -> bool {
    if let Some(v) = FORCED.with(|f| f.get()) {
        return v;
    }
    use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
    !unsafe { GetShellWindow() }.0.is_null()
}

/// Runs `f` with this thread believing there is no shell window (or that there is). For tests of the no-shell paths.
pub fn with_forced_shell<R>(present: bool, f: impl FnOnce() -> R) -> R {
    struct Reset(Option<bool>);
    impl Drop for Reset {
        fn drop(&mut self) {
            FORCED.with(|c| c.set(self.0));
        }
    }
    let _reset = Reset(FORCED.with(|c| c.replace(Some(present))));
    f()
}

/// [`with_forced_shell`] with no shell.
pub fn with_no_shell<R>(f: impl FnOnce() -> R) -> R {
    with_forced_shell(false, f)
}

/// For a test that needs the shell engine: true (after printing why) when there is no shell window and the test must return at once.
pub fn not_run_without_shell(test: &str) -> bool {
    if shell_present() {
        return false;
    }
    eprintln!("NOT RUN: {test} needs the Windows shell (the engine of IFileOperation / the shell's window list), and no shell window exists on this machine now");
    true
}

/// How long the engine may stay without a first sign of life before the caller is told.
pub const START_LIMIT: Duration = Duration::from_secs(60);

/// Raised by the engine's progress sink; read by [`supervise`].
#[derive(Default)]
pub struct Life(AtomicBool);

impl Life {
    pub fn new() -> Arc<Life> {
        Arc::new(Life::default())
    }

    /// The engine did something.
    pub fn signal(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn alive(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Runs `job` on a thread of its own and waits for its result, but gives up with [`EngineStalled`] when `life` has not been signalled
/// within `limit`. `poll`: how often the wait looks (the real callers use 100 ms).
pub fn supervise_with<T: Send + 'static>(op: Op, life: Arc<Life>, limit: Duration, poll: Duration, job: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    let (tx, rx) = std::sync::mpsc::channel::<anyhow::Result<T>>();
    let spawned = std::thread::Builder::new().name("rf-shell-engine".into()).spawn(move || {
        // (a panic inside the job must reach the caller as an error and not as a closed channel that looks like a stall)
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)).unwrap_or_else(|_| Err(anyhow::anyhow!("the shell file engine's job panicked")));
        let _ = tx.send(r);
    });
    if let Err(e) = spawned {
        anyhow::bail!("the shell file engine's thread did not start: {e}");
    }
    let began = Instant::now();
    loop {
        match rx.recv_timeout(poll) {
            Ok(r) => return r,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if !life.alive() && began.elapsed() >= limit {
                    let waited = began.elapsed();
                    log::warn!("shell engine: {} did not start in {} s; the caller is told, the engine's thread is left", op.what(), waited.as_secs());
                    return Err(EngineStalled { op, waited }.into());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("the shell file engine's thread ended without an answer"),
        }
    }
}

/// [`supervise_with`] with the real limit and poll.
pub fn supervise<T: Send + 'static>(op: Op, life: Arc<Life>, job: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> anyhow::Result<T> {
    supervise_with(op, life, START_LIMIT, Duration::from_millis(100), job)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_a_shell_everything_goes_to_the_engine() {
        for op in [Op::Rename, Op::PermanentDelete, Op::RecycleDelete, Op::Copy, Op::Move, Op::RestoreFromBin, Op::DeleteFromBin] {
            assert_eq!(plan(op, true), Plan::Shell, "{op:?}");
        }
    }

    #[test]
    fn without_a_shell_each_kind_has_its_own_answer() {
        assert_eq!(plan(Op::Rename, false), Plan::Own, "a rename is a plain file call");
        assert_eq!(plan(Op::PermanentDelete, false), Plan::Own, "our parallel delete does it");
        for op in [Op::RecycleDelete, Op::Copy, Op::Move, Op::RestoreFromBin, Op::DeleteFromBin] {
            assert_eq!(plan(op, false), Plan::Refuse, "{op:?} must be refused, never started");
        }
    }

    #[test]
    fn a_partly_done_operation_never_says_nothing_was_changed() {
        let copy = PartlyDone { op: Op::Copy, left: Some(2) }.to_string();
        assert!(copy.starts_with("2 items could not be copied") && copy.contains("Everything else was done"), "{copy}");
        assert!(!copy.contains("Nothing was changed"));
        let mv = PartlyDone { op: Op::Move, left: Some(1) }.to_string();
        assert!(mv.starts_with("1 item could not be moved"), "{mv}");
        let del = PartlyDone { op: Op::PermanentDelete, left: None }.to_string();
        assert!(del.starts_with("Some items could not be deleted") && !del.contains("Nothing was changed"), "{del}");
    }

    #[test]
    fn the_messages_say_what_is_true() {
        let m = NoShellEngine { op: Op::RecycleDelete }.to_string();
        assert!(m.contains("Recycle Bin") && m.contains("no shell is running") && m.contains("Nothing was changed"), "{m}");
        let s = EngineStalled { op: Op::Copy, waited: Duration::from_secs(60) }.to_string();
        assert!(s.contains("60 s") && s.contains("may still finish"), "{s}");
    }

    #[test]
    fn a_job_that_answers_is_not_hurried() {
        let r = supervise_with(Op::Rename, Life::new(), Duration::from_millis(50), Duration::from_millis(5), || Ok(7));
        assert_eq!(r.unwrap(), 7);
        // alive: a long job after its first sign of life is waited for
        let life = Life::new();
        let l2 = life.clone();
        let r = supervise_with(Op::Copy, life, Duration::from_millis(50), Duration::from_millis(5), move || {
            l2.signal();
            std::thread::sleep(Duration::from_millis(300));
            Ok("done")
        });
        assert_eq!(r.unwrap(), "done", "no limit once the engine has started");
    }

    #[test]
    fn an_engine_that_never_starts_ends_the_wait_and_says_so() {
        let t = Instant::now();
        let r = supervise_with(Op::Rename, Life::new(), Duration::from_millis(100), Duration::from_millis(5), || {
            std::thread::sleep(Duration::from_secs(3));
            Ok(())
        });
        let e = r.expect_err("a stall");
        assert!(t.elapsed() < Duration::from_secs(2), "the caller was held for {:?}", t.elapsed());
        let stalled = e.downcast_ref::<EngineStalled>().expect("the typed error");
        assert_eq!(stalled.op, Op::Rename);
    }

    #[test]
    fn a_panic_and_a_job_error_come_back_as_errors() {
        let r = supervise_with::<()>(Op::Move, Life::new(), Duration::from_secs(5), Duration::from_millis(5), || panic!("boom"));
        assert!(r.unwrap_err().to_string().contains("panicked"));
        let r = supervise_with::<()>(Op::Move, Life::new(), Duration::from_secs(5), Duration::from_millis(5), || anyhow::bail!("the item is gone"));
        assert_eq!(r.unwrap_err().to_string(), "the item is gone");
    }

    #[test]
    fn the_forced_belief_is_scoped_to_the_closure_and_the_thread() {
        let real = shell_present();
        with_no_shell(|| {
            assert!(!shell_present());
            with_forced_shell(true, || assert!(shell_present()));
            assert!(!shell_present(), "the outer belief is back");
            // another thread is not fooled
            let other = std::thread::spawn(shell_present).join().unwrap();
            assert_eq!(other, real);
        });
        assert_eq!(shell_present(), real);
    }

    #[test]
    fn without_a_shell_a_test_that_needs_one_reports_itself_as_not_run() {
        with_no_shell(|| assert!(not_run_without_shell("a test")));
        with_forced_shell(true, || assert!(!not_run_without_shell("a test")));
    }
}
