//! The hand-over of a launch to the rustyfiles that is already running (GitHub issue 15: "a folder only flickers and does not open on
//! the first try, the second try works").
//!
//! THE CAUSE. A second launch used to write its folder into the first instance's pipe and exit at once: it never learned whether
//! anything came of it. The first instance can be on its way out at that moment. Closing the last window starts a quit that is not
//! instant: the settings are saved, the name index is stopped (up to two seconds, on the UI thread), GPUI's quit is a task queued
//! behind everything already queued, and only then does the process end. For that whole time the process still holds the instance
//! mutex and still serves the pipe, so a launch in it is "forwarded" to a process that is leaving. What happens to the message
//! depends on the moment: if it arrived before the quit task was queued, the leaving process opens a window for it and the quit
//! task then closes it (the flicker); if it arrived later, it is dropped without a trace (nothing at all). Either way the second
//! process has already gone, so nobody shows the folder. The next try finds the mutex free and starts a process of its own.
//!
//! THE FIX, in three parts.
//! 1. The pipe talks both ways. The first instance answers `q` when it has taken the request, then `ok` once its UI thread has
//!    acted on it (a tab or a window is open), `leaving` when it is on its way out, or `failed` when it took the request and could
//!    not make a window for it. A second process leaves on `ok`, and also when it has waited [`Limits::stall`] for a first instance
//!    that has the request and is not getting to it: that one still holds the request and will show it, so a second window is
//!    never made beside it (that was a double window for one request).
//! 2. The quit is decided under a lock shared with the pipe ([`Gate`]): a request that has been taken and is not acted on yet
//!    stops the quit (the window it opens keeps the process), and once the quit is decided, requests are answered `leaving`.
//! 3. A second process that is told `leaving` (or sees the pipe close with no word) does not give up: it waits for the leaving
//!    process to be gone and becomes the first instance itself, showing the folder. One that is told `failed`, or finds no pipe at
//!    all, shows the folder itself at once. The time limits are in [`Limits`] and the choices in [`decide`], both pure and tested.
//!
//! The second process also hands the first its right to take the foreground (`AllowSetForegroundWindow`): it was started by
//! whoever has the foreground (Explorer, the taskbar), the first instance was not, and without the grant the first instance's
//! window comes up behind the others with only its taskbar button flashing.
//!
//! An older rustyfiles (from before this) has a pipe that only takes writes: opening it for reading is refused, which is how a
//! newer second launch knows it can get no answer and falls back to the old fire-and-forget write. A newer first instance takes
//! the old fire-and-forget writes too (they just never see an answer).
//!
//! `RUSTYFILES_TRACE_HANDOVER=1` writes the steps with time marks to `rustyfiles-handover.log` in the temp folder (or to the
//! file the variable names); off, a step costs one atomic load.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, ERROR_ACCESS_DENIED, ERROR_ALREADY_EXISTS, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT, PeekNamedPipe};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::{HRESULT, PCWSTR};

use crate::single_instance::{InstanceGuard, Role, instance_name, names_for};
use crate::wide::wide;

// ---------------------------------------------------------------------------------------------------------------------------
// the rules (pure)
// ---------------------------------------------------------------------------------------------------------------------------

/// The time limits of a hand-over.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// How long a second launch looks for the first instance's pipe while the mutex says there is one (it may be just starting, or
    /// just ending).
    pub connect: Duration,
    /// How long, counted from the send, the second launch stays for the first instance's last word. Past this it ends anyway and
    /// the request stays with the first instance (it was written to it: it shows it when its UI thread gets to it, and showing it
    /// here as well would be a second window). Long on purpose: a first instance still starting up on a loaded PC, or held by a
    /// modal loop, is slow, not gone; a gone one closes the pipe, which is heard at once.
    pub stall: Duration,
    /// How long a second launch waits for a leaving first instance to be gone before it shows the folder beside it.
    pub exit_wait: Duration,
}

pub const LIMITS: Limits = Limits {
    connect: Duration::from_millis(3_000),
    stall: Duration::from_millis(30_000),
    exit_wait: Duration::from_millis(6_000),
};

/// What the second launch has read from the first instance so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Seen {
    pub accepted: bool,
    pub done: bool,
    pub leaving: bool,
    /// It took the request and could not make a window for it.
    pub refused: bool,
    /// The pipe is closed and nothing is left to read.
    pub closed: bool,
}

/// What a line from the first instance means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Accepted,
    Done,
    Leaving,
    Refused,
}

pub fn parse_reply(line: &str) -> Option<Reply> {
    match line.trim() {
        "q" => Some(Reply::Accepted),
        "ok" => Some(Reply::Done),
        "leaving" => Some(Reply::Leaving),
        "failed" => Some(Reply::Refused),
        _ => None,
    }
}

impl Seen {
    pub fn note(&mut self, r: Reply) {
        match r {
            Reply::Accepted => self.accepted = true,
            Reply::Done => self.done = true,
            Reply::Leaving => self.leaving = true,
            Reply::Refused => self.refused = true,
        }
    }
}

/// Why a request did not come to a window in the first instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// It said it is leaving.
    Leaving,
    /// Its end of the pipe closed without the last word (the process is going down, or the request was dropped).
    Dropped,
    /// It did not even say it had the request, within [`Limits::stall`].
    Silent,
    /// It took the request and did not act on it within [`Limits::stall`].
    Slow,
    /// It took the request and could not show it.
    Refused,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Wait,
    Handed,
    Failed(Why),
}

/// What to make of the answers so far, `since_sent` after the request was written.
pub fn judge(seen: Seen, since_sent: Duration, lim: &Limits) -> Verdict {
    if seen.done {
        return Verdict::Handed;
    }
    if seen.leaving {
        return Verdict::Failed(Why::Leaving);
    }
    if seen.refused {
        return Verdict::Failed(Why::Refused);
    }
    if seen.closed {
        return Verdict::Failed(Why::Dropped);
    }
    if since_sent >= lim.stall {
        return Verdict::Failed(if seen.accepted { Why::Slow } else { Why::Silent });
    }
    Verdict::Wait
}

/// The state of the first instance as a second launch comes to know it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum First {
    /// No mutex: nobody runs (or the one that did has just gone).
    Gone,
    /// It took the request and acted on it.
    Answering,
    /// A copy from before the two-way pipe: the request was written, no answer is possible.
    Old,
    /// It is on its way out.
    Leaving,
    /// It runs but has no pipe to talk to (not up yet and not coming, or gone): nothing was handed over.
    Unresponsive,
    /// The request was written to it and it did not answer in time (a frozen or very slow UI thread). It still holds the request.
    Stalled,
    /// It took the request and could not make a window for it.
    Refused,
}

/// Who shows the folder, and what the second launch has to do first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The first instance shows it; the second launch ends.
    pub first_shows: bool,
    /// The second launch must not show the folder beside the first instance: it asks again until that one is gone (and then is the
    /// first instance itself, or hands the request to the one that took over), within [`Limits::exit_wait`].
    pub wait_for_exit: bool,
}

/// The one table: who shows what for the state of the first instance. There is always exactly one that shows it: the first
/// instance when it answered, when it is too old to be asked, or when it holds the request and is slow (it is the only one that
/// has it, and it will show it); otherwise this process.
pub fn decide(first: First) -> Decision {
    match first {
        First::Answering | First::Old | First::Stalled => Decision { first_shows: true, wait_for_exit: false },
        First::Gone | First::Unresponsive | First::Refused => Decision { first_shows: false, wait_for_exit: false },
        First::Leaving => Decision { first_shows: false, wait_for_exit: true },
    }
}

impl From<Why> for First {
    fn from(w: Why) -> First {
        match w {
            Why::Leaving | Why::Dropped => First::Leaving,
            Why::Silent | Why::Slow => First::Stalled,
            Why::Refused => First::Refused,
        }
    }
}

/// The in-flight bookkeeping of the first instance, shared by the pipe's threads and the UI thread. The quit and the arrival of a
/// request are decided under one lock, so a request is either refused (`leaving`) or it stops the quit; it cannot be taken and
/// then lost to a quit that was decided a moment before.
#[derive(Default)]
struct GateState {
    leaving: bool,
    /// Requests taken and not yet acted on.
    inflight: u32,
    /// A quit was refused because of `inflight`: when the last one is acted on, the quit has to be looked at again.
    deferred: bool,
}

pub struct Gate(Mutex<GateState>);

impl Gate {
    pub const fn new() -> Gate {
        Gate(Mutex::new(GateState { leaving: false, inflight: 0, deferred: false }))
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A request arrives: true when it is taken (it must be [`finished`](Gate::finished) later), false when the process is leaving.
    pub fn admit(&self) -> bool {
        let mut s = self.lock();
        if s.leaving {
            return false;
        }
        s.inflight += 1;
        true
    }

    /// A taken request has been acted on. True when a quit was refused meanwhile and nothing else is in flight: the caller looks at
    /// the quit again (the request normally opened a window, and then there is nothing to do).
    pub fn finished(&self) -> bool {
        let mut s = self.lock();
        s.inflight = s.inflight.saturating_sub(1);
        let again = s.inflight == 0 && s.deferred;
        if again {
            s.deferred = false;
        }
        again
    }

    /// The process wants to quit. True: go ahead, requests are refused from now on. False: a request is in flight, do not quit; the
    /// last `finished` says when to ask again.
    pub fn try_leave(&self) -> bool {
        let mut s = self.lock();
        if s.leaving {
            return true;
        }
        if s.inflight > 0 {
            s.deferred = true;
            return false;
        }
        s.leaving = true;
        true
    }

    /// The process ends whatever is in flight (an update restart): requests are refused from now on.
    pub fn leave_now(&self) {
        self.lock().leaving = true;
    }

    pub fn is_leaving(&self) -> bool {
        self.lock().leaving
    }

    /// A quit that was decided is called off (kept for a quit that could not go through).
    pub fn stay(&self) {
        self.lock().leaving = false;
    }
}

impl Default for Gate {
    fn default() -> Self {
        Gate::new()
    }
}

/// This process's gate.
pub static GATE: Gate = Gate::new();

// ---------------------------------------------------------------------------------------------------------------------------
// time marks
// ---------------------------------------------------------------------------------------------------------------------------

fn trace_sink() -> Option<&'static std::path::PathBuf> {
    static SINK: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    SINK.get_or_init(|| {
        let v = std::env::var_os("RUSTYFILES_TRACE_HANDOVER").filter(|v| !v.is_empty())?;
        Some(if v == "1" { std::env::temp_dir().join("rustyfiles-handover.log") } else { v.into() })
    })
    .as_ref()
}

/// One step of the hand-over with its time (UTC time of day to the millisecond, so that the lines of the two processes sort, and
/// the process's own age), when `RUSTYFILES_TRACE_HANDOVER` asks for it. The steps carry no paths.
pub fn trace(step: &str) {
    let Some(path) = trace_sink() else { return };
    use std::io::Write;
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
    let day = ms % 86_400_000;
    let line = format!(
        "{:02}:{:02}:{:02}.{:03} pid {:>6} age {:>6} ms  {step}\n",
        day / 3_600_000,
        day / 60_000 % 60,
        day / 1000 % 60,
        day % 1000,
        std::process::id(),
        crate::process_age_ms()
    );
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = f.write_all(line.as_bytes());
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// the first instance: the pipe
// ---------------------------------------------------------------------------------------------------------------------------

/// A taken request. Whoever acts on it calls [`done`](Ack::done); one that is dropped instead closes the pipe without the last word
/// (the second launch then shows the folder itself).
pub struct Ack {
    /// The client's end of the conversation (-1: none, an extra line of a client of the old kind).
    pipe: isize,
    /// Still counted in the gate's requests in flight.
    counted: bool,
    gate: &'static Gate,
}

impl Ack {
    /// The request has been acted on: tells the second launch, which then ends. True when a quit that was refused because of this
    /// request has to be looked at again.
    pub fn done(mut self) -> bool {
        trace("first: request acted on, telling the second launch");
        if self.pipe != -1 {
            say(self.pipe, "ok\n");
        }
        self.release()
    }

    /// The request could not be shown (no window could be made): tells the second launch, which then shows the folder itself.
    /// Same return as [`done`](Ack::done).
    pub fn refuse(mut self) -> bool {
        trace("first: request could not be shown, telling the second launch");
        if self.pipe != -1 {
            say(self.pipe, "failed\n");
        }
        self.release()
    }

    fn release(&mut self) -> bool {
        if self.pipe != -1 {
            // (not DisconnectNamedPipe: that throws away what the client has not read yet)
            unsafe {
                let _ = CloseHandle(HANDLE(self.pipe as *mut _));
            }
            self.pipe = -1;
        }
        std::mem::take(&mut self.counted) && self.gate.finished()
    }
}

impl Drop for Ack {
    fn drop(&mut self) {
        if self.counted {
            trace("first: request dropped without being acted on");
            self.release();
        }
    }
}

fn say(pipe: isize, text: &str) {
    unsafe {
        let _ = WriteFile(HANDLE(pipe as *mut _), Some(text.as_bytes()), None, None);
    }
}

/// Serves the pipe of this instance group on a background thread. `on_message` gets each request and its [`Ack`] (call `done` when
/// the folder is on the screen). Requests are refused with `leaving` once [`GATE`] says the process is leaving.
pub fn serve(on_message: impl Fn(String, Ack) + Send + Sync + 'static) {
    serve_with(&instance_name(), &GATE, on_message)
}

/// [`serve`] for a named group with a gate of its own (the tests use their own, so that they do not leave each other).
pub fn serve_with(app: &str, gate: &'static Gate, on_message: impl Fn(String, Ack) + Send + Sync + 'static) {
    let on_message: Arc<dyn Fn(String, Ack) + Send + Sync> = Arc::new(on_message);
    let (_, pipe_name) = names_for(app);
    std::thread::Builder::new()
        .name("rf-handover-pipe".into())
        .spawn(move || {
            let w = wide(&pipe_name);
            let mut refused = 0u32;
            loop {
                let pipe = unsafe {
                    CreateNamedPipeW(
                        PCWSTR(w.as_ptr()),
                        PIPE_ACCESS_DUPLEX,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        4096,
                        64 * 1024,
                        0,
                        None,
                    )
                };
                if pipe.is_invalid() {
                    // an instance of the same name made by an older copy that is still going down has another mode, and Windows refuses a
                    // second kind of instance: look again for a few seconds before giving up
                    refused += 1;
                    if refused > 60 {
                        log::warn!("single instance pipe failed");
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                    continue;
                }
                refused = 0;
                unsafe {
                    let _ = ConnectNamedPipe(pipe, None);
                }
                // each client on its own thread so a stalled client can't block the next one
                let raw = pipe.0 as isize;
                let cb = on_message.clone();
                std::thread::spawn(move || serve_client(raw, gate, cb));
            }
        })
        .ok();
}

fn serve_client(raw: isize, gate: &'static Gate, cb: Arc<dyn Fn(String, Ack) + Send + Sync>) {
    let pipe = HANDLE(raw as *mut _);
    let mut buf = vec![0u8; 16 * 1024];
    let mut total: Vec<u8> = Vec::new();
    // up to the end of the first line: a client of the two-way kind waits for the answer, so reading to the end of the pipe would
    // wait for it for ever
    loop {
        let mut n = 0u32;
        if unsafe { ReadFile(pipe, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
            break;
        }
        total.extend_from_slice(&buf[..n as usize]);
        if total.contains(&b'\n') || total.len() > 64 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&total).into_owned();
    let mut lines: Vec<&str> = text.lines().collect();
    let Some(last) = lines.pop() else {
        unsafe {
            let _ = CloseHandle(pipe);
        }
        return;
    };
    trace("first: request read from the pipe");
    if !gate.admit() {
        // (a client of the old kind has gone already and never reads this)
        trace("first: leaving, refusing the request");
        say(raw, "leaving\n");
        unsafe {
            let _ = CloseHandle(pipe);
        }
        return;
    }
    say(raw, "q\n");
    trace("first: request taken");
    // (a client of the old kind could in principle send several lines at once: only the last one is waited for)
    for line in lines {
        if gate.admit() {
            cb(line.to_string(), Ack { pipe: -1, counted: true, gate });
        }
    }
    cb(last.to_string(), Ack { pipe: raw, counted: true, gate });
}

// ---------------------------------------------------------------------------------------------------------------------------
// the second launch
// ---------------------------------------------------------------------------------------------------------------------------

/// What one try to hand a request over came to.
#[derive(Debug, PartialEq, Eq)]
enum Sent {
    Handed,
    /// Written to a pipe of the old kind.
    Old,
    Failed(Why),
    /// No pipe to talk to (not up yet, gone, or all instances busy).
    NoPipe,
}

fn open_pipe(w: &[u16], access: u32) -> Result<HANDLE, HRESULT> {
    unsafe {
        CreateFileW(PCWSTR(w.as_ptr()), access, FILE_SHARE_NONE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None).map_err(|e| e.code())
    }
}

/// Lets the first instance (the server of the pipe) take the foreground. This process was started by whoever is in front (Explorer,
/// the taskbar) and so may pass the right on; the first instance was not, and without it Windows refuses its `SetForegroundWindow`
/// and the folder opens behind the other windows with a flashing taskbar button. Granted before the write, so that it is there
/// when the first instance acts. When Windows refuses the grant (this process has no right to pass either) nothing else can be
/// done from here; the trace says so.
fn grant_foreground(h: HANDLE) {
    use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
    use windows::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow;
    let mut pid = 0u32;
    let granted = unsafe { GetNamedPipeServerProcessId(h, &mut pid) }.is_ok() && pid != 0 && unsafe { AllowSetForegroundWindow(pid) }.is_ok();
    trace(if granted { "second: foreground right passed to the first instance" } else { "second: foreground right could not be passed" });
}

fn request(pipe_name: &str, message: &str, lim: &Limits) -> Sent {
    let w = wide(pipe_name);
    let denied = HRESULT::from_win32(ERROR_ACCESS_DENIED.0);
    // asked to read too: a pipe of the old kind refuses that, and then it is written to as before
    let (h, two_way) = match open_pipe(&w, GENERIC_READ.0 | GENERIC_WRITE.0) {
        Ok(h) => (h, true),
        Err(e) if e == denied => match open_pipe(&w, GENERIC_WRITE.0) {
            Ok(h) => (h, false),
            Err(_) => return Sent::NoPipe,
        },
        Err(_) => return Sent::NoPipe,
    };
    grant_foreground(h);
    let mut bytes = message.as_bytes().to_vec();
    bytes.push(b'\n');
    let sent_at = Instant::now();
    let wrote = unsafe { WriteFile(h, Some(&bytes), None, None) }.is_ok();
    let close = |h: HANDLE| unsafe {
        let _ = CloseHandle(h);
    };
    if !wrote {
        close(h);
        return Sent::Failed(Why::Dropped);
    }
    if !two_way {
        close(h);
        return Sent::Old;
    }
    let mut seen = Seen::default();
    let mut pending = Vec::<u8>::new();
    loop {
        // everything that can be read now
        loop {
            let mut avail = 0u32;
            match unsafe { PeekNamedPipe(h, None, 0, None, Some(&mut avail), None) } {
                Ok(()) if avail > 0 => {
                    let mut buf = vec![0u8; avail as usize];
                    let mut n = 0u32;
                    if unsafe { ReadFile(h, Some(&mut buf), Some(&mut n), None) }.is_err() || n == 0 {
                        seen.closed = true;
                        break;
                    }
                    pending.extend_from_slice(&buf[..n as usize]);
                }
                Ok(()) => break,
                Err(_) => {
                    seen.closed = true;
                    break;
                }
            }
        }
        while let Some(i) = pending.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = pending.drain(..=i).collect();
            if let Some(r) = parse_reply(&String::from_utf8_lossy(&line)) {
                seen.note(r);
            }
        }
        match judge(seen, sent_at.elapsed(), lim) {
            Verdict::Wait => std::thread::sleep(Duration::from_millis(3)),
            Verdict::Handed => {
                close(h);
                return Sent::Handed;
            }
            Verdict::Failed(w) => {
                close(h);
                return Sent::Failed(w);
            }
        }
    }
}

enum Taken {
    /// The mutex did not exist: we made it, and so we are the first instance.
    Fresh(HANDLE),
    Exists,
    Error,
}

/// Tries to be the first instance. A handle to an existing mutex is closed again at once: a waiting second launch must not be the
/// one that keeps the mutex alive after the first instance has gone.
fn take_mutex(w: &[u16]) -> Taken {
    unsafe {
        let h = CreateMutexW(None, false, PCWSTR(w.as_ptr()));
        let exists = GetLastError() == ERROR_ALREADY_EXISTS;
        match h {
            Ok(h) if !exists => Taken::Fresh(h),
            Ok(h) => {
                let _ = CloseHandle(h);
                Taken::Exists
            }
            Err(_) => Taken::Error,
        }
    }
}

/// Decides the role of this launch and, when a first instance runs, hands it `message` and waits until its window or tab is there
/// (""= a new tab or window at Home). [`Role::Forwarded`]: the first instance has it, or is of the old kind and has been written to.
/// [`Role::Primary`]: this process shows the folder itself, either because nobody ran or because the one that ran was leaving or
/// did not answer.
pub fn acquire(message: &str) -> Role {
    acquire_in(&instance_name(), message, &LIMITS)
}

/// A second message to the running instance after [`acquire`] returned [`Role::Forwarded`] (the file to select in the folder just
/// handed over). It goes through the same conversation, so it counts as a request in flight in the first instance (a quit cannot
/// pass it) and is waited for. False when the first instance did not take it (it is leaving, or gone): the folder it belonged to is
/// open and only the selection is missing.
pub fn send_followup(message: &str) -> bool {
    let (_, pipe_name) = names_for(&instance_name());
    let begun = Instant::now();
    loop {
        match request(&pipe_name, message, &LIMITS) {
            Sent::Handed | Sent::Old => return true,
            // (the pipe thread has not made its next instance yet: a moment)
            Sent::NoPipe if begun.elapsed() < Duration::from_millis(1_000) => std::thread::sleep(Duration::from_millis(20)),
            other => {
                trace(&format!("second: the follow-up was not taken ({other:?})"));
                return false;
            }
        }
    }
}

/// [`acquire`] for a named group with the given limits (the tests use their own).
pub fn acquire_in(app: &str, message: &str, lim: &Limits) -> Role {
    let (mutex_name, pipe_name) = names_for(app);
    let w = wide(&mutex_name);
    let begun = Instant::now();
    // since when no pipe has been found / the first instance has been leaving (each ends at the first real answer)
    let mut no_pipe_since: Option<Instant> = None;
    let mut leaving_since: Option<Instant> = None;
    trace("second: launch, looking for a first instance");
    loop {
        match take_mutex(&w) {
            Taken::Fresh(h) => {
                trace(&format!("second: no first instance, this process is it (after {} ms)", begun.elapsed().as_millis()));
                return Role::Primary(InstanceGuard(h));
            }
            Taken::Error => return Role::Primary(InstanceGuard(HANDLE::default())),
            Taken::Exists => {}
        }
        let first = match request(&pipe_name, message, lim) {
            Sent::Handed => {
                trace(&format!("second: first instance acted on it after {} ms, this process ends", begun.elapsed().as_millis()));
                First::Answering
            }
            Sent::Old => {
                trace("second: first instance is of the old kind, written without an answer");
                First::Old
            }
            Sent::Failed(why) => {
                trace(&format!("second: first instance did not take it ({why:?}) after {} ms", begun.elapsed().as_millis()));
                no_pipe_since = None;
                why.into()
            }
            Sent::NoPipe => {
                // (it may be starting and has no pipe yet, or ending and has none any more: the mutex is looked at again)
                if no_pipe_since.get_or_insert_with(Instant::now).elapsed() < lim.connect {
                    std::thread::sleep(Duration::from_millis(25));
                    continue;
                }
                trace("second: the first instance has no pipe, showing the folder beside it");
                First::Unresponsive
            }
        };
        let d = decide(first);
        if d.first_shows {
            return Role::Forwarded;
        }
        if d.wait_for_exit {
            // Told that the first instance is going: ask again from the start until it is gone. The mutex is free then and this
            // process is the first instance; or another launch that was waiting too has won it and is up, and then this request is
            // handed to that one (so the others of several launches do not sit out the whole wait).
            if leaving_since.get_or_insert_with(Instant::now).elapsed() < lim.exit_wait {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            trace("second: the first instance is still there, showing the folder beside it");
        }
        // beside a first instance that does not answer: a handle to the same mutex, as before
        return match take_mutex(&w) {
            Taken::Fresh(h) => Role::Primary(InstanceGuard(h)),
            _ => Role::Primary(InstanceGuard(unsafe { CreateMutexW(None, false, PCWSTR(w.as_ptr())) }.unwrap_or_default())),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn replies_are_read_and_only_the_known_ones() {
        assert_eq!(parse_reply("q"), Some(Reply::Accepted));
        assert_eq!(parse_reply("ok\r"), Some(Reply::Done));
        assert_eq!(parse_reply("leaving"), Some(Reply::Leaving));
        assert_eq!(parse_reply("failed"), Some(Reply::Refused));
        assert_eq!(parse_reply("hello"), None);
        assert_eq!(parse_reply(""), None);
    }

    #[test]
    fn the_judge_waits_then_names_the_reason() {
        let l = LIMITS;
        let none = Seen::default();
        assert_eq!(judge(none, ms(0), &l), Verdict::Wait);
        // no time limit but the long one: a first instance that is slow to answer is not taken for a gone one
        assert_eq!(judge(none, ms(2_000), &l), Verdict::Wait);
        assert_eq!(judge(none, l.stall - ms(1), &l), Verdict::Wait);
        assert_eq!(judge(none, l.stall, &l), Verdict::Failed(Why::Silent));
        let taken = Seen { accepted: true, ..none };
        assert_eq!(judge(taken, ms(10_000), &l), Verdict::Wait);
        assert_eq!(judge(taken, l.stall, &l), Verdict::Failed(Why::Slow));
        assert_eq!(judge(Seen { accepted: true, done: true, ..none }, ms(5), &l), Verdict::Handed);
        assert_eq!(judge(Seen { leaving: true, ..none }, ms(5), &l), Verdict::Failed(Why::Leaving));
        assert_eq!(judge(Seen { accepted: true, refused: true, ..none }, ms(5), &l), Verdict::Failed(Why::Refused));
        // a pipe that closed after the last word is a success; before it, a loss
        assert_eq!(judge(Seen { accepted: true, done: true, closed: true, ..none }, ms(5), &l), Verdict::Handed);
        assert_eq!(judge(Seen { accepted: true, closed: true, ..none }, ms(5), &l), Verdict::Failed(Why::Dropped));
        // a refusal that came together with the close is still a refusal
        assert_eq!(judge(Seen { leaving: true, closed: true, ..none }, ms(5), &l), Verdict::Failed(Why::Leaving));
        assert_eq!(judge(Seen { accepted: true, refused: true, closed: true, ..none }, ms(5), &l), Verdict::Failed(Why::Refused));
    }

    #[test]
    fn exactly_one_process_shows_the_folder_in_every_state() {
        // the first shows it, or this process does; never both, never neither
        for first in [First::Gone, First::Answering, First::Old, First::Leaving, First::Unresponsive, First::Stalled, First::Refused] {
            let d = decide(first);
            let me = !d.first_shows;
            assert_eq!(d.first_shows as u8 + me as u8, 1, "{first:?}");
            if d.wait_for_exit {
                assert!(me, "{first:?}: waiting for the first to go only makes sense when this process shows it");
            }
        }
        assert_eq!(decide(First::Answering), Decision { first_shows: true, wait_for_exit: false });
        assert_eq!(decide(First::Old), Decision { first_shows: true, wait_for_exit: false });
        assert_eq!(decide(First::Gone), Decision { first_shows: false, wait_for_exit: false });
        // the case of the issue: the first is leaving, so this process waits for it and shows the folder
        assert_eq!(decide(First::Leaving), Decision { first_shows: false, wait_for_exit: true });
        // no pipe at all: nothing was handed over, this process shows it, with no waiting for a process that is not leaving
        assert_eq!(decide(First::Unresponsive), Decision { first_shows: false, wait_for_exit: false });
        // the first has the request (written, maybe taken) and is slow: it is the one that shows it, never this process as well
        assert_eq!(decide(First::Stalled), Decision { first_shows: true, wait_for_exit: false });
        // it said it could not: this process shows it at once
        assert_eq!(decide(First::Refused), Decision { first_shows: false, wait_for_exit: false });
        assert_eq!(First::from(Why::Leaving), First::Leaving);
        assert_eq!(First::from(Why::Dropped), First::Leaving);
        assert_eq!(First::from(Why::Silent), First::Stalled);
        assert_eq!(First::from(Why::Slow), First::Stalled);
        assert_eq!(First::from(Why::Refused), First::Refused);
    }

    #[test]
    fn the_limits_are_ordered_so_a_slow_start_is_not_taken_for_a_dead_one() {
        let l = LIMITS;
        assert!(l.stall >= ms(20_000), "a first instance still opening its first window (a cold start on a loaded PC) must not be given up on");
        assert!(l.connect >= ms(2_000), "never shorter than the 2 s the old retry loop gave a starting first instance");
        // a launch that has to take over after a quit waits for the quit, not longer than a person waits for a window to come
        assert!(l.exit_wait <= ms(10_000));
    }

    #[test]
    fn a_quit_and_a_request_cannot_pass_each_other() {
        let g = Gate::new();
        // a request in flight stops the quit, and the last one done says to look again
        assert!(g.admit());
        assert!(!g.try_leave());
        assert!(!g.is_leaving());
        assert!(g.finished());
        assert!(!g.finished(), "the quit is asked again once, not for every request");
        // nothing in flight: the quit goes ahead, and from then on requests are refused
        assert!(g.try_leave());
        assert!(!g.admit());
        assert!(g.try_leave(), "asking again while leaving is fine (the 500 ms poll)");
        g.stay();
        assert!(g.admit());
        // two in flight: asked again only after both
        assert!(g.admit());
        assert!(!g.try_leave());
        assert!(!g.finished());
        assert!(g.finished());
        // an update restart ends the process whatever is in flight
        let g = Gate::new();
        assert!(g.admit());
        g.leave_now();
        assert!(!g.admit());
        assert!(g.is_leaving());
    }

    /// A name of its own for each test: a mutex and a pipe are machine-wide.
    fn app(tag: &str) -> String {
        format!("rf-handover-{tag}-{}", std::process::id())
    }

    /// A gate of its own for a test (leaked: the pipe thread of a test never ends).
    fn fresh_gate() -> &'static Gate {
        Box::leak(Box::new(Gate::new()))
    }

    fn quick() -> Limits {
        Limits { connect: ms(600), stall: ms(700), exit_wait: ms(1_500) }
    }

    #[test]
    fn a_running_first_instance_answers_and_the_second_ends() {
        let name = app("answers");
        let Role::Primary(_guard) = acquire_in(&name, "first", &quick()) else { panic!("nobody ran: this one is the first") };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        serve_with(&name, fresh_gate(), move |m, ack| {
            tx.lock().unwrap().send(m).unwrap();
            // "the UI thread" acts a little later
            std::thread::spawn(move || {
                std::thread::sleep(ms(60));
                ack.done();
            });
        });
        let t = Instant::now();
        let role = acquire_in(&name, "folder-a", &quick());
        assert!(matches!(role, Role::Forwarded));
        assert!(t.elapsed() >= ms(55), "the second launch must have waited for the answer, not left on the write: {:?}", t.elapsed());
        assert_eq!(rx.recv_timeout(ms(500)).unwrap(), "folder-a");
    }

    #[test]
    fn a_first_instance_that_is_leaving_does_not_swallow_the_request() {
        // the issue: the first instance has the mutex and the pipe, and is quitting. The old code forwarded to it and the folder was
        // lost. Now the second launch is told, waits for the mutex to be free, and is the first instance itself.
        let name = app("leaving");
        let guard = match acquire_in(&name, "x", &quick()) {
            Role::Primary(g) => g,
            Role::Forwarded => panic!("nobody ran"),
        };
        let handled = Arc::new(AtomicUsize::new(0));
        let h = handled.clone();
        let gate = fresh_gate();
        serve_with(&name, gate, move |_, ack| {
            h.fetch_add(1, Ordering::SeqCst);
            ack.done();
        });
        // the quit is decided: requests are refused from now on
        gate.leave_now();
        let (tx, rx) = mpsc::channel();
        let n = name.clone();
        let t = std::thread::spawn(move || {
            let started = Instant::now();
            let role = acquire_in(&n, "folder-b", &quick());
            tx.send((matches!(role, Role::Primary(_)), started.elapsed())).unwrap();
            role
        });
        // the leaving process takes a while to go (its quit is not instant), then its mutex goes with it
        std::thread::sleep(ms(300));
        drop(guard);
        let (became_first, took) = rx.recv_timeout(ms(3_000)).expect("the second launch must come to a decision");
        let _role = t.join().unwrap();
        assert!(became_first, "told it is leaving, the second launch must show the folder itself");
        assert_eq!(handled.load(Ordering::SeqCst), 0, "the leaving process must not have been given the request");
        assert!(took >= ms(250), "it waited for the first instance to go instead of racing it: {took:?}");
    }

    #[test]
    fn the_wire_tells_leaving_dropped_and_done_apart() {
        // the three answers a first instance can give, read through the real pipe (not only the judge's table)
        let name = app("wire");
        let (_, pipe_name) = names_for(&name);
        let gate = fresh_gate();
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let mode = Arc::new(AtomicUsize::new(0));
        let m = mode.clone();
        serve_with(&name, gate, move |_, ack| match m.load(Ordering::SeqCst) {
            0 => {
                ack.done();
            }
            _ => drop(ack),
        });
        // wait for the pipe to be up
        let t = Instant::now();
        while request(&pipe_name, "warm-up", &quick()) == Sent::NoPipe {
            assert!(t.elapsed() < ms(3_000), "pipe never came up");
            std::thread::sleep(ms(10));
        }
        assert_eq!(request(&pipe_name, "a", &quick()), Sent::Handed);
        mode.store(1, Ordering::SeqCst);
        assert_eq!(request(&pipe_name, "b", &quick()), Sent::Failed(Why::Dropped));
        gate.leave_now();
        assert_eq!(request(&pipe_name, "c", &quick()), Sent::Failed(Why::Leaving));
    }

    #[test]
    fn a_first_instance_that_took_the_request_and_is_slow_is_not_given_a_rival() {
        // the request stays queued in the first instance; showing it beside that one as well made two windows for one request
        let name = app("slow");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let held: Arc<Mutex<Vec<Ack>>> = Arc::new(Mutex::new(Vec::new()));
        let acts = Arc::new(AtomicUsize::new(0));
        let (h, a) = (held.clone(), acts.clone());
        serve_with(&name, fresh_gate(), move |_, ack| {
            a.fetch_add(1, Ordering::SeqCst);
            h.lock().unwrap().push(ack); // a UI thread that has not got to it yet
        });
        let t = Instant::now();
        let role = acquire_in(&name, "folder-g", &quick());
        assert!(matches!(role, Role::Forwarded), "the first instance has the request queued: this process must not show it as well");
        assert!(t.elapsed() >= quick().stall, "it waited for the first instance's word for the whole limit: {:?}", t.elapsed());
        assert_eq!(acts.load(Ordering::SeqCst), 1, "one request, one act");
        // when the first instance gets to it after all, that is the one window; the second launch is long gone
        for ack in held.lock().unwrap().drain(..) {
            ack.done();
        }
    }

    #[test]
    fn a_slow_first_instance_that_answers_before_the_limit_ends_the_wait_at_once() {
        let name = app("late");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        serve_with(&name, fresh_gate(), move |_, ack| {
            std::thread::spawn(move || {
                std::thread::sleep(ms(400));
                ack.done();
            });
        });
        let t = Instant::now();
        assert!(matches!(acquire_in(&name, "folder-h", &quick()), Role::Forwarded));
        assert!(t.elapsed() >= ms(390) && t.elapsed() < quick().stall, "{:?}", t.elapsed());
    }

    #[test]
    fn a_first_instance_that_could_not_make_a_window_is_not_trusted_with_the_request() {
        // it took the request, then had no window to show it in (the GPU, the device): the second launch shows the folder itself, now
        let name = app("refused");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let refused = Arc::new(AtomicUsize::new(0));
        let r = refused.clone();
        serve_with(&name, fresh_gate(), move |_, ack| {
            r.fetch_add(1, Ordering::SeqCst);
            ack.refuse();
        });
        let t = Instant::now();
        let role = acquire_in(&name, "folder-i", &quick());
        assert!(matches!(role, Role::Primary(_)), "told it could not be shown, the second launch shows it");
        assert_eq!(refused.load(Ordering::SeqCst), 1);
        assert!(t.elapsed() < ms(400), "no waiting for a process that said it cannot: {:?}", t.elapsed());
    }

    #[test]
    fn a_refused_request_lets_the_held_back_quit_go_ahead() {
        // the gate's side of the same: a request that is refused is finished like one acted on, and the quit it held back is asked again
        let gate = fresh_gate();
        let name = app("refused-gate");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        serve_with(&name, gate, move |_, ack| {
            tx.lock().unwrap().send(ack).unwrap();
        });
        let n = name.clone();
        let t = std::thread::spawn(move || acquire_in(&n, "folder-j", &quick()));
        let ack = rx.recv_timeout(ms(2_000)).expect("the request must arrive");
        assert!(!gate.try_leave(), "a request in flight stops the quit");
        assert!(ack.refuse(), "the last request finished while a quit was held back: ask again");
        assert!(matches!(t.join().unwrap(), Role::Primary(_)));
        assert!(gate.try_leave(), "nothing in flight now");
    }

    #[test]
    fn a_first_instance_that_stays_leaving_past_the_wait_gets_the_folder_shown_beside_it() {
        // the exit never comes (a process stuck in its teardown): after the wait the folder is shown anyway, not lost
        let name = app("stuck");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let gate = fresh_gate();
        serve_with(&name, gate, move |_, ack| {
            ack.done();
        });
        gate.leave_now();
        let t = Instant::now();
        let role = acquire_in(&name, "folder-k", &quick());
        assert!(matches!(role, Role::Primary(_)));
        let took = t.elapsed();
        assert!(took >= quick().exit_wait && took < quick().exit_wait + ms(1_500), "{took:?}");
    }

    #[test]
    fn two_launches_waiting_for_a_leaving_first_instance_make_one_window_between_them() {
        // both are told `leaving`; when the first goes one takes over and shows its folder, the other hands its folder to that one
        let name = app("two-waiters");
        let guard = match acquire_in(&name, "x", &quick()) {
            Role::Primary(g) => g,
            Role::Forwarded => panic!("nobody ran"),
        };
        let gate = fresh_gate();
        serve_with(&name, gate, |_, ack| {
            ack.done();
        });
        gate.leave_now();
        let firsts = Arc::new(AtomicUsize::new(0));
        let forwarded = Arc::new(AtomicUsize::new(0));
        let taken = Arc::new(Mutex::new(Vec::<String>::new()));
        let guards = Arc::new(Mutex::new(Vec::new()));
        let mut threads = vec![];
        for i in 0..2 {
            let (name, firsts, forwarded, taken, guards) = (name.clone(), firsts.clone(), forwarded.clone(), taken.clone(), guards.clone());
            threads.push(std::thread::spawn(move || match acquire_in(&name, &format!("folder-{i}"), &quick()) {
                Role::Primary(g) => {
                    firsts.fetch_add(1, Ordering::SeqCst);
                    taken.lock().unwrap().push(format!("folder-{i}"));
                    let t2 = taken.clone();
                    // what the app does next: serve, and act on what comes
                    serve_with(&name, fresh_gate(), move |m, ack| {
                        t2.lock().unwrap().push(m);
                        ack.done();
                    });
                    guards.lock().unwrap().push(g);
                }
                Role::Forwarded => {
                    forwarded.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }
        std::thread::sleep(ms(250));
        drop(guard);
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!((firsts.load(Ordering::SeqCst), forwarded.load(Ordering::SeqCst)), (1, 1));
        let mut shown = taken.lock().unwrap().clone();
        shown.sort();
        assert_eq!(shown, ["folder-0", "folder-1"], "each folder is shown exactly once, by the one that took over");
    }

    #[test]
    fn a_request_taken_and_never_acted_on_is_not_lost_either() {
        let name = app("dropped");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        serve_with(&name, fresh_gate(), move |_, ack| drop(ack));
        let role = acquire_in(&name, "folder-c", &quick());
        assert!(matches!(role, Role::Primary(_)), "the pipe closed with no last word: the second launch shows the folder itself");
    }

    #[test]
    fn a_first_instance_without_a_pipe_gets_the_folder_shown_beside_it() {
        // alive (it holds the mutex) but nothing to talk to: the old code waited two seconds and did the same
        let name = app("mute");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let t = Instant::now();
        let role = acquire_in(&name, "folder-d", &quick());
        assert!(matches!(role, Role::Primary(_)));
        assert!(t.elapsed() >= quick().connect && t.elapsed() < ms(2_000), "{:?}", t.elapsed());
    }

    #[test]
    fn the_old_kind_of_pipe_is_still_written_to() {
        // a pipe that only takes writes, as an older copy's: no answer is possible and none is waited for
        let name = app("old");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let (_, pipe_name) = names_for(&name);
        let (tx, rx) = mpsc::channel();
        let w = wide(&pipe_name);
        let ready = Arc::new(std::sync::Barrier::new(2));
        let r2 = ready.clone();
        std::thread::spawn(move || unsafe {
            use windows::Win32::Storage::FileSystem::PIPE_ACCESS_INBOUND;
            let p = CreateNamedPipeW(PCWSTR(w.as_ptr()), PIPE_ACCESS_INBOUND, PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, PIPE_UNLIMITED_INSTANCES, 0, 64 * 1024, 0, None);
            r2.wait();
            let _ = ConnectNamedPipe(p, None);
            let mut buf = vec![0u8; 1024];
            let mut n = 0u32;
            let _ = ReadFile(p, Some(&mut buf), Some(&mut n), None);
            tx.send(String::from_utf8_lossy(&buf[..n as usize]).into_owned()).unwrap();
        });
        ready.wait();
        let t = Instant::now();
        assert!(matches!(acquire_in(&name, "folder-e", &quick()), Role::Forwarded));
        assert!(t.elapsed() < ms(250), "an old pipe cannot answer, so nothing is waited for: {:?}", t.elapsed());
        assert_eq!(rx.recv_timeout(ms(500)).unwrap().trim(), "folder-e");
    }

    #[test]
    fn a_write_only_client_of_the_old_kind_reaches_a_new_first_instance() {
        // the other direction: an older copy as the second launch (it writes and closes at once)
        let name = app("oldclient");
        let Role::Primary(_guard) = acquire_in(&name, "x", &quick()) else { panic!("nobody ran") };
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        serve_with(&name, fresh_gate(), move |m, ack| {
            tx.lock().unwrap().send(m).unwrap();
            ack.done();
        });
        let (_, pipe_name) = names_for(&name);
        let t = Instant::now();
        loop {
            let w = wide(&pipe_name);
            if let Ok(h) = open_pipe(&w, GENERIC_WRITE.0) {
                unsafe {
                    let _ = WriteFile(h, Some(b"folder-f\n"), None, None);
                    let _ = CloseHandle(h);
                }
                break;
            }
            assert!(t.elapsed() < ms(3_000), "pipe never came up");
            std::thread::sleep(ms(10));
        }
        assert_eq!(rx.recv_timeout(ms(1_000)).unwrap(), "folder-f");
    }

    #[test]
    fn two_launches_at_the_same_moment_make_one_first_instance_and_one_forward() {
        let name = app("same");
        let firsts = Arc::new(AtomicUsize::new(0));
        let forwarded = Arc::new(AtomicUsize::new(0));
        let (guards_tx, guards_rx) = mpsc::channel();
        let gate = Arc::new(std::sync::Barrier::new(2));
        let mut threads = vec![];
        for i in 0..2 {
            let (name, firsts, forwarded, gate, guards_tx) = (name.clone(), firsts.clone(), forwarded.clone(), gate.clone(), guards_tx.clone());
            threads.push(std::thread::spawn(move || {
                gate.wait();
                match acquire_in(&name, &format!("folder-{i}"), &quick()) {
                    Role::Primary(g) => {
                        firsts.fetch_add(1, Ordering::SeqCst);
                        // what the app does next: serve, and act on what comes
                        serve_with(&name, fresh_gate(), |_, ack| {
                            ack.done();
                        });
                        guards_tx.send(g).unwrap();
                    }
                    Role::Forwarded => {
                        forwarded.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }));
        }
        for t in threads {
            t.join().unwrap();
        }
        let _g = guards_rx.try_recv().ok();
        assert_eq!((firsts.load(Ordering::SeqCst), forwarded.load(Ordering::SeqCst)), (1, 1));
    }

    #[test]
    fn trace_is_silent_without_the_variable() {
        // (no variable is set in the test run: this only has to not fail and not create anything)
        trace("test step");
    }
}
