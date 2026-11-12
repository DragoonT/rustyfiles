//! What the views of the right island's panels (the control centre, the calendar, the notifications, the tray's overflow) draw while
//! the island's glass has grown into them: the same three things in each, so they are made here once. `grow_host::view_frame` says
//! where the glass is, how much of the panel's content is to be seen and where the island's row is; the view keeps drawing its own
//! body, exactly as it does for an ordinary panel, and hands it to [`body`].
//!
//! * [`wash`]: the panel's fill over its glass, in the glass's rectangle of the moment (the window is bigger than the glass while it
//!   grows). It comes in with the growth, see `Run::wash_alpha`. An ordinary panel paints its fill over the whole window instead.
//! * [`hairline`]: the thin line above the island's row, across the glass.
//! * [`body`]: the panel's own body in its own rectangle of the window (above the island's row and the gap), fading with the motion's
//!   second half. The body is sized to fill its parent, as it is in an ordinary panel's window.

use gpui_kit::*;

use super::grow_host::ViewFrame;
use super::look::Palette;

/// The panel's fill, over the glass of the moment. `fill` is `glass_watch::panel_fill(dark, g.plain)`.
pub fn wash(g: &ViewFrame, fill: Hsla) -> Div {
    div().absolute().left(px(g.wash[0])).top(px(g.wash[1])).w(px(g.wash[2])).h(px(g.wash[3])).rounded(px(g.wash_radius)).bg(fill.opacity(g.wash_alpha))
}

/// The line above the island's row, as wide as the glass is at this moment, as faint as the content is. `None` for a panel with a row
/// of its own.
pub fn hairline(g: &ViewFrame, pal: Palette) -> Option<Div> {
    g.line_y.map(|y| div().absolute().left(px(g.wash[0])).top(px(y)).w(px(g.wash[2])).h(px(rf_win::dock::startpage::DIVIDER_H)).bg(pal.separator().hsla()).opacity(g.content))
}

/// The panel's body in its rectangle of the window, with the content's opacity of the moment.
pub fn body(g: &ViewFrame, body: impl IntoElement) -> Div {
    div().absolute().left(px(g.area[0])).top(px(g.area[1])).w(px(g.area[2])).h(px(g.area[3])).opacity(g.content).child(body)
}
