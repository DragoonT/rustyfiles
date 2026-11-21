//! Explorer windows that other programs open, taken over: the window is read (its folder and the selected item), closed, and the
//! same place is opened in rustyfiles. An option, off by default.
//!
//! WHY (issue 37: download managers such as IDM and Mister Download Manager opened File Explorer for "Open containing folder"
//! although rustyfiles is the default file manager). Such a program starts `explorer.exe /select,"<file>"` by name, so nothing
//! that decides the default is asked; the only thing rustyfiles can do is notice the window that appears.
//!
//! HOW. A WinEvent hook (out of context, no code in Explorer) reports every window shown; a top-level `CabinetWClass` window not
//! seen before is looked at a moment later (its view has to be there), through the shell's list of its windows. A window whose
//! folder is no file-system folder (This PC, Control Panel) is left alone, and so is every window shown while rustyfiles itself
//! is opening Explorer ([`quiet_for`]).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EVENT_OBJECT_SHOW, GA_ROOT, GetAncestor, GetMessageW, KillTimer, MSG, OBJID_WINDOW, PostMessageW, SetTimer,
    TranslateMessage, WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_CLOSE, WM_TIMER,
};

/// Where an Explorer window was: its folder, and the item selected in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caught {
    pub folder: PathBuf,
    pub selected: Option<PathBuf>,
}

static ON: AtomicBool = AtomicBool::new(false);
static STARTED: AtomicBool = AtomicBool::new(false);
/// Until when (milliseconds since 1970) a new Explorer window is left alone: rustyfiles is opening one itself.
static QUIET_UNTIL: AtomicU64 = AtomicU64::new(0);
static HANDLER: Mutex<Option<Box<dyn Fn(Caught) + Send>>> = Mutex::new(None);

/// How long after it appears a window is read (Explorer fills its view after showing the frame).
const SETTLE: Duration = Duration::from_millis(600);
/// How long a window that is not ready yet is tried again.
const GIVE_UP: Duration = Duration::from_secs(4);

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Leaves the Explorer windows that appear in the next `d` alone: rustyfiles is opening Explorer itself (a place it hands back,
/// "Open in File Explorer").
pub fn quiet_for(d: Duration) {
    QUIET_UNTIL.store(now_ms() + d.as_millis() as u64, Ordering::Relaxed);
}

/// Switches the taking over on or off. The first switch on starts the watching thread; `found` is called (on that thread) for
/// every window taken over, after it was closed.
pub fn set(on: bool, found: impl Fn(Caught) + Send + 'static) {
    ON.store(on, Ordering::Relaxed);
    if !on {
        return;
    }
    if let Ok(mut h) = HANDLER.lock() {
        *h = Some(Box::new(found));
    }
    if STARTED.swap(true, Ordering::Relaxed) {
        return;
    }
    std::thread::Builder::new().name("explorer-catch".into()).spawn(run).ok();
}

thread_local! {
    /// Windows seen (shown at least once), and those waiting to be read: (window, first seen).
    static SEEN: std::cell::RefCell<HashSet<isize>> = std::cell::RefCell::new(HashSet::new());
    static WAITING: std::cell::RefCell<Vec<(isize, Instant)>> = const { std::cell::RefCell::new(Vec::new()) };
}

unsafe extern "system" fn on_show(_: HWINEVENTHOOK, _: u32, hwnd: HWND, id_object: i32, id_child: i32, _: u32, _: u32) {
    if id_object != OBJID_WINDOW.0 || id_child != 0 || hwnd.is_invalid() || !ON.load(Ordering::Relaxed) {
        return;
    }
    // top-level Explorer frames only
    if unsafe { GetAncestor(hwnd, GA_ROOT) } != hwnd || crate::look::target::class_of(hwnd) != "CabinetWClass" {
        return;
    }
    let key = hwnd.0 as isize;
    if !SEEN.with(|s| s.borrow_mut().insert(key)) {
        return;
    }
    if now_ms() < QUIET_UNTIL.load(Ordering::Relaxed) {
        return;
    }
    WAITING.with(|w| w.borrow_mut().push((key, Instant::now())));
}

fn run() {
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let hook = unsafe {
        SetWinEventHook(EVENT_OBJECT_SHOW, EVENT_OBJECT_SHOW, None, Some(on_show), 0, 0, WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS)
    };
    if hook.is_invalid() {
        log::warn!("explorer catch: the window hook could not be set");
        return;
    }
    // a look at the waiting windows five times a second
    let timer = unsafe { SetTimer(None, 0, 200, None) };
    let mut msg = MSG::default();
    while unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        if msg.message == WM_TIMER {
            look_at_waiting();
            continue;
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    let _ = unsafe { KillTimer(None, timer) };
}

fn look_at_waiting() {
    let due: Vec<(isize, Instant)> = WAITING.with(|w| w.borrow().iter().copied().filter(|(_, t)| t.elapsed() >= SETTLE).collect());
    for (key, first) in due {
        if !ON.load(Ordering::Relaxed) {
            WAITING.with(|w| w.borrow_mut().clear());
            return;
        }
        let hwnd = HWND(key as *mut _);
        let place = crate::look::target::explorer_place(key);
        let done = match place {
            Some(c) => {
                let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
                log::info!("explorer catch: took over {:?}", c.folder);
                if let Ok(h) = HANDLER.lock()
                    && let Some(f) = h.as_ref()
                {
                    f(c);
                }
                true
            }
            // not ready yet, or no file-system folder: tried again for a while, then left alone
            None => first.elapsed() >= GIVE_UP,
        };
        if done {
            WAITING.with(|w| w.borrow_mut().retain(|(k, _)| *k != key));
        }
    }
}

/// The command line that shows a caught place in rustyfiles: the selected item in its folder, or the folder.
pub fn args_for(c: &Caught) -> Vec<std::ffi::OsString> {
    match &c.selected {
        Some(item) if item.parent() == Some(c.folder.as_path()) => {
            let mut a = std::ffi::OsString::from("/select,");
            a.push(item.as_os_str());
            vec![a]
        }
        _ => vec![c.folder.clone().into_os_string()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_selected_item_of_the_folder_is_selected_and_anything_else_opens_the_folder() {
        let c = |f: &str, s: Option<&str>| Caught { folder: PathBuf::from(f), selected: s.map(PathBuf::from) };
        assert_eq!(args_for(&c(r"C:\Downloads", Some(r"C:\Downloads\a.zip"))), vec![std::ffi::OsString::from(r"/select,C:\Downloads\a.zip")]);
        assert_eq!(args_for(&c(r"C:\Downloads", None)), vec![std::ffi::OsString::from(r"C:\Downloads")]);
        // (a selection outside the folder, as a search's would be: the folder)
        assert_eq!(args_for(&c(r"C:\Downloads", Some(r"D:\x\a.zip"))), vec![std::ffi::OsString::from(r"C:\Downloads")]);
    }
}
