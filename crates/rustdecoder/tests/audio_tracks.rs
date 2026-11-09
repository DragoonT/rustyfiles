//! Several audio tracks: listing with language / name / default, the default choice, and mono PCM
//! tracks of one recording (camera MXF, MOV) played as one multichannel stream that equals the
//! tracks interleaved bit for bit, also after a seek. Clips are made with ffmpeg; tests skip
//! when it is missing.

#![cfg(windows)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{have_ffmpeg, temp_dir};
use rustdecoder::audio::AudioDecoder;
use rustdecoder::container;
use rustdecoder::mp4::Movie;

static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A 2 s clip (x264 video at 25 fps) with one audio stream per entry of `freqs` (a sine of that
/// frequency each, 48 kHz mono), encoded with `acodec`; `extra` are more output arguments
/// (metadata, dispositions) and the container follows the extension of `name`.
pub fn make(name: &str, freqs: &[u32], acodec: &[&str], extra: &[&str]) -> PathBuf {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let out = temp_dir().join(name);
    if out.exists() {
        return out;
    }
    let tmp = out.with_file_name(format!("part-{name}"));
    let mut c = Command::new("ffmpeg");
    c.args(["-y", "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25"]);
    for f in freqs {
        c.args(["-f", "lavfi", "-i"]).arg(format!("sine=f={f}:r=48000:d=2"));
    }
    c.args(["-map", "0:v"]);
    for i in 0..freqs.len() {
        c.args(["-map", &format!("{}:a", i + 1)]);
    }
    c.args(["-t", "2", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "ultrafast", "-g", "12"]);
    c.args(acodec).args(extra);
    if name.ends_with(".mxf") {
        c.args(["-f", "mxf"]);
    }
    let st = c.arg(&tmp).status().expect("ffmpeg");
    assert!(st.success(), "ffmpeg could not make {name}");
    std::fs::rename(&tmp, &out).unwrap();
    out
}

const PCM24: &[&str] = &["-c:a", "pcm_s24le"];

/// Language clip: two tracks, Turkish first, English marked default.
pub fn two_languages(ext: &str, acodec: &[&str]) -> PathBuf {
    make(
        &format!("tracks_two_lang.{ext}"),
        &[440, 880],
        acodec,
        &[
            "-metadata:s:a:0", "language=tur", "-metadata:s:a:1", "language=eng", "-metadata:s:a:0", "title=Turkce", "-metadata:s:a:1", "title=English",
            "-metadata:s:a:0", "handler_name=Turkce", "-metadata:s:a:1", "handler_name=English", "-disposition:a:1", "default",
        ],
    )
}

fn movie(p: &Path) -> Movie {
    container::read(&mut std::fs::File::open(p).unwrap()).unwrap()
}

/// Track `k` of `clip` decoded by ffmpeg to f32 mono.
fn ffmpeg_track(clip: &Path, k: usize) -> Vec<f32> {
    let o = Command::new("ffmpeg").args(["-v", "error", "-i"]).arg(clip).args(["-map", &format!("0:a:{k}"), "-f", "f32le", "-"]).output().expect("ffmpeg");
    assert!(o.status.success());
    o.stdout.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

fn lang_clip_checks(clip: &Path, names: bool) {
    let m = movie(clip);
    let list = m.audio_tracks();
    println!("{}: {list:#?}", clip.display());
    assert_eq!(list.len(), 2, "two separate tracks (different languages, not a group)");
    assert_eq!(list[0].language, "tur");
    assert_eq!(list[1].language, "eng");
    assert!(list.iter().all(|a| a.channels == 1 && a.sample_rate == 48000));
    if names {
        assert_eq!(list[0].name, "Turkce");
        assert_eq!(list[1].name, "English");
    }
    assert!(!list[0].default, "the first is not marked");
    assert!(list[1].default, "the second is marked");
    assert_eq!(Movie::default_audio_track(&list), Some(1));
    // the default decoder plays the English track (880 Hz)
    let mut d = AudioDecoder::open(clip).unwrap().unwrap();
    let (_, pcm) = d.decode_all().unwrap();
    assert!((zero_cross_freq(&pcm, 1, 48000) - 880.0).abs() < 5.0);
    // and the first one, when asked for, 440 Hz
    let mut d = AudioDecoder::open_choice(clip, &m, &list[0]).unwrap();
    let (_, pcm) = d.decode_all().unwrap();
    assert!((zero_cross_freq(&pcm, 1, 48000) - 440.0).abs() < 5.0);
}

/// Frequency of the first channel from rising zero crossings.
pub fn zero_cross_freq(x: &[f32], ch: usize, rate: u32) -> f64 {
    let mono: Vec<f32> = x.chunks_exact(ch).map(|f| f[0]).collect();
    let (a, b) = (mono.len() / 10, mono.len() * 9 / 10);
    let (mut first, mut last, mut n) = (None, 0.0, 0);
    for i in a..b - 1 {
        if mono[i] < 0.0 && mono[i + 1] >= 0.0 {
            let t = i as f64 + (-mono[i] as f64) / (mono[i + 1] - mono[i]) as f64;
            first.get_or_insert(t);
            last = t;
            n += 1;
        }
    }
    (n - 1) as f64 * rate as f64 / (last - first.unwrap())
}

#[test]
fn mkv_two_languages() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    lang_clip_checks(&two_languages("mkv", &["-c:a", "pcm_s16le"]), true);
}

#[test]
fn mp4_two_languages() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    let clip = two_languages("mp4", &["-c:a", "aac", "-b:a", "96k"]);
    let m = movie(&clip);
    let list = m.audio_tracks();
    println!("{list:#?}");
    assert_eq!(list.len(), 2);
    assert_eq!((list[0].language.as_str(), list[1].language.as_str()), ("tur", "eng"));
    assert_eq!((list[0].name.as_str(), list[1].name.as_str()), ("Turkce", "English"));
    assert!(list.iter().all(|a| a.codec == "AAC"));
    assert_eq!(Movie::default_audio_track(&list), Some(1), "{:?}", list.iter().map(|a| a.default).collect::<Vec<_>>());
}

#[test]
fn mov_pcm_two_languages_not_grouped() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    let clip = two_languages("mov", &["-c:a", "pcm_s16le"]);
    let list = movie(&clip).audio_tracks();
    assert_eq!(list.len(), 2, "mono PCM tracks of different languages stay apart");
}

/// Mono tracks as one stream: the decoded group is the tracks interleaved, bit for bit.
fn group_checks(clip: &Path, n: usize) {
    let m = movie(clip);
    let list = m.audio_tracks();
    println!("{}: {list:#?}", clip.display());
    assert_eq!(list.len(), 1, "one group");
    let g = &list[0];
    assert_eq!(g.channels as usize, n);
    assert_eq!(g.members.len(), n);
    assert_eq!(g.codec, "PCM");
    assert_eq!(Movie::default_audio_track(&list), Some(0));
    let mut d = AudioDecoder::open(clip).unwrap().unwrap();
    assert!(d.is_group());
    assert_eq!(d.info().channels as usize, n);
    let (first, got) = d.decode_all().unwrap();
    assert_eq!(first, 0.0);
    let tracks: Vec<Vec<f32>> = (0..n).map(|k| ffmpeg_track(clip, k)).collect();
    let frames = tracks.iter().map(Vec::len).min().unwrap();
    assert!(frames > 90_000, "{frames} frames");
    assert_eq!(got.len() / n, frames, "same length as the tracks");
    for f in 0..frames {
        for (k, t) in tracks.iter().enumerate() {
            assert_eq!(got[f * n + k].to_bits(), t[f].to_bits(), "frame {f} channel {k}");
        }
    }
    // each channel carries its own tone (440 + 110 k Hz)
    for k in 0..n {
        let mono: Vec<f32> = got.chunks_exact(n).map(|f| f[k]).collect();
        let hz = zero_cross_freq(&mono, 1, 48000);
        assert!((hz - (440.0 + 110.0 * k as f64)).abs() < 5.0, "channel {k}: {hz} Hz");
    }
    // a seek keeps every channel aligned: the samples from t on are the tail of the whole
    for t in [0.5f64, 1.234567, 0.0] {
        d.seek(t);
        let (at, part) = d.decode_all().unwrap();
        let skip = (t * 48000.0).round() as usize;
        assert!((at - skip as f64 / 48000.0).abs() < 1e-9, "restart at {at}, wanted {t}");
        assert_eq!(part.len(), (frames - skip) * n, "length after seeking to {t}");
        assert_eq!(&part[..], &got[skip * n..frames * n], "content after seeking to {t}");
    }
}

#[test]
fn mov_four_mono_tracks_are_one_group() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    group_checks(&make("tracks_4mono.mov", &[440, 550, 660, 770], PCM24, &[]), 4);
}

#[test]
fn mxf_four_mono_tracks_are_one_group() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    group_checks(&make("tracks_4mono.mxf", &[440, 550, 660, 770], PCM24, &[]), 4);
}

#[test]
fn mxf_eight_mono_tracks_are_one_group() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    let f: Vec<u32> = (0..8).map(|k| 440 + 110 * k).collect();
    group_checks(&make("tracks_8mono.mxf", &f, PCM24, &[]), 8);
}

#[test]
fn a_single_mono_track_is_not_a_group() {
    if !have_ffmpeg() {
        return eprintln!("skipped: ffmpeg not found");
    }
    let clip = make("tracks_1mono.mov", &[440], PCM24, &[]);
    let list = movie(&clip).audio_tracks();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].channels, 1);
    assert!(!AudioDecoder::open(&clip).unwrap().unwrap().is_group());
}
