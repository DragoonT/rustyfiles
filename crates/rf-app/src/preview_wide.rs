//! The preview pane laid out for a wide and low box: the pane under the file list (setting `preview_position` = bottom).
//!
//! The pane at the right is a tall column (picture, name, text, properties, hash). Here the same things stand side by side:
//! the picture or icon at the left, the name and the properties in columns beside it, a text / archive / shortcut block
//! after them when the file has one, and the hash action in a column at the right (or under the properties when the pane
//! is not wide enough for a column). A video's player bar stands under its picture. What goes where, and how many columns the
//! properties get, is decided by the pure rules in `rf_core::previewpane`; this file only draws them.
//!
//! This is a child module of `preview` (see the `#[path]` line there): it uses the pane's own state, so a video, a Windows
//! preview handler, the text and the properties behave exactly as at the right.

use super::*;
use rf_core::previewpane::{self as pp, VisualKind};

impl Preview {
    /// The pane for a box `size` = (width, height) in logical pixels (its top border is already taken off the height).
    pub(super) fn render_wide(&mut self, path: PathBuf, e: Entry, size: (f32, f32), window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = Palette::for_window(window);
        let cat = icons::category(&e);
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // (the same decisions as the pane at the right: a Windows handler, our player, else the thumbnail or the icon)
        let handler = !self.occluded && self.wants_handler(&path, &e, cx);
        let playable = !handler && self.playable(&path, &e);
        let video = matches!(cat, Category::Video);
        let video_on = playable && video && !self.occluded && self.player.as_ref().is_some_and(|pl| pl.path == path && pl.started);

        let text = self.text.as_ref().filter(|(tp, _)| *tp == path).map(|(_, t)| t.clone());
        let has_extra = self.extra.as_ref().is_some_and(|(xp, _)| *xp == path);
        let has_block = text.is_some() || has_extra;
        let kind = if has_block {
            VisualKind::Compact
        } else if handler || (playable && video) {
            VisualKind::Wide
        } else if playable {
            // audio: the player bar stands under the picture and needs a box wide enough for it
            VisualKind::Audio
        } else {
            VisualKind::Picture
        };
        let hash_el = self.wide_hash(&path, &e, &p, cx);
        let mut inner = pp::inner(size.0, size.1, kind);
        if hash_el.is_none() {
            // nothing for the actions column: its room goes to the properties
            if let Some(a) = inner.actions_w.take() {
                inner.props_w += a + pp::WIDE_GAP;
            }
        }

        // the hash action stands in the right column when there is one, else under the properties
        let (hash_under, hash_side) = if inner.actions_w.is_some() { (None, hash_el) } else { (hash_el, None) };

        // ---- the picture, and under it the player bar
        let bar_h = if playable { 36. + 8. } else { 0. };
        let (vw, vh) = (inner.visual_w, (inner.visual_h - bar_h).max(40.));
        let visual: AnyElement = if video_on {
            let me = cx.entity().downgrade();
            let hp = path.clone();
            div()
                .w(px(vw))
                .h(px(vh))
                .flex_shrink_0()
                .rounded(theme::RADIUS)
                .bg(gpui_kit::black())
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let _ = me.update(cx, |pv, _| pv.place_video(hp.clone(), bounds, window));
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                )
                .into_any_element()
        } else if handler {
            let me = cx.entity().downgrade();
            let hp = path.clone();
            div()
                .w(px(vw))
                .h(px(vh))
                .flex_shrink_0()
                .rounded(theme::RADIUS)
                .bg(p.hover)
                .child(
                    canvas(
                        move |bounds, window, cx| {
                            let _ = me.update(cx, |pv, _| pv.place_host(hp.clone(), bounds, window));
                        },
                        |_, _, _, _| {},
                    )
                    .size_full(),
                )
                .into_any_element()
        } else {
            self.hide_host();
            let thumb = self.thumb_or_hold(&dir, &e, PREVIEW_PX, cx);
            let inside: AnyElement = match thumb {
                Some(im) => img(im).w_full().h_full().object_fit(ObjectFit::Contain).into_any_element(),
                None => Icon::new(icons::icon_for(cat)).size(px((vh * 0.5).clamp(32., 72.))).text_color(theme::kind_color(cat, p.dark)).into_any_element(),
            };
            div()
                .w(px(vw))
                .h(px(vh))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .overflow_hidden()
                .rounded(theme::RADIUS)
                .bg(p.hover)
                .child(inside)
                .into_any_element()
        };
        let visual_col = v_flex()
            .w(px(vw))
            .flex_shrink_0()
            .h_full()
            .gap_2()
            .child(visual)
            .when(playable, |c| c.child(self.render_player_bar(&p, video, cx)));

        // ---- the name and the properties in columns
        let rows = self.property_rows(&path, &e, cx);
        // with a block of its own after them the properties keep to about half of the room, one or two columns
        let info_w = pp::info_width(inner.props_w, has_block);
        // (a few pixels of safety in the width: the box is measured by arithmetic, not by the layout. The height counts the name, the
        // gaps of the stack and the button under the properties; a value that wraps still makes its column taller, and the box scrolls)
        let plan = pp::property_plan((info_w - 4.).max(0.), pp::props_avail_h(inner.props_h, hash_under.is_some()), rows.len());
        let columns = plan.columns.iter().map(|r| {
            v_flex()
                .w(px(plan.col_w))
                .flex_shrink_0()
                .gap(px(pp::PROP_GAP))
                .children(rows[r.clone()].iter().map(|(k, v)| prop_row(&p, k, v.clone())))
        });
        // a name too long for its line is cut with an ellipsis; its whole is in a tooltip
        let full_name = SharedString::from(e.name.to_string());
        let cut = pp::name_may_be_cut(e.name.chars().count(), if has_block { info_w } else { inner.props_w });
        let info = v_flex()
            .id("pv-wide-info")
            .min_w_0()
            .h_full()
            .gap_1p5()
            .overflow_y_scroll()
            .when(has_block, |d| d.w(px(info_w)).flex_shrink_0())
            .when(!has_block, |d| d.flex_1())
            .child(
                div()
                    .id("pv-wide-name")
                    .h(px(pp::TITLE_H))
                    .flex_shrink_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(full_name.clone())
                    .when(cut, |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(full_name.clone()).build(w, cx))),
            )
            .child(h_flex().gap(px(pp::PROP_COL_GAP)).items_start().children(columns))
            .children(hash_under);

        // ---- a text / archive / shortcut block after the properties
        let block: Option<AnyElement> = if let Some(text) = text {
            let md = is_markdown(&e);
            Some(
                div()
                    .id("preview-text")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p_2()
                    .rounded(theme::RADIUS_SM)
                    .bg(p.tile_bg)
                    .border_1()
                    .border_color(p.border)
                    .text_xs()
                    .when(!md, |d| d.font_family("Cascadia Mono").text_color(p.text_secondary).child(text.clone()))
                    .when(md, |d| d.child(gpui_kit::component::text::TextView::markdown("pv-md", text.clone())))
                    .into_any_element(),
            )
        } else {
            self.extra
                .as_ref()
                .filter(|(xp, _)| *xp == path)
                .map(|(_, x)| div().flex_1().min_w_0().h_full().overflow_hidden().child(render_extra_sized(&p, x, true)).into_any_element())
        };

        // ---- the actions column at the right (wide panes only)
        let actions = inner.actions_w.zip(hash_side).map(|(w, el)| v_flex().id("pv-wide-actions").w(px(w)).flex_shrink_0().h_full().overflow_y_scroll().child(el));

        h_flex()
            .id("preview-wide")
            .size_full()
            .p_3()
            .gap_4()
            .items_start()
            .overflow_hidden()
            .child(visual_col)
            .child(info)
            .children(block)
            .children(actions)
            .into_any_element()
    }

    /// The hash action of the pane: the button, or the panel once it was pressed. Folders have none.
    fn wide_hash(&mut self, path: &Path, e: &Entry, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if e.is_dir() {
            return None;
        }
        if let Some(h) = self.hash.clone() {
            return Some(div().w_full().child(h).into_any_element());
        }
        let path2 = path.to_path_buf();
        Some(
            div()
                .id("hash-btn")
                .px_3()
                .py_1p5()
                .rounded(theme::RADIUS_SM)
                .bg(p.hover)
                .hover(|s| s.bg(p.pressed))
                .cursor_pointer()
                .text_sm()
                .child(t!("Hash hesapla (MD5, SHA…)"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let pth = path2.clone();
                    this.hash = Some(cx.new(|cx| crate::hash_panel::HashPanel::new(pth, cx)));
                    cx.notify();
                }))
                .into_any_element(),
        )
    }
}
