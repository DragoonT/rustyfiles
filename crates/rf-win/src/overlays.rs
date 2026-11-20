//! Shell icon overlays: the marks sync and version-control programs put on icons in Explorer (Google Drive's tick, OneDrive's
//! cloud, TortoiseGit's states, Dropbox, ...). A user on Discord, 2026-11-20: "Enable overlay/status icons (from Google Drive,
//! TortoiseGit, etc.) so they appear as in Windows Explorer."
//!
//! Each program registers an overlay handler (`IShellIconOverlayIdentifier`) under
//! `HKLM\...\Explorer\ShellIconOverlayIdentifiers`. Explorer asks the folder (`IShellIconOverlay` of its `IShellFolder`), which asks the
//! shell's overlay manager: it loads the handlers, keeps Windows' limit of 15 in the registry's order, and answers with the index of
//! the overlay's picture in the system image list. That picture is icon-sized with the mark in its corner: it is drawn over the item's
//! icon at the same size. The folder's answer has the shortcut arrow too, which rustyfiles draws itself (the caller skips links).
//!
//! The handlers are the programs' own DLLs, loaded into this process as Explorer loads them into its own. They are asked on one
//! thread of their own (an STA, as in Explorer), never the UI thread: a handler that asks its service over a pipe can take a while,
//! and one folder's questions wait for the previous folder's to stop (`cancel`).

use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Controls::{IImageList, ILD_TRANSPARENT};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    IShellFolder, IShellIconOverlay, OI_DEFAULT, SHBindToObject, SHGetImageList, SHIL_EXTRALARGE, SHIL_JUMBO, SHIL_LARGE, SHIL_SMALL,
    SHParseDisplayName,
};
use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;
use windows::core::{Interface, PCWSTR};

use crate::thumbs::Bgra;

type Job = Box<dyn FnOnce() + Send>;

fn worker() -> &'static Mutex<mpsc::Sender<Job>> {
    static WORKER: OnceLock<Mutex<mpsc::Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let _ = std::thread::Builder::new().name("overlays".into()).spawn(move || {
            // SAFETY: COM is set up once for this thread, which lives as long as the process.
            unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            }
            for job in rx {
                job();
            }
        });
        Mutex::new(tx)
    })
}

/// Runs `f` on the overlay thread and waits for its answer (`None` when the thread is gone).
fn on_worker<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let (tx, rx) = mpsc::sync_channel(1);
    worker()
        .lock()
        .send(Box::new(move || {
            let _ = tx.send(f());
        }))
        .ok()?;
    rx.recv().ok()
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

/// A shell item id list from the shell's allocator, freed on drop.
struct Pidl(*mut ITEMIDLIST);

impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: the list came from the shell's task allocator and is freed once.
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
        }
    }
}

/// The folder `dir` and its overlay interface (`None` for a folder that has none, like a phone's).
fn overlay_folder(dir: &Path) -> Option<(IShellFolder, IShellIconOverlay)> {
    let w = wide(dir.as_os_str());
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    // SAFETY: `w` is NUL-terminated; the list is owned by `Pidl` from here.
    unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None).ok()? };
    let pidl = Pidl(pidl);
    // SAFETY: `pidl` is a valid absolute list for the call.
    let folder: IShellFolder = unsafe { SHBindToObject(None, pidl.0, None).ok()? };
    let overlay = folder.cast::<IShellIconOverlay>().ok()?;
    Some((folder, overlay))
}

/// The overlays of the items `names` of `dir`: (name, picture index) for each item that has one. Stops early when `cancel` is set.
/// Blocking: call off the UI thread.
pub fn indexes(dir: &Path, names: Vec<String>, cancel: Arc<AtomicBool>) -> Vec<(String, i32)> {
    let dir = dir.to_path_buf();
    on_worker(move || {
        let Some((folder, overlay)) = overlay_folder(&dir) else { return Vec::new() };
        let mut out = Vec::new();
        for name in names {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let mut w = wide(std::ffi::OsStr::new(&name));
            let mut child: *mut ITEMIDLIST = std::ptr::null_mut();
            // SAFETY: `w` is NUL-terminated and writable as the call wants; the child list is owned by `Pidl` from here.
            if unsafe { folder.ParseDisplayName(windows::Win32::Foundation::HWND::default(), None, windows::core::PWSTR(w.as_mut_ptr()), None, &mut child, std::ptr::null_mut()) }.is_err() {
                continue;
            }
            let child = Pidl(child);
            let mut index = OI_DEFAULT as i32;
            // SAFETY: `child` is a list relative to `folder`; `index` is a valid in/out pointer.
            if unsafe { overlay.GetOverlayIconIndex(child.0, &mut index) }.is_ok() && index > 0 {
                out.push((name, index));
            }
        }
        out
    })
    .unwrap_or_default()
}

/// The overlay picture `index` (from [`indexes`]) at about `px` pixels: the system image list's size at or above it.
pub fn picture(index: i32, px: u32) -> Option<Bgra> {
    let list = match px {
        0..=16 => SHIL_SMALL,
        17..=32 => SHIL_LARGE,
        33..=48 => SHIL_EXTRALARGE,
        _ => SHIL_JUMBO,
    };
    on_worker(move || {
        // SAFETY: the image list is the process's system one; the icon it hands out is ours to destroy.
        unsafe {
            let il: IImageList = SHGetImageList(list as i32).ok()?;
            let icon = il.GetIcon(index, ILD_TRANSPARENT.0).ok()?;
            let b = crate::dock::icons::hicon_to_bgra(icon);
            let _ = DestroyIcon(icon);
            b
        }
    })
    .flatten()
}
