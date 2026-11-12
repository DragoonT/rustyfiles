//! The shell host's own facts (shell plan increment A1): in which mode the host runs, and a report of what it found. EVERYTHING HERE IS
//! READ-ONLY: windows are looked at, never messaged; processes are asked their names; the registry is opened for reading only.
//!
//! | file | what it is | tested with |
//! |---|---|---|
//! | `mode` | [`mode::ShellMode`] (Beside Explorer, or the shell), the pure decision, and the read-only check that feeds it | plain data |
//! | `probe` | the report `rustyfiles --shell-probe` prints | in-memory registry, plain data |

pub mod mode;
pub mod probe;

pub use mode::ShellMode;
