//! Drag files out of rustyfiles (to Explorer, the desktop, other apps — or back into
//! rustyfiles itself) with the standard shell data object and drag image.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{CoTaskMemFree, IDataObject};
use windows::Win32::System::Ole::{DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE};
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{ILFindLastID, SHCreateDataObject, SHDoDragDrop, SHParseDisplayName};
use windows::core::PCWSTR;

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::fileops::Transfer;
use crate::wide::wide;

struct Pidl(*mut ITEMIDLIST);
impl Drop for Pidl {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { CoTaskMemFree(Some(self.0 as *const _)) }
        }
    }
}

fn parse(path: &Path) -> anyhow::Result<Pidl> {
    let w = wide(path);
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    unsafe { SHParseDisplayName(PCWSTR(w.as_ptr()), None, &mut pidl, 0, None)? };
    Ok(Pidl(pidl))
}

/// Shell data object for items that share one parent folder.
pub fn data_object(items: &[PathBuf]) -> anyhow::Result<IDataObject> {
    anyhow::ensure!(!items.is_empty(), "no items");
    let parent = items[0].parent().ok_or_else(|| anyhow::anyhow!("no parent"))?;
    let folder = parse(parent)?;
    // Same-folder items only (child pidls are relative to `folder`).
    let items: Vec<&PathBuf> = items.iter().filter(|p| p.parent() == Some(parent)).collect();
    let abs: Vec<Pidl> = items.iter().map(|p| parse(p)).collect::<anyhow::Result<_>>()?;
    let children: Vec<*const ITEMIDLIST> = abs.iter().map(|p| unsafe { ILFindLastID(p.0) as *const _ }).collect();
    Ok(unsafe { SHCreateDataObject(Some(folder.0), Some(&children), None)? })
}

/// Runs a modal OLE drag on the calling thread (must be the shell STA thread — see `sta::run`).
/// `ui_thread` is the id of the thread that owns the window where the drag started; its input
/// state is attached so this thread can take mouse capture while the button is still down.
pub fn drag_out(owner: HWND, ui_thread: u32, items: &[PathBuf]) -> anyhow::Result<DROPEFFECT> {
    drag_out_allowing(owner, ui_thread, items, DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK)
}

/// [`drag_out`] for files that must not be MOVED by the target: a copy or a link only (the Start panel's recommended files and the
/// copies of its apps' shortcuts). The target still decides between the two by its own rules.
pub fn drag_out_copy_only(owner: HWND, ui_thread: u32, items: &[PathBuf]) -> anyhow::Result<DROPEFFECT> {
    // The allowed-effects mask binds the OLE targets of other programs, but our own windows take a drop as a plain list of paths and ask
    // `default_transfer`, which moves on the same drive. So the paths are also remembered here, and `default_transfer` copies them.
    mark_copy_only(items, DRAG_LIMIT);
    let r = drag_out_allowing(owner, ui_thread, items, DROPEFFECT_COPY | DROPEFFECT_LINK);
    // (the drop is handled a moment after the loop ends: the paths stay marked for that, then they are let go)
    mark_copy_only(items, AFTER_DRAG);
    r
}

/// How long a copy-only drag may last before its paths stop being marked (a drag that never returns must not mark them for good).
const DRAG_LIMIT: Duration = Duration::from_secs(600);
/// How long the paths stay marked after the drag's loop has ended.
const AFTER_DRAG: Duration = Duration::from_secs(5);

/// Paths of drags that must never be moved, each with the time its mark ends.
static COPY_ONLY: Mutex<Vec<(String, Instant)>> = Mutex::new(Vec::new());

fn key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Marks `items` as the subject of a copy-only drag for `for_how_long` (replacing an earlier mark of the same path).
pub fn mark_copy_only(items: &[PathBuf], for_how_long: Duration) {
    let now = Instant::now();
    let Ok(mut marks) = COPY_ONLY.lock() else { return };
    marks.retain(|(_, until)| *until > now);
    for p in items {
        let k = key(p);
        marks.retain(|(m, _)| *m != k);
        marks.push((k, now + for_how_long));
    }
}

/// Whether every one of `sources` belongs to a copy-only drag that is going on or has just ended at `now`.
fn copy_only_at(sources: &[PathBuf], now: Instant) -> bool {
    let Ok(marks) = COPY_ONLY.lock() else { return false };
    !sources.is_empty() && sources.iter().all(|s| marks.iter().any(|(m, until)| *until > now && *m == key(s)))
}

fn drag_out_allowing(owner: HWND, ui_thread: u32, items: &[PathBuf], allowed: DROPEFFECT) -> anyhow::Result<DROPEFFECT> {
    let data = data_object(items)?;
    unsafe {
        // The button may already be up (very short drags) — don't start a drag then.
        let down = |vk: i32| (GetAsyncKeyState(vk) as u16 & 0x8000) != 0;
        if !down(VK_LBUTTON.0 as i32) && !down(VK_RBUTTON.0 as i32) {
            return Ok(DROPEFFECT(0));
        }
        let me = GetCurrentThreadId();
        let attached = ui_thread != 0 && AttachThreadInput(me, ui_thread, true).as_bool();
        let r = SHDoDragDrop(Some(owner), &data, None, allowed);
        if attached {
            let _ = AttachThreadInput(me, ui_thread, false);
        }
        Ok(r?)
    }
}

/// Explorer's default for a plain drag: same volume → move, otherwise copy.
/// Ctrl forces copy, Shift forces move.
pub fn default_transfer(sources: &[PathBuf], dest_dir: &Path, ctrl: bool, shift: bool) -> Transfer {
    // (a drag that was started copy-only, the Start panel's recommended files, is copied by our own windows as well: not even Shift moves it)
    if copy_only_at(sources, Instant::now()) {
        return Transfer::Copy;
    }
    if ctrl {
        return Transfer::Copy;
    }
    if shift {
        return Transfer::Move;
    }
    let vol = |p: &Path| p.components().next().map(|c| c.as_os_str().to_string_lossy().to_lowercase());
    match sources.first() {
        Some(s) if vol(s) == vol(dest_dir) => Transfer::Move,
        _ => Transfer::Copy,
    }
}

/// True when dropping `sources` into `dest_dir` makes no sense (onto itself or its own child).
pub fn is_invalid_drop(sources: &[PathBuf], dest_dir: &Path) -> bool {
    sources.iter().any(|s| dest_dir == s || dest_dir.starts_with(s)) || sources.iter().all(|s| s.parent() == Some(dest_dir))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transfer_rules() {
        let src = vec![PathBuf::from(r"C:\a\x.txt")];
        assert_eq!(default_transfer(&src, Path::new(r"C:\b"), false, false), Transfer::Move);
        assert_eq!(default_transfer(&src, Path::new(r"D:\b"), false, false), Transfer::Copy);
        assert_eq!(default_transfer(&src, Path::new(r"D:\b"), false, true), Transfer::Move);
        assert_eq!(default_transfer(&src, Path::new(r"C:\b"), true, false), Transfer::Copy);
        assert!(is_invalid_drop(&src, Path::new(r"C:\a")));
        assert!(is_invalid_drop(&[PathBuf::from(r"C:\a")], Path::new(r"C:\a\sub")));
        assert!(!is_invalid_drop(&src, Path::new(r"C:\b")));
    }


    #[test]
    fn a_file_dragged_copy_only_is_copied_by_our_own_windows_even_on_the_same_drive() {
        let mine = PathBuf::from("C:/rf-copy-only-test/mine.txt");
        let other = PathBuf::from("C:/rf-copy-only-test/other.txt");
        let one = |p: &PathBuf, shift: bool| default_transfer(std::slice::from_ref(p), Path::new("C:/rf-copy-only-dest"), false, shift);
        // (before the fix a same-volume drop was a Move whatever the drag allowed)
        assert_eq!(one(&mine, false), Transfer::Move);
        mark_copy_only(std::slice::from_ref(&mine), Duration::from_secs(60));
        assert_eq!(one(&mine, false), Transfer::Copy);
        assert_eq!(one(&mine, true), Transfer::Copy, "Shift does not move it");
        assert_eq!(default_transfer(&[PathBuf::from("c:/RF-COPY-ONLY-TEST/MINE.TXT")], Path::new("C:/rf-copy-only-dest"), false, false), Transfer::Copy, "case does not matter");
        // another file, or a list that is not all marked, keeps the ordinary rule
        assert_eq!(one(&other, false), Transfer::Move);
        assert_eq!(default_transfer(&[mine.clone(), other], Path::new("C:/rf-copy-only-dest"), false, false), Transfer::Move);
        // the mark ends
        assert!(copy_only_at(std::slice::from_ref(&mine), Instant::now()));
        assert!(!copy_only_at(std::slice::from_ref(&mine), Instant::now() + Duration::from_secs(61)));
        mark_copy_only(std::slice::from_ref(&mine), Duration::ZERO);
        assert_eq!(one(&mine, false), Transfer::Move);
    }

    #[test]
    fn builds_data_object() {
        let d = std::env::temp_dir().join(format!("rf-dnd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), b"x").unwrap();
        crate::shell::ensure_com();
        assert!(data_object(&[d.join("a.txt")]).is_ok());
        std::fs::remove_dir_all(&d).unwrap();
    }
}
