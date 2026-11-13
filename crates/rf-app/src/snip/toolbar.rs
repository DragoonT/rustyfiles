//! The snip's toolbar: a small bar at the top centre of the monitor the pointer was on, with the three modes (Bölge, Pencere, Tam ekran)
//! and a close button. It is its own window, above the overlays, on HyperGlass like the dock's panels; until the glass runs (or when it
//! cannot) it paints the same plain panel fill the dock's panels paint, in the dock's tone (light text on dark glass, dark on light),
//! so it is never see-through with nothing behind it.
//!
//! It never takes the foreground (see `mod.rs`): a click on a button is a click, and nothing changes hands.

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::snip::model::Mode;

use super::State;
use crate::dock::look::Palette;
use crate::dock::panel_parts::Look;
use crate::glass_watch::{self, PopupGlass};
use crate::t;

/// The buttons, in order: the mode, its icon, its name.
fn modes() -> [(Mode, L, &'static str); 3] {
    [(Mode::Region, L::Crop, t!("Bölge")), (Mode::Window, L::AppWindow, t!("Pencere")), (Mode::Screen, L::Monitor, t!("Tam ekran"))]
}

pub struct ToolbarView {
    state: Entity<State>,
    hwnd: isize,
    dark: bool,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    _sub: Subscription,
}

impl ToolbarView {
    pub fn new(state: Entity<State>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(&state, |_, _, cx| cx.notify());
        ToolbarView { state, hwnd: crate::look::window::hwnd_of(window), dark: rf_win::accent::system_is_dark(), popup: None, _watch: None, _sub: sub }
    }

    /// The window has its place and is shown: the glass goes behind it. A window that cannot get glass keeps the plain fill.
    pub fn start_glass(&mut self, cx: &mut Context<Self>) {
        if self.popup.is_some() || self.hwnd == 0 {
            return;
        }
        // (the glass would otherwise show a bent copy of this very window behind it)
        crate::glass::keep_out_of_capture(self.hwnd);
        self.popup = Some(PopupGlass::start_panel(self.hwnd, self.dark));
        self._watch = Some(glass_watch::watch(cx, |t: &mut ToolbarView| t.popup.as_mut()));
    }

    /// The window is going.
    pub fn release_glass(&mut self) {
        self._watch = None;
        if let Some(mut g) = self.popup.take() {
            g.release();
        }
    }

    fn set_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        self.state.update(cx, |s, cx| {
            s.model.set_mode(mode);
            cx.notify();
        });
    }
}

impl Render for ToolbarView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let look = Look::of(Palette { mix: if self.dark { 0.0 } else { 1.0 } });
        let fill = glass_watch::panel_fill(self.dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let current = self.state.read(cx).model.mode();
        let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
        let mut bar = h_flex().id("snip-toolbar").size_full().bg(fill).px(px(8.)).gap(px(4.)).items_center().text_color(look.pal.text().hsla()).text_size(px(13.));
        for (i, (mode, icon, label)) in modes().into_iter().enumerate() {
            let on = mode == current;
            bar = bar.child(
                h_flex()
                    .id(("snip-mode", i))
                    .h(px(36.))
                    .px(px(12.))
                    .gap(px(7.))
                    .items_center()
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(look.accent).text_color(look.on_accent))
                    .when(!on, |d| d.hover(move |s| s.bg(hover)).active(move |s| s.bg(pressed)))
                    .child(Icon::new(icon).size(px(16.)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_mode(mode, cx))),
            );
        }
        bar.child(div().w(px(1.)).h(px(22.)).mx(px(4.)).flex_shrink_0().bg(look.pal.separator().hsla())).child(
            h_flex()
                .id("snip-close")
                .size(px(36.))
                .flex_shrink_0()
                .items_center()
                .justify_center()
                .rounded_full()
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Kapat")).build(w, cx))
                .child(Icon::new(L::X).size(px(16.)))
                .on_click(cx.listener(|_, _, _, cx| cx.defer(super::cancel))),
        )
    }
}

#[cfg(test)]
mod tests {
    // (explicit names: a glob would bring in the toolkit's `test` attribute)
    use super::{Mode, modes};

    #[test]
    fn the_buttons_are_the_three_modes_in_the_order_of_the_brief_and_the_region_one_is_the_default() {
        let m = modes();
        assert_eq!(m.map(|(mode, ..)| mode), [Mode::Region, Mode::Window, Mode::Screen]);
        assert_eq!(Mode::default(), Mode::Region);
        crate::dock_page::while_turkish(|| assert_eq!(modes().map(|(.., label)| label), ["Bölge", "Pencere", "Tam ekran"]));
    }
}
