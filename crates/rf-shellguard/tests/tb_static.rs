//! Why no test and no example of this crate can hide, show or message a window of Explorer's: the sources are scanned.
//!
//! * Only `src/win/tbwin.rs` calls `SetWindowPos` (and only with the hide / show flags, queued, after checking class and owner).
//! * Nothing in `tests/` or `examples/` names the functions that act on a foreign window, or the Win32 calls that could.
//! * The guard never writes an app-bar state (`ABM_SETSTATE`), a registry value of the taskbar, or sends a window message.

use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Code only: comment lines are skipped (they may talk about anything).
fn code_of(path: &Path) -> String {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    text.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn all(sub: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    rust_files(&root().join(sub), &mut v);
    v
}

const WINDOW_CHANGING: &[&str] = &["SetWindowPos", "ShowWindow", "SendMessage", "PostMessage", "SendNotifyMessage", "SetWindowLong", "MoveWindow", "DestroyWindow", "CloseWindow", "EndTask"];

#[test]
fn only_tbwin_changes_a_window_and_only_once_in_one_place() {
    for f in all("src") {
        let name = f.strip_prefix(root()).unwrap().to_string_lossy().replace('\\', "/");
        let code = code_of(&f);
        for bad in WINDOW_CHANGING {
            // (the words may appear in this crate's own test and doc strings; code is scanned without comments, and the
            // test modules of the files below only contain fakes)
            if code.contains(bad) && name != "src/win/tbwin.rs" {
                // the guard's other real files (hotkey) post to their own thread; those are listed, nothing else may
                let allowed = matches!(name.as_str(), "src/win/hotkey.rs" | "src/win/ui.rs" | "src/win/procs.rs") && !bad.contains("SetWindowPos");
                assert!(allowed, "{name} mentions {bad}");
            }
        }
    }
    let tb = code_of(&root().join("src/win/tbwin.rs"));
    assert_eq!(tb.matches("SetWindowPos(").count(), 1, "one call, in one place");
    assert!(tb.contains("SWP_ASYNCWINDOWPOS") && tb.contains("SWP_NOZORDER") && tb.contains("SWP_NOACTIVATE"));
    for bad in ["ShowWindow", "SendMessage", "PostMessage", "ABM_", "SHAppBarMessage", "SystemParametersInfo", "Reg", "SetWindowLong", "MoveWindow"] {
        assert!(!tb.contains(bad), "tbwin.rs mentions {bad}");
    }
}

#[test]
fn nothing_in_tests_or_examples_acts_on_a_window() {
    let this = root().join("tests").join("tb_static.rs");
    let mut files = all("tests");
    files.extend(all("examples"));
    for f in files {
        if f == this {
            continue;
        }
        let code = code_of(&f);
        // dryrun_proof.rs lists API names as DATA (what the dry run must never mention); there only our own functions are checked.
        let listed_as_data = f.file_name().is_some_and(|n| n == "dryrun_proof.rs");
        let api: &[&str] = if listed_as_data { &[] } else { WINDOW_CHANGING };
        for bad in api.iter().chain(["set_visible", "RealTaskbar", "sweep_show_all", "show_recorded", ".hide(", "show_and_verify", "SWP_HIDEWINDOW", "SWP_SHOWWINDOW"].iter()) {
            assert!(!code.contains(bad), "{} mentions {bad}", f.display());
        }
    }
}

#[test]
fn the_probe_only_lists() {
    let code = code_of(&root().join("examples").join("taskbar_probe.rs"));
    assert!(code.contains("list_taskbar_windows"));
    assert!(code.contains("ShellStateFile::peek") && !code.contains("::load("), "the state file is peeked, never moved or written");
    for bad in [".save(", "fs::write", "create_dir", "Command::new", "CreateWindow", "RegisterHotKey"] {
        assert!(!code.contains(bad), "the probe mentions {bad}");
    }
}

#[test]
fn the_guard_changes_no_persistent_taskbar_setting() {
    for f in all("src") {
        let code = code_of(&f);
        let name = f.strip_prefix(root()).unwrap().to_string_lossy().replace('\\', "/");
        assert!(!code.contains("ABM_SETSTATE") && !code.contains("SHAppBarMessage"), "{name} touches the app-bar state");
        // The work area is only ever set without the "update the profile" flag.
        if code.contains("SPI_SETWORKAREA") {
            assert!(name == "src/win/workarea.rs", "{name} sets the work area");
            assert!(!code.contains("SPIF_UPDATEINIFILE"), "the work area must not be written to the profile");
        }
    }
}

#[test]
fn the_guard_sets_the_work_area_as_a_per_monitor_aware_caller() {
    // The guard has no DPI manifest; the rectangles in the state file are physical pixels measured by the host. Without the thread
    // being made aware for the call, Windows would read them as virtualised coordinates.
    let code = code_of(&root().join("src/win/workarea.rs"));
    assert!(code.contains("SetThreadDpiAwarenessContext") && code.contains("DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2"));
    let at_set = code.find("SPI_SETWORKAREA, 0").expect("the call");
    let at_aware = code.find("SetThreadDpiAwarenessContext(DPI").expect("awareness");
    assert!(at_aware < at_set, "aware first, then the call");
}
