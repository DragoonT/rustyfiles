//! The two things of the system the right island shows ALL the time, so they run for as long as the dock does: the sound (volume
//! level and mute) and the network (Ethernet, Wi-Fi bars, not connected). Both come from `rf_win::sysstatus`, which calls back from
//! its own threads when Windows says something changed; here that is handed to the app's thread through a channel and kept in a
//! global that the island's symbols and the control centre read. Nothing is polled.
//!
//! Everything else the control centre shows (media, Bluetooth, battery, brightness) runs only while the panel is open and lives in
//! its view.

use futures::StreamExt;
use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_win::sysstatus::Subscription;
use rf_win::sysstatus::audio::{self, AudioSnapshot};
use rf_win::sysstatus::network::{self, NetworkSnapshot};

use super::cc_model;
use super::right::RightIsland;

pub struct SysState {
    pub audio: Option<AudioSnapshot>,
    pub network: Option<NetworkSnapshot>,
    /// The battery as last read (issue 149: the dock had no battery symbol, and Windows' own is not shown while it runs).
    pub battery: rf_win::sysstatus::power::BatterySnapshot,
    _subs: Vec<Subscription>,
}

impl Global for SysState {}

enum Event {
    Audio(AudioSnapshot),
    Network(NetworkSnapshot),
}

/// Starts following the sound and the network. Called once, when the dock's islands exist.
pub fn start(right: Entity<RightIsland>, cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<Event>();
    let to_audio = tx.clone();
    let a = audio::subscribe(move |s| {
        let _ = to_audio.unbounded_send(Event::Audio(s.clone()));
    });
    let n = network::subscribe(move |s| {
        let _ = tx.unbounded_send(Event::Network(s.clone()));
    });
    let battery = rf_win::sysstatus::power::read();
    let laptop = battery.present;
    cx.set_global(SysState { audio: audio::snapshot(), network: network::snapshot(), battery, _subs: vec![a, n] });
    // the battery is read again every 30 s (a cheap call; the change notification has one follower, the control centre's): only on
    // a machine that has one
    if laptop {
        let right = right.clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(30)).await;
                let now = cx.background_executor().spawn(async { rf_win::sysstatus::power::read() }).await;
                let changed = cx.update(|cx| {
                    let before = cx.try_global::<SysState>().map(|s| s.battery);
                    if before != Some(now) {
                        cx.update_global::<SysState, _>(|s, _| s.battery = now);
                        right.update(cx, |_, cx| cx.notify());
                    }
                    before != Some(now)
                });
                let _ = changed;
            }
        })
        .detach();
    }
    cx.spawn(async move |cx| {
        while let Some(ev) = rx.next().await {
            cx.update(|cx| {
                // (updating the global tells whoever observes it: the control centre, while it is open)
                cx.update_global::<SysState, _>(|s, _| match ev {
                    Event::Audio(a) => s.audio = Some(a),
                    Event::Network(n) => s.network = Some(n),
                });
                right.update(cx, |_, cx| cx.notify());
            });
        }
    })
    .detach();
}

pub fn audio(cx: &App) -> Option<AudioSnapshot> {
    cx.try_global::<SysState>().and_then(|s| s.audio.clone())
}

pub fn network(cx: &App) -> Option<NetworkSnapshot> {
    cx.try_global::<SysState>().and_then(|s| s.network.clone())
}

/// The right island's network symbol: what the machine is connected through (before the first reading, the full Wi-Fi symbol).
pub fn network_icon(cx: &App) -> L {
    network(cx).map_or(L::Wifi, |n| cc_model::network_icon(network::glyph(&n)))
}

/// Whether the machine has a battery (the right island shows the battery button only then).
pub fn has_battery(cx: &App) -> bool {
    cx.try_global::<SysState>().is_some_and(|s| s.battery.present)
}

/// The battery button's symbol: the level in steps, charging, low.
pub fn battery_icon(cx: &App) -> L {
    cx.try_global::<SysState>().map_or(L::Battery, |s| cc_model::battery_icon(&s.battery))
}

/// The battery button's tooltip: the level and what it is doing, and the time left when Windows says.
pub fn battery_tip(cx: &App) -> String {
    let Some(b) = cx.try_global::<SysState>().map(|s| s.battery) else { return crate::t!("Pil").to_string() };
    let t = cc_model::battery_text(&b, cc_model::Words::current());
    match b.minutes_left.filter(|_| !b.on_ac && !b.charging) {
        Some(m) => format!("{} · {}", t.subtitle, crate::t!("{h} sa {m} dk kaldı").replace("{h}", &(m / 60).to_string()).replace("{m}", &(m % 60).to_string())),
        None => t.subtitle.to_string(),
    }
}

/// The right island's volume symbol: the level in steps, muted, or no sound device (before the first reading, two waves).
pub fn volume_icon(cx: &App) -> L {
    audio(cx).map_or(L::Volume2, |a| cc_model::volume_icon(audio::glyph(&a)))
}
