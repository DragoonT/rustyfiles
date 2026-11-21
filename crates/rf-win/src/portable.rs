//! The portable copy keeps its settings next to itself.
//!
//! WHY (issue 71: "The portable version should save the settings in the same folder as the executable and not inside the system's
//! AppData/Roaming folder"). Velopack's portable zip is a folder with `Update.exe`, a `.portable` file, and the program in `current\`;
//! an update replaces `current\` and nothing else. So the data goes into `data\` beside `current\`: it moves with the folder (a USB
//! stick, another PC) and survives updates.
//!
//! The first time the folder is made, the files the program had kept in `%APPDATA%\rustyfiles` until now (its settings, layouts,
//! sessions) are COPIED into it, so a portable copy that ran before keeps what it had. Folders (the file index, the jobs) are not
//! copied: they are made again. Nothing is moved or deleted in AppData.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The data folder of a portable copy started from `exe`: `<root>\data` when `exe` is in `<root>\current` and `<root>\.portable`
/// exists (`is_file` says whether a path is a file).
pub fn data_dir_for(exe: &Path, is_file: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let current = exe.parent()?;
    if !current.file_name()?.to_string_lossy().eq_ignore_ascii_case("current") {
        return None;
    }
    let root = current.parent()?;
    is_file(&root.join(".portable")).then(|| root.join("data"))
}

/// A file bigger than this is not copied from AppData (none of the settings files comes near it).
const COPY_AT_MOST: u64 = 20 << 20;

/// Copies the files (not the folders) of `from` into `to`, which is made.
fn first_fill(from: &Path, to: &Path) {
    if std::fs::create_dir_all(to).is_err() {
        return;
    }
    let Ok(rd) = std::fs::read_dir(from) else { return };
    for e in rd.flatten() {
        let Ok(meta) = e.metadata() else { continue };
        if meta.is_file() && meta.len() <= COPY_AT_MOST {
            let _ = std::fs::copy(e.path(), to.join(e.file_name()));
        }
    }
}

/// This program's data folder when it runs as the portable copy (made, and filled from AppData the first time), else `None`.
pub fn data_dir() -> Option<PathBuf> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok()?;
        let dir = data_dir_for(&exe, &|p| p.is_file())?;
        if !dir.exists() {
            if let Some(old) = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("rustyfiles")) {
                first_fill(&old, &dir);
            }
            let _ = std::fs::create_dir_all(&dir);
        }
        Some(dir)
    })
    .clone()
}

/// The portable copy's own launcher, `<root>\rustyfiles.exe`: Velopack's stub beside `current\`, which an update leaves where it is.
/// `None` for an installed copy, or a portable folder without one.
pub fn launcher() -> Option<PathBuf> {
    data_dir()?;
    let exe = std::env::current_exe().ok()?;
    let stub = exe.parent()?.parent()?.join(exe.file_name()?);
    stub.is_file().then_some(stub)
}

/// Gives a window of the portable copy what the taskbar pins: the launcher ([`launcher`]), not the running program.
///
/// WHY (issue 184: "with each update the pinning goes away", on the portable copy). A pinned window is pinned as its program,
/// `current\rustyfiles.exe`; an update replaces `current\`, and Windows drops a pin whose file went away. The installed copy has
/// Velopack's Start menu shortcut, which the taskbar pins instead (same AppUserModelID); the portable copy has none. With the
/// window's relaunch command set to the launcher, the pin is a shortcut to the launcher, which every update keeps.
pub fn mark_window(hwnd: isize, app_id: &str) {
    let Some(stub) = launcher() else { return };
    let Ok(exe) = std::env::current_exe() else { return };
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Storage::EnhancedStorage::{
        PKEY_AppUserModel_ID, PKEY_AppUserModel_RelaunchCommand, PKEY_AppUserModel_RelaunchDisplayNameResource,
        PKEY_AppUserModel_RelaunchIconResource,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    let command = format!("\"{}\"", stub.display());
    let icon = format!("{},0", exe.display());
    let set = || -> windows::core::Result<()> {
        let store: IPropertyStore = unsafe { SHGetPropertyStoreForWindow(HWND(hwnd as *mut _))? };
        unsafe {
            store.SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(app_id))?;
            store.SetValue(&PKEY_AppUserModel_RelaunchCommand, &PROPVARIANT::from(command.as_str()))?;
            store.SetValue(&PKEY_AppUserModel_RelaunchDisplayNameResource, &PROPVARIANT::from("rustyfiles"))?;
            store.SetValue(&PKEY_AppUserModel_RelaunchIconResource, &PROPVARIANT::from(icon.as_str()))?;
            store.Commit()
        }
    };
    if let Err(e) = set() {
        log::warn!("portable: the window's relaunch command could not be set: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_copy_in_a_portable_folder_has_a_data_folder_of_its_own() {
        let marker = |root: &'static str| move |p: &Path| p == Path::new(root).join(".portable");
        let exe = Path::new(r"E:\Tools\rustyfiles\current\rustyfiles.exe");
        assert_eq!(data_dir_for(exe, &marker(r"E:\Tools\rustyfiles")), Some(PathBuf::from(r"E:\Tools\rustyfiles\data")));
        assert_eq!(data_dir_for(exe, &|_| false), None, "an installed copy has no .portable beside current");
        assert_eq!(data_dir_for(Path::new(r"E:\build\rustyfiles.exe"), &|_| true), None, "a build is not in current");
    }

    #[test]
    fn the_first_fill_copies_the_files_and_leaves_the_folders() {
        let base = std::env::temp_dir().join(format!("rf-portable-{}", std::process::id()));
        let (from, to) = (base.join("appdata"), base.join("data"));
        std::fs::create_dir_all(from.join("index")).unwrap();
        std::fs::write(from.join("settings.toml"), b"theme = \"dark\"").unwrap();
        std::fs::write(from.join("index").join("c.idx"), b"big").unwrap();
        first_fill(&from, &to);
        assert_eq!(std::fs::read(to.join("settings.toml")).unwrap(), b"theme = \"dark\"");
        assert!(!to.join("index").exists());
        assert!(from.join("settings.toml").exists(), "nothing leaves AppData");
        let _ = std::fs::remove_dir_all(base);
    }
}
