//! What of an island's window is the island's: the geometry that decides where the pointer is let through, and the one Win32 call that
//! carries it out.
//!
//! WHY A WINDOW REGION. An island's window is bigger than its glass (a transparent margin for the shadow, headroom for icons that grow,
//! room for the glass to widen), and the window is a DirectComposition one (no redirection bitmap) whose pixels the system does not
//! see, so it cannot tell a transparent pixel from an opaque one. The ways to let a click through to what lies below were weighed:
//!
//! * `WM_NCHITTEST` returning `HTTRANSPARENT`: documented to pass the message only on to windows of the SAME thread ("in a window
//!   currently covered by another window in the same thread"). A click on the margin would not reach another program's window (the
//!   Windows taskbar below the dock is Explorer's). Not enough.
//! * `WS_EX_LAYERED | WS_EX_TRANSPARENT`: passes everything through, to every process, but for the WHOLE window (the island would not get
//!   its own clicks), and a layered window's per pixel alpha is the one `UpdateLayeredWindow` gave, not what DirectComposition draws.
//! * A window region (`SetWindowRgn`): the system treats what lies outside the region as not being part of the window, for painting
//!   AND for hit testing. A click outside it goes to whatever is below, in any process, and the pointer's move and leave messages follow
//!   the region too. This is the one used. It is updated whenever the glass or an icon moves (a pure rectangle list, below).
//!
//! WHAT FOLLOWS FROM IT: the window shows nothing outside its region (the system's clip, not ours; `SetWindowRgn` is documented as the
//! area of the window where the system permits drawing). The region is therefore the glass (a rounded rectangle, the shape's own) plus
//! the boxes of the icons that stand out of it, grown by 1 px so that the glass's own anti aliased edge, which a region cannot have, is
//! not cut into. What lies outside lets the pointer through, and is not shown.
//!
//! THE SHADOW AND THE REGION EXCLUDE EACH OTHER. A shadow lies outside the glass, so a region of exactly the glass and the icons clips
//! it away; a region that includes the shadow's reach (22 px out, 26 px below the glass) would take the clicks meant for what lies
//! under it: the taskbar below the dock and the windows above it. The dock chooses the pointer: by default its windows keep only the
//! edge room the glass needs (`layout::EDGE_MARGIN`) and the glass draws no shadow. `RUSTYFILES_DOCK_SHADOW=1` gives the windows room
//! for the shadow and draws it, so that a first look at a real screen can see whether the system clips it (this has not been
//! seen). A shadow drawn in a window of its own that takes no input would have both; it is not built.

use super::layout::Rect;

/// How far the region goes beyond the glass's outline and the icons' boxes, device px: a region has hard, pixel-wide edges, and the
/// glass's own edge is one pixel of coverage wide, so the region must not start inside it.
pub const EDGE_SLACK: i32 = 1;

/// A rectangle with rounded corners, device px in the window's own coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoundRect {
    pub rect: Rect,
    pub radius: i32,
}

/// One piece of the region.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Round(RoundRect),
    Rect(Rect),
}

/// The pieces of the window that take the pointer: the glass (`glass` = x, y, w, h in the window's device px, `radius` its corner
/// radius) and the boxes `icons` (x, y, w, h each) that stand out of it, all grown by `EDGE_SLACK` and cut to the window
/// (`window_w` x `window_h`). A glass or an icon that has no area contributes nothing. Pure: nothing here touches a window.
pub fn parts(glass: [f32; 4], radius: f32, icons: &[[f32; 4]], window_w: i32, window_h: i32) -> Vec<Part> {
    let window = Rect::new(0, 0, window_w.max(0), window_h.max(0));
    // outward to whole pixels, then the slack, then the window
    let grown = |r: [f32; 4]| -> Option<Rect> {
        if !r.iter().all(|v| v.is_finite()) || r[2] <= 0.0 || r[3] <= 0.0 {
            return None;
        }
        let g = Rect::new(
            r[0].floor() as i32 - EDGE_SLACK,
            r[1].floor() as i32 - EDGE_SLACK,
            (r[0] + r[2]).ceil() as i32 + EDGE_SLACK,
            (r[1] + r[3]).ceil() as i32 + EDGE_SLACK,
        );
        let c = Rect::new(g.left.max(window.left), g.top.max(window.top), g.right.min(window.right), g.bottom.min(window.bottom));
        (c.width() > 0 && c.height() > 0).then_some(c)
    };
    let mut out = Vec::with_capacity(1 + icons.len());
    if let Some(r) = grown(glass) {
        // the radius belongs to the outline; the slack grows it by as much
        let max = (r.width().min(r.height()) / 2).max(0);
        let radius = if radius.is_finite() { (radius.max(0.0).round() as i32 + EDGE_SLACK).min(max) } else { 0 };
        out.push(Part::Round(RoundRect { rect: r, radius }));
    }
    out.extend(icons.iter().filter_map(|&i| grown(i)).map(Part::Rect));
    out
}

/// Whether the pointer at (`x`, `y`) in the window's coordinates is over the island (a piece of the region) rather than over what the
/// window lets through. The same decision the system makes from the region, for tests and for the view's own checks.
pub fn takes_pointer(parts: &[Part], x: i32, y: i32) -> bool {
    parts.iter().any(|p| match p {
        Part::Rect(r) => r.contains(x, y),
        Part::Round(rr) => {
            let r = rr.rect;
            if !r.contains(x, y) {
                return false;
            }
            let k = rr.radius;
            // inside the corner squares the point must be within the corner's circle (pixel centres)
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = if px < (r.left + k) as f32 { Some((r.left + k) as f32) } else if px > (r.right - k) as f32 { Some((r.right - k) as f32) } else { None };
            let cy = if py < (r.top + k) as f32 { Some((r.top + k) as f32) } else if py > (r.bottom - k) as f32 { Some((r.bottom - k) as f32) } else { None };
            match (cx, cy) {
                (Some(cx), Some(cy)) => (px - cx).powi(2) + (py - cy).powi(2) <= (k as f32).powi(2),
                _ => true,
            }
        }
    })
}

/// The rectangle that holds every piece (what the window needs to have been drawn within), `None` for no pieces.
pub fn bounds(parts: &[Part]) -> Option<Rect> {
    parts
        .iter()
        .map(|p| match p {
            Part::Rect(r) => *r,
            Part::Round(rr) => rr.rect,
        })
        .reduce(|a, b| Rect::new(a.left.min(b.left), a.top.min(b.top), a.right.max(b.right), a.bottom.max(b.bottom)))
}

/// Makes `parts` the window's region (replacing the one it has): from now on the system shows and hit tests only these pieces of the
/// window. No pieces: an empty region, the window takes no pointer at all. Call it on the thread that owns the window and from outside
/// any update of the app (like every call that changes a GPUI window, see `rf-app`'s `dock/host.rs`). Returns whether the system took
/// it. The system owns the region from then on; a region it did not take is freed here.
pub fn set_region(hwnd: isize, parts: &[Part]) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, HRGN, RGN_OR, SetWindowRgn};
    unsafe {
        let mut acc: Option<HRGN> = None;
        for p in parts {
            let piece = match p {
                // GDI's ellipse sizes are diameters; the region does not include the right and bottom edges, like a RECT
                Part::Rect(r) => CreateRectRgn(r.left, r.top, r.right, r.bottom),
                Part::Round(rr) => CreateRoundRectRgn(rr.rect.left, rr.rect.top, rr.rect.right + 1, rr.rect.bottom + 1, rr.radius * 2, rr.radius * 2),
            };
            if piece.is_invalid() {
                continue;
            }
            match acc {
                None => acc = Some(piece),
                Some(a) => {
                    CombineRgn(Some(a), Some(a), Some(piece), RGN_OR);
                    let _ = DeleteObject(piece.into());
                }
            }
        }
        let region = match acc {
            Some(r) => r,
            None => CreateRectRgn(0, 0, 0, 0),
        };
        // `false`: no repaint of the window's old area; the next frame the app draws covers it
        if SetWindowRgn(HWND(hwnd as *mut _), Some(region), false) == 0 {
            let _ = DeleteObject(region.into());
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The glass of a 720 x 72 island in a window with margins 68 / 28 (x, y, w, h).
    const GLASS: [f32; 4] = [68.0, 28.0, 720.0, 72.0];

    #[test]
    fn the_glass_alone_is_a_rounded_rectangle_a_pixel_larger_than_it() {
        let p = parts(GLASS, 24.0, &[], 856, 128);
        assert_eq!(p, vec![Part::Round(RoundRect { rect: Rect::new(67, 27, 789, 101), radius: 25 })]);
        // the margin lets the pointer through: 5 px left of the glass, above it, below it, and the window's corners
        for (x, y) in [(60, 60), (400, 10), (400, 110), (400, 127), (0, 0), (855, 127), (855, 0)] {
            assert!(!takes_pointer(&p, x, y), "({x}, {y}) is margin");
        }
        // the glass takes it, also at its edge row and column
        for (x, y) in [(400, 60), (68, 64), (787, 64), (400, 28), (400, 99)] {
            assert!(takes_pointer(&p, x, y), "({x}, {y}) is glass");
        }
        // the corners are round: a pixel just inside the corner of the bounding box is not the island's
        assert!(!takes_pointer(&p, 68, 28));
        assert!(!takes_pointer(&p, 787, 99));
        assert!(takes_pointer(&p, 68 + 24, 28 + 1));
    }

    #[test]
    fn an_icon_standing_out_of_the_glass_takes_the_pointer_over_its_box() {
        // an icon 68 px wide at x 300, whose top is 8 px above the glass's top edge (y 28)
        let icon = [300.0, 20.0, 68.0, 68.0];
        let p = parts(GLASS, 24.0, &[icon], 856, 128);
        assert_eq!(p.len(), 2);
        assert!(takes_pointer(&p, 330, 22), "over the glass's top edge, on the icon");
        assert!(takes_pointer(&p, 300, 20) && takes_pointer(&p, 367, 87));
        assert!(!takes_pointer(&p, 330, 17), "above the icon: the margin");
        assert!(!takes_pointer(&p, 290, 22), "beside the icon, above the glass: the margin");
        assert!(!takes_pointer(&p, 372, 22));
        // beside the icon but level with the glass: the glass
        assert!(takes_pointer(&p, 380, 40));
        assert_eq!(bounds(&p), Some(Rect::new(67, 19, 789, 101)));
    }

    /// At every moment of the animation what the pointer is let through to is decided by the pieces alone: a sweep of icons growing
    /// shows the region following them, with no point of the margin ever taken that is not within the slack of a piece.
    #[test]
    fn the_margin_stays_open_at_every_moment_of_the_growth() {
        for step in 0..=20 {
            let grow = step as f32 / 20.0;
            let size = 44.0 + 24.2 * grow;
            // an icon that stands 12 px above the island's bottom (glass bottom y = 100): its top is 100 - 12 - size
            let icon = [300.0, 100.0 - 12.0 - size, size, size];
            let p = parts(GLASS, 24.0, &[icon], 856, 128);
            for y in 0..128 {
                for x in 0..856 {
                    let in_glass_bbox = (66..=789).contains(&x) && (26..=101).contains(&y);
                    let in_icon = (x as f32) >= icon[0] - 2.0 && (x as f32) <= icon[0] + icon[2] + 2.0 && (y as f32) >= icon[1] - 2.0 && (y as f32) <= icon[1] + icon[3] + 2.0;
                    if takes_pointer(&p, x, y) {
                        assert!(in_glass_bbox || in_icon, "({x}, {y}) takes the pointer but is margin (growth {grow})");
                    }
                }
            }
            // and the whole box of the icon is the island's, so a grown icon is never half dead
            assert!(takes_pointer(&p, (icon[0] + 1.0) as i32, (icon[1] + 1.0) as i32), "the top left of the icon (growth {grow})");
            assert!(takes_pointer(&p, (icon[0] + icon[2] - 2.0) as i32, (icon[1] + 1.0) as i32), "the top right of the icon (growth {grow})");
        }
    }

    #[test]
    fn pieces_are_cut_to_the_window_and_bad_ones_dropped() {
        // a glass sticking out of the window is cut to it
        let p = parts([-10.0, -10.0, 50.0, 50.0], 8.0, &[], 100, 100);
        match p[0] {
            Part::Round(r) => assert_eq!((r.rect.left, r.rect.top, r.rect.right, r.rect.bottom), (0, 0, 41, 41)),
            _ => panic!("a rounded piece"),
        }
        // empty, negative, not a number, outside: nothing
        assert!(parts([0.0, 0.0, 0.0, 10.0], 8.0, &[], 100, 100).is_empty());
        assert!(parts([f32::NAN, 0.0, 10.0, 10.0], 8.0, &[[0.0, 0.0, f32::INFINITY, 4.0], [5.0, 5.0, -3.0, 3.0], [500.0, 500.0, 10.0, 10.0]], 100, 100).is_empty());
        assert!(parts(GLASS, 24.0, &[], 0, 0).is_empty());
        assert_eq!(bounds(&[]), None);
        assert!(!takes_pointer(&[], 5, 5));
        // a radius larger than the piece is a capsule, a radius that is not a number is square
        let capsule = parts([0.0, 0.0, 100.0, 40.0], 90.0, &[], 200, 200);
        assert!(matches!(capsule[0], Part::Round(RoundRect { radius: 20, .. })), "{capsule:?}");
        let square = parts([10.0, 10.0, 100.0, 40.0], f32::NAN, &[], 200, 200);
        assert!(matches!(square[0], Part::Round(RoundRect { radius: 0, .. })));
        assert!(takes_pointer(&square, 9, 9), "no rounding: the corner is taken");
    }

    #[test]
    fn a_side_island_in_a_small_window_is_all_glass_but_its_margin() {
        // the 48 px high capsule of a left island: window margins [12, 28, 28, 28]
        let p = parts([12.0, 28.0, 330.0, 48.0], 24.0, &[], 370, 104);
        assert!(takes_pointer(&p, 12 + 24, 28 + 24), "the middle of the left end");
        assert!(!takes_pointer(&p, 12, 28), "the capsule's corner");
        assert!(!takes_pointer(&p, 5, 52), "left of the glass, in the margin");
        assert!(!takes_pointer(&p, 100, 90), "below it");
        assert!(takes_pointer(&p, 100, 52));
    }

    #[test]
    fn a_non_window_is_refused_without_a_crash() {
        assert!(!set_region(0, &parts(GLASS, 24.0, &[], 856, 128)));
        assert!(!set_region(0, &[]));
    }
}
