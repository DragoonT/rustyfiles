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
//! THE MICROPHONE is the same module once more: a second thread of the same kind follows the default INPUT device (its level, its
//! mute, its name, the list of inputs) from the first [`subscribe_input`] on, and takes the same three writes
//! ([`set_input_volume`], [`set_input_muted`], [`set_default_input`]). Everything below that says "output" holds for it with
//! "input" ([`Flow`] is the only difference between the two threads).
//!
//! THE DEFAULT OUTPUT. [`set_default_output`] is the user's click on a device of the panel's list (issue 22, ElectroUndead: "When you
//! right click volume icon - you can change default output device, you can use EarTrumpet as reference. As for these menu is not
//! clickable"). Core Audio has NO documented call for it: Windows' own panels, and the tools that offer the same (EarTrumpet among
//! them), go through `IPolicyConfig` of the audio service's policy client, an interface Microsoft never published. It is declared
//! here by hand ([`IPolicyConfig`]) and only its `SetDefaultEndpoint` is called, on the audio thread, for the two roles of what
//! plays (`OUTPUT_ROLES`). Should a Windows to come drop the interface, the call fails, the log says so, and the list is what it
//! was before this: something to read, with the link to Windows' sound page under it.
//!
//! After the default device changes the thread waits 150 ms for the burst to end (a Bluetooth headset's profile switch is a handful
//! of removed / added events) and then attaches to the new default output.

#![allow(non_snake_case)] // the hand-declared interface keeps the names of its COM methods

use std::ffi::c_void;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioEndpointVolumeCallback, IAudioEndpointVolumeCallback_Impl};
use windows::Win32::Media::Audio::{
    AUDIO_VOLUME_NOTIFICATION_DATA, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole, IMMDevice, IMMDeviceEnumerator, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, eCapture, eConsole, eMultimedia, eRender,
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
    /// The user chose this device (its id) as the default output.
    Default(String),
}

struct Engine {
    hub: Hub<AudioSnapshot>,
    tx: Sender<Msg>,
    volume: Arc<Shared<f32>>,
    mute: Arc<Shared<bool>>,
}

/// Which side of the sound a thread follows: what plays, or what is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Output,
    Input,
}

impl Flow {
    fn data(self) -> EDataFlow {
        match self {
            Flow::Output => eRender,
            Flow::Input => eCapture,
        }
    }
}

static ENGINE: OnceLock<Engine> = OnceLock::new();
static INPUT: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    engine_of(Flow::Output)
}

fn engine_of(flow: Flow) -> &'static Engine {
    let (slot, name) = match flow {
        Flow::Output => (&ENGINE, "rf-audio"),
        Flow::Input => (&INPUT, "rf-audio-in"),
    };
    slot.get_or_init(|| {
        let hub = Hub::new();
        let (tx, rx) = channel::<Msg>();
        let (volume, mute) = (Shared::new(), Shared::new());
        let (h, t, v, m) = (hub.clone(), tx.clone(), volume.clone(), mute.clone());
        let started = std::thread::Builder::new().name(name.into()).spawn(move || run(flow, rx, t, h, v, m));
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

/// The user chose an output device in the panel's list: asks Windows to make it the default output. Returns at once; the picture
/// that follows (`subscribe`) has the new default when Windows has changed it. An id that is not in the list, or that is the default
/// already, is dropped.
pub fn set_default_output(id: &str) {
    let _ = engine().tx.send(Msg::Default(id.to_string()));
}

/// Is `id` a device the default output can be switched to: one of the list that is not the default now.
pub fn can_switch_to(s: &AudioSnapshot, id: &str) -> bool {
    !id.is_empty() && s.devices.iter().any(|d| d.id == id && !d.default)
}

// ------------------------------------------------------------------ the microphone (the same, for the default input)

/// Follows the default input device (the microphone): as [`subscribe`]. Starts its thread the first time.
pub fn subscribe_input(callback: impl Fn(&AudioSnapshot) + Send + Sync + 'static) -> Subscription {
    engine_of(Flow::Input).hub.subscribe(callback)
}

/// The microphone's state as last read (`None` before the first reading).
pub fn input_snapshot() -> Option<AudioSnapshot> {
    INPUT.get().and_then(|e| e.hub.latest())
}

/// The user moved the microphone's slider: as [`set_volume`].
pub fn set_input_volume(fraction: f32) {
    let e = engine_of(Flow::Input);
    let steps = e.hub.latest().map_or(0, |s| s.steps);
    if e.volume.push(slider_to_volume(fraction, steps)) {
        let _ = e.tx.send(Msg::Write);
    }
}

/// The user clicked the microphone: mutes or unmutes it.
pub fn set_input_muted(muted: bool) {
    let e = engine_of(Flow::Input);
    if e.mute.push(muted) {
        let _ = e.tx.send(Msg::Write);
    }
}

/// The user chose an input device in the panel's list: as [`set_default_output`].
pub fn set_default_input(id: &str) {
    let _ = engine_of(Flow::Input).tx.send(Msg::Default(id.to_string()));
}

/// Reads the state once on the calling thread (a probe; the panel follows with [`subscribe`]). Blocks.
pub fn read_once() -> AudioSnapshot {
    init_mta();
    let Ok(en) = (unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }) else { return AudioSnapshot::default() };
    let (tx, _rx) = channel::<Msg>();
    let ep = Endpoint::attach(Flow::Output, &en, &tx);
    build(Flow::Output, &en, ep.as_ref())
}

/// The microphone's state read once on the calling thread (a probe). Blocks.
pub fn read_input_once() -> AudioSnapshot {
    init_mta();
    let Ok(en) = (unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }) else { return AudioSnapshot::default() };
    let (tx, _rx) = channel::<Msg>();
    let ep = Endpoint::attach(Flow::Input, &en, &tx);
    build(Flow::Input, &en, ep.as_ref())
}

/// The default output device and what is registered on it. Dropping it takes the registration off.
struct Endpoint {
    id: String,
    name: String,
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
}

impl Endpoint {
    fn attach(flow: Flow, en: &IMMDeviceEnumerator, tx: &Sender<Msg>) -> Option<Endpoint> {
        unsafe {
            let dev = en.GetDefaultAudioEndpoint(flow.data(), eConsole).ok()?;
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

fn list_devices(flow: Flow, en: &IMMDeviceEnumerator, default_id: &str) -> Vec<Device> {
    let mut out = Vec::new();
    unsafe {
        let Ok(coll) = en.EnumAudioEndpoints(flow.data(), DEVICE_STATE_ACTIVE) else { return out };
        for i in 0..coll.GetCount().unwrap_or(0) {
            let Ok(dev) = coll.Item(i) else { continue };
            let id = device_id(&dev).unwrap_or_default();
            let name = friendly_name(&dev).unwrap_or_else(|| id.clone());
            out.push(Device { default: !default_id.is_empty() && id == default_id, id, name });
        }
    }
    out
}

fn build(flow: Flow, en: &IMMDeviceEnumerator, ep: Option<&Endpoint>) -> AudioSnapshot {
    match ep {
        Some(ep) => {
            let (volume, muted) = ep.level();
            AudioSnapshot { available: true, volume, muted, steps: ep.steps(), device: ep.name.clone(), devices: list_devices(flow, en, &ep.id) }
        }
        None => AudioSnapshot { devices: list_devices(flow, en, ""), ..AudioSnapshot::default() },
    }
}

fn run(flow: Flow, rx: Receiver<Msg>, tx: Sender<Msg>, hub: Hub<AudioSnapshot>, volume_q: Arc<Shared<f32>>, mute_q: Arc<Shared<bool>>) {
    init_mta();
    let Ok(en) = (unsafe { CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) }) else {
        log::warn!("audio: no device enumerator");
        hub.publish(AudioSnapshot::default());
        return;
    };
    let events: IMMNotificationClient = DeviceEvents { tx: tx.clone(), flow: flow.data() }.into();
    if let Err(e) = unsafe { en.RegisterEndpointNotificationCallback(&events) } {
        log::debug!("audio: no device notifications: {e}");
    }
    let mut ep = Endpoint::attach(flow, &en, &tx);
    let mut snap = build(flow, &en, ep.as_ref());
    hub.publish(snap.clone());
    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Devices => {
                // wait for the burst to end, keeping the last level that was reported meanwhile
                while rx.recv_timeout(BURST).is_ok() {}
                // (the old registration comes off first, then the new default output is attached)
                drop(ep.take());
                ep = Endpoint::attach(flow, &en, &tx);
                snap = build(flow, &en, ep.as_ref());
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
            Msg::Default(id) => {
                if can_switch_to(&snap, &id) {
                    match make_default(&id) {
                        // The service tells of the change by itself (`OnDefaultDeviceChanged`). The picture is read again either way,
                        // by the same path, once the burst of events has ended.
                        Ok(()) => {
                            let _ = tx.send(Msg::Devices);
                        }
                        Err(e) => log::warn!("audio: the default output was not changed: {e}"),
                    }
                }
            }
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

// ------------------------------------------------------------------ the default output (an interface Microsoft did not publish)

/// `IPolicyConfig` as the audio service's policy client has it since Windows 7 (the module docs say why it is here). The methods
/// are declared in their order ONLY to put `SetDefaultEndpoint` in its slot, the eleventh after `IUnknown`'s three; none of the
/// others is called, and the pointers of their arguments are left untyped.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    fn GetMixFormat(&self, id: *const u16, format: *mut *mut c_void) -> windows::core::HRESULT;
    fn GetDeviceFormat(&self, id: *const u16, default: i32, format: *mut *mut c_void) -> windows::core::HRESULT;
    fn ResetDeviceFormat(&self, id: *const u16) -> windows::core::HRESULT;
    fn SetDeviceFormat(&self, id: *const u16, endpoint: *mut c_void, mix: *mut c_void) -> windows::core::HRESULT;
    fn GetProcessingPeriod(&self, id: *const u16, default: i32, usual: *mut i64, least: *mut i64) -> windows::core::HRESULT;
    fn SetProcessingPeriod(&self, id: *const u16, period: *mut i64) -> windows::core::HRESULT;
    fn GetShareMode(&self, id: *const u16, mode: *mut c_void) -> windows::core::HRESULT;
    fn SetShareMode(&self, id: *const u16, mode: *mut c_void) -> windows::core::HRESULT;
    fn GetPropertyValue(&self, id: *const u16, key: *const PROPERTYKEY, value: *mut c_void) -> windows::core::HRESULT;
    fn SetPropertyValue(&self, id: *const u16, key: *const PROPERTYKEY, value: *mut c_void) -> windows::core::HRESULT;
    fn SetDefaultEndpoint(&self, id: *const u16, role: i32) -> windows::core::HRESULT;
    fn SetEndpointVisibility(&self, id: *const u16, visible: i32) -> windows::core::HRESULT;
}

/// The class that answers for [`IPolicyConfig`] (`CPolicyConfigClient`).
const POLICY_CONFIG_CLIENT: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

/// The roles a chosen output takes: what plays (music and films; games and the system's sounds). The device for calls is a choice
/// of its own in Windows and is left as it is, as Windows' own volume panel leaves it. The one this module follows (`eConsole`) is
/// the last, so that the change it is told of comes when both are done.
const OUTPUT_ROLES: [ERole; 2] = [eMultimedia, eConsole];

/// Makes the device the default output. On the calling thread, which must be in a COM apartment.
fn make_default(id: &str) -> windows::core::Result<()> {
    let policy: IPolicyConfig = unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL)? };
    let w = crate::wide::wide(id);
    for role in OUTPUT_ROLES {
        unsafe { policy.SetDefaultEndpoint(w.as_ptr(), role.0).ok()? };
    }
    Ok(())
}

// ------------------------------------------------------------------ the callbacks (post a message, nothing else)

#[implement(IMMNotificationClient)]
struct DeviceEvents {
    tx: Sender<Msg>,
    /// The side this thread follows: only its default device's change matters to it.
    flow: EDataFlow,
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
        if flow == self.flow && role == eConsole {
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
    fn the_default_output_is_switched_only_to_another_device_of_the_list() {
        let dev = |id: &str, default: bool| Device { id: id.into(), name: id.to_uppercase(), default };
        let s = AudioSnapshot { available: true, devices: vec![dev("speakers", true), dev("headset", false), dev("monitor", false)], ..Default::default() };
        assert!(can_switch_to(&s, "headset"));
        assert!(can_switch_to(&s, "monitor"));
        assert!(!can_switch_to(&s, "speakers"), "the default one already is");
        assert!(!can_switch_to(&s, "gone"), "a device of an older picture");
        assert!(!can_switch_to(&s, ""));
        assert!(!can_switch_to(&AudioSnapshot::default(), "headset"));
        // no default at all (the last one was unplugged): any device of the list is a choice
        let none = AudioSnapshot { available: false, devices: vec![dev("headset", false)], ..Default::default() };
        assert!(can_switch_to(&none, "headset"));
        // what plays, with the role this module follows as the last
        assert_eq!(OUTPUT_ROLES, [eMultimedia, eConsole]);
    }

    /// On the PC it is run on, by hand: the microphone as Windows tells it (how many inputs, never their names).
    #[test]
    #[ignore = "reads the input devices of the PC it runs on; run by hand"]
    fn what_this_pc_says_about_its_microphone() {
        let s = read_input_once();
        println!("available {} volume {:.2} muted {} steps {} inputs {} default among them {}", s.available, s.volume, s.muted, s.steps, s.devices.len(), s.devices.iter().any(|d| d.default));
    }

    /// On the PC it is run on: the audio service's policy client answers for the interface, and takes, for each role, the device
    /// that role ALREADY has (so nothing changes: the same device stays the default of the same role).
    #[test]
    #[ignore = "talks to the audio service of the PC it runs on; run by hand"]
    fn the_policy_client_takes_the_default_output_it_already_has() {
        init_mta();
        let en: IMMDeviceEnumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.expect("a device enumerator");
        let policy: IPolicyConfig = unsafe { CoCreateInstance(&POLICY_CONFIG_CLIENT, None, CLSCTX_ALL) }.expect("the policy client answers for IPolicyConfig");
        for role in OUTPUT_ROLES {
            // (a PC without a sound device has nothing to ask about)
            let Ok(dev) = (unsafe { en.GetDefaultAudioEndpoint(eRender, role) }) else { return };
            let id = unsafe { device_id(&dev) }.expect("the device has an id");
            let w = crate::wide::wide(&id);
            let hr = unsafe { policy.SetDefaultEndpoint(w.as_ptr(), role.0) };
            assert!(hr.is_ok(), "role {}: {hr:?}", role.0);
            let after = unsafe { en.GetDefaultAudioEndpoint(eRender, role).ok().and_then(|d| device_id(&d)) };
            assert_eq!(after.as_deref(), Some(id.as_str()), "role {}", role.0);
        }
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
