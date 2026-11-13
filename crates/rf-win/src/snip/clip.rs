//! Puts a snip on the clipboard as a picture: `PNG` (registered), `CF_DIBV5` and `CF_DIB`, in that order of preference (see `encode.rs`).
//!
//! The ORDER OF CALLS (open, empty, set each format, close) and what is done when one step fails are in [`write`], over the small
//! [`ClipApi`] trait; the tests run it against a fake that records the calls in memory. The real implementation, [`SystemClipboard`], is
//! never run by a test or a probe: writing the user's clipboard is not something a test may do.
//!
//! THE OWNER WINDOW. `OpenClipboard(NULL)` followed by `EmptyClipboard` leaves the clipboard with no owner, and `SetClipboardData` then
//! fails. So [`SystemClipboard`] makes a message-only window of its own on the calling thread for the time it needs (not one of the
//! overlay's windows: those close the moment the snip is taken, while the write runs on a worker thread). The data is handed over
//! rendered (no delayed rendering), so it stays on the clipboard after the window is destroyed.

use std::time::Duration;

use super::encode::Formats;

/// A format this module writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// The registered format named "PNG".
    Png,
    /// `CF_DIBV5`.
    DibV5,
    /// `CF_DIB`.
    Dib,
}

/// What the clipboard is asked to do. Every step may fail; the message is for the log.
pub trait ClipApi {
    fn open(&mut self) -> Result<(), String>;
    fn empty(&mut self) -> Result<(), String>;
    fn set(&mut self, format: Format, bytes: &[u8]) -> Result<(), String>;
    fn close(&mut self);
}

/// How a write went.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Written {
    /// Every format is on the clipboard.
    All,
    /// Some formats are there and some were refused: the picture can still be pasted (by the programs that read the ones that are
    /// there). The formats that failed.
    Partly(Vec<Format>),
}

/// The write. The clipboard is closed whatever happens once it was opened. An error leaves the previous content in place only if the
/// failure came before the clipboard was emptied (the open): a failure after that has already replaced it.
pub fn write(api: &mut dyn ClipApi, f: &Formats) -> Result<Written, String> {
    if !f.is_complete() {
        return Err("the picture could not be encoded".to_string());
    }
    api.open().map_err(|e| format!("the clipboard could not be opened: {e}"))?;
    let result = (|| {
        api.empty().map_err(|e| format!("the clipboard could not be emptied: {e}"))?;
        let mut failed = Vec::new();
        for (format, bytes) in [(Format::Png, &f.png), (Format::DibV5, &f.dibv5), (Format::Dib, &f.dib)] {
            if let Err(e) = api.set(format, bytes) {
                log::warn!("snip: {format:?} was not put on the clipboard: {e}");
                failed.push(format);
            }
        }
        match failed.len() {
            0 => Ok(Written::All),
            3 => Err("no format could be put on the clipboard".to_string()),
            _ => Ok(Written::Partly(failed)),
        }
    })();
    api.close();
    result
}

/// How long the real clipboard is waited for when another program holds it: another program may have it open for a moment (a
/// clipboard manager reading what just changed), and a snip that fails for that is the kind of failure a second try never needs.
const OPEN_TRIES: u32 = 20;
const OPEN_PAUSE: Duration = Duration::from_millis(25);

/// The real clipboard. Make it on the thread that writes and drop it there.
pub struct SystemClipboard {
    owner: windows::Win32::Foundation::HWND,
    png: u32,
    opened: bool,
}

impl SystemClipboard {
    pub fn new() -> Result<SystemClipboard, String> {
        use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WS_POPUP};
        use windows::core::w;
        let owner = unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("rustyfiles-snip-clipboard"), WS_POPUP, 0, 0, 0, 0, Some(HWND_MESSAGE), None, None, None) }
            .map_err(|e| format!("no owner window for the clipboard: {e}"))?;
        let png = unsafe { RegisterClipboardFormatW(w!("PNG")) };
        if png == 0 {
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(owner);
            }
            return Err("the PNG clipboard format could not be registered".to_string());
        }
        Ok(SystemClipboard { owner, png, opened: false })
    }
}

impl Drop for SystemClipboard {
    fn drop(&mut self) {
        self.close();
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.owner);
        }
    }
}

impl ClipApi for SystemClipboard {
    fn open(&mut self) -> Result<(), String> {
        use windows::Win32::System::DataExchange::OpenClipboard;
        let mut last = String::new();
        for _ in 0..OPEN_TRIES {
            match unsafe { OpenClipboard(Some(self.owner)) } {
                Ok(()) => {
                    self.opened = true;
                    return Ok(());
                }
                Err(e) => last = e.to_string(),
            }
            std::thread::sleep(OPEN_PAUSE);
        }
        Err(last)
    }

    fn empty(&mut self) -> Result<(), String> {
        unsafe { windows::Win32::System::DataExchange::EmptyClipboard() }.map_err(|e| e.to_string())
    }

    fn set(&mut self, format: Format, bytes: &[u8]) -> Result<(), String> {
        use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
        use windows::Win32::System::DataExchange::SetClipboardData;
        use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
        const CF_DIB: u32 = 8;
        const CF_DIBV5: u32 = 17;
        let id = match format {
            Format::Png => self.png,
            Format::DibV5 => CF_DIBV5,
            Format::Dib => CF_DIB,
        };
        unsafe {
            let h: HGLOBAL = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| e.to_string())?;
            let p = GlobalLock(h) as *mut u8;
            if p.is_null() {
                let _ = GlobalFree(Some(h));
                return Err("GlobalLock".to_string());
            }
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            let _ = GlobalUnlock(h);
            // (after a successful SetClipboardData the memory belongs to the system; after a failed one it is still ours)
            match SetClipboardData(id, Some(HANDLE(h.0))) {
                Ok(_) => Ok(()),
                Err(e) => {
                    let _ = GlobalFree(Some(h));
                    Err(e.to_string())
                }
            }
        }
    }

    fn close(&mut self) {
        if self.opened {
            self.opened = false;
            unsafe {
                let _ = windows::Win32::System::DataExchange::CloseClipboard();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snip::encode::Formats;
    use crate::snip::frame::Frame;

    /// Records the calls; fails the steps it is told to.
    #[derive(Default)]
    struct Fake {
        calls: Vec<String>,
        fail_open: bool,
        fail_empty: bool,
        fail: Vec<Format>,
        stored: Vec<(Format, Vec<u8>)>,
        is_open: bool,
    }

    impl ClipApi for Fake {
        fn open(&mut self) -> Result<(), String> {
            self.calls.push("open".into());
            if self.fail_open {
                return Err("busy".into());
            }
            self.is_open = true;
            Ok(())
        }
        fn empty(&mut self) -> Result<(), String> {
            self.calls.push("empty".into());
            if self.fail_empty { Err("denied".into()) } else { Ok(()) }
        }
        fn set(&mut self, format: Format, bytes: &[u8]) -> Result<(), String> {
            self.calls.push(format!("set {format:?}"));
            assert!(self.is_open, "a format was set on a closed clipboard");
            if self.fail.contains(&format) {
                return Err("refused".into());
            }
            self.stored.push((format, bytes.to_vec()));
            Ok(())
        }
        fn close(&mut self) {
            self.calls.push("close".into());
            self.is_open = false;
        }
    }

    fn formats() -> Formats {
        let f = Frame { width: 2, height: 2, bgra: (0..16).map(|i| i as u8 * 10).collect() };
        Formats::of(&f)
    }

    #[test]
    fn the_calls_are_open_empty_png_dibv5_dib_close_in_that_order() {
        let mut api = Fake::default();
        let fm = formats();
        assert_eq!(write(&mut api, &fm), Ok(Written::All));
        assert_eq!(api.calls, ["open", "empty", "set Png", "set DibV5", "set Dib", "close"]);
        // each format got its own bytes, whole
        assert_eq!(api.stored[0], (Format::Png, fm.png.clone()));
        assert_eq!(api.stored[1], (Format::DibV5, fm.dibv5.clone()));
        assert_eq!(api.stored[2], (Format::Dib, fm.dib.clone()));
    }

    #[test]
    fn a_clipboard_that_cannot_be_opened_is_an_error_before_anything_is_emptied() {
        let mut api = Fake { fail_open: true, ..Default::default() };
        let e = write(&mut api, &formats()).unwrap_err();
        assert!(e.contains("could not be opened") && e.contains("busy"), "{e}");
        assert_eq!(api.calls, ["open"], "the old content was not touched, and there is nothing to close");
    }

    #[test]
    fn a_clipboard_that_cannot_be_emptied_is_closed_again() {
        let mut api = Fake { fail_empty: true, ..Default::default() };
        assert!(write(&mut api, &formats()).is_err());
        assert_eq!(api.calls, ["open", "empty", "close"]);
    }

    #[test]
    fn one_refused_format_leaves_the_others_and_says_which() {
        let mut api = Fake { fail: vec![Format::Png], ..Default::default() };
        assert_eq!(write(&mut api, &formats()), Ok(Written::Partly(vec![Format::Png])));
        assert_eq!(api.stored.len(), 2);
        assert_eq!(api.calls.last().map(String::as_str), Some("close"));
    }

    #[test]
    fn when_every_format_is_refused_it_is_an_error_and_the_clipboard_is_closed() {
        let mut api = Fake { fail: vec![Format::Png, Format::DibV5, Format::Dib], ..Default::default() };
        assert!(write(&mut api, &formats()).is_err());
        assert_eq!(api.calls.last().map(String::as_str), Some("close"));
        assert!(!api.is_open);
    }

    #[test]
    fn nothing_is_opened_for_a_picture_that_was_not_made() {
        let mut api = Fake::default();
        assert!(write(&mut api, &Formats::default()).is_err());
        assert!(api.calls.is_empty(), "the clipboard is not touched for nothing");
    }

    #[test]
    fn the_real_clipboard_is_never_opened_by_a_test_in_this_file() {
        // (the needle is built at run time so that this test does not contain it)
        let src = include_str!("clip.rs");
        let tests = src.split("#[cfg(test)]").nth(1).unwrap_or_default();
        assert!(!tests.contains(&["System", "Clipboard"].concat()), "a test uses the real clipboard");
    }
}
