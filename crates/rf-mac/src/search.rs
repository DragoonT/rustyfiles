//! Searching a folder and every folder under it by name (the Windows app's `Location::Search`
//! when no index covers the folder: the disk is walked). The walk runs on rayon's threads, one
//! task per folder; the Swift side polls the hits and shows them as a gathered listing.

use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rf_core::entry::{contains_ignore_case, matches_patterns};

/// Folders that are one item to the user: they are found, not walked into.
const PACKAGES: &[&str] = &["app", "bundle", "framework", "photoslibrary", "musiclibrary", "tvlibrary", "plugin", "kext", "xcarchive", "pkg", "rtfd", "pages", "numbers", "key", "imovielibrary", "fcpbundle"];

/// The most hits a search keeps.
const MAX_HITS: usize = 50_000;

/// What a typed query asks for: `*` / `?` make it a pattern on the whole name ("*.pdf"), else
/// every word must be in the name ("rapor 2024").
#[derive(Clone)]
enum Query {
    Pattern(String),
    Words(Vec<String>),
}

impl Query {
    fn parse(q: &str) -> Query {
        let q = q.trim();
        if q.contains(['*', '?']) {
            Query::Pattern(q.to_string())
        } else {
            Query::Words(q.split_whitespace().map(str::to_string).collect())
        }
    }

    fn matches(&self, name: &str) -> bool {
        match self {
            Query::Pattern(p) => matches_patterns(p, name),
            Query::Words(w) => !w.is_empty() && w.iter().all(|w| contains_ignore_case(name, w)),
        }
    }
}

/// A search running in the background; dropped or cancelled, it stops.
#[derive(uniffi::Object)]
pub struct SearchJob {
    hits: Arc<Mutex<Vec<String>>>,
    done: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    folders: Arc<AtomicU64>,
}

#[uniffi::export]
impl SearchJob {
    /// Starts looking under `root` for names matching `query`. Hidden items (a leading dot)
    /// only with `include_hidden`; other disks mounted inside the folder are not walked.
    #[uniffi::constructor]
    pub fn start(root: String, query: String, include_hidden: bool) -> Arc<Self> {
        let job = Arc::new(SearchJob {
            hits: Arc::new(Mutex::new(Vec::new())),
            done: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            folders: Arc::new(AtomicU64::new(0)),
        });
        let (hits, done, cancelled, folders) = (job.hits.clone(), job.done.clone(), job.cancelled.clone(), job.folders.clone());
        let q = Query::parse(&query);
        std::thread::Builder::new()
            .name("rf-search".into())
            .spawn(move || {
                let root = Path::new(&root);
                let dev = std::fs::metadata(root).map(|m| m.dev()).unwrap_or(0);
                let w = Walk { q, include_hidden, dev, hits, cancelled, folders };
                w.walk(root);
                done.store(true, Ordering::Release);
            })
            .ok();
        job
    }

    /// The hits found since the last call (full paths).
    pub fn take(&self) -> Vec<String> {
        std::mem::take(&mut *self.hits.lock().unwrap())
    }

    /// Whether the walk has finished (or was cancelled).
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// How many folders were looked through so far.
    pub fn folders_seen(&self) -> u64 {
        self.folders.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for SearchJob {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

struct Walk {
    q: Query,
    include_hidden: bool,
    dev: u64,
    hits: Arc<Mutex<Vec<String>>>,
    cancelled: Arc<AtomicBool>,
    folders: Arc<AtomicU64>,
}

impl Walk {
    fn walk(&self, dir: &Path) {
        if self.cancelled.load(Ordering::Relaxed) {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        self.folders.fetch_add(1, Ordering::Relaxed);
        let mut found = Vec::new();
        let mut below = Vec::new();
        for e in rd.flatten() {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if !self.include_hidden && name.starts_with('.') {
                continue;
            }
            let path = e.path();
            if self.q.matches(&name) {
                found.push(path.display().to_string());
            }
            // folders (not links to them, not packages, not other disks) are walked too
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                let ext = name.rsplit_once('.').map(|(_, x)| x.to_ascii_lowercase()).unwrap_or_default();
                if PACKAGES.contains(&ext.as_str()) {
                    continue;
                }
                if std::fs::symlink_metadata(&path).is_ok_and(|m| m.dev() == self.dev) {
                    below.push(path);
                }
            }
        }
        if !found.is_empty() {
            let mut hits = self.hits.lock().unwrap();
            if hits.len() < MAX_HITS {
                hits.extend(found);
            } else {
                self.cancelled.store(true, Ordering::Relaxed);
            }
        }
        use rayon::prelude::*;
        below.par_iter().for_each(|d| self.walk(d));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_by_words_and_patterns_and_skips_hidden() {
        let d = std::env::temp_dir().join(format!("rf-mac-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::create_dir_all(d.join(".secret")).unwrap();
        for p in ["a/Rapor 2024.pdf", "a/b/rapor-eski.txt", "a/b/notes.md", ".secret/rapor.pdf"] {
            std::fs::write(d.join(p), b"x").unwrap();
        }
        let run = |q: &str, hidden: bool| {
            let job = SearchJob::start(d.display().to_string(), q.into(), hidden);
            while !job.is_done() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            let mut names: Vec<String> = job.take().iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
            names.sort();
            names
        };
        assert_eq!(run("rapor", false), ["Rapor 2024.pdf", "rapor-eski.txt"]);
        assert_eq!(run("rapor 2024", false), ["Rapor 2024.pdf"]);
        assert_eq!(run("*.pdf", false), ["Rapor 2024.pdf"]);
        assert_eq!(run("*.pdf", true), ["Rapor 2024.pdf", "rapor.pdf"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
