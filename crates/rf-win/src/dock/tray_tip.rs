//! The tooltip of a tray icon (shell plan increment D3): the timing rules of "rest the pointer on an icon and its tip appears above it",
//! the size of the little window for a given text, and where it goes. All of it pure: the pointer's facts and the time (milliseconds on any
//! steady clock) are arguments, the answers are what to do. The app side (`rf-app/src/dock/tray_tip.rs`) feeds the machine and carries the
//! answers out, in a window that is never activated (as the hover previews' is).
//!
//! THE RULES (what a user sees):
//! * the pointer rests on an icon whose program gave a tip for [`DWELL_MS`] (400 ms): the tip opens above the icon;
//! * while it is open, moving onto another icon with a tip swaps it at once; moving onto nothing, or onto an icon without a tip, closes it
//!   at once (a tip is not something to move the pointer into);
//! * a press on an icon closes it and it does not come back until the pointer has left that icon (a tip over the menu the click opens
//!   would be noise);
//! * a block (a drag, a menu, the dock hiding, the setting off) closes it at once and keeps it from opening.
//!
//! There is no polling: [`TipMachine::deadline`] says when the next [`TipMachine::tick`] is due.

use super::layout::{Rect, px};

/// How long the pointer must rest on an icon before its tip opens.
pub const DWELL_MS: u64 = 400;

/// What the pointer is doing, as the caller sees it. `K` names an icon.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TipInput<K> {
    /// The pointer is on this icon, which has a tip (`Some`), or on nothing a tip belongs to (`None`).
    Icon(Option<K>),
    /// A button was pressed on an icon.
    Press,
    /// Something forbids a tip (`true`) or no longer does (`false`).
    Block(bool),
}

/// What the caller has to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TipOutput<K> {
    Nothing,
    /// Open the tip of this icon.
    Show(K),
    /// The tip is open: show this icon's instead, without any delay.
    Swap(K),
    Hide,
}

#[derive(Clone, Debug)]
pub struct TipMachine<K> {
    shown: Option<K>,
    /// An icon the pointer rests on and since when (ms), while no tip is open.
    dwell: Option<(K, u64)>,
    on_icon: Option<K>,
    blocked: bool,
    /// After a press: no tip for this icon until the pointer has left it.
    suppressed: Option<K>,
}

impl<K> Default for TipMachine<K> {
    fn default() -> Self {
        TipMachine { shown: None, dwell: None, on_icon: None, blocked: false, suppressed: None }
    }
}

impl<K: Clone + PartialEq> TipMachine<K> {
    pub fn new() -> Self {
        Self::default()
    }

    /// The icon whose tip is open.
    pub fn shown(&self) -> Option<&K> {
        self.shown.as_ref()
    }

    fn hide(&mut self) -> TipOutput<K> {
        self.dwell = None;
        if self.shown.take().is_some() { TipOutput::Hide } else { TipOutput::Nothing }
    }

    /// Closes the tip now, whatever the pointer is doing, and forgets what the pointer was doing. A block stays as it is.
    pub fn dismiss(&mut self) -> TipOutput<K> {
        self.on_icon = None;
        self.hide()
    }

    pub fn input(&mut self, now_ms: u64, input: TipInput<K>) -> TipOutput<K> {
        match input {
            TipInput::Block(on) => {
                self.blocked = on;
                if on { self.hide() } else { TipOutput::Nothing }
            }
            TipInput::Press => {
                self.suppressed = self.on_icon.clone();
                self.hide()
            }
            TipInput::Icon(k) => {
                self.on_icon = k.clone();
                // the pointer left the icon that was pressed: it may ask again
                if self.suppressed.is_some() && self.suppressed != k {
                    self.suppressed = None;
                }
                if self.blocked {
                    return TipOutput::Nothing;
                }
                match (self.shown.clone(), k) {
                    (Some(shown), Some(k)) => {
                        if k == shown {
                            TipOutput::Nothing
                        } else if self.suppressed.as_ref() == Some(&k) {
                            self.hide()
                        } else {
                            self.shown = Some(k.clone());
                            TipOutput::Swap(k)
                        }
                    }
                    (Some(_), None) => self.hide(),
                    (None, Some(k)) => {
                        if self.suppressed.as_ref() == Some(&k) {
                            self.dwell = None;
                        } else if self.dwell.as_ref().map(|(d, _)| d) != Some(&k) {
                            self.dwell = Some((k, now_ms));
                        }
                        TipOutput::Nothing
                    }
                    (None, None) => {
                        self.dwell = None;
                        TipOutput::Nothing
                    }
                }
            }
        }
    }

    /// Time has passed: `now_ms` is at or after [`deadline`](Self::deadline).
    pub fn tick(&mut self, now_ms: u64) -> TipOutput<K> {
        if let Some((k, since)) = self.dwell.clone()
            && !self.blocked
            && now_ms.saturating_sub(since) >= DWELL_MS
        {
            self.dwell = None;
            self.shown = Some(k.clone());
            return TipOutput::Show(k);
        }
        TipOutput::Nothing
    }

    /// When the next tick is due (ms on the caller's clock), if anything is waiting.
    pub fn deadline(&self) -> Option<u64> {
        self.dwell.as_ref().filter(|_| !self.blocked).map(|(_, since)| since + DWELL_MS)
    }
}

// ---------------------------------------------------------------- the text and the window

/// What a tip is cut to: the programs' own limit is 128 characters; anything longer is somebody's mistake.
pub const MAX_CHARS: usize = 200;
/// The window's widest and narrowest width, logical px, and the padding inside it.
pub const MAX_W: f32 = 320.0;
pub const MIN_W: f32 = 48.0;
pub const PAD_X: f32 = 10.0;
pub const PAD_Y: f32 = 6.0;
pub const LINE_H: f32 = 16.0;
/// What one character takes at the tip's text size (12 px text; a guess that errs wide).
const CHAR_W: f32 = 7.0;
/// The most lines shown; the rest is cut.
pub const MAX_LINES: usize = 4;
/// The gap between the icon's top edge and the window's bottom edge, logical px.
pub const GAP: f32 = 8.0;

/// A tip as it is shown: control characters other than a line break are gone, runs of blanks are one, the ends are trimmed, and it is
/// cut at [`MAX_CHARS`]. An empty answer means the icon has no tip (nothing opens).
pub fn clean(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_CHARS * 2));
    let mut last_blank = true;
    for c in raw.replace("\r\n", "\n").replace('\r', "\n").chars() {
        let c = if c == '\n' { c } else if c.is_control() || c.is_whitespace() { ' ' } else { c };
        if c == ' ' {
            if !last_blank {
                out.push(' ');
            }
            last_blank = true;
        } else {
            // (a blank before a line break goes: it would be a line of its own that looks empty)
            if c == '\n' && out.ends_with(' ') {
                out.pop();
            }
            out.push(c);
            last_blank = c == '\n';
        }
    }
    let t: String = out.trim().chars().take(MAX_CHARS).collect();
    t.trim_end().to_string()
}

/// How many lines a text takes in a window whose text is `inner_w` logical px wide.
fn lines_for(text: &str, inner_w: f32) -> usize {
    let per_line = ((inner_w / CHAR_W).floor() as usize).max(1);
    text.split('\n').map(|l| l.chars().count().div_ceil(per_line).max(1)).sum::<usize>().clamp(1, MAX_LINES)
}

/// The window's size for a (cleaned) text, logical px: as wide as the longest line wants up to [`MAX_W`], then as many lines as
/// the text needs up to [`MAX_LINES`].
pub fn size(text: &str) -> (f32, f32) {
    let longest = text.split('\n').map(|l| l.chars().count()).max().unwrap_or(0);
    let w = (longest as f32 * CHAR_W + 2.0 * PAD_X).clamp(MIN_W, MAX_W);
    let lines = lines_for(text, w - 2.0 * PAD_X);
    (w, 2.0 * PAD_Y + lines as f32 * LINE_H)
}

/// The window's rectangle (device px, screen coordinates) for a tip of `size` logical px that sits above an icon whose top edge is
/// `anchor_top` and whose centre is `anchor_center_x`, kept inside the monitor's `work` area. Where there is no room above (an icon high
/// on the screen) the tip goes under the icon (`anchor_bottom`).
pub fn rect(anchor_center_x: i32, anchor_top: i32, anchor_bottom: i32, size: (f32, f32), scale: f32, work: Rect) -> Rect {
    let w = px(size.0, scale).clamp(1, work.width().max(1));
    let h = px(size.1, scale).clamp(1, work.height().max(1));
    let gap = px(GAP, scale);
    let above = anchor_top - gap - h;
    let top = if above >= work.top { above } else { (anchor_bottom + gap).min((work.bottom - h).max(work.top)) };
    let left = (anchor_center_x - w / 2).clamp(work.left, (work.right - w).max(work.left));
    Rect::from_xywh(left, top, w, h)
}

/// Whether the pointer (`cursor`, device px, screen coordinates) is off the icon the open tip belongs to. The toolkit's own hover events
/// do not say when the pointer leaves the island's window altogether (Windows sends no last move), so the app asks this now and then
/// while a tip is open.
pub fn pointer_off(cursor: (i32, i32), icon: Rect) -> bool {
    !icon.contains(cursor.0, cursor.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    type M = TipMachine<&'static str>;

    fn on(m: &mut M, t: u64, k: &'static str) -> TipOutput<&'static str> {
        m.input(t, TipInput::Icon(Some(k)))
    }
    fn off(m: &mut M, t: u64) -> TipOutput<&'static str> {
        m.input(t, TipInput::Icon(None))
    }

    #[test]
    fn resting_on_an_icon_opens_its_tip_after_400_ms_and_not_before() {
        let mut m = M::new();
        assert_eq!(on(&mut m, 1_000, "a"), TipOutput::Nothing);
        assert_eq!(m.deadline(), Some(1_400));
        assert_eq!(m.tick(1_399), TipOutput::Nothing);
        assert_eq!(m.tick(1_400), TipOutput::Show("a"));
        assert_eq!(m.shown(), Some(&"a"));
        assert_eq!(m.deadline(), None, "nothing waits while the pointer stays");
        // the same icon again changes nothing
        assert_eq!(on(&mut m, 2_000, "a"), TipOutput::Nothing);
    }

    #[test]
    fn moving_on_before_the_dwell_starts_the_wait_again() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        assert_eq!(on(&mut m, 300, "b"), TipOutput::Nothing);
        assert_eq!(m.deadline(), Some(700), "the wait for b starts at 300");
        assert_eq!(m.tick(500), TipOutput::Nothing);
        assert_eq!(m.tick(700), TipOutput::Show("b"));
        // left before it opened: nothing opens
        let mut n = M::new();
        on(&mut n, 0, "a");
        off(&mut n, 100);
        assert_eq!(n.deadline(), None);
        assert_eq!(n.tick(1_000), TipOutput::Nothing);
    }

    #[test]
    fn an_open_tip_swaps_at_once_and_closes_at_once() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(on(&mut m, 500, "b"), TipOutput::Swap("b"));
        assert_eq!(m.shown(), Some(&"b"));
        assert_eq!(off(&mut m, 600), TipOutput::Hide);
        assert_eq!(m.shown(), None);
        // coming back to an icon starts the dwell again
        assert_eq!(on(&mut m, 700, "b"), TipOutput::Nothing);
        assert_eq!(m.deadline(), Some(1_100));
    }

    #[test]
    fn a_press_closes_the_tip_and_it_does_not_come_back_until_the_pointer_has_left_the_icon() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(m.input(500, TipInput::Press), TipOutput::Hide);
        // still on "a": no new wait
        assert_eq!(on(&mut m, 600, "a"), TipOutput::Nothing);
        assert_eq!(m.deadline(), None);
        assert_eq!(m.tick(5_000), TipOutput::Nothing);
        // away and back: it may ask again
        off(&mut m, 700);
        on(&mut m, 800, "a");
        assert_eq!(m.deadline(), Some(1_200));
    }

    #[test]
    fn a_block_closes_it_and_keeps_it_from_opening() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(m.input(500, TipInput::Block(true)), TipOutput::Hide);
        assert_eq!(on(&mut m, 600, "b"), TipOutput::Nothing);
        assert_eq!(m.deadline(), None, "blocked: no wait");
        assert_eq!(m.tick(9_000), TipOutput::Nothing);
        // unblocked while the pointer rests on an icon: the wait starts at the next fact
        assert_eq!(m.input(1_000, TipInput::Block(false)), TipOutput::Nothing);
        on(&mut m, 1_100, "b");
        assert_eq!(m.deadline(), Some(1_500));
    }

    #[test]
    fn dismiss_closes_whatever_the_pointer_does() {
        let mut m = M::new();
        on(&mut m, 0, "a");
        m.tick(400);
        assert_eq!(m.dismiss(), TipOutput::Hide);
        assert_eq!(m.dismiss(), TipOutput::Nothing);
        // (it forgot where the pointer was: the next fact starts from nothing)
        assert_eq!(on(&mut m, 1_000, "a"), TipOutput::Nothing);
        assert_eq!(m.deadline(), Some(1_400));
    }

    #[test]
    fn a_tip_is_cleaned_before_it_is_shown() {
        assert_eq!(clean("  Posta \t  3 yeni ileti  "), "Posta 3 yeni ileti");
        assert_eq!(clean("a\r\nb\rc"), "a\nb\nc");
        assert_eq!(clean("a \n b"), "a\nb");
        assert_eq!(clean("a\u{7}b"), "a b", "a control character is a blank");
        assert_eq!(clean(""), "");
        assert_eq!(clean(" \n \t "), "");
        assert_eq!(clean(&"x".repeat(500)).chars().count(), MAX_CHARS);
        assert_eq!(clean("İstanbul ğ"), "İstanbul ğ");
    }

    #[test]
    fn the_window_is_as_wide_as_the_text_up_to_a_limit_and_as_tall_as_its_lines() {
        let (w, h) = size("Posta");
        assert_eq!(w, 5.0 * 7.0 + 20.0);
        assert_eq!(h, 12.0 + 16.0);
        // a long line is wrapped at the widest size
        let (w, h) = size(&"a".repeat(100));
        assert_eq!(w, MAX_W);
        let per_line = ((MAX_W - 20.0) / 7.0).floor() as usize;
        assert_eq!(h, 12.0 + 16.0 * 100usize.div_ceil(per_line) as f32);
        // two lines
        let (_, h) = size("a\nb");
        assert_eq!(h, 12.0 + 32.0);
        // never more than four lines, never narrower than the least
        let (_, h) = size(&"a\n".repeat(20));
        assert_eq!(h, 12.0 + 16.0 * MAX_LINES as f32);
        assert_eq!(size("").0, MIN_W);
    }

    #[test]
    fn a_tip_over_the_strip_stays_clear_of_the_islands_top_edge() {
        // the island's glass starts at 1334 and the 24 px cell inside it at 1346: measured from the cell the tip's bottom (1338) was over the island
        let work = Rect::new(0, 0, 2560, 1400);
        let from_cell = rect(2212, 1346, 1370, (100.0, 28.0), 1.0, work);
        assert_eq!(from_cell.bottom, 1346 - 8);
        assert!(from_cell.bottom > 1334, "(this overlapped the island's top edge)");
        let from_island = rect(2212, 1334, 1370, (100.0, 28.0), 1.0, work);
        assert_eq!(from_island.bottom, 1334 - 8, "measured from the island's top");
    }

    #[test]
    fn the_tip_sits_above_the_icon_centred_and_inside_the_screen() {
        let work = Rect::new(0, 0, 2560, 1392);
        // an icon (2200, 1346)-(2224, 1370): centre 2212; a 100 x 28 tip
        let r = rect(2212, 1346, 1370, (100.0, 28.0), 1.0, work);
        assert_eq!((r.width(), r.height()), (100, 28));
        assert_eq!(r.bottom, 1346 - 8, "8 px above the icon");
        assert_eq!(r.left, 2212 - 50);
        // at the right edge of the screen it is moved in
        let r = rect(2555, 1346, 1370, (100.0, 28.0), 1.0, work);
        assert_eq!(r.right, 2560);
        // at 150 % the numbers scale
        let r = rect(3000, 2000, 2036, (100.0, 28.0), 1.5, Rect::new(0, 0, 3840, 2100));
        assert_eq!((r.width(), r.height(), r.bottom), (150, 42, 2000 - 12));
    }

    #[test]
    fn an_icon_at_the_top_of_the_screen_gets_its_tip_below_it() {
        let work = Rect::new(0, 0, 800, 600);
        let r = rect(100, 4, 28, (100.0, 28.0), 1.0, work);
        assert_eq!(r.top, 28 + 8);
        assert!(work.covers(&r));
        // ... and one at the very bottom, with no room below either, is kept on the screen
        let r = rect(100, 590, 600, (100.0, 28.0), 1.0, Rect::new(0, 0, 800, 600));
        assert!(Rect::new(0, 0, 800, 600).covers(&r), "{r:?}");
    }

    #[test]
    fn the_pointer_is_off_an_icon_outside_its_rectangle() {
        let icon = Rect::new(2200, 1346, 2224, 1370);
        assert!(!pointer_off((2210, 1350), icon));
        assert!(pointer_off((2224, 1350), icon));
        assert!(pointer_off((2210, 1380), icon));
    }
}
