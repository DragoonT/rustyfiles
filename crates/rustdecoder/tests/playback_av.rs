//! Video and audio together. The sync tests drive a [`Playback`] with a fake audio sink whose
//! clock is a fake time, so every step is exact: pictures come out in presentation order next
//! to the audio clock, the audio the feed thread queues is contiguous, and after a seek both
//! sides land at the target. One more test opens the real WASAPI output muted (nothing is
//! audible: session volume 0, session mute, silent buffers) and checks its clock.
//! Tests skip with a message when ffmpeg, a GPU decoder or an audio device is missing.

#![cfg(windows)]

mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::av::*;
use common::have_ffmpeg;
use rustdecoder::audio_out::{AudioOutput, AudioSink, OutputOptions};
use rustdecoder::player::{FakeTime, TimeSource};
use rustdecoder::playback::Playback;

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Flush,
    Push { pts: f64, frames: usize },
}

#[derive(Default)]
struct St {
    rate: u32,
    channels: usize,
    log: Vec<Ev>,
    /// Seconds of sound pushed since the last flush.
    pushed: f64,
    base: Option<f64>,
    running: bool,
    played: f64,
    since: f64,
    ended: bool,
}

/// A sink that plays nothing and counts time by a fake clock.
struct FakeSink {
    time: Arc<FakeTime>,
    st: Mutex<St>,
}

impl FakeSink {
    fn new(time: Arc<FakeTime>) -> Arc<Self> {
        Arc::new(FakeSink { time, st: Mutex::new(St::default()) })
    }
    fn played(&self, s: &St) -> f64 {
        if s.running { s.played + (self.time.now() - s.since) } else { s.played }
    }
    fn log(&self) -> Vec<Ev> {
        self.st.lock().unwrap().log.clone()
    }
}

impl AudioSink for FakeSink {
    fn set_input(&self, rate: u32, channels: usize) {
        let mut s = self.st.lock().unwrap();
        s.rate = rate;
        s.channels = channels;
    }
    fn push(&self, pts: f64, pcm: &[f32]) {
        let mut s = self.st.lock().unwrap();
        let frames = pcm.len() / s.channels;
        s.base.get_or_insert(pts);
        s.pushed += frames as f64 / s.rate as f64;
        s.log.push(Ev::Push { pts, frames });
    }
    fn end_of_data(&self) {
        self.st.lock().unwrap().ended = true;
    }
    fn buffered(&self) -> f64 {
        let s = self.st.lock().unwrap();
        (s.pushed - self.played(&s)).max(0.0)
    }
    fn flush(&self) {
        let mut s = self.st.lock().unwrap();
        s.log.push(Ev::Flush);
        s.pushed = 0.0;
        s.played = 0.0;
        s.running = false;
        s.base = None;
        s.ended = false;
    }
    fn play(&self) {
        let mut s = self.st.lock().unwrap();
        if !s.running {
            s.since = self.time.now();
            s.running = true;
        }
    }
    fn pause(&self) {
        let mut s = self.st.lock().unwrap();
        if s.running {
            s.played += self.time.now() - s.since;
            s.running = false;
        }
    }
    fn position(&self) -> Option<f64> {
        let s = self.st.lock().unwrap();
        s.base.map(|b| b + self.played(&s))
    }
    fn time_source(&self) -> Arc<dyn TimeSource> {
        self.time.clone()
    }
}

fn clip() -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return None;
    }
    Some(make_av_clip("av_aac_st48.mp4", 48000, true, AAC))
}

fn clip_h264() -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return None;
    }
    Some(make_av_clip_h264("av_h264_aac_st48.mp4", 48000, true, AAC))
}

fn open(p: &std::path::Path, sink: Arc<FakeSink>) -> Option<Playback> {
    match Playback::open_with_sink(p, 6, Some(sink)) {
        Ok(p) => Some(p),
        Err(rustdecoder::Error::Gpu(m)) => {
            eprintln!("skipped: {m}");
            None
        }
        Err(e) => panic!("open: {e}"),
    }
}

/// One step of a 60 Hz display: time moves, the decoder is given time to be ahead, the picture
/// for the clock is asked for.
fn step(pb: &mut Playback, time: &FakeTime) -> Option<rustdecoder::player::Shown> {
    // the audio feed is a thread too: with a fake time that races ahead of real time it has to
    // be given the time to queue what the clock is about to play
    for _ in 0..1000 {
        if pb.audio_ended() || pb.sink().is_none_or(|s| s.buffered() > 0.05) {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    time.advance(1.0 / 60.0);
    let now = pb.time();
    pb.player.wait_ahead(now, Duration::from_secs(5));
    pb.frame()
}

fn pictures_follow_the_audio_clock_in_order(clip: PathBuf) {
    let time = Arc::new(FakeTime::new());
    let sink = FakeSink::new(time.clone());
    let Some(mut pb) = open(&clip, sink.clone()) else { return };
    assert!(pb.has_audio(), "{}", pb.audio_note());
    pb.play();
    let mut last_serial = 0;
    let mut last_pts = -1.0;
    let mut shown = 0;
    let mut worst_lag = 0.0f64;
    for _ in 0..150 {
        let Some(f) = step(&mut pb, &time) else { continue };
        let now = pb.time();
        assert!(f.pts <= now + 1e-9, "a picture from the future: {} at {now}", f.pts);
        worst_lag = worst_lag.max(now - f.pts);
        if f.serial != last_serial {
            assert!(f.pts > last_pts, "pictures out of order: {} after {last_pts}", f.pts);
            last_pts = f.pts;
            last_serial = f.serial;
            shown += 1;
        }
        // the audio clock is the movie clock
        let a = pb.audio_position().unwrap();
        assert!((a - now).abs() < 1e-6, "audio {a} clock {now}");
    }
    let st = pb.stats();
    println!("2.5 s at 60 Hz: {shown} pictures, dropped {}, late {}, worst lag {:.1} ms", st.dropped, st.late, worst_lag * 1000.0);
    assert!(shown >= 70 && shown <= 76, "{shown} pictures in 2.5 s of 30 fps");
    assert_eq!(st.dropped, 0);
    // (Matroska times are whole milliseconds: a frame can sit up to 0.7 ms off its true time)
    assert!(worst_lag <= 1.0 / 30.0 + 0.002);
    // the sound queued is one unbroken run from time 0
    let log = sink.log();
    let mut next = 0.0;
    let mut total = 0usize;
    for e in &log {
        match e {
            Ev::Push { pts, frames } => {
                assert!((pts - next).abs() < 1e-6, "audio gap: block at {pts}, expected {next}");
                next = pts + *frames as f64 / 48000.0;
                total += frames;
            }
            Ev::Flush => panic!("flush without a seek"),
        }
    }
    println!("audio queued: {total} frames ({next:.3} s) in {} blocks", log.len());
    assert!(next >= 2.5, "audio ahead of the clock");
}

fn seeking_lands_both_sides_on_the_target(clip: PathBuf) {
    let time = Arc::new(FakeTime::new());
    let sink = FakeSink::new(time.clone());
    let Some(mut pb) = open(&clip, sink.clone()) else { return };
    pb.play();
    for _ in 0..20 {
        step(&mut pb, &time);
    }
    for (n, target) in [1.5f64, 0.2, 2.9, 0.0].into_iter().enumerate() {
        pb.seek(target);
        assert!(pb.is_playing());
        assert!((pb.time() - target).abs() < 1e-9, "clock at {} after seek to {target}", pb.time());
        // audio: flushed, then the first block queued starts exactly at the target
        let mut first_block = None;
        for _ in 0..200 {
            let log = sink.log();
            // seek() returns after the feed thread flushed: the first block after the last
            // flush is where the audio restarted
            let flush = log.iter().rposition(|e| *e == Ev::Flush).expect("no flush");
            if let Some(e) = log[flush..].iter().find(|e| matches!(e, Ev::Push { .. })) {
                first_block = Some(e.clone());
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let Some(Ev::Push { pts, .. }) = first_block else { panic!("no audio after the seek to {target}") };
        assert!((pts - target).abs() < 1.0 / 48000.0, "audio restarts at {pts}, target {target}");
        // video: the first picture is the one at or just before the target
        let mut first = None;
        for _ in 0..30 {
            if let Some(f) = step(&mut pb, &time) {
                first = Some(f);
                break;
            }
        }
        let f = first.expect("no picture after the seek");
        let after = pb.time();
        println!("seek {n} to {target}: audio restarts at {pts:.6}, first picture {:.4} (clock {after:.4})", f.pts);
        assert!(f.pts <= after + 1e-9 && after - f.pts <= 1.0 / 30.0 + 1e-6, "picture {} for clock {after}", f.pts);
        // and they keep going together
        let mut last = f.pts;
        for _ in 0..30 {
            if let Some(g) = step(&mut pb, &time)
                && g.pts != last
            {
                assert!(g.pts > last, "out of order after a seek");
                last = g.pts;
            }
        }
        let a = pb.audio_position().unwrap();
        assert!((a - pb.time()).abs() < 1e-6, "audio {a} clock {}", pb.time());
    }
}

#[test]
fn paused_playback_stands_still() {
    let Some(clip) = clip() else { return };
    let time = Arc::new(FakeTime::new());
    let sink = FakeSink::new(time.clone());
    let Some(mut pb) = open(&clip, sink) else { return };
    pb.play();
    for _ in 0..30 {
        step(&mut pb, &time);
    }
    pb.pause();
    let t = pb.time();
    let a = pb.audio_position().unwrap();
    time.advance(5.0);
    assert_eq!(pb.time(), t);
    assert_eq!(pb.audio_position().unwrap(), a);
    pb.play();
    time.advance(0.5);
    assert!((pb.time() - t - 0.5).abs() < 1e-6);
}

#[test]
fn no_audio_track_keeps_the_system_clock() {
    if !have_ffmpeg() {
        return;
    }
    let clip = common::make_clip("m10_plain.mp4", "yuv420p10le", "bframes=2", false);
    let time = Arc::new(FakeTime::new());
    let sink = FakeSink::new(time);
    let Some(pb) = open(&clip, sink) else { return };
    assert!(!pb.has_audio());
    assert_eq!(pb.audio_note(), "no audio track");
}

#[test]
fn muted_device_clock_runs_in_real_time() {
    let out = match AudioOutput::open(OutputOptions { muted: true, volume: 1.0 }) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let (rate, ch) = out.device_format();
    println!("device: {} Hz {} ch ({})", rate, ch, out.device_name());
    let src_rate = 44100u32; // different from the device's on most machines: the resampler runs
    out.set_input(src_rate, 2);
    let tone: Vec<f32> = (0..src_rate as usize * 2).flat_map(|i| {
        let v = (i as f64 * 2.0 * std::f64::consts::PI * 440.0 / src_rate as f64).sin() as f32 * 0.5;
        [v, v]
    }).collect();
    out.push(10.0, &tone);
    assert!((out.buffered() - 2.0).abs() < 0.05, "buffered {}", out.buffered());
    let clock = out.time_source();
    let c0 = clock.now();
    out.play();
    let t0 = std::time::Instant::now();
    let mut last = c0;
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(25));
        let c = clock.now();
        assert!(c >= last, "clock went back: {c} after {last}");
        last = c;
    }
    let wall = t0.elapsed().as_secs_f64();
    let adv = clock.now() - c0;
    let pos = out.position().unwrap();
    println!("muted device: clock advanced {adv:.4} s in {wall:.4} s wall, position {pos:.4}, {} device periods, {} under-runs", out.device_periods(), out.underruns());
    assert!((adv - wall).abs() < 0.06, "clock {adv} wall {wall}");
    assert!((pos - 10.0 - adv).abs() < 0.03);
    assert!(out.device_periods() > 5);
    // pause: the clock stops
    out.pause();
    let p = clock.now();
    std::thread::sleep(Duration::from_millis(200));
    assert!((clock.now() - p).abs() < 1e-6);
    // flush and a new position
    out.flush();
    assert!(out.position().is_none());
    out.push(20.0, &tone[..44100]);
    out.play();
    std::thread::sleep(Duration::from_millis(300));
    let pos = out.position().unwrap();
    println!("after flush: position {pos:.4} (started at 20.0)");
    assert!(pos > 20.2 && pos < 20.45, "{pos}");
    // the queue ran out (0.5 s pushed); the stalled clock does not run on
    out.pause();
}

#[test]
fn muted_device_under_run_stops_the_clock() {
    let out = match AudioOutput::open(OutputOptions::default()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("skipped: {e}");
            return;
        }
    };
    let (rate, ch) = out.device_format();
    out.set_input(rate, ch);
    // 0.3 s of sound, then nothing: the device plays silence, the clock stands at 0.3 s
    out.push(0.0, &vec![0.0f32; (rate as usize * 3 / 10) * ch]);
    let clock = out.time_source();
    let c0 = clock.now();
    out.play();
    std::thread::sleep(Duration::from_millis(900));
    let adv = clock.now() - c0;
    println!("0.3 s queued, 0.9 s wall: clock advanced {adv:.3} s, under-runs {}", out.underruns());
    assert!(adv > 0.25 && adv < 0.36, "{adv}");
    assert!(out.underruns() >= 1);
    // end of data: silence is time again
    out.end_of_data();
    let c1 = clock.now();
    std::thread::sleep(Duration::from_millis(600));
    assert!(clock.now() - c1 > 0.3, "{}", clock.now() - c1);
    out.pause();
}

/// The sync tests on HEVC and on H.264.
macro_rules! both {
    ($($name:ident),*) => {
        mod hevc {
            use super::*;
            $(#[test] fn $name() { if let Some(c) = clip() { super::$name(c) } })*
        }
        mod h264 {
            use super::*;
            $(#[test] fn $name() { if let Some(c) = clip_h264() { super::$name(c) } })*
        }
    };
}

both!(pictures_follow_the_audio_clock_in_order, seeking_lands_both_sides_on_the_target);

// ------------------------------------------------------------------ Matroska / WebM

const MKV_H264: &[&str] = &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast", "-x264-params", "log-level=error:keyint=15:min-keyint=15:bframes=3:scenecut=0"];
const MKV_HEVC: &[&str] = &["-c:v", "libx265", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-x265-params", "log-level=error:keyint=15:min-keyint=15"];
const MKV_VP9: &[&str] = &["-c:v", "libvpx-vp9", "-pix_fmt", "yuv420p", "-deadline", "good", "-cpu-used", "5", "-g", "15", "-b:v", "1M"];

fn mkv_clip(name: &str, video: &[&str], audio: &[&str]) -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return None;
    }
    Some(common::make_mkv(name, video, audio, &[]))
}

macro_rules! containers {
    ($($m:ident: $name:expr, $video:expr, $audio:expr;)*) => {
        $(mod $m {
            use super::*;
            fn clip() -> Option<PathBuf> { mkv_clip($name, $video, $audio) }
            #[test] fn pictures_follow_the_audio_clock_in_order() { if let Some(c) = clip() { super::pictures_follow_the_audio_clock_in_order(c) } }
            #[test] fn seeking_lands_both_sides_on_the_target() { if let Some(c) = clip() { super::seeking_lands_both_sides_on_the_target(c) } }
        })*
    };
}

containers! {
    mkv_h264_aac: "pb_h264_aac.mkv", MKV_H264, &["-c:a", "aac", "-b:a", "96k"];
    mkv_hevc_pcm: "pb_hevc_pcm.mkv", MKV_HEVC, &["-c:a", "pcm_s16le"];
    mkv_vp9_pcm: "pb_vp9_pcm.mkv", MKV_VP9, &["-c:a", "pcm_s16le"];
    mkv_vp9_aac: "pb_vp9_aac.mkv", MKV_VP9, &["-c:a", "aac", "-b:a", "96k"];
}

/// Opus and Vorbis are not decoded yet: the video plays on the system clock and the note says why.
#[test]
fn webm_with_opus_plays_without_sound_and_says_so() {
    for (name, audio) in [("pb_vp9_opus.webm", &["-c:a", "libopus", "-b:a", "64k"]), ("pb_vp9_vorbis.webm", &["-c:a", "libvorbis", "-b:a", "64k"])] {
        let Some(clip) = mkv_clip(name, MKV_VP9, audio) else { return };
        let time = Arc::new(FakeTime::new());
        let sink = FakeSink::new(time);
        let Some(mut pb) = open(&clip, sink) else { return };
        assert!(!pb.has_audio());
        println!("{name}: audio note: {}", pb.audio_note());
        assert!(pb.audio_note().contains("not playable"), "{}", pb.audio_note());
        pb.play();
        let t0 = std::time::Instant::now();
        let mut shown = 0;
        let mut last = -1.0;
        while t0.elapsed() < Duration::from_millis(1200) {
            pb.player.wait_ahead(pb.time(), Duration::from_millis(50));
            if let Some(f) = pb.frame()
                && f.pts != last
            {
                last = f.pts;
                shown += 1;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
        println!("{name}: {shown} pictures in 1.2 s of wall time, clock at {:.3}", pb.time());
        assert!(shown >= 25, "{shown}");
        assert!(pb.time() > 1.0 && pb.time() < 1.5);
    }
}

// ------------------------------------------------------------------ ProRes in a MOV with PCM

/// A 3 s MOV: ProRes 422 HQ (decoded on the CPU) and 16-bit PCM, 48 kHz stereo, 30 fps.
fn clip_prores() -> Option<PathBuf> {
    if !have_ffmpeg() {
        eprintln!("skipped: ffmpeg not found");
        return None;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = common::temp_dir().join("av_prores_pcm_st48.mov");
    if out.exists() {
        return Some(out);
    }
    let tmp = out.with_file_name("part-av_prores_pcm_st48.mov");
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30", "-f", "lavfi", "-i", "sine=f=440:r=48000:d=3", "-f", "lavfi", "-i", "sine=f=660:r=48000:d=3"])
        .args(["-filter_complex", "[1:a][2:a]amerge=inputs=2[a]", "-map", "0:v", "-map", "[a]", "-t", "3"])
        .args(["-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le", "-c:a", "pcm_s16le", "-f", "mov"])
        .arg(&tmp)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg could not make the ProRes clip");
    std::fs::rename(&tmp, &out).unwrap();
    Some(out)
}

mod prores {
    use super::*;
    #[test]
    fn pictures_follow_the_audio_clock_in_order() {
        if let Some(c) = clip_prores() {
            super::pictures_follow_the_audio_clock_in_order(c)
        }
    }
    #[test]
    fn seeking_lands_both_sides_on_the_target() {
        if let Some(c) = clip_prores() {
            super::seeking_lands_both_sides_on_the_target(c)
        }
    }
}
