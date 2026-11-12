//! Bluetooth: whether the radio is on and which devices are connected (WinRT `Radio`, and `DeviceInformation` over the connected
//! devices' selectors). Read-only: switching the radio is `Radio.SetStateAsync`, which was not found solid for a program with no
//! package identity, so the tile opens Windows' Bluetooth page.
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
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Foundation::TypedEventHandler;

use super::{Subscription, init_mta};

/// Where the radio is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Power {
    #[default]
    Off,
    On,
    /// Turned off by a hardware switch or a policy: it cannot be turned on from here.
    Disabled,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BluetoothSnapshot {
    /// There is a Bluetooth radio.
    pub available: bool,
    pub power: Power,
    /// The names of the connected devices (in the order Windows gave them, without repeats).
    pub connected: Vec<String>,
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
    let all = Radio::GetRadiosAsync().ok()?.join().ok()?;
    all.into_iter().find(|r| r.Kind().ok() == Some(RadioKind::Bluetooth))
}

fn read() -> BluetoothSnapshot {
    let Some(radio) = radio() else { return BluetoothSnapshot::default() };
    let power = match radio.State().unwrap_or(RadioState::Unknown) {
        RadioState::On => Power::On,
        RadioState::Disabled => Power::Disabled,
        _ => Power::Off,
    };
    BluetoothSnapshot { available: true, power, connected: if power == Power::On { connected_names() } else { Vec::new() } }
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

fn connected_names() -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for sel in selectors() {
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
    fn a_default_snapshot_is_no_radio_and_nothing_connected() {
        let s = BluetoothSnapshot::default();
        assert!(!s.available && s.connected.is_empty());
        assert_eq!(s.power, Power::Off);
    }
}
