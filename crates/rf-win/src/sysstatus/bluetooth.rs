//! Bluetooth: whether the radio is on, which devices are paired and which of them are connected (WinRT `Radio`, and
//! `DeviceInformation` over the paired and the connected devices' selectors). Reading only: the radio is switched through
//! `super::radio` (the user's press on the tile's button), and a device is not connected or disconnected from here.
//!
//! RUNS only while the panel is open ([`subscribe`] starts a thread of its own, MTA; dropping the subscription ends it): the radio's
//! `StateChanged` and a `DeviceWatcher` on the connected devices (classic and low energy) post a message, the thread reads again.
//! The watcher also announces every device that is already connected when it starts; those announcements are one burst and are
//! read once.
//!
//! A machine without a Bluetooth radio (no adapter, or the driver is off) gives `available: false` and the tile is left out.

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use windows::Devices::Bluetooth::{BluetoothConnectionStatus, BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::{DeviceInformation, DeviceWatcher};
use windows::Devices::Radios::Radio;
use windows::Foundation::TypedEventHandler;

use super::{Subscription, init_mta};

pub use super::radio::Power;

/// One paired device.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Device {
    pub name: String,
    pub connected: bool,
    /// Its address (twelve hex digits), when Windows' id for it carries one: what its battery and its sound are found by.
    pub address: Option<String>,
    /// Its battery in percent, when it reports one.
    pub battery: Option<u8>,
    /// It has sound here (a headset, a speaker): its sound can be connected and let go (`btaudio::set_connected`).
    pub audio: bool,
}

/// The devices with what is known of each by its address: the battery it reports, whether it has sound.
///
/// ONE ROW FOR ONE DEVICE. A phone (and many a headset) is listed twice by Windows, once as a classic device and once as a low
/// energy one, with two names and the same address (seen on the development PC: "OzerO iPhone", not connected, and "4", connected,
/// both with the phone's battery). Two entries of one address are one device: connected if either is, under the longer of the two
/// names (the short one is what the low energy side happens to call itself), where the first of them stood.
pub fn with_details(mut devices: Vec<Device>, addresses: &[(String, String)], batteries: &[(String, u8)], audio: &[String]) -> Vec<Device> {
    for d in &mut devices {
        d.address = addresses.iter().find(|(name, _)| *name == d.name).map(|(_, a)| a.clone());
        if let Some(a) = &d.address {
            d.battery = batteries.iter().find(|(b, _)| b == a).map(|(_, level)| *level);
            d.audio = audio.contains(a);
        }
    }
    let mut out: Vec<Device> = Vec::new();
    for d in devices {
        match out.iter_mut().find(|o| o.address.is_some() && o.address == d.address) {
            Some(first) => {
                first.connected |= d.connected;
                if d.name.chars().count() > first.name.chars().count() {
                    first.name = d.name;
                }
            }
            None => out.push(d),
        }
    }
    out
}

/// The list of devices as it is shown: every paired device once, the connected ones first (in the order Windows gave them), the
/// others by name. A device that is connected without being in the list of paired ones is in it all the same.
pub fn devices_of(paired: Vec<String>, connected: &[String]) -> Vec<Device> {
    let mut out: Vec<Device> = connected.iter().map(|n| Device { name: n.clone(), connected: true, ..Default::default() }).collect();
    let mut rest: Vec<String> = paired.into_iter().filter(|n| !connected.contains(n)).collect();
    rest.sort_by_key(|n| n.to_lowercase());
    rest.dedup();
    out.extend(rest.into_iter().map(|name| Device { name, ..Default::default() }));
    out
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BluetoothSnapshot {
    /// There is a Bluetooth radio.
    pub available: bool,
    pub power: Power,
    /// The names of the connected devices (in the order Windows gave them, without repeats).
    pub connected: Vec<String>,
    /// The paired devices, the connected ones first (`devices_of`). Empty while the radio is off.
    pub devices: Vec<Device>,
    /// The adapter's Bluetooth version ("5.4"), when Windows tells its LMP version.
    pub version: Option<&'static str>,
}

/// The adapter's version, kept once it is known (the adapter does not change while it is there; one plugged in later is read when
/// it first answers).
fn adapter_version() -> Option<&'static str> {
    static KNOWN: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    if let Some(v) = KNOWN.get() {
        return Some(v);
    }
    let v = super::btaudio::adapter_lmp_version().and_then(super::btaudio::version_of_lmp)?;
    Some(KNOWN.get_or_init(|| v))
}

enum Msg {
    Read,
    Quit,
}

const BURST: Duration = Duration::from_millis(200);

/// Follows Bluetooth (see the module docs): `callback` is called from the thread of its own, at once with the first reading and at
/// every change after it.
pub fn subscribe(callback: impl Fn(&BluetoothSnapshot) + Send + Sync + 'static) -> Subscription {
    let (tx, rx) = channel::<Msg>();
    let wake = tx.clone();
    let spawned = std::thread::Builder::new().name("rf-bluetooth".into()).spawn(move || {
        init_mta();
        let mut keep = Follow::new(&wake);
        let mut last: Option<BluetoothSnapshot> = None;
        let mut tell = |s: BluetoothSnapshot| {
            if last.as_ref() != Some(&s) {
                callback(&s);
                last = Some(s);
            }
        };
        tell(read());
        while let Ok(msg) = rx.recv() {
            if matches!(msg, Msg::Quit) {
                break;
            }
            loop {
                match rx.recv_timeout(BURST) {
                    Ok(Msg::Quit) => {
                        keep.stop();
                        return;
                    }
                    Ok(Msg::Read) => continue,
                    Err(_) => break,
                }
            }
            // the radio may have appeared or gone since the handlers were put on it
            keep.refresh_radio(&wake);
            tell(read());
        }
        keep.stop();
    });
    if let Err(e) = spawned {
        log::warn!("bluetooth: the thread did not start: {e}");
    }
    Subscription::new(move || {
        let _ = tx.send(Msg::Quit);
    })
}

/// Reads once on the calling thread (a probe). Blocks.
pub fn read_once() -> BluetoothSnapshot {
    init_mta();
    read()
}

fn radio() -> Option<Radio> {
    super::radio::find(super::radio::Kind::Bluetooth)
}

fn read() -> BluetoothSnapshot {
    let Some(radio) = radio() else { return BluetoothSnapshot::default() };
    let power = super::radio::power_of(&radio);
    let version = adapter_version();
    if power != Power::On {
        return BluetoothSnapshot { available: true, power, version, ..Default::default() };
    }
    let (connected, mut addresses) = found(selectors());
    let (paired, more) = found(paired_selectors());
    addresses.extend(more);
    let devices = with_details(devices_of(paired, &connected), &addresses, &super::btaudio::battery_levels(), &super::btaudio::audio_addresses());
    // (the names the tile tells are the rows' own: a device listed twice is told once, by the name its row has)
    let connected = devices.iter().filter(|d| d.connected).map(|d| d.name.clone()).collect();
    BluetoothSnapshot { available: true, power, connected, devices, version }
}

fn paired_selectors() -> Vec<windows::core::HSTRING> {
    let mut v = Vec::new();
    if let Ok(s) = BluetoothDevice::GetDeviceSelectorFromPairingState(true) {
        v.push(s);
    }
    if let Ok(s) = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true) {
        v.push(s);
    }
    v
}

fn selectors() -> Vec<windows::core::HSTRING> {
    let mut v = Vec::new();
    if let Ok(s) = BluetoothDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected) {
        v.push(s);
    }
    if let Ok(s) = BluetoothLEDevice::GetDeviceSelectorFromConnectionStatus(BluetoothConnectionStatus::Connected) {
        v.push(s);
    }
    v
}

/// The devices the selectors find: their names, each once, in the order Windows gave them; and (name, address) for every one
/// whose id carries an address.
fn found(selectors: Vec<windows::core::HSTRING>) -> (Vec<String>, Vec<(String, String)>) {
    let (mut names, mut addresses): (Vec<String>, Vec<(String, String)>) = (Vec::new(), Vec::new());
    for sel in selectors {
        let Some(found) = DeviceInformation::FindAllAsyncAqsFilter(&sel).ok().and_then(|op| op.join().ok()) else { continue };
        for d in found {
            if let Ok(n) = d.Name() {
                let n = n.to_string();
                if n.trim().is_empty() {
                    continue;
                }
                if let Some(a) = d.Id().ok().and_then(|id| super::btaudio::address_of_winrt_id(&id.to_string())) {
                    addresses.push((n.clone(), a));
                }
                if !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    (names, addresses)
}

/// The handlers that make the thread read again, and what is needed to take them off.
struct Follow {
    radio: Option<(Radio, i64)>,
    watchers: Vec<(DeviceWatcher, i64, i64, i64)>,
}

impl Follow {
    fn new(wake: &Sender<Msg>) -> Follow {
        let mut f = Follow { radio: None, watchers: Vec::new() };
        f.refresh_radio(wake);
        for sel in selectors() {
            let Ok(w) = DeviceInformation::CreateWatcherAqsFilter(&sel) else { continue };
            let (a, u, r) = (poke(wake), poke(wake), poke(wake));
            let tokens = (
                w.Added(&TypedEventHandler::new(move |_, _| {
                    a();
                    Ok(())
                })),
                w.Updated(&TypedEventHandler::new(move |_, _| {
                    u();
                    Ok(())
                })),
                w.Removed(&TypedEventHandler::new(move |_, _| {
                    r();
                    Ok(())
                })),
            );
            if let (Ok(a), Ok(u), Ok(r)) = tokens
                && w.Start().is_ok()
            {
                f.watchers.push((w, a, u, r));
            }
        }
        f
    }

    /// Puts the state handler on the radio (again, when there is a different one than before).
    fn refresh_radio(&mut self, wake: &Sender<Msg>) {
        let Some(r) = radio() else {
            self.radio = None;
            return;
        };
        if self.radio.as_ref().is_some_and(|(old, _)| *old == r) {
            return;
        }
        if let Some((old, t)) = self.radio.take() {
            let _ = old.RemoveStateChanged(t);
        }
        let p = poke(wake);
        if let Ok(t) = r.StateChanged(&TypedEventHandler::new(move |_, _| {
            p();
            Ok(())
        })) {
            self.radio = Some((r, t));
        }
    }

    fn stop(&mut self) {
        if let Some((r, t)) = self.radio.take() {
            let _ = r.RemoveStateChanged(t);
        }
        for (w, a, u, r) in self.watchers.drain(..) {
            let _ = w.Stop();
            let _ = w.RemoveAdded(a);
            let _ = w.RemoveUpdated(u);
            let _ = w.RemoveRemoved(r);
        }
    }
}

fn poke(wake: &Sender<Msg>) -> impl Fn() + Send + 'static {
    let tx = wake.clone();
    move || {
        let _ = tx.send(Msg::Read);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_gets_its_battery_and_its_sound_by_its_address() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let list = devices_of(names(&["Kulaklık", "Fare", "Klavye"]), &names(&["Fare"]));
        let addresses = vec![("Fare".to_string(), "F00DCAFE0001".to_string()), ("Kulaklık".to_string(), "94DB56ABCDEF".to_string())];
        let out = with_details(list, &addresses, &[("F00DCAFE0001".to_string(), 49)], &["94DB56ABCDEF".to_string()]);
        let shown: Vec<(&str, Option<u8>, bool, bool)> = out.iter().map(|d| (d.name.as_str(), d.battery, d.audio, d.address.is_some())).collect();
        assert_eq!(shown, [("Fare", Some(49), false, true), ("Klavye", None, false, false), ("Kulaklık", None, true, true)]);
        // a phone that Windows lists twice (classic and low energy, one address): one row, connected, under its longer name
        let twice = devices_of(names(&["OzerO iPhone", "Klavye"]), &names(&["4"]));
        let phone = vec![("4".to_string(), "AABBCCDDEEFF".to_string()), ("OzerO iPhone".to_string(), "AABBCCDDEEFF".to_string())];
        let out = with_details(twice, &phone, &[("AABBCCDDEEFF".to_string(), 49)], &[]);
        let shown: Vec<(&str, bool, Option<u8>)> = out.iter().map(|d| (d.name.as_str(), d.connected, d.battery)).collect();
        assert_eq!(shown, [("OzerO iPhone", true, Some(49)), ("Klavye", false, None)]);
        // two devices without an address are two devices, whatever their names
        let none = with_details(devices_of(names(&["a", "b"]), &[]), &[], &[], &[]);
        assert_eq!(none.len(), 2);
    }

    #[test]
    fn the_paired_devices_are_listed_once_with_the_connected_ones_first() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let list = devices_of(names(&["Klavye", "kulaklık", "Fare", "Hoparlör", "Fare"]), &names(&["Fare", "Kumanda"]));
        let shown: Vec<(&str, bool)> = list.iter().map(|d| (d.name.as_str(), d.connected)).collect();
        assert_eq!(shown, [("Fare", true), ("Kumanda", true), ("Hoparlör", false), ("Klavye", false), ("kulaklık", false)]);
        assert!(devices_of(Vec::new(), &[]).is_empty());
    }

    #[test]
    fn a_default_snapshot_is_no_radio_and_nothing_connected() {
        let s = BluetoothSnapshot::default();
        assert!(!s.available && s.connected.is_empty());
        assert_eq!(s.power, Power::Off);
    }
}
