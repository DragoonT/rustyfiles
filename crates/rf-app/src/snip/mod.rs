//! Our own screen snip (Win+Shift+S is Explorer's, so without Explorer there was none). The Windows half is `rf_win::snip` (capture,
//! geometry, the pictures' bytes, the clipboard, the file); this is the interface: one borderless window per monitor that shows the
//! frozen screen dimmed, the small toolbar, and the notice that follows.
//!
//! HOW TO START IT
//! * Win+Shift+S, through the key layer (`shell/keylayer.rs`), only while Explorer is not the shell;
//! * the "Ekran alıntısı" button of the dock's control centre;
//! * `rustyfiles.exe --snip`: a process of its own that does only this and ends when it is over ([`run_standalone`]); it does not
//!   forward to a running rustyfiles and needs neither the dock nor the key layer.
//!
//! WHAT RUNS WHEN. Nothing exists until a snip is asked for: no thread, no hook, no window, no memory. A snip is: one thread takes the
//! picture of every monitor (`BitBlt`, tens of milliseconds) and ends; the pictures live inside the textures the overlays draw (once:
//! the cut reads the same bytes); a timer of 30 ms looks at the Escape key while the overlay is up; on release the windows close and
//! a thread cuts, encodes, copies and (with the setting) saves; the notice stays six seconds. Then nothing is left.
//!
//! NOTHING TAKES THE FOREGROUND. Every window here is `WS_EX_NOACTIVATE` and answers a click's activation with `MA_NOACTIVATE` itself
//! (the toolkit answers `MA_ACTIVATE`): the program the user was in keeps the keyboard, a click does not raise an overlay over its own
//! toolbar, and there is nothing to give back. So Escape is not read from a focused window. While the overlay is up a hook for that one
//! key (`rf_win::snip::escape`) reports it AND swallows it, so the same Escape does not also cancel the dialog or leave the full-screen
//! video underneath; if the hook cannot be installed the key's state is looked at instead (`place::escape_down`) and the key does reach
//! the other program. Every other key still goes to the program underneath.
//!
//! NOTHING STAYS UP BY ITSELF. An overlay that is up is taken away when another program comes to the front (Alt+Tab, a call, a lock
//! screen), when a monitor is plugged or unplugged, and after two minutes (`plan::SessionWatch`); a picture that has not come in ten
//! seconds is given up. Every failure before the overlay stands ends in the notice "Alıntı alınamadı", never in silence. A second start
//! while an overlay is up closes it and begins again.
//!
//! THE FILES
//! | file | what |
//! |---|---|
//! | `plan.rs` | pure: where the toolbar and the notice go, what is dimmed, the notice's words and rules, the Escape edge |
//! | `finish.rs` | the cut, the formats, the clipboard and the file, over a clipboard trait |
//! | `overlay.rs` | the window of one monitor |
//! | `toolbar.rs` | Bölge / Pencere / Tam ekran and the close button, on HyperGlass |
//! | `notice.rs` | "Panoya kopyalandı" and Kaydet, in the dock's tone |

mod finish;
mod notice;
mod overlay;
pub mod plan;
mod toolbar;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::*;
use rf_core::desktop::geom::Rect;
use rf_win::desktop::monitors::MonitorInfo;
use rf_win::snip::capture::Captured;
use rf_win::snip::escape::EscapeGrab;
use rf_win::snip::geom::Monitor;
use rf_win::snip::model::Model;

use crate::settings::Settings;
use crate::t;
use finish::Source;
use notice::NoticeView;
use overlay::OverlayView;
use toolbar::ToolbarView;

/// `rustyfiles.exe --snip`.
pub const ARG: &str = "--snip";

/// Whether the first command-line argument asks for the standalone snip.
pub fn is_arg(first: Option<&str>) -> bool {
    first == Some(ARG)
}

/// A picture is being taken (between the key and the overlay).
static CAPTURING: AtomicBool = AtomicBool::new(false);
/// Threads that are cutting and copying (between the release and the notice).
static WORKING: AtomicUsize = AtomicUsize::new(0);
/// This process is `--snip`: it ends when the snip is over.
static STANDALONE: AtomicBool = AtomicBool::new(false);

/// What the overlay's windows share: the model (the mode, the drag, the highlight) and the pictures.
pub struct State {
    pub model: Model,
    /// One per monitor, in the model's order.
    pub sources: Vec<Source>,
}

/// The overlay that is up.
struct Session {
    state: Entity<State>,
    overlays: Vec<WindowHandle<OverlayView>>,
    toolbar: Option<WindowHandle<ToolbarView>>,
    // (kept alive: dropping them stops them)
    _tasks: Vec<Task<()>>,
    /// The hook that takes Escape from the program underneath; it goes off with the session.
    _escape: Option<EscapeGrab>,
}

impl Global for Session {}

/// The notice that is up.
struct NoticeHost {
    handle: WindowHandle<NoticeView>,
}

impl Global for NoticeHost {}

/// A snip is over; a `--snip` process has nothing more to do when nothing of it is left.
fn ended(cx: &mut App) {
    if STANDALONE.load(Ordering::Relaxed)
        && plan::nothing_left(CAPTURING.load(Ordering::Relaxed), WORKING.load(Ordering::Relaxed), cx.has_global::<Session>(), cx.has_global::<NoticeHost>())
    {
        cx.quit();
    }
}

/// Work that has to finish whatever becomes of the windows (a write to the disk) is counted from here...
fn work_begin() {
    WORKING.fetch_add(1, Ordering::Relaxed);
}

/// ... to here.
fn work_end(cx: &mut App) {
    WORKING.fetch_sub(1, Ordering::Relaxed);
    ended(cx);
}

/// `--snip`: the process is only this. Called once the application runs.
pub fn run_standalone(cx: &mut App) {
    STANDALONE.store(true, Ordering::Relaxed);
    start(cx);
    // (nothing started: nothing to wait for)
    ended(cx);
}

/// Starts a snip after `wait` (the control centre's button: its panel is put away first, so that it is not in the picture).
pub fn start_after(wait: Duration, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(wait).await;
        cx.update(start);
    })
    .detach();
}

/// Starts a snip: the screen is captured on a thread, then the overlay opens. An overlay that is already up is closed and the snip
/// begins again; nothing is done while a picture is being taken or a copy is being made.
pub fn start(cx: &mut App) {
    let had_session = close_session(cx).is_some();
    if busy_for_start(cx) {
        log::info!("snip: one is already going on");
        return;
    }
    // the last snip's notice (or overlay) is put away first; the screen needs a moment to show the desktop without it
    let settle = cx.has_global::<NoticeHost>() || had_session;
    close_notice_window(cx);
    CAPTURING.store(true, Ordering::Relaxed);
    let (tx, rx) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new().name("rf-snip-capture".into()).spawn(move || {
        if settle {
            std::thread::sleep(Duration::from_millis(90));
        }
        let _ = tx.send(rf_win::snip::capture::capture_all());
    });
    if let Err(e) = spawned {
        CAPTURING.store(false, Ordering::Relaxed);
        return fail(&format!("the capture thread could not be started: {e}"), &[], cx);
    }
    cx.spawn(async move |cx| {
        // (a picture that never comes must not hold every later snip back: after the ceiling the wait is given up, and the thread, if
        // it ever answers, finds nobody)
        let timer = cx.background_executor().timer(Duration::from_millis(plan::CAPTURE_MAX_MS));
        let res = match futures::future::select(rx, timer).await {
            futures::future::Either::Left((r, _)) => Some(r),
            futures::future::Either::Right(_) => None,
        };
        cx.update(|cx| {
            CAPTURING.store(false, Ordering::Relaxed);
            match res {
                Some(Ok(Ok(captured))) => begin(captured, cx),
                Some(Ok(Err(e))) => fail(&format!("the screen could not be captured: {e}"), &[], cx),
                Some(Err(_)) => fail("the capture thread ended without a picture", &[], cx),
                None => fail("the capture did not finish in time", &[], cx),
            }
        });
    })
    .detach();
}

/// Something went wrong before the overlay could stand. The user pressed a key (or a button) and must hear that nothing came of it:
/// the notice says "Alıntı alınamadı" on the monitor under the pointer. `monitors` may be empty (the capture did not get far enough to
/// know them); they are read then.
fn fail(why: &str, monitors: &[Monitor], cx: &mut App) {
    log::warn!("snip: {why}");
    let owned;
    let monitors = if monitors.is_empty() {
        owned = rf_win::snip::capture::monitors();
        &owned[..]
    } else {
        monitors
    };
    if monitors.is_empty() {
        return ended(cx);
    }
    let on = plan::toolbar_monitor(rf_win::snip::place::cursor(), monitors);
    open_notice(finish::Made::failed(why), on, monitors, cx);
}

/// A snip may start unless a picture is being taken, an overlay is up or a copy is being made (a notice does not hold one back).
fn busy_for_start(cx: &App) -> bool {
    CAPTURING.load(Ordering::Relaxed) || WORKING.load(Ordering::Relaxed) > 0 || cx.has_global::<Session>()
}

fn display_of(m: &Monitor, infos: &[MonitorInfo], cx: &App) -> Option<DisplayId> {
    let info = infos.iter().find(|i| i.rect == m.rect)?;
    cx.displays()
        .into_iter()
        .find(|d| {
            let b = d.bounds();
            crate::shell::cover::is_same_display((f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)), info)
        })
        .map(|d| d.id())
}

/// A pop-up of the toolkit made hidden: it is shown by `rf_win::snip::place` once it has its place, so it does not flash in the wrong one.
fn hidden_popup(x: f32, y: f32, w: f32, h: f32, display: Option<DisplayId>, background: WindowBackgroundAppearance) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))),
        display_id: display,
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: background,
        ..Default::default()
    }
}

fn begin(captured: Captured, cx: &mut App) {
    let Captured { shot, monitors, windows, took } = captured;
    log::info!("snip: {} monitor(s) captured in {} ms, {} window(s) listed", monitors.len(), took.as_millis(), windows.len());
    let mut sources = Vec::with_capacity(shot.parts.len());
    for (rect, f) in shot.parts {
        // (the bytes move into the picture: no second copy of a screen)
        let Some(source) = Source::from_frame(rect, f) else {
            return fail("a captured picture is not the size it says", &monitors, cx);
        };
        sources.push(source);
    }
    if sources.len() != monitors.len() {
        return fail(&format!("{} pictures for {} monitors", sources.len(), monitors.len()), &monitors, cx);
    }
    let infos = rf_win::desktop::monitors::list();
    let pointer = rf_win::snip::place::cursor();
    let on = plan::toolbar_monitor(pointer, &monitors);
    let state = cx.new(|_| State { model: Model::new(monitors.clone(), windows), sources: sources.clone() });

    let mut overlays = Vec::new();
    let mut placed: Vec<(isize, Rect)> = Vec::new();
    for (i, m) in monitors.iter().enumerate() {
        let (x, y, w, h) = crate::shell::cover::logical_bounds(m.rect, m.scale);
        let opts = hidden_popup(x, y, w, h, display_of(m, &infos, cx), WindowBackgroundAppearance::Opaque);
        let (st, image) = (state.clone(), sources[i].image.clone());
        match cx.open_window(opts, move |_, cx| cx.new(|cx| OverlayView::new(i, st, image, cx))) {
            Ok(h) => {
                if let Ok(hwnd) = h.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) {
                    placed.push((hwnd, m.rect));
                    overlays.push(h);
                }
            }
            Err(e) => log::warn!("snip: a window for a monitor could not be made: {e:#}"),
        }
    }
    if overlays.is_empty() {
        return fail("no overlay window could be made", &monitors, cx);
    }
    // the sources are held by the state and the views; this local copy is not needed any more
    drop(sources);

    let bar = plan::toolbar_rect(&monitors[on]);
    let (bx, by, bw, bh) = crate::shell::cover::logical_bounds(bar, monitors[on].scale);
    let toolbar_hwnd_and_handle = {
        let opts = hidden_popup(bx, by, bw, bh, display_of(&monitors[on], &infos, cx), WindowBackgroundAppearance::Transparent);
        let st = state.clone();
        match cx.open_window(opts, move |window, cx| cx.new(|cx| ToolbarView::new(st, window, cx))) {
            Ok(h) => h.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).ok().map(|hwnd| (hwnd, h)),
            Err(e) => {
                log::warn!("snip: the toolbar could not be made: {e:#}");
                None
            }
        }
    };
    let toolbar = toolbar_hwnd_and_handle.as_ref().map(|(_, h)| *h);
    let toolbar_hwnd = toolbar_hwnd_and_handle.map(|(hwnd, _)| hwnd);

    // Escape: taken away from the program underneath by a hook for that one key while the overlay is up. If the hook cannot be installed
    // the keyboard's state is looked at instead (and the key then also reaches the other program).
    let grab = EscapeGrab::start();
    if grab.is_none() {
        log::warn!("snip: the Escape hook could not be installed; Escape cancels but also reaches the program underneath");
    }
    let started = Instant::now();
    let mut guard = plan::SessionWatch::new(rf_win::snip::place::foreground(), rf_win::snip::place::screen_signature());
    let esc = cx.spawn(async move |cx| {
        let mut watch = plan::EscWatch::default();
        loop {
            cx.background_executor().timer(Duration::from_millis(30)).await;
            let keep_going = cx.update(|cx| {
                if !cx.has_global::<Session>() {
                    return false;
                }
                let pressed = if rf_win::snip::escape::active() { rf_win::snip::escape::take_pressed() } else { watch.feed(rf_win::snip::place::escape_down()) };
                if pressed {
                    cancel(cx);
                    return false;
                }
                // nothing else ends an overlay nobody is looking at
                let fg = rf_win::snip::place::foreground();
                let ours = fg != 0 && rf_win::snip::place::is_ours(fg);
                if let Some(why) = guard.check(started.elapsed().as_millis() as u64, fg, ours, rf_win::snip::place::screen_signature()) {
                    log::info!("snip: ended by the watch: {why:?}");
                    cancel(cx);
                    return false;
                }
                true
            });
            if !keep_going {
                break;
            }
        }
    });
    cx.set_global(Session { state, overlays, toolbar, _tasks: vec![esc], _escape: grab });

    // the style, the procedure, the place and the show: from the message loop, the overlays first and the toolbar above them
    let bar_handle = toolbar;
    cx.spawn(async move |cx| {
        let mut shown = 0usize;
        for (hwnd, rect) in placed {
            if let Err(e) = rf_win::snip::place::prepare(hwnd) {
                log::warn!("snip: {e:#}");
                continue;
            }
            if rf_win::snip::place::place(hwnd, rect) {
                shown += 1;
            } else {
                log::warn!("snip: Windows refused to place an overlay");
            }
        }
        if shown == 0 {
            // nothing the user can see stands: the session is not kept (it would be invisible and hold every later snip back)
            cx.update(|cx| {
                close_session(cx);
                fail("no overlay window could be shown", &monitors, cx);
            });
            return;
        }
        if let (Some(hwnd), Some(h)) = (toolbar_hwnd, bar_handle) {
            rf_win::snip::place::prepare_small(hwnd);
            rf_win::snip::place::place_small(hwnd, bar);
            cx.update(|cx| {
                let _ = h.update(cx, |v, _, cx| v.start_glass(cx));
            });
        }
    })
    .detach();
}

/// The overlay's windows close and the session ends. Returns what it held, so that the caller can still use its state.
fn close_session(cx: &mut App) -> Option<Session> {
    if !cx.has_global::<Session>() {
        return None;
    }
    let s = cx.remove_global::<Session>();
    for h in &s.overlays {
        let _ = h.update(cx, |v, window, _| {
            v.release_picture(window);
            window.remove_window();
        });
    }
    if let Some(h) = &s.toolbar {
        let _ = h.update(cx, |v, window, _| {
            v.release_glass();
            window.remove_window();
        });
    }
    Some(s)
}

/// Escape, a right click or the close button.
pub fn cancel(cx: &mut App) {
    if close_session(cx).is_some() {
        log::info!("snip: cancelled");
        ended(cx);
    }
}

/// The user took `region` (device px of the virtual screen): the overlay goes away at once and the rest happens on a thread.
fn take(region: Rect, cx: &mut App) {
    let Some(session) = close_session(cx) else { return };
    let (sources, monitors) = {
        let st = session.state.read(cx);
        (st.sources.clone(), st.model.monitors().to_vec())
    };
    drop(session);
    let on = plan::notice_monitor(&region, &monitors);
    let autosave = cx.try_global::<Settings>().is_some_and(|s| s.snip_autosave);
    work_begin();
    let (tx, rx) = futures::channel::oneshot::channel();
    let spawned = std::thread::Builder::new().name("rf-snip-copy".into()).spawn(move || {
        let made = finish::run(&sources, region, autosave);
        // (the pictures go with this thread: the memory is back before the notice has been read)
        drop(sources);
        let _ = tx.send(made);
    });
    if let Err(e) = spawned {
        // (the failure notice first: a `--snip` process ends when nothing is left, and the notice is what is left)
        fail(&format!("the copy thread could not be started: {e}"), &monitors, cx);
        return work_end(cx);
    }
    cx.spawn(async move |cx| {
        let made = rx.await;
        cx.update(|cx| {
            WORKING.fetch_sub(1, Ordering::Relaxed);
            match made {
                Ok(m) => open_notice(m, on, &monitors, cx),
                Err(_) => ended(cx),
            }
        });
    })
    .detach();
}

fn open_notice(made: finish::Made, on: usize, monitors: &[Monitor], cx: &mut App) {
    let Some(m) = monitors.get(on) else { return ended(cx) };
    let o: &plan::Outcome = &made.outcome;
    log::info!("snip: copied {}, saved {}", o.copied.is_ok(), matches!(o.saved, Some(Ok(_))));
    let infos = rf_win::desktop::monitors::list();
    let work = infos.iter().find(|i| i.rect == m.rect).map_or(m.rect, |i| i.work);
    let rect = plan::notice_rect(m, &work);
    let (x, y, w, h) = crate::shell::cover::logical_bounds(rect, m.scale);
    let opts = hidden_popup(x, y, w, h, display_of(m, &infos, cx), WindowBackgroundAppearance::Transparent);
    let handle = match cx.open_window(opts, move |window, cx| cx.new(|cx| NoticeView::new(made, window, cx))) {
        Ok(h) => h,
        Err(e) => {
            log::warn!("snip: the notice could not be made: {e:#}");
            return ended(cx);
        }
    };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return ended(cx) };
    cx.set_global(NoticeHost { handle });
    cx.spawn(async move |cx| {
        rf_win::snip::place::prepare_small(hwnd);
        rf_win::snip::place::place_small(hwnd, rect);
        cx.update(|cx| {
            if let Some(h) = cx.try_global::<NoticeHost>().map(|n| n.handle) {
                let _ = h.update(cx, |v, _, cx| v.begin(cx));
            }
        });
    })
    .detach();
}

/// Puts the notice's window away (if there is one) without looking at what ends.
fn close_notice_window(cx: &mut App) {
    if !cx.has_global::<NoticeHost>() {
        return;
    }
    let host = cx.remove_global::<NoticeHost>();
    let _ = host.handle.update(cx, |v, window, _| {
        v.release_glass();
        window.remove_window();
    });
}

/// The notice goes (its time is over, or its close button).
fn close_notice(cx: &mut App) {
    close_notice_window(cx);
    ended(cx);
}

/// The Settings page's card for the one setting.
pub fn settings_cards(p: &crate::theme::Palette, s: &Settings) -> Vec<AnyElement> {
    use gpui_kit::assets::IconName as L;
    vec![
        crate::settings_view::section(p, t!("Ekran alıntısı")),
        crate::settings_view::card(
            p,
            L::Scissors,
            t!("Her alıntıyı Resimler içindeki Ekran Görüntüleri klasörüne kaydet"),
            Some(t!("Kapalıyken alıntı yalnızca panoya kopyalanır; bildirimdeki Kaydet düğmesi onu elle kaydeder. Açıkken her alıntı ayrıca Ekran alıntısı YYYY-AA-GG SSddss.png adıyla kaydedilir. Ekran alıntısı Explorer çalışmıyorken Win+Shift+S ile (Windows tuşu anahtarı açıkken), Dock'un denetim merkezindeki düğmeyle ya da rustyfiles.exe --snip ile başlar.")),
            crate::settings_view::sw("snip-autosave", p, s.snip_autosave, |s, v| s.snip_autosave = v),
        ),
    ]
}

#[cfg(test)]
mod tests {
    // (explicit names: a glob would bring in the toolkit's `test` attribute)
    use super::{Settings, WindowBackgroundAppearance, hidden_popup, is_arg};

    #[test]
    fn only_the_exact_argument_starts_the_standalone_snip() {
        assert!(is_arg(Some("--snip")));
        assert!(!is_arg(Some("--snip-x")));
        assert!(!is_arg(Some("C:\\snip")));
        assert!(!is_arg(None));
    }

    #[test]
    fn the_overlay_windows_are_made_hidden_and_never_take_the_focus() {
        let o = hidden_popup(1.0, 2.0, 300.0, 200.0, None, WindowBackgroundAppearance::Opaque);
        assert!(!o.show && !o.focus, "shown by `place` once placed, never focused");
        assert!(o.titlebar.is_none() && !o.is_movable && !o.is_resizable && !o.is_minimizable);
    }

    #[test]
    fn the_settings_card_is_off_by_default_and_changes_its_own_field() {
        let mut s = Settings::default();
        assert!(!s.snip_autosave, "nothing changes for a user who does not ask");
        s.snip_autosave = true;
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(json["snip_autosave"], true);
    }
}
