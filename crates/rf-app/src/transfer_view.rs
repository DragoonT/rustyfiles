//! A running copy, move or delete where the user looks: a strip in the status bar, a floating
//! card at the corner of the content, and the ring and pulse on the Status Center button.
//! The rules (what is shown, when, in which order, what gives way) are in
//! `rf_core::transfer`; this file only draws them.
//!
//! Nothing here draws a path. The ring is a small set of pictures of arcs made once (a 64 px
//! square is 16 KB; the window never allocates the toolkit's window-sized path textures for it),
//! the chart of the last minute is boxes, the bars are boxes.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::format;
use rf_core::transfer::{self as xfer, Outcome, Ring, Stage, Strip};
use rf_win::fastcopy::Resolve;

use crate::settings::TransferProgress;
use crate::status_center::{OpPhase, OpView, Ops, RingState, fmt_duration};
use crate::t;
use crate::theme::{self, Palette};

/// "45,3 MB/sn" in the app's own size formatting.
pub(crate) fn speed_text(bytes_per_sec: f64) -> String {
    format!("{}/{}", format::size(bytes_per_sec as u64), t!("sn"))
}

/// "~34 sn kaldı".
fn eta_text(secs: f64) -> String {
    format!("{} {}", fmt_duration(secs as f32), t!("kaldı"))
}

/// "3,2 GB / 8,1 GB".
fn bytes_text((done, total): (u64, u64)) -> String {
    format!("{} / {}", format::size(done), format::size(total))
}

// ---------------------------------------------------------------------------------------------
// Where the progress is shown
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Surface {
    Strip,
    Card,
    /// Only the Status Center (and the button's ring).
    Nothing,
}

/// The setting, made real: a strip needs a status bar to sit in, so when the user turned the
/// status bar off the card stands in for it; a file dialog of another program (`picker`) gets
/// neither.
pub(crate) fn surface(mode: TransferProgress, status_bar_on: bool, picker: bool) -> Surface {
    if picker {
        return Surface::Nothing;
    }
    match mode {
        TransferProgress::StatusBar if status_bar_on => Surface::Strip,
        TransferProgress::StatusBar | TransferProgress::FloatingCard => Surface::Card,
        TransferProgress::StatusCenter => Surface::Nothing,
    }
}

/// Whether a conflict question opens its dialog at once. With a strip or a card the question
/// is on them, and the dialog (whose backdrop would cover them) opens only from "Seç…". With
/// the Status Center only there is nothing else to ask on, and with no window at all the
/// dialog's code answers for itself.
pub(crate) fn question_in_dialog(mode: TransferProgress, any_window: bool) -> bool {
    mode == TransferProgress::StatusCenter || !any_window
}

// ---------------------------------------------------------------------------------------------
// The ring and the pulse on the Status Center button
// ---------------------------------------------------------------------------------------------

/// Side of a ring picture in pixels (the button is 32 logical pixels: twice that for sharp edges).
const RING_PX: usize = 64;

type RingKey = (u32, u8, u8);

/// Pictures kept at most; past it the cache starts over.
const RING_CACHE_MAX: usize = 300;

/// The ring pictures made so far, and the ones given up that the toolkit has yet to be told about.
#[derive(Default)]
struct RingCache {
    pictures: HashMap<RingKey, Arc<RenderImage>>,
    /// Dropping an `Arc` is not enough: GPUI keeps an uploaded picture in each window's sprite
    /// atlas until `drop_image` (see `release_retired`).
    retired: Vec<Arc<RenderImage>>,
}

impl RingCache {
    fn get(&mut self, key: RingKey, max: usize, make: impl FnOnce() -> Arc<RenderImage>) -> Arc<RenderImage> {
        if self.pictures.len() >= max && !self.pictures.contains_key(&key) {
            self.retired.extend(self.pictures.drain().map(|(_, v)| v));
        }
        self.pictures.entry(key).or_insert_with(make).clone()
    }
}

static RING_PICTURES: LazyLock<Mutex<RingCache>> = LazyLock::new(Default::default);

/// Gives the toolkit back the ring pictures the cache gave up (after a long session with many
/// accent colours). Called from where an operation begins and ends, which have an `App`; the
/// drop runs after the current update because a window being updated is out of the app's list
/// for a moment (as `image_cache.rs` does).
pub(crate) fn release_retired(cx: &mut App) {
    let old = std::mem::take(&mut RING_PICTURES.lock().unwrap_or_else(|e| e.into_inner()).retired);
    if old.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for img in old {
            cx.drop_image(img, None);
        }
    });
}

/// The picture of an arc in a colour, made the first time it is asked for. A user sees at
/// most the 40 lengths of one colour plus the 40 positions of the turning arc, about 1.3 MB for
/// a long copy; the cache starts over at [`RING_CACHE_MAX`] pictures (several colours) so it cannot grow.
fn ring_picture(color: Hsla, start: usize, sweep: usize) -> Arc<RenderImage> {
    let c: Rgba = color.into();
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b) = (ch(c.r), ch(c.g), ch(c.b));
    let key = ((u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b), start as u8, sweep as u8);
    let mut cache = RING_PICTURES.lock().unwrap_or_else(|e| e.into_inner());
    cache.get(key, RING_CACHE_MAX, || {
        let coverage = xfer::ring_coverage(RING_PX, 3.0, 4.0, start, sweep);
        // GPUI wants straight-alpha BGRA
        let mut px = Vec::with_capacity(RING_PX * RING_PX * 4);
        for a in coverage {
            px.extend_from_slice(&[b, g, r, a]);
        }
        let buf = image::RgbaImage::from_raw(RING_PX as u32, RING_PX as u32, px).expect("size matches");
        Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))
    })
}

fn ring_img(color: Hsla, start: usize, sweep: usize) -> Img {
    img(ring_picture(color, start, sweep)).size_full()
}

/// The ring around the button's icon (`None` when nothing runs and nothing is fading).
pub(crate) fn ring_layer(st: &RingState, accent: Hsla, track: Hsla, animate: bool) -> Option<AnyElement> {
    let layer = || div().absolute().inset_0();
    let track_layer = || layer().opacity(0.2).child(ring_img(track, 0, xfer::RING_STEPS));
    match xfer::ring(st.running, st.fraction, st.since_end_ms, animate, 0) {
        Ring::None => None,
        Ring::Track => Some(track_layer().into_any_element()),
        Ring::Arc { steps, .. } => match st.since_end_ms {
            // closed and fading: an animation of a fixed length that starts when the fade is first drawn (under the
            // run's id, so a redraw does not restart it). The length must not depend on how old the fade is: the
            // toolkit reads the length afresh each frame but keeps the start, so a length that shrinks with the
            // age would be fed back into the fade and run it far too fast.
            Some(_) => Some(
                layer()
                    .child(ring_img(accent, 0, steps))
                    .with_animation(("sc-ring-fade", st.end_epoch as usize), Animation::new(Duration::from_millis(xfer::RING_FADE_MS)), |el, t| el.opacity(xfer::fade_alpha(t)))
                    .into_any_element(),
            ),
            None => Some(layer().child(track_layer()).child(ring_img(accent, 0, steps)).into_any_element()),
        },
        // the arc has RING_STEPS positions: more redraws than that would only make the whole window
        // redraw at the monitor's rate for as long as the total stays unknown
        Ring::Spin { .. } => Some(
            layer()
                .child(track_layer())
                .child(layer().with_animation(
                    "sc-ring-spin",
                    Animation::new(Duration::from_millis(xfer::SPIN_MS)).repeat().with_max_fps(xfer::SPIN_FPS),
                    move |el, t| {
                        let start = ((t * xfer::RING_STEPS as f32) as usize) % xfer::RING_STEPS;
                        el.child(ring_img(accent, start, xfer::SPIN_SWEEP))
                    },
                ))
                .into_any_element(),
        ),
    }
}

/// The short attention pulse when an operation starts and when everything has finished: a
/// frame that grows out of the button and fades. Not with animations off.
pub(crate) fn pulse_layer(st: &RingState, color: Hsla, animate: bool) -> Option<AnyElement> {
    if !animate {
        return None;
    }
    // (the age is not used: the animation has the pulse's own fixed length and starts when it is first drawn)
    let (n, _age) = st.pulse?;
    Some(
        div()
            .absolute()
            .inset_0()
            .child(
                div()
                    .absolute()
                    .rounded(theme::RADIUS_SM)
                    .border_2()
                    .border_color(color)
                    .with_animation(("sc-pulse-ev", n as usize), Animation::new(Duration::from_millis(xfer::PULSE_MS)), move |el, t| {
                        // (the growth is eased inside `pulse_at`)
                        let (grow, alpha) = xfer::pulse_at(t);
                        let out = px(-7.0 * grow);
                        el.top(out).left(out).right(out).bottom(out).opacity(alpha)
                    }),
            )
            .into_any_element(),
    )
}

// ---------------------------------------------------------------------------------------------
// Small parts
// ---------------------------------------------------------------------------------------------

fn bar(p: &Palette, frac: Option<f32>, color: Hsla, w: Option<f32>) -> Div {
    // an unknown total (git, network): a short segment, as the Status Center draws it
    let f = frac.map_or(0.15, |f| f.max(0.02));
    div()
        .h(px(4.))
        .when_some(w, |d, w| d.w(px(w)))
        .when(w.is_none(), |d| d.w_full())
        .flex_shrink_0()
        .rounded_full()
        .bg(p.pressed)
        .child(div().h_full().rounded_full().bg(color).w(relative(f)))
}

fn icon_button(id: impl Into<ElementId>, p: &Palette, icon: L, tip: &'static str, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .size(px(22.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .text_color(p.text_secondary)
        .cursor_pointer()
        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
        .hover(|s| s.bg(p.pressed).text_color(p.text))
        .child(Icon::new(icon).size(px(12.)))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx)
        })
        .into_any_element()
}

fn text_button(id: impl Into<ElementId>, p: &Palette, label: impl Into<SharedString>, primary: bool, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .h(px(22.))
        .px_2()
        .flex_shrink_0()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .text_xs()
        .cursor_pointer()
        .when(primary, |d| d.bg(p.accent).text_color(p.accent_text).hover(|s| s.opacity(0.9)))
        .when(!primary, |d| d.text_color(p.text).border_1().border_color(p.border).hover(|s| s.bg(p.hover)).active(|s| s.bg(p.pressed)))
        .child(label.into())
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx)
        })
        .into_any_element()
}

fn open_center(ops: &Entity<Ops>, window: &mut Window, cx: &mut App) {
    let owner = window.window_handle();
    ops.update(cx, |o, cx| o.show_center(owner, cx));
}

/// The three answers the Status Center's dialog gives for everything, and its "choose for each".
fn answer_buttons(ops: &Entity<Ops>, p: &Palette, v: &OpView) -> Vec<AnyElement> {
    let id = v.id;
    let ans = |name: &'static str, label: &'static str, r: Resolve, primary: bool| {
        let ops = ops.clone();
        text_button((name, id as usize), p, label, primary, move |_, cx| ops.update(cx, |o, cx| o.answer(id, r, cx)))
    };
    vec![
        ans("q-replace", t!("Değiştir"), Resolve::Replace, false),
        ans("q-skip", t!("Atla"), Resolve::Skip, false),
        ans("q-both", t!("İkisini de tut"), Resolve::KeepBoth, true),
        // the full list is the dialog's: opened here, in the window the click is in
        text_button(("q-each", id as usize), p, t!("Seç…"), false, {
            let ops = ops.clone();
            move |window, cx| {
                let w = window.window_handle();
                ops.update(cx, |o, cx| o.choose_each(id, w, cx))
            }
        }),
    ]
}

fn question_text(v: &OpView) -> String {
    format!("{} ({})", t!("Hedefte aynı adlı öğeler var"), v.question.unwrap_or(0))
}

fn failure_text(v: &OpView) -> String {
    match &v.phase {
        OpPhase::Failed(e) => format!("{} {}", t!("Hata:"), crate::i18n::td(e)),
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------------------------
// The status bar's strip
// ---------------------------------------------------------------------------------------------

/// What the strip would show now (the window shows its status bar for it even on Home).
pub(crate) fn strip_due(ops: &Entity<Ops>, cx: &App) -> bool {
    let views = ops.read(cx).views();
    let briefs: Vec<xfer::Brief> = views.iter().map(|v| v.brief).collect();
    xfer::strip_pick(&briefs) != Strip::Nothing
}

/// The strip for the newest running operation (or a question, or an unseen failure), `avail`
/// pixels wide: the middle of the status bar.
pub(crate) fn strip(ops: &Entity<Ops>, p: &Palette, avail: f32, cx: &App) -> Option<AnyElement> {
    let views = ops.read(cx).views();
    let briefs: Vec<xfer::Brief> = views.iter().map(|v| v.brief).collect();
    let (v, others) = match xfer::strip_pick(&briefs) {
        Strip::Nothing => return None,
        Strip::Failed { index } => (&views[index], 0),
        Strip::Running { index, others } => (&views[index], others),
    };
    let fit = xfer::strip_fit(avail, others);
    let id = v.id;
    // (clipped: the room is an estimate of what the rest of the status bar leaves, and a wrong guess must cut the strip, not overlap its neighbours)
    let frame = h_flex().id("transfer-strip").h(px(24.)).flex_1().min_w_0().overflow_hidden().mx_3().gap(px(6.)).items_center().text_xs().text_color(p.text_secondary);
    let center = {
        let ops = ops.clone();
        move |_: &ClickEvent, window: &mut Window, cx: &mut App| open_center(&ops, window, cx)
    };
    let main = || {
        h_flex()
            .id("transfer-strip-main")
            .h_full()
            .flex_1()
            .min_w_0()
            .px_2()
            .gap(px(10.))
            .items_center()
            .rounded(theme::RADIUS_SM)
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Durum Merkezi")).build(w, cx))
            .on_click(center.clone())
    };
    let words = |s: String, color: Hsla| div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_color(color).child(s);

    // a failure nobody has looked at
    if matches!(v.phase, OpPhase::Failed(_)) {
        let dismiss = {
            let ops = ops.clone();
            icon_button(("strip-dismiss", id as usize), p, L::X, t!("Kapat"), move |_, cx| ops.update(cx, |o, cx| o.dismiss(id, cx)))
        };
        return Some(
            frame
                .child(
                    main()
                        .child(Icon::new(L::CircleAlert).size(px(14.)).text_color(p.danger))
                        .child(words(failure_text(v), p.text)),
                )
                .child(dismiss)
                .into_any_element(),
        );
    }

    // the copy waits for an answer
    if v.question.is_some() {
        let cancel = {
            let ops = ops.clone();
            icon_button(("strip-cancel", id as usize), p, L::X, t!("İptal"), move |_, cx| ops.update(cx, |o, cx| o.cancel_op(id, cx)))
        };
        return Some(
            frame
                .child(
                    main()
                        .child(Icon::new(L::CircleAlert).size(px(14.)).text_color(p.accent))
                        .child(words(question_text(v), p.text)),
                )
                .children(answer_buttons(ops, p, v))
                .child(cancel)
                .into_any_element(),
        );
    }

    let accent = if v.paused { p.text_tertiary } else { p.accent };
    let pct = v.frac.map(|f| crate::i18n::pct(f * 100.0)).unwrap_or_default();
    let speed = v.pace.speed.filter(|s| *s >= 1.0 && fit.speed).map(speed_text);
    let eta = v.pace.eta_secs.filter(|_| fit.eta).map(eta_text);
    let text = if v.paused { format!("{} · {}", v.words, t!("Duraklatıldı")) } else { v.words.clone() };
    let pause = {
        let ops = ops.clone();
        icon_button(
            ("strip-pause", id as usize),
            p,
            if v.paused { L::Play } else { L::Pause },
            if v.paused { t!("Devam et") } else { t!("Duraklat") },
            move |_, cx| ops.update(cx, |o, cx| o.toggle_pause(id, cx)),
        )
    };
    let cancel = {
        let ops = ops.clone();
        icon_button(("strip-cancel", id as usize), p, L::X, t!("İptal"), move |_, cx| ops.update(cx, |o, cx| o.cancel_op(id, cx)))
    };
    let chip = {
        let ops = ops.clone();
        div()
            .id("strip-more")
            .h(px(18.))
            .min_w(px(26.))
            .px_1p5()
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(p.accent)
            .text_color(p.accent_text)
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .hover(|s| s.opacity(0.9))
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Durum Merkezi")).build(w, cx))
            .child(format!("+{others}"))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                open_center(&ops, window, cx)
            })
            .into_any_element()
    };
    Some(
        frame
            .child(
                main()
                    .when(fit.text, |d| d.child(words(text, p.text)))
                    .child(bar(p, v.frac, accent, Some(96.)))
                    .child(div().w(px(36.)).flex_shrink_0().text_right().text_color(p.text).child(pct))
                    .children(speed.map(|s| div().flex_shrink_0().child(s)))
                    .children(eta.map(|s| div().flex_shrink_0().child(s))),
            )
            .when(fit.buttons, |d| d.child(pause).child(cancel))
            .when(fit.chip, |d| d.child(chip))
            .into_any_element(),
    )
}

// ---------------------------------------------------------------------------------------------
// The floating card
// ---------------------------------------------------------------------------------------------

const CARD_W: f32 = 340.0;

/// The cards at the bottom right corner of the content, `bottom` pixels above its lower edge
/// (above the status bar). `None` when no card is due.
pub(crate) fn cards(ops: &Entity<Ops>, p: &Palette, bottom: f32, cx: &App) -> Option<AnyElement> {
    let views = ops.read(cx).views();
    let stages: Vec<(u64, Stage)> = views.iter().map(|v| (v.id, v.stage)).collect();
    let stack = xfer::stack(&stages);
    if stack.shown.is_empty() {
        return None;
    }
    let animate = crate::settings::animations();
    let mut col = v_flex().id("transfer-cards").absolute().right(px(16.)).bottom(px(bottom)).w(px(CARD_W)).gap_2().items_end();
    if stack.more > 0 {
        let ops = ops.clone();
        col = col.child(
            div()
                .id("cards-more")
                .h(px(22.))
                .px_2()
                .flex()
                .items_center()
                .rounded_full()
                .bg(p.accent)
                .text_color(p.accent_text)
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Durum Merkezi")).build(w, cx))
                .child(format!("+{}", stack.more))
                .on_click(move |_, window, cx| open_center(&ops, window, cx)),
        );
    }
    for id in &stack.shown {
        let Some(v) = views.iter().find(|v| v.id == *id) else { continue };
        let card = card(ops, p, v);
        col = col.child(if animate {
            // slides up and fades in once (the id is the operation's, so a redraw does not restart it)
            card.with_animation(("card-in", v.id as usize), Animation::new(Duration::from_millis(220)).with_easing(ease_out_quint()), |el, t| el.opacity(t).mb(px(-10. * (1. - t)))).into_any_element()
        } else {
            card.into_any_element()
        });
    }
    Some(col.into_any_element())
}

fn card(ops: &Entity<Ops>, p: &Palette, v: &OpView) -> Stateful<Div> {
    let id = v.id as usize;
    let shell = v_flex()
        .id(("card", id))
        .w(px(CARD_W))
        .p_3()
        .gap_2()
        .rounded(theme::RADIUS)
        .bg(p.card_bg)
        .border_1()
        .border_color(p.border)
        .shadow_lg()
        // the list behind must not take a click that lands on the card
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .text_color(p.text);
    let title = |icon: L, color: Hsla, text: String| {
        h_flex()
            .gap_2()
            .items_center()
            .child(Icon::new(icon).size(px(14.)).text_color(color))
            .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().font_weight(FontWeight::SEMIBOLD).child(text))
    };
    let dismiss = {
        let ops = ops.clone();
        let vid = v.id;
        icon_button(("card-dismiss", id), p, L::X, t!("Kapat"), move |_, cx| ops.update(cx, |o, cx| o.dismiss(vid, cx)))
    };
    let cancel = {
        let ops = ops.clone();
        let vid = v.id;
        icon_button(("card-cancel", id), p, L::X, t!("İptal"), move |_, cx| ops.update(cx, |o, cx| o.cancel_op(vid, cx)))
    };
    let green: Hsla = gpui_kit::rgb(0x30D158).into();
    match v.stage {
        Stage::Result(Outcome::Failed) => {
            let details = {
                let ops = ops.clone();
                text_button(("card-details", id), p, t!("Ayrıntılar"), false, move |window, cx| open_center(&ops, window, cx))
            };
            shell
                .child(title(L::CircleAlert, p.danger, v.title.clone()))
                .child(div().text_xs().text_color(p.text_secondary).child(failure_text(v)))
                .child(h_flex().gap_2().items_center().child(details).child(div().flex_1()).child(dismiss))
        }
        Stage::Result(Outcome::Done) => {
            let open = v.open_dest.clone().map(|dir| {
                text_button(("card-open", id), p, t!("Hedefi aç"), false, move |_, cx| crate::workspace::route_ipc(dir.to_string_lossy().into_owned(), cx))
            });
            let undo = v.can_undo.then(|| {
                let ops = ops.clone();
                let vid = v.id;
                text_button(("card-undo", id), p, t!("Geri al"), false, move |_, cx| {
                    crate::undo::undo(cx);
                    ops.update(cx, |o, cx| o.dismiss(vid, cx));
                })
            });
            // a button the operation brought itself (the folder organizer's "Geri al")
            let own = v.action.clone().map(|(label, run)| text_button(("card-act", id), p, label, false, move |_, cx| run(cx)));
            shell
                .child(title(L::CircleCheck, green, v.title.clone()))
                .child(div().text_xs().text_color(p.text_secondary).child(t!("Tamamlandı")))
                .child(h_flex().gap_2().items_center().children(open).children(undo).children(own).child(div().flex_1()).child(dismiss))
        }
        Stage::Result(Outcome::Cancelled) => shell
            .child(h_flex().gap_2().items_center().child(div().flex_1().min_w_0().child(title(L::CircleSlash, p.text_secondary, v.title.clone()))).child(dismiss))
            .child(div().text_xs().text_color(p.text_secondary).child(t!("İptal edildi"))),
        Stage::Asking => shell
            .child(h_flex().gap_2().items_center().child(div().flex_1().min_w_0().child(title(L::CircleAlert, p.accent, v.title.clone()))).child(cancel))
            .child(div().text_xs().text_color(p.text_secondary).child(question_text(v)))
            .child(h_flex().gap_2().flex_wrap().children(answer_buttons(ops, p, v))),
        _ => running_card(ops, p, v, shell, cancel),
    }
}

fn running_card(ops: &Entity<Ops>, p: &Palette, v: &OpView, shell: Stateful<Div>, cancel: AnyElement) -> Stateful<Div> {
    let id = v.id as usize;
    let vid = v.id;
    let accent = if v.paused { p.text_tertiary } else { p.accent };
    let fold = {
        let ops = ops.clone();
        icon_button(("card-fold", id), p, if v.collapsed { L::ChevronUp } else { L::ChevronDown }, if v.collapsed { t!("Genişlet") } else { t!("Küçült") }, move |_, cx| {
            ops.update(cx, |o, cx| o.toggle_collapsed(vid, cx))
        })
    };
    let pause = {
        let ops = ops.clone();
        icon_button(("card-pause", id), p, if v.paused { L::Play } else { L::Pause }, if v.paused { t!("Devam et") } else { t!("Duraklat") }, move |_, cx| {
            ops.update(cx, |o, cx| o.toggle_pause(vid, cx))
        })
    };
    let pct = v.frac.map(|f| crate::i18n::pct(f * 100.0)).unwrap_or_default();
    let speed = v.pace.speed.filter(|s| *s >= 1.0).map(speed_text);
    let eta = v.pace.eta_secs.map(eta_text);

    // folded to one line: the pace and a thin bar
    if v.collapsed {
        let pace = [Some(pct), speed, eta].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let pace = if v.paused { t!("Duraklatıldı").to_string() } else { pace };
        return shell
            .p_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(L::LoaderCircle).size(px(14.)).text_color(accent))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_xs().text_color(p.text_secondary).child(pace))
                    .child(bar(p, v.frac, accent, Some(56.)))
                    .child(fold)
                    .child(cancel),
            );
    }

    let current = v.current.clone().map(|c| xfer::middle_truncate(&c, 44));
    let totals = v.bytes.map(bytes_text);
    let pace = [speed, eta].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    let bars = (!v.bars.is_empty()).then(|| {
        h_flex().h(px(20.)).gap(px(2.)).items_end().children(v.bars.iter().map(|b| {
            div().w(px(5.)).h(px(20. * b.max(0.07))).rounded(px(1.5)).bg(accent).opacity(0.35 + 0.65 * b)
        }))
    });
    shell
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Icon::new(L::LoaderCircle).size(px(14.)).text_color(accent))
                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().text_sm().font_weight(FontWeight::SEMIBOLD).child(v.title.clone()))
                .child(fold),
        )
        .children(current.map(|c| div().text_xs().text_color(p.text_secondary).overflow_hidden().whitespace_nowrap().child(c)))
        .child(h_flex().gap_2().items_center().child(div().flex_1().child(bar(p, v.frac, accent, None))).child(div().w(px(36.)).text_xs().text_right().child(pct)))
        .child(
            h_flex()
                .gap_2()
                .text_xs()
                .text_color(p.text_secondary)
                .child(div().flex_1().min_w_0().overflow_hidden().whitespace_nowrap().child(totals.unwrap_or_default()))
                .child(div().flex_shrink_0().child(if v.paused { t!("Duraklatıldı").to_string() } else { pace })),
        )
        .child(h_flex().gap_2().items_end().child(div().flex_1().children(bars)).child(pause).child(cancel))
}

#[cfg(test)]
mod tests {
    // (not `use super::*`: the toolkit's glob brings an attribute named `test` that shadows the real one)
    use super::{RingCache, Surface, bytes_text, question_in_dialog, ring_picture, speed_text, surface};
    use crate::settings::TransferProgress;
    use gpui_kit::rgb;
    use rf_core::{format, transfer as xfer};
    use std::sync::Arc;

    #[test]
    fn the_setting_picks_where_the_progress_goes() {
        use TransferProgress::{FloatingCard, StatusBar, StatusCenter};
        assert_eq!(surface(StatusBar, true, false), Surface::Strip);
        assert_eq!(surface(FloatingCard, true, false), Surface::Card);
        assert_eq!(surface(StatusCenter, true, false), Surface::Nothing);
        // no status bar to sit in: the card stands in
        assert_eq!(surface(StatusBar, false, false), Surface::Card);
        assert_eq!(surface(FloatingCard, false, false), Surface::Card);
        assert_eq!(surface(StatusCenter, false, false), Surface::Nothing);
        // a file dialog of another program gets nothing
        for m in [StatusBar, FloatingCard, StatusCenter] {
            assert_eq!(surface(m, true, true), Surface::Nothing);
        }
    }

    #[test]
    fn a_question_opens_its_dialog_at_once_only_where_nothing_else_can_ask_it() {
        use TransferProgress::{FloatingCard, StatusBar, StatusCenter};
        assert!(question_in_dialog(StatusCenter, true));
        assert!(!question_in_dialog(StatusBar, true), "the strip asks; its dialog would cover it");
        assert!(!question_in_dialog(FloatingCard, true));
        // no window at all: the dialog's own code answers the copy
        assert!(question_in_dialog(StatusBar, false));
        assert!(question_in_dialog(FloatingCard, false));
    }

    #[test]
    fn a_full_ring_cache_hands_back_what_it_gave_up() {
        let pic = || Arc::new(gpui_kit::RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]));
        let mut c = RingCache::default();
        let a = c.get((1, 0, 0), 2, pic);
        let b = c.get((2, 0, 0), 2, pic);
        // asking again for a kept one makes nothing and retires nothing
        let again = c.get((1, 0, 0), 2, || panic!("made twice"));
        assert!(Arc::ptr_eq(&a, &again) && c.retired.is_empty());
        // a third picture: the cache starts over, and the two it held are for the toolkit to drop
        let _new = c.get((3, 0, 0), 2, pic);
        assert_eq!(c.retired.len(), 2);
        assert!(c.retired.iter().any(|r| Arc::ptr_eq(r, &a)) && c.retired.iter().any(|r| Arc::ptr_eq(r, &b)));
        assert_eq!(c.pictures.len(), 1);
    }

    #[test]
    fn speeds_are_in_the_apps_own_sizes() {
        // (the word for "a second" is not looked at: another test walks through every interface language at the same time,
        // and the ending is that language's: "/sn", "/s", "/วินาที")
        let s = speed_text(46_137_344.0);
        assert!(s.contains("MB") && s.contains('/') && !s.ends_with('/'), "{s}");
        assert_eq!(bytes_text((0, 1024)), format!("{} / {}", format::size(0), format::size(1024)));
    }

    #[test]
    fn every_ring_picture_is_what_it_says() {
        let a = ring_picture(rgb(0x0078D4).into(), 0, xfer::RING_STEPS);
        let b = ring_picture(rgb(0x0078D4).into(), 0, xfer::RING_STEPS);
        assert!(Arc::ptr_eq(&a, &b), "made once per colour and arc");
        let c = ring_picture(rgb(0x0078D4).into(), 0, 10);
        assert!(!Arc::ptr_eq(&a, &c));
        let d = ring_picture(rgb(0xFF0000).into(), 0, xfer::RING_STEPS);
        assert!(!Arc::ptr_eq(&a, &d), "another colour is another picture");
    }
}
