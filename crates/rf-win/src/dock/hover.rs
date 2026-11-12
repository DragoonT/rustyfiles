//! The dock's slots (icons and thin separators) and the hit test along it. The growth of the icons under the pointer is `magnify`.
//! Pure: everything is an argument.

use super::layout::{DOCK_PADDING, ICON_GAP, ICON_REST, SEPARATOR_WIDTH};

/// What the dock holds, left to right: icons (apps and places) and thin separators.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Icon,
    Separator,
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
