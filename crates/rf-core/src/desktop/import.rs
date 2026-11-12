//! The one-time import of Explorer's icon positions: from raw points to our cells.
//!
//! WHERE THE POINTS COME FROM (`rf_win::desktop::import`): `IFolderView::GetItemPosition` on Explorer's desktop view, the one route
//! Microsoft documents for this (the `IconLayouts` registry blob is undocumented and is not parsed). WHAT IS NOT KNOWN: the units and
//! the origin of those points on this build (shell plan test T13: physical pixels or not, relative to the virtual screen's top-left
//! or to Progman's, in Explorer's DPI context or each monitor's). So nothing here guesses: [`convert`] takes the assumption as an
//! argument, checks it against the monitors (a point must land on a monitor, inside its grid) and REFUSES, with
//! [`ImportFailure::UnitsDoubtful`], when fewer than nine in ten fit. [`diagnose`] shows how well each common scale would fit, which
//! is the measurement T13 asks for.

use super::geom::{Rect, virtual_screen};
use super::grid::{Cell, Grid, IconSize, metrics};
use super::items::ItemId;
use super::layout::MonitorSpec;

/// A position exactly as Explorer's view returned it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawPosition {
    pub id: ItemId,
    pub x: i32,
    pub y: i32,
}

/// What the raw numbers are taken to be: relative to the top-left of the virtual screen, in pixels that are `scale_percent` of a
/// physical pixel (100: they are physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Assumption {
    pub scale_percent: u32,
}

impl Default for Assumption {
    fn default() -> Self {
        Assumption { scale_percent: 100 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportFailure {
    /// There is no desktop view to ask (Explorer is not the shell, or its desktop view is not registered).
    NoExplorerDesktop,
    /// A COM call failed; the text names the step.
    Com(String),
    /// The shell thread did not answer in time.
    TimedOut,
    /// The view has no items.
    NoItems,
    /// The positions do not fit the monitors under the assumption made.
    UnitsDoubtful { fit: usize, total: usize },
}

impl std::fmt::Display for ImportFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ImportFailure::NoExplorerDesktop => write!(f, "no Explorer desktop view to read"),
            ImportFailure::Com(s) => write!(f, "{s}"),
            ImportFailure::TimedOut => write!(f, "the shell thread did not answer"),
            ImportFailure::NoItems => write!(f, "the desktop view has no items"),
            ImportFailure::UnitsDoubtful { fit, total } => write!(f, "{fit} of {total} positions fit the monitors; the units are not trusted"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Converted {
    pub assignments: Vec<(ItemId, usize, Cell)>,
    pub fit: usize,
    pub total: usize,
}

/// The cell a raw point falls in under `assumption`, if it lands on a monitor's grid: the monitor index and the cell. The point is
/// the ICON's top-left corner (on the machine this was written on, medium icons were reported 13 pixels right of and 2 below their
/// cell's corner: (75 - 48) / 2 across, and a little down), so the cell is the one that holds the icon's centre. A point up to half a
/// cell outside a monitor still belongs to it.
fn locate(x: i32, y: i32, vs: Rect, monitors: &[MonitorSpec], grids: &[Grid], a: Assumption) -> Option<(usize, Cell)> {
    let px = (x as i64 * a.scale_percent as i64 + 50).div_euclid(100) as i32 + vs.left;
    let py = (y as i64 * a.scale_percent as i64 + 50).div_euclid(100) as i32 + vs.top;
    monitors.iter().enumerate().find_map(|(i, m)| {
        let g = &grids[i];
        let slack = (g.metrics.pitch_x / 2) as i32;
        let hit = Rect::new(m.rect.left - slack, m.rect.top - slack, m.rect.right + slack, m.rect.bottom + slack);
        if !hit.contains(px, py) {
            return None;
        }
        let half = g.metrics.icon_px as i32 / 2;
        let (lx, ly) = (px - m.rect.left - g.origin.0 + half, py - m.rect.top - g.origin.1 + half);
        let (pw, ph) = (g.metrics.pitch_x as i32, g.metrics.pitch_y as i32);
        let (cx, cy) = (lx.div_euclid(pw), ly.div_euclid(ph));
        (cx >= 0 && cy >= 0 && g.contains(Cell::new(cx as u32, cy as u32))).then(|| (i, Cell::new(cx as u32, cy as u32)))
    })
}

fn grids(monitors: &[MonitorSpec], size: IconSize) -> Vec<Grid> {
    monitors.iter().map(|m| Grid::new(m.work, metrics(size, m.scale_percent))).collect()
}

/// Turns the raw positions into cells, or says why it will not.
pub fn convert(raw: &[RawPosition], monitors: &[MonitorSpec], size: IconSize, a: Assumption) -> Result<Converted, ImportFailure> {
    if raw.is_empty() {
        return Err(ImportFailure::NoItems);
    }
    let vs = virtual_screen(&monitors.iter().map(|m| m.rect).collect::<Vec<_>>());
    let gs = grids(monitors, size);
    let assignments: Vec<(ItemId, usize, Cell)> = raw.iter().filter_map(|r| locate(r.x, r.y, vs, monitors, &gs, a).map(|(m, c)| (r.id.clone(), m, c))).collect();
    let (fit, total) = (assignments.len(), raw.len());
    if fit * 10 < total * 9 {
        return Err(ImportFailure::UnitsDoubtful { fit, total });
    }
    Ok(Converted { assignments, fit, total })
}

/// For each common scale: how many positions would land on a monitor's grid if the raw numbers were in units of that scale. The
/// probe prints this; it does not change anything.
pub fn diagnose(raw: &[RawPosition], monitors: &[MonitorSpec], size: IconSize) -> Vec<(u32, usize)> {
    let vs = virtual_screen(&monitors.iter().map(|m| m.rect).collect::<Vec<_>>());
    let gs = grids(monitors, size);
    [100u32, 125, 150, 175, 200].into_iter().map(|p| (p, raw.iter().filter(|r| locate(r.x, r.y, vs, monitors, &gs, Assumption { scale_percent: p }).is_some()).count())).collect()
}

/// `((min x, max x), (min y, max y))` of the raw points.
pub fn coordinate_range(raw: &[RawPosition]) -> Option<((i32, i32), (i32, i32))> {
    let (first, rest) = raw.split_first()?;
    let mut r = ((first.x, first.x), (first.y, first.y));
    for p in rest {
        r = ((r.0.0.min(p.x), r.0.1.max(p.x)), (r.1.0.min(p.y), r.1.1.max(p.y)));
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::geom::Size;
    use crate::desktop::store::Store;

    fn land() -> MonitorSpec {
        MonitorSpec { device: "L".into(), size: Size::new(1920, 1080), scale_percent: 100, work: Rect::new(0, 0, 1920, 1080), primary: true, rect: Rect::new(0, 0, 1920, 1080) }
    }
    fn port() -> MonitorSpec {
        MonitorSpec { device: "P".into(), size: Size::new(1440, 2560), scale_percent: 100, work: Rect::new(0, 0, 1440, 2560), primary: false, rect: Rect::new(-1440, -560, 0, 2000) }
    }
    fn raw(name: &str, x: i32, y: i32) -> RawPosition {
        RawPosition { id: ItemId::user(name), x, y }
    }
    // the virtual screen starts at (-1440, -560), so raw (0,0) is the portrait monitor's corner and raw (1440, 560) the landscape's

    #[test]
    fn points_become_cells_on_the_right_monitor_with_a_negative_origin() {
        let m = [land(), port()];
        let pts = [raw("a", 1440 + 75, 560 + 150), raw("b", 0, 0), raw("c", 78, 3 + 75), raw("d", 1440, 560)];
        let c = convert(&pts, &m, IconSize::Medium, Assumption::default()).unwrap();
        assert_eq!((c.fit, c.total), (4, 4));
        let at = |n: &str| c.assignments.iter().find(|(id, _, _)| *id == ItemId::user(n)).map(|(_, m, c)| (*m, c.col, c.row));
        assert_eq!(at("a"), Some((0, 1, 2)), "landscape monitor, cell (1,2)");
        assert_eq!(at("b"), Some((1, 0, 0)), "portrait monitor's corner");
        assert_eq!(at("c"), Some((1, 1, 1)), "a few pixels off the corner of cell (1,1) still lands in it");
        assert_eq!(at("d"), Some((0, 0, 0)));
    }

    #[test]
    fn a_scale_assumption_is_applied_to_the_raw_numbers() {
        let m = [land()];
        // raw numbers in units of 150 % pixels: 50 raw = 75 physical = one cell
        let pts = [raw("a", 50, 100)];
        let c = convert(&pts, &m, IconSize::Medium, Assumption { scale_percent: 150 }).unwrap();
        assert_eq!((c.assignments[0].2.col, c.assignments[0].2.row), (1, 2));
        let c = convert(&pts, &m, IconSize::Medium, Assumption::default()).unwrap();
        assert_eq!((c.assignments[0].2.col, c.assignments[0].2.row), (0, 1), "100 %: the icon at (50,100) has its centre at (74,124), inside cell (0,1)");
    }

    #[test]
    fn positions_that_do_not_fit_are_refused_not_guessed() {
        let m = [land()];
        // nothing near the monitor
        let far: Vec<_> = (0..10).map(|i| raw(&format!("f{i}"), 50_000 + i, 50_000)).collect();
        assert_eq!(convert(&far, &m, IconSize::Medium, Assumption::default()), Err(ImportFailure::UnitsDoubtful { fit: 0, total: 10 }));
        // 8 of 10 fit: under nine in ten
        let mut mixed: Vec<_> = (0..8).map(|i| raw(&format!("g{i}"), 75 * i, 0)).collect();
        mixed.extend([raw("x", 90_000, 0), raw("y", 0, 90_000)]);
        assert_eq!(convert(&mixed, &m, IconSize::Medium, Assumption::default()), Err(ImportFailure::UnitsDoubtful { fit: 8, total: 10 }));
        // 9 of 10 fit: accepted, the stray one is simply not imported (it is placed fresh later)
        mixed.pop();
        mixed.push(raw("z", 75 * 8, 0));
        let c = convert(&mixed, &m, IconSize::Medium, Assumption::default()).unwrap();
        assert_eq!((c.fit, c.total), (9, 10));
        assert_eq!(convert(&[], &m, IconSize::Medium, Assumption::default()), Err(ImportFailure::NoItems));
    }

    #[test]
    fn the_diagnosis_shows_which_scale_fits() {
        let m = [land()];
        // positions of a 150 % desktop reported in 150 % units: at 1920 x 1080 only the first 12 columns of that grid exist in raw units
        // of 150 %, so under "100 %" a few land outside
        let pts: Vec<_> = (0..14).map(|i| raw(&format!("p{i}"), 1100 + 75 * (i % 2), 500 + 75 * (i / 2))).collect();
        let d = diagnose(&pts, &m, IconSize::Medium);
        assert_eq!(d.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [100, 125, 150, 175, 200]);
        let fit = |p: u32| d.iter().find(|(q, _)| *q == p).unwrap().1;
        assert_eq!(fit(100), 14, "all inside at face value");
        assert!(fit(200) < fit(100), "doubling the numbers pushes many off the monitor: {d:?}");
        assert_eq!(coordinate_range(&pts), Some(((1100, 1175), (500, 950))));
        assert_eq!(coordinate_range(&[]), None);
    }

    #[test]
    fn imported_cells_go_into_the_store_and_new_items_find_the_free_cells() {
        let m = [land()];
        let pts = [raw("a", 0, 0), raw("b", 0, 75), raw("c", 150, 0)];
        let c = convert(&pts, &m, IconSize::Medium, Assumption::default()).unwrap();
        let mut s = Store::default();
        assert!(s.import_positions(&m, &c.assignments, false));
        let items = [ItemId::user("a"), ItemId::user("b"), ItemId::user("c"), ItemId::user("n")];
        let r = s.resolve(&m, crate::desktop::layout::Listing::complete(&items), 0);
        assert_eq!(r.placed[&ItemId::user("c")].cell, Cell::new(2, 0));
        assert_eq!(r.placed[&ItemId::user("n")].cell, Cell::new(0, 2), "the first free cell after the imported ones");
    }
}
