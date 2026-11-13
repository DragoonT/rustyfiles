//! Cover mode (shell plan F4): the shell host puts OUR desktop over Explorer's, one borderless window per monitor, each showing the
//! desktop view (`shell/desktop`) for that monitor. Switched on by the setting `shell_desktop_cover` (off by default; the Settings
//! page says: "Explorer's desktop stays underneath and comes back when the dock stops").
//!
//! WHAT RUNS WHEN. Nothing exists while the setting is off. Switched on (at the host's start, or by the file watch / `reload` that
//! brings in a changed settings file): one thread lists the Desktop folders and the layout file and, once, imports Explorer's icon
//! positions; the wallpaper follower reads the wallpaper; when the lists are in, a window is made for every monitor. The windows are
//! made WHEN FIRST NEEDED (that is: when the setting is on and the data is there) and are shown at once, so no window is kept
//! hidden and dirty for nothing; a window is made in tens of milliseconds, which is why monitors that come and go simply get
//! windows made and closed (after a quiet period: [`rf_core::desktop::cover::plan::Debounce`]).
//!
//! WHAT IT TOUCHES. Only its own windows (`rf_win::desktop::cover`); Explorer's windows are looked at, never changed. Stopping is
//! therefore closing the windows: [`stop`]. The desktop folders are written by the user's own actions in the view (rename, delete
//! to the Recycle Bin, paste, new), exactly as the preview's `--writable` mode does, for the user's own Desktop folder (Public and
//! virtual items keep the view's protections). `RUSTYFILES_DESKTOP_DIR` makes a scratch folder the desktop on every monitor, with a
//! layout file of its own (see `desktop::source`); a bad value fails closed to the real Desktop READ-ONLY, and so does a test copy
//! (`RUSTYFILES_CONFIG_DIR` set) that was given no scratch folder or a misspelt variable name ([`real_desktop_guard`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::*;
use rf_core::desktop::cover::geometry::cover_rect;
use rf_core::desktop::cover::plan::{self, Debounce, Have, Op, Want};
use rf_core::desktop::geom::Rect;
use rf_core::desktop::grid::{Spacing, metrics_spaced};
use rf_core::desktop::import::{Assumption, ImportFailure, RawPosition, RawView, convert_with};
use rf_core::desktop::items::{ItemChange, Visibility};
use rf_core::desktop::layout::MonitorSpec;
use rf_core::desktop::store::Store;
use rf_core::desktop::virtuals::VirtualKind;
use rf_core::desktop::wallpaper::Wallpaper;
use rf_win::desktop::monitors::{self, MonitorInfo};
use rf_win::desktop::wallwatch::{Follower, SystemWatch, WallpaperReader};

use super::desktop::shared::{Peers, SharedStore};
use super::desktop::source::{self, Backend, Loaded};
use super::desktop::{DesktopView, Hosted};
use crate::settings::Settings;

/// How long the monitors must be quiet before the windows follow them, and the longest a restless burst may hold that off.
const QUIET_MS: u64 = 400;
const MAX_MS: u64 = 2_500;

// ---------------------------------------------------------------- the pure parts

/// What came of the one-time import of Explorer's icon positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportOutcome {
    /// Nothing to do, and why.
    NotNeeded(&'static str),
    /// The positions were taken: this many of that many fit the monitors.
    Imported { fit: usize, total: usize },
    /// Explorer could not be asked, or its numbers did not fit. The store is untouched and NOT marked as imported, so that a later start
    /// tries again (until the user arranges something, which settles it for good).
    Failed(String),
}

/// The one-time import. `scratch`: the desktop is a scratch folder, which has nothing to do with Explorer's. `read` asks Explorer's
/// desktop view for its positions (it blocks: call this off the UI thread; a fake in the tests).
pub fn import_once(store: &mut Store, scratch: bool, specs: &[MonitorSpec], read: impl FnOnce() -> Result<Vec<RawPosition>, ImportFailure>) -> ImportOutcome {
    import_view_once(store, scratch, specs, || read().map(RawView::from))
}

/// [`import_once`] with everything the view said: its positions and the spacing and icon size it reports for itself.
pub fn import_view_once(store: &mut Store, scratch: bool, specs: &[MonitorSpec], read: impl FnOnce() -> Result<RawView, ImportFailure>) -> ImportOutcome {
    if scratch {
        return ImportOutcome::NotNeeded("a scratch folder (or a protected Desktop) has nothing to import");
    }
    if store.imported_from_explorer {
        return ImportOutcome::NotNeeded("already imported (or the layout was arranged here)");
    }
    if store.blocked().is_some() {
        // a layout file we may not write: importing into it would be lost, and the user's file is not ours to replace
        return ImportOutcome::NotNeeded("the layout file is not writable");
    }
    let view = match read() {
        Ok(r) => r,
        Err(e) => return ImportOutcome::Failed(e.to_string()),
    };
    // The grid the positions are turned into cells of is EXPLORER'S: the spacing its view reports for itself (75 across and 99 down
    // for medium icons on a default Windows 11; other numbers when the user changed Windows' icon spacing). Cells of a grid with
    // another row height would put the icons in uneven rows. The view's numbers are in the primary display's pixels. A view that
    // does not say keeps the store's grid.
    let before = store.spacing;
    let primary_scale = specs.iter().find(|m| m.primary).or(specs.first()).map_or(100, |m| m.scale_percent);
    if let Some(sp) = view.spacing.zip(view.icon_px).and_then(|(s, icon)| Spacing::from_view(s, icon, primary_scale)) {
        store.set_spacing(Some(sp));
    }
    let (step, spacing) = (store.icon_size_step(), store.spacing);
    match convert_with(&view.positions, specs, &|pct| metrics_spaced(step, pct, spacing), Assumption::default()) {
        Ok(c) => {
            store.import_positions(specs, &c.assignments, false);
            ImportOutcome::Imported { fit: c.fit, total: c.total }
        }
        Err(e) => {
            // (nothing was imported: the grid stays what it was)
            store.set_spacing(before);
            ImportOutcome::Failed(e.to_string())
        }
    }
}

/// A window's bounds in the toolkit's terms: logical pixels of the monitor it is on (physical / scale). `rect` is the cover rectangle.
pub fn logical_bounds(rect: Rect, scale: f32) -> (f32, f32, f32, f32) {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    (rect.left as f32 / s, rect.top as f32 / s, rect.width() as f32 / s, rect.height() as f32 / s)
}

/// Whether a display the toolkit reports (its bounds in logical pixels of its own scale) is the monitor `m`.
pub fn is_same_display(logical: (f32, f32, f32, f32), m: &MonitorInfo) -> bool {
    let s = m.dpi as f32 / 96.0;
    let (x, y, w, h) = logical_bounds(m.rect, s);
    (logical.0 - x).abs() < 1.5 && (logical.1 - y).abs() < 1.5 && (logical.2 - w).abs() < 1.5 && (logical.3 - h).abs() < 1.5
}

/// The environment variable that says a test copy really means the user's real Desktop (see [`real_desktop_guard`]).
pub const ENV_REAL_DESKTOP: &str = "RUSTYFILES_COVER_REAL_DESKTOP";

/// Why the real Desktop must not be writable in this start, or `None` when it may be. Cover mode writes to the user's own Desktop
/// (rename, delete, paste), which is right for the product and wrong for a test copy that only failed to say which scratch folder
/// to use. So it fails closed when the environment looks like a test copy that went wrong: `RUSTYFILES_CONFIG_DIR` is set (a test or
/// portable copy) and no scratch folder was given, or a `RUSTYFILES_DESKTOP...` variable exists that is not the one read (a misspelt
/// name). `real_ok` (`RUSTYFILES_COVER_REAL_DESKTOP=1`) is the explicit "yes, the real Desktop" for such a copy. `desktop_dir_given`: the
/// scratch variable has a value; the scratch rules (`source::read_only_for`) decide then.
pub fn real_desktop_guard(desktop_dir_given: bool, config_dir_set: bool, env_names: &[String], real_ok: bool) -> Option<String> {
    if real_ok || desktop_dir_given {
        return None;
    }
    let own = super::desktop::args::ENV_DIR;
    if let Some(stray) = env_names.iter().find(|n| {
        let up = n.to_ascii_uppercase();
        up.starts_with("RUSTYFILES_DESKTOP") && up != own
    }) {
        return Some(format!("{stray} is set but the variable that is read is {own}"));
    }
    config_dir_set.then(|| format!("RUSTYFILES_CONFIG_DIR is set (a test copy) and {own} is not"))
}

/// The dock's "nothing left to show" rule: every window that is open is a panel's or a cover window. Pure, over any kind of id.
pub fn only_these_left<T: PartialEq>(open: &[T], panels: &[T], covers: &[T]) -> bool {
    open.iter().all(|w| panels.contains(w) || covers.contains(w))
}

/// The toolkit's ids of the cover windows that exist now (none when cover mode is off).
pub fn window_ids(cx: &App) -> Vec<WindowId> {
    cx.try_global::<CoverHost>().map(|h| h.windows.iter().map(|w| w.handle.window_id()).collect()).unwrap_or_default()
}

/// Redraws the dock's windows and leaves the cover windows alone. The dock's animations (the glass, the tone) ask for a redraw of every
/// window several times a second; the desktops would be drawn again for nothing, one full monitor each. (They redraw when their own
/// data changes.)
pub fn refresh_dock_windows(cx: &mut App) {
    let covers = window_ids(cx);
    if covers.is_empty() {
        cx.refresh_windows();
        return;
    }
    for w in cx.windows() {
        if !covers.contains(&w.window_id()) {
            let _ = w.update(cx, |_, window, _| window.refresh());
        }
    }
}

fn want_of(m: &MonitorInfo) -> Want {
    Want { key: m.device_key.clone(), rect: m.rect, scale_percent: m.scale_percent, work: m.work }
}

// ---------------------------------------------------------------- the host

/// What the loader thread hands over.
struct Ready {
    loaded: Loaded,
    names: Vec<(VirtualKind, String)>,
}

enum Event {
    Loaded(Box<Ready>),
    Items(Vec<ItemChange>),
    Wallpaper(Box<Wallpaper>),
    /// The z-order may have changed under us (a foreground change, a minimize).
    ZMoved,
}

struct CoverWindow {
    key: String,
    handle: WindowHandle<DesktopView>,
    view: Entity<DesktopView>,
    hwnd: isize,
    /// The monitor this window was last made or placed for.
    seen: Have,
}

/// What was read once for all the monitors.
struct Shared {
    template: Backend,
    store: SharedStore,
    peers: Peers,
    note: Option<String>,
    names: Vec<(VirtualKind, String)>,
}

struct CoverHost {
    /// A number for this start of cover mode: events from an earlier one (switched off and on again quickly) are ignored.
    generation: u64,
    windows: Vec<CoverWindow>,
    shared: Option<Shared>,
    wallpaper: Option<Wallpaper>,
    started: Instant,
    debounce: Debounce,
    debounce_running: bool,
    // (kept alive: dropping them stops them)
    _follower: Follower,
    _zwatch: Option<rf_win::desktop::cover::ZWatch>,
}

impl Global for CoverHost {}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Wallpaper reads for the follower: the monitors as they are now, so that a monitor that came meanwhile has its picture.
struct SystemWallpaper;

impl WallpaperReader for SystemWallpaper {
    fn read(&self) -> Wallpaper {
        let infos = monitors::list();
        // with no shell desktop window the registry alone is read: the COM object may never answer without Explorer (see bare.rs)
        match rf_core::desktop::cover::bare::route_for(rf_win::desktop::bare::state_now()) {
            rf_core::desktop::cover::bare::Route::ComThenRegistry => source::read_wallpaper(&infos),
            route => rf_win::desktop::wallpaper::current_via(route, Arc::new(rf_win::desktop::registry::SystemRegistry), &monitors::device_rects(&infos)),
        }
    }
}

/// The settings file was (re)read, or the host started: brings cover mode in line with the dock switch and the part's own value
/// (`dock::parts`).
pub fn on_settings(cx: &mut App) {
    // the choices for the system icons (Settings -> Masaüstü) are taken first, so that a desktop that starts now reads them; one that
    // is running reads its items again when they changed
    if cx.try_global::<Settings>().is_some_and(super::desktop::choices::update_from_settings)
        && let Some(items) = cx.try_global::<CoverHost>().and_then(|h| h.shared.as_ref()).map(|s| super::desktop::source::SharedItems(s.template.items.0.clone()))
    {
        let spawned = std::thread::Builder::new().name("rf-desktop-icons".into()).spawn(move || items.refresh_virtuals());
        if let Err(e) = spawned {
            log::warn!("cover: could not start the system icons' refresh: {e}");
        }
    }
    let want = cx.try_global::<Settings>().is_some_and(|s| crate::dock::parts::runs(s, crate::dock::parts::Part::Desktop));
    match (want, cx.has_global::<CoverHost>()) {
        (true, false) => start(cx),
        (false, true) => stop(cx),
        _ => {}
    }
}

/// The shell host noticed that the shell's desktop window came or went (Explorer closed, restarted, replaced). The desktop windows are
/// placed again at once (to the bottom of the z-order without a shell desktop, directly above it with one) and the wallpaper is read
/// again, by the route that fits (see `rf_core::desktop::cover::bare`). Nothing at all while cover mode is off.
pub fn shell_changed(cx: &mut App) {
    let Some(host) = cx.try_global::<CoverHost>() else { return };
    host._follower.poke(rf_win::desktop::wallwatch::Cause::Manual);
    // (from the message loop, with the app not borrowed: moving a window answers messages the toolkit must be able to take)
    cx.foreground_executor()
        .spawn(async {
            rf_win::desktop::bare::shell_changed();
        })
        .detach();
}

/// The display configuration changed (a monitor came or went, a resolution, a scale, a work area). The windows follow once the
/// monitors have been quiet for a moment.
pub fn layout_changed(cx: &mut App) {
    let Some(host) = cx.try_global::<CoverHost>() else { return };
    let now = host.started.elapsed().as_millis() as u64;
    let running = host.debounce_running;
    let host = cx.global_mut::<CoverHost>();
    host.debounce.poke(now);
    if running {
        return;
    }
    host.debounce_running = true;
    cx.spawn(async move |cx| {
        loop {
            let wait = cx.update(debounce_step);
            match wait {
                Some(ms) => cx.background_executor().timer(Duration::from_millis(ms.max(10))).await,
                None => break,
            }
        }
    })
    .detach();
}

/// One look at the debounce: `Some(ms)` to wait that long and look again, `None` when it is over (the work was done or the host is gone).
fn debounce_step(cx: &mut App) -> Option<u64> {
    let host = cx.try_global::<CoverHost>()?;
    let now = host.started.elapsed().as_millis() as u64;
    if !host.debounce.pending() {
        cx.global_mut::<CoverHost>().debounce_running = false;
        return None;
    }
    if host.debounce.due(now) {
        let h = cx.global_mut::<CoverHost>();
        h.debounce.done();
        h.debounce_running = false;
        reconcile(cx);
        return None;
    }
    host.debounce.wait_ms(now)
}

/// Switches cover mode on.
fn start(cx: &mut App) {
    super::desktop::init(cx);
    let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let (tx, rx) = futures::channel::mpsc::unbounded::<Event>();

    // the scratch rule: RUSTYFILES_DESKTOP_DIR; the cover desktop is writable (for the user's own Desktop folder, or the scratch one),
    // unless this looks like a test copy that was meant to use a scratch folder and was not given one right
    let env_dir = std::env::var(super::desktop::args::ENV_DIR).ok();
    let names: Vec<String> = std::env::vars_os().filter_map(|(k, _)| k.into_string().ok()).collect();
    let config_dir_set = std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some_and(|d| !d.is_empty());
    let real_ok = std::env::var_os(ENV_REAL_DESKTOP).is_some_and(|v| v == "1");
    let guard = real_desktop_guard(env_dir.as_deref().is_some_and(|d| !d.trim().is_empty()), config_dir_set, &names, real_ok);
    if let Some(why) = &guard {
        log::warn!("cover: the real Desktop stays read-only: {why}");
    }
    let mut args = super::desktop::args::parse(&[], env_dir.as_deref());
    args.writable = guard.is_none();
    let settings = cx.global::<Settings>();
    let visibility = Visibility { show_hidden: settings.show_hidden, show_system: settings.show_system_files, dot_hidden: false };
    let config_dir = crate::settings::app_dir();

    let load_tx = tx.clone();
    let spawned = std::thread::Builder::new().name("rf-cover-load".into()).spawn(move || {
        let change_tx = parking_lot::Mutex::new(load_tx.clone());
        let mut loaded = source::load(&args, visibility, config_dir, move |c| {
            let _ = change_tx.lock().unbounded_send(Event::Items(c));
        });
        // The first start of cover mode on the real Desktop: take Explorer's icon positions once, BEFORE the first placement of the
        // items (the first placement would otherwise claim cells for all of them), so that the icons stand where the user left them.
        let scratch = loaded.backend.is_scratch();
        let (user, public) = (rf_win::desktop::items::user_desktop_dir(), rf_win::desktop::items::public_desktop_dir());
        let specs = loaded.backend.specs.clone();
        // (a protected Desktop imports nothing either: it is not meant to be touched, and the layout file is not ours to settle yet)
        let outcome = import_view_once(&mut loaded.store, scratch || loaded.backend.read_only, &specs, || rf_win::desktop::import::read_explorer_positions(user, public).map(|p| RawView { positions: p.positions, spacing: p.spacing, icon_px: p.icon_px }));
        match &outcome {
            ImportOutcome::Imported { fit, total } => log::info!("cover: imported the positions of {fit} of {total} icons from Explorer"),
            ImportOutcome::Failed(why) => log::warn!("cover: Explorer's icon positions were not imported: {why}"),
            ImportOutcome::NotNeeded(why) => log::info!("cover: no import: {why}"),
        }
        // (also for a scratch folder: its system icons, when the user chose to show them, carry the shell's own names too)
        let names = rf_win::sta::call(Duration::from_secs(5), || VirtualKind::ALL.iter().filter_map(|k| rf_win::shellfolder::display_name(&k.parsing_name()).map(|n| (*k, n))).collect::<Vec<_>>()).unwrap_or_default();
        let _ = load_tx.unbounded_send(Event::Loaded(Box::new(Ready { loaded, names })));
    });
    if let Err(e) = spawned {
        log::error!("cover: could not start the loader: {e}");
        crate::dock::parts_status::note(crate::dock::parts::Part::Desktop, Some(crate::dock::parts::Why::NotStarted));
        return;
    }
    crate::dock::parts_status::note(crate::dock::parts::Part::Desktop, None);

    let wall_tx = parking_lot::Mutex::new(tx.clone());
    let follower = Follower::start(&SystemWatch::standard(), Arc::new(SystemWallpaper), Arc::new(rf_win::desktop::wallpaper::DiskStamper), Duration::from_millis(400), move |wp, _| {
        let _ = wall_tx.lock().unbounded_send(Event::Wallpaper(Box::new(wp)));
    });
    let z_tx = parking_lot::Mutex::new(tx.clone());
    let zwatch = match rf_win::desktop::cover::ZWatch::start(move || {
        let _ = z_tx.lock().unbounded_send(Event::ZMoved);
    }) {
        Ok(z) => Some(z),
        Err(e) => {
            log::warn!("cover: the z-order watch did not start (Show Desktop will not bring the windows back): {e:#}");
            None
        }
    };
    rf_win::desktop::bare::begin();
    cx.set_global(CoverHost {
        generation,
        windows: Vec::new(),
        shared: None,
        wallpaper: None,
        started: Instant::now(),
        debounce: Debounce::new(QUIET_MS, MAX_MS),
        debounce_running: false,
        _follower: follower,
        _zwatch: zwatch,
    });
    log::info!("cover: on (generation {generation})");
    drive(rx, generation, cx);
}

/// Handles what the threads report, on the UI thread, until this start of cover mode is over.
fn drive(mut rx: UnboundedReceiver<Event>, generation: u64, cx: &mut App) {
    cx.spawn(async move |cx| {
        while let Some(ev) = rx.next().await {
            let alive = cx.update(|cx| match cx.try_global::<CoverHost>() {
                Some(h) if h.generation == generation => {
                    on_event(ev, cx);
                    true
                }
                _ => false,
            });
            if !alive {
                break;
            }
        }
    })
    .detach();
}

fn on_event(ev: Event, cx: &mut App) {
    match ev {
        Event::Loaded(r) => {
            let Ready { loaded, names } = *r;
            let Loaded { backend, store, outcome, note } = loaded;
            let layout_note = match outcome {
                rf_core::desktop::store::LoadOutcome::Recovered { .. } => Some(crate::t!("Yerleşim dosyası okunamadı; yenisi kullanılıyor").to_string()),
                rf_core::desktop::store::LoadOutcome::Newer { .. } | rf_core::desktop::store::LoadOutcome::Unreadable { .. } => {
                    Some(crate::t!("Yerleşim dosyası kaydedilmeyecek (daha yeni bir sürümden ya da okunamıyor)").to_string())
                }
                _ => None,
            };
            if let Some(n) = &note {
                log::warn!("cover: {n}");
            }
            let protected = backend.read_only && !backend.is_scratch();
            let note = layout_note
                .or(note.map(|_| crate::t!("İstenen klasör kullanılmadı; gerçek masaüstü korumalı gösteriliyor").to_string()))
                .or(protected.then(|| crate::t!("Gerçek masaüstü korumalı: deneme kopyasına RUSTYFILES_DESKTOP_DIR verilmedi").to_string()));
            let host = cx.global_mut::<CoverHost>();
            host.shared = Some(Shared { template: backend, store: SharedStore::new(store), peers: Peers::default(), note, names });
            reconcile(cx);
        }
        Event::Items(changes) => {
            for v in views(cx) {
                v.update(cx, |v, cx| v.items_changed(changes.clone(), cx));
            }
        }
        Event::Wallpaper(wp) => {
            let wp = *wp;
            cx.global_mut::<CoverHost>().wallpaper = Some(wp.clone());
            for v in views(cx) {
                v.update(cx, |v, cx| v.wallpaper_changed(wp.clone(), cx));
            }
        }
        // (from the message loop, with the app not borrowed: moving a window answers messages the toolkit must be able to take)
        Event::ZMoved => {
            cx.foreground_executor()
                .spawn(async {
                    rf_win::desktop::cover::reinsert_if_needed();
                })
                .detach();
        }
    }
}

fn views(cx: &App) -> Vec<Entity<DesktopView>> {
    cx.try_global::<CoverHost>().map(|h| h.windows.iter().map(|w| w.view.clone()).collect()).unwrap_or_default()
}

/// Makes the windows match the monitors: one per monitor, each placed at its monitor.
fn reconcile(cx: &mut App) {
    let infos = monitors::list();
    let Some(host) = cx.try_global::<CoverHost>() else { return };
    if host.shared.is_none() {
        return;
    }
    let want: Vec<Want> = infos.iter().map(want_of).collect();
    let have: Vec<Have> = host.windows.iter().map(|w| w.seen.clone()).collect();
    let ops = plan::reconcile(&have, &want);
    if ops.is_empty() {
        return;
    }
    log::info!("cover: {} monitor(s), {} change(s)", infos.len(), ops.len());
    for op in &ops {
        match op {
            Op::Remove(key) => remove(key, cx),
            Op::Create(key) => {
                if let Some(i) = infos.iter().position(|m| &m.device_key == key) {
                    create(&infos, i, cx);
                }
            }
            Op::Place(key) => {
                if let Some(m) = infos.iter().find(|m| &m.device_key == key) {
                    place_again(m, cx);
                }
            }
            Op::Relayout(key) => {
                if let Some(m) = infos.iter().find(|m| &m.device_key == key)
                    && let Some(w) = cx.global_mut::<CoverHost>().windows.iter_mut().find(|w| &w.key == key)
                {
                    w.seen = Have { key: key.clone(), rect: m.rect, scale_percent: m.scale_percent, work: m.work };
                }
            }
        }
    }
    // (a window that went leaves a dead handle among the views that know each other)
    if let Some(shared) = cx.try_global::<CoverHost>().and_then(|h| h.shared.as_ref()) {
        shared.peers.prune();
    }
    // every view follows the monitors as they are now (its index among them, its work area, ...)
    let list: Vec<(String, Entity<DesktopView>)> = cx.global::<CoverHost>().windows.iter().map(|w| (w.key.clone(), w.view.clone())).collect();
    for (key, v) in list {
        v.update(cx, |v, cx| v.monitors_changed(infos.clone(), &key, cx));
    }
    // the wallpaper's pictures follow the monitors' order: read it again
    if let Some(host) = cx.try_global::<CoverHost>() {
        host._follower.poke(rf_win::desktop::wallwatch::Cause::Manual);
    }
}

/// The toolkit's display for a monitor, if it reports one that matches.
fn display_of(m: &MonitorInfo, cx: &App) -> Option<DisplayId> {
    cx.displays().into_iter().find(|d| {
        let b = d.bounds();
        is_same_display((f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height)), m)
    })
    .map(|d| d.id())
}

fn create(infos: &[MonitorInfo], index: usize, cx: &mut App) {
    let m = &infos[index];
    let scale = m.dpi as f32 / 96.0;
    let (x, y, w, h) = logical_bounds(cover_rect(m.rect), scale);
    let display = display_of(m, cx);
    let Some(host) = cx.try_global::<CoverHost>() else { return };
    let Some(shared) = host.shared.as_ref() else { return };
    let hosted = Hosted {
        backend: Arc::new(shared.template.with_monitors(infos.to_vec(), index)),
        store: shared.store.clone(),
        peers: shared.peers.clone(),
        note: shared.note.clone(),
        wallpaper: host.wallpaper.clone(),
        virtual_names: shared.names.clone(),
    };
    // The window is made HIDDEN, and shown by `place` below once it is a cover window: made visible at once it would flash above
    // every program (a pop-up starts topmost) before the first message of the loop.
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(point(px(x), px(y)), size(px(w), px(h))))),
        display_id: display,
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Opaque,
        ..Default::default()
    };
    let handle = match cx.open_window(opts, move |_, cx| cx.new(|cx| DesktopView::hosted(hosted, cx))) {
        Ok(h) => h,
        Err(e) => {
            log::error!("cover: cannot open a window for a monitor: {e:#}");
            return;
        }
    };
    let Ok(hwnd) = handle.update(cx, |_, window, _| crate::look::window::hwnd_of(window)) else { return };
    let Ok(view) = handle.entity(cx) else { return };
    let seen = Have { key: m.device_key.clone(), rect: m.rect, scale_percent: m.scale_percent, work: m.work };
    cx.global_mut::<CoverHost>().windows.push(CoverWindow { key: m.device_key.clone(), handle, view: view.clone(), hwnd, seen });
    let rect = m.rect;
    // the style, the procedure, the place and the show: from the message loop
    cx.spawn(async move |cx| {
        match rf_win::desktop::cover::prepare(hwnd) {
            Ok(()) => {
                if !rf_win::desktop::cover::place(hwnd, rect) {
                    log::warn!("cover: Windows refused to place the window of a monitor");
                }
            }
            Err(e) => log::error!("cover: {e:#}"),
        }
        cx.update(|cx| view.update(cx, |v, _| v.set_window(hwnd)));
    })
    .detach();
}

fn place_again(m: &MonitorInfo, cx: &mut App) {
    let Some(w) = cx.global_mut::<CoverHost>().windows.iter_mut().find(|w| w.key == m.device_key) else { return };
    w.seen = Have { key: m.device_key.clone(), rect: m.rect, scale_percent: m.scale_percent, work: m.work };
    let (hwnd, rect) = (w.hwnd, m.rect);
    cx.foreground_executor()
        .spawn(async move {
            rf_win::desktop::cover::place(hwnd, rect);
        })
        .detach();
}

fn remove(key: &str, cx: &mut App) {
    let Some(host) = cx.try_global::<CoverHost>() else { return };
    let Some(i) = host.windows.iter().position(|w| w.key == key) else { return };
    let w = cx.global_mut::<CoverHost>().windows.remove(i);
    let _ = w.handle.update(cx, |_, window, _| window.remove_window());
}

/// Switches cover mode off: the windows close, the watches stop. Explorer's desktop was under them all the time and nothing of it was
/// changed, so there is nothing to put back. Safe to call when it is not on, and twice.
pub fn stop(cx: &mut App) {
    if !cx.has_global::<CoverHost>() {
        return;
    }
    let host = cx.remove_global::<CoverHost>();
    crate::dock::parts_status::note(crate::dock::parts::Part::Desktop, None);
    for w in &host.windows {
        // (the view saves the layout when it goes)
        let _ = w.handle.update(cx, |_, window, _| window.remove_window());
    }
    rf_win::desktop::cover::set_raised(false);
    rf_win::desktop::bare::end();
    drop(host);
    log::info!("cover: off");
}

#[cfg(test)]
mod tests {
    // (explicit names: `use super::*` would bring in the toolkit's own `test` attribute through its glob and break `#[test]`)
    use super::{ImportOutcome, MonitorInfo, MonitorSpec, Rect, RawPosition, RawView, ImportFailure, Spacing, Store, Settings, import_once, import_view_once, is_same_display, logical_bounds};
    use super::source::Mode;
    use rf_core::desktop::geom::Size;
    use rf_core::desktop::items::ItemId;

    fn spec(device: &str, w: u32, h: u32) -> MonitorSpec {
        MonitorSpec { device: device.into(), size: Size::new(w, h), scale_percent: 100, work: Rect::new(0, 0, w as i32, h as i32), primary: true, rect: Rect::new(0, 0, w as i32, h as i32) }
    }
    fn raw(name: &str, x: i32, y: i32) -> RawPosition {
        RawPosition { id: ItemId::user(name), x, y }
    }

    #[test]
    fn the_real_desktop_is_not_writable_for_a_test_copy_that_was_not_given_its_scratch_folder_right() {
        use super::real_desktop_guard as guard;
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // an ordinary start of the product: writable
        assert_eq!(guard(false, false, &names(&["PATH", "USERNAME"]), false), None);
        // the scratch variable has a value: the scratch rules decide
        assert_eq!(guard(true, true, &names(&["RUSTYFILES_DESKTOP_DIR"]), false), None);
        // a test copy (its own config folder) with no scratch folder: protected
        assert!(guard(false, true, &names(&["PATH"]), false).is_some());
        // a misspelt name, with or without a test config
        for typo in ["RUSTYFILES_DESKTOPDIR", "rustyfiles_desktop_folder", "RUSTYFILES_DESKTOP_DIRR"] {
            assert!(guard(false, false, &names(&[typo]), false).is_some(), "{typo}");
        }
        // the right name alone (set but empty, so not "given") is not a typo
        assert_eq!(guard(false, false, &names(&["RUSTYFILES_DESKTOP_DIR"]), false), None);
        // the explicit yes
        assert_eq!(guard(false, true, &names(&["RUSTYFILES_DESKTOPDIR"]), true), None);
        assert!(!super::ENV_REAL_DESKTOP.to_ascii_uppercase().starts_with("RUSTYFILES_DESKTOP"), "the explicit switch is not caught by the typo rule");
    }

    #[test]
    fn a_protected_desktop_is_not_imported_into() {
        // (the host passes `scratch || read_only`)
        let mut s = Store::default();
        let out = import_once(&mut s, true, &[spec("A", 1920, 1080)], || panic!("a protected Desktop asks nobody"));
        assert!(matches!(out, ImportOutcome::NotNeeded(_)));
        assert!(!s.is_dirty());
    }

    #[test]
    fn the_dock_has_nothing_left_to_show_when_only_panels_and_cover_windows_are_open() {
        use super::only_these_left;
        // islands (1, 2) gone; a panel (10) and two cover windows (20, 21) remain: the host must end
        assert!(only_these_left(&[10, 20, 21], &[10], &[20, 21]));
        assert!(only_these_left(&[20, 21], &[], &[20, 21]), "covers alone");
        assert!(only_these_left::<u32>(&[], &[10], &[]), "nothing open");
        // an island is still open: it goes on
        assert!(!only_these_left(&[1, 10, 20], &[10], &[20]));
        // the rule before cover mode counted the covers as live windows: with them as the only others it never ended
        assert!(!only_these_left(&[10, 20, 21], &[10], &[]), "what the rule said when covers were not told apart");
    }

    #[test]
    fn the_import_takes_explorers_own_spacing_so_rows_of_99_become_neighbouring_rows() {
        // what Explorer's view reports on this machine: icons 13 right of and 2 below their cell's corner, cells of 75 x 99
        let specs = [spec("A", 1920, 1080)];
        let view = || RawView { positions: vec![raw("a", 13, 2), raw("b", 13, 101), raw("c", 13, 200), raw("d", 13, 299), raw("e", 88, 2)], spacing: Some((75, 99)), icon_px: Some(48) };
        let mut s = Store::default();
        let out = import_view_once(&mut s, false, &specs, || Ok(view()));
        assert!(matches!(out, ImportOutcome::Imported { fit: 5, total: 5 }), "{out:?}");
        assert_eq!(s.spacing, Some(Spacing::WINDOWS_11));
        let cells = |s: &Store| {
            let l = s.layouts.values().next().unwrap().values().next().unwrap();
            ["a", "b", "c", "d", "e"].map(|n| { let slot = &l.items[ItemId::user(n).as_str()]; (slot.col, slot.row) })
        };
        assert_eq!(cells(&s), [(0, 0), (0, 1), (0, 2), (0, 3), (1, 0)], "one row each, no gap");
        assert_eq!(s.metrics_at(100).pitch_y, 99);

        // the same positions on the bare square grid (a view that does not say its spacing): rows of 75, and a gap after the third
        let mut bare = Store::default();
        import_view_once(&mut bare, false, &specs, || Ok(RawView { spacing: None, icon_px: None, ..view() }));
        assert_eq!(bare.spacing, None);
        assert_eq!(cells(&bare), [(0, 0), (0, 1), (0, 2), (0, 4), (1, 0)], "what the first version did");

        // an import that is refused leaves the grid as it was
        let mut refused = Store::default();
        let out = import_view_once(&mut refused, false, &specs, || Ok(RawView { positions: vec![raw("a", 90_000, 90_000)], spacing: Some((75, 99)), icon_px: Some(48) }));
        assert!(matches!(out, ImportOutcome::Failed(_)));
        assert_eq!(refused.spacing, None);
        assert!(!refused.imported_from_explorer);

        // a view on a 150 % primary display reports the same grid in bigger numbers
        let mut big = Store::default();
        let mut hi = spec("A", 2880, 1620);
        hi.scale_percent = 150;
        import_view_once(&mut big, false, &[hi], || Ok(RawView { positions: vec![raw("a", 20, 3)], spacing: Some((113, 149)), icon_px: Some(72) }));
        assert_eq!(big.spacing, Some(Spacing::WINDOWS_11));
    }

    #[test]
    fn the_first_start_takes_explorers_positions_once() {
        let mut s = Store::default();
        let specs = [spec("A", 1920, 1080)];
        let out = import_once(&mut s, false, &specs, || Ok(vec![raw("a", 75, 0), raw("b", 150, 150)]));
        assert!(matches!(out, ImportOutcome::Imported { fit: 2, total: 2 }), "{out:?}");
        assert!(s.imported_from_explorer && s.is_dirty());
        // a second time (the next start, or a replayed path) does nothing and does not even ask Explorer
        let out = import_once(&mut s, false, &specs, || panic!("must not ask Explorer again"));
        assert!(matches!(out, ImportOutcome::NotNeeded(_)));
    }

    #[test]
    fn a_scratch_desktop_never_asks_explorer() {
        let mut s = Store::default();
        let out = import_once(&mut s, true, &[spec("A", 1920, 1080)], || panic!("a scratch folder is not Explorer's desktop"));
        assert!(matches!(out, ImportOutcome::NotNeeded(_)));
        assert!(!s.imported_from_explorer && !s.is_dirty());
    }

    #[test]
    fn a_failure_leaves_the_store_alone_and_lets_the_next_start_try_again() {
        let specs = [spec("A", 1920, 1080)];
        for why in [ImportFailure::NoExplorerDesktop, ImportFailure::TimedOut, ImportFailure::NoItems, ImportFailure::Com("x".into())] {
            let mut s = Store::default();
            let out = import_once(&mut s, false, &specs, || Err(why.clone()));
            assert!(matches!(out, ImportOutcome::Failed(_)), "{why:?}");
            assert!(!s.imported_from_explorer && !s.is_dirty());
        }
        // numbers that do not fit the monitors are refused too (the units are not trusted)
        let mut s = Store::default();
        let out = import_once(&mut s, false, &specs, || Ok(vec![raw("a", 90_000, 90_000), raw("b", 80_000, 1)]));
        assert!(matches!(out, ImportOutcome::Failed(_)), "{out:?}");
        assert!(!s.imported_from_explorer);
    }

    #[test]
    fn a_layout_file_that_cannot_be_written_is_not_imported_into() {
        let dir = std::env::temp_dir().join(format!("rf-cover-blocked-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("l.json");
        std::fs::write(&path, r#"{"version": 99}"#).unwrap();
        let (mut s, _) = Store::load(&path);
        assert!(s.blocked().is_some());
        let out = import_once(&mut s, false, &[spec("A", 1920, 1080)], || panic!("not asked"));
        assert!(matches!(out, ImportOutcome::NotNeeded(_)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn logical_bounds_divide_by_the_monitors_own_scale() {
        assert_eq!(logical_bounds(Rect::new(0, 0, 2560, 1439), 1.0), (0.0, 0.0, 2560.0, 1439.0));
        assert_eq!(logical_bounds(Rect::new(3000, -600, 4500, 1000), 1.5), (2000.0, -400.0, 1000.0, 1066.6666));
        assert_eq!(logical_bounds(Rect::new(0, 0, 100, 100), 0.0), (0.0, 0.0, 100.0, 100.0), "a nonsense scale is 1");
    }

    fn info(rect: Rect, dpi: u32) -> MonitorInfo {
        MonitorInfo { device_key: "m".into(), adapter_name: String::new(), rect, work: rect, primary: false, dpi, scale_percent: (dpi * 100 + 48) / 96 }
    }

    #[test]
    fn a_toolkit_display_is_matched_to_its_monitor_by_its_logical_bounds() {
        // the portrait monitor of the development machine, right of and above the primary one, both at 100 %
        let portrait = info(Rect::new(2560, -618, 4000, 1942), 96);
        assert!(is_same_display((2560.0, -618.0, 1440.0, 2560.0), &portrait));
        assert!(!is_same_display((0.0, 0.0, 2560.0, 1440.0), &portrait));
        // at 150 % the toolkit reports logical numbers
        let scaled = info(Rect::new(3000, -600, 4500, 1000), 144);
        assert!(is_same_display((2000.0, -400.0, 1000.0, 1066.7), &scaled));
    }

    #[test]
    fn the_desktop_comes_with_the_dock_switch_unless_its_own_value_holds_it_off() {
        use crate::dock::parts::{Part, runs};
        // nothing runs without the one switch, and the own value defaults to true
        assert!(!runs(&Settings::default(), Part::Desktop));
        assert!(!runs(&Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap(), Part::Desktop), "a file from before the dock: off with the dock");
        let on = Settings { dock_enabled: true, ..Default::default() };
        assert!(runs(&on, Part::Desktop));
        let back = Settings::parse_lenient_pub(&toml::to_string_pretty(&on).unwrap()).unwrap();
        assert!(runs(&back, Part::Desktop));
        // a file that says false holds it off, and so does a value that cannot be read
        assert!(!runs(&Settings::parse_lenient_pub("dock_enabled = true\nshell_desktop_cover = false\n").unwrap(), Part::Desktop));
        assert!(!runs(&Settings::parse_lenient_pub("dock_enabled = true\nshell_desktop_cover = \"evet\"\n").unwrap(), Part::Desktop));
    }

    #[test]
    fn the_scratch_rule_applies_to_cover_mode_and_the_real_desktop_is_writable_only_when_no_folder_was_asked_for() {
        use super::source::{read_only_for, resolve_mode};
        use crate::shell::desktop::PreviewArgs;
        let dir = std::env::temp_dir().join(format!("rf-cover-scratch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let args = |folder: Option<&std::path::Path>| PreviewArgs { folder: folder.map(std::path::Path::to_path_buf), writable: true, monitor: None, ignored: Vec::new() };
        // RUSTYFILES_DESKTOP_DIR set to a folder: that folder is the desktop (writable, its own layout file)
        let a = args(Some(&dir));
        let (mode, _) = resolve_mode(&a, None, None);
        assert_eq!(mode, Mode::Scratch(dir.clone()));
        assert!(!read_only_for(&a, &mode));
        // no folder: the real Desktop, WRITABLE (it is the desktop: rename, delete to the Recycle Bin, paste and new must work)
        let a = args(None);
        let (mode, _) = resolve_mode(&a, None, None);
        assert_eq!(mode, Mode::Real);
        assert!(!read_only_for(&a, &mode));
        // a bad value fails closed: the real Desktop on screen and READ-ONLY, whatever "writable" says
        let bad = args(Some(&dir.join("not-there")));
        let (mode, note) = resolve_mode(&bad, None, None);
        assert_eq!(mode, Mode::Real);
        assert!(note.is_some());
        assert!(read_only_for(&bad, &mode), "a bad scratch value never makes the real Desktop writable");
        let _ = std::fs::remove_dir_all(dir);
    }
}
