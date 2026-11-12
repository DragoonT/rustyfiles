//! The app's side of the experimental Glass backdrop (the engine is `rf_win::glass`): the switch that forces it for one run, the reason it
//! was given up (for the settings page) and the one rule the app's own popups follow.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

use std::time::{Duration, Instant};

use gpui_kit::{Bounds, IntoElement, Pixels, Styled, Window, canvas, div, px};
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

/// The lens slots the app uses (the engine has `rf_win::glass::MAX_LENSES`): the sidebar's selected entry and the selected entry of the
/// settings page's own list. Both can be on screen at once (the sidebar stays beside the settings page).
pub const SLOT_SIDEBAR: u8 = 0;
pub const SLOT_SETTINGS: u8 = 1;

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

/// The lens for a row, in the engine's units. `rect` and `clip` are x, y, w, h in the window's logical px, the scale factor turns them
/// into device px. The corner radius is a capsule for a low row and `LENS_MAX_RADIUS` for a tall one. `None` when the numbers cannot
/// be drawn (not finite, no size, no scale).
pub(crate) fn lens_spec(key: u64, rect: [f32; 4], clip: [f32; 4], scale: f32) -> Option<LensSpec> {
    let ok = |a: &[f32; 4]| a.iter().all(|v| v.is_finite());
    if !ok(&rect) || !ok(&clip) || !scale.is_finite() || scale <= 0.0 || rect[2] <= 0.0 || rect[3] <= 0.0 {
        return None;
    }
    let dev = |a: [f32; 4]| [a[0] * scale, a[1] * scale, a[2] * scale, a[3] * scale];
    Some(LensSpec { key, rect: dev(rect), clip: dev(clip), radius: (rect[3] / 2.0).min(LENS_MAX_RADIUS) * scale })
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

/// THE LENS ONLY RESTS. The engine draws the lens into a layer of its own, behind the scene, and that layer reaches the screen a
/// frame or two after the scene's own frame. For a row that stands still that is invisible. For a row that MOVES (the list scrolls, the
/// window is resized) the pill would trail the row and catch up at every step: the user saw it "go up and down" while scrolling the
/// sidebar. So while a row moves, the lens is taken off at once and the row paints a plain stand-in pill itself, in the scene's own
/// frame; when the row has stood still for `SETTLE` the lens comes back, and the stand-in stays for `OVERLAP` more so there is no frame
/// with neither. (A change of selection is not a move: the rows stand still and the lens slides between them.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LensPhase {
    /// The row stands still: the engine's lens, nothing from the scene.
    Rest,
    /// The row is moving: no lens, the scene's stand-in.
    Moving,
    /// The row has just come to rest: the lens is back, the stand-in is still there for a moment.
    Settling,
}

const SETTLE: Duration = Duration::from_millis(140);
const OVERLAP: Duration = Duration::from_millis(70);

/// Where a slot's row was at its last paint, and when it last moved.
#[derive(Clone, Copy)]
struct Track {
    key: u64,
    rect: [f32; 4],
    clip: [f32; 4],
    last_move: Option<Instant>,
}

fn phase_at(last_move: Option<Instant>, now: Instant) -> LensPhase {
    match last_move.map(|t| now.saturating_duration_since(t)) {
        None => LensPhase::Rest,
        Some(d) if d < SETTLE => LensPhase::Moving,
        Some(d) if d < SETTLE + OVERLAP => LensPhase::Settling,
        Some(_) => LensPhase::Rest,
    }
}

/// Notes the row's place at this paint and says which phase the slot is in. A new entry, or another key (the selection moved), is at
/// rest; the same key at another place is a move.
fn note(track: &mut Option<Track>, key: u64, rect: [f32; 4], clip: [f32; 4], now: Instant) -> LensPhase {
    let near = |a: &[f32; 4], b: &[f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.05);
    let last_move = match track {
        Some(t) if t.key == key => {
            if near(&t.rect, &rect) && near(&t.clip, &clip) { t.last_move } else { Some(now) }
        }
        _ => None,
    };
    *track = Some(Track { key, rect, clip, last_move });
    phase_at(last_move, now)
}

static TRACKS: Mutex<BTreeMap<(isize, u8), Track>> = Mutex::new(BTreeMap::new());

fn tracks() -> std::sync::MutexGuard<'static, BTreeMap<(isize, u8), Track>> {
    TRACKS.lock().unwrap_or_else(|p| p.into_inner())
}

/// The phase a slot is in right now, for the row that is being built (it paints the stand-in in `Moving` and `Settling`).
pub fn lens_phase(hwnd: isize, slot: u8) -> LensPhase {
    phase_at(tracks().get(&(hwnd, slot)).and_then(|t| t.last_move), Instant::now())
}

/// The pill a moving row paints itself: the lens's shape and about its tone, without the glass.
pub fn lens_stand_in(dark: bool, height: f32) -> impl IntoElement {
    let (fill, edge) = if dark { (0.085, 0.20) } else { (0.55, 0.60) };
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .rounded(px((height / 2.0).min(LENS_MAX_RADIUS)))
        .bg(gpui_kit::hsla(0., 0., 1., fill))
        .border_1()
        .border_color(gpui_kit::hsla(0., 0., 1., edge))
}

/// Called from a canvas inside the selected row, on every paint: puts the lens on the row (`rect`) and cuts it at `clip` (the viewport
/// the row scrolls in). Only while this window shows glass, and only when something changed since the last call. While the row moves
/// the lens is off (see [`LensPhase`]) and a frame is asked for, so the phase is looked at again when nothing else redraws.
pub fn report_lens(window: &mut Window, slot: u8, key: u64, rect: Bounds<Pixels>, clip: Bounds<Pixels>) {
    let hwnd = crate::look::window::hwnd_of(window);
    if hwnd == 0 || !shown(hwnd) {
        return;
    }
    let (r, c) = (rect_of(rect), rect_of(clip));
    let spec = lens_spec(key, r, c, window.scale_factor());
    if spec.is_none() {
        // a row that cannot be drawn is no row: take the lens off, at once (it is not on screen to fade away from)
        clear_lens(hwnd, slot, false);
        return;
    }
    let phase = {
        let mut t = tracks();
        let mut cur = t.get(&(hwnd, slot)).copied();
        let ph = note(&mut cur, key, r, c, Instant::now());
        if let Some(cur) = cur {
            t.insert((hwnd, slot), cur);
        }
        ph
    };
    match phase {
        LensPhase::Moving => {
            if lenses().changed(hwnd, slot, None) {
                rf_win::glass::set_lens(hwnd, slot, None, false);
            }
            window.request_animation_frame();
        }
        LensPhase::Settling => {
            // back at once, under the stand-in that is still painted: no fade, there is nothing to fade from
            if lenses().changed(hwnd, slot, spec) {
                rf_win::glass::set_lens(hwnd, slot, spec, false);
            }
            window.request_animation_frame();
        }
        LensPhase::Rest => {
            if lenses().changed(hwnd, slot, spec) {
                rf_win::glass::set_lens(hwnd, slot, spec, crate::settings::animations());
            }
        }
    }
}

/// The element a selected row carries while the glass is on: it fills the row (the box the selection fill had), paints nothing, and
/// tells the engine on every paint where the row is. The clip is the content mask at that point, which is the viewport of the nearest
/// scrolling parent (the sidebar's list), cut by the parents' own masks; with no such parent it is the whole window.
pub fn lens_probe(slot: u8, key: u64) -> impl IntoElement {
    canvas(
        move |bounds, window, _| {
            let clip = window.content_mask().bounds;
            report_lens(window, slot, key, bounds, clip);
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
    // the row is gone: where it was says nothing about where it will be next
    tracks().remove(&(hwnd, slot));
    if !shown(hwnd) {
        // no glass: the engine has no lens, and none must be remembered for a glass that may start later
        lenses().forget(hwnd);
        return;
    }
    if lenses().changed(hwnd, slot, None) {
        rf_win::glass::set_lens(hwnd, slot, None, fade && crate::settings::animations());
    }
}

/// What a frame does with the window's two lens slots, decided apart from the UI so it can be tested.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct LensPlan {
    /// Take the sidebar's lens out: `Some(fade)`.
    pub clear_sidebar: Option<bool>,
    /// The focused pane owns the settings lens; with no owner the settings lens is taken out (at once).
    pub settings_owner: bool,
}

/// `glass_on`: this window shows glass. `sidebar_on_screen`: the full sidebar is built this frame (not hidden, not the rail).
/// `sidebar_claimed`: a selected row of it took the lens. `settings_focused`: the focused pane of the shown tab is on Settings.
pub(crate) fn lens_plan(glass_on: bool, sidebar_on_screen: bool, sidebar_claimed: bool, settings_focused: bool) -> LensPlan {
    LensPlan { clear_sidebar: (glass_on && !sidebar_claimed).then_some(sidebar_on_screen), settings_owner: glass_on && settings_focused }
}

/// The window's glass ends, fails or starts afresh, or the window closes: the engine drops the lenses with it, so what the app
/// remembers having sent is void; the next glass gets the lenses sent again.
pub fn forget_window(hwnd: isize) {
    lenses().forget(hwnd);
    tracks().retain(|&(h, _), _| h != hwnd);
}

#[cfg(test)]
mod tests {
    use super::{LensPhase, OVERLAP, SETTLE};
    use std::time::{Duration, Instant};

    #[test]
    fn a_row_that_moves_has_no_lens_until_it_has_stood_still() {
        let t0 = Instant::now();
        let mut tr = None;
        let (a, b, clip) = ([8.0, 100.0, 200.0, 32.0], [8.0, 76.0, 200.0, 32.0], [0.0, 0.0, 240.0, 600.0]);
        // first sight and an unchanged place: at rest
        assert_eq!(super::note(&mut tr, 1, a, clip, t0), LensPhase::Rest);
        assert_eq!(super::note(&mut tr, 1, a, clip, t0 + Duration::from_millis(16)), LensPhase::Rest);
        // the list scrolls: the same entry at another place
        let t1 = t0 + Duration::from_millis(32);
        assert_eq!(super::note(&mut tr, 1, b, clip, t1), LensPhase::Moving);
        // it stays there: still "moving" until SETTLE has passed, then the overlap, then rest
        assert_eq!(super::note(&mut tr, 1, b, clip, t1 + SETTLE - Duration::from_millis(1)), LensPhase::Moving);
        assert_eq!(super::note(&mut tr, 1, b, clip, t1 + SETTLE + Duration::from_millis(1)), LensPhase::Settling);
        assert_eq!(super::note(&mut tr, 1, b, clip, t1 + SETTLE + OVERLAP + Duration::from_millis(1)), LensPhase::Rest);
        // a move in the middle of the wait starts the wait again
        let t2 = t1 + Duration::from_secs(1);
        assert_eq!(super::note(&mut tr, 1, a, clip, t2), LensPhase::Moving);
        assert_eq!(super::note(&mut tr, 1, b, clip, t2 + Duration::from_millis(100)), LensPhase::Moving);
        assert_eq!(super::note(&mut tr, 1, b, clip, t2 + Duration::from_millis(100) + SETTLE - Duration::from_millis(1)), LensPhase::Moving);
        // only the clip changes (the list's viewport is resized): also a move
        let t3 = t2 + Duration::from_secs(1);
        assert_eq!(super::note(&mut tr, 1, b, [0.0, 0.0, 240.0, 500.0], t3), LensPhase::Moving);
        // another entry is selected: the rows stand still, the lens slides: at rest at once, even right after a move
        assert_eq!(super::note(&mut tr, 2, a, clip, t3 + Duration::from_millis(5)), LensPhase::Rest);
    }

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
        use super::{LensPlan, lens_plan};
        // sidebar on screen with a selected row, the focused pane on Settings: nothing is cleared, the pane owns the lens
        assert_eq!(lens_plan(true, true, true, true), LensPlan { clear_sidebar: None, settings_owner: true });
        // no selected row but the sidebar is there: the pill fades out
        assert_eq!(lens_plan(true, true, false, false).clear_sidebar, Some(true));
        // the sidebar hidden, collapsed to the rail or too narrow for the window: it takes the pill away at once
        assert_eq!(lens_plan(true, false, false, false).clear_sidebar, Some(false));
        // the focused pane of a split tab is a folder (the other pane may show Settings): no pane owns the settings lens
        assert!(!lens_plan(true, true, true, false).settings_owner);
        // without glass nothing is planned at all
        assert_eq!(lens_plan(false, true, false, true), LensPlan { clear_sidebar: None, settings_owner: false });
    }

    #[test]
    fn a_popup_without_glass_is_left_alone() {
        // 0 is no window: nothing is remembered or touched
        super::keep_out_of_capture(0);
    }
}
