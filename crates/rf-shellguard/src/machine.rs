//! The guard's state machine (plan section 6.2) as a pure type: `step(state, event, now)` returns
//! the next state and the actions to carry out. It touches nothing: no clock (the caller passes
//! `now`), no registry, no process. The runner (runner.rs) executes the actions through traits and
//! feeds the results back as events, so every rule here is tested with plain values.
//!
//! The one place where the machine hands out a single action at a time is the fall-back sequence:
//! each step's result decides the next (clearing the Shell value may fail, Explorer may not start),
//! so each action is answered by an `Event::Done` before the next is issued.

/// Milliseconds since the Unix epoch (wall clock). The persisted crash log needs a clock that
/// survives a restart of the guard, so the machine uses this one and defends itself against it
/// jumping (see `normalize`).
pub type Millis = u64;

/// Three crashes within this window mean a crash loop.
pub const CRASH_WINDOW_MS: Millis = 60_000;
pub const CRASH_LIMIT: usize = 3;
/// A host that dies within this long after it started has crashed "fast"; two fast crashes in a row are a loop.
pub const FAST_CRASH_MS: Millis = 3_000;
pub const FAST_CRASH_LIMIT: usize = 2;
/// Six crashes within ten minutes are a loop even when each ran for a while.
pub const LONG_WINDOW_MS: Millis = 600_000;
pub const LONG_LIMIT: usize = 6;
/// A host that has not said "ready" by then is treated as crashed.
pub const READY_DEADLINE_MS: Millis = 30_000;
/// How often a running host is checked for a hung window, and how many hung checks in a row kill it.
pub const HANG_TICK_MS: Millis = 10_000;
pub const HANG_TICKS: u32 = 2;
/// Wait before the n-th restart in the last ten minutes (0 s, 1 s, 3 s; the last repeats).
pub const BACKOFF_MS: [Millis; 3] = [0, 1_000, 3_000];
/// How long the fall-back waits for Explorer's desktop to show up.
pub const EXPLORER_WAIT_MS: Millis = 10_000;
/// A crash stamped further in the future than this (a clock that was wrong) is thrown away at load.
pub const FUTURE_TOLERANCE_MS: Millis = 86_400_000;
/// Host exit code meaning "the user asked to leave the shell": the guard returns to Explorer.
/// Deliberately not 0: a host also exits 0 when Windows signs the user out or shuts down, when it hands
/// over to another instance of itself, or after a clean stop it decided on alone, and none of those may
/// turn the switch off. 0 means "ended without being asked to leave" (see `EXIT_CLEAN`).
pub const EXIT_LEAVE: i32 = 78;
/// Host exit code 0: it ended on its own accord and said nothing about leaving. The guard starts it again
/// (a host that does this right after starting is counted as a crash, so it cannot loop for ever).
pub const EXIT_CLEAN: i32 = 0;
/// Host exit code meaning "restart me" (settings applied, update installed): restarted without counting as a crash.
pub const EXIT_RESTART: i32 = 77;

const MAX_CRASHES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Safe Mode: Explorer is started, the Shell value is left alone.
    SafeMode,
    /// Shift held at sign-in: Explorer once, the value is left alone.
    UserSkip,
    /// `--panic`.
    Panic,
    SelfTestFailed,
    /// The host exited with `EXIT_LEAVE`.
    UserExit,
    /// The escape chord.
    Escape,
    CrashLoop,
}

impl Reason {
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::SafeMode => "safe_mode",
            Reason::UserSkip => "user_skip",
            Reason::Panic => "panic",
            Reason::SelfTestFailed => "self_test_failed",
            Reason::UserExit => "user_exit",
            Reason::Escape => "escape",
            Reason::CrashLoop => "crash_loop",
        }
    }

    pub fn parse(s: &str) -> Option<Reason> {
        Some(match s {
            "safe_mode" => Reason::SafeMode,
            "user_skip" => Reason::UserSkip,
            "panic" => Reason::Panic,
            "self_test_failed" => Reason::SelfTestFailed,
            "user_exit" => Reason::UserExit,
            "escape" => Reason::Escape,
            "crash_loop" => Reason::CrashLoop,
            _ => return None,
        })
    }

    /// Safe Mode and the Shift skip start Explorer for this one logon only; every other reason turns the switch off.
    pub fn deletes_value(self) -> bool {
        !matches!(self, Reason::SafeMode | Reason::UserSkip)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crash {
    pub at: Millis,
    /// How long the host had been running.
    pub uptime_ms: Millis,
}

/// Everything the runner learned before the first event.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Facts {
    pub safe_mode: bool,
    pub shift_held: bool,
    pub panic: bool,
    /// `--panic` found a running guard and handed the job to it: this process starts nothing.
    pub panic_delegated: bool,
    /// The state file says a change to work areas or the taskbar was not undone, or it was corrupt (then we cannot know).
    pub pending_restore: bool,
    /// The Windows build differs from the last one a host ran on.
    pub build_changed: bool,
    /// explorer.exe already owns the desktop (a visible shell window of Explorer): the host would be started on top of it.
    pub explorer_is_shell: bool,
    /// A previous fall-back wrote its reason but could not clear the Shell value.
    pub unresolved: Option<Reason>,
    pub crashes: Vec<Crash>,
    pub build: Option<String>,
}

/// What the runner writes into the state file.
#[derive(Clone, Debug, PartialEq)]
pub enum Persist {
    Crashes(Vec<Crash>),
    Ready { build: Option<String>, crashes: Vec<Crash> },
    /// Written first in a fall-back: the reason, with "value not cleared yet" until proven otherwise.
    FallbackStart { reason: Reason, crashes: Vec<Crash> },
    /// Written last: what came of it.
    FallbackEnd { reason: Reason, cleared: bool, explorer_up: bool },
}

/// What the message box (the only UI the guard has) says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Message {
    /// The Shell value is still set: the next sign-in would start the guard again.
    pub value_left: bool,
    /// Explorer did not come up.
    pub explorer_down: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// Create the session's `ShellReadyEvent` (not yet signalled). Idempotent.
    ClaimIdentity,
    StartHost,
    /// Signal `ShellReadyEvent`: the desktop is usable.
    SignalShellReady,
    /// Close the shell identity (later also the shim window). Must happen before Explorer starts.
    ReleaseIdentity,
    ClearShellValue,
    KillHost,
    RestoreWorkAreas,
    ShowTaskbar,
    /// Is a visible Progman owned by explorer.exe already there? (no waiting)
    ProbeExplorer,
    StartExplorer,
    WaitExplorerUp { ms: Millis },
    SelfTest,
    WriteState(Persist),
    ShowMessage(Message),
    Exit { code: i32 },
}

/// The kind of an action, as echoed back in `Event::Done`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    ClaimIdentity,
    StartHost,
    SignalShellReady,
    ReleaseIdentity,
    ClearShellValue,
    KillHost,
    RestoreWorkAreas,
    ShowTaskbar,
    ProbeExplorer,
    StartExplorer,
    WaitExplorerUp,
    SelfTest,
    WriteState,
    ShowMessage,
    Exit,
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::ClaimIdentity => ActionKind::ClaimIdentity,
            Action::StartHost => ActionKind::StartHost,
            Action::SignalShellReady => ActionKind::SignalShellReady,
            Action::ReleaseIdentity => ActionKind::ReleaseIdentity,
            Action::ClearShellValue => ActionKind::ClearShellValue,
            Action::KillHost => ActionKind::KillHost,
            Action::RestoreWorkAreas => ActionKind::RestoreWorkAreas,
            Action::ShowTaskbar => ActionKind::ShowTaskbar,
            Action::ProbeExplorer => ActionKind::ProbeExplorer,
            Action::StartExplorer => ActionKind::StartExplorer,
            Action::WaitExplorerUp { .. } => ActionKind::WaitExplorerUp,
            Action::SelfTest => ActionKind::SelfTest,
            Action::WriteState(_) => ActionKind::WriteState,
            Action::ShowMessage(_) => ActionKind::ShowMessage,
            Action::Exit { .. } => ActionKind::Exit,
        }
    }

    /// One readable line, used by `--dry-run` and the log.
    pub fn describe(&self) -> String {
        match self {
            Action::ClaimIdentity => "create the session's ShellReadyEvent (not signalled yet)".into(),
            Action::StartHost => "start the shell host".into(),
            Action::SignalShellReady => "signal ShellReadyEvent (the desktop is usable)".into(),
            Action::ReleaseIdentity => "release the shell identity (before Explorer starts)".into(),
            Action::ClearShellValue => "delete the Winlogon Shell value of the current user".into(),
            Action::KillHost => "stop the shell host if it is still alive".into(),
            Action::RestoreWorkAreas => "restore the saved work areas".into(),
            Action::ShowTaskbar => "show the Windows taskbar again".into(),
            Action::ProbeExplorer => "check whether Explorer's desktop is already up".into(),
            Action::StartExplorer => "start explorer.exe by its full path".into(),
            Action::WaitExplorerUp { ms } => format!("wait up to {} s for Explorer's desktop", ms / 1000),
            Action::SelfTest => "self-test: the host program is there and starts like a program (it is not run)".into(),
            Action::WriteState(p) => match p {
                Persist::Crashes(c) => format!("write the state file ({} crash(es) logged)", c.len()),
                Persist::Ready { .. } => "write the state file (last known good build)".into(),
                Persist::FallbackStart { reason, .. } => format!("write the state file (fall-back reason: {})", reason.as_str()),
                Persist::FallbackEnd { cleared, explorer_up, .. } => {
                    format!("write the state file (value cleared: {cleared}, Explorer up: {explorer_up})")
                }
            },
            Action::ShowMessage(m) => format!("show a message (value still set: {}, Explorer down: {})", m.value_left, m.explorer_down),
            Action::Exit { code } => format!("exit the guard with code {code}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Boot(Facts),
    /// The result of an action the runner carried out (see `ActionKind`).
    Done { kind: ActionKind, ok: bool },
    HostStarted { pid: u32 },
    HostSpawnFailed,
    /// The host signalled its "ready" event.
    HostReady,
    HostExited { pid: u32, code: i32 },
    /// The wait timed out. `host_hung` is only looked at while the host runs.
    Tick { host_hung: bool },
    EscapePressed,
    /// `--panic` arrived (from this process's command line or another one).
    PanicRequested,
    SessionEnding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The fall-back sequence ran to its end.
    FellBack(Reason),
    SessionEnded,
    /// `--panic` was handed to a running guard.
    Delegated,
    /// Explorer already is the shell of this session (the guard was started by hand, or after a Shift skip):
    /// the host is not started over it.
    ExplorerHasShell,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    /// Before `Event::Boot`.
    Init,
    /// Waiting for the self-test's answer.
    SelfTest,
    /// `StartHost` issued, waiting for `HostStarted` / `HostSpawnFailed`.
    Launching,
    /// The host runs but has not said ready.
    Starting { pid: u32, since: Millis },
    Running { pid: u32, since: Millis, hung_ticks: u32 },
    /// Waiting before the next launch.
    Backoff { since: Millis, delay: Millis },
    Fallback(Fallback),
    Done(Outcome),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FbStep {
    Persist,
    Release,
    Clear,
    Kill,
    Restore,
    Taskbar,
    Probe,
    Start,
    Wait,
    Final,
    Message,
    Exit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExplorerState {
    Unknown,
    AlreadyUp,
    Started,
    Failed,
}

/// The fall-back sequence in progress.
#[derive(Clone, Debug, PartialEq)]
pub struct Fallback {
    reason: Reason,
    step: FbStep,
    cleared: Option<bool>,
    explorer: ExplorerState,
    attempts: u8,
    crashes: Vec<Crash>,
}

impl Fallback {
    pub fn reason(&self) -> Reason {
        self.reason
    }

    fn new(reason: Reason, crashes: Vec<Crash>) -> Fallback {
        Fallback { reason, step: FbStep::Persist, cleared: None, explorer: ExplorerState::Unknown, attempts: 0, crashes }
    }

    fn needs_message(&self) -> bool {
        (self.reason.deletes_value() && self.cleared == Some(false)) || self.explorer == ExplorerState::Failed
    }

    /// The action of the current step (steps that do not apply are skipped).
    fn action(&mut self) -> Action {
        loop {
            match self.step {
                FbStep::Persist => return Action::WriteState(Persist::FallbackStart { reason: self.reason, crashes: self.crashes.clone() }),
                FbStep::Release => return Action::ReleaseIdentity,
                FbStep::Clear if self.reason.deletes_value() => return Action::ClearShellValue,
                FbStep::Clear => self.step = FbStep::Kill,
                FbStep::Kill => return Action::KillHost,
                FbStep::Restore => return Action::RestoreWorkAreas,
                FbStep::Taskbar => return Action::ShowTaskbar,
                FbStep::Probe => return Action::ProbeExplorer,
                FbStep::Start => return Action::StartExplorer,
                FbStep::Wait => return Action::WaitExplorerUp { ms: EXPLORER_WAIT_MS },
                FbStep::Final => {
                    return Action::WriteState(Persist::FallbackEnd {
                        reason: self.reason,
                        cleared: self.cleared == Some(true) || !self.reason.deletes_value(),
                        explorer_up: matches!(self.explorer, ExplorerState::AlreadyUp | ExplorerState::Started),
                    });
                }
                FbStep::Message if self.needs_message() => {
                    return Action::ShowMessage(Message {
                        value_left: self.reason.deletes_value() && self.cleared == Some(false),
                        explorer_down: self.explorer == ExplorerState::Failed,
                    });
                }
                FbStep::Message => self.step = FbStep::Exit,
                FbStep::Exit => return Action::Exit { code: if self.explorer == ExplorerState::Failed { 1 } else { 0 } },
            }
        }
    }

    fn expects(&self) -> ActionKind {
        match self.step {
            FbStep::Persist | FbStep::Final => ActionKind::WriteState,
            FbStep::Release => ActionKind::ReleaseIdentity,
            FbStep::Clear => ActionKind::ClearShellValue,
            FbStep::Kill => ActionKind::KillHost,
            FbStep::Restore => ActionKind::RestoreWorkAreas,
            FbStep::Taskbar => ActionKind::ShowTaskbar,
            FbStep::Probe => ActionKind::ProbeExplorer,
            FbStep::Start => ActionKind::StartExplorer,
            FbStep::Wait => ActionKind::WaitExplorerUp,
            FbStep::Message => ActionKind::ShowMessage,
            FbStep::Exit => ActionKind::Exit,
        }
    }

    /// The result of the current step arrived: record it and move on.
    fn done(&mut self, ok: bool) -> Action {
        self.step = match self.step {
            FbStep::Persist => FbStep::Release,
            FbStep::Release => FbStep::Clear,
            FbStep::Clear => {
                // A failed delete does not stop the way back: Explorer still starts, the user is told.
                self.cleared = Some(ok);
                FbStep::Kill
            }
            FbStep::Kill => FbStep::Restore,
            FbStep::Restore => FbStep::Taskbar,
            FbStep::Taskbar => FbStep::Probe,
            FbStep::Probe => {
                if ok {
                    self.explorer = ExplorerState::AlreadyUp;
                    FbStep::Final
                } else {
                    FbStep::Start
                }
            }
            FbStep::Start if ok => FbStep::Wait,
            FbStep::Start | FbStep::Wait if ok => {
                self.explorer = ExplorerState::Started;
                FbStep::Final
            }
            FbStep::Start | FbStep::Wait => {
                // Two tries in all: start, wait, and if the desktop did not appear, start once more.
                self.attempts += 1;
                if self.attempts < 2 {
                    FbStep::Start
                } else {
                    self.explorer = ExplorerState::Failed;
                    FbStep::Final
                }
            }
            FbStep::Final => FbStep::Message,
            FbStep::Message => FbStep::Exit,
            FbStep::Exit => FbStep::Exit,
        };
        self.action()
    }
}

/// The whole machine state; plain data, cloned by `step`.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub phase: Phase,
    pub crashes: Vec<Crash>,
    /// The last `now` seen, to notice a clock that went backwards.
    pub(crate) last_seen: Millis,
    pub(crate) build: Option<String>,
}

impl State {
    pub fn new() -> State {
        State { phase: Phase::Init, crashes: Vec::new(), last_seen: 0, build: None }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done(_))
    }

    pub fn outcome(&self) -> Option<Outcome> {
        match self.phase {
            Phase::Done(o) => Some(o),
            _ => None,
        }
    }

    /// How long the runner may wait for an external event before it must send a `Tick`.
    /// Never longer than the timer's own length, even if the clock went backwards since the timer was set.
    pub fn next_wake_ms(&self, now: Millis) -> Option<Millis> {
        match &self.phase {
            Phase::Starting { since, .. } => Some((*since + READY_DEADLINE_MS).saturating_sub(now).clamp(1, READY_DEADLINE_MS)),
            Phase::Running { .. } => Some(HANG_TICK_MS),
            Phase::Backoff { since, delay } => Some((*since + *delay).saturating_sub(now).clamp(1, (*delay).max(1))),
            _ => None,
        }
    }
}

impl Default for State {
    fn default() -> Self {
        State::new()
    }
}

/// The host launch actions. Also used for a planned restart.
fn launch_actions() -> Vec<Action> {
    vec![Action::ClaimIdentity, Action::StartHost]
}

/// A clock that went backwards must not freeze a timer (a backoff that waits an hour) or hide a crash:
/// every stored time later than `now` is pulled back to `now`. That errs towards "just happened",
/// which can only make the guard return to Explorer sooner, never keep the user waiting.
fn normalize(st: &mut State, now: Millis) {
    if now < st.last_seen {
        match &mut st.phase {
            Phase::Starting { since, .. } | Phase::Running { since, .. } | Phase::Backoff { since, .. } => *since = (*since).min(now),
            _ => {}
        }
    }
    for c in &mut st.crashes {
        c.at = c.at.min(now);
    }
    // `last_seen` follows the clock down too, so a second backward step is noticed.
    st.last_seen = now;
}

/// Drops crashes too old to matter and the ones stamped impossibly far ahead, keeps the newest `MAX_CRASHES`.
pub fn prune(crashes: &mut Vec<Crash>, now: Millis) {
    crashes.retain(|c| c.at <= now.saturating_add(FUTURE_TOLERANCE_MS) && now.saturating_sub(c.at) <= LONG_WINDOW_MS);
    for c in crashes.iter_mut() {
        c.at = c.at.min(now);
    }
    if crashes.len() > MAX_CRASHES {
        let extra = crashes.len() - MAX_CRASHES;
        crashes.drain(..extra);
    }
}

/// The crash-loop rules of 6.2. `crashes` is oldest first and already pruned.
pub fn is_crash_loop(crashes: &[Crash], now: Millis) -> bool {
    let recent = crashes.iter().filter(|c| now.saturating_sub(c.at) <= CRASH_WINDOW_MS).count();
    if recent >= CRASH_LIMIT {
        return true;
    }
    if crashes.len() >= LONG_LIMIT && crashes.iter().filter(|c| now.saturating_sub(c.at) <= LONG_WINDOW_MS).count() >= LONG_LIMIT {
        return true;
    }
    // Two fast crashes in a row, both in the last minute.
    if crashes.len() >= FAST_CRASH_LIMIT {
        let last = &crashes[crashes.len() - FAST_CRASH_LIMIT..];
        if last.iter().all(|c| c.uptime_ms <= FAST_CRASH_MS && now.saturating_sub(c.at) <= CRASH_WINDOW_MS) {
            return true;
        }
    }
    false
}

/// The wait before the next launch, from how many crashes sit in the log.
pub fn backoff_for(crash_count: usize) -> Millis {
    BACKOFF_MS[crash_count.clamp(1, BACKOFF_MS.len()) - 1]
}

fn enter_fallback(mut st: State, reason: Reason) -> (State, Vec<Action>) {
    let mut fb = Fallback::new(reason, st.crashes.clone());
    let first = fb.action();
    st.phase = Phase::Fallback(fb);
    (st, vec![first])
}

fn crashed(mut st: State, now: Millis, uptime_ms: Millis) -> (State, Vec<Action>) {
    st.crashes.push(Crash { at: now, uptime_ms });
    prune(&mut st.crashes, now);
    if is_crash_loop(&st.crashes, now) {
        return enter_fallback(st, Reason::CrashLoop);
    }
    // Undo what the dead host may have left changed (work areas, a hidden taskbar), then try again.
    let mut actions = vec![Action::WriteState(Persist::Crashes(st.crashes.clone())), Action::RestoreWorkAreas, Action::ShowTaskbar];
    let delay = backoff_for(st.crashes.len());
    if delay == 0 {
        st.phase = Phase::Launching;
        actions.extend(launch_actions());
    } else {
        st.phase = Phase::Backoff { since: now, delay };
    }
    (st, actions)
}

fn launch(mut st: State) -> (State, Vec<Action>) {
    st.phase = Phase::Launching;
    (st, launch_actions())
}

fn boot(mut st: State, facts: Facts, now: Millis) -> (State, Vec<Action>) {
    st.build = facts.build.clone();
    st.crashes = facts.crashes.clone();
    prune(&mut st.crashes, now);
    // --panic comes first: it is the user's explicit "give me Explorer", whatever else is true.
    if facts.panic {
        if facts.panic_delegated {
            st.phase = Phase::Done(Outcome::Delegated);
            return (st, vec![Action::Exit { code: 0 }]);
        }
        return enter_fallback(st, Reason::Panic);
    }
    // A fall-back that wrote its reason but could not clear the value is finished first.
    if let Some(r) = facts.unresolved.filter(|r| r.deletes_value()) {
        return enter_fallback(st, r);
    }
    if facts.safe_mode {
        return enter_fallback(st, Reason::SafeMode);
    }
    if facts.shift_held {
        return enter_fallback(st, Reason::UserSkip);
    }
    if is_crash_loop(&st.crashes, now) {
        return enter_fallback(st, Reason::CrashLoop);
    }
    // Explorer owns the desktop already: starting the host would put two shells on one screen. Nothing is
    // changed and the Shell value stays (this run was not the one Winlogon started, or Explorer was asked for).
    if facts.explorer_is_shell {
        st.phase = Phase::Done(Outcome::ExplorerHasShell);
        return (st, vec![Action::Exit { code: 0 }]);
    }
    let mut actions = Vec::new();
    if facts.pending_restore {
        actions.push(Action::RestoreWorkAreas);
        actions.push(Action::ShowTaskbar);
    }
    if facts.build_changed {
        st.phase = Phase::SelfTest;
        actions.push(Action::SelfTest);
        return (st, actions);
    }
    let (st, more) = launch(st);
    actions.extend(more);
    (st, actions)
}

/// The machine. Pure: the same state, event and time always give the same answer.
pub fn step(mut st: State, ev: Event, now: Millis) -> (State, Vec<Action>) {
    normalize(&mut st, now);
    // Whatever the phase, these end it (the Done phase has nothing left to end).
    match (&ev, &st.phase) {
        (_, Phase::Done(_)) => return (st, vec![]),
        (Event::PanicRequested, Phase::Fallback(fb)) => {
            // A Safe Mode / Shift fall-back that has not yet reached the value is upgraded: panic means "delete it".
            if !fb.reason.deletes_value() && matches!(fb.step, FbStep::Persist | FbStep::Release | FbStep::Clear) {
                let mut fb = fb.clone();
                fb.reason = Reason::Panic;
                st.phase = Phase::Fallback(fb);
            }
            return (st, vec![]);
        }
        (Event::PanicRequested, _) => return enter_fallback(st, Reason::Panic),
        (Event::EscapePressed, Phase::Fallback(_)) => return (st, vec![]),
        (Event::EscapePressed, _) => return enter_fallback(st, Reason::Escape),
        (Event::SessionEnding, _) => {
            st.phase = Phase::Done(Outcome::SessionEnded);
            return (st, vec![Action::Exit { code: 0 }]);
        }
        _ => {}
    }

    match (st.phase.clone(), ev) {
        (Phase::Init, Event::Boot(facts)) => boot(st, facts, now),

        (Phase::SelfTest, Event::Done { kind: ActionKind::SelfTest, ok }) => {
            if ok {
                launch(st)
            } else {
                enter_fallback(st, Reason::SelfTestFailed)
            }
        }

        (Phase::Launching, Event::HostStarted { pid }) => {
            st.phase = Phase::Starting { pid, since: now };
            (st, vec![])
        }
        (Phase::Launching, Event::HostSpawnFailed) => crashed(st, now, 0),

        (Phase::Starting { pid, since }, Event::HostReady) => {
            st.phase = Phase::Running { pid, since, hung_ticks: 0 };
            let ready = Persist::Ready { build: st.build.clone(), crashes: st.crashes.clone() };
            (st, vec![Action::SignalShellReady, Action::WriteState(ready)])
        }
        (Phase::Starting { pid, since }, Event::HostExited { pid: p, .. }) if p == pid => crashed(st, now, now.saturating_sub(since)),
        (Phase::Starting { since, .. }, Event::Tick { .. }) => {
            if now.saturating_sub(since) >= READY_DEADLINE_MS {
                let up = now.saturating_sub(since);
                let (st, mut actions) = crashed(st, now, up);
                actions.insert(0, Action::KillHost);
                (st, actions)
            } else {
                (st, vec![])
            }
        }

        (Phase::Running { pid, since, .. }, Event::HostExited { pid: p, code }) if p == pid => {
            let up = now.saturating_sub(since);
            match code {
                EXIT_LEAVE => enter_fallback(st, Reason::UserExit),
                // A restart request, or a clean exit nobody asked for, is free, unless the host does it
                // again and again right after starting.
                EXIT_RESTART | EXIT_CLEAN if up > FAST_CRASH_MS => launch(st),
                _ => crashed(st, now, up),
            }
        }
        (Phase::Running { pid, since, hung_ticks }, Event::Tick { host_hung }) => {
            let ticks = if host_hung { hung_ticks + 1 } else { 0 };
            if ticks >= HANG_TICKS {
                let (st, mut actions) = crashed(st, now, now.saturating_sub(since));
                actions.insert(0, Action::KillHost);
                (st, actions)
            } else {
                st.phase = Phase::Running { pid, since, hung_ticks: ticks };
                (st, vec![])
            }
        }

        (Phase::Backoff { since, delay }, Event::Tick { .. }) => {
            if now.saturating_sub(since) >= delay {
                launch(st)
            } else {
                (st, vec![])
            }
        }

        (Phase::Fallback(mut fb), Event::Done { kind, ok }) if kind == fb.expects() => {
            let next = fb.done(ok);
            if let Action::Exit { .. } = next {
                st.phase = Phase::Done(Outcome::FellBack(fb.reason));
            } else {
                st.phase = Phase::Fallback(fb);
            }
            (st, vec![next])
        }

        // Anything else is stale (a result from an earlier phase, an exit of a host already replaced).
        _ => (st, vec![]),
    }
}
