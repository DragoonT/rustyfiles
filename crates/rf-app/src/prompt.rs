//! A small Fluent dialog (title, message, optional text box, buttons) drawn by the app, so
//! questions look like the rest of rustyfiles instead of a Win32 message box. One at a time;
//! the workspace of its window paints it on top.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::theme::{self, Palette};

/// Button handler: gets the text box value (if any). `Err(message)` keeps the dialog open and
/// shows the message under the text box (invalid input).
pub type OnButton = Rc<dyn Fn(Option<String>, &mut Window, &mut App) -> Result<(), String>>;

pub struct Button {
    pub label: SharedString,
    pub kind: Kind,
    pub run: OnButton,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Primary,
    Normal,
    Danger,
}

impl Button {
    pub fn new(label: impl Into<SharedString>, kind: Kind, run: impl Fn(Option<String>, &mut Window, &mut App) -> Result<(), String> + 'static) -> Self {
        Self { label: label.into(), kind, run: Rc::new(run) }
    }
    /// A button that just closes.
    pub fn cancel() -> Self {
        Self::new(crate::t!("İptal"), Kind::Normal, |_, _, _| Ok(()))
    }
}

/// A labelled row of mutually exclusive options (segmented control). The caller keeps the
/// `selected` cell and reads it in its button handler.
pub struct Choice {
    pub label: SharedString,
    pub options: Vec<SharedString>,
    pub selected: Rc<std::cell::Cell<usize>>,
}

/// A row with a check box (a torrent's files to download): its text, a quieter note at its right (a size) and whether it is ticked.
pub struct Check {
    pub label: SharedString,
    pub note: SharedString,
    pub on: Rc<std::cell::Cell<bool>>,
}

pub struct DialogView {
    title: SharedString,
    message: Option<SharedString>,
    error: Option<SharedString>,
    input: Option<Entity<InputState>>,
    /// The text box hides what is typed (a password): it has an eye button that shows it, and while shown it can be copied (issue
    /// 204: "A button to see what is being typed when extracting or creating a password protected compressed file").
    masked: bool,
    choices: Vec<Choice>,
    checks: Vec<Check>,
    buttons: Vec<Button>,
    focus: FocusHandle,
    /// The button the keyboard is on: it carries a ring, Enter and Space press it, Tab, Shift+Tab and the arrows move it (the
    /// owner, 2026-11-17: "hangi butonun seçili olduğu belli olsun. ok tuşlarıyla ya da tabla değişebilsin"). It starts on the
    /// main button (`main_button`); in a dialog with a text box it starts nowhere, and Tab leaves the box for the buttons.
    keyed: Option<usize>,
    /// The default button (the one Enter presses) and where it was drawn last: Windows' "Snap To" puts the pointer on it (issue 148).
    snap_ix: Option<usize>,
    snap_at: Rc<Cell<Option<Bounds<Pixels>>>>,
    _sub: Option<Subscription>,
}

struct Host(Option<(AnyWindowHandle, Entity<DialogView>, Option<FocusHandle>)>);
impl Global for Host {}

/// Shows a dialog. `input = Some(initial)` adds a text box (Enter = first Primary button).
pub fn show(
    title: impl Into<SharedString>,
    message: Option<String>,
    input: Option<(String, String)>,
    buttons: Vec<Button>,
    window: &mut Window,
    cx: &mut App,
) {
    show_with(title, message, input, Vec::new(), buttons, window, cx)
}

/// `show` plus option rows under the text box.
pub fn show_with(
    title: impl Into<SharedString>,
    message: Option<String>,
    input: Option<(String, String)>,
    choices: Vec<Choice>,
    buttons: Vec<Button>,
    window: &mut Window,
    cx: &mut App,
) {
    show_inner(title, message, input, false, choices, Vec::new(), buttons, window, cx)
}

/// A plain notice with one button.
pub fn notice(title: impl Into<SharedString>, message: String, window: &mut Window, cx: &mut App) {
    show(title, Some(message), None, vec![Button::new(crate::t!("Tamam"), Kind::Primary, |_, _, _| Ok(()))], window, cx)
}

/// A yes/no question. The receiver gets `true` for "Evet" and `false` for "Hayır"; Escape, or a dialog put away some other way,
/// drops the sender with the dialog, which a waiting side reads as "no" too (`rx.await.unwrap_or(false)`).
///
/// For questions that were Windows message boxes shown from the shell thread: those belonged to that thread's hidden window, so
/// Windows opened them BEHIND the program's own window and without the focus (issue 30: the question about a changed file
/// extension had to be found with Alt+Tab), and they looked like nothing else in the program.
pub fn ask(title: impl Into<SharedString>, message: String, window: &mut Window, cx: &mut App) -> futures::channel::oneshot::Receiver<bool> {
    let (tx, rx) = futures::channel::oneshot::channel::<bool>();
    let tx = Rc::new(std::cell::Cell::new(Some(tx)));
    let answer = |yes: bool| {
        let tx = tx.clone();
        move |_: Option<String>, _: &mut Window, _: &mut App| {
            if let Some(tx) = tx.take() {
                let _ = tx.send(yes);
            }
            Ok(())
        }
    };
    let buttons = vec![Button::new(crate::t!("Evet"), Kind::Primary, answer(true)), Button::new(crate::t!("Hayır"), Kind::Normal, answer(false))];
    show(title, Some(message), None, buttons, window, cx);
    rx
}

/// A dialog whose text box hides what is typed (passwords).
pub fn show_secret(title: impl Into<SharedString>, message: Option<String>, placeholder: String, buttons: Vec<Button>, window: &mut Window, cx: &mut App) {
    show_inner(title, message, Some((String::new(), placeholder)), true, Vec::new(), Vec::new(), buttons, window, cx)
}

/// A dialog with a list of check boxes (a long list scrolls).
pub fn show_checks(title: impl Into<SharedString>, message: Option<String>, checks: Vec<Check>, buttons: Vec<Button>, window: &mut Window, cx: &mut App) {
    show_inner(title, message, None, false, Vec::new(), checks, buttons, window, cx)
}

#[allow(clippy::too_many_arguments)]
fn show_inner(
    title: impl Into<SharedString>,
    message: Option<String>,
    input: Option<(String, String)>,
    masked: bool,
    choices: Vec<Choice>,
    checks: Vec<Check>,
    buttons: Vec<Button>,
    window: &mut Window,
    cx: &mut App,
) {
    let restore = window.focused(cx);
    let view = cx.new(|cx| {
        let input = input.map(|(initial, placeholder)| {
            cx.new(|cx| {
                let mut s = InputState::new(window, cx).placeholder(placeholder).masked(masked);
                s.set_value(initial, window, cx);
                s
            })
        });
        let sub = input.as_ref().map(|i| {
            cx.subscribe_in(i, window, |this: &mut DialogView, _, ev: &InputEvent, window, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    this.press_primary(window, cx);
                } else if let InputEvent::Change = ev {
                    if this.error.take().is_some() {
                        cx.notify();
                    }
                }
            })
        });
        let main = main_button(&buttons.iter().map(|b| b.kind).collect::<Vec<_>>());
        let keyed = if input.is_some() { None } else { main.or(Some(0)) };
        let snap_ix = main.or(if buttons.is_empty() { None } else { Some(0) });
        DialogView {
            title: title.into(),
            message: message.map(Into::into),
            error: None,
            input,
            masked,
            choices,
            checks,
            buttons,
            focus: cx.focus_handle(),
            keyed,
            snap_ix,
            snap_at: Rc::new(Cell::new(None)),
            _sub: sub,
        }
    });
    // Windows' "Snap To" (Mouse settings: "Automatically move pointer to the default button in a dialog box", issue 148): its own
    // dialogs put the pointer on their default button, so ours do too, once the dialog has come in (its animation is 160 ms)
    if rf_win::sysparams::snap_to_default_button() {
        let at = view.read(cx).snap_at.clone();
        window
            .spawn(cx, async move |cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(220)).await;
                let _ = cx.update(|window, _| {
                    let (Some(b), Some(hwnd)) = (at.get(), window_hwnd(window)) else { return };
                    let k = window.scale_factor();
                    let c = b.center();
                    rf_win::sysparams::put_pointer_in(hwnd, (f32::from(c.x) * k).round() as i32, (f32::from(c.y) * k).round() as i32);
                });
            })
            .detach();
    }
    match view.read(cx).input.clone() {
        Some(i) => i.update(cx, |s, cx| s.focus(window, cx)),
        None => window.focus(&view.read(cx).focus.clone(), cx),
    }
    cx.set_global(Host(Some((window.window_handle(), view, restore))));
    window.refresh();
}

fn window_hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
        _ => None,
    }
}

pub fn current(window: &Window, cx: &App) -> Option<Entity<DialogView>> {
    let (handle, view, _) = cx.try_global::<Host>()?.0.as_ref()?;
    (*handle == window.window_handle()).then(|| view.clone())
}

pub fn close(window: &mut Window, cx: &mut App) {
    let Some(Some((_, _, restore))) = cx.try_global::<Host>().map(|h| h.0.clone()) else { return };
    cx.set_global(Host(None));
    if let Some(f) = restore {
        window.focus(&f, cx);
    }
    window.refresh();
}

impl DialogView {
    fn value(&self, cx: &App) -> Option<String> {
        self.input.as_ref().map(|i| i.read(cx).value().to_string())
    }

    fn press(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.buttons.get(ix) else { return };
        let (run, value) = (b.run.clone(), self.value(cx));
        match run(value, window, cx) {
            Ok(()) => close(window, cx),
            Err(e) => {
                self.error = Some(e.into());
                cx.notify();
            }
        }
    }

    /// Moves the keyboard to the next (`step` 1) or the previous (-1) button, round. From the text box it goes to the first or the
    /// last button, and the box gives up the keyboard.
    fn step(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.buttons.len();
        if n == 0 {
            return;
        }
        self.keyed = Some(next_button(self.keyed, step, n));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Enter: the dialog's main button, which is the `Primary` one or, in a question whose answer destroys something, the `Danger`
    /// one. WHY (issue 42: "When using Shift + Delete ... pressing Enter does not confirm the action"): Enter looked for `Primary`
    /// only, and the permanent delete's "Delete" is `Danger`, so Enter did nothing there. Explorer's own question takes Enter too.
    fn press_primary(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = main_button(&self.buttons.iter().map(|b| b.kind).collect::<Vec<_>>()) {
            self.press(ix, window, cx);
        }
    }
}

impl DialogView {
    /// The check boxes, in a list that scrolls past ten rows.
    fn check_list(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut list = v_flex().id("dlg-checks").max_h(px(300.)).overflow_y_scroll().rounded(theme::RADIUS_SM).border_1().border_color(p.border).bg(p.content_bg);
        for (i, c) in self.checks.iter().enumerate() {
            let on = c.on.get();
            let cell = c.on.clone();
            let mark = div()
                .size(px(16.))
                .flex_shrink_0()
                .rounded(px(4.))
                .border_1()
                .border_color(if on { p.accent } else { p.border })
                .when(on, |d| d.bg(p.accent).flex().items_center().justify_center().child(gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Check).size(px(12.)).text_color(p.accent_text)));
            list = list.child(
                h_flex()
                    .id(("dlg-check", i))
                    .h(px(30.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .child(mark)
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(c.label.clone()))
                    .child(div().flex_shrink_0().text_xs().text_color(p.text_secondary).child(c.note.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cell.set(!cell.get());
                        cx.notify();
                    })),
            );
        }
        list.into_any_element()
    }
}

impl Render for DialogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let mut buttons = h_flex().gap_2().justify_end();
        for (ix, b) in self.buttons.iter().enumerate() {
            let (bg, fg) = match b.kind {
                Kind::Primary => (p.accent, p.accent_text),
                Kind::Danger => (p.danger, p.accent_text),
                Kind::Normal => (p.hover, p.text),
            };
            let keyed = self.keyed == Some(ix);
            // (the default button's place, for "Snap To")
            let probe = (self.snap_ix == Some(ix)).then(|| {
                let at = self.snap_at.clone();
                canvas(move |b, _, _| at.set(Some(b)), |_, _, _, _| {}).absolute().size_full()
            });
            buttons = buttons.child(
                div()
                    .id(("dlg-btn", ix))
                    .relative()
                    .children(probe)
                    // the keyboard's button: a ring in the text's colour around it, outside its own fill
                    .when(keyed, |d| d.border_2().border_color(p.text))
                    .min_w(px(96.))
                    .h(px(32.))
                    .px_4()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .text_sm()
                    .bg(bg)
                    .text_color(fg)
                    .when(b.kind == Kind::Normal && !keyed, |d| d.border_1().border_color(p.border))
                    .hover(|s| s.opacity(0.9))
                    .active(|s| s.opacity(0.8))
                    .child(b.label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| this.press(ix, window, cx))),
            );
        }
        let mut choice_rows = Vec::new();
        for (ci, c) in self.choices.iter().enumerate() {
            let mut seg = h_flex().p(px(2.)).gap(px(2.)).rounded(theme::RADIUS_SM).bg(p.content_bg).border_1().border_color(p.border);
            let cur = c.selected.get();
            for (oi, o) in c.options.iter().enumerate() {
                let on = oi == cur;
                let cell = c.selected.clone();
                seg = seg.child(
                    div()
                        .id(SharedString::from(format!("dlg-ch-{ci}-{oi}")))
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(4.))
                        .text_sm()
                        .when(on, |d| d.bg(p.card_bg).shadow_sm().font_weight(FontWeight::MEDIUM))
                        .when(!on, |d| d.text_color(p.text_secondary).hover(|s| s.bg(p.hover)))
                        .child(o.clone())
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cell.set(oi);
                            cx.notify();
                        })),
                );
            }
            choice_rows.push(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(p.text_secondary).child(c.label.clone()))
                    .child(seg),
            );
        }
        div()
            .id("dialog-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .key_context(crate::actions::DIALOG_CTX)
            .on_action(cx.listener(|this, _: &crate::actions::DialogNext, window, cx| this.step(1, window, cx)))
            .on_action(cx.listener(|this, _: &crate::actions::DialogPrev, window, cx| this.step(-1, window, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                // (the text box keeps its own keys while it has the keyboard)
                let in_box = this.input.as_ref().is_some_and(|i| i.read(cx).focus_handle(cx).is_focused(window));
                match ev.keystroke.key.as_str() {
                    "escape" => {
                        close(window, cx);
                        cx.stop_propagation();
                    }
                    "left" | "up" if !in_box => {
                        this.step(-1, window, cx);
                        cx.stop_propagation();
                    }
                    "right" | "down" if !in_box => {
                        this.step(1, window, cx);
                        cx.stop_propagation();
                    }
                    // (not a HELD Enter: the press that asked the question, still down, must not answer it)
                    "enter" | "space" if !in_box && !ev.is_held => {
                        match this.keyed {
                            Some(ix) => this.press(ix, window, cx),
                            None if ev.keystroke.key == "enter" => this.press_primary(window, cx),
                            None => {}
                        }
                        cx.stop_propagation();
                    }
                    _ => {}
                }
            }))
            .child(
                v_flex()
                    .w(px(440.))
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
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(self.title.clone()))
                            .children(self.message.clone().map(|m| div().text_sm().text_color(p.text_secondary).child(m)))
                            .children(self.input.as_ref().map(|i| {
                                // a password that is not shown cannot be copied or cut out (issue 204: with the eye button it could be,
                                // dots and all); shown, it can
                                let hidden = |i: &Entity<InputState>, cx: &App| i.read(cx).presentation().is_masked();
                                let (ic, ix) = (i.clone(), i.clone());
                                div()
                                    .when(self.masked, |d| {
                                        d.capture_action(move |_: &gpui_kit::component::input::Copy, _, cx| {
                                            if hidden(&ic, cx) {
                                                cx.stop_propagation();
                                            }
                                        })
                                        .capture_action(move |_: &gpui_kit::component::input::Cut, _, cx| {
                                            if hidden(&ix, cx) {
                                                cx.stop_propagation();
                                            }
                                        })
                                    })
                                    .h(px(34.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded(theme::RADIUS_SM)
                                    .bg(p.content_bg)
                                    .border_1()
                                    .border_color(if self.error.is_some() { p.danger } else { p.accent })
                                    .child(Input::new(i).appearance(false).when(self.masked, |d| d.mask_toggle()))
                            }))
                            .children(self.error.clone().map(|e| div().text_xs().text_color(p.danger).child(e)))
                            .children(choice_rows)
                            .when(!self.checks.is_empty(), |d| d.child(self.check_list(&p, cx))),
                    )
                    .child(div().px_6().py_4().bg(p.hover).border_t_1().border_color(p.divider).child(buttons))
                    .with_animation("dialog-in", Animation::new(std::time::Duration::from_millis(160)).with_easing(ease_out_quint()), |el, t| {
                        el.opacity(t).mt(px(10. * (1. - t)))
                    }),
            )
    }
}

/// The button after `from` going `step` (round); from none: the first going forward, the last going back.
fn next_button(from: Option<usize>, step: isize, n: usize) -> usize {
    match from {
        None if step >= 0 => 0,
        None => n - 1,
        Some(i) => (i as isize + step).rem_euclid(n as isize) as usize,
    }
}

/// Which button Enter presses: the first `Primary`, else the first `Danger`, else none.
fn main_button(kinds: &[Kind]) -> Option<usize> {
    kinds.iter().position(|k| *k == Kind::Primary).or_else(|| kinds.iter().position(|k| *k == Kind::Danger))
}

#[cfg(test)]
mod main_button_tests {
    use super::{Kind, main_button};

    #[test]
    fn the_keyboard_goes_round_the_buttons() {
        use super::next_button;
        assert_eq!((next_button(Some(0), 1, 2), next_button(Some(1), 1, 2)), (1, 0));
        assert_eq!((next_button(Some(0), -1, 2), next_button(None, 1, 3), next_button(None, -1, 3)), (1, 0, 2));
    }

    #[test]
    fn enter_presses_the_primary_button_else_the_danger_one() {
        assert_eq!(main_button(&[Kind::Normal, Kind::Primary]), Some(1));
        assert_eq!(main_button(&[Kind::Danger, Kind::Normal]), Some(0), "a permanent delete: Enter confirms it (issue 42)");
        assert_eq!(main_button(&[Kind::Danger, Kind::Primary]), Some(1), "a primary button wins over a danger one");
        assert_eq!(main_button(&[Kind::Normal]), None);
    }
}
