//! QuickLook (QL-Win) as the Space previewer instead of DirectLook: Settings › DirectLook › "Space opens QuickLook" (asked on
//! Discord). QuickLook watches Explorer's windows only, so rustyfiles hands it the file itself.
//!
//! QuickLook listens on the pipe `\\.\pipe\QuickLook.App.Pipe.<the user's SID>` for one line per connection:
//! `Message|path|options`. `Toggle` opens a preview of the file, or closes the preview when it already shows that file; `Switch`
//! moves an open preview to another file and does nothing while none is open (so it can be sent whenever the selection moves).
//! When the pipe is not there, QuickLook is started (the Microsoft Store package or a desktop install) and the message waits for it.
//!
//! Everything here blocks (pipe, process start): the callers run it on a worker thread.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// The Microsoft Store package of QuickLook (`family!app`).
const STORE_AUMID: &str = "21090PaddyXu.QuickLook_egxr34yet59cg!Main";
const TOGGLE: &str = "QuickLook.App.PipeMessages.Toggle";
const SWITCH: &str = "QuickLook.App.PipeMessages.Switch";
/// How long a just-started QuickLook gets to open its pipe.
const START_WAIT: Duration = Duration::from_secs(6);

fn pipe() -> Option<&'static str> {
    static PIPE: OnceLock<Option<String>> = OnceLock::new();
    PIPE.get_or_init(|| rf_win::dock::startuser::current_sid().map(|sid| format!(r"\\.\pipe\QuickLook.App.Pipe.{sid}"))).as_deref()
}

/// The line QuickLook reads: the message, the path, no options.
fn line(message: &str, path: &Path) -> String {
    format!("{message}|{}|\r\n", path.display())
}

/// One message on QuickLook's pipe; false when nobody listens. The server takes one connection at a time and reopens between them,
/// so a busy pipe is tried again for a moment.
fn send(message: &str, path: &Path) -> bool {
    let Some(pipe) = pipe() else { return false };
    let until = Instant::now() + Duration::from_millis(400);
    loop {
        match std::fs::OpenOptions::new().write(true).open(pipe) {
            Ok(mut f) => return f.write_all(line(message, path).as_bytes()).and_then(|_| f.flush()).is_ok(),
            // 231 = ERROR_PIPE_BUSY: between two connections
            Err(e) if e.raw_os_error() == Some(231) && Instant::now() < until => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => return false,
        }
    }
}

/// A desktop install's QuickLook.exe, if there is one.
fn desktop_exe() -> Option<PathBuf> {
    let mut places = Vec::new();
    if let Some(d) = std::env::var_os("LOCALAPPDATA") {
        places.push(PathBuf::from(d).join(r"Programs\QuickLook\QuickLook.exe"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(d) = std::env::var_os(var) {
            places.push(PathBuf::from(d).join(r"QuickLook\QuickLook.exe"));
        }
    }
    places.into_iter().find(|p| p.is_file())
}

/// Starts QuickLook; false when it is not installed (or would not start). The Store package is started the way the Start menu
/// does it, through Explorer: started by the activation manager (`rf_win::activate`) it ran but never opened its pipe (tried
/// 2026-11-19 with 4.5.0; through Explorer the pipe was there within a second).
fn start() -> bool {
    if let Some(exe) = desktop_exe() {
        return std::process::Command::new(exe).spawn().is_ok();
    }
    store_installed() && std::process::Command::new("explorer.exe").arg(format!(r"shell:AppsFolder\{STORE_AUMID}")).spawn().is_ok()
}

/// The Store package is installed for this user (its key in the package repository). Asking Explorer for a package that is not
/// there would show the user an error window.
fn store_installed() -> bool {
    let family = STORE_AUMID.split('!').next().unwrap_or_default();
    let (name, publisher) = family.split_once('_').unwrap_or((family, ""));
    let Ok(packages) = windows_registry::CURRENT_USER.open(r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages")
    else {
        return false;
    };
    packages.keys().is_ok_and(|mut keys| keys.any(|k| k.starts_with(name) && k.ends_with(publisher)))
}

/// Space: QuickLook opens `path`, or closes the preview of it. Starts QuickLook when it is not running; false when it could not be
/// reached at all (not installed).
pub fn toggle(path: &Path) -> bool {
    if send(TOGGLE, path) {
        return true;
    }
    if !start() {
        log::warn!("quicklook: not running and could not be started");
        return false;
    }
    let until = Instant::now() + START_WAIT;
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(150));
        if send(TOGGLE, path) {
            return true;
        }
    }
    log::warn!("quicklook: started, but its pipe did not open");
    false
}

/// The selection moved: an open QuickLook preview follows it (nothing happens while it is closed or QuickLook is not running).
pub fn follow(path: &Path) {
    let _ = send(SWITCH, path);
}

static USED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static FOLLOWED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Space in the list with QuickLook chosen: on a worker, as everything here blocks.
pub fn space(path: PathBuf) {
    *FOLLOWED.lock().unwrap_or_else(|e| e.into_inner()) = Some(path.clone());
    std::thread::spawn(move || {
        let ok = toggle(&path);
        log::info!("quicklook: Space on {} -> {}", path.display(), if ok { "sent" } else { "not reached" });
        if ok {
            USED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    });
}

/// The focused item of the active list (each frame): once QuickLook has been used, a new item is sent to it with `Switch`.
pub fn selection_is(path: &Path) {
    if !USED.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut last = FOLLOWED.lock().unwrap_or_else(|e| e.into_inner());
    if last.as_deref() == Some(path) {
        return;
    }
    *last = Some(path.to_path_buf());
    let path = path.to_path_buf();
    std::thread::spawn(move || follow(&path));
}

/// Whether QuickLook is installed (for the setting's description).
pub fn installed() -> bool {
    static FOUND: OnceLock<bool> = OnceLock::new();
    *FOUND.get_or_init(|| desktop_exe().is_some() || store_installed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_message_path_and_empty_options() {
        assert_eq!(line(TOGGLE, Path::new(r"C:\A folder\photo.jpg")), "QuickLook.App.PipeMessages.Toggle|C:\\A folder\\photo.jpg|\r\n");
    }

    /// Manual: `RF_QL_PATH=<file> cargo test -p rf-app --lib quicklook::tests::manual_toggle -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn manual_toggle() {
        let p = PathBuf::from(std::env::var("RF_QL_PATH").unwrap());
        println!("pipe {:?}", pipe());
        println!("send -> {}", send(TOGGLE, &p));
    }

    #[test]
    fn looking_for_the_store_package_does_not_fail() {
        // (installed on the developer's PC, maybe not on a build machine: only that the registry walk answers)
        let _ = store_installed();
    }

    #[test]
    fn the_pipe_is_named_after_the_user() {
        let p = pipe().expect("the user's SID");
        assert!(p.starts_with(r"\\.\pipe\QuickLook.App.Pipe.S-1-5-"), "{p}");
    }
}
