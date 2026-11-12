//! The Mac-like grow of the dock's icons: the one under the pointer goes from 44 to 56 px, its two neighbours to 50 px, eased over
//! about 120 ms. Pure: the pointer and the clock are arguments, so the animation can be tested at any moment.

use super::layout::{DOCK_PADDING, ICON_GAP, ICON_REST, SEPARATOR_WIDTH};

/// Size of the icon under the pointer, logical px.
pub const GROWN: f32 = 56.0;
/// Size of its two neighbours.
pub const NEIGHBOUR: f32 = 50.0;
/// How long the grow takes.
pub const DURATION_MS: u32 = 120;

/// What the dock holds, left to right: icons (apps and places) and thin separators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Icon,
    Separator,
}

/// The size each icon is heading for when icon `hovered` (a count of icons, separators not counted) is under the pointer. A
/// neighbour is the next icon on either side, also across a separator.
pub fn target_sizes(icons: usize, hovered: Option<usize>) -> Vec<f32> {
    (0..icons)
        .map(|i| match hovered {
            Some(h) if h == i => GROWN,
            Some(h) if h + 1 == i || i + 1 == h => NEIGHBOUR,
            _ => ICON_REST,
        })
        .collect()
}

/// Ease-out cubic, 0 -> 1 for `t` 0 -> 1 (clamped).
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// The value `elapsed_ms` after a change from `from` to `to`. A zero duration (the app's animations are off) is the end at once.
pub fn eased(from: f32, to: f32, elapsed_ms: f32, duration_ms: u32) -> f32 {
    if duration_ms == 0 || elapsed_ms >= duration_ms as f32 {
        return to;
    }
    from + (to - from) * ease_out(elapsed_ms / duration_ms as f32)
}

/// The sizes of all icons over time. Call [`HoverAnim::set_hover`] when the pointer moves onto another icon (or off the dock) and
/// [`HoverAnim::sizes`] for every frame; [`HoverAnim::settled`] says when frames are no longer needed.
///
/// A change of target in mid-flight starts from where each icon is at that moment, so moving along the dock never jumps.
#[derive(Clone, Debug)]
pub struct HoverAnim {
    from: Vec<f32>,
    to: Vec<f32>,
    started_ms: u64,
    duration_ms: u32,
    hovered: Option<usize>,
}

impl HoverAnim {
    /// `duration_ms` is [`DURATION_MS`], or 0 when the app's animation setting is off.
    pub fn new(icons: usize, duration_ms: u32) -> HoverAnim {
        let rest = vec![ICON_REST; icons];
        HoverAnim { from: rest.clone(), to: rest, started_ms: 0, duration_ms, hovered: None }
    }

    pub fn hovered(&self) -> Option<usize> {
        self.hovered
    }

    pub fn set_duration(&mut self, duration_ms: u32) {
        self.duration_ms = duration_ms;
    }

    /// The icons changed (an app started or ended): the count follows, the new ones start at rest, no hover.
    pub fn resize(&mut self, icons: usize, now_ms: u64) {
        let cur = self.sizes(now_ms);
        let mut from = cur;
        from.resize(icons, ICON_REST);
        self.from = from;
        self.to = vec![ICON_REST; icons];
        self.started_ms = now_ms;
        self.hovered = None;
    }

    /// The pointer is over icon `hovered` (`None`: not over any). Same icon again: nothing restarts.
    pub fn set_hover(&mut self, hovered: Option<usize>, now_ms: u64) {
        let hovered = hovered.filter(|&h| h < self.to.len());
        if hovered == self.hovered {
            return;
        }
        self.from = self.sizes(now_ms);
        self.to = target_sizes(self.to.len(), hovered);
        self.started_ms = now_ms;
        self.hovered = hovered;
    }

    pub fn sizes(&self, now_ms: u64) -> Vec<f32> {
        let elapsed = now_ms.saturating_sub(self.started_ms) as f32;
        self.from.iter().zip(&self.to).map(|(&a, &b)| eased(a, b, elapsed, self.duration_ms)).collect()
    }

    /// True once every icon has reached its target: no more frames are needed until the pointer moves.
    pub fn settled(&self, now_ms: u64) -> bool {
        self.duration_ms == 0 || self.from == self.to || now_ms.saturating_sub(self.started_ms) >= self.duration_ms as u64
    }
}

/// Where each item of the dock lies along its width: `(x, width)` in logical px from the island's left edge, for the given slots
/// and the icon sizes (one per [`Slot::Icon`], in order). The padding is included, so the first item starts at `DOCK_PADDING`.
pub fn slot_spans(slots: &[Slot], icon_sizes: &[f32]) -> Vec<(f32, f32)> {
    let mut x = DOCK_PADDING;
    let mut icon = 0usize;
    let mut out = Vec::with_capacity(slots.len());
    for (i, s) in slots.iter().enumerate() {
        if i > 0 {
            x += ICON_GAP;
        }
        let w = match s {
            Slot::Icon => {
                let w = icon_sizes.get(icon).copied().unwrap_or(ICON_REST);
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

/// Which icon (a count of icons, not of slots) is at `x`, logical px from the island's left edge. The gap between two icons belongs
/// to the nearer one so the grow does not flicker while the pointer crosses it; the padding at both ends belongs to the first and
/// the last icon; a separator belongs to nobody. `None` for no icons at all.
pub fn icon_at(x: f32, slots: &[Slot], icon_sizes: &[f32]) -> Option<usize> {
    icon_at_spans(x, slots, &slot_spans(slots, icon_sizes))
}

/// [`icon_at`] for items that lie where `spans` (`(x, width)` per slot) say, e.g. when the island is too narrow for the usual gaps.
pub fn icon_at_spans(x: f32, slots: &[Slot], spans: &[(f32, f32)]) -> Option<usize> {
    let mut icon = 0usize;
    // (icon index, left edge, right edge) of every icon
    let mut icons: Vec<(usize, f32, f32)> = Vec::new();
    for (s, &(sx, sw)) in slots.iter().zip(spans) {
        if *s == Slot::Icon {
            icons.push((icon, sx, sx + sw));
            icon += 1;
        }
    }
    let first = icons.first()?;
    let last = icons.last()?;
    if x < first.1 {
        return Some(first.0);
    }
    if x >= last.2 {
        return Some(last.0);
    }
    for (k, &(i, l, r)) in icons.iter().enumerate() {
        if x >= l && x < r {
            return Some(i);
        }
        // between this icon and the next one (a gap, maybe with a separator in it): the nearer one, but not the separator itself
        if let Some(&(j, nl, _)) = icons.get(k + 1)
            && x >= r
            && x < nl
        {
            let sep = spans.iter().zip(slots).any(|(&(sx, sw), s)| *s == Slot::Separator && x >= sx && x < sx + sw);
            if sep {
                return None;
            }
            return Some(if x - r < nl - x { i } else { j });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_grow_the_icon_and_its_two_neighbours() {
        assert_eq!(target_sizes(5, None), vec![44.0; 5]);
        assert_eq!(target_sizes(5, Some(2)), vec![44.0, 50.0, 56.0, 50.0, 44.0]);
        assert_eq!(target_sizes(5, Some(0)), vec![56.0, 50.0, 44.0, 44.0, 44.0]);
        assert_eq!(target_sizes(5, Some(4)), vec![44.0, 44.0, 44.0, 50.0, 56.0]);
        assert_eq!(target_sizes(0, Some(0)), Vec::<f32>::new());
    }

    #[test]
    fn easing_runs_from_start_to_end_and_never_overshoots() {
        assert_eq!(eased(44.0, 56.0, 0.0, 120), 44.0);
        assert_eq!(eased(44.0, 56.0, 120.0, 120), 56.0);
        assert_eq!(eased(44.0, 56.0, 500.0, 120), 56.0);
        let mid = eased(44.0, 56.0, 60.0, 120);
        assert!(mid > 50.0 && mid < 56.0, "ease-out is past half way at half the time: {mid}");
        let mut last = 44.0;
        for t in 0..=120 {
            let v = eased(44.0, 56.0, t as f32, 120);
            assert!(v >= last && v <= 56.0);
            last = v;
        }
        assert_eq!(eased(44.0, 56.0, 10.0, 0), 56.0, "animations off: at once");
        // shrinking works the same way
        assert_eq!(eased(56.0, 44.0, 120.0, 120), 44.0);
    }

    #[test]
    fn animation_follows_the_pointer_and_settles() {
        let mut a = HoverAnim::new(4, DURATION_MS);
        assert!(a.settled(0));
        a.set_hover(Some(1), 1000);
        assert!(!a.settled(1050));
        assert_eq!(a.sizes(1000), vec![44.0; 4]);
        let half = a.sizes(1060);
        assert!(half[1] > 50.0 && half[1] < 56.0);
        assert!(half[0] > 44.0 && half[0] < 50.0);
        assert_eq!(half[3], 44.0);
        assert_eq!(a.sizes(1120), vec![50.0, 56.0, 50.0, 44.0]);
        assert!(a.settled(1120));
        // moving to the next icon in mid flight starts from the current sizes
        a.set_hover(Some(2), 1200);
        a.set_hover(Some(3), 1230);
        let at_turn = a.sizes(1230);
        let before = {
            let mut b = HoverAnim::new(4, DURATION_MS);
            b.set_hover(Some(1), 1000);
            b.set_hover(Some(2), 1200);
            b.sizes(1230)
        };
        assert_eq!(at_turn, before, "no jump when the target changes in mid flight");
        // the pointer leaves
        a.set_hover(None, 2000);
        assert_eq!(a.sizes(2200), vec![44.0; 4]);
        assert_eq!(a.hovered(), None);
    }

    #[test]
    fn the_same_hover_does_not_restart_and_a_stale_index_is_ignored() {
        let mut a = HoverAnim::new(3, DURATION_MS);
        a.set_hover(Some(1), 100);
        let s = a.sizes(150);
        a.set_hover(Some(1), 140);
        assert_eq!(a.sizes(150), s);
        a.set_hover(Some(9), 300); // beyond the icons: same as off
        assert_eq!(a.hovered(), None);
    }

    #[test]
    fn animations_off_means_no_waiting() {
        let mut a = HoverAnim::new(3, 0);
        a.set_hover(Some(0), 10);
        assert_eq!(a.sizes(10), vec![56.0, 50.0, 44.0]);
        assert!(a.settled(10));
    }

    #[test]
    fn resize_keeps_old_sizes_and_drops_the_hover() {
        let mut a = HoverAnim::new(3, DURATION_MS);
        a.set_hover(Some(1), 0);
        a.resize(5, 120);
        assert_eq!(a.hovered(), None);
        let s = a.sizes(120);
        assert_eq!(s.len(), 5);
        assert_eq!(s[1], 56.0);
        assert_eq!(s[4], 44.0);
    }

    fn row() -> (Vec<Slot>, Vec<f32>) {
        // three apps, a separator, two places
        (vec![Slot::Icon, Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon], vec![44.0; 5])
    }

    #[test]
    fn spans_follow_padding_gaps_and_sizes() {
        let (slots, sizes) = row();
        let s = slot_spans(&slots, &sizes);
        assert_eq!(s[0], (12.0, 44.0));
        assert_eq!(s[1], (66.0, 44.0));
        assert_eq!(s[2], (120.0, 44.0));
        assert_eq!(s[3], (174.0, 1.0)); // 164 + 10
        assert_eq!(s[4], (185.0, 44.0));
        assert_eq!(s[5], (239.0, 44.0));
        // the island's width is the end of the last span plus the padding
        assert_eq!(s[5].0 + s[5].1 + 12.0, super::super::layout::middle_width(&sizes, 1));
        // grown sizes push the next items
        let g = slot_spans(&slots, &[44.0, 56.0, 44.0, 44.0, 44.0]);
        assert_eq!(g[2].0, 132.0);
    }

    #[test]
    fn hit_testing_picks_the_icon_under_the_pointer() {
        let (slots, sizes) = row();
        assert_eq!(icon_at(30.0, &slots, &sizes), Some(0));
        assert_eq!(icon_at(5.0, &slots, &sizes), Some(0), "the padding belongs to the first icon");
        assert_eq!(icon_at(80.0, &slots, &sizes), Some(1));
        assert_eq!(icon_at(500.0, &slots, &sizes), Some(4), "and the padding at the end to the last");
        // the gap between icon 0 (ends at 56) and icon 1 (starts at 66): the nearer one
        assert_eq!(icon_at(58.0, &slots, &sizes), Some(0));
        assert_eq!(icon_at(64.0, &slots, &sizes), Some(1));
        // the separator (174) lies between icon 2 (ends 164) and icon 3 (starts 185): the separator itself is nobody's
        assert_eq!(icon_at(174.5, &slots, &sizes), None);
        assert_eq!(icon_at(167.0, &slots, &sizes), Some(2));
        assert_eq!(icon_at(183.0, &slots, &sizes), Some(3));
        assert_eq!(icon_at(10.0, &[], &[]), None);
    }
}
