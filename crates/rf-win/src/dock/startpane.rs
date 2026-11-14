//! The Start panel's detail pane: what stands beside the result list for the result that is selected. Pure rules: which actions a
//! result has and in which order, what a key combination means, how many lines of an app's jump list are shown. The drawing and the
//! doing are the app's (`rf-app/src/dock/start_pane.rs`).
//!
//! WHY (the user, 2026-11-14: "başlatta bir uygulamaya tıklayınca mı ne sağ panel açılıyordu. dosya konumu yönetici olarak başlat
//! kaldır filan. orayı biz de yapalım hatta zenginleştirelim. iyi bir ui ux ile"). Windows' own search shows such a pane for the best
//! match. Ours shows it for whichever result is selected, with the same actions and with more: the app's own jump list (its tasks and
//! its recent documents), "open with" and "copy the path" for a file, where a file is and when it was changed.
//!
//! THE ORDER OF THE ACTIONS is by how often they are wanted and by how much they change: opening first, the other ways of opening
//! after it, then what changes the panel or the dock (pinning), and removal last, apart from the rest.

/// The result list's width while the pane stands beside it, logical px (the panel is 642 wide: the pane has the rest).
pub const LIST_W: f32 = 296.0;
/// The picture at the head of the pane.
pub const ICON: f32 = 56.0;
/// A row of the pane: an action, or a line of the app's jump list.
pub const ROW_H: f32 = 34.0;
/// The most lines of an app's jump list the pane shows (all its sections together): the pane is a glance, the whole list is the
/// dock icon's menu.
pub const JUMP_MAX: usize = 7;

/// What kind of thing the selected result is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    /// A program file found by its name (`chrome.exe`, `cmd`): it is started, not opened with something.
    Program,
    File,
    Folder,
    /// A bookmark of a browser.
    Link,
    /// "Search the web for ...".
    Web,
    /// A page of Windows' Settings or a Control Panel tool.
    Setting,
    /// The row that starts the main app in the background (the file index lives there).
    StartMain,
}

/// One thing the pane offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Act {
    /// The result's own "open": start the app, open the file, search the web.
    Open,
    /// Start the app with administrator rights (Windows asks, because the user chose this).
    Admin,
    /// Show the program or the file in its folder.
    Location,
    PinStart,
    UnpinStart,
    /// Add the app to the dock's own pins.
    PinDock,
    /// Windows' page of installed apps, where it is removed.
    Uninstall,
    /// Windows' "Open with" dialog.
    OpenWith,
    CopyPath,
    CopyLink,
}

/// What is known about the selected result that decides its actions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Facts {
    /// An app: the shell names a program for it (a classic app; a packaged app has none to show).
    pub has_program: bool,
    /// An app: it is among the panel's pinned apps.
    pub pinned_start: bool,
}

/// The actions of a result, top to bottom.
pub fn actions(kind: Kind, f: Facts) -> Vec<Act> {
    match kind {
        Kind::App => {
            let mut v = vec![Act::Open, Act::Admin];
            if f.has_program {
                v.push(Act::Location);
            }
            v.push(if f.pinned_start { Act::UnpinStart } else { Act::PinStart });
            v.push(Act::PinDock);
            v.push(Act::Uninstall);
            v
        }
        Kind::Program => vec![Act::Open, Act::Admin, Act::Location, Act::CopyPath],
        Kind::File => vec![Act::Open, Act::OpenWith, Act::Location, Act::CopyPath],
        Kind::Folder => vec![Act::Open, Act::Location, Act::CopyPath],
        Kind::Link => vec![Act::Open, Act::CopyLink],
        Kind::Web | Kind::Setting | Kind::StartMain => vec![Act::Open],
    }
}

/// The keys that do an action straight from the list, as the pane writes them beside it.
pub fn shortcut(act: Act) -> Option<&'static str> {
    match act {
        Act::Open => Some("Enter"),
        Act::Admin => Some("Ctrl+Shift+Enter"),
        Act::Location => Some("Ctrl+Enter"),
        _ => None,
    }
}

/// What Enter does with these modifier keys held: the three ways of opening Windows' own Start has.
pub fn enter_act(ctrl: bool, shift: bool) -> Act {
    match (ctrl, shift) {
        (true, true) => Act::Admin,
        (true, false) => Act::Location,
        _ => Act::Open,
    }
}

/// Whether an action is drawn apart from the ones before it (a thin line over it): the first that changes something lasting, and
/// removal.
pub fn starts_group(actions: &[Act], ix: usize) -> bool {
    let lasting = |a: Act| matches!(a, Act::PinStart | Act::UnpinStart | Act::PinDock);
    match actions.get(ix) {
        Some(Act::Uninstall) => true,
        Some(a) if lasting(*a) => ix > 0 && !lasting(actions[ix - 1]),
        _ => false,
    }
}

/// Whether a file with this extension (lower case, no dot) is a program: something that is started.
pub fn is_program_ext(ext: &str) -> bool {
    matches!(ext, "exe" | "com" | "bat" | "cmd")
}

/// The order the pane shows a jump list's sections in, as indexes into them: the TASKS first ("New window", "New incognito window":
/// what the user most often wants of an app that is not running in front), then the rest in the taskbar's order. `tasks_from` is the
/// index of the first section of the tasks (they are the list's last sections; a line between two groups of tasks makes a second
/// section without a heading), `None` when the list has none.
pub fn jump_order(sections: usize, tasks_from: Option<usize>) -> Vec<usize> {
    match tasks_from.filter(|&t| t < sections) {
        Some(t) => (t..sections).chain(0..t).collect(),
        None => (0..sections).collect(),
    }
}

/// How many lines of each section of a jump list are shown, when the sections have `lens` lines: in their order, [`JUMP_MAX`] lines
/// in all, and never a section of which nothing is left (its heading alone would be a title over nothing).
pub fn jump_shown(lens: &[usize], max: usize) -> Vec<usize> {
    let mut left = max;
    lens.iter()
        .map(|&n| {
            let take = n.min(left);
            left -= take;
            take
        })
        .collect()
}

/// The pane's keyboard cursor after Up or Down: over its `rows` (the actions, then the jump list's lines), round and round.
pub fn step(cursor: usize, up: bool, rows: usize) -> usize {
    super::startlist::step_results(cursor.min(rows.saturating_sub(1)), up, rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_is_always_first_and_every_kind_has_it() {
        for kind in [Kind::App, Kind::Program, Kind::File, Kind::Folder, Kind::Link, Kind::Web, Kind::Setting, Kind::StartMain] {
            for f in [Facts::default(), Facts { has_program: true, pinned_start: true }] {
                let a = actions(kind, f);
                assert_eq!(a[0], Act::Open, "{kind:?}");
                let unique: std::collections::HashSet<Act> = a.iter().copied().collect();
                assert_eq!(unique.len(), a.len(), "{kind:?}: no action twice");
            }
        }
    }

    #[test]
    fn an_app_offers_its_folder_only_when_it_has_a_program_and_pins_by_what_it_is_now() {
        let classic = actions(Kind::App, Facts { has_program: true, pinned_start: false });
        assert_eq!(classic, [Act::Open, Act::Admin, Act::Location, Act::PinStart, Act::PinDock, Act::Uninstall]);
        let packaged = actions(Kind::App, Facts { has_program: false, pinned_start: true });
        assert_eq!(packaged, [Act::Open, Act::Admin, Act::UnpinStart, Act::PinDock, Act::Uninstall]);
        // removal is last, and apart
        assert_eq!(*classic.last().unwrap(), Act::Uninstall);
        assert!(starts_group(&classic, classic.len() - 1));
        // the pins begin a group; the second pin row does not begin another
        assert!(starts_group(&classic, 3) && !starts_group(&classic, 4));
        assert!(!starts_group(&classic, 0) && !starts_group(&classic, 1) && !starts_group(&classic, 2));
    }

    #[test]
    fn files_folders_and_links() {
        assert_eq!(actions(Kind::File, Facts::default()), [Act::Open, Act::OpenWith, Act::Location, Act::CopyPath]);
        // a program file is started (also as administrator), never "opened with"
        assert_eq!(actions(Kind::Program, Facts::default()), [Act::Open, Act::Admin, Act::Location, Act::CopyPath]);
        assert!(is_program_ext("exe") && is_program_ext("cmd") && is_program_ext("bat") && is_program_ext("com"));
        assert!(!is_program_ext("txt") && !is_program_ext("") && !is_program_ext("lnk") && !is_program_ext("msi"));
        assert_eq!(actions(Kind::Folder, Facts::default()), [Act::Open, Act::Location, Act::CopyPath]);
        assert_eq!(actions(Kind::Link, Facts::default()), [Act::Open, Act::CopyLink]);
        for kind in [Kind::Web, Kind::Setting, Kind::StartMain] {
            assert_eq!(actions(kind, Facts::default()), [Act::Open]);
        }
        let file = actions(Kind::File, Facts::default());
        assert!((0..file.len()).all(|i| !starts_group(&file, i)), "nothing lasting among a file's actions");
    }

    #[test]
    fn enter_with_the_modifier_keys_is_the_three_ways_of_opening() {
        assert_eq!(enter_act(false, false), Act::Open);
        assert_eq!(enter_act(false, true), Act::Open, "Shift alone is nothing special");
        assert_eq!(enter_act(true, false), Act::Location);
        assert_eq!(enter_act(true, true), Act::Admin);
        // and the pane writes exactly those keys beside those actions
        assert_eq!(shortcut(Act::Open), Some("Enter"));
        assert_eq!(shortcut(Act::Location), Some("Ctrl+Enter"));
        assert_eq!(shortcut(Act::Admin), Some("Ctrl+Shift+Enter"));
        assert_eq!(shortcut(Act::CopyPath), None);
    }

    #[test]
    fn the_jump_list_is_cut_to_a_glance_and_no_heading_stands_over_nothing() {
        assert_eq!(jump_shown(&[2, 3], 7), [2, 3]);
        assert_eq!(jump_shown(&[4, 10, 5], 7), [4, 3, 0], "the third section has no line left");
        assert_eq!(jump_shown(&[10], 7), [7]);
        assert_eq!(jump_shown(&[], 7), Vec::<usize>::new());
        assert_eq!(jump_shown(&[0, 2], 7), [0, 2]);
        assert_eq!(jump_shown(&[3, 3], 0), [0, 0]);
    }

    #[test]
    fn the_tasks_of_a_jump_list_come_first_in_the_pane() {
        // pinned, most visited, recent, tasks, more tasks (after a line)
        assert_eq!(jump_order(5, Some(3)), [3, 4, 0, 1, 2]);
        assert_eq!(jump_order(3, None), [0, 1, 2]);
        assert_eq!(jump_order(2, Some(0)), [0, 1], "only tasks");
        assert_eq!(jump_order(2, Some(9)), [0, 1], "an index past the end is no tasks");
        assert_eq!(jump_order(0, None), Vec::<usize>::new());
    }

    #[test]
    fn the_cursor_goes_round_the_rows() {
        assert_eq!(step(0, false, 4), 1);
        assert_eq!(step(3, false, 4), 0);
        assert_eq!(step(0, true, 4), 3);
        assert_eq!(step(9, true, 4), 2, "a cursor past the end (the rows became fewer) counts from the last row");
        assert_eq!(step(0, false, 0), 0);
    }

    #[test]
    fn the_list_and_the_pane_share_the_panels_width() {
        let pane = super::super::startpage::PANEL_W - LIST_W;
        assert!(pane >= 320.0, "room for an action with its keys beside it: {pane}");
        assert!(LIST_W >= 260.0, "room for a result's name");
    }
}
