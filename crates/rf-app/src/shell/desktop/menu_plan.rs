//! What the desktop's two menus hold, decided from facts only. Pure.
//!
//! `menus.rs` draws the menus from these plans; the plans say WHICH commands are there for a given selection and place, in which
//! order, and which of them can be used. Written down once so that a table can test it: a command that does not belong to a kind of
//! item (Empty Recycle Bin on a folder, Open with on a system icon, Create shortcut in the read-only preview) is not offered, one
//! that belongs but cannot be used now (the bin is empty) is shown dimmed.
//!
//! The shell's own commands ("Show more options") are not planned here: they come from Windows and are loaded in the background.

use rf_core::desktop::grid::IconSize;
use rf_core::desktop::items::Origin;
use rf_core::desktop::store::SortName;
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

/// The top level of the empty desktop's menu: Windows 11's own, in its order (Görünüm, Sıralama ölçütü, Yenile | Yeni | Görüntü
/// ayarları, Kişiselleştir | Terminalde Aç | Daha fazla seçenek göster), plus the few commands of ours that can act only now and then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BgCmd {
    /// The submenu of the icon size, the arrangement and the switch for the icons (see [`view_menu`]).
    View,
    /// The submenu of the sort keys (see [`SORT_ORDER`]).
    SortBy,
    Refresh,
    /// Ours: shown only when files are on the clipboard and the desktop may be written.
    Paste,
    /// Ours: the same condition.
    PasteShortcut,
    /// Ours: shown only when there is something to undo / redo, with what it would undo.
    Undo(String),
    Redo(String),
    New,
    DisplaySettings,
    Personalize,
    OpenTerminal,
    /// Ours, but NOT at the top level: the first entry inside "more options" (see [`more_prefix`]).
    OpenInRustyfiles,
}

/// The menu of the empty desktop, as Windows 11's own is (a picture of it was the model):
///
/// ```text
/// Görünüm >            (submenu)
/// Sıralama ölçütü >    (submenu)
/// Yenile
/// -------------------  only when something below can act:
/// Yapıştır / Kısayol yapıştır / Geri al / Yinele      <- ours, one group, directly under Yenile
/// -------------------
/// Yeni >
/// -------------------
/// Görüntü ayarları
/// Kişiselleştir
/// -------------------
/// Terminalde Aç
/// -------------------
/// Daha fazla seçenek göster >
/// ```
///
/// Nothing else is at the top level: size, arrangement and the icons' switch are in Görünüm, "open in rustyfiles" is the first entry of
/// "more options".
pub fn background_menu(ctx: &BgCtx) -> Vec<Row<BgCmd>> {
    use BgCmd::*;
    let view = vec![Row::Cmd(View, true), Row::Cmd(SortBy, true), Row::Cmd(Refresh, true)];
    let mut ours = Vec::new();
    if paste_enabled(ctx) {
        ours.push(Row::Cmd(Paste, true));
        ours.push(Row::Cmd(PasteShortcut, true));
    }
    if !ctx.read_only {
        if let Some(u) = &ctx.undo {
            ours.push(Row::Cmd(Undo(u.clone()), true));
        }
        if let Some(r) = &ctx.redo {
            ours.push(Row::Cmd(Redo(r.clone()), true));
        }
    }
    let make = vec![Row::Cmd(New, !ctx.read_only)];
    // (Windows' own settings pages, which the shell's DesktopBackground handlers would otherwise be the only way to reach)
    let pages = vec![Row::Cmd(DisplaySettings, true), Row::Cmd(Personalize, true)];
    let terminal: Vec<Row<BgCmd>> = if ctx.has_folder { vec![Row::Cmd(OpenTerminal, true)] } else { Vec::new() };
    join(vec![view, ours, make, pages, terminal, vec![Row::More]])
}

/// Ours, put at the head of "more options" so that the top level stays Windows 11's: empty without a folder to open.
pub fn more_prefix(ctx: &BgCtx) -> Vec<BgCmd> {
    if ctx.has_folder { vec![BgCmd::OpenInRustyfiles] } else { Vec::new() }
}

/// Whether the paste rows are offered (Windows 11's desktop menu has no paste button above it; ours are rows, [`BgCmd::Paste`]).
pub fn paste_enabled(ctx: &BgCtx) -> bool {
    ctx.clipboard_files && !ctx.read_only
}

/// The commands inside Görünüm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewCmd {
    /// One of the three sizes; the chosen one is marked with a dot.
    Size(IconSize),
    AutoArrange,
    /// "Simgeleri kılavuz çizgisine hizala".
    AlignToGrid,
    /// "Masaüstü öğelerini göster".
    ShowIcons,
}

/// Görünüm: the three sizes (largest first, as Windows lists them), the two arrangement switches, the switch for the icons.
pub fn view_menu(ctx: &BgCtx) -> Vec<Row<ViewCmd>> {
    use ViewCmd::*;
    join(vec![
        vec![Row::Cmd(Size(IconSize::Large), true), Row::Cmd(Size(IconSize::Medium), true), Row::Cmd(Size(IconSize::Small), true)],
        // (the arrangement is our own layout file: allowed in the protected preview, like moving an icon)
        vec![Row::Cmd(AutoArrange, true), Row::Cmd(AlignToGrid, true)],
        vec![Row::Cmd(ShowIcons, !ctx.icons_forced_hidden)],
    ])
}

/// The keys that choose a size on the desktop, as Explorer's: Ctrl+Shift+2 large, 3 medium, 4 small.
pub const SIZE_KEYS: [(IconSize, &str); 3] = [(IconSize::Large, "ctrl-shift-2"), (IconSize::Medium, "ctrl-shift-3"), (IconSize::Small, "ctrl-shift-4")];

/// The hint shown at the right of a size row.
pub fn size_hint(size: IconSize) -> &'static str {
    match size {
        IconSize::Large => "Ctrl+Shift+2",
        IconSize::Medium => "Ctrl+Shift+3",
        IconSize::Small => "Ctrl+Shift+4",
    }
}

/// Sıralama ölçütü: Ad, Boyut, Öğe türü, Değiştirme tarihi.
pub const SORT_ORDER: [SortName; 4] = [SortName::Name, SortName::Size, SortName::Type, SortName::Modified];

/// The mark at the right of a sort row: the chosen key shows which way the order runs (an arrow up for ascending, down for
/// descending), the others show nothing. Without it a descending desktop looks like an ascending one, and picking the marked key to
/// "confirm" it would silently turn the icons round.
pub fn sort_hint(current: SortName, descending: bool, row: SortName) -> Option<&'static str> {
    (row == current).then_some(if descending { "\u{2193}" } else { "\u{2191}" })
}

/// What choosing the sort key `chosen` does when the desktop is sorted by `current` (`descending`): the key already in use turns the
/// order round, as it does in Explorer; another key sorts ascending.
pub fn sort_choice(current: SortName, descending: bool, chosen: SortName) -> (SortName, bool) {
    if chosen == current { (chosen, !descending) } else { (chosen, false) }
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
    fn no_doubled_or_edge_separators<C: PartialEq>(m: &[Row<C>]) -> bool {
        m.windows(2).all(|w| !(w[0] == Row::Sep && w[1] == Row::Sep)) && !matches!(m.first(), Some(Row::Sep)) && !matches!(m.last(), Some(Row::Sep))
    }

    #[test]
    fn the_empty_desktop_has_windows_11s_menu_in_its_order_and_grouping() {
        use BgCmd::*;
        // nothing on the clipboard, nothing to undo: exactly the picture
        assert_eq!(
            bg(|_| {}),
            [
                Row::Cmd(View, true),
                Row::Cmd(SortBy, true),
                Row::Cmd(Refresh, true),
                Row::Sep,
                Row::Cmd(New, true),
                Row::Sep,
                Row::Cmd(DisplaySettings, true),
                Row::Cmd(Personalize, true),
                Row::Sep,
                Row::Cmd(OpenTerminal, true),
                Row::Sep,
                Row::More
            ]
        );
    }

    #[test]
    fn our_paste_undo_and_redo_rows_are_one_group_directly_under_refresh_and_only_when_they_can_act() {
        use BgCmd::*;
        let full = bg(|c| {
            c.clipboard_files = true;
            c.undo = Some("Silme".into());
            c.redo = Some("Yeniden adlandırma".into());
        });
        assert_eq!(
            full,
            [
                Row::Cmd(View, true),
                Row::Cmd(SortBy, true),
                Row::Cmd(Refresh, true),
                Row::Sep,
                Row::Cmd(Paste, true),
                Row::Cmd(PasteShortcut, true),
                Row::Cmd(Undo("Silme".into()), true),
                Row::Cmd(Redo("Yeniden adlandırma".into()), true),
                Row::Sep,
                Row::Cmd(New, true),
                Row::Sep,
                Row::Cmd(DisplaySettings, true),
                Row::Cmd(Personalize, true),
                Row::Sep,
                Row::Cmd(OpenTerminal, true),
                Row::Sep,
                Row::More
            ]
        );
        // each appears alone when only its own condition holds
        let only_undo = bg(|c| c.undo = Some("Silme".into()));
        assert_eq!(bg_cmds(&only_undo)[3], (Undo("Silme".into()), true));
        assert!(!bg_cmds(&only_undo).iter().any(|(c, _)| matches!(c, Paste | PasteShortcut | Redo(_))));
        let only_paste = bg_cmds(&bg(|c| c.clipboard_files = true));
        assert!(only_paste.contains(&(Paste, true)) && only_paste.contains(&(PasteShortcut, true)));
        assert!(!only_paste.iter().any(|(c, _)| matches!(c, Undo(_) | Redo(_))));
        // the protected preview changes nothing: none of the four, whatever the clipboard and the undo stack hold
        let ro = bg_cmds(&bg(|c| {
            c.read_only = true;
            c.clipboard_files = true;
            c.undo = Some("Silme".into());
            c.redo = Some("Silme".into());
        }));
        assert!(!ro.iter().any(|(c, _)| matches!(c, Paste | PasteShortcut | Undo(_) | Redo(_))), "{ro:?}");
        assert!(ro.contains(&(New, false)), "New is there and dimmed");
    }

    #[test]
    fn nothing_else_is_at_the_top_level_and_no_separator_is_doubled_or_at_an_edge() {
        use BgCmd::*;
        for flags in 0u8..16 {
            for has_folder in [true, false] {
                let m = bg(|c| {
                    c.has_folder = has_folder;
                    c.read_only = flags & 1 != 0;
                    c.clipboard_files = flags & 2 != 0;
                    if flags & 4 != 0 {
                        c.undo = Some("a".into());
                    }
                    if flags & 8 != 0 {
                        c.redo = Some("b".into());
                    }
                });
                assert!(no_doubled_or_edge_separators(&m), "{flags} {has_folder}: {m:?}");
                assert_eq!(m.last(), Some(&Row::More));
                for (c, _) in bg_cmds(&m) {
                    assert!(matches!(c, View | SortBy | Refresh | Paste | PasteShortcut | Undo(_) | Redo(_) | New | DisplaySettings | Personalize | OpenTerminal), "{c:?} at the top level");
                }
            }
        }
    }

    #[test]
    fn open_in_rustyfiles_is_the_first_of_more_options_and_only_with_a_folder() {
        let with = BgCtx { has_folder: true, ..Default::default() };
        let without = BgCtx { has_folder: false, ..Default::default() };
        assert_eq!(more_prefix(&with), [BgCmd::OpenInRustyfiles]);
        assert!(more_prefix(&without).is_empty());
        assert!(!bg_cmds(&background_menu(&with)).iter().any(|(c, _)| *c == BgCmd::OpenInRustyfiles));
    }

    #[test]
    fn a_desktop_without_a_folder_has_no_terminal_and_keeps_the_pages_of_windows() {
        let m = bg(|c| c.has_folder = false);
        let cmds = bg_cmds(&m);
        assert!(!cmds.iter().any(|(c, _)| *c == BgCmd::OpenTerminal));
        assert!(cmds.contains(&(BgCmd::DisplaySettings, true)) && cmds.contains(&(BgCmd::Personalize, true)));
        assert!(no_doubled_or_edge_separators(&m));
    }

    #[test]
    fn paste_needs_files_on_the_clipboard_and_a_writable_desktop() {
        assert!(paste_enabled(&BgCtx { clipboard_files: true, ..Default::default() }));
        assert!(!paste_enabled(&BgCtx { clipboard_files: true, read_only: true, ..Default::default() }));
        assert!(!paste_enabled(&BgCtx::default()));
    }

    #[test]
    fn gorunum_holds_the_three_sizes_the_two_arrangement_switches_and_the_icons_switch() {
        use ViewCmd::*;
        let m = view_menu(&BgCtx::default());
        assert_eq!(
            m,
            [
                Row::Cmd(Size(IconSize::Large), true),
                Row::Cmd(Size(IconSize::Medium), true),
                Row::Cmd(Size(IconSize::Small), true),
                Row::Sep,
                Row::Cmd(AutoArrange, true),
                Row::Cmd(AlignToGrid, true),
                Row::Sep,
                Row::Cmd(ShowIcons, true)
            ]
        );
        // the layout is our own file: the protected preview keeps all of it
        assert_eq!(view_menu(&BgCtx { read_only: true, ..Default::default() }), m);
        // the settings hide every icon: the switch cannot bring them back, so it is shown dimmed
        assert_eq!(view_menu(&BgCtx { icons_forced_hidden: true, ..Default::default() }).last(), Some(&Row::Cmd(ShowIcons, false)));
        assert!(no_doubled_or_edge_separators(&m));
    }

    #[test]
    fn the_size_keys_are_explorers_and_each_has_its_hint() {
        assert_eq!(SIZE_KEYS, [(IconSize::Large, "ctrl-shift-2"), (IconSize::Medium, "ctrl-shift-3"), (IconSize::Small, "ctrl-shift-4")]);
        for (size, key) in SIZE_KEYS {
            // the hint is the key written as the menus write it
            let written: String = key.split('-').map(|w| w[..1].to_uppercase() + &w[1..]).collect::<Vec<_>>().join("+");
            assert_eq!(size_hint(size), written, "{size:?}");
        }
        // every size is reachable
        for s in [IconSize::Small, IconSize::Medium, IconSize::Large] {
            assert!(SIZE_KEYS.iter().any(|(k, _)| *k == s), "{s:?}");
        }
    }

    #[test]
    fn the_sort_menu_lists_ad_boyut_oge_turu_degistirme_tarihi_and_the_chosen_key_turns_the_order_round() {
        assert_eq!(SORT_ORDER, [SortName::Name, SortName::Size, SortName::Type, SortName::Modified]);
        // another key: ascending; the same key: the other way round, and back
        assert_eq!(sort_choice(SortName::Name, false, SortName::Size), (SortName::Size, false));
        assert_eq!(sort_choice(SortName::Name, true, SortName::Size), (SortName::Size, false), "a new key starts ascending");
        assert_eq!(sort_choice(SortName::Size, false, SortName::Size), (SortName::Size, true));
        assert_eq!(sort_choice(SortName::Size, true, SortName::Size), (SortName::Size, false));
    }

    #[test]
    fn the_chosen_sort_key_shows_which_way_the_order_runs_and_no_other_row_shows_anything() {
        assert_eq!(sort_hint(SortName::Name, false, SortName::Name), Some("\u{2191}"));
        assert_eq!(sort_hint(SortName::Name, true, SortName::Name), Some("\u{2193}"), "a descending desktop does not look like an ascending one");
        for row in SORT_ORDER.into_iter().filter(|r| *r != SortName::Size) {
            assert_eq!(sort_hint(SortName::Size, true, row), None, "{row:?}");
        }
    }
}
