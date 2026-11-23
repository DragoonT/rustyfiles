//! A Photoshop file's picture (PSD, PSB) for the list's tiles and the preview pane, read from the file itself and not
//! from the shell: Windows has no thumbnail provider for Photoshop files, so without one installed (Adobe Bridge's,
//! SageThumbs …) the shell gives only the type icon.
//!
//! Two pictures are in every file Photoshop writes:
//! - the merged image (the "image data" section at the end of the file): the whole document flattened, at full size.
//!   Only the rows that are needed are read: with RLE compression every row's length is listed before the data, so a
//!   row can be found without reading the ones before it, and a 2 GB poster costs a few MB of reads for a 1280 px picture.
//!   It is not there when the file was saved without "Maximize compatibility" (then it is blank): resource 1057 says so.
//! - a small JPEG thumbnail (resource 1036, at most 160 px), used when the merged image can't be read (no real merged
//!   data, a colour mode or depth not handled here).
//!
//! Layout as in Adobe's "Photoshop File Formats Specification" (PSB: version 2, 8-byte section lengths and 4-byte row
//! lengths). Not drawn: alpha (the merged image is already matted on white, as Photoshop shows it), Lab, indexed,
//! bitmap, 32-bit; those take the JPEG thumbnail.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use image::RgbaImage;

use crate::thumbs::Bgra;

/// Most bytes of the image resources section read (they hold the thumbnail; a few hundred KB in practice, more with
/// big ICC profiles or paths).
const RESOURCES_MAX: u64 = 64 << 20;
/// Largest width × height accepted (PSB allows 300 000 × 300 000).
const PIXELS_MAX: u64 = 300_000 * 300_000;

pub fn handles(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    matches!(ext.as_str(), "psd" | "psb" | "pdd")
}

/// The file's picture, its long side at most `size` (never enlarged); `None` when it has none that can be read.
pub fn thumbnail(path: &Path, size: u32) -> Option<Bgra> {
    let file = File::open(path).ok()?;
    let img = read(file, size.max(16))?;
    Some(crate::look::decode::bgra_within(img, size.max(16)))
}

struct Header {
    psb: bool,
    channels: u16,
    height: u32,
    width: u32,
    depth: u16,
    mode: u16,
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn read_u32(r: &mut impl Read) -> Option<u32> {
    let mut b = [0u8; 4];
    r.read_exact(&mut b).ok()?;
    Some(u32::from_be_bytes(b))
}

fn read_u64(r: &mut impl Read) -> Option<u64> {
    let mut b = [0u8; 8];
    r.read_exact(&mut b).ok()?;
    Some(u64::from_be_bytes(b))
}

fn read(file: File, size: u32) -> Option<RgbaImage> {
    let mut r = BufReader::with_capacity(64 << 10, file);
    let mut h = [0u8; 26];
    r.read_exact(&mut h).ok()?;
    if &h[0..4] != b"8BPS" {
        return None;
    }
    let version = be16(&h[4..6]);
    if version != 1 && version != 2 {
        return None;
    }
    let hd = Header { psb: version == 2, channels: be16(&h[12..14]), height: be32(&h[14..18]), width: be32(&h[18..22]), depth: be16(&h[22..24]), mode: be16(&h[24..26]) };
    if hd.width == 0 || hd.height == 0 || hd.width as u64 * hd.height as u64 > PIXELS_MAX {
        return None;
    }
    // colour mode data
    let len = read_u32(&mut r)? as i64;
    r.seek_relative(len).ok()?;
    // image resources
    let len = read_u32(&mut r)? as u64;
    let res = if len <= RESOURCES_MAX {
        let mut v = vec![0u8; len as usize];
        r.read_exact(&mut v).ok()?;
        Some(v)
    } else {
        r.seek_relative(len as i64).ok()?;
        None
    };
    let res = res.as_deref().map(resources).unwrap_or_default();
    // layer and mask information
    let len = if hd.psb { read_u64(&mut r)? } else { read_u32(&mut r)? as u64 };
    r.seek_relative(i64::try_from(len).ok()?).ok()?;
    if res.merged != Some(false) {
        let start = r.stream_position().ok()?;
        if let Some(img) = merged(r.get_mut(), start, &hd, size) {
            return Some(img);
        }
    }
    res.thumbnail
}

#[derive(Default)]
struct Resources {
    /// Resource 1057's hasRealMergedData; `None` when the file has no version info.
    merged: Option<bool>,
    thumbnail: Option<RgbaImage>,
}

fn resources(mut b: &[u8]) -> Resources {
    let mut out = Resources::default();
    while b.len() >= 12 && &b[0..4] == b"8BIM" {
        let id = be16(&b[4..6]);
        // Pascal string name, padded to an even length (length byte included)
        let name_len = b[6] as usize;
        let mut at = 6 + ((name_len + 2) & !1);
        if b.len() < at + 4 {
            break;
        }
        let size = be32(&b[at..at + 4]) as usize;
        at += 4;
        let Some(data) = b.get(at..at + size) else { break };
        match id {
            1057 if data.len() >= 5 => out.merged = Some(data[4] != 0),
            1036 | 1033 => {
                if let Some(img) = thumbnail_resource(data, id == 1033) {
                    out.thumbnail = Some(img);
                }
            }
            _ => {}
        }
        let next = at + ((size + 1) & !1);
        b = b.get(next..).unwrap_or(&[]);
    }
    out
}

/// Resource 1036 (1033: the same in Photoshop 4, its JPEG in BGR order): a 28-byte header, then a JFIF JPEG.
fn thumbnail_resource(d: &[u8], bgr: bool) -> Option<RgbaImage> {
    if d.len() < 28 || be32(&d[0..4]) != 1 {
        return None;
    }
    let mut img = image::load_from_memory_with_format(&d[28..], image::ImageFormat::Jpeg).ok()?.to_rgba8();
    if bgr {
        for px in img.pixels_mut() {
            px.0.swap(0, 2);
        }
    }
    Some(img)
}

/// The merged image, sampled down to about twice `size` (then resized smoothly to `size` by the caller).
fn merged(f: &mut File, start: u64, hd: &Header, size: u32) -> Option<RgbaImage> {
    let bytes = match hd.depth {
        8 => 1usize,
        16 => 2,
        _ => return None,
    };
    let planes: usize = match (hd.mode, hd.channels) {
        (1, c) if c >= 1 => 1, // grayscale
        (3, c) if c >= 3 => 3, // RGB
        (4, c) if c >= 4 => 4, // CMYK
        _ => return None,
    };
    let (w, h) = (hd.width as usize, hd.height as usize);
    let step = (w.max(h) / (size as usize * 2)).max(1);
    let (ow, oh) = (w.div_ceil(step), h.div_ceil(step));
    let row_bytes = w * bytes;

    f.seek(SeekFrom::Start(start)).ok()?;
    let mut c = [0u8; 2];
    f.read_exact(&mut c).ok()?;
    let compression = be16(&c);
    let data_at = start + 2;
    let channels = hd.channels as usize;
    // where each (plane, row) starts and how long it is
    let rows: Box<dyn Fn(usize, usize) -> (u64, usize)> = match compression {
        0 => Box::new(move |p, y| (data_at + ((p * h + y) * row_bytes) as u64, row_bytes)),
        1 => {
            let entry = if hd.psb { 4 } else { 2 };
            let count = channels.checked_mul(h)?;
            let mut table = vec![0u8; count.checked_mul(entry)?];
            let mut br = BufReader::with_capacity(256 << 10, &mut *f);
            br.read_exact(&mut table).ok()?;
            let lens: Vec<u32> = table.chunks_exact(entry).map(|e| if entry == 4 { be32(e) } else { be16(e) as u32 }).collect();
            let mut offsets = Vec::with_capacity(planes * h);
            let mut at = data_at + table.len() as u64;
            for &l in lens.iter().take(planes * h) {
                offsets.push(at);
                at += l as u64;
            }
            Box::new(move |p, y| (offsets[p * h + y], lens[p * h + y] as usize))
        }
        _ => return None,
    };

    let mut out = vec![0u8; ow * oh * planes];
    let mut packed = Vec::new();
    let mut row = vec![0u8; row_bytes];
    for p in 0..planes {
        for oy in 0..oh {
            let y = oy * step;
            let (at, len) = rows(p, y);
            f.seek(SeekFrom::Start(at)).ok()?;
            if compression == 0 {
                f.read_exact(&mut row).ok()?;
            } else {
                packed.resize(len, 0);
                f.read_exact(&mut packed).ok()?;
                unpack_bits(&packed, &mut row)?;
            }
            for ox in 0..ow {
                // high byte of a 16-bit sample (big-endian)
                out[(oy * ow + ox) * planes + p] = row[ox * step * bytes];
            }
        }
    }

    let mut img = RgbaImage::new(ow as u32, oh as u32);
    for (px, s) in img.pixels_mut().zip(out.chunks_exact(planes)) {
        px.0 = match planes {
            1 => [s[0], s[0], s[0], 255],
            3 => [s[0], s[1], s[2], 255],
            // CMYK is stored inverted (255 = no ink): naive conversion without a profile
            _ => {
                let k = s[3] as u32;
                [(s[0] as u32 * k / 255) as u8, (s[1] as u32 * k / 255) as u8, (s[2] as u32 * k / 255) as u8, 255]
            }
        };
    }
    Some(img)
}

/// PackBits: a header byte n, then n+1 literal bytes (n ≥ 0) or one byte repeated 1−n times (n < 0; −128 is a no-op).
fn unpack_bits(src: &[u8], dst: &mut [u8]) -> Option<()> {
    let (mut i, mut o) = (0usize, 0usize);
    while o < dst.len() && i < src.len() {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let n = n as usize + 1;
            let s = src.get(i..i + n)?;
            let room = (dst.len() - o).min(n);
            dst[o..o + room].copy_from_slice(&s[..room]);
            i += n;
            o += room;
        } else if n != -128 {
            let n = (1 - n as isize) as usize;
            let v = *src.get(i)?;
            i += 1;
            let room = (dst.len() - o).min(n);
            dst[o..o + room].fill(v);
            o += room;
        }
    }
    (o == dst.len()).then_some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packbits() {
        // Apple's example: FE AA 02 80 00 2A FD AA 03 80 00 2A 22 F7 AA
        let src = [0xFE, 0xAA, 0x02, 0x80, 0x00, 0x2A, 0xFD, 0xAA, 0x03, 0x80, 0x00, 0x2A, 0x22, 0xF7, 0xAA];
        let want = [0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0xAA, 0xAA, 0xAA, 0xAA, 0x80, 0x00, 0x2A, 0x22, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA, 0xAA];
        let mut dst = [0u8; 24];
        assert!(unpack_bits(&src, &mut dst).is_some());
        assert_eq!(dst, want);
    }

    /// A 3 × 2 RGB PSD, RLE, rows written as literals.
    fn tiny_psd(merged_flag: Option<bool>) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"8BPS");
        v.extend_from_slice(&1u16.to_be_bytes());
        v.extend_from_slice(&[0; 6]);
        v.extend_from_slice(&3u16.to_be_bytes()); // channels
        v.extend_from_slice(&2u32.to_be_bytes()); // height
        v.extend_from_slice(&3u32.to_be_bytes()); // width
        v.extend_from_slice(&8u16.to_be_bytes());
        v.extend_from_slice(&3u16.to_be_bytes()); // RGB
        v.extend_from_slice(&0u32.to_be_bytes()); // colour mode data
        let mut res = Vec::new();
        if let Some(m) = merged_flag {
            res.extend_from_slice(b"8BIM");
            res.extend_from_slice(&1057u16.to_be_bytes());
            res.extend_from_slice(&[0, 0]); // empty name, padded
            res.extend_from_slice(&6u32.to_be_bytes());
            res.extend_from_slice(&[0, 0, 0, 1, m as u8, 0]);
        }
        v.extend_from_slice(&(res.len() as u32).to_be_bytes());
        v.extend_from_slice(&res);
        v.extend_from_slice(&0u32.to_be_bytes()); // layers
        v.extend_from_slice(&1u16.to_be_bytes()); // RLE
        let planes: [[[u8; 3]; 2]; 3] = [[[255, 0, 0], [10, 20, 30]], [[0, 255, 0], [40, 50, 60]], [[0, 0, 255], [70, 80, 90]]];
        for _ in 0..6 {
            v.extend_from_slice(&4u16.to_be_bytes());
        }
        for p in planes {
            for row in p {
                v.push(2);
                v.extend_from_slice(&row);
            }
        }
        v
    }

    fn decode(bytes: &[u8]) -> Option<RgbaImage> {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("rf-psd-test-{}-{n}", std::process::id()));
        std::fs::write(&dir, bytes).unwrap();
        let r = read(File::open(&dir).unwrap(), 512);
        let _ = std::fs::remove_file(&dir);
        r
    }

    #[test]
    fn merged_rle_rgb() {
        let img = decode(&tiny_psd(Some(true))).unwrap();
        assert_eq!(img.dimensions(), (3, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(2, 1).0, [30, 60, 90, 255]);
    }

    #[test]
    fn no_real_merged_data_and_no_thumbnail() {
        assert!(decode(&tiny_psd(Some(false))).is_none());
        assert!(decode(&tiny_psd(None)).is_some());
    }
}
