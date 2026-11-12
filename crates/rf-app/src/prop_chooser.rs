//! "Diğer sütunlar…": the chooser for Windows properties as Details columns. A searchable list with
//! check marks; the common properties of pictures, music, videos and documents come first, then every
//! other property the system knows, by its name in the UI language. A click on a row adds or removes
//! the column at once (the settings hold the list; the folder views follow).
//!
//! Painted over the window by the workspace, like the organizer's layers; one at a time.

use std::sync::Arc;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::propcols::{self as pc, Category, ChooserRow, ChosenColumns};
use rf_win::propcols::{Listing, PropInfo};

use crate::folder_view::prop_cols;
use crate::t;
use crate::theme::{self, Palette};

struct Host(Option<(AnyWindowHandle, Entity<ChooserView>, Option<FocusHandle>)>);
impl Global for Host {}

/// Opens the chooser over the window.
pub fn open(window: &mut Window, cx: &mut App) {
    if is_open(window, cx) {
        return;
    }
    let restore = window.focused(cx);
    let view = cx.new(|cx| ChooserView::new(window, cx));
    view.read(cx).input.clone().update(cx, |s, cx| s.focus(window, cx));
    cx.set_global(Host(Some((window.window_handle(), view, restore))));
    window.refresh();
}

pub fn close(window: &mut Window, cx: &mut App) {
    let Some(Some((_, _, restore))) = cx.try_global::<Host>().map(|h| h.0.clone()) else { return };
    cx.set_global(Host(None));
    if let Some(f) = restore {
        window.focus(&f, cx);
    }
    window.refresh();
}

/// The chooser to paint over the window (the workspace puts it last), if one is open in it.
pub fn overlay(window: &Window, cx: &App) -> Option<AnyElement> {
    let (handle, view, _) = cx.try_global::<Host>()?.0.as_ref()?;
    (*handle == window.window_handle()).then(|| view.clone().into_any_element())
}

pub fn is_open(window: &Window, cx: &App) -> bool {
    cx.try_global::<Host>().and_then(|h| h.0.as_ref()).is_some_and(|(handle, _, _)| *handle == window.window_handle())
}

struct ChooserView {
    input: Entity<InputState>,
    /// Every property, by display name (empty until the system has answered).
    all: Arc<Vec<PropInfo>>,
    loaded: bool,
    rows: Vec<ChooserRow>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    /// A line under the list (the limit was reached).
    note: Option<SharedString>,
    _sub: Subscription,
}

fn category_name(c: Option<Category>) -> &'static str {
    match c {
        Some(Category::Pictures) => t!("Resimler"),
        Some(Category::Music) => t!("Müzik"),
        Some(Category::Video) => t!("Videolar"),
        Some(Category::Documents) => t!("Belgeler"),
        None => t!("Tüm özellikler"),
    }
}

impl ChooserView {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Özellik ara…")));
        let sub = cx.subscribe_in(&input, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.refilter(cx);
            }
        });
        let known = prop_cols::property_list();
        let mut this = Self {
            input,
            all: known.clone().unwrap_or_default(),
            loaded: known.is_some(),
            rows: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            note: None,
            _sub: sub,
        };
        this.refilter(cx);
        if known.is_none() {
            // the schema is loaded off the UI thread (it takes a moment the first time)
            cx.spawn(async move |this, cx| {
                let list = cx.background_executor().spawn(async move { rf_win::propcols::list_properties(Listing::Viewable) }).await;
                let list = prop_cols::set_property_list(list);
                let _ = this.update(cx, |v, cx| {
                    v.all = list;
                    v.loaded = true;
                    v.refilter(cx);
                });
            })
            .detach();
        }
        this
    }

    fn refilter(&mut self, cx: &mut Context<Self>) {
        let pairs: Vec<(String, String)> = self.all.iter().map(|p| (p.canonical.clone(), p.display.clone())).collect();
        let q = self.input.read(cx).value().to_string();
        self.rows = pc::chooser_rows(&pairs, &q);
        self.scroll.scroll_to_item(0, ScrollStrategy::Top);
        cx.notify();
    }

    fn toggle(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(info) = self.all.get(ix) else { return };
        let (name, width) = (info.canonical.clone(), info.default_width);
        let mut full = false;
        crate::settings::update(cx, |s| {
            let mut c = ChosenColumns::from_setting(&s.columns.properties);
            let had = c.contains(&name);
            c.toggle(&name, width);
            full = !had && !c.contains(&name);
            s.columns.properties = c.to_setting();
        });
        self.note = full.then(|| format!("{} {}", t!("Bu kadar özellik sütunu seçilebilir:"), pc::MAX_COLUMNS).into());
        cx.notify();
    }

    fn render_rows(&mut self, range: std::ops::Range<usize>, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let p = Palette::for_window(window);
        let chosen = ChosenColumns::from_setting(&cx.global::<crate::settings::Settings>().columns.properties);
        let mut out = Vec::with_capacity(range.len());
        for r in range {
            let Some(row) = self.rows.get(r).copied() else { break };
            match row {
                ChooserRow::Heading(c) => out.push(
                    // (the same height as a property row: a virtual list measures one row and lays out all by it)
                    div()
                        .h(px(32.))
                        .px_3()
                        .pt_2()
                        .flex()
                        .items_center()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_secondary)
                        .child(category_name(c))
                        .into_any_element(),
                ),
                ChooserRow::Item(i) => {
                    let Some(info) = self.all.get(i) else { continue };
                    let on = chosen.contains(&info.canonical);
                    out.push(
                        h_flex()
                            .id(("pc-row", r))
                            .h(px(32.))
                            .px_3()
                            .gap_3()
                            .items_center()
                            .rounded(theme::RADIUS_SM)
                            .text_sm()
                            .cursor_pointer()
                            .hover(|s| s.bg(p.hover))
                            .child(
                                div()
                                    .size(px(16.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(px(4.))
                                    .border_1()
                                    .when(on, |d| d.bg(p.accent).border_color(p.accent))
                                    .when(!on, |d| d.border_color(p.text_tertiary))
                                    .when(on, |d| d.child(Icon::new(L::Check).size(px(11.)).text_color(p.accent_text))),
                            )
                            .child(div().flex_1().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(info.display.clone()))
                            .child(div().text_xs().text_color(p.text_tertiary).whitespace_nowrap().child(info.canonical.clone()))
                            .on_click(cx.listener(move |this, _, _, cx| this.toggle(i, cx)))
                            .into_any_element(),
                    );
                }
            }
        }
        out
    }
}

impl Render for ChooserView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let n = self.rows.len();
        let list = uniform_list("pc-list", n, cx.processor(|this, range, window, cx| this.render_rows(range, window, cx)))
            .track_scroll(&self.scroll)
            .flex_1()
            .px_2();
        let empty = (n == 0).then(|| {
            let msg = if self.loaded { t!("Eşleşen özellik yok") } else { t!("Özellikler yükleniyor…") };
            div().p_4().text_sm().text_color(p.text_secondary).child(msg)
        });
        div()
            .id("prop-chooser-layer")
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(hsla(0., 0., 0., 0.35))
            .track_focus(&self.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, cx| close(window, cx)))
            .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, window, cx| {
                if ev.keystroke.key == "escape" {
                    cx.stop_propagation();
                    close(window, cx);
                }
            }))
            .child(
                v_flex()
                    .id("prop-chooser")
                    .w(px(560.))
                    .h(px(520.))
                    .max_h(relative(0.9))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .overflow_hidden()
                    .text_color(p.text)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        v_flex()
                            .p_4()
                            .gap_3()
                            .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).child(t!("Sütunları seç")))
                            .child(
                                div()
                                    .h(px(34.))
                                    .px_2()
                                    .flex()
                                    .items_center()
                                    .rounded(theme::RADIUS_SM)
                                    .bg(p.content_bg)
                                    .border_1()
                                    .border_color(p.accent)
                                    .child(Input::new(&self.input).appearance(false).prefix(Icon::new(L::Search).size(px(14.)).text_color(p.text_secondary))),
                            ),
                    )
                    .child(list)
                    .children(empty)
                    .child(
                        h_flex()
                            .px_4()
                            .py_3()
                            .gap_3()
                            .items_center()
                            .bg(p.hover)
                            .border_t_1()
                            .border_color(p.divider)
                            .child(div().flex_1().text_xs().text_color(p.text_secondary).children(self.note.clone()))
                            .child(
                                div()
                                    .id("pc-done")
                                    .min_w(px(96.))
                                    .h(px(32.))
                                    .px_4()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded(theme::RADIUS_SM)
                                    .text_sm()
                                    .bg(p.accent)
                                    .text_color(p.accent_text)
                                    .hover(|s| s.opacity(0.9))
                                    .child(t!("Bitti"))
                                    .on_click(cx.listener(|_, _, window, cx| close(window, cx))),
                            ),
                    ),
            )
    }
}
