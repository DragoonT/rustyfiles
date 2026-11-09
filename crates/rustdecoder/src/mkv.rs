//! Matroska and WebM.
//!
//! [`read`] walks the EBML structure of the Segment and returns the same [`Movie`] the MP4 reader
//! gives: tracks with a flat sample table (file offset and size of every frame, decode and
//! presentation time, sync flag), so the decoders and the player work on either container.
//!
//! The file is indexed, never loaded: only element headers and the few bytes in front of each
//! block (track number, time, flags, lace sizes) are read, one buffered window at a time, and
//! the payloads stay on disk. A multi-gigabyte file costs one `Sample` (32 bytes) per frame.
//! Because every cluster is indexed the `Cues` are not needed for seeking (the sample table has
//! every sync sample); they are skipped.
//!
//! Handled: unknown-size Segment and Clusters (live WebM), SimpleBlock and BlockGroup (a block
//! group is a sync sample when it has no `ReferenceBlock`), Xiph, EBML and fixed-size lacing
//! (every laced frame is its own sample), `DefaultDuration`, `CodecDelay`, the `Colour` element.
//! Tracks with `ContentEncodings` (header stripping, compression, encryption) are listed but
//! have no samples: their frames cannot be given as offset plus size.
//!
//! Times: a track's timescale is the ticks per second of `TimestampScale` (1000 for the usual
//! 1 ms), so `pts` is the container's own integer, as ffprobe prints it. Video `dts` are the
//! presentation times put in decoding order (Matroska stores no decode time), audio `dts` is
//! `pts`. Frames inside a lace are timed by `DefaultDuration`, else by `BlockDuration / n`, else
//! share the block's time.

use std::io::{Read, Seek, SeekFrom};

use crate::mp4::{Codec, Colour, Kind, Movie, Sample, Track};
use crate::{Error, Result};

const SAMPLES_MAX: usize = 50_000_000;
/// Element payloads parsed in memory (Info, Tracks) are refused above this.
const META_MAX: u64 = 64 << 20;

mod id {
    pub const EBML: u32 = 0x1A45_DFA3;
    pub const SEGMENT: u32 = 0x1853_8067;
    pub const INFO: u32 = 0x1549_A966;
    pub const TRACKS: u32 = 0x1654_AE6B;
    pub const CLUSTER: u32 = 0x1F43_B675;
    pub const TIMESTAMP_SCALE: u32 = 0x2A_D7B1;
    pub const DURATION: u32 = 0x4489;
    pub const TRACK_ENTRY: u32 = 0xAE;
    pub const TRACK_NUMBER: u32 = 0xD7;
    pub const TRACK_TYPE: u32 = 0x83;
    pub const FLAG_DEFAULT: u32 = 0x88;
    pub const FLAG_ENABLED: u32 = 0xB9;
    pub const DEFAULT_DURATION: u32 = 0x23_E383;
    pub const CODEC_ID: u32 = 0x86;
    pub const CODEC_PRIVATE: u32 = 0x63A2;
    pub const CODEC_DELAY: u32 = 0x56AA;
    pub const CONTENT_ENCODINGS: u32 = 0x6D80;
    pub const VIDEO: u32 = 0xE0;
    pub const PIXEL_WIDTH: u32 = 0xB0;
    pub const PIXEL_HEIGHT: u32 = 0xBA;
    pub const COLOUR: u32 = 0x55B0;
    pub const MATRIX: u32 = 0x55B1;
    pub const BITS_PER_CHANNEL: u32 = 0x55B2;
    pub const RANGE: u32 = 0x55B9;
    pub const TRANSFER: u32 = 0x55BA;
    pub const PRIMARIES: u32 = 0x55BB;
    pub const AUDIO: u32 = 0xE1;
    pub const SAMPLING_FREQUENCY: u32 = 0xB5;
    pub const OUTPUT_SAMPLING_FREQUENCY: u32 = 0x78B5;
    pub const CHANNELS: u32 = 0x9F;
    pub const BIT_DEPTH: u32 = 0x6264;
    pub const CLUSTER_TIMESTAMP: u32 = 0xE7;
    pub const SIMPLE_BLOCK: u32 = 0xA3;
    pub const BLOCK_GROUP: u32 = 0xA0;
    pub const BLOCK: u32 = 0xA1;
    pub const BLOCK_DURATION: u32 = 0x9B;
    pub const REFERENCE_BLOCK: u32 = 0xFB;
    pub const DISCARD_PADDING: u32 = 0x75A2;
    /// Elements that may sit inside a Cluster; anything else ends an unknown-size one.
    pub fn in_cluster(id: u32) -> bool {
        matches!(id, CLUSTER_TIMESTAMP | 0xA7 | 0xAB | SIMPLE_BLOCK | BLOCK_GROUP | 0xAF | 0x5854 | 0xEC | 0xBF)
    }
}

/// True when `head` (the first bytes of a file) starts an EBML document.
pub fn is_ebml(head: &[u8]) -> bool {
    head.len() >= 4 && head[..4] == id::EBML.to_be_bytes()
}

// ------------------------------------------------------------------ buffered window

struct Src<R> {
    r: R,
    len: u64,
    buf: Vec<u8>,
    base: u64,
}

impl<R: Read + Seek> Src<R> {
    /// `n` bytes at `pos` (fewer at the end of the file).
    fn get(&mut self, pos: u64, n: usize) -> Result<&[u8]> {
        if pos >= self.len {
            return Ok(&[]);
        }
        let n = n.min((self.len - pos) as usize);
        let inside = pos >= self.base && pos + n as u64 <= self.base + self.buf.len() as u64;
        if !inside {
            let want = n.max(32 * 1024).min((self.len - pos) as usize);
            self.buf.resize(want, 0);
            self.r.seek(SeekFrom::Start(pos))?;
            self.r.read_exact(&mut self.buf)?;
            self.base = pos;
        }
        let o = (pos - self.base) as usize;
        Ok(&self.buf[o..o + n])
    }
}

/// An element header: id, payload size (`None` = unknown), header length.
fn header(b: &[u8]) -> Option<(u32, Option<u64>, usize)> {
    let first = *b.first()?;
    let il = first.leading_zeros() as usize + 1;
    if il > 4 || b.len() < il {
        return None;
    }
    let mut idv = 0u32;
    for &x in &b[..il] {
        idv = idv << 8 | x as u32;
    }
    let (size, sl) = vint(&b[il..])?;
    Some((idv, size, il + sl))
}

/// A size-style variable length integer: (value, `None` when all value bits are 1, length).
fn vint(b: &[u8]) -> Option<(Option<u64>, usize)> {
    let first = *b.first()?;
    if first == 0 {
        return None;
    }
    let l = first.leading_zeros() as usize + 1;
    if b.len() < l {
        return None;
    }
    let mut v = (first as u64) & ((1u64 << (8 - l)) - 1);
    let mut ones = v == (1u64 << (8 - l)) - 1;
    for &x in &b[1..l] {
        v = v << 8 | x as u64;
        ones &= x == 0xff;
    }
    Some((if ones { None } else { Some(v) }, l))
}

fn uint(b: &[u8]) -> u64 {
    b.iter().take(8).fold(0u64, |a, &x| a << 8 | x as u64)
}

fn float(b: &[u8]) -> f64 {
    match b.len() {
        4 => f32::from_be_bytes(b.try_into().unwrap()) as f64,
        8 => f64::from_be_bytes(b.try_into().unwrap()),
        _ => 0.0,
    }
}

/// The children of a payload that is held in memory: (id, payload).
fn children(mut b: &[u8]) -> Vec<(u32, &[u8])> {
    let mut out = Vec::new();
    while let Some((i, size, hl)) = header(b) {
        let size = size.map_or(b.len() - hl, |s| s.min((b.len() - hl) as u64) as usize);
        out.push((i, &b[hl..hl + size]));
        b = &b[hl + size..];
    }
    out
}

// ------------------------------------------------------------------ tracks

#[derive(Default)]
struct RawTrack {
    number: u64,
    kind: u64,
    enabled: bool,
    default: bool,
    default_duration_ns: u64,
    codec_id: String,
    private: Vec<u8>,
    codec_delay_ns: u64,
    encoded: bool,
    width: u32,
    height: u32,
    colour: Option<Colour>,
    rate: f64,
    channels: u16,
    bits: u16,
}

fn parse_tracks(b: &[u8]) -> Vec<RawTrack> {
    let mut out = Vec::new();
    for (i, e) in children(b) {
        if i != id::TRACK_ENTRY {
            continue;
        }
        let mut t = RawTrack { enabled: true, default: true, channels: 1, rate: 8000.0, ..Default::default() };
        let mut output_rate = None;
        for (i, v) in children(e) {
            match i {
                id::TRACK_NUMBER => t.number = uint(v),
                id::TRACK_TYPE => t.kind = uint(v),
                id::FLAG_ENABLED => t.enabled = uint(v) != 0,
                id::FLAG_DEFAULT => t.default = uint(v) != 0,
                id::DEFAULT_DURATION => t.default_duration_ns = uint(v),
                id::CODEC_ID => t.codec_id = String::from_utf8_lossy(v).trim_end_matches('\0').to_string(),
                id::CODEC_PRIVATE => t.private = v.to_vec(),
                id::CODEC_DELAY => t.codec_delay_ns = uint(v),
                id::CONTENT_ENCODINGS => t.encoded = true,
                id::VIDEO => {
                    for (i, v) in children(v) {
                        match i {
                            id::PIXEL_WIDTH => t.width = uint(v) as u32,
                            id::PIXEL_HEIGHT => t.height = uint(v) as u32,
                            id::COLOUR => {
                                let mut c = Colour::default();
                                for (i, v) in children(v) {
                                    match i {
                                        id::MATRIX => c.matrix = uint(v) as u8,
                                        id::TRANSFER => c.transfer = uint(v) as u8,
                                        id::PRIMARIES => c.primaries = uint(v) as u8,
                                        id::BITS_PER_CHANNEL => c.bits_per_channel = uint(v) as u8,
                                        id::RANGE => c.full_range = match uint(v) {
                                            1 => Some(false),
                                            2 => Some(true),
                                            _ => None,
                                        },
                                        _ => {}
                                    }
                                }
                                t.colour = Some(c);
                            }
                            _ => {}
                        }
                    }
                }
                id::AUDIO => {
                    for (i, v) in children(v) {
                        match i {
                            id::SAMPLING_FREQUENCY => t.rate = float(v),
                            id::OUTPUT_SAMPLING_FREQUENCY => output_rate = Some(float(v)),
                            id::CHANNELS => t.channels = uint(v) as u16,
                            id::BIT_DEPTH => t.bits = uint(v) as u16,
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        // Opus is always decoded at 48 kHz; SBR AAC states the output rate separately
        if let Some(r) = output_rate.filter(|r| *r > 0.0) {
            t.rate = r;
        }
        if t.number != 0 {
            out.push(t);
        }
    }
    out
}

fn codec_of(t: &RawTrack) -> Codec {
    let cfg = t.private.clone();
    let c = t.codec_id.as_str();
    match c {
        "V_MPEG4/ISO/AVC" => Codec::H264 { config: cfg },
        "V_MPEGH/ISO/HEVC" => Codec::Hevc { config: cfg },
        "V_VP9" => Codec::Vp9 { config: cfg },
        "V_AV1" => Codec::Av1 { config: cfg },
        "A_OPUS" => Codec::Opus { config: cfg },
        "A_VORBIS" => Codec::Vorbis { config: cfg },
        "A_FLAC" => Codec::Flac { config: cfg },
        "A_MPEG/L3" => Codec::Mp3,
        "A_AC3" => Codec::Ac3 { config: cfg },
        "A_EAC3" => Codec::Eac3 { config: cfg },
        "A_PCM/INT/LIT" => Codec::Pcm { bits: t.bits, float: false, big_endian: false },
        "A_PCM/INT/BIG" => Codec::Pcm { bits: t.bits, float: false, big_endian: true },
        "A_PCM/FLOAT/IEEE" => Codec::Pcm { bits: t.bits, float: true, big_endian: false },
        _ if c.starts_with("A_AAC") => Codec::Aac { config: cfg },
        _ => Codec::Other,
    }
}

fn fourcc_of(c: &str) -> [u8; 4] {
    match c {
        "V_MPEG4/ISO/AVC" => *b"avc1",
        "V_MPEGH/ISO/HEVC" => *b"hvc1",
        "V_VP9" => *b"vp09",
        "V_AV1" => *b"av01",
        "A_OPUS" => *b"Opus",
        "A_VORBIS" => *b"vorb",
        "A_FLAC" => *b"fLaC",
        "A_MPEG/L3" => *b".mp3",
        "A_AC3" => *b"ac-3",
        "A_EAC3" => *b"ec-3",
        _ if c.starts_with("A_AAC") => *b"mp4a",
        _ if c.starts_with("A_PCM") => *b"lpcm",
        _ => *b"    ",
    }
}

// ------------------------------------------------------------------ blocks

/// A frame found in a cluster, before the track table is known.
struct Raw {
    track: u64,
    /// Time of the block (cluster time + relative time), in scale units.
    ts: i64,
    offset: u64,
    size: u32,
    key: bool,
    /// Position inside the lace and the lace's frame count.
    lace: (u32, u32),
    /// `BlockDuration` of the whole block, scale units.
    duration: Option<i64>,
    /// `DiscardPadding` (nanoseconds cut from the end of the block's sound).
    discard: Option<i64>,
}

/// Where the frames of one (Simple)Block are. Appends to `out`; `false` when it is unusable.
fn block<R: Read + Seek>(s: &mut Src<R>, pos: u64, size: u64, cluster_ts: i64, simple_key: Option<bool>, out: &mut Vec<Raw>) -> Result<bool> {
    let head = s.get(pos, 4 + 3)?;
    let Some((Some(track), tl)) = vint(head) else { return Ok(false) };
    if head.len() < tl + 3 || size < (tl + 3) as u64 {
        return Ok(false);
    }
    let rel = i16::from_be_bytes([head[tl], head[tl + 1]]) as i64;
    let flags = head[tl + 2];
    let key = simple_key.map(|_| flags & 0x80 != 0);
    let lacing = (flags >> 1) & 3;
    let mut at = pos + (tl + 3) as u64;
    let mut left = size - (tl + 3) as u64;
    let ts = cluster_ts + rel;
    let mk = |offset: u64, size: u32, i: u32, n: u32| Raw { track, ts, offset, size, key: key.unwrap_or(true), lace: (i, n), duration: None, discard: None };
    if lacing == 0 {
        if left > u32::MAX as u64 {
            return Ok(false);
        }
        out.push(mk(at, left as u32, 0, 1));
        return Ok(true);
    }
    let h = s.get(at, left.min(1 << 20) as usize)?.to_vec();
    let Some(&cnt) = h.first() else { return Ok(false) };
    let n = cnt as usize + 1;
    let mut p = 1usize;
    let mut sizes: Vec<u64> = Vec::with_capacity(n);
    match lacing {
        1 => {
            for _ in 0..n - 1 {
                let mut v = 0u64;
                loop {
                    let Some(&b) = h.get(p) else { return Ok(false) };
                    p += 1;
                    v += b as u64;
                    if b != 255 {
                        break;
                    }
                }
                sizes.push(v);
            }
        }
        3 if n > 1 => {
            let Some((Some(first), l)) = vint(&h[p.min(h.len())..]) else { return Ok(false) };
            p += l;
            sizes.push(first);
            let mut prev = first as i64;
            for _ in 1..n - 1 {
                let Some((Some(raw), l)) = vint(&h[p.min(h.len())..]) else { return Ok(false) };
                p += l;
                let bias = (1i64 << (7 * l - 1)) - 1;
                prev += raw as i64 - bias;
                if prev < 0 {
                    return Ok(false);
                }
                sizes.push(prev as u64);
            }
        }
        3 => {}
        _ => {
            let each = (left - 1) / n as u64;
            sizes.extend(std::iter::repeat_n(each, n - 1));
        }
    }
    at += p as u64;
    left = left.saturating_sub(p as u64);
    let used: u64 = sizes.iter().sum();
    if used > left {
        return Ok(false);
    }
    sizes.push(left - used);
    for (i, &sz) in sizes.iter().enumerate() {
        if sz > u32::MAX as u64 {
            return Ok(false);
        }
        out.push(mk(at, sz as u32, i as u32, n as u32));
        at += sz;
    }
    Ok(true)
}

/// Walks one cluster whose payload starts at `pos`; returns where the cluster ends.
fn cluster<R: Read + Seek>(s: &mut Src<R>, pos: u64, size: Option<u64>, out: &mut Vec<Raw>) -> Result<u64> {
    let end = size.map_or(s.len, |z| (pos + z).min(s.len));
    let mut p = pos;
    let mut ts = 0i64;
    while p < end {
        let Some((i, sz, hl)) = header(s.get(p, 12)?) else { return Ok(end) };
        if size.is_none() && !id::in_cluster(i) {
            return Ok(p);
        }
        let dp = p + hl as u64;
        let Some(z) = sz else { return Ok(end) };
        let next = dp.saturating_add(z);
        if next > s.len {
            // a cut file: the last element is incomplete
            return Ok(s.len);
        }
        match i {
            id::CLUSTER_TIMESTAMP => ts = uint(s.get(dp, z.min(8) as usize)?) as i64,
            id::SIMPLE_BLOCK => {
                block(s, dp, z, ts, Some(true), out)?;
            }
            id::BLOCK_GROUP => {
                let first = out.len();
                let (mut has_ref, mut duration, mut discard) = (false, None, None);
                let mut q = dp;
                let gend = next.min(end);
                while q < gend {
                    let Some((gi, gz, gh)) = header(s.get(q, 12)?) else { break };
                    let Some(gz) = gz else { break };
                    let gdp = q + gh as u64;
                    match gi {
                        id::BLOCK => {
                            block(s, gdp, gz, ts, None, out)?;
                        }
                        id::BLOCK_DURATION => duration = Some(uint(s.get(gdp, gz.min(8) as usize)?) as i64),
                        id::REFERENCE_BLOCK => has_ref = true,
                        id::DISCARD_PADDING => {
                            // signed, big endian, 1 to 8 bytes
                            let b = s.get(gdp, gz.min(8) as usize)?;
                            let mut v = if b.first().is_some_and(|x| x & 0x80 != 0) { -1i64 } else { 0 };
                            for x in b {
                                v = (v << 8) | *x as i64;
                            }
                            discard = Some(v);
                        }
                        _ => {}
                    }
                    q = gdp.saturating_add(gz);
                }
                for r in &mut out[first..] {
                    r.key = !has_ref;
                    r.duration = duration;
                    r.discard = discard;
                }
            }
            _ => {}
        }
        if next > end {
            return Ok(end);
        }
        p = next;
    }
    Ok(end)
}

// ------------------------------------------------------------------ the file

/// Reads the structure of a Matroska / WebM file.
pub fn read<R: Read + Seek>(r: &mut R) -> Result<Movie> {
    let len = r.seek(SeekFrom::End(0))?;
    let mut s = Src { r, len, buf: Vec::new(), base: 0 };
    let Some((i, ebml_size, hl)) = header(s.get(0, 12)?) else { return Err(Error::Invalid("not a Matroska file")) };
    if i != id::EBML {
        return Err(Error::Invalid("not a Matroska file"));
    }
    let mut pos = hl as u64 + ebml_size.ok_or(Error::Invalid("EBML header of unknown size"))?;
    // the Segment
    let seg_end;
    loop {
        let Some((i, sz, hl)) = header(s.get(pos, 12)?) else { return Err(Error::Invalid("no Segment")) };
        pos += hl as u64;
        if i == id::SEGMENT {
            seg_end = sz.map_or(len, |z| (pos + z).min(len));
            break;
        }
        pos = pos.saturating_add(sz.ok_or(Error::Invalid("no Segment"))?);
        if pos >= len {
            return Err(Error::Invalid("no Segment"));
        }
    }

    let mut scale = 1_000_000u64;
    let mut duration = 0f64;
    let mut tracks = Vec::new();
    let mut raws: Vec<Raw> = Vec::new();
    while pos < seg_end {
        let Some((i, sz, hl)) = header(s.get(pos, 12)?) else { break };
        let dp = pos + hl as u64;
        match i {
            id::CLUSTER => {
                pos = cluster(&mut s, dp, sz, &mut raws)?;
                if raws.len() > SAMPLES_MAX {
                    return Err(Error::Unsupported("more than 50 million frames"));
                }
                continue;
            }
            id::INFO | id::TRACKS => {
                let z = sz.ok_or(Error::Invalid("metadata of unknown size"))?;
                if z > META_MAX {
                    return Err(Error::Unsupported("Matroska header over 64 MB"));
                }
                let b = s.get(dp, z as usize)?.to_vec();
                if i == id::INFO {
                    for (i, v) in children(&b) {
                        match i {
                            id::TIMESTAMP_SCALE => scale = uint(v).max(1),
                            id::DURATION => duration = float(v),
                            _ => {}
                        }
                    }
                } else {
                    tracks.extend(parse_tracks(&b));
                }
            }
            _ => {}
        }
        match sz {
            Some(z) => pos = dp.saturating_add(z),
            None => break,
        }
    }

    let ticks = (1_000_000_000f64 / scale as f64).round().max(1.0) as u32;
    let mut out_tracks = Vec::new();
    for t in &tracks {
        if !t.enabled && tracks.len() > 1 {
            continue;
        }
        let kind = match t.kind {
            1 => Kind::Video,
            2 => Kind::Audio,
            _ => Kind::Other,
        };
        let dd = (t.default_duration_ns as f64 / scale as f64).round() as i64;
        // players start the sound CodecDelay (Opus pre-skip, AAC priming) before the block time
        let delay = if kind == Kind::Audio { (t.codec_delay_ns as f64 / scale as f64).round() as i64 } else { 0 };
        let mut samples = Vec::new();
        if !t.encoded && kind != Kind::Other {
            for r in raws.iter().filter(|r| r.track == t.number) {
                let ts = if r.lace.1 <= 1 || r.lace.0 == 0 {
                    r.ts
                } else if dd > 0 {
                    r.ts + r.lace.0 as i64 * dd
                } else if let Some(d) = r.duration {
                    r.ts + d * r.lace.0 as i64 / r.lace.1 as i64
                } else {
                    r.ts
                };
                let ts = ts - delay;
                samples.push(Sample { offset: r.offset, size: r.size, dts: ts, pts: ts, key: kind != Kind::Video || r.key });
            }
        }
        if kind == Kind::Video {
            // decode order is file order; decode times are the presentation times sorted
            let mut times: Vec<i64> = samples.iter().map(|s| s.pts).collect();
            times.sort_unstable();
            for (s, d) in samples.iter_mut().zip(times) {
                s.dts = d;
            }
        }
        let rate = t.rate.round() as u32;
        out_tracks.push(Track {
            id: t.number as u32,
            kind,
            fourcc: fourcc_of(&t.codec_id),
            codec: if t.encoded { Codec::Other } else { codec_of(t) },
            timescale: ticks,
            width: t.width,
            height: t.height,
            rotation: 0,
            channels: if kind == Kind::Audio { t.channels } else { 0 },
            sample_rate: if kind == Kind::Audio { rate } else { 0 },
            samples,
            colour: t.colour,
            default: t.default,
            codec_delay_ns: t.codec_delay_ns,
            discard_padding_ns: if kind == Kind::Audio { raws.iter().rev().find(|r| r.track == t.number).and_then(|r| r.discard).unwrap_or(0).max(0) as u64 } else { 0 },
        });
    }
    if out_tracks.iter().all(|t| t.samples.is_empty()) && raws.is_empty() && tracks.is_empty() {
        return Err(Error::Invalid("no tracks"));
    }
    let mut dur = (duration.max(0.0)) as u64;
    if dur == 0 {
        dur = out_tracks.iter().flat_map(|t| t.samples.iter().map(|s| s.pts.max(0) as u64)).max().unwrap_or(0);
    }
    Ok(Movie { timescale: ticks, duration: dur, tracks: out_tracks })
}
