//! What Windows starts at sign-in, as a list, and the plan of a run (shell plan increment H1).
//!
//! When rustyfiles is the shell, nobody else starts the user's start-up programs: Explorer did, and it is not running. This module
//! is the read-only half of that job plus a plan of what a run would do; the run itself exists only over traits and is covered by
//! fakes. NOTHING HERE STARTS A PROGRAM OR DELETES A VALUE BY ITSELF: `list()`, `plan::build` and the formatters are pure reads.
//!
//! | file | what it is | tested with |
//! |---|---|---|
//! | `model` | sources and their order, the entry, the counts | plain data |
//! | `command` | cutting a Run value into program and arguments as `CreateProcess` does; `%VARIABLES%` | fake disk and environment |
//! | `approved` | the `StartupApproved` binary value | the forms seen on a real machine, and unknown bytes |
//! | `runonce` | the `!` and `*` prefixes of a RunOnce name | strings |
//! | `collect` | the list, built through traits (registry, folders, shortcuts, processes, disk, environment) | fakes of all six |
//! | `plan` | order, stagger, what is skipped and why, what RunOnce deletes and when; the printer | lists made by hand |
//! | `run` | `run_all` over `Launcher`, `RunOnceStore` and `Pacer`; `run_session` adds the once-per-sign-in memory | fakes of all of them |
//! | `gate` | the token without which the real launcher and the real RunOnce delete cannot be built (no Explorer running) | lists made by hand |
//! | `system` | the real machine behind the traits | read-only parts on this machine; the acting parts (launch, delete, wait, the session file) are compiled and never run against the real machine |
//!
//! What was verified and what was not, the list of things only a second account or a VM can settle, and the packaged-app note are in
//! `docs/shell-startup.md`.
//!
//! # Where the entries come from (research note 1, section 5.1; order = the order of `Source`)
//!
//! 1. `Policies\Explorer\Run` (HKLM, HKLM 32-bit view, HKCU);
//! 2. `Run` (HKLM, HKLM 32-bit view, HKCU), with the user's on/off choice in `Explorer\StartupApproved\{Run, Run32}`;
//! 3. `RunOnce` (HKLM, HKLM 32-bit view, HKCU);
//! 4. the user's Startup folder, then the common one, with `StartupApproved\StartupFolder`.
//!
//! HKCU is read in the native view only (its `Run` key is not redirected, so a 32-bit view would list everything twice). The 32-bit
//! view of `Policies\Explorer\Run` (`PolicyRunMachine32`) is NOT named in the research notes; it is listed because the machine-wide
//! `Run` key has a 32-bit view and the policy key lives beside it. It is empty on the development machine.
//!
//! # What is NOT listed, and why
//!
//! * **Packaged apps' start-up tasks** (the "Startup" entries of Store apps). The research marks no documented, read-only way to
//!   list them: Explorer uses an internal WinRT class (`StartupTaskInternal`), the state lives under
//!   `...\AppModel\SystemAppData\<package family>\<task id>` in a layout that was only observed (`State` 2 = enabled, 1 = disabled by
//!   the user; the documented `StartupTaskState` enum is from memory), and what to start comes from each package's manifest. Listing
//!   them from those would be a guess about an undocumented layout. They are left out, and a run cannot start them either; the
//!   docs say so (`docs/shell-startup.md`).
//! * **`RunOnceEx`** (a legacy key of sub-keys with DLL and dependency values, not a list of command lines).
//! * Group Policy start-up scripts, logon scripts (`Userinit` ran those before the shell), and anything the Task Scheduler starts.
//!
//! # Not verified (the notes' own marks, unchanged)
//!
//! * The default delay Explorer applies (`Explorer\Serialize\StartupDelayInMSec`, absent on the development machine): the plan's
//!   `settle_ms` and `stagger_ms` are our own numbers, set in `PlanOptions`.
//! * Whether a REG_SZ (not REG_EXPAND_SZ) value with `%VARIABLES%` is expanded by Explorer: it is not here, and such an entry's
//!   program is "existence unknown", not "missing".
//! * The working folder Explorer gives a Run command: the launcher passes none (a shortcut keeps its own).
//! * WOW64 file-system redirection is not reproduced: a 32-bit entry that names `System32` is checked in the real `System32`.
//! * Whether the Startup folders and the policy keys are skipped in Safe Mode as Run is (documented for Run only); the plan skips
//!   them too, and the shell guard does not start the shell in Safe Mode at all.

pub mod approved;
pub mod collect;
pub mod command;
pub mod gate;
pub mod model;
pub mod plan;
pub mod run;
pub mod runonce;
pub mod system;

pub use model::{Approval, Counts, Presence, Running, ShortcutState, Source, SourceKind, StartupEntry, counts_by_source};
pub use plan::{Names, Plan, PlanOptions, PlanSummary, SkipReason, Step, format_plan, format_summary};
pub use gate::{Refusal, ShellModeToken};
pub use run::{Launcher, Pacer, RunOnceStore, RunReport, SessionMarker, run_all, run_session};
pub use system::{FileSessionMarker, ShellLauncher, RegistryRunOnce, ThreadPacer, is_safe_mode, list, logon_session_key, session_options};
