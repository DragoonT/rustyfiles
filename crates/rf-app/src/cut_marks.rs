//! The items on the clipboard to be MOVED (Ctrl+X), which the lists draw faded until they are pasted or the clipboard holds something
//! else, as Explorer does (issue 201: "The opacity of a file remains the same after cutting it").
//!
//! The mark lives as long as the clipboard content it was made with: the clipboard's sequence number, read once our cut is on it, is
//! kept with the paths; any later change of the clipboard (a copy here or in any other program, a paste that moved the items) gives
//! another number and the mark is gone.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

struct Marks {
    paths: HashSet<PathBuf>,
    /// The clipboard's sequence number with our cut on it; `None` until the clipboard has been written.
    seq: Option<u32>,
}

static MARKS: Mutex<Option<Marks>> = Mutex::new(None);

/// A cut of `paths` is being put on the clipboard (or `None`: a copy, which ends an earlier cut's marks).
pub fn cutting(paths: Option<&[PathBuf]>) {
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    *m = paths.map(|p| Marks { paths: p.iter().cloned().collect(), seq: None });
}

/// The clipboard now holds the cut (called on the thread that wrote it, right after).
pub fn written(paths: &[PathBuf]) {
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(marks) = m.as_mut()
        && marks.seq.is_none()
        && paths.iter().all(|p| marks.paths.contains(p))
    {
        marks.seq = Some(rf_win::clipboard::sequence());
    }
}

/// Whether there is a cut to draw at all (the lists ask this once per frame before asking per item).
pub fn any() -> bool {
    let mut m = MARKS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(marks) = m.as_ref()
        && let Some(seq) = marks.seq
        && seq != rf_win::clipboard::sequence()
    {
        *m = None;
    }
    m.is_some()
}

/// Whether `path` is cut (call after [`any`]).
pub fn is_cut(path: &Path) -> bool {
    MARKS.lock().unwrap_or_else(|e| e.into_inner()).as_ref().is_some_and(|m| m.paths.contains(path))
}
