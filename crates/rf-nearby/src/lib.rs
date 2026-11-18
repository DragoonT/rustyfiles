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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Peer {
    pub id: String,
    pub name: String,
    pub os: String,
    pub trusted: bool,
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
    /// At most about ten times a second per transfer, and once more at the end.
    fn progress(&self, transfer: u64, done: u64, total: u64);
    fn finished(&self, transfer: u64, result: TransferResult);
    /// A pairing with `peer` shows this code on both devices; answer with [`Nearby::confirm_pairing`].
    fn pairing_code(&self, peer: String, code: String);
    fn paired(&self, peer: String, ok: bool);
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
            quinn::Endpoint::server(server, SocketAddr::from(([0, 0, 0, 0], config.port))).map_err(|e| NearbyError::Network(e.to_string()))?
        };
        let inner = Arc::new(Inner {
            trust: Mutex::new(trust::TrustStore::load(&dir)),
            vis: AtomicU8::new(Visibility::Trusted as u8),
            peers: Mutex::new(HashMap::new()),
            offers: Mutex::new(HashMap::new()),
            pairings: Mutex::new(HashMap::new()),
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
        self.inner.vis.store(v as u8, Ordering::Relaxed);
        self.inner.advertise();
    }

    /// The devices seen on the network now, the trusted ones first.
    pub fn peers(&self) -> Vec<Peer> {
        self.inner.peer_list()
    }

    /// The paired devices, seen or not.
    pub fn trusted(&self) -> Vec<Peer> {
        let t = self.inner.trust.lock().unwrap_or_else(|e| e.into_inner());
        t.devices().iter().map(|d| Peer { id: d.id.clone(), name: d.name.clone(), os: String::new(), trusted: true }).collect()
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

    /// [`pair`](Self::pair) with a device at an address (`host:port`).
    pub fn pair_with_address(&self, address: String, peer_id: String) -> Result<(), NearbyError> {
        let addr: SocketAddr = address.parse().map_err(|_| NearbyError::UnknownPeer(address.clone()))?;
        let inner = self.inner.clone();
        self.spawn(transfer::pair_with(inner, vec![addr], peer_id, String::new()));
        Ok(())
    }

    /// This side's answer to the code shown for a pairing with `peer`.
    pub fn confirm_pairing(&self, peer: String, ok: bool) {
        if let Some(tx) = self.inner.pairings.lock().unwrap_or_else(|e| e.into_inner()).remove(&peer) {
            let _ = tx.send(ok);
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

/// The state the runtime's tasks share.
pub(crate) struct Inner {
    pub(crate) cfg: Config,
    pub(crate) me: identity::Identity,
    pub(crate) listener: Arc<dyn NearbyListener>,
    pub(crate) endpoint: quinn::Endpoint,
    pub(crate) client: quinn::ClientConfig,
    trust: Mutex<trust::TrustStore>,
    vis: AtomicU8,
    peers: Mutex<HashMap<String, discovery::Seen>>,
    offers: Mutex<HashMap<u64, oneshot::Sender<Option<PathBuf>>>>,
    pairings: Mutex<HashMap<String, oneshot::Sender<bool>>>,
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
        if let Some(d) = self.discovery.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
            && let Err(e) = d.advertise(&self.me.id, &self.cfg.name, &self.cfg.os, port, vis)
        {
            log::warn!("nearby: cannot advertise: {e:#}");
        }
    }

    pub(crate) fn trusts(&self, fingerprint: &str) -> bool {
        self.trust.lock().unwrap_or_else(|e| e.into_inner()).trusts(fingerprint)
    }

    pub(crate) fn trust_device(&self, id: &str, name: &str, fingerprint: &str) {
        let name = if name.is_empty() { self.peers.lock().unwrap_or_else(|e| e.into_inner()).get(id).map(|s| s.name.clone()).unwrap_or_else(|| id.to_string()) } else { name.to_string() };
        let device = trust::TrustedDevice { id: id.to_string(), name, fingerprint: fingerprint.to_string() };
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

    pub(crate) fn ask_pairing(&self, peer: &str) -> oneshot::Receiver<bool> {
        let (tx, rx) = oneshot::channel();
        self.pairings.lock().unwrap_or_else(|e| e.into_inner()).insert(peer.to_string(), tx);
        rx
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
            .map(|s| Peer { id: s.id.clone(), name: s.name.clone(), os: s.os.clone(), trusted: trust.is_trusted_id(&s.id) })
            // (a device that shows itself to its trusted devices only is listed by those alone)
            .filter(|p| p.trusted || peers.get(&p.id).is_some_and(|s| s.vis != "trusted"))
            .collect();
        list.sort_by(|a, b| b.trusted.cmp(&a.trusted).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        list
    }

    fn notify_peers(&self) {
        self.listener.peers_changed(self.peer_list());
    }
}
