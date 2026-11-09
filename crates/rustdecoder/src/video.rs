//! `VideoDecoder`: open a clip and get its pictures whatever the codec (H.264 or HEVC).

use std::fs::File;
use std::path::Path;

use crate::d3d11::{Gpu, Planes};
use crate::decoder::{Frame, HevcDecoder, StreamInfo};
use crate::h264dec::H264Decoder;
use crate::mp4::{self, Codec, Track};
use crate::{Error, Result};

pub enum VideoDecoder {
    H264(H264Decoder),
    Hevc(HevcDecoder),
}

macro_rules! each {
    ($s:expr, $d:ident => $e:expr) => {
        match $s {
            VideoDecoder::H264($d) => $e,
            VideoDecoder::Hevc($d) => $e,
        }
    };
}

impl VideoDecoder {
    /// Opens the first video track of an MP4 / MOV file with the decoder its codec needs.
    pub fn open(path: &Path) -> Result<Self> {
        let movie = mp4::read(&mut File::open(path)?)?;
        let track = movie.video().ok_or(Error::Invalid("no video track"))?;
        match track.codec {
            Codec::H264 { .. } => Ok(Self::H264(H264Decoder::open(path)?)),
            Codec::Hevc { .. } => Ok(Self::Hevc(HevcDecoder::open(path)?)),
            _ => Err(Error::Unsupported("video codec other than H.264 and HEVC")),
        }
    }

    pub fn codec_name(&self) -> &'static str {
        match self {
            Self::H264(_) => "H.264",
            Self::Hevc(_) => "HEVC",
        }
    }

    pub fn info(&self) -> &StreamInfo {
        each!(self, d => d.info())
    }

    /// The device the pictures live on.
    pub fn gpu(&self) -> &Gpu {
        each!(self, d => d.gpu())
    }

    /// The video track with its sample table.
    pub fn track(&self) -> &Track {
        each!(self, d => d.track())
    }

    /// Continues decoding at sync sample `i` (see the codec's `seek_to_sample`).
    pub fn seek_to_sample(&mut self, i: usize) {
        each!(self, d => d.seek_to_sample(i))
    }

    pub fn adapter(&self) -> &str {
        each!(self, d => d.adapter())
    }

    pub fn config_bitstream_raw(&self) -> Option<u32> {
        each!(self, d => d.config_bitstream_raw())
    }

    pub fn wait_idle(&self) -> Result<()> {
        each!(self, d => d.wait_idle())
    }

    /// The next picture in display order; `None` at the end of the stream. A frame stays valid
    /// until the next call.
    pub fn next_frame(&mut self) -> Result<Option<Frame>> {
        each!(self, d => d.next_frame())
    }

    pub fn read_back(&mut self, f: &Frame) -> Result<Planes> {
        each!(self, d => d.read_back(f))
    }
}
