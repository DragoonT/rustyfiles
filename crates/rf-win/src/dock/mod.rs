//! Everything the dock (`rustyfiles --dock`: three floating islands above the bottom of the screen) needs from Windows. The windows
//! themselves are GPUI's and live in rf-app; this module is what they stand on.
//!
//! Version 1 floats above the Windows taskbar; it does not hide it, replace it or become the shell, and it undoes everything it did
//! when it exits (the toolbar band is given back by dropping [`DockSystem`], the glass is detached by the app).
//!
//! | file | what it is | pure core |
//! |---|---|---|
//! | `layout` | the three rectangles from a work area and a scale; the middle island's width; the band our toolbar reserves | all of it |
//! | `hover` | the icons' grow on hover (44 -> 56, neighbours 50, ~120 ms eased), hit testing along the dock | all of it |
//! | `clock` | the right island's two lines for a time and the user's locale patterns | formatting |
//! | `model` | pinned + running apps merged in the taskbar's order, the places, the click decision | all of it |
//! | `pins` | the taskbar's pinned apps, read-only (pin folder + `Taskband\Favorites` order); which pins are alive, moved to a newer install, or dead | the order, the fate rules |
//! | `identity` | a running window's packaged-app id (its process's package), so a Store app's windows match its pin | the id handed to the windows |
//! | `icons` | one bitmap with alpha at an exact size for a program, a packaged app, a shell place, a window | the resampling |
//! | `island` | turns a GPUI window into an island, places it, shows / hides it | the style bits |
//! | `system` | primary monitor, the toolbar band, a hidden window that hears about display / DPI / theme / full-screen changes | the placement area |
//! | `fullscreen` | is a full-screen application in front on the primary monitor | the decision |
//! | `triggers` | Start, Win+A, Win+N as `SendInput` answers to a click | the key sequences |
//! | `launch` | start an app, bring a window forward, minimise it | - |
//!
//! HOW THE APP USES IT (the order that works):
//! 1. `system::DockSystem::start(cb)` once; `system::primary_monitor()` for the work area and the scale;
//!    `layout::reserved_thickness(scale)` -> `DockSystem::reserve(..)` when "Keep windows from going under the dock" is on.
//! 2. For each island window: `island::make_island(hwnd)`, then `island::place(hwnd, rect)` with the rectangle from
//!    `layout::islands(area, scale, widths)` where `area` is `DockSystem::placement_area()` (NOT the raw work area: see there).
//! 3. Glass: `crate::glass::attach_with(hwnd, GlassOptions { dark, radius_px: Some(24.0 * scale), cut_corners: true })`.
//! 4. On `SystemEvent::LayoutChanged` redo step 2 (and `GlassOptions` with the new radius via `glass::set_options`); on
//!    `Fullscreen(true)` call `island::set_visible(hwnd, false)` for all three, on `Fullscreen(false)` show them again.
//! 5. The middle island: `pins::read_pins()` (worker thread, again when the pin folder changes), `tasklist::tasks()` +
//!    `tasklist::grouped(..)` + `tasklist::watch(..)`, `model::merge(..)`, `icons::icon_bgra(..)` per item (worker thread).
//!
//! 6. Beside this module: `crate::weather` (the left island's weather, Open-Meteo over WinHTTP, nothing at all without a city),
//!    `crate::accent` (`color()` for the logo, `system_is_dark()` for the theme) and `crate::glass` (`attach_with`, `set_options`).
//!
//! Everything that blocks (COM, the shell, the disk, other programs' windows) is documented as such and meant for a worker thread.

pub mod clock;
pub mod fullscreen;
pub mod hover;
pub mod icons;
pub mod identity;
pub mod island;
pub mod launch;
pub mod layout;
pub mod model;
pub mod pins;
pub mod system;
pub mod triggers;

pub use layout::{Islands, Rect, Widths};
pub use model::{ClickAction, DockApp, DockItem, IconSource, Launch, PinnedApp, Place};
pub use system::{DockSystem, MonitorInfo, SystemEvent, primary_monitor};

/// The window in front for the click decision: its root owner (a dialog of an app counts as that app being in front), 0 when none.
pub fn foreground_root() -> isize {
    use windows::Win32::UI::WindowsAndMessaging::{GA_ROOTOWNER, GetAncestor, GetForegroundWindow};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return 0;
        }
        let root = GetAncestor(fg, GA_ROOTOWNER);
        (if root.0.is_null() { fg } else { root }).0 as isize
    }
}
