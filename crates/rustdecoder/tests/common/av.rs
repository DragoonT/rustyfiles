//! Clips with sound for the audio tests, made with ffmpeg.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

use super::temp_dir;

/// Makes (once) a 3 s clip: 640x360 test pattern at 30 fps (x265, a sync sample every 15
/// frames) with a sine tone track: 440 Hz, and 660 Hz on the right channel when `stereo`.
/// `acodec` are ffmpeg's audio encoder arguments; `name`'s extension picks the container.
pub fn make_av_clip(name: &str, rate: u32, stereo: bool, acodec: &[&str]) -> PathBuf {
    let out = temp_dir().join(name);
    if out.exists() {
        return out;
    }
    let tmp = out.with_file_name(format!("part-{name}"));
    let mut c = Command::new("ffmpeg");
    c.args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=640x360:rate=30"]);
    c.args(["-f", "lavfi", "-i"]).arg(format!("sine=f=440:r={rate}:d=3"));
    if stereo {
        c.args(["-f", "lavfi", "-i"]).arg(format!("sine=f=660:r={rate}:d=3"));
        c.args(["-filter_complex", "[1:a][2:a]amerge=inputs=2[a]", "-map", "0:v", "-map", "[a]"]);
    }
    let status = c
        .args(["-t", "3", "-c:v", "libx265", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-x265-params", "log-level=error:keyint=15:min-keyint=15"])
        .args(acodec)
        .arg(&tmp)
        .status()
        .expect("ffmpeg");
    assert!(status.success(), "ffmpeg could not make {name}");
    std::fs::rename(&tmp, &out).unwrap();
    out
}

/// The first audio stream of `clip` decoded by ffmpeg to interleaved `f32` at its own rate.
pub fn ffmpeg_audio(clip: &Path) -> Vec<f32> {
    let o = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(clip).args(["-map", "0:a:0", "-f", "f32le", "-"]).output().expect("ffmpeg");
    assert!(o.status.success(), "ffmpeg could not decode the audio");
    o.stdout.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

/// `start_time` in seconds of the first audio stream, per ffprobe.
pub fn ffprobe_audio_start(clip: &Path) -> f64 {
    let o = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "a:0", "-show_entries", "stream=start_time", "-of", "csv=p=0"])
        .arg(clip)
        .output()
        .expect("ffprobe");
    String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0.0)
}

pub const AAC: &[&str] = &["-c:a", "aac", "-b:a", "128k"];
pub const PCM16: &[&str] = &["-c:a", "pcm_s16le"];
pub const PCM24: &[&str] = &["-c:a", "pcm_s24le"];
