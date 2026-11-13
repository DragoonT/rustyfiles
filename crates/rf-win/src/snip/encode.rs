//! The bytes of a snip in the picture formats other programs read from the clipboard and from a file, pure: `CF_DIB`, `CF_DIBV5` and
//! PNG, each made from a [`Frame`] and checked in the tests by decoding it again.
//!
//! WHICH FORMAT FOR WHOM
//! * `CF_DIB` is what every program that pastes a picture understands (Paint, Word, mail clients, older tools), so it is made the way
//!   the oldest readers want it: a 40-byte header, 24 bits per pixel, no compression, rows bottom-up and padded to four bytes. A screen
//!   snip is opaque, and a 32-bit `BI_RGB` picture with an alpha byte is read by some programs as "all transparent".
//! * `CF_DIBV5` carries the same picture as 32 bits per pixel with explicit channel masks and the sRGB colour space, for programs that
//!   ask for it first (Windows itself, browsers, graphics programs); alpha is 255 everywhere.
//! * `PNG` (the registered clipboard format of that name) is what Discord, browsers, Slack and Office paste; also the file `Kaydet` writes.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};

use super::frame::Frame;

/// `BI_RGB`.
const BI_RGB: u32 = 0;
/// `BI_BITFIELDS`.
const BI_BITFIELDS: u32 = 3;
/// `LCS_sRGB`: the four letters "sRGB" read as a little-endian number.
const LCS_SRGB: u32 = 0x7352_4742;
/// `LCS_GM_IMAGES`: the rendering intent for photographs and screen pictures (saturation is not wanted, colorimetry is not either).
const LCS_GM_IMAGES: u32 = 4;

/// A pixel per metre of the screen's usual 96 dpi (3780): what Paint writes for a picture of the screen.
const PELS_PER_METRE: i32 = 3780;

fn put_u16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put_u32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

fn put_i32(v: &mut Vec<u8>, x: i32) {
    v.extend_from_slice(&x.to_le_bytes());
}

/// The first 40 bytes both headers start with.
fn info_header(v: &mut Vec<u8>, size: u32, f: &Frame, bits: u16, compression: u32, image_bytes: u32) {
    put_u32(v, size);
    put_i32(v, f.width as i32);
    // (positive: the rows are stored bottom-up, as every reader of a clipboard DIB expects)
    put_i32(v, f.height as i32);
    put_u16(v, 1);
    put_u16(v, bits);
    put_u32(v, compression);
    put_u32(v, image_bytes);
    put_i32(v, PELS_PER_METRE);
    put_i32(v, PELS_PER_METRE);
    put_u32(v, 0);
    put_u32(v, 0);
}

/// `CF_DIB`: BITMAPINFOHEADER and the pixels, 24 bits, bottom-up. Empty for an inconsistent frame.
pub fn dib(f: &Frame) -> Vec<u8> {
    if !f.is_consistent() {
        return Vec::new();
    }
    let (w, h) = (f.width as usize, f.height as usize);
    let stride = (w * 3).div_ceil(4) * 4;
    let mut v = Vec::with_capacity(40 + stride * h);
    info_header(&mut v, 40, f, 24, BI_RGB, (stride * h) as u32);
    for y in (0..h).rev() {
        let row = &f.bgra[y * w * 4..(y + 1) * w * 4];
        for px in row.as_chunks::<4>().0 {
            v.extend_from_slice(&px[..3]);
        }
        v.resize(v.len() + (stride - w * 3), 0);
    }
    v
}

/// `CF_DIBV5`: BITMAPV5HEADER and the pixels, 32 bits with masks, bottom-up. Empty for an inconsistent frame.
pub fn dibv5(f: &Frame) -> Vec<u8> {
    if !f.is_consistent() {
        return Vec::new();
    }
    let (w, h) = (f.width as usize, f.height as usize);
    let mut v = Vec::with_capacity(124 + w * h * 4);
    info_header(&mut v, 124, f, 32, BI_BITFIELDS, (w * h * 4) as u32);
    put_u32(&mut v, 0x00FF_0000); // red
    put_u32(&mut v, 0x0000_FF00); // green
    put_u32(&mut v, 0x0000_00FF); // blue
    put_u32(&mut v, 0xFF00_0000); // alpha
    put_u32(&mut v, LCS_SRGB);
    v.extend_from_slice(&[0u8; 36]); // the colour space's end points: unused for sRGB
    put_u32(&mut v, 0); // gamma red, green, blue: unused for sRGB
    put_u32(&mut v, 0);
    put_u32(&mut v, 0);
    put_u32(&mut v, LCS_GM_IMAGES);
    put_u32(&mut v, 0); // profile data
    put_u32(&mut v, 0); // profile size
    put_u32(&mut v, 0); // reserved
    debug_assert_eq!(v.len(), 124);
    for y in (0..h).rev() {
        // (the frame's BGRA is the masks' layout read as little-endian 0xAARRGGBB: a copy)
        v.extend_from_slice(&f.bgra[y * w * 4..(y + 1) * w * 4]);
    }
    v
}

/// A PNG of the frame, RGB, quick rather than small: a snip is one picture made while the user waits, and the fast level of the
/// encoder is several times quicker than its default on a screenful for a file that is a little larger. Empty for an inconsistent frame.
pub fn png(f: &Frame) -> Vec<u8> {
    if !f.is_consistent() {
        return Vec::new();
    }
    let mut rgb = Vec::with_capacity(f.bgra.len() / 4 * 3);
    for px in f.bgra.as_chunks::<4>().0 {
        rgb.extend_from_slice(&[px[2], px[1], px[0]]);
    }
    let mut out = Cursor::new(Vec::with_capacity(rgb.len() / 3));
    let enc = PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Sub);
    if enc.write_image(&rgb, f.width, f.height, ExtendedColorType::Rgb8).is_err() {
        return Vec::new();
    }
    out.into_inner()
}

/// The three formats of one snip.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Formats {
    pub dib: Vec<u8>,
    pub dibv5: Vec<u8>,
    pub png: Vec<u8>,
}

impl Formats {
    pub fn of(f: &Frame) -> Formats {
        Formats { dib: dib(f), dibv5: dibv5(f), png: png(f) }
    }

    /// All three were made.
    pub fn is_complete(&self) -> bool {
        !self.dib.is_empty() && !self.dibv5.is_empty() && !self.png.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with a different colour at every pixel (so a swapped channel or a flipped row shows), alpha 255.
    fn sample(w: u32, h: u32) -> Frame {
        let mut v = Vec::new();
        for y in 0..h {
            for x in 0..w {
                v.extend_from_slice(&[(x * 7 + 1) as u8, (y * 13 + 2) as u8, (x * 3 + y * 5 + 3) as u8, 255]);
            }
        }
        Frame { width: w, height: h, bgra: v }
    }

    fn u16_at(b: &[u8], at: usize) -> u16 {
        u16::from_le_bytes([b[at], b[at + 1]])
    }
    fn u32_at(b: &[u8], at: usize) -> u32 {
        u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
    }
    fn i32_at(b: &[u8], at: usize) -> i32 {
        u32_at(b, at) as i32
    }

    /// A DIB as a BMP file (the 14-byte file header in front), which any BMP reader decodes: the independent check of the bytes.
    fn as_bmp_file(dib: &[u8], pixel_offset: usize) -> Vec<u8> {
        let mut v = b"BM".to_vec();
        v.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
        v.extend_from_slice(&[0, 0, 0, 0]);
        v.extend_from_slice(&((14 + pixel_offset) as u32).to_le_bytes());
        v.extend_from_slice(dib);
        v
    }

    /// The rgb a decoder reports for the frame's pixel, to compare with the frame.
    fn rgb_of(f: &Frame, x: u32, y: u32) -> [u8; 3] {
        let p = f.pixel(x, y);
        [p[2], p[1], p[0]]
    }

    #[test]
    fn the_dib_has_the_old_header_24_bits_and_rows_padded_to_four_bytes() {
        let f = sample(5, 3);
        let d = dib(&f);
        assert_eq!(u32_at(&d, 0), 40);
        assert_eq!((i32_at(&d, 4), i32_at(&d, 8)), (5, 3), "positive height: bottom-up");
        assert_eq!((u16_at(&d, 12), u16_at(&d, 14)), (1, 24));
        assert_eq!(u32_at(&d, 16), 0, "BI_RGB");
        let stride = 16; // 5 * 3 = 15, padded to 16
        assert_eq!(u32_at(&d, 20) as usize, stride * 3);
        assert_eq!(d.len(), 40 + stride * 3);
        // the first stored row is the picture's LAST row, in B G R order
        assert_eq!(&d[40..43], &f.pixel(0, 2)[..3]);
        assert_eq!(d[40 + 15], 0, "the padding byte");
        assert_eq!(&d[40 + stride..40 + stride + 3], &f.pixel(0, 1)[..3]);
    }

    #[test]
    fn the_dib_decodes_to_the_same_picture() {
        for (w, h) in [(1, 1), (2, 2), (5, 3), (17, 9), (64, 48)] {
            let f = sample(w, h);
            let bmp = as_bmp_file(&dib(&f), 40);
            let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).unwrap().to_rgb8();
            assert_eq!((img.width(), img.height()), (w, h));
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(img.get_pixel(x, y).0, rgb_of(&f, x, y), "{w}x{h} at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn the_dibv5_has_the_v5_header_the_masks_and_the_srgb_space() {
        let f = sample(4, 2);
        let d = dibv5(&f);
        assert_eq!(u32_at(&d, 0), 124);
        assert_eq!((i32_at(&d, 4), i32_at(&d, 8)), (4, 2));
        assert_eq!((u16_at(&d, 12), u16_at(&d, 14)), (1, 32));
        assert_eq!(u32_at(&d, 16), 3, "BI_BITFIELDS");
        assert_eq!(u32_at(&d, 20), 4 * 2 * 4);
        assert_eq!((u32_at(&d, 40), u32_at(&d, 44), u32_at(&d, 48), u32_at(&d, 52)), (0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0xFF00_0000));
        assert_eq!(&d[56..60], b"BGRs", "LCS_sRGB, little-endian");
        assert_eq!(u32_at(&d, 108), 4);
        assert_eq!(d.len(), 124 + 4 * 2 * 4);
        // bottom-up, BGRA as it is
        assert_eq!(&d[124..128], &f.pixel(0, 1));
        assert_eq!(&d[124 + 16..124 + 20], &f.pixel(0, 0));
    }

    #[test]
    fn the_dibv5_decodes_to_the_same_picture() {
        for (w, h) in [(1, 1), (3, 2), (17, 9), (64, 48)] {
            let f = sample(w, h);
            let bmp = as_bmp_file(&dibv5(&f), 124);
            let img = image::load_from_memory_with_format(&bmp, image::ImageFormat::Bmp).unwrap().to_rgba8();
            assert_eq!((img.width(), img.height()), (w, h));
            for y in 0..h {
                for x in 0..w {
                    let want = rgb_of(&f, x, y);
                    let got = img.get_pixel(x, y).0;
                    assert_eq!(&got[..3], &want, "{w}x{h} at {x},{y}");
                    assert_eq!(got[3], 255);
                }
            }
        }
    }

    #[test]
    fn the_dibv5_pixels_read_back_by_hand_equal_the_frame() {
        // not through a decoder: the bytes after the header, read bottom-up as the masks say
        let f = sample(6, 4);
        let d = dibv5(&f);
        for y in 0..4u32 {
            for x in 0..6u32 {
                let row = 3 - y; // stored bottom-up
                let at = 124 + ((row * 6 + x) * 4) as usize;
                let px = u32_at(&d, at);
                let (r, g, b, a) = ((px >> 16) as u8, (px >> 8) as u8, px as u8, (px >> 24) as u8);
                let want = f.pixel(x, y);
                assert_eq!([b, g, r, a], want);
            }
        }
    }

    #[test]
    fn the_png_decodes_to_the_same_picture() {
        for (w, h) in [(1, 1), (7, 5), (64, 48), (300, 200)] {
            let f = sample(w, h);
            let bytes = png(&f);
            assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
            let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).unwrap().to_rgb8();
            assert_eq!((img.width(), img.height()), (w, h));
            for y in 0..h {
                for x in 0..w {
                    assert_eq!(img.get_pixel(x, y).0, rgb_of(&f, x, y), "{w}x{h} at {x},{y}");
                }
            }
        }
    }

    #[test]
    fn an_inconsistent_frame_makes_nothing() {
        let bad = Frame { width: 4, height: 4, bgra: vec![0; 10] };
        assert!(dib(&bad).is_empty() && dibv5(&bad).is_empty() && png(&bad).is_empty());
        assert!(!Formats::of(&bad).is_complete());
        let empty = Frame { width: 0, height: 0, bgra: Vec::new() };
        assert!(dib(&empty).is_empty());
        assert!(Formats::of(&sample(3, 3)).is_complete());
    }

    #[test]
    fn a_full_screen_picture_is_made_in_reasonable_time_and_size() {
        // a 1920x1080 frame of a gradient: the three formats, timed (loose bounds: this is a debug build)
        let f = sample(1920, 1080);
        let t = std::time::Instant::now();
        let formats = Formats::of(&f);
        let took = t.elapsed();
        assert!(formats.is_complete());
        assert_eq!(formats.dib.len(), 40 + 1920 * 3 * 1080);
        assert_eq!(formats.dibv5.len(), 124 + 1920 * 4 * 1080);
        assert!(took.as_secs() < 30, "{took:?}");
    }
}
