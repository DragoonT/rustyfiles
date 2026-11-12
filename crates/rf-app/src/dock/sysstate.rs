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
    cx.set_global(SysState { audio: audio::snapshot(), network: network::snapshot(), _subs: vec![a, n] });
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

/// The right island's volume symbol: the level in steps, muted, or no sound device (before the first reading, two waves).
pub fn volume_icon(cx: &App) -> L {
    audio(cx).map_or(L::Volume2, |a| cc_model::volume_icon(audio::glyph(&a)))
}
