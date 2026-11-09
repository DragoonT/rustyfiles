//! DNxHD / DNxHR on the CPU: every frame of clips made by ffmpeg's `dnxhd` encoder is compared
//! with what ffmpeg's own decoder gives (`-f rawvideo`), plane by plane, in the pixel format
//! the clip was made from.
mod common;

use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use common::{have_ffmpeg, temp_dir};
use rustdecoder::dnx::{self, DnxDecoder};
use rustdecoder::prores::Picture;

static LOCK: Mutex<()> = Mutex::new(());

struct Spec {
    name: &'static str,
    size: (u32, u32),
    /// ffmpeg encoder arguments.
    enc: &'static [&'static str],
    /// Pixel format of the encoder input; also what ffmpeg's decoder is asked for.
    pix_fmt: &'static str,
    frames: u32,
    /// `-r` of the test source.
    rate: u32,
}

fn make(s: &Spec) -> PathBuf {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = temp_dir().join(format!("dnx_{}_{}x{}.mov", s.name, s.size.0, s.size.1));
    if out.exists() {
        return out;
    }
    let tmp = out.with_extension("part.mov");
    let status = Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i"])
        .arg(format!("testsrc2=size={}x{}:rate={}", s.size.0, s.size.1, s.rate))
        .args(["-frames:v", &s.frames.to_string(), "-pix_fmt", s.pix_fmt])
        .args(s.enc)
        .args(["-f", "mov"])
        .arg(&tmp)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg could not make {}", s.name);
    std::fs::rename(&tmp, &out).unwrap();
    out
}

fn reference(path: &Path, fmt: &str) -> Vec<u8> {
    let o = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(path).args(["-f", "rawvideo", "-pix_fmt", fmt, "-"]).stderr(Stdio::inherit()).output().expect("ffmpeg");
    assert!(o.status.success());
    o.stdout
}

fn reference_raw(path: &Path, fmt: &str) -> Vec<u8> {
    let o = Command::new("ffmpeg").args(["-v", "error", "-f", "dnxhd", "-i"]).arg(path).args(["-f", "rawvideo", "-pix_fmt", fmt, "-"]).stderr(Stdio::inherit()).output().expect("ffmpeg");
    assert!(o.status.success());
    o.stdout
}

fn decode_all(path: &Path, threads: usize) -> Vec<Picture> {
    let mut f = File::open(path).unwrap();
    let movie = rustdecoder::container::read(&mut f).unwrap();
    let track = movie.video().unwrap();
    assert!(dnx::is_dnx_fourcc(&track.fourcc), "tag {:?}", String::from_utf8_lossy(&track.fourcc));
    let dec = DnxDecoder::new(threads);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    for i in 0..track.samples.len() {
        track.read_sample(&mut f, i, &mut buf).unwrap();
        let mut p = Picture::default();
        dec.decode(&buf, &mut p).unwrap();
        out.push(p);
    }
    out
}

/// (max error, mean error, count of different samples) of one plane; `wide` samples are 16 bit.
fn plane_err(ours: &[u16], stride: usize, w: usize, h: usize, theirs: &[u8], wide: bool) -> (i32, f64, usize) {
    let mut max = 0;
    let mut sum = 0u64;
    let mut n = 0;
    for y in 0..h {
        for x in 0..w {
            let a = ours[y * stride + x] as i32;
            let b = if wide {
                let i = (y * w + x) * 2;
                u16::from_le_bytes([theirs[i], theirs[i + 1]]) as i32
            } else {
                theirs[y * w + x] as i32
            };
            let d = (a - b).abs();
            max = max.max(d);
            sum += d as u64;
            if d != 0 {
                n += 1;
            }
        }
    }
    (max, sum as f64 / (w * h) as f64, n)
}

/// Compares every frame and returns the largest error found; prints a table row.
fn compare(s: &Spec) -> i32 {
    let path = make(s);
    let pics = decode_all(&path, 1);
    let par = decode_all(&path, 0);
    for (a, b) in pics.iter().zip(&par) {
        assert!(a.y == b.y && a.u == b.u && a.v == b.v, "{}: parallel decoding differs from single threaded", s.name);
    }
    let theirs = reference(&path, s.pix_fmt);
    let wide = s.pix_fmt.contains("p10") || s.pix_fmt.contains("p12");
    let bps = if wide { 2 } else { 1 };
    let (w, h) = (pics[0].width as usize, pics[0].height as usize);
    let (cw, ch) = {
        let (a, b) = pics[0].chroma_size();
        (a as usize, b as usize)
    };
    let frame_bytes = (w * h + 2 * cw * ch) * bps;
    assert_eq!(theirs.len(), frame_bytes * pics.len(), "{}: ffmpeg frame count", s.name);
    let mut worst = [(0, 0.0, 0usize); 3];
    for (i, p) in pics.iter().enumerate() {
        let f = &theirs[i * frame_bytes..][..frame_bytes];
        let e = [
            plane_err(&p.y, p.stride_y, w, h, &f[..w * h * bps], wide),
            plane_err(&p.u, p.stride_c, cw, ch, &f[w * h * bps..][..cw * ch * bps], wide),
            plane_err(&p.v, p.stride_c, cw, ch, &f[(w * h + cw * ch) * bps..][..cw * ch * bps], wide),
        ];
        for k in 0..3 {
            worst[k].0 = worst[k].0.max(e[k].0);
            worst[k].1 += e[k].1 / pics.len() as f64;
            worst[k].2 += e[k].2;
        }
    }
    let max = worst.iter().map(|e| e.0).max().unwrap();
    let total = (w * h + 2 * cw * ch) * pics.len();
    let fmt = |e: &(i32, f64, usize)| if e.2 == 0 { "exact".to_string() } else { format!("max {} mean {:.5}", e.0, e.1) };
    let diff: usize = worst.iter().map(|e| e.2).sum();
    // the header of the first frame, for the table
    let mut f = File::open(&path).unwrap();
    let movie = rustdecoder::container::read(&mut f).unwrap();
    let mut buf = Vec::new();
    movie.video().unwrap().read_sample(&mut f, 0, &mut buf).unwrap();
    let hdr = dnx::parse_header(&buf).unwrap();
    println!(
        "| {:<14} | {:>4}x{:<4} | CID {} | {:>2} bit {} | {:<11} | {} | {} | {} | {:.3}% |",
        s.name,
        w,
        h,
        hdr.cid,
        hdr.bits,
        if hdr.interlaced { "interlaced" } else { "progressive" },
        s.pix_fmt,
        fmt(&worst[0]),
        fmt(&worst[1]),
        fmt(&worst[2]),
        100.0 * diff as f64 / total as f64
    );
    max
}

fn check(specs: Vec<Spec>) {
    if !have_ffmpeg() {
        return;
    }
    println!("| clip | size | CID | format | compared as | Y | U | V | samples off by 1 |");
    let mut bad = Vec::new();
    for s in &specs {
        let max = compare(s);
        // 4:4:4 at 10 bit shows IDCT rounding differences of 2
        if max > if s.name.starts_with("444") { 2 } else { 1 } {
            bad.push(format!("{}: error {max}", s.name));
        }
    }
    assert!(bad.is_empty(), "{bad:?}");
}

fn hr(name: &'static str, profile: &'static [&'static str], pix_fmt: &'static str, size: (u32, u32), frames: u32) -> Spec {
    Spec { name, size, enc: profile, pix_fmt, frames, rate: 25 }
}

const LB: &[&str] = &["-c:v", "dnxhd", "-profile:v", "dnxhr_lb"];
const SQ: &[&str] = &["-c:v", "dnxhd", "-profile:v", "dnxhr_sq"];
const HQ: &[&str] = &["-c:v", "dnxhd", "-profile:v", "dnxhr_hq"];
const HQX: &[&str] = &["-c:v", "dnxhd", "-profile:v", "dnxhr_hqx"];
const H444: &[&str] = &["-c:v", "dnxhd", "-profile:v", "dnxhr_444"];

#[test]
fn dnxhr_8_bit_640() {
    check(vec![hr("lb", LB, "yuv422p", (640, 360), 3), hr("sq", SQ, "yuv422p", (640, 360), 3), hr("hq", HQ, "yuv422p", (640, 360), 3)]);
}

#[test]
fn dnxhr_8_bit_1080() {
    check(vec![hr("lb", LB, "yuv422p", (1920, 1080), 3), hr("sq", SQ, "yuv422p", (1920, 1080), 3), hr("hq", HQ, "yuv422p", (1920, 1080), 3)]);
}

#[test]
fn dnxhr_hqx_10_bit() {
    check(vec![hr("hqx", HQX, "yuv422p10le", (640, 360), 3), hr("hqx", HQX, "yuv422p10le", (1920, 1080), 3)]);
}

/// ffmpeg's encoder only writes 10 bit for these CIDs, so the 12 bit path is checked on a 10 bit
/// frame whose header says 12 bit: the same coefficients, read by ffmpeg's decoder as 12 bit.
#[test]
fn dnxhr_12_bit_by_header_patch() {
    if !have_ffmpeg() {
        return;
    }
    println!("| clip | size | CID | format | compared as | Y | U | V | samples off by 1 |");
    for (name, enc, pix_fmt, ref_fmt) in [("hqx", HQX, "yuv422p10le", "yuv422p12le"), ("444_10", H444, "yuv444p10le", "yuv444p12le")] {
        let path = make(&hr(name, enc, pix_fmt, (640, 360), 2));
        let mut f = File::open(&path).unwrap();
        let movie = rustdecoder::container::read(&mut f).unwrap();
        let mut frame = Vec::new();
        movie.video().unwrap().read_sample(&mut f, 0, &mut frame).unwrap();
        assert_eq!(frame[0x21] >> 5, 2, "the clip is 10 bit");
        frame[0x21] = (frame[0x21] & 0x1F) | (3 << 5);
        let raw = temp_dir().join(format!("dnx_{name}_as12.dnxhd"));
        std::fs::write(&raw, &frame).unwrap();
        let theirs = reference_raw(&raw, ref_fmt);
        let mut pic = Picture::default();
        let hdr = DnxDecoder::new(1).decode(&frame, &mut pic).unwrap();
        assert_eq!(hdr.bits, 12);
        let (w, h) = (pic.width as usize, pic.height as usize);
        let (cw, ch) = {
            let (a, b) = pic.chroma_size();
            (a as usize, b as usize)
        };
        assert_eq!(theirs.len(), (w * h + 2 * cw * ch) * 2, "ffmpeg frame size");
        let e = [
            plane_err(&pic.y, pic.stride_y, w, h, &theirs[..w * h * 2], true),
            plane_err(&pic.u, pic.stride_c, cw, ch, &theirs[w * h * 2..][..cw * ch * 2], true),
            plane_err(&pic.v, pic.stride_c, cw, ch, &theirs[(w * h + cw * ch) * 2..][..cw * ch * 2], true),
        ];
        let fmt = |e: &(i32, f64, usize)| if e.2 == 0 { "exact".to_string() } else { format!("max {} mean {:.5}", e.0, e.1) };
        let diff: usize = e.iter().map(|x| x.2).sum();
        println!("| {:<14} | {:>4}x{:<4} | CID {} | 12 bit (header patched) | {:<11} | {} | {} | {} | {:.3}% |", name, w, h, hdr.cid, ref_fmt, fmt(&e[0]), fmt(&e[1]), fmt(&e[2]), 100.0 * diff as f64 / (w * h + 2 * cw * ch) as f64);
        let max = e.iter().map(|x| x.0).max().unwrap();
        assert!(max <= 12, "{name}: error {max}"); // a 10 bit step is 4 here; ffmpeg's integer IDCT is off by up to 1 or 2 of those
    }
}

#[test]
fn dnxhr_444() {
    check(vec![hr("444_10", H444, "yuv444p10le", (640, 360), 3), hr("444_10", H444, "yuv444p10le", (1920, 1080), 2)]);
}

#[test]
fn dnxhr_odd_sizes() {
    check(vec![
        hr("hq", HQ, "yuv422p", (650, 366), 2),
        hr("lb", LB, "yuv422p", (322, 178), 2),
        hr("hqx", HQX, "yuv422p10le", (1001, 563), 2),
        hr("444_10", H444, "yuv444p10le", (333, 211), 2),
    ]);
}

#[test]
fn dnxhd_fixed_bitrates() {
    check(vec![
        Spec { name: "hd_8_120m", size: (1920, 1080), enc: &["-c:v", "dnxhd", "-b:v", "120M"], pix_fmt: "yuv422p", frames: 3, rate: 25 },
        Spec { name: "hd_8_36m", size: (1920, 1080), enc: &["-c:v", "dnxhd", "-b:v", "36M"], pix_fmt: "yuv422p", frames: 3, rate: 25 },
        Spec { name: "hd_10_185m", size: (1920, 1080), enc: &["-c:v", "dnxhd", "-b:v", "185M"], pix_fmt: "yuv422p10le", frames: 3, rate: 25 },
        Spec { name: "hd_720_8", size: (1280, 720), enc: &["-c:v", "dnxhd", "-b:v", "90M"], pix_fmt: "yuv422p", frames: 3, rate: 25 },
        Spec { name: "hd_720_10", size: (1280, 720), enc: &["-c:v", "dnxhd", "-b:v", "180M"], pix_fmt: "yuv422p10le", frames: 3, rate: 50 },
    ]);
}

#[test]
fn dnxhd_interlaced() {
    check(vec![
        Spec { name: "hd_8_i", size: (1920, 1080), enc: &["-c:v", "dnxhd", "-b:v", "120M", "-flags", "+ildct+ilme", "-top", "1"], pix_fmt: "yuv422p", frames: 3, rate: 25 },
        Spec { name: "hd_10_i", size: (1920, 1080), enc: &["-c:v", "dnxhd", "-b:v", "185M", "-flags", "+ildct+ilme", "-top", "1"], pix_fmt: "yuv422p10le", frames: 3, rate: 25 },
    ]);
}
