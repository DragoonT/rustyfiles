//! The right island: a network and a volume symbol (they show the real state and open our own control centre, `control_centre.rs`;
//! with "rustyfiles denetim merkezini kullan" off they open Windows' quick settings as before), a thin separator, the time and the date in two
//! lines (they open our own calendar panel, `cal_view.rs`; Windows' notification centre and calendar with "rustyfiles takvimini kullan"
//! off) and a bell (our own notification panel, `notif_view.rs`, with a small dot while there are notifications; Windows' notification
//! centre with "rustyfiles bildirim panelini kullan" off). All of it is the user's
//! own click on that element; nothing here ever presses a key by itself.
//!
//! THE SYMBOLS follow the system: the volume symbol is the level in steps, muted, or no sound device; the network symbol is Ethernet,
//! Wi-Fi in four strengths, or not connected (`sysstate.rs` keeps the sources, which call back when Windows says something changed).
//!
//! ON A SECONDARY MONITOR (`mon` is not the home dock) the island holds the clock and the bell only: the tray strip, the network and
//! volume symbols and the separator after them stay on the primary monitor's, as on Windows' secondary taskbars; on a narrow monitor the
//! bell goes too (`rf_win::dock::monitors::content`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::clock::{self, ClockLines};
use rf_win::dock::monitors::Content;

use super::backdrop::IslandGlass;
use super::host::{Kind, MonId};
use super::look::Palette;
use super::plan;

pub struct RightIsland {
    glass: Rc<RefCell<IslandGlass>>,
    /// The monitor whose dock this island is on.
    mon: MonId,
    /// What the host's layout last said this island holds (the bell may be gone on a narrow secondary monitor).
    content: Content,
    lines: ClockLines,
    reported: f32,
    /// What the tray strip adds to the island's width (logical px; 0 without one).
    tray_extra: f32,
    /// The keys, once the keyboard is in the tray strip (`tray_strip::focus_strip`). Made when the island first draws a strip: without
    /// one the island tracks no focus at all, so it behaves as it did before the strip existed.
    focus: Option<FocusHandle>,
    /// The drives have been read once: the first reading also tells the host the island's width with its own buttons.
    drives_read: bool,
}

/// How often the removable drives are looked at (off the UI thread: a few system calls).
const DRIVES_EVERY: Duration = Duration::from_secs(3);

impl RightIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, mon: MonId, lines: ClockLines, content: Content, cx: &mut Context<Self>) -> Self {
        let reported = if mon.is_home() { plan::right_width(&lines) } else { plan::right_width_secondary(plan::clock_width(&lines), content.bell) };
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
        // The island's own safe-removal button (the user asked for it and for Windows Security beside the network and volume
        // symbols, as ours and not as Windows' notification icons, which reach a taskbar that is not Explorer's only by luck). It is
        // there while a removable drive holds a volume and opens OUR panel with the drives (`eject_view.rs`). The drives are read
        // every few seconds on a worker thread, and the island is drawn again only when they changed.
        if mon.is_home() {
            cx.spawn(async move |this, cx| {
                loop {
                    let drives = cx.background_executor().spawn(async { super::eject_view::read_drives() }).await;
                    if this.update(cx, |me, cx| me.set_drives(drives, cx)).is_err() {
                        break;
                    }
                    cx.background_executor().timer(DRIVES_EVERY).await;
                }
            })
            .detach();
        }
        RightIsland { glass, mon, content, lines, reported, tray_extra: 0.0, focus: None, drives_read: false }
    }

    /// How many buttons of its own the island has before the network symbol: Windows Security, and the safe-removal button while
    /// there is something to remove.
    fn sys_buttons_for(&self, removable: bool) -> usize {
        if self.mon.is_home() { 1 + usize::from(removable) } else { 0 }
    }

    fn sys_buttons(&self, cx: &App) -> usize {
        self.sys_buttons_for(!super::eject_view::drives(cx).is_empty())
    }

    fn sys_extra(&self, cx: &App) -> f32 {
        self.sys_buttons(cx) as f32 * (plan::BUTTON + plan::RIGHT_GAP)
    }

    /// The drives were read (by this island's own look every few seconds): kept where the panel reads them too, and when they
    /// changed the island's width and picture follow.
    fn set_drives(&mut self, drives: Vec<(char, String)>, cx: &mut Context<Self>) {
        let changed = super::eject_view::set_drives(drives, cx);
        if changed || !self.drives_read {
            self.drives_read = true;
            self.drives_changed(cx);
        }
    }

    /// The removable drives changed (read here, or after a removal from the panel): the button comes or goes.
    pub fn drives_changed(&mut self, cx: &mut Context<Self>) {
        let w = self.wanted_width(cx);
        if (w - self.reported).abs() > 0.5 {
            self.reported = w;
            super::host::set_width(self.mon, Kind::Right, w, cx);
        }
        cx.notify();
    }

    /// The width the content needs now (logical px). The primary monitor's: the symbols, the clock and the bell, and the tray strip's
    /// room; a secondary monitor's: the clock and, when there is room, the bell.
    fn wanted_width(&self, cx: &App) -> f32 {
        if self.mon.is_home() { plan::right_width(&self.lines) + self.tray_extra + self.sys_extra(cx) } else { plan::right_width_secondary(plan::clock_width(&self.lines), self.content.bell) }
    }

    /// The host's layout may have changed what this island holds (the bell, by the monitor's width): true when it did.
    fn sync_content(&mut self, cx: &App) -> bool {
        let content = super::host::content_on(self.mon, cx);
        std::mem::replace(&mut self.content, content) != content
    }

    /// Reads the time and the user's formats again (a minute went by, or the system time or zone changed).
    pub fn refresh_clock(&mut self, cx: &mut Context<Self>) {
        let lines = clock::system_lines();
        if lines != self.lines {
            self.lines = lines;
            let w = self.wanted_width(cx);
            if (w - self.reported).abs() > 0.5 {
                self.reported = w;
                super::host::set_width(self.mon, Kind::Right, w, cx);
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
        let w = self.wanted_width(cx);
        if (w - self.reported).abs() > 0.5 {
            self.reported = w;
            super::host::set_width(self.mon, Kind::Right, w, cx);
        }
        cx.notify();
    }

    /// A right click at `at` (the window's logical px): on the island's empty glass it starts Task Manager; on a symbol, the clock, the
    /// bell or a tray icon it is theirs and does nothing here.
    fn right_click(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        let k = super::host::size_on(self.mon, cx);
        let [ml, mt, ..] = super::host::insets_on(self.mon, Kind::Right, cx).margins;
        let point = ((f32::from(at.x) - ml) / k, (f32::from(at.y) - mt) / k);
        let controls = if self.mon.is_home() {
            super::empty::right_controls(plan::clock_width(&self.lines), super::tray_strip::shape(cx), self.tray_extra, self.sys_buttons(cx))
        } else {
            super::empty::right_controls_secondary(plan::clock_width(&self.lines), self.content.bell)
        };
        if super::empty::side_is_empty(self.wanted_width(cx), &controls, point) {
            super::actions::open_task_manager();
        }
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

/// (`k` is the dock's size factor: the lengths are dock units.)
fn icon_button(id: &'static str, icon: L, pal: Palette, k: f32, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let (hover, pressed) = (pal.hover_fill().hsla(), pal.press_plate().hsla());
    div()
        .id(id)
        .size(px(plan::BUTTON * k))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8. * k))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .child(Icon::new(icon).size(px(16. * k)).text_color(pal.text().hsla()))
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
fn open_calendar(mon: MonId, cx: &mut App) {
    if own_calendar(cx) {
        super::panel::toggle_calendar_on(mon, cx);
    } else {
        rf_win::dock::triggers::notification_center();
    }
}

/// What a click on the bell does: our notification panel, or Windows' notification centre (Win+N) as before.
fn open_notifications(mon: MonId, cx: &mut App) {
    if own_notifications(cx) {
        super::panel::toggle_notifications_on(mon, cx);
    } else {
        rf_win::dock::triggers::notification_center();
    }
}

/// The bell, wearing a small dot in the system accent colour while there are notifications.
fn bell_button(pal: Palette, dot: Option<Hsla>, k: f32, on_click: impl Fn(&mut App) + 'static) -> impl IntoElement {
    let (hover, pressed) = (pal.hover_fill().hsla(), pal.press_plate().hsla());
    div()
        .id("dock-bell")
        .relative()
        .size(px(plan::BUTTON * k))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8. * k))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .child(Icon::new(L::Bell).size(px(16. * k)).text_color(pal.text().hsla()))
        .when_some(dot, |d, color| d.child(div().absolute().top(px(7. * k)).right(px(8. * k)).size(px(8. * k)).rounded_full().bg(color)))
        .on_click(move |_, _, cx| on_click(cx))
}

/// What a click on the network or the volume symbol does (the symbols are the primary monitor's).
fn open_controls(cx: &mut App) {
    if own_control_centre(cx) {
        super::panel::toggle_control_centre_on(MonId::HOME, cx);
    } else {
        rf_win::dock::triggers::quick_settings();
    }
}

impl Render for RightIsland {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (pal, fallback) = super::host::frame_palette(&self.glass, window);
        // While a panel's glass has grown out of this island (grow_host.rs) the island's own glass is put away and the whole row stays on
        // the panel's glass: the island paints no fill of its own over it (nothing of the row fades, so no frames are asked for).
        let fallback = fallback && super::grow_host::island_fade(self.mon, Kind::Right, cx).is_none();
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.press_plate().hsla());
        // every length below is a dock unit (the default size's logical px) times the dock's size factor
        let k = super::host::size_on(self.mon, cx);
        // what the layout says this island holds at this monitor's width: the width is reported again from a task, never from inside this render
        if self.sync_content(cx) {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |me, cx| {
                    let w = me.wanted_width(cx);
                    if (w - me.reported).abs() > 0.5 {
                        me.reported = w;
                        super::host::set_width(me.mon, Kind::Right, w, cx);
                    }
                });
            })
            .detach();
        }
        let (mon, home) = (self.mon, self.mon.is_home());

        let dot: Option<Hsla> = super::notif_state::dot_visible(cx).then(|| gpui_kit::rgb(cx.try_global::<super::look::Env>().map_or(0x0078D4, |e| e.accent)).into());
        let (network_icon, volume_icon) = (super::sysstate::network_icon(cx), super::sysstate::volume_icon(cx));

        let clock = v_flex()
            .id("dock-clock")
            .ml(px(if home { plan::RIGHT_SEP_GAP } else { 0.0 } * k))
            .w(px(plan::clock_width(&self.lines) * k))
            .h(px(plan::CLOCK_H * k))
            .px(px(plan::CLOCK_PAD / 2.0 * k))
            .items_end()
            .justify_center()
            .rounded(px(8. * k))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(div().text_size(px(13. * k)).line_height(px(16. * k)).child(self.lines.time.clone()))
            .child(div().text_size(px(12. * k)).line_height(px(15. * k)).text_color(pal.text_secondary().hsla()).child(self.lines.date.clone()))
            .on_click(move |_, _, cx| open_calendar(mon, cx));

        // the other programs' tray icons, with a thin separator after them (nothing at all without a tray source); the primary monitor's
        let tray = if home { super::tray_strip::strip_element(pal, cx) } else { None };

        // the safe-removal button: while a removable drive holds a volume; it opens our own panel with the drives
        let ejects: Option<AnyElement> = (home && !super::eject_view::drives(cx).is_empty())
            .then(|| div().mr(px(plan::RIGHT_GAP * k)).child(icon_button("dock-eject", L::Eject, pal, k, super::panel::toggle_eject)).into_any_element());
        // (keys are listened to, and focus tracked, only while a strip is drawn)
        let focus = tray.is_some().then(|| self.focus.get_or_insert_with(|| cx.focus_handle()).clone());
        super::host::island_box(pal, fallback, k)
            // a right click on the glass between and around the controls: Windows' Task Manager (the controls are told from the glass
            // by their boxes, `empty.rs::right_controls`, which mirror the row below in its order and distances: tray strip, network,
            // volume, separator, clock, bell; change one, change both)
            .on_mouse_up(MouseButton::Right, cx.listener(|me, ev: &MouseUpEvent, _, cx| me.right_click(ev.position, cx)))
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
                .px(px(plan::SIDE_PAD * k))
                .items_center()
                .children(tray)
                // (the network and volume symbols and the separator after them are the primary monitor's)
                .when(home, |d| {
                    // the island's own buttons: safe removal (while there is something to remove), then Windows Security
                    d.children(ejects).child(div().mr(px(plan::RIGHT_GAP * k)).child(icon_button("dock-security", L::ShieldCheck, pal, k, |_| super::panel_parts::open_uri("windowsdefender:"))))
                        .child(icon_button("dock-network", network_icon, pal, k, open_controls))
                        .child(div().ml(px(plan::RIGHT_GAP * k)).child(icon_button("dock-volume", volume_icon, pal, k, open_controls)))
                        .child(div().ml(px(plan::RIGHT_SEP_GAP * k)).w(px(rf_win::dock::layout::SEPARATOR_WIDTH * k)).h(px(plan::RIGHT_SEP_HEIGHT * k)).bg(pal.separator().hsla()))
                })
                .child(clock)
                .when(self.content.bell, |d| d.child(div().ml(px(plan::RIGHT_GAP * k)).child(bell_button(pal, dot, k, move |cx| open_notifications(mon, cx))))),
        )
    }
}
