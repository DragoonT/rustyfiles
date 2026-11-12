//! The Recycle Bin's own settings, as Explorer's Recycle Bin Properties has them: per drive a custom
//! maximum size (MB) or "remove files immediately", and "display delete confirmation". A small dialog drawn over the
//! bin view. It READS the current values when it opens and writes only when Save is pressed, and then only what
//! differs (`rf_core::recycle_bin::save`). The store behind it is a trait: the real one (`rf_win::recycle_settings`)
//! reads the registry; nothing here writes without the Save button.

use std::sync::Arc;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::format;
use rf_core::recycle_bin::{self, BinSettings, BinSettingsEdit, BinSettingsStore, SaveError, SaveOutcome};

use crate::t;
use crate::theme::{self, Palette};

pub type Store = Arc<dyn BinSettingsStore + Send + Sync>;

pub enum BinDialogEvent {
    /// The dialog is done (cancelled, or saved: `Some` says what was written).
    Closed(Option<SaveOutcome>),
}

impl EventEmitter<BinDialogEvent> for BinSettingsDialog {}

pub struct BinSettingsDialog {
    store: Store,
    /// The settings as read; `None` while they are being read.
    now: Option<BinSettings>,
    edit: Option<BinSettingsEdit>,
    inputs: Vec<Entity<InputState>>,
    _subs: Vec<Subscription>,
    problems: Vec<(usize, recycle_bin::Problem)>,
    /// A failure reading or writing, in one line.
    error: Option<String>,
    saving: bool,
    focus: FocusHandle,
    _task: Option<Task<()>>,
}

impl BinSettingsDialog {
    pub fn new(store: Store, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut this = Self {
            store: store.clone(),
            now: None,
            edit: None,
            inputs: Vec::new(),
            _subs: Vec::new(),
            problems: Vec::new(),
            error: None,
            saving: false,
            focus,
            _task: None,
        };
        // the read is off the UI thread (it asks the shell and the registry)
        let work = cx.background_spawn(async move { store.read().map_err(|e| format!("{e:#}")) });
        this._task = Some(cx.spawn_in(window, async move |this, cx| {
            let read = work.await;
            let _ = this.update_in(cx, |v, window, cx| {
                match read {
                    Ok(s) => v.loaded(s, window, cx),
                    Err(e) => v.error = Some(e),
                }
                cx.notify();
            });
        }));
        this
    }

    fn loaded(&mut self, s: BinSettings, window: &mut Window, cx: &mut Context<Self>) {
        let edit = recycle_bin::initial_edit(&s);
        for (i, v) in edit.volumes.iter().enumerate() {
            let text = v.size_text.clone();
            let input = cx.new(|cx| {
                let mut st = InputState::new(window, cx).placeholder(t!("Varsayılan"));
                st.set_value(text, window, cx);
                st
            });
            self._subs.push(cx.subscribe_in(&input, window, move |this: &mut Self, input, ev: &InputEvent, _window, cx| {
                if let InputEvent::Change = ev {
                    let value = input.read(cx).value().to_string();
                    if let Some(e) = this.edit.as_mut().and_then(|e| e.volumes.get_mut(i)) {
                        e.size_text = value;
                    }
                    this.problems.retain(|(j, _)| *j != i);
                    cx.notify();
                }
            }));
            self.inputs.push(input);
        }
        self.edit = Some(edit);
        self.now = Some(s);
    }

    fn close(&mut self, saved: Option<SaveOutcome>, cx: &mut Context<Self>) {
        cx.emit(BinDialogEvent::Closed(saved));
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let (Some(now), Some(edit)) = (self.now.clone(), self.edit.clone()) else { return };
        if self.saving {
            return;
        }
        // problems are shown before anything is written (the same check runs again inside `save`)
        if let Err(p) = recycle_bin::plan(&now, &edit) {
            self.problems = p;
            cx.notify();
            return;
        }
        self.saving = true;
        self.error = None;
        let store = self.store.clone();
        let work = cx.background_spawn(async move { recycle_bin::save(&*store, &now, &edit) });
        self._task = Some(cx.spawn(async move |this, cx| {
            let res = work.await;
            let _ = this.update(cx, |v, cx| {
                v.saving = false;
                match res {
                    Ok(outcome) => v.close(Some(outcome), cx),
                    Err(SaveError::Invalid(p)) => v.problems = p,
                    Err(SaveError::Failed(e)) => {
                        v.error = Some(e);
                        // some drives may have been written before the failure: the dialog's "now" is read again, so that a second
                        // Save writes only what is still different (and the problem is not shown against stale values)
                        v.refresh_now(cx);
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Reads the settings again (off the UI thread) and keeps them as the dialog's current values; the controls keep what the user typed.
    fn refresh_now(&mut self, cx: &mut Context<Self>) {
        let store = self.store.clone();
        let work = cx.background_spawn(async move { store.read() });
        cx.spawn(async move |this, cx| {
            if let Ok(s) = work.await {
                let _ = this.update(cx, |v, cx| {
                    // (the same drives in the same order: the controls are per drive index)
                    if v.now.as_ref().is_some_and(|n| n.volumes.iter().map(|x| &x.id).eq(s.volumes.iter().map(|x| &x.id))) {
                        v.now = Some(s);
                        cx.notify();
                    }
                });
            }
        })
        .detach();
    }
}

fn radio(p: &Palette, on: bool) -> Div {
    div()
        .size(px(16.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(if on { p.accent } else { p.text_secondary })
        .when(on, |d| d.child(div().size(px(8.)).rounded_full().bg(p.accent)))
}

impl Render for BinSettingsDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let mut body = v_flex().gap_3();
        if let (Some(now), Some(edit)) = (&self.now, &self.edit) {
            if now.volumes.is_empty() {
                body = body.child(div().text_sm().text_color(p.text_secondary).child(t!("Geri Dönüşüm Kutusu olan sürücü bulunamadı")));
            }
            for (i, v) in now.volumes.iter().enumerate() {
                let e = &edit.volumes[i];
                let nuke = e.remove_immediately;
                let size = if v.capacity_mb > 0 { format!(" · {}", format::size(v.capacity_mb * 1024 * 1024)) } else { String::new() };
                let problem = self.problems.iter().find(|(j, _)| *j == i).map(|(_, pr)| crate::recycle_text::problem_text(*pr));
                let input = self.inputs.get(i).cloned();
                body = body.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .rounded(theme::RADIUS_SM)
                        .bg(p.content_bg)
                        .border_1()
                        .border_color(p.border)
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(format!("{}{size}", v.label)))
                        .child(
                            h_flex()
                                .id(("bin-custom", i))
                                .gap_2()
                                .items_center()
                                .text_sm()
                                .cursor_pointer()
                                .child(radio(&p, !nuke))
                                .child(div().flex_1().child(t!("Özel boyut (en çok, MB)")))
                                .child(
                                    div()
                                        .w(px(120.))
                                        .h(px(30.))
                                        .px_2()
                                        .flex()
                                        .items_center()
                                        .rounded(theme::RADIUS_SM)
                                        .bg(p.card_bg)
                                        .border_1()
                                        .border_color(if problem.is_some() { p.danger } else { p.border })
                                        .when(nuke, |d| d.opacity(0.5))
                                        .children(input.filter(|_| !nuke).map(|i| Input::new(&i).appearance(false)))
                                        .when(nuke, |d| d.text_color(p.text_tertiary).child(e.size_text.clone())),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(e) = this.edit.as_mut().and_then(|e| e.volumes.get_mut(i)) {
                                        e.remove_immediately = false;
                                    }
                                    cx.notify();
                                })),
                        )
                        .child(
                            h_flex()
                                .id(("bin-nuke", i))
                                .gap_2()
                                .items_center()
                                .text_sm()
                                .cursor_pointer()
                                .child(radio(&p, nuke))
                                .child(div().flex_1().child(t!("Dosyaları Geri Dönüşüm Kutusu'na taşıma; silinir silinmez kaldır")))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if let Some(e) = this.edit.as_mut().and_then(|e| e.volumes.get_mut(i)) {
                                        e.remove_immediately = true;
                                    }
                                    this.problems.retain(|(j, _)| *j != i);
                                    cx.notify();
                                })),
                        )
                        .children(problem.map(|t| div().text_xs().text_color(p.danger).child(t))),
                );
            }
            let confirm = edit.confirm_delete;
            let locked = now.confirm_locked;
            body = body.child(
                h_flex()
                    .id("bin-confirm")
                    .gap_2()
                    .items_center()
                    .text_sm()
                    .when(!locked, |d| d.cursor_pointer())
                    .when(locked, |d| d.opacity(0.6))
                    .child(
                        div()
                            .size(px(18.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .when(confirm, |d| d.bg(p.accent))
                            .when(!confirm, |d| d.border_1().border_color(p.text_secondary))
                            .when(confirm, |d| d.child(Icon::new(L::Check).size(px(12.)).text_color(p.accent_text))),
                    )
                    .child(div().flex_1().child(if locked { t!("Silme onayını göster (bir ilke tarafından belirlenmiş)") } else { t!("Silme onayını göster") }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !locked && let Some(e) = this.edit.as_mut() {
                            e.confirm_delete = !e.confirm_delete;
                        }
                        cx.notify();
                    })),
            );
        } else if let Some(e) = &self.error {
            body = body.child(div().text_sm().text_color(p.danger).child(e.clone()));
        } else {
            body = body.child(div().text_sm().text_color(p.text_secondary).child(t!("Yükleniyor…")));
        }
        let ready = self.edit.is_some() && !self.saving;
        let button = |id: &'static str, label: &'static str, primary: bool, enabled: bool| {
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
                .when(primary, |d| d.bg(p.accent).text_color(p.accent_text))
                .when(!primary, |d| d.bg(p.hover).text_color(p.text).border_1().border_color(p.border))
                .when(enabled, |d| d.hover(|s| s.opacity(0.9)).cursor_pointer())
                .when(!enabled, |d| d.opacity(0.5))
                .child(label)
        };
        div()
            .id("bin-settings-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.close(None, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                v_flex()
                    .w(px(520.))
                    .max_h(px(560.))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(p.text)
                    .child(
                        v_flex()
                            .p_6()
                            .gap_3()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(t!("Geri Dönüşüm Kutusu ayarları")))
                            .child(body)
                            .children(self.error.clone().filter(|_| self.edit.is_some()).map(|e| div().text_xs().text_color(p.danger).child(e))),
                    )
                    .child(
                        h_flex()
                            .px_6()
                            .py_4()
                            .gap_2()
                            .justify_end()
                            .bg(p.hover)
                            .border_t_1()
                            .border_color(p.divider)
                            .child(button("bin-save", t!("Kaydet"), true, ready).on_click(cx.listener(|this, _, _, cx| this.save(cx))))
                            .child(button("bin-cancel", t!("İptal"), false, true).on_click(cx.listener(|this, _, _, cx| this.close(None, cx)))),
                    ),
            )
    }
}
