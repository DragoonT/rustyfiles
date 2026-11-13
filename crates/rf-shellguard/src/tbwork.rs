//! The work area while OUR hide of Explorer's taskbar is in force: the rules, with no Windows in them.
//!
//! WHY. A hidden taskbar keeps its strip (seen on the user's PC: work area 2560x1314 with the taskbar hidden, 48 px for the taskbar's
//! strip plus 78 px for the dock's band above it, so the dock floated 46 px above the screen's bottom with the wallpaper under it).
//! While the hide is in force the dock takes the bottom edge instead:
//!
//! * the primary monitor's work area becomes the monitor less the dock's own band ([`monitor_less_band`]) when `dock_reserve` is on,
//!   and the whole monitor when it is off;
//! * every other monitor whose taskbar we hid becomes the whole monitor;
//! * the dock does NOT register its band with Explorer in the meantime (a second band on top of ours; see `rf_win::dock::system`).
//!
//! This is the same rule the work-area keeper for a session with NO Explorer uses (`rf_win::shellhost::workarea::target` calls
//! [`monitor_less_band`]); the two owners stay separate, only the arithmetic is shared.
//!
//! WHAT IS TOUCHED, and what is not. Only the monitor of a taskbar that we hid, only a taskbar on the BOTTOM edge (Windows 11 has no
//! other; a bar at the top or a side, older Windows or a tweak, is left alone), and only where the geometry is unambiguous: the work
//! area is the monitor less that taskbar's strip (what a hidden taskbar still reserves), or the whole monitor (Explorer did not
//! reserve it), or - on the primary monitor, while the dock's own band is still registered with Explorer - the monitor less the strip
//! less a band at the bottom. Anything else means another toolbar reserves room there (a third-party bar, a sidebar): that monitor is
//! left alone ([`Verdict::Foreign`]) and the strip stays, as before this increment.
//!
//! The pure parts here are the target ([`target_for`]), the judgement of what is there ([`plan_edge`]), the rectangles to write down
//! before the first change ([`EdgePlan::originals`]) and what to put back on the way out ([`work_restore`]: the written-down
//! rectangles are checked against the monitors as they are NOW, because a resolution, scale or monitor change may have come between).
//! The ORDER of the steps is `tbctl`'s (and tested there).

use crate::statefile::WorkAreaRec;
use crate::tbmodel::{MonitorGeom, WorkChange};

/// The monitor less a band of `thickness` px at its bottom edge. At least one pixel of band and at least one pixel left, so the
/// result is never empty or inverted.
pub fn monitor_less_band(monitor: [i32; 4], thickness: i32) -> [i32; 4] {
    let [left, top, right, bottom] = monitor;
    [left, top, right, (bottom - thickness.max(1)).max(top + 1)]
}

/// Which edge of its monitor a taskbar sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Top,
    Right,
    Bottom,
}

fn near(a: i32, b: i32) -> bool {
    (a - b).abs() <= 1
}

fn same(a: [i32; 4], b: [i32; 4]) -> bool {
    a.iter().zip(b.iter()).all(|(x, y)| near(*x, *y))
}

fn area(a: [i32; 4], b: [i32; 4]) -> i64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0) as i64;
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0) as i64;
    w * h
}

/// The monitor a bar belongs to: the one it overlaps most.
fn owner_of(monitors: &[MonitorGeom], bar: &[i32; 4]) -> Option<usize> {
    monitors.iter().enumerate().filter(|(_, m)| area(m.rect, *bar) > 0).max_by_key(|(_, m)| area(m.rect, *bar)).map(|(i, _)| i)
}

/// `inner` is a non-empty rectangle that lies within `outer`.
fn inside(inner: [i32; 4], outer: [i32; 4]) -> bool {
    inner[2] > inner[0] && inner[3] > inner[1] && inner[0] >= outer[0] && inner[1] >= outer[1] && inner[2] <= outer[2] && inner[3] <= outer[3]
}

/// The edge a bar is on: it spans (nearly) the whole monitor along that edge and touches it. `None`: it is not a bar on an edge.
pub fn side_of(monitor: [i32; 4], bar: [i32; 4]) -> Option<Side> {
    let (mw, mh) = (monitor[2] - monitor[0], monitor[3] - monitor[1]);
    let (bw, bh) = (bar[2] - bar[0], bar[3] - bar[1]);
    if mw <= 0 || mh <= 0 || bw <= 0 || bh <= 0 {
        return None;
    }
    let wide = i64::from(bw) * 10 >= i64::from(mw) * 9;
    let tall = i64::from(bh) * 10 >= i64::from(mh) * 9;
    if wide && near(bar[3], monitor[3]) && bar[1] > monitor[1] + mh / 2 {
        Some(Side::Bottom)
    } else if wide && near(bar[1], monitor[1]) && bar[3] < monitor[1] + mh / 2 {
        Some(Side::Top)
    } else if tall && near(bar[0], monitor[0]) && bar[2] < monitor[0] + mw / 2 {
        Some(Side::Left)
    } else if tall && near(bar[2], monitor[2]) && bar[0] > monitor[0] + mw / 2 {
        Some(Side::Right)
    } else {
        None
    }
}

/// The monitor less the strip of a bar on its bottom edge: what the work area is with only that taskbar reserving room.
pub fn without_strip(monitor: [i32; 4], bar: [i32; 4]) -> [i32; 4] {
    [monitor[0], monitor[1], monitor[2], bar[1]]
}

/// The work area WE want on a monitor: the primary one keeps a band at the bottom when the dock reserves one (`band`: its thickness
/// in device px, `None` when `dock_reserve` is off), every other monitor is given whole.
pub fn target_for(monitor: [i32; 4], primary: bool, band: Option<i32>) -> [i32; 4] {
    match (primary, band) {
        (true, Some(t)) => monitor_less_band(monitor, t),
        _ => monitor,
    }
}

/// A work area that is `strip_only` with a band cut from its bottom edge: the other three edges agree and the bottom is above the
/// strip's, but not above its middle (a band is a fraction of the screen, never the screen).
fn band_cut(work: [i32; 4], strip_only: [i32; 4]) -> bool {
    near(work[0], strip_only[0]) && near(work[1], strip_only[1]) && near(work[2], strip_only[2]) && work[3] < strip_only[3] - 1 && work[3] > strip_only[1] + (strip_only[3] - strip_only[1]) / 2
}

/// What is there on one monitor, as far as the rule can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// None of the taskbars we hid is on this monitor: not ours to touch.
    NotOurs,
    /// Another toolbar reserves room here (or the taskbar is not on an edge): left alone, the strip stays.
    Foreign,
    /// The work area is the target already.
    AsWanted,
    /// The work area is what a hidden taskbar leaves (or the whole monitor): to be set to the target.
    Set,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MonitorPlan {
    pub id: String,
    pub primary: bool,
    pub rect: [i32; 4],
    /// The work area as read.
    pub work_now: [i32; 4],
    /// The work area we want (meaningful for [`Verdict::Set`] and [`Verdict::AsWanted`]).
    pub target: [i32; 4],
    /// The work area the user has WITHOUT us - the monitor less the taskbar's strip, with no band of the dock's - which is what is
    /// written down before the first change and put back on the way out (meaningful for [`Verdict::Set`] and [`Verdict::AsWanted`]).
    pub user_work: [i32; 4],
    /// Explorer still holds a band of the dock's here (the work area is the monitor less the strip less a band): the dock has not let
    /// go of its registration, or Explorer has not taken the removal. Only judged on the primary monitor.
    pub band_held: bool,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EdgePlan {
    pub monitors: Vec<MonitorPlan>,
}

impl EdgePlan {
    /// The monitors whose work area is to be set now.
    pub fn changes(&self) -> Vec<WorkChange> {
        self.monitors.iter().filter(|m| m.verdict == Verdict::Set).map(|m| WorkChange { id: m.id.clone(), from: m.work_now, to: m.target }).collect()
    }

    /// What is to be written down before the first change: the user's own work area of every monitor that is or will be ours. (As
    /// "changes" for [`crate::tbmodel::originals_to_save`], whose `from` is the rectangle to keep.)
    pub fn originals(&self) -> Vec<WorkChange> {
        self.monitors.iter().filter(|m| matches!(m.verdict, Verdict::Set | Verdict::AsWanted)).map(|m| WorkChange { id: m.id.clone(), from: m.user_work, to: m.target }).collect()
    }

    /// Whether any monitor is, or is about to be, ours.
    pub fn any_ours(&self) -> bool {
        self.monitors.iter().any(|m| matches!(m.verdict, Verdict::Set | Verdict::AsWanted))
    }

    /// The primary monitor's plan, if the rule found it.
    pub fn primary(&self) -> Option<&MonitorPlan> {
        self.monitors.iter().find(|m| m.primary)
    }

    /// Whether the dock sits on a monitor that is ours: then the dock must let go of its own band registration and place itself from
    /// the monitor's bottom edge.
    pub fn primary_ours(&self) -> bool {
        self.primary().is_some_and(|m| matches!(m.verdict, Verdict::Set | Verdict::AsWanted))
    }

    /// The bottom strip of the primary monitor that is the dock's own band (an empty strip at the very bottom when `dock_reserve` is
    /// off): what the dock's placement is told, `[left, bottom - thickness, right, bottom]`.
    pub fn dock_edge(&self, band: Option<i32>) -> Option<[i32; 4]> {
        let m = self.primary()?;
        let [l, _, r, b] = m.rect;
        let top = match band {
            Some(t) => monitor_less_band(m.rect, t)[3],
            None => b,
        };
        Some([l, top, r, b])
    }
}

/// Judges every monitor. `bars` are the rectangles of the taskbars we hid (a bar belongs to the monitor it overlaps most), `band` is
/// the dock's band thickness (`None`: the dock does not reserve one).
pub fn plan_edge(monitors: &[MonitorGeom], bars: &[[i32; 4]], band: Option<i32>) -> EdgePlan {
    plan_edge_held(monitors, bars, band, &[])
}

/// [`plan_edge`] for a host that remembers the work areas it set itself (`held`, one rectangle for each monitor it changed): a
/// work area that IS one of them is the host's own, whatever it looks like. Without that memory a band is told from a foreign
/// toolbar only by being cut deeper than the taskbar's strip, so a band THINNER than the strip (the thin dock at a small size, with
/// small margins) was taken for another program's and never changed again.
pub fn plan_edge_held(monitors: &[MonitorGeom], bars: &[[i32; 4]], band: Option<i32>, held: &[[i32; 4]]) -> EdgePlan {
    let mut out = Vec::with_capacity(monitors.len());
    for (i, m) in monitors.iter().enumerate() {
        let bar = bars.iter().find(|b| owner_of(monitors, b) == Some(i));
        let target = target_for(m.rect, m.primary, band);
        let mut plan = MonitorPlan { id: m.id.clone(), primary: m.primary, rect: m.rect, work_now: m.work, target, user_work: m.rect, band_held: false, verdict: Verdict::NotOurs };
        if let Some(bar) = bar {
            plan.verdict = Verdict::Foreign;
            // Only a taskbar on the bottom edge is ours to take the strip of: the dock sits at the bottom, and a bar elsewhere is left
            // as it is (the dock keeps its own registration and the strip stays).
            if side_of(m.rect, *bar) == Some(Side::Bottom) {
                let strip_only = without_strip(m.rect, *bar);
                // The user's own work area is what the taskbar's strip leaves. With the whole monitor read (an auto-hiding taskbar that
                // happens to be up reserves nothing) the monitor is what the user has and is what is written down: the strip is not
                // guessed into it.
                plan.user_work = if same(m.work, m.rect) { m.rect } else { strip_only };
                // (a work area that already is the target is ours, whatever it looks like)
                plan.band_held = m.primary && !same(m.work, target) && band_cut(m.work, strip_only);
                if same(m.work, target) {
                    plan.verdict = Verdict::AsWanted;
                } else if same(m.work, strip_only) || same(m.work, m.rect) || plan.band_held || held.iter().any(|h| same(m.work, *h)) {
                    // (band_held: the dock's own band is still registered with Explorer, the user's work area is the one without it;
                    // the last: a rectangle the host set itself, e.g. for a band that has changed since)
                    plan.verdict = Verdict::Set;
                }
            }
        }
        out.push(plan);
    }
    EdgePlan { monitors: out }
}

/// What the way out does with the written-down work areas.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WorkRestore {
    /// The rectangles to set, in the order they were written down.
    pub put: Vec<[i32; 4]>,
    /// How many of them are NOT the written-down ones: the monitor changed since (resolution, scale, the taskbar's size), so the work
    /// area the user has without us was worked out again from the taskbar as it is now.
    pub refreshed: usize,
    /// How many were left out: the monitor is gone, or the rectangle cannot belong to it. Nothing is set for them; Explorer computes
    /// the work area of a monitor itself.
    pub dropped: usize,
}

/// Judges the written-down work areas against the monitors as they are NOW. `bars` are the rectangles of the taskbars that are shown
/// again (Explorer's, visible).
///
/// A rectangle is put back as written when it still is what that monitor's taskbar leaves (or the whole monitor: nothing reserved).
/// When the monitor's geometry changed in between - another resolution or scale, a monitor unplugged, the taskbar a different size -
/// the written-down rectangle is of another screen: applied blindly it would shrink or overflow the work area of a monitor that has
/// nothing to do with it (`SPI_SETWORKAREA` takes it for "the monitor that holds it"). So it is worked out again from the taskbar
/// (the monitor less its strip), or left out when nothing can tell. With no monitor known at all nothing can be judged and the
/// rectangles are put back as written.
pub fn work_restore(originals: &[WorkAreaRec], monitors: &[MonitorGeom], bars: &[[i32; 4]]) -> WorkRestore {
    let mut out = WorkRestore::default();
    for o in originals {
        if monitors.is_empty() {
            if o.rect[2] > o.rect[0] && o.rect[3] > o.rect[1] {
                out.put.push(o.rect);
            } else {
                out.dropped += 1;
            }
            continue;
        }
        let Some((i, m)) = monitors.iter().enumerate().find(|(_, m)| m.id == o.monitor) else {
            out.dropped += 1;
            continue;
        };
        let fresh = bars.iter().find(|b| owner_of(monitors, b) == Some(i) && side_of(m.rect, **b) == Some(Side::Bottom)).map(|b| without_strip(m.rect, *b));
        match fresh {
            Some(f) if same(o.rect, f) || same(o.rect, m.rect) => out.put.push(o.rect),
            Some(f) => {
                out.put.push(f);
                out.refreshed += 1;
            }
            None if inside(o.rect, m.rect) => out.put.push(o.rect),
            None => out.dropped += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: [i32; 4] = [0, 0, 2560, 1440];
    const BAR: [i32; 4] = [0, 1392, 2560, 1440];

    fn mon(id: &str, rect: [i32; 4], work: [i32; 4], primary: bool) -> MonitorGeom {
        MonitorGeom { id: id.into(), rect, work, primary }
    }

    fn verdicts(p: &EdgePlan) -> Vec<Verdict> {
        p.monitors.iter().map(|m| m.verdict).collect()
    }

    // ---------------------------------------------------------------- the target

    #[test]
    fn the_band_is_cut_from_the_bottom_and_never_empties_the_monitor() {
        assert_eq!(monitor_less_band(A, 78), [0, 0, 2560, 1362]);
        assert_eq!(monitor_less_band(A, 0), [0, 0, 2560, 1439], "a band is at least one pixel");
        assert_eq!(monitor_less_band([0, 0, 100, 50], 500), [0, 0, 100, 1], "never an empty or inverted work area");
        assert_eq!(monitor_less_band([-1440, -200, 0, 2359], 78), [-1440, -200, 0, 2281], "a negative origin works the same");
    }

    #[test]
    fn the_target_keeps_a_band_on_the_primary_monitor_only_and_only_when_the_dock_reserves() {
        // dock_reserve on: the monitor less the dock's band
        assert_eq!(target_for(A, true, Some(78)), [0, 0, 2560, 1362]);
        // dock_reserve off: the whole monitor
        assert_eq!(target_for(A, true, None), A);
        // every other monitor whose taskbar we hid: the whole monitor, whatever the band is
        assert_eq!(target_for([-1440, -200, 0, 2359], false, Some(78)), [-1440, -200, 0, 2359]);
        assert_eq!(target_for([-1440, -200, 0, 2359], false, None), [-1440, -200, 0, 2359]);
    }

    #[test]
    fn the_numbers_of_the_users_pc_give_the_dock_the_bottom_edge() {
        // the monitor of the trial: 2560x1440, taskbar strip 48 px hidden, the dock's band 78 px registered with Explorer = 2560x1314
        let m = mon("A", A, [0, 0, 2560, 1314], true);
        let p = plan_edge(std::slice::from_ref(&m), &[BAR], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Set]);
        assert_eq!(p.monitors[0].target, [0, 0, 2560, 1362], "the work area ends where the band begins; no 48 px of strip under it");
        assert_eq!(p.monitors[0].user_work, [0, 0, 2560, 1392], "what the user has without the dock's band: the monitor less the taskbar");
        assert_eq!(p.dock_edge(Some(78)), Some([0, 1362, 2560, 1440]), "the dock's band is the bottom strip of the monitor");
        // the same with the dock's band already released: 2560x1392
        let m = mon("A", A, [0, 0, 2560, 1392], true);
        let p = plan_edge(std::slice::from_ref(&m), &[BAR], Some(78));
        assert_eq!((verdicts(&p), p.monitors[0].user_work), (vec![Verdict::Set], [0, 0, 2560, 1392]));
        // dock_reserve off: the whole monitor
        let p = plan_edge(&[m], &[BAR], None);
        assert_eq!(p.monitors[0].target, A);
        assert_eq!(p.dock_edge(None), Some([0, 1440, 2560, 1440]), "an empty strip at the bottom: the islands are still placed from the edge");
    }

    // ---------------------------------------------------------------- what is there

    #[test]
    fn a_work_area_that_is_the_target_already_is_as_wanted_and_remembers_the_users_own() {
        // a host restarted after an earlier one set it: nothing to set, but the band counts as ours and the way back is known
        let m = mon("A", A, [0, 0, 2560, 1362], true);
        let p = plan_edge(&[m], &[BAR], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::AsWanted]);
        assert_eq!(p.originals(), vec![WorkChange { id: "A".into(), from: [0, 0, 2560, 1392], to: [0, 0, 2560, 1362] }]);
        assert!(p.changes().is_empty() && p.primary_ours());
    }

    #[test]
    fn a_strip_that_explorer_does_not_reserve_is_set_too_and_the_whole_monitor_is_what_the_user_has() {
        let m = mon("A", A, A, true);
        let p = plan_edge(&[m], &[BAR], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Set]);
        // An auto-hiding taskbar that happens to be up looks like this: the bar is on the edge and the work area is the whole monitor.
        // The user's work area is the whole monitor; writing "monitor less the strip" down would put the work area 48 px short on the
        // way out.
        assert_eq!(p.monitors[0].user_work, A, "what is read is what is written down");
        assert_eq!(p.originals(), vec![WorkChange { id: "A".into(), from: A, to: [0, 0, 2560, 1362] }]);
    }

    #[test]
    fn the_work_area_read_as_the_target_already_keeps_the_whole_monitor_when_that_is_what_it_is() {
        // dock_reserve off, the target is the whole monitor, no earlier record: the monitor is what is there
        let m = mon("A", A, A, true);
        let p = plan_edge(&[m], &[BAR], None);
        assert_eq!((verdicts(&p), p.monitors[0].user_work), (vec![Verdict::AsWanted], A));
    }

    #[test]
    fn another_toolbar_means_the_monitor_is_left_alone() {
        // a third-party bar at the top of the screen: the work area is neither the taskbar-only one nor the whole monitor
        let m = mon("A", A, [0, 30, 2560, 1392], true);
        let p = plan_edge(&[m], &[BAR], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Foreign]);
        assert!(p.changes().is_empty() && p.originals().is_empty() && !p.any_ours() && !p.primary_ours());
        // a band much too thick to be a band (the work area is cut to a third)
        let m = mon("A", A, [0, 0, 2560, 400], true);
        assert_eq!(verdicts(&plan_edge(&[m], &[BAR], Some(78))), vec![Verdict::Foreign]);
        // on a monitor that is not the primary one a band-cut work area is not ours to explain: the dock sits on the primary
        let m = mon("B", A, [0, 0, 2560, 1314], false);
        assert_eq!(verdicts(&plan_edge(&[m], &[BAR], Some(78))), vec![Verdict::Foreign]);
    }

    #[test]
    fn a_taskbar_that_is_not_on_an_edge_or_a_monitor_without_ours_is_not_touched() {
        let m = mon("A", A, [0, 0, 2560, 1392], true);
        assert_eq!(verdicts(&plan_edge(std::slice::from_ref(&m), &[[100, 1392, 900, 1440]], Some(78))), vec![Verdict::Foreign], "a bar that does not span the monitor");
        assert_eq!(verdicts(&plan_edge(std::slice::from_ref(&m), &[[0, 600, 2560, 648]], Some(78))), vec![Verdict::Foreign], "not touching an edge");
        assert_eq!(verdicts(&plan_edge(std::slice::from_ref(&m), &[[5000, 1392, 7560, 1440]], Some(78))), vec![Verdict::NotOurs], "no monitor under it");
        assert_eq!(verdicts(&plan_edge(std::slice::from_ref(&m), &[], Some(78))), vec![Verdict::NotOurs], "no taskbar of ours at all");
        assert!(plan_edge(&[], &[BAR], Some(78)).monitors.is_empty());
    }

    #[test]
    fn two_monitors_are_decided_one_by_one_and_the_second_is_given_whole() {
        let a = mon("A", A, [0, 0, 2560, 1314], true);
        let b = mon("B", [-1440, -200, 0, 2359], [-1440, -200, 0, 2311], false);
        let bars = [BAR, [-1440, 2311, 0, 2359]];
        let p = plan_edge(&[a, b], &bars, Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Set, Verdict::Set]);
        let ch = p.changes();
        assert_eq!(ch[0].to, [0, 0, 2560, 1362]);
        assert_eq!(ch[1].to, [-1440, -200, 0, 2359], "the other monitor: the whole monitor, no band of ours");
        assert_eq!(p.originals()[1].from, [-1440, -200, 0, 2311]);
        // only the second monitor's taskbar was ours (the primary's was hidden by something else before): the dock is not told
        let a = mon("A", A, [0, 0, 2560, 1314], true);
        let b = mon("B", [-1440, -200, 0, 2359], [-1440, -200, 0, 2311], false);
        let p = plan_edge(&[a, b], &[[-1440, 2311, 0, 2359]], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::NotOurs, Verdict::Set]);
        assert!(!p.primary_ours(), "the primary monitor is not ours: the dock keeps its own registration there");
        assert!(p.any_ours());
    }

    #[test]
    fn a_taskbar_at_the_top_or_a_side_is_left_alone() {
        // portrait monitor to the left of the primary, taskbar on top: the dock sits at the bottom, there is nothing to take
        let m = mon("B", [-1440, -200, 0, 2359], [-1440, -152, 0, 2359], false);
        let p = plan_edge(&[m], &[[-1440, -200, 0, -152]], None);
        assert_eq!(verdicts(&p), vec![Verdict::Foreign]);
        assert!(p.changes().is_empty() && p.originals().is_empty() && !p.any_ours());
        // taskbar on the left of the primary monitor
        let m = mon("C", [0, 0, 1920, 1080], [48, 0, 1920, 1080], true);
        let p = plan_edge(&[m], &[[0, 0, 48, 1080]], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Foreign]);
        assert!(!p.primary_ours() && p.changes().is_empty(), "the dock keeps its own registration");
        // taskbar on the right
        let m = mon("D", [0, 0, 1920, 1080], [0, 0, 1872, 1080], true);
        assert_eq!(verdicts(&plan_edge(&[m], &[[1872, 0, 1920, 1080]], None)), vec![Verdict::Foreign]);
        // taskbar on top of the primary monitor with the dock's band registered at the bottom
        let m = mon("E", [0, 0, 1920, 1080], [0, 48, 1920, 1002], true);
        let p = plan_edge(&[m], &[[0, 0, 1920, 48]], Some(78));
        assert_eq!(verdicts(&p), vec![Verdict::Foreign]);
        assert!(!p.primary_ours());
    }

    #[test]
    fn a_band_still_held_by_explorer_is_told_apart_from_the_target_that_looks_like_one() {
        // the dock's band is registered: strip and band are both taken from the work area
        // a band thinner than the strip: unknown without the host's memory, the host's own with it
        let thin = [0, 0, 2560, 1440 - 40];
        assert_eq!(plan_edge(&[mon("A", A, thin, true)], &[BAR], Some(78)).monitors[0].verdict, Verdict::Foreign);
        let p = plan_edge_held(&[mon("A", A, thin, true)], &[BAR], Some(78), &[thin]);
        assert_eq!(p.monitors[0].verdict, Verdict::Set);
        assert_eq!(p.monitors[0].user_work, [0, 0, 2560, 1392], "the user's own rectangle is still the monitor less the strip");
        assert_eq!(plan_edge_held(&[mon("A", A, thin, true)], &[BAR], Some(78), &[[0, 0, 2560, 1300]]).monitors[0].verdict, Verdict::Foreign, "another rectangle is not this one");
        let p = plan_edge(&[mon("A", A, [0, 0, 2560, 1314], true)], &[BAR], Some(78));
        assert!(p.monitors[0].band_held);
        // the dock let go and Explorer recomputed: the strip alone
        let p = plan_edge(&[mon("A", A, [0, 0, 2560, 1392], true)], &[BAR], Some(78));
        assert!(!p.monitors[0].band_held);
        // the work area is our own target (which is a band cut from the monitor): that is ours, not a band Explorer holds
        let p = plan_edge(&[mon("A", A, [0, 0, 2560, 1362], true)], &[BAR], Some(78));
        assert!(!p.monitors[0].band_held && p.monitors[0].verdict == Verdict::AsWanted);
        // other monitors never hold a band of the dock's
        let p = plan_edge(&[mon("B", A, [0, 0, 2560, 1314], false)], &[BAR], Some(78));
        assert!(!p.monitors[0].band_held);
    }

    // ---------------------------------------------------------------- the way out

    fn rec(id: &str, rect: [i32; 4]) -> WorkAreaRec {
        WorkAreaRec { monitor: id.into(), rect }
    }

    #[test]
    fn work_areas_that_still_fit_their_monitors_are_put_back_as_written() {
        let a = mon("A", A, [0, 0, 2560, 1362], true);
        let b = mon("B", [-1440, -200, 0, 2359], [-1440, -200, 0, 2359], false);
        let bars = [BAR, [-1440, 2311, 0, 2359]];
        let r = work_restore(&[rec("A", [0, 0, 2560, 1392]), rec("B", [-1440, -200, 0, 2311])], &[a, b], &bars);
        assert_eq!(r, WorkRestore { put: vec![[0, 0, 2560, 1392], [-1440, -200, 0, 2311]], refreshed: 0, dropped: 0 });
    }

    #[test]
    fn a_whole_monitor_that_was_written_down_for_an_auto_hiding_taskbar_stays_the_whole_monitor() {
        let a = mon("A", A, [0, 0, 2560, 1362], true);
        let r = work_restore(&[rec("A", A)], &[a], &[BAR]);
        assert_eq!(r, WorkRestore { put: vec![A], refreshed: 0, dropped: 0 });
    }

    #[test]
    fn a_monitor_that_changed_resolution_gets_a_work_area_worked_out_from_its_taskbar_not_the_old_rectangle() {
        // 2560x1440 became 1920x1080 while the hide was in force: the taskbar is 48 px at the new bottom
        let a = mon("A", [0, 0, 1920, 1080], [0, 0, 1920, 1080], true);
        let r = work_restore(&[rec("A", [0, 0, 2560, 1392])], &[a], &[[0, 1032, 1920, 1080]]);
        assert_eq!(r, WorkRestore { put: vec![[0, 0, 1920, 1032]], refreshed: 1, dropped: 0 });
        // and the other way: 1920x1080 became 2560x1440; the old rectangle still lies inside the monitor and is still wrong
        let a = mon("A", A, A, true);
        let r = work_restore(&[rec("A", [0, 0, 1920, 1032])], &[a], &[BAR]);
        assert_eq!(r, WorkRestore { put: vec![[0, 0, 2560, 1392]], refreshed: 1, dropped: 0 });
        // the scale changed: the taskbar is thicker
        let a = mon("A", A, A, true);
        let r = work_restore(&[rec("A", [0, 0, 2560, 1392])], &[a], &[[0, 1368, 2560, 1440]]);
        assert_eq!(r.put, vec![[0, 0, 2560, 1368]]);
    }

    #[test]
    fn a_monitor_that_is_gone_gets_nothing_and_nothing_is_set_on_another_monitor_in_its_place() {
        let a = mon("A", A, [0, 0, 2560, 1362], true);
        let r = work_restore(&[rec("A", [0, 0, 2560, 1392]), rec("B", [-1440, -200, 0, 2311])], &[a], &[BAR]);
        assert_eq!(r, WorkRestore { put: vec![[0, 0, 2560, 1392]], refreshed: 0, dropped: 1 });
    }

    #[test]
    fn without_a_taskbar_to_measure_a_rectangle_is_kept_only_when_it_can_belong_to_its_monitor() {
        let a = mon("A", A, A, true);
        assert_eq!(work_restore(&[rec("A", [0, 0, 2560, 1392])], std::slice::from_ref(&a), &[]).put, vec![[0, 0, 2560, 1392]]);
        let r = work_restore(&[rec("A", [0, 0, 3840, 2100]), rec("A", [0, 10, 5, 10])], &[a], &[]);
        assert_eq!((r.put.len(), r.dropped), (0, 2), "bigger than the monitor, and an empty one");
    }

    #[test]
    fn with_no_monitor_known_the_written_down_rectangles_are_put_back_as_they_are() {
        let r = work_restore(&[rec("A", [0, 0, 2560, 1392])], &[], &[BAR]);
        assert_eq!(r, WorkRestore { put: vec![[0, 0, 2560, 1392]], refreshed: 0, dropped: 0 });
    }

    #[test]
    fn a_band_whose_thickness_changed_since_it_was_registered_is_still_the_dock_s_own() {
        // the dock was resized: the registered band is 92 px, the new wish 78
        let m = mon("A", A, [0, 0, 2560, 1300], true);
        let p = plan_edge(&[m], &[BAR], Some(78));
        assert_eq!(p.monitors[0].verdict, Verdict::Set);
        assert_eq!(p.monitors[0].user_work, [0, 0, 2560, 1392]);
    }

    #[test]
    fn the_four_combinations_of_reserve_and_monitor_in_one_table() {
        // (primary?, dock_reserve?) -> the work area we set for a monitor 2560x1440 whose 48 px taskbar we hid
        for (primary, band, want) in [(true, Some(78), [0, 0, 2560, 1362]), (true, None, A), (false, Some(78), A), (false, None, A)] {
            let m = mon("X", A, [0, 0, 2560, 1392], primary);
            let p = plan_edge(&[m], &[BAR], band);
            assert_eq!(p.monitors[0].target, want, "primary {primary}, band {band:?}");
            assert_eq!(p.changes().len(), 1);
        }
    }
}
