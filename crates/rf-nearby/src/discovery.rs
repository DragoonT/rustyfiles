//! Finding the other devices: every running rustyfiles advertises itself over mDNS / DNS-SD (the way printers and AirPlay do) as
//! `<device id>._rustyfiles._udp.local.` and browses for the others. No server, no account. The port is shared with the system's own
//! responder where there is one (mDNSResponder on macOS, Windows' own on Windows): the library binds 5353 with address reuse.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, Weak};

use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};

use crate::Inner;

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
                    _ => {}
                }
            }
        })?;
        Ok(Discovery { daemon, registered: Mutex::new(None) })
    }

    /// Advertises this device (or stops, with `vis` None). Replaces an earlier advertisement.
    pub fn advertise(&self, id: &str, name: &str, os: &str, port: u16, vis: Option<&str>) -> anyhow::Result<()> {
        let mut reg = self.registered.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(full) = reg.take() {
            let _ = self.daemon.unregister(&full);
        }
        let Some(vis) = vis else { return Ok(()) };
        let mut props = HashMap::new();
        props.insert("id".to_string(), id.to_string());
        props.insert("name".to_string(), name.to_string());
        props.insert("os".to_string(), os.to_string());
        props.insert("proto".to_string(), crate::proto::PROTOCOL.to_string());
        props.insert("vis".to_string(), vis.to_string());
        let info = ServiceInfo::new(SERVICE, id, &format!("{id}.local."), "", port, props)?.enable_addr_auto();
        let full = info.get_fullname().to_string();
        self.daemon.register(info)?;
        *reg = Some(full);
        Ok(())
    }

    pub fn shutdown(&self) {
        let _ = self.advertise("", "", "", 0, None);
        let _ = self.daemon.shutdown();
    }
}

fn parse(info: &ServiceInfo) -> Option<Seen> {
    let get = |k: &str| info.get_property_val_str(k).map(str::to_string);
    let id = get("id")?;
    Some(Seen {
        name: get("name").unwrap_or_else(|| id.clone()),
        os: get("os").unwrap_or_default(),
        proto: get("proto").and_then(|p| p.parse().ok()).unwrap_or(0),
        vis: get("vis").unwrap_or_else(|| "everyone".into()),
        addrs: info.get_addresses().iter().copied().collect(),
        port: info.get_port(),
        id,
    })
}
