//! Shows the Windows taskbar again (beside mode, plan 6.6; also part of the fall-back). The code is in `tbwin`, which is the only file
//! that hides or shows a window of Explorer's; this name stays for the callers that already use it.
//! NOT RUN by the author of this crate (it touches Explorer's windows).

pub use super::tbwin::RealTaskbar;
