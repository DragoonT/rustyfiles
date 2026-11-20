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
