//! Every way the shell host's process ends says why, in one line of the log, before it ends.
//!
//! WHY THIS EXISTS. On 2026-11-12 a dock that was started a moment after Explorer had been restarted wrote one line ("shell host: mode
//! beside") and was gone, with nothing to say what happened. Reading the start-up path by path, the process had SIX ways to end that this
//! file knows and only two of them wrote anything (a seventh row is the catch-all, [`Exit::RunReturned`]):
//!
//! | path | before | now |
//! |---|---|---|
//! | another host of the same group already runs: `dock::acquire()` is `None` and `lib.rs` simply `return`ed | nothing | [`Exit::AnotherHost`] |
//! | no monitor to put the dock on / no island could be opened (`dock::host::run`) | an error line without "exits" | [`Exit::NoMonitor`], [`Exit::NoIsland`] |
//! | a `stop` on the pipe (the main app's switch, an update, an outside caller) | "dock: stopped" without the cause | [`Exit::StopMessage`] |
//! | every island window closed for another reason (`on_window_closed`) | "dock: stopped" without the cause | [`Exit::IslandsClosed`] |
//! | the session ends (`WM_ENDSESSION`) | nothing | [`Exit::SessionEnding`] (written by `rf_win::dock::system`) |
//! | a panic (the release build aborts after the hook) | `crash.log` only | the same line in the log: [`on_panic`] |
//! | the application loop returns for a reason none of the rows above wrote (`lib.rs`, after `run`) | nothing | [`Exit::RunReturned`], written by [`after_run`] only when no other line was |
//!
//! WHICH ONE ENDED THE 2026-11-12 DOCK is not provable from here (the log of that run is not in this checkout). The first row fits every
//! fact told: it happens at the very start, right after "shell host: mode ..." and before anything else is logged, and it is the one path
//! that ends the process in silence. It also has a way to happen when Explorer was restarted just then: `single_instance::acquire_app`
//! greets the holder of the group's mutex through its pipe, and a previous host that is on its way out (a `stop` that was just handled)
//! still has both for a moment, so the new host takes the greeting for "another host runs" and leaves, and then the old one finishes
//! ending: no host at all. If that was it, the new line says "another host answered" at the time. Not changed here: the rule itself.
//!
//! A SECOND CANDIDATE for the same symptom, found when the change was read again: `acquire_app` takes a successful `WriteFile` to the
//! pipe for "the holder is alive". That proves a pipe instance exists, not that the process behind it is running: a dock that is hung
//! (blocked in a call to a dead or hung Explorer, say) still has the pipe and the mutex, takes the greeting, and the new host leaves.
//! The line [`Exit::AnotherHost`] alone does not tell the two apart (a host on its way out, or one that hangs); telling them would need
//! the holder's process id from the pipe and a check that it answers, which would change `single_instance`, shared with every app of the
//! bundle, and is not done here.
//!
//! What cannot speak: a kill (Task Manager, `taskkill /f`), a crash of the process in native code, a power cut. Nothing in-process can
//! write a line for those.

/// How the host ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// A host of the same single-instance group answered the greeting; this process leaves to it.
    AnotherHost,
    /// `stop` arrived on the pipe.
    StopMessage,
    /// The settings file says the Dock's switch is off, and it was on while this host ran (`dock::host::switch_stops`).
    SwitchOff,
    /// The session is ending.
    SessionEnding,
    /// The system has no monitor to put the dock on.
    NoMonitor,
    /// No island window could be opened.
    NoIsland,
    /// All the islands closed (something closed them), leaving only panels.
    IslandsClosed,
    /// A panic.
    Panic,
    /// The application loop ended and no other reason was written: the catch-all (see [`after_run`]).
    RunReturned,
}

impl Exit {
    pub const ALL: [Exit; 9] =
        [Exit::AnotherHost, Exit::StopMessage, Exit::SwitchOff, Exit::SessionEnding, Exit::NoMonitor, Exit::NoIsland, Exit::IslandsClosed, Exit::Panic, Exit::RunReturned];

    pub fn describe(self) -> &'static str {
        match self {
            Exit::AnotherHost => "another host of this group answered its greeting, so this one leaves to it",
            Exit::StopMessage => "a stop message arrived on the host's pipe",
            Exit::SwitchOff => "the Dock's switch is off in the settings file",
            Exit::SessionEnding => "the session is ending",
            Exit::NoMonitor => "this session has no monitor to put the dock on",
            Exit::NoIsland => "no island window could be opened",
            Exit::IslandsClosed => "all the island windows were closed",
            Exit::Panic => "a panic",
            Exit::RunReturned => "the application loop ended and nothing said why",
        }
    }
}

/// The log line: `shell host exits: <reason>[ (<detail>)]`.
pub fn line(exit: Exit, detail: &str) -> String {
    let detail = detail.trim();
    if detail.is_empty() { format!("shell host exits: {}", exit.describe()) } else { format!("shell host exits: {} ({detail})", exit.describe()) }
}

/// Writes the line (at warn level: visible with the default log filter, which hides info lines) and flushes nothing else; the log file
/// is appended whole line by line (`logfile::Tee`).
pub fn log_exit(exit: Exit, detail: &str) {
    WRITTEN.store(true, std::sync::atomic::Ordering::Relaxed);
    log::warn!("{}", line(exit, detail));
}

/// Whether a reason was written already (by any of the paths above).
static WRITTEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The catch-all line for the host process: `Some` when no reason was written. A normal stop writes its own line first, so the common
/// ends are not said twice.
pub fn catch_all_line(written: bool) -> Option<String> {
    (!written).then(|| line(Exit::RunReturned, ""))
}

/// Called when the application loop has returned in the host's process (`lib.rs`): says why when nothing else did. Other processes
/// (the main app) end as they always did.
pub fn after_run() {
    if crate::background::DOCK_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
        && let Some(l) = catch_all_line(WRITTEN.load(std::sync::atomic::Ordering::Relaxed))
    {
        log::warn!("{l}");
    }
}

/// From the panic hook: the host's process says it panicked, with the place and the message. Other processes (the main app) are left to
/// `crash.log` as before.
pub fn on_panic(info: &std::panic::PanicHookInfo<'_>) {
    if crate::background::DOCK_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
        log_exit(Exit::Panic, &info.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_way_out_has_its_own_reason() {
        let mut seen = std::collections::HashSet::new();
        for e in Exit::ALL {
            assert!(!e.describe().is_empty());
            assert!(seen.insert(e.describe()), "{e:?} repeats a reason");
        }
        assert_eq!(seen.len(), Exit::ALL.len());
    }

    #[test]
    fn the_catch_all_speaks_only_when_nothing_else_did() {
        assert_eq!(catch_all_line(false).as_deref(), Some("shell host exits: the application loop ended and nothing said why"));
        assert_eq!(catch_all_line(true), None, "a normal stop already wrote its own reason");
    }

    #[test]
    fn the_line_names_the_reason_and_the_detail() {
        assert_eq!(line(Exit::StopMessage, ""), "shell host exits: a stop message arrived on the host's pipe");
        assert_eq!(line(Exit::Panic, " src\\x.rs:1 boom "), "shell host exits: a panic (src\\x.rs:1 boom)");
    }

    /// Each reason is written from the place where that path ends (the source is searched, so a path cannot lose its line unnoticed).
    #[test]
    fn every_reason_is_written_where_the_process_ends() {
        let places: [(&str, &str); 6] = [
            ("AnotherHost", include_str!("../lib.rs")),
            ("StopMessage", include_str!("../dock/host.rs")),
            ("SwitchOff", include_str!("../dock/host.rs")),
            ("NoMonitor", include_str!("../dock/host.rs")),
            ("NoIsland", include_str!("../dock/host.rs")),
            ("IslandsClosed", include_str!("../dock/host.rs")),
        ];
        for (name, src) in places {
            assert!(src.contains(&format!("Exit::{name}")), "Exit::{name} is not written where that path ends");
        }
        assert!(include_str!("../lib.rs").contains("exits::on_panic"), "the panic hook does not write the line");
        assert!(include_str!("../lib.rs").contains("exits::after_run"), "the catch-all is not called after the application loop");
        assert!(include_str!("../../../rf-win/src/dock/system.rs").contains("the session is ending"), "the session end does not write its line");
    }
}
