//! The GPUI view of the desktop: state, what arrives from the worker threads, and drawing. The mouse and keyboard are `input.rs`, the
//! operations on files are `operations.rs`, the menus are `menus.rs`; all of them are `impl DesktopView`.
//!
//! COST. Nothing runs at rest: no timer, no per-frame work. The view repaints when something changed (a message from a worker, a
//! mouse or key event, an icon that arrived). The wallpaper is one bitmap, made on a worker thread for the size it is shown at and
//! uploaded once. Icons are asked of the shared `ImageCache` as they are drawn, which fetches them lazily and keeps them under its
//! own budget, as the file list does. A label's line breaking is worked out once per (name, width, font size) and remembered.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::InputState;
use gpui_kit::component::{Icon, Sizable as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::desktop::geom::{Rect, Rgb, Size};
use rf_core::desktop::grid::{Cell, Grid, metrics};
use rf_core::desktop::items::{DesktopItem, ItemChange, ItemId, Origin, Visibility, sort_items};
use rf_core::desktop::layout::{Listing, Resolved};
use rf_core::desktop::placement::Position;
use rf_core::desktop::store::{LoadOutcome, Store};
use rf_core::desktop::virtuals::VirtualKind;
use rf_core::desktop::wallpaper::Wallpaper;
use rf_core::entry::{Entry, EntryKind};

use super::args::PreviewArgs;
use super::drop::{self, Accepts, DropAction, Over, Where};
use super::geometry::Mapping;
use super::label::{self, Lines};
use super::names;
use super::select::{Placed, Selection, rect_of};
use super::source::{self, Backend, Loaded};
use super::tile::TileLayout;
use super::typeahead::TypeSelect;
use super::wall;
use crate::icons;
use crate::image_cache::{ImageCache, to_render_image};
use crate::t;
use crate::theme::{self, Palette};

/// Height of the strip under the canvas, in logical pixels.
pub(super) const STATUS_H: f32 = 26.0;

pub(super) enum Msg {
    Loaded(Box<Loaded>),
    Changed(Vec<ItemChange>),
    Wallpaper(Box<Wallpaper>),
    VirtualNames(Vec<(VirtualKind, String)>),
}

/// A mouse press on an item that may become a drag.
pub(super) struct Press {
    pub id: ItemId,
    pub at: Point<Pixels>,
}

pub(super) struct Band {
    pub start: (i32, i32),
    pub cur: (i32, i32),
    pub base: BTreeSet<ItemId>,
    pub additive: bool,
}

/// Icons being dragged inside the window.
pub(super) struct Drag {
    pub ids: Vec<ItemId>,
    pub grabbed: ItemId,
    pub start: (i32, i32),
    pub cur: (i32, i32),
    pub over: Option<Over>,
    /// Every dragged item is a file or folder on the disk (so it can be handed to a folder or leave the window).
    pub files: bool,
}

pub(super) struct Renaming {
    pub id: ItemId,
    pub input: Entity<InputState>,
    pub old: String,
    pub is_dir: bool,
    pub hidden_ext: Option<String>,
}

struct WallImage {
    image: Arc<RenderImage>,
    size: Size,
}

#[derive(Hash, PartialEq, Eq)]
struct WrapKey {
    name: String,
    width: u32,
    font: u32,
}

/// How a tile is drawn.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Look {
    Normal,
    /// The original of an item that is being dragged: faint, in its place.
    Faded,
    /// The copy that follows the pointer: faint, without handlers.
    Ghost,
}

/// What `render` works out once for all tiles.
struct Frame {
    p: Palette,
    tl: TileLayout,
    show_ext: bool,
    thumbs: bool,
    shadow: bool,
    /// Font size of the labels in logical pixels.
    font: f32,
    icon_px: u32,
}

pub struct DesktopView {
    pub(super) backend: Option<Arc<Backend>>,
    pub(super) store: Store,
    pub(super) images: Entity<ImageCache>,
    pub(super) focus: FocusHandle,
    focused_once: bool,
    // the model
    pub(super) items: Vec<DesktopItem>,
    pub(super) by_id: HashMap<ItemId, usize>,
    pub(super) resolved: Resolved,
    /// The items on the canvas, in column-major order.
    pub(super) placed: Vec<Placed>,
    pub(super) grid: Option<Grid>,
    pub(super) tl: Option<TileLayout>,
    virtual_names: HashMap<VirtualKind, String>,
    // geometry of the last frame
    pub(super) map: Mapping,
    // interaction
    pub(super) sel: Selection,
    pub(super) typing: super::typeahead::TypeSelect,
    pub(super) press: Option<Press>,
    pub(super) band: Option<Band>,
    pub(super) drag: Option<Drag>,
    pub(super) ext_hover: Option<(i32, i32)>,
    pub(super) renaming: Option<Renaming>,
    /// A new folder / file (or a rename the shell menu asked for) whose rename box opens as soon as the model shows the item; given up
    /// after a few seconds.
    pub(super) want_rename: Option<(ItemId, Instant)>,
    pub(super) reselect: Option<(Vec<ItemId>, Instant)>,
    pub(super) wheel: f32,
    pub(super) wheel_ms: u64,
    pub(super) notice: Option<String>,
    pub(super) boot: Instant,
    pub(super) rename_sub: Option<Subscription>,
    // wallpaper
    wall_src: Option<Wallpaper>,
    wall: Option<WallImage>,
    wall_asked: Option<Size>,
    /// A wallpaper is being made (one at a time: a picture of 25 megapixels is a hundred megabytes while it is decoded) ...
    wall_busy: bool,
    /// ... and which wallpaper setting it was made for, so that a picture of the old setting is thrown away.
    wall_gen: u64,
    // misc
    wrap: HashMap<WrapKey, Lines>,
    /// How many lines each item's label has in the last frame (its click area follows).
    line_counts: HashMap<ItemId, usize>,
    /// The user arranged something here (see `touch_layout`).
    touched: bool,
    /// The newest running file operation, for the strip (this window has no Status Center button), and the last failure shown.
    ops_line: Option<String>,
    ops_failure: u64,
    save_task: Option<Task<()>>,
    pub(super) load_note: Option<String>,
    pub(super) tx: mpsc::UnboundedSender<Msg>,
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

fn rgb_of(c: Rgb) -> Hsla {
    gpui_kit::rgb((u32::from(c.0) << 16) | (u32::from(c.1) << 8) | u32::from(c.2)).into()
}

fn white(a: f32) -> Hsla {
    hsla(0.0, 0.0, 1.0, a)
}

fn black(a: f32) -> Hsla {
    hsla(0.0, 0.0, 0.0, a)
}

impl DesktopView {
    pub fn new(args: PreviewArgs, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let images = cx.new(ImageCache::new);
        cx.observe(&images, |_, _, cx| cx.notify()).detach();
        // the file operations the desktop starts report to the Status Center, whose button and flyout belong to main windows: this
        // view shows the running one and the failures in its own strip
        if let Some(ops) = cx.try_global::<crate::status_center::OpsGlobal>().map(|g| g.0.clone()) {
            cx.observe(&ops, |this, ops, cx| this.on_ops(&ops, cx)).detach();
        }
        let (tx, mut rx) = mpsc::unbounded::<Msg>();
        cx.spawn(async move |this, cx| {
            while let Some(m) = rx.next().await {
                if this.update(cx, |v, cx| v.on_msg(m, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();

        // everything that can block (listing the folders, asking the shell where the Desktop is, reading the wallpaper, asking for the
        // localised names) happens on this thread; the shell's own questions go through `sta::call`
        let settings = cx.global::<crate::settings::Settings>();
        let visibility = Visibility { show_hidden: settings.show_hidden, show_system: settings.show_system_files, dot_hidden: false };
        let config_dir = crate::settings::app_dir();
        let (worker_tx, worker_args) = (tx.clone(), args.clone());
        let spawned = std::thread::Builder::new().name("rf-desktop-load".into()).spawn(move || {
            let change_tx = parking_lot::Mutex::new(worker_tx.clone());
            let loaded = source::load(&worker_args, visibility, config_dir, move |c| {
                let _ = change_tx.lock().unbounded_send(Msg::Changed(c));
            });
            let (infos, scratch) = (loaded.backend.infos.clone(), loaded.backend.is_scratch());
            let _ = worker_tx.unbounded_send(Msg::Loaded(Box::new(loaded)));
            let wp = source::read_wallpaper(&infos);
            let _ = worker_tx.unbounded_send(Msg::Wallpaper(Box::new(wp)));
            if !scratch {
                let names = rf_win::sta::call(Duration::from_secs(5), || {
                    VirtualKind::ALL.iter().filter_map(|k| rf_win::shellfolder::display_name(&k.parsing_name()).map(|n| (*k, n))).collect::<Vec<_>>()
                })
                .unwrap_or_default();
                let _ = worker_tx.unbounded_send(Msg::VirtualNames(names));
            }
        });
        if let Err(e) = spawned {
            log::error!("desktop preview: could not start the loader: {e}");
        }

        DesktopView {
            backend: None,
            store: Store::default(),
            images,
            focus: cx.focus_handle(),
            focused_once: false,
            items: Vec::new(),
            by_id: HashMap::new(),
            resolved: Resolved::default(),
            placed: Vec::new(),
            grid: None,
            tl: None,
            virtual_names: HashMap::new(),
            map: Mapping::new(Size::new(1920, 1080), (1920.0, 1080.0), 1.0),
            sel: Selection::default(),
            typing: TypeSelect::default(),
            press: None,
            band: None,
            drag: None,
            ext_hover: None,
            renaming: None,
            want_rename: None,
            reselect: None,
            wheel: 0.0,
            wheel_ms: 0,
            notice: None,
            boot: Instant::now(),
            rename_sub: None,
            wall_src: None,
            wall: None,
            wall_asked: None,
            wall_busy: false,
            wall_gen: 0,
            wrap: HashMap::new(),
            line_counts: HashMap::new(),
            touched: false,
            ops_line: None,
            ops_failure: 0,
            save_task: None,
            load_note: None,
            tx,
        }
    }

    pub(super) fn now_ms(&self) -> u64 {
        self.boot.elapsed().as_millis() as u64
    }

    pub(super) fn read_only(&self) -> bool {
        self.backend.as_ref().is_none_or(|b| b.read_only)
    }

    pub(super) fn item(&self, id: &ItemId) -> Option<&DesktopItem> {
        self.by_id.get(id).and_then(|i| self.items.get(*i))
    }

    fn canvas_size(&self) -> Size {
        self.backend.as_ref().and_then(|b| b.specs.get(b.canvas)).map_or(Size::new(1920, 1080), |s| s.size)
    }

    pub(super) fn scale_percent(&self) -> u32 {
        self.backend.as_ref().and_then(|b| b.specs.get(b.canvas)).map_or(100, |s| s.scale_percent)
    }

    /// The name shown under an item.
    pub(super) fn shown_name(&self, it: &DesktopItem, show_ext: bool) -> String {
        match it.origin {
            Origin::Virtual(k) => self.virtual_names.get(&k).cloned().unwrap_or_else(|| it.name.clone()),
            _ => names::display_name(&it.name, it.is_dir, show_ext),
        }
    }

    // ---------- what arrives ----------

    fn on_msg(&mut self, m: Msg, cx: &mut Context<Self>) {
        match m {
            Msg::Loaded(l) => {
                let Loaded { backend, store, outcome, note } = *l;
                self.store = store;
                self.load_note = match (&outcome, note) {
                    (LoadOutcome::Recovered { .. }, _) => Some(t!("Yerleşim dosyası okunamadı; yenisi kullanılıyor").to_string()),
                    (LoadOutcome::Newer { .. } | LoadOutcome::Unreadable { .. }, _) => Some(t!("Yerleşim dosyası kaydedilmeyecek (daha yeni bir sürümden ya da okunamıyor)").to_string()),
                    (_, Some(n)) => {
                        log::warn!("desktop preview: {n}");
                        Some(t!("İstenen klasör kullanılmadı; gerçek masaüstü korumalı gösteriliyor").to_string())
                    }
                    _ => None,
                };
                self.backend = Some(Arc::new(backend));
                self.recompute(cx);
            }
            Msg::Changed(changes) => {
                for c in &changes {
                    if let ItemChange::Renamed { from, to } = c {
                        // the cell goes with the item
                        self.store.rename(from, to);
                        self.sel.renamed(from, to);
                    }
                }
                self.recompute(cx);
            }
            Msg::Wallpaper(wp) => {
                self.wall_src = Some(*wp);
                self.wall_gen += 1;
                self.wall_asked = None;
                if let Some(w) = &mut self.wall {
                    w.size = Size::default(); // the picture is asked for again at the size it is shown at
                }
                cx.notify();
            }
            Msg::VirtualNames(v) => {
                self.virtual_names.extend(v);
                self.wrap.clear();
                cx.notify();
            }
        }
    }

    /// The items, their cells and the selection, brought up to date with the model.
    pub(super) fn recompute(&mut self, cx: &mut Context<Self>) {
        let Some(b) = self.backend.clone() else { return };
        let mut items = b.items.items();
        sort_items(&mut items, self.store.sort.key.key(), self.store.sort.dir());
        let model = b.items.listing();
        // new items are placed in the order of the sort setting (a first start), the model's own order is by origin
        let shown: Vec<ItemId> = items.iter().map(|i| i.id.clone()).collect();
        let resolved = self.store.resolve(&b.specs, Listing { shown: &shown, kept: &model.kept, complete: model.complete }, now_ms());
        self.by_id = items.iter().enumerate().map(|(i, it)| (it.id.clone(), i)).collect();
        self.line_counts.retain(|id, _| self.by_id.contains_key(id));
        self.items = items;
        self.resolved = resolved;
        self.rebuild_placed();
        self.sel.retain_existing(&self.placed);
        if let Some((ids, since)) = self.reselect.take() {
            let there: BTreeSet<ItemId> = ids.iter().filter(|i| self.by_id.contains_key(*i)).cloned().collect();
            if !there.is_empty() {
                self.sel.selected = there.clone();
                self.sel.cursor = there.iter().next().cloned();
                self.sel.anchor = self.sel.cursor.clone();
            }
            if there.len() < ids.len() && since.elapsed() < Duration::from_secs(5) {
                self.reselect = Some((ids, since));
            }
        }
        self.schedule_save(cx);
        cx.notify();
    }

    /// `placed` and the grid from `resolved` (after a move as well as after a listing).
    pub(super) fn rebuild_placed(&mut self) {
        let Some(b) = self.backend.clone() else { return };
        let Some(spec) = b.specs.get(b.canvas) else {
            self.placed.clear();
            self.grid = None;
            self.tl = None;
            return;
        };
        let m = metrics(self.store.icon_size_step(), spec.scale_percent);
        let grid = Grid::new(spec.work, m);
        let tl = TileLayout::new(m, spec.scale_percent);
        let mut placed: Vec<Placed> = self
            .items
            .iter()
            .filter_map(|it| {
                let p = self.resolved.placed.get(&it.id).filter(|p| p.monitor == b.canvas)?;
                let r = grid.cell_rect(p.cell).offset(p.dx, p.dy);
                Some(Placed { id: it.id.clone(), cell: p.cell, hit: tl.hit(r.left, r.top) })
            })
            .collect();
        placed.sort_by_key(|p| (p.cell.col, p.cell.row));
        self.placed = placed;
        self.grid = Some(grid);
        self.tl = Some(tl);
    }

    /// The cell's rectangle with the free offset of a not-aligned item, in canvas pixels.
    pub(super) fn cell_rect_of(&self, id: &ItemId) -> Option<Rect> {
        let g = self.grid.as_ref()?;
        let p = self.resolved.placed.get(id)?;
        Some(g.cell_rect(p.cell).offset(p.dx, p.dy))
    }

    /// The user arranged something (moved an icon, changed the size, the sort or the alignment): from now on the layout is worth
    /// keeping. Looking at the real Desktop writes nothing, not even our own layout file (see `source::keeps_layout`).
    pub(super) fn touch_layout(&mut self) {
        self.touched = true;
        let scratch = self.backend.as_ref().is_some_and(|b| b.is_scratch());
        source::arranged_by_user(&mut self.store, scratch);
    }

    fn keeps_layout(&self) -> bool {
        source::keeps_layout(self.backend.as_ref().is_some_and(|b| b.is_scratch()), self.touched)
    }

    /// Saves the layout file half a second after the last change (one timer at a time, none at rest).
    pub(super) fn schedule_save(&mut self, cx: &mut Context<Self>) {
        if !self.keeps_layout() || !self.store.is_dirty() || self.save_task.is_some() {
            return;
        }
        self.save_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = this.update(cx, |v, _| {
                v.save_task = None;
                v.save_now();
            });
        }));
    }

    pub(super) fn save_now(&mut self) {
        if !self.keeps_layout() {
            return;
        }
        let Some(b) = &self.backend else { return };
        if let Err(e) = self.store.save_if_dirty(&b.layout_path) {
            log::warn!("desktop layout not saved: {e}");
        }
    }

    // ---------- wallpaper ----------

    /// The wallpaper is wanted at `want` pixels. ONE picture is made at a time: decoding a large picture takes a hundred megabytes and
    /// seconds of a core, so a window that is dragged to a new size must not start another while the last one is still running. A
    /// request that comes in meanwhile is only noted; when the running one ends, the newest is started.
    fn want_wallpaper(&mut self, want: Size, cx: &mut Context<Self>) {
        if self.wall_src.is_none() || self.backend.is_none() {
            return;
        }
        if want.is_empty() || self.wall.as_ref().is_some_and(|w| w.size == want) || self.wall_asked == Some(want) {
            return;
        }
        self.wall_asked = Some(want);
        if !self.wall_busy {
            self.start_wallpaper(cx);
        }
    }

    fn start_wallpaper(&mut self, cx: &mut Context<Self>) {
        let (Some(wp), Some(b)) = (self.wall_src.clone(), self.backend.clone()) else { return };
        let first = self.wall.is_none();
        let gen_now = self.wall_gen;
        let cache_dir = crate::settings::local_data_dir().map(|d| d.join("wallcache-preview"));
        self.wall_busy = true;
        cx.spawn(async move |this, cx| {
            if !first {
                // a window being dragged asks for a new size on every frame: nothing is made until the size has held still
                loop {
                    let Ok(seen) = this.update(cx, |v, _| v.wall_asked) else { return };
                    cx.background_executor().timer(Duration::from_millis(150)).await;
                    match this.update(cx, |v, _| v.wall_asked) {
                        Ok(now) if now == seen => break,
                        Ok(_) => {}
                        Err(_) => return,
                    }
                }
            }
            let Ok(Some(want)) = this.update(cx, |v, _| v.wall_asked) else {
                let _ = this.update(cx, |v, _| v.wall_busy = false);
                return;
            };
            let canvas = b.canvas;
            let rendered = cx.background_spawn(async move { wall::render(&wp, canvas, want, cache_dir) }).await;
            let _ = this.update(cx, |v, cx| {
                v.wall_busy = false;
                if v.wall_gen == gen_now && v.wall_asked == Some(want) {
                    if let Some(note) = &rendered.note {
                        log::info!("desktop wallpaper: {note}");
                    }
                    if let Some(image) = to_render_image(&rendered.bitmap)
                        && let Some(old) = v.wall.replace(WallImage { image, size: want })
                    {
                        // the GPU atlas keeps an uploaded image until it is told to let go
                        cx.defer(move |cx| cx.drop_image(old.image, None));
                    }
                } else if v.wall_asked.is_some_and(|a| a != want && v.wall.as_ref().is_none_or(|w| w.size != a)) || v.wall_gen != gen_now {
                    // the size (or the setting) changed while this one was being made: the newest is wanted now
                    v.start_wallpaper(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    // ---------- file operations ----------

    /// The Status Center changed: show the newest running operation, and a failure (once) as a notice.
    fn on_ops(&mut self, ops: &Entity<crate::status_center::Ops>, cx: &mut Context<Self>) {
        let g = ops.read(cx).glance();
        self.ops_line = g.running.map(|(title, pct)| match pct {
            Some(p) => format!("{title} %{p}"),
            None => title,
        });
        if let Some((id, why)) = g.failed
            && id > self.ops_failure
        {
            self.ops_failure = id;
            self.notice = Some(format!("{}: {why}", t!("İşlem başarısız oldu")));
        }
        cx.notify();
    }

    // ---------- text ----------

    fn wrap_label(&mut self, name: &str, width: f32, font: f32, window: &Window) -> Lines {
        let key = WrapKey { name: name.to_string(), width: (width * 4.0) as u32, font: (font * 4.0) as u32 };
        if let Some(l) = self.wrap.get(&key) {
            return l.clone();
        }
        if self.wrap.len() > 4000 {
            self.wrap.clear();
        }
        let ts = window.text_system().clone();
        let style_font = window.text_style().font();
        let measure = move |s: &str| -> f32 {
            if s.is_empty() {
                return 0.0;
            }
            let run = TextRun { len: s.len(), font: style_font.clone(), color: black(1.0), background_color: None, underline: None, strikethrough: None };
            f32::from(ts.layout_line(s, px(font), &[run], None).width)
        };
        let lines = label::wrap(name, width, &measure);
        self.wrap.insert(key, lines.clone());
        lines
    }

    // ---------- drawing ----------

    /// One line of a label with the soft shadow that keeps it readable on any wallpaper: the text, and under it a few copies of it
    /// in translucent black, a pixel to each side and below (GPUI has no text shadow of its own).
    fn label_line(text: &str, w: f32, h: f32, font: f32, shadow: bool, scale: f32) -> Div {
        let one = |text: String| div().w(px(w)).h(px(h)).flex().justify_center().whitespace_nowrap().text_size(px(font)).line_height(px(h)).child(text);
        let mut line = div().relative().w(px(w)).h(px(h));
        if shadow {
            let d = (scale).max(0.6);
            for (dx, dy, a) in [(0.0, 1.0, 0.55), (1.0, 1.0, 0.40), (-1.0, 1.0, 0.40), (0.0, 2.0, 0.25)] {
                line = line.child(one(text.to_string()).absolute().left(px(dx * d)).top(px(dy * d)).text_color(black(a)));
            }
        }
        line.child(one(text.to_string()).absolute().left(px(0.)).top(px(0.)).text_color(white(0.97)))
    }

    fn icon_for(&mut self, it: &DesktopItem, frame: &Frame, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let b = self.backend.clone()?;
        match it.origin {
            Origin::Virtual(k) => self.images.update(cx, |c, cx| c.shell_icon(&k.parsing_name(), frame.icon_px, cx)),
            o => {
                let dir = b.dir_of(o)?;
                let e = Entry::new(&it.name, if it.is_dir { EntryKind::Dir } else { EntryKind::File }, it.size, it.modified_ms, 0, it.attrs);
                let (thumbs, placeholder, px) = (frame.thumbs, it.cloud_placeholder, frame.icon_px);
                // an online-only file is shown by its icon, never read for a thumbnail (that would download it)
                self.images.update(cx, |c, cx| if thumbs && !placeholder { c.thumbnail(&dir, &e, px, cx) } else { c.icon(&dir, &e, cx) })
            }
        }
    }

    fn fallback_icon(it: &DesktopItem, size: f32, dark: bool) -> AnyElement {
        match it.origin {
            Origin::Virtual(VirtualKind::RecycleBin) => Icon::new(L::Trash).size(px(size)).text_color(white(0.9)).into_any_element(),
            Origin::Virtual(VirtualKind::ThisPc) => Icon::new(L::Monitor).size(px(size)).text_color(white(0.9)).into_any_element(),
            Origin::Virtual(_) => Icon::new(L::Folder).size(px(size)).text_color(white(0.9)).into_any_element(),
            _ => {
                let e = Entry::new(&it.name, if it.is_dir { EntryKind::Dir } else { EntryKind::File }, it.size, it.modified_ms, 0, it.attrs);
                let cat = icons::category(&e);
                Icon::new(icons::icon_for(cat)).size(px(size * 0.7)).text_color(theme::kind_color(cat, dark)).into_any_element()
            }
        }
    }

    /// One tile. `shift`: canvas pixels to move it by (a dragged ghost).
    #[allow(clippy::too_many_arguments)]
    fn tile(&mut self, ix: usize, it: &DesktopItem, cell: Rect, shift: (i32, i32), look: Look, drop_target: bool, frame: &Frame, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let ghost = look == Look::Ghost;
        let tl = frame.tl;
        let p = &frame.p;
        let u = self.map.unit();
        let selected = self.sel.is_selected(&it.id) && !ghost;
        let renaming = self.renaming.as_ref().filter(|r| r.id == it.id).map(|r| r.input.clone());
        let name = self.shown_name(it, frame.show_ext);
        let text_w = self.map.len(tl.text_w() as f32);
        let lines = if renaming.is_some() { None } else { Some(self.wrap_label(&name, text_w, frame.font, window)) };
        let nlines = if renaming.is_some() { 2 } else { lines.as_ref().map_or(1, Lines::count) };
        self.line_counts.insert(it.id.clone(), nlines);
        let r = tl.rect(cell.left + shift.0, cell.top + shift.1, nlines);
        let (x, y, w, h) = self.map.rect(r);
        let (icon_x, icon_y) = tl.icon_at();
        let icon_l = self.map.len(tl.icon as f32);

        let image = self.icon_for(it, frame, cx);
        let visual: AnyElement = match image {
            Some(im) => img(im).size(px(icon_l)).object_fit(ObjectFit::Contain).into_any_element(),
            None => Self::fallback_icon(it, icon_l, p.dark),
        };
        let label_top = self.map.len((tl.pad + tl.icon + tl.pad / 2) as f32);
        let label: AnyElement = match (&renaming, &lines) {
            (Some(inp), _) => div()
                .absolute()
                .left(px(self.map.len(tl.text_pad as f32 / 2.0)))
                .top(px(label_top - 2.0))
                .w(px(w - self.map.len(tl.text_pad as f32)))
                .h(px(24.))
                .rounded(px(4.))
                .bg(p.card_bg)
                .border_1()
                .border_color(p.accent)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(gpui_kit::component::input::Input::new(inp).appearance(false).small().px_1().text_xs().text_center())
                .into_any_element(),
            (None, Some(l)) => {
                let lh = self.map.len(tl.line_h as f32);
                let mut col = div().absolute().left(px(self.map.len(tl.text_pad as f32))).top(px(label_top)).w(px(text_w)).flex().flex_col();
                col = col.child(Self::label_line(&l.first, text_w, lh, frame.font, frame.shadow, u));
                if let Some(second) = &l.second {
                    col = col.child(Self::label_line(second, text_w, lh, frame.font, frame.shadow, u));
                }
                col.into_any_element()
            }
            _ => div().into_any_element(),
        };

        let id = it.id.clone();
        let radius = px(self.map.len(6.0 * self.scale_percent() as f32 / 100.0).max(3.0));
        let sel_bg = Hsla { a: 0.34, ..p.accent };
        let mut el = div()
            .id(("dtile", ix + if ghost { 100_000 } else { 0 }))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .rounded(radius)
            .border_1()
            .border_color(transparent_black())
            .when(it.dimmed || look != Look::Normal, |d| d.opacity(match look { Look::Ghost => 0.6, Look::Faded => 0.35, Look::Normal => 0.6 }))
            .when(selected, |d| d.bg(sel_bg).border_color(p.selection_border).child(theme::selection_pill(p)))
            .when(drop_target, |d| d.bg(Hsla { a: 0.45, ..p.accent }).border_color(p.accent))
            .when(!selected && !ghost && !drop_target, |d| d.hover(|s| s.bg(white(0.10))))
            .child(div().absolute().left(px(self.map.len(icon_x as f32))).top(px(self.map.len(icon_y as f32))).size(px(icon_l)).flex().items_center().justify_center().child(visual))
            .child(label);
        if !ghost {
            let accepts = self.accepts_of_item(it);
            let takes = self.takes_files(it, accepts);
            let (id1, id2) = (id.clone(), id.clone());
            el = el
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, ev: &MouseDownEvent, window, cx| this.on_tile_down(&id1, ev, window, cx)))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, _: &MouseDownEvent, window, cx| this.on_tile_right(&id2, window, cx)));
            // (a folder that would refuse the files is not lit as a target: the drop falls through to the desktop's own decision, which
            // says why)
            if takes {
                let (acc, id3) = (p.accent, id.clone());
                el = el
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(Hsla { a: 0.45, ..acc }).border_color(acc))
                    .on_drop(cx.listener(move |this, paths: &ExternalPaths, window, cx| this.on_external_drop_on_tile(&id3, paths, window, cx)));
            }
        }
        el.into_any_element()
    }

    /// Whether files dropped on this item would be taken by it (so that it is lit while they are over it).
    fn takes_files(&self, it: &DesktopItem, accepts: Accepts) -> bool {
        match accepts {
            Accepts::Nothing => false,
            Accepts::Program => true,
            Accepts::Folder | Accepts::RecycleBin => !self.read_only() && it.origin != Origin::Public,
        }
    }

    /// The click areas follow the number of lines each label has now (a two-line label's tile is taller than a one-line one's).
    fn refresh_hits(&mut self) {
        let Some(tl) = self.tl else { return };
        for i in 0..self.placed.len() {
            let id = self.placed[i].id.clone();
            if let (Some(c), Some(n)) = (self.cell_rect_of(&id), self.line_counts.get(&id).copied()) {
                self.placed[i].hit = tl.hit_for(c.left, c.top, n);
            }
        }
    }

    pub(super) fn accepts_of_item(&self, it: &DesktopItem) -> Accepts {
        let program = !it.is_dir && drop::is_program_name(&it.name);
        drop::accepts_of(it.is_dir, program, matches!(it.origin, Origin::Virtual(VirtualKind::RecycleBin)), it.is_virtual())
    }

    /// The items that are on another display than the one drawn (they are not in the window and not in its count).
    fn on_other_display(&self) -> usize {
        let Some(b) = &self.backend else { return 0 };
        self.items.iter().filter(|it| self.resolved.placed.get(&it.id).is_some_and(|p| p.monitor != b.canvas)).count()
    }

    fn status(&self, p: &Palette, scaled: Option<u32>) -> Div {
        // (every piece may be cut with an ellipsis: on a narrow window the strip must not run over its edge)
        let piece = |text: String, color: Hsla| div().min_w_0().truncate().text_color(color).child(text);
        let mut bar = div().w_full().h(px(STATUS_H)).flex_shrink_0().px_3().flex().items_center().gap_4().bg(p.card_bg).border_t_1().border_color(p.divider).text_xs().text_color(p.text_secondary);
        if self.backend.is_none() {
            return bar.child(piece(t!("Yükleniyor…").to_string(), p.text_secondary));
        }
        if self.read_only() {
            bar = bar.child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(p.accent).child(t!("Salt okunur önizleme")));
        } else if self.backend.as_ref().is_some_and(|b| b.is_scratch()) {
            bar = bar.child(div().flex_shrink_0().text_color(p.accent).child(t!("Deneme klasörü masaüstü yerine geçiyor")));
        }
        if let Some(n) = &self.notice {
            bar = bar.child(piece(n.clone(), p.danger));
        }
        if let Some(l) = &self.ops_line {
            bar = bar.child(piece(l.clone(), p.accent));
        }
        if let Some(n) = &self.load_note {
            bar = bar.child(piece(n.clone(), p.text_secondary));
        }
        if let Some(pct) = scaled {
            bar = bar.child(piece(t!("Masaüstü pencereye sığsın diye %{n} ölçeğinde gösteriliyor").replace("{n}", &pct.to_string()), p.text_secondary));
        }
        if self.wall_src.as_ref().is_some_and(|w| w.position == Position::Span) {
            bar = bar.child(piece(t!("Yayılmış duvar kâğıdı: önizleme resmin tamamını bu tek ekrana sığdırır").to_string(), p.text_secondary));
        }
        let other = self.on_other_display();
        if other > 0 {
            bar = bar.child(piece(t!("Diğer ekranda {n} öğe var").replace("{n}", &other.to_string()), p.text_secondary));
        }
        let n = self.sel.selected.len();
        bar.child(div().flex_1()).child(div().flex_shrink_0().child(if n > 0 { crate::i18n::items_selected(n) } else { crate::i18n::items(self.placed.len()) }))
    }
}

impl Render for DesktopView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        if !std::mem::replace(&mut self.focused_once, true) {
            window.focus(&self.focus, cx);
        }
        // the item being renamed is gone (deleted, or renamed by another program): its box is not in the frame any more, so GPUI has
        // taken the keyboard away from it, and nothing would work until a click
        if self.renaming.as_ref().is_some_and(|r| !self.by_id.contains_key(&r.id)) {
            self.cancel_rename(window, cx);
        }
        // a new folder or file was asked for: its rename box opens as soon as the model shows it (this needs a window)
        if let Some((id, since)) = self.want_rename.clone() {
            if since.elapsed() > Duration::from_secs(5) {
                self.want_rename = None;
            } else if self.by_id.contains_key(&id) {
                self.want_rename = None;
                self.sel.only(&id);
                self.begin_rename_of(&id, window, cx);
            }
        }

        let vp = window.viewport_size();
        let (vw, vh) = (f32::from(vp.width), (f32::from(vp.height) - STATUS_H).max(1.0));
        let canvas_px = self.canvas_size();
        self.map = Mapping::new(canvas_px, (vw, vh), window.scale_factor());
        let (sw, sh) = self.map.shown_size();
        // the wallpaper is made for the physical size it is shown at
        let want = Size::new((sw * window.scale_factor()).round().max(1.0) as u32, (sh * window.scale_factor()).round().max(1.0) as u32);
        self.want_wallpaper(want, cx);

        let settings = cx.global::<crate::settings::Settings>();
        let (show_ext, thumbs) = (settings.show_extensions, settings.show_thumbnails);
        let frame = match (self.tl, self.backend.as_ref()) {
            (Some(tl), Some(b)) => Some(Frame { p, tl, show_ext, thumbs, shadow: b.label_shadow, font: self.map.len(tl.font as f32), icon_px: tl.icon }),
            _ => None,
        };

        // the wallpaper, or the background colour until it is there
        let bg = self.wall_src.as_ref().map_or(Rgb(32, 32, 32), |w| w.background);
        let wallpaper: AnyElement = match &self.wall {
            Some(w) => img(w.image.clone()).absolute().left(px(0.)).top(px(0.)).w(px(sw)).h(px(sh)).object_fit(ObjectFit::Fill).into_any_element(),
            None => div().absolute().left(px(0.)).top(px(0.)).w(px(sw)).h(px(sh)).bg(rgb_of(bg)).into_any_element(),
        };

        let mut tiles: Vec<AnyElement> = Vec::new();
        let mut ghosts: Vec<AnyElement> = Vec::new();
        let mut outline: Option<AnyElement> = None;
        if let Some(frame) = &frame {
            let dragging: BTreeSet<ItemId> = self.drag.as_ref().map(|d| d.ids.iter().cloned().collect()).unwrap_or_default();
            // (an item is lit only when dropping on it would hand it the files; files dragged in from other programs light the tile they
            // are over through GPUI's own drag-over style, which clears itself when they leave)
            let ro = self.read_only();
            let hot: Option<ItemId> = self.drag.as_ref().and_then(|d| d.over.as_ref().filter(|o| drop::lights_up(&d.ids, o, d.files, ro)).map(|o| o.id.clone()));
            let order: Vec<(usize, DesktopItem)> = self.placed.iter().filter_map(|pl| self.by_id.get(&pl.id).map(|i| (*i, self.items[*i].clone()))).collect();
            for (ix, it) in &order {
                let Some(cell) = self.cell_rect_of(&it.id) else { continue };
                let at_rest = !dragging.contains(&it.id);
                let look = if at_rest { Look::Normal } else { Look::Faded };
                tiles.push(self.tile(*ix, it, cell, (0, 0), look, hot.as_ref() == Some(&it.id), frame, window, cx));
                if let (false, Some(d)) = (at_rest, self.drag.as_ref()) {
                    let delta = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
                    ghosts.push(self.tile(*ix, it, cell, delta, Look::Ghost, false, frame, window, cx));
                }
            }
            // where the grabbed icon would land, outlined, unless the pointer is over something that takes the files
            if let (Some(d), Some(g)) = (self.drag.as_ref(), self.grid.as_ref())
                && let Some(cell) = self.cell_rect_of(&d.grabbed)
                && matches!(drop::decide_icons(&d.ids, &Where { over: d.over.as_ref(), cell: Some(Cell::new(0, 0)) }, d.files, ro), DropAction::MoveIcons { .. })
                && self.map.contains(d.cur)
            {
                let delta = (d.cur.0 - d.start.0, d.cur.1 - d.start.1);
                let (cell_c, _, _) = g.snap(cell.left + delta.0, cell.top + delta.1, self.store.align_to_grid).unwrap_or((Cell::new(0, 0), 0, 0));
                let target = if self.store.align_to_grid { g.cell_rect(cell_c) } else { Rect::from_size(cell.left + delta.0, cell.top + delta.1, g.metrics.pitch_x, g.metrics.pitch_y) };
                let (x, y, w, h) = self.map.rect(target);
                outline = Some(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).rounded(px(6.)).border_1().border_color(Hsla { a: 0.8, ..frame.p.accent }).bg(Hsla { a: 0.10, ..frame.p.accent }).into_any_element());
            }
        }

        self.refresh_hits();

        // the rubber band
        let band: Option<AnyElement> = self.band.as_ref().and_then(|b| {
            let r = rect_of(b.start, b.cur);
            if r.width() < 3 && r.height() < 3 {
                return None;
            }
            let (x, y, w, h) = self.map.rect(r);
            Some(div().absolute().left(px(x)).top(px(y)).w(px(w)).h(px(h)).bg(Hsla { a: 0.22, ..p.accent }).border_1().border_color(p.selection_border).into_any_element())
        });

        // window-level mouse listeners: a drag or a rubber band goes on while the pointer is outside the window (the window holds the
        // mouse from the press), which element listeners would not hear
        let this = cx.entity();
        let events = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        t.update(cx, |v, cx| v.on_pointer_move(ev, window, cx));
                    }
                });
                let t = this.clone();
                window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && ev.button == MouseButton::Left {
                        t.update(cx, |v, cx| v.on_pointer_up(ev, window, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        // while the desktop is being read, and when there is nothing on it, the canvas says so
        let note_over_canvas: Option<AnyElement> = match (&self.backend, self.items.is_empty()) {
            (None, _) => Some(t!("Yükleniyor…")),
            (Some(_), true) if self.load_note.is_none() => Some(t!("Bu masaüstünde öğe yok")),
            _ => None,
        }
        .map(|text| div().absolute().left(px(0.)).top(px(0.)).w(px(sw)).h(px(sh)).flex().items_center().justify_center().text_color(white(0.85)).child(text).into_any_element());
        let scaled = self.map.scaled_down().then(|| self.map.percent());
        let area = div()
            .id("desktop-canvas")
            .relative()
            .w_full()
            .h(px(vh))
            .flex_shrink_0()
            .overflow_hidden()
            .bg(p.card_bg)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, window, cx| this.on_background_down(ev, window, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &MouseDownEvent, window, cx| this.on_background_right(window, cx)))
            .on_scroll(cx.listener(|this, ev: &ScrollEvent, _, cx| this.on_wheel(ev, cx)))
            .on_drag_move::<ExternalPaths>(cx.listener(|this, ev: &DragMoveEvent<ExternalPaths>, _, cx| this.on_external_move(ev, cx)))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| this.on_external_drop_background(paths, window, cx)))
            .child(events)
            .child(wallpaper)
            .children(tiles)
            .children(outline)
            .children(ghosts)
            .children(band)
            .children(note_over_canvas);

        let root = div()
            .id("desktop-view")
            .key_context(KeyContext::parse(super::CTX).unwrap_or_default())
            .track_focus(&self.focus)
            .size_full()
            .flex()
            .flex_col()
            .text_color(p.text);
        self.bind_actions(root, cx).child(area).child(self.status(&p, scaled))
    }
}

impl Drop for DesktopView {
    fn drop(&mut self) {
        // the last half second of arranging is not lost by closing the window
        self.save_now();
    }
}
