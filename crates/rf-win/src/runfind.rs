//! Looking for a program by its bare name, the way the Run box would find it, WITHOUT the means to start it.
//!
//! `runcmd` finds a program and also starts it, and the app allows one place only to reach that module (the Enter handler of the
//! address field: a test of the app checks it). The search needs the looking alone (a query that is the name of a program gets a
//! row for it; the row is opened like any file when the user chooses it), so the looking is offered here on its own.

pub use crate::runcmd::{Found, find, is_program_file, search_dirs};
