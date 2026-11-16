//! Taskbar jump list (right-click on the taskbar button): "New window" / "New tab" tasks, the
//! pinned folders and the folders used last, like Files. Blocking + COM: call off the UI thread.

use std::path::{Path, PathBuf};

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink,
};
use windows::Win32::Storage::EnhancedStorage::PKEY_Title;
use windows::core::{Interface, PCWSTR};

use crate::shell::ensure_com;
use crate::wide::wide;

/// Command-line switch of the "New window" task.
pub const NEW_WINDOW_ARG: &str = "--new-window";

fn link(exe: &Path, args: &str, title: &str, icon: Option<(&str, i32)>) -> windows::core::Result<IShellLinkW> {
    unsafe {
        let l: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let we = wide(exe);
        l.SetPath(PCWSTR(we.as_ptr()))?;
        let wa = wide(args);
        l.SetArguments(PCWSTR(wa.as_ptr()))?;
        match icon {
            Some((file, ix)) => {
                let wi = wide(file);
                l.SetIconLocation(PCWSTR(wi.as_ptr()), ix)?;
            }
            None => l.SetIconLocation(PCWSTR(we.as_ptr()), 0)?,
        }
        let store: IPropertyStore = l.cast()?;
        store.SetValue(&PKEY_Title, &PROPVARIANT::from(title))?;
        store.Commit()?;
        Ok(l)
    }
}

/// A named group of folders in the jump list: `(title, folder)` each.
pub type Category = (String, Vec<(String, PathBuf)>);

/// One write of the list at a time in this process.
///
/// WHY. `CommitList` writes the list's file by replacing it. Two commits at once collide, and the loser fails with "unable to
/// remove the file to be replaced" (0x80070497): nine of those within five seconds in a user's log (the log in issue 33,
/// 2026-11-15), where every change of the recent folders started a write of its own on a thread of its own.
static WRITING: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// How often a write is tried again when the list's file was in other hands, and the wait before the first of those tries.
const BUSY_RETRIES: u32 = 2;
const BUSY_WAIT_MS: u64 = 150;

/// Whether a failed write is worth another try: the list's file was held for a moment (another program's commit for the same
/// taskbar identity, the shell reading it). Windows' codes for a file that cannot be replaced, is shared or is locked:
/// ERROR_UNABLE_TO_REMOVE_REPLACED, ERROR_UNABLE_TO_MOVE_REPLACEMENT and its second form, ERROR_SHARING_VIOLATION,
/// ERROR_LOCK_VIOLATION; and ERROR_ALREADY_EXISTS, which is what one of six writers at once got here (`writers_at_once_without_turns`,
/// 2026-11-16: three of them 0x80070497, one 0x800700B7, two got through).
pub fn busy(hresult: u32) -> bool {
    matches!(hresult, 0x8007_0497 | 0x8007_0498 | 0x8007_0499 | 0x8007_0020 | 0x8007_0021 | 0x8007_00B7)
}

/// Replaces the jump list: tasks + categories of folders, in the order given (Windows shows
/// only as many items as the list has room for; the first category is filled first).
/// Writers take turns (see [`WRITING`]), and a write that found the list's file busy is tried again.
pub fn set(exe: &Path, tasks: &[(&str, &str)], categories: &[Category]) -> anyhow::Result<()> {
    ensure_com();
    let _turn = WRITING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut tried = 0;
    loop {
        match write(exe, tasks, categories) {
            Ok(()) => return Ok(()),
            Err(e) if tried < BUSY_RETRIES && busy(e.code().0 as u32) => {
                tried += 1;
                std::thread::sleep(std::time::Duration::from_millis(BUSY_WAIT_MS * u64::from(tried)));
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// One write, as it is: no turn, no second try (see [`set`]).
fn write(exe: &Path, tasks: &[(&str, &str)], categories: &[Category]) -> windows::core::Result<()> {
    unsafe {
        let list: ICustomDestinationList = CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER)?;
        let mut slots = 0u32;
        let _removed: IObjectArray = list.BeginList(&mut slots)?;
        let folder_icon = std::env::var("SystemRoot").map(|r| format!(r"{r}\System32\imageres.dll")).unwrap_or_default();
        let mut room = slots.max(5) as usize;
        for (cat_title, folders) in categories {
            if folders.is_empty() || room == 0 {
                continue;
            }
            let coll: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
            for (title, path) in folders.iter().take(room) {
                let args = format!("\"{}\"", path.display());
                if let Ok(l) = link(exe, &args, title, Some((&folder_icon, 3))) {
                    coll.AddObject(&l)?;
                }
            }
            room = room.saturating_sub(folders.len());
            let wt = wide(cat_title);
            list.AppendCategory(PCWSTR(wt.as_ptr()), &coll.cast::<IObjectArray>()?)?;
        }
        if !tasks.is_empty() {
            let coll: IObjectCollection = CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER)?;
            for (title, args) in tasks {
                coll.AddObject(&link(exe, args, title, None)?)?;
            }
            list.AddUserTasks(&coll.cast::<IObjectArray>()?)?;
        }
        list.CommitList()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    /// Writes a jump list for the *test binary's* own taskbar identity (not the app's).
    #[test]
    fn builds_jump_list() {
        let exe = std::env::current_exe().unwrap();
        let pins = vec![("Belgeler".to_string(), std::env::temp_dir())];
        let recent = vec![("Son".to_string(), std::env::temp_dir()), ("Önceki".to_string(), std::env::temp_dir())];
        let cats = [("Sabitlenenler".to_string(), pins), ("Son klasörler".to_string(), recent), ("Boş".to_string(), Vec::new())];
        super::set(&exe, &[("Yeni pencere", super::NEW_WINDOW_ARG)], &cats).unwrap();
    }

    fn one_category(i: usize) -> [super::Category; 1] {
        [("Son klasörler".to_string(), vec![(format!("Klasör {i}"), std::env::temp_dir())])]
    }

    /// Several writers at once each get their turn (they collided until 2026-11-16: see `WRITING`).
    #[test]
    fn writers_at_once_take_turns() {
        let exe = std::env::current_exe().unwrap();
        let writers: Vec<_> = (0..6)
            .map(|i| {
                let exe = exe.clone();
                std::thread::spawn(move || super::set(&exe, &[("Yeni pencere", super::NEW_WINDOW_ARG)], &one_category(i)).map_err(|e| format!("{e:#}")))
            })
            .collect();
        for w in writers {
            w.join().unwrap().unwrap();
        }
    }

    /// What the turns prevent, for a look (`--ignored --nocapture`): the same writers without them. It shows Windows' behaviour,
    /// which is why it asserts nothing: on a quiet machine every one of them may get through.
    #[test]
    #[ignore]
    fn writers_at_once_without_turns() {
        let exe = std::env::current_exe().unwrap();
        let writers: Vec<_> = (0..6)
            .map(|i| {
                let exe = exe.clone();
                std::thread::spawn(move || {
                    crate::shell::ensure_com();
                    super::write(&exe, &[("Yeni pencere", super::NEW_WINDOW_ARG)], &one_category(i)).map_err(|e| format!("{:#x} {}", e.code().0, e.message()))
                })
            })
            .collect();
        for (i, w) in writers.into_iter().enumerate() {
            eprintln!("writer {i}: {:?}", w.join().unwrap());
        }
    }

    #[test]
    fn a_list_file_in_other_hands_is_worth_another_try_other_failures_are_not() {
        for code in [0x8007_0497u32, 0x8007_0498, 0x8007_0499, 0x8007_0020, 0x8007_0021, 0x8007_00B7] {
            assert!(super::busy(code), "{code:#x}");
        }
        // access denied (a policy that switches the lists off), not found, out of memory, a generic failure, success
        for code in [0x8007_0005u32, 0x8007_0002, 0x8007_000E, 0x8000_4005, 0] {
            assert!(!super::busy(code), "{code:#x}");
        }
    }
}