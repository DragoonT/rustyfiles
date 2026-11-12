//! Sound: the default output device's volume, mute and name, and the list of output devices (Core Audio).
//!
//! ONE THREAD, started by the first [`subscribe`] and kept for the life of the process (the right island shows the volume symbol all
//! the time). It is initialised as a multithreaded COM apartment, which is what the audio APIs and their callbacks want; it is not
//! the shell thread and it does not pump messages. It owns the `IMMDeviceEnumerator`, an `IMMNotificationClient` (devices come,
//! go, or the default one changes) and an `IAudioEndpointVolumeCallback` on the current default output (the volume or the mute is
//! changed by the keyboard keys, Windows' own panel, another program). The callbacks only post a small message to the thread's
//! channel and return: nothing is registered, unregistered or read inside a callback (COM callbacks of this kind can deadlock the
//! API that is calling them).
//!
//! WRITES. [`set_volume`] and [`set_muted`] are the user's drag or click in the panel. A drag sends a value for every pixel; each is
//! kept in a [`Latest`](super::Latest) slot, the thread is woken once, and only the newest value is applied: the slider never waits
//! for the COM call and the call is never made faster than it can be answered. Our own writes carry a context GUID of our own, and
//! the callback ignores the events that carry it (the thread has already updated its picture from what the endpoint answered), so the
//! slider does not fight the echo of its own drag.
//!
//! After the default device changes the thread waits 150 ms for the burst to end (a Bluetooth headset's profile switch is a handful
//! of removed / added events) and then attaches to the new default output.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, eConsole, eRender,
};
use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree, STGM_READ};
use windows::core::{GUID, PCWSTR, implement};

use super::{Hub, Shared, Subscription, init_mta};

/// One output device.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    /// The one Windows plays to now.
    pub default: bool,
}

/// What the interface draws the sound from.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct AudioSnapshot {
    /// There is a default output device that answered. False: no sound device (or the audio service is off): the symbol and the
    /// slider say so and do nothing.
    pub available: bool,
    /// The master volume as Windows' own slider sees it, 0.0 to 1.0.
    pub volume: f32,
    pub muted: bool,
    /// How many positions the endpoint's volume has (`GetVolumeStepInfo`; 0: not told).
    pub steps: u32,
    /// The default output device's name.
    pub device: String,
    /// The active output devices, the default one among them.
    pub devices: Vec<Device>,
}

// ------------------------------------------------------------------ pure: volume, slider, symbol

/// The slider's position (0.0 to 1.0) for a volume: the same scale (Windows' slider is the scalar volume).
pub fn volume_to_slider(volume: f32) -> f32 {
    clamp01(volume)
}

/// The volume to ask the endpoint for when the slider is at `fraction`: cut to 0..=1 and rounded to a position the endpoint has.
/// An endpoint with `steps` positions (`GetVolumeStepInfo`) takes `steps - 1` equal parts; one that did not say, or has more than
/// a hundred, is taken in whole percent (Windows shows whole percent).
pub fn slider_to_volume(fraction: f32, steps: u32) -> f32 {
    let parts = if steps >= 2 { (steps - 1).min(100) } else { 100 };
    let f = clamp01(fraction);
    (f * parts as f32).round() / parts as f32
}

fn clamp01(v: f32) -> f32 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

/// The volume as the whole percent Windows shows.
pub fn percent(volume: f32) -> u8 {
    (clamp01(volume) * 100.0).round() as u8
}

/// Which symbol the right island and the panel's slider show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeGlyph {
    /// No sound device.
    Unavailable,
    /// Muted (whatever the level).
    Muted,
    /// Level 0: a speaker with no waves.
    Silent,
    /// Up to the middle: one wave.
    Low,
    /// Above the middle: two waves.
    High,
}

pub fn glyph(s: &AudioSnapshot) -> VolumeGlyph {
    if !s.available {
        return VolumeGlyph::Unavailable;
    }
    if s.muted {
        return VolumeGlyph::Muted;
    }
    match percent(s.volume) {
        0 => VolumeGlyph::Silent,
        1..=50 => VolumeGlyph::Low,
        _ => VolumeGlyph::High,
    }
}

// ------------------------------------------------------------------ the thread

/// Marks our own writes (see the module docs).
const OWN_CONTEXT: GUID = GUID::from_u128(0x5a1c_3e7b_9d24_4f68_b0a3_72d1_c4e8_96f5);

/// How long the thread waits for a burst of device events to end.
const BURST: Duration = Duration::from_millis(150);

enum Msg {
    /// A device came, went, changed state, or the default changed.
    Devices,
    /// The volume or the mute of the current output changed (not by us).
    Level { volume: f32, muted: bool },
    /// A write is waiting in a slot.
    Write,
}

struct Engine {
    hub: Hub<AudioSnapshot>,
    tx: Sender<Msg>,
    volume: Arc<Shared<f32>>,
    mute: Arc<Shared<bool>>,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let hub = Hub::new();
        let (tx, rx) = channel::<Msg>();
        let (volume, mute) = (Shared::new(), Shared::new());
        let (h, t, v, m) = (hub.clone(), tx.clone(), volume.clone(), mute.clone());
        let started = std::thread::Builder::new().name("rf-audio".into()).spawn(move || run(rx, t, h, v, m));
        if let Err(e) = started {
            log::warn!("audio: the thread did not start: {e}");
            hub.publish(AudioSnapshot::default());
        }
        Engine { hub, tx, volume, mute }
    })
}

/// Follows the sound state. `callback` is called from the audio thread, with the current state at once (when it has been read) and
/// at every change. Starts the thread the first time.
pub fn subscribe(callback: impl Fn(&AudioSnapshot) + Send + Sync + 'static) -> Subscription {
    engine().hub.subscribe(callback)
}

/// The state as last read (`None` before the first reading, or when the thread never ran).
pub fn snapshot() -> Option<AudioSnapshot> {
    ENGINE.get().and_then(|e| e.hub.latest())
}

/// The user moved the slider: asks for this volume (`fraction` of the slider, 0.0 to 1.0). Returns at once; the newest value of a
/// drag is the one that is applied. Does nothing while there is no sound device.
pub fn set_volume(fraction: f32) {
    let e = engine();
    let steps = e.hub.latest().map_or(0, |s| s.steps);
    if e.volume.push(slider_to_volume(fraction, steps)) {
        let _ = e.tx.send(Msg::Write);
    }
}

/// The user clicked the speaker: mutes or unmutes.
pub fn set_muted(muted: bool) {
    let e = engine();
    if e.mute.push(muted) {
        let _ = e.tx.send(Msg::Write);
    }
}

/// Reads the state once on the calling thread (a probe; the panel follows with [`subscribe`]). Blocks.
pub fn read_once() -> AudioSnapshot {
    init_mta();
    let Ok(en) = (unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }) else { return AudioSnapshot::default() };
    let (tx, _rx) = channel::<Msg>();
    let ep = Endpoint::attach(&en, &tx);
    build(&en, ep.as_ref())
}

/// The default output device and what is registered on it. Dropping it takes the registration off.
struct Endpoint {
    id: String,
    name: String,
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Endpoint {
    fn attach(en: &IMMDeviceEnumerator, tx: &Sender<Msg>) -> Option<Endpoint> {
        unsafe {
            let dev = en.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            let volume: IAudioEndpointVolume = dev.Activate(CLSCTX_ALL, None).ok()?;
            let callback: IAudioEndpointVolumeCallback = VolumeEvents { tx: tx.clone() }.into();
            if let Err(e) = volume.RegisterControlChangeNotify(&callback) {
                log::debug!("audio: no volume notifications: {e}");
            }
            Some(Endpoint { id: device_id(&dev).unwrap_or_default(), name: friendly_name(&dev).unwrap_or_default(), volume, callback })
        }
    }

    fn level(&self) -> (f32, bool) {
        unsafe { (self.volume.GetMasterVolumeLevelScalar().unwrap_or(0.0), self.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false)) }
    }

    fn steps(&self) -> u32 {
        let (mut step, mut count) = (0u32, 0u32);
        unsafe {
            if self.volume.GetVolumeStepInfo(&mut step, &mut count).is_ok() { count } else { 0 }
        }
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        unsafe {
            let _ = self.volume.UnregisterControlChangeNotify(&self.callback);
        }
    }
}

unsafe fn device_id(dev: &IMMDevice) -> Option<String> {
    unsafe {
        let p = dev.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    }
}

unsafe fn friendly_name(dev: &IMMDevice) -> Option<String> {
    unsafe {
        let store = dev.OpenPropertyStore(STGM_READ).ok()?;
        let v = store.GetValue(&PKEY_Device_FriendlyName).ok()?;
        let s = PropVariantToStringAlloc(&v).ok()?;
        let out = s.to_string().ok();
        CoTaskMemFree(Some(s.0 as *const _));
        out.filter(|n| !n.trim().is_empty())
    }
}

fn list_devices(en: &IMMDeviceEnumerator, default_id: &str) -> Vec<Device> {
    let mut out = Vec::new();
    unsafe {
        let Ok(coll) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..coll.GetCount().unwrap_or(0) {
            let Ok(dev) = coll.Item(i) else { continue };
            let id = device_id(&dev).unwrap_or_default();
            let name = friendly_name(&dev).unwrap_or_else(|| id.clone());
            out.push(Device { default: !default_id.is_empty() && id == default_id, id, name });
        }
    }
    out
}

fn build(en: &IMMDeviceEnumerator, ep: Option<&Endpoint>) -> AudioSnapshot {
    match ep {
        Some(ep) => {
            let (volume, muted) = ep.level();
            AudioSnapshot { available: true, volume, muted, steps: ep.steps(), device: ep.name.clone(), devices: list_devices(en, &ep.id) }
        }
        None => AudioSnapshot { devices: list_devices(en, ""), ..AudioSnapshot::default() },
    }
}

fn run(rx: Receiver<Msg>, tx: Sender<Msg>, hub: Hub<AudioSnapshot>, volume_q: Arc<Shared<f32>>, mute_q: Arc<Shared<bool>>) {
    init_mta();
    let Ok(en) = (unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }) else {
        log::warn!("audio: no device enumerator");
        hub.publish(AudioSnapshot::default());
        return;
    };
    let events: IMMNotificationClient = DeviceEvents { tx: tx.clone() }.into();
    if let Err(e) = unsafe { en.RegisterEndpointNotificationCallback(&events) } {
        log::debug!("audio: no device notifications: {e}");
    }
    let mut ep = Endpoint::attach(&en, &tx);
    let mut snap = build(&en, ep.as_ref());
    hub.publish(snap.clone());
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Devices => {
                // wait for the burst to end, keeping the last level that was reported meanwhile
                while rx.recv_timeout(BURST).is_ok() {}
                // (the old registration comes off first, then the new default output is attached)
                drop(ep.take());
                ep = Endpoint::attach(&en, &tx);
                snap = build(&en, ep.as_ref());
                hub.publish(snap.clone());
            }
            Msg::Level { volume, muted } => {
                if snap.available && (snap.volume != volume || snap.muted != muted) {
                    snap.volume = volume;
                    snap.muted = muted;
                    hub.publish(snap.clone());
                }
            }
            Msg::Write => {}
        }
        // the writes the user is waiting for (also after a device change: a write that came meanwhile is not lost)
        if let (Some(ep), true) = (ep.as_ref(), snap.available) {
            let mut changed = false;
            if let Some(v) = volume_q.take() {
                unsafe {
                    if let Err(e) = ep.volume.SetMasterVolumeLevelScalar(v, &OWN_CONTEXT) {
                        log::debug!("audio: cannot set the volume: {e}");
                    }
                }
                changed = true;
            }
            if let Some(m) = mute_q.take() {
                unsafe {
                    if let Err(e) = ep.volume.SetMute(m, &OWN_CONTEXT) {
                        log::debug!("audio: cannot set the mute: {e}");
                    }
                }
                changed = true;
            }
            if changed {
                // what the endpoint answers is the truth (it may have taken the nearest position it has)
                let (volume, muted) = ep.level();
                if snap.volume != volume || snap.muted != muted {
                    snap.volume = volume;
                    snap.muted = muted;
                    hub.publish(snap.clone());
                }
            }
        } else {
            volume_q.take();
            mute_q.take();
        }
    }
    unsafe {
        let _ = en.UnregisterEndpointNotificationCallback(&events);
    }
}

// ------------------------------------------------------------------ the callbacks (post a message, nothing else)

#[implement(IMMNotificationClient)]
struct DeviceEvents {
    tx: Sender<Msg>,
}

#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for DeviceEvents_Impl {
    fn OnDeviceStateChanged(&self, _id: &PCWSTR, _state: DEVICE_STATE) -> windows::core::Result<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDeviceAdded(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _id: &PCWSTR) -> windows::core::Result<()> {
        let _ = self.tx.send(Msg::Devices);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, role: ERole, _id: &PCWSTR) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            let _ = self.tx.send(Msg::Devices);
        }
        Ok(())
    }
    // (fires for every property of every device, all the time: nothing we show depends on them)
    fn OnPropertyValueChanged(&self, _id: &PCWSTR, _key: &PROPERTYKEY) -> windows::core::Result<()> {
        Ok(())
    }
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents {
    tx: Sender<Msg>,
}

#[allow(non_snake_case)]
impl IAudioEndpointVolumeCallback_Impl for VolumeEvents_Impl {
    fn OnNotify(&self, notify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> windows::core::Result<()> {
        if notify.is_null() {
            return Ok(());
        }
        // SAFETY: Windows hands the callback a valid notification for the duration of the call
        let d = unsafe { &*notify };
        if d.guidEventContext != OWN_CONTEXT {
            let _ = self.tx.send(Msg::Level { volume: d.fMasterVolume, muted: d.bMuted.as_bool() });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(available: bool, volume: f32, muted: bool) -> AudioSnapshot {
        AudioSnapshot { available, volume, muted, ..Default::default() }
    }

    #[test]
    fn the_slider_and_the_volume_are_the_same_scale_and_come_back_whole() {
        for pct in 0..=100u32 {
            let v = pct as f32 / 100.0;
            let slider = volume_to_slider(v);
            assert_eq!(percent(slider_to_volume(slider, 0)), pct as u8, "{pct} %");
        }
        assert_eq!(volume_to_slider(1.4), 1.0);
        assert_eq!(volume_to_slider(-0.2), 0.0);
        assert_eq!(volume_to_slider(f32::NAN), 0.0);
    }

    #[test]
    fn a_pointer_position_is_cut_to_the_range_and_rounded_to_a_position_the_endpoint_has() {
        // no step info: whole percent
        assert_eq!(slider_to_volume(0.333, 0), 0.33);
        assert_eq!(slider_to_volume(7.0, 0), 1.0);
        assert_eq!(slider_to_volume(-1.0, 0), 0.0);
        assert_eq!(slider_to_volume(f32::NAN, 0), 0.0);
        // an endpoint with 21 positions (0..=20): a twentieth is the finest it takes
        let v = slider_to_volume(0.52, 21);
        assert!((v - 0.5).abs() < 1e-6, "{v}");
        assert!((slider_to_volume(0.53, 21) - 0.55).abs() < 1e-6);
        // an endpoint with more positions than a percent is taken in percent; one that reports a single position is not divided by zero
        assert_eq!(slider_to_volume(0.504, 1000), 0.5);
        assert_eq!(slider_to_volume(0.504, 1), 0.5);
        // two positions (off and full): the nearer one
        assert_eq!(slider_to_volume(0.504, 2), 1.0);
        assert_eq!(slider_to_volume(0.496, 2), 0.0);
    }

    #[test]
    fn the_symbol_follows_the_state_in_steps() {
        assert_eq!(glyph(&AudioSnapshot::default()), VolumeGlyph::Unavailable);
        assert_eq!(glyph(&snap(true, 0.8, true)), VolumeGlyph::Muted, "muted wins over the level");
        assert_eq!(glyph(&snap(true, 0.0, false)), VolumeGlyph::Silent);
        assert_eq!(glyph(&snap(true, 0.004, false)), VolumeGlyph::Silent, "rounds to 0 %");
        assert_eq!(glyph(&snap(true, 0.01, false)), VolumeGlyph::Low);
        assert_eq!(glyph(&snap(true, 0.50, false)), VolumeGlyph::Low);
        assert_eq!(glyph(&snap(true, 0.51, false)), VolumeGlyph::High);
        assert_eq!(glyph(&snap(true, 1.0, false)), VolumeGlyph::High);
        assert_eq!(glyph(&snap(false, 1.0, false)), VolumeGlyph::Unavailable, "a level without a device means nothing");
    }

    #[test]
    fn the_percent_is_what_windows_shows() {
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(0.455), 46);
        assert_eq!(percent(1.0), 100);
        assert_eq!(percent(9.0), 100);
    }

    #[test]
    fn our_own_writes_are_told_from_the_system_s() {
        // the context is a fixed non-null GUID: a write by another program carries none or its own
        assert_ne!(OWN_CONTEXT, GUID::zeroed());
    }
}
