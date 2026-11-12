//! The Recycle Bin's own settings on the real system, behind [`rf_core::recycle_bin::BinSettingsStore`].
//!
//! Where Windows keeps them (from the documented behaviour of the bin and from what third-party documentation of the
//! Recycle Bin says; NOT checked against Explorer's own dialog, see docs/design/recycle-bin.md):
//! * per drive, `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{volume GUID}`, with `MaxCapacity`
//!   (REG_DWORD, megabytes; absent = the bin's default size) and `NukeOnDelete` (REG_DWORD, 1 = "remove files immediately");
//! * the delete confirmation: the shell setting `fNoConfirmRecycle` of `SHGetSetSettings` (SSF_NOCONFIRMRECYCLE), which Windows
//!   keeps in the `ShellState` value; the group policy `HKCU\...\Policies\Explorer\ConfirmFileDelete` = 1 forces it on.
//!
//! Reading is harmless and runs in a probe. **Writing has never been run by the author** (the rule was: never write the
//! registry, never change a Windows setting): `write` is only reachable through `rf_core::recycle_bin::save`, which is called
//! from the settings dialog's Save button and nowhere else, and no test calls it.

use rf_core::recycle_bin::{BinSettings, BinSettingsStore, Changes, VolumeBin};
use windows::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;
use windows::Win32::UI::Shell::{SHELLSTATEA, SHGetSetSettings, SSF_NOCONFIRMRECYCLE};
use windows::core::PCWSTR;
use windows_registry::CURRENT_USER;

use crate::drives::{DriveKind, list_drives};
use crate::wide::{from_wide, wide};

const VOLUME_KEYS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume";
const POLICY_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
/// fNoConfirmRecycle is the third bit of SHELLSTATE's first bit field (after fShowAllObjects and fShowExtensions).
const NO_CONFIRM_BIT: i32 = 1 << 2;

/// `{GUID}` out of a volume name as `GetVolumeNameForVolumeMountPointW` gives it (`\\?\Volume{guid}\`), upper-cased the way the
/// bin's own keys are written. `None` for anything else.
pub fn volume_id_from_name(name: &str) -> Option<String> {
    let rest = name.strip_prefix(r"\\?\Volume")?.trim_end_matches('\\');
    is_guid_in_braces(rest).then(|| rest.to_ascii_uppercase())
}

/// `{8-4-4-4-12 hex digits}`. A key name that is not exactly this is never written to.
pub fn is_guid_in_braces(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 38 || b[0] != b'{' || b[37] != b'}' {
        return false;
    }
    b[1..37].iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

fn volume_id(root: &str) -> Option<String> {
    let w = wide(root);
    let mut buf = [0u16; 64];
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(w.as_ptr()), &mut buf) }.ok()?;
    volume_id_from_name(&from_wide(&buf))
}

/// The fixed drives that have a bin: (id, "C:", size in MB).
fn volumes() -> Vec<(String, String, u64)> {
    list_drives()
        .into_iter()
        .filter(|d| d.kind == DriveKind::Fixed)
        .filter_map(|d| Some((volume_id(&d.root.to_string_lossy())?, format!("{}:", d.letter), d.total / (1024 * 1024))))
        .collect()
}

fn confirm_state() -> bool {
    let mut ss = SHELLSTATEA::default();
    unsafe { SHGetSetSettings(Some(&mut ss), SSF_NOCONFIRMRECYCLE, false) };
    ss._bitfield1 & NO_CONFIRM_BIT == 0
}

fn confirm_forced_by_policy() -> bool {
    CURRENT_USER.open(POLICY_KEY).and_then(|k| k.get_u32("ConfirmFileDelete")).is_ok_and(|v| v == 1)
}

/// What the registry holds for one drive, read only: (MaxCapacity, NukeOnDelete).
fn read_volume(id: &str) -> (Option<u32>, bool) {
    match CURRENT_USER.open(format!(r"{VOLUME_KEYS}\{id}")) {
        Ok(k) => (k.get_u32("MaxCapacity").ok(), k.get_u32("NukeOnDelete").is_ok_and(|v| v != 0)),
        Err(_) => (None, false),
    }
}

pub struct SystemBinSettings;

impl BinSettingsStore for SystemBinSettings {
    fn read(&self) -> anyhow::Result<BinSettings> {
        crate::shell::ensure_com();
        let volumes = volumes()
            .into_iter()
            .map(|(id, label, capacity_mb)| {
                let (max_mb, remove_immediately) = read_volume(&id);
                VolumeBin { id, label, capacity_mb, max_mb, remove_immediately }
            })
            .collect();
        let locked = confirm_forced_by_policy();
        Ok(BinSettings { volumes, confirm_delete: locked || confirm_state(), confirm_locked: locked })
    }

    /// Writes the differences. UNRUN. A drive the system does not list, or an id that is not a volume GUID, is refused before
    /// anything is written. The drives are written one after the other and the confirmation last: when one step fails the ones before
    /// it stay written, which is why the dialog reads the settings again after a failure.
    fn write(&self, changes: &Changes) -> anyhow::Result<()> {
        crate::shell::ensure_com();
        let known = volumes();
        for v in &changes.volumes {
            anyhow::ensure!(is_guid_in_braces(&v.id) && known.iter().any(|(id, _, _)| id.eq_ignore_ascii_case(&v.id)), "unknown drive");
            if let Some(mb) = v.max_mb {
                anyhow::ensure!(mb >= 1, "size below 1 MB");
            }
        }
        for v in &changes.volumes {
            let key = CURRENT_USER.create(format!(r"{VOLUME_KEYS}\{}", v.id))?;
            if let Some(mb) = v.max_mb {
                key.set_u32("MaxCapacity", mb)?;
            }
            if let Some(nuke) = v.remove_immediately {
                key.set_u32("NukeOnDelete", u32::from(nuke))?;
            }
        }
        if let Some(confirm) = changes.confirm_delete {
            anyhow::ensure!(!confirm_forced_by_policy(), "the delete confirmation is set by a policy");
            let mut ss = SHELLSTATEA::default();
            if !confirm {
                ss._bitfield1 |= NO_CONFIRM_BIT;
            }
            unsafe { SHGetSetSettings(Some(&mut ss), SSF_NOCONFIRMRECYCLE, true) };
            // The call gives no answer of its own and the API is documented as no longer supported: read it back, and say so when the
            // system did not take it (the drives above are already written; the dialog reads the settings again after a failure).
            anyhow::ensure!(confirm_state() == confirm, "Windows silme onayı ayarını kabul etmedi");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_volume_name_gives_its_key() {
        assert_eq!(
            volume_id_from_name(r"\\?\Volume{0d54cb8d-1f2a-4c3b-9a8e-0123456789ab}\").as_deref(),
            Some("{0D54CB8D-1F2A-4C3B-9A8E-0123456789AB}")
        );
        assert_eq!(volume_id_from_name(r"\\?\Volume{0d54cb8d-1f2a-4c3b-9a8e-0123456789ab}").as_deref(), Some("{0D54CB8D-1F2A-4C3B-9A8E-0123456789AB}"));
        for bad in ["", r"C:\", r"\\?\Volume{}\", r"\\?\Volume{0d54cb8d-1f2a-4c3b-9a8e-0123456789a}\", r"\\?\Volume{zd54cb8d-1f2a-4c3b-9a8e-0123456789ab}\", r"\\?\C:\"] {
            assert_eq!(volume_id_from_name(bad), None, "{bad}");
        }
        assert!(!is_guid_in_braces(r"{0D54CB8D-1F2A-4C3B-9A8E-0123456789AB}\..\x"));
        assert!(!is_guid_in_braces("{0D54CB8D1F2A-4C3B-9A8E-0123456789AB--}"));
    }

    /// Reads the real settings (a read-only registry and shell-setting read, nothing is written) and checks they are consistent.
    #[test]
    fn the_real_settings_read() {
        let s = SystemBinSettings.read().unwrap();
        for v in &s.volumes {
            assert!(is_guid_in_braces(&v.id) && v.label.ends_with(':'));
            assert!(v.max_mb.is_none_or(|m| m > 0));
        }
        println!("bin settings: {} drives, confirmation {}", s.volumes.len(), s.confirm_delete);
    }
}
