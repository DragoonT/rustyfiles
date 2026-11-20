//! Which windows are asking for attention: the dock puts a dot on their app's icon until they are switched to.
//!
//! WHY (on the rustyfiles Discord: "it's hard to see when I get a notification. I'd love it if (1) or a dot could appear over the
//! application itself, so if I get a DM from discord I can see it"). A program that wants the user (Discord on a direct message, a
//! chat, a finished download) calls `FlashWindowEx`, and Windows' taskbar lights the program's button until the window is used. The
//! dock did not hear it.
//!
//! HOW. Windows tells every window registered with `RegisterShellHookWindow` (the message `RegisterWindowMessage("SHELLHOOK")`) about
//! it: `wParam = HSHELL_FLASH` with the window in `lParam`, and `HSHELL_WINDOWACTIVATED` / `HSHELL_RUDEAPPACTIVATED` when a window
//! comes to the front, `HSHELL_WINDOWDESTROYED` when it is gone. This works beside Explorer (it is told the same; nothing is taken from
//! it). A hidden window on a thread of its own listens for the life of the process and keeps the set of windows that flashed and have
//! not been brought forward since.
//!
//! NOT HEARD: the number badge Discord sets on its taskbar button (`ITaskbarList3::SetOverlayIcon`) goes to Explorer's taskbar alone.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

const HSHELL_WINDOWDESTROYED: usize = 2;
const HSHELL_WINDOWACTIVATED: usize = 4;
const HSHELL_RUDEAPPACTIVATED: usize = 0x8004;
const HSHELL_FLASH: usize = 0x8006;

static WANTING: Mutex<Option<HashSet<isize>>> = Mutex::new(None);
static STARTED: OnceLock<()> = OnceLock::new();
static NOTIFY: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// What one shell message does to the set of windows that want attention; `true` when the set changed. `foreground` is the window in
/// front when the message came: a window that flashes while it is in front is already seen.
pub fn apply(set: &mut HashSet<isize>, code: usize, hwnd: isize, foreground: isize) -> bool {
    match code {
        HSHELL_FLASH if hwnd != 0 && hwnd != foreground => set.insert(hwnd),
        HSHELL_WINDOWACTIVATED | HSHELL_RUDEAPPACTIVATED | HSHELL_WINDOWDESTROYED => set.remove(&hwnd),
        _ => false,
    }
}

/// Does window `hwnd` want attention?
pub fn wants(hwnd: isize) -> bool {
    WANTING.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|s| s.contains(&hwnd))
}

/// Starts listening (once per process; later calls do nothing). `changed` is called on the listener's thread whenever a window starts
/// or stops wanting attention.
pub fn start(changed: impl Fn() + Send + Sync + 'static) {
    if STARTED.set(()).is_err() {
        return;
    }
    let _ = NOTIFY.set(Box::new(changed));
    if let Err(e) = std::thread::Builder::new().name("rf-dock-attention".into()).spawn(run) {
        log::warn!("dock: cannot listen for windows that ask for attention: {e}");
    }
}

fn message_id() -> u32 {
    static ID: OnceLock<u32> = OnceLock::new();
    *ID.get_or_init(|| unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("SHELLHOOK")) })
}

fn run() {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW, RegisterShellHookWindow, TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::w;
    let _ = message_id();
    unsafe {
        let Ok(hinst) = GetModuleHandleW(None) else { return };
        let class = w!("RustyfilesDockAttention");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassExW(&wc);
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), None) {
            Ok(h) => h,
            Err(e) => {
                log::warn!("dock: the attention window: {e}");
                return;
            }
        };
        if !RegisterShellHookWindow(hwnd).as_bool() {
            log::warn!("dock: RegisterShellHookWindow failed; no dots for windows that ask for attention");
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, GetForegroundWindow};
    if msg != message_id() {
        return unsafe { DefWindowProcW(hwnd, msg, wp, lp) };
    }
    let front = unsafe { GetForegroundWindow() }.0 as isize;
    let changed = {
        let mut guard = WANTING.lock().unwrap_or_else(|e| e.into_inner());
        apply(guard.get_or_insert_with(HashSet::new), wp.0, lp.0, front)
    };
    if changed {
        if wp.0 == HSHELL_FLASH {
            log::debug!("dock: window {:#x} asks for attention", lp.0);
        }
        if let Some(f) = NOTIFY.get() {
            f();
        }
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_wants_attention_from_its_flash_until_it_comes_to_the_front_or_goes() {
        let mut s = HashSet::new();
        assert!(apply(&mut s, HSHELL_FLASH, 10, 99), "a window in the back flashes");
        assert!(!apply(&mut s, HSHELL_FLASH, 10, 99), "it flashes again: nothing new");
        assert!(!apply(&mut s, HSHELL_FLASH, 20, 20), "one that flashes while in front is already seen");
        assert!(!apply(&mut s, 6, 10, 99), "a plain redraw changes nothing");
        assert!(s.contains(&10) && !s.contains(&20));
        assert!(!apply(&mut s, HSHELL_WINDOWACTIVATED, 30, 30), "another window to the front");
        assert!(apply(&mut s, HSHELL_RUDEAPPACTIVATED, 10, 10), "the user switched to it");
        assert!(s.is_empty());
        apply(&mut s, HSHELL_FLASH, 11, 99);
        assert!(apply(&mut s, HSHELL_WINDOWDESTROYED, 11, 99), "a closed window wants nothing");
        assert!(!apply(&mut s, HSHELL_FLASH, 0, 99), "no window, no dot");
    }
}
