//! A dock on every monitor: the rules, with no window in them (and the one read of the display list they work on).
//!
//! The dock process used to have one of everything: the primary monitor, its three islands, its band. With "Dock'u her ekranda göster"
//! on, every other monitor gets a dock of its own, and this file is what decides, without Windows in the way:
//!
//! * WHICH DOCKS THERE ARE ([`reconcile`]): one per monitor that is not the primary one, keyed by the monitor's DEVICE NAME
//!   (`\\.\DISPLAY2`), never by its handle (the handle changes whenever the display configuration is rebuilt, the name is what the user
//!   sees in Windows' display settings). The primary monitor's dock is the dock the process always had (the "home" dock) and is not in
//!   the list. One monitor, or the switch off, is an empty list: exactly the single dock of before.
//! * WHAT A DOCK HOLDS ([`content`]): the primary monitor's dock holds everything it always held. A secondary monitor's dock holds the
//!   same three islands with these differences, which are the product owner's choices: no weather on the left, no tray strip, network
//!   or volume on the right (they stay on the primary, as on Windows' secondary taskbars), and, on a monitor too narrow for all of
//!   it, things give way in this order: the search field shrinks to a button, then the bell goes, then the middle island's icons
//!   squeeze as they always did on a narrow screen (that last step is the layout's own, `layout::islands`).
//! * WHICH APPS WHERE ([`keep_window`], [`filter_groups`]): "every dock shows the pins and every running app" or "an app that is not
//!   pinned only on the dock of the monitor its window is on, and a pinned one's running mark only there".
//! * WHICH MONITOR A PANEL OPENS ON ([`panel_target`], [`panel_request`]): the monitor whose island was pressed; for the Windows
//!   key, the monitor that has the pointer; one panel at a time over all monitors.

use super::layout::{MARGIN_SIDE, MIN_GAP, Rect};
use super::system::MonitorInfo;

/// A monitor as the list of docks sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Display {
    /// The device name (`\\.\DISPLAY2`): what a dock is known by.
    pub name: String,
    pub info: MonitorInfo,
    /// The primary monitor (its dock is the home dock, which is never in the list of secondary docks).
    pub primary: bool,
}

/// Every monitor now, the primary one first and the others in reading order (left to right, then top to bottom), so that the order of
/// the docks does not depend on the order Windows happens to list them in. Empty when they cannot be listed.
pub fn displays() -> Vec<Display> {
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::core::BOOL;

    unsafe extern "system" fn each(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam is the address of the Vec on the caller's stack, alive during EnumDisplayMonitors.
        unsafe {
            let list = &mut *(lparam.0 as *mut Vec<Display>);
            let mut mi = MONITORINFOEXW::default();
            mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(hmon, &mut mi.monitorInfo as *mut _ as *mut _).as_bool() {
                let name = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
                let (r, w) = (mi.monitorInfo.rcMonitor, mi.monitorInfo.rcWork);
                let (mut dx, mut dy) = (0u32, 0u32);
                let dpi = if GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy).is_ok() && dx > 0 { dx } else { 96 };
                list.push(Display {
                    name,
                    info: MonitorInfo { handle: hmon.0 as isize, rect: Rect::new(r.left, r.top, r.right, r.bottom), work: Rect::new(w.left, w.top, w.right, w.bottom), dpi },
                    // MONITORINFOF_PRIMARY = 1
                    primary: mi.monitorInfo.dwFlags & 1 != 0,
                });
            }
        }
        BOOL(1)
    }
    let mut v: Vec<Display> = Vec::new();
    // per-monitor aware for the call, so that the rectangles are device px whatever the process was started as
    let _aware = super::system::aware_thread();
    // SAFETY: `v` outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut v as *mut Vec<Display> as isize));
    }
    order(&mut v);
    v
}

/// The monitor (its handle) a window is on. A minimised window is on the monitor it will be restored to (its rectangle while minimised
/// is far off the screen). `None` when the window is gone or no monitor can be told. Cheap (one or two window calls): the dock asks it
/// of every window of the task list, off the UI thread, and only when apps are shown where their windows are.
pub fn monitor_of_window(hwnd: isize) -> Option<isize> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTONULL, MonitorFromRect, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, IsIconic, IsWindow, WINDOWPLACEMENT};
    let h = HWND(hwnd as *mut _);
    let _aware = super::system::aware_thread();
    // SAFETY: plain queries about a window handle; a handle that is gone makes them fail, which is the `None` below.
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return None;
        }
        let mon = if IsIconic(h).as_bool() {
            let mut wp = WINDOWPLACEMENT { length: size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
            GetWindowPlacement(h, &mut wp).ok()?;
            MonitorFromRect(&wp.rcNormalPosition, MONITOR_DEFAULTTONEAREST)
        } else {
            MonitorFromWindow(h, MONITOR_DEFAULTTONULL)
        };
        (!mon.is_invalid()).then_some(mon.0 as isize)
    }
}

/// The primary monitor first, the others left to right and then top to bottom (ties by name).
pub fn order(list: &mut [Display]) {
    list.sort_by(|a, b| b.primary.cmp(&a.primary).then(a.info.rect.left.cmp(&b.info.rect.left)).then(a.info.rect.top.cmp(&b.info.rect.top)).then(a.name.cmp(&b.name)));
}

// ---------------------------------------------------------------- which docks there are

/// What the host does to its list of secondary docks to make it match the monitors.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    /// A dock is no longer wanted (the monitor went, became the primary one, or the switch is off): its windows are closed.
    Close(String),
    /// The dock stays and its monitor's rectangle, work area or scale is not what it was placed for: place it again.
    Refresh(String),
    /// A monitor without a dock: open one.
    Open(String),
}

/// Whether a list of displays can be acted on: the system always has a primary monitor, so a list without one is a failed read.
pub fn usable(displays: &[Display]) -> bool {
    displays.iter().any(|d| d.primary)
}

/// Whether the dock should look at the monitors on its 2 s tick: when there is a secondary dock, or the list shows a monitor that
/// would get one. With one monitor and nothing open no timer looks at anything (a monitor that is plugged in is told by the system's
/// display-change message, which is not a timer).
pub fn watch_ticks(switch_on: bool, have_secondary: bool, displays: &[Display]) -> bool {
    have_secondary || (switch_on && displays.len() > 1)
}

/// The secondary docks that should exist: every monitor that is not the primary one, in the order given, when the switch is on.
pub fn wanted(switch_on: bool, displays: &[Display]) -> Vec<&Display> {
    if !switch_on {
        return Vec::new();
    }
    displays.iter().filter(|d| !d.primary).collect()
}

/// The changes that bring `current` (the secondary docks there are, by device name, with the monitor each was last placed for) to the
/// monitors now. Closes come first (a monitor that left must not keep a window on the desktop while the next is built), then the
/// docks to place again, then the opens. A dock is matched by NAME: a monitor whose handle changed with the same name is the same dock
/// and is only placed again; a monitor that became the primary one loses its secondary dock (the home dock moves there, as it always
/// did when the primary monitor changed).
pub fn reconcile(current: &[(String, MonitorInfo)], displays: &[Display], switch_on: bool) -> Vec<Change> {
    // A list that cannot be trusted (empty, or with no primary monitor: the system could not list them for a moment, as in a display
    // wake, a remote session attaching or a mode switch) says nothing about which monitors are gone. Closing every secondary dock for it
    // and opening them again a tick later would rebuild their windows, task watches and icons for nothing: nothing is changed.
    if switch_on && !usable(displays) {
        return Vec::new();
    }
    let want = wanted(switch_on, displays);
    let mut out: Vec<Change> = current.iter().filter(|(n, _)| !want.iter().any(|d| &d.name == n)).map(|(n, _)| Change::Close(n.clone())).collect();
    for (name, seen) in current {
        if let Some(d) = want.iter().find(|d| &d.name == name)
            && d.info != *seen
        {
            out.push(Change::Refresh(name.clone()));
        }
    }
    out.extend(want.iter().filter(|d| !current.iter().any(|(n, _)| n == &d.name)).map(|d| Change::Open(d.name.clone())));
    out
}

// ---------------------------------------------------------------- the band of a secondary dock

/// How many bands told to a monitor are remembered (the one in force, the one before it and one more): the work area lags the word.
pub const TOLD_KEPT: usize = 3;

/// Remembers a band just told to a monitor's work area (device px), newest first, each value once. `told` is what was remembered.
pub fn remember_band(told: &mut Vec<i32>, band: i32) {
    told.retain(|t| *t != band);
    told.insert(0, band);
    told.truncate(TOLD_KEPT);
}

/// The area a secondary monitor's dock is placed in. The dock reserves a band of its own at the bottom of its monitor's work area while
/// the host holds Explorer's taskbar hidden (`rf_shellguard::tbwork`), so the work area ends a band above the monitor's bottom edge;
/// the islands stand on the monitor's bottom edge inside that band, not at the top of the work area. A work area that is the monitor
/// less a band the host told (any of `told`: the work area follows the word a moment later) is put back to the monitor's bottom; any
/// other work area (a taskbar's strip, another program's toolbar, the whole monitor) is the area as it is.
pub fn placement_area(rect: Rect, work: Rect, told: &[i32]) -> Rect {
    let below = rect.bottom - work.bottom;
    let sides = work.left == rect.left && work.right == rect.right && work.top == rect.top;
    if sides && below > 0 && told.iter().any(|t| (*t - below).abs() <= 1) { Rect::new(work.left, work.top, work.right, rect.bottom) } else { work }
}

// ---------------------------------------------------------------- what a dock holds

/// How the left island's search shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Search {
    /// The field, with the word "Ara".
    Field,
    /// A button of the logo's size with the search picture: the field gave way on a narrow monitor.
    Button,
}

/// The widths (dock units) a monitor's islands want, and the room it has. Dock units, like every length of the layout: the monitor's
/// work width over the monitor's scale times the dock's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wants {
    pub area: f32,
    /// The left island with the search field, and with the search button.
    pub left_field: f32,
    pub left_button: f32,
    /// The right island with the bell (the clock and the bell: a secondary monitor's right island holds nothing else).
    pub right_with_bell: f32,
    /// The middle island at rest.
    pub middle: f32,
}

/// What a dock holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Content {
    /// The primary monitor's dock: the weather, the tray strip, the network and volume buttons (and everything below) are its own.
    pub primary: bool,
    pub search: Search,
    pub bell: bool,
}

impl Content {
    /// The weather is on the primary monitor's left island only.
    pub fn weather(&self) -> bool {
        self.primary
    }

    /// The tray strip and the network and volume buttons (and the separator after them) are on the primary monitor's right island only.
    pub fn system_controls(&self) -> bool {
        self.primary
    }

    /// The dock of the primary monitor: all of it, whatever the width (the squeeze of a narrow screen is the middle island's, as before).
    pub const HOME: Content = Content { primary: true, search: Search::Field, bell: true };
}

/// What a monitor's dock holds for the room it has. The primary monitor's dock is [`Content::HOME`] at every width. A secondary
/// monitor's keeps everything while the three islands fit with the middle island at the width it wants; when they do not, the search
/// field becomes a button; when they still do not, the bell goes; after that the middle island squeezes (`layout::islands`).
pub fn content(primary: bool, w: &Wants) -> Content {
    if primary {
        return Content::HOME;
    }
    let others = 2.0 * MARGIN_SIDE + 2.0 * MIN_GAP;
    // The order of giving way is the field, then the bell, then the middle island: the middle island is not asked to squeeze while
    // either of the others can still give room, so it is measured at its full width.
    let need_middle = w.middle;
    let fits = |left: f32, right: f32| w.area - others - left - right >= need_middle;
    if fits(w.left_field, w.right_with_bell) {
        return Content { primary: false, search: Search::Field, bell: true };
    }
    if fits(w.left_button, w.right_with_bell) {
        return Content { primary: false, search: Search::Button, bell: true };
    }
    // the bell goes (the clock stays: it is the island)
    Content { primary: false, search: Search::Button, bell: false }
}

// ---------------------------------------------------------------- which apps where

/// Setting "Uygulamalar hangi ekranda görünsün".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum AppsWhere {
    /// Every dock shows the pins and every running app.
    #[default]
    Everywhere,
    /// The pins are on every dock; a running app that is not pinned only on the dock of the monitor its window is on, and a pinned
    /// app's running mark only there.
    WindowMonitor,
}

/// The rule and the monitor (its handle) a dock applies when it lists its apps. With only one dock (one monitor, or the switch off) the
/// rule is "everywhere" whatever the setting says and no window is asked about: the dock of before.
pub fn rule_for(docks: usize, setting: AppsWhere, monitor_handle: isize) -> (AppsWhere, isize) {
    if docks <= 1 { (AppsWhere::Everywhere, 0) } else { (setting, monitor_handle) }
}

/// The ids in `want` that `have` does not hold yet (the thin dock gives each new dock a state of its own; the ones that went are dropped
/// by the caller with `retain`), in the order of `want`.
pub fn new_ids<K: PartialEq + Clone>(have: &[K], want: &[K]) -> Vec<K> {
    want.iter().filter(|k| !have.contains(k)).cloned().collect()
}

/// Whether a window counts on the dock of the monitor `this` (a monitor handle). `window_monitor` is the monitor the window is on;
/// `None` (it could not be told: a window that went while it was asked about) counts everywhere, so an app never vanishes from every
/// dock on a failed question.
pub fn keep_window(mode: AppsWhere, window_monitor: Option<isize>, this: isize) -> bool {
    match mode {
        AppsWhere::Everywhere => true,
        AppsWhere::WindowMonitor => window_monitor.is_none_or(|m| m == this),
    }
}

/// The running apps (groups of windows) as one monitor's dock shows them: only the windows that count there, and no group left with
/// none. Merged with the pins this gives the rule above: a pin whose group is gone has no running mark, and an app that is not pinned
/// has no icon at all. With [`AppsWhere::Everywhere`] the groups are returned as they are (nothing is asked about any window).
pub fn filter_groups<T>(mode: AppsWhere, this: isize, groups: Vec<(String, Vec<T>)>, monitor_of: impl Fn(&T) -> Option<isize>) -> Vec<(String, Vec<T>)> {
    if mode == AppsWhere::Everywhere {
        return groups;
    }
    groups
        .into_iter()
        .filter_map(|(key, windows)| {
            let kept: Vec<T> = windows.into_iter().filter(|w| keep_window(mode, monitor_of(w), this)).collect();
            (!kept.is_empty()).then_some((key, kept))
        })
        .collect()
}

// ---------------------------------------------------------------- which monitor a panel opens on

/// The monitor a panel opens on: the one whose island was pressed (`pressed`); without one (the Windows key) the monitor that has the
/// pointer; when the pointer is on none of them (it is between two, or unreadable) the home monitor. `rects` are the docks' monitors'
/// whole rectangles.
pub fn panel_target<K: Clone + PartialEq>(pressed: Option<&K>, pointer: Option<(i32, i32)>, rects: &[(K, Rect)], home: &K) -> K {
    if let Some(k) = pressed
        && rects.iter().any(|(r, _)| r == k)
    {
        return k.clone();
    }
    if let Some((x, y)) = pointer
        && let Some((k, _)) = rects.iter().find(|(_, r)| r.contains(x, y))
    {
        return k.clone();
    }
    home.clone()
}

/// The monitor the Windows key's tap works on: the monitor Start is open on (so that a tap closes it wherever the pointer has gone),
/// else the one that has the pointer (Start opens there).
pub fn key_target<K>(start_open_on: Option<K>, pointer: K) -> K {
    start_open_on.unwrap_or(pointer)
}

/// What a request for a panel means with a panel perhaps open already (one panel at a time over all monitors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelAction {
    /// Nothing is open: open it. (Another panel being open is the ordinary hand-over, which the panel module already does.)
    Open,
    /// The same panel on the same monitor is open: close it (a press on the open panel's button).
    Close,
    /// The same panel is open on another monitor: it goes there, out of the island it grew from and into the one pressed.
    MoveTo,
}

/// `open` is the panel that is open and the monitor it is on; `want` the panel asked for and the monitor.
pub fn panel_request<P: PartialEq, K: PartialEq>(open: Option<(&P, &K)>, want: (&P, &K)) -> PanelAction {
    match open {
        Some((p, k)) if p == want.0 && k == want.1 => PanelAction::Close,
        Some((p, _)) if p == want.0 => PanelAction::MoveTo,
        _ => PanelAction::Open,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(handle: isize, rect: Rect, dpi: u32) -> MonitorInfo {
        MonitorInfo { handle, rect, work: rect, dpi }
    }

    fn primary() -> Display {
        Display { name: r"\\.\DISPLAY1".into(), info: info(1, Rect::new(0, 0, 2560, 1440), 96), primary: true }
    }

    fn portrait() -> Display {
        Display { name: r"\\.\DISPLAY2".into(), info: info(2, Rect::new(2560, -618, 4000, 1942), 96), primary: false }
    }

    fn third() -> Display {
        Display { name: r"\\.\DISPLAY3".into(), info: info(3, Rect::new(-1920, 0, 0, 1080), 144), primary: false }
    }

    // ---- which docks there are

    #[test]
    fn one_monitor_or_the_switch_off_has_no_secondary_dock() {
        assert!(wanted(true, &[primary()]).is_empty(), "one monitor: today's single dock");
        assert!(wanted(false, &[primary(), portrait(), third()]).is_empty(), "the switch off: today's single dock");
        assert_eq!(reconcile(&[], &[primary()], true), vec![]);
        assert_eq!(reconcile(&[], &[primary(), portrait()], false), vec![]);
    }

    #[test]
    fn every_other_monitor_gets_a_dock_and_the_primary_one_does_not_get_a_second() {
        let all = [primary(), portrait(), third()];
        let names: Vec<&str> = wanted(true, &all).iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, [r"\\.\DISPLAY2", r"\\.\DISPLAY3"]);
        assert_eq!(reconcile(&[], &all, true), vec![Change::Open(r"\\.\DISPLAY2".into()), Change::Open(r"\\.\DISPLAY3".into())]);
    }

    #[test]
    fn a_monitor_that_is_added_gets_a_dock_and_one_that_is_removed_loses_it() {
        let have = vec![(portrait().name, portrait().info)];
        // a third monitor is plugged in
        let now = [primary(), portrait(), third()];
        assert_eq!(reconcile(&have, &now, true), vec![Change::Open(r"\\.\DISPLAY3".into())]);
        // the second one is unplugged
        assert_eq!(reconcile(&have, &[primary()], true), vec![Change::Close(r"\\.\DISPLAY2".into())]);
        // closes come before opens: the monitor that left does not keep a window while the next is built
        let swapped = [primary(), third()];
        assert_eq!(reconcile(&have, &swapped, true), vec![Change::Close(r"\\.\DISPLAY2".into()), Change::Open(r"\\.\DISPLAY3".into())]);
    }

    #[test]
    fn the_switch_going_off_closes_every_secondary_dock() {
        let have = vec![(portrait().name, portrait().info), (third().name, third().info)];
        assert_eq!(reconcile(&have, &[primary(), portrait(), third()], false), vec![Change::Close(r"\\.\DISPLAY2".into()), Change::Close(r"\\.\DISPLAY3".into())]);
    }

    #[test]
    fn a_dock_is_known_by_the_monitors_name_and_not_by_its_handle() {
        let have = vec![(portrait().name, portrait().info)];
        // the display configuration was rebuilt: same monitor, new handle (and a new place): the same dock, placed again
        let mut moved = portrait();
        moved.info = info(77, Rect::new(2560, 0, 4000, 2560), 96);
        assert_eq!(reconcile(&have, &[primary(), moved], true), vec![Change::Refresh(r"\\.\DISPLAY2".into())]);
        // nothing changed: nothing to do
        assert_eq!(reconcile(&have, &[primary(), portrait()], true), vec![]);
        // a new scale is a change too
        let mut scaled = portrait();
        scaled.info.dpi = 144;
        assert_eq!(reconcile(&have, &[primary(), scaled], true), vec![Change::Refresh(r"\\.\DISPLAY2".into())]);
    }

    #[test]
    fn when_the_primary_monitor_changes_the_new_primary_loses_its_secondary_dock_and_the_old_one_gets_one() {
        let have = vec![(portrait().name, portrait().info)];
        let mut p2 = portrait();
        p2.primary = true;
        let mut p1 = primary();
        p1.primary = false;
        let mut now = [p1, p2];
        order(&mut now);
        assert!(now[0].primary, "the primary one is first");
        assert_eq!(reconcile(&have, &now, true), vec![Change::Close(r"\\.\DISPLAY2".into()), Change::Open(r"\\.\DISPLAY1".into())]);
    }

    #[test]
    fn the_docks_are_ordered_by_where_the_monitors_are_and_not_by_the_order_windows_lists_them() {
        let mut v = vec![third(), portrait(), primary()];
        order(&mut v);
        assert_eq!(v.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), [r"\\.\DISPLAY1", r"\\.\DISPLAY3", r"\\.\DISPLAY2"]);
    }

    #[test]
    fn this_pcs_monitors_are_listed_when_there_is_a_display() {
        // read-only: the suite needs a desktop session anyway (system.rs makes the same assumption)
        let list = displays();
        assert!(!list.is_empty());
        assert_eq!(list.iter().filter(|d| d.primary).count(), 1, "exactly one primary monitor");
        assert!(list[0].primary);
        assert!(list.iter().all(|d| !d.name.is_empty() && d.info.rect.width() > 0 && d.info.scale() > 0.5));
        let names: std::collections::HashSet<_> = list.iter().map(|d| &d.name).collect();
        assert_eq!(names.len(), list.len(), "device names are unique");
    }

    // ---- what a dock holds

    fn wants(area: f32) -> Wants {
        // the numbers plan.rs gives: a left island of 8 + 32 + 8 + 220 + 8, a right one of 8 + clock + 4 + 32 + 8
        Wants { area, left_field: 276.0, left_button: 88.0, right_with_bell: 150.0, middle: 560.0 }
    }

    #[test]
    fn the_primary_docks_content_is_everything_at_every_width() {
        for area in [200.0, 800.0, 1366.0, 2560.0] {
            assert_eq!(content(true, &wants(area)), Content::HOME, "{area}");
        }
        assert!(Content::HOME.weather() && Content::HOME.system_controls());
    }

    #[test]
    fn a_secondary_dock_never_has_the_weather_the_tray_the_network_or_the_volume() {
        for area in [400.0, 1440.0, 2560.0] {
            let c = content(false, &wants(area));
            assert!(!c.weather() && !c.system_controls(), "{area}");
        }
    }

    #[test]
    fn a_wide_enough_secondary_monitor_keeps_the_field_and_the_bell() {
        // the user's portrait monitor: 1440 px at 100 %
        assert_eq!(content(false, &wants(1440.0)), Content { primary: false, search: Search::Field, bell: true });
    }

    #[test]
    fn on_a_narrow_monitor_the_search_field_gives_way_first_then_the_bell() {
        let others = 2.0 * MARGIN_SIDE + 2.0 * MIN_GAP;
        let w = wants(0.0);
        // with the field and the bell: 276 + 150 + everything the middle island wants
        let full = others + w.left_field + w.right_with_bell + w.middle;
        assert_eq!(content(false, &wants(full)).search, Search::Field);
        assert_eq!(content(false, &wants(full - 1.0)), Content { primary: false, search: Search::Button, bell: true }, "one unit short: the field becomes a button");
        let buttoned = others + w.left_button + w.right_with_bell + w.middle;
        assert_eq!(content(false, &wants(buttoned)), Content { primary: false, search: Search::Button, bell: true });
        assert_eq!(content(false, &wants(buttoned - 1.0)), Content { primary: false, search: Search::Button, bell: false }, "one more unit short: the bell goes");
        // and after that nothing else is taken away: the middle island squeezes (layout::islands)
        assert_eq!(content(false, &wants(100.0)), Content { primary: false, search: Search::Button, bell: false });
    }

    #[test]
    fn the_middle_island_is_not_asked_to_squeeze_while_the_field_or_the_bell_can_still_give_room() {
        // a crowded dock (30 icons want 1620) on the user's 1440 monitor: the field and then the bell go before any icon is squeezed
        let mut w = wants(1440.0);
        w.middle = 1620.0;
        assert_eq!(content(false, &w), Content { primary: false, search: Search::Button, bell: false });
        // room for the middle island's width and the button and the bell, but not the field: the field is the first to go
        w.middle = 1440.0 - (2.0 * MARGIN_SIDE + 2.0 * MIN_GAP) - w.left_button - w.right_with_bell;
        assert_eq!(content(false, &w), Content { primary: false, search: Search::Button, bell: true });
        // a dock of few icons keeps everything
        w.middle = 190.0;
        assert_eq!(content(false, &w), Content { primary: false, search: Search::Field, bell: true });
    }

    // ---- the band of a secondary dock

    #[test]
    fn the_islands_of_a_secondary_dock_stand_on_the_monitors_bottom_inside_the_band_it_reserved() {
        let rect = Rect::new(-1440, -200, 0, 2359);
        // the monitor less the 80 px band the host told: the islands go back down into the band
        let work = Rect::new(-1440, -200, 0, 2279);
        assert_eq!(placement_area(rect, work, &[80]), rect);
        // the work area lags the word by a moment: the band before it is still recognised, so the dock does not jump
        assert_eq!(placement_area(rect, work, &[56, 80]), rect);
        assert_eq!(placement_area(rect, Rect::new(-1440, -200, 0, 2303), &[56, 80]), rect, "the thin band (56 px) too");
        // no band told (the hide is not ours, or the dock reserves none): the area is the work area as it is
        assert_eq!(placement_area(rect, work, &[]), work);
        // a work area that no band of ours explains is not touched: Explorer's own 48 px strip, another program's toolbar, a side bar
        assert_eq!(placement_area(rect, Rect::new(-1440, -200, 0, 2311), &[80, 56]), Rect::new(-1440, -200, 0, 2311));
        assert_eq!(placement_area(rect, Rect::new(-1400, -200, 0, 2279), &[80]), Rect::new(-1400, -200, 0, 2279));
        // the whole monitor: nothing to put back
        assert_eq!(placement_area(rect, rect, &[80]), rect);
    }

    #[test]
    fn only_the_last_bands_told_are_remembered_and_each_once() {
        let mut told = Vec::new();
        for band in [80, 56, 80, 56, 80, 44, 92] {
            remember_band(&mut told, band);
        }
        assert_eq!(told, [92, 44, 80]);
        remember_band(&mut told, 44);
        assert_eq!(told, [44, 92, 80]);
    }

    // ---- the list of displays the host acts on

    #[test]
    fn a_list_that_failed_to_read_changes_nothing() {
        let have = vec![(portrait().name, portrait().info)];
        // empty (the system could not list the monitors for a moment) or with no primary monitor: a failed read, not a monitor that went
        assert!(!usable(&[]));
        assert_eq!(reconcile(&have, &[], true), vec![], "an empty list does not close the docks");
        assert_eq!(reconcile(&have, &[portrait()], true), vec![], "a list with no primary monitor does not either");
        // the switch going off still closes them (that list is empty on purpose)
        assert_eq!(reconcile(&have, &[], false), vec![Change::Close(portrait().name)]);
        assert!(usable(&[primary()]));
    }

    #[test]
    fn the_two_second_look_runs_only_when_there_is_a_second_monitor_to_look_at() {
        // one monitor, switch on (the default): no timer
        assert!(!watch_ticks(true, false, &[primary()]));
        // the switch off with several monitors, none open: no timer
        assert!(!watch_ticks(false, false, &[primary(), portrait()]));
        // a second monitor and the switch on: the look runs
        assert!(watch_ticks(true, false, &[primary(), portrait()]));
        // a secondary dock exists (even if the list is momentarily one monitor long): the look stays until it is closed
        assert!(watch_ticks(true, true, &[primary()]));
        assert!(watch_ticks(false, true, &[]));
    }

    #[test]
    fn a_series_of_plug_events_walks_the_list_of_docks_by_name() {
        // 1 monitor -> 2 -> 3 -> the middle one leaves -> the switch goes off
        let mut docks: Vec<(String, MonitorInfo)> = Vec::new();
        let apply = |docks: &mut Vec<(String, MonitorInfo)>, now: &[Display], switch: bool| {
            for c in reconcile(docks, now, switch) {
                match c {
                    Change::Close(n) => docks.retain(|(k, _)| *k != n),
                    Change::Refresh(n) => {
                        if let (Some(d), Some(slot)) = (now.iter().find(|d| d.name == n), docks.iter_mut().find(|(k, _)| *k == n)) {
                            slot.1 = d.info;
                        }
                    }
                    Change::Open(n) => {
                        if let Some(d) = now.iter().find(|d| d.name == n) {
                            docks.push((n, d.info));
                        }
                    }
                }
            }
            docks.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()
        };
        assert!(apply(&mut docks, &[primary()], true).is_empty(), "one monitor: today's dock");
        assert_eq!(apply(&mut docks, &[primary(), portrait()], true), [portrait().name]);
        assert_eq!(apply(&mut docks, &[primary(), portrait(), third()], true), [portrait().name, third().name]);
        assert_eq!(apply(&mut docks, &[primary(), third()], true), [third().name]);
        assert!(apply(&mut docks, &[primary(), third()], false).is_empty(), "the switch off: one dock again");
    }

    // ---- which apps where

    #[test]
    fn every_dock_shows_every_window_by_default() {
        assert_eq!(AppsWhere::default(), AppsWhere::Everywhere);
        for m in [Some(1), Some(2), None] {
            assert!(keep_window(AppsWhere::Everywhere, m, 1));
        }
        // nothing is asked about any window
        let groups = vec![("a".to_string(), vec![10, 11]), ("b".to_string(), vec![12])];
        let same = filter_groups(AppsWhere::Everywhere, 1, groups.clone(), |_| panic!("no window is asked about"));
        assert_eq!(same, groups);
    }

    #[test]
    fn one_dock_always_shows_everything_whatever_the_setting() {
        assert_eq!(rule_for(1, AppsWhere::WindowMonitor, 77), (AppsWhere::Everywhere, 0), "one monitor: nothing is asked about any window");
        assert_eq!(rule_for(0, AppsWhere::WindowMonitor, 77), (AppsWhere::Everywhere, 0));
        assert_eq!(rule_for(2, AppsWhere::WindowMonitor, 77), (AppsWhere::WindowMonitor, 77));
        assert_eq!(rule_for(2, AppsWhere::Everywhere, 77), (AppsWhere::Everywhere, 77));
    }

    #[test]
    fn a_dock_that_comes_gets_a_state_and_the_ones_that_stay_keep_theirs() {
        assert_eq!(new_ids(&[0u32], &[0, 1, 2]), [1, 2]);
        assert_eq!(new_ids(&[0u32, 1], &[0, 2]), [2]);
        assert!(new_ids(&[0u32, 1], &[0, 1]).is_empty());
        assert!(new_ids::<u32>(&[0], &[]).is_empty());
    }

    #[test]
    fn with_the_window_rule_an_app_is_where_its_window_is() {
        // windows 10 and 11 are on monitor 1, 12 and 13 on monitor 2
        let on = |w: &i32| Some(if *w < 12 { 1isize } else { 2 });
        let groups = vec![("browser".to_string(), vec![10, 12]), ("editor".to_string(), vec![11]), ("player".to_string(), vec![13])];
        let one = filter_groups(AppsWhere::WindowMonitor, 1, groups.clone(), on);
        assert_eq!(one, vec![("browser".to_string(), vec![10]), ("editor".to_string(), vec![11])], "an app with windows on both monitors is on both, with its own windows");
        let two = filter_groups(AppsWhere::WindowMonitor, 2, groups, on);
        assert_eq!(two, vec![("browser".to_string(), vec![12]), ("player".to_string(), vec![13])]);
    }

    #[test]
    fn a_window_that_moves_to_the_other_monitor_takes_its_app_with_it_at_the_next_list() {
        let groups = vec![("editor".to_string(), vec![11])];
        let before = filter_groups(AppsWhere::WindowMonitor, 1, groups.clone(), |_| Some(1));
        let after_here = filter_groups(AppsWhere::WindowMonitor, 1, groups.clone(), |_| Some(2));
        let after_there = filter_groups(AppsWhere::WindowMonitor, 2, groups, |_| Some(2));
        assert_eq!(before.len(), 1);
        assert!(after_here.is_empty(), "gone from the first monitor's dock");
        assert_eq!(after_there.len(), 1, "on the second monitor's dock");
    }

    #[test]
    fn a_window_whose_monitor_cannot_be_told_stays_on_every_dock() {
        let groups = vec![("a".to_string(), vec![1])];
        assert_eq!(filter_groups(AppsWhere::WindowMonitor, 1, groups.clone(), |_| None).len(), 1);
        assert_eq!(filter_groups(AppsWhere::WindowMonitor, 2, groups, |_| None).len(), 1);
    }

    // ---- which monitor a panel opens on

    const HOME: u32 = 0;

    fn rects() -> Vec<(u32, Rect)> {
        vec![(0, Rect::new(0, 0, 2560, 1440)), (1, Rect::new(2560, -618, 4000, 1942))]
    }

    #[test]
    fn a_panel_opens_on_the_monitor_whose_island_was_pressed() {
        assert_eq!(panel_target(Some(&1), Some((100, 100)), &rects(), &HOME), 1, "the island wins over the pointer");
        assert_eq!(panel_target(Some(&0), Some((3000, 100)), &rects(), &HOME), 0);
    }

    #[test]
    fn the_windows_key_opens_start_on_the_monitor_that_has_the_pointer() {
        assert_eq!(panel_target(None, Some((100, 900)), &rects(), &HOME), 0);
        assert_eq!(panel_target(None, Some((3000, -600)), &rects(), &HOME), 1);
        assert_eq!(panel_target(None, Some((2559, 5)), &rects(), &HOME), 0, "the edge pixel belongs to the left monitor");
        assert_eq!(panel_target(None, Some((2560, 5)), &rects(), &HOME), 1);
    }

    #[test]
    fn a_panel_with_no_pointer_or_a_pointer_on_no_monitor_or_an_island_that_is_gone_opens_on_the_home_monitor() {
        assert_eq!(panel_target(None, None, &rects(), &HOME), 0);
        assert_eq!(panel_target(None, Some((-5000, 0)), &rects(), &HOME), 0);
        assert_eq!(panel_target(Some(&9), Some((3000, 0)), &rects(), &HOME), 1, "an island that is gone is not pressed: the pointer decides");
        assert_eq!(panel_target(Some(&9), None, &rects(), &HOME), 0);
    }

    #[test]
    fn a_tap_of_the_windows_key_closes_start_wherever_the_pointer_is_and_opens_it_where_the_pointer_is() {
        // Start is open on monitor 0 and the pointer went to monitor 1: the tap works on monitor 0, so the request is a close
        let target = key_target(Some(0u32), 1u32);
        assert_eq!(target, 0);
        assert_eq!(panel_request(Some((&"start", &0u32)), (&"start", &target)), PanelAction::Close, "it closes, it does not slide to the other monitor");
        // nothing open: the tap opens it where the pointer is
        let target = key_target(None, 1u32);
        assert_eq!(panel_request::<&str, u32>(None, (&"start", &target)), PanelAction::Open);
        assert_eq!(target, 1);
    }

    #[test]
    fn one_panel_at_a_time_over_all_monitors() {
        // nothing open: open
        assert_eq!(panel_request::<&str, u32>(None, (&"start", &0)), PanelAction::Open);
        // the same panel's own button again: close
        assert_eq!(panel_request(Some((&"start", &0)), (&"start", &0)), PanelAction::Close);
        // the same panel from the other monitor's island: it moves
        assert_eq!(panel_request(Some((&"start", &0)), (&"start", &1)), PanelAction::MoveTo);
        // another panel, on either monitor: the ordinary hand-over (open; the open one closes)
        assert_eq!(panel_request(Some((&"start", &0)), (&"calendar", &0)), PanelAction::Open);
        assert_eq!(panel_request(Some((&"start", &0)), (&"calendar", &1)), PanelAction::Open);
    }
}
