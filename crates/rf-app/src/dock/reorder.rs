//! Reordering the dock's icons by dragging (a user's report: "dockta simgelerin yerini değiştiremiyorum"). Pure: the pointer, the slots
//! and the clock are arguments, so the rules and the state of a drag are tested without a window. `middle.rs` feeds it the pointer and
//! draws what it says; the model's order and the pin file are changed by what [`Drag::finish`] returns.
//!
//! THE RULES. A press that moves no more than [`DRAG_START`] sideways stays a click, whatever else the pointer does. Beyond it the icon is
//! picked up: it follows the pointer along the dock, the other icons make room by sliding into the slots the new order gives them, and on
//! release it stays where it was dropped.
//!
//! * A pinned app goes anywhere among the pinned apps (the dock's pin list is rewritten in that order).
//! * A running app that is not pinned goes anywhere among the running ones (the island's remembered order is rewritten); dropped among
//!   the pinned apps it is pinned there, if it can be started again (a pin that starts nothing is no pin).
//! * The separator, the Recycle Bin and the Downloads folder never move and nothing goes behind them.
//! * Let go well outside the dock, or cancelled with Escape or the right button: everything goes back, nothing is changed. Dragging an
//!   app out never unpins it (unpinning stays in the icon's menu).
//!
//! THE SLOTS are the positions along the dock, left to right; an icon sits in the slot its place in the order gives it, and a slot keeps its
//! size and place for the whole drag (the magnification is held still, `middle.rs::frozen`), so the icons that make room only change slots.

use rf_win::dock::pinorder::Placed;
use rf_win::dock::{DockItem, PinnedApp};

/// How far sideways a press has to move before it is a drag (dock units, which are logical px at the default size).
pub const DRAG_START: f32 = 5.0;
/// How long an icon takes to slide into its new slot.
pub const SLIDE_MS: u32 = 150;
/// How far above the glass's top edge, or below its bottom edge, the pointer is "well outside the dock" (dock units).
pub const OUTSIDE: f32 = 40.0;
/// How much bigger the icon in the hand is drawn (a fraction of its width), and how opaque.
pub const LIFT_GROW: f32 = 0.08;
pub const LIFT_OPACITY: f32 = 0.94;

/// A press that has moved `dx` sideways (dock units): a click or a drag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    Click,
    Drag,
}

pub fn press_kind(dx: f32) -> Press {
    if dx.abs() > DRAG_START { Press::Drag } else { Press::Click }
}

/// What an item of the dock is for the purpose of moving it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// An app pinned to the dock (running or not).
    Pinned,
    /// A running app that is not pinned; `can_pin`: it has a way to be started again, so a pin can be made from it.
    Running { can_pin: bool },
    /// The separator and the places.
    Fixed,
}

pub fn kinds(items: &[DockItem]) -> Vec<Kind> {
    items
        .iter()
        .map(|i| match i {
            DockItem::App(a) if a.pinned => Kind::Pinned,
            DockItem::App(a) => Kind::Running { can_pin: a.launch.is_some() },
            _ => Kind::Fixed,
        })
        .collect()
}

/// How many pinned apps stand first, and how many running ones follow them.
fn counts(kinds: &[Kind]) -> (usize, usize) {
    let pinned = kinds.iter().take_while(|k| **k == Kind::Pinned).count();
    let running = kinds[pinned..].iter().take_while(|k| matches!(k, Kind::Running { .. })).count();
    (pinned, running)
}

/// The slots (first, last, both included) the item `ix` may be carried to; `None` for an item that cannot be picked up.
pub fn range(kinds: &[Kind], ix: usize) -> Option<(usize, usize)> {
    let (np, nr) = counts(kinds);
    match kinds.get(ix)? {
        Kind::Pinned if ix < np => Some((0, np - 1)),
        Kind::Running { can_pin } if ix >= np && ix < np + nr => Some((if *can_pin { 0 } else { np }, np + nr - 1)),
        _ => None,
    }
}

/// Of the slots `lo..=hi`, the one whose centre is nearest to `x` (the nearer-to-the-left one on a tie).
pub fn nearest(centers: &[f32], lo: usize, hi: usize, x: f32) -> usize {
    let hi = hi.min(centers.len().saturating_sub(1));
    let mut best = lo.min(hi);
    for p in lo..=hi {
        if (centers[p] - x).abs() < (centers[best] - x).abs() {
            best = p;
        }
    }
    best
}

/// The order of `len` items with item `from` carried to slot `to`: `order[slot] = item`.
pub fn order_after(len: usize, from: usize, to: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).filter(|i| *i != from).collect();
    order.insert(to.min(order.len()), from);
    order
}

/// What a drop changes in the model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landing {
    Nothing,
    /// A pinned app from slot `from` to slot `to`: the pin list is moved.
    MovePinned { from: usize, to: usize },
    /// A running app that is not pinned, among the running ones (slots, counted from the dock's first).
    MoveRunning { from: usize, to: usize },
    /// A running app dropped among the pinned ones: pinned at index `at`.
    Pin { from: usize, at: usize },
}

/// What dropping item `from` at slot `to` does. `outside`: the pointer is well outside the dock, which cancels.
pub fn drop_result(kinds: &[Kind], from: usize, to: usize, outside: bool) -> Landing {
    if outside || from == to {
        return Landing::Nothing;
    }
    let Some((lo, hi)) = range(kinds, from) else { return Landing::Nothing };
    if to < lo || to > hi {
        return Landing::Nothing;
    }
    let (np, _) = counts(kinds);
    match kinds[from] {
        Kind::Pinned => Landing::MovePinned { from, to },
        Kind::Running { .. } if to < np => Landing::Pin { from, at: to },
        Kind::Running { .. } => Landing::MoveRunning { from, to },
        Kind::Fixed => Landing::Nothing,
    }
}

/// Changes the island's model for a drop: the pin list (`pins`, one pin per pinned item, in the items' order) or the remembered order of the
/// running apps (`seen`, the group keys). `items` are the island's items as they were drawn. False when nothing could be changed (a drop
/// of nothing, an app that cannot be pinned): the model is then as it was.
pub fn apply_landing(landing: Landing, items: &[DockItem], pins: &mut Vec<PinnedApp>, seen: &mut Vec<String>) -> bool {
    let pinned = items.iter().take_while(|i| matches!(i, DockItem::App(a) if a.pinned)).count();
    match landing {
        Landing::MovePinned { from, to } if from < pins.len() && to < pins.len() => {
            move_item(pins, from, to);
            true
        }
        Landing::MoveRunning { from, to } if from >= pinned && to >= pinned => {
            let mut keys: Vec<String> = items.iter().skip(pinned).filter_map(|i| if let DockItem::App(a) = i { (!a.pinned).then(|| a.key.clone()) } else { None }).collect();
            if from - pinned >= keys.len() || to - pinned >= keys.len() {
                return false;
            }
            move_item(&mut keys, from - pinned, to - pinned);
            *seen = fill_slots(seen, &keys);
            true
        }
        Landing::Pin { from, at } => {
            let Some(DockItem::App(app)) = items.get(from) else { return false };
            let Some(pin) = rf_win::dock::ownpins::pin_from_app(app) else { return false };
            pins.insert(at.min(pins.len()), pin.to_pinned());
            true
        }
        _ => false,
    }
}

/// Whether the pointer at height `y` is well outside a glass with top edge `glass_top` and bottom edge `glass_bottom` (dock units).
pub fn is_outside(y: f32, glass_top: f32, glass_bottom: f32) -> bool {
    y < glass_top - OUTSIDE || y > glass_bottom + OUTSIDE
}

/// Whether the pointer at (`x`, `y`) is clearly not on the dock: beside the glass, under it, or above it by more than the headroom a grown
/// icon reaches into (window units; `glass` is `[x, y, w, h]`). Asked when an icon in the hand is let go or put back: the pointer was
/// captured meanwhile, so the dock may never have been told that it left, and the icons would stay grown around a pointer that is gone.
/// A wrong "off" costs nothing: the next move over the dock brings the bell back.
pub fn off_island(x: f32, y: f32, glass: [f32; 4]) -> bool {
    x < glass[0] || x > glass[0] + glass[2] || y < glass[1] - rf_win::dock::layout::HEADROOM || y > glass[1] + glass[3]
}

/// The right button that puts an icon back is not a request for a menu or for Task Manager. Its release comes to two handlers, the
/// icon's (`right_click`) and the row's (`row_right_click`), the icon's first (the toolkit hands a mouse event to the innermost element
/// first: `Window::dispatch_mouse_event` walks its listeners in reverse). The icon's handler only looks; the row's, which comes last, takes
/// the flag, so that both are stopped by the one press. Every right press sets it afresh (a press that put nothing back clears it), so a
/// release that never came to the dock (it happened outside) cannot swallow the next click.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RightGuard {
    armed: bool,
}

impl RightGuard {
    /// The right button went down; `carrying`: an icon was in the hand, and this press puts it back.
    pub fn press(&mut self, carrying: bool) {
        self.armed = carrying;
    }

    /// The release reached an icon: swallowed? (Does not use the flag up: the row's handler comes next.)
    pub fn icon_up(&self) -> bool {
        self.armed
    }

    /// The release reached the row, the last handler: swallowed? Uses the flag up.
    pub fn row_up(&mut self) -> bool {
        std::mem::take(&mut self.armed)
    }
}

/// The places of the pinned apps after a drop, for the pin file (by position among the pinned apps, see `rf_win::dock::pinorder`), when
/// the drop changes the pin list; `pin_count` is how many pins the island showed. `None` for a drop that does not touch the file: the
/// running apps' order is the island's own, kept in memory for the session, and writing nothing for it also asks the shell for nothing.
pub fn placed(landing: Landing, items: &[DockItem], pin_count: usize) -> Option<Vec<Placed>> {
    match landing {
        Landing::MovePinned { from, to } if from < pin_count && to < pin_count => {
            let mut order: Vec<usize> = (0..pin_count).collect();
            move_item(&mut order, from, to);
            Some(order.into_iter().map(Placed::Existing).collect())
        }
        Landing::Pin { from, at } => {
            let Some(DockItem::App(app)) = items.get(from) else { return None };
            let mut order: Vec<Placed> = (0..pin_count).map(Placed::Existing).collect();
            order.insert(at.min(pin_count), Placed::New(Box::new(app.clone())));
            Some(order)
        }
        _ => None,
    }
}

/// Carries the element at `from` to `to`, the ones between shifting by one.
pub fn move_item<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if from >= v.len() || from == to {
        return;
    }
    let x = v.remove(from);
    v.insert(to.min(v.len()), x);
}

/// The list `all` with the elements that are in `subset` put in the order `subset` has, each in a place the subset held before: what
/// the island's remembered order of ALL running apps becomes when the unpinned ones are reordered among themselves. A subset that is not
/// made of exactly the elements it stands for leaves the list as it was.
pub fn fill_slots(all: &[String], subset: &[String]) -> Vec<String> {
    let held: Vec<usize> = all.iter().enumerate().filter(|(_, k)| subset.contains(k)).map(|(i, _)| i).collect();
    let mut sorted_a: Vec<&String> = held.iter().map(|&i| &all[i]).collect();
    let mut sorted_b: Vec<&String> = subset.iter().collect();
    sorted_a.sort();
    sorted_b.sort();
    if sorted_a != sorted_b {
        return all.to_vec();
    }
    let mut out = all.to_vec();
    for (slot, key) in held.iter().zip(subset) {
        out[*slot] = key.clone();
    }
    out
}

/// A slide: the icon is `from` dock units away from its slot at `t0` (ms on the dock's clock) and goes into it as the dock's
/// spring over the slide's time (`rf_win::dock::spring::curve`; a cubic ease-out until 2026-11-13).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Slide {
    pub from: f32,
    pub t0: u64,
}

impl Slide {
    /// How far the icon still is from its slot at `now`.
    pub fn offset(&self, now: u64, duration_ms: u32) -> f32 {
        let elapsed = now.saturating_sub(self.t0);
        if duration_ms == 0 || elapsed >= u64::from(duration_ms) {
            return 0.0;
        }
        self.from * (1.0 - rf_win::dock::spring::curve(elapsed as f32 / duration_ms as f32))
    }

    /// Whether it is still moving at `now`.
    pub fn running(&self, now: u64, duration_ms: u32) -> bool {
        self.from != 0.0 && now.saturating_sub(self.t0) < u64::from(duration_ms)
    }
}

/// The slide of an icon that stood at `old_x` (drawn `old_offset` away from it) and now has its slot at `new_x`.
pub fn slide_from(old_x: f32, old_offset: f32, new_x: f32) -> f32 {
    old_x + old_offset - new_x
}

/// Where the icon in the hand is drawn: the pointer less where it was grabbed, kept between the slots' ends.
pub fn follow_x(pointer_x: f32, grab: f32, lo: f32, hi: f32) -> f32 {
    let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
    (pointer_x - grab).clamp(lo, hi)
}

/// A drag in progress.
#[derive(Clone, Debug)]
pub struct Drag {
    /// The item in the hand (its index before the drag).
    pub ix: usize,
    kinds: Vec<Kind>,
    range: (usize, usize),
    /// Where the pointer was in the icon's slot when it was picked up: the icon keeps that distance from the pointer.
    grab: f32,
    /// `order[slot] = item`.
    order: Vec<usize>,
    /// Per item: how far it still is from its slot. (The item in the hand has its own, `lift`.)
    slides: Vec<Slide>,
    /// The item in the hand: its distance from where the pointer would put it, closing after it came back from outside.
    lift: Slide,
    /// The pointer is well outside the dock: everything is shown as it will be put back.
    outside: bool,
    pointer_x: f32,
    /// Where it was last seen vertically (`None` until the first move after the pick-up).
    pointer_y: Option<f32>,
    /// How long a slide takes: `SLIDE_MS`, or 0 when the app's animations are off (the icons arrive at once).
    dur: u32,
}

/// How a drag ended.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub drop: Landing,
    /// `order[slot] = item` of the items as they were before the drop; the model's new order for a drop that changes something, the
    /// identity otherwise.
    pub order: Vec<usize>,
    /// The slides to play on, indexed by the NEW item index (the slot).
    pub slides: Vec<Slide>,
    /// The duration the slides play over.
    pub dur: u32,
}

impl Drag {
    /// Picks up item `ix` with the pointer at `pointer_x` (window units, as the slots are). `None`: that item cannot be moved.
    pub fn begin(kinds: Vec<Kind>, ix: usize, pointer_x: f32, slots: &[(f32, f32)], dur: u32) -> Option<Drag> {
        let range = range(&kinds, ix)?;
        let slot_x = slots.get(ix)?.0;
        let n = kinds.len();
        Some(Drag { ix, kinds, range, grab: pointer_x - slot_x, order: (0..n).collect(), slides: vec![Slide::default(); n], lift: Slide::default(), outside: false, pointer_x, pointer_y: None, dur })
    }

    /// How many items the drag was made for.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    /// The slot item `item` is in now.
    pub fn slot_of(&self, item: usize) -> usize {
        self.order.iter().position(|&i| i == item).unwrap_or(item)
    }

    /// The slot the item in the hand would take if it were let go now.
    pub fn target(&self) -> usize {
        self.slot_of(self.ix)
    }

    /// Where the pointer was last seen (window units), `None` before the first move.
    pub fn pointer(&self) -> Option<(f32, f32)> {
        self.pointer_y.map(|y| (self.pointer_x, y))
    }

    #[cfg(test)]
    pub fn is_outside(&self) -> bool {
        self.outside
    }

    /// Where the item in the hand is drawn.
    pub fn held_x(&self, slots: &[(f32, f32)], now: u64) -> f32 {
        let base = if self.outside { slots[self.ix].0 } else { self.pointer_base(slots) };
        base + self.lift.offset(now, self.dur)
    }

    fn pointer_base(&self, slots: &[(f32, f32)]) -> f32 {
        follow_x(self.pointer_x, self.grab, slots[self.range.0].0, slots[self.range.1].0)
    }

    /// Where item `item` is drawn: its slot, and the rest of its slide into it. (Not for the item in the hand: `held_x`.)
    pub fn item_x(&self, item: usize, slots: &[(f32, f32)], now: u64) -> f32 {
        slots[self.slot_of(item)].0 + self.slides[item].offset(now, self.dur)
    }

    /// Whether anything is still sliding at `now`.
    pub fn sliding(&self, now: u64) -> bool {
        self.lift.running(now, self.dur) || self.slides.iter().any(|s| s.running(now, self.dur))
    }

    /// The pointer moved to (`x`, `y`), window units; `glass` is the glass `[x, y, w, h]`. True when what is drawn changes.
    pub fn moved(&mut self, x: f32, y: f32, glass: [f32; 4], slots: &[(f32, f32)], now: u64) -> bool {
        if slots.len() != self.order.len() {
            return false;
        }
        let was_x = self.held_x(slots, now);
        let was_outside = self.outside;
        self.pointer_x = x;
        self.pointer_y = Some(y);
        self.outside = is_outside(y, glass[1], glass[1] + glass[3]);
        let target = if self.outside {
            self.ix
        } else {
            // (the width of the icon's own slot, which does not change while the icon is carried: the width of the slot it is over now
            // would move the centre by the difference of two slots' widths at every change of target, and where that carries it back
            // over the boundary the target would flip on every move)
            let held_center = self.pointer_base(slots) + slots[self.ix].1 / 2.0;
            let centers: Vec<f32> = slots.iter().map(|&(sx, w)| sx + w / 2.0).collect();
            nearest(&centers, self.range.0, self.range.1, held_center)
        };
        let mut changed = false;
        if target != self.target() {
            let next = order_after(self.order.len(), self.ix, target);
            for (slot, &item) in next.iter().enumerate() {
                let before = self.slot_of(item);
                if before != slot && item != self.ix {
                    let off = self.slides[item].offset(now, self.dur);
                    self.slides[item] = Slide { from: slide_from(slots[before].0, off, slots[slot].0), t0: now };
                }
            }
            self.order = next;
            changed = true;
        }
        if was_outside != self.outside {
            // the icon in the hand goes back to its place, or leaves it for the pointer again, without a jump
            self.lift = Slide { from: was_x - if self.outside { slots[self.ix].0 } else { self.pointer_base(slots) }, t0: now };
            changed = true;
        }
        changed || (!self.outside && was_x != self.held_x(slots, now))
    }

    /// The drag ends. `commit`: the button was let go (not Escape, not the right button, not a lost release). Where the icons are drawn
    /// at this moment becomes the start of the slides that carry them to their final slots.
    pub fn finish(self, commit: bool, slots: &[(f32, f32)], now: u64) -> Finished {
        let n = self.order.len();
        let held = self.held_x(slots, now);
        let drop = if commit { drop_result(&self.kinds, self.ix, self.target(), self.outside) } else { Landing::Nothing };
        let order: Vec<usize> = if drop == Landing::Nothing { (0..n).collect() } else { self.order.clone() };
        let slides = (0..n)
            .map(|slot| {
                let item = order[slot];
                let at = if item == self.ix { held } else { self.item_x(item, slots, now) };
                Slide { from: slide_from(at, 0.0, slots[slot].0), t0: now }
            })
            .collect();
        Finished { drop, order, slides, dur: self.dur }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two pinned apps, two running ones, the separator and two places: `P P R R | B D`.
    fn row(can_pin: bool) -> Vec<Kind> {
        vec![Kind::Pinned, Kind::Pinned, Kind::Running { can_pin }, Kind::Running { can_pin }, Kind::Fixed, Kind::Fixed, Kind::Fixed]
    }

    /// Slots 54 apart, 44 wide, the separator thin.
    fn slots() -> Vec<(f32, f32)> {
        vec![(12.0, 44.0), (66.0, 44.0), (120.0, 44.0), (174.0, 44.0), (228.0, 1.0), (239.0, 44.0), (293.0, 44.0)]
    }

    const NOW: u64 = 1_000;

    #[test]
    fn a_press_is_a_click_until_it_moves_more_than_a_few_pixels_sideways() {
        assert_eq!(press_kind(0.0), Press::Click);
        assert_eq!(press_kind(DRAG_START), Press::Click, "exactly the limit is still a click");
        assert_eq!(press_kind(-DRAG_START), Press::Click);
        assert_eq!(press_kind(DRAG_START + 0.1), Press::Drag);
        assert_eq!(press_kind(-30.0), Press::Drag, "either way");
    }

    #[test]
    fn a_pinned_app_goes_among_the_pinned_a_running_one_among_the_running_and_the_rest_stay() {
        let k = row(false);
        assert_eq!(range(&k, 0), Some((0, 1)));
        assert_eq!(range(&k, 1), Some((0, 1)));
        assert_eq!(range(&k, 2), Some((2, 3)), "a running app that cannot be pinned stays among the running");
        assert_eq!(range(&k, 3), Some((2, 3)));
        for fixed in 4..7 {
            assert_eq!(range(&k, fixed), None, "the separator and the places are not picked up");
        }
        assert_eq!(range(&k, 9), None);
        // one that can be started again may also go among the pinned
        assert_eq!(range(&row(true), 2), Some((0, 3)));
        // no pinned app at all: the running ones have the whole row of apps
        let only_running = [Kind::Running { can_pin: true }, Kind::Running { can_pin: true }, Kind::Fixed];
        assert_eq!(range(&only_running, 1), Some((0, 1)));
        // nothing but the places
        assert_eq!(range(&[Kind::Fixed, Kind::Fixed], 0), None);
    }

    #[test]
    fn the_nearest_slot_is_found_within_the_allowed_ones() {
        let centers = [34.0, 88.0, 142.0, 196.0];
        assert_eq!(nearest(&centers, 0, 3, 40.0), 0);
        assert_eq!(nearest(&centers, 0, 3, 100.0), 1);
        assert_eq!(nearest(&centers, 0, 3, 1000.0), 3);
        assert_eq!(nearest(&centers, 0, 3, -1000.0), 0);
        assert_eq!(nearest(&centers, 0, 1, 196.0), 1, "never outside the range");
        assert_eq!(nearest(&centers, 2, 3, 0.0), 2);
        assert_eq!(nearest(&centers, 0, 3, 61.0), 0, "a tie goes to the left");
        assert_eq!(nearest(&centers, 3, 9, 1000.0), 3, "a range that runs past the row is cut to it");
    }

    #[test]
    fn the_order_after_carrying_an_item_shifts_the_ones_between() {
        assert_eq!(order_after(5, 1, 3), vec![0, 2, 3, 1, 4]);
        assert_eq!(order_after(5, 3, 1), vec![0, 3, 1, 2, 4]);
        assert_eq!(order_after(5, 2, 2), vec![0, 1, 2, 3, 4]);
        assert_eq!(order_after(3, 0, 2), vec![1, 2, 0]);
    }

    #[test]
    fn a_drop_among_its_own_kind_moves_it_and_a_drop_in_the_same_slot_changes_nothing() {
        let k = row(true);
        assert_eq!(drop_result(&k, 0, 1, false), Landing::MovePinned { from: 0, to: 1 });
        assert_eq!(drop_result(&k, 1, 0, false), Landing::MovePinned { from: 1, to: 0 });
        assert_eq!(drop_result(&k, 3, 2, false), Landing::MoveRunning { from: 3, to: 2 });
        assert_eq!(drop_result(&k, 1, 1, false), Landing::Nothing);
    }

    #[test]
    fn a_running_app_dropped_among_the_pinned_is_pinned_there() {
        let k = row(true);
        assert_eq!(drop_result(&k, 2, 0, false), Landing::Pin { from: 2, at: 0 });
        assert_eq!(drop_result(&k, 3, 1, false), Landing::Pin { from: 3, at: 1 });
        // one that cannot be started again is never pinned
        assert_eq!(drop_result(&row(false), 2, 0, false), Landing::Nothing);
    }

    #[test]
    fn nothing_goes_where_it_may_not_and_the_fixed_items_do_not_move() {
        let k = row(true);
        assert_eq!(drop_result(&k, 0, 2, false), Landing::Nothing, "a pinned app is not dropped among the running ones");
        assert_eq!(drop_result(&k, 0, 5, false), Landing::Nothing, "nor behind the separator");
        assert_eq!(drop_result(&k, 2, 5, false), Landing::Nothing);
        assert_eq!(drop_result(&k, 4, 0, false), Landing::Nothing, "the separator");
        assert_eq!(drop_result(&k, 6, 1, false), Landing::Nothing, "a place");
    }

    #[test]
    fn a_drop_well_outside_the_dock_cancels_and_never_unpins() {
        let k = row(true);
        assert_eq!(drop_result(&k, 0, 1, true), Landing::Nothing);
        assert_eq!(drop_result(&k, 2, 0, true), Landing::Nothing);
        // the glass runs from y 12 to 84: forty units beyond either edge is outside, less is not
        assert!(is_outside(-30.0, 12.0, 84.0));
        assert!(!is_outside(-27.0, 12.0, 84.0));
        assert!(!is_outside(50.0, 12.0, 84.0));
        assert!(is_outside(125.0, 12.0, 84.0));
        assert!(!is_outside(123.0, 12.0, 84.0));
    }

    #[test]
    fn items_move_in_a_list_and_a_subset_fills_the_places_it_held() {
        let mut v = vec![1, 2, 3, 4];
        move_item(&mut v, 0, 2);
        assert_eq!(v, vec![2, 3, 1, 4]);
        move_item(&mut v, 3, 0);
        assert_eq!(v, vec![4, 2, 3, 1]);
        move_item(&mut v, 9, 0);
        assert_eq!(v, vec![4, 2, 3, 1], "an index outside is ignored");
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        // the island remembers p (pinned), a, q (pinned), b; the unpinned a and b swap, the pinned ones keep their places
        assert_eq!(fill_slots(&s(&["p", "a", "q", "b"]), &s(&["b", "a"])), s(&["p", "b", "q", "a"]));
        assert_eq!(fill_slots(&s(&["p", "a", "q", "b"]), &s(&["a", "b"])), s(&["p", "a", "q", "b"]));
        // a subset that is not made of the list's own elements changes nothing
        assert_eq!(fill_slots(&s(&["a", "b"]), &s(&["b", "zzz"])), s(&["a", "b"]));
        assert_eq!(fill_slots(&s(&["a", "b"]), &s(&["a", "a"])), s(&["a", "b"]));
    }

    #[test]
    fn a_slide_starts_away_from_its_slot_ends_in_it_and_never_overshoots() {
        let s = Slide { from: 54.0, t0: 100 };
        assert_eq!(s.offset(100, SLIDE_MS), 54.0);
        assert_eq!(s.offset(100 + u64::from(SLIDE_MS), SLIDE_MS), 0.0);
        assert_eq!(s.offset(10_000, SLIDE_MS), 0.0);
        assert_eq!(s.offset(50, SLIDE_MS), 54.0, "a clock that is behind is the start");
        let mut last = 54.0;
        for ms in 0..=SLIDE_MS {
            let o = s.offset(100 + u64::from(ms), SLIDE_MS);
            assert!((0.0..=54.0).contains(&o) && o <= last + 1e-4, "monotonic and bounded at {ms}: {o}");
            last = o;
        }
        let back = Slide { from: -54.0, t0: 0 };
        assert!((-54.0..=0.0).contains(&back.offset(40, SLIDE_MS)));
        // no animation: arrives at once
        assert_eq!(s.offset(100, 0), 0.0);
        assert!(s.running(120, SLIDE_MS) && !s.running(100 + u64::from(SLIDE_MS), SLIDE_MS));
        assert!(!Slide::default().running(0, SLIDE_MS));
    }

    #[test]
    fn a_slide_starts_where_the_icon_was_drawn() {
        // it stood in the slot at 66, 10 units on the way somewhere, and its new slot is at 120: it is at 76, so 44 units to go back
        assert_eq!(slide_from(66.0, 10.0, 120.0), -44.0);
        assert_eq!(slide_from(120.0, 0.0, 66.0), 54.0);
    }

    #[test]
    fn the_icon_in_the_hand_follows_the_pointer_between_the_ends_of_its_zone() {
        assert_eq!(follow_x(100.0, 20.0, 12.0, 66.0), 66.0, "held at the end of its zone");
        assert_eq!(follow_x(50.0, 20.0, 12.0, 66.0), 30.0);
        assert_eq!(follow_x(0.0, 20.0, 12.0, 66.0), 12.0);
        assert_eq!(follow_x(50.0, 20.0, 66.0, 12.0), 30.0, "ends the wrong way round are tolerated");
    }

    fn pin_of(name: &str) -> rf_win::dock::PinnedApp {
        use rf_win::dock::{IconSource, Launch};
        let exe = format!("c:/apps/{name}.exe");
        rf_win::dock::PinnedApp { name: name.into(), aumid: None, exe: Some(exe.clone()), launch: Launch::Exe { path: exe.clone().into(), args: String::new() }, icon: IconSource::File(exe.into()) }
    }

    fn running(name: &str, hwnd: isize) -> (String, Vec<rf_win::tasklist::Task>) {
        let task = rf_win::tasklist::Task { hwnd, title: name.into(), pid: hwnd as u32, exe: Some(format!("c:/apps/{name}.exe").into()), app_id: None, minimized: false };
        (task.group(), vec![task])
    }

    /// The names of the apps in the items, in order.
    fn names(items: &[DockItem]) -> Vec<String> {
        items.iter().filter_map(|i| if let DockItem::App(a) = i { Some(format!("{}{}", a.name.to_lowercase(), if a.pinned { "*" } else { "" })) } else { None }).collect()
    }

    /// The model after a drop, rebuilt the way the island rebuilds it, shows the apps in the order the drag showed.
    #[test]
    fn the_models_new_order_is_the_order_the_drag_showed() {
        use rf_win::dock::model;
        // pinned a and b (b not running), running and not pinned: x then y
        let mut pins = vec![pin_of("a"), pin_of("b")];
        let groups = || vec![running("a", 1), running("x", 2), running("y", 3)];
        let mut seen: Vec<String> = Vec::new();
        let build = |pins: &[rf_win::dock::PinnedApp], seen: &mut Vec<String>| model::merge(pins, model::keep_order(seen, groups()), 0);
        let items = build(&pins, &mut seen);
        assert_eq!(names(&items), ["a*", "b*", "x", "y"]);
        let k = kinds(&items);
        let s = slots();
        let shown = |end: &Finished| end.order.iter().take(4).map(|&i| names(&items)[i].clone()).collect::<Vec<_>>();

        // the pinned a is carried behind b
        let mut d = Drag::begin(k.clone(), 0, 22.0, &s, SLIDE_MS).unwrap();
        d.moved(70.0, 50.0, glass(), &s, NOW);
        let end = d.finish(true, &s, NOW);
        let (mut p2, mut seen2) = (pins.clone(), seen.clone());
        assert!(apply_landing(end.drop, &items, &mut p2, &mut seen2));
        assert_eq!(names(&build(&p2, &mut seen2)), shown(&end), "what the drag showed");
        assert_eq!(names(&build(&p2, &mut seen2)), ["b*", "a*", "x", "y"]);

        // the running y is carried in front of x: only the remembered order changes, no pin is made
        let mut d = Drag::begin(k.clone(), 3, 180.0, &s, SLIDE_MS).unwrap();
        d.moved(100.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 2);
        let end = d.finish(true, &s, NOW);
        let (mut p2, mut seen2) = (pins.clone(), seen.clone());
        assert!(apply_landing(end.drop, &items, &mut p2, &mut seen2));
        assert_eq!(p2, pins, "no pin is made or moved");
        assert_eq!(names(&build(&p2, &mut seen2)), shown(&end));
        assert_eq!(names(&build(&p2, &mut seen2)), ["a*", "b*", "y", "x"]);

        // the running x is carried between the two pinned apps: it is pinned there
        let mut d = Drag::begin(k.clone(), 2, 130.0, &s, SLIDE_MS).unwrap();
        d.moved(66.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 1);
        let end = d.finish(true, &s, NOW);
        assert_eq!(end.drop, Landing::Pin { from: 2, at: 1 });
        let (mut p2, mut seen2) = (pins.clone(), seen.clone());
        assert!(apply_landing(end.drop, &items, &mut p2, &mut seen2));
        assert_eq!(p2.len(), 3);
        assert_eq!(names(&build(&p2, &mut seen2)), ["a*", "x*", "b*", "y"]);
        // a drop that changes nothing, and one that was cancelled, leave the model alone
        pins.truncate(2);
        let (mut p2, mut seen2) = (pins.clone(), seen.clone());
        assert!(!apply_landing(Landing::Nothing, &items, &mut p2, &mut seen2));
        assert_eq!((p2, seen2), (pins, seen));
    }

    fn glass() -> [f32; 4] {
        [0.0, 12.0, 400.0, 72.0]
    }

    #[test]
    fn carrying_an_icon_across_makes_the_others_slide_into_the_room() {
        let s = slots();
        // item 0 picked up where the pointer is 10 units into it
        let mut d = Drag::begin(row(true), 0, 22.0, &s, SLIDE_MS).unwrap();
        assert_eq!(d.target(), 0);
        // the pointer moves right, over the second slot: the icon follows (60 = 70 less the 10 it was grabbed at), the second icon moves
        // into the first slot
        assert!(d.moved(70.0, 50.0, glass(), &s, NOW));
        assert_eq!(d.target(), 1);
        assert_eq!(d.slot_of(1), 0);
        // the second icon is drawn where it stood (66) and slides to 12: at the moment of the change it is still at 66
        assert!((d.item_x(1, &s, NOW) - 66.0).abs() < 1e-3);
        assert!((d.item_x(1, &s, NOW + u64::from(SLIDE_MS)) - 12.0).abs() < 1e-3);
        assert!(d.sliding(NOW) && !d.sliding(NOW + u64::from(SLIDE_MS)));
        // the icon in the hand is not in a slot: it is under the pointer
        assert!((d.held_x(&s, NOW) - 60.0).abs() < 1e-3);
        // it cannot be carried out of its zone: way to the right it stops at the last pinned slot
        d.moved(900.0, 50.0, glass(), &s, NOW + 10);
        assert_eq!(d.target(), 1);
        assert!((d.held_x(&s, NOW + 10) - 66.0).abs() < 1e-3);
    }

    #[test]
    fn a_small_move_inside_the_slot_changes_the_look_but_not_the_order() {
        let s = slots();
        let mut d = Drag::begin(row(true), 0, 22.0, &s, SLIDE_MS).unwrap();
        assert!(d.moved(30.0, 50.0, glass(), &s, NOW), "the icon follows the pointer");
        assert_eq!(d.target(), 0);
        assert!(!d.moved(30.0, 50.0, glass(), &s, NOW), "the same place again draws nothing new");
    }

    #[test]
    fn dropping_commits_the_order_and_the_slides_start_where_the_icons_were_drawn() {
        let s = slots();
        let mut d = Drag::begin(row(true), 0, 22.0, &s, SLIDE_MS).unwrap();
        d.moved(70.0, 50.0, glass(), &s, NOW);
        let end = d.finish(true, &s, NOW + 10);
        assert_eq!(end.drop, Landing::MovePinned { from: 0, to: 1 });
        assert_eq!(end.order, vec![1, 0, 2, 3, 4, 5, 6]);
        // new item 0 is the old second icon, 10 ms into its slide from 66 to 12: it is still far from its slot (a spring picks up
        // speed: it has gone a tenth of the 54 units)
        assert!(end.slides[0].from > 46.0 && end.slides[0].from < 51.0, "{:?}", end.slides[0]);
        // new item 1 is the dragged one, drawn at 60 and let go over the slot at 66
        assert!((end.slides[1].from + 6.0).abs() < 1e-3, "{:?}", end.slides[1]);
        // the items that did not move have nothing to slide
        assert_eq!(end.slides[2].from, 0.0);
    }

    #[test]
    fn dropping_a_running_app_among_the_pinned_pins_it_and_dropping_it_on_the_places_does_not() {
        let s = slots();
        let mut d = Drag::begin(row(true), 2, 130.0, &s, SLIDE_MS).unwrap();
        d.moved(20.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 0);
        assert_eq!(d.clone().finish(true, &s, NOW).drop, Landing::Pin { from: 2, at: 0 });
        // carried far to the right it stops at the last running slot: a move among the running ones
        let mut d = Drag::begin(row(true), 2, 130.0, &s, SLIDE_MS).unwrap();
        d.moved(900.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 3);
        assert_eq!(d.finish(true, &s, NOW).drop, Landing::MoveRunning { from: 2, to: 3 });
        // one that cannot be pinned stays among the running ones whatever the pointer does
        let mut d = Drag::begin(row(false), 3, 180.0, &s, SLIDE_MS).unwrap();
        d.moved(-500.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 2);
    }

    #[test]
    fn the_places_and_the_separator_cannot_be_picked_up() {
        let s = slots();
        for fixed in 4..7 {
            assert!(Drag::begin(row(true), fixed, 100.0, &s, SLIDE_MS).is_none());
        }
    }

    #[test]
    fn leaving_the_dock_far_above_puts_everything_back_and_a_drop_there_does_nothing() {
        let s = slots();
        let mut d = Drag::begin(row(true), 0, 22.0, &s, SLIDE_MS).unwrap();
        d.moved(100.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 1);
        // the pointer goes well above the glass: the order is the old one again and the icon in the hand shows its place
        assert!(d.moved(100.0, -60.0, glass(), &s, NOW + 20));
        assert!(d.is_outside());
        assert_eq!(d.target(), 0);
        assert!((d.held_x(&s, NOW + 20 + u64::from(SLIDE_MS)) - 12.0).abs() < 1e-3);
        // let go there: nothing changes, and the icons are all in their old slots
        let end = d.clone().finish(true, &s, NOW + 30);
        assert_eq!(end.drop, Landing::Nothing);
        assert_eq!(end.order, vec![0, 1, 2, 3, 4, 5, 6]);
        // back inside the dock the drag goes on
        d.moved(100.0, 50.0, glass(), &s, NOW + 40);
        assert!(!d.is_outside());
        assert_eq!(d.target(), 1);
    }

    #[test]
    fn cancelling_with_escape_or_the_right_button_puts_everything_back() {
        let s = slots();
        let mut d = Drag::begin(row(true), 1, 80.0, &s, SLIDE_MS).unwrap();
        d.moved(20.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.target(), 0);
        let end = d.finish(false, &s, NOW + 5);
        assert_eq!(end.drop, Landing::Nothing);
        assert_eq!(end.order, vec![0, 1, 2, 3, 4, 5, 6]);
        // the icon in the hand slides back from where it was drawn (held at the first slot, 12; its own slot is 66)
        assert!(end.slides[1].from < -50.0 && end.slides[1].from > -61.0, "{:?}", end.slides[1]);
    }

    #[test]
    fn a_pointer_that_is_beside_under_or_far_above_the_glass_is_off_the_dock() {
        let g = [100.0, 12.0, 300.0, 72.0];
        assert!(!off_island(200.0, 50.0, g));
        assert!(!off_island(100.0, 12.0, g), "the glass's corners are on it");
        assert!(!off_island(400.0, 84.0, g));
        // a grown icon reaches above the glass into the headroom
        assert!(!off_island(200.0, 12.0 - rf_win::dock::layout::HEADROOM, g));
        assert!(off_island(200.0, 12.0 - rf_win::dock::layout::HEADROOM - 1.0, g));
        assert!(off_island(99.0, 50.0, g) && off_island(401.0, 50.0, g));
        assert!(off_island(200.0, 85.0, g));
    }

    #[test]
    fn the_drag_remembers_where_the_pointer_was_last() {
        let s = slots();
        let mut d = Drag::begin(row(true), 0, 22.0, &s, SLIDE_MS).unwrap();
        assert_eq!(d.pointer(), None);
        d.moved(70.0, 130.0, glass(), &s, NOW);
        assert_eq!(d.pointer(), Some((70.0, 130.0)));
    }

    #[test]
    fn the_right_press_that_puts_an_icon_back_is_swallowed_by_both_handlers_and_only_once() {
        let mut g = RightGuard::default();
        // nothing was in the hand: the release is a request for a menu
        g.press(false);
        assert!(!g.icon_up() && !g.row_up());
        // an icon was put back: the release reaches the icon (it only looks), then the row (which uses the flag up)
        g.press(true);
        assert!(g.icon_up(), "the icon's handler is stopped");
        assert!(g.row_up(), "and so is the row's, which no longer sees a drag");
        assert!(!g.icon_up() && !g.row_up(), "the next release is a real one");
    }

    #[test]
    fn a_swallowed_release_that_never_came_does_not_swallow_the_next_click() {
        // cancelled with the right button, let go outside the dock (no handler heard it), and later a right click on an icon
        let mut g = RightGuard::default();
        g.press(true);
        g.press(false);
        assert!(!g.icon_up() && !g.row_up());
    }

    /// The items of a dock with pins `a`, `b` and a running `x` and `y`, and the pin count 2.
    fn model_items() -> Vec<DockItem> {
        use rf_win::dock::model;
        let pins = vec![pin_of("a"), pin_of("b")];
        model::merge(&pins, vec![running("x", 2), running("y", 3)], 0)
    }

    #[test]
    fn only_a_drop_that_changes_the_pins_is_told_to_the_file_and_it_is_told_by_position() {
        let items = model_items();
        let positions = |p: Option<Vec<Placed>>| -> Vec<String> {
            p.unwrap().iter().map(|x| match x {
                Placed::Existing(i) => i.to_string(),
                Placed::New(a) => a.name.clone(),
            }).collect()
        };
        assert_eq!(positions(placed(Landing::MovePinned { from: 0, to: 1 }, &items, 2)), ["1", "0"]);
        assert_eq!(positions(placed(Landing::MovePinned { from: 1, to: 0 }, &items, 2)), ["1", "0"]);
        // a running app dropped between the pinned ones is pinned there (item 2 is the running x)
        assert_eq!(positions(placed(Landing::Pin { from: 2, at: 1 }, &items, 2)), ["0", "X", "1"]);
        assert_eq!(positions(placed(Landing::Pin { from: 3, at: 0 }, &items, 2)), ["Y", "0", "1"]);
        // the running apps' order is the island's own, and nothing at all changes nothing
        assert!(placed(Landing::MoveRunning { from: 2, to: 3 }, &items, 2).is_none());
        assert!(placed(Landing::Nothing, &items, 2).is_none());
        // a pin that is not there, an item that is not an app
        assert!(placed(Landing::MovePinned { from: 0, to: 5 }, &items, 2).is_none());
        assert!(placed(Landing::Pin { from: 4, at: 0 }, &items, 2).is_none(), "the separator is not an app");
    }

    #[test]
    fn what_the_file_gets_is_the_order_the_model_shows() {
        use rf_win::dock::ownpins::PinFile;
        let items = model_items();
        let k = kinds(&items);
        let s = slots();
        let file_of = |pins: &[rf_win::dock::PinnedApp]| {
            let mut f = PinFile::default();
            f.seed(pins);
            f
        };
        let names = |f: &PinFile| f.pins.iter().map(|p| p.name.to_lowercase()).collect::<Vec<_>>();
        let pins = vec![pin_of("a"), pin_of("b")];
        // the pinned a carried behind b
        let mut d = Drag::begin(k.clone(), 0, 22.0, &s, SLIDE_MS).unwrap();
        d.moved(70.0, 50.0, glass(), &s, NOW);
        let end = d.finish(true, &s, NOW);
        let (mut p2, mut seen) = (pins.clone(), Vec::new());
        let placed_order = placed(end.drop, &items, 2).unwrap();
        assert!(apply_landing(end.drop, &items, &mut p2, &mut seen));
        let mut f = file_of(&pins);
        assert!(f.apply_order(&placed_order, &["a".to_string(), "b".to_string()], |_| true));
        assert_eq!(names(&f), p2.iter().map(|p| p.name.to_lowercase()).collect::<Vec<_>>(), "the file and the model agree");
        assert_eq!(names(&f), ["b", "a"]);
        // the running x carried between them
        let mut d = Drag::begin(k, 2, 130.0, &s, SLIDE_MS).unwrap();
        d.moved(66.0, 50.0, glass(), &s, NOW);
        let end = d.finish(true, &s, NOW);
        let (mut p3, mut seen) = (pins.clone(), Vec::new());
        let placed_order = placed(end.drop, &items, 2).unwrap();
        assert!(apply_landing(end.drop, &items, &mut p3, &mut seen));
        let mut f = file_of(&pins);
        assert!(f.apply_order(&placed_order, &["a".to_string(), "b".to_string()], |_| true));
        assert_eq!(names(&f), p3.iter().map(|p| p.name.to_lowercase()).collect::<Vec<_>>());
        assert_eq!(names(&f), ["a", "x", "b"]);
    }

    /// The bell is held while an icon is carried, so the slots differ in width (68 under the pointer's place at pick-up down to 44).
    fn bell_slots() -> Vec<(f32, f32)> {
        vec![(12.0, 68.0), (90.0, 60.0), (160.0, 52.0), (222.0, 48.0), (280.0, 1.0), (291.0, 44.0), (345.0, 44.0)]
    }

    fn four_pinned() -> Vec<Kind> {
        vec![Kind::Pinned, Kind::Pinned, Kind::Pinned, Kind::Pinned, Kind::Fixed, Kind::Fixed, Kind::Fixed]
    }

    #[test]
    fn slots_of_different_widths_do_not_make_the_target_flip_back_and_forth_near_a_boundary() {
        let s = bell_slots();
        let mut d = Drag::begin(four_pinned(), 0, 30.0, &s, SLIDE_MS).unwrap();
        // a slow sweep to the right: the target may only grow, one change per boundary crossed
        let (mut last, mut changes) = (d.target(), 0);
        let mut x = 30.0;
        while x < 330.0 {
            d.moved(x, 50.0, glass(), &s, NOW);
            assert!(d.target() >= last, "the target went back from {last} to {} at x = {x}", d.target());
            if d.target() != last {
                changes += 1;
                last = d.target();
            }
            x += 0.25;
        }
        assert_eq!((last, changes), (3, 3));
        // and back to the left the same way
        while x > 30.0 {
            d.moved(x, 50.0, glass(), &s, NOW);
            assert!(d.target() <= last, "the target went forward from {last} to {} at x = {x}", d.target());
            last = d.target();
            x -= 0.25;
        }
        assert_eq!(last, 0);
    }

    #[test]
    fn a_drag_that_ends_where_it_began_changes_nothing() {
        let s = slots();
        let mut d = Drag::begin(row(true), 1, 80.0, &s, SLIDE_MS).unwrap();
        d.moved(84.0, 50.0, glass(), &s, NOW);
        assert_eq!(d.finish(true, &s, NOW).drop, Landing::Nothing);
    }
}
