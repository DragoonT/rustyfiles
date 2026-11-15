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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub connected: bool,
}

/// The list of devices as it is shown: every paired device once, the connected ones first (in the order Windows gave them), the
/// others by name. A device that is connected without being in the list of paired ones is in it all the same.
pub fn devices_of(paired: Vec<String>, connected: &[String]) -> Vec<Device> {
    let mut out: Vec<Device> = connected.iter().map(|n| Device { name: n.clone(), connected: true }).collect();
    let mut rest: Vec<String> = paired.into_iter().filter(|n| !connected.contains(n)).collect();
    rest.sort_by_key(|n| n.to_lowercase());
    rest.dedup();
    out.extend(rest.into_iter().map(|name| Device { name, connected: false }));
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
    if power != Power::On {
        return BluetoothSnapshot { available: true, power, ..Default::default() };
    }
    let connected = names_of(selectors());
    let devices = devices_of(names_of(paired_selectors()), &connected);
    BluetoothSnapshot { available: true, power, connected, devices }
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

/// The names of the devices the selectors find, each once, in the order Windows gave them.
fn names_of(selectors: Vec<windows::core::HSTRING>) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for sel in selectors {
        let Some(found) = DeviceInformation::FindAllAsyncAqsFilter(&sel).ok().and_then(|op| op.join().ok()) else { continue };
        for d in found {
            if let Ok(n) = d.Name() {
                let n = n.to_string();
                if !n.trim().is_empty() && !names.contains(&n) {
                    names.push(n);
                }
            }
        }
    }
    names
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
