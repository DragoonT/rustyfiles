//! Localized folder names, the way Explorer shows them: a read-only or system folder whose
//! `desktop.ini` has `LocalizedResourceName` (C:\Users → "Kullanıcılar", Downloads →
//! "İndirilenler" on a Turkish system). Only the shown name changes; paths stay real.
//!
//! Results are cached per folder and per resource string (a handful of distinct ones), so
//! breadcrumbs and tab titles can ask on every frame.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use windows::Win32::Storage::FileSystem::GetFileAttributesW;
use windows::Win32::UI::Shell::SHLoadIndirectString;
use windows::core::PCWSTR;

use crate::wide::{from_wide, long_path, wide};

const READONLY: u32 = 0x1;
const SYSTEM: u32 = 0x4;
const DIRECTORY: u32 = 0x10;

type Cache<K> = Mutex<HashMap<K, Option<Arc<str>>>>;

fn by_path() -> &'static Cache<PathBuf> {
    static C: OnceLock<Cache<PathBuf>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn by_resource() -> &'static Cache<String> {
    static C: OnceLock<Cache<String>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// The localized name of folder `dir`, if it has one. `attrs` are the folder's attributes
/// when already known (from a listing), else they are read.
pub fn folder_label(dir: &Path, attrs: Option<u32>) -> Option<Arc<str>> {
    if let Some(hit) = by_path().lock().ok()?.get(dir) {
        return hit.clone();
    }
    // breadcrumbs and tab titles ask while drawing: never touch a (possibly offline) network
    // path from here (the drive-type check itself does no network I/O)
    if is_network(dir) {
        return None;
    }
    let a = attrs.unwrap_or_else(|| unsafe { GetFileAttributesW(PCWSTR(long_path(dir).as_ptr())) });
    // Explorer only looks at desktop.ini of read-only or system folders (PathIsSystemFolder)
    let label = if a != u32::MAX && a & DIRECTORY != 0 && a & (READONLY | SYSTEM) != 0 {
        std::fs::read(dir.join("desktop.ini")).ok().and_then(|raw| resource_name(&decode(&raw))).and_then(|r| resolve(&r))
    } else {
        None
    };
    if let Ok(mut c) = by_path().lock() {
        if c.len() > 4096 {
            c.clear();
        }
        c.insert(dir.to_path_buf(), label.clone());
    }
    label
}

/// The localized name of `dir` if it was already asked for, from the cache alone: it never touches the disk, so it is the one to call
/// while drawing (a breadcrumb, a tab title). A folder that was not asked for yet shows under its real name until [`warm`] has run on
/// a worker thread. [`folder_label`] reads attributes and `desktop.ini`, and on a place that has to wake up first (a sleeping disk, a
/// phone, a WSL distribution) that holds the calling thread for seconds.
pub fn label_if_known(dir: &Path) -> Option<Arc<str>> {
    by_path().lock().ok()?.get(dir)?.clone()
}

/// Whether [`folder_label`] has already answered for `dir` (with a name or with "none").
pub fn is_known(dir: &Path) -> bool {
    by_path().lock().is_ok_and(|c| c.contains_key(dir))
}

/// Asks [`folder_label`] for every folder not asked for yet, so that [`label_if_known`] has the answers. Touches the disk: call it
/// from a worker thread only. Returns how many of them have a localized name (when it is more than none, what is on the screen
/// should be drawn again).
pub fn warm<I: IntoIterator<Item = PathBuf>>(dirs: I) -> usize {
    dirs.into_iter().filter(|d| !is_known(d)).filter(|d| folder_label(d, None).is_some()).count()
}

/// A UNC path or a network drive letter (reading desktop.ini there costs a round trip).
pub fn is_network(p: &Path) -> bool {
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    if s.starts_with(r"\\?\UNC\") || (s.starts_with(r"\\") && !s.starts_with(r"\\?\")) {
        return true;
    }
    let letter = match b {
        [l, b':', ..] => *l,
        [b'\\', b'\\', b'?', b'\\', l, b':', ..] => *l,
        _ => return false,
    };
    let root = wide(format!("{}:\\", letter as char));
    // DRIVE_REMOTE = 4
    unsafe { windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(root.as_ptr())) == 4 }
}

/// desktop.ini is UTF-16LE with a BOM, or the ANSI code page (ASCII in practice).
fn decode(raw: &[u8]) -> String {
    if raw.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = raw[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(raw)).into_owned()
    }
}

/// `LocalizedResourceName=` in the `[.ShellClassInfo]` section.
fn resource_name(ini: &str) -> Option<String> {
    let mut in_section = false;
    for line in ini.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line.eq_ignore_ascii_case("[.ShellClassInfo]");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            if k.trim().eq_ignore_ascii_case("LocalizedResourceName") {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

/// "@%SystemRoot%\system32\shell32.dll,-21813" → the string in the user's UI language; a
/// plain value is used as is.
fn resolve(res: &str) -> Option<Arc<str>> {
    if !res.starts_with('@') {
        return Some(res.into());
    }
    if let Some(hit) = by_resource().lock().ok()?.get(res) {
        return hit.clone();
    }
    let got = unsafe {
        let input = wide(expand_env(res));
        let mut out = vec![0u16; 512];
        SHLoadIndirectString(PCWSTR(input.as_ptr()), &mut out, None).ok().map(|_| from_wide(&out)).filter(|s| !s.is_empty()).map(Arc::<str>::from)
    };
    if let Ok(mut c) = by_resource().lock() {
        c.insert(res.to_string(), got.clone());
    }
    got
}

/// `%Name%` → the environment variable's value (unknown names stay as written).
pub fn expand_env(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 32);
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ini() {
        let ini = "\r\n[.ShellClassInfo]\r\nLocalizedResourceName=@%SystemRoot%\\system32\\shell32.dll,-21813\r\n";
        assert_eq!(resource_name(ini).as_deref(), Some("@%SystemRoot%\\system32\\shell32.dll,-21813"));
        assert_eq!(resource_name("[Other]\nLocalizedResourceName=x\n"), None);
        assert_eq!(resource_name("[.shellclassinfo]\nIconResource=a\nlocalizedresourcename = Klasörüm\n").as_deref(), Some("Klasörüm"));
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain("[.ShellClassInfo]\nLocalizedResourceName=Ad\n".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(resource_name(&decode(&utf16)).as_deref(), Some("Ad"));
    }

    #[test]
    fn users_folder_has_a_name() {
        // C:\Users ships with LocalizedResourceName on every Windows install
        let users = PathBuf::from(std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()) + "\\Users");
        let got = folder_label(&users, None);
        assert!(got.as_deref().is_some_and(|s| !s.is_empty() && !s.starts_with('@')), "{got:?}");
        // a plain folder has none
        assert_eq!(folder_label(&std::env::temp_dir(), None), None);
    }

    #[test]
    fn network_paths() {
        assert!(is_network(Path::new(r"\\nas\paylaşım\klasör")));
        assert!(is_network(Path::new(r"\\?\UNC\nas\x")));
        assert!(!is_network(Path::new(r"C:\Users")));
        assert!(!is_network(Path::new(r"\\?\C:\Users")));
    }

    /// The drawing path asks the cache only: a folder not asked for before is not looked at (nothing is cached for it), and after
    /// `warm` on a worker the same call has the answer.
    #[test]
    fn the_cache_only_call_never_reads_and_warm_fills_it() {
        let d = std::env::temp_dir().join(format!("rf-loc-warm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let sub = d.join("salt");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("desktop.ini"), "[.ShellClassInfo]\r\nLocalizedResourceName=Önbellek Adı\r\n").unwrap();
        let mut perm = std::fs::metadata(&sub).unwrap().permissions();
        perm.set_readonly(true); // Explorer reads desktop.ini of read-only or system folders only
        std::fs::set_permissions(&sub, perm).unwrap();

        assert_eq!(label_if_known(&sub), None);
        assert!(!is_known(&sub), "the cache-only call must not fill the cache");
        assert_eq!(warm([sub.clone(), d.clone()]), 1);
        assert!(is_known(&sub) && is_known(&d));
        assert_eq!(label_if_known(&sub).as_deref(), Some("Önbellek Adı"));
        assert_eq!(label_if_known(&d), None);
        // asked once: a second warm has nothing to do
        assert_eq!(warm([sub.clone()]), 0);

        let mut perm = std::fs::metadata(&sub).unwrap().permissions();
        perm.set_readonly(false);
        let _ = std::fs::set_permissions(&sub, perm);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn own_folder_with_plain_name() {
        let d = std::env::temp_dir().join(format!("rf-loc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("desktop.ini"), "[.ShellClassInfo]\r\nLocalizedResourceName=Özel Ad\r\n").unwrap();
        // not read-only / system yet: Explorer ignores the file, so do we
        assert_eq!(folder_label(&d, Some(DIRECTORY)), None);
        let sub = d.join("salt");
        std::fs::create_dir_all(&sub).unwrap();
        std::fs::write(sub.join("desktop.ini"), "[.ShellClassInfo]\r\nLocalizedResourceName=Özel Ad\r\n").unwrap();
        assert_eq!(folder_label(&sub, Some(DIRECTORY | READONLY)).as_deref(), Some("Özel Ad"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
