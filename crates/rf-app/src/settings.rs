//! Persistent user settings (%APPDATA%\rustyfiles\settings.toml) as a GPUI global.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};

use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::i18n::Lang;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThemePref {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DefaultView {
    Details,
    Grid,
    Columns,
    /// Picks per folder, like Explorer's folder types: mostly pictures/videos → grid, Pictures
    /// and Videos → grid, Downloads → newest first in date groups, otherwise details.
    #[default]
    Adaptive,
}

/// Checking copies after they are written (our engine).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CopyVerify {
    #[default]
    Off,
    /// Size and modification time.
    Quick,
    /// Every byte, read back from the disk.
    Full,
}

impl CopyVerify {
    pub fn engine(self) -> rf_win::fastcopy::Verify {
        match self {
            CopyVerify::Off => rf_win::fastcopy::Verify::Off,
            CopyVerify::Quick => rf_win::fastcopy::Verify::Quick,
            CopyVerify::Full => rf_win::fastcopy::Verify::Full,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DatePref {
    /// "5 dakika önce" for recent items.
    #[default]
    Application,
    System,
    Universal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum StartupPref {
    /// Reopen last session's tabs.
    #[default]
    Continue,
    /// One Home tab.
    NewTab,
    /// The folders in `startup_pages`.
    Pages,
}

/// Settings → Appearance → Animations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AnimPref {
    /// Follow Windows (Settings → Accessibility → Visual effects → Animation effects).
    #[default]
    System,
    On,
    Off,
}

/// Settings -> Dock: how much the dock's icons grow under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DockMagnify {
    /// They stay as they are.
    Off,
    /// The icon under the pointer grows to 1.25 times.
    Subtle,
    /// To 1.55 times, like the Mac's dock.
    #[default]
    Full,
}

impl DockMagnify {
    /// What the dock's magnification takes.
    pub fn strength(self) -> rf_win::dock::magnify::Strength {
        use rf_win::dock::magnify::Strength;
        match self {
            DockMagnify::Off => Strength::Off,
            DockMagnify::Subtle => Strength::Subtle,
            DockMagnify::Full => Strength::Full,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum BackdropPref {
    /// Default, like Explorer: a quiet desktop tint.
    #[default]
    Mica,
    /// Files' default: a more colorful variant of Mica.
    MicaAlt,
    Acrylic,
    Solid,
    /// Experimental: the screen behind the window, bent and frosted like glass (rf_win::glass).
    /// An older version does not know this value and reads the field as the default (Mica);
    /// the other settings are kept (see `parse_lenient`).
    Glass,
}

/// Hızlı Bakış penceresinin arka planı.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LookBackdrop {
    #[default]
    Mica,
    Acrylic,
    Tabbed,
    /// Düz renk (tema rengi ya da `look_backdrop_color`).
    None,
}

/// Hızlı Bakış resim görüntüleyicisinin zemini.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum LookImageTheme {
    #[default]
    Dark,
    Light,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeletePref {
    Always,
    /// Only for permanent deletes (the Recycle Bin can undo the rest) — Explorer's behaviour.
    #[default]
    PermanentOnly,
    Never,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ClickPref {
    #[default]
    Never,
    Folders,
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum IdePref {
    #[default]
    GitRepos,
    Always,
    Never,
}

/// Which of our own entries the context menu shows (Settings → General → Context menu).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MenuPrefs {
    pub open_in_new_tab: bool,
    pub open_in_new_window: bool,
    pub copy_path: bool,
    pub create_shortcut: bool,
    pub pin: bool,
    pub compress: bool,
    pub tags: bool,
    pub shelf: bool,
    pub terminal: bool,
    /// Shell extensions under "Show more options" (off = listed at the bottom of the menu).
    pub shell_submenu: bool,
}

impl Default for MenuPrefs {
    fn default() -> Self {
        Self {
            open_in_new_tab: true,
            open_in_new_window: true,
            copy_path: true,
            create_shortcut: true,
            pin: true,
            compress: true,
            tags: true,
            shelf: true,
            terminal: true,
            shell_submenu: true,
        }
    }
}

/// Details-view columns that can be hidden (name is always shown).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColumnPrefs {
    pub modified: bool,
    pub created: bool,
    pub kind: bool,
    pub size: bool,
    /// Widths (px) of modified, created, type, size; dragged in the header.
    pub widths: [u16; 4],
    /// Files' "File tag", "Git status" and "Sync status" columns. Git shows only inside a
    /// repository, sync only in folders with cloud files.
    pub tags: bool,
    pub git: bool,
    pub sync: bool,
    /// Widths of tags, git, sync.
    pub extra_widths: [u16; 3],
    /// Files' commit columns (inside a repository): last commit message, author, date, hash.
    pub git_message: bool,
    pub git_author: bool,
    pub git_date: bool,
    pub git_hash: bool,
    pub git_widths: [u16; 4],
    /// Columns for Windows properties the user added ("Diğer sütunlar…"): `System.Media.Duration:140`
    /// (canonical name, width), in the order they show after the fixed columns. Empty = none, and
    /// then nothing of the property columns runs and the file has no such key.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub properties: Vec<String>,
}

impl Default for ColumnPrefs {
    fn default() -> Self {
        Self {
            modified: true,
            created: false,
            kind: true,
            size: true,
            widths: [150, 150, 150, 100],
            tags: true,
            git: true,
            sync: true,
            extra_widths: [130, 120, 150],
            git_message: false,
            git_author: false,
            git_date: false,
            git_hash: false,
            git_widths: [240, 130, 140, 90],
            properties: Vec::new(),
        }
    }
}

/// Where the quick search's "search the web" row sends the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebSearch {
    #[default]
    Google,
    Bing,
    DuckDuckGo,
    Yandex,
}

/// The renderer mode a process runs in. The dock (a process of its own, three small windows) is kept on the graphics card when the
/// setting is Auto: under Auto a card that another program keeps busy (a video render) makes the renderer build a software device and
/// move the islands to it, which costs seconds of CPU for windows that are a few hundred pixels wide. A user who chose the software
/// renderer on purpose keeps it.
pub(crate) fn renderer_mode(pref: RendererPref, dock: bool) -> gpui_windows::hybrid::RendererMode {
    use gpui_windows::hybrid::RendererMode;
    match pref {
        RendererPref::Auto if dock => RendererMode::Gpu,
        RendererPref::Auto => RendererMode::Auto,
        RendererPref::Gpu => RendererMode::Gpu,
        RendererPref::Cpu => RendererMode::Cpu,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RendererPref {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemePref,
    /// UI language (`system` follows the Windows UI language).
    pub language: Lang,
    pub default_view: DefaultView,
    pub show_hidden: bool,
    pub preview_open: bool,
    pub sidebar_open: bool,
    /// The sidebar as a narrow strip of icons (drag it narrow to get there).
    pub sidebar_rail: bool,
    /// Pane widths (drag the pane edge to resize).
    pub sidebar_width: f32,
    /// Sidebar sections the user turned off ("drives", "cloud", "libraries", "network", "tags").
    pub sidebar_hidden: Vec<String>,
    /// Sections of the Home page that are hidden: "quick", "drives", "devices", "tags",
    /// "network", "recent".
    pub home_hidden: Vec<String>,
    /// "Clear" on Home's recent items: only items used after this moment are listed (Windows'
    /// own list of recent items is left alone).
    pub recent_cleared_ms: i64,
    /// The last searches run with Enter (newest first, ten at most): offered under the empty
    /// search box.
    pub search_history: Vec<String>,
    /// Buttons of the command bar that are hidden ("new", "cut", "copy", "paste", "rename",
    /// "share", "delete", "props", "filter", "sort", "view", "preview").
    pub toolbar_hidden: Vec<String>,
    /// Buttons of the command bar that are off unless asked for ("newfolder", "terminal",
    /// "hidden", "refresh").
    pub toolbar_extra: Vec<String>,
    pub preview_width: f32,
    pub renderer: RendererPref,
    pub restore_session: bool,
    /// Last session's tab locations ("" = Home).
    pub session_tabs: Vec<String>,
    pub session_active: usize,
    /// User pins. `None` = the default known folders.
    pub pinned: Option<Vec<PathBuf>>,
    /// Folder aliases: `@name` in the address box (Files' folder aliases).
    pub folder_aliases: std::collections::BTreeMap<String, PathBuf>,
    /// Closing the last window keeps rustyfiles in the notification area (Files' "leave app
    /// running in the background"): the next window opens instantly.
    pub run_in_background: bool,
    /// Status bar: listing time and the GPU/CPU renderer badge (developer info).
    pub show_perf_info: bool,
    /// A notification when a long operation finishes while rustyfiles is not in front.
    pub notify_long_ops: bool,
    /// Updates (GitHub Releases) are looked for and downloaded in the background.
    pub auto_update: bool,
    /// The one-time "star it on GitHub" strip: first start (Unix ms), starts so far, and
    /// whether it was answered (starred or dismissed).
    pub first_start_ms: i64,
    pub starts: u32,
    pub star_asked: bool,
    /// The "buy me a coffee" strip, now and then: not before this time (Unix ms; 0 = by the
    /// first-start rule), never again once turned off, and how many times it was answered (its
    /// message changes each time).
    pub donate_next_ms: i64,
    pub donate_never: bool,
    pub donate_seen: u32,
    /// The one-time move from "details" to "adaptive" (0.3.3) has been made.
    pub adaptive_offered: bool,
    pub show_extensions: bool,
    /// Custom shortcuts: keystroke → action, e.g. `"ctrl-k" = "NewFolder"` (names as in the
    /// command palette's actions; `rustyfiles::` prefix optional). Applied at startup.
    pub keybindings: std::collections::BTreeMap<String, String>,
    /// Last window placement: [x, y, width, height, maximized (0/1)].
    pub window: Option<[f32; 5]>,

    // ---- General
    pub date_format: DatePref,
    pub startup: StartupPref,
    pub startup_pages: Vec<String>,
    /// Switch to tabs opened with Ctrl+click / middle click / "Open in new tab".
    pub focus_new_tab: bool,
    /// New tabs start split in two panes.
    pub dual_pane_new_tabs: bool,
    /// Dual pane side by side (true) or stacked (false).
    pub dual_pane_vertical: bool,
    pub menu: MenuPrefs,
    pub smooth_scroll: bool,

    // ---- Appearance
    /// A scrollbar over the Details and Grid lists.
    pub show_scrollbar: bool,
    pub animations: AnimPref,
    pub backdrop: BackdropPref,
    /// Background tint color (0xRRGGBB), `None` = the backdrop as is.
    pub tint: Option<u32>,
    pub background_image: Option<PathBuf>,
    /// 0.05 … 0.6
    pub background_opacity: f32,
    /// HyperGlass look (Settings > Appearance, shown while the backdrop is Glass), each 0..=100; a value outside is cut to that when it is
    /// used. See `rf_win::glass::LookParams` for what the first four do; the surface value is the app's own two washes over the glass.
    pub hg_frost: u8,
    pub hg_tint: u8,
    pub hg_refraction: u8,
    pub hg_saturation: u8,
    pub hg_surface: u8,
    /// UI font family; `None` = Segoe UI Variable / system.
    pub font: Option<String>,
    pub show_toolbar: bool,
    pub show_statusbar: bool,

    // ---- Layout
    /// One layout/sort for every folder (off = each folder remembers its own).
    pub sync_layout: bool,
    pub default_sort: rf_core::SortKey,
    pub default_sort_desc: bool,
    pub default_group: String,
    pub folders_first: bool,
    /// Files before folders (ignored while `folders_first` is on).
    pub files_first: bool,
    pub columns: ColumnPrefs,

    // ---- Files & folders
    pub show_system_files: bool,
    /// Names starting with "." count as hidden when off.
    pub show_dot_files: bool,
    pub show_thumbnails: bool,
    pub show_checkboxes: bool,
    pub single_click: ClickPref,
    pub open_folders_in_new_tab: bool,
    pub confirm_delete: DeletePref,
    pub warn_extension_change: bool,
    pub select_on_hover: bool,
    pub double_click_up: bool,
    pub select_previous_on_up: bool,
    pub decimal_sizes: bool,
    pub calculate_folder_sizes: bool,
    /// Folders show their size on disk (files rounded up to whole clusters) instead of the sum
    /// of their file sizes.
    pub folder_size_on_disk: bool,
    /// Our own parallel engine for copies, moves to other drives and permanent deletes
    /// (off: Windows' engine for everything, like Explorer).
    pub fast_file_ops: bool,
    /// Check copies after writing them.
    pub copy_verify: CopyVerify,
    /// Copy speed limit in MB/s (0 = none).
    pub copy_speed_limit: u32,
    pub show_libraries: bool,
    /// A "Klasörler" section in the sidebar: the folder tree (drives and the profile folder as roots). Off by default.
    pub sidebar_tree: bool,
    /// The tree opens the branch to the open folder, marks it and scrolls it into view after every navigation.
    pub sidebar_tree_follow: bool,
    /// Double-clicking a ZIP/7z/tar opens it like a folder (read-only).
    pub browse_archives: bool,
    /// After extracting an archive into a new folder, that folder opens in a tab.
    pub open_after_extract: bool,
    /// Search through the Windows Search index (contents, AQS) where the folder is indexed.
    pub search_index: bool,
    /// Our own name index of the local drives: searching in subfolders takes milliseconds.
    pub name_index: bool,
    /// Folders left out of the name index (searching inside them walks the disk).
    pub index_excluded: Vec<String>,

    // ---- Developer tools
    pub ide_button: IdePref,
    /// Editor to open folders with ("code" when empty and VS Code is installed).
    pub ide_path: String,

    // ---- Advanced
    pub show_flatten: bool,

    // ---- DirectLook (Hızlı Bakış, ayrı program): ayarları kendi dosyasında (`look::config`)
    /// DirectLook kullanılsın mı (kapalıyken rustyfiles onu başlatmaz, Boşluk bir şey yapmaz).
    pub directlook_enabled: bool,

    // ---- Quick search (quick_search.rs)
    /// A global shortcut opens the quick search: apps, files, bookmarks, the web.
    pub quick_search: bool,
    /// Its shortcut, in GPUI's syntax ("alt-space").
    pub quick_search_keys: String,
    pub web_search: WebSearch,
    /// Chrome's and Edge's bookmarks are searched too (read on this PC only).
    pub quick_search_bookmarks: bool,

    // ---- Dock (dock/: `rustyfiles --dock`, a separate process; experimental)
    /// The dock runs: started with rustyfiles and when this is switched on, stopped when it is switched off.
    pub dock_enabled: bool,
    /// The dock keeps other windows from going under it (it registers as an application desktop toolbar).
    pub dock_reserve: bool,
    /// The city the dock's weather is for. Empty: no weather and no network request.
    pub dock_weather_city: String,
    /// The dock's logo opens our own Start panel and its search field our own search (on). Off: the old behaviour, the logo sends the
    /// Windows key and the field opens the quick search window.
    pub dock_own_start: bool,
    /// The dock's network and volume symbols open our own control centre (on). Off: the old behaviour, they send Win+A (Windows' quick
    /// settings).
    pub dock_own_control_centre: bool,
    /// The dock's clock opens our own calendar panel (on). Off: the old behaviour, it sends Win+N (Windows' notification centre and calendar).
    pub dock_own_calendar: bool,
    /// The dock's bell opens our own notification panel and wears a dot while there are notifications (on). Off: the old behaviour,
    /// it sends Win+N.
    pub dock_own_notifications: bool,
    /// How much the dock's icons grow under the pointer (off / subtle / full).
    pub dock_magnify: DockMagnify,
    /// Resting the pointer on a running app's icon in the dock shows a preview of its windows (on; the dock itself is opt-in). Off: nothing.
    pub dock_hover_previews: bool,
    /// A right click on an app's icon in the dock opens the dock's own app menu (on). Off: it does nothing, as before.
    pub dock_app_menu: bool,

    // ---- Shell host (shell/: `rustyfiles --shell`, the dock's process); experimental, every switch off by default
    /// Our desktop (the wallpaper, the icons) is drawn over Explorer's, one window per monitor, while the shell host runs. Explorer's
    /// desktop stays underneath and is visible again when the host stops.
    pub shell_desktop_cover: bool,
    /// A tap of the Windows key alone opens rustyfiles' Start panel instead of Windows' (the shell host listens to the keyboard with a
    /// system-wide hook while this or `shell_own_switcher` is on; see `shell/keylayer.rs`).
    pub shell_win_key: bool,
    /// Alt+Tab opens rustyfiles' window switcher instead of Windows' (same hook).
    pub shell_own_switcher: bool,
    /// The shell host owns a window of class `Shell_TrayWnd` in front of Explorer's, takes the icons programs send to the notification
    /// area, and passes every message on to Explorer's own window unchanged (see `shell/trayhost.rs`, `docs/design/tray-host.md`).
    pub shell_tray_host: bool,
    /// Explorer's taskbar windows (and with them its notification icons) are hidden while the dock runs and shown again when it stops;
    /// only with rustyfiles-shellguard.exe running as the safety net (see `shell/taskbar_hide.rs`).
    pub shell_hide_taskbar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemePref::System,
            language: Lang::System,
            default_view: DefaultView::Adaptive,
            show_hidden: false,
            preview_open: true,
            sidebar_open: true,
            sidebar_rail: false,
            sidebar_width: 232.,
            sidebar_hidden: Vec::new(),
            home_hidden: Vec::new(),
            recent_cleared_ms: 0,
            search_history: Vec::new(),
            toolbar_hidden: Vec::new(),
            toolbar_extra: Vec::new(),
            preview_width: 300.,
            renderer: RendererPref::Auto,
            restore_session: true,
            session_tabs: Vec::new(),
            session_active: 0,
            pinned: None,
            folder_aliases: Default::default(),
            run_in_background: false,
            show_perf_info: false,
            notify_long_ops: true,
            auto_update: true,
            first_start_ms: 0,
            starts: 0,
            star_asked: false,
            donate_next_ms: 0,
            donate_never: false,
            donate_seen: 0,
            adaptive_offered: false,
            show_extensions: true,
            keybindings: Default::default(),
            window: None,
            date_format: DatePref::Application,
            startup: StartupPref::Continue,
            startup_pages: Vec::new(),
            focus_new_tab: false,
            dual_pane_new_tabs: false,
            dual_pane_vertical: true,
            menu: MenuPrefs::default(),
            smooth_scroll: true,
            show_scrollbar: true,
            animations: AnimPref::System,
            backdrop: BackdropPref::Mica,
            tint: None,
            background_image: None,
            background_opacity: 0.2,
            hg_frost: HG_DEFAULT[0],
            hg_tint: HG_DEFAULT[1],
            hg_refraction: HG_DEFAULT[2],
            hg_saturation: HG_DEFAULT[3],
            hg_surface: HG_DEFAULT[4],
            font: None,
            show_toolbar: true,
            show_statusbar: true,
            sync_layout: false,
            default_sort: rf_core::SortKey::Name,
            default_sort_desc: false,
            default_group: "none".into(),
            folders_first: true,
            files_first: false,
            columns: ColumnPrefs::default(),
            show_system_files: false,
            show_dot_files: true,
            show_thumbnails: true,
            show_checkboxes: false,
            single_click: ClickPref::Never,
            open_folders_in_new_tab: false,
            confirm_delete: DeletePref::PermanentOnly,
            warn_extension_change: true,
            select_on_hover: false,
            double_click_up: true,
            select_previous_on_up: true,
            decimal_sizes: false,
            calculate_folder_sizes: false,
            folder_size_on_disk: false,
            fast_file_ops: true,
            copy_verify: CopyVerify::Off,
            copy_speed_limit: 0,
            show_libraries: false,
            sidebar_tree: false,
            sidebar_tree_follow: true,
            browse_archives: true,
            open_after_extract: false,
            search_index: true,
            name_index: true,
            index_excluded: Vec::new(),
            ide_button: IdePref::GitRepos,
            ide_path: String::new(),
            show_flatten: false,
            directlook_enabled: true,
            quick_search: true,
            quick_search_keys: "alt-space".into(),
            web_search: WebSearch::Google,
            quick_search_bookmarks: true,
            dock_enabled: false,
            dock_reserve: true,
            dock_weather_city: String::new(),
            dock_own_start: true,
            dock_own_control_centre: true,
            dock_own_calendar: true,
            dock_own_notifications: true,
            dock_magnify: DockMagnify::Full,
            dock_hover_previews: true,
            dock_app_menu: true,
            shell_desktop_cover: false,
            shell_win_key: false,
            shell_own_switcher: false,
            shell_tray_host: false,
            shell_hide_taskbar: false,
        }
    }
}

impl Global for Settings {}

/// The HyperGlass defaults in the order frost, tint, refraction, saturation, surface. The first four are the engine's own defaults
/// (`LookParams::DEFAULT`, a test keeps them equal); the surface default is the wash the app shipped with over glass.
pub const HG_DEFAULT: [u8; 5] = [52, 50, 67, 62, 50];

static THEME: AtomicU8 = AtomicU8::new(0);
/// The surface value (0..=100) the palette reads on every call, cheaper than the settings struct.
static HG_SURFACE: AtomicU8 = AtomicU8::new(HG_DEFAULT[4]);
static SOLID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

static ANIM: AtomicU8 = AtomicU8::new(0);

/// Whether UI animations play (Settings → Appearance → Animations; "System" follows Windows'
/// Animation effects, read each time: it's one cheap call and may change while we run).
pub fn animations() -> bool {
    match ANIM.load(Ordering::Relaxed) {
        x if x == AnimPref::On as u8 => true,
        x if x == AnimPref::Off as u8 => false,
        _ => rf_win::sysparams::client_area_animation(),
    }
}

/// "Solid" background material: the palette paints an opaque window background.
pub fn solid_backdrop() -> bool {
    SOLID.load(Ordering::Relaxed)
}

/// How strongly the app's own washes lie over the glass, 0..=100 (Settings > HyperGlass > Surface wash).
pub fn hg_surface() -> u8 {
    HG_SURFACE.load(Ordering::Relaxed)
}

/// Read by `Palette::for_window` without needing an `App`.
pub fn theme_override() -> ThemePref {
    match THEME.load(Ordering::Relaxed) {
        1 => ThemePref::Light,
        2 => ThemePref::Dark,
        _ => ThemePref::System,
    }
}

pub fn path() -> Option<PathBuf> {
    app_dir().map(|d| d.join("settings.toml"))
}

/// What reading the settings file gave.
enum FileRead {
    /// There is no file (or no folder to keep one in): the defaults apply.
    Missing,
    /// The file could not be read or understood.
    Bad,
    Read(Box<Settings>),
}

impl Settings {
    /// Reads the file. With `set_aside_bad` an unreadable or broken file is moved aside (so the next save does not overwrite what
    /// the user may want to repair); a reader that is not the owner of the file leaves it where it is.
    fn read_file(set_aside_bad: bool) -> FileRead {
        let Some(p) = path() else { return FileRead::Missing };
        let text = match rf_core::read_config(&p) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return FileRead::Missing,
            Err(e) => {
                // unreadable (not UTF-8, locked): keep it aside rather than overwrite it
                log::warn!("settings: {e}");
                if set_aside_bad {
                    rf_core::backup_corrupt(&p);
                }
                return FileRead::Bad;
            }
        };
        match Self::parse_lenient(text.trim_start_matches('\u{feff}')) {
            Ok(s) => FileRead::Read(Box::new(s)),
            Err(e) => {
                // a typo while hand-editing must not cost the user every setting:
                // keep the file aside, start with defaults
                log::warn!("settings: {e}");
                if set_aside_bad {
                    rf_core::backup_corrupt(&p);
                }
                FileRead::Bad
            }
        }
    }

    pub fn load() -> Self {
        let mut s = match Self::read_file(true) {
            FileRead::Read(s) => *s,
            FileRead::Missing | FileRead::Bad => Settings::default(),
        };
        s.migrate();
        s.apply_side_effects();
        s
    }

    /// For a process that only follows the file (the dock): the settings as the file has them now, brought up to date but with no
    /// side effects yet (call [`Settings::apply_side_effects`] on the thread that wants them) and without touching the file. `None`
    /// when the file is there but cannot be read or understood right now (locked, mid-edit): the caller keeps what it has.
    /// Blocks briefly (the read retries a locked file): not for the UI thread.
    pub fn load_readonly() -> Option<Self> {
        let mut s = match Self::read_file(false) {
            FileRead::Read(s) => *s,
            FileRead::Missing => Settings::default(),
            FileRead::Bad => return None,
        };
        s.migrate();
        Some(s)
    }

    /// Brings settings written by older versions up to date.
    fn migrate(&mut self) {
        // older files: "restore_session = false" means "start with a new tab"
        if !self.restore_session && self.startup == StartupPref::Continue {
            self.startup = StartupPref::NewTab;
            self.restore_session = true;
        }
        // "details" was the default before 0.3.3, so it says nothing about a choice: move it
        // once to "adaptive" (plain folders still open as details). Grid and columns stay.
        if !self.adaptive_offered {
            if self.default_view == DefaultView::Details {
                self.default_view = DefaultView::Adaptive;
            }
            self.adaptive_offered = true;
        }
    }

    /// Field by field: a value this version doesn't understand (a newer build's theme, a
    /// mistyped enum) falls back to its default instead of resetting every setting. Only a
    /// TOML syntax error fails.
    fn parse_lenient(text: &str) -> Result<Settings, toml::de::Error> {
        let table: toml::Table = toml::from_str(text)?;
        let mut base = toml::Table::try_from(Settings::default()).unwrap_or_default();
        for (k, v) in table {
            let mut trial = base.clone();
            trial.insert(k.clone(), v);
            if Settings::deserialize(toml::Value::Table(trial.clone())).is_ok() {
                base = trial;
            } else {
                log::warn!("settings: ignoring invalid value for '{k}'");
            }
        }
        Ok(Settings::deserialize(toml::Value::Table(base)).unwrap_or_default())
    }

    /// The four values the engine takes, cut to 0..=100.
    pub fn hyperglass_look(&self) -> rf_win::glass::LookParams {
        rf_win::glass::LookParams {
            frost: f32::from(self.hg_frost.min(100)),
            tint: f32::from(self.hg_tint.min(100)),
            refraction: f32::from(self.hg_refraction.min(100)),
            saturation: f32::from(self.hg_saturation.min(100)),
        }
    }

    /// Back to the shipped look (the "Reset to defaults" button).
    pub fn reset_hyperglass(&mut self) {
        [self.hg_frost, self.hg_tint, self.hg_refraction, self.hg_saturation, self.hg_surface] = HG_DEFAULT;
    }

    /// For "Import settings".
    pub fn parse_lenient_pub(text: &str) -> Result<Settings, toml::de::Error> {
        Self::parse_lenient(text)
    }

    pub(crate) fn apply_side_effects(&self) {
        sync_name_index(self.name_index, &self.index_excluded);
        crate::i18n::set(self.language);
        crate::aliases::set(self.folder_aliases.clone());
        crate::background::sync_tray(self.run_in_background);
        crate::quick_search::sync(self);
        rf_core::format::set_date_style(match self.date_format {
            DatePref::Application => rf_core::format::DateStyle::Relative,
            DatePref::System => rf_core::format::DateStyle::Local,
            DatePref::Universal => rf_core::format::DateStyle::Universal,
        });
        rf_core::format::set_decimal_sizes(self.decimal_sizes);
        SOLID.store(self.backdrop == BackdropPref::Solid, Ordering::Relaxed);
        ANIM.store(self.animations as u8, Ordering::Relaxed);
        // HyperGlass: the engine redraws every glass window once when the look changes, and does nothing for the same look again
        HG_SURFACE.store(self.hg_surface.min(100), Ordering::Relaxed);
        rf_win::glass::set_look(self.hyperglass_look());
        gpui_windows::SMOOTH_SCROLL.store(self.smooth_scroll, Ordering::Relaxed);
        THEME.store(
            match self.theme {
                ThemePref::System => 0,
                ThemePref::Light => 1,
                ThemePref::Dark => 2,
            },
            Ordering::Relaxed,
        );
        // An explicit RUSTYFILES_RENDERER env var wins (debugging).
        if std::env::var_os("RUSTYFILES_RENDERER").is_none() {
            gpui_windows::hybrid::set_mode(renderer_mode(self.renderer, crate::background::DOCK_PROCESS.load(Ordering::Relaxed)));
        }
    }

    pub fn save(&self) {
        // One writer at a time; a unique temp name per write (saves come from several threads).
        static LOCK: parking_lot::Mutex<u64> = parking_lot::Mutex::new(0);
        let mut n = LOCK.lock();
        *n += 1;
        let Some(p) = path() else { return };
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Some(dir) = p.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                // write-then-rename so a crash never leaves a half-written file
                let tmp = p.with_extension(format!("toml.{}.{}.tmp", std::process::id(), *n));
                if std::fs::write(&tmp, text).is_ok() {
                    let _ = std::fs::rename(&tmp, &p);
                }
            }
            Err(e) => log::warn!("settings save: {e}"),
        }
    }
}

/// Keeps gpui-component widgets (inputs, menus) in the same light/dark mode as our palette.
pub fn apply_component_theme(cx: &mut App) {
    use gpui_kit::component::theme::{Theme, ThemeMode};
    match cx.global::<Settings>().theme {
        ThemePref::Light => Theme::change(ThemeMode::Light, None, cx),
        ThemePref::Dark => Theme::change(ThemeMode::Dark, None, cx),
        ThemePref::System => Theme::sync_system_appearance(None, cx),
    }
    // The component root paints the theme background under every window; keep it clear so
    // the Mica backdrop shows through (our own surfaces bring their own fills).
    Theme::update(cx, |t| t.background = gpui_kit::transparent_black());
    // the chosen font, else Windows 11's own typeface (optical sizes) when present
    if let Some(f) = cx.global::<Settings>().font.clone() {
        Theme::update(cx, |t| t.font_family = f.into());
    } else if std::path::Path::new(r"C:\Windows\Fonts\SegUIVar.ttf").exists() {
        Theme::update(cx, |t| t.font_family = "Segoe UI Variable Text".into());
    }
}

/// Mutates the global settings, applies side effects, saves in the background and refreshes windows.
pub fn update(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    cx.update_global::<Settings, _>(|s, _| f(s));
    let s = cx.global::<Settings>().clone();
    s.apply_side_effects();
    apply_component_theme(cx);
    std::thread::spawn(move || {
        s.save();
        // after the save: the dock process reads the file when it is told to reload
        crate::dock::control::sync_process(&s);
    });
    cx.refresh_windows();
}

/// Like [`update`] for a control that changes on every step of a drag (a slider): the change is applied and drawn at once, the file is
/// written once the changes have settled (the pane resize does the same), so a drag is not hundreds of writes.
pub fn update_live(cx: &mut App, f: impl FnOnce(&mut Settings)) {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    cx.update_global::<Settings, _>(|s, _| f(s));
    cx.global::<Settings>().apply_side_effects();
    cx.refresh_windows();
    let mine = SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
        // a later step has its own timer: only the last one writes
        if SERIAL.load(Ordering::Relaxed) == mine {
            cx.update(|cx| {
                let snapshot = cx.global::<Settings>().clone();
                std::thread::spawn(move || {
                    snapshot.save();
                    // the running dock takes the look from the file: tell it, or it hears of a slider only at its next look at the file
                    crate::dock::control::sync_process(&snapshot);
                });
            });
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_dock_stays_on_the_graphics_card_unless_the_user_chose_the_software_renderer() {
        use super::{RendererPref as P, renderer_mode};
        use gpui_windows::hybrid::RendererMode as M;
        assert_eq!(renderer_mode(P::Auto, false), M::Auto, "the file manager keeps the automatic switch");
        assert_eq!(renderer_mode(P::Auto, true), M::Gpu, "the dock never builds a software device on its own");
        assert_eq!(renderer_mode(P::Cpu, true), M::Cpu, "a choice made on purpose is kept");
        assert_eq!(renderer_mode(P::Gpu, true), M::Gpu);
        assert_eq!(renderer_mode(P::Gpu, false), M::Gpu);
        assert_eq!(renderer_mode(P::Cpu, false), M::Cpu);
    }

    #[test]
    fn details_moves_to_adaptive_once() {
        // a file from before the move: details becomes adaptive
        let mut s = super::Settings::parse_lenient("default_view = \"details\"\n").unwrap();
        s.migrate();
        assert_eq!(s.default_view, super::DefaultView::Adaptive);
        assert!(s.adaptive_offered);
        // chosen afterwards: stays
        let mut s = super::Settings::parse_lenient("default_view = \"details\"\nadaptive_offered = true\n").unwrap();
        s.migrate();
        assert_eq!(s.default_view, super::DefaultView::Details);
        // grid was a real choice
        let mut s = super::Settings::parse_lenient("default_view = \"grid\"\n").unwrap();
        s.migrate();
        assert_eq!(s.default_view, super::DefaultView::Grid);
    }

    use super::{Settings, ThemePref};

    #[test]
    fn roundtrip_and_defaults() {
        let s = Settings { show_hidden: true, session_tabs: vec!["C:\\".into()], ..Default::default() };
        let t = toml::to_string_pretty(&s).unwrap();
        let back: Settings = toml::from_str(&t).unwrap();
        assert!(back.show_hidden);
        assert_eq!(back.session_tabs, vec!["C:\\".to_string()]);
        // unknown/missing keys fall back to defaults
        let partial: Settings = toml::from_str("theme = \"dark\"").unwrap();
        assert_eq!(partial.theme, ThemePref::Dark);
        assert!(partial.restore_session);
    }
}

/// Where data that can be rebuilt lives (remembered folder sizes): `%LOCALAPPDATA%\rustyfiles`,
/// or `RUSTYFILES_CONFIG_DIR` when set, so test copies keep their data apart.
pub fn local_data_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("RUSTYFILES_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(std::path::PathBuf::from(d));
    }
    dirs::data_local_dir().map(|d| d.join("rustyfiles"))
}

/// `%APPDATA%\rustyfiles`, or `RUSTYFILES_CONFIG_DIR` when set (portable use, tests).
pub fn app_dir() -> Option<std::path::PathBuf> {
    if let Some(d) = std::env::var_os("RUSTYFILES_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(std::path::PathBuf::from(d));
    }
    dirs::config_dir().map(|d| d.join("rustyfiles"))
}

#[cfg(test)]
mod lenient_tests {
    use super::Settings;

    #[test]
    fn unknown_values_fall_back_per_field() {
        let s = Settings::parse_lenient("theme = \"mor\"
show_hidden = true
sidebar_width = \"geniş\"
yeni_alan = 3").unwrap();
        assert!(s.show_hidden, "valid fields survive");
        assert_eq!(s.sidebar_width, Settings::default().sidebar_width);
        assert!(Settings::parse_lenient("[[[bozuk").is_err(), "syntax errors still fail");
    }

    #[test]
    fn glass_backdrop_round_trips() {
        use super::BackdropPref;
        let s = Settings { backdrop: BackdropPref::Glass, ..Default::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("backdrop = \"glass\""), "serialized as \"glass\": {text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.backdrop, BackdropPref::Glass);
        // through the loader the app uses
        assert_eq!(Settings::parse_lenient(&text).unwrap().backdrop, BackdropPref::Glass);
        // the default stays Mica
        assert_eq!(Settings::default().backdrop, BackdropPref::Mica);
    }

    #[test]
    fn hyperglass_defaults_match_the_engine_and_the_shipped_look() {
        let d = Settings::default();
        let engine = rf_win::glass::LookParams::DEFAULT;
        assert_eq!(d.hyperglass_look(), engine, "the settings default is the engine's default");
        assert_eq!(super::HG_DEFAULT.map(f32::from)[..4], [engine.frost, engine.tint, engine.refraction, engine.saturation]);
        assert_eq!(d.hg_surface, 50);
        assert_eq!(super::hg_surface(), 50, "the palette's atomic starts at the default as well");
    }

    #[test]
    fn hyperglass_values_round_trip() {
        let s = Settings { hg_frost: 0, hg_tint: 100, hg_refraction: 13, hg_saturation: 77, hg_surface: 5, ..Default::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        let back = Settings::parse_lenient(&text).unwrap();
        assert_eq!((back.hg_frost, back.hg_tint, back.hg_refraction, back.hg_saturation, back.hg_surface), (0, 100, 13, 77, 5));
        // an old file has none of them: every one is the default
        let old = Settings::parse_lenient("theme = \"dark\"
").unwrap();
        assert_eq!(super::HG_DEFAULT, [old.hg_frost, old.hg_tint, old.hg_refraction, old.hg_saturation, old.hg_surface]);
    }

    #[test]
    fn a_bad_hyperglass_value_costs_only_that_field() {
        // a negative number, a word, a float and a number too big for the field: each falls back alone, the neighbours and the other
        // settings survive
        let s = Settings::parse_lenient(
            "hg_frost = -3
hg_tint = \"cok\"
hg_refraction = 12.5
hg_saturation = 9999
hg_surface = 80
show_hidden = true
",
        )
        .unwrap();
        assert_eq!(s.hg_frost, super::HG_DEFAULT[0]);
        assert_eq!(s.hg_tint, super::HG_DEFAULT[1]);
        assert_eq!(s.hg_refraction, super::HG_DEFAULT[2]);
        assert_eq!(s.hg_saturation, super::HG_DEFAULT[3]);
        assert_eq!(s.hg_surface, 80, "the good value next to the bad ones is kept");
        assert!(s.show_hidden);
    }

    #[test]
    fn a_hyperglass_value_above_100_is_cut_when_used() {
        let s = Settings::parse_lenient("hg_frost = 200
hg_tint = 101
").unwrap();
        assert_eq!((s.hg_frost, s.hg_tint), (200, 101), "the file's number is kept as it is");
        let look = s.hyperglass_look();
        assert_eq!((look.frost, look.tint), (100.0, 100.0));
    }

    #[test]
    fn resetting_hyperglass_leaves_the_rest_alone() {
        let mut s = Settings { hg_frost: 1, hg_tint: 2, hg_refraction: 3, hg_saturation: 4, hg_surface: 5, show_hidden: true, ..Default::default() };
        s.reset_hyperglass();
        assert_eq!(super::HG_DEFAULT, [s.hg_frost, s.hg_tint, s.hg_refraction, s.hg_saturation, s.hg_surface]);
        assert!(s.show_hidden);
    }

    #[test]
    fn unknown_backdrop_value_costs_only_that_field() {
        // what a version without Glass meets after the user picked it and went back: the value
        // is dropped (Mica), every other setting of the file survives
        use super::BackdropPref;
        let s = Settings::parse_lenient("backdrop = \"cam\"
show_hidden = true
theme = \"dark\"
").unwrap();
        assert_eq!(s.backdrop, BackdropPref::Mica);
        assert!(s.show_hidden);
        assert_eq!(s.theme, super::ThemePref::Dark);
    }
}

/// Starts or stops the name index when the setting changes. The first start waits a few seconds
/// so it never competes with opening the window.
fn sync_name_index(on: bool, excluded: &[String]) {
    use std::sync::atomic::{AtomicU8, Ordering};
    // DirectLook reads these settings too: the index is rustyfiles' (a second indexer would walk
    // every drive again and write the same index files)
    if crate::background::side_process() {
        return;
    }
    // 0 = not decided yet, 1 = off, 2 = on
    static STATE: AtomicU8 = AtomicU8::new(0);
    static EXCLUDED: parking_lot::Mutex<Option<Vec<String>>> = parking_lot::Mutex::new(None);
    let list_changed = {
        let mut e = EXCLUDED.lock();
        let changed = e.as_deref() != Some(excluded);
        if changed {
            *e = Some(excluded.to_vec());
            rf_win::nameindex::set_excluded(&excluded.iter().map(std::path::PathBuf::from).collect::<Vec<_>>());
        }
        changed
    };
    let want = if on { 2 } else { 1 };
    let was = STATE.swap(want, Ordering::Relaxed);
    // a changed exclusion list rebuilds a running index (the fresh walk leaves the folders out)
    if was == want && !(on && list_changed && was != 0) {
        return;
    }
    if !on {
        std::thread::spawn(rf_win::indexer::stop);
        return;
    }
    let delay = if was == 0 { 5 } else { 0 };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(delay));
        // extra instances (tests, the speed test's) leave it to the main one
        // (RUSTYFILES_INDEX=1 indexes there too, for testing)
        let extra = (std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_some() || std::env::var_os("RUSTYFILES_INSTANCE").is_some()) && std::env::var_os("RUSTYFILES_INDEX").is_none();
        if STATE.load(Ordering::Relaxed) != 2 || std::env::var_os("RUSTYFILES_NO_INDEX").is_some() || extra {
            return;
        }
        // a copy with its own settings folder (tests, portable) keeps its index there too
        let dir = match std::env::var_os("RUSTYFILES_CONFIG_DIR") {
            Some(cfg) => std::path::PathBuf::from(cfg).join("index"),
            None => std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, std::path::PathBuf::from).join("rustyfiles").join("index"),
        };
        // RUSTYFILES_INDEX_ROOTS="E:\;F:\" picks the roots (testing)
        let roots = match std::env::var("RUSTYFILES_INDEX_ROOTS") {
            Ok(v) => v.split(';').filter(|s| !s.is_empty()).map(std::path::PathBuf::from).collect(),
            Err(_) => rf_win::indexer::local_roots(),
        };
        rf_win::indexer::start(roots, dir);
    });
}

#[cfg(test)]
mod dock_tests {
    use super::Settings;

    #[test]
    fn the_dock_is_off_keeps_windows_out_from_under_it_and_has_no_city() {
        let s = Settings::default();
        assert!(!s.dock_enabled, "experimental: off until asked for");
        assert!(s.dock_reserve);
        assert!(s.dock_weather_city.is_empty(), "no city, so no weather request");
        // a settings file from before the dock has none of the keys: the same defaults
        let old = Settings::parse_lenient("theme = \"dark\"\n").unwrap();
        assert!(!old.dock_enabled && old.dock_reserve && old.dock_weather_city.is_empty());
    }

    #[test]
    fn the_dock_opens_our_own_control_centre_unless_told_not_to() {
        assert!(Settings::default().dock_own_control_centre);
        assert!(Settings::parse_lenient("theme = \"dark\"
").unwrap().dock_own_control_centre, "a file from before it: on");
        let off = Settings { dock_own_control_centre: false, ..Default::default() };
        let back = Settings::parse_lenient(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_own_control_centre);
        assert!(Settings::parse_lenient("dock_own_control_centre = 3
").unwrap().dock_own_control_centre, "a wrong value falls back to the default");
    }

    #[test]
    fn the_dock_opens_our_own_calendar_and_notification_panels_unless_told_not_to() {
        let d = Settings::default();
        assert!(d.dock_own_calendar && d.dock_own_notifications);
        // a settings file from before them has neither key: both on
        let old = Settings::parse_lenient("theme = \"dark\"
").unwrap();
        assert!(old.dock_own_calendar && old.dock_own_notifications, "a file from before them: on");
        let off = Settings { dock_own_calendar: false, dock_own_notifications: false, ..Default::default() };
        let back = Settings::parse_lenient(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_own_calendar && !back.dock_own_notifications, "off survives a save and a load");
        let wrong = Settings::parse_lenient("dock_own_calendar = \"nein\"
dock_own_notifications = 3
").unwrap();
        assert!(wrong.dock_own_calendar && wrong.dock_own_notifications, "wrong values fall back to the default");
    }

    #[test]
    fn the_dock_opens_our_own_start_unless_told_not_to() {
        assert!(Settings::default().dock_own_start);
        assert!(Settings::parse_lenient("theme = \"dark\"\n").unwrap().dock_own_start, "a file from before it: on");
        let off = Settings { dock_own_start: false, ..Default::default() };
        let back = Settings::parse_lenient(&toml::to_string_pretty(&off).unwrap()).unwrap();
        assert!(!back.dock_own_start);
        assert!(Settings::parse_lenient("dock_own_start = \"nein\"\n").unwrap().dock_own_start, "a wrong value falls back to the default");
    }

    #[test]
    fn the_dock_settings_round_trip() {
        let s = Settings { dock_enabled: true, dock_reserve: false, dock_weather_city: "Erzincan".into(), ..Default::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        let back = Settings::parse_lenient(&text).unwrap();
        assert!(back.dock_enabled);
        assert!(!back.dock_reserve);
        assert_eq!(back.dock_weather_city, "Erzincan");
    }

    #[test]
    fn the_magnification_is_full_by_default_and_round_trips() {
        use super::DockMagnify;
        use rf_win::dock::magnify::Strength;
        assert_eq!(Settings::default().dock_magnify, DockMagnify::Full);
        assert_eq!(Settings::parse_lenient("theme = \"dark\"\n").unwrap().dock_magnify, DockMagnify::Full, "a file from before has none: the default");
        for (m, s, name) in [(DockMagnify::Off, Strength::Off, "off"), (DockMagnify::Subtle, Strength::Subtle, "subtle"), (DockMagnify::Full, Strength::Full, "full")] {
            assert_eq!(m.strength(), s);
            let text = toml::to_string_pretty(&Settings { dock_magnify: m, ..Default::default() }).unwrap();
            assert!(text.contains(&format!("dock_magnify = \"{name}\"")), "{text}");
            assert_eq!(Settings::parse_lenient(&text).unwrap().dock_magnify, m);
        }
        // a value this version does not know falls back to the default, the rest of the file is kept
        let s = Settings::parse_lenient("dock_magnify = \"huge\"\nshow_hidden = true\n").unwrap();
        assert_eq!(s.dock_magnify, DockMagnify::Full);
        assert!(s.show_hidden);
    }

    #[test]
    fn a_wrong_dock_value_falls_back_without_costing_the_other_settings() {
        let s = Settings::parse_lenient("dock_enabled = \"evet\"\ndock_reserve = false\nshow_hidden = true\n").unwrap();
        assert!(!s.dock_enabled);
        assert!(!s.dock_reserve);
        assert!(s.show_hidden);
    }
}
