//! The left island: the Windows logo (opens the Start menu), the search field (opens rustyfiles' quick search) and, at its right end,
//! the weather for the city the user typed in Settings.

use std::cell::RefCell;
use std::rc::Rc;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::*;
use rf_win::weather::{Observation, Outcome, WeatherFeed};

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::look::{self, Env};
use super::plan::{self, WeatherPicture};
use crate::t;

pub struct LeftIsland {
    glass: Rc<RefCell<IslandGlass>>,
    city: String,
    weather: Option<Observation>,
    /// Bumped whenever the city changes: a weather task of an older city ends when it sees it.
    weather_gen: u64,
    /// The width last reported to the host (logical px).
    reported: f32,
}

impl LeftIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, city: &str, cx: &mut Context<Self>) -> Self {
        let mut me = LeftIsland { glass, city: String::new(), weather: None, weather_gen: 0, reported: plan::left_width(None) };
        me.set_city(city, cx);
        me
    }

    pub fn width(&self) -> f32 {
        plan::left_width(self.weather.map(|o| o.text()).as_deref())
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

/// The Windows logo: four squares in the accent colour.
fn windows_logo(accent: Hsla) -> impl IntoElement {
    let square = move || div().size(px(8.)).bg(accent);
    v_flex().gap(px(2.)).child(h_flex().gap(px(2.)).child(square()).child(square())).child(h_flex().gap(px(2.)).child(square()).child(square()))
}

impl Render for LeftIsland {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let env = *cx.global::<Env>();
        let fallback = self.glass.borrow().fallback();
        let dark = env.dark;
        let (hover, pressed) = (look::hover_fill(dark).hsla(), look::pressed_fill(dark).hsla());
        let secondary = look::text_secondary(dark).hsla();
        let accent: Hsla = rgb(env.accent).into();

        let logo = div()
            .id("dock-logo")
            .size(px(plan::LOGO))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(8.))
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(windows_logo(accent))
            // the user's click is the only thing that ever sends the Windows key
            .on_click(|_, _, _| {
                rf_win::dock::triggers::start_menu();
            });

        let search = h_flex()
            .id("dock-search")
            .ml(px(plan::LOGO_GAP))
            .w(px(plan::SEARCH_W))
            .h(px(plan::SEARCH_H))
            .px(px(12.))
            .gap(px(8.))
            .items_center()
            .rounded_full()
            .bg(look::field_fill(dark).hsla())
            .cursor_pointer()
            .hover(move |s| s.bg(pressed))
            .child(Icon::new(L::Search).size(px(14.)).flex_shrink_0().text_color(secondary))
            .child(div().text_sm().text_color(secondary).child(t!("Ara")))
            .on_click(|_, _, _| super::actions::open_quick_search());

        let weather = self.weather.map(|o| {
            h_flex()
                .ml(px(plan::WEATHER_GAP))
                .gap(px(plan::WEATHER_TEXT_GAP))
                .items_center()
                .child(Icon::new(picture_icon(plan::weather_picture(o.code, o.is_day))).size(px(plan::WEATHER_ICON)).flex_shrink_0().text_color(look::text(dark).hsla()))
                .child(div().text_sm().child(o.text()))
        });

        super::host::island_box(&env, fallback)
            .child(h_flex().size_full().px(px(plan::SIDE_PAD)).items_center().child(logo).child(search).children(weather))
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
