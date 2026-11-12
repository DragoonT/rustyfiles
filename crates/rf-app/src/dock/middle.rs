//! The middle island, the dock itself: the apps pinned to the Windows taskbar in the taskbar's order, then the other running apps,
//! a thin separator, the Recycle Bin and the Downloads folder. Icons are 44 px at rest. With the pointer over the dock they grow like
//! the Mac's dock (`rf_win::dock::magnify`): by a smooth bell of the distance from the pointer to each icon's rest centre, up to 1.55
//! times for the one under it (a setting: off / subtle 1.25 / full 1.55), standing on one baseline so they grow UP, over the glass's top
//! edge, into the window's headroom, and pushing their neighbours apart. The glass widens with them, symmetrically around the island's
//! centre (its height never changes); coming in and going out are eased over about 160 ms, instantly when the app's animations are off.
//!
//! THE WINDOW IS BIGGER THAN THE GLASS (`layout::placements`): this view lays everything out in the whole window, from the margins the
//! host gives it (`host::insets`), and for every frame tells the glass engine where the glass is and the system which parts of the
//! window take the pointer (`host::set_middle_frame`): the glass and the icons that stand out of it, nothing else.
//!
//! At rest there are no timers and no frames. While the pointer moves over the dock: one layout, one shape for the engine, one region
//! for the system per frame; the icons are never loaded again for it.
//!
//! Left click: start the app / bring its last used window forward / minimise it when it is in front. Middle click: another instance.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::hover::Slot;
use rf_win::dock::layout::{ICON_REST, RADIUS};
use rf_win::dock::magnify::{self, Frame, Presence};
use rf_win::dock::{DockItem, IconSource, PinnedApp, Place, hit, model};
use rf_win::tasklist::{self, Task, TaskWatch};

use super::backdrop::IslandGlass;
use super::host::{self, Kind};
use super::icons::{self, IconCache};
use super::look::{self, Env};
use super::plan;
use crate::settings::Settings;
use crate::t;

/// How far the icons' bottom edge is above the island's bottom edge (logical px): every icon, grown or not, stands on this line, so
/// the running marks below them do not move.
const BASELINE: f32 = 12.0;
/// The running mark's distance below the icons.
const DOT_DROP: f32 = 7.0;
/// A pointer move of less than this many logical px while the bell is settled does not draw a frame.
const MIN_MOVE: f32 = 0.25;
/// How often the taskbar's pins are read again (they change when the user pins or unpins in Windows).
const PINS_EVERY: Duration = Duration::from_secs(20);

pub struct MiddleIsland {
    glass: Rc<RefCell<IslandGlass>>,
    pins: Vec<PinnedApp>,
    pins_loaded: bool,
    items: Vec<DockItem>,
    slots: Vec<Slot>,
    /// Per item: the key its icon is cached under and where the icon comes from (worked out once per refresh, not per frame).
    icon_srcs: Vec<Option<(String, IconSource)>>,
    icons: IconCache,
    /// The scale the cached icons were asked for.
    icon_scale: f32,
    /// Where the pointer last was over the dock (logical px from the REST island's left edge); `None` before it first came. It stays
    /// where it was when the pointer leaves, so the bell shrinks away in place.
    pointer_x: Option<f32>,
    /// The height of the bell over time (0 away, 1 over the dock).
    presence: Presence,
    /// Clock of the animation.
    t0: Instant,
    reported: f32,
    /// While a mouse button is down on the dock: where the pointer and the bell were when it went down. The icons stand still until it
    /// goes up, so the icon that was pressed is still under the pointer when the press ends and the click is not lost to the bell
    /// moving the icons about (a click is a press and a release on the same icon).
    frozen: Option<(f32, f32)>,
    /// A refresh is running; a change that arrives meanwhile makes it run once more.
    refreshing: bool,
    dirty: bool,
    _watch: Option<TaskWatch>,
}

impl MiddleIsland {
    pub fn new(glass: Rc<RefCell<IslandGlass>>, cx: &mut Context<Self>) -> Self {
        // task windows opening, closing or being activated: ask for a refresh (the callback runs on another thread). A window that
        // only changes its title (a terminal, a loading page) is not news: the dock shows no titles.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let watch = tasklist::watch(move |changes| {
            if changes.list || changes.active {
                let _ = tx.unbounded_send(());
            }
        })
        .map_err(|e| log::warn!("dock: cannot watch the task windows: {e:#}"))
        .ok();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                if this.update(cx, |me, cx| me.refresh(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        // the taskbar's pins: read now and then again, off the UI thread
        cx.spawn(async move |this, cx| {
            loop {
                let pins = cx.background_executor().spawn(async { rf_win::dock::pins::read_pins() }).await;
                let alive = this.update(cx, |me, cx| {
                    if !me.pins_loaded || me.pins != pins {
                        me.pins = pins;
                        me.pins_loaded = true;
                        me.refresh(cx);
                    }
                });
                if alive.is_err() {
                    break;
                }
                cx.background_executor().timer(PINS_EVERY).await;
            }
        })
        .detach();
        let mut me = MiddleIsland {
            glass,
            pins: Vec::new(),
            pins_loaded: false,
            items: Vec::new(),
            slots: Vec::new(),
            icon_srcs: Vec::new(),
            icons: IconCache::default(),
            icon_scale: cx.global::<Env>().scale,
            pointer_x: None,
            presence: Presence::new(plan::hover_duration(crate::settings::animations())),
            t0: Instant::now(),
            reported: 0.0,
            frozen: None,
            refreshing: false,
            dirty: false,
            _watch: watch,
        };
        // until the first refresh is done the dock shows what it always has: the separator and the two places
        me.items = model::merge(&[], Vec::new(), 0);
        me.slots = model::slots(&me.items);
        me.icon_srcs = icon_sources(&me.items);
        me.reported = me.width();
        me
    }

    fn now_ms(&self) -> u64 {
        self.t0.elapsed().as_millis() as u64
    }

    /// The island's width at rest (logical px).
    pub fn width(&self) -> f32 {
        plan::middle_width_at_rest(icon_count(&self.slots), self.slots.iter().filter(|s| **s == Slot::Separator).count())
    }

    /// Looks at the running windows again and rebuilds the items. The windows are listed on a worker thread; several requests that
    /// come close together run as one more round.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.refreshing {
            self.dirty = true;
            return;
        }
        self.refreshing = true;
        cx.spawn(async move |this, cx| {
            loop {
                let groups = cx.background_executor().spawn(async { tasklist::grouped(tasklist::tasks()) }).await;
                let foreground = rf_win::dock::foreground_root();
                let again = this.update(cx, |me, cx| {
                    me.apply(groups, foreground, cx);
                    let again = me.dirty;
                    me.dirty = false;
                    me.refreshing = again;
                    again
                });
                if !matches!(again, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    fn apply(&mut self, groups: Vec<(String, Vec<Task>)>, foreground: isize, cx: &mut Context<Self>) {
        self.items = model::merge(&self.pins, groups, foreground);
        self.slots = model::slots(&self.items);
        self.icon_srcs = icon_sources(&self.items);
        self.load_icons(cx);
        let w = self.width();
        if (w - self.reported).abs() > 0.5 {
            self.reported = w;
            super::host::set_width(Kind::Middle, w, cx);
        }
        cx.notify();
    }

    /// Asks for every icon not yet asked for (and every one that failed a while ago), on worker threads.
    fn load_icons(&mut self, cx: &mut Context<Self>) {
        let scale = cx.global::<Env>().scale;
        if (scale - self.icon_scale).abs() > 0.001 {
            // the bitmaps were made for another scale: ask again for all of them
            self.icon_scale = scale;
            self.icons.clear();
        }
        let px = plan::icon_request_px(scale);
        let wanted: Vec<(String, IconSource)> = self.icon_srcs.iter().flatten().cloned().collect();
        for (key, src) in wanted {
            if !self.icons.start_loading(&key, Instant::now()) {
                continue;
            }
            cx.spawn(async move |this, cx| {
                let asked = src.clone();
                let bgra = cx.background_executor().spawn(async move { icons::load_bgra(&asked, px) }).await;
                let retry = this.update(cx, |me, cx| {
                    let retry = me.icons.finish(&key, bgra.and_then(icons::to_image), Instant::now());
                    cx.notify();
                    retry
                });
                if matches!(retry, Ok(true)) {
                    // the shell may just not have been ready: look again after a while
                    cx.background_executor().timer(icons::RETRY_AFTER).await;
                    let _ = this.update(cx, |me, cx| me.load_icons(cx));
                }
            })
            .detach();
        }
    }

    /// The pointer is over the dock at `x`, logical px from the REST island's left edge: the bell follows it directly. `strength` is the
    /// setting; `button_down` whether a mouse button is held.
    fn pointer_moved(&mut self, x: f32, strength: magnify::Strength, button_down: bool, cx: &mut Context<Self>) {
        // a move with no button held ends a press whose release this view never saw (it happened outside the window)
        if !button_down && self.frozen.take().is_some() {
            cx.notify();
        }
        // With the magnification off nothing here changes what is drawn: no frame, no animation, no look at the animations setting.
        if strength == magnify::Strength::Off {
            return;
        }
        let now = self.now_ms();
        let entering = !self.presence.is_in();
        let delta = self.pointer_x.map_or(f32::INFINITY, |old| (old - x).abs());
        self.pointer_x = Some(x);
        if entering {
            // the setting is looked at when the bell starts, not on every mouse event (it is a system call)
            self.presence.set_duration(plan::hover_duration(crate::settings::animations()));
            self.presence.enter(now);
        }
        // a hair of movement while the bell is settled is not worth a frame: the sizes would not differ by a pixel
        if move_needs_frame(strength, entering, delta, self.presence.settled(now)) {
            cx.notify();
        }
    }

    fn pointer_left(&mut self, cx: &mut Context<Self>) {
        self.frozen = None;
        if self.presence.is_in() {
            let now = self.now_ms();
            self.presence.set_duration(plan::hover_duration(crate::settings::animations()));
            self.presence.leave(now);
            cx.notify();
        }
    }

    /// A button went down on the dock: the icons stand still until it is up.
    fn pressed(&mut self) {
        let now = self.now_ms();
        self.frozen = self.pointer_x.map(|x| (x, self.presence.value(now)));
    }

    fn released(&mut self, cx: &mut Context<Self>) {
        if self.frozen.take().is_some() {
            cx.notify();
        }
    }

    fn click(&mut self, ix: usize) {
        match self.items.get(ix) {
            Some(DockItem::App(app)) => super::actions::click_app(app, rf_win::dock::foreground_root()),
            Some(DockItem::Place(p)) => super::actions::open_place(*p),
            _ => {}
        }
    }

    fn middle_click(&mut self, ix: usize) {
        if let Some(DockItem::App(app)) = self.items.get(ix) {
            super::actions::middle_click_app(app);
        }
    }
}

/// Whether a pointer move over the dock is worth a frame: never with the magnification off; always when the bell starts or is still
/// moving; otherwise only when the pointer moved by `MIN_MOVE` (logical px) or more.
fn move_needs_frame(strength: magnify::Strength, entering: bool, delta: f32, settled: bool) -> bool {
    strength != magnify::Strength::Off && (entering || !settled || delta >= MIN_MOVE)
}

fn icon_count(slots: &[Slot]) -> usize {
    slots.iter().filter(|s| **s == Slot::Icon).count()
}

fn item_icon(item: &DockItem) -> Option<IconSource> {
    match item {
        DockItem::App(a) => Some(a.icon.clone()),
        DockItem::Place(p) => Some(p.icon_source()),
        DockItem::Separator => None,
    }
}

/// The icon key and source of every item, in the items' order. Worked out when the items change: the Downloads folder's place is a
/// call to the shell, and a key is a string, neither of which belongs in a frame.
fn icon_sources(items: &[DockItem]) -> Vec<Option<(String, IconSource)>> {
    items.iter().map(|i| item_icon(i).map(|src| (plan::icon_key(&src), src))).collect()
}

/// The name a placeholder takes its letter from.
fn item_name(item: &DockItem) -> String {
    match item {
        DockItem::App(a) => a.name.clone(),
        DockItem::Place(Place::RecycleBin) => t!("Geri dönüşüm kutusu").to_string(),
        DockItem::Place(Place::Downloads) => t!("İndirilenler").to_string(),
        DockItem::Separator => String::new(),
    }
}

impl Render for MiddleIsland {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let env = *cx.global::<Env>();
        let fallback = self.glass.borrow().fallback();
        let dark = env.dark;
        let strength = cx.global::<Settings>().dock_magnify.strength();
        let now = self.now_ms();
        if !self.presence.settled(now) && self.frozen.is_none() {
            // the bell is still coming in or going out: one more frame
            window.request_animation_frame();
        }
        let inset = host::insets(Kind::Middle, cx);
        let [ml, _, mr, mb] = inset.margins;
        // The window may be narrower than the icons want (the side islands and the screen leave it less): lay the icons out for the
        // island's real width at rest (the window less its margins), so none falls outside it.
        let size = window.viewport_size();
        let (vw, vh) = (f32::from(size.width), f32::from(size.height));
        let real = vw - ml - mr;
        let rest_width = if real > 0.0 { real.min(self.width()) } else { self.width() };
        let rest = plan::rest_layout(&self.slots, rest_width);
        // (a pressed button holds the sizes it went down with)
        let (pointer, height) = match self.frozen {
            Some((x, h)) => (Some(x), h),
            None => (self.pointer_x, self.presence.value(now)),
        };
        let frame = match pointer {
            Some(x) if height > 0.0 && strength != magnify::Strength::Off => magnify::frame(&self.slots, &rest, rest_width, x, height, strength, rest_width + 2.0 * inset.growth),
            _ => Frame::at_rest(&rest, rest_width),
        };
        let pic = plan::picture(&frame, &self.slots, inset.margins, BASELINE);

        // the glass and the part of the window that takes the pointer follow the frame (only what moved is sent on)
        let scale = env.scale;
        let boxes: Vec<[f32; 4]> = pic.out_of_glass.iter().map(|b| plan::to_device(*b, scale)).collect();
        let glass_px = plan::to_device(pic.glass, scale);
        let parts = hit::parts(glass_px, rf_win::dock::layout::px(RADIUS, scale) as f32, &boxes, (vw * scale).round() as i32, (vh * scale).round() as i32);
        host::set_middle_frame(&self.glass, glass_px, parts, cx);

        let mut cells: Vec<AnyElement> = Vec::with_capacity(self.items.len());
        for (ix, (item, &(x, w))) in self.items.iter().zip(&pic.items).enumerate() {
            let cell = match item {
                DockItem::Separator => div()
                    .absolute()
                    .left(px(x))
                    .bottom(px(mb + BASELINE + (ICON_REST - 36.0) / 2.0))
                    .w(px(w))
                    .h(px(36.))
                    .bg(look::separator(dark).hsla())
                    .into_any_element(),
                DockItem::App(_) | DockItem::Place(_) => {
                    let picture: AnyElement = match self.icon_srcs.get(ix).and_then(|k| k.as_ref()).and_then(|(key, _)| self.icons.get(key)) {
                        Some(img) => gpui_kit::img(img).size_full().into_any_element(),
                        None => div()
                            .size_full()
                            .rounded(px(w * 0.24))
                            .bg(look::placeholder_fill(dark).hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(w * 0.45))
                            .child(plan::initial(&item_name(item)))
                            .into_any_element(),
                    };
                    let (running, in_front) = match item {
                        DockItem::App(a) => (a.running(), a.active),
                        _ => (false, false),
                    };
                    div()
                        .id(("dock-item", ix))
                        .absolute()
                        .left(px(x))
                        .bottom(px(mb + BASELINE))
                        .w(px(w))
                        .h(px(w))
                        .cursor_pointer()
                        .child(picture)
                        .when(running, |d| {
                            let (dw, dh) = look::running_dot_size(in_front);
                            // the mark stays under its icon: it is the icon's child, and the icons stand on one baseline
                            d.child(
                                div()
                                    .absolute()
                                    .bottom(px(-DOT_DROP))
                                    .left(px((w - dw) / 2.0))
                                    .w(px(dw))
                                    .h(px(dh))
                                    .rounded_full()
                                    .bg(look::running_dot(dark, in_front).hsla()),
                            )
                        })
                        .on_click(cx.listener(move |me, _, _, _| me.click(ix)))
                        .on_mouse_up(MouseButton::Middle, cx.listener(move |me, _, _, _| me.middle_click(ix)))
                        .into_any_element()
                }
            };
            cells.push(cell);
        }

        // the glass paints its own fill only while the engine's glass is not running
        let fill = fallback.then(|| {
            div()
                .absolute()
                .left(px(pic.glass[0]))
                .top(px(pic.glass[1]))
                .w(px(pic.glass[2]))
                .h(px(pic.glass[3]))
                .rounded(px(RADIUS))
                .bg(look::fallback_fill(dark).hsla())
        });
        div()
            .size_full()
            .text_size(px(13.))
            .text_color(look::text(dark).hsla())
            .children(fill)
            .child(
                // The row covers the whole window, so the pointer over an icon that has grown out of the glass is still over the dock.
                // (What of the window the system hands to this view at all is the region, see `hit`.)
                div()
                    .id("dock-row")
                    .absolute()
                    .left(px(0.))
                    .top(px(0.))
                    .size_full()
                    .on_mouse_move(cx.listener(move |me, ev: &MouseMoveEvent, _, cx| {
                        let strength = cx.global::<Settings>().dock_magnify.strength();
                        me.pointer_moved(f32::from(ev.position.x) - host::insets(Kind::Middle, cx).margins[0], strength, ev.pressed_button.is_some(), cx)
                    }))
                    .on_mouse_down(MouseButton::Left, cx.listener(|me, _, _, _| me.pressed()))
                    .on_mouse_up(MouseButton::Left, cx.listener(|me, _, _, cx| me.released(cx)))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(|me, _, _, cx| me.released(cx)))
                    .on_hover(cx.listener(|me, hovered: &bool, _, cx| {
                        if !*hovered {
                            me.pointer_left(cx);
                        }
                    }))
                    .children(cells),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::{BASELINE, DOT_DROP, DockItem, ICON_REST, MIN_MOVE, Place, Slot, icon_count, item_icon, look, magnify, model, move_needs_frame, plan};

    #[test]
    fn a_pointer_move_draws_a_frame_only_when_something_could_look_different() {
        use magnify::Strength::{Full, Off, Subtle};
        // magnification off: never, whatever the pointer does
        assert!(!move_needs_frame(Off, true, 100.0, false));
        assert!(!move_needs_frame(Off, false, 100.0, true));
        // the bell starts or is still coming in or going out: every move
        assert!(move_needs_frame(Full, true, 0.0, true));
        assert!(move_needs_frame(Subtle, false, 0.0, false));
        // settled: a move of a quarter of a pixel or more, not less
        assert!(move_needs_frame(Full, false, MIN_MOVE, true));
        assert!(move_needs_frame(Full, false, 3.0, true));
        assert!(!move_needs_frame(Full, false, MIN_MOVE - 0.01, true));
        assert!(!move_needs_frame(Full, false, 0.0, true));
    }

    #[test]
    fn icons_are_counted_without_the_separator() {
        assert_eq!(icon_count(&[Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon]), 3);
        assert_eq!(icon_count(&[]), 0);
    }

    #[test]
    fn the_places_have_icons_and_the_separator_has_none() {
        assert!(item_icon(&DockItem::Separator).is_none());
        assert!(item_icon(&DockItem::Place(Place::RecycleBin)).is_some());
        assert!(item_icon(&DockItem::Place(Place::Downloads)).is_some());
    }

    #[test]
    fn the_dock_before_the_first_refresh_is_the_separator_and_the_two_places() {
        let items = model::merge(&[], Vec::new(), 0);
        let slots = model::slots(&items);
        assert_eq!(slots, vec![Slot::Separator, Slot::Icon, Slot::Icon]);
        // and its width is the one the layout formula gives for two icons and a separator
        assert_eq!(plan::middle_width_at_rest(2, 1), rf_win::dock::layout::middle_width(&[44.0, 44.0], 1));
    }

    #[test]
    fn the_running_marks_stand_below_the_icons_inside_the_island_and_the_icons_grow_into_the_headroom() {
        // the island is 72 high; the biggest icon (44 x 1.55) stands BASELINE up and reaches over the glass's top edge, into the window's
        // headroom (the mark, DOT_DROP below its bottom edge, stays inside the glass)
        let grown_top = BASELINE + ICON_REST * magnify::MAX_PEAK;
        let over = grown_top - rf_win::dock::layout::MIDDLE_HEIGHT;
        assert!(over > 0.0 && over + 1.0 <= rf_win::dock::layout::HEADROOM, "the grown icon reaches {over} px over the glass; the headroom is {}", rf_win::dock::layout::HEADROOM);
        assert!(BASELINE - DOT_DROP - look::running_dot_size(true).1 >= 0.0, "the mark stays inside the island");
    }
}
