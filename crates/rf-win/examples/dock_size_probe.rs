//! Prints what the dock's layout works out at each of the four sizes, for a 1920 x 1080 monitor at 100 % with the 48 px Windows taskbar
//! (work area 1920 x 1032): each island's glass and window, the window's client size (what the toolkit's window is, in px), the room the
//! magnified glass may widen into, the band the dock reserves, the rest centre of the first three icons of the middle island, and a point
//! of its EMPTY glass (between the first two icons) in client and in screen coordinates. Pure arithmetic: no window, no system call, no
//! file, nothing is changed. `cargo run --offline -p rf-win --example dock_size_probe`
//!
//! The widths are those of a dock with six apps and the two places (8 icons, one separator), no weather and no tray strip.

use rf_win::dock::empty_spot;
use rf_win::dock::hover::{Slot, slot_spans};
use rf_win::dock::layout::{self, ICON_REST, MIDDLE_HEIGHT, RADIUS, Rect, Widths};
use rf_win::dock::size::{self, DockSize};

fn main() {
    let monitor = Rect::new(0, 0, 1920, 1080);
    let work = Rect::new(0, 0, 1920, 1032);
    let (left_units, right_units) = (276.0, 215.0);
    let middle_units = layout::middle_width(&[ICON_REST; 8], 1);
    let widths = Widths { left: left_units, middle: middle_units, right: right_units };
    let mut slots = vec![Slot::Icon; 6];
    slots.push(Slot::Separator);
    slots.extend([Slot::Icon; 2]);
    let spans = slot_spans(&slots, &[ICON_REST; 8]);

    for s in DockSize::ALL {
        let unit = size::unit(1.0, s.factor());
        let islands = layout::islands(work, unit, widths);
        let p = layout::placements(&islands, monitor, unit, false);
        println!("== {s:?} (factor {}, unit {unit}) ==", s.factor());
        for (name, pl) in [("left  ", p.left), ("middle", p.middle), ("right ", p.right)] {
            let m = pl.margins();
            println!(
                "{name}: glass {:?} ({} x {})  window {:?}  client {} x {}  margins l{} t{} r{} b{}",
                pl.glass,
                pl.glass.width(),
                pl.glass.height(),
                pl.window,
                pl.window.width(),
                pl.window.height(),
                m[0],
                m[1],
                m[2],
                m[3]
            );
        }
        println!("growth room each side: {} px; band reserved above the taskbar: {} px", layout::growth_room(&islands, unit), layout::reserved_thickness(unit));
        let centre = |i: usize| p.middle.glass.left as f32 + (spans[i].0 + spans[i].1 / 2.0) * unit;
        println!("rest centres of icons 0..2 (screen x): {:.1} {:.1} {:.1}", centre(0), centre(1), centre(2));

        // an empty spot: between the first two icons, at the glass's vertical middle
        let gap_x = (spans[0].0 + spans[0].1 + spans[1].0) / 2.0;
        let glass_local = [0.0, 0.0, p.middle.glass.width() as f32, p.middle.glass.height() as f32];
        let local = (gap_x * unit, glass_local[3] / 2.0);
        // the icons' boxes at rest in the glass's coordinates, device px (they stand 12 units over the bottom)
        let boxes: Vec<[f32; 4]> = slots
            .iter()
            .zip(&spans)
            .filter(|(sl, _)| **sl == Slot::Icon)
            .map(|(_, &(x, w))| [x * unit, (MIDDLE_HEIGHT - 12.0 - w) * unit, w * unit, w * unit])
            .collect();
        let empty = empty_spot::is_empty(glass_local, RADIUS * unit, &boxes, local);
        let [ml, mt, ..] = p.middle.margins();
        let client = (ml as f32 + local.0, mt as f32 + local.1);
        let screen = (p.middle.glass.left as f32 + local.0, p.middle.glass.top as f32 + local.1);
        println!("empty spot of the middle island: client ({:.1}, {:.1}), screen ({:.1}, {:.1}), is_empty = {empty}", client.0, client.1, screen.0, screen.1);
        println!();
    }
}
