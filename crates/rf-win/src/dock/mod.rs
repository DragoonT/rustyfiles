//! Everything the dock (`rustyfiles --dock`: three floating islands above the bottom of the screen) needs from Windows. The windows
//! themselves are GPUI's and live in rf-app; this module is what they stand on.
//!
//! Version 1 floats above the Windows taskbar; it does not hide it, replace it or become the shell, and it undoes everything it did
//! when it exits (the toolbar band is given back by dropping [`DockSystem`], the glass is detached by the app).
//!
//! | file | what it is | pure core |
//! |---|---|---|
//! | `layout` | the three glass rectangles from a work area and a scale; the three WINDOWS around them (an edge margin, headroom and room to widen; room for a shadow only with `RUSTYFILES_DOCK_SHADOW=1`); the middle island's width; the band our toolbar reserves | all of it |
//! | `magnify` | the Mac-like magnification: a bell of the pointer's distance over the icons' rest centres, the glass widening with them, the ease in and out | all of it |
//! | `hit` | which part of an island's window takes the pointer (the glass and the icons standing out of it) and the window region that lets the rest through to other programs | the pieces |
//! | `hover` | the dock's slots and the icon hit test along it (the grow itself is `magnify`) | all of it |
//! | `clock` | the right island's two lines for a time and the user's locale patterns | formatting |
//! | `calendar` | the clock panel's month grid (six rows, the user's first day of the week), the Turkish and English names, how the keys move through it | all of it |
//! | `model` | pinned + running apps merged in the taskbar's order, the places, the click decision | all of it |
//! | `pins` | the taskbar's pinned apps, read-only (pin folder + `Taskband\Favorites` order); which pins are alive, moved to a newer install, or dead | the order, the fate rules |
//! | `identity` | a running window's packaged-app id (its process's package), so a Store app's windows match its pin | the id handed to the windows |
//! | `icons` | one bitmap with alpha at an exact size for a program, a packaged app, a shell place, a window | the resampling |
//! | `island` | turns a GPUI window into an island, places it, shows / hides it | the style bits |
//! | `system` | primary monitor, the toolbar band, a hidden window that hears about display / DPI / theme / full-screen changes | the placement area |
//! | `fullscreen` | is a full-screen application in front on the primary monitor | the decision |
//! | `triggers` | Start, Win+A, Win+N as `SendInput` answers to a click | the key sequences |
//! | `launch` | start an app, bring a window forward, minimise it | - |
//! | `panel` | where a panel above an island's visible glass (not its larger window) goes, when it opens / closes / switches, who gets the keyboard back, the 120 ms easing | all of it |
//! | `power` | the Start panel's power menu (lock, sleep, sign out, restart, shut down) and its "Emin misiniz?" step; the user's display name | the menu |
//! | `startlist` | the app list's order (Turkish alphabet), letter headers, rows, arrow-key movement through the pinned grid and the list | all of it |
//! | `startpins` | the Start panel's own pinned apps file and its first seeding from the taskbar's pins | all of it |
//! | `startapps` | start an app of "All apps", as administrator (the user's click), find its program (shell thread) | - |
//!
//! HOW THE APP USES IT (the order that works):
//! 1. `system::DockSystem::start(cb)` once; `system::primary_monitor()` for the work area and the scale;
//!    `layout::reserved_thickness(scale)` -> `DockSystem::reserve(..)` when "Keep windows from going under the dock" is on.
//! 2. For each island window: `island::make_island(hwnd)`, then `island::place(hwnd, rect)` with the WINDOW rectangle from
//!    `layout::placements(&layout::islands(area, scale, widths), monitor, scale, layout::shadow_on())` where `area` is `DockSystem::placement_area()` (NOT the
//!    raw work area: see there). The glass is `Placement::glass`, where `islands` put it; the window is bigger.
//! 3. Glass: `crate::glass::attach_with(hwnd, GlassOptions { dark, radius_px: Some(24.0 * scale), cut_corners: true, shape: Some(glass in
//!    the window), shadow: layout::shadow_on().then_some(Shadow::SOFT) })`; the shape follows the glass through `glass::set_options` (cheap, made for animation).
//!    `hit::set_region(hwnd, &hit::parts(..))` lets the pointer through everywhere but the glass and the icons standing out of it.
//! 4. On `SystemEvent::LayoutChanged` redo step 2 (and `GlassOptions` with the new radius via `glass::set_options`); on
//!    `Fullscreen(true)` call `island::set_visible(hwnd, false)` for all three, on `Fullscreen(false)` show them again.
//! 5. The middle island: `pins::read_pins()` (worker thread, again when the pin folder changes), `tasklist::tasks()` +
//!    `tasklist::grouped(..)` + `tasklist::watch(..)`, `model::merge(..)`, `icons::icon_bgra(..)` per item (worker thread).
//!
//! 6. Beside this module: `crate::weather` (the left island's weather, Open-Meteo over WinHTTP, nothing at all without a city),
//!    `crate::accent` (`color()` for the logo, `system_is_dark()` for the theme) and `crate::glass` (`attach_with`, `set_options`).
//!
//! Everything that blocks (COM, the shell, the disk, other programs' windows) is documented as such and meant for a worker thread.

pub mod calendar;
pub mod clock;
pub mod fullscreen;
pub mod hit;
pub mod hover;
pub mod icons;
pub mod identity;
pub mod island;
pub mod launch;
pub mod layout;
pub mod magnify;
pub mod model;
pub mod panel;
pub mod pins;
pub mod power;
pub mod startapps;
pub mod startlist;
pub mod startpins;
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
