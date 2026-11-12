//! The desktop's virtual items: This PC, Recycle Bin, Network, the user's files and Control Panel. Which of them are on the desktop is
//! the user's choice, kept by Windows as one DWORD per item under
//! `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\HideDesktopIcons\NewStartPanel` (named by the item's CLSID; 0 shows it, 1
//! hides it). Read-only here; Windows' own Settings page is the only writer.
//!
//! OUR OWN CHOICE on top of that ([`IconChoice`], [`Choices`], [`apply`]): each of the five icons, and the master "show desktop icons"
//! switch, can be left to Windows (the default: exactly what the registry says), forced on, or forced off, by a setting of this app.
//! Nothing is ever written to the registry: the choice only changes what [`read`] found, in memory. An organisation's `NoDesktop`
//! policy is stronger than any choice.
//!
//! What was seen on a real machine (note 4, 5.2): only This PC had a value (0). The CLSIDs of the other four, and the default for an
//! item with no value (only the Recycle Bin is on a fresh Windows 11 desktop), are the widely published ones and UNVERIFIED here.

use serde::{Deserialize, Serialize};

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

/// What the app's own setting says about one icon (or about "show desktop icons" as a whole).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IconChoice {
    /// Windows' own setting decides (the default).
    #[default]
    Follow,
    Show,
    Hide,
}

impl IconChoice {
    /// The choice applied to what Windows says.
    pub fn resolve(self, windows_shows: bool) -> bool {
        match self {
            IconChoice::Follow => windows_shows,
            IconChoice::Show => true,
            IconChoice::Hide => false,
        }
    }
}

/// The app's choices for the desktop's system icons (in [`VirtualKind::ALL`] order) and for "show desktop icons".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Choices {
    pub kinds: [IconChoice; 5],
    pub show_icons: IconChoice,
}

impl Choices {
    pub fn of(&self, k: VirtualKind) -> IconChoice {
        VirtualKind::ALL.iter().position(|x| *x == k).map_or(IconChoice::Follow, |i| self.kinds[i])
    }
    pub fn set(&mut self, k: VirtualKind, c: IconChoice) {
        if let Some(i) = VirtualKind::ALL.iter().position(|x| *x == k) {
            self.kinds[i] = c;
        }
    }
    /// Every choice is "follow Windows": [`apply`] changes nothing.
    pub fn is_default(&self) -> bool {
        *self == Choices::default()
    }
}

/// What Windows said, changed by the app's choices. The organisation's policy is kept as it is, so a `NoDesktop` policy still wins
/// ([`VirtualSettings::desktop_disabled`] stays true whatever the choice for "show desktop icons" is). With every choice on "follow"
/// the result is exactly `windows`.
pub fn apply(windows: VirtualSettings, choices: &Choices) -> VirtualSettings {
    let kinds = windows
        .kinds
        .iter()
        .map(|(k, why)| match choices.of(*k) {
            IconChoice::Follow => (*k, *why),
            c => (*k, Why::Set(c.resolve(why.shown()))),
        })
        .collect();
    VirtualSettings { kinds, show_icons: choices.show_icons.resolve(windows.show_icons), policy_no_desktop: windows.policy_no_desktop }
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

    fn windows_default() -> VirtualSettings {
        read(&MemRegistry::new())
    }

    #[test]
    fn following_windows_changes_nothing_at_all() {
        let w = windows_default();
        assert_eq!(apply(w.clone(), &Choices::default()), w);
        let busy = read(&MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::ThisPc.clsid(), 0).cu_dword(KEY_ADVANCED, "HideIcons", 1));
        assert_eq!(apply(busy.clone(), &Choices::default()), busy, "also when Windows has its own values");
        assert!(Choices::default().is_default());
    }

    #[test]
    fn a_choice_overrides_what_windows_says_in_both_directions() {
        // a fresh Windows 11: only the Recycle Bin. We add This PC and take the Recycle Bin away.
        let mut c = Choices::default();
        c.set(VirtualKind::ThisPc, IconChoice::Show);
        c.set(VirtualKind::RecycleBin, IconChoice::Hide);
        let s = apply(windows_default(), &c);
        assert_eq!(s.shown(), vec![VirtualKind::ThisPc]);
        // the other three still follow Windows (hidden by default)
        assert!(!s.is_shown(VirtualKind::Network) && !s.is_shown(VirtualKind::UserFiles) && !s.is_shown(VirtualKind::ControlPanel));
        assert!(!c.is_default());
        // "show" wins over Windows having hidden it explicitly, "hide" over Windows having shown it explicitly
        let explicit = read(&MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::Network.clsid(), 1).cu_dword(KEY_HIDE_ICONS, VirtualKind::ControlPanel.clsid(), 0));
        let mut c = Choices::default();
        c.set(VirtualKind::Network, IconChoice::Show);
        c.set(VirtualKind::ControlPanel, IconChoice::Hide);
        let s = apply(explicit, &c);
        assert!(s.is_shown(VirtualKind::Network) && !s.is_shown(VirtualKind::ControlPanel));
    }

    #[test]
    fn each_of_the_five_has_its_own_choice() {
        for k in VirtualKind::ALL {
            let mut c = Choices::default();
            c.set(k, IconChoice::Show);
            let shown = apply(windows_default(), &c).shown();
            assert!(shown.contains(&k), "{k:?}");
            // only that one (and the Recycle Bin, which is on by default) is shown
            let extra = shown.iter().filter(|x| **x != k && **x != VirtualKind::RecycleBin).count();
            assert_eq!(extra, 0, "{k:?}");
            assert_eq!(c.of(k), IconChoice::Show);
        }
    }

    #[test]
    fn the_master_switch_follows_windows_unless_chosen_and_the_policy_always_wins() {
        let hidden_by_windows = read(&MemRegistry::new().cu_dword(KEY_ADVANCED, "HideIcons", 1));
        let mut c = Choices::default();
        assert!(apply(hidden_by_windows.clone(), &c).desktop_disabled(), "follow: Windows hid the icons");
        c.show_icons = IconChoice::Show;
        assert!(!apply(hidden_by_windows, &c).desktop_disabled(), "our choice shows them");
        c.show_icons = IconChoice::Hide;
        assert!(apply(windows_default(), &c).desktop_disabled(), "our choice hides them");
        // an organisation that forbids desktop items: no choice brings them back
        let policy = read(&MemRegistry::new().cu_dword(KEY_POLICIES_EXPLORER, "NoDesktop", 1));
        for choice in [IconChoice::Follow, IconChoice::Show, IconChoice::Hide] {
            let mut c = Choices { show_icons: choice, ..Default::default() };
            c.set(VirtualKind::ThisPc, IconChoice::Show);
            let s = apply(policy.clone(), &c);
            assert!(s.policy_no_desktop && s.desktop_disabled(), "{choice:?}");
        }
    }

    #[test]
    fn the_choice_is_written_as_a_lowercase_word_and_a_wrong_word_is_refused() {
        for (c, word) in [(IconChoice::Follow, "follow"), (IconChoice::Show, "show"), (IconChoice::Hide, "hide")] {
            assert_eq!(serde_json::to_string(&c).unwrap(), format!("\"{word}\""));
            assert_eq!(serde_json::from_str::<IconChoice>(&format!("\"{word}\"")).unwrap(), c);
        }
        assert!(serde_json::from_str::<IconChoice>("\"maybe\"").is_err());
        assert_eq!(IconChoice::default(), IconChoice::Follow);
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
