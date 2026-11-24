//! Type-specific file actions from Files' context menu: set as desktop background, install a
//! font (for the current user — no administrator rights, no UAC). Blocking: off the UI thread.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::AddFontResourceW;
use windows::Win32::UI::WindowsAndMessaging::{
    HWND_BROADCAST, SMTO_ABORTIFHUNG, SPI_SETDESKWALLPAPER, SPIF_SENDCHANGE, SPIF_UPDATEINIFILE, SendMessageTimeoutW,
    SystemParametersInfoW, WM_FONTCHANGE,
};
use windows::core::PCWSTR;

use crate::wide::wide;

pub fn is_image(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "jpg" | "jpeg" | "png" | "bmp" | "gif" | "tif" | "tiff" | "webp" | "jfif" | "heic" | "avif" | "jxl")
}

pub fn is_font(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "ttf" | "otf" | "ttc" | "fon")
}

/// Executables that "Run as administrator" applies to. (Not a `.msi`: Windows gives it no such verb, see [`is_msi`].)
pub fn is_runnable(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "exe" | "bat" | "cmd" | "ps1" | "com")
}

/// A Windows Installer package: its verbs are install ("open"), repair and uninstall.
pub fn is_msi(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("msi"))
}

/// A disk image Windows can mount as a drive.
pub fn is_disk_image(p: &Path) -> bool {
    let e = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    matches!(e.as_str(), "iso" | "img" | "vhd" | "vhdx")
}

/// Whether Windows has this verb for the file's type as it is registered now (a `.iso` loses "mount" when another program takes
/// the type over, and Explorer's menu loses it too). Registry reads only.
pub fn has_verb(p: &Path, verb: &str) -> bool {
    use windows::Win32::UI::Shell::{ASSOCF_NONE, ASSOCSTR_COMMAND, ASSOCSTR_DELEGATEEXECUTE, AssocQueryStringW};
    let Some(ext) = p.extension().map(|e| format!(".{}", e.to_string_lossy())) else { return false };
    let (ext, verb) = (wide(&ext), wide(verb));
    // (a verb is a command line, or a class that runs it: "mount" is the second kind)
    [ASSOCSTR_COMMAND, ASSOCSTR_DELEGATEEXECUTE].into_iter().any(|what| {
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        // SAFETY: both strings end in a zero and `len` is the buffer's size in characters
        let hr = unsafe { AssocQueryStringW(ASSOCF_NONE, what, PCWSTR(ext.as_ptr()), PCWSTR(verb.as_ptr()), Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len) };
        hr == windows::Win32::Foundation::S_OK && buf[0] != 0
    })
}

pub fn set_wallpaper(path: &Path) -> anyhow::Result<()> {
    let mut w = wide(path);
    unsafe {
        SystemParametersInfoW(SPI_SETDESKWALLPAPER, 0, Some(w.as_mut_ptr() as *mut _), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)?;
    }
    Ok(())
}

/// Installs a font for the current user: copies it into
/// `%LOCALAPPDATA%\Microsoft\Windows\Fonts`, registers it under HKCU and tells running apps.
/// Returns the installed copy.
pub fn install_font_for_user(path: &Path) -> anyhow::Result<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow::anyhow!("LOCALAPPDATA yok"))?;
    let dir = PathBuf::from(base).join("Microsoft").join("Windows").join("Fonts");
    std::fs::create_dir_all(&dir)?;
    let name = path.file_name().ok_or_else(|| anyhow::anyhow!("geçersiz ad"))?;
    let dest = dir.join(name);
    if !dest.exists() {
        std::fs::copy(path, &dest)?;
    }
    // registry value name: the family as Windows would show it is not known without parsing the
    // font; the file stem plus the type works (Windows reads the real names from the file)
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let kind = match path.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("otf") => "(OpenType)",
        _ => "(TrueType)",
    };
    let key = windows_registry::CURRENT_USER.create(r"Software\Microsoft\Windows NT\CurrentVersion\Fonts")?;
    key.set_string(&format!("{stem} {kind}"), dest.to_string_lossy().as_ref())?;
    let w = wide(&dest);
    unsafe {
        AddFontResourceW(PCWSTR(w.as_ptr()));
        let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_FONTCHANGE, WPARAM(0), LPARAM(0), SMTO_ABORTIFHUNG, 1000, None);
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_verb_is_found_only_when_the_type_has_it() {
        // every Windows gives a program "run as administrator", and nothing gives it a verb of this name
        assert!(has_verb(Path::new("a.exe"), "runas"));
        assert!(!has_verb(Path::new("a.exe"), "rustyfiles-no-such-verb"));
        assert!(!has_verb(Path::new("no-extension"), "open"));
        // (what this PC answers for a disk image depends on which program owns the type)
        println!("mount for .iso: {}, .vhdx: {}", has_verb(Path::new("a.iso"), "mount"), has_verb(Path::new("a.vhdx"), "mount"));
    }
}
