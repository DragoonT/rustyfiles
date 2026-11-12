//! The name under an icon. Pure.
//!
//! The settings say whether extensions are shown; Explorer never shows the extension of a shortcut (`.lnk`, `.url`, `.pif`,
//! `.appref-ms`), whatever the setting says, and neither do we. A virtual item has the shell's localised name when the app has
//! asked for it and the English fallback of `VirtualKind::english_name` until then.

use rf_core::rename::split_ext;

/// Extensions the shell hides always (it marks them `NeverShowExt`).
const NEVER_SHOWN: [&str; 4] = [".lnk", ".url", ".pif", ".appref-ms"];

pub fn display_name(name: &str, is_dir: bool, show_extensions: bool) -> String {
    if is_dir {
        return name.to_string();
    }
    let (stem, ext) = split_ext(name, false);
    let always_hidden = NEVER_SHOWN.iter().any(|e| ext.eq_ignore_ascii_case(e));
    if (show_extensions && !always_hidden) || ext.is_empty() || stem.is_empty() { name.to_string() } else { stem.to_string() }
}

/// The part of a name that hides when it is a shortcut: the rename box has to put it back (see `rename_check::hidden_extension`).
pub fn extension_never_shown(name: &str) -> bool {
    let (_, ext) = split_ext(name, false);
    NEVER_SHOWN.iter().any(|e| ext.eq_ignore_ascii_case(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_follow_the_setting_but_shortcuts_never_show_theirs() {
        assert_eq!(display_name("report.docx", false, true), "report.docx");
        assert_eq!(display_name("report.docx", false, false), "report");
        assert_eq!(display_name("Chrome.lnk", false, true), "Chrome");
        assert_eq!(display_name("Site.URL", false, true), "Site");
        assert_eq!(display_name("Chrome.lnk", false, false), "Chrome");
        assert!(extension_never_shown("a.LNK") && !extension_never_shown("a.txt"));
    }

    #[test]
    fn folders_and_odd_names_are_left_alone() {
        assert_eq!(display_name("Photos.2024", true, false), "Photos.2024", "a folder's dot is not an extension");
        assert_eq!(display_name(".gitignore", false, false), ".gitignore", "nothing would be left of it");
        assert_eq!(display_name("README", false, false), "README");
        assert_eq!(display_name("archive.tar.gz", false, false), "archive.tar", "only the last extension goes");
    }
}
