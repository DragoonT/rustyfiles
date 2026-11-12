//! The dock's icons: one bitmap per app / place, asked from the shell on a worker thread at the biggest size it will ever be drawn
//! (so growing on hover never enlarges a bitmap) and kept until the dock ends or the scale changes. Until one has arrived, or when
//! an app has none, the island draws a placeholder with the app's first letter.
//!
//! The shell can fail to give an icon for a moment (its icon cache is cold right after sign-in and many icons are asked for at
//! once), so a failure is not final: the icon is asked for again a few times, a few seconds apart.

use std::collections::HashMap;
use std::sync::Arc;
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
            Some(State::Ready(img)) => Some(img.clone()),
            _ => None,
        }
    }

    /// Whether the caller has to start loading this icon now: a new one, or one that failed long enough ago and has tries left. It
    /// is marked as loading.
    pub fn start_loading(&mut self, key: &str, now: Instant) -> bool {
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

    /// Forgets everything (the scale changed: the bitmaps have the wrong size).
    pub fn clear(&mut self) {
        self.map.clear();
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Asks for the icon at `px` device px square, from the first of `srcs` that has one (the app's own, what its windows have set
/// themselves, its package's logo). Blocks (COM, the shell, other programs' windows): worker thread only.
pub fn load_bgra(srcs: &[IconSource], px: u32) -> Option<rf_win::thumbs::Bgra> {
    match rf_win::dock::icons::icon_bgra_chain(srcs, px) {
        Ok(bgra) => Some(bgra),
        Err(e) => {
            log::debug!("dock icon {:?}: {e:#}", srcs.first());
            None
        }
    }
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

    #[test]
    fn clearing_forgets_everything() {
        let mut c = IconCache::default();
        let t = Instant::now();
        c.start_loading("a", t);
        c.clear();
        assert_eq!(c.len(), 0);
        assert!(c.start_loading("a", t));
    }
}
