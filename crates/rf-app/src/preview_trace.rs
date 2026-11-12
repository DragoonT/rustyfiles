//! Time marks for the preview pane's picture, from "the focused item changed" to "the picture is in hand",
//! written only when the environment variable `RUSTYFILES_PREVIEW_TRACE` is set (a test copy; nothing is
//! read, written or timed otherwise: `on()` is one cached bool).
//!
//! The lines go to `preview-trace.log` in `RUSTYFILES_CONFIG_DIR` (the temp folder when it is not set):
//!
//! ```text
//! +   1234.5 ms [thread] text
//! ```
//!
//! The time is milliseconds since the first mark of the process. Files are named by a running number and
//! their extension and size only, never by name or path.

use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

fn state() -> &'static Option<(Instant, Mutex<std::fs::File>)> {
    static S: OnceLock<Option<(Instant, Mutex<std::fs::File>)>> = OnceLock::new();
    S.get_or_init(|| {
        std::env::var_os("RUSTYFILES_PREVIEW_TRACE")?;
        let dir = std::env::var_os("RUSTYFILES_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let _ = std::fs::create_dir_all(&dir);
        let f = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("preview-trace.log")).ok()?;
        Some((Instant::now(), Mutex::new(f)))
    })
}

pub fn on() -> bool {
    state().is_some()
}

/// One line. Cheap to call when off (returns before the text is built if the caller checks `on()` first).
pub fn mark(text: &str) {
    let Some((t0, f)) = state() else { return };
    if let Ok(mut f) = f.lock() {
        let _ = writeln!(f, "+{:>10.1} ms [{}] {text}", t0.elapsed().as_secs_f64() * 1000.0, std::thread::current().name().unwrap_or("ui"));
    }
}

/// (idle, kernel + user) CPU time of the whole machine, in 100 ns units.
fn cpu_times() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).ok()? };
    let n = |f: FILETIME| (u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime);
    Some((n(idle), n(kernel) + n(user)))
}

/// How busy the whole machine was between two readings of `cpu_times`, in percent.
fn busy_percent(from: Option<(u64, u64)>) -> String {
    match (from, cpu_times()) {
        (Some((i0, t0)), Some((i1, t1))) if t1 > t0 => format!("{:.0} %", 100.0 * (1.0 - (i1 - i0) as f64 / (t1 - t0) as f64)),
        _ => "unknown".to_string(),
    }
}

/// How this process was started: its priority class and whether Windows throttles it (the dialog is a process
/// started by another program, which may be running at a lower class, and a throttled one runs on slow cores).
pub fn process_facts(what: &str) {
    if !on() {
        return;
    }
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetPriorityClass, GetProcessInformation, PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
    };
    unsafe {
        let me = GetCurrentProcess();
        let class = GetPriorityClass(me);
        let mut st = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: 0, StateMask: 0 };
        let ok = GetProcessInformation(me, ProcessPowerThrottling, &mut st as *mut _ as *mut std::ffi::c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32).is_ok();
        mark(&format!(
            "{what}: process priority class 0x{class:x} (0x20 normal, 0x4000 below normal, 0x40 idle), power throttling control 0x{:x} state 0x{:x} (asked ok: {ok})",
            st.ControlMask, st.StateMask
        ));
    }
}

/// What the pane has done for the current target.
#[derive(Default)]
pub struct PaneTrace {
    seq: u32,
    selected_at: Option<Instant>,
    cpu_at_select: Option<(u64, u64)>,
    asked: bool,
    shown: bool,
}

impl PaneTrace {
    /// The focused item changed (`Preview::set_target`).
    pub fn selected(&mut self, ext: &str, size: u64) {
        if !on() {
            return;
        }
        self.seq += 1;
        self.selected_at = Some(Instant::now());
        self.cpu_at_select = cpu_times();
        self.asked = false;
        self.shown = false;
        mark(&format!("pane #{} selected: .{ext}, {} KB", self.seq, size / 1024));
    }

    /// The pane asked the cache for the picture (the first render that wanted one).
    pub fn asked(&mut self, px: u32, urgent: bool, held: bool) {
        if !on() || self.asked || self.selected_at.is_none() {
            return;
        }
        self.asked = true;
        let since = self.selected_at.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
        mark(&format!("pane #{} asks for the picture at {px} px (urgent lane: {urgent}), {since:.1} ms after selecting; previous picture held meanwhile: {held}", self.seq));
    }

    /// A render had the picture (it is drawn in this frame).
    pub fn shown(&mut self) {
        if !on() || self.shown || self.selected_at.is_none() {
            return;
        }
        self.shown = true;
        let since = self.selected_at.map_or(0.0, |t| t.elapsed().as_secs_f64() * 1000.0);
        mark(&format!("pane #{} PICTURE IN HAND {since:.1} ms after selecting; the machine was {} busy meanwhile", self.seq, busy_percent(self.cpu_at_select)));
    }
}

/// An answer of the thumbnail pool: every urgent one, and the list's own when it was slow.
pub fn pool_answer(urgent: bool, dropped: bool, ok: bool, waited: Duration, took: Duration) {
    if !on() {
        return;
    }
    let slow = waited > Duration::from_millis(500) || took > Duration::from_millis(250);
    if urgent || slow {
        mark(&format!(
            "pool answer: {} lane, dropped {dropped}, picture {ok}, waited {:.1} ms in the queue, made in {:.1} ms",
            if urgent { "urgent" } else { "worker" },
            waited.as_secs_f64() * 1000.0,
            took.as_secs_f64() * 1000.0
        ));
    }
}
