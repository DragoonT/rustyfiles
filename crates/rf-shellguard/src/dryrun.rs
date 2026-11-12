//! `--dry-run`: prints what the guard would do and does none of it.
//!
//! How to read the proof that it changes nothing: this file calls only `winro` (read-only queries),
//! `ShellStateFile::peek` (a file read; it never moves or writes a file) and `std::fs::metadata`
//! (does the host program exist). It never builds a `Sys`, a `Runner` or any real implementation
//! from `win/`. `tests/dryrun_proof.rs` fails if a write, window, hook or process API name shows
//! up in this file or in `winro.rs`.

use std::path::{Path, PathBuf};

use crate::beside;
use crate::cli::{Args, Mode, Simulation};
use crate::machine::{Action, ActionKind, Event, Millis, Phase, State, step};
use crate::paths;
use crate::runner::make_facts;
use crate::statefile::{self, ShellStateFile};
use crate::winro;

/// What was read from the system.
pub struct Inputs {
    pub shell_value: Result<Option<String>, String>,
    pub override_policy: Result<Option<u32>, String>,
    pub safe_mode: bool,
    pub shift_held: bool,
    pub build: Option<String>,
    pub state: ShellStateFile,
    pub state_path: PathBuf,
    pub host: PathBuf,
    pub host_exists: bool,
}

/// Reads. Nothing here writes.
pub fn gather(args: &Args) -> Inputs {
    let dir = paths::state_dir(args.state_dir.as_deref(), &|k| std::env::var_os(k));
    let state_path = statefile::path_in(&dir);
    let state = ShellStateFile::peek(&state_path);
    let host = paths::host_exe(args.host.as_deref(), &state, std::env::current_exe().ok().as_deref());
    let host_exists = std::fs::metadata(&host).map(|m| m.is_file()).unwrap_or(false);
    Inputs {
        shell_value: winro::read_shell_value(),
        override_policy: winro::read_override_policy(),
        safe_mode: winro::safe_mode(),
        shift_held: winro::shift_held(),
        build: winro::build(),
        state,
        state_path,
        host,
        host_exists,
    }
}

/// A program's file name only: reports name programs, not folders.
fn program_name(command: &str) -> String {
    let first = if let Some(rest) = command.strip_prefix('"') { rest.split('"').next().unwrap_or("") } else { command.split(' ').next().unwrap_or("") };
    Path::new(first).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "?".into())
}

pub fn render(inp: &Inputs, args: &Args, now: Millis) -> Vec<String> {
    let mut out = vec![
        "rustyfiles-shellguard --dry-run".to_string(),
        "Nothing is written, no window is created, no program is started, no hotkey or hook is registered.".to_string(),
        String::new(),
    ];
    if let Mode::Beside(pid) = args.mode {
        out.push(format!("Mode: beside-guard for process {pid} (not probed in a dry run)."));
        out.push("If the host is alive: wait on its process handle, doing nothing else. When it exits:".into());
        let mut s = beside::BState::Watching;
        let mut ev = beside::BEvent::HostGone;
        for _ in 0..4 {
            let (next, actions) = beside::step(s, ev);
            for a in &actions {
                out.push(format!("  would: {}", a.describe()));
            }
            s = next;
            ev = match actions.first() {
                Some(beside::BAction::ShowTaskbar) => beside::BEvent::Taskbar(beside::TaskbarResult::Shown),
                Some(beside::BAction::ClearPending) => beside::BEvent::PendingCleared,
                _ => break,
            };
        }
        return out;
    }

    out.push("Read from this PC (read-only):".into());
    out.push(match &inp.shell_value {
        Ok(Some(v)) => format!("  Winlogon Shell value (current user): set (program: {})", program_name(v)),
        Ok(None) => "  Winlogon Shell value (current user): not set, so Windows starts Explorer".into(),
        Err(e) => format!("  Winlogon Shell value (current user): could not be read ({e})"),
    });
    out.push(match &inp.override_policy {
        Ok(Some(v)) => format!("  OverrideShellProgram policy: {v}{}", if *v == 1 { " (the organisation sets the shell: the user's value is ignored)" } else { "" }),
        Ok(None) => "  OverrideShellProgram policy: not set".into(),
        Err(e) => format!("  OverrideShellProgram policy: could not be read ({e})"),
    });
    out.push(format!("  Safe Mode: {}   Shift held now: {}   Windows build: {}", inp.safe_mode, inp.shift_held, inp.build.as_deref().unwrap_or("unknown")));
    out.push(format!("  State file: {:?}", inp.state.status));
    out.push(format!(
        "  Shell host program: {} ({})",
        inp.host.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        if inp.host_exists { "found" } else { "NOT found next to the guard" }
    ));
    let would_start = matches!(&inp.shell_value, Ok(Some(_))) && inp.host_exists;
    out.push(format!("  Would the host be started by a real run at sign-in: {}", if would_start { "yes" } else { "no (see above)" }));
    out.push("  (A real run also checks whether Explorer already owns the desktop and then starts no host; a dry run does not probe that.)".into());
    out.push(String::new());

    let value_present = match &inp.shell_value {
        Ok(v) => Some(v.is_some()),
        Err(_) => None,
    };
    let facts = make_facts(&inp.state, inp.build.clone(), inp.safe_mode, inp.shift_held, args.panic, false, value_present, false);
    out.push("Plan, step by step (every step assumed to succeed; Explorer assumed not to be up yet):".into());
    if args.panic {
        out.push("  (--panic first asks a running guard to do this itself and starts nothing if one answers)".into());
    }
    out.extend(simulate(facts, args.simulate, now));
    out
}

/// Runs the pure machine against a world where every action succeeds.
pub fn simulate(facts: crate::machine::Facts, sim: Option<Simulation>, start: Millis) -> Vec<String> {
    let mut lines = Vec::new();
    let mut st = State::new();
    let mut t = start;
    let mut queue = std::collections::VecDeque::from([Event::Boot(facts)]);
    let mut n = 0;
    let mut crashes_fed = 0;
    for _ in 0..400 {
        let Some(ev) = queue.pop_front() else {
            // Nothing pending: the guard would now wait. Feed the chosen scenario, or stop.
            match (&st.phase, sim) {
                (Phase::Running { pid, .. }, Some(Simulation::CrashLoop)) => {
                    lines.push("  -- scenario: the host crashes 1 s after it started, again and again --".into());
                    t += 1_000;
                    queue.push_back(Event::HostExited { pid: *pid, code: 1 });
                    crashes_fed += 1;
                }
                (Phase::Running { pid, .. }, Some(Simulation::UserExit)) => {
                    lines.push("  -- scenario: the user chooses \"Return to Explorer\" (the host exits with code 78) --".into());
                    t += 60_000;
                    queue.push_back(Event::HostExited { pid: *pid, code: crate::machine::EXIT_LEAVE });
                }
                (Phase::Running { .. }, Some(Simulation::Escape)) => {
                    lines.push("  -- scenario: Ctrl+Alt+Shift+E is pressed --".into());
                    t += 60_000;
                    queue.push_back(Event::EscapePressed);
                }
                (Phase::Running { .. }, None) => {
                    lines.push("  then: the guard waits on the host's process handle, the escape chord and a 10 s hang check; nothing more happens until one of them fires.".into());
                    break;
                }
                (Phase::Backoff { delay, .. }, _) => {
                    t += delay;
                    queue.push_back(Event::Tick { host_hung: false });
                }
                (Phase::Starting { .. }, _) => queue.push_back(Event::HostReady),
                _ => break,
            }
            continue;
        };
        let (next, actions) = step(st, ev, t);
        st = next;
        for a in actions {
            n += 1;
            lines.push(format!("  {n:>2}. would: {}", a.describe()));
            match a {
                Action::StartHost => queue.push_back(Event::HostStarted { pid: 1000 + crashes_fed }),
                Action::Exit { .. } => return lines,
                // Explorer is assumed not to be up yet, so the sequence shows the start; the wait then succeeds.
                other => {
                    let ok = other.kind() != ActionKind::ProbeExplorer;
                    queue.push_back(Event::Done { kind: other.kind(), ok });
                }
            }
        }
        if let Some(o) = st.outcome() {
            lines.push(format!("  (ended: {o:?})"));
            break;
        }
    }
    lines
}
