//! The switcher's window: a centred glass panel with one card per window. The positions of every card come from
//! `rf_win::switcher::layout` (one source for what GPUI draws and for where DWM draws the live pictures); what each key and click
//! does is `rf_win::switcher::session`. This file draws and forwards.
//!
//! THE PICTURES. DWM draws a live picture on top of everything in the window's client area, so a card is: a box for the picture (a
//! plain wash behind it, which shows at the sides when the window is not as wide as the box), and under it the app's small icon and
//! the title. A minimized window, a suspended app, or one DWM refuses has no live picture: its box shows the app's icon large.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::icons::icon_bgra_chain;
use rf_win::switcher::chord::{Chord, Effect};
use rf_win::switcher::layout::Layout;
use rf_win::dock::panel as geo;
use rf_win::switcher::session::{Session, Step};
use rf_win::switcher::thumbs::{Dwm, Picture, ThumbSet};
use rf_win::switcher::watch;

use super::{Closing, keys};
use crate::glass_watch::{self, PopupGlass};
use crate::t;
use crate::theme::Palette as ThemePalette;

thread_local! {
    /// The icons already drawn once, kept between two openings so that the next one does not wait for the shell again. A few dozen
    /// small bitmaps: memory only, no window, thread or timer. Emptied by [`forget_icons`].
    static ICONS: RefCell<HashMap<String, Arc<RenderImage>>> = RefCell::new(HashMap::new());
}

/// Most icons kept between openings; past this the cache starts over (a machine with this many apps open is not the common case).
const ICON_CACHE_MAX: usize = 96;

/// Lets go of the icons kept between openings.
pub fn forget_icons() {
    ICONS.with(|c| c.borrow_mut().clear());
}

fn cached_icon(key: &str) -> Option<Arc<RenderImage>> {
    ICONS.with(|c| c.borrow().get(key).cloned())
}

fn remember_icon(key: String, img: Arc<RenderImage>) {
    ICONS.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= ICON_CACHE_MAX {
            c.clear();
        }
        c.insert(key, img);
    });
}

/// The picture GPUI draws from the shell's bitmap (straight BGRA, as the dock's icons are).
fn to_image(b: rf_win::thumbs::Bgra) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(b.width, b.height, b.data)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// How often an open switcher looks at the foreground and the mouse (the same as the dock's panels).
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_millis(50);

pub struct SwitcherView {
    hwnd: isize,
    session: Session,
    layout: Layout,
    /// The chord for the keys this window itself receives (the switcher opened by a command has no Alt held: `Sticky`).
    chord: Chord,
    dark: bool,
    /// The window that was in front when the switcher opened.
    previous: isize,
    /// What each card shows, once the pictures are registered (before: `None`, a plain box).
    pictures: Vec<Option<Picture>>,
    thumbs: Option<ThumbSet<Dwm>>,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// The look at the foreground and the mouse every 50 ms while the switcher is open (it ends with the window).
    _guard: Option<Task<()>>,
    /// The keys come from the host's keyboard hook: only a click outside puts the window away on its own.
    hooked: bool,
    /// The side, in px, the icons are asked for (part of their key: another monitor's scale is another bitmap).
    icon_px: u32,
    icons: HashMap<String, Arc<RenderImage>>,
    _tasks: Vec<Task<()>>,
    focus: FocusHandle,
    /// The window has been the active one at least once (losing the activation closes it only after that).
    been_active: bool,
    /// Closing has been asked for; further keys and clicks do nothing.
    closing: bool,
    _subs: Vec<gpui_kit::Subscription>,
}

impl SwitcherView {
    pub fn new(session: Session, layout: Layout, previous: isize, dark: bool, hooked: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut chord = Chord::new();
        chord.open_by_command();
        // losing the activation (the user clicked another window, an app took the foreground) puts the switcher away, once it has
        // been the active window at least once
        let sub = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.been_active = true;
            } else if this.been_active {
                this.finish(Closing::LostForeground, cx);
            }
        });
        // Alt+F4 while it is open cancels; the window is not destroyed by the system
        window.on_window_should_close(cx, |_, cx| {
            cx.defer(|cx| super::close(Closing::Cancelled, cx));
            false
        });
        let count = session.items().len();
        let icon_px = layout.large_icon().max(32) as u32;
        SwitcherView {
            hwnd: crate::look::window::hwnd_of(window),
            session,
            layout,
            chord,
            dark,
            previous,
            pictures: vec![None; count],
            thumbs: None,
            popup: None,
            _watch: None,
            _guard: None,
            hooked,
            icon_px,
            icons: HashMap::new(),
            _tasks: Vec::new(),
            focus: cx.focus_handle(),
            been_active: false,
            closing: false,
            _subs: vec![sub],
        }
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    pub fn previous(&self) -> isize {
        self.previous
    }

    /// (cards, live pictures) for the log.
    pub fn counts(&self) -> (usize, usize) {
        (self.session.items().len(), self.pictures.iter().filter(|p| matches!(p, Some(Picture::Live(_)))).count())
    }

    /// The window is on the screen: the glass goes on, then the live pictures (after the glass, so that nothing of the engine can be
    /// above them), and the keyboard goes to the view. The icons are asked for.
    pub fn on_shown(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.popup = Some(PopupGlass::start_panel(self.hwnd, self.dark));
        self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        let cards: Vec<(isize, bool, rf_win::dock::Rect)> =
            self.session.items().iter().enumerate().map(|(i, it)| (it.hwnd, it.no_live_picture(), self.layout.picture(i))).collect();
        let mut set = ThumbSet::new(Dwm, self.hwnd);
        self.pictures = set.show(&cards).into_iter().map(Some).collect();
        self.thumbs = Some(set);
        window.focus(&self.focus, cx);
        self.load_icons(cx);
        self.start_guard(cx);
        cx.notify();
    }

    /// While the window is open: a look at the foreground and the mouse every 50 ms. A click outside puts it away; a window that
    /// the foreground was refused (or that lost it) goes after the same grace the dock's panels have, which is what keeps a
    /// switcher that never got the keyboard from sitting over the monitor for ever. (The decision is `watch::should_dismiss`.)
    fn start_guard(&mut self, cx: &mut Context<Self>) {
        let opened = std::time::Instant::now();
        let (hwnd, rect, hooked) = (self.hwnd, self.layout.window, self.hooked);
        self._guard = Some(cx.spawn(async move |this, cx| {
            let mut was_down = geo::any_button_down();
            let mut been_front = false;
            loop {
                cx.background_executor().timer(WATCH_EVERY).await;
                let foreground = rf_win::dock::foreground_root();
                been_front |= foreground == hwnd;
                let down = geo::any_button_down();
                let look = watch::Look {
                    hwnd,
                    foreground,
                    been_front,
                    since_open_ms: opened.elapsed().as_millis() as u64,
                    was_down,
                    down,
                    cursor: geo::cursor(),
                    rect,
                };
                was_down = down;
                if watch::should_dismiss(&look, hooked) {
                    let _ = this.update(cx, |this, cx| this.finish(Closing::LostForeground, cx));
                    break;
                }
                // (the window is gone: the task is dropped with the view, but a view that is gone ends the loop here too)
                if this.update(cx, |_, _| ()).is_err() {
                    break;
                }
            }
        }));
    }

    /// Asks for the icon of every distinct app that is not known yet, on worker threads (the shell answers them one at a time).
    fn load_icons(&mut self, cx: &mut Context<Self>) {
        let side = self.icon_px;
        let mut wanted: Vec<(String, bool, Vec<rf_win::dock::IconSource>)> = Vec::new();
        for item in self.session.items() {
            let key = item.icon_key(side);
            if self.icons.contains_key(&key) || wanted.iter().any(|(k, _, _)| *k == key) {
                continue;
            }
            // (a key that is a window's handle may mean another window at the next opening: those are not kept between openings)
            let stable = item.icon_key_is_stable();
            if let Some(img) = stable.then(|| cached_icon(&key)).flatten() {
                self.icons.insert(key, img);
                continue;
            }
            wanted.push((key, stable, item.icon_chain()));
        }
        for (key, stable, chain) in wanted {
            let task = cx.spawn(async move |this, cx| {
                let bgra = cx.background_executor().spawn(async move { icon_bgra_chain(&chain, side).ok() }).await;
                if let Some(img) = bgra.and_then(to_image) {
                    if stable {
                        remember_icon(key.clone(), img.clone());
                    }
                    let _ = this.update(cx, |this, cx| {
                        this.icons.insert(key, img);
                        cx.notify();
                    });
                }
            });
            self._tasks.push(task);
        }
    }

    /// Takes everything the switcher made off the screen and out of the system: the pictures are unregistered, the glass is let go,
    /// the mark is taken off the window. After this the window may be removed.
    pub fn teardown(&mut self, _cx: &mut Context<Self>) {
        self.thumbs = None;
        self.popup = None;
        self._watch = None;
        self._guard = None;
        self._tasks.clear();
        rf_win::switcher::unmark_shell_window(self.hwnd);
    }

    // ---------------------------------------------------------------- what happens

    /// An effect of the chord (from this window's own keys, or from the host's keyboard hook).
    pub fn apply(&mut self, effect: Effect, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        match self.session.apply(effect) {
            Step::Nothing => {}
            Step::Redraw => cx.notify(),
            Step::Choose(hwnd) => self.finish(Closing::Chosen(hwnd), cx),
            Step::Cancel => self.finish(Closing::Cancelled, cx),
        }
    }

    fn finish(&mut self, how: Closing, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.chord.closed_elsewhere();
        cx.defer(move |cx| super::close(how, cx));
    }

    fn key_down(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        let reaction = self.chord.feed(keys::key_down(ev.keystroke.key.as_str(), m.shift, m.alt));
        if let Some(effect) = reaction.effect {
            self.apply(effect, cx);
        }
        if reaction.swallow {
            cx.stop_propagation();
        }
    }

    fn hover(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.closing && self.session.hover(i) == Step::Redraw {
            cx.notify();
        }
    }

    fn click(&mut self, i: usize, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if let Step::Choose(hwnd) = self.session.click(i) {
            self.finish(Closing::Chosen(hwnd), cx);
        }
    }
}

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

impl Render for SwitcherView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = if self.dark { ThemePalette::dark() } else { ThemePalette::light() };
        let fill = glass_watch::panel_fill(self.dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let scale = self.layout.scale;
        let lg = move |v: i32| px(v as f32 / scale);
        let wash = if self.dark { with_alpha(rgb(0xFFFFFF).into(), 0.07) } else { with_alpha(rgb(0x000000).into(), 0.06) };
        let letterbox = if self.dark { with_alpha(rgb(0x000000).into(), 0.28) } else { with_alpha(rgb(0x000000).into(), 0.10) };
        let selected = self.session.selected();
        let font = (12.0 * self.layout.factor).max(10.0);

        let mut root = div()
            .id("switcher")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(theme.text)
            .text_size(px(13.))
            .track_focus(&self.focus)
            .key_context("Switcher")
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key_down(ev, cx)));

        if self.session.is_empty() {
            return root
                .flex()
                .items_center()
                .justify_center()
                .child(div().text_color(theme.text_secondary).child(t!("Açık pencere yok")))
                .into_any_element();
        }

        for (i, item) in self.session.items().iter().enumerate() {
            let card = self.layout.card(i);
            let pic = self.layout.picture(i);
            let label = self.layout.label(i);
            let is_selected = selected == Some(i);
            let icon = self.icons.get(&item.icon_key(self.icon_px)).cloned();
            let live = matches!(self.pictures.get(i), Some(Some(Picture::Live(_))));
            let icon_side = self.layout.small_icon();
            let big = self.layout.large_icon();
            let title: SharedString = if item.title.trim().is_empty() { t!("(adsız)").into() } else { item.title.clone().into() };
            let picture_box = div()
                .absolute()
                .left(lg(pic.left - card.left))
                .top(lg(pic.top - card.top))
                .w(lg(pic.width()))
                .h(lg(pic.height()))
                .rounded(px(6.))
                .bg(letterbox)
                .flex()
                .items_center()
                .justify_center()
                // (a live picture is drawn over this box by DWM; the large icon is for the windows that have none)
                .when(!live, |d| d.when_some(icon.clone(), |d, img| d.child(gpui_kit::img(img).size(lg(big)))));
            let row = div()
                .absolute()
                .left(lg(label.left - card.left))
                .top(lg(label.top - card.top))
                .w(lg(label.width()))
                .h(lg(label.height()))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(div().size(lg(icon_side)).flex_shrink_0().when_some(icon, |d, img| d.child(gpui_kit::img(img).size(lg(icon_side)))))
                .child(div().flex_1().min_w_0().truncate().text_size(px(font)).child(title));
            root = root.child(
                div()
                    .id(("switcher-card", i))
                    .absolute()
                    .left(lg(card.left))
                    .top(lg(card.top))
                    .w(lg(card.width()))
                    .h(lg(card.height()))
                    .rounded(px(12.))
                    .bg(if is_selected { with_alpha(theme.accent, 0.22) } else { wash })
                    .cursor_pointer()
                    .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| this.hover(i, cx)))
                    .on_click(cx.listener(move |this, _, _, cx| this.click(i, cx)))
                    .child(picture_box)
                    .child(row)
                    // (the ring is a layer of its own: a border on the card would move its children, and the live pictures sit at
                    // the layout's positions)
                    .child(div().absolute().top_0().left_0().size_full().rounded(px(12.)).border_2().border_color(if is_selected { theme.accent } else { transparent_black() })),
            );
        }
        root.into_any_element()
    }
}
