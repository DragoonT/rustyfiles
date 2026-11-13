//! The Start panel's "Tüm uygulamalar" list: the order of the names (the current language's alphabet, Turkish letters included), the
//! letter headers between them, the flat rows the virtual list shows, and how the arrow keys move through the pinned grid and the
//! list. Pure: names in, rows out.

use std::cmp::Ordering;

use crate::apps::App;

/// The Turkish alphabet in the order of CLDR's `tr` collation.
const TURKISH: &str = "abcçdefgğhıijklmnoöpqrsştuüvwxyz";

/// What a character sorts as, ahead of letters: symbols, then digits, then letters in the alphabet's order.
fn rank(c: char, turkish: bool) -> u32 {
    let lower = fold_case(c, turkish);
    if let Some(i) = if turkish { TURKISH.chars().position(|a| a == lower) } else { None } {
        return 1_000 + i as u32;
    }
    if lower.is_ascii_digit() {
        return 500 + lower as u32 - '0' as u32;
    }
    if lower.is_alphabetic() {
        let base = strip_accent(lower);
        if base.is_ascii_lowercase() {
            // the same slots as the Turkish letters of the same name, so a Turkish `c` and a Turkish `ç` are neighbours too
            let i = if turkish { TURKISH.chars().position(|a| a == base).unwrap_or(0) } else { base as usize - 'a' as usize };
            return 1_000 + i as u32;
        }
        // another alphabet: after the Latin letters, by code point
        return 2_000 + lower as u32;
    }
    // spaces and punctuation, by code point, before the digits
    (lower as u32).min(499)
}

/// Lower case by the language's rules: in Turkish `I` is the dotless `ı` and `İ` is the dotted `i`.
fn fold_case(c: char, turkish: bool) -> char {
    match (c, turkish) {
        ('I', true) => 'ı',
        ('İ', _) => 'i',
        _ => c.to_lowercase().next().unwrap_or(c),
    }
}

/// A letter without its mark, for languages without that letter in their alphabet.
fn strip_accent(c: char) -> char {
    match c {
        'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'ç' => 'c',
        'è' | 'é' | 'ê' | 'ë' => 'e',
        'ì' | 'í' | 'î' | 'ï' | 'ı' => 'i',
        'ñ' => 'n',
        'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => 'o',
        'ù' | 'ú' | 'û' | 'ü' => 'u',
        'ý' | 'ÿ' => 'y',
        'ş' => 's',
        'ğ' => 'g',
        other => other,
    }
}

/// The sort key of a name: one rank per character. Names are compared by this first, so `Çalış` comes after `Cep` and before `Dosya`
/// in Turkish, and `ı` before `i`.
pub fn key(name: &str, turkish: bool) -> Vec<u32> {
    name.trim().chars().map(|c| rank(c, turkish)).collect()
}

/// The order of two names.
pub fn compare(a: &str, b: &str, turkish: bool) -> Ordering {
    key(a, turkish)
        .cmp(&key(b, turkish))
        // the same letters in a different case or with different marks: a stable, deterministic order
        .then_with(|| a.to_lowercase().cmp(&b.to_lowercase()))
        .then_with(|| a.cmp(b))
}

/// Sorts the apps by name in the language's order.
pub fn sort_apps(apps: &mut [App], turkish: bool) {
    // the keys are worked out once per app, not once per comparison (hundreds of apps, thousands of comparisons)
    apps.sort_by_cached_key(|a| (key(&a.name, turkish), a.name.to_lowercase(), a.name.clone()));
}

/// The header a name sits under: its first letter in capitals (`İ` and `I` are two headers in Turkish), `#` for digits, `&` for
/// everything else.
pub fn header_of(name: &str, turkish: bool) -> String {
    let Some(c) = name.trim().chars().next() else { return "&".into() };
    let r = rank(c, turkish);
    if (1_000..2_000).contains(&r) {
        let letter = if turkish { TURKISH.chars().nth((r - 1_000) as usize).unwrap_or(c) } else { char::from_u32('a' as u32 + r - 1_000).unwrap_or(c) };
        return upper(letter, turkish).to_string();
    }
    if r >= 2_000 {
        return c.to_uppercase().next().unwrap_or(c).to_string();
    }
    if (500..510).contains(&r) { "#".into() } else { "&".into() }
}

fn upper(c: char, turkish: bool) -> char {
    match (c, turkish) {
        ('i', true) => 'İ',
        ('ı', _) => 'I',
        _ => c.to_uppercase().next().unwrap_or(c),
    }
}

/// One row of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    Header(String),
    /// An app: its index in the sorted list.
    App(usize),
}

/// The rows for apps that are already sorted: a header before the first app of each letter.
pub fn rows(sorted: &[App], turkish: bool) -> Vec<Row> {
    let mut out = Vec::with_capacity(sorted.len() + 32);
    let mut last: Option<String> = None;
    for (i, a) in sorted.iter().enumerate() {
        let h = header_of(&a.name, turkish);
        if last.as_ref() != Some(&h) {
            out.push(Row::Header(h.clone()));
            last = Some(h);
        }
        out.push(Row::App(i));
    }
    out
}

/// The row an app has in [`rows`]' output (for scrolling it into view).
pub fn row_of_app(rows: &[Row], app: usize) -> Option<usize> {
    rows.iter().position(|r| *r == Row::App(app))
}

/// The app on the last row of the list that is fully in view, for a list scrolled `top_px` down (a positive number) in a viewport
/// `viewport_px` high whose rows are all `row_px` high (the list is a uniform one: headers and apps are the same height). A header on
/// that row gives the app above it. `None`: nothing measured yet (a viewport or row of no height) or no app at or above that row.
pub fn last_visible_app(rows: &[Row], top_px: f32, viewport_px: f32, row_px: f32) -> Option<usize> {
    if !(row_px > 0.0 && viewport_px >= row_px && top_px.is_finite() && viewport_px.is_finite()) || rows.is_empty() {
        return None;
    }
    // rows [0, n) end at or above the viewport's bottom edge; a row cut by the edge is not "fully in view"
    let n = (((top_px.max(0.0) + viewport_px) / row_px + 1e-3).floor() as usize).min(rows.len());
    rows[..n].iter().rev().find_map(|r| match r {
        Row::App(j) => Some(*j),
        Row::Header(_) => None,
    })
}

// ---------------------------------------------------------------- the keyboard

/// The pinned grid's width in cells.
pub const GRID_COLS: usize = 6;

/// Where the keyboard cursor is: on a pinned app or on an app of the list (its index in the sorted list).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cursor {
    Pin(usize),
    App(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
}

/// How many apps a page key jumps over in the list.
pub const PAGE: usize = 8;

/// The cursor after an arrow or page key. `None` in, a down or right arrow lands on the first thing there is; an arrow that has
/// nowhere to go leaves the cursor where it is. Left and right only move inside the grid (in the list the keys belong to the text
/// field).
pub fn step(cur: Option<Cursor>, mv: Move, pins: usize, apps: usize) -> Option<Cursor> {
    let first = || if pins > 0 { Some(Cursor::Pin(0)) } else if apps > 0 { Some(Cursor::App(0)) } else { None };
    let Some(cur) = cur else {
        return match mv {
            Move::Down | Move::Right | Move::PageDown => first(),
            _ => None,
        };
    };
    match cur {
        Cursor::Pin(i) => {
            let i = i.min(pins.saturating_sub(1));
            Some(match mv {
                Move::Left => Cursor::Pin(i.saturating_sub(1)),
                Move::Right => Cursor::Pin((i + 1).min(pins.saturating_sub(1))),
                Move::Up | Move::PageUp => Cursor::Pin(if i >= GRID_COLS { i - GRID_COLS } else { i }),
                Move::Down | Move::PageDown => {
                    if (i / GRID_COLS + 1) * GRID_COLS < pins {
                        Cursor::Pin((i + GRID_COLS).min(pins - 1))
                    } else if apps > 0 {
                        Cursor::App(0)
                    } else {
                        Cursor::Pin(i)
                    }
                }
            })
        }
        Cursor::App(j) => {
            let j = j.min(apps.saturating_sub(1));
            Some(match mv {
                Move::Left | Move::Right => Cursor::App(j),
                Move::Up => {
                    if j > 0 {
                        Cursor::App(j - 1)
                    } else if pins > 0 {
                        // the last pinned app (the list has no column to stay under)
                        Cursor::Pin(pins - 1)
                    } else {
                        Cursor::App(0)
                    }
                }
                Move::Down => Cursor::App((j + 1).min(apps.saturating_sub(1))),
                Move::PageUp => Cursor::App(j.saturating_sub(PAGE)),
                Move::PageDown => Cursor::App((j + PAGE).min(apps.saturating_sub(1))),
            })
        }
    }
}

/// A result list's selection after an up or down arrow, wrapping at both ends like the quick search does (`n` rows).
pub fn step_results(selected: usize, up: bool, n: usize) -> usize {
    if n == 0 {
        0
    } else if up {
        (selected + n - 1) % n
    } else {
        (selected + 1) % n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apps(names: &[&str]) -> Vec<App> {
        names.iter().enumerate().map(|(i, n)| App { name: (*n).into(), id: format!("id{i}") }).collect()
    }

    fn sorted_names(names: &[&str], turkish: bool) -> Vec<String> {
        let mut a = apps(names);
        sort_apps(&mut a, turkish);
        a.into_iter().map(|x| x.name).collect()
    }

    #[test]
    fn turkish_letters_sort_where_the_turkish_alphabet_puts_them() {
        // c < ç < d, g < ğ < h, h < ı < i, o < ö < p, s < ş < t, u < ü < v
        let got = sorted_names(&["Zip", "Şifre", "Sayaç", "Ürün", "Uygulama", "Çalış", "Cep", "Dosya", "Öğren", "Orman", "Ilık", "İzle", "Hesap", "Gül", "Ğ", "Pano"], true);
        assert_eq!(got, ["Cep", "Çalış", "Dosya", "Gül", "Ğ", "Hesap", "Ilık", "İzle", "Orman", "Öğren", "Pano", "Sayaç", "Şifre", "Uygulama", "Ürün", "Zip"]);
    }

    #[test]
    fn dotted_and_dotless_i_are_different_letters_in_turkish() {
        // ı (and the capital I) before i (and the capital İ)
        assert_eq!(compare("Işık", "İzle", true), Ordering::Less);
        assert_eq!(compare("ılık", "ilik", true), Ordering::Less);
        assert_eq!(compare("Isı", "İsim", true), Ordering::Less);
        // ... while in a language without the dotless letter I is just i
        assert_eq!(key("I", false), key("i", false));
        assert_ne!(key("I", true), key("i", true));
        assert_eq!(key("İ", true), key("i", true));
    }

    #[test]
    fn symbols_come_first_then_digits_then_letters_and_case_does_not_matter() {
        let got = sorted_names(&["zeta", "7-Zip", "Alpha", "(Beta)", "alpha2", "3D Builder", "Ábaküs"], true);
        assert_eq!(got, ["(Beta)", "3D Builder", "7-Zip", "Ábaküs", "Alpha", "alpha2", "zeta"]);
        assert_eq!(compare("alpha", "ALPHA", true), "alpha".cmp("ALPHA"), "only a tie-break by the exact text");
    }

    #[test]
    fn without_the_turkish_alphabet_accents_are_ignored_for_the_order() {
        let got = sorted_names(&["Zebra", "Çay", "Cep", "Écran", "Eda", "Öl", "Olu"], false);
        // ç sorts as c, é as e, ö as o: "Çay" is c-a-y, ahead of "Cep" (c-e-p)
        assert_eq!(got, ["Çay", "Cep", "Écran", "Eda", "Öl", "Olu", "Zebra"]);
    }

    #[test]
    fn headers_are_the_first_letter_in_capitals_with_the_turkish_ones_kept_apart() {
        assert_eq!(header_of("çalış", true), "Ç");
        assert_eq!(header_of("ışık", true), "I");
        assert_eq!(header_of("izle", true), "İ");
        assert_eq!(header_of("İzle", true), "İ");
        assert_eq!(header_of("şifre", true), "Ş");
        assert_eq!(header_of("ğ", true), "Ğ");
        assert_eq!(header_of("öğren", true), "Ö");
        assert_eq!(header_of("ürün", true), "Ü");
        assert_eq!(header_of("Chrome", true), "C");
        assert_eq!(header_of("7-Zip", true), "#");
        assert_eq!(header_of("(Beta)", true), "&");
        assert_eq!(header_of("  ", true), "&");
        // in a language without them they fold into the base letter
        assert_eq!(header_of("çay", false), "C");
        assert_eq!(header_of("izle", false), "I");
        assert_eq!(header_of("Éclair", false), "E");
        // another script is its own header
        assert_eq!(header_of("Яндекс", false), "Я");
    }

    #[test]
    fn a_header_comes_before_the_first_app_of_each_letter_and_each_letter_has_one_header() {
        let mut a = apps(&["Ayarlar", "Arşiv", "Çizim", "Cep", "Hesap Makinesi", "Işık", "İzle", "7-Zip"]);
        sort_apps(&mut a, true);
        let r = rows(&a, true);
        let headers: Vec<&str> = r.iter().filter_map(|x| if let Row::Header(h) = x { Some(h.as_str()) } else { None }).collect();
        assert_eq!(headers, ["#", "A", "C", "Ç", "H", "I", "İ"]);
        // every app appears once, in order, and the first row is a header
        let idx: Vec<usize> = r.iter().filter_map(|x| if let Row::App(i) = x { Some(*i) } else { None }).collect();
        assert_eq!(idx, (0..a.len()).collect::<Vec<_>>());
        assert!(matches!(r[0], Row::Header(_)));
        assert!(matches!(r[1], Row::App(0)));
        assert_eq!(row_of_app(&r, 0), Some(1));
        assert_eq!(row_of_app(&r, 99), None);
        // a header never follows a header
        assert!(r.windows(2).all(|w| !(matches!(w[0], Row::Header(_)) && matches!(w[1], Row::Header(_)))));
        assert!(rows(&[], true).is_empty());
    }

    #[test]
    fn three_hundred_apps_sort_into_contiguous_letters() {
        // names built from the whole Turkish alphabet and some symbols; a header must never appear twice
        let letters: Vec<char> = TURKISH.chars().collect();
        let mut names = Vec::new();
        for i in 0..300 {
            let a = letters[(i * 7) % letters.len()];
            let b = letters[(i * 13 + 5) % letters.len()];
            names.push(format!("{}{}{}", a.to_uppercase(), b, i));
        }
        let mut a = apps(&names.iter().map(String::as_str).collect::<Vec<_>>());
        sort_apps(&mut a, true);
        let r = rows(&a, true);
        let mut seen = std::collections::HashSet::new();
        for row in &r {
            if let Row::Header(h) = row {
                assert!(seen.insert(h.clone()), "header {h} twice");
            }
        }
        assert!(a.windows(2).all(|w| compare(&w[0].name, &w[1].name, true) != Ordering::Greater));
    }

    // ---- keyboard

    #[test]
    fn down_from_nothing_lands_on_the_first_pin_or_the_first_app() {
        assert_eq!(step(None, Move::Down, 5, 10), Some(Cursor::Pin(0)));
        assert_eq!(step(None, Move::Down, 0, 10), Some(Cursor::App(0)));
        assert_eq!(step(None, Move::Down, 0, 0), None);
        assert_eq!(step(None, Move::Up, 5, 10), None);
    }

    #[test]
    fn the_grid_moves_by_cells_and_rows_and_falls_through_to_the_list() {
        // 8 pins: a row of six and a row of two
        assert_eq!(step(Some(Cursor::Pin(0)), Move::Right, 8, 10), Some(Cursor::Pin(1)));
        assert_eq!(step(Some(Cursor::Pin(7)), Move::Right, 8, 10), Some(Cursor::Pin(7)), "no wrap");
        assert_eq!(step(Some(Cursor::Pin(0)), Move::Left, 8, 10), Some(Cursor::Pin(0)));
        assert_eq!(step(Some(Cursor::Pin(2)), Move::Down, 8, 10), Some(Cursor::Pin(7)), "the partial row's last cell");
        assert_eq!(step(Some(Cursor::Pin(7)), Move::Up, 8, 10), Some(Cursor::Pin(1)));
        assert_eq!(step(Some(Cursor::Pin(7)), Move::Down, 8, 10), Some(Cursor::App(0)));
        assert_eq!(step(Some(Cursor::Pin(7)), Move::Down, 8, 0), Some(Cursor::Pin(7)), "no list to fall into");
        assert_eq!(step(Some(Cursor::Pin(3)), Move::Up, 8, 10), Some(Cursor::Pin(3)), "top row");
    }

    #[test]
    fn the_list_moves_by_apps_and_pages_and_climbs_back_into_the_grid() {
        assert_eq!(step(Some(Cursor::App(3)), Move::Down, 5, 10), Some(Cursor::App(4)));
        assert_eq!(step(Some(Cursor::App(9)), Move::Down, 5, 10), Some(Cursor::App(9)));
        assert_eq!(step(Some(Cursor::App(0)), Move::Up, 5, 10), Some(Cursor::Pin(4)));
        assert_eq!(step(Some(Cursor::App(0)), Move::Up, 0, 10), Some(Cursor::App(0)));
        assert_eq!(step(Some(Cursor::App(3)), Move::PageDown, 5, 100), Some(Cursor::App(3 + PAGE)));
        assert_eq!(step(Some(Cursor::App(3)), Move::PageUp, 5, 100), Some(Cursor::App(0)));
        assert_eq!(step(Some(Cursor::App(95)), Move::PageDown, 5, 100), Some(Cursor::App(99)));
        // left and right belong to the text field
        assert_eq!(step(Some(Cursor::App(3)), Move::Left, 5, 10), Some(Cursor::App(3)));
    }

    #[test]
    fn a_cursor_beyond_a_list_that_shrank_is_pulled_back() {
        assert_eq!(step(Some(Cursor::App(50)), Move::Up, 0, 10), Some(Cursor::App(8)));
        assert_eq!(step(Some(Cursor::Pin(50)), Move::Left, 3, 10), Some(Cursor::Pin(1)));
    }

    #[test]
    fn results_wrap_at_both_ends() {
        assert_eq!(step_results(0, true, 5), 4);
        assert_eq!(step_results(4, false, 5), 0);
        assert_eq!(step_results(2, false, 5), 3);
        assert_eq!(step_results(0, true, 0), 0);
    }

    #[test]
    fn the_app_nearest_to_a_field_below_is_the_last_app_fully_in_view() {
        // rows: A header, apps 0 and 1, a header, apps 2 and 3 (rows 0..6), each 40 px high, a viewport of 120 px = three rows
        let rows = vec![Row::Header("A".into()), Row::App(0), Row::App(1), Row::Header("B".into()), Row::App(2), Row::App(3)];
        assert_eq!(last_visible_app(&rows, 0.0, 120.0, 40.0), Some(1), "rows 0..3 are in view: the last app among them");
        // scrolled by one row: rows 1..4 are in view, row 3 is a header, so the app above it
        assert_eq!(last_visible_app(&rows, 40.0, 120.0, 40.0), Some(1));
        // scrolled by half a row: row 3 is cut by the bottom edge, the last row fully in view is row 2
        assert_eq!(last_visible_app(&rows, 20.0, 120.0, 40.0), Some(1));
        // scrolled to the end (three rows in view: 3, 4, 5)
        assert_eq!(last_visible_app(&rows, 120.0, 120.0, 40.0), Some(3));
        // a viewport taller than the list shows all of it
        assert_eq!(last_visible_app(&rows, 0.0, 1000.0, 40.0), Some(3));
        // nothing measured, or only a header in view: nothing
        assert_eq!(last_visible_app(&rows, 0.0, 0.0, 40.0), None);
        assert_eq!(last_visible_app(&rows, 0.0, 120.0, 0.0), None);
        assert_eq!(last_visible_app(&rows, 0.0, 39.0, 40.0), None, "not even one whole row");
        assert_eq!(last_visible_app(&rows, 0.0, 40.0, 40.0), None, "only the first header fits");
        assert_eq!(last_visible_app(&[], 0.0, 120.0, 40.0), None);
        assert_eq!(last_visible_app(&rows, f32::NAN, 120.0, 40.0), None);
    }
}
