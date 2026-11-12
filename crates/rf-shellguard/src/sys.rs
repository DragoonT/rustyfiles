//! The system side of the guard, as small traits. The real implementations live in `win/` and are
//! compiled into the binary; tests and `--dry-run` use the fakes in `fakes.rs` or no system at all.
//! Nothing in the state machine or the runner calls Windows directly.

use std::path::{Path, PathBuf};

use crate::machine::{Message, Millis};
use crate::statefile::{ShellStateFile, WorkAreaRec};

/// Wall clock, milliseconds since the Unix epoch.
pub trait Clock {
    fn now_ms(&self) -> Millis;
}

/// The one registry value the guard is about: `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell`.
/// The guard only ever reads or deletes it. Writing it is the Settings switch's job, in another program.
pub trait Registry {
    fn read_shell_value(&mut self) -> Result<Option<String>, String>;
    /// The machine policy `WindowsLogon/OverrideShellProgram` (Some(1) beats the user's value).
    fn read_override_policy(&mut self) -> Result<Option<u32>, String>;
    /// `Ok(true)` when it was there and is gone, `Ok(false)` when it was not there.
    fn delete_shell_value(&mut self) -> Result<bool, String>;
}

/// How to start the shell host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostLaunch {
    pub exe: PathBuf,
    pub args: Vec<String>,
    /// Name of the event the host signals when its desktop is usable. The guard creates it and hands the name over
    /// in the environment variable `RUSTYFILES_SHELL_READY_EVENT`.
    pub ready_event: String,
}

/// What ended a wait.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Wait {
    HostExited { pid: u32, code: i32 },
    HostReady,
    /// The extra waitable (the escape chord's event) was signalled.
    Extra,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PidWait {
    Gone,
    Extra,
    Timeout,
}

pub trait Processes {
    /// Creates the ready event, then starts the host. Returns its process id.
    fn spawn_host(&mut self, launch: &HostLaunch) -> Result<u32, String>;
    /// Waits for the host to exit or become ready, for `extra` (a waitable handle, 0 for none) or for the timeout.
    fn wait(&mut self, extra: isize, timeout_ms: Option<Millis>) -> Wait;
    /// Waits for any process id (the beside-guard watches a host it did not start). Process id 0 with no
    /// extra handle is a plain timed wait. A process that cannot be opened counts as gone.
    fn wait_pid(&mut self, pid: u32, extra: isize, timeout_ms: Option<Millis>) -> PidWait;
    /// Does a top-level window of the running host not answer?
    fn host_hung(&mut self) -> bool;
    fn kill_host(&mut self);
    /// A host that this process did not start but the state file says is still running (`--panic` after its guard
    /// died or hung): `kill_host` ends it, but only if the process with this id really runs `image`, because a
    /// process id can be reused. Without a call to this, `kill_host` ends only a host this process started.
    fn adopt_host(&mut self, pid: u32, image: &Path);
    /// The host binary exists and answers `--check`.
    fn host_self_test(&mut self, launch: &HostLaunch) -> bool;
    /// `explorer.exe` by its full path, never through PATH or App Paths.
    fn start_explorer(&mut self) -> Result<(), String>;
    /// Is there a visible desktop window owned by explorer.exe? Waits up to `wait_ms` for it to appear (0 = look once).
    fn explorer_shell_up(&mut self, wait_ms: Millis) -> bool;
    /// Looked at once, before the host is started: does Explorer already own this session's desktop? Always false in
    /// `--test-shell` mode, where a stand-in program takes Explorer's place.
    fn explorer_owns_shell_window(&mut self) -> bool;
}

/// The escape chord (Ctrl+Alt+Shift+E by default).
pub trait Hotkey {
    fn arm(&mut self) -> Result<(), String>;
    /// A handle `Processes::wait` can wait on (0 when there is none).
    fn waitable(&self) -> isize;
    /// Was the chord pressed (or a `--panic` process asked for it) since the last call? Consumes the press. The
    /// event behind `waitable` is manual-reset for this reason: the wait that woke the runner must not already have
    /// used the press up, or this call would see nothing.
    fn take_pressed(&mut self) -> bool;
}

pub trait Session {
    /// `GetSystemMetrics(SM_CLEANBOOT)` is not zero.
    fn safe_mode(&mut self) -> bool;
    fn shift_held(&mut self) -> bool;
    /// "26300.9974" (build and update revision).
    fn build(&mut self) -> Option<String>;
    /// Creates the session's `ShellReadyEvent`, not signalled. A second call is harmless.
    fn create_shell_ready_event(&mut self) -> Result<(), String>;
    fn signal_shell_ready(&mut self) -> Result<(), String>;
    /// Closes what `create_shell_ready_event` opened (and later the shell window shim).
    fn release_identity(&mut self);
    /// `--panic`: tell a guard that is already running to do the fall-back. True only if one took the request up (it
    /// resets the event when it does); a guard that is hung does not, and then the caller does the fall-back itself.
    fn signal_running_guard_panic(&mut self) -> bool;
    fn session_ending(&mut self) -> bool;
}

pub trait WorkAreas {
    fn restore(&mut self, saved: &[WorkAreaRec]) -> Result<(), String>;
}

/// The Windows taskbar's visibility (beside mode hides it; the guard shows it again).
pub trait TaskbarVisibility {
    /// How many taskbar windows were shown (0 when Explorer has none, e.g. it is not running).
    fn show_windows_taskbar(&mut self) -> Result<usize, String>;
}

pub trait StateStore {
    fn load(&mut self) -> ShellStateFile;
    fn save(&mut self, file: &ShellStateFile) -> Result<(), String>;
}

/// The only UI the guard has.
pub trait Ui {
    fn message(&mut self, m: Message);
}

/// Everything the runner may touch.
pub struct Sys<'a> {
    pub clock: &'a dyn Clock,
    pub registry: &'a mut dyn Registry,
    pub procs: &'a mut dyn Processes,
    pub hotkey: &'a mut dyn Hotkey,
    pub session: &'a mut dyn Session,
    pub workarea: &'a mut dyn WorkAreas,
    pub taskbar: &'a mut dyn TaskbarVisibility,
    pub store: &'a mut dyn StateStore,
    pub ui: &'a mut dyn Ui,
}
