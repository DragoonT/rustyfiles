//! Undo (Ctrl+Z) and redo (Ctrl+Y) for operations done in rustyfiles. The shell's own undo
//! stack is not scriptable, so we record what each operation did and run the inverse.

use crate::t;
use std::path::PathBuf;

use gpui_kit::*;
use parking_lot::Mutex;
use rf_win::fileops::Transfer;

#[derive(Debug, Clone)]
pub enum UndoOp {
    Renamed { from: PathBuf, to: PathBuf },
    Created { path: PathBuf },
    /// (source, result) pairs reported by the shell.
    Transferred { kind: Transfer, pairs: Vec<(PathBuf, PathBuf)> },
    /// (original path, path inside the Recycle Bin)
    Recycled { pairs: Vec<(PathBuf, PathBuf)> },
    /// "Flatten folder" on `dir`.
    Flattened { dir: PathBuf, rec: crate::ops::Flattened },
    /// One run of the folder organizer (all its moves, renames and recycled files).
    Organized { rec: crate::organizer::Record },
}

static STACK: Mutex<Vec<UndoOp>> = Mutex::new(Vec::new());
static REDO: Mutex<Vec<UndoOp>> = Mutex::new(Vec::new());
const MAX: usize = 50;

/// Records a new user operation (a new operation drops whatever could be redone).
pub fn push(op: UndoOp) {
    REDO.lock().clear();
    let mut s = STACK.lock();
    s.push(op);
    if s.len() > MAX {
        s.remove(0);
    }
}

/// A short name for what an operation did, for a menu entry that says what it would undo ("Geri al: Silme").
fn label(op: &UndoOp) -> &'static str {
    match op {
        UndoOp::Renamed { .. } => t!("Yeniden adlandırma"),
        UndoOp::Created { .. } => t!("Oluşturma"),
        UndoOp::Transferred { kind: Transfer::Copy, .. } => t!("Kopyalama"),
        UndoOp::Transferred { kind: Transfer::Move, .. } => t!("Taşıma"),
        UndoOp::Recycled { .. } => t!("Silme"),
        UndoOp::Flattened { .. } => t!("Klasörü düzleştirme"),
        UndoOp::Organized { .. } => t!("Düzenleme"),
    }
}

/// What Ctrl+Z would undo now and what Ctrl+Y would redo (`None`: nothing).
pub fn next_labels() -> (Option<&'static str>, Option<&'static str>) {
    (STACK.lock().last().map(label), REDO.lock().last().map(label))
}

/// Reverses the most recent operation (in the background, through the Status Center).
pub fn undo(cx: &mut App) {
    let Some(op) = STACK.lock().pop() else { return };
    undo_op(op, cx);
}

/// Whether the most recent operation is a copy, move or recycle that made `path` (a copy or
/// move's new item, or the item's place in the Recycle Bin). The cards that offer "Geri al" ask
/// this: the button undoes the top of the stack, so it must be their own operation's record.
pub fn last_made(path: &std::path::Path) -> bool {
    top_made(&STACK.lock(), path)
}

fn top_made(stack: &[UndoOp], path: &std::path::Path) -> bool {
    match stack.last() {
        Some(UndoOp::Transferred { pairs, .. } | UndoOp::Recycled { pairs }) => pairs.iter().any(|(_, made)| made == path),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    // (not `use super::*`: the toolkit's glob brings an attribute named `test` that shadows the real one)
    use super::{PathBuf, Transfer, UndoOp, top_made};

    #[test]
    fn the_undo_button_of_a_card_needs_its_own_record_on_top_of_the_stack() {
        let made = PathBuf::from(r"C:\rf-test-card\made.bin");
        let copy = UndoOp::Transferred { kind: Transfer::Copy, pairs: vec![(PathBuf::from(r"C:\rf-test-card\src.bin"), made.clone())] };
        let bin = UndoOp::Recycled { pairs: vec![(PathBuf::from(r"C:\rf-test-card\gone.bin"), PathBuf::from(r"C:\$Recycle.Bin\S-1\$R123.bin"))] };
        assert!(!top_made(&[], &made), "an empty stack");
        assert!(top_made(&[copy.clone()], &made));
        assert!(!top_made(&[copy.clone()], std::path::Path::new(r"C:\rf-test-card\src.bin")), "the source is not what the copy made");
        assert!(top_made(std::slice::from_ref(&bin), std::path::Path::new(r"C:\$Recycle.Bin\S-1\$R123.bin")));
        // a newer operation on top: the button would undo that one instead
        assert!(!top_made(&[copy.clone(), UndoOp::Created { path: PathBuf::from(r"C:\rf-test-card\other") }], &made));
        assert!(top_made(&[bin, copy], &made));
    }
}

/// Takes one folder organizer run off the undo stack (the "Geri al" button of its Status Center row
/// undoes it itself) and keeps it for Ctrl+Y. False when it is not on the stack: it ran by itself, or
/// 50 newer operations have pushed it off, or it was undone already.
pub fn forget_organized(token: u64) -> bool {
    // (one lock for the search and the removal: a push from another thread cannot slip in between)
    let op = {
        let mut s = STACK.lock();
        let at = s.iter().rposition(|op| matches!(op, UndoOp::Organized { rec } if rec.token == token));
        at.map(|i| s.remove(i))
    };
    match op {
        Some(op) => {
            REDO.lock().push(op);
            true
        }
        None => false,
    }
}

fn undo_op(op: UndoOp, cx: &mut App) {
    // creating an item can't be replayed (its contents may have changed), everything else can
    if !matches!(op, UndoOp::Created { .. }) {
        REDO.lock().push(op.clone());
    }
    match op {
        UndoOp::Renamed { from, to } => {
            let name = from.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::status_center::start(cx, format!("{} {name}", t!("Geri alınıyor: yeniden adlandırma →")), move |_| {
                rf_win::fileops::rename(&to, &name)
            });
        }
        UndoOp::Created { path } => crate::status_center::delete_ex(cx, vec![path], false, false),
        UndoOp::Transferred { kind: Transfer::Copy, pairs } => {
            crate::status_center::delete_ex(cx, pairs.into_iter().map(|(_, dst)| dst).collect(), false, false)
        }
        UndoOp::Transferred { kind: Transfer::Move, pairs } => {
            // Move everything back, grouped by original folder.
            let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
            for (src, dst) in pairs {
                if let Some(parent) = src.parent() {
                    groups.entry(parent.to_path_buf()).or_default().push(dst);
                }
            }
            for (dir, items) in groups {
                crate::status_center::transfer_ex(cx, Transfer::Move, items, dir, false);
            }
        }
        UndoOp::Recycled { pairs } => {
            let n = pairs.len();
            crate::status_center::start(cx, format!("{} {}", t!("Geri dönüşüm kutusundan geri yükleniyor:"), crate::i18n::items(n)), move |_| {
                let wanted: Vec<String> = pairs.iter().map(|(_, bin)| bin.to_string_lossy().to_lowercase()).collect();
                let items: Vec<_> = rf_win::recycle::list()?
                    .into_iter()
                    .filter(|it| wanted.contains(&it.parsing_path.to_lowercase()))
                    .collect();
                anyhow::ensure!(!items.is_empty(), t!("öğeler geri dönüşüm kutusunda bulunamadı"));
                rf_win::recycle::restore(&items)?;
                Ok(true)
            });
        }
        UndoOp::Flattened { dir, rec } => {
            let name = rf_core::history::display_name(&dir);
            crate::status_center::start(cx, format!("{} {name}", t!("Geri alınıyor: düzleştirme →")), move |_| crate::ops::unflatten(&rec));
        }
        UndoOp::Organized { rec } => crate::organizer::undo_run(rec, None, cx),
    }
}

/// Repeats the last undone operation.
pub fn redo(cx: &mut App) {
    let Some(op) = REDO.lock().pop() else { return };
    // it becomes undoable again
    STACK.lock().push(op.clone());
    match op {
        UndoOp::Renamed { from, to } => {
            let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::status_center::start(cx, format!("{} {name}", t!("Yineleniyor: yeniden adlandırma →")), move |_| {
                rf_win::fileops::rename(&from, &name)
            });
        }
        UndoOp::Created { .. } => {}
        UndoOp::Transferred { kind, pairs } => {
            let mut groups: std::collections::BTreeMap<PathBuf, Vec<PathBuf>> = Default::default();
            for (src, dst) in pairs {
                if let Some(parent) = dst.parent() {
                    groups.entry(parent.to_path_buf()).or_default().push(src);
                }
            }
            for (dir, items) in groups {
                crate::status_center::transfer_ex(cx, kind, items, dir, false);
            }
        }
        UndoOp::Recycled { pairs } => {
            crate::status_center::delete_ex(cx, pairs.into_iter().map(|(orig, _)| orig).collect(), false, false)
        }
        UndoOp::Flattened { dir, rec } => {
            let name = rf_core::history::display_name(&dir);
            crate::status_center::start(cx, format!("{} {name}", t!("Yineleniyor: düzleştirme →")), move |_| crate::ops::reflatten(&rec));
        }
        UndoOp::Organized { rec } => crate::organizer::redo_run(rec, cx),
    }
}
