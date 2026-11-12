//! Which card is selected, and how Tab and the arrow keys move it. Pure.
//!
//! Tab and Shift+Tab go through the cards one by one and wrap. The arrows move in the grid the cards are laid out in: Left and Right
//! one card (wrapping from the end of a row to the next row, and from the last card to the first), Up and Down one row, keeping the
//! column; going down from the last row wraps to the first row, going up from the first row to the last, and in a short last row a
//! column that is not there lands on its last card.

use super::chord::Dir;
use super::chord::Scope;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    len: usize,
    cols: usize,
    index: usize,
}

impl Selection {
    /// The selection when the switcher opens: forward, the window used before the current one (the second card); backwards, the
    /// last card. With fewer than two cards, the only card.
    pub fn opened(len: usize, cols: usize, backwards: bool) -> Selection {
        Selection::opened_from(len, cols, backwards, true)
    }

    /// Like [`Selection::opened`], for when the first card may not be the window in front (`first_is_current` false: the foreground
    /// is the desktop, the dock or a panel of ours, none of them a card). Forward then selects the first card, which is the window
    /// used most recently: skipping it would be skipping a window for nothing.
    pub fn opened_from(len: usize, cols: usize, backwards: bool, first_is_current: bool) -> Selection {
        let index = match (len, backwards) {
            (0 | 1, _) => 0,
            (n, true) => n - 1,
            (_, false) => usize::from(first_is_current),
        };
        Selection { len, cols: cols.max(1), index }
    }

    pub fn index(&self) -> Option<usize> {
        (self.len > 0).then_some(self.index)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The columns changed (the window was laid out again).
    pub fn set_cols(&mut self, cols: usize) {
        self.cols = cols.max(1);
    }

    /// The pointer is over a card (or a click chose it).
    pub fn set(&mut self, index: usize) {
        if index < self.len {
            self.index = index;
        }
    }

    pub fn step(&mut self, backwards: bool) {
        if self.len == 0 {
            return;
        }
        self.index = if backwards { (self.index + self.len - 1) % self.len } else { (self.index + 1) % self.len };
    }

    pub fn move_dir(&mut self, dir: Dir) {
        if self.len == 0 {
            return;
        }
        match dir {
            Dir::Left => self.step(true),
            Dir::Right => self.step(false),
            Dir::Down | Dir::Up => {
                let cols = self.cols;
                let rows = self.len.div_ceil(cols);
                if rows < 2 {
                    return;
                }
                let (row, col) = (self.index / cols, self.index % cols);
                let to_row = if dir == Dir::Down { (row + 1) % rows } else { (row + rows - 1) % rows };
                // a short last row has no card in this column: its last card
                self.index = (to_row * cols + col).min(self.len - 1);
            }
        }
    }

    /// The card chosen by the key `Enter` / the release of Alt: the selection, if there is a card.
    pub fn chosen(&self) -> Option<usize> {
        self.index()
    }
}

/// Which windows the scope keeps: all of them, or those of the group of the first one (the app in front).
pub fn keep_scope<T>(items: Vec<T>, scope: Scope, group_of: impl Fn(&T) -> String) -> Vec<T> {
    match scope {
        Scope::AllWindows => items,
        Scope::ThisApp => {
            let Some(front) = items.first().map(&group_of) else { return items };
            items.into_iter().filter(|i| group_of(i) == front).collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_forward_selects_the_window_used_before_this_one() {
        assert_eq!(Selection::opened(5, 5, false).index(), Some(1));
        assert_eq!(Selection::opened(2, 2, false).index(), Some(1));
    }

    #[test]
    fn when_the_first_card_is_not_the_window_in_front_the_first_tab_selects_it() {
        assert_eq!(Selection::opened_from(5, 5, false, false).index(), Some(0));
        assert_eq!(Selection::opened_from(5, 5, false, true).index(), Some(1));
        assert_eq!(Selection::opened_from(5, 5, true, false).index(), Some(4), "backwards is the oldest either way");
    }

    #[test]
    fn opening_backwards_selects_the_last() {
        assert_eq!(Selection::opened(5, 5, true).index(), Some(4));
    }

    #[test]
    fn one_window_selects_itself_and_none_selects_nothing() {
        assert_eq!(Selection::opened(1, 1, false).index(), Some(0));
        assert_eq!(Selection::opened(1, 1, true).index(), Some(0));
        let mut none = Selection::opened(0, 3, false);
        assert_eq!(none.index(), None);
        none.step(false);
        none.move_dir(Dir::Down);
        assert_eq!(none.chosen(), None);
    }

    #[test]
    fn tab_wraps_both_ways() {
        let mut s = Selection::opened(3, 3, false); // 1
        s.step(false);
        assert_eq!(s.index(), Some(2));
        s.step(false);
        assert_eq!(s.index(), Some(0));
        s.step(true);
        assert_eq!(s.index(), Some(2));
    }

    #[test]
    fn left_and_right_run_through_the_rows() {
        // 7 cards, 3 columns: rows [0 1 2] [3 4 5] [6]
        let mut s = Selection::opened(7, 3, false);
        s.set(2);
        s.move_dir(Dir::Right);
        assert_eq!(s.index(), Some(3), "to the next row");
        s.move_dir(Dir::Left);
        assert_eq!(s.index(), Some(2));
        s.set(6);
        s.move_dir(Dir::Right);
        assert_eq!(s.index(), Some(0), "from the last card to the first");
    }

    #[test]
    fn up_and_down_keep_the_column_and_wrap() {
        let mut s = Selection::opened(7, 3, false);
        s.set(1);
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(4));
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(6), "the short last row has no column 1: its last card");
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(0), "from the last row to the first row, column 0 (6 % 3)");
        s.set(4);
        s.move_dir(Dir::Up);
        assert_eq!(s.index(), Some(1));
        s.move_dir(Dir::Up);
        assert_eq!(s.index(), Some(6), "from the first row to the last: column 1 is missing there");
    }

    #[test]
    fn with_one_row_up_and_down_do_nothing() {
        let mut s = Selection::opened(3, 5, false);
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(1));
        s.move_dir(Dir::Up);
        assert_eq!(s.index(), Some(1));
    }

    #[test]
    fn setting_a_card_that_is_not_there_is_ignored() {
        let mut s = Selection::opened(3, 3, false);
        s.set(9);
        assert_eq!(s.index(), Some(1));
        s.set(2);
        assert_eq!(s.index(), Some(2));
    }

    #[test]
    fn a_new_column_count_changes_the_vertical_moves() {
        let mut s = Selection::opened(6, 3, false);
        s.set(0);
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(3));
        s.set_cols(2);
        s.set(0);
        s.move_dir(Dir::Down);
        assert_eq!(s.index(), Some(2));
    }

    #[test]
    fn this_app_keeps_the_windows_of_the_app_in_front() {
        let items = vec![("a", 1), ("b", 2), ("a", 3), ("c", 4), ("a", 5)];
        let kept = keep_scope(items.clone(), Scope::ThisApp, |i| i.0.to_string());
        assert_eq!(kept, vec![("a", 1), ("a", 3), ("a", 5)]);
        assert_eq!(keep_scope(items.clone(), Scope::AllWindows, |i| i.0.to_string()), items);
        assert!(keep_scope(Vec::<(&str, i32)>::new(), Scope::ThisApp, |i| i.0.to_string()).is_empty());
    }
}
