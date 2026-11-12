//! Where an item that was just made, pasted or dropped goes: next to the pointer, as on Explorer's desktop. Pure.
//!
//! `Store::resolve` gives a NEW item the first free cell (top-left, down the first column), which is right for a file that appeared
//! by itself (another program made it) and wrong for one the user asked for with the mouse: "New folder" from a right click in the
//! middle of the screen, a paste there, a file dropped there. The app remembers the canvas point of that click or drop and, once the
//! item shows up in the model, calls [`place_near`]: it moves the new items next to that point with the rules of an icon the user
//! put down (`Store::drop_item`: with "align to grid" on the nearest FREE cell, with it off exactly there).
//!
//! With auto arrange on nothing is moved: the arrangement decides where everything goes, new items included.

use std::collections::HashSet;

use super::grid::{Cell, Grid};
use super::items::ItemId;
use super::layout::{MonitorSpec, Placement, Resolved};
use super::store::Store;

/// Moves `ids` (items that have just appeared, already placed by `resolve`) so that the first stands with its centre at `at` (monitor
/// pixels of monitor number `monitor`) and the others follow it down the column, then the next column, in the free cells. Returns
/// where each one went; empty when nothing was moved (auto arrange, no such monitor, nothing to move).
pub fn place_near(store: &mut Store, monitors: &[MonitorSpec], resolved: &mut Resolved, ids: &[ItemId], monitor: usize, at: (i32, i32)) -> Vec<(ItemId, Placement)> {
    let Some(m) = monitors.get(monitor) else { return Vec::new() };
    if store.auto_arrange || ids.is_empty() {
        return Vec::new();
    }
    let grid = Grid::new(m.work, store.metrics_at(m.scale_percent));
    if grid.is_empty() {
        return Vec::new();
    }
    let (w, h) = (grid.metrics.pitch_x as i32, grid.metrics.pitch_y as i32);
    // The arrivals give up the cells `resolve` gave them first, so that they can take each other's (the cell under the pointer may be
    // the first free one, where another arrival stands now). One that cannot be moved gets its old cell back.
    let old: Vec<(ItemId, Placement)> = ids.iter().filter_map(|i| resolved.placed.remove(i).map(|p| (i.clone(), p))).collect();
    let mut out: Vec<(ItemId, Placement)> = Vec::with_capacity(ids.len());
    let mut prev: Option<Cell> = None;
    for id in ids {
        let placed = match prev {
            None => store.drop_item(monitors, resolved, id, monitor, at.0 - w / 2, at.1 - h / 2),
            Some(p) => {
                let taken: HashSet<Cell> = resolved.placed.values().filter(|x| x.monitor == monitor).map(|x| x.cell).collect();
                // (the corner of a cell snaps to that cell, with no offset, whether or not icons align)
                grid.next_free_after(p, &taken).and_then(|next| {
                    let r = grid.cell_rect(next);
                    store.drop_item(monitors, resolved, id, monitor, r.left, r.top)
                })
            }
        };
        match placed {
            Some(p) => {
                prev = Some(p.cell);
                out.push((id.clone(), p));
            }
            None => {
                if let Some((_, p)) = old.iter().find(|(k, _)| k == id) {
                    resolved.placed.insert(id.clone(), *p);
                }
            }
        }
    }
    out
}

/// The monitor that a point lies on when the point is given in the pixels of monitor `from` and may be outside it (the pointer of a
/// drag that has left the window it started in): the other monitor's index and the point in THAT monitor's own pixels. `None` when
/// the point is on `from` itself, or on no monitor at all (a gap between monitors, past the edge of the virtual screen).
pub fn monitor_under(monitors: &[MonitorSpec], from: usize, p: (i32, i32)) -> Option<(usize, (i32, i32))> {
    let src = monitors.get(from)?;
    let (vx, vy) = (src.rect.left + p.0, src.rect.top + p.1);
    let to = monitors.iter().position(|m| m.rect.contains(vx, vy))?;
    (to != from).then(|| (to, (vx - monitors[to].rect.left, vy - monitors[to].rect.top)))
}

/// Moves icons to another monitor: each lands where its own top-left corner (`corner`, in the pixels of monitor `from`) plus the
/// drag's `delta` falls on monitor `to`, by the rules of an icon the user put down (`Store::drop_item`). Only the layout changes, so it
/// is for every kind of icon, files and system icons, the user's and the Public Desktop's alike. The movers give up their cells first
/// (they may take each other's). Nothing moves with auto arrange on.
pub fn move_across(store: &mut Store, monitors: &[MonitorSpec], resolved: &mut Resolved, items: &[(ItemId, (i32, i32))], from: usize, delta: (i32, i32), to: usize) -> Vec<(ItemId, Placement)> {
    let (Some(src), Some(dst)) = (monitors.get(from), monitors.get(to)) else { return Vec::new() };
    if store.auto_arrange || from == to {
        return Vec::new();
    }
    let old: Vec<(ItemId, Placement)> = items.iter().filter_map(|(i, _)| resolved.placed.remove(i).map(|p| (i.clone(), p))).collect();
    let mut out = Vec::new();
    for (id, corner) in items {
        let (x, y) = (src.rect.left + corner.0 + delta.0 - dst.rect.left, src.rect.top + corner.1 + delta.1 - dst.rect.top);
        match store.drop_item(monitors, resolved, id, to, x, y) {
            Some(p) => out.push((id.clone(), p)),
            None => {
                if let Some((_, p)) = old.iter().find(|(k, _)| k == id) {
                    resolved.placed.insert(id.clone(), *p);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::geom::{Rect, Size};

    /// 4 columns x 3 rows of the bare 75-pixel grid at 100 % (medium icons).
    fn screen() -> MonitorSpec {
        MonitorSpec { device: "A".into(), size: Size::new(300, 225), scale_percent: 100, work: Rect::new(0, 0, 300, 225), primary: true, rect: Rect::from_size(0, 0, 300, 225) }
    }
    fn ids(names: &[&str]) -> Vec<ItemId> {
        names.iter().map(|n| ItemId::user(n)).collect()
    }
    fn at(r: &Resolved, n: &str) -> (u32, u32, i32, i32) {
        let p = r.placed[&ItemId::user(n)];
        (p.cell.col, p.cell.row, p.dx, p.dy)
    }

    /// Three old items in the first column and a new one that `resolve` put in the first free cell.
    fn desktop() -> (Store, Vec<MonitorSpec>, Resolved) {
        let mut s = Store::default();
        let m = vec![screen()];
        s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        let r = s.resolve(&m, &ids(&["a", "b", "c", "new"]), 1);
        assert_eq!(at(&r, "new"), (1, 0, 0, 0), "the first free cell, where a file made by another program stays");
        (s, m, r)
    }

    #[test]
    fn a_new_item_goes_to_the_cell_under_the_pointer() {
        let (mut s, m, mut r) = desktop();
        // the pointer is in the middle of column 3, row 2 (pixels 225..300, 150..225)
        let moved = place_near(&mut s, &m, &mut r, &ids(&["new"]), 0, (262, 187));
        assert_eq!(at(&r, "new"), (3, 2, 0, 0));
        assert_eq!(moved.len(), 1);
        // the others did not move
        assert_eq!(at(&r, "a"), (0, 0, 0, 0));
        // and it is recorded: the next resolve keeps it there
        let again = s.resolve(&m, &ids(&["a", "b", "c", "new"]), 2);
        assert_eq!(at(&again, "new"), (3, 2, 0, 0));
    }

    #[test]
    fn a_taken_cell_sends_it_to_the_nearest_free_one() {
        let (mut s, m, mut r) = desktop();
        // the pointer is on item "b" (column 0, row 1)
        place_near(&mut s, &m, &mut r, &ids(&["new"]), 0, (37, 112));
        assert_eq!(at(&r, "new"), (1, 1, 0, 0), "the free cell beside b, not on top of it and not across the screen");
        assert_eq!(at(&r, "b"), (0, 1, 0, 0));
    }

    #[test]
    fn with_align_off_it_stands_exactly_where_the_pointer_put_it() {
        let (mut s, m, _) = desktop();
        s.set_align_to_grid(false);
        let mut r = s.resolve(&m, &ids(&["a", "b", "c", "new"]), 3);
        place_near(&mut s, &m, &mut r, &ids(&["new"]), 0, (200, 100));
        // the centre at (200, 100): the corner is (200 - 37, 100 - 37) = (163, 63): column 2, row 0, offset (13, 63) inside the cell
        assert_eq!(at(&r, "new"), (2, 0, 13, 63));
    }

    #[test]
    fn auto_arrange_decides_and_nothing_is_moved() {
        let (mut s, m, _) = desktop();
        s.set_auto_arrange(true);
        let mut r = s.resolve(&m, &ids(&["a", "b", "c", "new"]), 4);
        let before = r.clone();
        let stored = s.clone();
        assert!(place_near(&mut s, &m, &mut r, &ids(&["new"]), 0, (262, 187)).is_empty());
        assert_eq!(r, before);
        assert_eq!(s, stored, "the store is not touched either");
    }

    #[test]
    fn several_arrivals_follow_each_other_down_the_column_and_then_across() {
        let mut s = Store::default();
        let m = vec![screen()];
        s.resolve(&m, &ids(&["old"]), 0);
        let mut r = s.resolve(&m, &ids(&["old", "n1", "n2", "n3", "n4"]), 1);
        // `resolve` put them in (0,1) (0,2) (1,0) (1,1). Dropped in the middle of column 1, row 1: n1 there, n2 below it, n3 at the top
        // of the next column, n4 under n3 - although the cell under the pointer was n4's a moment ago
        let moved = place_near(&mut s, &m, &mut r, &ids(&["n1", "n2", "n3", "n4"]), 0, (112, 112));
        assert_eq!(moved.len(), 4);
        assert_eq!(at(&r, "n1"), (1, 1, 0, 0));
        assert_eq!(at(&r, "n2"), (1, 2, 0, 0));
        assert_eq!(at(&r, "n3"), (2, 0, 0, 0));
        assert_eq!(at(&r, "n4"), (2, 1, 0, 0));
        assert_eq!(at(&r, "old"), (0, 0, 0, 0));
        let cells: HashSet<(u32, u32)> = ["old", "n1", "n2", "n3", "n4"].iter().map(|n| (at(&r, n).0, at(&r, n).1)).collect();
        assert_eq!(cells.len(), 5, "no two on one cell");
    }

    #[test]
    fn nothing_to_move_or_no_such_monitor_changes_nothing() {
        let (mut s, m, mut r) = desktop();
        let before = r.clone();
        assert!(place_near(&mut s, &m, &mut r, &[], 0, (10, 10)).is_empty());
        assert!(place_near(&mut s, &m, &mut r, &ids(&["new"]), 5, (10, 10)).is_empty());
        assert_eq!(r, before);
    }

    /// Two monitors side by side: A (primary, 300 x 225) and B to its right (225 x 150, a different size), and a gap of nothing above B.
    fn two() -> Vec<MonitorSpec> {
        let a = screen();
        let b = MonitorSpec { device: "B".into(), size: Size::new(225, 150), scale_percent: 100, work: Rect::new(0, 0, 225, 150), primary: false, rect: Rect::from_size(300, 0, 225, 150) };
        vec![a, b]
    }

    #[test]
    fn a_point_past_the_edge_of_one_monitor_is_found_on_the_next_in_its_own_pixels() {
        let m = two();
        assert_eq!(monitor_under(&m, 0, (310, 20)), Some((1, (10, 20))));
        assert_eq!(monitor_under(&m, 0, (100, 100)), None, "still on the same monitor");
        assert_eq!(monitor_under(&m, 0, (310, 200)), None, "below B: on no monitor");
        assert_eq!(monitor_under(&m, 0, (-5, 10)), None, "left of the virtual screen");
        assert_eq!(monitor_under(&m, 1, (-20, 30)), Some((0, (280, 30))), "and back again");
        assert_eq!(monitor_under(&m, 5, (0, 0)), None);
    }

    #[test]
    fn icons_of_any_kind_move_to_the_other_monitor_by_their_own_corner() {
        let m = two();
        let mut s = Store::default();
        // a user file, a Public Desktop file and This PC on the first monitor
        let ids = vec![ItemId::user("doc"), ItemId::public("shared.lnk"), ItemId::virtual_item(crate::desktop::virtuals::VirtualKind::ThisPc)];
        let mut r = s.resolve(&m, &ids, 0);
        assert!(ids.iter().all(|i| r.placed[i].monitor == 0));
        // the three are dragged together 330 pixels to the right (onto monitor B) and 10 down
        let corners: Vec<(ItemId, (i32, i32))> = ids.iter().map(|i| (i.clone(), (r.placed[i].cell.col as i32 * 75, r.placed[i].cell.row as i32 * 75))).collect();
        let moved = move_across(&mut s, &m, &mut r, &corners, 0, (330, 10), 1);
        assert_eq!(moved.len(), 3);
        for i in &ids {
            assert_eq!(r.placed[i].monitor, 1, "{i:?} is on the second monitor");
        }
        // they keep their order and stand on distinct cells
        let cells: HashSet<Cell> = ids.iter().map(|i| r.placed[i].cell).collect();
        assert_eq!(cells.len(), 3);
        // and it is recorded: after the next resolve they are still there
        let again = s.resolve(&m, &ids, 1);
        assert!(ids.iter().all(|i| again.placed[i].monitor == 1));
    }

    #[test]
    fn nothing_crosses_with_auto_arrange_on_or_to_the_same_or_a_missing_monitor() {
        let m = two();
        let mut s = Store::default();
        let ids = ids(&["a"]);
        let mut r = s.resolve(&m, &ids, 0);
        let before = r.clone();
        assert!(move_across(&mut s, &m, &mut r, &[(ids[0].clone(), (0, 0))], 0, (330, 0), 0).is_empty());
        assert!(move_across(&mut s, &m, &mut r, &[(ids[0].clone(), (0, 0))], 0, (330, 0), 9).is_empty());
        s.set_auto_arrange(true);
        let mut ra = s.resolve(&m, &ids, 1);
        let before_auto = ra.clone();
        assert!(move_across(&mut s, &m, &mut ra, &[(ids[0].clone(), (0, 0))], 0, (330, 0), 1).is_empty());
        assert_eq!(ra, before_auto);
        assert_eq!(r, before);
    }

    #[test]
    fn a_pointer_outside_the_work_area_lands_on_the_nearest_edge_cell() {
        let (mut s, m, mut r) = desktop();
        place_near(&mut s, &m, &mut r, &ids(&["new"]), 0, (9000, 9000));
        assert_eq!(at(&r, "new"), (3, 2, 0, 0));
    }
}
