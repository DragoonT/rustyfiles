//! The network: Ethernet, Wi-Fi (its name and bars) or not connected. WinRT `NetworkInformation`, which works for a program with no
//! package identity (checked on this build) and needs no location consent for what is read here: the connection profile, its
//! connectivity level, its adapter type and, for Wi-Fi, `GetConnectedSsid` and `GetSignalBars` (no scan: the scanning calls are the
//! ones Windows gates behind the user's location consent, and none is made).
//!
//! EVENTS. `NetworkStatusChanged` says a connection came, went or changed its level. It does NOT say that a Wi-Fi signal got stronger
//! or weaker, so the bars on screen can lag behind the air: [`refresh`] reads again, and the panel calls it when it opens and every
//! 5 s while it is open (never from a timer while it is closed).
//!
//! ONE THREAD (MTA), started by the first [`subscribe`] and kept for the life of the process: the right island shows the network
//! symbol all the time. The WinRT handler only posts a message; the thread reads and publishes.
//!
//! No toggling: switching Wi-Fi off and on is `Radio.SetStateAsync`, which the research for this work did not find solid for a
//! program with no package identity (it was not tried: it would change the user's radios). The tile opens Windows' network page.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::OnceLock;
use std::time::Duration;

use windows::Networking::Connectivity::{ConnectionProfile, NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler};

use super::{Hub, Subscription, init_mta};

/// What the machine is connected through.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Link {
    /// No connection that reaches anything.
    #[default]
    Disconnected,
    Ethernet,
    /// `bars` is 0 to 5 (what Windows' own symbol uses).
    WiFi { name: String, bars: u8 },
    /// A mobile broadband connection.
    Cellular,
    /// Something else that is connected (a VPN adapter, a tunnel).
    Other,
}

/// How far the connection reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Reach {
    #[default]
    None,
    /// The local network only (no internet).
    Local,
    Internet,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct NetworkSnapshot {
    /// The network API answered. False: the source does not exist on this machine (the tile is left out).
    pub available: bool,
    pub link: Link,
    pub reach: Reach,
}

// ------------------------------------------------------------------ pure: the symbol

/// Which symbol the island shows for the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkGlyph {
    /// Not connected (or no network source).
    Off,
    Ethernet,
    /// Wi-Fi with 0..=3 steps of strength (the symbol set has four: none, low, medium, full).
    WiFi(u8),
    /// Connected through something else.
    Other,
}

pub fn glyph(s: &NetworkSnapshot) -> NetworkGlyph {
    if !s.available {
        return NetworkGlyph::Off;
    }
    match &s.link {
        Link::Disconnected => NetworkGlyph::Off,
        Link::Ethernet => NetworkGlyph::Ethernet,
        Link::WiFi { bars, .. } => NetworkGlyph::WiFi(wifi_steps(*bars)),
        Link::Cellular | Link::Other => NetworkGlyph::Other,
    }
}

/// Windows' 0..=5 bars as the 0..=3 steps of the symbols (0 stays empty; 1-2 low, 3-4 medium, 5 full).
pub fn wifi_steps(bars: u8) -> u8 {
    match bars {
        0 => 0,
        1 | 2 => 1,
        3 | 4 => 2,
        _ => 3,
    }
}

/// `IanaInterfaceType` of an adapter, to the kind of link it is: 6 Ethernet, 71 Wi-Fi, 243 / 244 mobile broadband; the rest (tunnels,
/// virtual adapters, 131 / 53) is "something else".
pub fn link_of_iana(iana: u32) -> &'static str {
    match iana {
        6 => "ethernet",
        71 => "wifi",
        243 | 244 => "cellular",
        _ => "other",
    }
}

// ------------------------------------------------------------------ the thread

enum Msg {
    /// The system said something changed, or somebody asked to read again.
    Read,
}

struct Engine {
    hub: Hub<NetworkSnapshot>,
    tx: Sender<Msg>,
}

static ENGINE: OnceLock<Engine> = OnceLock::new();

fn engine() -> &'static Engine {
    ENGINE.get_or_init(|| {
        let hub = Hub::new();
        let (tx, rx) = channel::<Msg>();
        let (h, t) = (hub.clone(), tx.clone());
        if let Err(e) = std::thread::Builder::new().name("rf-network".into()).spawn(move || run(rx, t, h)) {
            log::warn!("network: the thread did not start: {e}");
            hub.publish(NetworkSnapshot::default());
        }
        Engine { hub, tx }
    })
}

/// Follows the network state (see the module docs). The callback runs on the network thread.
pub fn subscribe(callback: impl Fn(&NetworkSnapshot) + Send + Sync + 'static) -> Subscription {
    engine().hub.subscribe(callback)
}

pub fn snapshot() -> Option<NetworkSnapshot> {
    ENGINE.get().and_then(|e| e.hub.latest())
}

/// Reads the network again (a Wi-Fi signal changes without an event). Returns at once.
pub fn refresh() {
    if let Some(e) = ENGINE.get() {
        let _ = e.tx.send(Msg::Read);
    }
}

/// Reads the state once on the calling thread (a probe). Blocks a moment.
pub fn read_once() -> NetworkSnapshot {
    init_mta();
    read()
}

/// How long the thread waits for a burst of events to end before it reads.
const BURST: Duration = Duration::from_millis(200);

fn run(rx: Receiver<Msg>, tx: Sender<Msg>, hub: Hub<NetworkSnapshot>) {
    init_mta();
    let token = {
        let tx = tx.clone();
        let handler = NetworkStatusChangedEventHandler::new(move |_| {
            let _ = tx.send(Msg::Read);
            Ok(())
        });
        NetworkInformation::NetworkStatusChanged(&handler).ok()
    };
    if token.is_none() {
        log::debug!("network: no change events; the state is read when the panel asks");
    }
    hub.publish(read());
    while rx.recv().is_ok() {
        // a connection coming up is several events in a row
        while rx.recv_timeout(BURST).is_ok() {}
        hub.publish(read());
    }
    if let Some(t) = token {
        let _ = NetworkInformation::RemoveNetworkStatusChanged(t);
    }
}

fn read() -> NetworkSnapshot {
    // A machine with no profile at all still answers (an error here is "not connected", not "no such source"): the source is
    // missing only when the class itself cannot be activated, which `GetInternetConnectionProfile` reports the same way, so the
    // list of profiles is asked as well.
    let internet = NetworkInformation::GetInternetConnectionProfile().ok();
    let known = NetworkInformation::GetConnectionProfiles().map(|v| v.Size().unwrap_or(0)).ok();
    if internet.is_none() && known.is_none() {
        return NetworkSnapshot { available: false, ..Default::default() };
    }
    // no internet profile: a profile that only reaches the local network may still be there
    let profile = internet.or_else(|| {
        NetworkInformation::GetConnectionProfiles().ok().and_then(|v| v.into_iter().find(|p| level_of(p) != NetworkConnectivityLevel::None))
    });
    let Some(profile) = profile else { return NetworkSnapshot { available: true, link: Link::Disconnected, reach: Reach::None } };
    let level = level_of(&profile);
    if level == NetworkConnectivityLevel::None {
        return NetworkSnapshot { available: true, link: Link::Disconnected, reach: Reach::None };
    }
    let reach = if level == NetworkConnectivityLevel::LocalAccess { Reach::Local } else { Reach::Internet };
    NetworkSnapshot { available: true, link: link_of(&profile), reach }
}

fn level_of(p: &ConnectionProfile) -> NetworkConnectivityLevel {
    p.GetNetworkConnectivityLevel().unwrap_or(NetworkConnectivityLevel::None)
}

fn link_of(p: &ConnectionProfile) -> Link {
    if p.IsWlanConnectionProfile().unwrap_or(false) {
        let name = p
            .WlanConnectionProfileDetails()
            .and_then(|d| d.GetConnectedSsid())
            .map(|s| s.to_string())
            .ok()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| p.ProfileName().ok().map(|s| s.to_string()))
            .unwrap_or_default();
        let bars = p.GetSignalBars().ok().and_then(|r| r.Value().ok()).unwrap_or(0).min(5);
        return Link::WiFi { name, bars };
    }
    if p.IsWwanConnectionProfile().unwrap_or(false) {
        return Link::Cellular;
    }
    let iana = p.NetworkAdapter().and_then(|a| a.IanaInterfaceType()).unwrap_or(0);
    match link_of_iana(iana) {
        "ethernet" => Link::Ethernet,
        "wifi" => Link::WiFi { name: p.ProfileName().map(|s| s.to_string()).unwrap_or_default(), bars: 0 },
        "cellular" => Link::Cellular,
        _ => Link::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(link: Link) -> NetworkSnapshot {
        NetworkSnapshot { available: true, link, reach: Reach::Internet }
    }

    #[test]
    fn the_symbol_follows_the_link() {
        assert_eq!(glyph(&NetworkSnapshot::default()), NetworkGlyph::Off, "no source");
        assert_eq!(glyph(&snap(Link::Disconnected)), NetworkGlyph::Off);
        assert_eq!(glyph(&snap(Link::Ethernet)), NetworkGlyph::Ethernet);
        assert_eq!(glyph(&snap(Link::WiFi { name: "x".into(), bars: 5 })), NetworkGlyph::WiFi(3));
        assert_eq!(glyph(&snap(Link::WiFi { name: "x".into(), bars: 0 })), NetworkGlyph::WiFi(0));
        assert_eq!(glyph(&snap(Link::Cellular)), NetworkGlyph::Other);
        assert_eq!(glyph(&snap(Link::Other)), NetworkGlyph::Other);
    }

    #[test]
    fn windows_five_bars_become_the_symbols_steps() {
        let steps: Vec<u8> = (0..=5).map(wifi_steps).collect();
        assert_eq!(steps, vec![0, 1, 1, 2, 2, 3]);
        assert_eq!(wifi_steps(200), 3, "a value out of range is full, not a panic");
    }

    #[test]
    fn adapter_types_are_told_apart() {
        assert_eq!(link_of_iana(6), "ethernet");
        assert_eq!(link_of_iana(71), "wifi");
        assert_eq!(link_of_iana(243), "cellular");
        assert_eq!(link_of_iana(131), "other");
        assert_eq!(link_of_iana(53), "other");
        assert_eq!(link_of_iana(0), "other");
    }
}
