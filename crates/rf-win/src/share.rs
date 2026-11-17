//! Windows' share sheet for files and folders, opened for one of our windows.
//!
//! WHY NOT THE CONTEXT MENU'S VERB (issue 48, "Share button does not work", 0.8.6 and 0.8.7): the button asked the shell's
//! context menu of the items to run its `Windows.ModernShare` verb, from the hidden owner window of the shell thread. On this PC
//! that failed with 0x80070483 ("no application is associated with the specified file for this operation") for an ordinary text
//! file, so the sheet never came. The verb is Explorer's own entry, not a promise to other programs. What Windows offers desktop
//! programs is the share interface of a window (`IDataTransferManagerInterop`): ask for the window's data transfer manager, answer
//! its `DataRequested` with the items, and show the sheet for that window.
//!
//! TWO STEPS ON TWO THREADS. [`prepare`] looks the items up as storage items, on a worker: waiting for that on the interface's
//! thread let Windows pump the window's messages inside the wait, and the window drew itself while the app was in the middle of
//! the click ("RefCell already borrowed" in the log, seen in a test copy). [`show`] then runs on the window's own thread, outside
//! any work of the app's.

use std::cell::RefCell;
use std::path::PathBuf;

use anyhow::Context as _;
use windows::ApplicationModel::DataTransfer::{DataRequestedEventArgs, DataTransferManager};
use windows::Foundation::TypedEventHandler;
use windows::Storage::{IStorageItem, StorageFile, StorageFolder};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::Shell::IDataTransferManagerInterop;
use windows::core::{AgileReference, HSTRING, Interface};
use windows_collections::IIterable;

thread_local! {
    /// The handler of the last sheet, taken off before the next one is put on (a window's manager keeps its handlers).
    static LAST: RefCell<Option<(DataTransferManager, i64)>> = const { RefCell::new(None) };
}

/// The items of a sheet, looked up and ready to hand over (any thread may hold it).
pub struct Prepared {
    items: AgileReference<IIterable<IStorageItem>>,
    count: usize,
}

/// Looks `paths` up as storage items. Blocks for a few milliseconds per item: call it on a worker, never on a window's thread.
pub fn prepare(paths: &[PathBuf]) -> anyhow::Result<Prepared> {
    anyhow::ensure!(!paths.is_empty(), "nothing to share");
    // (a worker of a pool may have no apartment: this makes sure the process has the multithreaded one, which it then joins)
    unsafe {
        let _ = windows::Win32::System::Com::CoIncrementMTAUsage();
    }
    let mut items: Vec<Option<IStorageItem>> = Vec::with_capacity(paths.len());
    for p in paths {
        let h = HSTRING::from(p.as_os_str());
        let item: IStorageItem = if p.is_dir() {
            StorageFolder::GetFolderFromPathAsync(&h)?.join().with_context(|| format!("folder {}", p.display()))?.cast()?
        } else {
            StorageFile::GetFileFromPathAsync(&h)?.join().with_context(|| format!("file {}", p.display()))?.cast()?
        };
        items.push(Some(item));
    }
    let list: IIterable<IStorageItem> = items.into();
    Ok(Prepared { items: AgileReference::new(&list)?, count: paths.len() })
}

/// Shows the share sheet with `items` over the window `hwnd`. On the thread that owns the window, outside the app's own work.
pub fn show(hwnd: isize, items: Prepared, title: &str) -> anyhow::Result<()> {
    let title = HSTRING::from(title);
    let interop = windows::core::factory::<DataTransferManager, IDataTransferManagerInterop>()?;
    let window = HWND(hwnd as *mut _);
    let manager: DataTransferManager = unsafe { interop.GetForWindow(window)? };
    LAST.with(|l| {
        if let Some((m, token)) = l.borrow_mut().take() {
            let _ = m.RemoveDataRequested(token);
        }
    });
    let count = items.count;
    let list = items.items;
    let token = manager.DataRequested(&TypedEventHandler::<DataTransferManager, DataRequestedEventArgs>::new(move |_, args| {
        let Some(args) = args.as_ref() else { return Ok(()) };
        let data = args.Request()?.Data()?;
        data.Properties()?.SetTitle(&title)?;
        data.SetStorageItemsReadOnly(&list.resolve()?)?;
        log::info!("share: {count} item(s) handed to the sheet");
        Ok(())
    }))?;
    LAST.with(|l| *l.borrow_mut() = Some((manager, token)));
    unsafe { interop.ShowShareUIForWindow(window)? };
    log::info!("share: the sheet was asked for, {count} item(s)");
    Ok(())
}
