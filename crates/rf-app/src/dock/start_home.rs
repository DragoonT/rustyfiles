//! The Start panel's pages, drawn: the search field, the HOME page (the pinned grid with its dots, "Önerilenler"), the ALL APPS page
//! and the bottom row (the account picture and name, the gear, the power button). A child module of `start.rs`: it draws that view's own
//! state and shares its private parts. The numbers are `rf_win::dock::startpage`, the choice of the recommended entries is
//! `rf_win::dock::startrecent`, the account picture is `rf_win::dock::startuser`; none of them is repeated here.
//!
//! WHAT IT COSTS. Nothing runs while the panel is hidden. An opening reads the recent-file shortcuts and (once for each size) the
//! account picture on worker threads, builds at most 18 pinned tiles and 6 recommended entries (a page of the grid, not all the pins),
//! and the all-apps list stays a virtual list that builds only the rows in view.

use std::time::{SystemTime, UNIX_EPOCH};

use rf_win::dock::clock;
use rf_win::dock::startrecent::{self, Pick, Source, When};
use rf_win::toasts::{self, Ago};

use super::*;
use crate::dock::cc_model::Words;

/// The account picture's size on the screen.
const AVATAR: f32 = 28.0;

/// A recommended entry as the panel draws it: what it opens and its second line, worked out when the reading came.
#[derive(Clone)]
pub(super) struct RecEntry {
    pub pick: Pick,
    pub line: SharedString,
}

/// The picture cache's key of a recommended document.
pub(super) fn rec_key(path: &std::path::Path) -> String {
    format!("rec:{}", path.to_string_lossy().to_lowercase())
}

/// Whether a picture stays in the cache when the panel is hidden: the apps' own pictures, and the pictures of the entries that are
/// recommended now (the next opening shows them at once). The pictures of documents that were recommended before are let go, so that
/// the cache does not grow with every document the user has opened over weeks.
pub(super) fn keep_icon_on_hide(key: &str, now_recommended: &std::collections::HashSet<String>) -> bool {
    key.starts_with("app:") || now_recommended.contains(key)
}

/// The device pixels the account picture is asked for at.
pub(super) fn avatar_px(scale: f32) -> u32 {
    (AVATAR * scale.max(1.0)).ceil() as u32
}

/// The second line of a recommended entry: how long ago a document was opened ("5 dk önce", "dün", the date), "Son eklenen" for an
/// app. `date` is the date already written in the user's own format.
pub(super) fn rec_line(when: When, date: &str, w: Words) -> String {
    let tr = w.tr;
    match when {
        When::Added => tr("Son eklenen").to_string(),
        When::Ago(Ago::Now) => tr("Az önce").to_string(),
        When::Ago(Ago::Minutes(n)) => format!("{n} {} {}", tr("dk"), tr("önce")),
        When::Ago(Ago::Hours(n)) => format!("{n} {} {}", tr("sa"), tr("önce")),
        When::Ago(Ago::Yesterday) => tr("dün").to_string(),
        When::Ago(Ago::Date) => date.to_string(),
    }
}

/// Reads the recent documents and chooses the entries: the newest first, each once, only those still there. Each comes with its date
/// written in the user's format (for an old one). Blocks (the disk, the shell thread): a worker thread.
fn read_recs() -> Vec<(Pick, String)> {
    let now_ms = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let picks = startrecent::choose(startrecent::recent_candidates(), now_ms, startrecent::LIMIT, |ms| toasts::local_day(ms.div_euclid(1000)), startrecent::still_there);
    if picks.is_empty() {
        return Vec::new();
    }
    let f = clock::system_formats();
    picks
        .into_iter()
        .map(|p| {
            let date = toasts::local_clock(p.at_ms.div_euclid(1000)).map(|t| clock::format_pattern(&f.date, &t, &f.am, &f.pm)).unwrap_or_default();
            (p, date)
        })
        .collect()
}

fn heading(text: &'static str, color: Hsla) -> Div {
    div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).text_color(color).child(text)
}

impl StartView {
    // ------------------------------------------------------------------ "Önerilenler": reading and using

    /// When the panel is hidden: the pictures that stay in the cache (see [`keep_icon_on_hide`]).
    pub(super) fn trim_icons_on_hide(&mut self) {
        let now: std::collections::HashSet<String> = self.recs.iter().filter_map(|e| if let Source::File(p) = &e.pick.source { Some(rec_key(p)) } else { None }).collect();
        self.icons.retain(|k| keep_icon_on_hide(k, &now));
    }

    /// Starts reading the recommended entries for this opening (the setting is read now). The entries of the last opening are put away
    /// first: a file that was deleted since must not be shown, not even for the moment the reading takes.
    pub(super) fn load_recs(&mut self, cx: &mut Context<Self>) {
        self.recs_wanted = cx.global::<Settings>().dock_start_recommended;
        self.recs.clear();
        self._recs_task = None;
        if !self.recs_wanted {
            self.recs_loaded = true;
            return;
        }
        self.recs_loaded = false;
        let work = cx.background_spawn(async move { read_recs() });
        self._recs_task = Some(cx.spawn(async move |this, cx| {
            let read = work.await;
            let _ = this.update(cx, |me, cx| {
                if !me.shown {
                    return;
                }
                let words = Words::current();
                me.recs = read.into_iter().map(|(pick, date)| RecEntry { line: rec_line(pick.when, &date, words).into(), pick }).collect();
                me.recs_loaded = true;
                cx.notify();
            });
        }));
    }

    /// How many recommended entries are on the screen (a short panel shows fewer rows).
    pub(super) fn rec_shown(&self) -> usize {
        if self.recs_wanted { self.recs.len().min(self.layout.rec_rows * page::REC_COLS) } else { 0 }
    }

    /// Enter, Ctrl+Enter or a click on a recommended entry: open the document, or show it in its folder.
    pub(super) fn activate_rec(&mut self, j: usize, how: How, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(entry) = self.recs.get(j).cloned() else { return };
        match (entry.pick.source, how) {
            (Source::File(path), How::Open) => {
                Self::open_in_shell(path);
                self.finish(cx);
            }
            (Source::File(path), How::ShowInFolder) => self.show_app_location(path, cx),
            (Source::App(id), how) => self.activate_app(AppEntry { id, name: entry.pick.name }, how, cx),
        }
    }

    // ------------------------------------------------------------------ the search field

    /// The search field: a pill, 36 px high, with the keyboard in it.
    pub(super) fn render_search_field(&mut self, pal: Palette) -> impl IntoElement {
        div().flex_shrink_0().px(px(page::SIDE_PAD)).pt(px(page::TOP_PAD)).child(
            h_flex()
                .h(px(page::SEARCH_H))
                .w_full()
                .px(px(14.))
                .gap_2()
                .items_center()
                .rounded(px(page::SEARCH_RADIUS))
                .bg(pal.field_fill().hsla())
                .border_1()
                .border_color(pal.separator().hsla())
                .child(Icon::new(L::Search).size(px(16.)).text_color(pal.text_secondary().hsla()))
                .child(div().flex_1().min_w_0().text_size(px(14.)).child(Input::new(&self.input).appearance(false))),
        )
    }

    // ------------------------------------------------------------------ the buttons that change the page

    /// "Tümü" (home to all apps) or "Geri" (back): a small soft button with a chevron and a keyboard stop.
    fn page_button(&self, id: &'static str, label: &'static str, back: bool, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> Stateful<Div> {
        let (hover, pressed, ring) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla(), theme.accent);
        let secondary = pal.text_secondary().hsla();
        let target = if back { Page::Home } else { Page::All };
        h_flex()
            .id(id)
            .h(px(28.))
            .px(px(8.))
            .gap(px(4.))
            .items_center()
            .rounded(px(6.))
            // (room for the keyboard ring, which is the border's colour)
            .border_2()
            .border_color(transparent_black())
            .cursor_pointer()
            .track_focus(&self.page_focus)
            .focus_visible(move |s| s.border_color(ring))
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .when(back, |d| d.child(Icon::new(L::ChevronLeft).size(px(12.)).text_color(secondary)))
            .child(div().text_size(px(12.)).child(label))
            .when(!back, |d| d.child(Icon::new(L::ChevronRight).size(px(12.)).text_color(secondary)))
            .on_click(cx.listener(move |this, _, window, cx| this.set_page(target, window, cx)))
    }

    // ------------------------------------------------------------------ the home page

    pub(super) fn render_home(&mut self, pal: Palette, theme: ThemePalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.apps_ready {
            let text = if self.apps_failed && !self.loading { t!("Uygulama listesi okunamadı. Yeniden denemek için paneli kapatıp açın.") } else { t!("Uygulamalar yükleniyor…") };
            return div().px(px(PAD)).text_color(pal.text_secondary().hsla()).child(text).into_any_element();
        }
        let secondary = pal.text_secondary().hsla();
        let text = pal.text().hsla();
        let (hover, sel_fill) = (pal.hover_fill().hsla(), theme.selection);

        // what the window's height pays for: the grid's rows, the recommended rows (the room of a section that has not been read yet is
        // kept, so that the grid does not grow when it arrives)
        let pins = self.pin_cells.len();
        let for_layout = if !self.recs_wanted {
            0
        } else if self.recs_loaded {
            self.recs.len()
        } else {
            page::REC_MAX
        };
        let layout = page::home_layout(page::page_room(f32::from(window.viewport_size().height), self.notice.is_some()), pins, for_layout);
        self.layout = layout;
        self.grid_page = page::clamp_page(self.grid_page, layout.pages);
        if let Some(Spot::Rec(j)) = self.spot
            && j >= self.rec_shown()
        {
            self.spot = None;
        }

        // ---- the pinned grid: one page of it, rows of six tiles
        let range = page::page_range(self.grid_page, layout.per_page, pins);
        let shown: Vec<usize> = range.collect();
        let mut grid = v_flex().id("pins").relative().w_full().flex_shrink_0().h(px(layout.rows as f32 * page::TILE_H)).on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| {
            if this.layout.pages < 2 || !this.live() {
                return;
            }
            let dy = f32::from(ev.delta.pixel_delta(px(20.)).y);
            let turn = this.wheel.feed(dy, crate::dock::backdrop::now_ms());
            if turn != 0 {
                let to = page::turned(this.grid_page, this.layout.pages, turn);
                if to != this.grid_page {
                    this.grid_page = to;
                    // (a cursor on a pin of the page that left would start an app that is not on the screen)
                    this.spot = page::spot_after_turn(this.spot);
                    cx.notify();
                }
            }
        }));
        if pins == 0 {
            grid = grid.child(div().h(px(44.)).flex().items_center().text_color(secondary).child(t!("Henüz sabitlenmiş uygulama yok. Bir uygulamaya sağ tıklayıp (ya da Menü tuşuna basıp) “Başlat'a sabitle”yi seçin.")));
        }
        for row in shown.chunks(page::COLS) {
            let mut line = h_flex().h(px(page::TILE_H)).flex_shrink_0();
            for &i in row {
                let Some(app) = self.pin_cells.get(i).and_then(|&a| self.apps.get(a)).cloned() else { continue };
                let selected = self.spot == Some(Spot::Pin(i));
                let icon = self.app_picture(&app, page::TILE_ICON, pal, cx);
                let (a1, a2) = (app.clone(), app.clone());
                line = line.child(
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
                        .child(icon)
                        // the name under the icon: up to two centred lines
                        .child(div().w_full().h(px(30.)).px(px(6.)).text_size(px(12.)).line_height(px(15.)).text_center().line_clamp(2).child(app.name.clone()))
                        .on_click(cx.listener(move |this, _, _, cx| this.launch_app(a1.clone(), cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(a2.clone(), ev.position, cx))),
                );
            }
            grid = grid.child(line);
        }
        // the dots at the grid's right edge, in the padding: one for each page
        if layout.pages > 1 {
            let mut dots = v_flex().absolute().right(px(-(page::SIDE_PAD - 4.0))).top_0().h_full().justify_center();
            for p in 0..layout.pages {
                let on = p == self.grid_page;
                dots = dots.child(
                    div()
                        .id(("pins-page", p))
                        .size(px(16.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .child(div().size(px(6.)).rounded_full().bg(if on { text } else { secondary.opacity(0.45) }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.live() {
                                this.grid_page = p;
                                this.spot = page::spot_after_turn(this.spot);
                                cx.notify();
                            }
                        })),
                );
            }
            grid = grid.child(dots);
        }

        // ---- "Önerilenler"
        let recommended = (layout.rec_rows > 0).then(|| {
            let block = v_flex().w_full().mt(px(page::SECTION_GAP)).h(px(page::HEAD_H + layout.rec_rows as f32 * page::REC_ROW_H)).flex_shrink_0();
            if !self.recs_loaded {
                return block.into_any_element();
            }
            let shown_n = self.rec_shown();
            let entries: Vec<RecEntry> = self.recs.iter().take(shown_n).cloned().collect();
            let mut col = v_flex().w_full();
            for (r, pair) in entries.chunks(page::REC_COLS).enumerate() {
                let mut line = h_flex().h(px(page::REC_ROW_H)).w_full();
                for (c, entry) in pair.iter().enumerate() {
                    let j = r * page::REC_COLS + c;
                    let selected = self.spot == Some(Spot::Rec(j));
                    let icon = match &entry.pick.source {
                        Source::File(path) => self.picture(rec_key(path), IconSource::File(path.clone()), &entry.pick.name, 32.0, pal, cx),
                        Source::App(id) => self.app_picture(&AppEntry { id: id.clone(), name: entry.pick.name.clone() }, 32.0, pal, cx),
                    };
                    line = line.child(
                        h_flex()
                            .id(("rec", j))
                            .relative()
                            .flex_1()
                            .min_w_0()
                            .h(px(page::REC_ROW_H))
                            .px(px(8.))
                            .gap(px(12.))
                            .items_center()
                            .rounded(px(8.))
                            .cursor_pointer()
                            .when(selected, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
                            .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                            .child(div().size(px(32.)).flex_shrink_0().flex().items_center().justify_center().child(icon))
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_size(px(13.)).truncate().child(entry.pick.name.clone()))
                                    .child(div().text_size(px(12.)).text_color(secondary).truncate().child(entry.line.clone())),
                            )
                            .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| this.activate_rec(j, if ev.modifiers().control { How::ShowInFolder } else { How::Open }, cx))),
                    );
                }
                if pair.len() < page::REC_COLS {
                    line = line.child(div().flex_1());
                }
                col = col.child(line);
            }
            block.child(div().h(px(page::HEAD_H)).flex().items_center().child(heading(t!("Önerilenler"), text))).child(col).into_any_element()
        });

        v_flex()
            .size_full()
            .px(px(PAD))
            .child(h_flex().h(px(page::HEAD_H)).flex_shrink_0().items_center().justify_between().child(heading(t!("Sabitlenenler"), text)).child(self.page_button("start-all", t!("Tümü"), false, pal, theme, cx)))
            .child(grid)
            .children(recommended)
            .into_any_element()
    }

    // ------------------------------------------------------------------ the all-apps page

    pub(super) fn render_all(&mut self, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        if !self.apps_ready {
            let text = if self.apps_failed && !self.loading { t!("Uygulama listesi okunamadı. Yeniden denemek için paneli kapatıp açın.") } else { t!("Uygulamalar yükleniyor…") };
            return div().px(px(PAD)).text_color(pal.text_secondary().hsla()).child(text).into_any_element();
        }
        let n = self.rows.len();
        let list = uniform_list(
            "start-all-apps",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let pal = this.island.borrow().palette(super::super::backdrop::now_ms());
                let theme = theme_of(pal);
                let (hover, sel_fill) = (pal.hover_fill().hsla(), theme.selection);
                let wanted: Vec<(usize, Row)> = range.filter_map(|r| this.rows.get(r).cloned().map(|row| (r, row))).collect();
                wanted
                    .into_iter()
                    .map(|(r, row)| match row {
                        Row::Header(h) => div()
                            .id(("start-header", r))
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .px(px(10.))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.text_secondary().hsla())
                            .child(h)
                            .into_any_element(),
                        Row::App(ai) => {
                            let Some(app) = this.apps.get(ai).cloned() else { return div().h(px(ROW_H)).into_any_element() };
                            let selected = this.cursor == Some(Cursor::App(ai));
                            let icon = this.app_picture(&app, 24.0, pal, cx);
                            let (a1, a2) = (app.clone(), app.clone());
                            h_flex()
                                .id(("start-app", r))
                                .relative()
                                .h(px(ROW_H))
                                .px(px(10.))
                                .gap_3()
                                .items_center()
                                .rounded(px(8.))
                                .cursor_pointer()
                                .when(selected, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
                                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                                .child(icon)
                                .child(div().flex_1().min_w_0().truncate().text_sm().child(app.name.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| this.launch_app(a1.clone(), cx)))
                                .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(a2.clone(), ev.position, cx)))
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        v_flex()
            .size_full()
            .px(px(PAD))
            .child(h_flex().h(px(page::HEAD_H)).flex_shrink_0().items_center().justify_between().child(heading(t!("Tüm uygulamalar"), pal.text().hsla())).child(self.page_button("start-back", t!("Geri"), true, pal, theme, cx)))
            .child(div().h(px(8.)).flex_shrink_0())
            .child(list)
            .child(div().h(px(12.)).flex_shrink_0())
            .into_any_element()
    }

    // ------------------------------------------------------------------ the bottom row

    /// The bottom row: the account picture and name at the left, the gear (Windows' Settings) and the power button at the right.
    pub(super) fn render_footer(&mut self, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
        let secondary = pal.text_secondary().hsla();
        let ring = theme.accent;
        let initial = self.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let button = |id: &'static str, icon: L, on: bool, focus: &FocusHandle, tip: &'static str| {
            div()
                .id(id)
                .size(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                // (room for the keyboard ring, which is the border's colour)
                .border_2()
                .border_color(transparent_black())
                .cursor_pointer()
                .track_focus(focus)
                .focus_visible(move |s| s.border_color(ring))
                .when(on, |d| d.bg(pressed))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .active(move |s| s.bg(pressed))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
                .child(Icon::new(icon).size(px(18.)).text_color(pal.text().hsla()))
        };
        // the account picture (round), or the initial in a disc until it has come or when the account has none
        let avatar = match &self.avatar {
            // (the radius is on the picture itself: a parent's overflow_hidden clips to a rectangle and would leave it square)
            Some(img) => div().size(px(AVATAR)).flex_shrink_0().child(gpui_kit::img(img.clone()).size(px(AVATAR)).rounded_full()).into_any_element(),
            None => div()
                .size(px(AVATAR))
                .flex_shrink_0()
                .rounded_full()
                .bg(pal.field_fill().hsla())
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(secondary)
                .child(initial)
                .into_any_element(),
        };
        let (settings_focus, power_focus) = (self.settings_focus.clone(), self.power_focus.clone());
        h_flex()
            .h(px(BOTTOM_H))
            .flex_shrink_0()
            .px(px(PAD))
            .gap_2()
            .items_center()
            .child(h_flex().flex_1().min_w_0().gap_3().items_center().child(avatar).child(div().min_w_0().truncate().text_size(px(14.)).child(self.user.clone())))
            .child(button("start-settings", L::Settings, false, &settings_focus, t!("Ayarlar")).on_click(cx.listener(|this, _, _, cx| {
                if !this.live() {
                    return;
                }
                // (Windows' Settings, as the gear of Windows' own Start does; rustyfiles' settings are in its own window)
                crate::dock::panel_parts::open_uri("ms-settings:");
                this.finish(cx);
            })))
            .child(button("start-power", L::Power, self.power != PowerMenu::Closed, &power_focus, t!("Güç")).on_click(cx.listener(|this, _, _, cx| {
                if !this.live() {
                    return;
                }
                this.app_menu = None;
                this.power_hi = 0;
                let toggled = this.power.toggled();
                this.set_power(toggled, cx);
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::{AVATAR, Words, avatar_px, keep_icon_on_hide, rec_key, rec_line};
    use rf_win::dock::startrecent::When;
    use rf_win::toasts::Ago;

    fn tr_tr(s: &'static str) -> &'static str {
        s
    }

    fn tr_en(s: &'static str) -> &'static str {
        crate::i18n_en::EN.iter().find(|(k, _)| *k == s).map(|(_, v)| *v).unwrap_or_else(|| panic!("no English pair for {s:?}"))
    }

    const TR: Words = Words { tr: tr_tr, pct: |p| format!("%{p}") };
    const EN: Words = Words { tr: tr_en, pct: |p| format!("{p}%") };

    #[test]
    fn the_second_line_of_a_recommended_entry_is_said_in_both_languages() {
        let say = |w, lang: Words| rec_line(w, "12.11.2026", lang);
        assert_eq!(say(When::Ago(Ago::Minutes(5)), TR), "5 dk önce");
        assert_eq!(say(When::Ago(Ago::Minutes(5)), EN), "5 min ago");
        assert_eq!(say(When::Ago(Ago::Hours(3)), TR), "3 sa önce");
        assert_eq!(say(When::Ago(Ago::Hours(3)), EN), "3 h ago");
        assert_eq!(say(When::Ago(Ago::Now), TR), "Az önce");
        assert_eq!(say(When::Ago(Ago::Now), EN), "Just now");
        assert_eq!(say(When::Ago(Ago::Yesterday), TR), "dün");
        assert_eq!(say(When::Ago(Ago::Yesterday), EN), "yesterday");
        assert_eq!(say(When::Ago(Ago::Date), TR), "12.11.2026");
        assert_eq!(say(When::Ago(Ago::Date), EN), "12.11.2026", "the date is already in the user's own format");
        assert_eq!(say(When::Added, TR), "Son eklenen");
        assert_eq!(say(When::Added, EN), "Recently added");
    }

    #[test]
    fn the_panels_words_have_english_pairs() {
        for key in ["Önerilenler", "Tümü", "Geri", "Sabitlenenler", "Tüm uygulamalar", "Son eklenen", "Az önce", "önce", "dk", "sa", "dün", "Başlat'ta önerilenler"] {
            assert!(crate::i18n_en::EN.iter().any(|(k, _)| *k == key), "no English pair for {key:?}");
        }
    }

    #[test]
    fn the_account_picture_is_asked_for_at_the_size_it_is_drawn() {
        assert_eq!(AVATAR, 28.0);
        assert_eq!(avatar_px(1.0), 28);
        assert_eq!(avatar_px(1.5), 42);
        assert_eq!(avatar_px(2.0), 56);
        assert_eq!(avatar_px(0.0), 28, "a scale that was not read is 100 %");
    }

    #[test]
    fn the_recommended_section_is_on_by_default_and_a_file_from_before_it_reads_as_on() {
        use crate::settings::Settings;
        assert!(Settings::default().dock_start_recommended);
        assert!(Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().dock_start_recommended);
        let off = Settings { dock_start_recommended: false, ..Default::default() };
        let back = Settings::parse_lenient_pub(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_start_recommended, "off survives a save and a load");
        assert!(Settings::parse_lenient_pub("dock_start_recommended = 3\n").unwrap().dock_start_recommended, "a wrong value falls back to the default");
    }

    #[test]
    fn a_hidden_panel_keeps_the_apps_and_the_current_recommendations_pictures_only() {
        let key = |p: &str| rec_key(std::path::Path::new(p));
        let now: std::collections::HashSet<String> = [key(r"C:\a\New.docx")].into_iter().collect();
        assert!(keep_icon_on_hide("app:x", &now));
        assert!(keep_icon_on_hide(&key(r"c:\A\new.DOCX"), &now), "the key is case-insensitive like the path");
        assert!(!keep_icon_on_hide(&key(r"C:\a\Old.docx"), &now), "a document that is no longer recommended is let go");
        assert!(!keep_icon_on_hide(r"file:c:\found.txt", &now), "a found file's picture is let go as before");
    }
}
