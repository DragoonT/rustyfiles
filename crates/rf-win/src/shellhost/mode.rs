//! In which mode does the shell host run? (shell plan 3: "beside Explorer" or "instead of Explorer".)
//!
//! The question is "who owns the shell window?" and it is answered by LOOKING, never by trusting an argument: `GetShellWindow()` is
//! the window of the process that is the shell (Explorer's `Progman`), and the image of that window's process says who it is. Explorer
//! owns it: [`ShellMode::Beside`]. Nobody owns it, or somebody else (including this very process): [`ShellMode::Shell`]. In this
//! increment `Shell` is only REPORTED; nothing is done differently because of it yet.
//!
//! A `--mode shell` on the command line (the guard passes one) is recorded by the caller as what was ASKED. It does not change the
//! answer here: a host that was told it is the shell while Explorer is plainly running would later act on Explorer's windows.

use std::path::PathBuf;

use crate::startup::command::path_key;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShellMode {
    /// Explorer owns the shell window: our dock, panels and desktop run beside it.
    Beside,
    /// Explorer does not: we are, or would be, the shell.
    Shell,
}

impl ShellMode {
    pub fn name(self) -> &'static str {
        match self {
            ShellMode::Beside => "beside",
            ShellMode::Shell => "shell",
        }
    }

    /// `--mode beside` / `--mode shell`, as typed.
    pub fn parse(s: &str) -> Option<ShellMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "beside" => Some(ShellMode::Beside),
            "shell" => Some(ShellMode::Shell),
            _ => None,
        }
    }
}

/// What is known of the process that owns a window. Program names only: this goes into a report that is read out loud.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub class: String,
    /// The file name of the image (`explorer.exe`); empty when it could not be read.
    pub program: String,
    /// The full path of the image, when it could be read.
    pub path: Option<String>,
    pub pid: u32,
    /// The window belongs to this very process.
    pub own_process: bool,
}

/// The decision. `windows_dir`: the Windows folder (`C:\Windows`).
///
/// Explorer is `<Windows>\explorer.exe`. When the path could not be read the name is trusted (Explorer's image can be read by its own
/// user, so an unreadable path is a stranger, but an Explorer that cannot be told from a copy by path is still better taken for
/// Explorer than the other way round: `Shell` is the mode that would later act in Explorer's place).
pub fn decide(shell_window_owner: Option<&Owner>, windows_dir: &str) -> ShellMode {
    let Some(o) = shell_window_owner else { return ShellMode::Shell };
    if o.own_process {
        return ShellMode::Shell;
    }
    let explorer = path_key(&format!("{}\\explorer.exe", windows_dir.trim_end_matches(['\\', '/'])));
    let is_explorer = match &o.path {
        Some(p) => path_key(p) == explorer,
        None => o.program.eq_ignore_ascii_case("explorer.exe"),
    };
    if is_explorer { ShellMode::Beside } else { ShellMode::Shell }
}

/// Who owns the shell window right now (`None`: nobody does).
pub fn shell_window_owner() -> Option<Owner> {
    use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
    let h = unsafe { GetShellWindow() };
    if h.0.is_null() {
        return None;
    }
    Some(owner_of(h.0 as isize))
}

/// Who owns the notification-area window `Shell_TrayWnd` (Explorer's taskbar; `None`: there is none).
pub fn tray_window_owner() -> Option<Owner> {
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::w;
    // FindWindow only searches; it sends nothing to the window it finds
    let h = unsafe { FindWindowW(w!("Shell_TrayWnd"), windows::core::PCWSTR::null()) }.ok()?;
    if h.0.is_null() {
        return None;
    }
    Some(owner_of(h.0 as isize))
}

/// The class and the owning process of a window. Read-only.
pub fn owner_of(hwnd: isize) -> Owner {
    use windows::Win32::Foundation::{CloseHandle, HWND};
    use windows::Win32::System::Threading::{GetCurrentProcessId, OpenProcess, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
    use windows::Win32::UI::WindowsAndMessaging::{GetClassNameW, GetWindowThreadProcessId};
    use windows::core::PWSTR;
    let h = HWND(hwnd as *mut _);
    let mut class = [0u16; 128];
    let n = unsafe { GetClassNameW(h, &mut class) };
    let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
    let mut path: Option<String> = None;
    if pid != 0
        && let Ok(p) = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
    {
        let mut buf = vec![0u16; 1024];
        let mut len = buf.len() as u32;
        if unsafe { QueryFullProcessImageNameW(p, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut len) }.is_ok() {
            path = Some(String::from_utf16_lossy(&buf[..len as usize]));
        }
        let _ = unsafe { CloseHandle(p) };
    }
    let program = path.as_deref().and_then(|p| PathBuf::from(p).file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
    Owner { class, program, path, pid, own_process: pid != 0 && pid == unsafe { GetCurrentProcessId() } }
}

/// The mode this host runs in, by looking.
pub fn detect() -> ShellMode {
    decide(shell_window_owner().as_ref(), &crate::startup::system::windows_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(program: &str, path: Option<&str>, own: bool) -> Owner {
        Owner { class: "Progman".into(), program: program.into(), path: path.map(str::to_string), pid: 1234, own_process: own }
    }

    #[test]
    fn explorer_owning_the_shell_window_is_beside() {
        let o = owner("explorer.exe", Some(r"C:\Windows\explorer.exe"), false);
        assert_eq!(decide(Some(&o), r"C:\Windows"), ShellMode::Beside);
        // spellings of the same path
        let o = owner("EXPLORER.EXE", Some(r"c:\windows\Explorer.EXE"), false);
        assert_eq!(decide(Some(&o), r"C:\Windows\"), ShellMode::Beside);
        let o = owner("explorer.exe", Some(r"\\?\C:\Windows\explorer.exe"), false);
        assert_eq!(decide(Some(&o), r"C:\Windows"), ShellMode::Beside);
    }

    #[test]
    fn nobody_owning_it_is_shell() {
        assert_eq!(decide(None, r"C:\Windows"), ShellMode::Shell);
    }

    #[test]
    fn another_program_or_ourselves_owning_it_is_shell() {
        assert_eq!(decide(Some(&owner("cairodesktop.exe", Some(r"C:\Cairo\CairoDesktop.exe"), false)), r"C:\Windows"), ShellMode::Shell);
        assert_eq!(decide(Some(&owner("rustyfiles.exe", Some(r"C:\x\rustyfiles.exe"), true)), r"C:\Windows"), ShellMode::Shell);
        // even an image called explorer.exe, if it is ours (a copy under that name)
        assert_eq!(decide(Some(&owner("explorer.exe", Some(r"C:\Windows\explorer.exe"), true)), r"C:\Windows"), ShellMode::Shell);
    }

    #[test]
    fn a_program_called_explorer_in_another_folder_is_not_explorer() {
        assert_eq!(decide(Some(&owner("explorer.exe", Some(r"D:\tools\explorer.exe"), false)), r"C:\Windows"), ShellMode::Shell);
    }

    #[test]
    fn an_owner_whose_path_could_not_be_read_is_judged_by_its_name() {
        assert_eq!(decide(Some(&owner("explorer.exe", None, false)), r"C:\Windows"), ShellMode::Beside);
        assert_eq!(decide(Some(&owner("", None, false)), r"C:\Windows"), ShellMode::Shell);
        assert_eq!(decide(Some(&owner("other.exe", None, false)), r"C:\Windows"), ShellMode::Shell);
    }

    #[test]
    fn the_mode_argument_is_understood_and_named() {
        assert_eq!(ShellMode::parse("shell"), Some(ShellMode::Shell));
        assert_eq!(ShellMode::parse(" Beside "), Some(ShellMode::Beside));
        assert_eq!(ShellMode::parse("both"), None);
        assert_eq!((ShellMode::Beside.name(), ShellMode::Shell.name()), ("beside", "shell"));
    }

    /// Read-only, windowless: the real question can be asked and answered.
    #[test]
    fn the_real_check_returns_without_side_effects() {
        let _ = detect();
        let _ = tray_window_owner();
    }
}
