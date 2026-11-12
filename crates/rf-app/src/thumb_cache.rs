//! The settings block of the thumbnail cache and the glue between the caches and it.
//!
//! WHAT THE CACHE IS: `image_cache::ImageCache` keeps decoded thumbnails and per-file / per-type
//! icons in MEMORY only (rustyfiles has no thumbnail files of its own), least recently used out
//! first once a byte budget is passed. There can be several such caches (one per workspace window,
//! the quick-search popup, the desktop view, the preview windows share one); each tells this module
//! how much it holds through a pair of counters, so "how big is it now" is two atomic loads per
//! cache: nothing is walked or read from a disk, and nothing waits.
//!
//! WHAT IT IS NOT: Windows' own thumbnail cache (the shell's database files in the user's Explorer folder) is
//! asked for pictures through the shell like any program does; its database files are never opened,
//! measured, emptied or deleted here. "Önbelleği temizle" empties the memory caches only; it
//! touches no file.
//!
//! The limit is a setting (`thumb_cache_mb`, absent from the settings file while it is the default
//! 64 MB), applied to each cache, so the total can be that much per cache that exists (the page
//! says so, and shows how many caches it adds up). The rule that turns the setting into a budget,
//! and which pictures go over it, is `rf_core::thumbcache`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_core::thumbcache;

use crate::image_cache::ImageCache;
use crate::select_box::{Choice, select_box};
use crate::settings::Settings;
use crate::settings_view::{button, card};
use crate::t;
use crate::theme::Palette;

/// What one cache holds: the bytes it charges and how many pictures it tracks.
#[derive(Default)]
pub struct CacheStats {
    bytes: AtomicUsize,
    images: AtomicUsize,
}

impl CacheStats {
    pub fn set(&self, bytes: usize, images: usize) {
        self.bytes.store(bytes, Ordering::Relaxed);
        self.images.store(images, Ordering::Relaxed);
    }
}

/// What "clear" removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cleared {
    pub images: usize,
    pub bytes: usize,
}

/// The caches that exist (weakly: a closed window's cache drops out) and the last clear's result.
#[derive(Default)]
struct Registry {
    caches: Vec<(WeakEntity<ImageCache>, Arc<CacheStats>)>,
    last_clear: Option<Cleared>,
}
impl Global for Registry {}

pub fn register(cache: WeakEntity<ImageCache>, stats: Arc<CacheStats>, cx: &mut App) {
    let r = cx.default_global::<Registry>();
    r.caches.retain(|(w, _)| w.upgrade().is_some());
    r.caches.push((cache, stats));
}

/// The byte budget a cache works with (the default when there is no setting yet, e.g. in a process
/// that has no settings global).
pub fn budget(cx: &App) -> usize {
    thumbcache::budget_bytes(cx.try_global::<Settings>().and_then(|s| s.thumb_cache_mb))
}

/// (bytes charged, pictures tracked, caches alive) over every cache.
pub fn totals(cx: &App) -> (usize, usize, usize) {
    let Some(r) = cx.try_global::<Registry>() else { return (0, 0, 0) };
    r.caches.iter().filter(|(w, _)| w.upgrade().is_some()).fold((0, 0, 0), |(b, i, n), (_, s)| {
        (b + s.bytes.load(Ordering::Relaxed), i + s.images.load(Ordering::Relaxed), n + 1)
    })
}

/// Empties every cache. Only memory: see the module comment.
pub fn clear_all(cx: &mut App) -> Cleared {
    let live: Vec<Entity<ImageCache>> = match cx.try_global::<Registry>() {
        Some(r) => r.caches.iter().filter_map(|(w, _)| w.upgrade()).collect(),
        None => Vec::new(),
    };
    let mut total = Cleared { images: 0, bytes: 0 };
    for c in live {
        let (images, bytes) = c.update(cx, |c, cx| c.clear_all(cx));
        total.images += images;
        total.bytes += bytes;
    }
    cx.default_global::<Registry>().last_clear = Some(total);
    total
}

/// The settings cards: the limit, and the size now with the clear button.
pub fn cards(p: &Palette, s: &Settings, cx: &App) -> Vec<AnyElement> {
    let current = thumbcache::limit_mb(s.thumb_cache_mb);
    let (bytes, images, caches) = totals(cx);
    let mut now = format!("{} {} · {}", images, t!("görsel"), rf_core::format::size(bytes as u64));
    if caches > 1 {
        // (each window has its own cache with its own share of the limit: the sum can pass it)
        now = format!("{now} · {caches} {}", t!("önbellek toplamı"));
    }
    if let Some(c) = cx.try_global::<Registry>().and_then(|r| r.last_clear) {
        now = format!("{now}. {}: {} {} · {}", t!("Son temizlik"), c.images, t!("görsel"), rf_core::format::size(c.bytes as u64));
    }
    let mut choices: Vec<u32> = thumbcache::CHOICES_MB.to_vec();
    if !choices.contains(&current) {
        // (a value written into the settings file by hand stays visible in the menu)
        choices.push(current);
        choices.sort_unstable();
    }
    vec![
        card(
            p,
            L::Image,
            t!("Küçük resim önbelleği"),
            Some(t!("Küçük resimler ve simgeler yalnızca bellekte tutulur; sınır her pencerenin kendi önbelleği içindir ve aşılınca en eskiden kullanılanlar atılır. Windows'un kendi küçük resim önbelleğine dokunulmaz.")),
            select_box("thumb-limit", p, format!("{current} MB"), move |_| {
                choices
                    .iter()
                    .map(|&mb| {
                        Choice::new(format!("{mb} MB"), mb == current, move |_, cx| {
                            crate::settings::update(cx, |s| s.thumb_cache_mb = thumbcache::setting_for(mb));
                        })
                    })
                    .collect()
            }),
        ),
        card(
            p,
            L::Eraser,
            t!("Önbellekte şu an"),
            Some(now.as_str()),
            button("thumb-clear", p, t!("Önbelleği temizle"), |window, cx| {
                clear_all(cx);
                window.refresh();
            }),
        ),
    ]
}

#[cfg(test)]
mod tests {
    /// The cache is memory only, and the clear must stay that way: none of the code that makes,
    /// measures or empties it may delete, create or open a file, or name Windows' thumbnail cache.
    #[test]
    fn the_cache_code_touches_no_file() {
        // (built from pieces so that this test does not contain what it forbids)
        let needles = [
            ["remove", "_file"].concat(),
            ["remove", "_dir"].concat(),
            ["std::", "fs"].concat(),
            ["fs", "::"].concat(),
            ["File::", "create"].concat(),
            ["File::", "open"].concat(),
            ["Open", "Options"].concat(),
            ["read", "_dir"].concat(),
            ["Create", "File"].concat(),
            ["Delete", "File"].concat(),
            ["SHFile", "Operation"].concat(),
            ["thumb", "cache_"].concat(),
            // the shell's thumbnail database folder, as a path would spell it in a string
            ["Windows", "\\\\", "Explorer"].concat(),
            ["Windows", "/", "Explorer"].concat(),
        ];
        for (name, src) in [("thumb_cache.rs", include_str!("thumb_cache.rs")), ("image_cache.rs", include_str!("image_cache.rs"))] {
            // (this test module names the needles; the code above it must not)
            let code = src.split("#[cfg(test)]").next().unwrap_or(src);
            for n in &needles {
                assert!(!code.contains(n.as_str()), "{name} contains {n}");
            }
        }
    }
}
