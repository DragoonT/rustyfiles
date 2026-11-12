//! Carrying out a plan, over traits. `run_all` itself touches nothing: starting a program, deleting a RunOnce value and waiting are
//! three small traits, and the tests below use fakes for all of them. The real implementations are in `system.rs`; they are
//! compiled and have never been run (nothing in this increment starts a start-up program or deletes a value).
//!
//! Two promises the run keeps:
//!
//! * A RunOnce value that is deleted before its command starts is never lost to a stop request in between: a stop is looked for
//!   between steps, but not between the two halves of a pair (delete + launch, or launch + delete-after). The pair is finished, then
//!   the run stops.
//! * A sign-in is run once. [`run_session`] asks a [`SessionMarker`] first and sets it after a run that was not stopped, so that a
//!   host the guard restarts mid-session does not start the whole list again. (A run that was stopped or that died half way is not
//!   marked: the next run starts what is not running by then, which the "already running" check makes safe.)

use std::cell::RefCell;
use std::collections::HashSet;

use super::model::{Source, StartupEntry};
use super::plan::{DeleteWhen, Plan, SkipReason, Step, clean};

/// What the launcher is asked to start.
pub struct LaunchRequest<'a> {
    pub entry: &'a StartupEntry,
    /// Wait for the process to end, at most this long.
    pub wait_exit_ms: Option<u32>,
    /// Asked while waiting for the process to end: the host wants the run to stop. The wait ends at once.
    pub stop: &'a dyn Fn() -> bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// Started (not waited for).
    Started,
    /// Started and ended, with this exit code.
    Exited(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchError {
    Failed(String),
    /// It started, but did not end in the time asked for.
    TimedOut,
    /// It started, and the wait for its end was given up because the host asked the run to stop.
    Interrupted,
}

pub trait Launcher {
    fn launch(&mut self, req: &LaunchRequest) -> Result<LaunchOutcome, LaunchError>;

    /// Whether a process of this entry's program runs NOW (the list was read some time ago). Asked just before a launch the plan
    /// marked `recheck_running`. The default knows nothing.
    fn already_running(&mut self, _entry: &StartupEntry) -> bool {
        false
    }
}

/// Deletes RunOnce values. The only write in this module's family, and it is behind this trait.
pub trait RunOnceStore {
    fn delete(&mut self, source: Source, name: &str) -> Result<(), String>;
}

/// Time and the shell's readiness.
pub trait Pacer {
    fn wait(&mut self, ms: u32);
    /// Waits until the shell is ready, at most `limit_ms`; `false` when the limit ran out.
    fn wait_shell_ready(&mut self, limit_ms: u32) -> bool;
    /// The host is shutting down: stop starting things.
    fn stop_requested(&mut self) -> bool {
        false
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Skipped(SkipReason),
    Launched,
    /// Launched and waited for; the exit code.
    LaunchedAndEnded(u32),
    LaunchFailed(String),
    LaunchTimedOut,
    /// The wait for a launched command was given up on a stop request; its RunOnce value stays.
    LaunchInterrupted,
    /// A RunOnce value that had to go first could not be deleted, so the command was NOT started (it would run at every sign-in).
    NotLaunchedDeleteFailed,
    Deleted(DeleteWhen),
    DeleteFailed(DeleteWhen, String),
    /// The wait for the shell: whether it was ready or the limit ran out.
    ShellReady(bool),
    /// The step named an entry that is not in the list.
    BadIndex,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    /// Index into the list; `None` for the steps that name no entry.
    pub entry: Option<usize>,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct RunReport {
    pub records: Vec<Record>,
    /// `stop_requested` ended the run before the last step.
    pub stopped_early: bool,
}

impl RunReport {
    pub fn launched(&self) -> usize {
        self.records.iter().filter(|r| matches!(r.outcome, Outcome::Launched | Outcome::LaunchedAndEnded(_))).count()
    }
    pub fn failed(&self) -> usize {
        self.records.iter().filter(|r| matches!(r.outcome, Outcome::LaunchFailed(_) | Outcome::LaunchTimedOut | Outcome::NotLaunchedDeleteFailed)).count()
    }

    /// One line per record, for the start-up log (`shell-startup.log`): source, decision, result. The entry's name is in the line:
    /// this is the user's own log, not a report.
    pub fn log_lines(&self, entries: &[StartupEntry]) -> Vec<String> {
        self.records
            .iter()
            .map(|r| {
                let who = r.entry.and_then(|i| entries.get(i)).map(|e| format!("{} #{} {}", e.source.label(), e.ordinal, clean(&e.name))).unwrap_or_else(|| "-".into());
                let what = match &r.outcome {
                    Outcome::Skipped(reason) => format!("skipped: {}", reason.short()),
                    Outcome::Launched => "launched".into(),
                    Outcome::LaunchedAndEnded(code) => format!("launched, ended with {code}"),
                    Outcome::LaunchFailed(why) => format!("launch failed: {why}"),
                    Outcome::LaunchTimedOut => "launched, did not end in time".into(),
                    Outcome::LaunchInterrupted => "launched, the wait for its end was stopped".into(),
                    Outcome::NotLaunchedDeleteFailed => "not launched: its RunOnce value could not be deleted".into(),
                    Outcome::Deleted(DeleteWhen::BeforeLaunch) => "RunOnce value deleted before launch".into(),
                    Outcome::Deleted(DeleteWhen::AfterRun) => "RunOnce value deleted after the run".into(),
                    Outcome::DeleteFailed(_, why) => format!("RunOnce value not deleted: {why}"),
                    Outcome::ShellReady(true) => "shell ready".into(),
                    Outcome::ShellReady(false) => "shell not ready in time, going on".into(),
                    Outcome::BadIndex => "step names an entry that is not in the list".into(),
                };
                format!("{who}: {what}")
            })
            .collect()
    }
}

/// Says whether this sign-in has been run already. The real one keeps its answer outside the host process (see `system.rs`).
pub trait SessionMarker {
    fn already_ran(&self) -> bool;
    fn mark_ran(&mut self);
}

/// `run_all` for a host: nothing at all when this sign-in already ran the list (`None`), otherwise the run, marked as done when it
/// was not stopped early.
pub fn run_session(
    entries: &[StartupEntry],
    plan: &Plan,
    launcher: &mut dyn Launcher,
    store: &mut dyn RunOnceStore,
    pacer: &mut dyn Pacer,
    marker: &mut dyn SessionMarker,
) -> Option<RunReport> {
    if marker.already_ran() {
        return None;
    }
    let report = run_all(entries, plan, launcher, store, pacer);
    if !report.stopped_early {
        marker.mark_ran();
    }
    Some(report)
}

/// Carries out `plan` over `entries`. A host that may be restarted by the guard uses [`run_session`] instead.
pub fn run_all(entries: &[StartupEntry], plan: &Plan, launcher: &mut dyn Launcher, store: &mut dyn RunOnceStore, pacer: &mut dyn Pacer) -> RunReport {
    let mut report = RunReport::default();
    // entries whose launch may not happen because their value could not be deleted first
    let mut blocked: HashSet<usize> = HashSet::new();
    // entries that were started, waited for, and ended: only those may have a "delete after" done
    let mut ended: HashSet<usize> = HashSet::new();
    let pacer = RefCell::new(pacer);
    let mut push = |entry: Option<usize>, outcome: Outcome| report.records.push(Record { entry, outcome });
    let mut stopped = false;
    // The step that follows is the second half of a pair (the delete of a RunOnce value just done and the launch it was done for, or
    // the launch that ended and the delete after it): a stop request waits until the pair is complete.
    let mut owed = false;
    for step in &plan.steps {
        if !owed && pacer.borrow_mut().stop_requested() {
            stopped = true;
            break;
        }
        owed = false;
        match *step {
            Step::WaitShellReady { limit_ms } => {
                let ready = pacer.borrow_mut().wait_shell_ready(limit_ms);
                push(None, Outcome::ShellReady(ready));
            }
            Step::Wait { ms } => pacer.borrow_mut().wait(ms),
            Step::Skip { entry, reason } => push(Some(entry), Outcome::Skipped(reason)),
            Step::DeleteRunOnce { entry, when } => {
                let Some(e) = entries.get(entry) else {
                    push(Some(entry), Outcome::BadIndex);
                    continue;
                };
                if when == DeleteWhen::AfterRun && !ended.contains(&entry) {
                    // the command did not run to its end: the value stays, and the next sign-in tries again
                    continue;
                }
                match store.delete(e.source, &e.name) {
                    Ok(()) => {
                        push(Some(entry), Outcome::Deleted(when));
                        owed = when == DeleteWhen::BeforeLaunch;
                    }
                    Err(why) => {
                        if when == DeleteWhen::BeforeLaunch {
                            blocked.insert(entry);
                        }
                        push(Some(entry), Outcome::DeleteFailed(when, why));
                    }
                }
            }
            Step::Launch { entry, wait_exit_ms, recheck_running, .. } => {
                let Some(e) = entries.get(entry) else {
                    push(Some(entry), Outcome::BadIndex);
                    continue;
                };
                if blocked.contains(&entry) {
                    push(Some(entry), Outcome::NotLaunchedDeleteFailed);
                    continue;
                }
                if recheck_running && launcher.already_running(e) {
                    push(Some(entry), Outcome::Skipped(SkipReason::AlreadyRunning));
                    continue;
                }
                let stop = || pacer.borrow_mut().stop_requested();
                match launcher.launch(&LaunchRequest { entry: e, wait_exit_ms, stop: &stop }) {
                    Ok(LaunchOutcome::Started) => push(Some(entry), Outcome::Launched),
                    Ok(LaunchOutcome::Exited(code)) => {
                        ended.insert(entry);
                        owed = true;
                        push(Some(entry), Outcome::LaunchedAndEnded(code));
                    }
                    Err(LaunchError::TimedOut) => push(Some(entry), Outcome::LaunchTimedOut),
                    Err(LaunchError::Interrupted) => push(Some(entry), Outcome::LaunchInterrupted),
                    Err(LaunchError::Failed(why)) => push(Some(entry), Outcome::LaunchFailed(why)),
                }
            }
        }
    }
    report.stopped_early = stopped;
    report
}

#[cfg(test)]
mod tests {
    use super::super::model::{Presence, Running};
    use super::super::plan::{PlanOptions, build, test_entry};
    use super::*;
    use Source::*;

    #[derive(Default)]
    struct FakeLauncher {
        started: Vec<String>,
        fail: HashSet<String>,
        time_out: HashSet<String>,
        exits: bool,
        asked_to_wait: Vec<(String, Option<u32>)>,
        /// Names that "run now" when asked just before the launch.
        running_now: HashSet<String>,
        /// Names whose wait is cut short by a stop request (the launcher polls `req.stop`).
        waits_until_stopped: HashSet<String>,
        rechecked: Vec<String>,
    }

    impl Launcher for FakeLauncher {
        fn already_running(&mut self, entry: &StartupEntry) -> bool {
            self.rechecked.push(entry.name.clone());
            self.running_now.contains(&entry.name)
        }
        fn launch(&mut self, req: &LaunchRequest) -> Result<LaunchOutcome, LaunchError> {
            self.asked_to_wait.push((req.entry.name.clone(), req.wait_exit_ms));
            if self.fail.contains(&req.entry.name) {
                return Err(LaunchError::Failed("no".into()));
            }
            self.started.push(req.entry.name.clone());
            if self.waits_until_stopped.contains(&req.entry.name) {
                return if (req.stop)() { Err(LaunchError::Interrupted) } else { Err(LaunchError::TimedOut) };
            }
            if self.time_out.contains(&req.entry.name) {
                return Err(LaunchError::TimedOut);
            }
            Ok(if req.wait_exit_ms.is_some() && self.exits { LaunchOutcome::Exited(0) } else { LaunchOutcome::Started })
        }
    }

    #[derive(Default)]
    struct FakeStore {
        deleted: Vec<(Source, String)>,
        refuse: HashSet<String>,
        // what happened in what order, shared with the launcher through the test's own log
    }

    impl RunOnceStore for FakeStore {
        fn delete(&mut self, source: Source, name: &str) -> Result<(), String> {
            if self.refuse.contains(name) {
                return Err("access denied".into());
            }
            self.deleted.push((source, name.to_string()));
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakePacer {
        waits: Vec<u32>,
        ready_asked: Vec<u32>,
        ready: bool,
        stop_after: Option<usize>,
        calls: usize,
    }

    impl Pacer for FakePacer {
        fn wait(&mut self, ms: u32) {
            self.waits.push(ms);
        }
        fn wait_shell_ready(&mut self, limit_ms: u32) -> bool {
            self.ready_asked.push(limit_ms);
            self.ready
        }
        fn stop_requested(&mut self) -> bool {
            self.calls += 1;
            self.stop_after.is_some_and(|n| self.calls > n)
        }
    }

    fn list() -> Vec<StartupEntry> {
        vec![
            test_entry(RunMachine, 1, "a", r"C:\a.exe", ""),
            test_entry(RunUser, 1, "off", r"C:\off.exe", ""),
            test_entry(RunUser, 2, "b", r"C:\b.exe", ""),
            test_entry(RunOnceUser, 1, "once", r"C:\once.exe", ""),
            test_entry(RunOnceUser, 2, "!after", r"C:\after.exe", ""),
        ]
    }

    #[test]
    fn a_run_starts_the_entries_in_order_waits_where_the_plan_says_and_deletes_runonce_values() {
        let mut l = list();
        l[1].approval = super::super::model::Approval::Disabled { since: None };
        let plan = build(&l, &PlanOptions::default());
        let (mut launcher, mut store, mut pacer) = (FakeLauncher { exits: true, ..Default::default() }, FakeStore::default(), FakePacer { ready: true, ..Default::default() });
        let report = run_all(&l, &plan, &mut launcher, &mut store, &mut pacer);
        assert_eq!(launcher.started, vec!["a", "b", "once", "!after"]);
        assert_eq!(store.deleted, vec![(RunOnceUser, "once".to_string()), (RunOnceUser, "!after".to_string())], "the stored names, prefixes and all");
        assert_eq!(pacer.ready_asked, vec![30_000]);
        assert_eq!(pacer.waits, vec![2_000, 500, 500, 500]);
        assert_eq!(launcher.asked_to_wait.iter().find(|(n, _)| n == "!after").unwrap().1, Some(60_000));
        assert_eq!(launcher.asked_to_wait.iter().find(|(n, _)| n == "a").unwrap().1, None);
        assert_eq!(report.launched(), 4);
        assert_eq!(report.failed(), 0);
        assert!(!report.stopped_early);
        assert!(report.records.iter().any(|r| r.entry == Some(1) && r.outcome == Outcome::Skipped(SkipReason::Disabled)));
    }

    #[test]
    fn the_order_of_delete_and_launch_is_the_plans() {
        // one log for all three fakes
        use std::cell::RefCell;
        use std::rc::Rc;
        let log: Rc<RefCell<Vec<String>>> = Rc::default();
        struct L(Rc<RefCell<Vec<String>>>);
        struct S(Rc<RefCell<Vec<String>>>);
        impl Launcher for L {
            fn launch(&mut self, req: &LaunchRequest) -> Result<LaunchOutcome, LaunchError> {
                self.0.borrow_mut().push(format!("launch {}", req.entry.name));
                Ok(if req.wait_exit_ms.is_some() { LaunchOutcome::Exited(3) } else { LaunchOutcome::Started })
            }
        }
        impl RunOnceStore for S {
            fn delete(&mut self, _: Source, name: &str) -> Result<(), String> {
                self.0.borrow_mut().push(format!("delete {name}"));
                Ok(())
            }
        }
        let l = vec![test_entry(RunOnceUser, 1, "once", r"C:\o.exe", ""), test_entry(RunOnceUser, 2, "!late", r"C:\l.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        run_all(&l, &plan, &mut L(log.clone()), &mut S(log.clone()), &mut FakePacer { ready: true, ..Default::default() });
        assert_eq!(*log.borrow(), vec!["delete once", "launch once", "launch !late", "delete !late"]);
    }

    #[test]
    fn a_runonce_value_that_cannot_be_deleted_first_is_not_started() {
        let l = vec![test_entry(RunOnceUser, 1, "once", r"C:\o.exe", ""), test_entry(RunUser, 1, "b", r"C:\b.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut store = FakeStore { refuse: ["once".to_string()].into(), ..Default::default() };
        let mut launcher = FakeLauncher::default();
        let report = run_all(&l, &plan, &mut launcher, &mut store, &mut FakePacer { ready: true, ..Default::default() });
        assert_eq!(launcher.started, vec!["b"]);
        assert!(report.records.iter().any(|r| r.entry == Some(0) && r.outcome == Outcome::NotLaunchedDeleteFailed));
        assert!(report.records.iter().any(|r| matches!(r.outcome, Outcome::DeleteFailed(DeleteWhen::BeforeLaunch, _))));
        assert_eq!(report.failed(), 1);
    }

    #[test]
    fn a_delete_after_is_done_only_when_the_command_ran_to_its_end() {
        let l = vec![test_entry(RunOnceUser, 1, "!timeout", r"C:\t.exe", ""), test_entry(RunOnceUser, 2, "!fail", r"C:\f.exe", ""), test_entry(RunOnceUser, 3, "!ok", r"C:\k.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut launcher = FakeLauncher { exits: true, time_out: ["!timeout".to_string()].into(), fail: ["!fail".to_string()].into(), ..Default::default() };
        let mut store = FakeStore::default();
        let report = run_all(&l, &plan, &mut launcher, &mut store, &mut FakePacer { ready: true, ..Default::default() });
        assert_eq!(store.deleted, vec![(RunOnceUser, "!ok".to_string())], "the others stay for the next sign-in");
        assert!(report.records.iter().any(|r| r.outcome == Outcome::LaunchTimedOut));
        assert!(report.records.iter().any(|r| matches!(r.outcome, Outcome::LaunchFailed(_))));
        assert_eq!(report.failed(), 2);
    }

    #[test]
    fn a_launch_that_fails_does_not_stop_the_run() {
        let l = vec![test_entry(RunUser, 1, "a", r"C:\a.exe", ""), test_entry(RunUser, 2, "b", r"C:\b.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut launcher = FakeLauncher { fail: ["a".to_string()].into(), ..Default::default() };
        let report = run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: true, ..Default::default() });
        assert_eq!(launcher.started, vec!["b"]);
        assert_eq!((report.launched(), report.failed()), (1, 1));
    }

    #[test]
    fn a_shell_that_is_not_ready_in_time_is_logged_and_the_run_goes_on() {
        let l = vec![test_entry(RunUser, 1, "a", r"C:\a.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut launcher = FakeLauncher::default();
        let report = run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: false, ..Default::default() });
        assert_eq!(launcher.started, vec!["a"]);
        assert!(report.records.iter().any(|r| r.outcome == Outcome::ShellReady(false)));
        let lines = report.log_lines(&l);
        assert!(lines.iter().any(|x| x == "-: shell not ready in time, going on"), "{lines:?}");
        assert!(lines.iter().any(|x| x == "HKCU Run #1 a: launched"), "{lines:?}");
    }

    #[test]
    fn a_stop_request_ends_the_run_between_steps() {
        let l = vec![test_entry(RunUser, 1, "a", r"C:\a.exe", ""), test_entry(RunUser, 2, "b", r"C:\b.exe", ""), test_entry(RunUser, 3, "c", r"C:\c.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        // the steps: wait for the shell, settle, a, wait, b, wait, c: stop after the fourth
        let mut pacer = FakePacer { ready: true, stop_after: Some(4), ..Default::default() };
        let mut launcher = FakeLauncher::default();
        let report = run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut pacer);
        assert_eq!(launcher.started, vec!["a"]);
        assert!(report.stopped_early);
    }

    #[test]
    fn a_stop_request_between_the_delete_of_a_runonce_value_and_its_launch_does_not_lose_the_job() {
        // the steps: wait for the shell, settle, delete, launch. The fake pacer says stop from its fourth question on, which is the
        // one asked just before the launch.
        let l = vec![test_entry(RunOnceUser, 1, "once", r"C:\o.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut pacer = FakePacer { ready: true, stop_after: Some(3), ..Default::default() };
        let (mut launcher, mut store) = (FakeLauncher::default(), FakeStore::default());
        run_all(&l, &plan, &mut launcher, &mut store, &mut pacer);
        assert_eq!(store.deleted, vec![(RunOnceUser, "once".to_string())]);
        assert_eq!(launcher.started, vec!["once"], "a value that was deleted is started: it will not be there at the next sign-in");
    }

    #[test]
    fn a_stop_request_between_a_launch_that_ended_and_its_delete_after_still_deletes() {
        // wait for the shell, settle, launch, delete-after: stop from the fourth question on, which is the one before the delete
        let l = vec![test_entry(RunOnceUser, 1, "!job", r"C:\j.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut pacer = FakePacer { ready: true, stop_after: Some(3), ..Default::default() };
        let (mut launcher, mut store) = (FakeLauncher { exits: true, ..Default::default() }, FakeStore::default());
        run_all(&l, &plan, &mut launcher, &mut store, &mut pacer);
        assert_eq!(launcher.started, vec!["!job"]);
        assert_eq!(store.deleted, vec![(RunOnceUser, "!job".to_string())], "the job ran to its end, so its value goes");
    }

    #[test]
    fn a_stop_request_after_a_pair_ends_the_run() {
        let l = vec![test_entry(RunOnceUser, 1, "once", r"C:\o.exe", ""), test_entry(RunUser, 1, "b", r"C:\b.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        // steps: wait shell, settle, delete, launch once, (wait 500), launch b ... stop from the fourth question on
        let mut pacer = FakePacer { ready: true, stop_after: Some(3), ..Default::default() };
        let mut launcher = FakeLauncher::default();
        let report = run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut pacer);
        assert_eq!(launcher.started, vec!["once"], "the pair is finished, nothing after it starts");
        assert!(report.stopped_early);
    }

    #[test]
    fn a_wait_for_a_bang_job_ends_when_the_host_asks_to_stop_and_the_value_stays() {
        let l = vec![test_entry(RunOnceUser, 1, "!hung", r"C:\h.exe", ""), test_entry(RunOnceUser, 2, "!hung2", r"C:\h2.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        // steps: wait shell, settle, launch hung (asks stop while waiting: the 4th question), ...
        let mut pacer = FakePacer { ready: true, stop_after: Some(3), ..Default::default() };
        let mut launcher = FakeLauncher { waits_until_stopped: ["!hung".to_string(), "!hung2".to_string()].into(), ..Default::default() };
        let mut store = FakeStore::default();
        let report = run_all(&l, &plan, &mut launcher, &mut store, &mut pacer);
        assert!(report.records.iter().any(|r| r.outcome == Outcome::LaunchInterrupted));
        assert!(store.deleted.is_empty(), "a job that did not end keeps its value");
        assert_eq!(launcher.started, vec!["!hung"], "and the next job does not start");
        assert!(report.stopped_early);
        assert_eq!(report.failed(), 0, "a stop is not a failure");
        assert!(report.log_lines(&l).iter().any(|x| x.ends_with("launched, the wait for its end was stopped")));
    }

    #[test]
    fn an_entry_that_runs_by_the_time_its_turn_comes_is_skipped_and_one_that_was_known_to_run_is_not_asked_about() {
        let l = vec![test_entry(RunUser, 1, "late", r"C:\late.exe", ""), test_entry(RunUser, 2, "calm", r"C:\calm.exe", ""), test_entry(RunOnceUser, 1, "job", r"C:\job.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut launcher = FakeLauncher { running_now: ["late".to_string(), "job".to_string()].into(), ..Default::default() };
        let report = run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: true, ..Default::default() });
        assert_eq!(launcher.rechecked, vec!["late", "calm"], "a RunOnce job is not a program that should stay up: not asked");
        assert!(report.records.iter().any(|r| r.entry == Some(0) && r.outcome == Outcome::Skipped(SkipReason::AlreadyRunning)));
        assert_eq!(launcher.started, vec!["calm", "job"]);
        // with skip_running off, nothing is asked
        let plan = build(&l, &PlanOptions { skip_running: false, ..Default::default() });
        let mut launcher = FakeLauncher { running_now: ["late".to_string()].into(), ..Default::default() };
        run_all(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: true, ..Default::default() });
        assert!(launcher.rechecked.is_empty());
        assert_eq!(launcher.started.len(), 3);
    }

    #[derive(Default)]
    struct FakeMarker {
        ran: bool,
        marks: usize,
    }

    impl SessionMarker for FakeMarker {
        fn already_ran(&self) -> bool {
            self.ran
        }
        fn mark_ran(&mut self) {
            self.ran = true;
            self.marks += 1;
        }
    }

    #[test]
    fn a_second_run_in_the_same_sign_in_starts_nothing() {
        let l = vec![test_entry(RunUser, 1, "a", r"C:\a.exe", ""), test_entry(RunOnceUser, 1, "once", r"C:\o.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut marker = FakeMarker::default();
        let mut launcher = FakeLauncher::default();
        let mut store = FakeStore::default();
        let first = run_session(&l, &plan, &mut launcher, &mut store, &mut FakePacer { ready: true, ..Default::default() }, &mut marker);
        assert_eq!(first.unwrap().launched(), 2);
        assert_eq!(marker.marks, 1);
        // the host crashed and was restarted: the program the user closed is NOT started again
        let again = run_session(&l, &plan, &mut launcher, &mut store, &mut FakePacer { ready: true, ..Default::default() }, &mut marker);
        assert!(again.is_none());
        assert_eq!(launcher.started, vec!["a", "once"]);
        assert_eq!(store.deleted.len(), 1, "and nothing is deleted a second time");
    }

    #[test]
    fn a_run_that_was_stopped_is_not_marked_as_done() {
        let l = vec![test_entry(RunUser, 1, "a", r"C:\a.exe", ""), test_entry(RunUser, 2, "b", r"C:\b.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let mut marker = FakeMarker::default();
        let mut launcher = FakeLauncher::default();
        let stopped = run_session(&l, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: true, stop_after: Some(3), ..Default::default() }, &mut marker);
        assert!(stopped.unwrap().stopped_early);
        assert!(!marker.ran);
        // the next run starts the rest; the first entry, now running, is left alone by the plan's own check
        let mut again = l.clone();
        again[0].running = Running::Yes;
        let plan = build(&again, &PlanOptions::default());
        let mut launcher = FakeLauncher::default();
        let report = run_session(&again, &plan, &mut launcher, &mut FakeStore::default(), &mut FakePacer { ready: true, ..Default::default() }, &mut marker).unwrap();
        assert_eq!(launcher.started, vec!["b"]);
        assert!(!report.stopped_early && marker.ran);
    }

    #[test]
    fn log_lines_hold_one_line_per_record_whatever_a_value_name_holds() {
        let l = vec![test_entry(RunUser, 1, "bad\nname\u{7}", r"C:\a.exe", "")];
        let plan = build(&l, &PlanOptions::default());
        let report = run_all(&l, &plan, &mut FakeLauncher::default(), &mut FakeStore::default(), &mut FakePacer { ready: true, ..Default::default() });
        let lines = report.log_lines(&l);
        assert!(lines.iter().all(|x| !x.contains(['\n', '\u{7}', '\0'])), "{lines:?}");
        assert!(lines.iter().any(|x| x == "HKCU Run #1 bad?name?: launched"), "{lines:?}");
    }

    #[test]
    fn a_plan_with_an_index_outside_the_list_is_survived() {
        let plan = Plan {
            steps: vec![
                Step::Launch { entry: 7, critical: false, wait_exit_ms: None, recheck_running: false },
                Step::DeleteRunOnce { entry: 7, when: DeleteWhen::BeforeLaunch },
                Step::Skip { entry: 7, reason: SkipReason::Disabled },
            ],
        };
        let report = run_all(&[], &plan, &mut FakeLauncher::default(), &mut FakeStore::default(), &mut FakePacer::default());
        assert_eq!(report.records.iter().filter(|r| r.outcome == Outcome::BadIndex).count(), 2);
        assert!(report.log_lines(&[]).iter().all(|l| l.starts_with("-: ")));
    }

    #[test]
    fn nothing_that_is_skipped_reaches_the_launcher_or_the_store() {
        let mut l = list();
        for e in &mut l {
            e.presence = Presence::Missing;
            e.running = Running::Yes;
        }
        let plan = build(&l, &PlanOptions::default());
        let (mut launcher, mut store) = (FakeLauncher::default(), FakeStore::default());
        let report = run_all(&l, &plan, &mut launcher, &mut store, &mut FakePacer::default());
        assert!(launcher.started.is_empty() && launcher.asked_to_wait.is_empty());
        assert!(store.deleted.is_empty());
        assert_eq!(report.launched(), 0);
    }
}
