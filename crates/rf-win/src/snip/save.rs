//! Where a snip is saved and what the file is called, plus the write itself. The name and the choice of a free name are pure; the write
//! takes the folder as an argument, so the tests use a folder of their own and never the user's Pictures.

use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A wall-clock time to the second, as the name carries it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LocalTime {
    pub year: u16,
    pub month: u16,
    pub day: u16,
    pub hour: u16,
    pub minute: u16,
    pub second: u16,
}

/// The file's name: `Ekran alıntısı YYYY-AA-GG SSddss.png` (year, month, day; hour, minute, second), for 13 November 2026 at 02:15:30:
/// `Ekran alıntısı 2026-11-13 021530.png`.
pub fn file_name(t: &LocalTime) -> String {
    format!("Ekran al\u{131}nt\u{131}s\u{131} {:04}-{:02}-{:02} {:02}{:02}{:02}.png", t.year, t.month, t.day, t.hour, t.minute, t.second)
}

/// The name with " (2)", " (3)" ... before the extension, for the `n`th try (1 is the plain name).
pub fn numbered(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    match name.rsplit_once('.') {
        Some((stem, ext)) => format!("{stem} ({n}).{ext}"),
        None => format!("{name} ({n})"),
    }
}

/// How many names are tried before giving up: two snips in the same second are the only reason for a second try.
const TRIES: u32 = 200;

/// Writes `png` into `dir` (made if it is missing) under the name for `t`, or the next free one: a second snip within the same second
/// is `... (2).png`, never an overwrite of the first. Returns the path written. The file is created exclusively, so a name that appears
/// between the look and the write is skipped, and a write that fails leaves nothing behind.
pub fn save_png(dir: &Path, png: &[u8], t: &LocalTime) -> std::io::Result<PathBuf> {
    if png.is_empty() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "no picture"));
    }
    std::fs::create_dir_all(dir)?;
    let base = file_name(t);
    for n in 1..=TRIES {
        let path = dir.join(numbered(&base, n));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut f) => {
                if let Err(e) = f.write_all(png).and_then(|()| f.flush()) {
                    drop(f);
                    let _ = std::fs::remove_file(&path);
                    return Err(e);
                }
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(std::io::Error::new(std::io::ErrorKind::AlreadyExists, "no free name"))
}

/// `Pictures\Screenshots` of the user (the folder Windows' own snips go to). Only the path; it is made by [`save_png`]. Asks the shell
/// for the Pictures folder, so call it from a worker thread.
pub fn screenshots_dir() -> Option<PathBuf> {
    crate::known::KnownFolder::Pictures.path().map(|p| p.join("Screenshots"))
}

/// The local time now.
pub fn local_time() -> LocalTime {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    LocalTime { year: t.wYear, month: t.wMonth, day: t.wDay, hour: t.wHour, minute: t.wMinute, second: t.wSecond }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: LocalTime = LocalTime { year: 2026, month: 11, day: 13, hour: 2, minute: 15, second: 30 };

    /// A folder of the test's own, removed at the end.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("rf-snip-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_name_is_ekran_alintisi_then_the_date_and_the_time_without_separators() {
        assert_eq!(file_name(&T), "Ekran alıntısı 2026-11-13 021530.png");
        let late = LocalTime { year: 2027, month: 1, day: 2, hour: 23, minute: 5, second: 9 };
        assert_eq!(file_name(&late), "Ekran alıntısı 2027-01-02 230509.png");
        let midnight = LocalTime { year: 2026, month: 12, day: 31, hour: 0, minute: 0, second: 0 };
        assert_eq!(file_name(&midnight), "Ekran alıntısı 2026-12-31 000000.png");
    }

    #[test]
    fn a_second_try_is_numbered_before_the_extension() {
        let n = file_name(&T);
        assert_eq!(numbered(&n, 1), n);
        assert_eq!(numbered(&n, 2), "Ekran alıntısı 2026-11-13 021530 (2).png");
        assert_eq!(numbered(&n, 10), "Ekran alıntısı 2026-11-13 021530 (10).png");
        assert_eq!(numbered("noext", 3), "noext (3)");
    }

    #[test]
    fn the_file_is_written_into_a_folder_that_is_made_when_missing() {
        let s = Scratch::new("write");
        let dir = s.0.join("Pictures").join("Screenshots");
        assert!(!dir.exists());
        let p = save_png(&dir, b"\x89PNG-bytes", &T).unwrap();
        assert_eq!(p, dir.join("Ekran alıntısı 2026-11-13 021530.png"));
        assert_eq!(std::fs::read(&p).unwrap(), b"\x89PNG-bytes");
    }

    #[test]
    fn two_snips_in_the_same_second_are_two_files_and_the_first_is_never_overwritten() {
        let s = Scratch::new("twice");
        let a = save_png(&s.0, b"first", &T).unwrap();
        let b = save_png(&s.0, b"second", &T).unwrap();
        let c = save_png(&s.0, b"third", &T).unwrap();
        assert_ne!(a, b);
        assert_eq!(b.file_name().unwrap().to_string_lossy(), "Ekran alıntısı 2026-11-13 021530 (2).png");
        assert_eq!(c.file_name().unwrap().to_string_lossy(), "Ekran alıntısı 2026-11-13 021530 (3).png");
        assert_eq!(std::fs::read(&a).unwrap(), b"first");
        assert_eq!(std::fs::read(&b).unwrap(), b"second");
        assert_eq!(std::fs::read(&c).unwrap(), b"third");
    }

    #[test]
    fn nothing_is_written_for_an_empty_picture() {
        let s = Scratch::new("empty");
        assert!(save_png(&s.0, b"", &T).is_err());
        assert!(!s.0.exists(), "not even the folder");
    }

    #[test]
    fn a_folder_that_cannot_be_made_is_an_error_and_not_a_panic() {
        let s = Scratch::new("blocked");
        std::fs::create_dir_all(&s.0).unwrap();
        // a FILE where the folder should be
        let file = s.0.join("Screenshots");
        std::fs::write(&file, b"x").unwrap();
        assert!(save_png(&file, b"png", &T).is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"x", "the file in the way is untouched");
    }

    #[test]
    fn the_local_time_is_a_real_date() {
        let t = local_time();
        assert!((2024..2200).contains(&t.year) && (1..=12).contains(&t.month) && (1..=31).contains(&t.day));
        assert!(t.hour < 24 && t.minute < 60 && t.second < 62);
    }
}
