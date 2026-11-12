//! The right island: a network and a volume symbol (they show the real state and open our own control centre, `control_centre.rs`;
//! with "rustyfiles denetim merkezini kullan" off they open Windows' quick settings as before), a thin separator, the time and the date in two
//! lines (they open our own calendar panel, `cal_view.rs`; Windows' notification centre and calendar with "rustyfiles takvimini kullan"
//! off) and a bell (our own notification panel, `notif_view.rs`, with a small dot while there are notifications; Windows' notification
//! centre with "rustyfiles bildirim panelini kullan" off). All of it is the user's
//! own click on that element; nothing here ever presses a key by itself.
//!
//! THE SYMBOLS follow the system: the volume symbol is the level in steps, muted, or no sound device; the network symbol is Ethernet,
//! Wi-Fi in four strengths, or not connected (`sysstate.rs` keeps the sources, which call back when Windows says something changed).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
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
    /// What the tray strip adds to the island's width (logical px; 0 without one).
    tray_extra: f32,
    /// The keys, once the keyboard is in the tray strip (`tray_strip::focus_strip`). Made when the island first draws a strip: without
    /// one the island tracks no focus at all, so it behaves as it did before the strip existed.
    focus: Option<FocusHandle>,
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
        RightIsland { glass, lines, reported, tray_extra: 0.0, focus: None }
    }

    /// Reads the time and the user's formats again (a minute went by, or the system time or zone changed).
    pub fn refresh_clock(&mut self, cx: &mut Context<Self>) {
        let lines = clock::system_lines();
        if lines != self.lines {
            self.lines = lines;
            let w = plan::right_width(&self.lines) + self.tray_extra;
            if (w - self.reported).abs() > 0.5 {
                self.reported = w;
                super::host::set_width(Kind::Right, w, cx);
            }
            cx.notify();
        }
    }

    /// The tray strip needs `extra` logical px of the island's width (0: it has gone). All three islands are placed again.
    pub fn set_tray_extra(&mut self, extra: f32, cx: &mut Context<Self>) {
        if (extra - self.tray_extra).abs() < 0.01 {
            return;
        }
        self.tray_extra = extra;
        let w = plan::right_width(&self.lines) + extra;
        if (w - self.reported).abs() > 0.5 {
            self.reported = w;
            super::host::set_width(Kind::Right, w, cx);
        }
        cx.notify();
    }

    /// The island's glass (the tray strip's tips take its tone).
    pub fn island_glass(&self) -> Rc<RefCell<IslandGlass>> {
        self.glass.clone()
    }

    /// The focus the island's keys go to while the keyboard is in the tray strip.
    pub fn focus_handle(&self) -> Option<FocusHandle> {
        self.focus.clone()
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

/// Whether the clock opens our own calendar (the setting "rustyfiles takvimini kullan"; on by default).
fn own_calendar(cx: &App) -> bool {
    cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.dock_own_calendar)
}

/// Whether the bell opens our own notification panel (the setting "rustyfiles bildirim panelini kullan"; on by default).
fn own_notifications(cx: &App) -> bool {
    cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.dock_own_notifications)
}

/// What a click on the clock does: our calendar, or Windows' notification centre and calendar (Win+N) as before.
fn open_calendar(cx: &mut App) {
    if own_calendar(cx) {
        super::panel::toggle_calendar(cx);
    } else {
        rf_win::dock::triggers::notification_center();
    }
}

/// What a click on the bell does: our notification panel, or Windows' notification centre (Win+N) as before.
fn open_notifications(cx: &mut App) {
    if own_notifications(cx) {
        super::panel::toggle_notifications(cx);
    } else {
        rf_win::dock::triggers::notification_center();
    }
}

/// The bell, wearing a small dot in the system accent colour while there are notifications.
fn bell_button(pal: Palette, dot: Option<Hsla>, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
    div()
        .id("dock-bell")
        .relative()
        .size(px(plan::BUTTON))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .child(Icon::new(L::Bell).size(px(16.)).text_color(pal.text().hsla()))
        .when_some(dot, |d, color| d.child(div().absolute().top(px(7.)).right(px(8.)).size(px(8.)).rounded_full().bg(color)))
        .on_click(move |_, _, cx| on_click(cx))
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

        let dot: Option<Hsla> = super::notif_state::dot_visible(cx).then(|| gpui_kit::rgb(cx.try_global::<super::look::Env>().map_or(0x0078D4, |e| e.accent)).into());
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
            .on_click(move |_, _, cx| open_calendar(cx));

        // the other programs' tray icons, with a thin separator after them (nothing at all without a tray source)
        let tray = super::tray_strip::strip_element(pal, cx);

        // (keys are listened to, and focus tracked, only while a strip is drawn)
        let focus = tray.is_some().then(|| self.focus.get_or_insert_with(|| cx.focus_handle()).clone());
        super::host::island_box(pal, fallback)
            .when_some(focus, |d, focus| {
                d.track_focus(&focus).on_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                    if super::tray_strip::key_down(ev, cx) {
                        cx.stop_propagation();
                    }
                }))
            })
            .child(
            h_flex()
                .size_full()
                .px(px(plan::SIDE_PAD))
                .items_center()
                .children(tray)
                .child(icon_button("dock-network", network_icon, pal, open_controls))
                .child(div().ml(px(plan::RIGHT_GAP)).child(icon_button("dock-volume", volume_icon, pal, open_controls)))
                .child(div().ml(px(plan::RIGHT_SEP_GAP)).w(px(rf_win::dock::layout::SEPARATOR_WIDTH)).h(px(plan::RIGHT_SEP_HEIGHT)).bg(pal.separator().hsla()))
                .child(clock)
                .child(div().ml(px(plan::RIGHT_GAP)).child(bell_button(pal, dot, open_notifications))),
        )
    }
}
