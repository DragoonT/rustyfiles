//! The dock's arithmetic and decisions that need neither a window nor the system: how wide the side islands are, where the items of
//! the middle island lie and what its window shows while the icons are magnified, which weather picture a code gets, how soon the
//! weather is asked again. (The three rectangles and their windows, the magnification, the pointer region, the click decision and the
//! full-screen decision are in `rf_win::dock`.)

use std::time::Duration;

use rf_win::dock::clock::ClockLines;
use rf_win::dock::hover::Slot;
use rf_win::dock::layout::{ICON_REST, MIDDLE_HEIGHT, Rect, SEPARATOR_WIDTH};
use rf_win::dock::magnify::{self, Frame};
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
/// What one character of the condition line under the temperature takes (11 px text). Words are wider than digits, and this is an
/// estimate that errs on the wide side: the text is drawn on one line and cut with an ellipsis rather than ever run into the field.
const WEATHER_SUB_CHAR: f32 = 6.4;
/// The least room the middle island is given (dock units) before the condition line may take any of the display, however few icons it
/// holds at rest: below it the line goes first. With more icons the middle island's own width counts instead.
pub const MIN_MIDDLE_ROOM: f32 = 320.0;
/// How long the last good reading is still shown when the weather from the PC's position cannot be refreshed (two hours: four of the
/// normal 30 minute refreshes). After that the stale number would mislead, and nothing is shown.
pub const STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);

/// The weather block's width (the picture, a gap and the text) for the temperature ("12°") and, under it, the condition in words: the picture, a gap and the LONGER of the two
/// lines. Dock units, like every length here: the host multiplies them by the monitor's scale and the dock's size, so the block is the
/// right width at every size.
pub fn weather_width_lines(temp: &str, condition: Option<&str>) -> f32 {
    WEATHER_ICON + WEATHER_TEXT_GAP + weather_text_width(temp, condition)
}

/// The width of the text column of the weather block (the longer of the two lines). The block gives the column exactly this width, so
/// a condition line wider than the estimate is cut with an ellipsis inside it and never pushes into the search field.
pub fn weather_text_width(temp: &str, condition: Option<&str>) -> f32 {
    let top = temp.chars().count() as f32 * WEATHER_CHAR;
    let sub = condition.map_or(0.0, |c| c.chars().count() as f32 * WEATHER_SUB_CHAR);
    top.max(sub)
}

/// The left island's width (logical px): the logo, the search field and, when there is weather, a gap and the weather.
pub fn left_width(weather_text: Option<&str>) -> f32 {
    left_width_lines(weather_text, None)
}

/// [`left_width`] with the condition line under the temperature.
pub fn left_width_lines(temp: Option<&str>, condition: Option<&str>) -> f32 {
    SIDE_PAD + LOGO + LOGO_GAP + SEARCH_W + temp.map_or(0.0, |t| WEATHER_GAP + weather_width_lines(t, condition)) + SIDE_PAD
}

/// Whether the condition line may be shown on this display: with it, the middle island must still fit, whole, between the left island
/// (that wide) and the right one. "Whole" is the middle island's own width at rest (more icons, more room), and never less than
/// [`MIN_MIDDLE_ROOM`]. All numbers are dock units: the display's width (work area over the monitor's scale times the dock's size), the
/// right island's width, the middle island's width and the left island's width WITH the line. When the answer is no (a narrow display,
/// a big dock size, many icons) the weather is the picture and the temperature, as before, and the icons keep their size.
pub fn condition_line_fits(display_units: f32, right_units: f32, middle_units: f32, left_units_with_line: f32) -> bool {
    use rf_win::dock::layout::{MARGIN_SIDE, MIN_GAP};
    let others = 2.0 * MARGIN_SIDE + 2.0 * MIN_GAP + right_units + left_units_with_line;
    display_units - others >= middle_units.max(MIN_MIDDLE_ROOM)
}

/// Whether a refresh that failed leaves the last good reading on the screen. Only for the weather from the PC's position: there a miss
/// (Windows has no position yet after a wake or a change of network) is common and passes, so the picture does not flicker away; a
/// typed city keeps the old behaviour (a failed request shows nothing). `since_good` is how long ago the shown reading was fetched
/// (`None`: nothing is shown). A refusal or an unknown place is not a miss and always clears it.
pub fn keeps_last_reading(location: bool, outcome: &rf_win::weather::Outcome, since_good: Option<Duration>) -> bool {
    location && matches!(outcome, rf_win::weather::Outcome::Failed) && since_good.is_some_and(|d| d < STALE_AFTER)
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
/// The clock block's height (its click target).
pub const CLOCK_H: f32 = 36.0;

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

/// Where each slot lies at rest (`(x, width)` in logical px from the island's left edge, padding included) in an island that is
/// `width` wide: 44 px icons with 10 px gaps, or, on a screen too narrow for that, the gaps and then the icons give way. This is
/// `rf_win::dock::hover::slot_spans` whenever the island has its full width.
pub fn rest_layout(slots: &[Slot], width: f32) -> Vec<(f32, f32)> {
    let icons = slots.iter().filter(|s| **s == Slot::Icon).count();
    magnify::fit(slots, &vec![ICON_REST; icons], width)
}

/// One picture of the middle island, in LOGICAL px in the window's own coordinates (the window is bigger than the glass: `margins`
/// are the distances from its edges to the glass at rest, left, top, right, bottom).
#[derive(Clone, Debug, PartialEq)]
pub struct DockPicture {
    /// The glass: x, y, w, h. It widens symmetrically around the rest island's centre while icons grow and its height never changes.
    pub glass: [f32; 4],
    /// Every slot: x, width (the icons stand on one baseline, the viewer places them from the bottom).
    pub items: Vec<(f32, f32)>,
    /// The boxes (x, y, w, h) of the icons that stand out over the glass's top edge; they are the island's too, for the pointer.
    pub out_of_glass: Vec<[f32; 4]>,
}

/// The picture of a frame of the magnification. `baseline` is how far the icons' bottom edges are above the glass's bottom edge.
pub fn picture(frame: &Frame, slots: &[Slot], margins: [f32; 4], baseline: f32) -> DockPicture {
    let [ml, mt, ..] = margins;
    let glass = [ml + frame.left, mt, frame.width, MIDDLE_HEIGHT];
    let items: Vec<(f32, f32)> = frame.spans.iter().map(|&(x, w)| (ml + x, w)).collect();
    let bottom = mt + MIDDLE_HEIGHT - baseline;
    let out_of_glass = slots
        .iter()
        .zip(&items)
        .filter(|(s, _)| **s == Slot::Icon)
        .map(|(_, &(x, w))| [x, bottom - w, w, w])
        .filter(|b| b[1] < mt)
        .collect();
    DockPicture { glass, items, out_of_glass }
}

/// Logical px to device px for a rectangle (x, y, w, h).
pub fn to_device(r: [f32; 4], scale: f32) -> [f32; 4] {
    r.map(|v| v * scale)
}

/// A window rectangle in device px as GPUI wants it: logical px (`x`, `y`, `w`, `h`) at this scale.
pub fn logical(r: Rect, scale: f32) -> (f32, f32, f32, f32) {
    let s = if scale > 0.0 { scale } else { 1.0 };
    (r.left as f32 / s, r.top as f32 / s, r.width() as f32 / s, r.height() as f32 / s)
}

/// How long the animation setting allows the magnification to come in and go out.
pub fn hover_duration(animations: bool) -> u32 {
    if animations { magnify::FADE_MS } else { 0 }
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

/// The sizes the pictures of an icon are kept at, smallest first, for a dock unit `scale` (the monitor's scale times the dock's size, so
/// a small dock on a 100 % monitor needs fewer pixels than a big one) and the magnification's `peak` factor (1 when it is off): the
/// rest size, the peak size (44 dock units grown by the peak, in device px: the most an icon is ever drawn at, so every picture is
/// only ever scaled DOWN) and the steps between (`rf_win::dock::iconpick::level_sizes`).
pub fn icon_sizes(scale: f32, peak: f32) -> Vec<u32> {
    rf_win::dock::iconpick::level_sizes(rf_win::dock::iconpick::rest_px(scale), rf_win::dock::iconpick::peak_px(scale, peak))
}

/// Logical px of the small picture the hover preview's title row (16) and the app menu's name row (18) draw an app's icon at: the
/// dock keeps one picture of about this size beside its big ones, so those two do not reduce the dock's 44+ px picture by a factor of
/// three or four with one bilinear tap of the toolkit.
pub const SMALL_ICON: f32 = 20.0;

/// [`icon_sizes`] with the small picture in front (device px at the monitor's scale `monitor_scale`, not the dock's size: the menus do
/// not follow the dock's size). The smallest kept size is the rest size, 37 dock units or more, so the small one is always below it.
pub fn icon_sizes_with_small(unit: f32, peak: f32, monitor_scale: f32) -> Vec<u32> {
    let mut sizes = icon_sizes(unit, peak);
    let small = ((SMALL_ICON * if monitor_scale.is_finite() && monitor_scale > 0.0 { monitor_scale } else { 1.0 }).round() as u32).max(rf_win::dock::iconpick::MIN_PX);
    if sizes.first().is_some_and(|&rest| small < rest) {
        sizes.insert(0, small);
    }
    sizes
}

/// The letter a placeholder shows until (or instead of) the app's icon.
pub fn initial(name: &str) -> String {
    name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::dock::hover::slot_spans;

    /// The most device px an icon is drawn at with the strongest magnification, at this dock unit.
    fn icon_request_px(scale: f32) -> u32 {
        rf_win::dock::iconpick::peak_px(scale, magnify::MAX_PEAK)
    }

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
        assert_eq!(weather_width_lines("12°", None), 18.0 + 6.0 + 3.0 * 8.5);
    }

    #[test]
    fn the_weather_block_is_as_wide_as_the_longer_of_its_two_lines() {
        // no condition line: exactly what it always was
                assert_eq!(left_width_lines(Some("12°"), None), left_width(Some("12°")));
        // a short word under a temperature: the temperature is the longer line
        assert_eq!(weather_width_lines("-12°", Some("Açık")), 18.0 + 6.0 + 4.0 * 8.5);
        // a long word: the word is
        let w = weather_width_lines("6°", Some("Parçalı bulutlu"));
        assert_eq!(w, 18.0 + 6.0 + 15.0 * 6.4);
        assert!(w > weather_width_lines("6°", None));
        // the island is the logo, the field and the block, with nothing shared between them
        let island = left_width_lines(Some("6°"), Some("Parçalı bulutlu"));
        assert_eq!(island, SIDE_PAD + LOGO + LOGO_GAP + SEARCH_W + WEATHER_GAP + w + SIDE_PAD);
        assert!(island > left_width_lines(Some("6°"), Some("Açık")));
        // without a temperature there is nothing at all, whatever the line
        assert_eq!(left_width_lines(None, Some("Açık")), left_width(None));
    }

    /// At every dock size the layout turns dock units into device px by the same unit, so the block's room for each line and the gap
    /// before it keep their proportions: the weather never reaches back into the field.
    #[test]
    fn at_every_dock_size_the_block_clears_the_search_field() {
        use rf_win::dock::layout::{self, Rect, Widths};
        use rf_win::dock::size::{self, DockSize};
        let units = left_width_lines(Some("-12°"), Some("Dondurucu yağmur"));
        let field_end = SIDE_PAD + LOGO + LOGO_GAP + SEARCH_W;
        let block = weather_width_lines("-12°", Some("Dondurucu yağmur"));
        assert_eq!(units, field_end + WEATHER_GAP + block + SIDE_PAD);
        for s in DockSize::ALL {
            for scale in [1.0, 1.25, 1.5, 2.0] {
                let unit = size::unit(scale, s.factor());
                let work = Rect::new(0, 0, 2560, 1400);
                let i = layout::islands(work, unit, Widths { left: units, middle: 400.0, right: 215.0 });
                // the island is the width the plan asked for (the side islands are never squeezed) ...
                assert_eq!(i.left.width(), layout::px(units, unit), "{s:?} at {scale}");
                // ... and the block starts after the field with the gap, and ends before the island's padding
                let start = layout::px(field_end + WEATHER_GAP, unit);
                let end = layout::px(units - SIDE_PAD, unit);
                assert!(end - start >= layout::px(block, unit) - 2, "{s:?} at {scale}: {start}..{end} for {}", layout::px(block, unit));
                assert!(start > layout::px(field_end, unit));
            }
        }
    }

    #[test]
    fn on_a_narrow_display_the_condition_line_is_the_first_thing_to_go() {
        let with = left_width_lines(Some("6°"), Some("Parçalı bulutlu"));
        let right = 215.0;
        let rest = middle_width_at_rest(2, 1);
        // 1920 px at 100 %, the four dock sizes: the display in dock units is 1920 / size
        for (display, fits) in [(1920.0, true), (1920.0 / 1.2, true), (1366.0, true), (1366.0 / 1.2, true), (1366.0 / 1.4, false), (1024.0, true), (900.0, false), (800.0, false)] {
            assert_eq!(condition_line_fits(display, right, rest, with), fits, "{display}");
        }
        // it goes before the weather itself would: without the line there is more room by exactly the line's width
        let without = left_width_lines(Some("6°"), None);
        let tight = 2.0 * 12.0 + 2.0 * 8.0 + right + without + MIN_MIDDLE_ROOM;
        assert!(condition_line_fits(tight + (with - without) + 0.01, right, rest, with));
        assert!(!condition_line_fits(tight + (with - without) - 1.0, right, rest, with));
        // ... and the temperature alone still fits where the line does not
        assert!(condition_line_fits(tight + (with - without) - 1.0, right, rest, without));
        // the right island's width counts
        assert!(!condition_line_fits(1366.0, 600.0, rest, with));
    }

    /// 1366 px at the default size. With the old fixed floor of 320 the line was judged to fit whatever the dock held, and a dock of
    /// fourteen icons (791 units wanted, against 703 of room) was squeezed by the text; with the middle island's real width the line
    /// goes first and the icons keep their size. Ten icons (565) still leave the line its room.
    #[test]
    fn many_icons_take_the_room_before_the_condition_line_does() {
        let with = left_width_lines(Some("6°"), Some("Parçalı bulutlu"));
        let right = 215.0;
        let (ten, fourteen) = (middle_width_at_rest(10, 1), middle_width_at_rest(14, 1));
        assert!(ten > MIN_MIDDLE_ROOM, "the case needs a middle island wider than the floor: {ten}");
        assert!(condition_line_fits(1366.0, right, middle_width_at_rest(2, 1), with), "few icons: room for the line");
        assert!(condition_line_fits(1366.0, right, ten, with), "ten icons: the middle island still fits whole beside the line");
        assert!(!condition_line_fits(1366.0, right, fourteen, with), "fourteen icons: no room, the line goes");
        assert!(condition_line_fits(1366.0, right, MIN_MIDDLE_ROOM, with), "the old fixed floor would have kept the line");
        // exactly at the edge: the middle island fits whole
        let edge = 2.0 * 12.0 + 2.0 * 8.0 + right + with + fourteen;
        assert!(condition_line_fits(edge, right, fourteen, with));
        assert!(!condition_line_fits(edge - 0.5, right, fourteen, with));
        // a middle island narrower than the floor still gets the floor
        assert_eq!(condition_line_fits(edge, right, 10.0, with), condition_line_fits(edge, right, MIN_MIDDLE_ROOM, with));
    }

    #[test]
    fn the_text_column_is_exactly_what_the_block_is_wider_than_the_picture() {
        for (t, c) in [("6°", None), ("-12°", Some("Açık")), ("6°", Some("Parçalı bulutlu")), ("21°", Some("Gök gürültülü"))] {
            assert_eq!(WEATHER_ICON + WEATHER_TEXT_GAP + weather_text_width(t, c), weather_width_lines(t, c), "{t} {c:?}");
        }
    }

    #[test]
    fn a_missed_refresh_keeps_the_reading_only_for_the_position_and_only_for_a_while() {
        use rf_win::weather::Outcome;
        let fresh = Some(Duration::from_secs(40 * 60));
        let stale = Some(STALE_AFTER);
        // the position: a miss keeps what is shown ...
        assert!(keeps_last_reading(true, &Outcome::Failed, fresh));
        // ... but not when nothing is shown, and not for ever
        assert!(!keeps_last_reading(true, &Outcome::Failed, None));
        assert!(!keeps_last_reading(true, &Outcome::Failed, stale));
        assert!(keeps_last_reading(true, &Outcome::Failed, Some(STALE_AFTER - Duration::from_secs(1))));
        // a refusal or an unknown place is not a miss
        assert!(!keeps_last_reading(true, &Outcome::UnknownPlace, fresh));
        // a typed city behaves as it always did: a failed request shows nothing
        assert!(!keeps_last_reading(false, &Outcome::Failed, fresh));
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
        let width = middle_width_at_rest(5, 1);
        let mine = rest_layout(&s, width);
        let theirs = slot_spans(&s, &[ICON_REST; 5]);
        assert_eq!(mine.len(), theirs.len());
        for (a, b) in mine.iter().zip(&theirs) {
            assert!((a.0 - b.0).abs() < 0.001 && (a.1 - b.1).abs() < 0.001, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_nearly_empty_dock_shrinks_its_icons_rather_than_overflow() {
        // one icon alone in an island too narrow for it: it shrinks to the room it has
        let one = rest_layout(&[Slot::Icon], 40.0);
        assert!(one[0].1 <= 16.0 + 0.01, "{one:?}");
        // the places only: two icons and the separator, in their own width
        let s = slots(2, Some(0));
        let w = middle_width_at_rest(2, 1);
        let spans = rest_layout(&s, w);
        let (x, ww) = *spans.last().unwrap();
        assert!(x + ww <= w - rf_win::dock::layout::DOCK_PADDING + 0.01);
    }

    #[test]
    fn nothing_to_lay_out() {
        assert!(rest_layout(&[], 100.0).is_empty());
    }

    /// The island is narrower than its items want (the side islands and the screen leave it less): they still lie inside it.
    #[test]
    fn in_a_squeezed_island_the_icons_lie_inside_it() {
        // 8 icons need 12 + 8*44 + 7*10 + 12 = 446 px; the screen leaves only 300
        let s = slots(8, None);
        let spans = rest_layout(&s, 300.0);
        let (last_x, last_w) = *spans.last().unwrap();
        assert!(last_x + last_w <= 300.0 - rf_win::dock::layout::DOCK_PADDING + 0.01, "every icon is inside the window: {spans:?}");
    }

    /// One picture of the dock: the glass is the rest island plus what the magnification adds on both sides, its height is the
    /// island's; the items are in the window's coordinates; the icons that stand out of the glass are listed for the pointer.
    #[test]
    fn a_picture_of_the_dock_is_in_the_windows_coordinates() {
        let s = slots(7, None);
        let w = middle_width_at_rest(7, 0);
        let rest = rest_layout(&s, w);
        let margins = [68.0, 28.0, 68.0, 28.0];
        let baseline = 12.0;
        // at rest: the glass is the island, nothing stands out
        let at_rest = picture(&Frame::at_rest(&rest, w), &s, margins, baseline);
        assert_eq!(at_rest.glass, [68.0, 28.0, w, 72.0]);
        assert!(at_rest.out_of_glass.is_empty());
        assert_eq!(at_rest.items[0], (68.0 + rest[0].0, 44.0));
        // the pointer on the middle icon: wider glass, centred on the island's centre, an icon out of the top
        let centre = rest[3].0 + rest[3].1 / 2.0;
        let f = magnify::frame(&s, &rest, w, centre, 1.0, magnify::Strength::Full, w + 80.0);
        let pic = picture(&f, &s, margins, baseline);
        assert!(pic.glass[2] > w && pic.glass[3] == 72.0);
        assert!((pic.glass[0] + pic.glass[2] / 2.0 - (68.0 + w / 2.0)).abs() < 0.01, "symmetric around the rest island's centre");
        assert!(!pic.out_of_glass.is_empty());
        for b in &pic.out_of_glass {
            assert!(b[1] < 28.0, "only icons over the glass's top edge");
            assert_eq!(b[1] + b[3], 28.0 + 72.0 - baseline, "standing on the baseline");
            assert!(b[1] >= 0.0, "inside the window's headroom: {b:?}");
        }
        // the biggest icon: 8.2 px over the glass
        let big = pic.out_of_glass.iter().map(|b| b[3]).fold(0.0, f32::max);
        assert!((big - 44.0 * 1.55).abs() < 0.01);
        // the device rectangle at 150 %
        assert_eq!(to_device([10.0, 20.0, 30.0, 40.0], 1.5), [15.0, 30.0, 45.0, 60.0]);
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
        assert_eq!(hover_duration(true), 160);
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
        // 44 px grown 1.55 times: 68.2, rounded up
        assert_eq!(icon_request_px(1.0), 69);
        assert_eq!(icon_request_px(1.5), 103);
        assert_eq!(icon_request_px(2.0), 137);
        assert_eq!(icon_request_px(0.0), 69);
        assert_eq!(icon_request_px(100.0), 512);
        // never smaller than an icon at the biggest size is wide at this scale
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 3.0] {
            assert!(icon_request_px(scale) as f32 >= ICON_REST * magnify::MAX_PEAK * scale - 0.001);
        }
    }

    #[test]
    fn icons_are_asked_for_at_the_size_the_dock_unit_draws_them() {
        // the unit is the monitor's scale times the dock's size: a small dock (0.85) on a 100 % monitor needs fewer pixels, a big one more
        assert_eq!(icon_request_px(0.85), 58);
        assert_eq!(icon_request_px(1.4), 96);
        assert_eq!(icon_request_px(1.25 * 1.2), 103);
        // the kept sizes: rest, peak, a step between; one when the magnification is off; two for the subtle one
        assert_eq!(icon_sizes(1.0, magnify::MAX_PEAK), vec![44, 55, 69]);
        assert_eq!(icon_sizes(1.0, 1.0), vec![44]);
        assert_eq!(icon_sizes(1.0, magnify::Strength::Subtle.peak()), vec![44, 55]);
        assert_eq!(icon_sizes(1.5, magnify::MAX_PEAK), vec![66, 82, 103]);
        assert_eq!(*icon_sizes(2.0, magnify::MAX_PEAK).last().unwrap(), icon_request_px(2.0));
    }

    #[test]
    fn the_picture_drawn_is_one_to_one_at_rest_and_at_the_peak_and_never_reduced_much_between() {
        use rf_win::dock::iconpick::{MAX_STEP, pick_level};
        // The layout (rest widths, the magnification's frames) decides the width an icon is drawn at; the kept sizes are chosen from the
        // unit and the peak alone. This ties the two together, for every dock size x monitor scale a user can have.
        let slots: Vec<Slot> = vec![Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon, Slot::Icon];
        let rest = rest_layout(&slots, 5.0 * 54.0 + 20.0);
        for unit in [0.85f32, 1.0, 1.1, 1.2, 1.25, 1.4, 1.5, 1.7, 1.75, 2.0, 2.4, 3.0] {
            for strength in [magnify::Strength::Subtle, magnify::Strength::Full] {
                let sizes = icon_sizes_with_small(unit, strength.peak(), unit);
                let pick = |w: f32| sizes[pick_level(&sizes, w * unit)] as f32;
                // at rest every icon is drawn at 44 units: the rest picture, pixel for pixel (within the rounding of a pixel)
                for &(_, w) in rest.iter().zip(&slots).filter(|(_, s)| **s == Slot::Icon).map(|(r, _)| r) {
                    assert!((pick(w) - w * unit).abs() <= 1.0, "rest at {unit}: drawn {} px, picture {} px, {sizes:?}", w * unit, pick(w));
                }
                // under the pointer at the full height of the bell: the icon is as big as it gets, and the top picture is the one
                let rest_width = 5.0 * 54.0 + 20.0;
                let centre_of_third = rest[3].0 + rest[3].1 / 2.0;
                let top = magnify::frame(&slots, &rest, rest_width, centre_of_third, 1.0, strength, 10_000.0);
                let biggest = top.spans.iter().zip(&slots).filter(|(_, s)| **s == Slot::Icon).map(|(s, _)| s.1).fold(0.0f32, f32::max);
                assert!((biggest - ICON_REST * strength.peak()).abs() < 0.01, "{biggest}");
                assert!((pick(biggest) - biggest * unit).abs() <= 1.0, "peak at {unit}: drawn {} px, picture {} px, {sizes:?}", biggest * unit, pick(biggest));
                assert_eq!(*sizes.last().unwrap() as f32, pick(biggest), "the peak is drawn with the largest picture");
                // in between, whatever the pointer and the height of the bell: not enlarged by more than a pixel, reduced by a quarter at most
                for step in 0..=20 {
                    let height = step as f32 / 20.0;
                    for x in (0..=24).map(|i| i as f32 * rest_width / 24.0) {
                        let f = magnify::frame(&slots, &rest, rest_width, x, height, strength, 10_000.0);
                        for (&(_, w), _) in f.spans.iter().zip(&slots).filter(|(_, s)| **s == Slot::Icon) {
                            let (drawn, kept) = (w * unit, pick(w));
                            assert!(kept + 1.0 >= drawn, "enlarged at {unit}: drawn {drawn}, picture {kept}, {sizes:?}");
                            assert!(kept / drawn <= MAX_STEP + 0.05, "reduced too much at {unit}: drawn {drawn}, picture {kept}, {sizes:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_small_picture_for_the_menus_is_below_the_rest_size_and_only_added_when_it_is() {
        // a 20 px picture at 100 %, 40 at 200 %: the preview's title (16) and the app menu's name row (18) are drawn at about that
        assert_eq!(icon_sizes_with_small(1.0, magnify::MAX_PEAK, 1.0), vec![20, 44, 55, 69]);
        assert_eq!(icon_sizes_with_small(2.0, magnify::MAX_PEAK, 2.0), vec![40, 88, 110, 137]);
        // a dock size of 0.85 on a 100 % monitor: the rest picture is 37 px, still above it (38 is the peak, rounded up)
        assert_eq!(icon_sizes_with_small(0.85, 1.0, 1.0), vec![20, 37, 38]);
        // nonsense scale of the monitor reads as 100 %
        assert_eq!(icon_sizes_with_small(1.0, 1.0, f32::NAN)[0], 20);
        // a rest picture that is not above it leaves the set alone
        assert_eq!(icon_sizes_with_small(0.3, 1.0, 1.0), icon_sizes(0.3, 1.0));
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
