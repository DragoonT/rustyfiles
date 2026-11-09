//! Keeps [`crate::nameindex::Index`]es of chosen roots (drives) ready for searching.
//!
//! [`start`] loads each root's saved index (usable at once), then walks the root again in the
//! background and swaps the fresh index in: changes made while the app wasn't running are picked
//! up that way (reading the USN journal would need administrator rights). A recursive watcher on
//! each root collects changed folders; once a second they are listed again
//! ([`crate::nameindex::Index::refresh_dirs`]). Indexes are saved after a build and at most every
//! ten minutes while they change. [`stop`] ends it all.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use parking_lot::{Mutex, RwLock};

use crate::nameindex::Index;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing usable yet: the first walk is running.
    Building,
    /// Searchable (a saved index may be refreshing behind it).
    Ready,
    /// The root couldn't be indexed.
    Failed,
}

struct Slot {
    root: PathBuf,
    file: PathBuf,
    index: RwLock<Option<Index>>,
    state: AtomicU8,
    dirty: Mutex<HashSet<PathBuf>>,
    changed_since_save: AtomicBool,
}

struct Service {
    slots: Vec<Arc<Slot>>,
    cancel: Arc<AtomicBool>,
    _watchers: Vec<notify::RecommendedWatcher>,
}

static SERVICE: OnceLock<Mutex<Option<Service>>> = OnceLock::new();

fn service() -> &'static Mutex<Option<Service>> {
    SERVICE.get_or_init(|| Mutex::new(None))
}

fn file_for(dir: &Path, root: &Path) -> PathBuf {
    let tag: String = root.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    dir.join(format!("{}.rfidx", tag.trim_matches('_')))
}

/// Indexes `roots`, saving the indexes in `dir`. Replaces a running set.
pub fn start(roots: Vec<PathBuf>, dir: PathBuf) {
    stop();
    let _ = std::fs::create_dir_all(&dir);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut slots = Vec::new();
    let mut watchers = Vec::new();
    for root in roots {
        let slot = Arc::new(Slot {
            file: file_for(&dir, &root),
            root,
            index: RwLock::new(None),
            state: AtomicU8::new(State::Building as u8),
            dirty: Mutex::new(HashSet::new()),
            changed_since_save: AtomicBool::new(false),
        });
        // changed folders, collected from the start (a walk running meanwhile misses nothing:
        // the fresh index is refreshed with them once it's swapped in)
        let s2 = slot.clone();
        let ignore = dir.clone();
        match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(ev) = res else { return };
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            let mut d = s2.dirty.lock();
            for p in ev.paths {
                if p.starts_with(&ignore) {
                    continue;
                }
                if let Some(parent) = p.parent() {
                    d.insert(parent.to_path_buf());
                }
            }
        }) {
            Ok(mut w) => {
                if let Err(e) = w.watch(&slot.root, RecursiveMode::Recursive) {
                    log::info!("indexer: no watch on {}: {e}", slot.root.display());
                } else {
                    watchers.push(w);
                }
            }
            Err(e) => log::info!("indexer: watcher: {e}"),
        }
        slots.push(slot);
    }
    for slot in &slots {
        let (slot, cancel) = (slot.clone(), cancel.clone());
        let _ = std::thread::Builder::new().name("rf-indexer".into()).spawn(move || run(slot, cancel));
    }
    *service().lock() = Some(Service { slots, cancel, _watchers: watchers });
}

/// Stops indexing (the indexes are dropped; saved files stay).
pub fn stop() {
    if let Some(s) = service().lock().take() {
        s.cancel.store(true, Ordering::Relaxed);
        for slot in &s.slots {
            save(slot);
        }
    }
}

fn save(slot: &Slot) {
    if !slot.changed_since_save.swap(false, Ordering::Relaxed) {
        return;
    }
    let mut guard = slot.index.write();
    if let Some(ix) = guard.as_mut() {
        if ix.len() > 0 && ix.len() - ix.live() > ix.len() / 5 {
            ix.compact();
        }
        if let Err(e) = ix.save(&slot.file) {
            log::info!("indexer: saving {}: {e}", slot.file.display());
        }
    }
}

fn run(slot: Arc<Slot>, cancel: Arc<AtomicBool>) {
    // 1. the saved index, searchable at once
    match Index::load(&slot.file) {
        Ok(ix) if ix.root() == slot.root => {
            log::info!("indexer: {} loaded, {} entries", slot.root.display(), ix.live());
            *slot.index.write() = Some(ix);
            slot.state.store(State::Ready as u8, Ordering::Relaxed);
        }
        _ => {}
    }
    // 2. a fresh walk (catches what changed while the app was closed)
    let t = Instant::now();
    let fresh = Index::build(&slot.root, &cancel);
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    log::info!("indexer: {} walked in {:.1} s, {} entries, {} MB", slot.root.display(), t.elapsed().as_secs_f32(), fresh.live(), fresh.memory() >> 20);
    if fresh.is_empty() && slot.index.read().is_none() {
        slot.state.store(State::Failed as u8, Ordering::Relaxed);
    } else {
        *slot.index.write() = Some(fresh);
        slot.state.store(State::Ready as u8, Ordering::Relaxed);
        slot.changed_since_save.store(true, Ordering::Relaxed);
        save(&slot);
    }
    // 3. keep up with changes
    let mut last_save = Instant::now();
    while !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_secs(1));
        let dirs: Vec<PathBuf> = slot.dirty.lock().drain().collect();
        if !dirs.is_empty() {
            if let Some(ix) = slot.index.write().as_mut() {
                ix.refresh_dirs(&dirs, &cancel);
            }
            slot.changed_since_save.store(true, Ordering::Relaxed);
        }
        if last_save.elapsed() > Duration::from_secs(600) {
            save(&slot);
            last_save = Instant::now();
        }
    }
}

/// Searches the index whose root holds `under`: `None` when no ready index covers it (search
/// the disk then).
pub fn search(pattern: &str, under: &Path, include_hidden: bool, max: usize) -> Option<Vec<PathBuf>> {
    let slot = {
        let g = service().lock();
        let s = g.as_ref()?;
        s.slots.iter().find(|s| under.starts_with(&s.root) || under.to_string_lossy().to_lowercase().starts_with(&s.root.to_string_lossy().to_lowercase())).cloned()?
    };
    if slot.state.load(Ordering::Relaxed) != State::Ready as u8 {
        return None;
    }
    let ix = slot.index.read();
    ix.as_ref()?.search(pattern, Some(under), include_hidden, max)
}

/// (root, state, live entries, bytes in memory) per root.
pub fn status() -> Vec<(PathBuf, State, usize, usize)> {
    let g = service().lock();
    let Some(s) = g.as_ref() else { return Vec::new() };
    s.slots
        .iter()
        .map(|slot| {
            let state = match slot.state.load(Ordering::Relaxed) {
                x if x == State::Ready as u8 => State::Ready,
                x if x == State::Failed as u8 => State::Failed,
                _ => State::Building,
            };
            let (live, mem) = slot.index.try_read().and_then(|i| i.as_ref().map(|i| (i.live(), i.memory()))).unwrap_or((0, 0));
            (slot.root.clone(), state, live, mem)
        })
        .collect()
}

/// The drives worth indexing: local, fixed, NTFS or ReFS. Cloud drives (Google Drive, a
/// mounted share) and removable disks stay out: walking them would download files or wake disks.
pub fn local_roots() -> Vec<PathBuf> {
    crate::drives::list_drives()
        .into_iter()
        .filter(|d| d.kind == crate::drives::DriveKind::Fixed && matches!(d.fs.to_ascii_uppercase().as_str(), "NTFS" | "REFS"))
        .map(|d| d.root)
        .collect()
}

pub fn running() -> bool {
    service().lock().is_some()
}
