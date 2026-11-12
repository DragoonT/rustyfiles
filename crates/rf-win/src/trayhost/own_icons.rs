//! Which notification icons are left out of the dock's tray strip because the dock already shows the same thing as a button of its
//! own. Pure.
//!
//! WHY THERE ARE ANY. On Windows 11 the volume, network and power indicators are not ordinary notification icons: Explorer draws them as
//! its own "system controls". But Explorer still carries the old systray shell service object (`stobject.dll`; its hidden window of
//! class `SystemTray_Main` is in Explorer's process on this build, read with a windowless listing) and the volume service object
//! (`SndVolSSO.dll`), and when the tray host's start broadcast makes programs add their icons again, Explorer's own parts add THEIRS
//! again through the same `Shell_NotifyIcon` door as everybody. So the host can be handed an icon whose owner is Explorer (the window
//! named in the block belongs to `explorer.exe`) and which is Windows' volume indicator, and the dock's right island has a volume
//! button of its own. Showing both is the duplicate the user saw.
//!
//! WHAT IS LEFT OUT, AND HOW IT IS TOLD. By the owner AND the icon's identity, never by its text (the tip changes with the language and
//! the state: "Hoparlör: %43") and never by its picture:
//!
//! * the owner must be a program of Windows itself: a file below the Windows directory (the path is read from the owner's process by
//!   the feed). When only the file name is known, `explorer.exe` is accepted and nothing else. When nothing is known the icon stays: a
//!   rule that cannot see whose icon it is does not hide it;
//! * the identity is the icon's GUID (`NIF_GUID`), compared with the three GUIDs Explorer's own parts use for these indicators
//!   ([`SystemIcon`]). An icon without a GUID has no identity this rule can name and stays (a legacy systray `uID` table would be a
//!   guess: none of those numbers is verified on this build, and a wrong one would hide a program's icon).
//!
//! AND ONLY WHILE THE DOCK SHOWS THE BUTTON ([`OwnButtons`]): the volume indicator goes only while the dock has a volume button, the
//! network indicator only while it has a network button, the power indicator only while it has a battery button (it has none today:
//! the power indicator stays). A third party's audio or network icon (an audio driver's control panel, a VPN) has another owner and
//! another identity, so it stays.
//!
//! The icon is NOT removed from the host's model: Explorer's list and the click engine are untouched. It is only not drawn.

use crate::tray_wire::WireGuid;

/// The indicators of Windows that the dock may show as buttons of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemIcon {
    Volume,
    Network,
    Power,
}

/// Which of those buttons the dock shows right now.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OwnButtons {
    pub volume: bool,
    pub network: bool,
    pub power: bool,
}

impl OwnButtons {
    /// Whether the dock has a button for `icon`.
    pub fn shows(self, icon: SystemIcon) -> bool {
        match icon {
            SystemIcon::Volume => self.volume,
            SystemIcon::Network => self.network,
            SystemIcon::Power => self.power,
        }
    }
}

/// Whose icon it is, as far as the caller could find out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner<'a> {
    /// The owner's program, by its full path.
    Path(&'a str),
    /// Only the file name could be had.
    Name(&'a str),
    /// Nothing is known.
    Unknown,
}

/// The GUIDs Explorer's own parts use for the system indicators, all of the family `{7820AE7x-23E3-4229-82C1-E41CB67D5B9C}`. They are
/// the ones another shell's source names for Explorer (ManagedShell, `NotificationArea.cs`: volume `...73`, network `...74`, power
/// `...75`; recorded in `notes/research/shell/2-tray-notifications.md`), NOT read from a block on this machine: the first real log of
/// the host (counts only) did not record identities. The rule is a table; a wrong row hides nothing that is not also owned by Windows.
pub fn system_guid(icon: SystemIcon) -> WireGuid {
    let last = match icon {
        SystemIcon::Volume => 0x73u32,
        SystemIcon::Network => 0x74,
        SystemIcon::Power => 0x75,
    };
    WireGuid::from_parts(0x7820_AE00 | last, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C])
}

/// The indicator a GUID names, if it is one of the three.
pub fn system_icon_of(guid: &WireGuid) -> Option<SystemIcon> {
    [SystemIcon::Volume, SystemIcon::Network, SystemIcon::Power].into_iter().find(|&i| system_guid(i) == *guid)
}

/// Whether `path` lies below `windows_dir` (`C:\Windows`): case blind, either slash, and only a real child (`C:\WindowsApps\x.exe` and
/// `C:\Windows.old\x.exe` are not below `C:\Windows`; neither is a path with `..` in it, which is never trusted).
pub fn below_dir(path: &str, windows_dir: &str) -> bool {
    let norm = |s: &str| s.trim().replace('/', "\\").to_lowercase();
    let (p, d) = (norm(path), norm(windows_dir).trim_end_matches('\\').to_string());
    if d.is_empty() || p.split('\\').any(|part| part == "..") {
        return false;
    }
    p.strip_prefix(&d).is_some_and(|rest| rest.len() > 1 && rest.starts_with('\\'))
}

/// Whether the owner is part of Windows (see the head). `windows_dir` is the Windows directory as the caller read it.
pub fn is_windows_own(owner: Owner<'_>, windows_dir: &str) -> bool {
    match owner {
        Owner::Path(p) => below_dir(p, windows_dir),
        Owner::Name(n) => n.trim().eq_ignore_ascii_case("explorer.exe"),
        Owner::Unknown => false,
    }
}

/// The indicator `guid` (`None`: the icon gave no GUID) duplicates, if it is a Windows-owned indicator whose button the dock shows.
/// `Some` means: leave the icon out of the strip and its panel.
pub fn duplicate_of(owner: Owner<'_>, guid: Option<&WireGuid>, windows_dir: &str, buttons: OwnButtons) -> Option<SystemIcon> {
    let icon = system_icon_of(guid?)?;
    (buttons.shows(icon) && is_windows_own(owner, windows_dir)).then_some(icon)
}

/// Whether the owner's path is worth looking up for an icon with this GUID: only the three indicators need an owner check, every
/// other icon is not asked about (the lookup opens a process).
pub fn needs_owner_check(guid: Option<&WireGuid>) -> bool {
    guid.is_some_and(|g| system_icon_of(g).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIN: &str = "C:\\Windows";
    const BOTH: OwnButtons = OwnButtons { volume: true, network: true, power: false };

    fn other_guid() -> WireGuid {
        WireGuid::from_parts(0xBA82_E2DC, 0xF405, 0x47AD, [0xB0, 0x32, 0xCF, 0x0F, 0xAA, 0x0E, 0x39, 0x33])
    }

    #[test]
    fn the_three_guids_are_the_family_and_tell_themselves_apart() {
        let s = |i| system_guid(i).to_string();
        assert_eq!(s(SystemIcon::Volume), "{7820AE73-23E3-4229-82C1-E41CB67D5B9C}");
        assert_eq!(s(SystemIcon::Network), "{7820AE74-23E3-4229-82C1-E41CB67D5B9C}");
        assert_eq!(s(SystemIcon::Power), "{7820AE75-23E3-4229-82C1-E41CB67D5B9C}");
        for i in [SystemIcon::Volume, SystemIcon::Network, SystemIcon::Power] {
            assert_eq!(system_icon_of(&system_guid(i)), Some(i));
        }
        assert_eq!(system_icon_of(&other_guid()), None);
        assert_eq!(system_icon_of(&WireGuid::default()), None);
        // the family's neighbours (hardware, health, update ...) are not indicators of the dock
        assert_eq!(system_icon_of(&WireGuid::from_parts(0x7820_AE78, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C])), None);
    }

    #[test]
    fn windows_own_volume_and_network_icons_go_while_the_dock_has_the_buttons() {
        let explorer = Owner::Path("C:\\Windows\\explorer.exe");
        assert_eq!(duplicate_of(explorer, Some(&system_guid(SystemIcon::Volume)), WIN, BOTH), Some(SystemIcon::Volume));
        assert_eq!(duplicate_of(explorer, Some(&system_guid(SystemIcon::Network)), WIN, BOTH), Some(SystemIcon::Network));
        // a program of Windows other than Explorer (the owner of a service object may be a host process)
        assert_eq!(duplicate_of(Owner::Path("c:\\windows\\system32\\sndvolsso-host.exe"), Some(&system_guid(SystemIcon::Volume)), WIN, BOTH), Some(SystemIcon::Volume));
    }

    #[test]
    fn only_while_the_dock_shows_that_button() {
        let explorer = Owner::Path("C:\\Windows\\explorer.exe");
        let vol = system_guid(SystemIcon::Volume);
        let net = system_guid(SystemIcon::Network);
        let no_volume = OwnButtons { volume: false, network: true, power: false };
        assert_eq!(duplicate_of(explorer, Some(&vol), WIN, no_volume), None, "no volume button: Windows' volume icon is the only one");
        assert_eq!(duplicate_of(explorer, Some(&net), WIN, no_volume), Some(SystemIcon::Network));
        assert_eq!(duplicate_of(explorer, Some(&vol), WIN, OwnButtons::default()), None);
        // the power indicator: the dock has no battery button, so it stays
        assert_eq!(duplicate_of(explorer, Some(&system_guid(SystemIcon::Power)), WIN, BOTH), None);
        let with_power = OwnButtons { power: true, ..BOTH };
        assert_eq!(duplicate_of(explorer, Some(&system_guid(SystemIcon::Power)), WIN, with_power), Some(SystemIcon::Power));
    }

    #[test]
    fn a_third_partys_audio_or_network_icon_stays() {
        let vol = system_guid(SystemIcon::Volume);
        // another program's icon: another owner (with or without the GUID), or another identity
        assert_eq!(duplicate_of(Owner::Path("C:\\Program Files\\Realtek\\Audio\\RtkNGUI64.exe"), Some(&vol), WIN, BOTH), None, "a driver that copied the GUID is still not Windows");
        assert_eq!(duplicate_of(Owner::Path("C:\\Program Files\\Realtek\\Audio\\RtkNGUI64.exe"), None, WIN, BOTH), None);
        assert_eq!(duplicate_of(Owner::Path("C:\\Windows\\explorer.exe"), Some(&other_guid()), WIN, BOTH), None, "an Explorer icon with another identity (hardware, security) stays");
        assert_eq!(duplicate_of(Owner::Path("C:\\Windows\\explorer.exe"), None, WIN, BOTH), None, "no GUID, no identity to name");
        assert_eq!(duplicate_of(Owner::Path("D:\\Tools\\explorer.exe"), Some(&vol), WIN, BOTH), None, "a program called explorer.exe elsewhere");
        assert_eq!(duplicate_of(Owner::Name("vpn.exe"), Some(&system_guid(SystemIcon::Network)), WIN, BOTH), None);
    }

    #[test]
    fn an_owner_that_cannot_be_told_keeps_the_icon() {
        let vol = system_guid(SystemIcon::Volume);
        assert_eq!(duplicate_of(Owner::Unknown, Some(&vol), WIN, BOTH), None);
        assert_eq!(duplicate_of(Owner::Name("EXPLORER.EXE"), Some(&vol), WIN, BOTH), Some(SystemIcon::Volume), "the file name alone: Explorer, any case");
        assert_eq!(duplicate_of(Owner::Name(""), Some(&vol), WIN, BOTH), None);
    }

    #[test]
    fn below_the_windows_directory_means_below_it() {
        assert!(below_dir("C:\\Windows\\explorer.exe", "C:\\Windows"));
        assert!(below_dir("c:/windows/System32/x.exe", "C:\\WINDOWS\\"));
        assert!(!below_dir("C:\\WindowsApps\\x.exe", "C:\\Windows"));
        assert!(!below_dir("C:\\Windows.old\\x.exe", "C:\\Windows"));
        assert!(!below_dir("C:\\Windows", "C:\\Windows"), "the directory itself is not a program in it");
        assert!(!below_dir("C:\\Windows\\..\\Evil\\x.exe", "C:\\Windows"));
        assert!(!below_dir("C:\\Windows\\x.exe", ""));
        assert!(!below_dir("", "C:\\Windows"));
    }

    #[test]
    fn only_the_indicators_are_worth_a_process_lookup() {
        assert!(needs_owner_check(Some(&system_guid(SystemIcon::Volume))));
        assert!(!needs_owner_check(Some(&other_guid())));
        assert!(!needs_owner_check(None));
    }

    #[test]
    fn the_rule_never_looks_at_text_or_pictures() {
        // (the signature is the proof: owner, GUID, directory, buttons. This keeps it that way.)
        let src = include_str!("own_icons.rs").split("#[cfg(test)]").next().unwrap_or("").to_string();
        let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
        for banned in ["tip", "image", "bitmap", "pixels"] {
            assert!(!code.to_lowercase().contains(banned), "the rule mentions {banned}");
        }
    }
}
