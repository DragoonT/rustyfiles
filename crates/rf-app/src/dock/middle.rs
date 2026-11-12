//! The middle island, the dock itself: the apps pinned to the Windows taskbar in the taskbar's order, then the other running apps,
//! a thin separator, the Recycle Bin and the Downloads folder. Icons are 44 px at rest; the one under the pointer grows to 56 px and
//! its two neighbours to 50 px (about 120 ms, eased; instantly when the app's animations are off). The island keeps its width while
//! an icon grows - the gaps give way (`plan::dock_layout`) - and its height never changes.
//!
//! Left click: start the app / bring its last used window forward / minimise it when it is in front. Middle click: another instance.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui_kit::component::h_flex;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::hover::{HoverAnim, Slot};
use rf_win::dock::{DockItem, IconSource, PinnedApp, Place, model};
use rf_win::tasklist::{self, Task, TaskWatch};

use super::backdrop::IslandGlass;
use super::host::Kind;
use super::icons::{self, IconCache};
use super::look::{self, Env};
use super::plan;
use crate::t;

/// How far the icons' bottom edge is above the island's bottom edge (logical px): every icon, grown or not, stands on this line, so
/// the running marks below them do not move.
const BASELINE: f32 = 12.0;
/// The running mark's distance below the icons.
const DOT_DROP: f32 = 7.0;
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
    /// The island window's real width (logical px) as last drawn: smaller than `width()` on a narrow screen.
    view_width: f32,
    /// Where the pointer last was over the island (logical px from its left edge); `None` when it is away.
    pointer_x: Option<f32>,
    anim: HoverAnim,
    /// The number of icons the animation was sized for.
    anim_icons: usize,
    /// Clock of the animation.
    t0: Instant,
    reported: f32,
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
            view_width: 0.0,
            pointer_x: None,
            anim: HoverAnim::new(0, plan::hover_duration(crate::settings::animations())),
            anim_icons: 0,
            t0: Instant::now(),
            reported: 0.0,
            refreshing: false,
            dirty: false,
            _watch: watch,
        };
        // until the first refresh is done the dock shows what it always has: the separator and the two places
        me.items = model::merge(&[], Vec::new(), 0);
        me.slots = model::slots(&me.items);
        me.icon_srcs = icon_sources(&me.items);
        me.anim_icons = icon_count(&me.slots);
        me.anim = HoverAnim::new(me.anim_icons, plan::hover_duration(crate::settings::animations()));
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
        let icons = icon_count(&self.slots);
        if icons != self.anim_icons {
            let now = self.now_ms();
            self.anim.resize(icons, now);
            self.anim_icons = icons;
            // the resize drops the hover; a pointer that has not moved is still over an icon
            if let Some(x) = self.pointer_x {
                self.pointer_moved(x, cx);
            }
        }
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

    fn pointer_moved(&mut self, x: f32, cx: &mut Context<Self>) {
        self.pointer_x = Some(x);
        let width = if self.view_width > 0.0 { self.view_width.min(self.width()) } else { self.width() };
        let target = plan::hover_target(x, &self.slots, width);
        if target != self.anim.hovered() {
            let now = self.now_ms();
            self.anim.set_duration(plan::hover_duration(crate::settings::animations()));
            self.anim.set_hover(target, now);
            cx.notify();
        }
    }

    fn pointer_left(&mut self, cx: &mut Context<Self>) {
        self.pointer_x = None;
        if self.anim.hovered().is_some() {
            let now = self.now_ms();
            self.anim.set_duration(plan::hover_duration(crate::settings::animations()));
            self.anim.set_hover(None, now);
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
        let (pal, fallback) = super::host::frame_palette(&self.glass, window);
        let now = self.now_ms();
        if !self.anim.settled(now) {
            // an icon is still growing or shrinking: one more frame
            window.request_animation_frame();
        }
        let sizes = self.anim.sizes(now);
        // The window may be narrower than the icons want (the side islands and the screen leave it less): lay the icons out for the
        // width it really has, so none falls outside it.
        let real = f32::from(window.viewport_size().width);
        self.view_width = real;
        let width = if real > 0.0 { real.min(self.width()) } else { self.width() };
        let spans = plan::dock_layout(&self.slots, &sizes, width);

        let mut cells: Vec<AnyElement> = Vec::with_capacity(self.items.len());
        for (ix, (item, &(x, w))) in self.items.iter().zip(&spans).enumerate() {
            let cell = match item {
                DockItem::Separator => div()
                    .absolute()
                    .left(px(x))
                    .bottom(px(BASELINE + (rf_win::dock::layout::ICON_REST - 36.0) / 2.0))
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
                        .bottom(px(BASELINE))
                        .w(px(w))
                        .h(px(w))
                        .cursor_pointer()
                        .child(picture)
                        .when(running, |d| {
                            let (dw, dh) = look::running_dot_size(in_front);
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
                        .on_click(cx.listener(move |me, _, _, _| me.click(ix)))
                        .on_mouse_up(MouseButton::Middle, cx.listener(move |me, _, _, _| me.middle_click(ix)))
                        .into_any_element()
                }
            };
            cells.push(cell);
        }

        super::host::island_box(pal, fallback).child(
            h_flex().size_full().child(
                div()
                    .id("dock-row")
                    .relative()
                    .size_full()
                    .on_mouse_move(cx.listener(|me, ev: &MouseMoveEvent, _, cx| me.pointer_moved(f32::from(ev.position.x), cx)))
                    .on_hover(cx.listener(|me, hovered: &bool, _, cx| {
                        if !*hovered {
                            me.pointer_left(cx);
                        }
                    }))
                    .children(cells),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{BASELINE, DOT_DROP, DockItem, Place, Slot, Task, icon_count, item_icons, look, model, plan};

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
    fn the_running_marks_stand_below_the_icons_inside_the_island() {
        // the island is 72 high; the biggest icon (56) stands BASELINE up, its mark DOT_DROP below its bottom edge
        let grown_top = BASELINE + rf_win::dock::hover::GROWN;
        assert!(grown_top <= rf_win::dock::layout::MIDDLE_HEIGHT, "the grown icon fits in the island");
        assert!(BASELINE - DOT_DROP - look::running_dot_size(true).1 >= 0.0, "the mark stays inside the island");
    }
}
