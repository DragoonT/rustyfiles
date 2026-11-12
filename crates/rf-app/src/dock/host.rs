//! The dock process's centre: the three island windows, their places, the system events they react to (display, DPI, theme,
//! full-screen, time), the settings file, and the clean exit.
//!
//! WHY WIN32 CALLS GO THROUGH A TASK. Moving or styling a GPUI window sends it messages (`WM_SIZE`, `WM_WINDOWPOSCHANGED`) that GPUI
//! answers by updating the window. When that happens while the app is being updated (inside a click handler, a render, a
//! `cx.update`), the answer cannot be taken and the window keeps its old size in GPUI's eyes. So every call that moves, styles,
//! shows or hides a window is made from a foreground-executor task, which runs from the message loop with nothing borrowed (this is
//! also how GPUI's own `Window::resize` does it).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::island;
use rf_win::dock::layout::{self, Islands, RADIUS, Rect, Widths};
use rf_win::dock::{DockSystem, MonitorInfo, SystemEvent};

use super::backdrop::{self, IslandGlass};
use super::control::{self, Command};
use super::left::LeftIsland;
use super::look::{Env, Palette};
use super::middle::MiddleIsland;
use super::plan;
use super::right::RightIsland;
use crate::settings::Settings;

/// How often the islands look at the brightness behind their glass: no faster than four times a second.
const TONE_EVERY: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Left,
    Middle,
    Right,
}

/// The settings the running dock acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prefs {
    /// Keep other windows from going under the dock (the application desktop toolbar).
    pub reserve: bool,
    /// The weather city, trimmed. Empty: no weather.
    pub city: String,
}

impl Prefs {
    pub fn of(s: &Settings) -> Prefs {
        Prefs { reserve: s.dock_reserve, city: s.dock_weather_city.trim().to_string() }
    }
}

/// What differs between two readings of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changes {
    pub reserve: bool,
    pub city: bool,
}

pub fn changes(old: &Prefs, new: &Prefs) -> Changes {
    Changes { reserve: old.reserve != new.reserve, city: old.city != new.city }
}

/// Whether the settings file is new since last looked at (its modification time differs; a file that appears or disappears counts).
pub fn file_changed(before: Option<SystemTime>, now: Option<SystemTime>) -> bool {
    before != now
}

struct Placed {
    kind: Kind,
    handle: AnyWindowHandle,
    hwnd: isize,
    glass: Rc<RefCell<IslandGlass>>,
}

pub struct Host {
    system: Option<DockSystem>,
    placed: Vec<Placed>,
    left: Option<Entity<LeftIsland>>,
    right: Option<Entity<RightIsland>>,
    /// The islands' logical widths (what their contents need).
    widths: Widths,
    fullscreen: bool,
    prefs: Prefs,
    settings_stamp: Option<SystemTime>,
    started: Instant,
}

impl Global for Host {}

impl Host {
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The monitor and the area the islands are placed in: the work area with our own toolbar band put back, so the islands do not
    /// climb the screen by the band's height.
    fn area(&self) -> Option<(MonitorInfo, Rect)> {
        self.system.as_ref().and_then(|s| s.placement_area()).or_else(|| rf_win::dock::primary_monitor().map(|m| (m, m.work)))
    }

    fn rectangles(&self) -> Option<(f32, Islands)> {
        let (mon, area) = self.area()?;
        let scale = mon.scale();
        Some((scale, layout::islands(area, scale, self.widths)))
    }
}

fn host_mut(cx: &mut App) -> Option<&mut Host> {
    if cx.has_global::<Host>() { Some(cx.global_mut::<Host>()) } else { None }
}

/// The screen rectangle (device px) of an island's VISIBLE glass: what a panel is put above and lined up with. Today an island's window
/// is exactly its glass, so this is the window's rectangle now; when the windows grow a margin around the glass (for a shadow), THIS is
/// the one place that has to take the margin off.
pub fn visible_glass_rect(kind: Kind, cx: &App) -> Option<Rect> {
    let host = cx.try_global::<Host>()?;
    let hwnd = host.placed.iter().find(|p| p.kind == kind)?.hwnd;
    island::rect(hwnd)
}

/// The primary monitor's scale and work area (device px): what a panel is kept inside.
pub fn monitor_work(cx: &App) -> Option<(f32, Rect)> {
    let (mon, _) = cx.try_global::<Host>()?.area()?;
    Some((mon.scale(), mon.work))
}

/// Whether a full-screen application is in front (the islands are hidden).
pub fn is_fullscreen(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.fullscreen)
}

/// The base of every island view: the window-sized box, text colour, and - only while the glass is not running - the island's own
/// rounded translucent fill, so the dock is never invisible. The colours are the island's palette at this moment (see `tone.rs`).
pub fn island_box(pal: Palette, fallback: bool) -> Div {
    div()
        .size_full()
        .text_size(px(13.))
        .text_color(pal.text().hsla())
        .when(fallback, |d| d.bg(pal.fallback_fill().hsla()).rounded(px(RADIUS)))
}

/// What a view needs of its island's glass for one frame: the palette now, and a request for another frame while the foreground is
/// still turning from one look to the other.
pub fn frame_palette(glass: &RefCell<IslandGlass>, window: &mut Window) -> (Palette, bool) {
    let now = backdrop::now_ms();
    let (pal, settled, fallback) = {
        let g = glass.borrow();
        (g.palette(now), g.tone_settled(now), g.fallback())
    };
    if !settled {
        window.request_animation_frame();
    }
    (pal, fallback)
}

/// Runs `job` from the message loop, outside any update of the app (see the module docs).
fn on_message_loop(cx: &App, job: impl FnOnce() + 'static) {
    cx.foreground_executor()
        .spawn(async move {
            job();
        })
        .detach();
}

/// An island view reports that its contents need another width (logical px): all three islands are placed again.
pub fn set_width(kind: Kind, width: f32, cx: &mut App) {
    let Some(host) = host_mut(cx) else { return };
    let slot = match kind {
        Kind::Left => &mut host.widths.left,
        Kind::Middle => &mut host.widths.middle,
        Kind::Right => &mut host.widths.right,
    };
    if (*slot - width).abs() < 0.5 {
        return;
    }
    *slot = width;
    request_layout(cx);
}

/// Places the three windows by the current work area, scale and widths. The rectangles are worked out now, the windows are moved
/// from the message loop.
pub fn request_layout(cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let Some((_, rects)) = host.rectangles() else { return };
    let jobs: Vec<(isize, Rect)> = host
        .placed
        .iter()
        .map(|p| {
            let r = match p.kind {
                Kind::Left => rects.left,
                Kind::Middle => rects.middle,
                Kind::Right => rects.right,
            };
            (p.hwnd, r)
        })
        .collect();
    on_message_loop(cx, move || {
        for (hwnd, r) in jobs {
            island::place(hwnd, r);
        }
    });
}

/// Shows or hides all three (a full-screen application is in front, or left).
fn apply_visibility(cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let visible = !host.fullscreen;
    let hwnds: Vec<isize> = host.placed.iter().map(|p| p.hwnd).collect();
    on_message_loop(cx, move || {
        for h in hwnds {
            island::set_visible(h, visible);
        }
    });
    // a panel above a hidden island would float on its own
    if !visible {
        super::panel::close(rf_win::dock::panel::CloseCause::FullScreen, cx);
    }
}

/// Puts the (re)read system facts where the views and the glass can see them.
fn publish_env(scale: f32, cx: &mut App) {
    let env = Env::read(scale);
    let changed = cx.try_global::<Env>().is_none_or(|old| *old != env);
    cx.set_global(env);
    if let Some(host) = cx.try_global::<Host>() {
        for p in &host.placed {
            p.glass.borrow_mut().sync(env.dark, env.scale, backdrop::now_ms());
        }
    }
    if changed {
        cx.refresh_windows();
    }
}

fn apply_reserve(cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let Some(system) = host.system.as_ref() else { return };
    if host.prefs.reserve {
        let scale = host.area().map_or(1.0, |(m, _)| m.scale());
        system.reserve(layout::reserved_thickness(scale));
    } else {
        system.release();
    }
}

fn on_system_event(ev: SystemEvent, cx: &mut App) {
    match ev {
        SystemEvent::LayoutChanged => {
            let scale = cx.try_global::<Host>().and_then(|h| h.area()).map_or(1.0, |(m, _)| m.scale());
            let scale_changed = cx.try_global::<Env>().is_none_or(|e| (e.scale - scale).abs() > 0.001);
            publish_env(scale, cx);
            if scale_changed {
                // the reserved band is in device px
                apply_reserve(cx);
            }
            // the islands move; a panel that is open would be left where they were
            super::panel::close(rf_win::dock::panel::CloseCause::LayoutChanged, cx);
            request_layout(cx);
        }
        SystemEvent::ThemeChanged => {
            let scale = cx.try_global::<Env>().map_or(1.0, |e| e.scale);
            publish_env(scale, cx);
        }
        SystemEvent::Fullscreen(on) => {
            if let Some(host) = host_mut(cx) {
                host.fullscreen = on;
            }
            apply_visibility(cx);
        }
        SystemEvent::TimeChanged => {
            if let Some(right) = cx.try_global::<Host>().and_then(|h| h.right.clone()) {
                right.update(cx, |v, cx| v.refresh_clock(cx));
            }
        }
    }
}

/// The settings changed (a `reload` message, or the file's time changed): read them and apply what the dock acts on. The file is
/// read on a worker thread (a locked file is retried for a moment) and is never touched: this process only follows it.
fn reload(cx: &mut App) {
    cx.spawn(async move |cx| {
        // the stamp first: a write that lands while the file is being read shows up as a change at the next look
        let (stamp, fresh) = cx.background_executor().spawn(async { (file_stamp(), Settings::load_readonly()) }).await;
        cx.update(|cx| apply_reloaded(fresh, stamp, cx));
    })
    .detach();
}

/// What a reload read: `None` is a file that could not be read or understood right now - the dock keeps what it has and looks again
/// when the file changes next.
fn apply_reloaded(fresh: Option<Settings>, stamp: Option<SystemTime>, cx: &mut App) {
    let Some(fresh) = fresh else {
        if let Some(host) = host_mut(cx) {
            host.settings_stamp = stamp;
        }
        return;
    };
    fresh.apply_side_effects();
    let new = Prefs::of(&fresh);
    cx.set_global(fresh);
    let Some(host) = host_mut(cx) else { return };
    host.settings_stamp = stamp;
    let ch = changes(&host.prefs, &new);
    host.prefs = new.clone();
    let left = host.left.clone();
    if ch.reserve {
        apply_reserve(cx);
    }
    if ch.city
        && let Some(left) = left
    {
        left.update(cx, |v, cx| v.set_city(&new.city, cx));
    }
    cx.refresh_windows();
}

fn file_stamp() -> Option<SystemTime> {
    crate::settings::path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}

/// Undoes everything the dock did and ends the process: the glass is taken off every island (the capture exclusion goes with it), the
/// windows are closed, the toolbar band is given back, then the app quits. Safe to call twice. Never saves the settings: this process
/// only ever reads them.
pub fn shutdown(cx: &mut App) {
    if !cx.has_global::<Host>() {
        return;
    }
    let host = cx.remove_global::<Host>();
    super::panel::shutdown(cx);
    for p in &host.placed {
        p.glass.borrow_mut().release();
        let _ = p.handle.update(cx, |_, window, _| window.remove_window());
    }
    // (dropping the system object unregisters the toolbar and ends its window; it must not be dropped from its own callback, and
    // this is not)
    drop(host);
    log::info!("dock: stopped");
    cx.quit();
}

/// Opens one island window at the rectangle (device px). The window is a transparent pop-up that is not activated when shown; its
/// style, its exact place and its glass follow from the message loop (see the module docs).
fn open_island<V: Render + 'static>(
    rect: Rect,
    scale: f32,
    display: Option<DisplayId>,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    cx: &mut App,
) -> Option<(AnyWindowHandle, isize, Entity<V>)> {
    let (x, y, w, h) = plan::logical(rect, scale);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))),
        display_id: display,
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let handle = match cx.open_window(opts, move |window, cx| cx.new(|cx| build(window, cx))) {
        Ok(h) => h,
        Err(e) => {
            log::error!("dock: cannot open an island: {e:#}");
            return None;
        }
    };
    let hwnd = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).ok()?;
    let entity = handle.entity(cx).ok()?;
    Some((handle.into(), hwnd, entity))
}

/// The dock process's start (called once, with the settings loaded, after the application is running).
pub fn run(messages: Option<UnboundedReceiver<String>>, cx: &mut App) {
    let prefs = Prefs::of(cx.global::<Settings>());
    let (tx, mut events) = futures::channel::mpsc::unbounded::<SystemEvent>();
    let system = match DockSystem::start(move |e| {
        let _ = tx.unbounded_send(e);
    }) {
        Ok(s) => Some(s),
        Err(e) => {
            // the islands still float; they just do not hear about display changes and cannot keep windows out from under them
            log::error!("dock: the system watcher did not start: {e:#}");
            None
        }
    };
    let area = system.as_ref().and_then(|s| s.placement_area()).or_else(|| rf_win::dock::primary_monitor().map(|m| (m, m.work)));
    let Some((monitor, area)) = area else {
        log::error!("dock: this session has no monitor to put the dock on");
        cx.quit();
        return;
    };
    let scale = monitor.scale();
    let env = Env::read(scale);
    cx.set_global(env);

    let lines = rf_win::dock::clock::system_lines();
    let widths = Widths { left: plan::left_width(None), middle: plan::middle_width_at_rest(2, 1), right: plan::right_width(&lines) };
    let rects = layout::islands(area, scale, widths);
    let display = cx.primary_display().map(|d| d.id());

    let (left_glass, middle_glass, right_glass) = (Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())));
    let city = prefs.city.clone();
    let panel_glass = left_glass.clone();
    let lg = left_glass.clone();
    let left = open_island(rects.left, scale, display, move |_, cx| LeftIsland::new(lg, &city, cx), cx);
    let mg = middle_glass.clone();
    let middle = open_island(rects.middle, scale, display, move |_, cx| MiddleIsland::new(mg, cx), cx);
    let rg = right_glass.clone();
    let panel_right_glass = right_glass.clone();
    let right_lines = lines.clone();
    let right = open_island(rects.right, scale, display, move |_, cx| RightIsland::new(rg, right_lines, cx), cx);

    let mut placed = Vec::new();
    let mut left_entity = None;
    let mut right_entity = None;
    if let Some((handle, hwnd, entity)) = left {
        placed.push(Placed { kind: Kind::Left, handle, hwnd, glass: left_glass });
        left_entity = Some(entity);
    }
    if let Some((handle, hwnd, _)) = middle {
        placed.push(Placed { kind: Kind::Middle, handle, hwnd, glass: middle_glass });
    }
    if let Some((handle, hwnd, entity)) = right {
        placed.push(Placed { kind: Kind::Right, handle, hwnd, glass: right_glass });
        right_entity = Some(entity);
    }
    if placed.is_empty() {
        log::error!("dock: no island could be opened");
        drop(system);
        cx.quit();
        return;
    }

    let panel_left = left_entity.clone();
    let sys_right = right_entity.clone();
    let fullscreen = system.as_ref().is_some_and(|s| s.fullscreen());
    let host = Host {
        system,
        placed,
        left: left_entity,
        right: right_entity,
        widths,
        fullscreen,
        prefs: prefs.clone(),
        settings_stamp: file_stamp(),
        started: Instant::now(),
    };
    // the first styling, placing and glass, from the message loop (the windows exist now but are still being set up)
    let setup: Vec<(isize, Rect, Rc<RefCell<IslandGlass>>)> = host
        .placed
        .iter()
        .map(|p| {
            let r = match p.kind {
                Kind::Left => rects.left,
                Kind::Middle => rects.middle,
                Kind::Right => rects.right,
            };
            (p.hwnd, r, p.glass.clone())
        })
        .collect();
    cx.set_global(host);
    // the panels (Start and search): their window is made now, hidden, so that opening one is a show and not a creation
    if let Some(left) = panel_left {
        super::panel::init(panel_glass, left, display, scale, rects.left, cx);
    }
    // the control centre's window, hidden too, and what the right island's symbols follow all the time (sound, network)
    super::panel::init_control_centre(panel_right_glass, display, scale, rects.right, cx);
    if let Some(right) = sys_right {
        super::sysstate::start(right, cx);
    }
    on_message_loop(cx, move || {
        for (hwnd, r, _) in &setup {
            if let Err(e) = island::make_island(*hwnd) {
                log::warn!("dock: island style: {e:#}");
            }
            island::place(*hwnd, *r);
        }
        let env = Env::read(scale);
        for (hwnd, _, glass) in &setup {
            glass.borrow_mut().start(*hwnd, env.dark, env.scale, 0);
        }
    });
    if fullscreen {
        apply_visibility(cx);
    }
    apply_reserve(cx);

    // system events -> the app
    cx.spawn(async move |cx| {
        while let Some(ev) = events.next().await {
            cx.update(|cx| on_system_event(ev, cx));
        }
    })
    .detach();

    // messages from the main rustyfiles (and from a second `--dock`): stop, reload
    if let Some(mut messages) = messages {
        cx.spawn(async move |cx| {
            while let Some(line) = messages.next().await {
                // the main app's answer to a file search (see search_pipe)
                if crate::search_pipe::is_reply(&line) {
                    cx.update(|cx| super::start::on_files_reply(&line, cx));
                    continue;
                }
                match control::parse_command(&line) {
                    Command::Stop => {
                        cx.update(shutdown);
                        break;
                    }
                    Command::Reload => cx.update(reload),
                    Command::Nothing => {}
                }
            }
        })
        .detach();
    }

    // the glass of each island: looked at often until it runs, then now and then (a glass lost later is noticed)
    cx.spawn(async move |cx| {
        loop {
            let wait = cx.update(|cx| {
                let host = cx.try_global::<Host>()?;
                let now = host.now_ms();
                let mut changed = false;
                let mut all_settled = true;
                for p in &host.placed {
                    let mut g = p.glass.borrow_mut();
                    changed |= g.tick(now);
                    all_settled &= g.settled();
                }
                if changed {
                    cx.refresh_windows();
                }
                Some(Duration::from_millis(if all_settled { 1000 } else { 100 }))
            });
            let Some(wait) = wait else { break };
            cx.background_executor().timer(wait).await;
        }
    })
    .detach();

    // The brightness behind each glass, four times a second: the engine measures it at its draws and keeps the last value per window;
    // here an island only looks at it. Nothing is drawn unless an island's foreground has to turn.
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(TONE_EVERY).await;
            let alive = cx.update(|cx| {
                let Some(host) = cx.try_global::<Host>() else { return false };
                let now = backdrop::now_ms();
                let mut changed = false;
                for p in &host.placed {
                    changed |= p.glass.borrow_mut().poll_tone(now);
                }
                if changed {
                    cx.refresh_windows();
                }
                true
            });
            if !alive {
                break;
            }
        }
    })
    .detach();

    // the settings file, for edits made by hand or by a rustyfiles that could not reach us
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            let alive = cx.update(|cx| {
                let Some(host) = cx.try_global::<Host>() else { return false };
                if file_changed(host.settings_stamp, file_stamp()) {
                    reload(cx);
                }
                true
            });
            if !alive {
                break;
            }
        }
    })
    .detach();

    // all islands gone for some other reason: nothing left to run
    cx.on_window_closed(|cx, _| {
        if super::panel::only_panels_left(cx) {
            shutdown(cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{Changes, Prefs, Settings, changes, file_changed};
    use std::time::{Duration, UNIX_EPOCH};

    fn prefs(reserve: bool, city: &str) -> Prefs {
        Prefs { reserve, city: city.into() }
    }

    #[test]
    fn preferences_come_from_the_settings_trimmed() {
        let s = Settings { dock_reserve: false, dock_weather_city: "  Erzincan ".into(), ..Default::default() };
        assert_eq!(Prefs::of(&s), prefs(false, "Erzincan"));
        let d = Prefs::of(&Settings::default());
        assert!(d.reserve);
        assert!(d.city.is_empty());
    }

    #[test]
    fn only_what_changed_is_applied() {
        let base = prefs(true, "Ankara");
        assert_eq!(changes(&base, &base), Changes { reserve: false, city: false });
        assert_eq!(changes(&base, &prefs(false, "Ankara")), Changes { reserve: true, city: false });
        assert_eq!(changes(&base, &prefs(true, "İzmir")), Changes { reserve: false, city: true });
        assert_eq!(changes(&base, &prefs(false, "")), Changes { reserve: true, city: true });
    }

    #[test]
    fn a_new_modification_time_means_read_again() {
        let t = UNIX_EPOCH + Duration::from_secs(100);
        assert!(!file_changed(Some(t), Some(t)));
        assert!(file_changed(Some(t), Some(t + Duration::from_secs(1))));
        assert!(file_changed(None, Some(t)), "the file appeared");
        assert!(file_changed(Some(t), None), "the file went");
        assert!(!file_changed(None, None));
    }
}
