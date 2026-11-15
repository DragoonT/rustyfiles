//! Which Windows this is, for the few things that are not there on every one.
//!
//! WHY IT EXISTS (issue 24, a user on Windows 10 LTSC 21H2, build 19044): the program asked for Windows 11's window materials
//! everywhere and never looked whether Windows has them. Mica, Mica Alt and the Acrylic of flyouts are one DWM attribute
//! (`DWMWA_SYSTEMBACKDROP_TYPE`) that exists from Windows 11 22H2 (build 22621) on. Below that our toolkit does nothing for Mica
//! (the window stays see-through under colours made to lie on Mica) and falls back to the old blur-behind for Acrylic, which Windows
//! draws over the WHOLE window rectangle: the invisible resize border of a window and the frame of a popup included, so every window
//! and menu stood in a blurred box.

use std::sync::OnceLock;

/// The first build that has `DWMWA_SYSTEMBACKDROP_TYPE` (Windows 11 22H2).
pub const SYSTEM_BACKDROP_BUILD: u32 = 22621;

/// Windows' build number (19044 for Windows 10 21H2, 22621 for Windows 11 22H2), 0 when it cannot be read. Read once.
///
/// From the registry and not `GetVersionEx`, which answers 9200 to a program without a matching manifest entry.
pub fn build() -> u32 {
    static BUILD: OnceLock<u32> = OnceLock::new();
    *BUILD.get_or_init(|| read_build().unwrap_or(0))
}

/// Whether this Windows gives a window a material (Mica, Mica Alt, Acrylic). An unreadable build counts as "yes": the program is
/// made for Windows 11, and the plain look is the exception to be sure of.
pub fn system_backdrops() -> bool {
    has_system_backdrops(build())
}

/// [`system_backdrops`] for a build number. Pure.
pub fn has_system_backdrops(build: u32) -> bool {
    build == 0 || build >= SYSTEM_BACKDROP_BUILD
}

fn read_build() -> Option<u32> {
    use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    let key = crate::wide::wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let value = crate::wide::wide("CurrentBuild");
    let mut buf = [0u16; 32];
    let mut len = (buf.len() * 2) as u32;
    let ok = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            windows::core::PCWSTR(key.as_ptr()),
            windows::core::PCWSTR(value.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut len),
        )
    };
    if ok.is_err() {
        return None;
    }
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materials_start_with_windows_11_22h2() {
        for old in [17763, 19041, 19044, 19045, 22000] {
            assert!(!has_system_backdrops(old), "{old}");
        }
        for new in [22621, 22631, 26100, 26300] {
            assert!(has_system_backdrops(new), "{new}");
        }
        assert!(has_system_backdrops(0), "an unreadable build is taken for a Windows that has them");
    }

    #[test]
    fn this_pc_tells_its_build() {
        assert!(build() >= 10240, "a Windows 10 or later build number: {}", build());
    }
}
