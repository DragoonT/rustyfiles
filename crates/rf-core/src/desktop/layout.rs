//! The rules that give every desktop item a cell on the monitors that are connected now (shell plan F2). Pure: it works on the
//! [`Store`] and plain monitor descriptions.
//!
//! THE RULES, in the order [`Store::resolve`] applies them:
//! 1. A monitor's layout is the one stored for its device path and exact `"{w}x{h}@{scale}"`. With none, the stored layout of the
//!    SAME monitor whose pixel count is nearest (then nearest scale) is used as a starting point and copied; the original stays in
//!    the store untouched, so going back to that resolution brings every icon back to its old cell.
//! 2. An item stored on two connected monitors' layouts belongs to the one with the larger `at` (the more recent placement; the
//!    primary monitor on a tie). Layouts of monitors that are not connected are only memory, they claim nothing.
//! 3. A claimed cell outside the grid of the new resolution (or of a work area that shrank because a dock appeared) is not used: the
//!    item is shown somewhere else for now, on the same monitor if it has room. THE CLAIM IS NOT TOUCHED: the shown place is a
//!    separate, provisional record, so the item goes back to its cell as soon as the grid has it again.
//! 4. Two items claiming one cell (with "align to grid" on; with it off, one cell AND offset): the one with the smaller `at` keeps it,
//!    the later one is shown at the next free cell after it in column-major order (provisionally, as in 3).
//! 5. Items nobody claims are placed fresh: the first free cell in column-major order (down the first column, then right) on the
//!    primary monitor, then on the next monitor from the left when that one is full. A recently deleted item that comes back (a
//!    restore from the Recycle Bin) gets its old cell first, if that is free. An item whose only claim is on a monitor that is not
//!    connected right now is shown provisionally on a connected one, and returns to its monitor when that one is back.
//! 6. Items of the exact layout that are no longer on the desktop are forgotten into `removed` for a day (their cells are free) BUT
//!    only when the listing is complete ([`Listing`]); items that exist and are merely not drawn (hidden files while "show hidden" is
//!    off, virtual items the user turned off, the whole desktop switched off) keep their cell for good, and an item that did not fit
//!    anywhere keeps its cell too.
//! 7. Auto arrange ignores all of it and lays the items out in the order given; the store is not changed.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use super::geom::{Rect, Size};
use super::grid::{Cell, Grid, IconSize, metrics_spaced};
use super::items::ItemId;
use super::store::{MonitorLayout, Removed, Slot, Store};

/// How long a deleted item's place is remembered.
pub const REMOVED_GRACE_MS: i64 = 24 * 60 * 60 * 1000;

/// What the desktop holds, as `resolve` needs to know it. The three parts keep three different situations apart, because an item that
/// is not drawn is not an item that is gone:
/// * `shown`: the items to place, in the order new ones are placed in;
/// * `kept`: items that exist but are not drawn (hidden by the "show hidden" settings, a virtual item the user switched off, the
///   desktop switched off): their cells are remembered, they take none now;
/// * `complete`: the folders were listed successfully. With `false` (a failed listing, a folder not found yet) nothing is
///   forgotten at all: absence from an incomplete list says nothing.
#[derive(Debug, Clone, Copy)]
pub struct Listing<'a> {
    pub shown: &'a [ItemId],
    pub kept: &'a [ItemId],
    pub complete: bool,
}

impl<'a> Listing<'a> {
    /// Every item that exists is in `shown`; whatever is not in it is gone.
    pub fn complete(shown: &'a [ItemId]) -> Self {
        Listing { shown, kept: &[], complete: true }
    }
    /// The listing may be missing items: nothing is forgotten.
    pub fn incomplete(shown: &'a [ItemId]) -> Self {
        Listing { shown, kept: &[], complete: false }
    }
    pub fn with_kept(mut self, kept: &'a [ItemId]) -> Self {
        self.kept = kept;
        self
    }
}

/// [`Listing`] with the lists owned (what `DesktopModel::listing` returns).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnedListing {
    pub shown: Vec<ItemId>,
    pub kept: Vec<ItemId>,
    pub complete: bool,
}

impl OwnedListing {
    pub fn as_listing(&self) -> Listing<'_> {
        Listing { shown: &self.shown, kept: &self.kept, complete: self.complete }
    }
}

impl<'a> From<&'a OwnedListing> for Listing<'a> {
    fn from(l: &'a OwnedListing) -> Self {
        l.as_listing()
    }
}

/// A bare list is a complete listing. Only for tests: the app always says what it knows.
#[cfg(test)]
impl<'a> From<&'a Vec<ItemId>> for Listing<'a> {
    fn from(v: &'a Vec<ItemId>) -> Self {
        Listing::complete(v)
    }
}

/// One connected monitor, as the layout needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorSpec {
    /// The stable device path.
    pub device: String,
    /// The monitor's size in physical pixels.
    pub size: Size,
    /// Display scale in percent (100, 125, 150, 200 ...).
    pub scale_percent: u32,
    /// The part icons may use, in the monitor's own coordinates (the work area: not under the dock).
    pub work: Rect,
    pub primary: bool,
    /// The monitor in virtual-screen coordinates (for ordering left to right, and for importing positions).
    pub rect: Rect,
}

impl MonitorSpec {
    pub fn layout_key(&self) -> String {
        layout_key(self.size.w, self.size.h, self.scale_percent)
    }
}

pub fn layout_key(w: u32, h: u32, scale_percent: u32) -> String {
    format!("{w}x{h}@{scale_percent}")
}

/// `"1920x1080@125"` -> (1920, 1080, 125).
pub fn parse_layout_key(k: &str) -> Option<(u32, u32, u32)> {
    let (res, pct) = k.split_once('@')?;
    let (w, h) = res.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?, pct.parse().ok()?))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Index into the monitor list that was given.
    pub monitor: usize,
    pub cell: Cell,
    pub dx: i32,
    pub dy: i32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resolved {
    pub placed: BTreeMap<ItemId, Placement>,
    /// Items that did not fit anywhere (every grid is full). Their stored cells are kept.
    pub unplaced: Vec<ItemId>,
    /// Items that got a cell now because they had none, or could not have theirs (new, outside the grid, collision, their monitor
    /// away) and were not already shown somewhere provisionally.
    pub fresh: Vec<ItemId>,
    /// Monitors whose layout was started from another resolution's.
    pub inherited: Vec<usize>,
}

/// Primary first, then left to right, top to bottom.
fn monitor_order(monitors: &[MonitorSpec]) -> Vec<usize> {
    let mut o: Vec<usize> = (0..monitors.len()).collect();
    o.sort_by_key(|&i| (!monitors[i].primary, monitors[i].rect.left, monitors[i].rect.top, i));
    o
}

/// Cells taken on each monitor while placing.
struct Board<'a> {
    grids: Vec<Grid>,
    order: Vec<usize>,
    taken: Vec<HashSet<Cell>>,
    exact: Vec<HashSet<(Cell, i32, i32)>>,
    align: bool,
    placed: &'a mut BTreeMap<ItemId, Placement>,
}

impl Board<'_> {
    fn free_for_claim(&self, m: usize, c: Cell, dx: i32, dy: i32) -> bool {
        if self.align { !self.taken[m].contains(&c) } else { !self.exact[m].contains(&(c, dx, dy)) }
    }
    fn put(&mut self, id: &ItemId, m: usize, c: Cell, dx: i32, dy: i32) {
        let (dx, dy) = if self.align { (0, 0) } else { (dx, dy) };
        self.taken[m].insert(c);
        self.exact[m].insert((c, dx, dy));
        self.placed.insert(id.clone(), Placement { monitor: m, cell: c, dx, dy });
    }
    /// First free cell on `m`, else on the other monitors in order.
    fn put_first_free(&mut self, id: &ItemId, prefer: Option<usize>) -> bool {
        let mut tries: Vec<usize> = prefer.into_iter().collect();
        tries.extend(self.order.iter().copied().filter(|m| Some(*m) != prefer));
        for m in tries {
            if let Some(c) = self.grids[m].first_free(&self.taken[m]) {
                self.put(id, m, c, 0, 0);
                return true;
            }
        }
        false
    }
}

struct Claim {
    id: ItemId,
    monitor: usize,
    slot: Slot,
}

/// An item that is shown somewhere other than its claimed cell, or that has no connected claim at all.
struct Displaced {
    id: ItemId,
    /// The `at` of the claim (0 for an item whose claim is on a monitor that is not connected): the order they are dealt with in.
    at: u64,
    /// The monitor of the claim, when it is connected.
    monitor: Option<usize>,
    /// The cell it wanted and found taken (a collision); `None`: outside the grid, or no connected claim.
    wanted: Option<Cell>,
}

/// Whether a deleted item's memory is still good.
fn recent(now_ms: i64, gone_ms: i64) -> bool {
    now_ms.saturating_sub(gone_ms) <= REMOVED_GRACE_MS
}

impl Store {
    /// The stored layout to start a monitor from: (layout, was it another resolution's).
    fn source_layout(&self, m: &MonitorSpec) -> Option<(MonitorLayout, bool)> {
        let by_key = self.layouts.get(&m.device)?;
        let key = m.layout_key();
        if let Some(l) = by_key.get(&key) {
            return Some((l.clone(), false));
        }
        let area = m.size.w as i64 * m.size.h as i64;
        by_key
            .iter()
            .filter_map(|(k, l)| parse_layout_key(k).map(|(w, h, p)| ((w as i64 * h as i64 - area).abs(), (p as i64 - m.scale_percent as i64).abs(), k, l)))
            .min_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
            .map(|(_, _, _, l)| (l.clone(), true))
    }

    /// Gives every item in the listing's `shown` a cell. `shown` is in the order new items are placed in (for a first start: the sort
    /// order; afterwards: the order they appeared in). The store is updated and marked dirty if anything changed; the result is what to
    /// draw. What the listing says about the items it does NOT show decides what is forgotten, see [`Listing`].
    pub fn resolve<'a>(&mut self, monitors: &[MonitorSpec], listing: impl Into<Listing<'a>>, now_ms: i64) -> Resolved {
        let listing: Listing<'a> = listing.into();
        let items = listing.shown;
        let mut out = Resolved::default();
        if monitors.is_empty() {
            out.unplaced = items.to_vec();
            return out;
        }
        let grids: Vec<Grid> = monitors.iter().map(|m| Grid::new(m.work, self.metrics_at(m.scale_percent))).collect();
        let order = monitor_order(monitors);
        let n = monitors.len();
        let align = self.align_to_grid;
        let mut placed = BTreeMap::new();
        let mut board = Board { grids, order: order.clone(), taken: vec![HashSet::new(); n], exact: vec![HashSet::new(); n], align, placed: &mut placed };

        if self.auto_arrange {
            for id in items {
                if !board.put_first_free(id, None) {
                    out.unplaced.push(id.clone());
                }
            }
            drop(board);
            out.placed = placed;
            return out;
        }

        let shown: HashSet<&str> = items.iter().map(|i| i.as_str()).collect();
        let kept: HashSet<&str> = shown.iter().copied().chain(listing.kept.iter().map(|i| i.as_str())).collect();

        // 1. the starting layouts
        let sources: Vec<Option<(MonitorLayout, bool)>> = monitors.iter().map(|m| self.source_layout(m)).collect();
        for (i, s) in sources.iter().enumerate() {
            if matches!(s, Some((_, true))) {
                out.inherited.push(i);
            }
        }

        // 2. claims, one per item: the most recent placement among the connected monitors
        let mut best: HashMap<&str, Claim> = HashMap::new();
        for &mi in &order {
            let Some((layout, _)) = &sources[mi] else { continue };
            for (key, slot) in &layout.items {
                let Some(&id) = shown.get(key.as_str()) else { continue };
                if best.get(id).is_none_or(|c| slot.at > c.slot.at) {
                    best.insert(id, Claim { id: ItemId::from_stored(id), monitor: mi, slot: *slot });
                }
            }
        }
        let winner: HashMap<String, usize> = best.iter().map(|(k, c)| ((*k).to_string(), c.monitor)).collect();

        // 3. honour the claims monitor by monitor, the older placement first; the rest are displaced
        let mut by_monitor: Vec<Vec<Claim>> = (0..n).map(|_| Vec::new()).collect();
        for (_, c) in best {
            by_monitor[c.monitor].push(c);
        }
        let mut displaced: Vec<Displaced> = Vec::new();
        for &mi in &order {
            let claims = &mut by_monitor[mi];
            claims.sort_by(|a, b| (a.slot.at, a.id.as_str()).cmp(&(b.slot.at, b.id.as_str())));
            for c in claims.drain(..) {
                let cell = Cell::new(c.slot.col, c.slot.row);
                if !board.grids[mi].contains(cell) {
                    displaced.push(Displaced { id: c.id, at: c.slot.at, monitor: Some(mi), wanted: None });
                } else if board.free_for_claim(mi, cell, c.slot.dx, c.slot.dy) {
                    board.put(&c.id, mi, cell, c.slot.dx, c.slot.dy);
                } else {
                    displaced.push(Displaced { id: c.id, at: c.slot.at, monitor: Some(mi), wanted: Some(cell) });
                }
            }
        }

        // 5a. items nobody claims: a restore (a deleted item came back) gets its old cell first, if it is free
        let claimed: HashSet<&str> = winner.keys().map(|k| k.as_str()).collect();
        let mut newcomers: Vec<&ItemId> = items.iter().filter(|i| !claimed.contains(i.as_str())).collect();
        // real placements made now (new and restored items): the only ones that are written as claims
        let mut new_real: BTreeSet<ItemId> = BTreeSet::new();
        let mut restored: Vec<(&ItemId, usize, Removed)> = Vec::new();
        for &id in &newcomers {
            let mut best_r: Option<(usize, Removed)> = None;
            for &mi in &order {
                let Some((layout, _)) = &sources[mi] else { continue };
                if let Some(r) = layout.removed.get(id.as_str())
                    && recent(now_ms, r.gone_ms)
                    && best_r.is_none_or(|(_, b)| r.at > b.at)
                {
                    best_r = Some((mi, *r));
                }
            }
            if let Some((mi, r)) = best_r {
                restored.push((id, mi, r));
            }
        }
        restored.sort_by(|a, b| (a.2.at, a.0).cmp(&(b.2.at, b.0)));
        let mut restored_ids: HashSet<ItemId> = HashSet::new();
        for (id, mi, r) in restored {
            let cell = Cell::new(r.col, r.row);
            if board.grids[mi].contains(cell) && board.free_for_claim(mi, cell, r.dx, r.dy) {
                board.put(id, mi, cell, r.dx, r.dy);
                restored_ids.insert(id.clone());
                new_real.insert(id.clone());
            }
        }
        newcomers.retain(|i| !restored_ids.contains(*i));

        // newcomers whose claim lives on a monitor that is not connected: the monitor may only be asleep, so what they get now is
        // provisional and the claim stays where it is
        let connected: HashSet<&str> = monitors.iter().map(|m| m.device.as_str()).collect();
        let away: HashSet<&str> = self
            .layouts
            .iter()
            .filter(|(device, _)| !connected.contains(device.as_str()))
            .flat_map(|(_, by_key)| by_key.values())
            .flat_map(|l| l.items.keys().map(|k| k.as_str()))
            .collect();
        let mut fresh_ids: Vec<&ItemId> = Vec::new();
        for id in newcomers {
            if away.contains(id.as_str()) {
                displaced.push(Displaced { id: id.clone(), at: 0, monitor: None, wanted: None });
            } else {
                fresh_ids.push(id);
            }
        }

        // 4. the displaced: where they were shown last time if that still works, else the next free cell after the contested one
        //    (a collision), else the first free cell (same monitor first)
        displaced.sort_by(|a, b| (a.at, a.id.as_str()).cmp(&(b.at, b.id.as_str())));
        let mut provisional_out: Vec<BTreeMap<String, Slot>> = (0..n).map(|_| BTreeMap::new()).collect();
        for d in displaced {
            let mut done = false;
            let mut tries: Vec<usize> = d.monitor.into_iter().collect();
            tries.extend(order.iter().copied().filter(|m| Some(*m) != d.monitor));
            for lm in tries {
                // only the exact layout of that monitor says where an item was shown (a copy borrowed from another resolution says nothing)
                let Some((layout, false)) = &sources[lm] else { continue };
                let Some(s) = layout.provisional.get(d.id.as_str()) else { continue };
                let cell = Cell::new(s.col, s.row);
                if board.grids[lm].contains(cell) && board.free_for_claim(lm, cell, s.dx, s.dy) {
                    board.put(&d.id, lm, cell, s.dx, s.dy);
                    done = true;
                    break;
                }
            }
            if !done {
                done = match (d.wanted, d.monitor) {
                    (Some(cell), Some(mi)) => match board.grids[mi].next_free_after(cell, &board.taken[mi]) {
                        Some(free) => {
                            board.put(&d.id, mi, free, 0, 0);
                            true
                        }
                        None => board.put_first_free(&d.id, Some(mi)),
                    },
                    _ => board.put_first_free(&d.id, d.monitor),
                };
                if done {
                    out.fresh.push(d.id.clone());
                }
            }
            let at = board.placed.get(&d.id).copied().filter(|_| done);
            match at {
                Some(p) => {
                    provisional_out[p.monitor].insert(d.id.as_str().to_string(), Slot { col: p.cell.col, row: p.cell.row, dx: p.dx, dy: p.dy, at: 0 });
                }
                None => out.unplaced.push(d.id),
            }
        }

        // 5b. new items take the first free cells
        for id in fresh_ids {
            if board.put_first_free(id, None) {
                out.fresh.push(id.clone());
                new_real.insert(id.clone());
            } else {
                out.unplaced.push(id.clone());
            }
        }
        for id in &restored_ids {
            out.fresh.push(id.clone());
        }
        drop(board);
        out.placed = placed;

        // 6. write back: the exact layout of every connected monitor. A stored claim is carried over as it is, whatever happened to the
        //    item on the screen; only new and restored items get a claim now.
        let mut changed = false;
        for (mi, m) in monitors.iter().enumerate() {
            let key = m.layout_key();
            let old = self.layouts.get(&m.device).and_then(|k| k.get(&key)).cloned();
            let (src_layout, inherited) = sources[mi].clone().unwrap_or_default();
            let mut layout = MonitorLayout::default();
            for (k, s) in &src_layout.items {
                if shown.contains(k.as_str()) {
                    // an item claimed on two connected monitors stays with the more recent claim
                    if winner.get(k) == Some(&mi) {
                        layout.items.insert(k.clone(), *s);
                    }
                } else if !listing.complete || kept.contains(k.as_str()) {
                    layout.items.insert(k.clone(), *s); // not drawn, or not known to be gone: its cell stays
                } else if !inherited {
                    // gone from a complete listing: forgotten for a day (only from this resolution's own layout: one borrowed from
                    // another resolution says nothing about what was deleted here)
                    layout.removed.insert(k.clone(), Removed { col: s.col, row: s.row, dx: s.dx, dy: s.dy, at: s.at, gone_ms: now_ms });
                }
            }
            for id in &new_real {
                if let Some(p) = out.placed.get(id).filter(|p| p.monitor == mi) {
                    let at = self.tick();
                    layout.items.insert(id.as_str().to_string(), Slot { col: p.cell.col, row: p.cell.row, dx: p.dx, dy: p.dy, at });
                }
            }
            // memory: tombstones carried over (not the ones restored now, not the expired). A date in the future (a clock set back, a
            // hand-edited file) counts as now, so that it expires like any other.
            for (k, r) in &src_layout.removed {
                if recent(now_ms, r.gone_ms) && !kept.contains(k.as_str()) && !layout.removed.contains_key(k) {
                    layout.removed.insert(k.clone(), Removed { gone_ms: r.gone_ms.min(now_ms), ..*r });
                }
            }
            layout.provisional = std::mem::take(&mut provisional_out[mi]);
            if old.is_none() && layout == MonitorLayout::default() {
                continue;
            }
            if old.as_ref() != Some(&layout) {
                changed = true;
                self.layouts.entry(m.device.clone()).or_default().insert(key, layout);
            }
        }
        if changed {
            self.mark();
        }
        out
    }

    /// The user dropped `id` at the corner (x, y) of monitor `to` (monitor pixels). Snaps, avoids a taken cell with "align to grid" on
    /// (the nearest free one), records it, and removes the item from the other connected monitors' layouts. Returns where it went.
    pub fn drop_item(&mut self, monitors: &[MonitorSpec], resolved: &mut Resolved, id: &ItemId, to: usize, x: i32, y: i32) -> Option<Placement> {
        let m = monitors.get(to)?;
        let grid = Grid::new(m.work, self.metrics_at(m.scale_percent));
        let (cell, dx, dy) = grid.snap(x, y, self.align_to_grid)?;
        let taken: HashSet<Cell> = resolved.placed.iter().filter(|(k, p)| *k != id && p.monitor == to).map(|(_, p)| p.cell).collect();
        let (cell, dx, dy) = if self.align_to_grid && taken.contains(&cell) { (grid.nearest_free(cell, &taken)?, 0, 0) } else { (cell, dx, dy) };
        let placement = Placement { monitor: to, cell, dx, dy };
        let at = self.tick();
        for (mi, other) in monitors.iter().enumerate() {
            if mi != to
                && let Some(l) = self.layouts.get_mut(&other.device).and_then(|k| k.get_mut(&other.layout_key()))
            {
                l.items.remove(id.as_str());
            }
        }
        // the user's own placement ends any provisional one, wherever it was shown
        for l in self.layouts.values_mut().flat_map(|by_key| by_key.values_mut()) {
            l.provisional.remove(id.as_str());
        }
        let layout = self.layouts.entry(m.device.clone()).or_default().entry(m.layout_key()).or_default();
        layout.items.insert(id.as_str().to_string(), Slot { col: cell.col, row: cell.row, dx, dy, at });
        layout.removed.remove(id.as_str());
        resolved.placed.insert(id.clone(), placement);
        Some(placement)
    }

    /// A rename keeps the cell: the record moves to the new id in every layout (and in the memory of removed items).
    pub fn rename(&mut self, from: &ItemId, to: &ItemId) {
        if from == to {
            return;
        }
        let (f, t) = (from.as_str(), to.as_str());
        for by_key in self.layouts.values_mut() {
            for l in by_key.values_mut() {
                if let Some(s) = l.items.remove(f) {
                    l.items.insert(t.to_string(), s);
                    self.dirty = true;
                }
                if let Some(s) = l.provisional.remove(f) {
                    l.provisional.insert(t.to_string(), s);
                    self.dirty = true;
                }
                if let Some(r) = l.removed.remove(f) {
                    l.removed.insert(t.to_string(), r);
                    self.dirty = true;
                }
            }
        }
    }

    /// The item is gone: its cell is free from now on, and remembered for a day.
    pub fn remove(&mut self, id: &ItemId, now_ms: i64) {
        let k = id.as_str();
        for by_key in self.layouts.values_mut() {
            for l in by_key.values_mut() {
                if l.provisional.remove(k).is_some() {
                    self.dirty = true;
                }
                if let Some(s) = l.items.remove(k) {
                    l.removed.insert(k.to_string(), Removed { col: s.col, row: s.row, dx: s.dx, dy: s.dy, at: s.at, gone_ms: now_ms });
                    self.dirty = true;
                }
            }
        }
    }

    /// A new icon size: every stored cell is mapped to the cell that holds its centre at the new size, so icons stay about where they
    /// are on the screen. Two that land in one cell are told apart by `resolve` (the later one is shown elsewhere).
    pub fn set_icon_size(&mut self, size: IconSize) {
        let old = self.icon_size_step();
        if old == size && self.icon_size == size.px() {
            return;
        }
        let spacing = self.spacing;
        for by_key in self.layouts.values_mut() {
            for (key, l) in by_key.iter_mut() {
                let pct = parse_layout_key(key).map_or(100, |(_, _, p)| p);
                let (a, b) = (metrics_spaced(old, pct, spacing), metrics_spaced(size, pct, spacing));
                let remap = |c: u32, o: u32, n: u32| ((c as u64 * o as u64 + o as u64 / 2) / n.max(1) as u64) as u32;
                let off = |d: i32, o: u32, n: u32| (d as i64 * n as i64 / o.max(1) as i64) as i32;
                for s in l.items.values_mut().chain(l.provisional.values_mut()) {
                    *s = Slot { col: remap(s.col, a.pitch_x, b.pitch_x), row: remap(s.row, a.pitch_y, b.pitch_y), dx: off(s.dx, a.pitch_x, b.pitch_x), dy: off(s.dy, a.pitch_y, b.pitch_y), at: s.at };
                }
                for r in l.removed.values_mut() {
                    r.col = remap(r.col, a.pitch_x, b.pitch_x);
                    r.row = remap(r.row, a.pitch_y, b.pitch_y);
                }
            }
        }
        self.icon_size = size.px();
        self.mark();
    }

    /// Turning "auto arrange" off: the arrangement on the screen becomes the stored positions.
    pub fn freeze(&mut self, monitors: &[MonitorSpec], resolved: &Resolved) {
        let items: Vec<(ItemId, Placement)> = resolved.placed.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (id, p) in items {
            let Some(m) = monitors.get(p.monitor) else { continue };
            let at = self.tick();
            let l = self.layouts.entry(m.device.clone()).or_default().entry(m.layout_key()).or_default();
            l.items.insert(id.as_str().to_string(), Slot { col: p.cell.col, row: p.cell.row, dx: p.dx, dy: p.dy, at });
            l.provisional.remove(id.as_str());
        }
        self.auto_arrange = false;
        self.mark();
    }

    /// The one-time import of Explorer's positions (already converted to cells by `import::convert`): written into the exact layouts.
    /// It is ONE time: once `imported_from_explorer` is set, a second call changes nothing and returns `false`, so that a repeated
    /// click (or a replayed first-run path) cannot overwrite what the user has arranged since. `force` is for an explicit "import
    /// again" the user asked for.
    pub fn import_positions(&mut self, monitors: &[MonitorSpec], assignments: &[(ItemId, usize, Cell)], force: bool) -> bool {
        if self.imported_from_explorer && !force {
            return false;
        }
        for (id, mi, cell) in assignments {
            let Some(m) = monitors.get(*mi) else { continue };
            let at = self.tick();
            let l = self.layouts.entry(m.device.clone()).or_default().entry(m.layout_key()).or_default();
            l.items.insert(id.as_str().to_string(), Slot { col: cell.col, row: cell.row, dx: 0, dy: 0, at });
            l.provisional.remove(id.as_str());
        }
        self.imported_from_explorer = true;
        self.mark();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::grid::IconSize;

    fn mon(device: &str, w: u32, h: u32, pct: u32, primary: bool, left: i32) -> MonitorSpec {
        MonitorSpec {
            device: device.into(),
            size: Size::new(w, h),
            scale_percent: pct,
            work: Rect::new(0, 0, w as i32, h as i32),
            primary,
            rect: Rect::from_size(left, 0, w, h),
        }
    }
    /// 3 columns x 2 rows of medium icons at 100 %.
    fn tiny() -> MonitorSpec {
        mon("A", 225, 150, 100, true, 0)
    }
    fn ids(names: &[&str]) -> Vec<ItemId> {
        names.iter().map(|n| ItemId::user(n)).collect()
    }
    fn ids_of(v: &[ItemId]) -> Vec<ItemId> {
        v.to_vec()
    }
    fn cell(r: &Resolved, name: &str) -> (usize, u32, u32) {
        let p = r.placed[&ItemId::user(name)];
        (p.monitor, p.cell.col, p.cell.row)
    }

    #[test]
    fn layout_keys() {
        assert_eq!(layout_key(2560, 1440, 125), "2560x1440@125");
        assert_eq!(parse_layout_key("2560x1440@125"), Some((2560, 1440, 125)));
        assert_eq!(parse_layout_key("2560x1440"), None);
        assert_eq!(parse_layout_key("axb@c"), None);
    }

    #[test]
    fn a_new_item_takes_the_first_free_cell_down_the_first_column() {
        let mut s = Store::default();
        let m = [tiny()];
        let r = s.resolve(&m, &ids(&["a", "b", "c", "d"]), 0);
        assert_eq!(cell(&r, "a"), (0, 0, 0));
        assert_eq!(cell(&r, "b"), (0, 0, 1));
        assert_eq!(cell(&r, "c"), (0, 1, 0));
        assert_eq!(cell(&r, "d"), (0, 1, 1));
        assert_eq!(r.fresh.len(), 4);
        assert!(s.is_dirty());
        // a fifth item arrives later: the next free cell, nothing else moves
        let r2 = s.resolve(&m, &ids(&["a", "b", "c", "d", "e"]), 1);
        assert_eq!(cell(&r2, "e"), (0, 2, 0));
        for n in ["a", "b", "c", "d"] {
            assert_eq!(cell(&r2, n), cell(&r, n));
        }
        assert_eq!(r2.fresh, ids(&["e"]));
    }

    #[test]
    fn positions_survive_a_restart_through_the_file_format() {
        let mut s = Store::default();
        let m = [tiny()];
        let first = s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        let json = serde_json::to_string(&s).unwrap();
        let mut back: Store = serde_json::from_str(&json).unwrap();
        let again = back.resolve(&m, &ids(&["c", "b", "a"]), 5); // another order of listing changes nothing
        assert_eq!(again.placed, first.placed);
        assert!(again.fresh.is_empty());
    }

    #[test]
    fn a_dropped_icon_moves_and_frees_its_old_cell() {
        let mut s = Store::default();
        let m = [tiny()];
        let mut r = s.resolve(&m, &ids(&["a", "b"]), 0);
        // drop "a" on the far corner: the cell nearest (150, 75) is (2, 1)
        let p = s.drop_item(&m, &mut r, &ItemId::user("a"), 0, 150, 75).unwrap();
        assert_eq!((p.cell.col, p.cell.row), (2, 1));
        // the old cell (0,0) is free: a new item takes it
        let r2 = s.resolve(&m, &ids(&["a", "b", "z"]), 1);
        assert_eq!(cell(&r2, "a"), (0, 2, 1));
        assert_eq!(cell(&r2, "z"), (0, 0, 0));
        // dropped on a taken cell with align on: the nearest free one
        let mut r3 = r2.clone();
        let p = s.drop_item(&m, &mut r3, &ItemId::user("z"), 0, 0, 75).unwrap(); // wants (0,1) where "b" is
        assert_ne!((p.cell.col, p.cell.row), (0, 1));
        assert_eq!((p.cell.col, p.cell.row), (0, 0), "its own cell is free again, and it is as near as it gets");
    }

    #[test]
    fn two_items_claiming_one_cell_the_later_one_moves() {
        let mut s = Store::default();
        let m = [tiny()];
        let mut l = MonitorLayout::default();
        l.items.insert("u:old".into(), Slot { col: 1, row: 0, dx: 0, dy: 0, at: 1 });
        l.items.insert("u:new".into(), Slot { col: 1, row: 0, dx: 0, dy: 0, at: 2 });
        s.layouts.entry("A".into()).or_default().insert("225x150@100".into(), l);
        let r = s.resolve(&m, &ids(&["new", "old"]), 0);
        assert_eq!(cell(&r, "old"), (0, 1, 0), "the earlier placement keeps the cell");
        assert_eq!(cell(&r, "new"), (0, 1, 1), "the later one goes to the next free cell after it");
        assert_eq!(r.fresh, ids(&["new"]));
        // and the store now holds the resolved state, so a second resolve changes nothing
        let r2 = s.resolve(&m, &ids(&["new", "old"]), 1);
        assert_eq!(r2.placed, r.placed);
        assert!(r2.fresh.is_empty());
    }

    #[test]
    fn a_collision_at_the_end_wraps_to_the_start() {
        let mut s = Store::default();
        let m = [tiny()];
        let mut l = MonitorLayout::default();
        l.items.insert("u:p".into(), Slot { col: 2, row: 1, dx: 0, dy: 0, at: 1 });
        l.items.insert("u:q".into(), Slot { col: 2, row: 1, dx: 0, dy: 0, at: 2 });
        s.layouts.entry("A".into()).or_default().insert("225x150@100".into(), l);
        let r = s.resolve(&m, &ids(&["p", "q"]), 0);
        assert_eq!(cell(&r, "q"), (0, 0, 0));
    }

    #[test]
    fn with_align_off_the_same_cell_is_fine_at_different_offsets() {
        let mut s = Store { align_to_grid: false, ..Store::default() };
        let m = [tiny()];
        let mut l = MonitorLayout::default();
        l.items.insert("u:p".into(), Slot { col: 1, row: 0, dx: 5, dy: 5, at: 1 });
        l.items.insert("u:q".into(), Slot { col: 1, row: 0, dx: 30, dy: 5, at: 2 });
        l.items.insert("u:r".into(), Slot { col: 1, row: 0, dx: 30, dy: 5, at: 3 });
        s.layouts.entry("A".into()).or_default().insert("225x150@100".into(), l);
        let r = s.resolve(&m, &ids(&["p", "q", "r"]), 0);
        assert_eq!((r.placed[&ItemId::user("p")].dx, r.placed[&ItemId::user("q")].dx), (5, 30));
        assert_eq!(cell(&r, "q"), (0, 1, 0));
        assert_ne!(cell(&r, "r"), (0, 1, 0), "the same cell AND offset is a collision");
    }

    #[test]
    fn delete_frees_the_cell_and_a_restore_returns_to_it() {
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        s.remove(&ItemId::user("b"), 1000);
        // b is gone: d takes b's cell (0,1)
        let r = s.resolve(&m, &ids(&["a", "c", "d"]), 1000);
        assert_eq!(cell(&r, "d"), (0, 0, 1));
        // b comes back from the bin while its old cell is taken: it gets a fresh cell
        let r2 = s.resolve(&m, &ids(&["a", "b", "c", "d"]), 2000);
        assert_eq!(cell(&r2, "b"), (0, 1, 1));
        // the other way round: nothing took the cell, the restored item returns to it
        let mut s = Store::default();
        s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        s.remove(&ItemId::user("b"), 1000);
        let r = s.resolve(&m, &ids(&["a", "c"]), 1000);
        assert!(!r.placed.contains_key(&ItemId::user("b")));
        let r2 = s.resolve(&m, &ids(&["a", "b", "c"]), 5000);
        assert_eq!(cell(&r2, "b"), (0, 0, 1), "back where it was");
        // within the grace period d returns to its cell (1,1) although (0,0) is free too; after it, it is a new item
        let mut s = Store::default();
        s.resolve(&m, &ids(&["a", "b", "c", "d"]), 0);
        s.remove(&ItemId::user("a"), 1000);
        s.remove(&ItemId::user("d"), 1000);
        s.resolve(&m, &ids(&["b", "c"]), 1000);
        let within = s.clone().resolve(&m, &ids(&["b", "c", "d"]), 2000);
        assert_eq!(cell(&within, "d"), (0, 1, 1));
        let after = s.resolve(&m, &ids(&["b", "c", "d"]), 1000 + REMOVED_GRACE_MS + 1);
        assert_eq!(cell(&after, "d"), (0, 0, 0), "forgotten: the first free cell");
    }

    #[test]
    fn items_that_vanish_without_an_event_are_forgotten_into_the_memory() {
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b"]), 0);
        s.resolve(&m, &ids(&["a"]), 10);
        let l = &s.layouts["A"]["225x150@100"];
        assert!(l.items.contains_key("u:a") && !l.items.contains_key("u:b"));
        assert_eq!(l.removed["u:b"].gone_ms, 10);
        // an expired memory is dropped on the next resolve
        s.resolve(&m, &ids(&["a"]), 10 + REMOVED_GRACE_MS + 1);
        assert!(s.layouts["A"]["225x150@100"].removed.is_empty());
    }

    #[test]
    fn a_rename_keeps_the_cell() {
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        s.rename(&ItemId::user("b"), &ItemId::user("b renamed"));
        let r = s.resolve(&m, &ids(&["a", "b renamed", "c"]), 1);
        assert_eq!(cell(&r, "b renamed"), (0, 0, 1));
        assert!(r.fresh.is_empty());
        // across the two folders it is a different identity: that is a move, not a rename, and the caller does not call rename
        let r2 = s.resolve(&m, &ids_of(&[ItemId::user("a"), ItemId::public("b renamed"), ItemId::user("c")]), 2);
        assert_eq!(r2.placed[&ItemId::public("b renamed")].cell, Cell::new(0, 1), "first free cell: the old one was just freed");
    }

    #[test]
    fn a_rename_also_moves_the_memory_of_removed_items() {
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b"]), 0);
        s.remove(&ItemId::user("b"), 5);
        s.rename(&ItemId::user("b"), &ItemId::user("bb"));
        let r = s.resolve(&m, &ids(&["a", "bb"]), 6);
        assert_eq!(cell(&r, "bb"), (0, 0, 1));
    }

    #[test]
    fn a_resolution_change_keeps_what_fits_places_the_rest_and_keeps_the_old_layout() {
        let mut s = Store::default();
        let big = [mon("A", 1920, 1080, 100, true, 0)]; // 25 x 14 cells
        let mut l = MonitorLayout::default();
        l.items.insert("u:fits".into(), Slot { col: 3, row: 2, dx: 0, dy: 0, at: 1 });
        l.items.insert("u:far-right".into(), Slot { col: 24, row: 0, dx: 0, dy: 0, at: 2 });
        l.items.insert("u:far-down".into(), Slot { col: 0, row: 13, dx: 0, dy: 0, at: 3 });
        s.layouts.entry("A".into()).or_default().insert("1920x1080@100".into(), l.clone());
        let names = ids(&["fits", "far-right", "far-down", "fresh"]);
        let r = s.resolve(&big, &names, 0);
        assert!(r.inherited.is_empty());
        assert_eq!(cell(&r, "far-right"), (0, 24, 0));
        // the monitor is now 1280 x 720 (17 x 9 cells)
        let small = [mon("A", 1280, 720, 100, true, 0)];
        let r2 = s.resolve(&small, &names, 1);
        assert_eq!(r2.inherited, vec![0]);
        assert_eq!(cell(&r2, "fits"), (0, 3, 2), "kept");
        assert!(r2.fresh.contains(&ItemId::user("far-right")) && r2.fresh.contains(&ItemId::user("far-down")));
        for n in ["far-right", "far-down"] {
            let p = r2.placed[&ItemId::user(n)];
            assert!(p.cell.col < 17 && p.cell.row < 9, "{n} is placed inside the new grid");
        }
        // the old layout is exactly as it was
        assert_eq!(s.layouts["A"]["1920x1080@100"].items.get("u:far-right").map(|x| (x.col, x.row)), Some((24, 0)));
        // back to the big resolution: every icon is at its old cell again
        let r3 = s.resolve(&big, &names, 2);
        assert_eq!(cell(&r3, "far-right"), (0, 24, 0));
        assert_eq!(cell(&r3, "far-down"), (0, 0, 13));
        assert_eq!(cell(&r3, "fits"), (0, 3, 2));
        assert!(r3.inherited.is_empty());
    }

    #[test]
    fn the_nearest_resolution_is_the_starting_point() {
        let mut s = Store::default();
        for (key, col) in [("1280x720@100", 1), ("3840x2160@100", 2), ("1920x1080@125", 3)] {
            let mut l = MonitorLayout::default();
            l.items.insert("u:x".into(), Slot { col, row: 0, dx: 0, dy: 0, at: 1 });
            s.layouts.entry("A".into()).or_default().insert(key.into(), l);
        }
        // 1600 x 900 is nearest in pixel count to 1280 x 720 (1.44 M vs 0.92 M; 1920 x 1080 is 2.07 M, 3840 x 2160 is 8.3 M)
        let r = s.resolve(&[mon("A", 1600, 900, 100, true, 0)], &ids(&["x"]), 0);
        assert_eq!(cell(&r, "x"), (0, 1, 0));
        // 1920 x 1080 at 100 %: the same pixel count as the @125 one, so that one
        let r = s.resolve(&[mon("A", 1920, 1080, 100, true, 0)], &ids(&["x"]), 0);
        assert_eq!(cell(&r, "x"), (0, 3, 0));
        // another monitor's layouts are not borrowed
        let r = s.resolve(&[mon("B", 1920, 1080, 100, true, 0)], &ids(&["x"]), 0);
        assert!(r.inherited.is_empty());
        assert_eq!(cell(&r, "x"), (0, 0, 0));
    }

    #[test]
    fn a_scale_change_is_a_resolution_change() {
        let mut s = Store::default();
        let at100 = [mon("A", 1920, 1080, 100, true, 0)]; // 25 x 14
        let at125 = [mon("A", 1920, 1080, 125, true, 0)]; // 20 x 11
        let mut l = MonitorLayout::default();
        l.items.insert("u:edge".into(), Slot { col: 22, row: 0, dx: 0, dy: 0, at: 1 });
        s.layouts.entry("A".into()).or_default().insert("1920x1080@100".into(), l);
        let r = s.resolve(&at125, &ids(&["edge"]), 0);
        assert_eq!(r.inherited, vec![0]);
        assert_eq!(cell(&r, "edge"), (0, 0, 0), "column 22 does not exist at 125 %");
        assert!(s.layouts["A"].contains_key("1920x1080@125") && s.layouts["A"].contains_key("1920x1080@100"));
        assert_eq!(s.resolve(&at100, &ids(&["edge"]), 1).placed[&ItemId::user("edge")].cell, Cell::new(22, 0));
    }

    #[test]
    fn two_monitors_primary_first_then_spill_to_the_next() {
        let mut s = Store::default();
        // the primary is the one on the right: left-to-right order only matters after the primary
        let m = [mon("LEFT", 225, 150, 100, false, -225), tiny()];
        let r = s.resolve(&m, &ids(&["a", "b", "c", "d", "e", "f", "g"]), 0);
        // 6 cells on the primary (index 1), the 7th spills onto the left monitor
        for (i, n) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
            assert_eq!(r.placed[&ItemId::user(n)].monitor, 1, "{n}");
            assert_eq!(r.placed[&ItemId::user(n)].cell, Cell::new((i / 2) as u32, (i % 2) as u32));
        }
        assert_eq!(cell(&r, "g"), (0, 0, 0));
    }

    #[test]
    fn an_unplugged_monitors_layout_is_remembered_and_its_icons_return_when_it_is_back() {
        let mut s = Store::default();
        let a = tiny();
        let b = mon("B", 225, 150, 100, false, 225);
        // both connected: x on B at (2,1), y on A
        let mut l = MonitorLayout::default();
        l.items.insert("u:x".into(), Slot { col: 2, row: 1, dx: 0, dy: 0, at: 1 });
        s.layouts.entry("B".into()).or_default().insert("225x150@100".into(), l);
        let r = s.resolve(&[a.clone(), b.clone()], &ids(&["x", "y"]), 0);
        assert_eq!(cell(&r, "x"), (1, 2, 1));
        let b_before = s.layouts["B"]["225x150@100"].clone();
        // B is unplugged (or asleep): x is shown on A, provisionally. B's layout is untouched memory and A's layout holds no claim for x
        let r = s.resolve(std::slice::from_ref(&a), &ids(&["x", "y"]), 1);
        assert_eq!(cell(&r, "x").0, 0);
        assert_eq!(s.layouts["B"]["225x150@100"], b_before);
        assert!(!s.layouts["A"]["225x150@100"].items.contains_key("u:x"), "what was made up for the screen is not a claim");
        assert!(s.layouts["A"]["225x150@100"].provisional.contains_key("u:x"));
        // the same again: x does not move about while B is away
        let again = s.resolve(std::slice::from_ref(&a), &ids(&["x", "y"]), 2);
        assert_eq!(again.placed, r.placed);
        assert!(again.fresh.is_empty());
        // B comes back: x goes back to its own cell, B's arrangement is whole, and nothing provisional is left
        let r = s.resolve(&[a.clone(), b.clone()], &ids(&["x", "y"]), 3);
        assert_eq!(cell(&r, "x"), (1, 2, 1));
        assert_eq!(s.layouts["B"]["225x150@100"].items, b_before.items);
        assert!(s.layouts["A"]["225x150@100"].provisional.is_empty());
    }

    #[test]
    fn a_placement_the_user_makes_while_a_monitor_is_away_beats_its_old_claim() {
        let mut s = Store::default();
        let a = tiny();
        let b = mon("B", 225, 150, 100, false, 225);
        let mut l = MonitorLayout::default();
        l.items.insert("u:x".into(), Slot { col: 2, row: 1, dx: 0, dy: 0, at: 1 });
        s.layouts.entry("B".into()).or_default().insert("225x150@100".into(), l);
        let mut r = s.resolve(std::slice::from_ref(&a), &ids(&["x"]), 0);
        // the user drags x to a cell of A: that is a real decision
        s.drop_item(std::slice::from_ref(&a), &mut r, &ItemId::user("x"), 0, 150, 75).unwrap();
        assert!(s.layouts["A"]["225x150@100"].provisional.is_empty());
        let r = s.resolve(&[a, b], &ids(&["x"]), 1);
        assert_eq!(cell(&r, "x"), (0, 2, 1), "the later claim, the user's, wins over the one on the returning monitor");
        assert!(!s.layouts["B"]["225x150@100"].items.contains_key("u:x"));
    }

    #[test]
    fn items_that_are_not_drawn_keep_their_cells_for_good() {
        // hidden files with "show hidden" off, a virtual item switched off, the whole desktop switched off: they exist, they are
        // not drawn, and their cells must outlive the day the memory of deleted items lasts
        let mut s = Store::default();
        let m = [tiny()];
        let all = ids(&["a", "b", "c", "d"]);
        let before = s.resolve(&m, &all, 0);
        let day = REMOVED_GRACE_MS;
        // the desktop is switched off: nothing is drawn, everything is kept
        let none: Vec<ItemId> = Vec::new();
        let r = s.resolve(&m, Listing::complete(&none).with_kept(&all), 1000);
        assert!(r.placed.is_empty());
        let r = s.resolve(&m, Listing::complete(&none).with_kept(&all), 1000 + 3 * day);
        assert!(r.placed.is_empty());
        let l = &s.layouts["A"]["225x150@100"];
        assert_eq!(l.items.len(), 4, "every cell is still in the layout");
        assert!(l.removed.is_empty(), "nothing was forgotten");
        // switched on again days later: everything is back where it was, nothing is new
        let r = s.resolve(&m, &all, 1000 + 4 * day);
        assert_eq!(r.placed, before.placed);
        assert!(r.fresh.is_empty());
        // one file is hidden (kept), the rest drawn; it is still hidden nine days later and then shown again: the same cell
        let drawn = ids(&["a", "b", "d"]);
        let hidden = ids(&["c"]);
        s.resolve(&m, Listing::complete(&drawn).with_kept(&hidden), 1000 + 5 * day);
        s.resolve(&m, Listing::complete(&drawn).with_kept(&hidden), 1000 + 9 * day);
        let r = s.resolve(&m, &all, 1000 + 10 * day);
        assert_eq!(r.placed, before.placed);
    }

    #[test]
    fn an_incomplete_listing_forgets_nothing() {
        // the folder lookup failed, or the folder was not listed yet: an empty list says nothing about what is on the desktop
        let mut s = Store::default();
        let m = [tiny()];
        let all = ids(&["a", "b", "c"]);
        let before = s.resolve(&m, &all, 0);
        let none: Vec<ItemId> = Vec::new();
        for k in 1..=3 {
            let r = s.resolve(&m, Listing::incomplete(&none), k * REMOVED_GRACE_MS);
            assert!(r.placed.is_empty() && r.unplaced.is_empty());
        }
        let l = &s.layouts["A"]["225x150@100"];
        assert_eq!((l.items.len(), l.removed.len()), (3, 0));
        let r = s.resolve(&m, &all, 4 * REMOVED_GRACE_MS);
        assert_eq!(r.placed, before.placed);
        assert!(r.fresh.is_empty());
        // a partial list does not forget the missing ones either, and still places what it shows
        let part = ids(&["a", "z"]);
        let r = s.resolve(&m, Listing::incomplete(&part), 5 * REMOVED_GRACE_MS);
        assert_eq!(cell(&r, "a"), (0, 0, 0));
        assert_eq!(s.layouts["A"]["225x150@100"].items.len(), 4, "a b c and the new z");
        assert!(s.layouts["A"]["225x150@100"].items.contains_key("u:b"));
    }

    #[test]
    fn a_work_area_that_shrinks_does_not_rewrite_what_it_covers() {
        // a dock appears at the bottom: the last row of the grid is gone. The icon in it is shown elsewhere, its own cell is kept
        let mut s = Store::default();
        let full = mon("A", 1920, 1080, 100, true, 0); // 25 x 14 cells
        let mut docked = full.clone();
        docked.work = Rect::new(0, 0, 1920, 980); // 25 x 13 cells
        let mut l = MonitorLayout::default();
        for (n, col, row, at) in [("i0", 0, 0, 1), ("i1", 5, 5, 2), ("i2", 0, 13, 3)] {
            l.items.insert(format!("u:{n}"), Slot { col, row, dx: 0, dy: 0, at });
        }
        s.layouts.entry("A".into()).or_default().insert("1920x1080@100".into(), l.clone());
        let names = ids(&["i0", "i1", "i2"]);
        assert_eq!(cell(&s.resolve(std::slice::from_ref(&full), &names, 0), "i2"), (0, 0, 13));

        let r = s.resolve(std::slice::from_ref(&docked), &names, 1);
        let p = r.placed[&ItemId::user("i2")];
        assert!(p.cell.row < 13, "shown inside the smaller grid");
        assert_eq!(r.fresh, ids(&["i2"]));
        assert_eq!(s.layouts["A"]["1920x1080@100"].items, l.items, "the claims are exactly as they were");
        // and it stays put from pass to pass
        let r2 = s.resolve(std::slice::from_ref(&docked), &names, 2);
        assert_eq!(r2.placed, r.placed);
        assert!(r2.fresh.is_empty());
        // the dock goes away: back to the cell it was dropped on
        let r3 = s.resolve(std::slice::from_ref(&full), &names, 3);
        assert_eq!(cell(&r3, "i2"), (0, 0, 13));
        assert!(s.layouts["A"]["1920x1080@100"].provisional.is_empty());
        assert_eq!(s.layouts["A"]["1920x1080@100"].items, l.items);
    }

    #[test]
    fn items_that_do_not_fit_keep_their_cells() {
        let mut s = Store::default();
        let full = tiny(); // 3 x 2
        let mut narrow = full.clone();
        narrow.work = Rect::new(0, 0, 75, 150); // 1 x 2
        let mut empty = full.clone();
        empty.work = Rect::new(0, 0, 0, 0); // a monitor that reports no usable area for a moment
        let names = ids(&["a", "b", "c", "d", "e", "f"]);
        let before = s.resolve(std::slice::from_ref(&full), &names, 0);
        let stored = s.layouts["A"]["225x150@100"].items.clone();
        assert_eq!(stored.len(), 6);

        let r = s.resolve(std::slice::from_ref(&narrow), &names, 1);
        assert_eq!((r.placed.len(), r.unplaced.len()), (2, 4));
        assert_eq!(s.layouts["A"]["225x150@100"].items, stored, "the four that do not fit are still in the layout, at their cells");

        let r = s.resolve(std::slice::from_ref(&empty), &names, 2);
        assert_eq!((r.placed.len(), r.unplaced.len()), (0, 6));
        assert_eq!(s.layouts["A"]["225x150@100"].items, stored, "an empty grid wipes nothing");

        let r = s.resolve(std::slice::from_ref(&full), &names, 3);
        assert_eq!(r.placed, before.placed, "everything is back where it was");
        assert!(r.fresh.is_empty());
    }

    #[test]
    fn a_displaced_icon_that_the_user_drags_gets_a_real_cell() {
        let mut s = Store::default();
        let full = mon("A", 1920, 1080, 100, true, 0);
        let mut docked = full.clone();
        docked.work = Rect::new(0, 0, 1920, 980);
        let mut l = MonitorLayout::default();
        l.items.insert("u:i".into(), Slot { col: 0, row: 13, dx: 0, dy: 0, at: 1 });
        s.layouts.entry("A".into()).or_default().insert("1920x1080@100".into(), l);
        let names = ids(&["i"]);
        let mut r = s.resolve(std::slice::from_ref(&docked), &names, 0);
        s.drop_item(std::slice::from_ref(&docked), &mut r, &ItemId::user("i"), 0, 300, 300).unwrap();
        let l = &s.layouts["A"]["1920x1080@100"];
        assert!(l.provisional.is_empty());
        assert_eq!((l.items["u:i"].col, l.items["u:i"].row), (4, 4));
        // undocking does not move it: the user's decision is newer than the old claim
        assert_eq!(cell(&s.resolve(std::slice::from_ref(&full), &names, 1), "i"), (0, 4, 4));
    }

    #[test]
    fn a_collision_is_shown_apart_but_the_claims_stay_and_the_loser_returns_when_the_cell_frees() {
        let mut s = Store::default();
        let m = [tiny()];
        let mut l = MonitorLayout::default();
        l.items.insert("u:old".into(), Slot { col: 1, row: 0, dx: 0, dy: 0, at: 1 });
        l.items.insert("u:new".into(), Slot { col: 1, row: 0, dx: 0, dy: 0, at: 2 });
        s.layouts.entry("A".into()).or_default().insert("225x150@100".into(), l);
        let r = s.resolve(&m, &ids(&["new", "old"]), 0);
        assert_eq!(cell(&r, "new"), (0, 1, 1));
        assert!(s.layouts["A"]["225x150@100"].items.contains_key("u:new") && s.layouts["A"]["225x150@100"].provisional.contains_key("u:new"));
        // the winner is deleted: the loser is back at the cell it claimed
        s.remove(&ItemId::user("old"), 5);
        let r = s.resolve(&m, &ids(&["new"]), 6);
        assert_eq!(cell(&r, "new"), (0, 1, 0));
        assert!(s.layouts["A"]["225x150@100"].provisional.is_empty());
    }

    #[test]
    fn rename_delete_and_icon_size_follow_the_provisional_places_too() {
        let mut s = Store::default();
        let mut l = MonitorLayout::default();
        l.provisional.insert("u:old".into(), Slot { col: 1, row: 1, dx: 0, dy: 0, at: 0 });
        s.layouts.entry("A".into()).or_default().insert("225x150@100".into(), l);
        s.rename(&ItemId::user("old"), &ItemId::user("new"));
        assert!(s.layouts["A"]["225x150@100"].provisional.contains_key("u:new"));
        s.set_icon_size(IconSize::Small); // 75 -> 64: (1,1) stays (1,1)
        let p = s.layouts["A"]["225x150@100"].provisional["u:new"];
        assert_eq!((p.col, p.row), (1, 1));
        s.remove(&ItemId::user("new"), 5);
        assert!(s.layouts["A"]["225x150@100"].provisional.is_empty());
    }

    #[test]
    fn a_memory_dated_in_the_future_expires_like_any_other() {
        // a clock set back, or a hand-edited file: gone_ms is later than now. It must not live for ever.
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b"]), 0);
        let mut l = s.layouts["A"]["225x150@100"].clone();
        l.removed.insert("u:ghost".into(), Removed { col: 2, row: 1, dx: 0, dy: 0, at: 1, gone_ms: i64::MAX });
        s.layouts.get_mut("A").unwrap().insert("225x150@100".into(), l);
        let now = 1_000_000;
        s.resolve(&m, &ids(&["a", "b"]), now);
        assert_eq!(s.layouts["A"]["225x150@100"].removed["u:ghost"].gone_ms, now, "counted from now");
        s.resolve(&m, &ids(&["a", "b"]), now + REMOVED_GRACE_MS + 1);
        assert!(s.layouts["A"]["225x150@100"].removed.is_empty());
        // and the extreme values do not overflow
        let mut l = s.layouts["A"]["225x150@100"].clone();
        l.removed.insert("u:old".into(), Removed { col: 0, row: 0, dx: 0, dy: 0, at: 1, gone_ms: i64::MIN });
        s.layouts.get_mut("A").unwrap().insert("225x150@100".into(), l);
        s.resolve(&m, &ids(&["a", "b", "old"]), i64::MAX);
    }

    #[test]
    fn the_counter_saturates_instead_of_wrapping() {
        let mut s = Store { clock: u64::MAX - 1, ..Store::default() };
        let m = [tiny()];
        let r = s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        assert_eq!(r.placed.len(), 3);
        assert_eq!(s.clock, u64::MAX);
        assert_eq!(s.tick(), u64::MAX);
    }

    #[test]
    fn the_import_is_one_time_unless_forced() {
        let mut s = Store::default();
        let m = [tiny()];
        assert!(s.import_positions(&m, &[(ItemId::user("a"), 0, Cell::new(2, 1))], false));
        // the user rearranges in the new desktop
        let mut r = s.resolve(&m, &ids(&["a"]), 0);
        s.drop_item(&m, &mut r, &ItemId::user("a"), 0, 0, 0).unwrap();
        let mine = s.clone();
        // a second import (a repeated click, a replayed first-run path) changes nothing
        assert!(!s.import_positions(&m, &[(ItemId::user("a"), 0, Cell::new(2, 1))], false));
        assert_eq!(s, mine);
        assert_eq!(cell(&s.resolve(&m, &ids(&["a"]), 1), "a"), (0, 0, 0));
        // unless the user asked for it
        assert!(s.import_positions(&m, &[(ItemId::user("a"), 0, Cell::new(2, 1))], true));
        assert_eq!(cell(&s.resolve(&m, &ids(&["a"]), 2), "a"), (0, 2, 1));
    }

    #[test]
    fn auto_arrange_lays_out_in_the_given_order_and_leaves_the_store_alone() {
        let mut s = Store::default();
        let m = [tiny()];
        s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        let before = s.layouts.clone();
        s.set_auto_arrange(true);
        let r = s.resolve(&m, &ids(&["c", "a", "b"]), 1);
        assert_eq!((cell(&r, "c"), cell(&r, "a"), cell(&r, "b")), ((0, 0, 0), (0, 0, 1), (0, 1, 0)));
        assert_eq!(s.layouts, before, "stored positions are not touched");
        // turning it off keeps what was on the screen
        s.freeze(&m, &r);
        assert!(!s.auto_arrange);
        let r2 = s.resolve(&m, &ids(&["c", "a", "b"]), 2);
        assert_eq!(r2.placed, r.placed);
    }

    #[test]
    fn more_items_than_cells_leave_the_rest_unplaced() {
        let mut s = Store::default();
        let r = s.resolve(&[tiny()], &ids(&["1", "2", "3", "4", "5", "6", "7", "8"]), 0);
        assert_eq!(r.placed.len(), 6);
        assert_eq!(r.unplaced, ids(&["7", "8"]));
        assert!(s.resolve(&[], &ids(&["1"]), 0).unplaced.len() == 1, "no monitor, no cells");
    }

    #[test]
    fn the_icon_size_remaps_cells_to_where_their_centres_are() {
        let mut s = Store::default();
        let m = [mon("A", 1920, 1080, 100, true, 0)];
        let mut l = MonitorLayout::default();
        l.items.insert("u:p".into(), Slot { col: 0, row: 0, dx: 0, dy: 0, at: 1 });
        l.items.insert("u:q".into(), Slot { col: 10, row: 4, dx: 0, dy: 0, at: 2 });
        s.layouts.entry("A".into()).or_default().insert("1920x1080@100".into(), l);
        s.set_icon_size(IconSize::Large); // cell 75 -> 123
        assert_eq!(s.icon_size, 96);
        let l = &s.layouts["A"]["1920x1080@100"];
        assert_eq!((l.items["u:p"].col, l.items["u:p"].row), (0, 0));
        assert_eq!((l.items["u:q"].col, l.items["u:q"].row), (6, 2)); // (10*75+37)/123 = 6, (4*75+37)/123 = 2
        let r = s.resolve(&m, &ids(&["p", "q"]), 0);
        assert_eq!(cell(&r, "q"), (0, 6, 2));
        let before = s.clone();
        s.set_icon_size(IconSize::Large);
        assert_eq!(s, before, "same size: nothing changes");
    }

    #[test]
    fn the_import_writes_into_the_exact_layout_and_marks_it_done() {
        let mut s = Store::default();
        let m = [tiny()];
        assert!(s.import_positions(&m, &[(ItemId::user("a"), 0, Cell::new(2, 1)), (ItemId::user("b"), 0, Cell::new(0, 0))], false));
        assert!(s.imported_from_explorer && s.is_dirty());
        let r = s.resolve(&m, &ids(&["a", "b", "c"]), 0);
        assert_eq!((cell(&r, "a"), cell(&r, "b"), cell(&r, "c")), ((0, 2, 1), (0, 0, 0), (0, 0, 1)));
    }
}
