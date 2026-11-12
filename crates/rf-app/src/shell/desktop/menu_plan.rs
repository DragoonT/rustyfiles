//! What the desktop's two menus hold, decided from facts only. Pure.
//!
//! `menus.rs` draws the menus from these plans; the plans say WHICH commands are there for a given selection and place, in which
//! order, and which of them can be used. Written down once so that a table can test it: a command that does not belong to a kind of
//! item (Empty Recycle Bin on a folder, Open with on a system icon, Create shortcut in the read-only preview) is not offered, one
//! that belongs but cannot be used now (the bin is empty) is shown dimmed.
//!
//! The shell's own commands ("Show more options") are not planned here: they come from Windows and are loaded in the background.

use rf_core::desktop::items::Origin;
use rf_core::desktop::virtuals::VirtualKind;

/// What one selected item is, as far as a menu cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A file or folder of the user's Desktop or of the Public one.
    File { dir: bool, origin: Origin },
    /// One of the five system icons.
    Virtual(VirtualKind),
}

impl Kind {
    pub fn is_virtual(self) -> bool {
        matches!(self, Kind::Virtual(_))
    }
}

/// Where the menu is opened, as it matters to the plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ItemCtx {
    /// The protected preview: nothing that changes the Desktop folder or the bin.
    pub read_only: bool,
    /// The Recycle Bin has something in it (what its icon shows).
    pub bin_full: bool,
    /// The bin on screen is the user's real one seen from a scratch desktop: it may not be emptied from there (`policy::empty_bin`).
    pub bin_locked: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemCmd {
    Open,
    /// The shell's "Open with" dialog: one file.
    OpenWith,
    /// A shortcut (.lnk) to each selected file or folder, made on the user's Desktop.
    CreateShortcut,
    /// Empty the Recycle Bin (Windows asks first).
    EmptyBin,
    CopyPath,
    Properties,
}

/// A line of a menu: a command (usable or dimmed), a separator, or the place of "Show more options".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row<C> {
    Cmd(C, bool),
    Sep,
    More,
}

/// Joins groups of commands with one separator between neighbours and none at the ends or doubled.
fn join<C>(groups: Vec<Vec<Row<C>>>) -> Vec<Row<C>> {
    let mut out = Vec::new();
    for g in groups.into_iter().filter(|g| !g.is_empty()) {
        if !out.is_empty() {
            out.push(Row::Sep);
        }
        out.extend(g);
    }
    out
}

/// Whether a system icon has a Properties window of its own (Control Panel is an applet list: it has none).
pub const fn virtual_has_properties(k: VirtualKind) -> bool {
    !matches!(k, VirtualKind::ControlPanel)
}

/// The commands of the menu of a selection (never empty: Open is always there).
pub fn item_menu(sel: &[Kind], ctx: ItemCtx) -> Vec<Row<ItemCmd>> {
    use ItemCmd::*;
    let files = !sel.is_empty() && sel.iter().all(|k| !k.is_virtual());
    let single = sel.len() == 1;
    let one_virtual = match sel {
        [Kind::Virtual(k)] => Some(*k),
        _ => None,
    };
    let mut open = vec![Row::Cmd(Open, true)];
    if single && matches!(sel[0], Kind::File { dir: false, .. }) {
        open.push(Row::Cmd(OpenWith, true));
    }
    if files {
        // the shortcut goes on the user's own Desktop, also for an item of the Public one
        open.push(Row::Cmd(CreateShortcut, !ctx.read_only));
    }
    if one_virtual == Some(VirtualKind::RecycleBin) {
        open.push(Row::Cmd(EmptyBin, ctx.bin_full && !ctx.read_only && !ctx.bin_locked));
    }
    let mut info = Vec::new();
    if files {
        info.push(Row::Cmd(CopyPath, true));
    }
    match one_virtual {
        Some(k) if virtual_has_properties(k) => info.push(Row::Cmd(Properties, true)),
        Some(_) => {}
        // several system icons, or files and system icons together: no one window shows them all
        None => info.push(Row::Cmd(Properties, files)),
    }
    join(vec![open, info, vec![Row::More]])
}

/// The four quick buttons above an item menu: cut, copy, rename, delete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Quick {
    pub cut: bool,
    pub copy: bool,
    pub rename: bool,
    pub delete: bool,
}

pub fn item_quick(sel: &[Kind], ctx: ItemCtx) -> Quick {
    // only the user's own files change: not the Public Desktop's (administrator rights) and not a system icon
    let own = !sel.is_empty() && sel.iter().all(|k| matches!(k, Kind::File { origin: Origin::User, .. }));
    let can_change = own && !ctx.read_only;
    Quick { cut: can_change, copy: sel.iter().any(|k| !k.is_virtual()), rename: can_change && sel.len() == 1, delete: can_change }
}

// ---------------------------------------------------------------- the empty desktop

/// The state of the desktop the background menu depends on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BgCtx {
    pub read_only: bool,
    /// The clipboard holds files (paste and paste shortcut).
    pub clipboard_files: bool,
    /// What Ctrl+Z would undo / Ctrl+Y redo (a short name), if anything.
    pub undo: Option<String>,
    pub redo: Option<String>,
    /// The desktop can open a terminal in its folder (it has one).
    pub has_folder: bool,
    /// The settings hide every icon: the menu's own switch cannot bring them back.
    pub icons_forced_hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BgCmd {
    IconSize,
    AlignToGrid,
    AutoArrange,
    ShowIcons,
    Sort,
    Refresh,
    New,
    PasteShortcut,
    Undo(String),
    Redo(String),
    OpenInRustyfiles,
    OpenTerminal,
    DisplaySettings,
    Personalize,
}

/// The menu of the empty desktop. (The paste button above it is its own quick row: `clipboard_files && !read_only`.)
pub fn background_menu(ctx: &BgCtx) -> Vec<Row<BgCmd>> {
    use BgCmd::*;
    let view = vec![
        Row::Cmd(IconSize, true),
        Row::Cmd(AlignToGrid, true),
        // (the arrangement is our own layout file: allowed in the protected preview, like moving an icon)
        Row::Cmd(AutoArrange, true),
        Row::Cmd(ShowIcons, !ctx.icons_forced_hidden),
        Row::Cmd(Sort, true),
        Row::Cmd(Refresh, true),
    ];
    let mut make = vec![Row::Cmd(New, !ctx.read_only), Row::Cmd(PasteShortcut, ctx.clipboard_files && !ctx.read_only)];
    if !ctx.read_only {
        if let Some(u) = &ctx.undo {
            make.push(Row::Cmd(Undo(u.clone()), true));
        }
        if let Some(r) = &ctx.redo {
            make.push(Row::Cmd(Redo(r.clone()), true));
        }
    }
    let mut places = Vec::new();
    if ctx.has_folder {
        places.push(Row::Cmd(OpenInRustyfiles, true));
        places.push(Row::Cmd(OpenTerminal, true));
    }
    // (Windows' own settings pages, which the shell's DesktopBackground handlers would otherwise be the only way to reach)
    places.push(Row::Cmd(DisplaySettings, true));
    places.push(Row::Cmd(Personalize, true));
    join(vec![view, make, places, vec![Row::More]])
}

/// The paste button above the background menu.
pub fn paste_enabled(ctx: &BgCtx) -> bool {
    ctx.clipboard_files && !ctx.read_only
}

#[cfg(test)]
mod tests {
    use super::ItemCmd::*;
    use super::*;

    const U: Origin = Origin::User;
    fn file() -> Kind {
        Kind::File { dir: false, origin: U }
    }
    fn dir() -> Kind {
        Kind::File { dir: true, origin: U }
    }
    fn public() -> Kind {
        Kind::File { dir: false, origin: Origin::Public }
    }
    fn v(k: VirtualKind) -> Kind {
        Kind::Virtual(k)
    }
    fn cmds(rows: &[Row<ItemCmd>]) -> Vec<(ItemCmd, bool)> {
        rows.iter().filter_map(|r| if let Row::Cmd(c, e) = r { Some((*c, *e)) } else { None }).collect()
    }
    const RW: ItemCtx = ItemCtx { read_only: false, bin_full: true, bin_locked: false };

    #[test]
    fn a_file_has_open_open_with_shortcut_path_and_properties() {
        let m = item_menu(&[file()], RW);
        assert_eq!(
            m,
            [
                Row::Cmd(Open, true),
                Row::Cmd(OpenWith, true),
                Row::Cmd(CreateShortcut, true),
                Row::Sep,
                Row::Cmd(CopyPath, true),
                Row::Cmd(Properties, true),
                Row::Sep,
                Row::More
            ]
        );
    }

    #[test]
    fn a_folder_or_several_items_have_no_open_with_but_may_get_a_shortcut() {
        assert!(!cmds(&item_menu(&[dir()], RW)).iter().any(|(c, _)| *c == OpenWith));
        let two = cmds(&item_menu(&[file(), file()], RW));
        assert!(!two.iter().any(|(c, _)| *c == OpenWith), "Open with is for one file");
        assert!(two.contains(&(CreateShortcut, true)));
        assert!(two.contains(&(Properties, true)));
    }

    #[test]
    fn the_read_only_preview_makes_no_shortcut_but_still_opens_and_shows_properties() {
        let m = cmds(&item_menu(&[file()], ItemCtx { read_only: true, bin_full: false, bin_locked: false }));
        assert!(m.contains(&(CreateShortcut, false)));
        assert!(m.contains(&(Open, true)) && m.contains(&(Properties, true)));
    }

    #[test]
    fn an_item_of_the_public_desktop_gets_a_shortcut_on_our_own_desktop() {
        assert!(cmds(&item_menu(&[public()], RW)).contains(&(CreateShortcut, true)));
    }

    #[test]
    fn the_recycle_bin_can_be_opened_emptied_and_asked_about() {
        let m = item_menu(&[v(VirtualKind::RecycleBin)], RW);
        assert_eq!(m, [Row::Cmd(Open, true), Row::Cmd(EmptyBin, true), Row::Sep, Row::Cmd(Properties, true), Row::Sep, Row::More]);
        // an empty bin has nothing to empty, and the protected preview does not empty it
        assert!(cmds(&item_menu(&[v(VirtualKind::RecycleBin)], ItemCtx { read_only: false, bin_full: false, bin_locked: false })).contains(&(EmptyBin, false)));
        assert!(cmds(&item_menu(&[v(VirtualKind::RecycleBin)], ItemCtx { read_only: true, bin_full: true, bin_locked: false })).contains(&(EmptyBin, false)));
    }

    #[test]
    fn a_scratch_desktops_bin_is_the_real_one_and_cannot_be_emptied_from_there() {
        let scratch = ItemCtx { read_only: false, bin_full: true, bin_locked: true };
        let m = cmds(&item_menu(&[v(VirtualKind::RecycleBin)], scratch));
        assert!(m.contains(&(EmptyBin, false)), "shown dimmed, not usable");
        // everything else of the bin's menu still works there
        assert!(m.contains(&(Open, true)) && m.contains(&(Properties, true)));
        // and the lock is about the bin only: files keep their shortcut command
        assert!(cmds(&item_menu(&[file()], scratch)).contains(&(CreateShortcut, true)));
    }

    #[test]
    fn no_other_icon_offers_to_empty_the_bin_or_make_shortcuts_to_itself() {
        for k in [VirtualKind::ThisPc, VirtualKind::UserFiles, VirtualKind::Network, VirtualKind::ControlPanel] {
            let m = cmds(&item_menu(&[v(k)], RW));
            for bad in [EmptyBin, OpenWith, CreateShortcut, CopyPath] {
                assert!(!m.iter().any(|(c, _)| *c == bad), "{k:?} offers {bad:?}");
            }
            assert!(m.contains(&(Open, true)));
        }
        assert!(!cmds(&item_menu(&[file()], RW)).iter().any(|(x, _)| *x == EmptyBin), "a file has no Empty Recycle Bin");
    }

    #[test]
    fn this_pc_has_open_and_properties_and_control_panel_only_open() {
        assert_eq!(item_menu(&[v(VirtualKind::ThisPc)], RW), [Row::Cmd(Open, true), Row::Sep, Row::Cmd(Properties, true), Row::Sep, Row::More]);
        assert_eq!(item_menu(&[v(VirtualKind::ControlPanel)], RW), [Row::Cmd(Open, true), Row::Sep, Row::More], "an applet list has no Properties");
        assert!(virtual_has_properties(VirtualKind::Network) && virtual_has_properties(VirtualKind::UserFiles) && !virtual_has_properties(VirtualKind::ControlPanel));
    }

    #[test]
    fn a_mixed_selection_opens_but_has_no_one_properties_window() {
        let m = cmds(&item_menu(&[file(), v(VirtualKind::ThisPc)], RW));
        assert!(m.contains(&(Open, true)));
        assert!(m.contains(&(Properties, false)));
        assert!(!m.iter().any(|(c, _)| matches!(c, CreateShortcut | CopyPath | OpenWith)));
        let two_icons = cmds(&item_menu(&[v(VirtualKind::ThisPc), v(VirtualKind::Network)], RW));
        assert!(two_icons.contains(&(Properties, false)));
    }

    #[test]
    fn no_menu_starts_or_ends_with_a_separator_or_has_two_in_a_row() {
        let sels: Vec<Vec<Kind>> = vec![
            vec![file()],
            vec![dir()],
            vec![public()],
            vec![file(), dir()],
            vec![file(), v(VirtualKind::ThisPc)],
            vec![v(VirtualKind::ThisPc)],
            vec![v(VirtualKind::ControlPanel)],
            vec![v(VirtualKind::RecycleBin)],
            vec![v(VirtualKind::Network), v(VirtualKind::UserFiles)],
        ];
        for sel in sels {
            for ctx in [RW, ItemCtx { read_only: true, bin_full: false, bin_locked: false }] {
                let m = item_menu(&sel, ctx);
                assert!(matches!(m.first(), Some(Row::Cmd(Open, true))), "{sel:?}");
                assert_eq!(m.last(), Some(&Row::More));
                assert!(m.windows(2).all(|w| !(w[0] == Row::Sep && w[1] == Row::Sep)), "{sel:?}");
                assert!(!matches!(m.last(), Some(Row::Sep)));
            }
        }
    }

    #[test]
    fn the_quick_buttons_follow_who_owns_the_item() {
        let q = |sel: &[Kind], ro: bool| item_quick(sel, ItemCtx { read_only: ro, bin_full: false, bin_locked: false });
        assert_eq!(q(&[file()], false), Quick { cut: true, copy: true, rename: true, delete: true });
        assert_eq!(q(&[file(), dir()], false), Quick { cut: true, copy: true, rename: false, delete: true }, "rename is for one");
        assert_eq!(q(&[public()], false), Quick { cut: false, copy: true, rename: false, delete: false }, "the Public Desktop is copied, not changed");
        assert_eq!(q(&[file(), public()], false), Quick { cut: false, copy: true, rename: false, delete: false });
        assert_eq!(q(&[v(VirtualKind::RecycleBin)], false), Quick { cut: false, copy: false, rename: false, delete: false }, "a system icon is not a file");
        assert_eq!(q(&[file()], true), Quick { cut: false, copy: true, rename: false, delete: false }, "the protected preview only copies");
        assert!(q(&[file(), v(VirtualKind::ThisPc)], false).copy);
        assert_eq!(q(&[], false), Quick { cut: false, copy: false, rename: false, delete: false });
    }

    // ---- the empty desktop

    fn bg(f: impl FnOnce(&mut BgCtx)) -> Vec<Row<BgCmd>> {
        let mut c = BgCtx { has_folder: true, ..Default::default() };
        f(&mut c);
        background_menu(&c)
    }
    fn bg_cmds(rows: &[Row<BgCmd>]) -> Vec<(BgCmd, bool)> {
        rows.iter().filter_map(|r| if let Row::Cmd(c, e) = r { Some((c.clone(), *e)) } else { None }).collect()
    }

    #[test]
    fn the_empty_desktop_has_the_view_commands_new_places_and_windows_own_pages_in_that_order() {
        let m = bg(|_| {});
        let order: Vec<BgCmd> = bg_cmds(&m).into_iter().map(|(c, _)| c).collect();
        assert_eq!(
            order,
            [
                BgCmd::IconSize,
                BgCmd::AlignToGrid,
                BgCmd::AutoArrange,
                BgCmd::ShowIcons,
                BgCmd::Sort,
                BgCmd::Refresh,
                BgCmd::New,
                BgCmd::PasteShortcut,
                BgCmd::OpenInRustyfiles,
                BgCmd::OpenTerminal,
                BgCmd::DisplaySettings,
                BgCmd::Personalize
            ]
        );
        assert_eq!(m.last(), Some(&Row::More));
        assert!(m.windows(2).all(|w| !(w[0] == Row::Sep && w[1] == Row::Sep)));
    }

    #[test]
    fn paste_shortcut_needs_files_on_the_clipboard_and_a_writable_desktop() {
        assert!(bg_cmds(&bg(|_| {})).contains(&(BgCmd::PasteShortcut, false)));
        assert!(bg_cmds(&bg(|c| c.clipboard_files = true)).contains(&(BgCmd::PasteShortcut, true)));
        assert!(bg_cmds(&bg(|c| {
            c.clipboard_files = true;
            c.read_only = true;
        }))
        .contains(&(BgCmd::PasteShortcut, false)));
        assert!(paste_enabled(&BgCtx { clipboard_files: true, ..Default::default() }));
        assert!(!paste_enabled(&BgCtx { clipboard_files: true, read_only: true, ..Default::default() }));
        assert!(!paste_enabled(&BgCtx::default()));
    }

    #[test]
    fn undo_and_redo_are_offered_with_what_they_would_undo_and_only_when_there_is_something() {
        let none = bg_cmds(&bg(|_| {}));
        assert!(!none.iter().any(|(c, _)| matches!(c, BgCmd::Undo(_) | BgCmd::Redo(_))));
        let both = bg_cmds(&bg(|c| {
            c.undo = Some("Silme".into());
            c.redo = Some("Yeniden adlandırma".into());
        }));
        assert!(both.contains(&(BgCmd::Undo("Silme".into()), true)));
        assert!(both.contains(&(BgCmd::Redo("Yeniden adlandırma".into()), true)));
        // the protected preview changes nothing, so it undoes nothing
        let ro = bg_cmds(&bg(|c| {
            c.undo = Some("Silme".into());
            c.read_only = true;
        }));
        assert!(!ro.iter().any(|(c, _)| matches!(c, BgCmd::Undo(_))));
    }

    #[test]
    fn the_read_only_preview_keeps_its_layout_commands_and_loses_new() {
        let m = bg_cmds(&bg(|c| c.read_only = true));
        assert!(m.contains(&(BgCmd::New, false)));
        for layout in [BgCmd::AlignToGrid, BgCmd::AutoArrange, BgCmd::Sort, BgCmd::IconSize] {
            assert!(m.contains(&(layout, true)), "arranging changes our own file only");
        }
    }

    #[test]
    fn show_icons_is_dimmed_when_the_settings_hide_every_icon() {
        assert!(bg_cmds(&bg(|_| {})).contains(&(BgCmd::ShowIcons, true)));
        assert!(bg_cmds(&bg(|c| c.icons_forced_hidden = true)).contains(&(BgCmd::ShowIcons, false)));
    }

    #[test]
    fn a_desktop_without_a_folder_offers_neither_rustyfiles_nor_a_terminal() {
        let m = bg_cmds(&bg(|c| c.has_folder = false));
        assert!(!m.iter().any(|(c, _)| matches!(c, BgCmd::OpenInRustyfiles | BgCmd::OpenTerminal)));
        assert!(m.contains(&(BgCmd::DisplaySettings, true)) && m.contains(&(BgCmd::Personalize, true)));
    }
}
