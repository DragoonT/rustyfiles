//! "Is a full-screen application in front?", so the islands can get out of the way of a game or a video and come back after it.
//!
//! The decision is pure ([`is_fullscreen`]): the window in front covers the whole monitor, is not the desktop or a shell window, is
//! not one of ours, and is not an ordinary maximised window. A maximised window covers the whole monitor too when the Windows
//! taskbar hides itself; it keeps its title bar, or (a frameless Electron or Qt window) its sizing frame together with the maximised
//! state. A game or a full-screen video has neither. A full-screen UWP app is the exception that keeps its title bar: it is told
//! apart by its frame window covering the monitor exactly.

use super::layout::Rect;

/// What the decision needs to know about the window in front.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Foreground {
    pub rect: Rect,
    /// The desktop, the taskbar, the Start menu or another piece of the shell.
    pub shell: bool,
    /// A window of this process (the islands themselves, the app's own windows).
    pub own: bool,
    /// Has both the border and the dialog-frame style bits that make a title bar (`WS_CAPTION`).
    pub caption: bool,
    /// Is maximised (`WS_MAXIMIZE`) and has a sizing frame (`WS_THICKFRAME`): an ordinary maximised window, however it paints its
    /// own title bar.
    pub maximized_frame: bool,
    /// An `ApplicationFrameWindow`: the frame every UWP app lives in.
    pub uwp_frame: bool,
}

/// The frame window of every UWP app.
pub const UWP_FRAME_CLASS: &str = "ApplicationFrameWindow";

/// Window classes of the shell: they may cover the whole screen without being a full-screen application.
pub const SHELL_CLASSES: [&str; 8] = [
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Windows.UI.Core.CoreWindow",
    "XamlExplorerHostIslandWindow",
    "MultitaskingViewFrame",
    "TaskListThumbnailWnd",
];

/// Whether `class` (a window class name) belongs to the shell.
pub fn is_shell_class(class: &str) -> bool {
    SHELL_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(class))
}

/// The decision: `fg` is the window in front (`None`: nothing is, or it could not be read), `monitor` the primary monitor's whole
/// rectangle (not its work area).
pub fn is_fullscreen(fg: Option<&Foreground>, monitor: Rect) -> bool {
    let Some(fg) = fg else { return false };
    if fg.shell || fg.own || fg.maximized_frame {
        return false;
    }
    if monitor.width() <= 0 || monitor.height() <= 0 || !fg.rect.covers(&monitor) {
        return false;
    }
    // a title bar means an ordinary window that happens to reach the edges - except a UWP app's frame, which keeps its title bar
    // in full screen and then fits the monitor exactly (a maximised one is inside the work area or a few px outside it)
    !fg.caption || (fg.uwp_frame && fg.rect == monitor)
}

/// `WS_CAPTION` is `WS_BORDER | WS_DLGFRAME`: both bits make a title bar.
pub const WS_CAPTION_BITS: u32 = 0x00C0_0000;

pub fn has_caption(style: u32) -> bool {
    style & WS_CAPTION_BITS == WS_CAPTION_BITS
}

const WS_MAXIMIZE: u32 = 0x0100_0000;
const WS_THICKFRAME: u32 = 0x0004_0000;

/// A maximised window with a sizing frame (see [`Foreground::maximized_frame`]).
pub fn has_maximized_frame(style: u32) -> bool {
    style & (WS_MAXIMIZE | WS_THICKFRAME) == (WS_MAXIMIZE | WS_THICKFRAME)
}

/// Reads the window in front. `None` when there is none or it cannot be measured.
pub fn foreground() -> Option<Foreground> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GWL_STYLE, GetClassNameW, GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId};
    unsafe {
        let h = GetForegroundWindow();
        if h.0.is_null() {
            return None;
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut class = [0u16; 128];
        let n = GetClassNameW(h, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        Some(Foreground {
            rect: Rect::new(r.left, r.top, r.right, r.bottom),
            shell: is_shell_class(&class),
            own: pid == GetCurrentProcessId(),
            caption: has_caption(style),
            maximized_frame: has_maximized_frame(style),
            uwp_frame: class.eq_ignore_ascii_case(UWP_FRAME_CLASS),
        })
    }
}

/// Whether a full-screen application is in front on the primary monitor right now. Cheap (a few window calls): fine to call every
/// half second or on every foreground change.
pub fn on_primary() -> bool {
    let Some(m) = super::system::primary_monitor() else { return false };
    is_fullscreen(foreground().as_ref(), m.rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 1920, 1080);

    fn fg(rect: Rect) -> Foreground {
        Foreground { rect, shell: false, own: false, caption: false, maximized_frame: false, uwp_frame: false }
    }

    #[test]
    fn a_game_covering_the_monitor_is_full_screen() {
        assert!(is_fullscreen(Some(&fg(MON)), MON));
        // a borderless window a few px bigger than the monitor
        assert!(is_fullscreen(Some(&fg(Rect::new(-1, -1, 1921, 1081))), MON));
    }

    #[test]
    fn an_ordinary_or_smaller_window_is_not() {
        assert!(!is_fullscreen(Some(&fg(Rect::new(0, 0, 1920, 1032))), MON), "a maximised window above the taskbar");
        assert!(!is_fullscreen(Some(&fg(Rect::new(100, 100, 900, 700))), MON));
        assert!(!is_fullscreen(Some(&fg(Rect::new(-32000, -32000, -31840, -31972))), MON), "a minimised window");
        assert!(!is_fullscreen(None, MON));
    }

    #[test]
    fn a_maximised_window_with_a_title_bar_is_not_even_without_a_taskbar() {
        // with the Windows taskbar on auto-hide a maximised window reaches the monitor's edges
        let max = Foreground { caption: true, ..fg(Rect::new(-8, -8, 1928, 1088)) };
        assert!(!is_fullscreen(Some(&max), MON));
    }

    #[test]
    fn a_frameless_maximised_window_is_not_full_screen_either() {
        // Electron / Qt windows without a title bar: maximised with a sizing frame, grown by the frame past the monitor
        let max = Foreground { maximized_frame: true, ..fg(Rect::new(-8, -8, 1928, 1088)) };
        assert!(!is_fullscreen(Some(&max), MON));
        // a game's borderless window has no sizing frame, whatever else it has
        assert!(is_fullscreen(Some(&fg(Rect::new(0, 0, 1920, 1080))), MON));
    }

    #[test]
    fn a_full_screen_uwp_app_keeps_its_title_bar_but_fits_the_monitor_exactly() {
        let uwp = |rect| Foreground { caption: true, uwp_frame: true, ..fg(rect) };
        assert!(is_fullscreen(Some(&uwp(MON)), MON));
        // maximised: a few px outside the monitor, not full screen
        assert!(!is_fullscreen(Some(&uwp(Rect::new(-8, -8, 1928, 1088))), MON));
        // an ordinary window with a title bar that fits the monitor exactly is still not
        assert!(!is_fullscreen(Some(&Foreground { caption: true, ..fg(MON) }), MON));
    }

    #[test]
    fn the_desktop_the_shell_and_ourselves_are_never_full_screen() {
        assert!(!is_fullscreen(Some(&Foreground { shell: true, ..fg(MON) }), MON));
        assert!(!is_fullscreen(Some(&Foreground { own: true, ..fg(MON) }), MON));
    }

    #[test]
    fn only_the_primary_monitor_counts() {
        // a game on a second monitor to the right does not cover the primary one
        assert!(!is_fullscreen(Some(&fg(Rect::new(1920, 0, 3840, 1080))), MON));
        // a degenerate monitor never makes everything full screen
        assert!(!is_fullscreen(Some(&fg(MON)), Rect::new(0, 0, 0, 0)));
    }

    #[test]
    fn shell_classes_and_styles() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("workerw"));
        assert!(is_shell_class("Shell_TrayWnd"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
        assert!(has_caption(0x00C0_0000 | 0x1000_0000));
        assert!(!has_caption(0x8000_0000)); // WS_POPUP alone
        assert!(!has_caption(0x0080_0000)); // WS_BORDER alone
        assert!(has_maximized_frame(0x0100_0000 | 0x0004_0000 | 0x1000_0000));
        assert!(!has_maximized_frame(0x0100_0000), "maximised without a sizing frame (a game)");
        assert!(!has_maximized_frame(0x0004_0000), "a sizing frame without being maximised");
    }

    /// Read-only: the live read of the window in front is self-consistent (a rectangle that is not inverted, or no window at all).
    #[test]
    fn the_live_read_is_sane() {
        if let Some(f) = foreground() {
            assert!(f.rect.right >= f.rect.left && f.rect.bottom >= f.rect.top, "{f:?}");
        }
        on_primary(); // must not panic or hang
    }
}
