//! Type-to-select for the desktop. Pure; the clock is a parameter.
//!
//! The same rules as the file list (`folder_view::on_typeahead`): typing keeps extending one text for 900 ms after the last key; the
//! first item whose folded name starts with it is selected, searching from the cursor; the same letter typed again and again cycles
//! through the items that start with it. The items are taken in column-major order (down the first column, then the next), the order
//! the desktop reads in. Names are folded with `rf_core::entry::fold_case` so that `I`, `i`, `İ` and `ı` meet.

use rf_core::desktop::items::ItemId;
use rf_core::entry::fold_case;

pub const WINDOW_MS: u64 = 900;

#[derive(Debug, Clone, Default)]
pub struct TypeSelect {
    text: String,
    last_ms: Option<u64>,
}

impl TypeSelect {
    /// A typed character (`ch` is the key's text). `items`: id and shown name in reading order; `cursor`: the item the keyboard is on.
    /// Returns the item to select, or `None` when nothing matches (the typed text is still kept, as it is in the file list).
    pub fn key(&mut self, ch: &str, now_ms: u64, items: &[(ItemId, String)], cursor: Option<&ItemId>) -> Option<ItemId> {
        if ch.is_empty() || ch.chars().any(char::is_control) || items.is_empty() {
            return None;
        }
        let fresh = self.last_ms.is_none_or(|t| now_ms.saturating_sub(t) > WINDOW_MS);
        if fresh {
            self.text.clear();
        }
        self.text.push_str(&fold_case(ch));
        self.last_ms = Some(now_ms);
        let first = self.text.chars().next()?;
        // one letter typed again and again cycles
        let single_repeat = self.text.chars().count() > 1 && self.text.chars().all(|c| c == first);
        let needle = if single_repeat { first.to_string() } else { self.text.clone() };
        let at = cursor.and_then(|c| items.iter().position(|(id, _)| id == c));
        let n = items.len();
        let start = match at {
            Some(c) if fresh || single_repeat => c + 1,
            Some(c) => c,
            None => 0,
        };
        (0..n).map(|k| &items[(start + k) % n]).find(|(_, name)| fold_case(name).starts_with(&needle)).map(|(id, _)| id.clone())
    }

    /// Typing is in progress (the Space key then belongs to the name, not to the previewer).
    pub fn typing(&self, now_ms: u64) -> bool {
        !self.text.is_empty() && self.last_ms.is_some_and(|t| now_ms.saturating_sub(t) <= WINDOW_MS)
    }

    pub fn reset(&mut self) {
        self.text.clear();
        self.last_ms = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(names: &[&str]) -> Vec<(ItemId, String)> {
        names.iter().map(|n| (ItemId::user(n), n.to_string())).collect()
    }

    #[test]
    fn typing_finds_the_first_name_that_starts_with_the_text_and_keeps_extending() {
        let it = items(&["Alpha", "Beta", "Bravo", "Brave new.txt"]);
        let mut t = TypeSelect::default();
        assert_eq!(t.key("b", 0, &it, None), Some(ItemId::user("Beta")));
        // "br" within the window: from the cursor (on Beta) the first that starts with "br"
        assert_eq!(t.key("r", 300, &it, Some(&ItemId::user("Beta"))), Some(ItemId::user("Bravo")));
        assert_eq!(t.key("a", 500, &it, Some(&ItemId::user("Bravo"))), Some(ItemId::user("Bravo")), "\"bra\" still matches the cursor item");
        assert_eq!(t.key("v", 600, &it, Some(&ItemId::user("Bravo"))), Some(ItemId::user("Bravo")));
        assert_eq!(t.key("e", 700, &it, Some(&ItemId::user("Bravo"))), Some(ItemId::user("Brave new.txt")));
        // nothing matches
        assert_eq!(t.key("z", 800, &it, Some(&ItemId::user("Brave new.txt"))), None);
    }

    #[test]
    fn a_pause_starts_a_new_text_and_the_search_begins_after_the_cursor() {
        let it = items(&["apple", "avocado", "banana"]);
        let mut t = TypeSelect::default();
        assert_eq!(t.key("a", 0, &it, Some(&ItemId::user("apple"))), Some(ItemId::user("avocado")), "fresh: after the cursor");
        // 1.5 s later: new text, after the cursor again (which wraps to apple)
        assert_eq!(t.key("a", 1500, &it, Some(&ItemId::user("avocado"))), Some(ItemId::user("apple")));
        assert!(t.typing(1600));
        assert!(!t.typing(1500 + WINDOW_MS + 1));
    }

    #[test]
    fn one_letter_repeated_cycles_through_the_items_that_start_with_it() {
        let it = items(&["car", "cat", "cow", "dog"]);
        let mut t = TypeSelect::default();
        let mut cur: Option<ItemId> = None;
        let mut seen = Vec::new();
        for i in 0..4 {
            cur = t.key("c", i * 100, &it, cur.as_ref());
            seen.push(cur.clone().unwrap().as_str().to_string());
        }
        assert_eq!(seen, ["u:car", "u:cat", "u:cow", "u:car"]);
    }

    #[test]
    fn turkish_dotted_and_dotless_i_meet_and_control_characters_are_ignored() {
        let it = items(&["Işık", "ilke", "İnek"]);
        let mut t = TypeSelect::default();
        assert_eq!(t.key("i", 0, &it, None), Some(ItemId::user("Işık")), "ı and i fold together");
        t.reset();
        assert_eq!(t.key("\u{8}", 10, &it, None), None);
        assert_eq!(t.key("", 20, &it, None), None);
        assert_eq!(t.key("x", 30, &[], None), None);
        assert!(!t.typing(31), "nothing was typed");
    }
}
