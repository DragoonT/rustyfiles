//! The tray strip's overflow panel as a slot of `panel.rs`: the same machinery as the Start panel, the control centre, the calendar and the
//! notifications (same placement above the right island, closing rules, glass, one panel at a time). This file is a child module of
//! `panel.rs` (it is included from there with `#[path]`, as `panel_more.rs` is) so that it can use the machinery's private parts without
//! making them public. Its window is made when the panel is first opened (`panel_more::toggle_made` calls [`create`] through the hook in
//! `panel_more::create`); nothing is made at the dock's start.
//!
//! The panel is lined up with the island's right edge like its neighbours (the calendar, the bell's panel), not above the chevron.

use std::rc::Rc;

use gpui_kit::*;
use rf_win::dock::layout::Rect;

use super::{Hooks, PanelId, Side, Slot, host, host_global, open_hidden, panel};
use crate::dock::host::Kind;
use crate::dock::tray_overflow::OverflowView;

/// The overflow panel.
pub const OVERFLOW: PanelId = PanelId("tray-overflow");

struct OverflowHooks {
    handle: WindowHandle<OverflowView>,
    view: Entity<OverflowView>,
}

impl Hooks for OverflowHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_reopen(&self, _dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_reopen(window, cx));
    }
    fn redraw(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |_, window, _| window.refresh());
    }
    fn on_shown(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_shown(dark, window, cx));
    }
    fn on_close(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_close(window, cx));
    }
    fn on_hidden(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
    }
    fn size(&self, cx: &App) -> (f32, f32) {
        self.view.read(cx).size(cx)
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// Makes the panel's window, hidden, and its slot. True when it exists afterwards (made now or before).
pub(super) fn create(cx: &mut App) -> bool {
    if panel(cx).is_some_and(|p| p.slot(OVERFLOW).is_some()) {
        return true;
    }
    let Some((scale, _)) = host::monitor_work(cx) else { return false };
    let Some(near) = host::visible_glass_rect(Kind::Right, cx) else { return false };
    // (the tone of this panel is the right island's, as the calendar's and the bell's)
    let Some(island) = panel(cx).and_then(|p| p.right_island.clone()) else { return false };
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    // (the window starts at the size the panel has now: its icons' grid, or the empty panel's text; it is fitted again as they change)
    let size = crate::dock::tray_strip::overflow_size(cx);
    let Some(handle) = open_hidden(size, display, near, scale, move |window, cx| OverflowView::new(isl, window, cx), cx) else { return false };
    let Ok(view) = handle.entity(cx) else { return false };
    let hwnd = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).unwrap_or(0);
    if hwnd == 0 {
        return false;
    }
    let window = handle.window_id();
    host_global(cx).slots.push(Slot { id: OVERFLOW, kind: Kind::Right, side: Side::Right, hwnd, window, island, hooks: Rc::new(OverflowHooks { handle, view }) });
    true
}

/// The chevron's click: opens the overflow panel, closes it when it is the open one.
pub fn toggle_overflow(cx: &mut App) {
    // (the tray strip is the home dock's)
    super::more::toggle_made(OVERFLOW, crate::dock::host::MonId::HOME, cx);
}

/// [`toggle_overflow`] for the keyboard walk in the tray strip: the foreground is the right island then (the strip took the keyboard for
/// it), so the panel would remember the island as the window to give the keyboard back to when it is closed with Escape, and the
/// user's program would never get it back. `previous` is the window the strip took the keyboard from (0: none).
pub fn toggle_overflow_from_keyboard(previous: isize, cx: &mut App) {
    // (only an opening needs it; a closing goes back to what the panel remembered)
    if panel(cx).and_then(|p| p.open) != Some(OVERFLOW) {
        host_global(cx).previous_hint = Some((previous, std::time::Instant::now()));
    }
    toggle_overflow(cx);
}

/// Where the overflow panel's own content is on the screen (device px, the client area) while it is open; `None` while it is closed or
/// easing out. (When the panel has grown out of the island the window is the whole glass, with the island's row; the icons' cells are
/// counted from the content's rectangle.)
pub fn overflow_rect(cx: &App) -> Option<Rect> {
    let p = panel(cx)?;
    (p.open == Some(OVERFLOW)).then_some(p.content)
}
