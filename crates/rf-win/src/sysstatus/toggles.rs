//! The small switches of the control centre's row of quick buttons that have something to switch: Windows' dark mode, "keep the PC
//! awake" and the power mode. (The other buttons of that row only open a page or a panel of Windows.)
//!
//! Each write is the user's press and nothing else.
//!
//! * DARK MODE is the two values Windows' own Settings write ("Choose your mode": the apps' and the system's), followed by the
//!   broadcast that tells the running programs. One button for both, as macOS has it.
//! * KEEP AWAKE is `SetThreadExecutionState` (the screen stays on, the PC does not go to sleep by itself), which belongs to the
//!   thread that asked: a thread of its own holds it for as long as the button is lit and gives it back when it is pressed again.
//!   It is the dock's process that holds it, so it ends with the dock; nothing is written anywhere.
//! * THE POWER MODE (Settings > System > Power: best power efficiency, balanced, best performance) is the "overlay" of the active
//!   power plan. Its two calls are exports of `powrprof.dll` that Microsoft's headers do not declare, in every Windows since 10
//!   version 1709; they are looked up when first needed, so a Windows without them only has no such button. A power plan that
//!   has no overlays (High performance, Ultimate) answers the reading with an error, and the button is left out too.

use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};

use windows::Win32::Foundation::{HMODULE, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState};
use windows::Win32::UI::WindowsAndMessaging::{HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE};
use windows::core::{GUID, s, w};

// ------------------------------------------------------------------ dark mode

const PERSONALIZE: &str = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize";

/// The two values of a mode, as Windows' own Settings write them (1 is light).
pub fn theme_values(dark: bool) -> [(&'static str, u32); 2] {
    let light = u32::from(!dark);
    [("AppsUseLightTheme", light), ("SystemUsesLightTheme", light)]
}

/// Whether programs are in dark mode now (no value: Windows' default, light).
pub fn is_dark() -> bool {
    windows_registry::CURRENT_USER.open(PERSONALIZE).ok().and_then(|k| k.get_u32("AppsUseLightTheme").ok()) == Some(0)
}

/// The user pressed the button: switches Windows to dark or to light, on a thread of its own (telling every program takes a
/// moment).
pub fn set_dark(dark: bool) {
    let spawned = std::thread::Builder::new().name("rf-theme".into()).spawn(move || {
        let Ok(key) = windows_registry::CURRENT_USER.create(PERSONALIZE) else {
            log::warn!("theme: the key could not be opened");
            return;
        };
        for (name, value) in theme_values(dark) {
            if let Err(e) = key.set_u32(name, value) {
                log::warn!("theme: {name} was not written: {e}");
            }
        }
        unsafe {
            // (what Windows' own Settings send; a program that does not answer is not waited for)
            let what = w!("ImmersiveColorSet");
            let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(what.as_ptr() as isize), SMTO_ABORTIFHUNG, 200, None);
        }
    });
    if let Err(e) = spawned {
        log::warn!("theme: the thread did not start: {e}");
    }
}

// ------------------------------------------------------------------ keep awake

/// The thread that holds the PC awake is told to let go by this sender being dropped.
static AWAKE: Mutex<Option<Sender<()>>> = Mutex::new(None);

pub fn is_awake() -> bool {
    AWAKE.lock().map(|h| h.is_some()).unwrap_or(false)
}

/// The user pressed the button: holds the screen and the PC awake, or lets go.
pub fn set_awake(on: bool) {
    let Ok(mut hold) = AWAKE.lock() else { return };
    match (on, hold.is_some()) {
        (true, false) => {
            let (tx, rx) = channel::<()>();
            let spawned = std::thread::Builder::new().name("rf-awake".into()).spawn(move || unsafe {
                SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED);
                // (until the sender goes)
                let _ = rx.recv();
                SetThreadExecutionState(ES_CONTINUOUS);
            });
            match spawned {
                Ok(_) => *hold = Some(tx),
                Err(e) => log::warn!("awake: the thread did not start: {e}"),
            }
        }
        (false, true) => *hold = None,
        _ => {}
    }
}

// ------------------------------------------------------------------ the power mode

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerMode {
    Efficiency,
    Balanced,
    Performance,
}

const EFFICIENCY: GUID = GUID::from_u128(0x961cc777_2547_4f9d_8174_7d86181b8a7a);
const PERFORMANCE: GUID = GUID::from_u128(0xded574b5_45a0_4f42_8737_46345c09c238);

/// The mode an overlay is. The plan with no overlay over it is "balanced"; an overlay this does not know (older Windows had a
/// fourth between two of these) is shown as balanced too, and a press from there goes on to "performance".
pub fn mode_of(overlay: &GUID) -> PowerMode {
    if *overlay == EFFICIENCY {
        PowerMode::Efficiency
    } else if *overlay == PERFORMANCE {
        PowerMode::Performance
    } else {
        PowerMode::Balanced
    }
}

pub fn overlay_of(mode: PowerMode) -> GUID {
    match mode {
        PowerMode::Efficiency => EFFICIENCY,
        PowerMode::Balanced => GUID::zeroed(),
        PowerMode::Performance => PERFORMANCE,
    }
}

/// What a press on the button goes to: balanced, performance, efficiency, and round again.
pub fn next_mode(now: PowerMode) -> PowerMode {
    match now {
        PowerMode::Balanced => PowerMode::Performance,
        PowerMode::Performance => PowerMode::Efficiency,
        PowerMode::Efficiency => PowerMode::Balanced,
    }
}

type OverlayCall = unsafe extern "system" fn(*mut GUID) -> u32;

/// One of `powrprof.dll`'s two calls, looked up by name (see the module docs).
fn overlay_call(name: windows::core::PCSTR) -> Option<OverlayCall> {
    unsafe {
        let lib: HMODULE = LoadLibraryW(w!("powrprof.dll")).ok()?;
        let f = GetProcAddress(lib, name)?;
        // SAFETY: both exports take one pointer to a GUID and return a Win32 error code
        Some(std::mem::transmute::<unsafe extern "system" fn() -> isize, OverlayCall>(f))
    }
}

/// The power mode now; `None` where Windows has none to tell (see the module docs).
pub fn power_mode() -> Option<PowerMode> {
    let read = overlay_call(s!("PowerGetEffectiveOverlayScheme"))?;
    let mut overlay = GUID::zeroed();
    (unsafe { read(&mut overlay) } == 0).then(|| mode_of(&overlay))
}

/// The user pressed the button: asks Windows for that power mode. True when Windows took it.
pub fn set_power_mode(mode: PowerMode) -> bool {
    let Some(write) = overlay_call(s!("PowerSetActiveOverlayScheme")) else { return false };
    let mut overlay = overlay_of(mode);
    let code = unsafe { write(&mut overlay) };
    if code != 0 {
        log::warn!("power mode: Windows did not take {mode:?} ({code})");
    }
    code == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_mode_is_the_two_values_windows_writes() {
        assert_eq!(theme_values(true), [("AppsUseLightTheme", 0), ("SystemUsesLightTheme", 0)]);
        assert_eq!(theme_values(false), [("AppsUseLightTheme", 1), ("SystemUsesLightTheme", 1)]);
    }

    #[test]
    fn the_pc_is_held_awake_while_asked_and_let_go_after() {
        assert!(!is_awake());
        set_awake(true);
        assert!(is_awake());
        // (asked again: still one hold)
        set_awake(true);
        assert!(is_awake());
        set_awake(false);
        assert!(!is_awake());
        set_awake(false);
        assert!(!is_awake());
    }

    #[test]
    fn the_power_modes_are_three_overlays_and_a_press_goes_round_them() {
        for m in [PowerMode::Efficiency, PowerMode::Balanced, PowerMode::Performance] {
            assert_eq!(mode_of(&overlay_of(m)), m);
        }
        assert_eq!(overlay_of(PowerMode::Balanced), GUID::zeroed());
        // an overlay nobody here knows is shown as balanced
        assert_eq!(mode_of(&GUID::from_u128(0x3af9b8d9_7c97_431d_ad78_34a8bfea439f)), PowerMode::Balanced);
        let mut m = PowerMode::Balanced;
        let mut seen = Vec::new();
        for _ in 0..3 {
            m = next_mode(m);
            seen.push(m);
        }
        assert_eq!(seen, [PowerMode::Performance, PowerMode::Efficiency, PowerMode::Balanced]);
    }

    /// On the PC it is run on, by hand: what Windows says (nothing is changed).
    #[test]
    #[ignore = "reads the theme and the power mode of the PC it runs on; run by hand"]
    fn what_this_pc_says() {
        println!("dark {} power mode {:?}", is_dark(), power_mode());
    }
}
