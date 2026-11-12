//! Noticing that the Recycle Bin filled or emptied, so that its desktop icon changes its picture.
//!
//! Every drive keeps its bin in `X:\$Recycle.Bin\<user's SID>`; a file sent to the bin appears there as an `$I...` / `$R...` pair and
//! an emptied bin has none. A watch on those folders (not on what is inside the entries) tells when the picture may have to change;
//! the answer itself is asked of the shell (`SHQueryRecycleBin`) by whoever is told. READ-ONLY: nothing in the bin is listed, opened,
//! restored or deleted here, and the folders are only watched.
//!
//! A drive whose bin folder of this user does not exist (nothing was ever deleted there) has no watch; one that appears later is not
//! picked up until the next start (the main drive, where the Desktop is, nearly always has it).

use std::path::PathBuf;

use windows::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};
use windows::core::PCWSTR;

/// The drive letters (`'C'`) of the fixed disks, from the bitmask `GetLogicalDrives` returns and a test of each one's type.
pub fn fixed_letters(mask: u32, is_fixed: impl Fn(char) -> bool) -> Vec<char> {
    (0..26u32).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i as u8) as char).filter(|c| is_fixed(*c)).collect()
}

/// The bin folder of `sid` on each drive.
pub fn bin_dirs(letters: &[char], sid: &str) -> Vec<PathBuf> {
    letters.iter().map(|l| PathBuf::from(format!("{l}:\\$Recycle.Bin\\{sid}"))).collect()
}

/// The bin folders of this user on the fixed disks that have one.
pub fn existing_bin_dirs() -> Vec<PathBuf> {
    let Some(sid) = crate::dock::startuser::current_sid() else { return Vec::new() };
    let letters = fixed_letters(unsafe { GetLogicalDrives() }, |c| {
        let root: Vec<u16> = format!("{c}:\\").encode_utf16().chain(std::iter::once(0)).collect();
        unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == 3 }
    });
    bin_dirs(&letters, &sid).into_iter().filter(|d| d.is_dir()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_come_from_the_mask_and_only_fixed_ones_stay() {
        // C, D and F present; D is a USB stick
        let mask = (1 << 2) | (1 << 3) | (1 << 5);
        assert_eq!(fixed_letters(mask, |c| c != 'D'), vec!['C', 'F']);
        assert!(fixed_letters(0, |_| true).is_empty());
        assert_eq!(fixed_letters(1 << 25, |_| true), vec!['Z']);
    }

    #[test]
    fn the_bin_folder_is_the_users_own_on_each_drive() {
        let d = bin_dirs(&['C', 'E'], "S-1-5-21-1-2-3-1001");
        assert_eq!(d, vec![PathBuf::from(r"C:\$Recycle.Bin\S-1-5-21-1-2-3-1001"), PathBuf::from(r"E:\$Recycle.Bin\S-1-5-21-1-2-3-1001")]);
        assert!(bin_dirs(&[], "S-1").is_empty());
    }

    #[test]
    fn the_real_lookup_only_lists_folders_and_never_fails() {
        // (read-only: a count at most)
        let dirs = existing_bin_dirs();
        assert!(dirs.iter().all(|d| d.is_dir()));
    }
}
