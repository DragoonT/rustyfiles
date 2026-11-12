//! What a click on the dock does to other programs: start an app, bring one of its windows forward, minimise it.
//!
//! Everything here can wait on another program (a window that does not answer, a shell extension), so call it from a worker thread,
//! never from the UI thread. COM is initialised on the calling thread by the functions that need it.

use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, SW_MINIMIZE, SW_RESTORE, ShowWindowAsync};

use super::model::{ClickAction, IconSource, Launch};

/// Starts an app (a middle click starts another instance of one that is running: the same call). Packaged apps go through the
/// application activation manager, everything else through the shell's "open", as a double click in Explorer would.
pub fn start(launch: &Launch) -> anyhow::Result<()> {
    match launch {
        Launch::Link(p) => open_with_shell(p.as_os_str(), "", None),
        Launch::Exe { path, args } => open_with_shell(path.as_os_str(), args, path.parent()),
        Launch::Packaged(id) => start_packaged(id),
    }
}

fn open_with_shell(file: &std::ffi::OsStr, args: &str, dir: Option<&std::path::Path>) -> anyhow::Result<()> {
    use windows::Win32::UI::Shell::{SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{PCWSTR, w};
    crate::shell::ensure_com();
    let file = crate::wide::wide(file);
    let args = (!args.is_empty()).then(|| crate::wide::wide(args));
    let dir = dir.filter(|d| !d.as_os_str().is_empty()).map(crate::wide::wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        // NOASYNC: the call returns when the shell is done. It runs on a thread that ends right after, and a verb the shell hands to
        // another thread (DDE, some extensions) would otherwise be lost with it.
        fMask: SEE_MASK_NOASYNC,
        lpVerb: w!("open"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: args.as_ref().map_or(PCWSTR::null(), |a| PCWSTR(a.as_ptr())),
        lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |d| PCWSTR(d.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

fn start_packaged(aumid: &str) -> anyhow::Result<()> {
    use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
    use windows::core::PCWSTR;
    crate::shell::ensure_com();
    let id = crate::wide::wide(aumid);
    let activated = unsafe {
        CoCreateInstance::<_, IApplicationActivationManager>(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)
            .and_then(|m| m.ActivateApplication(PCWSTR(id.as_ptr()), PCWSTR::null(), AO_NONE))
    };
    match activated {
        Ok(_pid) => Ok(()),
        Err(e) => {
            // an id the manager does not know (a classic app that sets an id of its own): the shell can start it by the same name
            log::debug!("dock: activation of {aumid} failed ({e}), trying the shell");
            crate::shellfolder::open(&format!("shell:AppsFolder\\{aumid}"))
        }
    }
}

/// Brings a task window to the front and gives it the keyboard; a minimised one is restored first. The restore is asked
/// without waiting (a window that hangs must not hang the dock).
pub fn activate(hwnd: isize) {
    let h = HWND(hwnd as *mut _);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        if IsIconic(h).as_bool() {
            let _ = ShowWindowAsync(h, SW_RESTORE);
        }
    }
    crate::window::bring_to_front(hwnd);
}

/// Minimises a task window, without waiting for it.
pub fn minimize(hwnd: isize) {
    unsafe {
        let _ = ShowWindowAsync(HWND(hwnd as *mut _), SW_MINIMIZE);
    }
}

/// What a click on an app does, once decided by [`super::model::click_action`]. `launch` is the app's way to start (needed for
/// [`ClickAction::Start`]; an app that cannot be started again just does nothing).
pub fn perform(action: ClickAction, launch: Option<&Launch>) -> anyhow::Result<()> {
    match action {
        ClickAction::Start => match launch {
            Some(l) => start(l),
            None => Ok(()),
        },
        ClickAction::Raise(h) => {
            activate(h);
            Ok(())
        }
        ClickAction::Minimize(h) => {
            minimize(h);
            Ok(())
        }
    }
}

/// What the app is called for a tooltip: the packaged app's display name, a program's file description (what Explorer's taskbar
/// shows), else the file's name. `None` when nothing can be read.
pub fn friendly_name(icon: &IconSource) -> Option<String> {
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName};
    match icon {
        IconSource::Packaged(id) => super::pins::packaged_name(id),
        IconSource::File(p) | IconSource::Resource { file: p, .. } => {
            use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
            crate::shell::ensure_com();
            let w = crate::wide::wide(p);
            unsafe {
                let described: Option<String> = SHGetPropertyStoreFromParsingName::<_, _, IPropertyStore>(windows::core::PCWSTR(w.as_ptr()), None, GPS_DEFAULT)
                    .ok()
                    .and_then(|store| {
                        // System.FileDescription
                        let key = windows::Win32::Foundation::PROPERTYKEY { fmtid: windows::core::GUID::from_u128(0x0cef7d53_fa64_11d1_a203_0000f81fedee), pid: 3 };
                        let v = store.GetValue(&key).ok()?;
                        let s = PropVariantToStringAlloc(&v).ok()?;
                        let out = s.to_string().ok();
                        windows::Win32::System::Com::CoTaskMemFree(Some(s.0 as *const _));
                        out
                    })
                    .filter(|s| !s.trim().is_empty());
                described.or_else(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
            }
        }
        IconSource::Shell(_) | IconSource::Window(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn nothing_to_start_is_not_an_error() {
        assert!(perform(ClickAction::Start, None).is_ok());
    }

    #[test]
    fn a_window_that_is_gone_is_left_alone() {
        // 0 and a stale number are not windows: neither call may touch anything or panic
        activate(0);
        minimize(0);
        assert!(perform(ClickAction::Raise(0), None).is_ok());
        assert!(perform(ClickAction::Minimize(0), None).is_ok());
    }

    /// Read-only: names come from the file's version information and the shell.
    #[test]
    fn names_of_programs_and_apps() {
        let n = friendly_name(&IconSource::File(PathBuf::from(r"C:\Windows\explorer.exe"))).unwrap();
        assert!(!n.is_empty());
        eprintln!("explorer.exe is called {n:?}");
        assert_eq!(friendly_name(&IconSource::Window(5)), None);
        assert_eq!(friendly_name(&IconSource::Shell("shell:RecycleBinFolder".into())), None);
    }
}
