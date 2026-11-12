//! The desktop's commands that are not file operations proper: the system icons (open, Properties, Empty Recycle Bin), shortcuts,
//! Open with, the arrangement switches (auto arrange, show desktop icons), and the shortcuts to Windows' own pages. Like
//! `operations.rs`, the ENGINES are the app's own (`rf_win::recycle`, `rf_win::shortcut`, `crate::undo`, `status_center`); this file
//! decides what to hand them and what the view does afterwards.
//!
//! The only rule in this file that is not a call is the Recycle Bin's job ([`empty_bin_job`]): the emptying and then, whatever came
//! of it, a fresh look at the bin so that its icon is right. It runs over a trait so that it can be tried without touching a bin.

use std::path::{Path, PathBuf};
use std::time::Instant;

use futures::channel::oneshot;
use gpui_kit::*;
use rf_core::Location;
use rf_core::desktop::items::{ItemId, Origin};
use rf_core::desktop::virtuals::VirtualKind;

use super::links::{self, Style};
use super::menupoint::Via;
use super::menu_plan::{Kind, virtual_has_properties};
use super::policy;
use super::view::{DesktopView, Near};
use crate::t;

/// What the Recycle Bin's commands need of the system.
pub(super) trait BinOps: Send + Sync {
    /// Empties the bin on every drive. Windows asks the user first; a "No" is an `Err` here and not a fault.
    fn empty(&self) -> Result<(), String>;
}

/// The real bin: the call the Recycle Bin page's "Kutuyu boşalt" makes, with Windows' own confirmation.
pub(super) struct SystemBin;

impl BinOps for SystemBin {
    fn empty(&self) -> Result<(), String> {
        rf_win::shell::ensure_com();
        rf_win::recycle::empty(true).map_err(|e| format!("{e:#}"))
    }
}

/// Empties the bin, then asks for its state to be read again (`reread`), whether or not it was emptied: the user may have said no, and
/// what is in it is the icon's business either way. Returns what the emptying said.
pub(super) fn empty_bin_job(ops: &dyn BinOps, reread: &dyn Fn()) -> Result<(), String> {
    let r = ops.empty();
    reread();
    r
}

/// Whether the Recycle Bin's state is read again now: a file operation (a delete into the bin, a move out of it) has just ended while
/// the bin's icon is on the desktop. The folder watch sees most of it; this also covers a drive whose bin folder is created by this
/// very delete, which no watch could have been placed on.
pub(super) fn reread_bin_after_ops(was_busy: bool, busy: bool, bin_on_desktop: bool) -> bool {
    was_busy && !busy && bin_on_desktop
}

impl DesktopView {
    /// This desktop shows a scratch folder (a test copy), not the real Desktop.
    pub(super) fn is_scratch(&self) -> bool {
        self.backend.as_ref().is_some_and(|b| b.is_scratch())
    }

    /// The Recycle Bin's icon is on this desktop.
    pub(super) fn shows_bin(&self) -> bool {
        self.items.iter().any(|it| matches!(it.origin, Origin::Virtual(VirtualKind::RecycleBin)))
    }

    /// The Status Center's busy state changed (see `view::on_ops`): the bin's icon is looked at again when work has just ended.
    pub(super) fn ops_busy_changed(&mut self, busy: bool) {
        let go = reread_bin_after_ops(self.ops_busy, busy, self.shows_bin());
        self.ops_busy = busy;
        if go {
            self.reread_system_icons();
        }
    }

    /// What kind each of `ids` is, for the menu plan.
    pub(super) fn kinds_of(&self, ids: &[ItemId]) -> Vec<Kind> {
        ids.iter()
            .filter_map(|i| self.item(i))
            .map(|it| match it.origin {
                Origin::Virtual(k) => Kind::Virtual(k),
                o => Kind::File { dir: it.is_dir, origin: o },
            })
            .collect()
    }

    /// Items that arrived by the user's hand: selected once the model shows them, and moved next to `at` when it is known.
    pub(super) fn arrive(&mut self, ids: Vec<ItemId>, at: Option<(i32, i32)>, cx: &mut Context<Self>) {
        if let Some(at) = at {
            self.near = Some(Near { ids: ids.clone(), at, since: Instant::now() });
        }
        self.reselect = Some((ids, Instant::now()));
        self.recompute(cx);
    }

    // ---------- the system icons ----------

    /// A double click or Enter on a system icon. This PC and the Recycle Bin open in our own views; the user's folder opens as the folder
    /// it is; Network opens in our window when we can list it ourselves; the rest (Control Panel) is the shell's own, as in Explorer.
    pub(super) fn open_virtual(&mut self, k: VirtualKind, cx: &mut Context<Self>) {
        let name = k.parsing_name();
        if let Some(loc) = Location::from_shell_name(&name) {
            cx.defer(move |cx| super::open::open_location(Some(&name), loc, cx));
            return;
        }
        // whether we can list it is a question for the shell
        let (tx, rx) = oneshot::channel();
        let asked = name.clone();
        rf_win::sta::run(move |_| {
            let _ = tx.send(rf_win::shellfolder::browsable(&asked));
        });
        cx.spawn(async move |_, cx| {
            let place = rx.await.ok().flatten();
            cx.update(|cx| match place {
                Some(p) => {
                    let loc = match p.fs_path {
                        Some(path) => Location::Path(path),
                        None => Location::Shell { parsing: p.parsing, name: p.name },
                    };
                    super::open::open_location(Some(&name), loc, cx);
                }
                None => rf_win::sta::run(move |_| {
                    if let Err(e) = rf_win::shellfolder::open(&name) {
                        log::warn!("desktop: open {name}: {e:#}");
                    }
                }),
            });
        })
        .detach();
    }

    /// Properties of one system icon: Windows' own window for This PC, the Recycle Bin and Network; the user's folder in ours.
    pub(super) fn virtual_properties(&mut self, k: VirtualKind, cx: &mut Context<Self>) {
        if !virtual_has_properties(k) {
            return;
        }
        if k == VirtualKind::UserFiles {
            // (the profile folder is asked of the shell, which is not done on this thread)
            let (tx, rx) = oneshot::channel();
            rf_win::sta::run(move |_| {
                let _ = tx.send(rf_win::desktop::items::profile_dir());
            });
            let images = self.images.clone();
            cx.spawn(async move |_, cx| {
                if let Ok(Some(dir)) = rx.await {
                    // (always locked: the user's profile folder is not a thing to rename or re-attribute from the desktop)
                    cx.update(|cx| crate::properties_view::open_with(vec![dir], images, true, cx));
                }
            })
            .detach();
            return;
        }
        let name = k.parsing_name();
        rf_win::sta::run(move |_| {
            if let Err(e) = rf_win::shell::properties(Path::new(&name)) {
                log::warn!("desktop: properties of {name}: {e:#}");
            }
        });
    }

    /// "Geri Dönüşüm Kutusu'nu boşalt": Windows asks, then the bin is emptied and its icon follows.
    pub(super) fn empty_bin(&mut self, cx: &mut Context<Self>) {
        // (the protected preview changes nothing, and a scratch desktop's bin icon is the user's real bin)
        if let Err(why) = policy::empty_bin(self.read_only(), self.is_scratch()) {
            return self.refuse(why, cx);
        }
        let Some(b) = self.backend.clone() else { return };
        let spawned = std::thread::Builder::new().name("rf-op-empty-bin".into()).spawn(move || {
            if let Err(e) = empty_bin_job(&SystemBin, &|| b.items.refresh_virtuals()) {
                // (a "No" to Windows' question comes back here too)
                log::info!("desktop: the Recycle Bin was not emptied: {e}");
            }
        });
        if let Err(e) = spawned {
            log::warn!("desktop: could not start emptying the Recycle Bin: {e}");
        }
    }

    // ---------- shortcuts, Open with ----------

    /// "Kısayol oluştur": a shortcut to each selected file or folder, on the user's own Desktop.
    pub(super) fn create_shortcuts(&mut self, cx: &mut Context<Self>) {
        if let Err(why) = policy::add_to_desktop(self.read_only()) {
            return self.refuse(why, cx);
        }
        let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) else { return };
        let targets = self.paths_of(&self.selection_ids());
        if !targets.is_empty() {
            self.make_links(targets, dir, Style::Created, None, cx);
        }
    }

    /// "Kısayol yapıştır": a shortcut to each file on the clipboard, here.
    pub(super) fn paste_shortcuts(&mut self, cx: &mut Context<Self>) {
        if let Err(why) = policy::add_to_desktop(self.read_only()) {
            return self.refuse(why, cx);
        }
        let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) else { return };
        let at = self.take_menu_point(Via::Menu);
        // GetClipboardData can block on a slow source app
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::clipboard::get_files(owner).map_err(|e| log::warn!("desktop paste shortcut: {e:#}")).ok().flatten());
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some((paths, _))) = rx.await {
                let _ = this.update(cx, |v, cx| v.make_links(paths, dir, Style::Pasted, at, cx));
            }
        })
        .detach();
    }

    fn make_links(&mut self, targets: Vec<PathBuf>, dir: PathBuf, style: Style, at: Option<(i32, i32)>, cx: &mut Context<Self>) {
        let (tx, rx) = oneshot::channel::<Vec<PathBuf>>();
        let word = t!("Kısayol").to_string();
        let spawned = std::thread::Builder::new().name("rf-op-shortcuts".into()).spawn(move || {
            rf_win::shell::ensure_com();
            let mut made = Vec::new();
            for target in targets {
                let Some(name) = target.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
                let file = links::unique(&links::base_name(&name, style, &word), |n| dir.join(n).exists());
                let link = dir.join(file);
                match rf_win::shortcut::create(&link, &target) {
                    Ok(()) => made.push(link),
                    Err(e) => log::warn!("desktop shortcut {link:?}: {e:#}"),
                }
            }
            let _ = tx.send(made);
        });
        if spawned.is_err() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let made = rx.await.unwrap_or_default();
            for p in &made {
                crate::undo::push(crate::undo::UndoOp::Created { path: p.clone() });
            }
            let ids: Vec<ItemId> = made.iter().filter_map(|p| p.file_name()).map(|n| ItemId::user(&n.to_string_lossy())).collect();
            if !ids.is_empty() {
                let _ = this.update(cx, |v, cx| v.arrive(ids, at, cx));
            }
        })
        .detach();
    }

    /// "Birlikte aç…": the shell's chooser for the one selected file.
    pub(super) fn open_with_selection(&mut self, _cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        let [id] = ids.as_slice() else { return };
        if self.item(id).is_none_or(|it| it.is_dir || it.is_virtual()) {
            return;
        }
        if let Some(path) = self.path_of(id) {
            rf_win::sta::run(move |_| {
                if let Err(e) = rf_win::shell::open_with(&path) {
                    log::warn!("desktop open with: {e:#}");
                }
            });
        }
    }

    // ---------- the arrangement ----------

    /// Auto arrange on or off. On: the icons are laid out in the sort order and stay so (an icon put down elsewhere goes back). Off: the
    /// arrangement on the screen becomes the stored positions, so nothing jumps.
    pub(super) fn set_auto_arrange_mode(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        if self.store.get().auto_arrange == on {
            return;
        }
        self.touch_layout();
        if on {
            self.store.edit().set_auto_arrange(true);
        } else {
            let resolved = self.resolved.clone();
            self.store.edit().freeze(&b.specs, &resolved);
        }
        self.recompute(cx);
        self.refresh_peers(cx);
    }

    /// "Masaüstü simgelerini göster": the menu's own switch (kept in the layout file); the settings' choice can still hide them.
    pub(super) fn set_show_icons(&mut self, on: bool, cx: &mut Context<Self>) {
        self.touch_layout();
        self.store.edit().set_show_icons(on);
        super::choices::set_menu_hidden(!on);
        // the item source reads its answer again (on a thread of its own: the Recycle Bin's state is asked of the shell)
        self.reread_system_icons();
        self.schedule_save(cx);
        cx.notify();
    }

    // ---------- places outside the desktop ----------

    pub(super) fn open_terminal_here(&mut self) {
        if let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) {
            let spawned = std::thread::Builder::new().name("rf-op-terminal".into()).spawn(move || {
                if let Err(e) = rf_win::shell::open_terminal(&dir) {
                    log::warn!("desktop: terminal: {e:#}");
                }
            });
            if let Err(e) = spawned {
                log::warn!("desktop: could not start a terminal: {e}");
            }
        }
    }

    /// A page of Windows' Settings app (`ms-settings:...`).
    pub(super) fn open_settings_page(&mut self, uri: &'static str) {
        rf_win::sta::run(move |_| {
            if let Err(e) = rf_win::shell::open(Path::new(uri)) {
                log::warn!("desktop: {uri}: {e:#}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    // (explicit names: `use super::*` would bring in the toolkit's own `test` attribute through its glob and break `#[test]`)
    use super::{BinOps, empty_bin_job, reread_bin_after_ops};
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct FakeBin {
        answer: Result<(), String>,
        emptied: AtomicUsize,
    }
    impl BinOps for FakeBin {
        fn empty(&self) -> Result<(), String> {
            self.emptied.fetch_add(1, Ordering::SeqCst);
            self.answer.clone()
        }
    }

    #[test]
    fn the_bin_is_read_again_when_work_ends_and_only_then() {
        // busy -> idle with the bin on the desktop: yes
        assert!(reread_bin_after_ops(true, false, true));
        // still running, starting, idle all along: no (progress ticks come many times a second)
        assert!(!reread_bin_after_ops(true, true, true));
        assert!(!reread_bin_after_ops(false, true, true));
        assert!(!reread_bin_after_ops(false, false, true));
        // no bin icon, nothing to read
        assert!(!reread_bin_after_ops(true, false, false));
    }

    #[test]
    fn the_bin_is_looked_at_again_after_it_was_emptied() {
        let bin = FakeBin { answer: Ok(()), emptied: AtomicUsize::new(0) };
        let looked = AtomicUsize::new(0);
        let r = empty_bin_job(&bin, &|| {
            // (by the time the state is read, the emptying has happened)
            assert_eq!(bin.emptied.load(Ordering::SeqCst), 1);
            looked.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(r, Ok(()));
        assert_eq!((bin.emptied.load(Ordering::SeqCst), looked.load(Ordering::SeqCst)), (1, 1));
    }

    #[test]
    fn a_no_to_windows_question_or_a_failure_still_refreshes_the_icon_and_is_reported() {
        let bin = FakeBin { answer: Err("cancelled".into()), emptied: AtomicUsize::new(0) };
        let looked = AtomicUsize::new(0);
        let r = empty_bin_job(&bin, &|| {
            looked.fetch_add(1, Ordering::SeqCst);
        });
        assert_eq!(r, Err("cancelled".to_string()));
        assert_eq!(looked.load(Ordering::SeqCst), 1);
    }
}
