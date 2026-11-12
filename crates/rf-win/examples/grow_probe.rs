//! Prints what the growing Start panel does for a screen, as arithmetic only: no window, nothing read from the system, nothing written.
//!
//!   cargo run -p rf-win --example grow_probe --offline -- [dpi] [dock size factor] [work width] [work height]
//!
//! Defaults: 1.0, 1.0, 2560, 1392 (a 2560 x 1440 monitor with the 48 px taskbar at 100 %). The left island is laid out with the dock unit
//! (the dpi times the size), the panel is placed with the monitor's dpi, as the dock does; then the glass is printed at 0, 60, 120 and 220 ms
//! after a press on the logo (the clock started at 0), and at 0, 40, 80 and 160 ms after a press that closes the open panel.

use rf_win::dock::grow::{self, Corner, Motion, Spec, Stays};
use rf_win::dock::hit;
use rf_win::dock::layout::{self, Rect, Widths};
use rf_win::dock::panel::{self, Side};
use rf_win::dock::{size, startpage};

fn main() {
    let a: Vec<f32> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let (dpi, factor) = (a.first().copied().unwrap_or(1.0), a.get(1).copied().unwrap_or(1.0));
    let work = Rect::new(0, 0, a.get(2).copied().unwrap_or(2560.0) as i32, a.get(3).copied().unwrap_or(1392.0) as i32);
    let unit = size::unit(dpi, factor);
    let islands = layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: 280.0 });
    let island = islands.left;
    let panel_rect = panel::place(island, (startpage::PANEL_W, startpage::PANEL_MAX_H), dpi, Side::Left, work);
    let spec = Spec::new(island, panel_rect, Corner::BottomLeft, layout::RADIUS * unit, 8.0 * dpi);
    println!("dpi {dpi}, dock size {factor}, unit {unit}, work area {}x{}", work.width(), work.height());
    println!("island glass {island:?}\npanel        {panel_rect:?}\ngrown glass  {:?} (the panel's window), corner fixed: {}", spec.grown, spec.corner_is_fixed());
    let region_area = |s: f32| {
        let f = spec.at(s);
        let parts = hit::parts(spec.in_window(&f), f.radius, &[], spec.grown.width(), spec.grown.height());
        hit::bounds(&parts).map_or(0, |b| b.width() * b.height())
    };
    for (name, opening) in [("opening", true), ("closing", false)] {
        let mut m = Motion::closed();
        if opening {
            m.open(0, true);
        } else {
            m.open(0, false);
            m.close(1_000, true);
        }
        let t0 = if opening { 0 } else { 1_000 };
        println!("{name} ({} ms):", if opening { grow::OPEN_MS } else { grow::CLOSE_MS });
        let times: [u64; 4] = if opening { [0, 60, 120, 220] } else { [0, 40, 80, 160] };
        for t in times {
            let s = m.s_at(t0 + t);
            let f = spec.at(s);
            let fade = grow::island_fade(Stays::Logo, &f, true);
            println!(
                "  +{t:>3} ms  s {s:.3}  glass x {:>6.1} y {:>6.1} w {:>6.1} h {:>6.1}  radius {:>5.1}  content {:.2}  island chip {:.2}  region {} px2",
                f.rect[0], f.rect[1], f.rect[2], f.rect[3], f.radius, f.content, fade.others, region_area(s)
            );
        }
    }
}
