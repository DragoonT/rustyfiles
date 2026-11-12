//! The Start panel's account, gear and power button, drawn in either of the two places they can stand: the panel's own bottom row
//! (`start_home.rs::render_footer`) or, while the left island's glass has grown into the panel, the glass's ONE bottom row, the island's
//! (`grow_row.rs` has the numbers). A child module of `start.rs`; it draws that view's own state and shares its private parts.
//!
//! In the island's row the logo stays where it is and the account's picture and name stand where the island's search field stood; the
//! gear and the power button end at the right edge with the island's own inner margin; the line sits directly above the row. The three
//! come and go with the panel's content (`ViewFrame::content` is their opacity), and the power menu opens just over its button.

use super::*;
use crate::dock::grow_row::BottomRow;

impl StartView {
    /// The account: its round picture (or the initial in a disc until the picture has come, or when the account has none) and its name.
    /// `size` is the picture's side, `text` the name's size and `gap` the space between them, logical px.
    pub(super) fn account(&self, size: f32, text: f32, gap: f32, pal: Palette) -> Div {
        let initial = self.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let picture = match &self.avatar {
            // (the radius is on the picture itself: a parent's overflow_hidden clips to a rectangle and would leave it square)
            Some(img) => div().size(px(size)).flex_shrink_0().child(gpui_kit::img(img.clone()).size(px(size)).rounded_full()).into_any_element(),
            None => div()
                .size(px(size))
                .flex_shrink_0()
                .rounded_full()
                .bg(pal.field_fill().hsla())
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(size * 13.0 / 28.0))
                .text_color(pal.text_secondary().hsla())
                .child(initial)
                .into_any_element(),
        };
        h_flex().flex_1().min_w_0().gap(px(gap)).items_center().child(picture).child(div().min_w_0().truncate().text_size(px(text)).child(self.user.clone()))
    }

    /// The gear: Windows' Settings (as the gear of Windows' own Start does; rustyfiles' settings are in its own window).
    pub(super) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        crate::dock::panel_parts::open_uri("ms-settings:");
        self.finish(cx);
    }

    /// The power button: opens the power menu, closes it when it is open.
    pub(super) fn toggle_power(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        self.app_menu = None;
        self.power_hi = 0;
        let toggled = self.power.toggled();
        self.set_power(toggled, cx);
    }

    /// A button of the bottom row: a square with a rounded plate that shows on hover, a keyboard stop and a tooltip.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn row_button(&self, id: &'static str, icon: L, on: bool, focus: &FocusHandle, tip: &'static str, side: f32, icon_size: f32, radius: f32, pal: Palette, theme: ThemePalette) -> Stateful<Div> {
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
        let ring = theme.accent;
        div()
            .id(id)
            .size(px(side))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(radius))
            // (room for the keyboard ring, which is the border's colour)
            .border_2()
            .border_color(transparent_black())
            .cursor_pointer()
            .track_focus(focus)
            .focus_visible(move |s| s.border_color(ring))
            .when(on, |d| d.bg(pressed))
            .when(!on, |d| d.hover(move |s| s.bg(hover)))
            .active(move |s| s.bg(pressed))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
            .child(Icon::new(icon).size(px(icon_size)).text_color(pal.text().hsla()))
    }

    /// The glass's one bottom row: the line directly above it, then the account where the island's search field stood and the gear and
    /// the power button at the right end, all on the logo's centre line. `opacity` is the panel's content opacity.
    pub(super) fn render_grown_row(&mut self, row: BottomRow, opacity: f32, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        let (settings_focus, power_focus) = (self.settings_focus.clone(), self.power_focus.clone());
        let power_on = self.power != PowerMenu::Closed;
        let gear = self
            .row_button("start-settings", L::Settings, false, &settings_focus, t!("Ayarlar"), row.button, row.icon, row.button_radius, pal, theme)
            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)));
        let power = self
            .row_button("start-power", L::Power, power_on, &power_focus, t!("Güç"), row.button, row.icon, row.button_radius, pal, theme)
            .on_click(cx.listener(|this, _, _, cx| this.toggle_power(cx)));
        div()
            .id("start-bottom-row")
            .absolute()
            .left_0()
            .right_0()
            .top(px(row.top))
            .h(px(row.height))
            .opacity(opacity)
            // the line: the row's top edge, across the whole glass
            .child(div().absolute().top_0().left_0().right_0().h(px(page::DIVIDER_H)).bg(pal.separator().hsla()))
            // (the row's things are centred in the row, whose middle is the logo's: `BottomRow::centre`)
            .child(
                h_flex()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(row.account_left))
                    .right(px(row.right_margin))
                    .gap(px(row.buttons_gap))
                    .items_center()
                    .child(self.account(row.avatar, row.name_text, row.name_gap, pal))
                    .child(gear)
                    .child(power),
            )
            .into_any_element()
    }
}
