//! The real Windows side of the tray host. NOT RUN by any test or probe: starting it creates a window of class `Shell_TrayWnd`, sends
//! messages to Explorer's tray window and broadcasts `TaskbarCreated`. The only tests in this file read pictures through GDI from
//! icons they make themselves (no window, no message, nothing of the shell).
//!
//! What lives here, each behind a trait of `traits.rs` / `relay.rs`:
//! * [`Win32Finder`], [`Win32Sender`]: look up and send to Explorer's tray window (`SendMessageTimeout` with `SMTO_ABORTIFHUNG`);
//! * [`Win32Window`]: our `Shell_TrayWnd` (hidden unless asked) with its `TrayNotifyWnd` child; kept the first one a program finds with
//!   `SetWindowPos(HWND_TOPMOST)` on OUR window only (Explorer's is never moved, hidden or restyled); its rectangle, its child's
//!   rectangle and its window properties (`TaskbandHWND`: programs read it from the first `Shell_TrayWnd` to find the taskbar's
//!   button window) are copied from Explorer's;
//! * [`Win32Broadcast`]: `SendNotifyMessage(HWND_BROADCAST, TaskbarCreated)`;
//! * [`Win32Owners`]: `IsWindow`, `GetWindowThreadProcessId`, `AllowSetForegroundWindow`, `SendNotifyMessage` to an icon's owner;
//! * [`Win32Pixels`]: `GetIconInfo` + `GetDIBits`, with the arithmetic in `pixels.rs`;
//! * the host thread: window class, message loop, the window procedure.
//!
//! No COM is used anywhere in the tray host, so nothing here goes through `rf_win::sta`.
//!
//! THE WINDOW PROCEDURE. It can be entered again while it waits for Explorer (another program's `WM_COPYDATA` is delivered to a thread
//! inside `SendMessageTimeout`), so it keeps no `RefCell` borrowed across a call that can wait: the `Core` is behind an `Rc` that is
//! cloned out of its cell for the call, and the lifecycle's cell is borrowed only for the short window operations. A panic must not
//! cross the `extern "system"` boundary (that aborts the process): the whole procedure runs inside `catch_unwind`. That works in the
//! test and development profiles only; the release profile sets `panic = "abort"`, where `catch_unwind` is inert and a panic ends the
//! process (the window goes with it). What protects the messages there is the order in `core.rs` (Explorer has a message before any
//! code of ours that could panic runs).

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use crossbeam_channel::Receiver;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ, ReleaseDC};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::{
    AllowSetForegroundWindow, CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumPropsExW, EnumWindows, FindWindowExW, GWL_EXSTYLE,
    FindWindowW, GetClassNameW, GetMessageW, GetPropW, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HICON, HWND_BROADCAST, HWND_TOPMOST, ICONINFO, IsWindow, KillTimer,
    LWA_ALPHA, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, RegisterWindowMessageW, RemovePropW, SMTO_ABORTIFHUNG, SW_SHOWNOACTIVATE, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW, SendNotifyMessageW, SetLayeredWindowAttributes, SetPropW, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, UnregisterClassW, WM_CLOSE, WM_COPYDATA, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_ENDSESSION, WM_QUERYENDSESSION, WM_SETTINGCHANGE,
    WM_TIMER, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
    GetIconInfo, InSendMessage,
};
use windows::core::{BOOL, PCWSTR, PWSTR, w};

use super::core::{Core, CoreConfig};
use super::host::TrayHostConfig;
use super::handshake::Handshake;
use super::lifecycle::{Lifecycle, TICK_MS, props_to_remove, should_mirror_prop};
use super::model::IconBitmap;
use super::pixels;
use super::relay::{MsgSender, RelayCore, TrayFinder};
use super::shared::{Shared, bump};
use super::traits::{Broadcast, IconPixels, OwnerSend, Raw, SystemClock, TrayWindow};
use crate::tray_wire::forward::{Method, Outcome, SelfIdentity, WindowInfo, method_for};
use crate::tray_wire::TRAY_CLASS;

/// How long a message passed on may wait for Explorer. The same 4 s as shell32's own wait for the taskbar: a shorter limit would fail
/// calls that would have succeeded, and Explorer would still process the message afterwards (a program that tries again adds twice).
pub const FORWARD_TIMEOUT_MS: u32 = 4000;

const TIMER_TICK: usize = 1;

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

// ---------------------------------------------------------------- finding and sending to Explorer's tray window

fn exe_file_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        String::from_utf16_lossy(&buf[..n as usize]).rsplit(['\\', '/']).next().map(str::to_string)
    }
}

fn class_of(h: HWND) -> Option<String> {
    let mut buf = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut buf) };
    (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
}

pub struct Win32Finder;

impl TrayFinder for Win32Finder {
    fn tray_windows(&self) -> Vec<WindowInfo> {
        unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
            let out = unsafe { &mut *(l.0 as *mut Vec<WindowInfo>) };
            if let Some(class) = class_of(h)
                && class == TRAY_CLASS
            {
                let mut pid = 0u32;
                unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
                out.push(WindowInfo { hwnd: h.0 as isize, class, pid, exe: exe_file_name(pid) });
            }
            BOOL(1)
        }
        let mut out: Vec<WindowInfo> = Vec::new();
        unsafe {
            let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
        }
        out
    }

    fn is_tray_window(&self, hwnd: isize) -> bool {
        let h = hwnd_of(hwnd);
        unsafe { IsWindow(Some(h)).as_bool() }.then(|| class_of(h)).flatten().is_some_and(|c| c == TRAY_CLASS)
    }
}

/// The first `Shell_TrayWnd` in z-order, without asking who owns it: the very call programs make (`FindWindow` by class, which stops
/// at the first match inside the window manager instead of walking every top-level window and copying its class name; the tick asks
/// every 250 ms).
fn first_tray_hwnd() -> Option<isize> {
    let h = unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }.ok()?;
    Some(h.0 as isize)
}

pub struct Win32Sender {
    pub timeout_ms: u32,
}

impl MsgSender for Win32Sender {
    fn send(&self, target: isize, raw: Raw) -> Outcome {
        let h = hwnd_of(target);
        unsafe {
            if !IsWindow(Some(h)).as_bool() {
                return Outcome::TargetGone;
            }
            let mut result = 0usize;
            let r = SendMessageTimeoutW(h, raw.msg, WPARAM(raw.wparam), LPARAM(raw.lparam), SMTO_ABORTIFHUNG, self.timeout_ms, Some(&mut result));
            if r.0 != 0 {
                Outcome::Delivered(result as isize)
            } else if IsWindow(Some(h)).as_bool() {
                Outcome::TimedOut
            } else {
                Outcome::TargetGone
            }
        }
    }

    fn post(&self, target: isize, raw: Raw) -> bool {
        unsafe { PostMessageW(Some(hwnd_of(target)), raw.msg, WPARAM(raw.wparam), LPARAM(raw.lparam)) }.is_ok()
    }
}

// ---------------------------------------------------------------- the owner of an icon

pub struct Win32Owners;

impl OwnerSend for Win32Owners {
    fn is_window(&self, hwnd: isize) -> bool {
        unsafe { IsWindow(Some(hwnd_of(hwnd))).as_bool() }
    }

    fn owner_pid(&self, hwnd: isize) -> Option<u32> {
        let mut pid = 0u32;
        let tid = unsafe { GetWindowThreadProcessId(hwnd_of(hwnd), Some(&mut pid)) };
        (tid != 0 && pid != 0).then_some(pid)
    }

    fn owner_exe(&self, pid: u32) -> Option<String> {
        exe_file_name(pid)
    }

    fn allow_foreground(&self, pid: u32) {
        let _ = unsafe { AllowSetForegroundWindow(pid) };
    }

    fn send_notify(&self, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> bool {
        unsafe { SendNotifyMessageW(hwnd_of(hwnd), msg, WPARAM(wparam), LPARAM(lparam)) }.is_ok()
    }
}

// ---------------------------------------------------------------- the broadcast

pub struct Win32Broadcast {
    pub taskbar_created: u32,
}

impl Broadcast for Win32Broadcast {
    fn taskbar_created(&mut self) {
        if self.taskbar_created != 0 {
            unsafe {
                let _ = SendNotifyMessageW(HWND_BROADCAST, self.taskbar_created, WPARAM(0), LPARAM(0));
            }
        }
    }
}

// ---------------------------------------------------------------- pixels

pub struct Win32Pixels;

fn bitmap_size(h: HBITMAP) -> Option<(u32, u32)> {
    let mut bm = BITMAP::default();
    let n = unsafe { GetObjectW(HGDIOBJ(h.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) };
    (n != 0 && bm.bmWidth > 0 && bm.bmHeight > 0).then_some((bm.bmWidth as u32, bm.bmHeight as u32))
}

/// The pixels of a bitmap as 32-bit BGRA rows, top-down.
fn dib32(bmp: HBITMAP, w: u32, rows: u32) -> Option<Vec<u8>> {
    let mut data = vec![0u8; (w as usize).checked_mul(rows as usize)?.checked_mul(4)?];
    let mut bmi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: w as i32,
            biHeight: -(rows as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    unsafe {
        let dc = GetDC(None);
        let lines = GetDIBits(dc, bmp, 0, rows, Some(data.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
        ReleaseDC(None, dc);
        (lines as u32 == rows).then_some(data)
    }
}

fn read_icon(hicon: isize) -> Option<IconBitmap> {
    let icon = HICON(hicon as *mut c_void);
    let mut ii = ICONINFO::default();
    unsafe { GetIconInfo(icon, &mut ii) }.ok()?;
    let result = (|| {
        if !ii.hbmColor.is_invalid() {
            let (w, h) = bitmap_size(ii.hbmColor)?;
            if w > pixels::MAX_SOURCE_PX || h > pixels::MAX_SOURCE_PX {
                return None;
            }
            let color = dib32(ii.hbmColor, w, h)?;
            let mask = if ii.hbmMask.is_invalid() { None } else { dib32(ii.hbmMask, w, h) };
            let bgra = pixels::from_color_and_mask(&color, mask.as_deref(), w, h)?;
            pixels::to_icon_bitmap(bgra, w, h)
        } else {
            // a monochrome icon: one bitmap, twice as tall as the picture
            let (w, rows) = bitmap_size(ii.hbmMask)?;
            let h = rows / 2;
            if h == 0 || w > pixels::MAX_SOURCE_PX || h > pixels::MAX_SOURCE_PX {
                return None;
            }
            let mask = dib32(ii.hbmMask, w, h * 2)?;
            let bgra = pixels::from_monochrome(&mask, w, h)?;
            pixels::to_icon_bitmap(bgra, w, h)
        }
    })();
    unsafe {
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
        }
    }
    result
}

impl IconPixels for Win32Pixels {
    fn read(&self, hicon: isize) -> Option<IconBitmap> {
        if hicon == 0 {
            return None;
        }
        read_icon(hicon)
    }
}

// ---------------------------------------------------------------- our window

/// The string-named properties of Explorer's tray window (name with its NUL, and the handle-sized value). Which of them are copied
/// onto ours is `lifecycle::should_mirror_prop`.
fn explorer_props(explorer: HWND) -> Vec<(Vec<u16>, isize)> {
    unsafe extern "system" fn cb(_: HWND, name: PCWSTR, data: HANDLE, user: usize) -> BOOL {
        // (a property named by an atom has the atom in the low word of the pointer: skip those)
        if (name.0 as usize) <= 0xFFFF || name.0.is_null() {
            return BOOL(1);
        }
        let out = unsafe { &mut *(user as *mut Vec<(Vec<u16>, isize)>) };
        let mut v = Vec::new();
        let mut p = name.0;
        unsafe {
            while *p != 0 && v.len() < 256 {
                v.push(*p);
                p = p.add(1);
            }
        }
        v.push(0);
        out.push((v, data.0 as isize));
        BOOL(1)
    }
    let mut out: Vec<(Vec<u16>, isize)> = Vec::new();
    unsafe {
        EnumPropsExW(explorer, Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// What a read-only look at a tray window shows (for the windowless probe): no message is sent to it.
#[derive(Clone, Debug, Default)]
pub struct TrayWindowFacts {
    pub rect: [i32; 4],
    pub ex_style: u32,
    pub visible: bool,
    pub child_rect: Option<[i32; 4]>,
    /// Names of its string-named window properties (`TaskbandHWND`, ...), not their values.
    pub props: Vec<String>,
}

/// `GetWindowRect`, `GetWindowLongPtr`, `IsWindowVisible`, `FindWindowEx` and `EnumPropsEx` on a window: queries that read the window
/// manager's own data and send nothing to the window or its program.
pub fn read_only_facts(hwnd: isize) -> TrayWindowFacts {
    use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    let h = hwnd_of(hwnd);
    let mut f = TrayWindowFacts::default();
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_ok() {
            f.rect = [r.left, r.top, r.right, r.bottom];
        }
        f.ex_style = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        f.visible = IsWindowVisible(h).as_bool();
        if let Ok(c) = FindWindowExW(Some(h), None, w!("TrayNotifyWnd"), PCWSTR::null()) {
            let mut cr = RECT::default();
            if GetWindowRect(c, &mut cr).is_ok() {
                f.child_rect = Some([cr.left, cr.top, cr.right, cr.bottom]);
            }
        }
    }
    f.props = explorer_props(h).into_iter().map(|(name, _)| String::from_utf16_lossy(&name[..name.len().saturating_sub(1)])).collect();
    f
}

/// The id `RegisterWindowMessage("TaskbarCreated")` gives in this session (0 if it cannot be had). Registering a name that exists only
/// looks it up.
pub fn taskbar_created_id() -> u32 {
    unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) }
}

pub struct Win32Window {
    hwnd: Option<HWND>,
    child: Option<HWND>,
    visible: bool,
    /// Names of the properties set on our window (removed before it is destroyed).
    props: Vec<Vec<u16>>,
    class_registered: bool,
}

impl Win32Window {
    pub fn new(visible: bool) -> Win32Window {
        Win32Window { hwnd: None, child: None, visible, props: Vec::new(), class_registered: false }
    }

    /// Removes the properties, forgets the handle, destroys the window. Called by the window procedure for `WM_CLOSE` (so the handle
    /// is forgotten there and a later `destroy` finds nothing to destroy a second time on a dead, possibly reused, handle) and by
    /// `destroy`. Safe when there is no window.
    pub fn destroy_window(&mut self) {
        unsafe {
            if let Some(h) = self.hwnd.take() {
                // (a property set on a window is removed before the window goes, or its name's atom stays behind)
                for name in self.props.drain(..) {
                    let _ = RemovePropW(h, PCWSTR(name.as_ptr()));
                }
                // the child goes with its parent
                self.child = None;
                let _ = DestroyWindow(h);
            }
        }
    }
}

impl TrayWindow for Win32Window {
    fn create(&mut self) -> Result<(), String> {
        if self.hwnd.is_some() {
            return Ok(());
        }
        unsafe {
            let hinst = GetModuleHandleW(None).map_err(|e| format!("module handle: {e}"))?;
            let class = w!("Shell_TrayWnd");
            let wc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, style: CS_DBLCLKS, lpfnWndProc: Some(wndproc), hInstance: hinst.into(), lpszClassName: class, ..Default::default() };
            if RegisterClassExW(&wc) == 0 && windows::Win32::Foundation::GetLastError() != windows::Win32::Foundation::ERROR_CLASS_ALREADY_EXISTS {
                return Err(format!("RegisterClassExW: {}", windows::core::Error::from_thread()));
            }
            let child_class = w!("TrayNotifyWnd");
            let cc = WNDCLASSEXW { cbSize: size_of::<WNDCLASSEXW>() as u32, lpfnWndProc: Some(child_proc), hInstance: hinst.into(), lpszClassName: child_class, ..Default::default() };
            let _ = RegisterClassExW(&cc);
            self.class_registered = true;
            // 1x1 and hidden; `mirror` gives it Explorer's rectangle
            let hwnd = CreateWindowExW(WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, class, w!(""), WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS, 0, 0, 1, 1, None, None, Some(hinst.into()), None)
                .map_err(|e| format!("CreateWindowExW: {e}"))?;
            self.hwnd = Some(hwnd);
            self.child = CreateWindowExW(Default::default(), child_class, w!(""), WS_CHILD, 0, 0, 1, 1, Some(hwnd), None, Some(hinst.into()), None).ok();
            if self.visible {
                // click-through and almost invisible: the taskbar underneath keeps working
                let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as isize);
                let _ = SetLayeredWindowAttributes(hwnd, Default::default(), 1, LWA_ALPHA);
                let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
        }
        Ok(())
    }

    fn destroy(&mut self) {
        self.destroy_window();
        unsafe {
            if self.class_registered {
                self.class_registered = false;
                if let Ok(hinst) = GetModuleHandleW(None) {
                    let _ = UnregisterClassW(CHILD_CLASS_W, Some(hinst.into()));
                    let _ = UnregisterClassW(w!("Shell_TrayWnd"), Some(hinst.into()));
                }
            }
        }
    }

    fn is_first(&mut self) -> bool {
        self.hwnd.is_some_and(|h| first_tray_hwnd() == Some(h.0 as isize))
    }

    fn raise(&mut self) {
        if let Some(h) = self.hwnd {
            // OUR window only; Explorer's is never touched
            unsafe {
                let _ = SetWindowPos(h, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
            }
        }
    }

    fn mirror(&mut self, explorer_tray: Option<isize>) {
        let (Some(me), Some(t)) = (self.hwnd, explorer_tray) else { return };
        let ex = hwnd_of(t);
        unsafe {
            let mut r = RECT::default();
            if GetWindowRect(ex, &mut r).is_ok() {
                let _ = SetWindowPos(me, None, r.left, r.top, (r.right - r.left).max(1), (r.bottom - r.top).max(1), SWP_NOZORDER | SWP_NOACTIVATE);
                if let (Some(child), Ok(ec)) = (self.child, FindWindowExW(Some(ex), None, w!("TrayNotifyWnd"), PCWSTR::null())) {
                    let mut cr = RECT::default();
                    if GetWindowRect(ec, &mut cr).is_ok() {
                        let _ = SetWindowPos(child, None, cr.left - r.left, cr.top - r.top, (cr.right - cr.left).max(1), (cr.bottom - cr.top).max(1), SWP_NOZORDER | SWP_NOACTIVATE);
                    }
                }
            }
            let theirs = explorer_props(ex);
            let their_names: Vec<Vec<u16>> = theirs.iter().map(|(n, _)| n.clone()).collect();
            for gone in props_to_remove(&self.props, &their_names) {
                // (a fact Explorer withdrew)
                let _ = RemovePropW(me, PCWSTR(gone.as_ptr()));
                self.props.retain(|p| *p != gone);
            }
            for (name, value) in theirs {
                // (only the properties that state a fact about the taskbar; the OLE, accessibility and edge-docking ones belong to
                // Explorer's own window and mean nothing on ours)
                if !should_mirror_prop(&String::from_utf16_lossy(&name[..name.len().saturating_sub(1)])) {
                    continue;
                }
                // (read back first: only a property that differs is written)
                let now = GetPropW(me, PCWSTR(name.as_ptr()));
                if now.0 as isize != value && SetPropW(me, PCWSTR(name.as_ptr()), Some(HANDLE(value as *mut c_void))).is_ok() && !self.props.contains(&name) {
                    self.props.push(name);
                }
            }
        }
    }

    fn hwnd(&self) -> Option<isize> {
        self.hwnd.map(|h| h.0 as isize)
    }
}

const CHILD_CLASS_W: PCWSTR = w!("TrayNotifyWnd");

// ---------------------------------------------------------------- the window procedure

thread_local! {
    static CORE: RefCell<Option<Rc<Core>>> = const { RefCell::new(None) };
    static LIFE: RefCell<Option<Lifecycle<Win32Window, Win32Broadcast>>> = const { RefCell::new(None) };
    static TASKBAR_CREATED: Cell<u32> = const { Cell::new(0) };
}

fn core_rc() -> Option<Rc<Core>> {
    CORE.with(|c| c.try_borrow().ok().and_then(|g| g.clone()))
}

unsafe extern "system" fn child_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match catch_unwind(AssertUnwindSafe(|| wndproc_inner(hwnd, msg, wp, lp))) {
        Ok(r) => r,
        Err(_) => {
            log::error!("tray host: the window procedure panicked (message {msg:#x}); the message is dropped");
            match msg {
                WM_COPYDATA => LRESULT(0),
                _ => unsafe { DefWindowProcW(hwnd, msg, wp, lp) },
            }
        }
    }
}

fn wndproc_inner(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tc = TASKBAR_CREATED.with(Cell::get);
    match msg {
        WM_COPYDATA => {
            let Some(core) = core_rc() else { return LRESULT(0) };
            // the COPYDATASTRUCT and its data are valid until this returns
            let cds = lp.0 as *const COPYDATASTRUCT;
            if cds.is_null() {
                return LRESULT(0);
            }
            let (dw_data, cb, ptr) = unsafe { ((*cds).dwData, (*cds).cbData as usize, (*cds).lpData as *const u8) };
            let bytes: &[u8] = if cb == 0 || ptr.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(ptr, cb) } };
            LRESULT(core.on_copydata(dw_data, bytes, Raw { msg, wparam: wp.0, lparam: lp.0 }))
        }
        WM_TIMER if wp.0 == TIMER_TICK => {
            tick();
            LRESULT(0)
        }
        WM_DISPLAYCHANGE | WM_DPICHANGED | WM_SETTINGCHANGE => {
            let explorer = core_rc().and_then(|c| c.explorer_tray());
            LIFE.with(|l| {
                if let Ok(mut g) = l.try_borrow_mut()
                    && let Some(life) = g.as_mut()
                {
                    life.window_mut().mirror(explorer);
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                destroy_own(hwnd);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            destroy_own(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_TICK);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        m if tc != 0 && m == tc => {
            // Explorer made its taskbar: the model stays (programs add their icons again and it merges them)
            if let Some(core) = core_rc() {
                core.on_taskbar_created();
                let explorer = core.explorer_tray();
                LIFE.with(|l| {
                    if let Ok(mut g) = l.try_borrow_mut()
                        && let Some(life) = g.as_mut()
                    {
                        life.on_taskbar_created(explorer);
                    }
                });
            }
            LRESULT(0)
        }
        _ => match method_for(msg, tc) {
            Method::Local => {
                note_local(msg);
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            m => match core_rc() {
                Some(core) => LRESULT(core.on_other(Raw { msg, wparam: wp.0, lparam: lp.0 }, m)),
                None => LRESULT(0),
            },
        },
    }
}

/// Destroys our window through the lifecycle's window object, so that the object forgets the handle (see `destroy_window`). Falls
/// back to destroying the handle itself if the lifecycle's cell is borrowed or empty.
fn destroy_own(hwnd: HWND) {
    let done = LIFE.with(|l| match l.try_borrow_mut() {
        Ok(mut g) => match g.as_mut() {
            Some(life) => {
                life.window_mut().destroy_window();
                true
            }
            None => false,
        },
        Err(_) => false,
    });
    if !done {
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

/// A message that was NOT passed on to Explorer's window (`forward::method_for` says `Local`) is counted when it can have come from
/// somebody else: a registered message (0xC000 and up; broadcasts such as `TaskbarCreated` are handled before this) or one sent by
/// another thread. Our own window's housekeeping messages (creation, size, position) come from our thread and are not counted. The
/// numbers say whether anything addresses the first `Shell_TrayWnd` with messages the host does not pass on (see the document).
fn note_local(msg: u32) {
    let Some(core) = core_rc() else { return };
    let c = &core.shared.counters;
    if msg >= 0xC000 {
        bump(&c.local_registered);
    } else if unsafe { InSendMessage() }.as_bool() {
        bump(&c.local_sent_by_others);
    }
}

fn tick() {
    let Some(core) = core_rc() else { return };
    core.shared.beat();
    // (Explorer's window is asked for outside the lifecycle's cell: the lookup can enumerate windows)
    let explorer = core.explorer_tray();
    let work = LIFE.with(|l| l.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(|life| life.tick(explorer))));
    if let Some(work) = work
        && work.raised
    {
        // (how often the keeper has to put ours back in front: if Explorer fights for the first place this number grows)
        bump(&core.shared.counters.raised);
    }
    if let Some(work) = work
        && work.sweep
    {
        let n = core.sweep();
        if n > 0 {
            log::debug!("tray host: {n} icon(s) dropped, their windows are gone");
        }
    }
}

// ---------------------------------------------------------------- the host thread

/// One host per process.
static RUNNING: AtomicBool = AtomicBool::new(false);

/// Starts the host thread and waits (at most 5 s) for it to say that its window exists. Returns the thread and a channel that gets a
/// message when the thread has ended. If the host is too slow, the starter gives up through `hs` and the host closes the window it
/// makes late (see `handshake`): a start that reported an error leaves no window behind.
pub(crate) fn spawn(cfg: &TrayHostConfig, shared: Arc<Shared>, hs: Arc<Handshake>) -> Result<(JoinHandle<()>, Receiver<()>), String> {
    if RUNNING.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("a tray host is already running in this process".into());
    }
    let (ready_tx, ready_rx) = crossbeam_channel::bounded::<Result<(), String>>(1);
    let (done_tx, done_rx) = crossbeam_channel::bounded::<()>(1);
    let cfg = cfg.clone();
    let hs_thread = hs.clone();
    let spawned = std::thread::Builder::new().name("rf-tray-host".into()).spawn(move || {
        let r = catch_unwind(AssertUnwindSafe(|| host_thread(&cfg, shared, &hs_thread, &ready_tx)));
        if r.is_err() {
            let _ = ready_tx.send(Err("the tray host thread panicked".into()));
        }
        // (thread-local destructors run after this: the lifecycle drops there, which destroys the window and broadcasts)
        hs_thread.cleared();
        RUNNING.store(false, Ordering::SeqCst);
        let _ = done_tx.send(());
    });
    let handle = match spawned {
        Ok(h) => h,
        Err(e) => {
            RUNNING.store(false, Ordering::SeqCst);
            return Err(format!("cannot start the tray host thread: {e}"));
        }
    };
    match ready_rx.recv_timeout(std::time::Duration::from_secs(5)) {
        Ok(Ok(())) => Ok((handle, done_rx)),
        Ok(Err(e)) => Err(e),
        Err(_) => {
            // the host is slow, not dead: whatever window it makes or has made is closed, by us if it exists, by the host if not yet
            request_close(hs.give_up());
            Err("the tray host did not start in time".into())
        }
    }
}

/// Asks the host's window to close (from any thread). The thread then destroys the window, ends its loop, and broadcasts.
pub(crate) fn request_close(hwnd: isize) {
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd_of(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
}

fn host_thread(cfg: &TrayHostConfig, shared: Arc<Shared>, hs: &Handshake, ready: &crossbeam_channel::Sender<Result<(), String>>) {
    unsafe {
        // physical pixels: the rectangles copied from Explorer's windows must match what programs compute from them
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    if hs.gave_up() {
        // (the starter stopped waiting before we began: no window is made)
        return;
    }
    let tc = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };
    TASKBAR_CREATED.with(|c| c.set(tc));
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "rustyfiles.exe".into());
    let relay = RelayCore::new(Win32Finder, Win32Sender { timeout_ms: FORWARD_TIMEOUT_MS }, SelfIdentity { hwnd: 0, pid: std::process::id(), exe });
    let core_cfg = CoreConfig { log_only: cfg.log_only, rect_packing: if cfg.log_only { None } else { cfg.rect_packing } };
    let core = Rc::new(Core::new(shared, core_cfg, Box::new(relay), Box::new(Win32Pixels), Arc::new(Win32Owners), Box::new(SystemClock)));
    // the procedure needs the core and the lifecycle BEFORE the window exists (messages arrive during creation)
    CORE.with(|c| *c.borrow_mut() = Some(core.clone()));
    let mut life = Lifecycle::new(Win32Window::new(cfg.visible), Win32Broadcast { taskbar_created: tc }, cfg.broadcast_on_start);
    let explorer = core.explorer_tray();
    if let Err(e) = life.start(explorer) {
        let _ = ready.send(Err(e));
        CORE.with(|c| *c.borrow_mut() = None);
        return;
    }
    let me = life.window().hwnd().unwrap_or(0);
    core.set_self(me);
    LIFE.with(|l| *l.borrow_mut() = Some(life));
    let closed_meanwhile = hs.published(me);
    log::info!(
        "tray host: window made{}; Explorer's tray window {}",
        if cfg.log_only { " (log-only: no picture or label is read)" } else { "" },
        if explorer.is_some() { "found" } else { "not found" }
    );
    unsafe {
        SetTimer(Some(hwnd_of(me)), TIMER_TICK, TICK_MS, None);
    }
    if closed_meanwhile {
        // (the starter gave up while the window was being made: it is closed by our own message loop, which ends and unwinds as usual)
        request_close(me);
    }
    let _ = ready.send(Ok(()));

    unsafe {
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }

    // the window is gone (WM_DESTROY ended the loop): Explorer's tray window is first again; now ask the programs to add their icons
    // again, which they do to Explorer's window
    LIFE.with(|l| {
        if let Some(mut life) = l.borrow_mut().take() {
            life.stop();
        }
    });
    CORE.with(|c| *c.borrow_mut() = None);
    log::info!("tray host: window destroyed, programs asked to add their icons again");
}

#[cfg(test)]
mod gdi_tests {
    //! Reads pictures through GDI from icons made here. No window, no message, nothing of the shell.
    use super::*;
    use windows::Win32::Graphics::Gdi::CreateBitmap;
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon};

    unsafe fn make_icon(color: Option<(&[u8], i32, i32)>, mask_bits: &[u8], mask_w: i32, mask_h: i32) -> HICON {
        unsafe {
            let hbm_mask = CreateBitmap(mask_w, mask_h, 1, 1, Some(mask_bits.as_ptr() as *const _));
            let hbm_color = match color {
                Some((px, w, h)) => CreateBitmap(w, h, 1, 32, Some(px.as_ptr() as *const _)),
                None => HBITMAP::default(),
            };
            let ii = ICONINFO { fIcon: BOOL(1), xHotspot: 0, yHotspot: 0, hbmMask: hbm_mask, hbmColor: hbm_color };
            let icon = CreateIconIndirect(&ii).expect("an icon from two bitmaps");
            let _ = DeleteObject(HGDIOBJ(hbm_mask.0));
            if !hbm_color.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(hbm_color.0));
            }
            icon
        }
    }

    #[test]
    fn a_colour_icon_with_alpha_is_read_back() {
        // 16x16, left half opaque red, right half fully transparent
        let mut px = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let o = (y * 16 + x) * 4;
                if x < 8 {
                    px[o..o + 4].copy_from_slice(&[0, 0, 255, 255]);
                }
            }
        }
        let mask = vec![0u8; 2 * 16]; // 1bpp, 16 px = 2 bytes per row, all zero
        let icon = unsafe { make_icon(Some((&px, 16, 16)), &mask, 16, 16) };
        let bmp = read_icon(icon.0 as isize).expect("the picture");
        unsafe {
            let _ = DestroyIcon(icon);
        }
        assert_eq!((bmp.width, bmp.height), (16, 16));
        assert_eq!(&bmp.bgra[0..4], &[0, 0, 255, 255], "left pixel: opaque red");
        assert_eq!(bmp.bgra[(15 * 4) + 3], 0, "right pixel: transparent");
    }

    #[test]
    fn a_large_icon_is_shrunk_to_64() {
        let px = vec![200u8; 128 * 128 * 4];
        let mask = vec![0u8; 16 * 128];
        let icon = unsafe { make_icon(Some((&px, 128, 128)), &mask, 128, 128) };
        let bmp = read_icon(icon.0 as isize).expect("the picture");
        unsafe {
            let _ = DestroyIcon(icon);
        }
        assert_eq!((bmp.width, bmp.height), (64, 64));
    }

    #[test]
    fn a_monochrome_icon_is_read_back() {
        // 8x8: AND plane (top) all 1 = transparent except the first row, XOR plane (bottom) all 0. Every row of a 1-bit bitmap is
        // padded to a whole word: two bytes
        let mut bits = Vec::new();
        for row in 0..16 {
            let word: [u8; 2] = match row {
                0 => [0x00, 0x00],
                1..=7 => [0xFF, 0xFF],
                _ => [0x00, 0x00],
            };
            bits.extend_from_slice(&word);
        }
        let icon = unsafe { make_icon(None, &bits, 8, 16) };
        let bmp = read_icon(icon.0 as isize).expect("the picture");
        unsafe {
            let _ = DestroyIcon(icon);
        }
        assert_eq!((bmp.width, bmp.height), (8, 8));
        assert_eq!(&bmp.bgra[0..4], &[0, 0, 0, 255], "AND 0, XOR 0: black");
        assert_eq!(bmp.bgra[(8 * 4) + 3], 0, "second row: transparent");
    }

    #[test]
    fn a_null_or_bad_handle_gives_nothing() {
        assert!(Win32Pixels.read(0).is_none());
        assert!(Win32Pixels.read(0x1234_5678).is_none());
    }
}
