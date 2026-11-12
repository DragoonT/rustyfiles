//! The plain data of the start-up list: where an entry comes from, what Windows stored, and what we found out about it.
//! Nothing here touches the system.

/// Registry hive.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

/// Which registry view a key is read through. `Wow32` is `KEY_WOW64_32KEY`: for HKLM `Software` it reaches `Wow6432Node`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum View {
    Native,
    Wow32,
}

/// One registry value, as stored. Only the kinds that matter here are told apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegData {
    /// REG_SZ (cut at the first NUL, as Windows reads it).
    Sz(String),
    /// REG_EXPAND_SZ: the only kind whose `%VARIABLES%` we expand (see `command.rs`).
    ExpandSz(String),
    Binary(Vec<u8>),
    Dword(u32),
    /// Any other kind (its REG_* number).
    Other(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RegValue {
    pub name: String,
    pub data: RegData,
}

/// Where an entry comes from. The derived order IS the order the sources are handled in (research note 1, section 5.1: policy keys,
/// then Run, then RunOnce, then the folders; the user's folder before the common one).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    PolicyRunMachine,
    PolicyRunMachine32,
    PolicyRunUser,
    RunMachine,
    RunMachine32,
    RunUser,
    RunOnceMachine,
    RunOnceMachine32,
    RunOnceUser,
    StartupFolderUser,
    StartupFolderCommon,
}

/// What kind of place a source is.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SourceKind {
    /// A key of command lines that run at every sign-in (Run, Policies\Explorer\Run).
    Run,
    /// A key of command lines that run once and are deleted (RunOnce).
    RunOnce,
    /// A Startup folder.
    Folder,
}

const RUN_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
const RUNONCE_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce";
const POLICY_RUN_PATH: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run";
const APPROVED_BASE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

impl Source {
    pub const ALL: [Source; 11] = [
        Source::PolicyRunMachine,
        Source::PolicyRunMachine32,
        Source::PolicyRunUser,
        Source::RunMachine,
        Source::RunMachine32,
        Source::RunUser,
        Source::RunOnceMachine,
        Source::RunOnceMachine32,
        Source::RunOnceUser,
        Source::StartupFolderUser,
        Source::StartupFolderCommon,
    ];

    /// The short English label used in logs and in the probe ("HKCU Run").
    pub fn label(self) -> &'static str {
        match self {
            Source::PolicyRunMachine => "HKLM Policies Run",
            Source::PolicyRunMachine32 => "HKLM Policies Run (32-bit)",
            Source::PolicyRunUser => "HKCU Policies Run",
            Source::RunMachine => "HKLM Run",
            Source::RunMachine32 => "HKLM Run (32-bit)",
            Source::RunUser => "HKCU Run",
            Source::RunOnceMachine => "HKLM RunOnce",
            Source::RunOnceMachine32 => "HKLM RunOnce (32-bit)",
            Source::RunOnceUser => "HKCU RunOnce",
            Source::StartupFolderUser => "Startup folder (user)",
            Source::StartupFolderCommon => "Startup folder (common)",
        }
    }

    pub fn kind(self) -> SourceKind {
        match self {
            Source::PolicyRunMachine | Source::PolicyRunMachine32 | Source::PolicyRunUser | Source::RunMachine | Source::RunMachine32 | Source::RunUser => {
                SourceKind::Run
            }
            Source::RunOnceMachine | Source::RunOnceMachine32 | Source::RunOnceUser => SourceKind::RunOnce,
            Source::StartupFolderUser | Source::StartupFolderCommon => SourceKind::Folder,
        }
    }

    /// The hive of a source (the folders' StartupApproved values live in the hive matching the folder: user -> HKCU, common -> HKLM).
    pub fn hive(self) -> Hive {
        match self {
            Source::PolicyRunUser | Source::RunUser | Source::RunOnceUser | Source::StartupFolderUser => Hive::CurrentUser,
            _ => Hive::LocalMachine,
        }
    }

    /// Machine-wide sources (HKLM and the common folder).
    pub fn is_machine_wide(self) -> bool {
        self.hive() == Hive::LocalMachine
    }

    /// The key whose values are the entries, with the view to read it through (`None` for the folders).
    ///
    /// HKCU is read in the native view only: `Software\...\Run` under HKCU is not redirected, so the 32-bit view is the same key and
    /// would list every value twice (the probe prints the 32-bit view's count to show it on a machine).
    pub fn registry_key(self) -> Option<(Hive, View, &'static str)> {
        let (view, path) = match self {
            Source::PolicyRunMachine => (View::Native, POLICY_RUN_PATH),
            Source::PolicyRunMachine32 => (View::Wow32, POLICY_RUN_PATH),
            Source::PolicyRunUser => (View::Native, POLICY_RUN_PATH),
            Source::RunMachine | Source::RunUser => (View::Native, RUN_PATH),
            Source::RunMachine32 => (View::Wow32, RUN_PATH),
            Source::RunOnceMachine | Source::RunOnceUser => (View::Native, RUNONCE_PATH),
            Source::RunOnceMachine32 => (View::Wow32, RUNONCE_PATH),
            Source::StartupFolderUser | Source::StartupFolderCommon => return None,
        };
        Some((self.hive(), view, path))
    }

    /// The StartupApproved key that holds the user's enable / disable choice for this source (`None`: the source has no such key; the
    /// policy keys and RunOnce are not listed in Task Manager's Startup tab).
    pub fn approval_key(self) -> Option<(Hive, String)> {
        let sub = match self {
            Source::RunMachine | Source::RunUser => "Run",
            Source::RunMachine32 => "Run32",
            Source::StartupFolderUser | Source::StartupFolderCommon => "StartupFolder",
            _ => return None,
        };
        Some((self.hive(), format!(r"{APPROVED_BASE}\{sub}")))
    }
}

/// The user's choice in StartupApproved, decoded (see `approved.rs`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Approval {
    /// This source has no approval key (policy keys, RunOnce).
    NotApplicable,
    /// The source has one, and it holds no value for this entry: Explorer runs the entry.
    NoRecord,
    /// A value that says enabled. `changed` is its FILETIME when it carries one (re-enabled entries do).
    Enabled { changed: Option<u64> },
    /// A value that says disabled, with the FILETIME of the moment when it holds one.
    Disabled { since: Option<u64> },
    /// A value whose layout the notes do not document. Never read as either answer.
    Unknown,
}

/// What the entry's name told (RunOnce only; see `runonce.rs`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub struct RunOnceFlags {
    /// `!`: the value is deleted after the command ran instead of before.
    pub delete_after: bool,
    /// `*`: runs in Safe Mode too.
    pub run_in_safe_mode: bool,
}

/// Whether the entry's program is on disk.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Presence {
    Found,
    Missing,
    /// Could not be decided (a shortcut to something that is not a file, a REG_SZ value with an unexpanded `%variable%`).
    Unknown,
}

/// Whether a process with the entry's image path runs now.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Running {
    Yes,
    No,
    /// No process with this exact image path was seen, but a process with the same file name runs whose path the user's token cannot
    /// read (an elevated or protected instance). Counted as running by the plan: a second instance is the worse mistake.
    Maybe,
    /// The question has no useful answer: the program is a generic host (rundll32, cmd, an interpreter), is not an `.exe`, or is not
    /// known.
    NotChecked,
}

/// How a Startup folder shortcut fared.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ShortcutState {
    /// The entry is not a `.lnk`.
    NotShortcut,
    Resolved,
    /// The `.lnk` could not be read (damaged, or the shell thread did not answer in time).
    Unreadable,
}

/// One thing Windows would start at sign-in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartupEntry {
    pub source: Source,
    /// 1-based position among the entries of the same source, in the order they were read. Reports name an entry by source and this
    /// number, never by its name.
    pub ordinal: usize,
    /// The value name as stored (RunOnce: with its `!` / `*` prefixes), or the file name in a Startup folder.
    pub name: String,
    /// The command line as stored. For a folder entry: the shortcut's target and arguments, or the file's path.
    pub command: String,
    /// The command after `%VARIABLES%` were expanded (the same text when nothing was expanded).
    pub expanded: String,
    /// The program, as found on disk when it was found, else as written. Empty: the command names no program.
    pub program: String,
    pub args: String,
    /// A shortcut's working folder, "" otherwise.
    pub working_dir: String,
    /// A shortcut's show command (`SW_*`).
    pub show: Option<i32>,
    /// For a folder entry: the full path of the file in the folder (what is opened to launch it).
    pub file_path: Option<String>,
    pub runonce: Option<RunOnceFlags>,
    pub shortcut: ShortcutState,
    /// The shortcut asks to run as administrator (`SLDF_RUNAS_USER`). Starting it would raise a UAC prompt, which the shell never
    /// does; the plan skips such an entry. Only a shortcut carries this flag: a program that needs elevation through its own manifest
    /// is stopped by the launcher (`CreateProcess` fails with `ERROR_ELEVATION_REQUIRED` instead of prompting).
    pub run_as_admin: bool,
    pub approval: Approval,
    pub presence: Presence,
    pub running: Running,
}

impl StartupEntry {
    /// The user did not switch it off in Task Manager / Settings (no record, an enabled record, or a source that has none). An
    /// `Unknown` record is NOT counted as enabled; see [`StartupEntry::approval_unknown`].
    pub fn enabled(&self) -> bool {
        matches!(self.approval, Approval::NotApplicable | Approval::NoRecord | Approval::Enabled { .. })
    }
    pub fn disabled(&self) -> bool {
        matches!(self.approval, Approval::Disabled { .. })
    }
    pub fn approval_unknown(&self) -> bool {
        self.approval == Approval::Unknown
    }
    pub fn missing(&self) -> bool {
        self.presence == Presence::Missing
    }
    pub fn already_running(&self) -> bool {
        self.running == Running::Yes
    }
    /// A process of this program runs, or one with its file name whose path could not be read.
    pub fn probably_running(&self) -> bool {
        matches!(self.running, Running::Yes | Running::Maybe)
    }
}

/// Counts of one source's entries, the numbers to hold against Task Manager's Startup tab.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub enabled: usize,
    pub disabled: usize,
    pub approval_unknown: usize,
    pub missing_program: usize,
    pub already_running: usize,
}

/// Per-source counts, in source order, for every source (also the empty ones).
pub fn counts_by_source(entries: &[StartupEntry]) -> Vec<(Source, Counts)> {
    Source::ALL
        .iter()
        .map(|&s| {
            let mut c = Counts::default();
            for e in entries.iter().filter(|e| e.source == s) {
                c.total += 1;
                c.enabled += usize::from(e.enabled());
                c.disabled += usize::from(e.disabled());
                c.approval_unknown += usize::from(e.approval_unknown());
                c.missing_program += usize::from(e.missing());
                c.already_running += usize::from(e.already_running());
            }
            (s, c)
        })
        .collect()
}
