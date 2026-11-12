//! The command line.
//!
//! ```text
//! rustyfiles-shellguard.exe                      run as the shell (what Winlogon starts)
//! rustyfiles-shellguard.exe --panic              give the user Explorer back, now
//! rustyfiles-shellguard.exe --beside-guard <pid> watch the dock host, show the Windows taskbar when it dies
//! rustyfiles-shellguard.exe --test-shell <exe>   supervise <exe> with an in-memory registry (never the real one)
//!                           --test-explorer <exe> [--test-hotkey]   (a test run keeps its files in a temp folder)
//! rustyfiles-shellguard.exe --dry-run [...]      print what it would do, change nothing
//!                           [--simulate crash-loop|escape|user-exit]
//! rustyfiles-shellguard.exe --check              self-test: prints its version, creates nothing
//! common: --state-dir <dir>  --host <exe>
//! ```
//!
//! Winlogon starts the guard from the Shell value, so an argument it does not know is logged and
//! ignored rather than refused: a typo in that value must not leave the user with no shell. A known
//! flag with a missing or bad value is an error (exit code 2), except that it can only happen in
//! the modes a person starts by hand.

use std::ffi::OsString;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Simulation {
    CrashLoop,
    Escape,
    UserExit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    Shell,
    Beside(u32),
    Check,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Args {
    pub mode: Mode,
    pub panic: bool,
    pub dry_run: bool,
    pub simulate: Option<Simulation>,
    /// Supervise this program instead of the real host, against an in-memory registry.
    pub test_shell: Option<PathBuf>,
    /// What `--test-shell` starts where the real guard would start Explorer.
    pub test_explorer: Option<PathBuf>,
    /// `--test-shell` arms the real escape chord only when asked.
    pub test_hotkey: bool,
    pub state_dir: Option<PathBuf>,
    pub host: Option<PathBuf>,
    /// Arguments that were not understood.
    pub ignored: Vec<String>,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            mode: Mode::Shell,
            panic: false,
            dry_run: false,
            simulate: None,
            test_shell: None,
            test_explorer: None,
            test_hotkey: false,
            state_dir: None,
            host: None,
            ignored: Vec::new(),
        }
    }
}

pub fn parse<I: IntoIterator<Item = OsString>>(args: I) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = args.into_iter();
    let mut beside = None;
    let mut check = false;
    while let Some(raw) = it.next() {
        let s = raw.to_string_lossy().into_owned();
        let mut value = |flag: &str| -> Result<OsString, String> { it.next().ok_or_else(|| format!("{flag} needs a value")) };
        match s.as_str() {
            "--panic" => a.panic = true,
            "--dry-run" => a.dry_run = true,
            "--check" => check = true,
            "--test-hotkey" => a.test_hotkey = true,
            "--beside-guard" => {
                let v = value("--beside-guard")?;
                let pid: u32 = v.to_string_lossy().parse().map_err(|_| "--beside-guard needs a process id".to_string())?;
                if pid == 0 {
                    return Err("--beside-guard needs a process id above 0".into());
                }
                beside = Some(pid);
            }
            "--test-shell" => a.test_shell = Some(PathBuf::from(value("--test-shell")?)),
            "--test-explorer" => a.test_explorer = Some(PathBuf::from(value("--test-explorer")?)),
            "--state-dir" => a.state_dir = Some(PathBuf::from(value("--state-dir")?)),
            "--host" => a.host = Some(PathBuf::from(value("--host")?)),
            "--simulate" => {
                let v = value("--simulate")?;
                a.simulate = Some(match v.to_string_lossy().as_ref() {
                    "crash-loop" => Simulation::CrashLoop,
                    "escape" => Simulation::Escape,
                    "user-exit" => Simulation::UserExit,
                    other => return Err(format!("--simulate: unknown scenario {other:?} (crash-loop, escape, user-exit)")),
                });
            }
            _ => a.ignored.push(s),
        }
    }
    if let Some(pid) = beside {
        if a.panic || a.test_shell.is_some() {
            return Err("--beside-guard cannot be combined with --panic or --test-shell".into());
        }
        a.mode = Mode::Beside(pid);
    }
    if check {
        a.mode = Mode::Check;
    }
    if a.simulate.is_some() && !a.dry_run {
        return Err("--simulate only makes sense with --dry-run".into());
    }
    if a.test_explorer.is_some() && a.test_shell.is_none() {
        return Err("--test-explorer only makes sense with --test-shell".into());
    }
    if a.test_shell.is_some() && a.panic {
        return Err("--test-shell cannot be combined with --panic".into());
    }
    // Without a stand-in the fall-back of a test run would start the real explorer.exe.
    if a.test_shell.is_some() && a.test_explorer.is_none() && !a.dry_run {
        return Err("--test-shell needs --test-explorer <exe>: the program that stands in for Explorer (the real one is never started in a test run)".into());
    }
    Ok(a)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Result<Args, String> {
        parse(args.iter().map(OsString::from))
    }

    #[test]
    fn no_arguments_is_the_shell() {
        assert_eq!(p(&[]).unwrap(), Args::default());
    }

    #[test]
    fn panic_and_dry_run() {
        let a = p(&["--panic"]).unwrap();
        assert!(a.panic && !a.dry_run && a.mode == Mode::Shell);
        let a = p(&["--dry-run", "--panic", "--simulate", "escape"]).unwrap();
        assert!(a.panic && a.dry_run && a.simulate == Some(Simulation::Escape));
    }

    #[test]
    fn beside_guard_takes_a_pid() {
        assert_eq!(p(&["--beside-guard", "4321"]).unwrap().mode, Mode::Beside(4321));
        assert!(p(&["--beside-guard"]).is_err());
        assert!(p(&["--beside-guard", "abc"]).is_err());
        assert!(p(&["--beside-guard", "0"]).is_err());
        assert!(p(&["--beside-guard", "-5"]).is_err());
        assert!(p(&["--beside-guard", "5", "--panic"]).is_err());
        assert!(p(&["--beside-guard", "5", "--test-shell", "x.exe"]).is_err());
    }

    #[test]
    fn test_shell_modes() {
        let a = p(&["--test-shell", "fake.exe", "--test-explorer", "notepad.exe", "--test-hotkey", "--state-dir", "d"]).unwrap();
        assert_eq!(a.test_shell, Some(PathBuf::from("fake.exe")));
        assert_eq!(a.test_explorer, Some(PathBuf::from("notepad.exe")));
        assert!(a.test_hotkey);
        assert_eq!(a.state_dir, Some(PathBuf::from("d")));
        assert!(p(&["--test-shell"]).is_err());
        // A test run with no stand-in would start the real Explorer at the end: refused. A dry run starts nothing.
        assert!(p(&["--test-shell", "fake.exe"]).is_err());
        assert!(p(&["--test-shell", "fake.exe", "--dry-run"]).is_ok());
        assert!(p(&["--test-explorer", "x.exe"]).is_err());
        assert!(p(&["--test-shell", "x.exe", "--panic"]).is_err());
    }

    #[test]
    fn simulate_needs_dry_run_and_a_known_name() {
        assert!(p(&["--simulate", "crash-loop"]).is_err());
        assert!(p(&["--dry-run", "--simulate", "boom"]).is_err());
        assert!(p(&["--dry-run", "--simulate"]).is_err());
        assert_eq!(p(&["--dry-run", "--simulate", "user-exit"]).unwrap().simulate, Some(Simulation::UserExit));
        assert_eq!(p(&["--dry-run", "--simulate", "crash-loop"]).unwrap().simulate, Some(Simulation::CrashLoop));
    }

    #[test]
    fn unknown_arguments_are_kept_and_ignored_not_fatal() {
        let a = p(&["--shell", "--mode", "shell", "extra"]).unwrap();
        assert_eq!(a.ignored, vec!["--shell", "--mode", "shell", "extra"]);
        assert_eq!(a.mode, Mode::Shell);
    }

    #[test]
    fn check_wins_over_the_default_mode() {
        assert_eq!(p(&["--check"]).unwrap().mode, Mode::Check);
    }

    #[test]
    fn a_flag_swallowing_the_next_flag_is_a_value_not_a_flag() {
        // "--host --panic" means the host program is called "--panic": the user typed the value wrong, the panic flag is not set.
        let a = p(&["--host", "--panic"]).unwrap();
        assert_eq!(a.host, Some(PathBuf::from("--panic")));
        assert!(!a.panic);
    }
}
