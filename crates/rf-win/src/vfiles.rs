//! Files that exist only inside another program's data object ("virtual files"), pasted as real files.
//!
//! WHY (issue 23, LokeYourC3PH, about Input Director, which carries the clipboard from one PC to the other: "if I simply copy a file
//! on Desktop 1, and press CTRL+V (or Paste) on Desktop 2 ... it is automatically being transferred via LAN ... This feature on Input
//! Director seems to so far only work when using Windows Explorer"). A program whose files are not files on THIS disk (a remote
//! desktop session, a tool that carries the clipboard between PCs, a mail program's attachments, an archive opened in Explorer)
//! cannot put paths on the clipboard (`CF_HDROP`), and paths were the only thing rustyfiles pasted. Such a program offers the
//! shell's two formats instead: "FileGroupDescriptorW", a list of names with sizes and times, and "FileContents", the bytes of the
//! i-th of them, usually as a stream that is read while the source fetches. Explorer pastes them; this module does the same.
//!
//! WHAT IS WRITTEN. Each item goes under the folder pasted into, at its relative path ("folder\sub\file.txt" makes the folders).
//! NOTHING IS EVER OVERWRITTEN: a top-level name that is already there gets " (2)" (or the next free number) and everything under
//! it follows; a file is made with "create new", so one that appeared meanwhile is left alone too. A name is the SOURCE's word and
//! is not trusted: a drive, a root, ".." or a character a file name cannot have never reach the disk ([`safe_rel`]). A file that
//! was not read to its end (the source failed, the user cancelled) is removed, not left half written. One item that cannot be
//! read does not stop the others: the result says how many failed and why the first did.
//!
//! THREAD. [`paste_clipboard`] wants a thread of its own: it starts OLE there, asks for the clipboard's data object and reads. The
//! reads run in the SOURCE program and take as long as its fetching takes.
//!
//! NOT READ: an item the source hands over only as a structured storage (`TYMED_ISTORAGE`: a mail program's message as a
//! compound file). It counts as an item that could not be read.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Context;
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, IStream, STGMEDIUM, STREAM_SEEK_SET, TYMED_HGLOBAL, TYMED_ISTREAM};
use windows::Win32::System::DataExchange::{IsClipboardFormatAvailable, RegisterClipboardFormatW};
use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
use windows::Win32::System::Ole::{CF_HDROP, OleGetClipboard, OleInitialize, OleUninitialize, ReleaseStgMedium};
use windows::core::w;

// ------------------------------------------------------------------ pure: the list, the names, the plan

/// One item of the source's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The relative path as the source names it (backslashes between folders).
    pub name: String,
    pub is_dir: bool,
    /// The size, when the source tells it.
    pub size: Option<u64>,
    /// The last write time (a FILETIME: 100 ns steps since 1601), when the source tells it.
    pub modified: Option<u64>,
}

/// The size of one `FILEDESCRIPTORW`: five 32-bit and 64-bit fields (72 bytes) and a name of `MAX_PATH` UTF-16 units.
const DESCRIPTOR: usize = 592;
const FD_ATTRIBUTES: u32 = 0x4;
const FD_WRITESTIME: u32 = 0x20;
const FD_FILESIZE: u32 = 0x40;
const ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// Reads a `FILEGROUPDESCRIPTORW` (a count, then that many descriptors). A count that says more than the bytes hold gives the items
/// that are there; the i-th item of the result is the i-th of the source, which is the index its contents are asked for with.
pub fn parse_group(bytes: &[u8]) -> Vec<Item> {
    let Some(count) = bytes.get(..4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { return Vec::new() };
    let mut out = Vec::new();
    for i in 0..count {
        let Some(d) = bytes.get(4 + i * DESCRIPTOR..4 + (i + 1) * DESCRIPTOR) else { break };
        let at = |o: usize| u32::from_le_bytes([d[o], d[o + 1], d[o + 2], d[o + 3]]);
        let flags = at(0);
        let attributes = if flags & FD_ATTRIBUTES != 0 { at(36) } else { 0 };
        let name: Vec<u16> = d[72..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|c| *c != 0).collect();
        out.push(Item {
            name: String::from_utf16_lossy(&name),
            is_dir: attributes & ATTRIBUTE_DIRECTORY != 0,
            size: (flags & FD_FILESIZE != 0).then(|| (u64::from(at(64)) << 32) | u64::from(at(68))),
            modified: (flags & FD_WRITESTIME != 0).then(|| (u64::from(at(60)) << 32) | u64::from(at(56))).filter(|t| *t != 0),
        });
    }
    out
}

/// Names Windows keeps for devices: a file of such a name is the device, whatever its extension.
fn is_device_name(stem: &str) -> bool {
    let s = stem.trim_end().to_ascii_uppercase();
    matches!(s.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (s.len() == 4 && (s.starts_with("COM") || s.starts_with("LPT")) && s.as_bytes()[3].is_ascii_digit() && s.as_bytes()[3] != b'0')
}

/// An item's name as a path under the folder pasted into: its parts, each a name a file can have. `None` for a name that tries to
/// climb out ("..") and for one that has nothing usable in it.
pub fn safe_rel(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['\\', '/']) {
        if part.trim() == ".." {
            return None;
        }
        // (a drive's colon and the other characters a name cannot have become "_"; Windows drops dots and spaces at the end itself)
        let clean: String = part.chars().map(|c| if c < ' ' || "<>:\"|?*".contains(c) { '_' } else { c }).collect();
        let clean = clean.trim_end_matches([' ', '.']);
        if clean.is_empty() {
            continue;
        }
        if is_device_name(clean.split('.').next().unwrap_or("")) {
            out.push(format!("_{clean}"));
        } else {
            out.push(clean);
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// One item with the place it goes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    /// The item's place in the source's list: the index its contents are asked for with.
    pub index: usize,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
    pub modified: Option<u64>,
}

fn free_name(name: &str, folder: bool, dest: &Path, taken: &dyn Fn(&Path) -> bool, used: &HashSet<String>) -> String {
    let free = |n: &str| !used.contains(&n.to_lowercase()) && !taken(&dest.join(n));
    if free(name) {
        return name.to_string();
    }
    (2u32..10_000)
        // (a folder's name has no extension to keep: "v1.2" becomes "v1.2 (2)")
        .map(|i| if folder { format!("{name} ({i})") } else { rf_core::organize::plan::numbered_name(name, i) })
        .find(|n| free(n))
        .unwrap_or_else(|| name.to_string())
}

/// Where every item goes under `dest`, and the top-level paths that will be new there. `taken` says whether a path is in use on
/// the disk. A folder of the source keeps what is under it together (one name for all its items); two FILES of the same name in
/// one list (a mail's two attachments) are two files.
pub fn plan(items: &[Item], dest: &Path, taken: &dyn Fn(&Path) -> bool) -> (Vec<Planned>, Vec<PathBuf>) {
    // the source's top folder (lower case) -> the name it gets; and every top-level name given out in this paste (lower case)
    let mut folders: HashMap<String, String> = HashMap::new();
    let mut used: HashSet<String> = HashSet::new();
    let (mut out, mut tops) = (Vec::new(), Vec::new());
    for (index, item) in items.iter().enumerate() {
        let Some(rel) = safe_rel(&item.name) else { continue };
        let mut parts: Vec<String> = rel.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        let first = parts[0].clone();
        let folder = item.is_dir || parts.len() > 1;
        let known = if folder { folders.get(&first.to_lowercase()).cloned() } else { None };
        parts[0] = known.unwrap_or_else(|| {
            let name = free_name(&first, folder, dest, taken, &used);
            if folder {
                folders.insert(first.to_lowercase(), name.clone());
            }
            used.insert(name.to_lowercase());
            tops.push(dest.join(&name));
            name
        });
        out.push(Planned { index, path: parts.iter().fold(dest.to_path_buf(), |p, s| p.join(s)), is_dir: item.is_dir, size: item.size, modified: item.modified });
    }
    (out, tops)
}

/// A FILETIME as a time of the standard library; `None` for a time before 1970.
fn system_time(filetime: u64) -> Option<std::time::SystemTime> {
    const TO_1970: u64 = 116_444_736_000_000_000;
    let since = filetime.checked_sub(TO_1970)?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::new(since / 10_000_000, (since % 10_000_000) as u32 * 100))
}

// ------------------------------------------------------------------ reading the data object

/// How far a paste is.
pub struct Progress<'a> {
    /// Bytes written, and the sum of the sizes the source told (0: it told none).
    pub bytes: (u64, u64),
    /// Files finished, and how many there are.
    pub files: (usize, usize),
    /// The file that is being written, when it has just begun.
    pub current: Option<&'a str>,
}

/// What a paste did.
#[derive(Debug, Default)]
pub struct Pasted {
    /// The top-level paths that are new in the folder.
    pub arrived: Vec<PathBuf>,
    /// How many items could not be read from the source, and why the first of them could not.
    pub failed: usize,
    pub first_error: Option<String>,
    pub cancelled: bool,
}

/// The two clipboard formats: the list and the contents.
fn formats() -> (u16, u16) {
    unsafe { (RegisterClipboardFormatW(w!("FileGroupDescriptorW")) as u16, RegisterClipboardFormatW(w!("FileContents")) as u16) }
}

/// Does the clipboard hold virtual files and no ordinary ones? (Files with paths are pasted as files: `clipboard::get_files`.)
pub fn clipboard_has() -> bool {
    let (group, contents) = formats();
    unsafe {
        IsClipboardFormatAvailable(u32::from(group)).is_ok() && IsClipboardFormatAvailable(u32::from(contents)).is_ok() && IsClipboardFormatAvailable(u32::from(CF_HDROP.0)).is_err()
    }
}

/// A medium that is given back when it goes out of scope.
struct Medium(STGMEDIUM);

impl Drop for Medium {
    fn drop(&mut self) {
        unsafe { ReleaseStgMedium(&mut self.0) }
    }
}

fn ask(data: &IDataObject, format: u16, index: i32, tymed: u32) -> windows::core::Result<Medium> {
    let f = FORMATETC { cfFormat: format, ptd: std::ptr::null_mut(), dwAspect: DVASPECT_CONTENT.0, lindex: index, tymed };
    unsafe { data.GetData(&f) }.map(Medium)
}

/// The bytes of a global block, at most `limit` of them (a block is as long as it was allocated, which can be more than was put
/// into it).
unsafe fn global_bytes(h: HGLOBAL, limit: Option<u64>) -> Vec<u8> {
    unsafe {
        let len = GlobalSize(h);
        let len = limit.map_or(len, |l| len.min(usize::try_from(l).unwrap_or(usize::MAX)));
        let p = GlobalLock(h) as *const u8;
        if p.is_null() {
            return Vec::new();
        }
        let v = std::slice::from_raw_parts(p, len).to_vec();
        let _ = GlobalUnlock(h);
        v
    }
}

/// Pours the contents of item `index` into `file`. `Ok(false)`: cancelled part way.
fn pour(data: &IDataObject, contents: u16, p: &Planned, file: &mut std::fs::File, wrote: &mut dyn FnMut(u64), cancel: &AtomicBool) -> anyhow::Result<bool> {
    let medium = ask(data, contents, p.index as i32, (TYMED_ISTREAM.0 | TYMED_HGLOBAL.0) as u32).context("the source did not hand the file over")?;
    if medium.0.tymed == TYMED_ISTREAM.0 as u32 {
        // SAFETY: the medium says its union holds a stream
        let stream: IStream = unsafe { (*medium.0.u.pstm).clone() }.context("the source handed over no stream")?;
        // (some sources hand the stream over at its END, having just written it)
        unsafe {
            let _ = stream.Seek(0, STREAM_SEEK_SET, None);
        }
        let mut buf = vec![0u8; 1 << 20];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Ok(false);
            }
            let mut got = 0u32;
            unsafe { stream.Read(buf.as_mut_ptr() as *mut c_void, buf.len() as u32, Some(&mut got)) }.ok().context("reading from the source")?;
            if got == 0 {
                return Ok(true);
            }
            file.write_all(&buf[..got as usize])?;
            wrote(u64::from(got));
        }
    }
    if medium.0.tymed == TYMED_HGLOBAL.0 as u32 {
        // SAFETY: the medium says its union holds a global block
        let bytes = unsafe { global_bytes(medium.0.u.hGlobal, p.size) };
        file.write_all(&bytes)?;
        wrote(bytes.len() as u64);
        return Ok(true);
    }
    anyhow::bail!("the source hands the file over in a form that is not read here (medium {})", medium.0.tymed)
}

/// Writes the data object's virtual files under `dest`. See the module docs for what is written and what never is.
pub fn write_all(data: &IDataObject, dest: &Path, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> anyhow::Result<Pasted> {
    let (group, contents) = formats();
    let list = ask(data, group, -1, TYMED_HGLOBAL.0 as u32).context("the source's list of files")?;
    anyhow::ensure!(list.0.tymed == TYMED_HGLOBAL.0 as u32, "the source's list of files came in a form that is not read here");
    // SAFETY: the medium says its union holds a global block
    let items = parse_group(&unsafe { global_bytes(list.0.u.hGlobal, None) });
    drop(list);
    let (planned, arrived) = plan(&items, dest, &|p| p.symlink_metadata().is_ok());
    anyhow::ensure!(!planned.is_empty(), "the source lists no file that can be written");
    let total: u64 = planned.iter().filter(|p| !p.is_dir).filter_map(|p| p.size).sum();
    let files = planned.iter().filter(|p| !p.is_dir).count();
    let mut out = Pasted { arrived, ..Default::default() };
    let (mut done, mut finished) = (0u64, 0usize);
    let fail = |out: &mut Pasted, p: &Planned, why: String| {
        out.failed += 1;
        out.first_error.get_or_insert_with(|| format!("{}: {why}", p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()));
    };
    for p in &planned {
        if cancel.load(Ordering::Relaxed) {
            out.cancelled = true;
            break;
        }
        if p.is_dir {
            if let Err(e) = std::fs::create_dir_all(&p.path) {
                fail(&mut out, p, e.to_string());
            }
            continue;
        }
        let name = p.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        progress(Progress { bytes: (done, total), files: (finished, files), current: Some(&name) });
        // "create new": a file that is there (it appeared since the plan, or the source lists one path twice) is not touched
        let made = p.path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::OpenOptions::new().write(true).create_new(true).open(&p.path));
        let mut file = match made {
            Ok(f) => f,
            Err(e) => {
                fail(&mut out, p, e.to_string());
                continue;
            }
        };
        let before = done;
        let poured = pour(data, contents, p, &mut file, &mut |n| {
            done += n;
            progress(Progress { bytes: (done, total), files: (finished, files), current: None });
        }, cancel);
        match poured {
            Ok(true) => {
                if let Some(t) = p.modified.and_then(system_time) {
                    let _ = file.set_modified(t);
                }
                finished += 1;
            }
            // not read to its end: it is not left half written
            Ok(false) | Err(_) => {
                drop(file);
                let _ = std::fs::remove_file(&p.path);
                done = before;
                match poured {
                    Err(e) => fail(&mut out, p, format!("{e:#}")),
                    _ => {
                        out.cancelled = true;
                        break;
                    }
                }
            }
        }
    }
    progress(Progress { bytes: (done, total), files: (finished, files), current: None });
    // (what never came to be is not reported as having arrived)
    out.arrived.retain(|p| p.symlink_metadata().is_ok());
    Ok(out)
}

/// Pastes the clipboard's virtual files under `dest`. On a thread of the caller's that is an STA or has no COM yet (see the module
/// docs); it blocks while the source fetches.
pub fn paste_clipboard(dest: &Path, progress: &mut dyn FnMut(Progress), cancel: &AtomicBool) -> anyhow::Result<Pasted> {
    /// OLE on this thread for as long as the data object is used (declared before it, so it goes after it).
    struct Ole(bool);
    impl Drop for Ole {
        fn drop(&mut self) {
            if self.0 {
                unsafe { OleUninitialize() }
            }
        }
    }
    let _ole = Ole(unsafe { OleInitialize(None) }.is_ok());
    let data = unsafe { OleGetClipboard() }.context("the clipboard's data object")?;
    write_all(&data, dest, progress, cancel)
}

// ------------------------------------------------------------------ drops

// WHY (issue 128, a browser's image dragged to a folder; and on Discord: "When I attempt to drag from Outlook attachments into the
// folder, it doesn't work"). Such a drag carries the same two formats as such a paste and no paths, and the window's drop target
// took paths only. The drop target asks [`drag_paths`] instead (registered by the app with the GPUI fork's
// `dnd_hint::set_virtual_reader`): the files are written into a folder of their own under %TEMP%\rustyfiles-drop at the drop, and
// the drop goes on as a drop of those files, so every place that takes files (a folder, a tab, the sidebar, the dock) takes these.
// The source's data object is only valid during the drop, so they are written then, before the drop handlers run.

/// The folder the drag that is over a window now will be written into: named at its first ask, made at its drop.
static DROP_DIR: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Where dropped virtual files are written, a folder per drop.
pub fn drops_root() -> PathBuf {
    std::env::temp_dir().join("rustyfiles-drop")
}

/// A drop's folder that has been there this long is removed at a later drop (a copy out of it has long finished).
const DROP_KEEP: std::time::Duration = std::time::Duration::from_secs(24 * 3600);

fn sweep_old_drops(root: &Path, keep: std::time::Duration) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > keep);
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn new_drop_dir(root: &Path) -> PathBuf {
    use std::sync::atomic::AtomicU32;
    static N: AtomicU32 = AtomicU32::new(0);
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    root.join(format!("{}-{stamp}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

/// The source's list of virtual files, when it offers one.
fn listed(data: &IDataObject) -> Option<Vec<Item>> {
    let (group, _) = formats();
    let list = ask(data, group, -1, TYMED_HGLOBAL.0 as u32).ok()?;
    (list.0.tymed == TYMED_HGLOBAL.0 as u32).then(|| parse_group(&unsafe { global_bytes(list.0.u.hGlobal, None) }))
}

/// The paths of a drag of virtual files under `root`: with `fetch` false the paths its files will have (nothing is written), with
/// `fetch` true the files are written and their real paths given. `None`: the drag lists no virtual file, or none could be written.
pub fn drag_paths_under(data: &IDataObject, fetch: bool, root: &Path) -> Option<Vec<PathBuf>> {
    let items = listed(data)?;
    let mut slot = DROP_DIR.lock().unwrap_or_else(|e| e.into_inner());
    if !fetch {
        let dir = slot.get_or_insert_with(|| new_drop_dir(root)).clone();
        let (_, tops) = plan(&items, &dir, &|_| false);
        return (!tops.is_empty()).then_some(tops);
    }
    let dir = slot.take().unwrap_or_else(|| new_drop_dir(root));
    drop(slot);
    sweep_old_drops(root, DROP_KEEP);
    std::fs::create_dir_all(&dir).ok()?;
    let pasted = write_all(data, &dir, &mut |_| {}, &AtomicBool::new(false));
    match pasted {
        Ok(p) => {
            if p.failed > 0 {
                log::warn!("a drop of virtual files: {} could not be read ({})", p.failed, p.first_error.unwrap_or_default());
            }
            if p.arrived.is_empty() {
                let _ = std::fs::remove_dir(&dir);
                return None;
            }
            Some(p.arrived)
        }
        Err(e) => {
            log::warn!("a drop of virtual files: {e:#}");
            let _ = std::fs::remove_dir(&dir);
            None
        }
    }
}

/// [`drag_paths_under`] the drops' folder, for the GPUI fork's drop target: `data` is the drag's `IDataObject`, borrowed.
pub fn drag_paths(data: *mut c_void, fetch: bool) -> Option<Vec<PathBuf>> {
    use windows::core::Interface;
    // SAFETY: the drop target hands over its data object for the length of the call
    let data = unsafe { IDataObject::from_raw_borrowed(&data) }?;
    drag_paths_under(data, fetch, &drops_root())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::ManuallyDrop;
    use windows::Win32::Foundation::{DV_E_FORMATETC, DV_E_LINDEX, E_NOTIMPL, E_OUTOFMEMORY, OLE_E_ADVISENOTSUPPORTED, S_OK};
    use windows::Win32::System::Com::{IAdviseSink, IDataObject_Impl, IEnumFORMATETC, IEnumSTATDATA, STGMEDIUM_0, STREAM_SEEK_END};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc};
    use windows::Win32::UI::Shell::SHCreateMemStream;
    use windows::core::{BOOL, HRESULT, Ref, implement};

    /// (name, size told, a folder, time told)
    type Told<'a> = (&'a str, Option<u64>, bool, Option<u64>);

    /// A `FILEGROUPDESCRIPTORW` as a source would build it.
    fn group(items: &[Told]) -> Vec<u8> {
        let mut out = (items.len() as u32).to_le_bytes().to_vec();
        for (name, size, dir, time) in items {
            let mut d = vec![0u8; DESCRIPTOR];
            let mut flags = 0u32;
            if *dir {
                flags |= FD_ATTRIBUTES;
                d[36..40].copy_from_slice(&ATTRIBUTE_DIRECTORY.to_le_bytes());
            }
            if let Some(s) = size {
                flags |= FD_FILESIZE;
                d[64..68].copy_from_slice(&((*s >> 32) as u32).to_le_bytes());
                d[68..72].copy_from_slice(&(*s as u32).to_le_bytes());
            }
            if let Some(t) = time {
                flags |= FD_WRITESTIME;
                d[56..60].copy_from_slice(&(*t as u32).to_le_bytes());
                d[60..64].copy_from_slice(&((*t >> 32) as u32).to_le_bytes());
            }
            d[0..4].copy_from_slice(&flags.to_le_bytes());
            for (i, u) in name.encode_utf16().take(259).enumerate() {
                d[72 + 2 * i..74 + 2 * i].copy_from_slice(&u.to_le_bytes());
            }
            out.extend(d);
        }
        out
    }

    #[test]
    fn the_sources_list_is_read_item_by_item() {
        let bytes = group(&[("rapor.pdf", Some(5_000_000_000), false, Some(133_000_000_000_000_000)), ("Klasör", None, true, None), (r"Klasör\not.txt", Some(0), false, None), ("adsız", None, false, None)]);
        assert_eq!(bytes.len(), 4 + 4 * DESCRIPTOR);
        let items = parse_group(&bytes);
        assert_eq!(items.len(), 4);
        assert_eq!(items[0], Item { name: "rapor.pdf".into(), is_dir: false, size: Some(5_000_000_000), modified: Some(133_000_000_000_000_000) });
        assert_eq!(items[1], Item { name: "Klasör".into(), is_dir: true, size: None, modified: None });
        assert_eq!(items[2], Item { name: r"Klasör\not.txt".into(), is_dir: false, size: Some(0), modified: None });
        assert_eq!(items[3].size, None, "a size that was not told is not zero");
        // a count that says more than there is: what is there; no bytes, a short block, a count alone: nothing
        let mut lying = bytes.clone();
        lying[0..4].copy_from_slice(&1000u32.to_le_bytes());
        assert_eq!(parse_group(&lying).len(), 4);
        assert_eq!(parse_group(&bytes[..4 + DESCRIPTOR + 10]).len(), 1);
        assert!(parse_group(&[]).is_empty());
        assert!(parse_group(&[1, 0]).is_empty());
        assert!(parse_group(&u32::MAX.to_le_bytes()).is_empty());
    }

    #[test]
    fn a_name_from_the_source_never_leaves_the_folder_and_is_a_name_a_file_can_have() {
        let rel = |s: &str| safe_rel(s).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(rel("a.txt").as_deref(), Some("a.txt"));
        assert_eq!(rel(r"klasör\alt\a.txt").as_deref(), Some(r"klasör\alt\a.txt"));
        assert_eq!(rel("klasör/alt/a.txt").as_deref(), Some(r"klasör\alt\a.txt"));
        // climbing out: not written at all
        assert_eq!(rel(r"..\evil.txt"), None);
        assert_eq!(rel(r"a\..\..\evil.txt"), None);
        assert_eq!(rel(" .. /x"), None);
        // a drive, a root, a share: what is left is under the folder
        assert_eq!(rel(r"C:\Windows\x.dll").as_deref(), Some(r"C_\Windows\x.dll"));
        assert_eq!(rel(r"\\server\share\x").as_deref(), Some(r"server\share\x"));
        assert_eq!(rel(r"\x").as_deref(), Some("x"));
        // characters a name cannot have, an alternate stream, dots and spaces at the end
        assert_eq!(rel("a<b>c|d?e*f\"g.txt").as_deref(), Some("a_b_c_d_e_f_g.txt"));
        assert_eq!(rel("x.txt:gizli").as_deref(), Some("x.txt_gizli"));
        assert_eq!(rel("ad. . ").as_deref(), Some("ad"));
        assert_eq!(rel("a\tb").as_deref(), Some("a_b"));
        // a device's name is no file
        assert_eq!(rel("NUL").as_deref(), Some("_NUL"));
        assert_eq!(rel("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(rel(r"x\COM1").as_deref(), Some(r"x\_COM1"));
        assert_eq!(rel("COM0").as_deref(), Some("COM0"));
        assert_eq!(rel("console.txt").as_deref(), Some("console.txt"));
        // nothing usable
        for none in ["", ".", r"\", "...", "  ", r".\."] {
            assert_eq!(rel(none), None, "{none:?}");
        }
    }

    #[test]
    fn nothing_that_is_there_is_written_over_and_a_folder_stays_together() {
        let item = |name: &str, dir: bool| Item { name: name.into(), is_dir: dir, size: None, modified: None };
        let dest = Path::new(r"D:\hedef");
        let on_disk = |p: &Path| [r"D:\hedef\a.txt", r"D:\hedef\a (2).txt", r"D:\hedef\Proje", r"D:\hedef\v1.2"].iter().any(|t| p == Path::new(t));
        let items = [
            item("a.txt", false),
            item("b.txt", false),
            item("Proje", true),
            item(r"Proje\src\main.rs", false),
            item(r"proje\README.md", false),
            item("a.txt", false),
            item(r"..\dışarı.txt", false),
            item(r"v1.2\x", false),
            item("B.TXT", false),
        ];
        let (planned, tops) = plan(&items, dest, &on_disk);
        let at = |i: usize| planned.iter().find(|p| p.index == i).map(|p| p.path.to_string_lossy().into_owned());
        assert_eq!(at(0).as_deref(), Some(r"D:\hedef\a (3).txt"), "two are taken on the disk");
        assert_eq!(at(1).as_deref(), Some(r"D:\hedef\b.txt"));
        assert_eq!(at(2).as_deref(), Some(r"D:\hedef\Proje (2)"), "the folder that is there is not merged into");
        assert_eq!(at(3).as_deref(), Some(r"D:\hedef\Proje (2)\src\main.rs"));
        assert_eq!(at(4).as_deref(), Some(r"D:\hedef\Proje (2)\README.md"), "the same folder, whatever the case");
        assert_eq!(at(5).as_deref(), Some(r"D:\hedef\a (4).txt"), "a second file of the same name in one list is a second file");
        assert_eq!(at(6), None, "the one that tries to climb out is not planned");
        assert_eq!(at(7).as_deref(), Some(r"D:\hedef\v1.2 (2)\x"), "a folder's dot is not an extension");
        assert_eq!(at(8).as_deref(), Some(r"D:\hedef\B (2).TXT"), "taken by this very paste");
        assert!(planned[2].is_dir && !planned[3].is_dir);
        let tops: Vec<String> = tops.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(tops, ["a (3).txt", "b.txt", "Proje (2)", "a (4).txt", "v1.2 (2)", "B (2).TXT"]);
    }

    #[test]
    fn a_filetime_is_a_time() {
        assert_eq!(system_time(116_444_736_000_000_000), Some(std::time::UNIX_EPOCH));
        assert_eq!(system_time(116_444_736_000_000_000 + 15_000_000), Some(std::time::UNIX_EPOCH + std::time::Duration::from_millis(1500)));
        assert_eq!(system_time(5), None, "before 1970: no time is set");
    }

    // ---------------------------------------------------------------- a source of our own (never the clipboard: it is the user's)

    #[implement(IDataObject)]
    struct Source {
        formats: (u16, u16),
        group: Vec<u8>,
        /// The contents by index; `None`: the source fails for that one.
        files: Vec<Option<Vec<u8>>>,
        /// Hand files over as streams (else as global blocks).
        streams: bool,
    }

    fn global_medium(bytes: &[u8]) -> windows::core::Result<STGMEDIUM> {
        unsafe {
            // (a block is never shorter than one byte, as a real source's is never exactly as long as the file)
            let h = GlobalAlloc(GMEM_MOVEABLE, bytes.len() + 16)?;
            let p = GlobalLock(h) as *mut u8;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
            std::ptr::write_bytes(p.add(bytes.len()), 0xEE, 16);
            let _ = GlobalUnlock(h);
            Ok(STGMEDIUM { tymed: TYMED_HGLOBAL.0 as u32, u: STGMEDIUM_0 { hGlobal: h }, pUnkForRelease: ManuallyDrop::new(None) })
        }
    }

    #[allow(non_snake_case)]
    impl IDataObject_Impl for Source_Impl {
        fn GetData(&self, f: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
            let f = unsafe { &*f };
            if f.cfFormat == self.formats.0 {
                return global_medium(&self.group);
            }
            if f.cfFormat != self.formats.1 {
                return Err(DV_E_FORMATETC.into());
            }
            let bytes = self.files.get(f.lindex as usize).and_then(|b| b.as_ref()).ok_or(windows::core::Error::from(DV_E_LINDEX))?;
            if self.streams && f.tymed & TYMED_ISTREAM.0 as u32 != 0 {
                let s = unsafe { SHCreateMemStream(Some(bytes)) }.ok_or(windows::core::Error::from(E_OUTOFMEMORY))?;
                // handed over at its END, as some sources do: the reader goes back to the start
                unsafe { s.Seek(0, STREAM_SEEK_END, None)? };
                return Ok(STGMEDIUM { tymed: TYMED_ISTREAM.0 as u32, u: STGMEDIUM_0 { pstm: ManuallyDrop::new(Some(s)) }, pUnkForRelease: ManuallyDrop::new(None) });
            }
            global_medium(bytes)
        }
        fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn QueryGetData(&self, f: *const FORMATETC) -> HRESULT {
            let f = unsafe { &*f };
            if f.cfFormat == self.formats.0 || f.cfFormat == self.formats.1 { S_OK } else { DV_E_FORMATETC }
        }
        fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
            E_NOTIMPL
        }
        fn SetData(&self, _: *const FORMATETC, _: *const STGMEDIUM, _: BOOL) -> windows::core::Result<()> {
            Err(E_NOTIMPL.into())
        }
        fn EnumFormatEtc(&self, _: u32) -> windows::core::Result<IEnumFORMATETC> {
            Err(E_NOTIMPL.into())
        }
        fn DAdvise(&self, _: *const FORMATETC, _: u32, _: Ref<IAdviseSink>) -> windows::core::Result<u32> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
        fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
        fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
            Err(OLE_E_ADVISENOTSUPPORTED.into())
        }
    }

    fn source(told: &[Told], files: Vec<Option<Vec<u8>>>, streams: bool) -> IDataObject {
        Source { formats: formats(), group: group(told), files, streams }.into()
    }

    /// A folder of the test's own under the temp folder, emptied first.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rf-vfiles-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn big() -> Vec<u8> {
        // (longer than the reading buffer, so that a stream is read in more than one piece)
        (0..2_500_000u32).map(|i| (i % 251) as u8).collect()
    }

    fn paste_into(dir: &Path, streams: bool) {
        std::fs::write(dir.join("not.txt"), b"eski").unwrap();
        let when = 116_444_736_000_000_000 + 1_700_000_000 * 10_000_000;
        let told: [Told; 6] = [
            ("not.txt", Some(5), false, Some(when)),
            ("Proje", None, true, None),
            (r"Proje\src\büyük.bin", Some(2_500_000), false, None),
            ("boş.txt", Some(0), false, None),
            (r"..\dışarı.txt", Some(3), false, None),
            ("bozuk.dat", Some(9), false, None),
        ];
        let files = vec![Some(b"yeni!".to_vec()), None, Some(big()), Some(Vec::new()), Some(b"xxx".to_vec()), None];
        let data = source(&told, files, streams);
        let mut last = (0u64, 0u64, 0usize, 0usize);
        let mut names = Vec::new();
        let got = write_all(
            &data,
            dir,
            &mut |p| {
                assert!(p.bytes.0 >= last.0, "the bytes written never go back while files arrive");
                last = (p.bytes.0, p.bytes.1, p.files.0, p.files.1);
                names.extend(p.current.map(str::to_string));
            },
            &AtomicBool::new(false),
        )
        .unwrap();
        // the file that was there is as it was; the new one stands beside it, with the time the source told
        assert_eq!(std::fs::read(dir.join("not.txt")).unwrap(), b"eski");
        assert_eq!(std::fs::read(dir.join("not (2).txt")).unwrap(), b"yeni!");
        let modified = std::fs::metadata(dir.join("not (2).txt")).unwrap().modified().unwrap();
        assert_eq!(modified, std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000));
        assert_eq!(std::fs::read(dir.join(r"Proje\src\büyük.bin")).unwrap(), big());
        assert_eq!(std::fs::read(dir.join("boş.txt")).unwrap(), b"", "an empty file is a file (and a block's spare bytes are not its contents)");
        // the one the source failed for is counted and not left behind; the one that climbs is nowhere
        assert_eq!(got.failed, 1);
        assert!(got.first_error.as_deref().is_some_and(|e| e.starts_with("bozuk.dat")), "{:?}", got.first_error);
        assert!(!dir.join("bozuk.dat").exists());
        assert!(!dir.parent().unwrap().join("dışarı.txt").exists());
        assert!(!got.cancelled);
        let arrived: Vec<String> = got.arrived.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(arrived, ["not (2).txt", "Proje", "boş.txt"]);
        // progress: the sizes the source told of what was planned (the failed one's too, not the climber's), three files of four
        assert_eq!(last, (5 + 2_500_000, 5 + 2_500_000 + 9, 3, 4));
        assert_eq!(names, ["not (2).txt", "büyük.bin", "boş.txt", "bozuk.dat"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_sources_streams_become_files() {
        paste_into(&scratch("streams"), true);
    }

    #[test]
    fn a_sources_global_blocks_become_files() {
        paste_into(&scratch("blocks"), false);
    }

    #[test]
    fn a_cancelled_paste_leaves_no_half_file() {
        let dir = scratch("cancel");
        let told: [Told; 2] = [("bir.bin", Some(2_500_000), false, None), ("iki.bin", Some(2_500_000), false, None)];
        let data = source(&told, vec![Some(big()), Some(big())], true);
        let cancel = AtomicBool::new(false);
        // the user cancels while the first file is arriving
        let got = write_all(&data, &dir, &mut |p| if p.bytes.0 > 0 { cancel.store(true, Ordering::Relaxed) }, &cancel).unwrap();
        assert!(got.cancelled);
        assert_eq!(got.failed, 0);
        assert!(got.arrived.is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "neither the half file nor the one that never began");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_source_without_a_usable_list_is_an_error_and_writes_nothing() {
        let dir = scratch("empty");
        let told: [Told; 1] = [(r"..\x", Some(1), false, None)];
        assert!(write_all(&source(&told, vec![Some(vec![1])], true), &dir, &mut |_| {}, &AtomicBool::new(false)).is_err());
        assert!(write_all(&source(&[], Vec::new(), true), &dir, &mut |_| {}, &AtomicBool::new(false)).is_err());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_drag_of_virtual_files_names_them_while_over_and_writes_them_at_the_drop() {
        let root = scratch("drag");
        // a drop's folder from long ago is swept at this drop; a recent one stays
        let old = root.join("old");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("x"), b"x").unwrap();
        {
            use std::os::windows::fs::OpenOptionsExt;
            // (a folder opens only with FILE_FLAG_BACKUP_SEMANTICS)
            let f = std::fs::OpenOptions::new().write(true).custom_flags(0x0200_0000).open(&old).unwrap();
            f.set_modified(std::time::SystemTime::now() - DROP_KEEP * 2).unwrap();
        }
        let recent = root.join("recent");
        std::fs::create_dir_all(&recent).unwrap();
        let told: [Told; 3] = [("Teklif.pdf", Some(4), false, None), ("Teklif.pdf", Some(3), false, None), ("resim.png", Some(2), false, None)];
        let data = source(&told, vec![Some(b"%PDF".to_vec()), Some(b"two".to_vec()), Some(b"PN".to_vec())], true);
        let over = drag_paths_under(&data, false, &root).unwrap();
        let names: Vec<_> = over.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        // a mail's two attachments of one name are two files
        assert_eq!(names, ["Teklif.pdf", "Teklif (2).pdf", "resim.png"]);
        assert!(over.iter().all(|p| !p.exists()), "nothing is written while the drag is only over the window");
        // asked again while over (DragEnter, then the drop's own ask): the same folder
        assert_eq!(drag_paths_under(&data, false, &root).unwrap(), over);
        let dropped = drag_paths_under(&data, true, &root).unwrap();
        assert_eq!(dropped, over, "the drop's files are where the drag said they would be");
        assert_eq!(std::fs::read(&dropped[0]).unwrap(), b"%PDF");
        assert_eq!(std::fs::read(&dropped[1]).unwrap(), b"two");
        assert!(!old.exists() && recent.exists());
        // the next drag gets a folder of its own
        let next = drag_paths_under(&data, false, &root).unwrap();
        assert_ne!(next[0].parent(), dropped[0].parent());
        // a drag that lists nothing is not taken
        assert!(drag_paths_under(&source(&[], Vec::new(), true), true, &root).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
