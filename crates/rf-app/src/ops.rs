//! File operations, clipboard, inline rename and the native context menu for a FolderView.
//! Everything that can block runs on worker threads or the shell STA thread.

use crate::t;
use std::path::PathBuf;

use futures::channel::oneshot;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use rf_win::ctxmenu::MenuOutcome;
use rf_win::fileops::{self, Transfer};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::folder_view::FolderView;
use rf_win::ctxmenu::ExtraItem;

const MENU_NEW_TAB: u32 = 1;
const MENU_PIN: u32 = 2;
const MENU_COPY_PATH: u32 = 3;
const MENU_TERMINAL: u32 = 4;
const MENU_NEW_FOLDER: u32 = 5;
const MENU_NEW_TEXT: u32 = 6;
const MENU_PASTE: u32 = 7;
const MENU_TAG_BASE: u32 = 100;
const MENU_EXTRACT_HERE: u32 = 10;
const MENU_EXTRACT_TO: u32 = 11;
const MENU_ZIP: u32 = 12;
const MENU_7Z: u32 = 13;
const MENU_TAGS: u32 = 14;
const MENU_OPEN_LOCATION: u32 = 15;
const MENU_BULK_RENAME: u32 = 16;
const MENU_SHELF: u32 = 17;
const MENU_OTHER_OPEN: u32 = 18;
const MENU_OTHER_COPY: u32 = 19;
const MENU_OTHER_MOVE: u32 = 20;
const MENU_EXTRACT_SMART: u32 = 21;
const MENU_EXTRACT_PICK: u32 = 22;

/// "photos.tar.gz" → "photos" (pure string work; the unique folder is chosen at extract time).
pub(crate) fn archive_stem(p: &std::path::Path) -> String {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let lower = name.to_lowercase();
    for suf in [".tar.gz", ".tar.zst", ".tar.xz", ".tar.bz2", ".tgz", ".tzst", ".txz", ".tbz2", ".tbz", ".zip", ".7z", ".rar", ".iso", ".tar", ".gz", ".xz", ".zst", ".bz2"] {
        if lower.ends_with(suf) && name.len() > suf.len() && name.is_char_boundary(name.len() - suf.len()) {
            return name[..name.len() - suf.len()].to_string();
        }
    }
    name
}

/// What "Flatten folder" did, for undo / redo: every file's (old, new) path and the folders it
/// removed.
#[derive(Debug, Clone, Default)]
pub struct Flattened {
    pub moves: Vec<(PathBuf, PathBuf)>,
    pub removed: Vec<PathBuf>,
}

/// Worker of "Flatten folder". `rec` gets every step that happened, also when it fails or is
/// cancelled half way, so even that can be undone.
fn flatten(dir: &std::path::Path, t: &fileops::Tracker, rec: &mut Flattened) -> anyhow::Result<bool> {
    use std::sync::atomic::Ordering;
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            let p = e.path();
            let ft = e.file_type()?;
            if ft.is_dir() && !ft.is_symlink() {
                stack.push(p.clone());
                dirs.push(p);
            } else if d != dir {
                files.push(p);
            }
        }
    }
    let total = files.len() as u32;
    for (i, f) in files.iter().enumerate() {
        if t.cancel.load(Ordering::Relaxed) {
            return Ok(false);
        }
        let name = f.file_name().unwrap_or_default().to_string_lossy().into_owned();
        (t.on_progress)(total, i as u32, Some(name.clone()));
        let mut dest = dir.join(&name);
        let (stem, ext) = match name.rfind('.') {
            Some(i) if i > 0 => (name[..i].to_string(), name[i..].to_string()),
            _ => (name.clone(), String::new()),
        };
        let mut n = 2;
        while dest.exists() {
            dest = dir.join(format!("{stem} ({n}){ext}"));
            n += 1;
        }
        std::fs::rename(f, &dest)?;
        rec.moves.push((f.clone(), dest));
    }
    // deepest first: a folder is empty once its children are gone
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        if std::fs::remove_dir(&d).is_ok() {
            rec.removed.push(d);
        }
    }
    (t.on_progress)(total, total, None);
    Ok(true)
}

/// Undo of a flatten: the folders come back, every file goes back to where it was. A file
/// that is gone, or whose old place is taken by a new one, is left alone (and reported).
pub(crate) fn unflatten(rec: &Flattened) -> anyhow::Result<bool> {
    // shallowest first (create_dir_all would manage anyway; this keeps it obvious)
    let mut dirs = rec.removed.clone();
    dirs.sort_by_key(|d| d.components().count());
    for d in &dirs {
        std::fs::create_dir_all(d)?;
    }
    let mut failed = 0usize;
    for (old, new) in rec.moves.iter().rev() {
        if old.exists() || !new.exists() {
            failed += 1;
            continue;
        }
        if let Some(parent) = old.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::rename(new, old).is_err() {
            failed += 1;
        }
    }
    anyhow::ensure!(failed == 0, "{failed} {}", t!("dosya eski yerine taşınamadı"));
    Ok(true)
}

/// Redo of a flatten: the same moves again, then the same folders removed (if still empty).
pub(crate) fn reflatten(rec: &Flattened) -> anyhow::Result<bool> {
    let mut failed = 0usize;
    for (old, new) in &rec.moves {
        if new.exists() || !old.exists() || std::fs::rename(old, new).is_err() {
            failed += 1;
        }
    }
    let mut dirs = rec.removed.clone();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for d in dirs {
        let _ = std::fs::remove_dir(&d);
    }
    anyhow::ensure!(failed == 0, "{failed} {}", t!("dosya taşınamadı"));
    Ok(true)
}

fn bg(name: &'static str, f: impl FnOnce() -> anyhow::Result<bool> + Send + 'static) {
    std::thread::Builder::new()
        .name(format!("rf-op-{name}"))
        .spawn(move || {
            if let Err(e) = f() {
                log::warn!("{name}: {e:#}");
            }
        })
        .ok();
}

impl FolderView {
    pub fn current_dir(&self) -> Option<PathBuf> {
        if let rf_core::Location::Library { file, .. } = self.location() {
            // new items / paste go to the library's default save folder
            return self.library_save_dir(file);
        }
        self.location().path().map(|p| p.to_path_buf())
    }

    /// Default save folder of a library (resolved in the background by `Places`).
    fn library_save_dir(&self, file: &std::path::Path) -> Option<PathBuf> {
        self.library_save.as_ref().filter(|(f, _)| f == file).map(|(_, d)| d.clone())
    }

    /// A shell folder (a phone…) is read-only here: says so and returns true.
    pub(crate) fn shell_read_only(&self, cx: &mut Context<Self>) -> bool {
        let shell = matches!(self.location(), rf_core::Location::Shell { .. });
        if shell {
            crate::shell_view::read_only_notice(cx);
        }
        shell
    }

    pub fn copy_to_clipboard(&mut self, kind: Transfer, _window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.location(), rf_core::Location::Shell { .. }) {
            // items of a phone: copied out with the shell's engine; a cut deletes them from the device once they were pasted
            crate::shell_view::copy_items(cx, self.selection_shell(), kind == rf_win::fileops::Transfer::Move);
            return;
        }
        let paths = self.selection_paths();
        if paths.is_empty() {
            return;
        }
        // items of an FTP server: as a phone's, fetched by Windows' engine into a temp folder and those copies put on the clipboard
        // (issue 223: the clipboard got the `ftp://` addresses, which no paste can use)
        if paths.first().is_some_and(|p| rf_win::ftp::is_ftp(p)) {
            crate::shell_view::copy_ftp(cx, self.ftp_items(&paths), kind == Transfer::Move);
            return;
        }
        let in_archive = self.current_dir().is_some_and(|d| crate::archive_view::is_browsable_path(&d));
        if in_archive {
            // items inside an archive: extract them (Status Center), then the copies go on
            // the clipboard; cut becomes copy
            crate::archive_view::copy_items(cx, paths);
            return;
        }
        // a cut is drawn faded until it is pasted or the clipboard changes (issue 201); a copy ends an earlier cut's marks
        crate::cut_marks::cutting((kind == Transfer::Move).then_some(paths.as_slice()));
        cx.notify();
        // OpenClipboard can wait on other apps: never on the UI thread.
        rf_win::sta::run(move |owner| match rf_win::clipboard::set_files(owner, &paths, kind) {
            Ok(_) => crate::cut_marks::written(&paths),
            Err(e) => {
                crate::cut_marks::cutting(None);
                log::warn!("clipboard: {e:#}");
            }
        });
    }

    /// Ctrl+Shift+C: plain paths, one per line (Ctrl+Alt+C: with quotes, see `copy_path_with`).
    pub fn copy_path(&mut self, cx: &mut Context<Self>) {
        self.copy_path_with(false, cx)
    }

    pub fn copy_path_with(&mut self, quoted: bool, cx: &mut Context<Self>) {
        let paths = self.selection_paths();
        let q = |s: String| if quoted { format!("\"{s}\"") } else { s };
        let text = if paths.is_empty() {
            self.current_dir().map(|d| q(d.to_string_lossy().into_owned())).unwrap_or_default()
        } else {
            paths.iter().map(|p| q(p.display().to_string())).collect::<Vec<_>>().join("\r\n")
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// Files' "Rotate left / right" on the selected images (JPEG: orientation tag only, others
    /// rotated losslessly through WIC). The list reloads; new thumbnails follow the new mtime.
    pub fn rotate_selection(&mut self, clockwise: bool, window: &mut Window, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .selection_paths()
            .into_iter()
            .filter(|p| rf_win::rotate::can_rotate(p) && crate::archive_view::split(p).is_none())
            .collect();
        if paths.is_empty() {
            return;
        }
        let task = cx.background_spawn(async move {
            let mut errors = Vec::new();
            for p in &paths {
                if let Err(e) = rf_win::rotate::rotate(p, clockwise) {
                    errors.push(format!("{}: {e:#}", p.file_name().unwrap_or_default().to_string_lossy()));
                }
            }
            errors
        });
        cx.spawn_in(window, async move |this, cx| {
            let errors = task.await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.soft_reload(cx);
                if !errors.is_empty() {
                    log::warn!("rotate: {}", errors.join("; "));
                    crate::prompt::show(
                        t!("Döndürülemedi"),
                        Some(errors.join("
")),
                        None,
                        vec![crate::prompt::Button::new(t!("Tamam"), crate::prompt::Kind::Primary, |_, _, _| Ok(()))],
                        window,
                        cx,
                    );
                }
            });
        })
        .detach();
    }

    /// FTP items of this listing as (address, is a folder), for the server's own operations.
    pub(crate) fn ftp_items(&self, paths: &[PathBuf]) -> Vec<(String, bool)> {
        paths
            .iter()
            .map(|p| {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                let is_dir = self.entries.iter().any(|e| *e.name == *name && e.is_dir());
                (p.to_string_lossy().replace('\\', "/"), is_dir)
            })
            .collect()
    }

    /// Delete on an FTP server: for good (it has no recycle bin), after asking; the server deletes, then the list is read again.
    fn delete_on_ftp(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.ftp_items(&paths);
        let what = if paths.len() == 1 { format!("\"{}\"", rf_core::history::display_name(&paths[0])) } else { crate::i18n::items(paths.len()) };
        let view = cx.entity().downgrade();
        crate::prompt::show(
            t!("Kalıcı olarak silinsin mi?"),
            Some(format!("{what} {}", t!("geri alınamayacak şekilde silinecek."))),
            None,
            vec![
                crate::prompt::Button::new(t!("Sil"), crate::prompt::Kind::Danger, move |_, _, cx| {
                    let (items, view) = (items.clone(), view.clone());
                    let title = format!("{} {}", t!("Kalıcı olarak siliniyor:"), crate::i18n::items(items.len()));
                    crate::status_center::start_with(
                        cx,
                        title,
                        move |_| {
                            for (url, is_dir) in &items {
                                rf_win::ftp::delete(url, *is_dir)?;
                            }
                            Ok(true)
                        },
                        Some(Box::new(move |_, cx| {
                            let _ = view.update(cx, |v, cx| v.refresh(cx));
                        })),
                    );
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    /// A shell folder's parsing name (a phone's folder): where a paste or a drop goes, through Windows' own engine (issue 221: the
    /// phone's folders were read-only here).
    pub(crate) fn shell_dest(&self) -> Option<PathBuf> {
        match self.location() {
            rf_core::Location::Shell { parsing, .. } => Some(PathBuf::from(parsing)),
            _ => None,
        }
    }

    /// The clipboard's files into the shell folder `dest` (a phone's), then the folder is read again (nothing watches it).
    fn paste_into_shell(&mut self, dest: PathBuf, cx: &mut Context<Self>) {
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::clipboard::get_files(owner).map_err(|e| log::warn!("paste: {e:#}")).ok().flatten());
        });
        cx.spawn(async move |this, cx| {
            let Ok(Some((paths, kind))) = rx.await else { return };
            let _ = cx.update(|cx| {
                crate::status_center::transfer_then(cx, kind, paths, dest, move |_, cx| {
                    let _ = this.update(cx, |v, cx| v.refresh(cx));
                })
            });
        })
        .detach();
    }

    /// Delete in a shell folder (a phone's): there is no recycle bin there, so it is for good, after asking; Windows' engine does it.
    fn delete_in_shell(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let items = self.selection_shell();
        if items.is_empty() {
            return;
        }
        let what = if items.len() == 1 { format!("\"{}\"", items[0].name) } else { crate::i18n::items(items.len()) };
        let paths: Vec<PathBuf> = items.iter().map(|c| c.fs_path.clone().unwrap_or_else(|| PathBuf::from(&c.parsing))).collect();
        let view = cx.entity().downgrade();
        crate::prompt::show(
            t!("Kalıcı olarak silinsin mi?"),
            Some(format!("{what} {}", t!("geri alınamayacak şekilde silinecek."))),
            None,
            vec![
                crate::prompt::Button::new(t!("Sil"), crate::prompt::Kind::Danger, move |_, _, cx| {
                    let view = view.clone();
                    crate::status_center::delete_in_shell(cx, paths.clone(), move |cx| {
                        let _ = view.update(cx, |v, cx| v.refresh(cx));
                    });
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    pub fn paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dest) = self.shell_dest() {
            self.paste_into_shell(dest, cx);
            return;
        }
        if self.shell_read_only(cx) {
            return;
        }
        let Some(dir) = self.current_dir() else { return };
        self.paste_to(dir, window, cx);
    }

    /// Ctrl+Shift+V: paste into the selected folder (falls back to the current one).
    pub fn paste_into_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell_read_only(cx) {
            return;
        }
        let target = if self.selected_is_single_dir() { self.selection_paths().into_iter().next() } else { None };
        match target.or_else(|| self.current_dir()) {
            Some(dir) => self.paste_to(dir, window, cx),
            None => {}
        }
    }

    /// Creates shortcuts (.lnk) to the clipboard items in the current folder.
    pub fn paste_shortcuts(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if self.shell_read_only(cx) {
            return;
        }
        let Some(dir) = self.current_dir() else { return };
        if rf_win::ftp::is_ftp(&dir) {
            return;
        }
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::clipboard::get_files(owner).map_err(|e| log::warn!("paste: {e:#}")).ok().flatten());
        });
        cx.spawn(async move |this, cx| {
            let Ok(Some((paths, _))) = rx.await else { return };
            let work = cx.background_spawn(async move {
                let mut made = Vec::new();
                for p in paths {
                    let stem = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "Kısayol".into());
                    // never overwrite: "x.lnk", "x (2).lnk", …
                    let mut link = dir.join(format!("{stem}.lnk"));
                    let mut n = 2;
                    while link.exists() {
                        link = dir.join(format!("{stem} ({n}).lnk"));
                        n += 1;
                    }
                    match rf_win::shortcut::create(&link, &p) {
                        Ok(()) => made.push(link),
                        Err(e) => log::warn!("shortcut {link:?}: {e:#}"),
                    }
                }
                // Ctrl+Z takes these shortcuts away. Without a record it undid the operation BEFORE them: after a copy of two
                // pictures and a mistaken "Paste shortcut", Ctrl+Z sent the two pictures to the Recycle Bin and left the shortcut
                // (a Discord report, 1.0.0).
                if !made.is_empty() {
                    crate::undo::push(crate::undo::UndoOp::CreatedAll { paths: made });
                }
            });
            work.await;
            let _ = this.update(cx, |v, cx| v.refresh(cx));
        })
        .detach();
    }

    pub(crate) fn paste_to(&mut self, dir: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        // GetClipboardData can block on a slow source app (delayed rendering).
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::clipboard::get_files(owner).map_err(|e| log::warn!("paste: {e:#}")).ok().flatten());
        });
        if rf_win::ftp::is_ftp(&dir) {
            // FTP: upload the clipboard files (folders are skipped), then refresh
            let url = dir.to_string_lossy().into_owned();
            cx.spawn(async move |this, cx| {
                if let Ok(Some((paths, _))) = rx.await {
                    let work = cx.background_spawn(async move {
                        // (folders with all they hold)
                        for p in &paths {
                            if let Err(e) = rf_win::ftp::upload_tree(p, &url) {
                                log::warn!("ftp upload {p:?}: {e}");
                            }
                        }
                    });
                    work.await;
                    let _ = this.update(cx, |v, cx| v.refresh(cx));
                }
            })
            .detach();
            return;
        }
        cx.spawn(async move |this, cx| {
            let clip = rx.await.ok().flatten();
            // No files with paths: files that live only in the program that copied them (a remote desktop session, a tool that
            // carries the clipboard between PCs) are pasted from that program's own data (`rf_win::vfiles`).
            if clip.is_none() && !rf_win::vfiles::clipboard_has() {
                let _ = cx.update(|cx| {
                    let item = cx.read_from_clipboard();
                    let text = item.as_ref().and_then(|c| c.text());
                    // a link on the clipboard, with HyperDownload installed: offered for download into this folder
                    if let Some(link) = text.as_deref().map(str::trim).filter(|t| crate::hyperdownload::is_link(t))
                        && crate::addons::installed(crate::hyperdownload::ID).is_some()
                    {
                        crate::hyperdownload::offer_pasted_link(link.to_string(), dir.clone(), cx);
                        return;
                    }
                    // text or a picture: a file of it here, with the rename box on it (issue 151)
                    let image = item.as_ref().and_then(|c| {
                        c.entries().iter().find_map(|e| match e {
                            ClipboardEntry::Image(i) => Some(i.clone()),
                            _ => None,
                        })
                    });
                    let made = match (text.filter(|t| !t.trim().is_empty()), image) {
                        (Some(t), _) => Some(Pasted::Text(t)),
                        (None, Some(i)) => Some(Pasted::Image(i)),
                        _ => None,
                    };
                    if let Some(made) = made {
                        paste_as_file(made, dir.clone(), this.clone(), cx);
                    }
                });
                return;
            }
            let _ = cx.update(|cx| {
                let here = dir.clone();
                let select = move |arrived: Vec<PathBuf>, cx: &mut App| {
                    // select what arrived, if this view still shows that folder
                    let names: Vec<String> = arrived.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
                    let _ = this.update(cx, |v, cx| {
                        if v.current_dir().as_deref() == Some(here.as_path()) {
                            v.select_when_listed(names, cx);
                        }
                    });
                };
                match clip {
                    Some((paths, kind)) => crate::status_center::transfer_then(cx, kind, paths, dir, select),
                    None => crate::status_center::paste_virtual(cx, dir, select),
                }
            });
        })
        .detach();
    }

    /// Copies of the selected items beside them ("x - Kopya", as a paste into the same folder names them); the copy of a single item
    /// gets the rename box at once (issue 161).
    pub fn duplicate_selected(&mut self, cx: &mut Context<Self>) {
        if self.shell_read_only(cx) {
            return;
        }
        let Some(dir) = self.current_dir() else { return };
        if rf_win::ftp::is_ftp(&dir) || crate::archive_view::split(&dir).is_some() {
            return;
        }
        let paths = self.selection_paths();
        if paths.is_empty() {
            return;
        }
        let this = cx.entity().downgrade();
        let here = dir.clone();
        crate::status_center::transfer_then(cx, Transfer::Copy, paths, dir, move |arrived, cx| {
            let mut names: Vec<String> = arrived.iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
            let _ = this.update(cx, |v, cx| {
                if v.current_dir().as_deref() != Some(here.as_path()) {
                    return;
                }
                if names.len() == 1 {
                    v.expect_new_item(names.pop(), cx);
                } else {
                    v.select_when_listed(names, cx);
                }
            });
        });
    }

    /// The selection into the other pane of a split tab (issue 152); nothing in a tab that is not split or inside an archive.
    pub fn to_other_pane(&mut self, kind: Transfer, cx: &mut Context<Self>) {
        if !self.dual || self.renaming.is_some() {
            return;
        }
        if self.current_dir().is_some_and(|d| crate::archive_view::split(&d).is_some()) {
            return;
        }
        let paths = self.selection_paths();
        if !paths.is_empty() {
            cx.emit(crate::folder_view::FolderEvent::ToOtherPane(kind, paths));
        }
    }

    /// Deletes the selection, asking first per Settings → "Confirm before deleting".
    pub fn delete_selected(&mut self, permanent: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell_dest().is_some() {
            self.delete_in_shell(window, cx);
            return;
        }
        let paths = self.selection_paths();
        self.delete_paths(paths, permanent, window, cx);
    }

    /// Deletes `paths` the way the selection is deleted (the same question first, the same Status Center work). The sidebar's
    /// folder menu uses it for the folder it was opened on (issue 53).
    pub fn delete_paths(&mut self, paths: Vec<PathBuf>, permanent: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::settings::DeletePref;
        if paths.is_empty() {
            return;
        }
        if rf_win::ftp::is_ftp(&paths[0]) {
            self.delete_on_ftp(paths, window, cx);
            return;
        }
        // inside an archive: remove the entries from it (always asks: there is no recycle bin).
        // An archive file itself splits as its own root (an empty inner path): that is a plain
        // file to delete (#11: deleting a .zip asked to delete "" from inside it and failed)
        let inside = |p: &PathBuf| crate::archive_view::split_inside(p);
        if let Some((archive, _)) = inside(&paths[0]) {
            let inners: Vec<String> = paths
                .iter()
                .filter_map(|p| inside(p).filter(|(a, _)| *a == archive))
                .map(|(_, i)| i.to_string_lossy().replace('\\', "/"))
                .collect();
            let what = if inners.len() == 1 { format!("\"{}\"", inners[0].rsplit('/').next().unwrap_or_default()) } else { crate::i18n::items(inners.len()) };
            crate::prompt::show(
                t!("Arşivden silinsin mi?"),
                Some(format!("{what} {}", t!("arşivden kalıcı olarak silinecek."))),
                None,
                vec![
                    crate::prompt::Button::new(t!("Sil"), crate::prompt::Kind::Danger, move |_, _, cx| {
                        crate::archive_view::delete_inside(cx, archive.clone(), inners.clone());
                        Ok(())
                    }),
                    crate::prompt::Button::cancel(),
                ],
                window,
                cx,
            );
            return;
        }
        // A WSL drive or a network share has no Recycle Bin: Delete there is a permanent delete, asked with our own question (Windows'
        // engine would ask its own "permanently delete?" instead: two different dialogs for the same thing, Discord 2026-11-23)
        let permanent = permanent || rf_win::localized::is_network(&paths[0]);
        let ask = match cx.global::<crate::settings::Settings>().confirm_delete {
            DeletePref::Always => true,
            DeletePref::PermanentOnly => permanent,
            DeletePref::Never => false,
        };
        // a WSL distribution's folder sends no change notices: the view is read again when the deletion is over (Discord, 2026-11-23:
        // "After deleting files from a WSL drive, they remain visible")
        let view = cx.entity().downgrade();
        let unwatched = paths.first().is_some_and(|p| rf_win::fastcopy::on_wsl(p));
        let run = move |cx: &mut App, paths: Vec<PathBuf>| {
            if unwatched {
                let view = view.clone();
                crate::status_center::delete_then(cx, paths, permanent, move |cx| {
                    let _ = view.update(cx, |v, cx| v.soft_reload(cx));
                });
            } else {
                crate::status_center::delete(cx, paths, permanent);
            }
        };
        if !ask {
            run(cx, paths);
            return;
        }
        let what = if paths.len() == 1 {
            format!("\"{}\"", rf_core::history::display_name(&paths[0]))
        } else {
            crate::i18n::items(paths.len())
        };
        let (title, msg) = if permanent {
            (t!("Kalıcı olarak silinsin mi?"), format!("{what} {}", t!("geri alınamayacak şekilde silinecek.")))
        } else {
            (t!("Geri dönüşüm kutusuna taşınsın mı?"), format!("{what} {}", t!("geri dönüşüm kutusuna taşınacak.")))
        };
        crate::prompt::show(
            title,
            Some(msg),
            None,
            vec![
                crate::prompt::Button::new(t!("Sil"), if permanent { crate::prompt::Kind::Danger } else { crate::prompt::Kind::Primary }, move |_, _, cx| {
                    run(cx, paths.clone());
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    pub fn create_new(&mut self, folder: bool, cx: &mut Context<Self>) {
        if self.shell_read_only(cx) {
            return;
        }
        let Some(dir) = self.current_dir() else { return };
        // inside an archive: a folder entry, or an empty text file added to it
        if let Some((archive, inner)) = crate::archive_view::split(&dir) {
            let taken: Vec<String> = self.entries.iter().map(|e| e.name.to_string()).collect();
            let inner = inner.to_string_lossy().replace('\\', "/").trim_matches('/').to_string();
            if folder {
                let name = crate::archive_view::unique_name(t!("Yeni klasör"), "", &taken);
                crate::archive_view::mkdir_inside(cx, archive, inner, name.clone());
                self.set_reselect(name);
            } else {
                let name = crate::archive_view::unique_name(t!("Yeni metin belgesi"), ".txt", &taken);
                let tmp = std::env::temp_dir().join(format!("rustyfiles-new-{}", std::process::id()));
                let file = tmp.join(&name);
                if std::fs::create_dir_all(&tmp).and_then(|_| std::fs::write(&file, b"")).is_ok() {
                    crate::archive_view::add_into(cx, archive, std::path::PathBuf::from(inner), vec![file]);
                    self.set_reselect(name);
                }
            }
            return;
        }
        let name = if folder { t!("Yeni klasör") } else { t!("Yeni metin belgesi") }.to_string();
        // on an FTP server: the server makes the folder, an empty file is uploaded (issue 223); then the rename box, as anywhere
        if rf_win::ftp::is_ftp(&dir) {
            let taken: Vec<String> = self.entries.iter().map(|e| e.name.to_lowercase()).collect();
            let ext = if folder { "" } else { ".txt" };
            let mut pick = format!("{name}{ext}");
            let mut n = 2;
            while taken.contains(&pick.to_lowercase()) {
                pick = format!("{name} ({n}){ext}");
                n += 1;
            }
            let url = dir.to_string_lossy().replace('\\', "/");
            let made = pick.clone();
            let task = cx.background_spawn(async move {
                if folder {
                    return rf_win::ftp::make_dir(&url, &made);
                }
                let tmp = std::env::temp_dir().join(format!("rustyfiles-new-{}", std::process::id()));
                std::fs::create_dir_all(&tmp)?;
                let file = tmp.join(&made);
                std::fs::write(&file, b"")?;
                let r = rf_win::ftp::upload(&file, &url);
                let _ = std::fs::remove_file(&file);
                r
            });
            cx.spawn(async move |this, cx| {
                if let Err(e) = task.await {
                    log::warn!("ftp: new item: {e}");
                    return;
                }
                let _ = this.update(cx, |v, cx| v.expect_new_item(Some(pick), cx));
            })
            .detach();
            return;
        }
        let task = cx.background_spawn(async move {
            if folder { fileops::new_folder(&dir, &name) } else { fileops::new_text_file(&dir, &name) }
        });
        cx.spawn(async move |this, cx| {
            if let Ok(path) = task.await {
                crate::undo::push(crate::undo::UndoOp::Created { path: path.clone() });
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                let _ = this.update(cx, |v, cx| {
                    v.expect_new_item(name, cx);
                });
            }
        })
        .detach();
    }

    /// "Create folder with selection": a new folder here, the selected items moved into it,
    /// then rename the folder inline.
    pub fn folder_with_selection(&mut self, items: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir() else { return };
        if items.is_empty() {
            return;
        }
        let name = t!("Yeni klasör").to_string();
        let task = cx.background_spawn(async move { fileops::new_folder(&dir, &name) });
        cx.spawn(async move |this, cx| {
            if let Ok(folder) = task.await {
                let name = folder.file_name().map(|n| n.to_string_lossy().into_owned());
                let _ = this.update(cx, |v, cx| {
                    crate::status_center::transfer(cx, Transfer::Move, items, folder.clone());
                    v.expect_new_item(name, cx);
                });
            }
        })
        .detach();
    }

    /// "Flatten folder": every file in the subfolders of `dir` moves up into `dir` (unique
    /// names on collisions), then the emptied subfolders are removed. Asks first.
    pub fn flatten_folder(&mut self, dir: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let name = rf_core::history::display_name(&dir);
        crate::prompt::show(
            t!("Klasör düzleştirilsin mi?"),
            Some(format!("{} \"{name}\" {}", t!("Alt klasörlerdeki tüm dosyalar"), t!("içine taşınır ve boş kalan alt klasörler silinir."))),
            None,
            vec![
                crate::prompt::Button::new(t!("Düzleştir"), crate::prompt::Kind::Primary, move |_, _, cx| {
                    let dir = dir.clone();
                    crate::status_center::start(cx, format!("{} {name}", t!("Düzleştiriliyor:")), move |t| {
                        let mut rec = Flattened::default();
                        let r = flatten(&dir, &t, &mut rec);
                        if !rec.moves.is_empty() || !rec.removed.is_empty() {
                            crate::undo::push(crate::undo::UndoOp::Flattened { dir: dir.clone(), rec });
                        }
                        r
                    });
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    pub fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.shell_read_only(cx) {
            return;
        }
        let Some(ei) = self.cursor_entry() else { return };
        let e = self.entry(ei);
        // Extensions hidden: edit only the name and keep the extension (as Explorer does)
        let name = match self.hidden_extension(ei, cx) {
            Some(_) => e.name[..e.stem_len()].to_string(),
            None => e.name.to_string(),
        };
        let sel_end = if e.is_dir() { name.len() } else { e.stem_len().min(name.len()) };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } | InputEvent::Blur => this.commit_rename(window, cx),
            _ => {}
        });
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(0..sel_end, cx);
        });
        self.set_renaming(Some((ei, input)), Some(sub));
        cx.notify();
    }

    /// Applies the name in the rename box, when one is open (a press outside it).
    pub fn commit_rename_if_open(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            self.commit_rename(window, cx);
        }
    }

    pub fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_renaming(None, None);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The extension (with its dot) that the list hides for this entry, if any.
    fn hidden_extension(&self, ei: u32, cx: &App) -> Option<String> {
        let e = self.entry(ei);
        let hidden = !cx.global::<crate::settings::Settings>().show_extensions && !e.is_dir() && !e.ext.is_empty();
        hidden.then(|| e.name[e.stem_len()..].to_string())
    }

    pub fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.commit_rename_quietly(window, cx);
    }

    /// [`commit_rename`](Self::commit_rename), saying whether the name went through without a question or a notice on the screen
    /// (Tab goes on to the next item only then).
    pub(crate) fn commit_rename_quietly(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some((ei, input)) = self.renaming.take() else { return false };
        let typed = input.read(cx).value().trim().to_string();
        // characters Windows does not allow: tidied rather than refused, so that what was typed is not lost (issue 370)
        let typed = if cx.global::<crate::settings::Settings>().fix_name_characters { rf_core::rename::tidy_name(&typed) } else { typed };
        let new_name = match self.hidden_extension(ei, cx) {
            Some(ext) if !typed.is_empty() => format!("{typed}{ext}"),
            _ => typed,
        };
        let old = self.entry(ei).name.to_string();
        self.set_renaming(None, None);
        // the list has the keyboard again from here on: a dialog opened below gives it back to the list when it closes
        window.focus(&self.focus, cx);
        // Explorer's rule, said up front instead of a shell error after the fact
        const INVALID: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
        if new_name.contains(INVALID) || new_name.ends_with('.') {
            let title = crate::t!("Yeniden adlandır").to_string();
            let msg = format!("{}\n\n{}", new_name, crate::t!("Dosya adı şu karakterleri içeremez: \\ / : * ? \" < > | ve nokta ile bitemez."));
            crate::prompt::notice(title, msg, window, cx);
            cx.notify();
            return false;
        }
        // Inside an archive: rewrite the archive with the new name. An archive file ITSELF splits as its own root, with an empty
        // inner path, and is a plain file to rename (issue 36: "after zip something, I couldn't rename the .zip", with "not found"
        // in the Status Center: the .zip was asked to rename "" inside itself. The same mistake as issue 11's delete).
        if !new_name.is_empty() && new_name != old {
            let from_path = self.parent_of(ei).map(|d| d.join(&old));
            if let Some((archive, inner)) = from_path.as_deref().and_then(crate::archive_view::split_inside) {
                let inner = inner.to_string_lossy().replace('\\', "/");
                crate::archive_view::rename_inside(cx, archive, inner, new_name.clone());
                self.set_reselect(new_name);
                cx.notify();
                return true;
            }
        }
        // still being copied here: the copy would go on under the old name (issue 297)
        if !new_name.is_empty() && new_name != old && self.parent_of(ei).is_some_and(|d| crate::status_center::still_arriving(&d.join(&old))) {
            let msg = format!("{old}\n\n{}", crate::t!("Bu öğe hâlâ kopyalanıyor. Kopyalama bitince yeniden adlandırın."));
            crate::prompt::notice(crate::t!("Yeniden adlandır").to_string(), msg, window, cx);
            cx.notify();
            return false;
        }
        let mut quiet = true;
        if !new_name.is_empty() && new_name != old {
            if let Some(dir) = self.parent_of(ei).map(|d| d.to_path_buf()) {
                let from = dir.join(&old);
                let to = dir.join(&new_name);
                self.set_reselect(new_name.clone());
                // (the row shows the new name at once, not the old one until the watcher has seen the rename)
                self.show_renamed(&old, &new_name);
                // Explorer-style warning when a file's extension changes.
                let ext = |n: &str| std::path::Path::new(n).extension().map(|e| e.to_string_lossy().to_lowercase());
                let warn = cx.global::<crate::settings::Settings>().warn_extension_change;
                // (a dialog of our own, in the window: it was a Windows message box, which opened behind the window, issue 30)
                let asks = warn && !self.entry(ei).is_dir() && ext(&old).is_some() && ext(&old) != ext(&new_name);
                quiet = !asks;
                let confirm_rx = if asks {
                    let msg = crate::t!("Dosya adı uzantısını değiştirirseniz dosya kullanılamaz hale gelebilir. Değiştirmek istediğinizden emin misiniz?").to_string();
                    Some(crate::prompt::ask(crate::t!("Dosya adı uzantısı"), msg, window, cx))
                } else {
                    None
                };
                let handle = window.window_handle();
                let (f2, n2) = (from.clone(), new_name.clone());
                cx.spawn(async move |this, cx| {
                    if let Some(r) = confirm_rx {
                        if !r.await.unwrap_or(false) {
                            let _ = this.update(cx, |v, cx| v.rename_failed(cx));
                            return;
                        }
                    }
                    // Another item already has that name (a case-only change is fine): say so
                    // plainly instead of the shell's "parameter is incorrect".
                    let (src, dst) = (f2.clone(), f2.with_file_name(&n2));
                    let clash = cx
                        .background_spawn(async move {
                            let case_only = src.to_string_lossy().to_lowercase() == dst.to_string_lossy().to_lowercase();
                            !case_only && std::fs::symlink_metadata(&dst).is_ok()
                        })
                        .await;
                    if clash {
                        let msg = format!("{}\n\n{}", n2, crate::t!("Bu adda bir öğe zaten var. Başka bir ad deneyin."));
                        let _ = handle.update(cx, |_, window, cx| crate::prompt::notice(crate::t!("Yeniden adlandır"), msg, window, cx));
                        let _ = this.update(cx, |v, cx| v.rename_failed(cx));
                        return;
                    }
                    let (tx, rx) = oneshot::channel::<anyhow::Result<bool>>();
                    let ftp = rf_win::ftp::is_ftp(&f2);
                    bg("rename", move || {
                        // (an FTP server's item: the server renames it, issue 223)
                        let r = if ftp { rf_win::ftp::rename(&f2.to_string_lossy(), &n2).map(|()| true).map_err(Into::into) } else { fileops::rename(&f2, &n2) };
                        let _ = tx.send(r);
                        Ok(true)
                    });
                    match rx.await {
                        Ok(Ok(true)) if ftp => {
                            // nothing watches a server: the list is read again
                            let _ = this.update(cx, |v, cx| v.refresh(cx));
                        }
                        Ok(Ok(true)) => {
                            crate::undo::push(crate::undo::UndoOp::Renamed { from: from.clone(), to: to.clone() });
                            let _ = cx.update(|cx| {
                                crate::tags::update(cx, |t| t.moved(&from, &to));
                                // a pinned folder renamed in the list: its pin follows it
                                crate::places::pins_follow(cx, &from, Some(&to));
                            });
                        }
                        other => {
                            if let Ok(Err(e)) = other {
                                log::warn!("rename: {e:#}");
                            }
                            // nothing to wait for
                            let _ = this.update(cx, |v, cx| v.rename_failed(cx));
                        }
                    }
                })
                .detach();
            }
        }
        cx.notify();
        quiet
    }

    /// The classic Explorer context menu (Shift+right-click) for the selection or background.
    pub fn show_native_menu(&mut self, background: bool, cx: &mut Context<Self>) {
        let Some(dir) = self.current_dir() else { return };
        let items = if background { Vec::new() } else { self.selection_paths() };
        let background = background || items.is_empty();
        // From the listing, not the disk: a stat here could hang on an offline network path.
        let single_dir = items.len() == 1 && self.selected_is_single_dir();
        let item = |id: u32, label: &str| ExtraItem { id, label: label.to_string(), enabled: true };
        let extra: Vec<ExtraItem> = if background {
            vec![
                item(MENU_NEW_FOLDER, t!("Yeni klasör\tCtrl+Shift+N")),
                item(MENU_NEW_TEXT, t!("Yeni metin belgesi")),
                ExtraItem { id: MENU_PASTE, label: t!("Yapıştır\tCtrl+V").into(), enabled: rf_win::clipboard::can_paste() },
                item(MENU_TERMINAL, t!("Terminalde aç")),
                item(MENU_COPY_PATH, t!("Yolu kopyala")),
            ]
        } else {
            let mut v = Vec::new();
            if self.is_search() && items.len() == 1 {
                v.push(item(MENU_OPEN_LOCATION, t!("Dosya konumunu aç")));
            }
            if single_dir {
                v.push(item(MENU_NEW_TAB, t!("Yeni sekmede aç")));
                v.push(item(MENU_PIN, t!("Kenar çubuğuna sabitle")));
                v.push(item(MENU_TERMINAL, t!("Terminalde aç")));
            }
            v.push(item(MENU_COPY_PATH, t!("Yolu kopyala\tCtrl+Shift+C")));
            if items.len() > 1 && !self.is_search() {
                v.push(item(MENU_BULK_RENAME, t!("Toplu yeniden adlandır…\tF2")));
            }
            if !items.is_empty() && items.iter().all(|p| rf_archive::is_supported(p)) {
                v.push(item(MENU_EXTRACT_HERE, t!("Buraya çıkar")));
                v.push(item(MENU_EXTRACT_SMART, t!("Akıllı çıkar")));
                if items.len() == 1 {
                    v.push(item(MENU_EXTRACT_TO, &format!("{} {}\\", t!("Klasöre çıkar:"), archive_stem(&items[0]))));
                } else {
                    v.push(item(MENU_EXTRACT_TO, t!("Her birini kendi klasörüne çıkar")));
                }
                v.push(item(MENU_EXTRACT_PICK, t!("Şuraya çıkar…")));
            }
            v.push(item(MENU_ZIP, t!("ZIP olarak sıkıştır")));
            v.push(item(MENU_7Z, t!("7z olarak sıkıştır")));
            v.push(item(MENU_TAGS, t!("Etiketle…")));
            v.push(item(MENU_SHELF, t!("Rafa ekle\tCtrl+Shift+S")));
            if self.dual {
                if single_dir {
                    v.push(item(MENU_OTHER_OPEN, t!("Diğer panelde aç")));
                }
                v.push(item(MENU_OTHER_COPY, t!("Diğer panele kopyala")));
                v.push(item(MENU_OTHER_MOVE, t!("Diğer panele taşı")));
            }
            v
        };
        let target_dir = if single_dir { items[0].clone() } else { dir.clone() };
        let tag_items = items.clone();
        let paste_dir_ui = dir.clone();
        let (tx, rx) = oneshot::channel::<(MenuOutcome, Option<(Vec<PathBuf>, Transfer)>)>();
        // (until the menu is answered: see `native_menu`)
        self.native_menu = true;
        cx.notify();
        // Read Shift here: the shell thread has its own input state.
        let extended = unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 };
        rf_win::sta::run(move |owner| {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let r = if background {
                rf_win::ctxmenu::show_for_background(owner, &dir, (pt.x, pt.y), extended, &extra)
            } else {
                rf_win::ctxmenu::show_for_items(owner, &items, (pt.x, pt.y), extended, &extra)
            };
            let r = r.unwrap_or_else(|e| {
                log::warn!("context menu: {e:#}");
                MenuOutcome::Dismissed
            });
            // Read the clipboard right here on the shell thread; the UI starts the transfer.
            let clip = if r == MenuOutcome::Custom(MENU_PASTE) {
                rf_win::clipboard::get_files(owner).ok().flatten()
            } else {
                None
            };
            let _ = tx.send((r, clip));
        });
        cx.spawn(async move |this, cx| {
            let (outcome, clip) = rx.await.unwrap_or((MenuOutcome::Dismissed, None));
            let _ = this.update(cx, |v, cx| {
                v.native_menu = false;
                cx.notify();
            });
            let _ = this.update(cx, |v, cx| match outcome {
                MenuOutcome::Custom(MENU_PASTE) => {
                    if let Some((paths, kind)) = clip {
                        crate::status_center::transfer(cx, kind, paths, paste_dir_ui.clone());
                    } else if rf_win::vfiles::clipboard_has() {
                        crate::status_center::paste_virtual(cx, paste_dir_ui.clone(), |_, _| {});
                    }
                }
                MenuOutcome::Rename => {
                    v.request_rename();
                    cx.notify();
                }
                MenuOutcome::Custom(MENU_NEW_FOLDER) => v.create_new(true, cx),
                MenuOutcome::Custom(MENU_NEW_TEXT) => v.create_new(false, cx),
                MenuOutcome::Custom(MENU_COPY_PATH) => v.copy_path(cx),
                MenuOutcome::Custom(MENU_TERMINAL) => {
                    let d = target_dir.clone();
                    std::thread::spawn(move || {
                        let _ = rf_win::shell::open_terminal(&d);
                    });
                }
                MenuOutcome::Custom(MENU_NEW_TAB) => {
                    cx.emit(crate::folder_view::FolderEvent::OpenInNewTab(rf_core::Location::Path(target_dir.clone())))
                }
                MenuOutcome::Custom(MENU_PIN) => v.pin(target_dir.clone(), cx),
                MenuOutcome::Custom(id @ (MENU_EXTRACT_HERE | MENU_EXTRACT_TO | MENU_EXTRACT_SMART)) => {
                    use crate::status_center::ExtractTo;
                    let to = match id {
                        MENU_EXTRACT_HERE => ExtractTo::Here,
                        MENU_EXTRACT_TO => ExtractTo::Named,
                        _ => ExtractTo::Smart,
                    };
                    crate::status_center::extract_to(cx, tag_items.clone(), to);
                }
                MenuOutcome::Custom(MENU_EXTRACT_PICK) => crate::status_center::extract_pick(cx, tag_items.clone()),
                MenuOutcome::Custom(id @ (MENU_ZIP | MENU_7Z)) => {
                    let format = if id == MENU_ZIP { rf_archive::Format::Zip } else { rf_archive::Format::SevenZ };
                    if let Some(first) = tag_items.first() {
                        let dir = first.parent().map(|p| p.to_path_buf()).unwrap_or_default();
                        let stem = if tag_items.len() == 1 {
                            first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Arşiv".into())
                        } else {
                            rf_core::history::display_name(&dir)
                        };
                        let dest = dir.join(format!("{stem}.{}", format.extension()));
                        crate::status_center::compress(cx, tag_items.clone(), dest, format);
                    }
                }
                MenuOutcome::Custom(MENU_TAGS) => v.tag_menu(tag_items.clone(), cx),
                MenuOutcome::Custom(MENU_SHELF) => cx.emit(crate::folder_view::FolderEvent::AddToShelf(tag_items.clone())),
                MenuOutcome::Custom(MENU_OTHER_OPEN) => {
                    cx.emit(crate::folder_view::FolderEvent::OpenInOtherPane(rf_core::Location::Path(target_dir.clone())))
                }
                MenuOutcome::Custom(id @ (MENU_OTHER_COPY | MENU_OTHER_MOVE)) => {
                    let kind = if id == MENU_OTHER_COPY { Transfer::Copy } else { Transfer::Move };
                    cx.emit(crate::folder_view::FolderEvent::ToOtherPane(kind, tag_items.clone()))
                }
                MenuOutcome::Custom(MENU_BULK_RENAME) => cx.emit(crate::folder_view::FolderEvent::BulkRename(tag_items.clone())),
                MenuOutcome::Custom(MENU_OPEN_LOCATION) => {
                    if let Some(p) = tag_items.first() {
                        if let (Some(parent), Some(name)) = (p.parent(), p.file_name()) {
                            v.set_reselect(name.to_string_lossy().into_owned());
                            v.navigate(rf_core::Location::Path(parent.to_path_buf()), cx);
                        }
                    }
                }
                _ => {}
            });
        })
        .detach();
    }

    /// Starts an OLE drag of the selection (to Explorer, the desktop, another tab…).
    pub fn start_drag_out(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        let items = self.selection_paths();
        // Members of an archive: Windows' own ZIP folder would hand them over as virtual files, which our windows (and many programs)
        // cannot take, so a drop on the sidebar did nothing. Up to a size they are extracted first and dragged as real files, as WinRAR
        // and 7-Zip do; dropping them anywhere then copies them there.
        if items.iter().any(|p| crate::archive_view::split(p).is_some() && !p.exists()) {
            let bytes: u64 = self.selected.iter().filter_map(|&i| self.entries.get(i as usize)).map(|e| e.size).sum();
            if bytes <= DRAG_EXTRACT_MAX {
                drag_archive_items_out(items);
                return;
            }
        }
        // An FTP server's items: Windows' FTP folder hands them over as virtual files too (and our own windows take none), so up to
        // the same size they are downloaded first and dragged as real files that may only be copied (issue 223).
        if items.first().is_some_and(|p| rf_win::ftp::is_ftp(p)) {
            let bytes: u64 = self.selected.iter().filter_map(|&i| self.entries.get(i as usize)).map(|e| e.size).sum();
            if bytes <= DRAG_EXTRACT_MAX {
                drag_ftp_items_out(self.ftp_items(&items));
                return;
            }
        }
        drag_paths_out(items);
    }
}

/// [`drag_paths_out`] for items of an FTP server: downloaded on a thread of their own into a temp folder first, then dragged as real
/// files that may only be copied.
fn drag_ftp_items_out(items: Vec<(String, bool)>) {
    let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
    }
    std::thread::spawn(move || {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dest = crate::archive_view::temp_root().join(format!("ftp-{stamp:x}"));
        let real: std::io::Result<Vec<PathBuf>> =
            std::fs::create_dir_all(&dest).and_then(|()| items.iter().map(|(url, is_dir)| rf_win::ftp::download_into(url, *is_dir, &dest)).collect());
        let real = match real {
            Ok(r) => r,
            Err(e) => {
                log::warn!("drag: could not download from the server: {e}");
                return;
            }
        };
        rf_win::sta::run_drag(move |owner| {
            if let Err(e) = rf_win::dnd::drag_out_copy_only(owner, ui_thread, &real) {
                log::warn!("drag: {e:#}");
            }
        });
    });
}

/// The most a drag out of an archive extracts before it starts (the button is held meanwhile); a larger selection is dragged the old
/// way, through Windows' own ZIP folder.
const DRAG_EXTRACT_MAX: u64 = 256 << 20;

/// [`drag_paths_out`] for members of an archive: extracted on a thread of their own first (the window does not wait), then dragged
/// as real files that may only be copied. A button let go before the extraction ended starts no drag (see `rf_win::dnd::drag_out`).
/// What the clipboard held when it had no files: text, or a picture.
enum Pasted {
    Text(String),
    Image(Image),
}

/// Writes the clipboard's text or picture into `dir` as a new file ("Yapıştırılan metin.txt", "Yapıştırılan resim.png"; a picture that
/// came as a bitmap is saved as PNG), then selects it with its rename box open, as a new file is.
fn paste_as_file(what: Pasted, dir: PathBuf, view: WeakEntity<FolderView>, cx: &mut App) {
    let task = cx.background_spawn(async move {
        let (stem, ext, bytes) = match what {
            Pasted::Text(t) => (t!("Yapıştırılan metin"), ".txt".to_string(), t.into_bytes()),
            Pasted::Image(i) => {
                let (ext, bytes) = match i.format {
                    ImageFormat::Png => ("png", i.bytes.clone()),
                    ImageFormat::Jpeg => ("jpg", i.bytes.clone()),
                    ImageFormat::Gif => ("gif", i.bytes.clone()),
                    ImageFormat::Svg => ("svg", i.bytes.clone()),
                    ImageFormat::Webp => ("webp", i.bytes.clone()),
                    // (a bitmap: PNG keeps it whole at a fraction of the size)
                    _ => {
                        let mut png = Vec::new();
                        image::load_from_memory(&i.bytes)?.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)?;
                        ("png", png)
                    }
                };
                (t!("Yapıştırılan resim"), format!(".{ext}"), bytes)
            }
        };
        let path = fileops::unique_name(&dir, stem, &ext);
        std::fs::OpenOptions::new().write(true).create_new(true).open(&path).and_then(|mut f| std::io::Write::write_all(&mut f, &bytes))?;
        anyhow::Ok(path)
    });
    cx.spawn(async move |cx| match task.await {
        Ok(path) => {
            crate::undo::push(crate::undo::UndoOp::Created { path: path.clone() });
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
            let _ = view.update(cx, |v, cx| v.expect_new_item(name, cx));
        }
        Err(e) => log::warn!("paste as a file: {e:#}"),
    })
    .detach();
}

pub(crate) fn drag_archive_items_out(items: Vec<PathBuf>) {
    let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
    unsafe {
        let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
    }
    std::thread::spawn(move || {
        let real = match crate::archive_view::extract_items(&items, &|| false, &mut |_| {}) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("drag: could not extract from the archive: {e:#}");
                return;
            }
        };
        rf_win::sta::run_drag(move |owner| {
            if let Err(e) = rf_win::dnd::drag_out_copy_only(owner, ui_thread, &real) {
                log::warn!("drag: {e:#}");
            }
        });
    });
}

/// An OLE drag of `items` out of the window (the selection, or a recent item of the Home page).
pub(crate) fn drag_paths_out(items: Vec<PathBuf>) {
    {
        if items.is_empty() {
            return;
        }
        let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        rf_win::sta::run_drag(move |owner| {
            if let Err(e) = rf_win::dnd::drag_out(owner, ui_thread, &items) {
                log::warn!("drag: {e:#}");
            }
        });
    }
}

impl FolderView {
    /// Second popup listing tags (● = every selected item has it); picking toggles the tag.
    pub fn tag_menu(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let store = &cx.global::<crate::tags::Tags>().0;
        let items: Vec<ExtraItem> = store
            .defs
            .iter()
            .map(|d| {
                let all = paths.iter().all(|p| store.has(p, d.id));
                ExtraItem { id: MENU_TAG_BASE + d.id, label: format!("{}  {}", if all { "●" } else { "○" }, crate::i18n::td(&d.name)), enabled: true }
            })
            .collect();
        let (tx, rx) = oneshot::channel::<Option<u32>>();
        rf_win::sta::run(move |owner| {
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
            }
            let _ = tx.send(rf_win::ctxmenu::show_simple(owner, (pt.x, pt.y), &items));
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some(id)) = rx.await {
                let _ = this.update(cx, |_, cx| {
                    crate::tags::update(cx, |t| t.toggle(&paths, id - MENU_TAG_BASE));
                    cx.notify();
                });
            }
        })
        .detach();
    }

    pub fn pin(&mut self, dir: PathBuf, cx: &mut Context<Self>) {
        let current: Vec<PathBuf> = self.places_pinned_paths(cx);
        crate::settings::update(cx, |s| {
            let mut v = s.pinned.clone().unwrap_or(current);
            if !v.contains(&dir) {
                v.push(dir);
            }
            s.pinned = Some(v);
        });
        self.refresh_places(cx);
    }

    /// Our Properties window for the selection (or the current folder when nothing is selected).
    pub fn properties(&self, cx: &mut App) {
        let mut paths = self.selection_paths();
        if paths.is_empty() {
            paths.extend(self.current_dir());
        }
        crate::properties_view::open(paths, self.images.clone(), cx);
    }

    /// Opens the selected files with one of the user's own programs for their type (a row of "Open with"; Settings > File types).
    /// A program that cannot be started is told to the user: this was asked for by name.
    pub fn open_selection_in(&mut self, program: String, window: &mut Window, cx: &mut Context<Self>) {
        let files = self.selection_paths();
        if files.is_empty() {
            return;
        }
        let (tx, rx) = oneshot::channel::<Option<String>>();
        let started = program.clone();
        std::thread::spawn(move || {
            let mut failed = None;
            for f in &files {
                if let Err(e) = rf_win::openrules::open_in(&started, f) {
                    log::warn!("open with {started:?}: {e:#}");
                    failed = Some(format!("{e:#}"));
                    break;
                }
            }
            let _ = tx.send(failed);
        });
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Some(why)) = rx.await {
                let _ = cx.update(|window, cx| {
                    crate::prompt::show(
                        format!("{}: {}", t!("Program başlatılamadı"), rf_core::openrules::program_label(&program)),
                        Some(format!("{program}\n{why}")),
                        None,
                        vec![crate::prompt::Button::new(t!("Tamam"), crate::prompt::Kind::Primary, |_, _, _| Ok(()))],
                        window,
                        cx,
                    );
                });
            }
        })
        .detach();
    }

    pub fn open_with(&self) {
        if let Some(p) = self.selection_paths().into_iter().next() {
            rf_win::sta::run(move |_| {
                let _ = rf_win::shell::open_with(&p);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    // not `super::*`: gpui's `test` macro would shadow the built-in one
    use super::{Flattened, fileops, flatten, reflatten, unflatten};
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn tree(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("Gün1/derin")).unwrap();
        std::fs::create_dir_all(root.join("Gün2")).unwrap();
        std::fs::create_dir_all(root.join("bos")).unwrap();
        std::fs::write(root.join("kok.txt"), b"kok").unwrap();
        std::fs::write(root.join("Gün1/a.mp4"), b"a1").unwrap();
        std::fs::write(root.join("Gün1/derin/a.mp4"), b"a2").unwrap();
        std::fs::write(root.join("Gün2/b.mp4"), b"b").unwrap();
    }

    fn listing(root: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap().flatten() {
                let rel = e.path().strip_prefix(root).unwrap().to_string_lossy().into_owned();
                if e.path().is_dir() {
                    stack.push(e.path());
                    out.push((rel, None));
                } else {
                    out.push((rel, Some(std::fs::read(e.path()).unwrap())));
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn flatten_undo_redo() {
        let root = std::env::temp_dir().join(format!("rf-flatten-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        tree(&root);
        let before = listing(&root);
        let t = fileops::Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false)));

        let mut rec = Flattened::default();
        assert!(flatten(&root, &t, &mut rec).unwrap());
        let flat = listing(&root);
        // same-named files don't overwrite each other; every folder is gone
        assert_eq!(flat.iter().filter(|(_, c)| c.is_none()).count(), 0, "{flat:?}");
        assert_eq!(flat.len(), 4, "{flat:?}");
        assert!(root.join("a (2).mp4").exists());
        assert_eq!(rec.moves.len(), 3);
        assert_eq!(rec.removed.len(), 4); // Gün1, Gün1/derin, Gün2, bos

        assert!(unflatten(&rec).unwrap());
        assert_eq!(listing(&root), before); // folders (also the empty one) and files back

        assert!(reflatten(&rec).unwrap());
        assert_eq!(listing(&root), flat);

        // a file deleted after flattening: the rest still goes back, the miss is reported
        std::fs::remove_file(root.join("b.mp4")).unwrap();
        assert!(unflatten(&rec).is_err());
        assert!(root.join("Gün1/derin/a.mp4").exists() && root.join("Gün1/a.mp4").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Copy or move for files dropped from another program (or another of our processes): [`rf_win::dnd::default_transfer`], but a
/// copy when the drag's source does not allow moving (an archive's files extracted for the drag, a phone, a mail's attachment).
/// Moving them anyway took the source's own files away, and Windows refused the drop outright before the GPUI fork chose an effect
/// the source allows: a file dragged from an archive onto the desktop did nothing (2026-11-18).
pub(crate) fn drop_transfer(sources: &[std::path::PathBuf], dest: &std::path::Path, ctrl: bool, shift: bool) -> rf_win::fileops::Transfer {
    use gpui_windows::dnd_hint;
    let kind = rf_win::dnd::default_transfer(sources, dest, ctrl, shift);
    if kind == rf_win::fileops::Transfer::Move && dnd_hint::allowed() & dnd_hint::MOVE == 0 { rf_win::fileops::Transfer::Copy } else { kind }
}

pub(crate) fn drop_on_program(program: PathBuf, files: Vec<PathBuf>, window: &mut Window, cx: &mut App) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
    let ctrl = unsafe { GetAsyncKeyState(VK_CONTROL.0 as i32) < 0 } || gpui_windows::dnd_hint::allowed() & gpui_windows::dnd_hint::MOVE == 0;
    let shift = unsafe { GetAsyncKeyState(VK_SHIFT.0 as i32) < 0 };
    let window = window.window_handle();
    let (tx, rx) = futures::channel::oneshot::channel();
    // COM work must not queue behind the shell STA's own modal drag.
    std::thread::spawn(move || {
        let args = files.iter().map(|p| p.as_os_str().to_owned()).collect();
        let result = crate::program_launch::launch(program, args, true, None);
        let _ = tx.send((files, result));
    });
    cx.spawn(async move |cx| {
        let Ok((files, result)) = rx.await else { return };
        match result {
            Ok(Some(dest)) if !rf_win::dnd::is_invalid_drop(&files, &dest) => {
                let kind = rf_win::dnd::default_transfer(&files, &dest, ctrl, shift);
                cx.update(|cx| crate::status_center::transfer(cx, kind, files, dest));
            }
            Err(err) => {
                let _ = window.update(cx, |_, window, cx| crate::program_launch::launch_error(err, window, cx));
            }
            _ => {}
        }
    }).detach();
}
