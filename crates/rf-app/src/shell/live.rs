//! The shell host while it runs: what happens when the shell window comes or goes (Explorer closed, crashed, restarted, replaced).
//!
//! `rf_win::dock::system` watches (one `GetShellWindow()` per 2 s tick, and the `TaskbarCreated` broadcast; see
//! `rf_win::shellhost::watch`) and sends `SystemEvent::ShellChanged`. [`on_shell_changed`] logs it and tells the parts that care:
//!
//! | part | what it does |
//! |---|---|
//! | the dock's band | needs no telling: `rf_win::dock::system` handles `TaskbarCreated` itself (registers the band again, `band.rs`) |
//! | the desktop cover | places its windows again at once (bottom of the z-order without a shell desktop, directly above it with one) and reads the wallpaper by the route that fits: [`super::cover::shell_changed`] |
//! | the media and volume keys | [`sync_keys`]: a hidden shell-hook window exists only while nobody owns the shell window |
//! | the work area | [`sync_work_area`]: only with no shell, never beside Explorer; every monitor with a dock gets the monitor less its dock's band, and the islands stand on the screen's bottom edge (`RUSTYFILES_SET_WORKAREA=0` leaves the work area alone) |
//! | minimized windows | [`sync_minimized`]: with no shell window at all, Windows is told to hide minimized windows (`ARW_HIDE`) as a shell does, instead of drawing them as small title bars at the bottom left; put back when the host ends; `RUSTYFILES_KEEP_MINIMIZED=1` leaves it alone |
//!
//! COST. With Explorer running and nothing changing this module does nothing at all: no thread, no window, no timer. A change costs one
//! worker thread per part for as long as the part's work takes. Switches: the keys follow the shell window by themselves (they act only
//! where, without them, the keys would be dead); the work area follows the shell window the same way (it was opt-in by the variable
//! until 0.9.8; WHY it is not any more: with Explorer closed, the strip its taskbar had reserved stays in the work area and nobody
//! answers the dock's own registration, so the dock floated a taskbar's height above the bottom edge with a gap under it. Seen on the
//! user's PC on 2026-11-23). It is the session's work area, never written to the profile, put back when the host ends, and Explorer
//! sets its own again when it comes back.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use gpui_kit::App;
use rf_win::dock::layout::Rect;
use rf_win::shellhost::ShellMode;
use rf_win::shellhost::appcmd::{Shell, shell_now};
use rf_win::shellhost::minimized::{self, MetricsApi, MinimizedKeeper, SystemMetrics};
use rf_win::shellhost::shellhook::ShellHook;
use rf_win::shellhost::workarea::{Done, ShellState, SystemSetter, SystemShell, WorkAreaKeeper};

use crate::settings::Settings;

/// The environment variable that keeps the host from setting the work area itself when no shell is running (`0`).
pub const ENV_WORK_AREA: &str = "RUSTYFILES_SET_WORKAREA";

/// Whether the host may set the work area with no shell: yes, unless the variable's value is exactly `0`.
pub fn work_area_wanted(value: Option<&str>) -> bool {
    value.is_none_or(|v| v.trim() != "0")
}

const UNKNOWN: u8 = 255;
static CURRENT: AtomicU8 = AtomicU8::new(UNKNOWN);

fn code(m: ShellMode) -> u8 {
    match m {
        ShellMode::Beside => 0,
        ShellMode::Shell => 1,
    }
}

/// The mode as last seen: the start-up look, kept up to date by the watch.
pub fn current() -> Option<ShellMode> {
    match CURRENT.load(Ordering::Relaxed) {
        0 => Some(ShellMode::Beside),
        1 => Some(ShellMode::Shell),
        _ => None,
    }
}

/// The host started with this mode (`shell::note_host_start`).
pub fn init(mode: ShellMode) {
    CURRENT.store(code(mode), Ordering::Relaxed);
}

/// What to say about a change of the mode, or `None` when it is the same.
pub fn change_line(before: Option<ShellMode>, now: ShellMode) -> Option<String> {
    match before {
        Some(b) if b == now => None,
        Some(b) => Some(format!("shell host: mode {} -> {}", b.name(), now.name())),
        None => Some(format!("shell host: mode {}", now.name())),
    }
}

static HOOK: Mutex<Option<ShellHook>> = Mutex::new(None);
/// The keepers of the work areas the host set itself, one per monitor (by its device name).
static WORK: Mutex<Vec<(String, WorkAreaKeeper)>> = Mutex::new(Vec::new());
/// A keeper holds a work area now (or is about to): the docks then stand on their monitors' bottom edges, inside the bands the host
/// told (`dock::host::area_of`).
static HOLDS: AtomicBool = AtomicBool::new(false);

/// Whether the host itself holds work areas now, with no shell (see [`sync_work_area`]).
pub fn holds_edges() -> bool {
    HOLDS.load(Ordering::Relaxed)
}
static MINIMIZED: Mutex<Option<MinimizedKeeper>> = Mutex::new(None);

/// The host's start, after its windows exist: brings the parts in line with the mode it started in.
pub fn start(cx: &mut App) {
    sync_keys();
    sync_minimized();
    sync_work_area(cx);
}

/// `SystemEvent::ShellChanged`: the shell window is another one now, the mode is `mode`.
pub fn on_shell_changed(mode: ShellMode, cx: &mut App) {
    if let Some(l) = change_line(current(), mode) {
        log::info!("{l}");
    }
    CURRENT.store(code(mode), Ordering::Relaxed);
    super::cover::shell_changed(cx);
    // Win+E and Win+D follow who owns the shell window
    super::keylayer::shell_changed(cx);
    sync_keys();
    sync_minimized();
    sync_work_area(cx);
}

/// The dock's layout changed, or a dock's band did (the work areas the host keeps follow). Beside Explorer it costs a comparison.
pub fn layout_changed(cx: &mut App) {
    sync_work_area(cx);
}

/// How long [`stop`] waits for the hook to end and the work areas to be given back. Each monitor's work area is one broadcast that
/// waits for every program's answer: with two monitors 2 s was passed in the live run of 2026-11-23 (the second monitor's was given
/// back after the warning; a host that had ended sooner would have left it).
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// Runs `job` on a thread of its own and waits for it at most `wait`. True: it finished in time. False: it is still running (or the
/// thread did not start) and the caller goes on; the job is not cut short.
fn run_waiting(name: &str, wait: std::time::Duration, job: impl FnOnce() + Send + 'static) -> bool {
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let spawned = std::thread::Builder::new().name(name.into()).spawn(move || {
        job();
        let _ = tx.send(());
    });
    spawned.is_ok() && rx.recv_timeout(wait).is_ok()
}

/// The host stops: the hook goes, and a work area this host set is put back. The app thread does not wait on either for long: dropping
/// the hook joins its thread, and giving the work area back broadcasts the change and waits for every program to answer.
pub fn stop(cx: &mut App) {
    let _ = cx;
    let done = run_waiting("rf-live-stop", STOP_WAIT, || {
        *HOOK.lock().unwrap_or_else(|p| p.into_inner()) = None;
        // minimized windows: the metrics from before are put back (only while there is still no shell, and only if they are still ours)
        if let Some(line) = minimized_stop(&mut MINIMIZED.lock().unwrap_or_else(|p| p.into_inner()), &SystemShell, &mut SystemMetrics) {
            log::info!("{line}");
        }
        let mut g = WORK.lock().unwrap_or_else(|p| p.into_inner());
        if g.iter().any(|(_, k)| k.active()) {
            let displays = rf_win::dock::monitors::displays();
            for (name, k) in g.iter_mut().filter(|(_, k)| k.active()) {
                if let Some(d) = displays.iter().find(|d| d.name == *name) {
                    let done = k.apply(&SystemShell, &mut SystemSetter, None, d.info.rect, d.info.work);
                    log::info!("work area of {name}: on stop {done:?}");
                }
            }
        }
        g.clear();
        HOLDS.store(false, Ordering::Relaxed);
    });
    if !done {
        log::warn!("shell host: the hook and the work area were still being given back after {} s; the host goes on", STOP_WAIT.as_secs());
    }
    rf_win::dock::system::set_own_band(None);
}

/// Starts or ends the shell-hook window by whether nobody owns the shell window NOW. On a worker thread: the window is made on a thread
/// of its own and waited for, and dropping it joins that thread.
fn sync_keys() {
    let spawned = std::thread::Builder::new().name("rf-keys-sync".into()).spawn(|| {
        let want = shell_now() == Shell::Nobody;
        let mut g = HOOK.lock().unwrap_or_else(|p| p.into_inner());
        match (want, g.is_some()) {
            (true, false) => match ShellHook::start() {
                Ok(h) => {
                    *g = Some(h);
                    log::info!("shell host: no shell window - the media and volume keys are taken");
                }
                Err(e) => log::warn!("shell host: the media keys could not be taken: {e:#}"),
            },
            (false, true) => {
                *g = None;
                log::info!("shell host: a shell window is there - the media and volume keys are left to it");
            }
            _ => {}
        }
    });
    if let Err(e) = spawned {
        log::warn!("shell host: the key watcher thread did not start: {e}");
    }
}

/// With NO window owning the shell window, tells Windows to hide minimized windows (a shell does that; without one they show as small
/// title bars at the bottom left). With a shell there it does nothing, and lets go of what it did (see `rf_win::shellhost::minimized`
/// for why Explorer's own value is never overwritten). On a worker thread: it is a system call, not for the interface thread.
fn sync_minimized() {
    if minimized::left_alone(std::env::var(minimized::ENV_KEEP).ok().as_deref()) {
        return;
    }
    let spawned = std::thread::Builder::new().name("rf-minimized-sync".into()).spawn(|| {
        let mut g = MINIMIZED.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(line) = minimized_sync(&mut g, &SystemShell, &mut SystemMetrics) {
            log::info!("{line}");
        }
        // (the windows that were minimized before the setting stood are still small title bars: they are hidden too)
        let moved = minimized::park_shown_bars();
        if moved > 0 {
            log::info!("shell host: {moved} minimized window(s) still shown as title bars were hidden");
        }
    });
    if let Err(e) = spawned {
        log::warn!("shell host: the minimized-windows thread did not start: {e}");
    }
}

/// The step of [`sync_minimized`] over the keeper in `slot` (made on first use), with the shell look and the metrics given in, so
/// that it is tested on fakes. The line to log, when something changed.
fn minimized_sync(slot: &mut Option<MinimizedKeeper>, state: &dyn ShellState, api: &mut dyn MetricsApi) -> Option<&'static str> {
    let k = slot.get_or_insert_with(MinimizedKeeper::new);
    let was = k.active();
    match k.apply(state, api, true) {
        minimized::Done::Hidden => Some("shell host: no shell window - minimized windows are hidden, as a shell does"),
        minimized::Done::Restored => Some("shell host: another shell is there - the minimized windows setting is back as it was"),
        minimized::Done::Nothing if was && !k.active() => Some("shell host: a shell window is there - minimized windows are left to it"),
        minimized::Done::Nothing => None,
    }
}

/// The host ends: what the keeper changed is put back (only while there is still no shell, and only if it is still ours).
fn minimized_stop(slot: &mut Option<MinimizedKeeper>, state: &dyn ShellState, api: &mut dyn MetricsApi) -> Option<String> {
    let k = slot.as_mut().filter(|k| k.active())?;
    Some(format!("minimized windows: on stop {:?}", k.apply(state, api, false)))
}

/// The thickness of the home dock's band from the settings alone, device px (`None`: no band wanted). Only for the moment before the
/// dock has said its band itself (`taskbar_hide::current_band`, which knows the thin band and a size fitted to a narrow monitor).
fn band_thickness(cx: &App) -> Option<i32> {
    let s = cx.try_global::<Settings>()?;
    if !s.dock_reserve {
        return None;
    }
    let m = rf_win::dock::primary_monitor()?;
    let unit = rf_win::dock::size::unit(m.scale(), s.dock_size.factor());
    Some(rf_win::dock::layout::reserved_thickness(unit))
}

/// One monitor of [`work_plan`]: its device name, its rectangle, its work area now, and the band its dock keeps (device px).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkWant {
    pub name: String,
    pub primary: bool,
    pub monitor: Rect,
    pub work: Rect,
    pub band: i32,
}

/// The monitors whose work area the host keeps while there is no shell, pure. The primary monitor always (the home dock stands on
/// it): its band is `home`, or nothing when the dock reserves none (`dock_reserve` off; the keeper then leaves the whole monitor
/// less one pixel, and the islands still stand on the bottom edge). Another monitor only when a dock on it reserves a band (`others`,
/// by device name). A monitor that is in no list keeps the work area it has.
pub fn work_plan(displays: &[rf_win::dock::monitors::Display], home: Option<i32>, others: &[(String, i32)]) -> Vec<WorkWant> {
    displays
        .iter()
        .filter_map(|d| {
            let band = if d.primary { Some(home.unwrap_or(0)) } else { others.iter().find(|(n, _)| *n == d.name).map(|(_, b)| *b) }?;
            Some(WorkWant { name: d.name.clone(), primary: d.primary, monitor: d.info.rect, work: d.info.work, band })
        })
        .collect()
}

/// Sets (or gives back) the work areas itself, when nobody owns the shell window. A worker thread: the call broadcasts the change and
/// may wait for programs that are slow to answer. Beside Explorer with nothing of ours in force it returns at once, with no thread.
fn sync_work_area(cx: &App) {
    if !work_area_wanted(std::env::var(ENV_WORK_AREA).ok().as_deref()) {
        return;
    }
    if current() != Some(ShellMode::Shell) && !holds_edges() {
        return;
    }
    let home = super::taskbar_hide::current_band().unwrap_or_else(|| band_thickness(cx));
    let others = super::taskbar_hide::other_bands();
    let spawned = std::thread::Builder::new().name("rf-workarea-sync".into()).spawn(move || {
        let mut g = WORK.lock().unwrap_or_else(|p| p.into_inner());
        // (the monitors are looked at with the lock held: a thread that looked before waiting for the one ahead of it had the work
        // area from before that one's change and set the same rectangle again; seen in the first live run, two and three times)
        let displays = rf_win::dock::monitors::displays();
        // (a list with no primary monitor is the system not being able to list them for a moment: nothing is decided from it)
        if !rf_win::dock::monitors::usable(&displays) {
            return;
        }
        // A shell window has an owner (Explorer came back): the work areas are its own again. Every keeper forgets what it did (that
        // is what `apply` does for a shell that is there) and the islands are placed from the work area as beside Explorer.
        if shell_now() != Shell::Nobody {
            for (name, k) in g.iter_mut() {
                if let Some(d) = displays.iter().find(|d| d.name == *name) {
                    let _ = k.apply(&SystemShell, &mut SystemSetter, None, d.info.rect, d.info.work);
                }
            }
            g.clear();
            HOLDS.store(false, Ordering::Relaxed);
            rf_win::dock::system::set_own_band(None);
            return;
        }
        let plan = work_plan(&displays, home, &others);
        // a monitor that left the plan (its dock went, or reserves nothing now) gets back what it had; one that is gone is forgotten
        for (name, k) in g.iter_mut() {
            if k.active()
                && !plan.iter().any(|w| w.name == *name)
                && let Some(d) = displays.iter().find(|d| d.name == *name)
            {
                let done = k.apply(&SystemShell, &mut SystemSetter, None, d.info.rect, d.info.work);
                log::info!("work area of {name}: {done:?}");
            }
        }
        g.retain(|(name, _)| plan.iter().any(|w| w.name == *name));
        // The bands are told to the docks' placement BEFORE the work areas shrink, so that the islands are not worked out from the
        // shrunken area (they float inside the band): the home dock's as its own band, the others' by the flag the host reads.
        if let Some(w) = plan.iter().find(|w| w.primary) {
            rf_win::dock::system::set_own_band(Some(Rect { top: w.monitor.bottom - w.band.max(1), ..w.monitor }));
        }
        HOLDS.store(!plan.is_empty(), Ordering::Relaxed);
        for w in &plan {
            if !g.iter().any(|(n, _)| *n == w.name) {
                g.push((w.name.clone(), WorkAreaKeeper::new()));
            }
            let Some((_, k)) = g.iter_mut().find(|(n, _)| *n == w.name) else { continue };
            match k.apply(&SystemShell, &mut SystemSetter, Some(w.band), w.monitor, w.work) {
                Done::Nothing => {}
                done => log::info!("work area of {}: {done:?}", w.name),
            }
        }
        // (what is really held: a call Windows refused, or a shell that came in between, leaves nothing)
        HOLDS.store(g.iter().any(|(_, k)| k.active()), Ordering::Relaxed);
        let home_held = plan.iter().find(|w| w.primary).is_some_and(|w| g.iter().any(|(n, k)| *n == w.name && k.active()));
        if !home_held {
            rf_win::dock::system::set_own_band(None);
        }
    });
    if let Err(e) = spawned {
        log::warn!("work area: the thread did not start: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_area_is_kept_unless_a_zero_forbids_it() {
        assert!(work_area_wanted(None));
        assert!(work_area_wanted(Some("")));
        assert!(work_area_wanted(Some("1")));
        assert!(work_area_wanted(Some("true")));
        assert!(!work_area_wanted(Some("0")));
        assert!(!work_area_wanted(Some(" 0 ")));
    }

    fn display(name: &str, primary: bool, rect: Rect, work: Rect) -> rf_win::dock::monitors::Display {
        rf_win::dock::monitors::Display { name: name.into(), info: rf_win::dock::MonitorInfo { handle: 1, rect, work, dpi: 96 }, primary }
    }

    #[test]
    fn every_monitor_with_a_dock_band_is_kept_and_the_primary_always() {
        // the user's desk on 2026-11-23: Explorer closed, its taskbar's 48 px still out of both work areas
        let (m1, m2) = (Rect::new(0, 0, 2560, 1440), Rect::new(2560, -618, 4000, 1942));
        let d = [display("A", true, m1, Rect { bottom: 1392, ..m1 }), display("B", false, m2, Rect { bottom: 1894, ..m2 })];
        let plan = work_plan(&d, Some(78), &[("B".into(), 78)]);
        assert_eq!(plan.len(), 2);
        assert_eq!((plan[0].name.as_str(), plan[0].primary, plan[0].band, plan[0].work.bottom), ("A", true, 78, 1392));
        assert_eq!((plan[1].name.as_str(), plan[1].primary, plan[1].band), ("B", false, 78));
        // a second monitor whose dock reserves nothing (or that has no dock) is left as it is
        assert_eq!(work_plan(&d, Some(78), &[]).iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["A"]);
        // the home dock reserves nothing: the primary is still kept (band 0), so the islands stand on the bottom edge
        assert_eq!(work_plan(&d, None, &[]).iter().map(|w| (w.name.as_str(), w.band)).collect::<Vec<_>>(), [("A", 0)]);
    }

    #[test]
    fn a_change_of_the_mode_is_said_once_and_the_same_mode_is_silent() {
        assert_eq!(change_line(None, ShellMode::Beside).as_deref(), Some("shell host: mode beside"));
        assert_eq!(change_line(Some(ShellMode::Beside), ShellMode::Shell).as_deref(), Some("shell host: mode beside -> shell"));
        assert_eq!(change_line(Some(ShellMode::Shell), ShellMode::Beside).as_deref(), Some("shell host: mode shell -> beside"));
        assert_eq!(change_line(Some(ShellMode::Shell), ShellMode::Shell), None);
    }

    #[test]
    fn a_stop_job_that_hangs_does_not_hold_the_caller_for_longer_than_the_wait() {
        let t0 = std::time::Instant::now();
        let finished = run_waiting("t-hang", std::time::Duration::from_millis(100), || std::thread::sleep(std::time::Duration::from_secs(3)));
        assert!(!finished, "a job that outlasts the wait is reported as unfinished");
        assert!(t0.elapsed() < std::time::Duration::from_secs(2), "the caller was held for {:?}", t0.elapsed());
        assert!(run_waiting("t-quick", std::time::Duration::from_secs(5), || {}), "a job that is done in time is reported as done");
    }

    /// A shell look the test sets, and a system that holds one value and counts the writes.
    struct FakeState(std::cell::Cell<rf_win::shellhost::appcmd::Shell>);
    impl ShellState for FakeState {
        fn shell(&self) -> rf_win::shellhost::appcmd::Shell {
            self.0.get()
        }
    }
    struct FakeMetrics {
        now: minimized::Metrics,
        writes: u32,
    }
    impl MetricsApi for FakeMetrics {
        fn get(&mut self) -> Option<minimized::Metrics> {
            Some(self.now)
        }
        fn set(&mut self, m: minimized::Metrics, _why: minimized::Write) -> bool {
            self.now = m;
            self.writes += 1;
            true
        }
    }

    #[test]
    fn the_hosts_minimized_windows_wiring_hides_with_no_shell_leaves_a_shell_alone_and_puts_the_old_value_back_on_stop() {
        use rf_win::shellhost::appcmd::Shell;
        let before = minimized::Metrics { width: 160, horz_gap: 4, vert_gap: 4, arrange: 0 };
        let state = FakeState(std::cell::Cell::new(Shell::Nobody));
        let mut api = FakeMetrics { now: before, writes: 0 };
        let mut slot: Option<MinimizedKeeper> = None;
        // a start with a shell: nothing is written, nothing logged
        state.0.set(Shell::Explorer);
        assert_eq!(minimized_sync(&mut slot, &state, &mut api), None);
        assert_eq!(api.writes, 0);
        // the shell goes away: the bars are hidden, once
        state.0.set(Shell::Nobody);
        assert!(minimized_sync(&mut slot, &state, &mut api).unwrap().contains("hidden"));
        assert!(api.now.hidden() && api.writes == 1);
        assert_eq!(minimized_sync(&mut slot, &state, &mut api), None, "asked again, nothing more is written");
        assert_eq!(api.writes, 1);
        // Explorer comes back: its own value is not overwritten
        state.0.set(Shell::Explorer);
        assert!(minimized_sync(&mut slot, &state, &mut api).unwrap().contains("left to it"));
        assert_eq!(api.writes, 1);
        assert_eq!(minimized_stop(&mut slot, &state, &mut api), None, "nothing is held any more, so nothing is put back");
        // the shell goes away again and the host ends while it is still the only shell: the metrics from before are put back exactly
        api.now = before;
        state.0.set(Shell::Nobody);
        assert!(minimized_sync(&mut slot, &state, &mut api).is_some());
        assert!(api.now.hidden());
        let line = minimized_stop(&mut slot, &state, &mut api).expect("the stop puts them back");
        assert!(line.contains("Restored"), "{line}");
        assert_eq!(api.now, before);
    }

    /// A guard, not coverage (the behaviour is in the test above): the real metrics writer is made here and nowhere else in the app,
    /// and the work is not done on the interface thread.
    #[test]
    fn the_real_minimized_metrics_writer_is_reached_only_from_here_and_off_the_interface_thread() {
        let here = include_str!("live.rs");
        let code = &here[..here.find("#[cfg(test)]").unwrap()];
        assert_eq!(code.matches("SystemMetrics").count(), 3, "the import and the two uses, in stop and in sync_minimized");
        let sync = &code[code.find("fn sync_minimized").unwrap()..];
        assert!(sync[..1200].contains("thread::Builder"), "a worker thread");
        for other in [include_str!("mod.rs"), include_str!("cover.rs"), include_str!("../dock/host.rs"), include_str!("../lib.rs")] {
            assert!(!other.contains("SystemMetrics"), "another file reaches the real writer");
        }
    }

    #[test]
    fn the_current_mode_follows_what_it_is_told() {
        init(ShellMode::Beside);
        assert_eq!(current(), Some(ShellMode::Beside));
        init(ShellMode::Shell);
        assert_eq!(current(), Some(ShellMode::Shell));
        init(ShellMode::Beside);
    }

    /// The only place a work area is set by this host is `sync_work_area` (and given back in `stop`): the variable can forbid it, a
    /// shell that is there stops it before any thread starts, and the real setter is made nowhere else in the app.
    #[test]
    fn the_real_work_area_setter_is_reached_only_from_here_and_only_with_no_shell() {
        let here = include_str!("live.rs");
        let code = &here[..here.find("#[cfg(test)]").unwrap()];
        assert_eq!(code.matches("SystemSetter").count(), 5, "the import, the use in stop and the three in sync_work_area");
        let sync = &code[code.find("fn sync_work_area").unwrap()..];
        assert!(sync[..200].contains("work_area_wanted"), "the variable is checked first");
        let (mode, thread) = (sync.find("current() != Some(ShellMode::Shell)").unwrap(), sync.find("thread::Builder").unwrap());
        assert!(mode < thread, "beside Explorer nothing starts");
        for other in [include_str!("mod.rs"), include_str!("cover.rs"), include_str!("../dock/host.rs"), include_str!("../lib.rs")] {
            assert!(!other.contains("SystemSetter"), "another file reaches the real setter");
        }
    }
}
