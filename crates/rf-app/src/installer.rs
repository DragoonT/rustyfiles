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
/// Where updates come from. TEMPORARY: the pmrochen fork, whose releases carry the ARM64 build (win-arm64 channel); back to REPO
/// once that is upstream.
const UPDATE_REPO: &str = "https://github.com/pmrochen/rustyfiles";
/// AppUserModelID of the installed app (Velopack names shortcuts "velopack.<pack id>").
pub const APP_ID: &str = "velopack.rustyfiles";

/// Whether a process started with this first argument may apply a waiting update as it starts. Velopack does that before any code
/// of ours runs, and it ends every running rustyfiles process to do it.
///
/// WHY NOT EVERY PROCESS (the updater's log on the user's PC, 2026-11-14: an update from 0.7.0 to 0.7.2 was applied by a process
/// started with `--pick`, that is by another program's Open dialog; the dock, the picker, the quick search and the main app in the
/// background are all started without the user asking for rustyfiles). Only a start the user means as "open rustyfiles" applies it:
/// no argument, a folder or file to open, or a Settings page. Everything that begins with `--` otherwise is a side process (and
/// the updater's own hooks, which never apply anything). The update is not lost: the next such start applies it, and so does the
/// button in Settings and in the status bar.
pub fn applies_update_at_start(first_arg: Option<&str>) -> bool {
    match first_arg {
        None => true,
        Some(a) if a.starts_with("--settings=") => true,
        Some(a) => !a.starts_with("--"),
    }
}

/// First thing in `main`: Velopack's install / update / uninstall hooks run here and exit.
pub fn startup() {
    let first = std::env::args().nth(1);
    VelopackApp::build()
        .set_auto_apply_on_startup(applies_update_at_start(first.as_deref()))
        .on_after_install_fast_callback(|_| {
            after_install_or_update();
            dialogs_on_by_default();
        })
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
    let _ = crate::dialog_registration(crate::DialogReg::Follow);
}

/// A fresh install turns on rustyfiles' Open/Save dialogs (per user, no admin rights; Settings →
/// Advanced turns them off). Updates don't, so someone who turned them off keeps them off.
fn dialogs_on_by_default() {
    if !rf_win::dialog_reg::is_enabled(rf_win::dialog_reg::CLASSES) {
        let _ = crate::dialog_registration(crate::DialogReg::Enable);
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
    // and so does a running dock (a process of its own that outlives rustyfiles' window)
    crate::dock::control::stop_for_update();
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    /// Not installed by Velopack: no updates.
    NotInstalled,
    Idle,
    Checking,
    UpToDate,
    /// Version and percent downloaded.
    Downloading(String, u8),
    /// Downloaded: applied on restart (or at the next start anyway).
    Ready(String),
    Failed(String),
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// The downloaded version, when a release between this one and it is marked important (see [`IMPORTANT_MARK`]).
static IMPORTANT: Mutex<Option<String>> = Mutex::new(None);

/// The mark that makes a release an important one: a line in its release notes on GitHub, an HTML comment, so nobody reading the
/// notes sees it. WHY (the owner, 2026-11-17, the morning 0.8.6 left the dock empty: "güncelleme denetimine de önemli fix modu
/// ekleyelim. bunun gibi önemli fixleri yayınlayınca önemli bir güncelleme olduğunu belirtsin uygulamada"): an update was always
/// announced the same quiet way, "ready, restart", which a fix for a broken dock should not be.
pub const IMPORTANT_MARK: &str = "<!-- rustyfiles: important -->";

/// Whether the downloaded `version` brings an important fix.
pub fn is_important(version: &str) -> bool {
    IMPORTANT.lock().unwrap_or_else(|p| p.into_inner()).as_deref() == Some(version)
}

/// "0.8.7", "v0.8.7", "0.8.7-beta.1" as numbers (a pre-release counts as its version).
fn version_parts(s: &str) -> Option<(u64, u64, u64)> {
    let core = s.trim().trim_start_matches('v').split(['-', '+']).next()?;
    let mut it = core.split('.').map(|x| x.parse::<u64>().ok());
    Some((it.next()??, it.next()??, it.next()??))
}

/// Whether one of `releases` (tag, notes) newer than `current` and not newer than `target` carries [`IMPORTANT_MARK`]: a copy that
/// skipped several releases is told when any of them was important.
fn any_important(releases: &[(String, String)], current: &str, target: &str) -> bool {
    let (Some(cur), Some(tgt)) = (version_parts(current), version_parts(target)) else { return false };
    releases.iter().any(|(tag, notes)| version_parts(tag).is_some_and(|v| v > cur && v <= tgt) && notes.contains(IMPORTANT_MARK))
}

/// Reads the latest releases' notes from GitHub and remembers whether the update to `target` is an important one. One request, only
/// when an update was downloaded; on any failure the update is an ordinary one. Blocking.
fn learn_importance(target: &str) {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(8))).http_status_as_error(false).build().into();
    let api = UPDATE_REPO.replace("https://github.com/", "https://api.github.com/repos/") + "/releases?per_page=15";
    let releases: Vec<(String, String)> = agent
        .get(api)
        .header("User-Agent", format!("rustyfiles/{}", version()))
        .header("Accept", "application/vnd.github+json")
        .call()
        .ok()
        .filter(|r| r.status() == 200)
        .and_then(|mut r| r.body_mut().read_to_string().ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .into_iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false))
        .filter_map(|r| Some((r["tag_name"].as_str()?.to_string(), r["body"].as_str().unwrap_or("").to_string())))
        .collect();
    let important = any_important(&releases, &version(), target);
    if important {
        log::info!("update: {target} is an important update");
    }
    *IMPORTANT.lock().unwrap_or_else(|p| p.into_inner()) = important.then(|| target.to_string());
}
static PENDING: Mutex<Option<UpdateInfo>> = Mutex::new(None);

pub fn state() -> State {
    STATE.lock().unwrap().clone().unwrap_or(State::Idle)
}

fn set(s: State) {
    if matches!(s, State::Ready(_)) && !matches!(state(), State::Ready(_)) {
        *READY_SINCE.lock().unwrap() = Some(std::time::Instant::now());
    }
    *STATE.lock().unwrap() = Some(s);
}

static READY_SINCE: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);

/// When the downloaded update became ready (the Status Center button pulses for a few seconds).
pub fn ready_since() -> Option<std::time::Instant> {
    *READY_SINCE.lock().unwrap()
}

/// A check or download is running (the About page redraws while this is true).
pub fn busy() -> bool {
    matches!(state(), State::Checking | State::Downloading(..))
}

/// `RUSTYFILES_FAKE_UPDATE=1` (test copies): check, download 0→100 % and "ready" without a network,
/// to see the About page's states.
fn fake_check() {
    use std::thread::sleep;
    set(State::Checking);
    sleep(Duration::from_millis(1500));
    for pct in (0..=100).step_by(4) {
        set(State::Downloading("9.9.9".into(), pct));
        sleep(Duration::from_millis(120));
    }
    // (`RUSTYFILES_FAKE_UPDATE=important`: the look of an important one)
    if std::env::var("RUSTYFILES_FAKE_UPDATE").is_ok_and(|v| v.trim() == "important") {
        *IMPORTANT.lock().unwrap_or_else(|p| p.into_inner()) = Some("9.9.9".into());
    }
    set(State::Ready("9.9.9".into()));
}

/// The beta switch of the settings (About page), as the update checks read it: they run off the UI thread and cannot ask the
/// settings themselves. Set at start and before every check.
static BETAS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_beta_updates(on: bool) {
    BETAS.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether a version is a beta: it has a pre-release part ("0.7.0-beta.1"; build metadata after a plus does not count). Pure.
pub fn is_beta(version: &str) -> bool {
    version.split('+').next().unwrap_or("").contains('-')
}

/// Whether a copy looks at GitHub's PRE-RELEASES too. A beta is published as a pre-release, which the update source leaves out
/// unless it is asked for them, so a copy on an ordinary version with the switch off never sees one and never downloads one. The
/// switch asks for them; so does a copy that is a beta itself, or whoever installed a beta by hand would stay on it with no way on
/// until the release. Pure.
pub fn takes_betas(switch_on: bool, this_version: &str) -> bool {
    switch_on || is_beta(this_version)
}

fn manager() -> Option<UpdateManager> {
    let betas = takes_betas(BETAS.load(std::sync::atomic::Ordering::Relaxed), env!("CARGO_PKG_VERSION"));
    UpdateManager::new(GithubSource::new(UPDATE_REPO, None, betas), None, None).ok()
}

/// This build's version (Velopack's when installed, else the crate's). Read once.
/// "0.1.0 · b241ace · 08.11.2026 02:40": the version, the commit and when it was built.
pub fn version_full() -> String {
    let built = env!("RUSTYFILES_BUILT").parse::<i64>().ok().and_then(|s| chrono::DateTime::from_timestamp(s, 0));
    let when = built.map(|t| t.with_timezone(&chrono::Local).format("%d.%m.%Y %H:%M").to_string()).unwrap_or_default();
    format!("{} · {} · {when}", version(), env!("RUSTYFILES_COMMIT"))
}

pub fn version() -> String {
    static V: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    V.get_or_init(|| manager().map(|m| m.get_current_version_as_string()).unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())).clone()
}

/// Whether the difference packages between the installed version and the newest one are less to download than the full package.
/// Pure.
///
/// WHY WE DECIDE IT: Velopack does not (read in 1.2.161 and 1.3.39: `MaximumDeltasBeforeFallback` is set and never used). It chains
/// EVERY delta the feed lists between the two versions and turns to the full package only when patching fails. A copy updates
/// straight to the newest version, so one that is several releases behind got one delta per release it skipped. Measured on our
/// releases up to 0.8.0: a delta is about 10 MB whatever the release changed, the full package 39 MB, so three releases behind was
/// 31 MB of deltas, four 41 MB (more than the full package), seven 79 MB. And the feed lists only the ten newest releases: a copy
/// older than those would download the whole chain, fail to patch (the first delta is not made from its version) and then download
/// the full package as well. With this rule nobody downloads more than the full package, and nobody more than before.
pub fn deltas_are_the_smaller_download(deltas: &[u64], full: u64) -> bool {
    let sum = deltas.iter().fold(0u64, |all, one| all.saturating_add(*one));
    !deltas.is_empty() && sum < full
}

/// Looks for an update and downloads it. Blocking: call off the UI thread.
pub fn check() {
    if std::env::var_os("RUSTYFILES_FAKE_UPDATE").is_some() {
        return fake_check();
    }
    let Some(um) = manager() else {
        set(State::NotInstalled);
        return;
    };
    if let Some(ready) = um.get_update_pending_restart() {
        learn_importance(&ready.Version);
        set(State::Ready(ready.Version));
        return;
    }
    set(State::Checking);
    match um.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(mut info)) => {
            let deltas: Vec<u64> = info.DeltasToTarget.iter().map(|a| a.Size).collect();
            if !deltas.is_empty() && !deltas_are_the_smaller_download(&deltas, info.TargetFullRelease.Size) {
                log::info!("update: {} deltas ({:?} bytes) against a full package of {} bytes: taking the full one", deltas.len(), deltas, info.TargetFullRelease.Size);
                info.DeltasToTarget.clear();
                info.BaseRelease = None;
            }
            let v = info.TargetFullRelease.Version.clone();
            set(State::Downloading(v.clone(), 0));
            // Velopack reports 0–100 on a channel; mirror it into the state while it downloads
            let (tx, rx) = std::sync::mpsc::channel::<i16>();
            let pv = v.clone();
            let progress = std::thread::spawn(move || {
                for pct in rx {
                    set(State::Downloading(pv.clone(), pct.clamp(0, 100) as u8));
                }
            });
            let result = um.download_updates(&info, Some(tx));
            let _ = progress.join();
            match result {
                Ok(()) => {
                    *PENDING.lock().unwrap() = Some(*info);
                    learn_importance(&v);
                    set(State::Ready(v));
                }
                Err(e) => set(State::Failed(e.to_string())),
            }
        }
        Ok(_) => set(State::UpToDate),
        Err(e) => set(State::Failed(e.to_string())),
    }
}

/// Starts the background checks: soon after start, then every 2 hours (when auto-download is
/// on; the Check button works either way). Calls `changed` on the UI thread after each.
pub fn spawn_checks(cx: &mut gpui_kit::App, changed: impl Fn(&mut gpui_kit::App) + 'static) {
    // test copies don't look for updates, unless they fake one (RUSTYFILES_FAKE_UPDATE)
    let fake = std::env::var_os("RUSTYFILES_FAKE_UPDATE").is_some();
    if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some() && !fake {
        return;
    }
    cx.spawn(async move |cx| {
        let mut wait = Duration::from_secs(if fake { 2 } else { 30 });
        loop {
            cx.background_executor().timer(wait).await;
            // a file manager stays open for days: a long gap meant releases waited a day to arrive
            // (GitHub allows 60 unauthenticated calls an hour; this is one call every two hours)
            wait = Duration::from_secs(2 * 3600);
            let (auto, betas) = cx.update(|cx| {
                let s = cx.global::<crate::settings::Settings>();
                (s.auto_update, s.beta_updates)
            });
            set_beta_updates(betas);
            if !auto {
                continue;
            }
            let run = cx.background_executor().spawn(async { check() });
            redraw_while_busy(cx).await;
            run.await;
            cx.update(|cx| changed(cx));
        }
    })
    .detach();
}

/// Redraws the windows a few times a second while a check or download runs (progress on About).
pub async fn redraw_while_busy(cx: &mut gpui_kit::AsyncApp) {
    cx.background_executor().timer(Duration::from_millis(50)).await;
    while busy() {
        cx.update(|cx| cx.refresh_windows());
        cx.background_executor().timer(Duration::from_millis(150)).await;
    }
    cx.update(|cx| cx.refresh_windows());
}

/// Restarts into the downloaded update. DirectLook is asked to quit first (its exe is replaced).
pub fn restart_to_update() {
    let Some(um) = manager() else { return };
    let pending = PENDING.lock().unwrap().clone();
    rf_win::single_instance::send_to(&crate::look::ipc::app_name(), &crate::look::ipc::Msg::Quit.encode());
    std::thread::sleep(Duration::from_millis(300));
    // the dock is its own process and holds the exe open too; the new version's start brings it back
    crate::dock::control::stop_for_update();
    let asset = match pending {
        Some(info) => info.TargetFullRelease.clone(),
        None => match um.get_update_pending_restart() {
            Some(asset) => asset,
            None => return,
        },
    };
    // our window with the installer's logo while Velopack applies the update silently (its own
    // box is a plain progress dialog); Velopack's box when that window couldn't start
    let splash = crate::update_splash::launch();
    match um.wait_exit_then_apply_updates(&asset, splash.is_some(), true, Vec::<String>::new()) {
        Ok(()) => {
            // a launch in the next moments must not be forwarded to a process that ends now (see rf_win::handover)
            rf_win::handover::GATE.leave_now();
            std::process::exit(0)
        }
        Err(e) => {
            if let Some(mut s) = splash {
                let _ = s.kill();
            }
            set(State::Failed(e.to_string()));
        }
    }
}

/// The one-time "star it on GitHub" strip: after a week and 30 starts, until answered.
pub fn star_due(s: &crate::settings::Settings) -> bool {
    let now = chrono::Utc::now().timestamp_millis();
    !s.star_asked && s.first_start_ms > 0 && s.starts >= 30 && now - s.first_start_ms >= 7 * 24 * 3600 * 1000
}

/// Where a coffee is bought (the README's button).
pub const DONATE: &str = "https://www.buymeacoffee.com/ozerguzel";

const DAY_MS: i64 = 24 * 3600 * 1000;

/// The "buy me a coffee" strip: after two weeks and 40 starts, then again some weeks after each
/// answer, until turned off. Never on the same day as the star strip.
pub fn donate_due(s: &crate::settings::Settings) -> bool {
    let now = chrono::Utc::now().timestamp_millis();
    if s.donate_never || star_due(s) || s.first_start_ms == 0 || s.starts < 40 {
        return false;
    }
    let first = s.first_start_ms + 14 * DAY_MS;
    now >= first.max(s.donate_next_ms)
}

/// How long the strip stays away after an answer: longer after a coffee than after "later".
pub fn donate_answered(s: &mut crate::settings::Settings, donated: bool) {
    let now = chrono::Utc::now().timestamp_millis();
    s.donate_next_ms = now + if donated { 120 } else { 30 } * DAY_MS;
    s.donate_seen = s.donate_seen.saturating_add(1);
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
    use super::{DAY_MS, donate_answered, donate_due, is_beta, star_due, takes_betas};

    /// Manual, needs the network: the update source's TLS setup (vendor/velopack: Windows' certificate store, issue 121) reaches
    /// GitHub. `cargo test -p rf-app --lib installer::tests::github_answers_through_the_windows_store -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn github_answers_through_the_windows_store() {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
            .build()
            .into();
        let body = agent.get("https://api.github.com/repos/gelenmedya/rustyfiles/releases?per_page=1").call().unwrap().body_mut().read_to_string().unwrap();
        println!("{} bytes", body.len());
        assert!(body.contains("releases.win.json"));
    }

    #[test]
    fn a_beta_is_a_version_with_a_pre_release_part() {
        assert!(is_beta("0.7.0-beta.1"));
        assert!(is_beta("1.0.0-rc.2+build.5"));
        assert!(!is_beta("0.6.5"));
        assert!(!is_beta("0.7.0"));
        assert!(!is_beta("0.7.0+with-a-hyphen-in-the-build-part"));
        assert!(!is_beta(""));
    }

    #[test]
    fn an_ordinary_copy_takes_betas_only_when_its_switch_is_on_and_a_beta_always_does() {
        assert!(!takes_betas(false, "0.6.5"), "the installed copies of today: a beta is never downloaded by itself");
        assert!(!takes_betas(false, "0.7.0"));
        assert!(takes_betas(true, "0.7.0"));
        assert!(takes_betas(false, "0.7.0-beta.1"), "installed by hand: the next betas and then the release");
        assert!(takes_betas(true, "0.7.0-beta.1"));
        // the default of the setting is off
        assert!(!crate::settings::Settings::default().beta_updates);
    }

    #[test]
    fn coffee_strip_now_and_then() {
        let now = chrono::Utc::now().timestamp_millis();
        let mut s = crate::settings::Settings::default();
        s.star_asked = true;
        s.first_start_ms = now - 15 * DAY_MS;
        s.starts = 39;
        assert!(!donate_due(&s)); // too few starts
        s.starts = 40;
        assert!(donate_due(&s));
        s.first_start_ms = now - 13 * DAY_MS;
        assert!(!donate_due(&s)); // not two weeks yet
        s.first_start_ms = now - 15 * DAY_MS;
        donate_answered(&mut s, false);
        assert!(!donate_due(&s) && s.donate_seen == 1); // "later": a month away
        s.donate_next_ms = now - 1;
        assert!(donate_due(&s));
        donate_answered(&mut s, true);
        assert!(s.donate_next_ms - now >= 119 * DAY_MS); // after a coffee: four months
        s.donate_next_ms = 0;
        s.donate_never = true;
        assert!(!donate_due(&s));
        // never together with the star strip
        let mut t = crate::settings::Settings::default();
        t.first_start_ms = now - 15 * DAY_MS;
        t.starts = 50;
        assert!(star_due(&t) && !donate_due(&t));
    }

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

#[cfg(test)]
mod delta_tests {
    use super::deltas_are_the_smaller_download;

    const MB: u64 = 1024 * 1024;

    /// With the sizes of our own releases (0.7.2 to 0.8.0: deltas of 10.0, 9.6, 9.7, 10.0 and 11.5 MB, a full package of 38.8 MB).
    #[test]
    fn deltas_are_taken_only_while_they_are_less_to_download() {
        let full = 388 * MB / 10;
        let chain = [115 * MB / 10, 100 * MB / 10, 97 * MB / 10, 96 * MB / 10, 100 * MB / 10];
        assert!(deltas_are_the_smaller_download(&chain[..1], full), "one release behind: 11.5 MB");
        assert!(deltas_are_the_smaller_download(&chain[..2], full), "two behind: 21.5 MB");
        assert!(deltas_are_the_smaller_download(&chain[..3], full), "three behind: 31.2 MB");
        assert!(!deltas_are_the_smaller_download(&chain[..4], full), "four behind: 40.8 MB is more than the full package");
        assert!(!deltas_are_the_smaller_download(&chain, full));
    }

    #[test]
    fn nothing_to_chain_or_an_unknown_size_means_the_full_package() {
        assert!(!deltas_are_the_smaller_download(&[], 40 * MB));
        assert!(!deltas_are_the_smaller_download(&[5 * MB], 0), "a feed that does not tell the full package's size");
        assert!(deltas_are_the_smaller_download(&[0], 40 * MB), "a delta of unknown size is still tried: Velopack falls back by itself");
        assert!(!deltas_are_the_smaller_download(&[u64::MAX, 1], u64::MAX), "no overflow");
        assert!(!deltas_are_the_smaller_download(&[40 * MB], 40 * MB), "the same size: the full package, which needs no patching");
    }
}

#[cfg(test)]
mod start_apply_tests {
    use super::applies_update_at_start;

    /// A waiting update is applied when the user opens rustyfiles, and never by a process started behind the user's back.
    #[test]
    fn only_a_start_the_user_means_applies_a_waiting_update() {
        for open in [None, Some(r"C:\Users\x\Documents"), Some("D:\\"), Some("report.pdf"), Some("--settings=dock"), Some("--settings=speed")] {
            assert!(applies_update_at_start(open), "{open:?}");
        }
        for side in ["--pick", "--dock", "--shell", "--quick-search", "--no-window", "--update-splash", "--bar-preview", "--shell-probe", "--veloapp-updated", "--veloapp-obsolete", "--open-with-folder"] {
            assert!(!applies_update_at_start(Some(side)), "{side}");
        }
    }
}

#[cfg(test)]
mod important_tests {
    use super::{IMPORTANT_MARK, any_important, version_parts};

    #[test]
    fn versions_are_read_as_numbers() {
        assert_eq!(version_parts("v0.8.7"), Some((0, 8, 7)));
        assert_eq!(version_parts("0.10.0-beta.2"), Some((0, 10, 0)));
        assert_eq!(version_parts("0.8"), None);
        assert!(version_parts("0.10.0") > version_parts("0.9.12"), "numbers, not text");
    }

    #[test]
    fn an_update_is_important_when_a_release_on_the_way_is_marked() {
        let notes = |important: bool| if important { format!("## Fixed\n{IMPORTANT_MARK}\n- the dock") } else { "## Fixed\n- a typo".to_string() };
        let rel = |tag: &str, imp: bool| (tag.to_string(), notes(imp));
        let list = vec![rel("v0.8.9", false), rel("v0.8.8", false), rel("v0.8.7", true), rel("v0.8.6", false)];
        assert!(any_important(&list, "0.8.6", "0.8.9"), "0.8.7 is on the way from 0.8.6 to 0.8.9");
        assert!(any_important(&list, "0.8.6", "0.8.7"));
        assert!(!any_important(&list, "0.8.7", "0.8.9"), "already past the important one");
        assert!(!any_important(&list, "0.8.5", "0.8.6"), "the important one is after the target");
        assert!(!any_important(&[rel("v0.8.7", false)], "0.8.6", "0.8.7"));
        assert!(!any_important(&list, "x", "0.8.9"), "an unreadable version: ordinary");
    }
}
