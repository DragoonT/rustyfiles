//! The preview pane's picture of an ordinary picture file (JPEG, PNG, …), made inside this process by the
//! Windows Imaging Component and not by the shell.
//!
//! Why: `thumbs::get_image` goes through the shell (`IShellItemImageFactory`), which is several hand-offs between
//! threads and processes (the thumbnail cache, the handler, COM calls that wait for another apartment). Each hand-off
//! needs the CPU for a moment, so on a machine whose cores are all busy (a build, an IDE, the program that asked for the
//! Open dialog) a picture that takes 25 ms on an idle machine took 250 ms to 1 s, and seconds when the list's own
//! thumbnails were being made through the same machinery at the same time. WIC decodes the file where the call is
//! made: no other process, no cache, no lock shared with the list's workers. The JPEG decoder also scales while it
//! decodes, so a 48 megapixel photo is not turned into a 48 megapixel bitmap first.
//!
//! Only formats WIC reads with its own codecs are done here; every other file (videos, documents, RAW, HEIC, …) and
//! every file this cannot read goes the shell's way, as before.

use std::path::Path;

use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICBitmapTransformFlipHorizontal, WICBitmapTransformFlipVertical,
    WICBitmapTransformOptions, WICBitmapTransformRotate0, WICBitmapTransformRotate180, WICBitmapTransformRotate270,
    WICBitmapTransformRotate90, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::core::{Interface, PCWSTR, w};

use crate::shell::ensure_com;
use crate::thumbs::Bgra;
use crate::wide::wide;

/// Extensions of the files WIC reads with the codecs every Windows has (WebP's is an optional package: when it is
/// missing the decode fails and the shell is asked instead).
pub fn handles(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "png" | "bmp" | "dib" | "gif" | "tif" | "tiff" | "webp")
}

/// How to turn a picture with this EXIF orientation (1 to 8) upright: WIC's flip and rotate options.
fn upright(orientation: u16) -> WICBitmapTransformOptions {
    match orientation {
        2 => WICBitmapTransformFlipHorizontal,
        3 => WICBitmapTransformRotate180,
        4 => WICBitmapTransformFlipVertical,
        5 => WICBitmapTransformOptions(WICBitmapTransformRotate270.0 | WICBitmapTransformFlipHorizontal.0),
        6 => WICBitmapTransformRotate90,
        7 => WICBitmapTransformOptions(WICBitmapTransformRotate90.0 | WICBitmapTransformFlipHorizontal.0),
        8 => WICBitmapTransformRotate270,
        _ => WICBitmapTransformRotate0,
    }
}

/// The size a picture of `w` x `h` is made at: its long side `size` at most, never enlarged, the same proportions.
fn fitted(w: u32, h: u32, size: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= size || long == 0 {
        return (w.max(1), h.max(1));
    }
    let s = f64::from(size) / f64::from(long);
    (((f64::from(w) * s).round() as u32).max(1), ((f64::from(h) * s).round() as u32).max(1))
}

/// The picture at `size` pixels on its long side (turned upright by its EXIF orientation), straight-alpha BGRA.
/// Blocking, COM: for a worker thread, never the UI thread.
pub fn decode(path: &Path, size: u32) -> anyhow::Result<Bgra> {
    ensure_com();
    let name = wide(path);
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory.CreateDecoderFromFilename(PCWSTR(name.as_ptr()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)?;
        let frame = decoder.GetFrame(0)?;
        let (mut fw, mut fh) = (0u32, 0u32);
        frame.GetSize(&mut fw, &mut fh)?;
        anyhow::ensure!(fw > 0 && fh > 0, "empty picture");
        // the orientation tag is only there for the formats that have one (JPEG, TIFF); no tag = upright
        let orientation = frame
            .GetMetadataQueryReader()
            .ok()
            .and_then(|r| {
                let mut v = PROPVARIANT::default();
                r.GetMetadataByName(w!("System.Photo.Orientation"), &mut v).ok()?;
                u16::try_from(&v).ok()
            })
            .filter(|o| (1..=8).contains(o))
            .unwrap_or(1);
        // fit the picture as it is shown (a turned one swaps its sides), then scale the stored one to match
        let (dw, dh) = if orientation >= 5 { (fh, fw) } else { (fw, fh) };
        let (sw, sh) = fitted(dw, dh, size);
        let (tw, th) = if orientation >= 5 { (sh, sw) } else { (sw, sh) };
        let mut src: IWICBitmapSource = frame.cast()?;
        if (tw, th) != (fw, fh) {
            let scaler = factory.CreateBitmapScaler()?;
            scaler.Initialize(&src, tw, th, WICBitmapInterpolationModeFant)?;
            src = scaler.cast()?;
        }
        if orientation != 1 {
            let rotator = factory.CreateBitmapFlipRotator()?;
            rotator.Initialize(&src, upright(orientation))?;
            src = rotator.cast()?;
        }
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(&src, &GUID_WICPixelFormat32bppBGRA, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)?;
        let stride = sw * 4;
        let mut data = vec![0u8; stride as usize * sh as usize];
        conv.CopyPixels(std::ptr::null(), stride, &mut data)?;
        Ok(Bgra { width: sw, height: sh, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A JPEG of `w` x `h` whose four corners are told apart by colour, with the EXIF orientation `o` written in
    /// (no tag at all for 0).
    fn corner_jpeg(path: &Path, w: u32, h: u32, o: u16) {
        use image::codecs::jpeg::JpegEncoder;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let (left, top) = (x < w / 2, y < h / 2);
            match (left, top) {
                (true, true) => image::Rgb([230, 20, 20]),
                (false, true) => image::Rgb([20, 230, 20]),
                (true, false) => image::Rgb([20, 20, 230]),
                (false, false) => image::Rgb([230, 230, 20]),
            }
        });
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 95).encode_image(&img).unwrap();
        if o != 0 {
            // SOI, then an APP1 "Exif" segment with one IFD entry: Orientation (0x0112, SHORT, 1 value)
            let mut seg = b"Exif\0\0MM\0\x2a\0\0\0\x08".to_vec();
            seg.extend_from_slice(&[0, 1, 0x01, 0x12, 0, 3, 0, 0, 0, 1, (o >> 8) as u8, o as u8, 0, 0, 0, 0, 0, 0]);
            let mut out = vec![0xFF, 0xD8, 0xFF, 0xE1];
            out.extend_from_slice(&((seg.len() + 2) as u16).to_be_bytes());
            out.extend_from_slice(&seg);
            out.extend_from_slice(&jpg[2..]);
            jpg = out;
        }
        std::fs::write(path, jpg).unwrap();
    }

    fn at(b: &Bgra, x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * b.width + x) * 4) as usize;
        (b.data[i + 2], b.data[i + 1], b.data[i])
    }

    /// The same upright picture the Quick Look decoder (`look::decode`, which applies the tag with the image crate)
    /// makes, for every orientation: the corners land in the same places.
    #[test]
    fn every_orientation_comes_out_as_quick_look_has_it() {
        let dir = std::env::temp_dir().join(format!("rf-pane-pic-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for o in 1..=8u16 {
            let p = dir.join(format!("o{o}.jpg"));
            corner_jpeg(&p, 96, 64, o);
            let ours = decode(&p, 1000).unwrap();
            let theirs = crate::look::decode::decode_full(&p, 1000).unwrap().frames.remove(0).0;
            assert_eq!((ours.width, ours.height), (theirs.width, theirs.height), "orientation {o}: size");
            // (both really read the tag: a picture turned on its side has its sides swapped, a turned-round one its corners)
            assert_eq!((ours.width, ours.height), if o >= 5 { (64, 96) } else { (96, 64) }, "orientation {o}: sides");
            if o == 3 {
                assert!(at(&ours, 5, 5).0 > 200 && at(&ours, 5, 5).1 > 200, "turned round: the yellow corner is at the top left");
            }
            let (w, h) = (ours.width, ours.height);
            for (x, y) in [(5, 5), (w - 6, 5), (5, h - 6), (w - 6, h - 6)] {
                let (a, b) = (at(&ours, x, y), at(&theirs, x, y));
                let close = |p: u8, q: u8| p.abs_diff(q) < 40;
                assert!(close(a.0, b.0) && close(a.1, b.1) && close(a.2, b.2), "orientation {o} at ({x},{y}): ours {a:?}, Quick Look {b:?}");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn long_side_is_the_size_asked_and_small_ones_are_not_enlarged() {
        let dir = std::env::temp_dir().join(format!("rf-pane-pic-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let big = dir.join("big.jpg");
        corner_jpeg(&big, 640, 360, 0);
        let b = decode(&big, 160).unwrap();
        assert_eq!((b.width, b.height), (160, 90));
        let small = dir.join("small.jpg");
        corner_jpeg(&small, 64, 48, 0);
        let s = decode(&small, 384).unwrap();
        assert_eq!((s.width, s.height), (64, 48));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A PNG's transparency comes out as straight alpha (what GPUI draws), not premultiplied.
    #[test]
    fn png_alpha_is_straight() {
        let dir = std::env::temp_dir().join(format!("rf-pane-pic-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 100, 50, 128])).save(&p).unwrap();
        let b = decode(&p, 64).unwrap();
        let i = 0usize;
        assert_eq!((b.data[i + 2], b.data[i + 1], b.data[i], b.data[i + 3]), (200, 100, 50, 128));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_wic_cannot_read_is_an_error_and_the_list_of_formats_is_short() {
        let dir = std::env::temp_dir().join(format!("rf-pane-pic-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("not-a-picture.jpg");
        std::fs::write(&p, b"this is text").unwrap();
        assert!(decode(&p, 64).is_err());
        assert!(handles(Path::new("a.JPG")) && handles(Path::new("a.png")));
        assert!(!handles(Path::new("a.mp4")) && !handles(Path::new("a.pdf")) && !handles(Path::new("a.heic")) && !handles(Path::new("noext")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn fitting() {
        assert_eq!(fitted(1920, 1080, 384), (384, 216));
        assert_eq!(fitted(1080, 1920, 384), (216, 384));
        assert_eq!(fitted(100, 50, 384), (100, 50));
        assert_eq!(fitted(8000, 1, 384), (384, 1));
    }
}
