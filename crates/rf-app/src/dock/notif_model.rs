//! What the bell's panel says and measures, as pure functions (`notif_view.rs` puts them on the screen): the panel's height for what
//! it shows, the words for the age of a notification and for a missing access, and how the keyboard cursor walks along the list.
//! The data side (grouping, order, the three-line clamp, the age, the difference between two polls) is `rf_win::toasts`.
//!
//! The words are passed a translator (`Words`), as in `cc_model.rs`, so a test can ask for both languages; every Turkish text used
//! here has its English pair in `i18n_en` (a test checks that).

use rf_win::toasts::{Access, Ago, Group, Toast};

use super::cc_model::Words;

// ------------------------------------------------------------------ measures (logical px)

pub const WIDTH: f32 = 380.0;
/// The space between the window's edge and the content.
pub const PAD: f32 = 12.0;
pub const HEADER_H: f32 = 44.0;
/// A program's heading in the list.
pub const GROUP_H: f32 = 32.0;
/// What a notification is estimated to be tall (a title and up to three lines of text); the list scrolls when the estimate is passed.
pub const ITEM_H: f32 = 84.0;
const MIN_H: f32 = 190.0;
const MAX_H: f32 = 600.0;

/// What the panel shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content {
    /// The list has not been read yet (its size is guessed from the last count).
    Loading { items: usize },
    /// Access is not granted: one line and a button.
    NoAccess,
    /// "Bildirim yok".
    Empty,
    List { groups: usize, items: usize },
}

/// The window's height for what it shows: the header, the content as estimated, the padding; held between a small and a large size
/// (the list scrolls inside). The panel is placed with this, 8 px above its island, and grows upwards.
pub fn height(c: Content) -> f32 {
    let body = match c {
        Content::Loading { items: 0 } | Content::Empty => 96.0,
        Content::NoAccess => 120.0,
        // (a guess of two notifications to a program, until the list has come)
        Content::Loading { items } => (items.div_ceil(2)) as f32 * GROUP_H + items as f32 * ITEM_H,
        Content::List { groups, items } => groups as f32 * GROUP_H + items as f32 * ITEM_H,
    };
    (HEADER_H + body + 2.0 * PAD).clamp(MIN_H, MAX_H)
}

// ------------------------------------------------------------------ words

/// The age as the right end of a notification says it: "şimdi", "5 dk", "2 sa", "dün", or the date `date` (already written in the
/// user's own format).
pub fn time_text(ago: Ago, date: &str, w: Words) -> String {
    let tr = w.tr;
    match ago {
        Ago::Now => tr("şimdi").to_string(),
        Ago::Minutes(n) => format!("{n} {}", tr("dk")),
        Ago::Hours(n) => format!("{n} {}", tr("sa")),
        Ago::Yesterday => tr("dün").to_string(),
        Ago::Date => date.to_string(),
    }
}

/// The one line the panel says when the list cannot be read, with the button under it.
pub fn access_line(a: Access, w: Words) -> &'static str {
    let tr = w.tr;
    match a {
        Access::Unavailable => tr("Bildirimler okunamadı."),
        _ => tr("Bildirimlere erişim kapalı. Windows'ta izin verin."),
    }
}

/// A program's heading when Windows would not say its name.
pub fn unknown_app(w: Words) -> &'static str {
    (w.tr)("Bilinmeyen uygulama")
}

// ------------------------------------------------------------------ the keyboard

/// The cursor after an arrow key. No cursor yet: Down takes the first, Up the last. The ends hold (no wrapping: the list is short
/// and a key that goes round loses the user's place).
pub fn step_cursor(order: &[u32], cursor: Option<u32>, down: bool) -> Option<u32> {
    if order.is_empty() {
        return None;
    }
    let at = cursor.and_then(|c| order.iter().position(|&i| i == c));
    let next = match (at, down) {
        (None, true) => 0,
        (None, false) => order.len() - 1,
        (Some(i), true) => (i + 1).min(order.len() - 1),
        (Some(i), false) => i.saturating_sub(1),
    };
    Some(order[next])
}

/// The position of the notification `id` among the children of the list's column (each program's heading is a child, then its
/// notifications), for scrolling the cursor into view.
pub fn child_index(groups: &[Group], toasts: &[Toast], id: u32) -> Option<usize> {
    let mut at = 0;
    for g in groups {
        at += 1;
        for &i in &g.items {
            if toasts[i].id == id {
                return Some(at);
            }
            at += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::toasts::group;

    fn tr_tr(s: &'static str) -> &'static str {
        s
    }

    fn tr_en(s: &'static str) -> &'static str {
        crate::i18n_en::EN.iter().find(|(k, _)| *k == s).map(|(_, v)| *v).unwrap_or_else(|| panic!("no English pair for {s:?}"))
    }

    const TR: Words = Words { tr: tr_tr, pct: |p| format!("%{p}") };
    const EN: Words = Words { tr: tr_en, pct: |p| format!("{p}%") };

    fn toast(id: u32, app_id: &str, created: i64) -> Toast {
        Toast { id, app_id: app_id.into(), app: app_id.into(), title: String::new(), body: String::new(), created }
    }

    #[test]
    fn the_age_is_said_in_both_languages() {
        let say = |a, w| time_text(a, "12.11.2026", w);
        assert_eq!(say(Ago::Now, TR), "şimdi");
        assert_eq!(say(Ago::Now, EN), "now");
        assert_eq!(say(Ago::Minutes(5), TR), "5 dk");
        assert_eq!(say(Ago::Minutes(5), EN), "5 min");
        assert_eq!(say(Ago::Hours(2), TR), "2 sa");
        assert_eq!(say(Ago::Hours(2), EN), "2 h");
        assert_eq!(say(Ago::Yesterday, TR), "dün");
        assert_eq!(say(Ago::Yesterday, EN), "yesterday");
        assert_eq!(say(Ago::Date, TR), "12.11.2026");
        assert_eq!(say(Ago::Date, EN), "12.11.2026", "the date is already in the user's own format");
    }

    #[test]
    fn the_missing_access_is_one_line_in_both_languages() {
        for a in [Access::Denied, Access::NotAsked, Access::Unavailable] {
            for w in [TR, EN] {
                let line = access_line(a, w);
                assert!(!line.is_empty() && !line.contains('\n'), "{a:?}: {line:?}");
            }
        }
        assert_ne!(access_line(Access::Denied, TR), access_line(Access::Unavailable, TR));
        assert_eq!(access_line(Access::Denied, EN), "Notification access is off. Allow it in Windows.");
        assert_eq!(unknown_app(EN), "Unknown app");
        // the other words of the panel
        for k in ["Bildirimler", "Bildirim yok", "Tümünü temizle", "Bu bildirimi kaldır", "Bildirim ayarlarını aç"] {
            assert!(!tr_en(k).is_empty());
        }
    }

    #[test]
    fn the_panels_height_follows_what_it_shows_and_stays_between_its_limits() {
        let h = |c| height(c);
        assert_eq!(h(Content::Empty), h(Content::Loading { items: 0 }));
        assert!(h(Content::Empty) >= MIN_H);
        assert!(h(Content::NoAccess) >= h(Content::Empty));
        assert!(h(Content::List { groups: 1, items: 1 }) > h(Content::Empty) - 1.0);
        assert!(h(Content::List { groups: 2, items: 3 }) > h(Content::List { groups: 1, items: 1 }));
        assert_eq!(h(Content::List { groups: 30, items: 90 }), MAX_H, "a long list scrolls inside the largest panel");
        // the guess before the first reading is a list of that many
        assert_eq!(h(Content::Loading { items: 4 }), h(Content::List { groups: 2, items: 4 }));
        assert!(h(Content::List { groups: 3, items: 6 }) <= MAX_H);
    }

    #[test]
    fn the_arrows_walk_the_list_and_stop_at_its_ends() {
        let o = [5, 6, 7];
        assert_eq!(step_cursor(&o, None, true), Some(5));
        assert_eq!(step_cursor(&o, None, false), Some(7));
        assert_eq!(step_cursor(&o, Some(5), true), Some(6));
        assert_eq!(step_cursor(&o, Some(7), true), Some(7), "no wrapping");
        assert_eq!(step_cursor(&o, Some(5), false), Some(5));
        assert_eq!(step_cursor(&o, Some(7), false), Some(6));
        assert_eq!(step_cursor(&o, Some(99), true), Some(5), "a cursor that is gone starts over");
        assert_eq!(step_cursor(&[], Some(1), true), None);
    }

    #[test]
    fn the_scroll_target_counts_the_headings() {
        let list = vec![toast(1, "a", 100), toast(2, "b", 300), toast(3, "a", 200)];
        let g = group(&list);
        // b (300): heading, id 2. a: heading, id 3 (200), id 1 (100)
        assert_eq!(child_index(&g, &list, 2), Some(1));
        assert_eq!(child_index(&g, &list, 3), Some(3));
        assert_eq!(child_index(&g, &list, 1), Some(4));
        assert_eq!(child_index(&g, &list, 9), None);
    }
}
