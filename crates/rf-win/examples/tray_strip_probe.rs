//! Prints where the dock's tray strip, its chevron, its overflow panel and a tip would be for N icons on a monitor of a given size and
//! scale: the arithmetic of `rf_win::dock::{tray_strip, tray_order, tray_tip}` only. Windowless and read-only: it calls nothing of
//! Windows, reads no tray, no icon and no file, and writes nothing. Use it to know where to point (the numbers are CLIENT coordinates of
//! the right island's window, origin at its top left corner, and the same on the screen for the monitor given).
//!
//!     cargo run -p rf-win --example tray_strip_probe --offline                           (6 icons, 2560 x 1440 at 100 %)
//!     cargo run -p rf-win --example tray_strip_probe --offline -- 14 1280 1.0 0           (14 icons, a 1280 px wide screen, none hidden)
//!     arguments: <icons> <work width px> <scale> <hidden by the user, from the end>

use rf_win::dock::layout::{self, Rect, Widths, px};
use rf_win::dock::tray_order::{Dest, StableId, TrayOrder};
use rf_win::dock::tray_strip as ts;
use rf_win::dock::tray_tip as tip;

/// The right island without a strip, logical px: padding, two buttons, a separator between gaps, a ten character clock, the bell, padding
/// (`rf-app`'s `plan::right_width`; the numbers are repeated here because rf-win cannot see rf-app).
const ISLAND_WITHOUT_STRIP: f32 = 8.0 + 32.0 + 4.0 + 32.0 + 8.0 + 1.0 + 8.0 + 78.0 + 4.0 + 32.0 + 8.0;

fn main() {
    let arg = |i: usize| std::env::args().nth(i);
    let icons: usize = arg(1).and_then(|a| a.parse().ok()).unwrap_or(6);
    let work_w: i32 = arg(2).and_then(|a| a.parse().ok()).unwrap_or(2560);
    let scale: f32 = arg(3).and_then(|a| a.parse().ok()).unwrap_or(1.0);
    let hide_last: usize = arg(4).and_then(|a| a.parse().ok()).unwrap_or(0);

    let ids: Vec<StableId> = (0..icons).map(|i| StableId::from_program(&format!("C:\\Fake\\App{i}.exe"), i as u32)).collect();
    let mut order = TrayOrder::new();
    for i in icons.saturating_sub(hide_last)..icons {
        order.reorder(&ids, i, Dest::Hidden, usize::MAX);
    }
    let arr = order.arrange(&ids);
    let limit = ts::strip_limit(work_w as f32 / scale);
    let lay = ts::layout(&arr.all, &arr.hidden, limit);
    println!("{icons} icons, {hide_last} hidden by the user, work area {work_w} px wide at {scale} x: the strip block may be {limit} px wide");
    println!("in the strip: {:?}   in the overflow: {:?}   chevron: {}", lay.visible, lay.overflow, lay.chevron);
    println!("the strip adds {} px to the right island: {} -> {} px wide", lay.extra_width(), ISLAND_WITHOUT_STRIP, ISLAND_WITHOUT_STRIP + lay.extra_width());

    // a 2560 x 1440 style screen: the work area ends 48 px above the bottom with the Windows taskbar
    let work = Rect::new(0, 0, work_w, 1392);
    let widths = Widths { left: 330.0, middle: 600.0, right: ISLAND_WITHOUT_STRIP + lay.extra_width() };
    let islands = layout::islands(work, scale, widths);
    let placements = layout::placements(&islands, Rect::new(0, 0, work_w, 1440), scale, false);
    let (glass, window) = (placements.right.glass, placements.right.window);
    println!("right island: glass {glass:?}  window {window:?}");
    let [ml, mt, ..] = placements.right.margins();
    println!("-- client coordinates of the right island's window (device px, origin at the window's top left)");
    for j in 0..lay.visible.len() {
        let c = ts::cell_rect(glass, scale, j);
        println!("cell {j}: x {}..{}  y {}..{}  (centre {}, {})", c.left - window.left, c.right - window.left, c.top - window.top, c.bottom - window.top, (c.left + c.right) / 2 - window.left, (c.top + c.bottom) / 2 - window.top);
    }
    let ch = ts::chevron_rect(glass, scale, lay.visible.len());
    println!("chevron: x {}..{}  y {}..{}  (centre {}, {})", ch.left - window.left, ch.right - window.left, ch.top - window.top, ch.bottom - window.top, (ch.left + ch.right) / 2 - window.left, (ch.top + ch.bottom) / 2 - window.top);
    println!("(the window's margin to the glass is {ml} px left, {mt} px top)");

    // the overflow panel above the island's right edge
    let (pw, ph) = ts::ovf_size(lay.overflow.len());
    let panel = rf_win::dock::panel::place(glass, (pw, ph), scale, rf_win::dock::panel::Side::Right, work);
    println!("-- overflow panel ({} cells in {:?} columns x rows): {pw} x {ph} logical px, on the screen {panel:?}", lay.overflow.len(), ts::ovf_dims(lay.overflow.len()));
    for k in 0..lay.overflow.len() {
        let [x, y, w, h] = ts::ovf_cell(lay.overflow.len(), k);
        println!("overflow cell {k}: panel client x {}..{}  y {}..{}", px(x, scale), px(x + w, scale), px(y, scale), px(y + h, scale));
    }

    // a tip over the first cell
    if !lay.visible.is_empty() {
        let c = ts::cell_rect(glass, scale, 0);
        let text = tip::clean("Posta - 3 yeni ileti");
        let r = tip::rect((c.left + c.right) / 2, c.top, c.bottom, tip::size(&text), scale, work);
        println!("-- a tip of {} characters over cell 0: {:?} logical px, on the screen {r:?}", text.chars().count(), tip::size(&text));
    }
}
