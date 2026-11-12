//! Carries out the state machine's actions through the `Sys` traits and feeds the results back as
//! events. This is the only loop of the guard; everything it decides comes from `machine::step`.

use std::collections::VecDeque;

use crate::machine::{Action, Event, Facts, Outcome, Phase, State, step};
use crate::statefile::ShellStateFile;
use crate::sys::{HostLaunch, Sys, Wait};

pub struct RunConfig {
    pub launch: HostLaunch,
    /// `--panic` on the command line.
    pub panic: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct RunReport {
    pub outcome: Option<Outcome>,
    pub exit_code: i32,
}

/// Builds the machine's facts from what was read. Pure, so `--dry-run` shares it.
/// `value_present`: whether the Shell value is set (`None`: could not be read).
#[allow(clippy::too_many_arguments)]
pub fn make_facts(
    file: &ShellStateFile,
    build: Option<String>,
    safe_mode: bool,
    shift_held: bool,
    panic: bool,
    panic_delegated: bool,
    value_present: Option<bool>,
    explorer_is_shell: bool,
) -> Facts {
    Facts {
        safe_mode,
        explorer_is_shell,
        shift_held,
        panic,
        panic_delegated,
        // A state file we could not read may have hidden a pending restore: restoring twice is harmless.
        pending_restore: file.pending_restore() || file.is_corrupt(),
        // The first run, an unknown build and a changed build all get the self-test.
        build_changed: !(build.is_some() && file.last_known_good_build().map(str::to_owned) == build),
        // A recorded failure to clear the value only matters while the value is still there.
        unresolved: file.unresolved().filter(|_| value_present != Some(false)),
        crashes: file.crashes(),
        build,
    }
}

/// What the guard knows before its first event.
pub fn gather_facts(sys: &mut Sys, cfg: &RunConfig, file: &ShellStateFile) -> Facts {
    let build = sys.session.build();
    let panic_delegated = cfg.panic && sys.session.signal_running_guard_panic();
    let value_present = sys.registry.read_shell_value().ok().map(|v| v.is_some());
    let (safe_mode, shift_held) = (sys.session.safe_mode(), sys.session.shift_held());
    // --panic wants Explorer whatever is running, so it does not ask.
    let explorer_is_shell = !cfg.panic && sys.procs.explorer_owns_shell_window();
    make_facts(file, build, safe_mode, shift_held, cfg.panic, panic_delegated, value_present, explorer_is_shell)
}

struct Runner<'s, 'a> {
    sys: &'s mut Sys<'a>,
    cfg: &'s RunConfig,
    /// The state file was unreadable when the guard started, so a restore is assumed pending until one has run.
    start_corrupt: bool,
    exit_code: i32,
    log: &'s mut dyn FnMut(&str),
}

impl Runner<'_, '_> {
    /// Read-modify-write of the state file. The host writes to the same file while it runs (work areas it changed,
    /// the pending-restore flag), so the guard never writes back a copy it read earlier: it reads the file again
    /// here, changes only its own keys and writes. `change` returns whether there is anything to write.
    fn update(&mut self, change: impl FnOnce(&mut ShellStateFile) -> bool) -> bool {
        let mut file = self.sys.store.load();
        if !change(&mut file) {
            return true;
        }
        match self.sys.store.save(&file) {
            Ok(()) => true,
            Err(e) => {
                (self.log)(&format!("state file not written: {e}"));
                false
            }
        }
    }

    /// The host is gone (or was ended): its process id must not stay in the file for a later `--panic` to act on.
    fn forget_host(&mut self) {
        self.update(|f| {
            let had = f.host_pid().is_some();
            f.set_host_pid(None);
            had
        });
    }

    /// Performs one action and returns the event that reports it.
    fn exec(&mut self, action: &Action) -> Option<Event> {
        (self.log)(&format!("action: {}", action.describe()));
        let kind = action.kind();
        let ok = match action {
            Action::ClaimIdentity => self.sys.session.create_shell_ready_event().map_err(|e| (self.log)(&format!("ShellReadyEvent: {e}"))).is_ok(),
            Action::StartHost => {
                return Some(match self.sys.procs.spawn_host(&self.cfg.launch) {
                    Ok(pid) => {
                        // So that `--panic` can end this host even if this guard is gone by then.
                        self.update(|f| {
                            f.set_host_pid(Some(pid));
                            true
                        });
                        Event::HostStarted { pid }
                    }
                    Err(e) => {
                        (self.log)(&format!("the host did not start: {e}"));
                        Event::HostSpawnFailed
                    }
                });
            }
            Action::SignalShellReady => self.sys.session.signal_shell_ready().map_err(|e| (self.log)(&format!("ShellReadyEvent: {e}"))).is_ok(),
            Action::ReleaseIdentity => {
                self.sys.session.release_identity();
                true
            }
            Action::ClearShellValue => match self.sys.registry.delete_shell_value() {
                Ok(was_there) => {
                    (self.log)(if was_there { "the Shell value is deleted" } else { "there was no Shell value" });
                    true
                }
                Err(e) => {
                    (self.log)(&format!("the Shell value could not be deleted: {e}"));
                    false
                }
            },
            Action::KillHost => {
                self.sys.procs.kill_host();
                self.forget_host();
                true
            }
            Action::RestoreWorkAreas => {
                // Read fresh: the rectangles are the host's, written after this guard started.
                let saved = self.sys.store.load().original_work_areas();
                let r = self.sys.workarea.restore(&saved);
                if let Err(e) = &r {
                    (self.log)(&format!("work areas not restored: {e}"));
                } else {
                    let assume_pending = self.start_corrupt;
                    self.update(|f| {
                        let pending = f.pending_restore() || assume_pending;
                        if pending {
                            f.set_pending_restore(false);
                        }
                        pending
                    });
                }
                r.is_ok()
            }
            Action::ShowTaskbar => match self.sys.taskbar.show_windows_taskbar() {
                Ok(_) => true,
                Err(e) => {
                    (self.log)(&format!("taskbar not shown: {e}"));
                    false
                }
            },
            Action::ProbeExplorer => self.sys.procs.explorer_shell_up(0),
            Action::StartExplorer => match self.sys.procs.start_explorer() {
                Ok(()) => true,
                Err(e) => {
                    (self.log)(&format!("explorer.exe did not start: {e}"));
                    false
                }
            },
            Action::WaitExplorerUp { ms } => self.sys.procs.explorer_shell_up(*ms),
            Action::SelfTest => self.sys.procs.host_self_test(&self.cfg.launch),
            Action::WriteState(p) => self.update(|f| {
                f.apply(p);
                true
            }),
            Action::ShowMessage(m) => {
                self.sys.ui.message(*m);
                true
            }
            Action::Exit { code } => {
                self.exit_code = *code;
                return None;
            }
        };
        Some(Event::Done { kind, ok })
    }
}

/// Runs the guard until the state machine is done. Returns how it ended.
pub fn run_guard(sys: &mut Sys, cfg: &RunConfig, log: &mut dyn FnMut(&str)) -> RunReport {
    let file = sys.store.load();
    (log)(&format!("state file: {:?}", file.status));
    let facts = gather_facts(sys, cfg, &file);
    (log)(&format!("facts: {facts:?}"));
    if !cfg.panic && let Err(e) = sys.hotkey.arm() {
        // Not fatal: the Shift-at-sign-in route and --panic remain.
        (log)(&format!("the escape chord could not be armed: {e}"));
    }

    // --panic that found no guard to hand the job to: a host that outlived its guard is ended by the id the guard left.
    if cfg.panic && !facts.panic_delegated && let Some(pid) = file.host_pid() {
        sys.procs.adopt_host(pid, &cfg.launch.exe);
    }
    let mut r = Runner { sys, cfg, start_corrupt: file.is_corrupt(), exit_code: 0, log };
    let mut state = State::new();
    let mut queue: VecDeque<Event> = VecDeque::from([Event::Boot(facts)]);
    loop {
        while let Some(ev) = queue.pop_front() {
            let now = r.sys.clock.now_ms();
            (r.log)(&format!("event: {}", short(&ev)));
            let (next, actions) = step(state, ev, now);
            state = next;
            for a in &actions {
                if let Some(e) = r.exec(a) {
                    queue.push_back(e);
                }
            }
            if state.is_done() {
                return RunReport { outcome: state.outcome(), exit_code: r.exit_code };
            }
        }
        let now = r.sys.clock.now_ms();
        let extra = r.sys.hotkey.waitable();
        let ev = match r.sys.procs.wait(extra, state.next_wake_ms(now)) {
            Wait::HostExited { pid, code } => {
                r.forget_host();
                // A host that ends while Windows signs the user out or shuts down has not crashed and has not been asked to
                // leave: it was told to close. Whatever its exit code, the guard leaves everything as it is.
                if r.sys.session.session_ending() { Event::SessionEnding } else { Event::HostExited { pid, code } }
            }
            Wait::HostReady => Event::HostReady,
            // The wake of the escape event: the press is still there for `take_pressed` (the event is manual-reset, so the
            // wait did not use it up). Not pressed: the wake was the session ending, or nothing, and the loop goes on.
            Wait::Extra => {
                if r.sys.session.session_ending() {
                    Event::SessionEnding
                } else if r.sys.hotkey.take_pressed() {
                    Event::EscapePressed
                } else {
                    continue;
                }
            }
            Wait::Timeout => Event::Tick { host_hung: matches!(state.phase, Phase::Running { .. }) && r.sys.procs.host_hung() },
        };
        queue.push_back(ev);
    }
}

fn short(ev: &Event) -> String {
    match ev {
        Event::Boot(_) => "Boot".into(),
        Event::Done { kind, ok } => format!("Done({kind:?}, {ok})"),
        other => format!("{other:?}"),
    }
}
