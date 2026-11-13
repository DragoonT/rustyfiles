//! What the snip overlay does with the pointer and the keys, pure: the three modes (Bölge / Pencere / Tam ekran), the drag of the region
//! mode, which rectangle is shown undimmed and what a release or a click takes. The overlay windows only feed events in and draw what
//! [`Model::highlight`] says; every decision is here, where it is tested without a window.
//!
//! One model serves every monitor's window: positions are points of the virtual screen (see `geom::to_screen`), so a drag that begins on
//! one monitor and ends on another is one drag.

use rf_core::desktop::geom::Rect;

use super::geom::{self, Monitor, Pt, Win};

/// What a click or a drag takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    /// Bölge: a rectangle the user drags.
    #[default]
    Region,
    /// Pencere: the window under the pointer.
    Window,
    /// Tam ekran: the monitor under the pointer.
    Screen,
}

/// What an event decided.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Nothing is decided; the overlay stays.
    Pending,
    /// The overlay closes and nothing is copied (Escape, a right click, the close button).
    Cancel,
    /// The overlay closes and this rectangle of the virtual screen is copied.
    Take(Rect),
}

#[derive(Clone, Debug)]
pub struct Model {
    mode: Mode,
    monitors: Vec<Monitor>,
    /// Topmost first.
    windows: Vec<Win>,
    limit: Rect,
    pointer: Option<Pt>,
    /// Where the button went down in the region mode.
    drag_from: Option<Pt>,
}

impl Model {
    pub fn new(monitors: Vec<Monitor>, windows: Vec<Win>) -> Model {
        let limit = geom::virtual_screen(&monitors);
        Model { mode: Mode::Region, monitors, windows, limit, pointer: None, drag_from: None }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    pub fn limit(&self) -> Rect {
        self.limit
    }

    pub fn pointer(&self) -> Option<Pt> {
        self.pointer
    }

    /// A mode was chosen on the toolbar. A drag in progress is dropped: its rectangle belonged to the other mode.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.drag_from = None;
    }

    pub fn pointer_moved(&mut self, p: Pt) {
        self.pointer = Some(p);
    }

    /// The left button went down at `p`.
    pub fn press(&mut self, p: Pt) {
        self.pointer = Some(p);
        if self.mode == Mode::Region {
            self.drag_from = Some(p);
        }
    }

    /// The left button came up at `p`: a finished drag, or a click.
    pub fn release(&mut self, p: Pt) -> Outcome {
        self.pointer = Some(p);
        match self.mode {
            Mode::Region => match self.drag_from.take() {
                Some(from) => geom::region_of_drag(from, p, &self.limit).map_or(Outcome::Pending, Outcome::Take),
                None => Outcome::Pending,
            },
            Mode::Window => self.window_under(p).map_or(Outcome::Pending, Outcome::Take),
            Mode::Screen => self.screen_under(p).map_or(Outcome::Pending, Outcome::Take),
        }
    }

    /// Escape, a right click, the close button.
    pub fn cancel(&mut self) -> Outcome {
        self.drag_from = None;
        Outcome::Cancel
    }

    /// Whether a region is being dragged now.
    pub fn dragging(&self) -> bool {
        self.drag_from.is_some()
    }

    fn window_under(&self, p: Pt) -> Option<Rect> {
        let i = geom::window_at(p, &self.windows, &self.limit)?;
        geom::window_region(&self.windows[i], &self.limit)
    }

    fn screen_under(&self, p: Pt) -> Option<Rect> {
        geom::nearest_monitor(p, &self.monitors).map(|i| self.monitors[i].rect)
    }

    /// The rectangle that is drawn undimmed now (and whose size is printed): the dragged region, the window or the monitor under the
    /// pointer. `None` when there is none (the pointer has not moved yet, no window is under it, a drag that is still a click).
    pub fn highlight(&self) -> Option<Rect> {
        let p = self.pointer?;
        match self.mode {
            Mode::Region => {
                let from = self.drag_from?;
                geom::region_of_drag(from, p, &self.limit)
            }
            Mode::Window => self.window_under(p),
            Mode::Screen => self.screen_under(p),
        }
    }

    /// The size printed next to the highlight, in pixels.
    pub fn size_label(&self) -> Option<(i32, i32)> {
        self.highlight().map(|r| (r.width(), r.height()))
    }
}

/// Where the size label goes: its top-left in the virtual screen, for a label `label` pixels wide and high beside `target`. Below the
/// rectangle's left edge when it fits on the screen, else above, else inside the rectangle at its bottom-left (a rectangle that fills the
/// screen). Always kept on the screen.
pub fn label_place(target: Rect, label: (i32, i32), limit: &Rect) -> Pt {
    const GAP: i32 = 8;
    let (w, h) = label;
    let x = target.left.min(limit.right - w).max(limit.left);
    let below = target.bottom + GAP;
    let above = target.top - GAP - h;
    let y = if below + h <= limit.bottom {
        below
    } else if above >= limit.top {
        above
    } else {
        (target.bottom - GAP - h).max(limit.top)
    };
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitors() -> Vec<Monitor> {
        vec![Monitor { rect: Rect::new(0, 0, 1920, 1080), scale: 1.0 }, Monitor { rect: Rect::new(-1440, -560, 0, 2000), scale: 1.5 }]
    }

    fn windows() -> Vec<Win> {
        vec![Win { hwnd: 11, rect: Rect::new(600, 300, 900, 500) }, Win { hwnd: 22, rect: Rect::new(100, 100, 1500, 900) }]
    }

    fn model() -> Model {
        Model::new(monitors(), windows())
    }

    #[test]
    fn the_region_mode_is_the_default() {
        assert_eq!(model().mode(), Mode::Region);
        assert_eq!(model().highlight(), None);
    }

    #[test]
    fn a_drag_shows_its_rectangle_while_it_goes_and_a_release_takes_it() {
        let mut m = model();
        m.pointer_moved((10, 10));
        assert_eq!(m.highlight(), None, "no button yet: only the cross");
        m.press((10, 10));
        assert!(m.dragging());
        assert_eq!(m.highlight(), None, "still a click");
        m.pointer_moved((50, 40));
        assert_eq!(m.highlight(), Some(Rect::new(10, 10, 50, 40)));
        assert_eq!(m.size_label(), Some((40, 30)));
        m.pointer_moved((110, 60));
        assert_eq!(m.release((110, 60)), Outcome::Take(Rect::new(10, 10, 110, 60)));
        assert!(!m.dragging());
    }

    #[test]
    fn a_click_without_a_drag_takes_nothing_and_the_overlay_stays() {
        let mut m = model();
        m.press((300, 300));
        assert_eq!(m.release((301, 302)), Outcome::Pending);
        assert_eq!(m.highlight(), None);
        // and a new press starts a new drag
        m.press((310, 310));
        assert_eq!(m.release((410, 360)), Outcome::Take(Rect::new(310, 310, 410, 360)));
    }

    #[test]
    fn a_release_without_a_press_in_this_overlay_is_nothing() {
        // (the press went to the toolbar, say, and the button came up on the dimmed picture)
        let mut m = model();
        assert_eq!(m.release((100, 100)), Outcome::Pending);
    }

    #[test]
    fn a_drag_from_one_monitor_to_another_is_one_region_in_device_pixels() {
        let mut m = model();
        let a = geom::to_screen(&m.monitors()[1].clone(), (900.0, 300.0));
        let b = geom::to_screen(&m.monitors()[0].clone(), (400.0, 200.0));
        m.press(a);
        m.pointer_moved(b);
        assert_eq!(m.size_label(), Some((490, 310)));
        assert_eq!(m.release(b), Outcome::Take(Rect::new(-90, -110, 400, 200)));
    }

    #[test]
    fn a_drag_that_runs_off_the_screen_is_cut_to_it() {
        let mut m = model();
        m.press((1800, 900));
        assert_eq!(m.release((2500, 1500)), Outcome::Take(Rect::new(1800, 900, 1920, 1500)));
    }

    #[test]
    fn escape_and_the_right_click_cancel_also_in_the_middle_of_a_drag() {
        let mut m = model();
        m.press((10, 10));
        m.pointer_moved((200, 200));
        assert_eq!(m.cancel(), Outcome::Cancel);
        assert!(!m.dragging());
        assert_eq!(m.highlight(), None);
    }

    #[test]
    fn the_window_mode_outlines_the_window_under_the_pointer_and_a_click_takes_it() {
        let mut m = model();
        m.set_mode(Mode::Window);
        m.pointer_moved((700, 400));
        assert_eq!(m.highlight(), Some(Rect::new(600, 300, 900, 500)), "the dialog is over the editor");
        m.pointer_moved((200, 200));
        assert_eq!(m.highlight(), Some(Rect::new(100, 100, 1500, 900)));
        m.press((200, 200));
        assert!(!m.dragging(), "no drag in the window mode");
        assert_eq!(m.release((200, 200)), Outcome::Take(Rect::new(100, 100, 1500, 900)));
        // over no window: nothing is outlined, a click takes nothing
        m.pointer_moved((1700, 1000));
        assert_eq!(m.highlight(), None);
        assert_eq!(m.release((1700, 1000)), Outcome::Pending);
    }

    #[test]
    fn the_whole_screen_mode_takes_the_monitor_under_the_pointer() {
        let mut m = model();
        m.set_mode(Mode::Screen);
        m.pointer_moved((-300, 100));
        assert_eq!(m.highlight(), Some(Rect::new(-1440, -560, 0, 2000)));
        assert_eq!(m.size_label(), Some((1440, 2560)));
        m.pointer_moved((1000, 500));
        assert_eq!(m.release((1000, 500)), Outcome::Take(Rect::new(0, 0, 1920, 1080)));
        // a point in the gap below the primary monitor belongs to the nearest one
        assert_eq!(m.release((500, 1500)), Outcome::Take(Rect::new(0, 0, 1920, 1080)));
    }

    #[test]
    fn changing_the_mode_drops_the_drag_that_was_going() {
        let mut m = model();
        m.press((10, 10));
        m.pointer_moved((300, 300));
        assert!(m.highlight().is_some());
        m.set_mode(Mode::Window);
        assert!(!m.dragging());
        m.set_mode(Mode::Region);
        assert_eq!(m.highlight(), None);
        assert_eq!(m.release((300, 300)), Outcome::Pending);
    }

    #[test]
    fn the_size_label_goes_below_the_rectangle_else_above_else_inside() {
        let limit = Rect::new(0, 0, 1920, 1080);
        // room below
        assert_eq!(label_place(Rect::new(100, 100, 300, 200), (80, 24), &limit), (100, 208));
        // no room below, room above
        assert_eq!(label_place(Rect::new(100, 900, 300, 1070), (80, 24), &limit), (100, 868));
        // a rectangle that fills the screen: inside, at the bottom left
        assert_eq!(label_place(limit, (80, 24), &limit), (0, 1048));
        // a label near the right edge is pulled in
        assert_eq!(label_place(Rect::new(1900, 100, 1920, 200), (80, 24), &limit), (1840, 208));
        // on a monitor with a negative origin
        let neg = Rect::new(-1440, -560, 1920, 2000);
        assert_eq!(label_place(Rect::new(-1440, -560, -1240, -400), (80, 24), &neg), (-1440, -392));
    }
}
