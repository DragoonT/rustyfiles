//! Two devices on this machine (two identities, two endpoints on 127.0.0.1), talking the real protocol over real QUIC: offers asked
//! and answered, files arriving whole, pairing with the same code on both sides, a trusted device's files taken without asking, and an
//! untrusted device turned away by "trusted devices only". Discovery (multicast) is off: the devices are reached by address.

use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rf_nearby::{Config, Nearby, NearbyListener, NetStatus, Offer, Peer, SpeedResult, TransferResult, Visibility};

#[derive(Debug)]
enum Ev {
    Offer(Offer),
    Receiving(Offer),
    Speed(String, SpeedResult),
    Finished(u64, TransferResult),
    Code(String, String),
    Paired(#[allow(dead_code)] String, bool),
}

struct Listener(Mutex<Sender<Ev>>);

impl NearbyListener for Listener {
    fn peers_changed(&self, _: Vec<Peer>) {}
    fn offer(&self, offer: Offer) {
        let _ = self.0.lock().unwrap().send(Ev::Offer(offer));
    }
    fn receiving(&self, offer: Offer) {
        let _ = self.0.lock().unwrap().send(Ev::Receiving(offer));
    }
    fn progress(&self, _: u64, done: u64, total: u64) {
        assert!(done <= total, "progress past the total: {done} / {total}");
    }
    fn finished(&self, t: u64, r: TransferResult) {
        let _ = self.0.lock().unwrap().send(Ev::Finished(t, r));
    }
    fn pairing_code(&self, peer: String, code: String) {
        let _ = self.0.lock().unwrap().send(Ev::Code(peer, code));
    }
    fn paired(&self, peer: String, ok: bool) {
        let _ = self.0.lock().unwrap().send(Ev::Paired(peer, ok));
    }
    fn visibility_changed(&self, _: Visibility) {}
    fn speed_measured(&self, peer: String, result: SpeedResult) {
        let _ = self.0.lock().unwrap().send(Ev::Speed(peer, result));
    }
    fn status(&self, _: NetStatus) {}
}

struct Device {
    nearby: Arc<Nearby>,
    events: Receiver<Ev>,
    inbox: tempfile::TempDir,
    _state: tempfile::TempDir,
}

fn device(name: &str, auto_accept_trusted: bool) -> Device {
    let state = tempfile::tempdir().unwrap();
    let inbox = tempfile::tempdir().unwrap();
    let (tx, rx) = channel();
    let config = Config {
        name: name.into(),
        os: "test".into(),
        state_dir: state.path().to_string_lossy().into(),
        inbox: inbox.path().to_string_lossy().into(),
        auto_accept_trusted,
        port: 0,
        discovery: false,
        everyone_minutes: 0,
        local_only: true,
    };
    let nearby = Nearby::start(config, Arc::new(Listener(Mutex::new(tx)))).unwrap();
    Device { nearby, events: rx, inbox, _state: state }
}

fn addr(d: &Device) -> String {
    format!("127.0.0.1:{}", d.nearby.port())
}

fn next(rx: &Receiver<Ev>) -> Ev {
    rx.recv_timeout(Duration::from_secs(20)).expect("no event within 20 s")
}

/// A folder with a few files of different sizes (one larger than a QUIC flow-control window), a sub folder and an empty folder.
fn sample(root: &Path) -> std::path::PathBuf {
    let top = root.join("project");
    std::fs::create_dir_all(top.join("sub")).unwrap();
    std::fs::create_dir_all(top.join("empty")).unwrap();
    std::fs::write(top.join("readme.txt"), b"hello nearby").unwrap();
    let when = std::time::UNIX_EPOCH + Duration::from_millis(1_600_000_000_123);
    std::fs::File::options().write(true).open(top.join("readme.txt")).unwrap().set_modified(when).unwrap();
    let big: Vec<u8> = (0..(20u32 << 20)).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    std::fs::write(top.join("sub").join("big.bin"), &big).unwrap();
    std::fs::write(top.join("zero.txt"), b"").unwrap();
    top
}

fn same_tree(a: &Path, b: &Path) {
    for e in std::fs::read_dir(a).unwrap().flatten() {
        let other = b.join(e.file_name());
        if e.path().is_dir() {
            assert!(other.is_dir(), "{} is missing", other.display());
            same_tree(&e.path(), &other);
        } else {
            assert_eq!(std::fs::read(e.path()).unwrap(), std::fs::read(&other).unwrap(), "{} differs", other.display());
        }
    }
}

#[test]
fn an_offer_is_asked_and_the_files_arrive_whole() {
    let a = device("A", true);
    let b = device("B", true);
    b.nearby.set_visibility(Visibility::Everyone);
    let src = tempfile::tempdir().unwrap();
    let top = sample(src.path());
    let file = src.path().join("single.txt");
    std::fs::write(&file, b"one file").unwrap();

    let t = a.nearby.send_to_address(addr(&b), vec![top.to_string_lossy().into(), file.to_string_lossy().into()]).unwrap();
    let Ev::Offer(offer) = next(&b.events) else { panic!("B was not asked") };
    assert_eq!((offer.from_name.as_str(), offer.trusted, offer.file_count), ("A", false, 4));
    assert_eq!(offer.names, vec!["project".to_string(), "single.txt".to_string()]);
    let dest = tempfile::tempdir().unwrap();
    b.nearby.accept(offer.id, dest.path().to_string_lossy().into());

    let Ev::Finished(bt, br) = next(&b.events) else { panic!("B did not finish") };
    assert_eq!(bt, offer.transfer);
    assert!(br.ok, "{br:?}");
    let Ev::Finished(at, ar) = next(&a.events) else { panic!("A did not finish") };
    assert_eq!(at, t);
    assert!(ar.ok, "{ar:?}");
    same_tree(&top, &dest.path().join("project"));
    assert!(dest.path().join("project").join("empty").is_dir());
    let kept = std::fs::metadata(dest.path().join("project").join("readme.txt")).unwrap().modified().unwrap();
    assert_eq!(kept, std::time::UNIX_EPOCH + Duration::from_millis(1_600_000_000_123), "the time was not kept");
    assert_eq!(std::fs::read(dest.path().join("single.txt")).unwrap(), b"one file");
    assert_eq!(br.paths.len(), 2);
    // nothing half-received stays behind
    let leftovers: Vec<_> = walkdir(dest.path()).into_iter().filter(|p| p.to_string_lossy().ends_with(rf_nearby::PART)).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn a_declined_offer_sends_nothing() {
    let a = device("A", true);
    let b = device("B", true);
    b.nearby.set_visibility(Visibility::Everyone);
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();
    a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    let Ev::Offer(offer) = next(&b.events) else { panic!() };
    b.nearby.reject(offer.id);
    let Ev::Finished(_, r) = next(&a.events) else { panic!() };
    assert!(!r.ok && !r.cancelled, "{r:?}");
    assert!(std::fs::read_dir(b.inbox.path()).unwrap().next().is_none());
}

#[test]
fn paired_devices_show_the_same_code_and_then_trust_each_other() {
    let a = device("A", true);
    let b = device("B", true);
    b.nearby.set_visibility(Visibility::Everyone);
    a.nearby.pair_with_address(addr(&b), b.nearby.id()).unwrap();
    let Ev::Code(peer_on_b, code_b) = next(&b.events) else { panic!("B shows no code") };
    let Ev::Code(peer_on_a, code_a) = next(&a.events) else { panic!("A shows no code") };
    assert_eq!(code_a, code_b);
    assert_eq!(code_a.len(), 6);
    assert_eq!((peer_on_a, peer_on_b), (b.nearby.id(), a.nearby.id()));
    a.nearby.confirm_pairing(b.nearby.id(), true);
    b.nearby.confirm_pairing(a.nearby.id(), true);
    assert!(matches!(next(&a.events), Ev::Paired(_, true)));
    assert!(matches!(next(&b.events), Ev::Paired(_, true)));
    assert_eq!(a.nearby.trusted().len(), 1);
    assert_eq!(b.nearby.trusted()[0].id, a.nearby.id());

    // a trusted device's files go straight to the inbox, even with "trusted devices only"
    b.nearby.set_visibility(Visibility::Trusted);
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("photo.jpg");
    std::fs::write(&file, b"jpeg").unwrap();
    a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    let Ev::Receiving(offer) = next(&b.events) else { panic!("B was asked") };
    assert!(offer.trusted && offer.from_name == "A" && offer.names == ["photo.jpg"]);
    let Ev::Finished(t, r) = next(&b.events) else { panic!("B did not finish") };
    assert_eq!(t, offer.transfer);
    assert!(r.ok, "{r:?}");
    assert_eq!(std::fs::read(b.inbox.path().join("photo.jpg")).unwrap(), b"jpeg");
}

#[test]
fn trusted_only_turns_strangers_away_and_a_refused_pairing_trusts_nobody() {
    let a = device("A", true);
    let b = device("B", true);
    // B shows itself to trusted devices only (the default): A is a stranger
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();
    a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    let Ev::Finished(_, r) = next(&a.events) else { panic!() };
    assert!(!r.ok, "{r:?}");
    assert!(b.events.recv_timeout(Duration::from_millis(500)).is_err(), "B was bothered");

    b.nearby.set_visibility(Visibility::Everyone);
    a.nearby.pair_with_address(addr(&b), b.nearby.id()).unwrap();
    let Ev::Code(..) = next(&b.events) else { panic!() };
    let Ev::Code(..) = next(&a.events) else { panic!() };
    a.nearby.confirm_pairing(b.nearby.id(), true);
    b.nearby.confirm_pairing(a.nearby.id(), false);
    assert!(matches!(next(&a.events), Ev::Paired(_, false)));
    assert!(matches!(next(&b.events), Ev::Paired(_, false)));
    assert!(a.nearby.trusted().is_empty() && b.nearby.trusted().is_empty());
}

fn walkdir(p: &Path) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(p).unwrap().flatten() {
        if e.path().is_dir() {
            out.extend(walkdir(&e.path()));
        }
        out.push(e.path());
    }
    out
}

#[test]
fn a_broken_transfer_goes_on_and_a_damaged_part_is_sent_again() {
    let a = device("A", true);
    let b = device("B", true);
    b.nearby.set_visibility(Visibility::Everyone);
    let src = tempfile::tempdir().unwrap();
    let top = sample(src.path());
    let dest = tempfile::tempdir().unwrap();
    // an earlier transfer broke off: it left the first 5 MB of big.bin, and a wrong beginning of readme.txt
    let stage = dest.path().join(format!("project{}", rf_nearby::PART));
    std::fs::create_dir_all(stage.join("sub")).unwrap();
    let big = std::fs::read(top.join("sub").join("big.bin")).unwrap();
    std::fs::write(stage.join("sub").join("big.bin"), &big[..5 << 20]).unwrap();
    std::fs::write(stage.join("readme.txt"), b"HELLO").unwrap();
    let send = || a.nearby.send_to_address(addr(&b), vec![top.to_string_lossy().into()]).unwrap();

    send();
    let Ev::Offer(offer) = next(&b.events) else { panic!() };
    b.nearby.accept(offer.id, dest.path().to_string_lossy().into());
    let Ev::Finished(_, r) = next(&b.events) else { panic!() };
    assert!(!r.ok, "a damaged file was kept: {r:?}");
    let Ev::Finished(_, r) = next(&a.events) else { panic!() };
    assert!(!r.ok);
    // the damaged part is gone, the rest waits under its staging name, nothing looks finished
    assert!(!stage.join("readme.txt").exists());
    assert_eq!(std::fs::read(stage.join("sub").join("big.bin")).unwrap(), big);
    assert!(!dest.path().join("project").exists());

    send();
    let Ev::Offer(offer) = next(&b.events) else { panic!() };
    b.nearby.accept(offer.id, dest.path().to_string_lossy().into());
    let Ev::Finished(_, r) = next(&b.events) else { panic!() };
    assert!(r.ok, "{r:?}");
    assert_eq!(r.paths, vec![dest.path().join("project").to_string_lossy().to_string()]);
    let Ev::Finished(_, r) = next(&a.events) else { panic!() };
    assert!(r.ok, "{r:?}");
    same_tree(&top, &dest.path().join("project"));
    assert!(!stage.exists());
}

#[test]
fn a_device_cannot_pile_up_offers() {
    let a = device("A", true);
    let b = device("B", true);
    b.nearby.set_visibility(Visibility::Everyone);
    let src = tempfile::tempdir().unwrap();
    let file = src.path().join("x.txt");
    std::fs::write(&file, b"x").unwrap();
    a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    let Ev::Offer(first) = next(&b.events) else { panic!() };
    // a second offer while the first waits: turned away without asking
    let second = a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    let Ev::Finished(t, r) = next(&a.events) else { panic!() };
    assert_eq!(t, second);
    assert!(!r.ok, "{r:?}");
    assert!(b.events.recv_timeout(Duration::from_millis(300)).is_err(), "B was asked twice");
    // once the first is answered, the device may offer again
    b.nearby.reject(first.id);
    let Ev::Finished(..) = next(&a.events) else { panic!() };
    a.nearby.send_to_address(addr(&b), vec![file.to_string_lossy().into()]).unwrap();
    assert!(matches!(next(&b.events), Ev::Offer(_)));
}

#[test]
fn a_speed_test_measures_both_ways_and_is_not_repeated_at_once() {
    let a = device("A", true);
    let b = device("B", true);
    // B shows itself to trusted devices only: a stranger's test is turned away
    a.nearby.measure_speed_address(addr(&b), b.nearby.id()).unwrap();
    let Ev::Speed(peer, r) = next(&a.events) else { panic!() };
    assert_eq!(peer, b.nearby.id());
    assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("trusted")), "{r:?}");

    b.nearby.set_visibility(Visibility::Everyone);
    a.nearby.measure_speed_address(addr(&b), b.nearby.id()).unwrap();
    let Ev::Speed(_, r) = next(&a.events) else { panic!() };
    assert!(r.ok, "{r:?}");
    assert!(r.up_bps > 1_000_000 && r.down_bps > 1_000_000, "{r:?}");
    // B was asked nothing and wrote nothing
    assert!(b.events.recv_timeout(Duration::from_millis(200)).is_err());
    assert!(std::fs::read_dir(b.inbox.path()).unwrap().next().is_none());
    // the same device again at once: busy
    a.nearby.measure_speed_address(addr(&b), b.nearby.id()).unwrap();
    let Ev::Speed(_, r) = next(&a.events) else { panic!() };
    assert!(!r.ok && r.error.as_deref().is_some_and(|e| e.contains("busy")), "{r:?}");
}
