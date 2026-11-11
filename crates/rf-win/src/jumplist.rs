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

/// Replaces the jump list: tasks + categories of folders, in the order given (Windows shows
/// only as many items as the list has room for; the first category is filled first).
pub fn set(exe: &Path, tasks: &[(&str, &str)], categories: &[Category]) -> anyhow::Result<()> {
    ensure_com();
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
}