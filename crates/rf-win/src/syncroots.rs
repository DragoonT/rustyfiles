//! The cloud folders Windows knows about: every sync root a Cloud Files provider registered (OneDrive, Proton Drive, pCloud,
//! Nextcloud, Box, MEGA, ...), as Explorer lists them in its navigation pane.
//!
//! WHY (issue 141: "The "Cloud and Linux" sidebar section only shows a fixed set of providers ... Other sync clients that use the
//! Windows Cloud Files API, such as Proton Drive, never appear there"). A provider registers each root with
//! `StorageProviderSyncRootManager.Register`, which Windows keeps under
//! `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager\<id>`: the id is "provider!user SID!account", the key
//! has the display name (`DisplayNameResource`, plain text or an indirect "@file,-id" string), and its `UserSyncRoots` subkey has
//! one value per user, named by the user's SID, whose data is the root's path. Only this account's roots are listed. Reading the
//! registry needs no package identity and no WinRT.

use std::path::PathBuf;

const KEY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager";

/// One registered sync root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncRoot {
    /// What Explorer calls it ("Proton Drive - name@example.com", "OneDrive - Personal").
    pub name: String,
    pub path: PathBuf,
    /// The provider's part of the id ("ProtonDrive", "OneDrive").
    pub provider: String,
}

/// The parts of a sync root id: (provider, user SID, account). An id without them gives the whole id as the provider.
pub fn id_parts(id: &str) -> (&str, &str, &str) {
    let mut it = id.splitn(3, '!');
    let provider = it.next().unwrap_or(id);
    (provider, it.next().unwrap_or(""), it.next().unwrap_or(""))
}

/// The name shown for a root: its display name when it has one, else the provider (and the account, when there is one).
pub fn shown_name(display: Option<&str>, id: &str) -> String {
    if let Some(d) = display.map(str::trim).filter(|d| !d.is_empty()) {
        return d.to_string();
    }
    match id_parts(id) {
        (p, _, "") => p.to_string(),
        (p, _, account) => format!("{p} - {account}"),
    }
}

fn resolve_resource(reference: &str) -> Option<String> {
    use windows::Win32::UI::Shell::SHLoadIndirectString;
    use windows::core::PCWSTR;
    let from = crate::wide::wide(reference);
    let mut buf = [0u16; 260];
    unsafe { SHLoadIndirectString(PCWSTR(from.as_ptr()), &mut buf, None).ok()? };
    Some(crate::wide::from_wide(&buf)).filter(|s| !s.is_empty())
}

/// This account's sync roots that exist on the disk, in the registry's order.
pub fn list() -> Vec<SyncRoot> {
    let Some(sid) = crate::dock::startuser::current_sid() else { return Vec::new() };
    let Ok(root) = windows_registry::LOCAL_MACHINE.open(KEY) else { return Vec::new() };
    let Ok(ids) = root.keys() else { return Vec::new() };
    let mut out: Vec<SyncRoot> = Vec::new();
    for id in ids {
        let Ok(key) = root.open(&id) else { continue };
        let Ok(path) = key.open("UserSyncRoots").and_then(|u| u.get_string(&sid)) else { continue };
        let path = PathBuf::from(crate::localized::expand_env(&path));
        if !path.is_dir() || out.iter().any(|r| r.path == path) {
            continue;
        }
        let display = key.get_string("DisplayNameResource").ok().and_then(|d| if d.starts_with('@') { resolve_resource(&d) } else { Some(d) });
        out.push(SyncRoot { name: shown_name(display.as_deref(), &id), path, provider: id_parts(&id).0.to_string() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_has_a_provider_a_user_and_an_account() {
        assert_eq!(id_parts("ProtonDrive!S-1-5-21-1-2-3-1001!user@proton.me"), ("ProtonDrive", "S-1-5-21-1-2-3-1001", "user@proton.me"));
        assert_eq!(id_parts("OneDrive!S-1-5-21-1!Personal"), ("OneDrive", "S-1-5-21-1", "Personal"));
        assert_eq!(id_parts("Odd"), ("Odd", "", ""));
    }

    #[test]
    fn a_root_is_named_by_its_display_name_else_by_its_id() {
        assert_eq!(shown_name(Some("Proton Drive"), "ProtonDrive!S-1!a@b"), "Proton Drive");
        assert_eq!(shown_name(Some("  "), "ProtonDrive!S-1!a@b"), "ProtonDrive - a@b");
        assert_eq!(shown_name(None, "pCloud!S-1!"), "pCloud");
    }

    #[test]
    fn listing_this_accounts_roots_does_not_fail() {
        // (what is listed depends on the machine: every root listed exists and is named)
        for r in list() {
            assert!(r.path.is_dir() && !r.name.is_empty(), "{r:?}");
        }
    }
}
