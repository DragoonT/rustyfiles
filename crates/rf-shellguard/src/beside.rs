//! `--beside-guard <host pid>` (plan 6.6): Explorer is still the shell, the dock host hides the
//! Windows taskbar, and this small process waits on the host's process handle. When the host dies
//! for any reason it shows the Windows taskbar again. No polling: the wait is the process handle.
//!
//! A pure machine like the main one, with its own actions, so the retry rule is tested without Windows.

use crate::sys::{PidWait, Sys};

/// Explorer may be restarting just then (a crash of the dock is often not alone): try this often, this far apart.
pub const RETRIES: u8 = 5;
pub const RETRY_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskbarResult {
    /// At least one taskbar window was shown.
    Shown,
    /// Found none: Explorer has no taskbar right now.
    NoneFound,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BState {
    Init,
    Watching,
    Showing { attempt: u8 },
    Retrying { attempt: u8, last_failed: bool },
    Clearing { code: i32 },
    Done { code: i32 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BEvent {
    Boot { host_alive: bool },
    HostGone,
    Taskbar(TaskbarResult),
    Delayed,
    PendingCleared,
    SessionEnding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BAction {
    ShowTaskbar,
    Delay { ms: u64 },
    /// Put the saved work areas back and clear `pending_restore` in the state file: the taskbar is visible again.
    ClearPending,
    Exit { code: i32 },
}

impl BAction {
    pub fn describe(&self) -> String {
        match self {
            BAction::ShowTaskbar => "show the Windows taskbar again".into(),
            BAction::Delay { ms } => format!("wait {ms} ms (Explorer may be restarting)"),
            BAction::ClearPending => "restore the saved work areas and clear pending_restore in the state file".into(),
            BAction::Exit { code } => format!("exit with code {code}"),
        }
    }
}

pub fn step(st: BState, ev: BEvent) -> (BState, Vec<BAction>) {
    use BAction as A;
    use BState as S;
    match (st, ev) {
        (S::Done { code }, _) => (S::Done { code }, vec![]),
        (_, BEvent::SessionEnding) => (S::Done { code: 0 }, vec![A::Exit { code: 0 }]),
        (S::Init, BEvent::Boot { host_alive: true }) => (S::Watching, vec![]),
        // The host was already gone when we started: the taskbar may be hidden with nobody to show it.
        (S::Init, BEvent::Boot { host_alive: false }) | (S::Watching, BEvent::HostGone) => (S::Showing { attempt: 1 }, vec![A::ShowTaskbar]),
        (S::Showing { attempt }, BEvent::Taskbar(r)) => match r {
            TaskbarResult::Shown => (S::Clearing { code: 0 }, vec![A::ClearPending]),
            _ if attempt < RETRIES => (S::Retrying { attempt, last_failed: r == TaskbarResult::Failed }, vec![A::Delay { ms: RETRY_MS }]),
            // Out of tries. Found none: nothing was hidden that we can reach, finish. Failed: keep the flag, report it.
            TaskbarResult::NoneFound => (S::Clearing { code: 0 }, vec![A::ClearPending]),
            TaskbarResult::Failed => (S::Done { code: 1 }, vec![A::Exit { code: 1 }]),
        },
        (S::Retrying { attempt, .. }, BEvent::Delayed) => (S::Showing { attempt: attempt + 1 }, vec![A::ShowTaskbar]),
        (S::Clearing { code }, BEvent::PendingCleared) => (S::Done { code }, vec![A::Exit { code }]),
        (st, _) => (st, vec![]),
    }
}

/// Runs the beside-guard to its end and returns the exit code.
pub fn run_beside(sys: &mut Sys, pid: u32, log: &mut dyn FnMut(&str)) -> i32 {
    let alive = sys.procs.wait_pid(pid, 0, Some(0)) != PidWait::Gone;
    log(&format!("beside-guard: host pid {pid} alive at start: {alive}"));
    let mut state = BState::Init;
    let mut pending = std::collections::VecDeque::from([BEvent::Boot { host_alive: alive }]);
    loop {
        while let Some(ev) = pending.pop_front() {
            let (next, actions) = step(state, ev);
            state = next;
            for a in actions {
                log(&format!("beside-guard: {}", a.describe()));
                match a {
                    BAction::ShowTaskbar => {
                        let r = match sys.taskbar.show_windows_taskbar() {
                            Ok(0) => TaskbarResult::NoneFound,
                            Ok(_) => TaskbarResult::Shown,
                            Err(e) => {
                                log(&format!("beside-guard: showing the taskbar failed: {e}"));
                                TaskbarResult::Failed
                            }
                        };
                        pending.push_back(BEvent::Taskbar(r));
                    }
                    BAction::Delay { ms } => {
                        // Process id 0 with no extra handle is a plain timed wait.
                        let _ = sys.procs.wait_pid(0, 0, Some(ms));
                        pending.push_back(BEvent::Delayed);
                    }
                    BAction::ClearPending => {
                        // `pending_restore` is one flag for everything the host changed: work areas as well as the
                        // taskbar. Clearing it after only the taskbar came back would leave a shrunk work area that no
                        // later start looks for, so the work areas are put back first, and if that fails the flag stays.
                        let f = sys.store.load();
                        let mut restored = true;
                        if f.pending_restore()
                            && let Err(e) = sys.workarea.restore(&f.original_work_areas())
                        {
                            log(&format!("beside-guard: work areas not restored: {e}"));
                            restored = false;
                        }
                        if restored {
                            // Read again for the write: the host's other keys may have changed since.
                            let mut f = sys.store.load();
                            f.set_pending_restore(false);
                            if let Err(e) = sys.store.save(&f) {
                                log(&format!("beside-guard: state file: {e}"));
                            }
                        }
                        pending.push_back(BEvent::PendingCleared);
                    }
                    BAction::Exit { code } => return code,
                }
            }
        }
        if let BState::Done { code } = state {
            return code;
        }
        // Watching: block on the host's process handle. Nothing else wakes this process.
        let _ = sys.procs.wait_pid(pid, 0, None);
        if sys.session.session_ending() {
            pending.push_back(BEvent::SessionEnding);
        } else {
            pending.push_back(BEvent::HostGone);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use BAction as A;
    use BState as S;

    #[test]
    fn host_alive_means_watch_and_nothing_else() {
        let (s, a) = step(S::Init, BEvent::Boot { host_alive: true });
        assert_eq!((s, a), (S::Watching, vec![]));
    }

    #[test]
    fn host_already_dead_at_start_shows_the_taskbar_at_once() {
        let (s, a) = step(S::Init, BEvent::Boot { host_alive: false });
        assert_eq!((s, a), (S::Showing { attempt: 1 }, vec![A::ShowTaskbar]));
    }

    #[test]
    fn host_death_shows_then_clears_the_flag_then_exits_zero() {
        let (s, a) = step(S::Watching, BEvent::HostGone);
        assert_eq!(a, vec![A::ShowTaskbar]);
        let (s, a) = step(s, BEvent::Taskbar(TaskbarResult::Shown));
        assert_eq!(a, vec![A::ClearPending]);
        let (s, a) = step(s, BEvent::PendingCleared);
        assert_eq!((s, a), (S::Done { code: 0 }, vec![A::Exit { code: 0 }]));
    }

    #[test]
    fn explorer_restarting_is_retried_with_a_pause_then_gives_up() {
        let mut s = S::Showing { attempt: 1 };
        for attempt in 1..RETRIES {
            let (n, a) = step(s, BEvent::Taskbar(TaskbarResult::Failed));
            assert_eq!(a, vec![A::Delay { ms: RETRY_MS }], "attempt {attempt}");
            let (n, a) = step(n, BEvent::Delayed);
            assert_eq!(a, vec![A::ShowTaskbar]);
            s = n;
        }
        assert_eq!(s, S::Showing { attempt: RETRIES });
        // Out of tries on an error: the flag stays set, exit code 1.
        let (s, a) = step(s, BEvent::Taskbar(TaskbarResult::Failed));
        assert_eq!((s, a), (S::Done { code: 1 }, vec![A::Exit { code: 1 }]));
    }

    #[test]
    fn no_taskbar_found_after_all_tries_is_a_clean_finish() {
        let (s, a) = step(S::Showing { attempt: RETRIES }, BEvent::Taskbar(TaskbarResult::NoneFound));
        assert_eq!((s, a), (S::Clearing { code: 0 }, vec![A::ClearPending]));
    }

    #[test]
    fn a_taskbar_that_appears_on_a_retry_ends_it() {
        let (s, _) = step(S::Showing { attempt: 1 }, BEvent::Taskbar(TaskbarResult::NoneFound));
        let (s, _) = step(s, BEvent::Delayed);
        let (_, a) = step(s, BEvent::Taskbar(TaskbarResult::Shown));
        assert_eq!(a, vec![A::ClearPending]);
    }

    #[test]
    fn session_end_exits_without_touching_anything_from_any_state() {
        for s in [S::Init, S::Watching, S::Showing { attempt: 2 }, S::Retrying { attempt: 2, last_failed: true }, S::Clearing { code: 0 }] {
            let (n, a) = step(s, BEvent::SessionEnding);
            assert_eq!((n, a), (S::Done { code: 0 }, vec![A::Exit { code: 0 }]));
        }
    }

    #[test]
    fn done_is_final_and_stale_events_are_ignored() {
        assert_eq!(step(S::Done { code: 1 }, BEvent::HostGone), (S::Done { code: 1 }, vec![]));
        assert_eq!(step(S::Watching, BEvent::Delayed), (S::Watching, vec![]));
        assert_eq!(step(S::Watching, BEvent::PendingCleared), (S::Watching, vec![]));
    }
}
