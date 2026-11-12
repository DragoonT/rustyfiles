//! Recycle Bin page: list deleted items, restore, empty. All shell work off the UI thread.
//! The list sorts by any column (a click on its header; again for the other direction) and
//! filters by a text typed in the search box (name or original location).
//! "Kalıcı olarak sil" removes the selected items from the bin for good (Delete or Shift+Delete, the
//! menu, the button) after a confirmation of ours; the bin's own settings open as a small dialog
//! (`recycle_settings_view`). Right click opens our menu.

use crate::t;
use std::collections::HashSet;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::format;
use rf_win::recycle::RecycleItem;

use crate::actions::{DeleteItems, DeletePermanently};
use crate::fluent_menu::{self, Item, Menu, MenuIcon};
use crate::image_cache::ImageCache;
use crate::recycle_settings_view::{BinDialogEvent, BinSettingsDialog, Store};
use crate::recycle_text;
use crate::theme::{self, Palette};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Col {
    Name,
    Location,
    Deleted,
    Size,
}

pub struct RecycleView {
    items: Vec<RecycleItem>,
    /// The rows shown: indices into `items`, filtered and in sort order.
    order: Vec<usize>,
    /// Indices into `items`.
    selected: HashSet<usize>,
    sort: (Col, bool),
    /// The search box (made at the first draw: it needs the window).
    search: Option<Entity<InputState>>,
    _search_sub: Option<Subscription>,
    loading: bool,
    busy: bool,
    _task: Option<Task<()>>,
    /// The view takes the keyboard once, so Delete and Shift+Delete reach it (and not the folder list behind it).
    focus: FocusHandle,
    took_focus: bool,
    /// What the last command did, under the title; `true` = it is a problem.
    status: Option<(String, bool)>,
    /// The bin's settings dialog, while it is open.
    settings: Option<Entity<BinSettingsDialog>>,
    _settings_sub: Option<Subscription>,
    /// The shared cache of the shell's own icons: a row asks it for the picture of the item's stored copy.
    images: Entity<ImageCache>,
    _images_sub: Subscription,
}

/// The rows to show: those matching `query` (name or original location, case-insensitive), by
/// `col` (`desc` = the other way round; names break ties).
fn arrange(items: &[RecycleItem], query: &str, col: Col, desc: bool) -> Vec<usize> {
    let q = rf_core::entry::fold_case(query.trim());
    let mut v: Vec<usize> = (0..items.len())
        .filter(|&i| q.is_empty() || rf_core::entry::fold_case(&items[i].name).contains(&q) || rf_core::entry::fold_case(&items[i].original_location).contains(&q))
        .collect();
    v.sort_by(|&a, &b| {
        let (x, y) = (&items[a], &items[b]);
        let o = match col {
            Col::Name => std::cmp::Ordering::Equal,
            Col::Location => rf_core::sort::natural_cmp(&x.original_location, &y.original_location),
            Col::Deleted => x.deleted_ms.cmp(&y.deleted_ms),
            Col::Size => x.size.cmp(&y.size),
        }
        .then_with(|| rf_core::sort::natural_cmp(&x.name, &y.name));
        if desc { o.reverse() } else { o }
    });
    v
}

impl RecycleView {
    pub fn new(images: Entity<ImageCache>, cx: &mut Context<Self>) -> Self {
        // (an icon arrives later than the row: the page draws again when the cache has news)
        let _images_sub = cx.observe(&images, |_, _, cx| cx.notify());
        let mut this = Self {
            items: Vec::new(),
            order: Vec::new(),
            selected: HashSet::new(),
            // newest deleted first, as before
            sort: (Col::Deleted, true),
            search: None,
            _search_sub: None,
            loading: true,
            busy: false,
            _task: None,
            focus: cx.focus_handle(),
            took_focus: false,
            status: None,
            settings: None,
            _settings_sub: None,
            images,
            _images_sub,
        };
        this.reload(cx);
        this
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.loading = true;
        let work = cx.background_spawn(async move {
            rf_win::shell::ensure_com();
            rf_win::recycle::list().unwrap_or_default()
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let items = work.await;
            let _ = this.update(cx, |v, cx| {
                v.items = items;
                v.rearrange(cx);
                v.selected.clear();
                v.loading = false;
                v.busy = false;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn rearrange(&mut self, cx: &App) {
        let query = self.search.as_ref().map(|s| s.read(cx).value().to_string()).unwrap_or_default();
        self.order = arrange(&self.items, &query, self.sort.0, self.sort.1);
        // an item filtered away is not acted on
        let shown: HashSet<usize> = self.order.iter().copied().collect();
        self.selected.retain(|i| shown.contains(i));
    }

    /// A click on a column's header: sorts by it; again for the other direction (dates and
    /// sizes start with the newest / biggest).
    fn sort_by(&mut self, col: Col, cx: &mut Context<Self>) {
        self.sort = if self.sort.0 == col { (col, !self.sort.1) } else { (col, matches!(col, Col::Deleted | Col::Size)) };
        self.rearrange(cx);
        cx.notify();
    }

    fn run(&mut self, cx: &mut Context<Self>, f: impl FnOnce() -> anyhow::Result<()> + Send + 'static) {
        self.busy = true;
        let work = cx.background_spawn(async move {
            rf_win::shell::ensure_com();
            if let Err(e) = f() {
                log::warn!("recycle: {e:#}");
            }
        });
        cx.spawn(async move |this, cx| {
            work.await;
            let _ = this.update(cx, |v, cx| v.reload(cx));
        })
        .detach();
        cx.notify();
    }

    fn restore_selected(&mut self, cx: &mut Context<Self>) {
        // (only rows that are shown: the same rule as the deletion's)
        let items: Vec<RecycleItem> = rf_core::recycle_bin::command_targets(&self.order, &self.selected).into_iter().filter_map(|i| self.items.get(i).cloned()).collect();
        if !items.is_empty() {
            self.status = None;
            self.run(cx, move || rf_win::recycle::restore(&items));
        }
    }

    fn empty_bin(&mut self, cx: &mut Context<Self>) {
        self.status = None;
        // confirm = true → Windows asks the user first
        self.run(cx, || rf_win::recycle::empty(true));
    }

    /// Delete, Shift+Delete and Ctrl+D reach the page also while the search box has the keyboard (the box does not take the
    /// two last ones); typing there is not a command on the rows, so it is ignored then.
    fn delete_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // (the action is handled here either way, so that it does not travel on to the folder list behind the page)
        cx.stop_propagation();
        if self.search.as_ref().is_some_and(|s| s.read(cx).focus_handle(cx).is_focused(window)) {
            return;
        }
        self.delete_selected(window, cx);
    }

    /// "Kalıcı olarak sil": asks (our dialog: how many items, that it cannot be undone), then deletes exactly the selected, shown items.
    pub fn delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy || self.settings.is_some() {
            return;
        }
        let rows = rf_core::recycle_bin::command_targets(&self.order, &self.selected);
        if rows.is_empty() {
            return;
        }
        let facts = rf_core::recycle_bin::delete_facts(rows.iter().map(|&i| (self.items[i].is_dir, self.items[i].size)));
        let name = (rows.len() == 1).then(|| self.items[rows[0]].name.clone());
        let keys: Vec<String> = rows.iter().map(|&i| rf_core::recycle_bin::key_of(&self.items[i].parsing_path)).collect();
        let me = cx.entity().downgrade();
        crate::prompt::show(
            recycle_text::delete_title(),
            Some(recycle_text::delete_message(facts, name.as_deref())),
            None,
            vec![
                crate::prompt::Button::new(t!("Kalıcı olarak sil"), crate::prompt::Kind::Danger, move |_, _, cx| {
                    let _ = me.update(cx, |v, cx| v.delete_now(keys.clone(), cx));
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    /// The deletion itself, off the UI thread; the list is read again and the status line says what happened (counted from the
    /// bin's listing before and after, not from the shell's answer alone).
    fn delete_now(&mut self, keys: Vec<String>, cx: &mut Context<Self>) {
        self.busy = true;
        self.status = None;
        let work = cx.background_spawn(async move { rf_core::recycle_bin::delete_through(&rf_win::recycle::ShellBin, &keys) });
        cx.spawn(async move |this, cx| {
            let report = work.await;
            if report.error.is_some() || report.collateral > 0 {
                log::warn!("recycle delete: requested {}, deleted {}, left {}, collateral {}, error {:?}", report.requested, report.deleted, report.left, report.collateral, report.error);
            }
            let _ = this.update(cx, |v, cx| {
                v.status = Some(recycle_text::delete_status(&report));
                v.reload(cx);
            });
        })
        .detach();
        cx.notify();
    }

    fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        let store: Store = std::sync::Arc::new(rf_win::recycle_settings::SystemBinSettings);
        let dlg = cx.new(|cx| BinSettingsDialog::new(store, window, cx));
        self._settings_sub = Some(cx.subscribe_in(&dlg, window, |this, _, ev: &BinDialogEvent, window, cx| {
            let BinDialogEvent::Closed(saved) = ev;
            this.settings = None;
            this.status = match saved {
                Some(rf_core::recycle_bin::SaveOutcome::Saved { .. }) => Some((t!("Geri Dönüşüm Kutusu ayarları kaydedildi").to_string(), false)),
                Some(rf_core::recycle_bin::SaveOutcome::Unchanged) => Some((t!("Ayarlarda değişiklik yok").to_string(), false)),
                None => this.status.take(),
            };
            window.focus(&this.focus, cx);
            cx.notify();
        }));
        self.settings = Some(dlg);
        cx.notify();
    }

    /// Our menu: on a row, for the rows selected; on the background, the bin's own commands.
    fn context_menu(&mut self, row: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_some() {
            return;
        }
        if let Some(i) = row {
            if !self.selected.contains(&i) {
                self.selected.clear();
                self.selected.insert(i);
            }
            cx.notify();
        }
        let has_sel = !rf_core::recycle_bin::command_targets(&self.order, &self.selected).is_empty() && !self.busy;
        let me = cx.entity().downgrade();
        let mut items = Vec::new();
        {
            let me = me.clone();
            items.push(
                Item::action(t!("Geri yükle"), MenuIcon::Lucide(L::Undo2), move |_, cx| {
                    let _ = me.update(cx, |v, cx| v.restore_selected(cx));
                })
                .enabled(has_sel),
            );
        }
        {
            let me = me.clone();
            items.push(
                Item::action(t!("Kalıcı olarak sil"), MenuIcon::Lucide(L::TrashOff), move |window, cx| {
                    let _ = me.update(cx, |v, cx| v.delete_selected(window, cx));
                })
                .hint("Shift+Del")
                .enabled(has_sel),
            );
        }
        items.push(Item::Separator);
        items.push(Item::action(t!("Geri Dönüşüm Kutusu ayarları…"), MenuIcon::Lucide(L::Settings), move |window, cx| {
            let _ = me.update(cx, |v, cx| v.open_settings(window, cx));
        }));
        fluent_menu::show(Menu { quick: Vec::new(), items }, window.mouse_position(), window, cx);
    }
}

fn button(id: &'static str, icon: L, label: &'static str, enabled: bool, p: &Palette) -> Stateful<Div> {
    h_flex()
        .id(id)
        .h(px(30.))
        .px_3()
        .gap_2()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .bg(p.hover)
        .when(enabled, |d| d.hover(|s| s.bg(p.pressed)).cursor_pointer())
        .when(!enabled, |d| d.opacity(0.5))
        .child(Icon::new(icon).size(px(14.)))
        .child(label)
}

/// The kind of a row by its name, for the glyph drawn until the shell's own icon has arrived (or when it cannot be had): a folder only
/// for a real folder, otherwise the file's own kind (a zip is an archive, not a folder).
fn row_category(it: &RecycleItem) -> crate::icons::Category {
    if it.is_dir {
        return crate::icons::Category::Folder;
    }
    let ext = it.name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    crate::icons::category_for_ext(&ext)
}

fn row_icon(it: &RecycleItem) -> L {
    crate::icons::icon_for(row_category(it))
}

/// The shell's own icon for an item, asked of the shared cache for the item's stored copy in the bin (`$R...`, which keeps the
/// extension, so a zip gets the zip icon). `None` while it is being made, or when the item has no stored path.
fn bin_icon(images: &Entity<ImageCache>, it: &RecycleItem, cx: &mut Context<RecycleView>) -> Option<std::sync::Arc<RenderImage>> {
    let path = std::path::Path::new(&it.parsing_path);
    let (dir, name) = (path.parent()?, path.file_name()?.to_str()?);
    let kind = if it.is_dir { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File };
    let entry = rf_core::Entry::new(name, kind, 0, 0, 0, 0);
    images.update(cx, |c, cx| c.icon(dir, &entry, cx))
}

impl Render for RecycleView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let total: u64 = self.items.iter().map(|i| i.size).sum();
        let has_sel = !self.selected.is_empty();
        let search = match &self.search {
            Some(s) => s.clone(),
            None => {
                let s = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Kutuda ara")));
                self._search_sub = Some(cx.subscribe(&s, |this, _, ev: &InputEvent, cx| {
                    if let InputEvent::Change = ev {
                        this.rearrange(cx);
                        cx.notify();
                    }
                }));
                self.search = Some(s.clone());
                s
            }
        };
        let filtered = self.order.len() != self.items.len();
        let (sort_col, sort_desc) = self.sort;
        let col_head = |id: &'static str, label: &'static str, col: Col| {
            h_flex()
                .id(id)
                .h_full()
                .gap_1()
                .items_center()
                .cursor_pointer()
                .hover(|s| s.text_color(p.text))
                .child(label)
                .when(sort_col == col, |d| d.child(Icon::new(if sort_desc { L::ChevronDown } else { L::ChevronUp }).size(px(12.))))
                .on_click(cx.listener(move |this, _, _, cx| this.sort_by(col, cx)))
        };
        let columns = h_flex()
            .h(px(30.))
            .mx_1p5()
            .px_3()
            .gap_2()
            .items_center()
            .text_xs()
            .text_color(p.text_secondary)
            .border_b_1()
            .border_color(p.divider)
            // (the width of a row's icon)
            .child(div().w(px(16.)).flex_shrink_0())
            .child(col_head("rb-h-name", t!("Ad"), Col::Name).flex_1().min_w_0())
            .child(col_head("rb-h-loc", t!("Özgün konum"), Col::Location).w(px(260.)).flex_shrink_0())
            .child(col_head("rb-h-del", t!("Silinme tarihi"), Col::Deleted).w(px(130.)).flex_shrink_0())
            .child(col_head("rb-h-size", t!("Boyut"), Col::Size).w(px(90.)).flex_shrink_0().justify_end());
        let header = h_flex()
            .px_4()
            .py_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(p.divider)
            .child(Icon::new(L::Trash).size(px(18.)).text_color(p.text_secondary))
            .child(
                v_flex()
                    .flex_1()
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!("Geri Dönüşüm Kutusu")))
                    .child(div().text_xs().text_color(p.text_secondary).child(if filtered {
                        format!("{} / {} · {}", self.order.len(), crate::i18n::items(self.items.len()), format::size(total))
                    } else {
                        format!("{} · {}", crate::i18n::items(self.items.len()), format::size(total))
                    }))
                    .children(self.status.as_ref().map(|(s, problem)| {
                        div().text_xs().text_color(if *problem { p.danger } else { p.text_secondary }).child(s.clone())
                    })),
            )
            .child(
                h_flex()
                    .w(px(220.))
                    .h(px(30.))
                    .px_2()
                    .gap_1()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .bg(p.hover)
                    .child(Icon::new(L::Search).size(px(14.)).text_color(p.text_secondary))
                    .child(Input::new(&search).appearance(false).cleanable(true)),
            )
            .child(
                button("rb-restore", L::Undo2, t!("Geri yükle"), has_sel && !self.busy, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.restore_selected(cx))),
            )
            .child(
                button("rb-delete", L::TrashOff, t!("Kalıcı olarak sil"), has_sel && !self.busy, &p)
                    .on_click(cx.listener(|this, _, window, cx| this.delete_selected(window, cx))),
            )
            .child(
                button("rb-empty", L::Trash, t!("Kutuyu boşalt"), !self.items.is_empty() && !self.busy, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.empty_bin(cx))),
            )
            .child(
                button("rb-settings", L::Settings, t!("Ayarlar"), true, &p).on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
            );

        let body: AnyElement = if self.loading && self.items.is_empty() {
            div().p_6().text_sm().text_color(p.text_secondary).child(t!("Yükleniyor…")).into_any_element()
        } else if self.items.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap_2()
                .child(Icon::new(L::Trash).size(px(40.)).text_color(p.text_tertiary))
                .child(div().text_color(p.text_secondary).child(t!("Geri dönüşüm kutusu boş")))
                .into_any_element()
        } else if self.order.is_empty() {
            div().p_6().text_sm().text_color(p.text_secondary).child(t!("Eşleşen öğe yok")).into_any_element()
        } else {
            let n = self.order.len();
            let images = self.images.clone();
            uniform_list(
                "recycle-rows",
                n,
                cx.processor(move |this, range: std::ops::Range<usize>, window, cx| {
                    let p = Palette::for_window(window);
                    range
                        .filter_map(|row| this.order.get(row).copied())
                        .map(|i| {
                            let it = &this.items[i];
                            let sel = this.selected.contains(&i);
                            let shell_icon = bin_icon(&images, it, cx);
                            h_flex()
                                .id(("rb", i))
                                .w_full()
                                .h(theme::ROW_H)
                                .px_3()
                                .gap_2()
                                .items_center()
                                .rounded(theme::RADIUS_SM)
                                .text_sm()
                                .when(sel, |d| d.bg(p.selection))
                                .when(!sel, |d| d.hover(|s| s.bg(p.hover)))
                                .child(match shell_icon {
                                    Some(im) => img(im).size(px(16.)).flex_shrink_0().into_any_element(),
                                    None => Icon::new(row_icon(it)).size(px(16.)).text_color(theme::kind_color(row_category(it), p.dark)).into_any_element(),
                                })
                                .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(it.name.clone()))
                                .child(
                                    div()
                                        .w(px(260.))
                                        .flex_shrink_0()
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .text_color(p.text_secondary)
                                        .child(it.original_location.clone()),
                                )
                                .child(div().w(px(130.)).flex_shrink_0().text_color(p.text_secondary).child(format::datetime(it.deleted_ms)))
                                .child(
                                    div()
                                        .w(px(90.))
                                        .flex_shrink_0()
                                        .flex()
                                        .justify_end()
                                        .text_color(p.text_secondary)
                                        .child(format::size(it.size)),
                                )
                                .on_mouse_down(
                                    MouseButton::Right,
                                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                        // the page's own right click (the background menu) must not open a second menu over this one
                                        cx.stop_propagation();
                                        window.focus(&this.focus, cx);
                                        this.context_menu(Some(i), window, cx);
                                    }),
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, window, cx| {
                                        window.focus(&this.focus, cx);
                                        if ev.modifiers.control {
                                            if !this.selected.remove(&i) {
                                                this.selected.insert(i);
                                            }
                                        } else {
                                            this.selected.clear();
                                            this.selected.insert(i);
                                        }
                                        cx.notify();
                                    }),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .flex_1()
            .p_1p5()
            .into_any_element()
        };
        if !self.took_focus {
            // once, when the view first shows: the keys of this page are its own
            self.took_focus = true;
            window.focus(&self.focus, cx);
        }
        v_flex()
            .id("recycle-view")
            .relative()
            .size_full()
            .track_focus(&self.focus)
            // the folder list's own Delete / Shift+Delete bindings reach here first, because the view has the focus
            .on_action(cx.listener(|this, _: &DeleteItems, window, cx| this.delete_key(window, cx)))
            .on_action(cx.listener(|this, _: &DeletePermanently, window, cx| this.delete_key(window, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.context_menu(None, window, cx);
                }),
            )
            .child(header)
            .when(!self.items.is_empty(), |d| d.child(columns))
            .child(body)
            .children(self.settings.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{Col, arrange};
    use rf_win::recycle::RecycleItem;

    fn item(name: &str, from: &str, deleted_ms: i64, size: u64) -> RecycleItem {
        RecycleItem { name: name.into(), original_location: from.into(), deleted_ms, size, parsing_path: String::new(), is_dir: false }
    }

    #[test]
    fn a_zip_in_the_bin_is_drawn_as_an_archive_and_a_folder_as_a_folder() {
        use super::row_icon;
        use gpui_kit::assets::IconName as L;
        let mut z = item("belgeler.zip", r"C:\x", 1, 1);
        assert!(matches!(row_icon(&z), L::FileArchive));
        z.name = "foto.JPG".into();
        assert!(matches!(row_icon(&z), L::FileImage));
        z.name = "README".into();
        assert!(matches!(row_icon(&z), L::File));
        z.name = "klasör".into();
        z.is_dir = true;
        assert!(matches!(row_icon(&z), L::Folder));
    }

    #[test]
    fn the_list_sorts_by_a_column_and_filters_by_name_or_location() {
        let items = [item("rapor 10.pdf", r"C:\Belgeler", 300, 5), item("Rapor 2.pdf", r"D:\İş", 100, 50), item("foto.jpg", r"C:\Resimler", 200, 500)];
        assert_eq!(arrange(&items, "", Col::Deleted, true), vec![0, 2, 1]);
        assert_eq!(arrange(&items, "", Col::Size, false), vec![0, 1, 2]);
        // names the natural way: 2 before 10, whatever the case
        assert_eq!(arrange(&items, "", Col::Name, false), vec![2, 1, 0]);
        assert_eq!(arrange(&items, "", Col::Location, false), vec![0, 2, 1]);
        assert_eq!(arrange(&items, "RAPOR", Col::Name, false), vec![1, 0]);
        // (the Turkish dotted İ of the location)
        assert_eq!(arrange(&items, "iş", Col::Name, false), vec![1]);
        assert!(arrange(&items, "yok", Col::Name, false).is_empty());
    }
}
