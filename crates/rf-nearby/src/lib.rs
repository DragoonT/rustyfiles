//! rf-nearby: send files to rustyfiles on the other devices of the same network, AirDrop-like, on every platform rustyfiles runs on
//! (Windows, macOS, and later Linux). Nothing in here is platform-specific: discovery is mDNS / DNS-SD, the transfer is QUIC with
//! TLS 1.3, files are plain `std` / `tokio` file I/O.
//!
//! The app holds one [`Nearby`] (it owns its own runtime) and hears everything through one [`NearbyListener`], called from the
//! crate's own threads (a UI hops to its main thread). The public API is plain data on purpose (strings, numbers, simple enums and
//! records), so a UniFFI wrapper (the macOS app) is a thin layer. The protocol is written down in docs/nearby-protocol.md.
//!
//! Security: every connection is encrypted and both sides prove a certificate; a device's id is derived from its certificate. A
//! device becomes trusted only by pairing (both users confirm the same six-digit code, drawn from the TLS session). An offer from a
//! device that is not trusted is always asked; with the visibility "trusted devices only", such offers are refused outright.

mod discovery;
mod identity;
mod proto;
mod tls;
mod transfer;
mod trust;

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::oneshot;

pub use transfer::PART;

/// A speed estimated from the two devices' links is shown this much of what the links' rates give (15% less).
const LINK_MARGIN: f64 = 0.85;

/// Offers waiting for the user at once, from all devices together (one each).
const MAX_WAITING_OFFERS: usize = 4;

/// What the app tells the crate once, at start.
#[derive(Clone, Debug)]
pub struct Config {
    /// The name other devices show for this one ("Özer's PC").
    pub name: String,
    /// "windows", "macos", "linux": shown by the others with the name.
    pub os: String,
    /// Where the identity and the trusted devices are kept (made when missing).
    pub state_dir: String,
    /// Where a trusted device's files go when they are taken without asking.
    pub inbox: String,
    /// Files from a trusted device are taken without asking (into `inbox`).
    pub auto_accept_trusted: bool,
    /// The UDP port to listen on; 0 lets the system choose.
    pub port: u16,
    /// Advertise and browse with mDNS. Off only for tests and for a build that must not touch the network's multicast.
    pub discovery: bool,
    /// [`Visibility::Everyone`] falls back to [`Visibility::Trusted`] after this many minutes (reported with
    /// [`NearbyListener::visibility_changed`]); 0 keeps it until it is changed.
    pub everyone_minutes: u32,
    /// Listen on 127.0.0.1 only: for tests (nothing from the network can reach it, and Windows' firewall does not ask about it).
    pub local_only: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    /// Not advertised; incoming connections are refused.
    Off,
    /// Advertised as "trusted devices only": the others don't list it, and offers from untrusted devices are refused.
    Trusted,
    /// Advertised to everyone; an untrusted device's offer is asked.
    Everyone,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub os: String,
    pub trusted: bool,
    /// Its network link, as its app said.
    pub link_kind: LinkKind,
    pub link_mbps: u32,
    /// What a transfer with it may reach, bytes a second (0: nothing to go by); `estimate_source` says from what.
    pub estimate_bps: u64,
    pub estimate_source: EstimateSource,
    /// The round trip seen on the last connection with it, ms (0: none yet).
    pub rtt_ms: u32,
}

/// A device's network link, as its app tells the crate ([`Nearby::set_link`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkKind {
    #[default]
    Unknown,
    Wifi,
    Ethernet,
    Other,
}

/// Where a speed estimate comes from, best first: a speed test, the last transfer, the two devices' links.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EstimateSource {
    #[default]
    Unknown,
    Link,
    Transfer,
    Measured,
}

/// A speed test's result ([`Nearby::measure_speed`]), seen from this device: up is from this device to the other one.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SpeedResult {
    pub ok: bool,
    pub up_bps: u64,
    pub down_bps: u64,
    pub rtt_ms: u32,
    pub error: Option<String>,
}

/// An offer of files from another device, waiting for [`Nearby::accept`] or [`Nearby::reject`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub id: u64,
    /// The transfer this offer becomes (its progress and its end are reported with this id).
    pub transfer: u64,
    pub from_id: String,
    pub from_name: String,
    pub from_os: String,
    pub trusted: bool,
    /// The top-level names (files and folders) offered.
    pub names: Vec<String>,
    pub file_count: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferResult {
    pub ok: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    /// On the receiving side: the top-level files and folders that arrived, at their final paths.
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NetStatus {
    Ready,
    /// Discovery could not start (no network, or on macOS the Local Network permission was refused): sending to a known address may
    /// still work, but no device will be listed.
    DiscoveryUnavailable { reason: String },
}

/// Everything the crate tells the app. Called from the crate's own threads.
pub trait NearbyListener: Send + Sync {
    fn peers_changed(&self, peers: Vec<Peer>);
    fn offer(&self, offer: Offer);
    /// A trusted device's files are being taken without asking ([`Config::auto_accept_trusted`]), into the inbox: the same record
    /// as an offer (its `id` is 0, there is nothing to answer); progress and the end follow under `offer.transfer`.
    fn receiving(&self, offer: Offer);
    /// At most about ten times a second per transfer, and once more at the end.
    fn progress(&self, transfer: u64, done: u64, total: u64);
    fn finished(&self, transfer: u64, result: TransferResult);
    /// A pairing with `peer` shows this code on both devices; answer with [`Nearby::confirm_pairing`].
    fn pairing_code(&self, peer: String, code: String);
    fn paired(&self, peer: String, ok: bool);
    /// The visibility changed by itself ([`Config::everyone_minutes`] ran out); not called for [`Nearby::set_visibility`].
    fn visibility_changed(&self, visibility: Visibility);
    /// A speed test with `peer` ended ([`Nearby::measure_speed`]); `peers_changed` follows with the new estimate.
    fn speed_measured(&self, peer: String, result: SpeedResult);
    fn status(&self, status: NetStatus);
}

#[derive(Debug, thiserror::Error)]
pub enum NearbyError {
    #[error("the network could not be used: {0}")]
    Network(String),
    #[error("no device {0} is known")]
    UnknownPeer(String),
    #[error("{0}")]
    Io(String),
}

pub struct Nearby {
    inner: Arc<Inner>,
    rt: Mutex<Option<tokio::runtime::Runtime>>,
}

impl Nearby {
    /// Starts listening (and, with `config.discovery`, advertising at [`Visibility::Trusted`] and browsing).
    pub fn start(config: Config, listener: Arc<dyn NearbyListener>) -> Result<Arc<Nearby>, NearbyError> {
        let net = |e: anyhow::Error| NearbyError::Network(format!("{e:#}"));
        let dir = PathBuf::from(&config.state_dir);
        let me = identity::Identity::load_or_create(&dir).map_err(|e| NearbyError::Io(format!("{e:#}")))?;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).thread_name("rf-nearby").enable_all().build().map_err(|e| NearbyError::Io(e.to_string()))?;
        let server = tls::server_config(&me).map_err(net)?;
        let client = tls::client_config(&me).map_err(net)?;
        let endpoint = {
            let _guard = rt.enter();
            quinn::Endpoint::server(server, SocketAddr::from((if config.local_only { [127, 0, 0, 1] } else { [0, 0, 0, 0] }, config.port))).map_err(|e| NearbyError::Network(e.to_string()))?
        };
        let inner = Arc::new(Inner {
            trust: Mutex::new(trust::TrustStore::load(&dir)),
            vis: AtomicU8::new(Visibility::Trusted as u8),
            vis_epoch: AtomicU64::new(0),
            peers: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            pairings: Mutex::new(HashMap::new()),
            asking: Mutex::new(HashSet::new()),
            link: Mutex::new((LinkKind::Unknown, 0)),
            known: Mutex::new(HashMap::new()),
            measuring: Mutex::new(HashSet::new()),
            speed_tests: Mutex::new(SpeedTests::default()),
            conns: Mutex::new(HashMap::new()),
            cancelled: Mutex::new(HashSet::new()),
            next: AtomicU64::new(1),
            discovery: Mutex::new(None),
            cfg: config,
            me,
            listener,
            endpoint,
            client,
        });
        // incoming connections
        let accept = inner.clone();
        rt.spawn(async move {
            while let Some(incoming) = accept.endpoint.accept().await {
                if accept.visibility() == Visibility::Off {
                    incoming.refuse();
                    continue;
                }
                let inner = accept.clone();
                tokio::spawn(async move {
                    if let Err(e) = transfer::incoming(inner, incoming).await {
                        log::info!("nearby: an incoming connection ended: {e:#}");
                    }
                });
            }
        });
        if inner.cfg.discovery {
            match discovery::Discovery::start(Arc::downgrade(&inner)) {
                Ok(d) => {
                    *inner.discovery.lock().unwrap_or_else(|e| e.into_inner()) = Some(d);
                    inner.advertise();
                    inner.listener.status(NetStatus::Ready);
                }
                Err(e) => inner.listener.status(NetStatus::DiscoveryUnavailable { reason: format!("{e:#}") }),
            }
        } else {
            inner.listener.status(NetStatus::Ready);
        }
        Ok(Arc::new(Nearby { inner, rt: Mutex::new(Some(rt)) }))
    }

    /// This device's id (what the others know it by).
    pub fn id(&self) -> String {
        self.inner.me.id.clone()
    }

    /// The SHA-256 of this device's certificate, for showing ("compare on both devices").
    pub fn fingerprint(&self) -> String {
        self.inner.me.fingerprint.clone()
    }

    /// The UDP port this device listens on.
    pub fn port(&self) -> u16 {
        self.inner.endpoint.local_addr().map(|a| a.port()).unwrap_or(0)
    }

    pub fn visibility(&self) -> Visibility {
        self.inner.visibility()
    }

    pub fn set_visibility(&self, v: Visibility) {
        let epoch = self.inner.vis_epoch.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.vis.store(v as u8, Ordering::Relaxed);
        self.inner.advertise();
        let minutes = self.inner.cfg.everyone_minutes;
        if v == Visibility::Everyone && minutes > 0 {
            let inner = self.inner.clone();
            self.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(u64::from(minutes) * 60)).await;
                // (only when nothing changed it in the meantime)
                if inner.vis_epoch.compare_exchange(epoch, epoch + 1, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
                    inner.vis.store(Visibility::Trusted as u8, Ordering::Relaxed);
                    inner.advertise();
                    inner.listener.visibility_changed(Visibility::Trusted);
                }
            });
        }
    }

    /// Looks for the devices again now and announces this one again: the list starts over and fills as they answer (for a device
    /// that came up and was missed). Discovery otherwise runs all the time; nothing to do without it.
    pub fn refresh(&self) {
        self.inner.peers.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.inner.notify_peers();
        if let Some(d) = self.inner.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && let Err(e) = d.browse_again(Arc::downgrade(&self.inner))
        {
            log::warn!("nearby: cannot look again: {e:#}");
        }
        self.inner.advertise();
    }

    /// The devices seen on the network now, the trusted ones first.
    pub fn peers(&self) -> Vec<Peer> {
        self.inner.peer_list()
    }

    /// The paired devices, seen or not.
    pub fn trusted(&self) -> Vec<Peer> {
        let t = self.inner.trust.lock().unwrap_or_else(|e| e.into_inner());
        t.devices()
            .iter()
            .map(|d| Peer {
                id: d.id.clone(),
                name: d.name.clone(),
                trusted: true,
                estimate_bps: d.speed_bps,
                estimate_source: match (d.speed_bps, d.speed_measured) {
                    (0, _) => EstimateSource::Unknown,
                    (_, true) => EstimateSource::Measured,
                    (_, false) => EstimateSource::Transfer,
                },
                ..Default::default()
            })
            .collect()
    }

    /// Sends files and folders to a device seen on the network. Returns the transfer id at once; the rest comes to the listener.
    pub fn send(&self, peer_id: String, paths: Vec<String>) -> Result<u64, NearbyError> {
        let addrs = self.inner.addrs_of(&peer_id).ok_or_else(|| NearbyError::UnknownPeer(peer_id.clone()))?;
        Ok(self.spawn_send(addrs, peer_id, paths))
    }

    /// [`send`](Self::send) to an address (`host:port`) instead of a discovered device: for a device the network does not list, and
    /// for tests.
    pub fn send_to_address(&self, address: String, paths: Vec<String>) -> Result<u64, NearbyError> {
        let addr: SocketAddr = address.parse().map_err(|_| NearbyError::UnknownPeer(address.clone()))?;
        Ok(self.spawn_send(vec![addr], String::new(), paths))
    }

    fn spawn_send(&self, addrs: Vec<SocketAddr>, peer_id: String, paths: Vec<String>) -> u64 {
        let tid = self.inner.next_id();
        let inner = self.inner.clone();
        let paths: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
        self.spawn(async move {
            let result = transfer::send(inner.clone(), tid, addrs, peer_id, paths).await;
            inner.listener.finished(tid, result);
        });
        tid
    }

    /// Takes an offer, into `dest_dir`.
    pub fn accept(&self, offer_id: u64, dest_dir: String) {
        self.inner.decide(offer_id, Some(PathBuf::from(dest_dir)));
    }

    pub fn reject(&self, offer_id: u64) {
        self.inner.decide(offer_id, None);
    }

    /// Starts pairing with a device seen on the network: both devices then show the same code ([`NearbyListener::pairing_code`]).
    pub fn pair(&self, peer_id: String) -> Result<(), NearbyError> {
        let addrs = self.inner.addrs_of(&peer_id).ok_or_else(|| NearbyError::UnknownPeer(peer_id.clone()))?;
        let name = self.inner.peers.lock().unwrap_or_else(|e| e.into_inner()).get(&peer_id).map(|s| s.name.clone()).unwrap_or_default();
        let inner = self.inner.clone();
        self.spawn(transfer::pair_with(inner, addrs, peer_id, name));
        Ok(())
    }

    /// Tells the crate this device's network link and its rate (Mbit/s): the other devices use it for a first speed estimate before
    /// anything was measured or sent. Call it again when the network or the rate changes: it is announced again only when the kind
    /// changes or the rate moves by more than a fifth (a Wi-Fi rate drifts all the time).
    pub fn set_link(&self, kind: LinkKind, mbps: u32) {
        let moved = {
            let mut l = self.inner.link.lock().unwrap_or_else(|e| e.into_inner());
            let (k0, m0) = *l;
            let moved = k0 != kind || (m0 == 0) != (mbps == 0) || (f64::from(m0) - f64::from(mbps)).abs() > f64::from(m0) * 0.2;
            if moved {
                *l = (kind, mbps);
            }
            moved
        };
        if moved {
            self.inner.advertise();
            self.inner.notify_peers();
        }
    }

    /// Measures the speed to a device seen on the network, both ways (about five seconds; nothing is written, nothing is asked
    /// there). The result comes to [`NearbyListener::speed_measured`]. Returns the test's id.
    pub fn measure_speed(&self, peer_id: String) -> Result<u64, NearbyError> {
        let addrs = self.inner.addrs_of(&peer_id).ok_or_else(|| NearbyError::UnknownPeer(peer_id.clone()))?;
        Ok(self.spawn_speed(addrs, peer_id))
    }

    /// [`measure_speed`](Self::measure_speed) to a device at an address (`host:port`).
    pub fn measure_speed_address(&self, address: String, peer_id: String) -> Result<u64, NearbyError> {
        let addr: SocketAddr = address.parse().map_err(|_| NearbyError::UnknownPeer(address.clone()))?;
        Ok(self.spawn_speed(vec![addr], peer_id))
    }

    fn spawn_speed(&self, addrs: Vec<SocketAddr>, peer_id: String) -> u64 {
        let id = self.inner.next_id();
        let inner = self.inner.clone();
        self.spawn(async move {
            let result = transfer::speed_test(inner.clone(), addrs, peer_id.clone()).await;
            inner.listener.speed_measured(peer_id, result);
            inner.notify_peers();
        });
        id
    }

    /// [`pair`](Self::pair) with a device at an address (`host:port`).
    pub fn pair_with_address(&self, address: String, peer_id: String) -> Result<(), NearbyError> {
        let addr: SocketAddr = address.parse().map_err(|_| NearbyError::UnknownPeer(address.clone()))?;
        let inner = self.inner.clone();
        self.spawn(transfer::pair_with(inner, vec![addr], peer_id, String::new()));
        Ok(())
    }

    /// This side's answer to the code shown for a pairing with `peer`.
    pub fn confirm_pairing(&self, peer: String, ok: bool) {
        if let Some(w) = self.inner.pairings.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer) {
            let _ = w.tx.send(ok);
        }
    }

    /// Removes a paired device: its offers are asked again from now on.
    pub fn forget(&self, peer_id: String) -> Result<(), NearbyError> {
        let mut t = self.inner.trust.lock().unwrap_or_else(|e| e.into_inner());
        t.remove(&peer_id).map_err(|e| NearbyError::Io(e.to_string()))?;
        drop(t);
        self.inner.notify_peers();
        Ok(())
    }

    /// Stops a transfer (either side). Its end is still reported, with `cancelled`.
    pub fn cancel(&self, transfer: u64) {
        self.inner.cancelled.lock().unwrap_or_else(|e| e.into_inner()).insert(transfer);
        if let Some(c) = self.inner.conns.lock().unwrap_or_else(|e| e.into_inner()).get(&transfer) {
            c.close(3u32.into(), b"cancelled");
        }
    }

    /// Stops advertising, listening and every transfer. The handle is of no use afterwards.
    pub fn stop(&self) {
        if let Some(d) = self.inner.discovery.lock().unwrap_or_else(|e| e.into_inner()).take() {
            d.shutdown();
        }
        self.inner.endpoint.close(0u32.into(), b"stopped");
        if let Some(rt) = self.rt.lock().unwrap_or_else(|e| e.into_inner()).take() {
            rt.shutdown_timeout(std::time::Duration::from_secs(2));
        }
    }

    fn spawn(&self, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        if let Some(rt) = self.rt.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            rt.spawn(fut);
        }
    }
}

impl Drop for Nearby {
    fn drop(&mut self) {
        self.stop();
    }
}

/// What was seen with a device in this run.
#[derive(Clone, Copy)]
pub(crate) struct Known {
    bps: u64,
    source: EstimateSource,
    rtt_ms: u32,
}

#[derive(Default)]
pub(crate) struct SpeedTests {
    running: HashSet<String>,
    last: HashMap<String, std::time::Instant>,
}

/// A pairing waiting for this side's user: the answer goes to `tx`.
struct PairingWait {
    tx: oneshot::Sender<bool>,
    /// This side opened the connection.
    outgoing: bool,
    ticket: u64,
}

/// Two devices that both start pairing with each other at the same moment have two pairings, one each way, and each device would
/// ask its user twice, the second asking ending the first (issue 398: the question went at once and "Pair" did nothing). Both keep
/// the one that the device with the smaller id opened: true when that is this connection (`outgoing`: this device, `me`, opened it).
pub(crate) fn crossing_winner(me: &str, peer: &str, outgoing: bool) -> bool {
    if outgoing { me < peer } else { peer < me }
}

/// The state the runtime's tasks share.
pub(crate) struct Inner {
    pub(crate) cfg: Config,
    pub(crate) me: identity::Identity,
    pub(crate) listener: Arc<dyn NearbyListener>,
    pub(crate) endpoint: quinn::Endpoint,
    pub(crate) client: quinn::ClientConfig,
    trust: Mutex<trust::TrustStore>,
    vis: AtomicU8,
    /// Bumped by every visibility change, so a pending fall-back knows when it is out of date.
    vis_epoch: AtomicU64,
    peers: Mutex<HashMap<String, discovery::Seen>>,
    offers: Mutex<HashMap<u64, oneshot::Sender<Option<PathBuf>>>>,
    pairings: Mutex<HashMap<String, PairingWait>>,
    /// The devices with an offer waiting for the user: one at a time each, and a few in all, so a device that floods offers is
    /// turned away instead of filling the screen with questions.
    asking: Mutex<HashSet<String>>,
    /// This device's link ([`Nearby::set_link`]).
    link: Mutex<(LinkKind, u32)>,
    /// What was seen with each device in this run: speed and round trip.
    known: Mutex<HashMap<String, Known>>,
    /// The devices this one is testing the speed to now (one test each).
    measuring: Mutex<HashSet<String>>,
    /// The tests other devices run against this one.
    speed_tests: Mutex<SpeedTests>,
    conns: Mutex<HashMap<u64, quinn::Connection>>,
    cancelled: Mutex<HashSet<u64>>,
    next: AtomicU64,
    discovery: Mutex<Option<discovery::Discovery>>,
}

impl Inner {
    pub(crate) fn next_id(&self) -> u64 {
        self.next.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn visibility(&self) -> Visibility {
        match self.vis.load(Ordering::Relaxed) {
            0 => Visibility::Off,
            2 => Visibility::Everyone,
            _ => Visibility::Trusted,
        }
    }

    fn advertise(&self) {
        let vis = match self.visibility() {
            Visibility::Off => None,
            Visibility::Trusted => Some("trusted"),
            Visibility::Everyone => Some("everyone"),
        };
        let port = self.endpoint.local_addr().map(|a| a.port()).unwrap_or(0);
        let (kind, mbps) = *self.link.lock().unwrap_or_else(|e| e.into_inner());
        let link = discovery::link_text(kind, mbps);
        if let Some(d) = self.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && let Err(e) = d.advertise(&self.me.id, &self.cfg.name, &self.cfg.os, port, vis, link.as_deref())
        {
            log::warn!("nearby: cannot advertise: {e:#}");
        }
    }

    pub(crate) fn trusts(&self, fingerprint: &str) -> bool {
        self.trust.lock().unwrap_or_else(|e| e.into_inner()).trusts(fingerprint)
    }

    pub(crate) fn trust_device(&self, id: &str, name: &str, fingerprint: &str) {
        let name = if name.is_empty() { self.peers.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_string()) } else { name.to_string() };
        let device = trust::TrustedDevice { id: id.to_string(), name, fingerprint: fingerprint.to_string(), ..Default::default() };
        if let Err(e) = self.trust.lock().unwrap_or_else(|e| e.into_inner()).add(device) {
            log::warn!("nearby: cannot keep the trusted device: {e}");
        }
        self.notify_peers();
    }

    /// Registers an offer and tells the listener; the receiver gets the user's answer (a folder, or None for no).
    pub(crate) fn ask(&self, offer: Offer) -> oneshot::Receiver<Option<PathBuf>> {
        let (tx, rx) = oneshot::channel();
        self.offers.lock().unwrap_or_else(|e| e.into_inner()).insert(offer.id, tx);
        self.listener.offer(offer);
        rx
    }

    fn decide(&self, offer_id: u64, dest: Option<PathBuf>) {
        if let Some(tx) = self.offers.lock().unwrap_or_else(|e| e.into_inner()).remove(&offer_id) {
            let _ = tx.send(dest);
        }
    }

    /// Takes a place among the offers waiting for the user, for `device`; false when it has one waiting already or too many wait.
    pub(crate) fn begin_asking(&self, device: &str) -> bool {
        let mut asking = self.asking.lock().unwrap_or_else(|e| e.into_inner());
        if asking.contains(device) || asking.len() >= MAX_WAITING_OFFERS {
            return false;
        }
        asking.insert(device.to_string())
    }

    pub(crate) fn end_asking(&self, device: &str) {
        self.asking.lock().unwrap_or_else(|e| e.into_inner()).remove(device);
    }

    /// Waits for this side's user on a pairing with `peer` over a connection this side opened (`outgoing`) or the peer did. `None`
    /// when another pairing with the same device is waiting already and wins over this one (see [`crossing_winner`]); a waiting one
    /// that loses gets no answer (its receiver ends) and steps back quietly.
    pub(crate) fn ask_pairing(&self, peer: &str, outgoing: bool) -> Option<(u64, oneshot::Receiver<bool>)> {
        let mut map = self.pairings.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(w) = map.get(peer)
            && !w.tx.is_closed()
        {
            // the same way again (Pair pressed again): the newer one; both ways at once: the one both devices pick
            let new_wins = w.outgoing == outgoing || crossing_winner(&self.me.id, peer, outgoing);
            if !new_wins {
                return None;
            }
        }
        let (tx, rx) = oneshot::channel();
        let ticket = self.next_id();
        map.insert(peer.to_string(), PairingWait { tx, outgoing, ticket });
        Some((ticket, rx))
    }

    /// Ends the wait `ticket` with `peer` (when it is still the one waiting).
    pub(crate) fn end_pairing(&self, peer: &str, ticket: u64) {
        let mut map = self.pairings.lock().unwrap_or_else(|e| e.into_inner());
        if map.get(peer).is_some_and(|w| w.ticket == ticket) {
            map.remove(peer);
        }
    }

    pub(crate) fn track(&self, tid: u64, conn: quinn::Connection) {
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).insert(tid, conn);
    }

    pub(crate) fn forget_transfer(&self, tid: u64) {
        self.conns.lock().unwrap_or_else(|e| e.into_inner()).remove(&tid);
    }

    pub(crate) fn was_cancelled(&self, tid: u64) -> bool {
        self.cancelled.lock().unwrap_or_else(|e| e.into_inner()).contains(&tid)
    }

    fn addrs_of(&self, peer_id: &str) -> Option<Vec<SocketAddr>> {
        let peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let s = peers.get(peer_id)?;
        // IPv4 first: an IPv6 link-local address needs its interface, which a plain address can't carry
        let mut addrs: Vec<IpAddr> = s.addrs.clone();
        addrs.sort_by_key(|a| a.is_ipv6());
        Some(addrs.into_iter().map(|a| SocketAddr::new(a, s.port)).collect())
    }

    pub(crate) fn peer_seen(&self, seen: discovery::Seen) {
        if seen.id == self.me.id || seen.proto != proto::PROTOCOL {
            return;
        }
        self.peers.lock().unwrap_or_else(|e| e.into_inner()).insert(seen.id.clone(), seen);
        self.notify_peers();
    }

    pub(crate) fn peer_gone(&self, id: &str) {
        if self.peers.lock().unwrap_or_else(|e| e.into_inner()).remove(id).is_some() {
            self.notify_peers();
        }
    }

    fn peer_list(&self) -> Vec<Peer> {
        let trust = self.trust.lock().unwrap_or_else(|e| e.into_inner());
        let peers = self.peers.lock().unwrap_or_else(|e| e.into_inner());
        let mut list: Vec<Peer> = peers
            .values()
            .map(|s| {
                let (estimate_bps, estimate_source, rtt_ms) = self.estimate(&s.id, (s.link_kind, s.link_mbps), &trust);
                Peer {
                    id: s.id.clone(),
                    name: s.name.clone(),
                    os: s.os.clone(),
                    trusted: trust.is_trusted_id(&s.id),
                    link_kind: s.link_kind,
                    link_mbps: s.link_mbps,
                    estimate_bps,
                    estimate_source,
                    rtt_ms,
                }
            })
            // (a device that shows itself to its trusted devices only is listed by those alone)
            .filter(|p| p.trusted || peers.get(&p.id).is_some_and(|s| s.vis != "trusted"))
            .collect();
        list.sort_by(|a, b| b.trusted.cmp(&a.trusted).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        list
    }

    /// A device's speed estimate: this run's speed test or transfer, else the one kept for a trusted device, else the slower of the
    /// two links at what such a link gives in practice (Wi-Fi about half its rate, Ethernet nine tenths).
    fn estimate(&self, id: &str, link: (LinkKind, u32), trust: &trust::TrustStore) -> (u64, EstimateSource, u32) {
        let known = self.known.lock().unwrap_or_else(|e| e.into_inner()).get(id).copied();
        let rtt = known.map(|k| k.rtt_ms).unwrap_or(0);
        if let Some(k) = known
            && k.bps > 0
        {
            return (k.bps, k.source, rtt);
        }
        if let Some(d) = trust.devices().iter().find(|d| d.id == id)
            && d.speed_bps > 0
        {
            return (d.speed_bps, if d.speed_measured { EstimateSource::Measured } else { EstimateSource::Transfer }, rtt);
        }
        let own = *self.link.lock().unwrap_or_else(|e| e.into_inner());
        let eff = |k: LinkKind| match k {
            LinkKind::Wifi => Some(0.5),
            LinkKind::Ethernet => Some(0.9),
            LinkKind::Other => Some(0.6),
            LinkKind::Unknown => None,
        };
        if let (Some(a), Some(b)) = (eff(own.0), eff(link.0))
            && own.1 > 0
            && link.1 > 0
        {
            // (and 15% under that: the owner compared it with real transfers and found it high, 2026-11-18)
            let mbps = (f64::from(own.1) * a).min(f64::from(link.1) * b) * LINK_MARGIN;
            return ((mbps * 1e6 / 8.0) as u64, EstimateSource::Link, rtt);
        }
        (0, EstimateSource::Unknown, rtt)
    }

    /// A speed seen with a device (a test, or a transfer that moved enough to tell); kept for a trusted device.
    pub(crate) fn note_speed(&self, id: &str, bps: u64, source: EstimateSource, rtt_ms: u32) {
        if bps == 0 {
            return;
        }
        // (one lock at a time: `peer_list` takes the trust store before this map)
        self.known.lock().unwrap_or_else(|e| e.into_inner()).insert(id.to_string(), Known { bps, source, rtt_ms });
        if let Err(e) = self.trust.lock().unwrap_or_else(|e| e.into_inner()).set_speed(id, bps, source == EstimateSource::Measured) {
            log::warn!("nearby: cannot keep a speed: {e}");
        }
        self.notify_peers();
    }

    /// A round trip seen with a device, when nothing else was learnt.
    pub(crate) fn note_rtt(&self, id: &str, rtt_ms: u32) {
        let mut known = self.known.lock().unwrap_or_else(|e| e.into_inner());
        known.entry(id.to_string()).or_insert(Known { bps: 0, source: EstimateSource::Unknown, rtt_ms }).rtt_ms = rtt_ms;
    }

    pub(crate) fn begin_measuring(&self, peer: &str) -> bool {
        self.measuring.lock().unwrap_or_else(|e| e.into_inner()).insert(peer.to_string())
    }

    pub(crate) fn end_measuring(&self, peer: &str) {
        self.measuring.lock().unwrap_or_else(|e| e.into_inner()).remove(peer);
    }

    /// Takes a place for a speed test `peer` runs against this device: one at a time each, one every few seconds each, a few at once.
    pub(crate) fn begin_speed_test(&self, peer: &str) -> bool {
        let mut t = self.speed_tests.lock().unwrap_or_else(|e| e.into_inner());
        if t.running.contains(peer) || t.running.len() >= transfer::SPEED_AT_ONCE || t.last.get(peer).is_some_and(|at| at.elapsed() < transfer::SPEED_SPACING) {
            return false;
        }
        t.last.insert(peer.to_string(), std::time::Instant::now());
        t.running.insert(peer.to_string())
    }

    pub(crate) fn end_speed_test(&self, peer: &str) {
        self.speed_tests.lock().unwrap_or_else(|e| e.into_inner()).running.remove(peer);
    }

    fn notify_peers(&self) {
        self.listener.peers_changed(self.peer_list());
    }
}

#[cfg(test)]
mod crossing_tests {
    use super::crossing_winner;

    /// Both devices must keep the same one of the two pairings: the one the device with the smaller id opened.
    #[test]
    fn both_devices_keep_the_same_pairing() {
        let (a, b) = ("aaa", "bbb");
        // the connection a opened: a sees it as outgoing, b as incoming; both keep it
        assert!(crossing_winner(a, b, true));
        assert!(crossing_winner(b, a, false));
        // the connection b opened: both let it go
        assert!(!crossing_winner(b, a, true));
        assert!(!crossing_winner(a, b, false));
    }
}
