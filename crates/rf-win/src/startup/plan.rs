//! The plan of a start-up run: a pure function from the list to a sequence of steps, and a formatter that prints it.
//!
//! What the plan does, in order:
//!
//! 1. **Shell-critical entries first.** The plan documents no list beyond examples ("TextInputHost / ctfmon / etc."), so the list is
//!    ours and short: `ctfmon.exe` (the input stack; userinit starts it only on a path we are not on). Anything the caller adds to
//!    `PlanOptions::critical_programs` goes first, in the list's own order. They start at once, a short gap apart.
//! 2. **Then wait for the shell.** One `WaitShellReady` step with a limit: run_all asks its `Pacer`; the host answers it when the
//!    tray and the dock exist and the packaged shell hosts (`sihost`, `ShellHost`) are up (the KB5072911 race: research note 1, trap 6).
//!    A wait that runs out is logged and the run goes on; nothing waits for ever.
//! 3. **Then everything else in source order**, `settle_ms` after the shell is ready and `stagger_ms` apart.
//!
//! What is skipped, and why (the first reason that applies wins, in this order):
//! Safe Mode (only RunOnce values named with `*` run there), HKLM RunOnce without an elevated shell (Windows runs those only for
//! administrators and the value could not be deleted; the shell never runs elevated), disabled by the user, a StartupApproved value of
//! unknown layout (the policy is in `PlanOptions::unknown_approval`; the default skips and says so), a shortcut that cannot be read,
//! no program in the command, the program is missing, a registry command that still holds a `%VARIABLE%` (a REG_SZ value: whether
//! Explorer expands it is not documented, and a command cut at the wrong space would start another program than Windows does), a
//! shortcut that asks to run as administrator (the shell never raises a UAC prompt), the program already runs (Run, policy and folder
//! entries only; a RunOnce entry is a job, not a program that should stay up; a process of the same file name whose path cannot be read
//! counts too), and a second entry with the same program and arguments as one that is already going to start.
//!
//! RunOnce: by default the value is deleted BEFORE the command starts (a failure is not retried); with `!` it is deleted AFTER the
//! command ran, which needs the launch to wait for the process. A value that is skipped is never deleted: we delete only what we
//! started. A RunOnce entry is never skipped as a duplicate of a Run entry: it is a job with a value to delete, and the program it
//! shares with a Run entry is run for that job all the same.
//!
//! Order: the `!` jobs wait for their process (up to `runonce_wait_ms` each), so they go after every other ordinary entry; a hung
//! installer then delays nothing the user sees. This is the one place where the plan departs from the source order.
//!
//! `recheck_running`: the list is read once, and launches happen seconds later. The launcher is asked again just before it starts an
//! entry that was "not running" at that time (an updater may have started the program in the meantime).

use super::command::{file_name_lower, path_key};
use super::model::{Approval, Presence, Running, ShortcutState, Source, SourceKind, StartupEntry};

/// What to do with a StartupApproved value of a layout nobody has documented.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum UnknownApproval {
    /// Do not start the entry: a program the user switched off starting by itself is worse than one that waits for a decision.
    Skip,
    /// Start it as Explorer would if it ignored the value.
    Run,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanOptions {
    /// The session is a Safe Mode session.
    pub safe_mode: bool,
    /// The shell is elevated (it never is; HKLM RunOnce entries are skipped when it is not).
    pub elevated: bool,
    pub unknown_approval: UnknownApproval,
    /// Do not start what already runs.
    pub skip_running: bool,
    /// File names (lower case) of the programs that go first.
    pub critical_programs: Vec<String>,
    /// Between two critical launches.
    pub critical_gap_ms: u32,
    /// How long the run waits for the shell at most.
    pub shell_ready_limit_ms: u32,
    /// After the shell is ready, before the first ordinary entry.
    pub settle_ms: u32,
    /// Between two ordinary launches.
    pub stagger_ms: u32,
    /// How long a RunOnce command named with `!` is waited for at most.
    pub runonce_wait_ms: u32,
}

impl Default for PlanOptions {
    fn default() -> Self {
        PlanOptions {
            safe_mode: false,
            elevated: false,
            unknown_approval: UnknownApproval::Skip,
            skip_running: true,
            critical_programs: vec!["ctfmon.exe".to_string()],
            critical_gap_ms: 100,
            shell_ready_limit_ms: 30_000,
            settle_ms: 2_000,
            stagger_ms: 500,
            runonce_wait_ms: 60_000,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SkipReason {
    SafeMode,
    /// A machine-wide RunOnce entry and the shell is not elevated.
    NeedsAdmin,
    Disabled,
    /// The StartupApproved value has an undocumented layout and the options say skip.
    ApprovalUnknown,
    ShortcutUnreadable,
    NoProgram,
    MissingProgram,
    /// A registry command with a `%VARIABLE%` that was not expanded (a REG_SZ value); see the module comment.
    UnexpandedVariable,
    /// A shortcut with "run as administrator": starting it would show a UAC prompt.
    NeedsElevation,
    AlreadyRunning,
    /// The same program with the same arguments is already going to start: this is the index (into the list) of that entry.
    Duplicate(usize),
}

impl SkipReason {
    /// A stable one-word name, for counts.
    pub fn short(self) -> &'static str {
        match self {
            SkipReason::SafeMode => "safe-mode",
            SkipReason::NeedsAdmin => "needs-admin",
            SkipReason::Disabled => "disabled",
            SkipReason::ApprovalUnknown => "approval-unknown",
            SkipReason::ShortcutUnreadable => "shortcut-unreadable",
            SkipReason::NoProgram => "no-program",
            SkipReason::MissingProgram => "missing",
            SkipReason::UnexpandedVariable => "unexpanded-variable",
            SkipReason::NeedsElevation => "needs-elevation",
            SkipReason::AlreadyRunning => "already-running",
            SkipReason::Duplicate(_) => "duplicate",
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DeleteWhen {
    BeforeLaunch,
    /// After the command ran (`!`); done only when the process was waited for and ended.
    AfterRun,
}

/// One step of a run. `entry` is an index into the list the plan was made from.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Step {
    WaitShellReady { limit_ms: u32 },
    Wait { ms: u32 },
    Skip { entry: usize, reason: SkipReason },
    DeleteRunOnce { entry: usize, when: DeleteWhen },
    /// `wait_exit_ms`: the launcher waits for the process to end, at most that long (RunOnce with `!`).
    /// `recheck_running`: ask the launcher whether the program runs by now, just before starting it.
    Launch { entry: usize, critical: bool, wait_exit_ms: Option<u32>, recheck_running: bool },
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Plan {
    pub steps: Vec<Step>,
}

/// The numbers of a plan.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct PlanSummary {
    pub launches: usize,
    pub critical_launches: usize,
    /// (reason name, count), in the order the names first appear.
    pub skips: Vec<(&'static str, usize)>,
    pub deletes_before: usize,
    pub deletes_after: usize,
    /// The fixed waits added up (not the wait for the shell).
    pub waits_ms: u64,
}

impl Plan {
    pub fn summary(&self) -> PlanSummary {
        let mut s = PlanSummary::default();
        for step in &self.steps {
            match *step {
                Step::Launch { critical, .. } => {
                    s.launches += 1;
                    s.critical_launches += usize::from(critical);
                }
                Step::Skip { reason, .. } => {
                    let name = reason.short();
                    match s.skips.iter_mut().find(|(n, _)| *n == name) {
                        Some((_, c)) => *c += 1,
                        None => s.skips.push((name, 1)),
                    }
                }
                Step::DeleteRunOnce { when: DeleteWhen::BeforeLaunch, .. } => s.deletes_before += 1,
                Step::DeleteRunOnce { when: DeleteWhen::AfterRun, .. } => s.deletes_after += 1,
                Step::Wait { ms } => s.waits_ms += u64::from(ms),
                Step::WaitShellReady { .. } => {}
            }
        }
        s
    }
}

/// A RunOnce entry named with `!`: the launch waits for its process.
fn waits_for_exit(e: &StartupEntry) -> bool {
    e.source.kind() == SourceKind::RunOnce && e.runonce.is_some_and(|f| f.delete_after)
}

fn is_critical(e: &StartupEntry, opts: &PlanOptions) -> bool {
    !e.program.is_empty() && opts.critical_programs.iter().any(|c| *c == file_name_lower(&e.program))
}

/// The first reason this entry is not started, ignoring other entries.
fn skip_reason(e: &StartupEntry, opts: &PlanOptions) -> Option<SkipReason> {
    let kind = e.source.kind();
    if opts.safe_mode && !(kind == SourceKind::RunOnce && e.runonce.is_some_and(|f| f.run_in_safe_mode)) {
        return Some(SkipReason::SafeMode);
    }
    if kind == SourceKind::RunOnce && e.source.is_machine_wide() && !opts.elevated {
        return Some(SkipReason::NeedsAdmin);
    }
    match e.approval {
        Approval::Disabled { .. } => return Some(SkipReason::Disabled),
        Approval::Unknown if opts.unknown_approval == UnknownApproval::Skip => return Some(SkipReason::ApprovalUnknown),
        _ => {}
    }
    if e.shortcut == ShortcutState::Unreadable {
        return Some(SkipReason::ShortcutUnreadable);
    }
    // a registry command must name a program; a shortcut to a non-file target is opened as the shortcut
    if e.file_path.is_none() && e.program.is_empty() {
        return Some(SkipReason::NoProgram);
    }
    if e.presence == Presence::Missing {
        return Some(SkipReason::MissingProgram);
    }
    if e.file_path.is_none() && e.presence == Presence::Unknown {
        // only a registry command gets here with an unknown program: it still holds a %VARIABLE%
        return Some(SkipReason::UnexpandedVariable);
    }
    if e.run_as_admin {
        return Some(SkipReason::NeedsElevation);
    }
    if opts.skip_running && kind != SourceKind::RunOnce && e.probably_running() {
        return Some(SkipReason::AlreadyRunning);
    }
    None
}

/// What "the same command" means: the same program file with the same arguments (case and separators aside).
fn same_command_key(e: &StartupEntry) -> String {
    match (&e.file_path, e.program.is_empty()) {
        (Some(f), true) => format!("file:{}", path_key(f)),
        _ => format!("{}\u{1}{}", path_key(&e.program), e.args.trim().to_lowercase()),
    }
}

/// Makes the plan for `entries` (the list in the order `collect` gave it).
pub fn build(entries: &[StartupEntry], opts: &PlanOptions) -> Plan {
    let critical: Vec<usize> = (0..entries.len()).filter(|&i| is_critical(&entries[i], opts)).collect();
    let mut ordinary: Vec<usize> = (0..entries.len()).filter(|&i| !is_critical(&entries[i], opts)).collect();
    // the jobs that are waited for go after everything else (a stable sort keeps the source order inside both halves)
    ordinary.sort_by_key(|&i| waits_for_exit(&entries[i]));

    // decide every entry once, in the order they will be handled, so that "a duplicate of" always points at an earlier launch
    let mut will_launch: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut decide = |i: usize| -> Option<SkipReason> {
        let e = &entries[i];
        if let Some(r) = skip_reason(e, opts) {
            return Some(r);
        }
        if e.source.kind() == SourceKind::RunOnce {
            // a job with a value to delete: neither a duplicate of a Run entry nor the original of one
            return None;
        }
        let key = same_command_key(e);
        if let Some(&first) = will_launch.get(&key) {
            return Some(SkipReason::Duplicate(first));
        }
        will_launch.insert(key, i);
        None
    };
    let critical_decisions: Vec<(usize, Option<SkipReason>)> = critical.iter().map(|&i| (i, decide(i))).collect();
    let ordinary_decisions: Vec<(usize, Option<SkipReason>)> = ordinary.iter().map(|&i| (i, decide(i))).collect();

    let mut steps = Vec::new();
    emit_group(entries, opts, &critical_decisions, true, opts.critical_gap_ms, &mut steps);
    if ordinary_decisions.iter().any(|(_, d)| d.is_none()) {
        steps.push(Step::WaitShellReady { limit_ms: opts.shell_ready_limit_ms });
        if opts.settle_ms > 0 {
            steps.push(Step::Wait { ms: opts.settle_ms });
        }
    }
    emit_group(entries, opts, &ordinary_decisions, false, opts.stagger_ms, &mut steps);
    Plan { steps }
}

fn emit_group(entries: &[StartupEntry], opts: &PlanOptions, group: &[(usize, Option<SkipReason>)], critical: bool, gap_ms: u32, steps: &mut Vec<Step>) {
    let mut first = true;
    for &(i, decision) in group {
        if let Some(reason) = decision {
            steps.push(Step::Skip { entry: i, reason });
            continue;
        }
        if !first && gap_ms > 0 {
            steps.push(Step::Wait { ms: gap_ms });
        }
        first = false;
        let e = &entries[i];
        let flags = e.runonce;
        let is_runonce = e.source.kind() == SourceKind::RunOnce;
        let delete_after = is_runonce && flags.is_some_and(|f| f.delete_after);
        if is_runonce && !delete_after {
            steps.push(Step::DeleteRunOnce { entry: i, when: DeleteWhen::BeforeLaunch });
        }
        let recheck_running = opts.skip_running && !is_runonce && e.running == Running::No;
        steps.push(Step::Launch { entry: i, critical, wait_exit_ms: delete_after.then_some(opts.runonce_wait_ms), recheck_running });
        if delete_after {
            steps.push(Step::DeleteRunOnce { entry: i, when: DeleteWhen::AfterRun });
        }
    }
}

// ---------------------------------------------------------------------------------------------------- printing

/// Whether the printed plan names the entries.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Names {
    /// "HKCU Run #3 "Discord": ..." (a log for the user's own eyes)
    Show,
    /// "HKCU Run #3: ..." (for a report that must not name the user's programs)
    Hide,
}

/// A text for a line of a log: control characters (a NUL, a line break in a value name) become `?`, so that one entry is one line.
pub(crate) fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { '?' } else { c }).collect()
}

fn describe(e: &StartupEntry, names: Names) -> String {
    match names {
        Names::Show => format!("{} #{} \"{}\"", e.source.label(), e.ordinal, clean(&e.name)),
        Names::Hide => format!("{} #{}", e.source.label(), e.ordinal),
    }
}

fn state_text(e: &StartupEntry) -> String {
    let presence = match e.presence {
        Presence::Found => "exists",
        Presence::Missing => "missing",
        Presence::Unknown => "existence unknown",
    };
    let approval = match e.approval {
        Approval::NotApplicable => "no on/off setting",
        Approval::NoRecord | Approval::Enabled { .. } => "enabled",
        Approval::Disabled { .. } => "disabled",
        Approval::Unknown => "setting unreadable",
    };
    let running = match e.running {
        Running::Yes => "already running",
        Running::Maybe => "a process of that name runs (path unreadable)",
        Running::No => "not running",
        Running::NotChecked => "running not checked",
    };
    format!("{presence}, {approval}, {running}")
}

/// The reason in words.
pub fn reason_text(r: SkipReason, entries: &[StartupEntry], names: Names) -> String {
    match r {
        SkipReason::SafeMode => "Safe Mode".into(),
        SkipReason::NeedsAdmin => "machine-wide RunOnce needs an elevated shell".into(),
        SkipReason::Disabled => "disabled by the user".into(),
        SkipReason::ApprovalUnknown => "on/off value of unknown layout".into(),
        SkipReason::ShortcutUnreadable => "shortcut cannot be read".into(),
        SkipReason::NoProgram => "no program in the command".into(),
        SkipReason::MissingProgram => "program missing".into(),
        SkipReason::UnexpandedVariable => "command holds a %VARIABLE% that is not expanded".into(),
        SkipReason::NeedsElevation => "shortcut asks to run as administrator".into(),
        SkipReason::AlreadyRunning => "already running".into(),
        SkipReason::Duplicate(i) => match entries.get(i) {
            Some(e) => format!("same command as {}", describe(e, names)),
            None => "same command as an earlier entry".into(),
        },
    }
}

/// The plan as text, one line per step.
pub fn format_plan(entries: &[StartupEntry], plan: &Plan, names: Names) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let line = |i: usize| entries.get(i).map(|e| format!("{}: {}", describe(e, names), state_text(e))).unwrap_or_else(|| format!("entry {i}: not in the list"));
    for step in &plan.steps {
        let _ = match *step {
            Step::WaitShellReady { limit_ms } => writeln!(out, "  .. wait for the shell to be ready (at most {limit_ms} ms)"),
            Step::Wait { ms } => writeln!(out, "  .. wait {ms} ms"),
            Step::Skip { entry, reason } => writeln!(out, "  {} -> skip ({})", line(entry), reason_text(reason, entries, names)),
            Step::DeleteRunOnce { entry, when } => {
                let when = match when {
                    DeleteWhen::BeforeLaunch => "before it starts",
                    DeleteWhen::AfterRun => "after it ran",
                };
                writeln!(out, "     delete the RunOnce value of {} {when}", entries.get(entry).map(|e| describe(e, names)).unwrap_or_default())
            }
            Step::Launch { entry, critical, wait_exit_ms, .. } => {
                let mut extra = String::new();
                if critical {
                    extra.push_str(" [first: shell-critical]");
                }
                if let Some(ms) = wait_exit_ms {
                    let _ = write!(extra, " [waits for it to end, at most {ms} ms]");
                }
                writeln!(out, "  {} -> launch{extra}", line(entry))
            }
        };
    }
    out
}

pub fn format_summary(s: &PlanSummary) -> String {
    let skips: Vec<String> = s.skips.iter().map(|(n, c)| format!("{c} {n}")).collect();
    format!(
        "{} to launch ({} shell-critical), skipped: {}; RunOnce values deleted: {} before, {} after; fixed waits {} ms",
        s.launches,
        s.critical_launches,
        if skips.is_empty() { "none".to_string() } else { skips.join(", ") },
        s.deletes_before,
        s.deletes_after,
        s.waits_ms
    )
}

/// Which sources the steps touch, for tests and reports.
pub fn launched_sources(entries: &[StartupEntry], plan: &Plan) -> Vec<Source> {
    plan.steps.iter().filter_map(|s| if let Step::Launch { entry, .. } = s { entries.get(*entry).map(|e| e.source) } else { None }).collect()
}

#[cfg(test)]
pub(crate) fn test_entry(source: Source, ordinal: usize, name: &str, program: &str, args: &str) -> StartupEntry {
    StartupEntry {
        source,
        ordinal,
        name: name.into(),
        command: format!("{program} {args}"),
        expanded: format!("{program} {args}"),
        program: program.into(),
        args: args.into(),
        working_dir: String::new(),
        show: None,
        file_path: (source.kind() == SourceKind::Folder).then(|| format!(r"C:\Startup\{name}")),
        runonce: (source.kind() == SourceKind::RunOnce).then(|| super::runonce::parse_name(name).0),
        shortcut: ShortcutState::NotShortcut,
        run_as_admin: false,
        approval: match source.approval_key() {
            Some(_) => Approval::NoRecord,
            None => Approval::NotApplicable,
        },
        presence: Presence::Found,
        running: Running::No,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Source::*;

    fn e(source: Source, n: usize, name: &str, program: &str) -> StartupEntry {
        test_entry(source, n, name, program, "")
    }

    fn launches(_entries: &[StartupEntry], plan: &Plan) -> Vec<usize> {
        plan.steps.iter().filter_map(|s| if let Step::Launch { entry, .. } = s { Some(*entry) } else { None }).collect()
    }

    fn skips(plan: &Plan) -> Vec<(usize, SkipReason)> {
        plan.steps.iter().filter_map(|s| if let Step::Skip { entry, reason } = s { Some((*entry, *reason)) } else { None }).collect()
    }

    #[test]
    fn launches_follow_the_list_order_with_the_shell_wait_and_the_stagger() {
        let list = vec![e(RunMachine, 1, "a", r"C:\a.exe"), e(RunUser, 1, "b", r"C:\b.exe"), e(StartupFolderUser, 1, "c.exe", r"C:\Startup\c.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(
            plan.steps,
            vec![
                Step::WaitShellReady { limit_ms: 30_000 },
                Step::Wait { ms: 2_000 },
                Step::Launch { entry: 0, critical: false, wait_exit_ms: None, recheck_running: true },
                Step::Wait { ms: 500 },
                Step::Launch { entry: 1, critical: false, wait_exit_ms: None, recheck_running: true },
                Step::Wait { ms: 500 },
                Step::Launch { entry: 2, critical: false, wait_exit_ms: None, recheck_running: true },
            ]
        );
        assert_eq!(launched_sources(&list, &plan), vec![RunMachine, RunUser, StartupFolderUser]);
        let s = plan.summary();
        assert_eq!((s.launches, s.critical_launches, s.waits_ms), (3, 0, 3_000));
    }

    #[test]
    fn critical_entries_go_first_at_once_and_the_rest_waits_for_the_shell() {
        let list = vec![e(RunMachine, 1, "a", r"C:\a.exe"), e(RunUser, 1, "ime", r"C:\WINDOWS\system32\CTFMON.EXE"), e(RunUser, 2, "b", r"C:\b.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(plan.steps[0], Step::Launch { entry: 1, critical: true, wait_exit_ms: None, recheck_running: true });
        assert_eq!(plan.steps[1], Step::WaitShellReady { limit_ms: 30_000 });
        assert_eq!(launches(&list, &plan), vec![1, 0, 2]);
        assert_eq!(plan.summary().critical_launches, 1);
        // two critical ones are a short gap apart
        let list = vec![e(RunUser, 1, "x", r"C:\ctfmon.exe"), e(RunUser, 2, "y", r"C:\other\ctfmon.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(
            plan.steps,
            vec![
                Step::Launch { entry: 0, critical: true, wait_exit_ms: None, recheck_running: true },
                Step::Wait { ms: 100 },
                Step::Launch { entry: 1, critical: true, wait_exit_ms: None, recheck_running: true },
            ],
            "two different files with the same name are two programs; only critical ones: no wait for the shell"
        );
    }

    #[test]
    fn the_critical_list_is_the_callers() {
        let list = vec![e(RunUser, 1, "a", r"C:\a.exe"), e(RunUser, 2, "b", r"C:\b.exe")];
        let opts = PlanOptions { critical_programs: vec!["b.exe".into()], ..Default::default() };
        assert_eq!(launches(&list, &build(&list, &opts)), vec![1, 0]);
        let none = PlanOptions { critical_programs: vec![], ..Default::default() };
        assert_eq!(launches(&list, &build(&list, &none)), vec![0, 1]);
    }

    #[test]
    fn without_ordinary_launches_the_run_does_not_wait_for_the_shell() {
        let mut off = e(RunUser, 1, "a", r"C:\a.exe");
        off.approval = Approval::Disabled { since: Some(1) };
        let plan = build(&[off], &PlanOptions::default());
        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(plan.steps[0], Step::Skip { reason: SkipReason::Disabled, .. }));
        assert!(build(&[], &PlanOptions::default()).steps.is_empty());
    }

    #[test]
    fn disabled_missing_running_and_unreadable_entries_are_skipped_with_their_reasons() {
        let mut disabled = e(RunUser, 1, "d", r"C:\d.exe");
        disabled.approval = Approval::Disabled { since: None };
        let mut missing = e(RunUser, 2, "m", r"C:\m.exe");
        missing.presence = Presence::Missing;
        let mut running = e(RunUser, 3, "r", r"C:\r.exe");
        running.running = Running::Yes;
        let mut broken = e(StartupFolderUser, 1, "b.lnk", "");
        broken.shortcut = ShortcutState::Unreadable;
        broken.presence = Presence::Unknown;
        let mut noprog = e(RunUser, 4, "n", "");
        noprog.presence = Presence::Missing;
        let ok = e(RunUser, 5, "ok", r"C:\ok.exe");
        let list = vec![disabled, missing, running, broken, noprog, ok];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(
            skips(&plan),
            vec![
                (0, SkipReason::Disabled),
                (1, SkipReason::MissingProgram),
                (2, SkipReason::AlreadyRunning),
                (3, SkipReason::ShortcutUnreadable),
                (4, SkipReason::NoProgram),
            ]
        );
        assert_eq!(launches(&list, &plan), vec![5]);
        let s = plan.summary();
        assert_eq!(s.skips.len(), 5);
        assert!(format_summary(&s).starts_with("1 to launch (0 shell-critical), skipped: 1 disabled, 1 missing"));
    }

    #[test]
    fn a_disabled_entry_that_is_also_missing_says_disabled() {
        let mut x = e(RunUser, 1, "d", r"C:\d.exe");
        x.approval = Approval::Disabled { since: None };
        x.presence = Presence::Missing;
        assert_eq!(skips(&build(&[x], &PlanOptions::default())), vec![(0, SkipReason::Disabled)]);
    }

    #[test]
    fn an_unknown_approval_value_follows_the_option_and_is_never_read_as_an_answer() {
        let mut x = e(RunUser, 1, "o", r"C:\o.exe");
        x.approval = Approval::Unknown;
        let list = vec![x];
        assert_eq!(skips(&build(&list, &PlanOptions::default())), vec![(0, SkipReason::ApprovalUnknown)]);
        let run = PlanOptions { unknown_approval: UnknownApproval::Run, ..Default::default() };
        assert_eq!(launches(&list, &build(&list, &run)), vec![0]);
    }

    #[test]
    fn existence_unknown_still_launches_and_running_can_be_switched_off() {
        let mut x = e(StartupFolderUser, 1, "uwp.lnk", "");
        x.presence = Presence::Unknown;
        x.shortcut = ShortcutState::Resolved;
        let mut r = e(RunUser, 1, "r", r"C:\r.exe");
        r.running = Running::Yes;
        let list = vec![x, r];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![0], "a shortcut to a non-file target is opened as the shortcut");
        let all = PlanOptions { skip_running: false, ..Default::default() };
        assert_eq!(launches(&list, &build(&list, &all)), vec![0, 1]);
    }

    #[test]
    fn the_same_program_in_two_sources_starts_once() {
        let mut a = test_entry(RunMachine, 1, "a", r"C:\Apps\Discord\Update.exe", "--processStart Discord.exe");
        a.expanded = a.command.clone();
        let b = test_entry(RunUser, 1, "b", r"c:/apps/discord/UPDATE.EXE", "--processstart discord.exe");
        let c = test_entry(RunUser, 2, "c", r"C:\Apps\Discord\Update.exe", "--other");
        let list = vec![a, b, c];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![0, 2], "other arguments are another command");
        assert_eq!(skips(&plan), vec![(1, SkipReason::Duplicate(0))]);
        let text = format_plan(&list, &plan, Names::Hide);
        assert!(text.contains("HKCU Run #1: exists, enabled, not running -> skip (same command as HKLM Run #1)"), "{text}");
    }

    #[test]
    fn a_duplicate_of_a_skipped_entry_is_not_skipped() {
        let mut off = e(RunMachine, 1, "a", r"C:\a.exe");
        off.approval = Approval::Disabled { since: None };
        let on = e(RunUser, 1, "a", r"C:\a.exe");
        let list = vec![off, on];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![1]);
        assert_eq!(skips(&plan), vec![(0, SkipReason::Disabled)]);
    }

    #[test]
    fn duplicates_of_a_critical_entry_point_at_it_even_though_it_moved() {
        let list = vec![e(RunMachine, 1, "a", r"C:\x\ctfmon.exe"), e(RunUser, 1, "b", r"C:\x\ctfmon.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![0]);
        assert_eq!(skips(&plan), vec![(1, SkipReason::Duplicate(0))]);
    }

    #[test]
    fn folder_entries_with_no_program_are_compared_by_their_file() {
        let mut a = e(StartupFolderUser, 1, "a.lnk", "");
        a.shortcut = ShortcutState::Resolved;
        a.presence = Presence::Unknown;
        let mut b = a.clone();
        b.ordinal = 2;
        b.name = "b.lnk".into();
        b.file_path = Some(r"C:\Startup\b.lnk".into());
        let list = vec![a, b];
        assert_eq!(launches(&list, &build(&list, &PlanOptions::default())), vec![0, 1]);
    }

    #[test]
    fn runonce_default_deletes_before_the_launch() {
        let list = vec![e(RunOnceUser, 1, "job", r"C:\job.exe")];
        let plan = build(&list, &PlanOptions::default());
        let rel: Vec<Step> = plan.steps[2..].to_vec();
        assert_eq!(
            rel,
            vec![Step::DeleteRunOnce { entry: 0, when: DeleteWhen::BeforeLaunch }, Step::Launch { entry: 0, critical: false, wait_exit_ms: None, recheck_running: false }]
        );
        let s = plan.summary();
        assert_eq!((s.deletes_before, s.deletes_after), (1, 0));
    }

    #[test]
    fn runonce_with_bang_waits_for_the_command_and_deletes_after() {
        let list = vec![e(RunOnceUser, 1, "!job", r"C:\job.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(
            plan.steps[2..].to_vec(),
            vec![Step::Launch { entry: 0, critical: false, wait_exit_ms: Some(60_000), recheck_running: false }, Step::DeleteRunOnce { entry: 0, when: DeleteWhen::AfterRun }]
        );
        let s = plan.summary();
        assert_eq!((s.deletes_before, s.deletes_after), (0, 1));
    }

    #[test]
    fn machine_runonce_needs_an_elevated_shell() {
        let list = vec![e(RunOnceMachine, 1, "msedge_cleanup", r"C:\x.exe"), e(RunOnceMachine32, 1, "other", r"C:\y.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(skips(&plan), vec![(0, SkipReason::NeedsAdmin), (1, SkipReason::NeedsAdmin)]);
        assert!(!plan.steps.iter().any(|s| matches!(s, Step::DeleteRunOnce { .. })), "a value that is not started is not deleted");
        let admin = PlanOptions { elevated: true, ..Default::default() };
        assert_eq!(launches(&list, &build(&list, &admin)), vec![0, 1]);
    }

    #[test]
    fn a_runonce_value_that_is_skipped_is_not_deleted() {
        let mut gone = e(RunOnceUser, 1, "job", r"C:\gone.exe");
        gone.presence = Presence::Missing;
        let plan = build(&[gone], &PlanOptions::default());
        assert_eq!(plan.steps, vec![Step::Skip { entry: 0, reason: SkipReason::MissingProgram }]);
    }

    #[test]
    fn a_runonce_command_is_started_even_if_its_program_runs() {
        let mut x = e(RunOnceUser, 1, "job", r"C:\job.exe");
        x.running = Running::Yes;
        let list = vec![x];
        assert_eq!(launches(&list, &build(&list, &PlanOptions::default())), vec![0]);
    }

    #[test]
    fn safe_mode_runs_only_runonce_values_marked_with_a_star() {
        let list = vec![
            e(RunUser, 1, "a", r"C:\a.exe"),
            e(StartupFolderUser, 1, "f.exe", r"C:\Startup\f.exe"),
            e(RunOnceUser, 1, "plain", r"C:\p.exe"),
            e(RunOnceUser, 2, "*star", r"C:\s.exe"),
            e(RunOnceUser, 3, "*!both", r"C:\b.exe"),
        ];
        let opts = PlanOptions { safe_mode: true, ..Default::default() };
        let plan = build(&list, &opts);
        assert_eq!(launches(&list, &plan), vec![3, 4]);
        assert_eq!(skips(&plan), vec![(0, SkipReason::SafeMode), (1, SkipReason::SafeMode), (2, SkipReason::SafeMode)]);
    }

    #[test]
    fn a_hostile_list_makes_a_plan_without_panicking() {
        let mut list = Vec::new();
        for i in 0..2_000 {
            let mut x = e(RunUser, i + 1, &"n".repeat(i % 50), &format!(r"C:\{}.exe", i % 7));
            x.args = "a\0b".repeat(i % 5);
            list.push(x);
        }
        let plan = build(&list, &PlanOptions::default());
        let s = plan.summary();
        assert_eq!(s.launches + s.skips.iter().map(|(_, c)| c).sum::<usize>(), 2_000, "every entry is either launched or skipped");
        assert!(s.launches <= 7 * 5);
        let text = format_plan(&list, &plan, Names::Hide);
        assert!(!text.contains('\0'));
        // Names::Show prints the value names, which are the hostile part: one entry stays one line whatever a name holds
        list[0].name = "a\0b\nfake line -> launch\r\u{7}".into();
        let shown = format_plan(&list, &build(&list, &PlanOptions::default()), Names::Show);
        assert!(!shown.contains(['\0', '\u{7}', '\r']), "control characters in the printed plan");
        let first = shown.lines().find(|l| l.contains("HKCU Run #1 \"")).unwrap();
        assert!(first.contains("\"a?b?fake line -> launch??\""), "{first}");
        assert_eq!(shown.lines().count(), format_plan(&list, &build(&list, &PlanOptions::default()), Names::Hide).lines().count());
    }

    #[test]
    fn a_shortcut_that_asks_for_administrator_is_skipped_and_never_prompts() {
        let mut admin = e(StartupFolderUser, 1, "a.lnk", r"C:\a.exe");
        admin.run_as_admin = true;
        admin.shortcut = ShortcutState::Resolved;
        let plain = e(StartupFolderUser, 2, "b.lnk", r"C:\b.exe");
        let list = vec![admin, plain];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(skips(&plan), vec![(0, SkipReason::NeedsElevation)]);
        assert_eq!(launches(&list, &plan), vec![1]);
        assert!(format_plan(&list, &plan, Names::Hide).contains("skip (shortcut asks to run as administrator)"));
        // it is not a way round the other reasons: a missing one is reported as missing
        let mut gone = e(StartupFolderUser, 3, "c.lnk", r"C:\c.exe");
        gone.run_as_admin = true;
        gone.presence = Presence::Missing;
        assert_eq!(skips(&build(&[gone], &PlanOptions::default())), vec![(0, SkipReason::MissingProgram)]);
    }

    #[test]
    fn a_registry_command_with_an_unexpanded_variable_is_not_started_but_a_shortcut_to_a_non_file_is() {
        let mut var = e(RunUser, 1, "v", r"%ProgramFiles%\My");
        var.args = r"App\app.exe -x".into();
        var.presence = Presence::Unknown;
        let mut uwp = e(StartupFolderUser, 1, "uwp.lnk", "");
        uwp.program = String::new();
        uwp.presence = Presence::Unknown;
        uwp.shortcut = ShortcutState::Resolved;
        let list = vec![var, uwp];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(skips(&plan), vec![(0, SkipReason::UnexpandedVariable)]);
        assert_eq!(launches(&list, &plan), vec![1]);
    }

    #[test]
    fn a_process_of_the_same_name_with_an_unreadable_path_counts_as_running() {
        let mut m = e(RunUser, 1, "elevated", r"C:\a.exe");
        m.running = Running::Maybe;
        let list = vec![m];
        assert_eq!(skips(&build(&list, &PlanOptions::default())), vec![(0, SkipReason::AlreadyRunning)]);
        assert!(format_plan(&list, &build(&list, &PlanOptions::default()), Names::Hide).contains("a process of that name runs (path unreadable)"));
        // and not when the options say to start them all
        assert_eq!(launches(&list, &build(&list, &PlanOptions { skip_running: false, ..Default::default() })), vec![0]);
    }

    #[test]
    fn only_a_program_that_was_known_not_to_run_is_checked_again_before_its_launch() {
        let mut unknown = e(RunUser, 2, "host", r"C:\WINDOWS\system32\rundll32.exe");
        unknown.running = Running::NotChecked;
        let list = vec![e(RunUser, 1, "a", r"C:\a.exe"), unknown, e(RunOnceUser, 1, "job", r"C:\j.exe")];
        let flags = |opts: &PlanOptions| -> Vec<(usize, bool)> {
            build(&list, opts).steps.iter().filter_map(|s| if let Step::Launch { entry, recheck_running, .. } = s { Some((*entry, *recheck_running)) } else { None }).collect()
        };
        assert_eq!(flags(&PlanOptions::default()), vec![(0, true), (1, false), (2, false)]);
        assert_eq!(flags(&PlanOptions { skip_running: false, ..Default::default() }), vec![(0, false), (1, false), (2, false)]);
    }

    #[test]
    fn jobs_that_are_waited_for_go_after_the_ordinary_entries_and_keep_their_order_among_themselves() {
        let list = vec![
            e(RunOnceUser, 1, "!first", r"C:\1.exe"),
            e(RunUser, 1, "a", r"C:\a.exe"),
            e(RunOnceUser, 2, "once", r"C:\o.exe"),
            e(RunOnceUser, 3, "!second", r"C:\2.exe"),
            e(StartupFolderUser, 1, "f.exe", r"C:\Startup\f.exe"),
        ];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![1, 2, 4, 0, 3], "bang jobs last, the rest in source order");
    }

    #[test]
    fn a_runonce_job_is_never_a_duplicate_of_a_run_entry_and_so_is_deleted_and_started() {
        let list = vec![e(RunUser, 1, "same", r"C:\same.exe"), e(RunOnceUser, 1, "job", r"C:\same.exe"), e(RunOnceUser, 2, "job2", r"C:\same.exe")];
        let plan = build(&list, &PlanOptions::default());
        assert_eq!(launches(&list, &plan), vec![0, 1, 2]);
        assert!(skips(&plan).is_empty());
        assert_eq!(plan.summary().deletes_before, 2, "both values go: nobody else would delete them");
        // two Run entries with the same command are still one launch
        let two = vec![e(RunUser, 1, "x", r"C:\x.exe"), e(StartupFolderUser, 1, "x.exe", r"C:\x.exe")];
        assert_eq!(skips(&build(&two, &PlanOptions::default())), vec![(1, SkipReason::Duplicate(0))]);
    }

    #[test]
    fn the_anonymous_print_names_no_program_and_no_name() {
        let mut secret = test_entry(RunUser, 3, "SecretApp", r"C:\Secret\secret.exe", "--secret-flag");
        secret.running = Running::Yes;
        let list = vec![secret, e(RunOnceUser, 1, "!job", r"C:\job.exe")];
        let plan = build(&list, &PlanOptions::default());
        let hidden = format_plan(&list, &plan, Names::Hide);
        for word in ["Secret", "secret", "job", "C:\\"] {
            assert!(!hidden.contains(word), "{word} in:\n{hidden}");
        }
        assert!(hidden.contains("HKCU Run #3: exists, enabled, already running -> skip (already running)"), "{hidden}");
        assert!(hidden.contains("HKCU RunOnce #1: exists, no on/off setting, not running -> launch [waits for it to end, at most 60000 ms]"), "{hidden}");
        assert!(hidden.contains("delete the RunOnce value of HKCU RunOnce #1 after it ran"), "{hidden}");
        let shown = format_plan(&list, &plan, Names::Show);
        assert!(shown.contains("HKCU Run #3 \"SecretApp\""));
    }

    #[test]
    fn an_index_outside_the_list_prints_without_panicking() {
        let plan = Plan { steps: vec![Step::Skip { entry: 9, reason: SkipReason::Duplicate(8) }, Step::Launch { entry: 9, critical: true, wait_exit_ms: None, recheck_running: false }] };
        let text = format_plan(&[], &plan, Names::Hide);
        assert!(text.contains("entry 9: not in the list"));
    }
}
