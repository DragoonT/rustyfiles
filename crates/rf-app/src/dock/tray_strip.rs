//! The tray strip of the right island (shell plan increment D3): the other programs' notification icons as a row of small pictures to the
//! left of the island's own symbols, with a chevron that opens the overflow panel (`tray_overflow.rs`, a panel of `panel.rs`) for the
//! rest. This file is the state and the glue; the arithmetic and the decisions are `rf_win::dock::{tray_strip, tray_order, tray_tip}`
//! (pure, tested), the icons and the clicks go through `tray_source.rs` (the one seam to the tray host).
//!
//! WHEN THERE IS NO SOURCE NOTHING IS MADE: `start` does nothing without `tray_source::available()`, and with a source that has no
//! snapshot (the tray host off) the strip draws nothing and the island is exactly what it was. The state exists from the dock's start
//! (the host may be turned on later), but while there is no snapshot it costs one subscription and one idle task: the order file is
//! NOT read until the first snapshot arrives (then on a worker thread), and a refresh that finds no snapshot, where there was none,
//! changes nothing and wakes nobody.
//!
//! THE STATE is one global (`TrayState`): the snapshot, the user's order (`tray-order.json`, read once, off the interface thread, when the
//! first snapshot comes; written only when the user changes something), the layout that follows from both, the pictures converted for the toolkit, and what the pointer and the keyboard are
//! doing. Changing it through `update_global` tells whoever observes it (the island, the overflow panel); the bookkeeping nobody draws
//! (a held button) is written without telling.
//!
//! THE POINTER. A press on an icon is not yet a click: it might become a drag. So a press records what and where and starts one small
//! task that looks at the real pointer and buttons every 16 ms until the button is up (the toolkit's own events do not follow a drag
//! across windows, from the overflow panel to the island, or out of the window at all). Released without having moved 4 px: the click
//! (left, right, middle, or the second press of a double click) goes to the program through the tray host. Released after a drag: the
//! icon is dropped on a gap of the strip (reorder), on the chevron or the open overflow panel (hide), or nowhere (nothing happens).
//! While it is held nothing else runs, and when nothing is pressed nothing runs at all.
//!
//! THE KEYBOARD. The islands are never activated, so keys reach the strip only after `focus_strip`: it takes the foreground for the
//! island's window (`rf_win::dock::tray_strip::take_keyboard`), which then gets the keys. The arrows, Home and End move a ring over the
//! icons and the chevron, Enter or Space is a left click, the context-menu key or Shift+F10 a right click, Tab and Escape leave. The
//! keyboard goes back to the window that had it (after Escape or Tab at once; after a click once the program has had 400 ms to take the
//! foreground itself, and only if nobody has; the overflow panel opened from here is told which window that was, so its Escape gives the
//! keyboard back to the user's program and not to the island). The ring is on an icon by its KEY, not by its place: an icon that comes or
//! goes under it moves the ring along, and a click goes to the icon the ring is on. NOTHING CALLS `focus_strip` ON ITS OWN: the lead wires a key to it at the merge (the fake
//! source can call it once after a delay, `RUSTYFILES_TRAY_FAKE_FOCUS_MS`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::Icon;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::layout::{Rect, px as device_px};
use rf_win::dock::panel::{self as geo, CloseCause};
use rf_win::dock::tray_order::{self, Arrangement, Dest, Loaded, StableId, TrayOrder};
use rf_win::dock::tray_strip::{self as ts, ClickKind, DropTarget, Geometry, Layout, PointerButton, Press, Step, StripOutcome};
use rf_win::dock::tray_tip as rules;

use super::host::{self, Kind};
use super::look::{Env, Palette};
use super::right::RightIsland;
use super::tray_source::{self, IconKey, TrayClick, TrayIcon, TraySnapshot};

/// How often a held button on an icon looks at the pointer.
const POLL_EVERY: Duration = Duration::from_millis(16);
/// How often the island with the keyboard in it looks at the foreground.
const KEY_WATCH_EVERY: Duration = Duration::from_millis(100);
/// How long after a click from the keyboard the program has to take the foreground; if the island still has it then, the window that
/// had the keyboard before gets it back.
const CLICK_SETTLE: Duration = Duration::from_millis(400);
/// The key the chevron's tooltip is told apart by.
const CHEVRON_KEY: IconKey = u64::MAX;

/// A place on the screen an icon (or the chevron) is drawn: the pointer events and the tips name it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Cell `j` of the strip.
    Strip(usize),
    Chevron,
    /// Cell `k` of the overflow panel.
    Overflow(usize),
}

#[derive(Clone, Debug)]
struct PressState {
    press: Press,
    source: Source,
    /// The icon pressed (`None`: the chevron).
    key: Option<IconKey>,
    /// Its identity at the press: the click is refused if the key belongs to another icon by the time the button is up.
    id: Option<StableId>,
}

#[derive(Clone, Copy, Debug)]
struct Drag {
    key: IconKey,
    target: DropTarget,
}

#[derive(Clone, Copy, Debug)]
struct Keyboard {
    cursor: usize,
    /// The icon the ring is on (`None`: the chevron). The ring follows this key when the icons move; `cursor` is where that is now.
    on: Option<IconKey>,
    /// The window that had the keyboard before (0: none, or the island itself).
    previous: isize,
    opened_at: Instant,
    been_front: bool,
}

/// Whether the order file has been read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OrderLoad {
    /// Not asked for yet (no snapshot has come).
    NotYet,
    /// A worker thread is reading it; snapshots wait for it (the strip draws nothing meanwhile).
    Reading,
    Done,
}

pub struct TrayState {
    right: WeakEntity<RightIsland>,
    snap: Option<TraySnapshot>,
    ids: Vec<StableId>,
    order: TrayOrder,
    order_path: Option<PathBuf>,
    /// Where the order file stands: it is read when the first snapshot arrives, not at the dock's start.
    order_load: OrderLoad,
    /// The order file exists and could not be read: it is set aside before the first write.
    unreadable: bool,
    arr: Arrangement,
    lay: Layout,
    /// How wide the strip block may be, logical px (a quarter of the monitor's work area, at most 240).
    limit: f32,
    /// How much of the monitor the hidden-icons panel may take (monitor-scale logical px): more icons than rows make it wider, never
    /// taller than this.
    ovf: ts::OvfLimits,
    /// The pictures converted for the toolkit, with the icon generation they were made from.
    images: HashMap<IconKey, (u64, Arc<RenderImage>)>,
    hover: Option<Source>,
    press: Option<PressState>,
    drag: Option<Drag>,
    kb: Option<Keyboard>,
    poll: Option<Task<()>>,
    kb_watch: Option<Task<()>>,
    _sub: Option<tray_source::Subscription>,
}

impl Global for TrayState {}

impl TrayState {
    fn icons(&self) -> &[TrayIcon] {
        self.snap.as_ref().map_or(&[], |s| &s.icons)
    }

    fn index_of(&self, key: IconKey) -> Option<usize> {
        self.icons().iter().position(|i| i.key == key)
    }

    fn icon_of(&self, src: Source) -> Option<usize> {
        match src {
            Source::Strip(j) => self.lay.visible.get(j).copied(),
            Source::Overflow(k) => self.lay.overflow.get(k).copied(),
            Source::Chevron => None,
        }
    }

    fn rearrange(&mut self) {
        self.arr = self.order.arrange(&self.ids);
        self.lay = ts::layout(&self.arr.all, &self.arr.hidden, self.limit);
    }

    /// The key of the icon at stop `stop` of the keyboard walk (`None`: the chevron).
    fn key_at_stop(&self, stop: usize) -> Option<IconKey> {
        self.lay.visible.get(stop).and_then(|&i| self.icons().get(i)).map(|i| i.key)
    }

    /// The keyboard's ring stays on its icon when the icons move; when its icon is gone it stays at the nearest stop.
    fn follow_ring(&mut self) {
        let Some(mut k) = self.kb else { return };
        let n = self.lay.visible.len();
        let stops = ts::stops(n, self.lay.chevron);
        let following = k.on.and_then(|key| self.lay.visible.iter().position(|&i| self.icons().get(i).is_some_and(|ic| ic.key == key)));
        k.cursor = match (k.on, following) {
            (_, Some(j)) => j,
            (None, None) => n.min(stops.saturating_sub(1)),
            (Some(_), None) => k.cursor.min(stops.saturating_sub(1)),
        };
        k.on = self.key_at_stop(k.cursor);
        self.kb = Some(k);
    }

    /// A new snapshot (`None`: the host is off) for a monitor whose work area is `work_width` logical px wide. Returns the pictures that
    /// are not used any more: GPUI keeps an uploaded picture in each window's sprite atlas until it is told to drop it, so the caller must
    /// hand them to `drop_image` (see `retire`).
    #[must_use]
    fn set_snapshot(&mut self, snap: Option<TraySnapshot>, work_width: Option<f32>) -> Vec<Arc<RenderImage>> {
        self.limit = ts::strip_limit(work_width.unwrap_or(1920.0));
        self.snap = snap;
        self.ids = self.icons().iter().map(|i| i.id.clone()).collect();
        self.rearrange();
        // the pictures: a new one is made only for an icon whose generation moved
        let old = std::mem::take(&mut self.images);
        let mut images = HashMap::new();
        for icon in self.icons() {
            let Some(pic) = &icon.picture else { continue };
            let kept = old.get(&icon.key).filter(|(g, _)| *g == icon.generation).cloned();
            if let Some(e) = kept.or_else(|| to_image(pic).map(|img| (icon.generation, img))) {
                images.insert(icon.key, e);
            }
        }
        // (a picture that was replaced, or whose icon went, is retired; one that was kept is the same Arc and stays)
        let retired: Vec<Arc<RenderImage>> = old.into_values().map(|(_, a)| a).filter(|a| !images.values().any(|(_, b)| Arc::ptr_eq(a, b))).collect();
        self.images = images;
        // what was held or pointed at may be gone
        if let Some(d) = self.drag
            && self.index_of(d.key).is_none()
        {
            self.drag = None;
        }
        self.follow_ring();
        if self.snap.is_none() {
            self.kb = None;
            self.hover = None;
            // a host that comes back must be told where the icons are again, even when they come back where they were
            SENT.with(|s| s.borrow_mut().forget());
        }
        retired
    }

    /// Moves present icon `icon` (see `tray_order::TrayOrder::reorder`). True when the user sees a change.
    fn move_icon(&mut self, icon: usize, dest: Dest, at: usize) -> bool {
        let changed = self.order.reorder(&self.ids, icon, dest, at);
        if changed {
            self.rearrange();
            self.follow_ring();
        }
        changed
    }
}

/// The picture the toolkit draws: its images hold blue first, the host's pixels have red first.
fn to_image(pic: &tray_source::Bitmap) -> Option<Arc<RenderImage>> {
    if !pic.is_whole() {
        return None;
    }
    let buf = image::RgbaImage::from_raw(pic.width, pic.height, ts::rgba_to_bgra(&pic.rgba))?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// Gives retired pictures back to the toolkit, after the current update (a window being updated is out of the app's window list for a
/// moment, so an immediate call would miss it), as `image_cache.rs` does.
fn retire(images: Vec<Arc<RenderImage>>, cx: &mut App) {
    if images.is_empty() {
        return;
    }
    cx.defer(move |cx| {
        for img in images {
            cx.drop_image(img, None);
        }
    });
}

// ---------------------------------------------------------------- starting

/// Called once, when the dock's islands exist. Makes the state, reads the order file, follows the source.
pub fn start(right: Entity<RightIsland>, cx: &mut App) {
    if !tray_source::available() || cx.has_global::<TrayState>() {
        return;
    }
    let path = crate::settings::app_dir().map(|d| d.join(tray_order::FILE));
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    let tx = Mutex::new(tx);
    let sub = tray_source::subscribe(move || {
        if let Ok(tx) = tx.lock() {
            let _ = tx.unbounded_send(());
        }
    });
    cx.set_global(TrayState {
        right: right.downgrade(),
        snap: None,
        ids: Vec::new(),
        order: TrayOrder::new(),
        order_path: path,
        order_load: OrderLoad::NotYet,
        unreadable: false,
        arr: Arrangement::default(),
        lay: Layout::default(),
        limit: ts::strip_limit(1920.0),
        ovf: ts::OvfLimits::for_work(1920.0, 1040.0),
        images: HashMap::new(),
        hover: None,
        press: None,
        drag: None,
        kb: None,
        poll: None,
        kb_watch: None,
        _sub: sub,
    });
    // the island draws again whenever the state changes
    let island = right.clone();
    cx.observe_global::<TrayState>(move |cx| {
        island.update(cx, |_, cx| cx.notify());
    })
    .detach();
    // the host says something changed: read the snapshot again (a burst is one reading)
    cx.spawn(async move |cx| {
        while rx.next().await.is_some() {
            while rx.try_recv().is_ok() {}
            cx.update(refresh);
        }
    })
    .detach();
    refresh(cx);
    if let Some(ms) = tray_source::fake_focus_after_ms() {
        cx.spawn(async move |cx| {
            cx.background_executor().timer(Duration::from_millis(ms)).await;
            cx.update(|cx| {
                focus_strip(cx);
            });
        })
        .detach();
    }
}

/// The dock ends.
pub fn shutdown(cx: &mut App) {
    if cx.has_global::<TrayState>() {
        let st = cx.remove_global::<TrayState>();
        let pictures: Vec<Arc<RenderImage>> = st.images.into_values().map(|(_, a)| a).collect();
        retire(pictures, cx);
    }
}

/// The monitor, its scale or its work area changed: the strip's room is worked out again.
pub fn layout_changed(cx: &mut App) {
    refresh(cx);
}

fn refresh(cx: &mut App) {
    if !cx.has_global::<TrayState>() {
        return;
    }
    let snap = tray_source::snapshot();
    let st = cx.global::<TrayState>();
    match refresh_plan(snap.is_some(), st.snap.is_some(), st.order_load) {
        // no host and none before: nothing to draw, so nothing to recompute and nobody to wake (a monitor change lands here often)
        RefreshPlan::Nothing => return,
        // the user's order is needed to place the first icons: read it off this thread; it comes back here when it is in
        RefreshPlan::ReadOrder => return read_order(cx),
        RefreshPlan::WaitForOrder => return,
        RefreshPlan::Apply => {}
    }
    // (the strip's room is a quarter of the screen in dock units: the cells are dock units wide)
    let size = super::look::dock_size(cx);
    let work_width = host::monitor_work(cx).map(|(scale, work)| work.width() as f32 / (scale * size).max(0.01));
    // (the hidden-icons panel is drawn at the monitor's scale, not the dock's size: its limits are the work area in those px)
    let panel_limits = host::monitor_work(cx).map(|(scale, work)| ts::OvfLimits::for_work(work.width() as f32 / scale.max(0.01), work.height() as f32 / scale.max(0.01)));
    let retired = cx.update_global::<TrayState, _>(|st, _| {
        if let Some(l) = panel_limits {
            st.ovf = l;
        }
        st.set_snapshot(snap, work_width)
    });
    retire(retired, cx);
    after_change(cx);
}

/// The state changed: the island gets its new width, the host its new rectangles, the open panel its new size.
fn after_change(cx: &mut App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    let (right, extra, gone) = (st.right.clone(), st.lay.extra_width(), st.snap.is_none());
    if let Some(r) = right.upgrade() {
        r.update(cx, |v, cx| v.set_tray_extra(extra, cx));
    }
    if overflow_rect(cx).is_some() {
        if gone {
            // the host went away under an open panel
            super::panel::close(CloseCause::LayoutChanged, cx);
        } else {
            super::panel::relayout_sized(super::panel::OVERFLOW, overflow_size(cx), cx);
        }
    }
    report_rects(cx);
    // an open tip follows what the icon under the pointer says now (its tip or its place changed)
    let hovered = cx.try_global::<TrayState>().and_then(|s| s.hover);
    if let Some(src) = hovered {
        let anchor = tip_anchor(src, cx);
        super::tray_tip::update(anchor, cx);
    }
}

/// What a refresh does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RefreshPlan {
    Nothing,
    ReadOrder,
    WaitForOrder,
    Apply,
}

/// `now`: the source has a snapshot; `before`: the state holds one; `order`: where the order file stands.
fn refresh_plan(now: bool, before: bool, order: OrderLoad) -> RefreshPlan {
    match (now, before, order) {
        (false, false, _) => RefreshPlan::Nothing,
        (true, _, OrderLoad::NotYet) => RefreshPlan::ReadOrder,
        (true, _, OrderLoad::Reading) => RefreshPlan::WaitForOrder,
        _ => RefreshPlan::Apply,
    }
}

/// Reads the order file on a worker thread, once, and refreshes when it is in. A file that is not there is an empty order; one that
/// cannot be read is an empty order too and is set aside before anything is written.
fn read_order(cx: &mut App) {
    let path = {
        let st = cx.global_mut::<TrayState>();
        st.order_load = OrderLoad::Reading;
        st.order_path.clone()
    };
    cx.spawn(async move |cx| {
        let loaded = match path {
            Some(path) => Some(cx.background_executor().spawn(async move { tray_order::load_checked(&path) }).await),
            None => None,
        };
        cx.update(|cx| {
            if !cx.has_global::<TrayState>() {
                return;
            }
            let st = cx.global_mut::<TrayState>();
            match loaded {
                Some(Loaded::Found(o)) => st.order = o,
                Some(Loaded::Unreadable) => {
                    log::warn!("tray: the order file cannot be read; it is set aside before anything is written");
                    st.unreadable = true;
                }
                _ => {}
            }
            st.order_load = OrderLoad::Done;
            refresh(cx);
        });
    })
    .detach();
}

// ---------------------------------------------------------------- where things are

/// The right island's visible glass and the device px per dock unit (the monitor's scale times the dock's size): what the strip's cells
/// and chevron are placed with. (The overflow panel's own cells keep the monitor's scale: see `source_rect`.)
fn glass_and_unit(cx: &App) -> Option<(Rect, f32)> {
    Some((host::visible_glass_rect(Kind::Right, cx)?, rf_win::dock::size::unit(host::monitor_work(cx)?.0, super::look::dock_size(cx))))
}

/// What the strip shows now, for the right island's empty-glass test (`empty.rs`): `None` without a strip.
pub fn shape(cx: &App) -> Option<super::empty::TrayShape> {
    let st = cx.try_global::<TrayState>()?;
    (st.snap.is_some() && st.lay.chevron).then_some(super::empty::TrayShape { visible: st.lay.visible.len(), chevron: st.lay.chevron })
}

/// The open overflow panel's rectangle (device px), `None` while it is closed.
pub fn overflow_rect(cx: &App) -> Option<Rect> {
    super::panel::overflow_rect(cx)
}

/// Where a source is on the screen now, device px.
fn source_rect(src: Source, cx: &App) -> Option<Rect> {
    let st = cx.try_global::<TrayState>()?;
    let (glass, unit) = glass_and_unit(cx)?;
    match src {
        Source::Strip(j) => (j < st.lay.visible.len()).then(|| ts::cell_rect(glass, unit, j)),
        Source::Chevron => st.lay.chevron.then(|| ts::chevron_rect(glass, unit, st.lay.visible.len())),
        Source::Overflow(k) => {
            // the overflow panel is a window of its own: its cells are in the monitor's scale, not the dock's size
            let scale = host::monitor_work(cx)?.0;
            let panel = overflow_rect(cx)?;
            let n = st.lay.overflow.len();
            (k < ts::ovf_grid(n, st.ovf).shown).then(|| {
                let [x, y, w, h] = ts::ovf_cell_in(n, k, st.ovf);
                Rect::from_xywh(panel.left + device_px(x, scale), panel.top + device_px(y, scale), device_px(w, scale), device_px(h, scale))
            })
        }
    }
}

/// What the tray host was last told about the icons' places, so that it is told again only when something moved.
#[derive(Default)]
struct RectMemo(Vec<(IconKey, Rect)>);

impl RectMemo {
    /// True when `rects` is not what the host was last told (and remembers it as told).
    fn changed(&mut self, rects: &[(IconKey, Rect)]) -> bool {
        if self.0 == rects {
            return false;
        }
        self.0 = rects.to_vec();
        true
    }

    /// The host went away: whatever comes next is new to it.
    fn forget(&mut self) {
        self.0.clear();
    }
}

thread_local! {
    static SENT: RefCell<RectMemo> = RefCell::new(RectMemo::default());
}

/// Tells the tray host where the icons are: the cells of the strip, and for an icon in the overflow its cell while the panel is open and
/// the chevron otherwise. Cheap and safe to call from a render (reads only).
pub fn report_rects(cx: &App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    let Some(snap) = &st.snap else { return };
    let Some((glass, unit)) = glass_and_unit(cx) else { return };
    let mut rects: Vec<(IconKey, Rect)> = Vec::with_capacity(snap.icons.len());
    for (j, &i) in st.lay.visible.iter().enumerate() {
        rects.push((snap.icons[i].key, ts::cell_rect(glass, unit, j)));
    }
    let chevron = ts::chevron_rect(glass, unit, st.lay.visible.len());
    for (k, &i) in st.lay.overflow.iter().enumerate() {
        let r = if overflow_rect(cx).is_some() { source_rect(Source::Overflow(k), cx).unwrap_or(chevron) } else { chevron };
        rects.push((snap.icons[i].key, r));
    }
    if SENT.with(|s| s.borrow_mut().changed(&rects)) {
        tray_source::report_icon_rects(&rects);
    }
}

// ---------------------------------------------------------------- drawing

fn accent(cx: &App) -> Hsla {
    gpui_kit::rgb(cx.try_global::<Env>().map_or(0x0078D4, |e| e.accent)).into()
}

fn with_alpha(mut c: Hsla, a: f32) -> Hsla {
    c.a = a;
    c
}

/// An icon's picture at `size` logical px: its bitmap, or its first letter on a plain square until (or if) it has one.
fn picture_of(st: &TrayState, i: usize, size: f32, pal: Palette) -> AnyElement {
    let Some(icon) = st.icons().get(i) else { return div().size(px(size)).into_any_element() };
    if let Some((_, img)) = st.images.get(&icon.key) {
        return gpui_kit::img(img.clone()).size(px(size)).into_any_element();
    }
    div()
        .size(px(size))
        .flex_shrink_0()
        .rounded(px(size * 0.24))
        .bg(pal.placeholder_fill().hsla())
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(size * 0.6))
        .child(super::plan::initial(&rules::clean(&icon.tip)))
        .into_any_element()
}

/// The pointer events of a cell: hovering (for the tip and the highlight) and the three buttons.
pub fn wire(cell: Stateful<Div>, src: Source) -> Stateful<Div> {
    cell.on_hover(move |on: &bool, _, cx| hover(src, *on, cx))
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, _, cx| press(src, PointerButton::Left, e.click_count, cx))
        .on_mouse_down(MouseButton::Right, move |e: &MouseDownEvent, _, cx| press(src, PointerButton::Right, e.click_count, cx))
        .on_mouse_down(MouseButton::Middle, move |e: &MouseDownEvent, _, cx| press(src, PointerButton::Middle, e.click_count, cx))
}

/// The strip for the right island: the cells, the chevron and the thin separator after them. `None` when there is nothing to show (no
/// source, the host off, no icon): the island is then what it was without a strip.
pub fn strip_element(pal: Palette, cx: &App) -> Option<AnyElement> {
    let st = cx.try_global::<TrayState>()?;
    if st.snap.is_none() || !st.lay.chevron {
        return None;
    }
    report_rects(cx);
    // every length below is a dock unit (the default size's logical px) times the dock's size factor
    let k = super::look::dock_size(cx);
    let ring = accent(cx);
    let (hover_bg, drop_bg) = (pal.hover_fill().hsla(), with_alpha(ring, 0.35));
    // the cell a button is held on (until it turns into a drag, which has its own look): the press plate
    let press_bg = pal.press_plate().hsla();
    let pressed = st.press.as_ref().filter(|p| !p.press.dragging).map(|p| p.source);
    let n = st.lay.visible.len();
    let dragging = st.drag.as_ref().map(|d| d.key);
    let target = st.drag.as_ref().map(|d| d.target);
    let mut row = div().id("tray-strip").relative().flex_shrink_0().w(px(st.lay.block_width() * k)).h(px(ts::CELL * k));
    for (j, &i) in st.lay.visible.iter().enumerate() {
        let key = st.icons()[i].key;
        let hot = dragging.is_none() && st.hover == Some(Source::Strip(j));
        let on_cursor = st.kb.as_ref().is_some_and(|k| k.cursor == j);
        let cell = div()
            .id(("tray-cell", j))
            .absolute()
            .left(px(j as f32 * ts::CELL * k))
            .top(px(0.))
            .size(px(ts::CELL * k))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(6. * k))
            .border_2()
            .border_color(if on_cursor { ring } else { transparent_black() })
            .cursor_pointer()
            .when(hot, |d| d.bg(hover_bg))
            .when(pressed == Some(Source::Strip(j)), |d| d.bg(press_bg))
            .when(dragging == Some(key), |d| d.opacity(0.35))
            .child(picture_of(st, i, ts::ICON * k, pal));
        row = row.child(wire(cell, Source::Strip(j)));
    }
    // where a dragged icon would land
    if let Some(DropTarget::Strip(drop_at)) = target {
        row = row.child(div().absolute().left(px(drop_at as f32 * ts::CELL * k - 1.0)).top(px(2. * k)).w(px(2.)).h(px((ts::CELL - 4.) * k)).rounded_full().bg(ring));
    }
    let chevron_hot = (dragging.is_none() && st.hover == Some(Source::Chevron)) || target == Some(DropTarget::Chevron);
    let chevron_cursor = st.kb.as_ref().is_some_and(|k| k.cursor == n);
    let chevron = div()
        .id("tray-chevron")
        .absolute()
        .left(px(n as f32 * ts::CELL * k))
        .top(px(0.))
        .w(px(ts::CHEVRON_W * k))
        .h(px(ts::CELL * k))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6. * k))
        .border_2()
        .border_color(if chevron_cursor { ring } else { transparent_black() })
        .cursor_pointer()
        .when(chevron_hot, |d| d.bg(if target == Some(DropTarget::Chevron) { drop_bg } else { hover_bg }))
        .when(pressed == Some(Source::Chevron), |d| d.bg(press_bg))
        .child(Icon::new(L::ChevronUp).size(px(14. * k)).text_color(pal.text_secondary().hsla()));
    row = row.child(wire(chevron, Source::Chevron));
    Some(
        div()
            .flex()
            .flex_row()
            .items_center()
            .flex_shrink_0()
            .child(row)
            .child(div().mx(px(ts::SEP_GAP * k)).w(px(ts::SEP_W * k)).h(px(super::plan::RIGHT_SEP_HEIGHT * k)).bg(pal.separator().hsla()))
            .into_any_element(),
    )
}

// ---------------------------------------------------------------- what the overflow panel needs

/// The overflow panel's content: how many icons, which one is being dragged, whether a dragged icon is over the panel.
pub struct OverflowData {
    /// The cells the panel draws (all the icons in it, unless the screen is too small for the grid, see `ts::ovf_grid`).
    pub count: usize,
    /// The icons in the panel: the grid is worked out from this number.
    pub total: usize,
    pub limits: ts::OvfLimits,
    pub dragged: Option<usize>,
    pub drop_into: bool,
    pub hover: Option<usize>,
    /// The cell a button is held on (not yet a drag).
    pub pressed: Option<usize>,
}

pub fn overflow_data(cx: &App) -> OverflowData {
    let Some(st) = cx.try_global::<TrayState>() else {
        return OverflowData { count: 0, total: 0, limits: ts::OvfLimits::UNBOUNDED, dragged: None, drop_into: false, hover: None, pressed: None };
    };
    let dragged = st.drag.as_ref().and_then(|d| st.index_of(d.key)).and_then(|i| st.lay.overflow.iter().position(|&o| o == i));
    let hover = match st.hover {
        Some(Source::Overflow(k)) if st.drag.is_none() => Some(k),
        _ => None,
    };
    let pressed = match st.press.as_ref().filter(|p| !p.press.dragging).map(|p| p.source) {
        Some(Source::Overflow(k)) => Some(k),
        _ => None,
    };
    let total = st.lay.overflow.len();
    // (the frame that says "drop here" is for an icon coming from the strip)
    OverflowData {
        count: ts::ovf_grid(total, st.ovf).shown,
        total,
        limits: st.ovf,
        dragged,
        drop_into: dragged.is_none() && st.drag.as_ref().is_some_and(|d| d.target == DropTarget::Overflow),
        hover,
        pressed,
    }
}

/// The empty panel's text, in the user's language, already broken into lines that fit the panel (see `ts::ovf_empty`): the title and the
/// quieter hint under it.
pub fn empty_panel() -> ts::EmptyPanel {
    ts::ovf_empty(crate::t!("Gizli simge yok"), crate::t!("Gizlemek için simgeleri buraya sürükleyin"))
}

/// The panel window's size now, logical px: its grid for the icons it holds (within what the screen allows), or the empty panel's text.
pub fn overflow_size(cx: &App) -> (f32, f32) {
    let d = overflow_data(cx);
    if d.total == 0 { empty_panel().size } else { ts::ovf_size_in(d.total, d.limits) }
}

/// The picture of the overflow panel's cell `k`.
pub fn overflow_picture(k: usize, size: f32, pal: Palette, cx: &App) -> AnyElement {
    match cx.try_global::<TrayState>().and_then(|st| st.lay.overflow.get(k).map(|&i| (st, i))) {
        Some((st, i)) => picture_of(st, i, size, pal),
        None => div().size(px(size)).into_any_element(),
    }
}

/// How many columns the panel has for the icons it holds now.
pub fn overflow_cols(cx: &App) -> usize {
    cx.try_global::<TrayState>().map_or(0, |st| ts::ovf_grid(st.lay.overflow.len(), st.ovf).cols)
}

/// The key of the icon in cell `k` of the overflow panel now.
pub fn overflow_key(k: usize, cx: &App) -> Option<IconKey> {
    let st = cx.try_global::<TrayState>()?;
    st.icons().get(st.icon_of(Source::Overflow(k))?).map(|i| i.key)
}

/// Where the icon with `key` is in the overflow panel now.
pub fn overflow_position(key: IconKey, cx: &App) -> Option<usize> {
    let st = cx.try_global::<TrayState>()?;
    st.lay.overflow.iter().position(|&i| st.icons().get(i).is_some_and(|ic| ic.key == key))
}

/// A click from the keyboard on the icon `key` of the overflow panel (the panel closes after it, whatever became of the icon: the view
/// that asked for this has stopped listening to keys).
pub fn overflow_key_click(key: IconKey, kind: ClickKind, cx: &mut App) {
    if let Some(k) = overflow_position(key, cx)
        && let Some(rect) = source_rect(Source::Overflow(k), cx)
        && let Some(icon) = cx.try_global::<TrayState>().and_then(|st| st.icon_of(Source::Overflow(k)))
    {
        deliver(icon, kind, (rect.left, rect.top), cx);
    }
    super::panel::close(CloseCause::Chosen, cx);
}

/// The island's glass (the tip's tone is the island's).
pub fn island_glass(cx: &App) -> Option<std::rc::Rc<RefCell<super::backdrop::IslandGlass>>> {
    let right = cx.try_global::<TrayState>()?.right.upgrade()?;
    Some(right.read(cx).island_glass())
}

// ---------------------------------------------------------------- the pointer

/// The pointer entered (`on`) or left a source.
pub fn hover(src: Source, on: bool, cx: &mut App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    if on {
        if st.hover != Some(src) {
            cx.update_global::<TrayState, _>(|s, _| s.hover = Some(src));
        }
        let anchor = tip_anchor(src, cx);
        super::tray_tip::hover(anchor, cx);
    } else if st.hover == Some(src) {
        cx.update_global::<TrayState, _>(|s, _| s.hover = None);
        // (moving from one cell to the next is a leave and then an enter in one go: the tip is told only if nothing was entered)
        cx.defer(|cx| {
            if cx.try_global::<TrayState>().is_some_and(|s| s.hover.is_none()) {
                super::tray_tip::hover(None, cx);
            }
        });
    }
}

/// The tip for a source: the icon's cleaned tip and where it is, or the chevron's name. `None`: the icon gave no tip.
fn tip_anchor(src: Source, cx: &App) -> Option<super::tray_tip::Anchor> {
    let st = cx.try_global::<TrayState>()?;
    let rect = source_rect(src, cx)?;
    let (key, text) = match src {
        Source::Chevron => (CHEVRON_KEY, crate::t!("Gizli simgeleri göster").to_string()),
        _ => {
            let icon = st.icons().get(st.icon_of(src)?)?;
            (icon.key, rules::clean(&icon.tip))
        }
    };
    // a tip over the strip clears the island's top edge, not just the 24 px cell inside it (an overflow cell has no island under it)
    let clear_top = match src {
        Source::Overflow(_) => rect.top,
        _ => glass_and_unit(cx).map_or(rect.top, |(glass, _)| glass.top.min(rect.top)),
    };
    (!text.is_empty()).then_some(super::tray_tip::Anchor { key, text, rect, clear_top })
}

/// A button went down on a source.
pub fn press(src: Source, button: PointerButton, count: usize, cx: &mut App) {
    let Some(at) = geo::cursor() else { return };
    let Some(st) = cx.try_global::<TrayState>() else { return };
    // (a second button pressed while one is held is ignored; every way out of `poll_once` that ends a press clears it)
    if st.press.is_some() {
        return;
    }
    let (key, id) = match src {
        Source::Chevron => (None, None),
        _ => match st.icon_of(src).and_then(|i| st.icons().get(i)) {
            Some(icon) => (Some(icon.key), Some(icon.id.clone())),
            None => return,
        },
    };
    super::tray_tip::press(cx);
    cx.global_mut::<TrayState>().press = Some(PressState { press: Press { button, count, at, dragging: false }, source: src, key, id });
    let task = cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(POLL_EVERY).await;
            if !cx.update(poll_once) {
                break;
            }
        }
    });
    cx.global_mut::<TrayState>().poll = Some(task);
}

/// One look at the held button. False when the press is over.
fn poll_once(cx: &mut App) -> bool {
    let Some(st) = cx.try_global::<TrayState>() else { return false };
    let Some(mut ps) = st.press.clone() else { return false };
    let scale = host::monitor_work(cx).map_or(1.0, |(s, _)| s);
    let cursor = geo::cursor().unwrap_or(ps.press.at);
    let down = geo::any_button_down();

    // the chevron is only ever clicked
    let Some(key) = ps.key else {
        if down {
            return true;
        }
        let inside = source_rect(Source::Chevron, cx).is_some_and(|r| r.contains(cursor.0, cursor.1));
        end_press(cx);
        if inside && ps.press.button == PointerButton::Left {
            super::panel::toggle_overflow(cx);
        }
        return false;
    };

    match ts::step(&mut ps.press, cursor, down, scale) {
        Step::Idle => {
            cx.global_mut::<TrayState>().press = Some(ps);
            true
        }
        Step::DragBegan => {
            cx.update_global::<TrayState, _>(|s, _| {
                s.press = Some(ps);
                s.drag = Some(Drag { key, target: DropTarget::Nowhere });
            });
            super::tray_tip::block(true, cx);
            true
        }
        Step::DragMove => {
            cx.global_mut::<TrayState>().press = Some(ps);
            if let Some(target) = drop_target_at(cursor, cx)
                && cx.try_global::<TrayState>().and_then(|s| s.drag).is_some_and(|d| d.target != target)
            {
                cx.update_global::<TrayState, _>(|s, _| {
                    if let Some(d) = &mut s.drag {
                        d.target = target;
                    }
                });
            }
            true
        }
        Step::Released { dragged } => {
            end_press(cx);
            if dragged {
                if let Some(target) = drop_target_at(cursor, cx) {
                    drop_icon(key, target, cx);
                }
            } else {
                finish_click(&ps, cursor, cx);
            }
            false
        }
    }
}

/// The press is over: nothing is held, dragged or blocking the tips.
fn end_press(cx: &mut App) {
    cx.update_global::<TrayState, _>(|s, _| {
        s.press = None;
        s.drag = None;
        s.poll = None;
    });
    sync_tip_block(cx);
}

/// Tips stay shut while an icon is dragged or the keyboard is in the strip (a tip opened by the pointer would hang over the ring and over
/// a menu the keyboard opened). Called whenever either changes.
fn sync_tip_block(cx: &mut App) {
    let on = cx.try_global::<TrayState>().is_some_and(|s| s.drag.is_some() || s.kb.is_some());
    super::tray_tip::block(on, cx);
}

fn drop_target_at(cursor: (i32, i32), cx: &App) -> Option<DropTarget> {
    let st = cx.try_global::<TrayState>()?;
    let (glass, unit) = glass_and_unit(cx)?;
    Some(ts::drop_target(&Geometry { glass, scale: unit, visible: st.lay.visible.len(), chevron: st.lay.chevron, overflow: overflow_rect(cx) }, cursor))
}

/// A dragged icon was let go on `target`: the order changes (and is written) when that moves something.
fn drop_icon(key: IconKey, target: DropTarget, cx: &mut App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    let Some(icon) = st.index_of(key) else { return };
    let in_overflow = st.lay.overflow.contains(&icon);
    let Some((dest, at)) = ts::drop_move(&st.lay.visible, icon, in_overflow, target, ts::capacity(st.limit)) else { return };
    let changed = cx.update_global::<TrayState, _>(|s, _| s.move_icon(icon, dest, at));
    if changed {
        save_order(cx);
        after_change(cx);
    }
}

/// A press that did not become a drag is a click, if the pointer is still on the cell it went down on.
fn finish_click(ps: &PressState, cursor: (i32, i32), cx: &mut App) {
    let Some(icon) = ps.key.and_then(|k| cx.try_global::<TrayState>().and_then(|s| s.index_of(k))) else { return };
    // the key must still be the icon that was pressed (the host refuses the click as well when it holds another one)
    if cx.try_global::<TrayState>().and_then(|s| s.icons().get(icon)).map(|i| &i.id) != ps.id.as_ref() {
        return;
    }
    if !source_rect(ps.source, cx).is_some_and(|r| r.contains(cursor.0, cursor.1)) {
        return;
    }
    deliver(icon, ts::pointer_click(ps.press.button, ps.press.count), cursor, cx);
    // what was chosen in the panel puts the panel away (a menu the program opens takes the foreground anyway)
    if matches!(ps.source, Source::Overflow(_)) {
        super::panel::close(CloseCause::Chosen, cx);
    }
}

fn deliver(icon: usize, kind: ClickKind, at: (i32, i32), cx: &App) {
    let Some((key, id)) = cx.try_global::<TrayState>().and_then(|s| s.icons().get(icon)).map(|i| (i.key, i.id.clone())) else { return };
    if !tray_source::deliver_click(key, TrayClick { kind, at, id }) {
        log::debug!("tray: a click on an icon that is not there any more");
    }
}

// ---------------------------------------------------------------- the order file

static NEXT_WRITE: AtomicU64 = AtomicU64::new(1);
/// The newest write that has been made; an older one that comes late is dropped (writes are numbered, as the pin files' are).
static LAST_WRITE: Mutex<u64> = Mutex::new(0);

/// Writes the order on a worker thread.
fn save_order(cx: &mut App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    let (order, path, aside) = (st.order.clone(), st.order_path.clone(), st.unreadable);
    let Some(path) = path else { return };
    cx.global_mut::<TrayState>().unreadable = false;
    let seq = NEXT_WRITE.fetch_add(1, Ordering::SeqCst);
    cx.background_executor()
        .spawn(async move {
            let mut last = LAST_WRITE.lock().unwrap_or_else(|p| p.into_inner());
            if seq < *last {
                return;
            }
            if aside && path.exists() && let Err(e) = tray_order::set_aside(&path) {
                log::warn!("tray: the unreadable order file could not be set aside, nothing is written: {e}");
                return;
            }
            match tray_order::save(&path, &order) {
                Ok(()) => *last = seq,
                Err(e) => log::warn!("tray: the order file could not be written: {e}"),
            }
        })
        .detach();
}

// ---------------------------------------------------------------- the keyboard

/// Moves the keyboard into the strip: the island's window takes the foreground and the ring is on the first icon. False: it cannot (no
/// icons, a full-screen program in front, the keyboard is in the strip already). Nothing calls this by itself.
pub fn focus_strip(cx: &mut App) -> bool {
    let Some(st) = cx.try_global::<TrayState>() else { return false };
    if !st.lay.chevron || st.kb.is_some() || host::is_fullscreen(cx) {
        return false;
    }
    let Some((handle, hwnd)) = host::island_window(Kind::Right, cx) else { return false };
    let Some(right) = st.right.upgrade() else { return false };
    // (the island has a focus handle once it has drawn a strip; before that there are no keys to give it)
    let Some(focus) = right.read(cx).focus_handle() else { return false };
    let previous = rf_win::dock::foreground_root();
    let first = st.key_at_stop(0);
    cx.update_global::<TrayState, _>(|s, _| {
        s.kb = Some(Keyboard { cursor: 0, on: first, previous: if previous == hwnd { 0 } else { previous }, opened_at: Instant::now(), been_front: false });
    });
    sync_tip_block(cx);
    // (taking the foreground sends the window messages that the toolkit answers by updating the app: from a task, not from here)
    cx.spawn(async move |cx| {
        let got = ts::take_keyboard(hwnd);
        cx.update(|cx| {
            if got {
                let _ = handle.update(cx, |_, window, cx| window.focus(&focus, cx));
            }
            log::info!("tray: the keyboard is in the strip ({})", if got { "the island has the foreground" } else { "Windows refused the foreground; it is given up after a moment" });
            let task = cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(KEY_WATCH_EVERY).await;
                    if !cx.update(|cx| key_look(hwnd, cx)) {
                        break;
                    }
                }
            });
            if let Some(s) = cx.try_global::<TrayState>().filter(|s| s.kb.is_some()) {
                let _ = s;
                cx.global_mut::<TrayState>().kb_watch = Some(task);
            }
        });
    })
    .detach();
    true
}

/// One look of the island with the keyboard in it at the foreground. False when the keyboard has left the strip.
fn key_look(hwnd: isize, cx: &mut App) -> bool {
    let Some(st) = cx.try_global::<TrayState>() else { return false };
    let Some(kb) = st.kb else { return false };
    let fg = rf_win::dock::foreground_root();
    let front = fg == hwnd;
    if front && !kb.been_front {
        cx.global_mut::<TrayState>().kb = Some(Keyboard { been_front: true, ..kb });
    }
    if geo::lost_foreground(hwnd, fg, kb.been_front || front, kb.opened_at.elapsed().as_millis() as u64) || host::is_fullscreen(cx) {
        leave_keyboard(GiveBack::No, cx);
        return false;
    }
    true
}

/// Whether the window that had the keyboard before gets it back when the keyboard leaves the strip (only when nobody took it meanwhile:
/// `geo::restore_to`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GiveBack {
    /// At once (Escape, Tab).
    Now,
    /// After [`CLICK_SETTLE`]: a click was sent, and the program may take the foreground itself (a menu, a window). If it did not (an icon
    /// that does its work in the background, a click that was refused), the island would keep the keyboard and swallow the user's typing.
    AfterClick,
    /// Never: the foreground is somebody else's already, or the overflow panel was opened and carries the window to go back to.
    No,
}

/// The keyboard leaves the strip.
fn leave_keyboard(back: GiveBack, cx: &mut App) {
    let Some(st) = cx.try_global::<TrayState>() else { return };
    let Some(kb) = st.kb else { return };
    cx.update_global::<TrayState, _>(|s, _| {
        s.kb = None;
        s.kb_watch = None;
    });
    sync_tip_block(cx);
    let hwnd = host::island_window(Kind::Right, cx).map_or(0, |(_, h)| h);
    if back != GiveBack::No && kb.previous != 0 {
        cx.spawn(async move |cx| {
            if back == GiveBack::AfterClick {
                cx.background_executor().timer(CLICK_SETTLE).await;
            }
            let fg = rf_win::dock::foreground_root();
            if let Some(target) = geo::restore_to(kb.previous, fg, hwnd, geo::exists(kb.previous)) {
                geo::give_back(target);
            }
        })
        .detach();
    }
}

/// A key pressed in the island's window. True when the strip used it.
pub fn key_down(ev: &KeyDownEvent, cx: &mut App) -> bool {
    let Some(st) = cx.try_global::<TrayState>() else { return false };
    let Some(kb) = st.kb else { return false };
    let m = ev.keystroke.modifiers;
    let Some(key) = ts::nav_key(&ev.keystroke.key, m.shift, m.control, m.alt) else { return false };
    let (visible, chevron) = (st.lay.visible.len(), st.lay.chevron);
    match ts::strip_nav(key, kb.cursor, visible, chevron) {
        StripOutcome::Stay => {}
        StripOutcome::Moved(c) => cx.update_global::<TrayState, _>(|s, _| {
            let on = s.key_at_stop(c);
            if let Some(k) = &mut s.kb {
                k.cursor = c;
                k.on = on;
            }
        }),
        StripOutcome::Click(kind, _) => {
            // the icon the ring is on by its key (not by the place the ring had when it was last drawn)
            let target = kb.on.and_then(|key| {
                let st = cx.try_global::<TrayState>()?;
                let j = st.lay.visible.iter().position(|&i| st.icons().get(i).is_some_and(|ic| ic.key == key))?;
                Some((st.lay.visible[j], j))
            });
            if let Some((icon, j)) = target
                && let Some(rect) = source_rect(Source::Strip(j), cx)
            {
                deliver(icon, kind, (rect.left, rect.top), cx);
            }
            leave_keyboard(GiveBack::AfterClick, cx);
        }
        StripOutcome::ToggleOverflow => {
            // (the panel is told which window had the keyboard: the foreground is the island now, which is no window to go back to)
            super::panel::toggle_overflow_from_keyboard(kb.previous, cx);
            leave_keyboard(GiveBack::No, cx);
        }
        StripOutcome::Leave => leave_keyboard(GiveBack::Now, cx),
    }
    true
}

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    // (not `super::*`: that would bring in the toolkit's own `test` attribute, which shadows the standard one)
    use super::{Arrangement, Arc, Dest, DropTarget, Drag, HashMap, IconKey, Instant, Keyboard, Layout, Mutex, Source, StableId, TrayOrder, TrayState, WeakEntity, rules, to_image, tray_source, ts};

    #[test]
    fn with_no_host_a_refresh_does_nothing_and_the_order_file_is_read_only_when_the_first_snapshot_comes() {
        use super::{OrderLoad::*, RefreshPlan::*, refresh_plan};
        // the host is off and was: no work, whatever the order file's state
        for o in [NotYet, Reading, Done] {
            assert_eq!(refresh_plan(false, false, o), Nothing);
        }
        // the first snapshot starts the read once; while it runs snapshots wait; then they are applied
        assert_eq!(refresh_plan(true, false, NotYet), ReadOrder);
        assert_eq!(refresh_plan(true, false, Reading), WaitForOrder);
        assert_eq!(refresh_plan(true, false, Done), Apply);
        assert_eq!(refresh_plan(true, true, Done), Apply);
        // a host that goes away must be shown going away
        assert_eq!(refresh_plan(false, true, Done), Apply);
    }

    #[test]
    fn the_strips_padding_is_the_islands() {
        assert_eq!(ts::SIDE_PAD, super::super::plan::SIDE_PAD);
        assert_eq!(ts::SEP_GAP, super::super::plan::RIGHT_SEP_GAP);
        assert_eq!(ts::SEP_W, rf_win::dock::layout::SEPARATOR_WIDTH);
    }

    #[test]
    fn a_picture_of_the_wrong_size_is_not_converted() {
        let good = tray_source::Bitmap { width: 2, height: 1, rgba: vec![1, 2, 3, 4, 5, 6, 7, 8] };
        assert!(to_image(&good).is_some());
        let bad = tray_source::Bitmap { width: 2, height: 2, rgba: vec![0; 7] };
        assert!(to_image(&bad).is_none());
        let empty = tray_source::Bitmap { width: 0, height: 0, rgba: vec![] };
        assert!(to_image(&empty).is_none());
    }

    fn state(limit: f32) -> TrayState {
        TrayState {
            right: WeakEntity::new_invalid(),
            snap: None,
            ids: Vec::new(),
            order: TrayOrder::new(),
            order_path: None,
            order_load: super::OrderLoad::Done,
            unreadable: false,
            arr: Arrangement::default(),
            lay: Layout::default(),
            limit,
            ovf: ts::OvfLimits::for_work(2560.0, 1392.0),
            images: HashMap::new(),
            hover: None,
            press: None,
            drag: None,
            kb: None,
            poll: None,
            kb_watch: None,
            _sub: None,
        }
    }

    /// A work area `work_width` px wide gives a quarter of it, at most 240. The user has put every one of the made-up icons on the strip
    /// (a new icon starts in the hidden-icons panel; the tests of the strip's layout begin from a strip that is full).
    fn loaded(work_width: f32) -> TrayState {
        let mut st = state(0.0);
        let snap = tray_source::test_snapshot(0);
        let ids: Vec<StableId> = snap.icons.iter().map(|i| i.id.clone()).collect();
        for i in 0..ids.len() {
            st.order.reorder(&ids, i, Dest::Strip, i);
        }
        let _ = st.set_snapshot(Some(snap), Some(work_width));
        st
    }

    #[test]
    fn with_nothing_placed_by_the_user_every_icon_waits_in_the_panel_and_the_chevron_is_there() {
        let mut st = state(0.0);
        let _ = st.set_snapshot(Some(tray_source::test_snapshot(0)), Some(2560.0));
        assert!(st.lay.visible.is_empty(), "as on Windows: only what the user drags to the strip is on it");
        // (icons nobody placed are listed by their identity, not by arrival: the order is `rf_win`'s rule, tested there; here all six are in it)
        let sorted = |v: &[usize]| {
            let mut v = v.to_vec();
            v.sort_unstable();
            v
        };
        assert_eq!(sorted(&st.lay.overflow), vec![0, 1, 2, 3, 4, 5]);
        assert!(st.lay.chevron);
        assert_eq!(st.lay.extra_width(), 24.0 + 17.0, "just the chevron and the separator");
        // the user drags the second icon to the strip: it is the only one there, the rest stay in the panel
        let (dest, at) = ts::drop_move(&st.lay.visible, 1, true, DropTarget::Strip(0), 9).unwrap();
        assert!(st.move_icon(1, dest, at));
        assert_eq!(st.lay.visible, vec![1]);
        assert_eq!(sorted(&st.lay.overflow), vec![0, 2, 3, 4, 5]);
        // a new program's icon arrives: in the panel
        let mut snap = tray_source::test_snapshot(0);
        let mut extra = snap.icons[0].clone();
        extra.key = 900;
        extra.id = StableId::from_program(r"C:\Fake\Yeni.exe", 9);
        snap.icons.push(extra);
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        assert_eq!(st.lay.visible, vec![1]);
        assert_eq!(st.lay.overflow.len(), 6);
    }

    #[test]
    fn the_chevron_stays_when_every_icon_is_on_the_strip_so_one_can_be_hidden_again() {
        let st = loaded(2560.0);
        assert!(st.lay.overflow.is_empty() && st.lay.chevron);
        assert_eq!(super::ts::ovf_grid(0, st.ovf).shown, 0);
    }

    #[test]
    fn the_panels_size_follows_its_icons_and_its_text() {
        let mut st = loaded(2560.0);
        // nothing in the panel: the two lines of text decide
        let e = super::empty_panel();
        assert_eq!(e.size.0, ts::OVF_EMPTY_W);
        assert!(e.size.1 > ts::OVF_EMPTY_H, "a title and a hint of at least one line each");
        // one, nine and thirty icons in the panel (rows of a fixed number; a fixed limit: the screen of the test)
        let mut counts = Vec::new();
        for n in [1usize, 9, 30] {
            let mut snap = tray_source::test_snapshot(0);
            let template = snap.icons[0].clone();
            snap.icons.clear();
            for i in 0..n {
                let mut ic = template.clone();
                ic.key = 1000 + i as u64;
                ic.id = StableId::from_program(r"C:\Fake\Many.exe", i as u32);
                snap.icons.push(ic);
            }
            let _ = st.set_snapshot(Some(snap), Some(2560.0));
            assert_eq!(st.lay.overflow.len(), n, "none of them was placed on the strip");
            let (cols, rows) = (ts::ovf_grid(n, st.ovf).cols, ts::ovf_grid(n, st.ovf).rows);
            let (w, h) = ts::ovf_size_in(n, st.ovf);
            assert!(w <= st.ovf.max_w && h <= st.ovf.max_h);
            counts.push((n, cols, rows));
        }
        assert_eq!(counts, vec![(1, 1, 1), (9, 5, 2), (30, 6, 5)]);
    }

    #[test]
    fn six_icons_fit_the_strip_and_each_has_a_picture() {
        let st = loaded(2560.0);
        assert_eq!(st.lay.visible, vec![0, 1, 2, 3, 4, 5]);
        assert!(st.lay.overflow.is_empty());
        assert!(st.lay.chevron);
        assert_eq!(st.images.len(), 6);
        assert_eq!(st.lay.extra_width(), 6.0 * 24.0 + 24.0 + 17.0);
    }

    #[test]
    fn on_a_narrow_screen_the_last_icons_go_to_the_overflow() {
        // 480 px work area: a quarter is 120 px: the chevron and four cells
        let st = loaded(480.0);
        assert_eq!(st.lay.visible, vec![0, 1, 2, 3]);
        assert_eq!(st.lay.overflow, vec![4, 5]);
        // the screen grows again: they come back
        let mut st = st;
        let _ = st.set_snapshot(Some(tray_source::test_snapshot(0)), Some(2560.0));
        assert_eq!(st.lay.visible.len(), 6);
    }

    #[test]
    fn a_picture_is_made_again_only_when_its_generation_moves() {
        let mut st = loaded(2560.0);
        let first = st.images[&101].1.clone();
        let other = st.images[&102].1.clone();
        let _ = st.set_snapshot(Some(tray_source::test_snapshot(0)), Some(2560.0));
        assert!(Arc::ptr_eq(&first, &st.images[&101].1), "the same generation: the same picture");
        let retired = st.set_snapshot(Some(tray_source::test_snapshot(1)), Some(2560.0));
        assert!(!Arc::ptr_eq(&first, &st.images[&101].1), "the mail icon changed: a new picture");
        assert_eq!(retired.len(), 1, "the replaced picture is handed back to be dropped from the atlas");
        assert!(Arc::ptr_eq(&retired[0], &first));
        assert!(Arc::ptr_eq(&other, &st.images[&102].1), "the others did not");
    }

    /// The whole way of a drop on the chevron, as `drop_icon` makes it: the strip is empty (every icon waits in the panel), so the
    /// chevron is the only place there is, and an icon that comes from the panel lands on the strip there.
    #[test]
    fn an_icon_dragged_from_the_panel_onto_the_chevron_of_an_empty_strip_comes_to_the_strip() {
        let mut st = state(0.0);
        let _ = st.set_snapshot(Some(tray_source::test_snapshot(0)), Some(2560.0));
        assert!(st.lay.visible.is_empty() && st.lay.chevron);
        let cap = ts::capacity(st.limit);
        for (icon, want) in [(3usize, vec![3usize]), (0, vec![3, 0])] {
            let in_overflow = st.lay.overflow.contains(&icon);
            assert!(in_overflow);
            let (dest, at) = ts::drop_move(&st.lay.visible, icon, in_overflow, DropTarget::Chevron, cap).unwrap();
            assert!(st.move_icon(icon, dest, at));
            assert_eq!(st.lay.visible, want);
            assert!(!st.lay.overflow.contains(&icon));
        }
        // and an icon of the strip let go there is hidden again
        let (dest, at) = ts::drop_move(&st.lay.visible, 3, st.lay.overflow.contains(&3), DropTarget::Chevron, cap).unwrap();
        assert!(st.move_icon(3, dest, at));
        assert_eq!(st.lay.visible, vec![0]);
        assert!(st.lay.overflow.contains(&3));
    }

    #[test]
    fn hiding_and_showing_an_icon_moves_it_between_the_strip_and_the_overflow() {
        let mut st = loaded(2560.0);
        // drop icon 1 (the second) on the chevron
        let (dest, at) = ts::drop_move(&st.lay.visible, 1, false, DropTarget::Chevron, 9).unwrap();
        assert!(st.move_icon(1, dest, at));
        assert_eq!(st.lay.visible, vec![0, 2, 3, 4, 5]);
        assert_eq!(st.lay.overflow, vec![1]);
        // drop it back before the first cell
        let (dest, at) = ts::drop_move(&st.lay.visible, 1, true, DropTarget::Strip(0), 9).unwrap();
        assert!(st.move_icon(1, dest, at));
        assert_eq!(st.lay.visible, vec![1, 0, 2, 3, 4, 5]);
        assert!(st.lay.overflow.is_empty());
    }

    #[test]
    fn pictures_that_are_replaced_or_whose_icon_went_are_retired_and_the_others_stay() {
        let mut st = loaded(2560.0);
        let before: Vec<_> = (101..=106).map(|k| st.images[&k].1.clone()).collect();
        // nothing changed: nothing is retired
        assert!(st.set_snapshot(Some(tray_source::test_snapshot(0)), Some(2560.0)).is_empty());
        // an icon went away
        let mut snap = tray_source::test_snapshot(0);
        snap.icons.retain(|i| i.key != 103);
        let retired = st.set_snapshot(Some(snap), Some(2560.0));
        assert_eq!(retired.len(), 1);
        assert!(Arc::ptr_eq(&retired[0], &before[2]));
        // the host went away: every picture that was left is retired
        let retired = st.set_snapshot(None, Some(2560.0));
        assert_eq!(retired.len(), 5);
        assert!(st.images.is_empty());
    }

    #[test]
    fn the_ring_stays_on_its_icon_when_icons_come_and_go() {
        let mut st = loaded(2560.0);
        // the ring is on the third icon (key 103)
        st.kb = Some(Keyboard { cursor: 2, on: st.key_at_stop(2), previous: 0, opened_at: Instant::now(), been_front: false });
        assert_eq!(st.kb.unwrap().on, Some(103));
        // a new program's icon is listed first: it waits in the panel, the strip is as it was and the ring stays
        let mut snap = tray_source::test_snapshot(0);
        let mut extra = snap.icons[0].clone();
        extra.key = 900;
        extra.id = rf_win::dock::tray_order::StableId::from_program(r"C:\Fake\Yeni.exe", 9);
        snap.icons.insert(0, extra);
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        let k = st.kb.unwrap();
        assert_eq!((k.cursor, k.on), (2, Some(103)));
        assert_eq!(st.lay.overflow.len(), 1);
        // the user puts the new icon at the front of the strip: everything is one place later and the ring went with its icon (by place
        // alone it would now be on the icon that was second)
        let moved = st.index_of(900).unwrap();
        let (dest, at) = ts::drop_move(&st.lay.visible, moved, true, DropTarget::Strip(0), 9).unwrap();
        assert!(st.move_icon(moved, dest, at));
        let k = st.kb.unwrap();
        assert_eq!((k.cursor, k.on), (3, Some(103)), "the ring is where its icon is now");
        assert_eq!(st.key_at_stop(k.cursor), Some(103));
        // its icon goes: the ring stays at the nearest stop and names the icon that is there
        let mut snap = tray_source::test_snapshot(0);
        snap.icons.retain(|i| i.key != 103);
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        let k = st.kb.unwrap();
        assert_eq!(st.key_at_stop(k.cursor), k.on);
        assert_ne!(k.on, Some(103));
    }

    #[test]
    fn dropping_an_overflow_icon_on_the_end_gap_of_a_full_strip_shows_it() {
        // 480 px work area: a quarter is 120 px, the chevron and four cells; icons 4 and 5 are in the overflow
        let mut st = loaded(480.0);
        let cap = ts::capacity(st.limit);
        assert_eq!((cap, st.lay.visible.len()), (4, 4));
        assert_eq!(st.lay.overflow, vec![4, 5]);
        let (dest, at) = ts::drop_move(&st.lay.visible, 4, true, DropTarget::Strip(4), cap).unwrap();
        assert!(st.move_icon(4, dest, at));
        assert!(st.lay.visible.contains(&4), "it is in the strip");
        assert_eq!(st.lay.visible.last(), Some(&4), "as its last cell");
        assert_eq!(st.lay.overflow.len(), 2, "the last icon of the strip made room by going to the overflow");
        // (before: the place past the shown icons, which un-hid it and left it in the overflow panel)
        let mut st = loaded(480.0);
        let (dest, at) = ts::drop_move(&st.lay.visible, 4, true, DropTarget::Strip(4), usize::MAX).unwrap();
        let _ = st.move_icon(4, dest, at);
        assert!(!st.lay.visible.contains(&4));
    }

    #[test]
    fn the_hosts_rectangles_are_resent_after_it_went_away_and_came_back() {
        let rects = vec![(101, super::Rect::from_xywh(1, 2, 24, 24))];
        let mut m = super::RectMemo::default();
        assert!(m.changed(&rects), "the first telling");
        assert!(!m.changed(&rects), "nothing moved: nothing is sent");
        m.forget();
        assert!(m.changed(&rects), "the host came back: it is told again, though nothing moved");
    }

    #[test]
    fn a_host_that_goes_away_forgets_what_it_was_told() {
        super::SENT.with(|s| {
            let _ = s.borrow_mut().changed(&[(1, super::Rect::from_xywh(0, 0, 24, 24))]);
        });
        let mut st = loaded(2560.0);
        let _ = st.set_snapshot(None, Some(2560.0));
        assert!(super::SENT.with(|s| s.borrow_mut().changed(&[(1, super::Rect::from_xywh(0, 0, 24, 24))])));
    }

    #[test]
    fn the_order_survives_the_hosts_listing_in_another_order() {
        let mut st = loaded(2560.0);
        let (dest, at) = ts::drop_move(&st.lay.visible, 5, false, DropTarget::Strip(0), 9).unwrap();
        st.move_icon(5, dest, at);
        // the host lists the same icons reversed
        let mut snap = tray_source::test_snapshot(0);
        snap.icons.reverse();
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        let keys: Vec<IconKey> = st.lay.visible.iter().map(|&i| st.icons()[i].key).collect();
        assert_eq!(keys[0], 106, "the icon the user put first is first");
    }

    #[test]
    fn a_dragged_icon_that_goes_away_ends_the_drag_and_a_host_that_goes_ends_the_strip() {
        let mut st = loaded(2560.0);
        st.drag = Some(Drag { key: 103, target: DropTarget::Nowhere });
        let mut snap = tray_source::test_snapshot(0);
        snap.icons.retain(|i| i.key != 103);
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        assert!(st.drag.is_none());
        assert_eq!(st.lay.visible.len(), 5);
        // the keyboard cursor was on the chevron; the strip is one shorter
        st.kb = Some(Keyboard { cursor: 5, on: Some(106), previous: 0, opened_at: Instant::now(), been_front: false });
        let mut snap = tray_source::test_snapshot(0);
        snap.icons.truncate(2);
        let _ = st.set_snapshot(Some(snap), Some(2560.0));
        assert_eq!(st.kb.unwrap().cursor, 2, "pulled onto the chevron, which is the last stop");
        // the host is off: no strip at all
        st.hover = Some(Source::Strip(0));
        let _ = st.set_snapshot(None, Some(2560.0));
        assert!(st.lay.visible.is_empty() && !st.lay.chevron);
        assert_eq!(st.lay.extra_width(), 0.0);
        assert!(st.kb.is_none() && st.hover.is_none());
    }

    #[test]
    fn the_cleaned_tip_is_what_a_tip_window_would_show() {
        let st = loaded(2560.0);
        let tips: Vec<String> = st.icons().iter().map(|i| rules::clean(&i.tip)).collect();
        assert_eq!(tips[0], "Posta - 3 yeni ileti");
        assert!(tips[5].is_empty(), "the icon with no tip has no tooltip");
        assert!(tips[3].contains('\n'));
    }

    /// The channel the source's callback uses is `Send + Sync` through a mutex (a plain sender is not required to be `Sync`).
    #[test]
    fn the_subscription_callback_can_be_shared_between_threads() {
        fn needs<T: Send + Sync + 'static>(_: &T) {}
        let (tx, _rx) = futures::channel::mpsc::unbounded::<()>();
        let tx = Mutex::new(tx);
        let cb = move || {
            if let Ok(tx) = tx.lock() {
                let _ = tx.unbounded_send(());
            }
        };
        needs(&cb);
    }
}
