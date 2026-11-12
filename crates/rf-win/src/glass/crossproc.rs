//! Glass windows of OTHER processes of ours: which of them a process leaves out of its own capture.
//!
//! THE PROBLEM THIS SOLVES. A capture session's exclusion list is per process (`wgc.rs`): it holds the glass windows this process
//! attached and its popups. Every other window is in the picture, a glass window of another process too, and that window's glass is a
//! picture of the screen behind IT. Two glass windows of two processes (two instance groups, a test copy, the dock and a file window)
//! that lie over each other therefore read each other: the front window's backdrop holds the back window's glass, which holds the front
//! window's glass, one frame older, and so on. The glass's grade (saturation above one, a contrast, a tint) is a gain above one on the
//! colours, and its dispersion shifts the red and the blue channel one way and the other every time round, so after a few dozen frames
//! the overlap is a flat saturated red, going over magenta into a flat blue, instead of a blurred picture. Inside one process this
//! cannot happen: all its glass windows are in the exclusion list, so each sees the screen behind all of them.
//!
//! THE RULE. A window X of another process is left out of this process's capture when it lies ABOVE one of this process's glass windows
//! W in the z order and near it (its rectangle meets W's, grown by the margin the glass reads around itself). The window that is
//! above is then seen by the window below it as nothing (the screen behind it), and sees the window below as it is shown. Two such
//! windows never read each other, so there is no loop, and where a glass window is over another one it shows that window as the user
//! sees it, blurred like anything else. Why this order and not the other: what is below is hidden by what is above, so taking the upper
//! window out of the lower one's picture only changes the lower glass at the edge of the overlap, through its frost; the upper glass is
//! the one that has to show the lower window. Windows that are not near each other do not enter the lists, so a click that only
//! brings one of two side by side windows to the front sends nothing.
//!
//! No cycle can form with more windows: a window's picture holds only windows of other processes that are not above (and near) any of
//! its process's glass windows, and so lie below its process's lowest glass window, and the same holds for each of those in turn: the
//! lowest window of the chain goes down at every step.
//!
//! THE CHANGE OF AN ORDER. Two processes scan on clocks of their own, so when a window is brought to the front one of them learns it
//! before the other. Adding a window to the list is immediate, taking one out waits `HOLD`: during a change both leave each other out
//! for a moment (a window shows what is behind both, which is what a glass over a glass of the same process always shows), and never
//! neither (a loop). `Foreign::update` is that rule and a test steps two clocks through every phase and a send latency up to 100 ms;
//! another test shows that without the hold some phases do loop.
//!
//! WHAT EACH PATH SHOWS. Session capture, windows of two processes: the front glass shows the back window as the user sees it, blurred
//! (the back window, below, shows what is behind the front one, which is hidden under it anyway). Two glass windows of ONE process: both
//! are in the process's list, so each shows what is behind both (one capture and one list per process cannot give two pictures); no loop.
//! Desktop Duplication: every glass window carries `WDA_EXCLUDEFROMCAPTURE`, so nobody's capture holds any of them: what is behind both
//! again, no loop, and nothing here is used. If the capture service refuses a list that has windows of other processes, the list is sent
//! without them (`send_list`): the old picture (and the old loop) for that session, never a session that falls back to Desktop
//! Duplication. A build without the mark (an older rustyfiles that still runs, a dock that outlived an update) is not seen: its windows
//! are not left out and it does not leave ours out, so against such a process the loop of old remains until it is restarted.
//!
//! HOW A PROCESS KNOWS THE OTHERS. Every glass window carries a window property (`PROP`, the value is its process id) from the moment
//! it is attached until its glass ends. A window property lives and dies with its window, so there is nothing to clean up after a crash
//! and a handle that is handed to another window later does not carry it. The engine looks at the top-level windows (`scan`, one
//! `GetPropW` per window and only a rectangle for the few that carry the property) every `SCAN_EVERY` while some glass is seen.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, RECT};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetPropW, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible, RemovePropW, SetPropW};
use windows::core::{BOOL, w};

use super::geom::Rc;

/// The window property glass windows carry. The version is in the name: another meaning of the value would be another name.
macro_rules! prop {
    () => {
        w!("rustyfiles.glass.v1")
    };
}

/// How often the engine looks at the other processes' windows while some glass is seen. A new window of another process is left out
/// of the capture at most this long (plus the engine's own turn, and one send of the list) after it showed. The turn is at most the
/// quiet wait of 50 ms while a glass window of another process is known; when none was seen at the last scan and no glass is in front
/// it may be `rest::BEHIND_QUIET_WAIT_MS` (100 ms, `rest::quiet_wait_ms`): the first such window is then noticed up to about 150 ms
/// after it showed, still well inside `HOLD`.
pub(super) const SCAN_EVERY: Duration = Duration::from_millis(50);

/// How long a window that no longer has to be left out stays in the list. Two processes scan on their own clocks, so after a window
/// went to the front, the process that now has to START leaving the other out learns it up to one scan (plus a turn and a send) after
/// the process that now has to STOP does. If the stopping one let go at once, both pictures would hold each other for that time (the
/// very loop this file prevents, a flash on every change of the z order). Letting go only after this long makes the order the safe
/// one: both leave each other out for a moment (a window shows what is behind both, as inside one process), never neither. It is far
/// more than the latency of a start (`SCAN_EVERY` + the engine's turn, 50 ms, or 100 ms before the first foreign window is known,
/// + a send of a few ms); a test steps both clocks through it, with a latency up to 100 ms.
pub(super) const HOLD: Duration = Duration::from_millis(250);

/// Marks a window as one that has glass (`true`) or takes the mark off. Cheap, from any thread; a handle that is not a window of this
/// process is left alone.
pub(crate) fn mark(hwnd: isize, on: bool) {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let mut pid = 0u32;
        if hwnd == 0 || GetWindowThreadProcessId(h, Some(&mut pid)) == 0 || pid != GetCurrentProcessId() {
            return;
        }
        if on {
            let _ = SetPropW(h, prop!(), Some(HANDLE(pid as usize as *mut _)));
        } else {
            let _ = RemovePropW(h, prop!());
        }
    }
}

/// Whether the window carries the mark (any process's: the property can be read across processes).
fn is_marked(h: HWND) -> bool {
    unsafe { !GetPropW(h, prop!()).is_invalid() }
}

/// One window in the z order, as the pure rule sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Entry {
    pub hwnd: isize,
    /// A glass window of this process (otherwise: of another process of ours).
    pub mine: bool,
    /// Where it is on the screen.
    pub rect: Rc,
}

/// The windows of other processes that this process leaves out of its capture: of the entries given from the TOP of the z order down,
/// every foreign one that lies above one of ours and, grown by `margin` px on every side, meets it. See the module docs for why.
pub(super) fn foreign_above(order: &[Entry], margin: i32) -> BTreeSet<isize> {
    let grow = |r: Rc| Rc::new(r.l.saturating_sub(margin), r.t.saturating_sub(margin), r.r.saturating_add(margin), r.b.saturating_add(margin));
    let mut out = BTreeSet::new();
    for (i, e) in order.iter().enumerate().filter(|(_, e)| !e.mine) {
        let near = grow(e.rect);
        if order[i + 1..].iter().any(|m| m.mine && near.meets(&m.rect)) {
            out.insert(e.hwnd);
        }
    }
    out
}

/// The windows of other processes this process is leaving out, with the delay for taking one out again (see `HOLD`).
#[derive(Default)]
pub(super) struct Foreign {
    set: BTreeSet<isize>,
    /// Listed windows that the rule no longer wants, and since when.
    leaving: BTreeMap<isize, Instant>,
    /// Another hold than `HOLD` (a test that shows what no hold does).
    hold: Option<Duration>,
}

impl Foreign {
    #[cfg(test)]
    fn with_hold(hold: Duration) -> Foreign {
        Foreign { hold: Some(hold), ..Foreign::default() }
    }

    fn hold(&self) -> Duration {
        self.hold.unwrap_or(HOLD)
    }

    pub(super) fn set(&self) -> &BTreeSet<isize> {
        &self.set
    }

    /// Takes in what the rule wants now: a window that is wanted is listed at once; one that is not wanted any more leaves `hold` after
    /// it was first seen unwanted, or at once when it is no window any more (its number could be handed to somebody else's window,
    /// which would then be cut out of the picture). Returns whether the set changed.
    pub(super) fn update(&mut self, wanted: &BTreeSet<isize>, now: Instant, mut exists: impl FnMut(isize) -> bool) -> bool {
        let mut changed = false;
        for &h in wanted {
            self.leaving.remove(&h);
            changed |= self.set.insert(h);
        }
        let unwanted: Vec<isize> = self.set.iter().copied().filter(|h| !wanted.contains(h)).collect();
        for h in unwanted {
            let since = *self.leaving.entry(h).or_insert(now);
            if now.saturating_duration_since(since) >= self.hold() || !exists(h) {
                self.set.remove(&h);
                self.leaving.remove(&h);
                changed = true;
            }
        }
        changed
    }
}

/// Sends an exclusion list that may hold windows of other processes. The capture service is not documented to honour a handle that
/// belongs to another process; when a send that carried such windows is refused, the same list is sent without them and `refused`
/// remembers it, so a refusal costs the picture (the loop is back) but never the session: an error here would make the engine give up
/// the session backend, which hides every glass window from screenshots and recordings. `send(with_foreign)` makes one call. An error
/// of the send without foreign windows is returned as it is.
pub(super) fn send_list<T, E>(refused: &mut bool, has_foreign: bool, mut send: impl FnMut(bool) -> Result<T, E>) -> Result<T, E> {
    if !has_foreign || *refused {
        return send(false);
    }
    match send(true) {
        Ok(v) => Ok(v),
        Err(_) => {
            let v = send(false)?;
            *refused = true;
            log::warn!("glass: the capture service refused an exclusion list with windows of other processes; leaving them out is off for this session");
            Ok(v)
        }
    }
}

struct Walk<'a> {
    mine: &'a BTreeSet<isize>,
    pid: u32,
    out: Vec<Entry>,
}

unsafe extern "system" fn each(h: HWND, lp: LPARAM) -> BOOL {
    let w = unsafe { &mut *(lp.0 as *mut Walk) };
    let key = h.0 as isize;
    let mine = w.mine.contains(&key);
    unsafe {
        // the property first: it is the cheap test that rules out nearly every window of the desktop
        if !mine && !is_marked(h) {
            return BOOL(1);
        }
        if !IsWindowVisible(h).as_bool() || IsIconic(h).as_bool() {
            return BOOL(1);
        }
        if !mine {
            let mut pid = 0u32;
            if GetWindowThreadProcessId(h, Some(&mut pid)) == 0 || pid == w.pid {
                // a window of this very process that carries the mark but is not one the engine holds: it is not "another process"
                return BOOL(1);
            }
        }
        let mut r = RECT::default();
        if GetWindowRect(h, &mut r).is_ok() {
            w.out.push(Entry { hwnd: key, mine, rect: Rc::new(r.left, r.top, r.right, r.bottom) });
        }
    }
    BOOL(1)
}

/// The visible glass windows of this process (`mine`) and of other processes, from the top of the z order down. Reads windows only.
pub(super) fn scan(mine: &BTreeSet<isize>) -> Vec<Entry> {
    let mut walk = Walk { mine, pid: unsafe { GetCurrentProcessId() }, out: Vec::new() };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(hwnd: isize, mine: bool, l: i32, t: i32, r: i32, b: i32) -> Entry {
        Entry { hwnd, mine, rect: Rc::new(l, t, r, b) }
    }

    /// The case of the report: a window of another process in front of ours, overlapping it. Ours leaves the front one out of its
    /// capture; the front one (its own process looks at the same z order) does not leave ours out.
    #[test]
    fn the_window_in_front_is_left_out_by_the_one_behind_and_not_the_other_way_round() {
        let front = 1;
        let back = 2;
        // z order, top first. The process of `back` sees `front` as foreign
        let seen_by_back = [e(front, false, 300, 150, 1300, 790), e(back, true, 40, 40, 1040, 680)];
        assert_eq!(foreign_above(&seen_by_back, 0), BTreeSet::from([front]));
        // the process of `front` sees `back` as foreign, below its own window
        let seen_by_front = [e(front, true, 300, 150, 1300, 790), e(back, false, 40, 40, 1040, 680)];
        assert!(foreign_above(&seen_by_front, 0).is_empty(), "the window in front must see the one behind it");
    }

    /// Both processes apply the rule to the same z order, so what one leaves out the other does not: that is what keeps the two
    /// pictures from depending on each other, whatever the order.
    #[test]
    fn two_processes_never_leave_out_each_other() {
        for front_first in [true, false] {
            let a = |mine| e(10, mine, 0, 0, 500, 500);
            let b = |mine| e(20, mine, 100, 100, 600, 600);
            let (from_a, from_b) = if front_first {
                ([a(true), b(false)], [a(false), b(true)])
            } else {
                ([b(false), a(true)], [b(true), a(false)])
            };
            let a_leaves_out_b = foreign_above(&from_a, 0).contains(&20);
            let b_leaves_out_a = foreign_above(&from_b, 0).contains(&10);
            assert!(a_leaves_out_b != b_leaves_out_a, "exactly one of the two leaves the other out (front_first = {front_first})");
        }
    }

    #[test]
    fn a_foreign_window_that_is_not_near_is_not_listed() {
        let order = [e(1, false, 1000, 0, 1500, 400), e(2, true, 0, 0, 500, 400)];
        assert!(foreign_above(&order, 0).is_empty(), "side by side");
        assert!(foreign_above(&order, 400).is_empty(), "500 px apart, beyond a 400 px margin");
        assert_eq!(foreign_above(&order, 501), BTreeSet::from([1]), "within a 501 px margin");
    }

    #[test]
    fn the_margin_catches_a_window_whose_frost_reaches_ours() {
        // 20 px apart: not overlapping, but the glass reads about a hundred px around itself
        let order = [e(1, false, 520, 0, 1000, 400), e(2, true, 0, 0, 500, 400)];
        assert!(foreign_above(&order, 0).is_empty());
        assert_eq!(foreign_above(&order, 96), BTreeSet::from([1]));
    }

    #[test]
    fn a_foreign_window_below_ours_is_never_listed() {
        let order = [e(2, true, 0, 0, 500, 400), e(1, false, 100, 100, 600, 500)];
        assert!(foreign_above(&order, 96).is_empty());
    }

    /// A foreign window between two of ours is above the lower one: it is left out (the one above it sees nothing of it, as inside a
    /// process, which is the price of one capture per process).
    #[test]
    fn a_foreign_window_between_two_of_ours_is_left_out() {
        let order = [e(3, true, 0, 0, 800, 600), e(1, false, 100, 100, 600, 500), e(2, true, 50, 50, 700, 550)];
        assert_eq!(foreign_above(&order, 0), BTreeSet::from([1]));
    }

    #[test]
    fn nothing_of_ours_means_nothing_listed() {
        let order = [e(1, false, 0, 0, 100, 100), e(2, false, 0, 0, 100, 100)];
        assert!(foreign_above(&order, 96).is_empty());
        assert!(foreign_above(&[], 96).is_empty());
    }

    #[test]
    fn a_huge_margin_does_not_overflow() {
        let order = [e(1, false, i32::MAX - 5, 0, i32::MAX, 10), e(2, true, 0, 0, 10, 10)];
        let _ = foreign_above(&order, i32::MAX);
    }

    /// The loop-freeness of the rule over a whole stack of windows of three processes: whatever the z order, the "who sees whom" graph
    /// has no cycle, so no picture depends on itself. (A window of process P sees a window of Q when Q's window is not left out by P.)
    #[test]
    fn no_picture_depends_on_itself_for_any_stack_of_three_processes() {
        // windows: process 0 has ids 0 and 1, process 1 has 2, process 2 has 3 and 4; all overlap each other
        let proc_of = [0usize, 0, 1, 2, 2];
        let rect = Rc::new(0, 0, 100, 100);
        let mut orders: Vec<Vec<usize>> = Vec::new();
        fn perms(cur: &mut Vec<usize>, left: &mut Vec<usize>, out: &mut Vec<Vec<usize>>) {
            if left.is_empty() {
                out.push(cur.clone());
                return;
            }
            for i in 0..left.len() {
                let x = left.remove(i);
                cur.push(x);
                perms(cur, left, out);
                cur.pop();
                left.insert(i, x);
            }
        }
        perms(&mut Vec::new(), &mut (0..5).collect(), &mut orders);
        for order in orders {
            // sees[p] = the processes whose windows process p's picture holds
            let mut sees = [[false; 3]; 3];
            for (p, row) in sees.iter_mut().enumerate() {
                let es: Vec<Entry> = order.iter().map(|&w| Entry { hwnd: w as isize, mine: proc_of[w] == p, rect }).collect();
                let left_out = foreign_above(&es, 0);
                for &w in &order {
                    if proc_of[w] != p && !left_out.contains(&(w as isize)) {
                        row[proc_of[w]] = true;
                    }
                }
            }
            // a cycle of any length in `sees` is a loop of pictures
            for a in 0..3 {
                for b in 0..3 {
                    assert!(!(a != b && sees[a][b] && sees[b][a]), "two processes see each other in the order {order:?}");
                    for c in 0..3 {
                        assert!(!(a != b && b != c && c != a && sees[a][b] && sees[b][c] && sees[c][a]), "a cycle of three in the order {order:?}");
                    }
                }
            }
        }
    }

    /// A real window of this process, never shown: a message-only window of a system class.
    fn hidden_window() -> HWND {
        use windows::Win32::UI::WindowsAndMessaging::{CreateWindowExW, HWND_MESSAGE, WINDOW_EX_STYLE, WS_POPUP};
        unsafe { CreateWindowExW(WINDOW_EX_STYLE(0), w!("STATIC"), w!("glass-test"), WS_POPUP, 0, 0, 10, 10, Some(HWND_MESSAGE), None, None, None).expect("a message-only window") }
    }

    #[test]
    fn marking_a_window_sets_and_takes_off_the_property() {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let h = hidden_window();
        assert!(!is_marked(h));
        mark(h.0 as isize, true);
        assert!(is_marked(h), "the mark is on after mark(true)");
        mark(h.0 as isize, false);
        assert!(!is_marked(h), "and off after mark(false)");
        unsafe { DestroyWindow(h).expect("destroy") };
    }

    #[test]
    fn marking_a_handle_that_is_no_window_does_nothing() {
        mark(0, true);
        mark(0x7fff_fff0, true);
        mark(0x7fff_fff0, false);
        assert!(!is_marked(HWND(0x7fff_fff0 as *mut _)));
        assert!(scan(&BTreeSet::new()).iter().all(|e| !e.mine));
    }

    /// A marked window that is not visible is not listed.
    #[test]
    fn a_hidden_marked_window_is_not_scanned() {
        use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;
        let h = hidden_window();
        mark(h.0 as isize, true);
        let seen = scan(&BTreeSet::from([h.0 as isize]));
        assert!(seen.iter().all(|e| e.hwnd != h.0 as isize), "a window that is not visible is not listed");
        unsafe { DestroyWindow(h).expect("destroy") };
    }

    fn set_of(v: &[isize]) -> BTreeSet<isize> {
        v.iter().copied().collect()
    }

    #[test]
    fn a_wanted_window_is_listed_at_once_and_an_unwanted_one_leaves_after_the_hold() {
        let t0 = Instant::now();
        let mut f = Foreign::default();
        assert!(f.update(&set_of(&[7]), t0, |_| true), "listed at once");
        assert!(!f.update(&set_of(&[7]), t0 + Duration::from_millis(10), |_| true), "nothing new, nothing to send");
        assert!(!f.update(&set_of(&[]), t0 + Duration::from_millis(20), |_| true), "no longer wanted: still listed");
        assert!(f.set().contains(&7));
        assert!(!f.update(&set_of(&[]), t0 + Duration::from_millis(20) + HOLD - Duration::from_millis(1), |_| true));
        assert!(f.update(&set_of(&[]), t0 + Duration::from_millis(20) + HOLD, |_| true), "it leaves after the hold");
        assert!(f.set().is_empty());
    }

    /// A window that comes back inside the hold stays, and its clock starts again the next time it is unwanted.
    #[test]
    fn a_window_wanted_again_inside_the_hold_stays_and_restarts_the_hold() {
        let t0 = Instant::now();
        let mut f = Foreign::default();
        f.update(&set_of(&[7]), t0, |_| true);
        f.update(&set_of(&[]), t0 + Duration::from_millis(10), |_| true);
        f.update(&set_of(&[7]), t0 + Duration::from_millis(200), |_| true);
        assert!(!f.update(&set_of(&[]), t0 + Duration::from_millis(210) + Duration::from_millis(100), |_| true), "the clock started again at 210 ms");
        assert!(f.set().contains(&7));
    }

    /// A handle that is no window any more goes at once (the number may belong to another program's window by now).
    #[test]
    fn a_window_that_is_gone_leaves_at_once() {
        let t0 = Instant::now();
        let mut f = Foreign::default();
        f.update(&set_of(&[7, 8]), t0, |_| true);
        assert!(f.update(&set_of(&[]), t0 + Duration::from_millis(1), |h| h != 7));
        assert_eq!(f.set(), &set_of(&[8]));
    }

    /// The two processes of the report, stepped through a change of the z order with their scans on clocks of their own. Window 1 is
    /// P's, window 2 is Q's. Before the change P is in front (or Q, with `p_goes_back` false); at 1000 ms the order flips. Each process
    /// scans every `SCAN_EVERY` from its own phase and its new list reaches its capture `latency` ms later. Returns the number of
    /// milliseconds in which NEITHER capture left the other's glass out: both pictures then hold each other (the loop of the report).
    fn both_include_ms(hold: Duration, phase_p: u64, phase_q: u64, latency: u64, p_goes_back: bool) -> u32 {
        let base = Instant::now();
        let at = |ms: u64| base + Duration::from_millis(ms);
        let every = SCAN_EVERY.as_millis() as u64;
        let (mut fp, mut fq) = (Foreign::with_hold(hold), Foreign::with_hold(hold));
        // what each capture leaves out as of the last list that reached it; before the change the one behind leaves the front one out
        let (mut p_excludes_q, mut q_excludes_p) = (!p_goes_back, p_goes_back);
        let mut pending_p: Vec<(u64, bool)> = vec![];
        let mut pending_q: Vec<(u64, bool)> = vec![];
        let mut both = 0;
        for ms in 0..3000u64 {
            let p_front = (ms < 1000) == p_goes_back;
            // the rule's answer: a process leaves the other's window out when it is above its own
            let want = |other_above: bool, w: isize| if other_above { set_of(&[w]) } else { set_of(&[]) };
            if ms % every == phase_p % every && fp.update(&want(!p_front, 2), at(ms), |_| true) {
                pending_p.push((ms + latency, fp.set().contains(&2)));
            }
            if ms % every == phase_q % every && fq.update(&want(p_front, 1), at(ms), |_| true) {
                pending_q.push((ms + latency, fq.set().contains(&1)));
            }
            pending_p.retain(|&(due, v)| {
                if due <= ms {
                    p_excludes_q = v;
                }
                due > ms
            });
            pending_q.retain(|&(due, v)| {
                if due <= ms {
                    q_excludes_p = v;
                }
                due > ms
            });
            if !p_excludes_q && !q_excludes_p {
                both += 1;
            }
        }
        both
    }

    /// Without the hold (the first version of the rule) a change of the z order makes a loop of up to a scan in some phases: this test
    /// is able to see it, so the next one means something.
    #[test]
    fn without_the_hold_a_change_of_the_order_makes_a_loop_in_some_phases() {
        let mut worst = 0;
        for phase_q in (0..50).step_by(5) {
            for phase_p in (0..50).step_by(5) {
                worst = worst.max(both_include_ms(Duration::ZERO, phase_p, phase_q, 5, true));
            }
        }
        assert!(worst > 0, "with no hold some interleaving must show both pictures holding each other");
    }

    /// With the hold there is no moment in which both hold each other, whatever the phases of the two scans, the direction of the change
    /// and a latency of the send of up to 100 ms.
    #[test]
    fn with_the_hold_the_two_pictures_never_hold_each_other() {
        for p_goes_back in [true, false] {
            for latency in [0, 5, 40, 100] {
                for phase_q in (0..50).step_by(3) {
                    for phase_p in (0..50).step_by(3) {
                        assert_eq!(both_include_ms(HOLD, phase_p, phase_q, latency, p_goes_back), 0, "phases {phase_p}/{phase_q}, latency {latency} ms, p_goes_back {p_goes_back}");
                    }
                }
            }
        }
    }

    #[test]
    fn a_refused_list_with_foreign_windows_is_sent_again_without_them_and_remembered() {
        let mut refused = false;
        let mut calls = vec![];
        let r: Result<u32, &str> = send_list(&mut refused, true, |with| {
            calls.push(with);
            if with { Err("refused") } else { Ok(5) }
        });
        assert_eq!(r, Ok(5));
        assert_eq!(calls, vec![true, false]);
        assert!(refused);
        // the next list does not try again
        let mut calls = vec![];
        let _: Result<u32, &str> = send_list(&mut refused, true, |with| {
            calls.push(with);
            Ok(1)
        });
        assert_eq!(calls, vec![false]);
    }

    #[test]
    fn a_list_that_fails_without_foreign_windows_too_is_an_error_and_nothing_is_remembered() {
        let mut refused = false;
        let r: Result<u32, &str> = send_list(&mut refused, true, |_| Err("broken"));
        assert_eq!(r, Err("broken"));
        assert!(!refused, "a session that is broken is not a refusal of foreign windows");
        // a list with no foreign windows is one call
        let mut n = 0;
        let _: Result<u32, &str> = send_list(&mut refused, false, |with| {
            n += 1;
            assert!(!with);
            Ok(0)
        });
        assert_eq!(n, 1);
    }

    /// The session's list holds the foreign windows, each once, and a change of them is a new version to send.
    #[test]
    fn the_exclusion_list_carries_the_foreign_windows_and_a_change_is_a_new_version() {
        use super::super::wgc::Exclusion;
        let mut ex = Exclusion::default();
        ex.attach(10);
        let v = ex.version();
        assert!(ex.set_foreign(&set_of(&[30, 10])));
        assert!(ex.version() > v);
        assert_eq!(ex.list(), vec![10, 30], "attached and foreign, each once");
        assert_eq!(ex.list_for(false), vec![10]);
        assert!(ex.has_foreign());
        let v = ex.version();
        assert!(!ex.set_foreign(&set_of(&[30, 10])), "the same set sends nothing");
        assert_eq!(ex.version(), v);
        assert!(ex.set_foreign(&set_of(&[])));
        assert!(!ex.has_foreign());
    }
}
