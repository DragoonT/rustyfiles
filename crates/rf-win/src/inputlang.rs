//! The input language (keyboard layout): which ones the user has, which one the window in front is typing in, and a request to switch.
//!
//! WHY (issue 21, a user of the dock: "also i don't see language switcher icon"). Windows' taskbar shows the input language beside the
//! clock ("ENG", "TUR") and switches it on a click. With the dock in the taskbar's place nothing showed it, and a user with two
//! layouts could not tell which one a window was in.
//!
//! WHAT IS READ. The layout is a property of a THREAD: each window types in the layout of the thread that owns it, and Alt+Shift (or
//! Win+Space) changes it for the window in front. So "the current language" is the layout of the foreground window's thread. Nothing
//! here is sent to that window to read it. Reading is two system calls; the dock looks a few times a second and only while the user
//! has more than one layout.
//!
//! SWITCHING is the request Windows itself defines for it (`WM_INPUTLANGCHANGEREQUEST`, posted to the window in front, as the
//! language bar does): the window's own thread makes the change. A window of a program that runs as administrator does not take a
//! posted message from this process; its layout then stays as it is, which the indicator goes on showing truthfully.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Globalization::{GetLocaleInfoEx, LCIDToLocaleName, LOCALE_SISO639LANGNAME2};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId, PostMessageW, WM_INPUTLANGCHANGEREQUEST};

/// The language part of a layout handle (its low word).
pub fn lang_id(layout: isize) -> u16 {
    (layout as usize & 0xFFFF) as u16
}

/// The layouts the user has installed, in the system's order.
pub fn layouts() -> Vec<isize> {
    unsafe {
        let n = GetKeyboardLayoutList(None);
        if n <= 0 {
            return Vec::new();
        }
        let mut list = vec![HKL::default(); n as usize];
        let got = GetKeyboardLayoutList(Some(&mut list));
        list.truncate(got.max(0) as usize);
        list.into_iter().map(|h| h.0 as isize).collect()
    }
}

/// The window that has the keyboard in the window in front, and the layout ITS thread types in. `None` when no window is in front for
/// the moment.
///
/// WHY THE KEYBOARD'S WINDOW (issue 138: the indicator showed another language than Windows' own, and "sometimes when you click the
/// language button, it switches language but does not update the name"). The window in front is not always the one that types: a
/// Store app's frame (`ApplicationFrameWindow`) belongs to a thread of the frame host, and a program may type in a window of another
/// thread. Its layout was read, and the request was posted, to the frame's thread, which neither showed nor took the real language.
/// `GetGUIThreadInfo` names the window with the keyboard focus, as Windows' indicator follows it.
pub fn of_foreground() -> Option<(isize, isize)> {
    use windows::Win32::UI::WindowsAndMessaging::{GUITHREADINFO, GetGUIThreadInfo};
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            return None;
        }
        let fg_tid = GetWindowThreadProcessId(fg, None);
        if fg_tid == 0 {
            return None;
        }
        let mut info = GUITHREADINFO { cbSize: size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        let focus = if GetGUIThreadInfo(fg_tid, &mut info).is_ok() && !info.hwndFocus.0.is_null() { info.hwndFocus } else { fg };
        let tid = match GetWindowThreadProcessId(focus, None) {
            0 => fg_tid,
            t => t,
        };
        let hkl = GetKeyboardLayout(tid);
        (!hkl.0.is_null()).then_some((focus.0 as isize, hkl.0 as isize))
    }
}

/// The short name Windows' own indicator writes for a layout's language: the three letters of ISO 639-2 in capitals ("ENG", "TUR",
/// "DEU"). When Windows has no such name for the language, the first letters of its locale name ("EN"); never empty for a layout
/// that names a language.
pub fn code(layout: isize) -> String {
    let lang = lang_id(layout);
    if lang == 0 {
        return String::new();
    }
    let mut name = [0u16; 85];
    let n = unsafe { LCIDToLocaleName(u32::from(lang), Some(&mut name), 0) };
    if n <= 1 {
        return String::new();
    }
    let mut out = [0u16; 16];
    let m = unsafe { GetLocaleInfoEx(windows::core::PCWSTR(name.as_ptr()), LOCALE_SISO639LANGNAME2, Some(&mut out)) };
    let three = if m > 1 { String::from_utf16_lossy(&out[..m as usize - 1]) } else { String::new() };
    short_code(&three, &String::from_utf16_lossy(&name[..n as usize - 1]))
}

/// [`code`] from what Windows said (pure): the ISO name when there is one, else the language part of the locale name.
pub fn short_code(iso3: &str, locale_name: &str) -> String {
    let iso = iso3.trim();
    let text = if iso.is_empty() { locale_name.split(['-', '_']).next().unwrap_or("").trim() } else { iso };
    text.chars().take(3).collect::<String>().to_uppercase()
}

/// The layout after `current` in `list`, round and round; the first when `current` is not among them. `None` when there is nothing
/// to switch to (fewer than two layouts).
pub fn next_layout(list: &[isize], current: isize) -> Option<isize> {
    if list.len() < 2 {
        return None;
    }
    let at = list.iter().position(|l| *l == current);
    Some(match at {
        Some(i) => list[(i + 1) % list.len()],
        None => list[0],
    })
}

/// What the indicator shows now.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Current {
    /// How many layouts the user has (the indicator is for two or more).
    pub layouts: usize,
    /// The short name of the layout the window in front types in; empty when there is no window in front or no name.
    pub code: String,
}

/// Reads [`Current`]. A few system calls, none of which waits for another program.
pub fn current() -> Current {
    let layouts = layouts().len();
    let code = if layouts >= 2 { of_foreground().map(|(_, l)| code(l)).unwrap_or_default() } else { String::new() };
    Current { layouts, code }
}

/// Asks the window with the keyboard in the window in front to type in the next layout. The short name of that layout when the
/// request was posted (the window's thread then switches; a window that does not take the message stays as it is, and the next
/// reading shows that).
pub fn switch_foreground_to_next() -> Option<String> {
    let (hwnd, layout) = of_foreground()?;
    let next = next_layout(&layouts(), layout)?;
    unsafe { PostMessageW(Some(HWND(hwnd as *mut _)), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(next)) }.ok()?;
    Some(code(next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_language_is_the_low_word_of_the_layout() {
        // Turkish Q: 0x041F041F; US English with the Turkish F layout loaded for it: 0xF0140409 (sign-extended on 64 bit)
        assert_eq!(lang_id(0x041F_041F), 0x041F);
        assert_eq!(lang_id(0xFFFF_FFFF_F014_0409_u64 as isize), 0x0409);
        assert_eq!(lang_id(0), 0);
    }

    #[test]
    fn the_short_name_is_three_capitals_and_falls_back_to_the_locale() {
        assert_eq!(short_code("eng", "en-US"), "ENG");
        assert_eq!(short_code("tur", "tr-TR"), "TUR");
        assert_eq!(short_code("", "tr-TR"), "TR");
        assert_eq!(short_code("  ", "sr-Cyrl-RS"), "SR");
        assert_eq!(short_code("", ""), "");
        assert_eq!(short_code("longer", "x"), "LON", "never wider than the button");
    }

    #[test]
    fn switching_goes_round_the_layouts_and_needs_two() {
        let (tr, en, de) = (0x041F_041F, 0x0409_0409, 0x0407_0407);
        assert_eq!(next_layout(&[tr, en, de], tr), Some(en));
        assert_eq!(next_layout(&[tr, en, de], de), Some(tr));
        assert_eq!(next_layout(&[tr, en], 0x1234), Some(tr), "a layout that is not in the list: the first");
        assert_eq!(next_layout(&[tr], tr), None);
        assert_eq!(next_layout(&[], tr), None);
    }

    /// On the PC the tests run on: the list is read, every layout in it has a short name, and reading the indicator does not fail
    /// whatever is in front.
    #[test]
    fn the_real_layouts_have_names() {
        let list = layouts();
        assert!(!list.is_empty(), "a Windows session has at least one layout");
        for l in &list {
            let c = code(*l);
            assert!((2..=3).contains(&c.chars().count()) && c == c.to_uppercase(), "{l:#x}: {c:?}");
        }
        let now = current();
        assert_eq!(now.layouts, list.len());
        assert!(now.code.chars().count() <= 3);
        assert!(now.layouts >= 2 || now.code.is_empty(), "with one layout there is nothing to show");
    }
}

#[cfg(test)]
mod manual {
    /// The keyboard's window in the window in front and its language, as the dock reads them (nothing is switched).
    #[test]
    #[ignore]
    fn manual_read_the_language_of_the_window_in_front() {
        let fg = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize;
        let got = super::of_foreground();
        println!("front {fg:#x}, keyboard's window and layout {got:x?}, code {:?}, layouts {}", got.map(|(_, l)| super::code(l)), super::layouts().len());
        assert!(got.is_some_and(|(_, l)| !super::code(l).is_empty()));
    }
}
