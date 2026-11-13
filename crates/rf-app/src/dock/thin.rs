//! The thin dock, app side (the rules are `rf_win::dock::thin`, tested without a window): while a maximised window the user can see is on
//! the dock's monitor the middle island is as high as the side islands and its band is the thin one; the pointer coming near grows it
//! back over the window; it goes thin again the moment the pointer has left and nothing of the dock is open (in one motion with the
//! magnification going out: a user saw two steps when it waited 400 ms).
//! Setting "Pencere büyütüldüğünde dock'u incelt" (`dock_thin_when_maximized`, on by default, a key of the running dock: off is the dock
//! of today and costs nothing but one atomic read per task-list event and per mouse move over the dock).
//!
//! WHERE THE FACTS COME FROM.
//! * A maximised window: the task list's events the dock already follows (`MiddleIsland` calls [`task_changed`] from its watch) and the
//!   placement events of `rf_win::dock::zoomwatch` (maximise, restore, snap; started only while the setting is on). Either one asks for
//!   ONE walk over the windows on a worker thread (`zoomwatch::maximized_on`), at most one at a time and one more if events came while it
//!   ran; the answer comes back to the UI thread only as a fact. No timer reads windows.
//! * The pointer: a window with a region is not told that the pointer is NEAR it, and the part above the thin glass is another
//!   program's window, so while the setting is on AND a maximised window is seen (and only then) the UI thread looks at the cursor
//!   every [`POLL_MS`] ms: two system calls, and the rest of the dock is touched only when `near` or `open` changed. The pointer on the
//!   island itself is known at once from its mouse events ([`pointer_on_island`]).
//! * Something open (a panel, the app menu, a hover preview): asked of those modules at every look; a panel opened from the keyboard
//!   (the Windows key) is an open panel like any other.
//!
//! WHAT IT DOES. The island's window never changes (it was always big enough for the full glass and the icons that grow out of it); only
//! the glass inside it and the icons do, in the frames the magnification already draws (`host::set_middle_frame` sends the glass and the
//! pointer region), so the growth costs what a pointer move over the dock costs and nothing moves under the user's hand: the icons keep
//! their centres, the glass keeps its bottom edge and its width. The band is another matter and is [`BandSettle`]'s: it follows the
//! window (with a settle time), not the pointer.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::*;
use rf_win::dock::hover::Slot;
use rf_win::dock::layout::MIDDLE_HEIGHT;
use rf_win::dock::magnify::{Frame, Presence};
use rf_win::dock::thin::{Model, Squeeze, Zone};
use rf_win::dock::zoomwatch::{self, ZoomWatch};

use super::host::{self, Kind};
use super::plan::{self, DockPicture};
use crate::settings::Settings;

/// How often the cursor is looked at while a maximised window is seen (ms).
const POLL_MS: u64 = 60;
/// ... and while the dock is grown under the pointer: the look that sees the pointer leave starts the way back to thin, and it must
/// come in the frame the magnification starts to go out, or the user sees two motions. Two system calls every frame, only for as
/// long as the pointer is at the dock.
const POLL_NEAR_MS: u64 = 16;
/// Events that arrive while a walk over the windows runs make one more walk, no sooner than this after the last one.
const READ_GAP_MS: u64 = 50;

/// The setting is on (read by the task-list callback on another thread: one atomic, nothing else, when it is off).
static ENABLED: AtomicBool = AtomicBool::new(false);
/// Where the task-list callback leaves its news for the UI thread.
static KICK: Mutex<Option<UnboundedSender<()>>> = Mutex::new(None);

/// The task list changed (a window came, went, was shown, hidden, minimised, restored or took the front). Called on a background
/// thread. With the setting off this is one atomic read.
pub fn task_changed() {
    if ENABLED.load(Ordering::Relaxed)
        && let Some(tx) = KICK.lock().unwrap_or_else(|p| p.into_inner()).as_ref()
    {
        let _ = tx.unbounded_send(());
    }
}

struct ThinHost {
    /// The facts (the setting, a maximised window seen, near, open) and the two rules; every sequence of them is tested in rf_win.
    model: Model,
    /// Where "near" is, from the last layout.
    zone: Option<Zone>,
    /// 1: the full dock, 0: the thin one. Starts full and settled.
    grow: Presence,
    t0: Instant,
    watch: Option<ZoomWatch>,
    reading: bool,
    read_again: bool,
    poll_alive: bool,
    poller: Option<Task<()>>,
    waker: Option<Task<()>>,
    wake_gen: u64,
}

impl Global for ThinHost {}

impl ThinHost {
    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }
}

fn me(cx: &mut App) -> Option<&mut ThinHost> {
    if cx.has_global::<ThinHost>() { Some(cx.global_mut::<ThinHost>()) } else { None }
}

// ---------------------------------------------------------------- what the middle island's view asks

/// How thin the middle island is at this moment (the full dock when the feature never made it thin).
pub fn squeeze(cx: &App) -> Squeeze {
    cx.try_global::<ThinHost>().map_or(Squeeze::FULL, |h| Squeeze::at(h.grow.value(h.now_ms())))
}

/// The island is growing or shrinking: another frame is wanted.
pub fn animating(cx: &App) -> bool {
    cx.try_global::<ThinHost>().is_some_and(|h| !h.grow.settled(h.now_ms()))
}

/// The band the dock reserves is the thin one right now.
pub fn band_is_thin(cx: &App) -> bool {
    cx.try_global::<ThinHost>().is_some_and(|h| h.model.band_thin())
}

/// The picture of a frame of the middle island at this squeeze: the glass lowers its top edge and keeps its bottom edge, the icons
/// shrink around their own centres (so the island is as wide as it was and nothing beside it moves), the rest is
/// `plan::picture`'s. At the full squeeze it IS `plan::picture`.
pub fn picture(frame: &Frame, slots: &[Slot], margins: [f32; 4], baseline: f32, sq: &Squeeze) -> DockPicture {
    let full = plan::picture(frame, slots, margins, baseline);
    if sq.is_full() {
        return full;
    }
    let mt = margins[1];
    let glass = [full.glass[0], mt + MIDDLE_HEIGHT - sq.glass_h, full.glass[2], sq.glass_h];
    let items: Vec<(f32, f32)> = slots
        .iter()
        .zip(&full.items)
        .map(|(s, &(x, w))| match s {
            Slot::Icon => {
                let w2 = w * sq.icon;
                (x + (w - w2) / 2.0, w2)
            }
            Slot::Separator => (x, w),
        })
        .collect();
    let bottom = mt + MIDDLE_HEIGHT - baseline;
    let out_of_glass = slots.iter().zip(&items).filter(|(s, _)| **s == Slot::Icon).map(|(_, &(x, w))| [x, bottom - w, w, w]).filter(|b| b[1] < glass[1]).collect();
    DockPicture { glass, items, out_of_glass }
}

/// The glass rectangle in device px for a squeeze that is not full: the bottom edge stays (`bottom_px`), the height is the thin one at
/// the thin end (exactly the side islands' pixels) and the full one (`full_h_px`) at the other.
pub fn fit_glass(glass_px: &mut [f32; 4], bottom_px: f32, sq: &Squeeze, unit: f32, full_h_px: f32) {
    if sq.is_full() {
        return;
    }
    let h = rf_win::dock::thin::glass_height_px(sq, unit, full_h_px);
    glass_px[3] = h;
    glass_px[1] = bottom_px - h;
}

// ---------------------------------------------------------------- the host's calls

/// At the dock's start, after the host exists.
pub fn start(cx: &mut App) {
    if cx.has_global::<ThinHost>() {
        return;
    }
    let (tx, mut rx) = unbounded::<()>();
    *KICK.lock().unwrap_or_else(|p| p.into_inner()) = Some(tx);
    let mut grow = Presence::new(plan::hover_duration(crate::settings::animations()));
    grow.enter(0);
    cx.set_global(ThinHost {
        model: Model::new(),
        zone: None,
        grow,
        // (a clock that started long ago: the full dock is settled, not just arrived)
        t0: Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now),
        watch: None,
        reading: false,
        read_again: false,
        poll_alive: false,
        poller: None,
        waker: None,
        wake_gen: 0,
    });
    cx.spawn(async move |cx| {
        while rx.next().await.is_some() {
            cx.update(read_maximized);
        }
    })
    .detach();
    layout_applied(cx);
    on_settings(cx);
}

/// The dock ends.
pub fn shutdown(cx: &mut App) {
    ENABLED.store(false, Ordering::Relaxed);
    *KICK.lock().unwrap_or_else(|p| p.into_inner()) = None;
    if cx.has_global::<ThinHost>() {
        cx.remove_global::<ThinHost>();
    }
}

/// The settings were read: follow the switch.
pub fn on_settings(cx: &mut App) {
    let want = cx.try_global::<Settings>().is_some_and(|s| s.dock_thin_when_maximized);
    let Some(h) = me(cx) else { return };
    if h.model.inputs.enabled == want {
        return;
    }
    h.model.inputs.enabled = want;
    ENABLED.store(want, Ordering::Relaxed);
    if want {
        // (the watch starts its thread and returns: the UI thread does not wait for it)
        h.watch = ZoomWatch::start(task_changed).map_err(|e| log::warn!("dock: cannot watch window placement: {e:#}")).ok();
        read_maximized(cx);
        reevaluate(cx);
        return;
    }
    // off: the dock of today, at once
    let now = h.now_ms();
    h.watch = None;
    h.poller = None;
    h.poll_alive = false;
    let was_thin_band = h.model.switch_off(now);
    reevaluate(cx);
    if was_thin_band {
        host::apply_reserve(cx);
    }
}

/// The host's layout was worked out (or worked out again): "near" is worked out from the new rectangles.
pub fn layout_applied(cx: &mut App) {
    if !cx.has_global::<ThinHost>() {
        return;
    }
    let zone = zone_now(cx);
    // (a rectangle that cannot be read for a moment does not take the last good zone away: the pointer resting on the dock would be
    // judged away at the next look)
    if let Some(h) = me(cx)
        && zone.is_some()
    {
        h.zone = zone;
    }
}

fn zone_now(cx: &App) -> Option<Zone> {
    let full = host::visible_glass_rect(Kind::Middle, cx)?;
    let (left, right) = (host::visible_glass_rect(Kind::Left, cx)?, host::visible_glass_rect(Kind::Right, cx)?);
    let unit = host::dock_unit(cx)?;
    let monitor = rf_win::dock::primary_monitor()?.rect;
    Some(rf_win::dock::thin::zone(full, left, right, monitor, unit))
}

/// The pointer is on the island (its mouse events say so): near, at once, without waiting for the next look.
pub fn pointer_on_island(cx: &mut App) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(h) = me(cx) else { return };
    if h.model.inputs.enabled && h.model.inputs.maximized && !h.model.inputs.near {
        h.model.inputs.near = true;
        reevaluate(cx);
    }
}

// ---------------------------------------------------------------- the work

/// A walk over the windows, off the UI thread; at most one at a time.
fn read_maximized(cx: &mut App) {
    let Some(h) = me(cx) else { return };
    if !h.model.inputs.enabled {
        return;
    }
    if h.reading {
        h.read_again = true;
        return;
    }
    h.reading = true;
    cx.spawn(async move |cx| {
        loop {
            let seen = cx.background_executor().spawn(async { rf_win::dock::primary_monitor().is_some_and(|m| zoomwatch::maximized_on(m.handle)) }).await;
            if !cx.update(|cx| apply_maximized(seen, cx)) {
                break;
            }
            cx.background_executor().timer(Duration::from_millis(READ_GAP_MS)).await;
        }
    })
    .detach();
}

/// The answer of a walk. True: events came meanwhile, walk again.
fn apply_maximized(seen: bool, cx: &mut App) -> bool {
    let Some(h) = me(cx) else { return false };
    let again = std::mem::take(&mut h.read_again);
    if !again || !h.model.inputs.enabled {
        h.reading = false;
    }
    if !h.model.inputs.enabled {
        return false;
    }
    if h.model.inputs.maximized != seen {
        h.model.inputs.maximized = seen;
        if !seen {
            h.model.inputs.near = false;
        }
        log::debug!("dock: a maximised window is {} on the dock's monitor", if seen { "there" } else { "gone" });
        sync_poller(cx);
        reevaluate(cx);
    }
    again
}

/// The cursor is looked at while, and only while, the dock can be thin or its band is still the thin one (`Model::needs_polling`).
fn sync_poller(cx: &mut App) {
    let Some(h) = me(cx) else { return };
    let wanted = h.model.needs_polling();
    if !wanted {
        h.poller = None;
        h.poll_alive = false;
        return;
    }
    if h.poll_alive {
        return;
    }
    h.poll_alive = true;
    let task = cx.spawn(async move |cx| {
        let mut wait = POLL_MS;
        loop {
            cx.background_executor().timer(Duration::from_millis(wait)).await;
            match cx.update(poll) {
                Some(next) => wait = next,
                None => break,
            }
        }
    });
    if let Some(h) = me(cx) {
        h.poller = Some(task);
    }
}

/// One look at the cursor and at what is open. Gives the time to the next look (ms); `None`: nothing more to look at.
fn poll(cx: &mut App) -> Option<u64> {
    let pointer = zoomwatch::pointer();
    let open = open_anything(cx);
    let Some(h) = me(cx) else { return None };
    if !h.model.needs_polling() {
        h.poll_alive = false;
        return None;
    }
    // (no zone known: neither near nor away; no cursor position, as on a locked desktop: away)
    let inside = h.zone.map(|z| pointer.is_some_and(|(x, y, _)| z.contains(x, y)));
    let button = pointer.is_some_and(|(_, _, down)| down);
    if h.model.observe(inside, button, open) {
        reevaluate(cx);
    }
    Some(next_poll(cx.try_global::<ThinHost>().is_some_and(|h| h.model.inputs.near)))
}

/// The time to the next look at the cursor: every frame while the pointer is at the dock, else [`POLL_MS`].
fn next_poll(near: bool) -> u64 {
    if near { POLL_NEAR_MS } else { POLL_MS }
}

/// A panel, the app menu or a hover preview of the dock is open.
fn open_anything(cx: &App) -> bool {
    super::panel::is_open(cx) || super::app_menu::is_open(cx) || super::hover_preview::is_shown(cx)
}

/// Works out thin or full and the band from the facts as they are now, starts the motion if the answer changed, sets the band if its
/// rule says so and asks for a look at the next deadline.
fn reevaluate(cx: &mut App) {
    let open = open_anything(cx);
    let Some(h) = me(cx) else { return };
    h.model.inputs.open = open;
    let now = h.now_ms();
    let look = h.model.look(now);
    let mut motion = false;
    if look.thin == h.grow.is_in() {
        h.grow.set_duration(plan::hover_duration(crate::settings::animations()));
        if look.thin {
            h.grow.leave(now);
        } else {
            h.grow.enter(now);
        }
        motion = true;
    }
    // (the band follows the window, not the pointer, and waits while a panel, a menu or a preview is open: see `Model::look`)
    let (band, wake) = (look.band, look.wake_at);
    h.wake_gen += 1;
    let gen_now = h.wake_gen;
    let waker = wake.map(|at| {
        let wait = Duration::from_millis(at.saturating_sub(now));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            cx.update(|cx| {
                if cx.try_global::<ThinHost>().is_some_and(|h| h.wake_gen == gen_now) {
                    reevaluate(cx);
                }
            });
        })
    });
    if let Some(h) = me(cx) {
        h.waker = waker;
    }
    if motion {
        // (a view that is being updated right now must not be updated again: after the current effects)
        cx.defer(|cx| {
            if let Some(m) = host::middle_island(cx) {
                m.update(cx, |_, cx| cx.notify());
            }
        });
    }
    if band.changed {
        log::info!("dock: the reserved band is now the {} one", if band.thin { "thin" } else { "full" });
        host::apply_reserve(cx);
    }
}

#[cfg(test)]
mod tests {
    // (explicit names: `super::*` would bring the toolkit's own `test` attribute along and shadow the standard one)
    use super::{ENABLED, Ordering, Slot, Squeeze, fit_glass, picture, plan, task_changed};
    use rf_win::dock::layout::{self, ICON_REST};
    use rf_win::dock::magnify::{self, Frame};

    const BASELINE: f32 = 12.0;

    fn row() -> (Vec<Slot>, Vec<(f32, f32)>, f32) {
        let slots = vec![Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon];
        let width = plan::middle_width_at_rest(4, 1);
        (slots.clone(), plan::rest_layout(&slots, width), width)
    }

    #[test]
    fn at_the_full_squeeze_the_picture_is_the_one_of_today() {
        let (slots, rest, width) = row();
        let margins = [44.0, 12.0, 44.0, 4.0];
        for frame in [Frame::at_rest(&rest, width), magnify::frame(&slots, &rest, width, 60.0, 1.0, magnify::Strength::Full, width + 80.0)] {
            assert_eq!(picture(&frame, &slots, margins, BASELINE, &Squeeze::FULL), plan::picture(&frame, &slots, margins, BASELINE));
        }
    }

    #[test]
    fn the_thin_picture_keeps_the_bottom_edge_the_width_and_every_icons_centre() {
        let (slots, rest, width) = row();
        let margins = [44.0, 12.0, 44.0, 4.0];
        let frame = Frame::at_rest(&rest, width);
        let full = plan::picture(&frame, &slots, margins, BASELINE);
        let thin = picture(&frame, &slots, margins, BASELINE, &Squeeze::at(0.0));
        // the glass: the same bottom edge, the same left edge and width, 48 high instead of 72
        assert_eq!(thin.glass[0], full.glass[0]);
        assert_eq!(thin.glass[2], full.glass[2]);
        assert_eq!(thin.glass[1] + thin.glass[3], full.glass[1] + full.glass[3]);
        assert_eq!(thin.glass[3], 48.0);
        for ((s, a), b) in slots.iter().zip(&full.items).zip(&thin.items) {
            match s {
                Slot::Icon => {
                    assert!((a.0 + a.1 / 2.0 - (b.0 + b.1 / 2.0)).abs() < 1e-4, "the centre moved: {a:?} {b:?}");
                    assert!((b.1 - 30.0).abs() < 1e-4, "an icon is 30 units thin");
                }
                Slot::Separator => assert_eq!(a, b),
            }
        }
        // nothing sticks out of the thin glass, and the icons stand inside it
        assert!(thin.out_of_glass.is_empty());
        let foot = thin.glass[1] + thin.glass[3] - BASELINE;
        for (s, &(_, w)) in slots.iter().zip(&thin.items) {
            if *s == Slot::Icon {
                assert!(foot - w >= thin.glass[1], "the icon's top is inside the glass");
            }
        }
    }

    #[test]
    fn a_point_on_a_thin_icon_stays_on_that_icon_all_through_the_growth() {
        // "the icon the user aims at stays under the pointer": the icons grow about their own centres and up from the same foot, so
        // every point of a thin icon is a point of the same icon at every moment of the growth, and at the end
        let (slots, rest, width) = row();
        let margins = [44.0, 12.0, 44.0, 4.0];
        let frame = Frame::at_rest(&rest, width);
        let rect = |sq: &Squeeze, ix: usize| {
            let p = picture(&frame, &slots, margins, BASELINE, sq);
            let (x, w) = p.items[ix];
            let foot = margins[1] + layout::MIDDLE_HEIGHT - BASELINE;
            (x, foot - w, x + w, foot)
        };
        for ix in [0usize, 1, 3, 4] {
            let (l0, t0, r0, b0) = rect(&Squeeze::at(0.0), ix);
            for k in 1..=20 {
                let (l, t, r, b) = rect(&Squeeze::at(k as f32 / 20.0), ix);
                assert!(l <= l0 + 1e-3 && t <= t0 + 1e-3 && r >= r0 - 1e-3 && (b - b0).abs() < 1e-3, "icon {ix} at step {k}: ({l},{t},{r},{b}) does not cover ({l0},{t0},{r0},{b0})");
            }
        }
    }

    #[test]
    fn the_icon_under_the_pointer_does_not_move_sideways_while_the_dock_grows() {
        // the pointer is over the third icon: its centre has the same x from the thin dock to the full one (the bell aside)
        let (slots, rest, width) = row();
        let margins = [44.0, 12.0, 44.0, 4.0];
        let frame = Frame::at_rest(&rest, width);
        let centre = |sq: &Squeeze| {
            let p = picture(&frame, &slots, margins, BASELINE, sq);
            p.items[3].0 + p.items[3].1 / 2.0
        };
        let at_thin = centre(&Squeeze::at(0.0));
        for k in 0..=20 {
            assert!((centre(&Squeeze::at(k as f32 / 20.0)) - at_thin).abs() < 1e-3, "step {k}");
        }
    }

    #[test]
    fn the_icon_the_pointer_aims_at_is_still_under_it_at_every_step_with_the_bell_coming_in() {
        // The real growth: the squeeze AND the magnification's bell (`Squeeze::bell`) come in together, the glass widens around the
        // island's centre and the icons beside the pointer swell. Whatever icon the pointer is over when the dock is thin is under it
        // at every step to the full dock, wherever on the icon it is (the pointer's x is in the frame's coordinates).
        let (slots, rest, width) = row();
        let margins = [44.0, 12.0, 44.0, 4.0];
        for ix in [0usize, 1, 3, 4] {
            let (rx, rw) = rest[ix];
            for frac in [0.1f32, 0.3, 0.5, 0.7, 0.9] {
                let pointer = rx + rw * frac;
                // (thin, the icon is 30/36 of its rest size around the same centre: the pointer is over it for frac within that)
                let thin_w = rw * Squeeze::at(0.0).icon;
                let thin_l = rx + (rw - thin_w) / 2.0;
                if pointer < thin_l || pointer > thin_l + thin_w {
                    continue;
                }
                for k in 0..=20 {
                    let sq = Squeeze::at(k as f32 / 20.0);
                    let frame = magnify::frame(&slots, &rest, width, pointer, sq.bell(1.0), magnify::Strength::Full, width + 80.0);
                    let p = picture(&frame, &slots, margins, BASELINE, &sq);
                    let (l, w) = (p.items[ix].0 - margins[0], p.items[ix].1);
                    assert!(pointer >= l - 1e-3 && pointer <= l + w + 1e-3, "icon {ix}, pointer at {frac} of it, step {k}: the icon spans {l}..{} and the pointer is at {pointer}", l + w);
                }
            }
        }
    }

    #[test]
    fn the_glass_in_device_px_is_exactly_the_side_islands_when_thin_and_the_layouts_when_full() {
        for (unit, full_px) in [(1.0, 72.0), (1.25, 90.0), (1.5, 108.0), (0.85, 61.0), (1.4 * 1.1, 111.0)] {
            let bottom = 1000.0;
            let thin = Squeeze::at(0.0);
            let mut g = [10.0, bottom - full_px, 400.0, full_px];
            fit_glass(&mut g, bottom, &thin, unit, full_px);
            assert_eq!(g[3], layout::px(layout::SIDE_HEIGHT, unit) as f32, "unit {unit}");
            assert_eq!(g[1] + g[3], bottom);
            // full: untouched
            let mut f = [10.0, bottom - full_px, 400.0, full_px];
            fit_glass(&mut f, bottom, &Squeeze::FULL, unit, full_px);
            assert_eq!(f, [10.0, bottom - full_px, 400.0, full_px]);
            // in between: monotonic, bottom fixed
            let mut last = g[3];
            // (at 1.0 the squeeze is the full one and the glass is not touched at all)
            for k in 1..10 {
                let mut m = [10.0, 0.0, 400.0, 0.0];
                fit_glass(&mut m, bottom, &Squeeze::at(k as f32 / 10.0), unit, full_px);
                assert!(m[3] >= last - 1e-3 && m[3] <= full_px + 1e-3);
                assert!((m[1] + m[3] - bottom).abs() < 1e-3);
                last = m[3];
            }
        }
    }

    #[test]
    fn the_thin_icons_are_a_part_of_the_rest_size() {
        assert!((Squeeze::at(0.0).icon * ICON_REST - 30.0).abs() < 1e-4);
    }

    #[test]
    fn the_setting_is_on_by_default_and_a_file_from_before_it_reads_as_on_and_off_is_kept() {
        use crate::settings::Settings;
        assert!(Settings::default().dock_thin_when_maximized);
        assert!(Settings::parse_lenient_pub("theme = \"dark\"
").unwrap().dock_thin_when_maximized);
        let off = Settings { dock_thin_when_maximized: false, ..Default::default() };
        let back = Settings::parse_lenient_pub(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_thin_when_maximized);
        assert!(Settings::parse_lenient_pub("dock_thin_when_maximized = 3
").unwrap().dock_thin_when_maximized, "a wrong value falls back to the default");
    }

    #[test]
    fn a_dock_with_the_setting_off_has_no_news_to_tell() {
        // nothing is listening: the call returns, and nothing is left behind
        ENABLED.store(false, Ordering::Relaxed);
        task_changed();
    }
}
