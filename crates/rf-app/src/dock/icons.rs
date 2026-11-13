//! The dock's icons: one bitmap per app / place, loaded on a worker thread from the best source for the biggest size it will ever be
//! drawn (so growing on hover never enlarges a bitmap) and kept until the dock ends or the scale changes. Until one has arrived, or when
//! an app has none, the island draws a placeholder with the app's first letter.
//!
//! The shell can fail to give an icon for a moment (its icon cache is cold right after sign-in and many icons are asked for at
//! once), so a failure is not final: the icon is asked for again a few times, a few seconds apart.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use gpui_kit::RenderImage;
use rf_win::dock::IconSource;

/// How many times in all an icon is asked for before the placeholder stays for good.
pub const MAX_TRIES: u8 = 4;
/// The wait before an icon that failed is asked for again.
pub const RETRY_AFTER: Duration = Duration::from_secs(6);

enum State {
    /// Being loaded; `tries` counts the asks so far, this one included.
    Loading { tries: u8 },
    Ready(Arc<RenderImage>),
    /// Was ready, but at a size that is not wanted any more (the dock's size or the display scale changed): still drawn, so the icon does
    /// not flash back to its letter, and to be asked for again.
    Stale(Arc<RenderImage>),
    /// A stale icon whose new bitmap is being loaded: still drawn until it arrives. `tries` counts the asks of the new bitmap so far.
    Refreshing { img: Arc<RenderImage>, tries: u8 },
    /// The new bitmap of a stale icon could not be had: the old one is still drawn (it is of another size, so a little soft or
    /// enlarged), and the new one is asked for again after [`RETRY_AFTER`] while `tries` is below [`MAX_TRIES`].
    RefreshFailed { img: Arc<RenderImage>, tries: u8, at: Instant },
    /// The last ask failed. Asked again after [`RETRY_AFTER`] while `tries` is below [`MAX_TRIES`].
    Missing { tries: u8, at: Instant },
}

#[derive(Default)]
pub struct IconCache {
    map: HashMap<String, State>,
}

impl IconCache {
    /// The finished icon for this key (`plan::icon_key`), if there is one.
    pub fn get(&self, key: &str) -> Option<Arc<RenderImage>> {
        match self.map.get(key) {
            Some(State::Ready(img) | State::Stale(img) | State::Refreshing { img, .. } | State::RefreshFailed { img, .. }) => Some(img.clone()),
            _ => None,
        }
    }

    /// Whether the caller has to start loading this icon now: a new one, or one that failed long enough ago and has tries left. It
    /// is marked as loading.
    pub fn start_loading(&mut self, key: &str, now: Instant) -> bool {
        match self.map.get(key) {
            Some(State::Stale(img)) => {
                let img = img.clone();
                self.map.insert(key.to_string(), State::Refreshing { img, tries: 1 });
                return true;
            }
            Some(State::RefreshFailed { img, tries, at }) => {
                if *tries >= MAX_TRIES || now.saturating_duration_since(*at) < RETRY_AFTER {
                    return false;
                }
                let (img, tries) = (img.clone(), *tries + 1);
                self.map.insert(key.to_string(), State::Refreshing { img, tries });
                return true;
            }
            _ => {}
        }
        let tries = match self.map.get(key) {
            None => 0,
            Some(State::Missing { tries, at }) if *tries < MAX_TRIES && now.saturating_duration_since(*at) >= RETRY_AFTER => *tries,
            Some(_) => return false,
        };
        self.map.insert(key.to_string(), State::Loading { tries: tries + 1 });
        true
    }

    /// A load ended. Returns true when it failed and the icon will be asked for again (the caller schedules that).
    pub fn finish(&mut self, key: &str, image: Option<Arc<RenderImage>>, now: Instant) -> bool {
        match image {
            Some(img) => {
                self.map.insert(key.to_string(), State::Ready(img));
                false
            }
            None if matches!(self.map.get(key), Some(State::Refreshing { .. })) => {
                // the new size could not be had: the old bitmap is still a picture of the app (a little soft), better than the letter,
                // and the new one is asked for again a few times - a hiccup of the shell or of a package folder must not leave the
                // icon at the wrong size for good
                let Some(State::Refreshing { img, tries }) = self.map.remove(key) else { return false };
                if tries < MAX_TRIES {
                    self.map.insert(key.to_string(), State::RefreshFailed { img, tries, at: now });
                    true
                } else {
                    self.map.insert(key.to_string(), State::Ready(img));
                    false
                }
            }
            None => {
                let tries = match self.map.get(key) {
                    Some(State::Loading { tries }) => *tries,
                    _ => 1,
                };
                self.map.insert(key.to_string(), State::Missing { tries, at: now });
                tries < MAX_TRIES
            }
        }
    }

    /// Forgets everything (the Start panel's icons: nothing of them is on show while they are asked for again).
    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// The bitmaps have the wrong size (the display scale or the dock's size changed): every finished icon stays on show until its new
    /// bitmap arrives, so nothing flashes back to a letter; what was still loading or had failed is forgotten, because it was asked for
    /// at the old size (a load of the old size that ends later is dropped by the caller).
    pub fn mark_stale(&mut self) {
        let old = std::mem::take(&mut self.map);
        for (key, state) in old {
            if let State::Ready(img) | State::Stale(img) | State::Refreshing { img, .. } | State::RefreshFailed { img, .. } = state {
                self.map.insert(key, State::Stale(img));
            }
        }
    }

    /// Keeps only the entries whose key `keep` accepts (a cache that must not grow for ever lets go of what is not worth keeping).
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        self.map.retain(|k, _| keep(k));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Asks for the icon at `px` device px square, from the best source of `srcs` (a package's own logo files, a program's icon frame, the
/// shell, a window's icon last: `rf_win::dock::iconload`), resampled by us. Blocks (the disk, COM, the shell, other programs'
/// windows): worker thread only.
pub fn load_bgra(srcs: &[IconSource], px: u32) -> Option<rf_win::thumbs::Bgra> {
    match rf_win::dock::iconload::load_one(srcs, px) {
        Ok(bgra) => Some(bgra),
        Err(e) => {
            log::debug!("dock icon {:?}: {e:#}", srcs.first());
            None
        }
    }
}

/// The same icon at every one of `sizes` (ascending; see `rf_win::dock::iconpick::level_sizes`), each made from the one best picture.
/// Worker thread only.
pub fn load_levels(srcs: &[IconSource], sizes: &[u32]) -> Option<Vec<rf_win::thumbs::Bgra>> {
    match rf_win::dock::iconload::load_levels(srcs, sizes) {
        Ok(got) => Some(got.levels),
        Err(e) => {
            log::debug!("dock icon {:?}: {e:#}", srcs.first());
            None
        }
    }
}

/// What a load made for one icon at one list of sizes, held by every dock that asked for exactly that while any of them still holds it.
pub type SharedLoad = OnceLock<Option<Vec<rf_win::thumbs::Bgra>>>;

/// How long the dock that made a load keeps it for the others after it has its own pictures: the island of another monitor reacts to
/// the same window event a few milliseconds later, and its shell call would otherwise be made again.
pub const SHARE_LINGER: Duration = Duration::from_millis(400);

/// The loads in progress or lingering: the icon's key and the sizes asked for, and the load. A load nobody holds any more is gone.
static LOADS: Mutex<Vec<((String, Vec<u32>), Weak<SharedLoad>)>> = Mutex::new(Vec::new());

/// [`load_levels`] for a dock that is not alone: the docks of monitors with the same dock unit ask for the same icon at the same sizes,
/// and the second to ask waits for the first one's load and takes its pictures (a copy) instead of asking the shell and resampling
/// again. The second return value is the load, to be held for [`SHARE_LINGER`] after the caller is done so that a dock that asks a
/// moment later finds it; dropped, the pictures are freed. With `shared` false (one dock) this is [`load_levels`] and nothing is kept.
/// Worker thread only.
pub fn load_levels_shared(key: &str, srcs: &[IconSource], sizes: &[u32], shared: bool) -> (Option<Vec<rf_win::thumbs::Bgra>>, Option<Arc<SharedLoad>>) {
    if !shared {
        return (load_levels(srcs, sizes), None);
    }
    let load = {
        let mut loads = LOADS.lock().unwrap_or_else(|p| p.into_inner());
        loads.retain(|(_, w)| w.strong_count() > 0);
        match loads.iter().find(|((k, s), _)| k == key && s == sizes).and_then(|(_, w)| w.upgrade()) {
            Some(load) => load,
            None => {
                let load = Arc::new(SharedLoad::new());
                loads.push(((key.to_string(), sizes.to_vec()), Arc::downgrade(&load)));
                load
            }
        }
    };
    // (the first caller runs the load; one that comes meanwhile waits here and gets the same answer)
    let got = load.get_or_init(|| load_levels(srcs, sizes)).clone();
    (got, Some(load))
}

/// The set of pictures a [`load_levels`] gave, ready to draw. UI thread.
pub fn to_levels(bgra: Vec<rf_win::thumbs::Bgra>) -> Option<super::icon_levels::Levels> {
    let sizes: Vec<u32> = bgra.iter().map(|b| b.width).collect();
    let images: Vec<Arc<RenderImage>> = bgra.into_iter().filter_map(to_image).collect();
    super::icon_levels::Levels::new(sizes, images)
}

/// The picture GPUI draws. GPUI's `RenderImage` holds BGRA bytes inside an image buffer (the `image` crate's type is only the
/// container, as in the earlier taskbar preview); the alpha is straight, as for any decoded picture. UI thread.
pub fn to_image(bgra: rf_win::thumbs::Bgra) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(bgra.width, bgra.height, bgra.data)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docks_that_ask_for_the_same_icon_at_the_same_sizes_share_one_load_while_one_holds_it() {
        // (no sources: the load fails at once, without the shell or a disk; what is checked is who shares what)
        let (a, load_a) = load_levels_shared("test-share-1", &[], &[44, 66], true);
        let (b, load_b) = load_levels_shared("test-share-1", &[], &[44, 66], true);
        assert!(a.is_none() && b.is_none());
        let (load_a, load_b) = (load_a.expect("held"), load_b.expect("held"));
        assert!(Arc::ptr_eq(&load_a, &load_b), "the second dock took the first one's load");
        // another size list (a monitor of another scale) or another icon is another load
        let (_, other_sizes) = load_levels_shared("test-share-1", &[], &[44, 88], true);
        let (_, other_icon) = load_levels_shared("test-share-2", &[], &[44, 66], true);
        assert!(!Arc::ptr_eq(&load_a, &other_sizes.as_ref().expect("held").clone()));
        assert!(!Arc::ptr_eq(&load_a, &other_icon.as_ref().expect("held").clone()));
        // one dock alone keeps nothing
        let (_, alone) = load_levels_shared("test-share-1", &[], &[44, 66], false);
        assert!(alone.is_none());
        // a load nobody holds is gone: the list does not grow for ever
        drop((load_a, load_b, other_sizes, other_icon));
        let _ = load_levels_shared("test-share-3", &[], &[44], true);
        let left = LOADS.lock().unwrap().iter().filter(|((k, _), _)| k.starts_with("test-share-1") || k.starts_with("test-share-2")).count();
        assert_eq!(left, 0, "released loads are forgotten");
    }

    #[test]
    fn an_icon_is_asked_for_once_at_a_time() {
        let mut c = IconCache::default();
        let t = Instant::now();
        assert!(c.start_loading("a", t));
        assert!(!c.start_loading("a", t), "already loading");
        assert!(c.start_loading("b", t));
        assert_eq!(c.len(), 2);
        assert!(c.get("a").is_none(), "still loading: the placeholder shows");
    }

    #[test]
    fn an_icon_that_failed_is_asked_for_again_a_few_times_and_then_left() {
        let mut c = IconCache::default();
        let mut t = Instant::now();
        let mut asks = 0;
        for _ in 0..10 {
            if c.start_loading("a", t) {
                asks += 1;
                let retry = c.finish("a", None, t);
                assert_eq!(retry, asks < MAX_TRIES, "ask {asks}");
                assert!(!c.start_loading("a", t), "not at once: the shell needs a moment");
            }
            t += RETRY_AFTER;
        }
        assert_eq!(asks, MAX_TRIES, "a few tries, not for ever");
        assert!(c.get("a").is_none());
    }

    #[test]
    fn a_retry_that_works_ends_the_placeholder() {
        let mut c = IconCache::default();
        let t = Instant::now();
        assert!(c.start_loading("a", t));
        assert!(c.finish("a", None, t));
        let later = t + RETRY_AFTER;
        assert!(c.start_loading("a", later));
        let img = Arc::new(RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]));
        assert!(!c.finish("a", Some(img), later));
        assert!(c.get("a").is_some());
        assert!(!c.start_loading("a", later + RETRY_AFTER * 10), "a finished icon is not asked for again");
    }

    fn picture() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]))
    }

    #[test]
    fn a_size_change_keeps_the_old_pictures_until_the_new_ones_arrive() {
        let mut c = IconCache::default();
        let t = Instant::now();
        let old = picture();
        assert!(c.start_loading("a", t));
        c.finish("a", Some(old.clone()), t);
        assert!(c.start_loading("loading", t));
        assert!(c.start_loading("failed", t));
        c.finish("failed", None, t);
        c.mark_stale();
        // no flash: the finished icon is still there to draw while it is asked for again
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &old));
        // what was loading or had failed at the old size is forgotten, so it is asked for at the new size
        assert!(c.get("loading").is_none() && c.start_loading("loading", t));
        assert!(c.start_loading("failed", t));
        // the stale icon is asked for once, and its old picture shows meanwhile
        assert!(c.start_loading("a", t));
        assert!(!c.start_loading("a", t), "already being loaded");
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &old));
        // the new bitmap replaces it
        let new = picture();
        assert!(!c.finish("a", Some(new.clone()), t));
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &new));
        assert!(!c.start_loading("a", t + RETRY_AFTER * 10), "a finished icon is not asked for again");
    }

    #[test]
    fn a_new_size_that_cannot_be_had_leaves_the_old_picture_on_show_and_is_asked_for_again() {
        let mut c = IconCache::default();
        let mut t = Instant::now();
        let old = picture();
        c.start_loading("a", t);
        c.finish("a", Some(old.clone()), t);
        c.mark_stale();
        assert!(c.start_loading("a", t));
        assert!(c.finish("a", None, t), "the caller is told to look again");
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &old), "the old picture stays on show");
        assert!(!c.start_loading("a", t), "not at once");
        t += RETRY_AFTER;
        assert!(c.start_loading("a", t), "after a while");
        assert!(!c.start_loading("a", t), "one ask at a time");
        // the ask that works replaces the picture
        let new = picture();
        assert!(!c.finish("a", Some(new.clone()), t));
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &new));
        assert!(!c.start_loading("a", t + RETRY_AFTER * 10), "a finished icon is not asked for again");
    }

    #[test]
    fn an_icon_whose_new_size_never_comes_is_asked_for_a_few_times_and_then_left_with_the_old_picture() {
        let mut c = IconCache::default();
        let mut t = Instant::now();
        let old = picture();
        c.start_loading("a", t);
        c.finish("a", Some(old.clone()), t);
        c.mark_stale();
        let mut asks = 0;
        for _ in 0..12 {
            if c.start_loading("a", t) {
                asks += 1;
                assert_eq!(c.finish("a", None, t), asks < MAX_TRIES, "ask {asks}");
            }
            t += RETRY_AFTER;
        }
        assert_eq!(asks, MAX_TRIES, "a few tries, not for ever");
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &old));
        // a second change of size starts over from the picture that is shown, however far the tries had come
        c.mark_stale();
        assert!(c.start_loading("a", t));
        assert!(c.finish("a", None, t), "the tries are counted again");
        c.mark_stale();
        c.start_loading("a", t);
        c.mark_stale();
        assert!(c.start_loading("a", t));
        assert!(Arc::ptr_eq(&c.get("a").unwrap(), &old));
    }

    #[test]
    fn forgetting_makes_everything_new() {
        let mut c = IconCache::default();
        let t = Instant::now();
        c.start_loading("a", t);
        c.mark_stale();
        assert_eq!(c.len(), 0, "a load in flight has no picture to keep");
        assert!(c.start_loading("a", t));
    }
}
