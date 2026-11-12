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
//! Right click: the dock's own app menu (`app_menu.rs`). The pointer resting on a running app's icon: a preview of its windows
//! (`hover_preview.rs`). Both are anchored on the icon's centre AT REST (it does not move while the icons grow). The pins are the
//! Windows taskbar's until the user pins or unpins from the menu, then the dock's own list (`own_pins.rs`).

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
use super::{app_menu, hover_preview};
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
    /// Per item: the key its icon is cached under and where the icon can come from, best first (worked out once per refresh, not per frame).
    icon_srcs: Vec<Option<(String, Vec<IconSource>)>>,
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
    /// The icon the pointer is on (for the hover preview), and where the items lie AT REST (logical px from the island's left edge): a
    /// preview or a menu is anchored on the icon's rest centre, which does not move while the icons grow and shrink under the pointer
    /// (a preview opens 400 ms after the pointer came, when the bell is still easing in, and the pointer then goes into the preview, so
    /// the bell shrinks away).
    hover_ix: Option<usize>,
    rest_items: Vec<(f32, f32)>,
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
                let pins = cx.background_executor().spawn(async { super::own_pins::effective_pins() }).await;
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
            hover_ix: None,
            rest_items: Vec::new(),
            _watch: watch,
        };
        // (the app menu tells it when the dock's own pin list changed)
        super::own_pins::register(cx.entity().downgrade(), cx);
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
                // the windows of a packaged app are given their process's package id first, so they meet the app's pin
                let groups = cx.background_executor().spawn(async { rf_win::dock::identity::running_groups() }).await;
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
        let items = model::merge(&self.pins, groups, foreground);
        if items == self.items {
            // Most task-list events change nothing the island shows (a window that only moved, was shown again, took the focus within
            // the same app: 26 of 28 refreshes in a minute of ordinary use). Every `notify` asks for a frame, and a frame keeps the
            // vsync thread following the display for a second, so repainting for nothing means dozens of wake-ups a second in a
            // process that should be idle. `DockItem` holds everything the island draws (windows, which one is in front), so equal
            // items are an equal picture. Only a new display scale still needs the icons asked for again.
            if self.scale_changed(cx) {
                self.load_icons(cx);
                cx.notify();
            }
            return;
        }
        self.items = items;
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

    /// The display scale is not the one the cached icons were asked for.
    fn scale_changed(&self, cx: &Context<Self>) -> bool {
        (cx.global::<Env>().scale - self.icon_scale).abs() > 0.001
    }

    /// Asks for every icon not yet asked for (and every one that failed a while ago), on worker threads.
    fn load_icons(&mut self, cx: &mut Context<Self>) {
        let scale = cx.global::<Env>().scale;
        if self.scale_changed(cx) {
            // the bitmaps were made for another scale: ask again for all of them
            self.icon_scale = scale;
            self.icons.clear();
        }
        let px = plan::icon_request_px(scale);
        let wanted: Vec<(String, Vec<IconSource>)> = self.icon_srcs.iter().flatten().cloned().collect();
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

    /// Reads the pins again now (the dock's own list was just changed from the app menu), off the UI thread.
    pub fn reload_pins(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let pins = cx.background_executor().spawn(async { super::own_pins::effective_pins() }).await;
            let _ = this.update(cx, |me, cx| {
                if !me.pins_loaded || me.pins != pins {
                    me.pins = pins;
                    me.pins_loaded = true;
                    me.refresh(cx);
                }
            });
        })
        .detach();
    }

    /// The icon of item `ix`, if it has been loaded.
    fn icon_for(&self, ix: usize) -> Option<std::sync::Arc<RenderImage>> {
        self.icon_srcs.get(ix).and_then(|k| k.as_ref()).and_then(|(key, _)| self.icons.get(key))
    }

    /// The centre of item `ix` at rest, device px in screen coordinates: the rest glass's left edge plus the rest layout's distance from it.
    fn icon_anchor_x(&self, ix: usize, cx: &App) -> Option<f32> {
        let &(x, w) = self.rest_items.get(ix)?;
        let glass = host::visible_glass_rect(Kind::Middle, cx)?;
        Some(screen_center_x(glass.left, x, w, cx.global::<Env>().scale))
    }

    /// The pointer came onto item `ix` (`on`) or left it: a running app's icon is what a preview belongs to.
    fn item_hover(&mut self, ix: usize, on: bool, cx: &mut Context<Self>) {
        if on {
            self.hover_ix = Some(ix);
            let info = match (self.items.get(ix), self.icon_anchor_x(ix, cx)) {
                (Some(DockItem::App(a)), Some(anchor_x)) if a.running() => {
                    Some(hover_preview::IconInfo { app: a.clone(), icon: self.icon_for(ix), anchor_x, glass: self.glass.clone() })
                }
                _ => None,
            };
            hover_preview::icon_hover(info, cx);
        } else if self.hover_ix == Some(ix) {
            // (a leave that arrives after the next icon's enter is not news)
            self.hover_ix = None;
            hover_preview::icon_hover(None, cx);
        }
    }

    /// A right click on item `ix`: the app menu, for an app.
    fn right_click(&mut self, ix: usize, cx: &mut Context<Self>) {
        if !cx.global::<Settings>().dock_app_menu {
            return;
        }
        let (Some(DockItem::App(app)), Some(anchor_x)) = (self.items.get(ix), self.icon_anchor_x(ix, cx)) else { return };
        app_menu::open(app_menu::MenuRequest { app: app.clone(), icon: self.icon_for(ix), anchor_x, glass: self.glass.clone() }, cx);
    }

    fn click(&mut self, ix: usize, cx: &mut Context<Self>) {
        // (a click answers the question a preview asks: it goes)
        hover_preview::clicked(cx);
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

/// The screen x (device px) of the centre of an item that lies at `x`, `w` wide (logical px from the rest island's left edge), given the
/// rest glass's left edge on the screen (`glass_left`, device px).
fn screen_center_x(glass_left: i32, x: f32, w: f32, scale: f32) -> f32 {
    glass_left as f32 + (x + w / 2.0) * scale
}

/// Whether a pointer move over the dock is worth a frame: never with the magnification off; always when the bell starts or is still
/// moving; otherwise only when the pointer moved by `MIN_MOVE` (logical px) or more.
fn move_needs_frame(strength: magnify::Strength, entering: bool, delta: f32, settled: bool) -> bool {
    strength != magnify::Strength::Off && (entering || !settled || delta >= MIN_MOVE)
}

fn icon_count(slots: &[Slot]) -> usize {
    slots.iter().filter(|s| **s == Slot::Icon).count()
}

/// Where an item's icon can come from, best first: an app's own, then what its windows have set themselves, then its package's logo;
/// a place's own. Only when none gives a picture does the island draw the app's letter.
fn item_icons(item: &DockItem) -> Option<Vec<IconSource>> {
    match item {
        DockItem::App(a) => Some(a.icon_chain()),
        DockItem::Place(p) => Some(vec![p.icon_source()]),
        DockItem::Separator => None,
    }
}

/// The icon key and sources of every item, in the items' order. Worked out when the items change: the Downloads folder's place is a
/// call to the shell, and a key is a string, neither of which belongs in a frame. The key is that of the first source, the app's own.
fn icon_sources(items: &[DockItem]) -> Vec<Option<(String, Vec<IconSource>)>> {
    items.iter().map(|i| item_icons(i).and_then(|srcs| srcs.first().map(|first| (plan::icon_key(first), srcs.clone())))).collect()
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
        // the palette follows the tone (what is behind the glass), the fill below it is the island's own while the glass is not running
        let (pal, fallback) = super::host::frame_palette(&self.glass, window);
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
        self.rest_items.clone_from(&rest);

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
                    .bg(pal.separator().hsla())
                    .into_any_element(),
                DockItem::App(_) | DockItem::Place(_) => {
                    let picture: AnyElement = match self.icon_srcs.get(ix).and_then(|k| k.as_ref()).and_then(|(key, _)| self.icons.get(key)) {
                        Some(img) => gpui_kit::img(img).size_full().into_any_element(),
                        None => div()
                            .size_full()
                            .rounded(px(w * 0.24))
                            .bg(pal.placeholder_fill().hsla())
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
                                    .bg(pal.running_dot(in_front).hsla()),
                            )
                        })
                        .on_click(cx.listener(move |me, _, _, cx| me.click(ix, cx)))
                        .on_hover(cx.listener(move |me, on: &bool, _, cx| me.item_hover(ix, *on, cx)))
                        .on_mouse_up(MouseButton::Right, cx.listener(move |me, _, _, cx| me.right_click(ix, cx)))
                        .on_mouse_up(MouseButton::Middle, cx.listener(move |me, _, _, _| me.middle_click(ix)))
                        .into_any_element()
                }
            };
            cells.push(cell);
        }

        // The glass paints its own fill only while the engine's glass is not running. The window is bigger than the glass, so the fill is
        // the glass's rectangle in it (`pic.glass`), not the whole view; its colours are the tone's, as on main.
        let fill = fallback.then(|| {
            div()
                .absolute()
                .left(px(pic.glass[0]))
                .top(px(pic.glass[1]))
                .w(px(pic.glass[2]))
                .h(px(pic.glass[3]))
                .rounded(px(RADIUS))
                .bg(pal.fallback_fill().hsla())
        });
        div()
            .size_full()
            .text_size(px(13.))
            .text_color(pal.text().hsla())
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
    use super::{BASELINE, DOT_DROP, DockItem, ICON_REST, MIN_MOVE, Place, Slot, Task, icon_count, item_icons, look, magnify, model, move_needs_frame, plan};

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
    fn an_icons_anchor_is_its_rest_centre_on_the_screen_at_any_scale() {
        // 100 %: the rest glass starts at screen x 700; the first icon lies 12 px (the padding) into it and is 44 wide
        assert_eq!(super::screen_center_x(700, 12.0, 44.0, 1.0), 734.0, "12 + 22 px into the glass");
        // 150 %: the same logical place is 1.5 times as far from the glass's edge in device px
        assert_eq!(super::screen_center_x(1050, 12.0, 44.0, 1.5), 1101.0);
        // the n-th icon of a row at rest lies 54 px (44 + the 10 px gap) after the one before
        assert_eq!(super::screen_center_x(700, 12.0 + 3.0 * 54.0, 44.0, 1.0), 734.0 + 162.0);
        // and the rest layout is what the island hands to it
        let rest = plan::rest_layout(&[Slot::Icon, Slot::Icon, Slot::Separator, Slot::Icon], plan::middle_width_at_rest(3, 1));
        assert_eq!(rest[0], (12.0, 44.0));
        assert_eq!(rest[1].0, 66.0);
    }

    #[test]
    fn icons_are_counted_without_the_separator() {
        assert_eq!(icon_count(&[Slot::Icon, Slot::Separator, Slot::Icon, Slot::Icon]), 3);
        assert_eq!(icon_count(&[]), 0);
    }

    #[test]
    fn the_places_have_icons_and_the_separator_has_none() {
        assert!(item_icons(&DockItem::Separator).is_none());
        assert!(item_icons(&DockItem::Place(Place::RecycleBin)).is_some());
        assert!(item_icons(&DockItem::Place(Place::Downloads)).is_some());
    }

    #[test]
    fn the_dock_before_the_first_refresh_is_the_separator_and_the_two_places() {
        let items = model::merge(&[], Vec::new(), 0);
        let slots = model::slots(&items);
        assert_eq!(slots, vec![Slot::Separator, Slot::Icon, Slot::Icon]);
        // and its width is the one the layout formula gives for two icons and a separator
        assert_eq!(plan::middle_width_at_rest(2, 1), rf_win::dock::layout::middle_width(&[44.0, 44.0], 1));
    }

    fn task(hwnd: isize, title: &str, app: &str) -> Task {
        Task { hwnd, title: title.into(), pid: hwnd as u32, exe: Some(std::path::PathBuf::from(app)), app_id: None, minimized: false }
    }

    /// `MiddleIsland::apply` repaints only when the items differ. What the island does not draw (a window's title, whether it is
    /// minimised) must not make them differ; a window opening or closing, or another app coming to the front, must.
    #[test]
    fn a_refresh_that_changes_nothing_on_the_island_gives_equal_items() {
        let groups = |t1: &str, t2: &str, min: bool| {
            let mut b = task(11, t2, "c:/apps/b.exe");
            b.minimized = min;
            vec![("c:/apps/a.exe".to_string(), vec![task(10, t1, "c:/apps/a.exe")]), ("c:/apps/b.exe".to_string(), vec![b])]
        };
        let base = model::merge(&[], groups("one", "two", false), 10);
        // the same windows with other titles, one minimised, the same one in front: the same picture
        assert_eq!(base, model::merge(&[], groups("a loading page", "another", true), 10));
        assert_eq!(base, model::merge(&[], groups("one", "two", false), 10));
        // another app in front: the running mark of both changes
        assert_ne!(base, model::merge(&[], groups("one", "two", false), 11));
        // a window more or less
        let mut more = groups("one", "two", false);
        more[0].1.push(task(12, "x", "c:/apps/a.exe"));
        assert_ne!(base, model::merge(&[], more, 10));
        assert_ne!(base, model::merge(&[], groups("one", "two", false)[..1].to_vec(), 10));
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
