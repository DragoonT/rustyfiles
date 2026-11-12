//! `rustyfiles --shell-probe`: a windowless, read-only report of what the shell host finds. It starts no window, sends no message,
//! writes nothing, and names programs, never windows' titles or anybody's files. The report is JSON so that a person and a test can
//! both read it.
//!
//! The gathering is split from the looking: [`Report::build`] takes what was found (the registry through [`RegRead`], the owners, the
//! monitors) and is tested on plain data; [`gather`] looks at the machine.

use std::path::Path;

use rf_core::desktop::registry::{Hive, RegRead};
use serde_json::{Value, json};

use super::mode::{Owner, ShellMode};
use crate::desktop::monitors::MonitorInfo;

/// The watchdog program that the per-user Shell value will point at one day; the host looks for it next to its own exe.
pub const GUARD_EXE: &str = "rustyfiles-shellguard.exe";

const WINLOGON: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon";
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";
const LOGON_POLICY: &str = r"SOFTWARE\Microsoft\PolicyManager\current\device\WindowsLogon";

/// What the report is made of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Report {
    pub mode: ShellMode,
    /// The `--mode` the host was started with, if any (what it was asked, as opposed to what it found).
    pub requested_mode: Option<String>,
    pub session: u32,
    /// `CurrentBuild.UBR`, e.g. `26300.9974`.
    pub build: Option<String>,
    pub shell_window: Option<Owner>,
    pub tray_window: Option<Owner>,
    /// The `Shell` values of Winlogon (read only): per user and per machine. Empty means not set.
    pub shell_value_user: Option<String>,
    pub shell_value_machine: Option<String>,
    /// `OverrideShellProgram` of the policy (an organisation's setting that beats the per-user value): `Some(1)` is set.
    pub override_shell_policy: Option<u32>,
    pub monitors: Vec<MonitorInfo>,
    /// The guard program is next to the exe.
    pub guard_next_to_exe: bool,
    /// Names of the settings of the shell track that are switched on.
    pub settings_on: Vec<String>,
}

/// What a caller hands over besides the machine.
#[derive(Clone, Debug, Default)]
pub struct Inputs {
    pub requested_mode: Option<String>,
    pub settings_on: Vec<String>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|v| !v.trim().is_empty())
}

/// The program of a shell value, without its folder: a value that points into a user's folder would carry the user's name into a
/// report that is read out loud. Arguments after the program are dropped too.
fn file_name_of(value: &str) -> String {
    let v = value.trim().trim_start_matches('"');
    let program = if let Some((quoted, _)) = value.trim().strip_prefix('"').and_then(|r| r.split_once('"')) { quoted } else { v.split_whitespace().next().unwrap_or(v) };
    program.rsplit(['\\', '/']).next().unwrap_or(program).to_string()
}

impl Report {
    /// Assembles the report from what was found. `guard_exists` answers "is a file there?" for a path (a fake in the tests).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        reg: &dyn RegRead,
        mode: ShellMode,
        session: u32,
        shell_window: Option<Owner>,
        tray_window: Option<Owner>,
        monitors: Vec<MonitorInfo>,
        exe_dir: Option<&Path>,
        guard_exists: &dyn Fn(&Path) -> bool,
        inputs: Inputs,
    ) -> Report {
        let build = reg.string(Hive::LocalMachine, CURRENT_VERSION, "CurrentBuild").map(|b| match reg.dword(Hive::LocalMachine, CURRENT_VERSION, "UBR") {
            Some(u) => format!("{b}.{u}"),
            None => b,
        });
        Report {
            mode,
            requested_mode: inputs.requested_mode,
            session,
            build,
            shell_window,
            tray_window,
            shell_value_user: nonempty(reg.string(Hive::CurrentUser, WINLOGON, "Shell")),
            shell_value_machine: nonempty(reg.string(Hive::LocalMachine, WINLOGON, "Shell")),
            override_shell_policy: reg.dword(Hive::LocalMachine, LOGON_POLICY, "OverrideShellProgram"),
            monitors,
            guard_next_to_exe: exe_dir.is_some_and(|d| guard_exists(&d.join(GUARD_EXE))),
            settings_on: inputs.settings_on,
        }
    }

    pub fn to_json(&self) -> Value {
        let owner = |o: &Option<Owner>| match o {
            None => Value::Null,
            // (program names and window classes only; never a path, which holds the user's name)
            Some(o) => json!({ "class": o.class, "program": o.program, "own_process": o.own_process }),
        };
        let rect = |l: i32, t: i32, r: i32, b: i32| json!({ "left": l, "top": t, "right": r, "bottom": b, "width": r - l, "height": b - t });
        json!({
            "mode": self.mode.name(),
            "requested_mode": self.requested_mode,
            "session": self.session,
            "build": self.build,
            "shell_window": owner(&self.shell_window),
            "tray_window": owner(&self.tray_window),
            "winlogon_shell": { "user": self.shell_value_user.as_deref().map(file_name_of), "machine": self.shell_value_machine.as_deref().map(file_name_of), "policy_override": self.override_shell_policy },
            "monitors": self.monitors.iter().enumerate().map(|(i, m)| json!({
                "index": i + 1,
                "primary": m.primary,
                "rect": rect(m.rect.left, m.rect.top, m.rect.right, m.rect.bottom),
                "work_area": rect(m.work.left, m.work.top, m.work.right, m.work.bottom),
                "dpi": m.dpi,
                "scale_percent": m.scale_percent,
            })).collect::<Vec<_>>(),
            "guard": { "program": GUARD_EXE, "next_to_exe": self.guard_next_to_exe },
            "settings_on": self.settings_on,
        })
    }

    pub fn to_pretty_string(&self) -> String {
        serde_json::to_string_pretty(&self.to_json()).unwrap_or_default()
    }
}

/// Looks at the machine. Read-only; makes the process per-monitor aware first so that the monitors come back in physical pixels.
pub fn gather(inputs: Inputs) -> Report {
    crate::desktop::monitors::make_process_per_monitor_aware();
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let shell_window = super::mode::shell_window_owner();
    let mode = super::mode::decide(shell_window.as_ref(), &crate::startup::system::windows_dir());
    Report::build(
        &crate::desktop::registry::SystemRegistry,
        mode,
        crate::single_instance::session_id(),
        shell_window,
        super::mode::tray_window_owner(),
        crate::desktop::monitors::list(),
        exe_dir.as_deref(),
        &|p| p.is_file(),
        inputs,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::geom::Rect;
    use rf_core::desktop::registry::MemRegistry;

    fn monitor(primary: bool, rect: Rect) -> MonitorInfo {
        MonitorInfo { device_key: "SECRET-DEVICE-KEY".into(), adapter_name: r"\\.\DISPLAY1".into(), rect, work: Rect::new(rect.left, rect.top, rect.right, rect.bottom - 48), primary, dpi: 96, scale_percent: 100 }
    }
    fn explorer() -> Owner {
        Owner { class: "Progman".into(), program: "explorer.exe".into(), path: Some(r"C:\Windows\explorer.exe".into()), pid: 4242, own_process: false }
    }
    fn tray() -> Owner {
        Owner { class: "Shell_TrayWnd".into(), ..explorer() }
    }
    fn reg() -> MemRegistry {
        MemRegistry::new()
            .with_string(Hive::LocalMachine, CURRENT_VERSION, "CurrentBuild", "26300")
            .with_dword(Hive::LocalMachine, CURRENT_VERSION, "UBR", 9974)
            .with_string(Hive::LocalMachine, WINLOGON, "Shell", "explorer.exe")
    }

    fn build(reg: &MemRegistry, guard: bool, inputs: Inputs) -> Report {
        Report::build(
            reg,
            ShellMode::Beside,
            1,
            Some(explorer()),
            Some(tray()),
            vec![monitor(true, Rect::new(0, 0, 2560, 1440)), monitor(false, Rect::new(2560, -618, 4000, 1942))],
            Some(Path::new(r"C:\Users\someone\app")),
            &|p| guard && p.ends_with(GUARD_EXE),
            inputs,
        )
    }

    #[test]
    fn the_report_says_what_was_found() {
        let r = build(&reg(), true, Inputs { requested_mode: Some("shell".into()), settings_on: vec!["shell_desktop_cover".into()] });
        assert_eq!(r.build.as_deref(), Some("26300.9974"));
        assert_eq!(r.shell_value_user, None, "not set for the user");
        assert_eq!(r.shell_value_machine.as_deref(), Some("explorer.exe"));
        assert_eq!(r.override_shell_policy, None);
        assert!(r.guard_next_to_exe);
        let j = r.to_json();
        assert_eq!(j["mode"], "beside");
        assert_eq!(j["requested_mode"], "shell");
        assert_eq!(j["shell_window"]["program"], "explorer.exe");
        assert_eq!(j["shell_window"]["class"], "Progman");
        assert_eq!(j["tray_window"]["class"], "Shell_TrayWnd");
        assert_eq!(j["monitors"].as_array().unwrap().len(), 2);
        assert_eq!(j["monitors"][1]["rect"]["left"], 2560);
        assert_eq!(j["monitors"][1]["rect"]["top"], -618);
        assert_eq!(j["monitors"][1]["work_area"]["height"], 2560 - 48);
        assert_eq!(j["guard"]["next_to_exe"], true);
        assert_eq!(j["settings_on"][0], "shell_desktop_cover");
    }

    #[test]
    fn nothing_that_belongs_to_the_user_leaks_into_the_report() {
        let text = build(&reg(), false, Inputs::default()).to_pretty_string();
        assert!(!text.contains("someone"), "no path with a user name");
        assert!(!text.contains("SECRET-DEVICE-KEY"), "no device identifiers");
        assert!(!text.contains("C:\\\\Windows"), "no paths at all");
        assert!(!text.contains("4242"), "no process ids");
    }

    #[test]
    fn a_missing_guard_a_missing_shell_and_an_unset_policy_are_reported_as_such() {
        let r = Report::build(&MemRegistry::new(), ShellMode::Shell, 2, None, None, vec![], None, &|_| true, Inputs::default());
        assert!(!r.guard_next_to_exe, "no exe directory, no guard");
        let j = r.to_json();
        assert!(j["shell_window"].is_null() && j["tray_window"].is_null());
        assert!(j["build"].is_null());
        assert_eq!(j["mode"], "shell");
        assert_eq!(j["monitors"].as_array().unwrap().len(), 0);
        let r = build(&reg(), false, Inputs::default());
        assert!(!r.guard_next_to_exe);
    }

    #[test]
    fn an_organisations_shell_policy_and_a_users_shell_value_are_both_shown() {
        let r = MemRegistry::new()
            .with_string(Hive::CurrentUser, WINLOGON, "Shell", r"C:\x\rustyfiles-shellguard.exe")
            .with_dword(Hive::LocalMachine, LOGON_POLICY, "OverrideShellProgram", 1);
        let rep = build(&r, true, Inputs::default());
        assert_eq!(rep.override_shell_policy, Some(1));
        assert!(rep.shell_value_user.is_some());
        // the report keeps the value as read; what is PRINTED names the program only (the folder may hold a user's name)
        let j = rep.to_json();
        assert_eq!(j["winlogon_shell"]["policy_override"], 1);
        assert_eq!(j["winlogon_shell"]["user"], "rustyfiles-shellguard.exe");
    }

    #[test]
    fn a_shell_value_is_printed_as_its_program_name() {
        assert_eq!(file_name_of("explorer.exe"), "explorer.exe");
        assert_eq!(file_name_of(r"C:\Users\Ada\AppData\Local\rustyfiles\shell\guard.exe"), "guard.exe");
        assert_eq!(file_name_of(r#""C:\Program Files\x\guard.exe" --flag"#), "guard.exe");
        assert_eq!(file_name_of(r"C:\Users\Ada\guard.exe --flag"), "guard.exe");
        assert_eq!(file_name_of(""), "");
    }

    #[test]
    fn an_empty_shell_value_counts_as_not_set() {
        let r = MemRegistry::new().with_string(Hive::CurrentUser, WINLOGON, "Shell", "  ");
        assert_eq!(build(&r, false, Inputs::default()).shell_value_user, None);
    }

    /// Windowless and read-only: the real gathering runs here.
    #[test]
    fn the_real_report_can_be_made_and_printed() {
        let r = gather(Inputs::default());
        assert!(!r.monitors.is_empty());
        let text = r.to_pretty_string();
        assert!(text.contains("\"mode\""));
        assert!(r.build.is_some());
    }
}
