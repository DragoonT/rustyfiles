//! Which point of an island is its EMPTY part: the glass where there is no icon, field, symbol or other control. A right click there
//! starts Windows' Task Manager, as a right click on the empty part of Windows' taskbar offers it. Pure: the glass, the boxes of the
//! controls and the point are arguments, all in one coordinate space (dock units or device px, from the glass's own top left corner;
//! the caller keeps them in the same one), so the decision is worked out from the layout's rectangles and never from what the toolkit
//! happens to have under the pointer.
//!
//! A control that is only shown (the weather, a thin separator, the running mark under an icon) is not a control: a right click on it
//! is a right click on the glass.

/// A rectangle as the layout gives it: x, y, width, height.
pub type Box4 = [f32; 4];

/// Whether `point` lies inside `r` (the left and top edges belong to it, the right and bottom ones do not).
pub fn contains(r: Box4, point: (f32, f32)) -> bool {
    point.0 >= r[0] && point.0 < r[0] + r[2] && point.1 >= r[1] && point.1 < r[1] + r[3]
}

/// Whether `point` lies inside the glass, a rounded rectangle (`radius` is clamped to half the short side, a capsule at most).
pub fn in_glass(glass: Box4, radius: f32, point: (f32, f32)) -> bool {
    if !contains(glass, point) {
        return false;
    }
    let r = if radius.is_finite() { radius.clamp(0.0, glass[2].min(glass[3]) / 2.0) } else { 0.0 };
    let (cx, cy) = (point.0.clamp(glass[0] + r, glass[0] + glass[2] - r), point.1.clamp(glass[1] + r, glass[1] + glass[3] - r));
    let (dx, dy) = (point.0 - cx, point.1 - cy);
    dx * dx + dy * dy <= r * r
}

/// Whether `point` is the empty part of the island: on its glass and on none of `controls`.
pub fn is_empty(glass: Box4, radius: f32, controls: &[Box4], point: (f32, f32)) -> bool {
    in_glass(glass, radius, point) && !controls.iter().any(|&c| contains(c, point))
}

#[cfg(test)]
mod tests {
    use super::*;

    // a 330 x 48 capsule with a logo and a search field in it (the left island at the default size)
    const GLASS: Box4 = [0.0, 0.0, 330.0, 48.0];
    const LOGO: Box4 = [8.0, 8.0, 32.0, 32.0];
    const SEARCH: Box4 = [48.0, 8.0, 220.0, 32.0];

    #[test]
    fn a_point_on_a_control_is_not_empty() {
        for p in [(8.0, 8.0), (24.0, 24.0), (39.9, 39.9), (48.0, 8.0), (150.0, 24.0), (267.9, 39.9)] {
            assert!(!is_empty(GLASS, 24.0, &[LOGO, SEARCH], p), "{p:?} is on a control");
        }
    }

    #[test]
    fn the_glass_between_and_around_the_controls_is_empty() {
        // between the logo and the field, right of the field, above and below them, the padding at the left
        for p in [(44.0, 24.0), (300.0, 24.0), (100.0, 3.0), (100.0, 45.0), (3.0, 24.0), (24.0, 44.0)] {
            assert!(is_empty(GLASS, 24.0, &[LOGO, SEARCH], p), "{p:?} is empty glass");
        }
        // the edges that belong to the next thing: the control's right and bottom edges are outside it
        assert!(is_empty(GLASS, 24.0, &[LOGO], (40.0, 24.0)));
        assert!(is_empty(GLASS, 24.0, &[LOGO], (24.0, 40.0)));
    }

    #[test]
    fn outside_the_glass_is_never_empty_glass() {
        for p in [(-1.0, 24.0), (330.0, 24.0), (100.0, -0.5), (100.0, 48.0), (f32::NAN, 5.0)] {
            assert!(!is_empty(GLASS, 24.0, &[], p), "{p:?} is not on the glass");
        }
        // the capsule's corners are not glass
        assert!(!is_empty(GLASS, 24.0, &[], (1.0, 1.0)));
        assert!(!is_empty(GLASS, 24.0, &[], (329.0, 47.0)));
        assert!(is_empty(GLASS, 24.0, &[], (24.0, 1.0)));
        assert!(is_empty(GLASS, 24.0, &[], (306.0, 24.0)));
    }

    #[test]
    fn a_radius_that_is_not_a_number_is_a_square_and_a_huge_one_a_capsule() {
        assert!(is_empty(GLASS, f32::NAN, &[], (0.5, 0.5)));
        assert!(!is_empty(GLASS, 1000.0, &[], (1.0, 1.0)));
        assert!(is_empty(GLASS, 1000.0, &[], (165.0, 24.0)));
    }

    #[test]
    fn no_controls_means_all_of_the_glass_is_empty() {
        assert!(is_empty(GLASS, 24.0, &[], (165.0, 24.0)));
    }

    #[test]
    fn an_icon_that_has_grown_out_of_the_glass_is_a_control_over_the_glass_too() {
        // the middle island: glass 72 high, an icon grown to 68 px standing 12 px above the bottom reaches 8 px over the glass's top edge
        let glass: Box4 = [0.0, 10.0, 600.0, 72.0];
        let grown: Box4 = [100.0, 2.0, 68.2, 68.2];
        assert!(!is_empty(glass, 24.0, &[grown], (130.0, 5.0)), "over the glass, on the icon: not on the glass at all and not empty");
        assert!(!is_empty(glass, 24.0, &[grown], (130.0, 40.0)), "on the glass but under the icon");
        assert!(is_empty(glass, 24.0, &[grown], (180.0, 40.0)), "beside it");
        assert!(!is_empty(glass, 24.0, &[], (130.0, 5.0)), "above the glass, nothing is the island's");
    }
}
