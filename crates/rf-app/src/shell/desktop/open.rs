//! Opening a folder from the desktop.
//!
//! In the preview (a main-app process) a folder opens in a rustyfiles window of the same process. In the shell host (the dock's
//! process, a SIDE process: no tray icon, no single-instance forwarding, it never saves the settings) it must not: a file window made
//! there would be a second, half-wired copy of the app inside the thing that draws the user's taskbar and desktop, and it would
//! die with it. There the place is handed to the main rustyfiles, in this order:
//!
//! 1. THROUGH ITS PIPE. A running rustyfiles serves a named pipe (`rf_win::single_instance`) that every second launch writes its
//!    argument to; the host writes the same line itself, at once, without starting a process. Before, every open started a new
//!    `rustyfiles.exe` (a large program: the loader, Velopack's start-up check, the panic log, GDI+ and the logger) that had to find
//!    the pipe and write the same line before the window could appear.
//! 2. BY STARTING `rustyfiles.exe <argument>` when nothing answers on the pipe (no main app runs): the start is a cold one, as a
//!    shortcut's would be.
//!
//! Both happen on a thread of their own: a pipe write and a process start can each stall on a busy PC, and the UI thread of the host
//! is the thread that draws the desktop. `RUSTYFILES_TRACE_OPEN=1` writes the time marks of an open to the log (at warn level, so that
//! they show with the default filter).

use std::time::Instant;

use gpui_kit::App;
use rf_core::Location;

/// The environment variable that turns the time marks on.
pub const ENV_TRACE: &str = "RUSTYFILES_TRACE_OPEN";

/// The command-line word that makes the main rustyfiles open this place, if the place has one.
pub fn launch_arg(parsing_name: Option<&str>, loc: &Location) -> Option<String> {
    match loc {
        Location::Path(p) => Some(p.to_string_lossy().into_owned()),
        // a shell place (This PC, the Recycle Bin, ...): the name the shell knows it by, which the launcher turns back into the place
        _ => parsing_name.map(str::to_string),
    }
}

/// The line that makes a RUNNING rustyfiles open the place: what its pipe takes (`workspace::parse_launch` reads it). A folder is its
/// path, This PC is the empty line (Home), the Recycle Bin and the shell folders have their own words. Places the pipe has no word for
/// give `None` and go by the command line, which resolves them itself.
pub fn pipe_message(loc: &Location) -> Option<String> {
    match loc {
        Location::Path(p) => Some(p.to_string_lossy().into_owned()),
        Location::Home => Some(String::new()),
        Location::RecycleBin => Some("shell:RecycleBinFolder".to_string()),
        Location::Shell { parsing, name } => Some(format!("{}{parsing}\t{name}", crate::workspace::SHELL_PREFIX)),
        _ => None,
    }
}

/// How a place reached the main rustyfiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// A running rustyfiles took it from its pipe.
    Pipe,
    /// No running rustyfiles answered: a new one was started with the argument.
    Started,
    /// Neither worked.
    Failed,
}

/// Hands a place over: the pipe first (when there is a line for it and pipes are allowed), the command line otherwise. The two ways
/// are parameters so that the order is tested without a process or a pipe.
pub fn deliver(message: Option<&str>, pipe_allowed: bool, send: impl FnOnce(&str) -> bool, start: impl FnOnce() -> std::io::Result<()>) -> Delivery {
    if pipe_allowed && message.is_some_and(send) {
        return Delivery::Pipe;
    }
    match start() {
        Ok(()) => Delivery::Started,
        Err(e) => {
            log::warn!("desktop: could not hand a folder to rustyfiles: {e}");
            Delivery::Failed
        }
    }
}

/// A copy started with `RUSTYFILES_MULTI_INSTANCE` is one of several that must not meet (a test copy): it never writes to the pipe of
/// the user's own rustyfiles, exactly as its own launches do not forward.
fn pipe_allowed() -> bool {
    std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_none()
}

pub(crate) fn tracing() -> bool {
    std::env::var_os(ENV_TRACE).is_some()
}

/// A time mark with the WALL CLOCK (milliseconds since 1970), so that the marks of two processes (the shell host that was
/// double-clicked and the main rustyfiles that shows the window) line up in their two logs: the difference of two marks is the time
/// between them. Does nothing unless `RUSTYFILES_TRACE_OPEN` is set (in the environment of both processes). Names no place: only what
/// happened.
pub(crate) fn mark(what: &str) {
    if tracing() {
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
        log::warn!("open-trace: {what} (wall clock {ms} ms)");
    }
}

/// Whether this process is a side process (the shell host / dock) and so hands folders to the main app.
pub fn hands_over() -> bool {
    crate::background::DOCK_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Opens `loc` (`parsing_name`: its shell name, for places that are not folders on a disk) in a rustyfiles window.
pub fn open_location(parsing_name: Option<&str>, loc: Location, cx: &mut App) {
    if !hands_over() {
        crate::open_main_window(Some(loc), None, None, cx);
        return;
    }
    let Some(arg) = launch_arg(parsing_name, &loc) else { return };
    let message = pipe_message(&loc);
    mark("host: the double click is being handled");
    let asked = Instant::now();
    let spawned = std::thread::Builder::new().name("rf-desktop-open".into()).spawn(move || {
        let on_thread = asked.elapsed();
        let how = deliver(
            message.as_deref(),
            pipe_allowed(),
            |m| rf_win::single_instance::send_to(&rf_win::single_instance::instance_name(), m),
            || {
                let exe = std::env::current_exe()?;
                std::process::Command::new(exe).arg(&arg).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map(|_| ())
            },
        );
        if tracing() {
            log::warn!("open-trace: {how:?}; thread up after {} us, handed over after {} us (from the double click being handled)", on_thread.as_micros(), asked.elapsed().as_micros());
            mark(if how == Delivery::Started { "host: rustyfiles.exe was started (the window comes after its own start-up: RUSTYFILES_TRACE_START=1 in its environment shows it)" } else { "host: handed over" });
        }
    });
    if let Err(e) = spawned {
        log::warn!("desktop: could not start the hand-over thread: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{Delivery, deliver, hands_over, launch_arg, pipe_message};
    use rf_core::Location;
    use std::path::PathBuf;

    #[test]
    fn a_folder_is_handed_over_by_its_path_and_a_shell_place_by_its_name() {
        assert_eq!(launch_arg(None, &Location::Path(PathBuf::from(r"C:\Users\x\Desktop\Sub"))).as_deref(), Some(r"C:\Users\x\Desktop\Sub"));
        assert_eq!(launch_arg(Some("shell:RecycleBinFolder"), &Location::RecycleBin).as_deref(), Some("shell:RecycleBinFolder"));
        assert_eq!(launch_arg(None, &Location::RecycleBin), None, "a place with no name to hand over is not guessed");
    }

    #[test]
    fn the_pipe_gets_the_line_a_second_launch_would_have_sent() {
        assert_eq!(pipe_message(&Location::Path(PathBuf::from(r"C:\Users\x"))).as_deref(), Some(r"C:\Users\x"));
        // This PC is Home: the empty line, as the launcher sends it
        assert_eq!(pipe_message(&Location::Home).as_deref(), Some(""));
        assert_eq!(pipe_message(&Location::RecycleBin).as_deref(), Some("shell:RecycleBinFolder"));
        let shell = Location::Shell { parsing: "::{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}".into(), name: "Ağ".into() };
        let m = pipe_message(&shell).unwrap();
        // what the running window reads back is the same place
        let (loc, sel) = crate::workspace::parse_launch(&m);
        assert_eq!((loc, sel), (shell, None));
        assert_eq!(crate::workspace::parse_launch(&pipe_message(&Location::Home).unwrap()).0, Location::Home);
        assert_eq!(crate::workspace::parse_launch(&pipe_message(&Location::RecycleBin).unwrap()).0, Location::RecycleBin);
        assert_eq!(pipe_message(&Location::Settings), None, "a place with no word on the pipe goes by the command line");
    }

    #[test]
    fn a_running_rustyfiles_takes_the_place_from_its_pipe_and_nothing_is_started() {
        let mut sent = Vec::new();
        let how = deliver(
            Some(r"C:\x"),
            true,
            |m| {
                sent.push(m.to_string());
                true
            },
            || panic!("no process when the pipe took it"),
        );
        assert_eq!((how, sent), (Delivery::Pipe, vec![r"C:\x".to_string()]));
        // This PC is the empty line: still the pipe
        assert_eq!(deliver(Some(""), true, |_| true, || panic!("no process")), Delivery::Pipe);
    }

    #[test]
    fn without_an_answer_on_the_pipe_a_process_is_started() {
        let mut started = 0;
        assert_eq!(
            deliver(Some(r"C:\x"), true, |_| false, || {
                started += 1;
                Ok(())
            }),
            Delivery::Started
        );
        assert_eq!(started, 1);
        // a place with no line, or a copy that must not use the pipe: never even asked
        assert_eq!(deliver(None, true, |_| panic!("no line, no pipe"), || Ok(())), Delivery::Started);
        assert_eq!(deliver(Some("x"), false, |_| panic!("pipes are off for a test copy"), || Ok(())), Delivery::Started);
        assert_eq!(deliver(Some("x"), true, |_| false, || Err(std::io::Error::other("no exe"))), Delivery::Failed);
    }

    #[test]
    fn the_preview_opens_its_own_windows() {
        // (the flag is set by the dock's start-up only)
        assert!(!hands_over());
    }
}
