//! The window that hears the media and volume keys nobody answered (`HSHELL_APPCOMMAND`), for a host that runs with no Explorer.
//!
//! IT EXISTS ONLY WHILE NO ONE OWNS THE SHELL WINDOW. The caller (`rf-app` `shell/live.rs`) starts a [`ShellHook`] when the shell window
//! goes away (or is not there at the host's start) and drops it when Explorer is back. Beside a running Explorer nothing is registered,
//! no thread exists and the keys are Explorer's alone. [`super::appcmd::decide`] is the second lock: every key looks at the shell
//! window again before it does anything.
//!
//! A hidden top-level pop-up on a thread of its own is registered with `RegisterShellHookWindow` (the documentation: "Registers a
//! specified Shell window to receive certain messages"; the message id is `RegisterWindowMessage("SHELLHOOK")` and `wParam` is the hook
//! code). The window procedure does nothing but look at `wParam == HSHELL_APPCOMMAND` and hand the command to a worker thread: the
//! work (WinRT, Core Audio) blocks and must not hold a window procedure. Dropping the hook deregisters the window and ends both threads.
//!
//! THE TASK MANAGER WINDOW. The same thread holds a second hidden window and registers it with `SetTaskmanWindow` (user32, not in the
//! documentation; Explorer registers its taskbar this way). WHY: Windows hides a minimized window only when the metrics say
//! `ARW_HIDE` (`minimized.rs`) AND such a window is registered; with the metrics alone every minimized window stayed a small title bar
//! at the bottom left (the user, 2026-11-23: "uygulamaları küçültünce sol alta gidiyor"). MEASURED on that PC with no Explorer and
//! `SM_ARRANGE` = 8: a window of our own minimized to (320, 1412); after `SetTaskmanWindow` on a hidden window, to (-32000, -32000);
//! when that window was destroyed the registration was gone by itself. It is a window of its own, not the hook window, so that a
//! shell message is not delivered twice to the window that acts on the keys. Nothing is registered when somebody else has one.
//!
//! NOT RUN in the increment that wrote it: the registration changes what Windows delivers to this process only, but it only does
//! anything useful without Explorer, which the test copy never has.

use std::sync::mpsc::{Sender, channel};
use std::time::{Duration, Instant};

use super::appcmd::{self, Executor, SystemOutputs};

/// `HSHELL_APPCOMMAND` (winuser.h).
pub const HSHELL_APPCOMMAND: usize = 12;

struct State {
    tx: Sender<u16>,
    message: u32,
}

/// `HSHELL_GETMINRECT` (winuser.h): "a window is being minimized or maximized. The system needs the coordinates of the minimized
/// rectangle for the window." `lParam` points to a `SHELLHOOKINFO` (the window and a rectangle to fill in).
pub const HSHELL_GETMINRECT: usize = 5;

/// Where each window goes when it is minimized: the place of its app's icon, per island (`island` is whatever its owner tells
/// them apart by; each monitor's dock has one). Screen coordinates, device px: (window, [left, top, right, bottom]).
///
/// WHY: with no Explorer the minimize animation flies to wherever the shell says the window's button is. Once this host hid the
/// minimized windows (the task manager window below), nobody answered that question any more and Windows minimized with no
/// animation at all (the user, 2026-11-24: "küçültme animasyonu çalışmıyor").
static MIN_TARGETS: std::sync::Mutex<Vec<(u32, Vec<(isize, [i32; 4])>)>> = std::sync::Mutex::new(Vec::new());

/// The island `island` says where its windows' icons are now (an empty list: it has none, or it is gone).
pub fn set_min_targets(island: u32, targets: Vec<(isize, [i32; 4])>) {
    let mut all = MIN_TARGETS.lock().unwrap_or_else(|p| p.into_inner());
    all.retain(|(i, _)| *i != island);
    if !targets.is_empty() {
        all.push((island, targets));
    }
}

/// The icon a window minimizes to: of the islands that show it, the one for which `on_windows_monitor` holds (the island on the
/// monitor the window is on), else the first. `None`: no island shows this window.
pub fn min_target(all: &[(u32, Vec<(isize, [i32; 4])>)], hwnd: isize, on_windows_monitor: &dyn Fn(&[i32; 4]) -> bool) -> Option<[i32; 4]> {
    let mut found = all.iter().filter_map(|(_, list)| list.iter().find(|(h, _)| *h == hwnd).map(|(_, r)| *r));
    let first = found.next()?;
    if on_windows_monitor(&first) {
        return Some(first);
    }
    Some(found.find(|r| on_windows_monitor(r)).unwrap_or(first))
}

/// Answers `HSHELL_GETMINRECT`: fills the rectangle in and says so. `false`: not a window of ours to place (Windows does as it
/// would alone).
unsafe fn answer_min_rect(lp: isize) -> bool {
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::SHELLHOOKINFO;
    if lp == 0 {
        return false;
    }
    unsafe {
        let info = &mut *(lp as *mut SHELLHOOKINFO);
        let mon = MonitorFromWindow(info.hwnd, MONITOR_DEFAULTTONEAREST);
        let same = |r: &[i32; 4]| MonitorFromPoint(POINT { x: (r[0] + r[2]) / 2, y: (r[1] + r[3]) / 2 }, MONITOR_DEFAULTTONEAREST) == mon;
        let all = MIN_TARGETS.lock().unwrap_or_else(|p| p.into_inner());
        match min_target(&all, info.hwnd.0 as isize, &same) {
            Some(r) => {
                info.rc = RECT { left: r[0], top: r[1], right: r[2], bottom: r[3] };
                log::info!("shell hook: window {:#x} minimizes to {r:?}", info.hwnd.0 as isize);
                true
            }
            None => false,
        }
    }
}

/// The hook window and its threads. Dropping it stops them.
pub struct ShellHook {
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn shell_hook_message() -> u32 {
    static ID: std::sync::OnceLock<u32> = std::sync::OnceLock::new();
    *ID.get_or_init(|| unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterWindowMessageW(windows::core::w!("SHELLHOOK")) })
}

impl ShellHook {
    /// Starts the window, registers it and starts the worker. Fails (and leaves nothing behind) when the window or the registration
    /// could not be made.
    pub fn start() -> anyhow::Result<ShellHook> {
        let (cmd_tx, cmd_rx) = channel::<u16>();
        let (ready_tx, ready_rx) = channel::<Result<isize, String>>();
        let thread = std::thread::Builder::new().name("rf-shellhook".into()).spawn(move || run(cmd_tx, ready_tx))?;
        let hwnd = match ready_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(h)) => h,
            Ok(Err(e)) => anyhow::bail!(e),
            Err(e) => anyhow::bail!("the shell hook window did not start: {e}"),
        };
        // From here on the value exists, so that a failure below drops it (and `Drop` closes the registered window and ends its thread)
        // instead of leaving a window behind that is registered as a shell hook for the rest of the process.
        let hook = ShellHook { hwnd, thread: Some(thread) };
        // The worker: one key after another. The shell is looked at again for each key, here and not in the window procedure (opening a
        // process to read its name blocks a little).
        std::thread::Builder::new().name("rf-shellhook-keys".into()).spawn(move || {
            let mut ex = Executor::new();
            let mut out = SystemOutputs;
            while let Ok(code) = cmd_rx.recv() {
                let shell = appcmd::shell_now();
                let d = appcmd::act(code, shell, &mut ex, Instant::now(), &mut out);
                log::debug!("shell hook: command {code}, shell {shell:?}: {d:?}");
            }
        })?;
        Ok(hook)
    }
}

impl Drop for ShellHook {
    fn drop(&mut self) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        unsafe {
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        if let Some(t) = self.thread.take()
            && t.thread().id() != std::thread::current().id()
        {
            let _ = t.join();
        }
    }
}

fn run(tx: Sender<u16>, ready: Sender<Result<isize, String>>) {
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, MSG, RegisterClassExW, RegisterShellHookWindow, TranslateMessage, WNDCLASSEXW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP,
    };
    use windows::core::w;
    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                let _ = ready.send(Err(format!("module handle: {e}")));
                return;
            }
        };
        let class = w!("RustyfilesShellHook");
        let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
        RegisterClassExW(&wc);
        let state = Box::into_raw(Box::new(State { tx, message: shell_hook_message() }));
        let hwnd = match CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst.into()), Some(state as *const _)) {
            Ok(h) => h,
            Err(e) => {
                drop(Box::from_raw(state));
                let _ = ready.send(Err(format!("the shell hook window: {e}")));
                return;
            }
        };
        if !RegisterShellHookWindow(hwnd).as_bool() {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            let _ = ready.send(Err("RegisterShellHookWindow failed".into()));
            return;
        }
        let taskman = register_taskman(hinst.into());
        if ready.send(Ok(hwnd.0 as isize)).is_err() {
            // `start` gave up waiting (its 5 s ran out) and nobody owns this window: take it back
            let _ = windows::Win32::UI::WindowsAndMessaging::DeregisterShellHookWindow(hwnd);
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            return;
        }
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // (its registration goes with it: Windows forgets a task manager window that is destroyed)
        if let Some(t) = taskman {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(t);
        }
    }
}

type SetTaskman = unsafe extern "system" fn(windows::Win32::Foundation::HWND) -> windows::core::BOOL;
type GetTaskman = unsafe extern "system" fn() -> windows::Win32::Foundation::HWND;

/// `SetTaskmanWindow` and `GetTaskmanWindow` of user32, found by name: they are exported but not in the import library (the linker
/// does not know them). `None` on a Windows that has none.
fn taskman_calls() -> Option<(SetTaskman, GetTaskman)> {
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::core::{s, w};
    // SAFETY: user32 is loaded in every process with a window; the two functions have had these signatures since Windows 2000.
    unsafe {
        let user32 = GetModuleHandleW(w!("user32.dll")).ok()?;
        let set = GetProcAddress(user32, s!("SetTaskmanWindow"))?;
        let get = GetProcAddress(user32, s!("GetTaskmanWindow"))?;
        Some((std::mem::transmute::<unsafe extern "system" fn() -> isize, SetTaskman>(set), std::mem::transmute::<unsafe extern "system" fn() -> isize, GetTaskman>(get)))
    }
}

/// Makes a hidden window on this thread and registers it as the session's task manager window, unless somebody has one (see the
/// module). `None`: nothing was registered. The window is the caller's to destroy.
unsafe fn register_taskman(hinst: windows::Win32::Foundation::HINSTANCE) -> Option<windows::Win32::Foundation::HWND> {
    use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_POPUP};
    use windows::core::w;
    let (set_taskman, get_taskman) = taskman_calls()?;
    unsafe {
        if !get_taskman().0.is_null() {
            return None;
        }
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("STATIC"), w!(""), WS_POPUP, 0, 0, 0, 0, None, None, Some(hinst), None).ok()?;
        if set_taskman(hwnd).as_bool() {
            log::info!("shell host: no task manager window - ours is registered, so that minimized windows are hidden");
            Some(hwnd)
        } else {
            log::warn!("shell host: the task manager window could not be registered; minimized windows stay as title bars");
            let _ = DestroyWindow(hwnd);
            None
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: windows::Win32::Foundation::HWND, msg: u32, wp: windows::Win32::Foundation::WPARAM, lp: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Foundation::LRESULT;
    use windows::Win32::UI::WindowsAndMessaging::{
        CREATESTRUCTW, DefWindowProcW, DeregisterShellHookWindow, DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, PostQuitMessage, SetWindowLongPtrW, WM_CLOSE, WM_DESTROY, WM_NCCREATE,
    };
    unsafe {
        if msg == WM_NCCREATE {
            let cs = &*(lp.0 as *const CREATESTRUCTW);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        match msg {
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                let _ = DeregisterShellHookWindow(hwnd);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                PostQuitMessage(0);
                LRESULT(0)
            }
            m if m == (*ptr).message => {
                if wp.0 == HSHELL_APPCOMMAND {
                    let _ = (*ptr).tx.send(appcmd::command_of_lparam(lp.0));
                }
                if wp.0 == HSHELL_GETMINRECT && answer_min_rect(lp.0) {
                    return LRESULT(1);
                }
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

#[cfg(test)]
mod min_target_tests {
    use super::min_target;

    #[test]
    fn a_window_minimizes_to_its_icon_on_its_own_monitor() {
        let home = [900, 1366, 960, 1448];
        let second = [2900, 1868, 2960, 1950];
        let all = vec![(0u32, vec![(11isize, home), (12, [960, 1366, 1020, 1448])]), (1u32, vec![(11isize, second)])];
        let on_second = |r: &[i32; 4]| r[0] >= 2560;
        let on_home = |r: &[i32; 4]| r[0] < 2560;
        assert_eq!(min_target(&all, 11, &on_home), Some(home));
        assert_eq!(min_target(&all, 11, &on_second), Some(second));
        // shown by one island only: that one, wherever the window is
        assert_eq!(min_target(&all, 12, &on_second), Some([960, 1366, 1020, 1448]));
        // a window no island shows is not ours to place
        assert_eq!(min_target(&all, 99, &on_home), None);
        assert_eq!(min_target(&[], 11, &on_home), None);
    }
}
