//! Where the desktop view gets its items, its monitors and its layout file from. This is the one small seam between the view and the
//! system: the view is given a [`Backend`] and a [`Store`] and does not know whether the items are the user's real Desktop or a
//! scratch folder. Shell plan F4 builds a `Backend::system` once and hands clones of it to its per-monitor windows.
//!
//! THE SCRATCH RULE. With a scratch folder only its contents are shown (no Public Desktop, no virtual items), the layout goes into a
//! file of its own beside the real one and never into it, and every operation acts on that folder. The real Desktop is the default
//! otherwise and is then READ-ONLY unless asked: see [`super::args`].
//!
//! Everything here may block (it lists folders, asks the shell for the Desktop's path and the wallpaper): call [`load`] and
//! [`read_wallpaper`] off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_core::desktop::items::{DesktopFile, FolderOrigin, ItemChange, Visibility};
use rf_core::desktop::layout::MonitorSpec;
use rf_core::desktop::registry::{Hive, RegRead};
use rf_core::desktop::store::{LoadOutcome, Store};
use rf_core::desktop::virtuals::{KEY_ADVANCED, VirtualKind, VirtualSettings, Why};
use rf_core::desktop::wallpaper::Wallpaper;
use rf_win::desktop::items::{DesktopItems, Folders, PathFolders, SystemFolders, SystemVirtuals, VirtualSource};
use rf_win::desktop::monitors::{self, MonitorInfo};
use rf_win::desktop::registry::SystemRegistry;

use super::args::PreviewArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Real,
    Scratch(PathBuf),
}

/// The live items, shareable between the loader thread, the folder watchers' threads and the UI thread.
///
/// `DesktopItems` owns a `RegWatch`, which is `Send` (rf-win says why) but not `Sync` because it holds an event handle. The handle is
/// only closed by `Drop`; every method of `DesktopItems` takes `&self` and locks what it touches, so sharing a reference between
/// threads is sound.
pub struct SharedItems(pub Arc<DesktopItems>);

unsafe impl Send for SharedItems {}
unsafe impl Sync for SharedItems {}

impl std::ops::Deref for SharedItems {
    type Target = DesktopItems;
    fn deref(&self) -> &DesktopItems {
        &self.0
    }
}

/// What the view needs of its surroundings, apart from the layout [`Store`] it owns.
pub struct Backend {
    pub mode: Mode,
    /// The Desktop folders must not change (the real Desktop without `--writable`).
    pub read_only: bool,
    pub items: SharedItems,
    pub folders: Arc<dyn Folders>,
    pub infos: Vec<MonitorInfo>,
    pub specs: Vec<MonitorSpec>,
    /// Index into `specs` of the monitor this view draws.
    pub canvas: usize,
    pub layout_path: PathBuf,
    pub visibility: Visibility,
    /// `ListviewShadow`: the soft shadow under icon labels.
    pub label_shadow: bool,
}

impl Backend {
    /// The folder new files, pasted files and dropped files go into: the user's Desktop, or the scratch folder.
    pub fn target_dir(&self) -> Option<PathBuf> {
        self.folders.dir(FolderOrigin::User)
    }
    pub fn dir_of(&self, origin: rf_core::desktop::items::Origin) -> Option<PathBuf> {
        use rf_core::desktop::items::Origin;
        match origin {
            Origin::User => self.folders.dir(FolderOrigin::User),
            Origin::Public => self.folders.dir(FolderOrigin::Public),
            Origin::Virtual(_) => None,
        }
    }
    pub fn is_scratch(&self) -> bool {
        matches!(self.mode, Mode::Scratch(_))
    }
}

/// Everything [`load`] hands back.
pub struct Loaded {
    pub backend: Backend,
    pub store: Store,
    pub outcome: LoadOutcome,
    /// Why the scratch folder was not used, when it was asked for and refused.
    pub note: Option<String>,
}

/// A scratch folder is a Desktop of its own: the Public one is empty, nothing is virtual.
struct ScratchFolders {
    user: PathFolders,
}

impl Folders for ScratchFolders {
    fn dir(&self, origin: FolderOrigin) -> Option<PathBuf> {
        match origin {
            FolderOrigin::User => self.user.dir(origin),
            FolderOrigin::Public => None,
        }
    }
    fn list(&self, origin: FolderOrigin) -> Result<Vec<DesktopFile>, String> {
        match origin {
            FolderOrigin::User => self.user.list(origin),
            FolderOrigin::Public => Ok(Vec::new()),
        }
    }
    fn stat(&self, origin: FolderOrigin, name: &str) -> Option<DesktopFile> {
        match origin {
            FolderOrigin::User => self.user.stat(origin, name),
            FolderOrigin::Public => None,
        }
    }
}

struct NoVirtuals;

impl VirtualSource for NoVirtuals {
    fn settings(&self) -> VirtualSettings {
        VirtualSettings { kinds: VirtualKind::ALL.into_iter().map(|k| (k, Why::Set(false))).collect(), show_icons: true, policy_no_desktop: false }
    }
    fn recycle_full(&self) -> bool {
        false
    }
}

/// The layout file's name. The real one is `desktop-layout.json`; a scratch folder gets one of its own, named after the folder, so
/// that two scratch folders do not share cells and the real file is never touched.
pub fn layout_file_name(scratch: Option<&Path>) -> String {
    match scratch {
        None => "desktop-layout.json".to_string(),
        Some(dir) => {
            let key = dir.to_string_lossy().to_lowercase().replace('/', "\\");
            let key = key.trim_end_matches('\\');
            // FNV-1a: stable across runs and Rust versions (std's hasher is neither promised)
            let mut h: u32 = 0x811c9dc5;
            for b in key.bytes() {
                h ^= u32::from(b);
                h = h.wrapping_mul(0x01000193);
            }
            format!("desktop-layout-scratch-{h:08x}.json")
        }
    }
}

fn same_folder(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().to_lowercase();
    let norm = |s: String| s.trim_start_matches(r"\\?\").trim_end_matches('\\').to_string();
    norm(canon(a)) == norm(canon(b))
}

/// Decides what the arguments mean: the scratch folder, unless it is not a folder or IS one of the real Desktop folders (then the
/// request is read as "the real Desktop", with the real Desktop's protection, and the note says so).
pub fn resolve_mode(args: &PreviewArgs, real_user: Option<&Path>, real_public: Option<&Path>) -> (Mode, Option<String>) {
    let Some(dir) = &args.folder else { return (Mode::Real, None) };
    if !dir.is_dir() {
        return (Mode::Real, Some(format!("not a folder: {}", dir.display())));
    }
    if [real_user, real_public].into_iter().flatten().any(|r| same_folder(r, dir)) {
        return (Mode::Real, Some("that is the real Desktop folder, so the real Desktop's protection applies".to_string()));
    }
    (Mode::Scratch(dir.clone()), None)
}

/// Whether the Desktop folders are protected. FAILS CLOSED: a scratch folder that was asked for and could not be used (a typo, a
/// folder that is gone, the real Desktop itself) leaves the real Desktop on screen, and `--writable` given in the same breath must
/// not open it for changes: only a request with no folder at all may honour `--writable`.
pub fn read_only_for(args: &PreviewArgs, mode: &Mode) -> bool {
    match mode {
        Mode::Scratch(_) => false,
        Mode::Real => args.folder.is_some() || !args.writable,
    }
}

/// Whether the layout file is written. A scratch folder has a file of its own and keeps its layout as it goes. The REAL Desktop's
/// layout file is the one the real desktop will use one day, so merely looking at the Desktop must not create or change it (the first
/// placement of every item would otherwise be saved as if the user had chosen it): it is kept from the first arrangement on.
pub fn keeps_layout(scratch: bool, touched: bool) -> bool {
    scratch || touched
}

/// The user arranged something in the real Desktop's layout. That also settles the one-time import of Explorer's icon positions:
/// what the user has arranged here must not be overwritten by it later (`Store::import_positions` does nothing once it is set).
pub fn arranged_by_user(store: &mut Store, scratch: bool) {
    if !scratch {
        store.mark_imported();
    }
}

fn pick_canvas(infos: &[MonitorInfo], wanted: Option<usize>) -> usize {
    wanted.map(|n| n.saturating_sub(1)).filter(|i| *i < infos.len()).unwrap_or(0)
}

/// Lists the folders, finds the monitors and loads the layout. `on_change` is called from watcher threads.
pub fn load(args: &PreviewArgs, visibility: Visibility, config_dir: Option<PathBuf>, on_change: impl Fn(Vec<ItemChange>) + Send + Sync + 'static) -> Loaded {
    let real_user = rf_win::desktop::items::user_desktop_dir();
    let real_public = rf_win::desktop::items::public_desktop_dir();
    let (mode, note) = resolve_mode(args, real_user.as_deref(), real_public.as_deref());
    let (folders, virtuals, watch_registry): (Arc<dyn Folders>, Arc<dyn VirtualSource>, bool) = match &mode {
        Mode::Scratch(dir) => (Arc::new(ScratchFolders { user: PathFolders { user: Some(dir.clone()), public: None } }), Arc::new(NoVirtuals), false),
        Mode::Real => (Arc::new(SystemFolders::new()), Arc::new(SystemVirtuals { registry: Arc::new(SystemRegistry) }), true),
    };
    // (the wrapper above is what makes this Arc fit for threads)
    #[allow(clippy::arc_with_non_send_sync)]
    let items = SharedItems(Arc::new(DesktopItems::start(folders.clone(), virtuals, visibility, watch_registry, on_change)));
    let infos = monitors::list();
    let specs: Vec<MonitorSpec> = infos.iter().map(|m| m.spec()).collect();
    let canvas = pick_canvas(&infos, args.monitor);
    let scratch_dir = match &mode {
        Mode::Scratch(d) => Some(d.as_path()),
        Mode::Real => None,
    };
    let layout_path = config_dir.unwrap_or_else(|| std::env::temp_dir().join("rustyfiles")).join(layout_file_name(scratch_dir));
    let (store, outcome) = Store::load(&layout_path);
    let read_only = read_only_for(args, &mode);
    let label_shadow = SystemRegistry.dword(Hive::CurrentUser, KEY_ADVANCED, "ListviewShadow").is_none_or(|v| v != 0);
    Loaded { backend: Backend { mode, read_only, items, folders, infos, specs, canvas, layout_path, visibility, label_shadow }, store, outcome, note }
}

/// The user's wallpaper (read-only), for the monitors given.
pub fn read_wallpaper(infos: &[MonitorInfo]) -> Wallpaper {
    rf_win::desktop::wallpaper::current(Arc::new(SystemRegistry), &monitors::device_rects(infos))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-deskprev-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn args(folder: Option<&Path>) -> PreviewArgs {
        PreviewArgs { folder: folder.map(Path::to_path_buf), ..Default::default() }
    }

    #[test]
    fn the_real_layout_file_is_never_a_scratch_layout_file() {
        let real = layout_file_name(None);
        assert_eq!(real, "desktop-layout.json");
        let a = layout_file_name(Some(Path::new(r"C:\Temp\Fake Desktop")));
        let b = layout_file_name(Some(Path::new(r"c:/temp/fake desktop/")));
        let c = layout_file_name(Some(Path::new(r"C:\Temp\Other")));
        assert_ne!(a, real);
        assert!(a.starts_with("desktop-layout-scratch-") && a.ends_with(".json"));
        assert_eq!(a, b, "the same folder written differently is the same file");
        assert_ne!(a, c, "another folder, another file");
    }

    #[test]
    fn a_folder_argument_makes_a_scratch_desktop_unless_it_is_not_a_folder_or_is_the_real_one() {
        let scratch = temp("mode");
        let real = temp("real");
        let public = temp("public");
        let (m, note) = resolve_mode(&args(Some(&scratch)), Some(&real), Some(&public));
        assert_eq!((m, note), (Mode::Scratch(scratch.clone()), None));
        // no argument: the real Desktop
        assert_eq!(resolve_mode(&args(None), Some(&real), Some(&public)), (Mode::Real, None));
        // the real Desktop folder given as the "scratch" one keeps the real protection (also written differently)
        let sneaky = PathBuf::from(format!("{}\\", real.display()));
        let (m, note) = resolve_mode(&args(Some(&sneaky)), Some(&real), Some(&public));
        assert_eq!(m, Mode::Real);
        assert!(note.unwrap().contains("real Desktop"));
        let (m, _) = resolve_mode(&args(Some(&public)), Some(&real), Some(&public));
        assert_eq!(m, Mode::Real, "the Public Desktop too");
        // a path that is not a folder
        let (m, note) = resolve_mode(&args(Some(&scratch.join("nope"))), Some(&real), Some(&public));
        assert_eq!(m, Mode::Real);
        assert!(note.is_some());
        for d in [scratch, real, public] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn a_scratch_desktop_has_no_public_folder_and_no_virtual_items_and_stays_inside_its_folder() {
        let dir = temp("scratch");
        std::fs::write(dir.join("a.txt"), b"a").unwrap();
        std::fs::create_dir_all(dir.join("Sub")).unwrap();
        let f = ScratchFolders { user: PathFolders { user: Some(dir.clone()), public: None } };
        let mut names: Vec<String> = f.list(FolderOrigin::User).unwrap().into_iter().map(|d| d.name).collect();
        names.sort();
        assert_eq!(names, ["Sub", "a.txt"]);
        assert_eq!(f.list(FolderOrigin::Public), Ok(Vec::new()), "an empty Public Desktop, not an unknown one: the listing counts as complete");
        assert_eq!(f.dir(FolderOrigin::Public), None);
        assert!(f.stat(FolderOrigin::Public, "a.txt").is_none());
        assert!(f.stat(FolderOrigin::User, "a.txt").is_some());
        let v = NoVirtuals.settings();
        assert!(v.shown().is_empty() && !v.desktop_disabled());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn loading_a_scratch_folder_uses_its_own_layout_file_beside_the_config_and_lists_only_its_contents() {
        let dir = temp("load");
        let cfg = temp("cfg");
        std::fs::write(dir.join("one.txt"), b"1").unwrap();
        std::fs::write(dir.join("two.txt"), b"2").unwrap();
        let l = load(&args(Some(&dir)), Visibility::default(), Some(cfg.clone()), |_| {});
        assert_eq!(l.backend.mode, Mode::Scratch(dir.clone()));
        assert!(!l.backend.read_only);
        assert_eq!(l.backend.layout_path.parent(), Some(cfg.as_path()));
        assert_ne!(l.backend.layout_path.file_name().unwrap(), "desktop-layout.json");
        assert_eq!(l.outcome, LoadOutcome::NoFile);
        let got: Vec<String> = l.backend.items.items().into_iter().map(|i| i.id.to_string()).collect();
        assert_eq!(got, ["u:one.txt", "u:two.txt"]);
        assert!(l.backend.items.listing().complete, "both folders count as listed");
        assert!(l.backend.specs.is_empty() || l.backend.canvas < l.backend.specs.len(), "the canvas index points at a monitor (or there is none)");
        drop(l);
        let _ = std::fs::remove_dir_all(dir);
        let _ = std::fs::remove_dir_all(cfg);
    }

    #[test]
    fn the_real_desktop_is_read_only_unless_writable() {
        let cfg = temp("cfgreal");
        // the real folders are only LISTED here (a count), nothing is changed
        let l = load(&args(None), Visibility::default(), Some(cfg.clone()), |_| {});
        assert_eq!(l.backend.mode, Mode::Real);
        assert!(l.backend.read_only);
        assert_eq!(l.backend.layout_path, cfg.join("desktop-layout.json"));
        drop(l);
        let a = PreviewArgs { writable: true, ..Default::default() };
        let l = load(&a, Visibility::default(), Some(cfg.clone()), |_| {});
        assert!(!l.backend.read_only);
        drop(l);
        // a scratch folder that is not there falls back to the real Desktop, still protected whatever --writable says
        let missing = PreviewArgs { folder: Some(cfg.join("not-there")), writable: true, ..Default::default() };
        let l = load(&missing, Visibility::default(), Some(cfg.clone()), |_| {});
        assert_eq!(l.backend.mode, Mode::Real);
        assert!(l.backend.read_only, "fail closed");
        drop(l);
        let _ = std::fs::remove_dir_all(cfg);
    }

    #[test]
    fn a_scratch_folder_that_could_not_be_used_never_makes_the_real_desktop_writable() {
        let scratch = temp("failclosed");
        let real = temp("failclosed-real");
        let writable = |folder: Option<&Path>| PreviewArgs { folder: folder.map(Path::to_path_buf), writable: true, ..Default::default() };
        // a typo / a folder that is not there, and --writable given anyway
        let bad = writable(Some(&scratch.join("deskk")));
        let (mode, _) = resolve_mode(&bad, Some(&real), None);
        assert_eq!(mode, Mode::Real);
        assert!(read_only_for(&bad, &mode), "asked for a scratch folder, got the real Desktop: stays protected");
        // the real Desktop folder named as the scratch folder
        let same = writable(Some(&real));
        let (mode, _) = resolve_mode(&same, Some(&real), None);
        assert!(read_only_for(&same, &mode));
        // a usable scratch folder is writable, as before
        let ok = writable(Some(&scratch));
        let (mode, _) = resolve_mode(&ok, Some(&real), None);
        assert_eq!(mode, Mode::Scratch(scratch.clone()));
        assert!(!read_only_for(&ok, &mode));
        // no folder: --writable is honoured, and its absence protects
        assert!(!read_only_for(&writable(None), &Mode::Real));
        assert!(read_only_for(&args(None), &Mode::Real));
        for d in [scratch, real] {
            let _ = std::fs::remove_dir_all(d);
        }
    }

    #[test]
    fn looking_at_the_real_desktop_writes_no_layout_file_until_something_is_arranged() {
        assert!(keeps_layout(true, false), "a scratch folder keeps its own file as it goes");
        assert!(!keeps_layout(false, false), "looking only: nothing is written");
        assert!(keeps_layout(false, true), "arranged: kept");
        assert!(keeps_layout(true, true));
    }

    #[test]
    fn arranging_the_real_desktop_settles_the_one_time_import_of_explorers_positions() {
        let mut real = Store::default();
        assert!(!real.imported_from_explorer);
        arranged_by_user(&mut real, false);
        assert!(real.imported_from_explorer, "the user's arrangement is not replaced by the import later");
        assert!(!real.import_positions(&[], &[], false), "and the import now does nothing");
        // a scratch folder has nothing to do with Explorer's positions
        let mut scratch = Store::default();
        arranged_by_user(&mut scratch, true);
        assert!(!scratch.imported_from_explorer);
    }

    #[test]
    fn the_monitor_option_picks_a_monitor_or_falls_back_to_the_first() {
        let mk = |n: usize| (0..n).map(|i| MonitorInfo { device_key: format!("m{i}"), adapter_name: String::new(), rect: Default::default(), work: Default::default(), primary: i == 0, dpi: 96, scale_percent: 100 }).collect::<Vec<_>>();
        assert_eq!(pick_canvas(&mk(2), None), 0);
        assert_eq!(pick_canvas(&mk(2), Some(2)), 1);
        assert_eq!(pick_canvas(&mk(2), Some(3)), 0, "no third monitor");
        assert_eq!(pick_canvas(&[], Some(1)), 0);
    }
}
