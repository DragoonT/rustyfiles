//! The shell host's tray host, the app's side (shell plan increment D2; the design, the forwarding rule and the steps of a first
//! careful try are in `docs/design/tray-host.md`, the host itself is `rf_win::trayhost`).
//!
//! One setting, OFF by default: `shell_tray_host`. While it is on, and only inside the shell host process (this module is called from
//! `dock::host` only), the host owns a window of class `Shell_TrayWnd` in front of Explorer's, keeps a model of the icons programs
//! send to the notification area, and passes EVERY message on to Explorer's tray window untouched, returning Explorer's answer. The
//! dock does not show the icons yet (the strip is increment D3); [`view`] is what it will read.
//!
//! WHAT RUNS WHEN. Nothing exists while the setting is off: no window, no thread, no timer, no message. When it goes on the host makes
//! a [`TrayHost`] (a thread with its own message loop and the window) and one small reporting thread that writes the COUNTS to the
//! log; when it goes off, at quit ([`stop`], called by the dock's shutdown) and when the global is dropped, the window is destroyed
//! FIRST and programs are asked to add their icons again, so every one of them is back with Explorer. [`decide`] is the whole rule
//! for "what does a settings reload do", tested without starting anything: with the setting off the answer is "nothing" or "stop",
//! never "start".
//!
//! LOG-ONLY MODE: `RUSTYFILES_TRAY_LOG_ONLY=1` (exactly "1"). The window exists and everything is passed on, but no picture is read,
//! no tip, balloon text or program name is kept, the model stays empty, and the log gets counts. The dock shows nothing in any mode.
//!
//! LOGGING. Counts only: never a tip, a title, a text, a name or a path.
//!
//! Environment (for the first careful try, none of them needed): `RUSTYFILES_TRAY_LOG_ONLY=1`; `RUSTYFILES_TRAY_NO_START_BROADCAST=1`
//! (do not ask programs to add their icons again when the host starts: no message of ours reaches any program at start, the model
//! fills only from what programs send by themselves); `RUSTYFILES_TRAY_RECT_PACKING=axes|corners` (answer `Shell_NotifyIconGetRect`
//! from the interface's rectangles with that packing; unset, Explorer answers those queries); `RUSTYFILES_TRAY_VISIBLE=1` (make the
//! window visible as a click-through, almost transparent rectangle).

use std::time::{Duration, Instant};

use crossbeam_channel::{RecvTimeoutError, Sender};
use gpui_kit::*;
use rf_win::trayhost::{Counts, StallEvent, StallWatch, TrayHost, TrayHostConfig, TrayView};
use rf_win::tray_wire::rect::RectPacking;

use crate::settings::Settings;

pub const ENV_LOG_ONLY: &str = "RUSTYFILES_TRAY_LOG_ONLY";
pub const ENV_RECT_PACKING: &str = "RUSTYFILES_TRAY_RECT_PACKING";
pub const ENV_VISIBLE: &str = "RUSTYFILES_TRAY_VISIBLE";
pub const ENV_NO_START_BROADCAST: &str = "RUSTYFILES_TRAY_NO_START_BROADCAST";

/// A status line is written at most this often, and only if messages came since the last one.
const REPORT_EVERY: Duration = Duration::from_secs(30);
/// How often the reporting thread wakes: to look at the host thread's heartbeat and to see whether a line is due. It waits on the
/// quit channel meanwhile, so ending the host wakes it at once; nothing else wakes it, and it never reads the event hints (those are
/// the strip's).
const REPORT_POLL: Duration = Duration::from_secs(5);
/// The host thread ticks every 250 ms; this long without a tick is reported as a stall (once).
const STALL_AFTER_SECS: u64 = 15;

/// Whether the settings ask for the tray host.
pub fn wanted(s: &Settings) -> bool {
    s.shell_tray_host
}

/// Whether the value of [`ENV_LOG_ONLY`] switches log-only mode on: exactly "1".
pub fn log_only_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// Whether the value of [`ENV_VISIBLE`] makes the window visible: exactly "1".
pub fn visible_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// The packing named by [`ENV_RECT_PACKING`]; anything else is "not answered" (the careful default).
pub fn rect_packing_from(value: Option<&str>) -> Option<RectPacking> {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("axes") => Some(RectPacking::Axes),
        Some("corners") => Some(RectPacking::Corners),
        _ => None,
    }
}

/// Whether the value of [`ENV_NO_START_BROADCAST`] leaves the start broadcast out: exactly "1".
pub fn no_start_broadcast_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// The host's configuration from the four variables.
pub fn config_from(log_only: Option<&str>, packing: Option<&str>, visible: Option<&str>, no_start_broadcast: Option<&str>) -> TrayHostConfig {
    TrayHostConfig {
        log_only: log_only_from(log_only),
        rect_packing: rect_packing_from(packing),
        visible: visible_from(visible),
        broadcast_on_start: !no_start_broadcast_from(no_start_broadcast),
    }
}

/// What a settings reload does about the tray host.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Stop,
    Nothing,
}

/// The one rule: the host starts only when the setting asks for it and it is not running; it stops when the setting is off and it runs.
pub fn decide(want: bool, running: bool) -> Action {
    match (want, running) {
        (true, false) => Action::Start,
        (false, true) => Action::Stop,
        _ => Action::Nothing,
    }
}

/// Whether a status line is due: messages came since the last line, and the last line is old enough.
pub fn report_due(messages_now: u64, messages_at_last_line: u64, since_last_line: Duration) -> bool {
    messages_now != messages_at_last_line && since_last_line >= REPORT_EVERY
}

/// The line the reporting thread writes: counts, never a text.
pub fn status_line(counts: &Counts, icons: usize, log_only: bool) -> String {
    format!("tray host: {}", counts_phrase(counts, icons, log_only))
}

fn counts_phrase(counts: &Counts, icons: usize, log_only: bool) -> String {
    format!("{} {}; {}", icons, if log_only { "icons seen" } else { "icons held" }, counts.summary())
}

struct TrayGlobal {
    host: TrayHost,
    view: TrayView,
    log_only: bool,
    /// Nothing is ever sent on it: dropping it wakes the reporting thread (a disconnected channel) and ends it.
    quit: Sender<()>,
}

impl Global for TrayGlobal {}

/// The settings file was (re)read, or the host started: brings the tray host in line with the setting. Cheap when nothing changed.
pub fn on_settings(cx: &mut App) {
    let want = cx.try_global::<Settings>().is_some_and(wanted);
    match decide(want, cx.has_global::<TrayGlobal>()) {
        Action::Start => start(cx),
        Action::Stop => stop(cx),
        Action::Nothing => {}
    }
}

/// Destroys the window (first), asks programs to add their icons again, and ends the reporting thread. Safe to call at any time.
pub fn stop(cx: &mut App) {
    if !cx.has_global::<TrayGlobal>() {
        return;
    }
    let TrayGlobal { mut host, view, log_only, quit } = cx.remove_global::<TrayGlobal>();
    drop(quit);
    let (counts, icons) = (view.counts(), view.icon_count());
    host.stop();
    log::info!("tray host: stopped; {}", counts_phrase(&counts, icons, log_only));
}

/// What the dock reads (the strip, increment D3): `None` while the host is off.
pub fn view(cx: &App) -> Option<TrayView> {
    cx.try_global::<TrayGlobal>().map(|g| g.view.clone())
}

fn start(cx: &mut App) {
    let cfg = config_from(
        std::env::var(ENV_LOG_ONLY).ok().as_deref(),
        std::env::var(ENV_RECT_PACKING).ok().as_deref(),
        std::env::var(ENV_VISIBLE).ok().as_deref(),
        std::env::var(ENV_NO_START_BROADCAST).ok().as_deref(),
    );
    let log_only = cfg.log_only;
    log::info!(
        "tray host: starting (log only: {}, rectangle answers: {}, window visible: {}, start broadcast: {})",
        if log_only { "yes" } else { "no" },
        if cfg.rect_packing.is_some() { "ours" } else { "Explorer's" },
        if cfg.visible { "yes" } else { "no" },
        if cfg.broadcast_on_start { "yes" } else { "no" }
    );
    // (the real window is made here: never from a test)
    let host = match TrayHost::start(cfg) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("tray host: could not start: {e}");
            return;
        }
    };
    let view = host.view();
    let (quit, quit_rx) = crossbeam_channel::bounded::<()>(0);
    spawn_reporter(view.clone(), quit_rx, log_only);
    cx.set_global(TrayGlobal { host, view, log_only, quit });
}

/// One small thread. Every [`REPORT_POLL`] it (1) feeds the host thread's heartbeat to a stall watch, which logs once when the host
/// thread has not ticked for [`STALL_AFTER_SECS`] and once when it ticks again (a stuck host cannot be repaired from here: a window
/// can only be destroyed by its own thread; the line is there so that nobody has to guess), and (2) writes the counts when messages
/// came and [`REPORT_EVERY`] has passed since the last line. It sleeps on the quit channel in between and does not read the event hints.
fn spawn_reporter(view: TrayView, quit: crossbeam_channel::Receiver<()>, log_only: bool) {
    let _ = std::thread::Builder::new().name("rf-tray-report".into()).spawn(move || {
        let began = Instant::now();
        let mut watch = StallWatch::new(STALL_AFTER_SECS);
        let mut last_line = Instant::now();
        let mut messages_then = 0u64;
        // (nothing is ever sent on the channel: anything but a timeout, a disconnect, is the signal to end)
        while let Err(RecvTimeoutError::Timeout) = quit.recv_timeout(REPORT_POLL) {
            match watch.observe(view.heartbeat(), began.elapsed().as_secs()) {
                Some(StallEvent::Stalled { secs }) => {
                    log::error!("tray host: the host thread has not ticked for {secs} s (stuck or very busy); its window can only go when it does or the process ends");
                }
                Some(StallEvent::Recovered) => {
                    log::info!("tray host: the host thread ticks again");
                }
                None => {}
            }
            let counts = view.counts();
            if report_due(counts.messages, messages_then, last_line.elapsed()) {
                log::info!("{}", status_line(&counts, view.icon_count(), log_only));
                messages_then = counts.messages;
                last_line = Instant::now();
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Action, REPORT_EVERY, Settings, config_from, decide, log_only_from, no_start_broadcast_from, rect_packing_from, report_due, status_line, visible_from, wanted};
    use rf_win::trayhost::Counts;
    use rf_win::tray_wire::rect::RectPacking;

    #[test]
    fn the_setting_is_off_by_default_and_round_trips() {
        assert!(!wanted(&Settings::default()), "experimental: off until asked for");
        assert!(!wanted(&Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap()));
        let on = Settings { shell_tray_host: true, ..Default::default() };
        let text = toml::to_string(&on).unwrap();
        assert!(wanted(&Settings::parse_lenient_pub(&text).unwrap()));
        assert!(!wanted(&Settings::parse_lenient_pub("shell_tray_host = \"evet\"\n").unwrap()), "a wrong value falls back to off");
    }

    #[test]
    fn only_a_one_switches_the_log_only_and_visible_options_on() {
        for f in [log_only_from, visible_from, no_start_broadcast_from] {
            assert!(f(Some("1")));
            assert!(f(Some(" 1 ")));
            assert!(!f(Some("0")));
            assert!(!f(Some("true")));
            assert!(!f(Some("")));
            assert!(!f(None));
        }
    }

    #[test]
    fn the_packing_is_named_or_not_answered() {
        assert_eq!(rect_packing_from(Some("axes")), Some(RectPacking::Axes));
        assert_eq!(rect_packing_from(Some(" Corners ")), Some(RectPacking::Corners));
        assert_eq!(rect_packing_from(Some("")), None);
        assert_eq!(rect_packing_from(Some("1")), None);
        assert_eq!(rect_packing_from(None), None);
    }

    #[test]
    fn the_default_configuration_is_the_careful_one() {
        let c = config_from(None, None, None, None);
        assert!(!c.log_only && !c.visible && c.rect_packing.is_none());
        assert!(c.broadcast_on_start, "the default asks programs to add their icons again, as the model needs");
        let c = config_from(Some("1"), Some("axes"), None, Some("1"));
        assert!(c.log_only);
        assert_eq!(c.rect_packing, Some(RectPacking::Axes));
        assert!(!c.broadcast_on_start, "the first careful run can leave the start broadcast out");
    }

    #[test]
    fn with_the_setting_off_a_reload_never_starts_anything() {
        assert_eq!(decide(false, false), Action::Nothing);
        assert_eq!(decide(false, true), Action::Stop);
        assert_eq!(decide(true, false), Action::Start);
        assert_eq!(decide(true, true), Action::Nothing);
        // and the setting is off unless the file says so
        assert!(!wanted(&Settings::default()));
    }

    #[test]
    fn a_status_line_is_due_only_for_new_messages_and_not_more_often_than_every_30_seconds() {
        assert!(!report_due(5, 5, REPORT_EVERY * 10), "nothing came");
        assert!(!report_due(6, 5, REPORT_EVERY / 2), "too soon");
        assert!(report_due(6, 5, REPORT_EVERY));
        assert!(!report_due(0, 0, REPORT_EVERY), "an idle host writes no line at all");
    }

    #[test]
    fn the_status_line_has_counts_and_nothing_else() {
        let c = Counts { adds: 3, forwarded: 5, ..Default::default() };
        assert_eq!(status_line(&c, 3, false), "tray host: 3 icons held; adds 3, forwarded 5");
        assert_eq!(status_line(&Counts::default(), 0, true), "tray host: 0 icons seen; nothing yet");
    }

    fn code() -> String {
        let src = include_str!("trayhost.rs");
        src.split("#[cfg(test)]").next().unwrap_or(src).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn no_log_line_prints_a_tip_a_title_a_name_or_a_path() {
        let code = code();
        for stmt in code.split("log::").skip(1) {
            let line = stmt.split(");").next().unwrap_or("");
            for banned in ["tip", "title", "name", "path", "exe", "text", "snapshot", "icons.", "{e:?}"] {
                assert!(!line.contains(banned), "a log line mentions `{banned}`: {line}");
            }
        }
    }

    #[test]
    fn the_app_side_calls_no_window_function_of_its_own() {
        // the window, the messages and the broadcast are the host's (rf_win::trayhost::win); this file only starts and stops it
        let code = code();
        for banned in ["SetWindowsHookEx", "SendInput", "RegisterHotKey", "SetWindowPos", "ShowWindow", "PostMessage", "SendMessage", "CreateWindow", "windows::", "Shell_TrayWnd"] {
            assert!(!code.contains(banned), "{banned}");
        }
    }

    #[test]
    fn no_test_starts_the_host() {
        let src = include_str!("trayhost.rs");
        let tests = src.split("#[cfg(test)]").nth(1).unwrap_or_default();
        let tests: String = tests.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        // (the needles are built at run time, so that this file does not contain them in its tests)
        for needle in [concat!("TrayHost::", "start("), concat!("fn ", "start("), concat!("on_", "settings(")] {
            assert!(!tests.contains(needle), "a test mentions {needle}");
        }
    }
}
