//! The dock's hover previews (shell plan increment B8): the timing rules of "rest the pointer on a running app's icon and a small
//! preview of its windows opens above it", the arithmetic that puts a pop-up above an icon, and the grid of the preview's cards.
//! All of it pure: the pointer's facts and the time (milliseconds on any steady clock) are arguments, the answers are what to do.
//! The app side (`rf-app/src/dock/hover_preview.rs`) feeds the machine from the icons' hover events and carries the answers out.
//!
//! THE RULES (what a user sees):
//! * the pointer rests on a RUNNING app's icon for [`DWELL_MS`] (80 ms: at once to the eye, so that two windows of an app are seen the moment the pointer is on it; it was 400 ms until a user asked; a pointer that only crosses the icon on its way still opens nothing): the preview opens;
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
pub const DWELL_MS: u64 = 80;
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

/// The gap between the top of the dock's glass and the bottom of a pop-up above an icon, logical px at the default dock size. An icon
/// under the pointer grows up to [`ICON_GROW`] px over the glass's top edge (`rf-app` middle island), so the gap is that and the
/// panels' own 8 px.
pub const ICON_GAP: f32 = 16.0;

/// How far an icon under the pointer grows above the top of the dock's glass, logical px at the default dock size (the middle island's
/// magnification).
pub const ICON_GROW: f32 = 8.0;

/// The gap above the glass at a dock size (`dock` is the size factor, 1.0 the default): the growth of the icons scales with the dock,
/// the 8 px beyond it do not (they are the pop-up's own distance from what it comes from).
pub fn icon_gap(dock: f32) -> f32 {
    ICON_GROW * super::size::sane(dock) + (ICON_GAP - ICON_GROW)
}

/// The rectangle (device px, screen coordinates) of a pop-up of `size` logical px that sits above the dock's glass (`glass_top`, its top
/// edge) and is centred on an icon (`icon_center`, device px), kept inside the monitor's `work` area. A pop-up wider than the work
/// area is narrowed to it; one taller than the room above the dock is shortened to that room. `scale` is the monitor's (the pop-up's
/// own contents do not follow the dock's size); `dock` is the dock's size factor, which only moves the pop-up higher.
pub fn above_icon(icon_center: i32, glass_top: i32, size: (f32, f32), scale: f32, dock: f32, work: Rect) -> Rect {
    let w = px(size.0, scale).clamp(1, work.width().max(1));
    let gap = px(icon_gap(dock), scale);
    let room = (glass_top - gap - work.top).max(1);
    let h = px(size.1, scale).clamp(1, room);
    let bottom = glass_top - gap;
    let top = (bottom - h).max(work.top);
    let left = (icon_center - w / 2).clamp(work.left, (work.right - w).max(work.left));
    Rect::from_xywh(left, top, w, h)
}

/// The room above the dock's glass for a pop-up, logical px (what [`above_icon`] shortens a tall pop-up to).
pub fn room_above(glass_top: i32, scale: f32, dock: f32, work: Rect) -> f32 {
    let room = (glass_top - px(icon_gap(dock), scale) - work.top).max(1);
    room as f32 / scale.max(0.01)
}

/// Whether the pointer (`cursor`, device px, screen coordinates) is on NEITHER the open preview (`preview`) NOR the dock's middle glass
/// (`glass`, with the part an icon grows over its top edge). The pointer's leaving is otherwise only known from the toolkit's hover
/// events, and Windows tells a window that the pointer left it without a last move event: a pointer that goes from a card straight out
/// of the preview, or from an icon up out of the dock's window, would leave the machine believing it is still there. The caller asks
/// this while a preview is open and, when it is true, tells the machine that the pointer left both. Inside the dock's glass the toolkit's
/// own events are reliable (the pointer moves inside that window), so the glass counts as "not away" whatever icon is under it.
pub fn pointer_away(cursor: (i32, i32), preview: Rect, glass: Rect, scale: f32, dock: f32) -> bool {
    let dock = Rect::new(glass.left, glass.top - px(ICON_GROW * super::size::sane(dock), scale), glass.right, glass.bottom);
    !preview.contains(cursor.0, cursor.1) && !dock.contains(cursor.0, cursor.1)
}

// ---------------------------------------------------------------- the preview's cards
//
// ONE PLACE GIVES EVERY RECTANGLE. [`layout`] works out the grid and the heights from the windows' shapes, and [`Layout::card`] gives, for
// card `i`, its rectangle, its picture box and its title row. The toolkit draws the card, the box and the title from those numbers
// (`rf-app/src/dock/hover_preview.rs`) and DWM is told where to draw the live picture from the very same box (`picture`, through
// [`to_device`]): there is no second formula that could drift from the first.
//
// A card is: CARD_PAD, the picture box (CARD_W - 2 CARD_PAD wide, as tall as the row's tallest window needs), the title row, CARD_PAD.
// The box is DYNAMIC IN HEIGHT: it follows the shape of the window it shows ([`pic_height`]), the cards of one row share the row's
// tallest box, and the window follows the rows.

/// A card's width, logical px. It does not change with the window's shape.
pub const CARD_W: f32 = 196.0;
/// Space between a card's edge and what is in it. Thin: the picture nearly fills the card.
pub const CARD_PAD: f32 = 3.0;
pub const CARD_GAP: f32 = 4.0;
/// Space between the window's edge and the cards. Thin, so the glass's rim does not read as a thick border.
pub const OUTER_PAD: f32 = 4.0;
pub const TITLE_H: f32 = 28.0;
/// The picture box's width.
pub const PIC_W: f32 = CARD_W - 2.0 * CARD_PAD;
/// The box is never lower than this share of its width (a very wide window is fitted into a box this low and centred) ...
pub const MIN_PIC_RATIO: f32 = 1.0 / 3.0;
/// ... and never higher than this (a very tall window is fitted and centred).
pub const MAX_PIC_RATIO: f32 = 1.1;
/// The height share of a window whose size is not known yet (16:10).
pub const UNKNOWN_PIC_RATIO: f32 = 0.625;
/// Most cards in a row, and most rows (more windows than that are not shown).
pub const MAX_COLS: usize = 6;
pub const MAX_ROWS: usize = 3;
/// The close button on a card (it sits at the right end of the title row, under the picture: DWM's picture is drawn over anything of
/// ours that lies on it): its side.
pub const CLOSE_SIZE: f32 = 22.0;

/// The picture box's height, logical px, for a window of this size (`None`: not known yet, 16:10): the box's width times the window's
/// height over its width, kept between [`MIN_PIC_RATIO`] and [`MAX_PIC_RATIO`] of the width and rounded to a whole px.
pub fn pic_height(source: Option<(i32, i32)>) -> f32 {
    let ratio = match source {
        Some((w, h)) if w > 0 && h > 0 => (h as f32 / w as f32).clamp(MIN_PIC_RATIO, MAX_PIC_RATIO),
        _ => UNKNOWN_PIC_RATIO,
    };
    (PIC_W * ratio).round()
}

/// A card's height for a picture box of `pic_h`.
fn card_height(pic_h: f32) -> f32 {
    CARD_PAD + pic_h + TITLE_H + CARD_PAD
}

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

/// What one card is made of: three rectangles (x, y, width, height), logical px in the preview window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CardBox {
    pub card: [f32; 4],
    /// The picture box: the toolkit paints its backdrop here and DWM draws the live window in it.
    pub picture: [f32; 4],
    /// The title row under the box (the small icon, the title, the close button).
    pub title: [f32; 4],
    /// `picture` and `title` again, relative to the card's own top left corner: the toolkit places them as children of the card, so its
    /// drawing takes these and subtracts nothing of its own (the numbers cannot drift from `picture`, which DWM is given).
    pub picture_in_card: [f32; 4],
    pub title_in_card: [f32; 4],
}

/// The grid and the height of each row's picture boxes.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    grid: Grid,
    /// Each row's picture box height: the tallest of the windows in the row.
    pic_h: Vec<f32>,
}

/// The layout for windows of these shapes (`sources`, in card order: a window's size in device px, `None` when it is not known yet) in
/// a room `room_w` logical px wide. Windows beyond what fits are not drawn and do not count in a row's height.
pub fn layout(sources: &[Option<(i32, i32)>], room_w: f32) -> Layout {
    layout_in(sources, room_w, f32::INFINITY)
}

/// [`layout`] for a room that is also `room_h` logical px high (what is above the dock): when the rows would not fit, every row's box is
/// capped at the highest value that does (a cap, not a squeeze: a window taller than the cap is fitted and centred in the box as one
/// beyond [`MAX_PIC_RATIO`] is), but never below [`MIN_PIC_RATIO`]'s height. Only when even the lowest boxes do not fit is the window
/// left taller than the room (the caller's placement then shortens it).
pub fn layout_in(sources: &[Option<(i32, i32)>], room_w: f32, room_h: f32) -> Layout {
    let mut l = layout_unbounded(sources, room_w);
    if l.size().1 > room_h {
        let floor = (PIC_W * MIN_PIC_RATIO).round();
        let mut cap = l.pic_h.iter().copied().fold(floor, f32::max);
        while cap > floor && window_height(l.pic_h.iter().map(|h| h.min(cap))) > room_h {
            cap -= 1.0;
        }
        for h in &mut l.pic_h {
            *h = h.min(cap);
        }
    }
    l
}

/// The window's height, logical px, for rows whose picture boxes are this high.
fn window_height(rows: impl Iterator<Item = f32>) -> f32 {
    let (n, sum) = rows.fold((0usize, 0.0f32), |(n, sum), h| (n + 1, sum + card_height(h)));
    2.0 * OUTER_PAD + sum + n.saturating_sub(1) as f32 * CARD_GAP
}

fn layout_unbounded(sources: &[Option<(i32, i32)>], room_w: f32) -> Layout {
    let g = grid(sources.len(), room_w);
    let pic_h = (0..g.rows)
        .map(|r| {
            let row = &sources[(r * g.cols).min(g.shown)..((r + 1) * g.cols).min(g.shown)];
            row.iter().map(|s| pic_height(*s)).fold(None::<f32>, |a, h| Some(a.map_or(h, |a| a.max(h)))).unwrap_or_else(|| pic_height(None))
        })
        .collect();
    Layout { grid: g, pic_h }
}

impl Layout {
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// The picture boxes' height in row `row`.
    pub fn row_pic_height(&self, row: usize) -> f32 {
        self.pic_h.get(row).copied().unwrap_or_else(|| pic_height(None))
    }

    /// The window's size, logical px.
    pub fn size(&self) -> (f32, f32) {
        let g = &self.grid;
        let w = 2.0 * OUTER_PAD + g.cols as f32 * CARD_W + (g.cols.saturating_sub(1)) as f32 * CARD_GAP;
        (w, window_height((0..g.rows).map(|r| self.row_pic_height(r))))
    }

    /// Card `i`'s rectangles. Cards fill the rows left to right; the last row is centred.
    pub fn card(&self, i: usize) -> CardBox {
        let g = &self.grid;
        let (row, col) = (i / g.cols, i % g.cols);
        let in_row = if row + 1 == g.rows { g.shown - row * g.cols } else { g.cols };
        let full = g.cols as f32 * CARD_W + (g.cols.saturating_sub(1)) as f32 * CARD_GAP;
        let this = in_row as f32 * CARD_W + (in_row.saturating_sub(1)) as f32 * CARD_GAP;
        let x = OUTER_PAD + (full - this) / 2.0 + col as f32 * (CARD_W + CARD_GAP);
        let y = OUTER_PAD + (0..row).map(|r| card_height(self.row_pic_height(r)) + CARD_GAP).sum::<f32>();
        let pic_h = self.row_pic_height(row);
        CardBox {
            card: [x, y, CARD_W, card_height(pic_h)],
            picture: [x + CARD_PAD, y + CARD_PAD, PIC_W, pic_h],
            title: [x + CARD_PAD, y + CARD_PAD + pic_h, PIC_W, TITLE_H],
            picture_in_card: [CARD_PAD, CARD_PAD, PIC_W, pic_h],
            title_in_card: [CARD_PAD, CARD_PAD + pic_h, PIC_W, TITLE_H],
        }
    }
}

/// A logical rectangle in device px (for DWM, which takes the destination in pixels).
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
        on(&mut m, DWELL_MS - 20, "b");
        assert_eq!(m.tick(DWELL_MS), Output::Nothing, "a's wait is not b's");
        assert_eq!(m.tick(2 * DWELL_MS - 20), Output::Open("b"));
        // the same icon again (a jitter within it) does not restart the wait
        let mut m = M::new();
        on(&mut m, 0, "a");
        on(&mut m, DWELL_MS - 20, "a");
        assert_eq!(m.tick(DWELL_MS), Output::Open("a"));
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
        let r = above_icon(1_000, 1_334, (200.0, 140.0), 1.0, 1.0, WORK);
        assert_eq!((r.width(), r.height()), (200, 140));
        assert_eq!(r.left, 900);
        assert_eq!(r.bottom, 1_334 - 16, "16 px above the glass");
        assert!(WORK.covers(&r));
    }

    #[test]
    fn a_popup_over_an_icon_near_the_edge_is_kept_on_the_screen() {
        let r = above_icon(40, 1_334, (300.0, 140.0), 1.0, 1.0, WORK);
        assert_eq!(r.left, 0);
        let r = above_icon(2_550, 1_334, (300.0, 140.0), 1.0, 1.0, WORK);
        assert_eq!(r.right, 2_560);
        assert!(WORK.covers(&r));
    }

    #[test]
    fn a_popup_on_a_monitor_with_a_negative_origin_stays_on_that_monitor() {
        // the portrait monitor to the left of the primary one: x from -1440, y from -400
        let work = Rect::new(-1_440, -400, 0, 2_100);
        let r = above_icon(-1_400, 2_050, (300.0, 200.0), 1.0, 1.0, work);
        assert_eq!(r.left, -1_440);
        assert!(work.covers(&r), "{r:?}");
        assert_eq!(r.bottom, 2_050 - 16);
    }

    #[test]
    fn a_tall_popup_is_cut_to_the_room_above_the_dock_and_scaled_with_the_monitor() {
        let r = above_icon(500, 200, (200.0, 900.0), 1.0, 1.0, Rect::new(0, 0, 1_000, 1_000));
        assert_eq!(r.top, 0);
        assert_eq!(r.bottom, 184);
        let r = above_icon(900, 1_500, (200.0, 100.0), 1.5, 1.0, Rect::new(0, 0, 2_880, 1_548));
        assert_eq!((r.width(), r.height()), (300, 150));
        assert_eq!(r.bottom, 1_500 - 24, "16 px at 150 %");
    }

    // ---- the cards

    #[test]
    fn a_pointer_that_is_on_neither_the_preview_nor_the_dock_is_away_and_the_edges_count_as_inside() {
        let preview = Rect::new(1000, 900, 1400, 1100);
        let glass = Rect::new(900, 1134, 1500, 1206);
        // on a card, on the dock's glass, on an icon grown over the glass's top edge (8 px at 100 %)
        assert!(!pointer_away((1200, 1000), preview, glass, 1.0, 1.0));
        assert!(!pointer_away((1200, 1170), preview, glass, 1.0, 1.0));
        assert!(!pointer_away((1200, 1127), preview, glass, 1.0, 1.0), "the grown icon is the dock");
        // the 16 px gap between them, beside the preview, above it, on another island, off the screen's work area
        assert!(pointer_away((1200, 1115), preview, glass, 1.0, 1.0), "the gap is neither");
        assert!(pointer_away((1450, 1000), preview, glass, 1.0, 1.0));
        assert!(pointer_away((1200, 850), preview, glass, 1.0, 1.0));
        assert!(pointer_away((300, 1170), preview, glass, 1.0, 1.0));
        // the right/bottom edge is outside, as everywhere else
        assert!(pointer_away((1400, 1000), preview, glass, 1.0, 1.0));
        // the growth is scaled with the monitor: 8 px at 100 % is 12 px at 150 %
        assert!(!pointer_away((1200, 1123), preview, glass, 1.5, 1.0));
        assert!(pointer_away((1200, 1123), preview, glass, 1.0, 1.0));
    }

    #[test]
    fn the_room_above_the_dock_is_what_a_tall_popup_is_cut_to() {
        let work = Rect::new(0, 0, 2560, 1392);
        // 100 %: 1334 - 16 - 0 = 1318; 150 %: the same device room is fewer logical px
        assert_eq!(room_above(1334, 1.0, 1.0, work), 1318.0);
        assert_eq!(room_above(1334, 2.0, 1.0, work), (1334 - 32) as f32 / 2.0);
        // never zero, even with no room at all
        assert!(room_above(10, 1.0, 1.0, work) > 0.0);
        let r = above_icon(1000, 1334, (272.0, 5000.0), 1.0, 1.0, work);
        assert_eq!(r.height() as f32, room_above(1334, 1.0, 1.0, work));
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

    const SRC_169: Option<(i32, i32)> = Some((1920, 1080));

    #[test]
    fn the_box_follows_the_windows_shape_between_a_third_and_one_and_a_tenth_of_its_width() {
        // 190 px wide boxes
        assert_eq!(PIC_W, 190.0);
        assert_eq!(pic_height(SRC_169), (190.0f32 * 9.0 / 16.0).round(), "16:9");
        assert_eq!(pic_height(Some((1000, 1000))), 190.0, "square");
        assert_eq!(pic_height(Some((1600, 1000))), 119.0, "16:10 as measured");
        // a very wide window (ultra-wide, a strip): the lowest box, the picture is fitted into it and centred
        let low = (190.0f32 / 3.0).round();
        assert_eq!(pic_height(Some((3440, 600))), low);
        assert_eq!(pic_height(Some((1000, 100))), low);
        // a very tall one: the highest box
        let high = (190.0f32 * 1.1).round();
        assert_eq!(pic_height(Some((500, 2000))), high);
        assert_eq!(pic_height(Some((100, 5000))), high);
        // exactly at the limits
        assert_eq!(pic_height(Some((300, 100))), low);
        assert_eq!(pic_height(Some((1000, 1100))), high);
        // not known: 16:10; nonsense is not known
        assert_eq!(pic_height(None), 119.0);
        assert_eq!(pic_height(Some((0, 0))), 119.0);
        assert_eq!(pic_height(Some((-5, 100))), 119.0);
        // monotone between the limits
        let mut last = 0.0;
        for h in (100..=1100).step_by(25) {
            let v = pic_height(Some((1000, h)));
            assert!(v >= last && v >= low && v <= high, "{h}: {v}");
            last = v;
        }
    }

    #[test]
    fn the_cards_of_one_row_share_the_tallest_box_and_each_row_has_its_own_height() {
        // a wide window, a tall one and a 16:9 one in one row
        let l = layout(&[Some((2560, 1080)), Some((800, 1000)), SRC_169], 1_900.0);
        assert_eq!((l.grid().cols, l.grid().rows), (3, 1));
        let tall = pic_height(Some((800, 1000)));
        for i in 0..3 {
            let c = l.card(i);
            assert_eq!(c.picture[3], tall, "card {i}");
            assert_eq!(c.card[3], CARD_PAD + tall + TITLE_H + CARD_PAD);
            assert_eq!(c.card[1], OUTER_PAD, "one row: every card starts at the same y");
        }
        // two rows: a low row above a tall one; the second starts where the first ends plus the gap
        let mut src = vec![Some((3000, 600)); 4];
        src.extend([Some((500, 900)), SRC_169, SRC_169]);
        let l = layout(&src, 1_900.0);
        assert_eq!((l.grid().cols, l.grid().rows), (4, 2));
        assert_eq!(l.row_pic_height(0), pic_height(Some((3000, 600))));
        assert_eq!(l.row_pic_height(1), pic_height(Some((500, 900))), "the row of 3 is as tall as its tallest");
        let (a, b) = (l.card(0), l.card(4));
        assert_eq!(b.card[1], a.card[1] + a.card[3] + CARD_GAP);
        assert_eq!(a.picture[3], l.row_pic_height(0));
        assert_eq!(b.picture[3], l.row_pic_height(1));
    }

    #[test]
    fn windows_that_are_not_shown_do_not_make_a_row_taller() {
        let mut src = vec![SRC_169; MAX_COLS * MAX_ROWS];
        src.push(Some((100, 1000))); // the 19th window: not drawn
        let l = layout(&src, 1_900.0);
        assert_eq!(l.grid().hidden, 1);
        assert_eq!(l.row_pic_height(l.grid().rows - 1), pic_height(SRC_169));
    }

    #[test]
    fn a_source_that_is_not_known_yet_is_sixteen_to_ten() {
        let l = layout(&[None, None], 1_900.0);
        assert_eq!(l.card(0).picture[3], pic_height(None));
        assert!((l.card(0).picture[2] / l.card(0).picture[3] - 1.6).abs() < 0.02, "16:10");
        // one known and one unknown share the taller of the two
        let l = layout(&[None, Some((800, 1000))], 1_900.0);
        assert_eq!(l.card(0).picture[3], pic_height(Some((800, 1000))));
    }

    #[test]
    fn the_window_is_exactly_as_big_as_its_cards() {
        for src in [vec![SRC_169; 3], vec![Some((600, 900)), None, Some((2560, 700))], vec![SRC_169; 9], vec![Some((500, 900)); 1]] {
            let l = layout(&src, 1_900.0);
            let (w, h) = l.size();
            let g = *l.grid();
            let last = l.card(g.shown - 1);
            assert_eq!(last.card[1] + last.card[3] + OUTER_PAD, h, "the last row ends one padding before the bottom edge");
            // the widest row reaches both sides' padding exactly
            let first = l.card(0);
            assert_eq!(first.card[0].min(l.card(g.cols - 1).card[0]), OUTER_PAD, "{src:?}");
            let right = (0..g.shown).map(|i| l.card(i).card[0] + l.card(i).card[2]).fold(0.0, f32::max);
            assert_eq!(right + OUTER_PAD, w);
        }
        let l = layout(&[SRC_169; 3], 1_900.0);
        let (w, h) = l.size();
        assert_eq!(w, 2.0 * OUTER_PAD + 3.0 * CARD_W + 2.0 * CARD_GAP);
        assert_eq!(h, 2.0 * OUTER_PAD + CARD_PAD + pic_height(SRC_169) + TITLE_H + CARD_PAD);
    }

    #[test]
    fn the_edges_are_thin() {
        // about 4 px outside the cards and 3 px inside a card at 100 %
        assert_eq!(OUTER_PAD, 4.0);
        assert_eq!(CARD_PAD, 3.0);
        let c = layout(&[SRC_169], 1_900.0).card(0);
        assert_eq!(c.card[0], OUTER_PAD);
        assert_eq!(c.picture[0] - c.card[0], CARD_PAD);
        assert_eq!(c.picture[1] - c.card[1], CARD_PAD);
        assert_eq!(c.card[0] + c.card[2] - (c.picture[0] + c.picture[2]), CARD_PAD, "the same on the right");
    }

    #[test]
    fn the_last_row_is_centred_and_everything_lies_inside_its_card_and_no_two_cards_overlap() {
        let mut src = vec![SRC_169; 5];
        src.extend([Some((500, 900)), Some((2000, 500)), None, SRC_169]);
        let l = layout(&src, 1_900.0);
        let g = *l.grid();
        assert_eq!((g.cols, g.rows), (5, 2), "5 and 4");
        let (w, _) = l.size();
        let a = l.card(5).card;
        let b = l.card(8).card;
        let left_gap = a[0];
        let right_gap = w - (b[0] + b[2]);
        assert!((left_gap - right_gap).abs() < 0.01, "{left_gap} vs {right_gap}");
        for i in 0..9 {
            let c = l.card(i);
            let inside = |p: [f32; 4]| p[0] >= c.card[0] && p[1] >= c.card[1] && p[0] + p[2] <= c.card[0] + c.card[2] && p[1] + p[3] <= c.card[1] + c.card[3];
            assert!(inside(c.picture) && inside(c.title), "card {i}");
            assert!(c.title[1] >= c.picture[1] + c.picture[3], "the title is under the box");
        }
        for i in 0..9 {
            for j in (i + 1)..9 {
                let (a, b) = (l.card(i).card, l.card(j).card);
                let apart = a[0] + a[2] <= b[0] || b[0] + b[2] <= a[0] || a[1] + a[3] <= b[1] || b[1] + b[3] <= a[1];
                assert!(apart, "{i} and {j} overlap");
            }
        }
    }

    #[test]
    fn three_tall_rows_are_capped_to_the_room_above_the_dock_and_never_below_the_lowest_box() {
        // 18 windows of ratio 1.0 or taller: 3 rows of the tallest box, 745 logical px high when nothing limits them
        let src = vec![Some((900, 1000)); MAX_COLS * MAX_ROWS];
        let free = layout(&src, 1_900.0);
        assert_eq!(free.grid().rows, 3);
        assert_eq!(free.size().1, 745.0, "what the old code asked of a short screen");
        for room in [640.0f32, 560.0, 450.0] {
            let l = layout_in(&src, 1_900.0, room);
            assert!(l.size().1 <= room, "room {room}: {}", l.size().1);
            // the window still has its three rows and every card lies inside the window (nothing is cut off)
            let h = l.size().1;
            for i in 0..l.grid().shown {
                let c = l.card(i).card;
                assert!(c[1] + c[3] + OUTER_PAD <= h + 0.001, "card {i} at room {room}");
            }
            // the cap is the highest that fits: one more px on every row would not
            let one_more = l.row_pic_height(0) + 1.0;
            assert!(window_height((0..3).map(|_| one_more)) > room || one_more > free.row_pic_height(0));
        }
        // a room that already holds the rows changes nothing
        assert_eq!(layout_in(&src, 1_900.0, 745.0), free);
        assert_eq!(layout_in(&src, 1_900.0, 5_000.0), free);
        // a room too small for even the lowest boxes: the lowest boxes, not less (the placement shortens the window then)
        let tiny = layout_in(&src, 1_900.0, 100.0);
        assert_eq!(tiny.row_pic_height(0), (PIC_W * MIN_PIC_RATIO).round());
        // a short row is not made taller by a cap, and a low row is left alone while a tall one is capped
        let mut mixed = vec![Some((3000, 600)); MAX_COLS];
        mixed.extend(vec![Some((900, 1000)); MAX_COLS * 2]);
        let l = layout_in(&mixed, 1_900.0, 480.0);
        assert!(l.size().1 <= 480.0);
        assert_eq!(l.row_pic_height(0), pic_height(Some((3000, 600))), "the low row keeps its shape");
        assert!(l.row_pic_height(1) < pic_height(Some((900, 1000))));
    }

    #[test]
    fn what_the_toolkit_places_inside_a_card_is_the_same_place_as_what_dwm_is_given() {
        let src = [Some((2560, 1392)), Some((900, 1400)), None, Some((3440, 600)), SRC_169, SRC_169, None];
        let l = layout(&src, 1_900.0);
        for i in 0..l.grid().shown {
            let c = l.card(i);
            // the child offsets plus the card's own corner give back the rectangles DWM is computed from (exactly: both are sums of the
            // same constants and the card's x, y)
            assert_eq!([c.card[0] + c.picture_in_card[0], c.card[1] + c.picture_in_card[1]], [c.picture[0], c.picture[1]], "{i}");
            assert_eq!([c.card[0] + c.title_in_card[0], c.card[1] + c.title_in_card[1]], [c.title[0], c.title[1]], "{i}");
            assert_eq!(c.picture_in_card[2..], c.picture[2..], "{i}: same size");
            assert_eq!(c.title_in_card[2..], c.title[2..], "{i}: same size");
        }
    }

    /// THE ONE FUNCTION: what the toolkit paints as the box and what DWM is given are the same numbers, at any scale, and the live picture
    /// fitted into the device box lies inside what the toolkit painted.
    #[test]
    fn the_box_the_toolkit_paints_and_the_one_dwm_draws_in_are_the_same_rectangle() {
        let src = [Some((2560, 1392)), Some((900, 1400)), None, Some((3440, 600))];
        let l = layout(&src, 1_900.0);
        for scale in [1.0f32, 1.25, 1.5, 2.0] {
            for (i, source) in src.iter().enumerate() {
                let painted = l.card(i).picture;
                let dev = to_device(painted, scale);
                // the same corners the toolkit's logical box has at this scale (its left/top and right/bottom edges rounded alike)
                assert_eq!(dev.left, px(painted[0], scale));
                assert_eq!(dev.top, px(painted[1], scale));
                assert_eq!(dev.right, px(painted[0] + painted[2], scale));
                assert_eq!(dev.bottom, px(painted[1] + painted[3], scale));
                let fitted = crate::switcher::layout::fit_picture(source.unwrap_or((1600, 1000)), dev);
                assert!(dev.covers(&fitted), "{i} at {scale}: {fitted:?} not inside {dev:?}");
            }
        }
    }

    #[test]
    fn a_picture_box_in_device_pixels_follows_the_scale() {
        let l = layout(&[None], 1_900.0);
        let p = l.card(0).picture;
        let r = to_device(p, 1.0);
        assert_eq!((r.width(), r.height()), (190, 119));
        let r = to_device(p, 1.5);
        // (both edges are rounded, not the height: 7 * 1.5 = 10.5 -> 11 and 126 * 1.5 = 189)
        assert_eq!((r.width(), r.height()), (285, 178));
    }
}
