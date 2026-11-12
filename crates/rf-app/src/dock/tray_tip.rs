//! The tooltip of a tray icon (shell plan increment D3): resting the pointer on an icon of the strip or of the overflow panel for 400 ms
//! opens a small glass pop-up above it with the program's tip. Built like the hover previews' window (`hover_preview.rs`), and for the same
//! reasons: it is NEVER ACTIVATED and never takes the keyboard (an island-styled tool window, shown with `SWP_NOACTIVATE`, never brought
//! to the front), its window is made when the first tip is due and kept hidden afterwards, and while it is open one 100 ms look reads the
//! real pointer (the toolkit's hover events do not report a pointer that leaves a window).
//!
//! THE TIMING is `rf_win::dock::tray_tip::TipMachine` (pure, tested). A press on an icon (which also covers the menu the program opens
//! from the click: the tip stays shut until the pointer has left the icon), a drag, and the keyboard being in the strip close the tip and
//! keep it from opening (`tray_strip::sync_tip_block`). Nothing runs while no tip is due or open.
//!
//! THE TEXT is the program's tip, cleaned (`rf_win::dock::tray_tip::clean`); an icon with no tip has no tooltip. Its picture is none: the
//! tip is text only.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::*;
use rf_win::dock::island;
use rf_win::dock::layout::Rect;
use rf_win::dock::panel as geo;
use rf_win::dock::tray_tip::{self as rules, TipInput, TipMachine, TipOutput};

use super::backdrop::IslandGlass;
use super::host;
use super::popup_win;
use super::tray_source::IconKey;
use crate::glass_watch::{self, PopupGlass};

/// How often an open tip looks at the pointer.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// What a tip is for: the icon, its cleaned text, and where the icon is on the screen (device px).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub key: IconKey,
    pub text: String,
    pub rect: Rect,
    /// The top edge the tip must stay clear of (device px): for an icon of the strip the island's glass top, which is higher than the
    /// icon's cell; for a cell of the overflow panel the cell's own top.
    pub clear_top: i32,
}

struct Made {
    handle: WindowHandle<TipView>,
    hwnd: isize,
}

struct TipHost {
    machine: TipMachine<IconKey>,
    /// The icon the pointer was last on.
    anchor: Option<Anchor>,
    /// What the open tip shows.
    shown_for: Option<Anchor>,
    made: Option<Made>,
    t0: Instant,
    timer: Option<Task<()>>,
    looker: Option<Task<()>>,
    shown: bool,
    /// Bumped at every show and hide: a task of an earlier state sees that and does nothing.
    generation: u64,
}

impl Global for TipHost {}

fn host_g(cx: &mut App) -> &mut TipHost {
    if !cx.has_global::<TipHost>() {
        cx.set_global(TipHost { machine: TipMachine::new(), anchor: None, shown_for: None, made: None, t0: Instant::now(), timer: None, looker: None, shown: false, generation: 0 });
    }
    cx.global_mut::<TipHost>()
}

/// The pointer is on an icon with a tip (`Some`), or on nothing a tip belongs to (`None`).
pub fn hover(anchor: Option<Anchor>, cx: &mut App) {
    let input = match anchor {
        Some(a) => {
            let key = a.key;
            host_g(cx).anchor = Some(a);
            TipInput::Icon(Some(key))
        }
        None => TipInput::Icon(None),
    };
    feed(input, cx);
}

/// What the icon the pointer is on says now (its tip or its place changed): an open tip for it is shown again, one for an icon that is
/// gone is closed.
pub fn update(anchor: Option<Anchor>, cx: &mut App) {
    let Some(h) = cx.try_global::<TipHost>() else { return };
    let Some(open) = h.machine.shown().copied() else { return };
    match anchor {
        Some(a) if a.key == open => {
            let same = h.shown_for.as_ref() == Some(&a);
            host_g(cx).anchor = Some(a);
            if !same {
                show_now(open, Instant::now(), cx);
            }
        }
        _ => dismiss(cx),
    }
}

/// A button was pressed on an icon: no tip until the pointer has left it.
pub fn press(cx: &mut App) {
    if cx.has_global::<TipHost>() {
        feed(TipInput::Press, cx);
    }
}

/// Something forbids a tip (`true`: a drag, the keyboard in the strip) or no longer does.
pub fn block(on: bool, cx: &mut App) {
    if on || cx.has_global::<TipHost>() {
        feed(TipInput::Block(on), cx);
    }
}

/// Closes the tip now (the panel it was above is going).
pub fn dismiss(cx: &mut App) {
    if !cx.has_global::<TipHost>() {
        return;
    }
    let out = cx.global_mut::<TipHost>().machine.dismiss();
    act(out, cx);
    rearm(cx);
}

/// The toolkit's id of the window, when it has been made.
pub fn window_ids(cx: &App) -> Vec<WindowId> {
    cx.try_global::<TipHost>().and_then(|h| h.made.as_ref().map(|m| m.handle.window_id())).into_iter().collect()
}

/// The dock ends: the window goes with it.
pub fn shutdown(cx: &mut App) {
    if !cx.has_global::<TipHost>() {
        return;
    }
    let host = cx.remove_global::<TipHost>();
    if let Some(m) = host.made {
        let _ = m.handle.update(cx, |v, window, cx| {
            v.on_hidden(cx);
            window.remove_window();
        });
    }
}

fn feed(input: TipInput<IconKey>, cx: &mut App) {
    let host = host_g(cx);
    let now = host.t0.elapsed().as_millis() as u64;
    let out = host.machine.input(now, input);
    act(out, cx);
    rearm(cx);
}

fn act(out: TipOutput<IconKey>, cx: &mut App) {
    match out {
        TipOutput::Show(key) | TipOutput::Swap(key) => {
            let started = Instant::now();
            // (a window is made, shown and styled from the message loop, not from the hover event that asked)
            cx.spawn(async move |cx| {
                cx.update(|cx| show_now(key, started, cx));
            })
            .detach();
        }
        TipOutput::Hide => {
            cx.spawn(async move |cx| {
                // Hiding makes Windows send the window messages at once, and the toolkit answers them by updating the app: the call is
                // made here, from the message loop and outside any update (as the hover preview's is).
                let Some((handle, hwnd)) = cx.update(hide_begin) else { return };
                geo::hide(hwnd);
                cx.update(|cx| {
                    let _ = handle.update(cx, |v, _, cx| v.on_hidden(cx));
                });
            })
            .detach();
        }
        TipOutput::Nothing => {}
    }
}

/// Sleeps until the machine's next deadline, if it has one. A new input replaces the sleep.
fn rearm(cx: &mut App) {
    let (due, now) = {
        let host = host_g(cx);
        (host.machine.deadline(), host.t0.elapsed().as_millis() as u64)
    };
    let timer = due.map(|due| {
        let wait = due.saturating_sub(now);
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            cx.update(tick);
        })
    });
    host_g(cx).timer = timer;
}

fn tick(cx: &mut App) {
    let Some(host) = cx.try_global::<TipHost>() else { return };
    let now = host.t0.elapsed().as_millis() as u64;
    let out = cx.global_mut::<TipHost>().machine.tick(now);
    act(out, cx);
    rearm(cx);
}

fn on_window_close(cx: &mut App) {
    dismiss(cx);
}

/// The window, made now if it was not. `None` when it cannot be made.
fn ensure_window(island_glass: &Rc<RefCell<IslandGlass>>, cx: &mut App) -> Option<(WindowHandle<TipView>, isize, bool)> {
    if let Some(m) = cx.try_global::<TipHost>().and_then(|h| h.made.as_ref()) {
        return Some((m.handle, m.hwnd, false));
    }
    let (scale, _, glass) = popup_win::anchor_basis(cx)?;
    let display = cx.primary_display().map(|d| d.id());
    let isl = island_glass.clone();
    let initial = rules::size("");
    let handle = popup_win::open_hidden(initial, display, glass, scale, on_window_close, move |_, _| TipView::new(isl), cx)?;
    let hwnd = handle
        .update(cx, |v, window, _| {
            let h = crate::look::window::hwnd_of(window);
            v.hwnd = h;
            h
        })
        .ok()
        .filter(|h| *h != 0)?;
    // never activated: a tool window with no activation on a click (the same styling as the islands)
    if let Err(e) = island::make_island(hwnd) {
        log::warn!("tray: the tip window could not be styled: {e:#}");
    }
    host_g(cx).made = Some(Made { handle, hwnd });
    Some((handle, hwnd, true))
}

/// Shows the tip of icon `key` (or swaps to it).
fn show_now(key: IconKey, started: Instant, cx: &mut App) {
    let Some(h) = cx.try_global::<TipHost>() else { return };
    if h.machine.shown() != Some(&key) {
        // closed or swapped meanwhile
        return;
    }
    let Some(anchor) = h.anchor.clone().filter(|a| a.key == key) else { return };
    if host::is_fullscreen(cx) {
        dismiss(cx);
        return;
    }
    let Some((scale, work, _)) = popup_win::anchor_basis(cx) else { return };
    let Some(island_glass) = super::tray_strip::island_glass(cx) else { return };
    let Some((handle, hwnd, made_now)) = ensure_window(&island_glass, cx) else {
        log::warn!("tray: cannot make the tip window");
        return;
    };
    let size = rules::size(&anchor.text);
    let rect = rules::rect((anchor.rect.left + anchor.rect.right) / 2, anchor.clear_top.min(anchor.rect.top), anchor.rect.bottom, size, scale, work);
    let dark = island_glass.borrow().dark();
    let text = anchor.text.clone();
    let _ = handle.update(cx, |v, _, cx| v.set_text(text, cx));
    let generation = {
        let host = host_g(cx);
        host.generation += 1;
        host.shown = true;
        host.shown_for = Some(anchor.clone());
        host.generation
    };
    // show without activating (never brought to the front: the program in front keeps the keyboard), then the glass
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        geo::show_at(hwnd, rect);
        cx.update(|cx| {
            let _ = handle.update(cx, |v, _, cx| v.on_shown(dark, cx));
            log::debug!("tray: tip shown in {:?}{}", started.elapsed(), if made_now { "; window made for it" } else { "" });
            start_looking(generation, anchor.rect, cx);
        });
    })
    .detach();
}

/// The tip is going off the screen (unless it was opened again meanwhile): the host forgets it and gives the window to hide.
fn hide_begin(cx: &mut App) -> Option<(WindowHandle<TipView>, isize)> {
    let h = cx.try_global::<TipHost>()?;
    if h.machine.shown().is_some() || !h.shown {
        return None;
    }
    let made = h.made.as_ref()?;
    let (handle, hwnd) = (made.handle, made.hwnd);
    let h = cx.global_mut::<TipHost>();
    h.shown = false;
    h.shown_for = None;
    h.generation += 1;
    h.looker = None;
    Some((handle, hwnd))
}

fn start_looking(generation: u64, icon: Rect, cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(LOOK_EVERY).await;
            if !cx.update(|cx| look_once(generation, icon, cx)) {
                break;
            }
        }
    });
    let host = host_g(cx);
    if host.generation == generation {
        host.looker = Some(task);
    }
}

/// One look of the open tip: the dock hid, or the pointer left the icon without the toolkit saying so (the tip closes).
fn look_once(generation: u64, icon: Rect, cx: &mut App) -> bool {
    let Some(h) = cx.try_global::<TipHost>() else { return false };
    if !h.shown || h.generation != generation {
        return false;
    }
    if host::is_fullscreen(cx) {
        dismiss(cx);
        return false;
    }
    if geo::cursor().is_some_and(|c| rules::pointer_off(c, icon)) {
        feed(TipInput::Icon(None), cx);
        return cx.try_global::<TipHost>().is_some_and(|h| h.generation == generation && h.machine.shown().is_some());
    }
    true
}

// ---------------------------------------------------------------- the view

pub struct TipView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    shown: bool,
    text: SharedString,
}

impl TipView {
    fn new(island: Rc<RefCell<IslandGlass>>) -> Self {
        TipView { island, hwnd: 0, popup: None, _watch: None, shown: false, text: SharedString::default() }
    }

    fn set_text(&mut self, text: String, cx: &mut Context<Self>) {
        self.text = text.into();
        self.shown = true;
        cx.notify();
    }

    /// The window is on the screen: the glass goes on.
    fn on_shown(&mut self, dark: bool, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        cx.notify();
    }

    /// Hidden: the glass comes off and the text is let go.
    fn on_hidden(&mut self, cx: &mut Context<Self>) {
        self.popup = None;
        self._watch = None;
        self.shown = false;
        self.text = SharedString::default();
        cx.notify();
    }
}

impl Render for TipView {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = host::frame_palette(&self.island, window);
        let dark = pal.mix < 0.5;
        let fill = glass_watch::panel_fill(dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        div()
            .size_full()
            .bg(fill)
            .text_color(pal.text().hsla())
            .text_size(px(12.))
            .px(px(rules::PAD_X))
            .py(px(rules::PAD_Y))
            .child(div().w_full().line_height(px(rules::LINE_H)).line_clamp(rules::MAX_LINES).child(self.text.clone()))
            .into_any_element()
    }
}
