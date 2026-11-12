//! Probe of the rules that came after the tray host's first real run (the tray strip after its first run). It touches NOTHING of
//! Windows: no window, no process, no registry, no message, no file. Everything is made up and runs over the host's fakes. It prints
//! JSON with:
//!
//!   * `adoption`: a `NIM_MODIFY` for an icon the host never saw added, in each of the situations `adopt.rs` tells apart (what the
//!     handler did with it, the counts, what the model holds);
//!   * `duplicates`: which made-up (owner, GUID) pairs `own_icons` leaves out of the strip, for the buttons the dock has now and for
//!     a dock without a volume button;
//!   * `placement`: where icons are for with an empty order list, after the user placed some, after the file was written and read
//!     again for a new run, and for a list written before `shown` existed;
//!   * `panel`: the hidden-icons panel for 0, 1, 9, 30 and 256 icons on three screens (columns x rows, size), and the text of the empty
//!     panel in both languages broken into lines.
//!
//!   cargo run -p rf-win --example tray_late_probe

use std::sync::Arc;

use rf_win::dock::tray_order::{Dest, StableId, TrayOrder};
use rf_win::dock::tray_strip as ts;
use rf_win::tray_wire::forward::Outcome;
use rf_win::tray_wire::notify::{DataTier, Nim, NotifyIconData, NotifyIconRequest, nif};
use rf_win::tray_wire::{DW_NOTIFY_ICON, WireGuid};
use rf_win::trayhost::core::{Core, CoreConfig};
use rf_win::trayhost::fakes::{FakeOwners, FakePixels, FakeRelay, ManualClock};
use rf_win::trayhost::own_icons::{self, OwnButtons, Owner, SystemIcon};
use rf_win::trayhost::shared::Shared;
use rf_win::trayhost::traits::Raw;
use serde_json::json;

fn data(hwnd: u32, uid: u32, flags: u32) -> NotifyIconData {
    NotifyIconData {
        cb_size: 956,
        hwnd,
        uid,
        flags,
        callback_message: 0x8005,
        icon: 0,
        tip: String::new(),
        state: 0,
        state_mask: 0,
        info: String::new(),
        timeout_or_version: 0,
        info_title: String::new(),
        info_flags: 0,
        guid: WireGuid::default(),
        balloon_icon: 0,
        tier: DataTier::Full,
    }
}

fn block(m: Nim, d: NotifyIconData) -> Vec<u8> {
    NotifyIconRequest { message: m, data: d, trailing_len: 0, trailing_word: None }.encode()
}

fn adoption() -> Vec<serde_json::Value> {
    let raw = Raw { msg: 0x4A, wparam: 1, lparam: 2 };
    // (name, explorer's answer, flags of the modify, a window for the owner?)
    let cases: [(&str, Outcome, u32, bool); 6] = [
        ("full modify, Explorer accepted", Outcome::Delivered(1), nif::MESSAGE | nif::ICON | nif::TIP, true),
        ("full modify, Explorer refused", Outcome::Delivered(0), nif::MESSAGE | nif::ICON | nif::TIP, true),
        ("full modify, Explorer silent", Outcome::TimedOut, nif::MESSAGE | nif::ICON | nif::TIP, true),
        ("tip only", Outcome::Delivered(1), nif::TIP, true),
        ("picture without a callback", Outcome::Delivered(1), nif::ICON | nif::TIP, true),
        ("full modify, owner window not a window", Outcome::Delivered(1), nif::MESSAGE | nif::ICON | nif::TIP, false),
    ];
    cases
        .into_iter()
        .map(|(name, explorer, flags, owner_window)| {
            let relay = Arc::new(FakeRelay::new(Some(200), explorer));
            let pixels = FakePixels::new();
            pixels.set(0x77, 16, 16, 9);
            let owners = FakeOwners::new();
            if owner_window {
                owners.add_window(0x10, 4242);
                owners.set_exe(4242, "demo.exe");
            }
            let (shared, _events) = Shared::new();
            let core = Core::new(Arc::new(shared), CoreConfig::default(), Box::new(relay), Box::new(pixels), Arc::new(owners), Box::new(ManualClock::default()));
            let mut d = data(0x10, 5, flags);
            d.icon = 0x77;
            d.tip = "made-up".into();
            let reply = core.on_copydata(DW_NOTIFY_ICON, &block(Nim::Modify, d), raw);
            json!({ "case": name, "reply": reply, "icons_in_the_model": core.shared.snapshot().icons.len(), "counts": core.shared.counters.snapshot().summary() })
        })
        .collect()
}

fn duplicates() -> serde_json::Value {
    let win = "C:\\Windows";
    let owners: [(&str, Owner); 5] = [
        ("explorer.exe in Windows", Owner::Path("C:\\Windows\\explorer.exe")),
        ("a driver's control panel", Owner::Path("C:\\Program Files\\Vendor\\Audio\\panel.exe")),
        ("explorer.exe elsewhere", Owner::Path("D:\\Tools\\explorer.exe")),
        ("only the name explorer.exe known", Owner::Name("explorer.exe")),
        ("nothing known", Owner::Unknown),
    ];
    let guids: [(&str, Option<WireGuid>); 4] = [
        ("volume GUID", Some(own_icons::system_guid(SystemIcon::Volume))),
        ("network GUID", Some(own_icons::system_guid(SystemIcon::Network))),
        ("power GUID", Some(own_icons::system_guid(SystemIcon::Power))),
        ("no GUID", None),
    ];
    let dock = OwnButtons { volume: true, network: true, power: false };
    let no_volume = OwnButtons { volume: false, network: true, power: false };
    let mut rows = Vec::new();
    for (oname, owner) in owners {
        for (gname, guid) in guids {
            rows.push(json!({
                "owner": oname,
                "identity": gname,
                "left_out_with_the_docks_buttons": own_icons::duplicate_of(owner, guid.as_ref(), win, dock).map(|i| format!("{i:?}")),
                "left_out_without_a_volume_button": own_icons::duplicate_of(owner, guid.as_ref(), win, no_volume).map(|i| format!("{i:?}")),
            }));
        }
    }
    json!(rows)
}

fn guid_icon(n: u8) -> StableId {
    StableId::from_guid(&format!("{{fa4e0000-0000-4000-8000-0000000000{n:02x}}}")).expect("a valid guid")
}

fn names(ids: &[usize], present: &[StableId]) -> Vec<String> {
    ids.iter().map(|&i| present[i].as_str().to_string()).collect()
}

fn placement() -> serde_json::Value {
    let present = vec![StableId::from_program("C:\\Apps\\Mail.exe", 1), guid_icon(2), StableId::from_program("C:\\Apps\\Cloud.exe", 3)];
    let mut order = TrayOrder::new();
    let fresh = order.arrange(&present);
    // the user drags the third icon, then the first, to the strip
    order.reorder(&present, 2, Dest::Strip, 0);
    order.reorder(&present, 0, Dest::Strip, 1);
    let placed = order.arrange(&present);
    let text = order.to_json();
    // a new run: the file is read, the programs arrive in another order, and a new one is among them
    let later = vec![guid_icon(2), StableId::from_program("C:\\Apps\\New.exe", 9), StableId::from_program("C:\\Apps\\Mail.exe", 1), StableId::from_program("C:\\Apps\\Cloud.exe", 3)];
    let again = TrayOrder::parse(&text).expect("the file reads back").arrange(&later);
    // a file from before `shown` existed
    let old = r#"{"version":1,"order":["guid:{fa4e0000-0000-4000-8000-000000000001}","guid:{fa4e0000-0000-4000-8000-000000000002}"],"hidden":["guid:{fa4e0000-0000-4000-8000-000000000002}"]}"#;
    let old_present = vec![guid_icon(1), guid_icon(2), guid_icon(3)];
    let migrated = TrayOrder::parse(old).expect("an old file reads").arrange(&old_present);
    json!({
        "empty_list": { "strip": names(&fresh.strip(), &present), "panel": names(&fresh.hidden_list(), &present) },
        "after_two_drags": { "strip": names(&placed.strip(), &present), "panel": names(&placed.hidden_list(), &present) },
        "next_run": { "strip": names(&again.strip(), &later), "panel": names(&again.hidden_list(), &later) },
        "file_before_shown_existed": { "strip": names(&migrated.strip(), &old_present), "panel": names(&migrated.hidden_list(), &old_present) },
        "file": text,
    })
}

fn panel() -> serde_json::Value {
    let screens = [("1366 x 768 laptop, 100 %", 1366.0, 728.0), ("1920 x 1080, 125 %", 1536.0, 854.0), ("2560 x 1440, 100 %", 2560.0, 1392.0)];
    let mut rows = Vec::new();
    for (name, w, h) in screens {
        let lim = ts::OvfLimits::for_work(w, h);
        for n in [1usize, 9, 30, 256] {
            let g = ts::ovf_grid(n, lim);
            let (pw, ph) = ts::ovf_size_in(n, lim);
            rows.push(json!({ "screen": name, "icons": n, "columns": g.cols, "rows": g.rows, "all_shown": g.shown == n, "panel_px": [pw, ph], "limit_px": [lim.max_w, lim.max_h] }));
        }
    }
    let tr = ts::ovf_empty("Gizli simge yok", "Gizlemek için simgeleri buraya sürükleyin");
    let en = ts::ovf_empty("No hidden icons", "Drag icons here to hide them");
    json!({
        "grids": rows,
        "empty_turkish": { "title": tr.title, "hint": tr.hint, "panel_px": [tr.size.0, tr.size.1] },
        "empty_english": { "title": en.title, "hint": en.hint, "panel_px": [en.size.0, en.size.1] },
    })
}

fn main() {
    let out = json!({
        "note": "nothing of Windows was touched: made-up blocks over the host's fakes, and pure rules",
        "adoption": adoption(),
        "duplicates": duplicates(),
        "placement": placement(),
        "panel": panel(),
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
