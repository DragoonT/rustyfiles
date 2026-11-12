//! Windowless, read-only probe of the tray host (shell plan increment D2). It creates no window, registers no class, sends and posts
//! no message, broadcasts nothing, and starts no host. It prints JSON with:
//!
//!   * `tray_windows`: every top-level window of class `Shell_TrayWnd` (found with `EnumWindows` and read back with `GetClassNameW`):
//!     window handle, owner program's file name, rectangle, extended style, visibility, its `TrayNotifyWnd` child's rectangle, and the
//!     NAMES of its window properties (`TaskbandHWND` is the one programs read to find the taskbar's button window; the host copies
//!     it). The values of the properties are not printed. Everything is read from the window manager's own data: nothing is sent to
//!     the window or its program;
//!   * `taskbar_created_id`: what `RegisterWindowMessage("TaskbarCreated")` gives in this session (a lookup: the name exists);
//!   * `fake_run`: the host's handler run over made-up blocks with FAKE traits (a made-up Explorer that answers, made-up pictures): the
//!     replies, the counts, and what the model holds. This is the logic of the host, not Windows.
//!
//!   cargo run -p rf-win --example trayhost_probe

use std::sync::Arc;

use rf_win::tray_wire::notify::{DataTier, Nim, NotifyIconData, NotifyIconRequest, nif};
use rf_win::tray_wire::forward::{Method, Outcome};
use rf_win::tray_wire::rect::{IconRectQuery, RectPacking};
use rf_win::tray_wire::{DW_APPBAR, DW_ICON_RECT, DW_NOTIFY_ICON, Rect, TRAY_CLASS, WireGuid};
use rf_win::trayhost::core::{Core, CoreConfig};
use rf_win::trayhost::fakes::{FakeOwners, FakePixels, FakeRelay, ManualClock};
use rf_win::trayhost::lifecycle::should_mirror_prop;
use rf_win::trayhost::relay::TrayFinder;
use rf_win::trayhost::shared::Shared;
use rf_win::trayhost::traits::Raw;
use rf_win::trayhost::win::{Win32Finder, read_only_facts, taskbar_created_id};
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

fn run_fake(target: Option<isize>, packing: Option<RectPacking>) -> serde_json::Value {
    let relay = Arc::new(FakeRelay::new(target, Outcome::Delivered(1)));
    let pixels = FakePixels::new();
    pixels.set(0x77, 32, 32, 200);
    let owners = FakeOwners::new();
    owners.add_window(0x10, 4242);
    owners.set_exe(4242, "demo.exe");
    let (shared, _events) = Shared::new();
    let core = Core::new(Arc::new(shared), CoreConfig { log_only: false, rect_packing: packing }, Box::new(relay.clone()), Box::new(pixels), Arc::new(owners), Box::new(ManualClock::default()));
    let raw = Raw { msg: 0x4A, wparam: 1, lparam: 2 };
    let mut replies = Vec::new();
    let mut send = |name: &str, dw: usize, bytes: Vec<u8>| {
        let r = core.on_copydata(dw, &bytes, raw);
        replies.push(json!({ "block": name, "reply": r }));
    };
    let mut add = data(0x10, 1, nif::MESSAGE | nif::ICON | nif::TIP);
    add.icon = 0x77;
    add.tip = "made-up tip".into();
    send("NIM_ADD", DW_NOTIFY_ICON, block(Nim::Add, add.clone()));
    send("NIM_ADD again", DW_NOTIFY_ICON, block(Nim::Add, add));
    let mut v = data(0x10, 1, 0);
    v.timeout_or_version = 4;
    send("NIM_SETVERSION 4", DW_NOTIFY_ICON, block(Nim::SetVersion, v));
    let mut balloon = data(0x10, 1, nif::INFO);
    balloon.info = "made-up balloon".into();
    send("NIM_MODIFY with a balloon", DW_NOTIFY_ICON, block(Nim::Modify, balloon));
    send("NIM_MODIFY, unknown icon", DW_NOTIFY_ICON, block(Nim::Modify, data(0x99, 9, nif::TIP)));
    send("appbar block", DW_APPBAR, vec![0u8; 64]);
    send("garbage", DW_NOTIFY_ICON, vec![1, 2, 3]);
    let id = core.shared.snapshot().icons.first().map(|i| i.id);
    if let Some(id) = id {
        core.shared.set_icon_rects(&[(id, Rect { left: 2319, top: 1392, right: 2335, bottom: 1424 })]);
    }
    for m in [1, 2] {
        let q = IconRectQuery { magic: 0x3475_3423, message: m, cb_size: 40, hwnd: 0x10, uid: 1, guid: WireGuid::default(), trailing_len: 0 };
        send(if m == 1 { "rect query 1" } else { "rect query 2" }, DW_ICON_RECT, q.encode());
    }
    send("NIM_DELETE", DW_NOTIFY_ICON, block(Nim::Delete, data(0x10, 1, 0)));
    let passed_on = relay.relayed().iter().filter(|(_, m)| *m == Method::Send).count();
    json!({
        "explorer_present": target.is_some(),
        "rect_packing": packing.map(|p| format!("{p:?}")),
        "replies": replies,
        "relay_calls": passed_on,
        "icons_left": core.shared.snapshot().icons.len(),
        "counts": core.shared.counters.snapshot().summary(),
    })
}

fn main() {
    let finder = Win32Finder;
    let windows: Vec<serde_json::Value> = finder
        .tray_windows()
        .into_iter()
        .map(|w| {
            let f = read_only_facts(w.hwnd);
            json!({
                "class": w.class,
                "is_expected_class": w.class == TRAY_CLASS,
                "hwnd": format!("{:#x}", w.hwnd),
                "pid": w.pid,
                "program": w.exe,
                "rect": f.rect,
                "ex_style": format!("{:#x}", f.ex_style),
                "visible": f.visible,
                "tray_notify_child_rect": f.child_rect,
                "property_names": f.props,
                "property_names_the_host_would_copy": f.props.iter().filter(|n| should_mirror_prop(n)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let out = json!({
        "note": "read-only: nothing was created, sent, posted or broadcast",
        "tray_windows": windows,
        "taskbar_created_id": taskbar_created_id(),
        "fake_run": [run_fake(Some(200), None), run_fake(Some(200), Some(RectPacking::Corners)), run_fake(None, None)],
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
