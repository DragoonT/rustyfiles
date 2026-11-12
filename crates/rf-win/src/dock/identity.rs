//! Who a running window belongs to, in the terms a pin of the Windows taskbar is written in.
//!
//! A pin of a packaged (Store or MSIX) app is its AppUserModelID, `Package.Name_publisherid!AppId`. The windows of such an app often
//! do not carry that id as a property of their own (Telegram, Spotify, WhatsApp, Claude, Windows Terminal do not): the PROCESS does,
//! through its package. Taskbar groups made from the window's property alone give those apps a group named after the program's path
//! under `WindowsApps`, which no pin matches - the dock then drew the app twice (the pin, and the running group), and drew the
//! program's icon from a path the shell cannot read (the generic white window) instead of the package's logo.
//!
//! So the id of the process's package is asked for each task window that sets none, before the windows are grouped. Only reads:
//! `GetApplicationUserModelId` on a process handle that can do nothing but be asked about itself.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

use crate::tasklist::{self, Task};

// kernel32 has had these since Windows 8; the `windows` crate feature that wraps them is not enabled here, and two read-only calls
// do not justify one.
unsafe extern "system" {
    fn GetApplicationUserModelId(process: HANDLE, len: *mut u32, id: *mut u16) -> i32;
}

/// The AppUserModelID of the package a process runs in (`None` for a program that is not packaged, or that cannot be asked).
pub fn process_aumid(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut len = 512u32;
        let mut buf = vec![0u16; len as usize];
        let r = GetApplicationUserModelId(h, &mut len, buf.as_mut_ptr());
        let _ = CloseHandle(h);
        // 0 is success; `len` counts the terminating NUL
        (r == 0 && len > 1).then(|| String::from_utf16_lossy(&buf[..(len - 1) as usize]))
    }
}

/// Gives every task window that sets no AppUserModelID the one its process's package has, asked with `lookup` once per process.
pub fn with_process_ids(tasks: Vec<Task>, mut lookup: impl FnMut(u32) -> Option<String>) -> Vec<Task> {
    let mut seen: HashMap<u32, Option<String>> = HashMap::new();
    tasks
        .into_iter()
        .map(|mut t| {
            if t.app_id.is_none() {
                t.app_id = seen.entry(t.pid).or_insert_with(|| lookup(t.pid)).clone();
            }
            t
        })
        .collect()
}

/// The task windows as the dock groups them: like the taskbar's buttons, by the window's AppUserModelID, else the process's package's,
/// else the program. Blocks (it opens a handle to every program that has a window): a worker thread.
pub fn running_groups() -> Vec<(String, Vec<Task>)> {
    tasklist::grouped(with_process_ids(tasklist::tasks(), process_aumid))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task(hwnd: isize, pid: u32, exe: &str, id: Option<&str>) -> Task {
        Task { hwnd, title: String::new(), pid, exe: Some(PathBuf::from(exe)), app_id: id.map(String::from), minimized: false }
    }

    // The identities below are the ones found on this machine (see the dock_probe example): the windows of these apps set no
    // AppUserModelID, their processes' packages have the ids on the right.
    const TELEGRAM: &str = "TelegramMessengerLLP.TelegramDesktop_t4vj0pshhgkwm!Telegram.TelegramDesktop.Store";
    const WHATSAPP: &str = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App";

    #[test]
    fn a_window_without_an_id_takes_its_process_s_and_one_with_an_id_keeps_it() {
        let tasks = vec![
            task(1, 10, r"C:\Program Files\WindowsApps\telegrammessengerllp.telegramdesktop_7.3.3.0_x64__t4vj0pshhgkwm\telegram.exe", None),
            task(2, 11, r"C:\Program Files\Google\Chrome\Application\chrome.exe", Some("Chrome")),
            task(3, 12, r"C:\Users\x\AppData\Local\Discord\app-1\discord.exe", None),
            task(4, 13, r"C:\Program Files\WindowsApps\5319275a.whatsappdesktop_2.2642.101.0_x64__cv1g1gvanyjgm\whatsapp.root.exe", None),
        ];
        let out = with_process_ids(tasks, |pid| match pid {
            10 => Some(TELEGRAM.to_string()),
            13 => Some(WHATSAPP.to_string()),
            11 => Some("never asked".to_string()),
            _ => None,
        });
        assert_eq!(out[0].app_id.as_deref(), Some(TELEGRAM));
        assert_eq!(out[1].app_id.as_deref(), Some("Chrome"), "the window's own id wins");
        assert_eq!(out[2].app_id, None, "a program that is not packaged stays grouped by its path");
        assert_eq!(out[0].group(), TELEGRAM.to_lowercase());
        assert_eq!(out[3].group(), WHATSAPP.to_lowercase(), "no longer a group named after a path under WindowsApps");
    }

    #[test]
    fn a_process_is_asked_once_however_many_windows_it_has() {
        let tasks = vec![task(1, 10, "a.exe", None), task(2, 10, "a.exe", None), task(3, 10, "a.exe", None), task(4, 20, "b.exe", None)];
        let mut asked = Vec::new();
        let out = with_process_ids(tasks, |pid| {
            asked.push(pid);
            Some(format!("P{pid}_x!A"))
        });
        assert_eq!(asked, vec![10, 20]);
        assert!(out.iter().take(3).all(|t| t.app_id.as_deref() == Some("P10_x!A")));
        // so the three windows form one group
        assert_eq!(tasklist::grouped(out).len(), 2);
    }

    #[test]
    fn the_real_process_of_a_plain_program_has_no_package_id() {
        // this test's own process is not packaged
        assert_eq!(process_aumid(std::process::id()), None);
        assert_eq!(process_aumid(0), None);
    }
}
