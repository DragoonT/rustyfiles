//! The clock's panel (the calendar) and the bell's panel (the notifications) as slots of `panel.rs`: the same machinery as the Start
//! panel and the control centre (same placement above the right island, closing rules, glass, one panel at a time), only the two
//! views and their hooks are new. This file is a child module of `panel.rs` (it is included from there with `#[path]`) so that it
//! can use the machinery's private parts without making them public.
//!
//! WHEN THE WINDOWS ARE MADE. Every panel's window (the Start panel's, the control centre's, the calendar's and the notifications') is
//! made when the panel is FIRST OPENED, by [`toggle_made`]: the click that asks for it makes the window from a task of the foreground
//! executor (outside any update of the app, like every other call that touches a window), hidden, opens it, and keeps it for every later
//! opening. Nothing is made at the dock's start; a panel nobody opens costs nothing, and a setting switched on while the dock runs needs
//! no special case. The click's time is kept for the log line that says how long the user waited (`panel::open_panel`).

use std::cell::RefCell;
use std::rc::Rc;

use super::{Hooks, PanelId, Side, Slot, host, host_global, open_hidden, panel};
use std::time::Instant;
use crate::dock::backdrop::IslandGlass;
use crate::dock::cal_view::{self, CalView};
use crate::dock::host::Kind;
use crate::dock::notif_view::{self, NotifView};
use gpui_kit::*;

/// The clock's panel.
pub const CALENDAR: PanelId = PanelId("calendar");
/// The bell's panel.
pub const NOTIFICATIONS: PanelId = PanelId("notifications");

// ---------------------------------------------------------------- the calendar

struct CalHooks {
    handle: WindowHandle<CalView>,
}

impl Hooks for CalHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
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
    fn size(&self, _cx: &App) -> (f32, f32) {
        cal_view::size()
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

// ---------------------------------------------------------------- the notifications

struct NotifHooks {
    handle: WindowHandle<NotifView>,
    view: Entity<NotifView>,
}

impl Hooks for NotifHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_shown(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_shown(dark, window, cx));
    }
    fn on_close(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_close(window, cx));
    }
    fn on_hidden(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
        scrub(&self.handle, cx);
    }
    fn size(&self, cx: &App) -> (f32, f32) {
        (notif_view::WIDTH, self.view.read(cx).height(cx))
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// Draws the hidden notification window twice with nothing in it. The view has dropped the user's texts, but GPUI keeps the last drawn
/// frame's elements (in the window) and the shaped text of the last two frames (in the text system, keyed by the strings) until the
/// next draws, and a hidden window asks for none: so they would stay in memory for as long as the panel stays closed. Two empty
/// draws replace both. A failure (the window is busy, or gone) is ignored: the texts then go with the panel's next opening.
fn scrub(handle: &WindowHandle<NotifView>, cx: &mut App) {
    for _ in 0..2 {
        let _ = cx.update_window((*handle).into(),|_, window, cx| window.draw(cx).clear(cx));
    }
}

// ---------------------------------------------------------------- making them

fn has_slot(id: PanelId, cx: &App) -> bool {
    panel(cx).is_some_and(|p| p.slot(id).is_some())
}

/// Makes the window of a panel, hidden. False when it cannot be made now (no monitor yet, the island's glass is not there). Does
/// nothing, and says true, when the panel is already made.
fn create(id: PanelId, cx: &mut App) -> bool {
    if has_slot(id, cx) {
        return true;
    }
    if id == PanelId::START {
        return super::create_start(cx);
    }
    if id == PanelId::CONTROL_CENTRE {
        return super::create_control_centre(cx);
    }
    if id == super::tray_panel::OVERFLOW {
        return super::tray_panel::create(cx);
    }
    let Some((scale, _)) = host::monitor_work(cx) else { return false };
    let Some(near) = host::visible_glass_rect(Kind::Right, cx) else { return false };
    // (the tone of the clock's and the bell's panel is the right island's)
    let Some(island) = panel(cx).and_then(|p| p.right_island.clone()) else { return false };
    let display = cx.primary_display().map(|d| d.id());
    if id == CALENDAR {
        let isl = island.clone();
        let Some(handle) = open_hidden(cal_view::size(), display, near, scale, move |window, cx| CalView::new(isl, window, cx), cx) else { return false };
        return register(id, island, handle.window_id(), hwnd_of(&handle, cx), Rc::new(CalHooks { handle }), cx);
    }
    let isl = island.clone();
    let Some(handle) = open_hidden((notif_view::WIDTH, 300.0), display, near, scale, move |window, cx| NotifView::new(isl, window, cx), cx) else { return false };
    let Ok(view) = handle.entity(cx) else { return false };
    register(id, island, handle.window_id(), hwnd_of(&handle, cx), Rc::new(NotifHooks { handle, view }), cx)
}

fn hwnd_of<V: Render + 'static>(handle: &WindowHandle<V>, cx: &mut App) -> isize {
    handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).unwrap_or(0)
}

fn register(id: PanelId, island: Rc<RefCell<IslandGlass>>, window: WindowId, hwnd: isize, hooks: Rc<dyn Hooks>, cx: &mut App) -> bool {
    if hwnd == 0 {
        return false;
    }
    host_global(cx).slots.push(Slot { id, kind: Kind::Right, side: Side::Right, hwnd, window, island, hooks });
    true
}

/// Called once from the dock's start-up, where the clock's and the bell's panels' windows used to be made: they are made when first
/// opened now (see the top of this file), so there is nothing to do.
pub fn init_extra(_cx: &mut App) {}

// ---------------------------------------------------------------- opening

/// The clock's click: opens the calendar, closes it when it is the open one.
pub fn toggle_calendar(cx: &mut App) {
    toggle_made(CALENDAR, cx);
}

/// The bell's click: opens the notifications, closes them when they are the open panel.
pub fn toggle_notifications(cx: &mut App) {
    toggle_made(NOTIFICATIONS, cx);
}

/// The panels whose window is being made by a task that has not run yet: a second click before it runs must not start a second task
/// (the second would find the window made and toggle it shut again).
#[derive(Default)]
struct Making(Vec<&'static str>);

impl Making {
    /// True when the caller is the one to make it; false when it is being made already.
    fn begin(&mut self, id: &'static str) -> bool {
        if self.0.contains(&id) {
            return false;
        }
        self.0.push(id);
        true
    }

    fn end(&mut self, id: &'static str) {
        self.0.retain(|i| *i != id);
    }
}

thread_local! {
    static MAKING: RefCell<Making> = RefCell::new(Making::default());
}

/// A panel's button was clicked: opens the panel, closes it when it is the open one; the first time, the window is made first.
pub(super) fn toggle_made(id: PanelId, cx: &mut App) {
    let clicked = Instant::now();
    {
        let p = host_global(cx);
        p.clicked_at = Some(clicked);
        p.made_for_click = false;
    }
    if has_slot(id, cx) {
        super::toggle(id, cx);
        return;
    }
    if !MAKING.with(|m| m.borrow_mut().begin(id.0)) {
        return;
    }
    // Not made yet: made now, from the message loop and not from this click's update, then opened.
    cx.spawn(async move |cx| {
        cx.update(|cx| {
            if create(id, cx) {
                let p = host_global(cx);
                p.clicked_at = Some(clicked);
                p.made_for_click = true;
                log::info!("dock: panel {} window made in {:?} from the click", id.0, clicked.elapsed());
                super::toggle(id, cx);
            } else {
                log::warn!("dock: cannot make the {} panel", id.0);
            }
        });
        MAKING.with(|m| m.borrow_mut().end(id.0));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // (not `super::*`: that would bring in gpui's own `test` attribute, which shadows the standard one)
    use super::Making;

    #[test]
    fn a_second_click_before_the_window_is_made_does_not_start_a_second_making() {
        let mut m = Making::default();
        assert!(m.begin("notifications"), "the first click makes it");
        assert!(!m.begin("notifications"), "the second finds it being made: it must not toggle the panel shut");
        assert!(m.begin("calendar"), "the other panel is independent");
        m.end("notifications");
        assert!(m.begin("notifications"), "once made (or failed), a later click may try again");
    }
}
