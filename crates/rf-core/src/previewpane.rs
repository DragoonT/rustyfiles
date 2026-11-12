//! The preview pane's two places: at the right of the file list (the default) or at the bottom, under the list.
//!
//! Everything here is plain arithmetic on logical pixels, so the rules can be tested without a window: whether the pane fits, the
//! heights of the list and of the pane at the bottom, how far a drag may go so that the list keeps a usable size, the layout inside
//! a wide and low pane (picture, properties, actions) and which properties stand in which column.
//!
//! The right position keeps the exact rule the window always had (`right_fits`); the tests pin it. Its rectangles are the window's
//! own flexbox, which this change does not touch, so there is nothing to model for them here.

use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Where the preview pane sits. Stored in the settings as "right" or "bottom"; anything else reads as the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum PreviewPos {
    #[default]
    Right,
    Bottom,
}

impl PreviewPos {
    pub fn is_right(&self) -> bool {
        matches!(self, PreviewPos::Right)
    }
    pub fn key(self) -> &'static str {
        match self {
            PreviewPos::Right => "right",
            PreviewPos::Bottom => "bottom",
        }
    }
}

impl From<String> for PreviewPos {
    fn from(s: String) -> Self {
        // a hand-edited or newer file with a value we do not know must not break the whole settings file
        if s.eq_ignore_ascii_case("bottom") { PreviewPos::Bottom } else { PreviewPos::Right }
    }
}

impl From<PreviewPos> for String {
    fn from(p: PreviewPos) -> Self {
        p.key().to_string()
    }
}

// ---------------------------------------------------------------- limits

/// The pane at the right: the width the file list keeps at least (the pane's own width range lives in the window, as it always did).
pub const LIST_MIN_W: f32 = 380.;

/// The pane at the bottom: its height range and the height it starts with.
pub const BOTTOM_MIN_H: f32 = 120.;
pub const BOTTOM_MAX_H: f32 = 520.;
pub const BOTTOM_DEFAULT_H: f32 = 200.;
/// The file list keeps at least this height (about six rows and the header), whatever the pane asks for.
pub const LIST_MIN_H: f32 = 180.;
/// Narrower than this the bottom pane has no room for the widest thing it has to show: an icon, a column of properties
/// (`PROP_MIN_COL_W`) and a text block (`BLOCK_MIN_W`) side by side. It is not shown (as the right pane is not shown in a narrow window).
pub const BOTTOM_MIN_W: f32 = 560.;

/// The pane at the right fits when the list keeps its minimum width. `viewport_w` is the window, `side_w` the sidebar if it is shown.
/// (This is the rule the window used before the pane could move; it stays as it was.)
pub fn right_fits(viewport_w: f32, side_w: f32, pane_w: f32) -> bool {
    viewport_w >= pane_w + LIST_MIN_W + side_w
}

/// The height the file list(s) above the bottom pane keep at least. In a split tab whose two panes are stacked there are two lists in
/// the area above the pane, and each of them needs its usable height.
pub fn list_min_h(stacked: bool) -> f32 {
    if stacked { 2. * LIST_MIN_H } else { LIST_MIN_H }
}

/// The tallest the bottom pane may be in an area `area_h` high: the list(s) keep `list_min_h`.
pub fn bottom_max(area_h: f32, stacked: bool) -> f32 {
    (area_h - list_min_h(stacked)).min(BOTTOM_MAX_H)
}

/// The bottom pane fits when it can be at least `BOTTOM_MIN_H` high with the list(s) above keeping their minimum, and wide enough.
pub fn bottom_fits(area_w: f32, area_h: f32, stacked: bool) -> bool {
    area_w >= BOTTOM_MIN_W && bottom_max(area_h, stacked) >= BOTTOM_MIN_H
}

/// A stored height made safe: finite and inside the range (a hand-edited file may hold anything).
pub fn stored_height(h: f32) -> f32 {
    if h.is_finite() { h.clamp(BOTTOM_MIN_H, BOTTOM_MAX_H) } else { BOTTOM_DEFAULT_H }
}

/// The height the bottom pane gets for the wish `want` in an area `area_h` high, or None when it does not fit.
pub fn bottom_height(want: f32, area_h: f32, area_w: f32, stacked: bool) -> Option<f32> {
    if !bottom_fits(area_w, area_h, stacked) {
        return None;
    }
    Some(stored_height(want).min(bottom_max(area_h, stacked)))
}

/// Splitter arithmetic: the pointer is at `pointer_y` while the dragged edge belongs to a pane that ends at `area_bottom`; the pane's
/// height is the distance between them, kept inside the limits for an area `area_h` high (in a window too small for any pane: the smallest).
pub fn height_from_drag(area_bottom: f32, pointer_y: f32, area_h: f32, stacked: bool) -> f32 {
    let max = bottom_max(area_h, stacked).max(BOTTOM_MIN_H);
    stored_height(area_bottom - pointer_y).min(max)
}

// ---------------------------------------------------------------- the two heights

/// The splitter is a thin strip that straddles the pane's top edge: half of it lies over the list. It starts a drag.
pub const SPLITTER: f32 = 6.;

/// How the area (the row under the command bar, right of the sidebar) is shared at the bottom position: the list(s) above, the pane under.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BottomLayout {
    /// The height of what stands above the pane (the file list(s) and the shelf): the whole area when there is no pane.
    pub list_h: f32,
    /// The pane's height, or None when it does not fit. The pane always spans the whole width of the area.
    pub pane_h: Option<f32>,
}

/// The window takes the pane's height from here every frame (the list gets what is left, which the flexbox gives it by itself).
pub fn bottom_layout(area_w: f32, area_h: f32, want: f32, stacked: bool) -> BottomLayout {
    match bottom_height(want, area_h, area_w, stacked) {
        Some(h) => BottomLayout { list_h: area_h - h, pane_h: Some(h) },
        None => BottomLayout { list_h: area_h, pane_h: None },
    }
}

// ---------------------------------------------------------------- inside the wide pane

/// Padding around the content of the bottom pane.
pub const WIDE_PAD: f32 = 12.;
/// Gap between the picture, the properties and the actions.
pub const WIDE_GAP: f32 = 16.;
/// From this pane width on the actions (hash) stand in a column at the right; narrower, under the properties.
pub const ACTIONS_RIGHT_FROM_W: f32 = 1100.;
pub const ACTIONS_W: f32 = 200.;
const VISUAL_MIN_W: f32 = 96.;
/// An audio file's picture box also holds the player bar (button, seek line, time), which needs about this much.
pub const AUDIO_MIN_W: f32 = 220.;
/// A text / archive / shortcut block beside the properties keeps at least this much width.
pub const BLOCK_MIN_W: f32 = 160.;
const VISUAL_MAX_W: f32 = 280.;
const VISUAL_WIDE_MAX_W: f32 = 640.;

/// What the box at the left shows, which decides how wide it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisualKind {
    /// A thumbnail or the type icon: about 4:3.
    Picture,
    /// A video, or a Windows preview handler (a document page): needs room, 16:9 at least.
    Wide,
    /// The file has a text, archive or shortcut block of its own beside the properties: the picture is only an icon.
    Compact,
    /// An audio file: a picture or icon about 4:3 with the player bar under it, so the box must be wide enough for the bar.
    Audio,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Inner {
    /// The picture or icon box at the left: it is as high as the pane's content.
    pub visual_w: f32,
    pub visual_h: f32,
    /// The name and the properties (and the text block, if any), beside the picture.
    pub props_w: f32,
    pub props_h: f32,
    /// Width of the actions column at the right, or None when they stand under the properties.
    pub actions_w: Option<f32>,
}

/// The layout inside a bottom pane `w` x `h` (its own padding is taken off below).
pub fn inner(w: f32, h: f32, kind: VisualKind) -> Inner {
    let content_h = (h - 2. * WIDE_PAD).max(48.);
    let actions_w = (w >= ACTIONS_RIGHT_FROM_W).then_some(ACTIONS_W);
    // The properties always keep a column of their own: the box at the left may take what is left of that, never more.
    let reserved = 2. * WIDE_PAD + WIDE_GAP + PROP_MIN_COL_W + actions_w.map_or(0., |a| a + WIDE_GAP);
    let visual_w = match kind {
        // a picture box about 4:3, never more than a third of the pane
        VisualKind::Picture => (content_h * 4. / 3.).clamp(VISUAL_MIN_W, VISUAL_MAX_W).min((w * 0.34).max(VISUAL_MIN_W)),
        // a video or a document page wants width: 16:9 of the height, or two fifths of the pane, whichever is more (not over 55 %)
        VisualKind::Wide => (content_h * 16. / 9.).max(w * 0.4).clamp(VISUAL_MIN_W, VISUAL_WIDE_MAX_W).min((w * 0.55).max(VISUAL_MIN_W)),
        VisualKind::Compact => VISUAL_MIN_W,
        VisualKind::Audio => (content_h * 4. / 3.).clamp(VISUAL_MIN_W, VISUAL_MAX_W).max(AUDIO_MIN_W),
    }
    .min((w - reserved).max(VISUAL_MIN_W));
    let mut props_w = w - 2. * WIDE_PAD - visual_w - WIDE_GAP;
    if let Some(a) = actions_w {
        props_w -= a + WIDE_GAP;
    }
    Inner { visual_w, visual_h: content_h, props_w: props_w.max(0.), props_h: content_h, actions_w }
}

/// The width of the name-and-properties block when a text / archive / shortcut block stands after it (`has_block`): about half of the
/// room, a column at least, and the block keeps `BLOCK_MIN_W`. Without a block the properties have all of `props_w`.
pub fn info_width(props_w: f32, has_block: bool) -> f32 {
    if !has_block {
        return props_w;
    }
    (props_w * 0.5).max(PROP_MIN_COL_W).min(props_w - WIDE_GAP - BLOCK_MIN_W).max(0.)
}

/// The name line above the properties, and the gap the stack puts between its parts.
pub const TITLE_H: f32 = 24.;
pub const STACK_GAP: f32 = 6.;
/// The hash button standing under the properties: two paddings of 6 and one line of 20.
pub const HASH_BTN_H: f32 = 32.;

/// The height the property columns may have in a block `props_h` high that holds the name, the columns and (under them) the hash button.
pub fn props_avail_h(props_h: f32, hash_under: bool) -> f32 {
    let button = if hash_under { STACK_GAP + HASH_BTN_H } else { 0. };
    (props_h - TITLE_H - STACK_GAP - button).max(0.)
}

/// Whether a name of `chars` characters may not fit on the one line of a block `width` wide (text_sm, semibold: about 8 px a character
/// at most). The window then adds a tooltip with the whole name.
pub fn name_may_be_cut(chars: usize, width: f32) -> bool {
    chars as f32 * 8. > width
}

/// One line of a property (text_xs, as in the pane at the right) and the gap between lines.
pub const PROP_ROW_H: f32 = 16.;
pub const PROP_GAP: f32 = 6.;
/// A column of properties needs about this much: the 92 px label and a value.
pub const PROP_MIN_COL_W: f32 = 240.;
pub const PROP_COL_GAP: f32 = 20.;
pub const PROP_MAX_COLS: usize = 4;

#[derive(Clone, Debug, PartialEq)]
pub struct PropPlan {
    /// Index ranges into the property list, one per column, in order.
    pub columns: Vec<Range<usize>>,
    pub col_w: f32,
    /// More rows than the height shows (the box then scrolls).
    pub overflow: bool,
}

/// How many columns the properties get in a box `avail_w` x `avail_h`, and which go in which one. The properties keep their order and
/// fill the columns left to right; as few columns are used as the height needs, never more than the width has room for.
/// When even the widest arrangement is too tall, the rows are shared evenly and the box scrolls.
pub fn property_plan(avail_w: f32, avail_h: f32, count: usize) -> PropPlan {
    if count == 0 {
        return PropPlan { columns: Vec::new(), col_w: avail_w.max(0.), overflow: false };
    }
    let rows_fit = (((avail_h + PROP_GAP) / (PROP_ROW_H + PROP_GAP)).floor() as usize).max(1);
    let by_width = (((avail_w + PROP_COL_GAP) / (PROP_MIN_COL_W + PROP_COL_GAP)).floor() as usize).clamp(1, PROP_MAX_COLS);
    let needed = count.div_ceil(rows_fit);
    let cols = needed.clamp(1, by_width);
    let per = count.div_ceil(cols);
    let columns: Vec<Range<usize>> = (0..cols).map(|c| (c * per).min(count)..((c + 1) * per).min(count)).filter(|r| !r.is_empty()).collect();
    let n = columns.len().max(1) as f32;
    PropPlan { col_w: ((avail_w - (n - 1.) * PROP_COL_GAP) / n).max(0.), overflow: per > rows_fit, columns }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_position_is_stored_as_a_word_and_unknown_words_mean_right() {
        assert_eq!(serde_json::to_string(&PreviewPos::Bottom).unwrap(), "\"bottom\"");
        assert_eq!(serde_json::to_string(&PreviewPos::Right).unwrap(), "\"right\"");
        assert_eq!(serde_json::from_str::<PreviewPos>("\"bottom\"").unwrap(), PreviewPos::Bottom);
        assert_eq!(serde_json::from_str::<PreviewPos>("\"right\"").unwrap(), PreviewPos::Right);
        assert_eq!(serde_json::from_str::<PreviewPos>("\"left\"").unwrap(), PreviewPos::Right);
        assert_eq!(serde_json::from_str::<PreviewPos>("\"\"").unwrap(), PreviewPos::Right);
        assert_eq!(PreviewPos::default(), PreviewPos::Right);
        assert!(PreviewPos::Right.is_right() && !PreviewPos::Bottom.is_right());
    }

    #[test]
    fn the_right_rule_is_the_one_the_window_always_had() {
        // The window used to say `viewport_w >= preview_w + 380 + sidebar`; these are that rule written out as numbers, so a change
        // of the constant or of the comparison shows here.
        assert_eq!(LIST_MIN_W, 380.);
        assert!(right_fits(1000., 232., 300.)); // 912 needed
        assert!(right_fits(912., 232., 300.)); // exactly
        assert!(!right_fits(911.9, 232., 300.));
        assert!(right_fits(680., 0., 300.)); // 680 needed without the sidebar
        assert!(!right_fits(679., 0., 300.));
        assert!(right_fits(1400., 56., 720.)); // 1156 needed
        assert!(!right_fits(1155., 56., 720.));
    }

    #[test]
    fn the_bottom_pane_leaves_the_list_a_usable_height() {
        // 600 high: the list keeps 180, so the pane may be 420 at most (inside the 520 cap)
        assert_eq!(bottom_max(600., false), 420.);
        assert_eq!(bottom_max(1200., false), 520.);
        assert_eq!(bottom_height(200., 600., 1000., false), Some(200.));
        assert_eq!(bottom_height(500., 600., 1000., false), Some(420.));
        // exactly the smallest room: list 180 + pane 120
        assert_eq!(bottom_height(200., 300., 1000., false), Some(120.));
        assert_eq!(bottom_height(200., 299.9, 1000., false), None);
        // too narrow
        assert_eq!(bottom_height(200., 800., BOTTOM_MIN_W - 1., false), None);
        assert_eq!(bottom_height(200., 800., BOTTOM_MIN_W, false), Some(200.));
        // wishes outside the range are brought inside it
        assert_eq!(bottom_height(10., 800., 1000., false), Some(120.));
        assert_eq!(bottom_height(f32::NAN, 800., 1000., false), Some(BOTTOM_DEFAULT_H));
        assert_eq!(stored_height(f32::INFINITY), BOTTOM_DEFAULT_H);
        assert_eq!(stored_height(9999.), 520.);
        // whatever the area, list + pane never exceed it and the list(s) never go below their minimum
        for stacked in [false, true] {
            for area_h in (300..1600).step_by(7) {
                for want in [0., 120., 200., 333., 520., 4000.] {
                    if let Some(h) = bottom_height(want, area_h as f32, 1200., stacked) {
                        assert!((BOTTOM_MIN_H..=BOTTOM_MAX_H).contains(&h));
                        assert!(area_h as f32 - h >= list_min_h(stacked), "stacked {stacked} area {area_h} want {want} gave {h}");
                    }
                }
            }
        }
    }

    #[test]
    fn two_stacked_lists_each_keep_a_usable_height() {
        // A split tab with the panes one above the other has two lists in the area above the pane. In a 500 px area the pane used to be
        // allowed 320 px, which left 90 px for each list (header and two rows); now each keeps LIST_MIN_H.
        assert_eq!(list_min_h(false), 180.);
        assert_eq!(list_min_h(true), 360.);
        assert_eq!(bottom_max(500., true), 140.);
        assert_eq!(bottom_height(300., 500., 1000., true), Some(140.));
        let area = 500.;
        let h = bottom_height(520., area, 1000., true).unwrap();
        assert!((area - h) / 2. >= LIST_MIN_H);
        // the pane needs 360 + 120 = 480 px of area when stacked; below that it is not shown
        assert_eq!(bottom_height(200., 479.9, 1000., true), None);
        assert_eq!(bottom_height(200., 480., 1000., true), Some(120.));
        // a single list (or two side by side) keeps the old rule
        assert_eq!(bottom_height(520., 500., 1000., false), Some(320.));
        // the drag follows the same limit
        assert_eq!(height_from_drag(900., 0., 500., true), 140.);
        assert_eq!(height_from_drag(900., 0., 500., false), 320.);
    }

    #[test]
    fn dragging_the_splitter_up_makes_the_pane_taller() {
        // the area ends at y = 900; the pointer at 700 means a pane 200 high
        assert_eq!(height_from_drag(900., 700., 700., false), 200.);
        assert_eq!(height_from_drag(900., 600., 700., false), 300.);
        // up past the list's minimum: held at the room that is there (700 - 180 = 520, and the cap is 520)
        assert_eq!(height_from_drag(900., 0., 700., false), 520.);
        assert_eq!(height_from_drag(900., 0., 500., false), 320.);
        // down past the bottom: the smallest pane
        assert_eq!(height_from_drag(900., 2000., 700., false), 120.);
        // in a window so small that nothing fits, the smallest pane (it is not shown anyway)
        assert_eq!(height_from_drag(900., 700., 250., false), 120.);
        assert_eq!(height_from_drag(900., 100., 250., false), 120.);
    }

    #[test]
    fn the_bottom_layout_tiles_the_area_and_spans_its_width() {
        for stacked in [false, true] {
            for h in [150., 200., 333., 520.] {
                let l = bottom_layout(900., 900., h, stacked);
                let pane = l.pane_h.unwrap();
                assert_eq!(l.list_h + pane, 900.);
                assert!(l.list_h >= list_min_h(stacked));
            }
        }
        // no room: the list keeps the area and there is no pane
        let l = bottom_layout(1000., 290., 200., false);
        assert_eq!((l.pane_h, l.list_h), (None, 290.));
        let l = bottom_layout(BOTTOM_MIN_W - 1., 900., 200., false);
        assert_eq!(l.pane_h, None);
    }

    #[test]
    fn inside_a_low_pane_the_picture_is_at_the_left_and_the_actions_fit() {
        // 200 high, 1200 wide: picture 176 high and 4:3 wide (234), properties, then an actions column
        let i = inner(1200., 200., VisualKind::Picture);
        assert_eq!(i.visual_h, 176.);
        assert!((i.visual_w - 234.667).abs() < 0.01);
        assert_eq!(i.actions_w, Some(ACTIONS_W));
        assert!((i.props_w - (1200. - 24. - i.visual_w - 16. - 216.)).abs() < 0.01);
        // narrower than 1100: the actions go under the properties and the properties get the room
        let n = inner(900., 200., VisualKind::Picture);
        assert_eq!(n.actions_w, None);
        assert!((n.props_w - (900. - 24. - n.visual_w - 16.)).abs() < 0.01);
        // the picture box is never more than about a third of the pane
        let s = inner(BOTTOM_MIN_W, 400., VisualKind::Picture);
        assert!(s.visual_w <= BOTTOM_MIN_W * 0.34 + 0.01 || s.visual_w == 96.);
        // a very low pane keeps a picture box that can show an icon
        assert!(inner(800., 120., VisualKind::Picture).visual_w >= 96.);
        // nothing is negative at the smallest size, whatever the kind
        for k in [VisualKind::Picture, VisualKind::Wide, VisualKind::Compact, VisualKind::Audio] {
            let t = inner(BOTTOM_MIN_W, 120., k);
            assert!(t.props_w > 0. && t.visual_h > 0., "{k:?}");
        }
    }

    #[test]
    fn a_video_or_document_box_gets_more_width_and_a_text_file_only_an_icon() {
        let pic = inner(1000., 200., VisualKind::Picture).visual_w;
        let wide = inner(1000., 200., VisualKind::Wide).visual_w;
        let compact = inner(1000., 200., VisualKind::Compact).visual_w;
        assert!(wide > pic && pic > compact);
        assert_eq!(compact, 96.);
        // two fifths of 1000, over the 16:9 of 176 (313)
        assert!((wide - 400.).abs() < 0.01);
        // never over 55 % of the pane, and the cap is 640
        assert!(inner(700., 520., VisualKind::Wide).visual_w <= 700. * 0.55 + 0.01);
        assert_eq!(inner(2400., 520., VisualKind::Wide).visual_w, 640.);
        // the properties column always keeps a positive width at a usable pane width
        assert!(inner(600., 200., VisualKind::Wide).props_w > 100.);
    }

    #[test]
    fn at_every_width_the_pane_can_be_shown_at_the_properties_and_blocks_keep_a_readable_column() {
        // The smallest width is what the three things need side by side.
        assert!(BOTTOM_MIN_W >= 2. * WIDE_PAD + VISUAL_MIN_W + WIDE_GAP + PROP_MIN_COL_W + WIDE_GAP + BLOCK_MIN_W);
        for kind in [VisualKind::Picture, VisualKind::Wide, VisualKind::Compact, VisualKind::Audio] {
            for w in (BOTTOM_MIN_W as i32..2600).step_by(11) {
                for h in (BOTTOM_MIN_H as i32..=BOTTOM_MAX_H as i32).step_by(20) {
                    let i = inner(w as f32, h as f32, kind);
                    // (the old layout left a video's properties 149 px at 420 and a text block 28 px)
                    assert!(i.props_w >= PROP_MIN_COL_W, "{kind:?} {w}x{h}: props {}", i.props_w);
                    if kind == VisualKind::Compact {
                        let info = info_width(i.props_w, true);
                        assert!(info >= PROP_MIN_COL_W - 0.01, "{w}x{h}: info {info}");
                        let block = i.props_w - info - WIDE_GAP;
                        assert!(block >= BLOCK_MIN_W - 0.01, "{w}x{h}: block {block}");
                    }
                    assert_eq!(info_width(i.props_w, false), i.props_w);
                }
            }
        }
        // the cases the review named
        assert!(inner(420., 200., VisualKind::Wide).props_w >= PROP_MIN_COL_W);
        // a text block beside a 284 px properties block (the old Compact layout at 420): the block now gets its 160 px or the width is refused
        assert_eq!(info_width(424., true), 240.);
    }

    #[test]
    fn an_audio_box_is_wide_enough_for_the_player_bar() {
        for h in [120., 200., 360., 520.] {
            for w in [BOTTOM_MIN_W, 700., 1000., 1500.] {
                assert!(inner(w, h, VisualKind::Audio).visual_w >= AUDIO_MIN_W, "{w}x{h}");
            }
        }
        // a plain picture at the lowest pane is narrower: that is what the bar could not live in
        assert!(inner(1000., 120., VisualKind::Picture).visual_w < AUDIO_MIN_W);
    }

    #[test]
    fn the_stack_of_name_properties_and_button_is_counted_in_full() {
        // name 24 + gap 6 + columns; with the button under them also a gap 6 and 32
        assert_eq!(props_avail_h(176., false), 146.);
        assert_eq!(props_avail_h(176., true), 108.);
        assert_eq!(props_avail_h(10., true), 0.);
        // the case the review named: a folder (no hash) with 7 properties at pane height 200. The stack needs 24 + 6 + (7*16 + 6*6) = 178
        // against 176 of content: it must not be planned as one column that overflows by 2 px.
        let p = property_plan(600., props_avail_h(176., false), 7);
        let need = |rows: usize| rows as f32 * PROP_ROW_H + rows.saturating_sub(1) as f32 * PROP_GAP;
        for r in &p.columns {
            assert!(TITLE_H + STACK_GAP + need(r.len()) <= 176., "{:?}", p.columns);
        }
        // every plan that does not overflow fits the content height, button or not
        for under in [false, true] {
            for count in 0..30 {
                for h in [120., 176., 300., 496.] {
                    let p = property_plan(900., props_avail_h(h - 24., under), count);
                    if !p.overflow {
                        let tallest = p.columns.iter().map(|r| r.len()).max().unwrap_or(0);
                        let stack = TITLE_H + STACK_GAP + need(tallest) + if under { STACK_GAP + HASH_BTN_H } else { 0. };
                        assert!(stack <= h - 24. + 0.01, "under {under} count {count} h {h}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_long_name_is_marked_for_a_tooltip() {
        assert!(!name_may_be_cut(10, 300.));
        assert!(name_may_be_cut(60, 300.));
        assert!(name_may_be_cut(1, 0.));
    }

    #[test]
    fn properties_use_as_few_columns_as_the_height_needs() {
        // 176 high shows 8 rows (8*16 + 7*6 = 170); 7 properties: one column
        let p = property_plan(600., 176., 7);
        assert_eq!(p.columns, vec![0..7]);
        assert!(!p.overflow);
        // 12 properties need two columns of 6
        let p = property_plan(600., 176., 12);
        assert_eq!(p.columns, vec![0..6, 6..12]);
        assert!(!p.overflow);
        assert_eq!(p.col_w, (600. - 20.) / 2.);
        // 17 properties need three columns, shared evenly (6, 6, 5)
        let p = property_plan(800., 176., 17);
        assert_eq!(p.columns, vec![0..6, 6..12, 12..17]);
        // a narrow box has room for one column only: the rows scroll
        let p = property_plan(250., 176., 12);
        assert_eq!(p.columns, vec![0..12]);
        assert!(p.overflow);
        // width limits the columns, never more than four
        let p = property_plan(2000., 40., 30);
        assert_eq!(p.columns.len(), 4);
        assert!(p.overflow);
        // every property is in exactly one column, in order
        for count in 0..40usize {
            for w in [200., 500., 900., 1600.] {
                for h in [20., 60., 176., 400.] {
                    let p = property_plan(w, h, count);
                    let flat: Vec<usize> = p.columns.iter().flat_map(|r| r.clone()).collect();
                    assert_eq!(flat, (0..count).collect::<Vec<_>>(), "w {w} h {h} count {count}");
                    assert!(p.columns.len() <= PROP_MAX_COLS);
                    assert!(p.col_w >= 0.);
                }
            }
        }
        // nothing to show
        assert!(property_plan(600., 176., 0).columns.is_empty());
        // a box shorter than one row still shows one row per column
        assert_eq!(property_plan(600., 5., 3).columns.len(), 2);
    }

    /// Windowless probe: the shares for common window sizes (run with --nocapture to read the table), and the invariants that must
    /// hold for every size.
    #[test]
    fn layouts_for_common_window_sizes() {
        println!("area        stacked  right fits  bottom: list h / pane h");
        for &(aw, ah) in &[(1280., 640.), (1600., 800.), (1920., 960.), (900., 500.), (700., 320.), (560., 280.), (380., 600.)] {
            for stacked in [false, true] {
                let l = bottom_layout(aw, ah, BOTTOM_DEFAULT_H, stacked);
                println!("{aw}x{ah}  {stacked:7}  {:10}  {} / {:?}", right_fits(aw + 232., 232., 300.), l.list_h, l.pane_h);
                // the list(s) and the pane never exceed the area, and the lists keep their minimum whenever there is a pane
                assert!(l.list_h + l.pane_h.unwrap_or(0.) <= ah + 0.001);
                if l.pane_h.is_some() {
                    assert!(l.list_h >= list_min_h(stacked) && aw >= BOTTOM_MIN_W);
                }
            }
        }
    }
}
