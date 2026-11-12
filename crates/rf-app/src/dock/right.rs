//! The right island: a network and a volume symbol (they show the real state and open our own control centre, `control_centre.rs`;
//! with "rustyfiles denetim merkezini kullan" off they open Windows' quick settings as before), a thin separator, the time and the date in two
//! lines (they open Windows' notification centre and calendar for now) and a bell (the notification centre). All of it is the user's
//! own click on that element; nothing here ever presses a key by itself.
//!
//! THE SYMBOLS follow the system: the volume symbol is the level in steps, muted, or no sound device; the network symbol is Ethernet,
//! Wi-Fi in four strengths, or not connected (`sysstate.rs` keeps the sources, which call back when Windows says something changed).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use rf_win::dock::clock::{self, ClockLines};

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::look::Palette;
use super::plan;

pub struct RightIsland {
    glass: Rc<RefCell<IslandGlass>>,
    lines: ClockLines,
    reported: f32,
}

impl RightIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, lines: ClockLines, cx: &mut Context<Self>) -> Self {
        let reported = plan::right_width(&lines);
        // the clock follows the wall clock: one wake-up per minute, a moment after the minute changes
        cx.spawn(async move |this, cx| {
            loop {
                let wait = clock::ms_until_next_minute(&clock::local_time());
                cx.background_executor().timer(Duration::from_millis(u64::from(wait) + 50)).await;
                if this.update(cx, |me, cx| me.refresh_clock(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        RightIsland { glass, lines, reported }
    }

    /// Reads the time and the user's formats again (a minute went by, or the system time or zone changed).
    pub fn refresh_clock(&mut self, cx: &mut Context<Self>) {
        let lines = clock::system_lines();
        if lines != self.lines {
            self.lines = lines;
            let w = plan::right_width(&self.lines);
            if (w - self.reported).abs() > 0.5 {
                self.reported = w;
                super::host::set_width(Kind::Right, w, cx);
            }
            cx.notify();
        }
    }
}

fn icon_button(id: &'static str, icon: L, pal: Palette, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
    div()
        .id(id)
        .size(px(plan::BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .child(Icon::new(icon).size(px(16.)).text_color(pal.text().hsla()))
        .on_click(move |_, _, cx| on_click(cx))
}

/// Whether the symbols open our own control centre (the setting "rustyfiles denetim merkezini kullan"; on by default).
fn own_control_centre(cx: &App) -> bool {
    cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.dock_own_control_centre)
}

/// What a click on the network or the volume symbol does.
fn open_controls(cx: &mut App) {
    if own_control_centre(cx) {
        super::panel::toggle_control_centre(cx);
    } else {
        rf_win::dock::triggers::quick_settings();
    }
}

impl Render for RightIsland {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (pal, fallback) = super::host::frame_palette(&self.glass, window);
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());

        let notifications = |_: &mut App| {
            rf_win::dock::triggers::notification_center();
        };
        let (network_icon, volume_icon) = (super::sysstate::network_icon(cx), super::sysstate::volume_icon(cx));

        let clock = v_flex()
            .id("dock-clock")
            .ml(px(plan::RIGHT_SEP_GAP))
            .w(px(plan::clock_width(&self.lines)))
            .h(px(36.))
            .px(px(plan::CLOCK_PAD / 2.0))
            .items_end()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(div().text_size(px(13.)).line_height(px(16.)).child(self.lines.time.clone()))
            .child(div().text_size(px(12.)).line_height(px(15.)).text_color(pal.text_secondary().hsla()).child(self.lines.date.clone()))
            .on_click(move |_, _, cx| notifications(cx));

        super::host::island_box(pal, fallback).child(
            h_flex()
                .size_full()
                .px(px(plan::SIDE_PAD))
                .items_center()
                .child(icon_button("dock-network", network_icon, pal, open_controls))
                .child(div().ml(px(plan::RIGHT_GAP)).child(icon_button("dock-volume", volume_icon, pal, open_controls)))
                .child(div().ml(px(plan::RIGHT_SEP_GAP)).w(px(rf_win::dock::layout::SEPARATOR_WIDTH)).h(px(plan::RIGHT_SEP_HEIGHT)).bg(pal.separator().hsla()))
                .child(clock)
                .child(div().ml(px(plan::RIGHT_GAP)).child(icon_button("dock-bell", L::Bell, pal, notifications))),
        )
    }
}
