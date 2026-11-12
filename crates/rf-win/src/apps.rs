//! Installed apps, as Windows' "All apps" lists them (the AppsFolder: classic programs and Store
//! apps alike), for the quick search: name, an id to start it with, and its icon.
//!
//! Blocks (shell enumeration, a few hundred ms): call from a worker thread.

use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{BHID_EnumItems, IEnumShellItems, IShellItem, SHCreateItemFromParsingName, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING};
use windows::core::PCWSTR;

use crate::shell::ensure_com;
use crate::wide::wide;

const APPS_FOLDER: &str = "shell:AppsFolder";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    pub name: String,
    /// The app's id in the AppsFolder (a Store app's AUMID, or a program's path / id).
    pub id: String,
}

impl App {
    /// What the shell takes to start it, or to draw its icon.
    pub fn parsing(&self) -> String {
        format!("{APPS_FOLDER}\\{}", self.id)
    }
}

fn display(item: &IShellItem, sigdn: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(sigdn).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

/// Entries of "All apps" that are documents or web links (a readme, a help file, a vendor's
/// web page), not something to start.
fn is_document(id: &str) -> bool {
    let l = id.to_ascii_lowercase();
    [".url", ".chm", ".pdf", ".txt", ".html", ".htm", ".rtf", ".doc", ".docx", ".hlp", ".ini", ".log"].iter().any(|e| l.ends_with(e))
        || l.starts_with("http://")
        || l.starts_with("https://")
}

/// An "Uninstall X" shortcut: nothing to start from a search.
fn is_uninstaller(name: &str) -> bool {
    let l = name.to_lowercase();
    l.starts_with("uninstall") || l.contains("kaldır") || l.ends_with(" uninstall") || l.ends_with(" uninstaller")
}

/// Everything in "All apps", sorted by name.
pub fn list() -> anyhow::Result<Vec<App>> {
    ensure_com();
    let w = wide(APPS_FOLDER);
    let folder: IShellItem = unsafe { SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None)? };
    let en: IEnumShellItems = unsafe { folder.BindToHandler(None, &BHID_EnumItems)? };
    let mut apps = Vec::new();
    loop {
        let mut buf: [Option<IShellItem>; 64] = std::array::from_fn(|_| None);
        let mut got = 0u32;
        let _ = unsafe { en.Next(&mut buf, Some(&mut got)) };
        for item in buf.iter().take(got as usize).flatten() {
            let (Some(id), Some(name)) = (display(item, SIGDN_PARENTRELATIVEPARSING), display(item, SIGDN_NORMALDISPLAY)) else { continue };
            if !is_document(&id) && !is_uninstaller(&name) && !name.trim().is_empty() {
                apps.push(App { name, id });
            }
        }
        if got < 64 {
            break;
        }
    }
    // the same program from the all-users and the per-user Start menu: one row
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps.dedup_by(|a, b| a.name.to_lowercase() == b.name.to_lowercase());
    Ok(apps)
}

/// Starts `app`: through the shell's own "open" of its All-apps entry, and when the shell refuses, by ourselves.
///
/// With no Explorer running the shell answers that "open" with "class not registered" (0x80040154; seen on the user's PC while it
/// ran on our dock alone: every entry of Start said it could not be started). The entry still tells what is behind it, so then the
/// program or shortcut is started as a file, and a packaged app through its activation.
pub fn launch(app: &App) -> anyhow::Result<()> {
    let first = match crate::shellfolder::open(&app.parsing()) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    match launch_directly(app) {
        Ok(()) => {
            log::info!("apps: the shell did not start {:?} ({first}); started directly", app.name);
            Ok(())
        }
        Err(second) => Err(anyhow::anyhow!("{first}; directly: {second}")),
    }
}

/// What is tried for an entry: its shortcut, the program with its arguments, or the packaged app's activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direct {
    /// The Start-menu shortcut of the entry, run by the shell's "open" of a file path (its arguments, start folder, show state and
    /// AppUserModelID come along). See `apps_direct` for how it is found.
    Shortcut(std::path::PathBuf),
    /// The program the entry points at, with the arguments the entry carries and its own folder as the start folder.
    File { path: std::path::PathBuf, args: String },
    Packaged(String),
    Nothing,
}

/// The pure choice: the entry's shortcut wins (nothing is lost); else the program it names, with the arguments the entry carries;
/// else an id of the form `family!app` is a packaged app.
pub fn direct_way(id: &str, target: Option<std::path::PathBuf>, args: Option<String>, shortcut: Option<std::path::PathBuf>) -> Direct {
    if let Some(s) = shortcut.filter(|s| !s.as_os_str().is_empty()) {
        return Direct::Shortcut(s);
    }
    match target {
        Some(t) if !t.as_os_str().is_empty() => Direct::File { path: t, args: args.unwrap_or_default() },
        _ if id.contains('!') => Direct::Packaged(id.to_string()),
        _ => Direct::Nothing,
    }
}

/// The decision for an entry, from what the shell says about it (shell thread).
#[doc(hidden)]
pub fn direct_way_of(app: &App) -> Direct {
    let parsing = app.parsing();
    let target = crate::dock::startapps::target_of(&parsing);
    let args = crate::apps_direct::arguments_of(&parsing);
    let shortcut = target.as_deref().and_then(|t| crate::apps_direct::find_shortcut(&app.name, t));
    direct_way(&app.id, target, args, shortcut)
}

/// For the probe example: the direct way alone.
#[doc(hidden)]
pub fn launch_directly_for_probe(app: &App) -> anyhow::Result<()> {
    launch_directly(app)
}

fn launch_directly(app: &App) -> anyhow::Result<()> {
    use crate::dock::model::Launch;
    match direct_way_of(app) {
        Direct::Shortcut(lnk) => crate::dock::launch::start(&Launch::Link(lnk)),
        Direct::File { path, args } => crate::dock::launch::start(&Launch::Exe { path, args }),
        Direct::Packaged(id) => crate::activate::packaged(&id, ""),
        Direct::Nothing => anyhow::bail!("the entry names no file and is not a packaged app"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_started_when_the_shell_refuses() {
        use std::path::PathBuf;
        let exe = || PathBuf::from(r"C:\Program Files\Notepad++\notepad++.exe");
        let id = r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe";
        // the entry's shortcut wins over its target: the start folder, the show state and the identity come with it
        assert_eq!(direct_way(id, Some(exe()), Some("-multiInst".into()), Some(PathBuf::from(r"C:\x\Notepad++.lnk"))), Direct::Shortcut(PathBuf::from(r"C:\x\Notepad++.lnk")));
        // no shortcut found: the program, with the arguments the entry carries (a shortcut such as "pythonw.exe + a script")
        assert_eq!(direct_way(id, Some(exe()), Some("-multiInst".into()), None), Direct::File { path: exe(), args: "-multiInst".into() });
        assert_eq!(direct_way(id, Some(exe()), None, None), Direct::File { path: exe(), args: String::new() });
        // an empty shortcut path is no shortcut
        assert_eq!(direct_way(id, Some(exe()), None, Some(PathBuf::new())), Direct::File { path: exe(), args: String::new() });
        // a file the entry names wins over an id that looks packaged
        assert_eq!(direct_way("Vendor.App!Main", Some(PathBuf::from(r"C:\x\a.exe")), None, None), Direct::File { path: PathBuf::from(r"C:\x\a.exe"), args: String::new() });
        // a packaged app has no file: its activation
        assert_eq!(direct_way("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", None, None, None), Direct::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        assert_eq!(direct_way("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", Some(PathBuf::new()), None, None), Direct::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        // neither: nothing is guessed
        assert_eq!(direct_way("Some.Classic.Id", None, None, None), Direct::Nothing);
    }

    #[test]
    fn documents_are_not_apps() {
        assert!(is_document(r"C:\Program Files\X\readme.txt"));
        assert!(is_document("https://example.com"));
        assert!(!is_document("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(!is_document(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe"));
        assert!(is_uninstaller("Uninstall Notepad++"));
        assert!(is_uninstaller("WinRAR'ı kaldır"));
        assert!(!is_uninstaller("Notepad++"));
    }

    /// Lists the apps of this PC (needs a desktop session; Explorer, Settings or Notepad is in
    /// every Windows install).
    #[test]
    fn lists_installed_apps() {
        let t = std::time::Instant::now();
        let apps = list().expect("apps folder");
        eprintln!("{} apps in {} ms", apps.len(), t.elapsed().as_millis());
        assert!(apps.len() > 5, "{apps:?}");
        assert!(apps.iter().all(|a| !a.id.is_empty() && !a.name.is_empty()));
    }
}

