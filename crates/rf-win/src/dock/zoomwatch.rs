//! Is a maximised window on the dock's monitor? The read (a walk over the top-level windows, the rules are `thin::counts_as_maximized`)
//! and the watch that says when to read again.
//!
//! The task list's events (`tasklist::watch`: a window comes, goes, is shown, hidden, cloaked, minimised, restored, takes the front) are
//! what the dock already listens to, and the thin dock listens to them too. They do not say that a window was MAXIMISED or restored (it
//! keeps its place in the list and in the order): that is a change of its placement. [`ZoomWatch`] adds the placement events and nothing
//! else, and exists only while the thin dock's setting is on:
//!
//! * `EVENT_SYSTEM_MOVESIZEEND`: a window dragged to the top edge (snapped to maximised) or off it (restored).
//! * `EVENT_OBJECT_LOCATIONCHANGE`, for whole top-level windows only: a maximise or a restore by the title bar's button, a double click,
//!   Win+Up, the program itself. The event also comes for every pixel of a window being dragged, and for the mouse cursor and the text
//!   caret; the hook function answers all of that without a system call (`id_object` is not the window) or with one style read, and
//!   passes on only a change of the MAXIMISED bit against the windows it remembers to be maximised (a handful). Counts are kept
//!   ([`Stats`]) and printed by `examples/zoomwatch_probe.rs`, which says what this costs on a live desktop.
//! * `EVENT_OBJECT_DESTROY`: forgets a maximised window that went away (a handle can be handed out again).
//!
//! Out of context, on a thread of its own with a message loop, as `tasklist::watch` does it. A second thread coalesces over 30 ms.
//! Starting does not wait for the thread (the dock's UI thread calls it); stopping before the thread is up is remembered and honoured.
//!
//! WHAT THE HOOK REMEMBERS ([`ZoomSet`]): the windows that have the maximised bit set and the monitor each one is on. The set is SEEDED
//! with the windows that are maximised when the watch starts (the dock starts, or the setting is switched on, with a browser already
//! maximised: its restore must be heard, too), and a maximised window that moves to another monitor (Win+Shift+Arrow keeps the bit
//! and changes the monitor) is passed on like a maximise or a restore is.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};

use super::thin::{WinFacts, counts_as_maximized};

const WS_MAXIMIZE: u32 = 0x0100_0000;
const WS_CHILD: u32 = 0x4000_0000;

/// How many events the hook function saw and what became of them.
#[derive(Debug, Default)]
pub struct Stats {
    /// Every call of the hook function.
    pub seen: AtomicU64,
    /// Of those, the ones for a whole window (not a cursor, a caret, a scroll bar, a list item).
    pub windows: AtomicU64,
    /// Of those, the ones that needed a style read (a top-level window's location changed).
    pub style_reads: AtomicU64,
    /// Passed on: the maximised bit of a window changed, a maximised window moved to another monitor or went away, a move or a resize ended.
    pub passed: AtomicU64,
    /// Told to the caller after the coalescing.
    pub told: AtomicU64,
}

// ---------------------------------------------------------------- what the hook remembers

/// The windows last seen with the maximised bit set, and the monitor (the `HMONITOR` as a number) each was on. Pure: the hook function
/// reads the system and asks this what the news is worth.
#[derive(Debug, Default)]
pub struct ZoomSet {
    zoomed: HashMap<isize, isize>,
}

impl ZoomSet {
    /// The windows that are maximised now, as (handle, monitor): the first walk.
    pub fn seed(&mut self, windows: impl IntoIterator<Item = (isize, isize)>) {
        self.zoomed.extend(windows);
    }

    /// A top-level window's location changed. True: the dock must read again, because the window was maximised, was restored, or is
    /// maximised on another monitor now. The pixels of a move or a resize that change nothing of that are not news.
    pub fn moved(&mut self, window: isize, maximized: bool, monitor: isize) -> bool {
        match (self.zoomed.get(&window).copied(), maximized) {
            (None, true) => {
                self.zoomed.insert(window, monitor);
                true
            }
            (Some(_), false) => {
                self.zoomed.remove(&window);
                true
            }
            (Some(was), true) if was != monitor => {
                self.zoomed.insert(window, monitor);
                true
            }
            _ => false,
        }
    }

    /// A window went away (its handle can be handed out again). True: it was a maximised one.
    pub fn destroyed(&mut self, window: isize) -> bool {
        self.zoomed.remove(&window).is_some()
    }

    pub fn len(&self) -> usize {
        self.zoomed.len()
    }

    pub fn is_empty(&self) -> bool {
        self.zoomed.is_empty()
    }
}

struct HookState {
    tx: Sender<()>,
    zoomed: ZoomSet,
    stats: Arc<Stats>,
}

thread_local! {
    static STATE: RefCell<Option<HookState>> = const { RefCell::new(None) };
}

unsafe extern "system" fn on_event(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    id_object: i32,
    id_child: i32,
    _thread: u32,
    _time: u32,
) {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_MOVESIZEEND, GWL_STYLE, GetWindowLongPtrW};
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        let Some(st) = s.as_mut() else { return };
        st.stats.seen.fetch_add(1, Ordering::Relaxed);
        // only whole windows (not the cursor, the caret, a scroll bar, an item of a list)
        if id_object != 0 || id_child != 0 || hwnd.is_invalid() {
            return;
        }
        st.stats.windows.fetch_add(1, Ordering::Relaxed);
        let h = hwnd.0 as isize;
        let pass = match event {
            EVENT_SYSTEM_MOVESIZEEND => true,
            EVENT_OBJECT_DESTROY => st.zoomed.destroyed(h),
            EVENT_OBJECT_LOCATIONCHANGE => {
                st.stats.style_reads.fetch_add(1, Ordering::Relaxed);
                let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
                if style & WS_CHILD != 0 {
                    false
                } else {
                    let maximized = style & WS_MAXIMIZE != 0;
                    // (the monitor is asked only for a maximised window: a handful, and only when it moves)
                    let monitor = if maximized { unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) }.0 as isize } else { 0 };
                    st.zoomed.moved(h, maximized, monitor)
                }
            }
            _ => false,
        };
        if pass {
            st.stats.passed.fetch_add(1, Ordering::Relaxed);
            let _ = st.tx.send(());
        }
    });
}

/// The top-level windows that have the maximised bit set now, with their monitors (what the hook is seeded with).
fn maximized_windows() -> Vec<(isize, isize)> {
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GWL_STYLE, GetWindowLongPtrW};
    use windows::core::BOOL;
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<(isize, isize)>) };
        let style = unsafe { GetWindowLongPtrW(h, GWL_STYLE) } as u32;
        if style & WS_MAXIMIZE != 0 {
            out.push((h.0 as isize, unsafe { MonitorFromWindow(h, MONITOR_DEFAULTTONULL) }.0 as isize));
        }
        BOOL(1)
    }
    let mut out = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut out as *mut Vec<(isize, isize)> as isize));
    }
    out
}

/// Watches for the placement events above; `on_change` runs on a background thread, at most once in 30 ms. Drop to stop (the hooks come
/// off and the threads end).
pub struct ZoomWatch {
    shared: Arc<Shared>,
    stats: Arc<Stats>,
}

/// What the hook thread and the owner agree on: the thread's id once its queue exists, and a stop that may come before that.
#[derive(Default)]
struct Shared {
    thread: AtomicU32,
    stop: AtomicBool,
}

impl Shared {
    fn quit(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{PostThreadMessageW, WM_QUIT};
        let id = self.thread.load(Ordering::SeqCst);
        if id != 0 {
            unsafe {
                let _ = PostThreadMessageW(id, WM_QUIT, WPARAM(0), LPARAM(0));
            }
        }
    }
}

impl ZoomWatch {
    /// Starts the threads and returns at once (it does not wait for the hooks to be set).
    pub fn start(on_change: impl Fn() + Send + 'static) -> anyhow::Result<ZoomWatch> {
        use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent};
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, EVENT_OBJECT_DESTROY, EVENT_OBJECT_LOCATIONCHANGE, EVENT_SYSTEM_MOVESIZEEND, GetMessageW, MSG, TranslateMessage, WINEVENT_OUTOFCONTEXT,
        };
        let stats = Arc::new(Stats::default());
        let shared = Arc::new(Shared::default());
        let (tx, rx): (Sender<()>, Receiver<()>) = std::sync::mpsc::channel();
        let hook_stats = stats.clone();
        let hook_shared = shared.clone();
        std::thread::Builder::new().name("rf-zoomwatch".into()).spawn(move || {
            STATE.with(|s| *s.borrow_mut() = Some(HookState { tx, zoomed: ZoomSet::default(), stats: hook_stats }));
            let ranges = [
                (EVENT_SYSTEM_MOVESIZEEND, EVENT_SYSTEM_MOVESIZEEND),
                (EVENT_OBJECT_DESTROY, EVENT_OBJECT_DESTROY),
                (EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE),
            ];
            // (our own windows move all the time, with the magnification; none of them is a maximised window)
            let hooks: Vec<_> = ranges
                .iter()
                .map(|&(a, b)| unsafe { SetWinEventHook(a, b, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT | windows::Win32::UI::WindowsAndMessaging::WINEVENT_SKIPOWNPROCESS) })
                .collect();
            if hooks.iter().any(|h| h.is_invalid()) {
                log::warn!("dock: a window placement hook could not be set; a maximise may go unnoticed until the next change of the task list");
            }
            // The windows that are maximised already: after the hooks are set (an event that comes meanwhile waits in the queue and is
            // handled after the seed), so that their restore is heard.
            let already = maximized_windows();
            STATE.with(|s| {
                if let Some(st) = s.borrow_mut().as_mut() {
                    st.zoomed.seed(already);
                }
            });
            // the queue exists before the owner can post the stop
            let mut probe = MSG::default();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(&mut probe, None, 0, 0, windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE);
            }
            hook_shared.thread.store(unsafe { windows::Win32::System::Threading::GetCurrentThreadId() }, Ordering::SeqCst);
            // (a stop that came before the id was known)
            if hook_shared.stop.load(Ordering::SeqCst) {
                hook_shared.quit();
            }
            let mut msg = MSG::default();
            unsafe {
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                for h in hooks {
                    let _ = UnhookWinEvent(h);
                }
            }
            // (the sender goes with the state: the coalescing thread ends)
            STATE.with(|s| *s.borrow_mut() = None);
        })?;
        let told = stats.clone();
        std::thread::Builder::new().name("rf-zoomwatch-coalesce".into()).spawn(move || {
            while rx.recv().is_ok() {
                // a maximise comes with a burst (the window's own steps, the work area's)
                let until = std::time::Instant::now() + Duration::from_millis(30);
                loop {
                    let left = until.saturating_duration_since(std::time::Instant::now());
                    match rx.recv_timeout(left) {
                        Ok(()) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                        Err(_) => return,
                    }
                }
                told.told.fetch_add(1, Ordering::Relaxed);
                on_change();
            }
        })?;
        Ok(ZoomWatch { shared, stats })
    }

    pub fn stats(&self) -> Arc<Stats> {
        self.stats.clone()
    }
}

impl Drop for ZoomWatch {
    fn drop(&mut self) {
        // (if the hook thread is not up yet it sees the flag as soon as it is)
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.quit();
    }
}

// ---------------------------------------------------------------- the read

/// Whether a maximised window the user can see is on `monitor` (an `HMONITOR` as the number `MonitorInfo::handle` holds). A walk over
/// the top-level windows that stops at the first one: a style read for each, and the rest of the questions only for the maximised ones.
/// Fine on a worker thread (the dock calls it from the one that follows the events), a few hundred microseconds on a desktop of 300
/// windows.
pub fn maximized_on(monitor: isize) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;
    use windows::core::BOOL;
    struct Walk {
        monitor: isize,
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn each(h: HWND, l: LPARAM) -> BOOL {
        let walk = unsafe { &mut *(l.0 as *mut Walk) };
        if let Some(facts) = facts_of(h, walk.monitor, walk.pid)
            && counts_as_maximized(&facts)
        {
            walk.found = true;
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut walk = Walk { monitor, pid: std::process::id(), found: false };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut walk as *mut Walk as isize));
    }
    walk.found
}

/// Where the pointer is (device px) and whether a mouse button is held. Two system calls, no allocation: the thin dock asks every 60 ms
/// while it is thin, because a window with a region cannot be told that the pointer is NEAR it (the toolkit only hears about the part
/// the region leaves). `None` when the position cannot be read (the desktop is locked).
pub fn pointer() -> Option<(i32, i32, bool)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_MBUTTON, VK_RBUTTON};
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut p = POINT::default();
    unsafe {
        GetCursorPos(&mut p).ok()?;
        let down = [VK_LBUTTON, VK_RBUTTON, VK_MBUTTON].iter().any(|k| GetAsyncKeyState(k.0 as i32) < 0);
        Some((p.x, p.y, down))
    }
}

/// What the rules need to know about a window; `None` when it is not even maximised (the cheap rejection that most windows get).
fn facts_of(h: HWND, monitor: isize, own_pid: u32) -> Option<WinFacts> {
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible};
    unsafe {
        let style = GetWindowLongPtrW(h, GWL_STYLE) as u32;
        if style & WS_MAXIMIZE == 0 {
            return None;
        }
        let mut cloaked: u32 = 0;
        let is_cloaked = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4).is_ok() && cloaked != 0;
        let mut pid = 0u32;
        GetWindowThreadProcessId(h, Some(&mut pid));
        let mut class = [0u16; 128];
        let n = GetClassNameW(h, &mut class);
        let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
        Some(WinFacts {
            visible: IsWindowVisible(h).as_bool(),
            minimized: IsIconic(h).as_bool(),
            cloaked: is_cloaked,
            zoomed: true,
            has_owner: GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()),
            ex_style: GetWindowLongPtrW(h, GWL_EXSTYLE) as u32,
            own: pid == own_pid,
            shell: super::fullscreen::is_shell_class(&class),
            on_dock_monitor: MonitorFromWindow(h, MONITOR_DEFAULTTONULL).0 as isize == monitor,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read-only: the walk finishes and gives a yes or a no (whatever is on this machine's desktop), and a monitor that does not exist
    /// has no maximised window.
    #[test]
    fn the_read_finishes_and_a_monitor_that_is_not_there_has_none() {
        let _ = maximized_on(super::super::primary_monitor().map_or(0, |m| m.handle));
        assert!(!maximized_on(0x7fff_fff0));
    }

    /// The watch starts, hears nothing it should not, and stops quickly. No window is made or moved.
    #[test]
    fn the_watch_starts_without_waiting_and_stops_quickly() {
        let t = std::time::Instant::now();
        let w = ZoomWatch::start(|| {}).unwrap();
        // (it returns before the hook thread is up: a UI thread must not wait for it)
        assert!(t.elapsed() < Duration::from_millis(500), "{:?}", t.elapsed());
        let stats = w.stats();
        drop(w);
        assert!(t.elapsed() < Duration::from_secs(2));
        // (it may have seen events of the machine meanwhile; the counts only ever grow)
        assert!(stats.seen.load(Ordering::Relaxed) >= stats.windows.load(Ordering::Relaxed));
    }

    /// A stop that comes at once, before the thread has its queue, still ends the thread (it is not left running with its hooks).
    #[test]
    fn a_stop_before_the_thread_is_up_is_not_lost() {
        for _ in 0..20 {
            let w = ZoomWatch::start(|| {}).unwrap();
            let shared = w.shared.clone();
            drop(w);
            let t = std::time::Instant::now();
            while shared.thread.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            // the thread has told its id (so it saw the stop) - and its message loop ends, which the Arc count shows: the owner's and
            // the test's are the only ones left once the thread has returned
            let t = std::time::Instant::now();
            while Arc::strong_count(&shared) > 1 && t.elapsed() < Duration::from_secs(5) {
                std::thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(Arc::strong_count(&shared), 1, "the hook thread is still running");
        }
    }

    // ---- what the hook remembers

    const MON_A: isize = 0x11;
    const MON_B: isize = 0x22;

    #[test]
    fn a_maximise_and_a_restore_are_news_and_the_pixels_between_are_not() {
        let mut z = ZoomSet::default();
        assert!(!z.moved(1, false, 0), "a window that moves about is not news");
        assert!(z.moved(1, true, MON_A), "maximised");
        assert!(!z.moved(1, true, MON_A), "the steps of the animation, the work area's resize");
        assert!(z.moved(1, false, 0), "restored");
        assert!(!z.moved(1, false, 0));
        assert!(z.is_empty());
    }

    #[test]
    fn a_window_that_was_maximised_when_the_watch_started_is_heard_when_it_is_restored() {
        // the dock starts (or the setting is switched on) with a browser already maximised: the first walk seeds the set
        let mut z = ZoomSet::default();
        z.seed([(7, MON_A), (8, MON_B)]);
        assert_eq!(z.len(), 2);
        assert!(!z.moved(7, true, MON_A), "still maximised: nothing new");
        assert!(z.moved(7, false, 0), "the restore button: the dock must read again");
        assert!(z.destroyed(8), "and a seeded window that goes away is heard, too");
        assert!(z.is_empty());
        // without the seed the restore would have been (false, false): nothing
        let mut unseeded = ZoomSet::default();
        assert!(!unseeded.moved(7, false, 0));
    }

    #[test]
    fn a_maximised_window_moved_to_another_monitor_is_news() {
        // Win+Shift+Arrow keeps the maximised bit and changes the monitor
        let mut z = ZoomSet::default();
        z.seed([(7, MON_B)]);
        assert!(z.moved(7, true, MON_A), "it came to the dock's monitor");
        assert!(!z.moved(7, true, MON_A));
        assert!(z.moved(7, true, MON_B), "and went away from it");
        assert!(!z.moved(7, true, MON_B));
    }

    #[test]
    fn a_handle_that_is_handed_out_again_starts_fresh() {
        let mut z = ZoomSet::default();
        assert!(z.moved(5, true, MON_A));
        assert!(z.destroyed(5));
        assert!(!z.destroyed(5), "a window that was not maximised is not news when it goes");
        assert!(z.moved(5, true, MON_B), "the new window with the old handle");
    }
}
