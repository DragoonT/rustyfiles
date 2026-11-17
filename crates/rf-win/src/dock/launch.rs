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

/// Opens `files` with an app, as dropping them on its dock icon does (issue 41: "drag-and-drop support for files onto running
/// applications in the Dock"). A program gets them as arguments after its own; a shortcut's program the same way, the shortcut's
/// arguments first; a packaged app, and a shortcut that leads to one, through the activation manager's "open" for files. A
/// shortcut whose program cannot be read (an installer's advertised shortcut) is opened by the shell with the files as its
/// parameters.
pub fn open_files(launch: &Launch, files: &[std::path::PathBuf]) -> anyhow::Result<()> {
    anyhow::ensure!(!files.is_empty(), "no files");
    let quoted = quote_all(files);
    match launch {
        Launch::Exe { path, args } => open_with_shell(path.as_os_str(), &after(args, &quoted), path.parent()),
        Launch::Packaged(id) => open_packaged(id, files),
        Launch::Link(lnk) => {
            crate::shell::ensure_com();
            match super::pins::read_link(lnk) {
                Some(f) if f.target.is_file() => open_with_shell(f.target.as_os_str(), &after(&f.args, &quoted), f.target.parent()),
                Some(f) if f.aumid.as_deref().is_some_and(|a| !a.is_empty()) => open_packaged(f.aumid.as_deref().unwrap_or_default(), files),
                _ => open_with_shell(lnk.as_os_str(), &quoted, None),
            }
        }
    }
}

/// The program behind a running window (for an app the dock has no way to start: it is given the files the same way).
pub fn program_of_window(hwnd: isize) -> Option<std::path::PathBuf> {
    let mut pid = 0u32;
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    (pid != 0).then(|| crate::switcher::source::exe_of(pid)).flatten()
}

/// `first`, then `rest`, with one space between when both are there.
fn after(first: &str, rest: &str) -> String {
    match (first.trim(), rest) {
        ("", r) => r.to_string(),
        (f, "") => f.to_string(),
        (f, r) => format!("{f} {r}"),
    }
}

/// Paths as arguments: each in quotes. A path that ends in a backslash (a drive's root, `C:\`) gets a second one, or the closing
/// quote would be read as a quote character (`"C:\"` is `C:"` to the program).
fn quote_all(files: &[std::path::PathBuf]) -> String {
    files
        .iter()
        .map(|f| {
            let s = f.to_string_lossy();
            if s.ends_with('\\') { format!("\"{s}\\\"") } else { format!("\"{s}\"") }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A packaged app's "open" for files. On a thread of its own and not waited for: the activation manager can start the app and not
/// return (see `crate::activate`); what it answers is logged.
fn open_packaged(aumid: &str, files: &[std::path::PathBuf]) -> anyhow::Result<()> {
    let (aumid, files) = (aumid.to_string(), files.to_vec());
    std::thread::Builder::new().name("dock-open-files".into()).spawn(move || {
        if let Err(e) = activate_for_files(&aumid, &files) {
            log::warn!("dock: {aumid} did not take the dropped files: {e:#}");
        }
    })?;
    Ok(())
}

fn activate_for_files(aumid: &str, files: &[std::path::PathBuf]) -> anyhow::Result<()> {
    use windows::Win32::System::Com::{CLSCTX_LOCAL_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{
        ApplicationActivationManager, IApplicationActivationManager, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists,
    };
    use windows::core::{PCWSTR, w};
    crate::shell::ensure_com();
    let pidls: Vec<_> = files
        .iter()
        .map(|f| {
            let w = crate::wide::wide(f.as_os_str());
            unsafe { ILCreateFromPathW(PCWSTR(w.as_ptr())) }
        })
        .filter(|p| !p.is_null())
        .collect();
    let list: Vec<_> = pidls.iter().map(|&p| p as *const _).collect();
    let array = unsafe { SHCreateShellItemArrayFromIDLists(&list) };
    for p in pidls {
        unsafe { ILFree(Some(p)) };
    }
    let array = array?;
    let manager: IApplicationActivationManager = unsafe { CoCreateInstance(&ApplicationActivationManager, None, CLSCTX_LOCAL_SERVER)? };
    let id = crate::wide::wide(aumid);
    unsafe { manager.ActivateForFile(PCWSTR(id.as_ptr()), &array, w!("open"))? };
    Ok(())
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
    start_packaged_with(aumid, "")
}

/// [`start`] of a packaged app with an argument string (a jump-list task such as "New window" carries one).
///
/// The activation goes through the one shared function (`crate::activate::packaged`: a thread of its own and a short wait, because
/// with no Explorer the activation manager starts the app and never returns). An error from it sends the start to the shell, as before;
/// when that fails too the error says both, so that "class not registered" from a shell that is not there is not taken for "the app
/// was moved". An activation that fails only AFTER its wait (it was reported as started) runs the same shell step from its own
/// thread, so the second step is not lost to a slow activation.
pub fn start_packaged_with(aumid: &str, args: &str) -> anyhow::Result<()> {
    let first = match crate::activate::packaged_with(aumid, args, crate::activate::Late::Shell) {
        Ok(()) => return Ok(()),
        Err(e) => e,
    };
    // an id the manager does not know (a classic app that sets an id of its own): the shell can start it by the same name
    log::debug!("dock: activation of {aumid} failed ({first}), trying the shell");
    crate::shellfolder::open(&format!("shell:AppsFolder\\{aumid}")).map_err(|second| anyhow::anyhow!("{first}; the shell: {second}"))
}

/// How a task window is brought forward, once its restore (if it was minimised) has been asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Way {
    /// The usual way: the foreground calls.
    Front,
    /// The way Alt+Tab itself switches (`SwitchToThisWindow`): for a minimised window whose restore Windows refused.
    AltTab,
}

/// The decision of [`activate`]. A restore is refused for a window of a program that runs with more rights than ours (as
/// administrator: `ShowWindowAsync` answers false, error 5), and a window that stays minimised cannot be brought forward by any
/// foreground call. Measured on the user's PC, 2026-11-14 ("docktaki bazı uygulamalara tıklayınca öne gelmiyorlar"; the app was the
/// one running as administrator): the restore was refused and the window stayed minimised, `SwitchToThisWindow` restored it and put
/// it in front.
pub fn way(minimized: bool, restore_taken: bool) -> Way {
    if minimized && !restore_taken { Way::AltTab } else { Way::Front }
}

/// Brings a task window to the front and gives it the keyboard; a minimised one is restored first. The restore is asked
/// without waiting (a window that hangs must not hang the dock).
pub fn activate(hwnd: isize) {
    let h = HWND(hwnd as *mut _);
    let (minimized, taken) = unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return;
        }
        let minimized = IsIconic(h).as_bool();
        (minimized, minimized && ShowWindowAsync(h, SW_RESTORE).as_bool())
    };
    match way(minimized, taken) {
        Way::AltTab => unsafe { windows::Win32::UI::WindowsAndMessaging::SwitchToThisWindow(h, true) },
        Way::Front => crate::window::bring_to_front(hwnd),
    }
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
    match icon {
        IconSource::Packaged(id) => super::pins::packaged_name(id),
        IconSource::File(p) | IconSource::Resource { file: p, .. } => file_description(p).or_else(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())),
        IconSource::Shell(_) | IconSource::Window(_) => None,
    }
}

/// `System.FileDescription` of a program (what Explorer's taskbar calls a classic program), `None` when it has none. Shell thread (see
/// [`crate::sta::call`]): [`known_names`] is the way to ask from anywhere else.
fn file_description(p: &std::path::Path) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::{GPS_DEFAULT, IPropertyStore, SHGetPropertyStoreFromParsingName};
    crate::shell::ensure_com();
    let w = crate::wide::wide(p);
    unsafe {
        SHGetPropertyStoreFromParsingName::<_, _, IPropertyStore>(windows::core::PCWSTR(w.as_ptr()), None, GPS_DEFAULT)
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
            .filter(|s| !s.trim().is_empty())
    }
}

/// What the shell can say about an app's name, the two sources the app menu prefers over the bare file name (see
/// [`super::appmenu::menu_name`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KnownNames {
    /// The name "All apps" (and so the Start panel) shows for a packaged app.
    pub display: Option<String>,
    /// The program file's description.
    pub description: Option<String>,
}

/// Asks the shell thread for the names of an app: the display name of a packaged app (`packaged`, its AppUserModelID) and the
/// description of a classic program (`exe`). Read-only; blocks up to a few seconds: a worker thread only.
pub fn known_names(packaged: Option<&str>, exe: Option<&std::path::Path>) -> KnownNames {
    let (packaged, exe) = (packaged.map(str::to_string), exe.map(std::path::Path::to_path_buf));
    crate::sta::call(std::time::Duration::from_secs(5), move || KnownNames {
        display: packaged.as_deref().and_then(super::pins::packaged_name),
        description: exe.as_deref().and_then(file_description),
    })
    .unwrap_or_default()
}

/// Of a jump-list identity's AppUserModelID (`model::jump_identity`), the part that is asked for a display name: only a packaged app's
/// id is (a classic program's explicit id is no package family). The `packaged` argument of [`known_names`].
pub fn packaged_id(aumid: Option<&str>) -> Option<&str> {
    aumid.filter(|a| super::model::looks_packaged(a))
}

#[cfg(test)]
mod name_tests {
    use super::packaged_id;

    #[test]
    fn only_a_packaged_apps_id_is_asked_for_a_display_name() {
        assert_eq!(packaged_id(Some("Microsoft.Paint_8wekyb3d8bbwe!App")), Some("Microsoft.Paint_8wekyb3d8bbwe!App"));
        assert_eq!(packaged_id(Some("Microsoft.VisualStudioCode")), None);
        assert_eq!(packaged_id(None), None);
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
    fn a_minimised_window_whose_restore_was_refused_is_switched_to_the_alt_tab_way() {
        assert_eq!(way(true, false), Way::AltTab, "a program that runs as administrator refuses our restore");
        assert_eq!(way(true, true), Way::Front, "the restore was taken: the usual calls follow");
        assert_eq!(way(false, false), Way::Front, "not minimised: nothing was asked, nothing was refused");
        assert_eq!(way(false, true), Way::Front);
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

#[cfg(test)]
mod open_files_tests {
    use super::{after, quote_all};
    use std::path::PathBuf;

    #[test]
    fn dropped_files_follow_the_programs_own_arguments_each_in_quotes() {
        let files = [PathBuf::from(r"C:\My Files\a.txt"), PathBuf::from(r"D:\")];
        assert_eq!(quote_all(&files), r#""C:\My Files\a.txt" "D:\\""#, "a root's backslash is doubled before the quote");
        assert_eq!(after("--profile work", &quote_all(&files[..1])), r#"--profile work "C:\My Files\a.txt""#);
        assert_eq!(after("  ", r#""x""#), r#""x""#);
    }
}
