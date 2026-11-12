//! The magnification of the dock's icons, like the Mac's dock. Pure arithmetic: the pointer, the rest layout, the strength and the clock
//! are arguments, so any moment of the animation can be worked out and tested without a window.
//!
//! At rest an icon is `ICON_REST` (44) px. With the pointer over the dock every icon grows by a smooth bell of the horizontal distance
//! between the pointer and the icon's REST centre (a raised cosine over `REACH_SLOTS` icon slots to each side), so the size is a
//! continuous function of the pointer: moving from one icon to the next changes every size a little, never one icon by a step. The
//! icons keep standing on one baseline (the view draws them from the bottom up), push their neighbours apart so nothing overlaps, and
//! the dock's glass widens with them, symmetrically around the island's centre.
//!
//! Coming in and going out are eased by [`Presence`] over `FADE_MS`: the bell's height goes from 0 to 1 and back, while the bell itself
//! sits where the pointer last was (so it shrinks away in place when the pointer leaves). While the pointer moves inside, the
//! presence is 1 and the sizes follow the pointer directly.

use super::hover::Slot;
use super::layout::{DOCK_PADDING, ICON_GAP, ICON_REST, SEPARATOR_WIDTH};

/// How far the bell reaches to each side, in icon slots (an icon and the gap after it).
pub const REACH_SLOTS: f32 = 2.5;
/// The same in logical px.
pub const REACH: f32 = REACH_SLOTS * (ICON_REST + ICON_GAP);
/// How long coming in and going out take.
pub const FADE_MS: u32 = 160;
/// The least gap two items are squeezed to when the dock has no more room to widen.
pub const MIN_GAP: f32 = 2.0;

/// How much the icon under the pointer grows (Settings: off / subtle / full).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Strength {
    Off,
    Subtle,
    #[default]
    Full,
}

impl Strength {
    /// The size factor of the icon right under the pointer.
    pub const fn peak(self) -> f32 {
        match self {
            Strength::Off => 1.0,
            Strength::Subtle => 1.25,
            Strength::Full => 1.55,
        }
    }
}

/// The biggest size factor any strength gives; icon bitmaps are loaded for this.
pub const MAX_PEAK: f32 = Strength::Full.peak();

/// The bell: 1 at the pointer, 0 at `REACH` px and beyond, a raised cosine between (zero slope at both ends, so no kink).
pub fn bell(distance: f32) -> f32 {
    let u = (distance.abs() / REACH).min(1.0);
    0.5 * (1.0 + (std::f32::consts::PI * u).cos())
}

/// The size factor of an icon whose rest centre is `distance` px from the pointer. `presence` is the bell's height, 0 to 1.
pub fn factor(distance: f32, strength: Strength, presence: f32) -> f32 {
    1.0 + (strength.peak() - 1.0) * presence.clamp(0.0, 1.0) * bell(distance)
}

/// The width of a row of items for these icon sizes, with the usual gap between every two items.
pub fn natural_width(slots: &[Slot], icon_sizes: &[f32]) -> f32 {
    if slots.is_empty() {
        return 2.0 * DOCK_PADDING;
    }
    let seps = slots.iter().filter(|s| **s == Slot::Separator).count();
    2.0 * DOCK_PADDING + icon_sizes.iter().sum::<f32>() + seps as f32 * SEPARATOR_WIDTH + (slots.len() - 1) as f32 * ICON_GAP
}

/// Where each item lies (`(x, width)`, logical px from the row's left edge, the padding included) while the icons have `icon_sizes`
/// (one per [`Slot::Icon`]) inside a row that is `width` wide. The row does not change its width: the gaps between the items give way
/// (down to `MIN_GAP`), and only when even that is not enough all icons shrink together to fit. At rest (44 px, 10 px gaps) this is
/// exactly `hover::slot_spans`.
pub fn fit(slots: &[Slot], icon_sizes: &[f32], width: f32) -> Vec<(f32, f32)> {
    if slots.is_empty() {
        return Vec::new();
    }
    let inner = (width - 2.0 * DOCK_PADDING).max(0.0);
    let separators = slots.iter().filter(|s| **s == Slot::Separator).count();
    let icon_total: f32 = icon_sizes.iter().sum();
    let gaps = slots.len() - 1;
    let fixed = icon_total + separators as f32 * SEPARATOR_WIDTH;
    let (gap, k) = if gaps == 0 {
        (0.0, if icon_total > inner && icon_total > 0.0 { inner / icon_total } else { 1.0 })
    } else {
        let gap = ((inner - fixed) / gaps as f32).min(ICON_GAP);
        if gap >= MIN_GAP {
            (gap, 1.0)
        } else {
            // not even the least gaps fit: all icons give a little
            let room = inner - separators as f32 * SEPARATOR_WIDTH - gaps as f32 * MIN_GAP;
            (MIN_GAP, if icon_total > 0.0 { (room / icon_total).clamp(0.0, 1.0) } else { 1.0 })
        }
    };
    let mut x = DOCK_PADDING;
    let mut icon = 0usize;
    let mut out = Vec::with_capacity(slots.len());
    for (i, s) in slots.iter().enumerate() {
        if i > 0 {
            x += gap;
        }
        let w = match s {
            Slot::Icon => {
                let w = icon_sizes.get(icon).copied().unwrap_or(ICON_REST) * k;
                icon += 1;
                w
            }
            Slot::Separator => SEPARATOR_WIDTH,
        };
        out.push((x, w));
        x += w;
    }
    out
}

/// One moment of the dock: where every item is and how wide the glass is. Coordinates are logical px along the dock, with 0 at the
/// REST island's left edge (so `left` is negative while the dock is wider than at rest).
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// `(x, width)` of every slot (icons AND separators), in slot order.
    pub spans: Vec<(f32, f32)>,
    /// The glass: its left edge and its width. The height does not change.
    pub left: f32,
    pub width: f32,
}

impl Frame {
    /// The frame with nothing magnified: the rest layout as it was given.
    pub fn at_rest(rest: &[(f32, f32)], rest_width: f32) -> Frame {
        Frame { spans: rest.to_vec(), left: 0.0, width: rest_width }
    }
}

/// The dock while the pointer is at `pointer_x` (logical px from the REST island's left edge) with the bell at `presence` (0 to 1).
///
/// * `rest`: where the slots lie at rest, `(x, width)` each, from the left edge of an island `rest_width` wide.
/// * `max_width`: the widest the glass may become (the room the neighbours and the window leave); at least `rest_width`.
///
/// The icons' sizes are their rest sizes times [`factor`] of their rest centre's distance from the pointer. The glass is the rest width
/// plus what the icons grew (or `max_width`, below which the gaps and then the icons give way), centred on the rest island's centre.
pub fn frame(slots: &[Slot], rest: &[(f32, f32)], rest_width: f32, pointer_x: f32, presence: f32, strength: Strength, max_width: f32) -> Frame {
    if slots.is_empty() || rest.len() != slots.len() {
        return Frame::at_rest(rest, rest_width);
    }
    let sizes: Vec<f32> = slots
        .iter()
        .zip(rest)
        .filter(|(s, _)| **s == Slot::Icon)
        .map(|(_, &(x, w))| w * factor(pointer_x - (x + w / 2.0), strength, presence))
        .collect();
    let max_width = max_width.max(rest_width);
    // The glass widens by what the icons grew, on top of the rest width: the rest layout may already have tighter gaps or smaller icons
    // than the usual (a narrow screen), and the widening must start from that, not from the width the usual gaps would give, or the
    // glass would jump the moment the bell begins.
    let grown: f32 = slots.iter().zip(rest).filter(|(s, _)| **s == Slot::Icon).map(|(_, &(_, w))| w).zip(&sizes).map(|(rest_w, &now)| now - rest_w).sum();
    let width = (rest_width + grown.max(0.0)).clamp(rest_width, max_width);
    let left = (rest_width - width) / 2.0;
    let spans = fit(slots, &sizes, width).into_iter().map(|(x, w)| (x + left, w)).collect();
    Frame { spans, left, width }
}

/// The bell's height over time. `enter` when the pointer comes onto the dock, `leave` when it goes; the height follows with an
/// ease in and out over `duration_ms` (0: the animations setting is off, it jumps). A change in mid flight starts from where the
/// height is, so nothing jumps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presence {
    from: f32,
    to: f32,
    started_ms: u64,
    duration_ms: u32,
}

/// Smoothstep: 0 to 1 for `t` 0 to 1, slow at both ends.
pub fn ease_in_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl Presence {
    /// Away. `duration_ms` is [`FADE_MS`], or 0 when animations are off.
    pub fn new(duration_ms: u32) -> Presence {
        Presence { from: 0.0, to: 0.0, started_ms: 0, duration_ms }
    }

    pub fn set_duration(&mut self, duration_ms: u32) {
        self.duration_ms = duration_ms;
    }

    /// The height at `now_ms`.
    pub fn value(&self, now_ms: u64) -> f32 {
        let elapsed = now_ms.saturating_sub(self.started_ms) as f32;
        if self.duration_ms == 0 || elapsed >= self.duration_ms as f32 {
            return self.to;
        }
        self.from + (self.to - self.from) * ease_in_out(elapsed / self.duration_ms as f32)
    }

    fn go(&mut self, to: f32, now_ms: u64) {
        if self.to == to {
            return;
        }
        self.from = self.value(now_ms);
        self.to = to;
        self.started_ms = now_ms;
    }

    /// The pointer is over the dock. The same call again changes nothing.
    pub fn enter(&mut self, now_ms: u64) {
        self.go(1.0, now_ms);
    }

    /// The pointer left.
    pub fn leave(&mut self, now_ms: u64) {
        self.go(0.0, now_ms);
    }

    /// Whether the pointer is (as far as the dock knows) over the dock.
    pub fn is_in(&self) -> bool {
        self.to == 1.0
    }

    /// True once the height has reached its target: no more frames are needed until the pointer moves or leaves.
    pub fn settled(&self, now_ms: u64) -> bool {
        self.duration_ms == 0 || now_ms.saturating_sub(self.started_ms) >= self.duration_ms as u64 || self.from == self.to
    }

    /// True when the height is 0 and staying there: the dock is at rest and nothing needs to be drawn differently.
    pub fn at_rest(&self, now_ms: u64) -> bool {
        self.to == 0.0 && self.settled(now_ms)
    }
}

/// The room the glass may widen into, logical px in all (half of it on each side): the least of what the window has set aside and what
/// the neighbouring islands leave, never negative. `reserve` is each side's own, `gap_left` / `gap_right` the free distance to the
/// neighbours (already without their minimum gap).
pub fn widening_room(reserve: f32, gap_left: f32, gap_right: f32) -> f32 {
    2.0 * reserve.min(gap_left).min(gap_right).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::hover::slot_spans;

    fn row(icons_before: usize, icons_after: usize) -> (Vec<Slot>, Vec<(f32, f32)>, f32) {
        let mut slots = vec![Slot::Icon; icons_before];
        slots.push(Slot::Separator);
        slots.extend(vec![Slot::Icon; icons_after]);
        let sizes = vec![ICON_REST; icons_before + icons_after];
        let rest = slot_spans(&slots, &sizes);
        let width = natural_width(&slots, &sizes);
        (slots, rest, width)
    }

    fn icons_only(n: usize) -> (Vec<Slot>, Vec<(f32, f32)>, f32) {
        let slots = vec![Slot::Icon; n];
        let sizes = vec![ICON_REST; n];
        (slots.clone(), slot_spans(&slots, &sizes), natural_width(&slots, &sizes))
    }

    #[test]
    fn the_bell_is_one_at_the_pointer_zero_at_the_reach_and_smooth() {
        assert_eq!(bell(0.0), 1.0);
        assert!(bell(REACH).abs() < 1e-6 && bell(REACH * 3.0) == 0.0);
        assert_eq!(bell(-40.0), bell(40.0), "symmetric");
        let mut last = 1.0;
        for d in 0..=(REACH as i32) {
            let b = bell(d as f32);
            assert!(b <= last + 1e-6, "falls monotonically");
            last = b;
        }
        // about 2.5 slots: the icon one slot away is clearly smaller than the one under the pointer, two slots away barely grown
        let slot = ICON_REST + ICON_GAP;
        assert!((0.5..0.8).contains(&bell(slot)), "{}", bell(slot));
        assert!(bell(2.0 * slot) < 0.15, "{}", bell(2.0 * slot));
    }

    #[test]
    fn the_strengths_are_off_subtle_full() {
        assert_eq!(Strength::Off.peak(), 1.0);
        assert_eq!(Strength::Subtle.peak(), 1.25);
        assert_eq!(Strength::Full.peak(), 1.55);
        assert_eq!(Strength::default(), Strength::Full);
        assert_eq!(factor(0.0, Strength::Full, 1.0), 1.55);
        assert_eq!(factor(0.0, Strength::Full, 0.0), 1.0, "bell at height 0: rest");
        assert_eq!(factor(0.0, Strength::Off, 1.0), 1.0);
        assert!((factor(0.0, Strength::Subtle, 0.5) - 1.125).abs() < 1e-6);
        assert_eq!(factor(10.0, Strength::Full, 7.0), factor(10.0, Strength::Full, 1.0), "presence is cut to 0..1");
    }

    #[test]
    fn the_item_under_the_pointer_is_the_biggest_and_the_ones_beside_it_less() {
        let (slots, rest, w) = icons_only(9);
        let centre = rest[4].0 + rest[4].1 / 2.0;
        let f = frame(&slots, &rest, w, centre, 1.0, Strength::Full, w + 200.0);
        let sizes: Vec<f32> = f.spans.iter().map(|s| s.1).collect();
        assert!((sizes[4] - 44.0 * 1.55).abs() < 1e-3, "{}", sizes[4]);
        assert!(sizes[3] < sizes[4] && sizes[3] > sizes[2] && sizes[2] > sizes[1] && sizes[1] >= 44.0);
        assert_eq!(sizes[0], 44.0, "four slots away: untouched");
        assert!(sizes[3] > 44.0 * 1.3, "the neighbour is clearly grown: {}", sizes[3]);
    }

    #[test]
    fn a_symmetric_dock_with_the_pointer_in_the_middle_is_symmetric() {
        let (slots, rest, w) = icons_only(9);
        let centre = w / 2.0;
        let f = frame(&slots, &rest, w, centre, 1.0, Strength::Full, w + 200.0);
        for i in 0..4 {
            let (a, b) = (f.spans[i], f.spans[8 - i]);
            assert!((a.1 - b.1).abs() < 1e-3, "sizes {i}: {a:?} {b:?}");
        }
        let mid = f.left + f.width / 2.0;
        assert!((mid - w / 2.0).abs() < 1e-3, "the glass widens around the rest island's centre: {mid} vs {}", w / 2.0);
        for i in 0..4 {
            let (a, b) = (f.spans[i], f.spans[8 - i]);
            assert!(((a.0 + a.1 / 2.0 - mid) + (b.0 + b.1 / 2.0 - mid)).abs() < 1e-2, "item {i} and its mirror are not mirrored about the centre");
        }
    }

    #[test]
    fn mirrored_pointer_positions_give_mirrored_frames() {
        let (slots, rest, w) = icons_only(7);
        for p in [3.0f32, 40.0, 77.7, 130.0, 199.0] {
            let a = frame(&slots, &rest, w, p, 1.0, Strength::Full, w + 120.0);
            let b = frame(&slots, &rest, w, w - p, 1.0, Strength::Full, w + 120.0);
            assert!((a.width - b.width).abs() < 1e-3 && (a.left - b.left).abs() < 1e-3);
            for i in 0..7 {
                assert!((a.spans[i].1 - b.spans[6 - i].1).abs() < 1e-3, "pointer {p}, icon {i}");
                let mirrored_x = w - (b.spans[6 - i].0 + b.spans[6 - i].1);
                assert!((a.spans[i].0 - mirrored_x).abs() < 1e-2, "pointer {p}, icon {i}: {} vs {mirrored_x}", a.spans[i].0);
            }
        }
    }

    #[test]
    fn nothing_overlaps_and_the_row_adds_up() {
        for (slots, rest, w) in [icons_only(1), icons_only(2), icons_only(12), row(5, 2), row(0, 2)] {
            let mut p = -80.0;
            while p < w + 80.0 {
                for room in [0.0f32, 30.0, 80.0, 400.0] {
                    let f = frame(&slots, &rest, w, p, 1.0, Strength::Full, w + room);
                    assert_eq!(f.spans.len(), slots.len());
                    assert!(f.width >= w - 1e-3 && f.width <= w + room + 1e-3, "width {} (rest {w}, room {room})", f.width);
                    // padding on both sides, nothing outside the glass
                    assert!((f.spans[0].0 - (f.left + DOCK_PADDING)).abs() < 1e-3);
                    let (lx, lw) = *f.spans.last().unwrap();
                    assert!(lx + lw <= f.left + f.width - DOCK_PADDING + 1e-3, "last item ends at {} of {}", lx + lw, f.left + f.width);
                    for pair in f.spans.windows(2) {
                        assert!(pair[1].0 - (pair[0].0 + pair[0].1) >= MIN_GAP - 1e-3, "overlap or too tight: {pair:?}");
                    }
                    // with room enough the row is exactly as wide as its items need
                    if room >= 400.0 {
                        let sizes: Vec<f32> = f.spans.iter().zip(&slots).filter(|(_, s)| **s == Slot::Icon).map(|(s, _)| s.1).collect();
                        assert!((f.width - natural_width(&slots, &sizes)).abs() < 1e-2);
                    }
                }
                p += 7.3;
            }
        }
    }

    #[test]
    fn the_pointer_moving_a_little_changes_everything_a_little() {
        let (slots, rest, w) = row(6, 3);
        let room = 120.0;
        let mut prev = frame(&slots, &rest, w, -20.0, 1.0, Strength::Full, w + room);
        let mut p = -20.0f32;
        let step = 0.5;
        while p < w + 20.0 {
            p += step;
            let f = frame(&slots, &rest, w, p, 1.0, Strength::Full, w + room);
            // sizes follow the bell (slope at most about 0.3 px per px), positions add the sizes of the items before
            assert!((f.width - prev.width).abs() <= 1.0 * step + 1e-3, "width jumps at {p}: {} -> {}", prev.width, f.width);
            for (a, b) in prev.spans.iter().zip(&f.spans) {
                assert!((a.1 - b.1).abs() <= 0.4 * step + 1e-3, "size jumps at {p}: {a:?} -> {b:?}");
                assert!((a.0 - b.0).abs() <= 1.5 * step + 1e-3, "position jumps at {p}: {a:?} -> {b:?}");
            }
            prev = f;
        }
    }

    #[test]
    fn at_rest_the_frame_is_the_rest_layout_exactly() {
        let (slots, rest, w) = row(4, 2);
        for p in [-50.0f32, 0.0, 100.0, 1000.0] {
            for f in [frame(&slots, &rest, w, p, 0.0, Strength::Full, w + 100.0), frame(&slots, &rest, w, p, 1.0, Strength::Off, w + 100.0)] {
                assert_eq!(f.left, 0.0);
                assert!((f.width - w).abs() < 1e-3);
                for (a, b) in f.spans.iter().zip(&rest) {
                    assert!((a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3, "{a:?} vs {b:?}");
                }
            }
        }
    }

    #[test]
    fn the_edges_of_the_island() {
        let (slots, rest, w) = icons_only(6);
        // the pointer on the first icon: it grows, the glass widens to both sides and the first icon stays inside it
        let on_first = frame(&slots, &rest, w, rest[0].0 + 22.0, 1.0, Strength::Full, w + 100.0);
        assert!(on_first.spans[0].1 > 60.0);
        assert!(on_first.left < 0.0 && on_first.width > w);
        assert!(on_first.spans[0].0 >= on_first.left + DOCK_PADDING - 1e-3);
        // the same at the right end
        let on_last = frame(&slots, &rest, w, rest[5].0 + 22.0, 1.0, Strength::Full, w + 100.0);
        assert!(on_last.spans[5].1 > 60.0);
        let (x, ww) = on_last.spans[5];
        assert!(x + ww <= on_last.left + on_last.width - DOCK_PADDING + 1e-3);
        // a pointer outside the island on either side still gives a valid, partly grown dock
        let before = frame(&slots, &rest, w, -30.0, 1.0, Strength::Full, w + 100.0);
        assert!(before.spans[0].1 > 44.0 && before.spans[5].1 == 44.0);
        let after = frame(&slots, &rest, w, w + 30.0, 1.0, Strength::Full, w + 100.0);
        assert!(after.spans[5].1 > 44.0 && after.spans[0].1 == 44.0);
        // far away: nothing grows
        let far = frame(&slots, &rest, w, 5000.0, 1.0, Strength::Full, w + 100.0);
        assert!(far.spans.iter().all(|s| s.1 == 44.0) && far.width == w);
    }

    #[test]
    fn no_items_and_one_item() {
        let f = frame(&[], &[], 24.0, 10.0, 1.0, Strength::Full, 200.0);
        assert!(f.spans.is_empty() && f.width == 24.0 && f.left == 0.0);
        let (slots, rest, w) = icons_only(1);
        let f = frame(&slots, &rest, w, 34.0, 1.0, Strength::Full, w + 60.0);
        assert_eq!(f.spans.len(), 1);
        assert!((f.spans[0].1 - 44.0 * 1.55).abs() < 1e-3);
        assert!((f.width - (2.0 * DOCK_PADDING + 44.0 * 1.55)).abs() < 1e-3);
        assert!((f.left + f.width / 2.0 - w / 2.0).abs() < 1e-3, "centred");
        // a lone separator, a mismatch between slots and rest: the rest layout, no panic
        let f = frame(&[Slot::Separator], &[(12.0, 1.0)], 25.0, 0.0, 1.0, Strength::Full, 100.0);
        assert_eq!(f.spans, vec![(12.0, 1.0)]);
        let f = frame(&[Slot::Icon, Slot::Icon], &[(12.0, 44.0)], 70.0, 0.0, 1.0, Strength::Full, 100.0);
        assert_eq!(f.spans.len(), 1);
    }

    #[test]
    fn a_dock_with_no_room_gives_way_through_the_gaps_and_then_the_icons() {
        let (slots, rest, w) = icons_only(8);
        let f = frame(&slots, &rest, w, w / 2.0, 1.0, Strength::Full, w); // no room at all
        assert_eq!(f.width, w);
        assert_eq!(f.left, 0.0);
        let (x, ww) = *f.spans.last().unwrap();
        assert!(x + ww <= w - DOCK_PADDING + 1e-3);
        for pair in f.spans.windows(2) {
            assert!(pair[1].0 >= pair[0].0 + pair[0].1 + MIN_GAP - 1e-3);
        }
        // but the icon under the pointer is still the biggest
        let sizes: Vec<f32> = f.spans.iter().map(|s| s.1).collect();
        let max = sizes.iter().cloned().fold(0.0, f32::max);
        assert!(sizes[3] == max || sizes[4] == max);
    }

    #[test]
    fn a_squeezed_rest_layout_widens_from_its_own_width_not_with_a_jump() {
        // 14 icons in an island 100 px narrower than the usual gaps need: the rest layout has tight gaps (and perhaps small icons)
        let slots = vec![Slot::Icon; 14];
        let usual = natural_width(&slots, &[ICON_REST; 14]);
        let rest_width = usual - 100.0;
        let rest = fit(&slots, &[ICON_REST; 14], rest_width);
        let room = 60.0;
        // presence 0: exactly the rest layout; a hair of presence: a hair of widening, never a step
        let at0 = frame(&slots, &rest, rest_width, rest_width / 2.0, 0.0, Strength::Full, rest_width + room);
        assert_eq!((at0.width, at0.left), (rest_width, 0.0));
        let mut prev = at0.width;
        for step in 1..=100 {
            let presence = step as f32 / 100.0;
            let f = frame(&slots, &rest, rest_width, rest_width / 2.0, presence, Strength::Full, rest_width + room);
            assert!(f.width >= prev - 1e-3 && f.width - prev < 1.5, "the glass jumps at presence {presence}: {prev} -> {}", f.width);
            prev = f.width;
        }
        assert!(prev > rest_width + 20.0, "and it does widen: {prev}");
        let f = frame(&slots, &rest, rest_width, rest_width / 2.0, 0.01, Strength::Full, rest_width + room);
        assert!(f.width - rest_width < 1.0, "a hair of presence widens the glass by a hair: {}", f.width - rest_width);
    }

    #[test]
    fn the_widening_is_bounded_by_the_least_room() {
        assert_eq!(widening_room(40.0, 100.0, 100.0), 80.0);
        assert_eq!(widening_room(40.0, 10.0, 100.0), 20.0);
        assert_eq!(widening_room(40.0, 100.0, 0.0), 0.0);
        assert_eq!(widening_room(40.0, -5.0, 100.0), 0.0, "never negative");
    }

    #[test]
    fn the_widest_the_glass_ever_gets_fits_the_reserve() {
        // many icons, the pointer anywhere: the extra width the bell can cause is at most the sum of the grown sizes; the window
        // reserves `layout::MIDDLE_RESERVE` per side, which must hold it
        let (slots, rest, w) = icons_only(30);
        let mut worst: f32 = 0.0;
        let mut p = 0.0;
        while p < w {
            let f = frame(&slots, &rest, w, p, 1.0, Strength::Full, w + 1000.0);
            worst = worst.max(f.width - w);
            p += 3.1;
        }
        assert!(worst / 2.0 <= super::super::layout::MIDDLE_RESERVE, "each side needs {} px, the window keeps {}", worst / 2.0, super::super::layout::MIDDLE_RESERVE);
    }

    #[test]
    fn presence_eases_in_and_out_and_never_jumps() {
        let mut p = Presence::new(FADE_MS);
        assert!(p.at_rest(0) && p.settled(0) && !p.is_in());
        assert_eq!(p.value(0), 0.0);
        p.enter(1000);
        assert!(p.is_in() && !p.settled(1050) && !p.at_rest(1050));
        assert_eq!(p.value(1000), 0.0);
        let half = p.value(1080);
        assert!((half - 0.5).abs() < 1e-3, "smoothstep is 0.5 half way: {half}");
        assert!(p.value(1020) < 0.1 && p.value(1140) > 0.9, "slow at both ends");
        assert_eq!(p.value(1160), 1.0);
        assert!(p.settled(1160));
        // the same call again restarts nothing
        p.enter(1200);
        assert_eq!(p.value(1200), 1.0);
        // leaving in mid flight comes back from where the height was
        p.leave(2000);
        let mid = p.value(2080);
        p.enter(2080);
        assert!((p.value(2080) - mid).abs() < 1e-6, "no jump when the direction changes");
        // monotonic, within 0..1
        let mut q = Presence::new(FADE_MS);
        q.enter(0);
        let mut last = 0.0;
        for t in 0..=160 {
            let v = q.value(t);
            assert!((0.0..=1.0).contains(&v) && v >= last);
            last = v;
        }
        q.leave(500);
        assert!(q.at_rest(700) && q.value(700) == 0.0);
    }

    #[test]
    fn with_animations_off_the_bell_jumps() {
        let mut p = Presence::new(0);
        p.enter(10);
        assert_eq!(p.value(10), 1.0);
        assert!(p.settled(10));
        p.leave(20);
        assert_eq!(p.value(20), 0.0);
        assert!(p.at_rest(20));
    }

    #[test]
    fn fit_at_rest_is_the_slot_spans() {
        let slots = vec![Slot::Icon, Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon];
        let sizes = vec![ICON_REST; 5];
        let w = natural_width(&slots, &sizes);
        let a = fit(&slots, &sizes, w);
        let b = slot_spans(&slots, &sizes);
        for (x, y) in a.iter().zip(&b) {
            assert!((x.0 - y.0).abs() < 1e-3 && (x.1 - y.1).abs() < 1e-3);
        }
        assert!(fit(&[], &[], 100.0).is_empty());
    }
}
