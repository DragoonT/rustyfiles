//! The thin dock: while a maximised window is on the dock's monitor the middle island drops to the height of the side islands, so the
//! screen gives that room to the window, and grows back to its full size when the pointer comes near. Pure rules, no window and no
//! clock of their own: the app feeds them facts and the time, and every rule has a test.
//!
//! WHAT IS THIN. The middle island's GLASS is `SIDE_HEIGHT` high (the same line as the other two: all three bottoms are the same
//! `MARGIN_BOTTOM` above the work area's bottom) and its icons are `THIN_ICON` instead of `ICON_REST`. The icons keep their CENTRES,
//! so the island is exactly as wide as it was and nothing beside it moves; only the glass lowers its top and the icons shrink in place.
//! That is also what keeps "the icon the user aims at stays under the pointer" true while the dock grows: horizontally nothing moves, and
//! the icons grow up from the same baseline the magnification uses.
//!
//! THE FIVE RULES (each is a type below):
//! 1. [`Follower`]: thin or full from (setting, a maximised window seen, pointer near, something open). Going back to thin waits
//!    [`LEAVE_MS`] after the pointer has left and nothing is open.
//! 2. [`BandSettle`]: the band the dock reserves is the thin one while a maximised window has been there for [`SETTLE_MS`], and it
//!    changes at most once in [`HOLD_MS`], never while a panel is open (a change of the band closes panels). It does NOT depend on the
//!    pointer: the dock grown under the pointer floats over the window and the work area stays where it is.
//! 3. [`Zone`]: what "near" is.
//! 4. [`counts_as_maximized`]: which windows make the dock thin.
//! 5. [`Squeeze`]: the numbers of every moment between thin and full, and [`glass_height_px`] for the device pixels.

use super::layout::{ICON_REST, MARGIN_BOTTOM, MIDDLE_HEIGHT, MIDDLE_RESERVE, Rect, SIDE_HEIGHT, px};

/// The thin dock's glass height: the side islands', so the three stand on one line and are one height.
pub const THIN_HEIGHT: f32 = SIDE_HEIGHT;
/// An icon of the thin dock, dock units. With the running mark below it (the same 7 and 4 as at full size) it fills the 48 units the
/// glass has: 12 under the icon's foot, 30 of icon, 6 over its head (`the_icon_and_its_mark_fit_the_thin_glass`).
pub const THIN_ICON: f32 = 30.0;
/// What the application desktop toolbar reserves while the dock is thin: the thin glass and the margin above and below it, as the full
/// band is the full glass and its margins (`layout::RESERVED_HEIGHT`).
pub const THIN_RESERVED_HEIGHT: f32 = THIN_HEIGHT + 2.0 * MARGIN_BOTTOM;
/// How far above the thin dock's band the pointer still counts as near, dock units.
pub const NEAR_ABOVE: f32 = 24.0;
/// How far to each side of the thin glass the pointer still counts as near: the room the grown glass may widen into, so the whole of a
/// widened glass is inside the zone.
pub const NEAR_SIDE: f32 = MIDDLE_RESERVE;
/// After the pointer has left (and nothing is open) the dock waits this long before it goes thin again.
pub const LEAVE_MS: u64 = 400;
/// A maximised window must have been there this long before the band becomes the thin one (and gone this long before it becomes the full
/// one again): a window maximised and restored in quick succession changes nothing for the other windows.
pub const SETTLE_MS: u64 = 600;
/// The band changes at most once in this time.
pub const HOLD_MS: u64 = 2000;

/// Windows' own taskbar, logical px of the monitor's scale: the strip a hidden taskbar still holds.
pub const TASKBAR_STRIP: f32 = 48.0;

/// The band's thickness in device px for the dock unit, on a monitor of this `scale`.
///
/// The thin band is never thinner than Windows' taskbar strip and two px: while the host holds the edge of a hidden taskbar, the
/// work area is judged by that strip (`rf_shellguard::tbwork::plan_edge`), and a band no thicker than it is not recognised as ours.
/// With the margins at 6 units the thin band is 60 units, which at the smallest dock size (0.75) is 45 px beside a 48 px strip; there
/// the band keeps the few px more above the thin glass. At every larger size the floor does nothing.
pub fn reserved_thickness(unit: f32, thin: bool, scale: f32) -> i32 {
    if thin { px(THIN_RESERVED_HEIGHT, unit).max(px(TASKBAR_STRIP, scale) + 2) } else { super::layout::reserved_thickness(unit) }
}

// ---------------------------------------------------------------- the numbers between thin and full

/// How thin the middle island is at one moment: `grow` 0 is the thin dock, 1 the full one (today's, bit for bit).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Squeeze {
    pub grow: f32,
    /// The glass's height, dock units.
    pub glass_h: f32,
    /// What an icon's rest size is multiplied by (the icon keeps its centre).
    pub icon: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Squeeze {
    /// The full dock.
    pub const FULL: Squeeze = Squeeze { grow: 1.0, glass_h: MIDDLE_HEIGHT, icon: 1.0 };

    pub fn at(grow: f32) -> Squeeze {
        let g = grow.clamp(0.0, 1.0);
        if g >= 1.0 {
            return Squeeze::FULL;
        }
        Squeeze { grow: g, glass_h: lerp(THIN_HEIGHT, MIDDLE_HEIGHT, g), icon: lerp(THIN_ICON / ICON_REST, 1.0, g) }
    }

    pub fn is_full(&self) -> bool {
        self.grow >= 1.0
    }

    /// The height of the magnification's bell: it is nothing while the dock is thin ("no magnification bell at rest") and comes in with
    /// the growth.
    pub fn bell(&self, presence: f32) -> f32 {
        presence * self.grow
    }
}

/// The glass's height in device px: the full one as given (`full_px`, the layout's), and the thin one exactly what the side islands'
/// glass is at this unit (`px(SIDE_HEIGHT, unit)`, rounded the way the layout rounds it, so all three are the same number of pixels).
pub fn glass_height_px(sq: &Squeeze, unit: f32, full_px: f32) -> f32 {
    if sq.is_full() {
        return full_px;
    }
    lerp(px(THIN_HEIGHT, unit) as f32, full_px, sq.grow)
}

/// The glass rectangle of the thin dock: the full one's bottom edge, the side islands' height.
pub fn thin_glass(full: Rect, unit: f32) -> Rect {
    Rect { top: full.bottom - px(THIN_HEIGHT, unit), ..full }
}

// ---------------------------------------------------------------- near

/// The area in which the pointer makes the thin dock grow: the thin glass and the band around it, `NEAR_SIDE` to each side, from
/// `NEAR_ABOVE` above the band's top edge down to the bottom of the monitor, less the two side islands (the pointer on the Start button
/// or the clock is not on the dock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Zone {
    pub near: Rect,
    pub left: Rect,
    pub right: Rect,
}

impl Zone {
    pub fn contains(&self, x: i32, y: i32) -> bool {
        self.near.contains(x, y) && !self.left.contains(x, y) && !self.right.contains(x, y)
    }
}

/// `full` is the middle island's glass at its full height (the layout's), `left` and `right` the side islands' glass.
pub fn zone(full: Rect, left: Rect, right: Rect, monitor: Rect, unit: f32) -> Zone {
    let thin = thin_glass(full, unit);
    let side = px(NEAR_SIDE, unit);
    let near = Rect {
        left: (thin.left - side).max(monitor.left),
        right: (thin.right + side).min(monitor.right),
        // the band's own margin above the thin glass is part of the band; the 24 units come on top of it
        top: (thin.top - px(MARGIN_BOTTOM + NEAR_ABOVE, unit)).max(monitor.top),
        bottom: monitor.bottom,
    };
    Zone { near, left, right }
}

// ---------------------------------------------------------------- thin or full

/// What the choice between thin and full depends on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// The setting "Pencere büyütüldüğünde dock'u incelt".
    pub enabled: bool,
    /// A maximised window the user can see is on the dock's monitor.
    pub maximized: bool,
    /// The pointer is in the [`Zone`] (or on the dock itself), or a mouse button is held near it.
    pub near: bool,
    /// A panel, a menu or a hover preview of the dock is open, or a press is on the dock.
    pub open: bool,
}

/// The answer of [`Follower::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verdict {
    /// The dock should be thin (false: full, grown or never made thin).
    pub thin: bool,
    /// When nothing else happens, ask again at this time (ms on the caller's clock): the end of the wait before going thin.
    pub wake_at: Option<u64>,
}

/// Remembers since when the pointer has been away, so that the dock goes thin [`LEAVE_MS`] after it left and not at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Follower {
    thin: bool,
    away_since: Option<u64>,
}

impl Follower {
    pub fn new() -> Follower {
        Follower::default()
    }

    pub fn thin(&self) -> bool {
        self.thin
    }

    /// One look at the facts at `now` (ms, any clock that only goes forward).
    pub fn step(&mut self, now: u64, i: Inputs) -> Verdict {
        // no setting or no maximised window: the dock of today, at once (a window restored gives the room back as it goes)
        if !i.enabled || !i.maximized {
            self.thin = false;
            self.away_since = None;
            return Verdict { thin: false, wake_at: None };
        }
        // near or something open: grown, and the wait starts again from the moment both are over
        if i.near || i.open {
            self.thin = false;
            self.away_since = None;
            return Verdict { thin: false, wake_at: None };
        }
        if self.thin {
            return Verdict { thin: true, wake_at: None };
        }
        let since = *self.away_since.get_or_insert(now);
        if now.saturating_sub(since) >= LEAVE_MS {
            self.thin = true;
            self.away_since = None;
            Verdict { thin: true, wake_at: None }
        } else {
            Verdict { thin: false, wake_at: Some(since + LEAVE_MS) }
        }
    }
}

// ---------------------------------------------------------------- the band

/// Decides when the reserved band changes between the full and the thin thickness.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BandSettle {
    /// The band now in force is the thin one.
    applied: bool,
    /// What was wanted at the last look, and since when.
    wanted: bool,
    wanted_since: u64,
    last_change: Option<u64>,
}

/// The answer of [`BandSettle::step`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    /// The thin band is the one in force.
    pub thin: bool,
    /// It changed with this look: set the band.
    pub changed: bool,
    pub wake_at: Option<u64>,
}

impl BandSettle {
    pub fn new() -> BandSettle {
        BandSettle::default()
    }

    pub fn thin(&self) -> bool {
        self.applied
    }

    /// `want_thin`: the setting is on and a maximised window is seen. `blocked`: something is open that a change of the band would close.
    /// The pointer is not an input: the band stays while the dock is grown under it.
    pub fn step(&mut self, now: u64, want_thin: bool, blocked: bool) -> Band {
        if want_thin != self.wanted {
            self.wanted = want_thin;
            self.wanted_since = now;
        }
        if self.wanted == self.applied {
            return Band { thin: self.applied, changed: false, wake_at: None };
        }
        let settled_at = self.wanted_since + SETTLE_MS;
        let allowed_at = self.last_change.map_or(0, |t| t + HOLD_MS);
        let due = settled_at.max(allowed_at);
        if now >= due && !blocked {
            self.applied = self.wanted;
            self.last_change = Some(now);
            return Band { thin: self.applied, changed: true, wake_at: None };
        }
        // (blocked: the caller looks again when the panel is gone; it has its own look every time something changes)
        Band { thin: self.applied, changed: false, wake_at: (!blocked).then_some(due) }
    }

    /// The setting was switched off: the full band is back now, whatever the hold says.
    pub fn reset(&mut self, now: u64) -> bool {
        let was = self.applied;
        self.applied = false;
        self.wanted = false;
        self.wanted_since = now;
        was
    }
}

// ---------------------------------------------------------------- the two rules together

/// The facts and the two rules ([`Follower`], [`BandSettle`]) as one piece, so that the sequences the app's wiring goes through (a
/// window restored while a menu is open, the setting switched, the poller that must keep running) can be tested without a window and
/// with a clock of the test's own. The app owns one and does nothing else but feed it and act on [`Look`].
#[derive(Clone, Copy, Debug, Default)]
pub struct Model {
    pub inputs: Inputs,
    follower: Follower,
    band: BandSettle,
}

/// What [`Model::look`] answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// The middle island should be thin.
    pub thin: bool,
    /// The band: `changed` says it must be set now.
    pub band: Band,
    /// When nothing else happens, look again at this time (ms): the earliest deadline of the two rules.
    pub wake_at: Option<u64>,
}

impl Model {
    pub fn new() -> Model {
        Model::default()
    }

    /// One look at the facts as they are at `now` (ms, a clock that only goes forward).
    pub fn look(&mut self, now: u64) -> Look {
        let i = self.inputs;
        let verdict = self.follower.step(now, i);
        // the band follows the window, not the pointer, and waits while something is open (a change of the band closes panels)
        let band = self.band.step(now, i.enabled && i.maximized, i.open);
        Look { thin: verdict.thin, band, wake_at: [verdict.wake_at, band.wake_at].into_iter().flatten().min() }
    }

    /// The band in force is the thin one.
    pub fn band_thin(&self) -> bool {
        self.band.thin()
    }

    /// Whether the cursor and the open panels must be looked at on a timer: while a maximised window is seen, AND while the band is
    /// still thin although the window is gone. The second half matters: a window can go while a menu is open, and the band's change
    /// is then held back with nothing to wake it - only the look at "is something still open" can (the band would stay thin, with
    /// the work area 24 units too tall, until the next maximise).
    pub fn needs_polling(&self) -> bool {
        self.inputs.enabled && (self.inputs.maximized || self.band.thin())
    }

    /// One look of the poller at the cursor and at what is open. `inside`: the cursor is in the [`Zone`] (`None`: the zone is not
    /// known at the moment, so the pointer is neither said to be near nor to have left); `button`: a mouse button is held. Returns
    /// whether `near` or `open` changed, which asks for a [`Model::look`].
    pub fn observe(&mut self, inside: Option<bool>, button: bool, open: bool) -> bool {
        let near = match inside {
            _ if !self.inputs.maximized => false,
            // (a press that began on the dock and slipped out of the zone: still near)
            Some(inside) => inside || (button && self.inputs.near),
            None => self.inputs.near,
        };
        let changed = near != self.inputs.near || open != self.inputs.open;
        self.inputs.near = near;
        self.inputs.open = open;
        changed
    }

    /// The setting was switched off: the dock of today, at once, and the full band back whatever the hold says. True: the band was
    /// the thin one, so it must be set again.
    pub fn switch_off(&mut self, now: u64) -> bool {
        self.inputs = Inputs { enabled: false, ..Inputs::default() };
        self.follower = Follower::new();
        self.band.reset(now)
    }
}

// ---------------------------------------------------------------- which windows count

/// What is known about one top-level window.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WinFacts {
    pub visible: bool,
    pub minimized: bool,
    /// Hidden by DWM: on another virtual desktop, a suspended app.
    pub cloaked: bool,
    pub zoomed: bool,
    pub has_owner: bool,
    pub ex_style: u32,
    /// A window of the dock's own process (its islands, its panels, the desktop).
    pub own: bool,
    /// A piece of the shell (the desktop, the taskbar, the Windows Start menu's host): never a window the user maximised
    /// (`fullscreen::is_shell_class`).
    pub shell: bool,
    /// On the monitor the dock is on.
    pub on_dock_monitor: bool,
}

/// Whether this window is a maximised window the user can see on the dock's monitor: the windows a taskbar button belongs to, by the
/// same style rules as the task list (`tasklist::style_is_task`), visible, not minimised, not cloaked, not ours, not the shell's.
pub fn counts_as_maximized(f: &WinFacts) -> bool {
    f.zoomed && f.visible && !f.minimized && !f.cloaked && !f.own && !f.shell && f.on_dock_monitor && crate::tasklist::style_is_task(f.has_owner, f.ex_style)
}

/// Whether any of these windows is such a window.
pub fn any_maximized(windows: impl IntoIterator<Item = WinFacts>) -> bool {
    windows.into_iter().any(|f| counts_as_maximized(&f))
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::layout::{self, Widths};

    fn on(maximized: bool, near: bool, open: bool) -> Inputs {
        Inputs { enabled: true, maximized, near, open }
    }

    const W: Widths = Widths { left: 330.0, middle: 600.0, right: 280.0 };

    // ---- thin or full

    #[test]
    fn without_the_setting_or_a_maximised_window_the_dock_is_the_dock_of_today() {
        let mut f = Follower::new();
        for (now, i) in [
            (0, Inputs { enabled: false, maximized: true, near: false, open: false }),
            (10_000, Inputs { enabled: false, maximized: true, near: false, open: false }),
            (20_000, on(false, false, false)),
            (30_000, on(false, true, true)),
        ] {
            assert_eq!(f.step(now, i), Verdict { thin: false, wake_at: None }, "{i:?}");
        }
    }

    #[test]
    fn a_maximised_window_with_the_pointer_far_makes_the_dock_thin_after_the_wait() {
        let mut f = Follower::new();
        // a window has just been maximised: the dock is full, and thin 400 ms later
        assert_eq!(f.step(1_000, on(true, false, false)), Verdict { thin: false, wake_at: Some(1_000 + LEAVE_MS) });
        assert_eq!(f.step(1_399, on(true, false, false)), Verdict { thin: false, wake_at: Some(1_400) });
        assert_eq!(f.step(1_400, on(true, false, false)), Verdict { thin: true, wake_at: None });
        // and it stays thin as long as nothing changes
        assert_eq!(f.step(60_000, on(true, false, false)), Verdict { thin: true, wake_at: None });
    }

    #[test]
    fn the_pointer_near_or_something_open_grows_it_at_once() {
        let mut f = Follower::new();
        f.step(0, on(true, false, false));
        f.step(500, on(true, false, false));
        assert!(f.thin());
        assert!(!f.step(600, on(true, true, false)).thin, "the pointer came near");
        assert!(!f.step(700, on(true, false, true)).thin, "a panel is open and the pointer is away");
        // ... and a tray overflow panel, the Start panel by the keyboard: the same input
        assert!(!f.step(800, on(true, false, true)).thin);
    }

    #[test]
    fn it_goes_thin_again_only_400_ms_after_the_pointer_left_and_the_last_thing_closed() {
        let mut f = Follower::new();
        f.step(0, on(true, true, false));
        // the pointer leaves at 1000: the wait runs from there
        assert_eq!(f.step(1_000, on(true, false, false)), Verdict { thin: false, wake_at: Some(1_400) });
        assert!(!f.step(1_399, on(true, false, false)).thin);
        assert!(f.step(1_400, on(true, false, false)).thin);
        // grown again, a panel stays open after the pointer left: the wait does not run while it is open
        f.step(2_000, on(true, true, false));
        assert!(!f.step(3_000, on(true, false, true)).thin);
        assert!(!f.step(9_000, on(true, false, true)).thin, "open for a long time");
        // it closes at 9000: 400 ms from then, not from when the pointer left
        assert_eq!(f.step(9_000, on(true, false, false)).wake_at, Some(9_400));
        assert!(f.step(9_400, on(true, false, false)).thin);
    }

    #[test]
    fn a_pointer_that_comes_back_during_the_wait_starts_the_wait_again() {
        let mut f = Follower::new();
        f.step(0, on(true, true, false));
        f.step(1_000, on(true, false, false)); // left
        f.step(1_300, on(true, true, false)); // back, 100 ms short of thin
        // left again at 1500: 400 ms from here, not from 1000
        assert_eq!(f.step(1_500, on(true, false, false)).wake_at, Some(1_900));
        assert!(!f.step(1_800, on(true, false, false)).thin);
        assert!(f.step(1_900, on(true, false, false)).thin);
    }

    #[test]
    fn the_window_restored_while_the_dock_is_thin_gives_the_full_dock_back_at_once() {
        let mut f = Follower::new();
        f.step(0, on(true, false, false));
        f.step(400, on(true, false, false));
        assert!(f.thin());
        assert_eq!(f.step(401, on(false, false, false)), Verdict { thin: false, wake_at: None });
        // and switching the setting off does the same
        f.step(500, on(true, false, false));
        f.step(1_000, on(true, false, false));
        assert!(f.thin());
        assert!(!f.step(1_001, Inputs { enabled: false, ..on(true, false, false) }).thin);
    }

    // ---- all of it together

    /// A short story played through both rules, the way the app's look at the facts does it (every 50 ms, plus the deadlines): a window
    /// is maximised, the pointer comes and goes, a panel opens, the window is restored, and a window is maximised and restored in a
    /// flicker. What the user sees (thin or full) and what the other windows see (the band) are told apart.
    #[test]
    fn a_story_of_a_session() {
        struct Fact {
            at: u64,
            maximized: bool,
            near: bool,
            open: bool,
        }
        let story = [
            Fact { at: 0, maximized: false, near: false, open: false },
            Fact { at: 1_000, maximized: true, near: false, open: false },   // a window is maximised; the pointer is far
            Fact { at: 5_000, maximized: true, near: true, open: false },    // the pointer comes to the dock
            Fact { at: 7_000, maximized: true, near: false, open: false },   // and goes
            Fact { at: 9_000, maximized: true, near: false, open: true },    // the Start panel is opened by the keyboard
            Fact { at: 10_000, maximized: true, near: false, open: false },  // and closed
            Fact { at: 15_000, maximized: false, near: false, open: false }, // the window is restored
            Fact { at: 16_000, maximized: true, near: false, open: false },  // a flicker: maximised ...
            Fact { at: 16_200, maximized: false, near: false, open: false }, // ... restored
            Fact { at: 16_300, maximized: true, near: false, open: false },  // ... maximised
            Fact { at: 16_500, maximized: false, near: false, open: false }, // ... restored
        ];
        let mut m = Model::new();
        m.inputs.enabled = true;
        let mut fact = &story[0];
        let (mut looks, mut band_changes) = (Vec::new(), Vec::new());
        let mut last_thin = false;
        for now in (0..30_000u64).step_by(50) {
            if let Some(next) = story.iter().find(|s| s.at == now) {
                fact = next;
            }
            m.inputs = Inputs { enabled: true, maximized: fact.maximized, near: fact.near, open: fact.open };
            let l = m.look(now);
            if l.band.changed {
                band_changes.push((now, l.band.thin));
            }
            if l.thin != last_thin {
                looks.push((now, l.thin));
                last_thin = l.thin;
            }
            if fact.near || fact.open {
                assert!(!l.thin, "grown at {now}");
            }
        }
        // thin 400 ms after the window came (1000 + 400), grown at 5000, thin again 400 ms after 7000, grown while the panel is open
        // (9000 to 10000), thin 400 ms after it closed, full when the window is restored at 15000 (the flicker never gets thin)
        assert_eq!(looks, [(1_400, true), (5_000, false), (7_400, true), (9_000, false), (10_400, true), (15_000, false)]);
        // the band: thin 600 ms after the window came, full 600 ms after it was restored (the hold of 2 s is long over by then); the
        // pointer and the panel's coming and going changed nothing, and the flicker changed nothing
        assert_eq!(band_changes, [(1_600, true), (15_600, false)]);
    }

    // ---- the band

    #[test]
    fn the_band_becomes_thin_after_the_settle_time_and_not_before() {
        let mut b = BandSettle::new();
        assert_eq!(b.step(1_000, true, false), Band { thin: false, changed: false, wake_at: Some(1_000 + SETTLE_MS) });
        assert!(!b.step(1_599, true, false).changed);
        assert_eq!(b.step(1_600, true, false), Band { thin: true, changed: true, wake_at: None });
        assert!(b.thin());
        // nothing more while the wish is the same
        assert_eq!(b.step(9_000, true, false), Band { thin: true, changed: false, wake_at: None });
    }

    #[test]
    fn a_window_maximised_and_restored_in_quick_succession_changes_nothing() {
        let mut b = BandSettle::new();
        let mut t = 0;
        for _ in 0..20 {
            // maximised, restored 200 ms later, maximised again 100 ms after that ...
            assert!(!b.step(t, true, false).changed);
            t += 200;
            assert!(!b.step(t, false, false).changed);
            t += 100;
        }
        assert!(!b.thin(), "the settle time was never reached");
    }

    #[test]
    fn the_band_changes_at_most_once_in_the_hold_time() {
        let mut b = BandSettle::new();
        b.step(0, true, false);
        assert!(b.step(SETTLE_MS, true, false).changed);
        let at = SETTLE_MS;
        // restored right after: the wish is settled at at + 600, but the hold keeps it until at + 2000
        let r = b.step(at + 100, false, false);
        assert_eq!(r, Band { thin: true, changed: false, wake_at: Some(at + HOLD_MS) });
        assert!(!b.step(at + HOLD_MS - 1, false, false).changed);
        assert_eq!(b.step(at + HOLD_MS, false, false), Band { thin: false, changed: true, wake_at: None });
        // the next change must wait again
        b.step(at + HOLD_MS + 10, true, false);
        assert!(!b.step(at + HOLD_MS + 10 + SETTLE_MS, true, false).changed, "the hold runs from the last change");
        assert!(b.step(at + 2 * HOLD_MS, true, false).changed);
    }

    #[test]
    fn the_band_does_not_change_while_something_is_open_and_changes_when_it_has_closed() {
        let mut b = BandSettle::new();
        b.step(0, true, false);
        // settled, but a panel is open: a change of the band closes panels
        assert_eq!(b.step(5_000, true, true), Band { thin: false, changed: false, wake_at: None });
        assert!(!b.step(60_000, true, true).changed);
        assert!(b.step(60_001, true, false).changed);
    }

    #[test]
    fn the_band_does_not_follow_the_pointer() {
        // the pointer is an input of the Model: the dock grows and shrinks under it, and every look says the same band
        let mut m = Model::new();
        m.inputs = on(true, false, false);
        m.look(0);
        assert!(m.look(SETTLE_MS).band.changed);
        let thickness = reserved_thickness(1.0, m.band_thin(), 1.0);
        assert_eq!(thickness, 60);
        let mut grown = 0;
        for (t, near) in [(1_000, false), (1_500, true), (2_000, false), (2_500, true), (3_000, false), (3_500, false), (4_000, true)] {
            m.inputs.near = near;
            let l = m.look(t);
            grown += usize::from(!l.thin);
            assert_eq!(l.band, Band { thin: true, changed: false, wake_at: None }, "at {t}");
            assert_eq!(reserved_thickness(1.0, m.band_thin(), 1.0), thickness);
        }
        assert!(grown >= 3, "the dock itself did follow the pointer");
    }

    #[test]
    fn switching_the_setting_off_gives_the_full_band_back_at_once() {
        let mut b = BandSettle::new();
        b.step(0, true, false);
        assert!(b.step(SETTLE_MS, true, false).changed);
        assert!(b.reset(SETTLE_MS + 5), "it was thin, so the band must be set again");
        assert!(!b.thin());
        assert!(!b.reset(SETTLE_MS + 6), "a second reset has nothing to give back");
        // (a window maximised right after still has to settle)
        assert!(!b.step(SETTLE_MS + 10, true, false).changed);
    }

    #[test]
    fn the_bands_thickness_in_each_state() {
        assert_eq!(reserved_thickness(1.0, false, 1.0), 84);
        assert_eq!(reserved_thickness(1.0, true, 1.0), 60);
        // the floor: at the smallest dock size the thin band would be 45 px beside a 48 px taskbar strip
        assert_eq!(reserved_thickness(0.75, true, 1.0), 50);
        assert_eq!(reserved_thickness(0.75 * 1.5, true, 1.5), 74);
        for unit in [0.85, 1.0, 1.2, 1.4, 1.25, 1.5 * 1.4, 2.0] {
            let (full, thin) = (reserved_thickness(unit, false, 1.0), reserved_thickness(unit, true, 1.0));
            assert!(thin < full, "unit {unit}");
            // the window gains the difference of the two glasses (24 units), within a pixel of rounding
            let gain = (full - thin) as f32;
            assert!((gain - 24.0 * unit).abs() <= 1.0, "unit {unit}: {gain}");
            // the thin band is the thin glass and the margin above and below it, as the full band is the full glass and its margins
            assert_eq!(thin, px(THIN_HEIGHT + 2.0 * MARGIN_BOTTOM, unit));
        }
    }

    // ---- the wiring's sequences (the Model is what the app feeds)

    /// The app's timer as the wiring runs it: a look at the cursor and at what is open every 60 ms while `needs_polling`, plus a look
    /// at every fact that arrives and at the deadline the last look asked for.
    struct Wiring {
        m: Model,
        now: u64,
        wake: Option<u64>,
        sets: Vec<(u64, bool)>,
        open: bool,
    }

    impl Wiring {
        fn new() -> Wiring {
            let mut m = Model::new();
            m.inputs.enabled = true;
            Wiring { m, now: 0, wake: None, sets: Vec::new(), open: false }
        }

        fn look(&mut self) {
            let l = self.m.look(self.now);
            self.wake = l.wake_at;
            if l.band.changed {
                self.sets.push((self.now, l.band.thin));
            }
        }

        /// Time passes by `ms`: the poller ticks while it runs, the deadline fires.
        fn run(&mut self, ms: u64) {
            let end = self.now + ms;
            while self.now < end {
                self.now += 20;
                if self.wake.is_some_and(|w| w <= self.now) {
                    self.look();
                }
                if self.now.is_multiple_of(60) && self.m.needs_polling() && self.m.observe(Some(false), false, self.open) {
                    self.look();
                }
            }
        }
    }

    #[test]
    fn the_last_window_going_while_a_menu_is_open_gives_the_band_back_when_the_menu_closes() {
        let mut w = Wiring::new();
        // a window is maximised, the dock goes thin, the band follows
        w.m.inputs.maximized = true;
        w.look();
        w.run(1_000);
        assert_eq!(w.sets, [(SETTLE_MS, true)]);
        // the pointer goes to the dock, a menu opens on an icon of that window; the menu's "close window" takes the window away
        w.open = true;
        w.m.inputs.near = true;
        w.m.inputs.open = true;
        w.look();
        w.m.inputs.maximized = false;
        w.m.inputs.near = false;
        w.look();
        w.run(10_000);
        assert_eq!(w.sets.len(), 1, "the menu is still open: the band stays");
        assert!(w.m.needs_polling(), "the poller must keep running: nothing else will notice the menu closing");
        // the menu closes: the band is the full one again, without any other fact arriving
        w.open = false;
        w.run(1_000);
        assert_eq!(w.sets.len(), 2, "{:?}", w.sets);
        assert!(!w.sets[1].1);
        assert!(!w.m.band_thin());
        assert!(!w.m.needs_polling(), "and the poller stops when there is nothing left to look at");
    }

    #[test]
    fn the_poller_runs_while_a_window_is_seen_and_while_the_band_is_still_thin_and_never_with_the_setting_off() {
        let mut m = Model::new();
        assert!(!m.needs_polling());
        m.inputs = on(true, false, false);
        assert!(m.needs_polling());
        m.look(0);
        m.look(SETTLE_MS);
        // the window is gone: the full band is owed, so the look goes on until it is given
        m.inputs.maximized = false;
        assert!(m.needs_polling());
        m.look(SETTLE_MS + 1);
        m.look(SETTLE_MS + HOLD_MS + SETTLE_MS);
        assert!(!m.band_thin());
        assert!(!m.needs_polling());
        // setting off: never
        m.inputs = on(true, false, false);
        m.switch_off(100_000);
        assert!(!m.needs_polling());
    }

    #[test]
    fn switching_the_setting_off_while_thin_gives_everything_back_at_once() {
        let mut w = Wiring::new();
        w.m.inputs.maximized = true;
        w.look();
        w.run(1_000);
        assert!(w.m.band_thin());
        assert!(w.m.switch_off(w.now), "the band was thin: it must be set again");
        assert_eq!(w.m.inputs, Inputs::default());
        let l = w.m.look(w.now);
        assert_eq!((l.thin, l.band.thin, l.band.changed, l.wake_at), (false, false, false, None));
        assert!(!w.m.switch_off(w.now), "nothing more to give back");
    }

    #[test]
    fn a_zone_that_is_not_known_neither_brings_the_pointer_near_nor_sends_it_away() {
        let mut m = Model::new();
        m.inputs = on(true, true, false);
        // (the layout is being worked out and a rectangle cannot be read for a moment)
        assert!(!m.observe(None, false, false));
        assert!(m.inputs.near, "a pointer resting on the dock is still on it");
        m.inputs.near = false;
        assert!(!m.observe(None, false, false));
        assert!(!m.inputs.near);
        // a known zone decides; a held button keeps it near after the pointer slipped out
        assert!(m.observe(Some(true), false, false));
        assert!(!m.observe(Some(false), true, false), "a press that began on the dock");
        assert!(m.observe(Some(false), false, false), "the button is let go and the pointer is away");
        // no maximised window: never near, whatever the cursor does
        m.inputs.maximized = false;
        m.inputs.near = true;
        assert!(m.observe(Some(true), false, false));
        assert!(!m.inputs.near);
    }

    #[test]
    fn the_work_area_follows_the_band_in_both_directions_at_every_size_and_scale_the_thin_band_included() {
        // The host-owned work area (taskbar hidden by us) judges what is there by the taskbar's strip: a band thinner than the strip
        // would not be recognised as a band. The thin band is 60 units, and never thinner than the strip and two px (the floor in
        // `reserved_thickness`, which acts at the smallest dock size, 0.75): at every size and every Windows scale it is thicker than
        // the taskbar's 48 px strip at that scale, so a full -> thin and a thin -> full change are both "Set".
        use rf_shellguard::tbmodel::MonitorGeom;
        use rf_shellguard::tbwork::{Verdict, monitor_less_band, plan_edge};
        for scale in [1.0f32, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0] {
            let (w, h) = ((2560.0 * scale.max(1.0)) as i32, (1440.0 * scale.max(1.0)) as i32);
            let monitor = [0, 0, w, h];
            let strip = (48.0 * scale).round() as i32;
            let bar = [0, h - strip, w, h];
            for factor in [0.75f32, 0.85, 1.0, 1.4, 1.5] {
                let unit = super::super::size::unit(scale, factor);
                let (full, thin) = (reserved_thickness(unit, false, scale), reserved_thickness(unit, true, scale));
                for (from, to) in [(full, thin), (thin, full)] {
                    let m = MonitorGeom { id: "A".into(), rect: monitor, work: monitor_less_band(monitor, from), primary: true };
                    let plan = plan_edge(&[m], &[bar], Some(to));
                    assert_eq!(plan.monitors[0].verdict, Verdict::Set, "scale {scale}, size {factor}: band {from} -> {to} px, strip {strip} px");
                    assert_eq!(plan.monitors[0].target, monitor_less_band(monitor, to));
                    assert_eq!(plan.monitors[0].user_work, [0, 0, w, h - strip], "the user's own rectangle is the monitor less the strip, whatever band was held");
                }
                assert!(thin > strip + 1, "scale {scale}, size {factor}: the thin band {thin} px is thicker than the strip {strip} px");
            }
        }
    }

    // ---- the thin layout

    #[test]
    fn the_thin_glass_is_the_side_islands_glass_on_the_same_line() {
        for (area, unit) in [
            (Rect::new(0, 0, 1920, 1032), 1.0),
            (Rect::new(0, 0, 2560, 1440), 1.0),
            (Rect::new(0, 0, 2560, 1440), 1.25),
            (Rect::new(100, 50, 2980, 1630), 1.5),
            (Rect::new(0, 0, 1366, 720), 0.85),
            (Rect::new(0, 0, 3840, 2160), 1.4),
            (Rect::new(0, 0, 3840, 2160), 1.1 * 1.2),
        ] {
            let i = layout::islands(area, unit, W);
            let thin = thin_glass(i.middle, unit);
            assert_eq!(thin.bottom, i.left.bottom, "one line at unit {unit}");
            assert_eq!(thin.bottom, i.right.bottom);
            assert_eq!(thin.top, i.left.top, "the same height as the left island at unit {unit}");
            assert_eq!(thin.top, i.right.top);
            assert_eq!(thin.height(), i.left.height());
            assert_eq!((thin.left, thin.right), (i.middle.left, i.middle.right), "as wide as it was: nothing beside it moves");
            // and the device height the view draws is the same number of pixels
            let sq = Squeeze::at(0.0);
            assert_eq!(glass_height_px(&sq, unit, i.middle.height() as f32), i.left.height() as f32);
        }
    }

    #[test]
    fn the_full_squeeze_is_exactly_the_dock_of_today() {
        let s = Squeeze::at(1.0);
        assert_eq!(s, Squeeze::FULL);
        assert_eq!((s.glass_h, s.icon), (MIDDLE_HEIGHT, 1.0));
        assert_eq!(glass_height_px(&s, 1.25, 90.0), 90.0);
        assert_eq!(Squeeze::at(7.0), Squeeze::FULL, "out of range is full");
        assert_eq!(Squeeze::at(-1.0), Squeeze::at(0.0));
    }

    #[test]
    fn the_numbers_between_thin_and_full_move_together_and_never_overshoot() {
        let mut last = Squeeze::at(0.0);
        assert_eq!((last.glass_h, last.icon), (48.0, THIN_ICON / ICON_REST));
        for k in 1..=100 {
            let s = Squeeze::at(k as f32 / 100.0);
            assert!(s.glass_h >= last.glass_h && s.icon >= last.icon && s.glass_h <= MIDDLE_HEIGHT && s.icon <= 1.0, "{s:?}");
            last = s;
        }
        // in the middle of the growth: halfway
        let h = Squeeze::at(0.5);
        assert!((h.glass_h - 60.0).abs() < 1e-4 && (h.icon - (1.0 + THIN_ICON / ICON_REST) / 2.0).abs() < 1e-4);
    }

    #[test]
    fn the_icon_and_its_mark_fit_the_thin_glass() {
        // the foot is 12 above the glass's bottom (BASELINE), the mark's lower edge 7 under it and 4 high (dock/look.rs, middle.rs)
        const BASELINE: f32 = 12.0;
        const DOT_DROP: f32 = 7.0;
        const DOT_HEIGHT: f32 = 4.0;
        // (the mark stays inside the glass; the icon too, with at least 4 units over its head)
        const { assert!(BASELINE - DOT_DROP - DOT_HEIGHT >= 0.0) };
        const { assert!(BASELINE + THIN_ICON < THIN_HEIGHT) };
        const { assert!(THIN_HEIGHT - (BASELINE + THIN_ICON) >= 4.0) };
        // the same at every dock size: all of it scales with the unit, and a device pixel of rounding cannot push the icon out
        for unit in [0.85, 1.0, 1.2, 1.4, 1.5 * 1.4] {
            let glass = px(THIN_HEIGHT, unit);
            let top = px(BASELINE, unit) + px(THIN_ICON, unit);
            assert!(top < glass, "unit {unit}: the icon's top is {top}, the glass {glass}");
            assert!(px(BASELINE, unit) - px(DOT_DROP, unit) - px(DOT_HEIGHT, unit) >= -1, "unit {unit}: the mark is inside the glass");
        }
    }

    #[test]
    fn the_bell_is_nothing_while_thin_and_whole_when_grown() {
        assert_eq!(Squeeze::at(0.0).bell(1.0), 0.0);
        assert_eq!(Squeeze::at(1.0).bell(1.0), 1.0);
        assert_eq!(Squeeze::at(1.0).bell(0.3), 0.3, "full: the presence as it is today");
        assert!((Squeeze::at(0.5).bell(1.0) - 0.5).abs() < 1e-6);
    }

    // ---- near

    #[test]
    fn near_is_the_band_the_thin_glass_stands_in_and_24_units_above_it() {
        let monitor = Rect::new(0, 0, 2560, 1440);
        let i = layout::islands(monitor, 1.0, W);
        let z = zone(i.middle, i.left, i.right, monitor, 1.0);
        let thin = thin_glass(i.middle, 1.0); // 1434 - 48 = 1386 .. 1434
        assert_eq!((thin.top, thin.bottom), (1386, 1434));
        // inside the thin glass
        assert!(z.contains(1280, 1380));
        // on the 6 px margin below the glass, down to the monitor's last row
        assert!(z.contains(1280, 1435) && z.contains(1280, 1439));
        assert!(!z.contains(1280, 1440), "the monitor ends");
        // the band's margin above the glass (6) and the 24 above that
        assert!(z.contains(1280, 1386 - 6) && z.contains(1280, 1386 - 30));
        assert!(!z.contains(1280, 1386 - 31), "31 units above the glass is beyond it");
        // to the sides: the room the glass widens into
        assert!(z.contains(thin.left - 40, 1380) && z.contains(thin.right + 39, 1380));
        assert!(!z.contains(thin.left - 41, 1380));
        // the whole of the grown glass is inside the zone, widened or not
        let grown = Rect { left: thin.left - 40, right: thin.right + 40, top: i.middle.top, bottom: thin.bottom };
        for (x, y) in [(grown.left, grown.top), (grown.right - 1, grown.top), (grown.left, grown.bottom - 1), (grown.right - 1, grown.bottom - 1)] {
            assert!(z.contains(x, y), "({x},{y})");
        }
    }

    #[test]
    fn the_side_islands_are_not_the_dock() {
        // squeeze the middle against the left island (a narrow screen): the Start button and the clock are not near the dock
        let monitor = Rect::new(0, 0, 1000, 700);
        let i = layout::islands(monitor, 1.0, W);
        let z = zone(i.middle, i.left, i.right, monitor, 1.0);
        let y = i.left.top + 24;
        assert!(!z.contains(i.left.right - 1, y) && !z.contains(i.left.left + 5, y));
        assert!(!z.contains(i.right.left + 1, y));
        assert!(z.contains(i.middle.left + 20, y));
    }

    #[test]
    fn the_zone_stays_on_the_monitor() {
        let monitor = Rect::new(-1920, 0, 0, 1080);
        let i = layout::islands(Rect::new(-1920, 0, 0, 1032), 1.0, W);
        let z = zone(i.middle, i.left, i.right, monitor, 1.0);
        assert!(monitor.covers(&z.near), "{:?}", z.near);
    }

    // ---- which windows count

    fn win() -> WinFacts {
        WinFacts { visible: true, zoomed: true, on_dock_monitor: true, ..Default::default() }
    }

    #[test]
    fn a_visible_maximised_window_on_the_docks_monitor_counts() {
        assert!(counts_as_maximized(&win()));
        assert!(any_maximized([WinFacts { zoomed: false, ..win() }, win()]));
        assert!(!any_maximized(std::iter::empty()));
    }

    #[test]
    fn windows_the_user_cannot_see_or_that_are_not_windows_of_a_taskbar_do_not_count() {
        for f in [
            WinFacts { zoomed: false, ..win() },
            WinFacts { visible: false, ..win() },
            WinFacts { minimized: true, ..win() },
            WinFacts { cloaked: true, ..win() },
            WinFacts { on_dock_monitor: false, ..win() },
            WinFacts { own: true, ..win() },
            WinFacts { shell: true, ..win() },
            // an owned dialog without a taskbar button, a tool window, a window that never takes the focus
            WinFacts { has_owner: true, ..win() },
            WinFacts { ex_style: windows::Win32::UI::WindowsAndMessaging::WS_EX_TOOLWINDOW.0, ..win() },
            WinFacts { ex_style: windows::Win32::UI::WindowsAndMessaging::WS_EX_NOACTIVATE.0, ..win() },
        ] {
            assert!(!counts_as_maximized(&f), "{f:?}");
        }
        // but an owned window that asks for a button does
        assert!(counts_as_maximized(&WinFacts { has_owner: true, ex_style: windows::Win32::UI::WindowsAndMessaging::WS_EX_APPWINDOW.0, ..win() }));
    }
}
