//! "These items already exist" (like Files' conflict dialog): one list of every clashing file
//! of a copy or move, each with its own choice (keep both, replace, skip), sizes and dates of
//! both versions, and one-click answers for all. Asked from the copy thread, which waits.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::fastcopy::{Conflict, Resolve};

use crate::select_box::{Choice, select_box};
use crate::t;
use crate::theme::{self, Palette};

/// Rows drawn; the rest follow the answer chosen for all (or keep both).
const SHOWN: usize = 200;

pub type Reply = Sender<Option<Vec<Resolve>>>;

pub struct ConflictView {
    list: Vec<Conflict>,
    choice: Vec<Resolve>,
    /// Taken by the first answer (the copy thread waits for exactly one).
    reply: Rc<RefCell<Option<Reply>>>,
    focus: FocusHandle,
}

struct Host(Option<(AnyWindowHandle, Entity<ConflictView>, Option<FocusHandle>)>);
impl Global for Host {}

/// Shows the dialog in `window` (or the active window, or any). No window: both are kept.
pub fn show(list: Vec<Conflict>, reply: Reply, window: Option<AnyWindowHandle>, cx: &mut App) {
    let target = window.or_else(|| cx.active_window()).or_else(|| cx.windows().first().copied());
    let Some(target) = target else {
        let _ = reply.send(Some(vec![Resolve::KeepBoth; list.len()]));
        return;
    };
    // if the window is gone, the reply is dropped: the copy thread takes that as "cancel"
    let r2 = Rc::new(RefCell::new(Some(reply)));
    let _ = target.update(cx, move |_, window, cx| {
        // one at a time: an older question still open is answered "cancel"
        if let Some(Some((_, old, _))) = cx.try_global::<Host>().map(|h| h.0.clone()) {
            old.update(cx, |v, _| v.send(None));
        }
        let restore = window.focused(cx);
        let n = list.len();
        let view = cx.new(|cx| ConflictView { list, choice: vec![Resolve::KeepBoth; n], reply: r2, focus: cx.focus_handle() });
        window.focus(&view.read(cx).focus.clone(), cx);
        window.activate_window();
        cx.set_global(Host(Some((window.window_handle(), view, restore))));
        window.refresh();
    });
}

pub fn current(window: &Window, cx: &App) -> Option<Entity<ConflictView>> {
    let (handle, view, _) = cx.try_global::<Host>()?.0.as_ref()?;
    (*handle == window.window_handle()).then(|| view.clone())
}

fn close(window: &mut Window, cx: &mut App) {
    let Some(Some((_, _, restore))) = cx.try_global::<Host>().map(|h| h.0.clone()) else { return };
    cx.set_global(Host(None));
    if let Some(f) = restore {
        window.focus(&f, cx);
    }
    window.refresh();
}

fn ms(t: Option<SystemTime>) -> Option<i64> {
    t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64)
}

impl ConflictView {
    fn send(&mut self, answer: Option<Vec<Resolve>>) {
        if let Some(r) = self.reply.borrow_mut().take() {
            let _ = r.send(answer);
        }
    }

    fn answer(&mut self, answer: Option<Vec<Resolve>>, window: &mut Window, cx: &mut Context<Self>) {
        self.send(answer);
        close(window, cx);
    }

    fn all(&mut self, r: Resolve, window: &mut Window, cx: &mut Context<Self>) {
        let v = vec![r; self.list.len()];
        self.answer(Some(v), window, cx);
    }

    /// Replace where the incoming file is newer, skip the rest.
    fn newer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let v = self
            .list
            .iter()
            .map(|c| match (c.src_time, c.dst_time) {
                (Some(a), Some(b)) if a > b && !c.mismatched() => Resolve::Replace,
                _ => Resolve::Skip,
            })
            .collect();
        self.answer(Some(v), window, cx);
    }
}

impl Drop for ConflictView {
    /// Closed any other way (the window went away): the copy must not wait forever.
    fn drop(&mut self) {
        self.send(None);
    }
}

fn label(r: Resolve) -> &'static str {
    match r {
        Resolve::KeepBoth => t!("İkisini de tut"),
        Resolve::Replace => t!("Değiştir"),
        Resolve::Skip => t!("Atla"),
    }
}

fn side(p: &Palette, title: &str, size: u64, is_dir: bool, time: Option<SystemTime>, newer: bool) -> Div {
    let mut parts: Vec<String> = Vec::new();
    if is_dir {
        parts.push(t!("Klasör").to_string());
    } else {
        parts.push(rf_core::format::size(size));
    }
    if let Some(ms) = ms(time) {
        parts.push(rf_core::format::datetime_full(ms));
    }
    h_flex()
        .gap_1()
        .text_xs()
        .text_color(p.text_secondary)
        .child(div().w(px(52.)).flex_shrink_0().child(title.to_string()))
        .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(parts.join(" · ")))
        .when(newer, |d| d.child(div().flex_shrink_0().px_1().rounded(px(3.)).bg(p.hover).text_color(p.accent).child(t!("daha yeni"))))
}

impl Render for ConflictView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let me = cx.entity();
        let n = self.list.len();
        let dest = self.list.first().and_then(|c| c.dst.parent()).map(rf_core::history::display_name).unwrap_or_default();
        let title = if n == 1 { t!("Bu ad hedefte zaten var").to_string() } else { format!("{} {}", n, t!("öğe hedefte zaten var")) };

        let quick = |id: &'static str, text: &'static str, run: fn(&mut ConflictView, &mut Window, &mut Context<ConflictView>)| {
            div()
                .id(id)
                .h(px(30.))
                .px_3()
                .flex()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .bg(p.hover)
                .border_1()
                .border_color(p.border)
                .text_sm()
                .cursor_pointer()
                .hover(|s| s.bg(p.pressed))
                .child(text)
                .on_click(cx.listener(move |v, _, window, cx| run(v, window, cx)))
        };

        let mut rows = v_flex().id("conflict-list").flex_1().min_h_0().overflow_y_scroll().gap_1();
        for (i, c) in self.list.iter().take(SHOWN).enumerate() {
            let name = c.src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let (src_newer, dst_newer) = match (c.src_time, c.dst_time) {
                (Some(a), Some(b)) => (a > b, b > a),
                _ => (false, false),
            };
            let options: Vec<Resolve> = if c.mismatched() { vec![Resolve::KeepBoth, Resolve::Skip] } else { vec![Resolve::KeepBoth, Resolve::Replace, Resolve::Skip] };
            let current = self.choice[i];
            let me2 = me.clone();
            let id = SharedString::from(format!("conflict-{i}"));
            rows = rows.child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .bg(p.content_bg)
                    .child(Icon::new(if c.src_dir { L::Folder } else { L::File }).size(px(18.)).text_color(p.text_secondary))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap(px(2.))
                            .child(div().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                            .child(side(&p, t!("Gelen"), c.src_size, c.src_dir, c.src_time, src_newer))
                            .child(side(&p, t!("Hedefte"), c.dst_size, c.dst_dir, c.dst_time, dst_newer)),
                    )
                    .child(div().w(px(170.)).flex_shrink_0().child(select_box(id, &p, label(current), move |_| {
                        options
                            .iter()
                            .map(|&r| {
                                let me3 = me2.clone();
                                Choice::new(label(r), r == current, move |_, cx| {
                                    me3.update(cx, |v, cx| {
                                        v.choice[i] = r;
                                        cx.notify();
                                    })
                                })
                            })
                            .collect()
                    }))),
            );
        }
        if n > SHOWN {
            rows = rows.child(div().px_3().py_2().text_xs().text_color(p.text_secondary).child(format!("+{} · {}", n - SHOWN, t!("diğerlerinde ikisi de tutulur"))));
        }

        let button = |id: &'static str, text: &'static str, primary: bool| {
            div()
                .id(id)
                .min_w(px(96.))
                .h(px(32.))
                .px_4()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SM)
                .text_sm()
                .bg(if primary { p.accent } else { p.hover })
                .text_color(if primary { p.accent_text } else { p.text })
                .when(!primary, |d| d.border_1().border_color(p.border))
                .hover(|s| s.opacity(0.9))
                .child(text)
        };

        div()
            .id("conflict-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                "escape" => {
                    v.answer(None, window, cx);
                    cx.stop_propagation();
                }
                "enter" => {
                    let c = v.choice.clone();
                    v.answer(Some(c), window, cx);
                    cx.stop_propagation();
                }
                _ => {}
            }))
            .child(
                v_flex()
                    .w(px(640.))
                    .max_h(relative(0.85))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(p.text)
                    .child(
                        v_flex()
                            .px_6()
                            .pt_6()
                            .pb_3()
                            .gap_1()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(div().text_sm().text_color(p.text_secondary).child(format!("{} {}", t!("Hedef:"), dest))),
                    )
                    .child(
                        h_flex()
                            .px_6()
                            .pb_3()
                            .gap_2()
                            .flex_wrap()
                            .child(quick("cf-replace", t!("Hepsini değiştir"), |v, w, cx| v.all(Resolve::Replace, w, cx)))
                            .child(quick("cf-skip", t!("Hepsini atla"), |v, w, cx| v.all(Resolve::Skip, w, cx)))
                            .child(quick("cf-both", t!("Hepsinde ikisini de tut"), |v, w, cx| v.all(Resolve::KeepBoth, w, cx)))
                            .child(quick("cf-newer", t!("Yalnızca daha yenileri değiştir"), |v, w, cx| v.newer(w, cx))),
                    )
                    .child(div().px_6().pb_2().text_xs().text_color(p.text_secondary).child(t!("Ya da her biri için seçin:")))
                    .child(v_flex().px_4().min_h_0().flex_1().child(rows))
                    .child(
                        h_flex()
                            .px_6()
                            .py_4()
                            .mt_3()
                            .gap_2()
                            .justify_end()
                            .bg(p.hover)
                            .border_t_1()
                            .border_color(p.divider)
                            .child(button("cf-cancel", t!("İptal"), false).on_click(cx.listener(|v, _, window, cx| v.answer(None, window, cx))))
                            .child(button("cf-go", t!("Devam"), true).on_click(cx.listener(|v, _, window, cx| {
                                let c = v.choice.clone();
                                v.answer(Some(c), window, cx);
                            }))),
                    )
                    .with_animation("conflict-in", Animation::new(std::time::Duration::from_millis(160)).with_easing(ease_out_quint()), |el, t| {
                        el.opacity(t).mt(px(10. * (1. - t)))
                    }),
            )
    }
}
