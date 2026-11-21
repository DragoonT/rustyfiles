//! The preview of a run: every step as a row (file, arrow, destination, the rule), each with a
//! checkbox. Nothing moves before "Uygula".

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::organize::summary::show_path;
use rf_core::organize::{Dest, Plan, Rule};

use super::{RunSpec, Source};
use crate::t;
use crate::theme::{self, Palette};

enum State {
    Scanning,
    /// `folder_problem`: the folder could not be opened (gone, unplugged, no access).
    Ready { plan: Plan, checked: Vec<bool>, folder_problem: Option<String> },
}

pub struct PreviewView {
    title: String,
    state: State,
    focus: FocusHandle,
    /// The row the keyboard is on.
    cursor: usize,
    scroll: UniformListScrollHandle,
    /// Set when the dialog goes away: a scan still running stops.
    cancel: Arc<AtomicBool>,
    _task: Option<Task<()>>,
}

impl Drop for PreviewView {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl PreviewView {
    pub fn new(title: String, rules: Vec<Rule>, source: Source, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        // the folder is read and the plan made off the UI thread
        let protected = super::protected();
        let plan = cx.background_spawn(async move {
            // (a folder that cannot be opened must not pass for "nothing matches")
            let folder_problem = match &source {
                Source::Scan { root, .. } => rf_win::organize::folder_problem(root),
                Source::Paths(_) => None,
            };
            (super::make_plan(&rules, &source, &HashSet::new(), &protected, &|| flag.load(Ordering::Relaxed)), folder_problem)
        });
        let task = cx.spawn(async move |this, cx| {
            let (plan, folder_problem) = plan.await;
            let _ = this.update(cx, |v, cx| {
                let n = plan.steps.len();
                v.state = State::Ready { plan, checked: vec![true; n], folder_problem };
                cx.notify();
            });
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { title, state: State::Scanning, focus, cursor: 0, scroll: UniformListScrollHandle::new(), cancel, _task: Some(task) }
    }

    fn selected(&self) -> usize {
        match &self.state {
            State::Ready { checked, .. } => checked.iter().filter(|c| **c).count(),
            State::Scanning => 0,
        }
    }

    fn toggle(&mut self, i: usize, cx: &mut Context<Self>) {
        if let State::Ready { checked, .. } = &mut self.state
            && let Some(c) = checked.get_mut(i)
        {
            *c = !*c;
            cx.notify();
        }
    }

    fn set_all(&mut self, on: bool, cx: &mut Context<Self>) {
        if let State::Ready { checked, .. } = &mut self.state {
            checked.iter_mut().for_each(|c| *c = on);
            cx.notify();
        }
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let State::Ready { plan, checked, .. } = &self.state else { return };
        let steps: Vec<_> = plan.steps.iter().zip(checked).filter(|(_, on)| **on).map(|(s, _)| s.clone()).collect();
        if steps.is_empty() {
            return;
        }
        let label = self.title.clone();
        super::close(window, cx);
        super::start_run(RunSpec { steps, automatic: false, label, on_finished: None }, cx);
    }

    fn move_cursor(&mut self, by: isize, cx: &mut Context<Self>) {
        let State::Ready { plan, .. } = &self.state else { return };
        let n = plan.steps.len();
        if n == 0 {
            return;
        }
        self.cursor = (self.cursor as isize + by).clamp(0, n as isize - 1) as usize;
        self.scroll.scroll_to_item(self.cursor, ScrollStrategy::Nearest);
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev.keystroke.key.as_str() {
            "escape" => super::close(window, cx),
            // (Enter on a focused button is that button's own click)
            "enter" if self.focus.is_focused(window) => self.apply(window, cx),
            "down" => self.move_cursor(1, cx),
            "up" => self.move_cursor(-1, cx),
            "pagedown" => self.move_cursor(10, cx),
            "pageup" => self.move_cursor(-10, cx),
            "space" if self.focus.is_focused(window) => self.toggle(self.cursor, cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

fn checkbox_box(on: bool, p: &Palette) -> Div {
    div()
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(4.))
        .border_1()
        .border_color(if on { p.accent } else { p.text_tertiary })
        .when(on, |d| d.bg(p.accent))
        .when(on, |d| d.child(Icon::new(L::Check).size(px(12.)).text_color(p.accent_text)))
}

fn button(id: &'static str, label: &'static str, primary: bool, enabled: bool, p: &Palette) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px_4()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .tab_index(0)
        .when(primary, |d| d.bg(p.accent).text_color(p.accent_text))
        .when(!primary, |d| d.bg(p.hover).border_1().border_color(p.border))
        .when(enabled, |d| d.cursor_pointer().hover(|s| s.opacity(0.85)))
        .when(!enabled, |d| d.opacity(0.45))
        .child(label)
}

/// The notes under the list: rules that could not work, files that were skipped (grouped by reason,
/// with the first names).
fn notes(plan: &Plan) -> Vec<String> {
    let mut v = Vec::new();
    for (_, name) in &plan.loops {
        v.push(format!("«{name}»: {}", t!("kural kendi sonucunu yeniden yakalıyor; atlandı. Koşulu, yeni ad artık uymayacak şekilde daraltın (örneğin ad \"IMG\" ile başlasın) ya da dosyaları başka bir klasöre gönderin.")));
    }
    for (_, name, why) in &plan.unusable {
        let why = match why {
            rf_core::organize::Unusable::Problem(p) => crate::i18n::td(p.text()),
            rf_core::organize::Unusable::Refused(r) => crate::i18n::td(r.text()),
        };
        v.push(format!("«{name}»: {why}"));
    }
    let mut groups: Vec<(rf_core::organize::SkipReason, Vec<String>)> = Vec::new();
    for s in &plan.skipped {
        let name = s.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match groups.iter_mut().find(|(r, _)| *r == s.reason) {
            Some((_, names)) => names.push(name),
            None => groups.push((s.reason, vec![name])),
        }
    }
    for (reason, names) in groups {
        let mut shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if names.len() > 3 {
            shown.push_str(&format!(" +{}", names.len() - 3));
        }
        v.push(format!("{} ({}): {shown}", super::plural(names.len(), t!("1 dosya atlandı"), t!("{n} dosya atlandı")), crate::i18n::td(reason.text())));
    }
    v
}

impl Render for PreviewView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let home = super::home();

        let (total, selected) = match &self.state {
            State::Ready { plan, .. } => (plan.steps.len(), self.selected()),
            State::Scanning => (0, 0),
        };
        let subtitle: String = match &self.state {
            State::Scanning => t!("Dosyalar taranıyor…").to_string(),
            State::Ready { plan, .. } if plan.steps.is_empty() => t!("Düzenlenecek dosya yok").to_string(),
            State::Ready { .. } => crate::i18n::items(total),
        };

        let header = h_flex()
            .px_4()
            .py_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.divider)
            .child(Icon::new(L::WandSparkles).size(px(18.)).text_color(p.text_secondary))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).overflow_hidden().text_ellipsis().whitespace_nowrap().child(format!("{}: {}", t!("Klasörü düzenle"), self.title)))
                    .child(div().text_xs().text_color(p.text_secondary).child(subtitle)),
            )
            .child(
                div()
                    .id("pv-close")
                    .size(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .tab_index(0)
                    .child(Icon::new(L::X).size(px(14.)))
                    .on_click(cx.listener(|_, _, window, cx| super::close(window, cx))),
            );

        let body: AnyElement = match &self.state {
            State::Scanning => div().flex_1().flex().items_center().justify_center().text_sm().text_color(p.text_secondary).child(t!("Dosyalar taranıyor…")).into_any_element(),
            State::Ready { folder_problem: Some(why), .. } => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_1()
                .px_6()
                .text_sm()
                .text_color(p.danger)
                .child(div().child(t!("Klasör bulunamadı ya da açılamadı.")))
                .child(div().text_xs().child(why.clone()))
                .into_any_element(),
            State::Ready { plan, .. } if plan.steps.is_empty() => {
                let mut lines = vec![t!("Kurallara uyan, yerinden oynatılacak bir dosya yok.").to_string()];
                if plan.in_place > 0 {
                    lines.push(super::plural(plan.in_place, t!("1 dosya zaten olması gereken yerde"), t!("{n} dosya zaten olması gereken yerde")));
                }
                v_flex().flex_1().items_center().justify_center().gap_1().text_sm().text_color(p.text_secondary).children(lines.into_iter().map(|l| div().child(l))).into_any_element()
            }
            State::Ready { plan, .. } => {
                let head = h_flex()
                    .px_4()
                    .py_1()
                    .gap_3()
                    .text_xs()
                    .text_color(p.text_secondary)
                    .border_b_1()
                    .border_color(p.divider)
                    .child(div().w(px(16.)))
                    .child(div().flex_1().child(t!("Dosya")))
                    .child(div().w(px(14.)))
                    .child(div().flex_1().child(t!("Hedef")))
                    .child(div().w(px(120.)).child(t!("Kural")));
                let _ = plan;
                let list = uniform_list(
                    "pv-rows",
                    total,
                    cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                        let p = Palette::for_window(window);
                        let home = super::home();
                        let State::Ready { plan, checked, .. } = &this.state else { return Vec::new() };
                        range
                            .map(|i| {
                                let step = &plan.steps[i];
                                let on = checked[i];
                                let name = step.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                                let from_dir = step.source.parent().map(|d| show_path(d, home.as_deref())).unwrap_or_default();
                                let (to_name, to_dir, recycle) = match &step.dest {
                                    Dest::Move(d) => (
                                        d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                                        d.parent().map(|d| show_path(d, home.as_deref())).unwrap_or_default(),
                                        false,
                                    ),
                                    Dest::Recycle => (t!("Geri Dönüşüm Kutusu").to_string(), String::new(), true),
                                };
                                // (the full destination is in the tooltip: a long name is cut at the end, and
                                // the number a clash adds, " (2)", sits there)
                                let full = match &step.dest {
                                    Dest::Move(d) => d.display().to_string(),
                                    Dest::Recycle => String::new(),
                                };
                                h_flex()
                                    .id(("pv-row", i))
                                    .w_full()
                                    .h(px(44.))
                                    .when(!full.is_empty(), |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(full.clone()).build(w, cx)))
                                    .px_3()
                                    .gap_3()
                                    .items_center()
                                    .rounded(theme::RADIUS_SM)
                                    .cursor_pointer()
                                    .when(i == this.cursor, |d| d.bg(p.hover))
                                    .when(!on, |d| d.opacity(0.5))
                                    .child(checkbox_box(on, &p))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(div().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                                            .child(div().text_xs().text_color(p.text_tertiary).overflow_hidden().text_ellipsis().whitespace_nowrap().child(from_dir)),
                                    )
                                    .child(Icon::new(L::ArrowRight).size(px(14.)).flex_shrink_0().text_color(p.text_tertiary))
                                    .child(
                                        v_flex()
                                            .flex_1()
                                            .min_w_0()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .items_center()
                                                    .text_sm()
                                                    .when(recycle, |d| d.text_color(p.danger))
                                                    .when(recycle, |d| d.child(Icon::new(L::Trash).size(px(13.))))
                                                    .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(to_name)),
                                            )
                                            .child(div().text_xs().text_color(p.text_tertiary).overflow_hidden().text_ellipsis().whitespace_nowrap().child(to_dir)),
                                    )
                                    .child(div().id(("pv-rule", i)).w(px(120.)).flex_shrink_0().text_xs().text_color(p.text_secondary).overflow_hidden().text_ellipsis().whitespace_nowrap().child(step.rule_name.clone()))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.cursor = i;
                                        this.toggle(i, cx);
                                    }))
                                    .into_any_element()
                            })
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .p_1p5();
                v_flex().flex_1().min_h_0().child(head).child(list).into_any_element()
            }
        };

        let note_lines = match &self.state {
            State::Ready { plan, .. } => notes(plan),
            State::Scanning => Vec::new(),
        };
        let notes_box = (!note_lines.is_empty()).then(|| {
            v_flex()
                .max_h(px(96.))
                .overflow_hidden()
                .px_4()
                .py_2()
                .gap_0p5()
                .bg(p.danger.opacity(0.10))
                .text_xs()
                .text_color(p.danger)
                .children(note_lines.into_iter().take(5).map(|n| h_flex().gap_1().items_start().child(Icon::new(L::TriangleAlert).size(px(12.)).flex_shrink_0()).child(div().child(n))))
        });

        let ready = matches!(self.state, State::Ready { .. });
        // with nothing to do the dialog only closes
        let nothing_to_do = ready && total == 0;
        let status = if ready { format!("{selected} / {total} {}", t!("seçili")) } else { String::new() };
        let all_on = total > 0 && selected == total;
        let footer = h_flex()
            .px_4()
            .py_3()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(p.divider)
            .child(div().text_sm().text_color(p.text_secondary).child(status))
            .when(total > 1, |d| {
                d.child(
                    div()
                        .id("pv-all")
                        .px_2()
                        .py_1()
                        .rounded(theme::RADIUS_SM)
                        .text_sm()
                        .text_color(p.accent)
                        .cursor_pointer()
                        .hover(|s| s.bg(p.hover))
                        .tab_index(0)
                        .child(if all_on { t!("Hiçbirini seçme") } else { t!("Tümünü seç") })
                        .on_click(cx.listener(move |this, _, _, cx| this.set_all(!all_on, cx))),
                )
            })
            .child(div().flex_1())
            .child(button("pv-cancel", if nothing_to_do { t!("Kapat") } else { t!("İptal") }, false, true, &p).on_click(cx.listener(|_, _, window, cx| super::close(window, cx))))
            .child(button("pv-apply", t!("Uygula"), true, selected > 0, &p).on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))));

        let _ = home;
        div()
            .id("organizer-preview")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            .child(
                v_flex()
                    .w(px(820.))
                    .max_w(relative(0.95))
                    .h(px(560.))
                    .max_h(relative(0.9))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(p.text)
                    .child(header)
                    .child(body)
                    .children(notes_box)
                    .child(footer)
                    .with_animation("organizer-preview-in", Animation::new(std::time::Duration::from_millis(160)).with_easing(ease_out_quint()), |el, t| el.opacity(t).mt(px(10. * (1. - t)))),
            )
    }
}
