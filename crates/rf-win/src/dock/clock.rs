//! The right island's clock: two lines, the time and the date, in the user's own formats.
//!
//! Windows describes a locale's formats as patterns (`HH:mm`, `h:mm tt`, `dd.MM.yyyy`, `M/d/yyyy`). [`format_pattern`] fills one in
//! for a given moment (pure, tested with the Turkish and the US patterns); [`system_lines`] reads the patterns of the user's locale
//! and the local time and puts the two together. [`ms_until_next_minute`] says when the lines can next change.

/// A moment of local time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    pub millisecond: u16,
}

/// The two lines under each other in the island (time above, date below), right aligned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockLines {
    pub time: String,
    pub date: String,
}

/// A locale's formats: Windows patterns plus the words for before and after noon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClockFormats {
    pub time: String,
    pub date: String,
    pub am: String,
    pub pm: String,
}

impl Default for ClockFormats {
    /// What a machine without readable locale data shows: 24 hour time and a day-first date.
    fn default() -> Self {
        ClockFormats { time: "HH:mm".into(), date: "dd.MM.yyyy".into(), am: "AM".into(), pm: "PM".into() }
    }
}

/// Fills a Windows time or date pattern. Understood: `d dd` (day), `M MM` (month number), `y yy yyyy` (year), `h hh` (12 hour clock),
/// `H HH` (24 hour clock), `m mm`, `s ss`, `t tt` (first letter of, or the whole AM / PM word) and `'quoted text'`. A month or day
/// NAME (`MMM`, `dddd`) would need the locale's word lists: those are written as their number, which keeps the line short and
/// readable (the taskbar's own short date never has them). Anything else is copied as it is.
pub fn format_pattern(pattern: &str, t: &ClockTime, am: &str, pm: &str) -> String {
    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // quoted text; '' is a quote
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' {
                    if chars.get(i + 1) == Some(&'\'') {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !matches!(c, 'd' | 'M' | 'y' | 'h' | 'H' | 'm' | 's' | 't') {
            out.push(c);
            i += 1;
            continue;
        }
        let mut n = 1;
        while chars.get(i + n) == Some(&c) {
            n += 1;
        }
        let hour12 = if t.hour.is_multiple_of(12) { 12 } else { t.hour % 12 };
        let two = |v: u32| format!("{v:02}");
        match c {
            'd' => out.push_str(&if n == 1 { t.day.to_string() } else { two(t.day as u32) }),
            'M' => out.push_str(&if n == 1 { t.month.to_string() } else { two(t.month as u32) }),
            'y' => match n {
                1 => out.push_str(&(t.year % 100).to_string()),
                2 => out.push_str(&two((t.year % 100) as u32)),
                _ => out.push_str(&format!("{:04}", t.year)),
            },
            'h' => out.push_str(&if n == 1 { hour12.to_string() } else { two(hour12 as u32) }),
            'H' => out.push_str(&if n == 1 { t.hour.to_string() } else { two(t.hour as u32) }),
            'm' => out.push_str(&if n == 1 { t.minute.to_string() } else { two(t.minute as u32) }),
            's' => out.push_str(&if n == 1 { t.second.to_string() } else { two(t.second as u32) }),
            't' => {
                let word = if t.hour < 12 { am } else { pm };
                if n == 1 {
                    out.extend(word.chars().next());
                } else {
                    out.push_str(word);
                }
            }
            _ => {}
        }
        i += n;
    }
    out
}

/// The two lines for a moment and a locale's formats.
pub fn lines(t: &ClockTime, f: &ClockFormats) -> ClockLines {
    ClockLines { time: format_pattern(&f.time, t, &f.am, &f.pm), date: format_pattern(&f.date, t, &f.am, &f.pm) }
}

/// How long until the minute changes (at least 1 ms, so a timer set from it never fires early and shows the old minute).
pub fn ms_until_next_minute(t: &ClockTime) -> u32 {
    let into = t.second as u32 * 1000 + t.millisecond as u32;
    (60_000 - into.min(59_999)).max(1)
}

/// The user's local time now.
pub fn local_time() -> ClockTime {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let s = unsafe { GetLocalTime() };
    ClockTime {
        year: s.wYear,
        month: s.wMonth as u8,
        day: s.wDay as u8,
        hour: s.wHour as u8,
        minute: s.wMinute as u8,
        second: s.wSecond as u8,
        millisecond: s.wMilliseconds,
    }
}

fn locale_string(kind: u32) -> Option<String> {
    use windows::Win32::Globalization::GetLocaleInfoEx;
    use windows::core::PCWSTR;
    // LOCALE_NAME_USER_DEFAULT is a null name
    let mut buf = [0u16; 80];
    let n = unsafe { GetLocaleInfoEx(PCWSTR::null(), kind, Some(&mut buf)) };
    (n > 1).then(|| crate::wide::from_wide(&buf)).filter(|s| !s.is_empty())
}

/// The patterns of the user's own locale (their short time, short date and AM / PM words), the defaults for what cannot be read.
pub fn system_formats() -> ClockFormats {
    use windows::Win32::Globalization::{LOCALE_S1159, LOCALE_S2359, LOCALE_SSHORTDATE, LOCALE_SSHORTTIME};
    let d = ClockFormats::default();
    ClockFormats {
        time: locale_string(LOCALE_SSHORTTIME).unwrap_or(d.time),
        date: locale_string(LOCALE_SSHORTDATE).unwrap_or(d.date),
        am: locale_string(LOCALE_S1159).unwrap_or(d.am),
        pm: locale_string(LOCALE_S2359).unwrap_or(d.pm),
    }
}

/// The clock as the island shows it now. The formats can change while the app runs (the user switches region settings): read
/// them again with [`system_formats`] now and then (at every minute change is cheap) rather than keeping them for ever.
pub fn system_lines() -> ClockLines {
    lines(&local_time(), &system_formats())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(h: u8, m: u8) -> ClockTime {
        ClockTime { year: 2026, month: 11, day: 12, hour: h, minute: m, second: 7, millisecond: 250 }
    }

    fn tr() -> ClockFormats {
        ClockFormats { time: "HH:mm".into(), date: "dd.MM.yyyy".into(), am: "ÖÖ".into(), pm: "ÖS".into() }
    }

    fn us() -> ClockFormats {
        ClockFormats { time: "h:mm tt".into(), date: "M/d/yyyy".into(), am: "AM".into(), pm: "PM".into() }
    }

    #[test]
    fn turkish_formats() {
        let l = lines(&at(14, 5), &tr());
        assert_eq!(l.time, "14:05");
        assert_eq!(l.date, "12.11.2026");
        assert_eq!(lines(&at(0, 9), &tr()).time, "00:09");
    }

    #[test]
    fn us_formats_use_the_12_hour_clock() {
        assert_eq!(lines(&at(14, 5), &us()).time, "2:05 PM");
        assert_eq!(lines(&at(14, 5), &us()).date, "11/12/2026");
        assert_eq!(lines(&at(0, 30), &us()).time, "12:30 AM", "midnight is 12, not 0");
        assert_eq!(lines(&at(12, 0), &us()).time, "12:00 PM");
        assert_eq!(lines(&at(9, 0), &us()).time, "9:00 AM");
    }

    #[test]
    fn other_patterns() {
        let t = at(7, 3);
        assert_eq!(format_pattern("yyyy-MM-dd", &t, "AM", "PM"), "2026-11-12");
        assert_eq!(format_pattern("d.M.yy", &t, "AM", "PM"), "12.11.26");
        assert_eq!(format_pattern("hh:mm t", &t, "AM", "PM"), "07:03 A");
        assert_eq!(format_pattern("H.mm.ss", &t, "AM", "PM"), "7.03.07");
        assert_eq!(format_pattern("HH 'saat' mm 'dk'", &t, "AM", "PM"), "07 saat 03 dk");
        assert_eq!(format_pattern("'it''s' H", &t, "AM", "PM"), "it's 7");
        // names are written as numbers, never dropped
        assert_eq!(format_pattern("d MMM yyyy", &t, "AM", "PM"), "12 11 2026");
        assert_eq!(format_pattern("", &t, "AM", "PM"), "");
        assert_eq!(format_pattern("dd/MM/yyyy", &ClockTime { year: 987, ..t }, "AM", "PM"), "12/11/0987");
    }

    #[test]
    fn next_change_is_at_the_next_minute() {
        assert_eq!(ms_until_next_minute(&at(1, 1)), 60_000 - 7_250);
        let end = ClockTime { second: 59, millisecond: 999, ..at(1, 1) };
        assert_eq!(ms_until_next_minute(&end), 1);
        let start = ClockTime { second: 0, millisecond: 0, ..at(1, 1) };
        assert_eq!(ms_until_next_minute(&start), 60_000);
    }

    #[test]
    fn the_defaults_are_sane() {
        let l = lines(&at(8, 8), &ClockFormats::default());
        assert_eq!((l.time.as_str(), l.date.as_str()), ("08:08", "12.11.2026"));
    }

    /// Read-only: the machine's own formats make two non-empty lines.
    #[test]
    fn system_clock_has_two_lines() {
        let l = system_lines();
        assert!(!l.time.is_empty() && !l.date.is_empty(), "{l:?}");
        assert!(l.time.contains(':') || l.time.contains('.'), "{l:?}");
    }
}
