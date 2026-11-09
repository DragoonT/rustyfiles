//! Audio of an MP4 / MOV file as interleaved `f32` PCM.
//!
//! * PCM tracks (`lpcm`, `sowt`, `twos`, `in24`, `in32`, `fl32`, `fl64`, `ipcm`, `fpcm`; 8 / 16 /
//!   24 / 32 bit integer, 32 / 64 bit float, either byte order) are converted here. Integers go
//!   to `x / 2^(bits-1)`, which is exact in `f32` up to 24 bits.
//! * AAC goes through Windows' own AAC decoder (`CMSAACDecMFT`, a Media Foundation transform):
//!   raw AAC frames in, the AudioSpecificConfig from `esds` as `MF_MT_USER_DATA`, float PCM
//!   out. This is the "OS decoder at first" of the design document; an own AAC decoder can
//!   replace [`AacMft`] later without touching the rest.
//!
//! Times follow the track's edit list the way [`crate::mp4`] applies it (an AAC encoder's delay
//! is an edit that skips the first 1024 samples, so the first packet starts at a negative
//! time). The decoder drops every frame before time 0 (and before a seek target), so the first
//! sample it returns is the sample at that time.

use std::fs::File;
use std::path::Path;

use crate::mp4::{Codec, Track};
use crate::{Error, Result};

/// Frames per block at most (a PCM chunk can be a second long).
const BLOCK_FRAMES: usize = 4096;

/// What a track's audio is like.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub codec: &'static str,
    /// Seconds from the track's first to its last sample time plus the last packet (approximate
    /// for AAC, exact for PCM).
    pub duration: f64,
}

/// A run of decoded audio: `samples.len() / channels` frames starting at `pts` seconds.
#[derive(Debug, Clone)]
pub struct AudioBlock {
    pub pts: f64,
    pub samples: Vec<f32>,
}

enum Kind_ {
    Pcm { bits: u16, float: bool, big_endian: bool },
    #[cfg(windows)]
    Aac(Box<AacMft>),
}

/// Reads the first audio track of a file and decodes it.
pub struct AudioDecoder {
    file: File,
    track: Track,
    info: AudioInfo,
    kind: Kind_,
    /// Next sample (packet / chunk) to read.
    next: usize,
    /// Frames before this are dropped (frame index in the output rate, 0 = time 0).
    skip_frame: i64,
    /// Frame index of the next frame the decoder will output.
    cursor: i64,
    cursor_set: bool,
    drained: bool,
    pending: Vec<f32>,
    pending_start: i64,
    pending_off: usize,
    buf: Vec<u8>,
}

impl AudioDecoder {
    /// Opens the first audio track of `path`. `Ok(None)` when the file has none.
    pub fn open(path: &Path) -> Result<Option<Self>> {
        let mut file = File::open(path)?;
        let movie = crate::container::read(&mut file)?;
        let Some(track) = movie.audio().cloned() else { return Ok(None) };
        Self::from_track(file, track).map(Some)
    }

    fn from_track(file: File, track: Track) -> Result<Self> {
        let mut sample_rate = track.sample_rate;
        let channels = track.channels.max(1);
        let (kind, codec) = match &track.codec {
            Codec::Pcm { bits, float, big_endian } => {
                if !matches!((*bits, *float), (8 | 16 | 24 | 32, false) | (32 | 64, true)) {
                    return Err(Error::Unsupported("PCM sample size"));
                }
                (Kind_::Pcm { bits: *bits, float: *float, big_endian: *big_endian }, "PCM")
            }
            #[cfg(windows)]
            Codec::Aac { config } => {
                let mft = AacMft::new(config, sample_rate, channels)?;
                sample_rate = mft.rate;
                (Kind_::Aac(Box::new(mft)), "AAC")
            }
            _ => return Err(Error::Unsupported("audio codec other than PCM and AAC")),
        };
        if sample_rate == 0 {
            return Err(Error::Invalid("audio sample rate"));
        }
        let channels = match &kind {
            #[cfg(windows)]
            Kind_::Aac(m) => m.channels,
            _ => channels,
        };
        let duration = {
            let last = track.samples.last();
            match (&kind, last) {
                (Kind_::Pcm { bits, .. }, Some(l)) => {
                    let fb = (*bits as u64 / 8) * channels as u64;
                    (l.pts.max(0) as f64 + (l.size as u64 / fb.max(1)) as f64) / track.timescale.max(1) as f64
                }
                (_, Some(l)) => track.seconds(l.pts) + 1024.0 / sample_rate as f64,
                _ => 0.0,
            }
        };
        let info = AudioInfo { sample_rate, channels, codec, duration };
        let mut d = AudioDecoder {
            file,
            track,
            info,
            kind,
            next: 0,
            skip_frame: 0,
            cursor: 0,
            cursor_set: false,
            drained: false,
            pending: Vec::new(),
            pending_start: 0,
            pending_off: 0,
            buf: Vec::new(),
        };
        d.seek(0.0);
        Ok(d)
    }

    pub fn info(&self) -> &AudioInfo {
        &self.info
    }

    pub fn track(&self) -> &Track {
        &self.track
    }

    /// Time of the first sample in seconds (before trimming to 0 for AAC this is negative).
    pub fn first_pts(&self) -> f64 {
        self.track.samples.first().map_or(0.0, |s| self.track.seconds(s.pts))
    }

    /// Moves to time `t` seconds: the next block starts at the frame at `t` (or at the first
    /// frame when `t` is before the start). AAC restarts two packets earlier to prime the
    /// decoder; what it outputs before `t` is dropped.
    pub fn seek(&mut self, t: f64) {
        let t = t.max(0.0);
        let rate = self.info.sample_rate as f64;
        self.skip_frame = (t * rate).round() as i64;
        let ts = self.track.timescale.max(1) as f64;
        let target = (t * ts).floor() as i64;
        // last packet whose time is not after the target
        let idx = self.track.samples.partition_point(|s| s.pts <= target).saturating_sub(1);
        self.next = match self.kind {
            Kind_::Pcm { .. } => idx,
            #[cfg(windows)]
            Kind_::Aac(_) => idx.saturating_sub(2),
        };
        self.pending.clear();
        self.pending_off = 0;
        self.cursor_set = false;
        self.drained = false;
        #[cfg(windows)]
        if let Kind_::Aac(m) = &mut self.kind {
            m.flush();
        }
    }

    /// The next block of audio, or `None` at the end.
    pub fn next_block(&mut self) -> Result<Option<AudioBlock>> {
        let ch = self.info.channels as usize;
        loop {
            let have = (self.pending.len() / ch).saturating_sub(self.pending_off);
            if have == 0 {
                if !self.refill()? {
                    return Ok(None);
                }
                continue;
            }
            let start = self.pending_start + self.pending_off as i64;
            let n = have.min(BLOCK_FRAMES);
            if start + (n as i64) <= self.skip_frame {
                self.pending_off += n;
                continue;
            }
            let cut = (self.skip_frame - start).max(0) as usize;
            let from = self.pending_off + cut;
            let n = n - cut;
            let samples = self.pending[from * ch..(from + n) * ch].to_vec();
            self.pending_off = from + n;
            return Ok(Some(AudioBlock { pts: (start + cut as i64) as f64 / self.info.sample_rate as f64, samples }));
        }
    }

    /// Everything from the current position as one vector, with the time of its first frame.
    pub fn decode_all(&mut self) -> Result<(f64, Vec<f32>)> {
        let mut first = None;
        let mut out = Vec::new();
        while let Some(b) = self.next_block()? {
            first.get_or_insert(b.pts);
            out.extend_from_slice(&b.samples);
        }
        Ok((first.unwrap_or(0.0), out))
    }

    /// Fills `pending` with the next decoded run; `false` at the end.
    fn refill(&mut self) -> Result<bool> {
        self.pending.clear();
        self.pending_off = 0;
        let rate = self.info.sample_rate as i128;
        let ts = self.track.timescale.max(1) as i128;
        match &mut self.kind {
            Kind_::Pcm { bits, float, big_endian } => {
                let Some(s) = self.track.samples.get(self.next).copied() else { return Ok(false) };
                self.track.read_sample(&mut self.file, self.next, &mut self.buf)?;
                self.next += 1;
                let derived = ((s.pts as i128 * rate + ts / 2).div_euclid(ts)) as i64;
                // blocks follow each other; a time rounded to the container's tick (1 ms in
                // Matroska) must not open a gap or an overlap
                let slack = (rate / ts) as i64 + 1;
                self.pending_start = if self.cursor_set && (derived - self.cursor).abs() <= slack { self.cursor } else { derived };
                self.cursor_set = true;
                convert_pcm(&self.buf, *bits, *float, *big_endian, &mut self.pending);
                let ch = self.info.channels as usize;
                self.pending.truncate(self.pending.len() / ch * ch);
                self.cursor = self.pending_start + (self.pending.len() / ch) as i64;
                Ok(true)
            }
            #[cfg(windows)]
            Kind_::Aac(m) => loop {
                if let Some(pcm) = m.pull()? {
                    self.pending_start = self.cursor;
                    self.cursor += (pcm.len() / self.info.channels as usize) as i64;
                    self.pending = pcm;
                    return Ok(true);
                }
                if self.drained {
                    return Ok(false);
                }
                let Some(s) = self.track.samples.get(self.next).copied() else {
                    m.drain();
                    self.drained = true;
                    continue;
                };
                self.track.read_sample(&mut self.file, self.next, &mut self.buf)?;
                if !self.cursor_set {
                    self.cursor = ((s.pts as i128 * rate + ts / 2).div_euclid(ts)) as i64;
                    if self.next == 0 && self.track.codec_delay_ns > 0 {
                        // Matroska: the first packet starts CodecDelay before 0, exactly
                        self.cursor = -((self.track.codec_delay_ns as i128 * rate + 500_000_000) / 1_000_000_000) as i64;
                    }
                    self.cursor_set = true;
                }
                self.next += 1;
                m.push(&self.buf)?;
            },
        }
    }
}

/// Raw PCM bytes to `f32`, appended to `out`.
pub fn convert_pcm(b: &[u8], bits: u16, float: bool, big: bool, out: &mut Vec<f32>) {
    match (bits, float) {
        (8, false) => out.extend(b.iter().map(|&x| x as i8 as f32 / 128.0)),
        (16, false) => out.extend(b.chunks_exact(2).map(|c| {
            let v = if big { i16::from_be_bytes([c[0], c[1]]) } else { i16::from_le_bytes([c[0], c[1]]) };
            v as f32 / 32768.0
        })),
        (24, false) => out.extend(b.chunks_exact(3).map(|c| {
            let v = if big { (c[0] as i32) << 16 | (c[1] as i32) << 8 | c[2] as i32 } else { (c[2] as i32) << 16 | (c[1] as i32) << 8 | c[0] as i32 };
            ((v << 8) >> 8) as f32 / 8_388_608.0
        })),
        (32, false) => out.extend(b.chunks_exact(4).map(|c| {
            let a = [c[0], c[1], c[2], c[3]];
            let v = if big { i32::from_be_bytes(a) } else { i32::from_le_bytes(a) };
            (v as f64 / 2_147_483_648.0) as f32
        })),
        (32, true) => out.extend(b.chunks_exact(4).map(|c| {
            let a = [c[0], c[1], c[2], c[3]];
            if big { f32::from_be_bytes(a) } else { f32::from_le_bytes(a) }
        })),
        (64, true) => out.extend(b.chunks_exact(8).map(|c| {
            let a: [u8; 8] = c.try_into().unwrap();
            (if big { f64::from_be_bytes(a) } else { f64::from_le_bytes(a) }) as f32
        })),
        _ => {}
    }
}

// ---------------------------------------------------------------------------------------------
// AAC through the OS decoder

#[cfg(windows)]
pub use mft::AacMft;

#[cfg(windows)]
mod mft {
    use std::sync::Once;

    use windows::Win32::Media::MediaFoundation::*;
    use windows::Win32::System::Com::*;
    use windows::core::GUID;

    use crate::{Error, Result};

    /// `CLSID_CMSAACDecMFT`
    const CLSID_AAC_DEC: GUID = GUID::from_u128(0x32d186a7_218f_4c75_8876_dd77273a8999);

    fn gpu_err(what: &str, e: windows::core::Error) -> Error {
        Error::Gpu(format!("AAC decoder: {what}: {e}"))
    }

    static START: Once = Once::new();

    /// COM (multithreaded, on this thread) and Media Foundation.
    fn startup() {
        START.call_once(|| unsafe {
            let _ = MFStartup(MF_VERSION, MFSTARTUP_FULL);
        });
        // an already initialised apartment of another kind is fine
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
    }

    /// The AAC decoder transform: packets in, interleaved `f32` out.
    pub struct AacMft {
        t: IMFTransform,
        out_sample: IMFSample,
        out_cap: u32,
        pub rate: u32,
        pub channels: u16,
        pcm16: bool,
    }

    // SAFETY: the transform is free threaded (both-threaded COM class) and is only ever used
    // from one thread at a time: the decoder owning it moves between threads, never shares.
    unsafe impl Send for AacMft {}

    impl AacMft {
        /// `config` is the AudioSpecificConfig; `rate` and `channels` come from the sample
        /// entry.
        pub fn new(config: &[u8], rate: u32, channels: u16) -> Result<Self> {
            startup();
            unsafe {
                let t: IMFTransform = CoCreateInstance(&CLSID_AAC_DEC, None, CLSCTX_INPROC_SERVER).map_err(|e| gpu_err("create", e))?;
                let ty = MFCreateMediaType().map_err(|e| gpu_err("media type", e))?;
                let e = |w: &'static str| move |e| gpu_err(w, e);
                ty.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio).map_err(e("major"))?;
                ty.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_AAC).map_err(e("subtype"))?;
                ty.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, rate).map_err(e("rate"))?;
                ty.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, channels as u32).map_err(e("channels"))?;
                ty.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16).map_err(e("bits"))?;
                ty.SetUINT32(&MF_MT_AAC_PAYLOAD_TYPE, 0).map_err(e("payload"))?;
                ty.SetUINT32(&MF_MT_AAC_AUDIO_PROFILE_LEVEL_INDICATION, 0x29).map_err(e("profile"))?;
                // HEAACWAVEINFO's tail (payload type, profile level, structure type, 2 reserved
                // words), then the AudioSpecificConfig
                let mut user = vec![0u8; 12];
                user[2..4].copy_from_slice(&0x29u16.to_le_bytes());
                user.extend_from_slice(config);
                ty.SetBlob(&MF_MT_USER_DATA, &user).map_err(e("user data"))?;
                t.SetInputType(0, &ty, 0).map_err(e("input type"))?;
                // output: float if offered, else 16-bit
                let (mut pick, mut pick16) = (None, None);
                let mut i = 0;
                while let Ok(o) = t.GetOutputAvailableType(0, i) {
                    i += 1;
                    match o.GetGUID(&MF_MT_SUBTYPE) {
                        Ok(g) if g == MFAudioFormat_Float => {
                            pick = Some(o);
                            break;
                        }
                        Ok(g) if g == MFAudioFormat_PCM && pick16.is_none() => pick16 = Some(o),
                        _ => {}
                    }
                }
                let pcm16 = pick.is_none();
                let o = pick.or(pick16).ok_or(Error::Unsupported("AAC decoder offers no PCM output"))?;
                t.SetOutputType(0, &o, 0).map_err(e("output type"))?;
                let out_rate = o.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(rate);
                let out_ch = o.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(channels as u32);
                t.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(e("begin"))?;
                t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(e("start"))?;
                let info = t.GetOutputStreamInfo(0).map_err(e("stream info"))?;
                let out_cap = info.cbSize.max(out_ch * 4 * 4096);
                let out_sample = MFCreateSample().map_err(e("sample"))?;
                let buf = MFCreateMemoryBuffer(out_cap).map_err(e("buffer"))?;
                out_sample.AddBuffer(&buf).map_err(e("add buffer"))?;
                Ok(AacMft { t, out_sample, out_cap, rate: out_rate, channels: out_ch as u16, pcm16 })
            }
        }

        /// Gives the decoder one raw AAC frame.
        pub fn push(&mut self, packet: &[u8]) -> Result<()> {
            unsafe {
                let buf = MFCreateMemoryBuffer(packet.len().max(1) as u32).map_err(|e| gpu_err("buffer", e))?;
                let mut p = std::ptr::null_mut();
                buf.Lock(&mut p, None, None).map_err(|e| gpu_err("lock", e))?;
                std::ptr::copy_nonoverlapping(packet.as_ptr(), p, packet.len());
                buf.Unlock().map_err(|e| gpu_err("unlock", e))?;
                buf.SetCurrentLength(packet.len() as u32).map_err(|e| gpu_err("length", e))?;
                let s = MFCreateSample().map_err(|e| gpu_err("sample", e))?;
                s.AddBuffer(&buf).map_err(|e| gpu_err("add buffer", e))?;
                self.t.ProcessInput(0, &s, 0).map_err(|e| gpu_err("input", e))
            }
        }

        /// The decoder will get no more packets: let it release what it holds.
        pub fn drain(&mut self) {
            unsafe {
                let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
            }
        }

        pub fn flush(&mut self) {
            unsafe {
                let _ = self.t.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
                let _ = self.t.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0);
            }
        }

        /// One run of decoded audio, interleaved `f32`; `None` when it wants more input.
        pub fn pull(&mut self) -> Result<Option<Vec<f32>>> {
            for _ in 0..4 {
                unsafe {
                    if let Ok(b) = self.out_sample.GetBufferByIndex(0) {
                        let _ = b.SetCurrentLength(0);
                    }
                    let mut out = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: std::mem::ManuallyDrop::new(Some(self.out_sample.clone())), dwStatus: 0, pEvents: std::mem::ManuallyDrop::new(None) }];
                    let mut status = 0u32;
                    let r = self.t.ProcessOutput(0, &mut out, &mut status);
                    // take the references back so they are released
                    let [o] = out;
                    drop(std::mem::ManuallyDrop::into_inner(o.pSample));
                    drop(std::mem::ManuallyDrop::into_inner(o.pEvents));
                    match r {
                        Ok(()) => {
                            let buf = self.out_sample.ConvertToContiguousBuffer().map_err(|e| gpu_err("output buffer", e))?;
                            let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
                            buf.Lock(&mut p, None, Some(&mut len)).map_err(|e| gpu_err("lock output", e))?;
                            let bytes = std::slice::from_raw_parts(p, len as usize).to_vec();
                            buf.Unlock().map_err(|e| gpu_err("unlock output", e))?;
                            let mut pcm = Vec::with_capacity(bytes.len() / 4);
                            if self.pcm16 {
                                super::convert_pcm(&bytes, 16, false, false, &mut pcm);
                            } else {
                                super::convert_pcm(&bytes, 32, true, false, &mut pcm);
                            }
                            if pcm.is_empty() {
                                continue;
                            }
                            return Ok(Some(pcm));
                        }
                        Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => return Ok(None),
                        Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                            let o = self.t.GetOutputAvailableType(0, 0).map_err(|e| gpu_err("stream change", e))?;
                            // keep float when it is what we had
                            let mut i = 0;
                            let mut chosen = None;
                            while let Ok(c) = self.t.GetOutputAvailableType(0, i) {
                                i += 1;
                                let want = if self.pcm16 { MFAudioFormat_PCM } else { MFAudioFormat_Float };
                                if c.GetGUID(&MF_MT_SUBTYPE).is_ok_and(|g| g == want) {
                                    chosen = Some(c);
                                    break;
                                }
                            }
                            let c = chosen.unwrap_or(o);
                            self.t.SetOutputType(0, &c, 0).map_err(|e| gpu_err("output type", e))?;
                            self.rate = c.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(self.rate);
                            self.channels = c.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(self.channels as u32) as u16;
                        }
                        Err(e) => return Err(gpu_err("output", e)),
                    }
                }
            }
            let _ = self.out_cap;
            Ok(None)
        }
    }
}
