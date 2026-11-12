//! The desktop's virtual items: This PC, Recycle Bin, Network, the user's files and Control Panel. Which of them are on the desktop is
//! the user's choice, kept by Windows as one DWORD per item under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel` (named by the item's CLSID; 0 shows it, 1
//! hides it). Read-only here; Windows' own Settings page is the only writer.
//!
//! What was seen on a real machine (note 4, 5.2): only This PC had a value (0). The CLSIDs of the other four, and the default for an
//! item with no value (only the Recycle Bin is on a fresh Windows 11 desktop), are the widely published ones and UNVERIFIED here.

use super::registry::{Hive, RegRead};

pub const KEY_HIDE_ICONS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel";
pub const KEY_ADVANCED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced";
pub const KEY_POLICIES_EXPLORER: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VirtualKind {
    ThisPc,
    UserFiles,
    Network,
    RecycleBin,
    ControlPanel,
}

impl VirtualKind {
    /// The order they are listed in, and the order a name sort puts them at the front of the desktop.
    pub const ALL: [VirtualKind; 5] = [VirtualKind::ThisPc, VirtualKind::UserFiles, VirtualKind::Network, VirtualKind::RecycleBin, VirtualKind::ControlPanel];

    pub const fn clsid(self) -> &'static str {
        match self {
            VirtualKind::ThisPc => "{20D04FE0-3AEA-1069-A2D8-08002B30309D}",
            VirtualKind::UserFiles => "{59031A47-3F72-44A7-89C5-5595FE6B30EE}",
            VirtualKind::Network => "{F02C1A0D-BE21-4350-88B0-7367FC96EF3C}",
            VirtualKind::RecycleBin => "{645FF040-5081-101B-9F08-00AA002F954E}",
            VirtualKind::ControlPanel => "{5399E694-6CE5-4D6C-8FCE-1D8870FDCBA0}",
        }
    }
    /// The parsing name `SHCreateItemFromParsingName` takes (`rf_win::shellfolder::THIS_PC` is the first).
    pub fn parsing_name(self) -> String {
        format!("::{}", self.clsid())
    }
    /// Shown on a fresh Windows 11 desktop when the user has never touched the setting.
    pub const fn shown_by_default(self) -> bool {
        matches!(self, VirtualKind::RecycleBin)
    }
    /// A fallback label; the app asks the shell for the localised one.
    pub const fn english_name(self) -> &'static str {
        match self {
            VirtualKind::ThisPc => "This PC",
            VirtualKind::UserFiles => "User's files",
            VirtualKind::Network => "Network",
            VirtualKind::RecycleBin => "Recycle Bin",
            VirtualKind::ControlPanel => "Control Panel",
        }
    }
    pub fn from_clsid(s: &str) -> Option<VirtualKind> {
        let s = s.trim().trim_start_matches("::");
        VirtualKind::ALL.into_iter().find(|k| k.clsid().eq_ignore_ascii_case(s))
    }
}

/// How the setting for one item was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// The registry has a value for it.
    Set(bool),
    /// No value: the default.
    Default(bool),
}

impl Why {
    pub fn shown(self) -> bool {
        match self {
            Why::Set(b) | Why::Default(b) => b,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VirtualSettings {
    /// In [`VirtualKind::ALL`] order.
    pub kinds: Vec<(VirtualKind, Why)>,
    /// "Show desktop icons" (`Explorer\Advanced\HideIcons` = 0, the default). Off: no icon at all, the desktop is just the wallpaper.
    pub show_icons: bool,
    /// The `NoDesktop` policy (current user or machine): the company wants no desktop items.
    pub policy_no_desktop: bool,
}

impl VirtualSettings {
    pub fn shown(&self) -> Vec<VirtualKind> {
        self.kinds.iter().filter(|(_, w)| w.shown()).map(|(k, _)| *k).collect()
    }
    pub fn is_shown(&self, k: VirtualKind) -> bool {
        self.kinds.iter().any(|(kk, w)| *kk == k && w.shown())
    }
    /// Nothing at all is to be drawn on the desktop.
    pub fn desktop_disabled(&self) -> bool {
        !self.show_icons || self.policy_no_desktop
    }
}

pub fn read(reg: &dyn RegRead) -> VirtualSettings {
    let cu = Hive::CurrentUser;
    let kinds = VirtualKind::ALL
        .into_iter()
        .map(|k| {
            let why = match reg.dword(cu, KEY_HIDE_ICONS, k.clsid()) {
                Some(v) => Why::Set(v == 0),
                None => Why::Default(k.shown_by_default()),
            };
            (k, why)
        })
        .collect();
    let policy = reg.dword(cu, KEY_POLICIES_EXPLORER, "NoDesktop").is_some_and(|v| v != 0)
        || reg.dword(Hive::LocalMachine, KEY_POLICIES_EXPLORER, "NoDesktop").is_some_and(|v| v != 0);
    VirtualSettings { kinds, show_icons: reg.dword(cu, KEY_ADVANCED, "HideIcons").is_none_or(|v| v == 0), policy_no_desktop: policy }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::registry::MemRegistry;

    #[test]
    fn a_fresh_desktop_shows_only_the_recycle_bin() {
        let s = read(&MemRegistry::new());
        assert_eq!(s.shown(), vec![VirtualKind::RecycleBin]);
        assert!(s.show_icons && !s.policy_no_desktop && !s.desktop_disabled());
        assert!(s.kinds.iter().all(|(_, w)| matches!(w, Why::Default(_))));
    }

    #[test]
    fn the_observed_machine_shows_this_pc() {
        // value 0 for This PC (seen on a real machine), nothing else
        let r = MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::ThisPc.clsid(), 0).cu_dword(KEY_ADVANCED, "HideIcons", 0);
        let s = read(&r);
        assert_eq!(s.shown(), vec![VirtualKind::ThisPc, VirtualKind::RecycleBin]);
        assert_eq!(s.kinds[0].1, Why::Set(true));
    }

    #[test]
    fn explicit_values_beat_defaults_in_both_directions() {
        let r = MemRegistry::new()
            .cu_dword(KEY_HIDE_ICONS, VirtualKind::RecycleBin.clsid(), 1)
            .cu_dword(KEY_HIDE_ICONS, VirtualKind::Network.clsid(), 0)
            .cu_dword(KEY_HIDE_ICONS, VirtualKind::ControlPanel.clsid(), 0)
            .cu_dword(KEY_HIDE_ICONS, VirtualKind::UserFiles.clsid(), 2); // any non-zero hides
        let s = read(&r);
        assert_eq!(s.shown(), vec![VirtualKind::Network, VirtualKind::ControlPanel]);
        assert!(!s.is_shown(VirtualKind::RecycleBin) && !s.is_shown(VirtualKind::UserFiles));
    }

    #[test]
    fn the_master_switch_and_the_policy() {
        let off = read(&MemRegistry::new().cu_dword(KEY_ADVANCED, "HideIcons", 1));
        assert!(!off.show_icons && off.desktop_disabled());
        let pol = read(&MemRegistry::new().cu_dword(KEY_POLICIES_EXPLORER, "NoDesktop", 1));
        assert!(pol.policy_no_desktop && pol.desktop_disabled());
        let machine = read(&MemRegistry::new().with_dword(Hive::LocalMachine, KEY_POLICIES_EXPLORER, "NoDesktop", 1));
        assert!(machine.policy_no_desktop);
        let zero = read(&MemRegistry::new().cu_dword(KEY_POLICIES_EXPLORER, "NoDesktop", 0));
        assert!(!zero.policy_no_desktop);
    }

    #[test]
    fn clsids_round_trip() {
        for k in VirtualKind::ALL {
            assert_eq!(VirtualKind::from_clsid(&k.parsing_name()), Some(k));
            assert_eq!(VirtualKind::from_clsid(&k.clsid().to_lowercase()), Some(k));
        }
        assert_eq!(VirtualKind::from_clsid("::{00000000-0000-0000-0000-000000000000}"), None);
        assert_eq!(VirtualKind::ThisPc.parsing_name(), "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
    }
}
