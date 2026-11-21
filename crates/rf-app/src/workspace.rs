//! Window chrome: title bar with tabs, toolbar (nav + breadcrumb/address + search),
//! sidebar, active tab content, status bar.

use crate::t;
use std::path::{Path, PathBuf};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::{Location, format};

use crate::actions::*;
use crate::folder_view::{FolderEvent, FolderView, ViewMode};
use crate::image_cache::ImageCache;
use crate::preview::Preview;
use crate::places::{self, Places};
use crate::theme::{self, Palette};

// the sidebar's folder tree (setting `sidebar_tree`): the view and the window's hooks for it
#[path = "sidebar_tree.rs"]
mod sidebar_tree;
// the preview pane under the file list (setting `preview_position` = bottom)
#[path = "preview_dock.rs"]
mod preview_dock;
// commands typed into the address field (setting `address_commands`): what the Enter handler does with text that is not a place
#[path = "address_cmd.rs"]
mod address_cmd;

/// Payload while a pinned sidebar entry is dragged (reordering).
#[derive(Clone)]
struct PinDrag {
    ix: usize,
    /// Identity of the dragged pin (indices of the drawn list can be stale).
    path: PathBuf,
    title: SharedString,
}

impl Render for PinDrag {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        div()
            .px_3()
            .py_1p5()
            .rounded(theme::RADIUS)
            .bg(p.card_bg)
            .border_1()
            .border_color(p.accent)
            .text_sm()
            .text_color(p.text)
            .shadow_md()
            .child(self.title.clone())
    }
}

/// Payload while a tab is dragged (reordering).
#[derive(Clone)]
struct TabDrag {
    ix: usize,
    title: SharedString,
}

impl Render for TabDrag {
    fn render(&mut self, window: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        div()
            .px_3()
            .py_1p5()
            .rounded(theme::RADIUS)
            .bg(p.tab_active)
            .border_1()
            .border_color(p.accent)
            .text_sm()
            .text_color(p.text)
            .shadow_md()
            .child(self.title.clone())
    }
}
#[derive(Clone)]
/// Window placement to go back to when leaving the compact overlay.
struct CompactRestore {
    rect: [i32; 4],
    maximized: bool,
    topmost: bool,
}

/// Compact overlay size and minimum (logical px).
const COMPACT_W: f32 = 420.;
const COMPACT_H: f32 = 560.;
const COMPACT_MIN: (u16, u16) = (340, 300);

/// A tab holds one folder view, or two side by side (dual pane).
#[derive(Clone)]
struct Tab {
    left: Entity<FolderView>,
    right: Option<Entity<FolderView>>,
    right_active: bool,
}

impl Tab {
    /// The pane that has focus (toolbar, sidebar and status bar act on it).
    fn view(&self) -> &Entity<FolderView> {
        match (&self.right, self.right_active) {
            (Some(r), true) => r,
            _ => &self.left,
        }
    }
}

pub struct Workspace {
    tabs: Vec<Tab>,
    /// Compact overlay (small, always-on-top window): the placement to restore on exit.
    compact: Option<CompactRestore>,
    active: usize,
    closed: Vec<Location>,
    places: Entity<Places>,
    images: Entity<ImageCache>,
    ops: Entity<crate::status_center::Ops>,
    preview: Entity<Preview>,
    preview_open: bool,
    /// Space: big preview of the focused item over the file list.
    quick_look: Option<Entity<Preview>>,
    shelf: Entity<crate::shelf_view::ShelfView>,
    shelf_open: bool,
    address: Entity<InputState>,
    editing_address: bool,
    /// The Omnibar is in search mode (Ctrl+F / its search button) instead of showing the path.
    search_mode: bool,
    /// (dark, system appearance) last pushed to the window frame (see render).
    frame_dark: Option<(bool, WindowAppearance)>,
    /// Background material last applied to the window.
    applied_backdrop: Option<crate::settings::BackdropPref>,
    /// The backdrop setting last seen: a change of it lets Glass try again after a failure.
    backdrop_pref_seen: Option<crate::settings::BackdropPref>,
    /// This window's Glass: the HWND the engine was given (0: none), the tint it was given, and whether it failed (the window is on Mica).
    glass_hwnd: isize,
    glass_dark: Option<bool>,
    glass_failed: bool,
    /// The engine has drawn this window's glass and the window is see-through now. Until then it keeps its Mica: a transparent window
    /// with nothing behind it shows the desktop through the palette's translucent layers.
    glass_shown: bool,
    /// How the engine last said it keeps the windows out of the capture: the settings page's note depends on it, and the engine can
    /// change it later (a session that fails makes it fall back).
    glass_capture: rf_win::glass::CaptureMode,
    /// Counts the attach attempts; a watcher of an older attempt stops.
    glass_gen: u32,
    /// Set by `render` around the call that builds the sidebar: the selected row of that sidebar is a glass lens (the engine draws the
    /// pill behind the scene) instead of the Fluent fill. False for the rail and for the rail's peek over the content, which is opaque.
    sidebar_lens: bool,
    /// The selected row of this frame's sidebar has claimed the lens (only the first of several selected rows does). A frame that ends
    /// without a claim takes the lens out.
    sidebar_lens_claimed: std::cell::Cell<bool>,
    /// Set by `render` around the call that builds the tab strip: the selected tab is a glass lens (the engine draws it behind the
    /// scene) instead of the Fluent fill.
    tab_lens: bool,
    /// This frame's strip gave the selected tab its lens / the "+" button its lens. A frame that ends without a claim takes the lens out.
    tab_lens_claimed: bool,
    plus_lens_claimed: bool,
    /// Address-bar folder completions for the typed text, and the highlighted one.
    addr_suggest: Vec<PathBuf>,
    addr_sel: Option<usize>,
    _addr_task: Option<Task<()>>,
    search: Entity<InputState>,
    /// Files' "Filter by" strip under the command bar (name filter for this folder).
    filter_bar: bool,
    filter_input: Entity<InputState>,
    sidebar_open: bool,
    sidebar_w: f32,
    sidebar_width_explorer_mode: bool,
    /// Icons only (RAIL_W wide); file dialogs start this way.
    sidebar_rail: bool,
    /// The full sidebar shown over the content while the pointer is on the rail (or on it).
    rail_peek: [bool; 2],
    /// The colour the window shows behind the rail (Mica's wallpaper tint), read off the screen
    /// when the full sidebar opens over the content: painted with it, the sidebar looks like the
    /// docked one instead of a neutral grey slab over a tinted window.
    rail_peek_bg: Option<Hsla>,
    /// The full sidebar is out over the content (the state the last frame drew), it is on its way back in, and a number that
    /// changes with every turn (the motions are keyed by it, and a way back in that was overtaken by a new opening ends nothing).
    rail_peek_open: bool,
    rail_peek_closing: bool,
    rail_peek_serial: u64,
    preview_w: f32,
    /// The preview pane at the bottom: its height, the measured row and the debounced write (idle while the pane is at the right).
    preview_dock: preview_dock::Dock,
    /// Debounced settings write after a pane resize.
    _width_save: Option<Task<()>>,
    /// The saved session this window holds (opened from it, or saved under it): its name stands at the far left of the tabs, so the
    /// workspace is known at a glance (a Reddit suggestion, 2026-11-18). Shown only while a session of that name is saved.
    session_name: Option<String>,
    palette: Option<Entity<crate::command_palette::CommandPalette>>,
    bulk: Option<Entity<crate::bulk_rename::BulkRename>>,
    /// Last window title / session snapshot we pushed (avoid redundant work every frame).
    last_title: String,
    /// Tab strip scrolling (many tabs): keeps the active tab in view.
    tab_scroll: ScrollHandle,
    sidebar_scroll_hovered: bool,
    tab_scrolled_to: Option<usize>,
    /// Tab being opened with an animation: (its view, a serial so each opening animates anew).
    tab_anim: Option<(Entity<FolderView>, u64)>,
    tab_anim_serial: u64,
    /// Files dragged over an inactive tab: (tab, serial). Held still there, the tab opens.
    drag_hover_tab: Option<(usize, u64)>,
    /// Files are being dragged over the window: the sidebar shows its "drop here to pin" row.
    sidebar_ext_drag: bool,
    /// The sidebar's folder tree (empty unless the setting is on).
    side_tree: sidebar_tree::Holder,
    /// This window answers another app's Open/Save dialog (see `picker`).
    picker: Option<std::rc::Rc<crate::picker::PickSession>>,
    /// Counts the dialog's size changes: only the last one is kept (`set_picker`).
    picker_size_gen: u64,
    /// Window width from the last frame (breadcrumb collapsing).
    viewport_w: f32,
    /// The address box's width for the path as last laid out (logical px; 0 before the first layout): what the breadcrumb folds its
    /// leading folders to fit (issue 164: a guess from the window's width folded them with room to spare).
    crumb_room: std::rc::Rc<std::cell::Cell<f32>>,
    /// HyperDownload is installed: the sidebar has its row (looked at on start and when the Add-ons page changes something).
    hd_installed: bool,
    /// `addons::CHANGES` when `hd_installed` was looked at.
    hd_seen: u64,
    last_session: Vec<String>,
    _bulk_sub: Option<Subscription>,
    _palette_sub: Option<Subscription>,
    _subs: Vec<Subscription>,
}

/// Every open main window's workspace, so app-level events (a second launch) can reach one.
#[derive(Default)]
pub struct OpenWorkspaces(Vec<(AnyWindowHandle, WeakEntity<Workspace>)>);
impl Global for OpenWorkspaces {}

#[derive(Clone)]
struct MainPlacement {
    bounds: [f32; 5],
    display: Option<(String, [f32; 2])>,
}

impl MainPlacement {
    fn capture(window: &Window, cx: &App) -> Option<Self> {
        let (b, max) = match window.window_bounds() {
            WindowBounds::Windowed(b) => (b, 0.),
            WindowBounds::Maximized(b) | WindowBounds::Fullscreen(b) => (b, 1.),
        };
        if b.size.width < px(500.) || b.size.height < px(350.) {
            return None;
        }
        let bounds = [f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height), max];
        let current_display = window.display(cx);
        let display = Workspace::hwnd(window)
            .and_then(rf_win::dock::monitors::monitor_of_window)
            .and_then(|h| {
                let on_monitor = |id| u64::from(id) == h as u64;
                current_display.as_ref().filter(|d| on_monitor(d.id())).cloned()
                    .or_else(|| cx.displays().into_iter().find(|d| on_monitor(d.id())))
            })
            .or(current_display)
            .and_then(|d| {
                let offset = b.origin - d.bounds().origin;
                Some((d.uuid().ok()?.to_string(), [f32::from(offset.x), f32::from(offset.y)]))
            });
        Some(Self { bounds, display })
    }

    fn apply(self, settings: &mut crate::settings::Settings) {
        settings.window = Some(self.bounds);
        settings.window_display = self.display;
    }
}

#[derive(Default)]
struct MainPlacements(std::collections::HashMap<WindowId, MainPlacement>);
impl Global for MainPlacements {}

fn remember_placement(window: &Window, cx: &mut App) {
    if let Some(placement) = MainPlacement::capture(window, cx) {
        cx.default_global::<MainPlacements>().0.insert(window.window_handle().window_id(), placement);
    }
}

pub(crate) fn save_closed_placement(id: WindowId, cx: &mut App) {
    if let Some(placement) = cx.default_global::<MainPlacements>().0.remove(&id) {
        cx.update_global::<crate::settings::Settings, _>(|s, _| placement.apply(s));
        cx.global::<crate::settings::Settings>().save();
    }
}

/// A second launch forwarded its argument: open it as a tab in the most recent live window
/// and bring that window up; with no window left, open a new one.
/// Taskbar jump list: "New window" / "New tab" and the pinned folders. Rewritten only when the
/// pins or the language change; test runs (several instances) leave the user's list alone.
fn update_jump_list(places: &crate::places::Places) {
    static LAST: std::sync::Mutex<Option<(Vec<(String, PathBuf)>, Vec<(String, PathBuf)>, [&'static str; 4])>> = std::sync::Mutex::new(None);
    if !places.loaded || !places.extras_loaded || std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_some() || std::env::var_os("RUSTYFILES_INSTANCE").is_some() {
        return;
    }
    // in the UI language, as the sidebar shows them (a default pin's title is its Turkish name)
    let pins: Vec<(String, PathBuf)> = places.pinned.iter().map(|p| (crate::i18n::td(&p.title), p.path.clone())).collect();
    // the folders used last (Windows' recent items), without the pinned ones
    let recent: Vec<(String, PathBuf)> = places
        .recent
        .iter()
        .filter(|r| r.is_dir && !pins.iter().any(|(_, p)| *p == r.target))
        .take(6)
        .map(|r| (rf_win::localized::label_if_known(&r.target).map_or_else(|| r.name.clone(), |l| l.to_string()), r.target.clone()))
        .collect();
    let labels = [t!("Yeni pencere"), t!("Yeni sekme"), t!("Sabitlenenler"), t!("Son klasörler")];
    {
        let mut last = LAST.lock().unwrap();
        if last.as_ref() == Some(&(pins.clone(), recent.clone(), labels)) {
            return;
        }
        *last = Some((pins.clone(), recent.clone(), labels));
    }
    let Ok(exe) = std::env::current_exe() else { return };
    // A run of changes is ONE write: of the last of them, a moment after it (`TURN` says which change is the last). WHY: the recent
    // folders can change several times within a second (nine times in five seconds in a user's log), every change started a write
    // on a thread of its own, and the writes collided ("unable to remove the file to be replaced", see `rf_win::jumplist::set`).
    use std::sync::atomic::Ordering::Relaxed;
    static TURN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let turn = TURN.fetch_add(1, Relaxed) + 1;
    std::thread::spawn(move || {
        std::thread::sleep(JUMP_LIST_SETTLE);
        if TURN.load(Relaxed) != turn {
            return;
        }
        let tasks = [(labels[0], rf_win::jumplist::NEW_WINDOW_ARG), (labels[1], "")];
        let cats = [(labels[2].to_string(), pins), (labels[3].to_string(), recent)];
        if let Err(e) = rf_win::jumplist::set(&exe, &tasks, &cats) {
            // (not written: the next look at the places writes it, even when nothing has changed by then)
            if TURN.load(Relaxed) == turn {
                *LAST.lock().unwrap() = None;
            }
            // one warning in a run of the app: where the list cannot be written at all (a policy), every try would add a line
            if WARNED.swap(true, Relaxed) {
                log::debug!("jump list: {e:#}");
            } else {
                log::warn!("jump list: {e:#}");
            }
        }
    });
}

/// How long the jump list waits after a change of its content before it is written (see `update_jump_list`).
const JUMP_LIST_SETTLE: std::time::Duration = std::time::Duration::from_millis(400);

/// Settings, asked for from outside a window (DirectLook's tray icon, the tray menu, a start with `--settings=<page>`): the window
/// that has Settings open comes forward, on that tab; a new window is opened only when none has. WHY (issue 33: "... opens the
/// Settings window multiple times (up to 5)"): every such request opened another window, so the five messages a tray sends for a
/// double click made five windows. 0.8.2 made those five one request; with this a second request is harmless whoever sends it.
/// Says whether the request is taken care of (see `route_ipc_shown`).
pub fn show_settings(cx: &mut App) -> bool {
    let list = cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default();
    for (handle, ws) in list.into_iter().rev() {
        let Some(ws) = ws.upgrade() else { continue };
        let tab = {
            let w = ws.read(cx);
            // (an Open/Save picker is another app's dialog, as in `route_ipc_shown`)
            if w.picker.is_some() { None } else { w.tabs.iter().position(|t| matches!(t.view().read(cx).location(), Location::Settings)) }
        };
        let Some(tab) = tab else { continue };
        let shown = handle
            .update(cx, |_, window, cx| {
                ws.update(cx, |w, cx| w.activate(tab, window, cx));
                window.activate_window();
            })
            .is_ok();
        if shown {
            return true;
        }
    }
    crate::open_main_window_made(Some(Location::Settings), None, None, cx)
}

/// Whether `path` lives on drive `letter` (`E:\…`, `\\?\E:\…`).
pub fn on_drive(path: &std::path::Path, letter: char) -> bool {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let mut c = s.chars();
    matches!((c.next(), c.next()), (Some(l), Some(':')) if l.eq_ignore_ascii_case(&letter))
}

/// Windows asks to eject `letter` ("Safely remove", eject from our sidebar or Explorer): every
/// window lets go of it — panes on that drive go Home (their folder watchers, size walks and
/// listings stop), its previews close, queued thumbnails are dropped.
pub fn release_drive(letter: char, cx: &mut App) {
    let list = cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default();
    for (handle, ws) in list {
        let Some(ws) = ws.upgrade() else { continue };
        let _ = handle.update(cx, |_, _, cx| ws.update(cx, |w, cx| w.release_drive(letter, cx)));
    }
}

/// Whether any app window (not a menu / flyout popup) is still open.
/// The open main windows (not Settings, DirectLook or popups).
pub fn main_windows(cx: &App) -> Vec<AnyWindowHandle> {
    let open = cx.windows();
    cx.try_global::<OpenWorkspaces>()
        .map(|g| open.into_iter().filter(|w| g.0.iter().any(|(h, ws)| ws.upgrade().is_some() && w.window_id() == h.window_id())).collect())
        .unwrap_or_default()
}

pub fn any_main_window(cx: &App) -> bool {
    let open = cx.windows();
    cx.try_global::<OpenWorkspaces>()
        .is_some_and(|g| g.0.iter().any(|(h, ws)| ws.upgrade().is_some() && open.iter().any(|w| w.window_id() == h.window_id())))
}

/// A second launch's message that asks for a file to be shown selected in its folder
/// (`rustyfiles /select,<file>`, like Explorer).
pub const SELECT_PREFIX: &str = "\u{1}select:";

/// A file to select in the window that just opened its folder ("Show in folder" from another app,
/// see rf_win::shell_window): selected in place, or its folder opened when no window shows it.
pub const SELECT_HERE_PREFIX: &str = "\u{1}select-here:";

/// A second launch on a shell folder (a phone, Network…) reaches the running window as this
/// prefix plus the folder's parsing name (and, in a saved session, a tab and its name).
pub const SHELL_PREFIX: &str = "rustyfiles-shell:";

/// How a tab is kept in the saved session: a folder's path, a search's folder, the Recycle Bin
/// and shell folders (a phone…) by name; other places come back as Home ("").
fn session_text(loc: &Location) -> String {
    match loc {
        Location::Path(p) => rf_core::history::redact_credentials(&p.to_string_lossy()),
        Location::Search { root, .. } => root.to_string_lossy().into_owned(),
        Location::RecycleBin => "shell:RecycleBinFolder".into(),
        Location::Downloads => "rustyfiles:downloads".into(),
        Location::Shell { parsing, name } => format!("{SHELL_PREFIX}{parsing}\t{name}"),
        _ => String::new(),
    }
}

/// What the session keeps of the tabs: the left pane of each, and for the tabs that are split the right pane with whether it had
/// the focus. `tabs` is each tab's left place and, when it is split, its right place and that flag.
fn session_record(tabs: &[(Location, Option<(Location, bool)>)]) -> (Vec<String>, Vec<crate::settings::SessionPane>) {
    let left = tabs.iter().map(|(l, _)| session_text(l)).collect();
    let panes = tabs
        .iter()
        .enumerate()
        .filter_map(|(tab, (_, right))| right.as_ref().map(|(loc, active)| crate::settings::SessionPane { tab, location: session_text(loc), active: *active }))
        .collect();
    (left, panes)
}

/// Where a place written by [`session_text`] leads back to.
fn session_place(text: &str) -> Location {
    if text.is_empty() { Location::Home } else { parse_launch(text).0 }
}

/// The second panes to open again when `tabs` tabs came back: for each tab that exists the first pane the record names for it (a
/// record from a session with more tabs, or one edited by hand, must not open a pane for a tab that is not there or two for one).
fn panes_to_restore(panes: &[crate::settings::SessionPane], tabs: usize) -> Vec<(usize, Location, bool)> {
    let mut seen = std::collections::BTreeSet::new();
    panes.iter().filter(|p| p.tab < tabs && seen.insert(p.tab)).map(|p| (p.tab, session_place(&p.location), p.active)).collect()
}

/// What tells that the session has to be written again: the places of all panes (not which pane has the focus: a click from one
/// pane to the other is not worth a write of the settings file; the focus is written when the places change and at exit).
fn session_key(left: &[String], panes: &[crate::settings::SessionPane]) -> Vec<String> {
    left.iter().cloned().chain(panes.iter().map(|p| format!("\u{1}{}\u{1}{}", p.tab, p.location))).collect()
}

/// What a launch message opens: a folder (or Home), and maybe a name to select there.
pub fn parse_launch(msg: &str) -> (Location, Option<String>) {
    let m = msg.trim();
    if let Some(rest) = m.strip_prefix(SHELL_PREFIX) {
        let (parsing, name) = rest.split_once('\t').unwrap_or((rest, ""));
        return (Location::Shell { parsing: parsing.to_string(), name: name.to_string() }, None);
    }
    if let Some(loc) = Location::from_shell_name(m) {
        return (loc, None);
    }
    if let Some(file) = m.strip_prefix(SELECT_PREFIX) {
        let p = PathBuf::from(file);
        if let (Some(dir), Some(name)) = (p.parent(), p.file_name()) {
            return (Location::Path(dir.to_path_buf()), Some(name.to_string_lossy().into_owned()));
        }
    }
    if m.is_empty() { (Location::Home, None) } else { (Location::Path(PathBuf::from(m)), None) }
}

/// DirectLook asked for the next or the previous item (an arrow key in its window): the list of the newest window moves its
/// selection by one and DirectLook is sent the item now selected. Only while DirectLook was opened from our list.
fn step_quick_look(forward: bool, cx: &mut App) {
    if !crate::folder_view::QUICK_LOOK_OPEN.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let list = cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default();
    for (handle, ws) in list.into_iter().rev() {
        let Some(ws) = ws.upgrade() else { continue };
        if ws.read(cx).picker.is_some() {
            continue;
        }
        let done = handle
            .update(cx, |_, _, cx| {
                let view = ws.read(cx).active_tab().clone();
                view.update(cx, |v, cx| v.step_selection(forward, cx));
                view.read(cx).preview_target()
            })
            .ok();
        if let Some(target) = done {
            if let Some((path, e)) = target {
                *LOOK_FOLLOWED.lock().unwrap() = Some(path.clone());
                crate::archive_view::look_send(path, e.size, true);
            }
            return;
        }
    }
}

/// Writes every open main window's session and placement into the settings, as closing a window does (a restart quits without
/// closing them, and the new process opened at the place the window had at its LAST close: seen in a test copy).
pub fn save_all_sessions(cx: &mut App) {
    let list = cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default();
    for (handle, ws) in list {
        let Some(ws) = ws.upgrade() else { continue };
        if ws.read(cx).picker.is_some() {
            continue;
        }
        let _ = handle.update(cx, |_, window, cx| {
            ws.update(cx, |w, cx| w.save_session(window, cx));
        });
    }
}

/// A saved session waiting for the window that opens it (`open_named_session` -> `Workspace::new`).
static PENDING_NAMED: std::sync::Mutex<Option<crate::settings::NamedSession>> = std::sync::Mutex::new(None);

/// Opens the session saved as `name` in a new window.
pub fn open_named_session(name: &str, cx: &mut App) {
    let Some(session) = cx.global::<crate::settings::Settings>().saved_sessions.iter().find(|s| s.name == name).cloned() else { return };
    *PENDING_NAMED.lock().unwrap() = Some(session);
    crate::open_main_window(None, None, None, cx);
    // (a window that could not open leaves nothing waiting for the next one)
    PENDING_NAMED.lock().unwrap().take();
}

/// The saved sessions after saving `session`: one of the same name is replaced where it stood, a new name goes at the end.
fn remember_named(list: &mut Vec<crate::settings::NamedSession>, session: crate::settings::NamedSession) {
    match list.iter_mut().find(|s| s.name.eq_ignore_ascii_case(&session.name)) {
        Some(old) => *old = session,
        None => list.push(session),
    }
}

/// Selects `file` in the newest window whose current tab shows its folder (and brings that window
/// forward); false when none does.
fn select_in_open_folder(file: &std::path::Path, cx: &mut App) -> bool {
    let (Some(dir), Some(name)) = (file.parent(), file.file_name()) else { return false };
    let key = |p: &std::path::Path| p.to_string_lossy().trim_end_matches(['\\', '/']).to_lowercase();
    let want = key(dir);
    let name = name.to_string_lossy().into_owned();
    let list = cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default();
    for (handle, ws) in list.into_iter().rev() {
        let Some(ws) = ws.upgrade() else { continue };
        let view = ws.read(cx).active_view();
        let here = matches!(view.read(cx).location(), Location::Path(p) if key(p) == want);
        if !here {
            continue;
        }
        let name = name.clone();
        if handle
            .update(cx, |_, window, cx| {
                view.update(cx, |v, cx| v.select_now(name, cx));
                window.activate_window();
            })
            .is_ok()
        {
            return true;
        }
    }
    false
}

/// The `/select,` file of the first launch, picked up by the first window.
pub static START_SELECT: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// DirectLook'a en son gönderilen dosya (seçim değişince yalnız yenisi gider).
static LOOK_FOLLOWED: std::sync::Mutex<Option<PathBuf>> = std::sync::Mutex::new(None);

/// Whether any non-picker main browser window is currently open.
fn has_browser_window(cx: &App) -> bool {
    let open = cx.windows();
    cx.try_global::<OpenWorkspaces>().is_some_and(|g| {
        g.0.iter().any(|(h, ws)| {
            ws.upgrade().is_some_and(|w| w.read(cx).picker.is_none())
                && open.iter().any(|w| w.window_id() == h.window_id())
        })
    })
}

/// Start location for a new window opened from an IPC launch when no existing window took it:
/// `None` restores the session (or startup pages) when no browser window is open and no target
/// was given, so reopening the app from background mode behaves like a fresh start.
fn launch_window_start(msg: &str, has_browser_window: bool, loc: Location) -> Option<Location> {
    if msg.trim().is_empty() && !has_browser_window { None } else { Some(loc) }
}

pub fn route_ipc(msg: String, cx: &mut App) {
    route_ipc_shown(msg, cx);
}

/// [`route_ipc`], saying whether the request is taken care of: false only when it needed a window and none could be made. The
/// launch of a second process waits for this answer (see `rf_win::handover`) and shows the folder itself on false.
pub fn route_ipc_shown(msg: String, cx: &mut App) -> bool {
    // the dock's file search asks this app over the pipe (the dock does not hold the name index), and a window-less start greets it
    if crate::search_pipe::handle_private(&msg, cx.global::<crate::settings::Settings>().name_index) {
        return true;
    }
    if let Some(pipe) = msg.strip_prefix(crate::picker::PICK_PREFIX) {
        crate::picker::begin(pipe.to_string(), cx);
        return true;
    }
    match crate::look::ipc::ToApp::decode(&msg) {
        Some(crate::look::ipc::ToApp::LookClosed) => {
            crate::folder_view::QUICK_LOOK_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
            *LOOK_FOLLOWED.lock().unwrap() = None;
            return true;
        }
        Some(crate::look::ipc::ToApp::Step(forward)) => {
            step_quick_look(forward, cx);
            return true;
        }
        Some(crate::look::ipc::ToApp::OpenSettings) => {
            crate::settings_view::open_page(crate::settings_view::Page::Look, cx);
            return show_settings(cx);
        }
        None => {}
    }
    if let Some(file) = msg.strip_prefix(SELECT_HERE_PREFIX) {
        if !select_in_open_folder(std::path::Path::new(file), cx) {
            return route_ipc_shown(format!("{SELECT_PREFIX}{file}"), cx);
        }
        return true;
    }
    if msg.trim() == rf_win::jumplist::NEW_WINDOW_ARG {
        return crate::open_main_window_made(Some(Location::Home), None, None, cx);
    }
    if msg.trim() == crate::downloads_view::LAUNCH_ARG {
        crate::downloads_view::open_window(cx);
        return true;
    }
    if msg.trim() == crate::quick_search::LAUNCH_ARG {
        crate::quick_search::toggle(cx);
        return true;
    }
    // a download the browser handed to HyperDownload: a card of the Status Center
    if let Some(gid) = msg.trim().strip_prefix(crate::hyperdownload::FOLLOW_PREFIX) {
        crate::hyperdownload::follow(gid.to_string(), cx);
        return true;
    }
    crate::shell::desktop::open::mark("main: the line to open a place arrived");
    let (loc, select) = parse_launch(&msg);
    // (with "open folders from other programs in a new window" on, no window takes it as a tab: issue 77)
    let list = if cx.global::<crate::settings::Settings>().launch_in_new_window {
        Vec::new()
    } else {
        cx.try_global::<OpenWorkspaces>().map(|g| g.0.clone()).unwrap_or_default()
    };
    for (handle, ws) in list.into_iter().rev() {
        let Some(ws) = ws.upgrade() else { continue };
        // an Open/Save picker is another app's dialog: a launch never becomes one of its tabs
        // (with only a picker open, rustyfiles started from the Start menu opened in it)
        if ws.read(cx).picker.is_some() {
            continue;
        }
        let (target, sel) = (loc.clone(), select.clone());
        let ok = handle
            .update(cx, |_, window, cx| {
                ws.update(cx, |w, cx| w.open_tab_selecting(target, sel, window, cx));
                window.activate_window();
            })
            .is_ok();
        if ok {
            crate::shell::desktop::open::mark("main: the place is open in a new tab and the window asked to come forward");
            return true;
        }
    }
    *START_SELECT.lock().unwrap() = select;
    let start = launch_window_start(&msg, has_browser_window(cx), loc);
    let made = crate::open_main_window_made(start, None, None, cx);
    crate::shell::desktop::open::mark("main: a new window was made for the place");
    made
}

/// Puts a search at the front of the remembered ones (once, whatever its case; ten are kept).
fn remember_search(history: &mut Vec<String>, q: &str) {
    let q = q.trim();
    if q.is_empty() {
        return;
    }
    history.retain(|h| !h.eq_ignore_ascii_case(q) && rf_core::entry::fold_case(h) != rf_core::entry::fold_case(q));
    history.insert(0, q.to_string());
    history.truncate(10);
}

/// The folder a tab's terminal button would open, if the tab has one: a real disk folder,
/// not an archive, not FTP or a network share. Called for every tab on every frame, so it
/// looks at the path only and never asks the disk (a stat of a share that went away blocks
/// the UI thread for seconds); a folder merely named like an archive goes without the button.
pub(crate) fn tab_terminal_dir(loc: &Location) -> Option<PathBuf> {
    let Location::Path(p) = loc else { return None };
    (!rf_win::ftp::is_ftp(p) && rf_win::network::server_root(p).is_none() && !crate::nav_touch::may_hold_archive(p))
        .then(|| p.clone())
}

impl Workspace {
    pub fn new(
        start: Option<Location>,
        ipc: Option<futures::channel::mpsc::UnboundedReceiver<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // a press on a file leaves the window behind until it is not a drag, as in Explorer (issue 169)
        rf_win::press_activate::install(crate::look::window::hwnd_of(window));
        if let Some(mut rx) = ipc {
            cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt;
                while let Some(msg) = rx.next().await {
                    let (loc, select) = parse_launch(&msg);
                    let ok = this
                        .update_in(cx, |ws, window, cx| {
                            ws.open_tab_selecting(loc, select, window, cx);
                            window.activate_window();
                        })
                        .is_ok();
                    if !ok {
                        break;
                    }
                }
            })
            .detach();
        }
        let places = cx.new(Places::new);
        let images = cx.new(ImageCache::new);
        cx.observe(&images, |_, _, cx| cx.notify()).detach();
        let ops = cx.global::<crate::status_center::OpsGlobal>().0.clone();
        cx.observe(&ops, |_, _, cx| cx.notify()).detach();
        let preview = {
            let images = images.clone();
            cx.new(|cx| Preview::new(images, cx))
        };
        cx.observe(&places, |_, places, cx| {
            update_jump_list(places.read(cx));
            cx.notify()
        })
        .detach();
        let address = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Yol girin")));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Ara")));
        let filter_input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Dosya adı")));

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&address, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { .. } => {
                let from_completion = this.addr_sel.and_then(|i| this.addr_suggest.get(i)).is_some();
                let text = match this.addr_sel.and_then(|i| this.addr_suggest.get(i)) {
                    Some(p) => p.to_string_lossy().into_owned(),
                    None => input.read(cx).value().to_string(),
                };
                this.clear_suggestions();
                this.editing_address = false;
                if text.trim().to_lowercase().starts_with("shell:") {
                    // resolve in the background (shell namespace), then navigate
                    let t = text.trim().to_string();
                    let work = cx.background_spawn(async move { rf_win::shell::resolve_shell_path(&t) });
                    cx.spawn_in(window, async move |this, cx| {
                        if let Some(p) = work.await {
                            let _ = this.update_in(cx, |ws, window, cx| ws.navigate(Location::Path(p), window, cx));
                        }
                    })
                    .detach();
                    this.focus_active(window, cx);
                    cx.notify();
                    return;
                }
                // a place as always; with the setting on, text that names no place is tried as a program (address_cmd.rs)
                this.enter_address_text(text, from_completion, window, cx);
            }
            InputEvent::Blur => {
                this.editing_address = false;
                this.clear_suggestions();
                cx.notify();
            }
            InputEvent::Change => {
                // the highlight belongs to the previous text
                this.addr_sel = None;
                if this.editing_address {
                    let text = input.read(cx).value().to_string();
                    this.update_suggestions(text, cx);
                }
            }
            _ => {}
        }));
        subs.push(cx.subscribe_in(&filter_input, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::Change => {
                let q = input.read(cx).value().to_string();
                if let Some(tab) = this.tabs.get(this.active).map(|t| t.view().clone()) {
                    tab.update(cx, |v, cx| v.set_query(q, cx));
                }
            }
            InputEvent::PressEnter { .. } => this.focus_active(window, cx),
            _ => {}
        }));
        subs.push(cx.subscribe_in(&search, window, |this, input, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                // Enter: search this folder and all subfolders.
                let q = input.read(cx).value().trim().to_string();
                this.run_search(q, window, cx);
                return;
            }
            if let InputEvent::Change = ev {
                let q = input.read(cx).value().to_string();
                if let Some(tab) = this.tabs.get(this.active).map(|t| t.view().clone()) {
                    tab.update(cx, |v, cx| v.set_query(q, cx));
                }
            }
            // clicking elsewhere: the Omnibar shows the path again (the query stays as a chip)
            if let InputEvent::Blur = ev {
                if this.search_mode {
                    this.search_mode = false;
                    cx.notify();
                }
            }
        }));

        // Repaint the status-bar renderer badge when the hybrid renderer switches devices.
        cx.spawn(async move |this, cx| {
            let mut last = (gpui_windows::hybrid::switch_count(), gpui_windows::hybrid::mode());
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
                let now = (gpui_windows::hybrid::switch_count(), gpui_windows::hybrid::mode());
                if now != last {
                    last = now;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();

        cx.observe_global::<crate::settings::Settings>(|this, cx| {
            // the language may have changed: the jump list follows
            update_jump_list(this.places.read(cx));
            // another window may have dragged the preview pane's height
            this.sync_preview_height(cx);
            let s = cx.global::<crate::settings::Settings>();
            // a file dialog keeps its own preview state (`picker_preview_open`)
            if this.picker.is_none() {
                this.preview_open = s.preview_open;
                this.sidebar_rail = s.sidebar_rail;
            }
            this.sidebar_open = s.sidebar_open;
            cx.notify();
        })
        .detach();
        cx.observe_global::<crate::tags::Tags>(|_, cx| cx.notify()).detach();
        // The system's light / dark switch reaches the window as an event only; nothing else redraws it. The palette and the glass's tint
        // (`set_dark`) are decided in `render`, so a window left alone would keep the old theme until the next click.
        cx.observe_window_appearance(window, |_: &mut Self, window, cx| {
            window.refresh();
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |_: &mut Self, _, cx| {
            if crate::settings::windows_file_explorer_theme() {
                cx.notify();
            }
        })
        .detach();
        // Keep the latest placement while the HWND is alive. Closing the native frame can
        // destroy it before GPUI is able to update the workspace from its should-close callback.
        remember_placement(window, cx);
        cx.observe_window_bounds(window, |this: &mut Self, window, cx| {
            if this.picker.is_none() {
                remember_placement(window, cx);
            }
            this.preview.update(cx, |pv, _| pv.invalidate_host());
            cx.notify();
        })
        .detach();
        let ws = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(ws) = ws.upgrade() {
                ws.update(cx, |w, cx| w.save_session(window, cx));
            }
            Self::release_preview_host(window);
            true
        });
        let prefs = cx.global::<crate::settings::Settings>().clone();
        {
            let entry = (window.window_handle(), cx.entity().downgrade());
            let list = &mut cx.default_global::<OpenWorkspaces>().0;
            list.retain(|(_, w)| w.upgrade().is_some());
            list.push(entry);
        }
        let shelf = cx.new(crate::shelf_view::ShelfView::new);
        let shelf_sub = cx.subscribe_in(&shelf, window, |this, _, ev, window, cx| this.on_shelf_event(ev, window, cx));

        let mut this = Self {
            tabs: Vec::new(),
            active: 0,
            closed: Vec::new(),
            places,
            images,
            ops,
            preview,
            quick_look: None,
            preview_open: prefs.preview_open,
            shelf,
            shelf_open: false,
            address,
            editing_address: false,
            search_mode: false,
            frame_dark: None,
            applied_backdrop: None,
            backdrop_pref_seen: None,
            glass_hwnd: 0,
            glass_dark: None,
            glass_failed: false,
            glass_shown: false,
            glass_capture: rf_win::glass::CaptureMode::Off,
            glass_gen: 0,
            sidebar_lens: false,
            sidebar_lens_claimed: std::cell::Cell::new(false),
            tab_lens: false,
            tab_lens_claimed: false,
            plus_lens_claimed: false,
            addr_suggest: Vec::new(),
            addr_sel: None,
            _addr_task: None,
            search,
            filter_bar: false,
            filter_input,
            sidebar_open: prefs.sidebar_open,
            compact: None,
            sidebar_w: if prefs.windows_file_explorer_theme {
                prefs.sidebar_width_for_explorer_look()
            } else {
                prefs.sidebar_width
            }
            .clamp(SIDEBAR_MIN, SIDEBAR_MAX),
            sidebar_width_explorer_mode: prefs.windows_file_explorer_theme,
            sidebar_rail: prefs.sidebar_rail,
            rail_peek: [false; 2],
            rail_peek_bg: None,
            rail_peek_open: false,
            rail_peek_closing: false,
            rail_peek_serial: 0,
            preview_w: prefs.preview_width.clamp(PREVIEW_MIN, PREVIEW_MAX),
            preview_dock: preview_dock::Dock::new(prefs.preview_height),
            _width_save: None,
            session_name: None,
            palette: None,
            bulk: None,
            last_title: String::new(),
            tab_scroll: ScrollHandle::new(),
            sidebar_scroll_hovered: false,
            tab_scrolled_to: None,
            tab_anim: None,
            tab_anim_serial: 0,
            drag_hover_tab: None,
            sidebar_ext_drag: false,
            side_tree: Default::default(),
            picker: None,
            picker_size_gen: 0,
            viewport_w: 1300.,
            crumb_room: Default::default(),
            hd_installed: crate::addons::installed(crate::hyperdownload::ID).is_some(),
            hd_seen: crate::addons::CHANGES.load(std::sync::atomic::Ordering::Relaxed),
            last_session: Vec::new(),
            _bulk_sub: None,
            _palette_sub: None,
            _subs: { let mut s = subs; s.push(shelf_sub); s },
        };
        match start {
            Some(loc) => {
                let select = START_SELECT.lock().unwrap().take();
                this.open_tab_selecting(loc, select, window, cx)
            }
            // a saved session asked for by name (`open_named_session`): its tabs, its split panes, its active tab
            None if PENDING_NAMED.lock().unwrap().is_some() => {
                let named = PENDING_NAMED.lock().unwrap().take().unwrap_or_default();
                this.session_name = Some(named.name.clone());
                for t in &named.tabs {
                    this.open_tab(session_place(t), window, cx);
                }
                if this.tabs.is_empty() {
                    this.open_tab(Location::Home, window, cx);
                }
                for (ix, loc, active) in panes_to_restore(&named.panes, this.tabs.len()) {
                    this.open_second_pane(ix, loc, active, window, cx);
                }
                let a = named.active.min(this.tabs.len() - 1);
                this.activate(a, window, cx);
            }
            None if prefs.startup == crate::settings::StartupPref::Continue && !prefs.session_tabs.is_empty() => {
                for t in &prefs.session_tabs {
                    this.open_tab(session_place(t), window, cx);
                }
                // the tabs that were split in two get their second pane back, at its own folder and with the focus where it was
                for (ix, loc, active) in panes_to_restore(&prefs.session_panes, this.tabs.len()) {
                    this.open_second_pane(ix, loc, active, window, cx);
                }
                let a = prefs.session_active.min(this.tabs.len() - 1);
                this.activate(a, window, cx);
            }
            None if prefs.startup == crate::settings::StartupPref::Pages && !prefs.startup_pages.is_empty() => {
                for t in &prefs.startup_pages {
                    let loc = parse_location(t).unwrap_or(Location::Home);
                    this.open_tab(loc, window, cx);
                }
                this.activate(0, window, cx);
            }
            None => this.open_tab(Location::Home, window, cx),
        }
        // The tabs a window opens WITH are simply there: the opening animation is for a tab that is opened in a window one is looking
        // at. WHY: `open_tab` started it for the first tab too, so every new window spent its first third of a second growing its
        // tab and sliding the "+" in (seen frame by frame, 2026-11-16: the folder is listed in the first picture, the tab strip moves
        // for 280 ms more), and the speed test, which waits for the picture to stand still, counted that as "not open yet".
        this.tab_anim = None;
        this
    }

    /// Open/Save dialog window: its answer, filter and session (see `picker`).
    /// The preview pane on/off: the saved setting, or only this window when it is a file dialog.
    fn toggle_preview(&mut self, cx: &mut Context<Self>) {
        let v = !self.preview_open;
        if self.picker.is_some() {
            self.preview_open = v;
            crate::settings::update(cx, |s| s.picker_preview_open = v);
            cx.notify();
        } else {
            crate::settings::update(cx, |s| s.preview_open = v);
        }
    }

    pub(crate) fn set_picker(&mut self, session: std::rc::Rc<crate::picker::PickSession>, window: &mut Window, cx: &mut Context<Self>) {
        let (patterns, folders_only) = (session.patterns(), session.folders_only());
        for t in &self.tabs {
            for v in std::iter::once(&t.left).chain(t.right.as_ref()) {
                v.update(cx, |v, cx| v.set_picking(patterns.clone(), folders_only, cx));
            }
        }
        self.picker = Some(session);
        cx.default_global::<MainPlacements>().0.remove(&window.window_handle().window_id());
        // the dialog's size is kept once the user resized it, a moment after the last change (issue 89)
        cx.observe_window_bounds(window, |this: &mut Self, window, cx| {
            let s = window.bounds().size;
            let want = [f32::from(s.width).round() as u16, f32::from(s.height).round() as u16];
            this.picker_size_gen += 1;
            let generation = this.picker_size_gen;
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
                let _ = this.update(cx, |this, cx| {
                    if this.picker_size_gen == generation && cx.global::<crate::settings::Settings>().picker_size != Some(want) {
                        crate::settings::update(cx, |s| s.picker_size = Some(want));
                    }
                });
            })
            .detach();
        })
        .detach();
        // the preview pane helps picking (Windows' dialogs have one too): on until it is closed in a dialog, which the next dialogs
        // remember (issue 87); the sidebar starts as a strip of icons (it opens over the list while hovered)
        self.preview_open = cx.global::<crate::settings::Settings>().picker_preview_open;
        self.sidebar_rail = true;
        // closing the window (X, Alt+F4) cancels; it is not the user's session
        let ws = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            if let Some(ws) = ws.upgrade() {
                ws.update(cx, |ws, _| crate::picker::answer_cancel(ws));
            }
            Self::release_preview_host(window);
            true
        });
        cx.notify();
    }

    pub(crate) fn picker(&self) -> Option<std::rc::Rc<crate::picker::PickSession>> {
        self.picker.clone()
    }

    pub(crate) fn active_view(&self) -> Entity<FolderView> {
        self.active_tab().clone()
    }

    /// Open/Save dialog: another file type chosen (1-based).
    pub(crate) fn set_pick_filter(&mut self, ix: u32, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(pk) = self.picker.clone() else { return };
        pk.filter_ix.set(ix);
        let patterns = pk.patterns();
        for t in &self.tabs {
            for v in std::iter::once(&t.left).chain(t.right.as_ref()) {
                v.update(cx, |v, cx| v.set_picking(patterns.clone(), pk.folders_only(), cx));
            }
        }
        cx.notify();
    }

    fn save_session(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.picker.is_some() {
            return;
        }
        let placement = MainPlacement::capture(window, cx);
        if let Some(placement) = &placement {
            cx.default_global::<MainPlacements>().0.insert(window.window_handle().window_id(), placement.clone());
        }
        let (tabs, panes) = self.session_now(cx);
        let active = self.active;
        cx.update_global::<crate::settings::Settings, _>(|s, _| {
            s.session_tabs = tabs;
            s.session_panes = panes;
            s.session_active = active;
            if let Some(placement) = placement {
                placement.apply(s);
            }
        });
        // synchronous: the process is about to exit
        cx.global::<crate::settings::Settings>().save();
    }

    /// The tabs as the session keeps them, now ([`session_record`]).
    fn session_now(&self, cx: &App) -> (Vec<String>, Vec<crate::settings::SessionPane>) {
        let tabs: Vec<(Location, Option<(Location, bool)>)> =
            self.tabs.iter().map(|t| (t.left.read(cx).location().clone(), t.right.as_ref().map(|r| (r.read(cx).location().clone(), t.right_active)))).collect();
        session_record(&tabs)
    }

    /// Gives tab `ix` its second pane at `loc` (a tab of the last session that was split); `active`: that pane has the focus.
    fn open_second_pane(&mut self, ix: usize, loc: Location, active: bool, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() || self.tabs[ix].right.is_some() {
            return;
        }
        let view = self.new_view(loc, window, cx);
        view.update(cx, |v, _| v.dual = true);
        self.tabs[ix].left.update(cx, |v, _| v.dual = true);
        self.tabs[ix].right = Some(view);
        self.tabs[ix].right_active = active;
    }

    fn active_tab(&self) -> &Entity<FolderView> {
        self.tabs[self.active].view()
    }

    /// Tab (`forward`) and Shift+Tab: the keyboard goes round the window's panes, the address bar, the folder tree (when it is on
    /// screen) and the file list, as Explorer's Tab does (issue 38: "TAB does not work anywhere"). A rename in the list keeps it.
    fn cycle_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.active_tab().read(cx).renaming.is_some() {
            return;
        }
        let tree = self.tree_focus(cx);
        let here = if self.editing_address {
            Pane::Address
        } else if tree.as_ref().is_some_and(|f| f.is_focused(window)) {
            Pane::Tree
        } else {
            Pane::List
        };
        match next_pane(here, forward, tree.is_some()) {
            Pane::Address => self.begin_address_edit(window, cx),
            to => {
                if self.editing_address {
                    self.editing_address = false;
                    self.clear_suggestions();
                }
                // (the pane the keyboard comes to shows where it is: its row in view, the first one when none was chosen; issue 38)
                match (to, tree) {
                    (Pane::Tree, Some(f)) => {
                        window.focus(&f, cx);
                        self.tree_keyboard_entered(cx);
                    }
                    _ => {
                        self.focus_active(window, cx);
                        self.active_tab().clone().update(cx, |v, cx| v.keyboard_entered(cx));
                    }
                }
                cx.notify();
            }
        }
    }

    fn focus_active(&self, window: &mut Window, cx: &mut Context<Self>) {
        let fh = self.active_tab().read(cx).focus.clone();
        window.focus(&fh, cx);
    }

    fn new_view(&mut self, loc: Location, window: &mut Window, cx: &mut Context<Self>) -> Entity<FolderView> {
        let places = self.places.clone();
        let images = self.images.clone();
        let tab = cx.new(|cx| FolderView::new(loc, places, images, cx));
        if let Some(pk) = self.picker.clone() {
            tab.update(cx, |v, cx| v.set_picking(pk.patterns(), pk.folders_only(), cx));
        }
        let sub = cx.subscribe_in(&tab, window, |this, emitter, ev: &FolderEvent, window, cx| match ev {
            FolderEvent::Picked(paths) => crate::picker::picked(this, paths.clone(), window, cx),
            FolderEvent::SidebarMenu(path) => this.sidebar_menu(path.clone(), window, cx),
            FolderEvent::ToOtherPane(kind, paths) => {
                if let Some(other) = this.other_pane(emitter) {
                    let dest = match other.read(cx).location() {
                        Location::Path(p) if !rf_win::ftp::is_ftp(p) && rf_win::network::server_root(p).is_none() => Some(p.clone()),
                        _ => None,
                    };
                    if let Some(dest) = dest {
                        crate::status_center::transfer(cx, *kind, paths.clone(), dest);
                    }
                }
            }
            FolderEvent::OpenInOtherPane(loc) => {
                if let Some(other) = this.other_pane(emitter) {
                    other.update(cx, |v, cx| v.navigate(loc.clone(), cx));
                }
            }
            FolderEvent::OpenInNewTab(loc) => {
                let keep = this.active;
                this.open_tab(loc.clone(), window, cx);
                // a view asked for by name (disk usage) is what the user wants to see now
                let wanted = matches!(loc, Location::DiskUsage(_));
                if !wanted && !cx.global::<crate::settings::Settings>().focus_new_tab {
                    this.activate(keep, window, cx);
                }
            }
            FolderEvent::OpenInNewWindow(loc) => {
                let loc = loc.clone();
                let mut b = window.bounds();
                b.origin.x += px(36.);
                b.origin.y += px(36.);
                cx.defer(move |cx| crate::open_main_window(Some(loc), None, Some(WindowBounds::Windowed(b)), cx));
            }
            FolderEvent::QuickLook => this.toggle_quick_look(cx),
            FolderEvent::StartFilter(text) => {
                // the first letter opens the bar with it; the next ones are typed into the bar, which has the focus now
                let q = format!("{}{text}", emitter.read(cx).filter.query);
                this.filter_bar = true;
                this.filter_input.update(cx, |s, cx| {
                    s.set_value(q.clone(), window, cx);
                    s.focus(window, cx);
                });
                emitter.update(cx, |v, cx| v.set_query(q, cx));
                cx.notify();
            }
            FolderEvent::NeedPassword(archive, wrong) => {
                let view = emitter.downgrade();
                crate::archive_view::ask_password(archive.clone(), *wrong, window, cx, move |cx| {
                    let _ = view.update(cx, |v, cx| v.refresh(cx));
                });
            }
            FolderEvent::BulkRename(paths) => this.open_bulk_rename(paths.clone(), window, cx),
            FolderEvent::AddToShelf(paths) => {
                let paths = paths.clone();
                this.shelf.update(cx, |s, cx| {
                    s.add(paths, cx);
                });
                this.shelf_open = true;
                cx.notify();
            }
        });
        self._subs.push(sub);
        cx.observe(&tab, |_, _, cx| cx.notify()).detach();
        tab
    }

    /// `open_tab`, then selects `select` (a name in that folder) once it is listed.
    pub fn open_tab_selecting(&mut self, loc: Location, select: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.open_tab(loc, window, cx);
        if let Some(name) = select {
            self.active_tab().update(cx, |v, _| v.set_reselect(name));
        }
    }

    pub fn open_tab(&mut self, loc: Location, window: &mut Window, cx: &mut Context<Self>) {
        let view = self.new_view(loc, window, cx);
        self.tabs.push(Tab { left: view.clone(), right: None, right_active: false });
        let ix = self.tabs.len() - 1;
        // Settings → Appearance → Animations: a new tab grows into the strip. Purely visual:
        // the tab is already open and usable.
        let animate = crate::settings::animations();
        cx.set_reduce_motion(!animate);
        if animate {
            self.tab_anim_serial += 1;
            let serial = self.tab_anim_serial;
            self.tab_anim = Some((view, serial));
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(std::time::Duration::from_millis(TAB_ANIM_MS + PLUS_ANIM_MS + 100)).await;
                let _ = this.update(cx, |ws, cx| {
                    if ws.tab_anim.as_ref().is_some_and(|(_, s)| *s == serial) {
                        ws.tab_anim = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
        self.activate(ix, window, cx);
        // Settings → "Open new tabs in dual pane"
        if cx.global::<crate::settings::Settings>().dual_pane_new_tabs && self.tabs[ix].left.read(cx).location().path().is_some() {
            self.toggle_dual_pane(window, cx);
        }
    }

    /// Files are being dragged; `over` tells whether the pointer is on tab `i`. Held on an
    /// inactive tab for a moment, that tab comes forward so the files can be dropped into it.
    fn drag_over_tab(&mut self, i: usize, over: bool, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.drag_hover_tab.map(|(t, _)| t);
        if !over {
            if current == Some(i) {
                self.drag_hover_tab = None;
            }
            return;
        }
        if i == self.active || current == Some(i) {
            return;
        }
        self.tab_anim_serial += 1;
        let serial = self.tab_anim_serial;
        self.drag_hover_tab = Some((i, serial));
        cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(DRAG_HOVER_MS)).await;
            let _ = this.update_in(cx, |ws, window, cx| {
                // still on the same tab and still dragging
                if ws.drag_hover_tab == Some((i, serial)) && cx.has_active_drag() && i < ws.tabs.len() {
                    ws.drag_hover_tab = None;
                    ws.activate(i, window, cx);
                }
            });
        })
        .detach();
    }

    /// Opens/closes the second pane of the current tab (it starts at the same folder).
    fn toggle_dual_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self.active;
        if self.tabs[ix].right.is_some() {
            self.tabs[ix].right = None;
            self.tabs[ix].right_active = false;
            self.tabs[ix].left.update(cx, |v, _| v.dual = false);
        } else {
            let loc = self.tabs[ix].left.read(cx).location().clone();
            let view = self.new_view(loc, window, cx);
            view.update(cx, |v, _| v.dual = true);
            self.tabs[ix].left.update(cx, |v, _| v.dual = true);
            self.tabs[ix].right = Some(view);
            self.tabs[ix].right_active = true;
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Names the pane that owns the settings page's lens: the focused pane of the shown tab when `owner`, else none.
    fn assign_settings_lens(&self, owner: bool, cx: &mut Context<Self>) {
        let tab = &self.tabs[self.active];
        let focused = tab.view().clone();
        for v in std::iter::once(&tab.left).chain(tab.right.as_ref()) {
            let on = owner && *v == focused;
            if v.read(cx).lens_owner != on {
                v.update(cx, |v, _| v.lens_owner = on);
            }
        }
    }

    /// The pane next to `view` when its tab is split.
    fn other_pane(&self, view: &Entity<FolderView>) -> Option<Entity<FolderView>> {
        self.tabs.iter().find_map(|t| match &t.right {
            Some(r) if r == view => Some(t.left.clone()),
            Some(r) if &t.left == view => Some(r.clone()),
            _ => None,
        })
    }

    fn set_active_pane(&mut self, right: bool, cx: &mut Context<Self>) {
        let t = &mut self.tabs[self.active];
        if t.right.is_some() && t.right_active != right {
            t.right_active = right;
            cx.notify();
        }
    }

    fn activate(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if ix >= self.tabs.len() {
            return;
        }
        self.active = ix;
        let q = self.active_tab().read(cx).filter.query.clone();
        self.search.update(cx, |s, cx| s.set_value(q, window, cx));
        self.editing_address = false;
        self.focus_active(window, cx);
        cx.notify();
    }

    fn close_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        // the active index may stay the same while the tab under it changes: re-scroll
        self.tab_scrolled_to = None;
        if self.tabs.len() <= 1 {
            self.save_session(window, cx);
            Self::release_preview_host(window);
            window.remove_window();
            return;
        }
        let tab = self.tabs.remove(ix);
        self.closed.push(tab.view().read(cx).location().clone());
        let next = if self.active > ix || self.active >= self.tabs.len() {
            self.active.saturating_sub(1)
        } else {
            self.active
        };
        self.activate(next.min(self.tabs.len() - 1), window, cx);
    }

    /// Opens the saved session `name` in this window: its tabs (with their split panes and its active tab) take the place of the ones
    /// that were open, which go to the closed tabs (Ctrl+Shift+T brings them back).
    fn take_named_session(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(named) = cx.global::<crate::settings::Settings>().saved_sessions.iter().find(|s| s.name == name).cloned() else { return };
        let old = self.tabs.len();
        for t in &named.tabs {
            self.open_tab(session_place(t), window, cx);
        }
        if self.tabs.len() == old {
            // (a session with no tab leaves the window as it is)
            return;
        }
        for (ix, loc, active) in panes_to_restore(&named.panes, self.tabs.len() - old) {
            self.open_second_pane(old + ix, loc, active, window, cx);
        }
        for i in (0..old).rev() {
            let tab = self.tabs.remove(i);
            self.closed.push(tab.view().read(cx).location().clone());
        }
        self.tab_scrolled_to = None;
        self.session_name = Some(named.name.clone());
        let a = named.active.min(self.tabs.len() - 1);
        self.activate(a, window, cx);
    }

    fn close_other_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let keep = self.active;
        for i in (0..self.tabs.len()).rev() {
            if i != keep {
                let tab = self.tabs.remove(i);
                self.closed.push(tab.view().read(cx).location().clone());
            }
        }
        self.tab_scrolled_to = None;
        self.activate(0, window, cx);
    }

    /// A new tab asked for by the user (Ctrl+T, "+", the tab menu): Home, or the folder of the tab in front when the setting says so.
    fn open_new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let here = (cx.global::<crate::settings::Settings>().new_tab_opens == crate::settings::NewTabOpens::Current)
            .then(|| self.tabs.get(self.active).map(|t| t.view().read(cx).location().clone()))
            .flatten();
        self.open_tab(here.unwrap_or(Location::Home), window, cx);
    }

    /// Closes every tab and leaves one at Home (the closed ones can be reopened, Ctrl+Shift+T).
    fn close_all_tabs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let old = self.tabs.len();
        self.open_tab(Location::Home, window, cx);
        // (the new tab is the last one)
        for i in (0..old.min(self.tabs.len().saturating_sub(1))).rev() {
            let tab = self.tabs.remove(i);
            self.closed.push(tab.view().read(cx).location().clone());
        }
        self.tab_scrolled_to = None;
        self.activate(0, window, cx);
    }

    fn close_tabs_to_side(&mut self, ix: usize, right: bool, window: &mut Window, cx: &mut Context<Self>) {
        let range: Vec<usize> = if right { (ix + 1..self.tabs.len()).collect() } else { (0..ix).collect() };
        for i in range.into_iter().rev() {
            let tab = self.tabs.remove(i);
            self.closed.push(tab.view().read(cx).location().clone());
        }
        let ix = if right { ix } else { 0 };
        self.tab_scrolled_to = None;
        self.activate(ix.min(self.tabs.len() - 1), window, cx);
    }

    fn move_tab(&mut self, from: usize, to: usize, window: &mut Window, cx: &mut Context<Self>) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let active_left = self.tabs[self.active].left.clone();
        let t = self.tabs.remove(from);
        self.tabs.insert(to, t);
        self.active = self.tabs.iter().position(|t| t.left == active_left).unwrap_or(0);
        let _ = window;
        cx.notify();
    }

    /// Asks for a name and saves this window's tabs under it (`settings::NamedSession`); a name already taken is replaced.
    fn save_named_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (tabs, panes) = self.session_now(cx);
        let active = self.active;
        let count = cx.global::<crate::settings::Settings>().saved_sessions.len();
        let first = self.session_name.clone().unwrap_or_else(|| format!("{} {}", t!("Oturum"), count + 1));
        let me = cx.entity().downgrade();
        crate::prompt::show(
            t!("Oturumu kaydet"),
            Some(t!("Bu penceredeki sekmeler bu adla kaydedilir; sekme menüsündeki Oturum aç ile yeni bir pencerede açılır.").to_string()),
            Some((first, String::new())),
            vec![
                crate::prompt::Button::new(t!("Kaydet"), crate::prompt::Kind::Primary, move |text, _, cx| {
                    let name = text.unwrap_or_default().trim().to_string();
                    if name.is_empty() {
                        return Err(t!("Bir ad yazın.").to_string());
                    }
                    let session = crate::settings::NamedSession { name: name.clone(), tabs: tabs.clone(), panes: panes.clone(), active };
                    crate::settings::update(cx, move |s| remember_named(&mut s.saved_sessions, session));
                    // (the window now holds that session: its name goes beside the tabs)
                    let _ = me.update(cx, |ws, cx| {
                        ws.session_name = Some(name.clone());
                        cx.notify();
                    });
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    fn tab_menu(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        use crate::fluent_menu::{Item, Menu, MenuIcon};
        let me = cx.entity().downgrade();
        let act = |f: fn(&mut Workspace, usize, &mut Window, &mut Context<Workspace>)| {
            let me = me.clone();
            move |w: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |ws, cx| f(ws, ix, w, cx));
            }
        };
        let n = self.tabs.len();
        let items = vec![
            Item::action(t!("Yeni sekme"), MenuIcon::Lucide(L::Plus), act(|ws, _, w, cx| ws.open_new_tab(w, cx))).hint("Ctrl+T"),
            Item::action(t!("Sekmeyi çoğalt"), MenuIcon::Lucide(L::Copy), act(|ws, ix, w, cx| {
                if let Some(loc) = ws.tabs.get(ix).map(|t| t.view().read(cx).location().clone()) {
                    ws.open_tab(loc, w, cx);
                }
            }))
            .hint("Ctrl+Shift+K"),
            Item::action(t!("Yeni pencereye taşı"), MenuIcon::Lucide(L::AppWindow), act(|ws, ix, w, cx| ws.detach_tab(ix, w, cx))).enabled(n > 1),
            Item::Separator,
            Item::action(t!("Diğer sekmeleri kapat"), MenuIcon::None, act(|ws, ix, w, cx| {
                ws.active = ix;
                ws.close_other_tabs(w, cx)
            }))
            .enabled(n > 1),
            Item::action(t!("Soldaki sekmeleri kapat"), MenuIcon::None, act(|ws, ix, w, cx| ws.close_tabs_to_side(ix, false, w, cx))).enabled(ix > 0),
            Item::action(t!("Sağdaki sekmeleri kapat"), MenuIcon::None, act(|ws, ix, w, cx| ws.close_tabs_to_side(ix, true, w, cx))).enabled(ix + 1 < n),
            Item::action(t!("Tüm sekmeleri kapat"), MenuIcon::None, act(|ws, _, w, cx| ws.close_all_tabs(w, cx))),
            Item::action(t!("Sekmeyi kapat"), MenuIcon::Lucide(L::X), act(|ws, ix, w, cx| ws.close_tab(ix, w, cx))).hint("Ctrl+W"),
            Item::Separator,
            Item::action(t!("Kapatılan sekmeyi yeniden aç"), MenuIcon::Lucide(L::RotateCcw), act(|ws, _, w, cx| {
                if let Some(loc) = ws.closed.pop() {
                    ws.open_tab(loc, w, cx);
                }
            }))
            .hint("Ctrl+Shift+T")
            .enabled(!self.closed.is_empty()),
            Item::Separator,
            Item::action(t!("Oturumu kaydet…"), MenuIcon::Lucide(L::Save), act(|ws, _, w, cx| ws.save_named_session(w, cx))),
        ];
        // the saved sessions: each opens in a new window; a second list removes one
        let saved: Vec<String> = cx.global::<crate::settings::Settings>().saved_sessions.iter().map(|s| s.name.clone()).collect();
        let mut items = items;
        if !saved.is_empty() {
            let open: Vec<Item> = saved
                .iter()
                .map(|n| {
                    let n = n.clone();
                    let me = me.clone();
                    Item::action(n.clone(), MenuIcon::None, move |w, cx| {
                        // (in this window when the setting says so: issue 41)
                        if cx.global::<crate::settings::Settings>().sessions_in_this_window
                            && let Some(ws) = me.upgrade()
                        {
                            ws.update(cx, |ws, cx| ws.take_named_session(&n, w, cx));
                        } else {
                            open_named_session(&n, cx);
                        }
                    })
                })
                .collect();
            let remove: Vec<Item> = saved
                .iter()
                .map(|n| {
                    let n = n.clone();
                    Item::action(n.clone(), MenuIcon::None, move |_, cx| {
                        let n = n.clone();
                        crate::settings::update(cx, move |s| s.saved_sessions.retain(|x| x.name != n));
                    })
                })
                .collect();
            items.push(Item::sub(t!("Oturum aç"), MenuIcon::Lucide(L::FolderOpen), open));
            items.push(Item::sub(t!("Oturumu sil"), MenuIcon::Lucide(L::Trash), remove));
        }
        crate::fluent_menu::show(Menu { quick: Vec::new(), items }, window.mouse_position(), window, cx);
    }

    /// Breadcrumb chevron: pick a subfolder of `dir` (or a drive when `dir` is None).
    fn crumb_menu(&mut self, dir: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        let is_drives = dir.is_none();
        // List on a worker (can stall on network/optical drives); the shell thread only shows the menu.
        let listing = cx.background_spawn(async move {
            let mut entries: Vec<(String, PathBuf)> = match &dir {
                None => rf_win::drives::visible_drives().into_iter().map(|d| (crate::places::drive_name(&d), d.root)).collect(),
                Some(d) => {
                    let mut v = Vec::new();
                    let _ = rf_win::list_dir(d, 4096, &|| false, &mut |b| {
                        for e in b {
                            if e.is_dir() && !e.is_hidden() {
                                v.push((e.name.to_string(), d.join(&*e.name)));
                            }
                        }
                    });
                    v.sort_by(|a, b| rf_core::sort::natural_cmp(&a.0, &b.0));
                    v
                }
            };
            entries.truncate(80);
            entries
        });
        let pos = window.mouse_position() + point(px(-12.), px(14.));
        cx.spawn_in(window, async move |this, cx| {
            use crate::fluent_menu::{Item, Menu, MenuIcon};
            let entries = listing.await;
            let _ = this.update_in(cx, |_, window, cx| {
                let me = cx.entity().downgrade();
                let folder = MenuIcon::Tinted(L::Folder, crate::theme::kind_color(crate::icons::Category::Folder, true));
                let items: Vec<Item> = if entries.is_empty() {
                    vec![Item::action(t!("(alt klasör yok)"), MenuIcon::None, |_, _| {}).enabled(false)]
                } else {
                    entries
                        .into_iter()
                        .map(|(n, path)| {
                            let me = me.clone();
                            let icon = if is_drives { MenuIcon::Lucide(L::HardDrive) } else { folder.clone() };
                            Item::action(n, icon, move |w, cx| {
                                let p = path.clone();
                                let _ = me.update(cx, |ws, cx| ws.navigate(Location::Path(p), w, cx));
                            })
                        })
                        .collect()
                };
                crate::fluent_menu::show(Menu { quick: Vec::new(), items }, pos, window, cx);
            });
        })
        .detach();
    }

    /// Boşluk: DirectLook'a (ayrı süreç) aç/kapat iletisi. DirectLook kapalıyken bir şey yapmaz.
    fn toggle_quick_look(&mut self, cx: &mut Context<Self>) {
        use std::sync::atomic::Ordering;
        // Settings › DirectLook › "Space opens QuickLook": the file goes to QuickLook (a file inside an archive has no path it can open)
        if cx.global::<crate::settings::Settings>().space_quicklook {
            match self.active_tab().read(cx).preview_target() {
                Some((path, _)) if crate::archive_view::split_inside(&path).is_none() => crate::quicklook::space(path),
                target => log::info!("quicklook: Space with nothing QuickLook can open ({:?})", target.map(|(p, _)| p)),
            }
            return;
        }
        if !cx.global::<crate::settings::Settings>().directlook_enabled {
            return;
        }
        let open = crate::folder_view::QUICK_LOOK_OPEN.load(Ordering::Relaxed);
        let target = self.active_tab().read(cx).preview_target().map(|(p, e)| (p, e.size));
        match (open, target) {
            (true, _) => {
                crate::look::ipc::send(crate::look::ipc::Msg::Close, false);
                crate::folder_view::QUICK_LOOK_OPEN.store(false, Ordering::Relaxed);
                *LOOK_FOLLOWED.lock().unwrap() = None;
            }
            (false, Some((path, size))) => {
                *LOOK_FOLLOWED.lock().unwrap() = Some(path.clone());
                // an item inside an archive is extracted first
                crate::archive_view::look_send(path, size, false);
                crate::folder_view::QUICK_LOOK_OPEN.store(true, Ordering::Relaxed);
            }
            (false, None) => {}
        }
        cx.notify();
    }

    fn release_drive(&mut self, letter: char, cx: &mut Context<Self>) {
        let views: Vec<Entity<FolderView>> =
            self.tabs.iter().flat_map(|t| std::iter::once(t.left.clone()).chain(t.right.clone())).collect();
        let mut moved = false;
        for v in views {
            let here = v.read(cx).location().path().is_some_and(|p| on_drive(p, letter));
            if here {
                v.update(cx, |v, cx| v.navigate(Location::Home, cx));
                moved = true;
            }
        }
        self.close_quick_look(cx);
        self.preview.update(cx, |pv, cx| pv.release_drive(letter, cx));
        self.images.update(cx, |c, _| c.cancel_pending());
        if moved {
            log::info!("{letter}: is being removed; its panes went Home");
        }
        cx.notify();
    }

    fn close_quick_look(&mut self, cx: &mut Context<Self>) {
        if let Some(ql) = self.quick_look.take() {
            // release the hosted preview window before the pane takes it back
            ql.update(cx, |pv, _| pv.set_occluded(true));
        }
        crate::folder_view::QUICK_LOOK_OPEN.store(false, std::sync::atomic::Ordering::Relaxed);
        cx.notify();
    }

    fn open_bulk_rename(&mut self, paths: Vec<PathBuf>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::bulk_rename::{BulkRename, BulkRenameEvent};
        if paths.len() < 2 {
            return;
        }
        let br = cx.new(|cx| BulkRename::new(paths, window, cx));
        self._bulk_sub = Some(cx.subscribe_in(&br, window, |this, _, ev: &BulkRenameEvent, window, cx| match ev {
            BulkRenameEvent::Close => {
                this.bulk = None;
                this._bulk_sub = None;
                this.with_tab(cx, |v, cx| v.soft_reload(cx));
                this.focus_active(window, cx);
                cx.notify();
            }
        }));
        self.bulk = Some(br);
        cx.notify();
    }

    /// The command palette (Ctrl+Shift+P), with past folders among the commands unless the setting turned that off.
    fn toggle_command_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let with_folders = cx.global::<crate::settings::Settings>().palette_folder_history;
        self.toggle_palette(crate::command_palette::PaletteMode::Commands { with_folders }, window, cx)
    }

    /// Opens the palette listing what `mode` says, or closes it when it is open.
    fn toggle_palette(&mut self, mode: crate::command_palette::PaletteMode, window: &mut Window, cx: &mut Context<Self>) {
        use crate::command_palette::{CommandPalette, PaletteEvent, PaletteMode};
        if self.palette.take().is_some() {
            self._palette_sub = None;
            self.focus_active(window, cx);
            cx.notify();
            return;
        }
        let folders = match mode {
            PaletteMode::Commands { with_folders: false } => Vec::new(),
            _ => crate::folder_history::snapshot(),
        };
        // the tab search: this window's open tabs, then the closed ones, the last closed first
        let tabs = if mode == PaletteMode::Tabs {
            use crate::command_palette::TabEntry;
            let open = self.tabs.iter().enumerate().map(|(i, t)| {
                let v = t.view().read(cx);
                let detail = v.location().path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
                TabEntry { label: v.title(cx), detail, closed: false, id: i }
            });
            let closed = self.closed.iter().enumerate().rev().take(30).map(|(i, loc)| TabEntry {
                label: loc.title(),
                detail: loc.path().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
                closed: true,
                id: i,
            });
            open.chain(closed).collect()
        } else {
            Vec::new()
        };
        let pal = cx.new(|cx| CommandPalette::new(mode, folders, tabs, window, cx));
        self._palette_sub = Some(cx.subscribe_in(&pal, window, |this, _, ev: &PaletteEvent, window, cx| {
            this.palette = None;
            this._palette_sub = None;
            this.focus_active(window, cx);
            match ev {
                PaletteEvent::Dismiss => {}
                PaletteEvent::Run(action) => window.dispatch_action(action.boxed_clone(), cx),
                PaletteEvent::Navigate(p) => {
                    if let Some(loc) = parse_location(&p.to_string_lossy()) {
                        this.navigate(loc, window, cx);
                    }
                }
                PaletteEvent::Tab { closed: false, id } => {
                    if *id < this.tabs.len() {
                        this.activate(*id, window, cx);
                    }
                }
                PaletteEvent::Tab { closed: true, id } => {
                    if *id < this.closed.len() {
                        let loc = this.closed.remove(*id);
                        this.open_tab(loc, window, cx);
                    }
                }
            }
            cx.notify();
        }));
        self.palette = Some(pal);
        cx.notify();
    }

    /// The "Filter by" strip: typing filters this folder by name; closing clears the filter.
    fn toggle_filter_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter_bar = !self.filter_bar;
        if self.filter_bar {
            self.filter_input.update(cx, |s, cx| s.focus(window, cx));
        } else {
            self.filter_input.update(cx, |s, cx| s.set_value("", window, cx));
            self.with_tab(cx, |v, cx| v.set_query(String::new(), cx));
            self.focus_active(window, cx);
        }
        cx.notify();
    }

    fn render_filter_bar(&mut self, p: &Palette, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.filter_bar {
            return None;
        }
        // follow the folder: entering another folder clears its filter
        let q = self.active_tab().read(cx).filter.query.clone();
        if self.filter_input.read(cx).value().as_ref() != q.as_str() {
            self.filter_input.update(cx, |s, cx| s.set_value(q, window, cx));
        }
        Some(
            h_flex()
                .px_4()
                .py_2()
                .gap_3()
                .items_center()
                .border_b_1()
                .border_color(p.divider)
                .child(Icon::new(L::Folder).size(px(16.)).text_color(crate::theme::kind_color(crate::icons::Category::Folder, true)))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!("Şunlara göre filtrele")))
                .child(
                    div()
                        .w(px(200.))
                        .h(px(30.))
                        .px_2()
                        .flex()
                        .items_center()
                        .rounded(theme::RADIUS_SM)
                        .bg(p.hover)
                        .border_1()
                        .border_color(p.border)
                        .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                            if ev.keystroke.key == "escape" {
                                this.toggle_filter_bar(window, cx);
                                cx.stop_propagation();
                            }
                        }))
                        .child(Input::new(&self.filter_input).appearance(false).cleanable(true)),
                )
                .into_any_element(),
        )
    }

    fn navigate(&mut self, loc: Location, window: &mut Window, cx: &mut Context<Self>) {
        self.active_tab().clone().update(cx, |v, cx| v.navigate(loc, cx));
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.focus_active(window, cx);
    }

    fn toggle_shelf(&mut self, cx: &mut Context<Self>) {
        self.shelf_open = !self.shelf_open;
        if self.shelf_open {
            self.shelf.update(cx, |s, cx| s.prune(cx));
        }
        cx.notify();
    }

    fn on_shelf_event(&mut self, ev: &crate::shelf_view::ShelfEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::shelf_view::ShelfEvent;
        match ev {
            ShelfEvent::Transfer(kind, items) => {
                let dest = match self.active_tab().read(cx).location() {
                    Location::Path(p)
                        if !rf_win::ftp::is_ftp(p) && rf_win::network::server_root(p).is_none() =>
                    {
                        Some(p.clone())
                    }
                    _ => None,
                };
                if let Some(dest) = dest {
                    crate::status_center::transfer(cx, *kind, items.clone(), dest);
                    if *kind == rf_win::fileops::Transfer::Move {
                        // moved items leave their old place; the shelf is done with them
                        self.shelf.update(cx, |s, cx| s.clear(cx));
                    }
                }
            }
            ShelfEvent::Reveal(p) => {
                if let Some(parent) = p.parent() {
                    self.navigate(Location::Path(parent.to_path_buf()), window, cx);
                }
            }
            ShelfEvent::Close => {
                self.shelf_open = false;
                cx.notify();
            }
        }
    }

    /// The corner button's menu, like Files' "Tab actions": new window, compact overlay,
    /// split / arrange / close panes — plus the sidebar toggle that used to live there.
    fn tab_actions_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::fluent_menu::{Item, Menu, MenuIcon};
        let me = cx.entity().downgrade();
        let with = |f: fn(&mut Workspace, &mut Window, &mut Context<Workspace>)| {
            let me = me.clone();
            move |w: &mut Window, cx: &mut App| {
                let _ = me.update(cx, |ws, cx| f(ws, w, cx));
            }
        };
        let split = self.tabs[self.active].right.is_some();
        let vertical = cx.global::<crate::settings::Settings>().dual_pane_vertical;
        let mut items = vec![
            Item::action(t!("Yeni pencere"), MenuIcon::Lucide(L::AppWindow), |w, cx| w.dispatch_action(Box::new(NewWindow), cx)).hint("Ctrl+N"),
            if self.compact.is_some() {
                Item::action(t!("Kompakt görünümden çık"), MenuIcon::Lucide(L::Maximize2), with(|ws, w, cx| ws.toggle_compact(w, cx)))
            } else {
                Item::action(t!("Kompakt görünüme geç"), MenuIcon::Lucide(L::PictureInPicture2), with(|ws, w, cx| ws.toggle_compact(w, cx)))
            },
        ];
        if split {
            items.push(Item::sub(
                t!("Bölmeleri düzenle"),
                MenuIcon::Lucide(L::LayoutPanelLeft),
                vec![
                    Item::action(t!("Yan yana"), MenuIcon::Lucide(L::Columns2), with(|ws, w, cx| ws.arrange_panes(true, w, cx))).checked(vertical),
                    Item::action(t!("Üst üste"), MenuIcon::Lucide(L::Rows2), with(|ws, w, cx| ws.arrange_panes(false, w, cx))).checked(!vertical),
                ],
            ));
            items.push(Item::action(t!("Etkin bölmeyi kapat"), MenuIcon::Lucide(L::X), with(|ws, w, cx| ws.close_active_pane(w, cx))));
        } else {
            items.push(Item::sub(
                t!("Bölmeyi ayır"),
                MenuIcon::Lucide(L::LayoutPanelLeft),
                vec![
                    Item::action(t!("Yan yana"), MenuIcon::Lucide(L::Columns2), with(|ws, w, cx| ws.split_pane(true, w, cx))).hint("Ctrl+Shift+D"),
                    Item::action(t!("Üst üste"), MenuIcon::Lucide(L::Rows2), with(|ws, w, cx| ws.split_pane(false, w, cx))),
                ],
            ));
        }
        items.push(Item::Separator);
        items.push(
            Item::action(t!("Kenar çubuğu"), MenuIcon::Lucide(L::PanelLeft), |w, cx| w.dispatch_action(Box::new(ToggleSidebar), cx))
                .checked(self.sidebar_open)
                .hint("Ctrl+B"),
        );
        let tree_on = cx.global::<crate::settings::Settings>().sidebar_tree;
        items.push(Item::action(t!("Klasör ağacı"), MenuIcon::Lucide(L::FolderTree), |_, cx| crate::settings::update(cx, |s| s.sidebar_tree = !s.sidebar_tree)).checked(tree_on));
        // where the preview pane sits (Alt+P shows and hides it); a file dialog's window keeps it at the right, so it has no choice to offer
        if self.picker.is_none() {
            let preview_pos = cx.global::<crate::settings::Settings>().preview_position;
            for (pos, label, icon) in [(rf_core::previewpane::PreviewPos::Right, t!("Önizleme sağda"), L::PanelRight), (rf_core::previewpane::PreviewPos::Bottom, t!("Önizleme altta"), L::PanelBottom)] {
                items.push(Item::action(label, MenuIcon::Lucide(icon), move |_, cx| preview_dock::set_preview_position(cx, pos)).checked(preview_pos == pos));
            }
        }
        let pos = point(px(6.), theme::TITLEBAR_H);
        crate::fluent_menu::show(Menu { quick: Vec::new(), items }, pos, window, cx);
    }

    fn split_pane(&mut self, side_by_side: bool, window: &mut Window, cx: &mut Context<Self>) {
        crate::settings::update(cx, |s| s.dual_pane_vertical = side_by_side);
        if self.tabs[self.active].right.is_none() {
            self.toggle_dual_pane(window, cx);
        }
    }

    fn arrange_panes(&mut self, side_by_side: bool, _: &mut Window, cx: &mut Context<Self>) {
        crate::settings::update(cx, |s| s.dual_pane_vertical = side_by_side);
        cx.notify();
    }

    /// Closes the focused pane of a split tab; the other one stays.
    fn close_active_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let t = &mut self.tabs[self.active];
        let Some(right) = t.right.take() else { return };
        if !t.right_active {
            t.left = right;
        }
        t.right_active = false;
        t.left.update(cx, |v, _| v.dual = false);
        self.focus_active(window, cx);
        cx.notify();
    }

    /// The preview pane shows documents in a window of the shell thread that this window owns. Destroying this window would wait
    /// for that thread to destroy it: the picker of an app's Open dialog hung on the screen with the PDF it previewed (issue 146).
    /// So the host is let go first, whenever this window is about to close.
    pub(crate) fn release_preview_host(window: &Window) {
        if let Some(owner) = Self::hwnd(window) {
            // (without waiting for the shell thread: it may be the one that is busy, see `detach_from`)
            rf_win::preview_host::detach_from(owner);
        }
    }

    fn hwnd(window: &Window) -> Option<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match HasWindowHandle::window_handle(window).ok()?.as_raw() {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
            _ => None,
        }
    }

    /// Files' "compact overlay": a small always-on-top window with just the file list (sidebar
    /// and preview hidden); leaving restores the previous size, position and z-order.
    fn toggle_compact(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(hwnd) = Self::hwnd(window) else { return };
        // The window is moved and resized from a TASK, never from inside this update. These calls deliver the new size and place at
        // once, while the app is still borrowed, and GPUI then drops them ("handling resize ...: RefCell already borrowed" in the
        // log): the window changes, but the content, and with it every place a click is tested against, stays at the old size
        // until the next resize. That is one way a click on the close button could land on something else (issue 31: "after ...
        // toggling window states ... the first click forces it to exit fullscreen / resize down"). `look/window.rs` found the same
        // with its own window.
        if let Some(r) = self.compact.take() {
            let (rect, maximized, topmost) = (r.rect, r.maximized, r.topmost);
            cx.spawn(async move |_, _| {
                rf_win::window::set_min_size(hwnd, None);
                rf_win::window::place(hwnd, rect);
                if maximized {
                    rf_win::window::maximize(hwnd);
                }
                if !topmost {
                    let _ = rf_win::window::set_topmost(hwnd, false);
                }
            })
            .detach();
        } else {
            let Some((rect, maximized)) = rf_win::window::rect(hwnd) else { return };
            // restore to the normal (not maximized) rectangle later
            let normal = if maximized { None } else { Some(rect) };
            let topmost = rf_win::window::is_topmost(hwnd);
            let scale = window.scale_factor();
            let (w, h) = ((COMPACT_W * scale) as i32, (COMPACT_H * scale) as i32);
            // keep the top-right corner where it was: the overlay hugs the side it came from
            let (right, top) = (rect[2], rect[1].max(0));
            let restore = normal.unwrap_or_else(|| {
                let b = window.bounds();
                let o = |v: Pixels| (f32::from(v) * scale) as i32;
                [o(b.origin.x), o(b.origin.y), o(b.origin.x + b.size.width), o(b.origin.y + b.size.height)]
            });
            let small = [right - w, top, right, top + h];
            cx.spawn(async move |_, _| {
                rf_win::window::set_min_size(hwnd, Some((COMPACT_MIN.0, COMPACT_MIN.1)));
                rf_win::window::place(hwnd, small);
                let _ = rf_win::window::set_topmost(hwnd, true);
            })
            .detach();
            self.compact = Some(CompactRestore { rect: restore, maximized, topmost });
        }
        cx.notify();
    }

    fn toggle_always_on_top(&mut self, window: &mut Window) {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = window.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                let hwnd = w.hwnd.get();
                let on = !rf_win::window::is_topmost(hwnd);
                if let Err(e) = rf_win::window::set_topmost(hwnd, on) {
                    log::warn!("always on top: {e}");
                }
            }
        }
    }

    fn with_tab(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut FolderView, &mut Context<FolderView>)) {
        self.active_tab().clone().update(cx, f);
    }

    fn clear_suggestions(&mut self) {
        self.addr_suggest.clear();
        self.addr_sel = None;
        self._addr_task = None;
    }

    /// Folder completions for the address text, computed off the UI thread (network paths can
    /// be slow); stale answers are dropped with the task.
    fn update_suggestions(&mut self, text: String, cx: &mut Context<Self>) {
        // "@pro": the matching folder aliases
        if text.trim_start().starts_with(crate::aliases::PREFIX) && !text.contains(['\\', '/']) {
            self._addr_task = None;
            self.addr_suggest = crate::aliases::matches(&text).into_iter().map(|(_, p)| p).take(8).collect();
            self.addr_sel = None;
            cx.notify();
            return;
        }
        let expanded = expand_env(text.trim());
        if !rf_win::complete::completable(&expanded) {
            self.clear_suggestions();
            cx.notify();
            return;
        }
        let hidden = cx.global::<crate::settings::Settings>().show_hidden;
        let work = cx.background_spawn(async move { rf_win::complete::complete_dirs(&expanded, 8, hidden) });
        self._addr_task = Some(cx.spawn(async move |this, cx| {
            let list = work.await;
            let _ = this.update(cx, |ws, cx| {
                if ws.editing_address {
                    ws.addr_suggest = list;
                    ws.addr_sel = None;
                    cx.notify();
                }
            });
        }));
    }

    /// Tab (arrives as the input's unhandled `IndentInline`): takes the highlighted (or first)
    /// completion into the text and lists its children.
    fn accept_suggestion(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_address && self.addr_suggest.is_empty() {
            // no suggestion to take: Tab goes on to the next pane, as it does from the list (issue 38)
            self.cycle_focus(true, window, cx);
            return;
        }
        if !self.editing_address || self.addr_suggest.is_empty() {
            cx.propagate();
            return;
        }
        let pick = self.addr_sel.unwrap_or(0);
        if let Some(p) = self.addr_suggest.get(pick).cloned() {
            let text = format!("{}\\", p.to_string_lossy().trim_end_matches('\\'));
            self.address.update(cx, |s, cx| s.set_value(text.clone(), window, cx));
            self.update_suggestions(text, cx);
        }
    }

    /// Up/Down move the highlight; Escape closes the list.
    fn address_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if !self.editing_address || self.addr_suggest.is_empty() {
            return;
        }
        let n = self.addr_suggest.len();
        match ev.keystroke.key.as_str() {
            "down" => {
                self.addr_sel = Some(self.addr_sel.map_or(0, |i| (i + 1) % n));
                cx.stop_propagation();
                cx.notify();
            }
            "up" => {
                self.addr_sel = Some(self.addr_sel.map_or(n - 1, |i| (i + n - 1) % n));
                cx.stop_propagation();
                cx.notify();
            }
            "escape" => {
                self.clear_suggestions();
                cx.notify();
            }
            _ => {}
        }
    }

    /// Searches the active tab's folder and its subfolders for `q`, and remembers the search.
    fn run_search(&mut self, q: String, window: &mut Window, cx: &mut Context<Self>) {
        let root = match self.active_tab().read(cx).location() {
            Location::Search { root, .. } => Some(root.clone()),
            l => l.path().map(Path::to_path_buf),
        };
        let (false, Some(root)) = (q.is_empty(), root) else { return };
        crate::settings::update(cx, |s| remember_search(&mut s.search_history, &q));
        self.active_tab().clone().update(cx, |v, cx| v.navigate(Location::Search { root, query: q }, cx));
        self.focus_active(window, cx);
    }

    /// Under the empty search box: the searches run before (a click runs one again).
    fn render_search_history(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.search_mode || !self.search.read(cx).value().trim().is_empty() {
            return None;
        }
        let history = cx.global::<crate::settings::Settings>().search_history.clone();
        if history.is_empty() {
            return None;
        }
        let mut list = v_flex().absolute().top(px(38.)).left_0().right_0().py_1().rounded(theme::RADIUS).bg(p.card_bg).border_1().border_color(p.border).shadow_lg();
        let row = |id: ElementId| h_flex().id(id).mx_1().px_2().h(px(28.)).gap_2().items_center().rounded(theme::RADIUS_SM).text_sm().hover(|s| s.bg(p.hover)).cursor_pointer();
        for (i, q) in history.into_iter().enumerate() {
            let query = q.clone();
            list = list.child(
                row(("search-hist", i).into())
                    .child(Icon::new(L::Clock).size(px(14.)).text_color(p.text_secondary))
                    .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().child(q))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.search.update(cx, |s, cx| s.set_value(query.clone(), window, cx));
                            this.run_search(query.clone(), window, cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        list = list.child(
            row("search-hist-clear".into())
                .text_color(p.text_secondary)
                .child(Icon::new(L::Trash).size(px(14.)))
                .child(t!("Önceki aramaları temizle"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|_, _, _, cx| {
                        cx.stop_propagation();
                        crate::settings::update(cx, |s| s.search_history.clear());
                        cx.notify();
                    }),
                ),
        );
        Some(deferred(list).with_priority(2).into_any_element())
    }

    fn render_suggestions(&self, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.editing_address || self.addr_suggest.is_empty() {
            return None;
        }
        let alias_mode = self.address.read(cx).value().trim_start().starts_with(crate::aliases::PREFIX);
        let mut list = v_flex()
            .absolute()
            .top(px(38.))
            .left_0()
            .right_0()
            .py_1()
            .rounded(theme::RADIUS)
            .bg(p.card_bg)
            .border_1()
            .border_color(p.border)
            .shadow_lg();
        for (i, path) in self.addr_suggest.iter().enumerate() {
            let sel = self.addr_sel == Some(i);
            let target = path.clone();
            list = list.child(
                h_flex()
                    .id(("addr-sug", i))
                    .mx_1()
                    .px_2()
                    .h(px(28.))
                    .gap_2()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .text_sm()
                    .when(sel, |d| d.bg(p.selection))
                    .hover(|s| s.bg(p.hover))
                    .cursor_pointer()
                    .child(Icon::new(L::Folder).size(px(14.)).text_color(p.accent))
                    .children(alias_mode.then(|| crate::aliases::name_of(path)).flatten().map(|n| {
                        div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(format!("@{n}"))
                    }))
                    .child(div().overflow_hidden().text_ellipsis().whitespace_nowrap().when(alias_mode, |d| d.text_color(p.text_secondary)).child(path.to_string_lossy().into_owned()))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, window, cx| {
                            cx.stop_propagation();
                            this.clear_suggestions();
                            this.editing_address = false;
                            this.navigate(Location::Path(target.clone()), window, cx);
                            this.focus_active(window, cx);
                            cx.notify();
                        }),
                    ),
            );
        }
        Some(deferred(list).with_priority(2).into_any_element())
    }

    fn begin_address_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = match self.active_tab().read(cx).location() {
            Location::Path(p) => p.to_string_lossy().into_owned(),
            Location::Search { root, .. } => root.to_string_lossy().into_owned(),
            _ => String::new(),
        };
        self.editing_address = true;
        self.address.update(cx, |s, cx| {
            s.set_value(text, window, cx);
            s.focus(window, cx);
            s.select_all(window, cx);
        });
        cx.notify();
    }

    // ---------- rendering ----------

    /// A menu, flyout or the Status Center is open: buttons under it show no tooltips (one
    /// would be painted over the popup).
    fn popups_open(&self, cx: &App) -> bool {
        crate::fluent_menu::is_open(cx) || crate::flyout::is_open(cx) || self.ops.read(cx).open
    }

    fn render_titlebar(&mut self, p: &Palette, window: &Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let popups_open = self.popups_open(cx);
        // the saved session this window holds, at the far left of the tabs
        let session_label = self.session_name.clone().filter(|n| cx.global::<crate::settings::Settings>().saved_sessions.iter().any(|s| &s.name == n)).map(|name| {
            h_flex()
                .id("session-name")
                .flex_shrink_0()
                .max_w(px(180.))
                .h(px(26.))
                .mb(px(3.))
                .ml_1()
                .px_2()
                .gap_1p5()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .bg(p.hover)
                .border_1()
                .border_color(p.border)
                .text_xs()
                .text_color(p.text_secondary)
                .when(!popups_open, |d| d.tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Kayıtlı oturum")).build(w, cx)))
                .child(Icon::new(L::Layers).size(px(13.)).text_color(p.accent))
                .child(div().min_w_0().truncate().child(name))
        });
        let mut tabs = h_flex()
            .id("tabs")
            // content-sized, shrinks (and scrolls) only when there is not enough room
            .flex_shrink(1.)
            .min_w_0()
            .h_full()
            .items_end()
            .gap_1()
            .pl_1()
            .overflow_x_scroll()
            .track_scroll(&self.tab_scroll);
        if self.tab_scrolled_to != Some(self.active) {
            // only when the active tab changes, so manual scrolling is not fought every frame
            self.tab_scrolled_to = Some(self.active);
            self.tab_scroll.scroll_to_item(self.active);
        }
        // Equal-width tabs that share the strip (like Files / WinUI TabView "Equal"): wide
        // while there are few, shrinking as more open, scrolling only below the minimum.
        let strip = (self.viewport_w - 150. - TITLEBAR_LEAD - 40.).max(120.);
        let n = self.tabs.len().max(1) as f32;
        let tab_w = ((strip - 4.) / n - 4.).clamp(TAB_MIN_W, TAB_MAX_W);
        // Settings > Appearance > "Tabs as wide as their names" (issue 119): each tab its measured name plus its icon and buttons, within
        // the same limits; the buttons are in that width, so they always show
        let fit_names = cx.global::<crate::settings::Settings>().tabs_fit_names;
        // Settings > Appearance > "Terminal button on tabs" (issue 119: off, nothing is in the way of a click on a narrow tab)
        let tab_terminal = cx.global::<crate::settings::Settings>().tab_terminal_button;
        // Settings > Appearance > "The folder's path on tabs" (issue 162: "show whatever number of characters it can on the tab")
        let tab_path = cx.global::<crate::settings::Settings>().tab_shows_path;
        let (name_px, name_style) = (rems(0.875).to_pixels(window.rem_size()), window.text_style());
        let roomy = fit_names || tab_w >= 110.;
        let mut tab_claimed = false;
        let windows_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        for (i, tab) in self.tabs.iter().enumerate() {
            let (title, icon, location) = {
                let view = tab.view().read(cx);
                (view.title(cx), view.tab_icon(), view.location().clone())
            };
            let tab_dir = location.path().map(Path::to_path_buf);
            let shell_tab_icon = if windows_theme && matches!(&location, Location::Path(_)) {
                tab_dir.as_ref().and_then(|path| {
                    self.images.update(cx, |images, cx| {
                        images.shell_icon(&path.to_string_lossy(), 16, cx)
                    })
                })
            } else {
                None
            };
            let windows_home_tab = windows_theme && matches!(&location, Location::Home);
            let term_dir = tab_terminal.then(|| tab_terminal_dir(&location)).flatten();
            let path_title: Option<SharedString> = tab_dir.as_ref().filter(|_| tab_path).map(|d| d.display().to_string().into());
            let shown: SharedString = path_title.clone().unwrap_or_else(|| title.clone().into());
            let tab_w = if fit_names {
                let name = window.text_system().layout_line(shown.as_ref(), name_px, &[name_style.to_run(shown.len())], None).width;
                (f32::from(name) + TAB_CHROME_W + if term_dir.is_some() { TAB_TERMINAL_W } else { 0. }).clamp(TAB_MIN_W, TAB_MAX_W)
            } else {
                tab_w
            };
            let active = i == self.active;
            let drag = TabDrag { ix: i, title: title.clone().into() };
            let accent = p.accent;
            let drop_bg = p.selection;
            // Over glass the selected tab is a lens the engine draws behind the scene (the tab's rectangle made taller downwards by its
            // corner, cut flat by the strip) instead of the Fluent fill. A stable key per tab, so another selection slides the lens.
            let tab_lens = (active && self.tab_lens && !tab_claimed).then(|| crate::glass::key_of(&tab.left.entity_id().as_u64()));
            tab_claimed |= tab_lens.is_some();
            let lens_tab = tab_lens.is_some();
            let anim = self.tab_anim.as_ref().filter(|(v, _)| *v == tab.left).map(|(_, s)| *s);
            let el = h_flex()
                    .id(("tab", i))
                    .group("tab")
                    .flex_shrink_0()
                    .h(px(32.))
                    .w(px(tab_w))
                    .pl_3()
                    .pr_1()
                    .gap_2()
                    .items_center()
                    // A closed shape that stands a little above the strip's bottom (issue 168: "since the base of the tab shape doesn't
                    // rest on anything, it looks strange"): nothing under the strip continues the tab, so a tab open at the bottom
                    // looked cut off. Over glass the selected tab is the engine's lens, whose shape is drawn standing on the strip.
                    .when(self.tab_lens, |d| d.rounded_t(theme::RADIUS))
                    .when(!self.tab_lens, |d| d.rounded(theme::RADIUS).mb_1())
                    .text_sm()
                    // while the tab moves (tabs open or close beside it, the window is resized) the scene paints the fill itself: the
                    // engine's lens would trail the tab (see glass::LensPhase)
                    .when(active && (tab_lens.is_none() || crate::glass::lens_phase(self.glass_hwnd, crate::glass::SLOT_TAB) != crate::glass::LensPhase::Rest), |d| d.bg(p.tab_active))
                    .when_some(tab_lens, |d, key| {
                        d.relative().child(crate::glass::lens_probe_shaped(
                            crate::glass::SLOT_TAB,
                            key,
                            crate::glass::LensShape::Tab { radius: f32::from(theme::RADIUS) },
                        ))
                    })
                    .when(!active, |d| d.text_color(p.text_secondary).hover(|s| s.bg(p.hover)))
                    .child(match shell_tab_icon {
                        Some(image) => {
                            img(image).size(px(14.)).flex_shrink_0().into_any_element()
                        }
                        None if windows_home_tab => crate::icons::windows_home_icon(px(14.)),
                        None => Icon::new(icon).size(px(14.)).flex_shrink_0().text_color(if active { p.accent } else { p.text_secondary }).into_any_element(),
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .when(path_title.is_some(), |d| d.text_xs())
                            .child(shown),
                    )
                    // the folder's whole path on hover (issue 162: "I am unable to see a way to see the full path on tabs")
                    .when_some(tab_dir.clone().filter(|_| !popups_open), |d, dir| {
                        let path = dir.display().to_string();
                        d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(w, cx))
                    })
                    .when_some(term_dir, |d, dir| {
                        d.child(
                            div()
                                .id(("tab-term", i))
                                .size(px(22.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::RADIUS_SM)
                                .text_color(if active { p.text_secondary } else { p.text_tertiary })
                                .when(!active, |d| d.invisible())
                                .when(!roomy, |d| d.hidden())
                                .group_hover("tab", |s| s.visible())
                                .hover(|s| s.bg(p.pressed).text_color(p.text))
                                .when(!popups_open, |d| {
                                    d.tooltip(|w, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(t!("Terminalde aç")).build(w, cx)
                                    })
                                })
                                .child(Icon::new(L::SquareTerminal).size(px(12.)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(move |_this, _, _, cx| {
                                    let dir = dir.clone();
                                    cx.background_spawn(async move {
                                        let _ = rf_win::shell::open_terminal(&dir);
                                    })
                                    .detach();
                                })),
                        )
                    })
                    .child(
                        div()
                            .id(("tab-close", i))
                            .size(px(22.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(theme::RADIUS_SM)
                            // like Files: every tab shows its close button while there is room
                            .when(!roomy && !active, |d| d.invisible())
                            .group_hover("tab", |s| s.visible())
                            .hover(|s| s.bg(p.pressed))
                            .child(Icon::new(L::X).size(px(12.)))
                            // the press stays with the x: the tab's own press would bring the tab forward first, so closing a tab in the
                            // back showed it for a moment (#145)
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(i, window, cx);
                            })),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            // Keep the title bar from turning a tab drag into a window move.
                            cx.stop_propagation();
                            this.activate(i, window, cx)
                        }),
                    )
                    .on_mouse_down(
                        MouseButton::Middle,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| this.close_tab(i, window, cx)),
                    )
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| this.tab_menu(i, window, cx)),
                    )
                    // files dragged over a tab and held there switch to it (Explorer / Files)
                    .on_drag_move::<ExternalPaths>(cx.listener(move |this, ev: &DragMoveEvent<ExternalPaths>, window, cx| {
                        let over = ev.bounds.contains(&ev.event.position);
                        this.drag_over_tab(i, over, window, cx);
                    }))
                    .on_drag(drag, |d: &TabDrag, _, _, cx| cx.new(|_| d.clone()))
                    // The drop marker of a tab that carries the glass lens must not change its box: the lens is measured from the box, and a
                    // 2 px left border made the pill jump and narrow while a dragged tab hovered the selected one. It gets a tint instead.
                    .drag_over::<TabDrag>(move |s, _, _, _| if lens_tab { s.bg(accent.opacity(0.22)) } else { s.border_l_2().border_color(accent) })
                    .on_drop(cx.listener(move |this, d: &TabDrag, window, cx| this.move_tab(d.ix, i, window, cx)))
                    .when_some(tab_dir, |d, dir| {
                        d.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg))
                            .on_drag_move::<ExternalPaths>(crate::folder_view::drop_effect_over(dir.clone()))
                            .on_drop(move |paths: &ExternalPaths, _, cx| drop_files(paths, dir.clone(), cx))
                    });
            tabs = match anim {
                // grows from a compact width to its full size, contents fading in
                Some(serial) => tabs.child(el.overflow_hidden().with_animation(
                    ("tab-open", serial),
                    Animation::new(std::time::Duration::from_millis(TAB_ANIM_MS)).with_easing(ease_out_quint()),
                    move |el, d| el.w(px(28. + (tab_w - 28.) * d)).h(px(28. + 4. * d)).opacity(0.25 + 0.75 * d),
                )),
                None => tabs.child(el),
            };
        }
        self.tab_lens_claimed = tab_claimed;
        // over glass the "+" sits in a round lens (a constant key: it never slides)
        let plus_lens = self.tab_lens;
        self.plus_lens_claimed = plus_lens;
        // outside the scrolling strip: always reachable
        let fluent_plus = || {
            div()
                .size(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(14.))
                .child("+")
                .into_any_element()
        };
        let new_tab_icon = if windows_theme {
            #[cfg(target_os = "windows")]
            {
                crate::icons::windows_theme_icon_glyph(L::Plus)
                    .map(|glyph| crate::icons::windows_theme_glyph(glyph, px(14.)))
                    .unwrap_or_else(fluent_plus)
            }
            #[cfg(not(target_os = "windows"))]
            {
                fluent_plus()
            }
        } else {
            Icon::new(L::Plus).size(px(14.)).into_any_element()
        };
        let new_tab = div()
            .id("new-tab")
            .flex_shrink_0()
            .ml_1()
            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Yeni sekme  (Ctrl+T)")).build(w, cx))
            .size(px(28.))
            .mb(px(if plus_lens { 2. } else { 6. }))
            .flex()
            .items_center()
            .justify_center()
            .when(!plus_lens, |d| d.rounded(theme::RADIUS_SM))
            .when(plus_lens, |d| {
                d.relative().rounded_full().child(crate::glass::lens_probe_shaped(
                    crate::glass::SLOT_NEW_TAB,
                    crate::glass::key_of("new-tab"),
                    crate::glass::LensShape::Circle,
                ))
            })
            .when(
                plus_lens && crate::glass::lens_phase(self.glass_hwnd, crate::glass::SLOT_NEW_TAB) != crate::glass::LensPhase::Rest,
                |d| d.bg(p.tab_active),
            )
            .text_color(p.text_secondary)
            .hover(|s| s.bg(p.hover))
            .child(new_tab_icon)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(cx.listener(|this, _, window, cx| this.open_new_tab(window, cx)));
        // after the new tab has grown, the next "+" slides out from behind it
        let new_tab: AnyElement = match self.tab_anim.as_ref().map(|(_, s)| *s) {
            Some(serial) => {
                let total = (TAB_ANIM_MS + PLUS_ANIM_MS) as f32;
                let start = TAB_ANIM_MS as f32 / total;
                new_tab
                    .relative()
                    .with_animation(
                        ("plus-in", serial),
                        Animation::new(std::time::Duration::from_millis(TAB_ANIM_MS + PLUS_ANIM_MS)),
                        move |el, d| {
                            let t = ((d - start) / (1. - start)).clamp(0., 1.);
                            let e = ease_out_quint()(t);
                            el.left(px(-26. * (1. - e))).opacity(e)
                        },
                    )
                    .into_any_element()
            }
            None => new_tab.into_any_element(),
        };

        TitleBar::new()
            .h(theme::TITLEBAR_H)
            .bg(p.titlebar_bg)
            .border_color(gpui_kit::transparent_black())
            .child(
                h_flex()
                    .h_full()
                    // explicit width: the component's title bar never shrinks its content, so
                    // many tabs would push the window buttons off-screen (min/max/close ~140 px)
                    .w(px((self.viewport_w - 150.).max(200.)))
                    .min_w_0()
                    .overflow_hidden()
                    .items_end()
                    .child(
                        // sidebar toggle in the corner (Files has the pane button here, no app name)
                        // centered over the Back button below (title bar pads 12 px, the nav row 8 px)
                        h_flex().flex_shrink_0().w(px(TITLEBAR_LEAD)).h_full().items_end().child(
                            div()
                                .id("tab-actions")
                                .ml(px(-2.))
                                .mb(px(2.))
                                .size(px(28.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(theme::RADIUS_SM)
                                .text_color(p.text_secondary)
                                .hover(|s| s.bg(p.hover))
                                .when(!popups_open, |d| d.tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Sekme eylemleri")).build(w, cx)))
                                .child(Icon::new(if self.compact.is_some() { L::PictureInPicture2 } else { L::PanelLeft }).size(px(16.)))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(cx.listener(|this, _, window, cx| this.tab_actions_menu(window, cx))),
                        ),
                    )
                    .children(session_label)
                    .child(tabs)
                    .child(new_tab),
            )
    }

    /// Live pane resize; the width is kept in settings (written with the next session save).
    fn set_pane_width(&mut self, sidebar: bool, w: f32, cx: &mut Context<Self>) {
        if sidebar {
            let rail = if self.sidebar_rail { w < RAIL_SNAP + 30. } else { w < RAIL_SNAP };
            if rail != self.sidebar_rail {
                self.sidebar_rail = rail;
                if self.picker.is_none() {
                    cx.update_global::<crate::settings::Settings, _>(|s, _| s.sidebar_rail = rail);
                }
                cx.notify();
            }
            if rail {
                return;
            }
        }
        let w = if sidebar {
            w.clamp(SIDEBAR_MIN, SIDEBAR_MAX)
        } else {
            // (the list keeps its minimum width beside the pane and the sidebar)
            let side = if !self.sidebar_open { 0. } else if self.sidebar_rail { RAIL_W } else { self.sidebar_w };
            let room = (self.viewport_w - side - rf_core::previewpane::LIST_MIN_W).max(PREVIEW_MIN);
            w.clamp(PREVIEW_MIN, room.min(PREVIEW_MAX))
        };
        let cur = if sidebar { &mut self.sidebar_w } else { &mut self.preview_w };
        if (*cur - w).abs() < 0.5 {
            return;
        }
        *cur = w;
        let explorer_width = self.sidebar_width_explorer_mode;
        cx.update_global::<crate::settings::Settings, _>(|s, _| {
            if sidebar {
                if explorer_width {
                    s.explorer_sidebar_width = Some(w);
                } else {
                    s.sidebar_width = w;
                }
            } else {
                s.preview_width = w;
            }
        });
        // write once the drag has settled
        self._width_save = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            let _ = cx.update(|cx| {
                let snapshot = cx.global::<crate::settings::Settings>().clone();
                std::thread::spawn(move || snapshot.save());
            });
        }));
        cx.notify();
    }

    /// The pin list as it is right now: the user's list from settings (updated synchronously),
    /// else the defaults shown by `Places`. `Places` itself refreshes asynchronously, so reading it
    /// right after a change would undo that change.
    fn current_pins(&self, cx: &App) -> Vec<PathBuf> {
        cx.global::<crate::settings::Settings>()
            .pinned
            .clone()
            .unwrap_or_else(|| self.places.read(cx).pinned.iter().map(|f| f.path.clone()).collect())
    }

    /// Reorders pinned entries (drag & drop in the sidebar); persisted as the user's pin list.
    /// By path, not index: the drawn list may still show the order from before a previous drag.
    fn move_pin(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        let mut list = self.current_pins(cx);
        let (Some(from), Some(to)) = (list.iter().position(|p| p == from), list.iter().position(|p| p == to)) else {
            return;
        };
        if from == to {
            return;
        }
        let item = list.remove(from);
        list.insert(to, item);
        crate::settings::update(cx, |s| s.pinned = Some(list));
        self.places.update(cx, |p, cx| p.refresh(cx));
    }

    /// Right-click on a sidebar folder/drive: the Explorer menu (eject, properties, format…)
    /// plus open in new tab/window and pin/unpin.
    fn sidebar_menu(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let shift = unsafe {
            windows::Win32::UI::Input::KeyboardAndMouse::GetKeyState(windows::Win32::UI::Input::KeyboardAndMouse::VK_SHIFT.0 as i32) < 0
        };
        self.sidebar_menu_at(path, shift, window.mouse_position(), window, cx);
    }

    /// The sidebar's folder menu at `at`; `classic`: Windows' own menu (extended verbs) instead of ours.
    fn sidebar_menu_at(&mut self, path: PathBuf, classic: bool, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        use crate::fluent_menu::{Item, Menu, MenuIcon, Sub};
        if classic {
            // the classic Explorer menu, extended verbs included
            let p2 = path.clone();
            rf_win::sta::run(move |owner| {
                let mut pt = windows::Win32::Foundation::POINT::default();
                unsafe {
                    let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
                }
                if let Err(e) = rf_win::ctxmenu::show_for_one(owner, &p2, (pt.x, pt.y), true, &[]) {
                    log::warn!("sidebar menu: {e:#}");
                }
            });
            return;
        }
        let me = cx.entity().downgrade();
        let is_pinned = self.current_pins(cx).contains(&path);
        let root = path.parent().is_none();
        let with = |f: fn(&mut Workspace, PathBuf, &mut Window, &mut Context<Workspace>)| {
            let (me, path) = (me.clone(), path.clone());
            move |w: &mut Window, cx: &mut App| {
                let p = path.clone();
                let _ = me.update(cx, |ws, cx| f(ws, p, w, cx));
            }
        };
        let mut items = vec![
            Item::action(t!("Aç"), MenuIcon::Lucide(L::SquareArrowOutUpRight), with(|ws, p, w, cx| ws.navigate(Location::Path(p), w, cx))),
            Item::action(t!("Yeni sekmede aç"), MenuIcon::Lucide(L::SquarePlus), with(|ws, p, w, cx| {
                let keep = ws.active;
                ws.open_tab(Location::Path(p), w, cx);
                if !cx.global::<crate::settings::Settings>().focus_new_tab {
                    ws.activate(keep, w, cx);
                }
            })),
            Item::action(t!("Yeni pencerede aç"), MenuIcon::Lucide(L::AppWindow), with(|_, p, w, cx| {
                let mut b = w.bounds();
                b.origin.x += px(36.);
                b.origin.y += px(36.);
                cx.defer(move |cx| crate::open_main_window(Some(Location::Path(p)), None, Some(WindowBounds::Windowed(b)), cx));
            })),
            Item::Separator,
            if is_pinned {
                Item::action(t!("Kenar çubuğundan kaldır"), MenuIcon::Lucide(L::PinOff), with(|ws, p, _, cx| ws.set_pinned(p, false, cx)))
            } else {
                Item::action(t!("Kenar çubuğuna sabitle"), MenuIcon::Lucide(L::Pin), with(|ws, p, _, cx| ws.set_pinned(p, true, cx)))
            },
        ];
        if !root && cx.global::<crate::settings::Settings>().menu.tags && !cx.global::<crate::tags::Tags>().0.defs.is_empty() {
            items.push(Item::sub(t!("Etiketler"), MenuIcon::Lucide(L::Tag), crate::tags::menu_items(std::slice::from_ref(&path), cx)));
        }
        items.extend([
            Item::action(t!("Yolu kopyala"), MenuIcon::Lucide(L::ClipboardCopy), with(|_, p, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(p.display().to_string()))
            })),
            Item::action(t!("Terminalde aç"), MenuIcon::Lucide(L::SquareTerminal), with(|_, p, _, _| {
                std::thread::spawn(move || {
                    let _ = rf_win::shell::open_terminal(&p);
                });
            })),
            // our Properties window (a drive gets its own page there)
            Item::action(t!("Özellikler"), MenuIcon::Lucide(L::Wrench), with(|ws, p, _, cx| {
                crate::properties_view::open(vec![p], ws.images.clone(), cx);
            }))
            .hint("Alt+Enter"),
            Item::Separator,
            Item::Sub { label: t!("Diğer seçenekleri göster").into(), icon: MenuIcon::Lucide(L::Ellipsis), items: Sub::Pending { key: "more", fallback: None }, click: None },
        ]);
        // a cloud or WSL place can be taken off the sidebar on its own (it comes back from Settings > Files and folders)
        if self.places.read(cx).cloud.iter().any(|c| c.path == path) {
            let at = items.len() - 2;
            let key = path.to_string_lossy().into_owned();
            items.insert(
                at,
                Item::action(t!("Bu konumu gizle"), MenuIcon::Lucide(L::EyeOff), move |_, cx| {
                    let key = key.clone();
                    crate::settings::update(cx, move |s| {
                        if !s.hidden_places.iter().any(|h| h.eq_ignore_ascii_case(&key)) {
                            s.hidden_places.push(key);
                        }
                    })
                }),
            );
        }
        // a drive can be taken off the lists of places (it comes back from Settings > Files and folders)
        if let Some(letter) = crate::settings_view::drive_letter(&path) {
            let at = items.len() - 2;
            items.insert(
                at,
                Item::action(t!("Bu sürücüyü gizle"), MenuIcon::Lucide(L::EyeOff), move |_, cx| crate::settings::update(cx, move |s| crate::settings_view::set_drive_hidden(s, letter, true))),
            );
        }
        // the row of quick buttons the file list's menu has (issue 53: "Can't do basic functions like copy, cut, delete, rename for
        // the folders at the left pane"). A drive's root can be pasted into, not cut, renamed or deleted.
        let quick = {
            use crate::fluent_menu::Quick;
            let clip = |kind: rf_win::fileops::Transfer| {
                let p = path.clone();
                move |_: &mut Window, _: &mut App| {
                    let p = p.clone();
                    rf_win::sta::run(move |owner| {
                        if let Err(e) = rf_win::clipboard::set_files(owner, &[p], kind) {
                            log::warn!("clipboard: {e:#}");
                        }
                    });
                }
            };
            vec![
                Quick::new(L::Scissors, t!("Kes  (Ctrl+X)"), !root, clip(rf_win::fileops::Transfer::Move)),
                Quick::new(L::Copy, t!("Kopyala  (Ctrl+C)"), true, clip(rf_win::fileops::Transfer::Copy)),
                Quick::new(L::ClipboardPaste, t!("Yapıştır  (Ctrl+V)"), rf_win::clipboard::can_paste(), with(|ws, p, w, cx| {
                    ws.active_tab().clone().update(cx, |v, cx| v.paste_to(p, w, cx))
                })),
                Quick::new(L::TextCursorInput, t!("Yeniden adlandır  (F2)"), !root, with(|ws, p, w, cx| ws.rename_place(p, w, cx))),
                Quick::new(
                    L::Share2,
                    t!("Paylaş"),
                    true,
                    with(|_, p, w, cx| {
                        crate::share::open(crate::look::window::hwnd_of(w), vec![p], cx);
                    }),
                ),
                Quick::new(L::Trash, t!("Sil  (Del)"), !root, with(|ws, p, w, cx| {
                    ws.active_tab().clone().update(cx, |v, cx| v.delete_paths(vec![p], false, w, cx))
                })),
                Quick::new(
                    L::Wrench,
                    t!("Özellikler  (Alt+Enter)"),
                    true,
                    with(|ws, p, _, cx| {
                        crate::properties_view::open(vec![p], ws.images.clone(), cx)
                    }),
                ),
            ]
        };
        let menu = if cx.global::<crate::settings::Settings>().windows_file_explorer_theme {
            crate::fluent_menu::show_context_original_icons(Menu { quick, items }, at, window, cx)
        } else {
            crate::fluent_menu::show(Menu { quick, items }, at, window, cx)
        };
        let view = self.active_tab().downgrade();
        crate::ctx_menu::load_shell_more(menu.downgrade(), rf_win::ctxmenu::ShellTarget::One(path), view, cx);
    }

    /// Renames a folder of the sidebar or the folder tree (its menu's Rename, issue 53): asks the new name, renames it off the
    /// interface's thread, and keeps what pointed at it (a pin, its tags, a tab inside it) pointing at it.
    fn rename_place(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(old) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
        let me = cx.entity().downgrade();
        crate::prompt::show(
            t!("Yeniden adlandır"),
            None,
            Some((old.clone(), String::new())),
            vec![
                crate::prompt::Button::new(t!("Yeniden adlandır"), crate::prompt::Kind::Primary, move |text, _, cx| {
                    let name = text.unwrap_or_default().trim().to_string();
                    const INVALID: &[char] = &['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
                    if name.is_empty() || name.contains(INVALID) || name.ends_with('.') {
                        return Err(t!("Dosya adı şu karakterleri içeremez: \\ / : * ? \" < > | ve nokta ile bitemez.").to_string());
                    }
                    if name == old {
                        return Ok(());
                    }
                    let to = path.with_file_name(&name);
                    let case_only = name.to_lowercase() == old.to_lowercase();
                    if !case_only && std::fs::symlink_metadata(&to).is_ok() {
                        return Err(t!("Bu adda bir öğe zaten var. Başka bir ad deneyin.").to_string());
                    }
                    let (from, me) = (path.clone(), me.clone());
                    let done = cx.background_spawn({
                        let (from, name) = (from.clone(), name.clone());
                        async move { rf_win::fileops::rename(&from, &name) }
                    });
                    cx.spawn(async move |cx| match done.await {
                        Ok(true) => {
                            let _ = cx.update(|cx| {
                                crate::undo::push(crate::undo::UndoOp::Renamed { from: from.clone(), to: to.clone() });
                                crate::tags::update(cx, |t| t.moved(&from, &to));
                                let _ = me.update(cx, |ws, cx| ws.place_renamed(&from, &to, cx));
                            });
                        }
                        Ok(false) => {}
                        Err(e) => log::warn!("rename: {e:#}"),
                    })
                    .detach();
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
    }

    /// After a sidebar folder was renamed: its pin follows it, and so does every tab that showed it or a folder inside it.
    fn place_renamed(&mut self, from: &std::path::Path, to: &std::path::Path, cx: &mut Context<Self>) {
        let pins = self.current_pins(cx);
        if let Some(list) = crate::places::pins_after(&pins, from, Some(to)) {
            crate::settings::update(cx, |s| s.pinned = Some(list));
        }
        self.places.update(cx, |p, cx| p.refresh(cx));
        for t in &self.tabs {
            for v in std::iter::once(&t.left).chain(t.right.as_ref()) {
                let moved = match v.read(cx).location() {
                    Location::Path(p) => p.strip_prefix(from).ok().map(|rest| to.join(rest)),
                    _ => None,
                };
                if let Some(now) = moved {
                    v.update(cx, |v, cx| v.navigate(Location::Path(now), cx));
                }
            }
        }
    }

    /// Folders dropped on the sidebar's pin area get pinned (files are ignored, as in Explorer).
    fn pin_dropped(&mut self, paths: &ExternalPaths, cx: &mut Context<Self>) {
        self.sidebar_ext_drag = false;
        let dirs: Vec<PathBuf> = paths.paths().iter().filter(|p| p.is_dir()).cloned().collect();
        if dirs.is_empty() {
            return;
        }
        let mut list = self.current_pins(cx);
        for d in dirs {
            if !list.contains(&d) {
                list.push(d);
            }
        }
        crate::settings::update(cx, |s| s.pinned = Some(list));
        self.places.update(cx, |p, cx| p.refresh(cx));
    }

    fn set_pinned(&mut self, path: PathBuf, pin: bool, cx: &mut Context<Self>) {
        let mut list = self.current_pins(cx);
        if !pin {
            list.retain(|p| *p != path);
        } else if !list.contains(&path) {
            list.push(path);
        }
        crate::settings::update(cx, |s| s.pinned = Some(list));
        self.places.update(cx, |p, cx| p.refresh(cx));
    }

    /// Moves tab `ix` into a new window (tab dragged onto the content area).
    fn detach_tab(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.tabs.len() < 2 || ix >= self.tabs.len() {
            return;
        }
        let loc = self.tabs[ix].left.read(cx).location().clone();
        self.close_tab(ix, window, cx);
        let mut b = window.bounds();
        b.origin.x += px(48.);
        b.origin.y += px(48.);
        cx.defer(move |cx| crate::open_main_window(Some(loc), None, Some(WindowBounds::Windowed(b)), cx));
    }

    /// Right-click on Back/Forward: the history as a menu.
    fn show_history_menu(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let tab = self.active_tab().clone();
        let titles: Vec<String> = {
            let v = tab.read(cx);
            let it: Box<dyn Iterator<Item = &Location>> =
                if back { Box::new(v.history.back_entries()) } else { Box::new(v.history.forward_entries()) };
            it.take(16)
                .map(|l| match l {
                    Location::Path(p) => p.to_string_lossy().into_owned(),
                    Location::DiskUsage(p) => crate::disk_usage::title(p),
                    other => crate::i18n::td(&other.title()),
                })
                .collect()
        };
        if titles.is_empty() {
            return;
        }
        use crate::fluent_menu::{Item, Menu, MenuIcon};
        let weak = tab.downgrade();
        let items: Vec<Item> = titles
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let weak = weak.clone();
                Item::action(t, MenuIcon::Lucide(L::Folder), move |_, cx| {
                    let _ = weak.update(cx, |v, cx| v.jump(back, i + 1, cx));
                })
            })
            .collect();
        crate::fluent_menu::show(Menu { quick: Vec::new(), items }, window.mouse_position(), window, cx);
    }

    fn nav_button(
        &self,
        id: &'static str,
        icon: L,
        enabled: bool,
        p: &Palette,
        cx: &mut Context<Self>,
        f: impl Fn(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        #[cfg(target_os = "windows")]
        let windows_nav_icon = cx
            .global::<crate::settings::Settings>()
            .windows_file_explorer_theme
            .then(|| crate::icons::windows_theme_icon_glyph(icon))
            .flatten()
            .map(|glyph| crate::icons::windows_theme_glyph(glyph, px(16.)));
        #[cfg(not(target_os = "windows"))]
        let windows_nav_icon = None;
        div()
            .id(id)
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tooltip_text(id)).build(w, cx))
            .size(px(32.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_SM)
            .text_color(if enabled { p.text } else { p.text_tertiary })
            .when(enabled, |d| {
                d.hover(|s| s.bg(p.hover))
                    .active(|s| s.bg(p.pressed))
                    .on_click(cx.listener(move |this, _, window, cx| f(this, window, cx)))
            })
            .child(windows_nav_icon.unwrap_or_else(|| Icon::new(icon).size(px(16.)).into_any_element()))
            .into_any_element()
    }

    /// Navigation row: back / forward / up / refresh, the Omnibar (one box for path, search and
    /// commands, like Files) and the Status Center button.
    fn render_toolbar(&mut self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let popups_open = self.popups_open(cx);
        let explorer_look = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let tab = self.active_tab().read(cx);
        let can_back = tab.history.can_back();
        let can_fwd = tab.history.can_forward();
        let loc = tab.location().clone();
        let can_up = loc.parent().is_some();
        // The Omnibar shows the search box only while it is being typed in; afterwards the path
        // comes back and a small chip keeps the active query visible (and clearable).
        let searching = self.search_mode;
        let live_query = self.search.read(cx).value().to_string();
        let active_query: Option<String> = match &loc {
            Location::Search { query, .. } => Some(query.clone()),
            _ => (!live_query.is_empty()).then_some(live_query),
        };

        let suggestions = if self.search_mode { self.render_search_history(p, cx) } else { self.render_suggestions(p, cx) };
        let field: AnyElement = if searching {
            Input::new(&self.search).appearance(false).cleanable(true).into_any_element()
        } else if self.editing_address {
            Input::new(&self.address).appearance(false).into_any_element()
        } else {
            self.render_breadcrumb(&loc, p, cx).into_any_element()
        };
        let focused = self.editing_address || searching;
        let chip = active_query.filter(|_| !searching).map(|q| {
            h_flex()
                .id("search-chip")
                .flex_shrink_0()
                .max_w(px(220.))
                .h(px(24.))
                .pl_2()
                .gap_1()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .bg(p.selection)
                .text_xs()
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Aramayı düzenle")).build(w, cx))
                .child(Icon::new(L::Search).size(px(12.)).text_color(p.accent))
                .child(div().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(q))
                .child(
                    div()
                        .id("search-chip-x")
                        .size(px(22.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme::RADIUS_SM)
                        .hover(|s| s.bg(p.hover))
                        .child(Icon::new(L::X).size(px(11.)))
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_click(cx.listener(|this, _, window, cx| this.exit_search(window, cx))),
                )
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(|this, _, window, cx| this.enter_search(window, cx)))
        });
        let mode_btn = |id: &'static str, icon: L, on: bool, tip: &'static str, cx: &mut Context<Self>, f: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            div()
                .id(id)
                .size(px(28.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SM)
                .text_color(if on { p.accent } else { p.text_secondary })
                .when(on, |d| d.bg(p.selection))
                .hover(|s| s.bg(p.hover))
                .when(!popups_open, |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(crate::i18n::t(tip)).build(w, cx)))
                .child(Icon::new(icon).size(px(15.)))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, window, cx| f(this, window, cx)))
        };
        let omnibar = h_flex()
            .id("omnibar")
            .flex_1()
            .min_w(px(160.))
            .h(px(if explorer_look { 40. } else { 36. }))
            .mx_1()
            .pl_2()
            .pr_1()
            .gap_1()
            .items_center()
            .when(explorer_look, |d| d.rounded(theme::RADIUS))
            .when(!explorer_look, |d| d.rounded_full())
            .bg(p.content_bg)
            .border_1()
            .border_color(if focused { p.accent } else { p.border })
            .relative()
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| {
                if this.search_mode && ev.keystroke.key == "escape" {
                    this.exit_search(window, cx);
                    cx.stop_propagation();
                    return;
                }
                if this.search_mode {
                    crate::rtl::handle_key_down(&this.search, ev, cx);
                } else if this.editing_address {
                    crate::rtl::handle_key_down(&this.address, ev, cx);
                }
                if !this.search_mode {
                    this.address_key(ev, window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &gpui_kit::component::input::IndentInline, window, cx| this.accept_suggestion(window, cx)))
            .on_action(cx.listener(|this, _: &gpui_kit::component::input::OutdentInline, window, cx| {
                if this.editing_address {
                    this.cycle_focus(false, window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .when(searching, |d| d.child(Icon::new(L::Search).size(px(14.)).text_color(p.text_secondary)))
            .child(div().flex_1().min_w_0().overflow_hidden().child(field))
            .children(suggestions)
            .children(chip)
            .child(div().w(px(1.)).h(px(18.)).mx_1().bg(p.divider))
            .child(mode_btn("omni-cmd", L::SquareChevronRight, self.palette.is_some(), "Komut paleti  (Ctrl+Shift+P)", cx, |this, window, cx| {
                this.toggle_command_palette(window, cx)
            }))
            .child(mode_btn("omni-search", L::Search, searching, "Ara  (Ctrl+F)", cx, |this, window, cx| this.enter_search(window, cx)))
            .on_click(cx.listener(|this, _, window, cx| {
                if !this.editing_address && !this.search_mode {
                    this.begin_address_edit(window, cx);
                }
            }));

        h_flex()
            .h(px(if explorer_look { 60. } else { 48. }))
            .when(explorer_look, |d| d.bg(p.address_bg))
            .px_2()
            .gap(if explorer_look { px(16.) } else { px(4.) })
            .items_center()
            .child(
                div()
                    .flex_shrink_0()
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.show_history_menu(true, window, cx)))
                    .child(self.nav_button("back", L::ArrowLeft, can_back, p, cx, |this, _, cx| this.with_tab(cx, |v, cx| v.back(cx)))),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, window, cx| this.show_history_menu(false, window, cx)))
                    .child(self.nav_button("fwd", L::ArrowRight, can_fwd, p, cx, |this, _, cx| this.with_tab(cx, |v, cx| v.forward(cx)))),
            )
            .child(self.nav_button("up", L::ArrowUp, can_up, p, cx, |this, _, cx| this.with_tab(cx, |v, cx| v.up(cx))))
            .child(self.nav_button("refresh", L::RotateCw, true, p, cx, |this, _, cx| this.with_tab(cx, |v, cx| v.refresh(cx))))
            .child(omnibar)
            .child(crate::status_center::render_button(&self.ops, p, cx))
    }

    fn enter_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_mode = true;
        self.editing_address = false;
        self.clear_suggestions();
        self.search.update(cx, |s, cx| s.focus(window, cx));
        cx.notify();
    }

    fn exit_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_mode = false;
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        if let Some(tab) = self.tabs.get(self.active).map(|t| t.view().clone()) {
            tab.update(cx, |v, cx| {
                v.set_query(String::new(), cx);
                // leave a recursive search result back to its folder
                if let Location::Search { root, .. } = v.location().clone() {
                    v.navigate(Location::Path(root), cx);
                }
            });
        }
        self.focus_active(window, cx);
        cx.notify();
    }

    /// Command bar above the file list (Files' toolbar): create, clipboard and item commands on
    /// the left; sort, view and the preview pane on the right.
    fn render_commandbar(&mut self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        use crate::fluent_menu::{Item, Menu, MenuIcon};

        let tab = self.active_tab().read(cx);
        let in_folder = tab.location().path().is_some();
        let (sel, _, _) = tab.selection_summary();
        let has_sel = sel > 0;
        // a phone's folder is read-only here: changing commands are greyed out, not refused later
        let writable_sel = has_sel && !matches!(tab.location(), Location::Shell { .. });
        let preview_open = self.preview_open;
        let windows_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let command_width = if windows_theme && self.viewport_w >= 1200. { px(46.) } else { px(36.) };
        let sort_label = (windows_theme && self.viewport_w >= 1200.).then_some(("Sırala", "Sort"));
        let view_label = (windows_theme && self.viewport_w >= 1200.).then_some(("Görünüm", "View"));
        let filter_on = self.filter_bar;
        let paste_ok = in_folder && rf_win::clipboard::can_paste();
        let toolbar_icon = |icon: L| {
            #[cfg(target_os = "windows")]
            if windows_theme {
                if let Some(glyph) = crate::icons::windows_theme_icon_glyph(icon) {
                    return crate::icons::windows_theme_glyph(glyph, px(16.));
                }
            }
            Icon::new(icon).size(px(16.)).into_any_element()
        };
        let preview_icon = toolbar_icon(L::PanelRight);
        // only while the folder is shown in groups (issue 181)
        let (has_groups, all_folded) = (tab.has_groups(), tab.all_groups_folded());

        let cmd = |id: &'static str, icon: L, tip: &'static str, enabled: bool, cx: &mut Context<Self>, run: fn(&mut Self, &mut Window, &mut Context<Self>)| {
            div()
                .id(id)
                .h(px(32.))
                .w(command_width)
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SM)
                .text_color(if enabled { p.text } else { p.text_tertiary })
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(crate::i18n::t(tip)).build(w, cx))
                .when(enabled, |d| {
                    d.hover(|s| s.bg(p.hover))
                        .active(|s| s.bg(p.pressed))
                        .on_click(cx.listener(move |this, _, window, cx| run(this, window, cx)))
                })
                .child(toolbar_icon(icon))
        };
        let popups_open = self.popups_open(cx);
        let drop = |id: &'static str, icon: L, label: Option<(&'static str, &'static str)>, tip: &'static str, enabled: bool, cx: &mut Context<Self>, which: fn(&Self, &App) -> crate::ctx_menu::BarMenu| {
            h_flex()
                .id(id)
                .h(px(32.))
                .px_2()
                .gap_1()
                .flex_shrink_0()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .text_sm()
                .text_color(if enabled { p.text } else { p.text_tertiary })
                .when(!popups_open, |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(crate::i18n::t(tip)).build(w, cx)))
                .child(toolbar_icon(icon))
                .children(label.map(|l| {
                    div().child(if windows_theme { crate::i18n::tx(l.0, l.1) } else { crate::i18n::t(l.0) })
                }))
                .child(div().text_color(p.text_secondary).child(toolbar_icon(L::ChevronDown)))
                .when(enabled, |d| {
                    d.hover(|s| s.bg(p.hover)).active(|s| s.bg(p.pressed)).on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                        let menu = which(this, cx);
                        // drop down from under the button (the bar's bottom edge)
                        let bars = if windows_theme { 60. + 58. } else { 48. + 44. };
                        let pos = point(ev.position().x - px(14.), theme::TITLEBAR_H + px(bars));
                        this.active_tab().clone().update(cx, |v, cx| v.show_bar_menu(menu, pos, window, cx));
                    }))
                })
        };
        let sep = || div().w(px(1.)).h(px(20.)).mx_1().bg(p.divider);
        // which buttons the user wants (Settings > Appearance > Toolbar buttons)
        let (hidden, extras, commands) = {
            let st = cx.global::<crate::settings::Settings>();
            (st.toolbar_hidden.clone(), st.toolbar_extra.clone(), st.toolbar_commands.clone())
        };
        let on = |k: &str| !hidden.iter().any(|h| h == k);
        let extra = |k: &str| extras.iter().any(|h| h == k);
        let mut inline_commands = Vec::new();
        let mut menu_commands = Vec::new();
        for (i, command) in commands.into_iter().enumerate() {
            let picture = command.icon_path()
                .and_then(|path| self.images.update(cx, |cache, cx| cache.shell_icon(&path, 32, cx)));
            if command.in_menu {
                let label = command.menu_label().to_owned();
                let icon = picture.map_or(MenuIcon::Lucide(L::SquareTerminal), MenuIcon::Image);
                let view = self.active_tab().downgrade();
                menu_commands.push(Item::action(label, icon, move |window, cx| {
                    if let Some(view) = view.upgrade() {
                        let view = view.read(cx);
                        crate::quick_commands::run(command.clone(), view.current_dir(), view.selection_paths(), window, cx);
                    }
                }));
            } else {
                inline_commands.push((i, command, picture));
            }
        }
        let menu_name = crate::i18n::tx("Özel komutlar", "Custom commands");
        let bar_border = if !windows_theme || p.dark || self.glass_shown { p.divider } else { gpui_kit::rgb(0xDDE2E6).into() };

        h_flex()
            .h(px(if windows_theme { 58. } else { 44. }))
            .when(windows_theme, |d| d.bg(p.toolbar_bg))
            .when(windows_theme, |d| d.px_1())
            .when(!windows_theme, |d| d.px_2())
            .gap_0p5()
            .items_center()
            .when(windows_theme, |d| d.border_t_1())
            .border_b_1()
            .border_color(bar_border)
            .children(on("new").then(|| drop("cb-new", L::CirclePlus, Some(("Yeni", "New")), "Yeni öğe", in_folder, cx, |_, _| crate::ctx_menu::BarMenu::New)))
            .children(extra("newfolder").then(|| {
                cmd("cb-newfolder", L::FolderPlus, "Yeni klasör  (Ctrl+Shift+N)", in_folder, cx, |_, w, cx| w.dispatch_action(Box::new(NewFolder), cx))
            }))
            .children(on("new").then(sep))
            .children(on("cut").then(|| {
                cmd("cb-cut", L::Scissors, "Kes  (Ctrl+X)", writable_sel, cx, |this, w, cx| {
                    this.with_tab(cx, |v, cx| v.copy_to_clipboard(rf_win::fileops::Transfer::Move, w, cx))
                })
            }))
            .children(on("copy").then(|| {
                cmd("cb-copy", L::Copy, "Kopyala  (Ctrl+C)", has_sel, cx, |this, w, cx| {
                    this.with_tab(cx, |v, cx| v.copy_to_clipboard(rf_win::fileops::Transfer::Copy, w, cx))
                })
            }))
            .children(on("paste").then(|| cmd("cb-paste", L::ClipboardPaste, "Yapıştır  (Ctrl+V)", paste_ok, cx, |this, w, cx| this.with_tab(cx, |v, cx| v.paste(w, cx)))))
            .children(on("rename").then(|| {
                cmd("cb-rename", L::TextCursorInput, "Yeniden adlandır  (F2)", writable_sel, cx, |this, w, cx| this.with_tab(cx, |v, cx| v.rename_selection(w, cx)))
            }))
            .children(on("share").then(|| cmd("cb-share", L::Share2, "Paylaş", writable_sel, cx, |this, w, cx| this.with_tab(cx, |v, cx| v.share(w, cx)))))
            .children(on("delete").then(|| {
                cmd("cb-delete", L::Trash, "Sil  (Del)", writable_sel, cx, |this, w, cx| this.with_tab(cx, |v, cx| v.delete_selected(false, w, cx)))
            }))
            .when(!windows_theme, |d| d.children(on("props").then(|| {
                cmd("cb-props", L::Wrench, "Özellikler  (Alt+Enter)", in_folder, cx, |this, _, cx| {
                    this.with_tab(cx, |v, cx| if v.selection_paths().is_empty() { v.folder_properties(cx) } else { v.properties(cx) })
                })
            })))
            .children(extra("terminal").then(|| cmd("cb-terminal", L::SquareTerminal, "Terminalde aç  (Ctrl+Shift+`)", in_folder, cx, |_, w, cx| w.dispatch_action(Box::new(OpenTerminal), cx))))
            .children(extra("hidden").then(|| cmd("cb-hidden", L::Eye, "Gizli öğeleri göster/gizle  (Ctrl+H)", in_folder, cx, |_, w, cx| w.dispatch_action(Box::new(ToggleHidden), cx))))
            .children(extra("refresh").then(|| cmd("cb-refresh", L::RotateCw, "Yenile  (F5)", true, cx, |_, w, cx| w.dispatch_action(Box::new(Refresh), cx))))
            .when(!windows_theme, |d| d.children(on("more").then(|| {
                drop("cb-more", L::Ellipsis, None, "Diğer", in_folder, cx, |_, _| crate::ctx_menu::BarMenu::More)
            })))
            .children((!inline_commands.is_empty() || !menu_commands.is_empty()).then(sep))
            .when(!inline_commands.is_empty(), |d| {
                d.child(
                    h_flex().id("cb-quick-commands").min_w_0().overflow_x_scroll().gap_1().children(
                        inline_commands.into_iter().map(|(i, command, picture)| {
                            let tip = command.tooltip_text().to_owned();
                            let label = command.button_label().map(str::to_owned);
                            let icon = if let Some(picture) = picture {
                                img(picture).size(px(18.)).object_fit(ObjectFit::Contain).flex_shrink_0().into_any_element()
                            } else {
                                Icon::new(L::SquareTerminal).size(px(18.)).flex_shrink_0().into_any_element()
                            };
                            div()
                                .id(("cb-command", i))
                                .role(Role::Button)
                                .aria_label(tip.clone())
                                .h(px(32.))
                                .max_w(px(160.))
                                .flex_shrink_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .rounded(theme::RADIUS_SM)
                                .text_sm()
                                .tab_index(0)
                                .cursor_pointer()
                                .hover(|s| s.bg(p.hover))
                                .active(|s| s.bg(p.pressed))
                                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
                                .when(label.is_none(), |d| d.w(command_width).justify_center())
                                .child(icon)
                                .children(label.map(|label| div().truncate().child(label)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    let view = this.active_tab().read(cx);
                                    crate::quick_commands::run(command.clone(), view.current_dir(), view.selection_paths(), window, cx);
                                }))
                        }),
                    ),
                )
            })
            .when(!menu_commands.is_empty(), |d| {
                d.child(
                    h_flex().id("cb-quick-menu")
                        .role(Role::Button).aria_label(menu_name)
                        .h(px(32.)).px_2().gap_1().flex_shrink_0().items_center()
                        .rounded(theme::RADIUS_SM).tab_index(0).cursor_pointer()
                        .hover(|s| s.bg(p.hover)).active(|s| s.bg(p.pressed))
                        .when(!popups_open, |d| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(menu_name).build(w, cx)))
                        .child(Icon::new(L::Layers).size(px(16.)))
                        .child(Icon::new(L::ChevronDown).size(px(12.)).text_color(p.text_secondary))
                        .on_click(move |ev, window, cx| {
                            let bars = if windows_theme { 60. + 58. } else { 48. + 44. };
                            let pos = point(ev.position().x - px(14.), theme::TITLEBAR_H + px(bars));
                            crate::fluent_menu::show(Menu { quick: Vec::new(), items: menu_commands.clone() }, pos, window, cx);
                        }),
                )
            })
            .when(windows_theme, |d| {
                d.children(on("sort").then(sep))
                    .children(on("sort").then(|| drop("cb-sort", L::ArrowUpDown, sort_label, "Sırala ve gruplandır", in_folder, cx, |_, _| crate::ctx_menu::BarMenu::Sort)))
                    .children(on("view").then(|| {
                        drop("cb-view", L::LayoutGrid, view_label, "Görünüm", true, cx, move |this, _| crate::ctx_menu::BarMenu::View { dual: this.tabs[this.active].right.is_some(), shelf: this.shelf_open })
                    }))
                    .children(on("more").then(|| drop("cb-more", L::Ellipsis, None, "Diğer", in_folder, cx, |_, _| crate::ctx_menu::BarMenu::More)))
                    .children(on("props").then(|| {
                        cmd("cb-props", L::Wrench, "Özellikler  (Alt+Enter)", in_folder, cx, |this, _, cx| {
                            this.with_tab(cx, |v, cx| if v.selection_paths().is_empty() { v.folder_properties(cx) } else { v.properties(cx) })
                        })
                    }))
            })
            .child(div().flex_1())
            .when(has_groups, |d| {
                let tip = if all_folded { t!("Tüm grupları genişlet") } else { t!("Tüm grupları daralt") };
                d.child(
                    div()
                        .id("cb-groups")
                        .h(px(32.))
                        .w(px(36.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme::RADIUS_SM)
                        .hover(|s| s.bg(p.hover))
                        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(format!("{tip}  (Ctrl+Shift+G)")).build(w, cx))
                        .on_click(cx.listener(|this, _, _, cx| {
                            let tab = this.active_tab().clone();
                            tab.update(cx, |v, cx| v.toggle_all_groups(cx));
                        }))
                        .child(Icon::new(if all_folded { L::ChevronsUpDown } else { L::ChevronsDownUp }).size(px(16.))),
                )
            })
            .children(on("filter").then(|| {
                div()
                    .id("cb-filter")
                    .h(px(32.))
                    .w(command_width)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Filtrele")).build(w, cx))
                    .when(filter_on, |d| d.bg(p.selection).text_color(p.accent))
                    .when(!filter_on, |d| d.hover(|s| s.bg(p.hover)))
                    .when(in_folder, |d| d.on_click(cx.listener(|this, _, window, cx| this.toggle_filter_bar(window, cx))))
                    .when(!in_folder, |d| d.text_color(p.text_tertiary))
                    .child(toolbar_icon(L::ListFilter))
            }))
            .when(!windows_theme, |d| {
                d.children(on("sort").then(|| drop("cb-sort", L::ArrowUpDown, None, "Sırala ve gruplandır", in_folder, cx, |_, _| crate::ctx_menu::BarMenu::Sort)))
                    .children(on("view").then(|| {
                        drop("cb-view", L::LayoutGrid, None, "Görünüm", true, cx, move |this, _| crate::ctx_menu::BarMenu::View { dual: this.tabs[this.active].right.is_some(), shelf: this.shelf_open })
                    }))
            })
            .children(on("preview").then(sep))
            .children(on("preview").then(|| {
                div()
                    .id("cb-preview")
                    .h(px(32.))
                    .w(command_width)
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Önizleme bölmesi  (Alt+P)")).build(w, cx))
                    .when(preview_open, |d| d.bg(p.selection).text_color(p.accent))
                    .when(!preview_open, |d| d.hover(|s| s.bg(p.hover)))
                    .child(preview_icon)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_preview(cx);
                    }))
            }))
    }

    fn render_breadcrumb(&mut self, loc: &Location, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let drop_bg = p.selection;
        // (the row fills the box; its own width, measured as it is laid out, is the room the segments have)
        let room = self.crumb_room.clone();
        let mut row = h_flex().flex_1().min_w_0().relative().gap_0p5().items_center().text_sm().overflow_hidden().child(
            canvas(
                move |bounds, window, _| {
                    let w = f32::from(bounds.size.width);
                    if (room.get() - w).abs() > 1.0 {
                        room.set(w);
                        window.refresh();
                    }
                },
                |_, _, _, _| (),
            )
            .absolute()
            .size_full(),
        );
        let crumbs: Vec<(String, Location)> = match loc {
            Location::Path(path) | Location::Search { root: path, .. } => breadcrumbs(path),
            // the shell's own name for the folder (the location may carry none)
            Location::Shell { .. } => vec![(self.active_tab().read(cx).title(cx), loc.clone())],
            Location::DiskUsage(path) => vec![(crate::disk_usage::title(path), loc.clone())],
            other => vec![(crate::i18n::td(&other.title()), other.clone())],
        };
        // Too long for the address box: hide leading segments after the drive behind "…"
        // (the current folder must stay visible). Widths are estimated from text length.
        let measured = self.crumb_room.get();
        let budget = if measured > 0. { measured + 30. } else { self.viewport_w - 260. - 4. * 36. - 9. * 36. - 90. };
        let est = |l: &str| l.chars().count() as f32 * 7.4 + 30.;
        let mut crumbs = crumbs;
        let mut hidden: Option<Location> = None;
        while crumbs.len() > 2 && crumbs.iter().map(|(l, _)| est(l)).sum::<f32>() + 60. > budget {
            let (_, loc) = crumbs.remove(1);
            hidden = Some(loc);
        }
        let n = crumbs.len();
        let windows_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let home_icon = div()
            .id("crumb-home")
            .px_1()
            .py_0p5()
            .rounded(theme::RADIUS_SM)
            .hover(|s| s.bg(p.hover))
            .child(if windows_theme {
                crate::icons::windows_home_icon(px(14.))
            } else {
                Icon::new(L::House).size(px(14.)).text_color(p.text_secondary).into_any_element()
            })
            .on_click(cx.listener(|this, _, window, cx| {
                cx.stop_propagation();
                this.navigate(Location::Home, window, cx);
            }));
        row = row.child(home_icon);
        let chevron = |id: (&'static str, usize), dir: Option<PathBuf>, p: &Palette, cx: &mut Context<Self>| {
            div()
                .id(id)
                .px_0p5()
                .py_1()
                .rounded(theme::RADIUS_SM)
                .hover(|s| s.bg(p.hover))
                .child(Icon::new(L::ChevronRight).size(px(12.)).text_color(p.text_tertiary))
                .on_click(cx.listener(move |this, _, window, cx| {
                    cx.stop_propagation();
                    this.crumb_menu(dir.clone(), window, cx);
                }))
        };
        let mut prev_dir: Option<PathBuf> = None;
        let last_dir = crumbs.last().and_then(|(_, t)| t.path().map(Path::to_path_buf));
        for (i, (label, target)) in crumbs.into_iter().enumerate() {
            if i == 1 {
                if let Some(h) = hidden.clone() {
                    // "…" stands for the hidden segments and opens the deepest of them
                    row = row.child(chevron(("crumb-chev-h", 0), prev_dir.clone(), p, cx));
                    prev_dir = h.path().map(Path::to_path_buf);
                    row = row.child(
                        div()
                            .id("crumb-ellipsis")
                            .px_1p5()
                            .py_0p5()
                            .rounded(theme::RADIUS_SM)
                            .text_color(p.text_secondary)
                            .hover(|s| s.bg(p.hover).text_color(p.text))
                            .child("…")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.navigate(h.clone(), window, cx);
                            })),
                    );
                }
            }
            row = row.child(chevron(("crumb-chev", i), prev_dir.clone(), p, cx));
            prev_dir = target.path().map(Path::to_path_buf);
            let last = i + 1 == n;
            row = row.child(
                div()
                    .id(("crumb", i))
                    .px_1p5()
                    .py_0p5()
                    .rounded(theme::RADIUS_SM)
                    .whitespace_nowrap()
                    .when(last, |d| d.font_weight(FontWeight::MEDIUM))
                    .when(!last, |d| d.text_color(p.text_secondary))
                    .hover(|s| s.bg(p.hover).text_color(p.text))
                    .child(crate::rtl::display_text(&label))
                    .when_some(target.path().map(Path::to_path_buf), |d, dir| {
                        d.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg))
                            .on_drag_move::<ExternalPaths>(crate::folder_view::drop_effect_over(dir.clone()))
                            .on_drop(move |paths: &ExternalPaths, _, cx| drop_files(paths, dir.clone(), cx))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.navigate(target.clone(), window, cx);
                    })),
            );
        }
        if let Some(d) = last_dir {
            row = row.child(chevron(("crumb-chev", 999), Some(d), p, cx));
        }
        row
    }

    fn sidebar_item(
        &self,
        id: ElementId,
        icon: L,
        icon_color: Hsla,
        label: String,
        target: Location,
        active: bool,
        pin_indicator: bool,
        extra: Option<AnyElement>,
        p: &Palette,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let target2 = target.clone();
        let target3 = target.clone();
        let drop_dir = target.path().map(Path::to_path_buf);
        let drop_bg = p.selection;
        let drop_border = p.accent;
        // on the rail only the icon; hovering the rail opens the full sidebar over the content
        let rail = self.sidebar_rail;
        // Over glass the selected row is a lens the engine draws behind the scene: no fill and no accent bar, but a probe that reports
        // the row's place on every paint (a scroll or a resize moves the lens with it). Two rows can be "selected" at once (a folder
        // that is pinned and also a library): the first gets the lens, the other keeps the Fluent selection.
        let lens_key = (active && self.sidebar_lens && !self.sidebar_lens_claimed.replace(true)).then(|| crate::glass::key_of(&target));
        let fluent = active && lens_key.is_none();
        // drives look alike as bare icons: on the rail their letter sits in the icon's corner
        let letter = target3.path().filter(|_| rail).and_then(|p| {
            let s = p.to_str()?;
            let b = s.as_bytes();
            (b.len() <= 3 && b.len() >= 2 && b[1] == b':' && b[0].is_ascii_alphabetic()).then(|| (b[0] as char).to_ascii_uppercase())
        });
        let windows_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let windows_home = windows_theme && matches!(&target, Location::Home);
        let recycle_icon_watch = windows_theme && matches!(&target, Location::RecycleBin);
        let shell_icon = if windows_theme {
            let icon_px = if rail { 18 } else { 16 };
            match &target {
                Location::Home => None,
                Location::RecycleBin => {
                    let state = self.places.read(cx).recycle_full.map_or(0, |full| if full { 2 } else { 1 });
                    self.images.update(cx, |images, cx| {
                        images.shell_icon_state("::{645ff040-5081-101b-9f08-00aa002f954e}", icon_px, state, cx)
                    })
                }
                _ => {
                    let parsing = target
                        .path()
                        .map(|path| path.to_string_lossy().into_owned())
                        .or_else(|| match &target {
                            Location::Shell { parsing, .. } => Some(parsing.clone()),
                            _ => None,
                        });
                    parsing.and_then(|path| {
                        self.images.update(cx, |images, cx| {
                            images.shell_icon(&path, icon_px, cx)
                        })
                    })
                }
            }
        } else {
            None
        };
        let tag_dot = target3.path().filter(|p| p.parent().is_some()).and_then(|p| crate::tags::first_color(p, cx));
        h_flex()
            .id(id)
            // A row of the scrolling sidebar never shrinks: squeezed to its least height, a drive's second line ran into the next row.
            .flex_shrink_0()
            .min_h(px(if windows_theme || rail { 36. } else { 32. }))
            .when(windows_theme, |d| d.pl(if rail { px(8.) } else { px(24.) }).pr_2())
            .when(!windows_theme, |d| d.px_2())
            .py_1()
            .gap(if windows_theme { px(4.) } else { px(8.) })
            .items_center()
            .when(rail, |d| d.justify_center())
            .rounded(theme::RADIUS_SM)
            .text_sm()
            .relative()
            .when(fluent, |d| d.bg(p.selection).child(theme::selection_pill(p)))
            // while the row moves (the sidebar scrolls) the scene paints the pill itself: the engine's lens would trail the row
            .when(lens_key.is_some() && crate::glass::lens_phase(self.glass_hwnd, crate::glass::SLOT_SIDEBAR) != crate::glass::LensPhase::Rest, |d| {
                d.child(crate::glass::lens_stand_in(p.dark, 32.))
            })
            .when_some(lens_key, |d, key| d.child(crate::glass::lens_probe(crate::glass::SLOT_SIDEBAR, key)))
            .when(!active, |d| d.hover(|s| s.bg(p.hover)))
            .child(
                div()
                    .relative()
                    .flex_shrink_0()
                    .child(match shell_icon {
                        Some(image) => img(image).size(px(if rail { 18. } else { 16. })).into_any_element(),
                        None if windows_home => crate::icons::windows_home_icon(px(if rail { 18. } else { 16. })),
                        None => Icon::new(icon).size(px(if rail { 18. } else { 16. })).text_color(icon_color).into_any_element(),
                    })
                    .when_some(tag_dot, |d, c| d.child(crate::tags::corner_dot(c, p.card_bg)))
                    .when_some(letter, |d, l| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(-9.))
                                .bottom(px(-7.))
                                .min_w(px(15.))
                                .h(px(15.))
                                .px(px(2.))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(4.))
                                .bg(p.sidebar_bg)
                                .border_1()
                                .border_color(p.border)
                                .text_size(px(11.))
                                .line_height(px(13.))
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.text)
                                .child(l.to_string()),
                        )
                    }),
            )
            .when(!rail, |d| {
                d.child(
                    v_flex()
                        .flex_1()
                        .overflow_hidden()
                        .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
                        .children(extra),
                )
            })
            .when(pin_indicator && !rail && windows_theme, |d| {
                d.child(Icon::new(L::Pin).size(px(16.)).text_color(p.text_tertiary).flex_shrink_0())
            })

            .on_click(cx.listener(move |this, _, window, cx| this.navigate(target.clone(), window, cx)))
            .when(recycle_icon_watch, |d| {
                d.on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                    this.places.update(cx, |places, cx| places.set_recycle_icon_hovered(*hovered, cx));
                }))
            })
            .when_some(target3.path().map(Path::to_path_buf), |d, path| {
                d.on_mouse_down(
                    MouseButton::Right,
                    cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.sidebar_menu(path.clone(), window, cx);
                    }),
                )
            })
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                    let keep = this.active;
                    this.open_tab(target2.clone(), window, cx);
                    this.activate(keep, window, cx);
                }),
            )
            .when_some(drop_dir, |d, dir| {
                d.drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg).border_1().border_color(drop_border))
                    .on_drag_move::<ExternalPaths>(crate::folder_view::drop_effect_over(dir.clone()))
                    .on_drop(move |paths: &ExternalPaths, _, cx| drop_files(paths, dir.clone(), cx))
            })
    }

    fn render_sidebar(&mut self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let changes = crate::addons::CHANGES.load(std::sync::atomic::Ordering::Relaxed);
        if changes != self.hd_seen {
            self.hd_seen = changes;
            self.hd_installed = crate::addons::installed(crate::hyperdownload::ID).is_some();
        }
        let cur = self.active_tab().read(cx).location().clone();
        let places = self.places.read(cx);
        let pinned = places.pinned.clone();
        let drives = places.drives.clone();
        // (the pins and drives are read off the UI thread and are there a moment after the window: see the tags below)
        let places_loaded = places.loaded;
        let rail = self.sidebar_rail;
        let explorer_look = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        let section_divider = if p.dark { p.divider } else { gpui_kit::rgb(0xD1D1D1).into() };
        let scrollbar_track = if p.dark { gpui_kit::rgb(0x303030) } else { gpui_kit::rgb(0xF0F0F0) };
        let scrollbar_thumb = if p.dark { gpui_kit::rgb(0x606060) } else { gpui_kit::rgb(0xB8B8B8) };
        let scrollbar_thumb_hover = if p.dark { gpui_kit::rgb(0xA0A0A0) } else { gpui_kit::rgb(0x5E5E5E) };
        let sidebar_scroll = self.side_tree.scroll_handle().clone();
        let sidebar_scrollable = sidebar_scroll.max_offset().y > px(0.);
        let header = |t: &'static str| {
            if rail {
                let d = div().mx_2().my_2().h(px(1.));
                return if explorer_look { d.flex_shrink_0().bg(section_divider) } else { d.bg(p.divider) };
            }
            let d = div()
                .px_2()
                .pt_3()
                .pb_1()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(p.text_tertiary)
                .child(t);
            if explorer_look { d.flex_shrink_0() } else { d }
        };
        // the "drop here to pin" row lives as long as a file drag does
        if !cx.has_active_drag() {
            self.sidebar_ext_drag = false;
        }
        let ext_drag = self.sidebar_ext_drag;
        let (drop_bg, drop_border) = (p.selection, p.accent);
        let mut col = v_flex()
            .id("sidebar")
            .w_full()
            .flex_1()
            .min_h_0()
            .gap_0p5()
            .overflow_y_scroll()
            .track_scroll(&sidebar_scroll)
            .when(explorer_look, |d| d.pl(px(2.)).pr_2().py_2().scrollbar_width(px(20.)))
            .when(!explorer_look, |d| d.px_2().py_1())
            .on_drag_move::<ExternalPaths>(cx.listener(|this, _: &DragMoveEvent<ExternalPaths>, _, cx| {
                if !this.sidebar_ext_drag {
                    this.sidebar_ext_drag = true;
                    cx.notify();
                }
            }));
        // Home and the pinned folders. With the folder tree on they stand above the sidebar's scrolling part and stay in view while the
        // tree is scrolled (issue 177: "the Pinned section is pushed out of view when the user scrolls down the folder tree"); a long
        // list of pins scrolls on its own, in at most 45% of the sidebar.
        let mut head = v_flex()
            .id("sidebar-pins")
            .w_full()
            .gap_0p5()
            .when(explorer_look, |d| d.flex_shrink_0())
            .on_drag_move::<ExternalPaths>(cx.listener(|this, _: &DragMoveEvent<ExternalPaths>, _, cx| {
                if !this.sidebar_ext_drag {
                    this.sidebar_ext_drag = true;
                    cx.notify();
                }
            }));
        head = head.child(self.sidebar_item(
            "sb-home".into(),
            L::House,
            p.accent,
            t!("Ana Sayfa").into(),
            Location::Home,
            cur == Location::Home,
            false,
            None,
            p,
            cx,
        ));
        if !rail && explorer_look {
            head = head.child(div().mx_2().my_2().h(px(1.)).flex_shrink_0().bg(section_divider));
        }
        // dropping folders on the header pins them
        head = head.child(
            div()
                .id("sb-pin-header")
                .flex_shrink_0()
                .rounded(theme::RADIUS_SM)
                .child(header(t!("Sabitlenenler")))
                .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg))
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.pin_dropped(paths, cx))),
        );
        let accent = p.accent;
        // the open folder is already a selected row of the sidebar (a pinned folder or a drive): the tree draws no second selection
        let mut shown_elsewhere = pinned.iter().any(|f| cur == Location::Path(f.path.clone()));
        for (i, f) in pinned.into_iter().enumerate() {
            let loc = Location::Path(f.path.clone());
            let active = cur == loc;
            let title = crate::i18n::td(&f.title);
            let drag = PinDrag { ix: i, path: f.path.clone(), title: title.clone().into() };
            let target_path = f.path.clone();
            let item = self.sidebar_item(("sb-pin", i).into(), f.icon, p.accent, title, loc, active, true, None, p, cx);
            // drag a pin onto another to reorder
            head = head.child(
                div()
                    .id(("sb-pin-wrap", i))
                    .when(explorer_look, |d| d.flex_shrink_0())
                    .on_drag(drag, |d: &PinDrag, _, _, cx| cx.new(|_| d.clone()))
                    .drag_over::<PinDrag>(move |s, d: &PinDrag, _, _| {
                        // moving down inserts after the target, moving up before it
                        if d.ix < i { s.border_b_2().border_color(accent) } else { s.border_t_2().border_color(accent) }
                    })
                    .on_drop(cx.listener(move |this, d: &PinDrag, _, cx| this.move_pin(&d.path, &target_path, cx)))
                    .child(item),
            );
        }
        if ext_drag {
            head = head.child(
                h_flex()
                    .id("sb-pin-drop")
                    .min_h(px(32.))
                    .px_2()
                    .gap_2()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .border_1()
                    .border_dashed()
                    .border_color(p.divider)
                    .text_sm()
                    .text_color(p.text_tertiary)
                    .child(Icon::new(L::Pin).size(px(16.)).flex_shrink_0())
                    .when(!rail, |d| d.child(t!("Sabitlemek için buraya bırakın")))
                    .drag_over::<ExternalPaths>(move |s, _, _, _| s.bg(drop_bg).border_color(drop_border))
                    .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.pin_dropped(paths, cx))),
            );
        }
        let pins_apart = self.shows_tree(rail) && !explorer_look;
        if explorer_look && !pins_apart {
            head = head.child(div().mx_2().my_2().h(px(1.)).flex_shrink_0().bg(section_divider));
        }
        let pins_top = if pins_apart {
            Some(
                head.px_2()
                    .pt_1()
                    .pb_1()
                    .flex_shrink_0()
                    .max_h(relative(0.45))
                    .overflow_y_scroll()
                    .border_b_1()
                    .border_color(if explorer_look { section_divider } else { p.divider }),
            )
        } else {
            col = col.child(head);
            None
        };
        // the folder tree stands right under the pinned folders, above the drives and the tags (where the user asked for it: the
        // tree is what one works in, the drives and tags are looked at less often)
        let hidden = cx.global::<crate::settings::Settings>().sidebar_hidden.clone();
        let shown = |k: &str| !hidden.iter().any(|h| h == k);
        // (with the folder tree on, the drives are the tree's roots: the user found the second list of them needless)
        let drives = if shown("drives") && !self.shows_tree(rail) { drives } else { Vec::new() };
        shown_elsewhere |= drives.iter().any(|d| cur == Location::Path(d.root.clone()));
        col = self.append_tree(col, &cur, shown_elsewhere, rail, p, cx);
        if !drives.is_empty() {
            col = col.child(header(t!("Sürücüler")));
        }
        let space_numbers = cx.global::<crate::settings::Settings>().sidebar_drive_space;
        for (i, d) in drives.into_iter().enumerate() {
            let loc = Location::Path(d.root.clone());
            let active = cur == loc;
            let frac = d.used_fraction();
            let bar = (d.total > 0).then(|| {
                let line = div()
                    .mt(px(5.))
                    .h(px(2.))
                    .w_full()
                    .rounded_full()
                    .bg(p.pressed)
                    .child(div().h_full().rounded_full().bg(if frac > 0.9 { p.danger } else { p.accent }).w(relative(frac)));
                if !space_numbers {
                    return line.into_any_element();
                }
                // (issue 167: "together with the visual size and usage bars, also numbers (example "238/500 GB")")
                v_flex()
                    .w_full()
                    .child(line)
                    .child(div().mt(px(2.)).text_xs().text_color(p.text_tertiary).whitespace_nowrap().child(drive_space_text(d.total, d.free)))
                    .into_any_element()
            });
            col = col.child(self.sidebar_item(
                ("sb-drive", i).into(),
                places::drive_icon(&d),
                p.text_secondary,
                crate::places::drive_name(&d),
                loc,
                active,
                false,
                bar,
                p,
                cx,
            ));
        }
        // phones and cameras under This PC (issue 49: "The phone also doesn't show up as a location in sidebar"); they are no roots of
        // the folder tree, so they are listed with the tree on too
        let devices = if shown("drives") { self.places.read(cx).devices.clone() } else { Vec::new() };
        if !devices.is_empty() {
            col = col.child(header(t!("Cihazlar")));
            for (i, d) in devices.into_iter().enumerate() {
                let loc = Location::Shell { parsing: d.parsing.clone(), name: d.name.clone() };
                let active = cur == loc;
                col = col.child(self.sidebar_item(("sb-dev", i).into(), L::Usb, p.text_secondary, d.name.clone(), loc, active, false, None, p, cx));
            }
        }
        let cloud = visible_places(self.places.read(cx).cloud.clone(), &cx.global::<crate::settings::Settings>().hidden_places);
        if !cloud.is_empty() && shown("cloud") {
            col = col.child(header(t!("Bulut ve Linux")));
            for (i, c) in cloud.into_iter().enumerate() {
                let loc = Location::Path(c.path.clone());
                let active = cur == loc;
                col = col.child(self.sidebar_item(("sb-cloud", i).into(), c.icon, p.accent, crate::i18n::td(&c.title), loc, active, false, None, p, cx));
            }
        }
        let libraries = self.places.read(cx).libraries.clone();
        // Libraries duplicate the pinned folders: off unless turned on in Settings
        if !libraries.is_empty() && cx.global::<crate::settings::Settings>().show_libraries {
            col = col.child(header(t!("Kütüphaneler")));
            for (i, l) in libraries.into_iter().enumerate() {
                let loc = Location::Library { file: l.file.clone(), name: l.name.clone() };
                let active = cur == loc;
                col = col.child(self.sidebar_item(("sb-lib", i).into(), L::Library, p.accent, l.name, loc, active, false, None, p, cx));
            }
        }
        let network = self.places.read(cx).network.clone();
        if !network.is_empty() && shown("network") {
            col = col.child(header(t!("Ağ")));
            for (i, (name, target)) in network.into_iter().enumerate() {
                let loc = Location::Path(target);
                let active = cur == loc;
                col = col.child(self.sidebar_item(("sb-net", i).into(), L::Network, p.text_secondary, name, loc, active, false, None, p, cx));
            }
        }
        // The tags are known at once, the pins and drives above them a moment later: drawn before those, the coloured tag rows stood
        // at the top of the sidebar for the first 90 ms of a cold start and then jumped down (seen frame by frame: a user's report of
        // a red and blue flash when a folder is opened with the app closed). They wait for the places.
        let tag_defs = if shown("tags") && places_loaded { cx.global::<crate::tags::Tags>().0.defs.clone() } else { Vec::new() };
        if !tag_defs.is_empty() {
            col = col.child(header(t!("Etiketler")));
        }
        for d in tag_defs {
            let loc = Location::Tag(d.id);
            let active = cur == loc;
            col = col.child(self.sidebar_item(
                ("sb-tag", d.id as usize).into(),
                L::Tag,
                crate::tags::color(d.color),
                crate::i18n::td(&d.name),
                loc,
                active,
                false,
                None,
                p,
                cx,
            ));
        }
        col = col.child(div().h_2().when(explorer_look, |d| d.flex_shrink_0()));
        // HyperDownload's page, when the add-on is installed (above the Recycle Bin)
        if self.hd_installed {
            col = col.child(self.sidebar_item(
                "sb-downloads".into(),
                L::ArrowDownToLine,
                crate::downloads_view::ember(),
                "HyperDownload".into(),
                Location::Downloads,
                cur == Location::Downloads,
                false,
                None,
                p,
                cx,
            ));
        }
        col = col.child(self.sidebar_item(
            "sb-recycle".into(),
            L::Trash,
            p.text_secondary,
            t!("Geri Dönüşüm Kutusu").into(),
            Location::RecycleBin,
            cur == Location::RecycleBin,
            false,
            None,
            p,
            cx,
        ));
        let settings = self.sidebar_item(
            "sb-settings".into(),
            L::Settings,
            p.text_secondary,
            t!("Ayarlar").into(),
            Location::Settings,
            cur == Location::Settings,
            false,
            None,
            p,
            cx,
        );
        // Files: Settings stays at the bottom of the sidebar, under a divider
        v_flex()
            .w(px(if self.sidebar_rail { RAIL_W } else { self.sidebar_w }))
            .h_full()
            .when(explorer_look, |d| d.bg(p.sidebar_bg))
            .child(
                v_flex()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .children(pins_top)
                    .child(col)
                    .when(explorer_look, |d| d.child(
                        div()
                            .id("sidebar-scrollbar-track")
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .right_0()
                            .w(px(20.))
                            .bg(scrollbar_track)
                            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                                if this.sidebar_scroll_hovered != *hovered {
                                    this.sidebar_scroll_hovered = *hovered;
                                    cx.notify();
                                }
                            }))
                            .when(
                                !sidebar_scrollable,
                                |d| {
                                    d.child(
                                        div()
                                            .absolute()
                                            .top(px(14.))
                                            .right(if self.sidebar_scroll_hovered { px(6.) } else { px(7.) })
                                            .w(px(if self.sidebar_scroll_hovered { 8. } else { 6. }))
                                            .h(relative(0.35))
                                            .rounded_full()
                                            .bg(if self.sidebar_scroll_hovered {
                                                scrollbar_thumb_hover
                                            } else {
                                                scrollbar_thumb
                                            }),
                                    )
                                },
                            )
                            .child(
                                gpui_kit::component::scroll::Scrollbar::vertical(&sidebar_scroll)
                                    .id(if rail { "sidebar-scrollbar-rail" } else { "sidebar-scrollbar" })
                                    .mode(gpui_kit::component::scroll::ScrollbarMode::Always)
                                    .viewport_from_layout()
                                    .styles(|styles| {
                                        styles
                                            .track(|track| track.width(px(20.)).bg(scrollbar_track.into()))
                                            .track_hover(|track| track.width(px(20.)).bg(scrollbar_track.into()))
                                            .track_active(|track| track.width(px(20.)).bg(scrollbar_track.into()))
                                            .thumb(|thumb| thumb.width(px(6.)).inset(px(7.)).bg(scrollbar_thumb))
                                            .thumb_hover(|thumb| thumb.width(px(8.)).inset(px(6.)).bg(scrollbar_thumb_hover))
                                            .thumb_active(|thumb| thumb.width(px(8.)).inset(px(6.)).bg(scrollbar_thumb_hover))
                                    }),
                            )
                            .when(self.sidebar_scroll_hovered, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(14.))
                                        .bg(scrollbar_track),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(14.))
                                        .bg(scrollbar_track),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(14.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(Icon::new(L::ChevronUp).size(px(10.)).text_color(p.text_tertiary)),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .bottom_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(14.))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(Icon::new(L::ChevronDown).size(px(10.)).text_color(p.text_tertiary)),
                                )
                            }),
                    )),
            )
            .child(div().mx_2().h(px(1.)).when(explorer_look, |d| d.flex_shrink_0()).bg(p.divider))
            .child(div().px_2().py_1().child(settings))
    }

    fn render_statusbar(&mut self, p: &Palette, strip_bar_w: Option<f32>, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let tab = self.active_tab().read(cx);
        let count = tab.item_count();
        let (sel, sel_size, has_dir) = tab.selection_summary();
        let loading = tab.loading;
        let ms = tab.load_ms;
        let in_folder = tab.location().path().is_some() || matches!(tab.location(), Location::Tag(_) | Location::Library { .. });
        let git = tab.git.clone();
        // Settings → Developer tools → "Open in editor" button
        let ide_button = {
            let prefs = cx.global::<crate::settings::Settings>();
            let show = match prefs.ide_button {
                crate::settings::IdePref::Always => true,
                crate::settings::IdePref::GitRepos => git.is_some(),
                crate::settings::IdePref::Never => false,
            };
            let dir = tab.location().path().map(Path::to_path_buf);
            let ide = prefs.ide_path.clone();
            (show && dir.is_some()).then(|| {
                let dir = dir.unwrap();
                h_flex()
                    .id("open-ide")
                    .h(px(22.))
                    .px_2()
                    .gap_1()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .hover(|s| s.bg(p.hover).text_color(p.text))
                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Klasörü düzenleyicide aç")).build(w, cx))
                    .child(Icon::new(L::Code).size(px(12.)))
                    .child(t!("Aç"))
                    .on_click(move |_, _, _| open_in_ide(dir.clone(), ide.clone()))
            })
        };
        let mut left = h_flex().gap_3();
        let mut busy = false;
        if in_folder {
            left = left.child(crate::i18n::items(count));
            if sel > 0 {
                let s = if has_dir || sel_size == 0 {
                    crate::i18n::items_selected(sel)
                } else {
                    format!("{} · {}", crate::i18n::items_selected(sel), format::size(sel_size))
                };
                left = left.child(div().text_color(p.text).child(s));
            }
            if loading {
                // a search says which step it is on (issue 67)
                let text = tab.search_stage.map_or_else(|| t!("Yükleniyor…").to_string(), |s| s.text().to_string());
                left = left.child(text);
                busy = true;
            }
            // values being read for a sort or grouping by a property column
            if let Some(s) = tab.prop_status() {
                left = left.child(s);
                busy = true;
            }
        }
        // a running operation's strip, given what else this bar holds (it takes the room that is left)
        let strip = strip_bar_w.and_then(|bar_w| {
            let held = rf_core::transfer::BarContents {
                items: in_folder,
                selected: in_folder && sel > 0,
                busy,
                git: git.is_some(),
                ide: ide_button.is_some(),
                update: matches!(crate::installer::state(), crate::installer::State::Ready(_)),
            };
            crate::transfer_view::strip(&self.ops, p, rf_core::transfer::strip_room(bar_w, held), cx)
        });
        let perf = cx.global::<crate::settings::Settings>().show_perf_info;
        h_flex()
            .h(px(32.))
            .flex_shrink_0()
            .px_3()
            .items_center()
            .justify_between()
            .border_t_1()
            .border_color(p.divider)
            .text_xs()
            .text_color(p.text_secondary)
            .child(left)
            // a running operation's progress (Settings → Aktarım ilerlemesi → Durum çubuğu)
            .children(strip)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .children(git.map(|g| {
                        // Files-style git controls: ↑↓ (sync menu) and the branch (branch menu)
                        let view = self.active_tab().downgrade();
                        let (root, ahead, behind) = (g.root.clone(), g.ahead, g.behind);
                        let pill = |id: &'static str| {
                            h_flex()
                                .id(id)
                                .h(px(22.))
                                .px_2()
                                .gap_1()
                                .items_center()
                                .rounded(theme::RADIUS_SM)
                                .hover(|s| s.bg(p.hover).text_color(p.text))
                                .active(|s| s.bg(p.pressed))
                        };
                        let (v1, r1) = (view.clone(), root.clone());
                        let changes = (!g.is_clean()).then(|| {
                            div().text_color(p.text_tertiary).child(format!("{} {}", g.files.len(), t!("değişiklik")))
                        });
                        h_flex()
                            .gap_1()
                            .items_center()
                            .children(changes)
                            .child(
                                pill("git-sync")
                                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Çek / Gönder / Eşitle")).build(w, cx))
                                    .child(Icon::new(L::ArrowDownUp).size(px(12.)))
                                    .child(format!("{behind} / {ahead}"))
                                    .on_click(move |ev: &ClickEvent, window, cx| {
                                        crate::git_ui::sync_menu(v1.clone(), r1.clone(), ahead, behind, ev.position(), window, cx)
                                    }),
                            )
                            .child(
                                pill("git-branch")
                                    .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Dallar")).build(w, cx))
                                    .child(Icon::new(L::GitBranch).size(px(12.)))
                                    .child(g.branch.clone())
                                    .on_click(move |ev: &ClickEvent, window, cx| {
                                        crate::git_ui::branch_menu(view.clone(), root.clone(), ev.position(), window, cx)
                                    }),
                            )
                    }))
                    .children(ide_button)
                    // a downloaded update: one click restarts into it
                    .children(matches!(crate::installer::state(), crate::installer::State::Ready(_)).then(|| {
                        h_flex()
                            .id("update-ready")
                            .h(px(22.))
                            .px_2()
                            .gap_1()
                            .items_center()
                            .rounded(theme::RADIUS_SM)
                            .text_color(p.accent)
                            .hover(|s| s.bg(p.hover))
                            .child(Icon::new(L::RefreshCw).size(px(12.)))
                            .child(t!("Güncelleme hazır"))
                            .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Yeniden başlat ve güncelle")).build(w, cx))
                            .on_click(|_, _, _| {
                                std::thread::spawn(crate::installer::restart_to_update);
                            })
                    }))
                    .children(ms.filter(|_| in_folder && perf).map(|ms| format!("{ms} ms")))
                    .children(perf.then(|| render_backend_badge(p))),
            )
    }
}

/// Title bar: width of the leading corner button, tab width range.
const TITLEBAR_LEAD: f32 = 32.;
/// New tab: the "+" box grows to a tab, then the next "+" slides out from behind it.
const TAB_ANIM_MS: u64 = 200;
const PLUS_ANIM_MS: u64 = 160;
/// Dragged files held over a tab this long open it (Explorer uses about the same).
const DRAG_HOVER_MS: u64 = 500;
const TAB_MIN_W: f32 = 72.;
const TAB_MAX_W: f32 = 220.;
/// A tab's width besides its name: the padding, the icon and its gap, the close button and its gap.
const TAB_CHROME_W: f32 = 12. + 14. + 8. + 8. + 22. + 4. + 2.;
/// The terminal button and its gap, on a tab that has one.
const TAB_TERMINAL_W: f32 = 22. + 8.;
const SIDEBAR_MIN: f32 = 160.;
/// The icons-only sidebar, and the width below which a drag collapses to it.
const RAIL_W: f32 = 56.;
/// The full sidebar's way out of the rail: the spring's time and how far it swings past its place (a little: it is a panel, not a
/// toy); and the time of its way back in, which has no swing.
const RAIL_OUT_MS: u64 = 240;
const RAIL_BOUNCE: f32 = 0.18;
const RAIL_IN_MS: u64 = 130;

/// The width of the sidebar that is coming out of the rail, for how far out it is (0: the rail's own width, 1: the full width;
/// past 1 while the spring swings, by a few px). Never narrower than the rail.
fn rail_peek_width(full: f32, shown: f32) -> f32 {
    (RAIL_W + (full - RAIL_W) * shown).max(RAIL_W)
}
const RAIL_SNAP: f32 = 120.;
const SIDEBAR_MAX: f32 = 420.;
const PREVIEW_MIN: f32 = 220.;
/// The widest a saved preview width is taken. While it is dragged the pane may grow as far as the list keeps its own minimum width
/// (720 px used to be the end: too narrow to read a whole PDF page, a user asked on Threads, 2026-11-18).
const PREVIEW_MAX: f32 = 2400.;

/// Drag payloads for the pane edges.
#[derive(Clone)]
struct ResizeSidebar;
#[derive(Clone)]
struct ResizePreview;

struct ResizeGhost;
impl Render for ResizeGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// A thin invisible strip on a pane edge (`left` = on the pane's left edge) that starts a resize.
fn resize_handle<T: Clone + 'static>(id: &'static str, payload: T, left: bool) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .top_0()
        .h_full()
        .w(px(6.))
        .when(left, |d| d.left(px(-3.)))
        // the sidebar's right edge: stay inside it, the content card is painted on top
        .when(!left, |d| d.right_0())
        .cursor_col_resize()
        .on_drag(payload, |_, _, _, cx| cx.new(|_| ResizeGhost))
}

/// Toolbar tooltips (with shortcuts).
fn tooltip_text(id: &str) -> &'static str {
    match id {
        "back" => t!("Geri  (Alt+←)"),
        "fwd" => t!("İleri  (Alt+→)"),
        "up" => t!("Üst klasör  (Alt+↑)"),
        "refresh" => t!("Yenile  (F5)"),
        "hidden" => t!("Gizli öğeleri göster/gizle  (Ctrl+H)"),
        "terminal" => t!("Terminalde aç"),
        "sort" => t!("Sıralama ve gruplama"),
        "v-details" => t!("Detaylar  (Ctrl+Shift+1)"),
        "v-grid" => t!("Izgara  (Ctrl+Shift+2)"),
        "v-cols" => t!("Sütunlar  (Ctrl+Shift+3)"),
        "v-dual" => t!("Çift panel  (Ctrl+Shift+D)"),
        "v-preview" => t!("Önizleme paneli  (Alt+P)"),
        "v-shelf" => t!("Raf"),
        _ => "",
    }
}

/// Drop onto a sidebar entry / breadcrumb segment: Explorer rules (same volume = move).
/// Opens `dir` in the user's editor (`ide` path, else VS Code's `code` launcher).
pub fn open_in_ide(dir: PathBuf, ide: String) {
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let r = if ide.is_empty() {
            // "automatic": the first of the editors we know that is installed (`rf_win::editors`; looked for here, on this thread)
            match rf_win::editors::installed().first() {
                Some(editor) => std::process::Command::new(&editor.exe).arg(&dir).spawn(),
                // none of them: `code` on the PATH, as it always was (a .cmd launcher: through cmd, without a console window)
                None => std::process::Command::new("cmd").args(["/c", "code", "."]).current_dir(&dir).creation_flags(CREATE_NO_WINDOW).spawn(),
            }
        } else {
            std::process::Command::new(&ide).arg(&dir).spawn()
        };
        if let Err(e) = r {
            log::warn!("open in editor: {e}");
        }
    });
}

pub(crate) fn drop_files(paths: &ExternalPaths, dest: PathBuf, cx: &mut App) {
    let sources: Vec<PathBuf> = paths.paths().to_vec();
    if sources.is_empty() || rf_win::dnd::is_invalid_drop(&sources, &dest) {
        return;
    }
    let (ctrl, shift) = unsafe {
        use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_CONTROL, VK_SHIFT};
        (GetAsyncKeyState(VK_CONTROL.0 as i32) < 0, GetAsyncKeyState(VK_SHIFT.0 as i32) < 0)
    };
    let sources = hold_archiver_drop(sources, &std::env::temp_dir());
    let kind = crate::ops::drop_transfer(&sources, &dest, ctrl, shift);
    crate::status_center::transfer(cx, kind, sources, dest);
}

/// Archivers hand a drop the files they extracted into a folder of their own under TEMP (7-Zip `7zE…`, WinRAR `Rar$DRa…`) and delete
/// that folder as soon as the drop returns. Our transfer starts after it (and after a "replace?" question) and found nothing: "The
/// system cannot find the file specified" (issue 139, 131). So while the drop is still going on such files are kept in rustyfiles'
/// own folder under TEMP, hard-linked (instant on the same volume, and the file lives on when the archiver deletes its name) or else
/// copied, and the transfer takes them from there. Anything else is returned as it was: a user's own files are never touched.
fn hold_archiver_drop(sources: Vec<PathBuf>, temp: &Path) -> Vec<PathBuf> {
    // (TEMP can be written short, `C:\Users\OZERO~1\...`, by one side and long by the other: both are compared in their long form)
    let long = |p: &Path| -> PathBuf {
        std::fs::canonicalize(p).map_or_else(|_| p.to_path_buf(), |c| PathBuf::from(c.to_string_lossy().trim_start_matches(r"\\?\").to_string()))
    };
    let temp_long = long(temp);
    let from_archiver = |p: &Path| {
        let p_long = long(p);
        [(p, temp), (p_long.as_path(), temp_long.as_path())].iter().any(|(p, t)| {
            p.strip_prefix(t).ok().and_then(|rest| rest.components().next()).is_some_and(|c| {
                let n = c.as_os_str().to_string_lossy().to_ascii_lowercase();
                n.starts_with("7z") || n.starts_with("rar$")
            })
        })
    };
    if sources.is_empty() || !sources.iter().all(|p| from_archiver(p)) {
        return sources;
    }
    let base = temp.join("rustyfiles-drops");
    // (the drops of earlier days: whatever a transfer left there, links that cost no room, or copies)
    if let Ok(old) = std::fs::read_dir(&base) {
        for e in old.flatten() {
            if e.metadata().and_then(|m| m.modified()).is_ok_and(|t| t.elapsed().is_ok_and(|a| a > std::time::Duration::from_secs(24 * 3600))) {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
    let here = base.join(stamp.to_string());
    let mut held = Vec::with_capacity(sources.len());
    for src in &sources {
        let Some(name) = src.file_name() else { return sources };
        let to = here.join(name);
        if let Err(e) = keep_tree(src, &to) {
            log::warn!("drop from an archiver: could not keep {}: {e}; the transfer takes it from where it is", src.display());
            return sources;
        }
        held.push(to);
    }
    log::info!("drop from an archiver: {} item(s) kept in {}", held.len(), here.display());
    held
}

/// `from` (a file or a folder) at `to`: files hard-linked, else copied; folders made and filled.
fn keep_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            keep_tree(&e.path(), &to.join(e.file_name()))?;
        }
        return Ok(());
    }
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::hard_link(from, to).or_else(|_| std::fs::copy(from, to).map(|_| ()))
}

#[cfg(test)]
mod archiver_drop_tests {
    use super::hold_archiver_drop;

    #[test]
    fn an_archivers_extracted_files_are_kept_after_it_deletes_them_and_other_files_are_left_alone() {
        let temp = std::env::temp_dir().join(format!("rf-archdrop-{}", std::process::id()));
        let seven = temp.join("7zE5A1B2C3D");
        std::fs::create_dir_all(seven.join("folder")).unwrap();
        std::fs::write(seven.join("doc.txt"), b"inside the archive").unwrap();
        std::fs::write(seven.join("folder").join("deep.txt"), b"deeper").unwrap();
        let held = hold_archiver_drop(vec![seven.join("doc.txt"), seven.join("folder")], &temp);
        assert!(held.iter().all(|p| p.starts_with(temp.join("rustyfiles-drops"))), "{held:?}");
        // the archiver deletes its folder when the drop returns: what the transfer will read is still there
        std::fs::remove_dir_all(&seven).unwrap();
        assert_eq!(std::fs::read(&held[0]).unwrap(), b"inside the archive");
        assert_eq!(std::fs::read(held[1].join("deep.txt")).unwrap(), b"deeper");
        // a user's own files (even under TEMP) go to the transfer as they are
        let own = temp.join("mine").join("notes.txt");
        std::fs::create_dir_all(own.parent().unwrap()).unwrap();
        std::fs::write(&own, b"x").unwrap();
        assert_eq!(hold_archiver_drop(vec![own.clone()], &temp), vec![own.clone()]);
        // and so does a drop that mixes the two
        assert_eq!(hold_archiver_drop(vec![own.clone(), temp.join("7zEother").join("a")], &temp).len(), 2);
        let _ = std::fs::remove_dir_all(&temp);
    }
}

/// Shows which device draws the window (hybrid renderer) and cycles Auto → GPU → CPU on click.
fn render_backend_badge(p: &Palette) -> impl IntoElement + use<> {
    use gpui_windows::hybrid::{self, RendererMode};
    let mode = hybrid::mode();
    let backend = hybrid::current_backend_label();
    let mode_label = match mode {
        RendererMode::Auto => t!("Oto"),
        RendererMode::Gpu => t!("Sabit"),
        RendererMode::Cpu => t!("Sabit"),
    };
    let is_cpu = backend == "CPU";
    h_flex()
        .id("renderer-badge")
        .px_2()
        .py_0p5()
        .gap_1()
        .items_center()
        .rounded_full()
        .bg(p.hover)
        .hover(|s| s.bg(p.pressed))
        .cursor_pointer()
        .child(Icon::new(if is_cpu { L::Cpu } else { L::Gauge }).size(px(12.)).text_color(if is_cpu { p.danger } else { p.accent }))
        .child(format!("{backend} · {mode_label}"))
        .on_click(|_, _, cx| {
            use crate::settings::RendererPref;
            let next = match hybrid::mode() {
                RendererMode::Auto => RendererPref::Gpu,
                RendererMode::Gpu => RendererPref::Cpu,
                RendererMode::Cpu => RendererPref::Auto,
            };
            crate::settings::update(cx, |s| s.renderer = next);
        })
}

/// "C:\Users\x" → [("C:", C:\), ("Users", C:\Users), ("x", C:\Users\x)]
fn breadcrumbs(path: &Path) -> Vec<(String, Location)> {
    let mut out = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        if matches!(comp, std::path::Component::RootDir) {
            // merge the root separator into the drive crumb
            if let Some(last) = out.last_mut() {
                let (_, loc): &mut (String, Location) = last;
                *loc = Location::Path(acc.clone());
            }
            continue;
        }
        // (the cache only: this runs while drawing; FolderView::warm_labels fills it from a worker)
        let label = match rf_win::localized::label_if_known(&acc) {
            Some(l) => l.to_string(),
            None => rf_core::history::redact_credentials(comp.as_os_str().to_string_lossy().trim_end_matches('\\')),
        };
        out.push((label, Location::Path(acc.clone())));
    }
    out
}

fn parse_location(text: &str) -> Option<Location> {
    let t = text.trim().trim_matches('"');
    if t.is_empty() {
        return None;
    }
    let lower = t.to_lowercase();
    if lower == "home" || lower == "ana sayfa" {
        return Some(Location::Home);
    }
    // Environment variables like %USERPROFILE%
    let expanded = expand_env(t);
    let mut p = PathBuf::from(&expanded);
    if expanded.starts_with(crate::aliases::PREFIX) {
        // "@name\sub": a folder alias; unknown names fall through as a plain path
        if let Some(r) = crate::aliases::resolve(&expanded) {
            p = PathBuf::from(r);
        }
    }
    if expanded.len() == 2 && expanded.ends_with(':') {
        p = PathBuf::from(format!("{expanded}\\"));
    }
    Some(Location::Path(p))
}

fn expand_env(s: &str) -> String {
    if let Some(r) = crate::aliases::resolve(s) {
        return r;
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(end) = after.find('%') {
            let name = &after[..end];
            match std::env::var(name) {
                Ok(v) => out.push_str(&v),
                Err(_) => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        } else {
            out.push_str(&rest[start..]);
            rest = "";
        }
    }
    out.push_str(rest);
    out
}

impl Workspace {
    /// Gives this window the glass (the window is already set transparent). A failure to even start is treated like a failure later:
    /// the window goes to Mica and the settings page says why.
    fn start_glass(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        let hwnd = crate::look::window::hwnd_of(window);
        self.glass_gen = self.glass_gen.wrapping_add(1);
        let attached = hwnd != 0 && rf_win::glass::attach(hwnd, dark);
        if !attached {
            let why = match rf_win::glass::state(hwnd) {
                rf_win::glass::GlassState::Failed(why) => why,
                _ => "the window has no handle".into(),
            };
            self.glass_hwnd = hwnd;
            self.give_up_glass(why, cx);
            return;
        }
        self.glass_hwnd = hwnd;
        self.glass_dark = Some(dark);
        // this is a window the user works in: its glass may freeze while it is not in front, if the setting is on (a dock island or a
        // menu never does)
        rf_win::glass::set_freezable(hwnd, true);
        // The engine reports from its own thread, so ask it: every 50 ms until it has drawn (then the window turns see-through), and
        // after that every 500 ms, so a glass that is lost later (a removed graphics device) is noticed within half a second.
        let generation = self.glass_gen;
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let Ok(shown) = this.update_in(cx, |ws, _, _| ws.glass_shown) else { break };
                let ms = if shown { 500 } else { 50 };
                cx.background_executor().timer(std::time::Duration::from_millis(ms)).await;
                let Ok(go_on) = this.update_in(cx, |ws, window, cx| ws.check_glass(generation, window, cx)) else { break };
                if !go_on {
                    break;
                }
            }
        })
        .detach();
    }

    /// One look at the engine's report. False when there is nothing more to watch.
    fn check_glass(&mut self, generation: u32, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if generation != self.glass_gen || self.glass_hwnd == 0 {
            return false;
        }
        match rf_win::glass::state(self.glass_hwnd) {
            rf_win::glass::GlassState::Failed(why) => {
                self.give_up_glass(why, cx);
                false
            }
            // taken off from elsewhere
            rf_win::glass::GlassState::Off => false,
            rf_win::glass::GlassState::Running => {
                if !self.glass_shown {
                    // the glass is on the window: now the scene may be see-through
                    self.glass_shown = true;
                    crate::glass::forget_window(self.glass_hwnd); // a glass that shows now has no lens yet: send them all again
                    crate::glass::set_shown(self.glass_hwnd, true); // the palette thins its washes over the glass
                    window.set_background_appearance(WindowBackgroundAppearance::Transparent);
                    cx.notify();
                }
                let mode = rf_win::glass::capture_mode();
                if mode != self.glass_capture {
                    self.glass_capture = mode;
                    cx.notify(); // the settings page tells whether the windows show in screenshots
                }
                true
            }
            // still starting: the window keeps its Mica
            rf_win::glass::GlassState::Starting => true,
        }
    }

    /// The glass is not kept: the engine lets go of the window, the settings page gets the reason and the next render puts Mica on. The
    /// saved setting stays; choosing it again (or another one and back) tries again.
    fn give_up_glass(&mut self, why: String, cx: &mut Context<Self>) {
        log::warn!("glass: giving up on this window, Mica is used: {why}");
        if self.glass_hwnd != 0 {
            rf_win::glass::detach(self.glass_hwnd);
            crate::glass::set_shown(self.glass_hwnd, false);
            crate::glass::forget_window(self.glass_hwnd); // the engine drops the lenses with the glass
        }
        self.glass_hwnd = 0;
        self.glass_dark = None;
        self.glass_shown = false;
        self.glass_failed = true;
        crate::glass::set_failure(Some(why));
        cx.notify();
    }

    /// Takes the glass off (the backdrop changed). Safe to call without glass.
    fn stop_glass(&mut self, window: &mut Window) {
        self.glass_gen = self.glass_gen.wrapping_add(1);
        if self.glass_hwnd != 0 {
            rf_win::glass::detach(self.glass_hwnd);
            crate::glass::set_shown(self.glass_hwnd, false);
            crate::glass::forget_window(self.glass_hwnd); // the engine drops the lenses with the glass
            self.glass_hwnd = 0;
            self.glass_dark = None;
        }
        self.glass_shown = false;
        // "Transparent" turns on a window accent policy that the Mica calls do not turn off: clear it first so the next material is clean
        window.set_background_appearance(WindowBackgroundAppearance::Opaque);
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        // the window closed
        if self.glass_hwnd != 0 {
            rf_win::glass::detach(self.glass_hwnd);
            crate::glass::set_shown(self.glass_hwnd, false);
            crate::glass::forget_window(self.glass_hwnd); // the engine drops the lenses with the glass
        }
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        {
            static FIRST: std::sync::Once = std::sync::Once::new();
            FIRST.call_once(|| crate::start_mark("first render"));
        }
        // the text size (Settings > Appearance, issue 89): the window's base size, which the texts are given in
        let rem = crate::settings::rem_for(cx.global::<crate::settings::Settings>().text_percent);
        if f32::from(window.rem_size()) != rem {
            window.set_rem_size(px(rem));
        }
        let prefs = cx.global::<crate::settings::Settings>();
        if self.sidebar_width_explorer_mode != prefs.windows_file_explorer_theme {
            self.sidebar_width_explorer_mode = prefs.windows_file_explorer_theme;
            self.sidebar_w = if self.sidebar_width_explorer_mode {
                prefs.sidebar_width_for_explorer_look()
            } else {
                prefs.sidebar_width
            }
            .clamp(SIDEBAR_MIN, SIDEBAR_MAX);
        }
        // Shortcuts (Ctrl+T, Ctrl+W, …) only reach elements on the focus path: with nothing
        // focused (a window opened from a desktop icon, a closed popup took focus with it) they
        // would do nothing. The current folder takes focus back.
        if window.focused(cx).is_none() && !self.tabs.is_empty() {
            self.focus_active(window, cx);
        }
        let p = Palette::for_window(window);
        // (the Open / Save window is matte: see `Palette::matte`)
        let p = if self.picker.is_some() { p.matte() } else { p };
        // Mica / frame follow the app's theme, not only the system one (re-applied when either
        // changes: GPUI resets it on a system theme switch)
        let prefs = cx.global::<crate::settings::Settings>();
        let (backdrop, tint, bg_image, bg_opacity, show_toolbar, show_statusbar) = (
            prefs.backdrop_in_use(),
            prefs.tint,
            prefs.background_image.clone(),
            prefs.background_opacity,
            prefs.show_toolbar,
            prefs.show_statusbar,
        );
        let windows_theme = prefs.windows_file_explorer_theme;
        // Home and Settings have nothing to count: no empty strip at the bottom
        // where a running operation's progress is shown; a strip brings the status bar up on Home and Settings too
        let surface = crate::transfer_view::surface(cx.global::<crate::settings::Settings>().transfer_progress, show_statusbar, self.picker.is_some());
        let strip_due = surface == crate::transfer_view::Surface::Strip && crate::transfer_view::strip_due(&self.ops, cx);
        let show_statusbar = show_statusbar && (strip_due || !matches!(self.active_tab().read(cx).location(), Location::Home | Location::Settings));
        use crate::settings::BackdropPref;
        // RUSTYFILES_GLASS=1 forces Glass for this run (nothing is saved); a window whose glass failed is on Mica until the setting changes
        if self.backdrop_pref_seen != Some(backdrop) {
            if self.backdrop_pref_seen.is_some() {
                // the user chose again: whatever was said about an earlier failure is old
                crate::glass::set_failure(None);
            }
            self.backdrop_pref_seen = Some(backdrop);
            self.glass_failed = false;
        }
        let backdrop = if crate::glass::forced() { BackdropPref::Glass } else { backdrop };
        let backdrop = if backdrop == BackdropPref::Glass && self.glass_failed { BackdropPref::Mica } else { backdrop };
        // (and it asks for no glass, Acrylic or tinted Mica of its own: the palette paints it; so does the pure black theme, whose
        // black must not let anything through)
        let black = crate::settings::theme_override() == crate::settings::ThemePref::Black;
        let backdrop = if self.picker.is_some() || black { BackdropPref::Solid } else { backdrop };
        if self.applied_backdrop != Some(backdrop) {
            let was_glass = self.applied_backdrop == Some(BackdropPref::Glass);
            self.applied_backdrop = Some(backdrop);
            if was_glass {
                self.stop_glass(window);
            }
            window.set_background_appearance(theme::main_window_material(match backdrop {
                BackdropPref::Mica => WindowBackgroundAppearance::MicaBackdrop,
                BackdropPref::MicaAlt => WindowBackgroundAppearance::MicaAltBackdrop,
                BackdropPref::Acrylic => WindowBackgroundAppearance::Blurred,
                // opaque surface painted by the palette; keep Mica under it so a switch back is instant
                BackdropPref::Solid => WindowBackgroundAppearance::MicaBackdrop,
                // The glass is a second DirectComposition target behind the scene, so the scene must become see-through, but only once the
                // engine has drawn (`check_glass` does that): until then, and for good if the engine gives up, the window has Mica.
                BackdropPref::Glass => WindowBackgroundAppearance::MicaBackdrop,
            }));
            if backdrop == BackdropPref::Glass {
                self.start_glass(p.dark, window, cx);
            }
        }
        // the theme changed: the glass changes its tint
        if self.glass_hwnd != 0 && self.glass_dark != Some(p.dark) {
            self.glass_dark = Some(p.dark);
            rf_win::glass::set_dark(self.glass_hwnd, p.dark);
        }
        // background color tint and picture (under everything)
        let mut backdrop_layers: Vec<AnyElement> = Vec::new();
        if let Some(c) = tint {
            let mut h: Hsla = rgb(c).into();
            h.a = if p.dark { 0.22 } else { 0.16 };
            backdrop_layers.push(div().absolute().inset_0().bg(h).into_any_element());
        }
        if let Some(path) = bg_image {
            backdrop_layers.push(
                img(path)
                    .absolute()
                    .inset_0()
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .opacity(bg_opacity.clamp(0.05, 0.6))
                    .into_any_element(),
            );
        }
        let frame_key = (p.dark, window.appearance());
        if self.frame_dark != Some(frame_key) {
            self.frame_dark = Some(frame_key);
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = window.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    rf_win::window::set_dark_mode(w.hwnd.get(), p.dark);
                }
            }
        }
        self.viewport_w = f32::from(window.viewport_size().width);
        // (the tab strip's selected tab and "+" button are lenses over glass; they claim them while the strip is built)
        let glass_on = self.glass_shown && self.glass_hwnd != 0;
        self.tab_lens = glass_on;
        self.tab_lens_claimed = false;
        self.plus_lens_claimed = false;
        let titlebar = self.render_titlebar(&p, window, cx);
        self.tab_lens = false;
        let strip_frame = crate::glass::StripFrame {
            on_screen: true,
            tab_claimed: self.tab_lens_claimed,
            plus_claimed: self.plus_lens_claimed,
        };
        let toolbar = self.render_toolbar(&p, cx);
        // the command bar acts on folder contents: not on Home, Settings, the Recycle Bin…
        let show_commandbar = matches!(self.active_tab().read(cx).location(), Location::Path(_) | Location::Search { .. } | Location::Library { .. } | Location::Tag(_) | Location::Shell { .. });
        let commandbar = (show_commandbar && show_toolbar).then(|| self.render_commandbar(&p, cx).into_any_element());
        let (top_commandbar, content_commandbar) = if windows_theme { (commandbar, None) } else { (None, commandbar) };
        let filter_strip = self.render_filter_bar(&p, window, cx);
        // Adaptive layout (as Files does): a narrow window gives the list its room first by
        // hiding the preview, then the sidebar. The settings themselves are not changed.
        const LIST_MIN: f32 = 380.;
        let side_w = if self.sidebar_rail { RAIL_W } else { self.sidebar_w };
        let show_sidebar = self.sidebar_open && self.compact.is_none() && self.viewport_w >= side_w + LIST_MIN;
        // (at the right this is the old rule, `viewport_w >= preview_w + LIST_MIN + sidebar`; at the bottom the height decides)
        let preview_bottom = self.preview_at_bottom(cx);
        let side_used = if show_sidebar { side_w } else { 0. };
        // (the height and the split's shape matter only to the bottom position: the right one reads nothing else)
        let (viewport_h, preview_stacked) = if preview_bottom { (f32::from(window.viewport_size().height), self.preview_stacked(cx)) } else { (0., false) };
        let room_for_preview = self.preview_room(preview_bottom, preview_stacked, side_used, viewport_h);
        // Lenses (the selection pills of the sidebar and of the settings list) exist only while this window shows glass. The sidebar's
        // selected row claims its lens while it is built; a frame without a claim (no selected row, hidden sidebar, the rail) takes it
        // out. The settings lens belongs to the focused pane when that shows Settings: a split tab with Settings in both panes would
        // otherwise have two probes setting one slot to two places on every frame. The rail's peek is opaque, so its rows keep the
        // Fluent selection.
        self.sidebar_lens = glass_on && !self.sidebar_rail;
        self.sidebar_lens_claimed.set(false);
        self.ensure_side_tree(window, cx);
        let sidebar = show_sidebar.then(|| self.render_sidebar(&p, cx));
        self.sidebar_lens = false;
        let plan = crate::glass::lens_plan(
            glass_on,
            show_sidebar && !self.sidebar_rail,
            self.sidebar_lens_claimed.get(),
            matches!(self.active_tab().read(cx).location(), Location::Settings),
            strip_frame,
        );
        if let Some(fade) = plan.clear_sidebar {
            crate::glass::clear_lens(self.glass_hwnd, crate::glass::SLOT_SIDEBAR, fade);
        }
        if let Some(fade) = plan.clear_tab {
            crate::glass::clear_lens(self.glass_hwnd, crate::glass::SLOT_TAB, fade);
        }
        if let Some(fade) = plan.clear_new_tab {
            crate::glass::clear_lens(self.glass_hwnd, crate::glass::SLOT_NEW_TAB, fade);
        }
        if glass_on && !plan.settings_owner {
            crate::glass::clear_lens(self.glass_hwnd, crate::glass::SLOT_SETTINGS, false);
        }
        self.assign_settings_lens(plan.settings_owner, cx);
        // The full sidebar over the content, while the pointer is on the rail or on it. It comes out of the rail on a SPRING and goes
        // back in quickly (the user, of the Open window: "buraya hızlı bir animasyon ekle ... spring animasyonu olması gerektiğini
        // unutma"): until then it was there, or not, from one frame to the next.
        let peek_wanted = show_sidebar && self.sidebar_rail && (self.rail_peek[0] || self.rail_peek[1]);
        if peek_wanted != self.rail_peek_open {
            self.rail_peek_open = peek_wanted;
            self.rail_peek_serial += 1;
            self.rail_peek_closing = !peek_wanted && crate::settings::animations();
            if self.rail_peek_closing {
                let serial = self.rail_peek_serial;
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(std::time::Duration::from_millis(RAIL_IN_MS)).await;
                    let _ = this.update(cx, |ws, cx| {
                        // (not when it was opened again meanwhile)
                        if ws.rail_peek_serial == serial {
                            ws.rail_peek_closing = false;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
        }
        let peek = (peek_wanted || self.rail_peek_closing).then(|| {
            self.sidebar_rail = false;
            self.set_tree_peek(true);
            let full = self.render_sidebar(&p, cx);
            self.set_tree_peek(false);
            self.sidebar_rail = true;
            let (full_w, opening, serial) = (self.sidebar_w, peek_wanted, self.rail_peek_serial);
            let panel = div()
                .id("rail-peek")
                .absolute()
                .top_0()
                .left_0()
                .h_full()
                .w(px(full_w))
                // (the sidebar inside keeps its full width while the panel's own width moves: nothing in it is laid out anew)
                .overflow_hidden()
                .bg(self.rail_peek_bg.unwrap_or(Hsla { a: 1., ..p.content_bg }))
                .border_r_1()
                .border_color(p.border)
                .rounded_r(theme::RADIUS)
                .shadow_lg()
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    this.rail_peek[1] = *h;
                    cx.notify();
                }))
                .child(div().h_full().w(px(full_w)).flex_shrink_0().child(full))
                // the rail's own handle lies under this panel while it is out: the panel's edge sizes the sidebar too, so a rail made by
                // dragging can be dragged wide again (a user on Discord, 2026-11-20: "now seems impossible")
                .child(resize_handle("rail-peek-resize", ResizeSidebar, false));
            if !crate::settings::animations() {
                return panel.into_any_element();
            }
            // out: a spring with a little swing past its place; in: the same curve with none, in less time
            let (ms, bounce) = if opening { (RAIL_OUT_MS, RAIL_BOUNCE) } else { (RAIL_IN_MS, 0.0) };
            panel
                .with_animation(
                    ("rail-peek-move", serial as usize),
                    Animation::new(std::time::Duration::from_millis(ms)).with_easing(move |t| rf_win::dock::spring::curve_with(t, bounce)),
                    move |el, t| {
                        let shown = if opening { t } else { 1.0 - t };
                        // (the width alone moves: the panel is matte from its first frame, nothing shows through it on the way)
                        el.w(px(rail_peek_width(full_w, shown)))
                    },
                )
                .into_any_element()
        });
        // (the status bar is the content panel's: the window less the sidebar when it is shown, and the panel's margin)
        let strip_bar_w = (surface == crate::transfer_view::Surface::Strip).then(|| (f32::from(window.viewport_size().width) - side_used - 10.0).max(0.0));
        let cards = (surface == crate::transfer_view::Surface::Card)
            .then(|| crate::transfer_view::cards(&self.ops, &p, if show_statusbar { 44.0 } else { 12.0 }, cx))
            .flatten();
        let status = self.render_statusbar(&p, strip_bar_w, cx);
        let picker_footer = self.picker.clone().map(|pk| crate::picker::footer(self, &pk, &p, window, cx));
        let star_strip = (self.picker.is_none() && crate::installer::star_due(cx.global::<crate::settings::Settings>())).then(|| {
            let action = |id: &'static str, label: &'static str, star: bool| {
                div()
                    .id(id)
                    .px_2()
                    .py_0p5()
                    .rounded(theme::RADIUS_SM)
                    .cursor_pointer()
                    .when(star, |d| d.text_color(p.accent))
                    .hover(|s| s.bg(p.hover))
                    .child(label)
                    .on_click(move |_, _, cx| {
                        if star {
                            std::thread::spawn(|| {
                                let _ = rf_win::shell::open(std::path::Path::new(crate::installer::REPO));
                            });
                        }
                        crate::settings::update(cx, |s| s.star_asked = true);
                    })
            };
            h_flex()
                .flex_shrink_0()
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(p.divider)
                .text_xs()
                .text_color(p.text_secondary)
                .child(Icon::new(L::Star).size(px(12.)).text_color(p.accent))
                .child(div().flex_1().child(t!("rustyfiles işinize yarıyor mu? GitHub'da bir yıldız, başkalarının da bulmasına yardım eder.")))
                .child(action("star-yes", t!("Yıldız ver"), true))
                .child(action("star-no", t!("Bir daha gösterme"), false))
        });
        // now and then, a note from the author; its message changes each time it is answered
        let donate_strip = (self.picker.is_none() && crate::installer::donate_due(cx.global::<crate::settings::Settings>())).then(|| {
            let s = cx.global::<crate::settings::Settings>();
            let all = crate::donate::messages(s.starts);
            let message = all[s.donate_seen as usize % all.len()].clone();
            let action = |id: &'static str, label: &'static str, kind: u8| {
                div()
                    .id(id)
                    .px_2()
                    .py_0p5()
                    .flex_shrink_0()
                    .rounded(theme::RADIUS_SM)
                    .cursor_pointer()
                    .when(kind == 0, |d| d.text_color(p.accent))
                    .hover(|s| s.bg(p.hover))
                    .child(label)
                    .on_click(move |_, _, cx| {
                        if kind == 0 {
                            std::thread::spawn(|| {
                                let _ = rf_win::shell::open(std::path::Path::new(crate::installer::DONATE));
                            });
                        }
                        crate::settings::update(cx, |s| match kind {
                            0 => crate::installer::donate_answered(s, true),
                            1 => crate::installer::donate_answered(s, false),
                            _ => s.donate_never = true,
                        });
                    })
            };
            h_flex()
                .flex_shrink_0()
                .px_3()
                .py_1()
                .gap_2()
                .items_center()
                .border_t_1()
                .border_color(p.divider)
                .text_xs()
                .text_color(p.text_secondary)
                .child(Icon::new(L::Coffee).size(px(12.)).text_color(p.accent))
                .child(div().flex_1().min_w_0().child(message))
                .child(action("coffee-yes", t!("Kahve ısmarla"), 0))
                .child(action("coffee-later", t!("Sonra"), 1))
                .child(action("coffee-no", t!("Bir daha gösterme"), 2))
        });
        // Taskbar title follows the active folder.
        let title = format!("{} — rustyfiles", self.active_tab().read(cx).title(cx));
        if title != self.last_title {
            window.set_window_title(&title);
            self.last_title = title;
        }
        // Keep the saved session current (so it survives a crash or forced shutdown).
        let (session, panes) = self.session_now(cx);
        let key = session_key(&session, &panes);
        if key != self.last_session && self.picker.is_none() {
            self.last_session = key;
            let active = self.active;
            cx.defer(move |cx| {
                cx.update_global::<crate::settings::Settings, _>(|s, _| {
                    s.session_tabs = session;
                    s.session_panes = panes;
                    s.session_active = active;
                });
                let snapshot = cx.global::<crate::settings::Settings>().clone();
                std::thread::spawn(move || snapshot.save());
            });
        }
        let tab_state = self.tabs[self.active].clone();
        let content: AnyElement = match &tab_state.right {
            None => div().size_full().child(tab_state.left.clone()).into_any_element(),
            Some(right) => {
                let side_by_side = cx.global::<crate::settings::Settings>().dual_pane_vertical;
                let pane = |view: Entity<FolderView>, is_right: bool, active: bool, cx: &mut Context<Self>| {
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .when(side_by_side, |d| d.h_full())
                        .when(!side_by_side, |d| d.w_full())
                        .relative()
                        .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                            this.set_active_pane(is_right, cx)
                        }))
                        .child(view)
                        .when(active, |d| {
                            d.child(div().absolute().top_0().left_0().right_0().h(px(2.)).bg(p.accent))
                        })
                };
                let left = pane(tab_state.left.clone(), false, !tab_state.right_active, cx);
                let right = pane(right.clone(), true, tab_state.right_active, cx);
                if side_by_side {
                    h_flex().size_full().child(left).child(div().w(px(1.)).h_full().bg(p.divider)).child(right).into_any_element()
                } else {
                    v_flex().size_full().child(left).child(div().h(px(1.)).w_full().bg(p.divider)).child(right).into_any_element()
                }
            }
        };
        let shelf = self.shelf_open.then(|| {
            div()
                .w(px(260.))
                .flex_shrink_0()
                .h_full()
                .border_l_1()
                .border_color(p.divider)
                .child(self.shelf.clone())
        });
        let pane_shown = self.preview_open && self.compact.is_none() && room_for_preview && self.active_tab().read(cx).location().path().is_some();
        // the hosted preview handler is a separate window: hide it whenever something of ours
        // could be under it (pane closed, a dialog or the Status Center open, palette, bulk rename)
        let overlay_open = crate::prompt::current(window, cx).is_some() || crate::conflict_dialog::current(window, cx).is_some() || self.ops.read(cx).open || self.palette.is_some() || self.bulk.is_some() || crate::organizer::is_open(window, cx) || crate::prop_chooser::is_open(window, cx);
        let covered = !pane_shown || overlay_open || self.quick_look.is_some();
        self.preview.update(cx, |pv, _| pv.set_occluded(covered));
        // an open QuickLook preview follows the focused item too (`quicklook::selection_is` sends only what changed)
        if cx.global::<crate::settings::Settings>().space_quicklook
            && window.is_window_active()
            && let Some((path, _)) = self.active_tab().read(cx).preview_target()
        {
            crate::quicklook::selection_is(&path);
        }
        // Hızlı Bakış penceresi uygulamadan açıldıysa odaktaki öğeyi izler (oklar burada çalışır)
        let look_open = crate::folder_view::QUICK_LOOK_OPEN.load(std::sync::atomic::Ordering::Relaxed);
        if look_open && window.is_window_active() {
            if let Some((path, e)) = self.active_tab().read(cx).preview_target() {
                let mut last = LOOK_FOLLOWED.lock().unwrap();
                if last.as_ref() != Some(&path) {
                    *last = Some(path.clone());
                    crate::archive_view::look_send(path, e.size, true);
                }
            }
        }
        // Quick Look follows the focused item (arrow keys keep working underneath)
        let ql_target = self.quick_look.as_ref().and_then(|_| self.active_tab().read(cx).preview_target());
        if self.quick_look.is_some() && ql_target.is_none() {
            self.close_quick_look(cx);
        }
        let quick_look_layer = self.quick_look.clone().map(|ql| {
            ql.update(cx, |pv, cx| {
                pv.set_occluded(overlay_open);
                pv.set_target(ql_target, cx);
            });
            div()
                .id("quick-look")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0., 0., 0., 0.45))
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close_quick_look(cx)))
                .child(
                    div()
                        .id("quick-look-card")
                        .w(relative(0.8))
                        .h(relative(0.82))
                        .p_4()
                        .rounded(theme::RADIUS)
                        .bg(p.card_bg)
                        .border_1()
                        .border_color(p.border)
                        .shadow_lg()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(ql)
                        .with_animation("ql-in", Animation::new(std::time::Duration::from_millis(140)).with_easing(ease_out_quint()), |el, t| {
                            el.opacity(t)
                        }),
                )
        });
        let preview = if pane_shown && !preview_bottom {
            let target = self.active_tab().read(cx).preview_target();
            self.preview.update(cx, |pv, cx| pv.set_target(target, cx));
            Some(
                div()
                    .w(px(self.preview_w))
                    .flex_shrink_0()
                    .h_full()
                    .relative()
                    .border_l_1()
                    .border_color(p.divider)
                    .child(self.preview.clone())
                    .child(resize_handle("preview-resize", ResizePreview, true)),
            )
        } else {
            None
        };
        let bottom_pane = self.preview_bottom_pane(preview_dock::BottomFrame { bottom: preview_bottom, shown: pane_shown, stacked: preview_stacked, side_w: side_used, viewport_h }, &p, cx);
        // the file list's slot, and the row that holds it with the shelf and the preview pane (at the right, or under them)
        let list_slot = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .relative()
            // Drop a tab here: it moves into a new window.
            .drag_over::<TabDrag>(move |s, _, _, _| s.bg(p.selection))
            .on_drop(cx.listener(|this, d: &TabDrag, window, cx| this.detach_tab(d.ix, window, cx)))
            .child(content);
        let main_row: AnyElement = if preview_bottom {
            self.preview_bottom_row(list_slot, shelf, bottom_pane, cx)
        } else {
            h_flex().flex_1().min_h_0().child(list_slot).children(shelf).children(preview).into_any_element()
        };

        v_flex()
            .id("workspace")
            .size_full()
            .bg(p.window_bg)
            .text_color(p.text)
            // the pointer coming over the window gives its glass back the full rate (and thaws it): one event on entering, none while
            // it moves
            .on_hover(cx.listener(|this, over: &bool, _, _| {
                if *over && this.glass_hwnd != 0 {
                    rf_win::glass::pointer_over(this.glass_hwnd);
                }
            }))
            .on_action(cx.listener(|this, _: &GoBack, _, cx| this.with_tab(cx, |v, cx| v.back(cx))))
            // Windows' own dialogs (credentials, reconnect at sign-in); the drive list notices the new / removed drive by itself.
            // On a thread of their own, as every other Windows dialog here (`settings_view::pick_into`): run here, inside the
            // action, the dialog's modal loop handed our windows their messages while the app was borrowed, and the app closed
            // (issue 46: "Clicking on map network drive crashes and closes the explorer window").
            .on_action(cx.listener(|_, _: &RestartApp, _, cx| crate::restart(cx)))
            .on_action(cx.listener(|this, _: &SaveSession, window, cx| this.save_named_session(window, cx)))
            .on_action(cx.listener(|_, _: &MapNetworkDrive, window, _| {
                let owner = Self::hwnd(window).unwrap_or(0);
                std::thread::spawn(move || rf_win::network::map_drive_dialog(owner));
            }))
            .on_action(cx.listener(|_, _: &DisconnectNetworkDrive, window, _| {
                let owner = Self::hwnd(window).unwrap_or(0);
                std::thread::spawn(move || rf_win::network::disconnect_drive_dialog(owner));
            }))
            .on_action(cx.listener(|this, _: &GoForward, _, cx| this.with_tab(cx, |v, cx| v.forward(cx))))
            .on_action(cx.listener(|this, _: &GoUp, _, cx| this.with_tab(cx, |v, cx| v.up(cx))))
            .on_action(cx.listener(|this, _: &GoHome, window, cx| this.navigate(Location::Home, window, cx)))
            .on_action(cx.listener(|this, _: &Refresh, _, cx| this.with_tab(cx, |v, cx| v.refresh(cx))))
            .on_action(cx.listener(|this, _: &ToggleHidden, _, cx| this.with_tab(cx, |v, cx| v.toggle_hidden(cx))))
            .on_action(cx.listener(|this, _: &NewTab, window, cx| this.open_new_tab(window, cx)))
            .on_action(cx.listener(|this, _: &NewWindow, window, cx| {
                // New window at the current folder, offset from this one.
                let loc = this.active_tab().read(cx).location().clone();
                let mut b = window.bounds();
                b.origin.x += px(36.);
                b.origin.y += px(36.);
                cx.defer(move |cx| crate::open_main_window(Some(loc), None, Some(WindowBounds::Windowed(b)), cx));
            }))
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                let i = this.active;
                this.close_tab(i, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| {
                let n = this.tabs.len();
                this.activate((this.active + 1) % n, window, cx)
            }))
            .on_action(cx.listener(|this, _: &PrevTab, window, cx| {
                let n = this.tabs.len();
                this.activate((this.active + n - 1) % n, window, cx)
            }))
            .on_action(cx.listener(|this, _: &ReopenClosedTab, window, cx| {
                if let Some(loc) = this.closed.pop() {
                    this.open_tab(loc, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &FocusAddress, window, cx| this.begin_address_edit(window, cx)))
            .on_action(cx.listener(|this, _: &FocusNextPane, window, cx| this.cycle_focus(true, window, cx)))
            .on_action(cx.listener(|this, _: &FocusPrevPane, window, cx| this.cycle_focus(false, window, cx)))
            .on_action(cx.listener(|this, _: &TogglePreview, _, cx| {
                this.toggle_preview(cx);
            }))
            .on_action(cx.listener(|this, _: &DuplicateTab, window, cx| {
                let loc = this.active_tab().read(cx).location().clone();
                this.open_tab(loc, window, cx);
            }))
            .on_action(cx.listener(|this, _: &CloseOtherTabs, window, cx| this.close_other_tabs(window, cx)))
            .on_action(cx.listener(|this, _: &CloseAllTabs, window, cx| this.close_all_tabs(window, cx)))
            .on_action(cx.listener(|this, _: &SplitPaneSideBySide, window, cx| this.split_pane(true, window, cx)))
            .on_action(cx.listener(|this, _: &SplitPaneStacked, window, cx| this.split_pane(false, window, cx)))
            .on_action(cx.listener(|this, _: &CloseActivePane, window, cx| this.close_active_pane(window, cx)))
            .on_action(cx.listener(|this, _: &OpenTerminalAdmin, _, cx| {
                if let Some(dir) = this.active_tab().read(cx).location().path().map(Path::to_path_buf) {
                    std::thread::spawn(move || {
                        if let Err(e) = rf_win::shell::open_terminal_admin(&dir) {
                            log::warn!("terminal (admin): {e:#}");
                        }
                    });
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleTheme, window, cx| {
                use crate::settings::ThemePref;
                let dark = crate::theme::Palette::for_window(window).dark;
                crate::settings::update(cx, |s| s.theme = if dark { ThemePref::Light } else { ThemePref::Dark });
            }))
            .on_action(cx.listener(|this, _: &OrganizeFolder, window, cx| {
                match this.active_tab().read(cx).location().path().map(Path::to_path_buf) {
                    Some(dir) => crate::organizer::organize_folder(dir, window, cx),
                    None => crate::organizer::not_a_folder(window, cx),
                }
            }))
            .on_action(cx.listener(|this, _: &OpenInIde, _, cx| {
                if let Some(dir) = this.active_tab().read(cx).location().path().map(Path::to_path_buf) {
                    let ide = cx.global::<crate::settings::Settings>().ide_path.clone();
                    open_in_ide(dir, ide);
                }
            }))
            .on_action(cx.listener(|_, _: &ToggleToolbar, _, cx| {
                let v = !cx.global::<crate::settings::Settings>().show_toolbar;
                crate::settings::update(cx, |s| s.show_toolbar = v);
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                let v = !this.sidebar_open;
                crate::settings::update(cx, |s| s.sidebar_open = v);
            }))
            .on_action(cx.listener(|this, _: &ToggleDualPane, window, cx| this.toggle_dual_pane(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleShelf, _, cx| this.toggle_shelf(cx)))
            .on_action(cx.listener(|_, _: &ToggleFullscreen, window, _| window.toggle_fullscreen()))
            .on_action(cx.listener(|this, _: &ToggleAlwaysOnTop, window, _| this.toggle_always_on_top(window)))
            .on_action(cx.listener(|this, _: &ToggleCompactOverlay, window, cx| this.toggle_compact(window, cx)))
            .on_action(cx.listener(|this, _: &GoToFolderAlias, window, cx| {
                // Files: the address box opens with "@" and lists the aliases
                this.begin_address_edit(window, cx);
                this.address.update(cx, |s, cx| s.set_value("@", window, cx));
                this.update_suggestions("@".into(), cx);
            }))
            .on_action(cx.listener(|this, _: &ToggleCommandPalette, window, cx| this.toggle_command_palette(window, cx)))
            .on_action(cx.listener(|this, _: &SearchFolderHistory, window, cx| {
                this.toggle_palette(crate::command_palette::PaletteMode::Folders, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SearchTabs, window, cx| this.toggle_palette(crate::command_palette::PaletteMode::Tabs, window, cx)))
            .on_action(cx.listener(|this, _: &GoSettings, window, cx| this.navigate(Location::Settings, window, cx)))
            .on_action(cx.listener(|this, _: &GoRecycleBin, window, cx| this.navigate(Location::RecycleBin, window, cx)))
            .on_action(cx.listener(|this, _: &SwitchPane, window, cx| {
                let r = !this.tabs[this.active].right_active;
                this.set_active_pane(r, cx);
                this.focus_active(window, cx);
            }))
            .on_action(cx.listener(|this, _: &ViewDetails, _, cx| this.with_tab(cx, |v, cx| v.set_view_mode(ViewMode::Details, cx))))
            .on_action(cx.listener(|this, _: &ViewGrid, _, cx| this.with_tab(cx, |v, cx| v.set_view_mode(ViewMode::Grid, cx))))
            .on_action(cx.listener(|this, _: &ViewColumns, _, cx| this.with_tab(cx, |v, cx| v.set_view_mode(ViewMode::Columns, cx))))
            .on_action(cx.listener(|this, _: &ViewCards, _, cx| {
                this.with_tab(cx, |v, cx| v.set_layout(ViewMode::Grid, crate::folder_view::GridStyle::Cards, cx))
            }))
            .on_action(cx.listener(|this, _: &ViewList, _, cx| {
                this.with_tab(cx, |v, cx| v.set_layout(ViewMode::Grid, crate::folder_view::GridStyle::List, cx))
            }))
            .on_action(cx.listener(|this, _: &FocusSearch, window, cx| {
                this.enter_search(window, cx);
            }))
            .on_action(cx.listener(|this, _: &OpenTerminal, _, cx| {
                if let Some(dir) = this.active_tab().read(cx).location().path().map(Path::to_path_buf) {
                    std::thread::spawn(move || {
                        let _ = rf_win::shell::open_terminal(&dir);
                    });
                }
            }))
            .relative()
            .children(backdrop_layers)
            .child(titlebar)
            .child(toolbar)
            .children(top_commandbar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizePreview>, _, cx| {
                        let margin = if cx.global::<crate::settings::Settings>().windows_file_explorer_theme { 0. } else { 9. };
                        let w = f32::from(e.bounds.right() - e.event.position.x) - margin;
                        this.set_pane_width(false, w, cx);
                    }))
                    .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizeSidebar>, _, cx| {
                        let w = f32::from(e.event.position.x - e.bounds.left());
                        this.set_pane_width(true, w, cx);
                    }))
                    .children(sidebar.map(|s| {
                        div()
                            .id("sidebar-slot")
                            .h_full()
                            .relative()
                            .flex_shrink_0()
                            .when(windows_theme, |d| d.border_r_1().border_color(p.divider))
                            .child(s)
                            .child(resize_handle("sidebar-resize", ResizeSidebar, false))
                            // on the rail, hovering opens the full sidebar over the content
                            .on_hover(cx.listener(|this, h: &bool, window, cx| {
                                if this.sidebar_rail && this.rail_peek[0] != *h {
                                    if *h && !this.rail_peek[1] {
                                        // before the sidebar covers it: a column near the rail's
                                        // left edge, clear of the icons
                                        let s = window.scale_factor();
                                        let height = f32::from(window.viewport_size().height);
                                        let at = |v: f32| (v * s) as i32;
                                        // (over glass the screen shows the desktop behind the window, not the window's tint)
                                        this.rail_peek_bg = rf_win::window::screen_color(crate::look::window::hwnd_of(window), at(3.), at(110.), at(height - 60.))
                                            .filter(|_| this.glass_hwnd == 0)
                                            .map(|[r, g, b]| Hsla::from(gpui_kit::rgb(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))));
                                    }
                                    this.rail_peek[0] = *h;
                                    cx.notify();
                                }
                            }))
                            // drawn after the list and the preview, so it lies over them
                            .children(peek.map(|d| deferred(d).with_priority(1)))
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .bg(p.content_bg)
                            .overflow_hidden()
                            .flex()
                            .flex_col()
                            .when(!windows_theme, |d| d.mr_2().rounded(theme::RADIUS).border_1().border_color(p.border))
                            .children(content_commandbar)
                            .children(filter_strip)
                            .child(main_row)
                            // the floating card of a running operation (Settings → Aktarım ilerlemesi → Yüzen kart)
                            .children(cards)
                            .children(star_strip)
                            .children(donate_strip)
                            .when(show_statusbar, |d| d.child(status))
                            .children(picker_footer),
                    ),
            )
            .when(!windows_theme, |d| d.child(div().h(px(8.))))
            // Last children = painted on top.
            .children(crate::status_center::render_flyout(
                &self.ops,
                &p,
                theme::TITLEBAR_H + px(if windows_theme { 60. } else { 44. }),
                cx,
            ))
            .children(crate::status_center::update_toast(&p, self.picker.is_some()))
            .children(quick_look_layer)
            .children(self.palette.clone())
            .children(crate::fluent_menu::current(window, cx))
            .children(crate::prompt::current(window, cx))
            .children(crate::prop_chooser::overlay(window, cx))
            .children(crate::conflict_dialog::current(window, cx))
            .children(crate::organizer::overlay(window, cx))
            .children(self.bulk.clone().map(|br| {
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(hsla(0., 0., 0., 0.3))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(br)
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::{breadcrumbs, parse_location};
    use crate::workspace::tab_terminal_dir;
    use rf_core::Location;
    use std::path::{Path, PathBuf};

    #[test]
    fn crumbs() {
        // the names are read on a worker in the app (FolderView::warm_labels); here the same call, first
        rf_win::localized::warm(crate::nav_touch::label_paths(Path::new(r"C:\Users\demo")));
        let c = breadcrumbs(Path::new(r"C:\Users\demo"));
        let labels: Vec<_> = c.iter().map(|(l, _)| l.as_str()).collect();
        // C:\Users shows Explorer's localized name ("Kullanıcılar" on Turkish Windows)
        let users = rf_win::localized::label_if_known(Path::new(r"C:\Users")).map_or("Users".to_string(), |l| l.to_string());
        assert_eq!(labels, vec!["C:", users.as_str(), "demo"]);
        assert_ne!(users, "");
        assert_eq!(c[0].1, Location::Path(PathBuf::from(r"C:\")));
        assert_eq!(parse_location("d:"), Some(Location::Path(PathBuf::from(r"d:\"))));
    }

    #[test]
    fn the_terminal_button_is_only_for_real_folders() {
        assert!(tab_terminal_dir(&Location::Home).is_none());
        assert!(tab_terminal_dir(&Location::Settings).is_none());
        let dir = std::env::temp_dir();
        assert_eq!(tab_terminal_dir(&Location::Path(dir.clone())), Some(dir));
        assert!(tab_terminal_dir(&Location::Path(PathBuf::from(r"C:\Downloads\photos.zip\2024"))).is_none());
        assert!(tab_terminal_dir(&Location::Path(PathBuf::from("ftp://example.com/pub"))).is_none());
    }
}

#[cfg(test)]
mod launch_tests {
    use super::{SELECT_PREFIX, parse_launch};
    use rf_core::Location;
    use std::path::PathBuf;

    #[test]
    fn select_messages() {
        let (loc, sel) = parse_launch(&format!(r"{SELECT_PREFIX}C:\Klasör\rapor ü.txt"));
        assert_eq!(loc, Location::Path(PathBuf::from(r"C:\Klasör")));
        assert_eq!(sel.as_deref(), Some("rapor ü.txt"));
        assert_eq!(parse_launch(r"C:\Windows"), (Location::Path(PathBuf::from(r"C:\Windows")), None));
        assert_eq!(parse_launch("  "), (Location::Home, None));
    }

    #[test]
    fn empty_launch_restores_session_only_when_no_browser_window_is_open() {
        use super::launch_window_start;
        assert_eq!(launch_window_start("", false, Location::Home), None);
        assert_eq!(launch_window_start("  ", false, Location::Home), None);
        assert_eq!(launch_window_start("", true, Location::Home), Some(Location::Home));
        let docs = Location::Path(PathBuf::from(r"C:\Docs"));
        assert_eq!(launch_window_start(r"C:\Docs", false, docs.clone()), Some(docs.clone()));
        assert_eq!(launch_window_start(r"C:\Docs", true, docs.clone()), Some(docs));
    }

    /// Issue 20: a window split in two panes at different folders came back with one pane. The second pane of a tab is kept with its
    /// own place and with whether it had the focus, and comes back for the tab it belonged to.
    #[test]
    fn the_second_pane_of_a_split_tab_comes_back_at_its_own_folder() {
        use super::{panes_to_restore, session_key, session_record};
        use crate::settings::SessionPane;
        let p = |s: &str| Location::Path(PathBuf::from(s));
        let tabs = vec![(p(r"C:\Work"), Some((p(r"D:\Backup"), true))), (p(r"C:\Users"), None), (Location::Home, Some((Location::RecycleBin, false)))];
        let (left, panes) = session_record(&tabs);
        assert_eq!(left, [r"C:\Work", r"C:\Users", ""]);
        assert_eq!(
            panes,
            [SessionPane { tab: 0, location: r"D:\Backup".into(), active: true }, SessionPane { tab: 2, location: "shell:RecycleBinFolder".into(), active: false }]
        );
        assert_eq!(panes_to_restore(&panes, 3), [(0, p(r"D:\Backup"), true), (2, Location::RecycleBin, false)]);
        // fewer tabs came back than the record knows (a tab's folder is gone, the list was edited): no pane for a tab that is not there
        assert_eq!(panes_to_restore(&panes, 2), [(0, p(r"D:\Backup"), true)]);
        // two entries for one tab: the first counts
        let twice = [SessionPane { tab: 1, location: r"C:\a".into(), active: false }, SessionPane { tab: 1, location: r"C:\b".into(), active: true }];
        assert_eq!(panes_to_restore(&twice, 2), [(1, p(r"C:\a"), false)]);
        assert!(panes_to_restore(&[], 4).is_empty(), "a session from before this, or one with no split tab");
        // the record is written again when a pane goes somewhere else or is opened or closed, not when only the focus moves
        let moved = session_record(&[(p(r"C:\Work"), Some((p(r"E:\"), true)))]);
        let focus = session_record(&[(p(r"C:\Work"), Some((p(r"D:\Backup"), false)))]);
        let one = session_record(&[(p(r"C:\Work"), Some((p(r"D:\Backup"), true)))]);
        let closed = session_record(&[(p(r"C:\Work"), None)]);
        assert_ne!(session_key(&one.0, &one.1), session_key(&moved.0, &moved.1));
        assert_ne!(session_key(&one.0, &one.1), session_key(&closed.0, &closed.1));
        assert_eq!(session_key(&one.0, &one.1), session_key(&focus.0, &focus.1));
    }

    #[test]
    fn session_tabs_come_back_as_they_were() {
        let phone = Location::Shell {
            parsing: r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\\?\usb#vid_04e8&pid_6860#r58n#{6ac27878-a6fa-4155-ba85-f98f491d4f33}".into(),
            name: "Galaxy S24".into(),
        };
        for loc in [Location::Path(PathBuf::from(r"C:\Kullanıcılar\Özer")), Location::RecycleBin, Location::Downloads, phone] {
            assert_eq!(parse_launch(&super::session_text(&loc)).0, loc);
        }
        // places a session can't keep come back as Home
        assert_eq!(super::session_text(&Location::Settings), "");
    }
}

#[cfg(test)]
mod rail_peek_tests {
    use super::{RAIL_W, rail_peek_width};

    /// The sidebar that comes out of the rail: from the rail's own width to the full one, a little past it while the spring swings,
    /// and never narrower than the rail.
    #[test]
    fn the_sidebar_comes_out_of_the_rail_and_is_never_narrower_than_it() {
        assert_eq!(rail_peek_width(240.0, 0.0), RAIL_W);
        assert_eq!(rail_peek_width(240.0, 1.0), 240.0);
        assert!(rail_peek_width(240.0, 1.05) > 240.0, "the swing past its place");
        assert_eq!(rail_peek_width(240.0, -0.2), RAIL_W);
        assert!(rail_peek_width(240.0, 0.5) > RAIL_W && rail_peek_width(240.0, 0.5) < 240.0);
    }
}

/// The stops of Tab in a window, in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Address,
    Tree,
    List,
}

/// The pane after `here` (`forward`) or before it, round; the tree is skipped when it is not on screen.
fn next_pane(here: Pane, forward: bool, tree: bool) -> Pane {
    let order: Vec<Pane> = [Pane::Address, Pane::Tree, Pane::List].into_iter().filter(|p| *p != Pane::Tree || tree).collect();
    let i = order.iter().position(|p| *p == here).unwrap_or(order.len() - 1);
    let n = order.len();
    order[if forward { (i + 1) % n } else { (i + n - 1) % n }]
}

#[cfg(test)]
mod pane_tests {
    use super::{Pane, next_pane};

    #[test]
    fn tab_goes_round_the_address_bar_the_tree_and_the_list() {
        assert_eq!(next_pane(Pane::List, true, true), Pane::Address);
        assert_eq!(next_pane(Pane::Address, true, true), Pane::Tree);
        assert_eq!(next_pane(Pane::Tree, true, true), Pane::List);
        assert_eq!(next_pane(Pane::List, false, true), Pane::Tree);
        assert_eq!(next_pane(Pane::Address, false, true), Pane::List);
        assert_eq!(next_pane(Pane::Address, true, false), Pane::List, "no tree on screen: it is skipped");
        assert_eq!(next_pane(Pane::List, false, false), Pane::Address);
    }
}

/// A drive's used and total space as the sidebar says it under the drive's bar: "238 GB / 500 GB".
fn drive_space_text(total: u64, free: u64) -> String {
    format!("{} / {}", rf_core::format::size(total.saturating_sub(free)), rf_core::format::size(total))
}

/// The cloud and WSL places that are not hidden one by one (`Settings::hidden_places`, paths compared without regard to case).
pub(crate) fn visible_places(places: Vec<crate::places::Pinned>, hidden: &[String]) -> Vec<crate::places::Pinned> {
    places.into_iter().filter(|c| !hidden.iter().any(|h| c.path.as_os_str().eq_ignore_ascii_case(std::ffi::OsStr::new(h)))).collect()
}

#[cfg(test)]
mod named_session_tests {
    use super::remember_named;
    use crate::settings::NamedSession;

    #[test]
    fn a_session_saved_again_under_its_name_replaces_the_old_one_in_place() {
        let s = |name: &str, tab: &str| NamedSession { name: name.into(), tabs: vec![tab.into()], ..Default::default() };
        let mut list = Vec::new();
        remember_named(&mut list, s("Work", "C:\\A"));
        remember_named(&mut list, s("Photos", "D:\\P"));
        remember_named(&mut list, s("work", "C:\\B"));
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].name.as_str(), list[0].tabs[0].as_str()), ("work", "C:\\B"), "same place, the new tabs");
        assert_eq!(list[1].name, "Photos");
    }
}

#[cfg(test)]
mod hidden_places_tests {
    use super::visible_places;
    use crate::places::Pinned;
    use gpui_kit::assets::IconName as L;

    #[test]
    fn a_place_hidden_on_its_own_leaves_the_others() {
        let place = |p: &str| Pinned { title: p.into(), path: p.into(), icon: L::Cloud };
        let all = vec![place(r"C:\Users\u\OneDrive"), place(r"C:\Users\u\Proton Drive"), place(r"\wsl.localhost\Ubuntu")];
        let shown = visible_places(all.clone(), &[r"c:\users\u\onedrive".into()]);
        let paths: Vec<_> = shown.iter().map(|p| p.path.to_string_lossy().into_owned()).collect();
        assert_eq!(paths, [r"C:\Users\u\Proton Drive", r"\wsl.localhost\Ubuntu"], "the case of a path does not matter");
        assert_eq!(visible_places(all, &[]).len(), 3);
    }
}
