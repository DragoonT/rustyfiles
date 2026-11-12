//! What a drop on the desktop means. Pure.
//!
//! Two kinds of drops reach the desktop. Icons dragged inside it (`dragged` are item ids) and files dragged in from other programs (or
//! back in, after leaving the window). Both end in one of four things:
//!
//! * the icons MOVE to the cell under the pointer: only our own layout file changes (allowed even in the read-only preview);
//! * the files are HANDED to the item under the pointer, as the file list does: a folder receives them (copy or move by the usual
//!   rules), a program is started with them, the Recycle Bin takes them;
//! * files from outside are TAKEN IN: dropped on an empty place of the desktop they are copied or moved into the Desktop folder;
//! * nothing happens (the pointer is outside the canvas, the drop would put an item into itself, the preview is read-only).

use rf_core::desktop::grid::Cell;
use rf_core::desktop::items::ItemId;
use rf_win::fileops::Transfer;

/// What an item under the pointer does with files dropped on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Accepts {
    /// A folder (also a folder shortcut is NOT one: a shortcut is a program of its own kind and is handed the files like a program).
    Folder,
    /// A program, or a shortcut: the files are given to it to open.
    Program,
    /// The Recycle Bin.
    RecycleBin,
    /// A document, a picture, This PC ...: nothing is done with files dropped on it, the drop counts as one on the empty cell.
    Nothing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Over {
    pub id: ItemId,
    pub accepts: Accepts,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropAction {
    /// Move the dragged icons so that the one that was grabbed lands on `cell`.
    MoveIcons { cell: Cell },
    /// Give the files to this item.
    HandTo { target: ItemId, accepts: Accepts },
    /// Files from outside, dropped on an empty place: into the Desktop folder.
    TakeIn,
    Reject(Why),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    OutsideCanvas,
    ReadOnly,
    /// A folder dropped into itself.
    IntoItself,
    /// Virtual items (This PC ...) have no file to give to a folder.
    NotFiles,
}

/// Where the drop is.
pub struct Where<'a> {
    /// The item under the pointer, if any.
    pub over: Option<&'a Over>,
    /// The cell under the pointer, `None` outside the grid.
    pub cell: Option<Cell>,
}

/// Dragged icons (inside the window).
/// * `all_files`: every dragged item is a file or folder on the disk (a virtual item can be moved but not handed anywhere);
/// * `read_only`: the preview does not change the Desktop folder, so nothing that moves, deletes or creates a file.
pub fn decide_icons(dragged: &[ItemId], at: &Where, all_files: bool, read_only: bool) -> DropAction {
    if let Some(over) = at.over {
        let itself = dragged.contains(&over.id);
        if !itself && all_files {
            match over.accepts {
                Accepts::Folder | Accepts::RecycleBin if read_only => return DropAction::Reject(Why::ReadOnly),
                Accepts::Folder | Accepts::RecycleBin | Accepts::Program => return DropAction::HandTo { target: over.id.clone(), accepts: over.accepts },
                Accepts::Nothing => {}
            }
        }
    }
    match at.cell {
        Some(cell) => DropAction::MoveIcons { cell },
        None => DropAction::Reject(Why::OutsideCanvas),
    }
}

/// Files dragged in, as paths. `from_here`: every path is one of this desktop's own items (they left the window and came back, or
/// were dragged from another window of this desktop): then it is a move of icons when dropped on the background.
pub fn decide_external(from_here: bool, dragged_here: &[ItemId], at: &Where, read_only: bool) -> DropAction {
    if let Some(over) = at.over {
        let itself = dragged_here.contains(&over.id);
        if !itself {
            match over.accepts {
                Accepts::Folder | Accepts::RecycleBin if read_only => return DropAction::Reject(Why::ReadOnly),
                Accepts::Folder | Accepts::RecycleBin | Accepts::Program => return DropAction::HandTo { target: over.id.clone(), accepts: over.accepts },
                Accepts::Nothing => {}
            }
        } else if from_here && matches!(over.accepts, Accepts::Folder) {
            // a folder dropped on itself: it stays where it is (a move of the icon to its own cell at most)
            return match at.cell {
                Some(cell) => DropAction::MoveIcons { cell },
                None => DropAction::Reject(Why::IntoItself),
            };
        }
    }
    if from_here {
        return match at.cell {
            Some(cell) => DropAction::MoveIcons { cell },
            None => DropAction::Reject(Why::OutsideCanvas),
        };
    }
    if at.cell.is_none() && at.over.is_none() {
        return DropAction::Reject(Why::OutsideCanvas);
    }
    if read_only { DropAction::Reject(Why::ReadOnly) } else { DropAction::TakeIn }
}

/// Whether dropping the dragged icons on `over` would hand them to it (so that it is lit as a target). The same decision as the drop's.
pub fn lights_up(dragged: &[ItemId], over: &Over, all_files: bool, read_only: bool) -> bool {
    let at = Where { over: Some(over), cell: Some(Cell::new(0, 0)) };
    matches!(decide_icons(dragged, &at, all_files, read_only), DropAction::HandTo { .. })
}

/// What the pointer shows while files from another program are dragged over the desktop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    /// The drop would do nothing (refused): the "not allowed" pointer.
    Refused,
    Copy,
    Move,
}

/// The pointer's effect for what the drop WOULD do, so that it never promises a copy or a move that will not happen. `take_in`: the
/// usual copy-or-move into the Desktop folder; `into_folder`: the same for the folder under the pointer (`None` when it is not one).
pub fn effect_of(action: &DropAction, take_in: Transfer, into_folder: Option<Transfer>) -> Effect {
    let of = |t: Transfer| match t {
        Transfer::Copy => Effect::Copy,
        Transfer::Move => Effect::Move,
    };
    match action {
        DropAction::Reject(_) => Effect::Refused,
        // our own items put down on another cell: nothing is copied
        DropAction::MoveIcons { .. } => Effect::Move,
        DropAction::TakeIn => of(take_in),
        DropAction::HandTo { accepts: Accepts::Folder, .. } => into_folder.map_or(Effect::Refused, of),
        DropAction::HandTo { accepts: Accepts::RecycleBin, .. } => Effect::Move,
        // a program is started with the files: they are not moved or copied anywhere
        DropAction::HandTo { accepts: Accepts::Program, .. } => Effect::Copy,
        DropAction::HandTo { accepts: Accepts::Nothing, .. } => Effect::Refused,
    }
}

/// What kind of target an item is, from what the file list knows about it: a directory, an executable or shortcut, the bin.
pub fn accepts_of(is_dir: bool, is_shortcut_or_program: bool, is_recycle_bin: bool, is_virtual: bool) -> Accepts {
    if is_recycle_bin {
        Accepts::RecycleBin
    } else if is_virtual {
        Accepts::Nothing
    } else if is_dir {
        Accepts::Folder
    } else if is_shortcut_or_program {
        Accepts::Program
    } else {
        Accepts::Nothing
    }
}

/// A file name that is given the files dropped on it to open: programs and shortcuts.
pub fn is_program_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    [".exe", ".lnk", ".bat", ".cmd", ".com"].iter().any(|e| lower.ends_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: &str) -> ItemId {
        ItemId::user(n)
    }
    fn over(n: &str, a: Accepts) -> Over {
        Over { id: id(n), accepts: a }
    }
    const C: Cell = Cell::new(3, 4);

    #[test]
    fn icons_dropped_on_an_empty_cell_move() {
        let at = Where { over: None, cell: Some(C) };
        assert_eq!(decide_icons(&[id("a")], &at, true, false), DropAction::MoveIcons { cell: C });
        // moving only touches our layout file, so the read-only preview allows it
        assert_eq!(decide_icons(&[id("a")], &at, true, true), DropAction::MoveIcons { cell: C });
        // outside the grid: nothing
        let out = Where { over: None, cell: None };
        assert_eq!(decide_icons(&[id("a")], &out, true, false), DropAction::Reject(Why::OutsideCanvas));
    }

    #[test]
    fn icons_dropped_on_themselves_or_on_something_that_takes_nothing_move_to_that_cell() {
        let itself = over("a", Accepts::Folder);
        let at = Where { over: Some(&itself), cell: Some(C) };
        assert_eq!(decide_icons(&[id("a"), id("b")], &at, true, false), DropAction::MoveIcons { cell: C }, "onto itself");
        let doc = over("doc.txt", Accepts::Nothing);
        let at = Where { over: Some(&doc), cell: Some(C) };
        assert_eq!(decide_icons(&[id("a")], &at, true, false), DropAction::MoveIcons { cell: C }, "the store finds the nearest free cell");
    }

    #[test]
    fn icons_dropped_on_a_folder_a_program_or_the_bin_are_handed_over() {
        for (a, name) in [(Accepts::Folder, "dir"), (Accepts::Program, "app.exe"), (Accepts::RecycleBin, "bin")] {
            let o = over(name, a);
            let at = Where { over: Some(&o), cell: Some(C) };
            assert_eq!(decide_icons(&[id("a")], &at, true, false), DropAction::HandTo { target: id(name), accepts: a });
        }
    }

    #[test]
    fn the_read_only_preview_hands_files_to_programs_but_not_into_folders_or_the_bin() {
        let prog = over("app.exe", Accepts::Program);
        let at = Where { over: Some(&prog), cell: Some(C) };
        assert!(matches!(decide_icons(&[id("a")], &at, true, true), DropAction::HandTo { .. }));
        for a in [Accepts::Folder, Accepts::RecycleBin] {
            let o = over("x", a);
            let at = Where { over: Some(&o), cell: Some(C) };
            assert_eq!(decide_icons(&[id("a")], &at, true, true), DropAction::Reject(Why::ReadOnly));
        }
    }

    #[test]
    fn virtual_items_can_be_moved_but_not_handed_to_a_folder() {
        let dir = over("dir", Accepts::Folder);
        let at = Where { over: Some(&dir), cell: Some(C) };
        let this_pc = ItemId::virtual_item(rf_core::desktop::virtuals::VirtualKind::ThisPc);
        assert_eq!(decide_icons(&[this_pc], &at, false, false), DropAction::MoveIcons { cell: C });
    }

    #[test]
    fn files_from_outside_on_an_empty_place_are_taken_in_unless_read_only() {
        let at = Where { over: None, cell: Some(C) };
        assert_eq!(decide_external(false, &[], &at, false), DropAction::TakeIn);
        assert_eq!(decide_external(false, &[], &at, true), DropAction::Reject(Why::ReadOnly));
        let out = Where { over: None, cell: None };
        assert_eq!(decide_external(false, &[], &out, false), DropAction::Reject(Why::OutsideCanvas));
        // on a document: the same as on the empty place
        let doc = over("d.txt", Accepts::Nothing);
        let at = Where { over: Some(&doc), cell: Some(C) };
        assert_eq!(decide_external(false, &[], &at, false), DropAction::TakeIn);
    }

    #[test]
    fn files_from_outside_on_a_folder_a_program_or_the_bin_are_handed_over() {
        let dir = over("dir", Accepts::Folder);
        let at = Where { over: Some(&dir), cell: Some(C) };
        assert_eq!(decide_external(false, &[], &at, false), DropAction::HandTo { target: id("dir"), accepts: Accepts::Folder });
        assert_eq!(decide_external(false, &[], &at, true), DropAction::Reject(Why::ReadOnly));
        let prog = over("p.lnk", Accepts::Program);
        let at = Where { over: Some(&prog), cell: Some(C) };
        assert!(matches!(decide_external(false, &[], &at, true), DropAction::HandTo { .. }));
    }

    #[test]
    fn our_own_items_that_come_back_are_moves_not_copies() {
        let at = Where { over: None, cell: Some(C) };
        assert_eq!(decide_external(true, &[id("a")], &at, false), DropAction::MoveIcons { cell: C });
        assert_eq!(decide_external(true, &[id("a")], &at, true), DropAction::MoveIcons { cell: C }, "also read-only: only the layout changes");
        // a folder dropped on itself does not go into itself
        let me = over("a", Accepts::Folder);
        let at = Where { over: Some(&me), cell: Some(C) };
        assert_eq!(decide_external(true, &[id("a")], &at, false), DropAction::MoveIcons { cell: C });
        let at = Where { over: Some(&me), cell: None };
        assert_eq!(decide_external(true, &[id("a")], &at, false), DropAction::Reject(Why::IntoItself));
        // our item dropped on another folder is still handed to it
        let other = over("b", Accepts::Folder);
        let at = Where { over: Some(&other), cell: Some(C) };
        assert!(matches!(decide_external(true, &[id("a")], &at, false), DropAction::HandTo { .. }));
    }

    #[test]
    fn only_a_target_that_will_take_the_files_lights_up() {
        let dir = over("dir", Accepts::Folder);
        let prog = over("app.exe", Accepts::Program);
        let doc = over("d.txt", Accepts::Nothing);
        assert!(lights_up(&[id("a")], &dir, true, false));
        assert!(!lights_up(&[id("a")], &dir, true, true), "read-only: the folder does not take them, so it is not lit");
        assert!(lights_up(&[id("a")], &prog, true, true), "a program is still handed the files");
        assert!(!lights_up(&[id("a")], &doc, true, false));
        assert!(!lights_up(&[id("dir")], &dir, true, false), "not onto itself");
        assert!(!lights_up(&[id("a")], &dir, false, false), "virtual items cannot be handed to a folder");
    }

    #[test]
    fn the_pointer_never_promises_what_the_drop_will_not_do() {
        let at_cell = Where { over: None, cell: Some(C) };
        let dir = over("dir", Accepts::Folder);
        let on_dir = Where { over: Some(&dir), cell: Some(C) };
        let prog = over("p.exe", Accepts::Program);
        let on_prog = Where { over: Some(&prog), cell: Some(C) };
        let bin = over("bin", Accepts::RecycleBin);
        let on_bin = Where { over: Some(&bin), cell: Some(C) };
        let out = Where { over: None, cell: None };
        // writable: the usual rules
        let e = |a: &DropAction, t: Transfer, f: Option<Transfer>| effect_of(a, t, f);
        assert_eq!(e(&decide_external(false, &[], &at_cell, false), Transfer::Move, None), Effect::Move);
        assert_eq!(e(&decide_external(false, &[], &at_cell, false), Transfer::Copy, None), Effect::Copy);
        assert_eq!(e(&decide_external(false, &[], &on_dir, false), Transfer::Move, Some(Transfer::Copy)), Effect::Copy, "the folder's own rule, not the desktop's");
        assert_eq!(e(&decide_external(false, &[], &on_bin, false), Transfer::Copy, None), Effect::Move);
        assert_eq!(e(&decide_external(false, &[], &out, false), Transfer::Copy, None), Effect::Refused);
        // read-only: a refused drop shows the "not allowed" pointer, and so does anything else that changes nothing
        assert_eq!(e(&decide_external(false, &[], &at_cell, true), Transfer::Move, None), Effect::Refused);
        assert_eq!(e(&decide_external(false, &[], &on_dir, true), Transfer::Move, Some(Transfer::Move)), Effect::Refused);
        assert_eq!(e(&decide_external(false, &[], &on_bin, true), Transfer::Move, None), Effect::Refused);
        // handing files to a program moves nothing, so it never says Move
        assert_eq!(e(&decide_external(false, &[], &on_prog, true), Transfer::Move, None), Effect::Copy);
        assert_eq!(e(&decide_external(false, &[], &on_prog, false), Transfer::Move, None), Effect::Copy);
        // a folder that cannot take them (the Public Desktop's) is refused too
        assert_eq!(e(&decide_external(false, &[], &on_dir, false), Transfer::Move, None), Effect::Refused);
    }

    #[test]
    fn target_kinds() {
        assert_eq!(accepts_of(true, false, false, false), Accepts::Folder);
        assert_eq!(accepts_of(false, true, false, false), Accepts::Program);
        assert_eq!(accepts_of(false, false, false, false), Accepts::Nothing);
        assert_eq!(accepts_of(true, false, true, true), Accepts::RecycleBin);
        assert_eq!(accepts_of(true, false, false, true), Accepts::Nothing, "This PC takes nothing");
        assert!(is_program_name("Setup.EXE") && is_program_name("Chrome.lnk") && !is_program_name("notes.txt"));
    }
}
