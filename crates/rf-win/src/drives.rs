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

/// Lists drive letters. Volume info can stall on disconnected network / empty optical drives,
/// so call this off the UI thread.
pub fn list_drives() -> Vec<Drive> {
    let mask = unsafe { GetLogicalDrives() };
    let mut out = Vec::new();
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let letter = (b'A' + i as u8) as char;
        let root = format!("{letter}:\\");
        let w = wide(&root);
        let kind = match unsafe { GetDriveTypeW(PCWSTR(w.as_ptr())) } {
            2 => DriveKind::Removable,
            3 => DriveKind::Fixed,
            4 => DriveKind::Network,
            5 => DriveKind::Optical,
            6 => DriveKind::RamDisk,
            _ => DriveKind::Unknown,
        };
        let mut label = [0u16; 261];
        let mut fs = [0u16; 261];
        let has_vol = unsafe {
            GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), None, None, None, Some(&mut fs))
        }
        .is_ok();
        if !has_vol && kind == DriveKind::Optical {
            // empty DVD tray: still list it, without sizes
        }
        let (mut free, mut total) = (0u64, 0u64);
        if has_vol {
            unsafe {
                let _ = GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), Some(&mut total), None);
            }
        }
        out.push(Drive {
            root: PathBuf::from(root),
            letter,
            label: from_wide(&label),
            kind,
            total,
            free,
            fs: from_wide(&fs),
        });
    }
    out
}

#[cfg(test)]
mod tests {
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
