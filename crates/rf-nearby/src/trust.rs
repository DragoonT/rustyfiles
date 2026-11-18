//! The devices this one trusts: paired once (both sides confirmed the same code), then known by their certificate's fingerprint.
//! Kept as a small JSON file in the state folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "trusted.json";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrustedDevice {
    pub id: String,
    pub name: String,
    pub fingerprint: String,
    /// The last speed measured with it, or of the last transfer with it (bytes a second; 0: none yet).
    #[serde(default)]
    pub speed_bps: u64,
    /// `speed_bps` came from a speed test (else from a transfer).
    #[serde(default)]
    pub speed_measured: bool,
}

pub struct TrustStore {
    path: PathBuf,
    devices: Vec<TrustedDevice>,
}

impl TrustStore {
    pub fn load(dir: &Path) -> TrustStore {
        let path = dir.join(FILE);
        let devices = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        TrustStore { path, devices }
    }

    pub fn devices(&self) -> &[TrustedDevice] {
        &self.devices
    }

    /// Whether a peer that proved this fingerprint is trusted.
    pub fn trusts(&self, fingerprint: &str) -> bool {
        self.devices.iter().any(|d| d.fingerprint == fingerprint)
    }

    pub fn is_trusted_id(&self, id: &str) -> bool {
        self.devices.iter().any(|d| d.id == id)
    }

    /// Adds (or renames) a device and writes the file.
    pub fn add(&mut self, device: TrustedDevice) -> std::io::Result<()> {
        self.devices.retain(|d| d.fingerprint != device.fingerprint);
        self.devices.push(device);
        self.save()
    }

    /// Keeps the speed seen with a trusted device. Nothing for a device that is not trusted.
    pub fn set_speed(&mut self, id: &str, bps: u64, measured: bool) -> std::io::Result<()> {
        let Some(d) = self.devices.iter_mut().find(|d| d.id == id) else { return Ok(()) };
        d.speed_bps = bps;
        d.speed_measured = measured;
        self.save()
    }

    /// Forgets a device by id. False when it was not there.
    pub fn remove(&mut self, id: &str) -> std::io::Result<bool> {
        let before = self.devices.len();
        self.devices.retain(|d| d.id != id);
        if self.devices.len() == before {
            return Ok(false);
        }
        self.save().map(|_| true)
    }

    fn save(&self) -> std::io::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.devices).unwrap_or_default())?;
        std::fs::rename(&tmp, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_devices_are_kept_and_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = TrustStore::load(dir.path());
        s.add(TrustedDevice { id: "a1".into(), name: "Desk".into(), fingerprint: "ff".into(), ..Default::default() }).unwrap();
        let s2 = TrustStore::load(dir.path());
        assert!(s2.trusts("ff") && s2.is_trusted_id("a1"));
        assert!(s.remove("a1").unwrap());
        assert!(!TrustStore::load(dir.path()).trusts("ff"));
    }
}
