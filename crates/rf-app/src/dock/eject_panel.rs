//! The safe-removal panel as a slot of `panel.rs`: the same machinery as the calendar, the bell's panel and the tray strip's overflow
//! (placed above the right island, one panel at a time, the glass, the closing rules). A child module of `panel.rs` (included there
//! with `#[path]`, as `tray_panel.rs` is) so that it can use the machinery's private parts. Its window is made when the panel is first
//! opened; nothing is made at the dock's start.

use std::rc::Rc;

use gpui_kit::*;

use super::{Hooks, PanelId, Side, Slot, host, host_global, open_hidden, panel};
use crate::dock::eject_view::EjectView;
use crate::dock::host::Kind;

/// The safe-removal panel.
pub const EJECT: PanelId = PanelId("eject");

struct EjectHooks {
    handle: WindowHandle<EjectView>,
    view: Entity<EjectView>,
}

impl Hooks for EjectHooks {
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
    if panel(cx).is_some_and(|p| p.slot(EJECT).is_some()) {
        return true;
    }
    let Some((scale, _)) = host::monitor_work(cx) else { return false };
    let Some(near) = host::visible_glass_rect(Kind::Right, cx) else { return false };
    let Some(island) = panel(cx).and_then(|p| p.right_island.clone()) else { return false };
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    let size = crate::dock::eject_view::size_for(crate::dock::eject_view::drives(cx).len());
    let Some(handle) = open_hidden(size, display, near, scale, move |window, cx| EjectView::new(isl, window, cx), cx) else { return false };
    let Ok(view) = handle.entity(cx) else { return false };
    let hwnd = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).unwrap_or(0);
    if hwnd == 0 {
        return false;
    }
    let window = handle.window_id();
    host_global(cx).slots.push(Slot { id: EJECT, kind: Kind::Right, side: Side::Right, hwnd, window, island, hooks: Rc::new(EjectHooks { handle, view }) });
    true
}

/// The removal button's click: opens the panel, closes it when it is the open one. (The button is the home dock's.)
pub fn toggle_eject(cx: &mut App) {
    super::more::toggle_made(EJECT, crate::dock::host::MonId::HOME, cx);
}
