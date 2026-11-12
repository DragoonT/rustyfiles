//! Our own shell, as far as it exists (shell plan `notes/design/2026-11-12-kabuk-plan.md`).
//!
//! * `rustyfiles.exe --shell` is the SHELL HOST (increment A1): today it is the dock's process under another name (`--dock` stays as
//!   an alias and does exactly what it did), one process for the islands, the panels and, when asked, the desktop. It knows in which
//!   [`ShellMode`] it runs - [`ShellMode::Beside`] when Explorer owns the shell window, [`ShellMode::Shell`] otherwise - and in this
//!   increment only reports it. `rustyfiles.exe --shell-probe` prints what the host finds, windowless and read-only ([`probe`]).
//! * The desktop view (`desktop/`, increment F3) draws the desktop in an ordinary window (`--desktop-preview`).
//! * Cover mode (`cover`, increment F4): with the setting `shell_desktop_cover` on, the host puts that same view over Explorer's
//!   desktop, one borderless window per monitor.
//! * The window switcher (`switcher`, increment G1): our own Alt+Tab panel, behind `--switcher-preview`, and (increment B7) from
//!   Alt+Tab itself when the setting `shell_own_switcher` is on.
//! * Hiding Explorer's taskbar (`taskbar_hide`, increment E1): with `shell_hide_taskbar` on, the host hides Explorer's taskbar windows
//!   while the guard (`rustyfiles-shellguard.exe`) is confirmed running; docs/design/taskbar-hide.md.
//! * The keyboard layer (`keylayer`, increment B7): with `shell_win_key` and/or `shell_own_switcher` on, the host listens to the
//!   keyboard with one system-wide hook, looks only at the Windows key and Alt+Tab, and opens OUR Start panel and switcher.
//! * The tray host (`trayhost`, increment D2): with `shell_tray_host` on, the host owns a `Shell_TrayWnd` window in front of Explorer's,
//!   keeps a model of the icons programs send to it, and passes every message on to Explorer's window untouched.
//! * While it runs (`live`, `exits`): the host notices that the shell window came or went (Explorer closed, restarted, replaced), tells the
//!   desktop cover, the media-key window and the work-area keeper, and every way the process ends writes `shell host exits: <reason>`.
//!   docs/design/shell-host.md, "While it runs".

pub mod cover;
pub mod desktop;
pub mod exits;
pub mod keylayer;
pub mod live;
pub mod probe;
pub mod switcher;
pub mod trayhost;
pub mod taskbar_hide;

use std::sync::OnceLock;

pub use rf_win::shellhost::ShellMode;

/// The command-line argument that makes this process the shell host (the same process as `--dock`).
pub const ARG: &str = "--shell";

/// Whether the first command-line word makes the process the dock / shell host.
pub fn is_host_arg(first: Option<&str>) -> bool {
    matches!(first, Some(crate::dock::ARG) | Some(ARG))
}

/// What the shell host found out about itself at its start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostInfo {
    /// What the look at the shell window said.
    pub mode: ShellMode,
    /// The `--mode` the process was started with, if any: what it was ASKED, which decides nothing.
    pub requested: Option<ShellMode>,
}

static HOST: OnceLock<HostInfo> = OnceLock::new();

/// `--mode beside|shell` in a command line (after the program name), if there is one that is understood.
pub fn requested_mode(args: &[String]) -> Option<ShellMode> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if let Some(v) = a.strip_prefix("--mode=") {
            return ShellMode::parse(v);
        }
        if a == "--mode" {
            return it.next().and_then(|v| ShellMode::parse(v));
        }
    }
    None
}

/// Called once by the host at its start: looks at the shell window (read-only), logs what it found and remembers it.
pub fn note_host_start(args: &[String]) -> HostInfo {
    let info = HostInfo { mode: rf_win::shellhost::mode::detect(), requested: requested_mode(args) };
    if info.requested.is_some_and(|r| r != info.mode) {
        // (the guard says "shell" when it starts us; if Explorer owns the shell window all the same, the look wins: see rf_win::shellhost::mode)
        log::warn!("shell host: asked to run as {:?}, found {:?}", info.requested.map(ShellMode::name), info.mode.name());
    }
    log::info!("shell host: mode {}", info.mode.name());
    live::init(info.mode);
    HOST.get_or_init(|| info).clone()
}

/// The mode the host runs in NOW: what it found at its start, kept up to date while it runs (`None` before it has started, and in
/// processes that are not the host).
pub fn mode() -> Option<ShellMode> {
    live::current().or_else(|| HOST.get().map(|h| h.mode))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn both_words_make_the_host_and_nothing_else_does() {
        assert!(is_host_arg(Some("--dock")));
        assert!(is_host_arg(Some("--shell")));
        assert!(!is_host_arg(Some("--shell-probe")));
        assert!(!is_host_arg(Some("--desktop-preview")));
        assert!(!is_host_arg(Some(r"C:\Users")));
        assert!(!is_host_arg(None));
    }

    #[test]
    fn the_requested_mode_is_read_from_the_command_line() {
        assert_eq!(requested_mode(&args(&["--shell", "--mode", "shell"])), Some(ShellMode::Shell));
        assert_eq!(requested_mode(&args(&["--shell", "--mode=beside"])), Some(ShellMode::Beside));
        assert_eq!(requested_mode(&args(&["--shell"])), None);
        assert_eq!(requested_mode(&args(&["--shell", "--mode", "both"])), None);
        assert_eq!(requested_mode(&args(&["--shell", "--mode"])), None);
    }
}
