//! What the desktop may do to files, decided from facts only. Pure.
//!
//! Every operation that could change the user's Desktop asks here first, so that the rules are written down once and tested as a
//! table, and a new way into the files (a drop on a shortcut, a dialog) has to go through the same door:
//!
//! * the READ-ONLY preview (the real Desktop without `--writable`, and any request for a scratch folder that could not be used) never
//!   changes, creates, moves in or deletes a file. Moving an icon to another cell is not here: that changes our own layout file only;
//! * the Public Desktop and the virtual items (This PC, the Recycle Bin ...) are never changed or written into from here, in any mode
//!   (changing the Public folder asks for administrator rights, which this program never triggers);
//! * files that come from the Public Desktop are copied, never moved out of it.

use std::path::Path;

use rf_core::desktop::items::Origin;
use rf_win::fileops::Transfer;

/// Why something was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    ReadOnly,
    /// A Public Desktop item, a virtual item, or a target in the Public Desktop.
    NotYours,
    /// A scratch (test) desktop shows the user's REAL Recycle Bin; emptying it from there is not done.
    RealBin,
}

/// The items themselves change (cut, delete, rename, their properties): every one of them must be the user's own file.
pub fn change_items(read_only: bool, origins: &[Origin]) -> Result<(), Refusal> {
    if read_only {
        return Err(Refusal::ReadOnly);
    }
    if origins.iter().any(|o| *o != Origin::User) {
        return Err(Refusal::NotYours);
    }
    Ok(())
}

/// Emptying the Recycle Bin: not in the protected preview, and not from a scratch desktop either. A scratch desktop is a place to try
/// things on files of its own, but its Recycle Bin icon is the user's real bin, and emptying it would destroy what the user has kept
/// in it. (What the user sends to the bin from a scratch desktop is only that desktop's own files.)
pub fn empty_bin(read_only: bool, scratch: bool) -> Result<(), Refusal> {
    if read_only {
        Err(Refusal::ReadOnly)
    } else if scratch {
        Err(Refusal::RealBin)
    } else {
        Ok(())
    }
}

/// Something is put into the Desktop folder (paste, new, files dropped on an empty place).
pub fn add_to_desktop(read_only: bool) -> Result<(), Refusal> {
    if read_only { Err(Refusal::ReadOnly) } else { Ok(()) }
}

/// Files are handed to a folder (an item of the desktop, or the folder a shortcut points at): `sources_public` when any of them
/// comes from the Public Desktop, `dest_public` when the folder is in it. `default` is the usual copy-or-move rule.
pub fn into_folder(read_only: bool, sources_public: bool, dest_public: bool, default: Transfer) -> Result<Transfer, Refusal> {
    add_to_desktop(read_only)?;
    if dest_public {
        return Err(Refusal::NotYours);
    }
    Ok(if sources_public { Transfer::Copy } else { default })
}

/// `path` is `dir` or lies below it. Compared by components and without regard to case: `C:\Users\Public\Desktop2` is not below
/// `C:\Users\Public\Desktop`.
pub fn is_under(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().to_lowercase().replace('/', "\\").trim_start_matches(r"\\?\").trim_end_matches('\\').to_string();
    let (p, d) = (norm(path), norm(dir));
    !d.is_empty() && (p == d || p.starts_with(&format!("{d}\\")))
}

/// More than this many items opened at once is asked about first (Explorer asks above 15).
pub const CONFIRM_OPEN_ABOVE: usize = 15;
/// At most this many folders are opened in windows by one Enter.
pub const MAX_FOLDER_WINDOWS: usize = 8;

/// What Enter does with a selection of `files` files (or programs) and `dirs` folders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenPlan {
    /// Ask before opening anything.
    pub confirm: bool,
    /// Folders that are not opened because of the cap (the user is told).
    pub dirs_skipped: usize,
}

pub fn open_plan(files: usize, dirs: usize) -> OpenPlan {
    OpenPlan { confirm: files + dirs > CONFIRM_OPEN_ABOVE, dirs_skipped: dirs.saturating_sub(MAX_FOLDER_WINDOWS) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::virtuals::VirtualKind;

    const VIRT: Origin = Origin::Virtual(VirtualKind::ThisPc);

    #[test]
    fn changing_items_needs_a_writable_desktop_and_the_users_own_files() {
        assert_eq!(change_items(false, &[Origin::User, Origin::User]), Ok(()));
        assert_eq!(change_items(true, &[Origin::User]), Err(Refusal::ReadOnly));
        assert_eq!(change_items(false, &[Origin::User, Origin::Public]), Err(Refusal::NotYours));
        assert_eq!(change_items(false, &[VIRT]), Err(Refusal::NotYours));
        // read-only wins when both apply: that is what the status line says
        assert_eq!(change_items(true, &[Origin::Public]), Err(Refusal::ReadOnly));
        // properties (rename, attributes) are the same rule
        assert_eq!(change_items(true, &[Origin::User]), Err(Refusal::ReadOnly), "the Properties window is locked in the read-only preview");
    }

    #[test]
    fn the_bin_is_emptied_only_from_the_real_writable_desktop() {
        assert_eq!(empty_bin(false, false), Ok(()));
        assert_eq!(empty_bin(true, false), Err(Refusal::ReadOnly));
        assert_eq!(empty_bin(false, true), Err(Refusal::RealBin), "a scratch desktop's bin icon is the real bin");
        assert_eq!(empty_bin(true, true), Err(Refusal::ReadOnly), "read-only is what the status line says first");
    }

    #[test]
    fn adding_to_the_desktop_needs_a_writable_one() {
        assert_eq!(add_to_desktop(false), Ok(()));
        assert_eq!(add_to_desktop(true), Err(Refusal::ReadOnly));
    }

    #[test]
    fn a_folder_takes_files_only_when_writable_and_never_in_the_public_desktop() {
        for default in [Transfer::Copy, Transfer::Move] {
            assert_eq!(into_folder(false, false, false, default), Ok(default));
            assert_eq!(into_folder(true, false, false, default), Err(Refusal::ReadOnly));
            assert_eq!(into_folder(false, false, true, default), Err(Refusal::NotYours), "a folder of the Public Desktop is not written into");
        }
        // files from the Public Desktop are copied even when the usual rule says move
        assert_eq!(into_folder(false, true, false, Transfer::Move), Ok(Transfer::Copy));
        assert_eq!(into_folder(true, true, false, Transfer::Move), Err(Refusal::ReadOnly));
    }

    /// The route that was open before: a shortcut that points at a folder is such a folder, so it follows the same function. The
    /// read-only preview must never turn a drop on a shortcut into a move.
    #[test]
    fn a_shortcut_to_a_folder_cannot_move_desktop_files_in_the_read_only_preview() {
        for default in [Transfer::Copy, Transfer::Move] {
            assert_eq!(into_folder(true, false, false, default), Err(Refusal::ReadOnly));
        }
    }

    #[test]
    fn under_is_by_components_and_ignores_case_and_slash_direction() {
        let public = Path::new(r"C:\Users\Public\Desktop");
        assert!(is_under(Path::new(r"C:\Users\Public\Desktop\a.txt"), public));
        assert!(is_under(Path::new(r"c:/users/public/desktop/Sub/a.txt"), public));
        assert!(is_under(Path::new(r"C:\Users\Public\Desktop\"), public));
        assert!(is_under(public, public));
        assert!(!is_under(Path::new(r"C:\Users\Public\Desktop2\a.txt"), public), "a longer name is another folder");
        assert!(!is_under(Path::new(r"C:\Users\Public\DesktopX"), public));
        assert!(!is_under(Path::new(r"C:\Users\Public"), public));
        assert!(!is_under(Path::new(r"C:\anything"), Path::new("")), "no folder, nothing is below it");
        assert!(is_under(Path::new(r"\\?\C:\Users\Public\Desktop\x"), public));
    }

    #[test]
    fn many_items_are_asked_about_and_many_folders_are_capped() {
        assert_eq!(open_plan(1, 0), OpenPlan { confirm: false, dirs_skipped: 0 });
        assert_eq!(open_plan(10, 5), OpenPlan { confirm: false, dirs_skipped: 0 }, "15 is still fine");
        assert_eq!(open_plan(10, 6), OpenPlan { confirm: true, dirs_skipped: 0 });
        assert_eq!(open_plan(100, 0), OpenPlan { confirm: true, dirs_skipped: 0 });
        assert_eq!(open_plan(0, 8), OpenPlan { confirm: false, dirs_skipped: 0 });
        assert_eq!(open_plan(0, 12), OpenPlan { confirm: false, dirs_skipped: 4 });
        assert_eq!(open_plan(20, 30), OpenPlan { confirm: true, dirs_skipped: 22 });
    }
}
