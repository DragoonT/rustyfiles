//! The shell host's own facts (shell plan increment A1): in which mode the host runs, and a report of what it found. `mode`, `watch` and `probe` are
//! READ-ONLY: windows are looked at, never messaged; processes are asked their names; the registry is opened for reading only. `appcmd`,
//! `shellhook` and `workarea` ACT, and only when nobody owns the shell window (they look again before every action).
//!
//! | file | what it is | tested with |
//! |---|---|---|
//! | `mode` | [`mode::ShellMode`] (Beside Explorer, or the shell), the pure decision, and the read-only check that feeds it | plain data |
//! | `watch` | noticing while running that the shell window came or went (one cheap read per 2 s tick) | fake look |
//! | `appcmd`, `shellhook` | media and volume keys nobody answered, taken only when no shell owns the shell window | fake outputs; the hook is not run |
//! | `minimized` | minimized windows hidden (`ARW_HIDE`) while no shell exists, put back when the host ends; behind two locks | fake metrics and fake look |
//! | `workarea` | the work area set by the host itself, only with no shell at all, behind three locks | fake setter and fake look |
//! | `probe` | the report `rustyfiles --shell-probe` prints | in-memory registry, plain data |

pub mod appcmd;
pub mod minimized;
pub mod mode;
pub mod probe;
pub mod shellhook;
pub mod watch;
pub mod workarea;

pub use mode::ShellMode;
