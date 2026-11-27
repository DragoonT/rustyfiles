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
use gpui_kit::component::scroll::{Scrollbar, ScrollbarMode};
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
        /// What a click on the row itself does (the submenu still opens on hover and with the right arrow). "Sıkıştır" compresses
        /// in the default format with a click (Discord: "be able to just click compress from the get-go").
        click: Option<Run>,
    },
    Separator,
}

impl Item {
    pub fn action(label: impl Into<SharedString>, icon: MenuIcon, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Item::Action { label: label.into(), icon, hint: None, enabled: true, checked: false, run: Rc::new(run) }
    }
    pub fn sub(label: impl Into<SharedString>, icon: MenuIcon, items: Vec<Item>) -> Self {
        Item::Sub { label: label.into(), icon, items: Sub::Items(Rc::new(items)), click: None }
    }
    /// A submenu whose row does `run` when it is clicked.
    pub fn sub_click(label: impl Into<SharedString>, icon: MenuIcon, items: Vec<Item>, run: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        Item::Sub { label: label.into(), icon, items: Sub::Items(Rc::new(items)), click: Some(Rc::new(run)) }
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
    /// Rows held back until their lazy items arrive (`hold_back`): added at the end, after a line, only when something came.
    held: std::collections::HashMap<&'static str, Item>,
    hover_gen: u64,
    focus: FocusHandle,
    /// Set when the levels live in their own Acrylic popup windows.
    popup: Option<PopupState>,
    /// The menu was opened from a window that shows glass (or from a popup that does): the liquid look, whose rows have other
    /// measures, so the layout reads it too.
    liquid: bool,
    /// File-list context menus use Explorer-style icons when that theme is enabled.
    explorer_context: bool,
    background: WindowBackgroundAppearance,
    /// Some popups borrow the Explorer surface while keeping the app's original icons.
    original_icons: bool,
    /// The highlight capsule of each level (keyed by the path of open submenus that leads to it), drawn at every paint of that level.
    glides: RefCell<BTreeMap<Vec<usize>, Glide>>,
    /// The scroll of each level's list (a level taller than the screen scrolls): a submenu opens beside its row where the row is now.
    scrolls: RefCell<Vec<ScrollHandle>>,
}

/// Popup mode: each level is a borderless Acrylic window (the material of Windows 11 menus),
/// positioned in screen space so menus are not clipped by the app window.
struct PopupState {
    /// Where the menu was opened, in screen (logical) coordinates.
    anchor: Point<Pixels>,
    /// The display the menu must stay on.
    area: Bounds<Pixels>,
    /// That display, named to every level window (`theme::popup_display`, issue 47).
    display: Option<DisplayId>,
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

/// How often an open pop-up menu is looked at from outside (`watch_orphan`).
const ORPHAN_LOOK: Duration = Duration::from_millis(120);

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
    show_ex(menu, pos, None, false, false, None, window, cx)
}

/// Opens a file-list context menu with Explorer-style icons when its theme is enabled.
pub fn show_context(menu: Menu, pos: Point<Pixels>, window: &mut Window, cx: &mut App) -> Entity<MenuView> {
    let explorer_context = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
    show_ex(menu, pos, None, explorer_context, false, None, window, cx)
}

/// [`show_context`] with the lazy submenu `hold` held back from the start (see [`MenuView::hold_back`]): taken out before the menu's
/// window is made, so the window does not open with the row's height and then shrink (issue 368).
pub fn show_context_holding(menu: Menu, pos: Point<Pixels>, hold: &'static str, window: &mut Window, cx: &mut App) -> Entity<MenuView> {
    let explorer_context = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
    show_ex(menu, pos, None, explorer_context, false, Some(hold), window, cx)
}

/// Opens a file-list context menu with the original app icons.
pub fn show_context_original_icons(menu: Menu, pos: Point<Pixels>, window: &mut Window, cx: &mut App) -> Entity<MenuView> {
    let explorer_context = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
    show_ex(menu, pos, None, explorer_context, true, None, window, cx)
}

/// Desktop menus match the Dock's glass in Automatic mode, even when their own window has no glass.
pub fn show_on_glass(menu: Menu, pos: Point<Pixels>, window: &mut Window, cx: &mut App) -> Entity<MenuView> {
    show_ex(menu, pos, Some(crate::settings::materials_available()), false, false, None, window, cx)
}

fn show_ex(
    menu: Menu,
    pos: Point<Pixels>,
    auto_liquid: Option<bool>,
    explorer_context: bool,
    original_icons: bool,
    hold: Option<&'static str>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<MenuView> {
    if host(cx).is_some() {
        close(window, cx);
    }
    let material = cx.global::<crate::settings::Settings>().menu_material;
    let auto_liquid = auto_liquid.unwrap_or_else(|| crate::glass::liquid(crate::look::window::hwnd_of(window)));
    let (liquid, background) = material.resolve(auto_liquid, explorer_context, crate::settings::materials_available());
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
        held: Default::default(),
        hover_gen: 0,
        focus: cx.focus_handle(),
        popup: None,
        liquid,
        explorer_context,
        background,
        original_icons,
        glides: RefCell::new(BTreeMap::new()),
        scrolls: RefCell::new(Vec::new()),
    });
    if let Some(key) = hold {
        view.update(cx, |m, _| m.hold_back(key));
    }

    // Acrylic popups (Windows 11); RF_INWINDOW_MENU=1 or a failure falls back to drawing the
    // menu inside the window.
    if std::env::var_os("RF_INWINDOW_MENU").is_none() {
        let anchor = window.bounds().origin + pos;
        let area = window.display(cx).map(|d| d.bounds()).unwrap_or_else(|| window.bounds());
        let display = crate::theme::popup_display(window, cx);
        let wins: Wins = Default::default();
        view.update(cx, |m, _| m.popup = Some(PopupState { anchor, area, display, wins: wins.clone() }));
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
            watch_orphan(view.clone(), cx);
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

/// The screen rectangle of a window of ours (device px); `None` when it is gone.
fn screen_rect(w: AnyWindowHandle, cx: &mut App) -> Option<rf_win::dock::layout::Rect> {
    let hwnd = w.update(cx, |_, window, _| crate::look::window::hwnd_of(window)).ok()?;
    let mut r = windows::Win32::Foundation::RECT::default();
    // SAFETY: a plain read of a window's rectangle.
    unsafe { windows::Win32::UI::WindowsAndMessaging::GetWindowRect(windows::Win32::Foundation::HWND(hwnd as *mut _), &mut r).ok()? };
    Some(rf_win::dock::layout::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom })
}

/// Whether the window in front belongs to this process.
fn front_is_ours() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain reads.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.0.is_null() {
            // (nothing is in front for the moment between two windows: not another program's)
            return true;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        pid == windows::Win32::System::Threading::GetCurrentProcessId()
    }
}

/// Watches an open pop-up menu from outside and puts it away when the usual way cannot (`geo::menu_orphaned`: a menu whose window
/// never got the foreground is never told that it lost it). A look every [`ORPHAN_LOOK`] while this menu is the open one; nothing
/// runs once it is closed.
fn watch_orphan(view: Entity<MenuView>, cx: &mut App) {
    use rf_win::dock::panel as geo;
    let opened = Instant::now();
    cx.spawn(async move |cx| {
        let mut was_down = geo::any_button_down();
        loop {
            cx.background_executor().timer(ORPHAN_LOOK).await;
            let go_on = cx.update(|cx| {
                let Some(h) = host(cx).filter(|h| h.view == view && h.popup) else { return false };
                let down = geo::any_button_down();
                let pressed = down && !std::mem::replace(&mut was_down, down);
                was_down = down;
                let wins: Vec<AnyWindowHandle> = h.wins.borrow().iter().map(|w| w.0).collect();
                // (a press counts as outside only when every window of the menu could be asked and none holds the pointer)
                let outside = pressed
                    && geo::cursor().is_some_and(|c| {
                        let rects: Vec<_> = wins.iter().filter_map(|w| screen_rect(*w, cx)).collect();
                        rects.len() == wins.len() && !rects.iter().any(|r| r.contains(c.0, c.1))
                    });
                if geo::menu_orphaned(outside, front_is_ours(), opened.elapsed().as_millis() as u64) {
                    log::info!("menu: put away by its watch (it was not the window in front)");
                    let _ = h.main.update(cx, |_, window, cx| close(window, cx));
                    return false;
                }
                true
            });
            if !go_on {
                break;
            }
        }
    })
    .detach();
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
        // A menu opened meanwhile (a right click on another file while this one was open, which closes this one first): the app's
        // window is not activated again, that took the new menu's windows out of the front and so closed the new menu at once
        // (issue 203: "Right-clicking on another file ... only dismisses the previous context menu").
        if host(cx).is_some() {
            return;
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
    let (wanted, have, dark, liquid, display, background) = {
        let m = view.read(cx);
        let Some(pp) = &m.popup else { return false };
        let display = pp.display;
        let rects = m.layout(pp.area, pp.anchor);
        let wanted: Vec<LevelKey> = rects
            .iter()
            .enumerate()
            .map(|(lv, r)| LevelKey { root_gen: m.root_gen, path: m.open[..lv].to_vec(), bounds: *r })
            .collect();
        let have: Vec<(AnyWindowHandle, LevelKey)> = pp.wins.borrow().clone();
        (
            wanted,
            have,
            matches!(cx.window_appearance(), WindowAppearance::Dark | WindowAppearance::VibrantDark),
            m.liquid,
            display,
            m.background,
        )
    };
    let dark = match crate::settings::theme_override() {
        crate::settings::ThemePref::Light => false,
        crate::settings::ThemePref::Dark | crate::settings::ThemePref::Black => true,
        crate::settings::ThemePref::System => dark,
    };
    let keep = have.iter().zip(&wanted).take_while(|(h, w)| h.1 == **w).count();
    let mut wins: Vec<(AnyWindowHandle, LevelKey)> = have[..keep].to_vec();
    // open the new ones first (so focus moves to a menu window, never to nothing), then drop
    // the stale ones
    for (lv, key) in wanted.iter().enumerate().skip(keep) {
        match open_level(view.clone(), lv, key.bounds, lv == 0, dark, liquid, display, background, cx) {
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

#[allow(clippy::too_many_arguments)]
fn open_level(
    view: Entity<MenuView>,
    level: usize,
    bounds: Bounds<Pixels>,
    focus: bool,
    dark: bool,
    liquid: bool,
    display: Option<DisplayId>,
    background: WindowBackgroundAppearance,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let opaque = matches!(background, WindowBackgroundAppearance::Opaque);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: display,
        titlebar: None,
        focus,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // The panel is opaque. Its rounded corners must clear to transparent instead of white.
        window_background: if opaque { WindowBackgroundAppearance::Transparent } else { theme::window_material(background) },
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
        cx.new(|cx| LevelView::new(view, level, liquid.then_some(dark), opaque, w, cx))
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
    opaque: bool,
    focus: FocusHandle,
    /// The glass behind this level's window, in the liquid look.
    glass: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl LevelView {
    /// `liquid`: `Some(dark)` to sit on glass, `None` for the Acrylic window.
    fn new(
        menu: Entity<MenuView>,
        level: usize,
        liquid: Option<bool>,
        opaque: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
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
        Self { menu, level, opaque, focus, glass, _watch: watch, _subs: subs }
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
        // Solid menus take their colors from the theme.
        let surface = if self.opaque {
            div().size_full().rounded(theme::RADIUS).bg(p.card_bg).border_1().border_color(p.border).shadow_lg()
        } else {
            let fill = self.glass.as_ref().map(|g| glass_watch::panel_fill(p.dark, g.fallback())).or_else(|| theme::popup_fill(&p));
            div().size_full().when_some(fill, |d, f| d.bg(f))
        };
        surface
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

/// A list without separators at its ends or two in a row (what is left when a row between them goes).
fn without_stray_separators(items: Vec<Item>) -> Vec<Item> {
    let mut out: Vec<Item> = Vec::with_capacity(items.len());
    for it in items {
        if matches!(it, Item::Separator) && out.last().is_none_or(|l| matches!(l, Item::Separator)) {
            continue;
        }
        out.push(it);
    }
    while matches!(out.last(), Some(Item::Separator)) {
        out.pop();
    }
    out
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

    /// Takes the lazy submenu `key` out of the menu for now: it is added at the end (after a line) when its items arrive and are not
    /// empty, and never shown when they are (issue 368: "Show more options" showed for a moment and then went).
    pub fn hold_back(&mut self, key: &'static str) {
        let pos = self.root.iter().position(|it| matches!(it, Item::Sub { items: Sub::Pending { key: k, .. }, .. } if *k == key));
        if let Some(ix) = pos {
            let mut root: Vec<Item> = self.root.iter().cloned().collect();
            let item = root.remove(ix);
            self.root = Rc::new(without_stray_separators(root));
            self.held.insert(key, item);
        }
    }

    pub fn set_pending(&mut self, key: &'static str, p: Pending, cx: &mut Context<Self>) {
        let empty = matches!(&p, Pending::Ready(v) if v.is_empty());
        // a held-back row whose items came: it joins the menu at the end
        if !empty && matches!(p, Pending::Ready(_)) && let Some(item) = self.held.remove(key) {
            let mut root: Vec<Item> = self.root.iter().cloned().collect();
            root.push(Item::Separator);
            root.push(item);
            self.root = Rc::new(without_stray_separators(root));
            self.root_gen += 1;
            self.glides.borrow_mut().clear();
        }
        self.pending.insert(key, p);
        // Nothing came for a submenu that has nothing else to show: its row goes, rather than opening on "No other options"
        // (issue 368: "Show more options" with nothing under it)
        if empty {
            let pos = self.root.iter().position(|it| matches!(it, Item::Sub { items: Sub::Pending { key: k, fallback: None }, click: None, .. } if *k == key));
            if let Some(ix) = pos {
                let mut root: Vec<Item> = self.root.iter().cloned().collect();
                root.remove(ix);
                self.root = Rc::new(without_stray_separators(root));
                self.root_gen += 1;
                self.open.clear();
                self.hl = None;
                self.glides.borrow_mut().clear();
            }
        }
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
        self.activate_as(level, ix, false, window, cx)
    }

    /// `open_sub`: a submenu row opens its submenu even when a click on it does something (the right arrow).
    fn activate_as(&mut self, level: usize, ix: usize, open_sub: bool, window: &mut Window, cx: &mut Context<Self>) {
        let levels = self.levels();
        let Some(item) = levels.get(level).and_then(|l| l.get(ix)).cloned() else { return };
        match item {
            Item::Action { enabled: true, run, .. } => run_in_main(run, window, cx),
            Item::Sub { click: Some(run), .. } if !open_sub => run_in_main(run, window, cx),
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
                        self.activate_as(lv, ix, true, window, cx);
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
        let explorer_toolbar = self.explorer_context && self.quick.len() >= 4;
        let mut row = h_flex().h(px(QUICK_H - 1.)).px_1().items_center();
        if explorer_toolbar {
            row = row.w_full();
        } else {
            row = row.gap_1();
        }
        for (i, q) in self.quick.iter().enumerate() {
            let run = q.run.clone();
            let tip = q.tip.clone();
            let icon = q.icon;
            let tint = (!self.original_icons && self.explorer_context).then(|| explorer_icon_color(icon, p.dark)).flatten();
            row = row.child(
                div()
                    .id(("mq", i))
                    .when(explorer_toolbar, |d| d.flex_1())
                    .when(!explorer_toolbar, |d| d.w(px(36.)))
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
                    .child(if self.original_icons {
                        Icon::new(q.icon).size(px(16.)).into_any_element()
                    } else if self.explorer_context {
                        lucide_menu_icon(icon, true, false, tint)
                    } else {
                        Icon::new(q.icon).size(px(16.)).into_any_element()
                    }),
            );
            if explorer_toolbar && i + 1 < self.quick.len() {
                row = row.child(div().w(px(1.)).h(px(30.)).bg(p.divider));
            }
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
            // (no row shrinks: in a level that scrolls, the list squeezed its rows to fit instead, and a submenu then opened beside
            // where its row would have been; issue 175)
            Item::Separator => div()
                .h(px(m.sep_h))
                .flex_shrink_0()
                .flex()
                .items_center()
                .child(div().h(px(1.)).flex_1().mx(px(m.sep_inset)).bg(p.divider))
                .into_any_element(),
            Item::Action { label, icon, hint, enabled, checked, .. } => {
                let enabled = *enabled;
                let content = if self.liquid {
                    // the tick is in the leading column and the icon keeps its place after it
                    row_content(
                        label.clone(),
                        icon.clone(),
                        hint.clone(),
                        false,
                        enabled,
                        liquid::tick_slot(column, *checked),
                        p,
                        self.explorer_context,
                        self.original_icons,
                    )
                } else {
                    row_content(
                        label.clone(),
                        if *checked { MenuIcon::Lucide(L::Check) } else { icon.clone() },
                        hint.clone(),
                        false,
                        enabled,
                        liquid::TickSlot::None,
                        p,
                        self.explorer_context,
                        self.original_icons,
                    )
                };
                div()
                    .id(("mr", level * 1000 + ix))
                    .h(px(m.row_h))
                    .flex_shrink_0()
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
                .flex_shrink_0()
                .when(!self.liquid, |d| d.rounded(px(4.)).when(highlighted, |d| d.bg(p.hover)))
                .on_hover(cx.listener(move |m, on: &bool, _, cx| {
                    if *on {
                        m.hover_row(level, ix, true, cx)
                    }
                }))
                .on_click(cx.listener(move |m, _, window, cx| m.activate(level, ix, window, cx)))
                .child(row_content(
                    label.clone(),
                    icon.clone(),
                    None,
                    true,
                    true,
                    liquid::tick_slot(column, false),
                    p,
                    self.explorer_context,
                    self.original_icons,
                ))
                .into_any_element(),
        }
    }
}

fn explorer_icon_color(icon: L, dark: bool) -> Option<Hsla> {
    let color = match icon {
        L::Scissors | L::Copy | L::ClipboardCopy | L::ClipboardPaste | L::Share2 | L::Link | L::CirclePlus | L::SquareArrowOutUpRight
        | L::TextCursorInput | L::PencilLine => {
            if dark { 0x60CDFF } else { 0x0067C0 }
        }
        L::Folder | L::FolderOpen | L::FolderDown | L::FolderInput | L::FolderOutput | L::FolderTree => {
            if dark { 0xF0C75E } else { 0xE2B53A }
        }
        L::Trash | L::Wrench | L::Info | L::FileText => {
            if dark { 0xC5C6C8 } else { 0x626262 }
        }
        _ => return None,
    };
    Some(rgb(color).into())
}

fn placeholder(text: &str) -> Item {
    Item::Action { label: text.to_string().into(), icon: MenuIcon::None, hint: None, enabled: false, checked: false, run: Rc::new(|_, _| {}) }
}

fn row_content(
    label: SharedString,
    icon: MenuIcon,
    hint: Option<SharedString>,
    sub: bool,
    enabled: bool,
    tick: liquid::TickSlot,
    p: &Palette,
    explorer_context: bool,
    original_icons: bool,
) -> AnyElement {
    let icon_el: AnyElement = match icon {
        MenuIcon::None => div().into_any_element(),
        MenuIcon::Lucide(l) => {
            let tint = (!original_icons && explorer_context).then(|| explorer_icon_color(l, p.dark)).flatten();
            lucide_menu_icon(l, explorer_context, original_icons, tint)
        }
        MenuIcon::Tinted(l, c) => lucide_menu_icon(l, explorer_context, original_icons, Some(c)),
        MenuIcon::Image(img) => div()
            .when(!enabled, |d| d.opacity(0.45))
            .child(gpui_kit::img(img).size(px(16.)))
            .into_any_element(),
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

fn lucide_menu_icon(icon: L, explorer_context: bool, original_icons: bool, tint: Option<Hsla>) -> AnyElement {
    #[cfg(target_os = "windows")]
    if explorer_context && !original_icons {
        if let Some(glyph) = crate::icons::windows_theme_icon_glyph(icon) {
            return crate::icons::windows_theme_glyph(glyph, px(16.));
        }
    }

    if original_icons {
        return Icon::new(icon).size(px(16.)).into_any_element();
    }

    let icon = Icon::new(icon).size(px(16.));
    match tint {
        Some(color) => icon.text_color(color).into_any_element(),
        None => icon.into_any_element(),
    }
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
                    // (a parent that scrolls: where the row is now; the offset is 0 or below)
                    let scrolled = f32::from(self.scroll_of(lv - 1).offset().y);
                    let row_y = py0 + m.pad_y + if parent_quick { QUICK_H } else { 0. } + parent[..ix].iter().map(|i| i.height(&m)).sum::<f32>() + scrolled;
                    let mut x = px0 + W - 2.;
                    if x + W > right - EDGE {
                        x = px0 - W + 2.;
                    }
                    x = x.max(ax + EDGE);
                    let mut y = row_y - m.pad_y;
                    if y + h > bottom - EDGE {
                        y = (bottom - EDGE - h).max(ay + EDGE);
                    }
                    // (its row scrolled up past the screen's top: the submenu stays on the screen; above it, its window was given
                    // a place on no display and came up as large as the screen, issue 205)
                    y = y.max(ay + EDGE);
                    (x, y)
                }
            };
            out.push(Bounds::new(point(px(x), px(y)), size(px(W), px(h))));
        }
        out
    }

    /// The scroll of level `lv`'s list (made on first use).
    fn scroll_of(&self, lv: usize) -> ScrollHandle {
        let mut v = self.scrolls.borrow_mut();
        while v.len() <= lv {
            v.push(ScrollHandle::new());
        }
        v[lv].clone()
    }

    /// Content of one level (quick row + rows), scrolling when taller than `max_h`.
    fn render_level(&self, lv: usize, items: &Rc<Vec<Item>>, p: &Palette, max_h: Option<f32>, cx: &mut Context<Self>) -> AnyElement {
        let m = liquid::metrics(self.liquid);
        let quick = lv == 0 && !self.quick.is_empty();
        let full = level_height(items, quick, &m);
        let scroll = max_h.is_some_and(|mh| full > mh + 0.5);
        let list_max = max_h.unwrap_or(full) - 2. * m.pad_y - if quick { QUICK_H } else { 0. };
        let handle = self.scroll_of(lv);
        let mut list =
            v_flex().id(("menu-list", lv)).relative().when(scroll, |d| d.max_h(px(list_max)).overflow_y_scroll().track_scroll(&handle));
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
        // a level taller than the screen scrolls, and shows that it does (issue 175: "the context menu often gets cut off")
        let body: AnyElement = if scroll {
            div()
                .relative()
                .child(list)
                .child(Scrollbar::vertical(&handle).id(("menu-scrollbar", lv)).mode(ScrollbarMode::Always))
                .into_any_element()
        } else {
            list.into_any_element()
        };
        v_flex()
            .w_full()
            .px(px(m.pad_x))
            .py(px(m.pad_y))
            .when(quick, |d| d.child(self.render_quick(p, cx)))
            .child(body)
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
    use super::{Capsule, Item, MenuIcon, capsule_target, level_height, without_stray_separators};

    /// Issue 368: when an empty "Show more options" row goes, the separators around it do not pile up.
    #[test]
    fn a_removed_row_leaves_no_stray_separators() {
        let a = || Item::action("a", MenuIcon::None, |_, _| {});
        let kinds = |v: &[Item]| v.iter().map(|i| if matches!(i, Item::Separator) { '-' } else { 'a' }).collect::<String>();
        assert_eq!(kinds(&without_stray_separators(vec![a(), Item::Separator, Item::Separator, a()])), "a-a");
        assert_eq!(kinds(&without_stray_separators(vec![a(), Item::Separator])), "a");
        assert_eq!(kinds(&without_stray_separators(vec![Item::Separator, a(), Item::Separator, a()])), "a-a");
    }
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
