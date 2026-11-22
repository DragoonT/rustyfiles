use std::path::PathBuf;

use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
use windows::core::PCWSTR;

use crate::wide::{from_wide, wide};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    RamDisk,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct Drive {
    /// "C:\"
    pub root: PathBuf,
    pub letter: char,
    pub label: String,
    pub kind: DriveKind,
    pub total: u64,
    pub free: u64,
    pub fs: String,
}

impl Drive {
    pub fn display_name(&self) -> String {
        let label = if self.label.is_empty() {
            match self.kind {
                DriveKind::Removable => "USB Sürücü",
                DriveKind::Network => "Ağ Sürücüsü",
                DriveKind::Optical => "DVD Sürücü",
                _ => "Yerel Disk",
            }
            .to_string()
        } else {
            self.label.clone()
        };
        format!("{label} ({}:)", self.letter)
    }
    pub fn used_fraction(&self) -> f32 {
        if self.total == 0 { 0.0 } else { 1.0 - self.free as f32 / self.total as f32 }
    }
}

// ---------------------------------------------------------------- drives that are not listed

/// The bit of a drive letter in a drive mask ('A' is bit 0), in either case; `None` for what is no drive letter.
pub fn letter_bit(letter: char) -> Option<u32> {
    let l = letter.to_ascii_uppercase();
    l.is_ascii_uppercase().then(|| 1u32 << (l as u8 - b'A'))
}

/// The mask of the letters as the settings keep them or a user writes them: "V", "v:", "V:\\". What is no drive letter is skipped.
pub fn mask_of_letters<S: AsRef<str>>(letters: &[S]) -> u32 {
    letters.iter().filter_map(|s| s.as_ref().trim().chars().next().and_then(letter_bit)).fold(0, |m, b| m | b)
}

/// The letters of a mask, A to Z.
pub fn letters_of_mask(mask: u32) -> Vec<char> {
    (0..26u8).filter(|i| mask & (1 << i) != 0).map(|i| (b'A' + i) as char).collect()
}

/// The drives the user hides in rustyfiles (Settings > Files and folders), as a mask. One value for the process, set from the
/// settings whenever they are applied.
static HIDDEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

pub fn set_hidden(mask: u32) {
    HIDDEN.store(mask, std::sync::atomic::Ordering::Relaxed);
}

pub fn hidden() -> u32 {
    HIDDEN.load(std::sync::atomic::Ordering::Relaxed)
}

/// The drives Explorer itself is told not to show: the `NoDrives` value of Explorer's policies, the user's and the machine's (what
/// "Hide these specified drives in My Computer" and the tweak tools write). A drive hidden there is hidden here too.
pub fn policy_hidden() -> u32 {
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    let read = |root: &windows_registry::Key| root.open(KEY).ok().and_then(|k| k.get_u32("NoDrives").ok()).unwrap_or(0);
    (read(windows_registry::CURRENT_USER) | read(windows_registry::LOCAL_MACHINE)) & 0x03FF_FFFF
}

/// The drives a list of places shows: all of them less the ones the user hides and the ones Explorer hides.
///
/// WHY (issue 23, LokeYourC3PH, with a picture of four drives of which one was a raw encrypted volume: "any encrypted volume which is
/// not the system volume, stills shows up, even though I can hide it in the Windows Explorer ... I would like for an option where we
/// can hide volumes in rustyfiles"). A hidden drive is only not LISTED: its address typed into the address bar opens it as before.
/// It is also not asked for its name and its size, which is where a volume that cannot be read answers slowly or with an error.
pub fn visible_drives() -> Vec<Drive> {
    list_drives_except(hidden() | policy_hidden())
}

/// Lists drive letters. Volume info can stall on disconnected network / empty optical drives,
/// so call this off the UI thread.
pub fn list_drives() -> Vec<Drive> {
    list_drives_except(0)
}

fn list_drives_except(skip: u32) -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() } & !skip;
    let letters: Vec<(char, DriveKind)> = (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let letter = (b'A' + i as u8) as char;
            (letter, kind_of(letter))
        })
        .collect();
    // Each drive is asked for its name and size on a thread of its own, all at once. A network drive whose server cannot be reached
    // answers after seconds, and they were asked one after the other while the sidebar waited for the list (issue 194: three
    // unavailable network drives, "it takes around 10 seconds before I can use it"). A network drive that has not answered by
    // NETWORK_WAIT is listed without its name and size; the other kinds are waited for as before.
    let asks: Vec<_> = letters
        .iter()
        .map(|&(letter, kind)| {
            let (tx, rx) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("drive info".into()).spawn(move || {
                let _ = tx.send(volume_of(letter, kind));
            });
            rx
        })
        .collect();
    let deadline = std::time::Instant::now() + NETWORK_WAIT;
    letters
        .into_iter()
        .zip(asks)
        .map(|((letter, kind), rx)| {
            let got = if kind == DriveKind::Network { rx.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now())).ok() } else { rx.recv().ok() };
            got.unwrap_or_else(|| {
                if kind == DriveKind::Network {
                    log::info!("drives: {letter}: did not answer in time (a network drive whose server is away?), listed without its name");
                }
                Drive { root: PathBuf::from(format!("{letter}:\\")), letter, label: String::new(), kind, total: 0, free: 0, fs: String::new() }
            })
        })
        .collect()
}

/// How long the list waits for a network drive's name and size (`list_drives_except`).
const NETWORK_WAIT: std::time::Duration = std::time::Duration::from_millis(400);

fn kind_of(letter: char) -> DriveKind {
    let w = wide(&format!("{letter}:\\"));
    match unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } {
        2 => DriveKind::Removable,
        3 => DriveKind::Fixed,
        4 => DriveKind::Network,
        5 => DriveKind::Optical,
        6 => DriveKind::RamDisk,
        _ => DriveKind::Unknown,
    }
}

/// A drive with its name, file system and sizes (none for an empty DVD tray or a volume that cannot be read).
fn volume_of(letter: char, kind: DriveKind) -> Drive {
    let root = format!("{letter}:\\");
    let w = wide(&root);
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let has_vol = unsafe { GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs)) }.is_ok();
    let (mut free, mut total) = (0u64, 0u64);
    if has_vol {
        unsafe {
            let _ = GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None);
        }
    }
    Drive { root: PathBuf::from(root), letter, label: from_wide(&label), kind, total, free, fs: from_wide(&fs) }
}

#[cfg(test)]
mod tests {
    use super::{letter_bit, letters_of_mask, list_drives_except, mask_of_letters, policy_hidden};

    /// Network paths are slow without touching the network; a file on the system disk is not.
    #[test]
    fn hidden_drives_are_a_mask_of_their_letters() {
        assert_eq!(letter_bit('A'), Some(1));
        assert_eq!(letter_bit('c'), Some(4));
        assert_eq!(letter_bit('Z'), Some(1 << 25));
        assert_eq!(letter_bit('1'), None);
        assert_eq!(letter_bit('ç'), None);
        assert_eq!(mask_of_letters(&["V", "c:", " Z:\\ ", "", "9", "é"]), (1 << 21) | 4 | (1 << 25));
        assert_eq!(mask_of_letters::<&str>(&[]), 0);
        assert_eq!(letters_of_mask((1 << 21) | 4 | 1), ['A', 'C', 'V']);
        assert!(letters_of_mask(0).is_empty());
        // back and forth
        let letters: Vec<String> = letters_of_mask(0b1010_0100).into_iter().map(String::from).collect();
        assert_eq!(mask_of_letters(&letters), 0b1010_0100);
    }

    /// On the PC the tests run on: a drive that is hidden is not listed and not asked anything, and the others are as they were.
    #[test]
    fn a_hidden_drive_is_left_out_of_the_list() {
        let all: Vec<char> = list_drives_except(0).into_iter().map(|d| d.letter).collect();
        let Some(&first) = all.first() else { return };
        let without: Vec<char> = list_drives_except(letter_bit(first).unwrap()).into_iter().map(|d| d.letter).collect();
        assert_eq!(without, all[1..]);
        assert!(list_drives_except(0x03FF_FFFF).is_empty(), "every letter hidden: nothing is listed");
        // (reading Explorer's policy never fails: no value is no drive)
        assert_eq!(policy_hidden() & !0x03FF_FFFF, 0);
    }

    #[test]
    fn slow_places() {
        use std::path::Path;
        assert!(super::slow_to_read(Path::new(r"\\nas\share\film.mkv")));
        assert!(super::slow_to_read(Path::new(r"\\?\UNC\nas\share\film.mkv")));
        assert!(!super::slow_to_read(&std::env::temp_dir()));
        assert!(!super::slow_to_read(Path::new(r"C:\Windows\notepad.exe")));
    }

    #[test]
    fn has_c_drive() {
        let d = super::list_drives();
        let c = d.iter().find(|d| d.letter == 'C').expect("C: exists");
        assert!(c.total > 0 && c.free <= c.total);
    }
}

/// Whether `path` is on a removable drive (USB stick, SD card).
pub fn is_removable(path: &std::path::Path) -> bool {
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    if b.len() < 2 || b[1] != b':' {
        return false;
    }
    let root = crate::wide::wide(format!("{}:\\", b[0] as char));
    // 2 = DRIVE_REMOVABLE
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == 2 }
}

/// Whether reading `path` may take seconds: a network share, a removable or optical disk, or a
/// cloud file whose data is not on this PC (reading it downloads it). DirectLook opens such files
/// off the UI thread.
pub fn slow_to_read(path: &std::path::Path) -> bool {
    if crate::localized::is_network(path) {
        return true;
    }
    let s = path.to_string_lossy();
    let letter = match s.as_bytes() {
        [l, b':', ..] => Some(*l),
        [b'\\', b'\\', b'?', b'\\', l, b':', ..] => Some(*l),
        _ => None,
    };
    if let Some(l) = letter {
        let root = crate::wide::wide(format!("{}:\\", l as char));
        // 2 = DRIVE_REMOVABLE, 5 = DRIVE_CDROM
        if matches!(unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) }, 2 | 5) {
            return true;
        }
    }
    // FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_OPEN, FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path).is_ok_and(|m| m.file_attributes() & (0x1000 | 0x4_0000 | 0x40_0000) != 0)
}

/// Bitmask of present drive letters (cheap; poll it to notice USB plug/unplug).
pub fn drive_mask() -> u32 {
    unsafe { GetLogicalDrives() }
}

/// Whether the disk holding `path` has a seek penalty (a spinning disk): `None` when it can't
/// be told (network paths, no access). Reads the device's own property; no admin needed.
pub fn seek_penalty(path: &std::path::Path) -> Option<bool> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{
        DEVICE_SEEK_PENALTY_DESCRIPTOR, IOCTL_STORAGE_QUERY_PROPERTY, PropertyStandardQuery, STORAGE_PROPERTY_QUERY,
        StorageDeviceSeekPenaltyProperty,
    };
    let s = path.to_str()?;
    let b = s.as_bytes();
    if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() {
        return None;
    }
    let dev = crate::wide::wide(format!(r"\\.\{}:", b[0] as char));
    unsafe {
        // no access rights needed for a property query
        let h = CreateFileW(
            windows::core::PCWSTR(dev.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
        .map_err(|e| log::debug!("seek_penalty open: {e}"))
        .ok()?;
        let q = STORAGE_PROPERTY_QUERY { PropertyId: StorageDeviceSeekPenaltyProperty, QueryType: PropertyStandardQuery, AdditionalParameters: [0] };
        let mut d = DEVICE_SEEK_PENALTY_DESCRIPTOR::default();
        let mut got = 0u32;
        let ok = DeviceIoControl(
            h,
            IOCTL_STORAGE_QUERY_PROPERTY,
            Some(&q as *const _ as *const _),
            std::mem::size_of::<STORAGE_PROPERTY_QUERY>() as u32,
            Some(&mut d as *mut _ as *mut _),
            std::mem::size_of::<DEVICE_SEEK_PENALTY_DESCRIPTOR>() as u32,
            Some(&mut got),
            None,
        );
        let _ = CloseHandle(h);
        ok.map_err(|e| log::debug!("seek_penalty ioctl: {e}")).ok()?;
        Some(d.IncursSeekPenalty)
    }
}

#[cfg(test)]
mod seek_tests {
    #[test]
    fn system_drive_answers() {
        // the system drive always exists: the query must not panic (virtual disks, as on CI
        // runners, may not answer it, so the result itself isn't asserted)
        let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
        let _ = super::seek_penalty(std::path::Path::new(&format!("{sys}\\")));
        assert_eq!(super::seek_penalty(std::path::Path::new(r"\\server\share")), None);
    }
}
