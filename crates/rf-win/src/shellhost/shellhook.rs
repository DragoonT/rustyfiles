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
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}
