//! Shared cache of shell icons and thumbnails. Misses are queued to rf-win's low-priority
//! worker pool; rows render a symbol fallback until the real image arrives.

use std::collections::HashMap;
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
fn file_picture_key(pane: bool, dir: &Path, name: &str, modified_ms: &impl Hash, size: u32) -> u64 {
    key_of(&[&(if pane { "pane" } else { "thumb" }), &dir, &name, modified_ms, &size])
}

/// Changes when the icons of customised folders are to be asked for again (`refresh_folder_icons`).
static FOLDER_ICON_GEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// The icons of customised folders are asked for again: a folder's icon was changed in our Properties, or the user refreshed (F5).
///
/// WHY (issue 65: "anything changed does not get reflected in the treeview or fileview"): such an icon is kept under the folder's
/// time and attributes, and changing the icon of a folder that already has one changes neither (the folder is read-only already,
/// and rewriting its desktop.ini does not touch the folder's time), so the old picture stayed.
pub fn refresh_folder_icons() {
    FOLDER_ICON_GEN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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

    /// Small shell icon for a row. Generic types are cached per extension, plain folders once; a folder that can have an icon of its
    /// own (`folder_has_own_icon`) is asked for it, like a program or a shortcut.
    pub fn icon(&mut self, dir: &Path, e: &Entry, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let unique = own_picture(dir, e);
        // A user's theme-folder picture wins over a freedesktop pack (issues 93 and 94).
        if let Some(f) = crate::themes::icon_file(e.is_dir(), &e.ext, unique) {
            return self.theme_icon(f, ICON_PX, cx);
        }
        let pack = crate::settings::icon_theme_pack();
        if let Some(icon) = themed_picture(dir, e, crate::settings::icon_theme(), &pack, false, ICON_PX) {
            return Some(icon);
        }
        let key = if unique && e.is_dir() {
            // (with the folder's time and attributes: an icon chosen later changes one of them, and the old picture is not kept)
            let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
            key_of(&[&"dirpath", &dir, &&*e.name, &e.modified_ms, &e.attrs, &ICON_PX, &generation])
        } else if e.is_dir() {
            key_of(&[&"dir", &ICON_PX])
        } else if unique {
            key_of(&[&"path", &dir, &&*e.name, &ICON_PX])
        } else {
            key_of(&[&"ext", &&*e.ext, &ICON_PX])
        };
        let path = dir.join(&*e.name);
        // Per-file icons (exe/lnk/…) are unbounded in number: keep them in the LRU too.
        let r = self.lookup(key, || Request::new(key, path, ICON_PX, Kind::Icon), unique);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    /// A theme folder's picture (`crate::themes`) at `px`. Few (one per kind of item): kept for good.
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
        let key = key_of(&[&"shell", &parsing, &px]);
        let path = std::path::PathBuf::from(parsing);
        let r = self.lookup(key, || Request::new(key, path, px, Kind::Thumbnail), true);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
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
        self.thumbnail_as(dir, e, size, false, cx)
    }

    /// The picture the user is looking at right now (the preview pane's, Quick Look's): made by the pool's urgent lane,
    /// at normal priority and not behind the list's own tiles. The list's tiles use `thumbnail`.
    pub fn pane_picture(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        self.thumbnail_as(dir, e, size, true, cx)
    }

    fn thumbnail_as(&mut self, dir: &Path, e: &Entry, size: u32, urgent: bool, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        // the theme folder's picture for what has no content picture (a folder too: the theme's folder wins over its preview)
        if !crate::icons::has_thumbnail(e)
            && let Some(f) = crate::themes::icon_file(e.is_dir(), &e.ext, own_picture(dir, e))
        {
            return self.theme_icon(f, size, cx);
        }
        // a folder at a large size shows what is in it, as Explorer's do (issue 158): the shell draws that picture (the folder with its
        // first pictures in it); a folder it has none for gets the folder icon back (`rf_win::thumbs`)
        // (not a folder with an icon of its own, nor one inside an archive or on FTP, which the shell cannot open)
        let folder_preview = e.is_dir()
            && size >= FOLDER_PREVIEW_MIN
            && !own_picture(dir, e)
            && !rf_win::ftp::is_ftp(dir)
            && !crate::nav_touch::may_hold_archive(dir);
        let wants_thumb = crate::icons::has_thumbnail(e) || folder_preview;
        // A pack picture applies when no real content thumbnail is pending. Folder previews keep the shell picture: the pack has no
        // first-pictures-in-it artwork. Without thumbnails the pack still supplies the folder/file kind icon.
        let pack = crate::settings::icon_theme_pack();
        if !folder_preview {
            if let Some(icon) = themed_picture(dir, e, crate::settings::icon_theme(), &pack, true, size) {
                return Some(icon);
            }
        }
        let per_file = wants_thumb || own_picture(dir, e);
        let key = if per_file && e.is_dir() && !wants_thumb {
            key_of(&[&"dirpathL", &dir, &&*e.name, &e.modified_ms, &e.attrs, &size])
        } else if per_file {
            file_picture_key(urgent && wants_thumb, dir, &e.name, &e.modified_ms, size)
        } else if e.is_dir() {
            key_of(&[&"dirL", &size])
        } else {
            key_of(&[&"extL", &&*e.ext, &size])
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

/// Whether an entry's icon is its own and not its kind's: a program, a shortcut and the like, or a folder with a real
/// `desktop.ini` icon. A READONLY folder with no icon (or only a Windows type icon) is not one: the theme draws it.
fn own_picture(dir: &Path, e: &Entry) -> bool {
    if e.is_dir() { folder_keeps_shell_icon(dir, e) } else { has_unique_icon(&e.ext) }
}

/// A `desktop.ini` icon that cannot give the folder a picture: a Windows type icon (Pictures, Documents,
/// ...) the pack draws itself, or a file that no longer exists. Only a real, present user-chosen icon keeps the shell picture.
fn folder_icon_is_real(file: &str, folder: &Path) -> bool {
    let file = rf_win::dock::pins::expand_vars(file.trim(), |name| std::env::var(name).ok());
    let path = PathBuf::from(file);
    let path = if path.is_relative() { folder.join(path) } else { path };
    let lower = path.to_string_lossy().to_ascii_lowercase().replace('\\', "/");
    let system_dll = matches!(path.file_name().and_then(|n| n.to_str()).map(str::to_ascii_lowercase).as_deref(), Some("imageres.dll" | "shell32.dll"));
    let in_windows = std::env::var("SystemRoot").map(|r| {
        let root = r.trim_end_matches(['\\', '/']).to_ascii_lowercase().replace('\\', "/");
        lower.starts_with(&format!("{root}/"))
    }).unwrap_or(false);
    !system_dll && !in_windows && path.exists()
}

/// The mark alone is not enough: `C:\Users` and `Program Files (x86)` are READONLY but have no custom picture.
/// `desktop.ini` is parsed once per folder state; the refresh generation (F5/Properties) invalidates it.
pub(crate) fn folder_keeps_shell_icon(dir: &Path, e: &Entry) -> bool {
    if !folder_has_own_icon(e.attrs) {
        return false;
    }
    let path = dir.join(&*e.name);
    let generation = FOLDER_ICON_GEN.load(std::sync::atomic::Ordering::Relaxed);
    static CACHE: OnceLock<Mutex<HashMap<(PathBuf, i64, u32, u32), bool>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (path.clone(), e.modified_ms, e.attrs, generation);
    if let Some(hit) = cache.lock().unwrap().get(&key) {
        return *hit;
    }
    let found = rf_win::folder_custom::folder_icon(&path).is_some_and(|(file, _)| folder_icon_is_real(&file, &path));
    cache.lock().unwrap().insert(key, found);
    found
}

/// Pack picture for an entry, or `None` when the shell (or a content thumbnail) owns it.
/// `skip_thumbnail_kinds` is set on the thumbnail path: there an image's own preview must win; with thumbnails off the pack still
/// supplies the file-kind picture.
fn themed_picture(dir: &Path, e: &Entry, pref: crate::settings::IconThemePref, pack: &str, skip_thumbnail_kinds: bool, px: u32) -> Option<Arc<RenderImage>> {
    // A pack owns system folders too; only a real `desktop.ini` icon wins. Programs and shortcuts keep their shell picture.
    if own_picture(dir, e) || (skip_thumbnail_kinds && crate::icons::has_thumbnail(e)) {
        return None;
    }
    let themed = if e.is_dir() {
        let names = crate::icon_theme::special_folder_names(&dir.join(&*e.name));
        (!names.is_empty()).then(|| crate::icon_theme::image_for_names(pack, names, px)).flatten()
    } else {
        None
    };
    themed.or_else(|| crate::icon_theme::image(pref, pack, crate::icons::category(e), &e.ext, px))
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the toolkit's glob import brings a `test` attribute of its own that shadows the standard one)
    use super::{cost, file_picture_key, folder_keeps_shell_icon, forget_pending, own_picture, refresh_folder_icons, themed_picture};
    use crate::settings::IconThemePref;

    /// A plain folder shares the folders' one picture; a folder Windows marks as customised (read-only or system) is asked for its
    /// own, as a program is; a document is not.
    #[test]
    fn a_customised_folder_and_a_program_have_pictures_of_their_own() {
        use rf_core::{Entry, EntryKind};
        let dir = |attrs: u32| Entry::new("klasör", EntryKind::Dir, 0, 0, 0, 0x10 | attrs);
        let here = Path::new(r"C:\");
        assert!(!own_picture(here, &dir(0)), "a plain folder");
        assert!(!own_picture(here, &dir(0x2)), "a hidden folder is not a customised one");
        assert!(!own_picture(here, &dir(0x1)), "read-only alone has no icon");
        assert!(!own_picture(here, &dir(0x4)), "a system folder has no icon of its own");
        assert!(own_picture(here, &Entry::new("a.exe", EntryKind::File, 1, 0, 0, 0x20)));
        assert!(!own_picture(here, &Entry::new("a.txt", EntryKind::File, 1, 0, 0, 0x21)), "a read-only document is still its kind's picture");
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
        assert!(themed_picture(dir, &doc, legacy, "", true, 16).is_none());
        assert!(themed_picture(dir, &pic, legacy, "", false, 16).is_none());
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

    /// A type icon from Windows (the Pictures/Documents desktop.ini on OneDrive) is not a user's choice: a pack must be able to replace it.
    #[test]
    fn a_windows_type_icon_does_not_keep_the_shell_picture() {
        use rf_core::{Entry, EntryKind};
        let root = std::env::temp_dir().join(format!("rf-icon-type-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let folder = root.join("Pictures");
        std::fs::create_dir_all(&folder).unwrap();
        let icon = root.join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        let e = || Entry::new("Pictures", EntryKind::Dir, 0, 0, 0, 0x10 | 0x1);
        for icon in ["%SystemRoot%\\system32\\imageres.dll,-112", "C:\\Windows\\System32\\shell32.dll,4", "imageres.dll,-113"] {
            std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={icon}\n")).unwrap();
            refresh_folder_icons();
            assert!(!folder_keeps_shell_icon(&root, &e()), "{icon} is a Windows type icon");
        }
        std::fs::write(folder.join("desktop.ini"), format!("[.ShellClassInfo]\nIconResource={},0\n", icon.display())).unwrap();
        refresh_folder_icons();
        assert!(folder_keeps_shell_icon(&root, &e()), "a user-chosen icon still wins");
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
        let root = std::env::temp_dir().join(format!("rf-icon-drive-{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&root).unwrap();
        let icon = root.join("my.ico");
        std::fs::write(&icon, b"ico").unwrap();
        assert!(super::folder_icon_is_real(&icon.to_string_lossy(), &root));
        assert!(!super::folder_icon_is_real("C:\\Windows\\System32\\imageres.dll", &root));
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
