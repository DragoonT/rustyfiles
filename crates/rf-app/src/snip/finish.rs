//! What happens to a region once the user has taken it: cut it out of the captured pictures, make the three formats, put them on the
//! clipboard and, when the setting says so, write the PNG to `Pictures\Screenshots`. All of it runs on a thread of its own, after the
//! overlay is gone ([`run`]); the steps are in [`process`], over a clipboard trait, so that the tests run them against a fake clipboard
//! and a folder of their own and the real clipboard is never touched by a test.

use std::path::Path;
use std::sync::Arc;

use gpui_kit::RenderImage;
use rf_core::desktop::geom::Rect;
use rf_win::snip::clip::{ClipApi, SystemClipboard, write};
use rf_win::snip::encode::Formats;
use rf_win::snip::frame::{Part, crop_parts};
use rf_win::snip::save::{LocalTime, save_png};

use super::plan::Outcome;

/// One captured monitor as the overlay holds it: its place on the virtual screen and the picture, which the overlay draws and the cut
/// reads. The pixels live once, inside the picture (B G R A, top row first).
#[derive(Clone)]
pub struct Source {
    pub rect: Rect,
    pub width: u32,
    pub height: u32,
    pub image: Arc<RenderImage>,
}

impl Source {
    /// The captured monitor as the overlay holds it. The bytes move into the picture (no second copy of a screen). `None` when the
    /// frame's bytes are not its size.
    pub fn from_frame(rect: Rect, f: rf_win::snip::frame::Frame) -> Option<Source> {
        let (width, height) = (f.width, f.height);
        let buf = image::RgbaImage::from_raw(width, height, f.bgra)?;
        Some(Source { rect, width, height, image: Arc::new(RenderImage::new(vec![image::Frame::new(buf)])) })
    }

    fn part(&self) -> Part<'_> {
        // (a picture without bytes makes a part of the wrong size, which the cut refuses)
        Part { rect: self.rect, width: self.width, height: self.height, bgra: self.image.as_bytes(0).unwrap_or(&[]) }
    }
}

/// What [`process`] made: the notice's facts and the PNG the notice's `Kaydet` writes.
pub struct Made {
    pub outcome: Outcome,
    pub png: Option<Arc<Vec<u8>>>,
}

impl Made {
    /// A snip that came to nothing before there was a picture: the notice says "Alıntı alınamadı". `why` goes to the log, not the user.
    pub fn failed(why: &str) -> Made {
        Made { outcome: Outcome { size: None, copied: Err(why.to_string()), saved: None, has_png: false }, png: None }
    }
}

/// The steps. `save_dir`: `Some` writes the PNG there too (the setting that saves every snip).
pub fn process(parts: &[Part<'_>], region: Rect, save_dir: Option<&Path>, clip: &mut dyn ClipApi, now: &LocalTime) -> Made {
    let Some(frame) = crop_parts(parts, region) else {
        return Made { outcome: Outcome { size: None, copied: Err("the region could not be cut".into()), saved: None, has_png: false }, png: None };
    };
    let size = (frame.width, frame.height);
    let formats = Formats::of(&frame);
    drop(frame);
    let copied = write(clip, &formats);
    let png = (!formats.png.is_empty()).then(|| Arc::new(formats.png.clone()));
    let saved = match (save_dir, &png) {
        (Some(dir), Some(png)) => Some(save_png(dir, png, now).map_err(|e| e.to_string())),
        _ => None,
    };
    Made { outcome: Outcome { size: Some(size), copied, saved, has_png: png.is_some() }, png }
}

/// A clipboard that could not even be made: every step says why.
struct NoClipboard(String);

impl ClipApi for NoClipboard {
    fn open(&mut self) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn empty(&mut self) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn set(&mut self, _: rf_win::snip::clip::Format, _: &[u8]) -> Result<(), String> {
        Err(self.0.clone())
    }
    fn close(&mut self) {}
}

/// The real thing, for a thread of its own: the clipboard, the folder and the clock are the system's.
pub fn run(sources: &[Source], region: Rect, autosave: bool) -> Made {
    let parts: Vec<Part<'_>> = sources.iter().map(Source::part).collect();
    let dir = if autosave { rf_win::snip::save::screenshots_dir() } else { None };
    let now = rf_win::snip::save::local_time();
    match SystemClipboard::new() {
        Ok(mut clip) => process(&parts, region, dir.as_deref(), &mut clip, &now),
        Err(e) => process(&parts, region, dir.as_deref(), &mut NoClipboard(e), &now),
    }
}

/// `Kaydet`: writes the PNG to `Pictures\Screenshots` under the name for this moment. Runs on a thread of its own.
pub fn save_now(png: &[u8]) -> Result<std::path::PathBuf, String> {
    let dir = rf_win::snip::save::screenshots_dir().ok_or_else(|| "the Pictures folder could not be found".to_string())?;
    save_png(&dir, png, &rf_win::snip::save::local_time()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::snip::clip::{Format, Written};
    use rf_win::snip::frame::Frame;

    /// A fake clipboard that keeps what it is given.
    #[derive(Default)]
    struct Fake {
        stored: Vec<(Format, Vec<u8>)>,
        fail_open: bool,
    }
    impl ClipApi for Fake {
        fn open(&mut self) -> Result<(), String> {
            if self.fail_open { Err("busy".into()) } else { Ok(()) }
        }
        fn empty(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn set(&mut self, f: Format, b: &[u8]) -> Result<(), String> {
            self.stored.push((f, b.to_vec()));
            Ok(())
        }
        fn close(&mut self) {}
    }

    const NOW: LocalTime = LocalTime { year: 2026, month: 11, day: 13, hour: 3, minute: 4, second: 5 };

    /// A frame whose pixel at (x, y) says where it is.
    fn marked(w: u32, h: u32, tag: u8) -> Frame {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[x as u8, y as u8, tag, 255]);
            }
        }
        Frame { width: w, height: h, bgra: v }
    }

    fn two_monitors<'a>(a: &'a Frame, b: &'a Frame) -> Vec<Part<'a>> {
        vec![
            Part { rect: Rect::new(0, 0, 200, 100), width: 200, height: 100, bgra: &a.bgra },
            Part { rect: Rect::new(-120, -50, 0, 250), width: 120, height: 300, bgra: &b.bgra },
        ]
    }

    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("rf-snip-finish-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_source_made_from_a_captured_frame_hands_the_cut_the_same_bytes_in_the_same_order() {
        // Frame -> RenderImage (what the overlay draws) -> Source::part (what the cut reads): the bytes, their order and the size survive
        let f = marked(7, 5, 9);
        let want = f.bgra.clone();
        let src = Source::from_frame(Rect::new(-7, 10, 0, 15), f).expect("a frame of its own size");
        let part = src.part();
        assert_eq!((part.width, part.height, part.rect), (7, 5, Rect::new(-7, 10, 0, 15)));
        assert_eq!(part.bgra, &want[..], "B G R A, top row first, no padding, no swapped channels");
        // and the cut out of it, through the whole chain, has the pixel the frame had there
        let made = process(&[part], Rect::new(-5, 11, -2, 13), None, &mut Fake::default(), &NOW);
        assert_eq!(made.outcome.size, Some((3, 2)));
        let png = made.png.unwrap();
        let img = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap().to_rgb8();
        // frame pixel (2, 1) = [x, y, tag] in B G R order = r 9, g 1, b 2
        assert_eq!(img.get_pixel(0, 0).0, [9, 1, 2]);
        assert_eq!(img.get_pixel(2, 1).0, [9, 2, 4]);
    }

    #[test]
    fn a_frame_whose_bytes_are_not_its_size_makes_no_source() {
        let bad = Frame { width: 10, height: 10, bgra: vec![0; 12] };
        assert!(Source::from_frame(Rect::new(0, 0, 10, 10), bad).is_none());
    }

    #[test]
    fn a_snip_that_failed_before_a_picture_exists_says_so_and_offers_nothing() {
        use crate::snip::plan::{Headline, headline, offers_save};
        let m = Made::failed("BitBlt failed");
        assert_eq!(headline(&m.outcome), Headline::Failed);
        assert!(!offers_save(&m.outcome) && m.png.is_none());
    }

    #[test]
    fn a_region_across_two_monitors_reaches_the_clipboard_in_all_three_formats_and_decodes_to_the_pixels() {
        let (a, b) = (marked(200, 100, 1), marked(120, 300, 2));
        let parts = two_monitors(&a, &b);
        let region = Rect::new(-20, 0, 30, 40);
        let mut clip = Fake::default();
        let made = process(&parts, region, None, &mut clip, &NOW);
        assert_eq!(made.outcome.size, Some((50, 40)));
        assert!(matches!(made.outcome.copied, Ok(Written::All)));
        assert_eq!(made.outcome.saved, None);
        assert_eq!(clip.stored.iter().map(|(f, _)| *f).collect::<Vec<_>>(), [Format::Png, Format::DibV5, Format::Dib]);
        // the PNG on the clipboard is the region: the left 20 columns are the left monitor's, the rest the primary's
        let png = &clip.stored[0].1;
        let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (50, 40));
        assert_eq!(img.get_pixel(0, 0).0, [2, 50, 100], "r g b of the left monitor's pixel (100, 50)");
        assert_eq!(img.get_pixel(20, 0).0, [1, 0, 0], "the primary's first column");
        assert_eq!(img.get_pixel(49, 39).0, [1, 39, 29]);
        // the old DIB has the same size in its header, and the kept PNG is the same bytes
        assert_eq!(i32::from_le_bytes(clip.stored[2].1[4..8].try_into().unwrap()), 50);
        assert_eq!(made.png.as_deref(), Some(png));
    }

    #[test]
    fn with_the_setting_on_the_png_is_written_too_and_with_it_off_nothing_is_written() {
        let (a, b) = (marked(200, 100, 1), marked(120, 300, 2));
        let parts = two_monitors(&a, &b);
        let s = Scratch::new("auto");
        let dir = s.0.join("Screenshots");
        let made = process(&parts, Rect::new(10, 10, 60, 40), Some(&dir), &mut Fake::default(), &NOW);
        let path = made.outcome.saved.clone().unwrap().unwrap();
        assert_eq!(path, dir.join("Ekran alıntısı 2026-11-13 030405.png"));
        assert_eq!(std::fs::read(&path).unwrap(), **made.png.as_ref().unwrap(), "the file is the PNG of the clipboard");
        // off: no folder is even made
        let s2 = Scratch::new("off");
        let made = process(&parts, Rect::new(10, 10, 60, 40), None, &mut Fake::default(), &NOW);
        assert_eq!(made.outcome.saved, None);
        assert!(!s2.0.exists());
    }

    #[test]
    fn a_clipboard_that_is_busy_still_leaves_the_file_and_the_kept_png() {
        let (a, b) = (marked(200, 100, 1), marked(120, 300, 2));
        let parts = two_monitors(&a, &b);
        let s = Scratch::new("busy");
        let mut clip = Fake { fail_open: true, ..Default::default() };
        let made = process(&parts, Rect::new(10, 10, 60, 40), Some(&s.0), &mut clip, &NOW);
        assert!(made.outcome.copied.is_err());
        assert!(matches!(made.outcome.saved, Some(Ok(_))));
        assert!(made.png.is_some() && made.outcome.has_png, "Kaydet can still be offered");
        assert!(clip.stored.is_empty());
    }

    #[test]
    fn a_failed_save_is_reported_and_does_not_undo_the_copy() {
        let (a, b) = (marked(200, 100, 1), marked(120, 300, 2));
        let parts = two_monitors(&a, &b);
        let s = Scratch::new("blocked");
        std::fs::create_dir_all(&s.0).unwrap();
        let file = s.0.join("not-a-folder");
        std::fs::write(&file, b"x").unwrap();
        let made = process(&parts, Rect::new(10, 10, 60, 40), Some(&file), &mut Fake::default(), &NOW);
        assert!(matches!(made.outcome.copied, Ok(Written::All)));
        assert!(matches!(made.outcome.saved, Some(Err(_))));
        assert_eq!(std::fs::read(&file).unwrap(), b"x");
    }

    #[test]
    fn a_region_that_cannot_be_cut_touches_neither_the_clipboard_nor_the_disk() {
        let (a, b) = (marked(200, 100, 1), marked(120, 300, 2));
        let parts = two_monitors(&a, &b);
        let s = Scratch::new("nothing");
        let mut clip = Fake::default();
        let made = process(&parts, Rect::new(5, 5, 5, 50), Some(&s.0), &mut clip, &NOW);
        assert_eq!(made.outcome.size, None);
        assert!(made.png.is_none() && clip.stored.is_empty() && !s.0.exists());
        // a part whose bytes are not its size
        let short = [Part { rect: Rect::new(0, 0, 10, 10), width: 10, height: 10, bgra: &[0; 12] }];
        assert_eq!(process(&short, Rect::new(0, 0, 5, 5), None, &mut clip, &NOW).outcome.size, None);
    }

    #[test]
    fn a_clipboard_that_could_not_be_made_says_why_on_every_step() {
        let mut n = NoClipboard("no window".into());
        assert_eq!(n.open(), Err("no window".into()));
        let f = Frame { width: 1, height: 1, bgra: vec![1, 2, 3, 255] };
        let e = write(&mut n, &Formats::of(&f)).unwrap_err();
        assert!(e.contains("no window"), "{e}");
    }
}
