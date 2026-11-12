//! The selection model of the desktop and the keyboard's way round a grid with empty cells. Pure.
//!
//! Items sit on a grid that is mostly empty, so the usual "previous / next in the list" does not describe what the arrow keys, Shift
//! or the rubber band mean. The rules, which are this file's own and were chosen to match what Explorer's icon view does:
//!
//! * a plain click selects that item only; a click on an item that is already part of a bigger selection keeps the selection until the
//!   button comes up without a drag (so the whole selection can be dragged), then narrows it to that item;
//! * Ctrl+click toggles one item: an unselected item is added at once, a selected one is taken out when the button comes up without
//!   a drag (so that Ctrl-dragging a selected item moves the whole group, as in Explorer); Shift+click selects every item whose cell lies in the RECTANGLE of cells between the anchor and the
//!   clicked item (an icon view selects by area, not by list order); Ctrl+Shift+click adds that rectangle to what was selected;
//! * the rubber band selects the items whose hit rectangle it touches; with Ctrl or Shift it adds to what was selected;
//! * Down / Up move inside the cursor's column to the next item below / above; Right / Left go to the nearest column that has an item
//!   and take the item in it whose row is nearest to the cursor's (the upper one on a tie); there is no wrap round and no jump to
//!   the next column at the end of one; Home / End are the first / last item in column-major order;
//! * with nothing selected, any arrow key selects the first item (End: the last);
//! * Shift+arrow extends: the rectangle between the anchor and the new cursor.

use std::collections::BTreeSet;

use rf_core::desktop::geom::Rect;
use rf_core::desktop::grid::Cell;
use rf_core::desktop::items::ItemId;

/// One item that is on the canvas: where, and the part of the canvas a click or the rubber band must touch to mean this item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub id: ItemId,
    pub cell: Cell,
    pub hit: Rect,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Mods {
    pub ctrl: bool,
    pub shift: bool,
}

/// The item to move the cursor to, or `None` when there is nowhere to go (the cursor stays).
pub fn neighbour(items: &[Placed], from: Option<&ItemId>, dir: Dir) -> Option<ItemId> {
    let first = || items.iter().min_by_key(|p| (p.cell.col, p.cell.row)).map(|p| p.id.clone());
    let last = || items.iter().max_by_key(|p| (p.cell.col, p.cell.row)).map(|p| p.id.clone());
    match dir {
        Dir::Home => return first(),
        Dir::End => return last(),
        _ => {}
    }
    let Some(cur) = from.and_then(|id| items.iter().find(|p| &p.id == id)) else {
        return first();
    };
    let c = cur.cell;
    let pick = match dir {
        Dir::Down => items.iter().filter(|p| p.cell.col == c.col && p.cell.row > c.row).min_by_key(|p| p.cell.row),
        Dir::Up => items.iter().filter(|p| p.cell.col == c.col && p.cell.row < c.row).max_by_key(|p| p.cell.row),
        Dir::Right => {
            let col = items.iter().filter(|p| p.cell.col > c.col).map(|p| p.cell.col).min()?;
            nearest_row(items, col, c.row)
        }
        Dir::Left => {
            let col = items.iter().filter(|p| p.cell.col < c.col).map(|p| p.cell.col).max()?;
            nearest_row(items, col, c.row)
        }
        Dir::Home | Dir::End => None,
    };
    pick.map(|p| p.id.clone())
}

fn nearest_row(items: &[Placed], col: u32, row: u32) -> Option<&Placed> {
    items.iter().filter(|p| p.cell.col == col).min_by_key(|p| ((p.cell.row as i64 - row as i64).abs(), p.cell.row))
}

/// The items whose cell is in the rectangle of cells spanned by `a` and `b` (both ends included).
pub fn cells_between(items: &[Placed], a: Cell, b: Cell) -> BTreeSet<ItemId> {
    let (c0, c1) = (a.col.min(b.col), a.col.max(b.col));
    let (r0, r1) = (a.row.min(b.row), a.row.max(b.row));
    items.iter().filter(|p| (c0..=c1).contains(&p.cell.col) && (r0..=r1).contains(&p.cell.row)).map(|p| p.id.clone()).collect()
}

/// The items the rubber band touches.
pub fn band_hits(items: &[Placed], band: Rect) -> BTreeSet<ItemId> {
    if band.is_empty() {
        return BTreeSet::new();
    }
    items.iter().filter(|p| !p.hit.intersect(&band).is_empty()).map(|p| p.id.clone()).collect()
}

/// The rectangle between two canvas points, however they are ordered, at least one pixel in each direction.
pub fn rect_of(a: (i32, i32), b: (i32, i32)) -> Rect {
    Rect::new(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0) + 1, a.1.max(b.1) + 1)
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub selected: BTreeSet<ItemId>,
    /// The item the keyboard is on (and F2, Enter and the menu key act on when nothing else says).
    pub cursor: Option<ItemId>,
    /// The fixed end of a Shift range.
    pub anchor: Option<ItemId>,
    /// A plain press landed on an item of a bigger selection: [`Selection::release`] narrows it to that item unless a drag began.
    pending_narrow: Option<ItemId>,
    /// A Ctrl press landed on a selected item: [`Selection::release`] takes it out unless a drag began.
    pending_toggle: Option<ItemId>,
}

fn cell_of(items: &[Placed], id: &ItemId) -> Option<Cell> {
    items.iter().find(|p| &p.id == id).map(|p| p.cell)
}

impl Selection {
    pub fn is_selected(&self, id: &ItemId) -> bool {
        self.selected.contains(id)
    }
    pub fn clear(&mut self) {
        self.selected.clear();
        self.pending_narrow = None;
        self.pending_toggle = None;
    }
    pub fn only(&mut self, id: &ItemId) {
        self.selected.clear();
        self.selected.insert(id.clone());
        self.cursor = Some(id.clone());
        self.anchor = Some(id.clone());
        self.pending_narrow = None;
        self.pending_toggle = None;
    }
    pub fn select_all(&mut self, items: &[Placed]) {
        self.selected = items.iter().map(|p| p.id.clone()).collect();
        self.pending_narrow = None;
        self.pending_toggle = None;
        if self.cursor.is_none() {
            self.cursor = neighbour(items, None, Dir::Home);
            self.anchor = self.cursor.clone();
        }
    }

    /// A mouse press on an item.
    pub fn press(&mut self, id: &ItemId, m: Mods, items: &[Placed]) {
        self.pending_narrow = None;
        self.pending_toggle = None;
        if m.shift {
            let anchor = self.anchor.clone().filter(|a| cell_of(items, a).is_some()).unwrap_or_else(|| id.clone());
            let (Some(a), Some(b)) = (cell_of(items, &anchor), cell_of(items, id)) else { return self.only(id) };
            let range = cells_between(items, a, b);
            if m.ctrl {
                self.selected.extend(range);
            } else {
                self.selected = range;
            }
            self.anchor = Some(anchor);
            self.cursor = Some(id.clone());
        } else if m.ctrl {
            if self.selected.contains(id) {
                self.pending_toggle = Some(id.clone());
            } else {
                self.selected.insert(id.clone());
            }
            self.cursor = Some(id.clone());
            self.anchor = Some(id.clone());
        } else if self.selected.contains(id) {
            // inside a selection: keep it, in case it is about to be dragged
            if self.selected.len() > 1 {
                self.pending_narrow = Some(id.clone());
            }
            self.cursor = Some(id.clone());
            self.anchor = Some(id.clone());
        } else {
            self.only(id);
        }
    }

    /// The button came up on the item without a drag in between.
    pub fn release(&mut self) {
        if let Some(id) = self.pending_narrow.take() {
            self.only(&id);
        }
        if let Some(id) = self.pending_toggle.take() {
            self.selected.remove(&id);
        }
    }

    /// A drag began: the selection stays as it is.
    pub fn drag_began(&mut self) {
        self.pending_narrow = None;
        self.pending_toggle = None;
    }

    /// The rubber band is at `band` (canvas pixels); `base` is what was selected when it started, kept when `additive`.
    pub fn rubber_band(&mut self, base: &BTreeSet<ItemId>, band: Rect, items: &[Placed], additive: bool) {
        let mut next = if additive { base.clone() } else { BTreeSet::new() };
        next.extend(band_hits(items, band));
        self.selected = next;
        self.pending_narrow = None;
        self.pending_toggle = None;
    }

    /// An arrow key, Home or End. With `extend` the selection becomes the rectangle from the anchor to the new cursor.
    pub fn move_cursor(&mut self, dir: Dir, extend: bool, items: &[Placed]) {
        let Some(to) = neighbour(items, self.cursor.as_ref(), dir) else {
            // nowhere to go; with nothing selected yet the cursor item becomes the selection
            if self.selected.is_empty()
                && let Some(c) = self.cursor.clone().filter(|c| cell_of(items, c).is_some())
            {
                self.only(&c);
            }
            return;
        };
        if extend {
            let anchor = self.anchor.clone().filter(|a| cell_of(items, a).is_some()).or_else(|| self.cursor.clone()).unwrap_or_else(|| to.clone());
            if let (Some(a), Some(b)) = (cell_of(items, &anchor), cell_of(items, &to)) {
                self.selected = cells_between(items, a, b);
            }
            self.anchor = Some(anchor);
            self.cursor = Some(to);
            self.pending_narrow = None;
            self.pending_toggle = None;
        } else {
            self.only(&to);
        }
    }

    /// Items that are gone (deleted, renamed away, on another monitor now) leave the selection.
    pub fn retain_existing(&mut self, items: &[Placed]) {
        let there: BTreeSet<&ItemId> = items.iter().map(|p| &p.id).collect();
        self.selected.retain(|i| there.contains(i));
        if self.cursor.as_ref().is_some_and(|c| !there.contains(c)) {
            self.cursor = None;
        }
        if self.anchor.as_ref().is_some_and(|c| !there.contains(c)) {
            self.anchor = None;
        }
        if self.pending_narrow.as_ref().is_some_and(|c| !there.contains(c)) {
            self.pending_narrow = None;
        }
        if self.pending_toggle.as_ref().is_some_and(|c| !there.contains(c)) {
            self.pending_toggle = None;
        }
    }

    /// An item was renamed: it stays selected under its new name.
    pub fn renamed(&mut self, from: &ItemId, to: &ItemId) {
        if self.selected.remove(from) {
            self.selected.insert(to.clone());
        }
        for slot in [&mut self.cursor, &mut self.anchor, &mut self.pending_narrow, &mut self.pending_toggle] {
            if slot.as_ref() == Some(from) {
                *slot = Some(to.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> ItemId {
        ItemId::user(n)
    }
    /// `(name, col, row)`; the hit rectangle is the 75-pixel cell shrunk by 5.
    fn grid(spec: &[(&str, u32, u32)]) -> Vec<Placed> {
        spec.iter()
            .map(|(n, c, r)| Placed { id: id(n), cell: Cell::new(*c, *r), hit: Rect::from_size(*c as i32 * 75 + 5, *r as i32 * 75 + 5, 65, 65) })
            .collect()
    }
    fn names(s: &BTreeSet<ItemId>) -> Vec<String> {
        s.iter().map(|i| i.as_str().trim_start_matches("u:").to_string()).collect()
    }

    //   col0   col1   col2   col3
    // r0  a      .      e      .
    // r1  b      d      .      .
    // r2  .      .      f      g
    // r3  c      .      .      .
    fn sparse() -> Vec<Placed> {
        grid(&[("a", 0, 0), ("b", 0, 1), ("c", 0, 3), ("d", 1, 1), ("e", 2, 0), ("f", 2, 2), ("g", 3, 2)])
    }

    #[test]
    fn down_and_up_stay_in_the_column_and_skip_empty_cells() {
        let it = sparse();
        assert_eq!(neighbour(&it, Some(&id("b")), Dir::Down), Some(id("c")), "the empty cell (0,2) is skipped");
        assert_eq!(neighbour(&it, Some(&id("c")), Dir::Up), Some(id("b")));
        assert_eq!(neighbour(&it, Some(&id("c")), Dir::Down), None, "the end of the column is the end: no jump to the next column");
        assert_eq!(neighbour(&it, Some(&id("a")), Dir::Up), None);
        assert_eq!(neighbour(&it, Some(&id("d")), Dir::Down), None, "d is alone in its column");
    }

    #[test]
    fn right_and_left_take_the_nearest_column_and_the_nearest_row_in_it() {
        let it = sparse();
        // from b (0,1): column 1 has d at row 1
        assert_eq!(neighbour(&it, Some(&id("b")), Dir::Right), Some(id("d")));
        // from d (1,1): column 2 has e (row 0) and f (row 2), both one row away: the upper wins
        assert_eq!(neighbour(&it, Some(&id("d")), Dir::Right), Some(id("e")));
        // from c (0,3): column 1 has only d
        assert_eq!(neighbour(&it, Some(&id("c")), Dir::Right), Some(id("d")));
        // from g (3,2): column 2, row 2 is f
        assert_eq!(neighbour(&it, Some(&id("g")), Dir::Left), Some(id("f")));
        // from f (2,2) left: column 1 -> d (the only one)
        assert_eq!(neighbour(&it, Some(&id("f")), Dir::Left), Some(id("d")));
        assert_eq!(neighbour(&it, Some(&id("a")), Dir::Left), None);
        assert_eq!(neighbour(&it, Some(&id("g")), Dir::Right), None);
    }

    #[test]
    fn home_and_end_follow_column_major_order_and_nothing_selected_starts_at_the_first() {
        let it = sparse();
        assert_eq!(neighbour(&it, Some(&id("f")), Dir::Home), Some(id("a")));
        assert_eq!(neighbour(&it, Some(&id("a")), Dir::End), Some(id("g")));
        for d in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
            assert_eq!(neighbour(&it, None, d), Some(id("a")), "{d:?}");
        }
        // a cursor on an item that is no longer there counts as none
        assert_eq!(neighbour(&it, Some(&id("gone")), Dir::Down), Some(id("a")));
        assert_eq!(neighbour(&[], None, Dir::Home), None);
    }

    #[test]
    fn a_plain_click_selects_one_ctrl_toggles_shift_selects_the_rectangle() {
        let it = sparse();
        let mut s = Selection::default();
        s.press(&id("a"), Mods::default(), &it);
        assert_eq!(names(&s.selected), ["a"]);
        s.press(&id("e"), Mods { ctrl: true, shift: false }, &it);
        assert_eq!(names(&s.selected), ["a", "e"], "an unselected item is added at once");
        s.press(&id("a"), Mods { ctrl: true, shift: false }, &it);
        assert_eq!(names(&s.selected), ["a", "e"], "a selected one stays until the button comes up");
        s.release();
        assert_eq!(names(&s.selected), ["e"], "ctrl toggles off on release");
        s.only(&id("e"));
        // shift from the anchor (e at (2,0)) to f (2,2): the column segment
        s.press(&id("f"), Mods { ctrl: false, shift: true }, &it);
        assert_eq!(names(&s.selected), ["e", "f"]);
        // shift to d (1,1): rectangle cols 1..=2, rows 0..=1 -> d, e (f is on row 2)
        s.press(&id("d"), Mods { ctrl: false, shift: true }, &it);
        assert_eq!(names(&s.selected), ["d", "e"]);
        assert_eq!(s.cursor, Some(id("d")));
        assert_eq!(s.anchor, Some(id("e")), "the anchor does not move");
        // ctrl+shift adds
        s.press(&id("a"), Mods { ctrl: true, shift: false }, &it);
        s.release();
        s.press(&id("c"), Mods { ctrl: true, shift: true }, &it);
        assert!(names(&s.selected).contains(&"d".to_string()) && names(&s.selected).contains(&"c".to_string()));
    }

    #[test]
    fn a_press_inside_a_selection_keeps_it_for_a_drag_and_narrows_on_a_plain_release() {
        let it = sparse();
        let mut s = Selection::default();
        s.select_all(&it);
        s.press(&id("d"), Mods::default(), &it);
        assert_eq!(s.selected.len(), 7, "kept: it may be dragged");
        s.release();
        assert_eq!(names(&s.selected), ["d"]);
        // a drag in between keeps it
        s.select_all(&it);
        s.press(&id("d"), Mods::default(), &it);
        s.drag_began();
        s.release();
        assert_eq!(s.selected.len(), 7);
        // a release with no press pending changes nothing
        s.release();
        assert_eq!(s.selected.len(), 7);
        // pressing on a single selected item does not arm a narrowing
        s.only(&id("a"));
        s.press(&id("a"), Mods::default(), &it);
        s.release();
        assert_eq!(names(&s.selected), ["a"]);
    }

    #[test]
    fn ctrl_dragging_a_selected_item_moves_the_whole_group() {
        let it = sparse();
        let mut s = Selection::default();
        s.select_all(&it);
        s.press(&id("d"), Mods { ctrl: true, shift: false }, &it);
        s.drag_began();
        s.release();
        assert_eq!(s.selected.len(), 7, "the drag keeps every item selected");
        // a Ctrl press on an item that is not selected adds it, and a drag from there takes the group along
        s.only(&id("a"));
        s.press(&id("b"), Mods { ctrl: true, shift: false }, &it);
        s.drag_began();
        s.release();
        assert_eq!(names(&s.selected), ["a", "b"]);
        // a pending toggle does not survive the item vanishing
        s.press(&id("a"), Mods { ctrl: true, shift: false }, &it);
        let fewer: Vec<Placed> = it.iter().filter(|p| p.id != id("a")).cloned().collect();
        s.retain_existing(&fewer);
        s.release();
        assert_eq!(names(&s.selected), ["b"]);
    }

    #[test]
    fn shift_home_and_shift_end_extend_to_the_first_and_last_item() {
        let it = sparse();
        let mut s = Selection::default();
        s.only(&id("d"));
        s.move_cursor(Dir::Home, true, &it);
        // the rectangle between d (1,1) and a (0,0): a, b, d
        assert_eq!(names(&s.selected), ["a", "b", "d"]);
        assert_eq!(s.anchor, Some(id("d")));
        s.only(&id("d"));
        s.move_cursor(Dir::End, true, &it);
        // d (1,1) .. g (3,2): d, f, g
        assert_eq!(names(&s.selected), ["d", "f", "g"]);
    }

    #[test]
    fn the_rubber_band_selects_what_it_touches_and_adds_with_a_modifier() {
        let it = sparse();
        let mut s = Selection::default();
        // from the middle of cell (0,0) to the middle of cell (1,1): touches a, b, d
        let band = rect_of((30, 30), (110, 110));
        s.rubber_band(&BTreeSet::new(), band, &it, false);
        assert_eq!(names(&s.selected), ["a", "b", "d"]);
        // shrink: only a
        s.rubber_band(&BTreeSet::new(), rect_of((30, 30), (40, 40)), &it, false);
        assert_eq!(names(&s.selected), ["a"]);
        // additive keeps the base
        let base: BTreeSet<ItemId> = [id("g")].into_iter().collect();
        s.rubber_band(&base, rect_of((30, 30), (40, 40)), &it, true);
        assert_eq!(names(&s.selected), ["a", "g"]);
        // a band in the gap between two icons (inside the 10 px margin) touches nothing
        s.rubber_band(&BTreeSet::new(), rect_of((70, 70), (79, 79)), &it, false);
        assert!(s.selected.is_empty());
        // an empty rectangle selects nothing
        assert!(band_hits(&it, Rect::default()).is_empty());
        // reversed corners make the same rectangle
        assert_eq!(rect_of((110, 110), (30, 30)), rect_of((30, 30), (110, 110)));
    }

    #[test]
    fn arrows_move_the_selection_and_shift_arrows_extend_it() {
        let it = sparse();
        let mut s = Selection::default();
        s.move_cursor(Dir::Down, false, &it);
        assert_eq!(names(&s.selected), ["a"]);
        s.move_cursor(Dir::Down, false, &it);
        assert_eq!(names(&s.selected), ["b"]);
        s.move_cursor(Dir::Right, false, &it);
        assert_eq!(names(&s.selected), ["d"]);
        // extend from d (anchor d) to the right: e (2,0): rectangle cols 1..=2, rows 0..=1 -> d, e
        s.move_cursor(Dir::Right, true, &it);
        assert_eq!(names(&s.selected), ["d", "e"]);
        assert_eq!(s.anchor, Some(id("d")));
        // at the end of the line, nothing moves
        s.only(&id("g"));
        s.move_cursor(Dir::Right, false, &it);
        assert_eq!(names(&s.selected), ["g"]);
        // a cursor with an empty selection (after Escape) is selected again by an arrow that has nowhere to go
        s.clear();
        s.move_cursor(Dir::Right, false, &it);
        assert_eq!(names(&s.selected), ["g"]);
    }

    #[test]
    fn items_that_vanish_leave_and_renamed_items_stay() {
        let it = sparse();
        let mut s = Selection::default();
        s.select_all(&it);
        s.cursor = Some(id("c"));
        let fewer: Vec<Placed> = it.iter().filter(|p| p.id != id("c")).cloned().collect();
        s.retain_existing(&fewer);
        assert_eq!(s.selected.len(), 6);
        assert_eq!(s.cursor, None);
        s.only(&id("a"));
        s.renamed(&id("a"), &id("a2"));
        assert_eq!(names(&s.selected), ["a2"]);
        assert_eq!((s.cursor, s.anchor), (Some(id("a2")), Some(id("a2"))));
    }
}
