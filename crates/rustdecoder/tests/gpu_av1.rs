//! AV1 GPU decoding compared with ffmpeg's software decoder (libdav1d): every sample of every
//! shown frame, in order, has to match. Clips are made with ffmpeg + SVT-AV1 / libaom into a temp
//! folder (nothing is kept in git); tests skip with a message when ffmpeg or a GPU decoder is
//! missing.

#![cfg(windows)]

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use common::{have_ffmpeg, temp_dir};
use rustdecoder::VideoDecoder;

struct Clip<'a> {
    name: &'a str,
    size: &'a str,
    secs: u32,
    pix_fmt: &'a str,
    /// Encoder name and the arguments after it.
    encoder: &'a str,
    args: &'a [&'a str],
    /// Test source filter chain after the noise (`clean` for no noise at all).
    vf: &'a str,
}

impl<'a> Clip<'a> {
    fn svt(name: &'a str, args: &'a [&'a str]) -> Self {
        Self { name, size: "640x360", secs: 3, pix_fmt: "yuv420p", encoder: "libsvtav1", args, vf: "" }
    }
}

/// Makes (once) a clip of noisy test pattern; the container comes from the extension.
fn make_clip(c: &Clip) -> PathBuf {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = temp_dir().join(c.name);
    if out.exists() {
        return out;
    }
    let ext = out.extension().unwrap().to_string_lossy().to_string();
    let tmp = out.with_file_name(format!("part-{}", c.name));
    let vf = match c.vf {
        "" => "noise=alls=6:allf=t".to_string(),
        "clean" => "null".to_string(),
        f => format!("noise=alls=6:allf=t,{f}"),
    };
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size={}:rate=30", c.size))
        .args(["-t", &c.secs.to_string(), "-vf", &vf, "-pix_fmt", c.pix_fmt, "-c:v", c.encoder])
        .args(c.args)
        .args(["-f", if ext == "mp4" { "mp4" } else { "webm" }])
        .arg(&tmp)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg could not make {}", c.name);
    std::fs::rename(&tmp, &out).unwrap();
    out
}

#[derive(Debug)]
struct Report {
    frames: usize,
    first_mismatch: Option<String>,
    fps: f64,
    sizes: Vec<(u32, u32)>,
}

/// Decodes on the GPU and compares each frame with ffmpeg's output, read from a pipe as it is
/// produced. `None` when there is no GPU decoder for the clip.
fn compare(clip: &Path) -> Option<Report> {
    let mut dec = match VideoDecoder::open(clip) {
        Ok(d) => d,
        Err(rustdecoder::Error::Gpu(m)) => {
            eprintln!("skipped: {m}");
            return None;
        }
        Err(e) => panic!("open {}: {e}", clip.display()),
    };
    let info = dec.info().clone();
    let ten = info.bit_depth == 10;
    let mut ff = Command::new("ffmpeg")
        .args(["-v", "error", "-noautorotate", "-c:v", "libdav1d", "-i"])
        .arg(clip)
        .args(["-noautoscale", "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", if ten { "p010le" } else { "nv12" }, "-"])
        .stdout(Stdio::piped())
        .spawn()
        .expect("ffmpeg");
    let mut out = ff.stdout.take().unwrap();
    let mut report = Report { frames: 0, first_mismatch: None, fps: 0.0, sizes: Vec::new() };
    let t0 = std::time::Instant::now();
    let bytes = if ten { 2 } else { 1 };
    let mut ref_frame = Vec::new();
    loop {
        let Some(f) = dec.next_frame().unwrap() else { break };
        let planes = dec.read_back(&f).unwrap();
        let (w, h) = (planes.width as usize, planes.height as usize);
        if report.sizes.last() != Some(&(w as u32, h as u32)) {
            report.sizes.push((w as u32, h as u32));
        }
        let y_len = w * h * bytes;
        ref_frame.resize(y_len + w * (h / 2) * bytes, 0);
        if out.read_exact(&mut ref_frame).is_err() {
            report.first_mismatch = Some(format!("ffmpeg has fewer frames than we decoded (we are at {})", report.frames));
            break;
        }
        let (ry, ruv) = ref_frame.split_at(y_len);
        let at = |plane: &[u8], i: usize| -> u16 {
            if ten { u16::from_le_bytes([plane[2 * i], plane[2 * i + 1]]) >> 6 } else { plane[i] as u16 }
        };
        let bad = planes.y.iter().enumerate().find(|(i, v)| **v != at(ry, *i)).map(|(i, v)| format!("Y x={} y={} gpu={} ffmpeg={}", i % w, i / w, v, at(ry, i)));
        let bad = bad.or_else(|| planes.uv.iter().enumerate().find(|(i, v)| **v != at(ruv, *i)).map(|(i, v)| format!("UV x={} y={} gpu={} ffmpeg={}", i % w, i / w, v, at(ruv, i))));
        if let Some(b) = bad {
            report.first_mismatch.get_or_insert(format!("frame {} (pts {}, {w}x{h}): {b}", report.frames, f.pts));
        }
        report.frames += 1;
    }
    report.fps = report.frames as f64 / t0.elapsed().as_secs_f64();
    if report.first_mismatch.is_none() {
        let mut extra = [0u8; 1];
        if out.read(&mut extra).unwrap_or(0) > 0 {
            report.first_mismatch = Some(format!("ffmpeg has more frames than the {} we decoded", report.frames));
        }
    }
    let _ = ff.kill();
    let _ = ff.wait();
    Some(report)
}

fn check_path(name: &str, clip: &Path, frames: usize) {
    let Some(r) = compare(clip) else { return };
    eprintln!("{name}: {} frames compared with ffmpeg ({:.0} fps incl. read-back), sizes {:?}: {:?}", r.frames, r.fps, r.sizes, r.first_mismatch);
    assert_eq!(r.frames, frames, "{name}: frame count");
    assert_eq!(r.first_mismatch, None, "{name}");
}

fn check(c: Clip, frames: usize) {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return;
    }
    let clip = make_clip(&c);
    check_path(c.name, &clip, frames);
}

#[test]
fn svt_default_mp4_8_bit() {
    check(Clip::svt("av1_svt.mp4", &["-b:v", "1M", "-g", "60"]), 90);
}

#[test]
fn svt_default_webm_8_bit() {
    check(Clip::svt("av1_svt.webm", &["-b:v", "1M", "-g", "60"]), 90);
}

#[test]
fn svt_10_bit_mp4() {
    let mut c = Clip::svt("av1_svt10.mp4", &["-b:v", "1M", "-g", "60"]);
    c.pix_fmt = "yuv420p10le";
    check(c, 90);
}

#[test]
fn svt_10_bit_webm() {
    let mut c = Clip::svt("av1_svt10.webm", &["-b:v", "1M", "-g", "60"]);
    c.pix_fmt = "yuv420p10le";
    check(c, 90);
}

#[test]
fn svt_tiles_720p() {
    let mut c = Clip::svt("av1_svt_tiles.mp4", &["-b:v", "2M", "-g", "60", "-svtav1-params", "tile-columns=2:tile-rows=1"]);
    c.size = "1280x720";
    check(c, 90);
}

#[test]
fn svt_film_grain() {
    check(Clip::svt("av1_svt_grain.mp4", &["-b:v", "1M", "-g", "60", "-svtav1-params", "film-grain=8"]), 90);
}

#[test]
fn libaom_defaults() {
    let mut c = Clip::svt("av1_aom.mp4", &["-b:v", "1M", "-cpu-used", "6", "-g", "60", "-lag-in-frames", "16", "-row-mt", "1"]);
    c.encoder = "libaom-av1";
    check(c, 90);
}

#[test]
fn libaom_tiles_and_odd_size() {
    let mut c = Clip::svt("av1_aom_odd.webm", &["-b:v", "800k", "-cpu-used", "6", "-g", "60", "-tiles", "2x2", "-row-mt", "1"]);
    c.encoder = "libaom-av1";
    c.size = "642x362";
    check(c, 90);
}

#[test]
fn decode_only_no_read_back() {
    let c = Clip::svt("av1_svt.mp4", &["-b:v", "1M", "-g", "60"]);
    let clip = make_clip(&c);
    let mut dec = VideoDecoder::open(&clip).unwrap();
    let mut n = 0;
    while dec.next_frame().unwrap().is_some() {
        n += 1;
    }
    dec.wait_idle().unwrap();
    eprintln!("decoded {n} frames");
}

#[test]
fn rav1e_defaults() {
    let mut c = Clip::svt("av1_rav1e.mp4", &["-b:v", "1M", "-speed", "8", "-g", "60"]);
    c.encoder = "librav1e";
    check(c, 90);
}
