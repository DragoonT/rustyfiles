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
use rf_win::dock::layout::Rect;
use rf_win::dock::panel::{self as geo, CloseCause, PanelId, Side, Toggle};

use super::backdrop::IslandGlass;
use super::control_centre::CcView;
use super::host::{self, Kind};
use super::left::LeftIsland;
use super::start::StartView;

// The clock's panel and the bell's panel (the calendar and the notifications): their slots and hooks are in `panel_more.rs`, a child of
// this module so that it can use the machinery below.
#[path = "panel_more.rs"]
mod more;
pub use more::{NOTIFICATIONS, init_extra, toggle_calendar, toggle_notifications};

/// The Start panel's size, logical px.
pub const START_SIZE: (f32, f32) = (620.0, 680.0);

/// How often an open panel looks at the foreground and the mouse.
const WATCH_EVERY: Duration = Duration::from_millis(50);

/// What a panel's view is told, and what is asked of it. Each call is made from outside any update of the view.
trait Hooks {
    /// About to be shown: start from a clean state and begin easing in.
    fn on_open(&self, dark: bool, cx: &mut App);
    /// On the screen: the glass goes on, the keyboard goes to the view.
    fn on_shown(&self, dark: bool, cx: &mut App);
    /// Being put away: ease out, stop what was running for it.
    fn on_close(&self, cx: &mut App);
    /// Hidden: the glass comes off, nothing is kept alive.
    fn on_hidden(&self, cx: &mut App);
    /// The size it wants now, logical px (its height may depend on what it shows).
    fn size(&self, cx: &App) -> (f32, f32);
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
    /// Where the open panel is, device px.
    rect: Rect,
    /// The window that had the foreground when the panel opened.
    previous: isize,
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
            previous: 0,
            opened_at: Instant::now(),
            been_front: false,
            mouse_was_down: false,
            watcher: None,
            typed: String::new(),
            left_island: None,
            right_island: None,
            clicked_at: None,
            made_for_click: false,
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
        START_SIZE
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

/// The Start panel's view (the file answers from the main app are handed to it).
pub fn view(cx: &App) -> Option<Entity<StartView>> {
    panel(cx).and_then(|p| p.start.clone())
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
    let left = p.left.clone();
    cx.global_mut::<PanelHost>().typed = text.to_string();
    if let Some(left) = left {
        left.update(cx, |_, cx| cx.notify());
    }
}

/// The island button's click: opens the Start panel, closes it when it is the open one.
pub fn toggle_start(cx: &mut App) {
    more::toggle_made(PanelId::START, cx);
}

/// [`toggle_start`], and says whether it ended with the Start panel opened or closed as asked. False: it was asked to open and could
/// not (a full-screen program in front, the dock being rebuilt, no glass to sit above); nothing happened. The keyboard layer needs
/// the answer: it has already kept Windows' own Start shut by then.
pub fn toggle_start_reported(cx: &mut App) -> bool {
    let is_open = |cx: &App| panel(cx).is_some_and(|p| p.open == Some(PanelId::START));
    let was_open = is_open(cx);
    toggle_start(cx);
    was_open || is_open(cx)
}

/// The right island's network or volume symbol: opens the control centre, closes it when it is the open one.
pub fn toggle_control_centre(cx: &mut App) {
    more::toggle_made(PanelId::CONTROL_CENTRE, cx);
}

fn toggle(id: PanelId, cx: &mut App) {
    let open = panel(cx).and_then(|p| p.open);
    match geo::toggle(open, id) {
        Toggle::Open | Toggle::Switch => open_panel(id, cx),
        Toggle::Close => close(CloseCause::Button, cx),
    }
}

/// Whether the open panel is dark: the tone of the island it sits above at this moment decides (the island follows what is behind its
/// glass, and the panel is opened over the same wallpaper, so it takes the same look; it does not follow the island while open).
fn dark_now(island: &RefCell<IslandGlass>) -> bool {
    island.borrow().dark()
}

fn open_panel(id: PanelId, cx: &mut App) {
    if host::is_fullscreen(cx) {
        return;
    }
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd, island, hooks) = (slot.kind, slot.side, slot.hwnd, slot.island.clone(), slot.hooks.clone());
    let other = p.open.filter(|o| *o != id);
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let rect = geo::place(glass, hooks.size(cx), scale, side, work);
    // another panel is open: it goes at once (no easing: this one takes its place)
    if let Some(o) = other {
        put_away_now(o, cx);
    }
    let dark = dark_now(&island);
    let serial = {
        let p = cx.global_mut::<PanelHost>();
        p.serial += 1;
        // (a panel that is still easing out has no `open`, and the window in front is the panel itself then: nothing to remember)
        if p.open.is_none() {
            let front = rf_win::dock::foreground_root();
            if !p.slots.iter().any(|s| s.hwnd == front) {
                p.previous = front;
            }
        }
        p.open = Some(id);
        p.rect = rect;
        p.opened_at = Instant::now();
        p.been_front = false;
        p.mouse_was_down = geo::any_button_down();
        p.serial
    };
    hooks.on_open(dark, cx);
    // the rectangle again: opening may have changed what the panel wants to show (its height)
    let rect = geo::place(glass, hooks.size(cx), scale, side, work);
    cx.global_mut::<PanelHost>().rect = rect;
    // show, bring to the front, then glass (in this order: the engine attaches to a window that is on the screen)
    let h = hooks.clone();
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        geo::show_at(hwnd, rect);
        // (the quiet kind: no tap of the Alt key into the window that is in front; the click that opened the panel was ours)
        rf_win::window::bring_to_front_quietly(hwnd);
        cx.update(|cx| {
            h.on_shown(dark, cx);
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
    let Some(p) = cx.try_global::<PanelHost>() else { return };
    if p.open != Some(id) {
        return;
    }
    let Some(slot) = p.slot(id) else { return };
    let (kind, side, hwnd) = (slot.kind, slot.side, slot.hwnd);
    let Some((scale, work)) = host::monitor_work(cx) else { return };
    let Some(glass) = host::visible_glass_rect(kind, cx) else { return };
    let rect = geo::place(glass, size, scale, side, work);
    if cx.global::<PanelHost>().rect == rect {
        return;
    }
    cx.global_mut::<PanelHost>().rect = rect;
    cx.spawn(async move |_| {
        geo::show_at(hwnd, rect);
    })
    .detach();
}

/// One look of the open panel at the foreground and the mouse. False when this watch is over (the panel closed or reopened).
fn watch_once(serial: u64, cx: &mut App) -> bool {
    let Some(p) = cx.try_global::<PanelHost>() else { return false };
    let Some(open) = p.open.filter(|_| p.serial == serial) else { return false };
    let Some(slot) = p.slot(open) else { return false };
    let (hwnd, kind) = (slot.hwnd, slot.kind);
    let (rect, opened_at, been_front, was_down) = (p.rect, p.opened_at, p.been_front, p.mouse_was_down);
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
    let (previous, left) = (p.previous, p.left.clone());
    let animate = crate::settings::animations();
    {
        let p = cx.global_mut::<PanelHost>();
        p.open = None;
        p.serial += 1;
        p.watcher = None;
        p.typed.clear();
    }
    if let Some(left) = left {
        left.update(cx, |_, cx| cx.notify());
    }
    hooks.on_close(cx);
    let restore = cause.restores_foreground();
    cx.spawn(async move |cx| {
        if animate {
            cx.background_executor().timer(Duration::from_millis(u64::from(geo::ANIM_MS) + 20)).await;
        }
        // opened again meanwhile (this panel itself): leave everything as it is. Another panel that opened has put this one away.
        let again = cx.update(|cx| panel(cx).is_some_and(|p| p.open == Some(id)));
        if again {
            return;
        }
        let fg = rf_win::dock::foreground_root();
        geo::hide(hwnd);
        cx.update(|cx| hooks.on_hidden(cx));
        if restore && let Some(target) = geo::restore_to(previous, fg, hwnd, geo::exists(previous)) {
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
    hooks.on_close(cx);
    geo::hide(hwnd);
    hooks.on_hidden(cx);
}

/// The dock ends: the panels' windows go with it.
pub fn shutdown(cx: &mut App) {
    // the pop-ups above the icons (made when first needed) go with the dock
    super::app_menu::shutdown(cx);
    super::hover_preview::shutdown(cx);
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
