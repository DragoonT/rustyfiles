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
//! THE ROUTE (the user, 2026-11-15, of the network tile's detail: "burası daha da gelişebilir. tıklanabilir modem arayüzü, dns
//! (modemden yani issden gelen mi yoska bilindik cloudflare google filan mı) yazabilir."): for the adapter the connection is on,
//! Windows' own table of adapters tells the gateway (the modem, whose own pages are at that address), the name servers and the
//! speed of the link (`GetAdaptersAddresses`). Whose a name server is is worked out here ([`dns_owner`]): the modem itself (then
//! the names are asked of the modem, which by default asks the provider's servers), one of the public ones people set by hand, an
//! address of the home network that is not the modem (a filter on a small computer), or an address this does not know.
//!
//! Reading only. Wi-Fi's radio, its list of networks and connecting are `super::radio` and `super::wifi`; this module is what the
//! machine is connected through right now, for the island's symbol and the panel's network tile.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::OnceLock;
use std::time::Duration;

use windows::Networking::Connectivity::{ConnectionProfile, NetworkConnectivityLevel, NetworkInformation, NetworkStatusChangedEventHandler};
use windows::Networking::HostNameType;
use windows::Win32::NetworkManagement::IpHelper::{GAA_FLAG_INCLUDE_GATEWAYS, GAA_FLAG_SKIP_ANYCAST, GAA_FLAG_SKIP_MULTICAST, GetAdaptersAddresses, IP_ADAPTER_ADDRESSES_LH};
use windows::Win32::Networking::WinSock::{AF_INET, AF_INET6, SOCKADDR_IN, SOCKADDR_IN6, SOCKET_ADDRESS};

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
    /// The machine's IPv4 address on that connection, when Windows tells one.
    pub address: Option<String>,
    /// The gateway of that connection (the modem): its IPv4 address when it has one, else its IPv6 one.
    pub gateway: Option<String>,
    /// The name servers the connection asks, in Windows' order, each once.
    pub dns: Vec<String>,
    /// The speed of the link in megabits a second, when the adapter tells one.
    pub speed_mbps: Option<u64>,
}

// ------------------------------------------------------------------ pure: whose a name server is, the modem's pages

/// Whose a name server is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DnsOwner {
    /// The modem itself (the connection's gateway, or an address only this link has): the names are asked of the modem.
    Router,
    /// One of the public servers people set by hand.
    Known(&'static str),
    /// An address of the home network that is not the modem.
    Local,
    /// An address this does not know (a provider's own server set by hand, a company's).
    Other,
}

/// The public name servers people set by hand, by their operators.
pub fn known_resolver(ip: &std::net::IpAddr) -> Option<&'static str> {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => match v4.octets() {
            [1, 1, 1, 1..=3] | [1, 0, 0, 1..=3] => Some("Cloudflare"),
            [8, 8, 8, 8] | [8, 8, 4, 4] => Some("Google"),
            [9, 9, 9, 9..=11] | [149, 112, 112, 9..=11] | [149, 112, 112, 112] => Some("Quad9"),
            [208, 67, 222, 222 | 123] | [208, 67, 220, 220 | 123] => Some("OpenDNS"),
            [94, 140, 14, 14..=15] | [94, 140, 15, 15..=16] => Some("AdGuard"),
            [77, 88, 8, 1 | 8 | 2 | 88 | 3 | 7] => Some("Yandex"),
            [45, 90, 28 | 30, _] => Some("NextDNS"),
            [76, 76, 2 | 10, _] => Some("Control D"),
            [195, 175, 39, 39 | 40 | 49 | 50] => Some("Türk Telekom"),
            _ => None,
        },
        IpAddr::V6(v6) => match v6.segments() {
            [0x2606, 0x4700, 0x4700, 0, 0, 0, 0, 0x1111 | 0x1001 | 0x1112 | 0x1002 | 0x1113 | 0x1003] => Some("Cloudflare"),
            [0x2001, 0x4860, 0x4860, 0, 0, 0, 0, 0x8888 | 0x8844] => Some("Google"),
            [0x2620, 0xfe, 0, 0, 0, 0, 0, 0xfe | 0x9 | 0x10 | 0x11] => Some("Quad9"),
            [0x2620, 0x119, 0x35 | 0x53, 0, 0, 0, 0, 0x35 | 0x53] => Some("OpenDNS"),
            [0x2a10, 0x50c0, 0, 0, 0, 0, 0xad1 | 0xad2, 0xff] => Some("AdGuard"),
            _ => None,
        },
    }
}

/// Whose the name server `server` is, on a connection whose gateway is `gateway`.
pub fn dns_owner(server: &str, gateway: Option<&str>) -> DnsOwner {
    use std::net::IpAddr;
    let Ok(ip) = server.trim().parse::<IpAddr>() else { return DnsOwner::Other };
    if gateway.and_then(|g| g.trim().parse::<IpAddr>().ok()) == Some(ip) {
        return DnsOwner::Router;
    }
    if let Some(name) = known_resolver(&ip) {
        return DnsOwner::Known(name);
    }
    match ip {
        // (an address only this link has is the modem's own, whatever its number)
        IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80 => DnsOwner::Router,
        IpAddr::V6(v6) if (v6.segments()[0] & 0xfe00) == 0xfc00 => DnsOwner::Local,
        IpAddr::V4(v4) if v4.is_private() || v4.is_link_local() => DnsOwner::Local,
        _ => DnsOwner::Other,
    }
}

/// The address of the modem's own pages, for a gateway that has an IPv4 address: "http://192.168.1.1".
pub fn router_page(gateway: &str) -> Option<String> {
    gateway.trim().parse::<std::net::Ipv4Addr>().ok().map(|ip| format!("http://{ip}"))
}

/// An adapter's GUID as Windows' table of adapters names it ("{4D36E972-...}"), for telling which row is the connection's.
pub fn adapter_name(id: &windows::core::GUID) -> String {
    format!("{{{id:?}}}").to_ascii_uppercase()
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
    let Some(profile) = profile else { return NetworkSnapshot { available: true, ..Default::default() } };
    let level = level_of(&profile);
    if level == NetworkConnectivityLevel::None {
        return NetworkSnapshot { available: true, ..Default::default() };
    }
    let reach = if level == NetworkConnectivityLevel::LocalAccess { Reach::Local } else { Reach::Internet };
    let route = profile.NetworkAdapter().and_then(|a| a.NetworkAdapterId()).map(|id| route_of(&adapter_name(&id))).unwrap_or_default();
    NetworkSnapshot { available: true, link: link_of(&profile), reach, address: address_of(&profile), gateway: route.0, dns: route.1, speed_mbps: route.2 }
}

/// An address of Windows' table of adapters as text.
unsafe fn ip_of(a: &SOCKET_ADDRESS) -> Option<std::net::IpAddr> {
    unsafe {
        let sa = a.lpSockaddr.as_ref()?;
        if sa.sa_family == AF_INET && a.iSockaddrLength as usize >= size_of::<SOCKADDR_IN>() {
            let v4 = &*(a.lpSockaddr as *const SOCKADDR_IN);
            return Some(std::net::IpAddr::V4(std::net::Ipv4Addr::from(u32::from_be(v4.sin_addr.S_un.S_addr))));
        }
        if sa.sa_family == AF_INET6 && a.iSockaddrLength as usize >= size_of::<SOCKADDR_IN6>() {
            let v6 = &*(a.lpSockaddr as *const SOCKADDR_IN6);
            return Some(std::net::IpAddr::V6(std::net::Ipv6Addr::from(v6.sin6_addr.u.Byte)));
        }
        None
    }
}

/// The gateway, the name servers and the speed of the adapter named `adapter` (see `adapter_name`); nothing for one that is not in
/// Windows' table.
fn route_of(adapter: &str) -> (Option<String>, Vec<String>, Option<u64>) {
    let flags = GAA_FLAG_INCLUDE_GATEWAYS | GAA_FLAG_SKIP_ANYCAST | GAA_FLAG_SKIP_MULTICAST;
    // (the table's size is asked for, then the table; it can grow in between: a few tries)
    let mut size = 16 * 1024u32;
    let mut table: Vec<u64> = Vec::new();
    for _ in 0..4 {
        table = vec![0u64; (size as usize).div_ceil(8)];
        let code = unsafe { GetAdaptersAddresses(0, flags, None, Some(table.as_mut_ptr() as *mut IP_ADAPTER_ADDRESSES_LH), &mut size) };
        match code {
            0 => break,
            // ERROR_BUFFER_OVERFLOW: `size` now says how much it wants
            111 => continue,
            _ => return (None, Vec::new(), None),
        }
    }
    let mut out = (None, Vec::new(), None);
    // SAFETY: Windows filled `table` with a linked list that lies inside it; it is read while `table` lives
    unsafe {
        let mut at = table.as_ptr() as *const IP_ADAPTER_ADDRESSES_LH;
        while let Some(a) = at.as_ref() {
            at = a.Next;
            if !a.AdapterName.to_string().is_ok_and(|n| n.eq_ignore_ascii_case(adapter)) {
                continue;
            }
            let mut gateways = Vec::new();
            let mut g = a.FirstGatewayAddress;
            while let Some(gw) = g.as_ref() {
                gateways.extend(ip_of(&gw.Address));
                g = gw.Next;
            }
            // (the IPv4 one is the address of the modem's pages; an IPv6 one is told only when there is no other)
            out.0 = gateways.iter().find(|ip| ip.is_ipv4()).or(gateways.first()).map(|ip| ip.to_string());
            let mut d = a.FirstDnsServerAddress;
            while let Some(dns) = d.as_ref() {
                if let Some(ip) = ip_of(&dns.Address).map(|ip| ip.to_string())
                    && !out.1.contains(&ip)
                {
                    out.1.push(ip);
                }
                d = dns.Next;
            }
            // (an adapter that does not know its speed says the largest number there is)
            out.2 = (a.TransmitLinkSpeed != u64::MAX && a.TransmitLinkSpeed >= 1_000_000).then(|| a.TransmitLinkSpeed / 1_000_000);
            break;
        }
    }
    out
}

/// The IPv4 address the machine has on the profile's adapter.
fn address_of(p: &ConnectionProfile) -> Option<String> {
    let adapter = p.NetworkAdapter().ok()?.NetworkAdapterId().ok()?;
    NetworkInformation::GetHostNames().ok()?.into_iter().find_map(|h| {
        if h.Type().ok()? != HostNameType::Ipv4 {
            return None;
        }
        let on = h.IPInformation().ok()?.NetworkAdapter().ok()?.NetworkAdapterId().ok()?;
        if on != adapter {
            return None;
        }
        h.CanonicalName().ok().map(|s| s.to_string())
    })
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
        NetworkSnapshot { available: true, link, reach: Reach::Internet, ..Default::default() }
    }

    #[test]
    fn a_name_server_is_the_modems_a_known_one_a_home_one_or_unknown() {
        let gw = Some("192.168.1.1");
        assert_eq!(dns_owner("192.168.1.1", gw), DnsOwner::Router);
        assert_eq!(dns_owner(" 192.168.1.1 ", gw), DnsOwner::Router);
        assert_eq!(dns_owner("1.1.1.1", gw), DnsOwner::Known("Cloudflare"));
        assert_eq!(dns_owner("1.0.0.1", gw), DnsOwner::Known("Cloudflare"));
        assert_eq!(dns_owner("8.8.8.8", gw), DnsOwner::Known("Google"));
        assert_eq!(dns_owner("8.8.4.4", None), DnsOwner::Known("Google"));
        assert_eq!(dns_owner("9.9.9.9", gw), DnsOwner::Known("Quad9"));
        assert_eq!(dns_owner("149.112.112.112", gw), DnsOwner::Known("Quad9"));
        assert_eq!(dns_owner("208.67.222.222", gw), DnsOwner::Known("OpenDNS"));
        assert_eq!(dns_owner("94.140.14.14", gw), DnsOwner::Known("AdGuard"));
        assert_eq!(dns_owner("45.90.28.77", gw), DnsOwner::Known("NextDNS"));
        assert_eq!(dns_owner("195.175.39.39", gw), DnsOwner::Known("Türk Telekom"));
        assert_eq!(dns_owner("2606:4700:4700::1111", gw), DnsOwner::Known("Cloudflare"));
        assert_eq!(dns_owner("2001:4860:4860::8844", gw), DnsOwner::Known("Google"));
        assert_eq!(dns_owner("2620:fe::fe", gw), DnsOwner::Known("Quad9"));
        // a home address that is not the modem (a filter on a small computer); an address only the link has is the modem's
        assert_eq!(dns_owner("192.168.1.53", gw), DnsOwner::Local);
        assert_eq!(dns_owner("10.0.0.2", gw), DnsOwner::Local);
        assert_eq!(dns_owner("fe80::1", gw), DnsOwner::Router);
        assert_eq!(dns_owner("fd00::53", gw), DnsOwner::Local);
        // anything else, and what is no address
        assert_eq!(dns_owner("212.156.4.4", gw), DnsOwner::Other);
        assert_eq!(dns_owner("1.1.1.9", gw), DnsOwner::Other, "next to a known one is not the known one");
        assert_eq!(dns_owner("modem", gw), DnsOwner::Other);
        assert_eq!(dns_owner("", None), DnsOwner::Other);
        // a known server that IS the gateway (a strange network) is the gateway
        assert_eq!(dns_owner("8.8.8.8", Some("8.8.8.8")), DnsOwner::Router);
    }

    #[test]
    fn the_modems_pages_are_at_its_ipv4_address() {
        assert_eq!(router_page("192.168.1.1").as_deref(), Some("http://192.168.1.1"));
        assert_eq!(router_page(" 10.0.0.138 ").as_deref(), Some("http://10.0.0.138"));
        for none in ["fe80::1", "", "192.168.1", "192.168.1.1/admin", "modem.local", "192.168.1.1:8080"] {
            assert_eq!(router_page(none), None, "{none:?}");
        }
        assert_eq!(adapter_name(&windows::core::GUID::from_u128(0x4d36e972_e325_11ce_bfc1_08002be10318)), "{4D36E972-E325-11CE-BFC1-08002BE10318}");
    }

    /// On the PC it is run on, by hand: what Windows tells of the connection's route (whose the name servers are, never their
    /// numbers).
    #[test]
    #[ignore = "reads the network adapters of the PC it runs on; run by hand"]
    fn what_this_pcs_route_says() {
        let s = read_once();
        let owners: Vec<DnsOwner> = s.dns.iter().map(|d| dns_owner(d, s.gateway.as_deref())).collect();
        println!("gateway told {} pages {} name servers {:?} speed {:?}", s.gateway.is_some(), s.gateway.as_deref().and_then(router_page).is_some(), owners, s.speed_mbps);
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
