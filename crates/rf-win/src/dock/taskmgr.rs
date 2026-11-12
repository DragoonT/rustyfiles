//! Starting Windows' Task Manager from the dock: a right click on the empty part of an island (`empty_spot`), as a right click on
//! Windows' taskbar offers it. This file is the decision that it starts ONCE and the way it is started; the call itself is
//! `launch::start` (the shell's "open", as a double click in Explorer would), made on the shell thread (`sta`) by `rf-app`.
//!
//! NO ELEVATION IS REQUESTED BY US. The verb is "open", never "runas": Task Manager's own manifest says what it needs, and the system
//! asks the user (or not) by its own rules, exactly as when the taskbar starts it. If Task Manager is already open, starting it again
//! brings the running one forward, which is what is wanted.

use std::path::{Path, PathBuf};

use super::model::Launch;

/// What is started: Windows' own Task Manager, the file of this name in the system folder.
pub const PROGRAM: &str = "taskmgr.exe";

/// The system folder from what was found: the known folder if it could be read, else `%SystemRoot%\System32`, else the usual place.
fn system_dir_from(known: Option<PathBuf>, system_root: Option<std::ffi::OsString>) -> PathBuf {
    known.filter(|p| p.is_absolute()).unwrap_or_else(|| {
        let root = system_root.map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        root.join("System32")
    })
}

/// Windows' system folder (`FOLDERID_System`, normally `C:\Windows\System32`).
pub fn system_dir() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_System, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
    let known = unsafe {
        SHGetKnownFolderPath(&FOLDERID_System, KF_FLAG_DEFAULT, None).ok().and_then(|p| {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            s.map(PathBuf::from)
        })
    };
    system_dir_from(known, std::env::var_os("SystemRoot"))
}

/// How the shell is asked: the program by its full path in the system folder, no arguments. A bare name would be looked for by the
/// shell's own search (the application's current folder among the places), so a `taskmgr.exe` put in a folder the dock runs from could
/// be started in place of Windows'.
pub fn launch_in(system_dir: &Path) -> Launch {
    Launch::Exe { path: system_dir.join(PROGRAM), args: String::new() }
}

/// [`launch_in`] of the real system folder.
pub fn launch() -> Launch {
    launch_in(&system_dir())
}

/// Windows' "the user cancelled" (`ERROR_CANCELLED`, 1223) as the error of a failed call: what the shell answers when the elevation
/// prompt is declined.
const CANCELLED: i32 = 0x800704C7_u32 as i32;

/// Whether a failed start only means that the user said no at the elevation prompt (Task Manager asks for it by its own manifest): that
/// is an answer, not a fault, and not worth a warning in the log.
pub fn was_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<windows::core::Error>().is_some_and(|w| w.code().0 == CANCELLED)
}

/// How long a start may hold the gate before it is taken as lost. The call does not return while the user answers an elevation prompt
/// (which closes itself after a couple of minutes), and a right click in that time must not queue another start behind it; this is only
/// the net for a call that never comes back, so that it does not shut the door for good.
pub const MAX_BUSY_MS: u64 = 300_000;
/// How long after a start returned that another right click is still not a new start: the window takes a moment to appear, and a
/// second right click in that moment (or a double click's second half) must not start another.
pub const COOLDOWN_MS: u64 = 2_000;

/// Lets one start through at a time and not again for a moment after. Times are milliseconds on one clock of the caller's choosing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StartGate {
    /// When the start in progress began, if one is.
    busy_since: Option<u64>,
    /// No new start before this time.
    quiet_until: u64,
}

impl StartGate {
    /// A gate that is open (const, so it can sit in a `static`).
    pub const fn new() -> StartGate {
        StartGate { busy_since: None, quiet_until: 0 }
    }

    /// A right click at `now`: true when Task Manager is to be started (the gate is closed behind it until [`StartGate::finished`]).
    pub fn begin(&mut self, now: u64) -> bool {
        if let Some(since) = self.busy_since
            && now.saturating_sub(since) < MAX_BUSY_MS
        {
            return false;
        }
        if now < self.quiet_until {
            return false;
        }
        self.busy_since = Some(now);
        true
    }

    /// The start call came back (well or badly) at `now`: the gate stays shut for the cooldown, then opens.
    pub fn finished(&mut self, now: u64) {
        self.busy_since = None;
        self.quiet_until = now + COOLDOWN_MS;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_right_click_starts_it_and_a_second_while_it_starts_does_not() {
        let mut g = StartGate::default();
        assert!(g.begin(1_000));
        assert!(!g.begin(1_050), "still starting");
        assert!(!g.begin(1_900));
    }

    #[test]
    fn after_it_started_the_gate_stays_shut_for_the_cooldown_and_then_opens() {
        let mut g = StartGate::default();
        assert!(g.begin(0));
        g.finished(300);
        assert!(!g.begin(301), "the window has not appeared yet");
        assert!(!g.begin(300 + COOLDOWN_MS - 1));
        assert!(g.begin(300 + COOLDOWN_MS), "a later right click starts it again (or brings the open one forward)");
    }

    #[test]
    fn a_start_that_never_came_back_does_not_shut_the_door_for_good() {
        let mut g = StartGate::default();
        assert!(g.begin(0));
        assert!(!g.begin(MAX_BUSY_MS - 1));
        assert!(g.begin(MAX_BUSY_MS));
    }

    #[test]
    fn a_start_waiting_for_the_user_at_an_elevation_prompt_keeps_the_door_shut() {
        let mut g = StartGate::default();
        assert!(g.begin(0));
        // a prompt left for a minute, or two: another right click must not queue a second start behind it
        assert!(!g.begin(60_000));
        assert!(!g.begin(120_000));
        // answered after two minutes: the call comes back, and the cooldown runs from there
        g.finished(125_000);
        assert!(!g.begin(125_001));
        assert!(g.begin(125_000 + COOLDOWN_MS));
    }

    #[test]
    fn a_declined_elevation_prompt_is_told_from_a_fault() {
        let hr = |v: u32| windows::core::Error::from(windows::core::HRESULT(v as i32));
        assert!(was_cancelled(&anyhow::Error::from(hr(0x800704C7))));
        // with a context added on the way up, as callers do
        assert!(was_cancelled(&anyhow::Error::from(hr(0x800704C7)).context("start")));
        assert!(!was_cancelled(&anyhow::Error::from(hr(0x80070002))));
        assert!(!was_cancelled(&anyhow::anyhow!("no such program")));
    }

    #[test]
    fn a_failed_start_also_waits_out_the_cooldown() {
        let mut g = StartGate::default();
        assert!(g.begin(10));
        g.finished(20);
        assert!(!g.begin(21));
        assert!(g.begin(20 + COOLDOWN_MS));
    }

    #[test]
    fn it_asks_the_shell_to_open_the_program_by_its_full_path_and_asks_for_nothing_else() {
        match launch_in(Path::new(r"C:\Windows\System32")) {
            Launch::Exe { path, args } => {
                assert_eq!(path, Path::new(r"C:\Windows\System32\taskmgr.exe"));
                assert!(args.is_empty());
            }
            other => panic!("{other:?}"),
        }
        // the real one: absolute, so that the shell does no search of its own, and the file is there
        match launch() {
            Launch::Exe { path, .. } => {
                assert!(path.is_absolute(), "{path:?}");
                assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("taskmgr.exe"));
                assert!(path.exists(), "{path:?}");
            }
            other => panic!("{other:?}"),
        }
        // the shell call that starts it uses the verb "open" only: a source check so that "runas" never creeps into the dock's start of
        // programs (`launch::open_with_shell`) without a test noticing
        let src = include_str!("launch.rs");
        assert!(src.contains("lpVerb: w!(\"open\")"));
        assert!(!src.contains("runas"));
    }

    #[test]
    fn the_system_folder_falls_back_to_something_absolute() {
        let known = PathBuf::from(r"D:\Win\System32");
        assert_eq!(system_dir_from(Some(known.clone()), None), known);
        assert_eq!(system_dir_from(None, Some(r"E:\WINNT".into())), PathBuf::from(r"E:\WINNT\System32"));
        assert_eq!(system_dir_from(None, None), PathBuf::from(r"C:\Windows\System32"));
        // a relative answer (a planted environment variable) is not used
        assert_eq!(system_dir_from(Some("System32".into()), Some(r"..\x".into())), PathBuf::from(r"C:\Windows\System32"));
    }
}
