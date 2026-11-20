//! "Open the folder and select this file" from other apps while rustyfiles is the default file
//! manager: `SHOpenFolderAndSelectItems`, what Telegram's and the browsers' "Show in folder" call.
//!
//! Windows opens the folder through the folder verb we took over, so the file to select never
//! reaches our command line. It then looks for an Explorer window showing that folder (the
//! shell's window list, `IShellWindows`) and asks that window to select the item. We answer as
//! one: an invisible window registered with the shell for the folder, whose
//! `IShellView::SelectItem` hands us the item.
//!
//! The shell's side, as Explorer++ and Files found it (neither documented by Microsoft): it
//! finds the window with `FindWindowSW` (type `SWC_BROWSER` only), asks the registered object
//! for `IWebBrowserApp`, takes `get_Document`, asks that for `IServiceProvider`, queries the
//! view (`IShellView`) and calls `SelectItem` with the item's relative ID list. A pending
//! registration on the same thread must come first, the third `RegisterPending` argument is an
//! empty VARIANT, and the location is the folder's ID list as a byte-array VARIANT.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::Win32::Foundation::{E_NOINTERFACE, E_NOTIMPL, HWND, LPARAM, RECT, S_FALSE, SHANDLE_PTR, VARIANT_BOOL, VARIANT_FALSE, VARIANT_TRUE, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, DISPATCH_FLAGS, DISPPARAMS, EXCEPINFO, IDispatch, IDispatch_Impl, IServiceProvider,
    IServiceProvider_Impl, ITypeInfo,
};
use windows::Win32::System::Ole::IOleWindow_Impl;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{InitVariantFromBuffer, VARIANT};
use windows::Win32::UI::Controls::LPFNSVADDPROPSHEETPAGE;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERSETTINGS, ILCombine, ILFree, ILGetSize, IShellBrowser, IShellView, IShellView_Impl, IShellWindows, IWebBrowser_Impl, IWebBrowserApp, IWebBrowserApp_Impl, SHGetNameFromIDList,
    SHParseDisplayName, SIGDN_FILESYSPATH, SVSI_EDIT, SVSI_ENSUREVISIBLE, SVSI_FOCUSED, SVSI_SELECT, SWC_BROWSER, ShellWindows, _SVGIO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, KillTimer, MSG, PostMessageW, SetTimer, TranslateMessage, WINDOW_STYLE, WM_APP, WM_TIMER, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, BSTR, GUID, HRESULT, HSTRING, IUnknown, IUnknownImpl, Interface, PCWSTR, Ref, Result, implement, w};

#[implement(IWebBrowserApp, IServiceProvider, IShellView)]
struct ShellWindow {
    hwnd: HWND,
    /// The folder's absolute ID list (its bytes: the object may outlive the caller's copy).
    folder: Vec<u8>,
    name: String,
    /// Set by SelectItem, which COM may call on another thread than the waiting one.
    picked: Arc<Mutex<Option<PathBuf>>>,
}

fn not_impl<T>() -> Result<T> {
    Err(E_NOTIMPL.into())
}

impl IDispatch_Impl for ShellWindow_Impl {
    // the shell asks for IDispatch but never calls it
    fn GetTypeInfoCount(&self) -> Result<u32> {
        Ok(0)
    }
    fn GetTypeInfo(&self, _: u32, _: u32) -> Result<ITypeInfo> {
        not_impl()
    }
    fn GetIDsOfNames(&self, _: *const GUID, _: *const PCWSTR, _: u32, _: u32, _: *mut i32) -> Result<()> {
        not_impl()
    }
    fn Invoke(&self, _: i32, _: *const GUID, _: u32, _: DISPATCH_FLAGS, _: *const DISPPARAMS, _: *mut VARIANT, _: *mut EXCEPINFO, _: *mut u32) -> Result<()> {
        not_impl()
    }
}

impl IWebBrowser_Impl for ShellWindow_Impl {
    fn GoBack(&self) -> Result<()> {
        not_impl()
    }
    fn GoForward(&self) -> Result<()> {
        not_impl()
    }
    fn GoHome(&self) -> Result<()> {
        not_impl()
    }
    fn GoSearch(&self) -> Result<()> {
        not_impl()
    }
    fn Navigate(&self, _: &BSTR, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT, _: *const VARIANT) -> Result<()> {
        not_impl()
    }
    fn Refresh(&self) -> Result<()> {
        Ok(())
    }
    fn Refresh2(&self, _: *const VARIANT) -> Result<()> {
        Ok(())
    }
    fn Stop(&self) -> Result<()> {
        Ok(())
    }
    fn Application(&self) -> Result<IDispatch> {
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn Parent(&self) -> Result<IDispatch> {
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn Container(&self) -> Result<IDispatch> {
        not_impl()
    }
    /// The "document": this same object, which answers IServiceProvider.
    fn Document(&self) -> Result<IDispatch> {
        crate::handover::trace("shell window: the shell asked for its document");
        self.to_interface::<IWebBrowserApp>().cast()
    }
    fn TopLevelContainer(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn Type(&self) -> Result<BSTR> {
        Ok(BSTR::from("File Folder"))
    }
    fn Left(&self) -> Result<i32> {
        Ok(0)
    }
    fn SetLeft(&self, _: i32) -> Result<()> {
        Ok(())
    }
    fn Top(&self) -> Result<i32> {
        Ok(0)
    }
    fn SetTop(&self, _: i32) -> Result<()> {
        Ok(())
    }
    fn Width(&self) -> Result<i32> {
        Ok(0)
    }
    fn SetWidth(&self, _: i32) -> Result<()> {
        Ok(())
    }
    fn Height(&self) -> Result<i32> {
        Ok(0)
    }
    fn SetHeight(&self, _: i32) -> Result<()> {
        Ok(())
    }
    fn LocationName(&self) -> Result<BSTR> {
        Ok(BSTR::from(self.name.as_str()))
    }
    fn LocationURL(&self) -> Result<BSTR> {
        not_impl()
    }
    fn Busy(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
}

impl IWebBrowserApp_Impl for ShellWindow_Impl {
    fn Quit(&self) -> Result<()> {
        Ok(())
    }
    fn ClientToWindow(&self, _: *mut i32, _: *mut i32) -> Result<()> {
        not_impl()
    }
    fn PutProperty(&self, _: &BSTR, _: &VARIANT) -> Result<()> {
        not_impl()
    }
    fn GetProperty(&self, _: &BSTR) -> Result<VARIANT> {
        not_impl()
    }
    fn Name(&self) -> Result<BSTR> {
        Ok(BSTR::from("rustyfiles"))
    }
    fn HWND(&self) -> Result<SHANDLE_PTR> {
        Ok(SHANDLE_PTR(self.hwnd.0 as isize))
    }
    fn FullName(&self) -> Result<BSTR> {
        not_impl()
    }
    fn Path(&self) -> Result<BSTR> {
        not_impl()
    }
    fn Visible(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_TRUE)
    }
    fn SetVisible(&self, _: VARIANT_BOOL) -> Result<()> {
        Ok(())
    }
    fn StatusBar(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetStatusBar(&self, _: VARIANT_BOOL) -> Result<()> {
        Ok(())
    }
    fn StatusText(&self) -> Result<BSTR> {
        Ok(BSTR::new())
    }
    fn SetStatusText(&self, _: &BSTR) -> Result<()> {
        Ok(())
    }
    fn ToolBar(&self) -> Result<i32> {
        Ok(0)
    }
    fn SetToolBar(&self, _: i32) -> Result<()> {
        Ok(())
    }
    fn MenuBar(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetMenuBar(&self, _: VARIANT_BOOL) -> Result<()> {
        Ok(())
    }
    fn FullScreen(&self) -> Result<VARIANT_BOOL> {
        Ok(VARIANT_FALSE)
    }
    fn SetFullScreen(&self, _: VARIANT_BOOL) -> Result<()> {
        Ok(())
    }
}

impl IServiceProvider_Impl for ShellWindow_Impl {
    /// Whatever service is named, the view (IShellView / IFolderView are asked for).
    fn QueryService(&self, service: *const GUID, riid: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        let unk: IUnknown = self.to_interface::<IShellView>().cast()?;
        let r = unsafe { unk.query(riid, ppv).ok() };
        if crate::handover::tracing() {
            let g = |p: *const GUID| if p.is_null() { "-".to_string() } else { format!("{:?}", unsafe { *p }) };
            crate::handover::trace(&format!("shell window: the shell asked for service {} as {}: {}", g(service), g(riid), if r.is_ok() { "given" } else { "not given" }));
        }
        r
    }
}

impl IOleWindow_Impl for ShellWindow_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        Ok(self.hwnd)
    }
    fn ContextSensitiveHelp(&self, _: BOOL) -> Result<()> {
        not_impl()
    }
}

impl IShellView_Impl for ShellWindow_Impl {
    fn TranslateAccelerator(&self, _: *const MSG) -> HRESULT {
        S_FALSE
    }
    fn EnableModeless(&self, _: BOOL) -> Result<()> {
        Ok(())
    }
    fn UIActivate(&self, _: u32) -> Result<()> {
        Ok(())
    }
    fn Refresh(&self) -> Result<()> {
        Ok(())
    }
    fn CreateViewWindow(&self, _: Ref<IShellView>, _: *const FOLDERSETTINGS, _: Ref<IShellBrowser>, _: *const RECT) -> Result<HWND> {
        not_impl()
    }
    fn DestroyViewWindow(&self) -> Result<()> {
        Ok(())
    }
    fn GetCurrentInfo(&self) -> Result<FOLDERSETTINGS> {
        Ok(FOLDERSETTINGS::default())
    }
    fn AddPropertySheetPages(&self, _: u32, _: LPFNSVADDPROPSHEETPAGE, _: LPARAM) -> Result<()> {
        not_impl()
    }
    fn SaveViewState(&self) -> Result<()> {
        Ok(())
    }
    /// The item to show selected: its full path goes to the waiting caller.
    fn SelectItem(&self, item: *const ITEMIDLIST, flags: u32) -> Result<()> {
        let wanted = (SVSI_SELECT.0 | SVSI_FOCUSED.0 | SVSI_ENSUREVISIBLE.0 | SVSI_EDIT.0) as u32;
        crate::handover::trace(&format!("shell window: SelectItem, flags {flags:#x}, item {}", if item.is_null() { "none" } else { "given" }));
        if item.is_null() || flags & wanted == 0 {
            return Ok(());
        }
        unsafe {
            let full = ILCombine(Some(self.folder.as_ptr().cast()), Some(item));
            if full.is_null() {
                return Ok(());
            }
            let name = SHGetNameFromIDList(full, SIGDN_FILESYSPATH);
            ILFree(Some(full));
            if let Ok(name) = name {
                let path = name.to_string().ok().map(PathBuf::from);
                CoTaskMemFree(Some(name.0 as *const _));
                if let Some(path) = path {
                    *self.picked.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
                    // ends the wait below, from whichever thread COM called us on
                    let _ = PostMessageW(Some(self.hwnd), WM_PICKED, WPARAM(0), LPARAM(0));
                }
            }
        }
        Ok(())
    }
    fn GetItemObject(&self, _: &_SVGIO, _: *const GUID, ppv: *mut *mut core::ffi::c_void) -> Result<()> {
        if !ppv.is_null() {
            unsafe { *ppv = std::ptr::null_mut() };
        }
        Err(E_NOINTERFACE.into())
    }
}

/// Registers a shell window for `folder` on this thread and waits up to `wait` for the shell to
/// ask for an item to be selected there: that item's path. Blocking (a message loop); call it
/// on a thread of its own, right when rustyfiles is started on a folder.
pub fn wait_for_select(folder: &Path, wait: Duration) -> Option<PathBuf> {
    unsafe {
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let r = register_and_wait(folder, wait);
        if init {
            CoUninitialize();
        }
        r
    }
}

/// The ID lists a folder is known by. Issue 97: the shell finds a window only by the very ID list it was given, and the two ways
/// of naming a folder give two different lists (`SHParseDisplayName`: 60 bytes for Downloads here; `ILCreateFromPathW`, which
/// Chromium's "Show in folder" uses: 379 bytes, a plain folder 468 against 611). A window registered under one was not found by the
/// other (`probe_lookup_by_ilcreatefrompath`), and on the reporter's Windows the shell then waited 20 s and gave up. So the window
/// is registered under each of them.
unsafe fn id_lists(folder: &Path) -> Vec<Vec<u8>> {
    unsafe {
        let name = HSTRING::from(folder.as_os_str());
        let mut out: Vec<Vec<u8>> = Vec::new();
        let mut take = |pidl: *mut ITEMIDLIST| {
            if !pidl.is_null() {
                let bytes = std::slice::from_raw_parts(pidl as *const u8, ILGetSize(Some(pidl)) as usize).to_vec();
                CoTaskMemFree(Some(pidl as *const _));
                if !out.contains(&bytes) {
                    out.push(bytes);
                }
            }
        };
        let mut parsed: *mut ITEMIDLIST = std::ptr::null_mut();
        if SHParseDisplayName(&name, None, &mut parsed, 0, None).is_ok() {
            take(parsed);
        }
        take(windows::Win32::UI::Shell::ILCreateFromPathW(&name));
        out
    }
}

unsafe fn register_and_wait(folder: &Path, wait: Duration) -> Option<PathBuf> {
    unsafe {
        let lists = id_lists(folder);
        if lists.is_empty() {
            return None;
        }
        let picked = Arc::new(Mutex::new(None));
        let name = folder.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| folder.to_string_lossy().into_owned());
        let windows: IShellWindows = match CoCreateInstance(&ShellWindows, None, CLSCTX_ALL) {
            Ok(w) => w,
            Err(e) => {
                crate::handover::trace(&format!("shell window: no window list: {e}"));
                return None;
            }
        };
        // one invisible window (never shown, never activated, as Files' launcher) and one registration for each ID list
        let mut held: Vec<(HWND, IWebBrowserApp, Option<i32>)> = Vec::new();
        for bytes in &lists {
            let Ok(location) = InitVariantFromBuffer(bytes.as_ptr().cast(), bytes.len() as u32) else { continue };
            let Ok(hwnd) = CreateWindowExW(WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE, w!("STATIC"), w!("rustyfiles shell window"), WINDOW_STYLE(0), 0, 0, 0, 0, None, None, None, None) else { continue };
            let app: IWebBrowserApp = ShellWindow { hwnd, folder: bytes.clone(), name: name.clone(), picked: picked.clone() }.into();
            let registered = (|| -> Result<i32> {
                let empty = VARIANT::default();
                windows.RegisterPending(GetCurrentThreadId() as i32, &location, &empty, SWC_BROWSER)?;
                let dispatch: IDispatch = app.cast()?;
                let cookie = windows.Register(&dispatch, hwnd.0 as isize as i32, SWC_BROWSER)?;
                windows.OnNavigate(cookie, &location)?;
                Ok(cookie)
            })();
            match &registered {
                Ok(_) => {
                    crate::handover::trace(&format!("shell window: registered for {} (ID list of {} bytes)", folder.display(), bytes.len()));
                    if crate::handover::tracing() {
                        let mut found = 0i32;
                        let empty = VARIANT::default();
                        let seen = windows.FindWindowSW(&location, &empty, SWC_BROWSER, &mut found, windows::Win32::UI::Shell::SWFO_NEEDDISPATCH);
                        crate::handover::trace(&format!("shell window: found again by that ID list: {}", if seen.is_ok() { "yes" } else { "no" }));
                    }
                }
                Err(e) => {
                    log::warn!("shell window for {}: {e}", folder.display());
                    crate::handover::trace(&format!("shell window: registration for {} ({} bytes) failed: {e}", folder.display(), bytes.len()));
                }
            }
            held.push((hwnd, app, registered.ok()));
        }
        if let Some(&(main, _, _)) = held.iter().find(|h| h.2.is_some()) {
            SetTimer(Some(main), 1, wait.as_millis().min(u32::MAX as u128) as u32, None);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                let ours = held.iter().any(|h| h.0 == msg.hwnd);
                if ours && (msg.message == WM_PICKED || (msg.hwnd == main && msg.message == WM_TIMER)) {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
            let _ = KillTimer(Some(main), 1);
        }
        for (hwnd, app, cookie) in held {
            if let Some(c) = cookie {
                let _ = windows.Revoke(c);
            }
            let _ = DestroyWindow(hwnd);
            drop(app);
        }
        let got = picked.lock().unwrap_or_else(|e| e.into_inner()).take();
        crate::handover::trace(if got.is_some() { "shell window: an item to select came" } else { "shell window: no item came before the wait ended" });
        got
    }
}

/// Posted to the window when an item was picked.
const WM_PICKED: u32 = WM_APP + 1;

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::IServiceProvider;
    use windows::Win32::UI::Shell::{IFolderView, SWFO_NEEDDISPATCH};

    /// Manual: how the shell names the Desktop folder and the parent of a file on it (issue 97).
    /// `RF_DESKTOP_FILE=<a file on the desktop> cargo test -p rf-win --lib shell_window::tests::desktop_pidls -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn desktop_pidls() {
        use windows::Win32::UI::Shell::{ILClone, ILIsEqual, ILRemoveLastID};
        let file = PathBuf::from(std::env::var("RF_DESKTOP_FILE").unwrap());
        let dir = file.parent().unwrap().to_path_buf();
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let parse = |p: &Path| {
                let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                SHParseDisplayName(&HSTRING::from(p.as_os_str()), None, &mut pidl, 0, None).unwrap();
                pidl
            };
            let (f, d) = (parse(&file), parse(&dir));
            let parent = ILClone(f);
            let _ = ILRemoveLastID(Some(parent));
            println!("file pidl {} bytes, its parent {} bytes, the folder {} bytes, parent == folder: {}", ILGetSize(Some(f)), ILGetSize(Some(parent)), ILGetSize(Some(d)), ILIsEqual(parent, d).as_bool());
        }
    }

    /// Plays the shell's part of "open and select": finds our registered window for a folder the
    /// way SHOpenFolderAndSelectItems does and asks it to select a file; the waiting side gets
    /// the file's path.
    #[test]
    fn the_shell_finds_the_window_and_selection_arrives() {
        shell_select_round(false);
    }

    /// The same, the folder named as Chromium's "Show in folder" names it (issue 97).
    #[test]
    fn the_shell_finds_the_window_by_chromiums_id_list_too() {
        shell_select_round(true);
    }

    fn shell_select_round(chromium: bool) {
        // IShellWindows is served by the shell: without a shell window the registration waits for ever (seen without Explorer)
        if crate::engine_guard::not_run_without_shell("shell_window::tests::shell_select_round") {
            return;
        }
        let dir = std::env::temp_dir().join(format!("rf-shellwin-{}-{chromium}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // the shell answers with long names: where TEMP is given in its short form
        // (C:\Users\RUNNER~1\... on GitHub's runners) the folder is taken by its long name too
        let dir = std::fs::canonicalize(&dir).map(|p| std::path::PathBuf::from(p.to_string_lossy().trim_start_matches(r"\\?\"))).unwrap_or(dir);
        let file = dir.join("rapor ü.txt");
        std::fs::write(&file, b"x").unwrap();
        let d = dir.clone();
        let t0 = std::time::Instant::now();
        let waiter = std::thread::spawn(move || (wait_for_select(&d, Duration::from_secs(10)), t0.elapsed()));
        // the shell's side, on another thread (its own apartment)
        let shell = std::thread::spawn({
            let dir = dir.clone();
            move || unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).unwrap();
                let pidl = if chromium {
                    windows::Win32::UI::Shell::ILCreateFromPathW(&HSTRING::from(dir.as_os_str()))
                } else {
                    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
                    SHParseDisplayName(&HSTRING::from(dir.as_os_str()), None, &mut pidl, 0, None).unwrap();
                    pidl
                };
                let loc = InitVariantFromBuffer(pidl as *const _, ILGetSize(Some(pidl))).unwrap();
                let empty = VARIANT::default();
                let mut found = None;
                for _ in 0..100 {
                    let mut hwnd = 0i32;
                    if let Ok(disp) = windows.FindWindowSW(&loc, &empty, SWC_BROWSER, &mut hwnd, SWFO_NEEDDISPATCH) {
                        found = Some(disp);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                let disp = found.expect("registered window found");
                let app: IWebBrowserApp = disp.cast().unwrap();
                let doc = app.Document().unwrap();
                let sp: IServiceProvider = doc.cast().unwrap();
                let view: IShellView = sp.QueryService::<IShellView>(&IFolderView::IID).unwrap();
                // the file's ID list relative to the folder: its last item
                let mut fpidl: *mut ITEMIDLIST = std::ptr::null_mut();
                if chromium {
                    fpidl = windows::Win32::UI::Shell::ILCreateFromPathW(&HSTRING::from(dir.join("rapor ü.txt").as_os_str()));
                } else {
                    SHParseDisplayName(&HSTRING::from(dir.join("rapor ü.txt").as_os_str()), None, &mut fpidl, 0, None).unwrap();
                }
                let child = windows::Win32::UI::Shell::ILFindLastID(fpidl);
                view.SelectItem(child, (SVSI_SELECT.0 | SVSI_ENSUREVISIBLE.0 | SVSI_FOCUSED.0) as u32).unwrap();
                CoTaskMemFree(Some(pidl as *const _));
                CoTaskMemFree(Some(fpidl as *const _));
                CoUninitialize();
            }
        });
        shell.join().unwrap();
        let (got, took) = waiter.join().unwrap();
        eprintln!("selection after {took:?}");
        // the wait ends with the selection, not at its limit
        assert!(took < Duration::from_secs(5), "{took:?}");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(got.as_deref().map(|p| p.to_string_lossy().to_lowercase()), Some(file.to_string_lossy().to_lowercase()));
    }

    /// Manual (issue 97): is our window for a folder found when the shell is given the folder the way Chromium names it
    /// (`ILCreateFromPathW`) rather than the way we register it (`SHParseDisplayName`)? Downloads and a plain folder.
    /// `cargo test -p rf-win --lib shell_window::tests::probe_lookup_by_ilcreatefrompath -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_lookup_by_ilcreatefrompath() {
        let plain = std::env::temp_dir().join(format!("rf-shellwin-probe-{}", std::process::id()));
        std::fs::create_dir_all(&plain).unwrap();
        for dir in [PathBuf::from(std::env::var("USERPROFILE").unwrap()).join("Downloads"), plain.clone()] {
            let d = dir.clone();
            let waiter = std::thread::spawn(move || wait_for_select(&d, Duration::from_secs(3)));
            let found = unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).unwrap();
                let lookups = [("ILCreateFromPathW", windows::Win32::UI::Shell::ILCreateFromPathW(&HSTRING::from(dir.as_os_str()))), ("SHParseDisplayName", {
                    let mut p: *mut ITEMIDLIST = std::ptr::null_mut();
                    SHParseDisplayName(&HSTRING::from(dir.as_os_str()), None, &mut p, 0, None).unwrap();
                    p
                })];
                let mut out = Vec::new();
                std::thread::sleep(Duration::from_millis(300));
                for (how, pidl) in lookups {
                    let loc = InitVariantFromBuffer(pidl as *const _, ILGetSize(Some(pidl))).unwrap();
                    let empty = VARIANT::default();
                    let mut hwnd = 0i32;
                    let ok = windows.FindWindowSW(&loc, &empty, SWC_BROWSER, &mut hwnd, SWFO_NEEDDISPATCH).is_ok();
                    out.push(format!("{how} ({} bytes): {}", ILGetSize(Some(pidl)), if ok { "found" } else { "NOT found" }));
                    CoTaskMemFree(Some(pidl as *const _));
                }
                out
            };
            let _ = waiter.join();
            println!("{}: {}", dir.display(), found.join(", "));
        }
        let _ = std::fs::remove_dir_all(&plain);
    }
}
