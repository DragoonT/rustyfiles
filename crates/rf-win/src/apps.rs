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

/// What is tried for an entry: the file behind it, or the packaged app's activation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Direct {
    File(std::path::PathBuf),
    Packaged(String),
    Nothing,
}

/// The pure choice: a file the entry names wins (a program, a shortcut); an id of the form `family!app` is a packaged app.
pub fn direct_way(id: &str, target: Option<std::path::PathBuf>) -> Direct {
    match target {
        Some(t) if !t.as_os_str().is_empty() => Direct::File(t),
        _ if id.contains('!') => Direct::Packaged(id.to_string()),
        _ => Direct::Nothing,
    }
}

/// For the probe example: the direct way alone.
#[doc(hidden)]
pub fn launch_directly_for_probe(app: &App) -> anyhow::Result<()> {
    launch_directly(app)
}

fn launch_directly(app: &App) -> anyhow::Result<()> {
    match direct_way(&app.id, crate::dock::startapps::target_of(&app.parsing())) {
        Direct::File(path) => crate::dock::launch::start(&crate::dock::model::Launch::Link(path)),
        Direct::Packaged(id) => activate_packaged(&id),
        Direct::Nothing => anyhow::bail!("the entry names no file and is not a packaged app"),
    }
}

/// How long the activation of a packaged app is waited for. With no Explorer the call starts the app and then does not come back
/// (measured: the app's window was up, the call still out after 40 s), so the answer cannot be waited for: an error within this
/// time is an error, silence is taken as started.
const ACTIVATION_WAIT: std::time::Duration = std::time::Duration::from_millis(600);

/// What the wait for the activation's answer means.
pub fn activation_outcome(answer: Option<Result<(), String>>) -> Result<(), String> {
    match answer {
        Some(r) => r,
        // no answer in time: the app is starting (or started) and the call is still out
        None => Ok(()),
    }
}

/// A packaged app by its id, without the shell's folder: the activation manager, as a local server first and in our process second.
/// On a thread of its own, never the shared shell thread: the call can stay out for as long as there is no Explorer.
fn activate_packaged(aumid: &str) -> anyhow::Result<()> {
    let id_text = aumid.to_string();
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new().name("rf-activate-app".into()).spawn(move || {
        use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
        use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
        ensure_com();
        let id = wide(&id_text);
        let mut last = String::new();
        for ctx in [CLSCTX_LOCAL_SERVER, CLSCTX_INPROC_SERVER] {
            let done = unsafe {
                CoCreateInstance::<_, IApplicationActivationManager>(&ApplicationActivationManager, None, ctx).and_then(|m| m.ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR::null(), AO_NONE))
            };
            match done {
                Ok(_) => {
                    let _ = tx.send(Ok(()));
                    return;
                }
                Err(e) => last = e.to_string(),
            }
        }
        let _ = tx.send(Err(format!("activation: {last}")));
    })?;
    let answer = rx.recv_timeout(ACTIVATION_WAIT).ok();
    if answer.is_none() {
        log::info!("apps: the activation of {aumid} has not answered in {ACTIVATION_WAIT:?}; taken as started");
    }
    activation_outcome(answer).map_err(|e| anyhow::anyhow!(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_started_when_the_shell_refuses() {
        use std::path::PathBuf;
        // a program or a shortcut behind the entry is started as a file, whatever the id looks like
        assert_eq!(direct_way(r"{6D809377-6AF0-444B-8957-A3773F02200E}\Notepad++\notepad++.exe", Some(PathBuf::from(r"C:\Program Files\Notepad++\notepad++.exe"))), Direct::File(PathBuf::from(r"C:\Program Files\Notepad++\notepad++.exe")));
        assert_eq!(direct_way("Vendor.App!Main", Some(PathBuf::from(r"C:\x\a.lnk"))), Direct::File(PathBuf::from(r"C:\x\a.lnk")));
        // a packaged app has no file: its activation
        assert_eq!(direct_way("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", None), Direct::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        assert_eq!(direct_way("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", Some(PathBuf::new())), Direct::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        // neither: nothing is guessed
        assert_eq!(direct_way("Some.Classic.Id", None), Direct::Nothing);
    }

    #[test]
    fn an_activation_that_does_not_answer_is_taken_as_started_and_an_error_in_time_is_an_error() {
        assert_eq!(activation_outcome(Some(Ok(()))), Ok(()));
        assert_eq!(activation_outcome(Some(Err("activation: no such app".into()))), Err("activation: no such app".into()));
        assert_eq!(activation_outcome(None), Ok(()), "with no Explorer the call starts the app and stays out");
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

