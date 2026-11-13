//! The overlay window of one monitor: the frozen picture of that monitor, dimmed, with the clear rectangle of the drag (or of the
//! window or the monitor under the pointer) cut out of the dim, an outline in the system accent and its size in pixels beside it.
//!
//! The view only feeds events into the shared model and draws what the model says; every decision is `rf_win::snip::model`'s. The
//! picture is drawn once, full size, and the dim is laid over it as strips around the clear rectangle (`plan::dim_strips`), so nothing
//! is drawn twice and nothing is clipped with tricks.

use std::sync::Arc;

use gpui_kit::*;
use rf_core::desktop::geom::Rect;
use rf_win::snip::geom::{self, Pt};
use rf_win::snip::model::{self, Outcome};

use super::{State, plan};

/// How dark the dim is: black at this opacity over the frozen picture.
const DIM: f32 = 0.5;

pub struct OverlayView {
    ix: usize,
    state: Entity<State>,
    image: Arc<RenderImage>,
    /// The last pointer position handed to the model (device px): a move of less than a pixel is not news.
    last: Option<Pt>,
    _sub: Subscription,
}

impl OverlayView {
    pub fn new(ix: usize, state: Entity<State>, image: Arc<RenderImage>, cx: &mut Context<Self>) -> Self {
        // every overlay redraws when the model changes (a drag that began on another monitor shows here too)
        let sub = cx.observe(&state, |_, _, cx| cx.notify());
        OverlayView { ix, state, image, last: None, _sub: sub }
    }

    /// The window is going: its picture leaves the toolkit's atlas now (a screen is tens of megabytes).
    pub fn release_picture(&mut self, window: &mut Window) {
        let _ = window.drop_image(self.image.clone());
    }

    fn to_screen(&self, pos: Point<Pixels>, cx: &App) -> Pt {
        let m = self.state.read(cx).model.monitors()[self.ix];
        geom::to_screen(&m, (f32::from(pos.x), f32::from(pos.y)))
    }

    fn moved(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let p = self.to_screen(pos, cx);
        if self.last == Some(p) {
            return;
        }
        self.last = Some(p);
        self.state.update(cx, |s, cx| {
            s.model.pointer_moved(p);
            cx.notify();
        });
    }

    fn pressed(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let p = self.to_screen(pos, cx);
        self.last = Some(p);
        self.state.update(cx, |s, cx| {
            s.model.press(p);
            cx.notify();
        });
    }

    fn released(&mut self, pos: Point<Pixels>, cx: &mut Context<Self>) {
        let p = self.to_screen(pos, cx);
        self.last = Some(p);
        let outcome = self.state.update(cx, |s, cx| {
            let o = s.model.release(p);
            cx.notify();
            o
        });
        // (windows are closed from outside the handler of one of their own events)
        match outcome {
            Outcome::Take(region) => cx.defer(move |cx| super::take(region, cx)),
            Outcome::Cancel => cx.defer(super::cancel),
            Outcome::Pending => {}
        }
    }

    fn cancelled(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |s, _| {
            s.model.cancel();
        });
        cx.defer(super::cancel);
    }
}

/// The accent as a toolkit colour.
fn accent() -> Hsla {
    rgb(rf_win::accent::color()).into()
}

impl Render for OverlayView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let size = window.viewport_size();
        let (w, h) = (f32::from(size.width), f32::from(size.height));
        let (m, target, limit, mode) = {
            let st = self.state.read(cx);
            (st.model.monitors()[self.ix], st.model.highlight(), st.model.limit(), st.model.mode())
        };
        let hole = target.map(|r| geom::to_local(&m, r));
        let dim: Hsla = hsla(0.0, 0.0, 0.0, DIM);

        let mut root = div()
            .id(("snip-overlay", self.ix))
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(black())
            .cursor_crosshair()
            .child(img(self.image.clone()).absolute().top_0().left_0().size_full().object_fit(ObjectFit::Fill));
        for (x, y, sw, sh) in plan::dim_strips(w, h, hole) {
            root = root.child(div().absolute().left(px(x)).top(px(y)).w(px(sw)).h(px(sh)).bg(dim));
        }
        if let Some((x, y, hw, hh)) = hole.and_then(|r| plan::clip_to(r, w, h)) {
            // the outline: thicker in the window mode (it points at a thing) than around a region
            let outline = div().absolute().left(px(x)).top(px(y)).w(px(hw)).h(px(hh)).border_color(accent());
            root = root.child(if mode == model::Mode::Window { outline.border_4() } else { outline.border_1() });
        }
        if let Some(r) = target {
            // the label goes to the overlay that holds its corner
            let text = plan::size_text(r.width(), r.height());
            let lw = (plan::label_width(text.chars().count()) * m.scale).round() as i32;
            let lh = (plan::LABEL_H * m.scale).round() as i32;
            let at = model::label_place(r, (lw, lh), &limit);
            if self.state.read(cx).model.monitors().get(self.ix).is_some_and(|mm| mm.rect.contains(at.0, at.1)) {
                let (lx, ly, ..) = geom::to_local(&m, Rect::new(at.0, at.1, at.0 + 1, at.1 + 1));
                root = root.child(
                    div()
                        .absolute()
                        .left(px(lx))
                        .top(px(ly))
                        .h(px(plan::LABEL_H))
                        .w(px(plan::label_width(text.chars().count())))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .bg(hsla(0.0, 0.0, 0.0, 0.78))
                        .text_color(hsla(0.0, 0.0, 1.0, 0.95))
                        .text_size(px(12.5))
                        .child(text),
                );
            }
        }
        root.on_mouse_move(cx.listener(|this, ev: &MouseMoveEvent, _, cx| this.moved(ev.position, cx)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, _, cx| {
                    this.pressed(ev.position, cx);
                }),
            )
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &MouseDownEvent, _, cx| this.cancelled(cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, ev: &MouseUpEvent, _, cx| this.released(ev.position, cx)))
            // (a drag is delivered to the window it began in, also when the button comes up beyond its edge)
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, ev: &MouseUpEvent, _, cx| this.released(ev.position, cx)))
    }
}
