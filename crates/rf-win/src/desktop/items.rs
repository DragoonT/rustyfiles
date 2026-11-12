//! The desktop's items, live (shell plan F2): the user's Desktop folder and the Public one, found by asking the system for the
//! known folders (a OneDrive-redirected or renamed Desktop is whatever Windows says, no path is ever built by hand), merged with the
//! virtual items the user enabled, kept up to date by `watch.rs` (`ReadDirectoryChangesW`) on both folders and by a registry watch on
//! the settings that decide the virtual items. The merge, the hidden / system rules and the added / removed / renamed diff are
//! `rf_core::desktop::items`.
//!
//! NOT HERE (yet): `SHChangeNotifyRegister` (needs a window, F3 owns one; whether it works with no Explorer is unverified, test T4, so
//! `watch.rs` is the floor), the shell's own icons and overlays (F3), and the shell's localised labels of the virtual items.
//!
//! READ-ONLY. Listing a folder never opens a file; a OneDrive placeholder is listed by its attributes and nothing is downloaded.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use rf_core::desktop::items::{DesktopFile, DesktopItem, DesktopModel, FolderOrigin, ItemChange, VirtualState, Visibility};
use rf_core::desktop::registry::RegRead;
use rf_core::desktop::virtuals::{self, KEY_ADVANCED, KEY_HIDE_ICONS, KEY_POLICIES_EXPLORER, VirtualSettings};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop, KF_FLAG_DEFAULT, SHGetKnownFolderPath};
use windows::core::GUID;

use crate::watch::{Changes, DirWatch};

/// The two folders on disk.
pub trait Folders: Send + Sync {
    fn dir(&self, origin: FolderOrigin) -> Option<PathBuf>;
    /// A full listing. `Err` is "could not list" (the model keeps what it had; it is never read as an empty folder).
    fn list(&self, origin: FolderOrigin) -> Result<Vec<DesktopFile>, String>;
    /// One name as it is on disk now (`None`: it is not there).
    fn stat(&self, origin: FolderOrigin, name: &str) -> Option<DesktopFile>;
}

/// Where the virtual items' settings and the Recycle Bin's state come from.
pub trait VirtualSource: Send + Sync {
    fn settings(&self) -> VirtualSettings;
    fn recycle_full(&self) -> bool;
}

/// The user's own folder (`C:\Users\name`), what the "User's files" desktop icon stands for.
pub fn profile_dir() -> Option<PathBuf> {
    crate::known::KnownFolder::Profile.path()
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    let id = *id;
    crate::sta::call(super::SHELL_LIMIT, move || unsafe {
        let p = SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    })
    .flatten()
}

/// The user's Desktop folder, wherever Windows has put it (`FOLDERID_Desktop`: localised, possibly redirected to OneDrive).
pub fn user_desktop_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop)
}

/// The Public (all users) Desktop folder (`FOLDERID_PublicDesktop`).
pub fn public_desktop_dir() -> Option<PathBuf> {
    known_folder(&FOLDERID_PublicDesktop)
}

/// The real folders. A path is asked of the system until it has been answered once: the shell thread can be busy for longer than the
/// limit at start-up (the dock loads its icons through it), and a "no answer" must not become a Desktop that is empty for the whole
/// session.
pub struct SystemFolders {
    user: Mutex<Option<PathBuf>>,
    public: Mutex<Option<PathBuf>>,
}

impl SystemFolders {
    pub fn new() -> Self {
        SystemFolders { user: Mutex::new(user_desktop_dir()), public: Mutex::new(public_desktop_dir()) }
    }
    fn lookup(&self, origin: FolderOrigin) -> Option<PathBuf> {
        let (slot, ask) = match origin {
            FolderOrigin::User => (&self.user, user_desktop_dir as fn() -> Option<PathBuf>),
            FolderOrigin::Public => (&self.public, public_desktop_dir as fn() -> Option<PathBuf>),
        };
        let mut known = slot.lock();
        if known.is_none() {
            *known = ask();
        }
        known.clone()
    }
}

impl Default for SystemFolders {
    fn default() -> Self {
        Self::new()
    }
}

/// Folders given as paths (tests, or a scratch folder pretending to be the Desktop).
pub struct PathFolders {
    pub user: Option<PathBuf>,
    pub public: Option<PathBuf>,
}

fn list_path(dir: &Path) -> Result<Vec<DesktopFile>, String> {
    let mut out = Vec::new();
    crate::list_dir(dir, 512, &|| false, &mut |batch| out.extend(batch.iter().map(DesktopFile::from_entry))).map_err(|e| e.to_string())?;
    Ok(out)
}

fn stat_path(dir: &Path, name: &str) -> Option<DesktopFile> {
    crate::stat_entry(dir, name).ok().flatten().map(|e| DesktopFile::from_entry(&e))
}

impl Folders for PathFolders {
    fn dir(&self, origin: FolderOrigin) -> Option<PathBuf> {
        match origin {
            FolderOrigin::User => self.user.clone(),
            FolderOrigin::Public => self.public.clone(),
        }
    }
    fn list(&self, origin: FolderOrigin) -> Result<Vec<DesktopFile>, String> {
        list_dir_of(self.dir(origin), origin)
    }
    fn stat(&self, origin: FolderOrigin, name: &str) -> Option<DesktopFile> {
        stat_path(&self.dir(origin)?, name)
    }
}

impl Folders for SystemFolders {
    fn dir(&self, origin: FolderOrigin) -> Option<PathBuf> {
        self.lookup(origin)
    }
    fn list(&self, origin: FolderOrigin) -> Result<Vec<DesktopFile>, String> {
        list_dir_of(self.lookup(origin), origin)
    }
    fn stat(&self, origin: FolderOrigin, name: &str) -> Option<DesktopFile> {
        stat_path(&self.lookup(origin)?, name)
    }
}

/// A folder that is not known is NOT an empty folder: `Err`, so that the model stays "not listed" and nothing is forgotten.
fn list_dir_of(dir: Option<PathBuf>, origin: FolderOrigin) -> Result<Vec<DesktopFile>, String> {
    match dir {
        Some(d) => list_path(&d),
        None => Err(format!("the {origin:?} desktop folder is not known (the system did not say where it is)")),
    }
}

/// The last answer the shell gave about the Recycle Bin. A question the shell did not answer (its thread was busy past the limit,
/// or the call failed) is NOT "empty": the bin keeps the picture it had, so that a refresh that lands while the shell is occupied
/// (a context menu, a start-up burst) never flips a full bin to the empty picture.
#[derive(Default)]
pub struct LastKnown(AtomicBool);

impl LastKnown {
    /// `answer`: `Some(full)` when the shell said, `None` when it did not. Returns what to show.
    pub fn settle(&self, answer: Option<bool>) -> bool {
        match answer {
            Some(full) => {
                self.0.store(full, Ordering::SeqCst);
                full
            }
            None => self.0.load(Ordering::SeqCst),
        }
    }
}

/// How the real bin is asked: on the shell thread, `None` when it did not answer in time or failed.
fn ask_the_shell() -> Option<bool> {
    crate::sta::call(super::SHELL_LIMIT, || crate::recycle::info().ok().map(|(n, _)| n > 0)).flatten()
}

/// The real settings: the registry, and the Recycle Bin through `SHQueryRecycleBin` on the shell thread.
pub struct SystemVirtuals {
    pub registry: Arc<dyn RegRead + Send + Sync>,
    last: LastKnown,
    ask: Arc<dyn Fn() -> Option<bool> + Send + Sync>,
}

impl SystemVirtuals {
    pub fn new(registry: Arc<dyn RegRead + Send + Sync>) -> Self {
        SystemVirtuals { registry, last: LastKnown::default(), ask: Arc::new(ask_the_shell) }
    }
    /// With another way to ask about the bin (tests: no real bin is touched or even looked at).
    pub fn with_asker(registry: Arc<dyn RegRead + Send + Sync>, ask: Arc<dyn Fn() -> Option<bool> + Send + Sync>) -> Self {
        SystemVirtuals { registry, last: LastKnown::default(), ask }
    }
}

impl VirtualSource for SystemVirtuals {
    fn settings(&self) -> VirtualSettings {
        virtuals::read(self.registry.as_ref())
    }
    fn recycle_full(&self) -> bool {
        self.last.settle((self.ask)())
    }
}

/// The real (or a scratch) settings with the app's own choices on top ([`virtuals::apply`]): which icons are shown is Windows' answer
/// changed by what the user chose in this app's settings. `choices` is asked at every read, so a changed setting needs only a
/// [`DesktopItems::refresh_virtuals`]. With every choice on "follow Windows" the answer is Windows' own, unchanged.
pub struct ChosenVirtuals {
    pub inner: Arc<dyn VirtualSource>,
    pub choices: Arc<dyn Fn() -> virtuals::Choices + Send + Sync>,
}

impl VirtualSource for ChosenVirtuals {
    fn settings(&self) -> VirtualSettings {
        virtuals::apply(self.inner.settings(), &(self.choices)())
    }
    fn recycle_full(&self) -> bool {
        self.inner.recycle_full()
    }
}

struct State {
    model: DesktopModel,
}

/// The live desktop items. Dropping it stops the watches.
pub struct DesktopItems {
    state: Arc<Mutex<State>>,
    folders: Arc<dyn Folders>,
    virtuals: Arc<dyn VirtualSource>,
    on_change: Arc<dyn Fn(Vec<ItemChange>) + Send + Sync>,
    /// The folders that are watched. A folder whose path was not known at start is added by [`rescan`](DesktopItems::rescan).
    dir_watches: Mutex<Vec<(FolderOrigin, DirWatch)>>,
    _reg_watch: Option<super::regnotify::RegWatch>,
    /// The Recycle Bin's folders, watched while its icon is shown (see [`DesktopItems::watch_recycle_bin`]).
    bin: Arc<BinFollow>,
}

/// What the bin's folder watch holds: one watch per folder, and the job they poke (it ends when this is dropped).
#[derive(Default)]
struct BinWatch {
    watches: Vec<(PathBuf, DirWatch)>,
    job: Option<super::Coalescer>,
}

/// Keeps the Recycle Bin's icon in step with the bin. The watch exists only while it was asked for AND the bin's icon is shown (a
/// hidden icon needs no watcher per drive and no shell question), and each [`sync`](BinFollow::sync) also picks up a bin folder
/// that did not exist before (a drive's first delete creates it).
struct BinFollow {
    state: Arc<Mutex<State>>,
    virtuals: Arc<dyn VirtualSource>,
    on_change: Arc<dyn Fn(Vec<ItemChange>) + Send + Sync>,
    /// The folders to watch now (the real ones; tests give their own).
    dirs: Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
    wanted: AtomicBool,
    shown: AtomicBool,
    slot: Mutex<BinWatch>,
}

impl BinFollow {
    fn new(state: Arc<Mutex<State>>, virtuals: Arc<dyn VirtualSource>, on_change: Arc<dyn Fn(Vec<ItemChange>) + Send + Sync>, dirs: Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>) -> Self {
        BinFollow { state, virtuals, on_change, dirs, wanted: AtomicBool::new(false), shown: AtomicBool::new(false), slot: Mutex::new(BinWatch::default()) }
    }

    /// The bin's icon is shown (or not) now: start, top up or stop the watch.
    fn sync(&self, shown: bool) {
        self.shown.store(shown, Ordering::SeqCst);
        self.apply();
    }

    fn apply(&self) {
        if !self.wanted.load(Ordering::SeqCst) {
            return;
        }
        let mut w = self.slot.lock();
        if !self.shown.load(Ordering::SeqCst) {
            *w = BinWatch::default();
            return;
        }
        let poke = w
            .job
            .get_or_insert_with(|| {
                // (this job does not call back into `sync`: it would hold the follower alive from its own thread)
                let (st, vs, cb) = (self.state.clone(), self.virtuals.clone(), self.on_change.clone());
                super::Coalescer::start(Duration::from_millis(300), Duration::from_secs(2), move || {
                    let changes = refresh(&st, vs.as_ref(), None);
                    if !changes.is_empty() {
                        cb(changes);
                    }
                })
            })
            .sender();
        for dir in (self.dirs)() {
            if w.watches.iter().any(|(d, _)| *d == dir) {
                continue;
            }
            let poke = poke.clone();
            match crate::watch::watch_dir(&dir, Duration::from_millis(200), move || {
                let _ = poke.send(());
            }) {
                Ok(watch) => w.watches.push((dir, watch)),
                Err(e) => log::warn!("recycle bin folder not watched: {e}"),
            }
        }
    }
}

/// Watches one folder (150 ms debounce) and feeds the bursts to the model.
fn watch_folder(state: &Arc<Mutex<State>>, folders: &Arc<dyn Folders>, cb: &Arc<dyn Fn(Vec<ItemChange>) + Send + Sync>, o: FolderOrigin) -> Option<DirWatch> {
    let dir = folders.dir(o)?;
    let (st, fo, cb) = (state.clone(), folders.clone(), cb.clone());
    match crate::watch::watch_dir_changes(&dir, Duration::from_millis(150), move |c: Changes| {
        let changes = apply_burst(&st, fo.as_ref(), o, c);
        if !changes.is_empty() {
            cb(changes);
        }
    }) {
        Ok(w) => Some(w),
        Err(e) => {
            log::warn!("desktop folder {o:?} not watched: {e}");
            None
        }
    }
}

fn virtual_state(v: &dyn VirtualSource) -> VirtualState {
    let s = v.settings();
    let shown = s.shown();
    let recycle_full = shown.contains(&virtuals::VirtualKind::RecycleBin) && v.recycle_full();
    VirtualState { shown, recycle_full, disabled: s.desktop_disabled() }
}

impl DesktopItems {
    /// Lists both folders once (this blocks: call it off the UI thread), then watches them. `on_change` is called from watcher
    /// threads with what changed after each burst; the list itself is [`items`](Self::items).
    pub fn start(
        folders: Arc<dyn Folders>,
        virtuals: Arc<dyn VirtualSource>,
        visibility: Visibility,
        watch_registry: bool,
        on_change: impl Fn(Vec<ItemChange>) + Send + Sync + 'static,
    ) -> DesktopItems {
        Self::start_with_bin_dirs(folders, virtuals, visibility, watch_registry, on_change, Arc::new(super::binwatch::existing_bin_dirs))
    }

    /// [`start`](Self::start) with another answer to "which folders is the Recycle Bin in" (tests: a folder of their own).
    fn start_with_bin_dirs(
        folders: Arc<dyn Folders>,
        virtuals: Arc<dyn VirtualSource>,
        visibility: Visibility,
        watch_registry: bool,
        on_change: impl Fn(Vec<ItemChange>) + Send + Sync + 'static,
        bin_dirs: Arc<dyn Fn() -> Vec<PathBuf> + Send + Sync>,
    ) -> DesktopItems {
        let mut model = DesktopModel::new(visibility);
        for o in [FolderOrigin::User, FolderOrigin::Public] {
            match folders.list(o) {
                Ok(files) => {
                    model.set_files(o, files);
                }
                Err(e) => log::warn!("desktop folder {o:?} not listed: {e}"),
            }
        }
        let first = virtual_state(virtuals.as_ref());
        let bin_shown = first.shown.contains(&virtuals::VirtualKind::RecycleBin);
        model.set_virtuals(first);
        let state = Arc::new(Mutex::new(State { model }));
        let on_change: Arc<dyn Fn(Vec<ItemChange>) + Send + Sync> = Arc::new(on_change);
        let bin = Arc::new(BinFollow::new(state.clone(), virtuals.clone(), on_change.clone(), bin_dirs));
        bin.shown.store(bin_shown, Ordering::SeqCst);

        let watches: Vec<(FolderOrigin, DirWatch)> =
            [FolderOrigin::User, FolderOrigin::Public].into_iter().filter_map(|o| watch_folder(&state, &folders, &on_change, o).map(|w| (o, w))).collect();
        let reg_watch = watch_registry.then(|| {
            let (st, vs, cb, bin) = (state.clone(), virtuals.clone(), on_change.clone(), bin.clone());
            // Explorer writes under `Advanced` all the time: a burst is read once (each read asks the shell about the Recycle Bin), and
            // only the values of the three keys themselves are watched, not whatever sits below them
            let settled = super::Coalescer::start(Duration::from_millis(250), Duration::from_secs(2), move || {
                let changes = refresh(&st, vs.as_ref(), Some(&bin));
                if !changes.is_empty() {
                    cb(changes);
                }
            });
            super::regnotify::watch_keys_with(&[KEY_HIDE_ICONS, KEY_ADVANCED, KEY_POLICIES_EXPLORER], false, Arc::new(move |_| settled.poke()))
        });
        DesktopItems { state, folders, virtuals, on_change, dir_watches: Mutex::new(watches), _reg_watch: reg_watch, bin }
    }

    /// Follows the Recycle Bin's folders from now on, so that the bin's icon changes when something is sent to the bin or the bin is
    /// emptied, by this app or any other program: the state is read again (a burst once) and the change reported like any other.
    /// Costs one directory watch per fixed drive that has a bin of this user, only while the bin's icon is shown, and nothing while
    /// nothing happens. Safe to call again.
    pub fn watch_recycle_bin(&self) {
        self.bin.wanted.store(true, Ordering::SeqCst);
        self.bin.apply();
    }

    /// What `Store::resolve` needs: the items drawn, the ones that exist but are not drawn, and whether both folders have been listed
    /// (see `rf_core::desktop::layout::Listing`). Never hand `items()` alone to the layout.
    pub fn listing(&self) -> rf_core::desktop::layout::OwnedListing {
        self.state.lock().model.listing()
    }

    /// The visible items now (virtual first, then the user's Desktop, then the Public one).
    pub fn items(&self) -> Vec<DesktopItem> {
        self.state.lock().model.items()
    }

    /// The app's "show hidden" settings changed.
    pub fn set_visibility(&self, v: Visibility) {
        let changes = self.state.lock().model.set_visibility(v);
        if !changes.is_empty() {
            (self.on_change)(changes);
        }
    }

    /// Reads the virtual items' settings and the Recycle Bin again (also what the registry watch does).
    pub fn refresh_virtuals(&self) {
        let changes = refresh(&self.state, self.virtuals.as_ref(), Some(&self.bin));
        if !changes.is_empty() {
            (self.on_change)(changes);
        }
    }

    /// F5: lists both folders again, and starts the watch of a folder that could not be watched yet (its path was not known at start).
    pub fn rescan(&self) {
        {
            let mut watches = self.dir_watches.lock();
            for o in [FolderOrigin::User, FolderOrigin::Public] {
                if !watches.iter().any(|(w, _)| *w == o)
                    && let Some(w) = watch_folder(&self.state, &self.folders, &self.on_change, o)
                {
                    watches.push((o, w));
                }
            }
        }
        let mut all = Vec::new();
        for o in [FolderOrigin::User, FolderOrigin::Public] {
            if let Ok(files) = self.folders.list(o) {
                all.extend(self.state.lock().model.set_files(o, files));
            }
        }
        if !all.is_empty() {
            (self.on_change)(all);
        }
    }
}

fn refresh(state: &Mutex<State>, v: &dyn VirtualSource, follow: Option<&BinFollow>) -> Vec<ItemChange> {
    let vs = virtual_state(v); // (the shell is asked outside the lock)
    if let Some(f) = follow {
        f.sync(vs.shown.contains(&virtuals::VirtualKind::RecycleBin));
    }
    state.lock().model.set_virtuals(vs)
}

/// One burst of the folder watcher: re-stat the named entries (or list again when it says so) and apply them.
fn apply_burst(state: &Mutex<State>, folders: &dyn Folders, o: FolderOrigin, c: Changes) -> Vec<ItemChange> {
    if c.rescan {
        return match folders.list(o) {
            Ok(files) => state.lock().model.set_files(o, files),
            Err(e) => {
                log::warn!("desktop folder {o:?} not listed again: {e}");
                Vec::new()
            }
        };
    }
    let stats: Vec<(String, Option<DesktopFile>)> = c
        .names
        .into_iter()
        .map(|n| {
            let now = folders.stat(o, &n);
            (n, now)
        })
        .collect();
    state.lock().model.apply_stats(o, stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::items::ItemId;
    use rf_core::desktop::registry::MemRegistry;
    use rf_core::desktop::virtuals::VirtualKind;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-items-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    struct FakeVirtuals {
        reg: Mutex<MemRegistry>,
        full: AtomicBool,
    }
    impl VirtualSource for FakeVirtuals {
        fn settings(&self) -> VirtualSettings {
            virtuals::read(&*self.reg.lock())
        }
        fn recycle_full(&self) -> bool {
            self.full.load(Ordering::SeqCst)
        }
    }

    fn wait(mut cond: impl FnMut() -> bool) -> bool {
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        cond()
    }

    #[test]
    fn two_real_folders_merge_and_follow_adds_removes_and_renames() {
        let (user, public) = (temp("user"), temp("public"));
        std::fs::write(user.join("Notes.txt"), b"abc").unwrap();
        std::fs::create_dir_all(user.join("Projects")).unwrap();
        std::fs::write(user.join("shared.lnk"), b"1").unwrap();
        std::fs::write(public.join("Shared.LNK"), b"22").unwrap();
        std::fs::write(public.join("desktop.ini"), b"[x]").unwrap();
        // desktop.ini is hidden + system, like Windows makes it
        let w = crate::wide::wide(public.join("desktop.ini"));
        unsafe {
            use windows::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW};
            SetFileAttributesW(windows::core::PCWSTR(w.as_ptr()), FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM).unwrap();
        }
        let virt = Arc::new(FakeVirtuals {
            reg: Mutex::new(MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::ThisPc.clsid(), 0)),
            full: AtomicBool::new(true),
        });
        let log: Arc<Mutex<Vec<ItemChange>>> = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let di = DesktopItems::start(Arc::new(PathFolders { user: Some(user.clone()), public: Some(public.clone()) }), virt.clone(), Visibility::default(), false, move |c| l2.lock().extend(c));

        let ids = |di: &DesktopItems| di.items().into_iter().map(|i| i.id.to_string()).collect::<Vec<_>>();
        let first = ids(&di);
        assert_eq!(
            first,
            [VirtualKind::ThisPc.parsing_name(), VirtualKind::RecycleBin.parsing_name(), "u:projects".into(), "u:notes.txt".into(), "u:shared.lnk".into(), "p:shared.lnk".into()],
            "virtual first, the shared name twice, desktop.ini hidden"
        );
        assert_eq!(di.items()[1].recycle_full, Some(true));
        // counts by source (what the probe prints)
        let by = |f: fn(&DesktopItem) -> bool| di.items().iter().filter(|i| f(i)).count();
        assert_eq!((by(|i| i.is_virtual()), by(|i| matches!(i.origin, rf_core::desktop::items::Origin::User)), by(|i| matches!(i.origin, rf_core::desktop::items::Origin::Public))), (2, 3, 1));

        // show hidden + system: desktop.ini appears
        di.set_visibility(Visibility { show_hidden: true, show_system: true, dot_hidden: false });
        assert!(ids(&di).contains(&"p:desktop.ini".to_string()));
        di.set_visibility(Visibility::default());
        log.lock().clear();

        // an added file and a deleted one, seen by the watcher
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(user.join("new file.txt"), b"x").unwrap();
        std::fs::remove_file(user.join("Notes.txt")).unwrap();
        assert!(wait(|| ids(&di).contains(&"u:new file.txt".to_string()) && !ids(&di).contains(&"u:notes.txt".to_string())), "{:?}", ids(&di));
        assert!(log.lock().contains(&ItemChange::Added(ItemId::user("new file.txt"))) || log.lock().iter().any(|c| matches!(c, ItemChange::Renamed { .. })));

        // a rename keeps its identity pairing: same size and time, another name
        log.lock().clear();
        std::fs::rename(user.join("Projects"), user.join("Work")).unwrap();
        assert!(wait(|| ids(&di).contains(&"u:work".to_string()) && !ids(&di).contains(&"u:projects".to_string())), "{:?}", ids(&di));
        assert!(wait(|| log.lock().contains(&ItemChange::Renamed { from: ItemId::user("projects"), to: ItemId::user("work") })), "{:?}", log.lock());

        // the Public folder is watched too
        std::fs::write(public.join("Zoom.lnk"), b"z").unwrap();
        assert!(wait(|| ids(&di).contains(&"p:zoom.lnk".to_string())));

        // the Recycle Bin empties and This PC is turned off: refresh_virtuals reads both again
        virt.full.store(false, Ordering::SeqCst);
        *virt.reg.lock() = MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::ThisPc.clsid(), 1);
        di.refresh_virtuals();
        assert_eq!(di.items().iter().filter(|i| i.is_virtual()).count(), 1);
        assert_eq!(di.items()[0].recycle_full, Some(false));

        // F5
        std::fs::write(user.join("late.txt"), b"x").unwrap();
        di.rescan();
        assert!(ids(&di).contains(&"u:late.txt".to_string()));

        drop(di);
        let _ = std::fs::remove_dir_all(&user);
        let _ = std::fs::remove_dir_all(&public);
    }

    #[test]
    fn the_apps_own_choices_decide_what_is_shown_and_a_refresh_picks_a_change_up() {
        use rf_core::desktop::virtuals::{Choices, IconChoice};
        let windows = Arc::new(FakeVirtuals { reg: Mutex::new(MemRegistry::new()), full: AtomicBool::new(true) });
        let choices = Arc::new(Mutex::new(Choices::default()));
        let c2 = choices.clone();
        let chosen = Arc::new(ChosenVirtuals { inner: windows, choices: Arc::new(move || *c2.lock()) });
        let changes: Arc<Mutex<Vec<ItemChange>>> = Arc::new(Mutex::new(Vec::new()));
        let ch2 = changes.clone();
        let di = DesktopItems::start(Arc::new(PathFolders { user: None, public: None }), chosen, Visibility::default(), false, move |c| ch2.lock().extend(c));
        let ids = |di: &DesktopItems| di.items().into_iter().map(|i| i.id.to_string()).collect::<Vec<_>>();
        // every choice on "follow": a fresh Windows shows the Recycle Bin, and the bin knows it is full
        assert_eq!(ids(&di), [VirtualKind::RecycleBin.parsing_name()]);
        assert_eq!(di.items()[0].recycle_full, Some(true));
        // add This PC and the user's folder, take the Recycle Bin away
        {
            let mut c = choices.lock();
            c.set(VirtualKind::ThisPc, IconChoice::Show);
            c.set(VirtualKind::UserFiles, IconChoice::Show);
            c.set(VirtualKind::RecycleBin, IconChoice::Hide);
        }
        di.refresh_virtuals();
        assert_eq!(ids(&di), [VirtualKind::ThisPc.parsing_name(), VirtualKind::UserFiles.parsing_name()]);
        assert!(changes.lock().contains(&ItemChange::Added(ItemId::virtual_item(VirtualKind::ThisPc))));
        assert!(changes.lock().contains(&ItemChange::Removed(ItemId::virtual_item(VirtualKind::RecycleBin))));
        // "hide all icons" empties the desktop of the system icons, and bringing them back is one refresh
        choices.lock().show_icons = IconChoice::Hide;
        di.refresh_virtuals();
        assert!(di.items().is_empty());
        choices.lock().show_icons = IconChoice::Follow;
        di.refresh_virtuals();
        assert_eq!(di.items().len(), 2);
    }

    /// A fake bin: a folder of the test's own that stands for `X:\$Recycle.Bin\<sid>`, and a flag for "something is in it".
    fn bin_fixture(dirs: Arc<Mutex<Vec<PathBuf>>>, reg: MemRegistry) -> (DesktopItems, Arc<FakeVirtuals>, Arc<Mutex<Vec<ItemChange>>>) {
        let virt = Arc::new(FakeVirtuals { reg: Mutex::new(reg), full: AtomicBool::new(false) });
        let log: Arc<Mutex<Vec<ItemChange>>> = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let di = DesktopItems::start_with_bin_dirs(
            Arc::new(PathFolders { user: None, public: None }),
            virt.clone(),
            Visibility::default(),
            false,
            move |c| l2.lock().extend(c),
            Arc::new(move || dirs.lock().clone()),
        );
        (di, virt, log)
    }

    fn bin_full_now(di: &DesktopItems) -> Option<bool> {
        di.items().iter().find(|i| matches!(i.origin, rf_core::desktop::items::Origin::Virtual(VirtualKind::RecycleBin))).and_then(|i| i.recycle_full)
    }

    #[test]
    fn the_bin_picture_follows_a_change_in_the_bin_folder_and_a_folder_that_appears_later_is_picked_up() {
        let (first, later) = (temp("bin-a"), temp("bin-b"));
        let dirs = Arc::new(Mutex::new(vec![first.clone()]));
        let (di, virt, _log) = bin_fixture(dirs.clone(), MemRegistry::new());
        assert_eq!(bin_full_now(&di), Some(false));
        di.watch_recycle_bin();
        di.watch_recycle_bin(); // (again: still one watch per folder)
        assert_eq!(di.bin.slot.lock().watches.len(), 1);
        // something is sent to the bin: a pair of files appears in the folder, the shell now says "full"
        virt.full.store(true, Ordering::SeqCst);
        std::fs::write(first.join("$IABC.txt"), b"i").unwrap();
        assert!(wait(|| bin_full_now(&di) == Some(true)), "the watch told the model");
        // a bin folder that did not exist at start (the drive's first delete creates it) joins at the next sync, which every
        // refresh does
        dirs.lock().push(later.clone());
        di.refresh_virtuals();
        assert_eq!(di.bin.slot.lock().watches.len(), 2);
        virt.full.store(false, Ordering::SeqCst);
        std::fs::write(later.join("$IDEF.txt"), b"i").unwrap();
        assert!(wait(|| bin_full_now(&di) == Some(false)), "the late folder is watched too");
        drop(di);
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&later);
    }

    #[test]
    fn no_watch_runs_while_the_bin_icon_is_hidden_and_it_stops_and_starts_with_the_icon() {
        use rf_core::desktop::virtuals::{Choices, IconChoice};
        let dir = temp("bin-hidden");
        let dirs = Arc::new(Mutex::new(vec![dir.clone()]));
        // Windows hides the bin
        let windows = Arc::new(FakeVirtuals { reg: Mutex::new(MemRegistry::new().cu_dword(KEY_HIDE_ICONS, VirtualKind::RecycleBin.clsid(), 1)), full: AtomicBool::new(false) });
        let choices = Arc::new(Mutex::new(Choices::default()));
        let c2 = choices.clone();
        let chosen = Arc::new(ChosenVirtuals { inner: windows, choices: Arc::new(move || *c2.lock()) });
        let di = DesktopItems::start_with_bin_dirs(Arc::new(PathFolders { user: None, public: None }), chosen, Visibility::default(), false, |_| {}, Arc::new(move || dirs.lock().clone()));
        di.watch_recycle_bin();
        assert!(di.bin.slot.lock().watches.is_empty() && di.bin.slot.lock().job.is_none(), "hidden icon: no watcher, no job thread");
        // the user chooses to show it: the watch starts
        choices.lock().set(VirtualKind::RecycleBin, IconChoice::Show);
        di.refresh_virtuals();
        assert_eq!(di.bin.slot.lock().watches.len(), 1);
        // the job's thread holds the model while it lives; once the icon is hidden again it lets go (the thread ended)
        let held = Arc::strong_count(&di.state);
        choices.lock().set(VirtualKind::RecycleBin, IconChoice::Hide);
        di.refresh_virtuals();
        assert!(di.bin.slot.lock().watches.is_empty() && di.bin.slot.lock().job.is_none());
        assert!(wait(|| Arc::strong_count(&di.state) < held), "the job thread ended");
        drop(di);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropping_the_items_ends_the_bin_job_thread() {
        let dir = temp("bin-drop");
        let dirs = Arc::new(Mutex::new(vec![dir.clone()]));
        let (di, _virt, _log) = bin_fixture(dirs, MemRegistry::new());
        di.watch_recycle_bin();
        let state = di.state.clone();
        let before = Arc::strong_count(&state);
        drop(di);
        // (the registry-less items held some; the job thread's share goes when the watch is dropped)
        assert!(wait(|| Arc::strong_count(&state) == 1), "everything that held the model let go (was {before})");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shell_that_did_not_answer_leaves_the_bin_picture_as_it_was() {
        use std::sync::atomic::AtomicUsize;
        let script: Arc<Mutex<Vec<Option<bool>>>> = Arc::new(Mutex::new(vec![Some(true), None, None, Some(false), None]));
        let (s2, calls) = (script.clone(), Arc::new(AtomicUsize::new(0)));
        let c2 = calls.clone();
        let v = SystemVirtuals::with_asker(
            Arc::new(MemRegistry::new()),
            Arc::new(move || {
                c2.fetch_add(1, Ordering::SeqCst);
                s2.lock().remove(0)
            }),
        );
        let seen: Vec<bool> = (0..5).map(|_| v.recycle_full()).collect();
        assert_eq!(seen, [true, true, true, false, false], "a timeout keeps the last answer, in both directions");
        assert_eq!(calls.load(Ordering::SeqCst), 5);
        // before the shell ever answered, nothing is known: empty
        let fresh = SystemVirtuals::with_asker(Arc::new(MemRegistry::new()), Arc::new(|| None));
        assert!(!fresh.recycle_full());
    }

    /// A set of folders that can be made to fail the way a busy shell thread makes the real ones fail at start-up.
    struct Flaky {
        up: std::sync::atomic::AtomicBool,
        user: PathBuf,
    }
    impl Folders for Flaky {
        fn dir(&self, o: FolderOrigin) -> Option<PathBuf> {
            (o == FolderOrigin::User && self.up.load(Ordering::SeqCst)).then(|| self.user.clone())
        }
        fn list(&self, o: FolderOrigin) -> Result<Vec<DesktopFile>, String> {
            match self.dir(o) {
                Some(d) => list_path(&d),
                None if o == FolderOrigin::Public => Ok(Vec::new()), // this one is fine: an empty Public Desktop
                None => Err("the known folder was not answered".into()),
            }
        }
        fn stat(&self, o: FolderOrigin, name: &str) -> Option<DesktopFile> {
            stat_path(&self.dir(o)?, name)
        }
    }

    #[test]
    fn a_folder_that_was_not_found_is_not_an_empty_one_and_is_picked_up_later() {
        let user = temp("flaky");
        std::fs::write(user.join("keep.txt"), b"k").unwrap();
        let folders = Arc::new(Flaky { up: std::sync::atomic::AtomicBool::new(false), user: user.clone() });
        let virt = Arc::new(FakeVirtuals { reg: Mutex::new(MemRegistry::new()), full: AtomicBool::new(false) });
        let di = DesktopItems::start(folders.clone(), virt, Visibility::default(), false, |_| {});
        // the lookup failed: nothing is known, which the layout is told, so no position is forgotten for it
        let l = di.listing();
        assert!(!l.complete, "the Desktop was not listed");
        assert!(!l.shown.iter().any(|i| i.as_str().starts_with("u:")));
        // the system answers now: a rescan lists the folder AND starts its watch
        folders.up.store(true, Ordering::SeqCst);
        di.rescan();
        assert!(di.listing().complete);
        assert!(di.items().iter().any(|i| i.id == ItemId::user("keep.txt")));
        std::fs::write(user.join("later.txt"), b"x").unwrap();
        assert!(wait(|| di.items().iter().any(|i| i.id == ItemId::user("later.txt"))), "the late watch sees a new file");
        drop(di);
        let _ = std::fs::remove_dir_all(&user);
    }

    #[test]
    fn the_known_folder_lookup_is_repeated_until_it_has_answered() {
        // SystemFolders keeps the first answer it gets and asks again while it has none
        let sf = SystemFolders { user: Mutex::new(None), public: Mutex::new(None) };
        let p = sf.dir(FolderOrigin::User).expect("the Desktop folder, asked of the system on the first use");
        assert_eq!(sf.user.lock().as_deref(), Some(p.as_path()));
        // an unknown folder is an error to list, not an empty folder
        let none = PathFolders { user: None, public: None };
        assert!(none.list(FolderOrigin::User).is_err());
    }

    #[test]
    fn a_missing_or_unlistable_folder_is_an_empty_one_or_a_kept_one_never_a_crash() {
        let virt = Arc::new(FakeVirtuals { reg: Mutex::new(MemRegistry::new()), full: AtomicBool::new(false) });
        // no known folders at all
        let di = DesktopItems::start(Arc::new(PathFolders { user: None, public: None }), virt.clone(), Visibility::default(), false, |_| {});
        assert_eq!(di.items().len(), 1, "only the Recycle Bin");
        // a folder path that does not exist: the listing fails and the model stays as it was (empty here)
        let gone = std::env::temp_dir().join(format!("rf-items-nope-{}", std::process::id()));
        let di = DesktopItems::start(Arc::new(PathFolders { user: Some(gone), public: None }), virt, Visibility::default(), false, |_| {});
        assert_eq!(di.items().len(), 1);
    }

    #[test]
    fn the_known_folders_are_asked_of_the_system() {
        let (u, p) = (user_desktop_dir().expect("the Desktop folder"), public_desktop_dir().expect("the Public Desktop folder"));
        assert!(u.is_dir() && p.is_dir());
        assert_ne!(u, p);
        // and the real listing works on them without reading any file (count only)
        let f = SystemFolders::new();
        assert!(f.list(FolderOrigin::User).is_ok() && f.list(FolderOrigin::Public).is_ok());
        assert!(f.stat(FolderOrigin::User, "no such file anywhere.zzz").is_none());
    }
}
