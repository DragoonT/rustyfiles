//! The calendar of the clock's panel (a click on the clock in the right island): the arithmetic of dates, the month grid, the names of
//! months and days, and how the keyboard moves through it. All of it is pure and tested; the two small calls into Windows at the end
//! only read the user's first day of the week and the long time pattern.
//!
//! DATES. A [`Date`] is a proleptic Gregorian day (the civil calendar every Windows locale uses for its clock). The panel works in the
//! years [`MIN_YEAR`]..=[`MAX_YEAR`] (1900..2100): the week-day arithmetic is exact for any year, the limit is only where the
//! interface stops (a month button that would go past it does nothing). No time zones: the panel shows the local date and time, as the
//! island's clock does, and nothing else. No agenda: there is no event source that works without an account and a package.
//!
//! THE GRID. [`month_grid`] always gives six rows of seven days, so the panel keeps its height from month to month. The week starts on
//! the user's own first day ([`first_day_of_week`], Windows' `LOCALE_IFIRSTDAYOFWEEK`: 0 is Monday, 6 is Sunday). Days of the
//! neighbouring months are in the grid, marked `in_month: false` (the panel dims them). Week numbers are not shown.
//!
//! NAMES. The month and day names are this module's own tables, Turkish and English, picked by the app's language, not by the
//! Windows region: the rest of the panel's words come from the app's language setting, and a mix of the two would read badly. Any
//! other language of the app gets the English table. (Windows' locale API could give the names of any locale, at the price of the
//! panel speaking Windows' language where the app speaks another.)

/// The first and last year the panel goes to.
pub const MIN_YEAR: i32 = 1900;
pub const MAX_YEAR: i32 = 2100;

/// The language of the names. Anything but Turkish is English.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Tr,
    En,
}

/// A day of the calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

pub fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

pub fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(year) => 29,
        2 => 28,
        _ => 0,
    }
}

/// Days since 1970-01-01 of a civil date (Howard Hinnant's algorithm; exact for every year, negative before 1970).
pub fn days_from_civil(year: i32, month: u8, day: u8) -> i64 {
    let y = i64::from(year) - i64::from(month <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The civil date of a day count (the inverse of [`days_from_civil`]).
pub fn civil_from_days(days: i64) -> (i32, u8, u8) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u8;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u8;
    let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
    (year, month, day)
}

impl Date {
    /// The date, or `None` when the month or the day does not exist.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Date> {
        ((1..=12).contains(&month) && day >= 1 && day <= days_in_month(year, month)).then_some(Date { year, month, day })
    }

    pub fn first() -> Date {
        Date { year: MIN_YEAR, month: 1, day: 1 }
    }

    pub fn last() -> Date {
        Date { year: MAX_YEAR, month: 12, day: 31 }
    }

    pub fn days(self) -> i64 {
        days_from_civil(self.year, self.month, self.day)
    }

    pub fn from_days(days: i64) -> Date {
        let (year, month, day) = civil_from_days(days);
        Date { year, month, day }
    }

    /// 0 is Monday, 6 is Sunday. (1970-01-01 was a Thursday.)
    pub fn weekday(self) -> u8 {
        (self.days() + 3).rem_euclid(7) as u8
    }

    /// The date `n` days later (earlier for a negative `n`), held inside [`MIN_YEAR`]..=[`MAX_YEAR`].
    pub fn add_days(self, n: i64) -> Date {
        Date::from_days(self.days() + n).clamped()
    }

    /// The same day `n` months later; a day the month does not have (31 March plus one month) becomes the last day of the month.
    /// Held inside the years of the panel.
    pub fn add_months(self, n: i32) -> Date {
        let index = self.year * 12 + i32::from(self.month) - 1 + n;
        let (year, month) = (index.div_euclid(12), (index.rem_euclid(12) + 1) as u8);
        let day = self.day.min(days_in_month(year, month));
        Date { year, month, day }.clamped()
    }

    pub fn clamped(self) -> Date {
        if self < Date::first() {
            Date::first()
        } else if self > Date::last() {
            Date::last()
        } else {
            self
        }
    }
}

/// One day of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub date: Date,
    /// The day belongs to the month the grid is of (a day of the month before or after is dimmed).
    pub in_month: bool,
    pub today: bool,
}

/// Six rows of seven days.
pub type Grid = [[Cell; 7]; 6];

/// The grid of a month. `first_dow` is the day the week starts on (0 Monday .. 6 Sunday; a larger number is read modulo 7). The grid
/// starts on the last such day on or before the 1st of the month. Dates outside the years of the panel are still given (the grid of
/// January 1900 starts in December 1899): they are only dimmed days.
pub fn month_grid(year: i32, month: u8, first_dow: u8, today: Option<Date>) -> Grid {
    let month = month.clamp(1, 12);
    let first = Date { year, month, day: 1 };
    let back = i64::from((first.weekday() + 7 - first_dow % 7) % 7);
    let start = first.days() - back;
    std::array::from_fn(|row| {
        std::array::from_fn(|col| {
            let date = Date::from_days(start + (row * 7 + col) as i64);
            Cell { date, in_month: date.year == year && date.month == month, today: today == Some(date) }
        })
    })
}

/// Windows' value of the first day of the week as a number 0 (Monday) .. 6 (Sunday); anything that is not such a number is Monday.
pub fn parse_first_dow(text: &str) -> u8 {
    text.trim().parse::<u8>().ok().filter(|n| *n <= 6).unwrap_or(0)
}

/// The weekday numbers in the order the columns of a grid starting on `first_dow` have them.
pub fn column_weekdays(first_dow: u8) -> [u8; 7] {
    std::array::from_fn(|i| (first_dow % 7 + i as u8) % 7)
}

// ------------------------------------------------------------------ names

const MONTHS_TR: [&str; 12] = ["Ocak", "Şubat", "Mart", "Nisan", "Mayıs", "Haziran", "Temmuz", "Ağustos", "Eylül", "Ekim", "Kasım", "Aralık"];
const MONTHS_EN: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS_TR: [&str; 7] = ["Pazartesi", "Salı", "Çarşamba", "Perşembe", "Cuma", "Cumartesi", "Pazar"];
const DAYS_EN: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
const DAYS_SHORT_TR: [&str; 7] = ["Pt", "Sa", "Ça", "Pe", "Cu", "Ct", "Pz"];
const DAYS_SHORT_EN: [&str; 7] = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];

/// A month's name (1 January .. 12 December). An impossible number gives an empty name.
pub fn month_name(month: u8, lang: Lang) -> &'static str {
    let table = if lang == Lang::Tr { &MONTHS_TR } else { &MONTHS_EN };
    table.get(usize::from(month).wrapping_sub(1)).copied().unwrap_or("")
}

/// A weekday's name (0 Monday .. 6 Sunday).
pub fn day_name(weekday: u8, lang: Lang) -> &'static str {
    let table = if lang == Lang::Tr { &DAYS_TR } else { &DAYS_EN };
    table.get(usize::from(weekday)).copied().unwrap_or("")
}

/// A weekday's two-letter head for the top of a grid column.
pub fn day_short(weekday: u8, lang: Lang) -> &'static str {
    let table = if lang == Lang::Tr { &DAYS_SHORT_TR } else { &DAYS_SHORT_EN };
    table.get(usize::from(weekday)).copied().unwrap_or("")
}

/// The title over the grid: "Kasım 2026" / "November 2026".
pub fn month_title(year: i32, month: u8, lang: Lang) -> String {
    format!("{} {}", month_name(month, lang), year)
}

/// Today's long date, the top of the panel: "12 Kasım 2026 Perşembe" / "Thursday, November 12, 2026".
pub fn long_date(d: Date, lang: Lang) -> String {
    match lang {
        Lang::Tr => format!("{} {} {} {}", d.day, month_name(d.month, lang), d.year, day_name(d.weekday(), lang)),
        Lang::En => format!("{}, {} {}, {}", day_name(d.weekday(), lang), month_name(d.month, lang), d.day, d.year),
    }
}

// ------------------------------------------------------------------ the keyboard

/// The keys the grid answers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    Home,
}

impl Key {
    pub fn from_name(name: &str) -> Option<Key> {
        Some(match name {
            "left" => Key::Left,
            "right" => Key::Right,
            "up" => Key::Up,
            "down" => Key::Down,
            "pageup" => Key::PageUp,
            "pagedown" => Key::PageDown,
            "home" => Key::Home,
            _ => return None,
        })
    }
}

/// Where a key takes the chosen day: the arrows a day (left, right) or a week (up, down: the day above or below in the grid), the page
/// keys a month, Home today. Held inside the panel's years.
pub fn key_target(chosen: Date, key: Key, today: Date) -> Date {
    match key {
        Key::Left => chosen.add_days(-1),
        Key::Right => chosen.add_days(1),
        Key::Up => chosen.add_days(-7),
        Key::Down => chosen.add_days(7),
        Key::PageUp => chosen.add_months(-1),
        Key::PageDown => chosen.add_months(1),
        Key::Home => today.clamped(),
    }
}

// ------------------------------------------------------------------ Windows

/// The user's first day of the week: 0 Monday .. 6 Sunday (`LOCALE_IFIRSTDAYOFWEEK` of the user's locale). Monday when it cannot be
/// read.
pub fn first_day_of_week() -> u8 {
    use windows::Win32::Globalization::{GetLocaleInfoEx, LOCALE_IFIRSTDAYOFWEEK};
    use windows::core::PCWSTR;
    let mut buf = [0u16; 8];
    // (LOCALE_NAME_USER_DEFAULT is a null name)
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_IFIRSTDAYOFWEEK, Some(&mut buf)) };
    if n > 1 { parse_first_dow(&crate::wide::from_wide(&buf)) } else { 0 }
}

/// The short pattern with seconds in it: `H:mm` becomes `H:mm:ss`, `h:mm tt` becomes `h:mm:ss tt`, `HH.mm` becomes `HH.mm.ss` (the
/// separator between hours and minutes is used again). A pattern that has seconds is returned as it is; one with no minutes at all
/// gets `HH:mm:ss`.
pub fn with_seconds(pattern: &str) -> String {
    if pattern.contains('s') {
        return pattern.to_string();
    }
    let Some(at) = pattern.find("mm") else { return "HH:mm:ss".to_string() };
    let sep: String = pattern[..at].chars().rev().take_while(|c| !c.is_alphabetic()).collect::<Vec<_>>().into_iter().rev().collect();
    let sep = if sep.is_empty() { ":" } else { sep.as_str() };
    let end = at + 2;
    format!("{}{sep}ss{}", &pattern[..end], &pattern[end..])
}

fn locale_text(lctype: u32) -> String {
    use windows::Win32::Globalization::GetLocaleInfoEx;
    use windows::core::PCWSTR;
    let mut buf = [0u16; 80];
    // (LOCALE_NAME_USER_DEFAULT is a null name)
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), lctype, Some(&mut buf)) };
    if n > 1 { crate::wide::from_wide(&buf) } else { String::new() }
}

/// The user's long time pattern (with seconds: `HH:mm:ss`, `h:mm:ss tt`), for the clock in the panel. When the locale's long pattern
/// has no seconds, the short pattern with `:ss` added ([`with_seconds`]), so a 12-hour clock stays a 12-hour clock.
pub fn long_time_pattern() -> String {
    use windows::Win32::Globalization::{LOCALE_SSHORTTIME, LOCALE_STIMEFORMAT};
    let long = locale_text(LOCALE_STIMEFORMAT);
    if long.contains('s') { long } else { with_seconds(&locale_text(LOCALE_SSHORTTIME)) }
}

/// Today's date by the local clock, held inside the panel's years.
pub fn today() -> Date {
    let t = super::clock::local_time();
    Date { year: i32::from(t.year), month: t.month, day: t.day }.clamped()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u8, day: u8) -> Date {
        Date::new(y, m, day).unwrap()
    }

    #[test]
    fn leap_years_follow_the_gregorian_rule() {
        for (y, leap) in [(1900, false), (1904, true), (2000, true), (2024, true), (2026, false), (2028, true), (2100, false)] {
            assert_eq!(is_leap(y), leap, "{y}");
            assert_eq!(days_in_month(y, 2), if leap { 29 } else { 28 }, "{y}");
        }
        assert_eq!([1, 3, 4, 6, 11, 12].map(|m| days_in_month(2026, m)), [31, 31, 30, 30, 30, 31]);
        assert_eq!(days_in_month(2026, 0), 0);
        assert_eq!(days_in_month(2026, 13), 0);
    }

    #[test]
    fn dates_that_do_not_exist_are_refused() {
        assert!(Date::new(2024, 2, 29).is_some());
        assert!(Date::new(2026, 2, 29).is_none());
        assert!(Date::new(1900, 2, 29).is_none());
        assert!(Date::new(2026, 4, 31).is_none());
        assert!(Date::new(2026, 0, 1).is_none());
        assert!(Date::new(2026, 12, 0).is_none());
    }

    #[test]
    fn the_day_count_and_the_weekday_are_exact_across_the_whole_range() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(d(1970, 1, 1).weekday(), 3, "a Thursday");
        // known days: 1900-01-01 was a Monday, 2000-01-01 a Saturday, 2026-11-12 a Thursday, 2100-12-31 a Friday
        assert_eq!(d(1900, 1, 1).weekday(), 0);
        assert_eq!(d(2000, 1, 1).weekday(), 5);
        assert_eq!(d(2026, 11, 12).weekday(), 3);
        assert_eq!(d(2100, 12, 31).weekday(), 4);
        assert_eq!(d(2024, 2, 29).weekday(), 3);
        // the round trip over every day of the panel's years, and the weekday advancing by one each time
        let (mut day, mut wd) = (Date::first().days(), Date::first().weekday());
        let end = Date::last().days();
        while day <= end {
            let date = Date::from_days(day);
            assert_eq!(date.days(), day);
            assert_eq!(date.weekday(), wd);
            assert_eq!(Date::new(date.year, date.month, date.day), Some(date));
            day += 1;
            wd = (wd + 1) % 7;
        }
    }

    #[test]
    fn days_and_months_are_added_and_held_inside_the_panels_years() {
        assert_eq!(d(2026, 11, 30).add_days(1), d(2026, 12, 1));
        assert_eq!(d(2026, 1, 1).add_days(-1), d(2025, 12, 31));
        assert_eq!(d(2024, 2, 28).add_days(1), d(2024, 2, 29));
        assert_eq!(d(2026, 2, 28).add_days(1), d(2026, 3, 1));
        assert_eq!(d(2026, 3, 31).add_months(1), d(2026, 4, 30), "a day the month does not have becomes its last");
        assert_eq!(d(2024, 1, 31).add_months(1), d(2024, 2, 29));
        assert_eq!(d(2026, 12, 15).add_months(1), d(2027, 1, 15));
        assert_eq!(d(2026, 1, 15).add_months(-1), d(2025, 12, 15));
        assert_eq!(d(2026, 11, 12).add_months(-23), d(2024, 12, 12));
        // the ends
        assert_eq!(d(1900, 1, 1).add_days(-1), d(1900, 1, 1));
        assert_eq!(d(1900, 1, 20).add_months(-1), d(1900, 1, 1), "clamped to the first day of the first year");
        assert_eq!(d(2100, 12, 31).add_days(1), d(2100, 12, 31));
        assert_eq!(d(2100, 12, 5).add_months(1), d(2100, 12, 31));
    }

    #[test]
    fn a_grid_is_six_rows_of_seven_starting_on_the_first_day_of_the_week() {
        // November 2026 starts on a Sunday. Monday first: the 1st is in the last column of row 0.
        let g = month_grid(2026, 11, 0, None);
        assert_eq!(g[0][0].date, d(2026, 10, 26));
        assert!(!g[0][0].in_month);
        assert_eq!(g[0][6].date, d(2026, 11, 1));
        assert!(g[0][6].in_month);
        // Sunday first: the 1st is the first cell
        let g = month_grid(2026, 11, 6, None);
        assert_eq!(g[0][0].date, d(2026, 11, 1));
        assert!(g[0][0].in_month);
        // the last row ends where six weeks end
        assert_eq!(g[5][6].date, d(2026, 12, 12));
        // every cell is the day after the one before it
        for first in 0..7 {
            let g = month_grid(2026, 2, first, None);
            let flat: Vec<Date> = g.iter().flatten().map(|c| c.date).collect();
            assert_eq!(flat.len(), 42);
            assert!(flat.windows(2).all(|w| w[1].days() == w[0].days() + 1), "first day {first}");
            // the column heads and the cells agree: row 0 column i is a day with the head's weekday
            for (i, wd) in column_weekdays(first).into_iter().enumerate() {
                assert_eq!(g[0][i].date.weekday(), wd, "first day {first}, column {i}");
            }
            // each day of the month appears once, as in_month
            assert_eq!(g.iter().flatten().filter(|c| c.in_month).count(), 28, "February 2026");
        }
    }

    #[test]
    fn leap_february_and_the_month_lengths_show_in_the_grid() {
        let in_month = |y, m| month_grid(y, m, 0, None).iter().flatten().filter(|c| c.in_month).count();
        assert_eq!(in_month(2024, 2), 29);
        assert_eq!(in_month(2023, 2), 28);
        assert_eq!(in_month(1900, 2), 28, "1900 is not a leap year");
        assert_eq!(in_month(2000, 2), 29);
        assert_eq!(in_month(2100, 2), 28);
        for m in 1..=12 {
            assert_eq!(in_month(2026, m), usize::from(days_in_month(2026, m)), "month {m}");
        }
    }

    #[test]
    fn the_grid_marks_today_once_and_only_in_its_own_cell() {
        let today = d(2026, 11, 12);
        let g = month_grid(2026, 11, 0, Some(today));
        let marked: Vec<&Cell> = g.iter().flatten().filter(|c| c.today).collect();
        assert_eq!(marked.len(), 1);
        assert_eq!(marked[0].date, today);
        assert!(marked[0].in_month);
        // a month that shows today as a day of the next month marks it there too (the number is on the screen; October 2026 runs to 8 November)
        let g = month_grid(2026, 10, 0, Some(d(2026, 11, 3)));
        assert_eq!(g.iter().flatten().filter(|c| c.today).count(), 1);
        assert!(!g.iter().flatten().find(|c| c.today).unwrap().in_month);
        // a month far away has no today
        assert_eq!(month_grid(2025, 3, 0, Some(today)).iter().flatten().filter(|c| c.today).count(), 0);
        assert_eq!(month_grid(2026, 11, 0, None).iter().flatten().filter(|c| c.today).count(), 0);
    }

    #[test]
    fn the_ends_of_the_range_make_a_grid_with_dimmed_days_outside_the_years() {
        let g = month_grid(1900, 1, 0, None);
        // 1900-01-01 is a Monday: the grid starts on it
        assert_eq!(g[0][0].date, d(1900, 1, 1));
        let g = month_grid(1900, 1, 6, None);
        assert_eq!(g[0][0].date, d(1899, 12, 31), "Sunday first: the week of the 1st starts in 1899");
        assert!(!g[0][0].in_month);
        let g = month_grid(2100, 12, 0, None);
        assert_eq!(g.iter().flatten().filter(|c| c.in_month).count(), 31);
        assert_eq!(g[5][6].date.year, 2101, "the last row runs into 2101");
        // a month number out of range is held to a month
        assert_eq!(month_grid(2026, 0, 0, None)[1][0].date.year, 2026);
    }

    #[test]
    fn the_first_day_of_the_week_is_read_as_windows_writes_it() {
        assert_eq!(parse_first_dow("0"), 0);
        assert_eq!(parse_first_dow("6"), 6);
        assert_eq!(parse_first_dow(" 3 "), 3);
        assert_eq!(parse_first_dow("7"), 0);
        assert_eq!(parse_first_dow(""), 0);
        assert_eq!(parse_first_dow("x"), 0);
        for first in 0..7 {
            let heads = column_weekdays(first);
            assert_eq!(heads[0], first);
            let mut sorted = heads;
            sorted.sort_unstable();
            assert_eq!(sorted, [0, 1, 2, 3, 4, 5, 6]);
        }
        assert_eq!(column_weekdays(0), [0, 1, 2, 3, 4, 5, 6]);
        assert_eq!(column_weekdays(6), [6, 0, 1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_names_are_turkish_or_english() {
        assert_eq!(month_name(11, Lang::Tr), "Kasım");
        assert_eq!(month_name(11, Lang::En), "November");
        assert_eq!(month_name(1, Lang::Tr), "Ocak");
        assert_eq!(month_name(8, Lang::Tr), "Ağustos");
        assert_eq!(month_name(0, Lang::En), "");
        assert_eq!(month_name(13, Lang::Tr), "");
        assert_eq!(day_name(0, Lang::Tr), "Pazartesi");
        assert_eq!(day_name(6, Lang::Tr), "Pazar");
        assert_eq!(day_name(3, Lang::En), "Thursday");
        assert_eq!(day_name(7, Lang::En), "");
        assert_eq!(day_short(0, Lang::Tr), "Pt");
        assert_eq!(day_short(2, Lang::Tr), "Ça");
        assert_eq!(day_short(6, Lang::En), "Su");
        // every table is complete and no name is empty
        for lang in [Lang::Tr, Lang::En] {
            for m in 1..=12 {
                assert!(!month_name(m, lang).is_empty());
            }
            for w in 0..7 {
                assert!(!day_name(w, lang).is_empty() && !day_short(w, lang).is_empty());
            }
        }
        assert_eq!(month_title(2026, 11, Lang::Tr), "Kasım 2026");
        assert_eq!(month_title(2026, 11, Lang::En), "November 2026");
        assert_eq!(long_date(d(2026, 11, 12), Lang::Tr), "12 Kasım 2026 Perşembe");
        assert_eq!(long_date(d(2026, 11, 12), Lang::En), "Thursday, November 12, 2026");
        assert_eq!(long_date(d(2024, 2, 29), Lang::En), "Thursday, February 29, 2024");
    }

    #[test]
    fn the_keys_move_a_day_a_week_a_month_or_to_today() {
        let today = d(2026, 11, 12);
        let at = |from, key| key_target(from, key, today);
        let c = d(2026, 3, 31);
        assert_eq!(at(c, Key::Left), d(2026, 3, 30));
        assert_eq!(at(c, Key::Right), d(2026, 4, 1));
        assert_eq!(at(c, Key::Up), d(2026, 3, 24));
        assert_eq!(at(c, Key::Down), d(2026, 4, 7));
        assert_eq!(at(c, Key::PageDown), d(2026, 4, 30));
        assert_eq!(at(c, Key::PageUp), d(2026, 2, 28));
        assert_eq!(at(c, Key::Home), today);
        // the ends
        assert_eq!(at(d(1900, 1, 1), Key::Left), d(1900, 1, 1));
        assert_eq!(at(d(2100, 12, 31), Key::Right), d(2100, 12, 31));
        assert_eq!(at(d(2100, 12, 31), Key::PageDown), d(2100, 12, 31));
        assert_eq!(Key::from_name("pagedown"), Some(Key::PageDown));
        assert_eq!(Key::from_name("home"), Some(Key::Home));
        assert_eq!(Key::from_name("enter"), None);
    }

    #[test]
    fn seconds_are_added_to_a_short_pattern_keeping_its_clock() {
        assert_eq!(with_seconds("H:mm"), "H:mm:ss");
        assert_eq!(with_seconds("HH:mm"), "HH:mm:ss");
        assert_eq!(with_seconds("h:mm tt"), "h:mm:ss tt", "a 12-hour clock stays one");
        assert_eq!(with_seconds("tt h:mm"), "tt h:mm:ss");
        assert_eq!(with_seconds("HH.mm"), "HH.mm.ss", "the separator between hours and minutes is reused");
        assert_eq!(with_seconds("HH:mm:ss"), "HH:mm:ss", "already has seconds");
        assert_eq!(with_seconds("h:mm:ss tt"), "h:mm:ss tt");
        assert_eq!(with_seconds(""), "HH:mm:ss");
        assert_eq!(with_seconds("H"), "HH:mm:ss", "no minutes: the fixed pattern");
    }

    /// Read-only: what this machine's locale says.
    #[test]
    fn the_machines_own_week_and_time_pattern_are_sane() {
        assert!(first_day_of_week() <= 6);
        let p = long_time_pattern();
        assert!(p.contains('s'), "{p:?}");
        let t = today();
        assert!((MIN_YEAR..=MAX_YEAR).contains(&t.year));
        assert!(Date::new(t.year, t.month, t.day).is_some());
    }
}
