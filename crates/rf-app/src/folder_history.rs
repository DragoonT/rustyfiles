//! The folders the user went to, kept across runs (`%LOCALAPPDATA%\rustyfiles\folder_history.json`), so they can be found again
//! by a few letters: in the command palette (Ctrl+Shift+P, unless `palette_folder_history` is off) and in the folder-only search
//! (Ctrl+P, `SearchFolderHistory`).
//!
//! A folder a tab lands on is recorded by [`record`]; the list is written a moment later on a worker, merged with what the file
//! holds by then, so two rustyfiles processes keep each other's visits. [`rank`] orders the list for what was typed.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use rf_core::Location;
use serde::{Deserialize, Serialize};

/// How many folders are kept (the least recently visited go first).
const KEEP: usize = 500;
const FILE: &str = "folder_history.json";
/// A burst of visits is written once, this long after the first of them.
const SAVE_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Visit {
    pub path: PathBuf,
    /// When it was last visited (ms since the Unix epoch).
    pub last_ms: i64,
    /// How many times it was visited.
    pub count: u32,
}

#[derive(Default)]
struct Store {
    /// Most recently visited first.
    visits: Vec<Visit>,
    loaded: bool,
    save_pending: bool,
}

static STORE: LazyLock<Mutex<Store>> = LazyLock::new(|| Mutex::new(Store::default()));

fn file() -> Option<PathBuf> {
    // tests load folders too: they must not read or write the user's own history
    if cfg!(test) {
        return None;
    }
    crate::settings::local_data_dir().map(|d| d.join(FILE))
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn read_file() -> Vec<Visit> {
    file().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn loaded(st: &mut Store) -> &mut Vec<Visit> {
    if !st.loaded {
        st.loaded = true;
        st.visits = merge(std::mem::take(&mut st.visits), read_file());
    }
    &mut st.visits
}

/// The same folder whatever its case or a trailing separator (Windows paths).
fn key(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase()
}

/// Notes a visit to `loc` (folders on disk or the network only; a path carrying a password is never kept).
pub fn record(loc: &Location) {
    let Location::Path(p) = loc else { return };
    let s = p.to_string_lossy();
    if s.is_empty() || rf_core::history::redact_credentials(&s) != s {
        return;
    }
    let mut st = STORE.lock();
    // only the list in memory: this runs on the UI thread at every folder load, the first one during the start, so the file is read
    // by the save below on its worker (a visit made before that merges with what the file holds)
    touch(&mut st.visits, p, now_ms());
    if !st.save_pending {
        st.save_pending = true;
        std::thread::spawn(|| {
            std::thread::sleep(SAVE_AFTER);
            save();
        });
    }
}

fn touch(visits: &mut Vec<Visit>, p: &Path, now: i64) {
    let k = key(p);
    match visits.iter().position(|v| key(&v.path) == k) {
        Some(i) => {
            let mut v = visits.remove(i);
            // the folder that is already the latest again (a refresh, a reload) is not another visit
            if i != 0 {
                v.count = v.count.saturating_add(1);
            }
            v.last_ms = now;
            v.path = p.to_path_buf();
            visits.insert(0, v);
        }
        None => {
            visits.insert(0, Visit { path: p.to_path_buf(), last_ms: now, count: 1 });
            visits.truncate(KEEP);
        }
    }
}

/// Both lists in one: a folder in both keeps its latest visit and its larger count; most recent first, at most [`KEEP`].
fn merge(a: Vec<Visit>, b: Vec<Visit>) -> Vec<Visit> {
    let mut out: Vec<Visit> = Vec::with_capacity(a.len().max(b.len()));
    for v in a.into_iter().chain(b) {
        let k = key(&v.path);
        match out.iter_mut().find(|o| key(&o.path) == k) {
            Some(o) => {
                if v.last_ms > o.last_ms {
                    o.last_ms = v.last_ms;
                    o.path = v.path;
                }
                o.count = o.count.max(v.count);
            }
            None => out.push(v),
        }
    }
    out.sort_by(|x, y| y.last_ms.cmp(&x.last_ms));
    out.truncate(KEEP);
    out
}

fn save() {
    let Some(path) = file() else { return };
    // what another process wrote meanwhile is kept; the file is read before the lock is taken, so a visit never waits for the disk
    let on_disk = read_file();
    let mut st = STORE.lock();
    st.save_pending = false;
    let all = merge(st.visits.clone(), on_disk);
    st.visits = all.clone();
    st.loaded = true;
    drop(st);
    let Ok(json) = serde_json::to_vec(&all) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // written beside and moved over, so a crash mid-write cannot leave half a file
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path)) {
        log::warn!("folder history save: {e}");
    }
}

/// The remembered folders, most recently visited first.
pub fn snapshot() -> Vec<Visit> {
    loaded(&mut STORE.lock()).clone()
}

/// The folders matching `q` (any case; empty: all of them), best first, each with how well its name or path matches (comparable
/// with the command palette's command scores). Among equal matches the more recent and the more visited come first.
pub fn rank<'a>(visits: &'a [Visit], q: &str) -> Vec<(i32, &'a Visit)> {
    let q = q.trim().to_lowercase();
    let mut v: Vec<(i32, i32, usize)> = visits
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let matched = if q.is_empty() {
                0
            } else {
                let name = rf_core::history::display_name(&v.path);
                // the folder's own name counts more than a match somewhere in its parents
                let by_name = crate::command_palette::score(&name, &q).map(|s| s + 20);
                let by_path = crate::command_palette::score(&v.path.to_string_lossy(), &q);
                by_name.max(by_path)?
            };
            let recent = 10 - i.min(10) as i32;
            let often = v.count.clamp(1, 32).ilog2() as i32;
            Some((matched, matched + recent + often, i))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.2.cmp(&b.2)));
    v.into_iter().map(|(m, _, i)| (m, &visits[i])).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn visit(p: &str, last_ms: i64, count: u32) -> Visit {
        Visit { path: PathBuf::from(p), last_ms, count }
    }

    #[test]
    fn a_visit_goes_to_the_front_once_and_a_reload_is_not_counted() {
        let mut v = Vec::new();
        touch(&mut v, Path::new(r"C:\a"), 1);
        touch(&mut v, Path::new(r"C:\b"), 2);
        touch(&mut v, Path::new(r"c:\A\"), 3);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].path.as_path(), v[0].count), (Path::new(r"c:\A\"), 2));
        touch(&mut v, Path::new(r"C:\a"), 4);
        assert_eq!(v[0].count, 2, "the latest folder again is a reload");
        assert_eq!(v[0].last_ms, 4);
    }

    #[test]
    fn merging_keeps_the_latest_visit_and_the_larger_count() {
        let m = merge(vec![visit(r"C:\a", 5, 1), visit(r"C:\b", 1, 9)], vec![visit(r"c:\B", 7, 2), visit(r"C:\c", 3, 1)]);
        let got: Vec<_> = m.iter().map(|v| (v.path.to_string_lossy().into_owned(), v.last_ms, v.count)).collect();
        assert_eq!(got, [(r"c:\B".into(), 7, 9), (r"C:\a".into(), 5, 1), (r"C:\c".into(), 3, 1)]);
    }

    #[test]
    fn the_name_beats_the_parents_and_recency_breaks_ties() {
        let v = [visit(r"C:\Users\me\Downloads\old", 3, 1), visit(r"D:\Downloads", 2, 1), visit(r"E:\x", 1, 1)];
        let r: Vec<_> = rank(&v, "down").into_iter().map(|(_, v)| v.path.to_string_lossy().into_owned()).collect();
        assert_eq!(r, [r"D:\Downloads", r"C:\Users\me\Downloads\old"]);
        // nothing typed: all of them, most recent first
        let all: Vec<_> = rank(&v, "").into_iter().map(|(_, v)| v.last_ms).collect();
        assert_eq!(all, [3, 2, 1]);
        assert!(rank(&v, "zzz").is_empty());
    }

    #[test]
    fn a_path_with_a_password_is_not_kept() {
        let before = snapshot().len();
        record(&Location::Path(PathBuf::from("ftp://user:secret@host/share")));
        assert_eq!(snapshot().len(), before);
    }
}
