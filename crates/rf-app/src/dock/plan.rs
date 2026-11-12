//! The dock's arithmetic and decisions that need neither a window nor the system: how wide the side islands are, where the icons
//! lie while one grows, which icon the pointer is over, which weather picture a code gets, how soon the weather is asked again.
//! (The three rectangles, the hover animation, the click decision and the full-screen decision are in `rf_win::dock`.)

use std::time::Duration;

use rf_win::dock::clock::ClockLines;
use rf_win::dock::hover::{self, Slot};
use rf_win::dock::layout::{DOCK_PADDING, ICON_GAP, ICON_REST, Rect, SEPARATOR_WIDTH};
use rf_win::weather::Condition;

// ---- the left island: [Windows logo] [search field] [weather]

/// Space between the island's edge and its first / last item.
pub const SIDE_PAD: f32 = 8.0;
/// The Windows logo button (a square).
pub const LOGO: f32 = 32.0;
/// Gap between the logo and the search field.
pub const LOGO_GAP: f32 = 8.0;
/// The search field.
pub const SEARCH_W: f32 = 220.0;
pub const SEARCH_H: f32 = 32.0;
/// Gap between the search field and the weather.
pub const WEATHER_GAP: f32 = 12.0;
/// The weather picture and the gap before the temperature.
pub const WEATHER_ICON: f32 = 18.0;
pub const WEATHER_TEXT_GAP: f32 = 6.0;
/// What one character of the temperature takes (14 px text, tabular enough for "12°" and "-3°").
const WEATHER_CHAR: f32 = 8.5;

/// The weather block's width for this text ("12°").
pub fn weather_width(text: &str) -> f32 {
    WEATHER_ICON + WEATHER_TEXT_GAP + text.chars().count() as f32 * WEATHER_CHAR
}

/// The left island's width (logical px): the logo, the search field and, when there is weather, a gap and the weather.
pub fn left_width(weather_text: Option<&str>) -> f32 {
    SIDE_PAD + LOGO + LOGO_GAP + SEARCH_W + weather_text.map_or(0.0, |t| WEATHER_GAP + weather_width(t)) + SIDE_PAD
}

// ---- the right island: [network] [volume] | [time / date] [bell]

pub const BUTTON: f32 = 32.0;
pub const RIGHT_GAP: f32 = 4.0;
/// Space around the thin separator.
pub const RIGHT_SEP_GAP: f32 = 8.0;
pub const RIGHT_SEP_HEIGHT: f32 = 20.0;
/// What one character of the clock's lines takes (12 px text).
const CLOCK_CHAR: f32 = 7.0;
/// The clock block is never narrower than a ten character date ("12.11.2026"), so 9:59 -> 10:00 does not move the island.
const CLOCK_MIN_CHARS: usize = 10;
/// Padding inside the clock block, so its click target is not tight around the digits.
pub const CLOCK_PAD: f32 = 8.0;

pub fn clock_width(lines: &ClockLines) -> f32 {
    let chars = lines.time.chars().count().max(lines.date.chars().count()).max(CLOCK_MIN_CHARS);
    chars as f32 * CLOCK_CHAR + CLOCK_PAD
}

/// The right island's width: padding, two buttons, a separator between gaps, the clock, the bell, padding.
pub fn right_width(lines: &ClockLines) -> f32 {
    SIDE_PAD + BUTTON + RIGHT_GAP + BUTTON + RIGHT_SEP_GAP + SEPARATOR_WIDTH + RIGHT_SEP_GAP + clock_width(lines) + RIGHT_GAP + BUTTON + SIDE_PAD
}

// ---- the middle island

/// The middle island's width at rest for this many icons and separators (`rf_win` has the formula; icons at rest are 44 px).
pub fn middle_width_at_rest(icons: usize, separators: usize) -> f32 {
    rf_win::dock::layout::middle_width(&vec![ICON_REST; icons], separators)
}

/// The smallest gap two items are squeezed to while an icon grows.
const MIN_GROWN_GAP: f32 = 2.0;

/// Where each item lies (`(x, width)` in logical px from the island's left edge, padding included) while the icons have `icon_sizes`
/// (one per [`Slot::Icon`]) inside an island that is `width` wide. The island does not change its width when an icon grows: the
/// gaps between the items give way (down to 2 px), and only when even that is not enough all icons shrink together to fit. At rest
/// the sizes are 44 px and the gaps 10 px, which is exactly `rf_win::dock::hover::slot_spans`.
pub fn dock_layout(slots: &[Slot], icon_sizes: &[f32], width: f32) -> Vec<(f32, f32)> {
    if slots.is_empty() {
        return Vec::new();
    }
    let inner = (width - 2.0 * DOCK_PADDING).max(0.0);
    let separators = slots.iter().filter(|s| **s == Slot::Separator).count();
    let icon_total: f32 = icon_sizes.iter().sum();
    let gaps = slots.len() - 1;
    let fixed = icon_total + separators as f32 * SEPARATOR_WIDTH;
    let (gap, k) = if gaps == 0 {
        (0.0, if icon_total > inner && icon_total > 0.0 { inner / icon_total } else { 1.0 })
    } else {
        let gap = ((inner - fixed) / gaps as f32).min(ICON_GAP);
        if gap >= MIN_GROWN_GAP {
            (gap, 1.0)
        } else {
            // not even 2 px gaps fit: all icons give a little
            let room = inner - separators as f32 * SEPARATOR_WIDTH - gaps as f32 * MIN_GROWN_GAP;
            (MIN_GROWN_GAP, if icon_total > 0.0 { (room / icon_total).clamp(0.0, 1.0) } else { 1.0 })
        }
    };
    let mut x = DOCK_PADDING;
    let mut icon = 0usize;
    let mut out = Vec::with_capacity(slots.len());
    for (i, s) in slots.iter().enumerate() {
        if i > 0 {
            x += gap;
        }
        let w = match s {
            Slot::Icon => {
                let w = icon_sizes.get(icon).copied().unwrap_or(ICON_REST) * k;
                icon += 1;
                w
            }
            Slot::Separator => SEPARATOR_WIDTH,
        };
        out.push((x, w));
        x += w;
    }
    out
}

/// The icon the pointer is over, `x` logical px from the island's left edge. Judged against the icons AT REST, never against the
/// growing ones: a grown icon moving under a still pointer would otherwise change the answer again and again (the dock would
/// flicker). The gaps and the padding belong to the nearest icon, a separator to none.
///
/// `width` is the island's real width: when the screen is too narrow for it the icons lie closer (see [`dock_layout`]), and the
/// pointer is judged against where they really are.
pub fn hover_target(x: f32, slots: &[Slot], width: f32) -> Option<usize> {
    let icons = slots.iter().filter(|s| **s == Slot::Icon).count();
    hover::icon_at_spans(x, slots, &dock_layout(slots, &vec![ICON_REST; icons], width))
}

/// A window rectangle in device px as GPUI wants it: logical px (`x`, `y`, `w`, `h`) at this scale.
pub fn logical(r: Rect, scale: f32) -> (f32, f32, f32, f32) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    (r.left as f32 / s, r.top as f32 / s, r.width() as f32 / s, r.height() as f32 / s)
}

/// How long the animation setting allows the hover grow to take.
pub fn hover_duration(animations: bool) -> u32 {
    if animations { hover::DURATION_MS } else { 0 }
}

// ---- weather

/// The picture a weather code gets (the shell's font has no weather set; the dock draws these with its own icons).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WeatherPicture {
    Sun,
    Moon,
    CloudSun,
    CloudMoon,
    Cloud,
    Fog,
    Drizzle,
    Rain,
    Snow,
    Thunder,
    Hail,
}

pub fn weather_picture(code: u8, is_day: bool) -> WeatherPicture {
    match rf_win::weather::condition(code) {
        Condition::Clear | Condition::MainlyClear => {
            if is_day {
                WeatherPicture::Sun
            } else {
                WeatherPicture::Moon
            }
        }
        Condition::PartlyCloudy => {
            if is_day {
                WeatherPicture::CloudSun
            } else {
                WeatherPicture::CloudMoon
            }
        }
        Condition::Overcast | Condition::Unknown => WeatherPicture::Cloud,
        Condition::Fog => WeatherPicture::Fog,
        Condition::Drizzle => WeatherPicture::Drizzle,
        Condition::Rain | Condition::FreezingRain | Condition::RainShowers => WeatherPicture::Rain,
        Condition::Snow | Condition::SnowGrains | Condition::SnowShowers => WeatherPicture::Snow,
        Condition::Thunderstorm => WeatherPicture::Thunder,
        Condition::ThunderstormHail => WeatherPicture::Hail,
    }
}

/// The first wait after a failure (a network that was down at start-up must not leave the island empty for half an hour), and the
/// longest one after repeated failures.
const RETRY_FIRST: Duration = Duration::from_secs(5 * 60);
const RETRY_LONGEST: Duration = Duration::from_secs(30 * 60);
/// A city the geocoder does not know stays unknown: it is asked about again only now and then, in case the service learned it.
const UNKNOWN_PLACE_RETRY: Duration = Duration::from_secs(60 * 60);

/// When the weather is asked again. `failures` counts the failures in a row up to and including this one (0 after a success).
/// Success: every 30 minutes. Failure: 5 minutes, doubling with each failure in a row up to 30 minutes. An unknown place: an hour.
pub fn next_weather_refresh(outcome: &rf_win::weather::Outcome, failures: u32) -> Duration {
    use rf_win::weather::Outcome;
    match outcome {
        Outcome::Weather(_) => rf_win::weather::REFRESH_EVERY,
        Outcome::UnknownPlace => UNKNOWN_PLACE_RETRY,
        Outcome::Failed => RETRY_FIRST.saturating_mul(1u32 << failures.saturating_sub(1).min(4)).min(RETRY_LONGEST),
    }
}

// ---- icons

/// The key an icon is cached under (`IconSource` is not hashable).
pub fn icon_key(src: &rf_win::dock::IconSource) -> String {
    format!("{src:?}")
}

/// The size to ask the shell for, in device px: the biggest an icon ever gets (56 logical px), so growing never enlarges a bitmap.
pub fn icon_request_px(scale: f32) -> u32 {
    ((hover::GROWN * scale.max(1.0)).ceil() as u32).clamp(16, 512)
}

/// The letter a placeholder shows until (or instead of) the app's icon.
pub fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::dock::hover::slot_spans;

    fn slots(n_icons: usize, sep_at: Option<usize>) -> Vec<Slot> {
        let mut v: Vec<Slot> = (0..n_icons).map(|_| Slot::Icon).collect();
        if let Some(i) = sep_at {
            v.insert(i, Slot::Separator);
        }
        v
    }

    #[test]
    fn the_left_island_grows_by_the_weather() {
        assert_eq!(left_width(None), 8.0 + 32.0 + 8.0 + 220.0 + 8.0);
        let with = left_width(Some("12°"));
        assert!(with > left_width(None) + WEATHER_GAP);
        // a longer text is wider
        assert!(left_width(Some("-12°")) > with);
        assert_eq!(weather_width("12°"), 18.0 + 6.0 + 3.0 * 8.5);
    }

    #[test]
    fn the_right_island_does_not_move_when_the_hour_gets_a_digit() {
        let nine = ClockLines { time: "9:59".into(), date: "12.11.2026".into() };
        let ten = ClockLines { time: "10:00".into(), date: "12.11.2026".into() };
        assert_eq!(right_width(&nine), right_width(&ten));
        // a long locale pattern widens it
        let long = ClockLines { time: "10:00:00 PM".into(), date: "12.11.2026".into() };
        assert!(right_width(&long) > right_width(&ten));
    }

    #[test]
    fn at_rest_the_layout_is_the_one_rf_win_hit_tests() {
        let s = slots(5, Some(3));
        let sizes = vec![ICON_REST; 5];
        let width = middle_width_at_rest(5, 1);
        let mine = dock_layout(&s, &sizes, width);
        let theirs = slot_spans(&s, &sizes);
        assert_eq!(mine.len(), theirs.len());
        for (a, b) in mine.iter().zip(&theirs) {
            assert!((a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_growing_icon_takes_its_room_from_the_gaps_and_the_island_keeps_its_width() {
        let s = slots(8, Some(6));
        let width = middle_width_at_rest(8, 1);
        for hovered in 0..8 {
            let sizes = hover::target_sizes(8, Some(hovered));
            let spans = dock_layout(&s, &sizes, width);
            let (x, w) = *spans.last().unwrap();
            assert!(x + w <= width - DOCK_PADDING + 0.01, "icon {hovered}: the last item ends at {} of {}", x + w, width - DOCK_PADDING);
            assert_eq!(spans[0].0, DOCK_PADDING);
            // no two items overlap, and no gap is below the least
            for pair in spans.windows(2) {
                assert!(pair[1].0 - (pair[0].0 + pair[0].1) >= MIN_GROWN_GAP - 0.01);
            }
        }
    }

    #[test]
    fn the_hovered_icon_really_is_bigger_in_the_layout() {
        let s = slots(6, Some(4));
        let width = middle_width_at_rest(6, 1);
        let sizes = hover::target_sizes(6, Some(2));
        let spans = dock_layout(&s, &sizes, width);
        assert_eq!(spans[2].1, hover::GROWN);
        assert_eq!(spans[1].1, hover::NEIGHBOUR);
        assert_eq!(spans[3].1, hover::NEIGHBOUR);
        assert_eq!(spans[0].1, ICON_REST);
    }

    #[test]
    fn a_nearly_empty_dock_shrinks_its_icons_rather_than_overflow() {
        // the places only: two icons and the separator
        let s = slots(2, Some(0));
        let width = middle_width_at_rest(2, 1);
        let sizes = hover::target_sizes(2, Some(0)); // 56 + 50 > what 44 + 44 and two 10 px gaps leave
        let spans = dock_layout(&s, &sizes, width);
        let (x, w) = *spans.last().unwrap();
        assert!(x + w <= width - DOCK_PADDING + 0.01);
        // one icon alone: it shrinks to the room it has
        let one = dock_layout(&[Slot::Icon], &[56.0], middle_width_at_rest(1, 0));
        assert!(one[0].1 <= ICON_REST + 0.01, "{one:?}");
    }

    #[test]
    fn nothing_to_lay_out() {
        assert!(dock_layout(&[], &[], 100.0).is_empty());
        assert_eq!(hover_target(10.0, &[], 100.0), None);
    }

    #[test]
    fn the_pointer_is_judged_against_the_icons_at_rest() {
        let s = slots(4, None);
        let w = middle_width_at_rest(4, 0);
        // padding 12, icons 44 + 10 gap: icon 0 is 12..56, icon 1 66..110
        assert_eq!(hover_target(0.0, &s, w), Some(0), "the padding belongs to the first icon");
        assert_eq!(hover_target(30.0, &s, w), Some(0));
        assert_eq!(hover_target(80.0, &s, w), Some(1));
        assert_eq!(hover_target(1000.0, &s, w), Some(3), "past the end belongs to the last icon");
        // the separator is nobody's
        let with_sep = slots(2, Some(1));
        let sep = DOCK_PADDING + ICON_REST + ICON_GAP; // the separator's left edge
        assert_eq!(hover_target(sep + 0.2, &with_sep, middle_width_at_rest(2, 1)), None);
    }

    #[test]
    fn in_a_squeezed_island_the_icons_and_the_pointer_agree() {
        // 8 icons need 12 + 8*44 + 7*10 + 12 = 446 px; the screen leaves only 300
        let s = slots(8, None);
        let squeezed = 300.0;
        let spans = dock_layout(&s, &vec![ICON_REST; 8], squeezed);
        let (last_x, last_w) = *spans.last().unwrap();
        assert!(last_x + last_w <= squeezed - DOCK_PADDING + 0.01, "every icon is inside the window: {spans:?}");
        // the pointer over the middle of each drawn icon finds that icon
        for (i, &(x, w)) in spans.iter().enumerate() {
            assert_eq!(hover_target(x + w / 2.0, &s, squeezed), Some(i), "icon {i}");
        }
        // with the full width the same x finds an earlier icon: the answer depends on the real width
        let (x7, w7) = spans[7];
        assert_ne!(hover_target(x7 + w7 / 2.0, &s, middle_width_at_rest(8, 0)), Some(7));
    }

    #[test]
    fn device_pixels_become_logical_ones() {
        let r = Rect::from_xywh(300, 900, 150, 108);
        assert_eq!(logical(r, 1.5), (200.0, 600.0, 100.0, 72.0));
        // a broken scale never divides by zero
        assert_eq!(logical(r, 0.0), (300.0, 900.0, 150.0, 108.0));
    }

    #[test]
    fn the_animation_setting_turns_the_grow_into_a_jump() {
        assert_eq!(hover_duration(true), 120);
        assert_eq!(hover_duration(false), 0);
    }

    #[test]
    fn weather_codes_get_pictures() {
        assert_eq!(weather_picture(0, true), WeatherPicture::Sun);
        assert_eq!(weather_picture(0, false), WeatherPicture::Moon);
        assert_eq!(weather_picture(2, true), WeatherPicture::CloudSun);
        assert_eq!(weather_picture(2, false), WeatherPicture::CloudMoon);
        assert_eq!(weather_picture(3, true), WeatherPicture::Cloud);
        assert_eq!(weather_picture(45, true), WeatherPicture::Fog);
        assert_eq!(weather_picture(53, true), WeatherPicture::Drizzle);
        assert_eq!(weather_picture(63, true), WeatherPicture::Rain);
        assert_eq!(weather_picture(82, false), WeatherPicture::Rain);
        assert_eq!(weather_picture(73, true), WeatherPicture::Snow);
        assert_eq!(weather_picture(95, true), WeatherPicture::Thunder);
        assert_eq!(weather_picture(99, true), WeatherPicture::Hail);
        // a code nobody documented still gets a picture
        assert_eq!(weather_picture(200, true), WeatherPicture::Cloud);
    }

    #[test]
    fn the_weather_is_asked_again_by_how_the_last_ask_ended() {
        use rf_win::weather::{Observation, Outcome};
        let ok = Outcome::Weather(Observation { temp_c: 1.0, code: 0, is_day: true });
        assert_eq!(next_weather_refresh(&ok, 0), Duration::from_secs(30 * 60));
        // a failure: sooner, then slower and slower, never beyond the normal rate
        let waits: Vec<u64> = (1..=8).map(|n| next_weather_refresh(&Outcome::Failed, n).as_secs()).collect();
        assert_eq!(waits, vec![300, 600, 1200, 1800, 1800, 1800, 1800, 1800]);
        assert!(waits[0] < 30 * 60);
        assert_eq!(next_weather_refresh(&Outcome::Failed, 0), Duration::from_secs(300), "no count yet: the first wait");
        assert_eq!(next_weather_refresh(&Outcome::Failed, u32::MAX), Duration::from_secs(1800), "no overflow");
        // a city nobody knows is not asked about every few minutes
        assert_eq!(next_weather_refresh(&Outcome::UnknownPlace, 0), Duration::from_secs(3600));
    }

    #[test]
    fn icons_are_asked_for_at_the_biggest_size_they_get() {
        assert_eq!(icon_request_px(1.0), 56);
        assert_eq!(icon_request_px(1.5), 84);
        assert_eq!(icon_request_px(0.0), 56);
        assert_eq!(icon_request_px(100.0), 512);
    }

    #[test]
    fn icon_keys_tell_sources_apart() {
        use rf_win::dock::IconSource;
        let a = icon_key(&IconSource::File("C:\\a.exe".into()));
        let b = icon_key(&IconSource::File("C:\\b.exe".into()));
        let c = icon_key(&IconSource::Packaged("a!b".into()));
        assert_ne!(a, b);
        assert_ne!(a, c);
        assert_eq!(a, icon_key(&IconSource::File("C:\\a.exe".into())));
    }

    #[test]
    fn placeholders_show_a_letter() {
        assert_eq!(initial("chrome"), "C");
        assert_eq!(initial("  1Password"), "1");
        assert_eq!(initial("İstanbul"), "İ");
        assert_eq!(initial(""), "?");
        assert_eq!(initial("--"), "?");
    }
}
