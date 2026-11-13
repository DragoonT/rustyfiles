//! The dock's panels: borderless windows on HyperGlass that open above an island (the Start panel above the left island, the control
//! centre above the right one). The arithmetic and the decisions are `rf_win::dock::panel` (placement, toggle, what closes a panel,
//! who gets the keyboard back, the 120 ms easing); this file puts them on windows. Each panel is a slot: its id, the island it sits
//! above and lines up with, its window, and the hooks that tell its view what is happening to it.
//!
//! WHAT WAS CHOSEN. A panel's window is made WHEN IT IS FIRST OPENED (the click that asks for it makes it, from a task of the foreground
//! executor, `panel_more::toggle_made`) and kept, HIDDEN, from then on; every later opening is a move, a show and a bring to the front.
//! Until a panel is first asked for it costs nothing at all: no window, no swap chain, no view tree. (They used to be made hidden at the
//! dock's start; a dock that never opens a panel, or one that opens only the Start panel, held up to four idle GPUI windows for
//! nothing.) The price is one window creation at the first click of each panel; the time from the click to the panel being shown is
//! logged at info level (`dock: panel ... shown in ...`) with a note when the window was made for it. A panel at rest has no timer,
//! no thread and no glass: the glass engine is attached when the panel opens and taken off when it has closed. While the glass starts
//! (a few frames) the panel paints the plain fill the popups paint (`glass_watch::panel_fill`).
//!
//! THE KEYBOARD. The islands are never activated; a panel is. On opening, the window that is in front is remembered; the panel is
//! brought to the front (the click that opened it was this process's own input, which is what lets it take the foreground) and its
//! view takes the keys. When the panel is put away with Escape or by its button the remembered window gets the foreground back (if
//! nobody else took it meanwhile); after a click elsewhere or a thing started from the panel the foreground is left alone.
//!
//! WHAT CLOSES IT. Escape; a mouse button going down outside the panel and outside its own island (the island's button toggles it);
//! another window taking the foreground; the dock hiding for a full-screen application; the monitor, its scale or its work area
//! changing (the islands move, the panel would be left behind); Alt+F4 (the window is kept, only put away); its button again. While a panel is open one
//! task looks every 50 ms at the foreground and the mouse buttons for the last two; while none is open nothing runs. Only one panel
//! is open at a time: opening the other puts the first away at once.
//!
//! A PANEL'S HEIGHT may change while it is open (the control centre gains a card when a media session starts): [`relayout`] places it
//! again, still 8 px above its island, so it grows upwards.
//!
//! ALL WIN32 CALLS THAT MOVE, SHOW, HIDE OR FOCUS A WINDOW run in a task of the foreground executor, outside any update of the app
//! (see the notes at the top of `host.rs`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rf_win::dock::grow::Corner;
use rf_win::dock::grow_panels;
use rf_win::dock::layout::Rect;
use rf_win::dock::panel::{self as geo, CloseCause, PanelId, Side};

use super::backdrop::IslandGlass;
use super::control_centre::CcView;
use super::grow_host;
use super::host::{self, Kind, MonId};
use super::left::LeftIsland;
use super::start::StartView;

// The clock's panel and the bell's panel (the calendar and the notifications): their slots and hooks are in `panel_more.rs`, a child of
// this module so that it can use the machinery below.
#[path = "panel_more.rs"]
mod more;
pub use more::{CALENDAR, NOTIFICATIONS, init_extra, toggle_calendar_on, toggle_notifications_on};

// The overflow panel of the tray strip (its slot and hooks are in `tray_panel.rs`, a child module for the same reason).
#[path = "tray_panel.rs"]
mod tray_panel;
pub use tray_panel::{OVERFLOW, overflow_rect, toggle_overflow, toggle_overflow_from_keyboard};

/// The Start panel's size when the screen has the room, logical px (642 x 720; `StartHooks::size` shortens it to the room above the
/// island).
pub const START_SIZE: (f32, f32) = (rf_win::dock::startpage::PANEL_W, rf_win::dock::startpage::PANEL_MAX_H);

/// How often an open panel looks at the foreground and the mouse.
const WATCH_EVERY: Duration = Duration::from_millis(50);

/// What a panel's view is told, and what is asked of it. Each call is made from outside any update of the view.
trait Hooks {
    /// About to be shown: start from a clean state and begin easing in.
    fn on_open(&self, dark: bool, cx: &mut App);
    /// Opened again while it was still shrinking back into its island (the motion turns round): keep what it shows, as a panel that was
    /// never closed. Others start from a clean state, as `on_open` does.
    fn on_reopen(&self, dark: bool, cx: &mut App) {
        self.on_open(dark, cx);
    }
    /// On the screen: the glass goes on, the keyboard goes to the view.
    fn on_shown(&self, dark: bool, cx: &mut App);
    /// Being put away: ease out, stop what was running for it.
    fn on_close(&self, cx: &mut App);
    /// Hidden: the glass comes off, nothing is kept alive.
    fn on_hidden(&self, cx: &mut App);
    /// The size it wants now, logical px (its height may depend on what it shows).
    fn size(&self, cx: &App) -> (f32, f32);
    /// Draw again: the growth out of the island started, moved to a new rectangle or ended (a view that draws only while an animation
    /// asks for frames would otherwise keep the last picture).
    fn redraw(&self, _cx: &mut App) {}
    /// The keyboard goes to the view's text field (only the Start panel has one).
    fn focus_field(&self, _cx: &mut App) {}
    /// The dock ends: the window goes with it.
    fn remove(&self, cx: &mut App);
}

struct Slot {
    id: PanelId,
    /// The island it sits above.
    kind: Kind,
    side: Side,
    hwnd: isize,
    window: WindowId,
    /// The island's glass: its tone (dark or light) is the panel's.
    island: Rc<RefCell<IslandGlass>>,
    hooks: Rc<dyn Hooks>,
}

struct PanelHost {
    slots: Vec<Slot>,
    /// The left island (it shows what is typed in the Start panel).
    left: Option<Entity<LeftIsland>>,
    start: Option<Entity<StartView>>,
    /// The panel that is open, if any (only one at a time).
    open: Option<PanelId>,
    /// Bumped at every open and close: a task of an earlier state sees that and does nothing.
    serial: u64,
    /// Where the open panel's window is, device px (the grown glass's rectangle while the panel grows out of its island).
    rect: Rect,
    /// Where the open panel's own content is, device px: `rect` for an ordinary panel; inside the grown glass for one that grows (the
    /// glass also holds the island's row). The tray strip finds the overflow panel's cells from it.
    content: Rect,
    /// The panel that opens when the one now leaving (shrinking back into its island) is gone: a press on another button of the island's
    /// row while a panel is open (`rf_win::dock::grow_panels::press`).
    after: Option<PanelId>,
    /// How many times each panel has been told to close: the closing task of an earlier close leaves the hiding to a later one.
    close_seqs: grow_panels::CloseSeqs,
    /// The window that had the foreground when the panel opened.
    previous: isize,
    /// A window the opener hands over as the one to go back to, and when (see `tray_panel::toggle_overflow_from_keyboard`).
    previous_hint: Option<(isize, Instant)>,
    opened_at: Instant,
    been_front: bool,
    mouse_was_down: bool,
    watcher: Option<Task<()>>,
    /// What is typed in the Start panel's field (the left island shows it).
    typed: String,
    /// The islands' glass the panels made later take their tone from (set at the dock's start by `init` and `init_control_centre`).
    left_island: Option<Rc<RefCell<IslandGlass>>>,
    right_island: Option<Rc<RefCell<IslandGlass>>>,
    /// When the button that asked for the panel now opening was clicked, and whether its window had to be made for it (both for the log).
    clicked_at: Option<Instant>,
    made_for_click: bool,
    /// The monitor the panel asked for last is to open on (the dock whose island was pressed; the Windows key: the one with the pointer).
    /// `open_panel` makes it the active monitor of the pop-ups (`host::force_active`) and everything the panel asks about the islands
    /// (their glass, the work area, the unit) is then about that monitor's dock.
    want_mon: MonId,
}

impl Global for PanelHost {}

impl PanelHost {
    fn slot(&self, id: PanelId) -> Option<&Slot> {
        self.slots.iter().find(|s| s.id == id)
    }
}

fn host_global(cx: &mut App) -> &mut PanelHost {
    if !cx.has_global::<PanelHost>() {
        cx.set_global(PanelHost {
            slots: Vec::new(),
            left: None,
            start: None,
            open: None,
            serial: 0,
            rect: Rect::default(),
            content: Rect::default(),
            after: None,
            close_seqs: grow_panels::CloseSeqs::default(),
            previous: 0,
            previous_hint: None,
            opened_at: Instant::now(),
            been_front: false,
            mouse_was_down: false,
            watcher: None,
            typed: String::new(),
            left_island: None,
            right_island: None,
            clicked_at: None,
            made_for_click: false,
            want_mon: MonId::HOME,
        });
    }
    cx.global_mut::<PanelHost>()
}

/// The hidden window of a panel: a transparent pop-up that is not shown yet (made by `popup_win`, which the dock's other pop-ups share;
/// Alt+F4 in an open panel puts it away, it must not destroy the window: a destroyed window is never opened again).
fn open_hidden<V: Render + 'static>(
    initial: (f32, f32),
    display: Option<DisplayId>,
    near: Rect,
    scale: f32,
    build: impl FnOnce(&mut Window, &mut Context<V>) -> V + 'static,
    cx: &mut App,
) -> Option<WindowHandle<V>> {
    super::popup_win::open_hidden(initial, display, near, scale, |cx| close(CloseCause::WindowClose, cx), build, cx)
}

// ---------------------------------------------------------------- the Start panel

struct StartHooks {
    handle: WindowHandle<StartView>,
}

impl Hooks for StartHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        // the input field's colours come from the component theme: make it the panel's tone
        apply_input_theme(dark, cx);
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_reopen(&self, dark: bool, cx: &mut App) {
        apply_input_theme(dark, cx);
        let _ = self.handle.update(cx, |v, window, cx| v.on_reopen(window, cx));
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
        // 720 where the room above the island allows it, else all of that room (the window is placed from this; `geo::place` also
        // keeps it inside the work area)
        let room = host::monitor_work(cx)
            .zip(host::visible_glass_rect(Kind::Left, cx))
            .map(|((scale, work), glass)| rf_win::dock::startpage::room_above(glass.top, work.top, scale, geo::GAP));
        (START_SIZE.0, room.map_or(START_SIZE.1, rf_win::dock::startpage::panel_height))
    }
    fn redraw(&self, cx: &mut App) {
        // (the growth starting, moving or ending is what moves the field between the panel's top and the island's row: its placeholder follows)
        let _ = self.handle.update(cx, |v, window, cx| {
            v.sync_placeholder(window, cx);
            window.refresh();
        });
    }
    fn focus_field(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.focus_search(window, cx));
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// The Start panel at the dock's start-up: only remembers the left island (its glass gives the panel its tone, the entity shows what is
/// typed in the panel). The window is made when the panel is first opened, see [`create_start`]. The other parameters are what the window
/// used to be made from at this point and are looked up again when it is made (the display may have changed by then).
pub fn init(island: Rc<RefCell<IslandGlass>>, left: Entity<LeftIsland>, _display: Option<DisplayId>, _scale: f32, _near: Rect, cx: &mut App) {
    let p = host_global(cx);
    p.left = Some(left);
    p.left_island = Some(island);
}

/// Makes the Start panel's window, hidden, and its slot. True when it exists afterwards (made now or before).
fn create_start(cx: &mut App) -> bool {
    if panel(cx).is_some_and(|p| p.slot(PanelId::START).is_some()) {
        return true;
    }
    let Some((scale, _)) = host::monitor_work(cx) else { return false };
    let Some(near) = host::visible_glass_rect(Kind::Left, cx) else { return false };
    let Some(island) = panel(cx).and_then(|p| p.left_island.clone()) else { return false };
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    let Some(handle) = open_hidden(START_SIZE, display, near, scale, move |window, cx| StartView::new(isl, window, cx), cx) else { return false };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return false };
    let Ok(view) = handle.entity(cx) else { return false };
    let window = handle.window_id();
    let p = host_global(cx);
    p.start = Some(view);
    p.slots.push(Slot { id: PanelId::START, kind: Kind::Left, side: Side::Left, hwnd, window, island, hooks: Rc::new(StartHooks { handle }) });
    true
}

// ---------------------------------------------------------------- the control centre

struct CcHooks {
    handle: WindowHandle<CcView>,
    view: Entity<CcView>,
}

impl Hooks for CcHooks {
    fn on_open(&self, dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_open(dark, window, cx));
    }
    fn on_reopen(&self, _dark: bool, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| v.on_reopen(window, cx));
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
        (super::cc_model::WIDTH, self.view.read(cx).height(cx))
    }
    fn redraw(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |_, window, _| window.refresh());
    }
    fn remove(&self, cx: &mut App) {
        let _ = self.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

/// The control centre at the dock's start-up: only remembers the right island (the glass that gives this panel and the clock's and the
/// bell's their tone). The window is made when the panel is first opened, see [`create_control_centre`].
pub fn init_control_centre(island: Rc<RefCell<IslandGlass>>, _display: Option<DisplayId>, _scale: f32, _near: Rect, cx: &mut App) {
    host_global(cx).right_island = Some(island);
}

/// Makes the control centre's window, hidden, and its slot. True when it exists afterwards (made now or before).
fn create_control_centre(cx: &mut App) -> bool {
    if panel(cx).is_some_and(|p| p.slot(PanelId::CONTROL_CENTRE).is_some()) {
        return true;
    }
    let Some((scale, _)) = host::monitor_work(cx) else { return false };
    let Some(near) = host::visible_glass_rect(Kind::Right, cx) else { return false };
    let Some(island) = panel(cx).and_then(|p| p.right_island.clone()) else { return false };
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    let Some(handle) = open_hidden((super::cc_model::WIDTH, 300.0), display, near, scale, move |window, cx| CcView::new(isl, window, cx), cx) else { return false };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return false };
    let Ok(view) = handle.entity(cx) else { return false };
    let window = handle.window_id();
    let p = host_global(cx);
    p.slots.push(Slot { id: PanelId::CONTROL_CENTRE, kind: Kind::Right, side: Side::Right, hwnd, window, island, hooks: Rc::new(CcHooks { handle, view }) });
    true
}

// ---------------------------------------------------------------- opening, watching, closing

fn panel(cx: &App) -> Option<&PanelHost> {
    cx.try_global::<PanelHost>()
}

/// Whether a panel is open, or the one that was is still on its way out (or the next one waits for it). The thin dock stays grown, and
/// its band stays, while this is true.
pub fn is_open(cx: &App) -> bool {
    panel(cx).is_some_and(|p| p.open.is_some() || p.after.is_some())
}

/// The Start panel's view (the file answers from the main app are handed to it).
pub fn view(cx: &App) -> Option<Entity<StartView>> {
    panel(cx).and_then(|p| p.start.clone())
}

/// The left island's view on the active monitor (the growth tells it to draw again when it starts and ends).
pub fn left_view(cx: &App) -> Option<Entity<LeftIsland>> {
    host::left_island_on(host::active(cx), cx).or_else(|| panel(cx).and_then(|p| p.left.clone()))
}

/// What the user has typed in the open Start panel's field, for the left island's search field to show; `None` when nothing is typed.
pub fn typed_text(cx: &App) -> Option<String> {
    panel(cx).map(|p| p.typed.clone()).filter(|t| !t.is_empty())
}

/// The Start panel's field changed: the left island shows it.
pub fn set_typed(text: &str, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    if p.typed == text {
        return;
    }
    cx.global_mut::<PanelHost>().typed = text.to_string();
    // (every left island draws again: the one whose panel it is shows the word)
    for left in host::left_islands(cx) {
        left.update(cx, |_, cx| cx.notify());
    }
}

/// A press on the left island's search field while the panel's glass has grown out of it: the panel's own field stands under the
/// island's window there, so the press cannot reach it; the keyboard goes to it from here (a field that has it already keeps it).
pub fn focus_start_field(cx: &mut App) {
    let Some(hooks) = panel(cx).and_then(|p| p.slot(PanelId::START)).map(|s| s.hooks.clone()) else { return };
    hooks.focus_field(cx);
}

/// The island button's click on this monitor's dock: opens the Start panel there, closes it when it is the open one there (open on
/// another monitor's dock, it moves).
pub fn toggle_start_on(mon: MonId, cx: &mut App) {
    more::toggle_made(PanelId::START, mon, cx);
}

/// [`toggle_start`], and says whether it ended with the Start panel opened or closed as asked. False: it was asked to open and could
/// not (a full-screen program in front, the dock being rebuilt, no glass to sit above); nothing happened. The keyboard layer needs
/// the answer: it has already kept Windows' own Start shut by then.
pub fn toggle_start_reported(cx: &mut App) -> bool {
    let seen = |cx: &App| panel(cx).map_or(grow_panels::Seen { open: None, waiting: None }, |p| grow_panels::Seen { open: p.open, waiting: p.after });
    let before = seen(cx);
    // (the Windows key: Start opens on the monitor that has the pointer, but a tap while Start is open CLOSES it, wherever the pointer is)
    let pointer = host::monitor_at_pointer(cx);
    let open_on = before.open.is_some_and(|id| id == PanelId::START).then(|| host::active(cx));
    toggle_start_on(rf_win::dock::monitors::key_target(open_on, pointer), cx);
    // (a tap while another panel is grown only QUEUES Start behind the one shrinking into its island: that is not "could not open")
    grow_panels::took_effect(before, seen(cx), PanelId::START)
}

/// The right island's network or volume symbol: opens the control centre, closes it when it is the open one.
pub fn toggle_control_centre_on(mon: MonId, cx: &mut App) {
    more::toggle_made(PanelId::CONTROL_CENTRE, mon, cx);
}

/// A button of the island of this monitor's dock asked for panel `id`. One panel at a time over all monitors
/// (`rf_win::dock::monitors::panel_request`): the same panel open on another monitor moves to this one.
fn toggle(id: PanelId, mon: MonId, cx: &mut App) {
    let (open, waiting) = panel(cx).map_or((None, None), |p| (p.open, p.after));
    host_global(cx).want_mon = mon;
    let on = host::active(cx);
    if rf_win::dock::monitors::panel_request(open.as_ref().map(|o| (o, &on)), (&id, &mon)) == rf_win::dock::monitors::PanelAction::MoveTo {
        // out of the island it grew from and into the one pressed: it is put away (shrinking back), and opens again on the new monitor
        host_global(cx).after = Some(id);
        close(CloseCause::Button, cx);
        // (nothing to put away after all: it opens now rather than wait for a window that is not going)
        if panel(cx).is_some_and(|p| p.open.is_some()) {
            host_global(cx).after = None;
            open_panel(id, cx);
        }
        return;
    }
    // A panel that has grown out of its island: the island's row stays, so a button of the row can be pressed while it is open. The
    // open panel shrinks into the island and the pressed one grows out of it when the first has gone (`grow_panels` says why it is not
    // one glass going from panel to panel). A panel that is leaving is still the growth's (it is no longer `open`).
    let state = grow_panels::PressState {
        open,
        open_grown: open.is_some_and(|o| grow_host::active_for(o, cx)),
        leaving: grow_host::current_id(cx).filter(|g| Some(*g) != open),
        waiting,
    };
    match grow_panels::press(&state, id) {
        grow_panels::Press::OpenNow => {
            // (a press on the panel that is shrinking turns it round; whatever waited for it to go is dropped)
            host_global(cx).after = None;
            open_panel(id, cx);
        }
        grow_panels::Press::SwitchNow => {
            host_global(cx).after = None;
            open_panel(id, cx);
        }
        grow_panels::Press::Close => close(CloseCause::Button, cx),
        grow_panels::Press::CloseThenOpen => {
            host_global(cx).after = Some(id);
            close(CloseCause::Button, cx);
            // (nothing to put away after all: it opens now rather than wait for a window that is not going)
            if panel(cx).is_some_and(|p| p.open.is_some()) {
                host_global(cx).after = None;
                open_panel(id, cx);
            }
        }
        grow_panels::Press::OpenWhenHome => host_global(cx).after = Some(id),
        grow_panels::Press::Withdraw => host_global(cx).after = None,
    }
}

/// The panel that was waiting for the one leaving is opened now that the leaving one is gone (its window hidden, its growth ended).
fn open_waiting(cx: &mut App) {
    let Some(id) = host_global(cx).after.take() else { return };
    if panel(cx).is_some_and(|p| p.open.is_none()) {
        open_panel(id, cx);
    }
}

/// Where a panel's own content stands: inside the grown glass when it grows, else it is the window.
fn content_of(id: PanelId, grown: Option<Rect>, panel_rect: Rect, side: Side) -> Rect {
    grown.map_or(panel_rect, |g| grow_panels::content_rect_in(g, panel_rect, Corner::from(side), id != OVERFLOW))
}

/// Whether the open panel is dark: the tone of the island it sits above at this moment decides (the island follows what is behind its
/// glass, and the panel is opened over the same wallpaper, so it takes the same look; it does not follow the island while open).
fn dark_now(island: &RefCell<IslandGlass>) -> bool {
    island.borrow().dark()
}

fn open_panel(id: PanelId, cx: &mut App) {
    // the dock whose island asked is the one the panel sits above: everything below asks about THAT monitor
    let want = panel(cx).map_or(MonId::HOME, |p| p.want_mon);
    host::force_active(want, cx);
    if host::is_fullscreen(cx) {
        return;
    }
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd, hooks) = (slot.kind, slot.side, slot.hwnd, slot.hooks.clone());
    // the tone is the glass of the island of the active monitor (the panel's own slot remembers the home dock's)
    let island = host::island_glass(kind, cx).unwrap_or_else(|| slot.island.clone());
    let other = p.open.filter(|o| *o != id);
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let dark = dark_now(&island);
    // A panel that grows out of its island (with the setting on) has a window as big as the grown glass; see grow_host.rs.
    let size = hooks.size(cx);
    let panel_rect = geo::place(glass, size, scale, side, work);
    // (not where the grown glass would lie under another island: that screen opens the panel the ordinary way, see `grow_host::fits`)
    let grow_cfg = grow_host::config(id, kind, cx).filter(|cfg| grow_host::fits(*cfg, glass, panel_rect, scale, cx));
    // (a panel that grows gives up its own bottom row, the island's row is its bottom row: placed with the smaller height)
    let panel_rect = grow_cfg.map_or(panel_rect, |cfg| geo::place(glass, grow_host::panel_size(cfg, size), scale, side, work));
    // pressed again while the panel shrinks back into its island: the motion turns round and the panel keeps what it shows
    let reversal = grow_cfg.is_some() && grow_host::is_closing(id, cx);
    let grown = grow_cfg.and_then(|cfg| grow_host::prepare(grow_host::Prep { id, cfg, side, island_glass: glass, panel: panel_rect, hwnd, island: island.clone(), dark, scale }, cx));
    // (the setting was switched off while this panel was still shrinking: the growth is over, the window opens as an ordinary one)
    if grow_cfg.is_none() && grow_host::active_for(id, cx) {
        grow_host::abort(cx);
    }
    let rect = grown.unwrap_or(panel_rect);
    let content = content_of(id, grown, panel_rect, side);
    // another panel is open: it goes at once (no easing: this one takes its place)
    if let Some(o) = other {
        put_away_now(o, cx);
    }
    let front = rf_win::dock::foreground_root();
    let front_is_island = host::is_island_window(front, cx);
    let serial = {
        let p = cx.global_mut::<PanelHost>();
        p.serial += 1;
        // (a panel that is still easing out has no `open`, and the window in front is the panel itself then: nothing to remember. An island
        // in front is the tray strip's keyboard walk: not a window to go back to either; the strip says which one it took the keyboard from.)
        let hint = p.previous_hint.take().map(|(w, at)| (w, at.elapsed().as_millis() as u64));
        if p.open.is_none() {
            let whose = if p.slots.iter().any(|s| s.hwnd == front) {
                geo::FrontIs::PanelSlot
            } else if front_is_island {
                geo::FrontIs::Island
            } else {
                geo::FrontIs::Other
            };
            if let Some(prev) = geo::previous_on_open(front, whose, hint) {
                p.previous = prev;
            }
        }
        p.open = Some(id);
        p.rect = rect;
        p.content = content;
        p.opened_at = Instant::now();
        p.been_front = false;
        p.mouse_was_down = geo::any_button_down();
        p.serial
    };
    start_state_changed(id, true, cx);
    if reversal {
        hooks.on_reopen(dark, cx);
    } else {
        hooks.on_open(dark, cx);
    }
    // the rectangle again: opening may have changed what the panel wants to show (its height)
    let size = hooks.size(cx);
    let panel_rect = geo::place(glass, grow_cfg.map_or(size, |cfg| grow_host::panel_size(cfg, size)), scale, side, work);
    let grown = grow_cfg.and_then(|cfg| grow_host::prepare(grow_host::Prep { id, cfg, side, island_glass: glass, panel: panel_rect, hwnd, island: island.clone(), dark, scale }, cx));
    let rect = grown.unwrap_or(panel_rect);
    {
        let p = cx.global_mut::<PanelHost>();
        p.rect = rect;
        p.content = content_of(id, grown, panel_rect, side);
    }
    // show, bring to the front, then glass (in this order: the engine attaches to a window that is on the screen)
    let h = hooks.clone();
    let is_grown = grown.is_some();
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        if is_grown {
            // the window's region is the island's rectangle (or where the glass is) before the window is shown (the styling and the
            // region are window calls: made here, with nothing of the app borrowed, like the showing below)
            if let Some(prepare_window) = cx.update(grow_host::before_show) {
                prepare_window();
            }
        }
        geo::show_at(hwnd, rect);
        // (the quiet kind: no tap of the Alt key into the window that is in front; the click that opened the panel was ours)
        rf_win::window::bring_to_front_quietly(hwnd);
        if is_grown && let Some(island) = cx.update(|cx| grow_host::island_to_raise(cx)) {
            // the island's window goes back above the panel's, so the logo is drawn over the grown glass and takes its clicks
            rf_win::dock::island::raise(island);
        }
        cx.update(|cx| {
            h.on_shown(dark, cx);
            if is_grown {
                grow_host::shown(cx);
            }
            // the time the user waited: from the click on the button to the panel being on the screen with its glass attached
            if let Some(p) = cx.try_global::<PanelHost>()
                && let Some(t) = p.clicked_at
            {
                log::info!("dock: panel {} shown in {:?} from the click{}", id.0, t.elapsed(), if p.made_for_click { " (window made for it)" } else { "" });
            }
        });
    })
    .detach();
    // the watch of an open panel
    let watcher = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(WATCH_EVERY).await;
            if !cx.update(|cx| watch_once(serial, cx)) {
                break;
            }
        }
    });
    cx.global_mut::<PanelHost>().watcher = Some(watcher);
}

/// A panel that changed its mind about its height while open: placed again above its island (it grows upwards). `height` is the new
/// one, logical px (the view's own height cannot be read from inside its own update).
pub fn relayout(id: PanelId, height: f32, cx: &mut App) {
    relayout_sized(id, (super::cc_model::WIDTH, height), cx);
}

/// [`relayout`] for a panel that is not as wide as the control centre.
pub fn relayout_sized(id: PanelId, size: (f32, f32), cx: &mut App) {
    refit(id, Some(size), cx);
}

/// The host's layout was worked out again (an island's width changed: the clock's text, an icon moved between the tray strip and the
/// overflow panel). An ordinary panel needs nothing (it is lined up with the island's edge, which does not move). A panel grown out of
/// its island works its glass out again for the island's new rectangle and moves its window. Called from inside the layout, so the
/// work is deferred.
pub fn layout_applied(cx: &mut App) {
    let Some(id) = grow_host::current_id(cx) else { return };
    if panel(cx).and_then(|p| p.open) != Some(id) {
        return;
    }
    cx.defer(move |cx| refit(id, None, cx));
}

/// Places the open panel again: with the size it asks for now (`size`, or the hooks' own when `None`) and the island's glass as it is
/// now. A panel that grows out of its island keeps growing: its glass is worked out again and the window follows it.
fn refit(id: PanelId, size: Option<(f32, f32)>, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    if p.open != Some(id) {
        return;
    }
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd, hooks) = (slot.kind, slot.side, slot.hwnd, slot.hooks.clone());
    let island = host::island_glass(kind, cx).unwrap_or_else(|| slot.island.clone());
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let size = size.unwrap_or_else(|| hooks.size(cx));
    let grow_cfg = grow_host::cfg_of(id, cx);
    // The islands may have moved since the panel opened (a pinned or launched app widens the middle island): the grown glass is checked
    // again against them, with the same rule the opening used. One that now lies under another island cannot stay: the panel is put
    // away at once (a deferred close: this can run inside the panel's own update, which must not be re-entered).
    if let Some(cfg) = grow_cfg
        && !grow_host::fits(cfg, glass, geo::place(glass, size, scale, side, work), scale, cx)
    {
        cx.defer(|cx| close(CloseCause::LayoutChanged, cx));
        return;
    }
    let panel_rect = geo::place(glass, grow_cfg.map_or(size, |cfg| grow_host::panel_size(cfg, size)), scale, side, work);
    let grown = grow_cfg.and_then(|cfg| {
        let dark = dark_now(&island);
        grow_host::prepare(grow_host::Prep { id, cfg, side, island_glass: glass, panel: panel_rect, hwnd, island, dark, scale }, cx)
    });
    let rect = grown.unwrap_or(panel_rect);
    let content = content_of(id, grown, panel_rect, side);
    let p = cx.global::<PanelHost>();
    if p.rect == rect && p.content == content {
        return;
    }
    {
        let p = cx.global_mut::<PanelHost>();
        p.rect = rect;
        p.content = content;
    }
    // (the island's window is raised again in the same task: moving a topmost window puts it at the top of the topmost windows, over the
    // island's row, which would stay covered and swallow its clicks until the watch looked at the order 50 ms later)
    let island_above = grown.and_then(|_| grow_host::island_to_raise(cx));
    // (the glass and the region are in the window's coordinates: they are sent in the same task as the move, not a frame later)
    let resend = grown.and_then(|_| grow_host::after_move(cx));
    cx.spawn(async move |_| {
        geo::show_at(hwnd, rect);
        if let Some(island) = island_above {
            rf_win::dock::island::raise(island);
        }
        if let Some(resend) = resend {
            resend();
        }
    })
    .detach();
    // (and a frame is asked for, outside the update this may be running in: the view's own update cannot be entered again)
    if grown.is_some() {
        cx.defer(move |cx| redraw(id, cx));
    }
}

/// Asks a panel's view to draw again.
pub fn redraw(id: PanelId, cx: &mut App) {
    let Some(hooks) = panel(cx).and_then(|p| p.slot(id)).map(|s| s.hooks.clone()) else { return };
    hooks.redraw(cx);
}

/// One look of the open panel at the foreground and the mouse. False when this watch is over (the panel closed or reopened).
fn watch_once(serial: u64, cx: &mut App) -> bool {
    let Some(p) = cx.try_global::<PanelHost>() else { return false };
    let Some(open) = p.open.filter(|_| p.serial == serial) else { return false };
    let Some(slot) = p.slot(open) else { return false };
    let (hwnd, kind) = (slot.hwnd, slot.kind);
    let (rect, opened_at, been_front, was_down) = (p.rect, p.opened_at, p.been_front, p.mouse_was_down);
    // (a panel that grows is "on" where its glass is now, not where it will be: a click on the desktop above the half-grown glass is outside)
    let rect = grow_host::visible_rect(cx).unwrap_or(rect);
    // (and the island stays above it: the panel's window covers the island's row, and a panel that was activated again is on top)
    grow_host::keep_island_above(cx);
    let keep = host::visible_glass_rect(kind, cx).unwrap_or_default();
    let fg = rf_win::dock::foreground_root();
    let front = fg == hwnd;
    let down = geo::any_button_down();
    let outside = geo::cursor().is_some_and(|c| geo::outside_click(was_down, down, c, rect, keep));
    {
        let p = cx.global_mut::<PanelHost>();
        p.been_front |= front;
        p.mouse_was_down = down;
    }
    if outside {
        close(CloseCause::ClickOutside, cx);
        return false;
    }
    if geo::lost_foreground(hwnd, fg, been_front || front, opened_at.elapsed().as_millis() as u64) {
        close(CloseCause::ForegroundLost, cx);
        return false;
    }
    true
}

/// Puts the open panel away: it eases out over 120 ms (at once with animations off), then the window is hidden and the glass taken
/// off. Does nothing when none is open.
pub fn close(cause: CloseCause, cx: &mut App) {
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    let Some(id) = p.open else { return };
    let Some(slot) = p.slot(id) else { return };
    let (hwnd, hooks) = (slot.hwnd, slot.hooks.clone());
    let (previous, left) = (p.previous, host::left_island_on(host::active(cx), cx).or_else(|| p.left.clone()));
    // A panel that grows out of its island shrinks back into it (the clock turns; `wait` is how long the window stays). A layout that
    // changed under it or a full-screen program do not wait: the island has moved or is hidden. Else the 120 ms easing of any panel.
    let forced = matches!(cause, CloseCause::LayoutChanged | CloseCause::FullScreen);
    if forced {
        // (a hand-over to another panel is off too: the island moved, or is hidden)
        host_global(cx).after = None;
    }
    let wait = if !grow_host::active_for(id, cx) {
        None
    } else if forced {
        grow_host::abort(cx);
        Some(0)
    } else {
        grow_host::begin_close(cx)
    };
    let was_grown = wait.is_some();
    let animate = crate::settings::animations() && !(forced && was_grown);
    {
        let p = cx.global_mut::<PanelHost>();
        p.open = None;
        p.serial += 1;
        p.watcher = None;
        p.typed.clear();
    }
    // (the number this close's task keeps: see `CloseSeqs`)
    let mine = host_global(cx).close_seqs.bump(id);
    if let Some(left) = left {
        left.update(cx, |l, cx| if id == PanelId::START { l.panel_changed(false, cx) } else { cx.notify() });
    }
    hooks.on_close(cx);
    let restore = cause.restores_foreground();
    let wait_ms = match wait {
        // (a growth says how long itself: a little over the motion's end and its deadline, which gives the island its glass back, also
        // with animations off)
        Some(ms) => u64::from(ms),
        None if animate => u64::from(geo::ANIM_MS) + 20,
        None => 0,
    };
    cx.spawn(async move |cx| {
        if wait_ms > 0 {
            cx.background_executor().timer(Duration::from_millis(wait_ms)).await;
        }
        // opened again meanwhile (this panel itself): leave everything as it is. Another panel that opened has put this one away. And a
        // later close of this panel (opened again, closed again) owns the hiding: this task is not the one to do it in the middle of that.
        let stale = cx.update(|cx| panel(cx).is_some_and(|p| p.open == Some(id) || !p.close_seqs.is_latest(id, mine)));
        if stale {
            return;
        }
        let fg = rf_win::dock::foreground_root();
        geo::hide(hwnd);
        // a panel waits for this one to be gone (a press on another button of the island's row): it opens now, and the window that had
        // the keyboard before this one is not given it back in between: the next panel is handed it as the window to go back to (the
        // foreground at this moment is whatever Windows activated when this window went, not the user's window), takes the keyboard and
        // gives it back when it is put away. A waiting panel that could not open leaves the keyboard with nobody: it is given back here.
        let handing_over = cx.update(|cx| {
            hooks.on_hidden(cx);
            if was_grown {
                grow_host::finish(id, cx);
            }
            let waiting = panel(cx).is_some_and(|p| p.after.is_some());
            let ours = waiting && host_global(cx).previous_hint.is_none();
            if ours {
                host_global(cx).previous_hint = Some((previous, Instant::now()));
            }
            open_waiting(cx);
            let opened = panel(cx).is_some_and(|p| p.open.is_some());
            if ours && !opened {
                host_global(cx).previous_hint = None;
            }
            grow_panels::hands_keyboard_on(waiting, opened)
        });
        if restore && !handing_over && let Some(target) = geo::restore_to(previous, fg, hwnd, geo::exists(previous)) {
            geo::give_back(target);
        }
    })
    .detach();
}

/// A panel goes at once: its view is told, its window hidden, nothing eased (another panel is taking its place).
fn put_away_now(id: PanelId, cx: &mut App) {
    let Some(slot) = panel(cx).and_then(|p| p.slot(id)) else { return };
    let (hwnd, hooks) = (slot.hwnd, slot.hooks.clone());
    {
        let p = cx.global_mut::<PanelHost>();
        p.open = None;
        p.watcher = None;
        p.typed.clear();
    }
    start_state_changed(id, false, cx);
    hooks.on_close(cx);
    // (a panel that was growing out of its island: the island has its glass, the window its region and frame back; before the window
    // goes, so the island never stands without a glass for a frame)
    // (the setting may have been switched off meanwhile, so this asks whether a growth is going on and not what the setting says)
    if grow_host::active_for(id, cx) {
        grow_host::abort(cx);
    }
    geo::hide(hwnd);
    hooks.on_hidden(cx);
}

/// The Start panel opened or closed: the logo on the left island keeps its "active" plate while it is open and plays the press and
/// release when the panel was opened by something other than a click on the logo. (Other panels are none of the logo's business.)
fn start_state_changed(id: PanelId, open: bool, cx: &mut App) {
    if id != PanelId::START {
        return;
    }
    let Some(left) = host::left_island_on(host::active(cx), cx).or_else(|| panel(cx).and_then(|p| p.left.clone())) else { return };
    left.update(cx, |l, cx| l.panel_changed(open, cx));
}

/// The dock ends: the panels' windows go with it.
pub fn shutdown(cx: &mut App) {
    // the pop-ups above the icons (made when first needed) go with the dock
    super::app_menu::shutdown(cx);
    super::hover_preview::shutdown(cx);
    super::tray_tip::shutdown(cx);
    super::tray_strip::shutdown(cx);
    if !cx.has_global::<PanelHost>() {
        return;
    }
    let p = cx.remove_global::<PanelHost>();
    for s in &p.slots {
        s.hooks.remove(cx);
    }
}

/// True when every window left is a panel's or one of the desktop's cover windows (or none is left): the islands are gone, the dock
/// has nothing to show. (The cover windows are not the dock: counting them as live windows would keep the host running with no dock.)
pub fn only_panels_left(cx: &App) -> bool {
    let mut mine: Vec<WindowId> = panel(cx).map(|p| p.slots.iter().map(|s| s.window).collect()).unwrap_or_default();
    mine.extend(super::app_menu::window_ids(cx));
    mine.extend(super::hover_preview::window_ids(cx));
    mine.extend(super::tray_tip::window_ids(cx));
    let open: Vec<WindowId> = cx.windows().iter().map(|w| w.window_id()).collect();
    crate::shell::cover::only_these_left(&open, &mine, &crate::shell::cover::window_ids(cx))
}

/// The component theme (the text field's colours) follows the panel's tone, not the app's theme setting: the dock process has no
/// other component to disagree.
fn apply_input_theme(dark: bool, cx: &mut App) {
    use gpui_kit::component::theme::{Theme, ThemeMode};
    Theme::change(if dark { ThemeMode::Dark } else { ThemeMode::Light }, None, cx);
    Theme::update(cx, |t| t.background = gpui_kit::transparent_black());
    if std::path::Path::new(r"C:\Windows\Fonts\SegUIVar.ttf").exists() {
        Theme::update(cx, |t| t.font_family = "Segoe UI Variable Text".into());
    }
}
