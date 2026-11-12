//! What the desktop does to files: open, copy, cut, paste, delete, new, rename, move into a folder. The ENGINES are the file manager's
//! own (`status_center::transfer` / `delete`, `fileops`, `undo`, `rf_win::clipboard`, `rf_win::dnd`, `prompt`): the desktop only
//! decides what to hand them. The file list's methods for the same things are bound to the file list's own selection and entries,
//! which is why this is glue of its own and not a call into them.
//!
//! READ-ONLY. In the read-only preview every operation that would change the Desktop folders is refused with a notice in the status
//! strip: cut, paste, delete, new, rename, moving into a folder (also into a shortcut that points at one), taking files in, and the
//! Properties window's edits (it opens locked). Copying, opening, the menus and handing files to a program work, and so does moving
//! icons to other cells (that changes our own layout file only).
//!
//! THE PUBLIC DESKTOP AND VIRTUAL ITEMS are never changed from here (changing the Public folder asks for administrator rights, which
//! this program never triggers) and nothing is written into a folder of the Public Desktop; they can be opened, copied and moved on
//! the grid. The rules themselves are `super::policy`'s (pure, tested); this file only asks it and carries out the answer.

use std::path::PathBuf;
use std::time::Instant;

use futures::channel::oneshot;
use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::*;
use rf_core::Location;
use rf_core::desktop::items::{ItemId, Origin};
use rf_core::desktop::store::{SortName, SortPref};
use rf_win::fileops::{self, Transfer};

use super::drop::Accepts;
use super::policy::{self, Refusal};
use super::rename_check::{self, Check, Problem};
use super::view::{DesktopView, Msg, Renaming};
use super::{names, source};
use crate::t;

impl DesktopView {
    /// The selected items' ids, in id order.
    pub(super) fn selection_ids(&self) -> Vec<ItemId> {
        self.sel.selected.iter().cloned().collect()
    }

    pub(super) fn path_of(&self, id: &ItemId) -> Option<PathBuf> {
        let it = self.item(id)?;
        Some(self.backend.as_ref()?.dir_of(it.origin)?.join(&it.name))
    }

    /// The paths of the file-system items among `ids`.
    pub(super) fn paths_of(&self, ids: &[ItemId]) -> Vec<PathBuf> {
        ids.iter().filter_map(|i| self.path_of(i)).collect()
    }

    fn public_dir(&self) -> Option<PathBuf> {
        self.backend.as_ref().and_then(|b| b.dir_of(Origin::Public))
    }

    /// Any of `paths` is in the Public Desktop (by components: `Desktop2` is not `Desktop`).
    pub(super) fn under_public(&self, paths: &[PathBuf]) -> bool {
        let Some(public) = self.public_dir() else { return false };
        paths.iter().any(|p| policy::is_under(p, &public))
    }

    /// Where `sources` would go when dropped on the folder item `target`, and what the rules say about it: read-only, the Public
    /// Desktop, copy or move (`policy::into_folder`). One answer for the pointer while dragging and for the drop itself.
    pub(super) fn drop_target_folder(&self, target: &ItemId, sources: &[PathBuf], ctrl: bool, shift: bool) -> Option<(PathBuf, Result<Transfer, Refusal>)> {
        let dest = self.path_of(target)?;
        let default = rf_win::dnd::default_transfer(sources, &dest, ctrl, shift);
        let dest_public = self.public_dir().is_some_and(|p| policy::is_under(&dest, &p));
        let verdict = policy::into_folder(self.read_only(), self.under_public(sources), dest_public, default);
        Some((dest, verdict))
    }

    pub(super) fn say(&mut self, text: &str, cx: &mut Context<Self>) {
        self.notice = Some(text.to_string());
        cx.notify();
    }

    /// Tells the user why something was not done.
    pub(super) fn refuse(&mut self, why: Refusal, cx: &mut Context<Self>) {
        let text = match why {
            Refusal::ReadOnly => t!("Bu işlem salt okunur önizlemede kapalı"),
            Refusal::NotYours => t!("Ortak masaüstü ve sistem öğeleri burada değiştirilmez"),
        };
        self.say(text, cx);
    }

    /// Whether the desktop's files may be changed now; says why not, when not. `ids`: the items the change is for.
    fn may_change(&mut self, ids: &[ItemId], cx: &mut Context<Self>) -> bool {
        // an item the model no longer knows is not ours to change either
        let origins: Vec<Origin> = ids.iter().map(|i| self.item(i).map_or(Origin::Public, |it| it.origin)).collect();
        match policy::change_items(self.read_only(), &origins) {
            Ok(()) => true,
            Err(why) => {
                self.refuse(why, cx);
                false
            }
        }
    }

    // ---------- open ----------

    pub(super) fn open_item(&mut self, id: &ItemId, cx: &mut Context<Self>) {
        let Some(it) = self.item(id).cloned() else { return };
        match it.origin {
            Origin::Virtual(k) => {
                let name = k.parsing_name();
                if let Some(loc) = Location::from_shell_name(&name) {
                    let shell_name = name.clone();
                    cx.defer(move |cx| super::open::open_location(Some(&shell_name), loc, cx));
                } else {
                    rf_win::sta::run(move |_| {
                        if let Err(e) = rf_win::shellfolder::open(&name) {
                            log::warn!("desktop: open {name}: {e:#}");
                        }
                    });
                }
            }
            _ => {
                let Some(path) = self.path_of(id) else { return };
                if it.is_dir {
                    // a folder opens in a rustyfiles window
                    cx.defer(move |cx| super::open::open_location(None, Location::Path(path), cx));
                } else {
                    crate::folder_view::open_in_background(path, cx);
                }
            }
        }
    }

    /// Enter: the item the keyboard is on, or all of the selection. Many items are asked about first, and folders open in a handful of
    /// windows at most (`policy::open_plan`).
    pub(super) fn open_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        match (self.sel.cursor.clone(), ids.len()) {
            (Some(c), 0 | 1) => self.open_item(&c, cx),
            _ => {
                let dirs = ids.iter().filter(|i| self.item(i).is_some_and(|it| it.is_dir)).count();
                if !policy::open_plan(ids.len() - dirs, dirs).confirm {
                    return self.open_ids(ids, cx);
                }
                let me = cx.entity().downgrade();
                let what = format!("{} {}", crate::i18n::items(ids.len()), t!("aynı anda açılacak."));
                crate::prompt::show(
                    t!("Öğeler açılsın mı?"),
                    Some(what),
                    None,
                    vec![
                        crate::prompt::Button::new(t!("Aç"), crate::prompt::Kind::Primary, move |_, _, cx| {
                            let _ = me.update(cx, |v, cx| v.open_ids(ids.clone(), cx));
                            Ok(())
                        }),
                        crate::prompt::Button::cancel(),
                    ],
                    window,
                    cx,
                );
            }
        }
    }

    fn open_ids(&mut self, ids: Vec<ItemId>, cx: &mut Context<Self>) {
        let mut windows = 0;
        for id in &ids {
            if self.item(id).is_some_and(|it| it.is_dir) {
                windows += 1;
                if windows > policy::MAX_FOLDER_WINDOWS {
                    continue;
                }
            }
            self.open_item(id, cx);
        }
        if windows > policy::MAX_FOLDER_WINDOWS {
            self.say(t!("Yalnızca ilk 8 klasör açıldı"), cx);
        }
    }

    /// The Desktop folder itself in a rustyfiles window ("Open in rustyfiles").
    pub(super) fn open_desktop_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) {
            cx.defer(move |cx| super::open::open_location(None, Location::Path(dir), cx));
        }
    }

    /// Alt+Enter. The window can rename and change attributes, so it opens LOCKED (everything shown, nothing applied) whenever
    /// changing these files is not allowed: in the read-only preview, and for Public Desktop items.
    pub(super) fn properties(&mut self, cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        let origins: Vec<Origin> = ids.iter().map(|i| self.item(i).map_or(Origin::Public, |it| it.origin)).collect();
        let locked = policy::change_items(self.read_only(), &origins).is_err();
        let mut paths = self.paths_of(&ids);
        if paths.is_empty() {
            paths.extend(self.backend.as_ref().and_then(|b| b.target_dir()));
        }
        if !paths.is_empty() {
            crate::properties_view::open_with(paths, self.images.clone(), locked, cx);
        }
    }

    /// Space: the previewer (DirectLook), as in the file list.
    pub(super) fn preview_toggle(&mut self, cx: &mut Context<Self>) {
        if !cx.global::<crate::settings::Settings>().directlook_enabled {
            return;
        }
        let target = self.sel.cursor.clone().and_then(|c| self.path_of(&c));
        if let Some(path) = target {
            crate::look::ipc::send(crate::look::ipc::Msg::Toggle(path), true);
        }
    }

    // ---------- clipboard ----------

    pub(super) fn copy_selection(&mut self, kind: Transfer, cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        if ids.is_empty() || (kind == Transfer::Move && !self.may_change(&ids, cx)) {
            return;
        }
        let paths = self.paths_of(&ids);
        if paths.is_empty() {
            return;
        }
        // OpenClipboard can wait on other apps: never on the UI thread
        rf_win::sta::run(move |owner| {
            if let Err(e) = rf_win::clipboard::set_files(owner, &paths, kind) {
                log::warn!("desktop clipboard: {e:#}");
            }
        });
    }

    pub(super) fn copy_paths_as_text(&mut self, cx: &mut Context<Self>) {
        let paths = self.paths_of(&self.selection_ids());
        if !paths.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join("\r\n")));
        }
    }

    pub(super) fn paste(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Err(why) = policy::add_to_desktop(self.read_only()) {
            return self.refuse(why, cx);
        }
        let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) else { return };
        // GetClipboardData can block on a slow source app
        let (tx, rx) = oneshot::channel();
        rf_win::sta::run(move |owner| {
            let _ = tx.send(rf_win::clipboard::get_files(owner).map_err(|e| log::warn!("desktop paste: {e:#}")).ok().flatten());
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Some((paths, kind))) = rx.await {
                cx.update(|cx| {
                    crate::status_center::transfer_then(cx, kind, paths, dir, move |arrived, cx| select_arrived(&this, arrived, cx));
                });
            }
        })
        .detach();
    }

    /// Files from other programs dropped on an empty place: into the Desktop folder, copied or moved by the usual rules.
    pub(super) fn take_in(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if let Err(why) = policy::add_to_desktop(self.read_only()) {
            return self.refuse(why, cx);
        }
        let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) else { return };
        if paths.is_empty() || rf_win::dnd::is_invalid_drop(&paths, &dir) {
            return;
        }
        let m = window.modifiers();
        let kind = rf_win::dnd::default_transfer(&paths, &dir, m.control, m.shift);
        let this = cx.entity().downgrade();
        crate::status_center::transfer_then(cx, kind, paths, dir, move |arrived, cx| select_arrived(&this, arrived, cx));
    }

    // ---------- delete ----------

    pub(super) fn delete_selection(&mut self, permanent: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.selection_ids();
        if ids.is_empty() || self.renaming.is_some() || !self.may_change(&ids, cx) {
            return;
        }
        let paths = self.paths_of(&ids);
        self.delete_paths(paths, permanent, window, cx);
    }

    /// Asks first, per Settings -> "Confirm before deleting", then hands the paths to the Status Center's delete (which records the
    /// undo).
    fn delete_paths(&mut self, paths: Vec<PathBuf>, permanent: bool, window: &mut Window, cx: &mut Context<Self>) {
        use crate::settings::DeletePref;
        if paths.is_empty() {
            return;
        }
        if self.under_public(&paths) {
            self.say(t!("Ortak masaüstü ve sistem öğeleri burada değiştirilmez"), cx);
            return;
        }
        let ask = match cx.global::<crate::settings::Settings>().confirm_delete {
            DeletePref::Always => true,
            DeletePref::PermanentOnly => permanent,
            DeletePref::Never => false,
        };
        if !ask {
            crate::status_center::delete(cx, paths, permanent);
            return;
        }
        let what = if paths.len() == 1 { format!("\"{}\"", rf_core::history::display_name(&paths[0])) } else { crate::i18n::items(paths.len()) };
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
                    crate::status_center::delete(cx, paths.clone(), permanent);
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    // ---------- new ----------

    pub(super) fn new_item(&mut self, folder: bool, cx: &mut Context<Self>) {
        if let Err(why) = policy::add_to_desktop(self.read_only()) {
            return self.refuse(why, cx);
        }
        let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) else { return };
        // (named in the interface language, as in a folder window)
        let name = if folder { t!("Yeni klasör") } else { t!("Yeni metin belgesi") }.to_string();
        let task = cx.background_spawn(async move { if folder { fileops::new_folder(&dir, &name) } else { fileops::new_text_file(&dir, &name) } });
        cx.spawn(async move |this, cx| {
            match task.await {
                Ok(path) => {
                    crate::undo::push(crate::undo::UndoOp::Created { path: path.clone() });
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned());
                    // its rename box opens as soon as the folder watcher has told the model about it
                    let _ = this.update(cx, |v, cx| {
                        v.want_rename = name.map(|n| (ItemId::user(&n), Instant::now()));
                        cx.notify();
                    });
                }
                Err(e) => {
                    log::warn!("desktop new item: {e:#}");
                    let text = format!("{}: {e:#}", t!("Yeni öğe oluşturulamadı"));
                    let _ = this.update(cx, |v, cx| v.say(&text, cx));
                }
            }
        })
        .detach();
    }

    // ---------- rename ----------

    pub(super) fn begin_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        let id = self.sel.cursor.clone().filter(|c| self.sel.is_selected(c)).or_else(|| self.sel.selected.iter().next().cloned());
        if let Some(id) = id {
            self.begin_rename_of(&id, window, cx);
        }
    }

    pub(super) fn begin_rename_of(&mut self, id: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(it) = self.item(id).cloned() else { return };
        if !self.may_change(std::slice::from_ref(id), cx) {
            return;
        }
        let shown = cx.global::<crate::settings::Settings>().show_extensions && !names::extension_never_shown(&it.name);
        let (text, select_to) = rename_check::edit_text(&it.name, it.is_dir, shown);
        let hidden_ext = rename_check::hidden_extension(&it.name, it.is_dir, shown);
        let input = cx.new(|cx| InputState::new(window, cx).default_value(text));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_rename(window, cx);
            }
        });
        input.update(cx, |s, cx| {
            s.focus(window, cx);
            s.set_selected_range(0..select_to, cx);
        });
        // a focus target that is not part of the frame being drawn is dropped by GPUI: focus once more on the next frame
        cx.on_next_frame(window, |this, window, cx| {
            if let Some(r) = &this.renaming {
                r.input.update(cx, |s, cx| s.focus(window, cx));
            }
        });
        self.renaming = Some(Renaming { id: id.clone(), input, old: it.name, is_dir: it.is_dir, hidden_ext });
        self.rename_sub = Some(sub);
        cx.notify();
    }

    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming = None;
        self.rename_sub = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(super) fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.renaming.take() else { return };
        self.rename_sub = None;
        window.focus(&self.focus, cx);
        let typed = r.input.read(cx).value().to_string();
        let siblings: Vec<String> = self.items.iter().filter(|i| i.origin == Origin::User && i.id != r.id).map(|i| i.name.clone()).collect();
        let refs: Vec<&str> = siblings.iter().map(String::as_str).collect();
        let alert = |text: String| {
            let title = t!("Yeniden adlandır").to_string();
            rf_win::sta::run(move |owner| rf_win::shell::alert(owner, &title, &text));
        };
        match rename_check::check(&r.old, &typed, r.hidden_ext.as_deref(), &refs) {
            Check::Unchanged => {}
            Check::Refuse(problem) => {
                let why = match problem {
                    Problem::Empty => return cx.notify(),
                    Problem::Characters => t!("Dosya adı şu karakterleri içeremez: \\ / : * ? \" < > | ve nokta ile bitemez."),
                    Problem::Reserved => t!("Bu ad Windows tarafından ayrılmıştır; başka bir ad deneyin."),
                    Problem::TooLong => t!("Ad çok uzun (en çok 255 karakter)."),
                    Problem::Taken => t!("Bu adda bir öğe zaten var. Başka bir ad deneyin."),
                };
                alert(format!("{}\n\n{why}", typed.trim()));
            }
            Check::Rename(new_name) => self.do_rename(r, new_name, cx),
        }
        cx.notify();
    }

    fn do_rename(&mut self, r: Renaming, new_name: String, cx: &mut Context<Self>) {
        let Some(dir) = self.backend.as_ref().and_then(|b| b.dir_of(Origin::User)) else { return };
        let warn = cx.global::<crate::settings::Settings>().warn_extension_change && rename_check::changes_extension(&r.old, &new_name, r.is_dir);
        // Explorer-style warning when a file's extension changes
        let confirm = warn.then(|| {
            let (tx, rx) = oneshot::channel::<bool>();
            let title = t!("Dosya adı uzantısı").to_string();
            let msg = t!("Dosya adı uzantısını değiştirirseniz dosya kullanılamaz hale gelebilir. Değiştirmek istediğinizden emin misiniz?").to_string();
            rf_win::sta::run(move |owner| {
                let _ = tx.send(rf_win::shell::confirm(owner, &title, &msg));
            });
            rx
        });
        let (from, to) = (dir.join(&r.old), dir.join(&new_name));
        let (old_id, new_id) = (ItemId::user(&r.old), ItemId::user(&new_name));
        cx.spawn(async move |this, cx| {
            if let Some(rx) = confirm
                && !rx.await.unwrap_or(false)
            {
                return;
            }
            // the shell's rename engine on a thread of its own, as the file list does it (not on a pool thread that other work shares)
            let (f2, n2) = (from.clone(), new_name.clone());
            let (tx, rx) = oneshot::channel::<anyhow::Result<bool>>();
            let spawned = std::thread::Builder::new().name("rf-op-rename".into()).spawn(move || {
                let _ = tx.send(fileops::rename(&f2, &n2));
            });
            if spawned.is_err() {
                return;
            }
            match rx.await.unwrap_or(Ok(false)) {
                Ok(true) => {
                    crate::undo::push(crate::undo::UndoOp::Renamed { from: from.clone(), to: to.clone() });
                    cx.update(|cx| crate::tags::update(cx, |t| t.moved(&from, &to)));
                    // the cell goes with the item; the folder watcher shows the new name a moment later
                    let _ = this.update(cx, |v, cx| {
                        v.store.edit().rename(&old_id, &new_id);
                        v.sel.renamed(&old_id, &new_id);
                        cx.notify();
                    });
                }
                // (false: the shell asked the user something and the answer was no)
                Ok(false) => {}
                Err(e) => {
                    log::warn!("desktop rename: {e:#}");
                    let text = format!("{}: {e:#}", t!("Yeniden adlandırılamadı"));
                    let _ = this.update(cx, |v, cx| v.say(&text, cx));
                }
            }
        })
        .detach();
    }

    /// The shell's own menu asked for a rename.
    pub(super) fn rename_from_menu(&mut self, cx: &mut Context<Self>) {
        self.want_rename = self.sel.cursor.clone().map(|c| (c, Instant::now()));
        cx.notify();
    }

    // ---------- icons: move, hand over ----------

    /// Moves the icons by `delta` canvas pixels: each lands in the cell its own top-left corner is nearest to (so the group keeps its
    /// shape), a taken cell sends the icon to the nearest free one. Only our layout file changes.
    pub(super) fn move_icons(&mut self, ids: &[ItemId], delta: (i32, i32), cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        if delta == (0, 0) {
            return;
        }
        self.touch_layout();
        let from: Vec<(ItemId, rf_core::desktop::geom::Rect)> = ids.iter().filter_map(|i| self.cell_rect_of(i).map(|r| (i.clone(), r))).collect();
        // the moving icons give up their cells first, so that they can take each other's
        for (id, _) in &from {
            self.resolved.placed.remove(id);
        }
        for (id, r) in &from {
            self.store.edit().drop_item(&b.specs, &mut self.resolved, id, b.canvas, r.left + delta.0, r.top + delta.1);
        }
        self.rebuild_placed();
        self.schedule_save(cx);
        cx.notify();
    }

    /// Files dropped on an item that takes them: a folder, a program (or shortcut), the Recycle Bin.
    pub(super) fn hand_to(&mut self, target: &ItemId, accepts: Accepts, sources: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        if sources.is_empty() {
            return;
        }
        match accepts {
            Accepts::Folder => {
                let m = window.modifiers();
                let Some((dest, verdict)) = self.drop_target_folder(target, &sources, m.control, m.shift) else { return };
                if rf_win::dnd::is_invalid_drop(&sources, &dest) {
                    return;
                }
                // read-only: no; into the Public Desktop: no; the Public Desktop's own files are copied, never moved out
                match verdict {
                    Ok(kind) => crate::status_center::transfer(cx, kind, sources, dest),
                    Err(why) => self.refuse(why, cx),
                }
            }
            Accepts::RecycleBin => {
                if let Err(why) = policy::change_items(self.read_only(), &[]) {
                    return self.refuse(why, cx);
                }
                self.delete_paths(sources, false, window, cx);
            }
            Accepts::Program => {
                if let Some(program) = self.path_of(target) {
                    // a shortcut that turns out to point at a folder is such a folder: it follows the same rules as one
                    let m = window.modifiers();
                    let rules = FolderRules { read_only: self.read_only(), public_dir: self.public_dir(), sources_public: self.under_public(&sources), ctrl: m.control, shift: m.shift };
                    give_to_program(program, sources, rules, cx);
                }
            }
            Accepts::Nothing => {}
        }
    }

    // ---------- the rest ----------

    pub(super) fn step_icon_size(&mut self, larger: bool, cx: &mut Context<Self>) {
        let now = self.store.get().icon_size_step();
        let next = if larger { now.larger() } else { now.smaller() };
        if next != now {
            self.set_icon_size(next, cx);
        }
    }

    pub(super) fn set_icon_size(&mut self, size: rf_core::desktop::grid::IconSize, cx: &mut Context<Self>) {
        self.touch_layout();
        self.store.edit().set_icon_size(size);
        self.recompute(cx);
        self.refresh_peers(cx);
    }

    pub(super) fn set_align(&mut self, on: bool, cx: &mut Context<Self>) {
        self.touch_layout();
        self.store.edit().set_align_to_grid(on);
        self.recompute(cx);
        self.refresh_peers(cx);
    }

    /// "Sort by": the icons are laid out once in that order from the top-left and stay where they are put from then on.
    pub(super) fn sort_by(&mut self, key: SortName, descending: bool, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        self.touch_layout();
        let pref = SortPref { key, descending };
        self.store.edit().set_sort(pref);
        let mut items = b.items.items();
        rf_core::desktop::items::sort_items(&mut items, key.key(), pref.dir());
        let shown: Vec<ItemId> = items.iter().map(|i| i.id.clone()).collect();
        let model = b.items.listing();
        self.store.edit().set_auto_arrange(true);
        let resolved = self.store.edit().resolve(&b.specs, rf_core::desktop::layout::Listing { shown: &shown, kept: &model.kept, complete: model.complete }, chrono::Utc::now().timestamp_millis());
        self.store.edit().freeze(&b.specs, &resolved);
        self.recompute(cx);
        self.refresh_peers(cx);
    }

    /// F5: lists the folders and the wallpaper again (on a thread of its own: the shell may take its time).
    pub(super) fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            b.items.rescan();
            b.items.refresh_virtuals();
            let _ = tx.unbounded_send(Msg::Wallpaper(Box::new(source::read_wallpaper(&b.infos))));
            let _ = tx.unbounded_send(Msg::Changed(Vec::new()));
        });
        cx.notify();
    }
}

/// Selects what arrived (a paste, a drop) once the model shows it.
fn select_arrived(this: &WeakEntity<DesktopView>, arrived: Vec<PathBuf>, cx: &mut App) {
    let ids: Vec<ItemId> = arrived.iter().filter_map(|p| p.file_name()).map(|n| ItemId::user(&n.to_string_lossy())).collect();
    let _ = this.update(cx, |v, cx| {
        v.reselect = Some((ids, Instant::now()));
        v.recompute(cx);
    });
}

/// What a shortcut that points at a FOLDER needs to know to treat the files like any folder of the desktop (`policy::into_folder`).
struct FolderRules {
    read_only: bool,
    public_dir: Option<PathBuf>,
    sources_public: bool,
    ctrl: bool,
    shift: bool,
}

/// A program (or a shortcut to one) is started with the files as its arguments. A shortcut to a folder takes them like a folder, by
/// the same rules (`rules`): never in the read-only preview, never into the Public Desktop, copied out of the Public Desktop. Never
/// elevated: a program that asks for administrator rights is not started from here.
fn give_to_program(program: PathBuf, files: Vec<PathBuf>, rules: FolderRules, cx: &mut Context<DesktopView>) {
    enum Outcome {
        Started,
        IntoFolder(PathBuf),
        Failed(String),
    }
    let files_for_folder = files.clone();
    let (tx, rx) = oneshot::channel::<Outcome>();
    let job = move || {
        use std::os::windows::process::CommandExt;
        let is_link = program.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk"));
        let (exe, extra, cwd) = if is_link {
            // reading a shortcut is shell work: on the shell thread, never on a worker
            let link = program.clone();
            match rf_win::sta::call(std::time::Duration::from_secs(5), move || rf_win::shortcut::resolve(&link).map_err(|e| format!("{e:#}"))) {
                Some(Ok(info)) if info.target.is_dir() => return Outcome::IntoFolder(info.target),
                Some(Ok(info)) if !info.target.as_os_str().is_empty() => (info.target, info.args, info.working_dir),
                Some(Ok(_)) => return Outcome::Failed("the shortcut does not point at a program".into()),
                Some(Err(e)) => return Outcome::Failed(e),
                None => return Outcome::Failed("the shell did not answer".into()),
            }
        } else {
            let cwd = program.parent().map(PathBuf::from).unwrap_or_default();
            (program, String::new(), cwd)
        };
        let mut cmd = std::process::Command::new(&exe);
        if !extra.trim().is_empty() {
            cmd.raw_arg(extra);
        }
        cmd.args(&files);
        if cwd.is_dir() {
            cmd.current_dir(cwd);
        }
        match cmd.spawn() {
            Ok(_) => Outcome::Started,
            Err(e) => Outcome::Failed(e.to_string()),
        }
    };
    if std::thread::Builder::new().name("rf-op-hand-to-program".into()).spawn(move || {
        let _ = tx.send(job());
    })
    .is_err()
    {
        return;
    }
    cx.spawn(async move |this, cx| match rx.await.unwrap_or(Outcome::Failed("cancelled".into())) {
        Outcome::Started => {}
        Outcome::IntoFolder(dest) => {
            if rf_win::dnd::is_invalid_drop(&files_for_folder, &dest) {
                return;
            }
            let default = rf_win::dnd::default_transfer(&files_for_folder, &dest, rules.ctrl, rules.shift);
            let dest_public = rules.public_dir.as_deref().is_some_and(|p| policy::is_under(&dest, p));
            match policy::into_folder(rules.read_only, rules.sources_public, dest_public, default) {
                Ok(kind) => {
                    cx.update(|cx| crate::status_center::transfer(cx, kind, files_for_folder, dest));
                }
                Err(why) => {
                    let _ = this.update(cx, |v, cx| v.refuse(why, cx));
                }
            }
        }
        Outcome::Failed(why) => {
            log::warn!("desktop: could not hand the files to the program: {why}");
            let _ = this.update(cx, |v, cx| v.say(t!("Program başlatılamadı. Yönetici izni gerekiyor olabilir."), cx));
        }
    })
    .detach();
}
