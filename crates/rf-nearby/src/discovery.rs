//! Finding the other devices: every running rustyfiles advertises itself over mDNS / DNS-SD (the way printers and AirPlay do) as
//! `<device id>._rustyfiles._udp.local.` and browses for the others. No server, no account. The port is shared with the system's own
//! responder where there is one (mDNSResponder on macOS, Windows' own on Windows): the library binds 5353 with address reuse.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, Weak};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::{Inner, LinkKind};

pub const SERVICE: &str = "_rustyfiles._udp.local.";

/// What a peer's advertisement says.
#[derive(Clone, Debug)]
pub struct Seen {
    pub id: String,
    pub name: String,
    pub os: String,
    pub proto: u32,
    /// "trusted" or "everyone".
    pub vis: String,
    pub addrs: Vec<IpAddr>,
    pub port: u16,
    /// Its network link, as its app said (`link=wifi:866`).
    pub link_kind: LinkKind,
    pub link_mbps: u32,
}

pub struct Discovery {
    daemon: ServiceDaemon,
    registered: Mutex<Option<String>>,
}

impl Discovery {
    /// Starts browsing; what is found goes to `inner` (`peer_seen` / `peer_gone`) from a thread of its own.
    pub fn start(inner: Weak<Inner>) -> anyhow::Result<Discovery> {
        let daemon = ServiceDaemon::new()?;
        let rx = daemon.browse(SERVICE)?;
        listen(rx, inner)?;
        Ok(Discovery { daemon, registered: Mutex::new(None) })
    }

    /// Browses anew: the network is asked again at once (the daemon otherwise asks again only now and then). The thread of the
    /// earlier browse ends with it.
    pub fn browse_again(&self, inner: Weak<Inner>) -> anyhow::Result<()> {
        let _ = self.daemon.stop_browse(SERVICE);
        let rx = self.daemon.browse(SERVICE)?;
        listen(rx, inner)
    }

    /// Advertises this device (or stops, with `vis` None). An earlier advertisement is updated in place: unregistering it first sent
    /// a goodbye that the other devices act on a second later (RFC 6762's one-second grace), after the new announcement had arrived,
    /// and they dropped this device from their lists at every refresh and every change of visibility (seen on Windows and Mac).
    pub fn advertise(&self, id: &str, name: &str, os: &str, port: u16, vis: Option<&str>, link: Option<&str>) -> anyhow::Result<()> {
        let mut reg = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        let Some(vis) = vis else {
            if let Some(full) = reg.take() {
                let _ = self.daemon.unregister(&full);
            }
            return Ok(());
        };
        let mut props = HashMap::new();
        props.insert("id".to_string(), id.to_string());
        props.insert("name".to_string(), name.to_string());
        props.insert("os".to_string(), os.to_string());
        props.insert("proto".to_string(), crate::proto::PROTOCOL.to_string());
        props.insert("vis".to_string(), vis.to_string());
        if let Some(link) = link {
            props.insert("link".to_string(), link.to_string());
        }
        let info = ServiceInfo::new(SERVICE, id, &format!("{id}.local."), "", port, props)?.enable_addr_auto();
        let full = info.get_fullname().to_string();
        self.daemon.register(info)?;
        *reg = Some(full);
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.advertise("", "", "", 0, None, None);
        let _ = self.daemon.shutdown();
    }
}

fn listen(rx: mdns_sd::Receiver<ServiceEvent>, inner: Weak<Inner>) -> anyhow::Result<()> {
    std::thread::Builder::new().name("rf-nearby-mdns".into()).spawn(move || {
        while let Ok(event) = rx.recv() {
            let Some(inner) = inner.upgrade() else { break };
            match event {
                ServiceEvent::ServiceResolved(info) => {
                    if let Some(seen) = parse(&info) {
                        inner.peer_seen(seen);
                    }
                }
                ServiceEvent::ServiceRemoved(_, fullname) => {
                    let id = fullname.split('.').next().unwrap_or_default().to_string();
                    inner.peer_gone(&id);
                }
                ServiceEvent::SearchStopped(_) => break,
                _ => {}
            }
        }
    })?;
    Ok(())
}

/// `wifi:866` -> (Wi-Fi, 866 Mbit/s); anything odd is unknown.
pub fn parse_link(s: &str) -> (LinkKind, u32) {
    let (kind, mbps) = s.split_once(':').unwrap_or((s, "0"));
    let kind = match kind {
        "wifi" => LinkKind::Wifi,
        "ethernet" => LinkKind::Ethernet,
        "other" => LinkKind::Other,
        _ => return (LinkKind::Unknown, 0),
    };
    (kind, mbps.parse().unwrap_or(0))
}

/// The TXT value for a link; none when it is not known.
pub fn link_text(kind: LinkKind, mbps: u32) -> Option<String> {
    let k = match kind {
        LinkKind::Wifi => "wifi",
        LinkKind::Ethernet => "ethernet",
        LinkKind::Other => "other",
        LinkKind::Unknown => return None,
    };
    (mbps > 0).then(|| format!("{k}:{mbps}"))
}

fn parse(info: &ServiceInfo) -> Option<Seen> {
    let get = |k: &str| info.get_property_val_str(k).map(str::to_string);
    let id = get("id")?;
    let link = get("link").map(|l| parse_link(&l)).unwrap_or_default();
    Some(Seen {
        name: get("name").unwrap_or_else(|| id.clone()),
        os: get("os").unwrap_or_default(),
        proto: get("proto").and_then(|p| p.parse().ok()).unwrap_or(0),
        vis: get("vis").unwrap_or_else(|| "everyone".into()),
        link_kind: link.0,
        link_mbps: link.1,
        addrs: info.get_addresses().iter().copied().collect(),
        port: info.get_port(),
        id,
    })
}
