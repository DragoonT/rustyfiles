//! The tray spy (probe P1 of the shell plan): a window of class `Shell_TrayWnd` that sits in front of Explorer's, writes down every
//! `WM_COPYDATA` that programs send to the tray (hex dump and parsed form) and passes every message on to Explorer's real window
//! unchanged, returning Explorer's own answer. Its job is to settle, on the real machine, what the wire format of `rf_win::tray_wire`
//! only reads from documents: sizes, versions, trailing bytes, 32/64-bit senders, whether a repeated `NIM_ADD` fails.
//!
//! NOT RUN BY THE AUTHOR. Starting it creates a window of the tray class on the user's machine; that is the lead's decision with the user.
//!
//! What running it touches:
//!   * one hidden top-level window of class `Shell_TrayWnd` (no `WS_VISIBLE` unless `--show`, never activated), kept topmost by a
//!     250 ms timer so that programs that look the tray up find it first. Explorer's own tray window is not moved, hidden or altered.
//!     The process makes itself per-monitor DPI aware first, so the rectangle it copies from Explorer's tray window is in physical
//!     pixels on every monitor. `--show` makes the window visible as a faint tinted rectangle over the real taskbar; it is created
//!     click-through (layered + transparent), so the taskbar underneath keeps working;
//!   * every program that calls `Shell_NotifyIcon`, `Shell_NotifyIconGetRect` or `SHAppBarMessage` while it runs: its call goes through
//!     this process (the sender waits for the spy, which waits for Explorer, with a 1.5 s limit). The rustyfiles dock's own appbar
//!     registration and rustyfiles' tray icon go through it too;
//!   * the log file given with `--out` (default: `rf-tray-spy.log` in the temp folder): it holds icon tips and balloon texts of
//!     whatever programs sent, so it is the user's data. An existing file is appended to only if it is empty or one of the spy's own
//!     logs (first line `# rf-win tray_spy log`); anything else is refused. Lines are buffered and written once per message, after
//!     the message has been passed on, so the sender never waits for the disk;
//!   * messages: `WM_COPYDATA`, `WM_COMMAND`, `WM_ACTIVATEAPP` and the `WM_USER` / `WM_APP` ranges are passed on. Registered messages
//!     (0xC000 and up) are not, because most are broadcasts that Explorer's window gets by itself; each distinct one is written to the
//!     log once (`not relayed: registered message ...`), so the probe shows if anything meant for Explorer alone was held back;
//!   * with `--broadcast-start` / `--broadcast-exit`: a `TaskbarCreated` broadcast, which makes every program that handles it add its
//!     icons again (through the spy to Explorer, where an icon that is already there may be refused: that is probe P7). Off by default.
//!
//! Nothing else: no registry, no Winlogon, no shell window, no hook, no hotkey, no settings, Explorer is only sent the messages the
//! programs sent.
//!
//! How it stops (any of these): `--seconds N` elapsed (default 120, 0 = no limit), the file given with `--stop-file` appearing,
//! Ctrl+C / Ctrl+Break / closing the console, `WM_CLOSE` to its window, a session end. On stop it destroys its window (Explorer's tray
//! window is first in the z-order again at once), writes a summary, and optionally broadcasts `TaskbarCreated`. If it is killed instead,
//! the window goes with the process and the effect is the same minus the summary and the broadcast.
//!
//! Safety defaults: it does nothing without `--run`; without a tray window owned by `explorer.exe` to forward to it refuses to start
//! (`--allow-no-target` overrides: then every message goes to a window of unknown owner if there is one, else it is refused with reply
//! 0, which is what senders get when there is no tray at all). A forward that Explorer does not answer within 4 s (shell32's own limit
//! for the sender) is logged as TIMED OUT and answered with 0; Explorer may still process the message later.
//!
//!   cargo run -p rf-win --example tray_spy -- --run --seconds 300 --out C:\path\to\spy.log
//!
//! Options: --run, --seconds N, --out FILE, --stop-file FILE, --broadcast-start, --broadcast-exit, --show, --allow-no-target,
//! --accept-other-hosts, --full-hex (dump up to 64 KiB per block instead of 1 KiB).

use std::cell::RefCell;
use std::ffi::c_void;
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};

use rf_win::tray_wire::dump::{LOG_HEADER, describe_block, existing_log_acceptable, hex_dump};
use rf_win::tray_wire::forward::{Action, Method, Outcome, PickPolicy, SelfIdentity, WindowInfo, decide, method_for, real_tray_present, reply_for, should_log_local, should_retry};
use rf_win::tray_wire::TRAY_CLASS;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_HIGHLIGHT, GetSysColorBrush};
use windows::Win32::System::DataExchange::{COPYDATASTRUCT, GetClipboardFormatNameW};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, EnumWindows, GetClassNameW, GetMessageW, GetWindowRect,
    GWL_EXSTYLE, GetWindowLongPtrW, GetWindowThreadProcessId, HWND_BROADCAST, HWND_TOPMOST, IsWindow, KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, LWA_ALPHA, SMTO_ABORTIFHUNG, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SendMessageTimeoutW,
    SendNotifyMessageW, SetLayeredWindowAttributes, SetTimer, SetWindowLongPtrW, SetWindowPos, ShowWindow, TranslateMessage, UnregisterClassW, WM_CLOSE, WM_COPYDATA, WM_DESTROY,
    WM_DISPLAYCHANGE, WM_ENDSESSION, WM_QUERYENDSESSION, WM_TIMER, WNDCLASSEXW, WS_CLIPCHILDREN, WS_CLIPSIBLINGS, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};
use windows::core::{BOOL, PWSTR, w};

const TIMER_KEEP_ON_TOP: usize = 1;
const TIMER_STOP_CHECK: usize = 2;
const KEEP_ON_TOP_MS: u32 = 250;
const STOP_CHECK_MS: u32 = 500;
/// How long a forward may take before the spy gives up on Explorer for that message (the sender is waiting meanwhile). The same 4 s
/// as shell32's own wait for the taskbar: a shorter limit would fail calls that would have succeeded, and Explorer would still
/// process the message afterwards, so a retrying program would add a duplicate.
const FORWARD_TIMEOUT_MS: u32 = 4000;
/// Re-mirror Explorer's tray rectangle onto the spy's window every this many keep-on-top ticks (4 s).
const MIRROR_EVERY_TICKS: u32 = 16;
/// Opacity (0..255) of the `--show` rectangle.
const SHOW_ALPHA: u8 = 40;
/// Size of the log buffer: a block of 964 bytes dumped in hex is about 7 KiB, so a message normally never reaches the disk before it
/// has been passed on.
const LOG_BUFFER: usize = 256 * 1024;

struct Config {
    seconds: u64,
    out: PathBuf,
    stop_file: Option<PathBuf>,
    broadcast_start: bool,
    broadcast_exit: bool,
    show: bool,
    allow_no_target: bool,
    accept_other_hosts: bool,
    full_hex: bool,
}

#[derive(Default)]
struct Counts {
    messages: u64,
    by_kind: [u64; 4],
    other_kind: u64,
    unparsed: u64,
    forwarded: u64,
    refused: u64,
    timed_out: u64,
    target_gone: u64,
    passed_on_other: u64,
}

struct State {
    cfg: Config,
    start: Instant,
    log: std::io::BufWriter<std::fs::File>,
    me: SelfIdentity,
    policy: PickPolicy,
    taskbar_created: u32,
    counts: Counts,
    ticks: u32,
    deadline: Option<Instant>,
    /// Registered messages already written to the log as not relayed.
    local_seen: HashSet<u32>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

/// The spy's window, for the console handler (which runs on another thread).
static OUR_HWND: AtomicIsize = AtomicIsize::new(0);

fn hwnd_of(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

fn logln(line: &str) {
    STATE.with(|s| {
        if let Ok(mut guard) = s.try_borrow_mut()
            && let Some(st) = guard.as_mut()
        {
            let t = st.start.elapsed().as_secs_f64();
            let _ = writeln!(st.log, "[+{t:9.3}s] {line}");
        }
    });
}

/// Writes the buffered log lines out. Called after a message has been passed on, and by the timer.
fn flush_log() {
    STATE.with(|s| {
        if let Ok(mut guard) = s.try_borrow_mut()
            && let Some(st) = guard.as_mut()
        {
            let _ = st.log.flush();
        }
    });
}

// ---- finding the real tray ----

fn exe_name_of(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n);
        let _ = CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..n as usize]);
        full.rsplit(['\\', '/']).next().map(str::to_string)
    }
}

/// Every top-level window of class `Shell_TrayWnd`, topmost first. The class is read back with `GetClassNameW`, not assumed.
fn tray_windows() -> Vec<WindowInfo> {
    unsafe extern "system" fn cb(h: HWND, l: LPARAM) -> BOOL {
        let out = unsafe { &mut *(l.0 as *mut Vec<WindowInfo>) };
        let mut buf = [0u16; 128];
        let n = unsafe { GetClassNameW(h, &mut buf) };
        if n <= 0 {
            return BOOL(1);
        }
        let class = String::from_utf16_lossy(&buf[..n as usize]);
        if class != TRAY_CLASS {
            return BOOL(1);
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        out.push(WindowInfo { hwnd: h.0 as isize, class, pid, exe: exe_name_of(pid) });
        BOOL(1)
    }
    let mut out: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&mut out as *mut _ as isize));
    }
    out
}

/// A copy of what the window procedure needs, so that no borrow of the state is held while waiting for Explorer (a sent message to
/// this thread can arrive while it waits, and it needs the state).
fn identity() -> (SelfIdentity, PickPolicy) {
    STATE.with(|s| {
        let g = s.borrow();
        let st = g.as_ref().expect("state exists while the window does");
        (st.me.clone(), st.policy)
    })
}

fn current_action() -> (Action, Vec<WindowInfo>) {
    let (me, policy) = identity();
    let cands = tray_windows();
    (decide(&cands, &me, policy), cands)
}

// ---- forwarding ----

fn send_timeout(target: isize, msg: u32, wp: WPARAM, lp: LPARAM) -> Outcome {
    let h = hwnd_of(target);
    unsafe {
        if !IsWindow(Some(h)).as_bool() {
            return Outcome::TargetGone;
        }
        let mut result = 0usize;
        let r = SendMessageTimeoutW(h, msg, wp, lp, SMTO_ABORTIFHUNG, FORWARD_TIMEOUT_MS, Some(&mut result));
        if r.0 != 0 {
            Outcome::Delivered(result as isize)
        } else if IsWindow(Some(h)).as_bool() {
            Outcome::TimedOut
        } else {
            Outcome::TargetGone
        }
    }
}

/// Sends to the real tray; if its window vanished meanwhile (Explorer restarted) looks it up again once and retries.
fn forward_with_retry(msg: u32, wp: WPARAM, lp: LPARAM) -> (Action, Outcome) {
    let (action, _) = current_action();
    let Action::Forward { target } = action else {
        return (action, Outcome::TargetGone);
    };
    let mut outcome = send_timeout(target, msg, wp, lp);
    let mut used = action;
    if should_retry(outcome) {
        let (again, _) = current_action();
        if let Action::Forward { target: t2 } = again {
            outcome = send_timeout(t2, msg, wp, lp);
            used = again;
        }
    }
    (used, outcome)
}

fn count_outcome(action: Action, outcome: Outcome) {
    STATE.with(|s| {
        if let Ok(mut g) = s.try_borrow_mut()
            && let Some(st) = g.as_mut()
        {
            match (action, outcome) {
                (Action::Refuse { .. }, _) => st.counts.refused += 1,
                (_, Outcome::Delivered(_) | Outcome::Posted) => st.counts.forwarded += 1,
                (_, Outcome::TimedOut) => st.counts.timed_out += 1,
                (_, Outcome::TargetGone) => st.counts.target_gone += 1,
            }
        }
    });
}

fn on_copydata(sender: WPARAM, lp: LPARAM) -> LRESULT {
    // the COPYDATASTRUCT and its data are valid until this handler returns
    let cds = lp.0 as *const COPYDATASTRUCT;
    if cds.is_null() {
        return LRESULT(0);
    }
    let (dw_data, cb, data_ptr) = unsafe { ((*cds).dwData, (*cds).cbData as usize, (*cds).lpData as *const u8) };
    let bytes: &[u8] = if cb == 0 || data_ptr.is_null() { &[] } else { unsafe { std::slice::from_raw_parts(data_ptr, cb) } };

    // log and parse first, inside catch_unwind: a bug in the parser must never stop the message from being passed on
    let seq = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| log_incoming(sender, dw_data, bytes))).unwrap_or(0);

    let started = Instant::now();
    let (action, outcome) = forward_with_retry(WM_COPYDATA, sender, lp);
    let reply = match action {
        Action::Forward { .. } => reply_for(outcome),
        Action::Refuse { reply } => reply,
    };
    count_outcome(action, outcome);
    let how = match (action, outcome) {
        (Action::Refuse { .. }, _) => "REFUSED (no real tray window) reply=0".to_string(),
        (Action::Forward { target }, Outcome::Delivered(r)) => format!("forwarded to {target:#x}, Explorer replied {r}"),
        (Action::Forward { target }, Outcome::TimedOut) => format!("forward to {target:#x} TIMED OUT, reply=0"),
        (Action::Forward { target }, o) => format!("forward to {target:#x}: {o:?}, reply=0"),
    };
    logln(&format!("#{seq} -> {how} ({} ms)", started.elapsed().as_millis()));
    flush_log();
    LRESULT(reply)
}

fn log_incoming(sender: WPARAM, dw_data: usize, bytes: &[u8]) -> u64 {
    let (seq, full_hex) = STATE.with(|s| {
        let mut g = s.borrow_mut();
        let st = g.as_mut().expect("state exists while the window does");
        st.counts.messages += 1;
        match dw_data {
            0..=3 => st.counts.by_kind[dw_data] += 1,
            _ => st.counts.other_kind += 1,
        }
        (st.counts.messages, st.cfg.full_hex)
    });
    let described = describe_block(dw_data, bytes);
    if described.starts_with("UNPARSED") {
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.counts.unparsed += 1;
            }
        });
    }
    let sender_pid = {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd_of(sender.0 as isize), Some(&mut pid)) };
        pid
    };
    let sender_exe = exe_name_of(sender_pid).unwrap_or_else(|| "?".into());
    logln(&format!("#{seq} COPYDATA from={:#x} ({sender_exe} pid {sender_pid}) dwData={dw_data} cbData={}", sender.0, bytes.len()));
    logln(&format!("#{seq}   {described}"));
    let cap = if full_hex { 64 * 1024 } else { 1024 };
    for line in hex_dump(bytes, cap).lines() {
        logln(&format!("#{seq}   {line}"));
    }
    seq
}

/// Anything other than `WM_COPYDATA` that is meant for the tray: passed on, logged briefly.
fn pass_on_other(msg: u32, wp: WPARAM, lp: LPARAM, method: Method) -> LRESULT {
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            st.counts.passed_on_other += 1;
        }
    });
    let (action, _) = current_action();
    let Action::Forward { target } = action else {
        logln(&format!("msg {msg:#06x} wParam={:#x} lParam={:#x}: REFUSED (no real tray window)", wp.0, lp.0));
        flush_log();
        return LRESULT(0);
    };
    match method {
        Method::Post => {
            let ok = unsafe { PostMessageW(Some(hwnd_of(target)), msg, wp, lp) }.is_ok();
            logln(&format!("msg {msg:#06x} wParam={:#x} lParam={:#x}: posted to {target:#x} ok={ok}", wp.0, lp.0));
            flush_log();
            LRESULT(reply_for(Outcome::Posted))
        }
        _ => {
            let outcome = send_timeout(target, msg, wp, lp);
            logln(&format!("msg {msg:#06x} wParam={:#x} lParam={:#x}: sent to {target:#x} -> {outcome:?}", wp.0, lp.0));
            flush_log();
            LRESULT(reply_for(outcome))
        }
    }
}

/// A registered message that is not passed on: written to the log the first time its id arrives, with its registered name.
fn note_local(msg: u32, taskbar_created: u32, wp: WPARAM, lp: LPARAM) {
    let first = STATE.with(|s| {
        s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(|st| should_log_local(msg, taskbar_created, &mut st.local_seen))).unwrap_or(false)
    });
    if !first {
        return;
    }
    let mut buf = [0u16; 256];
    let n = unsafe { GetClipboardFormatNameW(msg, &mut buf) };
    let name = if n > 0 { String::from_utf16_lossy(&buf[..n as usize]) } else { "?".to_string() };
    logln(&format!(
        "not relayed: registered message {msg:#06x} \"{name}\" wParam={:#x} lParam={:#x} (first of this id; later ones are not logged)",
        wp.0, lp.0
    ));
    flush_log();
}

// ---- housekeeping ----

/// Puts the spy first among the `Shell_TrayWnd` windows when something else is. Explorer's window is never touched.
fn keep_on_top(hwnd: HWND) {
    let first = tray_windows().first().map(|w| w.hwnd);
    if first != Some(hwnd.0 as isize) {
        unsafe {
            let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
        }
    }
}

/// Gives the spy's (hidden) window the rectangle of the real tray, for programs that look the tray up and then read its rectangle.
fn mirror_rect(hwnd: HWND) {
    if let (Action::Forward { target }, _) = current_action() {
        let mut r = RECT::default();
        unsafe {
            if GetWindowRect(hwnd_of(target), &mut r).is_ok() {
                let _ = SetWindowPos(hwnd, None, r.left, r.top, (r.right - r.left).max(1), (r.bottom - r.top).max(1), SWP_NOZORDER | SWP_NOACTIVATE);
            }
        }
    }
}

fn on_timer(hwnd: HWND, id: usize) {
    match id {
        TIMER_KEEP_ON_TOP => {
            flush_log();
            keep_on_top(hwnd);
            let tick = STATE.with(|s| {
                let mut g = s.borrow_mut();
                let st = g.as_mut().expect("state");
                st.ticks += 1;
                st.ticks
            });
            if tick.is_multiple_of(MIRROR_EVERY_TICKS) {
                mirror_rect(hwnd);
            }
        }
        TIMER_STOP_CHECK => {
            let (due, file) = STATE.with(|s| {
                let g = s.borrow();
                let st = g.as_ref().expect("state");
                (st.deadline.is_some_and(|d| Instant::now() >= d), st.cfg.stop_file.clone())
            });
            if due {
                logln("stopping: --seconds elapsed");
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            } else if file.is_some_and(|f| f.exists()) {
                logln("stopping: stop file found");
                unsafe {
                    let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                }
            }
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let tc = STATE.with(|s| s.try_borrow().ok().and_then(|g| g.as_ref().map(|st| st.taskbar_created)).unwrap_or(0));
    match msg {
        WM_COPYDATA => on_copydata(wp, lp),
        WM_TIMER => {
            on_timer(hwnd, wp.0);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            mirror_rect(hwnd);
            unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                unsafe {
                    let _ = DestroyWindow(hwnd);
                }
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            unsafe {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe {
                let _ = KillTimer(Some(hwnd), TIMER_KEEP_ON_TOP);
                let _ = KillTimer(Some(hwnd), TIMER_STOP_CHECK);
                PostQuitMessage(0);
            }
            LRESULT(0)
        }
        _ => match method_for(msg, tc) {
            Method::Local => {
                note_local(msg, tc, wp, lp);
                unsafe { DefWindowProcW(hwnd, msg, wp, lp) }
            }
            m => pass_on_other(msg, wp, lp, m),
        },
    }
}

// ---- Ctrl+C and friends ----

#[link(name = "kernel32")]
unsafe extern "system" {
    fn SetConsoleCtrlHandler(handler: Option<unsafe extern "system" fn(u32) -> i32>, add: i32) -> i32;
}

/// Runs on a thread of its own: asks the window to close, which ends the run the same way `--seconds` does.
unsafe extern "system" fn ctrl_handler(_kind: u32) -> i32 {
    let h = OUR_HWND.load(Ordering::SeqCst);
    if h != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd_of(h)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        1
    } else {
        0
    }
}

// ---- main ----

fn parse_args() -> Option<Config> {
    let mut args = std::env::args().skip(1);
    let mut run = false;
    let mut cfg = Config {
        seconds: 120,
        out: std::env::temp_dir().join("rf-tray-spy.log"),
        stop_file: None,
        broadcast_start: false,
        broadcast_exit: false,
        show: false,
        allow_no_target: false,
        accept_other_hosts: false,
        full_hex: false,
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--run" => run = true,
            "--seconds" => cfg.seconds = args.next()?.parse().ok()?,
            "--out" => cfg.out = PathBuf::from(args.next()?),
            "--stop-file" => cfg.stop_file = Some(PathBuf::from(args.next()?)),
            "--broadcast-start" => cfg.broadcast_start = true,
            "--broadcast-exit" => cfg.broadcast_exit = true,
            "--show" => cfg.show = true,
            "--allow-no-target" => cfg.allow_no_target = true,
            "--accept-other-hosts" => cfg.accept_other_hosts = true,
            "--full-hex" => cfg.full_hex = true,
            _ => return None,
        }
    }
    run.then_some(cfg)
}

fn usage() {
    eprintln!("tray_spy: creates a window of class Shell_TrayWnd that logs and forwards the tray traffic (see the file header).");
    eprintln!("It does nothing without --run. Options: --run --seconds N --out FILE --stop-file FILE --broadcast-start --broadcast-exit");
    eprintln!("         --show --allow-no-target --accept-other-hosts --full-hex");
}

fn broadcast_taskbar_created(tc: u32) {
    unsafe {
        let _ = SendNotifyMessageW(HWND_BROADCAST, tc, WPARAM(0), LPARAM(0));
    }
}

fn main() {
    let Some(cfg) = parse_args() else {
        usage();
        return;
    };
    // physical pixels everywhere: the rectangle copied from Explorer's tray window must match what programs compute from it
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "tray_spy.exe".into());
    let policy = PickPolicy { accept_other_hosts: cfg.accept_other_hosts };
    let tc = unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) };

    // is there a real tray to forward to? if not, refuse to start unless told to
    let probe_me = SelfIdentity { hwnd: 0, pid: std::process::id(), exe: exe.clone() };
    let before = tray_windows();
    // only Explorer's own window opens the gate: a window of unknown owner could be another relay
    if !real_tray_present(&before, &probe_me) && !cfg.allow_no_target {
        eprintln!("tray_spy: no Shell_TrayWnd owned by explorer.exe was found, so there is nothing to forward to. Not starting (--allow-no-target overrides).");
        return;
    }
    if let Action::Forward { target } = decide(&before, &probe_me, policy) {
        eprintln!("tray_spy: will forward to window {target:#x}");
    }

    // an existing file is appended to only when it is empty or an earlier log of this program
    if cfg.out.exists() {
        let mut head = [0u8; 64];
        let n = std::fs::File::open(&cfg.out).and_then(|mut f| f.read(&mut head)).unwrap_or(usize::MAX);
        if n == usize::MAX || !existing_log_acceptable(&head[..n]) {
            eprintln!("tray_spy: {} exists and is not a tray_spy log (or cannot be read). Not touching it; pick another --out.", cfg.out.display());
            return;
        }
    }
    let mut file = match OpenOptions::new().create(true).append(true).open(&cfg.out) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("tray_spy: cannot open the log {}: {e}", cfg.out.display());
            return;
        }
    };
    if file.metadata().map(|m| m.len() == 0).unwrap_or(true) {
        let _ = writeln!(file, "{LOG_HEADER}");
    }

    let deadline = (cfg.seconds > 0).then(|| Instant::now() + Duration::from_secs(cfg.seconds));
    let (broadcast_start, broadcast_exit, show) = (cfg.broadcast_start, cfg.broadcast_exit, cfg.show);
    let out_path = cfg.out.clone();
    STATE.with(|s| {
        *s.borrow_mut() = Some(State {
            cfg,
            start: Instant::now(),
            log: std::io::BufWriter::with_capacity(LOG_BUFFER, file),
            me: SelfIdentity { hwnd: 0, pid: std::process::id(), exe },
            policy,
            taskbar_created: tc,
            counts: Counts::default(),
            ticks: 0,
            deadline,
            local_seen: HashSet::new(),
        });
    });

    unsafe {
        let hinst = match GetModuleHandleW(None) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("tray_spy: module handle: {e}");
                return;
            }
        };
        let class = w!("Shell_TrayWnd");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: Some(wndproc),
            hbrBackground: GetSysColorBrush(COLOR_HIGHLIGHT),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        if RegisterClassExW(&wc) == 0 {
            eprintln!("tray_spy: RegisterClassExW failed: {}", windows::core::Error::from_thread());
            return;
        }
        // created 1x1 and hidden; mirror_rect gives it the real tray's rectangle below. `--show` makes it visible at that size.
        let hwnd = match CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!(""),
            WS_POPUP | WS_CLIPCHILDREN | WS_CLIPSIBLINGS,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        ) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("tray_spy: CreateWindowExW failed: {e}");
                let _ = UnregisterClassW(class, Some(hinst.into()));
                return;
            }
        };
        OUR_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        STATE.with(|s| {
            if let Some(st) = s.borrow_mut().as_mut() {
                st.me.hwnd = hwnd.0 as isize;
            }
        });
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), 1);

        logln(&format!(
            "tray_spy started: pid {}, window {:#x}, log {}, seconds {}, broadcast start/exit {}/{}, TaskbarCreated id {tc}",
            std::process::id(),
            hwnd.0 as isize,
            out_path.display(),
            deadline.map(|_| "limited").unwrap_or("unlimited"),
            broadcast_start,
            broadcast_exit
        ));
        for w in tray_windows() {
            logln(&format!("  tray window {:#x} class {} pid {} exe {:?}", w.hwnd, w.class, w.pid, w.exe));
        }
        eprintln!("tray_spy: running, window {:#x}, logging to {}", hwnd.0 as isize, out_path.display());

        mirror_rect(hwnd);
        if show {
            // click-through first: an unpainted topmost rectangle over the taskbar would otherwise eat the user's clicks
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_LAYERED.0 | WS_EX_TRANSPARENT.0) as isize);
            let _ = SetLayeredWindowAttributes(hwnd, Default::default(), SHOW_ALPHA, LWA_ALPHA);
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        }
        keep_on_top(hwnd);
        SetTimer(Some(hwnd), TIMER_KEEP_ON_TOP, KEEP_ON_TOP_MS, None);
        SetTimer(Some(hwnd), TIMER_STOP_CHECK, STOP_CHECK_MS, None);

        if broadcast_start {
            logln("broadcasting TaskbarCreated (start)");
            broadcast_taskbar_created(tc);
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // the window is gone: Explorer's tray window is first in the z-order again
        OUR_HWND.store(0, Ordering::SeqCst);
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), 0);
        let _ = UnregisterClassW(class, Some(hinst.into()));
    }

    if broadcast_exit {
        logln("broadcasting TaskbarCreated (exit)");
        broadcast_taskbar_created(tc);
    }
    STATE.with(|s| {
        if let Some(st) = s.borrow_mut().as_mut() {
            let c = &st.counts;
            let line = format!(
                "summary: {} COPYDATA (appbar {}, notify {}, service {}, rect {}, other kinds {}), {} not understood by the parser, {} forwarded, {} refused, {} timed out, {} target gone, {} other messages passed on",
                c.messages, c.by_kind[0], c.by_kind[1], c.by_kind[2], c.by_kind[3], c.other_kind, c.unparsed, c.forwarded, c.refused, c.timed_out, c.target_gone, c.passed_on_other
            );
            let t = st.start.elapsed().as_secs_f64();
            let _ = writeln!(st.log, "[+{t:9.3}s] {line}");
            let _ = st.log.flush();
            eprintln!("tray_spy: {line}");
        }
    });
}
