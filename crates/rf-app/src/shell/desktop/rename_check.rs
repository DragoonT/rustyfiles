//! The check before an in-place rename is carried out. Pure.
//!
//! The rules are the file list's (`ops::commit_rename`): Explorer's own (no `\ / : * ? " < > |`, no name ending in a dot), plus the
//! name rules the bulk rename uses (`rf_core::rename::validate`: reserved names such as `CON`, a name that ends in a space, over 255
//! characters). Two more are the file list's too: an unchanged name does nothing, and a name that is another item's is refused
//! instead of being left to the shell's "the parameter is incorrect"; a change of case alone is fine.
//!
//! EXTENSIONS. When the settings hide extensions the box holds the name without its extension and the extension is put back on what
//! was typed; the part of the text that starts selected is the name without its extension (a whole folder name).

use rf_core::rename::{RenameError, split_ext, validate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    Empty,
    /// `\ / : * ? " < > |`, a control character, or a name that ends in a dot or a space.
    Characters,
    Reserved,
    TooLong,
    /// Another item of the desktop already has this name.
    Taken,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    /// The same name (or the box was emptied): nothing to do, no complaint.
    Unchanged,
    /// Rename to this full name.
    Rename(String),
    Refuse(Problem),
}

/// What the box starts with, and how many bytes of it start selected.
pub fn edit_text(name: &str, is_dir: bool, extensions_shown: bool) -> (String, usize) {
    let (stem, ext) = split_ext(name, is_dir);
    if extensions_shown || ext.is_empty() {
        (name.to_string(), stem.len())
    } else {
        (stem.to_string(), stem.len())
    }
}

/// The extension that is hidden from the box (with its dot), if any.
pub fn hidden_extension(name: &str, is_dir: bool, extensions_shown: bool) -> Option<String> {
    let (_, ext) = split_ext(name, is_dir);
    (!extensions_shown && !ext.is_empty()).then(|| ext.to_string())
}

/// `typed` is what is in the box; `siblings` are the names of the other items in the same folder.
pub fn check(old: &str, typed: &str, hidden_ext: Option<&str>, siblings: &[&str]) -> Check {
    let typed = typed.trim();
    if typed.is_empty() {
        return Check::Unchanged;
    }
    let new_name = match hidden_ext {
        Some(ext) => format!("{typed}{ext}"),
        None => typed.to_string(),
    };
    if new_name == old {
        return Check::Unchanged;
    }
    if let Err(e) = validate(&new_name) {
        return Check::Refuse(match e {
            RenameError::Empty => Problem::Empty,
            RenameError::Reserved => Problem::Reserved,
            RenameError::TooLong => Problem::TooLong,
            _ => Problem::Characters,
        });
    }
    // a name ending in a dot is refused by `validate` too, but a name that only differs in case from the old one is the user's right
    let case_only = new_name.to_lowercase() == old.to_lowercase();
    if !case_only && siblings.iter().any(|s| s.to_lowercase() == new_name.to_lowercase()) {
        return Check::Refuse(Problem::Taken);
    }
    Check::Rename(new_name)
}

/// Whether a change of extension is being made (the file list asks "are you sure" per its setting).
pub fn changes_extension(old: &str, new: &str, is_dir: bool) -> bool {
    if is_dir {
        return false;
    }
    let ext = |n: &str| split_ext(n, false).1.to_lowercase();
    !ext(old).is_empty() && ext(old) != ext(new)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_box_starts_with() {
        assert_eq!(edit_text("report.docx", false, true), ("report.docx".into(), 6), "extension shown: the stem is selected");
        assert_eq!(edit_text("report.docx", false, false), ("report".into(), 6), "extension hidden: only the stem is in the box");
        assert_eq!(edit_text("Photos", true, true), ("Photos".into(), 6), "a folder is selected whole");
        assert_eq!(edit_text("v1.2", true, true), ("v1.2".into(), 4), "a folder's dot is not an extension");
        assert_eq!(edit_text(".gitignore", false, false), (".gitignore".into(), 10));
        assert_eq!(hidden_extension("a.txt", false, false).as_deref(), Some(".txt"));
        assert_eq!(hidden_extension("a.txt", false, true), None);
        assert_eq!(hidden_extension("dir.d", true, false), None);
    }

    #[test]
    fn an_unchanged_or_emptied_name_does_nothing() {
        assert_eq!(check("a.txt", "a.txt", None, &[]), Check::Unchanged);
        assert_eq!(check("a.txt", "  ", None, &[]), Check::Unchanged);
        assert_eq!(check("a.txt", "a", Some(".txt"), &[]), Check::Unchanged, "the hidden extension is put back before comparing");
    }

    #[test]
    fn a_good_new_name_is_renamed_with_its_hidden_extension() {
        assert_eq!(check("a.txt", "b", Some(".txt"), &["c.txt"]), Check::Rename("b.txt".into()));
        assert_eq!(check("a.txt", "  b.md  ", None, &[]), Check::Rename("b.md".into()));
        assert_eq!(check("Dir", "Dir 2", None, &["other"]), Check::Rename("Dir 2".into()));
    }

    #[test]
    fn explorers_rules_are_kept() {
        for bad in ["a/b", "a\\b", "a:b", "a*b", "a?b", "a\"b", "a<b", "a>b", "a|b", "ends.", "tab\tin"] {
            assert_eq!(check("x", bad, None, &[]), Check::Refuse(Problem::Characters), "{bad:?}");
        }
        assert_eq!(check("x", "trailing ", Some(""), &[]), Check::Rename("trailing".into()), "typed text is trimmed first");
        assert_eq!(check("x", "CON", None, &[]), Check::Refuse(Problem::Reserved));
        assert_eq!(check("x", "nul.txt", None, &[]), Check::Refuse(Problem::Reserved));
        assert_eq!(check("x", &"a".repeat(256), None, &[]), Check::Refuse(Problem::TooLong));
    }

    #[test]
    fn another_items_name_is_refused_but_a_change_of_case_is_fine() {
        assert_eq!(check("a.txt", "B.TXT", None, &["b.txt"]), Check::Refuse(Problem::Taken), "names compare without case");
        assert_eq!(check("a.txt", "A.txt", None, &["b.txt"]), Check::Rename("A.txt".into()));
        assert_eq!(check("a.txt", "A.txt", None, &["a.txt"]), Check::Rename("A.txt".into()), "its own entry in the list is no clash");
    }

    #[test]
    fn extension_changes_are_noticed() {
        assert!(changes_extension("a.txt", "a.md", false));
        assert!(changes_extension("a.txt", "a", false));
        assert!(!changes_extension("a.txt", "b.TXT", false));
        assert!(!changes_extension("a", "a.md", false), "a name without an extension has nothing to lose");
        assert!(!changes_extension("a.d", "b", true));
    }
}
