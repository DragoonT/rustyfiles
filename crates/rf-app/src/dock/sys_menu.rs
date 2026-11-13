//! The Start button's own menu: what a right click on the logo offers.
//!
//! A user's report: a right click on our Start button gave nothing, where Windows' gives its list of system tools ("Win+X"), and
//! ours should be at least that. So it is that list and a little more: the tools Windows' own menu has (installed apps, power
//! options, system, Device Manager, network connections, Disk Management, Computer Management, Event Viewer, Terminal, Task
//! Manager, Settings, Run), plus Services, the Control Panel and rustyfiles' own settings and file manager, and the power menu.
//!
//! The menu is drawn INSIDE the Start panel (like its power menu and its app menu: a menu window of its own would take the
//! activation and close the panel), at the bottom left, above the logo. This file is the list and what each row does; the panel
//! (`start.rs`) draws and runs it.

use std::sync::atomic::{AtomicBool, Ordering};

use gpui_kit::assets::IconName as L;

use crate::t;

/// What a row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// A page of Windows' Settings, or another URI the shell opens.
    Uri(&'static str),
    /// A tool the shell starts by its file name (a console, a Control Panel file, a program on the search path).
    Open(&'static str),
    /// Windows Terminal with administrator rights (Windows shows its consent prompt: the user asked with this click).
    TerminalAdmin,
    TaskManager,
    /// rustyfiles' own Settings window.
    OwnSettings,
    /// A rustyfiles window.
    Files,
    /// The panel's power menu (lock, sleep, sign out, restart, shut down).
    Power,
}

#[derive(Clone, Copy, Debug)]
pub struct Row {
    pub label: &'static str,
    pub icon: L,
    pub act: Act,
    /// Rows of one group stand together; a hairline separates the groups.
    pub group: u8,
}

/// The menu, top to bottom.
pub fn rows() -> Vec<Row> {
    let r = |label, icon, act, group| Row { label, icon, act, group };
    vec![
        r(t!("Yüklü uygulamalar"), L::AppWindow, Act::Uri("ms-settings:appsfeatures"), 0),
        r(t!("Güç seçenekleri"), L::Zap, Act::Uri("ms-settings:powersleep"), 0),
        r(t!("Sistem"), L::Monitor, Act::Uri("ms-settings:about"), 0),
        r(t!("Aygıt Yöneticisi"), L::Cpu, Act::Open("devmgmt.msc"), 0),
        r(t!("Ağ bağlantıları"), L::Network, Act::Open("ncpa.cpl"), 0),
        r(t!("Disk Yönetimi"), L::HardDrive, Act::Open("diskmgmt.msc"), 0),
        r(t!("Bilgisayar Yönetimi"), L::Server, Act::Open("compmgmt.msc"), 0),
        r(t!("Olay Görüntüleyicisi"), L::ScrollText, Act::Open("eventvwr.msc"), 0),
        r(t!("Hizmetler"), L::Cog, Act::Open("services.msc"), 0),
        r(t!("Terminal"), L::Terminal, Act::Open("wt.exe"), 1),
        r(t!("Terminal (Yönetici)"), L::ShieldCheck, Act::TerminalAdmin, 1),
        r(t!("Görev Yöneticisi"), L::Activity, Act::TaskManager, 1),
        r(t!("Denetim Masası"), L::SlidersHorizontal, Act::Open("control.exe"), 1),
        r(t!("Windows Ayarları"), L::Settings, Act::Uri("ms-settings:"), 1),
        r(t!("rustyfiles ayarları"), L::Settings2, Act::OwnSettings, 1),
        r(t!("Dosya yöneticisi"), L::Folder, Act::Files, 2),
        // (the shell's own Run dialog)
        r(t!("Çalıştır"), L::Play, Act::Open("shell:::{2559a1f3-21d7-11d4-bdaf-00c04f60b9f0}"), 2),
        r(t!("Güç"), L::Power, Act::Power, 3),
    ]
}

/// The menu's height for these rows: the rows, a hairline with its room between two groups, and the padding.
pub fn height(rows: &[Row], row_h: f32) -> f32 {
    let groups = rows.windows(2).filter(|w| w[0].group != w[1].group).count();
    rows.len() as f32 * row_h + groups as f32 * SEPARATOR_H + 12.0
}

/// A hairline between two groups and the room around it.
pub const SEPARATOR_H: f32 = 9.0;

/// The Start panel is asked to come up with this menu open (a right click on the logo while the panel is closed: the panel is
/// opened first, and reads this when it is shown).
static WANTED: AtomicBool = AtomicBool::new(false);

pub fn want() {
    WANTED.store(true, Ordering::Relaxed);
}

pub fn take_wanted() -> bool {
    WANTED.swap(false, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_has_windows_own_tools_and_ours_in_groups_and_no_row_twice() {
        let rows = rows();
        // (the labels are looked up in the display language of the moment: compare what does not depend on it)
        let acts: Vec<Act> = rows.iter().map(|r| r.act).collect();
        for want in [Act::Open("devmgmt.msc"), Act::Open("diskmgmt.msc"), Act::Open("compmgmt.msc"), Act::Open("eventvwr.msc"), Act::Open("ncpa.cpl"), Act::Uri("ms-settings:appsfeatures"), Act::Uri("ms-settings:"), Act::TaskManager, Act::TerminalAdmin, Act::OwnSettings, Act::Files, Act::Power] {
            assert!(acts.contains(&want), "{want:?}");
        }
        for (i, a) in acts.iter().enumerate() {
            assert!(!acts[..i].contains(a), "{a:?} twice");
        }
        // the groups only go up, and the power row is the last one, on its own
        assert!(rows.windows(2).all(|w| w[0].group <= w[1].group));
        assert_eq!(rows.last().map(|r| (r.act, r.group)), Some((Act::Power, 3)));
        assert_eq!(rows.iter().filter(|r| r.group == 3).count(), 1);
        assert!(rows.iter().all(|r| !r.label.is_empty()));
    }

    #[test]
    fn the_height_counts_the_rows_the_groups_and_the_padding() {
        let rows = rows();
        assert_eq!(height(&rows, 34.0), rows.len() as f32 * 34.0 + 3.0 * SEPARATOR_H + 12.0);
        assert_eq!(height(&rows[..2], 34.0), 2.0 * 34.0 + 12.0);
    }

    #[test]
    fn the_wish_is_taken_once() {
        want();
        assert!(take_wanted());
        assert!(!take_wanted());
    }
}
