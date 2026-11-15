//! Wi-Fi: the radio, the networks in range (which of them is the connected one, which ones Windows has saved), and connecting.
//!
//! SOURCES. The radio is WinRT `Radio` (`super::radio`). The networks are the Native Wifi API (`wlanapi`), the list Windows' own
//! flyout shows: name, signal, secured or not, saved or not, connected or not (`WlanGetAvailableNetworkList`), a scan
//! (`WlanScan`), connect and disconnect. Both work for a program with no package identity and without elevation.
//!
//! LOCATION. Since Windows 11 24H2 the names of the networks in range are location data: with location off (Settings > Privacy &
//! security > Location, or its "Let desktop apps access your location") the list is refused with "access denied". The snapshot
//! says so ([`Listing::Denied`]) and the panel shows the way to that page; nothing is switched on the user's behalf.
//!
//! RUNS only while the control centre is open ([`subscribe`] starts a thread of its own, MTA; dropping the [`Wifi`] it returns ends
//! it). Windows tells when a scan ended, when a connection came or went and when the radio changed (WLAN notifications, and the
//! radio's `StateChanged`); the handlers only post a message and the thread reads again. A scan is asked for when the thread
//! starts and whenever the panel asks ([`Wifi::scan`]: its list was opened); Windows scans by itself too while the radio is on.
//!
//! WRITES, each the user's click and nothing else: the radio (`radio::set`), [`connect`], [`disconnect`].
//!
//! CONNECTING. A network Windows has saved is connected with its saved profile. An open network is connected as it is. A network
//! that wants a password Windows does not have ([`Network::needs_password`]) is connected with the password the user types in the
//! panel ([`connect_with_password`]) when it is a home network of the two kinds in use today (WPA2 and WPA3 "personal": [`Auth`]):
//! a profile is made for it as Windows' own flyout makes one (saved, connecting by itself from then on), Windows is asked to
//! connect, and when it has not connected after a while (the password was wrong) the profile is taken away again, so that no
//! network with a wrong password stays saved. Any other kind (a company's network with a sign-in, the old WEP and WPA) goes to
//! Windows' own Wi-Fi page.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use windows::Devices::Radios::Radio;
use windows::Foundation::TypedEventHandler;
use windows::Win32::Foundation::{ERROR_ACCESS_DENIED, HANDLE};
use windows::Win32::NetworkManagement::WiFi::{
    DOT11_AUTH_ALGO_RSNA_PSK, DOT11_AUTH_ALGO_WPA3_SAE, DOT11_SSID, WLAN_PROFILE_USER, WlanDeleteProfile, WlanSetProfile, L2_NOTIFICATION_DATA, WLAN_AVAILABLE_NETWORK, WLAN_AVAILABLE_NETWORK_CONNECTED, WLAN_AVAILABLE_NETWORK_HAS_PROFILE, WLAN_AVAILABLE_NETWORK_LIST, WLAN_CONNECTION_PARAMETERS,
    WLAN_INTERFACE_INFO, WLAN_INTERFACE_INFO_LIST, WLAN_NOTIFICATION_SOURCE_ACM, WLAN_NOTIFICATION_SOURCE_MSM, WLAN_NOTIFICATION_SOURCE_NONE, WlanCloseHandle, WlanConnect, WlanDisconnect,
    WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList, WlanOpenHandle, WlanRegisterNotification, WlanScan, dot11_BSS_type_infrastructure, wlan_connection_mode_discovery_unsecure,
    wlan_connection_mode_profile, wlan_interface_state_connected,
};
use windows::core::{GUID, PCWSTR};

use super::init_mta;
use super::radio::{self, Kind, Power};

/// How a secured network lets a device in, as far as connecting from here goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Auth {
    /// WPA2 with a password ("personal").
    Wpa2,
    /// WPA3 with a password ("personal").
    Wpa3,
    /// Anything else: none, a sign-in (a company's network), the old kinds. Not connected with a typed password from here.
    #[default]
    Other,
}

/// One network in range.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Network {
    pub ssid: String,
    /// 0 to 5, as Windows' own symbol counts.
    pub bars: u8,
    pub secured: bool,
    /// The name of the profile Windows has saved for it, when it has one.
    pub profile: Option<String>,
    pub connected: bool,
    pub auth: Auth,
}

impl Network {
    /// Windows has it saved: it connects without asking anything.
    pub fn known(&self) -> bool {
        self.profile.is_some()
    }

    /// It wants a password that Windows does not have.
    pub fn needs_password(&self) -> bool {
        self.secured && self.profile.is_none()
    }

    /// The password can be typed in the panel: a home network of today's two kinds (see the module docs).
    pub fn takes_typed_password(&self) -> bool {
        self.needs_password() && self.auth != Auth::Other
    }
}

/// Whether Windows gave the list of networks.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Listing {
    #[default]
    Ok,
    /// Refused: location is off for desktop programs (see the module docs).
    Denied,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct WifiSnapshot {
    /// The machine has a Wi-Fi radio.
    pub available: bool,
    pub power: Power,
    /// The networks in range: the connected one first, then the saved ones, then by signal.
    pub networks: Vec<Network>,
    pub listing: Listing,
}

impl WifiSnapshot {
    pub fn connected(&self) -> Option<&Network> {
        self.networks.iter().find(|n| n.connected)
    }
}

// ------------------------------------------------------------------ pure: bars, names, the list

/// Windows' signal quality (0 to 100) as the 0 to 5 bars of its symbol.
pub fn bars_of_quality(quality: u32) -> u8 {
    quality.min(100).div_ceil(20) as u8
}

/// A network's name from the bytes it sends (UTF-8 by convention; whatever is not is shown as it can be).
pub fn ssid_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).trim_matches('\0').to_string()
}

/// The list as it is shown, from the entries Windows gives. Windows lists a network once for every profile it has for it and once
/// more without one, and a network of several access points once: here it is ONE row per name, with the best signal seen, saved if
/// any entry is, connected if any entry is. A network that does not tell its name is left out (it cannot be chosen by name).
pub fn merged(raw: Vec<Network>) -> Vec<Network> {
    let mut out: Vec<Network> = Vec::new();
    for n in raw {
        if n.ssid.trim().is_empty() {
            continue;
        }
        match out.iter_mut().find(|o| o.ssid == n.ssid) {
            Some(o) => {
                o.bars = o.bars.max(n.bars);
                o.secured |= n.secured;
                o.connected |= n.connected;
                if o.profile.is_none() {
                    o.profile = n.profile;
                }
                if o.auth == Auth::Other {
                    o.auth = n.auth;
                }
            }
            None => out.push(n),
        }
    }
    out.sort_by(|a, b| {
        (b.connected, b.known(), b.bars).cmp(&(a.connected, a.known(), a.bars)).then_with(|| a.ssid.to_lowercase().cmp(&b.ssid.to_lowercase()))
    });
    out
}

// ------------------------------------------------------------------ the Native Wifi handle

struct Wlan(HANDLE);

impl Drop for Wlan {
    fn drop(&mut self) {
        unsafe {
            WlanCloseHandle(self.0, None);
        }
    }
}

fn wide_text(w: &[u16]) -> String {
    String::from_utf16_lossy(&w[..w.iter().position(|c| *c == 0).unwrap_or(w.len())])
}

impl Wlan {
    fn open() -> Option<Wlan> {
        let (mut version, mut handle) = (0u32, HANDLE::default());
        (unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) } == 0).then_some(Wlan(handle))
    }

    /// The Wi-Fi interface to work with: the connected one, else the first.
    fn interface(&self) -> Option<GUID> {
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if unsafe { WlanEnumInterfaces(self.0, None, &mut list) } != 0 || list.is_null() {
            return None;
        }
        // SAFETY: Windows wrote `dwNumberOfItems` entries after the header and the block is ours until it is freed below
        let found = unsafe {
            let items: &[WLAN_INTERFACE_INFO] = std::slice::from_raw_parts((*list).InterfaceInfo.as_ptr(), (*list).dwNumberOfItems as usize);
            items.iter().find(|i| i.isState == wlan_interface_state_connected).or(items.first()).map(|i| i.InterfaceGuid)
        };
        unsafe { WlanFreeMemory(list as *const c_void) };
        found
    }

    /// The networks in range as Windows lists them; the error code when it does not.
    fn networks(&self, interface: &GUID) -> Result<Vec<Network>, u32> {
        let mut list: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let code = unsafe { WlanGetAvailableNetworkList(self.0, interface, 0, None, &mut list) };
        if code != 0 || list.is_null() {
            return Err(code);
        }
        // SAFETY: as above
        let out = unsafe {
            let items: &[WLAN_AVAILABLE_NETWORK] = std::slice::from_raw_parts((*list).Network.as_ptr(), (*list).dwNumberOfItems as usize);
            items
                .iter()
                .map(|n| {
                    let profile = wide_text(&n.strProfileName);
                    Network {
                        ssid: ssid_text(&n.dot11Ssid.ucSSID[..(n.dot11Ssid.uSSIDLength as usize).min(32)]),
                        bars: bars_of_quality(n.wlanSignalQuality),
                        secured: n.bSecurityEnabled.as_bool(),
                        profile: (n.dwFlags & WLAN_AVAILABLE_NETWORK_HAS_PROFILE != 0 && !profile.is_empty()).then_some(profile),
                        connected: n.dwFlags & WLAN_AVAILABLE_NETWORK_CONNECTED != 0,
                        auth: if n.dot11DefaultAuthAlgorithm == DOT11_AUTH_ALGO_RSNA_PSK {
                            Auth::Wpa2
                        } else if n.dot11DefaultAuthAlgorithm == DOT11_AUTH_ALGO_WPA3_SAE {
                            Auth::Wpa3
                        } else {
                            Auth::Other
                        },
                    }
                })
                .collect()
        };
        unsafe { WlanFreeMemory(list as *const c_void) };
        Ok(out)
    }

    fn scan(&self, interface: &GUID) {
        unsafe {
            WlanScan(self.0, interface, None, None, None);
        }
    }
}

/// Reads everything once: the radio, and the list when the radio is on.
fn read(wlan: Option<&Wlan>) -> WifiSnapshot {
    let power = radio::find(Kind::WiFi).map(|r| radio::power_of(&r));
    let interface = wlan.and_then(|w| w.interface().map(|g| (w, g)));
    if power.is_none() && interface.is_none() {
        return WifiSnapshot::default();
    }
    let power = power.unwrap_or(Power::On);
    let mut snap = WifiSnapshot { available: true, power, ..Default::default() };
    if power == Power::On
        && let Some((wlan, guid)) = interface
    {
        match wlan.networks(&guid) {
            Ok(raw) => snap.networks = merged(raw),
            Err(code) if code == ERROR_ACCESS_DENIED.0 => snap.listing = Listing::Denied,
            // (the radio is going down, the interface is busy: an empty list until Windows says something again)
            Err(code) => log::debug!("wifi: no list of networks ({code})"),
        }
    }
    snap
}

/// Reads once on the calling thread (a probe). Blocks.
pub fn read_once() -> WifiSnapshot {
    init_mta();
    read(Wlan::open().as_ref())
}

// ------------------------------------------------------------------ following

enum Msg {
    Read,
    Scan,
    Quit,
}

const BURST: Duration = Duration::from_millis(250);

/// The follower of [`subscribe`]. Dropping it ends its thread.
pub struct Wifi {
    tx: Sender<Msg>,
}

impl Wifi {
    /// Asks Windows to look for networks now (the list was opened); the list follows when the scan has ended.
    pub fn scan(&self) {
        let _ = self.tx.send(Msg::Scan);
    }
}

impl Drop for Wifi {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
    }
}

/// What Windows' notification is given to find its way back: the sender, behind a lock because Windows may call from any thread.
struct Wake(Mutex<Sender<Msg>>);

unsafe extern "system" fn on_wlan(_data: *mut L2_NOTIFICATION_DATA, context: *mut c_void) {
    if context.is_null() {
        return;
    }
    // SAFETY: the context is the `Wake` the registration was made with, which lives until the registration is taken off
    let wake = unsafe { &*(context as *const Wake) };
    if let Ok(tx) = wake.0.lock() {
        let _ = tx.send(Msg::Read);
    }
}

/// Follows Wi-Fi (see the module docs): `callback` is called from the thread of its own, at once with the first reading and at every
/// change after it.
pub fn subscribe(callback: impl Fn(&WifiSnapshot) + Send + Sync + 'static) -> Wifi {
    let (tx, rx) = channel::<Msg>();
    let wake_tx = tx.clone();
    let spawned = std::thread::Builder::new().name("rf-wifi".into()).spawn(move || {
        init_mta();
        let wlan = Wlan::open();
        // Windows' own notifications: a scan ended, a connection came or went, the radio changed
        let wake = Box::new(Wake(Mutex::new(wake_tx.clone())));
        if let Some(w) = &wlan {
            unsafe {
                WlanRegisterNotification(w.0, WLAN_NOTIFICATION_SOURCE_ACM | WLAN_NOTIFICATION_SOURCE_MSM, true, Some(on_wlan), Some(&*wake as *const Wake as *const c_void), None, None);
            }
        }
        // the radio's own word (it is the switch Windows shows)
        let radio: Option<(Radio, i64)> = radio::find(Kind::WiFi).and_then(|r| {
            let tx = wake_tx.clone();
            let token = r.StateChanged(&TypedEventHandler::new(move |_, _| {
                let _ = tx.send(Msg::Read);
                Ok(())
            }));
            token.ok().map(|t| (r, t))
        });
        let scan = |wlan: &Option<Wlan>| {
            if let Some(w) = wlan
                && let Some(g) = w.interface()
            {
                w.scan(&g);
            }
        };
        let mut last: Option<WifiSnapshot> = None;
        let mut tell = |s: WifiSnapshot| {
            if last.as_ref() != Some(&s) {
                callback(&s);
                last = Some(s);
            }
        };
        tell(read(wlan.as_ref()));
        scan(&wlan);
        'follow: while let Ok(msg) = rx.recv() {
            let mut want_scan = matches!(msg, Msg::Scan);
            if matches!(msg, Msg::Quit) {
                break;
            }
            loop {
                match rx.recv_timeout(BURST) {
                    Ok(Msg::Quit) => break 'follow,
                    Ok(Msg::Scan) => want_scan = true,
                    Ok(Msg::Read) => continue,
                    Err(_) => break,
                }
            }
            if want_scan {
                scan(&wlan);
            }
            tell(read(wlan.as_ref()));
        }
        if let Some((r, t)) = radio {
            let _ = r.RemoveStateChanged(t);
        }
        // (the registration comes off before what its callback points to goes: Windows waits for a running callback here)
        if let Some(w) = &wlan {
            unsafe {
                WlanRegisterNotification(w.0, WLAN_NOTIFICATION_SOURCE_NONE, true, None, None, None, None);
            }
        }
        drop(wlan);
        drop(wake);
    });
    if let Err(e) = spawned {
        log::warn!("wifi: the thread did not start: {e}");
    }
    Wifi { tx }
}

// ------------------------------------------------------------------ the user's clicks

/// How a network is connected: with the profile Windows has saved for it, or as an open network. `None`: not from here (it wants a
/// password Windows does not have).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Way {
    Profile(String),
    Open,
}

pub fn way_to(n: &Network) -> Option<Way> {
    match (&n.profile, n.secured) {
        (Some(p), _) => Some(Way::Profile(p.clone())),
        (None, false) => Some(Way::Open),
        (None, true) => None,
    }
}

fn connect_blocking(ssid: &str) -> Result<(), String> {
    let wlan = Wlan::open().ok_or("the Wi-Fi service did not answer")?;
    let guid = wlan.interface().ok_or("no Wi-Fi interface")?;
    let list = merged(wlan.networks(&guid).map_err(|c| format!("no list of networks ({c})"))?);
    let network = list.iter().find(|n| n.ssid == ssid).ok_or("the network is not in range any more")?;
    let code = match way_to(network).ok_or("the network wants a password that is not saved")? {
        Way::Profile(name) => {
            let w = crate::wide::wide(&name);
            let p = WLAN_CONNECTION_PARAMETERS {
                wlanConnectionMode: wlan_connection_mode_profile,
                strProfile: PCWSTR(w.as_ptr()),
                pDot11Ssid: std::ptr::null_mut(),
                pDesiredBssidList: std::ptr::null_mut(),
                dot11BssType: dot11_BSS_type_infrastructure,
                dwFlags: 0,
            };
            unsafe { WlanConnect(wlan.0, &guid, &p, None) }
        }
        Way::Open => {
            let bytes = ssid.as_bytes();
            let mut name = DOT11_SSID { uSSIDLength: bytes.len().min(32) as u32, ucSSID: [0; 32] };
            name.ucSSID[..bytes.len().min(32)].copy_from_slice(&bytes[..bytes.len().min(32)]);
            let p = WLAN_CONNECTION_PARAMETERS {
                wlanConnectionMode: wlan_connection_mode_discovery_unsecure,
                strProfile: PCWSTR::null(),
                pDot11Ssid: &mut name,
                pDesiredBssidList: std::ptr::null_mut(),
                dot11BssType: dot11_BSS_type_infrastructure,
                dwFlags: 0,
            };
            unsafe { WlanConnect(wlan.0, &guid, &p, None) }
        }
    };
    if code == 0 { Ok(()) } else { Err(format!("Windows refused the connection ({code})")) }
}

// ------------------------------------------------------------------ a password the user types

/// A password a home network can have: 8 to 63 characters.
pub fn valid_password(password: &str) -> bool {
    (8..=63).contains(&password.chars().count())
}

fn xml_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The profile Windows keeps for a home network: its name, how it lets a device in, the password. As Windows' own flyout writes
/// it for a network the user ticked "connect automatically" on. `None` for a kind of network this does not connect.
pub fn profile_xml(ssid: &str, auth: Auth, password: &str) -> Option<String> {
    let authentication = match auth {
        Auth::Wpa2 => "WPA2PSK",
        Auth::Wpa3 => "WPA3SAE",
        Auth::Other => return None,
    };
    let hex: String = ssid.bytes().map(|b| format!("{b:02X}")).collect();
    let (name, key) = (xml_text(ssid), xml_text(password));
    Some(format!(
        "<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\"><name>{name}</name><SSIDConfig><SSID><hex>{hex}</hex><name>{name}</name></SSID></SSIDConfig>\
<connectionType>ESS</connectionType><connectionMode>auto</connectionMode><MSM><security><authEncryption><authentication>{authentication}</authentication><encryption>AES</encryption>\
<useOneX>false</useOneX></authEncryption><sharedKey><keyType>passPhrase</keyType><protected>false</protected><keyMaterial>{key}</keyMaterial></sharedKey></security></MSM></WLANProfile>"
    ))
}

/// How long Windows is given to connect with a typed password before its profile is taken away again.
const JOIN_WAIT: Duration = Duration::from_secs(15);

fn connect_with_password_blocking(ssid: &str, password: &str) -> Result<(), String> {
    let wlan = Wlan::open().ok_or("the Wi-Fi service did not answer")?;
    let guid = wlan.interface().ok_or("no Wi-Fi interface")?;
    let list = merged(wlan.networks(&guid).map_err(|c| format!("no list of networks ({c})"))?);
    let network = list.iter().find(|n| n.ssid == ssid).ok_or("the network is not in range any more")?;
    let xml = profile_xml(ssid, network.auth, password).ok_or("this kind of network is not connected from here")?;
    let (xml_w, name_w) = (crate::wide::wide(&xml), crate::wide::wide(ssid));
    let set = |flags: u32| {
        let mut reason = 0u32;
        (unsafe { WlanSetProfile(wlan.0, &guid, flags, PCWSTR(xml_w.as_ptr()), PCWSTR::null(), true, None, &mut reason) }, reason)
    };
    // (for every user of the PC, as Windows' own flyout saves a network; for this user alone where that is not allowed)
    let (mut code, mut reason) = set(0);
    if code == ERROR_ACCESS_DENIED.0 {
        (code, reason) = set(WLAN_PROFILE_USER);
    }
    if code != 0 {
        return Err(format!("Windows did not take the network's profile ({code}, reason {reason})"));
    }
    let forget = || unsafe {
        WlanDeleteProfile(wlan.0, &guid, PCWSTR(name_w.as_ptr()), None);
    };
    let p = WLAN_CONNECTION_PARAMETERS {
        wlanConnectionMode: wlan_connection_mode_profile,
        strProfile: PCWSTR(name_w.as_ptr()),
        pDot11Ssid: std::ptr::null_mut(),
        pDesiredBssidList: std::ptr::null_mut(),
        dot11BssType: dot11_BSS_type_infrastructure,
        dwFlags: 0,
    };
    let code = unsafe { WlanConnect(wlan.0, &guid, &p, None) };
    if code != 0 {
        forget();
        return Err(format!("Windows refused the connection ({code})"));
    }
    // connected within the wait: the network stays saved. Not connected (the password was wrong, the network went): it does not.
    let until = std::time::Instant::now() + JOIN_WAIT;
    while std::time::Instant::now() < until {
        std::thread::sleep(Duration::from_millis(700));
        if wlan.networks(&guid).is_ok_and(|raw| merged(raw).iter().any(|n| n.ssid == ssid && n.connected)) {
            return Ok(());
        }
    }
    forget();
    Err("not connected with that password; the network was not saved".to_string())
}

/// The user typed a network's password in the panel: see the module docs. Returns at once; the list that follows tells whether it
/// is connected.
pub fn connect_with_password(ssid: &str, password: &str) {
    let (ssid, password) = (ssid.to_string(), password.to_string());
    on_a_thread("connect with a password", move || connect_with_password_blocking(&ssid, &password));
}

fn on_a_thread(what: &'static str, work: impl FnOnce() -> Result<(), String> + Send + 'static) {
    let spawned = std::thread::Builder::new().name("rf-wifi-do".into()).spawn(move || {
        if let Err(e) = work() {
            log::warn!("wifi: {what}: {e}");
        }
    });
    if let Err(e) = spawned {
        log::warn!("wifi: the thread did not start: {e}");
    }
}

/// The user chose a network of the list: asks Windows to connect to it (see the module docs for which ones). Returns at once; the
/// list that follows tells whether it is connected.
pub fn connect(ssid: &str) {
    let ssid = ssid.to_string();
    on_a_thread("connect", move || connect_blocking(&ssid));
}

/// The user chose the connected network: asks Windows to leave it.
pub fn disconnect() {
    on_a_thread("disconnect", || {
        let wlan = Wlan::open().ok_or("the Wi-Fi service did not answer")?;
        let guid = wlan.interface().ok_or("no Wi-Fi interface")?;
        let code = unsafe { WlanDisconnect(wlan.0, &guid, None) };
        if code == 0 { Ok(()) } else { Err(format!("Windows refused ({code})")) }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn net(ssid: &str, bars: u8, secured: bool, profile: Option<&str>, connected: bool) -> Network {
        Network { ssid: ssid.into(), bars, secured, profile: profile.map(str::to_string), connected, auth: if secured { Auth::Wpa2 } else { Auth::Other } }
    }

    #[test]
    fn a_home_networks_profile_is_written_as_windows_writes_one() {
        let xml = profile_xml("Ev Ağı", Auth::Wpa2, "gizli-şifre-1").unwrap();
        assert!(xml.starts_with("<?xml version=\"1.0\"?><WLANProfile xmlns=\"http://www.microsoft.com/networking/WLAN/profile/v1\">"));
        assert!(xml.contains("<name>Ev Ağı</name><SSIDConfig><SSID><hex>45762041C49FC4B1</hex><name>Ev Ağı</name></SSID></SSIDConfig>"), "{xml}");
        assert!(xml.contains("<connectionType>ESS</connectionType><connectionMode>auto</connectionMode>"));
        assert!(xml.contains("<authentication>WPA2PSK</authentication><encryption>AES</encryption><useOneX>false</useOneX>"));
        assert!(xml.contains("<keyType>passPhrase</keyType><protected>false</protected><keyMaterial>gizli-şifre-1</keyMaterial>"));
        assert!(xml.ends_with("</WLANProfile>") && !xml.contains('\n'));
        assert!(profile_xml("Ev", Auth::Wpa3, "12345678").unwrap().contains("<authentication>WPA3SAE</authentication>"));
        assert_eq!(profile_xml("İş", Auth::Other, "12345678"), None);
        // what XML reads as its own is written as text
        let odd = profile_xml("A&B <ev>", Auth::Wpa2, "p<a>&\"s's").unwrap();
        assert!(odd.contains("<name>A&amp;B &lt;ev&gt;</name>") && odd.contains("<keyMaterial>p&lt;a&gt;&amp;&quot;s&apos;s</keyMaterial>"), "{odd}");
        assert!(odd.contains("<hex>41264220 3C65763E".replace(' ', "").as_str()));
        // a password of a home network is 8 to 63 characters
        assert!(!valid_password("1234567") && valid_password("12345678") && valid_password(&"a".repeat(63)) && !valid_password(&"a".repeat(64)));
        assert!(valid_password("şifreşif"), "counted in characters, not bytes");
        // which networks take a typed password
        let home = Network { auth: Auth::Wpa3, ..net("Komşu", 5, true, None, false) };
        let work = Network { auth: Auth::Other, ..net("İş", 5, true, None, false) };
        assert!(home.takes_typed_password() && !work.takes_typed_password());
        assert!(!net("Ev", 4, true, Some("Ev"), false).takes_typed_password(), "a saved one needs none");
        assert!(!net("Kafe", 2, false, None, false).takes_typed_password());
    }

    #[test]
    fn the_quality_is_counted_in_windows_bars() {
        assert_eq!(bars_of_quality(0), 0);
        assert_eq!(bars_of_quality(1), 1);
        assert_eq!(bars_of_quality(20), 1);
        assert_eq!(bars_of_quality(21), 2);
        assert_eq!(bars_of_quality(80), 4);
        assert_eq!(bars_of_quality(81), 5);
        assert_eq!(bars_of_quality(100), 5);
        assert_eq!(bars_of_quality(4000), 5, "a quality out of range is full, not more");
    }

    #[test]
    fn a_name_is_read_from_its_bytes() {
        assert_eq!(ssid_text(b"Ev"), "Ev");
        assert_eq!(ssid_text("Çatı Katı".as_bytes()), "Çatı Katı");
        assert_eq!(ssid_text(b""), "");
        assert_eq!(ssid_text(&[0, 0, 0]), "", "a hidden network that sends zeros has no name");
        assert_eq!(ssid_text(&[b'a', 0xFF, b'b']), "a\u{FFFD}b");
    }

    #[test]
    fn one_row_for_a_name_with_the_connected_and_the_saved_ones_first() {
        let list = merged(vec![
            net("Komşu", 5, true, None, false),
            net("Ev", 3, true, None, false),
            net("Ev", 4, true, Some("Ev"), true),
            net("", 5, true, None, false),
            net("Kafe", 2, false, None, false),
            net("İş", 1, true, Some("İş 2"), false),
            net("Ev", 2, true, Some("Ev eski"), false),
            net("avlu", 2, false, None, false),
        ]);
        let names: Vec<&str> = list.iter().map(|n| n.ssid.as_str()).collect();
        assert_eq!(names, ["Ev", "İş", "Komşu", "avlu", "Kafe"]);
        assert_eq!(list[0], net("Ev", 4, true, Some("Ev"), true), "the best signal, the first profile, connected");
        // the kind of a network is taken from the entry that tells it
        let kinds = merged(vec![Network { auth: Auth::Other, ..net("Ev", 3, true, None, false) }, Network { auth: Auth::Wpa3, ..net("Ev", 2, true, None, false) }]);
        assert_eq!(kinds[0].auth, Auth::Wpa3);
        assert!(merged(Vec::new()).is_empty());
        // the snapshot's connected network is the list's
        let snap = WifiSnapshot { available: true, power: Power::On, networks: list, listing: Listing::Ok };
        assert_eq!(snap.connected().map(|n| n.ssid.as_str()), Some("Ev"));
        assert_eq!(WifiSnapshot::default().connected(), None);
    }

    #[test]
    fn a_network_is_connected_by_its_saved_profile_or_as_an_open_one_and_else_not_from_here() {
        let saved = net("Ev", 4, true, Some("Ev profili"), false);
        let open = net("Kafe", 2, false, None, false);
        let locked = net("Komşu", 5, true, None, false);
        assert_eq!(way_to(&saved), Some(Way::Profile("Ev profili".into())));
        assert_eq!(way_to(&open), Some(Way::Open));
        assert_eq!(way_to(&locked), None);
        assert!(saved.known() && !saved.needs_password());
        assert!(!open.known() && !open.needs_password());
        assert!(locked.needs_password());
        // an open network that is saved goes by its profile
        assert_eq!(way_to(&net("Otel", 3, false, Some("Otel"), false)), Some(Way::Profile("Otel".into())));
    }

    fn say(s: &WifiSnapshot) {
        // (how many, never their names)
        println!("available {} power {:?} listing {:?} networks {} saved {} connected {}", s.available, s.power, s.listing, s.networks.len(), s.networks.iter().filter(|n| n.known()).count(), s.connected().is_some());
    }

    /// On the PC it is run on, by hand: what Windows gives.
    #[test]
    #[ignore = "reads the Wi-Fi state of the PC it runs on; run by hand"]
    fn what_this_pc_says() {
        say(&read_once());
    }

    /// On a PC whose Wi-Fi is off and unused, by hand: the radio goes on for a few seconds, the follower tells what it hears (the
    /// first reading, then the list when Windows' scan has ended), and the radio goes off again.
    #[test]
    #[ignore = "switches the Wi-Fi radio of the PC it runs on for a few seconds; run by hand"]
    fn the_follower_tells_the_networks_while_the_radio_is_on_for_a_moment() {
        if radio::read_once(Kind::WiFi) != Some(Power::Off) {
            return;
        }
        radio::set_blocking(Kind::WiFi, true).expect("the radio on");
        let (tx, rx) = channel();
        let wifi = subscribe(move |s| {
            let _ = tx.send(s.clone());
        });
        let end = std::time::Instant::now() + Duration::from_secs(9);
        let mut last = None;
        while let Ok(s) = rx.recv_timeout(end.saturating_duration_since(std::time::Instant::now())) {
            say(&s);
            last = Some(s);
        }
        drop(wifi);
        radio::set_blocking(Kind::WiFi, false).expect("the radio off again");
        let s = last.expect("the follower told something");
        assert!(s.available && s.power == Power::On);
        assert_eq!(radio::read_once(Kind::WiFi), Some(Power::Off));
    }
}
