//! An icon's label: at most two lines, centred under the icon. Pure; the width of a piece of text is a parameter (the view passes
//! the font's real measure, the tests a fixed one).
//!
//! RULES. A name that fits on one line stays on one line. Otherwise the first line ends at the last place where a name can be broken
//! (after a space, `-`, `_`, `.`, `(`, `[`, `,`) that still fits; a name with no such place, or whose first piece would be tiny, is
//! cut where the text no longer fits. A no-break space is not a place to break (Windows' own names use it: the Turkish Recycle
//! Bin is "Geri Dönüşüm Kutusu" with one after "Geri", and Explorer shows "Geri Dönüşü" / "m Kutusu"). Compared with Explorer on a
//! real desktop, a name of several words breaks between words there too ("Ultimate" / "Vocal R..."), and a rule tried here that
//! filled the first line whenever that showed the whole name ("Ultimate Voc" / "al Remover") was taken out again: it read worse.
//! The second line is what is left. When that does not fit either, its MIDDLE goes: the start and
//! the end of the line stay and an ellipsis stands between them, because the end of a file name is the part that tells files apart
//! (the extension, a number, a version). The end kept is the extension and the three characters before it, or the last four
//! characters of a name without one.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lines {
    pub first: String,
    pub second: Option<String>,
}

impl Lines {
    pub fn count(&self) -> usize {
        1 + usize::from(self.second.is_some())
    }
}

const ELLIPSIS: char = '…';

fn breaks_after(c: char) -> bool {
    matches!(c, ' ' | '-' | '_' | '.' | '(' | '[' | ',' | '+' | '&')
}

/// The longest prefix (in bytes, on a character boundary) of `s` that is at most `max_w` wide; at least one character.
fn longest_prefix(s: &str, max_w: f32, measure: &dyn Fn(&str) -> f32) -> usize {
    let ends: Vec<usize> = s.char_indices().map(|(i, c)| i + c.len_utf8()).collect();
    // widths grow with the prefix: binary search on the number of characters
    let (mut lo, mut hi) = (1usize, ends.len());
    if hi == 0 {
        return 0;
    }
    if measure(&s[..ends[0]]) > max_w {
        return ends[0];
    }
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if measure(&s[..ends[mid - 1]]) <= max_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    ends[lo - 1]
}

/// `text` with its middle replaced by an ellipsis so that it fits `max_w`, keeping the end described at the top of the file.
pub fn middle_ellipsis(text: &str, max_w: f32, measure: &dyn Fn(&str) -> f32) -> String {
    if measure(text) <= max_w {
        return text.to_string();
    }
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    let n = chars.len();
    let tail_chars = match text.rfind('.') {
        Some(dot) if dot > 0 && text[dot..].chars().count() <= 9 => text[dot..].chars().count() + 3,
        _ => 4,
    };
    let mut tail_n = tail_chars.min(n.saturating_sub(1)).max(1);
    loop {
        let tail = &text[chars[n - tail_n].0..];
        // the longest head that fits next to the ellipsis and the tail
        let fits = |head_n: usize| {
            let head = &text[..if head_n == 0 { 0 } else { chars[head_n - 1].0 + chars[head_n - 1].1.len_utf8() }];
            measure(&format!("{head}{ELLIPSIS}{tail}")) <= max_w
        };
        if fits(1) || tail_n == 1 {
            let max_head = n - tail_n;
            let (mut lo, mut hi) = (0usize, max_head);
            // fits() is monotone: a shorter head always fits if a longer one does
            while lo < hi {
                let mid = (lo + hi).div_ceil(2);
                if fits(mid) {
                    lo = mid;
                } else {
                    hi = mid - 1;
                }
            }
            let head_end = if lo == 0 { 0 } else { chars[lo - 1].0 + chars[lo - 1].1.len_utf8() };
            let head = text[..head_end].trim_end();
            return format!("{head}{ELLIPSIS}{tail}");
        }
        tail_n -= 1;
    }
}

pub fn wrap(text: &str, max_w: f32, measure: &dyn Fn(&str) -> f32) -> Lines {
    let text = text.trim();
    if text.is_empty() || measure(text) <= max_w {
        return Lines { first: text.to_string(), second: None };
    }
    // where the first line ends: the last place where a break is allowed inside what fits (between two characters, when the one
    // before is a break character or the one after is a space); a break so early that the first line would be a sliver is not taken
    let fit = longest_prefix(text, max_w, measure);
    let allowed = |p: usize| {
        text.is_char_boundary(p) && p > 0 && p < text.len() && (text[..p].chars().next_back().is_some_and(breaks_after) || text[p..].starts_with(' '))
    };
    let cut = (1..=fit).rev().find(|&p| allowed(p)).filter(|&p| measure(text[..p].trim_end()) * 3.0 >= max_w).unwrap_or(fit);
    let first = text[..cut].trim_end().to_string();
    let rest = text[cut..].trim_start();
    if rest.is_empty() {
        return Lines { first, second: None };
    }
    Lines { first, second: Some(middle_ellipsis(rest, max_w, measure)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every character is one unit wide.
    fn unit(s: &str) -> f32 {
        s.chars().count() as f32
    }
    /// 'i', 'l', '.', ' ' are half a unit, 'W' and 'M' one and a half, the rest one.
    fn varied(s: &str) -> f32 {
        s.chars().map(|c| if "il. ".contains(c) { 0.5 } else if "WM".contains(c) { 1.5 } else { 1.0 }).sum()
    }

    #[test]
    fn a_no_break_space_is_not_a_place_to_break() {
        // 11 units a line. Windows' own name of the Turkish Recycle Bin has a no-break space after "Geri": the first line is filled,
        // as on Explorer's desktop
        let l = wrap("Geri\u{a0}Dönüşüm Kutusu", 11.0, &unit);
        assert_eq!((l.first.as_str(), l.second.as_deref()), ("Geri\u{a0}Dönüşü", Some("m Kutusu")));
        // with an ordinary space the words stay whole and the second line loses its middle
        let l = wrap("Geri Dönüşüm Kutusu", 11.0, &unit);
        assert_eq!(l.first, "Geri");
        assert!(l.second.as_deref().is_some_and(|s| s.contains('…') && unit(s) <= 11.0), "{l:?}");
        // several words: between words, like Explorer
        let l = wrap("Ultimate Vocal Remover", 11.0, &unit);
        assert_eq!(l.first, "Ultimate");
        let l = wrap("Google Chrome", 11.0, &unit);
        assert_eq!((l.first.as_str(), l.second.as_deref()), ("Google", Some("Chrome")));
    }

    #[test]
    fn a_short_name_is_one_line() {
        assert_eq!(wrap("Notes.txt", 10.0, &unit), Lines { first: "Notes.txt".into(), second: None });
        assert_eq!(wrap("  padded  ", 10.0, &unit).first, "padded");
        assert_eq!(wrap("", 10.0, &unit), Lines { first: String::new(), second: None });
    }

    #[test]
    fn a_long_name_breaks_at_a_space_and_the_second_line_is_the_rest() {
        let l = wrap("Quarterly report final", 12.0, &unit);
        assert_eq!(l.first, "Quarterly");
        assert_eq!(l.second.as_deref(), Some("report final"));
        assert_eq!(l.count(), 2);
        // the space is not shown at the end of the first or the start of the second line
        let l = wrap("aaaaaaaaaa bbbbbbbbbb", 10.0, &unit);
        assert_eq!((l.first.as_str(), l.second.as_deref()), ("aaaaaaaaaa", Some("bbbbbbbbbb")));
    }

    #[test]
    fn a_name_without_a_break_is_cut_where_it_stops_fitting() {
        let l = wrap("abcdefghijklmnopqrstuvwxyz", 10.0, &unit);
        assert_eq!(l.first, "abcdefghij");
        // 16 characters left do not fit in 10: the middle goes, the end stays
        let second = l.second.unwrap();
        assert!(unit(&second) <= 10.0, "{second}");
        assert!(second.contains('…') && second.ends_with("wxyz"), "{second}");
        assert!(second.starts_with('k'), "{second}");
    }

    #[test]
    fn the_extension_and_a_few_characters_before_it_stay_visible() {
        let l = wrap("2026-11-12 holiday photos from the coast (final edit v3).jpeg", 20.0, &unit);
        let second = l.second.unwrap();
        assert!(unit(&second) <= 20.0, "{second}");
        assert!(second.ends_with("v3).jpeg") || second.ends_with("3).jpeg"), "{second}");
        assert!(second.contains('…'));
        // the first line is whole words or a clean cut, never longer than the width
        assert!(unit(&l.first) <= 20.0, "{}", l.first);
    }

    #[test]
    fn a_very_narrow_label_still_gives_something_that_fits() {
        for w in [1.0f32, 2.0, 3.0, 5.0] {
            let l = wrap("some_long_file_name.final.tar.gz", w, &unit);
            assert!(unit(&l.first) <= w.max(1.0), "{w}: {}", l.first);
            if let Some(s) = &l.second {
                assert!(unit(s) <= w.max(1.0) + 1.0, "{w}: {s}"); // an ellipsis and one character is the least there is
            }
        }
    }

    #[test]
    fn it_works_with_characters_of_different_widths_and_multibyte_text() {
        let l = wrap("WWWWWWWWWW iiiiiiiiii", 8.0, &varied);
        assert!(varied(&l.first) <= 8.0, "{}", l.first);
        let l = wrap("Çok uzun bir Türkçe dosya adı örneği.docx", 12.0, &unit);
        assert!(unit(&l.first) <= 12.0);
        let second = l.second.unwrap();
        assert!(unit(&second) <= 12.0 && second.ends_with(".docx"), "{second}");
        // multibyte characters are never cut in the middle
        let l = wrap("ğüşiöçĞÜŞİÖÇğüşiöçĞÜŞİÖÇ", 7.0, &unit);
        assert_eq!(l.first.chars().count(), 7);
    }

    #[test]
    fn the_middle_ellipsis_alone() {
        assert_eq!(middle_ellipsis("short.txt", 20.0, &unit), "short.txt");
        let m = middle_ellipsis("abcdefghijklmnopqrstuvwxyz.txt", 12.0, &unit);
        assert!(unit(&m) <= 12.0 && m.ends_with("xyz.txt") && m.starts_with("abc"), "{m}");
        // a name that is nothing but a long extension-less run keeps its last four characters
        let m = middle_ellipsis("abcdefghijklmnopqrstuvwxyz", 9.0, &unit);
        assert_eq!(unit(&m), 9.0);
        assert!(m.ends_with("wxyz"), "{m}");
    }
}
