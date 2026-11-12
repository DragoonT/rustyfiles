//! The wallpaper maths: where a picture goes on one monitor for each of Windows' six position modes. Pure; the pixels are moved by
//! `rf_win::desktop::render`.
//!
//! Everything is in whole pixels and in the MONITOR's own coordinates (0,0 is the monitor's top-left), because one bitmap is made per
//! monitor. The monitor and the virtual screen come in virtual-screen coordinates, which are negative left of or above the primary
//! monitor (a portrait monitor on the left is typical).
//!
//! Rounding: sizes that come from a ratio are rounded half up; a picture centred in a smaller or larger area is placed with the
//! spare pixel (when the difference is odd) on the right / bottom, i.e. the offset is floored.
//!
//! FACTS AND GUESSES (shell plan note 4, 4.5): the six modes and their names are documented (`DESKTOP_WALLPAPER_POSITION`). That a
//! tile starts at the virtual screen's top-left rather than at each monitor's is the note's reading of the enum documentation and is
//! UNVERIFIED, so it is a parameter ([`TileAnchor`]). How Windows treats the dead space of a span across monitors of unequal height is
//! UNVERIFIED too; here the picture is laid over the bounding rectangle of all monitors, dead space included.

use serde::{Deserialize, Serialize};

use super::geom::{Rect, Size};

/// `DESKTOP_WALLPAPER_POSITION`: the values 0 to 5 are the same numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Position {
    Center,
    Tile,
    Stretch,
    /// The whole picture, aspect kept, bars in the background colour.
    Fit,
    /// The picture covers the monitor, aspect kept, the overflow is cropped.
    Fill,
    /// One picture across all monitors.
    Span,
}

impl Position {
    pub const fn from_dwpos(v: i32) -> Option<Position> {
        Some(match v {
            0 => Position::Center,
            1 => Position::Tile,
            2 => Position::Stretch,
            3 => Position::Fit,
            4 => Position::Fill,
            5 => Position::Span,
            _ => return None,
        })
    }
    pub const fn to_dwpos(self) -> i32 {
        match self {
            Position::Center => 0,
            Position::Tile => 1,
            Position::Stretch => 2,
            Position::Fit => 3,
            Position::Fill => 4,
            Position::Span => 5,
        }
    }
    /// The registry's `WallpaperStyle` / `TileWallpaper` strings (center 0/0, tile 0/1, stretch 2/0, fit 6/0, fill 10/0, span 22/0).
    /// Only 10/0 = Fill is confirmed on the build this was written on; the others are the widely published values. `None` for any
    /// other style number (the caller decides what to assume).
    pub fn from_registry(style: &str, tile: &str) -> Option<Position> {
        // Tile is a style 0 with the tile flag; a stale TileWallpaper=1 left behind by a tool that then wrote another style says nothing
        if tile.trim() == "1" && matches!(style.trim(), "0" | "") {
            return Some(Position::Tile);
        }
        match style.trim() {
            "0" => Some(Position::Center),
            "2" => Some(Position::Stretch),
            "6" => Some(Position::Fit),
            "10" => Some(Position::Fill),
            "22" => Some(Position::Span),
            _ => None,
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Position::Center => "center",
            Position::Tile => "tile",
            Position::Stretch => "stretch",
            Position::Fit => "fit",
            Position::Fill => "fill",
            Position::Span => "span",
        }
    }
}

/// Where the first tile of a tiled picture starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum TileAnchor {
    /// At the top-left of the virtual screen, so the pattern runs unbroken across monitors (the shell plan's reading, UNVERIFIED).
    #[default]
    VirtualScreen,
    /// At the top-left of each monitor.
    Monitor,
}

/// What to draw on one monitor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The bitmap: the monitor's size in physical pixels.
    pub target: Size,
    /// The part of the picture to use, in picture pixels (empty when no picture is drawn).
    pub src: Rect,
    /// Where it goes, in monitor pixels. Its size is the size the cropped part is scaled to.
    pub dst: Rect,
    /// The parts of the monitor the picture does not cover, to be filled with the background colour.
    pub bars: Vec<Rect>,
    /// `Some((x, y))` for a tiled picture: the whole picture repeats from this top-left corner (monitor pixels, in `-w+1 ..= 0` and
    /// `-h+1 ..= 0`), at its own size; `src` and `dst` then cover the whole picture and the whole monitor.
    pub tile: Option<(i32, i32)>,
}

impl Plan {
    /// Nothing but the background colour.
    pub fn solid(target: Size) -> Plan {
        let all = Rect::from_size(0, 0, target.w, target.h);
        Plan { target, src: Rect::default(), dst: Rect::default(), bars: if all.is_empty() { Vec::new() } else { vec![all] }, tile: None }
    }
    pub fn has_picture(&self) -> bool {
        !self.dst.is_empty() || self.tile.is_some()
    }
    /// The size the cropped source part is resampled to (`dst`'s size).
    pub fn scaled_size(&self) -> Size {
        self.dst.size()
    }
}

/// Integer `a * b / c` rounded half up (all positive).
fn mul_div_round(a: i64, b: i64, c: i64) -> i64 {
    (2 * a * b + c) / (2 * c)
}

/// The pieces of `outer` (monitor-local, starting at 0,0) that `inner` does not cover: top band, bottom band, then left and right of
/// the middle. Empty pieces are left out.
fn bars_around(outer: Size, inner: Rect) -> Vec<Rect> {
    let (w, h) = (outer.w as i32, outer.h as i32);
    let all = Rect::new(0, 0, w, h);
    if inner.is_empty() {
        return if all.is_empty() { Vec::new() } else { vec![all] };
    }
    [
        Rect::new(0, 0, w, inner.top),
        Rect::new(0, inner.bottom, w, h),
        Rect::new(0, inner.top, inner.left, inner.bottom),
        Rect::new(inner.right, inner.top, w, inner.bottom),
    ]
    .into_iter()
    .filter(|r| !r.is_empty())
    .collect()
}

/// The part of a `pw` x `ph` picture that covers a `tw` x `th` area when scaled by "fill" (cover): aspect kept, the overflow cut
/// away equally on both sides.
fn cover_crop(pw: i64, ph: i64, tw: i64, th: i64) -> Rect {
    if tw * ph >= th * pw {
        // the area is relatively wider: use the full width, cut height
        let sh = mul_div_round(th, pw, tw).clamp(1, ph);
        let y = (ph - sh) / 2;
        Rect::new(0, y as i32, pw as i32, (y + sh) as i32)
    } else {
        let sw = mul_div_round(tw, ph, th).clamp(1, pw);
        let x = (pw - sw) / 2;
        Rect::new(x as i32, 0, (x + sw) as i32, ph as i32)
    }
}

/// The plan for ONE monitor.
///
/// * `picture`: the decoded picture's size in pixels.
/// * `monitor`: the monitor's rectangle in virtual-screen coordinates, physical pixels.
/// * `virtual_screen`: the bounding rectangle of all monitors (it matters for span and, with [`TileAnchor::VirtualScreen`], tile).
pub fn plan(position: Position, picture: Size, monitor: Rect, virtual_screen: Rect, anchor: TileAnchor) -> Plan {
    let target = monitor.size();
    if picture.is_empty() || target.is_empty() {
        return Plan::solid(target);
    }
    let (pw, ph) = (picture.w as i64, picture.h as i64);
    let (mw, mh) = (target.w as i64, target.h as i64);
    let whole_src = Rect::new(0, 0, pw as i32, ph as i32);
    let whole_dst = Rect::new(0, 0, mw as i32, mh as i32);
    match position {
        Position::Stretch => Plan { target, src: whole_src, dst: whole_dst, bars: Vec::new(), tile: None },
        Position::Center => {
            let (ox, oy) = ((mw - pw).div_euclid(2), (mh - ph).div_euclid(2));
            // a picture larger than the monitor loses its overhang on both sides, a smaller one is surrounded by bars
            let (sx, dx, w) = if ox >= 0 { (0, ox, pw) } else { (-ox, 0, mw) };
            let (sy, dy, h) = if oy >= 0 { (0, oy, ph) } else { (-oy, 0, mh) };
            let src = Rect::new(sx as i32, sy as i32, (sx + w) as i32, (sy + h) as i32);
            let dst = Rect::new(dx as i32, dy as i32, (dx + w) as i32, (dy + h) as i32);
            Plan { target, src, dst, bars: bars_around(target, dst), tile: None }
        }
        Position::Fit => {
            let (dw, dh) = if mw * ph <= mh * pw {
                (mw, mul_div_round(ph, mw, pw).clamp(1, mh))
            } else {
                (mul_div_round(pw, mh, ph).clamp(1, mw), mh)
            };
            let (x, y) = ((mw - dw) / 2, (mh - dh) / 2);
            let dst = Rect::new(x as i32, y as i32, (x + dw) as i32, (y + dh) as i32);
            Plan { target, src: whole_src, dst, bars: bars_around(target, dst), tile: None }
        }
        Position::Fill => Plan { target, src: cover_crop(pw, ph, mw, mh), dst: whole_dst, bars: Vec::new(), tile: None },
        Position::Tile => {
            let origin = |anchor_at: i32, pic: i64| -> i32 {
                let r = (anchor_at as i64).rem_euclid(pic);
                if r == 0 { 0 } else { (r - pic) as i32 }
            };
            let tile = match anchor {
                TileAnchor::Monitor => (0, 0),
                TileAnchor::VirtualScreen => (origin(virtual_screen.left - monitor.left, pw), origin(virtual_screen.top - monitor.top, ph)),
            };
            Plan { target, src: whole_src, dst: whole_dst, bars: Vec::new(), tile: Some(tile) }
        }
        Position::Span => {
            let (vw, vh) = (virtual_screen.width() as i64, virtual_screen.height() as i64);
            if vw <= 0 || vh <= 0 || virtual_screen.intersect(&monitor) != monitor {
                // the monitor is not inside the virtual screen it was given: nothing sensible to cut, treat the monitor alone
                return plan(Position::Fill, picture, monitor, virtual_screen, anchor);
            }
            let crop = cover_crop(pw, ph, vw, vh);
            let (cw, ch) = (crop.width() as i64, crop.height() as i64);
            // each edge is rounded on its own, so neighbouring monitors meet without a gap or an overlap
            let edge = |rel: i32, total: i64, len: i64, start: i32| start + mul_div_round(rel as i64, len, total) as i32;
            let (rx0, rx1) = (monitor.left - virtual_screen.left, monitor.right - virtual_screen.left);
            let (ry0, ry1) = (monitor.top - virtual_screen.top, monitor.bottom - virtual_screen.top);
            let (x0, x1) = (edge(rx0, vw, cw, crop.left), edge(rx1, vw, cw, crop.left));
            let (y0, y1) = (edge(ry0, vh, ch, crop.top), edge(ry1, vh, ch, crop.top));
            let src = Rect::new(x0, y0, x1.max(x0 + 1), y1.max(y0 + 1));
            Plan { target, src, dst: whole_dst, bars: Vec::new(), tile: None }
        }
    }
}

/// The plans for every monitor of a set (same order), sharing the virtual screen.
pub fn plan_all(position: Position, picture: Size, monitors: &[Rect], anchor: TileAnchor) -> Vec<Plan> {
    let v = super::geom::virtual_screen(monitors);
    monitors.iter().map(|m| plan(position, picture, *m, v, anchor)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LANDSCAPE: Rect = Rect::new(0, 0, 1920, 1080);
    const PORTRAIT: Rect = Rect::new(-1440, -560, 0, 2000);
    const V: Rect = Rect::new(-1440, -560, 1920, 2000);

    fn one(position: Position, pw: u32, ph: u32, monitor: Rect) -> Plan {
        plan(position, Size::new(pw, ph), monitor, monitor, TileAnchor::VirtualScreen)
    }

    #[test]
    fn dwpos_numbers_and_registry_styles() {
        for (n, p) in [(0, Position::Center), (1, Position::Tile), (2, Position::Stretch), (3, Position::Fit), (4, Position::Fill), (5, Position::Span)] {
            assert_eq!(Position::from_dwpos(n), Some(p));
            assert_eq!(p.to_dwpos(), n);
        }
        assert_eq!(Position::from_dwpos(6), None);
        assert_eq!(Position::from_dwpos(-1), None);
        assert_eq!(Position::from_registry("10", "0"), Some(Position::Fill));
        assert_eq!(Position::from_registry("0", "0"), Some(Position::Center));
        assert_eq!(Position::from_registry("0", "1"), Some(Position::Tile));
        // a stale tile flag does not override a style that was written later
        assert_eq!(Position::from_registry("10", "1"), Some(Position::Fill));
        assert_eq!(Position::from_registry("6", "1"), Some(Position::Fit));
        assert_eq!(Position::from_registry("22", "1"), Some(Position::Span));
        assert_eq!(Position::from_registry("2", "1"), Some(Position::Stretch));
        assert_eq!(Position::from_registry("7", "1"), None);
        assert_eq!(Position::from_registry("2", "0"), Some(Position::Stretch));
        assert_eq!(Position::from_registry("6", "0"), Some(Position::Fit));
        assert_eq!(Position::from_registry("22", "0"), Some(Position::Span));
        assert_eq!(Position::from_registry("7", "0"), None);
        assert_eq!(Position::from_registry("", ""), None);
    }

    #[test]
    fn center_a_small_picture_leaves_four_bars() {
        let p = one(Position::Center, 800, 600, LANDSCAPE);
        assert_eq!(p.src, Rect::new(0, 0, 800, 600));
        assert_eq!(p.dst, Rect::new(560, 240, 1360, 840));
        assert_eq!(
            p.bars,
            vec![Rect::new(0, 0, 1920, 240), Rect::new(0, 840, 1920, 1080), Rect::new(0, 240, 560, 840), Rect::new(1360, 240, 1920, 840)]
        );
        assert_eq!(p.scaled_size(), Size::new(800, 600), "centre does not scale");
    }

    #[test]
    fn center_a_large_picture_is_cropped_equally() {
        let p = one(Position::Center, 3000, 2000, LANDSCAPE);
        assert_eq!(p.src, Rect::new(540, 460, 2460, 1540));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));
        assert!(p.bars.is_empty());
        // a picture wider but not taller: bars only above and below
        let q = one(Position::Center, 3000, 600, LANDSCAPE);
        assert_eq!(q.src, Rect::new(540, 0, 2460, 600));
        assert_eq!(q.dst, Rect::new(0, 240, 1920, 840));
        assert_eq!(q.bars, vec![Rect::new(0, 0, 1920, 240), Rect::new(0, 840, 1920, 1080)]);
    }

    #[test]
    fn center_with_an_odd_difference_puts_the_spare_pixel_right_and_below() {
        let p = one(Position::Center, 100, 100, Rect::new(0, 0, 101, 101));
        assert_eq!(p.dst, Rect::new(0, 0, 100, 100));
        assert_eq!(p.bars, vec![Rect::new(0, 100, 101, 101), Rect::new(100, 0, 101, 100)]);
    }

    #[test]
    fn stretch_uses_everything() {
        let p = one(Position::Stretch, 4000, 3000, LANDSCAPE);
        assert_eq!((p.src, p.dst), (Rect::new(0, 0, 4000, 3000), Rect::new(0, 0, 1920, 1080)));
        assert!(p.bars.is_empty() && p.tile.is_none());
    }

    #[test]
    fn fit_letterboxes_and_pillarboxes() {
        // 4:3 on 16:9: pillars
        let p = one(Position::Fit, 4000, 3000, LANDSCAPE);
        assert_eq!(p.src, Rect::new(0, 0, 4000, 3000));
        assert_eq!(p.dst, Rect::new(240, 0, 1680, 1080));
        assert_eq!(p.bars, vec![Rect::new(0, 0, 240, 1080), Rect::new(1680, 0, 1920, 1080)]);
        // 3:1 on 16:9: letterbox
        let q = one(Position::Fit, 3000, 1000, LANDSCAPE);
        assert_eq!(q.dst, Rect::new(0, 220, 1920, 860));
        assert_eq!(q.bars, vec![Rect::new(0, 0, 1920, 220), Rect::new(0, 860, 1920, 1080)]);
        // a picture smaller than the monitor is enlarged: 100x100 on 200x100
        let r = one(Position::Fit, 100, 100, Rect::new(0, 0, 200, 100));
        assert_eq!(r.dst, Rect::new(50, 0, 150, 100));
        // the same aspect: no bars
        let s = one(Position::Fit, 1920, 1080, LANDSCAPE);
        assert_eq!(s.dst, Rect::new(0, 0, 1920, 1080));
        assert!(s.bars.is_empty());
    }

    #[test]
    fn fill_crops_the_overflow() {
        let p = one(Position::Fill, 4000, 3000, LANDSCAPE);
        assert_eq!(p.src, Rect::new(0, 375, 4000, 2625));
        assert_eq!(p.dst, Rect::new(0, 0, 1920, 1080));
        // the portrait monitor takes a narrow middle slice (1687.5 rounds up to 1688)
        let q = one(Position::Fill, 4000, 3000, PORTRAIT);
        assert_eq!(q.src, Rect::new(1156, 0, 2844, 3000));
        assert_eq!(q.target, Size::new(1440, 2560));
        // same aspect: the whole picture
        let r = one(Position::Fill, 1920, 1080, LANDSCAPE);
        assert_eq!(r.src, Rect::new(0, 0, 1920, 1080));
    }

    #[test]
    fn tile_origin_follows_the_virtual_screen_or_the_monitor() {
        let pic = Size::new(100, 100);
        // landscape monitor: the virtual screen starts 1440 left and 560 above it, so the pattern's first corner is 40 left, 60 above
        let a = plan(Position::Tile, pic, LANDSCAPE, V, TileAnchor::VirtualScreen);
        assert_eq!(a.tile, Some((-40, -60)));
        assert_eq!(a.src, Rect::new(0, 0, 100, 100));
        assert_eq!(a.dst, Rect::new(0, 0, 1920, 1080));
        // the monitor at the virtual screen's corner starts a tile at its own corner
        let b = plan(Position::Tile, pic, PORTRAIT, V, TileAnchor::VirtualScreen);
        assert_eq!(b.tile, Some((0, 0)));
        // per-monitor anchoring
        let c = plan(Position::Tile, pic, LANDSCAPE, V, TileAnchor::Monitor);
        assert_eq!(c.tile, Some((0, 0)));
        // the origin is always in -w+1 ..= 0
        for left in [-1441, -1440, -7, 0, 99, 100, 101] {
            let m = Rect::new(left, 0, left + 500, 500);
            let v = Rect::new(-1440, 0, 1000, 500);
            let (ox, _) = plan(Position::Tile, pic, m, v, TileAnchor::VirtualScreen).tile.unwrap();
            assert!((-99..=0).contains(&ox), "{left}: {ox}");
            assert_eq!((m.left + ox - v.left).rem_euclid(100), 0, "tile edges line up with the virtual screen: {left}");
        }
    }

    #[test]
    fn span_cuts_each_monitor_out_of_one_picture() {
        // the virtual screen is 3360 x 2560; a picture of exactly twice that size maps at 2:1
        let pic = Size::new(6720, 5120);
        let land = plan(Position::Span, pic, LANDSCAPE, V, TileAnchor::VirtualScreen);
        assert_eq!(land.src, Rect::new(2880, 1120, 6720, 3280));
        assert_eq!(land.src.size(), Size::new(3840, 2160));
        assert_eq!(land.dst, Rect::new(0, 0, 1920, 1080));
        let port = plan(Position::Span, pic, PORTRAIT, V, TileAnchor::VirtualScreen);
        assert_eq!(port.src, Rect::new(0, 0, 2880, 5120));
        assert_eq!(port.dst, Rect::new(0, 0, 1440, 2560));
        // together they use the picture from x = 0 to 6720 with no gap and no overlap
        assert_eq!(port.src.right, land.src.left);
    }

    #[test]
    fn span_of_a_wide_picture_crops_the_middle_first() {
        let pic = Size::new(8000, 2560);
        let port = plan(Position::Span, pic, PORTRAIT, V, TileAnchor::VirtualScreen);
        let land = plan(Position::Span, pic, LANDSCAPE, V, TileAnchor::VirtualScreen);
        // 8000 x 2560 over 3360 x 2560: the middle 3360 columns (from 2320) are used at 1:1
        assert_eq!(port.src, Rect::new(2320, 0, 3760, 2560));
        assert_eq!(land.src, Rect::new(3760, 560, 5680, 1640));
    }

    #[test]
    fn span_on_a_single_monitor_is_fill() {
        let pic = Size::new(4000, 3000);
        let s = plan(Position::Span, pic, LANDSCAPE, LANDSCAPE, TileAnchor::VirtualScreen);
        let f = plan(Position::Fill, pic, LANDSCAPE, LANDSCAPE, TileAnchor::VirtualScreen);
        assert_eq!(s, f);
    }

    #[test]
    fn plan_all_shares_the_virtual_screen() {
        let ps = plan_all(Position::Span, Size::new(6720, 5120), &[LANDSCAPE, PORTRAIT], TileAnchor::VirtualScreen);
        assert_eq!(ps[0].src, Rect::new(2880, 1120, 6720, 3280));
        assert_eq!(ps[1].src, Rect::new(0, 0, 2880, 5120));
    }

    #[test]
    fn no_picture_or_no_monitor_is_a_solid_colour() {
        let p = one(Position::Fill, 0, 0, LANDSCAPE);
        assert!(!p.has_picture());
        assert_eq!(p.bars, vec![Rect::new(0, 0, 1920, 1080)]);
        let q = plan(Position::Fill, Size::new(10, 10), Rect::default(), V, TileAnchor::Monitor);
        assert!(q.bars.is_empty() && !q.has_picture());
        // a 1 x 1 picture is fine in every mode
        for pos in [Position::Center, Position::Tile, Position::Stretch, Position::Fit, Position::Fill, Position::Span] {
            let p = plan(pos, Size::new(1, 1), LANDSCAPE, V, TileAnchor::VirtualScreen);
            assert!(p.has_picture(), "{pos:?}");
        }
    }
}
