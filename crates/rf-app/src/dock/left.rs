//! The left island: the Windows logo (opens the Start menu), the search field (opens rustyfiles' quick search) and, at its right end,
//! the weather: for the PC's position when "Konumumu kullan" is on and Windows gives it, else for the city the user typed in Settings
//! (the order is in `rf_win::weather_location`).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::weather::{Observation, Outcome};
use rf_win::weather_location::LocatedWeather;

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::plan::{self, WeatherPicture};
use super::start_button::{self, StartButton};
use super::weather_text;
use crate::t;

pub struct LeftIsland {
    glass: Rc<RefCell<IslandGlass>>,
    city: String,
    /// The weather is for the PC's position (the city is the fallback).
    location: bool,
    weather: Option<Observation>,
    /// The display has room for the condition line under the temperature (see `plan::condition_line_fits`); on a narrow display it is
    /// the first thing to go.
    condition_room: bool,
    /// Bumped whenever the source changes: a weather task of an older source ends when it sees it.
    weather_gen: u64,
    /// The width last reported to the host (logical px).
    reported: f32,
    /// The Start button's states and motion (`start_button.rs`), and the clock its events are timed on.
    button: StartButton,
    t0: Instant,
}

impl LeftIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, city: &str, location: bool, cx: &mut Context<Self>) -> Self {
        let mut me = LeftIsland {
            glass,
            city: String::new(),
            location: false,
            weather: None,
            condition_room: true,
            weather_gen: 0,
            reported: plan::left_width(None),
            button: StartButton::new(),
            t0: Instant::now(),
        };
        me.set_weather_source(city, location, cx);
        me
    }

    pub fn width(&self) -> f32 {
        let temp = self.weather.map(|o| o.text());
        plan::left_width_lines(temp.as_deref(), self.condition_text())
    }

    /// The condition in words, when there is weather and the display has room for the line.
    fn condition_text(&self) -> Option<&'static str> {
        self.weather.filter(|_| self.condition_room).and_then(|o| weather_text::code_label(o.code))
    }

    /// Works out whether the display has room for the condition line (told by the host's last layout). True when that changed, so the
    /// island's width has to be reported again.
    fn sync_room(&mut self, cx: &App) -> bool {
        let Some(o) = self.weather else { return false };
        let Some(label) = weather_text::code_label(o.code) else { return false };
        let Some((display, right, middle)) = super::host::weather_room(cx) else { return false };
        let fits = plan::condition_line_fits(display, right, middle, plan::left_width_lines(Some(&o.text()), Some(label)));
        std::mem::replace(&mut self.condition_room, fits) != fits
    }

    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    /// Where the pointer is now relative to the logo (told by the probe in `render` on every mouse move and when the pointer leaves
    /// the window). The toolkit's own hover callback is not used: it reports the pointer as gone on the first mouse move while a
    /// button is down on the element, even one pixel inside it, which would end the press before the button comes up.
    fn logo_pointer(&mut self, inside: bool, cx: &mut Context<Self>) {
        if inside == self.button.is_hovered() {
            return;
        }
        let (now, animate) = (self.now_ms(), crate::settings::animations());
        if inside {
            self.button.enter(now, animate);
        } else {
            self.button.leave(now, animate);
        }
        cx.notify();
    }

    fn logo_press(&mut self, cx: &mut Context<Self>) {
        let (now, animate) = (self.now_ms(), crate::settings::animations());
        self.button.press(now, animate);
        cx.notify();
    }

    /// The mouse button came up on the logo (`inside`) or somewhere else after going down on it.
    fn logo_release(&mut self, inside: bool, cx: &mut Context<Self>) {
        if !self.button.is_pressed() {
            return;
        }
        let (now, animate) = (self.now_ms(), crate::settings::animations());
        self.button.release(now, inside, animate);
        cx.notify();
    }

    /// The Start panel opened or closed (told by `panel.rs`): the logo keeps its plate while it is open, and plays one press and release
    /// when it was opened by the keyboard or the search field.
    pub fn panel_changed(&mut self, open: bool, cx: &mut Context<Self>) {
        let (now, animate) = (self.now_ms(), crate::settings::animations());
        self.button.panel_changed(open, now, animate);
        cx.notify();
    }

    /// A right click at `at` (the window's logical px): on the island's empty glass it starts Task Manager; on the logo or the field it is
    /// theirs and does nothing here.
    fn right_click(&mut self, at: Point<Pixels>, cx: &mut Context<Self>) {
        // While the glass has grown into the Start panel this row is its bottom row: the account's name stands over what is the island's
        // empty glass at rest, and a right click on it is not a request for Task Manager.
        if super::grow_host::island_fade(Kind::Left, cx).is_some() {
            return;
        }
        let k = super::look::dock_size(cx);
        let [ml, mt, ..] = super::host::insets(Kind::Left, cx).margins;
        let point = ((f32::from(at.x) - ml) / k, (f32::from(at.y) - mt) / k);
        if super::empty::side_is_empty(self.width(), &super::empty::left_controls(), point) {
            super::actions::open_task_manager();
        }
    }

    /// Tells the host the width the content needs now. Not while the Start panel is grown out of this island: the grown glass, the place
    /// of its gear and power button and the check that the island does not lie over them (`grow_host::fits`) were made for the width the
    /// island had when the panel opened, and a weather text that came in meanwhile would widen the island's window over them. The
    /// width is told when the growth is over (`render` asks again then).
    fn report_width(&mut self, cx: &mut Context<Self>) {
        self.sync_room(cx);
        let growing = super::grow_host::growing_from(Kind::Left, cx);
        if let Some(w) = width_to_report(self.reported, self.width(), growing) {
            self.reported = w;
            super::host::set_width(Kind::Left, w, cx);
        }
    }

    /// The weather's source changed (or is set for the first time): the city, or the switch for the PC's position. With neither, no
    /// weather is shown and no request of any kind is made (Windows' location service is not asked either).
    pub fn set_weather_source(&mut self, city: &str, location: bool, cx: &mut Context<Self>) {
        let city = city.trim().to_string();
        if city == self.city && location == self.location && self.weather_gen != 0 {
            return;
        }
        self.weather_gen += 1;
        self.city = city.clone();
        self.location = location;
        self.weather = None;
        self.report_width(cx);
        cx.notify();
        if city.is_empty() && !location {
            return;
        }
        let generation = self.weather_gen;
        cx.spawn(async move |this, cx| {
            let mut feed = LocatedWeather::new(&city, location);
            let mut failures = 0u32;
            // when the reading on the screen was fetched (see `plan::keeps_last_reading`)
            let mut good_at: Option<Instant> = None;
            loop {
                // Still the city the user wants? Checked before every request, also after the long wait: a city that was cleared or
                // changed while this task slept must not cost one more request.
                if !matches!(this.update(cx, |me, _| me.weather_gen == generation), Ok(true)) {
                    break;
                }
                // the requests block (and so does Windows' answer about the position): a worker thread; the feed comes back with what it holds
                let (back, outcome) = cx
                    .background_executor()
                    .spawn(async move {
                        let o = feed.poll();
                        (feed, o)
                    })
                    .await;
                feed = back;
                failures = if matches!(outcome, Outcome::Failed) { failures + 1 } else { 0 };
                let observation = outcome.observation();
                let keep = plan::keeps_last_reading(location, &outcome, good_at.map(|t| t.elapsed()));
                if observation.is_some() {
                    good_at = Some(Instant::now());
                }
                let alive = this.update(cx, |me, cx| {
                    if me.weather_gen != generation {
                        return false;
                    }
                    // a miss of the position-based weather leaves the last reading (and the island's width) as it is
                    if !keep {
                        me.weather = observation;
                        me.report_width(cx);
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
                cx.background_executor().timer(plan::next_weather_refresh(&outcome, failures)).await;
            }
        })
        .detach();
    }
}

/// The width (logical px) to tell the host, if any: the content's width when it differs from the one told last, and never while a panel
/// is growing out of the island.
fn width_to_report(reported: f32, wanted: f32, growing: bool) -> Option<f32> {
    (!growing && (wanted - reported).abs() > 0.5).then_some(wanted)
}

fn picture_icon(p: WeatherPicture) -> L {
    match p {
        WeatherPicture::Sun => L::Sun,
        WeatherPicture::Moon => L::Moon,
        WeatherPicture::CloudSun => L::CloudSun,
        WeatherPicture::CloudMoon => L::CloudMoon,
        WeatherPicture::Cloud => L::Cloud,
        WeatherPicture::Fog => L::CloudFog,
        WeatherPicture::Drizzle => L::CloudDrizzle,
        WeatherPicture::Rain => L::CloudRain,
        WeatherPicture::Snow => L::CloudSnow,
        WeatherPicture::Thunder => L::CloudLightning,
        WeatherPicture::Hail => L::CloudHail,
    }
}

/// The Start button's plate corner radius, logical px.
const LOGO_RADIUS: f32 = 8.0;

/// Follows the pointer over the logo's `bounds` for the frame being painted: every mouse move says whether it is inside, and the
/// pointer leaving the window says it is not. Mouse capture while a button is down keeps the moves coming from outside the window.
fn pointer_probe(me: Entity<LeftIsland>, bounds: Bounds<Pixels>, window: &mut Window) {
    let moved = me.clone();
    window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            let inside = bounds.contains(&e.position);
            moved.update(cx, |l, cx| l.logo_pointer(inside, cx));
        }
    });
    window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
        if phase == DispatchPhase::Bubble {
            me.update(cx, |l, cx| l.logo_pointer(false, cx));
        }
    });
}

/// Whether the logo and the search field open our own Start panel (the setting "rustyfiles Başlat menüsünü kullan"; on by default).
fn own_start(cx: &App) -> bool {
    cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.dock_own_start)
}

/// The island's small text: the toolkit's own `text_sm` at the default size (so the default dock is exactly what it was, whatever
/// that style's value is), and 14 dock units times the size factor at any other size.
fn text_14<E: Styled>(el: E, k: f32) -> E {
    if k == 1.0 { el.text_sm() } else { el.text_size(px(14. * k)) }
}

impl Render for LeftIsland {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typed = super::panel::typed_text(cx);
        let (pal, fallback) = super::host::frame_palette(&self.glass, window);
        let pressed = pal.pressed_fill().hsla();
        let secondary = pal.text_secondary().hsla();

        // the Start button: the plate, the logo's size and whether more frames are needed all come from its states at this moment
        let now = self.now_ms();
        let frame = self.button.frame(now);
        if self.button.moving(now) {
            window.request_animation_frame();
        }
        // a layout that changed the display's room (a monitor, a size) may take the condition line away or give it back: the width is
        // reported again from a task, never from inside this render
        if self.sync_room(cx) {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |me, cx| me.report_width(cx));
            })
            .detach();
        }
        let scale_factor = window.scale_factor();
        // While the panel's glass has grown out of this island (grow_host.rs) the island's own glass is put away: the weather fades, the
        // search field is the panel's now (it is not drawn here), and the island paints no fill of its own.
        let grown = super::grow_host::island_fade(Kind::Left, cx);
        if grown.is_some_and(|g| g.moving) {
            window.request_animation_frame();
        }
        let fallback = fallback && grown.is_none();
        // a width that was held back while the panel grew out of this island is told now (not from inside the render: after it)
        if width_to_report(self.reported, self.width(), super::grow_host::growing_from(Kind::Left, cx)).is_some() {
            let me = cx.entity();
            cx.defer(move |cx| me.update(cx, |l, cx| l.report_width(cx)));
        }
        let (others, field_hidden) = grown.map_or((1.0, false), |g| (g.others, g.field_hidden));
        // every length below is a dock unit (the default size's logical px) times the dock's size factor
        let k = super::look::dock_size(cx);
        let me = cx.entity();
        let logo = div()
            .id("dock-logo")
            .relative()
            .size(px(plan::LOGO * k))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .children(start_button::plate(frame.plate, pal.hover_fill(), pal.mix, LOGO_RADIUS * k))
            .child(start_button::logo(frame.scale * k, scale_factor))
            .child(canvas(|_, _, _| (), move |bounds, _, window, _| pointer_probe(me, bounds, window)).absolute().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(|me, _, _, cx| me.logo_press(cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|me, _, _, cx| me.logo_release(true, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|me, _, _, cx| me.logo_release(false, cx)))
            // the user's click is the only thing that ever sends the Windows key (and only when our own Start is switched off)
            .on_click(|_, _, cx| {
                if own_start(cx) {
                    super::panel::toggle_start(cx);
                } else {
                    rf_win::dock::triggers::start_menu();
                }
            });

        let search = h_flex()
            .id("dock-search")
            .ml(px(plan::LOGO_GAP * k))
            .w(px(plan::SEARCH_W * k))
            .h(px(plan::SEARCH_H * k))
            .px(px(12. * k))
            .gap(px(8. * k))
            .items_center()
            .rounded_full()
            .bg(pal.field_fill().hsla())
            // (away: the panel's field stands in its place and the account's picture and name stand over this spot: no hand and no
            // hover plate for a click that does nothing; it keeps its room, so the weather does not move)
            .when(field_hidden, |d| d.opacity(0.0))
            .when(!field_hidden, |d| d.cursor_pointer().hover(move |s| s.bg(pressed)))
            .child(Icon::new(L::Search).size(px(14. * k)).flex_shrink_0().text_color(secondary))
            // what is being typed in the panel above is shown here too; the panel's own field takes the keys (this window is never
            // activated, so it cannot)
            .child(match typed {
                Some(text) => text_14(div().flex_1().min_w_0().truncate(), k).child(text),
                None => text_14(div(), k).text_color(secondary).child(t!("Ara")),
            })
            .when(!field_hidden, |d| {
                d.on_click(|_, _, cx| {
                    if own_start(cx) {
                        super::panel::toggle_start(cx);
                    } else {
                        super::actions::open_quick_search();
                    }
                })
            });

        // the picture at the left of the temperature and, when the display has room, the condition in words under it
        let words = self.condition_text();
        let weather = self.weather.map(|o| {
            // the text column is exactly as wide as the plan says (the island's width is the sum), whatever the real font makes of the
            // words: a longer one is cut with an ellipsis inside the column and never pushes into the search field
            let column = px(plan::weather_text_width(&o.text(), words) * k);
            let text = match words {
                Some(words) => v_flex()
                    .w(column)
                    .flex_shrink_0()
                    .overflow_hidden()
                    .child(text_14(div(), k).line_height(px(17. * k)).child(o.text()))
                    .child(div().text_size(px(11. * k)).line_height(px(14. * k)).text_color(secondary).truncate().child(words))
                    .into_any_element(),
                None => text_14(div(), k).child(o.text()).into_any_element(),
            };
            h_flex()
                .ml(px(plan::WEATHER_GAP * k))
                .when(grown.is_some(), |d| d.opacity(others))
                .gap(px(plan::WEATHER_TEXT_GAP * k))
                .items_center()
                .child(Icon::new(picture_icon(plan::weather_picture(o.code, o.is_day))).size(px(plan::WEATHER_ICON * k)).flex_shrink_0().text_color(pal.text().hsla()))
                .child(text)
        });

        super::host::island_box(pal, fallback, k)
            // a right click on the glass between and around the controls: Windows' Task Manager (the controls are told from the glass
            // by their boxes, `empty.rs::left_controls`, which mirror the row below: padding, logo, gap, field; change one, change both)
            .on_mouse_up(MouseButton::Right, cx.listener(|me, ev: &MouseUpEvent, _, cx| me.right_click(ev.position, cx)))
            .child(h_flex().size_full().px(px(plan::SIDE_PAD * k)).items_center().child(logo).child(search).children(weather))
    }
}

#[cfg(test)]
mod tests {
    use super::{WeatherPicture, picture_icon, width_to_report};

    /// A weather text that comes in while the Start panel is grown out of the island must not widen the island's window (over the gear
    /// and the power button): the width is held back and told once the growth is over.
    #[test]
    fn the_islands_width_is_not_changed_while_a_panel_is_grown_out_of_it() {
        assert_eq!(width_to_report(330.0, 372.0, true), None, "held back while it grows");
        assert_eq!(width_to_report(330.0, 372.0, false), Some(372.0), "told when it is over");
        assert_eq!(width_to_report(330.0, 330.2, false), None, "a change below half a pixel is no change");
        assert_eq!(width_to_report(372.0, 330.0, true), None);
        assert_eq!(width_to_report(372.0, 330.0, false), Some(330.0));
    }

    #[test]
    fn different_conditions_get_different_pictures() {
        use WeatherPicture::*;
        let all = [Sun, Moon, CloudSun, CloudMoon, Cloud, Fog, Drizzle, Rain, Snow, Thunder, Hail];
        let icons: std::collections::HashSet<_> = all.iter().map(|p| format!("{:?}", picture_icon(*p))).collect();
        assert_eq!(icons.len(), all.len(), "two conditions share a picture");
    }
}
