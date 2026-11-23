//! Updates through Velopack, as on Windows (rf-app's installer.rs): the same GitHub releases,
//! the `osx` channel (releases.osx.json). A copy that Velopack did not pack (a build from
//! macos/build.sh) runs without updates and says so.

use std::sync::Mutex;

use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateInfo, UpdateManager, VelopackApp};

const REPO: &str = "https://github.com/gelenmedya/rustyfiles";

/// What the last check found.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum UpdateState {
    /// Not packed by Velopack: no updates (a development build).
    NotInstalled,
    UpToDate,
    /// Downloaded and waiting for a restart: the new version.
    Ready { version: String },
    Failed { message: String },
}

/// The downloaded update, kept for the restart.
static PENDING: Mutex<Option<UpdateInfo>> = Mutex::new(None);

/// First thing the app does: Velopack's hooks run here, and an update downloaded before is
/// applied as the app starts (it then starts again on the new version).
#[uniffi::export]
pub fn updates_startup() {
    VelopackApp::build().set_auto_apply_on_startup(true).run();
}

/// Whether a version is a beta ("0.9.8-beta.1"; build metadata after "+" does not count).
pub(crate) fn is_beta(version: &str) -> bool {
    version.split('+').next().unwrap_or("").contains('-')
}

fn manager(betas: bool) -> Option<UpdateManager> {
    // a copy that is a beta itself takes the next betas, as on Windows (installer::takes_betas)
    let betas = betas || is_beta(&installed_version().unwrap_or_default());
    UpdateManager::new(GithubSource::new(REPO, None, betas), None, None).ok()
}

/// The installed version as Velopack knows it, or None for a copy it did not pack.
#[uniffi::export]
pub fn installed_version() -> Option<String> {
    UpdateManager::new(GithubSource::new(REPO, None, false), None, None).ok().map(|m| m.get_current_version_as_string())
}

/// Whether the delta packages are less to download than the full one (see rf-app's
/// installer::deltas_are_the_smaller_download for why Velopack's own choice is not kept).
pub(crate) fn deltas_are_the_smaller_download(deltas: &[u64], full: u64) -> bool {
    let sum = deltas.iter().fold(0u64, |all, one| all.saturating_add(*one));
    !deltas.is_empty() && sum < full
}

/// Looks for a newer version and downloads it. Blocks: call it off the main thread.
#[uniffi::export]
pub fn update_check(betas: bool) -> UpdateState {
    let Some(um) = manager(betas) else { return UpdateState::NotInstalled };
    if let Some(ready) = um.get_update_pending_restart() {
        return UpdateState::Ready { version: ready.Version };
    }
    match um.check_for_updates() {
        Ok(UpdateCheck::UpdateAvailable(mut info)) => {
            let deltas: Vec<u64> = info.DeltasToTarget.iter().map(|a| a.Size).collect();
            if !deltas.is_empty() && !deltas_are_the_smaller_download(&deltas, info.TargetFullRelease.Size) {
                info.DeltasToTarget.clear();
                info.BaseRelease = None;
            }
            let version = info.TargetFullRelease.Version.clone();
            match um.download_updates(&info, None) {
                Ok(()) => {
                    *PENDING.lock().unwrap() = Some(*info);
                    UpdateState::Ready { version }
                }
                Err(e) => UpdateState::Failed { message: e.to_string() },
            }
        }
        Ok(_) => UpdateState::UpToDate,
        Err(e) => UpdateState::Failed { message: e.to_string() },
    }
}

/// Applies the downloaded update once the app has quit, and starts the new version. The app
/// should quit right after this returns true.
#[uniffi::export]
pub fn update_apply_and_restart() -> bool {
    let Some(um) = manager(false) else { return false };
    let asset = match PENDING.lock().unwrap().clone() {
        Some(info) => info.TargetFullRelease.clone(),
        None => match um.get_update_pending_restart() {
            Some(a) => a,
            None => return false,
        },
    };
    um.wait_exit_then_apply_updates(&asset, false, true, Vec::<String>::new()).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn betas_and_deltas_are_told_apart() {
        assert!(is_beta("0.9.8-beta.1"));
        assert!(!is_beta("0.9.8"));
        assert!(!is_beta("0.9.8+abc"));
        assert!(deltas_are_the_smaller_download(&[10, 10], 39));
        assert!(!deltas_are_the_smaller_download(&[10, 10, 10, 10], 39));
        assert!(!deltas_are_the_smaller_download(&[], 39));
        // a build from build.sh is not a Velopack install
        assert_eq!(update_check(false), UpdateState::NotInstalled);
    }
}
