//! The middle island says it takes a drop: while an app is dragged out of the Start panel and the pointer is over the island, the
//! island's glass carries the dock's quiet highlight (the fill a button under the pointer has, `look::hover_fill`) and a hairline.
//!
//! The Start panel's drag (`start_drag.rs`) switches it on and off ([`set`]); the island draws it ([`overlay`], one call in its render).
//! It is a flag and nothing else: at rest nothing is drawn and no frame is asked for.

use gpui_kit::*;

use super::look::Palette;

/// Whether a drop is about to land on the island.
#[derive(Default)]
struct DropHint(bool);

impl Global for DropHint {}

/// Whether the island is showing that it takes the drop.
pub fn is_on(cx: &App) -> bool {
    cx.try_global::<DropHint>().is_some_and(|h| h.0)
}

/// Switches the highlight on or off. The island draws again only when the flag changed.
pub fn set(on: bool, cx: &mut App) {
    if is_on(cx) == on {
        return;
    }
    cx.set_global(DropHint(on));
    if let Some(island) = super::host::middle_island(cx) {
        island.update(cx, |_, cx| cx.notify());
    }
}

/// The highlight over the island's glass (`glass`: left, top, width, height in the island's window), `None` when no drop is coming.
/// Drawn under the icons.
pub fn overlay(cx: &App, pal: Palette, glass: [Pixels; 4], radius: Pixels) -> Option<Div> {
    is_on(cx).then(|| div().absolute().left(glass[0]).top(glass[1]).w(glass[2]).h(glass[3]).rounded(radius).bg(pal.hover_fill().hsla()).border_1().border_color(pal.separator().hsla()))
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's own `test` attribute would come with it)
    use super::DropHint;

    #[test]
    fn the_flag_is_off_until_a_drag_switches_it_on() {
        // (no app here: only the pure default can be looked at)
        assert!(!DropHint::default().0);
    }
}
