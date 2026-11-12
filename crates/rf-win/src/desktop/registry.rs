//! The real registry for the desktop model: read-only. The trait (`rf_core::desktop::registry::RegRead`) has no write method, and this
//! file opens every key with read access only (`windows_registry::Key::open` is read-only; nothing here calls `create`).

use rf_core::desktop::registry::{Hive, RegRead};
use windows_registry::{CURRENT_USER, Key, LOCAL_MACHINE};

pub struct SystemRegistry;

fn open(hive: Hive, key: &str) -> Option<Key> {
    match hive {
        Hive::CurrentUser => CURRENT_USER.open(key).ok(),
        Hive::LocalMachine => LOCAL_MACHINE.open(key).ok(),
    }
}

impl RegRead for SystemRegistry {
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String> {
        open(hive, key)?.get_string(name).ok()
    }
    fn dword(&self, hive: Hive, key: &str, name: &str) -> Option<u32> {
        open(hive, key)?.get_u32(name).ok()
    }
    fn binary(&self, hive: Hive, key: &str, name: &str) -> Option<Vec<u8>> {
        let v = open(hive, key)?.get_value(name).ok()?;
        Some(v.to_vec())
    }
    fn subkeys(&self, hive: Hive, key: &str) -> Vec<String> {
        open(hive, key).and_then(|k| k.keys().ok().map(|it| it.collect())).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads values that exist on every Windows machine; writes nothing.
    #[test]
    fn reads_the_real_registry() {
        let r = SystemRegistry;
        // Windows always has a ProductName under HKLM (a string), a build number (a string) and sub-keys under Software
        assert!(r.string(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName").is_some());
        assert!(r.dword(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "ProductName").is_none(), "a string is not a dword");
        assert!(r.string(Hive::LocalMachine, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "NoSuchValueAnywhere").is_none());
        assert!(r.string(Hive::CurrentUser, r"No\Such\Key", "x").is_none());
        assert!(!r.subkeys(Hive::CurrentUser, "Software").is_empty());
        assert!(r.subkeys(Hive::CurrentUser, r"No\Such\Key").is_empty());
        // a binary value: the build's UBR is a dword, not binary, so use the digital product id's cousin that always exists
        assert!(r.binary(Hive::CurrentUser, r"Control Panel\Desktop", "NoSuchBinary").is_none());
    }
}
