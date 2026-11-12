//! The dock's app menu (shell plan increment B8): what a right click on an app's icon offers, in which order, how the keyboard moves
//! through it and how tall it is. Pure: the facts about the app are arguments. The app side (`rf-app/src/dock/app_menu.rs`) draws it.
//!
//! TOP TO BOTTOM, like the taskbar's jump list: the sections the app's jump list has (Tasks, the app's own categories, Pinned, Recent
//! or Frequent; the source orders and names them), then the FIXED part: the app's name (starts a new instance), "pin to the dock" or
//! "remove from the dock" (the dock's own pin list, never Windows'), and, for a running app, "close the window" (one window) or "close
//! all windows (n)".
//!
//! The menu opens at once with the fixed part; the sections are read on a worker thread and appear when they arrive
//! ([`Menu::set_sections`]). A highlighted row stays highlighted when they do.

/// A row of the fixed part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fixed {
    /// The app's name: starts a new instance.
    Open,
    /// Pin the app to the dock.
    Pin,
    /// Take the app off the dock.
    Unpin,
    /// Ask the app's one window to close.
    CloseWindow,
    /// Ask all of the app's windows (the count) to close.
    CloseAll(usize),
}

/// Which fixed rows an app gets. `can_start`: the app can be started (it has a launch command); `can_pin`: it can be put on the dock's
/// pin list (the same condition for an app that is not pinned yet); `pinned`: it is on the list; `windows`: how many windows it has.
pub fn fixed_rows(can_start: bool, can_pin: bool, pinned: bool, windows: usize) -> Vec<Fixed> {
    let mut rows = Vec::new();
    if can_start {
        rows.push(Fixed::Open);
    }
    if pinned {
        rows.push(Fixed::Unpin);
    } else if can_pin {
        rows.push(Fixed::Pin);
    }
    match windows {
        0 => {}
        1 => rows.push(Fixed::CloseWindow),
        n => rows.push(Fixed::CloseAll(n)),
    }
    rows
}

/// One row of the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowId {
    /// Entry `.1` of section `.0` of the jump list.
    Entry(usize, usize),
    Fixed(Fixed),
}

/// What one jump-list section looks like to the menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    pub titled: bool,
    pub entries: usize,
}

/// A thing in the menu's column, top to bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    /// The title of section `.0`.
    Header(usize),
    Row(RowId),
    Separator,
}

pub const WIDTH: f32 = 272.0;
pub const ROW_H: f32 = 32.0;
pub const HEADER_H: f32 = 26.0;
pub const SEPARATOR_H: f32 = 9.0;
/// Space inside the menu's edge.
pub const PAD: f32 = 6.0;

/// Most lines one section of the jump list shows (a long Recent list is cut here, as the taskbar's is).
pub const MAX_ENTRIES: usize = 10;

/// How many lines of each section the menu shows (same order and length as `sections`, a count may be 0): at most [`MAX_ENTRIES`] of a
/// section, and fewer when the whole menu would be taller than `room` (logical px; the room above the dock). Lines are dropped from the
/// END of the LAST section first (the least important: Recent comes after Tasks), so the fixed part is never cut off and the keyboard
/// never moves onto a row that is not on the screen.
pub fn fit_entries(sections: &[Shape], fixed: &[Fixed], room: f32) -> Vec<usize> {
    let mut counts: Vec<usize> = sections.iter().map(|s| s.entries.min(MAX_ENTRIES)).collect();
    let height = |counts: &[usize]| {
        let mut m = Menu::new(fixed.to_vec());
        m.set_sections(sections.iter().zip(counts).map(|(s, &n)| Shape { titled: s.titled, entries: n }).collect());
        m.height()
    };
    while height(&counts) > room {
        match counts.iter().rposition(|&n| n > 0) {
            Some(i) => counts[i] -= 1,
            None => break,
        }
    }
    counts
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Menu {
    shapes: Vec<Shape>,
    fixed: Vec<Fixed>,
    hi: Option<RowId>,
}

impl Menu {
    /// A menu with the fixed part only (the sections have not arrived).
    pub fn new(fixed: Vec<Fixed>) -> Menu {
        Menu { shapes: Vec::new(), fixed, hi: None }
    }

    /// The sections arrived. Empty ones are the caller's to leave out. A highlighted fixed row stays highlighted; one in a section
    /// that was replaced does not (there was none the first time they arrived).
    pub fn set_sections(&mut self, shapes: Vec<Shape>) {
        self.shapes = shapes.into_iter().filter(|s| s.entries > 0).collect();
        if matches!(self.hi, Some(RowId::Entry(..))) {
            self.hi = None;
        }
    }

    pub fn fixed(&self) -> &[Fixed] {
        &self.fixed
    }

    pub fn sections(&self) -> usize {
        self.shapes.len()
    }

    /// The highlighted row, if the keyboard or the pointer has chosen one.
    pub fn highlighted(&self) -> Option<RowId> {
        self.hi
    }

    /// Every row the keyboard can reach, top to bottom.
    pub fn rows(&self) -> Vec<RowId> {
        let mut rows = Vec::new();
        for (s, shape) in self.shapes.iter().enumerate() {
            rows.extend((0..shape.entries).map(|e| RowId::Entry(s, e)));
        }
        rows.extend(self.fixed.iter().map(|f| RowId::Fixed(*f)));
        rows
    }

    /// The column, top to bottom: each section (its header when it has a title, its rows), a separator between sections and before the
    /// fixed part, the fixed rows.
    pub fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        for (s, shape) in self.shapes.iter().enumerate() {
            if s > 0 {
                items.push(Item::Separator);
            }
            if shape.titled {
                items.push(Item::Header(s));
            }
            items.extend((0..shape.entries).map(|e| Item::Row(RowId::Entry(s, e))));
        }
        if !self.shapes.is_empty() && !self.fixed.is_empty() {
            items.push(Item::Separator);
        }
        items.extend(self.fixed.iter().map(|f| Item::Row(RowId::Fixed(*f))));
        items
    }

    /// The menu's height, logical px.
    pub fn height(&self) -> f32 {
        2.0 * PAD
            + self
                .items()
                .iter()
                .map(|i| match i {
                    Item::Header(_) => HEADER_H,
                    Item::Row(_) => ROW_H,
                    Item::Separator => SEPARATOR_H,
                })
                .sum::<f32>()
    }

    /// Down (`1`) or up (`-1`) one row, wrapping round; from nothing, down chooses the first row and up the last.
    pub fn step(&mut self, dir: i32) {
        let rows = self.rows();
        if rows.is_empty() {
            self.hi = None;
            return;
        }
        let n = rows.len() as i32;
        let at = self.hi.and_then(|h| rows.iter().position(|r| *r == h));
        let next = match (at, dir.signum()) {
            (None, d) if d < 0 => n - 1,
            (None, _) => 0,
            (Some(i), d) => (i as i32 + d).rem_euclid(n),
        };
        self.hi = Some(rows[next as usize]);
    }

    pub fn home(&mut self) {
        self.hi = self.rows().first().copied();
    }

    pub fn end(&mut self) {
        self.hi = self.rows().last().copied();
    }

    /// The pointer is over a row (`Some`) or over none (`None`).
    pub fn hover(&mut self, row: Option<RowId>) {
        if row.is_some() || self.hi.is_some() {
            self.hi = row.filter(|r| self.rows().contains(r));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_app_with_one_window_gets_open_pin_and_close() {
        assert_eq!(fixed_rows(true, true, false, 1), vec![Fixed::Open, Fixed::Pin, Fixed::CloseWindow]);
    }

    #[test]
    fn a_pinned_app_offers_to_be_removed_and_several_windows_close_together() {
        assert_eq!(fixed_rows(true, true, true, 3), vec![Fixed::Open, Fixed::Unpin, Fixed::CloseAll(3)]);
    }

    #[test]
    fn an_app_that_is_not_running_has_no_close_row() {
        assert_eq!(fixed_rows(true, true, true, 0), vec![Fixed::Open, Fixed::Unpin]);
        assert_eq!(fixed_rows(true, true, false, 0), vec![Fixed::Open, Fixed::Pin]);
    }

    #[test]
    fn an_app_that_cannot_be_started_again_still_closes_and_a_pinned_one_can_always_be_unpinned() {
        assert_eq!(fixed_rows(false, false, false, 2), vec![Fixed::CloseAll(2)]);
        assert_eq!(fixed_rows(false, false, true, 0), vec![Fixed::Unpin]);
        assert!(fixed_rows(false, false, false, 0).is_empty());
    }

    fn shapes() -> Vec<Shape> {
        vec![Shape { titled: false, entries: 2 }, Shape { titled: true, entries: 1 }]
    }

    #[test]
    fn the_menu_before_the_sections_arrive_is_the_fixed_part_alone() {
        let m = Menu::new(fixed_rows(true, true, false, 1));
        assert_eq!(m.items(), vec![Item::Row(RowId::Fixed(Fixed::Open)), Item::Row(RowId::Fixed(Fixed::Pin)), Item::Row(RowId::Fixed(Fixed::CloseWindow))]);
        assert_eq!(m.height(), 2.0 * PAD + 3.0 * ROW_H);
        assert_eq!(m.sections(), 0);
    }

    #[test]
    fn sections_come_first_with_a_separator_between_them_and_before_the_fixed_part() {
        let mut m = Menu::new(vec![Fixed::Open, Fixed::Pin]);
        m.set_sections(shapes());
        assert_eq!(
            m.items(),
            vec![
                Item::Row(RowId::Entry(0, 0)),
                Item::Row(RowId::Entry(0, 1)),
                Item::Separator,
                Item::Header(1),
                Item::Row(RowId::Entry(1, 0)),
                Item::Separator,
                Item::Row(RowId::Fixed(Fixed::Open)),
                Item::Row(RowId::Fixed(Fixed::Pin)),
            ]
        );
        assert_eq!(m.height(), 2.0 * PAD + 5.0 * ROW_H + HEADER_H + 2.0 * SEPARATOR_H);
    }

    #[test]
    fn empty_sections_are_dropped_and_no_separator_hangs_loose() {
        let mut m = Menu::new(vec![Fixed::Open]);
        m.set_sections(vec![Shape { titled: true, entries: 0 }, Shape { titled: false, entries: 0 }]);
        assert_eq!(m.sections(), 0);
        assert_eq!(m.items(), vec![Item::Row(RowId::Fixed(Fixed::Open))]);
        // sections but nothing fixed: no trailing separator
        let mut m = Menu::new(vec![]);
        m.set_sections(vec![Shape { titled: false, entries: 1 }]);
        assert_eq!(m.items(), vec![Item::Row(RowId::Entry(0, 0))]);
    }

    #[test]
    fn a_long_section_is_cut_to_the_most_a_section_shows() {
        let long = vec![Shape { titled: true, entries: 25 }, Shape { titled: true, entries: 3 }];
        assert_eq!(fit_entries(&long, &[Fixed::Open, Fixed::Pin], 10_000.0), vec![MAX_ENTRIES, 3]);
    }

    #[test]
    fn when_the_room_is_short_lines_go_from_the_end_of_the_last_section_and_the_fixed_part_stays() {
        let fixed = [Fixed::Open, Fixed::Pin, Fixed::CloseWindow];
        let shapes = vec![Shape { titled: true, entries: 4 }, Shape { titled: true, entries: 10 }];
        // 4 + 10 lines, two headers, two separators and 3 fixed rows are far taller than a 400 px room
        let counts = fit_entries(&shapes, &fixed, 400.0);
        let mut m = Menu::new(fixed.to_vec());
        m.set_sections(shapes.iter().zip(&counts).map(|(s, &n)| Shape { titled: s.titled, entries: n }).collect());
        assert!(m.height() <= 400.0, "the menu fits: {}", m.height());
        assert_eq!(counts[0], 4, "the first section keeps all its lines while the last one can give");
        assert!(counts[1] < 10);
        // one more line would not have fitted
        let mut more = counts.clone();
        more[1] += 1;
        let mut m2 = Menu::new(fixed.to_vec());
        m2.set_sections(shapes.iter().zip(&more).map(|(s, &n)| Shape { titled: s.titled, entries: n }).collect());
        assert!(m2.height() > 400.0);
        // the fixed rows are all still rows the keyboard reaches
        assert!(fixed.iter().all(|f| m.rows().contains(&RowId::Fixed(*f))));
        assert_eq!(m.rows().len(), counts.iter().sum::<usize>() + fixed.len());
    }

    #[test]
    fn a_room_that_holds_only_the_fixed_part_drops_every_line() {
        let fixed = [Fixed::Open, Fixed::Pin];
        let counts = fit_entries(&[Shape { titled: false, entries: 5 }, Shape { titled: true, entries: 5 }], &fixed, 2.0 * PAD + 2.0 * ROW_H);
        assert_eq!(counts, vec![0, 0]);
        // and a room smaller than that ends the loop instead of looking for more to drop
        assert_eq!(fit_entries(&[Shape { titled: false, entries: 5 }], &fixed, 1.0), vec![0]);
    }

    #[test]
    fn the_arrows_walk_the_rows_top_to_bottom_and_wrap() {
        let mut m = Menu::new(vec![Fixed::Open, Fixed::Pin]);
        m.set_sections(vec![Shape { titled: false, entries: 1 }]);
        assert_eq!(m.highlighted(), None);
        m.step(1);
        assert_eq!(m.highlighted(), Some(RowId::Entry(0, 0)));
        m.step(1);
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Open)));
        m.step(1);
        m.step(1);
        assert_eq!(m.highlighted(), Some(RowId::Entry(0, 0)), "wrapped from the last to the first");
        m.step(-1);
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Pin)), "and back");
    }

    #[test]
    fn up_from_nothing_chooses_the_last_row_and_home_and_end_go_to_the_ends() {
        let mut m = Menu::new(vec![Fixed::Open, Fixed::Pin, Fixed::CloseWindow]);
        m.step(-1);
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::CloseWindow)));
        m.home();
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Open)));
        m.end();
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::CloseWindow)));
    }

    #[test]
    fn the_sections_arriving_keep_a_highlighted_fixed_row() {
        let mut m = Menu::new(vec![Fixed::Open, Fixed::Pin]);
        m.step(1);
        m.step(1);
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Pin)));
        m.set_sections(shapes());
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Pin)), "the row the user moved to is still the one");
        m.step(-1);
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Open)));
        m.step(-1);
        assert_eq!(m.highlighted(), Some(RowId::Entry(1, 0)), "up from the first fixed row reaches the last entry");
    }

    #[test]
    fn a_menu_with_no_rows_has_nothing_to_highlight() {
        let mut m = Menu::new(vec![]);
        m.step(1);
        m.home();
        m.end();
        assert_eq!(m.highlighted(), None);
        assert_eq!(m.height(), 2.0 * PAD);
    }

    #[test]
    fn the_pointer_highlights_the_row_under_it_and_none_when_it_leaves() {
        let mut m = Menu::new(vec![Fixed::Open, Fixed::Pin]);
        m.hover(Some(RowId::Fixed(Fixed::Pin)));
        assert_eq!(m.highlighted(), Some(RowId::Fixed(Fixed::Pin)));
        m.hover(None);
        assert_eq!(m.highlighted(), None);
        // a row that is not in the menu is not highlighted
        m.hover(Some(RowId::Entry(5, 5)));
        assert_eq!(m.highlighted(), None);
    }
}
