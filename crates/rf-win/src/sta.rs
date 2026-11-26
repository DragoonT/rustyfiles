//! A long-lived single-threaded-apartment "shell thread" with a message pump and a hidden
//! owner window. Context menus, property sheets and other modeless shell UI must live on a
//! thread that keeps pumping messages — and must never be the UI thread, so a slow shell
//! extension can't freeze rustyfiles.

use std::collections::VecDeque;
use std::sync::OnceLock;

use parking_lot::Mutex;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::OleInitialize;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG, PostMessageW, RegisterClassW,
    TranslateMessage, WINDOW_EX_STYLE, WM_APP, WNDCLASSW, WS_EX_TOOLWINDOW, WS_POPUP,
};
use windows::core::w;

type Job = Box<dyn FnOnce(HWND) + Send>;

struct ShellThread {
    hwnd: isize,
    queue: Mutex<VecDeque<Job>>,
}

thread_local! {
    /// Set on the shell thread itself, so a [`call`] made from a job running there runs in place instead of waiting for itself.
    static ON_SHELL_THREAD: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

static THREAD: OnceLock<ShellThread> = OnceLock::new();
/// The thread of [`run_drag`]: a second one of the same kind, for drags only.
static DRAG_THREAD: OnceLock<ShellThread> = OnceLock::new();
const WM_RUN: u32 = WM_APP + 1;

thread_local! {
    /// Window-message hook installed while a context menu is open (IContextMenu2/3 forwarding).
    pub(crate) static MENU_HOOK: std::cell::RefCell<Option<Box<dyn Fn(u32, WPARAM, LPARAM) -> Option<LRESULT>>>> =
        const { std::cell::RefCell::new(None) };
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_RUN {
        drain(hwnd);
        return LRESULT(0);
    }
    let hooked = MENU_HOOK.with(|h| h.borrow().as_ref().and_then(|f| f(msg, wp, lp)));
    if let Some(r) = hooked {
        return r;
    }
    unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
}

fn drain(hwnd: HWND) {
    // (both threads share this window procedure: the window says whose queue it is)
    let Some(t) = [THREAD.get(), DRAG_THREAD.get()].into_iter().flatten().find(|t| t.hwnd == hwnd.0 as isize) else { return };
    loop {
        let job = t.queue.lock().pop_front();
        match job {
            Some(j) => j(hwnd),
            None => break,
        }
    }
}

fn start(name: &str, shell_thread: bool) -> ShellThread {
    let (tx, rx) = std::sync::mpsc::channel::<isize>();
    std::thread::Builder::new()
        .name(name.into())
        .spawn(move || unsafe {
            ON_SHELL_THREAD.with(|c| c.set(shell_thread));
            let _ = OleInitialize(None);
            let hinst = GetModuleHandleW(None).unwrap_or_default();
            let class = w!("rustyfiles.ShellOwner");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: hinst.into(),
                lpszClassName: class,
                ..Default::default()
            };
            RegisterClassW(&wc);
            let hwnd = CreateWindowExW(
                WS_EX_TOOLWINDOW,
                class,
                w!(""),
                WS_POPUP,
                0,
                0,
                0,
                0,
                None,
                None,
                Some(hinst.into()),
                None,
            )
            .unwrap_or_default();
            let _ = tx.send(hwnd.0 as isize);
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        })
        .expect("spawn shell thread");
    let hwnd = rx.recv().unwrap_or(0);
    ShellThread { hwnd, queue: Mutex::new(VecDeque::new()) }
}

/// Runs `f` on the shell STA thread. `f` receives the hidden owner window.
pub fn run(f: impl FnOnce(HWND) + Send + 'static) {
    post(THREAD.get_or_init(|| start("rf-shell-sta", true)), Box::new(f));
}

/// Runs `f` on the drag thread: a thread like the shell thread (OLE, a message pump, a hidden owner window that `f` receives) that
/// does nothing but drags out of our windows.
///
/// WHY: a drag used to be one more job in the shell thread's queue, behind whatever that thread was doing: building a context menu,
/// reading a file's properties for a column (a slow handler may take seconds), a preview. While it waited the press had already
/// selected the item and nothing else happened, and when its turn came the button was often up again, so no drag began at all
/// (a Discord report, 2026-11-24: "I cannot drag an item out of the app to another app. It only gets highlighted"). A drag is a
/// gesture the hand is in the middle of: it must begin at once, whatever else the shell is asked.
pub fn run_drag(f: impl FnOnce(HWND) + Send + 'static) {
    post(DRAG_THREAD.get_or_init(|| start("rf-drag-sta", false)), Box::new(f));
}

fn post(t: &ShellThread, job: Job) {
    t.queue.lock().push_back(job);
    unsafe {
        let _ = PostMessageW(Some(HWND(t.hwnd as *mut _)), WM_RUN, WPARAM(0), LPARAM(0));
    }
}

/// Runs `f` on the shell thread and waits for its answer, at most `limit`. `None` when the thread did not get to it or did not
/// finish in that time (the job still runs to its end there; its answer is dropped).
///
/// For shell work that must not run on worker threads. A worker that calls the shell becomes a single-threaded apartment that never
/// pumps messages (`shell::ensure_com`), and the shell does not survive several of those asking at once: in the dock's first live
/// runs, with eight workers extracting icons at the same moment, the logos of packaged apps never arrived (two to seven of them,
/// never the same ones). Windows' wait-chain view showed the asking thread waiting for an apartment on a thread the shell had made
/// itself, which was blocked, and workers waiting for each other's apartments. Sending only the packaged apps' requests here did
/// not help (the very first one still hung while the workers were busy with files' icons); sending every shell request of the dock
/// here did: 25 of 25 icons in each of four runs, and no worker apartment left in the process. The limit is there so that no shell
/// answer, whatever its cause, can hold the caller for ever.
pub fn call<R: Send + 'static>(limit: std::time::Duration, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    if ON_SHELL_THREAD.with(|c| c.get()) {
        return Some(f());
    }
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    run(move |_| {
        let _ = tx.send(f());
    });
    rx.recv_timeout(limit).ok()
}

#[allow(dead_code)]
fn _unused(_: WINDOW_EX_STYLE) {}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    #[test]
    fn a_call_gives_its_answer_and_a_call_from_inside_a_call_does_not_wait_for_itself() {
        assert_eq!(super::call(Duration::from_secs(10), || 6 * 7), Some(42));
        let nested = super::call(Duration::from_secs(10), || super::call(Duration::from_secs(10), || "inner"));
        assert_eq!(nested, Some(Some("inner")));
    }

    #[test]
    fn a_drag_does_not_wait_for_the_work_of_the_shell_thread() {
        // the shell thread is busy for a long while (a slow property handler, a menu being built)
        super::run(|_| std::thread::sleep(Duration::from_millis(1500)));
        let (tx, rx) = std::sync::mpsc::channel();
        super::run_drag(move |owner| {
            let (mut kind, mut qualifier) = Default::default();
            let apartment = unsafe { windows::Win32::System::Com::CoGetApartmentType(&mut kind, &mut qualifier) }.map(|()| kind);
            let name = std::thread::current().name().map(str::to_string);
            let _ = tx.send((owner.0 as isize, apartment, name));
        });
        let (owner, apartment, name) = rx.recv_timeout(Duration::from_millis(700)).expect("the drag's job ran while the shell thread was busy");
        assert_ne!(owner, 0, "it has an owner window of its own");
        // (a single-threaded apartment, as OLE's drag needs. The first one made in a process is its "main" one: which thread that is
        // depends on the order the tests of a full run start in)
        let apartment = apartment.ok();
        use windows::Win32::System::Com::{APTTYPE_MAINSTA, APTTYPE_STA};
        assert!(apartment == Some(APTTYPE_STA) || apartment == Some(APTTYPE_MAINSTA), "{apartment:?}");
        assert_eq!(name.as_deref(), Some("rf-drag-sta"));
        // a second job finds the same thread, and a job of the shell thread still runs on its own
        let (tx, rx) = std::sync::mpsc::channel();
        super::run_drag(move |_| {
            let _ = tx.send(std::thread::current().name().map(str::to_string));
        });
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap().as_deref(), Some("rf-drag-sta"));
        assert_eq!(super::call(Duration::from_secs(10), || std::thread::current().name().map(str::to_string)), Some(Some("rf-shell-sta".to_string())));
    }

    #[test]
    fn a_call_that_takes_too_long_gives_none_and_the_thread_lives_on() {
        let slow = super::call(Duration::from_millis(30), || std::thread::sleep(Duration::from_millis(300)));
        assert_eq!(slow, None);
        // the slow job ends; the next call is answered
        assert_eq!(super::call(Duration::from_secs(10), || 1), Some(1));
    }
}
