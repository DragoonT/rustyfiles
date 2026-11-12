//! The one-time import of Explorer's current icon positions, read-only (shell plan F2, note 4, 5.4).
//!
//! THE ROUTE CHOSEN: `IShellWindows::FindWindowSW(CSIDL_DESKTOP, SWC_DESKTOP)` -> the desktop's `IShellBrowser` -> its `IShellView` ->
//! `IFolderView`, then `Items(SVGIO_ALLVIEW)` and `GetItemPosition(pidl)` for each item. Why this one: it is the only route Microsoft
//! documents for icon positions (Raymond Chen's desktop-icon samples). The alternative, the binary `HKCU\...\Shell\Bags\1\Desktop\
//! IconLayouts` value, is undocumented, its coordinates have never been decoded in public, and it may have moved on newer builds; it
//! is not parsed. Sending messages straight to the desktop's list view was cut off on Windows 10 1809 and is not done.
//!
//! ONLY READS: three interface calls on Explorer's view (`GetFolder`, `Items`, `GetItemPosition`) and name lookups on its folder. It
//! never calls `SelectAndPositionItems` or anything else that moves or changes an icon. It runs on the shell thread through
//! `sta::call` with a limit, so a hung Explorer costs the limit and not a frozen caller.
//!
//! WHAT IS NOT KNOWN: the units and origin of the points (shell plan test T13). This file only returns them as they are; turning them
//! into cells is `rf_core::desktop::import::convert`, which refuses numbers that do not fit the monitors. If Explorer is not the shell
//! (no desktop view to ask) the answer is [`ImportFailure::NoExplorerDesktop`], not an empty success.

use std::path::{Path, PathBuf};

use rf_core::desktop::import::{ImportFailure, RawPosition};
use rf_core::desktop::items::{FolderOrigin, ItemId};
use rf_core::desktop::virtuals::VirtualKind;
use rf_core::desktop::wallpaper::normalise_path;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, IServiceProvider};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::UI::Shell::{
    IEnumIDList, IFolderView, IShellBrowser, IShellFolder, IShellWindows, SHGDN_FORPARSING, SID_STopLevelBrowser, SVGIO_ALLVIEW, ShellWindows, StrRetToStrW, SWC_DESKTOP,
    SWFO_NEEDDISPATCH,
};
use windows::core::PWSTR;

/// Explorer's positions, with counts of what could not be named.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ExplorerPositions {
    pub positions: Vec<RawPosition>,
    /// Items in Explorer's view.
    pub total: usize,
    /// Items whose name is neither in one of the two Desktop folders nor one of our virtual items (the user's profile folder, OneDrive, a
    /// library ...): they are not imported and are placed fresh.
    pub unmapped: usize,
}

/// What a parsing name from Explorer's view is, in our identities. A file-system item comes as its full path
/// (`C:\Users\x\Desktop\a.txt`), a virtual one as `::{CLSID}`. Case, slash direction and a `\\?\` prefix do not matter.
pub fn classify(parsing: &str, user_dir: Option<&Path>, public_dir: Option<&Path>) -> Option<ItemId> {
    let p = parsing.trim();
    if p.starts_with("::{") {
        return VirtualKind::from_clsid(p).map(ItemId::virtual_item);
    }
    let path = PathBuf::from(p.replace('/', "\\"));
    let parent = normalise_path(path.parent()?).trim_end_matches('\\').to_string();
    let name = path.file_name()?.to_str()?;
    for (origin, dir) in [(FolderOrigin::User, user_dir), (FolderOrigin::Public, public_dir)] {
        if let Some(d) = dir
            && normalise_path(d).trim_end_matches('\\') == parent
        {
            return Some(ItemId::file(origin, name));
        }
    }
    None
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn com(step: &str, e: windows::core::Error) -> ImportFailure {
    ImportFailure::Com(format!("{step}: {e}"))
}

/// Must run on the shell thread (an STA that pumps).
fn read_on_shell_thread(user_dir: Option<PathBuf>, public_dir: Option<PathBuf>) -> Result<ExplorerPositions, ImportFailure> {
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).map_err(|e| com("CoCreateInstance(ShellWindows)", e))?;
        let loc = VARIANT::from(0i32); // CSIDL_DESKTOP
        let root = VARIANT::default();
        let mut hwnd = 0i32;
        // not found is S_FALSE with a null dispatch, which the binding turns into an error
        let disp = windows.FindWindowSW(&loc, &root, SWC_DESKTOP, &mut hwnd, SWFO_NEEDDISPATCH).map_err(|_| ImportFailure::NoExplorerDesktop)?;
        let sp: IServiceProvider = windows::core::Interface::cast(&disp).map_err(|e| com("IServiceProvider", e))?;
        let browser: IShellBrowser = sp.QueryService(&SID_STopLevelBrowser).map_err(|e| com("QueryService(top level browser)", e))?;
        let view = browser.QueryActiveShellView().map_err(|e| com("QueryActiveShellView", e))?;
        let fv: IFolderView = windows::core::Interface::cast(&view).map_err(|e| com("IFolderView", e))?;
        let folder: IShellFolder = fv.GetFolder().map_err(|e| com("IFolderView::GetFolder", e))?;
        let en: IEnumIDList = fv.Items(SVGIO_ALLVIEW).map_err(|e| com("IFolderView::Items", e))?;

        let mut out = ExplorerPositions::default();
        loop {
            let mut batch = [std::ptr::null_mut::<ITEMIDLIST>(); 32];
            let mut got = 0u32;
            let hr = en.Next(&mut batch, Some(&mut got));
            for &pidl in batch.iter().take(got as usize) {
                out.total += 1;
                let named = (|| {
                    let mut sr = STRRET::default();
                    folder.GetDisplayNameOf(pidl, SHGDN_FORPARSING, &mut sr).ok()?;
                    let mut ws = PWSTR::null();
                    StrRetToStrW(&mut sr, Some(pidl), &mut ws).ok()?;
                    classify(&take(ws), user_dir.as_deref(), public_dir.as_deref())
                })();
                match (named, fv.GetItemPosition(pidl)) {
                    (Some(id), Ok(pt)) => out.positions.push(RawPosition { id, x: pt.x, y: pt.y }),
                    _ => out.unmapped += 1,
                }
                CoTaskMemFree(Some(pidl as *const _));
            }
            if hr.0 != 0 || got == 0 {
                break; // S_FALSE: the end
            }
        }
        if out.total == 0 {
            return Err(ImportFailure::NoItems);
        }
        Ok(out)
    }
}

/// Reads the positions Explorer's desktop view reports now. Blocks up to the shell thread's limit.
pub fn read_explorer_positions(user_dir: Option<PathBuf>, public_dir: Option<PathBuf>) -> Result<ExplorerPositions, ImportFailure> {
    // Without a shell window there is no Explorer desktop to ask: say so without activating anything (the COM class would be started
    // by the system for us, to find nothing)
    if !explorer_is_the_shell() {
        return Err(ImportFailure::NoExplorerDesktop);
    }
    match super::COM_ACTIVATION.call(std::time::Duration::from_secs(10), move || read_on_shell_thread(user_dir, public_dir)) {
        Ok(r) => r,
        Err(_) => Err(ImportFailure::TimedOut),
    }
}

/// Some process owns the shell window (`GetShellWindow`): Explorer, until this app takes the job over.
pub fn explorer_is_the_shell() -> bool {
    unsafe { !windows::Win32::UI::WindowsAndMessaging::GetShellWindow().0.is_null() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_explorers_view_become_our_identities() {
        let u = Path::new(r"C:\Users\Ada\OneDrive\Masaüstü");
        let p = Path::new(r"C:\Users\Public\Desktop");
        let id = |s: &str| classify(s, Some(u), Some(p));
        assert_eq!(id(r"C:\Users\Ada\OneDrive\Masaüstü\Notes.TXT"), Some(ItemId::user("notes.txt")));
        assert_eq!(id(r"c:\users\public\desktop\Zoom.lnk"), Some(ItemId::public("zoom.lnk")));
        assert_eq!(id(r"\\?\C:\Users\Public\Desktop\Zoom.lnk"), Some(ItemId::public("zoom.lnk")));
        assert_eq!(id("C:/Users/Ada/OneDrive/Masaüstü/slash.txt"), Some(ItemId::user("slash.txt")));
        assert_eq!(id("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}"), Some(ItemId::virtual_item(VirtualKind::ThisPc)));
        assert_eq!(id("::{645ff040-5081-101b-9f08-00aa002f954e}"), Some(ItemId::virtual_item(VirtualKind::RecycleBin)));
        // not ours: an unknown shell object, a folder somewhere else, a deeper path, a bare name
        assert_eq!(id("::{031E4825-7B94-4DC3-B131-E946B44C8DD5}"), None); // libraries
        assert_eq!(id(r"C:\Users\Ada"), None);
        assert_eq!(id(r"C:\Users\Ada\OneDrive\Masaüstü\Sub\deep.txt"), None);
        assert_eq!(id("Notes.txt"), None);
        assert_eq!(id(""), None);
        // no known folder: nothing file-system can be named
        assert_eq!(classify(r"C:\Users\Ada\OneDrive\Masaüstü\Notes.TXT", None, None), None);
        // a trailing separator on the folder does not matter
        assert_eq!(classify(r"C:\d\a.txt", Some(Path::new(r"C:\d\")), None), Some(ItemId::user("a.txt")));
    }
}
