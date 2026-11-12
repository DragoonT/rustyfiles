//! The middle island's icons kept at several sizes. One picture per kept size (`rf_win::dock::iconpick::level_sizes`: the rest size,
//! the peak size and a step or two between, each made by our own resampling), and the one drawn is the smallest that is not smaller
//! than the size the icon is drawn at right then. So the picture at rest is pixel for pixel what is on the screen, the one at the
//! peak is too, and between them the toolkit's plain bilinear sampling reduces a picture by a quarter at most (so a bilinear tap never
//! skips more than a fraction of a source pixel). The picture is swapped for the next one where the drawn size passes a kept size, and
//! the sharpness takes a step there (up to that quarter); whether that step, or the picture's look while it grows, is visible was not
//! seen on a screen.
//!
//! COST. A set holds `sum(size^2) * 4` bytes of pixels, about twice the largest picture alone (37 KB at 100 % with the full
//! magnification, 150 KB at 200 %); the toolkit's sprite atlas holds the same again once they are drawn. The store is bounded
//! (`LIMIT`): when more than that many apps have come and gone only the ones in the dock are kept, and the pictures of a set that is
//! replaced or dropped are handed back to the toolkit (`retire`), which would otherwise keep them in its atlas for ever.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui_kit::{App, RenderImage};
use rf_win::dock::iconpick;

/// How many icon sets the store keeps before it lets go of the ones no item of the dock uses.
pub const LIMIT: usize = 64;

/// One icon at its kept sizes.
pub struct Levels {
    sizes: Vec<u32>,
    images: Vec<Arc<RenderImage>>,
}

impl Levels {
    /// The set for pictures of these sizes (ascending), or `None` when there is nothing to draw.
    pub fn new(sizes: Vec<u32>, images: Vec<Arc<RenderImage>>) -> Option<Self> {
        (!sizes.is_empty() && sizes.len() == images.len()).then_some(Levels { sizes, images })
    }

    /// The picture to draw an icon that is `drawn_px` device px wide with.
    pub fn pick(&self, drawn_px: f32) -> &Arc<RenderImage> {
        &self.images[iconpick::pick_level(&self.sizes, drawn_px).min(self.images.len() - 1)]
    }

    /// The largest picture (what a menu or a preview that draws the icon at its own size starts from).
    pub fn top(&self) -> &Arc<RenderImage> {
        &self.images[self.images.len() - 1]
    }

    /// The bytes of pixels the set holds.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.sizes.iter().map(|&s| (s * s * 4) as usize).sum()
    }

    fn into_images(self) -> Vec<Arc<RenderImage>> {
        self.images
    }
}

/// The sets of the icons of the dock, by icon key.
#[derive(Default)]
pub struct LevelStore {
    map: HashMap<String, Levels>,
}

impl LevelStore {
    pub fn get(&self, key: &str) -> Option<&Levels> {
        self.map.get(key)
    }

    /// Stores the set for `key`; the set it replaces is returned, to be retired.
    pub fn insert(&mut self, key: String, levels: Levels) -> Option<Levels> {
        self.map.insert(key, levels)
    }

    /// When more than `limit` sets are held, lets go of the ones `live` does not name. Returns them, to be retired.
    pub fn prune(&mut self, live: &HashSet<String>, limit: usize) -> Vec<Levels> {
        prune_keys(self.map.keys().map(String::as_str), live, limit).into_iter().filter_map(|k| self.map.remove(&k)).collect()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// The keys a store of `held` keys drops when it holds more than `limit`: those not in `live`, in key order.
pub fn prune_keys<'a>(held: impl Iterator<Item = &'a str>, live: &HashSet<String>, limit: usize) -> Vec<String> {
    let held: Vec<&str> = held.collect();
    if held.len() <= limit {
        return Vec::new();
    }
    let mut dead: Vec<String> = held.into_iter().filter(|k| !live.contains(*k)).map(str::to_string).collect();
    dead.sort();
    dead
}

/// The sets' pictures, given back to the toolkit after the current update (a window being updated is out of the app's window list for
/// a moment, so an immediate call would miss it), as `tray_strip` and `image_cache` do.
pub fn retire(sets: Vec<Levels>, cx: &mut App) {
    let images: Vec<Arc<RenderImage>> = sets.into_iter().flat_map(Levels::into_images).collect();
    if images.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for img in images {
            cx.drop_image(img, None);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picture() -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(1, 1))]))
    }

    fn set(sizes: &[u32]) -> Levels {
        Levels::new(sizes.to_vec(), sizes.iter().map(|_| picture()).collect()).unwrap()
    }

    #[test]
    fn the_picture_drawn_is_the_smallest_that_is_not_smaller_than_the_icon() {
        let l = set(&[44, 55, 69]);
        assert!(Arc::ptr_eq(l.pick(44.0), &l.images[0]), "at rest: the rest picture, one pixel to one");
        assert!(Arc::ptr_eq(l.pick(50.0), &l.images[1]));
        assert!(Arc::ptr_eq(l.pick(68.2), &l.images[2]), "at the peak: the peak picture");
        assert!(Arc::ptr_eq(l.pick(90.0), &l.images[2]), "bigger than all: the biggest");
        assert!(Arc::ptr_eq(l.top(), &l.images[2]));
        assert_eq!(l.bytes(), (44 * 44 + 55 * 55 + 69 * 69) * 4);
    }

    #[test]
    fn a_set_needs_one_picture_per_size() {
        assert!(Levels::new(vec![], vec![]).is_none());
        assert!(Levels::new(vec![44, 55], vec![picture()]).is_none());
        assert!(Levels::new(vec![44], vec![picture()]).is_some());
    }

    #[test]
    fn a_store_lets_go_of_the_dead_only_when_it_holds_too_many() {
        let live: HashSet<String> = ["a", "b"].iter().map(|s| s.to_string()).collect();
        let held = ["a", "b", "c", "d"];
        assert!(prune_keys(held.into_iter(), &live, 4).is_empty(), "not over the limit: nothing goes, an app that comes back finds its icon");
        assert_eq!(prune_keys(held.into_iter(), &live, 3), vec!["c".to_string(), "d".to_string()]);
        // the dock's own are never dropped, however low the limit
        assert_eq!(prune_keys(["a", "b", "c"].into_iter(), &live, 0), vec!["c".to_string()]);
    }

    #[test]
    fn the_store_hands_back_what_it_replaces_and_what_it_drops() {
        let mut s = LevelStore::default();
        assert!(s.insert("a".into(), set(&[44, 69])).is_none());
        assert!(s.insert("b".into(), set(&[44])).is_none());
        assert!(s.insert("c".into(), set(&[44, 55, 69])).is_none());
        let replaced = s.insert("a".into(), set(&[44])).expect("the old set of a");
        assert_eq!(replaced.sizes, vec![44, 69]);
        let live: HashSet<String> = ["a".to_string()].into();
        assert!(s.prune(&live, 10).is_empty());
        let dropped = s.prune(&live, 2);
        assert_eq!(dropped.len(), 2);
        assert_eq!(s.len(), 1);
        assert!(s.get("a").is_some() && s.get("b").is_none());
    }
}
