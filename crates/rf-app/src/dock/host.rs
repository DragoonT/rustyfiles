//! The dock process's centre: the docks (one per monitor), their island windows, their places, the system events they react to (display,
//! DPI, theme, full-screen, time), the settings file, and the clean exit.
//!
//! ONE DOCK PER MONITOR. The host holds a list of [`Dock`]s: a value that is everything ONE monitor has (the monitor, its layout, its three
//! windows and their glass, its widths, its pointer region, what it holds, whether it is hidden for a full-screen program there). The first
//! one, [`MonId::HOME`], is the dock the process always had: it follows the primary monitor, takes its band and holds everything (the
//! weather, the tray strip, the network and volume buttons). The others are the secondary monitors' docks, found by the monitors'
//! DEVICE NAMES (`rf_win::dock::monitors`), opened and closed as monitors come and go, and only there while "Dock'u her ekranda göster"
//! is on. Everything else (the settings, the glass engine, the system watcher, the panels, the icon cache) is shared. With one monitor, or
//! the switch off, the list holds the home dock only and the code below does exactly what it did before the list existed.
//!
//! WHICH MONITOR A QUESTION IS ABOUT. An island's own view always knows its monitor ([`MonId`], asked with the `_on` functions). The
//! panels, the app menu and the previews are shared by all monitors (one at a time), so the functions they use (`visible_glass_rect`,
//! `monitor_work`, `island_window`, ...) answer for the ACTIVE monitor: the one whose island was pressed or hovered last, which an open
//! panel keeps until it is gone ([`set_active`]).
//!
//! THE WINDOWS ARE BIGGER THAN THE GLASS. Each island's window holds its glass plus a transparent margin (a few px; room for the shadow only with `RUSTYFILES_DOCK_SHADOW=1`; for the middle
//! island also headroom for the icons that grow out of the glass and room for the glass to widen). `layout::placements` works out
//! both rectangles; the host keeps the result (`Dock::geo`) for the views, tells the glass engine where the glass is inside its window
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
use rf_win::dock::hit;
use rf_win::dock::island;
use rf_win::dock::layout::{self, Islands, Placement, Placements, RADIUS, Rect, Widths};
use rf_win::dock::monitors::{self, Change, Content, Search, Wants};
use rf_win::dock::size_live::{self, Act, File as SizeFile, Live as SizeLive, Msg as SizeMsg, Redo, Step};
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

/// How often a full-screen program on a secondary monitor is looked for (the home dock's is the system watcher's, which looks as often).
const SECONDARY_FULLSCREEN_EVERY: Duration = Duration::from_millis(500);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Left,
    Middle,
    Right,
}

/// A dock (one monitor's three islands). Numbers are never reused while the process runs, so a view that outlives its dock by a moment
/// asks for a dock that is not there and gets the answer "none", never another monitor's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MonId(pub u32);

impl MonId {
    /// The dock the process always had: the primary monitor's.
    pub const HOME: MonId = MonId(0);

    pub fn is_home(self) -> bool {
        self == MonId::HOME
    }
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
    /// A dock on every monitor.
    pub all_monitors: bool,
    /// Which dock shows which running app.
    pub apps_where: crate::settings::DockAppsWhere,
}

impl Prefs {
    pub fn of(s: &Settings) -> Prefs {
        Prefs {
            reserve: s.dock_reserve,
            city: s.dock_weather_city.trim().to_string(),
            location: s.dock_weather_location,
            size: s.dock_size,
            all_monitors: s.dock_all_monitors,
            apps_where: s.dock_apps_where,
        }
    }
}

/// What differs between two readings of the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Changes {
    pub reserve: bool,
    /// The weather's source changed: the city or the switch for the PC's position.
    pub city: bool,
    pub size: bool,
    /// The switch for a dock on every monitor.
    pub monitors: bool,
    /// Which dock shows which app.
    pub apps: bool,
}

pub fn changes(old: &Prefs, new: &Prefs) -> Changes {
    Changes {
        reserve: old.reserve != new.reserve,
        city: old.city != new.city || old.location != new.location,
        size: old.size != new.size,
        monitors: old.all_monitors != new.all_monitors,
        apps: old.apps_where != new.apps_where,
    }
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
/// the middle island's glass may widen. Device px. Kept (see `Dock::geo`) so the views do not ask Windows for it on every frame.
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

/// The insets of an island window of the dock on this monitor, from the host's last layout (real logical px of the window; a view that
/// lays itself out in dock units divides them by the size factor).
pub fn insets_on(mon: MonId, kind: Kind, cx: &App) -> Insets {
    cx.try_global::<Host>().and_then(|h| h.dock(mon)).and_then(|d| d.geo).map_or_else(|| Insets::assumed(kind, size_on(mon, cx)), |g| Insets::of(&g, kind))
}

/// ONE MONITOR'S DOCK: the value the host keeps a list of. See the top of this file.
struct Dock {
    id: MonId,
    /// The device name of a secondary dock's monitor (`None` for the home dock, which follows the primary monitor whatever it is called).
    name: Option<String>,
    /// The monitor as this dock was last placed for. The home dock's is only kept for the rectangle lists; it asks the system watcher for
    /// its area (the band) every time.
    monitor: MonitorInfo,
    placed: Vec<Placed>,
    left: Option<Entity<LeftIsland>>,
    /// The middle island, told when the dock's size changes (its icons are asked for again at the new size).
    middle: Option<Entity<MiddleIsland>>,
    right: Option<Entity<RightIsland>>,
    /// The islands' logical widths (what their contents need).
    widths: Widths,
    /// The last layout (see [`Geo`]); `None` until a monitor has been seen.
    geo: Option<Geo>,
    /// What the islands hold at this width (see `rf_win::dock::monitors::content`).
    content: Content,
    /// A full-screen program is in front on this monitor: the islands are hidden.
    fullscreen: bool,
    /// The region the middle window has been given last (`None`: none yet, or it must be given again).
    region: Option<Vec<hit::Part>>,
    /// The bands (device px) told to this monitor's work area lately, newest first (`monitors::remember_band`); empty for the home dock,
    /// whose band is the desktop toolbar's.
    told_bands: Vec<i32>,
}

impl Dock {
    fn placed_of(&self, kind: Kind) -> Option<&Placed> {
        self.placed.iter().find(|p| p.kind == kind)
    }
}

pub struct Host {
    system: Option<DockSystem>,
    /// The docks: the home dock first, then the secondary monitors' in the order of `monitors::order`.
    docks: Vec<Dock>,
    /// The next number for a secondary dock.
    next_id: u32,
    /// The monitor the panels, the app menu and the previews work on (see the top of this file).
    active: MonId,
    /// The width of the clock block (dock units): the secondary right islands are made of it and the bell.
    clock_width: f32,
    prefs: Prefs,
    settings_stamp: Option<SystemTime>,
    started: Instant,
    /// The size the Dock page's slider gives while it is dragged (see `rf_win::dock::size_live`): the dock follows the slider's messages
    /// and keeps the settings file from overruling it until the drag is over.
    live: SizeLive,
    /// Looks for a full-screen program on the secondary monitors; there only while one exists.
    secondary_watch: Option<Task<()>>,
    /// The primary monitor as the last layout saw it: a layout event that comes from another monitor's change does not move the home
    /// dock, and a panel open on the home dock stays where it is for it.
    last_primary: Option<MonitorInfo>,
    /// Our hide of Explorer's taskbar is in force: the work areas of the monitors with a dock of ours hold the docks' bands.
    hide_in_force: bool,
}

impl Global for Host {}

impl Host {
    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn dock(&self, mon: MonId) -> Option<&Dock> {
        self.docks.iter().find(|d| d.id == mon)
    }

    fn dock_mut(&mut self, mon: MonId) -> Option<&mut Dock> {
        self.docks.iter_mut().find(|d| d.id == mon)
    }

    fn home(&self) -> &Dock {
        &self.docks[0]
    }

    /// The dock the panels work on (the home dock when it is gone).
    fn active_dock(&self) -> &Dock {
        self.dock(self.active).unwrap_or_else(|| self.home())
    }

    /// The monitor and the area a dock's islands are placed in. The home dock: the work area with our own toolbar band put back, so the
    /// islands do not climb the screen by the band's height. A secondary dock: its monitor's work area, with the band the host holds
    /// there (while Explorer's taskbar is hidden by us) put back (`monitors::placement_area`).
    fn area_of(&self, dock: &Dock) -> Option<(MonitorInfo, Rect)> {
        if dock.id.is_home() {
            self.system.as_ref().and_then(|s| s.placement_area()).or_else(|| rf_win::dock::primary_monitor().map(|m| (m, m.work)))
        } else {
            let told: &[i32] = if self.hide_in_force { &dock.told_bands } else { &[] };
            Some((dock.monitor, monitors::placement_area(dock.monitor.rect, dock.monitor.work, told)))
        }
    }

    /// The monitor and the area the HOME dock's islands are placed in.
    fn area(&self) -> Option<(MonitorInfo, Rect)> {
        self.area_of(self.home())
    }

    /// The layout for a dock's display as it is now.
    fn compute_geo(&self, dock: &Dock) -> Option<Geo> {
        let (mon, area) = self.area_of(dock)?;
        Some(geo_for(&mon, area, dock.widths, fitted_factor(self.prefs.size, &mon)))
    }

    /// The scale and the size factor a dock is drawn at.
    fn scale_size_of(&self, dock: &Dock) -> Option<(f32, f32)> {
        let (mon, _) = self.area_of(dock)?;
        Some((mon.scale(), fitted_factor(self.prefs.size, &mon)))
    }

    /// What a dock holds at its width now (the home dock: everything, at every width).
    fn content_of(&self, dock: &Dock) -> Content {
        if dock.id.is_home() {
            return Content::HOME;
        }
        let Some((mon, area)) = self.area_of(dock) else { return dock.content };
        let unit = rf_win::dock::size::unit(mon.scale(), fitted_factor(self.prefs.size, &mon)).max(0.01);
        monitors::content(
            false,
            &Wants {
                area: area.width() as f32 / unit,
                left_field: plan::left_width_secondary(Search::Field),
                left_button: plan::left_width_secondary(Search::Button),
                right_with_bell: plan::right_width_secondary(self.clock_width, true),
                middle: dock.widths.middle,
            },
        )
    }

    /// Whether there is more than the home dock (apps shown where their windows are only make sense then).
    fn has_secondary(&self) -> bool {
        self.docks.len() > 1
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

// ---------------------------------------------------------------- what the views and the panels ask

/// The screen rectangle (device px) of an island's VISIBLE glass at rest, on the ACTIVE monitor: what a panel is put above and lined up
/// with, and what a click on the island's own button is told from a click outside. NOT the island's window: that is larger (a transparent
/// margin, the middle island's headroom and widening room) and, besides, keeps the system's invisible resize frame. The numbers are the
/// layout's own (`Placement::glass`), the very rectangle the glass is drawn in, so they cannot disagree with it and do not ask Windows
/// anything.
pub fn visible_glass_rect(kind: Kind, cx: &App) -> Option<Rect> {
    let h = cx.try_global::<Host>()?;
    h.active_dock().geo.map(|g| g.glass(kind))
}

/// [`visible_glass_rect`] for the dock on this monitor.
pub fn visible_glass_rect_on(mon: MonId, kind: Kind, cx: &App) -> Option<Rect> {
    cx.try_global::<Host>()?.dock(mon)?.geo.map(|g| g.glass(kind))
}

/// The active monitor's scale and work area (device px): what a panel is kept inside.
pub fn monitor_work(cx: &App) -> Option<(f32, Rect)> {
    let h = cx.try_global::<Host>()?;
    let (mon, _) = h.area_of(h.active_dock())?;
    Some((mon.scale(), mon.work))
}

/// The home (primary) monitor's scale and work area: what the tray strip, which is the home dock's, works from.
pub fn home_work(cx: &App) -> Option<(f32, Rect)> {
    let (mon, _) = cx.try_global::<Host>()?.area()?;
    Some((mon.scale(), mon.work))
}

/// Device px per dock unit (the monitor's scale times the dock's size) as of the last layout, on the active monitor: what the islands'
/// lengths, and the island's corner radius, are worked out with. `None` before a layout.
pub fn dock_unit(cx: &App) -> Option<f32> {
    let h = cx.try_global::<Host>()?;
    h.active_dock().geo.map(|g| g.unit)
}

/// [`dock_unit`] for the dock on this monitor.
pub fn dock_unit_on(mon: MonId, cx: &App) -> Option<f32> {
    cx.try_global::<Host>()?.dock(mon)?.geo.map(|g| g.unit)
}

/// What the left island needs to decide whether its weather has room for the condition line: the display's width (the work area) and
/// the right and the middle islands' widths, all in dock units. From the last layout the host worked out, so no question is asked of
/// Windows (the left island asks at every frame). `None` before the host has seen a monitor. The weather is the home dock's.
pub fn weather_room(cx: &App) -> Option<(f32, f32, f32)> {
    let host = cx.try_global::<Host>()?;
    let home = host.home();
    let geo = home.geo?;
    Some((geo.area_width as f32 / geo.unit.max(0.01), home.widths.right, home.widths.middle))
}

/// An island's window on the home monitor: for the tray strip, which takes the keyboard for the home dock's right island.
pub fn home_island_window(kind: Kind, cx: &App) -> Option<(AnyWindowHandle, isize)> {
    cx.try_global::<Host>()?.home().placed_of(kind).map(|p| (p.handle, p.hwnd))
}

/// An island's window on this monitor's dock: for the grown glass, which raises the island it grew from (its own monitor's, whatever
/// the pointer has made the active one since).
pub fn island_window_on(mon: MonId, kind: Kind, cx: &App) -> Option<(AnyWindowHandle, isize)> {
    cx.try_global::<Host>()?.dock(mon)?.placed_of(kind).map(|p| (p.handle, p.hwnd))
}

/// An island's glass on the active monitor (its tone is the panel's).
pub fn island_glass(kind: Kind, cx: &App) -> Option<Rc<RefCell<IslandGlass>>> {
    let h = cx.try_global::<Host>()?;
    h.active_dock().placed_of(kind).map(|p| p.glass.clone())
}

/// The right island's view of the dock on this monitor (a panel that grows out of it tells it to draw again when the growth starts and ends).
pub fn right_island_on(mon: MonId, cx: &App) -> Option<Entity<RightIsland>> {
    cx.try_global::<Host>()?.dock(mon)?.right.clone()
}

/// Every dock's right island (the bell's dot is on all of them).
pub fn right_islands(cx: &App) -> Vec<Entity<RightIsland>> {
    cx.try_global::<Host>().map(|h| h.docks.iter().filter_map(|d| d.right.clone()).collect()).unwrap_or_default()
}

/// Every dock's left island (what is typed in the Start panel is shown on the one whose panel it is, and kept on the others).
pub fn left_islands(cx: &App) -> Vec<Entity<LeftIsland>> {
    cx.try_global::<Host>().map(|h| h.docks.iter().filter_map(|d| d.left.clone()).collect()).unwrap_or_default()
}

/// The left island's view of the dock on this monitor.
pub fn left_island_on(mon: MonId, cx: &App) -> Option<Entity<LeftIsland>> {
    cx.try_global::<Host>()?.dock(mon)?.left.clone()
}

/// Whether this window is one of the islands' windows, on any monitor.
pub fn is_island_window(hwnd: isize, cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.docks.iter().any(|d| d.placed.iter().any(|p| p.hwnd == hwnd)))
}

/// The middle island's view of the dock on this monitor.
pub(super) fn middle_island_on(mon: MonId, cx: &App) -> Option<Entity<MiddleIsland>> {
    cx.try_global::<Host>()?.dock(mon)?.middle.clone()
}

/// Every dock's middle island (the thin dock, a new size and the settings tell them all).
pub(super) fn middle_islands(cx: &App) -> Vec<(MonId, Entity<MiddleIsland>)> {
    cx.try_global::<Host>().map(|h| h.docks.iter().filter_map(|d| d.middle.clone().map(|m| (d.id, m))).collect()).unwrap_or_default()
}

/// Whether a full-screen application is in front on the ACTIVE monitor (its islands are hidden): what the panels and the pop-ups ask.
pub fn is_fullscreen(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.active_dock().fullscreen)
}

/// Whether a full-screen application is in front on ANY monitor that has a dock: the keyboard layer stands down for it.
pub fn any_fullscreen(cx: &App) -> bool {
    cx.try_global::<Host>().is_some_and(|h| h.docks.iter().any(|d| d.fullscreen))
}

/// What the dock on this monitor holds.
pub fn content_on(mon: MonId, cx: &App) -> Content {
    cx.try_global::<Host>().and_then(|h| h.dock(mon)).map_or(Content::HOME, |d| d.content)
}

/// The dock's size factor on this monitor (1.0 before the dock has published its environment). The home dock's is the global `Env`'s.
pub fn size_on(mon: MonId, cx: &App) -> f32 {
    env_on(mon, cx).size
}

/// The environment of the dock on this monitor: the system's look (shared) with this monitor's scale and size. The home dock's is the
/// global one, bit for bit.
pub fn env_on(mon: MonId, cx: &App) -> Env {
    let env = cx.try_global::<Env>().copied().unwrap_or_else(|| Env::read_sized(1.0, 1.0));
    if mon.is_home() {
        return env;
    }
    let Some(h) = cx.try_global::<Host>() else { return env };
    match h.dock(mon).and_then(|d| h.scale_size_of(d)) {
        Some((scale, size)) => Env { scale: if scale > 0.0 { scale } else { 1.0 }, size: rf_win::dock::size::sane(size), ..env },
        None => env,
    }
}

/// The docks' monitors: the number of each dock and its monitor as it was last placed for. The home dock's is the primary monitor's.
pub fn dock_monitors(cx: &App) -> Vec<(MonId, MonitorInfo)> {
    let Some(h) = cx.try_global::<Host>() else { return Vec::new() };
    h.docks.iter().filter_map(|d| h.area_of(d).map(|(m, _)| (d.id, m))).collect()
}

/// What apps the dock on this monitor shows: the rule and the monitor (its handle) it is applied for. Everywhere (nothing is asked about
/// any window) when there is only one dock: a single monitor, or the switch off, is the dock of before.
pub fn apps_rule(mon: MonId, cx: &App) -> (monitors::AppsWhere, isize) {
    let Some(h) = cx.try_global::<Host>() else { return (monitors::AppsWhere::Everywhere, 0) };
    let handle = h.dock(mon).and_then(|d| h.area_of(d)).map_or(0, |(m, _)| m.handle);
    monitors::rule_for(h.docks.len(), h.prefs.apps_where.rule(), handle)
}

/// How many docks there are (1: today's single dock).
pub fn dock_count(cx: &App) -> usize {
    cx.try_global::<Host>().map_or(1, |h| h.docks.len())
}

/// The dock whose monitor the pointer is on (the home dock when it is on none).
pub fn monitor_at_pointer(cx: &App) -> MonId {
    let pointer = rf_win::dock::zoomwatch::pointer().map(|(x, y, _)| (x, y));
    let rects: Vec<(MonId, Rect)> = dock_monitors(cx).into_iter().map(|(id, m)| (id, m.rect)).collect();
    monitors::panel_target(None, pointer, &rects, &MonId::HOME)
}

/// The monitor the panels, the app menu and the previews work on now.
pub fn active(cx: &App) -> MonId {
    cx.try_global::<Host>().map_or(MonId::HOME, |h| h.active_dock().id)
}

/// An island of this monitor was pressed or hovered: the pop-ups work there from now on. Ignored while a panel is open (it keeps its
/// monitor until it is gone).
pub fn set_active(mon: MonId, cx: &mut App) {
    if super::panel::is_open(cx) {
        return;
    }
    force_active(mon, cx);
}

/// An island of this monitor wants a pop-up (a preview, a menu) placed on its own monitor: true when the pop-ups work there now. False
/// while a panel is open on another monitor's dock: everything the pop-ups ask (work area, glass, unit) would answer for that other
/// monitor, so the pop-up would be placed from the wrong monitor's numbers.
pub fn claim_active(mon: MonId, cx: &mut App) -> bool {
    set_active(mon, cx);
    active(cx) == mon
}

/// A panel is being opened on this monitor.
pub(super) fn force_active(mon: MonId, cx: &mut App) {
    if let Some(h) = host_mut(cx)
        && h.dock(mon).is_some()
    {
        h.active = mon;
    }
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
    mon: MonId,
    kind: Kind,
    /// Inset the view by the margins (the left and the right island); the middle island lays itself out in the whole window.
    inset: bool,
}

impl<V: Render + 'static> Render for Frame<V> {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let [l, t, r, b] = if self.inset { insets_on(self.mon, self.kind, cx).margins } else { [0.0; 4] };
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

/// An island view reports that its contents need another width (logical px): the islands are placed again.
pub fn set_width(mon: MonId, kind: Kind, width: f32, cx: &mut App) {
    let Some(host) = host_mut(cx) else { return };
    let Some(dock) = host.dock_mut(mon) else { return };
    let slot = match kind {
        Kind::Left => &mut dock.widths.left,
        Kind::Middle => &mut dock.widths.middle,
        Kind::Right => &mut dock.widths.right,
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

/// Places the windows of every dock by the current work area, scale and widths. The rectangles are worked out now, the windows are moved
/// from the message loop. Each side island's glass is told where it is in its window, and its window's region is set to its glass.
/// The middle island is different: its region and its glass follow the icons that stand out of the glass, which only its view knows
/// (a rest region here would cut off an icon that is grown at this moment), so it is only told to draw again, and the region it was
/// last given is forgotten so that the view's next frame sets it for the new window even if its pieces came out the same.
pub fn request_layout(cx: &mut App) {
    refresh_hide_flag(cx);
    let Some(host) = host_mut(cx) else { return };
    let mut jobs: Vec<Job> = Vec::new();
    let ids: Vec<MonId> = host.docks.iter().map(|d| d.id).collect();
    for id in ids {
        // (what the dock holds depends on the room it has, and the layout on what it holds: the content first)
        let content = host.dock(id).map(|d| host.content_of(d));
        let Some(dock) = host.dock(id) else { continue };
        let Some(geo) = host.compute_geo(dock) else { continue };
        if let Some(dock) = host.dock_mut(id) {
            if let Some(c) = content {
                dock.content = c;
            }
            dock.geo = Some(geo);
            for p in &dock.placed {
                let pl = geo.placement(p.kind);
                let own = p.kind != Kind::Middle;
                if own {
                    p.glass.borrow_mut().set_shape(pl.glass_in_window());
                }
                jobs.push(Job { hwnd: p.hwnd, window: pl.window, parts: own.then(|| rest_parts(&pl, geo.unit)) });
            }
            dock.region = None;
        }
    }
    on_message_loop(cx, move || {
        for j in jobs {
            island::place(j.hwnd, j.window);
            if let Some(parts) = &j.parts {
                hit::set_region(j.hwnd, parts);
            }
        }
    });
    // the views' margins follow, and the middle islands send their region and their glass for the new window
    cx.refresh_windows();
    // a panel grown out of an island works its glass out again for the island's new rectangle (nothing when none is grown)
    super::panel::layout_applied(cx);
    // where the thin dock's "near" is follows the rectangles
    super::thin::layout_applied(cx);
}

/// A middle island's frame of the animation: where its glass is inside its window (device px) and which parts of the window take the
/// pointer. Called for every frame while the icons are magnified (and when the layout changes). Nothing is sent when neither differs
/// from what was sent last. What is sent goes in ONE task from the message loop, the region first and then the glass: the window's
/// region only ever lags the scene by the task, and never lets the system clip a glass or an icon that has just grown (the region is
/// only a pixel larger than they are).
pub fn set_middle_frame(mon: MonId, glass: &Rc<RefCell<IslandGlass>>, shape: [f32; 4], parts: Vec<hit::Part>, cx: &mut App) {
    let (hwnd, shape_moved) = {
        let g = glass.borrow();
        (g.hwnd(), g.shape() != shape)
    };
    if hwnd == 0 {
        return;
    }
    let Some(dock) = host_mut(cx).and_then(|h| h.dock_mut(mon)) else { return };
    let region_moved = region_differs(dock.region.as_deref(), &parts);
    if !region_moved && !shape_moved {
        return;
    }
    if region_moved {
        dock.region = Some(parts.clone());
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

/// Shows or hides the islands of one dock (a full-screen application is in front on its monitor, or left it).
fn apply_visibility(mon: MonId, cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let Some(dock) = host.dock(mon) else { return };
    let visible = !dock.fullscreen;
    let hwnds: Vec<isize> = dock.placed.iter().map(|p| p.hwnd).collect();
    let panel_here = host.active_dock().id == mon;
    on_message_loop(cx, move || {
        for h in hwnds {
            island::set_visible(h, visible);
        }
    });
    // a panel above a hidden island would float on its own
    if !visible && panel_here {
        super::panel::close(rf_win::dock::panel::CloseCause::FullScreen, cx);
    }
}

/// Puts the (re)read system facts where the views and the glass can see them. `scale` is the home monitor's.
fn publish_env(scale: f32, cx: &mut App) {
    let size = cx.try_global::<Host>().map_or(1.0, |h| h.area().map_or_else(|| h.prefs.size.factor(), |(m, _)| fitted_factor(h.prefs.size, &m)));
    let env = Env::read_sized(scale, size);
    let changed = cx.try_global::<Env>().is_none_or(|old| *old != env);
    cx.set_global(env);
    if let Some(host) = cx.try_global::<Host>() {
        for d in &host.docks {
            // (the glass's corner radius is in device px: it follows the monitor's scale AND the dock's size)
            let unit = if d.id.is_home() { env.unit() } else { host.scale_size_of(d).map_or(env.unit(), |(s, z)| rf_win::dock::size::unit(s, z)) };
            for p in &d.placed {
                p.glass.borrow_mut().sync(env.dark, unit, backdrop::now_ms());
            }
        }
    }
    if changed {
        cx.refresh_windows();
    }
}

/// The band the home dock reserves is a desktop toolbar on the primary monitor (or, with Explorer's taskbar hidden by us, the primary
/// monitor's work area), and the bands of the docks on the other monitors follow it (`apply_secondary_bands`).
pub(super) fn apply_reserve(cx: &mut App) {
    apply_home_reserve(cx);
    apply_secondary_bands(cx);
}

/// Tells the host that holds Explorer's taskbar hidden the band each secondary dock needs: while the hide is in force the work area of
/// such a monitor is the monitor less its dock's band, restored like the primary's on every way out (`rf_shellguard::tbwork`). Without
/// the hide a secondary dock reserves nothing: the monitor's own taskbar, if it has one, reserves its strip, and the dock stands above it
/// inside the work area (it does not register a desktop toolbar on a second monitor; see docs/design/dock.md). With one monitor, or the
/// switch off, the list is empty and nothing is sent that was not sent before (a lock and a comparison).
pub(super) fn apply_secondary_bands(cx: &mut App) {
    refresh_hide_flag(cx);
    let list: Vec<(String, i32)> = {
        let Some(host) = cx.try_global::<Host>() else { return };
        if !host.prefs.reserve {
            Vec::new()
        } else {
            host.docks
                .iter()
                .filter_map(|d| {
                    let name = d.name.clone()?;
                    let (scale, size) = host.scale_size_of(d)?;
                    let unit = rf_win::dock::size::unit(scale, size);
                    Some((name, rf_win::dock::thin::reserved_thickness(unit, super::thin::band_is_thin_on(d.id, cx))))
                })
                .collect()
        }
    };
    if let Some(host) = host_mut(cx) {
        // (remembered: the work area follows the word a moment later, and the islands are placed from it)
        for d in &mut host.docks {
            if let Some(name) = &d.name {
                match list.iter().find(|(n, _)| n == name) {
                    Some((_, band)) => monitors::remember_band(&mut d.told_bands, *band),
                    None => d.told_bands.clear(),
                }
            }
        }
    }
    crate::shell::taskbar_hide::other_bands_changed(list, cx);
}

/// Looks at whether our hide of Explorer's taskbar is in force (the layout of a secondary dock depends on it).
fn refresh_hide_flag(cx: &mut App) {
    let force = crate::shell::taskbar_hide::in_force(cx);
    if let Some(host) = host_mut(cx) {
        host.hide_in_force = force;
    }
}

fn apply_home_reserve(cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let Some(system) = host.system.as_ref() else { return };
    let band = if host.prefs.reserve {
        // the band is the dock's own height and margins: it follows the size too
        let unit = host.area().map_or(1.0, |(m, _)| rf_win::dock::size::unit(m.scale(), fitted_factor(host.prefs.size, &m)));
        // (the thin band while a maximised window has been there for a while: see thin.rs)
        let thickness = rf_win::dock::thin::reserved_thickness(unit, super::thin::band_is_thin(cx));
        system.reserve(thickness);
        Some(thickness)
    } else {
        system.release();
        None
    };
    // with Explorer's taskbar hidden by us the host sets the work area itself, from this band (see shell::taskbar_hide)
    crate::shell::taskbar_hide::band_changed(band, cx);
}

fn on_system_event(ev: SystemEvent, cx: &mut App) {
    match ev {
        SystemEvent::LayoutChanged => {
            refresh_hide_flag(cx);
            let scale = cx.try_global::<Host>().and_then(|h| h.area()).map_or(1.0, |(m, _)| m.scale());
            let scale_changed = cx.try_global::<Env>().is_none_or(|e| (e.scale - scale).abs() > 0.001);
            publish_env(scale, cx);
            if scale_changed {
                // the reserved band is in device px
                apply_reserve(cx);
                // and so are the icons: they were asked for at the old scale's pixels (kept on show until the new arrive)
                if let Some(middle) = cx.try_global::<Host>().and_then(|h| h.home().middle.clone()) {
                    middle.update(cx, |v, cx| v.size_changed(cx));
                }
            }
            // the other monitors' docks: opened for a monitor that came, closed for one that went, placed again for one that changed
            let moved = sync_docks(cx);
            // the islands move; a panel that is open would be left where they were - the panel of a dock that moved, that is: a change of
            // another monitor does not move the island a panel on this monitor grew from
            let home_moved = host_mut(cx).is_none_or(|h| {
                let now = rf_win::dock::primary_monitor();
                std::mem::replace(&mut h.last_primary, now) != now
            });
            if panel_is_hit(active(cx), home_moved, &moved) {
                super::panel::close(rf_win::dock::panel::CloseCause::LayoutChanged, cx);
            }
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
            if let Some(host) = host_mut(cx)
                && let Some(d) = host.dock_mut(MonId::HOME)
            {
                d.fullscreen = on;
            }
            apply_visibility(MonId::HOME, cx);
        }
        SystemEvent::ShellChanged(mode) => crate::shell::live::on_shell_changed(mode, cx),
        SystemEvent::TimeChanged => {
            for right in right_islands(cx) {
                right.update(cx, |v, cx| v.refresh_clock(cx));
            }
        }
    }
}

// ---------------------------------------------------------------- the other monitors' docks

/// Brings the list of secondary docks in line with the monitors and the switch (`rf_win::dock::monitors::reconcile` decides what to do;
/// this does it). Closes first, so a monitor that went does not keep a window while the next is built.
///
/// Costs, by case: the switch off and no secondary dock: nothing (no list is read). The switch on (the default) and one monitor: one read
/// of the list for each display-change message of the system, and no timer. A second monitor: the 2 s look of the system window too.
///
/// Returns the docks whose monitor changed under them (placed again), so that a panel open on one of them is put away.
fn sync_docks(cx: &mut App) -> Vec<MonId> {
    let Some(host) = cx.try_global::<Host>() else { return Vec::new() };
    let switch = host.prefs.all_monitors;
    let have_any = host.has_secondary();
    if !switch && !have_any {
        // today's single dock: no list of monitors is read at all
        rf_win::dock::system::watch_all_monitors(false);
        return Vec::new();
    }
    let displays = if switch { monitors::displays() } else { Vec::new() };
    // (the 2 s look at the other monitors runs only while there is one to look at; a monitor that is plugged in is the system's message)
    rf_win::dock::system::watch_all_monitors(monitors::watch_ticks(switch, have_any, &displays));
    let current: Vec<(String, MonitorInfo)> = host.docks.iter().filter_map(|d| d.name.clone().map(|n| (n, d.monitor))).collect();
    let changes = monitors::reconcile(&current, &displays, switch);
    if changes.is_empty() {
        return Vec::new();
    }
    let mut moved = Vec::new();
    for change in changes {
        match change {
            Change::Close(name) => close_dock(&name, cx),
            Change::Refresh(name) => {
                let Some(d) = displays.iter().find(|d| d.name == name) else { continue };
                let mut dpi_changed = false;
                let mut middle = None;
                let mut id = None;
                if let Some(dock) = host_mut(cx).and_then(|h| h.docks.iter_mut().find(|k| k.name.as_deref() == Some(&name))) {
                    dpi_changed = dock.monitor.dpi != d.info.dpi;
                    dock.monitor = d.info;
                    middle = dock.middle.clone();
                    id = Some(dock.id);
                }
                if let Some(id) = id {
                    moved.push(id);
                    // (the glass's corner radius is in device px: it follows the monitor's new scale at once, not at the next theme event)
                    resync_glass_of(id, cx);
                }
                if dpi_changed && let Some(middle) = middle {
                    middle.update(cx, |v, cx| v.size_changed(cx));
                }
            }
            Change::Open(name) => {
                if let Some(d) = displays.iter().find(|d| d.name == name) {
                    open_dock(d, cx);
                }
            }
        }
    }
    sync_secondary_watch(cx);
    super::thin::docks_changed(cx);
    // the bands of the docks on the other monitors: one more, one fewer, or another scale
    apply_secondary_bands(cx);
    // the middle islands show the apps their monitor's rule asks for: the set of docks changed
    refresh_apps(cx);
    moved
}

/// Tells the glass of each island of one dock the dock unit it has now (the monitor's scale times the dock's size) and the tone.
fn resync_glass_of(id: MonId, cx: &mut App) {
    let dark = cx.try_global::<Env>().is_some_and(|e| e.dark);
    let Some(host) = cx.try_global::<Host>() else { return };
    let Some(dock) = host.dock(id) else { return };
    let Some((scale, size)) = host.scale_size_of(dock) else { return };
    let unit = rf_win::dock::size::unit(scale, size);
    for p in &dock.placed {
        p.glass.borrow_mut().sync(dark, unit, backdrop::now_ms());
    }
}

/// Whether a layout change puts away the panel that is open on `panel_on`: the home dock's panel when the primary monitor changed, any
/// other dock's when that dock's monitor did. (A change of one monitor does not move the islands of another.)
fn panel_is_hit(panel_on: MonId, home_moved: bool, moved: &[MonId]) -> bool {
    if panel_on.is_home() { home_moved } else { moved.contains(&panel_on) }
}

/// Opens the dock of a secondary monitor: its three windows, placed from the first layout, with the glass started.
fn open_dock(d: &monitors::Display, cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let id = MonId(host.next_id);
    let lines = rf_win::dock::clock::system_lines();
    let widths = Widths { left: plan::left_width_secondary(Search::Field), middle: plan::middle_width_at_rest(2, 1), right: plan::right_width_secondary(plan::clock_width(&lines), true) };
    let mut dock = Dock {
        id,
        name: Some(d.name.clone()),
        monitor: d.info,
        placed: Vec::new(),
        left: None,
        middle: None,
        right: None,
        widths,
        geo: None,
        content: Content { primary: false, search: Search::Field, bell: true },
        fullscreen: false,
        region: None,
        told_bands: Vec::new(),
    };
    dock.content = host.content_of(&dock);
    // (the islands are as wide as their content says: the search as a button on a narrow monitor, no bell when it has gone)
    dock.widths.left = plan::left_width_secondary(dock.content.search);
    dock.widths.right = plan::right_width_secondary(plan::clock_width(&lines), dock.content.bell);
    let Some(geo) = host.compute_geo(&dock) else { return };
    let (scale, size) = (d.info.scale(), fitted_factor(host.prefs.size, &d.info));
    dock.geo = Some(geo);
    if !make_islands(&mut dock, &geo, scale, &lines, "", false, cx) {
        log::error!("dock: no island could be opened on {}", d.name);
        close_windows(&mut dock, cx);
        return;
    }
    log::info!("dock: a dock on {} ({}x{} at {} %)", d.name, d.info.rect.width(), d.info.rect.height(), d.info.dpi * 100 / 96);
    let setup = setup_jobs(&dock, &geo);
    let unit = rf_win::dock::size::unit(scale, size);
    if let Some(h) = host_mut(cx) {
        h.next_id += 1;
        h.docks.push(dock);
    }
    let dark = cx.try_global::<Env>().is_some_and(|e| e.dark);
    on_message_loop(cx, move || start_windows(setup, dark, unit));
    cx.refresh_windows();
}

/// Closes the dock of a monitor that went (or that became the primary one, or whose switch went off): the dock leaves the list FIRST, then
/// its glass is taken off and its windows are closed, so nothing that runs in between finds a half-closed dock.
fn close_dock(name: &str, cx: &mut App) {
    let Some(host) = host_mut(cx) else { return };
    let Some(ix) = host.docks.iter().position(|d| d.name.as_deref() == Some(name)) else { return };
    if ix == 0 {
        return;
    }
    // a panel that stands above it goes with it - first, while the dock is still there for the panel to put its glass back on; a panel
    // on another monitor's dock is not this dock's business
    let id = host.docks[ix].id;
    if host.active == id {
        super::panel::close(rf_win::dock::panel::CloseCause::LayoutChanged, cx);
    }
    let Some(host) = host_mut(cx) else { return };
    let Some(ix) = host.docks.iter().position(|d| d.id == id) else { return };
    let mut dock = host.docks.remove(ix);
    if host.active == dock.id {
        host.active = MonId::HOME;
    }
    log::info!("dock: the dock on {name} is closed");
    close_windows(&mut dock, cx);
}

fn close_windows(dock: &mut Dock, cx: &mut App) {
    for p in dock.placed.drain(..) {
        p.glass.borrow_mut().release();
        let _ = p.handle.update(cx, |_, window, _| window.remove_window());
    }
}

/// The first styling, placing, region and glass of a dock's windows (the windows exist but are still being set up): one entry for each,
/// and the glass to start.
struct Setup {
    job: Job,
    glass: Rc<RefCell<IslandGlass>>,
}

fn setup_jobs(dock: &Dock, geo: &Geo) -> Vec<Setup> {
    dock.placed
        .iter()
        .map(|p| {
            let pl = geo.placement(p.kind);
            p.glass.borrow_mut().set_shape(pl.glass_in_window());
            Setup { job: Job { hwnd: p.hwnd, window: pl.window, parts: Some(rest_parts(&pl, geo.unit)) }, glass: p.glass.clone() }
        })
        .collect()
}

/// Styles, places and gives a region to the windows, then starts their glass. Called from the message loop.
fn start_windows(setup: Vec<Setup>, dark: bool, unit: f32) {
    for s in &setup {
        if let Err(e) = island::make_island(s.job.hwnd) {
            log::warn!("dock: island style: {e:#}");
        }
        island::place(s.job.hwnd, s.job.window);
        if let Some(parts) = &s.job.parts {
            hit::set_region(s.job.hwnd, parts);
        }
    }
    for s in &setup {
        s.glass.borrow_mut().start(s.job.hwnd, dark, unit, 0);
    }
}

/// Opens the three windows of a dock. The home dock gets the weather source; a secondary one has no weather. False when no island could
/// be opened.
fn make_islands(dock: &mut Dock, geo: &Geo, scale: f32, lines: &rf_win::dock::clock::ClockLines, city: &str, locate: bool, cx: &mut App) -> bool {
    let mon = dock.id;
    let content = dock.content;
    let display = cx.primary_display().map(|d| d.id());
    let (left_glass, middle_glass, right_glass) = (Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())), Rc::new(RefCell::new(IslandGlass::new())));
    let (city, lg) = (city.to_string(), left_glass.clone());
    let left = open_island(geo.placements.left.window, scale, display, mon, Kind::Left, true, move |_, cx| LeftIsland::new(lg, mon, &city, locate, content, cx), cx);
    let mg = middle_glass.clone();
    let middle = open_island(geo.placements.middle.window, scale, display, mon, Kind::Middle, false, move |_, cx| MiddleIsland::new(mg, mon, cx), cx);
    let (rg, right_lines) = (right_glass.clone(), lines.clone());
    let right = open_island(geo.placements.right.window, scale, display, mon, Kind::Right, true, move |_, cx| RightIsland::new(rg, mon, right_lines, content, cx), cx);
    if let Some((handle, hwnd, entity)) = left {
        dock.placed.push(Placed { kind: Kind::Left, handle, hwnd, glass: left_glass });
        dock.left = Some(entity);
    }
    if let Some((handle, hwnd, entity)) = middle {
        dock.placed.push(Placed { kind: Kind::Middle, handle, hwnd, glass: middle_glass });
        dock.middle = Some(entity);
    }
    if let Some((handle, hwnd, entity)) = right {
        dock.placed.push(Placed { kind: Kind::Right, handle, hwnd, glass: right_glass });
        dock.right = Some(entity);
    }
    !dock.placed.is_empty()
}

/// The middle islands show the apps their monitor's rule asks for: look at the windows again (after the rule, or the set of docks,
/// changed).
fn refresh_apps(cx: &mut App) {
    for (_, middle) in middle_islands(cx) {
        middle.update(cx, |v, cx| v.refresh(cx));
    }
}

/// A full-screen program on a secondary monitor hides that monitor's dock only. The home dock's is the system watcher's; this looks at the
/// others, twice a second, only while there is a secondary dock.
fn sync_secondary_watch(cx: &mut App) {
    let Some(host) = cx.try_global::<Host>() else { return };
    let want = host.has_secondary();
    if want == host.secondary_watch.is_some() {
        return;
    }
    if !want {
        if let Some(h) = host_mut(cx) {
            h.secondary_watch = None;
        }
        return;
    }
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(SECONDARY_FULLSCREEN_EVERY).await;
            let alive = cx.update(look_for_fullscreen);
            if !alive {
                break;
            }
        }
    });
    if let Some(h) = host_mut(cx) {
        h.secondary_watch = Some(task);
    }
}

/// One look at the window in front, for every secondary monitor. False: the dock is gone.
fn look_for_fullscreen(cx: &mut App) -> bool {
    let Some(host) = cx.try_global::<Host>() else { return false };
    if !host.has_secondary() {
        return true;
    }
    // (one read of the window in front serves every monitor)
    let fg = rf_win::dock::fullscreen::foreground();
    let verdicts: Vec<(MonId, bool)> = host.docks.iter().filter(|d| !d.id.is_home()).map(|d| (d.id, rf_win::dock::fullscreen::is_fullscreen(fg.as_ref(), d.monitor.rect))).collect();
    let mut changed = Vec::new();
    if let Some(h) = host_mut(cx) {
        for (id, on) in verdicts {
            if let Some(d) = h.dock_mut(id)
                && d.fullscreen != on
            {
                d.fullscreen = on;
                changed.push(id);
            }
        }
    }
    for id in changed {
        apply_visibility(id, cx);
    }
    true
}

// ---------------------------------------------------------------- the settings

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
    let left = host.home().left.clone();
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
    if ch.monitors {
        // the switch for a dock on every monitor: docks open or close now
        sync_docks(cx);
        request_layout(cx);
    } else if ch.apps {
        refresh_apps(cx);
    }
    // The parts follow the one dock switch and their own values (`parts.rs`). The same order as at start-up, and its mirror on the way
    // out: Explorer's taskbar is hidden LAST (after the tray window and the keyboard hook that make up for it exist) and given back
    // FIRST (before they are taken down). The switch itself going off ends the dock process (`shutdown`); this is for a part whose own
    // value changed while the dock runs, or several parts at once ("Hepsini aç").
    let taskbar_first = crate::shell::taskbar_hide::goes_first(cx);
    if taskbar_first {
        crate::shell::taskbar_hide::on_settings(cx);
    }
    crate::shell::cover::on_settings(cx);
    crate::shell::keylayer::on_settings(cx);
    crate::shell::trayhost::on_settings(cx);
    if !taskbar_first {
        crate::shell::taskbar_hide::on_settings(cx);
    }
    // the thin dock follows its switch (see thin.rs)
    super::thin::on_settings(cx);
    cx.refresh_windows();
}

/// Does the pieces of work a [`Redo`] lists, in the order `Redo::steps` gives them (so what the dock does while a slider is dragged,
/// when it rests and for a size from the file is the table in `size_live`, not a copy of it). Reads the size from `Host::prefs`.
///
/// * `Environment`: the environment the views read (the unit) and the glass's corner radius.
/// * `ClosePanels`: a panel that is open would stay where the islands were.
/// * `TrayStrip`: the strip's room.
/// * `Windows`: the windows' places and regions (every dock's).
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
                for (_, middle) in middle_islands(cx) {
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
    let mut host = cx.remove_global::<Host>();
    // the news the Settings page read about the parts goes with the dock
    super::parts_status::stop();
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
    super::thin::shutdown(cx);
    super::panel::shutdown(cx);
    rf_win::dock::system::watch_all_monitors(false);
    host.secondary_watch = None;
    for dock in &mut host.docks {
        close_windows(dock, cx);
    }
    // (dropping the system object unregisters the toolbar and ends its window; it must not be dropped from its own callback, and
    // this is not)
    drop(host);
    log::info!("dock: stopped");
    cx.quit();
}

/// Opens one island window at the rectangle (device px). The window is a transparent pop-up that is not activated when shown; its
/// style, its exact place and its glass follow from the message loop (see the module docs).
#[allow(clippy::too_many_arguments)]
fn open_island<V: Render + 'static>(
    rect: Rect,
    scale: f32,
    display: Option<DisplayId>,
    mon: MonId,
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
        cx.new(|_| Frame { inner: view, mon, kind, inset: framed })
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

    let mut home = Dock {
        id: MonId::HOME,
        name: None,
        monitor,
        placed: Vec::new(),
        left: None,
        middle: None,
        right: None,
        widths,
        geo: Some(geo),
        content: Content::HOME,
        fullscreen: false,
        region: None,
        told_bands: Vec::new(),
    };
    let (city, locate) = (prefs.city.clone(), prefs.location);
    if !make_islands(&mut home, &geo, scale, &lines, &city, locate, cx) {
        log::error!("dock: no island could be opened");
        crate::shell::exits::log_exit(crate::shell::exits::Exit::NoIsland, "");
        drop(system);
        cx.quit();
        return;
    }
    let panel_left = home.left.clone();
    let sys_right = home.right.clone();
    let tray_right = home.right.clone();
    let panel_glass = home.placed_of(Kind::Left).map(|p| p.glass.clone());
    let panel_right_glass = home.placed_of(Kind::Right).map(|p| p.glass.clone());
    let fullscreen = system.as_ref().is_some_and(|s| s.fullscreen());
    home.fullscreen = fullscreen;
    let host = Host {
        system,
        docks: vec![home],
        next_id: 1,
        active: MonId::HOME,
        clock_width: plan::clock_width(&lines),
        prefs: prefs.clone(),
        settings_stamp: file_stamp(),
        started: Instant::now(),
        live: SizeLive::new(prefs.size.permille()),
        secondary_watch: None,
        last_primary: rf_win::dock::primary_monitor(),
        hide_in_force: false,
    };
    // the first styling, placing, region and glass, from the message loop (the windows exist now but are still being set up). Each glass
    // is told where it is in its window now, before it starts: it never covers the whole window, not even for a frame.
    let setup = setup_jobs(host.home(), &geo);
    cx.set_global(host);
    // the views were drawn once with the margins assumed before the layout was known
    cx.refresh_windows();
    // The panels (Start and search, the control centre, the calendar, the notifications): only the islands' glass is handed over now (for
    // the tone); each panel's window is made when it is first opened (`panel_more::toggle_made`), so a panel nobody opens costs nothing.
    // A panel is not an island, it is never in a dock's `placed` and gets no shape and no region. The island's glass that is passed is only
    // read, for the tone.
    let display = cx.primary_display().map(|d| d.id());
    if let (Some(left), Some(glass)) = (panel_left, panel_glass) {
        super::panel::init(glass, left, display, scale, geo.placements.left.glass, cx);
    }
    if let Some(glass) = panel_right_glass {
        super::panel::init_control_centre(glass, display, scale, geo.placements.right.glass, cx);
    }
    // the clock's panel and the bell's panel (each only when its setting is on)
    super::panel::init_extra(cx);
    // what the right island's symbols follow all the time (sound, network)
    if let Some(right) = sys_right {
        // (the bell's dot: a count of the notifications, followed only with the setting on)
        super::notif_state::init(right.clone(), cx);
        super::sysstate::start(right, cx);
    }
    let dark = env.dark;
    let unit = env.unit();
    on_message_loop(cx, move || start_windows(setup, dark, unit));
    if fullscreen {
        apply_visibility(MonId::HOME, cx);
    }
    apply_reserve(cx);
    // the thin dock (the setting's switch; nothing runs when it is off)
    super::thin::start(cx);
    // The other programs' tray icons (nothing at all without a tray source: see tray_source.rs). After the setup job above has been queued:
    // the strip asks for a wider right island at once, and that placement must come after the first one, which was worked out for the
    // island without a strip and would otherwise be the last word.
    if let Some(right) = tray_right {
        super::tray_strip::start(right, cx);
    }
    // the parts the one switch brings (`parts.rs`): the desktop cover, the keyboard layer, the tray host, Explorer's taskbar. The tray
    // host and the keyboard layer come before the taskbar is hidden, so that no icon and no key is lost in between.
    crate::shell::cover::on_settings(cx);
    crate::shell::keylayer::on_settings(cx);
    crate::shell::trayhost::on_settings(cx);
    crate::shell::taskbar_hide::on_settings(cx);
    // what a part that could not start tells the Settings page
    super::parts_status::start(cx);
    // the parts that depend on who owns the shell window (see shell::live)
    crate::shell::live::start(cx);
    // the other monitors' docks (nothing at all with the switch off and no secondary dock: no list of monitors is even read)
    sync_docks(cx);

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
                for p in host.docks.iter().flat_map(|d| &d.placed) {
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
                for p in host.docks.iter().flat_map(|d| &d.placed) {
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
    use super::{Changes, Geo, Kind, MonId, Prefs, Settings, changes, file_changed, panel_is_hit, region_differs};
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
        Prefs { reserve, city: city.into(), location: false, size: Default::default(), all_monitors: true, apps_where: crate::settings::DockAppsWhere::All }
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
        assert_eq!(changes(&base, &located), Changes { reserve: false, city: true, size: false, monitors: false, apps: false });
        assert_eq!(changes(&located, &located), Changes { reserve: false, city: false, size: false, monitors: false, apps: false });
        // off by default, and the settings carry it through
        assert!(!Prefs::of(&Settings::default()).location);
        assert!(Prefs::of(&Settings { dock_weather_location: true, ..Default::default() }).location);
    }

    #[test]
    fn only_what_changed_is_applied() {
        let base = prefs(true, "Ankara");
        assert_eq!(changes(&base, &base), Changes { reserve: false, city: false, size: false, monitors: false, apps: false });
        assert_eq!(changes(&base, &prefs(false, "Ankara")), Changes { reserve: true, city: false, size: false, monitors: false, apps: false });
        assert_eq!(changes(&base, &prefs(true, "İzmir")), Changes { reserve: false, city: true, size: false, monitors: false, apps: false });
        assert_eq!(changes(&base, &prefs(false, "")), Changes { reserve: true, city: true, size: false, monitors: false, apps: false });
        let bigger = Prefs { size: crate::dock::size_pref::DockSizePref::from_permille(1200), ..base.clone() };
        assert_eq!(changes(&base, &bigger), Changes { reserve: false, city: false, size: true, monitors: false, apps: false });
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
    fn a_dock_on_every_monitor_is_on_by_default_and_the_apps_are_on_every_dock() {
        let d = Prefs::of(&Settings::default());
        assert!(d.all_monitors, "the user asked for it: on by default");
        assert_eq!(d.apps_where, crate::settings::DockAppsWhere::All);
        assert_eq!(d.apps_where.rule(), rf_win::dock::monitors::AppsWhere::Everywhere);
        assert_eq!(crate::settings::DockAppsWhere::Window.rule(), rf_win::dock::monitors::AppsWhere::WindowMonitor);
    }

    #[test]
    fn the_switch_and_the_apps_rule_are_changes_the_running_dock_acts_on() {
        let base = prefs(true, "Ankara");
        let off = Prefs { all_monitors: false, ..base.clone() };
        assert_eq!(changes(&base, &off), Changes { reserve: false, city: false, size: false, monitors: true, apps: false });
        let by_window = Prefs { apps_where: crate::settings::DockAppsWhere::Window, ..base.clone() };
        assert_eq!(changes(&base, &by_window), Changes { reserve: false, city: false, size: false, monitors: false, apps: true });
        assert_eq!(changes(&base, &base), Changes { reserve: false, city: false, size: false, monitors: false, apps: false });
    }

    #[test]
    fn a_layout_change_puts_away_the_panel_of_the_dock_that_moved_and_no_other() {
        let second = MonId(1);
        let third = MonId(2);
        // a panel on the home dock: only a change of the primary monitor moves it
        assert!(panel_is_hit(MonId::HOME, true, &[]));
        assert!(!panel_is_hit(MonId::HOME, false, &[second]), "another monitor's change leaves the home dock's panel where it is");
        // a panel on a secondary dock: only a change of its own monitor
        assert!(panel_is_hit(second, false, &[second]));
        assert!(!panel_is_hit(second, true, &[third]), "the primary monitor's change leaves a panel on the second monitor");
        assert!(!panel_is_hit(second, false, &[]));
    }

    #[test]
    fn the_home_dock_is_number_zero_and_is_the_only_one_that_is() {
        use super::MonId;
        assert!(MonId::HOME.is_home());
        assert!(!MonId(1).is_home());
        assert_eq!(MonId::HOME, MonId(0));
    }

    /// A secondary dock's islands are laid out by the same pure functions from its monitor's rectangle: on the user's portrait monitor
    /// (1440 x 2560 at 100 %, to the right of the primary one and 618 px above its top edge) the three islands stand on its bottom and
    /// inside it, and no window reaches into the neighbour.
    #[test]
    fn a_secondary_monitors_islands_stand_on_its_own_bottom_inside_it() {
        use rf_win::dock::monitors::Search;
        let monitor = Rect::new(2560, -618, 4000, 1942);
        let work = monitor;
        let widths = Widths { left: crate::dock::plan::left_width_secondary(Search::Field), middle: 600.0, right: crate::dock::plan::right_width_secondary(80.0, true) };
        let g = geo(monitor, work, 1.0);
        let islands = layout::islands(work, 1.0, widths);
        assert_eq!(islands.left.bottom, 1942 - 4, "4 px above the monitor's bottom edge");
        assert_eq!(islands.left.left, 2560 + 12);
        assert_eq!(islands.right.right, 4000 - 12);
        let placements = layout::placements(&islands, monitor, 1.0, false);
        for p in [placements.left, placements.middle, placements.right] {
            assert!(monitor.covers(&p.window), "a window reaches into the next monitor: {p:?}");
            assert!(p.window.covers(&p.glass));
        }
        // and the primary monitor's layout is not the secondary's: they are worked out apart, each from its own rectangle
        let primary = Rect::new(0, 0, 2560, 1440);
        let home = geo(primary, primary, 1.0);
        assert_eq!(home.glass(Kind::Left).left, 12);
        assert_eq!(g.glass(Kind::Left).left, 2560 + 12);
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
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 980, 342, 1028));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 976, 346, 1032), "the window is 4 px larger on every side");
        let (start, cc) = panels(&g, work);
        // 8 px above the glass's top edge (980), lined up with its left edge (12)
        assert_eq!(start, Rect::new(12, 252, 654, 972));
        assert_eq!(start.bottom, g.glass(Kind::Left).top - 8);
        // 8 px above the right glass (top 980), its right edge (1908)
        assert_eq!(g.glass(Kind::Right), Rect::new(1628, 980, 1908, 1028));
        assert_eq!(cc, Rect::new(1548, 572, 1908, 972));
        // from the windows' rectangles the numbers would be wrong
        let wrong = panel_geo::place(g.placement(Kind::Left).window, crate::dock::panel::START_SIZE, 1.0, Side::Left, work);
        assert_ne!(wrong, start);
        assert_eq!((wrong.left, wrong.bottom), (8, 968));
    }

    #[test]
    fn a_panel_is_placed_from_the_glass_and_not_from_the_larger_window_at_150_percent() {
        let (monitor, work) = (Rect::new(0, 0, 2880, 1620), Rect::new(0, 0, 2880, 1548));
        let g = geo(monitor, work, 1.5);
        assert_eq!(g.glass(Kind::Left), Rect::new(18, 1470, 513, 1542));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(12, 1464, 519, 1548), "the window is 6 px (4 logical) larger on every side");
        let (start, cc) = panels(&g, work);
        // 12 px (8 logical) above the glass, 963 x 1080 (642 x 720 logical)
        assert_eq!(start, Rect::new(18, 378, 981, 1458));
        assert_eq!(start.bottom, g.glass(Kind::Left).top - 12);
        assert_eq!(g.glass(Kind::Right), Rect::new(2442, 1470, 2862, 1542));
        assert_eq!(cc, Rect::new(2322, 858, 2862, 1458));
        assert_eq!(cc.right, g.glass(Kind::Right).right, "lined up with the glass's right edge");
        let wrong = panel_geo::place(g.placement(Kind::Right).window, (360.0, 400.0), 1.5, Side::Right, work);
        assert_ne!(wrong, cc);
    }

    /// A 2560 x 1440 monitor at 100 %: with the 48 px taskbar (work area 1392 high) and with an auto-hiding one (the work area is the
    /// whole monitor, so the glass floats 4 px above the screen's edge and the window's lower margin is 4 px, not more).
    #[test]
    fn a_panel_is_placed_from_the_glass_on_a_2560_by_1440_monitor() {
        let monitor = Rect::new(0, 0, 2560, 1440);
        let work = Rect::new(0, 0, 2560, 1392);
        let g = geo(monitor, work, 1.0);
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 1340, 342, 1388));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 1336, 346, 1392));
        let (start, cc) = panels(&g, work);
        // 8 px above the glass's top (1340), lined up with its left edge (12)
        assert_eq!(start, Rect::new(12, 612, 654, 1332));
        assert_eq!(g.glass(Kind::Right), Rect::new(2268, 1340, 2548, 1388));
        assert_eq!(cc, Rect::new(2188, 932, 2548, 1332), "lined up with the right glass's right edge");

        let g = geo(monitor, monitor, 1.0);
        assert_eq!(g.glass(Kind::Left), Rect::new(12, 1388, 342, 1436));
        assert_eq!(g.placement(Kind::Left).window, Rect::new(8, 1384, 346, 1440));
        let (start, cc) = panels(&g, monitor);
        assert_eq!(start, Rect::new(12, 660, 654, 1380));
        assert_eq!(cc, Rect::new(2188, 980, 2548, 1380));
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
