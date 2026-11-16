//! The code editors installed on this machine, for the "Open in editor" button (Settings > Developer tools).
//!
//! WHY (the user, 2026-11-16: "geliştirici araçları bölümünde antigravity, cursor, kiro vs. otomatik algılasa mı?"). The button
//! knew one editor, VS Code, and that only as `code` on the PATH: on a machine with another editor it did nothing until the user
//! had found the editor's exe by hand. Now the editors we know by name are looked for in the two places their installers leave
//! them: their folder under a programs folder, and their uninstall entry (which also finds one installed somewhere else).
//! Everything here only reads.
//!
//! The look itself blocks (folder checks, a walk over the uninstall entries): [`refresh`] and [`installed`] are for a thread that
//! may wait, [`peek`] is for the interface.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// An editor that is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Editor {
    pub name: &'static str,
    pub exe: PathBuf,
}

/// An editor we know: the name we show, its folder under a programs folder, its program, and the words its uninstall entry's
/// name has.
struct Known {
    name: &'static str,
    folder: &'static str,
    exe: &'static str,
    entry: &'static str,
}

/// In the order "automatic" takes them when several are installed. The folders and programs of the two Antigravity entries were
/// read off a machine that has them (2026-11-16); the others are their installers' usual ones, tried on no machine of ours.
const KNOWN: &[Known] = &[
    Known { name: "VS Code", folder: "Microsoft VS Code", exe: "Code.exe", entry: "Visual Studio Code" },
    Known { name: "VS Code Insiders", folder: "Microsoft VS Code Insiders", exe: "Code - Insiders.exe", entry: "Visual Studio Code Insiders" },
    Known { name: "Cursor", folder: "cursor", exe: "Cursor.exe", entry: "Cursor" },
    Known { name: "Windsurf", folder: "Windsurf", exe: "Windsurf.exe", entry: "Windsurf" },
    Known { name: "Antigravity", folder: "Antigravity", exe: "Antigravity.exe", entry: "Antigravity" },
    Known { name: "Antigravity IDE", folder: "Antigravity IDE", exe: "Antigravity IDE.exe", entry: "Antigravity IDE" },
    Known { name: "Kiro", folder: "Kiro", exe: "Kiro.exe", entry: "Kiro" },
    Known { name: "VSCodium", folder: "VSCodium", exe: "VSCodium.exe", entry: "VSCodium" },
    Known { name: "Trae", folder: "Trae", exe: "Trae.exe", entry: "Trae" },
    Known { name: "Zed", folder: "Zed", exe: "Zed.exe", entry: "Zed" },
];

/// A list is taken as it is for this long before it is looked for again (an editor installed while the app runs is found then).
const FRESH: Duration = Duration::from_secs(15);

/// The editors found under these programs folders: `<root>\<folder>\<program>` (a known editor's index, and its program).
fn in_folders(roots: &[PathBuf]) -> Vec<(usize, PathBuf)> {
    let mut out = Vec::new();
    for (i, k) in KNOWN.iter().enumerate() {
        for root in roots {
            let exe = root.join(k.folder).join(k.exe);
            if exe.is_file() {
                out.push((i, exe));
            }
        }
    }
    out
}

/// Which known editor an uninstall entry is, and its program: the entry's name has the editor's words AND its icon is the
/// editor's program. Both, because a name alone is not enough ("Antigravity IDE" has "Antigravity" in it, an entry of another
/// program may have "Zed" or "Cursor" in its name) and a program's name alone is a guess.
fn from_entry(display: &str, icon: &str) -> Option<(usize, PathBuf)> {
    let exe = PathBuf::from(icon.split(',').next().unwrap_or("").trim().trim_matches('"'));
    let file = exe.file_name()?.to_str()?;
    let display = display.to_lowercase();
    let i = KNOWN.iter().position(|k| file.eq_ignore_ascii_case(k.exe) && display.contains(&k.entry.to_lowercase()))?;
    Some((i, exe))
}

/// Every uninstall entry's name and icon, per user and per machine.
fn uninstall_entries() -> Vec<(String, String)> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, RegCloseKey, RegEnumKeyExW, RegOpenKeyExW};
    const KEYS: [&str; 2] = [r"Software\Microsoft\Windows\CurrentVersion\Uninstall", r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"];
    let mut out = Vec::new();
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        for base in KEYS {
            let w = crate::wide::wide(base);
            let mut key = HKEY::default();
            if unsafe { RegOpenKeyExW(root, windows::core::PCWSTR(w.as_ptr()), Some(0), KEY_READ, &mut key) }.is_err() {
                continue;
            }
            for i in 0.. {
                let mut name = [0u16; 256];
                let mut len = name.len() as u32;
                if unsafe { RegEnumKeyExW(key, i, Some(windows::core::PWSTR(name.as_mut_ptr())), &mut len, None, None, None, None) }.is_err() {
                    break;
                }
                let sub = format!(r"{base}\{}", String::from_utf16_lossy(&name[..len as usize]));
                let Some(icon) = crate::bench::reg_string(root, &sub, "DisplayIcon") else { continue };
                let display = crate::bench::reg_string(root, &sub, "DisplayName").unwrap_or_default();
                out.push((display, icon));
            }
            unsafe {
                let _ = RegCloseKey(key);
            }
        }
    }
    out
}

/// One list out of what the two looks found: each program once (however its path is spelled), in the order of [`KNOWN`].
fn merged(mut found: Vec<(usize, PathBuf)>) -> Vec<Editor> {
    found.sort_by_key(|(i, _)| *i);
    let mut seen = std::collections::HashSet::new();
    found.into_iter().filter(|(_, exe)| seen.insert(exe.to_string_lossy().to_lowercase())).map(|(i, exe)| Editor { name: KNOWN[i].name, exe }).collect()
}

/// The programs folders an installer for one user or for the machine uses.
fn program_roots() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    [var("LOCALAPPDATA").map(|p| p.join("Programs")), var("ProgramFiles"), var("ProgramFiles(x86)")].into_iter().flatten().collect()
}

/// Looks for the editors now. Blocking.
fn detect() -> Vec<Editor> {
    let mut found = in_folders(&program_roots());
    found.extend(uninstall_entries().iter().filter_map(|(display, icon)| from_entry(display, icon)).filter(|(_, exe)| exe.is_file()));
    merged(found)
}

struct State {
    list: Vec<Editor>,
    at: Option<Instant>,
}

static STATE: Mutex<State> = Mutex::new(State { list: Vec::new(), at: None });

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The list as it was last found, and whether that was a moment ago. Never blocks: for the interface, which asks for a
/// [`refresh`] on another thread when the list is not fresh.
pub fn peek() -> (Vec<Editor>, bool) {
    let s = state();
    (s.list.clone(), s.at.is_some_and(|t| t.elapsed() < FRESH))
}

/// Looks for the editors and keeps the list; says whether it is another list than before. Blocking.
pub fn refresh() -> bool {
    let list = detect();
    let mut s = state();
    let changed = s.list != list;
    *s = State { list, at: Some(Instant::now()) };
    changed
}

/// The installed editors, looked for now unless that was done a moment ago. Blocking.
pub fn installed() -> Vec<Editor> {
    let (list, fresh) = peek();
    if fresh {
        return list;
    }
    refresh();
    peek().0
}

/// The name of the editor a program is, when it is one of the installed ones (for a setting that holds a path).
pub fn name_of(list: &[Editor], exe: &Path) -> Option<&'static str> {
    let wanted = exe.to_string_lossy().to_lowercase();
    list.iter().find(|e| e.exe.to_string_lossy().to_lowercase() == wanted).map(|e| e.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(i: usize) -> &'static str {
        KNOWN[i].name
    }

    /// An uninstall entry is an editor when its name has the editor's words and its icon is the editor's program: both.
    #[test]
    fn an_uninstall_entry_is_an_editor_by_its_name_and_its_program() {
        let entry = |display: &str, icon: &str| from_entry(display, icon).map(|(i, exe)| (name(i), exe));
        // as read off a machine that has it: the IDE is not taken for the editor whose name its own starts with
        let ide = r"C:\Users\x\AppData\Local\Programs\Antigravity IDE\Antigravity IDE.exe";
        assert_eq!(entry("Antigravity IDE (User)", ide), Some(("Antigravity IDE", PathBuf::from(ide))));
        assert_eq!(entry("Antigravity (User)", r"C:\P\Antigravity\Antigravity.exe").map(|e| e.0), Some("Antigravity"));
        // an icon as installers write it: quoted, with the icon's number
        assert_eq!(entry("Microsoft Visual Studio Code (User)", r#""D:\Tools\VS Code\Code.exe",0"#), Some(("VS Code", PathBuf::from(r"D:\Tools\VS Code\Code.exe"))));
        assert_eq!(entry("Microsoft Visual Studio Code Insiders (User)", r"C:\P\Code - Insiders.exe").map(|e| e.0), Some("VS Code Insiders"));
        assert_eq!(entry("Cursor (User)", r"c:\p\cursor\CURSOR.EXE").map(|e| e.0), Some("Cursor"), "letters' case does not matter");
        // the name fits but the icon is another program (an uninstaller, another product with the word in its name)
        assert_eq!(entry("Kiro", r"C:\P\Kiro\unins000.exe"), None);
        assert_eq!(entry("Zed Attack Proxy", r"C:\P\ZAP\ZAP.exe"), None);
        // the program's name fits but the entry is something else
        assert_eq!(entry("Barcode Studio", r"C:\P\Barcode\Code.exe"), None);
        assert_eq!(entry("File Pilot", r"C:\P\FilePilot\FPilot.exe"), None);
        assert_eq!(entry("Cursor", ""), None);
    }

    /// The programs folders are looked through for each editor's own folder and program.
    #[test]
    fn editors_are_found_in_their_folders_under_a_programs_folder() {
        let root = std::env::temp_dir().join(format!("rf-editors-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (folder, exe) in [("cursor", "Cursor.exe"), ("Kiro", "Kiro.exe"), ("Windsurf", "notes.txt"), ("Somebody Else", "Code.exe")] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
            std::fs::write(root.join(folder).join(exe), b"").unwrap();
        }
        let found = in_folders(&[root.join("not there"), root.clone()]);
        assert_eq!(found.iter().map(|(i, _)| name(*i)).collect::<Vec<_>>(), ["Cursor", "Kiro"], "in the table's order; a folder without the program is none");
        assert_eq!(found[0].1, root.join("cursor").join("Cursor.exe"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// What the folders and the uninstall entries both found is one editor, and the list is in the table's order.
    #[test]
    fn an_editor_found_twice_is_listed_once_in_the_tables_order() {
        let at = |n: &str| KNOWN.iter().position(|k| k.name == n).unwrap();
        let list = merged(vec![
            (at("Kiro"), PathBuf::from(r"C:\P\Kiro\Kiro.exe")),
            (at("Cursor"), PathBuf::from(r"C:\P\cursor\Cursor.exe")),
            (at("Kiro"), PathBuf::from(r"c:\p\kiro\kiro.exe")),
            (at("Cursor"), PathBuf::from(r"D:\Portable\Cursor\Cursor.exe")),
        ]);
        assert_eq!(list.iter().map(|e| e.name).collect::<Vec<_>>(), ["Cursor", "Cursor", "Kiro"], "two installs of one editor are two");
        assert_eq!(name_of(&list, Path::new(r"C:\p\KIRO\Kiro.exe")), Some("Kiro"));
        assert_eq!(name_of(&list, Path::new(r"C:\Tools\mine.exe")), None);
        assert!(merged(Vec::new()).is_empty());
    }

    /// The look on this machine: for a read of what it finds (`--ignored --nocapture`).
    #[test]
    #[ignore]
    fn the_editors_on_this_machine() {
        for e in detect() {
            eprintln!("{}: {}", e.name, e.exe.display());
        }
    }
}
