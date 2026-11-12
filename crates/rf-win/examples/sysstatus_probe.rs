//! Prints what each system source of the dock's control centre reports on this machine. Read-only: no volume, mute, media key, radio,
//! brightness or power change, and no window. Media titles and artists are not printed (only whether a session exists and the
//! program's short name); the Wi-Fi name and the Bluetooth devices' names are not printed either (counts only).
//!
//!     cargo run -p rf-win --example sysstatus_probe --offline

use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

use rf_win::sysstatus::{audio, bluetooth, brightness, media, network, power};

fn main() {
    println!("-- audio (read once)");
    let a = audio::read_once();
    println!(
        "available: {}  volume: {} %  muted: {}  steps: {}  default output: {:?}  outputs: {}  glyph: {:?}  slider fraction: {:.2}",
        a.available,
        audio::percent(a.volume),
        a.muted,
        a.steps,
        a.device,
        a.devices.len(),
        audio::glyph(&a),
        audio::volume_to_slider(a.volume),
    );
    for d in &a.devices {
        println!("  output: {:?}{}", d.name, if d.default { "  (default)" } else { "" });
    }

    println!("-- audio (the event thread: the first callback after subscribing)");
    let (tx, rx) = channel();
    let started = Instant::now();
    let sub = audio::subscribe(move |s| {
        let _ = tx.send(s.clone());
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(s) => println!("first callback after {} ms: available {}, volume {} %, muted {}", started.elapsed().as_millis(), s.available, audio::percent(s.volume), s.muted),
        Err(_) => println!("NO callback within 5 s"),
    }
    drop(sub);

    println!("-- network (read once)");
    let n = network::read_once();
    let link = match &n.link {
        network::Link::Disconnected => "not connected".to_string(),
        network::Link::Ethernet => "Ethernet".to_string(),
        network::Link::WiFi { name, bars } => format!("Wi-Fi ({} bars, name has {} characters)", bars, name.chars().count()),
        network::Link::Cellular => "mobile broadband".to_string(),
        network::Link::Other => "something else".to_string(),
    };
    println!("source available: {}  link: {}  reach: {:?}  glyph: {:?}", n.available, link, n.reach, network::glyph(&n));
    println!("-- network (the event thread: the first callback)");
    let (tx, rx) = channel();
    let started = Instant::now();
    let sub = network::subscribe(move |s| {
        let _ = tx.send(s.clone());
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(s) => println!("first callback after {} ms: {:?}", started.elapsed().as_millis(), network::glyph(&s)),
        Err(_) => println!("NO callback within 5 s"),
    }
    drop(sub);

    println!("-- media session (read once)");
    let m = media::read_once();
    match &m.session {
        Some(s) => println!(
            "manager available: {}  a session exists: yes  program: {:?}  playing: {}  previous/next/toggle enabled: {}/{}/{}  artwork: {}",
            m.available,
            s.app,
            s.playing,
            s.can_previous,
            s.can_next,
            s.can_toggle,
            s.artwork.as_ref().map_or("none".to_string(), |a| format!("{0}x{0} px, {1} bytes", a.size, a.bgra.len())),
        ),
        None => println!("manager available: {}  a session exists: no", m.available),
    }

    println!("-- bluetooth (read once)");
    let b = bluetooth::read_once();
    println!("radio present: {}  power: {:?}  connected devices: {}", b.available, b.power, b.connected.len());

    println!("-- the follow functions that run while the panel is open (first callback; no command is sent)");
    {
        let (tx, rx) = channel();
        let started = Instant::now();
        let follow = media::subscribe(move |s| {
            let _ = tx.send((s.available, s.session.as_ref().map(|x| x.app.clone())));
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok((available, app)) => println!("media: first callback after {} ms: manager {}, a session exists: {}{}", started.elapsed().as_millis(), available, if app.is_some() { "yes" } else { "no" }, app.map(|a| format!(" ({a})")).unwrap_or_default()),
            Err(_) => println!("media: NO callback within 5 s"),
        }
        drop(follow);

        let (tx, rx) = channel();
        let started = Instant::now();
        let sub = bluetooth::subscribe(move |s| {
            let _ = tx.send(s.clone());
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(s) => println!("bluetooth: first callback after {} ms: radio {}, {:?}, {} connected", started.elapsed().as_millis(), s.available, s.power, s.connected.len()),
            Err(_) => println!("bluetooth: NO callback within 5 s"),
        }
        drop(sub);

        let (tx, rx) = channel();
        let started = Instant::now();
        let br = brightness::open(move |s| {
            let _ = tx.send(*s);
        });
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(s) => println!("brightness: first callback after {} ms: available {}, value {}", started.elapsed().as_millis(), s.available, s.value),
            Err(_) => println!("brightness: NO callback within 5 s"),
        }
        drop(br);

        // (registers nothing on a machine without a battery)
        let sub = power::subscribe(|_| {});
        drop(sub);
        println!("power: subscribed and unsubscribed");
        // give the threads a moment to end, then show that nothing of ours is left running but the two that live for the process
        std::thread::sleep(Duration::from_millis(500));
    }

    println!("-- battery");
    let p = power::read();
    println!("{p:?}");

    println!("-- brightness (DDC/CI of the primary monitor, one read)");
    let started = Instant::now();
    let br = brightness::read_once();
    println!("available: {}  range {}..={}  value {}  fraction {:.2}  ({} ms)", br.available, br.min, br.max, br.value, br.fraction(), started.elapsed().as_millis());
}
