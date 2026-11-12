//! rf-shellguard: the watchdog that the per-user Winlogon `Shell` value will point at one day.
//!
//! It is the only program that can leave a user without a desktop, so it is small and its rules
//! are data: `machine` is a pure state machine (crash window, backoff, the way back to Explorer),
//! `statefile` is the file it shares with the shell host, `runner` carries the machine's actions
//! out through the traits of `sys`, and `win` holds the real implementations of those traits.
//! Tests and `--dry-run` never go through `win` (except `winro`, which only reads).
//!
//! Hiding Explorer's taskbar beside Explorer (plan E1) lives here too, because the host that hides it and the guard that shows it again
//! must share the rules: `tbmodel` (what is a taskbar window, what may be acted on, the gate, the work-area rule; pure),
//! `tbctl` (the host's controller over the `HideWorld` trait; pure), `tbfakes` (a pretend desktop), `win/tbwin` (the only file that
//! hides or shows a real window). `beside` is the guard's side. docs/design/taskbar-hide.md.
//!
//! Plan: notes/design/2026-11-12-kabuk-plan.md, sections 4.1, 4.2, 5 (A3, E1) and 6.

#![deny(clippy::unwrap_used)]
#![cfg_attr(test, allow(clippy::unwrap_used))]

pub mod beside;
pub mod cli;
pub mod dryrun;
pub mod fakes;
pub mod json;
pub mod machine;
pub mod messages;
pub mod paths;
pub mod runner;
pub mod statefile;
pub mod sys;
pub mod tbctl;
pub mod tbfakes;
pub mod tbmodel;
pub mod win;
pub mod winro;

#[cfg(test)]
mod machine_tests;
#[cfg(test)]
mod runner_tests;
#[cfg(test)]
mod tbbeside_tests;
#[cfg(test)]
mod tbctl_tests;
