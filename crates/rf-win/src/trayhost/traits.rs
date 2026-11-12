//! The small traits through which the tray host touches Windows. Everything the host decides is written against these; the real
//! implementations are in `win.rs` (never run by a test), the stand-ins in `fakes.rs`.
//!
//! | trait | what it is | real implementation |
//! |---|---|---|
//! | [`Relay`] | passes a message on to Explorer's tray window and says what came back | `relay::RelayCore` over `win::Win32Finder` + `win::Win32Sender` |
//! | [`TrayWindow`] | our own `Shell_TrayWnd` (and its `TrayNotifyWnd` child): make, destroy, keep first, mirror | `win::Win32Window` |
//! | [`Broadcast`] | the `TaskbarCreated` broadcast | `win::Win32Broadcast` |
//! | [`OwnerSend`] | questions about and messages to an icon's owner window | `win::Win32Owners` |
//! | [`IconPixels`] | the pixels of an `HICON` | `win::Win32Pixels` |
//! | [`Clock`] | milliseconds, for the model's balloon stamps | `SystemClock` |

use super::model::IconBitmap;
use crate::tray_wire::forward::{Method, Outcome};

/// A message as the window procedure got it, to be passed on untouched. For `WM_COPYDATA` the `lparam` is the pointer to the
/// sender's `COPYDATASTRUCT`; it is valid only while the window procedure runs, which is why a [`Relay`] call is synchronous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Raw {
    pub msg: u32,
    pub wparam: usize,
    pub lparam: isize,
}

/// What a [`Relay`] call came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Relayed {
    /// There is no tray window of Explorer's to pass the message to.
    NoTarget,
    /// It was passed on (or posted); how it ended.
    Done(Outcome),
}

/// Passes messages to Explorer's own tray window. All methods take `&self`: a window procedure can be entered again while it waits
/// for Explorer (another program's `WM_COPYDATA` is delivered to a thread that is inside `SendMessageTimeout`), so no implementation
/// may hold a lock across the send.
pub trait Relay: Send + Sync {
    /// Passes `raw` on with `method` ([`Method::Send`] or [`Method::Post`]). A target that vanished is looked up once more.
    fn relay(&self, raw: Raw, method: Method) -> Relayed;
    /// Explorer's tray window as of now (cached, checked, looked up again when it is gone), if there is one.
    fn target(&self) -> Option<isize>;
    /// Forget the cached target: the next message looks again (Explorer was restarted).
    fn invalidate(&self);
    /// Tells the relay which window is ours, so that it never forwards to it.
    fn set_self(&self, hwnd: isize);
}

/// Our own tray window.
pub trait TrayWindow {
    /// Makes `Shell_TrayWnd` and its `TrayNotifyWnd` child. Hidden unless the configuration says otherwise. Not twice.
    fn create(&mut self) -> Result<(), String>;
    /// Destroys the windows. Safe to call when there are none, and twice.
    fn destroy(&mut self);
    /// Whether ours is the first `Shell_TrayWnd` a program finds.
    fn is_first(&mut self) -> bool;
    /// Puts ours first again (never touches Explorer's window).
    fn raise(&mut self);
    /// Copies Explorer's tray rectangles (and the properties programs read from its window) onto ours.
    fn mirror(&mut self, explorer_tray: Option<isize>);
    /// The window's handle while it exists.
    fn hwnd(&self) -> Option<isize>;
}

/// The `TaskbarCreated` broadcast: every program that handles it adds its icons again.
pub trait Broadcast {
    fn taskbar_created(&mut self);
}

/// Questions about an icon's owner window, and the messages sent to it.
pub trait OwnerSend: Send + Sync {
    fn is_window(&self, hwnd: isize) -> bool;
    /// The process that owns the window.
    fn owner_pid(&self, hwnd: isize) -> Option<u32>;
    /// The file name of a process's program (`chrome.exe`), no path.
    fn owner_exe(&self, pid: u32) -> Option<String>;
    /// `AllowSetForegroundWindow(pid)`: the owner may take the foreground (its menu, its window).
    fn allow_foreground(&self, pid: u32);
    /// `SendNotifyMessage`: queued when the owner is on another thread, so a hung program cannot freeze the caller. False when the
    /// message could not be sent.
    fn send_notify(&self, hwnd: isize, msg: u32, wparam: usize, lparam: isize) -> bool;
}

/// Reads the pixels of an `HICON` (given as the sign-extended handle of the block), at most `MAX_ICON_PX` square on return.
pub trait IconPixels: Send + Sync {
    fn read(&self, hicon: isize) -> Option<IconBitmap>;
}

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// Milliseconds since the Unix epoch.
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
    }
}
