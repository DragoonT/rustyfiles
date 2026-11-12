//! Rectangles, sizes and colours in whole pixels. Coordinates are `i32` because the virtual screen has negative coordinates when a
//! monitor sits left of or above the primary one.

use serde::{Deserialize, Serialize};

/// `right` and `bottom` are exclusive, like Win32's `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub const fn new(left: i32, top: i32, right: i32, bottom: i32) -> Self {
        Self { left, top, right, bottom }
    }
    pub const fn from_size(left: i32, top: i32, w: u32, h: u32) -> Self {
        Self { left, top, right: left + w as i32, bottom: top + h as i32 }
    }
    pub const fn width(&self) -> i32 {
        self.right - self.left
    }
    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }
    pub const fn is_empty(&self) -> bool {
        self.right <= self.left || self.bottom <= self.top
    }
    pub fn size(&self) -> Size {
        Size { w: self.width().max(0) as u32, h: self.height().max(0) as u32 }
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.width() as i64 * self.height() as i64 }
    }
    pub const fn offset(&self, dx: i32, dy: i32) -> Self {
        Self { left: self.left + dx, top: self.top + dy, right: self.right + dx, bottom: self.bottom + dy }
    }
    /// The smallest rectangle holding both (an empty one is ignored).
    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        Rect { left: self.left.min(o.left), top: self.top.min(o.top), right: self.right.max(o.right), bottom: self.bottom.max(o.bottom) }
    }
    pub fn intersect(&self, o: &Rect) -> Rect {
        let r = Rect { left: self.left.max(o.left), top: self.top.max(o.top), right: self.right.min(o.right), bottom: self.bottom.min(o.bottom) };
        if r.is_empty() { Rect::default() } else { r }
    }
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }
}

/// The bounding rectangle of all monitors (the virtual screen).
pub fn virtual_screen(monitors: &[Rect]) -> Rect {
    monitors.iter().fold(Rect::default(), |acc, r| acc.union(r))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }
    pub const fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// A Win32 `COLORREF` (0x00BBGGRR).
    pub const fn from_colorref(c: u32) -> Self {
        Rgb((c & 0xFF) as u8, ((c >> 8) & 0xFF) as u8, ((c >> 16) & 0xFF) as u8)
    }
    pub const fn to_colorref(self) -> u32 {
        self.0 as u32 | (self.1 as u32) << 8 | (self.2 as u32) << 16
    }
    /// The registry's "R G B" (`Control Panel\Colors\Background` = `195 0 82`): three decimal numbers 0 to 255 separated by
    /// white space. Anything else is `None`, never a guess.
    pub fn parse_triple(s: &str) -> Option<Self> {
        let mut it = s.split_whitespace();
        let mut next = || it.next()?.parse::<u8>().ok();
        let (r, g, b) = (next()?, next()?, next()?);
        if it.next().is_some() {
            return None;
        }
        Some(Rgb(r, g, b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_basics_with_negative_origin() {
        let portrait = Rect::new(-1440, -560, 0, 2000);
        let landscape = Rect::new(0, 0, 1920, 1080);
        let v = virtual_screen(&[landscape, portrait]);
        assert_eq!(v, Rect::new(-1440, -560, 1920, 2000));
        assert_eq!((v.width(), v.height()), (3360, 2560));
        assert!(v.contains(-1440, -560) && !v.contains(1920, 0));
        assert_eq!(portrait.intersect(&landscape), Rect::default(), "touching edges do not overlap");
        assert_eq!(landscape.offset(-1440, -560), Rect::new(-1440, -560, 480, 520));
        assert_eq!(Rect::default().union(&landscape), landscape);
        assert_eq!(landscape.area(), 1920 * 1080);
    }

    #[test]
    fn colour_parsing() {
        assert_eq!(Rgb::parse_triple("195 0 82"), Some(Rgb(195, 0, 82)));
        assert_eq!(Rgb::parse_triple("  0   0  0 "), Some(Rgb(0, 0, 0)));
        for bad in ["", "1 2", "1 2 3 4", "256 0 0", "-1 0 0", "a b c", "1,2,3"] {
            assert_eq!(Rgb::parse_triple(bad), None, "{bad:?}");
        }
        // COLORREF is blue-green-red
        assert_eq!(Rgb::from_colorref(0x0052_00C3), Rgb(195, 0, 82));
        assert_eq!(Rgb(195, 0, 82).to_colorref(), 0x0052_00C3);
    }
}
