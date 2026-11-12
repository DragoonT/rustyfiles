//! `rustyfiles.exe --shell-probe`: what the shell host finds, printed as JSON. Windowless and read-only: it opens no window, starts no
//! process, sends no message, writes nothing (not even a log line to the settings folder), and reads the settings file without
//! touching it. It runs before the app starts, so it is safe to run at any time, Explorer or not.
//!
//! A release build of rustyfiles.exe has no console of its own: run it as `rustyfiles.exe --shell-probe > probe.json` (or pipe it)
//! to see the report. A debug build prints to its console.

use std::io::Write as _;

use rf_win::shellhost::probe::{self, Inputs};

use crate::settings::Settings;

pub const ARG: &str = "--shell-probe";

/// Names of the shell host's settings that are switched on (the dock's and this track's), for the report.
pub fn settings_on(s: &Settings) -> Vec<String> {
    [
        ("dock_enabled", s.dock_enabled),
        ("dock_reserve", s.dock_reserve),
        ("dock_own_start", s.dock_own_start),
        ("dock_own_control_centre", s.dock_own_control_centre),
        ("shell_desktop_cover", s.shell_desktop_cover),
        ("shell_win_key", s.shell_win_key),
        ("shell_own_switcher", s.shell_own_switcher),
    ]
    .into_iter()
    .filter(|(_, on)| *on)
    .map(|(name, _)| name.to_string())
    .collect()
}

/// The report as text.
pub fn report(args: &[String], settings: &Settings) -> String {
    let requested_mode = super::requested_mode(args).map(|m| m.name().to_string());
    probe::gather(Inputs { requested_mode, settings_on: settings_on(settings) }).to_pretty_string()
}

/// Runs the probe and returns the process's exit code.
pub fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // (the settings file is read, never written; a file that cannot be read is reported as "defaults")
    let settings = Settings::load_readonly().unwrap_or_default();
    let text = report(&args, &settings);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{text}");
    let _ = out.flush();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settings_that_are_on_are_named_and_the_experimental_one_is_not_by_default() {
        let on = settings_on(&Settings::default());
        assert!(!on.contains(&"shell_desktop_cover".to_string()), "off by default");
        assert!(!on.contains(&"dock_enabled".to_string()), "the dock is off by default too");
        assert!(on.contains(&"dock_reserve".to_string()));
        let all = settings_on(&Settings { shell_desktop_cover: true, dock_enabled: true, ..Default::default() });
        assert!(all.contains(&"shell_desktop_cover".to_string()) && all.contains(&"dock_enabled".to_string()));
    }

    #[test]
    fn the_report_is_json_and_carries_what_was_asked() {
        let text = report(&["--shell-probe".to_string(), "--mode".to_string(), "shell".to_string()], &Settings { shell_desktop_cover: true, ..Default::default() });
        let j: serde_json::Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(j["requested_mode"], "shell");
        assert!(j["mode"] == "beside" || j["mode"] == "shell");
        assert!(j["settings_on"].as_array().unwrap().iter().any(|v| v == "shell_desktop_cover"));
        assert!(j["monitors"].as_array().is_some_and(|m| !m.is_empty()));
        assert!(j["guard"]["program"].as_str().is_some_and(|p| p.ends_with(".exe")));
    }
}
