//! Where the keys go in the Start panel when its search field is at the BOTTOM of the glass (the panel has grown out of the left
//! island: the field stands in the island's row, under the content). Pure: the view (`rf-app`'s `dock/start.rs`) asks and obeys.
//!
//! WHY THIS EXISTS. With the field at the top, "Down from the field" enters the content at its first thing and "Up" has nowhere to go.
//! With the field at the bottom the content is ABOVE it, so the key that points at the content is Up, and the way back to the field is
//! Down from the content's bottom edge. Everything else (Left and Right belong to the text field unless the cursor is in the grid,
//! Enter starts what the cursor or the selection is on) is as it was.
//!
//! * HOME PAGE ([`home_step`]). From the field (no cursor): Up and Page Up land on the thing NEAREST to the field, the first tile of the
//!   last row that is drawn (the recommended entries when there are any, else the grid's bottom row); Down, Right and Page Down still
//!   land on the first thing there is (so a key that used to work keeps working). Down on the content's bottom row, where it used to
//!   stay put, leaves the cursor: the keyboard is back in the field. Every other move is the content's own (`startpage::step_home`).
//! * ALL APPS PAGE ([`list_step`]). Up and Page Up from the field land on the app on the last row that is fully in view (the list is
//!   scrolled; the view measures the row, `startlist::last_visible_app`, and passes it in), the row nearest to the field, as on the home
//!   page; Down from the field lands on the first app, as before. Down on the last app returns to the field.
//! * RESULTS. The selection is always on a row (the best match is the first row and is what Enter starts); Up and Down move it and wrap,
//!   so there is no step "to the field" to route: the arrows never leave the list, Left, Right, Home and End stay the field's.
//!   [`results_step`] is `startlist::step_results` under the name the rule has.
//! * TAB ([`tab_stop`]). Tab visits the stops in the order they are on the screen: with the field at the bottom the header's gear and
//!   power button come first, then "Tümü" or "Geri" in the content, then the field; with the field on top, as it was: the field, "Tümü",
//!   the gear, the power button.

use super::startlist::{self, Cursor, Move};
use super::startpage::{self, COLS, REC_COLS, Spot, TabStop};

/// Where the panel's search field is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldAt {
    /// At the top of the panel (the panel's own place; the panel is not grown out of its island).
    Top,
    /// In the island's row at the bottom of the grown glass.
    Bottom,
}

/// Where the search field is, from whether it stands in the island's row (`grow_host::field_in_row`).
pub fn field_at(in_row: bool) -> FieldAt {
    if in_row { FieldAt::Bottom } else { FieldAt::Top }
}

/// The first tile of the last drawn row of the home page's content: what is nearest to a field below it. `None`: nothing to land on.
fn nearest_to_a_field_below(page: usize, per_page: usize, pins: usize, recs: usize) -> Option<Spot> {
    if recs > 0 {
        return Some(Spot::Rec((recs - 1) / REC_COLS * REC_COLS));
    }
    if pins == 0 {
        return None;
    }
    let per_page = per_page.max(COLS);
    let range = startpage::page_range(page, per_page, pins);
    (!range.is_empty()).then(|| Spot::Pin(range.start + (range.len() - 1) / COLS * COLS))
}

/// The home page's cursor after an arrow or page key, for a field at `field`. The arguments are `startpage::step_home`'s. `None` is
/// "the keyboard is in the field".
pub fn home_step(field: FieldAt, cur: Option<Spot>, mv: Move, page: usize, per_page: usize, pins: usize, recs: usize) -> Option<Spot> {
    let next = startpage::step_home(cur, mv, page, per_page, pins, recs);
    if field == FieldAt::Top {
        return next;
    }
    match (cur, mv) {
        // from the field, towards the content that lies above it
        (None, Move::Up | Move::PageUp) => nearest_to_a_field_below(page, per_page, pins, recs),
        // down on the bottom row: nothing below it but the field
        (Some(c), Move::Down) if next == Some(c) => None,
        _ => next,
    }
}

/// The all-apps page's cursor after an arrow or page key, for a field at `field`. `apps` is the number of apps in the list;
/// `nearest` is the app on the last row that is fully in view (`startlist::last_visible_app`), what is nearest to a field below the
/// list. With the field at the bottom, Up and Page Up from the field land on it (as on the home page they land on the row nearest to
/// the field); Down and Right still land on the first app, so a key that used to work keeps working.
pub fn list_step(field: FieldAt, cur: Option<Cursor>, mv: Move, apps: usize, nearest: Option<usize>) -> Option<Cursor> {
    let next = startlist::step(cur, mv, 0, apps);
    match (field, cur, mv) {
        (FieldAt::Bottom, Some(c), Move::Down) if next == Some(c) => None,
        (FieldAt::Bottom, None, Move::Up | Move::PageUp) => nearest.filter(|&j| j < apps).map(Cursor::App),
        _ => next,
    }
}

/// The result list's selection after an up or down arrow: wraps at both ends, whatever the field's place.
pub fn results_step(selected: usize, up: bool, n: usize) -> usize {
    startlist::step_results(selected, up, n)
}

/// The stop Tab (or Shift+Tab with `back`) goes to from `at`, for a field at `field`. The page button is a stop only while it is drawn
/// (see `startpage::next_tab_stop`).
pub fn tab_stop(field: FieldAt, at: TabStop, back: bool, page_button_shown: bool) -> TabStop {
    if field == FieldAt::Top {
        return startpage::next_tab_stop(at, back, page_button_shown);
    }
    // the order on the screen, top to bottom: the header's gear and power button, the content's page button, the field
    let stops: &[TabStop] = if page_button_shown { &[TabStop::Gear, TabStop::Power, TabStop::PageButton, TabStop::Field] } else { &[TabStop::Gear, TabStop::Power, TabStop::Field] };
    // a stop that is not there any more (the page button went away while it had the focus) counts as the field
    let i = stops.iter().position(|s| *s == at).unwrap_or(stops.len() - 1);
    stops[if back { (i + stops.len() - 1) % stops.len() } else { (i + 1) % stops.len() }]
}

#[cfg(test)]
mod tests {
    use super::*;

    const PER_PAGE: usize = 18;

    #[test]
    fn with_the_field_on_top_nothing_changes() {
        // every move from every place gives what the content's own rule gives
        let spots = [None, Some(Spot::Pin(0)), Some(Spot::Pin(7)), Some(Spot::Pin(17)), Some(Spot::Rec(0)), Some(Spot::Rec(5))];
        for cur in spots {
            for mv in [Move::Left, Move::Right, Move::Up, Move::Down, Move::PageUp, Move::PageDown] {
                assert_eq!(home_step(FieldAt::Top, cur, mv, 0, PER_PAGE, 30, 6), startpage::step_home(cur, mv, 0, PER_PAGE, 30, 6), "{cur:?} {mv:?}");
            }
        }
        for cur in [None, Some(Cursor::App(0)), Some(Cursor::App(9))] {
            for mv in [Move::Up, Move::Down, Move::PageUp, Move::PageDown] {
                assert_eq!(list_step(FieldAt::Top, cur, mv, 10, Some(4)), startlist::step(cur, mv, 0, 10));
            }
        }
        // Tab: the field, the page button, the gear, the power button
        let mut at = TabStop::Field;
        let mut seen = Vec::new();
        for _ in 0..4 {
            at = tab_stop(FieldAt::Top, at, false, true);
            seen.push(at);
        }
        assert_eq!(seen, [TabStop::PageButton, TabStop::Gear, TabStop::Power, TabStop::Field]);
    }

    #[test]
    fn up_from_a_field_below_lands_on_the_thing_nearest_to_it() {
        // recommended entries: the first tile of their last row (six entries, two columns: row 2 starts at 4)
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Up, 0, PER_PAGE, 30, 6), Some(Spot::Rec(4)));
        assert_eq!(home_step(FieldAt::Bottom, None, Move::PageUp, 0, PER_PAGE, 30, 5), Some(Spot::Rec(4)), "five entries: the last row has one");
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Up, 0, PER_PAGE, 30, 1), Some(Spot::Rec(0)));
        // no recommended entries: the grid's bottom row of the page that is shown (three rows of six)
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Up, 0, PER_PAGE, 30, 0), Some(Spot::Pin(12)));
        // (the second page holds pins 18..30: two rows, the last starts at 24)
        assert_eq!(startpage::page_range(1, PER_PAGE, 30), 18..30);
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Up, 1, PER_PAGE, 30, 0), Some(Spot::Pin(24)));
        // a short last page, and one with a partial last row (eight pins: the last row starts at 6)
        assert_eq!(nearest_to_a_field_below(1, PER_PAGE, 20, 0), Some(Spot::Pin(18)));
        assert_eq!(nearest_to_a_field_below(0, PER_PAGE, 8, 0), Some(Spot::Pin(6)));
        // nothing drawn: nowhere to land
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Up, 0, PER_PAGE, 0, 0), None);
        // Left from the field is the text field's
        assert_eq!(home_step(FieldAt::Bottom, None, Move::Left, 0, PER_PAGE, 30, 6), None);
    }

    #[test]
    fn the_keys_that_used_to_enter_the_content_still_do() {
        for mv in [Move::Down, Move::Right, Move::PageDown] {
            assert_eq!(home_step(FieldAt::Bottom, None, mv, 0, PER_PAGE, 30, 6), Some(Spot::Pin(0)), "{mv:?}");
            assert_eq!(home_step(FieldAt::Bottom, None, mv, 0, PER_PAGE, 0, 6), Some(Spot::Rec(0)), "{mv:?}");
        }
    }

    #[test]
    fn down_on_the_bottom_row_goes_back_to_the_field_and_anywhere_else_it_moves_on() {
        // with recommended entries the bottom row is theirs (six entries: rows at 0, 2, 4)
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Rec(4)), Move::Down, 0, PER_PAGE, 30, 6), None);
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Rec(5)), Move::Down, 0, PER_PAGE, 30, 6), None);
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Rec(2)), Move::Down, 0, PER_PAGE, 30, 6), Some(Spot::Rec(4)));
        // the grid's bottom row, with recommended entries below it: on to them (under the grid's left half the left column)
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(14)), Move::Down, 0, PER_PAGE, 30, 6), Some(Spot::Rec(0)));
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(16)), Move::Down, 0, PER_PAGE, 30, 6), Some(Spot::Rec(1)));
        // the grid alone: its bottom row is the content's
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(14)), Move::Down, 0, PER_PAGE, 30, 0), None);
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(2)), Move::Down, 0, PER_PAGE, 30, 0), Some(Spot::Pin(8)));
        // a partial last row (eight pins: six and two): the tile above it moves down to its last tile first, then the field
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(2)), Move::Down, 0, PER_PAGE, 8, 0), Some(Spot::Pin(7)));
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(7)), Move::Down, 0, PER_PAGE, 8, 0), None);
        // up never leaves the content (the header is not a stop of the arrows)
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(1)), Move::Up, 0, PER_PAGE, 30, 6), Some(Spot::Pin(1)));
        // pages and sideways moves are the content's own
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(1)), Move::PageDown, 0, PER_PAGE, 30, 6), Some(Spot::Pin(19)));
        assert_eq!(home_step(FieldAt::Bottom, Some(Spot::Pin(1)), Move::Right, 0, PER_PAGE, 30, 6), Some(Spot::Pin(2)));
    }

    #[test]
    fn the_all_apps_list_returns_to_the_field_from_its_last_app_and_up_enters_at_the_row_nearest_to_it() {
        assert_eq!(list_step(FieldAt::Bottom, None, Move::Down, 5, Some(3)), Some(Cursor::App(0)));
        // Up from the field: the app on the last row in view (the view measured it), not nothing
        assert_eq!(list_step(FieldAt::Bottom, None, Move::Up, 5, Some(3)), Some(Cursor::App(3)));
        assert_eq!(list_step(FieldAt::Bottom, None, Move::PageUp, 5, Some(3)), Some(Cursor::App(3)));
        // a list that has not been laid out yet (nothing measured), or an app that is gone: nowhere to land
        assert_eq!(list_step(FieldAt::Bottom, None, Move::Up, 5, None), None);
        assert_eq!(list_step(FieldAt::Bottom, None, Move::Up, 3, Some(3)), None);
        // with the field on top Up from the field has nowhere to go whatever was measured
        assert_eq!(list_step(FieldAt::Top, None, Move::Up, 5, Some(3)), None);
        assert_eq!(list_step(FieldAt::Bottom, Some(Cursor::App(2)), Move::Down, 5, Some(3)), Some(Cursor::App(3)));
        assert_eq!(list_step(FieldAt::Bottom, Some(Cursor::App(4)), Move::Down, 5, Some(3)), None);
        assert_eq!(list_step(FieldAt::Bottom, Some(Cursor::App(0)), Move::Up, 5, Some(3)), Some(Cursor::App(0)));
        // a page jump that ends on the last app does not leave the list
        assert_eq!(list_step(FieldAt::Bottom, Some(Cursor::App(0)), Move::PageDown, 5, Some(3)), Some(Cursor::App(4)));
        // and an empty list has nowhere to go either way
        assert_eq!(list_step(FieldAt::Bottom, None, Move::Down, 0, None), None);
    }

    #[test]
    fn the_field_is_below_exactly_when_it_stands_in_the_islands_row() {
        assert_eq!(field_at(true), FieldAt::Bottom);
        assert_eq!(field_at(false), FieldAt::Top);
    }

    #[test]
    fn the_results_selection_wraps_and_never_leaves_the_list() {
        // the best match is the first row; up from it wraps to the last, down from the last to the first
        assert_eq!(results_step(0, true, 5), 4);
        assert_eq!(results_step(4, false, 5), 0);
        assert_eq!(results_step(1, false, 5), 2);
        assert_eq!(results_step(0, false, 0), 0);
    }

    #[test]
    fn tab_follows_the_order_on_the_screen_when_the_field_is_below() {
        let walk = |start: TabStop, back: bool, button: bool, n: usize| {
            let mut at = start;
            (0..n).map(|_| {
                at = tab_stop(FieldAt::Bottom, at, back, button);
                at
            }).collect::<Vec<_>>()
        };
        assert_eq!(walk(TabStop::Field, false, true, 4), [TabStop::Gear, TabStop::Power, TabStop::PageButton, TabStop::Field]);
        assert_eq!(walk(TabStop::Field, true, true, 4), [TabStop::PageButton, TabStop::Power, TabStop::Gear, TabStop::Field]);
        // no page button over a result list: gear, power, field
        assert_eq!(walk(TabStop::Field, false, false, 3), [TabStop::Gear, TabStop::Power, TabStop::Field]);
        // a page button that went away while it had the focus counts as the field
        assert_eq!(tab_stop(FieldAt::Bottom, TabStop::PageButton, false, false), TabStop::Gear);
    }
}
