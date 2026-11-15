//! A window of another program moved to a monitor, or brought inside its monitor's work area: two kinds of row in a dock icon's
//! menu (the user, 2026-11-16: "docka şunu ekleyelim: Monitörde aç: 1, 2 ,3 butonları. pencerenin hangi monitöre taşıyabileceğimizi
//! ayarlayabilelim. ekrana sığdır filan da olabilir").
//!
//! WHICH NUMBER A MONITOR HAS: the number in its device name (`\\.\DISPLAY2` is 2), which is the number Windows' display settings
//! show for it in the usual case. A name with no number takes its place in the list.
//!
//! WHERE A MOVED WINDOW GOES ([`moved_rect`]): it keeps its size (made smaller only when the other monitor's work area is smaller)
//! and the same share of the free room around it: a window in the middle of one monitor is in the middle of the other, one in a
//! corner stays in that corner. A maximized window is maximized on the other monitor.
//!
//! AN APP THAT IS NOT RUNNING ("ekle bence", of starting one on a chosen monitor): nobody can tell another program where to open
//! its window. The app is started as always, and the first window of its program that was not there before ([`new_window_of`],
//! waited for by [`wait_for_new_window`]) is moved to the monitor. The window is seen where the program opened it for a moment
//! and then goes over. A program whose window belongs to another program than the one that was started (a launcher, a copy that
//! hands over to one already running, a packaged app) is not found, and its window stays where it opened.
//!
//! WHAT "FIT" DOES ([`fitted_rect`]): a window larger than its monitor's work area is made as large as it, and a window that hangs
//! over an edge (or sits beyond it, after a monitor was unplugged) is moved inside. A window that is inside already stays as it is.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::layout::Rect;
use super::model::Launch;
use super::monitors::{self, Display};
use super::winops::WinRef;

/// A monitor a window can be sent to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Screen {
    pub number: u32,
    /// Its `HMONITOR`.
    pub handle: isize,
    pub work: Rect,
}

/// The number at the end of a display's device name (`\\.\DISPLAY12` -> 12).
pub fn display_number(name: &str) -> Option<u32> {
    let digits = name.len() - name.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    name[name.len() - digits..].parse().ok().filter(|n| *n > 0)
}

/// The displays with their numbers, in the order of the numbers.
pub fn numbered(displays: &[Display]) -> Vec<Screen> {
    let mut v: Vec<Screen> = displays
        .iter()
        .enumerate()
        .map(|(i, d)| Screen { number: display_number(&d.name).unwrap_or(i as u32 + 1), handle: d.info.handle, work: d.info.work })
        .collect();
    v.sort_by_key(|s| s.number);
    v
}

/// The monitors now.
pub fn screens() -> Vec<Screen> {
    numbered(&monitors::displays())
}

/// The numbers of the monitors a window that is on `on` (a monitor's handle) can be sent to: all the others. None with one monitor.
pub fn targets(screens: &[Screen], on: Option<isize>) -> Vec<u32> {
    if screens.len() < 2 {
        return Vec::new();
    }
    screens.iter().filter(|s| Some(s.handle) != on).map(|s| s.number).collect()
}

/// A length's share of the free room, carried over to another room.
fn carried(pos: i32, start: i32, free_from: i32, free_to: i32) -> i32 {
    if free_from <= 0 {
        return free_to.max(0) / 2;
    }
    let share = (f64::from(pos - start) / f64::from(free_from)).clamp(0.0, 1.0);
    (share * f64::from(free_to.max(0))).round() as i32
}

/// Where a window that stands at `win` in the work area `from` goes in the work area `to` (see the module's note).
pub fn moved_rect(win: Rect, from: Rect, to: Rect) -> Rect {
    let (w, h) = (win.width().min(to.width()), win.height().min(to.height()));
    let x = to.left + carried(win.left, from.left, from.width() - win.width(), to.width() - w);
    let y = to.top + carried(win.top, from.top, from.height() - win.height(), to.height() - h);
    Rect::new(x, y, x + w, y + h)
}

/// A window brought inside a work area: no larger than it and with no part outside it.
pub fn fitted_rect(win: Rect, work: Rect) -> Rect {
    let (w, h) = (win.width().min(work.width()), win.height().min(work.height()));
    let x = win.left.clamp(work.left, (work.right - w).max(work.left));
    let y = win.top.clamp(work.top, (work.bottom - h).max(work.top));
    Rect::new(x, y, x + w, y + h)
}

/// The program a launch starts, when that is a program on disk: the program itself, or the target of a shortcut. `None` for a
/// packaged app and for a shortcut to something that is no program.
pub fn program_of(launch: &Launch) -> Option<PathBuf> {
    let is_exe = |p: &Path| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"));
    match launch {
        Launch::Exe { path, .. } => is_exe(path).then(|| path.clone()),
        Launch::Link(link) => crate::shortcut::resolve(link).ok().map(|s| s.target).filter(|t| is_exe(t)),
        Launch::Packaged(_) => None,
    }
}

/// The first of `tasks` (a window, its process, that process's program) that is a window of `program` and is not among `known`.
pub fn new_window_of(tasks: &[(isize, u32, Option<PathBuf>)], known: &HashSet<isize>, program: &Path) -> Option<WinRef> {
    let same = |p: &Path| p.as_os_str().to_string_lossy().eq_ignore_ascii_case(&program.as_os_str().to_string_lossy());
    tasks.iter().find(|(hwnd, _, exe)| !known.contains(hwnd) && exe.as_deref().is_some_and(same)).map(|(hwnd, pid, _)| WinRef { hwnd: *hwnd, pid: *pid })
}

fn tasks_now() -> Vec<(isize, u32, Option<PathBuf>)> {
    crate::tasklist::tasks().into_iter().map(|t| (t.hwnd, t.pid, t.exe)).collect()
}

/// The windows there are now: taken BEFORE an app is started, to tell its new window from the ones that were there.
pub fn known_windows() -> HashSet<isize> {
    tasks_now().into_iter().map(|t| t.0).collect()
}

/// Waits, at most `limit`, for a window of `program` that is not among `known`, looking five times a second. For a worker thread.
pub fn wait_for_new_window(program: &Path, known: &HashSet<isize>, limit: Duration) -> Option<WinRef> {
    let start = Instant::now();
    loop {
        if let Some(w) = new_window_of(&tasks_now(), known, program) {
            return Some(w);
        }
        if start.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// The window as it stands when it is neither minimized nor maximized, and whether it was maximized. It is restored for that (a
/// minimized window first to what it was before, then, if that was maximized, to its normal place).
fn restored(hwnd: isize) -> Option<(Rect, bool)> {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, IsIconic, IsZoomed, SW_RESTORE, ShowWindow};
    let h = HWND(hwnd as *mut _);
    // SAFETY: plain calls on a window handle; a window that is gone makes them fail, which is read.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let was_max = IsZoomed(h).as_bool();
        if was_max {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        let mut r = RECT::default();
        GetWindowRect(h, &mut r).ok()?;
        Some((Rect::new(r.left, r.top, r.right, r.bottom), was_max))
    }
}

fn place(hwnd: isize, r: Rect) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos};
    // SAFETY: a plain call; it fails for a window that is gone or that Windows does not let this process move (an elevated one).
    unsafe { SetWindowPos(HWND(hwnd as *mut _), None, r.left, r.top, r.width().max(1), r.height().max(1), SWP_NOZORDER | SWP_NOACTIVATE).is_ok() }
}

/// Moves the window to the monitor of that number. False when the window is not the one that was seen any more, there is no such
/// monitor, or Windows refused (a window of a program that runs as administrator). Not for the UI thread: the window's program is
/// sent messages and may be slow to answer.
pub fn move_to_screen(w: WinRef, number: u32) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SW_MAXIMIZE, ShowWindow};
    if !w.is_still() {
        return false;
    }
    let _aware = super::system::aware_thread();
    let all = screens();
    let Some(to) = all.iter().find(|s| s.number == number) else { return false };
    let from = monitors::monitor_of_window(w.hwnd).and_then(|m| all.iter().find(|s| s.handle == m)).map_or(to.work, |s| s.work);
    let Some((win, was_max)) = restored(w.hwnd) else { return false };
    let ok = place(w.hwnd, moved_rect(win, from, to.work));
    if was_max {
        // SAFETY: as above.
        unsafe {
            let _ = ShowWindow(HWND(w.hwnd as *mut _), SW_MAXIMIZE);
        }
    }
    ok
}

/// Brings the window inside the work area of the monitor it is on. A maximized window fits already and is left as it is. False as
/// for [`move_to_screen`].
pub fn fit_to_screen(w: WinRef) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed, SW_RESTORE, ShowWindow};
    if !w.is_still() {
        return false;
    }
    let _aware = super::system::aware_thread();
    let h = HWND(w.hwnd as *mut _);
    // SAFETY: plain calls on a window handle.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if IsZoomed(h).as_bool() {
            return true;
        }
    }
    let all = screens();
    let Some(work) = monitors::monitor_of_window(w.hwnd).and_then(|m| all.iter().find(|s| s.handle == m)).map(|s| s.work) else { return false };
    let Some((win, _)) = restored(w.hwnd) else { return false };
    let to = fitted_rect(win, work);
    to == win || place(w.hwnd, to)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window of an app that was just started: of its program, and not one that was there before.
    #[test]
    fn the_new_window_of_a_started_program_is_told_from_the_old_ones() {
        let exe = |s: &str| Some(PathBuf::from(s));
        let tasks = vec![(10, 1, exe(r"C:\Apps\Notes.exe")), (11, 2, exe(r"C:\Apps\Other.exe")), (12, 3, exe(r"c:\apps\notes.EXE")), (13, 4, None)];
        let known: HashSet<isize> = [10, 11].into_iter().collect();
        assert_eq!(new_window_of(&tasks, &known, Path::new(r"C:\Apps\Notes.exe")), Some(WinRef { hwnd: 12, pid: 3 }), "the one that was not there, whatever the case");
        assert_eq!(new_window_of(&tasks, &known, Path::new(r"C:\Apps\Other.exe")), None, "its window was there before");
        assert_eq!(new_window_of(&tasks, &HashSet::new(), Path::new(r"C:\Apps\None.exe")), None);
        // what is started: a program, or a shortcut's program; never a packaged app
        assert_eq!(program_of(&Launch::Exe { path: PathBuf::from(r"C:\Apps\Notes.exe"), args: String::new() }), Some(PathBuf::from(r"C:\Apps\Notes.exe")));
        assert_eq!(program_of(&Launch::Exe { path: PathBuf::from(r"C:\Docs\a.pdf"), args: String::new() }), None);
        assert_eq!(program_of(&Launch::Packaged("x!App".into())), None);
    }

    #[test]
    fn a_displays_number_is_the_one_in_its_name() {
        assert_eq!(display_number(r"\\.\DISPLAY1"), Some(1));
        assert_eq!(display_number(r"\\.\DISPLAY12"), Some(12));
        assert_eq!(display_number("DISPLAY"), None);
        assert_eq!(display_number(""), None);
        assert_eq!(display_number(r"\\.\DISPLAY0"), None, "Windows counts from 1");
    }

    #[test]
    fn a_window_can_be_sent_to_every_monitor_but_its_own() {
        let s = |number, handle| Screen { number, handle, work: Rect::new(0, 0, 100, 100) };
        assert!(targets(&[s(1, 10)], Some(10)).is_empty(), "one monitor: nowhere to send it");
        assert_eq!(targets(&[s(1, 10), s(2, 20), s(3, 30)], Some(20)), vec![1, 3]);
        assert_eq!(targets(&[s(1, 10), s(2, 20)], None), vec![1, 2], "a window on no known monitor can go to either");
    }

    #[test]
    fn a_moved_window_keeps_its_size_and_its_place_in_the_room() {
        let (a, b) = (Rect::new(0, 0, 2560, 1400), Rect::new(2560, -600, 4000, 1900));
        // in the middle of one: in the middle of the other
        let mid = moved_rect(Rect::new(780, 300, 1780, 1100), a, b);
        assert_eq!((mid.width(), mid.height()), (1000, 800));
        assert_eq!((mid.left - b.left, b.right - mid.right), (220, 220));
        assert_eq!((mid.top - b.top, b.bottom - mid.bottom), (850, 850));
        // in the lower right corner: in that corner
        let corner = moved_rect(Rect::new(1560, 600, 2560, 1400), a, b);
        assert_eq!((corner.right, corner.bottom), (b.right, b.bottom));
        // wider than the other monitor: as wide as it, and inside it
        let wide = moved_rect(Rect::new(100, 100, 2400, 900), a, b);
        assert_eq!((wide.left, wide.right), (b.left, b.right));
        // a window that hung over the first monitor's edge is inside the second
        let over = moved_rect(Rect::new(-300, -50, 700, 750), a, b);
        assert!(over.left >= b.left && over.top >= b.top && over.right <= b.right && over.bottom <= b.bottom);
    }

    #[test]
    fn fitting_brings_a_window_inside_and_leaves_one_that_is_inside_alone() {
        let work = Rect::new(0, 0, 1920, 1040);
        let inside = Rect::new(100, 100, 900, 700);
        assert_eq!(fitted_rect(inside, work), inside);
        assert_eq!(fitted_rect(Rect::new(1500, 800, 2300, 1400), work), Rect::new(1120, 440, 1920, 1040), "over the lower right edge");
        assert_eq!(fitted_rect(Rect::new(-5000, -3000, -4200, -2400), work), Rect::new(0, 0, 800, 600), "beyond every edge");
        assert_eq!(fitted_rect(Rect::new(-200, -200, 3000, 2000), work), work, "larger than the work area");
        // a work area that does not start at 0 (a second monitor, a taskbar on the left)
        let second = Rect::new(2560, -618, 4000, 1894);
        let f = fitted_rect(Rect::new(0, 0, 1000, 800), second);
        assert_eq!((f.left, f.top, f.width(), f.height()), (2560, 0, 1000, 800));
    }

    #[test]
    fn displays_are_numbered_by_name_and_listed_in_that_order() {
        use super::super::system::MonitorInfo;
        let d = |name: &str, handle| Display { name: name.into(), info: MonitorInfo { handle, rect: Rect::new(0, 0, 10, 10), work: Rect::new(0, 0, 10, 9), dpi: 96 }, primary: false };
        let v = numbered(&[d(r"\\.\DISPLAY2", 20), d(r"\\.\DISPLAY1", 10)]);
        assert_eq!(v.iter().map(|s| (s.number, s.handle)).collect::<Vec<_>>(), vec![(1, 10), (2, 20)]);
    }
}
