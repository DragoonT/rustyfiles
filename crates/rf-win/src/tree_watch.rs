//! A folder and everything below it watched for changes (the folder organizer's rules with
//! "include subfolders"). Like [`crate::watch`], but recursive, and it reports full paths.
//! One watcher and one thread per watched tree; dropping the value stops both.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct TreeWatch {
    _w: RecommendedWatcher,
}

/// What changed in one burst.
#[derive(Debug, Default, Clone)]
pub struct TreeChanges {
    /// Paths that were added, removed, renamed from or to, or changed.
    pub paths: Vec<PathBuf>,
    /// The paths can't be told (events were lost, too many): look at the whole tree again.
    pub rescan: bool,
}

/// More changed paths than this in one burst: looking at the tree again is cheaper.
const MAX_PATHS: usize = 2048;

/// Calls `on_change` (from a background thread) once changes in the tree pause for `debounce`,
/// and during a burst that never pauses at least every `debounce` x 5.
pub fn watch_tree(dir: &Path, debounce: Duration, on_change: impl Fn(TreeChanges) + Send + Sync + 'static) -> anyhow::Result<TreeWatch> {
    enum Ev {
        Paths(Vec<PathBuf>),
        Rescan,
    }
    let (tx, rx) = crossbeam_channel::unbounded::<Ev>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
        Ok(ev) => {
            if !matches!(ev.kind, notify::EventKind::Access(_)) {
                let _ = tx.send(if ev.need_rescan() || ev.paths.is_empty() { Ev::Rescan } else { Ev::Paths(ev.paths) });
            }
        }
        Err(_) => {
            let _ = tx.send(Ev::Rescan);
        }
    })?;
    w.watch(dir, RecursiveMode::Recursive)?;
    let on_change = Arc::new(on_change);
    let max_wait = debounce * 5;
    std::thread::Builder::new().name("rf-tree-watch".into()).spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut ch = TreeChanges::default();
            let mut seen = std::collections::HashSet::new();
            let take = |ev: Ev, ch: &mut TreeChanges, seen: &mut std::collections::HashSet<PathBuf>| match ev {
                Ev::Rescan => ch.rescan = true,
                Ev::Paths(ps) => {
                    for p in ps {
                        if !ch.rescan && seen.insert(p.clone()) {
                            ch.paths.push(p);
                            if ch.paths.len() > MAX_PATHS {
                                ch.rescan = true;
                            }
                        }
                    }
                }
            };
            take(first, &mut ch, &mut seen);
            let burst = std::time::Instant::now();
            loop {
                let left = max_wait.saturating_sub(burst.elapsed());
                if left.is_zero() {
                    break;
                }
                match rx.recv_timeout(debounce.min(left)) {
                    Ok(ev) => take(ev, &mut ch, &mut seen),
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                }
            }
            if ch.rescan {
                ch.paths.clear();
            }
            on_change(ch);
        }
    })?;
    Ok(TreeWatch { _w: w })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn reports_files_in_subfolders() {
        let dir = std::env::temp_dir().join(format!("rf-treewatch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("alt").join("derin")).unwrap();
        let got = Arc::new(std::sync::Mutex::new(Vec::<TreeChanges>::new()));
        let g2 = got.clone();
        let w = watch_tree(&dir, Duration::from_millis(100), move |c| g2.lock().unwrap().push(c)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(dir.join("alt").join("derin").join("yeni şarkı.txt"), b"x").unwrap();
        let start = Instant::now();
        let mut found = false;
        while start.elapsed() < Duration::from_secs(4) && !found {
            for c in got.lock().unwrap().iter() {
                found |= c.rescan || c.paths.iter().any(|p| p.file_name().is_some_and(|n| n == "yeni şarkı.txt"));
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(w);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(found, "{:?}", got.lock().unwrap());
    }
}
