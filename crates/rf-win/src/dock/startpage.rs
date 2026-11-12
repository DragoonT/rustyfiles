//! The Start panel's two pages and the arithmetic of the home page. Pure: numbers and decisions in, numbers and decisions out (the
//! view in `rf-app/src/dock/start.rs` puts them on the screen).
//!
//! * THE PAGES. The panel always opens on the HOME page (search, the pinned grid, "Önerilenler"). "Tümü" opens the ALL APPS page (the
//!   alphabetical list); "Geri", Escape and Backspace on an empty search lead back. Only from the home page does Escape close the panel.
//!   A typed query is a third state shown over either page: the result list. [`escape`] and [`backspace_goes_back`] say where a key leads.
//! * THE PINNED GRID. Six columns; three rows of tiles when "Önerilenler" is shown (the panel's height is then spent), up to five when
//!   it is not ("the grid gets its room"); more pins than a page holds are reached by PAGES with dots at the right edge of the grid
//!   (the mouse wheel, the dots, the keyboard), not by scrolling: a page never shows half a tile, the keyboard cursor needs no
//!   scroll-into-view arithmetic, and it is how Windows 11's Start does it. [`home_layout`] fits the rows and the recommended rows to the
//!   panel's height, [`step_home`] moves the keyboard cursor through the grid and the recommended entries, [`WheelPager`] turns the
//!   wheel's smooth stream of small steps into one page at a time.
//! * THE PANEL'S SIZE. 642 x 720 logical px at most; [`panel_height`] never lets it be taller than the room above the island.

use super::startlist::Move;

// ---------------------------------------------------------------- measures (logical px)

/// The panel's width.
pub const PANEL_W: f32 = 642.0;
/// The panel's height when the screen has the room for it.
pub const PANEL_MAX_H: f32 = 720.0;
/// Space left and right of everything.
pub const SIDE_PAD: f32 = 28.0;
/// Space above the search field.
pub const TOP_PAD: f32 = 24.0;
/// The search field.
pub const SEARCH_H: f32 = 36.0;
/// Its corner radius (a pill).
pub const SEARCH_RADIUS: f32 = 18.0;
/// Between the search field and the first heading, and between two sections.
pub const SECTION_GAP: f32 = 24.0;
/// A section's heading row ("Sabitlenenler" and its "Tümü" button; "Önerilenler").
pub const HEAD_H: f32 = 28.0;
/// A pinned tile: the icon, the name on up to two lines.
pub const TILE_H: f32 = 94.0;
/// The tile's icon.
pub const TILE_ICON: f32 = 36.0;
/// A recommended entry.
pub const REC_ROW_H: f32 = 52.0;
/// The bottom row (the account, the gear, the power button), without the line above it.
pub const FOOTER_H: f32 = 64.0;
/// The one-line notice ("Başlatılamadı…") between the page and the line above the bottom row, while there is one.
pub const NOTICE_H: f32 = 28.0;
/// The line between the content and the bottom row.
pub const DIVIDER_H: f32 = 1.0;

/// The pinned grid's width in tiles.
pub const COLS: usize = 6;
/// "Önerilenler" is two columns of entries, filled across.
pub const REC_COLS: usize = 2;
/// At most this many recommended entries.
pub const REC_MAX: usize = 6;
/// The pinned grid's rows next to "Önerilenler".
pub const ROWS: usize = 3;
/// The pinned grid's rows when there is nothing recommended and the grid takes the room.
pub const ROWS_ROOMY: usize = 5;

/// What the panel spends on its frame whatever it shows: the top, the search field, the gap under it, the line and the bottom row.
const CHROME_H: f32 = TOP_PAD + SEARCH_H + SECTION_GAP + DIVIDER_H + FOOTER_H;

// ---------------------------------------------------------------- the size

/// The room above the island, logical px: from the work area's top edge down to the panel's gap above the island's glass.
pub fn room_above(glass_top: i32, work_top: i32, scale: f32, gap: f32) -> f32 {
    let scale = if scale > 0.0 { scale } else { 1.0 };
    ((glass_top - work_top) as f32 / scale - gap).max(0.0)
}

/// The panel's height for `room` logical px above the island: 720 where there is room, else all the room (never more: a panel that is
/// taller than the room above the island would cover it, or leave the screen). At least 1.
pub fn panel_height(room: f32) -> f32 {
    if room.is_nan() { PANEL_MAX_H } else { room.clamp(1.0, PANEL_MAX_H) }
}

// ---------------------------------------------------------------- the pages

/// Which page is shown (a typed query is shown over either).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Home,
    All,
}

/// What Escape does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Escape {
    /// There is a query: it is emptied (the page underneath shows again).
    ClearQuery,
    /// The all-apps page, nothing typed: back to the home page.
    Back,
    /// The home page, nothing typed: the panel closes.
    Close,
}

pub fn escape(page: Page, query_empty: bool) -> Escape {
    match (query_empty, page) {
        (false, _) => Escape::ClearQuery,
        (true, Page::All) => Escape::Back,
        (true, Page::Home) => Escape::Close,
    }
}

/// Whether Backspace takes the all-apps page back to the home page: when the search is empty (there is nothing for the key to delete).
pub fn backspace_goes_back(page: Page, query_empty: bool) -> bool {
    page == Page::All && query_empty
}

// ---------------------------------------------------------------- Tab

/// A keyboard stop of the panel, in the order Tab visits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TabStop {
    Field,
    /// The "Tümü" / "Geri" button.
    PageButton,
    Gear,
    Power,
}

/// The stop Tab (or Shift+Tab with `back`) goes to from `at`. The page button is a stop only while it is on the screen: the result list
/// and the "loading" text draw none, and a stop on a thing that is not drawn is an invisible dead end. A `at` that is no longer a
/// stop (the button went away while it had the focus) counts as the field.
pub fn next_tab_stop(at: TabStop, back: bool, page_button_shown: bool) -> TabStop {
    let stops: &[TabStop] = if page_button_shown { &[TabStop::Field, TabStop::PageButton, TabStop::Gear, TabStop::Power] } else { &[TabStop::Field, TabStop::Gear, TabStop::Power] };
    let i = stops.iter().position(|s| *s == at).unwrap_or(0);
    stops[if back { (i + stops.len() - 1) % stops.len() } else { (i + 1) % stops.len() }]
}

/// Whether the page button is drawn: not over a result list, and not before the apps list has come.
pub fn page_button_shown(searching: bool, apps_ready: bool) -> bool {
    !searching && apps_ready
}

// ---------------------------------------------------------------- the home page's rows

/// What the home page spends its height on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HomeLayout {
    /// Rows of pinned tiles on a page.
    pub rows: usize,
    /// Tiles on a page.
    pub per_page: usize,
    /// Pages for the pins there are (at least one).
    pub pages: usize,
    /// Rows of recommended entries; 0: the section is not drawn.
    pub rec_rows: usize,
}

impl HomeLayout {
    /// The height of everything the layout draws (the frame included), for the panel's height.
    pub fn height(self) -> f32 {
        CHROME_H + pinned_cost(self.rows) + rec_cost(self.rec_rows)
    }
}

fn pinned_cost(rows: usize) -> f32 {
    HEAD_H + rows as f32 * TILE_H
}

fn rec_cost(rows: usize) -> f32 {
    if rows == 0 { 0.0 } else { SECTION_GAP + HEAD_H + rows as f32 * REC_ROW_H }
}

/// The height the home page may fit itself to when the window is `h` high: the notice row, while there is one, is paid for first, so
/// that a page that fits exactly does not draw over it.
pub fn page_room(h: f32, notice: bool) -> f32 {
    if notice { (h - NOTICE_H).max(0.0) } else { h }
}

/// Fits the home page to the panel's height `h`. `pins` is how many pinned apps there are, `recs` how many recommended entries there
/// are to show (0: none, or the section is switched off; a panel that does not know yet passes [`REC_MAX`], so that the grid does not
/// grow when the entries arrive). Three rows of tiles and all the recommended rows when they fit; on a short panel first the grid
/// gives up a row (down to two), then "Önerilenler" (down to one row), then the grid down to one row, and last "Önerilenler" itself.
/// Without recommended entries the grid takes the room, up to five rows, as many as the pins need.
pub fn home_layout(h: f32, pins: usize, recs: usize) -> HomeLayout {
    let body = (h - CHROME_H).max(0.0);
    let mut rec_rows = recs.min(REC_MAX).div_ceil(REC_COLS);
    let mut rows = if rec_rows == 0 { pins.div_ceil(COLS).clamp(ROWS, ROWS_ROOMY) } else { ROWS };
    while pinned_cost(rows) + rec_cost(rec_rows) > body {
        if rows > 2 {
            rows -= 1;
        } else if rec_rows > 1 {
            rec_rows -= 1;
        } else if rows > 1 {
            rows -= 1;
        } else if rec_rows > 0 {
            rec_rows = 0;
        } else {
            break;
        }
    }
    let per_page = rows * COLS;
    HomeLayout { rows, per_page, pages: pins.div_ceil(per_page).max(1), rec_rows }
}

/// The page a pinned app (its index among the pins) is on.
pub fn page_of(pin: usize, per_page: usize) -> usize {
    pin / per_page.max(1)
}

/// The pins a page shows: the range of their indexes.
pub fn page_range(page: usize, per_page: usize, pins: usize) -> std::ops::Range<usize> {
    let from = (page * per_page).min(pins);
    from..(from + per_page).min(pins)
}

/// A page number held to the pages there are (the pins or the height changed under it).
pub fn clamp_page(page: usize, pages: usize) -> usize {
    page.min(pages.max(1) - 1)
}

// ---------------------------------------------------------------- the keyboard on the home page

/// Where the keyboard cursor is on the home page: on a pinned app (its index among all the pins) or on a recommended entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spot {
    Pin(usize),
    Rec(usize),
}

/// The cursor after an arrow or page key. `page` is the page the grid shows, `per_page` and `pins` as in [`home_layout`], `recs` the
/// recommended entries drawn. No cursor yet: Down, Right and Page Down land on the first thing there is. The ends hold (no wrapping).
/// Down from the grid's bottom row goes on to the recommended entries, Up from their first row goes back to the grid's bottom row;
/// Page Down and Page Up turn the grid's pages; Left and Right run along the pins page after page, and between the two columns of
/// the recommended entries.
pub fn step_home(cur: Option<Spot>, mv: Move, page: usize, per_page: usize, pins: usize, recs: usize) -> Option<Spot> {
    let per_page = per_page.max(COLS);
    let first = || if pins > 0 { Some(Spot::Pin(page_range(page, per_page, pins).start)) } else if recs > 0 { Some(Spot::Rec(0)) } else { None };
    let Some(cur) = cur else {
        return match mv {
            Move::Down | Move::Right | Move::PageDown => first(),
            _ => None,
        };
    };
    match cur {
        Spot::Pin(i) => {
            if pins == 0 {
                return if recs > 0 { Some(Spot::Rec(0)) } else { None };
            }
            let i = i.min(pins - 1);
            let at = page_of(i, per_page);
            let page_end = ((at + 1) * per_page).min(pins);
            Some(match mv {
                Move::Left => Spot::Pin(i.saturating_sub(1)),
                Move::Right => Spot::Pin((i + 1).min(pins - 1)),
                Move::Up => Spot::Pin(if i % per_page >= COLS { i - COLS } else { i }),
                Move::Down => {
                    if (i / COLS + 1) * COLS < page_end {
                        // (a partial last row: its last tile)
                        Spot::Pin((i + COLS).min(page_end - 1))
                    } else if recs > 0 {
                        // under the grid's left half the left column, under its right half the right one
                        Spot::Rec(if recs > 1 && (i % COLS) >= COLS / 2 { 1 } else { 0 })
                    } else {
                        Spot::Pin(i)
                    }
                }
                Move::PageDown => {
                    if page_end < pins { Spot::Pin((i + per_page).min(pins - 1)) } else { Spot::Pin(i) }
                }
                Move::PageUp => {
                    if at > 0 { Spot::Pin(i - per_page) } else { Spot::Pin(i) }
                }
            })
        }
        Spot::Rec(j) => {
            if recs == 0 {
                return None;
            }
            let j = j.min(recs - 1);
            Some(match mv {
                Move::Left => Spot::Rec(if j % REC_COLS == 1 { j - 1 } else { j }),
                Move::Right => Spot::Rec(if j % REC_COLS == 0 && j + 1 < recs { j + 1 } else { j }),
                Move::Down | Move::PageDown => {
                    if (j / REC_COLS + 1) * REC_COLS < recs {
                        Spot::Rec((j + REC_COLS).min(recs - 1))
                    } else {
                        Spot::Rec(j)
                    }
                }
                Move::Up | Move::PageUp => {
                    if j >= REC_COLS {
                        Spot::Rec(j - REC_COLS)
                    } else if pins > 0 {
                        // the grid's bottom row of the shown page, under the column the entry was in
                        let start = page_range(page, per_page, pins).start;
                        let end = (start + per_page).min(pins);
                        let bottom = start + (per_page / COLS - 1) * COLS;
                        Spot::Pin((bottom + (j % REC_COLS) * (COLS / 2) + COLS / 4).min(end - 1))
                    } else {
                        Spot::Rec(j)
                    }
                }
            })
        }
    }
}

// ---------------------------------------------------------------- the wheel

/// Turns the mouse wheel's stream into pages. This toolkit spreads one notch over several small events and a touchpad sends many, so a
/// page turns when enough has been rolled in one direction and the rest of the stream is ignored for a moment.
#[derive(Clone, Copy, Debug, Default)]
pub struct WheelPager {
    rolled: f32,
    quiet_until_ms: u64,
}

/// How far (in device-independent px of scrolling) the wheel is rolled before a page turns.
pub const WHEEL_TURN: f32 = 36.0;
/// After a page turned, the wheel is ignored for this long (the tail of the same notch, a touchpad's coasting).
pub const WHEEL_QUIET_MS: u64 = 320;

impl WheelPager {
    /// One wheel event: `dy` is the scroll distance (positive: the wheel rolled away from the user, the way Windows reports it, which
    /// scrolls UP), `now_ms` a clock. Returns -1 (the previous page), +1 (the next one) or 0.
    pub fn feed(&mut self, dy: f32, now_ms: u64) -> i32 {
        if now_ms < self.quiet_until_ms {
            self.rolled = 0.0;
            return 0;
        }
        // (rolling the other way drops what was rolled before)
        if self.rolled * dy < 0.0 {
            self.rolled = 0.0;
        }
        self.rolled += dy;
        if self.rolled.abs() < WHEEL_TURN {
            return 0;
        }
        let turn = if self.rolled < 0.0 { 1 } else { -1 };
        self.rolled = 0.0;
        self.quiet_until_ms = now_ms + WHEEL_QUIET_MS;
        turn
    }
}

/// The keyboard cursor after the grid turned to another page by the wheel or a dot (not by the cursor itself). A cursor on a pin stays
/// on the page that left, where Enter would start an app that is not on the screen, so it is dropped; the next arrow key lands on the
/// page shown. A cursor on a recommended entry is not on the grid and stays.
pub fn spot_after_turn(spot: Option<Spot>) -> Option<Spot> {
    match spot {
        Some(Spot::Pin(_)) => None,
        other => other,
    }
}

/// The page after a turn of `turn` (-1, 0, +1), held to the pages there are.
pub fn turned(page: usize, pages: usize, turn: i32) -> usize {
    let p = if turn < 0 { page.saturating_sub(1) } else if turn > 0 { page + 1 } else { page };
    clamp_page(p, pages)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escape_steps_back_one_state_at_a_time_and_only_the_home_page_closes() {
        assert_eq!(escape(Page::Home, false), Escape::ClearQuery);
        assert_eq!(escape(Page::All, false), Escape::ClearQuery, "a query is emptied first, on either page");
        assert_eq!(escape(Page::All, true), Escape::Back);
        assert_eq!(escape(Page::Home, true), Escape::Close);
    }

    #[test]
    fn backspace_leads_back_only_from_the_all_apps_page_with_nothing_typed() {
        assert!(backspace_goes_back(Page::All, true));
        assert!(!backspace_goes_back(Page::All, false), "with text typed it deletes");
        assert!(!backspace_goes_back(Page::Home, true), "the home page has no page behind it");
        assert_eq!(Page::default(), Page::Home, "the panel opens on the home page");
    }

    #[test]
    fn the_panel_is_never_taller_than_the_room_above_the_island() {
        assert_eq!(panel_height(900.0), PANEL_MAX_H);
        assert_eq!(panel_height(720.0), 720.0);
        assert_eq!(panel_height(540.0), 540.0);
        assert_eq!(panel_height(0.0), 1.0);
        assert_eq!(panel_height(-30.0), 1.0);
        assert_eq!(panel_height(f32::NAN), PANEL_MAX_H);
        // 1920 x 1080 at 100 %: the island's glass is 974 px down and the work area starts at 0: 974 - 8 = 966 px of room
        assert_eq!(room_above(974, 0, 1.0, 8.0), 966.0);
        // at 150 % the same screen is 2880 x 1620 and the numbers are device px: the room is in logical px again
        assert_eq!(room_above(1461, 0, 1.5, 8.0), 966.0);
        // a work area that starts lower (a bar on top) and a glass near it
        assert_eq!(room_above(100, 60, 1.0, 8.0), 32.0);
        assert_eq!(room_above(50, 60, 1.0, 8.0), 0.0, "no room is no negative room");
        assert_eq!(room_above(500, 0, 0.0, 8.0), 492.0, "a scale that was not read is 100 %");
    }

    #[test]
    fn at_full_height_there_are_three_rows_of_pins_and_three_of_recommended_and_it_all_fits() {
        let l = home_layout(PANEL_MAX_H, 13, 6);
        assert_eq!((l.rows, l.per_page, l.pages, l.rec_rows), (3, 18, 1, 3));
        assert!(l.height() <= PANEL_MAX_H, "{}", l.height());
        // two columns across: five entries are three rows, four are two
        assert_eq!(home_layout(PANEL_MAX_H, 13, 5).rec_rows, 3);
        assert_eq!(home_layout(PANEL_MAX_H, 13, 4).rec_rows, 2);
        assert_eq!(home_layout(PANEL_MAX_H, 13, 1).rec_rows, 1);
        assert_eq!(home_layout(PANEL_MAX_H, 13, 40).rec_rows, 3, "six entries at most");
    }

    #[test]
    fn more_pins_than_a_page_holds_make_pages() {
        assert_eq!(home_layout(PANEL_MAX_H, 18, 6).pages, 1);
        assert_eq!(home_layout(PANEL_MAX_H, 19, 6).pages, 2);
        assert_eq!(home_layout(PANEL_MAX_H, 36, 6).pages, 2);
        assert_eq!(home_layout(PANEL_MAX_H, 37, 6).pages, 3);
        assert_eq!(home_layout(PANEL_MAX_H, 0, 6).pages, 1, "no pins is one empty page, not none");
    }

    #[test]
    fn with_nothing_recommended_the_grid_takes_the_room() {
        // up to five rows, as many as the pins need, never fewer than three
        assert_eq!(home_layout(PANEL_MAX_H, 13, 0).rows, 3);
        assert_eq!(home_layout(PANEL_MAX_H, 19, 0).rows, 4);
        assert_eq!(home_layout(PANEL_MAX_H, 31, 0).rows, 5);
        assert_eq!(home_layout(PANEL_MAX_H, 200, 0).rows, 5);
        let l = home_layout(PANEL_MAX_H, 31, 0);
        assert_eq!((l.per_page, l.pages, l.rec_rows), (30, 2, 0));
        assert!(l.height() <= PANEL_MAX_H);
    }

    #[test]
    fn a_short_panel_gives_up_rows_in_order_and_keeps_the_pins() {
        // every height from the smallest to the largest: what is drawn always fits, and shrinks in the stated order
        let mut last = home_layout(PANEL_MAX_H, 30, 6);
        let mut h = PANEL_MAX_H;
        while h > 200.0 {
            h -= 4.0;
            let l = home_layout(h, 30, 6);
            if l.height() > h {
                // only the very smallest panels (the pins' own one row and the frame do not fit) may overflow
                assert_eq!((l.rows, l.rec_rows), (1, 0), "overflow at {h} px with {l:?}");
            }
            assert!(l.rows <= last.rows && l.rec_rows <= last.rec_rows, "never grows as the panel shrinks: {h}");
            last = l;
        }
        // the order: a grid row goes first, then recommended rows, then the grid's last rows, then the section
        let at = |h| {
            let l = home_layout(h, 30, 6);
            (l.rows, l.rec_rows)
        };
        assert_eq!(at(720.0), (3, 3));
        assert_eq!(at(560.0), (2, 2));
        assert_eq!(at(480.0), (2, 1));
        assert_eq!(at(440.0), (1, 1));
        assert_eq!(at(300.0), (1, 0));
    }

    #[test]
    fn the_grid_does_not_grow_when_the_recommended_entries_arrive() {
        // the view passes REC_MAX while the recommended entries are still being read, so the grid does not grow when they come
        let waiting = home_layout(PANEL_MAX_H, 13, REC_MAX);
        let arrived = home_layout(PANEL_MAX_H, 13, 6);
        assert_eq!(waiting, arrived);
    }

    #[test]
    fn pages_hold_their_pins_and_a_page_number_is_held_to_the_pages() {
        assert_eq!(page_of(0, 18), 0);
        assert_eq!(page_of(17, 18), 0);
        assert_eq!(page_of(18, 18), 1);
        assert_eq!(page_range(0, 18, 30), 0..18);
        assert_eq!(page_range(1, 18, 30), 18..30);
        assert_eq!(page_range(5, 18, 30), 30..30, "a page past the end is empty");
        assert_eq!(clamp_page(4, 2), 1);
        assert_eq!(clamp_page(0, 0), 0);
        assert_eq!(turned(0, 3, 1), 1);
        assert_eq!(turned(2, 3, 1), 2, "no wrap");
        assert_eq!(turned(0, 3, -1), 0);
        assert_eq!(turned(1, 3, 0), 1);
    }

    // ---- the keyboard

    #[test]
    fn down_from_nothing_lands_on_the_first_tile_of_the_shown_page_or_the_first_entry() {
        assert_eq!(step_home(None, Move::Down, 0, 18, 30, 6), Some(Spot::Pin(0)));
        assert_eq!(step_home(None, Move::Down, 1, 18, 30, 6), Some(Spot::Pin(18)), "the page on the screen");
        assert_eq!(step_home(None, Move::Down, 0, 18, 0, 6), Some(Spot::Rec(0)));
        assert_eq!(step_home(None, Move::Down, 0, 18, 0, 0), None);
        assert_eq!(step_home(None, Move::Up, 0, 18, 30, 6), None);
    }

    #[test]
    fn the_grid_moves_by_tiles_and_rows_and_a_partial_row_gives_its_last_tile() {
        // 8 pins on one page: a row of six and a row of two
        let st = |c, mv| step_home(Some(Spot::Pin(c)), mv, 0, 18, 8, 0);
        assert_eq!(st(0, Move::Right), Some(Spot::Pin(1)));
        assert_eq!(st(7, Move::Right), Some(Spot::Pin(7)), "no wrap");
        assert_eq!(st(0, Move::Left), Some(Spot::Pin(0)));
        assert_eq!(st(2, Move::Down), Some(Spot::Pin(7)), "the partial row's last tile");
        assert_eq!(st(7, Move::Up), Some(Spot::Pin(1)));
        assert_eq!(st(3, Move::Up), Some(Spot::Pin(3)), "top row");
        assert_eq!(st(7, Move::Down), Some(Spot::Pin(7)), "nothing recommended and nothing below: it holds");
    }

    #[test]
    fn left_and_right_run_on_across_the_pages_and_page_keys_turn_them() {
        let st = |c, mv| step_home(Some(Spot::Pin(c)), mv, page_of(c, 18), 18, 40, 0);
        assert_eq!(st(17, Move::Right), Some(Spot::Pin(18)), "onto the next page");
        assert_eq!(st(18, Move::Left), Some(Spot::Pin(17)));
        assert_eq!(st(5, Move::PageDown), Some(Spot::Pin(23)), "the same place on the next page");
        assert_eq!(st(23, Move::PageUp), Some(Spot::Pin(5)));
        assert_eq!(st(5, Move::PageUp), Some(Spot::Pin(5)), "first page");
        assert_eq!(st(39, Move::PageDown), Some(Spot::Pin(39)), "last page");
        assert_eq!(st(35, Move::PageDown), Some(Spot::Pin(39)), "the next page is short: its last tile");
        // Up from the top row of a later page stays there; the row above belongs to the previous page
        assert_eq!(st(18, Move::Up), Some(Spot::Pin(18)));
        assert_eq!(st(24, Move::Up), Some(Spot::Pin(18)));
    }

    #[test]
    fn down_from_the_bottom_row_goes_to_the_recommended_entries_under_the_same_half() {
        // 13 pins: rows of six, six and one
        let st = |c, mv| step_home(Some(Spot::Pin(c)), mv, 0, 18, 13, 6);
        assert_eq!(st(7, Move::Down), Some(Spot::Pin(12)), "the third row's only tile");
        assert_eq!(st(12, Move::Down), Some(Spot::Rec(0)));
        // a full bottom row: the left half leads to the left column, the right half to the right one
        let full = |c| step_home(Some(Spot::Pin(c)), Move::Down, 0, 18, 18, 6);
        assert_eq!(full(12), Some(Spot::Rec(0)));
        assert_eq!(full(14), Some(Spot::Rec(0)));
        assert_eq!(full(15), Some(Spot::Rec(1)));
        assert_eq!(full(17), Some(Spot::Rec(1)));
        // one entry has no right column
        assert_eq!(step_home(Some(Spot::Pin(17)), Move::Down, 0, 18, 18, 1), Some(Spot::Rec(0)));
    }

    #[test]
    fn the_recommended_entries_move_across_and_down_and_up_returns_to_the_grid() {
        let st = |c, mv| step_home(Some(Spot::Rec(c)), mv, 0, 18, 13, 5);
        assert_eq!(st(0, Move::Right), Some(Spot::Rec(1)));
        assert_eq!(st(1, Move::Right), Some(Spot::Rec(1)));
        assert_eq!(st(1, Move::Left), Some(Spot::Rec(0)));
        assert_eq!(st(0, Move::Left), Some(Spot::Rec(0)));
        assert_eq!(st(0, Move::Down), Some(Spot::Rec(2)));
        assert_eq!(st(3, Move::Down), Some(Spot::Rec(4)), "the last row has one entry: the right column falls to it");
        assert_eq!(st(4, Move::Down), Some(Spot::Rec(4)), "the end holds");
        assert_eq!(st(4, Move::Right), Some(Spot::Rec(4)), "no entry to its right");
        assert_eq!(st(3, Move::Up), Some(Spot::Rec(1)));
        // the first row goes back to the grid's bottom row: the 13 pins end in a row of one, so the nearest tile is the last
        assert_eq!(st(0, Move::Up), Some(Spot::Pin(12)));
        // with a full bottom row it is the tile under the column
        let full = |c| step_home(Some(Spot::Rec(c)), Move::Up, 0, 18, 18, 6);
        assert_eq!(full(0), Some(Spot::Pin(13)));
        assert_eq!(full(1), Some(Spot::Pin(16)));
        // no pins: nothing to go back to
        assert_eq!(step_home(Some(Spot::Rec(0)), Move::Up, 0, 18, 0, 6), Some(Spot::Rec(0)));
    }

    #[test]
    fn a_cursor_beyond_what_shrank_is_pulled_back() {
        assert_eq!(step_home(Some(Spot::Pin(50)), Move::Left, 0, 18, 3, 0), Some(Spot::Pin(1)));
        assert_eq!(step_home(Some(Spot::Rec(9)), Move::Left, 0, 18, 3, 2), Some(Spot::Rec(0)));
        assert_eq!(step_home(Some(Spot::Rec(0)), Move::Down, 0, 18, 3, 0), None, "the entries are gone");
        assert_eq!(step_home(Some(Spot::Pin(0)), Move::Down, 0, 18, 0, 3), Some(Spot::Rec(0)), "the pins are gone");
    }

    // ---- the wheel

    #[test]
    fn a_page_turns_when_the_wheel_has_rolled_enough_and_the_rest_of_the_notch_is_ignored() {
        let mut w = WheelPager::default();
        // the wheel rolled toward the user (negative) goes to the next page; the toolkit sends small steps
        let mut t = 1_000;
        let mut turns = Vec::new();
        for _ in 0..10 {
            turns.push(w.feed(-12.0, t));
            t += 12;
        }
        assert_eq!(turns.iter().filter(|&&x| x == 1).count(), 1, "one notch, one page: {turns:?}");
        assert_eq!(turns.iter().filter(|&&x| x == -1).count(), 0);
        // after the quiet time it can turn again, the other way too
        t += WHEEL_QUIET_MS;
        assert_eq!(w.feed(40.0, t), -1);
    }

    #[test]
    fn rolling_back_drops_what_was_rolled_and_a_small_roll_turns_nothing() {
        let mut w = WheelPager::default();
        assert_eq!(w.feed(-20.0, 0), 0);
        assert_eq!(w.feed(15.0, 10), 0, "the other way: starts again from this");
        assert_eq!(w.feed(15.0, 20), 0);
        assert_eq!(w.feed(15.0, 30), -1, "15 + 15 + 15 is enough");
        assert_eq!(w.feed(0.0, 40), 0);
    }

    #[test]
    fn a_page_turned_by_the_wheel_or_a_dot_leaves_no_cursor_on_the_page_that_went() {
        // 40 pins, 18 a page: the cursor is on pin 2 of page 0, the wheel turns to page 1
        let per: usize = 18;
        let pins: usize = 40;
        let cursor = Some(Spot::Pin(2));
        assert_eq!(page_of(2, per), 0);
        let shown = turned(0, pins.div_ceil(per), 1);
        assert_eq!(shown, 1);
        // without the drop, Enter would start pin 2 while page 1 is on the screen
        assert!(page_range(shown, per, pins).start > 2);
        assert_eq!(spot_after_turn(cursor), None);
        // the next arrow key lands on the page that is shown
        assert_eq!(step_home(spot_after_turn(cursor), Move::Down, shown, per, pins, 0), Some(Spot::Pin(page_range(shown, per, pins).start)));
        // a recommended entry is not on the grid
        assert_eq!(spot_after_turn(Some(Spot::Rec(1))), Some(Spot::Rec(1)));
        assert_eq!(spot_after_turn(None), None);
    }

    #[test]
    fn tab_skips_the_page_button_when_it_is_not_drawn() {
        use TabStop::*;
        // drawn: field, button, gear, power, round again
        assert_eq!(next_tab_stop(Field, false, true), PageButton);
        assert_eq!(next_tab_stop(PageButton, false, true), Gear);
        assert_eq!(next_tab_stop(Power, false, true), Field);
        assert_eq!(next_tab_stop(Field, true, true), Power);
        // not drawn (results, loading): the field goes straight to the gear and back
        assert_eq!(next_tab_stop(Field, false, false), Gear);
        assert_eq!(next_tab_stop(Gear, true, false), Field);
        assert_eq!(next_tab_stop(Power, false, false), Field);
        assert_eq!(next_tab_stop(Field, true, false), Power);
        // the button went away while it had the focus: like the field
        assert_eq!(next_tab_stop(PageButton, false, false), Gear);
        assert!(page_button_shown(false, true));
        assert!(!page_button_shown(true, true), "over the results");
        assert!(!page_button_shown(false, false), "before the apps came");
    }

    #[test]
    fn a_notice_row_is_paid_for_before_the_page_is_fitted() {
        // every height and every number of recommended entries: the page plus the notice fits in the window, down to the rows' minimum
        for h in [300.0f32, 420.0, 520.0, 600.0, 660.0, 700.0, 720.0] {
            for recs in [0usize, 2, 6] {
                let l = home_layout(page_room(h, true), 30, recs);
                if l.rows >= 1 && l.height() <= page_room(h, true) {
                    assert!(l.height() + NOTICE_H <= h, "h {h} recs {recs}: {} + notice", l.height());
                }
                // fitted to the whole window instead, the same page can use up the room the notice needs
                let whole = home_layout(h, 30, recs);
                if whole.height() > h - NOTICE_H && whole.height() <= h {
                    assert_ne!(whole, l, "h {h} recs {recs}: the notice must change the fit");
                }
            }
        }
        assert_eq!(page_room(720.0, false), 720.0);
        assert_eq!(page_room(720.0, true), 720.0 - NOTICE_H);
        assert_eq!(page_room(10.0, true), 0.0);
        // a concrete case: three rows and three recommended rows take 667 px of a 680 px window and leave 13: not the 28 the notice needs
        let whole = home_layout(680.0, 30, 6);
        assert_eq!(whole.height(), 667.0);
        let with_notice = home_layout(page_room(680.0, true), 30, 6);
        assert!(with_notice.height() + NOTICE_H <= 680.0, "{with_notice:?}");
        assert!(with_notice.height() < whole.height());
        // at 720 the slack covers it and nothing changes
        assert_eq!(home_layout(page_room(720.0, true), 30, 6), home_layout(720.0, 30, 6));
    }
}
