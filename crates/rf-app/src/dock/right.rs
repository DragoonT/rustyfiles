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
//! THE ISLAND'S OWN BUTTONS are those two symbols, Windows Security and safe removal (while a removable drive holds a volume). Their
//! order and which of them are shown are the user's (Settings > Dock > "Durum simgeleri"; `rf_win::dock::status_order`). A button
//! can also be DRAGGED to another place among them, as the programs' tray icons beside them are: the press is followed by looks at
//! the pointer (`tray_strip::step`, the same rule and the same four logical px before a press is a drag), the dragged button is drawn
//! faint, a thin mark in the accent colour shows where it would land, and the release writes the new order to the settings. A press
//! that never became a drag is the button's click.
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
use rf_win::dock::panel as geo;
use rf_win::dock::status_order::{self, Status};
use rf_win::dock::tray_strip::{self as ts, PointerButton, Press, Step};

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
    /// One of the island's own buttons is held, or dragged to another place.
    status_press: Option<StatusPress>,
    /// The input language of the window in front and how many keyboard layouts the user has (`rf_win::inputlang`; the primary
    /// monitor's island reads it). The button is there with two layouts or more.
    lang: rf_win::inputlang::Current,
    /// The own control the pointer rests on, for its tooltip (`own_hover`).
    tip_on: Option<OwnTip>,
    /// A part of a wheel notch turned over the volume symbol and not used yet (a touchpad turns in small parts).
    vol_rest: f32,
}

/// A button held on one of the island's own buttons, from the press to the release.
struct StatusPress {
    which: Status,
    press: Press,
    /// While it is dragged: where it would be put, an index into the buttons the island shows (`status_order::drop_index`); `None`
    /// while the pointer is away from the island (a release there changes nothing).
    target: Option<usize>,
}

/// How often the pointer is looked at while one of the island's own buttons is held (the tray strip's rate).
const STATUS_POLL: Duration = Duration::from_millis(16);

/// How often the removable drives are looked at (off the UI thread: a few system calls).
const DRIVES_EVERY: Duration = Duration::from_secs(3);

/// How often the input language of the window in front is looked at while the user has more than one keyboard layout (off the UI
/// thread: three system calls; the island hears of it only when it changed).
const LANG_EVERY: Duration = Duration::from_millis(350);
/// The same with one layout: only whether a second one was added.
const LANG_IDLE: Duration = Duration::from_secs(5);
/// The longest the worker looks without telling the island anything (the island may be gone: then the looking ends).
const LANG_ROUND: Duration = Duration::from_secs(10);

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
            // The input language of the window in front (issue 21: "i don't see language switcher icon"). Read on a worker, which
            // comes back when it changed (or after a while, so that the looking ends with the island).
            cx.spawn(async move |this, cx| {
                let mut last = rf_win::inputlang::Current::default();
                loop {
                    let (prev, timers) = (last.clone(), cx.background_executor().clone());
                    let now = cx
                        .background_executor()
                        .spawn(async move {
                            let began = std::time::Instant::now();
                            loop {
                                let now = read_language();
                                // (no window in front for a moment is not a change of language)
                                let same = now.layouts == prev.layouts && (now.code == prev.code || now.code.is_empty());
                                if !same || began.elapsed() >= LANG_ROUND {
                                    return now;
                                }
                                timers.timer(if now.layouts >= 2 { LANG_EVERY } else { LANG_IDLE }).await;
                            }
                        })
                        .await;
                    let Ok(kept) = this.update(cx, |me, cx| me.set_language(now, cx)) else { break };
                    last = kept;
                }
            })
            .detach();
        }
        RightIsland { glass, mon, content, lines, reported, tray_extra: 0.0, focus: None, drives_read: false, status_press: None, lang: Default::default(), vol_rest: 0.0, tip_on: None }
    }

    /// The input language was read: the button follows it. A reading without a window in front keeps the name that was shown.
    /// Returns what the island now holds.
    fn set_language(&mut self, mut now: rf_win::inputlang::Current, cx: &mut Context<Self>) -> rf_win::inputlang::Current {
        if now.code.is_empty() && now.layouts >= 2 {
            now.code = self.lang.code.clone();
        }
        if now != self.lang {
            self.lang = now;
            // (the button comes and goes with the second layout: the island's width follows, as with the drives)
            self.drives_changed(cx);
        }
        self.lang.clone()
    }

    /// The island's own buttons to draw now, in the user's order and without the ones the user hid (the settings `dock_status_order`
    /// and `dock_status_hidden`; `status_order::shown`). Safe removal is among them only while there is something to remove. They are
    /// the primary monitor's: a secondary monitor's island has none.
    fn status_buttons(&self, cx: &App) -> Vec<Status> {
        if !self.mon.is_home() {
            return Vec::new();
        }
        let (removable, languages) = (!super::eject_view::drives(cx).is_empty(), self.lang.layouts >= 2);
        match cx.try_global::<crate::settings::Settings>() {
            Some(s) => status_order::shown(&s.dock_status_order, &s.dock_status_hidden, removable, languages),
            None => status_order::shown(&[], &[], removable, languages),
        }
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

    /// The island's own buttons on the screen (device px): the first one's left edge, the distance from one's left edge to the next
    /// one's, a button's width, and the glass they stand in as it is drawn now.
    fn status_geo(&self, cx: &App) -> Option<(i32, i32, i32, rf_win::dock::layout::Rect)> {
        let glass = super::host::current_glass_rect_on(self.mon, Kind::Right, cx)?;
        let unit = super::host::dock_unit_on(self.mon, cx)?;
        let u = |v: f32| rf_win::dock::layout::px(v, unit);
        Some((glass.left + u(plan::SIDE_PAD + self.tray_extra), u(plan::BUTTON + plan::RIGHT_GAP), u(plan::BUTTON), glass))
    }

    /// Where the pointer at `cursor` would put a dragged button: `None` when it is away from the island (more than the island's
    /// height above or below it, or more than a button beside it).
    fn status_target(&self, cursor: (i32, i32), cx: &App) -> Option<usize> {
        let (first, step, button, glass) = self.status_geo(cx)?;
        let near = cursor.1 >= glass.top - glass.height() && cursor.1 < glass.bottom + glass.height() && cursor.0 >= glass.left - step && cursor.0 < glass.right + step;
        near.then(|| status_order::drop_index(self.status_buttons(cx).len(), first, step, button, cursor.0))
    }

    /// Whether the pointer at `cursor` is on this button.
    fn status_hit(&self, which: Status, cursor: (i32, i32), cx: &App) -> bool {
        let Some((first, step, button, glass)) = self.status_geo(cx) else { return false };
        let Some(i) = self.status_buttons(cx).iter().position(|s| *s == which) else { return false };
        rf_win::dock::layout::Rect::from_xywh(first + i as i32 * step, glass.top + (glass.height() - button) / 2, button, button).contains(cursor.0, cursor.1)
    }

    /// The left button went down on one of the island's own buttons: the pointer is looked at until it is up again.
    fn status_down(&mut self, which: Status, count: usize, cx: &mut Context<Self>) {
        // (a second button pressed while one is held is ignored)
        if self.status_press.is_some() {
            return;
        }
        let Some(at) = geo::cursor() else { return };
        self.status_press = Some(StatusPress { which, press: Press { button: PointerButton::Left, count, at, dragging: false }, target: None });
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(STATUS_POLL).await;
                if !this.update(cx, |me, cx| me.status_poll(cx)).unwrap_or(false) {
                    break;
                }
            }
        })
        .detach();
    }

    /// One look at the held button. False when the press is over: a drag that ended on the island wrote the new order, a press that
    /// never became a drag and ended on its button was that button's click.
    fn status_poll(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(mut sp) = self.status_press.take() else { return false };
        let cursor = geo::cursor().unwrap_or(sp.press.at);
        let scale = super::host::home_work(cx).map_or(1.0, |(s, _)| s);
        match ts::step(&mut sp.press, cursor, geo::any_button_down(), scale) {
            Step::Idle => {
                self.status_press = Some(sp);
                true
            }
            step @ (Step::DragBegan | Step::DragMove) => {
                let target = self.status_target(cursor, cx);
                if step == Step::DragBegan || target != sp.target {
                    cx.notify();
                }
                sp.target = target;
                self.status_press = Some(sp);
                true
            }
            Step::Released { dragged } => {
                if dragged {
                    let new = self.status_target(cursor, cx).and_then(|insert| {
                        let saved = cx.try_global::<crate::settings::Settings>().map(|s| s.dock_status_order.clone()).unwrap_or_default();
                        status_order::dropped(&saved, &self.status_buttons(cx), sp.which, insert)
                    });
                    if let Some(new) = new {
                        // (this process's own change of the settings: the save writes this key on top of the file as it is now)
                        crate::settings::update(cx, move |s| s.dock_status_order = new);
                    }
                } else if self.status_hit(sp.which, cursor, cx) {
                    run_status(sp.which, cx);
                }
                cx.notify();
                false
            }
        }
    }

    /// The island's own buttons changed (the removable drives were read here or one was removed from the panel, or the settings gave
    /// them another order or hid one): the island's width and its picture follow.
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
        if self.mon.is_home() {
            plan::right_width_for(&self.lines, self.status_buttons(cx).len()) + self.tray_extra + if show_desktop_on(cx) { plan::SHOW_DESKTOP_EXTRA } else { 0.0 }
        } else {
            plan::right_width_secondary(plan::clock_width(&self.lines), self.content.bell)
        }
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
            let mut c = super::empty::right_controls(plan::clock_width(&self.lines), super::tray_strip::shape(cx), self.tray_extra, self.status_buttons(cx).len());
            // the "Show desktop" strip after the bell (the last box), the island's full height
            if show_desktop_on(cx)
                && let Some(bell) = c.last().copied()
            {
                c.push([bell[0] + bell[2] + plan::RIGHT_GAP, 0.0, plan::SHOW_DESKTOP_W, super::host::side_height_on(self.mon, cx)]);
            }
            c
        } else {
            super::empty::right_controls_secondary(plan::clock_width(&self.lines), self.content.bell)
        };
        if super::empty::side_is_empty(self.wanted_width(cx), super::host::side_height_on(self.mon, cx), &controls, point) {
            super::actions::open_task_manager();
        }
    }

    /// The pointer rests on (`on`) or left one of the island's own controls: the tray's tooltip window shows its name above it, as
    /// Windows' taskbar does for its symbols and its clock (issue 147: "The icons in the right dock don't show a tooltip when hovering
    /// over them"). A toolkit tooltip cannot be used: it is drawn inside the island's window, which is cut to the island's shape.
    fn own_hover(&mut self, which: OwnTip, on: bool, cx: &mut Context<Self>) {
        if !on {
            // (moving from one control to the next is a leave and then an enter: the tip is told only if nothing was entered)
            self.tip_on = self.tip_on.filter(|t| *t != which);
            cx.spawn(async move |this, cx| {
                let still = this.update(cx, |me, _| me.tip_on.is_some()).unwrap_or(false);
                if !still {
                    let _ = cx.update(|cx| super::tray_tip::hover(None, cx));
                }
            })
            .detach();
            return;
        }
        self.tip_on = Some(which);
        let anchor = self.own_anchor(which, cx);
        super::tray_tip::hover(anchor, cx);
    }

    /// Where one of the island's own controls is on the screen (device px), with its tip's text.
    fn own_anchor(&self, which: OwnTip, cx: &App) -> Option<super::tray_tip::Anchor> {
        let glass = super::host::current_glass_rect_on(self.mon, Kind::Right, cx)?;
        let unit = rf_win::dock::size::unit(super::host::home_work(cx)?.0, super::look::dock_size(cx));
        let clock_w = plan::clock_width(&self.lines);
        let (controls, first_button) = if self.mon.is_home() {
            let shape = super::tray_strip::shape(cx);
            let cells = shape.map_or(0, |s| s.visible + usize::from(s.chevron));
            (super::empty::right_controls(clock_w, shape, self.tray_extra, self.status_buttons(cx).len()), cells)
        } else {
            (super::empty::right_controls_secondary(clock_w, self.content.bell), 0)
        };
        let buttons = self.status_buttons(cx);
        let clock_ix = if self.mon.is_home() { first_button + buttons.len() } else { 0 };
        let (ix, text) = match which {
            OwnTip::Status(s) => (first_button + buttons.iter().position(|b| *b == s)?, super::status_card::name(s).to_string()),
            OwnTip::Clock => (clock_ix, rf_win::dock::clock::long_date().unwrap_or_else(|| self.lines.date.clone())),
            OwnTip::Bell => (clock_ix + 1, crate::t!("Bildirimler").to_string()),
            OwnTip::ShowDesktop => (clock_ix + 2, crate::t!("Masaüstünü göster").to_string()),
        };
        let b = match (which, controls.get(ix).copied()) {
            (_, Some(b)) => b,
            // (the "Show desktop" strip is not among the controls: it stands after the bell, the island's full height)
            (OwnTip::ShowDesktop, None) => {
                let bell = controls.last().copied()?;
                [bell[0] + bell[2] + plan::RIGHT_GAP, 0.0, plan::SHOW_DESKTOP_W, super::host::side_height_on(self.mon, cx)]
            }
            _ => return None,
        };
        let d = |v: f32| rf_win::dock::layout::px(v, unit);
        let rect = rf_win::dock::layout::Rect::from_xywh(glass.left + d(b[0]), glass.top + d(b[1]), d(b[2]), d(b[3]));
        Some(super::tray_tip::Anchor { key: which.key(), text, rect, clear_top: glass.top.min(rect.top) })
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

/// What a click on one of the island's own buttons does: safe removal opens our own panel with the drives, Windows Security opens
/// that app, the input language asks the window in front for the next keyboard layout (the island does not take the focus, so the
/// window the user types in is still in front; the button shows the new language when the window has changed it), the network and the
/// volume symbol open the control centre.
fn run_status(which: Status, cx: &mut App) {
    match which {
        Status::Eject => super::panel::toggle_eject(cx),
        Status::Security => super::panel_parts::open_uri("windowsdefender:"),
        Status::Language => match rf_win::inputlang::switch_foreground_to_next() {
            // the button shows the new language at once; the next reading confirms it (or shows the old one again when the window
            // did not take the request)
            Some(code) => {
                if let Some(island) = super::host::right_island_on(super::host::MonId::HOME, cx) {
                    island.update(cx, |me, cx| {
                        let mut now = me.lang.clone();
                        now.code = code;
                        me.set_language(now, cx);
                    });
                }
            }
            None => log::info!("dock: the input language was not switched (no window in front, or one layout)"),
        },
        Status::Network | Status::Volume => open_controls(cx),
    }
}

/// One of the island's own buttons as it looks; what it does is `run_status`, started by the island when a press ends on it
/// (`RightIsland::status_poll`), because the same press may turn into a drag. (`k` is the dock's size factor: the lengths are dock
/// units.)
fn icon_button(id: &'static str, icon: L, pal: Palette, k: f32) -> impl IntoElement {
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
}

/// The input language as the island reads it. `RUSTYFILES_DOCK_LANG=ENG` stands in for a second keyboard layout on a PC that has one,
/// so that the button can be looked at there (a developer's switch: a click still asks the window in front, which then has nothing
/// to switch to).
fn read_language() -> rf_win::inputlang::Current {
    match std::env::var("RUSTYFILES_DOCK_LANG") {
        Ok(code) if !code.trim().is_empty() => rf_win::inputlang::Current { layouts: 2, code: rf_win::inputlang::short_code(&code, "") },
        _ => rf_win::inputlang::current(),
    }
}

/// The input-language button: the language's short name ("TUR", "ENG") in a button of the others' size, as Windows' taskbar writes it.
/// Before the first reading with a window in front there is no name yet: the symbol stands in.
fn lang_button(code: &str, pal: Palette, k: f32) -> gpui::AnyElement {
    if code.is_empty() {
        return icon_button("dock-language", L::Languages, pal, k).into_any_element();
    }
    let (hover, pressed) = (pal.hover_fill().hsla(), pal.press_plate().hsla());
    div()
        .id("dock-language")
        .size(px(plan::BUTTON * k))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8. * k))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .active(move |s| s.bg(pressed))
        .text_size(px(11. * k))
        .line_height(px(14. * k))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(pal.text().hsla())
        .child(SharedString::from(code.to_string()))
        .into_any_element()
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

/// The setting "Masaüstünü göster şeridi" (issue 127).
fn show_desktop_on(cx: &App) -> bool {
    cx.try_global::<crate::settings::Settings>().is_some_and(|s| s.dock_show_desktop)
}

/// The "Show desktop" strip: a thin bar with a hairline on its left, as at the end of Windows' taskbar. A click minimizes every window
/// and shows the desktop, the next one brings them back (Win+D's own toggle, `shell::keylayer`).
fn show_desktop_strip(pal: Palette, k: f32) -> impl IntoElement {
    div()
        .id("dock-show-desktop")
        .ml(px(plan::RIGHT_GAP * k))
        .w(px(plan::SHOW_DESKTOP_W * k))
        .h(px(plan::BUTTON * k))
        .flex_shrink_0()
        .rounded(px(3. * k))
        .border_l_1()
        .border_color(pal.separator().hsla())
        .hover(move |s| s.bg(pal.hover_fill().hsla()))
        .on_click(|_, _, _| crate::shell::keylayer::toggle_show_desktop())
}

/// One of the island's own controls that has a tooltip (`RightIsland::own_hover`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OwnTip {
    Status(Status),
    Clock,
    Bell,
    ShowDesktop,
}

impl OwnTip {
    /// The tip machine's key: apart from every tray icon's (theirs are hashes; the chevron's is `u64::MAX`).
    fn key(self) -> super::tray_source::IconKey {
        let n = match self {
            OwnTip::Status(s) => 4 + s as u64,
            OwnTip::Clock => 1,
            OwnTip::Bell => 2,
            OwnTip::ShowDesktop => 3,
        };
        u64::MAX - 0x100 - n
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

impl RightIsland {
    /// The wheel was turned over the volume symbol: the system's volume goes 2 % up or down a notch, as over Windows' own symbol (issue
    /// 136). Muting is not touched (Windows does not either).
    fn volume_scrolled(&mut self, ev: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => if p.y != 0.0 { p.y } else { p.x },
            ScrollDelta::Pixels(p) => f32::from(p.y) / 16.0,
        };
        let notches = rf_win::appvolume::take_notches(&mut self.vol_rest, rf_win::appvolume::notches_of_lines(lines, rf_win::appvolume::wheel_lines()));
        let Some(now) = rf_win::sysstatus::audio::snapshot().filter(|s| s.available) else { return };
        if notches == 0 {
            return;
        }
        rf_win::sysstatus::audio::set_volume(volume_after_notches(now.volume, notches));
        cx.stop_propagation();
    }
}

/// The volume after `notches` wheel notches from `volume` (0.0 to 1.0): 2 % a notch, as Windows' own symbol does.
fn volume_after_notches(volume: f32, notches: i32) -> f32 {
    ((volume * 50.0).round() + notches as f32).clamp(0.0, 50.0) / 50.0
}

/// What a click on the network or the volume symbol does (the symbols are the primary monitor's).
fn open_controls(cx: &mut App) {
    if own_control_centre(cx) {
        super::panel::toggle_control_centre_on(MonId::HOME, cx);
    } else {
        rf_win::dock::triggers::quick_settings();
    }
}

/// A right click on the volume symbol: the output devices, to choose the one Windows plays to (issue 22). Our control centre opens
/// with its list of them open; with Windows' own quick settings in use, it is Windows' sound page.
fn open_output_devices(cx: &mut App) {
    if own_control_centre(cx) {
        super::panel::show_output_devices_on(MonId::HOME, cx);
    } else {
        super::panel_parts::open_uri("ms-settings:sound");
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
        // (also when a setting changed what it holds: the "Show desktop" strip)
        if self.sync_content(cx) || (self.wanted_width(cx) - self.reported).abs() > 0.5 {
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

        // the island's own buttons, in the user's order (none on a secondary monitor's island, and none when the user hid them all)
        let buttons = self.status_buttons(cx);
        let clock = v_flex()
            .id("dock-clock")
            // (the gap after the separator that follows the buttons; without buttons there is no separator)
            .ml(px(if buttons.is_empty() { 0.0 } else { plan::RIGHT_SEP_GAP } * k))
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
            .on_hover(cx.listener(|me, on: &bool, _, cx| me.own_hover(OwnTip::Clock, *on, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| super::tray_tip::press(cx))
            .on_click(move |_, _, cx| open_calendar(mon, cx));

        // The island's own buttons with a gap between two of them. One that is being dragged is faint, and a thin mark in the accent
        // colour stands where it would land (as in the tray strip beside them).
        let dragged = self.status_press.as_ref().filter(|p| p.press.dragging).map(|p| (p.which, p.target));
        let status_row = (!buttons.is_empty()).then(|| {
            let mut row = h_flex().relative().flex_shrink_0().items_center();
            for (i, which) in buttons.iter().copied().enumerate() {
                let button = match which {
                    Status::Eject => icon_button("dock-eject", L::Eject, pal, k).into_any_element(),
                    Status::Security => icon_button("dock-security", L::ShieldCheck, pal, k).into_any_element(),
                    Status::Language => lang_button(&self.lang.code, pal, k),
                    Status::Network => icon_button("dock-network", network_icon, pal, k).into_any_element(),
                    Status::Volume => icon_button("dock-volume", volume_icon, pal, k).into_any_element(),
                };
                row = row.child(
                    div()
                        .ml(px(if i == 0 { 0.0 } else { plan::RIGHT_GAP } * k))
                        .when(dragged.is_some_and(|(w, _)| w == which), |d| d.opacity(0.35))
                        .id(("dock-status", i))
                        .on_hover(cx.listener(move |me, on: &bool, _, cx| me.own_hover(OwnTip::Status(which), *on, cx)))
                        .on_mouse_down(MouseButton::Left, cx.listener(move |me, e: &MouseDownEvent, _, cx| {
                            super::tray_tip::press(cx);
                            me.status_down(which, e.click_count, cx)
                        }))
                        // a right click on the volume symbol: the output devices, to choose from
                        .when(which == Status::Volume, |d| d.on_mouse_up(MouseButton::Right, |_, _, cx| open_output_devices(cx)))
                        // the wheel over it: the system's volume, as over Windows' own symbol (issue 136)
                        .when(which == Status::Volume, |d| d.on_scroll_wheel(cx.listener(|me, ev: &ScrollWheelEvent, _, cx| me.volume_scrolled(ev, cx))))
                        .child(button),
                );
            }
            if let Some((which, Some(insert))) = dragged
                && let Some(at) = buttons.iter().position(|b| *b == which)
                && status_order::drop_moves(at, insert)
            {
                let x = insert as f32 * (plan::BUTTON + plan::RIGHT_GAP) - plan::RIGHT_GAP / 2.0;
                let accent: Hsla = gpui_kit::rgb(cx.try_global::<super::look::Env>().map_or(0x0078D4, |e| e.accent)).into();
                row = row.child(div().absolute().left(px(x * k - 1.0)).top(px(4. * k)).w(px(2.)).h(px((plan::BUTTON - 8.) * k)).rounded_full().bg(accent));
            }
            row
        });

        // the other programs' tray icons, with a thin separator after them (nothing at all without a tray source); the primary monitor's
        let tray = if home { super::tray_strip::strip_element(pal, cx) } else { None };

        // (keys are listened to, and focus tracked, only while a strip is drawn)
        let focus = tray.is_some().then(|| self.focus.get_or_insert_with(|| cx.focus_handle()).clone());
        super::host::island_box(pal, fallback, k)
            // a right click on the glass between and around the controls: Windows' Task Manager (the controls are told from the glass
            // by their boxes, `empty.rs::right_controls`, which mirror the row below in its order and distances: tray strip, the
            // island's own buttons, separator, clock, bell; change one, change both)
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
                // the island's own buttons (`status_row` above), then the separator
                .children(status_row)
                .when(!buttons.is_empty(), |d| {
                    d.child(div().ml(px(plan::RIGHT_SEP_GAP * k)).w(px(rf_win::dock::layout::SEPARATOR_WIDTH * k)).h(px(plan::RIGHT_SEP_HEIGHT * k)).bg(pal.separator().hsla()))
                })
                .child(clock)
                .when(self.content.bell, |d| {
                    d.child(
                        div()
                            .id("dock-bell-tip")
                            .ml(px(plan::RIGHT_GAP * k))
                            .on_hover(cx.listener(|me, on: &bool, _, cx| me.own_hover(OwnTip::Bell, *on, cx)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| super::tray_tip::press(cx))
                            .child(bell_button(pal, dot, k, move |cx| open_notifications(mon, cx))),
                    )
                })
                .when(home && show_desktop_on(cx), |d| {
                    d.child(
                        div()
                            .id("dock-show-desktop-tip")
                            .on_hover(cx.listener(|me, on: &bool, _, cx| me.own_hover(OwnTip::ShowDesktop, *on, cx)))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| super::tray_tip::press(cx))
                            .child(show_desktop_strip(pal, k)),
                    )
                }),
        )
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_wheel_notch_over_the_volume_symbol_is_two_percent() {
        assert_eq!(super::volume_after_notches(0.40, 1), 0.42);
        assert_eq!(super::volume_after_notches(0.40, -3), 0.34);
        assert_eq!(super::volume_after_notches(0.99, 2), 1.0);
        assert_eq!(super::volume_after_notches(0.01, -1), 0.0);
        // a level between steps comes to the nearest step first
        assert_eq!(super::volume_after_notches(0.411, 1), 0.44, "0.411 is taken as the step 0.42, one notch up");
    }
}
