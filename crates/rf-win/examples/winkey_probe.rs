//! The keyboard layer's decisions, windowless, through the FAKES only: scripted key sequences go through the real decision code
//! (`rf_win::winkey::KeyLayer`) behind a `FakeFeed`, and the probe prints what was decided for each: swallowed or passed, the mask
//! key asked for, what was posted. It installs NO hook, sends NO key, reads NO real key, creates NO window. The real hook, the real
//! `SendInput` and the real keyboard probe are not reached by it (their constructors are not even called).
//!
//!   cargo run -p rf-win --example winkey_probe

use std::sync::Arc;

use rf_win::switcher::hook::{FakeFeed, KeyFeed, LLKHF_ALTDOWN, LLKHF_INJECTED, VK_LMENU, VK_TAB};
use rf_win::winkey::fakes::FakeMask;
use rf_win::winkey::machine::VK_LWIN;
use rf_win::winkey::{Clock, KeyLayer, Posted, STALE_AFTER_MS, Shared};

struct Rig {
    feed: FakeFeed,
    shared: Arc<Shared>,
    clock: Arc<Clock>,
    mask: Arc<FakeMask>,
}

fn rig(log_only: bool) -> Rig {
    let clock = Arc::new(Clock::manual());
    clock.set(10_000);
    let shared = Arc::new(Shared::new(clock.clone(), log_only));
    shared.set_wanted(true, true);
    shared.beat(false);
    let mask = Arc::new(FakeMask::default());
    let mut layer = KeyLayer::new(shared.clone(), mask.clone(), false, false);
    let mut feed = FakeFeed::default();
    feed.start_raw(Box::new(move |k| layer.on_event(k))).expect("a fake feed starts");
    Rig { feed, shared, clock, mask }
}

fn show(name: &str, r: &mut Rig, script: &[(&str, u32, u32, u32, bool)]) {
    let before = r.mask.sent();
    let mut line = String::new();
    for (what, vk, scan, flags, down) in script {
        let swallowed = r.feed.push_raw(*vk, *scan, *flags, *down);
        line.push_str(&format!("{what}{}{} ", if *down { "+" } else { "-" }, if swallowed { "(swallowed)" } else { "" }));
    }
    let posts: Vec<Posted> = std::iter::from_fn(|| r.shared.pop()).collect();
    println!("{name}\n  events: {line}\n  mask keys asked for: {}\n  posted: {posts:?}", r.mask.sent() - before);
}

fn main() {
    let tap = [("Win", VK_LWIN, 0x5B, 0, true), ("Win", VK_LWIN, 0x5B, 0, false)];
    let chord = [("Win", VK_LWIN, 0x5B, 0, true), ("E", 0x45, 0x12, 0, true), ("E", 0x45, 0x12, 0, false), ("Win", VK_LWIN, 0x5B, 0, false)];
    let alt_tab = [
        ("Alt", VK_LMENU, 0x38, LLKHF_ALTDOWN, true),
        ("Tab", VK_TAB, 0x0F, LLKHF_ALTDOWN, true),
        ("Tab", VK_TAB, 0x0F, LLKHF_ALTDOWN, false),
        ("Alt", VK_LMENU, 0x38, 0, false),
    ];
    let injected_tap = [("Win", VK_LWIN, 0x5B, LLKHF_INJECTED, true), ("Win", VK_LWIN, 0x5B, LLKHF_INJECTED, false)];

    let mut r = rig(false);
    show("a clean tap of the Windows key", &mut r, &tap);
    show("Win+E (a chord)", &mut r, &chord);
    show("a tap sent by a program (the dock's own Start button)", &mut r, &injected_tap);
    show("Alt+Tab, released", &mut r, &alt_tab);

    r.clock.set(10_000 + STALE_AFTER_MS + 1);
    show("Alt+Tab with a stale heartbeat (fails open)", &mut r, &alt_tab);
    show("a tap with a stale heartbeat (fails open)", &mut r, &tap);

    let mut l = rig(true);
    show("log-only: a tap", &mut l, &tap);
    show("log-only: Alt+Tab", &mut l, &alt_tab);
}
