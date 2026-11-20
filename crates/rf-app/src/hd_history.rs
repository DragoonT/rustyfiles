//! What HyperDownload has finished: the "Finished" list of the Downloads page (`downloads_view.rs`). rustyfiles keeps it itself (the
//! add-on forgets a download once rustyfiles has taken its result), in `<local data>\hyperdownload-history.json`, newest first, at
//! most [`MAX`] entries. A download's card writes its entry when the file is complete (`hyperdownload::download_source`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

/// The most entries kept.
const MAX: usize = 500;

/// Bumped at every change: a page that drew the list at another number reads it again.
static GENERATION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// What the row shows: the file's name, or a torrent's name.
    pub name: String,
    /// The file, or a torrent's folder (or its single file).
    pub path: PathBuf,
    /// Its size in bytes when it finished.
    pub size: u64,
    /// Where it came from: the link, or the `.torrent` file (shown on hover).
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub torrent: bool,
    /// When it finished (milliseconds since 1970, UTC).
    pub finished_ms: i64,
}

fn file() -> Option<PathBuf> {
    crate::settings::local_data_dir().map(|d| d.join("hyperdownload-history.json"))
}

/// The list's change counter.
pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// The entries, newest first.
pub fn load() -> Vec<Entry> {
    file().and_then(|f| std::fs::read(f).ok()).and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save(list: &[Entry]) {
    let Some(f) = file() else { return };
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = f.with_extension("json.tmp");
    if serde_json::to_vec_pretty(list).ok().is_some_and(|b| std::fs::write(&tmp, b).is_ok()) {
        let _ = std::fs::rename(&tmp, &f);
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// `list` with `e` put first: an older entry for the same file goes, and the list keeps at most [`MAX`].
pub fn with_entry(mut list: Vec<Entry>, e: Entry) -> Vec<Entry> {
    list.retain(|o| o.path != e.path);
    list.insert(0, e);
    list.truncate(MAX);
    list
}

/// Adds a finished download.
pub fn record(e: Entry) {
    save(&with_entry(load(), e));
}

/// Takes one file off the list (the file itself stays).
pub fn forget(path: &Path) {
    let mut list = load();
    list.retain(|e| e.path != path);
    save(&list);
}

/// Empties the list (the files stay).
pub fn clear() {
    save(&[]);
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(path: &str, at: i64) -> Entry {
        Entry { name: path.into(), path: PathBuf::from(path), size: 1, source: String::new(), torrent: false, finished_ms: at }
    }

    #[test]
    fn the_newest_comes_first_and_a_file_is_listed_once() {
        let list = with_entry(with_entry(Vec::new(), e("a", 1)), e("b", 2));
        assert_eq!(list.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        let list = with_entry(list, e("a", 3));
        assert_eq!(list.iter().map(|x| (x.name.as_str(), x.finished_ms)).collect::<Vec<_>>(), [("a", 3), ("b", 2)]);
        let many = (0..MAX + 10).fold(Vec::new(), |l, i| with_entry(l, e(&i.to_string(), i as i64)));
        assert_eq!(many.len(), MAX);
        assert_eq!(many[0].name, (MAX + 9).to_string());
    }
}
