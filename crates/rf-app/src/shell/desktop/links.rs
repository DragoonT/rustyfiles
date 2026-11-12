//! Names of the shortcuts (.lnk) the desktop makes: "Create shortcut" on items and "Paste shortcut" from the clipboard. Pure.
//!
//! The same names the file list uses (`ctx_menu::create_shortcuts`, `ops::paste_shortcuts`), so a shortcut made here and one made
//! there look alike: "Notes.txt - Shortcut.lnk" for Create shortcut, "Notes.txt.lnk" for Paste shortcut; never over an existing file:
//! "... (2).lnk", "... (3).lnk".

/// How the name is made.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    /// "Create shortcut" on an item: the name, then " - " and the word for shortcut in the interface language.
    Created,
    /// "Paste shortcut": the name itself.
    Pasted,
}

/// The name before the numbering and the extension.
pub fn base_name(item_name: &str, style: Style, word_shortcut: &str) -> String {
    match style {
        Style::Created => format!("{item_name} - {word_shortcut}"),
        Style::Pasted => item_name.to_string(),
    }
}

/// `base`.lnk, or the first of `base (2)`.lnk, `base (3)`.lnk ... that `taken` does not know. `taken` is asked with the file name
/// (the caller's folder is its business); the file system does not tell "A.lnk" from "a.lnk", so `taken` should not either.
pub fn unique(base: &str, taken: impl Fn(&str) -> bool) -> String {
    let first = format!("{base}.lnk");
    if !taken(&first) {
        return first;
    }
    (2..10_000).map(|n| format!("{base} ({n}).lnk")).find(|c| !taken(c)).unwrap_or_else(|| format!("{base} (10000).lnk"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_created_shortcut_is_named_after_its_item_and_the_word_for_shortcut() {
        assert_eq!(base_name("Notes.txt", Style::Created, "Kısayol"), "Notes.txt - Kısayol");
        assert_eq!(base_name("Notes.txt", Style::Created, "Shortcut"), "Notes.txt - Shortcut");
        assert_eq!(base_name("Projects", Style::Pasted, "Kısayol"), "Projects", "a pasted shortcut keeps the name");
    }

    #[test]
    fn a_name_in_use_gets_a_number_and_the_numbering_skips_taken_ones() {
        let none = |_: &str| false;
        assert_eq!(unique("A - Shortcut", none), "A - Shortcut.lnk");
        let taken = |n: &str| ["a.lnk", "a (2).lnk", "a (3).lnk"].contains(&n);
        assert_eq!(unique("a", taken), "a (4).lnk");
        let first_only = |n: &str| n == "a.lnk";
        assert_eq!(unique("a", first_only), "a (2).lnk");
    }

    #[test]
    fn the_search_ends_even_when_everything_is_taken() {
        assert_eq!(unique("a", |_| true), "a (10000).lnk");
    }
}
