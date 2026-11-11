//! Recycle Bin page: list deleted items, restore, empty. All shell work off the UI thread.
//! The list sorts by any column (a click on its header; again for the other direction) and
//! filters by a text typed in the search box (name or original location).

use crate::t;
use std::collections::HashSet;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::format;
use rf_win::recycle::RecycleItem;

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
    pub fn new(cx: &mut Context<Self>) -> Self {
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
        let items: Vec<RecycleItem> = self.selected.iter().filter_map(|&i| self.items.get(i).cloned()).collect();
        if !items.is_empty() {
            self.run(cx, move || rf_win::recycle::restore(&items));
        }
    }

    fn empty_bin(&mut self, cx: &mut Context<Self>) {
        // confirm = true → Windows asks the user first
        self.run(cx, || rf_win::recycle::empty(true));
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
                button("rb-empty", L::Trash, t!("Kutuyu boşalt"), !self.items.is_empty() && !self.busy, &p)
                    .on_click(cx.listener(|this, _, _, cx| this.empty_bin(cx))),
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
                                .child(
                                    Icon::new(if it.is_dir { L::Folder } else { L::File })
                                        .size(px(16.))
                                        .text_color(p.text_secondary),
                                )
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
                                    MouseButton::Left,
                                    cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
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
        v_flex().size_full().child(header).when(!self.items.is_empty(), |d| d.child(columns)).child(body)
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
