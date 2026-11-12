//! The clock's panel: what opens above the right island when its clock is clicked (`panel.rs` opens, places and closes it; this is what
//! is inside). About 340 logical px wide, on HyperGlass, in the look of the control centre's.
//!
//! Top to bottom: today's long date; the month with the buttons to the month before and after (a click on the month's name goes back to
//! today); the month grid (six rows, the week starts on the user's own first day, today filled with the accent, the days of the
//! neighbouring months dimmed, no week numbers); the time WITH SECONDS; and "Tarih ve saat ayarları", which opens Windows' page for it.
//!
//! NOT IN THIS VERSION: an agenda (there is no source of events that works without an account), other time zones, alarms.
//!
//! THE KEYBOARD. The arrow keys move the chosen day (left and right a day, up and down a week), Page Up and Page Down a month, Home
//! goes to today, Escape closes. Tab visits the two month buttons, the month's name and the settings link. The arithmetic is
//! `rf_win::dock::calendar` (pure and tested); [`CalModel`] below holds the chosen day and today.
//!
//! WHAT RUNS. The seconds tick only while the panel is open: one task, ended when the panel is hidden, that wakes at each second.
//! Closed, the panel costs nothing (its window is made at the first click and kept hidden after that).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::calendar::{self, Date, Grid, Key, Lang};
use rf_win::dock::clock;
use rf_win::dock::panel::{self as geo, CloseCause};

use super::backdrop::IslandGlass;
use super::panel_parts::{self, Look};
use crate::glass_watch::{self, PopupGlass};
use crate::t;

/// The panel's width, logical px.
pub const WIDTH: f32 = 340.0;
const PAD: f32 = 16.0;
const CELL_W: f32 = (WIDTH - 2.0 * PAD) / 7.0;
const CELL_H: f32 = 36.0;
const DATE_H: f32 = 28.0;
const NAV_H: f32 = 36.0;
const HEAD_H: f32 = 24.0;
const TIME_H: f32 = 52.0;
const LINK_H: f32 = 40.0;
const GAP: f32 = 8.0;

/// The panel's height, logical px: the same whatever the month, as the grid always has six rows.
pub fn height() -> f32 {
    2.0 * PAD + DATE_H + NAV_H + HEAD_H + 6.0 * CELL_H + TIME_H + LINK_H + 4.0 * GAP
}

pub fn size() -> (f32, f32) {
    (WIDTH, height())
}

/// The language of the names.
pub fn lang() -> Lang {
    if crate::i18n::is_tr() { Lang::Tr } else { Lang::En }
}

/// The chosen day and today: what the keys and the buttons change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CalModel {
    pub chosen: Date,
    pub today: Date,
}

impl CalModel {
    pub fn new(today: Date) -> CalModel {
        CalModel { chosen: today, today }
    }

    /// A key of the grid.
    pub fn key(&mut self, key: Key) {
        self.chosen = calendar::key_target(self.chosen, key, self.today);
    }

    /// The month before (-1) or after (+1) the chosen day's.
    pub fn step_month(&mut self, n: i32) {
        self.chosen = self.chosen.add_months(n);
    }

    /// Whether the month button can go that way (the panel's years end at January 1900 and December 2100).
    pub fn can_step(&self, n: i32) -> bool {
        self.chosen.add_months(n).year != self.chosen.year || self.chosen.add_months(n).month != self.chosen.month
    }

    pub fn go_today(&mut self) {
        self.chosen = self.today;
    }

    pub fn pick(&mut self, d: Date) {
        self.chosen = d.clamped();
    }

    /// The date changed under the open panel (midnight): a panel that was showing today keeps showing today.
    pub fn new_day(&mut self, today: Date) {
        if self.chosen == self.today {
            self.chosen = today;
        }
        self.today = today;
    }

    pub fn grid(&self, first_dow: u8) -> Grid {
        calendar::month_grid(self.chosen.year, self.chosen.month, first_dow, Some(self.today))
    }
}

/// Milliseconds to the next full second (at least 1, so a timer set from it never shows the old second).
pub fn ms_to_next_second(millisecond: u16) -> u64 {
    (1000 - u64::from(millisecond.min(999))).max(1)
}

pub struct CalView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// On the screen or easing out (false while hidden: nothing is built then).
    shown: bool,
    closing: bool,
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    stops: RefCell<HashMap<(&'static str, usize), FocusHandle>>,
    model: CalModel,
    first_dow: u8,
    /// The user's time pattern with seconds, and the AM / PM words, read when the panel opens.
    pattern: String,
    am: String,
    pm: String,
    /// The seconds' ticker; it exists only while the panel is open.
    _tick: Option<Task<()>>,
}

impl CalView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        CalView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            stops: RefCell::new(HashMap::new()),
            model: CalModel::new(calendar::today()),
            first_dow: 0,
            pattern: "HH:mm:ss".into(),
            am: "AM".into(),
            pm: "PM".into(),
            _tick: None,
        }
    }

    fn stop(&self, key: &'static str, i: usize, cx: &mut Context<Self>) -> FocusHandle {
        self.stops.borrow_mut().entry((key, i)).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    /// About to be shown: today, the user's week and time pattern, and the ticker for the seconds.
    pub fn on_open(&mut self, _dark: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = Some((Instant::now(), true));
        self.model = CalModel::new(calendar::today());
        self.first_dow = calendar::first_day_of_week();
        self.pattern = calendar::long_time_pattern();
        let f = clock::system_formats();
        self.am = f.am;
        self.pm = f.pm;
        self.start_ticker(cx);
        cx.notify();
    }

    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        cx.notify();
    }

    /// Hidden: the glass comes off and the ticker ends.
    pub fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self._tick = None;
        cx.notify();
    }

    /// One task that wakes at each second while the panel is open, and ends when it is not.
    fn start_ticker(&mut self, cx: &mut Context<Self>) {
        self._tick = Some(cx.spawn(async move |this, cx| {
            loop {
                let wait = ms_to_next_second(clock::local_time().millisecond) + 5;
                cx.background_executor().timer(Duration::from_millis(wait)).await;
                let alive = this.update(cx, |me, cx| {
                    if !me.shown {
                        return false;
                    }
                    me.model.new_day(calendar::today());
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        }));
    }

    // ------------------------------------------------------------------ doing things (each is the user's own click or key)

    fn live(&self) -> bool {
        self.shown && !self.closing
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        self.closing = true;
        panel_parts::open_uri("ms-settings:dateandtime");
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    fn time_text(&self) -> String {
        clock::format_pattern(&self.pattern, &clock::local_time(), &self.am, &self.pm)
    }

    // ------------------------------------------------------------------ drawing the parts

    fn render_nav(&self, look: &Look, lang: Lang, cx: &mut Context<Self>) -> AnyElement {
        let (prev_ok, next_ok) = (self.model.can_step(-1), self.model.can_step(1));
        let (prev_focus, title_focus, next_focus) = (self.stop("cal-prev", 0, cx), self.stop("cal-title", 0, cx), self.stop("cal-next", 0, cx));
        let (hover, pressed, ring) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla(), look.accent);
        let title = div()
            .id("cal-title")
            .flex_1()
            .min_w_0()
            .h(px(NAV_H - 4.0))
            .px(px(8.))
            .flex()
            .items_center()
            .rounded(px(8.))
            .border_2()
            .border_color(transparent_black())
            .cursor_pointer()
            .text_size(px(14.))
            .font_weight(FontWeight::SEMIBOLD)
            .track_focus(&title_focus)
            .focus_visible(move |s| s.border_color(ring))
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Bugüne dön")).build(w, cx))
            .child(calendar::month_title(self.model.chosen.year, self.model.chosen.month, lang))
            .on_click(cx.listener(|this, _, _, cx| {
                this.model.go_today();
                cx.notify();
            }));
        h_flex()
            .h(px(NAV_H))
            .flex_shrink_0()
            .items_center()
            .gap(px(4.))
            .child(title)
            .child(
                panel_parts::round_button("cal-prev", L::ChevronLeft, 32.0, look, Some(&prev_focus), Some(t!("Önceki ay")), prev_ok).on_click(cx.listener(|this, _, _, cx| {
                    this.model.step_month(-1);
                    cx.notify();
                })),
            )
            .child(
                panel_parts::round_button("cal-next", L::ChevronRight, 32.0, look, Some(&next_focus), Some(t!("Sonraki ay")), next_ok).on_click(cx.listener(|this, _, _, cx| {
                    this.model.step_month(1);
                    cx.notify();
                })),
            )
            .into_any_element()
    }

    fn render_grid(&self, look: &Look, lang: Lang, cx: &mut Context<Self>) -> AnyElement {
        let secondary = look.pal.text_secondary().hsla();
        let hover = look.pal.hover_fill().hsla();
        let mut heads = h_flex().h(px(HEAD_H)).flex_shrink_0();
        for wd in calendar::column_weekdays(self.first_dow) {
            heads = heads.child(div().w(px(CELL_W)).flex().items_center().justify_center().text_xs().text_color(secondary).child(calendar::day_short(wd, lang)));
        }
        let mut rows = v_flex().flex_shrink_0().child(heads);
        for (r, row) in self.model.grid(self.first_dow).iter().enumerate() {
            let mut line = h_flex().h(px(CELL_H)).flex_shrink_0();
            for (c, cell) in row.iter().enumerate() {
                let date = cell.date;
                let chosen = date == self.model.chosen && !cell.today;
                let (text, fill) = if cell.today {
                    (look.on_accent, Some(look.accent))
                } else if cell.in_month {
                    (look.pal.text().hsla(), None)
                } else {
                    (secondary.opacity(0.55), None)
                };
                let ring = look.accent;
                line = line.child(
                    div()
                        .id(("cal-day", r * 7 + c))
                        .w(px(CELL_W))
                        .h(px(CELL_H))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(
                            div()
                                .size(px(CELL_H - 4.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .border_2()
                                .border_color(if chosen { ring } else { transparent_black() })
                                .text_size(px(13.))
                                .text_color(text)
                                .when_some(fill, |d, f| d.bg(f).font_weight(FontWeight::SEMIBOLD))
                                .when(fill.is_none(), |d| d.hover(move |s| s.bg(hover)))
                                .child(date.day.to_string()),
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.model.pick(date);
                            cx.notify();
                        })),
                );
            }
            rows = rows.child(line);
        }
        rows.into_any_element()
    }

    fn render_link(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.stop("cal-settings", 0, cx);
        let (hover, pressed, ring) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla(), look.accent);
        let secondary = look.pal.text_secondary().hsla();
        h_flex()
            .id("cal-settings")
            .h(px(LINK_H))
            .flex_shrink_0()
            .px(px(10.))
            .gap(px(8.))
            .items_center()
            .rounded(px(10.))
            .border_2()
            .border_color(transparent_black())
            .bg(look.pal.field_fill().hsla())
            .cursor_pointer()
            .text_sm()
            .track_focus(&focus)
            .focus_visible(move |s| s.border_color(ring))
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(div().flex_1().min_w_0().truncate().child(t!("Tarih ve saat ayarları")))
            .child(Icon::new(L::ExternalLink).size(px(14.)).text_color(secondary))
            .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx)))
            .into_any_element()
    }
}

impl Render for CalView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let look = Look::of(pal);
        let lang = lang();
        let fill = glass_watch::panel_fill(look.dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let frame = match self.anim {
            Some((t, opening)) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
            None => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
        };
        if !frame.done {
            window.request_animation_frame();
        }

        let body = v_flex()
            .size_full()
            .p(px(PAD))
            .gap(px(GAP))
            .opacity(frame.opacity)
            .mt(px(frame.drop_px))
            .child(div().h(px(DATE_H)).flex_shrink_0().flex().items_center().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(calendar::long_date(self.model.today, lang)))
            .child(self.render_nav(&look, lang, cx))
            .child(self.render_grid(&look, lang, cx))
            .child(
                div()
                    .h(px(TIME_H))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(30.))
                    .font_weight(FontWeight::LIGHT)
                    .text_color(look.pal.text().hsla())
                    .child(self.time_text()),
            )
            .child(self.render_link(&look, cx));
        div()
            .id("calendar-panel")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(look.pal.text().hsla())
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                let key = ev.keystroke.key.as_str();
                if key == "escape" {
                    cx.stop_propagation();
                    cx.defer(|cx| super::panel::close(CloseCause::Escape, cx));
                } else if key == "tab" {
                    cx.stop_propagation();
                    if ev.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                } else if let Some(k) = Key::from_name(key) {
                    cx.stop_propagation();
                    this.model.key(k);
                    cx.notify();
                }
            }))
            .child(body)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // (not `use super::*`: that would bring GPUI's own `test` attribute in and take the place of the built-in one)
    use super::{CalModel, height, ms_to_next_second};
    use rf_win::dock::calendar::{Date, Key};

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn the_panel_opens_on_today_and_home_and_the_title_come_back_to_it() {
        let mut m = CalModel::new(d(2026, 11, 12));
        assert_eq!(m.chosen, d(2026, 11, 12));
        m.key(Key::PageUp);
        m.key(Key::Left);
        assert_eq!(m.chosen, d(2026, 10, 11));
        m.key(Key::Home);
        assert_eq!(m.chosen, m.today);
        m.step_month(3);
        assert_eq!(m.chosen, d(2027, 2, 12));
        m.go_today();
        assert_eq!(m.chosen, m.today);
    }

    #[test]
    fn the_month_buttons_stop_at_the_ends_of_the_years() {
        let mut m = CalModel::new(d(1900, 1, 15));
        assert!(!m.can_step(-1));
        assert!(m.can_step(1));
        m.step_month(-1);
        assert_eq!(m.chosen, d(1900, 1, 1), "held inside the years");
        let mut m = CalModel::new(d(2100, 12, 15));
        assert!(!m.can_step(1));
        assert!(m.can_step(-1));
        m.step_month(1);
        assert_eq!(m.chosen.year, 2100);
        let m = CalModel::new(d(2026, 6, 15));
        assert!(m.can_step(-1) && m.can_step(1));
    }

    #[test]
    fn a_click_on_a_dimmed_day_moves_to_its_month() {
        let mut m = CalModel::new(d(2026, 11, 12));
        let grid = m.grid(0);
        let first = grid[0][0];
        assert!(!first.in_month, "26 October is in the grid of November");
        m.pick(first.date);
        assert_eq!(m.grid(0)[1][0].date.month, 10, "the grid is October's now");
    }

    #[test]
    fn a_panel_left_open_over_midnight_follows_the_day_only_if_it_was_on_today() {
        let mut m = CalModel::new(d(2026, 11, 12));
        m.new_day(d(2026, 11, 13));
        assert_eq!((m.chosen, m.today), (d(2026, 11, 13), d(2026, 11, 13)));
        let mut m = CalModel::new(d(2026, 11, 12));
        m.key(Key::Right);
        m.key(Key::Right);
        m.new_day(d(2026, 11, 13));
        assert_eq!(m.chosen, d(2026, 11, 14), "the user moved away: left where they put it");
        assert_eq!(m.today, d(2026, 11, 13));
    }

    #[test]
    fn the_seconds_tick_on_the_second() {
        assert_eq!(ms_to_next_second(0), 1000);
        assert_eq!(ms_to_next_second(250), 750);
        assert_eq!(ms_to_next_second(999), 1);
        assert_eq!(ms_to_next_second(5000), 1, "a bad reading still waits");
    }

    #[test]
    fn the_height_does_not_depend_on_the_month() {
        // six rows always: February 2026 (four weeks exactly) and a month that needs six rows are the same height
        assert!(height() > 380.0 && height() < 520.0, "{}", height());
        assert_eq!(CalModel::new(d(2026, 2, 1)).grid(0).len(), 6);
        assert_eq!(CalModel::new(d(2026, 8, 1)).grid(0).len(), 6);
    }
}
