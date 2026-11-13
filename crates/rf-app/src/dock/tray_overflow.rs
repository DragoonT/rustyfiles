//! The overflow panel of the tray strip: what opens above the right island when the strip's chevron is clicked (`panel.rs` opens, places
//! and closes it; `tray_panel.rs` is its slot). A small panel on HyperGlass holding the icons that did not fit the strip and the ones the
//! user moved here, in rows of up to six, each a 20 px picture in a 40 px cell.
//!
//! WHAT IT DOES. A click on a cell is the click on the icon (left, right, middle, double): it goes to the program through the tray host
//! and the panel puts itself away (a menu the program opens takes the foreground anyway). Dragging a cell out of the panel onto a gap of
//! the strip shows the icon again; dragging an icon of the strip onto the panel or onto the chevron hides it. The drag is followed by
//! `tray_strip.rs` (it reads the real pointer, because the toolkit's events stop at the window's edge), this view only draws the cells,
//! the dimmed icon that is being dragged and the frame that says "drop here". A tip after 400 ms of rest, as in the strip.
//!
//! THE KEYBOARD. The panel is activated like the other panels, so it has the keys: the arrows move a ring over the cells (down into a shorter
//! last row lands on its last cell), Tab and Shift+Tab walk on and wrap, Enter or Space is a left click, the context-menu key or Shift+F10
//! a right click, Escape closes. The cursor starts on nothing: the first arrow puts it on the first cell.
//!
//! NOTHING IS KEPT while it is hidden: the cells are drawn from `tray_strip`'s state every time, so there is nothing to scrub.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::panel::{self as geo, CloseCause};
use rf_win::dock::tray_strip as ts;

use super::backdrop::IslandGlass;
use super::panel_parts::Look;
use super::tray_source;
use super::tray_strip::{self, Source, TrayState};
use crate::glass_watch::{self, PopupGlass};

pub struct OverflowView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// On the screen or easing out (false while hidden).
    shown: bool,
    closing: bool,
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    /// The cell the keyboard is on, and the icon that is in it (the ring follows the icon when the cells change).
    cursor: Option<usize>,
    cursor_key: Option<tray_source::IconKey>,
    /// The size last given to the window (logical px).
    reported: (f32, f32),
    _obs: Subscription,
}

impl OverflowView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // the strip's state changed (an icon arrived, went, was dragged): the cells and the window's size follow
        // (a hidden window is not drawn again for it: nothing is notified while the panel is closed)
        let obs = cx.observe_global::<TrayState>(|this: &mut Self, cx| {
            if this.shown {
                this.refit(cx);
                cx.notify();
            }
        });
        OverflowView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            cursor: None,
            cursor_key: None,
            reported: tray_strip::overflow_size(cx),
            _obs: obs,
        }
    }

    /// The window's size now, logical px: the grid of the icons in it (wider rather than taller on a small screen), or the empty
    /// panel's text.
    pub fn size(&self, cx: &App) -> (f32, f32) {
        tray_strip::overflow_size(cx)
    }

    fn live(&self) -> bool {
        self.shown && !self.closing
    }

    /// The icons changed under an open panel: its window is placed again (it grows or shrinks upwards, still above the island).
    fn refit(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let count = tray_strip::overflow_data(cx).count;
        if let Some(c) = self.cursor {
            // the ring stays on its icon; when the icon is gone it stays at the nearest cell
            self.cursor = match self.cursor_key.and_then(|key| tray_strip::overflow_position(key, cx)) {
                Some(k) => Some(k),
                None => (count > 0).then(|| c.min(count - 1)),
            };
            self.cursor_key = self.cursor.and_then(|k| tray_strip::overflow_key(k, cx));
        }
        let size = tray_strip::overflow_size(cx);
        if size != self.reported {
            self.reported = size;
            super::panel::relayout_sized(super::panel::OVERFLOW, size, cx);
        }
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    pub fn on_open(&mut self, _dark: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = Some((Instant::now(), true));
        self.cursor = None;
        self.cursor_key = None;
        self.reported = self.size(cx);
        cx.notify();
    }

    /// The button was pressed again while the panel was shrinking back into its island (`grow_host.rs`): the motion turns round and the
    /// panel is open again as it was.
    pub fn on_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            // an island growing into this panel: its glass is a shape in the window that the growth moves (grow_host.rs)
            self.popup = Some(match super::grow_host::shape_for_attach(super::panel::OVERFLOW, cx) {
                Some((shape, radius)) => PopupGlass::start_panel_shaped(self.hwnd, dark, shape, radius),
                None => PopupGlass::start_panel(self.hwnd, dark),
            });
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        cx.notify();
    }

    /// Hidden: the glass comes off. The tip of an icon in the panel goes with it.
    pub fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self.cursor = None;
        self.cursor_key = None;
        cx.notify();
        cx.defer(super::tray_tip::dismiss);
    }

    // ------------------------------------------------------------------ the keyboard

    fn key(&mut self, ev: &KeyDownEvent, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let m = ev.keystroke.modifiers;
        let Some(key) = ts::nav_key(&ev.keystroke.key, m.shift, m.control, m.alt) else { return };
        cx.stop_propagation();
        let (count, cols) = (tray_strip::overflow_data(cx).count, tray_strip::overflow_cols(cx));
        match ts::grid_nav(key, self.cursor, count, cols) {
            ts::GridOutcome::Stay => {}
            ts::GridOutcome::Moved(c) => {
                self.cursor = Some(c);
                self.cursor_key = tray_strip::overflow_key(c, cx);
                cx.notify();
            }
            ts::GridOutcome::Click(kind, k) => {
                // the icon the ring is on, by its key: whatever happens to the cells before the deferred call below runs, the click goes
                // to that icon or to none
                let Some(key) = self.cursor_key.or_else(|| tray_strip::overflow_key(k, cx)) else { return };
                self.closing = true;
                // (the click closes the panel, which updates this view: not from inside its own update. It closes the panel on every
                // path, because `closing` makes this view deaf to keys, Escape included.)
                cx.defer(move |cx| tray_strip::overflow_key_click(key, kind, cx));
            }
            ts::GridOutcome::Close => cx.defer(|cx| super::panel::close(CloseCause::Escape, cx)),
        }
    }
}

impl Render for OverflowView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let look = Look::of(pal);
        // The island's glass is growing into this panel (`grow_host.rs`): the motion decides what is seen, the 120 ms easing stays out of it.
        let grown = super::grow_host::view_frame(super::panel::OVERFLOW, cx);
        let fill = glass_watch::panel_fill(look.dark, grown.map_or_else(|| self.popup.as_ref().is_none_or(|g| g.fallback()), |g| g.plain));
        let frame = match (self.anim, grown) {
            (_, Some(_)) | (None, None) => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
            (Some((t, opening)), None) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
        };
        if !frame.done || grown.is_some_and(|g| g.moving) {
            window.request_animation_frame();
        }
        tray_strip::report_rects(cx);
        let data = tray_strip::overflow_data(cx);
        let (hover, ring) = (look.pal.hover_fill().hsla(), look.accent);
        let press_bg = look.pal.press_plate().hsla();

        let mut body = div().relative().size_full().opacity(frame.opacity).mt(px(frame.drop_px));
        if data.count == 0 {
            // The two texts are broken into lines by the same rule that gave the window its height (`ts::ovf_empty`, which counts a little
            // wide), and each line is drawn on its own without wrapping again, so the text can neither run out at the side nor be cut at
            // the bottom, in either language.
            let empty = tray_strip::empty_panel();
            let line = |text: &str, size: f32, height: f32, color: Hsla| div().w_full().h(px(height)).flex().justify_center().text_size(px(size)).text_color(color).whitespace_nowrap().child(text.to_string());
            let (title_color, hint_color) = (look.pal.text().hsla(), look.pal.text_secondary().hsla());
            body = body.child(
                div()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(ts::OVF_PAD))
                    .children(empty.title.iter().map(|l| line(l, ts::OVF_TITLE_PX, ts::OVF_TITLE_LINE, title_color)))
                    .child(div().h(px(ts::OVF_EMPTY_GAP)))
                    .children(empty.hint.iter().map(|l| line(l, ts::OVF_HINT_PX, ts::OVF_HINT_LINE, hint_color))),
            );
        }
        for k in 0..data.count {
            let [x, y, w, _] = ts::ovf_cell_in(data.total, k, data.limits);
            let cell = div()
                .id(("tray-ovf", k))
                .absolute()
                .left(px(x))
                .top(px(y))
                .size(px(w))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .border_2()
                .border_color(if self.cursor == Some(k) { ring } else { transparent_black() })
                .cursor_pointer()
                .when(data.hover == Some(k), |d| d.bg(hover))
                .when(data.pressed == Some(k), |d| d.bg(press_bg))
                .when(data.dragged == Some(k), |d| d.opacity(0.35))
                .child(tray_strip::overflow_picture(k, ts::OVF_ICON, pal, cx));
            body = body.child(tray_strip::wire(cell, Source::Overflow(k)));
        }
        // a dragged icon over the panel: the panel is the place to let it go
        if data.drop_into {
            let mut edge = ring;
            edge.a = 0.8;
            body = body.child(div().absolute().inset_0().rounded(px(8.)).border_2().border_color(edge));
        }
        div()
            .id("tray-overflow-panel")
            .relative()
            .size_full()
            .when(grown.is_none(), |d| d.bg(fill))
            .children(grown.map(|g| super::grow_view::wash(&g, fill)))
            .children(grown.and_then(|g| super::grow_view::hairline(&g, look.pal)))
            .text_color(look.pal.text().hsla())
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| this.key(ev, cx)))
            .child(match grown {
                Some(g) => super::grow_view::body(&g, body).into_any_element(),
                None => body.into_any_element(),
            })
            .into_any_element()
    }
}
