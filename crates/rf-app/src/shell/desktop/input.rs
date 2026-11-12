//! The mouse and the keyboard of the desktop view. The decisions are the pure modules' (`select`, `typeahead`, `drop`); this file
//! feeds them events and carries out what they decide.
//!
//! KEYS. The view has the file list's key context, so the file list's own actions arrive here (the arrows, Home / End, Enter, F2,
//! Delete, Ctrl+A / C / X / V / Z / Y, Shift+F10 and the menu key, F5, Alt+Enter, Escape), and so do the user's own bindings from
//! `settings.toml`. Shift+Left / Right are the desktop's own (`super::ExtendLeft`).
//!
//! DRAGS. A press on an icon followed by a movement of a few pixels drags the selected icons. Inside the window that is our own
//! gesture (the icons follow the pointer, the cell they would land in is outlined, a folder, program or the Recycle Bin under the
//! pointer lights up). When the pointer leaves the window with files in the drag, the gesture becomes the system's OLE drag, as in the
//! file list, so that other programs can take the files. The window listens to the mouse itself while a drag or a rubber band is on,
//! because the window holds the mouse from the press and the pointer may be anywhere.

use std::path::PathBuf;

use gpui_kit::*;
use rf_core::desktop::items::{ItemId, Origin};

use super::drop::{self, Accepts, DropAction, Effect, Over, Where, Why};
use super::menupoint::Via;
use super::policy::Refusal;
use super::select::{Dir, Mods, rect_of};
use super::view::{Band, DesktopView, Drag, Press};
use super::{ExtendEnd, ExtendHome, ExtendLeft, ExtendRight, IconsLarge, IconsMedium, IconsSmall};
use crate::actions::*;

/// How far a press must move before it is a drag (the file list uses the same).
const DRAG_START: f32 = 6.0;

impl DesktopView {
    /// Hooks every action and key of the view up to `root`.
    pub(super) fn bind_actions(&mut self, root: Stateful<Div>, cx: &mut Context<Self>) -> Stateful<Div> {
        root.on_action(cx.listener(|this, _: &SelectNext, _, cx| this.key_move(Dir::Down, false, cx)))
            .on_action(cx.listener(|this, _: &SelectPrev, _, cx| this.key_move(Dir::Up, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLeft, _, cx| this.key_move(Dir::Left, false, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _, cx| this.key_move(Dir::Right, false, cx)))
            .on_action(cx.listener(|this, _: &SelectFirst, _, cx| this.key_move(Dir::Home, false, cx)))
            .on_action(cx.listener(|this, _: &SelectLast, _, cx| this.key_move(Dir::End, false, cx)))
            .on_action(cx.listener(|this, _: &ExtendNext, _, cx| this.key_move(Dir::Down, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendPrev, _, cx| this.key_move(Dir::Up, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendLeft, _, cx| this.key_move(Dir::Left, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendRight, _, cx| this.key_move(Dir::Right, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendHome, _, cx| this.key_move(Dir::Home, true, cx)))
            .on_action(cx.listener(|this, _: &ExtendEnd, _, cx| this.key_move(Dir::End, true, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| {
                if this.renaming.is_none() {
                    this.sel.select_all(&this.placed);
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleSelection, _, cx| {
                if let Some(c) = this.sel.cursor.clone() {
                    if !this.sel.selected.remove(&c) {
                        this.sel.selected.insert(c);
                    }
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ClearSelection, window, cx| {
                if this.renaming.is_some() {
                    this.cancel_rename(window, cx);
                } else {
                    this.sel.clear();
                    this.notice = None;
                    this.menu_at.clear();
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &OpenSelected, window, cx| {
                if this.renaming.is_some() {
                    this.commit_rename(window, cx);
                } else {
                    this.open_selection(window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Properties, _, cx| this.properties(cx)))
            .on_action(cx.listener(|this, _: &CopyItems, _, cx| this.copy_selection(rf_win::fileops::Transfer::Copy, cx)))
            .on_action(cx.listener(|this, _: &CutItems, _, cx| this.copy_selection(rf_win::fileops::Transfer::Move, cx)))
            .on_action(cx.listener(|this, _: &PasteItems, window, cx| this.paste(Via::Keyboard, window, cx)))
            .on_action(cx.listener(|this, _: &DeleteItems, window, cx| this.delete_selection(false, window, cx)))
            .on_action(cx.listener(|this, _: &DeletePermanently, window, cx| this.delete_selection(true, window, cx)))
            .on_action(cx.listener(|this, _: &RenameItem, window, cx| this.begin_rename(window, cx)))
            .on_action(cx.listener(|this, _: &NewFolder, _, cx| this.new_item(true, Via::Keyboard, cx)))
            .on_action(cx.listener(|this, _: &NewTextFile, _, cx| this.new_item(false, Via::Keyboard, cx)))
            .on_action(cx.listener(|this, _: &CopyPath, _, cx| this.copy_paths_as_text(cx)))
            .on_action(cx.listener(|this, _: &Undo, _, cx| {
                if this.renaming.is_none() && !this.read_only() {
                    crate::undo::undo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Redo, _, cx| {
                if this.renaming.is_none() && !this.read_only() {
                    crate::undo::redo(cx);
                }
            }))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.refresh(cx)))
            .on_action(cx.listener(|this, _: &IconsLarge, _, cx| this.set_icon_size(rf_core::desktop::grid::IconSize::Large, cx)))
            .on_action(cx.listener(|this, _: &IconsMedium, _, cx| this.set_icon_size(rf_core::desktop::grid::IconSize::Medium, cx)))
            .on_action(cx.listener(|this, _: &IconsSmall, _, cx| this.set_icon_size(rf_core::desktop::grid::IconSize::Small, cx)))
            .on_action(cx.listener(|this, _: &ZoomIn, _, cx| this.step_icon_size(true, cx)))
            .on_action(cx.listener(|this, _: &ZoomOut, _, cx| this.step_icon_size(false, cx)))
            .on_action(cx.listener(|this, _: &ShowContextMenu, window, cx| {
                // (a menu opened from the keyboard has no pointer position: an older right click's point is not its "here")
                this.menu_at.clear();
                if this.sel.selected.is_empty() {
                    this.show_background_menu(window, cx);
                } else {
                    this.show_item_menu(window, cx);
                }
            }))
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
    }

    fn key_move(&mut self, dir: Dir, extend: bool, cx: &mut Context<Self>) {
        if self.renaming.is_some() {
            return;
        }
        self.typing.reset();
        self.sel.move_cursor(dir, extend, &self.placed);
        cx.notify();
    }

    /// Type to select, and Space for the previewer.
    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let m = &ev.keystroke.modifiers;
        // Ctrl+Alt together is AltGr on Windows (Turkish Q: AltGr+Q = @): that is text
        let altgr = m.control && m.alt;
        if ((m.control || m.alt) && !altgr) || m.platform || self.renaming.is_some() {
            return;
        }
        let now = self.now_ms();
        if ev.keystroke.key == "space" && !m.shift && !self.typing.typing(now) {
            // holding Space repeats the key: one toggle per press
            if !ev.is_held {
                self.preview_toggle(cx);
            }
            cx.stop_propagation();
            return;
        }
        let Some(ch) = ev.keystroke.key_char.as_ref() else { return };
        let names: Vec<(ItemId, String)> = {
            let show_ext = cx.global::<crate::settings::Settings>().show_extensions;
            self.placed.iter().filter_map(|p| self.item(&p.id).map(|it| (p.id.clone(), self.shown_name(it, show_ext)))).collect()
        };
        if let Some(id) = self.typing.key(ch, now, &names, self.sel.cursor.as_ref()) {
            self.sel.only(&id);
            cx.stop_propagation();
            cx.notify();
        }
    }

    /// Ctrl + wheel steps through the icon sizes (up = larger).
    pub(super) fn on_wheel(&mut self, ev: &ScrollEvent, cx: &mut Context<Self>) {
        if !ev.modifiers.control {
            return;
        }
        // a touchpad sends many small deltas, a wheel one large: a step per 40 pixels of either. What was gathered going the other way,
        // or a while ago, does not count towards this step.
        let d = f32::from(ev.delta.pixel_delta(px(40.)).y);
        let now = self.now_ms();
        if (d > 0.0) != (self.wheel > 0.0) || now.saturating_sub(self.wheel_ms) > 400 {
            self.wheel = 0.0;
        }
        self.wheel_ms = now;
        self.wheel += d;
        if self.wheel.abs() >= 40.0 {
            let up = self.wheel > 0.0;
            self.wheel = 0.0;
            self.step_icon_size(up, cx);
        }
        cx.stop_propagation();
    }

    // ---------- mouse: press, band, drag ----------

    pub(super) fn on_tile_down(&mut self, id: &ItemId, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        self.notice = None;
        self.menu_at.clear();
        self.typing.reset();
        if self.renaming.is_some() {
            self.commit_rename(window, cx);
        }
        window.focus(&self.focus, cx);
        let mods = Mods { ctrl: ev.modifiers.control, shift: ev.modifiers.shift };
        if !mods.ctrl && !mods.shift && ev.click_count >= 2 {
            self.sel.only(id);
            self.press = None;
            self.open_item(id, cx);
            cx.notify();
            return;
        }
        self.sel.press(id, mods, &self.placed);
        self.press = Some(Press { id: id.clone(), at: ev.position });
        cx.notify();
    }

    pub(super) fn on_tile_right(&mut self, id: &ItemId, window: &mut Window, cx: &mut Context<Self>) {
        cx.stop_propagation();
        window.focus(&self.focus, cx);
        if self.renaming.is_some() {
            self.commit_rename(window, cx);
        }
        if self.sel.is_selected(id) {
            self.sel.cursor = Some(id.clone());
        } else {
            self.sel.only(id);
        }
        self.show_item_menu(window, cx);
        cx.notify();
    }

    pub(super) fn on_background_down(&mut self, ev: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        self.notice = None;
        self.menu_at.clear();
        self.typing.reset();
        if self.renaming.is_some() {
            self.commit_rename(window, cx);
        }
        let additive = ev.modifiers.control || ev.modifiers.shift;
        if !additive {
            self.sel.clear();
        }
        let pt = self.map.to_canvas(f32::from(ev.position.x), f32::from(ev.position.y));
        self.band = Some(Band { start: pt, cur: pt, base: self.sel.selected.clone(), additive });
        cx.notify();
    }

    pub(super) fn on_background_right(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus, cx);
        if self.renaming.is_some() {
            self.commit_rename(window, cx);
        }
        if !window.modifiers().control {
            self.sel.clear();
        }
        // "New" and "Paste" from this menu mean "here"
        let m = window.mouse_position();
        self.menu_at.set(self.map.to_canvas(f32::from(m.x), f32::from(m.y)), std::time::Instant::now());
        self.show_background_menu(window, cx);
        cx.notify();
    }

    /// The window-level mouse move: the rubber band grows, a press becomes a drag, a drag follows the pointer.
    pub(super) fn on_pointer_move(&mut self, ev: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        let pt = self.map.to_canvas(f32::from(ev.position.x), f32::from(ev.position.y));
        let left = ev.pressed_button == Some(MouseButton::Left);
        if self.band.is_some() {
            if left {
                if let Some(b) = &mut self.band {
                    b.cur = pt;
                }
                self.apply_band();
            } else {
                self.band = None;
            }
            cx.notify();
            return;
        }
        if let Some(press) = &self.press {
            if !left {
                self.press = None;
                return;
            }
            let d = ev.position - press.at;
            if self.drag.is_none() && d.x.abs() + d.y.abs() > px(DRAG_START) {
                self.begin_drag(window, cx);
            }
        }
        // a drag of icons that can only be moved on the layout (system icons, the Public Desktop's) never turns into the system's file
        // drag: it stays here, and where the button comes up decides the monitor (see `finish_drag`)
        let layout_only = self.drag.as_ref().is_some_and(|d| self.layout_only(&d.ids));
        let Some(d) = &mut self.drag else { return };
        if !left {
            // the button came up where this window did not see it
            self.drag = None;
            cx.notify();
            return;
        }
        let vp = window.viewport_size();
        let outside = ev.position.x < px(0.) || ev.position.y < px(0.) || ev.position.x > vp.width || ev.position.y > vp.height;
        if outside && d.files && !layout_only {
            self.leave_window_with_the_drag(window, cx);
            return;
        }
        d.cur = pt;
        let ids = d.ids.clone();
        let over = self.over_at(pt, &ids);
        if let Some(d) = &mut self.drag {
            d.over = over;
        }
        cx.notify();
    }

    fn apply_band(&mut self) {
        let Some(b) = &self.band else { return };
        let (r, base, additive) = (rect_of(b.start, b.cur), b.base.clone(), b.additive);
        self.sel.rubber_band(&base, r, &self.placed, additive);
    }

    fn begin_drag(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(press) = self.press.take() else { return };
        let ids: Vec<ItemId> = if self.sel.is_selected(&press.id) { self.sel.selected.iter().cloned().collect() } else { vec![press.id.clone()] };
        if ids.is_empty() {
            return;
        }
        let files = ids.iter().all(|i| self.item(i).is_some_and(|it| !it.is_virtual()));
        let start = self.map.to_canvas(f32::from(press.at.x), f32::from(press.at.y));
        self.sel.drag_began();
        self.drag = Some(Drag { ids, grabbed: press.id, start, cur: start, over: None, files });
        cx.notify();
    }

    /// The pointer left the window with files in the drag: the system's drag takes over (as in the file list), so that other programs
    /// can receive them. Not in the read-only preview: a drag out can MOVE files.
    fn leave_window_with_the_drag(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.drag.take() else { return };
        if self.read_only() {
            self.say(crate::t!("Dosyalar salt okunur önizlemeden sürüklenip çıkarılamaz"), cx);
            return;
        }
        let paths = self.paths_of(&d.ids);
        if paths.is_empty() {
            return;
        }
        // a drag out can MOVE files, and nothing is moved out of the Public Desktop from here
        if self.under_public(&paths) {
            self.refuse(Refusal::NotYours, cx);
            return;
        }
        let _ = window;
        let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        rf_win::sta::run(move |owner| {
            if let Err(e) = rf_win::dnd::drag_out(owner, ui_thread, &paths) {
                log::warn!("desktop drag out: {e:#}");
            }
        });
        cx.notify();
    }

    pub(super) fn on_pointer_up(&mut self, _ev: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(d) = self.drag.take() {
            self.press = None;
            self.finish_drag(d, window, cx);
            cx.notify();
            return;
        }
        if self.band.take().is_some() {
            // Cover mode: a press on empty space did not activate the window (so that it never takes the keyboard from the user's
            // program for nothing). If the rubber band picked icons, the keyboard belongs to them now: Delete, F2 and Ctrl+C must
            // act on the icons just chosen, as they do on Explorer's desktop.
            // (not `Window::activate_window`: the toolkit's version moves the window back to its first rectangle and sends fake key
            // presses to win the foreground; ours has just been clicked, which is the right to take it)
            if let Some(hwnd) = self.cover.filter(|h| *h != 0)
                && !self.sel.selected.is_empty()
            {
                rf_win::desktop::cover::activate(hwnd);
            }
            cx.notify();
        }
        if self.press.take().is_some() {
            self.sel.release();
            cx.notify();
        }
    }

    /// The item under a canvas point, topmost first (not one of `exclude`).
    pub(super) fn over_at(&self, pt: (i32, i32), exclude: &[ItemId]) -> Option<Over> {
        self.placed.iter().rev().find(|p| p.hit.contains(pt.0, pt.1) && !exclude.contains(&p.id)).and_then(|p| {
            let it = self.item(&p.id)?;
            Some(Over { id: p.id.clone(), accepts: self.accepts_of_item(it) })
        })
    }

    /// Whether these icons can only be moved on the layout, never handed to another program: a system icon has no file, and nothing is
    /// moved or copied out of the Public Desktop from here. Only in cover mode with more than one monitor, where the other monitors are
    /// other windows of this very desktop and a drag that leaves the window is a move to that monitor.
    fn layout_only(&self, ids: &[ItemId]) -> bool {
        if self.cover.is_none() || self.backend.as_ref().is_none_or(|b| b.specs.len() < 2) {
            return false;
        }
        ids.iter().any(|i| self.item(i).is_some_and(|it| it.is_virtual() || it.origin == Origin::Public))
    }

    /// The icons of `d` put down on monitor `to`: only the layout changes.
    fn move_icons_across(&mut self, d: &Drag, to: usize, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        if self.store.get().auto_arrange {
            return self.say(crate::t!("Otomatik düzenleme açıkken simgeler taşınamaz"), cx);
        }
        let delta = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
        let items: Vec<(ItemId, (i32, i32))> = d.ids.iter().filter_map(|i| self.cell_rect_of(i).map(|r| (i.clone(), (r.left, r.top)))).collect();
        self.touch_layout();
        let moved = rf_core::desktop::arrival::move_across(&mut self.store.edit(), &b.specs, &mut self.resolved, &items, b.canvas, delta, to);
        if !moved.is_empty() {
            self.rebuild_placed();
            self.sel.retain_existing(&self.placed);
            self.schedule_save(cx);
            // the other monitor's window draws the icons now
            self.refresh_peers(cx);
        }
        cx.notify();
    }

    fn finish_drag(&mut self, d: Drag, window: &mut Window, cx: &mut Context<Self>) {
        let inside = self.map.contains(d.cur);
        // let go on another monitor: the icons that cannot leave as files are moved there
        if !inside
            && self.layout_only(&d.ids)
            && let Some((to, _)) = self.backend.as_ref().and_then(|b| rf_core::desktop::arrival::monitor_under(&b.specs, b.canvas, d.cur))
        {
            return self.move_icons_across(&d, to, cx);
        }
        let cell = if inside { self.grid.as_ref().and_then(|g| g.snap(d.cur.0, d.cur.1, true)).map(|(c, _, _)| c) } else { None };
        let over = d.over.clone().or_else(|| self.over_at(d.cur, &d.ids));
        let at = Where { over: over.as_ref(), cell };
        match drop::decide_icons(&d.ids, &at, d.files, self.read_only()) {
            DropAction::MoveIcons { .. } => {
                let delta = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
                self.move_icons(&d.ids, delta, cx);
            }
            DropAction::HandTo { target, accepts } => {
                let sources = self.paths_of(&d.ids);
                self.hand_to(&target, accepts, sources, window, cx);
            }
            DropAction::Reject(why) => self.reject(why, cx),
            DropAction::TakeIn => {}
        }
    }

    pub(super) fn reject(&mut self, why: Why, cx: &mut Context<Self>) {
        if why == Why::ReadOnly {
            self.refuse(Refusal::ReadOnly, cx);
        }
        cx.notify();
    }

    // ---------- files dropped in from other programs ----------

    pub(super) fn on_external_move(&mut self, ev: &DragMoveEvent<ExternalPaths>, cx: &mut Context<Self>) {
        let pt = self.map.to_canvas(f32::from(ev.event.position.x), f32::from(ev.event.position.y));
        self.ext_hover = Some(pt);
        if let Some(dir) = self.backend.as_ref().and_then(|b| b.target_dir()) {
            let paths: Vec<PathBuf> = ev.drag(cx).paths().to_vec();
            // the pointer shows what the drop would do with what is under it, by the same decision the drop makes
            let (action, _, _) = self.plan_external(&paths, None, pt);
            let take_in = rf_win::dnd::default_transfer(&paths, &dir, false, false);
            let into_folder = match &action {
                DropAction::HandTo { target, accepts: Accepts::Folder } => self.drop_target_folder(target, &paths, false, false).and_then(|(_, verdict)| verdict.ok()),
                _ => None,
            };
            gpui_windows::dnd_hint::set(match drop::effect_of(&action, take_in, into_folder) {
                Effect::Refused => gpui_windows::dnd_hint::NONE,
                Effect::Copy => gpui_windows::dnd_hint::COPY,
                Effect::Move => gpui_windows::dnd_hint::MOVE,
            });
        }
        cx.notify();
    }

    pub(super) fn on_external_drop_background(&mut self, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let pt = self.ext_hover.take().unwrap_or_else(|| {
            let m = window.mouse_position();
            self.map.to_canvas(f32::from(m.x), f32::from(m.y))
        });
        self.drop_external(paths.paths().to_vec(), None, pt, window, cx);
    }

    pub(super) fn on_external_drop_on_tile(&mut self, id: &ItemId, paths: &ExternalPaths, window: &mut Window, cx: &mut Context<Self>) {
        let pt = self.ext_hover.take().unwrap_or_else(|| {
            let m = window.mouse_position();
            self.map.to_canvas(f32::from(m.x), f32::from(m.y))
        });
        let over = self.item(id).map(|it| Over { id: id.clone(), accepts: self.accepts_of_item(it) });
        self.drop_external(paths.paths().to_vec(), over, pt, window, cx);
    }

    /// The desktop's own items among `paths` (a path directly inside the Desktop folder or the Public one), if ALL of them are.
    fn own_items(&self, paths: &[PathBuf]) -> Option<Vec<ItemId>> {
        let b = self.backend.as_ref()?;
        let same = |a: &std::path::Path, d: &Option<PathBuf>| d.as_deref().is_some_and(|d| a.to_string_lossy().trim_end_matches('\\').eq_ignore_ascii_case(d.to_string_lossy().trim_end_matches('\\')));
        let (user, public) = (b.dir_of(Origin::User), b.dir_of(Origin::Public));
        let mut out = Vec::new();
        for p in paths {
            let (parent, name) = (p.parent()?, p.file_name()?.to_string_lossy().into_owned());
            let id = if same(parent, &user) {
                ItemId::user(&name)
            } else if same(parent, &public) {
                ItemId::public(&name)
            } else {
                return None;
            };
            self.by_id.contains_key(&id).then_some(())?;
            out.push(id);
        }
        (!out.is_empty()).then_some(out)
    }

    /// What dropping `paths` at the canvas point `pt` does (`over`: the item the drop landed on, when GPUI told which; else the item
    /// under the point), and which of the desktop's own items they are. Used by the pointer while dragging and by the drop itself.
    fn plan_external(&self, paths: &[PathBuf], over: Option<Over>, pt: (i32, i32)) -> (DropAction, Vec<ItemId>, bool) {
        let own = self.own_items(paths);
        let ids = own.clone().unwrap_or_default();
        let over = over.or_else(|| self.over_at(pt, &[]));
        let cell = if self.map.contains(pt) { self.grid.as_ref().and_then(|g| g.snap(pt.0, pt.1, true)).map(|(c, _, _)| c) } else { None };
        let at = Where { over: over.as_ref(), cell };
        (drop::decide_external(own.is_some(), &ids, &at, self.read_only()), ids, own.is_some())
    }

    fn drop_external(&mut self, paths: Vec<PathBuf>, over: Option<Over>, pt: (i32, i32), window: &mut Window, cx: &mut Context<Self>) {
        if paths.is_empty() {
            return;
        }
        let (action, ids, _) = self.plan_external(&paths, over, pt);
        match action {
            DropAction::MoveIcons { .. } => {
                // the item that was grabbed is not known to the drop: the first one stands for the others, which keep their distance
                let first = ids.first().cloned();
                if let Some(r) = first.as_ref().and_then(|f| self.cell_rect_of(f)) {
                    let centre = (r.left + r.width() / 2, r.top + r.height() / 2);
                    let delta = (pt.0 - centre.0, pt.1 - centre.1);
                    self.move_icons(&ids, delta, cx);
                }
            }
            DropAction::HandTo { target, accepts } => self.hand_to(&target, accepts, paths, window, cx),
            DropAction::TakeIn => self.take_in(paths, pt, window, cx),
            DropAction::Reject(why) => self.reject(why, cx),
        }
        cx.notify();
    }
}
