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

const NAMESPACE: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Desktop\NameSpace";

/// The folders a sync client pinned to Explorer's navigation pane itself, outside the Cloud Files API: each is a CLSID under this
/// user's `Desktop\NameSpace` whose class has `System.IsPinnedToNameSpaceTree` = 1 and an `Instance\InitPropertyBag` with the
/// folder's `TargetFolderPath`. MEGA puts every one of its sync folders there (a request on Discord, 2026-11-23: "Files shows them
/// automatically"; Files reads the same keys), and Dropbox, Box and Nextcloud have used it too. Only folders that exist.
pub fn pinned_folders() -> Vec<SyncRoot> {
    let Ok(classes) = windows_registry::CLASSES_ROOT.open("CLSID") else { return Vec::new() };
    pinned_in(windows_registry::CURRENT_USER, NAMESPACE, &classes).into_iter().filter(|r| r.path.is_dir()).collect()
}

fn pinned_in(hive: &windows_registry::Key, namespace: &str, classes: &windows_registry::Key) -> Vec<SyncRoot> {
    let Ok(ns) = hive.open(namespace) else { return Vec::new() };
    let Ok(ids) = ns.keys() else { return Vec::new() };
    let mut out: Vec<SyncRoot> = Vec::new();
    for id in ids {
        let Ok(class) = classes.open(&id) else { continue };
        if class.get_u32("System.IsPinnedToNameSpaceTree").ok() != Some(1) {
            continue;
        }
        let Ok(target) = class.open(r"Instance\InitPropertyBag").and_then(|b| b.get_string("TargetFolderPath")) else { continue };
        let path = PathBuf::from(crate::localized::expand_env(&target));
        if out.iter().any(|r| r.path == path) {
            continue;
        }
        let provider = ns.open(&id).and_then(|k| k.get_string("")).unwrap_or_default().trim().to_string();
        let label = class.get_string("").unwrap_or_default();
        out.push(SyncRoot { name: pinned_name(&provider, &label, &path), path, provider });
    }
    out
}

/// The name of a pinned folder: the class's own name, else the provider's, else the folder's. MEGA calls every one of its folders
/// "MEGA", so the folder's name is added to tell them apart ("MEGA (Projects)"), as Files does.
fn pinned_name(provider: &str, label: &str, path: &std::path::Path) -> String {
    let folder = path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_else(|| path.display().to_string());
    let base = [label.trim(), provider.trim()].into_iter().find(|s| !s.is_empty()).unwrap_or(&folder).to_string();
    if base.eq_ignore_ascii_case("MEGA") && !folder.eq_ignore_ascii_case("MEGA") { format!("MEGA ({folder})") } else { base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_pinned_to_the_navigation_pane_is_read_with_its_name() {
        let base = format!(r"Software\rustyfiles-test\pinned-{}", std::process::id());
        let cu = windows_registry::CURRENT_USER;
        let ns = format!(r"{base}\NameSpace");
        let mega = "{11111111-2222-3333-4444-555555555555}";
        let unpinned = "{66666666-2222-3333-4444-555555555555}";
        cu.create(format!(r"{ns}\{mega}")).unwrap().set_string("", "MEGA").unwrap();
        cu.create(format!(r"{ns}\{unpinned}")).unwrap().set_string("", "Other").unwrap();
        let class = cu.create(format!(r"{base}\CLSID\{mega}")).unwrap();
        class.set_string("", "MEGA").unwrap();
        class.set_u32("System.IsPinnedToNameSpaceTree", 1).unwrap();
        cu.create(format!(r"{base}\CLSID\{mega}\Instance\InitPropertyBag")).unwrap().set_string("TargetFolderPath", r"D:\Work\Projects").unwrap();
        let other = cu.create(format!(r"{base}\CLSID\{unpinned}")).unwrap();
        other.set_u32("System.IsPinnedToNameSpaceTree", 0).unwrap();
        cu.create(format!(r"{base}\CLSID\{unpinned}\Instance\InitPropertyBag")).unwrap().set_string("TargetFolderPath", r"D:\Elsewhere").unwrap();
        let classes = cu.open(format!(r"{base}\CLSID")).unwrap();
        let got = pinned_in(cu, &ns, &classes);
        let _ = cu.remove_tree(r"Software\rustyfiles-test");
        assert_eq!(got, [SyncRoot { name: "MEGA (Projects)".into(), path: PathBuf::from(r"D:\Work\Projects"), provider: "MEGA".into() }]);
    }

    #[test]
    fn a_pinned_folder_is_named_by_its_class_then_its_provider_then_itself() {
        let p = std::path::Path::new(r"C:\Users\a\Box");
        assert_eq!(pinned_name("Box", "Box Drive", p), "Box Drive");
        assert_eq!(pinned_name("Nextcloud", " ", p), "Nextcloud");
        assert_eq!(pinned_name("", "", p), "Box");
        assert_eq!(pinned_name("MEGA", "MEGA", std::path::Path::new(r"C:\Users\a\MEGA")), "MEGA");
    }

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
