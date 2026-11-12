//! What the Start panel needs to start a classic "All apps" entry by itself when the shell refuses (no Explorer): the entry's shortcut
//! file, or, failing that, the program with the arguments the entry carries.
//!
//! WHY NOT JUST THE TARGET. The AppsFolder item of a classic program is its Start-menu shortcut. The shell's "open" of the item runs the
//! shortcut: its arguments, its start folder, its show state, its own AppUserModelID. The item exposes the shortcut's TARGET
//! (`System.Link.TargetParsingPath`) and, when it has any, its ARGUMENTS (`System.Link.Arguments`; read on this PC: 46 of 295 classic
//! entries have them), but not its start folder and not the shortcut's own path (`SIGDN_FILESYSPATH` fails for every entry,
//! `System.AppUserModel.BestShortcut` held no file path in any of the 295). Starting only the target would lose the start folder, the show state and
//! the identity, and would start a shortcut such as "pythonw.exe + a script" or "chrome.exe --profile-directory=..." wrongly. So:
//!
//! 1. the shortcut file itself is looked for in the two Start-menu folders, by the entry's name (the AppsFolder shows a shortcut under
//!    its file name) and CONFIRMED by its target being the entry's target ([`find_shortcut`]); the shell then runs it like a double
//!    click would (`ShellExecuteEx` on a file path needs no Explorer);
//! 2. when no shortcut is found (the entry comes from a registered program, not a shortcut), the program is started with the entry's
//!    arguments and its own folder as the start folder.
//!
//! Only used after the shell's own open failed, so with Explorer running none of this runs.

use std::path::{Path, PathBuf};

/// How deep the Start-menu folders are searched (the shortcuts of a vendor's folder are one or two levels down).
const SEARCH_DEPTH: u32 = 3;

/// Whether `file_name` is the shortcut the AppsFolder shows under `app_name`: the same name with `.lnk`, any case.
pub fn is_shortcut_named(file_name: &std::ffi::OsStr, app_name: &str) -> bool {
    let f = file_name.to_string_lossy();
    match f.rsplit_once('.') {
        Some((stem, ext)) => ext.eq_ignore_ascii_case("lnk") && stem.to_lowercase() == app_name.trim().to_lowercase(),
        None => false,
    }
}

/// The first shortcut named for `app_name` under `roots` (depth-limited) that `confirm` accepts. `confirm` reads the shortcut and says
/// whether its target is the entry's target: a second shortcut of the same name that starts something else is not the entry.
pub fn find_shortcut_in(roots: &[PathBuf], app_name: &str, confirm: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    fn walk(dir: &Path, depth: u32, app_name: &str, confirm: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
        let rd = std::fs::read_dir(dir).ok()?;
        let mut subdirs = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            match e.file_type() {
                Ok(t) if t.is_file() && is_shortcut_named(&e.file_name(), app_name) && confirm(&p) => return Some(p),
                Ok(t) if t.is_dir() && depth > 0 => subdirs.push(p),
                _ => {}
            }
        }
        // (files of this level first, then the folders below: the shallow shortcut is the likelier one)
        subdirs.into_iter().find_map(|d| walk(&d, depth - 1, app_name, confirm))
    }
    roots.iter().find_map(|r| walk(r, SEARCH_DEPTH, app_name, confirm))
}

/// The two Start-menu program folders (all users, then this user's), those that exist.
pub fn start_menu_roots() -> Vec<PathBuf> {
    ["ProgramData", "APPDATA"].iter().filter_map(std::env::var_os).map(|d| PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs")).filter(|p| p.is_dir()).collect()
}

/// Whether two program paths name the same file (case and slash style do not matter).
pub fn same_target(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase();
    !a.as_os_str().is_empty() && norm(a) == norm(b)
}

/// The shortcut file of an entry: found by name in the Start menu and confirmed by its target. Reads shortcuts through COM: shell thread.
pub fn find_shortcut(app_name: &str, target: &Path) -> Option<PathBuf> {
    find_shortcut_in(&start_menu_roots(), app_name, &|lnk| crate::shortcut::resolve(lnk).is_ok_and(|info| same_target(&info.target, target)))
}

/// `System.Link.Arguments` of an AppsFolder item (`shell:AppsFolder\<id>`), when it has any.
pub fn arguments_of(parsing: &str) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_DEFAULT, IPropertyStore, PSGetPropertyKeyFromName, SHGetPropertyStoreFromParsingName};
    use windows::core::{PCWSTR, w};
    crate::shell::ensure_com();
    let path = crate::wide::wide(parsing);
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreFromParsingName(PCWSTR(path.as_ptr()), None, GPS_DEFAULT).ok()?;
        let mut key = Default::default();
        PSGetPropertyKeyFromName(w!("System.Link.Arguments"), &mut key).ok()?;
        let v = store.GetValue(&key).ok()?;
        let s = PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|a| !a.trim().is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn a_shortcut_is_the_one_the_entry_names() {
        assert!(is_shortcut_named(OsStr::new("Notepad++.lnk"), "Notepad++"));
        assert!(is_shortcut_named(OsStr::new("NOTEPAD++.LNK"), "notepad++"));
        assert!(is_shortcut_named(OsStr::new("IDLE (Python 3.12 64-bit).lnk"), " IDLE (Python 3.12 64-bit) "));
        assert!(!is_shortcut_named(OsStr::new("Notepad++.url"), "Notepad++"));
        assert!(!is_shortcut_named(OsStr::new("Notepad++ Help.lnk"), "Notepad++"));
        assert!(!is_shortcut_named(OsStr::new("Notepad++"), "Notepad++"));
    }

    #[test]
    fn the_same_program_by_two_spellings_is_one_target() {
        assert!(same_target(Path::new(r"C:\Program Files\X\x.exe"), Path::new(r"c:/program files/x/X.EXE")));
        assert!(!same_target(Path::new(r"C:\a\x.exe"), Path::new(r"C:\b\x.exe")));
        assert!(!same_target(Path::new(""), Path::new("")), "an empty target confirms nothing");
    }

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-appdirect-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_shortcut_is_found_by_name_below_a_vendor_folder_and_confirmed_by_its_target() {
        let root = scratch("find");
        std::fs::create_dir_all(root.join("Vendor/Tools")).unwrap();
        std::fs::write(root.join("Vendor/Tools/Tool.lnk"), b"x").unwrap(); // same name, starts something else
        std::fs::write(root.join("Vendor/Tool.lnk"), b"y").unwrap(); // the right one
        std::fs::write(root.join("Other.lnk"), b"z").unwrap();
        let wanted = root.join("Vendor/Tool.lnk");
        let confirm = |p: &Path| std::fs::read(p).map(|b| b == b"y").unwrap_or(false);
        assert_eq!(find_shortcut_in(std::slice::from_ref(&root), "Tool", &confirm), Some(wanted));
        // nobody confirms: not found, and nothing is guessed from the name alone
        assert_eq!(find_shortcut_in(std::slice::from_ref(&root), "Tool", &|_| false), None);
        // a name nothing carries
        assert_eq!(find_shortcut_in(std::slice::from_ref(&root), "Nothing", &|_| true), None);
        // a root that does not exist is skipped
        assert_eq!(find_shortcut_in(&[root.join("missing"), root.clone()], "Other", &|_| true), Some(root.join("Other.lnk")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folders_deeper_than_the_limit_are_not_searched() {
        let root = scratch("deep");
        std::fs::create_dir_all(root.join("a/b/c/d")).unwrap();
        std::fs::write(root.join("a/b/c/d/Deep.lnk"), b"x").unwrap();
        assert_eq!(find_shortcut_in(std::slice::from_ref(&root), "Deep", &|_| true), None);
        std::fs::write(root.join("a/b/c/Near.lnk"), b"x").unwrap();
        assert!(find_shortcut_in(std::slice::from_ref(&root), "Near", &|_| true).is_some());
        let _ = std::fs::remove_dir_all(&root);
    }
}
