//! rustyfiles-shellguard.exe. See the library's docs and `cli.rs` for the command line.
//!
//! Built for the Windows subsystem: when Winlogon starts it as the shell there must be no console
//! window. Text output (`--dry-run`, `--check`, errors) goes through `win::console`.

#![windows_subsystem = "windows"]

use std::io::Write;
use std::process::ExitCode;

use rf_shellguard::cli::{self, Mode};
use rf_shellguard::fakes::{MemRegistry, NoHotkey, NullTaskbar, NullWorkAreas, new_rec};
use rf_shellguard::runner::{RunConfig, run_guard};
use rf_shellguard::statefile::{self, FsStore, ShellStateFile};
use rf_shellguard::sys::{Clock, HostLaunch, HostLink, Hotkey, Registry, Sys, TaskbarVisibility, WorkAreas};
use rf_shellguard::win::hotkey::RealHotkey;
use rf_shellguard::win::link::RealLink;
use rf_shellguard::win::procs::RealProcesses;
use rf_shellguard::win::registry::RealRegistry;
use rf_shellguard::win::session::{Names, RealSession, install_end_session_handler};
use rf_shellguard::win::taskbar::RealTaskbar;
use rf_shellguard::win::ui::RealUi;
use rf_shellguard::win::workarea::RealWorkAreas;
use rf_shellguard::win::{SystemClock, console};
use rf_shellguard::{beside, dryrun, paths};

/// A log file next to the state file, trimmed when it gets large. Failing to log never matters.
struct Log {
    file: Option<std::fs::File>,
}

impl Log {
    fn open(dir: &std::path::Path) -> Log {
        let path = dir.join("shell-guard.log");
        let _ = std::fs::create_dir_all(dir);
        if std::fs::metadata(&path).map(|m| m.len() > 200 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&path, dir.join("shell-guard.log.old"));
        }
        Log { file: std::fs::OpenOptions::new().create(true).append(true).open(path).ok() }
    }

    fn line(&mut self, text: &str) {
        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{} [{}] {text}", SystemClock.now_ms() / 1000, std::process::id());
        }
    }
}

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args_os().skip(1)) {
        Ok(a) => a,
        Err(e) => {
            let _ = writeln!(console::out(), "rustyfiles-shellguard: {e}");
            return ExitCode::from(2);
        }
    };

    if args.mode == Mode::Check {
        let dir = paths::state_dir(args.state_dir.as_deref(), &|k| std::env::var_os(k));
        let state = ShellStateFile::peek(&statefile::path_in(&dir));
        let _ = writeln!(console::out(), "rustyfiles-shellguard {} ok (state file: {:?})", env!("CARGO_PKG_VERSION"), state.status);
        return ExitCode::SUCCESS;
    }

    if args.dry_run {
        let inputs = dryrun::gather(&args);
        let mut out = console::out();
        for line in dryrun::render(&inputs, &args, SystemClock.now_ms()) {
            let _ = writeln!(out, "{line}");
        }
        return ExitCode::SUCCESS;
    }

    // A test run keeps its state file and log in a folder of its own, never in the real settings folder.
    let dir = paths::choose_state_dir(args.state_dir.as_deref(), args.test_shell.is_some(), &|k| std::env::var_os(k), &std::env::temp_dir(), std::process::id());
    let mut log = Log::open(&dir);
    log.line(&format!("start: {:?} panic={} test_shell={} ignored={:?}", args.mode, args.panic, args.test_shell.is_some(), args.ignored));
    let mut logf = |s: &str| log.line(s);
    let code = match args.mode {
        Mode::Beside(pid) => run_beside(&args, &dir, pid, &mut logf),
        _ => run_shell(&args, &dir, &mut logf),
    };
    log.line(&format!("exit {code}"));
    ExitCode::from(code.clamp(0, 255) as u8)
}

fn run_beside(args: &cli::Args, dir: &std::path::Path, pid: u32, log: &mut dyn FnMut(&str)) -> i32 {
    let mut registry = RealRegistry;
    let mut procs = RealProcesses::new(None);
    let mut hotkey = NoHotkey;
    let mut session = RealSession::new(Names::real());
    let mut workarea = RealWorkAreas;
    let mut taskbar = RealTaskbar;
    let mut store = FsStore { path: statefile::path_in(dir) };
    let mut ui = RealUi;
    install_end_session_handler();
    let mut sys = Sys {
        clock: &SystemClock,
        registry: &mut registry,
        procs: &mut procs,
        hotkey: &mut hotkey,
        session: &mut session,
        workarea: &mut workarea,
        taskbar: &mut taskbar,
        store: &mut store,
        ui: &mut ui,
    };
    // With the host's two events the guard confirms to the host that it is armed and hears when the host restored cleanly.
    let mut link = args.link.as_ref().map(|(r, d)| RealLink::new(r, d));
    beside::run_beside_linked(&mut sys, link.as_mut().map(|l| l as &mut dyn HostLink), pid, log)
}

fn run_shell(args: &cli::Args, dir: &std::path::Path, log: &mut dyn FnMut(&str)) -> i32 {
    let state = ShellStateFile::peek(&statefile::path_in(dir));
    let test = args.test_shell.is_some();

    // The registry: in test mode an in-memory one that starts with a Shell value set, so the fall-back has something to clear.
    let mut real_registry = RealRegistry;
    let mut mem_registry = MemRegistry::new(Some("test-shell"), new_rec());
    let registry: &mut dyn Registry = if test { &mut mem_registry } else { &mut real_registry };

    let mut procs = RealProcesses::new(args.test_explorer.clone());
    let mut real_hotkey = RealHotkey::new(if test { "Local\\rustyfiles-test-shellguard-escape" } else { "Local\\rustyfiles-shellguard-escape" });
    let mut no_hotkey = NoHotkey;
    let hotkey: &mut dyn Hotkey = if !test || args.test_hotkey { &mut real_hotkey } else { &mut no_hotkey };
    let mut session = RealSession::new(if test { Names::test() } else { Names::real() });
    let mut real_work = RealWorkAreas;
    let mut null_work = NullWorkAreas::new(new_rec());
    let workarea: &mut dyn WorkAreas = if test { &mut null_work } else { &mut real_work };
    let mut real_taskbar = RealTaskbar;
    let mut null_taskbar = NullTaskbar::new(new_rec());
    let taskbar: &mut dyn TaskbarVisibility = if test { &mut null_taskbar } else { &mut real_taskbar };
    let mut store = FsStore { path: statefile::path_in(dir) };
    let mut ui = RealUi;
    install_end_session_handler();

    let exe = match &args.test_shell {
        Some(p) => p.clone(),
        None => paths::host_exe(args.host.as_deref(), &state, std::env::current_exe().ok().as_deref()),
    };
    let host_args = if test { Vec::new() } else { vec!["--shell".to_string(), "--mode".to_string(), "shell".to_string()] };
    let cfg = RunConfig { launch: HostLaunch { exe, args: host_args, ready_event: format!("Local\\rustyfiles-shell-ready-{}", std::process::id()) }, panic: args.panic };

    let mut sys = Sys {
        clock: &SystemClock,
        registry,
        procs: &mut procs,
        hotkey,
        session: &mut session,
        workarea,
        taskbar,
        store: &mut store,
        ui: &mut ui,
    };
    run_guard(&mut sys, &cfg, log).exit_code
}
