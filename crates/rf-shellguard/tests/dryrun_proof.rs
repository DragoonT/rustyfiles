//! Why `--dry-run` is safe to run on the user's PC, as tests.
//!
//! 1. The two source files it can reach (`dryrun.rs`, `winro.rs`) are scanned: no write, window,
//!    process, hook, event or file-changing API name appears in them (comments are skipped), and the
//!    only registry functions are open-for-query, query and close.
//! 2. `render` is run in-process against inputs built by hand and against a temp folder, and the
//!    folder is shown to be byte-for-byte unchanged afterwards (a corrupt state file stays where it is).

use std::path::{Path, PathBuf};

use rf_shellguard::cli::{Args, Mode, Simulation};
use rf_shellguard::dryrun::{Inputs, gather, render};
use rf_shellguard::statefile::ShellStateFile;

fn source(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    // Comments may talk about anything; only code is scanned.
    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

const FORBIDDEN: &[&str] = &[
    // registry writes and write rights
    "RegSetValue", "RegDeleteValue", "RegDeleteKey", "RegCreateKey", "RegSaveKey", "RegRestoreKey", "RegLoadKey", "RegRenameKey",
    "KEY_SET_VALUE", "KEY_WRITE", "KEY_ALL_ACCESS", "KEY_CREATE_SUB_KEY", "reg_set", "reg_delete",
    // windows, messages, input, hooks, hotkeys
    "CreateWindow", "MessageBox", "ShowWindow", "SetWindowPos", "SendMessage", "PostMessage", "SendInput", "keybd_event", "mouse_event",
    "RegisterHotKey", "SetWindowsHookEx", "RegisterShellHook", "SetShellWindow", "SetTaskmanWindow", "SystemParametersInfo", "AllocConsole",
    // processes and events
    "CreateProcess", "ShellExecute", "Command::new", "std::process", "TerminateProcess", "OpenProcess", "CreateEvent", "SetEvent", "OpenEvent",
    // files: anything but reading metadata
    "fs::write", "File::create", "create_dir", "remove_file", "remove_dir", "fs::rename", "fs::copy", "OpenOptions", "set_permissions",
    // the real implementations and the runner
    "crate::win::", "rf_shellguard::win::", "Runner", "run_guard", "run_beside", "Sys {", "StateStore", ".save(", "::load(", "quarantine",
];

#[test]
fn the_dry_run_files_name_no_write_window_process_or_hook_api() {
    for rel in ["src/dryrun.rs", "src/winro.rs"] {
        let code = source(rel);
        for bad in FORBIDDEN {
            assert!(!code.contains(bad), "{rel} mentions {bad:?}");
        }
    }
}

#[test]
fn the_only_registry_calls_are_open_query_and_close_with_the_query_right() {
    let code = source("src/winro.rs");
    let mut found = std::collections::BTreeSet::new();
    let bytes = code.as_bytes();
    let mut i = 0;
    while i + 3 < bytes.len() {
        if &bytes[i..i + 3] == b"Reg" && (i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_')) && bytes[i + 3].is_ascii_uppercase() {
            let end = (i..bytes.len()).find(|&j| !bytes[j].is_ascii_alphanumeric()).unwrap_or(bytes.len());
            found.insert(code[i..end].to_string());
            i = end;
        } else {
            i += 1;
        }
    }
    let expected: std::collections::BTreeSet<String> = ["RegCloseKey", "RegOpenKeyExW", "RegQueryValueExW"].iter().map(|s| s.to_string()).collect();
    assert_eq!(found, expected);
    assert!(code.contains("KEY_QUERY_VALUE"));
}

#[test]
fn dryrun_reads_the_state_file_with_peek_only() {
    let code = source("src/dryrun.rs");
    assert!(code.contains("ShellStateFile::peek"));
    assert!(!code.contains("ShellStateFile::load"));
}

// ---- the output ----------------------------------------------------------------------------

fn inputs(shell: Option<&str>) -> Inputs {
    Inputs {
        shell_value: Ok(shell.map(str::to_owned)),
        override_policy: Ok(None),
        safe_mode: false,
        shift_held: false,
        build: Some("26300.9974".into()),
        state: ShellStateFile::empty(),
        state_path: PathBuf::from("shell-state.json"),
        host: PathBuf::from("C:\\somewhere\\rustyfiles.exe"),
        host_exists: true,
    }
}

fn args() -> Args {
    Args { dry_run: true, ..Args::default() }
}

fn text(lines: &[String]) -> String {
    lines.join("\n")
}

fn line_of(lines: &[String], needle: &str) -> usize {
    lines.iter().position(|l| l.contains(needle)).unwrap_or_else(|| panic!("no line with {needle:?} in:\n{}", text(lines)))
}

#[test]
fn the_default_plan_starts_the_host_and_then_waits() {
    let lines = render(&inputs(Some("\"C:\\Users\\someone\\secret folder\\guard.exe\"")), &args(), 1_000_000);
    let all = text(&lines);
    assert!(all.contains("Nothing is written, no window is created, no program is started"));
    assert!(all.contains("set (program: guard.exe)"));
    assert!(!all.contains("someone") && !all.contains("secret"), "folders and user names are never printed:\n{all}");
    assert!(line_of(&lines, "create the session's ShellReadyEvent") < line_of(&lines, "start the shell host"));
    assert!(line_of(&lines, "start the shell host") < line_of(&lines, "signal ShellReadyEvent"));
    assert!(all.contains("then: the guard waits on the host's process handle"));
    assert!(!all.contains("delete the Winlogon Shell value"), "a plain run deletes nothing");
    assert!(all.contains("Would the host be started by a real run at sign-in: yes"));
}

#[test]
fn no_shell_value_or_no_host_means_no_start() {
    let all = text(&render(&inputs(None), &args(), 0));
    assert!(all.contains("not set, so Windows starts Explorer"));
    assert!(all.contains("Would the host be started by a real run at sign-in: no"));
    let mut i = inputs(Some("guard.exe"));
    i.host_exists = false;
    assert!(text(&render(&i, &args(), 0)).contains("NOT found"));
}

#[test]
fn a_policy_that_overrides_the_shell_is_called_out() {
    let mut i = inputs(Some("guard.exe"));
    i.override_policy = Ok(Some(1));
    assert!(text(&render(&i, &args(), 0)).contains("the organisation sets the shell"));
}

#[test]
fn panic_plan_deletes_the_value_before_it_starts_explorer() {
    let lines = render(&inputs(Some("guard.exe")), &Args { panic: true, ..args() }, 0);
    assert!(line_of(&lines, "release the shell identity") < line_of(&lines, "delete the Winlogon Shell value"));
    assert!(line_of(&lines, "delete the Winlogon Shell value") < line_of(&lines, "start explorer.exe by its full path"));
    assert!(text(&lines).contains("exit the guard with code 0"));
}

#[test]
fn safe_mode_and_shift_plans_leave_the_value() {
    for (safe, shift) in [(true, false), (false, true)] {
        let mut i = inputs(Some("guard.exe"));
        i.safe_mode = safe;
        i.shift_held = shift;
        let all = text(&render(&i, &args(), 0));
        assert!(!all.contains("delete the Winlogon Shell value"));
        assert!(all.contains("start explorer.exe by its full path"));
    }
}

#[test]
fn scenarios_show_the_whole_way_back() {
    let crash = text(&render(&inputs(Some("guard.exe")), &Args { simulate: Some(Simulation::CrashLoop), ..args() }, 1_000_000));
    assert!(crash.contains("fall-back reason: crash_loop") && crash.contains("delete the Winlogon Shell value") && crash.contains("exit the guard with code 0"), "{crash}");
    let escape = text(&render(&inputs(Some("guard.exe")), &Args { simulate: Some(Simulation::Escape), ..args() }, 0));
    assert!(escape.contains("fall-back reason: escape") && escape.contains("exit the guard with code 0"));
    let leave = text(&render(&inputs(Some("guard.exe")), &Args { simulate: Some(Simulation::UserExit), ..args() }, 0));
    assert!(leave.contains("fall-back reason: user_exit"));
}

#[test]
fn the_beside_plan_only_shows_the_taskbar_and_the_flag() {
    let lines = render(&inputs(None), &Args { mode: Mode::Beside(4242), ..args() }, 0);
    let all = text(&lines);
    assert!(all.contains("beside-guard for process 4242"));
    assert!(all.contains("show the Windows taskbar again") && all.contains("clear pending_restore"));
    assert!(!all.contains("explorer.exe"));
}

// ---- gather leaves the disk alone ------------------------------------------------------------

fn snapshot(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| {
            let e = e.unwrap();
            (e.file_name().to_string_lossy().into_owned(), std::fs::read(e.path()).unwrap())
        })
        .collect();
    v.sort();
    v
}

#[test]
fn gather_and_render_change_nothing_on_disk_even_for_a_corrupt_state_file() {
    let dir = std::env::temp_dir().join(format!("rf-shellguard-test-{}-dryrun", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("shell-state.json"), b"{ not json").unwrap();
    std::fs::write(dir.join("other.txt"), b"x").unwrap();
    let before = snapshot(&dir);

    let a = Args { dry_run: true, state_dir: Some(dir.clone()), host: Some(dir.join("no-such-host.exe")), ..Args::default() };
    let inp = gather(&a); // the real read-only queries
    let lines = render(&inp, &a, 1_000_000);
    assert!(text(&lines).contains("State file: Corrupt"));
    assert!(text(&lines).contains("NOT found"));

    assert_eq!(snapshot(&dir), before, "the dry run must leave the folder as it found it");
    let _ = std::fs::remove_dir_all(&dir);
}
