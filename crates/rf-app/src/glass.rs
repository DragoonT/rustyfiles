//! The app's side of the experimental Glass backdrop (the engine is `rf_win::glass`): the switch that forces it for one run, the reason it
//! was given up (for the settings page) and the one rule the app's own popups follow.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use gpui_kit::{Bounds, IntoElement, Pixels, Styled, Window, canvas};
use rf_win::glass::LensSpec;

/// `RUSTYFILES_GLASS=1`: every main window uses the Glass backdrop for this run, whatever the saved setting says (and nothing is saved).
/// Read once. "0" and an empty value mean off.
pub fn forced() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("RUSTYFILES_GLASS").is_ok_and(|v| !v.is_empty() && v != "0"))
}

/// Why the engine gave up on a window, for the settings page. Only the latest failure is kept: a failed window is on Mica, the page
/// says so under the backdrop card.
static FAILURE: Mutex<Option<String>> = Mutex::new(None);

pub fn failure() -> Option<String> {
    FAILURE.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

pub fn set_failure(why: Option<String>) {
    *FAILURE.lock().unwrap_or_else(|p| p.into_inner()) = why;
}

/// The windows whose glass is on the screen right now (their scene is see-through). The palette asks for every window it paints, so a
/// flag answers first when no window has glass at all.
static SHOWN: Mutex<Vec<isize>> = Mutex::new(Vec::new());
static SHOWN_ANY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_shown(hwnd: isize, on: bool) {
    let mut v = SHOWN.lock().unwrap_or_else(|p| p.into_inner());
    v.retain(|&h| h != hwnd);
    if on && hwnd != 0 {
        v.push(hwnd);
    }
    SHOWN_ANY.store(!v.is_empty(), std::sync::atomic::Ordering::Relaxed);
}

/// Whether this window has its glass on the screen.
pub fn shown(hwnd: isize) -> bool {
    SHOWN_ANY.load(std::sync::atomic::Ordering::Relaxed) && SHOWN.lock().unwrap_or_else(|p| p.into_inner()).contains(&hwnd)
}

/// A popup, menu or flyout of ours (its HWND at hand) stays out of the screen capture while some window has glass. Otherwise the glass
/// would show a copy of the popup, bent, behind the popup itself. Called for every such window, glass or not: the engine remembers it,
/// applies the exclusion when glass starts (also for a window that is already open) and lifts it when the last glass goes.
pub(crate) fn keep_out_of_capture(hwnd: isize) {
    if hwnd != 0 {
        rf_win::glass::exclude_from_capture(hwnd, true);
    }
}

// ---------------------------------------------------------------- lenses (the selection pill of a list)

/// The lens slots the app uses (the engine has `rf_win::glass::MAX_LENSES`): the sidebar's selected entry, the selected entry of the
/// settings page's own list (both can be on screen at once: the sidebar stays beside the settings page), the selected tab of the tab
/// strip and the round glass behind the strip's "+" button.
pub const SLOT_SIDEBAR: u8 = 0;
pub const SLOT_SETTINGS: u8 = 1;
pub const SLOT_TAB: u8 = 2;
pub const SLOT_NEW_TAB: u8 = 3;

/// A selection pill is at most this round (logical px); a row lower than twice this gets a capsule.
const LENS_MAX_RADIUS: f32 = 14.0;

/// A stable number for "this entry": the same value gives the same key, so the engine knows the selection did not move and only the
/// place did (a scroll, a resize); another entry gives another key and the lens slides to it.
pub fn key_of<T: Hash + ?Sized>(what: &T) -> u64 {
    // `DefaultHasher::new()` has fixed keys: the same input hashes the same all through the run, which is all a key needs
    let mut h = std::collections::hash_map::DefaultHasher::new();
    what.hash(&mut h);
    h.finish()
}

/// What a lens looks like on the box its probe sits in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LensShape {
    /// The selected row of a list: a capsule for a low row, `LENS_MAX_RADIUS` for a tall one.
    Row,
    /// The selected tab. The glass takes the tab's own rectangle, made taller downwards by the corner radius, and the clip (the tab
    /// strip) cuts the extra part off: the top corners are round and the bottom is flat, where the tab meets the toolbar. `radius` is the
    /// tab's corner radius in logical px.
    Tab { radius: f32 },
    /// A circle as high as the box, centred on it (the round glass behind the "+" button).
    Circle,
}

/// A tab's corner is at most this round (logical px); the glass never gets rounder than the tab it sits in.
const TAB_MAX_RADIUS: f32 = 12.0;

/// The lens for a row, in the engine's units (`shape_spec` with the row shape; the probes call that one). `rect` and `clip` are x, y, w, h in the window's logical px, the scale factor turns them
/// into device px. The corner radius is a capsule for a low row and `LENS_MAX_RADIUS` for a tall one. `None` when the numbers cannot
/// be drawn (not finite, no size, no scale).
#[cfg(test)]
pub(crate) fn lens_spec(key: u64, rect: [f32; 4], clip: [f32; 4], scale: f32) -> Option<LensSpec> {
    shape_spec(LensShape::Row, key, rect, clip, scale)
}

/// The lens for a box of the given shape (see [`LensShape`]); `rect` is the box, `clip` where the lens must stay, both in logical px.
pub(crate) fn shape_spec(shape: LensShape, key: u64, rect: [f32; 4], clip: [f32; 4], scale: f32) -> Option<LensSpec> {
    let ok = |a: &[f32; 4]| a.iter().all(|v| v.is_finite());
    if !ok(&rect) || !ok(&clip) || !scale.is_finite() || scale <= 0.0 || rect[2] <= 0.0 || rect[3] <= 0.0 {
        return None;
    }
    let (rect, radius) = match shape {
        LensShape::Row => (rect, (rect[3] / 2.0).min(LENS_MAX_RADIUS)),
        LensShape::Tab { radius } => {
            if !radius.is_finite() {
                return None;
            }
            let r = radius.clamp(0.0, TAB_MAX_RADIUS).min(rect[2] / 2.0);
            ([rect[0], rect[1], rect[2], rect[3] + r], r)
        }
        LensShape::Circle => {
            let d = rect[3];
            ([rect[0] + (rect[2] - d) / 2.0, rect[1], d, d], d / 2.0)
        }
    };
    let dev = |a: [f32; 4]| [a[0] * scale, a[1] * scale, a[2] * scale, a[3] * scale];
    Some(LensSpec { key, rect: dev(rect), clip: dev(clip), radius: radius * scale })
}

/// What was last sent to the engine for each window and slot, so a lens that did not change since the last paint sends nothing (the row
/// reports its place on every paint). An absent entry is the same as `None`.
#[derive(Default)]
pub(crate) struct LensMemory {
    sent: BTreeMap<(isize, u8), LensSpec>,
}

impl LensMemory {
    /// Remembers `now` for the window and slot and says whether it differs from what was remembered, that is whether it must be sent.
    pub(crate) fn changed(&mut self, hwnd: isize, slot: u8, now: Option<LensSpec>) -> bool {
        if self.sent.get(&(hwnd, slot)).copied() == now {
            return false;
        }
        match now {
            Some(l) => self.sent.insert((hwnd, slot), l),
            None => self.sent.remove(&(hwnd, slot)),
        };
        true
    }

    /// Forgets everything about one window (its glass is gone or starts afresh: the engine has no lens of it).
    pub(crate) fn forget(&mut self, hwnd: isize) {
        self.sent.retain(|&(h, _), _| h != hwnd);
    }
}

static LENSES: Mutex<LensMemory> = Mutex::new(LensMemory { sent: BTreeMap::new() });

fn lenses() -> std::sync::MutexGuard<'static, LensMemory> {
    LENSES.lock().unwrap_or_else(|p| p.into_inner())
}

fn rect_of(b: Bounds<Pixels>) -> [f32; 4] {
    [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)]
}

/// Called from a canvas inside the selected row, on every paint: puts the lens on the row (`rect`) and cuts it at `clip` (the viewport
/// the row scrolls in). Only while this window shows glass, and only when something changed since the last call.
pub fn report_lens(window: &Window, slot: u8, key: u64, shape: LensShape, rect: Bounds<Pixels>, clip: Bounds<Pixels>) {
    let hwnd = crate::look::window::hwnd_of(window);
    if hwnd == 0 || !shown(hwnd) {
        return;
    }
    let now = shape_spec(shape, key, rect_of(rect), rect_of(clip), window.scale_factor());
    if now.is_none() {
        // a row that cannot be drawn is no row: take the lens off, at once (it is not on screen to fade away from)
        clear_lens(hwnd, slot, false);
        return;
    }
    if lenses().changed(hwnd, slot, now) {
        rf_win::glass::set_lens(hwnd, slot, now, crate::settings::animations());
    }
}

/// The element a selected row carries while the glass is on: it fills the row (the box the selection fill had), paints nothing, and
/// tells the engine on every paint where the row is. The clip is the content mask at that point, which is the viewport of the nearest
/// scrolling parent (the sidebar's list), cut by the parents' own masks; with no such parent it is the whole window.
pub fn lens_probe(slot: u8, key: u64) -> impl IntoElement {
    lens_probe_shaped(slot, key, LensShape::Row)
}

/// [`lens_probe`] for a lens that is not a list row's pill: the selected tab, the round glass of the "+" button.
pub fn lens_probe_shaped(slot: u8, key: u64, shape: LensShape) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let clip = window.content_mask().bounds;
            report_lens(window, slot, key, shape, bounds, clip);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
}

/// Takes the lens out of a slot (the list has no selected entry, is hidden or is not the one on show). Cheap when nothing is there.
/// `fade`: the list is still on screen and only its selection went away, so the pill fades out where it was; when the list itself is
/// gone (another tab, a hidden sidebar) the pill goes with it at once, instead of hanging over whatever replaced the list.
pub fn clear_lens(hwnd: isize, slot: u8, fade: bool) {
    if hwnd == 0 {
        return;
    }
    if !shown(hwnd) {
        // no glass: the engine has no lens, and none must be remembered for a glass that may start later
        lenses().forget(hwnd);
        return;
    }
    if lenses().changed(hwnd, slot, None) {
        rf_win::glass::set_lens(hwnd, slot, None, fade && crate::settings::animations());
    }
}

/// What a frame did with the tab strip at the top of the window, for [`lens_plan`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub(crate) struct StripFrame {
    /// The strip is built and on the screen this frame.
    pub on_screen: bool,
    /// A tab of it took the tab lens (the selected tab, when the strip shows its tabs over glass).
    pub tab_claimed: bool,
    /// The "+" button took its round lens.
    pub plus_claimed: bool,
}

/// What a frame does with the window's lens slots, decided apart from the UI so it can be tested.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LensPlan {
    /// Take the sidebar's lens out: `Some(fade)`.
    pub clear_sidebar: Option<bool>,
    /// The focused pane owns the settings lens; with no owner the settings lens is taken out (at once).
    pub settings_owner: bool,
    /// Take the selected tab's lens out: `Some(fade)`. It fades while the strip is still there (no tab selected), and goes at once when
    /// the strip is not shown.
    pub clear_tab: Option<bool>,
    /// Take the "+" button's lens out, the same way.
    pub clear_new_tab: Option<bool>,
}

/// `glass_on`: this window shows glass. `sidebar_on_screen`: the full sidebar is built this frame (not hidden, not the rail).
/// `sidebar_claimed`: a selected row of it took the lens. `settings_focused`: the focused pane of the shown tab is on Settings.
/// `strip`: what the tab strip did this frame. (The window has one tab strip for all its panes, so there is no strip of an unfocused
/// pane to keep apart: the tab lens is always the active tab's.)
pub(crate) fn lens_plan(glass_on: bool, sidebar_on_screen: bool, sidebar_claimed: bool, settings_focused: bool, strip: StripFrame) -> LensPlan {
    LensPlan {
        clear_sidebar: (glass_on && !sidebar_claimed).then_some(sidebar_on_screen),
        settings_owner: glass_on && settings_focused,
        clear_tab: (glass_on && !strip.tab_claimed).then_some(strip.on_screen),
        clear_new_tab: (glass_on && !strip.plus_claimed).then_some(strip.on_screen),
    }
}

/// The window's glass ends, fails or starts afresh, or the window closes: the engine drops the lenses with it, so what the app
/// remembers having sent is void; the next glass gets the lenses sent again.
pub fn forget_window(hwnd: isize) {
    lenses().forget(hwnd);
}

#[cfg(test)]
mod tests {
    fn spec(key: u64, y: f32) -> super::LensSpec {
        super::lens_spec(key, [8.0, y, 200.0, 32.0], [0.0, 0.0, 240.0, 600.0], 1.0).unwrap()
    }

    #[test]
    fn logical_px_become_device_px() {
        let l = super::lens_spec(7, [10.0, 20.0, 200.0, 40.0], [0.0, 5.0, 240.0, 600.0], 1.5).unwrap();
        assert_eq!(l.key, 7);
        assert_eq!(l.rect, [15.0, 30.0, 300.0, 60.0]);
        assert_eq!(l.clip, [0.0, 7.5, 360.0, 900.0]);
        // a 40 px row would be a 20 px capsule, the cap is 14 px: 14 * 1.5
        assert_eq!(l.radius, 21.0);
    }

    #[test]
    fn a_low_row_gets_a_capsule_a_tall_one_the_capped_radius() {
        let low = super::lens_spec(1, [0.0, 0.0, 100.0, 24.0], [0.0; 4], 2.0).unwrap();
        assert_eq!(low.radius, 24.0); // 12 logical, 24 device: half the 48 px height
        let tall = super::lens_spec(1, [0.0, 0.0, 100.0, 80.0], [0.0; 4], 1.0).unwrap();
        assert_eq!(tall.radius, 14.0);
    }

    #[test]
    fn a_lens_that_cannot_be_drawn_is_none() {
        let c = [0.0, 0.0, 10.0, 10.0];
        assert!(super::lens_spec(1, [0.0, 0.0, 0.0, 10.0], c, 1.0).is_none());
        assert!(super::lens_spec(1, [0.0, 0.0, 10.0, -1.0], c, 1.0).is_none());
        assert!(super::lens_spec(1, [f32::NAN, 0.0, 10.0, 10.0], c, 1.0).is_none());
        assert!(super::lens_spec(1, [0.0, 0.0, 10.0, 10.0], [0.0, 0.0, f32::INFINITY, 1.0], 1.0).is_none());
        assert!(super::lens_spec(1, [0.0, 0.0, 10.0, 10.0], c, 0.0).is_none());
    }

    #[test]
    fn the_same_lens_again_is_not_sent() {
        let mut m = super::LensMemory::default();
        assert!(m.changed(5, 0, Some(spec(1, 10.0))), "the first one is new");
        assert!(!m.changed(5, 0, Some(spec(1, 10.0))), "the same again");
        assert!(m.changed(5, 0, Some(spec(1, 12.0))), "moved by a scroll");
        assert!(m.changed(5, 0, Some(spec(2, 12.0))), "another entry");
        assert!(m.changed(5, 0, None), "taken off");
        assert!(!m.changed(5, 0, None), "nothing there, nothing to take off");
    }

    #[test]
    fn windows_and_slots_have_their_own_memory() {
        let mut m = super::LensMemory::default();
        assert!(m.changed(5, 0, Some(spec(1, 10.0))));
        assert!(m.changed(6, 0, Some(spec(1, 10.0))), "another window");
        assert!(m.changed(5, 1, Some(spec(1, 10.0))), "another slot");
        assert!(!m.changed(5, 0, Some(spec(1, 10.0))));
        assert!(!m.changed(6, 0, Some(spec(1, 10.0))));
    }

    #[test]
    fn forgetting_a_window_sends_its_lenses_again_and_leaves_the_others() {
        let mut m = super::LensMemory::default();
        m.changed(5, 0, Some(spec(1, 10.0)));
        m.changed(5, 1, Some(spec(2, 50.0)));
        m.changed(6, 0, Some(spec(3, 10.0)));
        m.forget(5);
        assert!(m.changed(5, 0, Some(spec(1, 10.0))));
        assert!(m.changed(5, 1, Some(spec(2, 50.0))));
        assert!(!m.changed(6, 0, Some(spec(3, 10.0))), "the other window is untouched");
        // a forgotten window has nothing to take off either
        m.forget(5);
        assert!(!m.changed(5, 0, None));
    }

    #[test]
    fn keys_follow_the_entry_not_the_place() {
        use rf_core::history::Location;
        let a = Location::Path("C:\\Users\\a\\Desktop".into());
        let b = Location::Path("C:\\Users\\a\\Downloads".into());
        assert_eq!(super::key_of(&a), super::key_of(&a.clone()));
        assert_ne!(super::key_of(&a), super::key_of(&b));
        assert_ne!(super::key_of(&Location::Home), super::key_of(&Location::Settings));
    }

    #[test]
    fn without_glass_nothing_is_sent_or_remembered() {
        // window 901 never had glass: clearing through the public calls must not panic, and `shown` stays false
        assert!(!super::shown(901));
        super::clear_lens(901, super::SLOT_SIDEBAR, true);
        super::clear_lens(0, super::SLOT_SETTINGS, false);
        super::forget_window(901);
    }

    #[test]
    fn failure_is_kept_and_cleared() {
        super::set_failure(Some("no duplication".into()));
        assert_eq!(super::failure().as_deref(), Some("no duplication"));
        super::set_failure(None);
        assert_eq!(super::failure(), None);
    }

    #[test]
    fn shown_windows_are_remembered_one_by_one() {
        assert!(!super::shown(41));
        super::set_shown(41, true);
        super::set_shown(42, true);
        assert!(super::shown(41) && super::shown(42) && !super::shown(43));
        super::set_shown(41, false);
        assert!(!super::shown(41) && super::shown(42));
        super::set_shown(42, false);
        assert!(!super::shown(42));
    }

    #[test]
    fn a_frame_without_a_claim_clears_and_the_focused_pane_owns_the_settings_lens() {
        use super::{LensPlan, StripFrame, lens_plan};
        let none = StripFrame::default();
        let both = StripFrame { on_screen: true, tab_claimed: true, plus_claimed: true };
        // sidebar on screen with a selected row, the focused pane on Settings: nothing is cleared, the pane owns the lens
        assert_eq!(
            lens_plan(true, true, true, true, both),
            LensPlan { clear_sidebar: None, settings_owner: true, clear_tab: None, clear_new_tab: None }
        );
        // no selected row but the sidebar is there: the pill fades out
        assert_eq!(lens_plan(true, true, false, false, both).clear_sidebar, Some(true));
        // the sidebar hidden, collapsed to the rail or too narrow for the window: it takes the pill away at once
        assert_eq!(lens_plan(true, false, false, false, both).clear_sidebar, Some(false));
        // the focused pane of a split tab is a folder (the other pane may show Settings): no pane owns the settings lens
        assert!(!lens_plan(true, true, true, false, both).settings_owner);
        // without glass nothing is planned at all, whatever the strip did
        assert_eq!(
            lens_plan(false, true, false, true, none),
            LensPlan { clear_sidebar: None, settings_owner: false, clear_tab: None, clear_new_tab: None }
        );
    }

    #[test]
    fn the_tab_lenses_follow_the_strip() {
        use super::{StripFrame, lens_plan};
        let plan = |glass_on, strip| lens_plan(glass_on, true, true, false, strip);
        // shown: the selected tab and the "+" button have their lenses, nothing is cleared
        let shown = StripFrame { on_screen: true, tab_claimed: true, plus_claimed: true };
        assert_eq!((plan(true, shown).clear_tab, plan(true, shown).clear_new_tab), (None, None));
        // the strip is built but no tab is selected (a tab that cannot be drawn): the tab pill fades, the "+" stays
        let no_tab = StripFrame { tab_claimed: false, ..shown };
        assert_eq!((plan(true, no_tab).clear_tab, plan(true, no_tab).clear_new_tab), (Some(true), None));
        // the strip is not shown this frame: both go at once, without a fade over whatever replaced it
        let hidden = StripFrame::default();
        assert_eq!((plan(true, hidden).clear_tab, plan(true, hidden).clear_new_tab), (Some(false), Some(false)));
        // no glass: the lenses are not touched (the engine has none and the memory is forgotten with the glass)
        assert_eq!((plan(false, hidden).clear_tab, plan(false, hidden).clear_new_tab), (None, None));
        assert_eq!((plan(false, shown).clear_tab, plan(false, shown).clear_new_tab), (None, None));
        // the tab lens and the "+" lens are decided apart
        let only_plus = StripFrame { on_screen: true, tab_claimed: false, plus_claimed: true };
        assert_eq!((plan(true, only_plus).clear_tab, plan(true, only_plus).clear_new_tab), (Some(true), None));
    }

    #[test]
    fn the_slots_are_all_different_and_inside_the_engine() {
        let slots = [super::SLOT_SIDEBAR, super::SLOT_SETTINGS, super::SLOT_TAB, super::SLOT_NEW_TAB];
        for (i, a) in slots.iter().enumerate() {
            assert!((*a as usize) < rf_win::glass::MAX_LENSES);
            assert!(slots[i + 1..].iter().all(|b| a != b), "slot {a} is used twice");
        }
    }

    #[test]
    fn a_tab_lens_is_the_tab_made_taller_by_its_corner_and_clipped_by_the_strip() {
        use super::{LensShape, shape_spec};
        // a 140 x 32 tab at (50, 8) in a strip that is 40 high (so the tab ends at the strip's bottom edge)
        let strip = [0.0, 0.0, 600.0, 40.0];
        let l = shape_spec(LensShape::Tab { radius: 8.0 }, 5, [50.0, 8.0, 140.0, 32.0], strip, 1.0).unwrap();
        assert_eq!(l.rect, [50.0, 8.0, 140.0, 40.0], "taller by the radius, downwards");
        assert_eq!(l.clip, strip, "the clip is the strip, not the tab");
        assert_eq!(l.radius, 8.0);
        assert_eq!(l.rect[1] + l.rect[3] - (strip[1] + strip[3]), 8.0, "the extra part is below the strip's bottom edge, so it is cut flat");
        // device px
        let l = shape_spec(LensShape::Tab { radius: 8.0 }, 5, [50.0, 8.0, 140.0, 32.0], strip, 1.5).unwrap();
        assert_eq!((l.rect, l.clip[3], l.radius), ([75.0, 12.0, 210.0, 60.0], 60.0, 12.0));
        // the tab's own radius, at most 12
        assert_eq!(shape_spec(LensShape::Tab { radius: 20.0 }, 5, [0.0, 0.0, 100.0, 32.0], strip, 1.0).unwrap().radius, 12.0);
        assert_eq!(shape_spec(LensShape::Tab { radius: 0.0 }, 5, [0.0, 0.0, 100.0, 32.0], strip, 1.0).unwrap().rect[3], 32.0);
        // never rounder than half the width
        assert_eq!(shape_spec(LensShape::Tab { radius: 12.0 }, 5, [0.0, 0.0, 10.0, 32.0], strip, 1.0).unwrap().radius, 5.0);
        // a radius that is not a number, or a box that cannot be drawn, gives no lens
        assert!(shape_spec(LensShape::Tab { radius: f32::NAN }, 5, [0.0, 0.0, 100.0, 32.0], strip, 1.0).is_none());
        assert!(shape_spec(LensShape::Tab { radius: 8.0 }, 5, [0.0, 0.0, 0.0, 32.0], strip, 1.0).is_none());
    }

    #[test]
    fn the_plus_lens_is_a_circle_of_the_buttons_height_centred_on_it() {
        use super::{LensShape, shape_spec};
        let strip = [0.0, 0.0, 600.0, 40.0];
        // a square 28 px button
        let l = shape_spec(LensShape::Circle, 9, [300.0, 8.0, 28.0, 28.0], strip, 1.0).unwrap();
        assert_eq!((l.rect, l.radius, l.clip), ([300.0, 8.0, 28.0, 28.0], 14.0, strip));
        // a wider box: the circle keeps the height and sits in the middle
        let l = shape_spec(LensShape::Circle, 9, [300.0, 8.0, 40.0, 28.0], strip, 2.0).unwrap();
        assert_eq!(l.rect, [612.0, 16.0, 56.0, 56.0]);
        assert_eq!(l.radius, 28.0, "half the (device) height: a circle");
        assert_eq!(l.rect[2], l.rect[3]);
    }

    #[test]
    fn a_row_shape_is_what_lens_spec_always_made() {
        use super::{LensShape, lens_spec, shape_spec};
        let (rect, clip) = ([8.0, 100.0, 200.0, 32.0], [0.0, 0.0, 240.0, 600.0]);
        assert_eq!(shape_spec(LensShape::Row, 3, rect, clip, 1.25), lens_spec(3, rect, clip, 1.25));
    }

    #[test]
    fn a_popup_without_glass_is_left_alone() {
        // 0 is no window: nothing is remembered or touched
        super::keep_out_of_capture(0);
    }
}
