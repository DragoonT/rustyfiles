//! Which windows the switcher lists, as pure rules over made-up facts. Nothing here touches a window: `source.rs` reads the facts
//! from Windows, these functions decide, and the tests below build the facts by hand for every rule.
//!
//! THE RULES (shell plan 5 G1; research note 4-desktop.md section 6.2):
//!
//! 1. The classic Alt+Tab walk (Raymond Chen, "Which windows appear in the Alt+Tab list?"): take the window's root owner, follow
//!    the chain of last active popups down while the popup found is not visible, and list the window only if the walk ends on it.
//!    A tool window counts as not visible, and a window with `WS_EX_APPWINDOW` counts as having no owner. So an app with a dialog
//!    open is one entry, and the entry is the dialog (the window the user was last in), not two.
//! 2. Windows that ask never to be activated (`WS_EX_NOACTIVATE`) without `WS_EX_APPWINDOW`, windows of zero size (an invisible
//!    helper), the desktop's own windows (`Progman`, `WorkerW`, the taskbar classes) and the hosted `CoreWindow` of a Store app
//!    (its `ApplicationFrameWindow` is the window to list) are never listed.
//! 3. Cloaked windows (`DWMWA_CLOAKED`; `IsWindowVisible` alone is wrong, a cloaked window reports visible):
//!    * cloaked by the app itself (bit 1: hidden helper windows, `ThumbnailDeviceHelperWnd`...) is not a window to switch to;
//!    * cloaked by the shell (bit 2) or inherited from an owner (bit 4) is either a window of ANOTHER virtual desktop or a SUSPENDED
//!      Store app on this one. `IVirtualDesktopManager::IsWindowOnCurrentVirtualDesktop` tells them apart ([`Desktop`]): another
//!      desktop is left out (counted; the options can put it back), a suspended app is kept and shown as a minimized window is.
//!      A suspended app is a MINIMIZED one: the shell cloaks the frame of a Store app that was minimized and suspended. A frame that
//!      is cloaked on this desktop and NOT minimized (read live on this machine: a titled `ApplicationFrameWindow` with no hosted
//!      `CoreWindow`, left behind by an app that was closed) is not a window anyone can switch to, so it is left out and counted.
//!      When the manager cannot answer the window is left out: showing another desktop's windows would be the worse mistake.
//!    * a cloaked window needs a title to be listed (the shell cloaks empty frames of apps that are not open at all).
//! 4. Our own shell windows (dock islands, panels, desktop windows, the switcher) are never listed: they are marked
//!    ([`super::mark_shell_window`]) and also fall to the tool-window rule.
//!
//! ORDER is the z-order of the facts given (`EnumWindows` order: top first), which follows the order the user last used the windows
//! in only for the windows that are neither minimized nor always-on-top; it is NOT a true most-recently-used list:
//! * always-on-top windows are above the others in the z-order whether or not they were used last, so [`select`] lists them after
//!   the ordinary windows (keeping their order among themselves), and [`promote_foreground`] puts the window in front first, which is
//!   what makes a flip between two windows work (Alt+Tab goes to the window used before, never to a pinned note);
//! * a minimized window sits at the bottom of the z-order, where Windows put it when it was minimized, not where the user last used
//!   it. Only a record of activations can fix that (a foreground event hook in the shell host: the plan's "ordering by last
//!   activation"); nothing here keeps state, so nothing here can.

use std::collections::HashMap;

pub const WS_EX_TOPMOST: u32 = 0x0000_0008;
pub const WS_EX_TOOLWINDOW: u32 = 0x0000_0080;
pub const WS_EX_APPWINDOW: u32 = 0x0004_0000;
pub const WS_EX_NOACTIVATE: u32 = 0x0800_0000;

/// `DWM_CLOAKED_APP`: the app cloaked the window itself.
pub const CLOAKED_APP: u32 = 1;
/// `DWM_CLOAKED_SHELL`: the shell cloaked it (another virtual desktop, a suspended Store app).
pub const CLOAKED_SHELL: u32 = 2;
/// `DWM_CLOAKED_INHERITED`: its owner is cloaked.
pub const CLOAKED_INHERITED: u32 = 4;

/// Window classes that are never switch targets.
const NEVER_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    // the hosted window of a Store app: the ApplicationFrameWindow around it is the one to list
    "Windows.UI.Core.CoreWindow",
];

/// What Windows says about one top-level window. Made by `source.rs`; tests make them by hand.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowFacts {
    pub hwnd: isize,
    pub visible: bool,
    pub ex_style: u32,
    /// `GetAncestor(GA_ROOTOWNER)`; the window itself (or 0) when it has no owner.
    pub root_owner: isize,
    /// `GetLastActivePopup`: the window itself (or 0) when it has no active popup.
    pub last_popup: isize,
    /// `DWMWA_CLOAKED` bits, 0 when not cloaked.
    pub cloak: u32,
    pub minimized: bool,
    pub title: String,
    pub class: String,
    pub width: i32,
    pub height: i32,
    /// The process the window should be attributed to (for a Store app's frame, the app's own process, not the frame host).
    pub pid: u32,
    /// One of our own shell windows.
    pub shell_window: bool,
}

/// Where a cloaked window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Current,
    Other,
    /// The virtual desktop manager could not answer.
    Unknown,
}

/// Why a window is not listed (for the counts and the tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    NotVisible,
    OurShellWindow,
    NeverListed,
    NoActivate,
    ZeroSize,
    /// The Alt+Tab walk ends on another window.
    NotTheLastPopup,
    CloakedByApp,
    /// Cloaked and without a title.
    Untitled,
    /// Cloaked by the shell on this desktop, but not minimized: not a suspended app, a frame with nothing behind it.
    CloakedNotMinimized,
    /// Cloaked, and the virtual desktop manager could not say where it is.
    DesktopUnknown,
}

/// What phase one decides, before any virtual desktop is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pre {
    Listed,
    /// Cloaked by the shell: the virtual desktop manager must say whether it is here or elsewhere.
    NeedsDesktop,
    Skip(Why),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Listed,
    /// A suspended Store app on this desktop: listed, with no live picture.
    Suspended,
    /// On another virtual desktop: not listed unless the options say so.
    OtherDesktop,
    Skip(Why),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Options {
    /// List windows of other virtual desktops too (they are marked `other_desktop`).
    pub other_desktops: bool,
}

fn is_tool(f: &WindowFacts) -> bool {
    f.ex_style & WS_EX_TOOLWINDOW != 0 && f.ex_style & WS_EX_APPWINDOW == 0
}

/// Whether the walk counts the window as visible: visible, not a tool window, not hidden by its own app. (A window the shell
/// cloaked counts as visible: its dialog is still the entry of its app.)
fn walk_visible(f: &WindowFacts) -> bool {
    f.visible && !is_tool(f) && f.cloak & CLOAKED_APP == 0
}

/// Raymond Chen's walk. `index` holds every top-level window (invisible ones too: an invisible owner is a step of the walk).
///
/// One change to his version: when the walk ends on a window that can not be an entry (a tool palette, a hidden popup that still is
/// the owner's "last active" one) the root the walk started from is the entry, so an app is not lost from the list because of a
/// palette it used last. His version would list neither.
fn ends_on_itself(f: &WindowFacts, index: &HashMap<isize, &WindowFacts>) -> bool {
    let app = f.ex_style & WS_EX_APPWINDOW != 0;
    let start = if app || f.root_owner == 0 { f.hwnd } else { f.root_owner };
    let mut walk = start;
    let mut found = walk;
    // (the chain of popups is short; the bound keeps a corrupt answer from looping)
    for _ in 0..32 {
        let popup = match index.get(&walk) {
            Some(w) if w.last_popup != 0 => w.last_popup,
            _ => walk,
        };
        found = popup;
        if popup == walk {
            break;
        }
        if index.get(&popup).is_some_and(|p| walk_visible(p)) {
            break;
        }
        walk = popup;
    }
    let entry = if index.get(&found).is_some_and(|w| walk_visible(w)) { found } else { start };
    entry == f.hwnd
}

/// Phase one: everything that can be decided from the window alone.
pub fn pre(f: &WindowFacts, index: &HashMap<isize, &WindowFacts>) -> Pre {
    if f.shell_window {
        return Pre::Skip(Why::OurShellWindow);
    }
    if !f.visible {
        return Pre::Skip(Why::NotVisible);
    }
    if NEVER_CLASSES.iter().any(|c| f.class.eq_ignore_ascii_case(c)) {
        return Pre::Skip(Why::NeverListed);
    }
    if f.ex_style & WS_EX_NOACTIVATE != 0 && f.ex_style & WS_EX_APPWINDOW == 0 {
        return Pre::Skip(Why::NoActivate);
    }
    // (a minimized window's rectangle is parked far away but is never empty; some helpers are 0 x 0)
    if !f.minimized && (f.width <= 0 || f.height <= 0) {
        return Pre::Skip(Why::ZeroSize);
    }
    if f.cloak & CLOAKED_APP != 0 {
        return Pre::Skip(Why::CloakedByApp);
    }
    if is_tool(f) || !ends_on_itself(f, index) {
        return Pre::Skip(if is_tool(f) { Why::NeverListed } else { Why::NotTheLastPopup });
    }
    if f.cloak & (CLOAKED_SHELL | CLOAKED_INHERITED) != 0 {
        if f.title.trim().is_empty() {
            return Pre::Skip(Why::Untitled);
        }
        return Pre::NeedsDesktop;
    }
    Pre::Listed
}

/// One window chosen by [`select`]: where it is in the facts it came from, and how it is listed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub index: usize,
    pub suspended: bool,
    pub other_desktop: bool,
}

/// What was left out, by reason (counts only).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub windows_seen: usize,
    pub listed: usize,
    pub minimized: usize,
    pub suspended: usize,
    pub on_other_desktop: usize,
    pub cloaked_by_app: usize,
    pub desktop_unknown: usize,
    /// Cloaked by the shell on this desktop and not minimized (left out: nothing to switch to).
    pub cloaked_not_minimized: usize,
}

/// The windows that need a virtual desktop answer, so that the caller can ask for all of them in one go.
pub fn needing_desktop(facts: &[WindowFacts]) -> Vec<isize> {
    let index: HashMap<isize, &WindowFacts> = facts.iter().map(|f| (f.hwnd, f)).collect();
    facts.iter().filter(|f| pre(f, &index) == Pre::NeedsDesktop).map(|f| f.hwnd).collect()
}

/// The verdict for one window once its desktop is known (`Unknown` for a window that needed none is ignored). `minimized` is the
/// window's own state: on this desktop only a minimized one is a suspended app.
pub fn verdict(pre: Pre, desktop: Desktop, minimized: bool) -> Verdict {
    match pre {
        Pre::Listed => Verdict::Listed,
        Pre::Skip(w) => Verdict::Skip(w),
        Pre::NeedsDesktop => match desktop {
            Desktop::Current if minimized => Verdict::Suspended,
            Desktop::Current => Verdict::Skip(Why::CloakedNotMinimized),
            Desktop::Other => Verdict::OtherDesktop,
            Desktop::Unknown => Verdict::Skip(Why::DesktopUnknown),
        },
    }
}

/// The listing: the windows of `facts` (top of the z-order first) that the switcher shows, and the counts of what it left out.
/// `desktops` answers for the windows [`needing_desktop`] named; one it does not mention is `Unknown`.
pub fn select(facts: &[WindowFacts], desktops: &HashMap<isize, Desktop>, opts: Options) -> (Vec<Chosen>, Counts) {
    let index: HashMap<isize, &WindowFacts> = facts.iter().map(|f| (f.hwnd, f)).collect();
    let mut out = Vec::new();
    let mut counts = Counts { windows_seen: facts.len(), ..Counts::default() };
    for (i, f) in facts.iter().enumerate() {
        let p = pre(f, &index);
        let v = verdict(p, desktops.get(&f.hwnd).copied().unwrap_or(Desktop::Unknown), f.minimized);
        match v {
            Verdict::Listed => out.push(Chosen { index: i, suspended: false, other_desktop: false }),
            Verdict::Suspended => {
                counts.suspended += 1;
                out.push(Chosen { index: i, suspended: true, other_desktop: false });
            }
            Verdict::OtherDesktop => {
                counts.on_other_desktop += 1;
                if opts.other_desktops {
                    out.push(Chosen { index: i, suspended: true, other_desktop: true });
                }
            }
            Verdict::Skip(Why::CloakedByApp) => counts.cloaked_by_app += 1,
            Verdict::Skip(Why::DesktopUnknown) => counts.desktop_unknown += 1,
            Verdict::Skip(Why::CloakedNotMinimized) => counts.cloaked_not_minimized += 1,
            Verdict::Skip(_) => {}
        }
    }
    // (the z-order puts always-on-top windows first whether or not they were used last: they go after the ordinary ones; the sort is
    // stable, so each group keeps its z-order)
    out.sort_by_key(|c| facts[c.index].ex_style & WS_EX_TOPMOST != 0);
    counts.listed = out.len();
    counts.minimized = out.iter().filter(|c| facts[c.index].minimized).count();
    (out, counts)
}

/// Puts the window in front (or the entry of its app, when the foreground window is not itself listed: a window owned by the one
/// listed) first, keeping the order of the others. The window in front may be an always-on-top one that [`select`] put last, or
/// one the z-order does not put first for another reason; without this "one step forward" would pick the wrong window. Answers whether the foreground
/// is one of the listed windows: when it is not (the desktop, the dock, a panel) the first card is NOT the window in front and the
/// switcher must open on it, not skip it.
pub fn promote_foreground(chosen: &mut [Chosen], facts: &[WindowFacts], foreground_root: isize) -> bool {
    if foreground_root == 0 {
        return false;
    }
    let pos = chosen.iter().position(|c| {
        let f = &facts[c.index];
        f.hwnd == foreground_root || f.root_owner == foreground_root
    });
    match pos {
        Some(p) => {
            if p > 0 {
                chosen[..=p].rotate_right(1);
            }
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A visible, plain, 800 x 600 top-level window.
    fn win(hwnd: isize, title: &str) -> WindowFacts {
        WindowFacts { hwnd, visible: true, root_owner: hwnd, last_popup: hwnd, title: title.into(), class: "App".into(), width: 800, height: 600, pid: hwnd as u32, ..Default::default() }
    }

    fn list(facts: &[WindowFacts]) -> Vec<isize> {
        list_with(facts, &HashMap::new(), Options::default())
    }

    fn list_with(facts: &[WindowFacts], desktops: &HashMap<isize, Desktop>, opts: Options) -> Vec<isize> {
        select(facts, desktops, opts).0.iter().map(|c| facts[c.index].hwnd).collect()
    }

    #[test]
    fn plain_windows_are_listed_in_the_order_given() {
        let f = [win(1, "a"), win(2, "b"), win(3, "c")];
        assert_eq!(list(&f), vec![1, 2, 3]);
    }

    #[test]
    fn invisible_windows_are_not_listed() {
        let mut hidden = win(2, "hidden");
        hidden.visible = false;
        assert_eq!(list(&[win(1, "a"), hidden]), vec![1]);
    }

    #[test]
    fn a_tool_window_is_not_listed_but_appwindow_wins_over_toolwindow() {
        let mut tool = win(1, "palette");
        tool.ex_style = WS_EX_TOOLWINDOW;
        let mut both = win(2, "forced");
        both.ex_style = WS_EX_TOOLWINDOW | WS_EX_APPWINDOW;
        assert_eq!(list(&[tool, both]), vec![2]);
    }

    #[test]
    fn a_window_that_is_never_activated_is_not_listed() {
        let mut w = win(1, "toast");
        w.ex_style = WS_EX_NOACTIVATE;
        let mut forced = win(2, "forced");
        forced.ex_style = WS_EX_NOACTIVATE | WS_EX_APPWINDOW;
        assert_eq!(list(&[w, forced]), vec![2]);
    }

    #[test]
    fn zero_size_helpers_are_not_listed_but_a_minimized_window_is() {
        let mut helper = win(1, "helper");
        helper.width = 0;
        helper.height = 0;
        let mut min = win(2, "minimized");
        min.minimized = true;
        min.width = 0; // (a parked rectangle; the size is not asked of a minimized window)
        assert_eq!(list(&[helper, min]), vec![2]);
    }

    #[test]
    fn the_desktop_and_the_taskbar_are_never_listed() {
        let mk = |h, class: &str| WindowFacts { class: class.into(), ..win(h, "x") };
        let f = [mk(1, "Progman"), mk(2, "WorkerW"), mk(3, "Shell_TrayWnd"), mk(4, "Shell_SecondaryTrayWnd"), mk(5, "Notepad")];
        assert_eq!(list(&f), vec![5]);
    }

    #[test]
    fn our_own_shell_windows_are_never_listed() {
        let mut island = win(1, "island");
        island.shell_window = true;
        assert_eq!(list(&[island, win(2, "files")]), vec![2]);
    }

    #[test]
    fn a_dialog_replaces_its_owner_as_the_entry_of_the_app() {
        // owner 10 has an open dialog 11 (visible, owned by 10): Alt+Tab shows one entry, the dialog
        let mut owner = win(10, "Editor");
        owner.last_popup = 11;
        let mut dialog = win(11, "Save as");
        dialog.root_owner = 10;
        dialog.last_popup = 11;
        let f = [dialog, owner, win(20, "Browser")];
        assert_eq!(list(&f), vec![11, 20]);
    }

    #[test]
    fn with_the_dialog_closed_the_owner_is_the_entry_again() {
        let mut owner = win(10, "Editor");
        owner.last_popup = 10;
        let mut dialog = win(11, "Save as");
        dialog.visible = false; // closed (hidden), still owned
        dialog.root_owner = 10;
        let f = [owner, dialog];
        assert_eq!(list(&f), vec![10]);
    }

    #[test]
    fn an_invisible_last_popup_is_walked_past() {
        // owner 10 -> last popup 11 (hidden) -> last popup 12 (visible): the walk ends on 12
        let mut owner = win(10, "main");
        owner.last_popup = 11;
        let mut hidden = win(11, "hidden dialog");
        hidden.visible = false;
        hidden.root_owner = 10;
        hidden.last_popup = 12;
        let mut shown = win(12, "shown dialog");
        shown.root_owner = 10;
        shown.last_popup = 12;
        let f = [shown, hidden, owner];
        assert_eq!(list(&f), vec![12]);
    }

    #[test]
    fn a_popup_that_is_a_tool_window_does_not_take_the_entry_from_its_owner() {
        // the owner's last active popup is a tool palette: the app stays in the list, as its main window
        let mut owner = win(10, "main");
        owner.last_popup = 11;
        let mut palette = win(11, "palette");
        palette.ex_style = WS_EX_TOOLWINDOW;
        palette.root_owner = 10;
        palette.last_popup = 11;
        let f = [palette, owner];
        assert_eq!(list(&f), vec![10]);
    }

    #[test]
    fn appwindow_makes_a_window_ownerless_for_the_walk() {
        // a window owned by 10 that asks for its own button is its own entry, and the owner stays one too
        let mut owner = win(10, "main");
        owner.last_popup = 10;
        let mut child = win(11, "own button");
        child.ex_style = WS_EX_APPWINDOW;
        child.root_owner = 10;
        child.last_popup = 11;
        assert_eq!(list(&[child, owner]), vec![11, 10]);
    }

    #[test]
    fn a_window_cloaked_by_its_own_app_is_not_listed_and_counted() {
        let mut w = win(1, "helper");
        w.cloak = CLOAKED_APP;
        let (chosen, counts) = select(&[w], &HashMap::new(), Options::default());
        assert!(chosen.is_empty());
        assert_eq!(counts.cloaked_by_app, 1);
    }

    #[test]
    fn a_shell_cloaked_minimized_window_on_this_desktop_is_a_suspended_app_and_is_kept() {
        let mut w = win(1, "Calculator");
        w.cloak = CLOAKED_SHELL;
        w.minimized = true;
        w.class = "ApplicationFrameWindow".into();
        let facts = [w];
        assert_eq!(needing_desktop(&facts), vec![1]);
        let desktops = HashMap::from([(1, Desktop::Current)]);
        let (chosen, counts) = select(&facts, &desktops, Options::default());
        assert_eq!(chosen, vec![Chosen { index: 0, suspended: true, other_desktop: false }]);
        assert_eq!((counts.suspended, counts.on_other_desktop, counts.listed), (1, 0, 1));
    }

    #[test]
    fn a_shell_cloaked_window_on_this_desktop_that_is_not_minimized_is_no_window_to_switch_to() {
        // the leftover frame of a Store app that was closed: titled, visible, cloaked by the shell, not minimized
        let mut w = win(1, "Leftover");
        w.cloak = CLOAKED_SHELL;
        w.class = "ApplicationFrameWindow".into();
        let facts = [w, win(2, "here")];
        let desktops = HashMap::from([(1, Desktop::Current)]);
        let (chosen, counts) = select(&facts, &desktops, Options::default());
        assert_eq!(chosen.iter().map(|c| facts[c.index].hwnd).collect::<Vec<_>>(), vec![2]);
        assert_eq!((counts.cloaked_not_minimized, counts.suspended, counts.listed), (1, 0, 1));
        // the option for other desktops does not bring it back: it is not on another desktop
        let (chosen, _) = select(&facts, &desktops, Options { other_desktops: true });
        assert_eq!(chosen.len(), 1);
    }

    #[test]
    fn a_shell_cloaked_window_on_another_desktop_is_left_out_by_default_and_counted() {
        let mut w = win(1, "Document");
        w.cloak = CLOAKED_SHELL;
        let facts = [w, win(2, "here")];
        let desktops = HashMap::from([(1, Desktop::Other)]);
        let (chosen, counts) = select(&facts, &desktops, Options::default());
        assert_eq!(chosen.len(), 1);
        assert_eq!(counts.on_other_desktop, 1);
        // the option puts it back, marked
        let (chosen, _) = select(&facts, &desktops, Options { other_desktops: true });
        assert_eq!(chosen.len(), 2);
        assert!(chosen[0].other_desktop);
    }

    #[test]
    fn when_the_desktop_cannot_be_told_a_cloaked_window_is_left_out() {
        let mut w = win(1, "Mystery");
        w.cloak = CLOAKED_SHELL;
        let (chosen, counts) = select(&[w], &HashMap::new(), Options::default());
        assert!(chosen.is_empty());
        assert_eq!(counts.desktop_unknown, 1);
    }

    #[test]
    fn a_cloaked_window_without_a_title_is_not_a_window_of_anything() {
        let mut w = win(1, "  ");
        w.cloak = CLOAKED_SHELL;
        let facts = [w];
        assert!(needing_desktop(&facts).is_empty(), "no need to ask");
        assert!(list(&facts).is_empty());
    }

    #[test]
    fn the_hosted_core_window_of_a_store_app_is_not_listed_its_frame_is() {
        let mut core = win(1, "Settings");
        core.class = "Windows.UI.Core.CoreWindow".into();
        let mut frame = win(2, "Settings");
        frame.class = "ApplicationFrameWindow".into();
        assert_eq!(list(&[core, frame]), vec![2]);
    }

    #[test]
    fn only_the_windows_that_need_it_are_asked_about_their_desktop() {
        let plain = win(1, "plain");
        let mut cloaked = win(2, "cloaked");
        cloaked.cloak = CLOAKED_SHELL;
        let mut inherited = win(3, "dialog of an other-desktop app");
        inherited.cloak = CLOAKED_INHERITED;
        let mut app = win(4, "helper");
        app.cloak = CLOAKED_APP;
        assert_eq!(needing_desktop(&[plain, cloaked, inherited, app]), vec![2, 3]);
    }

    #[test]
    fn minimized_windows_are_listed_and_counted() {
        let mut m = win(1, "min");
        m.minimized = true;
        let (chosen, counts) = select(&[m, win(2, "up")], &HashMap::new(), Options::default());
        assert_eq!(chosen.len(), 2);
        assert_eq!((counts.minimized, counts.listed, counts.windows_seen), (1, 2, 2));
    }

    #[test]
    fn the_window_in_front_goes_first_whatever_the_z_order_says() {
        // 5 is above everything in the z-order; the user is in 7
        let facts = [win(5, "above"), win(6, "other"), win(7, "current"), win(8, "older")];
        let (mut chosen, _) = select(&facts, &HashMap::new(), Options::default());
        assert!(promote_foreground(&mut chosen, &facts, 7));
        let order: Vec<isize> = chosen.iter().map(|c| facts[c.index].hwnd).collect();
        assert_eq!(order, vec![7, 5, 6, 8], "the others keep their order");
    }

    fn order_of(facts: &[WindowFacts], foreground: isize) -> Vec<isize> {
        let (mut chosen, _) = select(facts, &HashMap::new(), Options::default());
        promote_foreground(&mut chosen, facts, foreground);
        chosen.iter().map(|c| facts[c.index].hwnd).collect()
    }

    fn topmost(mut w: WindowFacts) -> WindowFacts {
        w.ex_style |= WS_EX_TOPMOST;
        w
    }

    #[test]
    fn an_always_on_top_window_does_not_stand_between_two_windows_that_are_flipped() {
        // a pinned note T is above everything in the z-order. The user is in A and came from B: Alt+Tab must go back to B.
        let t = topmost(win(1, "pinned note"));
        let (a, b) = (win(2, "A"), win(3, "B"));
        // z-order [T, A, B], A in front
        assert_eq!(order_of(&[t.clone(), a.clone(), b.clone()], 2), vec![2, 3, 1], "the first Tab (index 1) is B");
        // after the flip: z-order [T, B, A], B in front: the next Tab goes back to A
        assert_eq!(order_of(&[t.clone(), b.clone(), a.clone()], 3), vec![3, 2, 1]);
        // and flipping again
        assert_eq!(order_of(&[t, a, b], 2), vec![2, 3, 1]);
    }

    #[test]
    fn an_always_on_top_window_that_is_in_front_is_first_and_the_others_keep_their_order() {
        let t = topmost(win(1, "pinned note"));
        assert_eq!(order_of(&[t, win(2, "A"), win(3, "B")], 1), vec![1, 2, 3]);
    }

    #[test]
    fn always_on_top_windows_come_after_the_ordinary_ones_and_keep_their_order_among_themselves() {
        let facts = [topmost(win(1, "t1")), win(2, "a"), topmost(win(3, "t2")), win(4, "b")];
        let (chosen, _) = select(&facts, &HashMap::new(), Options::default());
        assert_eq!(chosen.iter().map(|c| facts[c.index].hwnd).collect::<Vec<_>>(), vec![2, 4, 1, 3]);
    }

    #[test]
    fn the_foreground_that_is_a_dialog_promotes_the_entry_of_its_app() {
        let mut owner = win(10, "main");
        owner.last_popup = 11;
        let mut dialog = win(11, "dialog");
        dialog.root_owner = 10;
        dialog.last_popup = 11;
        let facts = [win(1, "top"), dialog, owner];
        let (mut chosen, _) = select(&facts, &HashMap::new(), Options::default());
        promote_foreground(&mut chosen, &facts, 10);
        assert_eq!(facts[chosen[0].index].hwnd, 11);
    }

    #[test]
    fn no_foreground_or_an_unlisted_one_changes_nothing() {
        let facts = [win(1, "a"), win(2, "b")];
        let (mut chosen, _) = select(&facts, &HashMap::new(), Options::default());
        let before = chosen.clone();
        assert!(!promote_foreground(&mut chosen, &facts, 0), "no foreground: the first card is not the window in front");
        assert!(!promote_foreground(&mut chosen, &facts, 99), "a foreground that is not listed (the dock, a panel) likewise");
        assert_eq!(chosen, before);
        assert!(promote_foreground(&mut chosen, &facts, 1), "the first window in front stays first and is found");
    }

    #[test]
    fn a_corrupt_popup_chain_cannot_loop() {
        // 1 -> 2 -> 3 -> 2 -> 3 ...: two hidden popups name each other as the last active one. The bound ends the walk, which ends
        // on no visible window, and the window the walk started from stays the entry of its app
        let mut a = win(1, "a");
        a.last_popup = 2;
        let mut b = win(2, "b");
        b.root_owner = 1;
        b.last_popup = 3;
        b.visible = false;
        let mut c = win(3, "c");
        c.root_owner = 1;
        c.last_popup = 2;
        c.visible = false;
        assert_eq!(list(&[a, b, c]), vec![1]);
    }
}
