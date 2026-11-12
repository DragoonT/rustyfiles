//! Recycle Bin through the shell namespace (list / restore / empty / info).
//! Blocking + COM: call off the UI thread.

use windows::Win32::Foundation::{HWND, PROPERTYKEY, S_OK};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::Common::{ITEMIDLIST, STRRET};
use windows::Win32::System::SystemServices::{SFGAO_FOLDER, SFGAO_STREAM};
use windows::Win32::System::Variant::{PSTF_UTC, VARIANT, VariantToFileTime, VariantToStringAlloc, VariantToUInt64};
use windows::Win32::UI::Shell::{
    CMF_NORMAL, CMINVOKECOMMANDINFO, FOLDERID_RecycleBinFolder, IContextMenu, IEnumIDList, IShellFolder2,
    KF_FLAG_DEFAULT, SHBindToObject, SHCONTF_FOLDERS, SHCONTF_INCLUDEHIDDEN, SHCONTF_NONFOLDERS,
    SEE_MASK_FLAG_NO_UI, SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND, SHGDN_FORPARSING, SHGDN_NORMAL,
    SHGetKnownFolderIDList, SHQUERYRBINFO, SHQueryRecycleBinW, StrRetToBufW,
};
use windows::Win32::UI::WindowsAndMessaging::{CreatePopupMenu, DestroyMenu, SW_SHOWNORMAL};
use windows::core::{GUID, PCSTR, PCWSTR, s};

use crate::shell::ensure_com;

/// One item in the Recycle Bin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecycleItem {
    /// Original file name as shown by Explorer.
    pub name: String,
    /// Folder the item was deleted from.
    pub original_location: String,
    /// Deletion time, Unix milliseconds (UTC); 0 when unknown.
    pub deleted_ms: i64,
    pub size: u64,
    /// Real location inside `$Recycle.Bin\<SID>\$R...`; also the key used by [`restore`].
    pub parsing_path: String,
    pub is_dir: bool,
}

// {9B174B33-40FF-11D2-A27E-00C04FC30871}: pid 2 = original location, pid 3 = date deleted.
const FMTID_DISPLACED: GUID = GUID::from_u128(0x9B174B33_40FF_11D2_A27E_00C04FC30871);
const PKEY_DELETED_FROM: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_DISPLACED, pid: 2 };
const PKEY_DATE_DELETED: PROPERTYKEY = PROPERTYKEY { fmtid: FMTID_DISPLACED, pid: 3 };
// System.Size
const PKEY_SIZE: PROPERTYKEY =
    PROPERTYKEY { fmtid: GUID::from_u128(0xB725F130_47EF_101A_A5F1_02608C9EEBAC), pid: 12 };

struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
        }
    }
}

fn bin_folder() -> anyhow::Result<IShellFolder2> {
    ensure_com();
    unsafe {
        let pidl = Pidl(SHGetKnownFolderIDList(&FOLDERID_RecycleBinFolder, KF_FLAG_DEFAULT.0 as u32, None)?);
        Ok(SHBindToObject::<_, _, IShellFolder2>(None, pidl.0, None)?)
    }
}

/// Calls `f(folder, relative_pidl)` for each item in the bin.
fn for_each_item(folder: &IShellFolder2, mut f: impl FnMut(&IShellFolder2, *mut ITEMIDLIST)) -> anyhow::Result<()> {
    unsafe {
        let mut en: Option<IEnumIDList> = None;
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
        let hr = folder.EnumObjects(HWND::default(), flags, &mut en);
        if hr != S_OK {
            hr.ok()?; // S_FALSE with no enumerator = empty bin
            return Ok(());
        }
        let Some(en) = en else { return Ok(()) };
        loop {
            let mut item = [std::ptr::null_mut::<ITEMIDLIST>()];
            let mut fetched = 0u32;
            if en.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                break;
            }
            let p = Pidl(item[0]);
            f(folder, p.0);
        }
    }
    Ok(())
}

fn display_name(folder: &IShellFolder2, pidl: *mut ITEMIDLIST, flags: windows::Win32::UI::Shell::SHGDNF) -> String {
    unsafe {
        let mut sr = STRRET::default();
        if folder.GetDisplayNameOf(pidl as *const _, flags, &mut sr).is_err() {
            return String::new();
        }
        let mut buf = [0u16; 1024];
        if StrRetToBufW(&mut sr, Some(pidl as *const _), &mut buf).is_ok() {
            crate::wide::from_wide(&buf)
        } else {
            String::new()
        }
    }
}

fn detail(folder: &IShellFolder2, pidl: *mut ITEMIDLIST, key: &PROPERTYKEY) -> Option<VARIANT> {
    unsafe { folder.GetDetailsEx(pidl as *const _, key).ok() }
}

fn read_item(folder: &IShellFolder2, pidl: *mut ITEMIDLIST) -> RecycleItem {
    // The bin reports the full original path as the "normal" name; keep only the file name.
    let full = display_name(folder, pidl, SHGDN_NORMAL);
    let name = full.rsplit('\\').next().unwrap_or(&full).to_string();
    let parsing_path = display_name(folder, pidl, SHGDN_FORPARSING);
    let original_location = detail(folder, pidl, &PKEY_DELETED_FROM)
        .and_then(|v| unsafe { VariantToStringAlloc(&v).ok() })
        .map(|p| unsafe {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
        .unwrap_or_default();
    let deleted_ms = detail(folder, pidl, &PKEY_DATE_DELETED)
        .and_then(|v| unsafe { VariantToFileTime(&v, PSTF_UTC).ok() })
        .map(|ft| rf_core::format::filetime_to_unix_ms(((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64))
        .unwrap_or(0);
    let size = detail(folder, pidl, &PKEY_SIZE).and_then(|v| unsafe { VariantToUInt64(&v).ok() }).unwrap_or(0);
    // A compressed (zip) folder is a folder to the shell but also a stream (a file); only a folder the file system agrees is one is
    // drawn as one. The file system is asked only for what the shell calls a folder (the rest cost no extra call).
    let mut attrs: u32 = SFGAO_FOLDER.0 | SFGAO_STREAM.0;
    let shell_ok = unsafe { folder.GetAttributesOf(&[pidl as *const _], &mut attrs) }.is_ok();
    let shell_folder = shell_ok && attrs & SFGAO_FOLDER.0 != 0;
    let shell_stream = shell_ok && attrs & SFGAO_STREAM.0 != 0;
    // (a junction or a directory link is not a directory to a look that does not follow links: for those the shell decides)
    let fs_is_dir = if shell_folder {
        std::fs::symlink_metadata(&parsing_path).ok().and_then(|m| rf_core::recycle_bin::fs_dir_answer(m.is_dir(), m.file_type().is_symlink()))
    } else {
        None
    };
    let is_dir = shell_folder && rf_core::recycle_bin::is_real_folder(shell_folder, shell_stream, fs_is_dir);
    RecycleItem { name, original_location, deleted_ms, size, parsing_path, is_dir }
}

/// Counts of what the shell says about the bin's items against what the file system says (a read-only probe, counts only):
/// `(items, shell says folder, shell says folder and stream, file system says directory, shell says folder but the file system a file)`.
pub fn attribute_survey() -> anyhow::Result<(usize, usize, usize, usize, usize)> {
    let folder = bin_folder()?;
    let mut s = (0usize, 0usize, 0usize, 0usize, 0usize);
    for_each_item(&folder, |f, p| {
        s.0 += 1;
        let mut attrs: u32 = SFGAO_FOLDER.0 | SFGAO_STREAM.0;
        let ok = unsafe { f.GetAttributesOf(&[p as *const _], &mut attrs) }.is_ok();
        let (shell_folder, stream) = (ok && attrs & SFGAO_FOLDER.0 != 0, ok && attrs & SFGAO_STREAM.0 != 0);
        let fs_dir = std::fs::symlink_metadata(display_name(f, p, SHGDN_FORPARSING)).ok().map(|m| m.is_dir());
        s.1 += usize::from(shell_folder);
        s.2 += usize::from(shell_folder && stream);
        s.3 += usize::from(fs_dir == Some(true));
        s.4 += usize::from(shell_folder && fs_dir == Some(false));
    })?;
    Ok(s)
}

/// The keys ([`rf_core::recycle_bin::key_of`]) of everything in the bin, and nothing else is read from the items (no sizes, dates or
/// attributes): the deletion lists the bin before and after, and a full listing of a big bin on the shared shell thread is costly.
pub fn list_keys_only() -> anyhow::Result<Vec<String>> {
    let folder = bin_folder()?;
    let mut out = Vec::new();
    for_each_item(&folder, |f, p| out.push(rf_core::recycle_bin::key_of(&display_name(f, p, SHGDN_FORPARSING))))?;
    Ok(out)
}

/// The bin's own verbs (restore, delete) are carried out by the shell's file engine; with no shell window they are refused with a true
/// message (`engine_guard::NoShellEngine`) instead of being started. There is no time limit on the verb itself once it runs: it has no
/// sign of life to wait for, and its conflict prompts wait for the user.
fn refuse_without_shell(op: crate::engine_guard::Op) -> anyhow::Result<()> {
    use crate::engine_guard::{NoShellEngine, Plan, plan, shell_present};
    match plan(op, shell_present()) {
        Plan::Shell => Ok(()),
        _ => Err(NoShellEngine { op }.into()),
    }
}

/// Lists everything in the Recycle Bin (all drives), in shell order.
pub fn list() -> anyhow::Result<Vec<RecycleItem>> {
    let folder = bin_folder()?;
    let mut out = Vec::new();
    for_each_item(&folder, |f, p| out.push(read_item(f, p)))?;
    Ok(out)
}

/// Restores `items` to their original locations by invoking the shell's "undelete" verb
/// (name-collision prompts come from the shell). Items no longer in the bin are ignored;
/// errors if none matched.
pub fn restore(items: &[RecycleItem]) -> anyhow::Result<()> {
    if items.is_empty() {
        return Ok(());
    }
    // the bin's verb runs the shell's file engine: with no shell window it would sit in its own progress window for ever
    refuse_without_shell(crate::engine_guard::Op::RestoreFromBin)?;
    let folder = bin_folder()?;
    let wanted: Vec<String> = items.iter().map(|i| i.parsing_path.to_lowercase()).collect();
    // Keep the matching pidls alive past enumeration.
    let mut found: Vec<Pidl> = Vec::new();
    unsafe {
        let mut en: Option<IEnumIDList> = None;
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
        let hr = folder.EnumObjects(HWND::default(), flags, &mut en);
        if let (S_OK, Some(en)) = (hr, en) {
            loop {
                let mut item = [std::ptr::null_mut::<ITEMIDLIST>()];
                let mut fetched = 0u32;
                if en.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                    break;
                }
                let p = Pidl(item[0]);
                if wanted.contains(&display_name(&folder, p.0, SHGDN_FORPARSING).to_lowercase()) {
                    found.push(p);
                }
            }
        }
        anyhow::ensure!(!found.is_empty(), "Öğeler Geri Dönüşüm Kutusu'nda bulunamadı");
        let raw: Vec<*const ITEMIDLIST> = found.iter().map(|p| p.0 as *const _).collect();
        let cm: IContextMenu = folder.GetUIObjectOf(HWND::default(), &raw, None)?;
        let menu = CreatePopupMenu()?;
        let res = (|| -> anyhow::Result<()> {
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL).ok()?;
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                lpVerb: PCSTR(s!("undelete").as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            cm.InvokeCommand(&info)?;
            Ok(())
        })();
        let _ = DestroyMenu(menu);
        res
    }
}

/// The items of the bin whose key (`rf_core::recycle_bin::key_of` of the parsing path) is in `wanted`. Keeps the matching ID lists
/// alive for the caller; nothing is changed.
fn find_by_keys(folder: &IShellFolder2, wanted: &[String]) -> anyhow::Result<Vec<Pidl>> {
    let mut found: Vec<Pidl> = Vec::new();
    unsafe {
        let mut en: Option<IEnumIDList> = None;
        let flags = (SHCONTF_FOLDERS.0 | SHCONTF_NONFOLDERS.0 | SHCONTF_INCLUDEHIDDEN.0) as u32;
        let hr = folder.EnumObjects(HWND::default(), flags, &mut en);
        if let (S_OK, Some(en)) = (hr, en) {
            loop {
                let mut item = [std::ptr::null_mut::<ITEMIDLIST>()];
                let mut fetched = 0u32;
                if en.Next(&mut item, Some(&mut fetched)) != S_OK || fetched == 0 {
                    break;
                }
                let p = Pidl(item[0]);
                let key = rf_core::recycle_bin::key_of(&display_name(folder, p.0, SHGDN_FORPARSING));
                if !key.is_empty() && wanted.contains(&key) {
                    found.push(p);
                }
            }
        }
    }
    Ok(found)
}

/// Deletes exactly the items whose keys are given, for good, by invoking the bin's own "delete" verb on their ID lists
/// (the same route as [`restore`] takes with "undelete"). The verb is the bin's, so it removes the item's `$R` data and `$I`
/// record together, and its scope is the list of ID lists handed to `GetUIObjectOf`, which are only those found by key.
/// No keys, or none found: an error, nothing is invoked. NOT RUN against a real bin by the author; see docs/design/recycle-bin.md.
pub fn delete_by_keys(keys: &[String]) -> anyhow::Result<()> {
    anyhow::ensure!(!keys.is_empty(), "no items to delete");
    refuse_without_shell(crate::engine_guard::Op::DeleteFromBin)?;
    let folder = bin_folder()?;
    let found = find_by_keys(&folder, keys)?;
    anyhow::ensure!(!found.is_empty(), "Öğeler Geri Dönüşüm Kutusu'nda bulunamadı");
    anyhow::ensure!(found.len() <= keys.len(), "more items matched than were asked for");
    unsafe {
        let raw: Vec<*const ITEMIDLIST> = found.iter().map(|p| p.0 as *const _).collect();
        let cm: IContextMenu = folder.GetUIObjectOf(HWND::default(), &raw, None)?;
        let menu = CreatePopupMenu()?;
        let res = (|| -> anyhow::Result<()> {
            cm.QueryContextMenu(menu, 0, 1, 0x7FFF, CMF_NORMAL).ok()?;
            let info = CMINVOKECOMMANDINFO {
                cbSize: std::mem::size_of::<CMINVOKECOMMANDINFO>() as u32,
                // our own dialog has asked already: the shell is not to ask again (CMIC_MASK_FLAG_NO_UI is SEE_MASK_FLAG_NO_UI).
                // How the bin's verb treats this flag is NOT known (skip its question, or refuse because it cannot ask): with
                // RUSTYFILES_BIN_DELETE_SHELL_UI=1 the flag is left out, so that the first live run can try both without a rebuild.
                fMask: if std::env::var_os("RUSTYFILES_BIN_DELETE_SHELL_UI").is_some_and(|v| v == "1") { 0 } else { SEE_MASK_FLAG_NO_UI },
                lpVerb: PCSTR(s!("delete").as_ptr()),
                nShow: SW_SHOWNORMAL.0,
                ..Default::default()
            };
            cm.InvokeCommand(&info)?;
            Ok(())
        })();
        let _ = DestroyMenu(menu);
        res
    }
}

/// [`rf_core::recycle_bin::BinBackend`] on the real bin. Both calls go through the shell thread ([`crate::sta::call`]).
pub struct ShellBin;

/// How long a listing or a deletion may keep the caller waiting.
const SHELL_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

impl rf_core::recycle_bin::BinBackend for ShellBin {
    fn list_keys(&self) -> anyhow::Result<Vec<String>> {
        crate::sta::call(SHELL_LIMIT, list_keys_only).unwrap_or_else(|| Err(anyhow::Error::new(rf_core::recycle_bin::TimedOut)))
    }
    fn delete_keys(&self, keys: &[String]) -> anyhow::Result<()> {
        let keys = keys.to_vec();
        // (on a timeout the job is still queued on the shell thread and may run later: the caller is told it is unknown)
        crate::sta::call(SHELL_LIMIT, move || delete_by_keys(&keys)).unwrap_or_else(|| Err(anyhow::Error::new(rf_core::recycle_bin::TimedOut)))
    }
}

/// Empties the bin on all drives. With `confirm == false` no confirmation dialog, progress UI or
/// sound is shown (the caller is expected to have asked already).
pub fn empty(confirm: bool) -> anyhow::Result<()> {
    ensure_com();
    let flags = if confirm { 0 } else { SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND };
    unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), flags)? };
    Ok(())
}

/// `(item_count, total_bytes)` across all drives.
pub fn info() -> anyhow::Result<(u64, u64)> {
    let mut i = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut i)? };
    Ok((i.i64NumItems.max(0) as u64, i.i64Size.max(0) as u64))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_works() {
        let (count, bytes) = info().unwrap();
        println!("recycle bin: {count} items, {bytes} bytes");
    }

    #[test]
    fn list_is_ok_and_consistent() {
        let items = list().unwrap();
        for i in &items {
            assert!(!i.parsing_path.is_empty() && !i.name.contains('\\'));
        }
        println!("{items:#?}");
    }

    /// The keys-only listing the deletion uses sees exactly the items the full listing sees (read-only; counts only).
    #[test]
    fn the_keys_only_listing_matches_the_full_one() {
        let mut full: Vec<String> = list().unwrap().iter().map(|i| rf_core::recycle_bin::key_of(&i.parsing_path)).collect();
        let mut keys = list_keys_only().unwrap();
        full.sort();
        keys.sort();
        assert_eq!(keys, full);
        println!("bin keys: {}", keys.len());
    }

    #[test]
    fn restore_empty_slice_is_noop() {
        restore(&[]).unwrap();
    }

    /// With no shell window the bin's verbs are refused before anything is looked up or invoked (the real bin is not touched).
    #[test]
    fn without_a_shell_restoring_and_deleting_in_the_bin_are_refused() {
        use crate::engine_guard::{NoShellEngine, Op, with_no_shell};
        let item = RecycleItem { name: "x".into(), original_location: String::new(), deleted_ms: 0, size: 0, parsing_path: "no-such-item".into(), is_dir: false };
        let e = with_no_shell(|| restore(&[item])).expect_err("refused");
        assert_eq!(e.downcast_ref::<NoShellEngine>().map(|n| n.op), Some(Op::RestoreFromBin));
        let e = with_no_shell(|| delete_by_keys(&["no-such-key".to_string()])).expect_err("refused");
        assert_eq!(e.downcast_ref::<NoShellEngine>().map(|n| n.op), Some(Op::DeleteFromBin));
    }
}
