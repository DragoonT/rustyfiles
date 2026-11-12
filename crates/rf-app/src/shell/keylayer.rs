//! The shell host's opt-in keyboard layer, the app's side (shell plan increment B7; the rules and the design are in
//! `docs/design/shell-keys.md`, the hook and its callback are `rf_win::winkey`).
//!
//! Two settings, both OFF by default, each one switch on the dock's Settings page:
//! * `shell_win_key`: a tap of the Windows key alone toggles OUR Start panel (`dock::toggle_start`) instead of Windows' Start;
//! * `shell_own_switcher`: Alt+Tab opens OUR switcher (`shell::switcher::open` with `hooked: true`) and the chord's effects drive it
//!   (`apply_effect`); `on_closed` tells the chord when the switcher closes by itself.
//!
//! WHAT RUNS WHEN. Nothing exists while both settings are off: no hook, no thread, no timer, no task. When the first goes on the host
//! makes a [`KeysHost`] (the service with the one keyboard hook, the dispatcher thread that carries the callback's posts, and two
//! tasks: one that receives the posts, one that beats the heartbeat every 250 ms); when both are off, at quit ([`stop`], called by the
//! dock's shutdown) and when the global is dropped, all of it is removed.
//!
//! FAIL OPEN. The beat is what proves this thread is alive: when it stops (this thread stuck for a second) the hook passes every key
//! (`rf_win::winkey::shared`). A full-screen program in front stands the layer down too (`dock::is_fullscreen`, stamped with the beat).
//!
//! LOGGING. Lines say what was DECIDED ("start: toggled", "switcher: opened"), never a key code. `RUSTYFILES_KEYS_LOG_ONLY=1` makes
//! the layer decide and log but swallow nothing, send nothing and open nothing: the first thing to try (the log says "would ...").
//!
//! The decision of what to do with a post is [`handle`], over the [`Actions`] trait; the tests drive it with a fake and with the real
//! service over a fake feed. The real [`GpuiActions`] is exercised only in a live run.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use gpui_kit::*;
use rf_win::switcher::chord::{Effect, Scope};
use rf_win::switcher::hook::LowLevelHook;
use rf_win::winkey::{Beat, Deliver, HEARTBEAT_EVERY_MS, KeyService, Posted, Shared};

use super::switcher::{self, Closing, OpenRequest};
use crate::settings::Settings;

/// `RUSTYFILES_KEYS_LOG_ONLY=1`: decide and log, swallow nothing, send nothing, open nothing.
pub const ENV_LOG_ONLY: &str = "RUSTYFILES_KEYS_LOG_ONLY";

/// Whether the value of [`ENV_LOG_ONLY`] switches log-only mode on: exactly "1".
pub fn log_only_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// Which of the two layers the settings ask for: (Windows key, Alt+Tab).
pub fn wanted(s: &Settings) -> (bool, bool) {
    (s.shell_win_key, s.shell_own_switcher)
}

// ---------------------------------------------------------------- what a post does (pure over a trait)

/// What the host can be asked to do with a post.
pub trait Actions {
    /// Opens the Start panel, or closes it when it is open. False when it could not be opened.
    fn toggle_start(&mut self) -> bool;
    /// Asks Windows to open ITS Start (a tap of the Windows key, sent as an injected event, which the layer ignores). The way out when
    /// our panel could not open after the layer had already kept Windows' Start shut for this tap.
    fn windows_start(&mut self);
    /// Opens the switcher for the opening numbered `seq`. False when it could not be opened.
    fn open_switcher(&mut self, scope: Scope, backwards: bool, seq: u16) -> bool;
    /// An effect of the chord, for the open switcher.
    fn apply(&mut self, effect: Effect);
    /// Closes the switcher if one is open and says whether there was one.
    fn close_switcher(&mut self) -> bool;
    /// A log line (what was decided).
    fn log(&mut self, line: &str);
}

/// Does what a post asks. In log-only mode nothing is done and the line says what would have been.
pub fn handle(p: Posted, log_only: bool, shared: &Shared, a: &mut dyn Actions) {
    match p {
        Posted::StartTap => {
            if log_only {
                a.log("start: would toggle (log only)");
            } else if a.toggle_start() {
                a.log("start: toggled");
            } else {
                // the mask key already kept Windows' Start shut for this tap: without this the Windows key would do nothing at all
                a.windows_start();
                a.log("start: our panel could not be opened; Windows' Start was asked for");
            }
        }
        Posted::Chord { effect: Effect::Open { scope, backwards }, seq } => {
            if log_only {
                a.log("switcher: would open (log only)");
            } else if a.open_switcher(scope, backwards, seq) {
                a.log("switcher: opened");
            } else {
                // the swallowed chord is given up at once: Alt+Tab goes to Windows until Alt is released
                shared.give_up(seq);
                a.log("switcher: could not be opened; the chord was given up");
            }
        }
        Posted::Chord { effect, .. } => {
            // (what is said about the two effects that end a chord: the verb for log-only, the verb for done)
            let what = match effect {
                Effect::Choose => Some(("choose", "chose")),
                Effect::Cancel => Some(("cancel", "cancelled")),
                _ => None,
            };
            if log_only {
                if let Some((would, _)) = what {
                    a.log(&format!("switcher: would {would} (log only)"));
                }
                return;
            }
            a.apply(effect);
            if let Some((_, done)) = what {
                a.log(&format!("switcher: {done}"));
            }
        }
        Posted::Lost => {
            if !log_only && a.close_switcher() {
                a.log("switcher: closed (the keyboard layer lost track of the chord)");
            }
        }
    }
}

// ---------------------------------------------------------------- the real actions

struct GpuiActions<'a> {
    cx: &'a mut App,
    shared: Arc<Shared>,
}

impl Actions for GpuiActions<'_> {
    fn toggle_start(&mut self) -> bool {
        crate::dock::toggle_start(self.cx)
    }

    fn windows_start(&mut self) {
        // (the dock's own helper for the logo when our Start is switched off; injected, so the layer does not answer it)
        rf_win::dock::triggers::start_menu();
    }

    fn open_switcher(&mut self, scope: Scope, backwards: bool, seq: u16) -> bool {
        let shared = self.shared.clone();
        let req = OpenRequest {
            scope,
            backwards,
            hooked: true,
            // (however the switcher closes, the chord of this opening is over: without this it would go on swallowing Tab, the arrows,
            // Enter and Escape for a switcher that is gone)
            on_closed: Some(Rc::new(move |_how: Closing, _cx: &mut App| shared.chord_closed(seq))),
        };
        switcher::open(req, self.cx)
    }

    fn apply(&mut self, effect: Effect) {
        switcher::apply_effect(effect, self.cx);
    }

    fn close_switcher(&mut self) -> bool {
        if !switcher::is_open(self.cx) {
            return false;
        }
        switcher::close(Closing::Cancelled, self.cx);
        true
    }

    fn log(&mut self, line: &str) {
        log::info!("{line}");
    }
}

// ---------------------------------------------------------------- the host

struct KeysHost {
    service: KeyService<LowLevelHook>,
    log_only: bool,
    // (kept alive: dropping a task stops it)
    _beat: Task<()>,
    _posts: Task<()>,
}

impl Global for KeysHost {}

/// The settings file was (re)read, or the host started: brings the layer in line with the two settings. Cheap when nothing changed.
pub fn on_settings(cx: &mut App) {
    let (win, alt) = cx.try_global::<Settings>().map_or((false, false), wanted);
    if !win && !alt {
        stop(cx);
        return;
    }
    if !cx.has_global::<KeysHost>() {
        start(cx);
    }
    if let Err(e) = cx.global_mut::<KeysHost>().service.set_wanted(win, alt) {
        log::warn!("keys: the keyboard layer could not start: {e:#}");
    }
}

/// Removes the hook and everything that served it (both settings off, or the host ends). Safe to call at any time.
pub fn stop(cx: &mut App) {
    if !cx.has_global::<KeysHost>() {
        return;
    }
    let mut host = cx.remove_global::<KeysHost>();
    host.service.stop();
}

fn start(cx: &mut App) {
    let log_only = log_only_from(std::env::var(ENV_LOG_ONLY).ok().as_deref());
    if log_only {
        log::info!("keys: log-only mode ({ENV_LOG_ONLY}): decisions are logged, no key is swallowed or sent, nothing is opened");
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Posted>();
    let deliver: Deliver = Arc::new(move |p| {
        let _ = tx.unbounded_send(p);
    });
    let service = KeyService::system(deliver, log_only);
    let posts = cx.spawn(async move |cx| {
        while let Some(p) = rx.next().await {
            cx.update(|cx| on_posted(p, cx));
        }
    });
    let beat = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(HEARTBEAT_EVERY_MS)).await;
            if !cx.update(beat_once) {
                break;
            }
        }
    });
    cx.set_global(KeysHost { service, log_only, _beat: beat, _posts: posts });
}

/// One beat: proves this thread is alive to the hook and lets the service look at the hook. False when the layer is gone.
fn beat_once(cx: &mut App) -> bool {
    if !cx.has_global::<KeysHost>() {
        return false;
    }
    let fullscreen = crate::dock::is_fullscreen(cx);
    if matches!(cx.global_mut::<KeysHost>().service.heartbeat(fullscreen), Beat::GaveUp) {
        log::debug!("keys: the layer has given up");
    }
    true
}

fn on_posted(p: Posted, cx: &mut App) {
    let Some((log_only, shared)) = cx.try_global::<KeysHost>().and_then(|h| h.service.shared().map(|s| (h.log_only, s))) else { return };
    let mut actions = GpuiActions { cx, shared: shared.clone() };
    handle(p, log_only, &shared, &mut actions);
}

#[cfg(test)]
mod tests {
    // (explicit names: `super::*` would bring in the toolkit's own `test` attribute, which replaces the standard one)
    use super::{Actions, Settings, handle, log_only_from, wanted};
    use rf_win::switcher::chord::{Effect, Scope};
    use rf_win::winkey::{Deliver, KeyService, Posted, Shared};
    use std::sync::Arc;
    use std::time::Duration;
    use rf_win::switcher::chord::Dir;
    use rf_win::switcher::hook::{FakeFeed, LLKHF_ALTDOWN, VK_LMENU, VK_TAB};
    use rf_win::winkey::fakes::{FakeMask, FakeProbe};
    use rf_win::winkey::{Clock, KeyProbe};

    #[derive(Default)]
    struct Fake {
        started: u32,
        windows_started: u32,
        /// Whether our Start panel can open.
        start_works: bool,
        opened: Vec<(Scope, bool, u16)>,
        applied: Vec<Effect>,
        closed: u32,
        lines: Vec<String>,
        can_open: bool,
        switcher_up: bool,
    }

    impl Actions for Fake {
        fn toggle_start(&mut self) -> bool {
            self.started += 1;
            self.start_works
        }
        fn windows_start(&mut self) {
            self.windows_started += 1;
        }
        fn open_switcher(&mut self, scope: Scope, backwards: bool, seq: u16) -> bool {
            self.opened.push((scope, backwards, seq));
            self.switcher_up = self.can_open;
            self.can_open
        }
        fn apply(&mut self, effect: Effect) {
            self.applied.push(effect);
        }
        fn close_switcher(&mut self) -> bool {
            self.closed += 1;
            std::mem::take(&mut self.switcher_up)
        }
        fn log(&mut self, line: &str) {
            self.lines.push(line.to_string());
        }
    }

    fn shared() -> Shared {
        Shared::new(Arc::new(Clock::manual()), false)
    }

    #[test]
    fn both_settings_are_off_by_default_and_round_trip() {
        let d = Settings::default();
        assert_eq!(wanted(&d), (false, false), "experimental: off until asked for");
        assert_eq!(wanted(&Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap()), (false, false));
        let on = Settings { shell_win_key: true, shell_own_switcher: true, ..Default::default() };
        let text = toml::to_string(&on).unwrap();
        let back = Settings::parse_lenient_pub(&text).unwrap();
        assert_eq!(wanted(&back), (true, true));
        assert_eq!(wanted(&Settings::parse_lenient_pub("shell_win_key = \"evet\"\nshell_own_switcher = 3\n").unwrap()), (false, false), "wrong values fall back to off");
        let one = Settings { shell_own_switcher: true, ..Default::default() };
        assert_eq!(wanted(&one), (false, true));
    }

    #[test]
    fn only_a_one_switches_log_only_mode_on() {
        assert!(log_only_from(Some("1")));
        assert!(log_only_from(Some(" 1 ")));
        assert!(!log_only_from(Some("0")));
        assert!(!log_only_from(Some("true")));
        assert!(!log_only_from(Some("")));
        assert!(!log_only_from(None));
    }

    #[test]
    fn a_tap_toggles_the_start_panel_and_says_so() {
        let mut f = Fake { start_works: true, ..Default::default() };
        handle(Posted::StartTap, false, &shared(), &mut f);
        assert_eq!((f.started, f.windows_started), (1, 0));
        assert_eq!(f.lines, vec!["start: toggled"]);
    }

    #[test]
    fn a_tap_whose_panel_could_not_open_asks_windows_for_its_start_and_does_not_say_toggled() {
        // the mask key had already kept Windows' Start shut for this tap
        let mut f = Fake { start_works: false, ..Default::default() };
        handle(Posted::StartTap, false, &shared(), &mut f);
        assert_eq!((f.started, f.windows_started), (1, 1));
        assert_eq!(f.lines, vec!["start: our panel could not be opened; Windows' Start was asked for"]);
    }

    #[test]
    fn in_log_only_mode_nothing_is_done_and_the_lines_say_would() {
        let mut f = Fake::default();
        let s = shared();
        handle(Posted::StartTap, true, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Open { scope: Scope::AllWindows, backwards: false }, seq: 1 }, true, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Step { backwards: false }, seq: 1 }, true, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Choose, seq: 1 }, true, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Cancel, seq: 1 }, true, &s, &mut f);
        handle(Posted::Lost, true, &s, &mut f);
        assert_eq!((f.started, f.windows_started, f.opened.len(), f.applied.len(), f.closed), (0, 0, 0, 0, 0), "log-only opens, toggles and applies nothing");
        assert!(f.lines.iter().all(|l| l.contains("would")), "{:?}", f.lines);
        assert_eq!(f.lines.len(), 4);
        assert_eq!(s.take_reset(), None, "and gives nothing up");
    }

    #[test]
    fn the_chord_opens_the_switcher_then_drives_it() {
        let mut f = Fake { can_open: true, ..Default::default() };
        let s = shared();
        handle(Posted::Chord { effect: Effect::Open { scope: Scope::ThisApp, backwards: true }, seq: 4 }, false, &s, &mut f);
        assert_eq!(f.opened, vec![(Scope::ThisApp, true, 4)]);
        handle(Posted::Chord { effect: Effect::Step { backwards: false }, seq: 4 }, false, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Move(Dir::Left), seq: 4 }, false, &s, &mut f);
        handle(Posted::Chord { effect: Effect::Choose, seq: 4 }, false, &s, &mut f);
        assert_eq!(f.applied, vec![Effect::Step { backwards: false }, Effect::Move(Dir::Left), Effect::Choose]);
        assert_eq!(f.lines, vec!["switcher: opened", "switcher: chose"]);
        assert_eq!(s.take_reset(), None);
    }

    #[test]
    fn a_switcher_that_could_not_be_opened_gives_the_chord_up_at_once() {
        let mut f = Fake { can_open: false, ..Default::default() };
        let s = shared();
        handle(Posted::Chord { effect: Effect::Open { scope: Scope::AllWindows, backwards: false }, seq: 9 }, false, &s, &mut f);
        assert_eq!(s.take_reset(), Some(rf_win::winkey::shared::Reset { seq: 9, give_up: true }));
        assert_eq!(f.lines, vec!["switcher: could not be opened; the chord was given up"]);
    }

    #[test]
    fn a_layer_that_lost_track_closes_the_switcher_only_if_one_is_open() {
        let mut f = Fake { switcher_up: true, ..Default::default() };
        let s = shared();
        handle(Posted::Lost, false, &s, &mut f);
        assert_eq!(f.lines, vec!["switcher: closed (the keyboard layer lost track of the chord)"]);
        handle(Posted::Lost, false, &s, &mut f);
        assert_eq!(f.closed, 2);
        assert_eq!(f.lines.len(), 1, "no switcher was open the second time: nothing to say");
    }

    #[test]
    fn no_log_line_names_a_key() {
        // every line this module can write, from the code that writes them
        let src = include_str!("keylayer.rs");
        let code: String = src.split("#[cfg(test)]").next().unwrap_or(src).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for stmt in code.split("a.log(").skip(1).chain(code.split("log::").skip(1)) {
            let line = stmt.split(';').next().unwrap_or("");
            for banned in ["vk", "scan", "flags", "Key::", "RawKey", "KeyInput", "{p", "{effect", "{:?}"] {
                assert!(!line.contains(banned), "a log line mentions `{banned}`: {line}");
            }
        }
    }

    /// The whole path but the window: the real service over a fake feed, its posts into `handle`, a fake in place of GPUI.
    struct SharedProbe(Arc<FakeProbe>);
    impl KeyProbe for SharedProbe {
        fn down(&self, vk: u32) -> bool {
            self.0.down(vk)
        }
    }

    #[test]
    fn alt_tab_through_the_service_the_dispatcher_and_the_handler_opens_chooses_and_masks() {
        let got: Arc<parking_lot::Mutex<Vec<Posted>>> = Default::default();
        let g = got.clone();
        let deliver: Deliver = Arc::new(move |p| g.lock().push(p));
        let clock = Arc::new(Clock::manual());
        clock.set(50);
        let mask = Arc::new(FakeMask::default());
        let mut svc = KeyService::new(FakeFeed::default(), mask.clone(), Box::new(SharedProbe(Arc::new(FakeProbe::default()))), deliver, clock, false);
        svc.set_wanted(false, true).unwrap();
        let shared = svc.shared().unwrap();
        let f = svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true));
        f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, false);
        f.push_raw(VK_LMENU, 0x38, 0, false);
        for _ in 0..400 {
            if got.lock().len() >= 2 {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let posts = got.lock().clone();
        let mut fake = Fake { can_open: true, ..Default::default() };
        for p in posts {
            handle(p, false, &shared, &mut fake);
        }
        assert_eq!(fake.opened.len(), 1);
        assert_eq!(fake.applied, vec![Effect::Choose]);
        assert_eq!(mask.sent(), 1, "the Alt release was masked");
    }

    #[test]
    fn when_the_switcher_cannot_open_the_next_tab_goes_to_windows_through_the_whole_path() {
        let got: Arc<parking_lot::Mutex<Vec<Posted>>> = Default::default();
        let g = got.clone();
        let deliver: Deliver = Arc::new(move |p| g.lock().push(p));
        let clock = Arc::new(Clock::manual());
        clock.set(50);
        let mut svc = KeyService::new(FakeFeed::default(), Arc::new(FakeMask::default()), Box::new(SharedProbe(Arc::new(FakeProbe::default()))), deliver, clock, false);
        svc.set_wanted(true, true).unwrap();
        let shared = svc.shared().unwrap();
        svc.feed_mut().push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true), "swallowed first");
        for _ in 0..400 {
            if !got.lock().is_empty() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let first = got.lock()[0];
        let mut fake = Fake { can_open: false, ..Default::default() };
        handle(first, false, &shared, &mut fake);
        svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, false);
        assert!(!svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true), "given up: the next Tab is Windows'");
    }

    #[test]
    fn the_keyboard_layer_calls_no_window_function_of_its_own() {
        // the file asks the switcher and the dock for everything, and never touches a window handle or a hook itself
        let src = include_str!("keylayer.rs");
        let code: String = src.split("#[cfg(test)]").next().unwrap_or(src).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for banned in ["SetWindowsHookEx", "SendInput", "RegisterHotKey", "SetWindowPos", "ShowWindow", "PostMessage", "SendMessage"] {
            assert!(!code.contains(banned), "{banned}");
        }
    }
}
