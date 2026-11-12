//! Commands typed into the address field (GitHub issue 14; setting `address_commands`, on by default as in Explorer).
//!
//! Explorer lets one focus the address bar, type `cmd` (or `powershell`, `wt`, `"C:\Tools\a b\x.exe" /s`) and press Enter: the
//! program starts with the open folder as its working directory and the window stays where it is. This file does that for the
//! text confirmed with Enter, in this order:
//!
//! 1. the text is a PLACE, read exactly as before (`parse_location`: absolute or relative path, drive, `%VAR%`, `@alias`, `home`;
//!    `shell:` places never get here). A relative name is also tried inside the open folder, first, so a folder or file there with
//!    the typed name always wins over a program of that name. If a place exists the tab goes there.
//! 2. only when no such place exists is the text a COMMAND: the first word (or the quoted first part) is the program, the rest are
//!    its arguments as typed. The program is looked for as the Run box would (`rf_win::runcmd`), and started through the shell with
//!    the open folder as the working directory (the profile folder when the tab shows something that is not a folder on disk). The
//!    tab stays, the field shows its path again. No elevation is asked for.
//!    A full path to a program typed ALONE (no arguments) names a file that exists, so it is a place and the tab goes to that file,
//!    as the brief wants ("a folder or file that has the typed name always wins"); the same path with any argument is a command.
//! 3. neither: the field does what it always did with a path that does not exist (the tab goes there and shows its message).
//!    Nothing is started. A web address typed here is also "neither": it is handled as every other missing path was, nothing new.
//!
//! Looking at the disk, searching for the program and starting it all happen off the UI thread. The look at the disk is given
//! [`LOOK_LIMIT`]: a dead mapped drive or a slow share must not hold the field; when the disk does not answer in time the text is
//! taken as a place (what it always was), never as a program.
//!
//! A program is started ONLY from this file, and this file is entered only from the address field's Enter handler
//! ([`Workspace::enter_address_text`], and not for a completion that was picked from the list): `parse_location`, `expand_env`, the
//! completions and every other way a path reaches the window turn text into a place and cannot start anything. A test below reads
//! the sources to keep it that way.
//!
//! This is a child module of `workspace` (see the `#[path]` line there).

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rf_core::addrcmd::{self, Intent};
use rf_win::runcmd::{self, Started};

use super::*;

/// What the background work came to.
enum Outcome {
    /// A place that exists (maybe the typed name inside the open folder): go there.
    Go(PathBuf),
    /// No place and no program: the field does what it did before.
    Neither,
    /// A program was started (or Windows is still starting it, or the user declined its own prompt: nothing to show).
    Started,
    /// A program was found and Windows refused to start it: the message to show.
    Failed(String),
}

impl Workspace {
    /// The address field's Enter. `from_completion`: the text is a completion that was picked in the list (arrow keys), which is a
    /// place by construction and never a command.
    ///
    /// Called by the Enter handler of the address field only.
    pub(super) fn enter_address_text(&mut self, text: String, from_completion: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(loc) = parse_location(&text) else {
            self.focus_active(window, cx);
            cx.notify();
            return;
        };
        let typed = match &loc {
            Location::Path(p) => p.clone(),
            _ => PathBuf::new(),
        };
        let tab = self.active_tab().clone();
        let open_dir = match tab.read(cx).location() {
            Location::Path(p) if !rf_win::ftp::is_ftp(p) && rf_win::network::server_root(p).is_none() && crate::archive_view::split(p).is_none() => Some(p.clone()),
            _ => None,
        };
        let command_possible = command_possible(
            matches!(loc, Location::Path(_)),
            from_completion,
            self.picker.is_some(),
            cx.global::<crate::settings::Settings>().address_commands,
            &text,
            typed.is_relative() && open_dir.is_some(),
        );
        if !command_possible {
            // the way it always was: go to the place
            self.navigate(loc, window, cx);
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let work = cx.background_spawn(async move { resolve_and_run(&text, &typed, open_dir.as_deref()) });
        self.focus_active(window, cx);
        cx.spawn_in(window, async move |this, cx| {
            let outcome = work.await;
            let _ = this.update_in(cx, |ws, window, cx| ws.finish_address_text(outcome, tab, loc, window, cx));
        })
        .detach();
        cx.notify();
    }

    fn finish_address_text(&mut self, outcome: Outcome, tab: Entity<FolderView>, typed: Location, window: &mut Window, cx: &mut Context<Self>) {
        let go = |ws: &mut Self, loc: Location, window: &mut Window, cx: &mut Context<Self>| {
            if ws.tabs.get(ws.active).is_some_and(|t| t.view() == &tab) {
                ws.navigate(loc, window, cx);
            } else {
                // the user switched tabs in the meantime: the answer belongs to the tab that asked
                tab.update(cx, |v, cx| v.navigate(loc, cx));
            }
        };
        match outcome {
            Outcome::Go(p) => go(self, Location::Path(p), window, cx),
            Outcome::Neither => go(self, typed, window, cx),
            Outcome::Started => {}
            Outcome::Failed(why) => {
                use crate::prompt::{Button, Kind};
                crate::prompt::show(t!("Program başlatılamadı"), Some(why), None, vec![Button::new(t!("Tamam"), Kind::Primary, |_, _, _| Ok(()))], window, cx);
            }
        }
        cx.notify();
    }
}

/// Is this Enter allowed to look at the disk and maybe start a program? Not for a completion that was picked, nor while a picker is
/// open, nor with the setting off, nor for text that is not a path (`home`). Text that can only be a place goes there at once
/// (no disk look), except a relative name while a folder is open: that is read against the open folder, which takes the look.
fn command_possible(is_path: bool, from_completion: bool, picker_open: bool, enabled: bool, text: &str, relative_in_open_dir: bool) -> bool {
    is_path && !from_completion && !picker_open && enabled && (relative_in_open_dir || !addrcmd::is_plain_place(text))
}

/// How long the look at the disk may take in all. Past it the text is taken as a place (see the module doc).
const LOOK_LIMIT: Duration = Duration::from_millis(1500);

/// Runs `f` on a thread of its own and waits for it at most `limit`. `None` when it was not done in time (the thread runs on and
/// its answer is dropped; a file call that hangs on a dead share ends when the network gives up).
fn within<T: Send + 'static>(limit: Duration, f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new().name("rf-addr-look".into()).spawn(move || drop(tx.send(f()))).ok()?;
    rx.recv_timeout(limit).ok()
}

/// [`path_exists`] with the shared time limit of one Enter. A path that cannot be answered in time counts as existing: the tab goes
/// there (as it did before this feature) and shows its own message, and nothing is started on a guess.
fn path_exists_in_time(p: &Path, began: Instant) -> bool {
    let left = LOOK_LIMIT.saturating_sub(began.elapsed()).max(Duration::from_millis(1));
    let p = p.to_path_buf();
    within(left, move || path_exists(&p)).unwrap_or_else(|| {
        log::warn!("address command: the disk did not answer in time; the text is taken as a place");
        true
    })
}

/// The readings of the typed path as a place, in the order they are tried, with `.` and `..` folded away. A relative name is read
/// against the open folder ONLY when there is one: the folder of the process (the system folder after an autostart) is not where the
/// user is looking, and a file there must not hide a program (`notepad.exe`).
fn place_readings(typed: &Path, open_dir: Option<&Path>) -> Vec<PathBuf> {
    let one = match open_dir.filter(|_| typed.is_relative()) {
        Some(dir) => dir.join(typed),
        None => typed.to_path_buf(),
    };
    vec![addrcmd::normalize_lexical(&one)]
}

/// Does `p` exist (a folder or a file)? A path that exists but cannot be read counts: it is a place, and the folder view says why
/// it cannot be opened.
fn path_exists(p: &Path) -> bool {
    match std::fs::metadata(p) {
        Ok(_) => true,
        Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
    }
}

/// The folder a started program gets as its working directory: the open folder when it is a folder on the disk, else the profile
/// folder.
fn working_dir(open_dir: Option<&Path>) -> PathBuf {
    open_dir.filter(|d| d.is_dir()).map(Path::to_path_buf).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from(r"C:\"))
}

/// Place or command or neither, and the starting itself. Runs on a worker thread.
fn resolve_and_run(text: &str, typed: &Path, open_dir: Option<&Path>) -> Outcome {
    let began = Instant::now();
    match addrcmd::decide(text, &place_readings(typed, open_dir), &|p| path_exists_in_time(p, began)) {
        Intent::Place(p) => Outcome::Go(p),
        Intent::Neither => Outcome::Neither,
        Intent::Command(readings) => {
            let dir = working_dir(open_dir);
            let Some(found) = runcmd::find(&readings, &dir) else { return Outcome::Neither };
            match runcmd::start(&found, &dir) {
                Started::Yes => {
                    log::info!("address command: started (arguments: {} characters)", found.args.chars().count());
                    Outcome::Started
                }
                Started::Declined => {
                    log::info!("address command: the program's own prompt was declined");
                    Outcome::Started
                }
                Started::Pending => {
                    log::warn!("address command: still starting when the wait ended");
                    Outcome::Started
                }
                Started::Failed(why) => {
                    log::warn!("address command: {why}");
                    Outcome::Failed(why)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    fn sources() -> Vec<(PathBuf, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(PathBuf, String)>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
                }
            }
        }
        let mut out = Vec::new();
        walk(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
        out
    }

    /// Starting a program from typed text is reachable from one place: the Enter handler of the address field.
    #[test]
    fn only_the_enter_handler_can_reach_the_code_that_starts_programs() {
        let all = sources();
        for (path, text) in &all {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if name == "address_cmd.rs" {
                continue;
            }
            assert!(!text.contains("runcmd::"), "{name} reaches rf_win::runcmd");
            // no module next to workspace.rs (they can all see a `pub(super)` method) may call the Enter handler's entry point
            if name != "workspace.rs" {
                assert!(!text.contains("enter_address_text("), "{name} calls enter_address_text");
            }
        }
        let (_, ws) = all.iter().find(|(p, _)| p.file_name().is_some_and(|n| n == "workspace.rs")).expect("workspace.rs");
        assert_eq!(ws.matches("enter_address_text(").count(), 1, "one caller");
        let at = ws.find("enter_address_text(").unwrap();
        // the source has multi-byte text: move the start of the slice back to a character boundary
        let mut start = at.saturating_sub(2500);
        while !ws.is_char_boundary(start) {
            start -= 1;
        }
        let before = &ws[start..at];
        assert!(before.contains("InputEvent::PressEnter"), "the caller is the address field's Enter handler");
        // the pure place parsers know nothing of programs
        let parse = &ws[ws.find("fn parse_location").unwrap()..ws.find("fn expand_env").unwrap()];
        assert!(!parse.contains("address_cmd") && !parse.contains("runcmd") && !parse.contains("spawn"));
    }

    #[test]
    fn the_address_field_looks_at_the_disk_only_when_it_may() {
        use super::command_possible as ok;
        // (is_path, from_completion, picker_open, enabled, text, relative_in_open_dir)
        assert!(ok(true, false, false, true, "cmd /k dir", false));
        // the setting is off: the way it always was
        assert!(!ok(true, false, false, false, "cmd /k dir", false));
        // a completion that was picked, or a picker that is open, never starts anything
        assert!(!ok(true, true, false, true, "cmd /k dir", false));
        assert!(!ok(true, false, true, true, "cmd /k dir", false));
        // text that is not a path (home, shell places)
        assert!(!ok(false, false, false, true, "home", false));
        // text that can only be a place goes there without a look...
        assert!(!ok(true, false, false, true, r"\\server\share", false));
        assert!(!ok(true, false, false, true, r"C:\Users\", false));
        // ...but a relative name with a folder open is read against that folder, which takes a look ('docs\' like 'docs')
        assert!(ok(true, false, false, true, r"docs\", true));
        // a command that ends with a separator is a command
        assert!(ok(true, false, false, true, r"cmd /k cd C:\", false));
    }

    #[test]
    fn a_relative_name_is_read_against_the_open_folder_only_and_dots_are_folded() {
        use super::place_readings;
        use std::path::Path;
        let open = Path::new(r"C:\a\b");
        // the folder of the process is not a second reading (a cmd.exe there must not hide the program)
        assert_eq!(place_readings(Path::new("cmd.exe"), Some(open)), vec![PathBuf::from(r"C:\a\b\cmd.exe")]);
        assert_eq!(place_readings(Path::new(".."), Some(open)), vec![PathBuf::from(r"C:\a")]);
        assert_eq!(place_readings(Path::new(r"..\c"), Some(open)), vec![PathBuf::from(r"C:\a\c")]);
        assert_eq!(place_readings(Path::new(r"docs\"), Some(open)), place_readings(Path::new("docs"), Some(open)));
        // no folder open: the text is read as it always was
        assert_eq!(place_readings(Path::new("cmd.exe"), None), vec![PathBuf::from("cmd.exe")]);
        // an absolute path ignores the open folder
        assert_eq!(place_readings(Path::new(r"D:\x\..\y"), Some(open)), vec![PathBuf::from(r"D:\y")]);
    }

    #[test]
    fn a_slow_disk_look_is_cut_short() {
        use std::time::{Duration, Instant};
        assert_eq!(super::within(Duration::from_secs(5), || 7), Some(7));
        let t = Instant::now();
        assert_eq!(super::within(Duration::from_millis(30), || std::thread::sleep(Duration::from_millis(600))), None);
        assert!(t.elapsed() < Duration::from_millis(400));
        // a path that is not there answers "no" at once, with the whole limit still ahead
        let missing = std::env::temp_dir().join(format!("rf-addrcmd-missing-{}", std::process::id()));
        assert!(!super::path_exists_in_time(&missing, Instant::now()));
    }

    #[test]
    fn a_relative_name_is_tried_inside_the_open_folder_first_and_a_program_only_when_no_place_exists() {
        let root = std::env::temp_dir().join(format!("rf-addrcmd-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("cmd")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        // a folder of the same name as the program wins
        assert!(matches!(super::resolve_and_run("cmd", std::path::Path::new("cmd"), Some(&root)), super::Outcome::Go(p) if p == root.join("cmd")));
        assert!(matches!(super::resolve_and_run("docs", std::path::Path::new("docs"), Some(&root)), super::Outcome::Go(p) if p == root.join("docs")));
        // an absolute place that exists
        assert!(matches!(super::resolve_and_run(&root.to_string_lossy(), &root, None), super::Outcome::Go(p) if p == root));
        // neither a place nor a program: nothing is started
        assert!(matches!(super::resolve_and_run("no-such-thing-rustyfiles", std::path::Path::new("no-such-thing-rustyfiles"), Some(&root)), super::Outcome::Neither));
        assert!(matches!(super::resolve_and_run("https://example.com", std::path::Path::new("https://example.com"), Some(&root)), super::Outcome::Neither));
        assert!(matches!(super::resolve_and_run(r"Z:\no\such\dir", std::path::Path::new(r"Z:\no\such\dir"), None), super::Outcome::Neither));
        let _ = std::fs::remove_dir_all(&root);
    }
}
