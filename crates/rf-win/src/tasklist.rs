//! The windows a taskbar shows, found with the same rules as Explorer's taskbar, grouped by
//! application (first step of our own taskbar, see notes/design: görev çubuğu). Read-only:
//! nothing here changes any window.
//!
//! A window is a task when it is visible, not cloaked by DWM (other virtual desktops, suspended
//! UWP frames), and either has no owner and isn't a tool window, or asks for a taskbar button
//! with `WS_EX_APPWINDOW`. Windows are grouped by their AppUserModelID, or by their program
//! when they have none — as Explorer groups them.

use std::path::PathBuf;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GW_OWNER, GWL_EXSTYLE, GetWindow, GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic,
    IsWindowVisible, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, PWSTR};

/// One taskbar button's window.
#[derive(Clone, Debug)]
pub struct Task {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
    /// The program, when it can be read (not for elevated processes of other users).
    pub exe: Option<PathBuf>,
    /// The window's AppUserModelID, when it sets one.
    pub app_id: Option<String>,
    pub minimized: bool,
}

impl Task {
    /// What Explorer groups by: the app id, else the program path (case-insensitive).
    pub fn group(&self) -> String {
        match (&self.app_id, &self.exe) {
            (Some(id), _) => id.to_lowercase(),
            (None, Some(exe)) => exe.to_string_lossy().to_lowercase(),
            (None, None) => format!("pid:{}", self.pid),
        }
    }
}

/// The style rules alone (testable without windows): owner, extended style.
pub fn style_is_task(has_owner: bool, ex_style: u32) -> bool {
    let app = ex_style & WS_EX_APPWINDOW.0 != 0;
    if app {
        return true;
    }
    !has_owner && ex_style & WS_EX_TOOLWINDOW.0 == 0 && ex_style & WS_EX_NOACTIVATE.0 == 0
}

fn cloaked(h: HWND) -> bool {
    let mut v: u32 = 0;
    unsafe { DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut v as *mut u32 as *mut _, 4) }.is_ok() && v != 0
}

fn title(h: HWND) -> String {
    let n = unsafe { GetWindowTextLengthW(h) };
    if n <= 0 {
        return String::new();
    }
    let mut buf = vec![0u16; n as usize + 1];
    let got = unsafe { GetWindowTextW(h, &mut buf) };
    String::from_utf16_lossy(&buf[..got.max(0) as usize])
}

fn exe_of(pid: u32) -> Option<PathBuf> {
    unsafe {
        let p = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(p, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(p);
        r.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// The window's AppUserModelID (`System.AppUserModel.ID` in its property store).
fn app_id_of(h: HWND) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{IPropertyStore, SHGetPropertyStoreForWindow};
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    unsafe {
        let store: IPropertyStore = SHGetPropertyStoreForWindow(h).ok()?;
        let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

fn task_of(h: HWND) -> Option<Task> {
    unsafe {
        if !IsWindowVisible(h).as_bool() {
            return None;
        }
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE) as u32;
        let has_owner = GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid());
        if !style_is_task(has_owner, ex) || cloaked(h) {
            return None;
        }
        let mut r = RECT::default();
        let _ = windows::Win32::UI::WindowsAndMessaging::GetWindowRect(h, &mut r);
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let title = title(h);
        // untitled, zero-size helpers are not buttons (Explorer skips them too)
        if title.is_empty() && (r.right - r.left <= 0 || r.bottom - r.top <= 0) {
            return None;
        }
        Some(Task { hwnd: h.0 as isize, title, pid, exe: exe_of(pid), app_id: app_id_of(h), minimized: IsIconic(h).as_bool() })
    }
}

/// The current task windows, in Z order (the active one first).
pub fn tasks() -> Vec<Task> {
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<Task>) };
        if let Some(t) = task_of(h) {
            out.push(t);
        }
        BOOL(1)
    }
    let mut out: Vec<Task> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut Vec<Task> as isize));
    }
    out
}

/// Tasks grouped like taskbar buttons: (group key, its windows), in order of first appearance.
pub fn grouped(tasks: Vec<Task>) -> Vec<(String, Vec<Task>)> {
    let mut groups: Vec<(String, Vec<Task>)> = Vec::new();
    for t in tasks {
        let k = t.group();
        match groups.iter_mut().find(|g| g.0 == k) {
            Some(g) => g.1.push(t),
            None => groups.push((k, vec![t])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn style_rules_match_explorer() {
        let tool = WS_EX_TOOLWINDOW.0;
        let app = WS_EX_APPWINDOW.0;
        assert!(style_is_task(false, 0), "a plain top-level window");
        assert!(!style_is_task(true, 0), "an owned dialog has no button");
        assert!(!style_is_task(false, tool), "tool windows have none");
        assert!(style_is_task(true, app), "APPWINDOW forces a button, even owned");
        assert!(style_is_task(false, tool | app), "APPWINDOW wins over TOOLWINDOW");
        assert!(!style_is_task(false, WS_EX_NOACTIVATE.0));
    }

    #[test]
    fn groups_by_app_id_then_program() {
        let t = |pid, exe: &str, id: Option<&str>| Task { hwnd: pid as isize, title: String::new(), pid, exe: Some(PathBuf::from(exe)), app_id: id.map(String::from), minimized: false };
        let g = grouped(vec![
            t(1, r"C:\a\Chrome.exe", None),
            t(2, r"C:\b\code.exe", Some("Microsoft.VisualStudioCode")),
            t(3, r"C:\A\chrome.exe", None),
            t(4, r"C:\c\other.exe", Some("microsoft.visualstudiocode")),
        ]);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].1.iter().map(|t| t.pid).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(g[1].1.iter().map(|t| t.pid).collect::<Vec<_>>(), vec![2, 4]);
    }

    #[test]
    fn lists_without_failing() {
        // whatever is open on the test machine: each listed task has a pid and is visible
        for t in tasks() {
            assert!(t.pid != 0);
        }
    }
}
