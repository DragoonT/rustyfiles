//! Starting and stopping the dock process from the main rustyfiles, and the dock's side of that conversation.
//!
//! The dock is its own process (`rustyfiles.exe --dock`), a single instance of its own group ("rustyfiles-dock": a named mutex and a
//! named pipe, the same plumbing the main app uses, with the settings-folder suffix so a test copy has a dock of its own). The pipe is
//! also how it is found and told things: a one-word message per line.
//!
//! * `ping`    - "are you there?" (the answer is only that the pipe could be written to)
//! * `reload`  - the settings file changed: read it again
//! * `stop`    - undo everything and exit
//!
//! The dock KEEPS RUNNING when rustyfiles' window closes or rustyfiles quits: it is the user's taskbar. It stops only when the
//! switch in Settings goes off (a `stop` message), when it is asked to from outside the same way, or when Windows ends the session.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::settings::Settings;

/// The single-instance group of the dock.
pub const APP: &str = "rustyfiles-dock";

/// What a message on the dock's pipe asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Stop,
    Reload,
    /// "ping", the hello of a second `--dock` launch, or anything unknown: nothing to do.
    Nothing,
}

pub fn parse_command(line: &str) -> Command {
    match line.trim() {
        "stop" => Command::Stop,
        "reload" => Command::Reload,
        _ => Command::Nothing,
    }
}

/// What the main rustyfiles does about the dock for a given setting and a given state of the world.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Start,
    Stop,
    /// Running and wanted: tell it to read the settings again (a city or the reserved space may have changed).
    Reload,
    Nothing,
}

pub fn plan(wanted: bool, running: bool) -> Step {
    match (wanted, running) {
        (true, false) => Step::Start,
        (true, true) => Step::Reload,
        (false, true) => Step::Stop,
        (false, false) => Step::Nothing,
    }
}

/// `RUSTYFILES_NO_DOCK=1`: this rustyfiles never starts or stops a dock (automated runs, a second copy for testing).
fn disabled_by_environment() -> bool {
    std::env::var_os("RUSTYFILES_NO_DOCK").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Whether a dock process answers on its pipe right now.
pub fn is_running() -> bool {
    rf_win::single_instance::send_to(APP, "ping")
}

/// Starts the process. It outlives this one (its own process group, no console).
fn start_process() -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    let exe = std::env::current_exe()?;
    std::process::Command::new(exe)
        .arg(super::ARG)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .map(|_| ())
}

/// One decision at a time (the start-up thread and the settings thread may both come here).
static BUSY: Mutex<()> = Mutex::new(());

/// When this process last started a dock, and what that dock was told to read: a process that has just been started has no pipe
/// yet, and the "turn it off" that comes in that window must still reach it.
struct Told {
    started: Option<Instant>,
    keys: Option<Keys>,
}

static TOLD: Mutex<Told> = Mutex::new(Told { started: None, keys: None });

/// The settings the dock process acts on. A save that changes none of them does not need the dock to read the file again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub reserve: bool,
    pub city: String,
}

impl Keys {
    pub fn of(s: &Settings) -> Keys {
        Keys { reserve: s.dock_reserve, city: s.dock_weather_city.trim().to_string() }
    }
}

/// Whether a `reload` has to be sent: the dock has not been told these keys yet. Remembers them when it says yes.
pub fn needs_reload(told: &mut Option<Keys>, now: Keys) -> bool {
    if told.as_ref() == Some(&now) {
        return false;
    }
    *told = Some(now);
    true
}

/// How long after a start a dock may still be coming up (it has no pipe before that).
const STARTING: Duration = Duration::from_secs(8);

/// Whether a dock started at `started` can still be starting at `now`.
pub fn may_still_be_starting(started: Option<Instant>, now: Instant) -> bool {
    started.is_some_and(|t| now.saturating_duration_since(t) < STARTING)
}

/// Waits (a bounded time) for a dock that has been started to answer on its pipe. True when it does.
fn wait_until_running(started: Option<Instant>) -> bool {
    while may_still_be_starting(started, Instant::now()) {
        if is_running() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    is_running()
}

/// Brings the dock process in line with the settings: starts it when it is wanted and not there, stops it when it is there and not
/// wanted, tells it to read the file again when it stays and a setting it acts on changed. Blocks briefly (a pipe write, a process
/// spawn, at most a few seconds when a dock that was just started has to be stopped again): call it off the UI thread.
/// Does nothing in a side process (DirectLook, the dock itself) or when `RUSTYFILES_NO_DOCK` is set.
pub fn sync_process(s: &Settings) {
    if crate::background::side_process() || disabled_by_environment() {
        return;
    }
    let _one_at_a_time = BUSY.lock().unwrap_or_else(|p| p.into_inner());
    let mut running = is_running();
    if !running && !s.dock_enabled {
        // switched off while the process this rustyfiles started is still coming up: wait for its pipe, then stop it
        let started = TOLD.lock().unwrap_or_else(|p| p.into_inner()).started;
        if may_still_be_starting(started, Instant::now()) {
            running = wait_until_running(started);
        }
    }
    match plan(s.dock_enabled, running) {
        Step::Start => match start_process() {
            Ok(()) => {
                log::info!("dock: started");
                let mut told = TOLD.lock().unwrap_or_else(|p| p.into_inner());
                told.started = Some(Instant::now());
                // the new process reads the file itself
                told.keys = Some(Keys::of(s));
            }
            Err(e) => log::warn!("dock: cannot start: {e}"),
        },
        Step::Stop => {
            if rf_win::single_instance::send_to(APP, "stop") {
                log::info!("dock: asked to stop");
            }
            let mut told = TOLD.lock().unwrap_or_else(|p| p.into_inner());
            told.started = None;
            told.keys = None;
        }
        Step::Reload => {
            let send = needs_reload(&mut TOLD.lock().unwrap_or_else(|p| p.into_inner()).keys, Keys::of(s));
            if send {
                let _ = rf_win::single_instance::send_to(APP, "reload");
            }
        }
        Step::Nothing => {}
    }
}

/// Before an update or an uninstall replaces the program files: a running dock holds `rustyfiles.exe` open (it outlives the
/// window), so it is asked to stop and given a moment to let go. The settings switch stays as it is: the new version's start brings
/// the dock back. Blocks up to a few seconds: call it where waiting is fine (the update's restart, the uninstall hook).
pub fn stop_for_update() {
    if disabled_by_environment() || !is_running() {
        return;
    }
    if rf_win::single_instance::send_to(APP, "stop") {
        log::info!("dock: asked to stop for an update");
    }
    // the process ends itself: wait for its pipe to go
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(100));
        if !is_running() {
            break;
        }
    }
    let mut told = TOLD.lock().unwrap_or_else(|p| p.into_inner());
    told.started = None;
    told.keys = None;
}

/// At the start of the main rustyfiles: starts the dock when the setting is on (off the UI thread).
pub fn start_if_wanted(s: &Settings) {
    if !s.dock_enabled || crate::background::side_process() || disabled_by_environment() {
        return;
    }
    let s = s.clone();
    std::thread::spawn(move || sync_process(&s));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_are_understood_and_unknown_ones_are_harmless() {
        assert_eq!(parse_command("stop"), Command::Stop);
        assert_eq!(parse_command("stop\r"), Command::Stop);
        assert_eq!(parse_command("  reload "), Command::Reload);
        assert_eq!(parse_command("ping"), Command::Nothing);
        assert_eq!(parse_command("hello"), Command::Nothing);
        assert_eq!(parse_command(""), Command::Nothing);
        assert_eq!(parse_command("STOP now"), Command::Nothing);
    }

    #[test]
    fn what_to_do_about_the_dock() {
        assert_eq!(plan(true, false), Step::Start);
        assert_eq!(plan(true, true), Step::Reload);
        assert_eq!(plan(false, true), Step::Stop);
        assert_eq!(plan(false, false), Step::Nothing);
    }

    fn keys(reserve: bool, city: &str) -> Keys {
        Keys { reserve, city: city.into() }
    }

    #[test]
    fn the_dock_reads_the_file_again_only_when_a_setting_it_acts_on_changed() {
        let mut told = None;
        assert!(needs_reload(&mut told, keys(true, "Ankara")), "never told anything yet");
        assert!(!needs_reload(&mut told, keys(true, "Ankara")), "an unrelated save");
        assert!(needs_reload(&mut told, keys(false, "Ankara")));
        assert!(needs_reload(&mut told, keys(false, "")));
        assert!(!needs_reload(&mut told, keys(false, "")));
    }

    #[test]
    fn the_keys_are_trimmed_like_the_dock_trims_them() {
        let s = Settings { dock_weather_city: "  Erzincan ".into(), dock_reserve: false, ..Default::default() };
        assert_eq!(Keys::of(&s), keys(false, "Erzincan"));
    }

    #[test]
    fn a_dock_just_started_may_not_have_a_pipe_yet() {
        let t0 = Instant::now();
        assert!(!may_still_be_starting(None, t0));
        assert!(may_still_be_starting(Some(t0), t0 + Duration::from_secs(1)));
        assert!(!may_still_be_starting(Some(t0), t0 + Duration::from_secs(9)));
        // a clock that went back is not "starting for ever"
        assert!(may_still_be_starting(Some(t0 + Duration::from_secs(5)), t0));
    }
}
