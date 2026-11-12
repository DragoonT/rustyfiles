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
//! | the work area | [`sync_work_area`]: only with `RUSTYFILES_SET_WORKAREA=1`, only with no shell, never beside Explorer |
//!
//! COST. With Explorer running and nothing changing this module does nothing at all: no thread, no window, no timer. A change costs one
//! worker thread per part for as long as the part's work takes. Switches: the keys follow the shell window by themselves (they act only
//! where, without them, the keys would be dead); the work area is opt-in by the environment variable above, because it changes a
//! system-wide setting and there is no Settings row for it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, Ordering};

use gpui_kit::App;
use rf_win::dock::layout::Rect;
use rf_win::shellhost::ShellMode;
use rf_win::shellhost::appcmd::{Shell, shell_now};
use rf_win::shellhost::shellhook::ShellHook;
use rf_win::shellhost::workarea::{Done, SystemSetter, SystemShell, WorkAreaKeeper};

use crate::settings::Settings;

/// The environment variable that lets the host set the work area itself when no shell is running.
pub const ENV_WORK_AREA: &str = "RUSTYFILES_SET_WORKAREA";

/// Whether the variable's value asks for it: exactly `1`.
pub fn work_area_wanted(value: Option<&str>) -> bool {
    value.is_some_and(|v| v.trim() == "1")
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
static WORK: Mutex<Option<WorkAreaKeeper>> = Mutex::new(None);

/// The host's start, after its windows exist: brings the parts in line with the mode it started in.
pub fn start(cx: &mut App) {
    sync_keys();
    sync_work_area(cx);
}

/// `SystemEvent::ShellChanged`: the shell window is another one now, the mode is `mode`.
pub fn on_shell_changed(mode: ShellMode, cx: &mut App) {
    if let Some(l) = change_line(current(), mode) {
        log::info!("{l}");
    }
    CURRENT.store(code(mode), Ordering::Relaxed);
    super::cover::shell_changed(cx);
    sync_keys();
    sync_work_area(cx);
}

/// The dock's layout changed (the band may have to follow). Costs a comparison when the work area is not asked for.
pub fn layout_changed(cx: &mut App) {
    if work_area_wanted(std::env::var(ENV_WORK_AREA).ok().as_deref()) {
        sync_work_area(cx);
    }
}

/// How long [`stop`] waits for the hook to end and the work area to be given back.
const STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

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
        let mut g = WORK.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(k) = g.as_mut()
            && k.active()
            && let Some(m) = rf_win::dock::primary_monitor()
        {
            let done = k.apply(&SystemShell, &mut SystemSetter, None, m.rect, m.work);
            log::info!("work area: on stop {done:?}");
        }
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

/// The thickness of the dock's band for the work area, device px (`None`: no band wanted).
fn band_thickness(cx: &App) -> Option<i32> {
    let s = cx.try_global::<Settings>()?;
    if !s.dock_reserve {
        return None;
    }
    let m = rf_win::dock::primary_monitor()?;
    let unit = rf_win::dock::size::unit(m.scale(), s.dock_size.factor());
    Some(rf_win::dock::layout::reserved_thickness(unit))
}

/// Sets (or gives back) the work area itself, when it is asked for and nobody owns the shell window. A worker thread: the call
/// broadcasts the change and may wait for programs that are slow to answer.
fn sync_work_area(cx: &App) {
    if !work_area_wanted(std::env::var(ENV_WORK_AREA).ok().as_deref()) {
        return;
    }
    let thickness = band_thickness(cx);
    let spawned = std::thread::Builder::new().name("rf-workarea-sync".into()).spawn(move || {
        let Some(m) = rf_win::dock::primary_monitor() else { return };
        let mut g = WORK.lock().unwrap_or_else(|p| p.into_inner());
        let k = g.get_or_insert_with(WorkAreaKeeper::new);
        // the band is told to the dock's placement BEFORE the work area shrinks, so that the islands are not worked out from the
        // shrunken area (they float inside the band)
        if let Some(t) = thickness {
            rf_win::dock::system::set_own_band(Some(Rect { top: m.rect.bottom - t, ..m.rect }));
        }
        match k.apply(&SystemShell, &mut SystemSetter, thickness, m.rect, m.work) {
            Done::Nothing => {
                if !k.active() {
                    rf_win::dock::system::set_own_band(None);
                }
            }
            done => log::info!("work area: {done:?}"),
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
    fn the_work_area_is_asked_for_by_a_one_only() {
        assert!(!work_area_wanted(None));
        assert!(!work_area_wanted(Some("")));
        assert!(!work_area_wanted(Some("0")));
        assert!(!work_area_wanted(Some("true")));
        assert!(work_area_wanted(Some("1")));
        assert!(work_area_wanted(Some(" 1 ")));
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

    #[test]
    fn the_current_mode_follows_what_it_is_told() {
        init(ShellMode::Beside);
        assert_eq!(current(), Some(ShellMode::Beside));
        init(ShellMode::Shell);
        assert_eq!(current(), Some(ShellMode::Shell));
        init(ShellMode::Beside);
    }

    /// The only place a work area is set by this host is `sync_work_area`, behind the environment variable, and the real setter is made
    /// nowhere else in the app.
    #[test]
    fn the_real_work_area_setter_is_reached_only_from_here_and_only_behind_the_variable() {
        let here = include_str!("live.rs");
        let code = &here[..here.find("#[cfg(test)]").unwrap()];
        assert_eq!(code.matches("SystemSetter").count(), 3, "the import and the two uses, in stop and in sync_work_area");
        let sync = &code[code.find("fn sync_work_area").unwrap()..];
        assert!(sync[..200].contains("work_area_wanted"), "the variable is checked first");
        for other in [include_str!("mod.rs"), include_str!("cover.rs"), include_str!("../dock/host.rs"), include_str!("../lib.rs")] {
            assert!(!other.contains("SystemSetter"), "another file reaches the real setter");
        }
    }
}
