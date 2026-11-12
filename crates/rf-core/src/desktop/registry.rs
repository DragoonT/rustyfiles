//! The registry as the desktop model reads it: read-only, behind a trait, with an in-memory fake for tests. There is deliberately no
//! write method anywhere in this module or in the real implementation (`rf_win::desktop::registry`): the desktop only reads what
//! Windows keeps (shell plan 2.1: "Windows state we will never write").

use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Hive {
    CurrentUser,
    LocalMachine,
}

/// Read-only access. Key paths have no hive prefix and use `\`; key and value names are case-insensitive, as in the registry.
pub trait RegRead {
    /// A `REG_SZ` / `REG_EXPAND_SZ` value (not expanded).
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String>;
    fn dword(&self, hive: Hive, key: &str, name: &str) -> Option<u32>;
    fn binary(&self, hive: Hive, key: &str, name: &str) -> Option<Vec<u8>>;
    /// The names of the direct sub-keys (empty when the key is missing).
    fn subkeys(&self, hive: Hive, key: &str) -> Vec<String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Val {
    Str(String),
    Dword(u32),
    Bin(Vec<u8>),
}

/// An in-memory registry for tests. Every value is set through the builder methods; reading goes through [`RegRead`].
#[derive(Debug, Default, Clone)]
pub struct MemRegistry {
    /// (hive, lower-case key, lower-case value name) -> value
    values: BTreeMap<(u8, String, String), Val>,
    /// (hive, lower-case key) -> the key as first written (case kept for sub-key names)
    keys: BTreeMap<(u8, String), String>,
}

fn h(hive: Hive) -> u8 {
    match hive {
        Hive::CurrentUser => 0,
        Hive::LocalMachine => 1,
    }
}

impl MemRegistry {
    pub fn new() -> Self {
        Self::default()
    }
    fn touch_key(&mut self, hive: Hive, key: &str) {
        // every parent exists too
        let mut path = String::new();
        for part in key.split('\\').filter(|p| !p.is_empty()) {
            if !path.is_empty() {
                path.push('\\');
            }
            path.push_str(part);
            self.keys.entry((h(hive), path.to_lowercase())).or_insert_with(|| path.clone());
        }
    }
    pub fn with_key(mut self, hive: Hive, key: &str) -> Self {
        self.touch_key(hive, key);
        self
    }
    pub fn with_string(mut self, hive: Hive, key: &str, name: &str, v: &str) -> Self {
        self.touch_key(hive, key);
        self.values.insert((h(hive), key.to_lowercase(), name.to_lowercase()), Val::Str(v.to_string()));
        self
    }
    pub fn with_dword(mut self, hive: Hive, key: &str, name: &str, v: u32) -> Self {
        self.touch_key(hive, key);
        self.values.insert((h(hive), key.to_lowercase(), name.to_lowercase()), Val::Dword(v));
        self
    }
    pub fn with_binary(mut self, hive: Hive, key: &str, name: &str, v: &[u8]) -> Self {
        self.touch_key(hive, key);
        self.values.insert((h(hive), key.to_lowercase(), name.to_lowercase()), Val::Bin(v.to_vec()));
        self
    }
    /// Shorthands for the current user's hive, which is nearly everything here.
    pub fn cu_string(self, key: &str, name: &str, v: &str) -> Self {
        self.with_string(Hive::CurrentUser, key, name, v)
    }
    pub fn cu_dword(self, key: &str, name: &str, v: u32) -> Self {
        self.with_dword(Hive::CurrentUser, key, name, v)
    }
    pub fn cu_binary(self, key: &str, name: &str, v: &[u8]) -> Self {
        self.with_binary(Hive::CurrentUser, key, name, v)
    }
    fn get(&self, hive: Hive, key: &str, name: &str) -> Option<&Val> {
        self.values.get(&(h(hive), key.trim_matches('\\').to_lowercase(), name.to_lowercase()))
    }
}

impl RegRead for MemRegistry {
    fn string(&self, hive: Hive, key: &str, name: &str) -> Option<String> {
        match self.get(hive, key, name)? {
            Val::Str(s) => Some(s.clone()),
            _ => None,
        }
    }
    fn dword(&self, hive: Hive, key: &str, name: &str) -> Option<u32> {
        match self.get(hive, key, name)? {
            Val::Dword(d) => Some(*d),
            _ => None,
        }
    }
    fn binary(&self, hive: Hive, key: &str, name: &str) -> Option<Vec<u8>> {
        match self.get(hive, key, name)? {
            Val::Bin(b) => Some(b.clone()),
            _ => None,
        }
    }
    fn subkeys(&self, hive: Hive, key: &str) -> Vec<String> {
        let prefix = format!("{}\\", key.trim_matches('\\').to_lowercase());
        let mut out = Vec::new();
        for ((hv, lower), original) in &self.keys {
            if *hv != h(hive) {
                continue;
            }
            if let Some(rest) = lower.strip_prefix(&prefix)
                && !rest.contains('\\')
            {
                out.push(original.rsplit('\\').next().unwrap_or(original).to_string());
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_is_case_insensitive_and_typed() {
        let r = MemRegistry::new()
            .cu_string(r"Control Panel\Desktop", "WallPaper", "x")
            .cu_dword(r"Control Panel\Desktop", "TranscodedImageCount", 2)
            .cu_binary(r"Software\A\B", "Blob", &[1, 2, 3])
            .with_dword(Hive::LocalMachine, r"Software\Policies", "NoDesktop", 1);
        let cu = Hive::CurrentUser;
        assert_eq!(r.string(cu, r"control panel\DESKTOP", "wallpaper").as_deref(), Some("x"));
        assert_eq!(r.dword(cu, r"Control Panel\Desktop", "TranscodedImageCount"), Some(2));
        assert_eq!(r.string(cu, r"Control Panel\Desktop", "TranscodedImageCount"), None, "a dword is not a string");
        assert_eq!(r.binary(cu, r"Software\A\B", "blob"), Some(vec![1, 2, 3]));
        assert_eq!(r.dword(cu, r"Software\Policies", "NoDesktop"), None, "other hive");
        assert_eq!(r.dword(Hive::LocalMachine, r"Software\Policies", "NoDesktop"), Some(1));
        assert_eq!(r.subkeys(cu, "Software"), vec!["A".to_string()]);
        assert_eq!(r.subkeys(cu, r"Software\A"), vec!["B".to_string()]);
        assert!(r.subkeys(cu, r"Nope").is_empty());
    }
}
