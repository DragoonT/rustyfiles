//! The dock's hover previews (shell plan increment B8): resting the pointer on a RUNNING app's icon for about 400 ms opens a small glass
//! preview above the icon with one card per window of the app: a live picture of the window (DWM, through the switcher's `ThumbApi`,
//! with this pop-up as the destination), the app's large icon for a minimised window, the title, and on the card under the pointer a
//! small x. A click on a card brings that window to the front with the switcher's activation sequence; the x asks the window to close.
//!
//! THE TIMING is `rf_win::dock::preview::Machine` (pure, tested): this file feeds it the icons' hover events and the preview's own, carries
//! out what it answers, and sleeps until its next deadline (no polling for the timing). Moving from the icon into the preview keeps it
//! open, moving to a neighbouring running icon swaps it at once, leaving both closes it after 300 ms, a click on an icon closes it, a
//! right click replaces it with the menu (the menu blocks the machine while it is open), and it closes when the dock hides.
//!
//! IT IS NEVER ACTIVATED AND NEVER TAKES THE KEYBOARD. The dock's panels are activated (they need the keys); this is not a panel. Its
//! window is an island-styled one (`island::make_island`: a tool window with `WS_EX_NOACTIVATE` and an answer of `MA_NOACTIVATE` to
//! `WM_MOUSEACTIVATE`, so even a click on a card does not make it the active window), it is shown with `SWP_NOACTIVATE` and it is never
//! brought to the front: the program the user is typing in keeps the keyboard throughout. A click on a card is still input of OUR
//! process, which is what lets the activation sequence bring the chosen window forward.
//!
//! THE WINDOW is made when the first preview is due and kept hidden afterwards. While it is open one 100 ms look removes the cards of
//! windows that have closed (the x, or the program itself), closes the preview when the dock hides or the monitor changes, and reads the
//! real pointer: the toolkit's hover events do not report a pointer that leaves a window (Windows sends the window no last move), so
//! when the pointer is on neither the preview nor the dock's glass the look tells the machine that it left both and the 300 ms of grace
//! runs from there. Closed, nothing runs. There is no easing: DWM draws the pictures at full strength, so a fading card would show its
//! picture first.
//!
//! THE WINDOWS OF THE APP are held as `winops::WinRef` (handle and process), taken when the pointer comes onto the icon: a card is
//! drawn, activated or closed only while its handle still belongs to the process it was seen in.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::island;
use rf_win::dock::layout::Rect;
use rf_win::dock::panel as geo;
use rf_win::dock::preview::{self, Input, Layout, Machine, Output};
use rf_win::dock::winops::{self, WinRef};
use rf_win::dock::DockApp;
use rf_win::switcher::thumbs::{Dwm, Picture, ThumbSet, probe_size};

use super::backdrop::IslandGlass;
use super::host;
use super::popup_win;
use crate::glass_watch::{self, PopupGlass};
use crate::settings::Settings;

/// How often an open preview looks at its windows, the dock and the pointer. The pointer is read here because the toolkit does not
/// tell when it leaves a window (see `preview::pointer_away`), and the machine's 300 ms of grace should not be followed by a long wait
/// for the next look.
const LOOK_EVERY: Duration = Duration::from_millis(100);

/// What the middle island tells about the icon the pointer is on.
#[derive(Clone)]
pub struct IconInfo {
    pub app: DockApp,
    /// The app's picture for the big icon of a window with no live picture (48 px), if the island has it by now.
    pub icon: Option<Arc<RenderImage>>,
    /// The same for the title row's small icon (16 px): a picture kept near that size, not the big one reduced.
    pub icon_small: Option<Arc<RenderImage>>,
    /// The icon's centre, device px, screen coordinates.
    pub anchor_x: f32,
    /// The middle island's glass (its tone is the preview's).
    pub glass: Rc<RefCell<IslandGlass>>,
}

// ---------------------------------------------------------------- the host side

struct Made {
    handle: WindowHandle<PreviewView>,
    hwnd: isize,
}

struct PreviewHost {
    machine: Machine<String>,
    /// The icon the pointer was last on (what an opening is made from).
    info: Option<IconInfo>,
    /// The windows of that app as seen when the pointer came onto its icon (handle and process; see `winops::WinRef`). The cards, the
    /// activation and the close request work from these, so a handle that was reused by another program is never acted on.
    refs: Vec<WinRef>,
    /// Where the open preview is on the screen (device px).
    rect: Rect,
    made: Option<Made>,
    /// Clock of the machine.
    t0: Instant,
    timer: Option<Task<()>>,
    looker: Option<Task<()>>,
    /// The window is on the screen.
    shown: bool,
    /// Bumped at every show and hide: a task of an earlier state sees that and does nothing.
    generation: u64,
    basis: Option<(f32, Rect, i32)>,
}

impl Global for PreviewHost {}

fn host_g(cx: &mut App) -> &mut PreviewHost {
    if !cx.has_global::<PreviewHost>() {
        cx.set_global(PreviewHost {
            machine: Machine::new(),
            info: None,
            refs: Vec::new(),
            rect: Rect::default(),
            made: None,
            t0: Instant::now(),
            timer: None,
            looker: None,
            shown: false,
            generation: 0,
            basis: None,
        });
    }
    cx.global_mut::<PreviewHost>()
}

fn enabled(cx: &App) -> bool {
    cx.try_global::<Settings>().is_none_or(|s| s.dock_hover_previews)
}

/// The pointer is on the icon of a running app (`Some`), or on nothing a preview belongs to (`None`).
pub fn icon_hover(info: Option<IconInfo>, cx: &mut App) {
    if !enabled(cx) {
        return;
    }
    let input = match info {
        Some(i) => {
            let key = i.app.key.clone();
            let refs = winops::refs(&i.app.windows);
            let host = host_g(cx);
            host.refs = refs;
            host.info = Some(i);
            Input::Icon(Some(key))
        }
        None => Input::Icon(None),
    };
    feed(input, cx);
}

/// A button was pressed on an icon.
pub fn clicked(cx: &mut App) {
    if cx.has_global::<PreviewHost>() {
        feed(Input::Click, cx);
    }
}

/// A menu is open (`true`) or no longer is: no preview meanwhile.
pub fn block(on: bool, cx: &mut App) {
    if on || cx.has_global::<PreviewHost>() {
        feed(Input::Block(on), cx);
    }
}

/// The pointer entered or left the preview window.
fn preview_hover(inside: bool, cx: &mut App) {
    feed(Input::Preview(inside), cx);
}

/// The toolkit's id of the window, when it has been made.
pub fn window_ids(cx: &App) -> Vec<WindowId> {
    cx.try_global::<PreviewHost>().and_then(|h| h.made.as_ref().map(|m| m.handle.window_id())).into_iter().collect()
}

/// The dock ends: the window goes with it.
pub fn shutdown(cx: &mut App) {
    if !cx.has_global::<PreviewHost>() {
        return;
    }
    let host = cx.remove_global::<PreviewHost>();
    if let Some(m) = host.made {
        let _ = m.handle.update(cx, |v, window, cx| {
            v.on_hidden(window, cx);
            window.remove_window();
        });
    }
}

fn feed(input: Input<String>, cx: &mut App) {
    let host = host_g(cx);
    let now = host.t0.elapsed().as_millis() as u64;
    let out = host.machine.input(now, input);
    act(out, cx);
    rearm(cx);
}

fn act(out: Output<String>, cx: &mut App) {
    match out {
        Output::Open(key) | Output::Swap(key) => {
            let started = Instant::now();
            // (a window is made, shown and styled from the message loop, not from the hover event that asked)
            cx.spawn(async move |cx| {
                cx.update(|cx| show_now(key, started, cx));
            })
            .detach();
        }
        Output::Close => {
            cx.spawn(async move |cx| {
                // Hiding makes Windows send the window messages at once, and the toolkit answers them by updating the app: the call is
                // made here, from the message loop and outside any update, as the panels do. Made inside an update it left "RefCell
                // already borrowed" errors in the log and the toolkit's idea of the window out of step (seen live).
                let Some((handle, hwnd)) = cx.update(hide_begin) else { return };
                geo::hide(hwnd);
                cx.update(|cx| {
                    let _ = handle.update(cx, |v, window, cx| v.on_hidden(window, cx));
                });
            })
            .detach();
        }
        Output::Nothing => {}
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
    let Some(host) = cx.try_global::<PreviewHost>() else { return };
    let now = host.t0.elapsed().as_millis() as u64;
    let out = cx.global_mut::<PreviewHost>().machine.tick(now);
    act(out, cx);
    rearm(cx);
}

/// Closes the preview now, whatever the pointer is doing.
fn dismiss(cx: &mut App) {
    if !cx.has_global::<PreviewHost>() {
        return;
    }
    let out = cx.global_mut::<PreviewHost>().machine.dismiss();
    act(out, cx);
    rearm(cx);
}

fn on_window_close(cx: &mut App) {
    dismiss(cx);
}

/// The window, made now if it was not. `None` when it cannot be made.
fn ensure_window(island: &Rc<RefCell<IslandGlass>>, cx: &mut App) -> Option<(WindowHandle<PreviewView>, isize, bool)> {
    if let Some(m) = cx.try_global::<PreviewHost>().and_then(|h| h.made.as_ref()) {
        return Some((m.handle, m.hwnd, false));
    }
    let (scale, _, glass) = popup_win::anchor_basis(cx)?;
    let display = cx.primary_display().map(|d| d.id());
    let isl = island.clone();
    let handle = popup_win::open_hidden(preview::layout(&[None], 1000.0).size(), display, glass, scale, on_window_close, move |_, _| PreviewView::new(isl), cx)?;
    let hwnd = handle.update(cx, |v, window, _| {
        let h = crate::look::window::hwnd_of(window);
        v.hwnd = h;
        h
    }).ok().filter(|h| *h != 0)?;
    // never activated: a tool window with no activation on a click (the same styling as the islands)
    if let Err(e) = island::make_island(hwnd) {
        log::warn!("dock: the preview window could not be styled: {e:#}");
    }
    host_g(cx).made = Some(Made { handle, hwnd });
    Some((handle, hwnd, true))
}

/// Shows the preview of the app `key` (or swaps to it): its cards, then the window, then the glass and the live pictures.
fn show_now(key: String, started: Instant, cx: &mut App) {
    let Some(h) = cx.try_global::<PreviewHost>() else { return };
    if h.machine.shown() != Some(&key) {
        // closed or swapped meanwhile
        return;
    }
    let Some(info) = h.info.clone().filter(|i| i.app.key == key) else { return };
    if host::is_fullscreen(cx) {
        dismiss(cx);
        return;
    }
    let Some((scale, work, glass)) = popup_win::anchor_basis(cx) else { return };
    let Some((handle, hwnd, made_now)) = ensure_window(&info.glass, cx) else {
        log::warn!("dock: cannot make the hover preview's window");
        return;
    };
    let alive: Vec<WinRef> = cx.try_global::<PreviewHost>().map(|h| h.refs.iter().copied().filter(|w| w.is_still()).collect()).unwrap_or_default();
    if alive.is_empty() {
        dismiss(cx);
        return;
    }
    let grid = preview::grid(alive.len(), work.width() as f32 / scale);
    let cards: Vec<Card> = alive.iter().take(grid.shown).map(|&w| Card { win: w, title: winops::window_title(w.hwnd), minimized: winops::is_minimized(w.hwnd) }).collect();
    // The card's height follows the window's shape, so the shapes are asked BEFORE the window is sized: each source is registered to the
    // (hidden or shown) preview window for a moment, asked and given back (`probe_size`); the windows that get no card stay unknown.
    let probe_started = Instant::now();
    let mut dwm = Dwm;
    let mut sources: Vec<Option<(i32, i32)>> = cards.iter().map(|c| probe_size(&mut dwm, hwnd, c.win.hwnd, c.minimized)).collect();
    sources.resize(alive.len(), None);
    let unknown = sources.iter().take(cards.len()).filter(|s| s.is_none()).count();
    let probe_took = probe_started.elapsed();
    // Never taller than the room above the dock: a layout that would not fit has its boxes capped (the rows are not cut off).
    let layout = preview::layout_in(&sources, work.width() as f32 / scale, preview::room_above(glass.top, scale, super::look::dock_size(cx), work));
    let size = layout.size();
    let rect = preview::above_icon(info.anchor_x.round() as i32, glass.top, size, scale, super::look::dock_size(cx), work);
    let dark = info.glass.borrow().dark();
    let (count, hidden, anchor) = (cards.len(), grid.hidden, info.anchor_x.round() as i32);
    let _ = handle.update(cx, |v, _, cx| v.set_cards(cards, layout, info.icon.clone(), info.icon_small.clone(), scale, cx));
    let generation = {
        let host = host_g(cx);
        host.generation += 1;
        host.shown = true;
        host.basis = Some((scale, work, glass.top));
        host.rect = rect;
        host.generation
    };
    // show without activating (never brought to the front: the program in front keeps the keyboard), then glass and pictures
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        geo::show_at(hwnd, rect);
        cx.update(|cx| {
            let live = handle.update(cx, |v, _, cx| v.on_shown(dark, cx)).unwrap_or(0);
            log::info!(
                "dock: hover preview shown in {:?} at x={anchor} ({count} cards, {live} live pictures, shapes asked in {probe_took:?}, {unknown} of unknown shape{}{})",
                started.elapsed(),
                if hidden > 0 { format!(", {hidden} windows not shown") } else { String::new() },
                if made_now { "; window made for it" } else { "" }
            );
            start_looking(generation, cx);
        });
    })
    .detach();
}

/// The preview is going off the screen (unless it was opened again meanwhile): the host forgets it and gives the window to hide. The
/// caller hides it outside an update and then tells the view.
fn hide_begin(cx: &mut App) -> Option<(WindowHandle<PreviewView>, isize)> {
    let h = cx.try_global::<PreviewHost>()?;
    if h.machine.shown().is_some() || !h.shown {
        return None;
    }
    let made = h.made.as_ref()?;
    let (handle, hwnd) = (made.handle, made.hwnd);
    let h = cx.global_mut::<PreviewHost>();
    h.shown = false;
    h.generation += 1;
    h.looker = None;
    Some((handle, hwnd))
}

fn start_looking(generation: u64, cx: &mut App) {
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(LOOK_EVERY).await;
            if !cx.update(|cx| look_once(generation, cx)) {
                break;
            }
        }
    });
    let host = host_g(cx);
    if host.generation == generation {
        host.looker = Some(task);
    }
}

/// One look of the open preview: the dock hid or the screen changed (close it); the pointer left both the preview and the dock without
/// the toolkit saying so (the machine's grace starts); a window has closed (its card goes).
fn look_once(generation: u64, cx: &mut App) -> bool {
    let Some(h) = cx.try_global::<PreviewHost>() else { return false };
    if !h.shown || h.generation != generation {
        return false;
    }
    let then = h.basis;
    let key = h.machine.shown().cloned();
    let rect = h.rect;
    let now = popup_win::anchor_basis(cx).map(|(s, w, g)| (s, w, g.top));
    // (the setting switched off while it is open: the pointer's events are not followed any more, so it must not stay)
    if host::is_fullscreen(cx) || now != then || !enabled(cx) {
        dismiss(cx);
        return false;
    }
    let Some(key) = key else { return false };
    // The pointer. Windows tells a window that the pointer left it but sends no last move, so a pointer that goes from a card out of
    // the preview, or from an icon up out of the dock's window, is never reported by the toolkit's hover events. Read it here; the
    // machine ignores a repeat of what it already knows.
    if let (Some(cursor), Some((scale, _, glass))) = (geo::cursor(), popup_win::anchor_basis(cx))
        && preview::pointer_away(cursor, rect, glass, scale, super::look::dock_size(cx))
    {
        feed(Input::Icon(None), cx);
        feed(Input::Preview(false), cx);
        if cx.try_global::<PreviewHost>().is_none_or(|h| h.generation != generation || h.machine.shown().is_none()) {
            return false;
        }
    }
    // the windows that are gone: the list shrinks and the preview is made again from it
    let host = host_g(cx);
    let before = host.refs.len();
    host.refs.retain(|w| w.is_still());
    if host.refs.len() == before {
        return true;
    }
    if host.refs.is_empty() {
        dismiss(cx);
        return false;
    }
    show_now(key, Instant::now(), cx);
    false
}

/// A card was clicked: its window is brought forward with the switcher's activation sequence (it may wait up to a quarter of a second
/// for a minimised window to restore, so not on the UI thread), and the preview goes.
fn activate_window(win: WinRef, cx: &mut App) {
    // (the window the card showed is gone, or its number belongs to another program now: nothing is brought forward)
    if !win.is_still() {
        log::debug!("dock: the window of a preview card is gone");
        feed(Input::Click, cx);
        return;
    }
    let hwnd = win.hwnd;
    std::thread::spawn(move || {
        use rf_win::switcher::activate::{self, Outcome, Win32Ops};
        match activate::activate(&mut Win32Ops, hwnd) {
            Outcome::Done => {}
            Outcome::Gone => log::debug!("dock: the window of a preview card is gone"),
            Outcome::Flashed => log::info!("dock: the window would not come forward; its button flashes"),
        }
    });
    feed(Input::Click, cx);
}

/// The x of a card: the window is asked to close. Its card goes when the window has (the next look).
fn close_window(win: WinRef) {
    if !winops::close_window(win) {
        log::debug!("dock: a window could not be asked to close");
    }
}

// ---------------------------------------------------------------- the view

#[derive(Clone, Debug)]
struct Card {
    win: WinRef,
    title: String,
    minimized: bool,
}

pub struct PreviewView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    shown: bool,
    cards: Vec<Card>,
    /// Where everything of every card is: the drawing and the live pictures both take their rectangles from here.
    layout: Layout,
    icon: Option<Arc<RenderImage>>,
    icon_small: Option<Arc<RenderImage>>,
    scale: f32,
    thumbs: Option<ThumbSet<Dwm>>,
    pictures: Vec<Picture>,
    hover: Option<usize>,
}

impl PreviewView {
    fn new(island: Rc<RefCell<IslandGlass>>) -> Self {
        PreviewView {
            island,
            hwnd: 0,
            popup: None,
            _watch: None,
            shown: false,
            cards: Vec::new(),
            layout: preview::layout(&[], 1000.0),
            icon: None,
            icon_small: None,
            scale: 1.0,
            thumbs: None,
            pictures: Vec::new(),
            hover: None,
        }
    }

    /// New cards: whatever pictures the last ones had are let go first (DWM thumbnails are unregistered before the window moves or
    /// changes size, and registered again once it has).
    fn set_cards(&mut self, cards: Vec<Card>, layout: Layout, icon: Option<Arc<RenderImage>>, icon_small: Option<Arc<RenderImage>>, scale: f32, cx: &mut Context<Self>) {
        self.thumbs = None;
        self.pictures.clear();
        self.cards = cards;
        self.layout = layout;
        self.icon = icon;
        self.icon_small = icon_small;
        self.scale = scale;
        self.hover = None;
        self.shown = true;
        cx.notify();
    }

    /// The window is on the screen: the glass goes on, then the live pictures (after the glass, so that nothing of the engine can be
    /// above them). Returns how many cards have a live picture.
    fn on_shown(&mut self, dark: bool, cx: &mut Context<Self>) -> usize {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel_thin_rim(self.hwnd, dark, rim_cap()));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        // THE SAME BOX the card is painted with (`Layout::card(i).picture`), in device px. DWM measures its destination from the window's
        // own top left corner, and the client area of this window starts after the system's invisible frame (8 px at 100 % on the left,
        // none at the top): the pictures were drawn that far left of their boxes. The origin is read now, with the window on the screen.
        let origin = thumb_origin(self.hwnd);
        let boxes: Vec<(isize, bool, Rect)> =
            self.cards.iter().enumerate().map(|(i, c)| (c.win.hwnd, c.minimized, preview::to_device(self.layout.card(i).picture, self.scale))).collect();
        if let Some(first) = boxes.first() {
            log::info!(
                "dock: hover preview pictures: {} boxes, the first {}x{} at ({}, {}) in client px, client area starts at {:?} in the window",
                boxes.len(),
                first.2.width(),
                first.2.height(),
                first.2.left,
                first.2.top,
                origin
            );
        }
        let mut set = ThumbSet::new(Dwm, self.hwnd).with_origin(origin);
        self.pictures = set.show(&boxes);
        self.thumbs = Some(set);
        cx.notify();
        self.pictures.iter().filter(|p| matches!(p, Picture::Live(_))).count()
    }

    /// Hidden: the pictures are unregistered and the glass comes off.
    fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.thumbs = None;
        self.pictures.clear();
        self.popup = None;
        self._watch = None;
        self.cards.clear();
        self.hover = None;
        self.shown = false;
        self.icon = None;
        self.icon_small = None;
        cx.notify();
    }

    fn hover_card(&mut self, i: usize, on: bool, cx: &mut Context<Self>) {
        if on {
            if self.hover != Some(i) {
                self.hover = Some(i);
                cx.notify();
            }
        } else if self.hover == Some(i) {
            self.hover = None;
            cx.notify();
        }
    }
}

/// How wide the glass's bent rim band may be, CSS px. The window's own is a third of its short side (up to 44 px), which under 4 px of
/// padding read as a thick border; this is the thin edge. Tune it here.
const RIM_CAP: f32 = 8.0;

/// [`RIM_CAP`], or `RUSTYFILES_PREVIEW_RIM=<css px>` (1 to 44) to compare edges on the screen without a rebuild.
fn rim_cap() -> f32 {
    std::env::var("RUSTYFILES_PREVIEW_RIM").ok().and_then(|v| v.trim().parse::<f32>().ok()).filter(|v| (1.0..=44.0).contains(v)).unwrap_or(RIM_CAP)
}

/// Where DWM measures a thumbnail's destination from, relative to the client area: the client area's offset inside the window (the
/// system's invisible frame). `RUSTYFILES_PREVIEW_THUMB_ORIGIN=0` sends the client rectangles as they are (to compare on the screen).
fn thumb_origin(hwnd: isize) -> (i32, i32) {
    if std::env::var("RUSTYFILES_PREVIEW_THUMB_ORIGIN").is_ok_and(|v| v == "0") {
        return (0, 0);
    }
    geo::client_origin_in_window(hwnd)
}

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown || self.cards.is_empty() {
            return div().size_full().into_any_element();
        }
        let (pal, _) = host::frame_palette(&self.island, window);
        let dark = pal.mix < 0.5;
        let fill = glass_watch::panel_fill(dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let (card_fill, card_hot, secondary, text) = (pal.field_fill().hsla(), pal.hover_fill().hsla(), pal.text_secondary().hsla(), pal.text().hsla());
        let letterbox = if dark { with_alpha(rgb(0x000000).into(), 0.28) } else { with_alpha(rgb(0x000000).into(), 0.10) };
        let mut root = div()
            .id("hover-preview")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(text)
            .text_size(px(12.))
            .on_hover(cx.listener(|_, on: &bool, _, cx| preview_hover(*on, cx)));
        // a thin, light frame on every card (drawn as a child over the card's fill, not as a border of the card: a border would move the
        // card's absolutely placed children in by its width)
        let frame = if dark { with_alpha(rgb(0xFFFFFF).into(), 0.09) } else { with_alpha(rgb(0x000000).into(), 0.10) };
        for (i, card) in self.cards.iter().enumerate() {
            // every rectangle of the card comes from the layout, the same ones the live pictures are placed from
            let b = self.layout.card(i);
            let [x, y, w, h] = b.card;
            let live = matches!(self.pictures.get(i), Some(Picture::Live(_)));
            let hot = self.hover == Some(i);
            let title: SharedString = if card.title.trim().is_empty() { crate::t!("(adsız)").into() } else { card.title.clone().into() };
            let win = card.win;
            // the picture box: DWM draws the live window over it; a window with no live picture shows the app's large icon in it
            let picture_box = div()
                .absolute()
                .left(px(b.picture_in_card[0]))
                .top(px(b.picture_in_card[1]))
                .w(px(b.picture_in_card[2]))
                .h(px(b.picture_in_card[3]))
                .rounded(px(4.))
                .bg(letterbox)
                .flex()
                .items_center()
                .justify_center()
                .when(!live, |d| d.when_some(self.icon.clone(), |d, img| d.child(gpui_kit::img(img).size(px(48.)))));
            // the title row: the small icon, the title, and the x on the card under the pointer (not over the picture: DWM's picture is
            // drawn over everything of ours that lies on it)
            let row = div()
                .absolute()
                .left(px(b.title_in_card[0]))
                .top(px(b.title_in_card[1]))
                .w(px(b.title_in_card[2]))
                .h(px(b.title_in_card[3]))
                .px(px(3.))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(px(16.)).flex_shrink_0().when_some(self.icon_small.clone(), |d, img| d.child(gpui_kit::img(img).size(px(16.)))))
                .child(div().flex_1().min_w_0().truncate().child(title))
                .when(hot, |d| {
                    d.child(
                        div()
                            .id(("pv-close", i))
                            .size(px(preview::CLOSE_SIZE))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .cursor_pointer()
                            .hover(move |s| s.bg(card_hot))
                            .child(Icon::new(L::X).size(px(12.)).text_color(secondary))
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                close_window(win);
                            })),
                    )
                });
            root = root.child(
                div()
                    .id(("pv-card", i))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(w))
                    .h(px(h))
                    .rounded(px(7.))
                    .bg(if hot { card_hot } else { card_fill })
                    .cursor_pointer()
                    .on_hover(cx.listener(move |this, on: &bool, _, cx| this.hover_card(i, *on, cx)))
                    .on_click(cx.listener(move |_, _, _, cx| activate_window(win, cx)))
                    .child(div().absolute().top_0().left_0().size_full().rounded(px(7.)).border_1().border_color(frame))
                    .child(picture_box)
                    .child(row),
            );
        }
        root.into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::with_alpha;

    #[test]
    fn the_previews_are_on_by_default_and_a_file_from_before_them_reads_as_on() {
        assert!(crate::settings::Settings::default().dock_hover_previews);
        assert!(crate::settings::Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().dock_hover_previews, "a file from before it: on");
        let off = crate::settings::Settings { dock_hover_previews: false, ..Default::default() };
        let back = crate::settings::Settings::parse_lenient_pub(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_hover_previews, "off survives a save and a load");
        assert!(crate::settings::Settings::parse_lenient_pub("dock_hover_previews = \"nein\"\n").unwrap().dock_hover_previews, "a wrong value falls back to the default");
    }

    #[test]
    fn a_wash_keeps_its_colour_and_takes_the_opacity() {
        let c = with_alpha(gpui_kit::rgb(0x000000).into(), 0.28);
        assert!((c.a - 0.28).abs() < 1e-6);
        assert_eq!(c.l, 0.0);
    }
}
