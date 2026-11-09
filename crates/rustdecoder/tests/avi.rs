//! The AVI reader against ffprobe, plus hand-built files for what ffmpeg's muxer does not write
//! at this size (OpenDML with several `RIFF AVIX` parts, files without an index).
//!
//! What is compared with `ffprobe -show_packets`:
//! - count, sync flag, size and CRC32 of every packet's chunk bytes (for Annex B H.264 the chunk
//!   as stored; `Sample::size` is the length prefixed size, whose NAL lengths are checked to
//!   fill the sample);
//! - decode times: ffprobe's `dts` (AVI has no `pts`) in the stream's time base equals the
//!   sample's `dts` and `pts` for streams without B-frames. With B-frames ffprobe shows only the
//!   chunk order; there `pts` has to be the display order (a dense run of slots, `dts <= pts`)
//!   and the GPU decoder's pictures are compared with ffmpeg's in display order.

mod common;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use common::*;
use rustdecoder::mp4::{Codec, Kind, Storage, Track};

const H264: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-x264-params", "keyint=30:bframes=0"];
const H264_B: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-x264-params", "keyint=30:bframes=3"];
const AVI: &[&str] = &["-f", "avi"];

fn raw_bytes(f: &mut File, t: &Track, i: usize) -> Vec<u8> {
    let s = t.samples[i];
    let n = match &t.storage {
        Storage::AnnexB(a) => a.raw[i],
        _ => s.size,
    } as usize;
    let mut b = vec![0u8; n];
    f.seek(SeekFrom::Start(s.offset)).unwrap();
    f.read_exact(&mut b).unwrap();
    b
}

fn check_track(clip: &Path, t: &Track, stream: &str, times: bool) -> usize {
    let want = ffprobe_full(clip, stream);
    assert_eq!(t.samples.len(), want.len(), "{}: {stream} packet count", clip.display());
    let mut f = File::open(clip).unwrap();
    for (i, (s, w)) in t.samples.iter().zip(&want).enumerate() {
        assert_eq!(s.key, w.key, "{stream} packet {i}: key");
        let raw = raw_bytes(&mut f, t, i);
        assert_eq!(raw.len() as u32, w.size, "{stream} packet {i}: size");
        assert_eq!(crc32fast::hash(&raw), w.crc, "{stream} packet {i}: CRC32");
        if times {
            assert_eq!(s.dts, w.dts, "{stream} packet {i}: dts");
            assert_eq!(s.pts, w.dts, "{stream} packet {i}: pts");
        }
    }
    want.len()
}

fn check_nal_lengths(clip: &Path, t: &Track) {
    let mut f = File::open(clip).unwrap();
    let mut buf = Vec::new();
    for i in 0..t.samples.len() {
        t.read_sample(&mut f, i, &mut buf).unwrap();
        assert_eq!(buf.len() as u32, t.samples[i].size, "sample {i}: size");
        let mut p = 0usize;
        while p < buf.len() {
            let n = u32::from_be_bytes(buf[p..p + 4].try_into().unwrap()) as usize;
            p += 4 + n;
        }
        assert_eq!(p, buf.len(), "sample {i}: NAL units do not fill the sample");
    }
}

fn open(clip: &Path) -> rustdecoder::mp4::Movie {
    rustdecoder::container::read(&mut File::open(clip).unwrap()).unwrap()
}

fn check_clip(name: &str, video: &[&str], audio: &[&str], a_ok: fn(&Codec) -> bool, a_fmt: (u32, u16), b_frames: bool) {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return;
    }
    let clip = make_av(name, video, audio, AVI);
    let movie = open(&clip);
    let v = movie.tracks.iter().find(|t| t.kind == Kind::Video).expect("video track");
    assert!(matches!(v.codec, Codec::H264 { .. }), "{name}: {:?}", v.codec);
    assert_eq!((v.width, v.height), (640, 360));
    let nv = check_track(&clip, v, "v:0", !b_frames);
    check_nal_lengths(&clip, v);
    if b_frames {
        let mut pts: Vec<i64> = v.samples.iter().map(|s| s.pts).collect();
        assert!(v.samples.iter().all(|s| s.dts <= s.pts), "dts after pts");
        pts.sort_unstable();
        let step = v.samples.iter().map(|s| s.dts).max().unwrap() / (v.samples.len() as i64 - 1).max(1);
        assert!(pts.windows(2).all(|w| w[1] - w[0] == step.max(1)), "display slots are not a dense run: {:?}", &pts[..10]);
        assert_eq!(pts[0], 0);
    }
    let a = movie.tracks.iter().find(|t| t.kind == Kind::Audio).expect("audio track");
    assert!(a_ok(&a.codec), "{name}: audio {:?}", a.codec);
    assert_eq!((a.sample_rate, a.channels), a_fmt);
    let na = check_track(&clip, a, "a:0", false);
    // audio times: the end of the last chunk is the length of the sound (3 s)
    let last = a.samples.last().unwrap();
    assert!(a.seconds(last.pts) <= 3.1 && a.seconds(last.pts) > 2.5, "{name}: last audio chunk at {}", a.seconds(last.pts));
    eprintln!("{name}: {nv} video + {na} audio packets match ffprobe (timescale v {} a {}), movie {:.2} s", v.timescale, a.timescale, movie.duration_secs());
}

#[test]
fn h264_pcm() {
    check_clip("a_h264_pcm.avi", H264, &["-c:a", "pcm_s16le"], |c| matches!(c, Codec::Pcm { bits: 16, float: false, big_endian: false }), (48000, 1), false);
}

#[test]
fn h264_mp3_with_b_frames() {
    check_clip("a_h264_mp3_b.avi", H264_B, &["-c:a", "libmp3lame", "-b:a", "128k"], |c| matches!(c, Codec::Mp3), (48000, 1), true);
}

#[test]
fn mjpeg_is_listed() {
    if !have_ffmpeg() {
        return;
    }
    let clip = make_av("a_mjpeg_pcm.avi", &["-c:v", "mjpeg", "-pix_fmt", "yuvj420p"], &["-c:a", "pcm_s16le"], AVI);
    let movie = open(&clip);
    let v = movie.tracks.iter().find(|t| t.kind == Kind::Video).unwrap();
    assert_eq!(&v.fourcc, b"MJPG");
    assert!(matches!(v.codec, Codec::Other));
    assert_eq!((v.width, v.height), (640, 360));
    check_track(&clip, v, "v:0", true);
    assert!(v.samples.iter().all(|s| s.key));
}

#[cfg(windows)]
mod decode {
    use super::*;

    fn decode(name: &str, video: &[&str], audio: &[&str]) {
        if !have_ffmpeg() {
            return;
        }
        let clip = make_av(name, video, audio, AVI);
        let Some((n, first)) = ffcompare::compare_decode(&clip, 100_000) else { return };
        eprintln!("{name}: {n} frames decoded on the GPU and compared with ffmpeg: {first:?}");
        assert_eq!(n, 90, "{name}: frame count");
        assert_eq!(first, None, "{name}");
    }

    #[test]
    fn h264_pcm() {
        decode("a_h264_pcm.avi", H264, &["-c:a", "pcm_s16le"]);
    }

    #[test]
    fn h264_mp3_with_b_frames() {
        decode("a_h264_mp3_b.avi", H264_B, &["-c:a", "libmp3lame", "-b:a", "128k"]);
    }
}
