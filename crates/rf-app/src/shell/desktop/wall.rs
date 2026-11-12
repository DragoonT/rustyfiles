//! The wallpaper under the icons: the user's own picture, placed for the rectangle the canvas is shown in (the model's placement maths
//! and decoder, `rf_win::desktop::render`), as ONE bitmap of exactly that size. Nothing is drawn per frame: the view uploads the
//! bitmap once and asks for a new one when the shown size changes (after the window has stopped resizing) or when the wallpaper
//! setting changes.
//!
//! Placement is computed for the SHOWN rectangle, as the shell plan asks ("placed for the window's rectangle"). Fill, fit, stretch and
//! span therefore look as they will on the monitor, only smaller; center and tile show the picture at its real pixel size inside the
//! smaller rectangle, so they look different from the monitor when the canvas is scaled down. The preview says so in its status line
//! (the scale is there).

use std::path::PathBuf;
use std::sync::Arc;

use rf_core::desktop::geom::{Rect, Rgb, Size};
use rf_core::desktop::placement::TileAnchor;
use rf_core::desktop::wallpaper::{MonitorWallpaper, Source, Wallpaper};
use rf_win::desktop::render::{ImageDecoder, RenderOptions, render_all};
use rf_win::desktop::wallcache::DiskCache;
use rf_win::desktop::wallpaper::DiskStamper;
use rf_win::thumbs::Bgra;

/// How many bitmaps the preview keeps on disk (one per size it has been shown at).
pub const CACHE_ENTRIES: usize = 6;

/// The wallpaper as it applies to ONE monitor shown in a rectangle of `size` pixels: that monitor's picture alone, in a rectangle that
/// starts at (0, 0).
pub fn for_canvas(wp: &Wallpaper, canvas: usize, size: Size) -> Wallpaper {
    let picture = wp.pictures().get(canvas).cloned().flatten();
    let device = wp.monitors.get(canvas).map(|m| m.device.clone()).unwrap_or_default();
    Wallpaper {
        // the picture is already resolved above (a virtual desktop's own value included), so the per-desktop logic must not run again
        source: Source::DesktopWallpaperCom,
        monitors: vec![MonitorWallpaper { device, rect: Rect::from_size(0, 0, size.w, size.h), picture }],
        per_desktop: Vec::new(),
        ..wp.clone()
    }
}

pub struct Rendered {
    pub bitmap: Arc<Bgra>,
    pub background: Rgb,
    /// Why there is no picture in it, when there is none (missing file, undecodable).
    pub note: Option<String>,
}

/// Blocking: decodes (or reads the disk cache). Run it off the UI thread.
pub fn render(wp: &Wallpaper, canvas: usize, size: Size, cache_dir: Option<PathBuf>) -> Rendered {
    let one = for_canvas(wp, canvas, size);
    let cache = cache_dir.map(|d| DiskCache::new(d, CACHE_ENTRIES));
    let mut out = render_all(&one, &RenderOptions { anchor: TileAnchor::Monitor, cache: cache.as_ref() }, &ImageDecoder, &DiskStamper);
    let m = out.pop().expect("one monitor, one bitmap");
    Rendered { bitmap: m.bitmap, background: one.background, note: m.note }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::placement::Position;
    use rf_core::desktop::wallpaper::BackgroundKind;

    fn wp(pictures: [Option<&str>; 2]) -> Wallpaper {
        Wallpaper {
            source: Source::Registry,
            monitors: vec![
                MonitorWallpaper { device: "A".into(), rect: Rect::new(0, 0, 2560, 1440), picture: pictures[0].map(PathBuf::from) },
                MonitorWallpaper { device: "B".into(), rect: Rect::new(2560, -618, 4000, 1942), picture: pictures[1].map(PathBuf::from) },
            ],
            position: Position::Fit,
            position_assumed: false,
            background: Rgb(195, 0, 82),
            kind: BackgroundKind::Picture,
            slideshow: None,
            per_desktop: Vec::new(),
        }
    }

    #[test]
    fn the_canvas_gets_its_own_monitors_picture_in_a_rectangle_of_the_shown_size() {
        let w = wp([Some(r"C:\a.png"), Some(r"C:\b.png")]);
        let one = for_canvas(&w, 1, Size::new(720, 1280));
        assert_eq!(one.monitors.len(), 1);
        assert_eq!(one.monitors[0].picture, Some(PathBuf::from(r"C:\b.png")));
        assert_eq!(one.monitors[0].rect, Rect::new(0, 0, 720, 1280), "no negative origin: the canvas is its own world");
        assert_eq!(one.monitors[0].device, "B");
        assert_eq!((one.position, one.background), (Position::Fit, Rgb(195, 0, 82)), "mode and colour are the system's");
        assert_eq!(for_canvas(&w, 0, Size::new(10, 10)).monitors[0].picture, Some(PathBuf::from(r"C:\a.png")));
    }

    #[test]
    fn no_picture_and_solid_colour_stay_solid() {
        assert_eq!(for_canvas(&wp([None, None]), 0, Size::new(10, 10)).monitors[0].picture, None);
        let mut w = wp([Some(r"C:\a.png"), None]);
        w.kind = BackgroundKind::SolidColor;
        let one = for_canvas(&w, 0, Size::new(10, 10));
        assert_eq!(one.pictures(), vec![None], "a solid colour shows no picture");
        // a canvas index past the monitors gives an empty picture, not a panic
        assert_eq!(for_canvas(&w, 9, Size::new(10, 10)).monitors[0].picture, None);
    }

    #[test]
    fn a_virtual_desktops_own_picture_is_resolved_once_and_not_again() {
        use rf_core::desktop::wallpaper::DesktopWallpaperValue;
        let mut w = wp([Some(r"C:\transcoded"), Some(r"C:\transcoded")]);
        w.per_desktop = vec![DesktopWallpaperValue { desktop: "{x}".into(), current: true, picture: Some(PathBuf::from(r"C:\original.jpg")) }];
        let one = for_canvas(&w, 0, Size::new(10, 10));
        assert_eq!(one.monitors[0].picture, Some(PathBuf::from(r"C:\original.jpg")), "the original, not the transcoded copy");
        assert!(one.per_desktop.is_empty());
        assert_eq!(one.pictures(), vec![Some(PathBuf::from(r"C:\original.jpg"))]);
    }

    #[test]
    fn rendering_a_missing_picture_gives_the_colour_and_says_why() {
        let w = wp([Some(r"C:\definitely\not\there.png"), None]);
        let r = render(&w, 0, Size::new(8, 6), None);
        assert_eq!((r.bitmap.width, r.bitmap.height), (8, 6));
        assert_eq!(&r.bitmap.data[..4], &[82, 0, 195, 255], "BGRA of the background colour");
        assert!(r.note.is_some());
        assert_eq!(r.background, Rgb(195, 0, 82));
    }

    #[test]
    fn rendering_a_real_picture_fills_the_shown_rectangle() {
        let dir = std::env::temp_dir().join(format!("rf-deskwall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pic = dir.join("red.png");
        image::RgbaImage::from_pixel(40, 20, image::Rgba([255, 0, 0, 255])).save_with_format(&pic, image::ImageFormat::Png).unwrap();
        let mut w = wp([None, None]);
        w.monitors[0].picture = Some(pic.clone());
        w.position = Position::Stretch;
        let cache = dir.join("cache");
        let r = render(&w, 0, Size::new(16, 12), Some(cache.clone()));
        assert_eq!((r.bitmap.width, r.bitmap.height), (16, 12));
        assert_eq!(&r.bitmap.data[..4], &[0, 0, 255, 255], "red, as BGRA");
        assert!(r.note.is_none());
        // the second time it comes from the disk cache
        let again = render(&w, 0, Size::new(16, 12), Some(cache));
        assert_eq!(again.bitmap.data, r.bitmap.data);
        let _ = std::fs::remove_dir_all(dir);
    }
}
