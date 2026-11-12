//! The app's own choices for the desktop's system icons (Settings, "Masaüstü" page), as the desktop reads them.
//!
//! Which of This PC, Recycle Bin, Network, the user's folder and Control Panel are on the desktop used to be Windows' business alone
//! (`HideDesktopIcons`, which only Windows' own Settings page writes). Now each has a setting of ours with three states, "follow
//! Windows" (the default: nothing changes), "show" and "hide", and so does "show desktop icons" as a whole. The rules are
//! `rf_core::desktop::virtuals::apply` (pure, tested there); this file only carries the current choices to where the desktop reads
//! them: a process-wide cell, because the desktop's item source is built on a worker thread and asks for them at every read.
//!
//! Besides the settings there is ONE more way to hide the icons: the desktop's own right-click menu ("Masaüstü simgelerini göster"),
//! whose state lives in the layout file (`Store::show_icons`) and is kept here as `menu_hidden`. Either one hiding them hides them.
//! Nothing here touches the registry.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use parking_lot::RwLock;
use rf_core::desktop::virtuals::{Choices, IconChoice, VirtualKind};

use crate::settings::Settings;

static SETTINGS: RwLock<Choices> = RwLock::new(Choices { kinds: [IconChoice::Follow; 5], show_icons: IconChoice::Follow });
static MENU_HIDDEN: AtomicBool = AtomicBool::new(false);

/// The choices the settings hold.
pub fn from_settings(s: &Settings) -> Choices {
    let mut c = Choices { show_icons: s.desktop_show_icons, ..Default::default() };
    c.set(VirtualKind::ThisPc, s.desktop_icon_this_pc);
    c.set(VirtualKind::UserFiles, s.desktop_icon_user_files);
    c.set(VirtualKind::Network, s.desktop_icon_network);
    c.set(VirtualKind::RecycleBin, s.desktop_icon_recycle_bin);
    c.set(VirtualKind::ControlPanel, s.desktop_icon_control_panel);
    c
}

/// The setting field of each kind (used by the Settings page and its tests, which check that every control writes its own field).
pub fn get(s: &Settings, k: VirtualKind) -> IconChoice {
    from_settings(s).of(k)
}

pub fn set(s: &mut Settings, k: VirtualKind, c: IconChoice) {
    match k {
        VirtualKind::ThisPc => s.desktop_icon_this_pc = c,
        VirtualKind::UserFiles => s.desktop_icon_user_files = c,
        VirtualKind::Network => s.desktop_icon_network = c,
        VirtualKind::RecycleBin => s.desktop_icon_recycle_bin = c,
        VirtualKind::ControlPanel => s.desktop_icon_control_panel = c,
    }
}

/// What "hide all icons" comes to when the menu's own switch is also considered: hidden by the menu is hidden, whatever the setting
/// says; otherwise the setting's choice stands. Pure.
pub fn effective(settings: Choices, menu_hidden: bool) -> Choices {
    if menu_hidden { Choices { show_icons: IconChoice::Hide, ..settings } } else { settings }
}

/// The choices to apply now.
pub fn current() -> Choices {
    effective(*SETTINGS.read(), MENU_HIDDEN.load(Ordering::Relaxed))
}

/// The same as a function the item source can hold.
pub fn source() -> Arc<dyn Fn() -> Choices + Send + Sync> {
    Arc::new(current)
}

/// Takes the choices from the settings. `true` when they changed (the desktop must read its items again).
pub fn update_from_settings(s: &Settings) -> bool {
    let new = from_settings(s);
    let mut cur = SETTINGS.write();
    if *cur == new {
        return false;
    }
    *cur = new;
    true
}

/// The menu's "show desktop icons" switch (from the layout file at load, from the menu after that).
pub fn set_menu_hidden(hidden: bool) -> bool {
    MENU_HIDDEN.swap(hidden, Ordering::Relaxed) != hidden
}

pub fn menu_hidden() -> bool {
    MENU_HIDDEN.load(Ordering::Relaxed)
}

/// Whether the settings alone hide every icon (then the menu's switch has nothing to offer).
pub fn hidden_by_settings() -> bool {
    SETTINGS.read().show_icons == IconChoice::Hide
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_follow_windows_everywhere() {
        let c = from_settings(&Settings::default());
        assert!(c.is_default(), "an untouched profile changes nothing about the desktop");
    }

    #[test]
    fn each_setting_reaches_its_own_kind() {
        for k in VirtualKind::ALL {
            let mut s = Settings::default();
            set(&mut s, k, IconChoice::Show);
            let c = from_settings(&s);
            for other in VirtualKind::ALL {
                assert_eq!(c.of(other), if other == k { IconChoice::Show } else { IconChoice::Follow }, "{k:?} changed {other:?}");
            }
            assert_eq!(get(&s, k), IconChoice::Show);
            assert_eq!(c.show_icons, IconChoice::Follow);
        }
        let s = Settings { desktop_show_icons: IconChoice::Hide, ..Default::default() };
        assert_eq!(from_settings(&s).show_icons, IconChoice::Hide);
    }

    #[test]
    fn the_menu_can_hide_the_icons_but_never_show_what_the_settings_hide() {
        let hide = Choices { show_icons: IconChoice::Hide, ..Default::default() };
        let show = Choices { show_icons: IconChoice::Show, ..Default::default() };
        let follow = Choices::default();
        assert_eq!(effective(follow, false), follow);
        assert_eq!(effective(show, false), show);
        assert_eq!(effective(follow, true).show_icons, IconChoice::Hide);
        assert_eq!(effective(show, true).show_icons, IconChoice::Hide, "hidden from the menu stays hidden even when the setting says show");
        assert_eq!(effective(hide, false).show_icons, IconChoice::Hide);
        // the kinds are not touched by it
        let mut c = Choices::default();
        c.set(VirtualKind::ThisPc, IconChoice::Show);
        assert_eq!(effective(c, true).of(VirtualKind::ThisPc), IconChoice::Show);
    }

    #[test]
    fn the_settings_are_carried_over_once_and_a_second_identical_update_is_no_change() {
        // (the cell is process-wide and other tests run in this process: this test leaves it as it found it)
        let before = *SETTINGS.read();
        let mut s = Settings::default();
        set(&mut s, VirtualKind::Network, IconChoice::Show);
        update_from_settings(&s);
        assert!(!update_from_settings(&s), "nothing changed the second time");
        assert_eq!(current().of(VirtualKind::Network), IconChoice::Show);
        *SETTINGS.write() = before;
    }
}
