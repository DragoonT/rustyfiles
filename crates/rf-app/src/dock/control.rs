//! Starting and stopping the dock process from the main rustyfiles, and the dock's side of that conversation.
//!
//! The dock is its own process (`rustyfiles.exe --dock`), a single instance of its own group ("rustyfiles-dock": a named mutex and a
//! named pipe, the same plumbing the main app uses, with the settings-folder suffix so a test copy has a dock of its own). The pipe is
//! also how it is found and told things: a one-word message per line.
//!
//! * `ping`    - "are you there?" (the answer is only that the pipe could be written to)
//! * `reload`  - the settings file changed: read it again
//! * `stop`    - undo everything and exit
//! * `size <stamp> <permille> <0|1>` - the Dock page's slider moved (1: it was let go): follow it now, without waiting for the file
//!   (`rf_win::dock::size_live`)
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
    /// The size slider moved or was let go.
    Size(rf_win::dock::size_live::Msg),
    /// "ping", the hello of a second `--dock` launch, or anything unknown: nothing to do.
    Nothing,
}

pub fn parse_command(line: &str) -> Command {
    match line.trim() {
        "stop" => Command::Stop,
        "reload" => Command::Reload,
        other => rf_win::dock::size_live::parse_message(other).map_or(Command::Nothing, Command::Size),
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

/// Whether this process may send the size slider's numbers to a dock: not a side process, not when `RUSTYFILES_NO_DOCK` is set, and not
/// when `RUSTYFILES_CONFIG_DIR` is (a test copy). WHY THE LAST: the pipe's name is the same for every copy, so a test copy would resize
/// the user's own dock with a number from its own test settings, and the user's dock, reading its real file, would keep that number for
/// a while. `sync_process` only asks the dock to read the file again, which can do no such harm.
pub fn may_send_size() -> bool {
    may_send_size_in(crate::background::side_process(), disabled_by_environment(), std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some_and(|d| !d.is_empty()))
}

fn may_send_size_in(side_process: bool, no_dock: bool, config_dir_override: bool) -> bool {
    !side_process && !no_dock && !config_dir_override
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

/// The settings the dock process acts on. A save that changes none of them does not need the dock to read the file again. The look of
/// the glass (the four HyperGlass sliders), the growth of the icons and the animations setting are the dock's too: it draws its own
/// glass with the engine's process-wide look and decides by the animations setting whether sizes ease or jump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Keys {
    pub reserve: bool,
    pub city: String,
    /// The weather from the PC's position: the running dock must start (or stop) asking Windows for it.
    pub weather_location: bool,
    /// Frost, tint, refraction, saturation.
    pub look: [u8; 4],
    pub magnify: crate::settings::DockMagnify,
    pub animations: crate::settings::AnimPref,
    /// Whether the logo and the search field open our own Start panel, and the right island's symbols our own control centre: the dock
    /// reads both from the file when it opens a panel, so a switch in Settings must reach it.
    pub own_start: bool,
    pub own_control_centre: bool,
    /// The clock opens our own calendar and the bell our own notification panel (else Win+N): read from the file when they open.
    pub own_calendar: bool,
    pub own_notifications: bool,
    /// The previews and the menu of the middle island's icons: read from the file when they are asked for.
    pub hover_previews: bool,
    pub app_menu: bool,
    /// The middle island is thin while a window is maximised: the running dock starts or stops following the windows when it reads the file.
    pub thin_when_maximized: bool,
    /// The Start panel's "Önerilenler" section: read from the settings when the panel opens.
    pub start_recommended: bool,
    /// The Start panel grows out of the left island: read from the settings when the panel opens.
    pub start_grow: bool,
    /// The dock's size: the islands, the icons and everything else change with it, so the running dock must read the file again.
    pub size: super::size_pref::DockSizePref,
    /// Which of the dock's five parts run (`parts::run_flags`), in this order: desktop cover, hiding Windows' taskbar, the Windows key,
    /// Alt+Tab, the tray host. The dock starts or stops each of them when it reads the file, so the reload must not wait for its 5 s
    /// look at the file's date. These are the RESULT of the one switch and each part's own value, so the one switch moves all five.
    pub shell: [bool; 5],
}

impl Keys {
    pub fn of(s: &Settings) -> Keys {
        Keys {
            reserve: s.dock_reserve,
            city: s.dock_weather_city.trim().to_string(),
            weather_location: s.dock_weather_location,
            look: [s.hg_frost, s.hg_tint, s.hg_refraction, s.hg_saturation],
            magnify: s.dock_magnify,
            animations: s.animations,
            own_start: s.dock_own_start,
            own_control_centre: s.dock_own_control_centre,
            own_calendar: s.dock_own_calendar,
            own_notifications: s.dock_own_notifications,
            hover_previews: s.dock_hover_previews,
            app_menu: s.dock_app_menu,
            thin_when_maximized: s.dock_thin_when_maximized,
            start_recommended: s.dock_start_recommended,
            start_grow: s.dock_start_grow,
            size: s.dock_size,
            shell: super::parts::run_flags(s),
        }
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
    fn only_the_real_settings_may_resize_the_real_dock() {
        assert!(may_send_size_in(false, false, false));
        assert!(!may_send_size_in(true, false, false), "a side process");
        assert!(!may_send_size_in(false, true, false), "RUSTYFILES_NO_DOCK");
        assert!(!may_send_size_in(false, false, true), "a test copy with its own settings folder");
    }

    #[test]
    fn a_size_message_is_a_command_and_a_malformed_one_is_harmless() {
        use rf_win::dock::size_live::Msg;
        assert_eq!(parse_command("size 12 1250 0"), Command::Size(Msg { stamp: 12, permille: 1250, released: false }));
        assert_eq!(parse_command("size 13 1250 1\r"), Command::Size(Msg { stamp: 13, permille: 1250, released: true }));
        for bad in ["size", "size 1", "size 1 2", "size a b c", "size 1 1000 5", "sizes 1 1000 0"] {
            assert_eq!(parse_command(bad), Command::Nothing, "{bad}");
        }
    }

    #[test]
    fn what_to_do_about_the_dock() {
        assert_eq!(plan(true, false), Step::Start);
        assert_eq!(plan(true, true), Step::Reload);
        assert_eq!(plan(false, true), Step::Stop);
        assert_eq!(plan(false, false), Step::Nothing);
    }

    fn keys(reserve: bool, city: &str) -> Keys {
        Keys { reserve, city: city.into(), weather_location: false, look: [52, 50, 67, 62], magnify: crate::settings::DockMagnify::Full, animations: crate::settings::AnimPref::System, own_start: true, own_control_centre: true, own_calendar: true, own_notifications: true, hover_previews: true, app_menu: true, thin_when_maximized: true, start_recommended: true, start_grow: true, size: Default::default(), shell: [false; 5] }
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

    /// The glass of the dock is drawn by the dock process with the look it read from the file: a slider, the growth of the icons or the
    /// animations setting must make the dock read the file again, or it would hold the old look until its next look at the file's date.
    #[test]
    fn the_look_the_growth_and_the_animations_are_the_docks_to_follow() {
        let base = Settings::default();
        let k = Keys::of(&base);
        let mut told = Some(k.clone());
        assert!(!needs_reload(&mut told, Keys::of(&base)), "nothing changed");
        for change in [
            Settings { hg_frost: 80, ..base.clone() },
            Settings { hg_tint: 10, ..base.clone() },
            Settings { hg_refraction: 100, ..base.clone() },
            Settings { hg_saturation: 0, ..base.clone() },
            Settings { dock_magnify: crate::settings::DockMagnify::Off, ..base.clone() },
            Settings { animations: crate::settings::AnimPref::Off, ..base.clone() },
            Settings { dock_own_start: false, ..base.clone() },
            Settings { dock_own_control_centre: false, ..base.clone() },
            Settings { dock_own_calendar: false, ..base.clone() },
            Settings { dock_own_notifications: false, ..base.clone() },
            Settings { dock_hover_previews: false, ..base.clone() },
            Settings { dock_app_menu: false, ..base.clone() },
            Settings { dock_thin_when_maximized: false, ..base.clone() },
            Settings { dock_start_recommended: false, ..base.clone() },
            Settings { dock_start_grow: false, ..base.clone() },
            Settings { dock_size: crate::dock::size_pref::DockSizePref::from_permille(1400), ..base.clone() },
            Settings { dock_size: crate::dock::size_pref::DockSizePref::from_permille(1050), ..base.clone() },
            Settings { dock_weather_location: true, ..base.clone() },
        ] {
            let mut told = Some(k.clone());
            assert!(needs_reload(&mut told, Keys::of(&change)), "{:?}", Keys::of(&change));
        }
        // the surface wash is the app's own business: the dock has none
        let mut told = Some(k);
        assert!(!needs_reload(&mut told, Keys::of(&Settings { hg_surface: 99, ..base })));
    }

    /// The five parts follow the ONE switch and their own values: a change in what runs must reach the running dock at once, and a
    /// change that leaves what runs as it was (an override of a part while the switch is off) need not.
    #[test]
    fn the_dock_reads_the_file_again_when_what_runs_changes_and_not_otherwise() {
        use super::super::parts::Part;
        let base = Settings { dock_enabled: true, ..Default::default() };
        let mut told = Some(Keys::of(&base));
        assert!(!needs_reload(&mut told, Keys::of(&base)));
        // the switch moves all five at once
        let mut told = Some(Keys::of(&base));
        assert!(needs_reload(&mut told, Keys::of(&Settings { dock_enabled: false, ..base.clone() })));
        assert_eq!(Keys::of(&Settings { dock_enabled: false, ..base.clone() }).shell, [false; 5]);
        assert_eq!(Keys::of(&base).shell, [true; 5]);
        // each part's own value moves its own flag while the switch is on
        for (i, p) in Part::ALL.into_iter().enumerate() {
            let mut s = base.clone();
            p.set_own_value(&mut s, false);
            let mut told = Some(Keys::of(&base));
            assert!(needs_reload(&mut told, Keys::of(&s)), "{p:?}");
            let mut want = [true; 5];
            want[i] = false;
            assert_eq!(Keys::of(&s).shell, want);
            // with the switch off the override changes nothing that runs: no reload
            let off = Settings { dock_enabled: false, ..Default::default() };
            let mut off_held = off.clone();
            p.set_own_value(&mut off_held, false);
            let mut told = Some(Keys::of(&off));
            assert!(!needs_reload(&mut told, Keys::of(&off_held)), "{p:?}: nothing runs either way");
        }
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
