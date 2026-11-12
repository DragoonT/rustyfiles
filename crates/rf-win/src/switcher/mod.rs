//! Our own window switcher (shell plan increment G1): what Alt+Tab shows, in the order it shows it, and the pieces that choose a
//! window. Windows' own switcher lives in `explorer.exe` and goes with it, so the shell has to bring one; beside Explorer this one
//! can be tried on a command (`rustyfiles --switcher-preview`) while the real Alt+Tab stays Windows'.
//!
//! | file | what it is | pure core |
//! |---|---|---|
//! | `rules` | which windows are listed: the Alt+Tab owner / last-popup walk, tool windows, cloaked windows split into "another desktop" and "suspended app", our own windows left out | all of it, tested with made-up facts |
//! | `source` | the facts read from Windows (`EnumWindows`, DWM cloak bits, `IVirtualDesktopManager`), the mark on our shell windows | - (read only) |
//! | `chord` | Alt held + Tab / Shift+Tab / arrows / Enter / Escape / Alt released / the same-app key, as a state machine over key events | all of it |
//! | `select` | which card is selected, Tab and the arrows over a grid, the "windows of this app" scope | all of it |
//! | `session` | one opening as a value: the windows shown, the selection, what each effect of the chord does | all of it |
//! | `layout` | the panel's size and every card's rectangles, up to about 12 windows at full size, more scale the cards down | all of it |
//! | `thumbs` | DWM live pictures behind a trait; minimized windows get an icon; everything registered is unregistered on close | the order of calls, the fitting |
//! | `activate` | bringing the chosen window to the front: restore a minimized one, foreground, once more through the thread-input route, flash | the order of calls |
//! | `watch` | when an open switcher puts itself away: a click outside, a foreground that stays somebody else's | all of it |
//! | `hook` | the keyboard feed that will drive the chord in the shell host: a trait, the real `WH_KEYBOARD_LL` hook (NOT installed by anything in this track) and a fake | the key translation |
//!
//! WHAT RUNS, AND WHEN. Nothing exists while the switcher is closed: no window, no thumbnails, no timer, no hook. (While it is
//! open, a 50 ms look at the foreground and the mouse runs, [`watch`]; it ends with the window.) Opening costs one
//! walk of the top-level windows ([`list`]: `EnumWindows` and a handful of cheap calls per window, one COM round trip only when a
//! shell-cloaked window exists), the creation of the panel's window by the app, and one `DwmRegisterThumbnail` per live card. The
//! numbers measured for the listing are printed by the `switcher_probe` example.

pub mod activate;
pub mod chord;
pub mod hook;
pub mod layout;
pub mod rules;
pub mod select;
pub mod session;
pub mod source;
pub mod thumbs;
pub mod watch;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::dock::model::{IconSource, looks_packaged};

pub use rules::{Counts, Desktop, Options};
pub use source::{ComRoute, is_shell_window, mark_shell_window, monitor_at_cursor, unmark_shell_window};

/// One window of the switcher's list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchItem {
    pub hwnd: isize,
    pub title: String,
    pub pid: u32,
    /// The program, when it can be read.
    pub exe: Option<PathBuf>,
    /// The AppUserModelID of the process's package, for a Store app.
    pub app_id: Option<String>,
    /// What "windows of this app" groups by: the package, else the program, else the process.
    pub group: String,
    pub minimized: bool,
    /// A Store app that is suspended on this desktop: listed, no live picture.
    pub suspended: bool,
    /// On another virtual desktop (only listed when asked for).
    pub other_desktop: bool,
}

impl SwitchItem {
    /// Whether the card shows the app's icon in place of a live picture: DWM has nothing to show of a window that is minimized or
    /// suspended.
    pub fn no_live_picture(&self) -> bool {
        self.minimized || self.suspended || self.other_desktop
    }

    /// Everything the icon can come from, best first (the dock's chain: the app's own, then what its window set). A packaged app's
    /// program lies under `WindowsApps`, which the shell cannot read an icon from, so its package comes first and its program is not tried.
    pub fn icon_chain(&self) -> Vec<IconSource> {
        let mut chain = Vec::new();
        match (&self.app_id, &self.exe) {
            (Some(id), _) if looks_packaged(id) => chain.push(IconSource::Packaged(id.clone())),
            (_, Some(exe)) => chain.push(IconSource::File(exe.clone())),
            _ => {}
        }
        chain.push(IconSource::Window(self.hwnd));
        chain
    }

    /// The key of this card's icon bitmap at `side` px: where it comes from and how big it was drawn (a bitmap made for a 100 %
    /// monitor is blurry on a 150 % one).
    pub fn icon_key(&self, side: u32) -> String {
        format!("{:?}@{side}", self.icon_chain().first())
    }

    /// Whether the icon's key means the same thing at the next opening: a program or a package does; a window's own handle does
    /// not (Windows hands a destroyed window's handle value to another one), so that icon is asked for again every time.
    pub fn icon_key_is_stable(&self) -> bool {
        !matches!(self.icon_chain().first(), Some(IconSource::Window(_)) | None)
    }
}

fn group_of(app_id: &Option<String>, exe: &Option<PathBuf>, pid: u32) -> String {
    match (app_id, exe) {
        (Some(id), _) => id.to_lowercase(),
        (None, Some(exe)) => exe.to_string_lossy().to_lowercase(),
        (None, None) => format!("pid:{pid}"),
    }
}

/// How long each part of a listing took.
#[derive(Clone, Copy, Debug, Default)]
pub struct Timings {
    pub facts: Duration,
    pub desktops: Duration,
    pub items: Duration,
    pub total: Duration,
}

/// The switcher's list and what was left out.
#[derive(Clone, Debug)]
pub struct Listing {
    /// Most recently used first (see `rules` for how close the z-order comes to that); the window in front is the first when
    /// `front_listed`.
    pub items: Vec<SwitchItem>,
    /// Whether the window in front is one of the listed windows (and so the first). False when the foreground is the desktop, our
    /// dock or a panel: then the first card is a window the user has NOT been in, and the first Tab must not skip it.
    pub front_listed: bool,
    pub counts: Counts,
    pub took: Timings,
}

/// The switchable windows now. Blocks for a few milliseconds (and up to [`source::DESKTOP_LIMIT`] when the shell thread is busy
/// and a shell-cloaked window exists): call it from the thread that opens the switcher, never in a loop.
pub fn list(route: ComRoute, opts: Options) -> Listing {
    let t0 = Instant::now();
    let facts = source::gather();
    let t_facts = t0.elapsed();

    let t1 = Instant::now();
    let desktops: HashMap<isize, Desktop> = source::desktops(rules::needing_desktop(&facts), route);
    let t_desktops = t1.elapsed();

    let t2 = Instant::now();
    let (mut chosen, counts) = rules::select(&facts, &desktops, opts);
    let front_listed = rules::promote_foreground(&mut chosen, &facts, crate::dock::foreground_root());
    let mut exes: HashMap<u32, (Option<PathBuf>, Option<String>)> = HashMap::new();
    let items = chosen
        .iter()
        .map(|c| {
            let f = &facts[c.index];
            let pid = source::app_process(f.hwnd, &f.class, f.pid);
            let (exe, app_id) = exes.entry(pid).or_insert_with(|| (source::exe_of(pid), crate::dock::identity::process_aumid(pid))).clone();
            SwitchItem {
                hwnd: f.hwnd,
                title: f.title.clone(),
                pid,
                group: group_of(&app_id, &exe, pid),
                exe,
                app_id,
                minimized: f.minimized,
                suspended: c.suspended,
                other_desktop: c.other_desktop,
            }
        })
        .collect();
    let t_items = t2.elapsed();
    Listing { items, front_listed, counts, took: Timings { facts: t_facts, desktops: t_desktops, items: t_items, total: t0.elapsed() } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(app_id: Option<&str>, exe: Option<&str>) -> SwitchItem {
        SwitchItem {
            hwnd: 9,
            title: "t".into(),
            pid: 1,
            exe: exe.map(PathBuf::from),
            app_id: app_id.map(String::from),
            group: String::new(),
            minimized: false,
            suspended: false,
            other_desktop: false,
        }
    }

    #[test]
    fn the_icon_chain_is_the_apps_own_then_its_window() {
        let plain = item(None, Some(r"C:\x\a.exe"));
        assert_eq!(plain.icon_chain(), vec![IconSource::File(PathBuf::from(r"C:\x\a.exe")), IconSource::Window(9)]);
        let none = item(None, None);
        assert_eq!(none.icon_chain(), vec![IconSource::Window(9)]);
    }

    #[test]
    fn a_packaged_app_asks_for_its_package_not_for_its_program_under_windowsapps() {
        let pk = item(Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), Some(r"C:\Program Files\WindowsApps\x\calc.exe"));
        assert_eq!(pk.icon_chain(), vec![IconSource::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()), IconSource::Window(9)]);
    }

    #[test]
    fn an_icon_is_kept_by_where_it_comes_from_and_how_big_it_was_drawn() {
        let a = item(None, Some(r"C:\x\a.exe"));
        assert_ne!(a.icon_key(32), a.icon_key(48), "another size is another bitmap");
        assert_eq!(a.icon_key(32), item(None, Some(r"C:\x\a.exe")).icon_key(32));
        assert_ne!(a.icon_key(32), item(None, Some(r"C:\x\b.exe")).icon_key(32));
        assert!(a.icon_key_is_stable());
        let pk = item(Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), None);
        assert!(pk.icon_key_is_stable());
    }

    #[test]
    fn an_icon_that_comes_from_a_window_handle_is_not_kept_for_the_next_opening() {
        // a window's handle value is reused by Windows: the card of another app must not inherit this one's icon
        let w = item(None, None);
        assert!(!w.icon_key_is_stable());
        let other = SwitchItem { hwnd: 77, ..item(None, None) };
        assert_ne!(w.icon_key(32), other.icon_key(32));
    }

    /// No key is ever sent into another program's window (the "Alt tap" trick lights up the menu bar of the program in front), no
    /// hotkey is registered, no message is posted to a window of another program. A scan of the code, so that adding one later fails
    /// here; a fake's log could not show it. Only the code before each file's tests is read, and comment lines are skipped.
    #[test]
    fn nothing_in_the_switcher_sends_a_key_or_registers_a_hotkey() {
        let files = [
            ("activate.rs", include_str!("activate.rs")),
            ("source.rs", include_str!("source.rs")),
            ("thumbs.rs", include_str!("thumbs.rs")),
            ("rules.rs", include_str!("rules.rs")),
            ("session.rs", include_str!("session.rs")),
            ("layout.rs", include_str!("layout.rs")),
            ("select.rs", include_str!("select.rs")),
            ("chord.rs", include_str!("chord.rs")),
            ("hook.rs", include_str!("hook.rs")),
            ("watch.rs", include_str!("watch.rs")),
            ("mod.rs", include_str!("mod.rs")),
        ];
        for (name, text) in files {
            let code: String =
                text.split("#[cfg(test)]").next().unwrap_or(text).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            for call in ["SendInput", "keybd_event", "PostMessageW", "SendMessageW", "SendNotifyMessage", "RegisterHotKey"] {
                assert!(!code.contains(call), "{name} uses {call}");
            }
        }
    }

    #[test]
    fn groups_are_the_package_then_the_program_case_insensitively() {
        assert_eq!(group_of(&Some("Pkg_abc!App".into()), &Some(PathBuf::from("C:\\A.exe")), 1), "pkg_abc!app");
        assert_eq!(group_of(&None, &Some(PathBuf::from(r"C:\A\Chrome.exe")), 1), r"c:\a\chrome.exe");
        assert_eq!(group_of(&None, &None, 7), "pid:7");
    }

    #[test]
    fn minimized_suspended_and_other_desktop_windows_have_no_live_picture() {
        let mut i = item(None, None);
        assert!(!i.no_live_picture());
        i.minimized = true;
        assert!(i.no_live_picture());
        i.minimized = false;
        i.suspended = true;
        assert!(i.no_live_picture());
        i.suspended = false;
        i.other_desktop = true;
        assert!(i.no_live_picture());
    }

    /// Read-only, on whatever is open: the list obeys its own contract.
    #[test]
    fn the_real_list_obeys_the_rules() {
        let l = list(ComRoute::CallingThread, Options::default());
        assert_eq!(l.counts.listed, l.items.len());
        assert_eq!(l.counts.minimized, l.items.iter().filter(|i| i.minimized).count());
        let mut seen = std::collections::HashSet::new();
        for i in &l.items {
            assert!(seen.insert(i.hwnd), "a window is listed once");
            // (a window that closed between the listing and this look has lost its process: that is not what is checked here)
            assert!(i.pid != 0 || !crate::window::exists(i.hwnd));
            assert!(!i.other_desktop, "other desktops are left out by default");
        }
    }
}
