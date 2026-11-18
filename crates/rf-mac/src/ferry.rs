//! Ferry, sending files to rustyfiles on other devices nearby, for the Swift app: rf-nearby's
//! API as UniFFI types. Nothing is decided here; the crate does the work, this only carries
//! its calls and its events across the FFI.

use std::sync::Arc;

use rf_nearby::{Config, EstimateSource, LinkKind, NearbyListener, NetStatus, Offer, Peer, SpeedResult, TransferResult, Visibility};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FerryVisibility {
    /// Not advertised; incoming connections are refused.
    Off,
    /// Seen by trusted devices only; offers from others are refused.
    Trusted,
    /// Seen by everyone (for a while); an untrusted device's offer is asked.
    Everyone,
}

impl From<Visibility> for FerryVisibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Off => FerryVisibility::Off,
            Visibility::Trusted => FerryVisibility::Trusted,
            Visibility::Everyone => FerryVisibility::Everyone,
        }
    }
}

impl From<FerryVisibility> for Visibility {
    fn from(v: FerryVisibility) -> Self {
        match v {
            FerryVisibility::Off => Visibility::Off,
            FerryVisibility::Trusted => Visibility::Trusted,
            FerryVisibility::Everyone => Visibility::Everyone,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FerryConfig {
    pub name: String,
    pub os: String,
    pub state_dir: String,
    pub inbox: String,
    pub auto_accept_trusted: bool,
    pub port: u16,
    pub discovery: bool,
    pub everyone_minutes: u32,
    pub local_only: bool,
}

/// A device's network link, as its app said.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FerryLink {
    Unknown,
    Wifi,
    Ethernet,
    Other,
}

impl From<LinkKind> for FerryLink {
    fn from(k: LinkKind) -> Self {
        match k {
            LinkKind::Unknown => FerryLink::Unknown,
            LinkKind::Wifi => FerryLink::Wifi,
            LinkKind::Ethernet => FerryLink::Ethernet,
            LinkKind::Other => FerryLink::Other,
        }
    }
}

impl From<FerryLink> for LinkKind {
    fn from(k: FerryLink) -> Self {
        match k {
            FerryLink::Unknown => LinkKind::Unknown,
            FerryLink::Wifi => LinkKind::Wifi,
            FerryLink::Ethernet => LinkKind::Ethernet,
            FerryLink::Other => LinkKind::Other,
        }
    }
}

/// What a speed estimate comes from, best first: a speed test, the last transfer, the links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum FerryEstimate {
    Unknown,
    Link,
    Transfer,
    Measured,
}

impl From<EstimateSource> for FerryEstimate {
    fn from(e: EstimateSource) -> Self {
        match e {
            EstimateSource::Unknown => FerryEstimate::Unknown,
            EstimateSource::Link => FerryEstimate::Link,
            EstimateSource::Transfer => FerryEstimate::Transfer,
            EstimateSource::Measured => FerryEstimate::Measured,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FerryPeer {
    pub id: String,
    pub name: String,
    pub os: String,
    pub trusted: bool,
    pub link_kind: FerryLink,
    pub link_mbps: u32,
    pub estimate_bps: u64,
    pub estimate_source: FerryEstimate,
    pub rtt_ms: u32,
}

impl From<Peer> for FerryPeer {
    fn from(p: Peer) -> Self {
        FerryPeer {
            id: p.id,
            name: p.name,
            os: p.os,
            trusted: p.trusted,
            link_kind: p.link_kind.into(),
            link_mbps: p.link_mbps,
            estimate_bps: p.estimate_bps,
            estimate_source: p.estimate_source.into(),
            rtt_ms: p.rtt_ms,
        }
    }
}

/// A speed test's result, seen from this device: up is from here to the other device.
#[derive(Debug, Clone, uniffi::Record)]
pub struct FerrySpeed {
    pub ok: bool,
    pub up_bps: u64,
    pub down_bps: u64,
    pub rtt_ms: u32,
    pub error: Option<String>,
}

impl From<SpeedResult> for FerrySpeed {
    fn from(r: SpeedResult) -> Self {
        FerrySpeed { ok: r.ok, up_bps: r.up_bps, down_bps: r.down_bps, rtt_ms: r.rtt_ms, error: r.error }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FerryOffer {
    pub id: u64,
    pub transfer: u64,
    pub from_id: String,
    pub from_name: String,
    pub from_os: String,
    pub trusted: bool,
    pub names: Vec<String>,
    pub file_count: u64,
    pub total_bytes: u64,
}

impl From<Offer> for FerryOffer {
    fn from(o: Offer) -> Self {
        FerryOffer {
            id: o.id,
            transfer: o.transfer,
            from_id: o.from_id,
            from_name: o.from_name,
            from_os: o.from_os,
            trusted: o.trusted,
            names: o.names,
            file_count: o.file_count,
            total_bytes: o.total_bytes,
        }
    }
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct FerryResult {
    pub ok: bool,
    pub cancelled: bool,
    pub error: Option<String>,
    pub paths: Vec<String>,
}

impl From<TransferResult> for FerryResult {
    fn from(r: TransferResult) -> Self {
        FerryResult { ok: r.ok, cancelled: r.cancelled, error: r.error, paths: r.paths }
    }
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum FerryStatus {
    Ready,
    DiscoveryUnavailable { reason: String },
}

impl From<NetStatus> for FerryStatus {
    fn from(s: NetStatus) -> Self {
        match s {
            NetStatus::Ready => FerryStatus::Ready,
            NetStatus::DiscoveryUnavailable { reason } => FerryStatus::DiscoveryUnavailable { reason },
        }
    }
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum FerryError {
    #[error("{message}")]
    Network { message: String },
    #[error("{message}")]
    UnknownPeer { message: String },
    #[error("{message}")]
    Io { message: String },
}

impl From<rf_nearby::NearbyError> for FerryError {
    fn from(e: rf_nearby::NearbyError) -> Self {
        let message = e.to_string();
        match e {
            rf_nearby::NearbyError::Network(_) => FerryError::Network { message },
            rf_nearby::NearbyError::UnknownPeer(_) => FerryError::UnknownPeer { message },
            rf_nearby::NearbyError::Io(_) => FerryError::Io { message },
        }
    }
}

/// What Ferry tells the app, implemented in Swift. Called from Ferry's own threads.
#[uniffi::export(with_foreign)]
pub trait FerryListener: Send + Sync {
    fn peers_changed(&self, peers: Vec<FerryPeer>);
    fn offer(&self, offer: FerryOffer);
    fn receiving(&self, offer: FerryOffer);
    fn progress(&self, transfer: u64, done: u64, total: u64);
    fn finished(&self, transfer: u64, result: FerryResult);
    fn pairing_code(&self, peer: String, code: String);
    fn paired(&self, peer: String, ok: bool);
    fn visibility_changed(&self, visibility: FerryVisibility);
    fn status(&self, status: FerryStatus);
    fn speed_measured(&self, peer: String, result: FerrySpeed);
}

/// rf-nearby's listener, forwarding to the Swift one.
struct Bridge(Arc<dyn FerryListener>);

impl NearbyListener for Bridge {
    fn peers_changed(&self, peers: Vec<Peer>) {
        self.0.peers_changed(peers.into_iter().map(Into::into).collect())
    }
    fn offer(&self, offer: Offer) {
        self.0.offer(offer.into())
    }
    fn receiving(&self, offer: Offer) {
        self.0.receiving(offer.into())
    }
    fn progress(&self, transfer: u64, done: u64, total: u64) {
        self.0.progress(transfer, done, total)
    }
    fn finished(&self, transfer: u64, result: TransferResult) {
        self.0.finished(transfer, result.into())
    }
    fn pairing_code(&self, peer: String, code: String) {
        self.0.pairing_code(peer, code)
    }
    fn paired(&self, peer: String, ok: bool) {
        self.0.paired(peer, ok)
    }
    fn visibility_changed(&self, visibility: Visibility) {
        self.0.visibility_changed(visibility.into())
    }
    fn status(&self, status: NetStatus) {
        self.0.status(status.into())
    }
    fn speed_measured(&self, peer: String, result: SpeedResult) {
        self.0.speed_measured(peer, result.into())
    }
}

/// A running Ferry: advertising, listening, sending.
#[derive(uniffi::Object)]
pub struct Ferry {
    inner: Arc<rf_nearby::Nearby>,
}

#[uniffi::export]
impl Ferry {
    #[uniffi::constructor]
    pub fn start(config: FerryConfig, listener: Arc<dyn FerryListener>) -> Result<Arc<Self>, FerryError> {
        let config = Config {
            name: config.name,
            os: config.os,
            state_dir: config.state_dir,
            inbox: config.inbox,
            auto_accept_trusted: config.auto_accept_trusted,
            port: config.port,
            discovery: config.discovery,
            everyone_minutes: config.everyone_minutes,
            local_only: config.local_only,
        };
        let inner = rf_nearby::Nearby::start(config, Arc::new(Bridge(listener)))?;
        Ok(Arc::new(Ferry { inner }))
    }

    pub fn id(&self) -> String {
        self.inner.id()
    }
    pub fn fingerprint(&self) -> String {
        self.inner.fingerprint()
    }
    pub fn port(&self) -> u16 {
        self.inner.port()
    }
    pub fn visibility(&self) -> FerryVisibility {
        self.inner.visibility().into()
    }
    pub fn set_visibility(&self, visibility: FerryVisibility) {
        self.inner.set_visibility(visibility.into())
    }
    pub fn peers(&self) -> Vec<FerryPeer> {
        self.inner.peers().into_iter().map(Into::into).collect()
    }
    pub fn trusted(&self) -> Vec<FerryPeer> {
        self.inner.trusted().into_iter().map(Into::into).collect()
    }
    /// Starts sending; the transfer's id comes back at once, its progress and end through the listener.
    pub fn send(&self, peer_id: String, paths: Vec<String>) -> Result<u64, FerryError> {
        Ok(self.inner.send(peer_id, paths)?)
    }
    pub fn send_to_address(&self, address: String, paths: Vec<String>) -> Result<u64, FerryError> {
        Ok(self.inner.send_to_address(address, paths)?)
    }
    pub fn accept(&self, offer_id: u64, dest_dir: String) {
        self.inner.accept(offer_id, dest_dir)
    }
    pub fn reject(&self, offer_id: u64) {
        self.inner.reject(offer_id)
    }
    pub fn pair(&self, peer_id: String) -> Result<(), FerryError> {
        Ok(self.inner.pair(peer_id)?)
    }
    pub fn pair_with_address(&self, address: String, peer_id: String) -> Result<(), FerryError> {
        Ok(self.inner.pair_with_address(address, peer_id)?)
    }
    pub fn confirm_pairing(&self, peer: String, ok: bool) {
        self.inner.confirm_pairing(peer, ok)
    }
    pub fn forget(&self, peer_id: String) -> Result<(), FerryError> {
        Ok(self.inner.forget(peer_id)?)
    }
    /// This device's link, told again whenever it changes (it is announced to the others).
    pub fn set_link(&self, kind: FerryLink, mbps: u32) {
        self.inner.set_link(kind.into(), mbps)
    }
    /// Measures the speed to and from a device (about 5 s); the result comes through the listener.
    pub fn measure_speed(&self, peer_id: String) -> Result<u64, FerryError> {
        Ok(self.inner.measure_speed(peer_id)?)
    }
    pub fn measure_speed_address(&self, address: String, peer_id: String) -> Result<u64, FerryError> {
        Ok(self.inner.measure_speed_address(address, peer_id)?)
    }
    /// Forgets the devices seen, looks again and announces this device again.
    pub fn refresh(&self) {
        self.inner.refresh()
    }
    pub fn cancel(&self, transfer: u64) {
        self.inner.cancel(transfer)
    }
    pub fn stop(&self) {
        self.inner.stop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::mpsc::{Sender, channel};
    use std::time::Duration;

    enum Ev {
        Offer(FerryOffer),
        Finished(FerryResult),
        Speed(FerrySpeed),
    }

    /// A Swift-side listener stand-in: what crosses the bridge, collected.
    struct Collect(Mutex<Sender<Ev>>);

    impl FerryListener for Collect {
        fn peers_changed(&self, _: Vec<FerryPeer>) {}
        fn offer(&self, offer: FerryOffer) {
            let _ = self.0.lock().unwrap().send(Ev::Offer(offer));
        }
        fn receiving(&self, _: FerryOffer) {}
        fn progress(&self, _: u64, done: u64, total: u64) {
            assert!(done <= total);
        }
        fn finished(&self, _: u64, result: FerryResult) {
            let _ = self.0.lock().unwrap().send(Ev::Finished(result));
        }
        fn pairing_code(&self, _: String, _: String) {}
        fn paired(&self, _: String, _: bool) {}
        fn visibility_changed(&self, _: FerryVisibility) {}
        fn status(&self, _: FerryStatus) {}
        fn speed_measured(&self, _: String, result: FerrySpeed) {
            let _ = self.0.lock().unwrap().send(Ev::Speed(result));
        }
    }

    fn scratch(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rf-mac-ferry-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn device(name: &str, dir: &std::path::Path) -> (Arc<Ferry>, std::sync::mpsc::Receiver<Ev>) {
        let (tx, rx) = channel();
        let config = FerryConfig {
            name: name.into(),
            os: "macos".into(),
            state_dir: dir.join("state").display().to_string(),
            inbox: dir.join("inbox").display().to_string(),
            auto_accept_trusted: false,
            port: 0,
            discovery: false,
            everyone_minutes: 0,
            local_only: true,
        };
        (Ferry::start(config, Arc::new(Collect(Mutex::new(tx)))).unwrap(), rx)
    }

    /// A file goes from one Ferry to another across the bridge: the offer reaches the Swift
    /// side's listener, is accepted, and the file arrives whole.
    #[test]
    fn a_file_crosses_the_bridge() {
        let d = scratch("send");
        let (a, _a_events) = device("A", &d.join("a"));
        let (b, b_events) = device("B", &d.join("b"));
        b.set_visibility(FerryVisibility::Everyone);
        let file = d.join("hello.txt");
        std::fs::write(&file, b"merhaba").unwrap();
        a.send_to_address(format!("127.0.0.1:{}", b.port()), vec![file.display().to_string()]).unwrap();
        let wait = Duration::from_secs(20);
        let Ev::Offer(offer) = b_events.recv_timeout(wait).unwrap() else { panic!("expected an offer") };
        assert_eq!((offer.from_name.as_str(), offer.file_count, offer.total_bytes), ("A", 1, 7));
        let dest = d.join("received");
        b.accept(offer.id, dest.display().to_string());
        let Ev::Finished(result) = b_events.recv_timeout(wait).unwrap() else { panic!("expected the end") };
        assert!(result.ok, "{:?}", result.error);
        assert_eq!(std::fs::read(dest.join("hello.txt")).unwrap(), b"merhaba");
        a.stop();
        b.stop();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A speed test runs across the bridge and comes back with both directions measured.
    #[test]
    fn a_speed_test_crosses_the_bridge() {
        let d = scratch("speed");
        let (a, a_events) = device("A", &d.join("a"));
        let (b, _b_events) = device("B", &d.join("b"));
        b.set_visibility(FerryVisibility::Everyone);
        a.measure_speed_address(format!("127.0.0.1:{}", b.port()), b.id()).unwrap();
        let Ev::Speed(r) = a_events.recv_timeout(Duration::from_secs(30)).unwrap() else { panic!("expected a speed result") };
        assert!(r.ok, "{:?}", r.error);
        assert!(r.up_bps > 0 && r.down_bps > 0, "{} / {}", r.up_bps, r.down_bps);
        a.stop();
        b.stop();
        let _ = std::fs::remove_dir_all(&d);
    }
}
