//! Where the guard looks for its files and the host. Pure: the environment is passed in.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::statefile::ShellStateFile;

pub const HOST_FILE: &str = "rustyfiles.exe";

/// The folder holding `shell-state.json`: `--state-dir`, else `RUSTYFILES_CONFIG_DIR` (tests, portable use),
/// else `%APPDATA%\rustyfiles`, the same place the app keeps its settings.
pub fn state_dir(arg: Option<&Path>, env: &dyn Fn(&str) -> Option<OsString>) -> PathBuf {
    if let Some(d) = arg.filter(|d| !d.as_os_str().is_empty()) {
        return d.to_path_buf();
    }
    if let Some(d) = env("RUSTYFILES_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(d);
    }
    match env("APPDATA").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d).join("rustyfiles"),
        // No APPDATA (it should always be set at sign-in): next to the guard.
        None => PathBuf::from("."),
    }
}

/// A `--test-shell` run never touches the user's real state folder (`%APPDATA%\rustyfiles` holds the crash log and
/// the fall-back record of the real switch): without an explicit `--state-dir` it gets a folder of its own under the
/// temp folder. `RUSTYFILES_CONFIG_DIR` is not consulted either, because that points at a copy of the real settings.
pub fn choose_state_dir(arg: Option<&Path>, test_mode: bool, env: &dyn Fn(&str) -> Option<OsString>, temp: &Path, pid: u32) -> PathBuf {
    if !test_mode {
        return state_dir(arg, env);
    }
    match arg.filter(|d| !d.as_os_str().is_empty()) {
        Some(d) => d.to_path_buf(),
        None => temp.join(format!("rustyfiles-shellguard-test-{pid}")),
    }
}

/// The shell host program: `--host`, else the `host_exe` key the Settings switch wrote, else `rustyfiles.exe`
/// in the guard's own folder.
pub fn host_exe(arg: Option<&Path>, state: &ShellStateFile, current_exe: Option<&Path>) -> PathBuf {
    if let Some(p) = arg {
        return p.to_path_buf();
    }
    if let Some(p) = state.host_exe() {
        return PathBuf::from(p);
    }
    current_exe.and_then(Path::parent).map(|d| d.join(HOST_FILE)).unwrap_or_else(|| PathBuf::from(HOST_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |k| pairs.iter().find(|(n, _)| *n == k).map(|(_, v)| OsString::from(v))
    }

    #[test]
    fn state_dir_order() {
        let env = env_of(&[("RUSTYFILES_CONFIG_DIR", "C:\\cfg"), ("APPDATA", "C:\\roaming")]);
        assert_eq!(state_dir(Some(Path::new("D:\\x")), &env), PathBuf::from("D:\\x"));
        assert_eq!(state_dir(None, &env), PathBuf::from("C:\\cfg"));
        let env = env_of(&[("APPDATA", "C:\\roaming")]);
        assert_eq!(state_dir(None, &env), PathBuf::from("C:\\roaming").join("rustyfiles"));
        let env = env_of(&[("RUSTYFILES_CONFIG_DIR", ""), ("APPDATA", "C:\\roaming")]);
        assert_eq!(state_dir(Some(Path::new("")), &env), PathBuf::from("C:\\roaming").join("rustyfiles"));
        assert_eq!(state_dir(None, &env_of(&[])), PathBuf::from("."));
    }

    #[test]
    fn test_mode_never_picks_the_real_state_folder() {
        let env = env_of(&[("RUSTYFILES_CONFIG_DIR", "C:\\cfg"), ("APPDATA", "C:\\roaming")]);
        let temp = Path::new("C:\\tmp");
        assert_eq!(choose_state_dir(None, true, &env, temp, 77), PathBuf::from("C:\\tmp").join("rustyfiles-shellguard-test-77"));
        assert_eq!(choose_state_dir(Some(Path::new("")), true, &env, temp, 77), PathBuf::from("C:\\tmp").join("rustyfiles-shellguard-test-77"));
        assert_eq!(choose_state_dir(Some(Path::new("D:\\mine")), true, &env, temp, 77), PathBuf::from("D:\\mine"));
        // Outside test mode the order is the old one.
        assert_eq!(choose_state_dir(None, false, &env, temp, 77), PathBuf::from("C:\\cfg"));
        assert_eq!(choose_state_dir(Some(Path::new("D:\\x")), false, &env, temp, 77), PathBuf::from("D:\\x"));
    }

    #[test]
    fn host_order() {
        let mut state = ShellStateFile::empty();
        let exe = Path::new("C:\\g\\guard.exe");
        assert_eq!(host_exe(None, &state, Some(exe)), PathBuf::from("C:\\g").join(HOST_FILE));
        state.set_host_exe("C:\\app\\rustyfiles.exe");
        assert_eq!(host_exe(None, &state, Some(exe)), PathBuf::from("C:\\app\\rustyfiles.exe"));
        assert_eq!(host_exe(Some(Path::new("X.exe")), &state, Some(exe)), PathBuf::from("X.exe"));
        assert_eq!(host_exe(None, &ShellStateFile::empty(), None), PathBuf::from(HOST_FILE));
    }
}
