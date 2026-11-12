//! The right island: a network and a volume symbol (they open Windows' quick settings), a thin separator, the time and the date in
//! two lines (they open the notification centre and the calendar) and a bell (the notification centre). All of it is the user's own
//! click on that element; nothing here ever presses a key by itself.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use rf_win::dock::clock::{self, ClockLines};

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::look::{self, Env};
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

fn icon_button(id: &'static str, icon: L, dark: bool, on_click: impl Fn() + 'static) -> impl IntoElement {
    let (hover, pressed) = (look::hover_fill(dark).hsla(), look::pressed_fill(dark).hsla());
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
        .child(Icon::new(icon).size(px(16.)).text_color(look::text(dark).hsla()))
        .on_click(move |_, _, _| on_click())
}

impl Render for RightIsland {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let env = *cx.global::<Env>();
        let fallback = self.glass.borrow().fallback();
        let dark = env.dark;
        let (hover, pressed) = (look::hover_fill(dark).hsla(), look::pressed_fill(dark).hsla());

        let quick_settings = || {
            rf_win::dock::triggers::quick_settings();
        };
        let notifications = || {
            rf_win::dock::triggers::notification_center();
        };

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
            .child(div().text_size(px(12.)).line_height(px(15.)).text_color(look::text_secondary(dark).hsla()).child(self.lines.date.clone()))
            .on_click(move |_, _, _| notifications());

        super::host::island_box(&env, fallback).child(
            h_flex()
                .size_full()
                .px(px(plan::SIDE_PAD))
                .items_center()
                .child(icon_button("dock-network", L::Wifi, dark, quick_settings))
                .child(div().ml(px(plan::RIGHT_GAP)).child(icon_button("dock-volume", L::Volume2, dark, quick_settings)))
                .child(div().ml(px(plan::RIGHT_SEP_GAP)).w(px(rf_win::dock::layout::SEPARATOR_WIDTH)).h(px(plan::RIGHT_SEP_HEIGHT)).bg(look::separator(dark).hsla()))
                .child(clock)
                .child(div().ml(px(plan::RIGHT_GAP)).child(icon_button("dock-bell", L::Bell, dark, notifications))),
        )
    }
}
