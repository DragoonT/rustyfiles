//! Shared cache of shell icons and thumbnails. Misses are queued to rf-win's low-priority
//! worker pool; rows render a symbol fallback until the real image arrives.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::*;
use rf_core::Entry;
use rf_win::thumbs::{Bgra, Kind, Request, Response, ThumbPool, folder_has_own_icon, has_unique_icon};

enum Slot {
    Pending,
    Ready(Arc<RenderImage>),
    Failed,
}

/// Result of a freedesktop-pack lookup. `Native` hands the row back to the shell icon path;
/// `Pending` repaints once a worker has decoded the artwork.
#[derive(Clone)]
pub enum PackPicture {
    Ready(Arc<RenderImage>),
    Pending,
    Native,
}

/// Pack and appearance changes invalidate every pending/ready pack slot. Reuse the existing "clear cache"
/// path: rare, and it also drops shell pictures that may have been drawn with the old icon theme.
pub fn clear_for_pack_change(cx: &mut App) {
    let images = crate::look::LookManager::images(cx);
    let _ = images.update(cx, |c, cx| {
        c.clear_all(cx);
    });
}

pub struct ImageCache {
    pool: ThumbPool,
    map: HashMap<u64, Slot>,
    /// Thumbnails (and per-file icons): key → (last use, pixel bytes). Evicted least recently
    /// used first once over `THUMB_BUDGET`; generic icons are few and stay.
    thumb_use: HashMap<u64, (u64, usize)>,
    /// Running sum of `cost()` over `thumb_use`.
    thumb_cost: usize,
    /// How many of the `thumb_use` pictures are made (`Slot::Ready`): pending and failed slots are
    /// tracked and charged, but they are not pictures. What the settings page counts.
    thumb_ready: usize,
    tick: u64,
    /// Evicted images whose GPU atlas tiles must be released (see `release_evicted`).
    evicted: Vec<Arc<RenderImage>>,
    /// Pixel budget for thumbnails: Settings → Advanced → thumbnail cache (64 MB unless the user
    /// chose another). By bytes, not count: a 4K screen of small tiles shows hundreds at once (a
    /// count cap would evict on-screen images every frame), large tiles cost ~300 KB each.
    budget: usize,
    /// What the settings page shows about this cache (see `thumb_cache`).
    stats: Arc<crate::thumb_cache::CacheStats>,
    _task: Task<()>,
    _settings_sub: Subscription,
}

fn cost(bytes: usize) -> usize {
    rf_core::thumbcache::charge(bytes)
}
pub const ICON_PX: u32 = 32;

/// The scale of the screen the main window was last drawn on, in hundredths (100 = 100 %).
static UI_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(100);

/// Told by the main window at each draw: the pictures asked for in the interface's pixels ([`ImageCache::shell_icon_ui`]) are made
/// at this screen's pixels.
pub fn set_ui_scale(scale: f32) {
    let v = ((scale.clamp(0.5, 5.0)) * 100.0).round() as u32;
    UI_SCALE.store(v, std::sync::atomic::Ordering::Relaxed);
}

/// `px` of the interface in the screen's pixels.
pub fn ui_px(px: u32) -> u32 {
    let s = UI_SCALE.load(std::sync::atomic::Ordering::Relaxed);
    ((px * s + 99) / 100).max(px)
}

/// Takes back what `lookup` charged for a picture that was asked for and then dropped before it was made.
fn forget_pending(thumb_use: &mut HashMap<u64, (u64, usize)>, thumb_cost: &mut usize, key: u64) {
    if let Some((_, bytes)) = thumb_use.remove(&key) {
        *thumb_cost -= cost(bytes).min(*thumb_cost);
    }
}

/// The smallest picture size at which a folder shows what is in it (Explorer: from its medium icons up; the Grid's smallest tiles are
/// 64 px and the Cards' 96 px, the lists' rows far less).
const FOLDER_PREVIEW_MIN: u32 = 64;

/// The key of a file's picture. The preview pane's picture (made by the pool's urgent lane) has a key space of its own:
/// were it the list's key, a tile the list had already queued at the same size would answer the pane's request
/// "pending" and the pane would wait behind the list again (no size is shared today; this keeps it so).
fn file_picture_key(pane: bool, dir: &Path, name: &str, modified_ms: &impl Hash, size: impl Hash) -> u64 {
    key_of(&[&(if pane { "pane" } else { "thumb" }), &dir, &name, modified_ms, &size])
}

/// Changes when the icons of customised folders are to be asked for again (`refresh_folder_icons`).
static FOLDER_ICON_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

type FolderCheckKey = (PathBuf, i64, u32, u32);
static FOLDER_CHECK_DONE: OnceLock<Mutex<HashMap<FolderCheckKey, bool>>> = OnceLock::new();
static FOLDER_CHECK_BUSY: OnceLock<Mutex<HashSet<FolderCheckKey>>> = OnceLock::new();
const FOLDER_CHECK_LIMIT: usize = 4096;
/// [`ImageCache::folder_names_icon`]'s answers and the folders being read for it.
static NAMES_ICON_DONE: OnceLock<Mutex<HashMap<(PathBuf, u32), bool>>> = OnceLock::new();
static NAMES_ICON_BUSY: OnceLock<Mutex<HashSet<(PathBuf, u32)>>> = OnceLock::new();

fn remember_folder_check(key: FolderCheckKey, found: bool) {
    let map = FOLDER_CHECK_DONE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = map.lock().unwrap();
    if map.len() >= FOLDER_CHECK_LIMIT {
        map.clear();
    }
    map.insert(key, found);
}

fn mark_folder_check_busy(key: &FolderCheckKey) -> bool {
    let map = FOLDER_CHECK_BUSY.get_or_init(|| Mutex::new(HashSet::new()));
    let mut map = map.lock().unwrap();
    if map.len() >= FOLDER_CHECK_LIMIT {
        map.clear();
    }
    map.insert(key.clone())
}

/// The icons of customised folders are asked for again: a folder's icon was changed in our Properties, or the user refreshed (F5).
///
/// WHY (issue 65: "anything changed does not get reflected in the treeview or fileview"): such an icon is kept under the folder's
/// time and attributes, and changing the icon of a folder that already has one changes neither (the folder is read-only already,
/// and rewriting its desktop.ini does not touch the folder's time), so the old picture stayed.
pub fn refresh_folder_icons() {
    FOLDER_ICON_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// Folders given an icon in our Properties since the start.
static ICON_GIVEN: OnceLock<Mutex<HashSet<PathBuf>>> = OnceLock::new();

/// A folder's icon was set (or taken away) in our Properties: its picture is asked for again everywhere.
///
/// WHY the folder is also written down (issue 349: "changing a folder icon does not always update its icon in the sidebar"): the
/// folder tree draws Windows' picture only for a folder its listing marked as customised (the read-only attribute), and it keeps that
/// listing until the branch is read again. A folder that gets its FIRST icon was listed without the mark, so the tree went on
/// drawing the plain folder; one that already had an icon changed at once. [`folder_icon_given`] is what the tree asks as well.
pub fn folder_icon_changed(folder: &Path) {
    ICON_GIVEN.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap().insert(folder.to_path_buf());
    refresh_folder_icons();
}

/// Whether `folder` got an icon in our Properties since the start (nothing is locked until the first such change).
pub fn folder_icon_given(folder: &Path) -> bool {
    ICON_GIVEN.get().is_some_and(|s| s.lock().unwrap().contains(folder))
}

fn key_of(parts: &[&dyn HashPart]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in parts {
        p.hash_part(&mut h);
    }
    h.finish()
}

trait HashPart {
    fn hash_part(&self, h: &mut std::collections::hash_map::DefaultHasher);
}
impl<T: Hash> HashPart for T {
    fn hash_part(&self, h: &mut std::collections::hash_map::DefaultHasher) {
        self.hash(h)
    }
}

pub(crate) fn to_render_image(b: &Bgra) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(b.width, b.height, b.data.clone())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

impl ImageCache {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let (tx, mut rx) = mpsc::unbounded::<Response>();
        let workers = std::thread::available_parallelism().map(|n| (n.get() / 4).clamp(2, 4)).unwrap_or(2);
        let pool = ThumbPool::new(workers, move |r| {
            let _ = tx.unbounded_send(r);
        });
        let task = cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                let mut batch = vec![first];
                while let Ok(r) = rx.try_recv() {
                    batch.push(r);
                }
                let ok = this
                    .update(cx, |c, cx| {
                        for r in batch {
                            crate::preview_trace::pool_answer(r.urgent, r.dropped, r.image.is_some(), r.waited, r.took);
                            if r.dropped {
                                c.map.remove(&r.key);
                                // (and what was charged for it: a dropped pane picture is not asked for again once the
                                // user has moved on, so nothing else would ever take its entry away)
                                forget_pending(&mut c.thumb_use, &mut c.thumb_cost, r.key);
                                continue;
                            }
                            // evicted while it was being made: nobody wants it any more
                            if !matches!(c.map.get(&r.key), Some(Slot::Pending)) {
                                continue;
                            }
                            let slot = match r.image.as_deref().and_then(to_render_image) {
                                Some(img) => {
                                    if let Some(u) = c.thumb_use.get_mut(&r.key) {
                                        let bytes = r.image.as_ref().map_or(0, |b| b.data.len());
                                        c.thumb_cost = c.thumb_cost - cost(u.1) + cost(bytes);
                                        u.1 = bytes;
                                        c.thumb_ready += 1;
                                    }
                                    Slot::Ready(img)
                                }
                                None => Slot::Failed,
                            };
                            c.map.insert(r.key, slot);
                        }
                        c.evict_over_budget();
                        c.release_evicted(cx);
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        });
        let stats = Arc::new(crate::thumb_cache::CacheStats::default());
        let weak = cx.weak_entity();
        crate::thumb_cache::register(weak, stats.clone(), cx);
        // a changed limit applies at once (a lower one frees pictures now, not at the next miss)
        let sub = cx.observe_global::<crate::settings::Settings>(|this, cx| {
            let budget = crate::thumb_cache::budget(cx);
            if budget != this.budget {
                this.budget = budget;
                this.evict_over_budget();
                this.release_evicted(cx);
                cx.notify();
            }
        });
        Self {
            pool,
            map: HashMap::new(),
            thumb_use: HashMap::new(),
            thumb_cost: 0,
            thumb_ready: 0,
            tick: 0,
            evicted: Vec::new(),
            budget: crate::thumb_cache::budget(cx),
            stats,
            _task: task,
            _settings_sub: sub,
        }
    }

    /// Tells the settings page how much this cache holds (two relaxed stores).
    fn publish(&self) {
        self.stats.set(self.thumb_cost, self.thumb_ready);
    }

    /// "Önbelleği temizle": drops every picture held (thumbnails, per-file and generic icons), so the
    /// next draw asks for them again. Pictures still being made stay pending. Returns (pictures
    /// dropped, pixel bytes of the tracked ones). Memory only: no file is touched, and Windows' own
    /// thumbnail cache is not asked about.
    pub fn clear_all(&mut self, cx: &mut Context<Self>) -> (usize, usize) {
        let mut images = 0usize;
        let mut dropped_keys = Vec::new();
        let evicted = &mut self.evicted;
        self.map.retain(|k, slot| match slot {
            Slot::Pending => true,
            Slot::Ready(img) => {
                images += 1;
                evicted.push(img.clone());
                dropped_keys.push(*k);
                false
            }
            Slot::Failed => {
                dropped_keys.push(*k);
                false
            }
        });
        let mut bytes = 0usize;
        for k in dropped_keys {
            if let Some((_, b)) = self.thumb_use.remove(&k) {
                bytes += b;
                self.thumb_cost -= cost(b).min(self.thumb_cost);
            }
        }
        // (every made picture is gone; what is still tracked is pending)
        self.thumb_ready = 0;
        self.publish();
        self.release_evicted(cx);
        cx.notify();
        (images, bytes)
    }

    fn lookup(&mut self, key: u64, make: impl FnOnce() -> Request, is_thumb: bool) -> Option<Arc<RenderImage>> {
        if is_thumb {
            // every use refreshes recency, so on-screen images are the last to go
            self.tick += 1;
            let tick = self.tick;
            match self.thumb_use.get_mut(&key) {
                Some(u) => u.0 = tick,
                None => {
                    self.thumb_use.insert(key, (tick, 0));
                    self.thumb_cost += cost(0);
                }
            }
        }
        match self.map.get(&key) {
            Some(Slot::Ready(img)) => Some(img.clone()),
            Some(Slot::Pending) | Some(Slot::Failed) => None,
            None => {
                self.map.insert(key, Slot::Pending);
                self.pool.request(make());
                None
            }
        }
    }

    /// Drops least recently used thumbnails until under budget (cheap check; the sort only
    /// runs when over budget, and then frees a batch down to 90 % to avoid doing it per frame).
    fn evict_over_budget(&mut self) {
        if self.thumb_cost > self.budget {
            // (which ones go is `rf_core::thumbcache::eviction_victims`, tested on its own)
            let items: Vec<(u64, u64, usize)> = self.thumb_use.iter().map(|(k, (t, b))| (*k, *t, *b)).collect();
            for key in rf_core::thumbcache::eviction_victims(&items, self.thumb_cost, self.budget) {
                if let Some((_, bytes)) = self.thumb_use.remove(&key) {
                    self.thumb_cost -= cost(bytes).min(self.thumb_cost);
                }
                if let Some(Slot::Ready(img)) = self.map.remove(&key) {
                    self.evicted.push(img);
                    self.thumb_ready = self.thumb_ready.saturating_sub(1);
                }
            }
        }
        self.publish();
    }

    /// Dropping the `Arc` is not enough: GPUI keeps an uploaded image in each window's sprite
    /// atlas until `drop_image`. Do it after the current window update (a window being updated
    /// is temporarily out of the app's window list, so an immediate call would miss it).
    fn release_evicted(&mut self, cx: &mut Context<Self>) {
        if self.evicted.is_empty() {
            return;
        }
        let imgs = std::mem::take(&mut self.evicted);
        cx.defer(move |cx| {
            for img in imgs {
                cx.drop_image(img, None);
            }
        });
    }

    /// Small icon for a row (`ICON_PX`). Generic types are cached per extension, plain folders once;
    /// a folder that can have an icon of its own (`folder_has_own_icon`) is asked per folder.
    pub fn icon(&mut self, dir: &Path, e: &Entry, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.icon_sized(dir, e, ICON_PX, cx)
    }

    /// Icon at an explicit pixel size (e.g. Grid/Cards/Desktop tiles when content thumbnails are off).
    pub fn icon_sized(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        // A configured theme folder outranks a pack, as it does in `thumbnail_as`.
        if let Some(f) = crate::themes::icon_file(e.is_dir(), &e.ext, shell_own_picture(e)) {
            return self.theme_icon(f, size, cx);
        }
        match self.themed_picture_async(dir, e, false, size, cx) {
            PackPicture::Ready(icon) => return Some(icon),
            // A pack owns this row: wait for the worker instead of showing a shell picture first.
            PackPicture::Pending => return None,
            PackPicture::Native => {}
        }
        self.native_icon(dir, e, size, cx)
    }

    /// Shell (or theme-folder) icon, without any freedesktop-pack work.
    fn native_icon(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(f) = crate::themes::icon_file(e.is_dir(), &e.ext, shell_own_picture(e)) {
            return self.theme_icon(f, size, cx);
        }
        let unique = shell_own_picture(e);
        let key = if unique && e.is_dir() {
            // (with the folder's time and attributes: an icon chosen later changes one of them, and the old picture is not kept)
            let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
            key_of(&[&"dirpath", &dir, &&*e.name, &e.modified_ms, &e.attrs, &size, &generation])
        } else if e.is_dir() {
            key_of(&[&"dir", &size])
        } else if unique {
            key_of(&[&"path", &dir, &&*e.name, &size])
        } else {
            key_of(&[&"ext", &&*e.ext, &size])
        };
        let path = dir.join(&*e.name);
        // Per-file icons (exe/lnk/…) are unbounded in number: keep them in the LRU too.
        let r = self.lookup(key, || Request::new(key, path, size, Kind::Icon), unique);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    /// Resolve and decode a pack picture off the UI thread. `key` must identify the exact request
    /// (pack, icon names/size), so a pending row waits for its own worker and repaints when it lands.
    pub fn pack_picture(
        &mut self,
        key: u64,
        work: impl FnOnce() -> PackPicture + Send + 'static,
        cx: &mut Context<Self>,
    ) -> PackPicture {
        match self.map.get(&key) {
            Some(Slot::Ready(im)) => return PackPicture::Ready(im.clone()),
            Some(Slot::Pending) => return PackPicture::Pending,
            Some(Slot::Failed) => return PackPicture::Native,
            None => {}
        }
        self.map.insert(key, Slot::Pending);
        let task = cx.background_spawn(async move { work() });
        cx.spawn(async move |this, cx| {
            let outcome = task.await;
            let _ = this.update(cx, |c, cx| {
                c.map.insert(
                    key,
                    match outcome {
                        PackPicture::Ready(im) => Slot::Ready(im),
                        PackPicture::Native | PackPicture::Pending => Slot::Failed,
                    },
                );
                cx.notify();
            });
        })
        .detach();
        PackPicture::Pending
    }

    /// Whether the folder `path`'s `desktop.ini` names an icon (`IconResource` / `IconFile`: Downloads, Documents, a folder given a
    /// picture under Properties > Customize). Read on a worker: `false` until it is known, and the cache notifies when it lands.
    /// The folder tree draws Windows' picture only for such a folder (issue 225: `Program Files` is read-only, the mark of a
    /// customised folder, but has no picture of its own, and stood in the tree with Windows 10's old folder among ours).
    pub fn folder_names_icon(&mut self, path: &Path, cx: &mut Context<Self>) -> bool {
        let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
        let key = (path.to_path_buf(), generation);
        let done = NAMES_ICON_DONE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(found) = done.lock().unwrap().get(&key) {
            return *found;
        }
        let busy = NAMES_ICON_BUSY.get_or_init(|| Mutex::new(HashSet::new()));
        if busy.lock().unwrap().insert(key.clone()) {
            let folder = key.0.clone();
            let task = cx.background_spawn(async move { rf_win::folder_custom::folder_icon_info(&folder).is_some() });
            cx.spawn(async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |_, cx| {
                    let mut done = NAMES_ICON_DONE.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap();
                    if done.len() >= FOLDER_CHECK_LIMIT {
                        done.clear();
                    }
                    done.insert(key.clone(), found);
                    drop(done);
                    NAMES_ICON_BUSY.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap().remove(&key);
                    cx.notify();
                });
            })
            .detach();
        }
        false
    }

    /// `desktop.ini` is only read on a worker. Until it lands, prefer the pack: a real custom icon flips the row back
    /// on the redraw. The generation is part of the key, so F5/Properties re-checks.
    fn folder_check_async(&mut self, path: PathBuf, modified_ms: i64, attrs: u32, cx: &mut Context<Self>) -> bool {
        if !folder_has_own_icon(attrs) {
            return false;
        }
        let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
        let key: FolderCheckKey = (path.clone(), modified_ms, attrs, generation);
        let done = FOLDER_CHECK_DONE.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(found) = done.lock().unwrap().get(&key) {
            return *found;
        }
        if mark_folder_check_busy(&key) {
            let task = cx.background_spawn(async move { folder_path_keeps_shell_icon(&path, modified_ms, attrs) });
            let busy_key = key.clone();
            cx.spawn(async move |this, cx| {
                let found = task.await;
                let _ = this.update(cx, |_, cx| {
                    remember_folder_check(key, found);
                    FOLDER_CHECK_BUSY.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap().remove(&busy_key);
                    cx.notify();
                });
            })
            .detach();
        }
        false
    }

    /// Queue one pack lookup. Names and pack are moved to the worker; only a hashable key and an atomic
    /// appearance flag are built here.
    pub fn pack_picture_for_names(&mut self, pack: &str, names: &[&'static str], px: u32, cx: &mut Context<Self>) -> PackPicture {
        if names.is_empty() {
            return PackPicture::Native;
        }
        let dark = crate::settings::effective_dark();
        let joined = names.join("\0");
        let key = key_of(&[&"pack", &pack, &dark, &joined, &px]);
        let pack = pack.to_string();
        let names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
        self.pack_picture(key, move || {
            let names: Vec<&str> = names.iter().map(String::as_str).collect();
            match crate::icon_theme::image_for_names(&pack, &names, px) {
                Some(icon) => PackPicture::Ready(icon),
                None => PackPicture::Native,
            }
        }, cx)
    }

    /// Pack artwork for an entry, or `Native` when the shell owns it. Nothing reads a pack file or `desktop.ini` here.
    fn themed_picture_async(&mut self, dir: &Path, e: &Entry, skip_thumbnail_kinds: bool, px: u32, cx: &mut Context<Self>) -> PackPicture {
        let pack = crate::settings::icon_theme_pack();
        if !matches!(crate::settings::icon_theme(), crate::settings::IconThemePref::Freedesktop) || pack.is_empty() {
            return PackPicture::Native;
        }
        if skip_thumbnail_kinds && crate::icons::has_sure_thumbnail(e) {
            return PackPicture::Native;
        }
        let path = dir.join(&*e.name);
        let keeps_shell = if e.is_dir() {
            self.folder_check_async(path.clone(), e.modified_ms, e.attrs, cx)
        } else {
            has_unique_icon(&e.ext)
        };
        if keeps_shell {
            return PackPicture::Native;
        }
        let names: Vec<&'static str> = if e.is_dir() {
            let mut names = crate::icon_theme::special_folder_names(&path).to_vec();
            names.extend_from_slice(crate::icon_theme::FOLDER_NAMES);
            names
        } else {
            crate::icon_theme::image_names(crate::icons::category(e), &e.ext)
        };
        self.pack_picture_for_names(&pack, &names, px, cx)
    }

    /// Same lookup for callers that hold a path, not an `Entry` (Home, the sidebar, the tab strip).
    /// On `Native` they get the shell's own picture instead of a Lucide fallback.
    /// `lucide_fallback` is for Home: without a matching picture the caller draws its own glyph,
    /// except for folders with a real custom icon and for `.exe`/`.lnk`, which keep the shell picture.
    pub fn themed_or_native_picture(
        &mut self,
        dir: &Path,
        e: &Entry,
        names: &[&'static str],
        px: u32,
        lucide_fallback: bool,
        cx: &mut Context<Self>,
    ) -> PackPicture {
        let pack = crate::settings::icon_theme_pack();
        let freedesktop = matches!(crate::settings::icon_theme(), crate::settings::IconThemePref::Freedesktop);
        if lucide_fallback && !freedesktop {
            return PackPicture::Native;
        }
        if names.is_empty() || !freedesktop || pack.is_empty() {
            return self.native_icon(dir, e, px, cx).map_or(PackPicture::Native, PackPicture::Ready);
        }
        let path = dir.join(&*e.name);
        let keeps_shell = if e.is_dir() {
            self.folder_check_async(path, e.modified_ms, e.attrs, cx)
        } else {
            has_unique_icon(&e.ext)
        };
        if keeps_shell {
            return self.native_icon(dir, e, px, cx).map_or(PackPicture::Native, PackPicture::Ready);
        }
        match self.pack_picture_for_names(&pack, names, px, cx) {
            PackPicture::Native if lucide_fallback => PackPicture::Native,
            PackPicture::Native => self.native_icon(dir, e, px, cx).map_or(PackPicture::Native, PackPicture::Ready),
            other => other,
        }
    }

    fn theme_icon(&mut self, file: std::path::PathBuf, px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = key_of(&[&"theme", &file, &px]);
        match self.map.get(&key) {
            Some(Slot::Ready(im)) => return Some(im.clone()),
            Some(_) => return None,
            None => {}
        }
        self.map.insert(key, Slot::Pending);
        let work = cx.background_spawn(async move {
            match rf_win::look::decode::decode_full(&file, px) {
                Ok(d) => d.frames.into_iter().next().map(|(b, _)| b),
                Err(e) => {
                    log::warn!("theme icon {file:?}: {e:#}");
                    None
                }
            }
        });
        cx.spawn(async move |this, cx| {
            let b = work.await;
            let _ = this.update(cx, |c, cx| {
                c.map.insert(key, b.as_ref().and_then(to_render_image).map_or(Slot::Failed, Slot::Ready));
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// Another program's overlay picture `index` (`rf_win::overlays`) at `px`. Few and small (one per kind of mark): kept for good.
    pub fn overlay(&mut self, index: i32, px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = key_of(&[&"overlay", &index, &px]);
        match self.map.get(&key) {
            Some(Slot::Ready(im)) => return Some(im.clone()),
            Some(_) => return None,
            None => {}
        }
        self.map.insert(key, Slot::Pending);
        let work = cx.background_spawn(async move { rf_win::overlays::picture(index, px) });
        cx.spawn(async move |this, cx| {
            let b = work.await;
            let _ = this.update(cx, |c, cx| {
                c.map.insert(key, b.as_ref().and_then(to_render_image).map_or(Slot::Failed, Slot::Ready));
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// Large thumbnail (falls back to the large icon when the file has no thumbnail).
    /// The shell's picture for a parsing name (an app of "All apps": `shell:AppsFolder\<id>`).
    /// Asked as a thumbnail: inside rustyfiles the icon of a Store app comes back as the shell's
    /// blank app placeholder, its thumbnail is the app's own logo.
    pub fn shell_icon(&mut self, parsing: &str, px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        // (asked again after F5 or an icon change in Properties: in the Explorer look a pinned folder that was given a new icon kept
        // the old picture until the app was started again, issue 349)
        let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
        let key = key_of(&[&"shell", &parsing, &px, &generation]);
        let path = std::path::PathBuf::from(parsing);
        let mut r = self.lookup(key, || Request::new(key, path, px, Kind::Thumbnail), true);
        if r.is_none() && generation > 0 {
            // the picture of before stays while the new one is made: the row does not blink
            let before = key_of(&[&"shell", &parsing, &px, &(generation - 1)]);
            if let Some(Slot::Ready(img)) = self.map.get(&before) {
                r = Some(img.clone());
            }
        }
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    /// [`shell_icon`](Self::shell_icon) for a picture drawn `px` big in the interface's pixels (a drive in the sidebar, a card on the
    /// Home page, a menu's row): made at the screen's pixels. Asked at `px` itself, a 16 px drive icon was stretched to 28 of the
    /// screen's pixels at 175 % and came out blurred (issue 253).
    pub fn shell_icon_ui(&mut self, parsing: &str, px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.shell_icon(parsing, ui_px(px), cx)
    }

    /// [`shell_icon_state`](Self::shell_icon_state) for a picture drawn `px` big in the interface's pixels (see [`Self::shell_icon_ui`]).
    pub fn shell_icon_state_ui(&mut self, parsing: &str, px: u32, state: u8, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.shell_icon_state(parsing, ui_px(px), state, cx)
    }

    /// [`shell_icon`](Self::shell_icon) for a place whose picture depends on a state the shell keeps (the Recycle Bin: full or empty).
    /// The state is part of the key, so a picture made for one state is never shown for the other and a change of state asks the
    /// shell again; `shell_icon` itself keeps one picture for ever.
    pub fn shell_icon_state(&mut self, parsing: &str, px: u32, state: u8, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = key_of(&[&"shell-state", &parsing, &px, &state]);
        let path = std::path::PathBuf::from(parsing);
        let r = self.lookup(key, || Request::new(key, path, px, Kind::Thumbnail), true);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    pub fn thumbnail(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.thumbnail_as(dir, e, size, false, None, cx)
    }

    /// `thumbnail` for an asker that knows how many of the screen's pixels its box has (`box_px`): a picture Windows hands over
    /// bigger than that is brought to it on the worker, and the box then draws it pixel for pixel. A folder's preview always
    /// comes at 256 px, and squeezed into a tile by the graphics card it was soft and jagged (issue 253). The grid's tiles ask
    /// this way. `size` is what Windows is asked for, as in `thumbnail`.
    pub fn thumbnail_fit(&mut self, dir: &Path, e: &Entry, size: u32, box_px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.thumbnail_as(dir, e, size, false, Some(box_px), cx)
    }

    /// The picture the user is looking at right now (the preview pane's, Quick Look's): made by the pool's urgent lane,
    /// at normal priority and not behind the list's own tiles. The list's tiles use `thumbnail`.
    pub fn pane_picture(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.thumbnail_as(dir, e, size, true, None, cx)
    }

    fn thumbnail_as(&mut self, dir: &Path, e: &Entry, size: u32, urgent: bool, fit: Option<u32>, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        // (a picture made to fit a box is another picture than the one left as it came: its own place in the cache)
        let key_size = (size, fit);
        // A type outside our own list that a thumbnail provider is registered for (a sound file, a font, what an installed extension
        // draws: issue 307): its own picture when it has one. Until that is known, and when it has none, a tile shows what it would
        // for any other file (the theme's, the pack's or the type's icon, below); the pane, which asks urgently, shows its own.
        // (not on an FTP server: every file would be downloaded to look for a picture in it)
        if crate::icons::may_have_thumbnail(e) && !rf_win::ftp::is_ftp(dir) {
            let key = file_picture_key(urgent, dir, &e.name, &e.modified_ms, key_size);
            let path = dir.join(&*e.name);
            let own = self.lookup(
                key,
                || {
                    let mut req = Request::new(key, path, size, Kind::ThumbnailOnly);
                    req.urgent = urgent;
                    req.fit = fit;
                    req
                },
                true,
            );
            if own.is_some() || urgent {
                self.evict_over_budget();
                self.release_evicted(cx);
                return own;
            }
        }
        // The theme folder's picture for what has no content picture (a folder too: the theme's folder wins over its preview).
        if !crate::icons::has_sure_thumbnail(e)
            && let Some(f) = crate::themes::icon_file(e.is_dir(), &e.ext, shell_own_picture(e))
        {
            return self.theme_icon(f, size, cx);
        }
        // When a freedesktop pack is active, folders and non-thumbnail files take the pack's artwork at `size`
        // unless the user gave a folder its own custom `desktop.ini` icon; real content thumbnails still win.
        match self.themed_picture_async(dir, e, true, size, cx) {
            PackPicture::Ready(icon) => return Some(icon),
            PackPicture::Pending => return None,
            PackPicture::Native => {}
        }
        // In native mode (or when the pack has no folder icon), a plain folder at >= 64px shows Explorer's folder preview.
        let own = shell_own_picture(e);
        let folder_preview = e.is_dir()
            && size >= FOLDER_PREVIEW_MIN
            && !own
            && !rf_win::ftp::is_ftp(dir)
            && !crate::nav_touch::may_hold_archive(dir);
        let wants_thumb = crate::icons::has_sure_thumbnail(e) || folder_preview;
        let per_file = wants_thumb || own;
        let key = if per_file && e.is_dir() && !wants_thumb {
            let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
            key_of(&[&"dirpathL", &dir, &&*e.name, &e.modified_ms, &e.attrs, &key_size, &generation])
        } else if per_file {
            file_picture_key(urgent && wants_thumb, dir, &e.name, &e.modified_ms, key_size)
        } else if e.is_dir() {
            key_of(&[&"dirL", &key_size])
        } else {
            key_of(&[&"extL", &&*e.ext, &key_size])
        };
        let path = dir.join(&*e.name);
        let kind = if wants_thumb { Kind::Thumbnail } else { Kind::Icon };
        // every per-file picture (an exe's or a shortcut's icon too) is in the LRU, or browsing
        // folders of programs would keep them all until the app closes
        let r = self.lookup(
            key,
            || {
                let mut req = Request::new(key, path, size, kind);
                req.urgent = urgent && wants_thumb;
                req.fit = fit;
                req
            },
            per_file,
        );
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    pub fn cancel_pending(&mut self) {
        self.pool.clear();
        let map = &mut self.map;
        map.retain(|_, s| !matches!(s, Slot::Pending));
        let mut freed = 0;
        self.thumb_use.retain(|k, (_, b)| {
            let keep = map.contains_key(k);
            if !keep {
                freed += cost(*b);
            }
            keep
        });
        self.thumb_cost -= freed;
        self.publish();
    }
}

/// Native shell cache policy: a program, a shortcut and the like, or any folder Windows marks as
/// potentially customised (`READONLY | SYSTEM`) gets a per-path cache key so `Music`, `Videos` and
/// custom folders never share or poison the plain folder slot (`"dir"` / `"dirL"`).
fn shell_own_picture(e: &Entry) -> bool {
    if e.is_dir() { folder_has_own_icon(e.attrs) } else { has_unique_icon(&e.ext) }
}

/// Default Windows folder/known-folder icon indices in `shell32.dll` and `imageres.dll`. Any other
/// index picked by the user via Properties → Customize → Change Icon (`shell32.dll,127`, `shell32.dll,15`)
/// is a real user customisation and beats the icon pack.
fn is_windows_default_folder_resource(dll_lower: &str, index: i32) -> bool {
    match dll_lower {
        "shell32.dll" => matches!(index, 3 | 4 | -4 | -5 | -173 | -235 | -236 | -237 | -238),
        "imageres.dll" => matches!(
            index,
            -3 | -4 | -5 | -18 | -108 | -112 | -113 | -115 | -117 | -123 | -181 | -183 | -184 | -185 | -186 | -189 | -198
        ),
        "windows.storage.dll" => true,
        _ => false,
    }
}

/// Whether `info` from `desktop.ini` represents a real, present user-chosen folder icon rather than
/// a built-in Windows folder type (`Music`, `Pictures`, ...) or OneDrive's automatic sync stamp.
fn folder_icon_is_real(info: &rf_win::folder_custom::FolderIcon, folder: &Path) -> bool {
    let expanded = rf_win::folder_custom::expand_env(info.file.trim());
    if expanded.is_empty() {
        return false;
    }
    let raw = PathBuf::from(&expanded);
    let file_name = raw.file_name().and_then(|n| n.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    if file_name == "onedrive.exe" {
        return false;
    }
    let is_sys_dll = matches!(file_name.as_str(), "imageres.dll" | "shell32.dll" | "windows.storage.dll");
    if is_sys_dll && (info.localized || is_windows_default_folder_resource(&file_name, info.index)) {
        return false;
    }
    let resolved = if raw.is_relative() {
        let local = folder.join(&raw);
        if local.exists() {
            local
        } else if !expanded.contains(['\\', '/']) {
            std::env::var("SystemRoot").map(|r| PathBuf::from(r).join("System32").join(&raw)).unwrap_or(local)
        } else {
            local
        }
    } else {
        raw
    };
    resolved.exists()
}

/// The mark alone is not enough: `C:\Users` and `Program Files (x86)` are READONLY but have no custom picture.
/// `desktop.ini` is parsed once per folder state; the refresh generation (F5/Properties) invalidates it.
#[cfg(test)]
pub(crate) fn folder_keeps_shell_icon(dir: &Path, e: &Entry) -> bool {
    if !folder_has_own_icon(e.attrs) {
        return false;
    }
    folder_path_keeps_shell_icon(&dir.join(&*e.name), e.modified_ms, e.attrs)
}

pub(crate) fn folder_path_keeps_shell_icon(path: &Path, modified_ms: i64, attrs: u32) -> bool {
    if !folder_has_own_icon(attrs) {
        return false;
    }
    let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
    let key = (path.to_path_buf(), modified_ms, attrs, generation);
    let done = FOLDER_CHECK_DONE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(hit) = done.lock().unwrap().get(&key) {
        return *hit;
    }
    let found = rf_win::folder_custom::folder_icon_info(path).is_some_and(|info| folder_icon_is_real(&info, path));
    remember_folder_check(key, found);
    found
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the toolkit's glob import brings a `test` attribute of its own that shadows the standard one)
    use super::{cost, file_picture_key, folder_icon_is_real, folder_keeps_shell_icon, forget_pending, refresh_folder_icons, shell_own_picture};
    use crate::settings::IconThemePref;

    /// Native mode keeps per-folder keys for any READONLY/SYSTEM folder so `Music`, `Videos` and custom folders
    /// never share or poison the plain folder slot (`"dir"` / `"dirL"`).
    #[test]
    fn a_customised_folder_and_a_program_have_pictures_of_their_own() {
        use rf_core::{Entry, EntryKind};
        let dir = |attrs: u32| Entry::new("klasör", EntryKind::Dir, 0, 0, 0, 0x10 | attrs);
        assert!(!shell_own_picture(&dir(0)), "a plain folder");
        assert!(!shell_own_picture(&dir(0x2)), "a hidden folder is not a customised one");
        assert!(shell_own_picture(&dir(0x1)), "read-only: how Explorer marks a folder with desktop.ini");
        assert!(shell_own_picture(&dir(0x4)), "a system folder (Downloads, Documents)");
        assert!(shell_own_picture(&Entry::new("a.exe", EntryKind::File, 1, 0, 0, 0x20)));
        assert!(!shell_own_picture(&Entry::new("a.txt", EntryKind::File, 1, 0, 0, 0x21)), "a read-only document is still its kind's picture");
    }
    use std::collections::HashMap;
    use std::path::Path;

    /// A picture asked for and dropped leaves nothing charged behind; one that holds pixels gives back what it cost.
    #[test]
    fn a_dropped_picture_takes_back_its_charge() {
        let mut thumb_use: HashMap<u64, (u64, usize)> = HashMap::new();
        let mut thumb_cost = 0usize;
        // what `lookup` does for a new picture: an entry with no pixels yet, charged the nominal cost
        thumb_use.insert(7, (1, 0));
        thumb_cost += cost(0);
        thumb_use.insert(8, (2, 4096));
        thumb_cost += cost(4096);
        assert!(thumb_cost > 0);
        forget_pending(&mut thumb_use, &mut thumb_cost, 7);
        assert!(!thumb_use.contains_key(&7));
        assert_eq!(thumb_cost, cost(4096), "only the other picture is still charged");
        // a key that is not tracked changes nothing
        forget_pending(&mut thumb_use, &mut thumb_cost, 99);
        assert_eq!(thumb_cost, cost(4096));
        forget_pending(&mut thumb_use, &mut thumb_cost, 8);
        assert_eq!((thumb_use.len(), thumb_cost), (0, 0));
    }

    /// The pane's picture of a file never has the key of the list's picture of it, whatever the size; the same
    /// question always gets the same key.
    #[test]
    fn the_pane_has_keys_of_its_own() {
        let dir = Path::new(r"C:\pics");
        let list = file_picture_key(false, dir, "a.jpg", &5u64, 384);
        let pane = file_picture_key(true, dir, "a.jpg", &5u64, 384);
        assert_ne!(list, pane);
        assert_eq!(pane, file_picture_key(true, dir, "a.jpg", &5u64, 384));
        assert_ne!(pane, file_picture_key(true, dir, "a.jpg", &6u64, 384), "a changed file is a new picture");
        assert_ne!(pane, file_picture_key(true, dir, "a.jpg", &5u64, 1280), "another size is another picture");
    }
    /// An old `icon_theme = "xp"` setting is inert: the shell still owns every picture.
    #[test]
    fn a_legacy_xp_setting_keeps_shell_icons() {
        use rf_core::{Entry, EntryKind};
        let doc = Entry::new("a.txt", EntryKind::File, 1, 0, 0, 0x21);
        let pic = Entry::new("a.jpg", EntryKind::File, 1, 0, 0, 0x21);
        let legacy = IconThemePref::Legacy;
        let dir = Path::new(r"C:\");
        assert!(crate::icon_theme::image(legacy, "", crate::icons::category(&doc), &doc.ext, 16).is_none());
        assert!(crate::icon_theme::image(legacy, "", crate::icons::category(&pic), &pic.ext, 16).is_none());
    }

    /// READONLY alone means "localized name" on `C:\Users`; only a user-chosen icon must keep the shell picture.
    #[test]
    fn only_a_real_desktop_ini_icon_beats_the_pack() {
        use rf_core::{Entry, EntryKind};
        let root = std::env::temp_dir().join(format!("rf-icon-ini-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Eigene");
        std::fs::create_dir_all(&folder).unwrap();
        let icon = folder.parent().unwrap().join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        let e = || Entry::new("Eigene", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        assert!(!folder_keeps_shell_icon(&root, &e()), "READONLY with no desktop.ini");
        std::fs::write(folder.join("desktop.ini"), "[.ShellClassInfo]\nLocalizedResourceName=@shell32.dll,-42\n").unwrap();
        refresh_folder_icons();
        assert!(!folder_keeps_shell_icon(&root, &e()), "localization is not an icon");
        std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={},0\n", icon.display())).unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()), "a user-chosen icon is");
    }

    /// Built-in Windows folder type icons and OneDrive sync markers yield to the pack, while a user-chosen
    /// `shell32.dll` icon (Properties → Customize → Change Icon) or `.ico` file beats the pack.
    #[test]
    fn windows_type_and_onedrive_icons_yield_to_pack_while_user_shell32_icons_win() {
        use rf_core::{Entry, EntryKind};
        let root = std::env::temp_dir().join(format!("rf-icon-type-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Pictures");
        std::fs::create_dir_all(&folder).unwrap();
        let icon = root.join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        let e = || Entry::new("Pictures", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        for sys_icon in ["%SystemRoot%\\system32\\imageres.dll,-112", "C:\\Windows\\System32\\shell32.dll,4", "imageres.dll,-113"] {
            std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={sys_icon}\n")).unwrap();
            refresh_folder_icons();
            assert!(!folder_keeps_shell_icon(&root, &e()), "{sys_icon} is a Windows type icon");
        }
        std::fs::write(
            folder.join("desktop.ini"),
            "[.ShellClassInfo]\nLocalizedResourceName=@%SystemRoot%\\system32\\shell32.dll,-21790\nIconResource=%SystemRoot%\\system32\\imageres.dll,-108\n",
        )
        .unwrap();
        refresh_folder_icons();
        assert!(!folder_keeps_shell_icon(&root, &e()), "localized Music folder yields to the pack");
        std::fs::write(folder.join("desktop.ini"), "[.ShellClassInfo]\nIconResource=C:\\Program Files\\Microsoft OneDrive\\OneDrive.exe,7\n").unwrap();
        refresh_folder_icons();
        assert!(!folder_keeps_shell_icon(&root, &e()), "OneDrive sync stamp yields to the pack");
        // User-chosen shell32.dll icon (no LocalizedResourceName, non-default index like 127 or 15) must beat the pack.
        std::fs::write(folder.join("desktop.ini"), "[.ShellClassInfo]\nIconResource=%SystemRoot%\\System32\\shell32.dll,127\n").unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()), "a user-chosen shell32.dll icon beats the pack");
        std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={},0\n", icon.display())).unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()), "a user-chosen .ico file beats the pack");
        std::fs::remove_dir_all(root).ok();
    }

    /// `IconResource=my.ico` is relative to the folder, not to the process; the pack must not replace it.
    #[test]
    fn a_relative_desktop_ini_icon_is_real() {
        use rf_core::{Entry, EntryKind};
        let root = std::env::temp_dir().join(format!("rf-icon-relative-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Eigene");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("my.ico"), b"ico").unwrap();
        let e = || Entry::new("Eigene", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        for icon in ["my.ico", ".\\my.ico"] {
            std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={icon},0\n")).unwrap();
            refresh_folder_icons();
            assert!(folder_keeps_shell_icon(&root, &e()), "{icon} is a user-chosen icon");
        }
        std::fs::remove_dir_all(root).ok();
    }

    /// `%TEMP%\\my.ico` must resolve like Explorer does, not be mistaken for a Windows type icon.
    #[test]
    fn an_env_variable_in_a_desktop_ini_icon_is_expanded() {
        use rf_core::{Entry, EntryKind};
        let Ok(temp) = std::env::var("TEMP") else { return };
        let root = std::env::temp_dir().join(format!("rf-icon-env-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Eigene");
        std::fs::create_dir_all(&folder).unwrap();
        let icon = std::path::Path::new(&temp).join(format!("rf-icon-env-{}.ico", std::process::id()));
        std::fs::write(&icon, b"ico").unwrap();
        let e = || Entry::new("Eigene", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource=%TEMP%\\{}\n", icon.file_name().unwrap().to_string_lossy())).unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()), "an existing %TEMP% icon is user-chosen");
        std::fs::remove_file(&icon).ok();
        std::fs::remove_dir_all(root).ok();
    }

    /// A user icon under a Windows installation on another drive is still a user icon, not a type icon.
    #[test]
    fn a_windows_path_on_another_drive_is_not_blanket_rejected() {
        use rf_win::folder_custom::FolderIcon;
        let root = std::env::temp_dir().join(format!("rf-icon-drive-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let icon = root.join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        assert!(folder_icon_is_real(&FolderIcon { file: icon.to_string_lossy().into_owned(), index: 0, localized: false }, &root));
        assert!(!folder_icon_is_real(&FolderIcon { file: "C:\\Windows\\System32\\imageres.dll".into(), index: -112, localized: false }, &root));
        std::fs::remove_dir_all(root).ok();
    }

    /// F5/Properties must reread the ini: removing an icon frees the folder for the pack.
    #[test]
    fn refreshing_rereads_a_removed_icon() {
        use rf_core::{Entry, EntryKind};
        let root = std::env::temp_dir().join(format!("rf-icon-refresh-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Eigene");
        std::fs::create_dir_all(&folder).unwrap();
        let icon = root.join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        let e = || Entry::new("Eigene", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={},0\n", icon.display())).unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()));
        std::fs::write(folder.join("desktop.ini"), "[.ShellClassInfo]\nLocalizedResourceName=@shell32.dll,-42\n").unwrap();
        refresh_folder_icons();
        assert!(!folder_keeps_shell_icon(&root, &e()), "F5/Properties rereads the ini");
        std::fs::remove_dir_all(root).ok();
    }
}
