//! Fragmented MP4 (`moof` + `mdat`) against ffprobe: every packet's size, presentation and
//! decode time and keyframe flag has to match, for the ways ffmpeg's muxer fragments a file
//! (and for a copy that was cut in the middle of its last `mdat`).

mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::*;
use rustdecoder::mp4::{Kind, Track};

#[derive(Debug, PartialEq, Eq, Clone)]
struct Pkt {
    pts: i64,
    dts: i64,
    size: u32,
    key: bool,
}

/// Packets as ffprobe lists them, with their file position.
fn ffprobe_pos(clip: &Path, stream: &str) -> Vec<(Pkt, u64)> {
    let o = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", stream, "-show_entries", "packet=pts,dts,size,flags,pos", "-of", "compact"])
        .arg(clip)
        .output()
        .expect("ffprobe");
    assert!(o.status.success(), "ffprobe failed");
    String::from_utf8_lossy(&o.stdout)
        .lines()
        .filter(|l| l.starts_with("packet"))
        .map(|l| {
            let get = |k: &str| l.split('|').find_map(|f| f.strip_prefix(&format!("{k}="))).unwrap_or("");
            (
                Pkt { pts: get("pts").parse().unwrap(), dts: get("dts").parse().unwrap(), size: get("size").parse().unwrap(), key: get("flags").starts_with('K') },
                get("pos").parse().unwrap(),
            )
        })
        .collect()
}

fn ffprobe(clip: &Path, stream: &str) -> Vec<Pkt> {
    ffprobe_pos(clip, stream).into_iter().map(|p| p.0).collect()
}

const H264: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-x264-params", "log-level=error:keyint=15:min-keyint=15:bframes=3:scenecut=0"];
const HEVC: &[&str] = &["-c:v", "libx265", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-x265-params", "log-level=error:keyint=15:min-keyint=15"];

/// Makes (once) a 3 s H.264 / HEVC + AAC clip with the given muxer arguments.
fn make(name: &str, video: &[&str], mux: &[&str]) -> PathBuf {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = temp_dir().join(name);
    if out.exists() {
        return out;
    }
    let tmp = out.with_file_name(format!("part-{name}"));
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-f", "lavfi", "-i", "sine=f=440:r=48000:d=3", "-t", "3"])
        .args(video)
        .args(["-c:a", "aac", "-b:a", "96k"])
        .args(mux)
        .args(["-f", "mp4"])
        .arg(&tmp)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg could not make {name}");
    std::fs::rename(&tmp, &out).unwrap();
    out
}

fn compare_track(clip: &Path, t: &Track, stream: &str) -> usize {
    let mut want = ffprobe_pos(clip, stream);
    let got: Vec<Pkt> = t.samples.iter().map(|s| Pkt { pts: s.pts, dts: s.dts, size: s.size, key: s.key }).collect();
    // ffprobe hands out the partial packet a cut file ends with (its size is what is left);
    // we keep whole samples only
    let len = std::fs::metadata(clip).unwrap().len();
    if want.len() == got.len() + 1 && want.last().is_some_and(|(p, pos)| pos + p.size as u64 == len) {
        want.pop();
    }
    assert_eq!(got.len(), want.len(), "{}: {stream} packet count", clip.display());
    // Negative composition offsets (CMAF, `negative_cts_offsets`): the spec has the presentation
    // time as decode time + offset, ffmpeg moves every presentation time up by the most negative
    // offset so none is below its decode time. Decode times and everything else are the same,
    // presentation times differ by one constant, which is only allowed for those clips.
    let shift = want.first().map_or(0, |w| w.0.pts - got[0].pts);
    for (i, (g, w)) in got.iter().zip(&want).enumerate() {
        let g = Pkt { pts: g.pts + shift, ..g.clone() };
        assert_eq!(g, w.0, "{}: {stream} packet {i} (pts shift {shift})", clip.display());
    }
    if shift != 0 {
        eprintln!("{}: {stream}: our presentation times are {shift} below ffprobe's (negative composition offsets)", clip.display());
        assert!(clip.to_string_lossy().contains("cmaf") || clip.to_string_lossy().contains("negcts"));
    }
    got.len()
}

/// Compares both tracks with ffprobe; returns (video packets, audio packets, video keys).
fn check(clip: &Path) -> (usize, usize, usize) {
    let movie = rustdecoder::container::read(&mut File::open(clip).unwrap()).unwrap();
    let v = movie.tracks.iter().find(|t| t.kind == Kind::Video).expect("video track");
    let a = movie.tracks.iter().find(|t| t.kind == Kind::Audio).expect("audio track");
    assert_eq!((v.width, v.height), (640, 360));
    let nv = compare_track(clip, v, "v:0");
    let na = compare_track(clip, a, "a:0");
    // every sample lies inside the file and reads back whole
    let len = std::fs::metadata(clip).unwrap().len();
    for t in [v, a] {
        assert!(t.samples.iter().all(|s| s.offset + s.size as u64 <= len));
    }
    (nv, na, v.samples.iter().filter(|s| s.key).count())
}

fn run(name: &str, video: &[&str], mux: &[&str], want_video: usize) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return None;
    }
    let clip = make(name, video, mux);
    let (nv, na, keys) = check(&clip);
    eprintln!("{name}: {nv} video + {na} audio packets match ffprobe ({keys} keyframes)");
    assert_eq!(nv, want_video);
    Some(clip)
}

macro_rules! variants {
    ($($fn:ident, $name:literal, $mux:expr;)*) => {
        mod h264 { use super::*; $(#[test] fn $fn() { run(concat!("frag_h264_", $name, ".mp4"), H264, $mux, 90); })* }
        mod hevc { use super::*; $(#[test] fn $fn() { run(concat!("frag_hevc_", $name, ".mp4"), HEVC, $mux, 90); })* }
    };
}

variants! {
    frag_keyframe_empty_moov, "kf", &["-movflags", "frag_keyframe+empty_moov"];
    default_base_moof, "dbm", &["-movflags", "frag_keyframe+empty_moov+default_base_moof"];
    frag_duration, "dur", &["-movflags", "empty_moov", "-frag_duration", "500000"];
    dash, "dash", &["-movflags", "+dash"];
    separate_moof, "sep", &["-movflags", "frag_keyframe+empty_moov+separate_moof"];
    cmaf, "cmaf", &["-movflags", "+cmaf"];
    negative_cts, "negcts", &["-movflags", "frag_keyframe+empty_moov+negative_cts_offsets", "-avoid_negative_ts", "disabled"];
    mixed_moov_and_fragments, "mixed", &["-movflags", "frag_keyframe"];
    one_fragment_per_frame, "perframe", &["-movflags", "empty_moov+default_base_moof", "-frag_duration", "1"];
}

/// Top-level boxes as (start, size, type).
fn top_boxes(b: &[u8]) -> Vec<(usize, usize, [u8; 4])> {
    let mut out = Vec::new();
    let mut p = 0;
    while p + 8 <= b.len() {
        let size = u32::from_be_bytes(b[p..p + 4].try_into().unwrap()) as usize;
        out.push((p, size, b[p + 4..p + 8].try_into().unwrap()));
        p += size.max(8);
    }
    out
}

#[test]
fn cut_in_the_middle_of_the_last_mdat() {
    let Some(clip) = run("frag_h264_kf.mp4", H264, &["-movflags", "frag_keyframe+empty_moov"], 90) else { return };
    let b = std::fs::read(&clip).unwrap();
    let boxes = top_boxes(&b);
    let (start, size, _) = *boxes.iter().rev().find(|x| &x.2 == b"mdat").unwrap();
    let cut = temp_dir().join("frag_h264_kf_cut.mp4");
    std::fs::write(&cut, &b[..start + 8 + (size - 8) / 2]).unwrap();
    let (nv, na, _) = check(&cut);
    eprintln!("cut in the last mdat: {nv} video + {na} audio packets match ffprobe");
    assert!(nv < 90 && nv > 60);
    // and cut inside the last moof: that fragment is dropped
    let (mstart, msize, _) = *boxes.iter().rev().find(|x| &x.2 == b"moof").unwrap();
    let cut2 = temp_dir().join("frag_h264_kf_cut_moof.mp4");
    std::fs::write(&cut2, &b[..mstart + msize / 2]).unwrap();
    let movie = rustdecoder::container::read(&mut File::open(&cut2).unwrap()).unwrap();
    let v = movie.video().unwrap();
    eprintln!("cut in the last moof: {} video packets, ffprobe says {}", v.samples.len(), ffprobe(&cut2, "v:0").len());
    assert_eq!(v.samples.len(), ffprobe(&cut2, "v:0").len());
}

#[test]
fn cut_mixed_file() {
    let Some(clip) = run("frag_h264_mixed.mp4", H264, &["-movflags", "frag_keyframe"], 90) else { return };
    let b = std::fs::read(&clip).unwrap();
    let boxes = top_boxes(&b);
    let (start, size, _) = *boxes.iter().rev().find(|x| &x.2 == b"mdat").unwrap();
    let cut = temp_dir().join("frag_h264_mixed_cut.mp4");
    std::fs::write(&cut, &b[..start + 8 + (size - 8) / 2]).unwrap();
    let (nv, na, _) = check(&cut);
    eprintln!("cut mixed file: {nv} video + {na} audio packets match ffprobe");
}

#[test]
fn plain_files_are_unchanged() {
    // a file without fragments reads exactly as before (tests/mp4.rs covers its numbers)
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/h264_aac.mp4");
    let m = rustdecoder::mp4::read(&mut File::open(p).unwrap()).unwrap();
    assert_eq!(m.video().unwrap().samples.len(), 25);
}
