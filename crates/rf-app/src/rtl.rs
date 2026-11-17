use std::ops::Range;

use gpui_kit::{Context, Entity, KeyDownEvent, component::input::InputState};
use unicode_bidi::{BidiClass, BidiInfo, Level, bidi_class};
use unicode_segmentation::UnicodeSegmentation;

pub fn display_text(text: &str) -> String {
    if text.starts_with('\u{2067}') && text.ends_with('\u{2069}') {
        return text.to_owned();
    }
    if first_strong_is_rtl(text) {
        format!("\u{2067}{text}\u{2069}")
    } else {
        text.to_owned()
    }
}

pub fn handle_key_down<T: 'static>(
    state: &Entity<InputState>,
    ev: &KeyDownEvent,
    cx: &mut Context<T>,
) {
    let modifiers = &ev.keystroke.modifiers;
    if modifiers.control || modifiers.alt || modifiers.platform {
        return;
    }

    let key = match ev.keystroke.key.as_str() {
        "left" => VisualDirection::Left,
        "right" => VisualDirection::Right,
        _ => return,
    };
    let text = state.read(cx).value();
    let Some((range, reversed)) = move_selection(
        &text,
        state.read(cx).selected_range(),
        state.read(cx).cursor(),
        state.read(cx).is_selection_reversed(),
        key,
        modifiers.shift,
    ) else {
        return;
    };

    state.update(cx, |input, cx| {
        input.set_selected_range_with_direction(range, reversed, cx)
    });
    cx.stop_propagation();
}

#[derive(Clone, Copy)]
enum VisualDirection {
    Left,
    Right,
}

fn first_strong_is_rtl(text: &str) -> bool {
    text.chars()
        .find_map(|c| match bidi_class(c) {
            BidiClass::R | BidiClass::AL => Some(true),
            BidiClass::L => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

fn has_rtl(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(bidi_class(c), BidiClass::R | BidiClass::AL))
}

fn visual_boundaries(text: &str) -> Vec<usize> {
    let base_level = if first_strong_is_rtl(text) {
        Level::rtl()
    } else {
        Level::ltr()
    };
    let bidi = BidiInfo::new(text, Some(base_level));
    let Some(paragraph) = bidi.paragraphs.first() else {
        return vec![0];
    };
    let levels = bidi.reordered_levels(paragraph, paragraph.range.clone());
    let graphemes: Vec<(usize, usize, Level)> = text
        .grapheme_indices(true)
        .map(|(start, grapheme)| (start, start + grapheme.len(), levels[start]))
        .collect();
    let levels: Vec<Level> = graphemes.iter().map(|(_, _, level)| *level).collect();
    let order = BidiInfo::reorder_visual(&levels);

    let mut boundaries = Vec::with_capacity(graphemes.len() + 1);
    for index in order {
        let (start, end, level) = graphemes[index];
        let pair = if level.is_rtl() {
            [end, start]
        } else {
            [start, end]
        };
        for boundary in pair {
            if boundaries.last() != Some(&boundary) {
                boundaries.push(boundary);
            }
        }
    }
    boundaries
}

fn move_selection(
    text: &str,
    range: Range<usize>,
    cursor: usize,
    reversed: bool,
    direction: VisualDirection,
    extend: bool,
) -> Option<(Range<usize>, bool)> {
    if !has_rtl(text) {
        return None;
    }

    let boundaries = visual_boundaries(text);
    let step = match direction {
        VisualDirection::Left => -1isize,
        VisualDirection::Right => 1,
    };

    if !extend && !range.is_empty() {
        let edge = boundaries
            .iter()
            .enumerate()
            .filter(|(_, offset)| range.start <= **offset && **offset <= range.end)
            .map(|(position, offset)| (position, *offset));
        let edge = match step {
            -1 => edge.min_by_key(|(position, _)| *position),
            _ => edge.max_by_key(|(position, _)| *position),
        }
        .map(|(_, offset)| offset)
        .unwrap_or(cursor);
        return Some((edge..edge, false));
    }

    let positions: Vec<usize> = boundaries
        .iter()
        .enumerate()
        .filter_map(|(i, offset)| (*offset == cursor).then_some(i))
        .collect();
    let position = if step < 0 {
        positions.iter().rev().copied().find(|i| *i > 0)
    } else {
        positions
            .iter()
            .copied()
            .find(|i| *i + 1 < boundaries.len())
    };
    let next = position
        .and_then(|i| i.checked_add_signed(step))
        .map(|i| boundaries[i])
        .unwrap_or(cursor);

    let anchor = if extend {
        if range.is_empty() {
            cursor
        } else if reversed {
            range.end
        } else {
            range.start
        }
    } else {
        next
    };
    let (start, end) = if extend {
        (anchor.min(next), anchor.max(next))
    } else {
        (next, next)
    };
    Some((start..end, extend && next < anchor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolates_only_rtl_first_strong_display_text() {
        assert_eq!(display_text("سلام"), "\u{2067}سلام\u{2069}");
        assert_eq!(display_text("\u{2067}سلام\u{2069}"), "\u{2067}سلام\u{2069}");
        assert_eq!(display_text("Folder سلام"), "Folder سلام");
        assert_eq!(display_text("123 سلام"), "\u{2067}123 سلام\u{2069}");
    }

    #[test]
    fn horizontal_caret_moves_in_visual_order_for_rtl_and_mixed_text() {
        let arabic = "ابت";
        assert_eq!(visual_boundaries(arabic), vec![arabic.len(), 4, 2, 0]);
        assert_eq!(
            move_selection(
                arabic,
                arabic.len()..arabic.len(),
                arabic.len(),
                false,
                VisualDirection::Right,
                false
            ),
            Some((4..4, false))
        );

        let mixed = "a אב";
        let boundaries = visual_boundaries(mixed);
        assert_eq!(boundaries.first(), Some(&0));
        assert!(
            boundaries
                .iter()
                .all(|offset| mixed.is_char_boundary(*offset))
        );
        assert!(move_selection(mixed, 0..0, 0, false, VisualDirection::Right, false).is_some());

        let combining = "שָ";
        assert_eq!(visual_boundaries(combining), vec![combining.len(), 0]);
    }

    #[test]
    fn shift_and_selection_keep_the_active_caret_endpoint() {
        let text = "אב";
        let (range, reversed) =
            move_selection(text, 0..0, 0, false, VisualDirection::Left, true).unwrap();
        assert_eq!(range, 0..2);
        assert!(!reversed);

        let (collapsed, reversed) =
            move_selection(text, 0..2, 2, false, VisualDirection::Left, false).unwrap();
        assert_eq!(collapsed, 2..2);
        assert!(!reversed);
    }
}
