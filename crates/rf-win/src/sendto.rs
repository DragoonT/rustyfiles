//! "Send to" (issue 117): the items of the user's SendTo folder (`shell:sendto`) for a menu of our own, and sending files to one of
//! them or to a drive. A send is a drop of the files on that item, which is all Explorer's own Send to menu does: a shortcut runs its
//! program with the files, "Compressed (zipped) folder" makes a zip, "Desktop (create shortcut)" makes shortcuts, a drive copies.

use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{HWND, POINTL};
use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_NONE, IDropTarget};
use windows::Win32::System::SystemServices::MK_LBUTTON;
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_SendTo, IShellFolder, IShellItem, KF_FLAG_DEFAULT, SHBindToParent, SHCreateItemFromParsingName, SHGetKnownFolderPath,
    SIGDN_NORMALDISPLAY,
};
use windows::core::PCWSTR;

use crate::wide::wide;

/// One entry of the SendTo folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    /// The shell's name for it ("Documents" for `Documents.mydocs`, a shortcut without `.lnk`).
    pub name: String,
}

/// The user's SendTo folder.
pub fn folder() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_SendTo, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// The SendTo folder's entries sorted by name, as Explorer lists them; hidden and system files (its `desktop.ini`) left out. Call on a
/// thread with COM (the names come from the shell): the shell STA thread.
pub fn targets() -> Vec<Target> {
    let Some(dir) = folder() else { return Vec::new() };
    let Ok(read) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Target> = read
        .flatten()
        .filter_map(|e| {
            let attrs = e.metadata().ok()?.file_attributes();
            if attrs & (FILE_ATTRIBUTE_HIDDEN.0 | FILE_ATTRIBUTE_SYSTEM.0) != 0 {
                return None;
            }
            let path = e.path();
            let name = display_name(&path).unwrap_or_else(|| fallback_name(&path));
            Some(Target { path, name })
        })
        .collect();
    out.sort_by_key(|t| t.name.to_lowercase());
    out
}

/// A name without the shell: the file name without its extension (`Documents.mydocs` → `Documents`).
fn fallback_name(path: &Path) -> String {
    path.file_stem().or(path.file_name()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn display_name(path: &Path) -> Option<String> {
    let w = wide(path);
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// Sends `items` (files of one folder) to `target`, an entry of [`targets`] or a drive root: drops them on it. A copy or a link only,
/// never a move (Explorer's Send to never takes the files away from where they are). Call on the shell STA thread (`sta::run`) with its
/// owner window.
pub fn send(owner: HWND, target: &Path, items: &[PathBuf]) -> anyhow::Result<()> {
    let data = crate::dnd::data_object(items)?;
    let drop = drop_target(owner, target)?;
    let pt = POINTL::default();
    let mut effect = DROPEFFECT_COPY | DROPEFFECT_LINK;
    unsafe { drop.DragEnter(&data, MK_LBUTTON, pt, &mut effect)? };
    if effect == DROPEFFECT_NONE {
        unsafe {
            let _ = drop.DragLeave();
        }
        anyhow::bail!("{} takes none of these items", target.display());
    }
    let mut effect = DROPEFFECT_COPY | DROPEFFECT_LINK;
    unsafe { drop.Drop(&data, MK_LBUTTON, pt, &mut effect)? };
    Ok(())
}

/// The item's drop handler, bound through its real parent in the shell namespace (a drive root's parent is "This PC").
fn drop_target(owner: HWND, path: &Path) -> anyhow::Result<IDropTarget> {
    let abs = crate::dnd::parse(path)?;
    let mut last: *mut ITEMIDLIST = std::ptr::null_mut();
    let folder: IShellFolder = unsafe { SHBindToParent(abs.0, Some(&mut last))? };
    let children = [last as *const ITEMIDLIST];
    Ok(unsafe { folder.GetUIObjectOf(owner, &children, None)? })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_without_the_shell_drops_the_extension() {
        assert_eq!(fallback_name(Path::new(r"C:\x\SendTo\Documents.mydocs")), "Documents");
        assert_eq!(fallback_name(Path::new(r"C:\x\SendTo\Transcode for Discord.lnk")), "Transcode for Discord");
        assert_eq!(fallback_name(Path::new(r"C:\x\SendTo\noext")), "noext");
    }

    #[test]
    fn the_sendto_folder_is_found_and_its_desktop_ini_is_left_out() {
        crate::shell::ensure_com();
        let Some(dir) = folder() else { return };
        assert!(dir.ends_with("SendTo"), "{}", dir.display());
        assert!(targets().iter().all(|t| !t.path.file_name().is_some_and(|n| n.eq_ignore_ascii_case("desktop.ini"))));
    }

    /// Sends a file of a temporary folder to "Compressed (zipped) folder" and waits for the zip to appear next to it, as Explorer's
    /// Send to makes it. Touches nothing outside that folder. `cargo test -p rf-win sendto -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn manual_send_to_the_zip_entry() {
        crate::shell::ensure_com();
        let zip_target = targets().into_iter().find(|t| t.path.extension().is_some_and(|e| e.eq_ignore_ascii_case("ZFSendToTarget")));
        let Some(zip_target) = zip_target else { panic!("no Compressed (zipped) folder entry in SendTo") };
        println!("entry: {} ({})", zip_target.name, zip_target.path.display());
        let dir = std::env::temp_dir().join(format!("rf-sendto-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("hello.txt");
        std::fs::write(&file, b"hello").unwrap();
        send(HWND::default(), &zip_target.path, &[file]).unwrap();
        let start = std::time::Instant::now();
        let zip = loop {
            let found = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("zip")));
            if let Some(z) = found.filter(|z| std::fs::metadata(z).is_ok_and(|m| m.len() > 0)) {
                break z;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(15), "no zip after 15 s");
            // the zip is written by the shell on this thread's message loop
            unsafe {
                use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, MSG, PM_REMOVE, PeekMessageW, TranslateMessage};
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        println!("made {} in {:?}", zip.display(), start.elapsed());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// What issue 117 uses Send to for: a shortcut to a script, which gets the sent file as its argument. Both live in a temporary
    /// folder (nothing is put in the real SendTo folder). `cargo test -p rf-win sendto -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn manual_send_to_a_shortcut_runs_its_script_with_the_file() {
        crate::shell::ensure_com();
        let dir = std::env::temp_dir().join(format!("rf-sendto-lnk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("got.txt");
        let script = dir.join("take.cmd");
        std::fs::write(&script, format!("@echo %~1> \"{}\"\r\n", out.display())).unwrap();
        let link = dir.join("Take it.lnk");
        crate::shortcut::create(&link, &script).unwrap();
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"not a video").unwrap();
        send(HWND::default(), &link, std::slice::from_ref(&file)).unwrap();
        let start = std::time::Instant::now();
        let got = loop {
            if let Ok(s) = std::fs::read_to_string(&out) {
                break s;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(15), "the script did not run in 15 s");
            std::thread::sleep(std::time::Duration::from_millis(100));
        };
        println!("the script got {:?} after {:?}", got.trim(), start.elapsed());
        assert_eq!(got.trim(), file.to_string_lossy());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
