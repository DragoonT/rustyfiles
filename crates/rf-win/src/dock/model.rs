//! What the middle island shows and what a click on it does. Pure: the pinned apps, the running windows and the foreground window
//! are arguments, so the merge, its order and the click decisions are tested without a window.

use std::path::PathBuf;

use crate::tasklist::Task;

/// How to start an app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launch {
    /// Open this shortcut (`.lnk`) like Explorer does: its arguments, working directory and "run as" come along.
    Link(PathBuf),
    /// Start this program.
    Exe { path: PathBuf, args: String },
    /// A packaged (Store) app by its AppUserModelID.
    Packaged(String),
}

/// Where an app's icon comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconSource {
    /// A program, a document or a folder on disk: the shell's own icon for it.
    File(PathBuf),
    /// An icon stored in a file (a shortcut's own icon location).
    Resource { file: PathBuf, index: i32 },
    /// A packaged app by its AppUserModelID (`shell:AppsFolder`).
    Packaged(String),
    /// A shell place by its parsing name (`shell:RecycleBinFolder`).
    Shell(String),
    /// The icon a running window has set itself (a program with no file the shell can ask).
    Window(isize),
}

/// An app pinned to the Windows taskbar (read from the taskbar's own pin folder, never written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinnedApp {
    /// The shortcut's name without `.lnk`, or the packaged app's display name.
    pub name: String,
    /// What a running window is matched against (see [`pin_matches`]): the AppUserModelID, lower case, when the pin has one.
    pub aumid: Option<String>,
    /// The program the pin starts, lower case, when it has one that exists.
    pub exe: Option<String>,
    pub launch: Launch,
    pub icon: IconSource,
}

/// A place the dock links to after the apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Place {
    RecycleBin,
    Downloads,
}

impl Place {
    /// The shell parsing name the icon is asked with.
    pub fn icon_source(self) -> IconSource {
        match self {
            Place::RecycleBin => IconSource::Shell("shell:RecycleBinFolder".into()),
            Place::Downloads => match crate::known::KnownFolder::Downloads.path() {
                Some(p) => IconSource::File(p),
                None => IconSource::Shell("shell:Downloads".into()),
            },
        }
    }

    /// The argument rustyfiles opens the place with (the same strings its command line takes): the Recycle Bin's shell name, the
    /// Downloads folder's path.
    pub fn open_argument(self) -> String {
        match self {
            Place::RecycleBin => "shell:RecycleBinFolder".into(),
            Place::Downloads => crate::known::KnownFolder::Downloads.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_else(|| "shell:Downloads".into()),
        }
    }
}

/// One app in the dock, pinned or only running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockApp {
    /// A stable identity (lower case): the AppUserModelID, else the program path, else the window's process.
    pub key: String,
    pub name: String,
    pub pinned: bool,
    /// How to start it; `None` for a running program that cannot be started again (no known program).
    pub launch: Option<Launch>,
    pub icon: IconSource,
    /// Its task windows, the one used last first (the order [`crate::tasklist::tasks`] gives).
    pub windows: Vec<isize>,
    /// The foreground window is one of them.
    pub active: bool,
}

impl DockApp {
    pub fn running(&self) -> bool {
        !self.windows.is_empty()
    }

    /// Everything its icon can come from, best first: the icon the app was given, then the icon one of its task windows has set itself
    /// (a few windows are tried: the first may be a splash screen with none), then, for a packaged app, the logo of its package. The
    /// dock asks for them in turn and draws a letter only when none gives a picture.
    pub fn icon_chain(&self) -> Vec<IconSource> {
        let mut chain = vec![self.icon.clone()];
        // the same package under another letter case (the key is lower case) is the same logo
        let same = |a: &IconSource, b: &IconSource| match (a, b) {
            (IconSource::Packaged(x), IconSource::Packaged(y)) => x.eq_ignore_ascii_case(y),
            _ => a == b,
        };
        let mut add = |s: IconSource| {
            if !chain.iter().any(|c| same(c, &s)) {
                chain.push(s);
            }
        };
        for &w in self.windows.iter().take(3) {
            add(IconSource::Window(w));
        }
        if looks_packaged(&self.key) {
            add(IconSource::Packaged(self.key.clone()));
        }
        chain
    }
}

/// An item of the middle island, left to right.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DockItem {
    App(DockApp),
    /// The thin line between the apps and the places.
    Separator,
    Place(Place),
}

/// What a click does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickAction {
    /// Not running: start it.
    Start,
    /// Running but not in front: bring this window (its last used one) to the front, restoring it if minimised.
    Raise(isize),
    /// In front: minimise this window.
    Minimize(isize),
}

/// The decision for a click on an app whose task windows are `windows` (last used first) while `foreground` is in front.
pub fn click_action(windows: &[isize], foreground: isize) -> ClickAction {
    match windows.first() {
        None => ClickAction::Start,
        Some(&first) => {
            if windows.contains(&foreground) {
                ClickAction::Minimize(foreground)
            } else {
                ClickAction::Raise(first)
            }
        }
    }
}

/// A middle click: another instance, whether or not one is running.
pub fn middle_click_action() -> ClickAction {
    ClickAction::Start
}

/// Whether a pinned app is the same app as a running group. `group` is [`Task::group`] (lower case AppUserModelID, else the
/// program path; the windows of a packaged app are given their process's package id first, see [`super::identity`]), `tasks` the
/// group's windows. They match when the identities agree, or - a pin with no AppUserModelID of its own, or
/// a window that sets another one than its shortcut carries (a program that names itself at run time) - when the program is the
/// same.
pub fn pin_matches(pin: &PinnedApp, group: &str, tasks: &[Task]) -> bool {
    if let Some(id) = &pin.aumid
        && id == group
    {
        return true;
    }
    if let Some(exe) = &pin.exe {
        if exe == group {
            return true;
        }
        return tasks.iter().any(|t| t.exe.as_ref().is_some_and(|p| p.to_string_lossy().to_lowercase() == *exe));
    }
    false
}

/// The AppUserModelID of a packaged app looks like `Package.Name_publisherid!AppId`.
pub fn looks_packaged(aumid: &str) -> bool {
    let Some((family, app)) = aumid.split_once('!') else { return false };
    !app.is_empty() && family.contains('_') && !aumid.contains(char::is_whitespace) && !aumid.contains(['\\', '/', ':'])
}

/// What an app's jump list (and its names) are looked up by: its AppUserModelID when it has one that a jump-list file is named by
/// (the launch is a packaged app, or the key looks like one), and its program when it has one on disk (the launch, else an icon that
/// is a `.exe`). The app menu and the probe that measures it both read this, so a count taken by the probe is the menu's.
pub fn jump_identity(app: &DockApp) -> (Option<String>, Option<PathBuf>) {
    let aumid = match &app.launch {
        Some(Launch::Packaged(id)) => Some(id.clone()),
        _ if looks_packaged(&app.key) => Some(app.key.clone()),
        _ => None,
    };
    let exe = match &app.launch {
        Some(Launch::Exe { path, .. }) => Some(path.clone()),
        _ => match &app.icon {
            IconSource::File(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) => Some(p.clone()),
            _ => None,
        },
    };
    (aumid, exe)
}

/// A name for a running app that has no pin to take it from: the program's file name, else the first window's title.
pub fn fallback_name(tasks: &[Task]) -> String {
    let t = tasks.first();
    if let Some(stem) = t.and_then(|t| t.exe.as_ref()).and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()) {
        let mut c = stem.chars();
        return match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => stem,
        };
    }
    if let Some(t) = t {
        if !t.title.is_empty() {
            return t.title.clone();
        }
        if let Some(id) = &t.app_id {
            return id.clone();
        }
    }
    String::new()
}

fn running_app(group: &str, tasks: &[Task], foreground: isize) -> DockApp {
    let first = &tasks[0];
    let packaged = first.app_id.as_deref().filter(|id| looks_packaged(id));
    let launch = match (packaged, &first.exe) {
        (Some(id), _) => Some(Launch::Packaged(id.to_string())),
        (None, Some(exe)) => Some(Launch::Exe { path: exe.clone(), args: String::new() }),
        (None, None) => None,
    };
    let icon = match (packaged, &first.exe) {
        (Some(id), _) => IconSource::Packaged(id.to_string()),
        (None, Some(exe)) => IconSource::File(exe.clone()),
        (None, None) => IconSource::Window(first.hwnd),
    };
    let windows: Vec<isize> = tasks.iter().map(|t| t.hwnd).collect();
    DockApp { key: group.to_string(), name: fallback_name(tasks), pinned: false, launch, icon, active: windows.contains(&foreground), windows }
}

/// The middle island's items: the pinned apps in their given order (each with its windows when it is running), then the running apps
/// that are not pinned in the order of `groups` (the order the windows first appeared in), then a separator, then the Recycle Bin
/// and the Downloads folder. A group is taken by the first pin that matches it and by no other.
pub fn merge(pins: &[PinnedApp], groups: Vec<(String, Vec<Task>)>, foreground: isize) -> Vec<DockItem> {
    let mut groups: Vec<Option<(String, Vec<Task>)>> = groups.into_iter().filter(|(_, t)| !t.is_empty()).map(Some).collect();
    let mut items = Vec::new();
    for pin in pins {
        let hit = groups.iter().position(|g| g.as_ref().is_some_and(|(k, t)| pin_matches(pin, k, t)));
        let (key, windows) = match hit {
            Some(i) => {
                let (k, t) = groups[i].take().unwrap_or_default();
                (k, t.iter().map(|t| t.hwnd).collect::<Vec<_>>())
            }
            None => (pin.aumid.clone().or_else(|| pin.exe.clone()).unwrap_or_else(|| pin.name.to_lowercase()), Vec::new()),
        };
        items.push(DockItem::App(DockApp {
            key,
            name: pin.name.clone(),
            pinned: true,
            launch: Some(pin.launch.clone()),
            icon: pin.icon.clone(),
            active: windows.contains(&foreground),
            windows,
        }));
    }
    for (k, t) in groups.into_iter().flatten() {
        items.push(DockItem::App(running_app(&k, &t, foreground)));
    }
    items.push(DockItem::Separator);
    items.push(DockItem::Place(Place::RecycleBin));
    items.push(DockItem::Place(Place::Downloads));
    items
}

/// The `hover` slots of an item list: one per item, icons for apps and places, a separator for the line.
pub fn slots(items: &[DockItem]) -> Vec<super::hover::Slot> {
    items.iter().map(|i| if matches!(i, DockItem::Separator) { super::hover::Slot::Separator } else { super::hover::Slot::Icon }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(hwnd: isize, exe: &str, id: Option<&str>) -> Task {
        Task { hwnd, title: format!("w{hwnd}"), pid: hwnd as u32, exe: Some(PathBuf::from(exe)), app_id: id.map(String::from), minimized: false }
    }

    fn group(tasks: Vec<Task>) -> (String, Vec<Task>) {
        let k = tasks[0].group();
        (k, tasks)
    }

    fn pin(name: &str, exe: Option<&str>, id: Option<&str>) -> PinnedApp {
        PinnedApp {
            name: name.into(),
            aumid: id.map(|s| s.to_lowercase()),
            exe: exe.map(|s| s.to_lowercase()),
            launch: Launch::Link(PathBuf::from(format!(r"C:\pins\{name}.lnk"))),
            icon: IconSource::File(PathBuf::from(exe.unwrap_or("x.exe"))),
        }
    }

    fn app_of(key: &str, launch: Option<Launch>, icon: IconSource) -> DockApp {
        DockApp { key: key.into(), name: "x".into(), pinned: false, launch, icon, windows: vec![1], active: false }
    }

    #[test]
    fn a_jump_list_is_looked_up_by_the_packaged_id_and_the_program_whichever_the_app_has() {
        let exe = PathBuf::from(r"C:\a\Code.exe");
        // a classic program started from its file
        let (aumid, found) = jump_identity(&app_of(r"c:\a\code.exe", Some(Launch::Exe { path: exe.clone(), args: String::new() }), IconSource::Window(1)));
        assert_eq!((aumid, found), (None, Some(exe.clone())));
        // a packaged app: its id, and no program
        let (aumid, found) = jump_identity(&app_of("microsoft.paint_8wekyb3d8bbwe!app", Some(Launch::Packaged("Microsoft.Paint_8wekyb3d8bbwe!App".into())), IconSource::Window(1)));
        assert_eq!((aumid.as_deref(), found), (Some("Microsoft.Paint_8wekyb3d8bbwe!App"), None));
        // a running app whose launch is not known: the id from its key, the program from an icon that is an .exe (not from any other file)
        let (aumid, found) = jump_identity(&app_of("vendor.app_abc!main", None, IconSource::File(exe.clone())));
        assert_eq!((aumid.as_deref(), found), (Some("vendor.app_abc!main"), Some(exe)));
        let (aumid, found) = jump_identity(&app_of("pid:42", None, IconSource::File(PathBuf::from(r"C:\a\doc.txt"))));
        assert_eq!((aumid, found), (None, None));
    }

    fn apps(items: &[DockItem]) -> Vec<&DockApp> {
        items.iter().filter_map(|i| if let DockItem::App(a) = i { Some(a) } else { None }).collect()
    }

    #[test]
    fn pinned_first_in_their_order_then_the_other_running_apps_then_the_places() {
        let pins = vec![pin("Chrome", Some(r"C:\a\chrome.exe"), None), pin("Code", Some(r"C:\b\code.exe"), Some("Microsoft.VisualStudioCode")), pin("Paint", Some(r"C:\c\mspaint.exe"), None)];
        let groups = vec![
            group(vec![task(10, r"C:\z\notepad.exe", None)]),
            group(vec![task(11, r"C:\b\code.exe", Some("Microsoft.VisualStudioCode")), task(12, r"C:\b\code.exe", Some("Microsoft.VisualStudioCode"))]),
            group(vec![task(13, r"C:\a\Chrome.exe", None)]),
            group(vec![task(14, r"C:\y\other.exe", None)]),
        ];
        let items = merge(&pins, groups, 12);
        let a = apps(&items);
        assert_eq!(a.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Chrome", "Code", "Paint", "Notepad", "Other"]);
        assert_eq!(a.iter().map(|a| a.pinned).collect::<Vec<_>>(), vec![true, true, true, false, false]);
        assert_eq!(a[0].windows, vec![13]);
        assert_eq!(a[1].windows, vec![11, 12]);
        assert!(a[2].windows.is_empty(), "Paint is pinned and not running");
        assert!(a[1].active && !a[0].active && !a[3].active);
        assert_eq!(items[5], DockItem::Separator);
        assert_eq!(items[6], DockItem::Place(Place::RecycleBin));
        assert_eq!(items[7], DockItem::Place(Place::Downloads));
        assert_eq!(items.len(), 8);
    }

    #[test]
    fn a_running_group_is_taken_by_one_pin_only() {
        // two pins of the same program (two profiles): the first takes the windows, the second is just a launcher
        let pins = vec![pin("Chrome", Some(r"C:\a\chrome.exe"), None), pin("Chrome (work)", Some(r"C:\a\chrome.exe"), None)];
        let items = merge(&pins, vec![group(vec![task(1, r"C:\a\chrome.exe", None)])], 0);
        let a = apps(&items);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].windows, vec![1]);
        assert!(a[1].windows.is_empty());
    }

    #[test]
    fn a_pin_matches_by_app_id_then_by_program() {
        let t = |exe: &str, id: Option<&str>| vec![task(1, exe, id)];
        let p = pin("X", Some(r"C:\x\x.exe"), Some("Vendor.X"));
        assert!(pin_matches(&p, "vendor.x", &t(r"C:\other.exe", Some("Vendor.X"))), "same app id");
        assert!(pin_matches(&p, "something.else", &t(r"C:\X\x.exe", Some("Something.Else"))), "the window names itself differently: same program");
        assert!(!pin_matches(&p, "vendor.y", &t(r"C:\y\y.exe", Some("Vendor.Y"))));
        let no_exe = pin("Store app", None, Some("Pkg_abc!App"));
        assert!(pin_matches(&no_exe, "pkg_abc!app", &[]));
        assert!(!pin_matches(&no_exe, r"c:\x\x.exe", &t(r"C:\x\x.exe", None)));
        // a pin with a program path equal to the group key (a window without an app id)
        let q = pin("Q", Some(r"C:\q\q.exe"), None);
        assert!(pin_matches(&q, r"c:\q\q.exe", &t(r"C:\q\q.exe", None)));
    }

    #[test]
    fn packaged_apps_start_by_their_id_and_others_by_their_program() {
        let items = merge(
            &[],
            vec![
                group(vec![task(1, r"C:\Windows\System32\ApplicationFrameHost.exe", Some("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"))]),
                group(vec![task(2, r"C:\p\chrome.exe", Some("Chrome"))]),
            ],
            0,
        );
        let a = apps(&items);
        assert_eq!(a[0].launch, Some(Launch::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into())));
        assert_eq!(a[0].icon, IconSource::Packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App".into()));
        assert_eq!(a[1].launch, Some(Launch::Exe { path: PathBuf::from(r"C:\p\chrome.exe"), args: String::new() }));
        assert_eq!(a[1].icon, IconSource::File(PathBuf::from(r"C:\p\chrome.exe")));
        assert_eq!(a[1].name, "Chrome");
    }

    #[test]
    fn a_program_nobody_can_name_still_gets_a_button() {
        let t = Task { hwnd: 5, title: "Secret".into(), pid: 9, exe: None, app_id: None, minimized: false };
        let items = merge(&[], vec![group(vec![t])], 0);
        let a = apps(&items);
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].launch, None);
        assert_eq!(a[0].icon, IconSource::Window(5));
        assert_eq!(a[0].name, "Secret");
        assert_eq!(a[0].key, "pid:9");
    }

    #[test]
    fn no_pins_no_windows_leaves_the_separator_and_the_places() {
        let items = merge(&[], vec![], 0);
        assert_eq!(items, vec![DockItem::Separator, DockItem::Place(Place::RecycleBin), DockItem::Place(Place::Downloads)]);
    }

    #[test]
    fn click_decision() {
        assert_eq!(click_action(&[], 7), ClickAction::Start, "not running: start it");
        assert_eq!(click_action(&[3, 4], 9), ClickAction::Raise(3), "running, not in front: its last used window");
        assert_eq!(click_action(&[3, 4], 4), ClickAction::Minimize(4), "in front: minimise the one in front");
        assert_eq!(click_action(&[3, 4], 3), ClickAction::Minimize(3));
        assert_eq!(click_action(&[3], 0), ClickAction::Raise(3), "nothing in front at all");
        assert_eq!(middle_click_action(), ClickAction::Start, "middle click: another instance");
    }

    // The shapes below are what this machine has: the taskbar pins Telegram and Spotify by package id; their windows set no id of their
    // own, their processes' packages carry it (`identity::process_aumid`); WhatsApp is running and not pinned.
    const TELEGRAM: &str = "TelegramMessengerLLP.TelegramDesktop_t4vj0pshhgkwm!Telegram.TelegramDesktop.Store";
    const SPOTIFY: &str = "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify";
    const WHATSAPP: &str = "5319275A.WhatsAppDesktop_cv1g1gvanyjgm!App";

    fn packaged_pin(name: &str, id: &str) -> PinnedApp {
        PinnedApp { name: name.into(), aumid: Some(id.to_lowercase()), exe: None, launch: Launch::Packaged(id.into()), icon: IconSource::Packaged(id.into()) }
    }

    #[test]
    fn a_packaged_app_is_one_item_whether_pinned_or_not() {
        let pins = vec![packaged_pin("Telegram Desktop", TELEGRAM), packaged_pin("Spotify", SPOTIFY)];
        let tasks = vec![
            task(10, r"C:\Program Files\WindowsApps\telegrammessengerllp.telegramdesktop_7.3.3.0_x64__t4vj0pshhgkwm\telegram.exe", None),
            task(11, r"C:\Program Files\WindowsApps\spotifyab.spotifymusic_1.308.620.0_x64__zpdnekdrzrea0\spotify.exe", None),
            task(12, r"C:\Program Files\WindowsApps\5319275a.whatsappdesktop_2.2642.101.0_x64__cv1g1gvanyjgm\whatsapp.root.exe", None),
        ];
        let ids = |pid: u32| match pid {
            10 => Some(TELEGRAM.to_string()),
            11 => Some(SPOTIFY.to_string()),
            12 => Some(WHATSAPP.to_string()),
            _ => None,
        };
        let items = merge(&pins, crate::tasklist::grouped(super::super::identity::with_process_ids(tasks, ids)), 0);
        let a = apps(&items);
        assert_eq!(a.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), vec!["Telegram Desktop", "Spotify", "Whatsapp.root"], "no second Telegram or Spotify");
        assert_eq!(a[0].windows, vec![10]);
        assert_eq!(a[1].windows, vec![11]);
        assert!(a[0].pinned && a[1].pinned && !a[2].pinned);
        // WhatsApp's icon is its package's logo, not the picture of an exe the shell cannot open
        assert_eq!(a[2].icon, IconSource::Packaged(WHATSAPP.into()));
        assert_eq!(a[2].launch, Some(Launch::Packaged(WHATSAPP.into())));
        // the same windows WITHOUT the process ids are the bug that was seen: two items for each app
        let tasks = vec![task(10, r"C:\Program Files\WindowsApps\telegrammessengerllp.telegramdesktop_7.3.3.0_x64__t4vj0pshhgkwm\telegram.exe", None)];
        let items = merge(&pins[..1], crate::tasklist::grouped(tasks), 0);
        assert_eq!(apps(&items).len(), 2, "the id is what lets the pin and the window meet");
    }

    #[test]
    fn a_pin_and_a_window_that_name_the_same_id_in_other_case_still_meet() {
        let p = packaged_pin("Spotify", SPOTIFY);
        assert!(pin_matches(&p, &SPOTIFY.to_lowercase(), &[]));
        assert!(!pin_matches(&p, &TELEGRAM.to_lowercase(), &[]), "another package");
        assert!(!pin_matches(&p, "spotifyab.spotifymusic_zpdnekdrzrea0!other", &[]), "another app of the same package is another app");
    }

    #[test]
    fn the_icon_chain_tries_the_app_then_its_windows_then_its_package() {
        let t = |h: isize| task(h, r"C:\x\a.exe", None);
        let app = |a: Vec<DockItem>| match a.into_iter().next() {
            Some(DockItem::App(a)) => a,
            _ => panic!("an app"),
        };
        // a plain program: its icon, then up to three of its windows
        let plain = app(merge(&[], vec![group(vec![t(1), t(2), t(3), t(4)])], 0));
        assert_eq!(plain.icon_chain(), vec![IconSource::File(PathBuf::from(r"C:\x\a.exe")), IconSource::Window(1), IconSource::Window(2), IconSource::Window(3)]);
        // a packaged app: its logo is the app's own icon, so it is not asked for twice
        let pk = app(merge(&[], vec![group(vec![task(9, r"C:\p\telegram.exe", Some(TELEGRAM))])], 0));
        assert_eq!(pk.icon_chain(), vec![IconSource::Packaged(TELEGRAM.into()), IconSource::Window(9)]);
        // a pinned program that is not running: only its own icon
        let idle = app(merge(&[pin("Idle", Some(r"C:\i\i.exe"), None)], vec![], 0));
        assert_eq!(idle.icon_chain().len(), 1);
        // a pin with a program path whose windows can supply what the shell cannot: its package logo comes last
        let mut packaged_pin_running = app(merge(&[pin("P", Some(r"C:\i\i.exe"), Some("Pkg.Name_abcdefghijklm!App"))], vec![group(vec![t(5)])], 0));
        packaged_pin_running.windows = vec![5];
        assert_eq!(packaged_pin_running.icon_chain().last(), Some(&IconSource::Packaged("pkg.name_abcdefghijklm!app".into())));
    }

    #[test]
    fn packaged_ids_are_recognised() {
        assert!(looks_packaged("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"));
        assert!(looks_packaged("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"));
        assert!(!looks_packaged("Chrome"));
        assert!(!looks_packaged("Microsoft.Windows.Explorer"));
        assert!(!looks_packaged(r"C:\Program Files\x_y!z.exe"));
        assert!(!looks_packaged("Pkg_abc!"));
    }

    #[test]
    fn slots_follow_the_items() {
        let items = merge(&[pin("A", Some("a.exe"), None)], vec![], 0);
        use super::super::hover::Slot::*;
        assert_eq!(slots(&items), vec![Icon, Separator, Icon, Icon]);
    }

    #[test]
    fn places_know_their_icon_and_open_argument() {
        assert_eq!(Place::RecycleBin.open_argument(), "shell:RecycleBinFolder");
        assert!(!Place::Downloads.open_argument().is_empty());
        assert!(matches!(Place::RecycleBin.icon_source(), IconSource::Shell(_)));
    }
}
