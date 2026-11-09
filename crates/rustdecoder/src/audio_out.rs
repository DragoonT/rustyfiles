//! Audio output through WASAPI (shared mode, event driven) and the audio clock.
//!
//! [`AudioOutput`] opens the default render device in the device's own mix format, converts
//! what it is given (any channel count and rate: [`crate::resample`]) into that format, and a
//! render thread feeds the device from a queue whenever the device asks. It implements
//! [`AudioSink`], which is all [`crate::playback::Playback`] needs, so tests can swap in a fake.
//!
//! # The clock
//!
//! `IAudioClock::GetPosition` says how many frames the device has played since the stream was
//! last reset. Two things are corrected so the clock follows the sound that was actually heard:
//!
//! * frames the render thread wrote as silence because the queue ran dry (an under-run) are
//!   subtracted once they have played, so the clock stands still while the sound does, and the
//!   video waits instead of running ahead; silence after [`AudioSink::end_of_data`] is real
//!   time and counts;
//! * the position only moves in device periods (10 ms in shared mode), so it is interpolated
//!   with the QPC time `GetPosition` reports for it, never further than 20 ms, and never
//!   backwards.
//!
//! [`AudioClockSource`] (from [`AudioSink::time_source`]) is that clock as a [`TimeSource`]: seconds of
//! sound played since the output was opened, standing still when paused. A [`crate::player::Clock`]
//! over it is the A/V master clock.
//!
//! # Silence
//!
//! `muted: true` does three things before the stream starts: sets the session's master volume
//! to 0, sets the session mute, and from then on hands every buffer to the device flagged
//! `AUDCLNT_BUFFERFLAGS_SILENT` (zeros), while still consuming the queue at the normal speed
//! so the clock runs as usual. Tests and examples use it.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};

use crate::player::TimeSource;
use crate::resample::{Resampler, map_channels};
use crate::{Error, Result};

/// What [`crate::playback::Playback`] needs from an audio output.
pub trait AudioSink: Send + Sync {
    /// The format of what [`AudioSink::push`] gets: interleaved `f32` at `rate`.
    fn set_input(&self, rate: u32, channels: usize);
    /// Queues audio that belongs at movie time `pts` seconds. The first push after a
    /// [`AudioSink::flush`] gives the sink its position; later pushes continue it.
    fn push(&self, pts: f64, pcm: &[f32]);
    /// No more audio until the next flush: the clock keeps running over the silence after the
    /// queue is empty.
    fn end_of_data(&self);
    /// Seconds of audio queued and not yet played.
    fn buffered(&self) -> f64;
    /// Throws away everything queued and played so far; the sink is paused afterwards.
    fn flush(&self);
    fn play(&self);
    fn pause(&self);
    /// Movie time of the sound being heard now; `None` before the first push.
    fn position(&self) -> Option<f64>;
    /// The clock as a time source (seconds of sound played, see the module comment).
    fn time_source(&self) -> Arc<dyn TimeSource>;
}

/// Options for [`AudioOutput::open`].
#[derive(Debug, Clone, Copy)]
pub struct OutputOptions {
    /// Nothing audible: session volume 0, session mute, silent buffers. See the module comment.
    pub muted: bool,
    /// Linear gain 0..1 applied to the samples (ignored while muted).
    pub volume: f32,
}

impl Default for OutputOptions {
    fn default() -> Self {
        // silent unless asked otherwise: nothing in tests makes a sound by accident
        OutputOptions { muted: true, volume: 1.0 }
    }
}

struct SendBox<T>(T);
// SAFETY: WASAPI interfaces and event handles are used from several threads of the same
// multithreaded apartment, which they allow.
unsafe impl<T> Send for SendBox<T> {}
unsafe impl<T> Sync for SendBox<T> {}

#[derive(Clone, Copy, PartialEq)]
enum SampleFormat {
    F32,
    I16,
}

struct Ring {
    /// Device-format samples (interleaved, `dev_channels` per frame) not yet handed over.
    data: VecDeque<f32>,
    eos: bool,
    /// Movie time of the first frame after the last flush.
    base_media: Option<f64>,
    /// Device frames written since the stream's last reset.
    written: u64,
    /// Silence the render thread wrote for lack of data: (first device frame, frames).
    silence: Vec<(u64, u64)>,
    underruns: u64,
    /// Seconds of non-silent sound played before the last reset.
    cum: f64,
    /// Position the device reported right after the last reset, in seconds.
    pos_zero: f64,
    /// Greatest value returned by the clock since the last reset (it never goes back).
    last: f64,
    running: bool,
}

struct Converter {
    in_rate: u32,
    in_channels: usize,
    resampler: Option<Resampler>,
}

struct Inner {
    ring: Mutex<Ring>,
    conv: Mutex<Converter>,
    clock: SendBox<IAudioClock>,
    volume: SendBox<ISimpleAudioVolume>,
    freq: f64,
    rate: u32,
    channels: usize,
    gain: AtomicU32,
    muted: AtomicBool,
    qpc_freq: f64,
    stop: AtomicBool,
    started: AtomicU64,
}

impl Inner {
    fn qpc_100ns(&self) -> f64 {
        let mut c = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut c);
        }
        c as f64 * 1e7 / self.qpc_freq
    }

    /// Device position in seconds since the last reset, interpolated.
    fn device_secs(&self, running: bool, last: f64) -> f64 {
        let (mut p, mut q) = (0u64, 0u64);
        if unsafe { self.clock.0.GetPosition(&mut p, Some(&mut q)) }.is_err() {
            return last;
        }
        let mut s = p as f64 / self.freq;
        if running {
            let dt = ((self.qpc_100ns() - q as f64) * 1e-7).clamp(0.0, 0.02);
            s += dt;
        }
        s
    }

    /// Seconds of sound (silence from under-runs excluded) played in total.
    fn played(&self) -> f64 {
        let mut r = self.ring.lock().unwrap();
        let s = (self.device_secs(r.running, r.last) - r.pos_zero).max(0.0);
        let frames = s * self.rate as f64;
        let silent: f64 = r.silence.iter().map(|&(a, n)| (frames - a as f64).clamp(0.0, n as f64)).sum();
        let t = r.cum + (frames - silent) / self.rate as f64;
        let t = t.max(r.last_total());
        r.last = t - r.cum;
        t
    }
}

impl Ring {
    fn last_total(&self) -> f64 {
        self.cum + self.last
    }
}

/// The audio clock as a time source.
pub struct AudioClockSource(Arc<Inner>);

impl TimeSource for AudioClockSource {
    fn now(&self) -> f64 {
        self.0.played()
    }
}

enum Cmd {
    Play(Sender<()>),
    Pause(Sender<()>),
    Flush(Sender<()>),
    Quit,
}

/// WASAPI shared-mode output.
pub struct AudioOutput {
    inner: Arc<Inner>,
    cmd: Sender<Cmd>,
    wake: SendBox<HANDLE>,
    thread: Mutex<Option<JoinHandle<()>>>,
    device_name: String,
}

impl AudioOutput {
    /// Opens the default render device. Fails when there is none.
    pub fn open(opts: OutputOptions) -> Result<AudioOutput> {
        let (init_tx, init_rx) = channel::<std::result::Result<(SendBox<HANDLE>, String), String>>();
        let (cmd_tx, cmd_rx) = channel::<Cmd>();
        let inner_slot: Arc<Mutex<Option<Arc<Inner>>>> = Arc::new(Mutex::new(None));
        let slot = inner_slot.clone();
        let thread = std::thread::Builder::new()
            .name("rustdecoder-audio-out".into())
            .spawn(move || render_thread(opts, init_tx, cmd_rx, slot))
            .map_err(Error::Io)?;
        let (wake, name) = match init_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = thread.join();
                return Err(Error::Gpu(format!("audio output: {e}")));
            }
            Err(_) => return Err(Error::Gpu("audio output: no answer from the device".into())),
        };
        let inner = inner_slot.lock().unwrap().take().ok_or(Error::Gpu("audio output: not initialised".into()))?;
        Ok(AudioOutput { inner, cmd: cmd_tx, wake, thread: Mutex::new(Some(thread)), device_name: name })
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    /// The device's mix format: rate and channels.
    pub fn device_format(&self) -> (u32, usize) {
        (self.inner.rate, self.inner.channels)
    }

    pub fn set_volume(&self, v: f32) {
        self.inner.gain.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    /// Switches the silence of [`OutputOptions::muted`] on or off.
    pub fn set_muted(&self, muted: bool) {
        self.inner.muted.store(muted, Ordering::SeqCst);
        unsafe {
            if muted {
                let _ = self.inner.volume.0.SetMasterVolume(0.0, std::ptr::null());
                let _ = self.inner.volume.0.SetMute(true, std::ptr::null());
            } else {
                let _ = self.inner.volume.0.SetMute(false, std::ptr::null());
                let _ = self.inner.volume.0.SetMasterVolume(1.0, std::ptr::null());
            }
        }
    }

    /// Times the render thread had to write silence because the queue was empty.
    pub fn underruns(&self) -> u64 {
        self.inner.ring.lock().unwrap().underruns
    }

    /// Times the render thread has been woken by the device (shows the stream is alive).
    pub fn device_periods(&self) -> u64 {
        self.inner.started.load(Ordering::Relaxed)
    }

    fn send(&self, make: impl FnOnce(Sender<()>) -> Cmd) {
        let (tx, rx) = channel();
        if self.cmd.send(make(tx)).is_ok() {
            unsafe {
                let _ = SetEvent(self.wake.0);
            }
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

impl Drop for AudioOutput {
    fn drop(&mut self) {
        self.inner.stop.store(true, Ordering::SeqCst);
        let _ = self.cmd.send(Cmd::Quit);
        unsafe {
            let _ = SetEvent(self.wake.0);
        }
        if let Some(h) = self.thread.lock().ok().and_then(|mut g| g.take()) {
            let _ = h.join();
        }
    }
}

impl AudioSink for AudioOutput {
    fn set_input(&self, rate: u32, channels: usize) {
        let mut c = self.inner.conv.lock().unwrap();
        c.in_rate = rate;
        c.in_channels = channels.max(1);
        c.resampler = (rate != self.inner.rate).then(|| Resampler::new(rate, self.inner.rate, self.inner.channels));
    }

    fn push(&self, pts: f64, pcm: &[f32]) {
        let mut mapped = Vec::new();
        let mut out = Vec::new();
        {
            let mut c = self.inner.conv.lock().unwrap();
            map_channels(pcm, c.in_channels, self.inner.channels, &mut mapped);
            match c.resampler.as_mut() {
                Some(r) => r.process(&mapped, &mut out),
                None => out = mapped,
            }
        }
        let mut r = self.inner.ring.lock().unwrap();
        r.base_media.get_or_insert(pts);
        r.data.extend(out);
    }

    fn end_of_data(&self) {
        // the resampler holds back the last few input frames: give them out
        let mut out = Vec::new();
        if let Some(rs) = self.inner.conv.lock().unwrap().resampler.as_mut() {
            rs.finish(&mut out);
        }
        let mut r = self.inner.ring.lock().unwrap();
        r.data.extend(out);
        r.eos = true;
    }

    fn buffered(&self) -> f64 {
        let r = self.inner.ring.lock().unwrap();
        (r.data.len() / self.inner.channels) as f64 / self.inner.rate as f64
    }

    fn flush(&self) {
        if let Some(rs) = self.inner.conv.lock().unwrap().resampler.as_mut() {
            rs.reset();
        }
        self.send(Cmd::Flush);
    }

    fn play(&self) {
        self.send(Cmd::Play);
    }

    fn pause(&self) {
        self.send(Cmd::Pause);
    }

    fn position(&self) -> Option<f64> {
        let base = self.inner.ring.lock().unwrap().base_media?;
        // `played` counts from the stream's start; the ring's `cum` was reset at the flush,
        // so subtract what had been played at that moment
        let played = self.inner.played();
        let at_flush = self.inner.ring.lock().unwrap().cum_at_flush();
        Some(base + (played - at_flush).max(0.0))
    }

    fn time_source(&self) -> Arc<dyn TimeSource> {
        Arc::new(AudioClockSource(self.inner.clone()))
    }
}

impl Ring {
    /// `cum` is bumped at every flush by what had played; the movie position counts from there.
    fn cum_at_flush(&self) -> f64 {
        self.cum
    }
}

// ---------------------------------------------------------------------------------------------
// the render thread

fn fail<T>(what: &str, e: windows::core::Error) -> std::result::Result<T, String> {
    Err(format!("{what}: {e}"))
}

fn render_thread(
    opts: OutputOptions,
    init_tx: Sender<std::result::Result<(SendBox<HANDLE>, String), String>>,
    cmd_rx: Receiver<Cmd>,
    slot: Arc<Mutex<Option<Arc<Inner>>>>,
) {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let setup = (|| -> std::result::Result<_, String> {
        unsafe {
            let en: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                Ok(e) => e,
                Err(e) => return fail("device enumerator", e),
            };
            let dev = match en.GetDefaultAudioEndpoint(eRender, eConsole) {
                Ok(d) => d,
                Err(e) => return fail("no default output device", e),
            };
            let client: IAudioClient = match dev.Activate(CLSCTX_ALL, None) {
                Ok(c) => c,
                Err(e) => return fail("activate", e),
            };
            let mix = match client.GetMixFormat() {
                Ok(m) => m,
                Err(e) => return fail("mix format", e),
            };
            let wf = *mix;
            let tag = wf.wFormatTag;
            let bits = wf.wBitsPerSample;
            let format = if tag == 3 || (tag == 0xFFFE && {
                let ext = &*(mix as *const WAVEFORMATEXTENSIBLE);
                ext.SubFormat.data1 == 3
            }) {
                (bits == 32).then_some(SampleFormat::F32)
            } else if tag == 1 || tag == 0xFFFE {
                (bits == 16).then_some(SampleFormat::I16)
            } else {
                None
            };
            let Some(format) = format else {
                CoTaskMemFree(Some(mix as *const _));
                return Err(format!("mix format not supported (tag {tag:#x}, {bits} bits)"));
            };
            let (rate, channels) = (wf.nSamplesPerSec, wf.nChannels as usize);
            if let Err(e) = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, 1_000_000, 0, mix, None) {
                CoTaskMemFree(Some(mix as *const _));
                return fail("initialize", e);
            }
            CoTaskMemFree(Some(mix as *const _));
            let event = match CreateEventW(None, false, false, None) {
                Ok(h) => h,
                Err(e) => return fail("event", e),
            };
            let wake = match CreateEventW(None, false, false, None) {
                Ok(h) => h,
                Err(e) => return fail("event", e),
            };
            if let Err(e) = client.SetEventHandle(event) {
                return fail("event handle", e);
            }
            let size = match client.GetBufferSize() {
                Ok(s) => s,
                Err(e) => return fail("buffer size", e),
            };
            let render: IAudioRenderClient = match client.GetService() {
                Ok(r) => r,
                Err(e) => return fail("render client", e),
            };
            let clock: IAudioClock = match client.GetService() {
                Ok(r) => r,
                Err(e) => return fail("clock", e),
            };
            let volume: ISimpleAudioVolume = match client.GetService() {
                Ok(r) => r,
                Err(e) => return fail("session volume", e),
            };
            let freq = clock.GetFrequency().map_err(|e| format!("clock frequency: {e}"))? as f64;
            let name = String::from("default render device");
            Ok((client, render, clock, volume, event, wake, size, rate, channels, format, freq, name))
        }
    })();
    let (client, render, clock, volume, event, wake, size, rate, channels, format, freq, name) = match setup {
        Ok(v) => v,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    // silence first: before anything can be played
    unsafe {
        if opts.muted {
            let _ = volume.SetMasterVolume(0.0, std::ptr::null());
            let _ = volume.SetMute(true, std::ptr::null());
        } else {
            let _ = volume.SetMute(false, std::ptr::null());
            let _ = volume.SetMasterVolume(1.0, std::ptr::null());
        }
    }
    let mut qf = 0i64;
    unsafe {
        let _ = QueryPerformanceFrequency(&mut qf);
    }
    let inner = Arc::new(Inner {
        ring: Mutex::new(Ring {
            data: VecDeque::new(),
            eos: false,
            base_media: None,
            written: 0,
            silence: Vec::new(),
            underruns: 0,
            cum: 0.0,
            pos_zero: 0.0,
            last: 0.0,
            running: false,
        }),
        conv: Mutex::new(Converter { in_rate: rate, in_channels: channels, resampler: None }),
        clock: SendBox(clock),
        volume: SendBox(volume),
        freq,
        rate,
        channels,
        gain: AtomicU32::new(opts.volume.clamp(0.0, 1.0).to_bits()),
        muted: AtomicBool::new(opts.muted),
        qpc_freq: qf.max(1) as f64,
        stop: AtomicBool::new(false),
        started: AtomicU64::new(0),
    });
    *slot.lock().unwrap() = Some(inner.clone());
    let _ = init_tx.send(Ok((SendBox(wake), name)));

    let handles = [event, wake];
    let fill = |max_frames: u32| {
        if max_frames == 0 {
            return;
        }
        unsafe {
            let Ok(p) = render.GetBuffer(max_frames) else { return };
            let n = max_frames as usize;
            let muted = inner.muted.load(Ordering::Relaxed);
            let gain = f32::from_bits(inner.gain.load(Ordering::Relaxed));
            let mut flags = 0u32;
            let mut r = inner.ring.lock().unwrap();
            let have = (r.data.len() / channels).min(n);
            let short = n - have;
            let mut tmp = Vec::with_capacity(have * channels);
            tmp.extend(r.data.drain(..have * channels));
            if short > 0 && !r.eos {
                let start = r.written + have as u64;
                r.silence.push((start, short as u64));
                r.underruns += 1;
            }
            r.written += n as u64;
            drop(r);
            if muted || have == 0 {
                flags |= AUDCLNT_BUFFERFLAGS_SILENT.0 as u32;
            }
            match format {
                SampleFormat::F32 => {
                    let out = std::slice::from_raw_parts_mut(p as *mut f32, n * channels);
                    if flags != 0 {
                        out.fill(0.0);
                    } else {
                        for (o, s) in out.iter_mut().zip(&tmp) {
                            *o = (s * gain).clamp(-1.0, 1.0);
                        }
                        out[tmp.len()..].fill(0.0);
                    }
                }
                SampleFormat::I16 => {
                    let out = std::slice::from_raw_parts_mut(p as *mut i16, n * channels);
                    if flags != 0 {
                        out.fill(0);
                    } else {
                        for (o, s) in out.iter_mut().zip(&tmp) {
                            *o = ((s * gain).clamp(-1.0, 1.0) * 32767.0).round() as i16;
                        }
                        out[tmp.len()..].fill(0);
                    }
                }
            }
            let _ = render.ReleaseBuffer(max_frames, flags);
        }
    };

    let mut running = false;
    loop {
        // woken by the device (buffer space) or by a command
        let wait = unsafe { WaitForMultipleObjects(&handles, false, if running { 500 } else { INFINITE }) };
        if inner.stop.load(Ordering::SeqCst) {
            break;
        }
        while let Ok(c) = cmd_rx.try_recv() {
            match c {
                Cmd::Quit => {
                    unsafe {
                        let _ = client.Stop();
                    }
                    return;
                }
                Cmd::Pause(ack) => {
                    if running {
                        unsafe {
                            let _ = client.Stop();
                        }
                        running = false;
                        inner.ring.lock().unwrap().running = false;
                    }
                    let _ = ack.send(());
                }
                Cmd::Play(ack) => {
                    if !running {
                        // fill the device buffer before it starts, so it never starts empty
                        let pad = unsafe { client.GetCurrentPadding() }.unwrap_or(0);
                        fill(size.saturating_sub(pad));
                        inner.ring.lock().unwrap().running = true;
                        // refresh the interpolation base: the position is at rest until now
                        unsafe {
                            let _ = client.Start();
                        }
                        running = true;
                    }
                    let _ = ack.send(());
                }
                Cmd::Flush(ack) => {
                    unsafe {
                        if running {
                            let _ = client.Stop();
                        }
                        running = false;
                        let (mut before, mut after) = (0u64, 0u64);
                        let _ = inner.clock.0.GetPosition(&mut before, None);
                        let _ = client.Reset();
                        let _ = inner.clock.0.GetPosition(&mut after, None);
                        let mut r = inner.ring.lock().unwrap();
                        // sound played so far, with the silence taken out
                        let b = (before as f64 / freq - r.pos_zero).max(0.0) * rate as f64;
                        let silent: f64 = r.silence.iter().map(|&(a, n)| (b - a as f64).clamp(0.0, n as f64)).sum();
                        r.cum += (b - silent) / rate as f64;
                        r.pos_zero = after as f64 / freq;
                        r.silence.clear();
                        r.written = 0;
                        r.last = 0.0;
                        r.data.clear();
                        r.eos = false;
                        r.base_media = None;
                        r.running = false;
                    }
                    let _ = ack.send(());
                }
            }
        }
        if running && wait == WAIT_OBJECT_0 {
            inner.started.fetch_add(1, Ordering::Relaxed);
            let pad = unsafe { client.GetCurrentPadding() }.unwrap_or(0);
            fill(size.saturating_sub(pad));
        } else if running {
            // timeout or wake-up: top up anyway
            let pad = unsafe { client.GetCurrentPadding() }.unwrap_or(0);
            fill(size.saturating_sub(pad));
        }
    }
    unsafe {
        let _ = client.Stop();
    }
}
