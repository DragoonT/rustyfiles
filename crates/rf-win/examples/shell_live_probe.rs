//! Windowless, read-only probe of what the shell host does while it runs (the track "a dock and a desktop that survive Explorer leaving
//! and coming back"). It creates no window and no hook, registers no class, sends and posts no message, sets no work area, and starts
//! no host. It prints JSON with:
//!
//!   * `shell_now`: who owns the shell window right now (`explorer`, `foreign` or `nobody`) and the mode;
//!   * `watch`: a `ModeWatch` started from the real look and checked five times at 2 s steps against the real system: with Explorer
//!     running and nothing changing it must say `nothing` every time and never open a process after the first look;
//!   * `key_decisions`: what the media-key decision answers for the seven commands under the REAL shell state (all `nothing` while
//!     Explorer owns the shell window);
//!   * `work_area`: the primary monitor, its work area, how much of the monitor's bottom the work area leaves out, beside the thickness of the
//!     dock's band at its default size (the keeper's own check needs Explorer's answer to a band query, which a probe does not ask), and whether the real work-area setter would be allowed (`permitted`);
//!   * `bare`: the cover windows' state, place and wallpaper route for the real shell window.
//!
//!   cargo run -p rf-win --example shell_live_probe

use rf_core::desktop::cover::bare;
use rf_win::dock::layout::reserved_thickness;
use rf_win::shellhost::appcmd::{self, AppCommand, Shell};
use rf_win::shellhost::mode;
use rf_win::shellhost::watch::{Cause, ModeWatch, SystemLook};
use rf_win::shellhost::workarea;
use serde_json::json;

fn main() {
    let shell = appcmd::shell_now();
    let shell_name = match shell {
        Shell::Explorer => "explorer",
        Shell::Foreign => "foreign",
        Shell::Nobody => "nobody",
    };
    let mut w = ModeWatch::new(&SystemLook);
    let mut steps = Vec::new();
    for i in 0..5u64 {
        steps.push(format!("{:?}", w.check(&SystemLook, i * 2_000, Cause::Tick)));
        if i < 4 {
            std::thread::sleep(std::time::Duration::from_millis(2_000));
        }
    }
    let names = [(8u16, "mute"), (9, "volume down"), (10, "volume up"), (11, "next"), (12, "previous"), (13, "stop"), (14, "play/pause")];
    let decisions: Vec<_> = names.iter().map(|(c, n)| json!({ "command": n, "does": format!("{:?}", appcmd::decide(AppCommand::from_code(*c), shell)) })).collect();
    let work = rf_win::dock::primary_monitor().map(|m| {
        json!({
            "monitor": [m.rect.left, m.rect.top, m.rect.right, m.rect.bottom],
            "work": [m.work.left, m.work.top, m.work.right, m.work.bottom],
            "dpi": m.dpi,
            // What the band keeper compares needs the rectangle Explorer grants for the band (ABM_QUERYPOS, a message to Explorer's
            // taskbar window: this probe sends none), so it cannot be worked out here. What can be read: how much of the monitor's
            // bottom the work area leaves out (the taskbar, plus the dock's band while a dock holds one), and the band's thickness
            // at the dock's default size, to set beside it.
            "bottom_left_out_of_work_area_px": m.rect.bottom - m.work.bottom,
            "band_thickness_at_default_size_px": reserved_thickness(m.scale()),
            "real_setter_permitted": workarea::permitted(shell),
        })
    });
    let state = rf_win::desktop::bare::state_now();
    let out = json!({
        "shell_now": { "who": shell_name, "mode": mode::detect().name() },
        "watch": { "mode": w.mode().name(), "five_checks_at_2s": steps },
        "key_decisions": decisions,
        "work_area": work,
        "bare": { "state": format!("{state:?}"), "place": format!("{:?}", bare::place_for(state)), "wallpaper_route": format!("{:?}", bare::route_for(state)) },
    });
    println!("{}", serde_json::to_string_pretty(&out).unwrap());
}
