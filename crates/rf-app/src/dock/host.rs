//! The dock process's centre: the three island windows, their places, the system events they react to (display, DPI, theme,
//! full-screen, time), the settings file, and the clean exit.
//!
//! THE WINDOWS ARE BIGGER THAN THE GLASS. Each island's window holds its glass plus a transparent margin (a few px; room for the shadow only with `RUSTYFILES_DOCK_SHADOW=1`; for the middle
//! island also headroom for the icons that grow out of the glass and room for the glass to widen). `layout::placements` works out
//! both rectangles; the host keeps the result (`Host::geo`) for the views, tells the glass engine where the glass is inside its window
//! (`IslandGlass::set_shape`) and gives every window a REGION (`rf_win::dock::hit`) so that the margin lets the pointer through to
//! other programs' windows.
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
use rf_win::dock::size_live::{self, Act, File as SizeFile, Live as SizeLive, Msg as SizeMsg, Redo, Step};
use rf_win::dock::layout::{self, Islands, Placement, Placements, RADIUS, Rect, Widths};
use rf_win::dock::hit;
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
    /// The weather city, trimmed. Empty: no weather from a city.
    pub city: String,
    /// The weather from the PC's position (Windows' location service); the city is the fallback.
    pub location: bool,
    /// How big the whole dock is.
    pub size: super::size_pref::DockSizePref,
}

impl Prefs {
    pub fn of(s: &Settings) -> Prefs {
        Prefs { reserve: s.dock_reserve, city: s.dock_weather_city.trim().to_string(), location: s.dock_weather_location, size: s.dock_size }
    }
}

/// What differs between two readings of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changes {
    pub reserve: bool,
    /// The weather's source changed: the city or the switch for the PC's position.
    pub city: bool,
    pub size: bool,
}

pub fn changes(old: &Prefs, new: &Prefs) -> Changes {
    Changes { reserve: old.reserve != new.reserve, city: old.city != new.city || old.location != new.location, size: old.size != new.size }
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

/// Everything the host works out from the display and the islands' widths: the glass rectangles, the windows around them, and how far
/// the middle island's glass may widen. Device px. Kept (see `Host::geo`) so the views do not ask Windows for it on every frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geo {
    /// The monitor's DPI scale: device px per logical px of a window's own coordinates.
    pub scale: f32,
    /// Device px per dock unit (the monitor's scale times the dock's size): what the layout was worked out with. Equal to `scale` at the
    /// default size.
    pub unit: f32,
    pub islands: Islands,
    pub placements: Placements,
    /// How far the middle glass may widen on each side.
    pub growth_px: i32,
    /// The width of the area the islands were placed in (device px): the display's room, kept so that the left island can tell whether
    /// the weather's condition line fits without asking Windows about the monitor on every frame.
    pub area_width: i32,
}

impl Geo {
    pub fn placement(&self, kind: Kind) -> Placement {
        match kind {
            Kind::Left => self.placements.left,
            Kind::Middle => self.placements.middle,
            Kind::Right => self.placements.right,
        }
    }

    /// An island's visible glass at rest, screen coordinates (device px): see [`visible_glass_rect`].
    pub fn glass(&self, kind: Kind) -> Rect {
        self.placement(kind).glass
    }
}

/// What a view needs to know about its window, logical px: the distances from the window's edges to the glass at rest (left, top, right,
/// bottom) and how far the glass may widen to each side (the middle island's).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Insets {
    pub margins: [f32; 4],
    pub growth: f32,
}

impl Insets {
    pub fn of(geo: &Geo, kind: Kind) -> Insets {
        let m = geo.placement(kind).margins();
        Insets { margins: m.map(|v| v as f32 / geo.scale), growth: geo.growth_px as f32 / geo.scale }
    }

    /// What a window has before the host has worked out the real numbers (the first frame): the margins the layout wants, all round,
    /// for a dock of this size factor (logical px of the window's own coordinates).
    pub fn assumed(kind: Kind, size: f32) -> Insets {
        let (edge, head) = if layout::shadow_on() { (layout::SHADOW_MARGIN, layout::SHADOW_MARGIN) } else { (layout::EDGE_MARGIN, layout::HEADROOM) };
        let (side, top) = match kind {
            Kind::Middle => (edge + layout::MIDDLE_RESERVE, head),
            _ => (edge, edge),
        };
        let grow = if kind == Kind::Middle { layout::MIDDLE_RESERVE } else { 0.0 };
        Insets { margins: [side * size, top * size, side * size, edge * size], growth: grow * size }
    }
}

/// The insets of an island's window, from the host's last layout (real logical px of the window; a view that lays itself out in dock
/// units divides them by the size factor).
pub fn insets(kind: Kind, cx: &App) -> Insets {
    cx.try_global::<Host>().and_then(|h| h.geo).map_or_else(|| Insets::assumed(kind, super::look::dock_size(cx)), |g| Insets::of(&g, kind))
}

pub struct Host {
    system: Option<DockSystem>,
    placed: Vec<Placed>,
    left: Option<Entity<LeftIsland>>,
    /// The middle island, told when the dock's size changes (its icons are asked for again at the new size).
    middle: Option<Entity<MiddleIsland>>,
    right: Option<Entity<RightIsland>>,
    /// The islands' logical widths (what their contents need).
    widths: Widths,
    /// The last layout (see [`Geo`]); `None` until a monitor has been seen.
    geo: Option<Geo>,
    fullscreen: bool,
    prefs: Prefs,
    settings_stamp: Option<SystemTime>,
    started: Instant,
    /// The size the Dock page's slider gives while it is dragged (see `rf_win::dock::size_live`): the dock follows the slider's messages
    /// and keeps the settings file from overruling it until the drag is over.
    live: SizeLive,
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

    /// The layout for the display as it is now.
    fn compute_geo(&self) -> Option<Geo> {
        let (mon, area) = self.area()?;
        Some(geo_for(&mon, area, self.widths, fitted_factor(self.prefs.size, &mon)))
    }
}

/// The layout for a monitor, the area the islands are placed in, the islands' widths (dock units) and the dock's size factor. Every
/// length of the layout is a number of dock units, so the layout is given the unit (the monitor's scale times the size) where it used
/// to be given the monitor's scale: the one place the size enters.
fn geo_for(mon: &MonitorInfo, area: Rect, widths: Widths, size: f32) -> Geo {
    let scale = mon.scale();
    let unit = rf_win::dock::size::unit(scale, size);
    let islands = layout::islands(area, unit, widths);
    Geo { scale, unit, islands, placements: layout::placements(&islands, mon.rect, unit, layout::shadow_on()), growth_px: layout::growth_room(&islands, unit), area_width: area.width() }
}

/// The size factor the dock is drawn at on this monitor: the setting, cut to what the screen's width can hold (`size_live::fit_to_width`).
/// The ONE place the setting becomes a factor, so the layout, the band and the views cannot disagree about it.
fn fitted_factor(size: super::size_pref::DockSizePref, mon: &MonitorInfo) -> f32 {
    size_live::factor(size_live::fit_to_width(size.permille(), mon.work.width() as f32 / mon.scale()))
}

/// Whether a size slider is at work: the islands follow it but the deferred work (the band, the icons) has not been done yet.
pub fn size_drag_active(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.live.dragging())
}

fn host_mut(cx: &mut App) -> Option<&mut Host> {
    if cx.has_global::<Host>() { Some(cx.global_mut::<Host>()) } else { None }
}

/// The screen rectangle (device px) of an island's VISIBLE glass at rest: what a panel is put above and lined up with, and what a click
/// on the island's own button is told from a click outside. NOT the island's window: that is larger (a transparent margin, the middle
/// island's headroom and widening room) and, besides, keeps the system's invisible resize frame. The numbers are the layout's own
/// (`Placement::glass`), the very rectangle the glass is drawn in, so they cannot disagree with it and do not ask Windows anything.
pub fn visible_glass_rect(kind: Kind, cx: &App) -> Option<Rect> {
    cx.try_global::<Host>()?.geo.map(|g| g.glass(kind))
}

/// The primary monitor's scale and work area (device px): what a panel is kept inside.
pub fn monitor_work(cx: &App) -> Option<(f32, Rect)> {
    let (mon, _) = cx.try_global::<Host>()?.area()?;
    Some((mon.scale(), mon.work))
}

/// Device px per dock unit (the monitor's scale times the dock's size) as of the last layout: what the islands' lengths, and the island's
/// corner radius, are worked out with. `None` before a layout.
pub fn dock_unit(cx: &App) -> Option<f32> {
    cx.try_global::<Host>()?.geo.map(|g| g.unit)
}

/// What the left island needs to decide whether its weather has room for the condition line: the display's width (the work area) and
/// the right and the middle islands' widths, all in dock units. From the last layout the host worked out, so no question is asked of
/// Windows (the left island asks at every frame). `None` before the host has seen a monitor.
pub fn weather_room(cx: &App) -> Option<(f32, f32, f32)> {
    let host = cx.try_global::<Host>()?;
    let geo = host.geo?;
    Some((geo.area_width as f32 / geo.unit.max(0.01), host.widths.right, host.widths.middle))
}

/// An island's window: the toolkit's handle and the system's. For the tray strip, which takes the keyboard for the right island.
pub fn island_window(kind: Kind, cx: &App) -> Option<(AnyWindowHandle, isize)> {
    cx.try_global::<Host>()?.placed.iter().find(|p| p.kind == kind).map(|p| (p.handle, p.hwnd))
}

/// The right island's view (a panel that grows out of that island tells it to draw again when the growth starts and ends).
pub fn right_island(cx: &App) -> Option<Entity<RightIsland>> {
    cx.try_global::<Host>()?.right.clone()
}

/// Whether a full-screen application is in front (the islands are hidden).
pub fn is_fullscreen(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.fullscreen)
}

/// The base of every island view: the window-sized box, text colour, and - only while the glass is not running - the island's own
/// rounded translucent fill, so the dock is never invisible. The colours are the island's palette at this moment (see `tone.rs`);
/// `size` is the dock's size factor (the lengths here are dock units).
pub fn island_box(pal: Palette, fallback: bool, size: f32) -> Div {
    div()
        .size_full()
        .text_size(px(13. * size))
        .text_color(pal.text().hsla())
        .when(fallback, |d| d.bg(pal.fallback_fill().hsla()).rounded(px(RADIUS * size)))
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

/// The root of the left and the right island's window: the island's own view, inset by its margin, so that the view
/// sees a box exactly as big as the glass (which is how it was laid out when the window was as big as the glass). The middle island lays
/// itself out in the whole window.
struct Frame<V: Render + 'static> {
    inner: Entity<V>,
    kind: Kind,
    /// Inset the view by the margins (the left and the right island); the middle island lays itself out in the whole window.
    inset: bool,
}

impl<V: Render + 'static> Render for Frame<V> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [l, t, r, b] = if self.inset { insets(self.kind, cx).margins } else { [0.0; 4] };
        div().size_full().pl(px(l)).pt(px(t)).pr(px(r)).pb(px(b)).child(self.inner.clone())
    }
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

/// What the system is told about one island's window for a layout: where it is, and which part of it takes the pointer (`None`: the
/// island's own view decides, see [`request_layout`]).
struct Job {
    hwnd: isize,
    window: Rect,
    parts: Option<Vec<hit::Part>>,
}

/// The pieces of a window that take the pointer when nothing sticks out of the glass: the glass itself.
fn rest_parts(pl: &Placement, unit: f32) -> Vec<hit::Part> {
    hit::parts(pl.glass_in_window(), layout::px(RADIUS, unit) as f32, &[], pl.window.width(), pl.window.height())
}

thread_local! {
    /// The region the middle window has been given last (`None`: none yet, or it must be given again).
    static MIDDLE_REGION: RefCell<Option<Vec<hit::Part>>> = const { RefCell::new(None) };
}

/// Places the three windows by the current work area, scale and widths. The rectangles are worked out now, the windows are moved
/// from the message loop. Each side island's glass is told where it is in its window, and its window's region is set to its glass.
/// The middle island is different: its region and its glass follow the icons that stand out of the glass, which only its view knows
/// (a rest region here would cut off an icon that is grown at this moment), so it is only told to draw again, and the region it was
/// last given is forgotten so that the view's next frame sets it for the new window even if its pieces came out the same.
pub fn request_layout(cx: &mut App) {
    let Some(host) = host_mut(cx) else { return };
    let Some(geo) = host.compute_geo() else { return };
    host.geo = Some(geo);
    let jobs: Vec<Job> = host
        .placed
        .iter()
        .map(|p| {
            let pl = geo.placement(p.kind);
            let own = p.kind != Kind::Middle;
            if own {
                p.glass.borrow_mut().set_shape(pl.glass_in_window());
            }
            Job { hwnd: p.hwnd, window: pl.window, parts: own.then(|| rest_parts(&pl, geo.unit)) }
        })
        .collect();
    MIDDLE_REGION.with(|r| *r.borrow_mut() = None);
    on_message_loop(cx, move || {
        for j in jobs {
            island::place(j.hwnd, j.window);
            if let Some(parts) = &j.parts {
                hit::set_region(j.hwnd, parts);
            }
        }
    });
    // the views' margins follow, and the middle island sends its region and its glass for the new window
    cx.refresh_windows();
    // a panel grown out of an island works its glass out again for the island's new rectangle (nothing when none is grown)
    super::panel::layout_applied(cx);
}

/// The middle island's frame of the animation: where its glass is inside its window (device px) and which parts of the window take the
/// pointer. Called for every frame while the icons are magnified (and when the layout changes). Nothing is sent when neither differs
/// from what was sent last. What is sent goes in ONE task from the message loop, the region first and then the glass: the window's
/// region only ever lags the scene by the task, and never lets the system clip a glass or an icon that has just grown (the region is
/// only a pixel larger than they are).
pub fn set_middle_frame(glass: &Rc<RefCell<IslandGlass>>, shape: [f32; 4], parts: Vec<hit::Part>, cx: &App) {
    let (hwnd, shape_moved) = {
        let g = glass.borrow();
        (g.hwnd(), g.shape() != shape)
    };
    if hwnd == 0 {
        return;
    }
    let region_moved = MIDDLE_REGION.with(|r| region_differs(r.borrow().as_deref(), &parts));
    if !region_moved && !shape_moved {
        return;
    }
    if region_moved {
        MIDDLE_REGION.with(|r| *r.borrow_mut() = Some(parts.clone()));
    }
    let glass = glass.clone();
    on_message_loop(cx, move || {
        if region_moved {
            hit::set_region(hwnd, &parts);
        }
        glass.borrow_mut().set_shape(shape);
    });
}

/// Whether the region the window was last given (`None`: none, or forgotten because the window was placed again) is not the one these
/// `parts` make. A forgotten region always differs, even from the same pieces: it is the new window that has none.
fn region_differs(last: Option<&[hit::Part]>, parts: &[hit::Part]) -> bool {
    last != Some(parts)
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
    let size = cx.try_global::<Host>().map_or(1.0, |h| h.area().map_or_else(|| h.prefs.size.factor(), |(m, _)| fitted_factor(h.prefs.size, &m)));
    let env = Env::read_sized(scale, size);
    let changed = cx.try_global::<Env>().is_none_or(|old| *old != env);
    cx.set_global(env);
    if let Some(host) = cx.try_global::<Host>() {
        for p in &host.placed {
            // (the glass's corner radius is in device px: it follows the monitor's scale AND the dock's size)
            p.glass.borrow_mut().sync(env.dark, env.unit(), backdrop::now_ms());
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
        // the band is the dock's own height and margins: it follows the size too
        let unit = host.area().map_or(1.0, |(m, _)| rf_win::dock::size::unit(m.scale(), fitted_factor(host.prefs.size, &m)));
        system.reserve(layout::reserved_thickness(unit));
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
                // and so are the icons: they were asked for at the old scale's pixels (kept on show until the new arrive)
                if let Some(middle) = cx.try_global::<Host>().and_then(|h| h.middle.clone()) {
                    middle.update(cx, |v, cx| v.size_changed(cx));
                }
            }
            // the islands move; a panel that is open would be left where they were
            super::panel::close(rf_win::dock::panel::CloseCause::LayoutChanged, cx);
            // the tray strip's room is a quarter of the new work area
            super::tray_strip::layout_changed(cx);
            request_layout(cx);
            // the desktop's windows follow the monitors, once they have been quiet (see shell::cover)
            crate::shell::cover::layout_changed(cx);
            // the work area the host sets itself follows the band's size (only with RUSTYFILES_SET_WORKAREA=1, see shell::live)
            crate::shell::live::layout_changed(cx);
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
        SystemEvent::ShellChanged(mode) => crate::shell::live::on_shell_changed(mode, cx),
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
        let (stamp, fresh) = cx.background_executor().spawn(async { (file_stamp(), Settings::load_followed()) }).await;
        cx.update(|cx| {
            // this process replaces its settings with what it read: its baseline for saves moves to the same table (a slider's save must
            // not write back what another process set since)
            let fresh = fresh.map(|(settings, file)| {
                Settings::follow_file(file);
                settings
            });
            apply_reloaded(fresh, stamp, cx)
        });
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
    let mut fresh = fresh;
    // A size slider that is being dragged (or just was) is ahead of the file: the file may hold an earlier step of the same drag.
    let size_verdict = host_mut(cx).map(|h| h.live.file(Instant::now(), fresh.dock_size.permille()));
    if let Some(SizeFile::Keep { recheck }) = size_verdict
        && let Some(host) = cx.try_global::<Host>()
    {
        fresh.dock_size = host.prefs.size;
        if let Some(after) = recheck {
            // The file may hold the final value although the message that carried it was lost: look at it again once the hold is over.
            cx.spawn(async move |cx| {
                cx.background_executor().timer(after).await;
                cx.update(|cx| {
                    if let Some(host) = host_mut(cx) {
                        host.live.recheck_fired();
                    }
                    reload(cx);
                });
            })
            .detach();
        }
    }
    fresh.apply_side_effects();
    let new = Prefs::of(&fresh);
    cx.set_global(fresh);
    // the bell's dot follows its setting
    super::notif_state::sync(cx);
    let Some(host) = host_mut(cx) else { return };
    host.settings_stamp = stamp;
    let ch = changes(&host.prefs, &new);
    host.prefs = new.clone();
    let left = host.left.clone();
    if ch.size {
        // A size from the file: everything at once (what a drag does step by step and at its end).
        perform(size_live::FROM_FILE, cx);
    } else if size_verdict == Some(SizeFile::Take { settle: true }) {
        // the sizes agree but the end-of-drag work was never done (its timers were lost)
        perform(size_live::AT_REST, cx);
    }
    if ch.reserve && !ch.size {
        apply_reserve(cx);
    }
    if ch.city
        && let Some(left) = left
    {
        left.update(cx, |v, cx| v.set_weather_source(&new.city, new.location, cx));
    }
    // the desktop cover follows its own switch (see shell::cover)
    crate::shell::cover::on_settings(cx);
    // the keyboard layer follows its two switches (see shell::keylayer)
    crate::shell::keylayer::on_settings(cx);
    // the tray host follows its switch (see shell::trayhost)
    crate::shell::trayhost::on_settings(cx);
    // Explorer's taskbar follows its switch (see shell::taskbar_hide)
    crate::shell::taskbar_hide::on_settings(cx);
    cx.refresh_windows();
}

/// Does the pieces of work a [`Redo`] lists, in the order `Redo::steps` gives them (so what the dock does while a slider is dragged,
/// when it rests and for a size from the file is the table in `size_live`, not a copy of it). Reads the size from `Host::prefs`.
///
/// * `Environment`: the environment the views read (the unit) and the glass's corner radius.
/// * `ClosePanels`: a panel that is open would stay where the islands were.
/// * `TrayStrip`: the strip's room.
/// * `Windows`: the three windows' places and regions.
/// * `ReservedBand`: the band the dock reserves (changing it resizes other programs' windows).
/// * `Icons`: asked for again at the new pixel size; the old pictures stay on show until the new ones come (a size change must not
///   leave the dock soft, nor flash letters).
fn perform(redo: Redo, cx: &mut App) {
    for step in redo.steps() {
        match step {
            Step::Environment => {
                let scale = cx.try_global::<Env>().map_or(1.0, |e| e.scale);
                publish_env(scale, cx);
            }
            Step::ClosePanels => super::panel::close(rf_win::dock::panel::CloseCause::LayoutChanged, cx),
            Step::TrayStrip => super::tray_strip::layout_changed(cx),
            Step::Windows => request_layout(cx),
            Step::ReservedBand => apply_reserve(cx),
            Step::Icons => {
                if let Some(middle) = cx.try_global::<Host>().and_then(|h| h.middle.clone()) {
                    middle.update(cx, |v, cx| v.size_changed(cx));
                }
            }
        }
    }
}

/// A size message of the Dock page's slider.
fn on_size_message(m: SizeMsg, cx: &mut App) {
    let out = match host_mut(cx) {
        Some(host) => host.live.message(Instant::now(), m),
        None => return,
    };
    run_size_out(out, cx);
}

/// Does what the pacing says and asks for the timer it wants.
fn run_size_out(out: size_live::Out, cx: &mut App) {
    for act in out.acts {
        if let Act::Layout(permille) = act
            && let Some(host) = host_mut(cx)
        {
            host.prefs.size = super::size_pref::DockSizePref::from_permille(permille);
        }
        perform(act.redo(), cx);
    }
    if let Some(wait) = out.wake {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            cx.update(|cx| {
                let next = match host_mut(cx) {
                    Some(host) => host.live.tick(Instant::now()),
                    None => return,
                };
                run_size_out(next, cx);
            });
        })
        .detach();
    }
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
    // the keyboard hook goes first of all: from here on Windows' own Start and Alt+Tab are back
    crate::shell::keylayer::stop(cx);
    // Explorer's taskbar comes back before anything else is torn down
    crate::shell::taskbar_hide::stop(cx);
    // the media-key window goes, and a work area the host set itself is put back
    crate::shell::live::stop(cx);
    // the tray host's window goes next: Explorer's tray window is the first again and programs are asked to add their icons again
    crate::shell::trayhost::stop(cx);
    // the desktop cover goes next: its windows close and Explorer's desktop is simply visible again
    crate::shell::cover::stop(cx);
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
    kind: Kind,
    framed: bool,
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
    // the view the caller asked for; for a framed island it is the frame's child, not the window's root
    let inner: Rc<RefCell<Option<Entity<V>>>> = Rc::new(RefCell::new(None));
    let slot = inner.clone();
    let handle = match cx.open_window(opts, move |window, cx| {
        let view = cx.new(|cx| build(window, cx));
        *slot.borrow_mut() = Some(view.clone());
        cx.new(|_| Frame { inner: view, kind, inset: framed })
    }) {
        Ok(h) => h,
        Err(e) => {
            log::error!("dock: cannot open an island: {e:#}");
            return None;
        }
    };
    let hwnd = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).ok()?;
    let entity = inner.borrow_mut().take()?;
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
        crate::shell::exits::log_exit(crate::shell::exits::Exit::NoMonitor, "");
        cx.quit();
        return;
    };
    let scale = monitor.scale();
    let size = fitted_factor(prefs.size, &monitor);
    let env = Env::read_sized(scale, size);
    cx.set_global(env);

    let lines = rf_win::dock::clock::system_lines();
    let widths = Widths { left: plan::left_width(None), middle: plan::middle_width_at_rest(2, 1), right: plan::right_width(&lines) };
    let geo = geo_for(&monitor, area, widths, size);
    let display = cx.primary_display().map(|d| d.id());

    let (left_glass, middle_glass, right_glass) = (Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())));
    let (city, locate) = (prefs.city.clone(), prefs.location);
    let panel_glass = left_glass.clone();
    let lg = left_glass.clone();
    let left = open_island(geo.placements.left.window, scale, display, Kind::Left, true, move |_, cx| LeftIsland::new(lg, &city, locate, cx), cx);
    let mg = middle_glass.clone();
    let middle = open_island(geo.placements.middle.window, scale, display, Kind::Middle, false, move |_, cx| MiddleIsland::new(mg, cx), cx);
    let rg = right_glass.clone();
    let panel_right_glass = right_glass.clone();
    let right_lines = lines.clone();
    let right = open_island(geo.placements.right.window, scale, display, Kind::Right, true, move |_, cx| RightIsland::new(rg, right_lines, cx), cx);

    let mut placed = Vec::new();
    let mut left_entity = None;
    let mut middle_entity = None;
    let mut right_entity = None;
    if let Some((handle, hwnd, entity)) = left {
        placed.push(Placed { kind: Kind::Left, handle, hwnd, glass: left_glass });
        left_entity = Some(entity);
    }
    if let Some((handle, hwnd, entity)) = middle {
        placed.push(Placed { kind: Kind::Middle, handle, hwnd, glass: middle_glass });
        middle_entity = Some(entity);
    }
    if let Some((handle, hwnd, entity)) = right {
        placed.push(Placed { kind: Kind::Right, handle, hwnd, glass: right_glass });
        right_entity = Some(entity);
    }
    if placed.is_empty() {
        log::error!("dock: no island could be opened");
        crate::shell::exits::log_exit(crate::shell::exits::Exit::NoIsland, "");
        drop(system);
        cx.quit();
        return;
    }

    let panel_left = left_entity.clone();
    let sys_right = right_entity.clone();
    let tray_right = right_entity.clone();
    let fullscreen = system.as_ref().is_some_and(|s| s.fullscreen());
    let host = Host {
        system,
        placed,
        left: left_entity,
        middle: middle_entity,
        right: right_entity,
        widths,
        geo: Some(geo),
        fullscreen,
        prefs: prefs.clone(),
        settings_stamp: file_stamp(),
        started: Instant::now(),
        live: SizeLive::new(prefs.size.permille()),
    };
    // the first styling, placing, region and glass, from the message loop (the windows exist now but are still being set up). Each glass
    // is told where it is in its window now, before it starts: it never covers the whole window, not even for a frame.
    let setup: Vec<(Job, Rc<RefCell<IslandGlass>>)> = host
        .placed
        .iter()
        .map(|p| {
            let pl = geo.placement(p.kind);
            p.glass.borrow_mut().set_shape(pl.glass_in_window());
            (Job { hwnd: p.hwnd, window: pl.window, parts: Some(rest_parts(&pl, geo.unit)) }, p.glass.clone())
        })
        .collect();
    cx.set_global(host);
    // the views were drawn once with the margins assumed before the layout was known
    cx.refresh_windows();
    // The panels (Start and search, the control centre, the calendar, the notifications): only the islands' glass is handed over now (for
    // the tone); each panel's window is made when it is first opened (`panel_more::toggle_made`), so a panel nobody opens costs nothing.
    // A panel is not an island, it is never in `Host::placed` and gets no shape and no region. The island's glass that is passed is only
    // read, for the tone.
    if let Some(left) = panel_left {
        super::panel::init(panel_glass, left, display, scale, geo.placements.left.glass, cx);
    }
    super::panel::init_control_centre(panel_right_glass, display, scale, geo.placements.right.glass, cx);
    // the clock's panel and the bell's panel (each only when its setting is on)
    super::panel::init_extra(cx);
    // what the right island's symbols follow all the time (sound, network)
    if let Some(right) = sys_right {
        // (the bell's dot: a count of the notifications, followed only with the setting on)
        super::notif_state::init(right.clone(), cx);
        super::sysstate::start(right, cx);
    }
    on_message_loop(cx, move || {
        for (job, _) in &setup {
            if let Err(e) = island::make_island(job.hwnd) {
                log::warn!("dock: island style: {e:#}");
            }
            island::place(job.hwnd, job.window);
            if let Some(parts) = &job.parts {
                hit::set_region(job.hwnd, parts);
            }
        }
        let env = Env::read_sized(scale, size);
        for (job, glass) in &setup {
            glass.borrow_mut().start(job.hwnd, env.dark, env.unit(), 0);
        }
    });
    if fullscreen {
        apply_visibility(cx);
    }
    apply_reserve(cx);
    // The other programs' tray icons (nothing at all without a tray source: see tray_source.rs). After the setup job above has been queued:
    // the strip asks for a wider right island at once, and that placement must come after the first one, which was worked out for the
    // island without a strip and would otherwise be the last word.
    if let Some(right) = tray_right {
        super::tray_strip::start(right, cx);
    }
    // the desktop cover, when its setting is on (see shell::cover)
    crate::shell::cover::on_settings(cx);
    // the keyboard layer, when one of its settings is on (see shell::keylayer)
    crate::shell::keylayer::on_settings(cx);
    // the tray host, when its setting is on (see shell::trayhost)
    crate::shell::trayhost::on_settings(cx);
    // Explorer's taskbar, when its setting is on (see shell::taskbar_hide)
    crate::shell::taskbar_hide::on_settings(cx);
    // the parts that depend on who owns the shell window (see shell::live)
    crate::shell::live::start(cx);

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
                        crate::shell::exits::log_exit(crate::shell::exits::Exit::StopMessage, "");
                        cx.update(shutdown);
                        break;
                    }
                    Command::Reload => cx.update(reload),
                    Command::Size(m) => cx.update(|cx| on_size_message(m, cx)),
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
                    crate::shell::cover::refresh_dock_windows(cx);
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
                    crate::shell::cover::refresh_dock_windows(cx);
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
            crate::shell::exits::log_exit(crate::shell::exits::Exit::IslandsClosed, "");
            shutdown(cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::{Changes, Geo, Kind, Prefs, Settings, changes, file_changed, region_differs};
    use rf_win::dock::hit::{self, Part};
    use rf_win::dock::layout::{self, Rect, Widths};
    use rf_win::dock::panel::{self as panel_geo, Side};
    use std::time::{Duration, UNIX_EPOCH};

    #[test]
    fn a_region_given_again_after_a_layout_is_sent_even_when_its_pieces_are_the_same() {
        let grown = hit::parts([44.0, 12.0, 600.0, 72.0], 24.0, &[[200.0, 4.0, 68.0, 68.0]], 700, 100);
        let rest = hit::parts([44.0, 12.0, 600.0, 72.0], 24.0, &[], 700, 100);
        assert!(matches!(grown.last(), Some(Part::Rect(r)) if *r == Rect::new(199, 3, 269, 73)));
        // nothing sent yet, or forgotten by a layout (the window was placed again): always send
        assert!(region_differs(None, &grown));
        // the same pieces as last time: nothing to send
        assert!(!region_differs(Some(&grown), &grown));
        // other pieces: send
        assert!(region_differs(Some(&grown), &rest));
        assert!(region_differs(Some(&rest), &grown));
        // an empty region is a region too: it is sent the first time
        assert!(region_differs(None, &[]));
        assert!(!region_differs(Some(&[]), &[]));
    }

    fn prefs(reserve: bool, city: &str) -> Prefs {
        Prefs { reserve, city: city.into(), location: false, size: Default::default() }
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
    fn the_switch_for_the_position_changes_the_weather_source_like_the_city_does() {
        let base = prefs(true, "Ankara");
        let located = Prefs { location: true, ..base.clone() };
        assert_eq!(changes(&base, &located), Changes { reserve: false, city: true, size: false });
        assert_eq!(changes(&located, &located), Changes { reserve: false, city: false, size: false });
        // off by default, and the settings carry it through
        assert!(!Prefs::of(&Settings::default()).location);
        assert!(Prefs::of(&Settings { dock_weather_location: true, ..Default::default() }).location);
    }

    #[test]
    fn only_what_changed_is_applied() {
        let base = prefs(true, "Ankara");
        assert_eq!(changes(&base, &base), Changes { reserve: false, city: false, size: false });
        assert_eq!(changes(&base, &prefs(false, "Ankara")), Changes { reserve: true, city: false, size: false });
        assert_eq!(changes(&base, &prefs(true, "İzmir")), Changes { reserve: false, city: true, size: false });
        assert_eq!(changes(&base, &prefs(false, "")), Changes { reserve: true, city: true, size: false });
        let bigger = Prefs { size: crate::dock::size_pref::DockSizePref::from_permille(1200), ..base.clone() };
        assert_eq!(changes(&base, &bigger), Changes { reserve: false, city: false, size: true });
    }

    /// What a window has before the first layout is the layout's own margins, times the dock's size factor.
    #[test]
    fn the_assumed_margins_follow_the_size() {
        use super::Insets;
        let m = Insets::assumed(Kind::Middle, 1.0);
        assert_eq!((m.margins, m.growth), ([44.0, 12.0, 44.0, 4.0], 40.0), "the default size is the dock as it was");
        assert_eq!(Insets::assumed(Kind::Left, 1.0).margins, [4.0; 4]);
        let big = Insets::assumed(Kind::Middle, 1.4);
        assert!((big.margins[0] - 61.6).abs() < 1e-4 && (big.margins[1] - 16.8).abs() < 1e-4 && (big.growth - 56.0).abs() < 1e-4, "{big:?}");
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

    /// The layout of a monitor of `monitor` px whose work area ends `taskbar` px above its bottom, as the host works it out.
    fn geo(monitor: Rect, work: Rect, scale: f32) -> Geo {
        let widths = Widths { left: 330.0, middle: 600.0, right: 280.0 };
        let islands = layout::islands(work, scale, widths);
        Geo { scale, unit: scale, islands, placements: layout::placements(&islands, monitor, scale, false), growth_px: layout::growth_room(&islands, scale), area_width: work.width() }
    }

    /// The Start panel (its real size, `START_SIZE`: 642 x 720) over the left island and the control centre (360 x 400) over the right one, placed from the glass.
    fn panels(g: &Geo, work: Rect) -> (Rect, Rect) {
        (
            panel_geo::place(g.glass(Kind::Left), crate::dock::panel::START_SIZE, g.scale, Side::Left, work),
            panel_geo::place(g.glass(Kind::Right), (360.0, 400.0), g.scale, Side::Right, work),
        )
    }

    /// The islands' windows are larger than their glass. A panel opens 8 px above the GLASS and lines up with its edge; placed from
    /// the window's rectangle it would sit 4 px too low and 4 px out to the side, over the invisible margin.
    #[test]
    fn a_panel_is_placed_from_the_glass_and_not_from_the_larger_window_at_100_percent() {
        let (monitor, work) = (Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1032));
        let g = geo(monitor, work, 1.0);
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 974, 342, 1022));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 970, 346, 1026), "the window is 4 px larger on every side");
        let (start, cc) = panels(&g, work);
        // 8 px above the glass's top edge (974), lined up with its left edge (12)
        assert_eq!(start, Rect::new(12, 246, 654, 966));
        assert_eq!(start.bottom, g.glass(Kind::Left).top - 8);
        // 8 px above the right glass (top 974), its right edge (1908)
        assert_eq!(g.glass(Kind::Right), Rect::new(1628, 974, 1908, 1022));
        assert_eq!(cc, Rect::new(1548, 566, 1908, 966));
        // from the windows' rectangles the numbers would be wrong
        let wrong = panel_geo::place(g.placement(Kind::Left).window, crate::dock::panel::START_SIZE, 1.0, Side::Left, work);
        assert_ne!(wrong, start);
        assert_eq!((wrong.left, wrong.bottom), (8, 962));
    }

    #[test]
    fn a_panel_is_placed_from_the_glass_and_not_from_the_larger_window_at_150_percent() {
        let (monitor, work) = (Rect::new(0, 0, 2880, 1620), Rect::new(0, 0, 2880, 1548));
        let g = geo(monitor, work, 1.5);
        assert_eq!(g.glass(Kind::Left), Rect::new(18, 1461, 513, 1533));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(12, 1455, 519, 1539), "the window is 6 px (4 logical) larger on every side");
        let (start, cc) = panels(&g, work);
        // 12 px (8 logical) above the glass, 963 x 1080 (642 x 720 logical)
        assert_eq!(start, Rect::new(18, 369, 981, 1449));
        assert_eq!(start.bottom, g.glass(Kind::Left).top - 12);
        assert_eq!(g.glass(Kind::Right), Rect::new(2442, 1461, 2862, 1533));
        assert_eq!(cc, Rect::new(2322, 849, 2862, 1449));
        assert_eq!(cc.right, g.glass(Kind::Right).right, "lined up with the glass's right edge");
        let wrong = panel_geo::place(g.placement(Kind::Right).window, (360.0, 400.0), 1.5, Side::Right, work);
        assert_ne!(wrong, cc);
    }

    /// A 2560 x 1440 monitor at 100 %: with the 48 px taskbar (work area 1392 high) and with an auto-hiding one (the work area is the
    /// whole monitor, so the glass floats 10 px above the screen's edge and the window's lower margin is 4 px, not more).
    #[test]
    fn a_panel_is_placed_from_the_glass_on_a_2560_by_1440_monitor() {
        let monitor = Rect::new(0, 0, 2560, 1440);
        let work = Rect::new(0, 0, 2560, 1392);
        let g = geo(monitor, work, 1.0);
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 1334, 342, 1382));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 1330, 346, 1386));
        let (start, cc) = panels(&g, work);
        // 8 px above the glass's top (1334), lined up with its left edge (12)
        assert_eq!(start, Rect::new(12, 606, 654, 1326));
        assert_eq!(g.glass(Kind::Right), Rect::new(2268, 1334, 2548, 1382));
        assert_eq!(cc, Rect::new(2188, 926, 2548, 1326), "lined up with the right glass's right edge");

        let g = geo(monitor, monitor, 1.0);
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 1382, 342, 1430));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 1378, 346, 1434));
        let (start, cc) = panels(&g, monitor);
        assert_eq!(start, Rect::new(12, 654, 654, 1374));
        assert_eq!(cc, Rect::new(2188, 974, 2548, 1374));
    }

    /// The click on an island's own button is not "outside" the panel: the glass is what is kept, and the rest of the window (a margin that
    /// the window region lets the pointer through) is another program's.
    #[test]
    fn a_click_on_the_glass_is_the_islands_own_and_one_on_its_margin_is_outside() {
        let (monitor, work) = (Rect::new(0, 0, 1920, 1080), Rect::new(0, 0, 1920, 1032));
        let g = geo(monitor, work, 1.0);
        let (start, _) = panels(&g, work);
        let keep = g.glass(Kind::Left);
        let outside = |x, y| panel_geo::outside_click(false, true, (x, y), start, keep);
        assert!(!outside(100, 1000), "on the logo or the field: the island's button toggles the panel itself");
        assert!(outside(9, 1000), "on the margin, outside the glass (the window is there, the glass is not)");
        assert!(outside(1500, 100), "elsewhere");
        assert!(!outside(300, 500), "on the panel");
    }
}
