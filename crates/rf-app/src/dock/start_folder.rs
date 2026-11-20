//! Folders of the pinned grid (issue 88: "group icons on start menu ... the same behavior of windows"). An app dropped on the middle of
//! another pinned app makes a folder of the two; dropped on a folder it joins it. A folder is one tile: a little square of its first apps'
//! icons and its name. A click opens it over the grid: its name (edited in place) and its apps, which start with a click; the right
//! button's menu takes one out. The rules (which pins are a tile, what a drop or a take-out does to the pin file) are
//! `rf_win::dock::startdrag`; the file keeps a folder as a name on each of its pins.

use rf_win::dock::startdrag as rules;

use super::*;

/// A folder tile of the pinned grid: its name and its apps (indexes into the app list), in the file's order.
#[derive(Clone, Debug)]
pub(super) struct PinFolder {
    pub name: String,
    pub apps: Vec<usize>,
}

/// The folder that is open: which tile, the name it had when it opened, and the field its name is edited in (made at the next frame,
/// which has the window a field needs).
pub(super) struct OpenFolder {
    pub tile: usize,
    pub name: String,
    pub input: Option<Entity<InputState>>,
    _sub: Option<Subscription>,
}

/// The open folder's card: tiles across and its widest.
const FOLDER_COLS: usize = 4;
const FOLDER_PAD: f32 = 16.0;
const FOLDER_HEAD: f32 = 44.0;

impl StartView {
    /// The folder at tile `i`, if that tile is one.
    pub(super) fn folder_at(&self, i: usize) -> Option<&PinFolder> {
        self.pin_folders.get(i).and_then(Option::as_ref)
    }

    /// The little square a folder tile shows: up to four of its apps' icons, two by two.
    pub(super) fn folder_picture(&mut self, apps: &[usize], logical: f32, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let pad = logical / 12.0;
        let gap = logical / 18.0;
        let mini = (logical - 2.0 * pad - gap) / 2.0;
        let mut cells: Vec<AnyElement> = Vec::new();
        for &a in apps.iter().take(4) {
            if let Some(app) = self.apps.get(a).cloned() {
                cells.push(div().size(px(mini)).flex().items_center().justify_center().child(self.app_picture(&app, mini, pal, cx)).into_any_element());
            }
        }
        let mut rows = v_flex().gap(px(gap));
        let mut it = cells.into_iter();
        for _ in 0..2 {
            let mut line = h_flex().gap(px(gap)).h(px(mini));
            for _ in 0..2 {
                line = line.child(it.next().unwrap_or_else(|| div().size(px(mini)).into_any_element()));
            }
            rows = rows.child(line);
        }
        div().size(px(logical)).p(px(pad)).rounded(px(logical / 4.5)).bg(pal.hover_fill().hsla()).child(rows).into_any_element()
    }

    /// A folder's tile in the grid (as an app's tile: the square, the name under it).
    pub(super) fn folder_tile(&mut self, i: usize, folder: &PinFolder, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> Stateful<Div> {
        let (hover, sel_fill) = (pal.hover_fill().hsla(), theme.selection);
        let selected = self.spot == Some(Spot::Pin(i));
        let picture = self.folder_picture(&folder.apps, page::TILE_ICON, pal, cx);
        let name = folder.name.clone();
        let held_name = folder.name.clone();
        v_flex()
            .id(("pin", i))
            .relative()
            .w(px(CELL_W))
            .h(px(page::TILE_H))
            .flex_shrink_0()
            .items_center()
            .pt(px(12.))
            .gap(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
            .when(!selected, |d| d.hover(move |s| s.bg(hover)))
            .child(picture)
            .child(div().w_full().h(px(30.)).px(px(6.)).text_size(px(12.)).line_height(px(15.)).text_center().line_clamp(2).child(name))
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.click_allowed() {
                    this.open_folder(i, cx)
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                    let origin = this.tile_origin(i);
                    this.drag_press(drag::Picked::Folder { name: held_name.clone(), slot: i }, ev.position, origin, cx)
                }),
            )
    }

    /// Opens the folder at tile `i` (a click, or Enter on it).
    pub(super) fn open_folder(&mut self, i: usize, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(f) = self.folder_at(i) else { return };
        let name = f.name.clone();
        self.app_menu = None;
        self.folder = Some(OpenFolder { tile: i, name, input: None, _sub: None });
        cx.notify();
    }

    /// The open folder's name field, made once the window is at hand (the frame after the folder opened).
    pub(super) fn ensure_folder_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(f) = self.folder.as_mut() else { return };
        if f.input.is_some() {
            return;
        }
        let name = f.name.clone();
        let input = cx.new(|cx| InputState::new(window, cx).default_value(name));
        let sub = cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                this.commit_folder_name(cx);
            }
        });
        f.input = Some(input);
        f._sub = Some(sub);
    }

    /// The name typed in the open folder's field becomes the folder's (a name another folder has, or an empty one, is not taken).
    pub(super) fn commit_folder_name(&mut self, cx: &mut Context<Self>) {
        let Some(f) = self.folder.as_ref() else { return };
        let Some(typed) = f.input.as_ref().map(|i| i.read(cx).value().to_string()) else { return };
        let old = f.name.clone();
        if !self.pins_loaded || !rules::rename_folder(&mut self.pins.pins, &old, &typed) {
            return;
        }
        if let Some(f) = self.folder.as_mut() {
            f.name = typed.trim().to_string();
        }
        self.rebuild_pin_cells();
        save_pins(self.pins.clone());
        cx.notify();
    }

    /// Closes the open folder (its name is kept first).
    pub(super) fn close_folder(&mut self, cx: &mut Context<Self>) {
        if self.folder.is_some() {
            self.commit_folder_name(cx);
            self.folder = None;
            self.app_menu = None;
            cx.notify();
        }
    }

    /// The held app at tile `from` was let go on the middle of tile `onto`: the two are a folder, or the app joins `onto`'s folder.
    pub(super) fn commit_group(&mut self, from: usize, onto: usize, cx: &mut Context<Self>) {
        if !self.pins_loaded {
            return;
        }
        let name = rules::fresh_folder_name(&self.pins.pins, t!("Klasör"));
        let tiles = self.pin_places.clone();
        if rules::group_onto(&mut self.pins.pins, &tiles, from, onto, &name) {
            self.rebuild_pin_cells();
            save_pins(self.pins.clone());
            self.spot = None;
        }
        cx.notify();
    }

    /// The app is taken out of its folder (the menu of an app in the open folder): it stands right after the folder.
    pub(super) fn take_out_of_folder(&mut self, app: &AppEntry, cx: &mut Context<Self>) {
        if !self.live() || !self.pins_loaded {
            return;
        }
        let Some(pos) = self.pins.pins.iter().position(|p| startpins::same_id(&p.id, &app.id)) else { return };
        rules::take_out_of_folder(&mut self.pins.pins, pos);
        self.rebuild_pin_cells();
        save_pins(self.pins.clone());
        // (a folder left with one app is no folder: it closes)
        if self.folder.as_ref().is_some_and(|f| self.folder_at(f.tile).is_none_or(|g| g.name != f.name)) {
            self.folder = None;
        }
        cx.notify();
    }

    /// The open folder over the grid, with a backdrop that closes it on a press outside.
    pub(super) fn render_folder(&mut self, pal: Palette, view_w: f32, view_h: f32, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(f) = self.folder.as_ref() else { return Vec::new() };
        // (a search typed meanwhile shows its results: the folder gives way)
        if !self.query.trim().is_empty() || self.page != Page::Home {
            return Vec::new();
        }
        let Some(folder) = self.folder_at(f.tile).cloned() else { return Vec::new() };
        let input = f.input.clone();
        let (hover, border) = (pal.hover_fill().hsla(), pal.separator().hsla());
        let dark = pal.mix < 0.5;
        // HyperGlass: the card is the glass's own wash, as the panel's menus over the glass are, and the grid under it fades out while it
        // is open (`render_home`) so that only the glass shows through; without the glass it is the plain panel
        let plain = self.popup.as_ref().is_none_or(|g| g.fallback());
        let fill = glass_watch::panel_fill(dark, plain);
        let rows = folder.apps.len().div_ceil(FOLDER_COLS).max(1);
        let w = FOLDER_COLS as f32 * CELL_W + 2.0 * FOLDER_PAD;
        let room = (view_h - 2.0 * PAD - FOLDER_HEAD - 2.0 * FOLDER_PAD).max(page::TILE_H);
        let grid_h = (rows as f32 * page::TILE_H).min(room);
        let h = FOLDER_HEAD + grid_h + 2.0 * FOLDER_PAD;
        let (x, y) = (((view_w - w) / 2.0).max(8.0), ((view_h - h) / 2.5).max(8.0));
        let mut grid = v_flex().id("start-folder-apps").w_full().h(px(grid_h)).overflow_y_scroll();
        for line in folder.apps.chunks(FOLDER_COLS) {
            let mut row = h_flex().h(px(page::TILE_H)).flex_shrink_0();
            for &a in line {
                let Some(app) = self.apps.get(a).cloned() else { continue };
                let icon = self.app_picture(&app, page::TILE_ICON, pal, cx);
                let (a1, a2) = (app.clone(), app.clone());
                row = row.child(
                    v_flex()
                        .id(("folder-app", a))
                        .w(px(CELL_W))
                        .h(px(page::TILE_H))
                        .flex_shrink_0()
                        .items_center()
                        .pt(px(12.))
                        .gap(px(6.))
                        .rounded(px(8.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .child(icon)
                        .child(div().w_full().h(px(30.)).px(px(6.)).text_size(px(12.)).line_height(px(15.)).text_center().line_clamp(2).child(app.name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.click_allowed() {
                                this.launch_app(a1.clone(), cx)
                            }
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_menu_in_folder(a2.clone(), ev.position, cx)
                        })),
                );
            }
            grid = grid.child(row);
        }
        let close = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            this.close_folder(cx);
        });
        let close_r = cx.listener(|this: &mut Self, _: &MouseDownEvent, _, cx| {
            cx.stop_propagation();
            this.close_folder(cx);
        });
        let backdrop = div().id("start-folder-backdrop").absolute().inset_0().on_mouse_down(MouseButton::Left, close).on_mouse_down(MouseButton::Right, close_r);
        let name_field = match input {
            Some(i) => div().w(px((w - 2.0 * FOLDER_PAD).min(260.0))).text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(Input::new(&i).appearance(false)).into_any_element(),
            None => div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(folder.name.clone()).into_any_element(),
        };
        let card = v_flex()
            .id("start-folder")
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .p(px(FOLDER_PAD))
            .rounded(theme::RADIUS)
            .bg(fill)
            .border_1()
            .border_color(border)
            .shadow_lg()
            .occlude()
            // (a press in the card stays in it: the panel's own press would take the keyboard to the search field from the name)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(h_flex().h(px(FOLDER_HEAD)).flex_shrink_0().items_center().px(px(12.)).child(name_field))
            .child(grid);
        vec![backdrop.into_any_element(), card.into_any_element()]
    }
}
