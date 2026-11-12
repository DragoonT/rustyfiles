//! The app's own context menu, drawn like Files / Windows 11 (Fluent): an optional row of
//! quick icon buttons on top (cut, copy, paste…), then icon + label + shortcut rows with
//! cascading submenus. The shell's own items arrive later into a lazy submenu ("Show more
//! options"), so the menu itself appears instantly.
//!
//! One menu is open at a time (a global); each window's workspace paints it when it is
//! for that window.
//!
//! Opened from a window that shows glass, the menu is the liquid look (`liquid`): every level is a popup on real glass
//! (`glass_watch::PopupGlass`, a plain fill until it runs), rows are 34 high, a ticked row has its tick at the leading edge and the
//! highlight is one capsule that glides from row to row.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::glass_watch::{self, PopupGlass};
use crate::liquid::{self, Glide, Metrics};
use crate::theme::{self, Palette};

pub type Run = Rc<dyn Fn(&mut Window, &mut App)>;

#[derive(Clone)]
pub enum MenuIcon {
    None,
    Lucide(L),
    Tinted(L, Hsla),
    Image(Arc<RenderImage>),
    /// A colored dot (tags).
    Dot(Hsla),
}

#[derive(Clone)]
pub enum Sub {
    Items(Rc<Vec<Item>>),
    /// Filled in later through [`MenuView::set_pending`] under `key` (shell items); until
    /// then `fallback` is shown, or a "Loading…" row.
    Pending { key: &'static str, fallback: Option<Rc<Vec<Item>>> },
}

#[derive(Clone)]
pub enum Item {
    Action {
        label: SharedString,
        icon: MenuIcon,
        hint: Option<SharedString>,
        enabled: bool,
        checked: bool,
        run: Run,
    },
    Sub {
        label: SharedString,
        icon: MenuIcon,
        items: Sub,
    },
    Separator,
}

impl Item {
    pub fn action(label: impl Into<SharedString>, icon: MenuIcon, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Item::Action { label: label.into(), icon, hint: None, enabled: true, checked: false, run: Rc::new(run) }
    }
    pub fn sub(label: impl Into<SharedString>, icon: MenuIcon, items: Vec<Item>) -> Self {
        Item::Sub { label: label.into(), icon, items: Sub::Items(Rc::new(items)) }
    }
    pub fn hint(mut self, h: impl Into<SharedString>) -> Self {
        if let Item::Action { hint, .. } = &mut self {
            *hint = Some(h.into());
        }
        self
    }
    pub fn enabled(mut self, on: bool) -> Self {
        if let Item::Action { enabled, .. } = &mut self {
            *enabled = on;
        }
        self
    }
    pub fn checked(mut self, on: bool) -> Self {
        if let Item::Action { checked, .. } = &mut self {
            *checked = on;
        }
        self
    }
    fn height(&self, m: &Metrics) -> f32 {
        match self {
            Item::Separator => m.sep_h,
            _ => m.row_h,
        }
    }
    fn selectable(&self) -> bool {
        match self {
            Item::Separator => false,
            Item::Action { enabled, .. } => *enabled,
            Item::Sub { .. } => true,
        }
    }
}

pub struct Quick {
    pub icon: L,
    pub tip: SharedString,
    pub enabled: bool,
    pub run: Run,
}

impl Quick {
    pub fn new(icon: L, tip: impl Into<SharedString>, enabled: bool, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Self { icon, tip: tip.into(), enabled, run: Rc::new(run) }
    }
}

#[derive(Default)]
pub struct Menu {
    pub quick: Vec<Quick>,
    pub items: Vec<Item>,
}

/// State of the lazy submenu.
pub enum Pending {
    Ready(Rc<Vec<Item>>),
    Failed,
}

const W: f32 = 300.;
const QUICK_H: f32 = 45.;
const EDGE: f32 = 8.;
const HOVER_DELAY: Duration = Duration::from_millis(220);

pub struct MenuView {
    quick: Vec<Quick>,
    root: Rc<Vec<Item>>,
    /// Bumped by `set_root` (the root popup must be rebuilt).
    root_gen: u64,
    pos: Point<Pixels>,
    /// `open[d]` = index of the open submenu row in level `d`.
    open: Vec<usize>,
    /// Keyboard highlight: (level, index).
    hl: Option<(usize, usize)>,
    pending: std::collections::HashMap<&'static str, Pending>,
    hover_gen: u64,
    focus: FocusHandle,
    /// Set when the levels live in their own Acrylic popup windows.
    popup: Option<PopupState>,
    /// The menu was opened from a window that shows glass (or from a popup that does): the liquid look, whose rows have other
    /// measures, so the layout reads it too.
    liquid: bool,
    /// The highlight capsule of each level (keyed by the path of open submenus that leads to it), drawn at every paint of that level.
    glides: RefCell<BTreeMap<Vec<usize>, Glide>>,
}

/// Popup mode: each level is a borderless Acrylic window (the material of Windows 11 menus),
/// positioned in screen space so menus are not clipped by the app window.
struct PopupState {
    /// Where the menu was opened, in screen (logical) coordinates.
    anchor: Point<Pixels>,
    /// The display the menu must stay on.
    area: Bounds<Pixels>,
    /// Open level windows with the layout key they were built for (shared with `Host`, so
    /// closing never has to read the view, which may be the one being updated).
    wins: Wins,
}

/// Identifies what a level window shows; a different key means the window is rebuilt.
#[derive(Clone, PartialEq)]
struct LevelKey {
    root_gen: u64,
    path: Vec<usize>,
    bounds: Bounds<Pixels>,
}

type Wins = Rc<std::cell::RefCell<Vec<(AnyWindowHandle, LevelKey)>>>;

#[derive(Clone)]
struct HostState {
    main: AnyWindowHandle,
    view: Entity<MenuView>,
    /// Focus to give back on close — kept here, not in the view, so closing from inside the
    /// menu's own handlers never has to read the view being updated.
    restore: Option<FocusHandle>,
    popup: bool,
    wins: Wins,
}

struct Host(Option<HostState>);
impl Global for Host {}

fn host(cx: &App) -> Option<HostState> {
    cx.try_global::<Host>().and_then(|h| h.0.clone())
}

/// Opens `menu` at `pos` (window coordinates), replacing any open menu.
pub fn show(menu: Menu, pos: Point<Pixels>, window: &mut Window, cx: &mut App) -> Entity<MenuView> {
    if host(cx).is_some() {
        close(window, cx);
    }
    let restore = window.focused(cx);
    let main = window.window_handle();
    let view = cx.new(|cx| MenuView {
        quick: menu.quick,
        root: Rc::new(menu.items),
        root_gen: 0,
        pos,
        open: Vec::new(),
        hl: None,
        pending: Default::default(),
        hover_gen: 0,
        focus: cx.focus_handle(),
        popup: None,
        liquid: crate::glass::liquid(crate::look::window::hwnd_of(window)),
        glides: RefCell::new(BTreeMap::new()),
    });

    // Acrylic popups (Windows 11); RF_INWINDOW_MENU=1 or a failure falls back to drawing the
    // menu inside the window.
    if std::env::var_os("RF_INWINDOW_MENU").is_none() {
        let anchor = window.bounds().origin + pos;
        let area = window.display(cx).map(|d| d.bounds()).unwrap_or_else(|| window.bounds());
        let wins: Wins = Default::default();
        view.update(cx, |m, _| m.popup = Some(PopupState { anchor, area, wins: wins.clone() }));
        cx.set_global(Host(Some(HostState { main, view: view.clone(), restore: restore.clone(), popup: true, wins })));
        gpui_windows::KEEP_ACTIVE_LOOK.store(true, std::sync::atomic::Ordering::Relaxed);
        if sync_popups(&view, cx) {
            // follow level changes (submenus opening, shell items arriving)
            cx.observe(&view, |view, cx| {
                if host(cx).is_some_and(|h| h.view == view) {
                    sync_popups(&view, cx);
                }
            })
            .detach();
            return view;
        }
        log::warn!("menu: popup window failed, drawing in-window");
        gpui_windows::KEEP_ACTIVE_LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
        view.update(cx, |m, _| m.popup = None);
    }
    window.focus(&view.read(cx).focus.clone(), cx);
    cx.set_global(Host(Some(HostState { main, view: view.clone(), restore, popup: false, wins: Default::default() })));
    window.refresh();
    view
}

/// Whether any menu is open (toolbar buttons hide their tooltips meanwhile).
pub fn is_open(cx: &App) -> bool {
    host(cx).is_some()
}

/// The open in-window menu, if it belongs to `window` (the workspace paints it as its top layer).
pub fn current(window: &Window, cx: &App) -> Option<Entity<MenuView>> {
    let h = host(cx)?;
    (!h.popup && h.main == window.window_handle()).then_some(h.view)
}

/// Closes the open menu (if any) and gives focus back. `window` is the window whose handler
/// asks (the app window, or one of the menu's own popups).
pub fn close(window: &mut Window, cx: &mut App) {
    let Some(h) = host(cx) else { return };
    cx.set_global(Host(None));
    if !h.popup {
        if let Some(f) = h.restore {
            window.focus(&f, cx);
        }
        window.refresh();
        return;
    }
    gpui_windows::KEEP_ACTIVE_LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
    let wins: Vec<AnyWindowHandle> = h.wins.borrow().iter().map(|w| w.0).collect();
    // after the current update: a window can't be removed while it is the one being updated
    cx.defer(move |cx| {
        for w in wins {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
        let _ = h.main.update(cx, |_, window, cx| {
            window.activate_window();
            if let Some(f) = &h.restore {
                window.focus(f, cx);
            }
            window.refresh();
        });
    });
}

/// Runs a menu command in the app window (never in a popup), after the menu is gone.
fn run_in_main(run: Run, window: &mut Window, cx: &mut App) {
    let main = host(cx).filter(|h| h.popup).map(|h| h.main);
    close(window, cx);
    match main {
        Some(main) => cx.defer(move |cx| {
            let _ = main.update(cx, |_, window, cx| run(window, cx));
        }),
        None => run(window, cx),
    }
}

/// Opens / rebuilds / closes level windows to match the menu state. False if the root popup
/// could not be created.
fn sync_popups(view: &Entity<MenuView>, cx: &mut App) -> bool {
    let (wanted, have, dark, liquid) = {
        let m = view.read(cx);
        let Some(pp) = &m.popup else { return false };
        let rects = m.layout(pp.area, pp.anchor);
        let wanted: Vec<LevelKey> = rects
            .iter()
            .enumerate()
            .map(|(lv, r)| LevelKey { root_gen: m.root_gen, path: m.open[..lv].to_vec(), bounds: *r })
            .collect();
        let have: Vec<(AnyWindowHandle, LevelKey)> = pp.wins.borrow().clone();
        (wanted, have, matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark), m.liquid)
    };
    let dark = match crate::settings::theme_override() {
        crate::settings::ThemePref::Light => false,
        crate::settings::ThemePref::Dark => true,
        crate::settings::ThemePref::System => dark,
    };
    let keep = have.iter().zip(&wanted).take_while(|(h, w)| h.1 == **w).count();
    let mut wins: Vec<(AnyWindowHandle, LevelKey)> = have[..keep].to_vec();
    // open the new ones first (so focus moves to a menu window, never to nothing), then drop
    // the stale ones
    for (lv, key) in wanted.iter().enumerate().skip(keep) {
        match open_level(view.clone(), lv, key.bounds, lv == 0, dark, liquid, cx) {
            Some(h) => wins.push((h, key.clone())),
            None if lv == 0 => return false,
            None => break,
        }
    }
    let stale: Vec<AnyWindowHandle> = have[keep..].iter().map(|h| h.0).collect();
    if let Some(pp) = &view.read(cx).popup {
        *pp.wins.borrow_mut() = wins;
    }
    if !stale.is_empty() {
        cx.defer(move |cx| {
            for w in stale {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
        });
    }
    true
}

fn open_level(view: Entity<MenuView>, level: usize, bounds: Bounds<Pixels>, focus: bool, dark: bool, liquid: bool, cx: &mut App) -> Option<AnyWindowHandle> {
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // over glass the window is see-through and the glass engine draws behind it (the view paints its own fill until it does)
        window_background: if liquid { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Blurred },
        ..Default::default()
    };
    let r = cx.open_window(opts, move |w, cx| {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = w.window_handle() {
            if let RawWindowHandle::Win32(h) = h.as_raw() {
                rf_win::window::set_dark_mode(h.hwnd.get(), dark);
                crate::glass::keep_out_of_capture(h.hwnd.get());
            }
        }
        cx.new(|cx| LevelView::new(view, level, liquid.then_some(dark), w, cx))
    });
    match r {
        Ok(h) => Some(h.into()),
        Err(e) => {
            log::warn!("menu popup: {e:#}");
            None
        }
    }
}

/// One level of a popup menu, filling its own Acrylic window.
struct LevelView {
    menu: Entity<MenuView>,
    level: usize,
    focus: FocusHandle,
    /// The glass behind this level's window, in the liquid look.
    glass: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl LevelView {
    /// `liquid`: `Some(dark)` to sit on glass, `None` for the Acrylic window.
    fn new(menu: Entity<MenuView>, level: usize, liquid: Option<bool>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&menu, |_, _, cx| cx.notify()),
            // clicking anywhere outside the menu's windows closes it
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    return;
                }
                let menu = this.menu.clone();
                cx.defer(move |cx| {
                    let Some(h) = host(cx).filter(|h| h.view == menu) else { return };
                    let ours: Vec<AnyWindowHandle> = h.wins.borrow().iter().map(|w| w.0).collect();
                    let active = cx.active_window();
                    if active.is_none_or(|a| !ours.contains(&a)) {
                        let _ = h.main.update(cx, |_, window, cx| close(window, cx));
                    }
                });
            }),
        ];
        let focus = cx.focus_handle();
        if level == 0 {
            window.focus(&focus, cx);
        }
        let glass = liquid.map(|dark| PopupGlass::start(crate::look::window::hwnd_of(window), dark));
        let watch = glass.is_some().then(|| glass_watch::watch(cx, |this: &mut Self| this.glass.as_mut()));
        Self { menu, level, focus, glass, _watch: watch, _subs: subs }
    }
}

impl Render for LevelView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let level = self.level;
        let vh = f32::from(window.viewport_size().height);
        let panel = self.menu.update(cx, |m, cx| {
            let levels = m.levels();
            levels.get(level).cloned().map(|items| m.render_level(level, &items, &p, Some(vh), cx))
        });
        // the highlight capsule is still gliding: draw again next frame (and only then)
        if self.menu.read(cx).glide_busy(level) {
            window.request_animation_frame();
        }
        // over glass a wash, or the plain fill until the glass runs (never a see-through panel with nothing behind it)
        let fill = self.glass.as_ref().map(|g| glass_watch::panel_fill(p.dark, g.fallback()));
        div()
            .size_full()
            .when_some(fill, |d, f| d.bg(f))
            .track_focus(&self.focus)
            .text_color(p.text)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                this.menu.update(cx, |m, cx| m.on_key(ev, window, cx));
            }))
            .children(panel)
    }
}

// ---------- shell icons: one RenderImage per distinct bitmap, reused across menus ----------

#[derive(Default)]
struct IconCache(std::collections::HashMap<u64, Arc<RenderImage>>);
impl Global for IconCache {}

pub fn shell_icon(icon: &rf_win::ctxmenu::ShellIcon, cx: &mut App) -> Option<Arc<RenderImage>> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (icon.width, icon.height).hash(&mut h);
    icon.bgra.hash(&mut h);
    let key = h.finish();
    if !cx.has_global::<IconCache>() {
        cx.set_global(IconCache::default());
    }
    if let Some(img) = cx.global::<IconCache>().0.get(&key) {
        return Some(img.clone());
    }
    let buf = image::RgbaImage::from_raw(icon.width, icon.height, icon.bgra.clone())?;
    let img = Arc::new(RenderImage::new(vec![image::Frame::new(buf)]));
    let cache = &mut cx.global_mut::<IconCache>().0;
    if cache.len() >= 256 {
        // rare (hundreds of distinct extension icons): start over, free the atlas tiles
        let old: Vec<Arc<RenderImage>> = cache.drain().map(|(_, v)| v).collect();
        cx.defer(move |cx| {
            for img in old {
                cx.drop_image(img, None);
            }
        });
    }
    cx.global_mut::<IconCache>().0.insert(key, img.clone());
    Some(img)
}

/// What the highlight capsule of one level does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Capsule {
    /// Sits behind this row.
    Row(usize),
    /// The pointer is on a row that cannot be chosen: no capsule is drawn, but its place is kept.
    Hidden,
    /// No row has it (and it forgets its place).
    Gone,
}

/// The row the highlight capsule sits behind in level `lv`: the keyboard's or the pointer's row (`hl` is (level, row)), else the row
/// whose submenu is open (`open[lv]`).
fn capsule_target(hl: Option<(usize, usize)>, open: &[usize], lv: usize, items: &[Item]) -> Capsule {
    let on_level = hl.filter(|&(l, ix)| l == lv && ix < items.len());
    if let Some((_, ix)) = on_level.filter(|&(_, ix)| items[ix].selectable()) {
        return Capsule::Row(ix);
    }
    if let Some(ix) = open.get(lv).copied().filter(|&ix| ix < items.len()) {
        return Capsule::Row(ix);
    }
    if on_level.is_some() { Capsule::Hidden } else { Capsule::Gone }
}

fn level_height(items: &[Item], quick: bool, m: &Metrics) -> f32 {
    m.pad_y * 2. + if quick { QUICK_H } else { 0. } + items.iter().map(|i| i.height(m)).sum::<f32>()
}

impl MenuView {
    /// Replaces the top-level items (a drop-down whose content arrives late).
    pub fn set_root(&mut self, items: Vec<Item>, cx: &mut Context<Self>) {
        self.root = Rc::new(items);
        self.root_gen += 1;
        self.open.clear();
        self.hl = None;
        self.glides.borrow_mut().clear();
        cx.notify();
    }

    /// Replaces the lazy submenu `key` in the top level by its (loaded) items.
    pub fn inline_pending(&mut self, key: &'static str, cx: &mut Context<Self>) {
        let Some(Pending::Ready(items)) = self.pending.get(key) else { return };
        let items = items.clone();
        let mut root: Vec<Item> = Vec::with_capacity(self.root.len() + items.len());
        for it in self.root.iter() {
            match it {
                Item::Sub { items: Sub::Pending { key: k, .. }, .. } if *k == key => root.extend(items.iter().cloned()),
                other => root.push(other.clone()),
            }
        }
        // no separator at the end
        while matches!(root.last(), Some(Item::Separator)) {
            root.pop();
        }
        self.root = Rc::new(root);
        self.root_gen += 1;
        self.open.clear();
        self.glides.borrow_mut().clear();
        cx.notify();
    }

    pub fn set_pending(&mut self, key: &'static str, p: Pending, cx: &mut Context<Self>) {
        self.pending.insert(key, p);
        cx.notify();
    }

    fn sub_items(&self, item: &Item) -> Option<Rc<Vec<Item>>> {
        match item {
            Item::Sub { items: Sub::Items(v), .. } => Some(v.clone()),
            Item::Sub { items: Sub::Pending { key, fallback }, .. } => Some(match (self.pending.get(key), fallback) {
                (Some(Pending::Ready(v)), _) if !v.is_empty() => v.clone(),
                (Some(Pending::Ready(_) | Pending::Failed), Some(f)) | (None, Some(f)) => f.clone(),
                (Some(Pending::Ready(_)), None) => Rc::new(vec![placeholder(crate::t!("Başka seçenek yok"))]),
                (None, None) => Rc::new(vec![placeholder(crate::t!("Yükleniyor…"))]),
                (Some(Pending::Failed), None) => Rc::new(vec![placeholder(crate::t!("Seçenekler yüklenemedi"))]),
            }),
            _ => None,
        }
    }

    /// Items of every visible level: root, then each open submenu.
    fn levels(&self) -> Vec<Rc<Vec<Item>>> {
        let mut out = vec![self.root.clone()];
        for &ix in &self.open {
            let Some(item) = out.last().and_then(|l| l.get(ix)).cloned() else { break };
            match self.sub_items(&item) {
                Some(v) => out.push(v),
                None => break,
            }
        }
        out
    }

    fn activate(&mut self, level: usize, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let levels = self.levels();
        let Some(item) = levels.get(level).and_then(|l| l.get(ix)).cloned() else { return };
        match item {
            Item::Action { enabled: true, run, .. } => run_in_main(run, window, cx),
            Item::Sub { .. } => {
                self.open.truncate(level);
                self.open.push(ix);
                self.hl = Some((level + 1, usize::MAX));
                self.skip_to_selectable(1, cx);
                cx.notify();
            }
            _ => {}
        }
    }

    fn hover_row(&mut self, level: usize, ix: usize, is_sub: bool, cx: &mut Context<Self>) {
        self.hl = Some((level, ix));
        self.hover_gen += 1;
        let gen_ = self.hover_gen;
        // already showing exactly this: nothing to schedule
        if is_sub && self.open.len() == level + 1 && self.open[level] == ix {
            cx.notify();
            return;
        }
        if !is_sub && self.open.len() <= level {
            cx.notify();
            return;
        }
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOVER_DELAY).await;
            let _ = this.update(cx, |m, cx| {
                if m.hover_gen != gen_ {
                    return;
                }
                m.open.truncate(level);
                if is_sub {
                    m.open.push(ix);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Moves the keyboard highlight within its level by `dir`, skipping separators.
    fn skip_to_selectable(&mut self, dir: isize, _cx: &mut Context<Self>) {
        let levels = self.levels();
        let (lv, start) = self.hl.unwrap_or((0, usize::MAX));
        let Some(items) = levels.get(lv) else { return };
        let n = items.len() as isize;
        if n == 0 {
            return;
        }
        // usize::MAX = "before the first / after the last"
        let mut i = if start == usize::MAX { if dir > 0 { -1 } else { n } } else { start as isize };
        for _ in 0..n {
            i = (i + dir).rem_euclid(n);
            if items[i as usize].selectable() {
                self.hl = Some((lv, i as usize));
                return;
            }
        }
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        match key {
            "escape" => close(window, cx),
            "down" | "up" => {
                if self.hl.is_none() {
                    self.hl = Some((self.open.len(), usize::MAX));
                }
                self.skip_to_selectable(if key == "down" { 1 } else { -1 }, cx);
                cx.notify();
            }
            "right" => {
                if let Some((lv, ix)) = self.hl {
                    let is_sub = self.levels().get(lv).and_then(|l| l.get(ix)).is_some_and(|i| matches!(i, Item::Sub { .. }));
                    if is_sub {
                        self.activate(lv, ix, window, cx);
                    }
                }
            }
            "left" => {
                if let Some((lv, _)) = self.hl.filter(|(lv, _)| *lv > 0) {
                    let parent = self.open[lv - 1];
                    self.open.truncate(lv - 1);
                    self.hl = Some((lv - 1, parent));
                    cx.notify();
                }
            }
            "enter" | "space" => {
                if let Some((lv, ix)) = self.hl {
                    self.activate(lv, ix, window, cx);
                }
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    fn render_quick(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let mut row = h_flex().h(px(QUICK_H - 1.)).px_1().gap_1().items_center();
        for (i, q) in self.quick.iter().enumerate() {
            let run = q.run.clone();
            let tip = q.tip.clone();
            row = row.child(
                div()
                    .id(("mq", i))
                    .w(px(36.))
                    .h(px(32.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
                    .text_color(if q.enabled { p.text } else { p.text_tertiary })
                    .when(q.enabled, |d| {
                        d.hover(|s| s.bg(p.hover))
                            .active(|s| s.bg(p.pressed))
                            .on_click(cx.listener(move |_, _, window, cx| run_in_main(run.clone(), window, cx)))
                    })
                    .child(Icon::new(q.icon).size(px(16.))),
            );
        }
        v_flex().child(row).child(div().h(px(1.)).mx(px(-liquid::metrics(self.liquid).pad_x)).bg(p.divider)).into_any_element()
    }

    fn capsule_row(&self, lv: usize, items: &[Item]) -> Capsule {
        capsule_target(self.hl, &self.open, lv, items)
    }

    /// Where the capsule of level `lv` is drawn now (top, in the list's own coordinates), or `None` when no row has it. The capsule
    /// glides: a new row aims it there and it moves over about 120 ms.
    fn capsule_top(&self, lv: usize, items: &[Item], m: &Metrics) -> Option<f32> {
        let key = self.open.get(..lv).unwrap_or(&self.open).to_vec();
        let ix = match self.capsule_row(lv, items) {
            Capsule::Row(ix) => ix,
            // over a row that cannot be chosen: not drawn, but it keeps its place, so the next row it goes to is a glide from there
            Capsule::Hidden => return None,
            Capsule::Gone => {
                self.glides.borrow_mut().remove(&key);
                return None;
            }
        };
        let row_top: f32 = items[..ix].iter().map(|i| i.height(m)).sum();
        let to = liquid::highlight_top(row_top, m);
        let mut all = self.glides.borrow_mut();
        let g = all.entry(key).or_insert_with(|| Glide::at(to));
        Some(g.aim(to, Instant::now(), crate::settings::animations()))
    }

    /// Whether the capsule of level `lv` is still on its way.
    fn glide_busy(&self, lv: usize) -> bool {
        if !self.liquid {
            return false;
        }
        let key = self.open.get(..lv).unwrap_or(&self.open).to_vec();
        self.glides.borrow().get(&key).is_some_and(|g| g.running(Instant::now(), crate::settings::animations()))
    }

    /// Whether any level has a capsule on its way (the in-window menu draws all its levels in one view).
    fn any_glide_busy(&self) -> bool {
        (0..=self.open.len()).any(|lv| self.glide_busy(lv))
    }

    /// `column`: the level reserves the leading tick column (liquid look, some row ticked).
    fn render_row(&self, level: usize, ix: usize, item: &Item, column: bool, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let highlighted = self.hl == Some((level, ix)) || self.open.get(level) == Some(&ix);
        let m = liquid::metrics(self.liquid);
        match item {
            Item::Separator => div()
                .h(px(m.sep_h))
                .flex()
                .items_center()
                .child(div().h(px(1.)).flex_1().mx(px(m.sep_inset)).bg(p.divider))
                .into_any_element(),
            Item::Action { label, icon, hint, enabled, checked, .. } => {
                let enabled = *enabled;
                let content = if self.liquid {
                    // the tick is in the leading column and the icon keeps its place after it
                    row_content(label.clone(), icon.clone(), hint.clone(), false, enabled, liquid::tick_slot(column, *checked), p)
                } else {
                    row_content(label.clone(), if *checked { MenuIcon::Lucide(L::Check) } else { icon.clone() }, hint.clone(), false, enabled, liquid::TickSlot::None, p)
                };
                div()
                    .id(("mr", level * 1000 + ix))
                    .h(px(m.row_h))
                    // (the liquid highlight is the capsule behind the rows, not a fill of the row)
                    .when(!self.liquid, |d| d.rounded(px(4.)).when(highlighted && enabled, |d| d.bg(p.hover)).when(enabled, |d| d.active(|s| s.bg(p.pressed))))
                    .on_hover(cx.listener(move |m, on: &bool, _, cx| {
                        if *on {
                            m.hover_row(level, ix, false, cx)
                        }
                    }))
                    .when(enabled, |d| d.on_click(cx.listener(move |m, _, window, cx| m.activate(level, ix, window, cx))))
                    .child(content)
                    .into_any_element()
            }
            Item::Sub { label, icon, .. } => div()
                .id(("mr", level * 1000 + ix))
                .h(px(m.row_h))
                .when(!self.liquid, |d| d.rounded(px(4.)).when(highlighted, |d| d.bg(p.hover)))
                .on_hover(cx.listener(move |m, on: &bool, _, cx| {
                    if *on {
                        m.hover_row(level, ix, true, cx)
                    }
                }))
                .on_click(cx.listener(move |m, _, window, cx| m.activate(level, ix, window, cx)))
                .child(row_content(label.clone(), icon.clone(), None, true, true, liquid::tick_slot(column, false), p))
                .into_any_element(),
        }
    }
}

fn placeholder(text: &str) -> Item {
    Item::Action { label: text.to_string().into(), icon: MenuIcon::None, hint: None, enabled: false, checked: false, run: Rc::new(|_, _| {}) }
}

fn row_content(label: SharedString, icon: MenuIcon, hint: Option<SharedString>, sub: bool, enabled: bool, tick: liquid::TickSlot, p: &Palette) -> AnyElement {
    let icon_el: AnyElement = match icon {
        MenuIcon::None => div().into_any_element(),
        MenuIcon::Lucide(l) => Icon::new(l).size(px(16.)).into_any_element(),
        MenuIcon::Tinted(l, c) => Icon::new(l).size(px(16.)).text_color(c).into_any_element(),
        MenuIcon::Image(img) => gpui_kit::img(img).size(px(16.)).into_any_element(),
        MenuIcon::Dot(c) => div().size(px(10.)).rounded_full().bg(c).into_any_element(),
    };
    h_flex()
        .h_full()
        .px_3()
        .gap_3()
        .items_center()
        .text_sm()
        .text_color(if enabled { p.text } else { p.text_tertiary })
        .children(liquid::tick_cell(tick, if enabled { p.text } else { p.text_tertiary }))
        .child(div().w(px(16.)).flex_shrink_0().flex().items_center().justify_center().child(icon_el))
        .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(label))
        .children(hint.map(|h| div().flex_shrink_0().text_xs().text_color(p.text_secondary).child(h)))
        .when(sub, |d| d.child(Icon::new(L::ChevronRight).size(px(12.)).text_color(p.text_secondary)))
        .into_any_element()
}

impl MenuView {
    /// Rects of every visible level inside `area`, for a menu opened at `anchor` (same space:
    /// window-local for the in-window menu, screen for popups). Fixed width + known row heights
    /// mean positions are computed, not measured, so cascading submenus flip left / move up
    /// before the first frame.
    fn layout(&self, area: Bounds<Pixels>, anchor: Point<Pixels>) -> Vec<Bounds<Pixels>> {
        let levels = self.levels();
        let m = liquid::metrics(self.liquid);
        let (ax, ay) = (f32::from(area.origin.x), f32::from(area.origin.y));
        let (right, bottom) = (ax + f32::from(area.size.width), ay + f32::from(area.size.height));
        let max_h = (bottom - ay - 2. * EDGE).max(m.row_h);
        let mut out: Vec<Bounds<Pixels>> = Vec::new();
        for (lv, items) in levels.iter().enumerate() {
            let quick = lv == 0 && !self.quick.is_empty();
            let h = level_height(items, quick, &m).min(max_h);
            let (x, y) = match out.last() {
                None => {
                    let (px_, py_) = (f32::from(anchor.x), f32::from(anchor.y));
                    let mut x = if px_ + W > right - EDGE { px_ - W } else { px_ };
                    x = x.clamp(ax + EDGE, (right - EDGE - W).max(ax + EDGE));
                    let mut y = if py_ + h > bottom - EDGE { py_ - h } else { py_ };
                    if y < ay + EDGE {
                        y = (bottom - EDGE - h).max(ay + EDGE);
                    }
                    (x, y)
                }
                Some(prev) => {
                    let (px0, py0) = (f32::from(prev.origin.x), f32::from(prev.origin.y));
                    let parent = &levels[lv - 1];
                    let ix = self.open[lv - 1];
                    let parent_quick = lv == 1 && !self.quick.is_empty();
                    let row_y = py0 + m.pad_y + if parent_quick { QUICK_H } else { 0. } + parent[..ix].iter().map(|i| i.height(&m)).sum::<f32>();
                    let mut x = px0 + W - 2.;
                    if x + W > right - EDGE {
                        x = px0 - W + 2.;
                    }
                    x = x.max(ax + EDGE);
                    let mut y = row_y - m.pad_y;
                    if y + h > bottom - EDGE {
                        y = (bottom - EDGE - h).max(ay + EDGE);
                    }
                    (x, y)
                }
            };
            out.push(Bounds::new(point(px(x), px(y)), size(px(W), px(h))));
        }
        out
    }

    /// Content of one level (quick row + rows), scrolling when taller than `max_h`.
    fn render_level(&self, lv: usize, items: &Rc<Vec<Item>>, p: &Palette, max_h: Option<f32>, cx: &mut Context<Self>) -> AnyElement {
        let m = liquid::metrics(self.liquid);
        let quick = lv == 0 && !self.quick.is_empty();
        let full = level_height(items, quick, &m);
        let scroll = max_h.is_some_and(|mh| full > mh + 0.5);
        let list_max = max_h.unwrap_or(full) - 2. * m.pad_y - if quick { QUICK_H } else { 0. };
        let mut list = v_flex().id(("menu-list", lv)).relative().when(scroll, |d| d.max_h(px(list_max)).overflow_y_scroll());
        // the leading tick column: only in the liquid look, only when some row of this level is ticked
        let column = liquid::has_tick_column(self.liquid, items.iter().any(|i| matches!(i, Item::Action { checked: true, .. })));
        if self.liquid {
            // one capsule behind the rows (the list's first child), gliding from row to row
            if let Some(top) = self.capsule_top(lv, items, &m) {
                list = list.child(liquid::highlight_capsule(top, &m, p.dark));
            }
        }
        for (ix, item) in items.iter().enumerate() {
            list = list.child(self.render_row(lv, ix, item, column, p, cx));
        }
        v_flex()
            .w_full()
            .px(px(m.pad_x))
            .py(px(m.pad_y))
            .when(quick, |d| d.child(self.render_quick(p, cx)))
            .child(list)
            .into_any_element()
    }
}

impl Render for MenuView {
    /// The in-window fallback (popup mode renders through `LevelView`s instead).
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let vp = window.viewport_size();
        let area = Bounds::new(point(px(0.), px(0.)), vp);
        let levels = self.levels();
        let rects = self.layout(area, self.pos);
        let mut panels: Vec<AnyElement> = Vec::new();
        for (lv, (items, r)) in levels.iter().zip(&rects).enumerate() {
            let (x, y) = (f32::from(r.origin.x), f32::from(r.origin.y));
            let content = self.render_level(lv, items, &p, Some(f32::from(r.size.height)), cx);
            panels.push(
                div()
                    .id(("menu", lv))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(W))
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .text_color(p.text)
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                    .on_scroll(|_, _, cx| cx.stop_propagation())
                    .child(content)
                    // Fluent flyout entrance: a short fade with a few pixels of drop
                    .with_animation(("menu-in", lv), Animation::new(Duration::from_millis(140)).with_easing(ease_out_quint()), move |el, t| {
                        el.opacity(t).top(px(y - 6. * (1. - t)))
                    })
                    .into_any_element(),
            );
        }

        // a highlight capsule still on its way: draw again next frame (and only then)
        if self.any_glide_busy() {
            window.request_animation_frame();
        }
        div()
            .id("menu-layer")
            .absolute()
            .inset_0()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| m.on_key(ev, window, cx)))
            // click-away
            .on_mouse_down(MouseButton::Left, cx.listener(|_, _, window, cx| close(window, cx)))
            // right-click elsewhere: the click also reaches the list below, which may open a
            // new menu (in either order) — only close if this menu is still the open one
            .on_mouse_down(MouseButton::Right, cx.listener(|_, _, window, cx| {
                let me = cx.entity_id();
                if host(cx).is_some_and(|h| h.view.entity_id() == me) {
                    close(window, cx)
                }
            }))
            .on_scroll(cx.listener(|_, _, window, cx| close(window, cx)))
            .children(panels)
    }
}

#[cfg(test)]
mod tests {
    use super::{Capsule, Item, MenuIcon, capsule_target, level_height};
    use crate::liquid::metrics;

    fn rows(n: usize, seps: usize) -> Vec<Item> {
        let mut v: Vec<Item> = (0..n).map(|i| Item::action(format!("row {i}"), MenuIcon::None, |_, _| {})).collect();
        v.extend((0..seps).map(|_| Item::Separator));
        v
    }

    #[test]
    fn the_classic_menu_keeps_its_measures() {
        let m = metrics(false);
        // 4 px above and below, 32 px rows, 9 px separators: what the menu always was
        assert_eq!(level_height(&rows(5, 2), false, &m), 8.0 + 5.0 * 32.0 + 2.0 * 9.0);
        // the quick row of icon buttons sits on top
        assert_eq!(level_height(&rows(1, 0), true, &m), 8.0 + 45.0 + 32.0);
    }

    #[test]
    fn the_capsule_follows_the_pointer_and_keeps_its_place_over_a_disabled_row() {
        let items = vec![
            Item::action("a", MenuIcon::None, |_, _| {}),
            Item::action("b (disabled)", MenuIcon::None, |_, _| {}).enabled(false),
            Item::Separator,
            Item::action("c", MenuIcon::None, |_, _| {}),
            Item::sub("d", MenuIcon::None, vec![]),
        ];
        // a chosen row, a sub
        assert_eq!(capsule_target(Some((0, 0)), &[], 0, &items), Capsule::Row(0));
        assert_eq!(capsule_target(Some((0, 4)), &[], 0, &items), Capsule::Row(4));
        // on a disabled row (or a separator) there is no capsule but it is not forgotten: a -> b -> c glides from a to c
        assert_eq!(capsule_target(Some((0, 1)), &[], 0, &items), Capsule::Hidden);
        assert_eq!(capsule_target(Some((0, 2)), &[], 0, &items), Capsule::Hidden);
        // nothing highlighted in this level (the highlight is in another one): it is gone
        assert_eq!(capsule_target(None, &[], 0, &items), Capsule::Gone);
        assert_eq!(capsule_target(Some((1, 0)), &[], 0, &items), Capsule::Gone);
        // a row beyond the level (the items changed under the highlight) is not a row
        assert_eq!(capsule_target(Some((0, 9)), &[], 0, &items), Capsule::Gone);
        // the row whose submenu is open keeps the capsule while the pointer is in the submenu, and over a disabled row of this level
        assert_eq!(capsule_target(Some((1, 0)), &[4], 0, &items), Capsule::Row(4));
        assert_eq!(capsule_target(Some((0, 1)), &[4], 0, &items), Capsule::Row(4));
        // ... but a chosen row under the pointer wins
        assert_eq!(capsule_target(Some((0, 0)), &[4], 0, &items), Capsule::Row(0));
    }

    #[test]
    fn the_liquid_menu_has_34_px_rows_and_6_px_above_and_below() {
        let m = metrics(true);
        assert_eq!(level_height(&rows(5, 2), false, &m), 12.0 + 5.0 * 34.0 + 2.0 * 9.0);
        assert_eq!(Item::Separator.height(&m), 9.0);
        assert_eq!(rows(1, 0)[0].height(&m), 34.0);
    }
}
