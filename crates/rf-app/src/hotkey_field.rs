//! The Shortcut tab's "Shortcut key" field, editable the way Explorer's is: click it and press the
//! combination; Backspace (or Delete) empties it; a combination Windows will not take is refused with
//! one line of text under the field. The rules are `rf_core::hotkey` (pure, tested); this file only
//! catches the keys and draws the box.
//!
//! The keys are caught with a keystroke interceptor, so the pressed keys never reach the actions
//! they are bound to (Ctrl+Alt+N must not open a window while it is being recorded). The interceptor
//! is installed the first time a field is armed (nothing runs at start-up) and only acts while the
//! armed field has the keyboard focus, so there is no state to get stuck: Tab and an unmodified Esc
//! pass through to leave the field.

use std::rc::Rc;

use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::hotkey::{self, Decision, Held, Refusal};

use crate::t;
use crate::theme::{self, Palette};

type OnKey = Rc<dyn Fn(Decision, &mut Window, &mut App)>;

/// The field that takes the keys now, and what to do with them.
#[derive(Default)]
struct Armed(Option<(WeakFocusHandle, OnKey)>);
impl Global for Armed {}

/// What a Properties window keeps for the field.
pub struct HotkeyState {
    pub focus: FocusHandle,
    /// The refusal under the field, until the next key.
    pub message: Option<&'static str>,
    /// Whether the field has the keyboard now (set by the window's render, which has the window; see [`HotkeyState::sync`]).
    focused: bool,
    _subs: Vec<Subscription>,
}

impl HotkeyState {
    /// `T` is the view that owns the field: it is redrawn when the field gains or loses the focus.
    pub fn new<T: 'static>(window: &mut Window, cx: &mut Context<T>) -> Self {
        let focus = cx.focus_handle();
        let subs = vec![cx.on_focus(&focus, window, |_, _, cx| cx.notify()), cx.on_focus_out(&focus, window, |_, _, _, cx| cx.notify())];
        Self { focus, message: None, focused: false, _subs: subs }
    }

    /// Called from the owner's render: remembers whether the field has the focus.
    pub fn sync(&mut self, window: &Window) {
        self.focused = self.focus.is_focused(window);
        if !self.focused {
            self.message = None;
        }
    }
}

/// The decision a key press means for a field, from what GPUI tells about the keystroke; `None` for a press the field leaves alone.
pub fn decide_keystroke(k: &Keystroke) -> Option<Decision> {
    let m = &k.modifiers;
    // leaving the field: Tab, and Esc with nothing held
    if (k.key == "tab" || k.key == "escape") && !m.control && !m.alt && !m.platform {
        return None;
    }
    let held = Held { ctrl: m.control, alt: m.alt, shift: m.shift, win: m.platform };
    Some(match hotkey::vk_from_key_name(&k.key) {
        Some(vk) => hotkey::decide(vk, held),
        None => Decision::Refused(Refusal::KeyNotAllowed),
    })
}

/// Makes `focus` the field that takes the keys, calling `on_key` for each press.
pub fn arm(focus: &FocusHandle, on_key: OnKey, window: &mut Window, cx: &mut App) {
    if cx.try_global::<Armed>().is_none() {
        cx.set_global(Armed::default());
        cx.intercept_keystrokes(|ev, window, cx| {
            let Some((handle, on_key)) = cx.try_global::<Armed>().and_then(|a| a.0.clone()) else { return };
            let Some(handle) = handle.upgrade() else { return };
            if !handle.is_focused(window) {
                return;
            }
            let Some(decision) = decide_keystroke(&ev.keystroke) else { return };
            cx.stop_propagation();
            on_key(decision, window, cx);
        })
        .detach();
    }
    cx.global_mut::<Armed>().0 = Some((focus.downgrade(), on_key));
    window.focus(focus, cx);
}

/// The row: the label, the box with the combination (or a hint), and the refusal under it.
/// `word` is the stored hot key; `on_press` arms the field (see [`arm`]).
pub fn row(
    p: &Palette,
    state: &HotkeyState,
    label: &'static str,
    word: u16,
    on_press: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    let focused = state.focused;
    let text = hotkey::text(word);
    let (shown, dim) = match (text.is_empty(), focused) {
        (false, _) => (text, false),
        (true, true) => (t!("Tuşlara basın…").to_string(), true),
        (true, false) => (t!("Yok").to_string(), true),
    };
    h_flex()
        .w_full()
        .gap_3()
        .items_start()
        .py(px(4.))
        .text_sm()
        .child(div().w(px(130.)).flex_shrink_0().h(px(30.)).flex().items_center().text_color(p.text_secondary).child(label))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(
                    div()
                        .id("lnk-hotkey")
                        .track_focus(&state.focus)
                        .h(px(30.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(theme::RADIUS_SM)
                        .bg(p.content_bg)
                        .border_1()
                        .border_color(if state.message.is_some() { p.danger } else if focused { p.accent } else { p.border })
                        .cursor_pointer()
                        .when(dim, |d| d.text_color(p.text_tertiary))
                        .child(shown)
                        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                            on_press(window, cx);
                            cx.stop_propagation();
                        }),
                )
                .when(focused && state.message.is_none(), |d| d.child(div().text_xs().text_color(p.text_tertiary).child(t!("Silmek için Backspace"))))
                .children(state.message.map(|m| div().text_xs().text_color(p.danger).child(m))),
        )
        .into_any_element()
}
