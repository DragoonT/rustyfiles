//! The thin dock, app side (the rules are `rf_win::dock::thin`, tested without a window): while a window the user can see stands on the
//! band on the dock's monitor (a maximised one, or one docked to the band's edge under the dock) the middle island is as high as the
//! side islands; the pointer coming near grows it back over the window; it goes thin again the moment the pointer has left and nothing
//! of the dock is open (in one motion with the magnification going out: a user saw two steps when it waited 400 ms). THE BAND the dock
//! reserves is the thin one for as long as the setting is on and does not follow the windows: a window is maximised into the room it
//! keeps, in one step (`rf_win::dock::thin::BandRule` says why).
//! Setting "Pencere büyütüldüğünde dock'u incelt" (`dock_thin_when_maximized`, on by default, a key of the running dock: off is the dock
//! of today and costs nothing but one atomic read per task-list event and per mouse move over the dock).
//!
//! EACH MONITOR ON ITS OWN. With a dock on every monitor each dock has its own thin state ([`MonThin`]): it is thin while a maximised
//! window is on ITS monitor, it grows when the pointer comes near ITS glass, and what is open counts for it only when the open panel, menu
//! or preview is on its monitor. The windows are read once for all of them (one walk over the windows for each dock's monitor), the
//! placement hook and the cursor look are shared. The BAND: the home dock's is the desktop toolbar's (or the primary monitor's work area
//! while Explorer's taskbar is hidden by us); a secondary dock's, thin or full by ITS state, is told to the host that holds the taskbar
//! hidden and reaches that monitor's work area only then (`host::apply_secondary_bands`).
//!
//! WHERE THE FACTS COME FROM.
//! * A maximised window: the task list's events the dock already follows (`MiddleIsland` calls [`task_changed`] from its watch) and the
//!   placement events of `rf_win::dock::zoomwatch` (maximise, restore, snap; started only while the setting is on). Either one asks for
//!   ONE walk over the windows on a worker thread (`zoomwatch::stands_on_band`), at most one at a time and one more if events came while it
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
//! their centres, the glass keeps its bottom edge and its width. The band is another matter: it follows the setting alone
//! (`rf_win::dock::thin::BandRule`).

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

use super::host::{self, Kind, MonId};
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

/// One dock's thin state.
struct MonThin {
    mon: MonId,
    /// The facts (the setting, a maximised window seen, near, open) and the two rules; every sequence of them is tested in rf_win.
    model: Model,
    /// Where "near" is, from the last layout.
    zone: Option<Zone>,
    /// 1: the full dock, 0: the thin one. Starts full and settled.
    grow: Presence,
    waker: Option<Task<()>>,
    wake_gen: u64,
}

impl MonThin {
    fn new(mon: MonId, enabled: bool) -> MonThin {
        let mut grow = Presence::new(plan::hover_duration(crate::settings::animations()));
        grow.enter(0);
        let mut model = Model::new();
        model.inputs.enabled = enabled;
        MonThin { mon, model, zone: None, grow, waker: None, wake_gen: 0 }
    }
}

struct ThinHost {
    mons: Vec<MonThin>,
    /// The setting is on.
    enabled: bool,
    t0: Instant,
    watch: Option<ZoomWatch>,
    reading: bool,
    read_again: bool,
    poll_alive: bool,
    poller: Option<Task<()>>,
}

impl Global for ThinHost {}

impl ThinHost {
    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    fn mon(&self, mon: MonId) -> Option<&MonThin> {
        self.mons.iter().find(|m| m.mon == mon)
    }

    fn mon_mut(&mut self, mon: MonId) -> Option<&mut MonThin> {
        self.mons.iter_mut().find(|m| m.mon == mon)
    }
}

fn me(cx: &mut App) -> Option<&mut ThinHost> {
    if cx.has_global::<ThinHost>() { Some(cx.global_mut::<ThinHost>()) } else { None }
}

// ---------------------------------------------------------------- what the middle island's view asks

/// How thin the middle island of this dock is at this moment (the full dock when the feature never made it thin).
pub fn squeeze(mon: MonId, cx: &App) -> Squeeze {
    cx.try_global::<ThinHost>().and_then(|h| h.mon(mon).map(|m| Squeeze::at(m.grow.value(h.now_ms())))).unwrap_or(Squeeze::FULL)
}

/// The island is growing or shrinking: another frame is wanted.
pub fn animating(mon: MonId, cx: &App) -> bool {
    cx.try_global::<ThinHost>().and_then(|h| h.mon(mon).map(|m| !m.grow.settled(h.now_ms()))).unwrap_or(false)
}

/// The band the dock reserves is the thin one right now. The band is the home dock's.
pub fn band_is_thin(cx: &App) -> bool {
    band_is_thin_on(MonId::HOME, cx)
}

/// The band the dock on this monitor reserves is the thin one right now: the setting is on (every dock's band follows it alone). Asked
/// before the thin dock has started (the host sets its first band earlier), the answer is the setting's too, so that the first band is
/// the one that stays and no window is moved twice at the dock's start.
pub fn band_is_thin_on(_mon: MonId, cx: &App) -> bool {
    match cx.try_global::<ThinHost>() {
        Some(h) => h.enabled,
        None => cx.try_global::<Settings>().is_some_and(|s| s.dock_thin_when_maximized),
    }
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
    let mons = host::dock_monitors(cx).into_iter().map(|(id, _)| MonThin::new(id, false)).collect();
    cx.set_global(ThinHost {
        mons,
        enabled: false,
        // (a clock that started long ago: the full dock is settled, not just arrived)
        t0: Instant::now().checked_sub(Duration::from_secs(5)).unwrap_or_else(Instant::now),
        watch: None,
        reading: false,
        read_again: false,
        poll_alive: false,
        poller: None,
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

/// The docks of the host changed (a monitor's dock came or went): each dock has a thin state of its own.
pub fn docks_changed(cx: &mut App) {
    if !cx.has_global::<ThinHost>() {
        return;
    }
    let ids: Vec<MonId> = host::dock_monitors(cx).into_iter().map(|(id, _)| id).collect();
    let Some(h) = me(cx) else { return };
    let enabled = h.enabled;
    h.mons.retain(|m| ids.contains(&m.mon));
    let have: Vec<MonId> = h.mons.iter().map(|m| m.mon).collect();
    for id in rf_win::dock::monitors::new_ids(&have, &ids) {
        h.mons.push(MonThin::new(id, enabled));
    }
    layout_applied(cx);
    if enabled {
        // (the new dock's monitor may have a maximised window already)
        read_maximized(cx);
    }
}

/// The settings were read: follow the switch.
pub fn on_settings(cx: &mut App) {
    let want = cx.try_global::<Settings>().is_some_and(|s| s.dock_thin_when_maximized);
    let Some(h) = me(cx) else { return };
    if h.enabled == want {
        return;
    }
    h.enabled = want;
    for m in &mut h.mons {
        m.model.inputs.enabled = want;
    }
    ENABLED.store(want, Ordering::Relaxed);
    if want {
        // (the watch starts its thread and returns: the UI thread does not wait for it)
        h.watch = ZoomWatch::start(task_changed).map_err(|e| log::warn!("dock: cannot watch window placement: {e:#}")).ok();
        read_maximized(cx);
        reevaluate_all(cx);
        return;
    }
    // off: the dock of today, at once
    h.watch = None;
    h.poller = None;
    h.poll_alive = false;
    let (mut home, mut other) = (false, false);
    for m in &mut h.mons {
        let thin_band = m.model.switch_off();
        home |= thin_band && m.mon.is_home();
        other |= thin_band && !m.mon.is_home();
    }
    reevaluate_all(cx);
    // the full bands are back: the home dock's is the desktop toolbar's, a second monitor's is told to the host that holds the taskbar hidden
    if home {
        host::apply_reserve(cx);
    }
    if other {
        host::apply_secondary_bands(cx);
    }
}

/// The host's layout was worked out (or worked out again): "near" is worked out from the new rectangles.
pub fn layout_applied(cx: &mut App) {
    if !cx.has_global::<ThinHost>() {
        return;
    }
    let ids: Vec<MonId> = cx.global::<ThinHost>().mons.iter().map(|m| m.mon).collect();
    for id in ids {
        let zone = zone_now(id, cx);
        // (a rectangle that cannot be read for a moment does not take the last good zone away: the pointer resting on the dock would be
        // judged away at the next look)
        if let Some(h) = me(cx)
            && zone.is_some()
            && let Some(m) = h.mon_mut(id)
        {
            m.zone = zone;
        }
    }
}

fn zone_now(mon: MonId, cx: &App) -> Option<Zone> {
    let full = host::visible_glass_rect_on(mon, Kind::Middle, cx)?;
    let (left, right) = (host::visible_glass_rect_on(mon, Kind::Left, cx)?, host::visible_glass_rect_on(mon, Kind::Right, cx)?);
    let unit = host::dock_unit_on(mon, cx)?;
    let monitor = host::dock_monitors(cx).into_iter().find(|(id, _)| *id == mon)?.1.rect;
    Some(rf_win::dock::thin::zone(full, left, right, monitor, unit))
}

/// The pointer is on this dock's island (its mouse events say so): near, at once, without waiting for the next look at the cursor.
pub fn pointer_on_island(mon: MonId, cx: &mut App) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
    let Some(h) = me(cx) else { return };
    let Some(m) = h.mon_mut(mon) else { return };
    if m.model.inputs.enabled && m.model.inputs.maximized && !m.model.inputs.near {
        m.model.inputs.near = true;
        reevaluate(mon, cx);
    }
}

// ---------------------------------------------------------------- the work

/// What a walk asks: each dock, its monitor, and the full glass a docked window is measured against (`host::full_glass_span_on`: the
/// middle island's, and the side islands' too when they are as tall as it).
fn what_to_ask(cx: &mut App) -> Vec<(MonId, isize, Option<rf_win::dock::layout::Rect>)> {
    host::dock_monitors(cx).into_iter().map(|(id, m)| (id, m.handle, host::full_glass_span_on(id, cx))).collect()
}

/// A walk over the windows, off the UI thread; at most one at a time. One question for each dock's monitor.
fn read_maximized(cx: &mut App) {
    let Some(h) = me(cx) else { return };
    if !h.enabled {
        return;
    }
    if h.reading {
        h.read_again = true;
        return;
    }
    h.reading = true;
    cx.spawn(async move |cx| {
        loop {
            // (asked anew for every walk: a dock may have come, gone or changed its size since the last one)
            let asked = cx.update(what_to_ask);
            let seen: Vec<(MonId, bool)> = cx
                .background_executor()
                .spawn(async move {
                    asked
                        .into_iter()
                        .map(|(id, handle, glass)| {
                            // (the home dock follows the primary monitor, whichever handle it has now)
                            let handle = if id.is_home() { rf_win::dock::primary_monitor().map_or(handle, |p| p.handle) } else { handle };
                            (id, zoomwatch::stands_on_band(handle, glass))
                        })
                        .collect()
                })
                .await;
            if !cx.update(|cx| apply_maximized(seen, cx)) {
                break;
            }
            cx.background_executor().timer(Duration::from_millis(READ_GAP_MS)).await;
        }
    })
    .detach();
}

/// The answer of a walk. True: events came meanwhile, walk again.
fn apply_maximized(seen: Vec<(MonId, bool)>, cx: &mut App) -> bool {
    let Some(h) = me(cx) else { return false };
    let again = std::mem::take(&mut h.read_again);
    if !again || !h.enabled {
        h.reading = false;
    }
    if !h.enabled {
        return false;
    }
    let mut changed = Vec::new();
    for (id, now) in seen {
        if let Some(m) = h.mon_mut(id)
            && m.model.inputs.maximized != now
        {
            m.model.inputs.maximized = now;
            if !now {
                m.model.inputs.near = false;
            }
            log::debug!("dock: a window on the band is {} on the monitor of dock {}", if now { "there" } else { "gone" }, id.0);
            changed.push(id);
        }
    }
    if !changed.is_empty() {
        sync_poller(cx);
        for id in changed {
            reevaluate(id, cx);
        }
    }
    again
}

/// The cursor is looked at while, and only while, a dock can be thin: a window stands on its band (`Model::needs_polling`).
fn sync_poller(cx: &mut App) {
    let Some(h) = me(cx) else { return };
    let wanted = h.mons.iter().any(|m| m.model.needs_polling());
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
    let active = host::active(cx);
    let h = me(cx)?;
    if !h.mons.iter().any(|m| m.model.needs_polling()) {
        h.poll_alive = false;
        return None;
    }
    let button = pointer.is_some_and(|(_, _, down)| down);
    let mut changed = Vec::new();
    for m in &mut h.mons {
        if !m.model.needs_polling() {
            continue;
        }
        // (no zone known: neither near nor away; no cursor position, as on a locked desktop: away)
        let inside = m.zone.map(|z| pointer.is_some_and(|(x, y, _)| z.contains(x, y)));
        // (a panel, a menu or a preview keeps the dock it belongs to grown, and no other)
        if m.model.observe(inside, button, open && active == m.mon) {
            changed.push(m.mon);
        }
    }
    for id in changed {
        reevaluate(id, cx);
    }
    Some(next_poll(cx.try_global::<ThinHost>().is_some_and(|h| h.mons.iter().any(|m| m.model.inputs.near))))
}

/// The time to the next look at the cursor: every frame while the pointer is at a dock, else [`POLL_MS`].
fn next_poll(near: bool) -> u64 {
    if near { POLL_NEAR_MS } else { POLL_MS }
}

/// A panel, the app menu or a hover preview of the dock is open.
fn open_anything(cx: &App) -> bool {
    super::panel::is_open(cx) || super::app_menu::is_open(cx) || super::hover_preview::is_shown(cx)
}

fn reevaluate_all(cx: &mut App) {
    let ids: Vec<MonId> = cx.try_global::<ThinHost>().map(|h| h.mons.iter().map(|m| m.mon).collect()).unwrap_or_default();
    for id in ids {
        reevaluate(id, cx);
    }
}

/// Works out thin or full and the band for one dock from the facts as they are now, starts the motion if the answer changed, sets the band
/// if its rule says so (the home dock's) and asks for a look at the next deadline.
fn reevaluate(mon: MonId, cx: &mut App) {
    let open = open_anything(cx) && host::active(cx) == mon;
    let Some(h) = me(cx) else { return };
    let now = h.now_ms();
    let Some(m) = h.mon_mut(mon) else { return };
    m.model.inputs.open = open;
    let look = m.model.look(now);
    let mut motion = false;
    if look.thin == m.grow.is_in() {
        m.grow.set_duration(plan::hover_duration(crate::settings::animations()));
        if look.thin {
            m.grow.leave(now);
        } else {
            m.grow.enter(now);
        }
        motion = true;
    }
    // (the band follows the setting alone: it changes here only when the setting was switched on)
    let (band, wake) = (look.band, look.wake_at);
    m.wake_gen += 1;
    let gen_now = m.wake_gen;
    let waker = wake.map(|at| {
        let wait = Duration::from_millis(at.saturating_sub(now));
        cx.spawn(async move |cx| {
            cx.background_executor().timer(wait).await;
            cx.update(|cx| {
                if cx.try_global::<ThinHost>().and_then(|h| h.mon(mon)).is_some_and(|m| m.wake_gen == gen_now) {
                    reevaluate(mon, cx);
                }
            });
        })
    });
    if let Some(m) = me(cx).and_then(|h| h.mon_mut(mon)) {
        m.waker = waker;
    }
    if motion {
        // (a view that is being updated right now must not be updated again: after the current effects)
        cx.defer(move |cx| {
            if let Some(m) = host::middle_island_on(mon, cx) {
                m.update(cx, |_, cx| cx.notify());
            }
        });
    }
    if band.changed {
        log::info!("dock: the reserved band{} is now the {} one", if mon.is_home() { "" } else { " of a second monitor" }, if band.thin { "thin" } else { "full" });
        if mon.is_home() {
            host::apply_reserve(cx);
        } else {
            // (a second monitor's dock: the band is its own, and only the host that holds the taskbar hidden is told)
            host::apply_secondary_bands(cx);
        }
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
