//! Looking for and starting the program of a command typed into the address field (GitHub issue 14).
//!
//! The rules that decide whether the text is a command at all are in `rf_core::addrcmd`; this file finds the program the way
//! Windows' Run box would and starts it through the shell:
//!
//! 1. a program typed with a folder part (`C:\Tools\x.exe`, `tools\x.exe`) is looked for there only (a relative one in the folder
//!    that is open), with the usual extensions added;
//! 2. a bare name (`cmd`, `pwsh`, `wt`) is looked for in the system folders and the folders of `PATH` (the folder that is open is
//!    NOT searched: a file planted in a downloaded folder must not be started by typing a well-known name), including
//!    `%LOCALAPPDATA%\Microsoft\WindowsApps`, where the app execution aliases such as `wt.exe` live;
//! 3. then the `App Paths` entries of the registry (current user first). They are only read; nothing here writes the registry.
//!
//! Looking is plain file-system and registry reading: call it off the UI thread. [`start`] goes to the shell thread by itself.

use std::path::{Path, PathBuf};
use std::time::Duration;

use rf_core::addrcmd::{self, CommandLine};

use crate::wide::wide;

/// A program that was found, and the arguments to give it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub exe: PathBuf,
    pub args: String,
}

/// Is `p` a file a program can be started from? App execution aliases (`wt.exe`, `python.exe` stubs in `WindowsApps`) are
/// reparse points that `std::fs::metadata` (and so `Path::exists`) refuses to follow, so the link itself is looked at.
pub fn is_program_file(p: &Path) -> bool {
    std::fs::symlink_metadata(p).map(|m| !m.is_dir()).unwrap_or(false)
}

/// The folders a bare program name is searched in, in order: System32, the Windows folder, `PATH`, the app execution aliases.
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |d: PathBuf| {
        if !d.as_os_str().is_empty() && !dirs.iter().any(|x| same_path(x, &d)) {
            dirs.push(d);
        }
    };
    if let Some(root) = std::env::var_os("SystemRoot").map(PathBuf::from) {
        add(root.join("System32"));
        add(root);
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            // an entry may be quoted or hold a %VAR%
            let s = d.to_string_lossy().trim_matches('"').to_string();
            add(PathBuf::from(addrcmd::expand_vars(&s, &|n| std::env::var(n).ok())));
        }
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        add(local.join(r"Microsoft\WindowsApps"));
    }
    dirs
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().trim_end_matches(['\\', '/']).eq_ignore_ascii_case(b.to_string_lossy().trim_end_matches(['\\', '/']))
}

/// The `App Paths` entry of `exe_name` (`wt.exe`): its default value, current user first. Read-only.
fn app_paths_lookup(exe_name: &str) -> Option<PathBuf> {
    let key = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe_name}");
    for root in [windows_registry::CURRENT_USER, windows_registry::LOCAL_MACHINE] {
        let Ok(k) = root.open(&key) else { continue };
        let Ok(v) = k.get_string("") else { continue };
        let v = addrcmd::expand_vars(v.trim().trim_matches('"'), &|n| std::env::var(n).ok());
        if !v.is_empty() {
            return Some(PathBuf::from(v));
        }
    }
    None
}

/// Looks for the program of the first reading of `readings` that names one. `work_dir` is the folder a relative program is
/// taken from.
pub fn find(readings: &[CommandLine], work_dir: &Path) -> Option<Found> {
    find_with(readings, work_dir, &search_dirs(), &app_paths_lookup, &is_program_file)
}

/// [`find`] with the world passed in (tests use folders they made themselves).
pub fn find_with(
    readings: &[CommandLine],
    work_dir: &Path,
    dirs: &[PathBuf],
    app_paths: &dyn Fn(&str) -> Option<PathBuf>,
    is_file: &dyn Fn(&Path) -> bool,
) -> Option<Found> {
    for r in readings {
        let program = addrcmd::expand_vars(r.program.trim(), &|n| std::env::var(n).ok());
        if program.is_empty() {
            continue;
        }
        let hit = if addrcmd::has_dir_part(&program) {
            let base = if addrcmd::is_absolute(&program) { PathBuf::from(&program) } else { work_dir.join(&program) };
            let base = base.to_string_lossy().into_owned();
            addrcmd::candidate_names(&base).into_iter().map(PathBuf::from).find(|p| is_file(p))
        } else {
            let names = addrcmd::candidate_names(&program);
            dirs.iter()
                .flat_map(|d| names.iter().map(move |n| d.join(n)))
                .find(|p| is_file(p))
                .or_else(|| app_paths(&addrcmd::app_paths_name(&program)).filter(|p| is_file(p)))
        };
        if let Some(exe) = hit {
            return Some(Found { exe, args: r.args.clone() });
        }
    }
    None
}

/// What starting a program came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Started {
    Yes,
    /// The program asked for administrator rights by its own prompt and the user said no. Not an error: nothing to show.
    Declined,
    /// The shell had not answered when the wait ended. The start is still going on in its own thread (a program that elevates
    /// itself waits there for its consent prompt, which is on the screen); whether it succeeds is not known to the caller.
    Pending,
    /// Windows refused (the program is damaged, access is denied, ...): the message for the user.
    Failed(String),
}

/// How long [`start`] waits for the shell. `ShellExecuteEx` returns in milliseconds for a normal program; only a program that
/// elevates itself keeps it waiting for the user, and that wait is not ours to hold.
const START_WAIT: Duration = Duration::from_secs(15);

/// ERROR_CANCELLED as an HRESULT: the user said no at the consent prompt.
const DECLINED: u32 = 0x8007_04C7;

/// A failure of `ShellExecuteEx`: its HRESULT and the message.
type ExecError = (u32, String);

/// Starts `found` through the shell (`ShellExecuteEx`, the default verb), with the arguments as typed and `work_dir` as the
/// working directory. No elevation is asked for: a program that wants it asks by its own prompt.
///
/// The call is made on a thread of its own that ends right after (as the dock's launcher and `shell::open_terminal_admin` do), NOT
/// on the shared shell thread of `sta`: a program that elevates itself keeps `ShellExecuteEx` waiting for the consent prompt, and
/// the dock, the desktop and the context menus all queue behind that thread. Call this from a worker thread, never from the UI
/// thread (it waits up to [`START_WAIT`]).
pub fn start(found: &Found, work_dir: &Path) -> Started {
    let (exe, args, dir) = (found.exe.clone(), found.args.clone(), work_dir.to_path_buf());
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new().name("rf-runcmd".into()).spawn(move || {
        let _ = tx.send(exec(&exe, &args, &dir));
    });
    if let Err(e) = spawned {
        return Started::Failed(e.to_string());
    }
    wait_for_start(&rx, START_WAIT)
}

/// Turns the answer of the starting thread (or its absence) into a [`Started`].
pub fn wait_for_start(rx: &std::sync::mpsc::Receiver<Result<(), ExecError>>, limit: Duration) -> Started {
    use std::sync::mpsc::RecvTimeoutError;
    match rx.recv_timeout(limit) {
        Ok(Ok(())) => Started::Yes,
        Ok(Err((DECLINED, _))) => Started::Declined,
        Ok(Err((_, message))) => Started::Failed(message),
        Err(RecvTimeoutError::Timeout) => {
            log::warn!("address command: the shell had not answered after {limit:?}");
            Started::Pending
        }
        Err(RecvTimeoutError::Disconnected) => Started::Failed("the thread that starts programs ended without an answer".into()),
    }
}

fn exec(exe: &Path, args: &str, dir: &Path) -> Result<(), ExecError> {
    use windows::Win32::UI::Shell::{SEE_MASK_FLAG_NO_UI, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::PCWSTR;

    crate::shell::ensure_com();
    let file = wide(exe);
    let params = wide(args);
    let cwd = wide(dir);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        // no verb: the default one, "open", exactly as a double click; the error boxes are ours to show
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: if args.is_empty() { PCWSTR::null() } else { PCWSTR(params.as_ptr()) },
        lpDirectory: PCWSTR(cwd.as_ptr()),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    // SAFETY: the buffers outlive the call; the call returns after the process was created (NOASYNC)
    unsafe { ShellExecuteExW(&mut info) }.map_err(|e| (e.code().0 as u32, e.message()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cl(p: &str, a: &str) -> CommandLine {
        CommandLine { program: p.into(), args: a.into() }
    }

    /// A folder of this test's own, removed at the end. Nothing in it is ever run.
    struct Temp(PathBuf);
    impl Temp {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rf-runcmd-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Temp(p)
        }
        fn file(&self, rel: &str) -> PathBuf {
            let p = self.0.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, b"not a program").unwrap();
            p
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_bare_name_is_searched_in_the_given_folders_in_order_and_gets_an_extension() {
        let t = Temp::new("bare");
        let (a, b) = (t.0.join("a"), t.0.join("b"));
        let in_b = t.file(r"b\tool.exe");
        let found = find_with(&[cl("tool", "/x y")], &t.0, &[a.clone(), b.clone()], &|_| None, &is_program_file);
        assert_eq!(found, Some(Found { exe: in_b.clone(), args: "/x y".into() }));
        // the first folder that has it wins
        let in_a = t.file(r"a\tool.cmd");
        let found = find_with(&[cl("tool", "")], &t.0, &[a, b], &|_| None, &is_program_file);
        assert_eq!(found.map(|f| f.exe), Some(in_a));
        // a folder named like the program is not a program
        std::fs::create_dir_all(t.0.join(r"c\dir.exe")).unwrap();
        assert_eq!(find_with(&[cl("dir", "")], &t.0, &[t.0.join("c")], &|_| None, &is_program_file), None);
    }

    #[test]
    fn the_open_folder_is_not_searched_for_a_bare_name() {
        let t = Temp::new("planted");
        t.file("cmd.exe");
        assert_eq!(find_with(&[cl("cmd", "")], &t.0, &[], &|_| None, &is_program_file), None);
    }

    #[test]
    fn a_program_with_a_folder_part_is_taken_from_there() {
        let t = Temp::new("dirpart");
        let exe = t.file(r"My Tools\a b.exe");
        let abs = exe.to_string_lossy().into_owned();
        assert_eq!(find_with(&[cl(&abs, "-q")], Path::new("Z:\\nowhere"), &[], &|_| None, &is_program_file), Some(Found { exe: exe.clone(), args: "-q".into() }));
        // without the extension
        let no_ext = abs.trim_end_matches(".exe").to_string();
        assert_eq!(find_with(&[cl(&no_ext, "")], Path::new("Z:\\nowhere"), &[], &|_| None, &is_program_file).map(|f| f.exe), Some(exe.clone()));
        // relative: from the folder that is open
        assert_eq!(find_with(&[cl(r"My Tools\a b", "")], &t.0, &[], &|_| None, &is_program_file).map(|f| f.exe), Some(exe.clone()));
        assert_eq!(find_with(&[cl(r"My Tools\a b", "")], &t.0.join("My Tools"), &[], &|_| None, &is_program_file), None);
        // an unquoted full path with spaces: the later reading is the one that exists
        let readings = rf_core::addrcmd::split_command(&format!("{abs} status"));
        assert!(readings.len() >= 2);
        assert_eq!(find_with(&readings, &t.0, &[], &|_| None, &is_program_file), Some(Found { exe, args: "status".into() }));
    }

    #[test]
    fn app_paths_come_after_the_folders() {
        let t = Temp::new("apppaths");
        let reg = t.file(r"Reg\prog.exe");
        let on_path = t.file(r"Path\prog.exe");
        let ask = |n: &str| (n == "prog.exe").then(|| reg.clone());
        assert_eq!(find_with(&[cl("prog", "")], &t.0, &[t.0.join("Path")], &ask, &is_program_file).map(|f| f.exe), Some(on_path));
        assert_eq!(find_with(&[cl("prog", "")], &t.0, &[], &ask, &is_program_file).map(|f| f.exe), Some(reg));
        // an entry that points at nothing is no program
        let gone = |_: &str| Some(t.0.join("gone.exe"));
        assert_eq!(find_with(&[cl("prog", "")], &t.0, &[], &gone, &is_program_file), None);
        // not found at all
        assert_eq!(find_with(&[cl("nothing-like-this", "")], &t.0, &[], &|_| None, &is_program_file), None);
    }

    #[test]
    fn variables_in_the_program_are_expanded() {
        let t = Temp::new("vars");
        let exe = t.file("x.exe");
        // SAFETY of the test: the variable is this test's own and only this test reads it
        unsafe { std::env::set_var("RF_RUNCMD_TEST_DIR", &t.0) };
        let found = find_with(&[cl(r"%RF_RUNCMD_TEST_DIR%\x", "")], Path::new("Z:\\"), &[], &|_| None, &is_program_file);
        assert_eq!(found.map(|f| f.exe), Some(exe));
    }

    #[test]
    fn the_answer_of_the_starting_thread_becomes_a_result() {
        use std::sync::mpsc::sync_channel;
        let short = Duration::from_millis(20);
        let (tx, rx) = sync_channel(1);
        tx.send(Ok(())).unwrap();
        assert_eq!(wait_for_start(&rx, short), Started::Yes);
        // the user said no at the consent prompt of a program that elevates itself: not an error
        let (tx, rx) = sync_channel(1);
        tx.send(Err((0x8007_04C7, "The operation was canceled by the user.".to_string()))).unwrap();
        assert_eq!(wait_for_start(&rx, short), Started::Declined);
        // any other refusal is shown
        let (tx, rx) = sync_channel(1);
        tx.send(Err((0x8007_0005, "Access is denied.".to_string()))).unwrap();
        assert_eq!(wait_for_start(&rx, short), Started::Failed("Access is denied.".into()));
        // no answer in time is not a success
        let (tx, rx) = sync_channel::<Result<(), ExecError>>(1);
        assert_eq!(wait_for_start(&rx, short), Started::Pending);
        // a thread that died without answering is a failure
        drop(tx);
        assert!(matches!(wait_for_start(&rx, short), Started::Failed(_)));
    }

    /// Probe (run with `--nocapture`): which well-known programs this machine resolves, as yes / no and the way they were found
    /// (a folder of the search path or an App Paths entry). Files are looked for, never started.
    #[test]
    fn probe_well_known_programs() {
        for name in ["cmd", "powershell", "pwsh", "wt", "notepad", "calc", "code", "no-such-program-rustyfiles-test"] {
            let r = [cl(name, "")];
            let via_dirs = find_with(&r, Path::new("C:\\"), &search_dirs(), &|_| None, &is_program_file).is_some();
            let via_any = find(&r, Path::new("C:\\")).is_some();
            let way = if via_dirs { "search path" } else if via_any { "App Paths" } else { "not found" };
            println!("probe: {name:<34} {way}");
        }
        assert!(find(&[cl("powershell", "")], Path::new("C:\\")).is_some());
    }

    /// The one probe that looks at the real machine: where `cmd` is found (the file is looked for, never started).
    #[test]
    fn cmd_is_found_in_the_system_folder() {
        let found = find(&[cl("cmd", "/k dir")], Path::new("C:\\")).expect("cmd should be found");
        assert!(found.exe.to_string_lossy().to_lowercase().ends_with(r"\system32\cmd.exe"), "{:?}", found.exe);
        assert_eq!(found.args, "/k dir");
        assert!(find(&[cl("no-such-program-rustyfiles-test", "")], Path::new("C:\\")).is_none());
    }
}
