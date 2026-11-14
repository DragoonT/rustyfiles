//! A program of rustyfiles does not keep its own installation folder as its working directory.
//!
//! WHY (2026-11-14, the user: "0.7.0 dan 0.7.2 ye güncellenmiyor"). The updater replaces the installed program by renaming its
//! folder (`...\rustyfiles\current`), and Windows refuses to rename a folder that is some process's working directory (error 32,
//! "used by another process"; tried in a scratch folder: a child whose working directory is the folder blocks the rename, and the
//! rename goes through the moment the child ends). The installed shortcuts start rustyfiles IN that folder ("Start in":
//! `...\current`), and so does the updater when it starts the program again. Everything the program starts without naming a folder
//! inherits it: the window shown while an update is applied (a copy of the program run from the temporary folder, so the updater's
//! own check for "processes running from the installation" does not see it), every document opened from a folder view, every app
//! started from the dock. The updater's log on the user's PC: five of six updates that day gave up after ten tries with "one or more
//! running processes prevented it" and started the old version again. Every version from 0.7.0-beta.1 (the first with the update
//! window) to 0.7.3 has this.
//!
//! THE RULE. First thing in `main`, a program whose working directory lies inside its own installation moves to a neutral folder
//! (the user's profile; the temporary folder when there is none). A working directory anywhere else is left alone: a terminal that
//! starts `rustyfiles .` means the folder it stands in, and relative arguments keep their meaning. Nothing in the program reads
//! files relative to the working directory.

use std::path::{Path, PathBuf};

/// A path's parts for a comparison: without the `\\?\` prefix, either slash, lower case (Windows paths do not tell cases apart).
fn parts(p: &Path) -> Vec<String> {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    s.split(['\\', '/']).filter(|c| !c.is_empty()).map(str::to_lowercase).collect()
}

/// Whether `dir` is `root` or lies under it.
pub fn is_inside(dir: &Path, root: &Path) -> bool {
    let (d, r) = (parts(dir), parts(root));
    !r.is_empty() && d.len() >= r.len() && d[..r.len()] == r[..]
}

/// The folder an update or an uninstall has to be able to move: the program's own folder, and when that is the updater's `current`
/// the whole installation above it.
pub fn install_root(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let is_current = dir.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("current"));
    match dir.parent() {
        Some(up) if is_current && up.parent().is_some() => Some(up.to_path_buf()),
        _ => Some(dir.to_path_buf()),
    }
}

/// Where the process goes instead: the user's profile folder when there is one, else the temporary folder.
pub fn neutral(profile: Option<PathBuf>, temp: PathBuf) -> PathBuf {
    profile.unwrap_or(temp)
}

/// Whether a process started from `exe` with the working directory `cwd` has to leave it (pure: the decision of [`leave`]).
pub fn must_leave(exe: &Path, cwd: &Path) -> bool {
    install_root(exe).is_some_and(|root| is_inside(cwd, &root))
}

/// Called first thing by every program of rustyfiles: leaves the installation folder when the process was started in it. Returns
/// the folder it moved to, `None` when it stays where it is (or cannot move: then nothing has changed).
pub fn leave() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let cwd = std::env::current_dir().ok()?;
    if !must_leave(&exe, &cwd) {
        return None;
    }
    let to = neutral(std::env::var_os("USERPROFILE").map(PathBuf::from).filter(|p| p.is_dir()), std::env::temp_dir());
    // (a neutral folder that is itself inside the installation would be no way out)
    if is_inside(&to, &install_root(&exe)?) {
        return None;
    }
    std::env::set_current_dir(&to).ok().map(|()| to)
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\x\AppData\Local\rustyfiles\current\rustyfiles.exe";

    #[test]
    fn the_installation_is_the_folder_above_current() {
        assert_eq!(install_root(Path::new(EXE)), Some(PathBuf::from(r"C:\Users\x\AppData\Local\rustyfiles")));
        assert_eq!(install_root(Path::new(r"C:\Users\x\AppData\Local\rustyfiles\CURRENT\directlook.exe")), Some(PathBuf::from(r"C:\Users\x\AppData\Local\rustyfiles")));
        // a portable copy or a build: the program's own folder
        assert_eq!(install_root(Path::new(r"D:\tools\rustyfiles\rustyfiles.exe")), Some(PathBuf::from(r"D:\tools\rustyfiles")));
        assert_eq!(install_root(Path::new(r"C:\dev\rustyfiles\target\release\rustyfiles.exe")), Some(PathBuf::from(r"C:\dev\rustyfiles\target\release")));
        // a folder called "current" at the top of a drive is not the updater's layout: the drive is never the installation
        assert_eq!(install_root(Path::new(r"C:\current\rustyfiles.exe")), Some(PathBuf::from(r"C:\current")));
    }

    #[test]
    fn a_process_started_in_its_installation_leaves_and_any_other_stays() {
        let exe = Path::new(EXE);
        // what the shortcut's "Start in" and the updater's restart give
        assert!(must_leave(exe, Path::new(r"C:\Users\x\AppData\Local\rustyfiles\current")));
        assert!(must_leave(exe, Path::new(r"C:\Users\x\AppData\Local\rustyfiles\current\")));
        assert!(must_leave(exe, Path::new(r"\\?\C:\Users\x\AppData\Local\rustyfiles\current")));
        assert!(must_leave(exe, Path::new(r"c:\users\X\appdata\local\RUSTYFILES\Current")), "cases are not told apart");
        assert!(must_leave(exe, Path::new(r"C:\Users\x\AppData\Local\rustyfiles")), "the installation itself: an uninstall removes it");
        assert!(must_leave(exe, Path::new(r"C:/Users/x/AppData/Local/rustyfiles/current")));
        // a terminal that starts `rustyfiles .` somewhere else means that folder
        assert!(!must_leave(exe, Path::new(r"C:\Users\x\Documents")));
        assert!(!must_leave(exe, Path::new(r"C:\Users\x\AppData\Local")));
        assert!(!must_leave(exe, Path::new(r"C:\Users\x\AppData\Local\rustyfiles-data")), "a folder that only begins with the same letters");
        assert!(!must_leave(exe, Path::new(r"D:\")));
        // the copy of the program that shows the update window runs from the temporary folder
        let copy = Path::new(r"C:\Users\x\AppData\Local\Temp\rustyfiles-update\rustyfiles-update.exe");
        assert!(!must_leave(copy, Path::new(r"C:\Users\x\AppData\Local\Temp")));
        assert!(must_leave(copy, Path::new(r"C:\Users\x\AppData\Local\Temp\rustyfiles-update")));
    }

    #[test]
    fn inside_means_the_folder_or_below_it() {
        assert!(is_inside(Path::new(r"C:\a\b"), Path::new(r"C:\a")));
        assert!(is_inside(Path::new(r"C:\a"), Path::new(r"C:\a")));
        assert!(!is_inside(Path::new(r"C:\a"), Path::new(r"C:\a\b")));
        assert!(!is_inside(Path::new(r"C:\ab"), Path::new(r"C:\a")));
        assert!(!is_inside(Path::new(r"C:\a"), Path::new("")), "no folder holds everything");
    }

    #[test]
    fn the_way_out_is_the_profile_and_the_temporary_folder_without_one() {
        assert_eq!(neutral(Some(PathBuf::from(r"C:\Users\x")), PathBuf::from(r"C:\T")), PathBuf::from(r"C:\Users\x"));
        assert_eq!(neutral(None, PathBuf::from(r"C:\T")), PathBuf::from(r"C:\T"));
    }

    /// The reason for this module, on the system the tests run on: a folder that is a process's working directory cannot be renamed,
    /// and can be the moment that process is gone. (A child process stands in the folder: the test process's own working directory is
    /// one value for all tests of this binary and is not touched.)
    #[test]
    fn a_folder_that_is_a_working_directory_cannot_be_renamed_until_the_process_is_gone() {
        let base = std::env::temp_dir().join(format!("rf-ownfolder-{}", std::process::id()));
        let (here, moved) = (base.join("here"), base.join("moved"));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&here).unwrap();
        // a child that only waits (a few seconds at most; it is ended below)
        // (one process and no shell around it: a shell's own child would inherit the folder and outlive the shell)
        let mut child = std::process::Command::new("ping")
            .args(["-n", "6", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .current_dir(&here)
            .spawn()
            .expect("ping starts");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let blocked = std::fs::rename(&here, &moved).is_err();
        let _ = child.kill();
        let _ = child.wait();
        // (the folder is free a moment after the process has ended)
        let free = (0..20).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(100));
            std::fs::rename(&here, &moved).is_ok()
        });
        let _ = std::fs::remove_dir_all(&base);
        assert!(blocked, "Windows renamed a folder that was a working directory: the reason for this module is gone");
        assert!(free, "the folder could not be renamed after the process had ended");
    }
}
