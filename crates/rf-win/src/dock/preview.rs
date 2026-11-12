//! The dock's hover previews (shell plan increment B8): the timing rules of "rest the pointer on a running app's icon and a small
//! preview of its windows opens above it", the arithmetic that puts a pop-up above an icon, and the grid of the preview's cards.
//! All of it pure: the pointer's facts and the time (milliseconds on any steady clock) are arguments, the answers are what to do.
//! The app side (`rf-app/src/dock/hover_preview.rs`) feeds the machine from the icons' hover events and carries the answers out.
//!
//! THE RULES (what a user sees):
//! * the pointer rests on a RUNNING app's icon for [`DWELL_MS`] (400 ms): the preview opens;
//! * while it is open, moving the pointer onto another running app's icon swaps the preview at once (no waiting);
//! * moving from the icon into the preview keeps it open; leaving both (onto nothing, onto a gap, onto an icon that is not running)
//!   closes it after [`GRACE_MS`], and coming back to the icon or the preview inside that time cancels the closing;
//! * a click on an icon closes it and it does not come back until the pointer has left that icon (clicking is an answer, not a
//!   question: a preview popping up under a window that has just come forward would be noise);
//! * a block (a menu is open, the dock hides, the setting is off) closes it at once and keeps it from opening.
//!
//! There is no polling: [`Machine::deadline`] says when the next [`Machine::tick`] is due and the caller sleeps until then.

use super::layout::{Rect, px};

/// How long the pointer must rest on an icon before its preview opens.
pub const DWELL_MS: u64 = 400;
/// How long the preview stays after the pointer has left the icon and the preview.
pub const GRACE_MS: u64 = 300;

/// What the pointer is doing, as the caller sees it. `K` names an app (its dock key).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Input<K> {
    /// The pointer is on the icon of this running app (`Some`), or on nothing a preview belongs to (`None`: a gap, the separator, an
    /// icon that is not running, outside the dock).
    Icon(Option<K>),
    /// The pointer entered (`true`) or left (`false`) the preview window.
    Preview(bool),
    /// A button was pressed on an icon.
    Click,
    /// Something forbids a preview (`true`) or no longer does (`false`).
    Block(bool),
}

/// What the caller has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output<K> {
    Nothing,
    /// Open the preview of this app.
    Open(K),
    /// The preview is open: show this app's instead, without any delay.
    Swap(K),
    Close,
}

#[derive(Clone, Debug)]
pub struct Machine<K> {
    shown: Option<K>,
    /// An icon the pointer rests on and since when (ms), while no preview is open for it.
    dwell: Option<(K, u64)>,
    on_icon: Option<K>,
    in_preview: bool,
    /// While a preview is open and the pointer is on neither it nor its icon: since when (ms).
    away_since: Option<u64>,
    blocked: bool,
    /// After a click: no preview for this app until the pointer has left its icon.
    suppressed: Option<K>,
}

impl<K> Default for Machine<K> {
    fn default() -> Self {
        Machine { shown: None, dwell: None, on_icon: None, in_preview: false, away_since: None, blocked: false, suppressed: None }
    }
}

impl<K: Clone + PartialEq> Machine<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The app whose preview is open.
    pub fn shown(&self) -> Option<&K> {
        self.shown.as_ref()
    }

    pub fn is_blocked(&self) -> bool {
        self.blocked
    }

    fn close(&mut self) -> Output<K> {
        self.dwell = None;
        self.away_since = None;
        self.in_preview = false;
        if self.shown.take().is_some() { Output::Close } else { Output::Nothing }
    }

    /// Closes the preview now, whatever the pointer is doing (the dock hid, the monitor changed, its windows are gone), and forgets what
    /// the pointer was doing. A block stays as it is.
    pub fn dismiss(&mut self) -> Output<K> {
        self.on_icon = None;
        self.close()
    }

    /// A fact about the pointer, at `now_ms`.
    pub fn input(&mut self, now_ms: u64, input: Input<K>) -> Output<K> {
        match input {
            Input::Block(on) => {
                self.blocked = on;
                if on { self.close() } else { Output::Nothing }
            }
            Input::Click => {
                self.suppressed = self.on_icon.clone();
                self.close()
            }
            Input::Preview(inside) => {
                self.in_preview = inside;
                if self.shown.is_some() {
                    if inside {
                        self.away_since = None;
                    } else if self.on_icon != self.shown && self.away_since.is_none() {
                        self.away_since = Some(now_ms);
                    }
                }
                Output::Nothing
            }
            Input::Icon(k) => {
                self.on_icon = k.clone();
                // the pointer left the icon that was clicked: it may ask again
                if self.suppressed.is_some() && self.suppressed != k {
                    self.suppressed = None;
                }
                if self.blocked {
                    return Output::Nothing;
                }
                match (self.shown.clone(), k) {
                    (Some(shown), Some(k)) => {
                        self.away_since = None;
                        self.dwell = None;
                        if k == shown {
                            Output::Nothing
                        } else if self.suppressed.as_ref() == Some(&k) {
                            // (a click on this very icon a moment ago: leave the other preview as it is, it closes with its own grace)
                            Output::Nothing
                        } else {
                            self.shown = Some(k.clone());
                            Output::Swap(k)
                        }
                    }
                    (Some(_), None) => {
                        if !self.in_preview && self.away_since.is_none() {
                            self.away_since = Some(now_ms);
                        }
                        Output::Nothing
                    }
                    (None, Some(k)) => {
                        if self.suppressed.as_ref() == Some(&k) {
                            self.dwell = None;
                        } else if self.dwell.as_ref().map(|(d, _)| d) != Some(&k) {
                            self.dwell = Some((k, now_ms));
                        }
                        Output::Nothing
                    }
                    (None, None) => {
                        self.dwell = None;
                        Output::Nothing
                    }
                }
            }
        }
    }

    /// Time has passed: `now_ms` is at or after [`deadline`](Self::deadline).
    pub fn tick(&mut self, now_ms: u64) -> Output<K> {
        if let Some((k, since)) = self.dwell.clone()
            && !self.blocked
            && now_ms.saturating_sub(since) >= DWELL_MS
        {
            self.dwell = None;
            self.away_since = None;
            self.shown = Some(k.clone());
            return Output::Open(k);
        }
        if let Some(since) = self.away_since
            && now_ms.saturating_sub(since) >= GRACE_MS
        {
            return self.close();
        }
        Output::Nothing
    }

    /// When the next tick is due (ms on the caller's clock), if anything is waiting.
    pub fn deadline(&self) -> Option<u64> {
        let dwell = self.dwell.as_ref().filter(|_| !self.blocked).map(|(_, since)| since + DWELL_MS);
        let away = self.away_since.map(|since| since + GRACE_MS);
        match (dwell, away) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }
}

// ---------------------------------------------------------------- where a pop-up above an icon goes

/// The gap between the top of the dock's glass and the bottom of a pop-up above an icon, logical px. An icon under the pointer grows
/// up to 8 px over the glass's top edge (`rf-app` middle island), so the gap is that and the panels' own 8 px.
pub const ICON_GAP: f32 = 16.0;

/// The rectangle (device px, screen coordinates) of a pop-up of `size` logical px that sits above the dock's glass (`glass_top`, its top
/// edge) and is centred on an icon (`icon_center`, device px), kept inside the monitor's `work` area. A pop-up wider than the work
/// area is narrowed to it; one taller than the room above the dock is shortened to that room. `scale` is the monitor's.
pub fn above_icon(icon_center: i32, glass_top: i32, size: (f32, f32), scale: f32, work: Rect) -> Rect {
    let w = px(size.0, scale).clamp(1, work.width().max(1));
    let room = (glass_top - px(ICON_GAP, scale) - work.top).max(1);
    let h = px(size.1, scale).clamp(1, room);
    let bottom = glass_top - px(ICON_GAP, scale);
    let top = (bottom - h).max(work.top);
    let left = (icon_center - w / 2).clamp(work.left, (work.right - w).max(work.left));
    Rect::from_xywh(left, top, w, h)
}

/// How far an icon under the pointer grows above the top of the dock's glass, logical px (the middle island's magnification).
pub const ICON_GROW: f32 = 8.0;

/// The room above the dock's glass for a pop-up, logical px (what [`above_icon`] shortens a tall pop-up to).
pub fn room_above(glass_top: i32, scale: f32, work: Rect) -> f32 {
    let room = (glass_top - px(ICON_GAP, scale) - work.top).max(1);
    room as f32 / scale.max(0.01)
}

/// Whether the pointer (`cursor`, device px, screen coordinates) is on NEITHER the open preview (`preview`) NOR the dock's middle glass
/// (`glass`, with the part an icon grows over its top edge). The pointer's leaving is otherwise only known from the toolkit's hover
/// events, and Windows tells a window that the pointer left it without a last move event: a pointer that goes from a card straight out
/// of the preview, or from an icon up out of the dock's window, would leave the machine believing it is still there. The caller asks
/// this while a preview is open and, when it is true, tells the machine that the pointer left both. Inside the dock's glass the toolkit's
/// own events are reliable (the pointer moves inside that window), so the glass counts as "not away" whatever icon is under it.
pub fn pointer_away(cursor: (i32, i32), preview: Rect, glass: Rect, scale: f32) -> bool {
    let dock = Rect::new(glass.left, glass.top - px(ICON_GROW, scale), glass.right, glass.bottom);
    !preview.contains(cursor.0, cursor.1) && !dock.contains(cursor.0, cursor.1)
}

// ---------------------------------------------------------------- the preview's cards

/// A card: the picture box (DWM draws the window live in it), and under it the app's small icon, the title and, on the card under the
/// pointer, a small close button.
pub const CARD_W: f32 = 196.0;
pub const PIC_H: f32 = 110.0;
pub const TITLE_H: f32 = 28.0;
/// Space between a card's edge and what is in it.
pub const CARD_PAD: f32 = 6.0;
pub const CARD_GAP: f32 = 6.0;
/// Space between the window's edge and the cards.
pub const OUTER_PAD: f32 = 8.0;
/// Most cards in a row, and most rows (more windows than that are not shown).
pub const MAX_COLS: usize = 6;
pub const MAX_ROWS: usize = 3;
/// The close button on a card (it sits at the right end of the title row, under the picture: DWM's picture is drawn over anything of
/// ours that lies on it): its side.
pub const CLOSE_SIZE: f32 = 22.0;

pub const CARD_H: f32 = CARD_PAD + PIC_H + TITLE_H + CARD_PAD;

/// How many cards go where.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// How many windows get a card.
    pub shown: usize,
    /// How many windows do not (more than fit).
    pub hidden: usize,
}

/// The grid for `windows` windows in a room `room_w` logical px wide (the most the window may be). Few windows make one row; more wrap
/// into rows of the widest size that fits, evened out (7 windows in rows of 4 and 3, not 6 and 1).
pub fn grid(windows: usize, room_w: f32) -> Grid {
    let fit = (((room_w - 2.0 * OUTER_PAD + CARD_GAP) / (CARD_W + CARD_GAP)).floor() as usize).clamp(1, MAX_COLS);
    let capacity = fit * MAX_ROWS;
    let shown = windows.min(capacity);
    let rows = shown.div_ceil(fit).max(1);
    let cols = shown.div_ceil(rows).max(1);
    Grid { cols, rows, shown, hidden: windows - shown }
}

/// The window's size, logical px.
pub fn size(g: &Grid) -> (f32, f32) {
    let w = 2.0 * OUTER_PAD + g.cols as f32 * CARD_W + (g.cols.saturating_sub(1)) as f32 * CARD_GAP;
    let h = 2.0 * OUTER_PAD + g.rows as f32 * CARD_H + (g.rows.saturating_sub(1)) as f32 * CARD_GAP;
    (w, h)
}

/// Card `i`'s rectangle (x, y, width, height), logical px in the window. Cards fill the rows left to right; the last row is centred.
pub fn card_rect(g: &Grid, i: usize) -> [f32; 4] {
    let (row, col) = (i / g.cols, i % g.cols);
    let in_row = if row + 1 == g.rows { g.shown - row * g.cols } else { g.cols };
    let full = g.cols as f32 * CARD_W + (g.cols.saturating_sub(1)) as f32 * CARD_GAP;
    let this = in_row as f32 * CARD_W + (in_row.saturating_sub(1)) as f32 * CARD_GAP;
    let x = OUTER_PAD + (full - this) / 2.0 + col as f32 * (CARD_W + CARD_GAP);
    let y = OUTER_PAD + row as f32 * (CARD_H + CARD_GAP);
    [x, y, CARD_W, CARD_H]
}

/// Card `i`'s picture box (x, y, width, height), logical px in the window.
pub fn picture_rect(g: &Grid, i: usize) -> [f32; 4] {
    let [x, y, w, _] = card_rect(g, i);
    [x + CARD_PAD, y + CARD_PAD, w - 2.0 * CARD_PAD, PIC_H]
}

/// A logical rectangle in device px (for DWM, which takes the destination in the window's client pixels).
pub fn to_device(r: [f32; 4], scale: f32) -> Rect {
    let (l, t) = (px(r[0], scale), px(r[1], scale));
    Rect::new(l, t, px(r[0] + r[2], scale), px(r[1] + r[3], scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    type M = Machine<&'static str>;

    fn on(m: &mut M, t: u64, k: &'static str) -> Output<&'static str> {
        m.input(t, Input::Icon(Some(k)))
    }
    fn off(m: &mut M, t: u64) -> Output<&'static str> {
        m.input(t, Input::Icon(None))
    }

    #[test]
    fn resting_on_a_running_icon_opens_the_preview_after_the_dwell_and_not_before() {
        let mut m = M::new();
        assert_eq!(on(&mut m, 1_000, "a"), Output::Nothing);
        assert_eq!(m.deadline(), Some(1_000 + DWELL_MS));
        assert_eq!(m.tick(1_000 + DWELL_MS - 1), Output::Nothing);
        assert_eq!(m.tick(1_000 + DWELL_MS), Output::Open("a"));
        assert_eq!(m.shown(), Some(&"a"));
        assert_eq!(m.deadline(), None, "nothing waits while the pointer stays");
    }

    #[test]
    fn a_pointer_that_leaves_before_the_dwell_is_over_opens_nothing() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        off(&mut m, 200);
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(900), Output::Nothing);
        assert_eq!(m.shown(), None);
    }

    #[test]
    fn moving_to_another_icon_before_the_dwell_restarts_it() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        on(&mut m, 300, "b");
        assert_eq!(m.tick(400), Output::Nothing, "a's 400 ms are not b's");
        assert_eq!(m.tick(700), Output::Open("b"));
        // the same icon again (a jitter within it) does not restart the wait
        let mut m = M::new();
        on(&mut m, 0, "a");
        on(&mut m, 300, "a");
        assert_eq!(m.tick(400), Output::Open("a"));
    }

    #[test]
    fn a_neighbouring_running_icon_swaps_the_open_preview_at_once() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(on(&mut m, 450, "b"), Output::Swap("b"), "no delay");
        assert_eq!(m.shown(), Some(&"b"));
        assert_eq!(m.deadline(), None);
        // and back
        assert_eq!(on(&mut m, 460, "a"), Output::Swap("a"));
    }

    #[test]
    fn crossing_the_gap_between_two_icons_swaps_and_does_not_close() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        // the pointer is on nothing for 40 ms between the icons, then on b
        assert_eq!(off(&mut m, 500), Output::Nothing);
        assert_eq!(m.deadline(), Some(500 + GRACE_MS));
        assert_eq!(on(&mut m, 540, "b"), Output::Swap("b"));
        assert_eq!(m.deadline(), None, "the grace was cancelled by the swap");
        assert_eq!(m.tick(5_000), Output::Nothing);
    }

    #[test]
    fn leaving_the_icon_and_the_preview_closes_after_the_grace() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        off(&mut m, 1_000);
        assert_eq!(m.tick(1_000 + GRACE_MS - 1), Output::Nothing);
        assert_eq!(m.tick(1_000 + GRACE_MS), Output::Close);
        assert_eq!(m.shown(), None);
        assert_eq!(m.tick(9_000), Output::Nothing, "closed once");
    }

    #[test]
    fn moving_from_the_icon_into_the_preview_keeps_it_open() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        // the pointer leaves the icon (hover end) and enters the preview within the grace; either order of the two events
        off(&mut m, 1_000);
        assert_eq!(m.input(1_050, Input::Preview(true)), Output::Nothing);
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(3_000), Output::Nothing, "open for as long as the pointer is in it");
        assert_eq!(m.shown(), Some(&"a"));
        // the other order: the preview's enter arrives before the icon's leave
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        m.input(1_000, Input::Preview(true));
        off(&mut m, 1_001);
        assert_eq!(m.deadline(), None);
    }

    #[test]
    fn leaving_the_preview_starts_the_grace_unless_the_pointer_went_back_to_the_icon() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        off(&mut m, 500);
        m.input(520, Input::Preview(true));
        // out of the preview onto nothing
        m.input(2_000, Input::Preview(false));
        assert_eq!(m.deadline(), Some(2_000 + GRACE_MS));
        assert_eq!(m.tick(2_000 + GRACE_MS), Output::Close);
        // out of the preview back onto the icon
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        off(&mut m, 500);
        m.input(520, Input::Preview(true));
        on(&mut m, 2_000, "a");
        m.input(2_001, Input::Preview(false));
        assert_eq!(m.deadline(), None, "the pointer is on the icon");
    }

    #[test]
    fn coming_back_inside_the_grace_cancels_the_closing() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        off(&mut m, 1_000);
        on(&mut m, 1_000 + GRACE_MS - 10, "a");
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(1_000 + 2 * GRACE_MS), Output::Nothing);
        assert_eq!(m.shown(), Some(&"a"));
    }

    #[test]
    fn a_click_closes_the_preview_and_it_stays_away_until_the_pointer_has_left_the_icon() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(m.input(600, Input::Click), Output::Close);
        // the pointer is still on the icon (jitter inside it): no new dwell
        on(&mut m, 700, "a");
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(5_000), Output::Nothing);
        // it leaves and comes back: a preview again after the dwell
        off(&mut m, 5_100);
        on(&mut m, 6_000, "a");
        assert_eq!(m.tick(6_000 + DWELL_MS), Output::Open("a"));
    }

    #[test]
    fn a_click_before_the_preview_opened_cancels_the_dwell() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        assert_eq!(m.input(100, Input::Click), Output::Nothing, "nothing was open");
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(1_000), Output::Nothing);
    }

    #[test]
    fn a_block_closes_at_once_and_keeps_it_from_opening() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(m.input(500, Input::Block(true)), Output::Close);
        assert_eq!(on(&mut m, 600, "b"), Output::Nothing);
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(5_000), Output::Nothing);
        assert!(m.is_blocked());
        // unblocked: a pointer that is still on the icon does not make a preview by itself (no new hover event came); a fresh arrival does
        assert_eq!(m.input(6_000, Input::Block(false)), Output::Nothing);
        assert_eq!(m.tick(9_000), Output::Nothing);
        on(&mut m, 9_100, "c");
        assert_eq!(m.tick(9_100 + DWELL_MS), Output::Open("c"));
    }

    #[test]
    fn a_block_while_dwelling_cancels_the_dwell() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.input(100, Input::Block(true));
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(1_000), Output::Nothing);
    }

    #[test]
    fn a_not_running_icon_never_opens_one_and_ends_the_dwell() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        off(&mut m, 100);
        assert_eq!(m.tick(500), Output::Nothing);
        // an open preview and the pointer on a not-running icon: the grace runs
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        off(&mut m, 600);
        assert_eq!(m.tick(600 + GRACE_MS), Output::Close);
    }

    #[test]
    fn dismissing_closes_at_once_and_leaves_a_block_alone() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        m.input(450, Input::Preview(true));
        assert_eq!(m.dismiss(), Output::Close);
        assert_eq!(m.shown(), None);
        assert_eq!(m.deadline(), None);
        assert_eq!(m.dismiss(), Output::Nothing, "nothing left to close");
        // the pointer entered a preview that is gone: a new one still opens normally afterwards
        on(&mut m, 1_000, "b");
        assert_eq!(m.tick(1_000 + DWELL_MS), Output::Open("b"));
        // a block set before stays set
        let mut m = M::new();
        m.input(0, Input::Block(true));
        m.dismiss();
        assert!(m.is_blocked());
    }

    #[test]
    fn a_stationary_machine_has_no_deadline() {
        let m = M::new();
        assert_eq!(m.deadline(), None);
        assert_eq!(m.shown(), None);
    }

    // ---- placement

    const WORK: Rect = Rect::new(0, 0, 2560, 1392);

    #[test]
    fn a_popup_is_centred_over_its_icon_and_sits_above_the_glass() {
        let r = above_icon(1_000, 1_334, (200.0, 140.0), 1.0, WORK);
        assert_eq!((r.width(), r.height()), (200, 140));
        assert_eq!(r.left, 900);
        assert_eq!(r.bottom, 1_334 - 16, "16 px above the glass");
        assert!(WORK.covers(&r));
    }

    #[test]
    fn a_popup_over_an_icon_near_the_edge_is_kept_on_the_screen() {
        let r = above_icon(40, 1_334, (300.0, 140.0), 1.0, WORK);
        assert_eq!(r.left, 0);
        let r = above_icon(2_550, 1_334, (300.0, 140.0), 1.0, WORK);
        assert_eq!(r.right, 2_560);
        assert!(WORK.covers(&r));
    }

    #[test]
    fn a_popup_on_a_monitor_with_a_negative_origin_stays_on_that_monitor() {
        // the portrait monitor to the left of the primary one: x from -1440, y from -400
        let work = Rect::new(-1_440, -400, 0, 2_100);
        let r = above_icon(-1_400, 2_050, (300.0, 200.0), 1.0, work);
        assert_eq!(r.left, -1_440);
        assert!(work.covers(&r), "{r:?}");
        assert_eq!(r.bottom, 2_050 - 16);
    }

    #[test]
    fn a_tall_popup_is_cut_to_the_room_above_the_dock_and_scaled_with_the_monitor() {
        let r = above_icon(500, 200, (200.0, 900.0), 1.0, Rect::new(0, 0, 1_000, 1_000));
        assert_eq!(r.top, 0);
        assert_eq!(r.bottom, 184);
        let r = above_icon(900, 1_500, (200.0, 100.0), 1.5, Rect::new(0, 0, 2_880, 1_548));
        assert_eq!((r.width(), r.height()), (300, 150));
        assert_eq!(r.bottom, 1_500 - 24, "16 px at 150 %");
    }

    // ---- the cards

    #[test]
    fn a_pointer_that_is_on_neither_the_preview_nor_the_dock_is_away_and_the_edges_count_as_inside() {
        let preview = Rect::new(1000, 900, 1400, 1100);
        let glass = Rect::new(900, 1134, 1500, 1206);
        // on a card, on the dock's glass, on an icon grown over the glass's top edge (8 px at 100 %)
        assert!(!pointer_away((1200, 1000), preview, glass, 1.0));
        assert!(!pointer_away((1200, 1170), preview, glass, 1.0));
        assert!(!pointer_away((1200, 1127), preview, glass, 1.0), "the grown icon is the dock");
        // the 16 px gap between them, beside the preview, above it, on another island, off the screen's work area
        assert!(pointer_away((1200, 1115), preview, glass, 1.0), "the gap is neither");
        assert!(pointer_away((1450, 1000), preview, glass, 1.0));
        assert!(pointer_away((1200, 850), preview, glass, 1.0));
        assert!(pointer_away((300, 1170), preview, glass, 1.0));
        // the right/bottom edge is outside, as everywhere else
        assert!(pointer_away((1400, 1000), preview, glass, 1.0));
        // the growth is scaled with the monitor: 8 px at 100 % is 12 px at 150 %
        assert!(!pointer_away((1200, 1123), preview, glass, 1.5));
        assert!(pointer_away((1200, 1123), preview, glass, 1.0));
    }

    #[test]
    fn the_room_above_the_dock_is_what_a_tall_popup_is_cut_to() {
        let work = Rect::new(0, 0, 2560, 1392);
        // 100 %: 1334 - 16 - 0 = 1318; 150 %: the same device room is fewer logical px
        assert_eq!(room_above(1334, 1.0, work), 1318.0);
        assert_eq!(room_above(1334, 2.0, work), (1334 - 32) as f32 / 2.0);
        // never zero, even with no room at all
        assert!(room_above(10, 1.0, work) > 0.0);
        let r = above_icon(1000, 1334, (272.0, 5000.0), 1.0, work);
        assert_eq!(r.height() as f32, room_above(1334, 1.0, work));
    }

    #[test]
    fn few_windows_make_one_row_and_many_wrap_into_even_rows() {
        let g = grid(3, 1_900.0);
        assert_eq!((g.cols, g.rows, g.shown, g.hidden), (3, 1, 3, 0));
        let g = grid(7, 1_900.0);
        assert_eq!((g.cols, g.rows), (4, 2), "4 and 3, not 6 and 1");
        let g = grid(6, 1_900.0);
        assert_eq!((g.cols, g.rows), (6, 1));
        let g = grid(1, 1_900.0);
        assert_eq!((g.cols, g.rows, g.shown), (1, 1, 1));
        let g = grid(0, 1_900.0);
        assert_eq!((g.cols, g.rows, g.shown), (1, 1, 0));
    }

    #[test]
    fn windows_beyond_what_fits_are_counted_not_drawn() {
        let g = grid(40, 1_900.0);
        assert_eq!((g.shown, g.hidden), (MAX_COLS * MAX_ROWS, 40 - MAX_COLS * MAX_ROWS));
        assert!(g.rows <= MAX_ROWS && g.cols <= MAX_COLS);
    }

    #[test]
    fn a_narrow_room_means_fewer_columns() {
        // room for exactly two cards and their padding
        let g = grid(5, 2.0 * OUTER_PAD + 2.0 * CARD_W + CARD_GAP);
        assert_eq!(g.cols, 2);
        assert_eq!(g.rows, 3);
        let g = grid(5, 50.0);
        assert_eq!(g.cols, 1, "never fewer than one");
    }

    #[test]
    fn the_window_is_exactly_as_big_as_its_cards() {
        let g = grid(3, 1_900.0);
        let (w, h) = size(&g);
        assert_eq!(w, 2.0 * OUTER_PAD + 3.0 * CARD_W + 2.0 * CARD_GAP);
        assert_eq!(h, 2.0 * OUTER_PAD + CARD_H);
        let last = card_rect(&g, 2);
        assert_eq!(last[0] + last[2] + OUTER_PAD, w, "the last card ends one padding before the edge");
        assert_eq!(last[1] + last[3] + OUTER_PAD, h);
    }

    #[test]
    fn the_last_row_is_centred_and_the_pictures_lie_inside_their_cards() {
        let g = grid(9, 1_900.0);
        assert_eq!((g.cols, g.rows), (5, 2), "5 and 4");
        let (w, _) = size(&g);
        let a = card_rect(&g, 5);
        let b = card_rect(&g, 8);
        let left_gap = a[0];
        let right_gap = w - (b[0] + b[2]);
        assert!((left_gap - right_gap).abs() < 0.01, "{left_gap} vs {right_gap}");
        for i in 0..9 {
            let c = card_rect(&g, i);
            let p = picture_rect(&g, i);
            assert!(p[0] >= c[0] && p[1] >= c[1] && p[0] + p[2] <= c[0] + c[2] && p[1] + p[3] <= c[1] + c[3]);
        }
        // no two cards overlap
        for i in 0..9 {
            for j in (i + 1)..9 {
                let (a, b) = (card_rect(&g, i), card_rect(&g, j));
                let apart = a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0] || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1];
                assert!(apart, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn a_picture_box_in_device_pixels_follows_the_scale() {
        let g = grid(1, 1_900.0);
        let p = picture_rect(&g, 0);
        let r = to_device(p, 1.0);
        assert_eq!((r.width(), r.height()), (184, 110));
        let r = to_device(p, 1.5);
        assert_eq!((r.width(), r.height()), (276, 165));
    }
}
