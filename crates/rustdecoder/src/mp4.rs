//! ISO base media files: MP4, MOV, M4V, M4A, 3GP.
//!
//! [`read`] walks the top-level boxes (skipping `mdat` without reading it), loads `moov` and turns
//! every track's sample tables into one flat list: where each sample is, how big, when it is
//! decoded and shown, and whether decoding can start there. Times follow the first edit of the
//! edit list the way players do (a B-frame stream's first picture is shown at 0, its first
//! decode time is negative). Nothing from the file is trusted: every table is bounded by the box
//! that holds it, and tables that disagree are cut to the shortest.
//!
//! Fragmented files (`moof`) are not read yet.

use std::io::{Read, Seek, SeekFrom};

use crate::{Error, Result};

/// `moov` larger than this is refused (a two-hour film's is a few MB).
const MOOV_MAX: u64 = 256 << 20;
/// Samples per track at most (a day of 240 fps video is 21 million).
const SAMPLES_MAX: usize = 50_000_000;

/// The whole file: movie timescale and the tracks.
#[derive(Debug, Clone)]
pub struct Movie {
    /// Units per second of [`Movie::duration`].
    pub timescale: u32,
    pub duration: u64,
    pub tracks: Vec<Track>,
}

impl Movie {
    pub fn duration_secs(&self) -> f64 {
        if self.timescale == 0 { 0.0 } else { self.duration as f64 / self.timescale as f64 }
    }

    /// The first video track.
    pub fn video(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == Kind::Video && !t.samples.is_empty())
    }

    /// The first audio track.
    pub fn audio(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == Kind::Audio && !t.samples.is_empty())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    /// H.264; the `avcC` box's payload (parameter sets, NAL length size).
    H264 { config: Vec<u8> },
    /// HEVC; the `hvcC` box's payload.
    Hevc { config: Vec<u8> },
    /// AAC; the AudioSpecificConfig from `esds`.
    Aac { config: Vec<u8> },
    /// Uncompressed audio.
    Pcm { bits: u16, float: bool, big_endian: bool },
    /// Anything else, by its sample entry's four letters.
    Other,
}

#[derive(Debug, Clone)]
pub struct Track {
    pub id: u32,
    pub kind: Kind,
    /// The sample entry's four letters (`avc1`, `hvc1`, `mp4a`, `twos`, …).
    pub fourcc: [u8; 4],
    pub codec: Codec,
    /// Units per second of the sample times.
    pub timescale: u32,
    /// Coded size (video).
    pub width: u32,
    pub height: u32,
    /// Clockwise degrees (0, 90, 180, 270) to turn the picture for display.
    pub rotation: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub samples: Vec<Sample>,
}

/// One sample: a compressed picture, or a block of audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Byte position in the file.
    pub offset: u64,
    pub size: u32,
    /// Decode time, in the track's timescale.
    pub dts: i64,
    /// Presentation time, in the track's timescale.
    pub pts: i64,
    /// Decoding can start here (a sync sample).
    pub key: bool,
}

impl Track {
    pub fn seconds(&self, t: i64) -> f64 {
        if self.timescale == 0 { 0.0 } else { t as f64 / self.timescale as f64 }
    }

    /// Reads sample `i` into `buf` (replacing its contents).
    pub fn read_sample<R: Read + Seek>(&self, r: &mut R, i: usize, buf: &mut Vec<u8>) -> Result<()> {
        let s = self.samples.get(i).ok_or(Error::Invalid("sample index"))?;
        buf.clear();
        buf.resize(s.size as usize, 0);
        r.seek(SeekFrom::Start(s.offset))?;
        r.read_exact(buf)?;
        Ok(())
    }

    /// Frames per second from the sample times (video), 0 when unknown.
    pub fn frame_rate(&self) -> f64 {
        let n = self.samples.len();
        if n < 2 || self.timescale == 0 {
            return 0.0;
        }
        let first = self.samples.iter().map(|s| s.dts).min().unwrap_or(0);
        let last = self.samples.iter().map(|s| s.dts).max().unwrap_or(0);
        if last <= first {
            return 0.0;
        }
        (n - 1) as f64 * self.timescale as f64 / (last - first) as f64
    }
}

/// Reads the structure of an ISO base media file.
pub fn read<R: Read + Seek>(r: &mut R) -> Result<Movie> {
    let end = r.seek(SeekFrom::End(0))?;
    let mut pos = 0u64;
    let mut moov = None;
    let mut fragmented = false;
    while pos + 8 <= end {
        r.seek(SeekFrom::Start(pos))?;
        let mut h = [0u8; 16];
        r.read_exact(&mut h[..8])?;
        let size32 = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as u64;
        let kind = [h[4], h[5], h[6], h[7]];
        let (size, header) = match size32 {
            0 => (end - pos, 8),
            1 => {
                r.read_exact(&mut h[8..16])?;
                (u64::from_be_bytes(h[8..16].try_into().unwrap()), 16)
            }
            n => (n, 8),
        };
        if size < header || pos + size > end {
            // a cut file: what was read so far still counts
            if moov.is_some() {
                break;
            }
            return Err(Error::Invalid("top-level box runs past the end of the file"));
        }
        match &kind {
            b"moov" => {
                let len = size - header;
                if len > MOOV_MAX {
                    return Err(Error::Unsupported("movie header over 256 MB"));
                }
                let mut b = vec![0u8; len as usize];
                r.read_exact(&mut b)?;
                moov = Some(b);
            }
            b"moof" => fragmented = true,
            _ => {}
        }
        pos += size;
    }
    let Some(moov) = moov else {
        return Err(if fragmented { Error::Unsupported("fragmented MP4") } else { Error::Invalid("no movie header (moov)") });
    };
    parse_moov(&moov)
}

// ------------------------------------------------------------------ byte cursor

struct Cur<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, p: 0 }
    }
    fn left(&self) -> usize {
        self.b.len() - self.p
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.left() < n {
            return Err(Error::Invalid("box cut short"));
        }
        let s = &self.b[self.p..self.p + n];
        self.p += n;
        Ok(s)
    }
    fn skip(&mut self, n: usize) -> Result<()> {
        self.bytes(n).map(|_| ())
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }
    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(self.bytes(8)?.try_into().unwrap()))
    }
    fn fourcc(&mut self) -> Result<[u8; 4]> {
        Ok(self.bytes(4)?.try_into().unwrap())
    }
    /// Version and flags of a full box.
    fn full(&mut self) -> Result<(u8, u32)> {
        let v = self.u32()?;
        Ok(((v >> 24) as u8, v & 0x00ff_ffff))
    }
    fn rest(&mut self) -> &'a [u8] {
        let s = &self.b[self.p..];
        self.p = self.b.len();
        s
    }
}

/// The child boxes of a box's payload, as (type, payload).
fn children(b: &[u8]) -> Result<Vec<([u8; 4], &[u8])>> {
    let mut out = Vec::new();
    let mut c = Cur::new(b);
    while c.left() >= 8 {
        let size32 = c.u32()? as u64;
        let kind = c.fourcc()?;
        let (size, header) = match size32 {
            0 => (c.left() as u64 + 8, 8u64),
            1 => (c.u64()?, 16),
            n => (n, 8),
        };
        if size < header || size - header > c.left() as u64 {
            return Err(Error::Invalid("box runs past its parent"));
        }
        out.push((kind, c.bytes((size - header) as usize)?));
    }
    Ok(out)
}

fn child<'a>(boxes: &[([u8; 4], &'a [u8])], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes.iter().find(|(k, _)| k == kind).map(|(_, b)| *b)
}

// ------------------------------------------------------------------ moov

fn parse_moov(b: &[u8]) -> Result<Movie> {
    let boxes = children(b)?;
    let mvhd = child(&boxes, b"mvhd").ok_or(Error::Invalid("no movie header (mvhd)"))?;
    let mut c = Cur::new(mvhd);
    let (v, _) = c.full()?;
    let (timescale, duration) = if v == 1 {
        c.skip(16)?;
        (c.u32()?, c.u64()?)
    } else {
        c.skip(8)?;
        (c.u32()?, c.u32()? as u64)
    };
    let mut tracks = Vec::new();
    for (k, t) in &boxes {
        if k == b"trak" {
            match parse_trak(t, timescale) {
                Ok(t) => tracks.push(t),
                // one broken track doesn't take the others down
                Err(e) => log::debug!("mp4: skipping a track: {e}"),
            }
        }
    }
    Ok(Movie { timescale, duration, tracks })
}

/// Clockwise degrees from a track header's matrix (screen coordinates, y down).
fn rotation(m: &[i32; 9]) -> u16 {
    let (a, b) = (m[0] as f64, m[1] as f64);
    let deg = b.atan2(a).to_degrees();
    (((deg / 90.0).round() as i32).rem_euclid(4) * 90) as u16
}

fn parse_trak(b: &[u8], movie_timescale: u32) -> Result<Track> {
    let boxes = children(b)?;
    let tkhd = child(&boxes, b"tkhd").ok_or(Error::Invalid("no track header"))?;
    let mut c = Cur::new(tkhd);
    let (v, _) = c.full()?;
    let id = if v == 1 {
        c.skip(16)?;
        let id = c.u32()?;
        c.skip(4 + 8)?;
        id
    } else {
        c.skip(8)?;
        let id = c.u32()?;
        c.skip(4 + 4)?;
        id
    };
    c.skip(8 + 2 + 2 + 2 + 2)?;
    let mut m = [0i32; 9];
    for x in &mut m {
        *x = c.i32()?;
    }
    let rot = rotation(&m);

    let mdia = children(child(&boxes, b"mdia").ok_or(Error::Invalid("no media box"))?)?;
    let mdhd = child(&mdia, b"mdhd").ok_or(Error::Invalid("no media header"))?;
    let mut c = Cur::new(mdhd);
    let (v, _) = c.full()?;
    let timescale = if v == 1 {
        c.skip(16)?;
        c.u32()?
    } else {
        c.skip(8)?;
        c.u32()?
    };
    let handler = child(&mdia, b"hdlr").map(|h| {
        let mut c = Cur::new(h);
        let _ = c.full();
        let _ = c.u32();
        c.fourcc().unwrap_or(*b"    ")
    });
    let kind = match handler.as_ref() {
        Some(b"vide") => Kind::Video,
        Some(b"soun") => Kind::Audio,
        _ => Kind::Other,
    };
    let minf = children(child(&mdia, b"minf").ok_or(Error::Invalid("no media information"))?)?;
    let stbl = children(child(&minf, b"stbl").ok_or(Error::Invalid("no sample table"))?)?;

    let mut t = Track {
        id,
        kind,
        fourcc: *b"    ",
        codec: Codec::Other,
        timescale,
        width: 0,
        height: 0,
        rotation: if kind == Kind::Video { rot } else { 0 },
        channels: 0,
        sample_rate: 0,
        samples: Vec::new(),
    };
    if let Some(stsd) = child(&stbl, b"stsd") {
        parse_stsd(stsd, &mut t)?;
    }
    t.samples = if kind == Kind::Audio && matches!(t.codec, Codec::Pcm { .. }) { pcm_chunks(&stbl)? } else { samples(&stbl)? };
    let shift = edit_shift(child(&boxes, b"edts"), movie_timescale, timescale)?;
    if shift != 0 {
        for s in &mut t.samples {
            s.dts += shift;
            s.pts += shift;
        }
    }
    Ok(t)
}

/// How far the first edit moves the track's times: an empty edit delays it, a media time
/// skips into it (encoder delay, B-frame reordering).
fn edit_shift(edts: Option<&[u8]>, movie_timescale: u32, timescale: u32) -> Result<i64> {
    let Some(edts) = edts else { return Ok(0) };
    let Some(elst) = child(&children(edts)?, b"elst") else { return Ok(0) };
    let mut c = Cur::new(elst);
    let (v, _) = c.full()?;
    let n = c.u32()?;
    let mut delay = 0i64;
    for _ in 0..n.min(64) {
        let (dur, media_time) = if v == 1 { (c.u64()? as i64, c.u64()? as i64) } else { (c.u32()? as i64, c.i32()? as i64) };
        c.skip(4)?;
        if media_time == -1 {
            // empty edit: nothing shown for `dur` (movie timescale)
            if movie_timescale > 0 {
                delay += (dur as i128 * timescale as i128 / movie_timescale as i128) as i64;
            }
            continue;
        }
        return Ok(delay - media_time);
    }
    Ok(delay)
}

// ------------------------------------------------------------------ sample description

fn parse_stsd(b: &[u8], t: &mut Track) -> Result<()> {
    let mut c = Cur::new(b);
    c.full()?;
    let n = c.u32()?;
    let entries = children(c.rest())?;
    let Some((fourcc, e)) = entries.into_iter().take(n as usize).next() else { return Ok(()) };
    t.fourcc = fourcc;
    match t.kind {
        Kind::Video => {
            let mut c = Cur::new(e);
            c.skip(6 + 2 + 2 + 2 + 12)?;
            t.width = c.u16()? as u32;
            t.height = c.u16()? as u32;
            c.skip(4 + 4 + 4 + 2 + 32 + 2 + 2)?;
            let kids = children(c.rest())?;
            t.codec = match &fourcc {
                b"avc1" | b"avc3" => child(&kids, b"avcC").map_or(Codec::Other, |x| Codec::H264 { config: x.to_vec() }),
                b"hvc1" | b"hev1" => child(&kids, b"hvcC").map_or(Codec::Other, |x| Codec::Hevc { config: x.to_vec() }),
                _ => Codec::Other,
            };
        }
        Kind::Audio => {
            let mut c = Cur::new(e);
            c.skip(6 + 2)?;
            let version = c.u16()?;
            c.skip(2 + 4)?;
            let mut channels = c.u16()?;
            let mut bits = c.u16()?;
            c.skip(2 + 2)?;
            let mut rate = c.u32()? >> 16;
            let mut lpcm_flags = 0u32;
            match version {
                1 => c.skip(16)?,
                2 => {
                    c.skip(4)?;
                    rate = f64::from_bits(c.u64()?) as u32;
                    channels = c.u32()? as u16;
                    c.skip(4)?;
                    bits = c.u32()? as u16;
                    lpcm_flags = c.u32()?;
                    c.skip(8)?;
                }
                _ => {}
            }
            t.channels = channels;
            t.sample_rate = rate;
            let kids = children(c.rest()).unwrap_or_default();
            t.codec = match &fourcc {
                b"mp4a" => esds_config(&kids).map_or(Codec::Other, |config| Codec::Aac { config }),
                b"twos" => Codec::Pcm { bits: if bits == 8 { 8 } else { 16 }, float: false, big_endian: true },
                b"sowt" => Codec::Pcm { bits: 16, float: false, big_endian: false },
                b"in24" => Codec::Pcm { bits: 24, float: false, big_endian: !little_endian(&kids) },
                b"in32" => Codec::Pcm { bits: 32, float: false, big_endian: !little_endian(&kids) },
                b"fl32" => Codec::Pcm { bits: 32, float: true, big_endian: !little_endian(&kids) },
                b"fl64" => Codec::Pcm { bits: 64, float: true, big_endian: !little_endian(&kids) },
                // QuickTime LPCM: flags 1 = float, 2 = big endian
                b"lpcm" => Codec::Pcm { bits, float: lpcm_flags & 1 != 0, big_endian: lpcm_flags & 2 != 0 },
                b"ipcm" | b"fpcm" => {
                    let le = child(&kids, b"pcmC").and_then(|p| p.get(4)).is_some_and(|f| f & 1 != 0);
                    Codec::Pcm { bits, float: &fourcc == b"fpcm", big_endian: !le }
                }
                _ => Codec::Other,
            };
        }
        Kind::Other => {}
    }
    Ok(())
}

/// QuickTime keeps byte order in `wave` → `enda`.
fn little_endian(kids: &[([u8; 4], &[u8])]) -> bool {
    child(kids, b"wave").and_then(|w| children(w).ok()).and_then(|w| child(&w, b"enda").map(|e| e.get(1) == Some(&1) || e.first() == Some(&1))).unwrap_or(false)
}

/// The AudioSpecificConfig inside `esds` (directly, or inside QuickTime's `wave`).
fn esds_config(kids: &[([u8; 4], &[u8])]) -> Option<Vec<u8>> {
    let esds = child(kids, b"esds").or_else(|| {
        let wave = child(kids, b"wave")?;
        let w = children(wave).ok()?;
        child(&w, b"esds")
    })?;
    let mut c = Cur::new(esds);
    c.full().ok()?;
    // descriptors: tag, length in 7-bit groups
    fn desc<'a>(c: &mut Cur<'a>) -> Option<(u8, Cur<'a>)> {
        let tag = c.u8().ok()?;
        let mut len = 0usize;
        for _ in 0..4 {
            let b = c.u8().ok()?;
            len = (len << 7) | (b & 0x7f) as usize;
            if b & 0x80 == 0 {
                break;
            }
        }
        Some((tag, Cur::new(c.bytes(len.min(c.left())).ok()?)))
    }
    let (tag, mut es) = desc(&mut c)?;
    if tag != 3 {
        return None;
    }
    es.skip(2).ok()?;
    let flags = es.u8().ok()?;
    if flags & 0x80 != 0 {
        es.skip(2).ok()?;
    }
    if flags & 0x40 != 0 {
        let n = es.u8().ok()? as usize;
        es.skip(n).ok()?;
    }
    if flags & 0x20 != 0 {
        es.skip(2).ok()?;
    }
    let (tag, mut dc) = desc(&mut es)?;
    if tag != 4 {
        return None;
    }
    dc.skip(13).ok()?;
    let (tag, info) = desc(&mut dc)?;
    (tag == 5).then(|| info.b.to_vec())
}

// ------------------------------------------------------------------ sample tables

/// Uncompressed audio lists every sample (one per channel group: 48 000 a second), so its
/// samples are taken a chunk at a time instead: one entry per chunk, its size the chunk's bytes.
fn pcm_chunks(stbl: &[([u8; 4], &[u8])]) -> Result<Vec<Sample>> {
    let Some(stsz) = child(stbl, b"stsz") else { return samples(stbl) };
    let mut c = Cur::new(stsz);
    c.full()?;
    let size = c.u32()? as u64;
    let count = c.u32()? as u64;
    if size == 0 {
        return samples(stbl);
    }
    // decode times as runs (count, delta), walked alongside the chunks
    let mut stts = Vec::new();
    if let Some(b) = child(stbl, b"stts") {
        let mut c = Cur::new(b);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 8);
        for _ in 0..n {
            stts.push((c.u32()? as u64, c.u32()? as i64));
        }
    }
    let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        let mut c = Cur::new(stco);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 4);
        (0..n).map(|_| c.u32().map(|x| x as u64)).collect::<Result<_>>()?
    } else if let Some(co64) = child(stbl, b"co64") {
        let mut c = Cur::new(co64);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 8);
        (0..n).map(|_| c.u64()).collect::<Result<_>>()?
    } else {
        return Err(Error::Invalid("no chunk offsets"));
    };
    let stsc = child(stbl, b"stsc").ok_or(Error::Invalid("no sample-to-chunk table"))?;
    let mut c = Cur::new(stsc);
    c.full()?;
    let n = (c.u32()? as usize).min(c.left() / 12);
    let mut runs = Vec::with_capacity(n);
    for _ in 0..n {
        let first = c.u32()?;
        let per = c.u32()?;
        c.skip(4)?;
        runs.push((first.max(1) as usize, per as u64));
    }
    let (mut run, mut left, mut t, mut si) = (0usize, count, 0i64, 0usize);
    let mut in_run = stts.first().map_or(0, |r| r.0);
    let mut out = Vec::with_capacity(chunks.len());
    for (ci, &offset) in chunks.iter().enumerate() {
        while run + 1 < runs.len() && runs[run + 1].0 <= ci + 1 {
            run += 1;
        }
        let per = runs.get(run).filter(|r| r.0 <= ci + 1).map_or(0, |r| r.1).min(left);
        if per == 0 {
            if left == 0 {
                break;
            }
            continue;
        }
        let dts = t;
        // advance the clock by `per` samples
        let mut k = per;
        while k > 0 && si < stts.len() {
            let take = k.min(in_run);
            t += take as i64 * stts[si].1;
            k -= take;
            in_run -= take;
            if in_run == 0 {
                si += 1;
                in_run = stts.get(si).map_or(0, |r| r.0);
            }
        }
        let bytes = (per * size).min(u32::MAX as u64) as u32;
        out.push(Sample { offset, size: bytes, dts, pts: dts, key: true });
        left -= per;
    }
    Ok(out)
}

fn samples(stbl: &[([u8; 4], &[u8])]) -> Result<Vec<Sample>> {
    // sizes
    let mut sizes: Vec<u32> = Vec::new();
    let mut count;
    let mut constant = None;
    if let Some(stsz) = child(stbl, b"stsz") {
        let mut c = Cur::new(stsz);
        c.full()?;
        let size = c.u32()?;
        count = c.u32()? as usize;
        if size != 0 {
            constant = Some(size);
        } else {
            count = count.min(c.left() / 4);
            sizes.reserve(count);
            for _ in 0..count {
                sizes.push(c.u32()?);
            }
        }
    } else if let Some(stz2) = child(stbl, b"stz2") {
        let mut c = Cur::new(stz2);
        c.full()?;
        c.skip(3)?;
        let field = c.u8()?;
        count = c.u32()? as usize;
        match field {
            4 => {
                count = count.min(c.left() * 2);
                for i in 0..count {
                    let b = c.b[c.p + i / 2];
                    sizes.push(if i % 2 == 0 { (b >> 4) as u32 } else { (b & 15) as u32 });
                }
            }
            8 => {
                count = count.min(c.left());
                for _ in 0..count {
                    sizes.push(c.u8()? as u32);
                }
            }
            16 => {
                count = count.min(c.left() / 2);
                for _ in 0..count {
                    sizes.push(c.u16()? as u32);
                }
            }
            _ => return Err(Error::Invalid("compact sample size field")),
        }
    } else {
        return Ok(Vec::new());
    }

    // decode times
    let mut dts: Vec<i64> = Vec::new();
    if let Some(stts) = child(stbl, b"stts") {
        let mut c = Cur::new(stts);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 8);
        let mut t = 0i64;
        'outer: for _ in 0..n {
            let k = c.u32()?;
            let delta = c.u32()? as i64;
            for _ in 0..k {
                if dts.len() >= count.min(SAMPLES_MAX) {
                    break 'outer;
                }
                dts.push(t);
                t += delta;
            }
        }
    }
    count = count.min(dts.len()).min(SAMPLES_MAX);
    if constant.is_some() {
        sizes = vec![constant.unwrap(); count];
    }
    count = count.min(sizes.len());

    // composition offsets
    let mut cts = vec![0i64; count];
    if let Some(ctts) = child(stbl, b"ctts") {
        let mut c = Cur::new(ctts);
        let (v, _) = c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 8);
        let mut i = 0usize;
        for _ in 0..n {
            let k = c.u32()? as usize;
            let raw = c.u32()?;
            let off = if v == 0 && raw <= i32::MAX as u32 { raw as i64 } else { raw as i32 as i64 };
            for _ in 0..k {
                if i >= count {
                    break;
                }
                cts[i] = off;
                i += 1;
            }
        }
    }

    // chunk offsets
    let chunks: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        let mut c = Cur::new(stco);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 4);
        (0..n).map(|_| c.u32().map(|x| x as u64)).collect::<Result<_>>()?
    } else if let Some(co64) = child(stbl, b"co64") {
        let mut c = Cur::new(co64);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 8);
        (0..n).map(|_| c.u64()).collect::<Result<_>>()?
    } else {
        return Err(Error::Invalid("no chunk offsets"));
    };

    // samples per chunk: runs of (first chunk, samples per chunk)
    let stsc = child(stbl, b"stsc").ok_or(Error::Invalid("no sample-to-chunk table"))?;
    let mut c = Cur::new(stsc);
    c.full()?;
    let n = (c.u32()? as usize).min(c.left() / 12);
    let mut runs = Vec::with_capacity(n);
    for _ in 0..n {
        let first = c.u32()?;
        let per = c.u32()?;
        c.skip(4)?;
        runs.push((first.max(1) as usize, per as usize));
    }

    // sync samples (none listed: every sample is one)
    let mut key = vec![child(stbl, b"stss").is_none(); count];
    if let Some(stss) = child(stbl, b"stss") {
        let mut c = Cur::new(stss);
        c.full()?;
        let n = (c.u32()? as usize).min(c.left() / 4);
        for _ in 0..n {
            let i = c.u32()? as usize;
            if (1..=count).contains(&i) {
                key[i - 1] = true;
            }
        }
    }

    let mut out = Vec::with_capacity(count);
    let mut i = 0usize;
    let mut run = 0usize;
    'chunks: for (ci, &base) in chunks.iter().enumerate() {
        let chunk_no = ci + 1;
        while run + 1 < runs.len() && runs[run + 1].0 <= chunk_no {
            run += 1;
        }
        let per = runs.get(run).filter(|r| r.0 <= chunk_no).map_or(0, |r| r.1);
        let mut off = base;
        for _ in 0..per {
            if i >= count {
                break 'chunks;
            }
            out.push(Sample { offset: off, size: sizes[i], dts: dts[i], pts: dts[i] + cts[i], key: key[i] });
            off += sizes[i] as u64;
            i += 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn rotation_from_the_matrix() {
        let one = 0x10000;
        assert_eq!(super::rotation(&[one, 0, 0, 0, one, 0, 0, 0, 0x4000_0000]), 0);
        assert_eq!(super::rotation(&[0, one, 0, -one, 0, 0, 0, 0, 0x4000_0000]), 90);
        assert_eq!(super::rotation(&[-one, 0, 0, 0, -one, 0, 0, 0, 0x4000_0000]), 180);
        assert_eq!(super::rotation(&[0, -one, 0, one, 0, 0, 0, 0, 0x4000_0000]), 270);
    }
}
