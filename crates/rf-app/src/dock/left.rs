//! The left island: the Windows logo (opens the Start menu), the search field (opens rustyfiles' quick search) and, at its right end,
//! the weather for the city the user typed in Settings.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex};
use gpui_kit::*;
use rf_win::weather::{Observation, Outcome, WeatherFeed};

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::plan::{self, WeatherPicture};
use super::start_button::{self, StartButton};
use crate::t;

pub struct LeftIsland {
    glass: Rc<RefCell<IslandGlass>>,
    city: String,
    weather: Option<Observation>,
    /// Bumped whenever the city changes: a weather task of an older city ends when it sees it.
    weather_gen: u64,
    /// The width last reported to the host (logical px).
    reported: f32,
    /// The Start button's states and motion (`start_button.rs`), and the clock its events are timed on.
    button: StartButton,
    t0: Instant,
}

impl LeftIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, city: &str, cx: &mut Context<Self>) -> Self {
        let mut me = LeftIsland { glass, city: String::new(), weather: None, weather_gen: 0, reported: plan::left_width(None), button: StartButton::new(), t0: Instant::now() };
        me.set_city(city, cx);
        me
    }

    pub fn width(&self) -> f32 {
        plan::left_width(self.weather.map(|o| o.text()).as_deref())
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
        let k = super::look::dock_size(cx);
        let [ml, mt, ..] = super::host::insets(Kind::Left, cx).margins;
        let point = ((f32::from(at.x) - ml) / k, (f32::from(at.y) - mt) / k);
        if super::empty::side_is_empty(self.width(), &super::empty::left_controls(), point) {
            super::actions::open_task_manager();
        }
    }

    fn report_width(&mut self, cx: &mut Context<Self>) {
        let w = self.width();
        if (w - self.reported).abs() > 0.5 {
            self.reported = w;
            super::host::set_width(Kind::Left, w, cx);
        }
    }

    /// The city changed (or is set for the first time). An empty city shows no weather and makes no request at all.
    pub fn set_city(&mut self, city: &str, cx: &mut Context<Self>) {
        let city = city.trim().to_string();
        if city == self.city && self.weather_gen != 0 {
            return;
        }
        self.weather_gen += 1;
        self.city = city.clone();
        self.weather = None;
        self.report_width(cx);
        cx.notify();
        if city.is_empty() {
            return;
        }
        let generation = self.weather_gen;
        cx.spawn(async move |this, cx| {
            let mut feed = WeatherFeed::new(&city);
            let mut failures = 0u32;
            loop {
                // Still the city the user wants? Checked before every request, also after the long wait: a city that was cleared or
                // changed while this task slept must not cost one more request.
                if !matches!(this.update(cx, |me, _| me.weather_gen == generation), Ok(true)) {
                    break;
                }
                // the requests block: a worker thread; the feed comes back with the place it has found
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
                let alive = this.update(cx, |me, cx| {
                    if me.weather_gen != generation {
                        return false;
                    }
                    me.weather = observation;
                    me.report_width(cx);
                    cx.notify();
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
        let scale_factor = window.scale_factor();
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
            .cursor_pointer()
            .hover(move |s| s.bg(pressed))
            .child(Icon::new(L::Search).size(px(14. * k)).flex_shrink_0().text_color(secondary))
            // what is being typed in the panel above is shown here too; the panel's own field takes the keys (this window is never
            // activated, so it cannot)
            .child(match typed {
                Some(text) => text_14(div().flex_1().min_w_0().truncate(), k).child(text),
                None => text_14(div(), k).text_color(secondary).child(t!("Ara")),
            })
            .on_click(|_, _, cx| {
                if own_start(cx) {
                    super::panel::toggle_start(cx);
                } else {
                    super::actions::open_quick_search();
                }
            });

        let weather = self.weather.map(|o| {
            h_flex()
                .ml(px(plan::WEATHER_GAP * k))
                .gap(px(plan::WEATHER_TEXT_GAP * k))
                .items_center()
                .child(Icon::new(picture_icon(plan::weather_picture(o.code, o.is_day))).size(px(plan::WEATHER_ICON * k)).flex_shrink_0().text_color(pal.text().hsla()))
                .child(text_14(div(), k).child(o.text()))
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
    use super::{WeatherPicture, picture_icon};

    #[test]
    fn different_conditions_get_different_pictures() {
        use WeatherPicture::*;
        let all = [Sun, Moon, CloudSun, CloudMoon, Cloud, Fog, Drizzle, Rain, Snow, Thunder, Hail];
        let icons: std::collections::HashSet<_> = all.iter().map(|p| format!("{:?}", picture_icon(*p))).collect();
        assert_eq!(icons.len(), all.len(), "two conditions share a picture");
    }
}
