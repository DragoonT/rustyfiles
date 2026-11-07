//! Installing and updating through Velopack (see notes/design/2026-11-08-install-update-design.md):
//! the Setup.exe on GitHub Releases installs per user, updates come from the same Releases.
//! A copy that wasn't installed by Velopack (a dev build, scripts/install.ps1) runs without
//! updates.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackApp};

pub const REPO: &str = "https://github.com/gelenmedya/rustyfiles";

/// First thing in `main`: Velopack's install / update / uninstall hooks run here and exit.
pub fn startup() {
    VelopackApp::build()
        .on_after_install_fast_callback(|_| after_install_or_update())
        .on_after_update_fast_callback(|_| after_install_or_update())
        .on_before_uninstall_fast_callback(|_| before_uninstall())
        .run();
}

/// Registrations that are on follow this install: the default file manager, DirectLook's start
/// with Windows, and the Open/Save dialog DLL (a per-build copy).
fn after_install_or_update() {
    let Ok(me) = std::env::current_exe() else { return };
    if rf_win::default_fm::is_enabled(rf_win::default_fm::CLASSES) {
        let _ = rf_win::default_fm::enable(&me.display().to_string(), rf_win::default_fm::CLASSES);
    }
    if rf_win::dialogs::starts_with_windows_as("DirectLook") {
        let _ = rf_win::dialogs::set_start_with_windows_as("DirectLook", &me.with_file_name("directlook.exe"), &["--background"], true);
    }
    if rf_win::dialog_reg::is_enabled(rf_win::dialog_reg::CLASSES) {
        if let Some(dll) = crate::dialog_dll() {
            rf_win::dialog_reg::point_to(&dll, rf_win::dialog_reg::CLASSES);
        }
    }
}

/// Everything rustyfiles changed in Windows goes back (settings in %APPDATA% stay).
fn before_uninstall() {
    let classes = rf_win::default_fm::CLASSES;
    if rf_win::default_fm::is_enabled(classes) {
        let _ = rf_win::default_fm::disable(classes);
    }
    let _ = rf_win::dialog_reg::disable(rf_win::dialog_reg::CLASSES);
    let none = Path::new("");
    let _ = rf_win::dialogs::set_start_with_windows_as("DirectLook", none, &[], false);
    let _ = rf_win::dialogs::set_start_with_windows(none, false);
    // DirectLook holds its exe open
    rf_win::single_instance::send_to(&crate::look::ipc::app_name(), &crate::look::ipc::Msg::Quit.encode());
    std::thread::sleep(Duration::from_millis(500));
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not installed by Velopack: no updates.
    NotInstalled,
    Idle,
    Checking,
    UpToDate,
    Downloading(String),
    /// Downloaded: applied on restart (or at the next start anyway).
    Ready(String),
    Failed(String),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static PENDING: Mutex<Option<UpdateInfo>> = Mutex::new(None);

pub fn state() -> State {
    STATE.lock().unwrap().clone().unwrap_or(State::Idle)
}

fn set(s: State) {
    *STATE.lock().unwrap() = Some(s);
}

fn manager() -> Option<UpdateManager> {
    UpdateManager::new(GithubSource::new(REPO, None, false), None, None).ok()
}

/// This build's version (Velopack's when installed, else the crate's). Read once.
pub fn version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| manager().map(|m| m.get_current_version_as_string()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())).clone()
}

/// Looks for an update and downloads it. Blocking: call off the UI thread.
pub fn check() {
    let Some(um) = manager() else {
        set(State::NotInstalled);
        return;
    };
    if let Some(ready) = um.get_update_pending_restart() {
        set(State::Ready(ready.Version));
        return;
    }
    set(State::Checking);
    match um.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(info)) => {
            let v = info.TargetFullRelease.Version.clone();
            set(State::Downloading(v.clone()));
            match um.download_updates(&info, None) {
                Ok(()) => {
                    *PENDING.lock().unwrap() = Some(*info);
                    set(State::Ready(v));
                }
                Err(e) => set(State::Failed(e.to_string())),
            }
        }
        Ok(_) => set(State::UpToDate),
        Err(e) => set(State::Failed(e.to_string())),
    }
}

/// Starts the background checks: soon after start, then every 12 hours (when auto-download is
/// on; the Check button works either way). Calls `changed` on the UI thread after each.
pub fn spawn_checks(cx: &mut gpui_kit::App, changed: impl Fn(&mut gpui_kit::App) + 'static) {
    if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some() {
        return; // test copies
    }
    cx.spawn(async move |cx| {
        let mut wait = Duration::from_secs(30);
        loop {
            cx.background_executor().timer(wait).await;
            wait = Duration::from_secs(12 * 3600);
            let auto = cx.update(|cx| cx.global::<crate::settings::Settings>().auto_update);
            if !auto {
                continue;
            }
            cx.background_executor().spawn(async { check() }).await;
            cx.update(|cx| changed(cx));
        }
    })
    .detach();
}

/// Restarts into the downloaded update. DirectLook is asked to quit first (its exe is replaced).
pub fn restart_to_update() {
    let Some(um) = manager() else { return };
    let pending = PENDING.lock().unwrap().clone();
    rf_win::single_instance::send_to(&crate::look::ipc::app_name(), &crate::look::ipc::Msg::Quit.encode());
    std::thread::sleep(Duration::from_millis(300));
    let r = match pending {
        Some(info) => um.apply_updates_and_restart(&info),
        None => match um.get_update_pending_restart() {
            Some(asset) => um.apply_updates_and_restart(&asset),
            None => return,
        },
    };
    if let Err(e) = r {
        set(State::Failed(e.to_string()));
    }
}

/// The one-time "star it on GitHub" strip: after a week and 30 starts, until answered.
pub fn star_due(s: &crate::settings::Settings) -> bool {
    let now = chrono::Utc::now().timestamp_millis();
    !s.star_asked && s.first_start_ms > 0 && s.starts >= 30 && now - s.first_start_ms >= 7 * 24 * 3600 * 1000
}

/// Counted at each start of the app (not for test copies or a dialog-only start).
pub fn count_start(cx: &mut gpui_kit::App) {
    if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some() {
        return;
    }
    let now = chrono::Utc::now().timestamp_millis();
    crate::settings::update(cx, |s| {
        if s.first_start_ms == 0 {
            s.first_start_ms = now;
        }
        s.starts = s.starts.saturating_add(1);
    });
}

#[cfg(test)]
mod tests {
    use super::star_due;

    #[test]
    fn star_strip_waits_a_week_and_30_starts() {
        let now = chrono::Utc::now().timestamp_millis();
        let week = 7 * 24 * 3600 * 1000;
        let mut s = crate::settings::Settings::default();
        assert!(!star_due(&s)); // never started
        s.first_start_ms = now - week - 1;
        s.starts = 29;
        assert!(!star_due(&s));
        s.starts = 30;
        assert!(star_due(&s));
        s.first_start_ms = now - week + 60_000;
        assert!(!star_due(&s)); // not a week yet
        s.first_start_ms = now - week - 1;
        s.star_asked = true;
        assert!(!star_due(&s));
    }
}
