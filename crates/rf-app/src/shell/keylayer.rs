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
//! WIN+E, WIN+D AND WIN+SHIFT+S (same switch as the Windows key, `shell_win_key`): Explorer answers these chords, and without it they are
//! dead. While the shell host runs with NO Explorer owning the shell window (`live::current()` is `Shell`), Win+E opens a rustyfiles
//! window (This PC / Home), Win+D shows the desktop (our Show Desktop, `rf_win::desktop::showdesktop`) and Win+Shift+S starts our own
//! screen snip (`crate::snip`). Beside Explorer they are left to it (`combos_wanted`). The other Explorer chords stay untouched; the
//! list is in docs/design/shell-keys.md.
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
use rf_win::shellhost::ShellMode;
use rf_win::switcher::chord::{Effect, Scope};
use rf_win::switcher::hook::LowLevelHook;
use rf_win::winkey::{Beat, Combo, Deliver, HEARTBEAT_EVERY_MS, KeyService, Posted, Shared};

use super::switcher::{self, Closing, OpenRequest};
use crate::dock::parts::Part;
use crate::settings::Settings;

/// `RUSTYFILES_KEYS_LOG_ONLY=1`: decide and log, swallow nothing, send nothing, open nothing.
pub const ENV_LOG_ONLY: &str = "RUSTYFILES_KEYS_LOG_ONLY";

/// Whether the value of [`ENV_LOG_ONLY`] switches log-only mode on: exactly "1".
pub fn log_only_from(value: Option<&str>) -> bool {
    value.map(str::trim) == Some("1")
}

/// Which of the two layers the settings ask for: (Windows key, Alt+Tab).
pub fn wanted(s: &Settings) -> (bool, bool) {
    // (the dock switch and each part's own value: `dock::parts`)
    (crate::dock::parts::runs(s, Part::WinKey), crate::dock::parts::runs(s, Part::Switcher))
}

/// Whether Win+E, Win+D and Win+Shift+S are ours: the Windows-key switch is on and Explorer is NOT the shell (`mode` is what the host last saw; `None`
/// before it has looked: left to Windows). Beside Explorer it answers them itself, and taking them would only change what they do.
pub fn combos_wanted(win_key: bool, mode: Option<ShellMode>) -> bool {
    win_key && mode == Some(ShellMode::Shell)
}

/// The one line the host writes when the layer is ready (or its state changed): what each key now does. Never names a key code.
pub fn ready_line(win_key: bool, own_switcher: bool, combos: bool, log_only: bool) -> String {
    let mut parts: Vec<&str> = Vec::new();
    if win_key {
        parts.push("a tap of the Windows key opens our Start panel");
        parts.push(if combos {
            "Win+E opens a file window, Win+D shows the desktop and Win+Shift+S starts the screen snip"
        } else {
            "Win+E, Win+D and Win+Shift+S are left to Explorer"
        });
    }
    if own_switcher {
        parts.push("Alt+Tab opens our switcher");
    }
    let tail = if log_only { " (log only: nothing is swallowed, sent or opened)" } else { "" };
    format!("keys: ready - {}{}", parts.join("; "), tail)
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
    /// Win+E: a rustyfiles file window.
    fn open_files_window(&mut self);
    /// Win+D: our Show Desktop (minimizes the windows, or brings them back).
    fn show_desktop(&mut self);
    /// Win+Shift+S: our screen snip.
    fn start_snip(&mut self);
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
        Posted::WinCombo(Combo::Explorer) => {
            if log_only {
                a.log("win+e: would open a file window (log only)");
            } else {
                a.open_files_window();
                a.log("win+e: opened a file window");
            }
        }
        Posted::WinCombo(Combo::Desktop) => {
            if log_only {
                a.log("win+d: would show the desktop (log only)");
            } else {
                a.show_desktop();
                a.log("win+d: show desktop toggled");
            }
        }
        Posted::WinCombo(Combo::Snip) => {
            if log_only {
                a.log("win+shift+s: would start the screen snip (log only)");
            } else {
                a.start_snip();
                a.log("win+shift+s: screen snip started");
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

/// How long after a tap of the Windows key the window in front is looked at (Windows' Start needs a moment to come up).
const AFTER_TAP_MS: u64 = 700;

/// Whether a window's description (`rf_win::window::describe`: handle, program, class, title) is Windows' own Start menu or its
/// search, which is what a tap of the Windows key opens when nothing holds it shut. Pure.
pub fn is_windows_start(described: &str) -> bool {
    let d = described.to_ascii_lowercase();
    d.contains("startmenuexperiencehost.exe") || d.contains("searchhost.exe")
}

struct GpuiActions<'a> {
    cx: &'a mut App,
    shared: Arc<Shared>,
}

impl Actions for GpuiActions<'_> {
    fn toggle_start(&mut self) -> bool {
        let done = crate::dock::toggle_start(self.cx);
        // Whether the mask key kept Windows' own Start shut cannot be seen from the hook: only from what is in front a moment later
        // (`rf_win::winkey::mask`: "whether that holds on this Windows build is the first thing the live test looks at"). One line
        // for each tap that was taken; another window's title is written only when it is Windows' Start.
        self.cx
            .spawn(async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(AFTER_TAP_MS)).await;
                let front = rf_win::window::describe(rf_win::window::foreground());
                if is_windows_start(&front) {
                    log::warn!("start: Windows' own Start is in front {AFTER_TAP_MS} ms after the tap ({front}): the mask key did not keep it shut");
                } else {
                    log::info!("start: Windows' own Start is not in front {AFTER_TAP_MS} ms after the tap");
                }
            })
            .detach();
        done
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

    fn open_files_window(&mut self) {
        // (the program itself, as the dock's places do: the running rustyfiles takes it as a new tab, none running starts one; "This PC"
        // is a place we show ourselves, so this never goes to Explorer)
        crate::dock::actions::run_rustyfiles(vec!["shell:MyComputerFolder".to_string()]);
    }

    fn show_desktop(&mut self) {
        toggle_show_desktop();
    }

    fn start_snip(&mut self) {
        crate::snip::start(self.cx);
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

/// Our Show Desktop. The driver remembers which windows it minimized, so it lives as long as the process; the work is done on a thread
/// of its own (it enumerates every window and asks each to minimize or restore, without waiting).
fn toggle_show_desktop() {
    use rf_win::desktop::showdesktop::{Driver, SystemOps};
    static DRIVER: parking_lot::Mutex<Option<Driver<SystemOps>>> = parking_lot::Mutex::new(None);
    let spawned = std::thread::Builder::new().name("rf-show-desktop".into()).spawn(|| {
        DRIVER.lock().get_or_insert_with(|| Driver::new(SystemOps)).toggle_show_desktop();
    });
    if let Err(e) = spawned {
        log::warn!("win+d: the thread did not start: {e}");
    }
}

struct KeysHost {
    service: KeyService<LowLevelHook>,
    log_only: bool,
    /// The last "ready" line written, so that the same state is not written again at every reload of the settings.
    last_ready: String,
    // (kept alive: dropping a task stops it)
    _beat: Task<()>,
    _posts: Task<()>,
}

impl Global for KeysHost {}

/// The settings file was (re)read, or the host started: brings the layer in line with the two settings. Cheap when nothing changed.
pub fn on_settings(cx: &mut App) {
    let (win, alt) = cx.try_global::<Settings>().map_or((false, false), wanted);
    if !win && !alt {
        stop_because(cx, "both settings are off");
        note_hook(win, alt, true);
        return;
    }
    if !cx.has_global::<KeysHost>() {
        start(cx);
    }
    let combos = combos_wanted(win, super::live::current());
    let host = cx.global_mut::<KeysHost>();
    host.service.set_combos(combos);
    match host.service.set_wanted(win, alt) {
        Ok(()) => {
            // the one line that says the layer is ready and what it does now (again when that changes, e.g. Explorer came back)
            let line = ready_line(win, alt, combos, host.log_only);
            if line != host.last_ready {
                log::info!("{line}");
                host.last_ready = line;
            }
            note_hook(win, alt, true);
        }
        Err(e) => {
            log::warn!("keys: the keyboard layer could not start: {e:#}");
            note_hook(win, alt, false);
        }
    }
}

/// Tells the Dock page whether the hook of the parts that want it could be had (one hook serves both).
fn note_hook(win: bool, alt: bool, ok: bool) {
    use crate::dock::parts::Why;
    for (part, wanted) in [(Part::WinKey, win), (Part::Switcher, alt)] {
        crate::dock::parts_status::note(part, (wanted && !ok).then_some(Why::HookRefused));
    }
}

/// The shell window came or went (`live::on_shell_changed`): Win+E and Win+D follow who owns it.
pub fn shell_changed(cx: &mut App) {
    if cx.has_global::<KeysHost>() {
        on_settings(cx);
    }
}

/// Removes the hook and everything that served it, because the host ends. Safe to call at any time.
pub fn stop(cx: &mut App) {
    stop_because(cx, "the host is ending");
}

/// [`stop`] with the reason that goes into the log line (`keys: layer off, hook removed (<why>)`).
fn stop_because(cx: &mut App, why: &str) {
    if !cx.has_global::<KeysHost>() {
        return;
    }
    let mut host = cx.remove_global::<KeysHost>();
    host.service.stop_because(why);
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
    cx.set_global(KeysHost { service, log_only, last_ready: String::new(), _beat: beat, _posts: posts });
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
    use super::{Actions, Settings, combos_wanted, handle, is_windows_start, log_only_from, ready_line, wanted};
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
        files: u32,
        desktops: u32,
        snips: u32,
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
        fn open_files_window(&mut self) {
            self.files += 1;
        }
        fn show_desktop(&mut self) {
            self.desktops += 1;
        }
        fn start_snip(&mut self) {
            self.snips += 1;
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
    fn both_layers_come_with_the_dock_switch_unless_their_own_values_hold_them_off() {
        let d = Settings::default();
        assert_eq!(wanted(&d), (false, false), "the dock is off: no hook");
        assert_eq!(wanted(&Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap()), (false, false));
        let on = Settings { dock_enabled: true, ..Default::default() };
        assert_eq!(wanted(&on), (true, true));
        let text = toml::to_string(&on).unwrap();
        let back = Settings::parse_lenient_pub(&text).unwrap();
        assert_eq!(wanted(&back), (true, true));
        // wrong values hold the layer off (a typo must not install a keyboard hook); a false does too
        assert_eq!(wanted(&Settings::parse_lenient_pub("dock_enabled = true\nshell_win_key = \"evet\"\nshell_own_switcher = 3\n").unwrap()), (false, false));
        let one = Settings { dock_enabled: true, shell_win_key: false, ..Default::default() };
        assert_eq!(wanted(&one), (false, true));
        // the file an older build saved: every part false, the dock on: no hook is installed by the update
        let old = Settings::parse_lenient_pub("dock_enabled = true\nshell_win_key = false\nshell_own_switcher = false\n").unwrap();
        assert_eq!(wanted(&old), (false, false));
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
    fn windows_own_start_is_told_from_any_other_window_in_front() {
        assert!(is_windows_start("0x1a2b StartMenuExperienceHost.exe[4120] Windows.UI.Core.CoreWindow 'Başlat'"));
        assert!(is_windows_start("0x77 SearchHost.exe[991] Windows.UI.Core.CoreWindow 'Ara'"));
        assert!(!is_windows_start("0x5 rustyfiles.exe[10] Zed::Window ''"));
        assert!(!is_windows_start("0x9 chrome.exe[7] Chrome_WidgetWin_1 'start menu experience host - Google'"), "a title does not make it Start");
        assert!(!is_windows_start("none"));
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

    #[test]
    fn win_e_opens_a_file_window_and_win_d_shows_the_desktop_and_each_says_so() {
        let mut f = Fake::default();
        handle(Posted::WinCombo(rf_win::winkey::Combo::Explorer), false, &shared(), &mut f);
        handle(Posted::WinCombo(rf_win::winkey::Combo::Desktop), false, &shared(), &mut f);
        assert_eq!((f.files, f.desktops), (1, 1));
        assert_eq!(f.lines, vec!["win+e: opened a file window", "win+d: show desktop toggled"]);
    }

    #[test]
    fn win_shift_s_starts_the_screen_snip_and_says_so() {
        let mut f = Fake::default();
        handle(Posted::WinCombo(rf_win::winkey::Combo::Snip), false, &shared(), &mut f);
        assert_eq!((f.snips, f.files, f.desktops), (1, 0, 0));
        assert_eq!(f.lines, vec!["win+shift+s: screen snip started"]);
    }

    #[test]
    fn in_log_only_mode_the_chords_open_and_toggle_nothing() {
        let mut f = Fake::default();
        handle(Posted::WinCombo(rf_win::winkey::Combo::Explorer), true, &shared(), &mut f);
        handle(Posted::WinCombo(rf_win::winkey::Combo::Desktop), true, &shared(), &mut f);
        handle(Posted::WinCombo(rf_win::winkey::Combo::Snip), true, &shared(), &mut f);
        assert_eq!((f.files, f.desktops, f.snips), (0, 0, 0));
        assert!(f.lines.iter().all(|l| l.contains("would")), "{:?}", f.lines);
    }

    #[test]
    fn the_chords_are_ours_only_with_the_switch_on_and_no_explorer_as_the_shell() {
        use rf_win::shellhost::ShellMode::{Beside, Shell};
        assert!(combos_wanted(true, Some(Shell)));
        assert!(!combos_wanted(true, Some(Beside)), "Explorer answers them itself");
        assert!(!combos_wanted(true, None), "the host has not looked yet: left to Windows");
        assert!(!combos_wanted(false, Some(Shell)), "the switch is off");
    }

    #[test]
    fn the_ready_line_says_what_each_key_does_and_names_no_key_code() {
        assert_eq!(ready_line(true, false, true, false), "keys: ready - a tap of the Windows key opens our Start panel; Win+E opens a file window, Win+D shows the desktop and Win+Shift+S starts the screen snip");
        assert_eq!(ready_line(true, true, false, false), "keys: ready - a tap of the Windows key opens our Start panel; Win+E, Win+D and Win+Shift+S are left to Explorer; Alt+Tab opens our switcher");
        assert_eq!(ready_line(false, true, false, true), "keys: ready - Alt+Tab opens our switcher (log only: nothing is swallowed, sent or opened)");
    }
}
