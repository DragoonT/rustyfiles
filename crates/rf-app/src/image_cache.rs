//! Shared cache of shell icons and thumbnails. Misses are queued to rf-win's low-priority
//! worker pool; rows render a symbol fallback until the real image arrives.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::Arc;

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::*;
use rf_core::Entry;
use rf_win::thumbs::{Bgra, Kind, Request, Response, ThumbPool, has_unique_icon};

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
                            if r.dropped {
                                c.map.remove(&r.key);
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

    /// Small shell icon for a row. Generic types are cached per extension, folders once.
    pub fn icon(&mut self, dir: &Path, e: &Entry, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let unique = !e.is_dir() && has_unique_icon(&e.ext);
        let key = if e.is_dir() {
            key_of(&[&"dir", &ICON_PX])
        } else if unique {
            key_of(&[&"path", &dir, &&*e.name, &ICON_PX])
        } else {
            key_of(&[&"ext", &&*e.ext, &ICON_PX])
        };
        let path = dir.join(&*e.name);
        // Per-file icons (exe/lnk/…) are unbounded in number: keep them in the LRU too.
        let r = self.lookup(key, || Request { key, path, size: ICON_PX, kind: Kind::Icon }, unique);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    /// Large thumbnail (falls back to the large icon when the file has no thumbnail).
    /// The shell's picture for a parsing name (an app of "All apps": `shell:AppsFolder\<id>`).
    /// Asked as a thumbnail: inside rustyfiles the icon of a Store app comes back as the shell's
    /// blank app placeholder, its thumbnail is the app's own logo.
    pub fn shell_icon(&mut self, parsing: &str, px: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let key = key_of(&[&"shell", &parsing, &px]);
        let path = std::path::PathBuf::from(parsing);
        let r = self.lookup(key, || Request { key, path, size: px, kind: Kind::Thumbnail }, true);
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
        let r = self.lookup(key, || Request { key, path, size: px, kind: Kind::Thumbnail }, true);
        self.evict_over_budget();
        self.release_evicted(cx);
        r
    }

    pub fn thumbnail(&mut self, dir: &Path, e: &Entry, size: u32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let wants_thumb = crate::icons::has_thumbnail(e);
        let per_file = wants_thumb || (!e.is_dir() && has_unique_icon(&e.ext));
        let key = if per_file {
            key_of(&[&"thumb", &dir, &&*e.name, &e.modified_ms, &size])
        } else if e.is_dir() {
            key_of(&[&"dirL", &size])
        } else {
            key_of(&[&"extL", &&*e.ext, &size])
        };
        let path = dir.join(&*e.name);
        let kind = if wants_thumb { Kind::Thumbnail } else { Kind::Icon };
        // every per-file picture (an exe's or a shortcut's icon too) is in the LRU, or browsing
        // folders of programs would keep them all until the app closes
        let r = self.lookup(key, || Request { key, path, size, kind }, per_file);
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
