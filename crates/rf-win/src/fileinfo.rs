//! Facts for the Properties window: times, attributes, size on disk, default app. Blocking
//! (disk / registry): call off the UI thread.

use std::os::windows::fs::MetadataExt;
use std::path::Path;

use windows::Win32::Storage::FileSystem::{
    FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_READONLY, FILE_FLAGS_AND_ATTRIBUTES, GetCompressedFileSizeW, GetDiskFreeSpaceW,
    INVALID_FILE_SIZE, SetFileAttributesW,
};
use windows::Win32::UI::Shell::{ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_FRIENDLYAPPNAME, AssocQueryStringW};
use windows::core::{PCWSTR, PWSTR, w};

use crate::wide::{from_wide, wide};

#[derive(Debug, Clone, Default)]
pub struct Info {
    pub is_dir: bool,
    pub size: u64,
    /// Allocated size (compression / sparse / cluster rounding). Files only.
    pub size_on_disk: Option<u64>,
    pub created_ms: i64,
    pub modified_ms: i64,
    pub accessed_ms: i64,
    pub attrs: u32,
    /// Friendly name of the app that opens this type ("Fotoğraflar"). Files only.
    pub opens_with: Option<String>,
}

fn ms(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn info(path: &Path) -> std::io::Result<Info> {
    let m = std::fs::metadata(path)?;
    let is_dir = m.is_dir();
    Ok(Info {
        is_dir,
        size: if is_dir { 0 } else { m.len() },
        size_on_disk: if is_dir { None } else { size_on_disk(path) },
        created_ms: ms(m.created()),
        modified_ms: ms(m.modified()),
        accessed_ms: ms(m.accessed()),
        attrs: m.file_attributes(),
        opens_with: if is_dir { None } else { opens_with(path) },
    })
}

/// Bytes the file occupies: compressed size rounded up to whole clusters.
pub fn size_on_disk(path: &Path) -> Option<u64> {
    let w = wide(path);
    let mut high = 0u32;
    let low = unsafe { GetCompressedFileSizeW(PCWSTR(w.as_ptr()), Some(&mut high)) };
    if low == INVALID_FILE_SIZE && unsafe { windows::Win32::Foundation::GetLastError() }.0 != 0 {
        return None;
    }
    let raw = ((high as u64) << 32) | low as u64;
    let cluster = cluster_size(path).unwrap_or(4096).max(1);
    Some(raw.div_ceil(cluster) * cluster)
}

fn cluster_size(path: &Path) -> Option<u64> {
    let root = path.ancestors().last()?.to_path_buf();
    let mut r = root.to_string_lossy().to_string();
    if !r.ends_with('\\') {
        r.push('\\');
    }
    let w = wide(r);
    let (mut spc, mut bps, mut free, mut total) = (0u32, 0u32, 0u32, 0u32);
    unsafe { GetDiskFreeSpaceW(PCWSTR(w.as_ptr()), Some(&mut spc), Some(&mut bps), Some(&mut free), Some(&mut total)) }.ok()?;
    Some(spc as u64 * bps as u64)
}

/// Friendly name of the default app for the file's type.
pub fn opens_with(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_string_lossy().to_string();
    let ext = wide(format!(".{ext}"));
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    unsafe {
        AssocQueryStringW(ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_FRIENDLYAPPNAME, PCWSTR(ext.as_ptr()), w!("open"), Some(PWSTR(buf.as_mut_ptr())), &mut len)
            .ok()
            .ok()?;
    }
    Some(from_wide(&buf)).filter(|s| !s.is_empty())
}

fn zone_stream(path: &Path) -> std::ffi::OsString {
    let mut s = path.as_os_str().to_owned();
    s.push(":Zone.Identifier");
    s
}

/// Whether Windows treats the file as downloaded (its Zone.Identifier stream names the Internet
/// or Restricted zone): Explorer's "This file came from another computer… Unblock".
pub fn zone_blocked(path: &Path) -> bool {
    std::fs::read(zone_stream(path)).is_ok_and(|b| {
        // usually ANSI, sometimes UTF-16 (written by other tools)
        let text = if b.starts_with(&[0xff, 0xfe]) {
            String::from_utf16_lossy(&b[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
        } else {
            String::from_utf8_lossy(&b).into_owned()
        };
        text.lines().any(|l| l.trim().strip_prefix("ZoneId=").and_then(|v| v.trim().parse::<u32>().ok()).is_some_and(|z| z >= 3))
    })
}

/// Removes the downloaded mark (the Zone.Identifier stream), as Explorer's "Unblock" does.
pub fn unblock(path: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(zone_stream(path)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

/// Sets / clears read-only and hidden, keeping the other attribute bits.
pub fn set_flags(path: &Path, readonly: Option<bool>, hidden: Option<bool>) -> std::io::Result<()> {
    let cur = std::fs::metadata(path)?.file_attributes();
    let mut a = cur;
    let mut put = |bit: u32, on: Option<bool>| match on {
        Some(true) => a |= bit,
        Some(false) => a &= !bit,
        None => {}
    };
    put(FILE_ATTRIBUTE_READONLY.0, readonly);
    put(FILE_ATTRIBUTE_HIDDEN.0, hidden);
    if a == cur {
        return Ok(());
    }
    let w = wide(path);
    unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a)) }.map_err(|e| std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))
}

#[cfg(test)]
mod tests {
    #[test]
    fn downloaded_mark_is_read_and_removed() {
        let p = std::env::temp_dir().join(format!("rf-zone-{}.exe", std::process::id()));
        std::fs::write(&p, b"MZ").unwrap();
        assert!(!super::zone_blocked(&p));
        std::fs::write(super::zone_stream(&p), "[ZoneTransfer]\r\nZoneId=3\r\nHostUrl=https://example.com/\r\n").unwrap();
        assert!(super::zone_blocked(&p));
        super::unblock(&p).unwrap();
        assert!(!super::zone_blocked(&p));
        assert_eq!(std::fs::read(&p).unwrap(), b"MZ"); // the file itself untouched
        super::unblock(&p).unwrap(); // nothing to remove: fine
        // a local-intranet mark is not a block
        std::fs::write(super::zone_stream(&p), "[ZoneTransfer]\r\nZoneId=1\r\n").unwrap();
        assert!(!super::zone_blocked(&p));
        std::fs::remove_file(&p).unwrap();
    }

    use super::*;

    #[test]
    fn info_and_flags_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rf-fileinfo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.txt");
        std::fs::write(&f, vec![b'x'; 10]).unwrap();
        let i = info(&f).unwrap();
        assert_eq!(i.size, 10);
        assert!(i.size_on_disk.unwrap() >= 10 || i.size_on_disk == Some(0));
        set_flags(&f, Some(true), Some(true)).unwrap();
        let a = info(&f).unwrap().attrs;
        assert!(a & FILE_ATTRIBUTE_READONLY.0 != 0 && a & FILE_ATTRIBUTE_HIDDEN.0 != 0);
        set_flags(&f, Some(false), Some(false)).unwrap();
        let a = info(&f).unwrap().attrs;
        assert!(a & FILE_ATTRIBUTE_READONLY.0 == 0 && a & FILE_ATTRIBUTE_HIDDEN.0 == 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
