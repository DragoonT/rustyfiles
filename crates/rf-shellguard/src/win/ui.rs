//! The guard's only UI: one message box, in the Windows display language (Turkish or English).
//! NOT RUN by the author of this crate (it opens a window).

use windows::Win32::Globalization::GetUserDefaultUILanguage;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONWARNING, MB_OK, MB_SETFOREGROUND, MessageBoxW};
use windows::core::PCWSTR;

use super::wide;
use crate::machine::Message;
use crate::messages::{self, Lang};
use crate::sys::Ui;

pub struct RealUi;

fn lang() -> Lang {
    // SAFETY: a plain query. The primary language is the low 10 bits; Turkish is 0x1f.
    let id = unsafe { GetUserDefaultUILanguage() };
    if id & 0x3ff == 0x1f { Lang::Tr } else { Lang::En }
}

impl Ui for RealUi {
    fn message(&mut self, m: Message) {
        let body = messages::body(m, lang());
        if body.is_empty() {
            return;
        }
        let (text, title) = (wide(&body), wide(messages::TITLE));
        // SAFETY: both strings are NUL-terminated. Blocks until the user closes it.
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONWARNING | MB_SETFOREGROUND);
        }
    }
}
