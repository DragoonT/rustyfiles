//! The colours of Windows' own menus shown by this process (the classic context menu of "Show more options" and Shift+right click,
//! the small tag menu): dark or light as rustyfiles is, instead of always light (issue 211).
//!
//! Windows has no documented call for it. Explorer, Notepad++ and many others use two exports of `uxtheme.dll` that have no names,
//! only numbers, and have stayed the same since Windows 10 1903 (build 18362): 135, `SetPreferredAppMode(mode)`, the theme the
//! process's menus are drawn in, and 136, `FlushMenuThemes()`, which makes a change reach menus opened from then on. Below build
//! 18362 export 135 was another function (it took a BOOL), so nothing is called there. A missing export is no error: the menus stay
//! as Windows draws them.

use std::sync::atomic::{AtomicU8, Ordering};

/// What the app wants: 0 follow the system's app theme, 1 light, 2 dark.
static WANTED: AtomicU8 = AtomicU8::new(0);
/// The mode last given to Windows (`u8::MAX`: none yet).
static GIVEN: AtomicU8 = AtomicU8::new(u8::MAX);

/// `SetPreferredAppMode`'s modes.
const FORCE_DARK: u8 = 2;
const FORCE_LIGHT: u8 = 3;

/// The app's theme: `Some(true)` dark, `Some(false)` light, `None` the system's app theme. Cheap; nothing is called until a menu opens.
pub fn set_wanted(dark: Option<bool>) {
    WANTED.store(
        match dark {
            None => 0,
            Some(false) => 1,
            Some(true) => 2,
        },
        Ordering::Relaxed,
    );
}

/// The mode for a wish (see [`set_wanted`]) and the system's app theme.
fn mode_for(wanted: u8, system_dark: bool) -> u8 {
    let dark = match wanted {
        1 => false,
        2 => true,
        _ => system_dark,
    };
    if dark { FORCE_DARK } else { FORCE_LIGHT }
}

/// Call right before a menu of Windows' is opened: the process's menus are given the app's theme (nothing when it is the one given
/// last time).
pub fn apply() {
    let mode = mode_for(WANTED.load(Ordering::Relaxed), crate::accent::apps_use_dark());
    if GIVEN.swap(mode, Ordering::Relaxed) == mode || crate::osver::build() < 18362 {
        return;
    }
    unsafe {
        use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
        use windows::core::{PCSTR, w};
        let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else { return };
        // (an export without a name is asked for by its number in the low word of the name pointer)
        let set = GetProcAddress(ux, PCSTR(135usize as *const u8));
        let flush = GetProcAddress(ux, PCSTR(136usize as *const u8));
        if let (Some(set), Some(flush)) = (set, flush) {
            let set: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set);
            let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
            set(i32::from(mode));
            flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menus_follow_the_apps_theme_and_the_system_only_when_asked() {
        assert_eq!(mode_for(2, false), FORCE_DARK);
        assert_eq!(mode_for(1, true), FORCE_LIGHT);
        assert_eq!(mode_for(0, true), FORCE_DARK);
        assert_eq!(mode_for(0, false), FORCE_LIGHT);
    }
}
