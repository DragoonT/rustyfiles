//! The dock's OWN pin list (shell plan increment B8, row B2): a small JSON file in the config folder (`dock-pins.json`).
//!
//! Until the user pins or unpins something from the dock itself, the middle island follows the apps pinned to the Windows taskbar (read
//! only, as it always has). The first pin or unpin from the dock's own menu SEEDS this file once from what the taskbar has pinned at that
//! moment, applies the change and from then on the file is the list: Windows' pins are not read for the island any more, and Windows'
//! own stores (the taskbar's pin folder, `Taskband`) are never written. Not seeded = nothing changed for a user who never uses the
//! menu.
//!
//! An entry holds what the island needs to show and start the app without asking Windows again: the name, the identities a running
//! window is matched by, how to start it, and where its icon comes from. A file that exists and cannot be read is moved aside, never
//! written over (it may be the user's own list); a seeding whose taskbar read did not come back writes nothing.
//!
//! Pure apart from [`load_checked`], [`save`], [`set_aside`] and [`change`], which only take a path.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::model::{DockApp, IconSource, Launch, PinnedApp, looks_packaged};

/// The file's name in the config folder.
pub const FILE: &str = "dock-pins.json";

/// Most pins the first seeding takes from the taskbar.
pub const SEED_MAX: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum LaunchRec {
    Link { path: PathBuf },
    Exe { path: PathBuf, #[serde(default)] args: String },
    Packaged { id: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum IconRec {
    File { path: PathBuf },
    Resource { file: PathBuf, index: i32 },
    Packaged { id: String },
    Shell { name: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnPin {
    pub name: String,
    /// The AppUserModelID a running window is matched by, lower case.
    #[serde(default)]
    pub aumid: Option<String>,
    /// The program, lower case.
    #[serde(default)]
    pub exe: Option<String>,
    pub launch: LaunchRec,
    pub icon: IconRec,
}

impl OwnPin {
    pub fn from_pinned(p: &PinnedApp) -> OwnPin {
        OwnPin {
            name: p.name.clone(),
            aumid: p.aumid.clone(),
            exe: p.exe.clone(),
            launch: match &p.launch {
                Launch::Link(path) => LaunchRec::Link { path: path.clone() },
                Launch::Exe { path, args } => LaunchRec::Exe { path: path.clone(), args: args.clone() },
                Launch::Packaged(id) => LaunchRec::Packaged { id: id.clone() },
            },
            icon: match &p.icon {
                IconSource::File(path) => IconRec::File { path: path.clone() },
                IconSource::Resource { file, index } => IconRec::Resource { file: file.clone(), index: *index },
                IconSource::Packaged(id) => IconRec::Packaged { id: id.clone() },
                IconSource::Shell(name) => IconRec::Shell { name: name.clone() },
                // (a pin made from a running window never gets a window's own icon: it is gone with the window; see `pin_from_app`)
                IconSource::Window(_) => IconRec::Shell { name: String::new() },
            },
        }
    }

    pub fn to_pinned(&self) -> PinnedApp {
        PinnedApp {
            name: self.name.clone(),
            aumid: self.aumid.clone(),
            exe: self.exe.clone(),
            launch: match &self.launch {
                LaunchRec::Link { path } => Launch::Link(path.clone()),
                LaunchRec::Exe { path, args } => Launch::Exe { path: path.clone(), args: args.clone() },
                LaunchRec::Packaged { id } => Launch::Packaged(id.clone()),
            },
            icon: match &self.icon {
                IconRec::File { path } => IconSource::File(path.clone()),
                IconRec::Resource { file, index } => IconSource::Resource { file: file.clone(), index: *index },
                IconRec::Packaged { id } => IconSource::Packaged(id.clone()),
                IconRec::Shell { name } => IconSource::Shell(name.clone()),
            },
        }
    }

    /// The program this pin starts, lower case, when it names one.
    fn program(&self) -> Option<String> {
        self.exe.clone().or_else(|| match &self.launch {
            LaunchRec::Exe { path, .. } | LaunchRec::Link { path } => Some(path.to_string_lossy().to_lowercase()),
            LaunchRec::Packaged { .. } => None,
        })
    }
}

/// The pin for a dock app that is not pinned yet. `None` when the app cannot be started again (no known way to start it): a pin that
/// does nothing is no pin.
pub fn pin_from_app(app: &DockApp) -> Option<OwnPin> {
    let launch = app.launch.as_ref()?;
    let (launch_rec, exe, aumid) = match launch {
        Launch::Link(path) => (LaunchRec::Link { path: path.clone() }, None, None),
        Launch::Exe { path, args } => (LaunchRec::Exe { path: path.clone(), args: args.clone() }, Some(path.to_string_lossy().to_lowercase()), looks_packaged(&app.key).then(|| app.key.to_lowercase())),
        Launch::Packaged(id) => (LaunchRec::Packaged { id: id.clone() }, None, Some(id.to_lowercase())),
    };
    // the icon: the app's own; a running window's own icon cannot be kept, so the program's (or the package's) is used
    let icon = match &app.icon {
        IconSource::File(path) => IconRec::File { path: path.clone() },
        IconSource::Resource { file, index } => IconRec::Resource { file: file.clone(), index: *index },
        IconSource::Packaged(id) => IconRec::Packaged { id: id.clone() },
        IconSource::Shell(name) => IconRec::Shell { name: name.clone() },
        IconSource::Window(_) => match launch {
            Launch::Exe { path, .. } | Launch::Link(path) => IconRec::File { path: path.clone() },
            Launch::Packaged(id) => IconRec::Packaged { id: id.clone() },
        },
    };
    Some(OwnPin { name: app.name.clone(), aumid, exe, launch: launch_rec, icon })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinFile {
    pub version: u32,
    /// The taskbar's pins have been copied in (once): from here on the file is the list, an empty list is the user's choice.
    pub seeded: bool,
    pub pins: Vec<OwnPin>,
}

impl Default for PinFile {
    fn default() -> Self {
        PinFile { version: 1, seeded: false, pins: Vec::new() }
    }
}

impl PinFile {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// `None` for text that is not a pin file. An entry this version cannot read costs only itself.
    pub fn parse(text: &str) -> Option<PinFile> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        let obj = v.as_object()?;
        let pins = obj
            .get("pins")
            .and_then(|p| p.as_array())
            .map(|a| a.iter().filter_map(|e| serde_json::from_value::<OwnPin>(e.clone()).ok()).filter(|p| !p.name.trim().is_empty()).collect())
            .unwrap_or_default();
        Some(PinFile {
            version: obj.get("version").and_then(|x| x.as_u64()).unwrap_or(1) as u32,
            seeded: obj.get("seeded").and_then(|x| x.as_bool()).unwrap_or(false),
            pins,
        })
    }

    /// Copies the taskbar's pins in and marks the file seeded. Done once; an already seeded file is left alone.
    pub fn seed(&mut self, taskbar: &[PinnedApp]) {
        if self.seeded {
            return;
        }
        if self.pins.is_empty() {
            self.pins = taskbar.iter().take(SEED_MAX).map(OwnPin::from_pinned).collect();
        }
        self.seeded = true;
    }

    /// The index of the pin that stands for this dock app, if any. A running pinned app's key is its window's identity, which may differ
    /// from the pin's, so the match goes from the surest to the weakest: the pin's AppUserModelID is the app's key; the pin's program is
    /// the key; the pin starts the very program the app does; the same name (only for an app the dock shows as pinned).
    pub fn find(&self, app: &DockApp) -> Option<usize> {
        let key = app.key.to_lowercase();
        let same = |f: &dyn Fn(&OwnPin) -> bool| self.pins.iter().position(f);
        same(&|p| p.aumid.as_deref() == Some(key.as_str()))
            .or_else(|| same(&|p| p.exe.as_deref() == Some(key.as_str())))
            .or_else(|| {
                let theirs = match &app.launch {
                    Some(Launch::Exe { path, .. }) | Some(Launch::Link(path)) => Some(path.to_string_lossy().to_lowercase()),
                    _ => None,
                }?;
                same(&|p| p.program().as_deref() == Some(theirs.as_str()))
            })
            .or_else(|| {
                // The name is the weakest evidence: a different program with the same name (a second build of the same app, a copy)
                // must not count as pinned. It is used only for an app the dock itself already shows as pinned (the dock matched it
                // to a pin by rules this file cannot see, such as "any of its windows' programs"), to find which pin that was.
                let name = app.name.trim().to_lowercase();
                (app.pinned && !name.is_empty()).then(|| same(&|p| p.name.trim().to_lowercase() == name)).flatten()
            })
    }

    pub fn is_pinned(&self, app: &DockApp) -> bool {
        self.find(app).is_some()
    }

    /// Pins an app at the end of the list. False when it already is, or cannot be.
    pub fn pin(&mut self, app: &DockApp) -> bool {
        if self.is_pinned(app) {
            return false;
        }
        match pin_from_app(app) {
            Some(p) => {
                self.pins.push(p);
                true
            }
            None => false,
        }
    }

    /// Takes an app off the list. False when it was not on it.
    pub fn unpin(&mut self, app: &DockApp) -> bool {
        match self.find(app) {
            Some(i) => {
                self.pins.remove(i);
                true
            }
            None => false,
        }
    }

    /// The pins as the island shows them: those whose program or shortcut still exists (`exists`), in the file's order. A pin whose
    /// program is gone stays in the file (it may come back) and out of the island.
    pub fn alive(&self, exists: impl Fn(&Path) -> bool) -> Vec<PinnedApp> {
        self.pins
            .iter()
            .filter(|p| match &p.launch {
                LaunchRec::Link { path } | LaunchRec::Exe { path, .. } => exists(path),
                LaunchRec::Packaged { .. } => true,
            })
            .map(OwnPin::to_pinned)
            .collect()
    }
}

/// Makes seeded pins independent of Windows' pin folder (done by [`change`] only once the change is known to be written). A taskbar pin is a shortcut (`.lnk`) in a folder Windows owns, and the shortcut
/// holds what the pin needs (the program, its arguments, the AppUserModelID); if the dock's own pin pointed at that file, unpinning in
/// Windows would silently take the dock's pin away too. So at the seeding each shortcut is COPIED (`copy` is given the file and returns
/// the copy's path, or `None` when it cannot copy) and the pin starts the copy. A pin whose copy failed keeps pointing at the original (it
/// still works, it just stays tied to Windows' folder). Other kinds of pin are returned as they are.
pub fn localize_links(pins: Vec<PinnedApp>, mut copy: impl FnMut(&Path) -> Option<PathBuf>) -> Vec<PinnedApp> {
    pins.into_iter()
        .map(|mut p| {
            if let Launch::Link(original) = &p.launch
                && let Some(own) = copy(original)
            {
                p.launch = Launch::Link(own);
            }
            p
        })
        .collect()
}

/// What the pin file turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// There is none: the island follows the taskbar.
    Absent,
    /// There is one and it could not be read (locked for a moment, half written, damaged): it may hold the user's own list, so it is never
    /// written over without being set aside first, and the island follows the taskbar meanwhile.
    Unreadable,
    Found(PinFile),
}

pub fn load_checked(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => PinFile::parse(&text).map_or(Loaded::Unreadable, Loaded::Found),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Absent,
        Err(_) => Loaded::Unreadable,
    }
}

/// The user's own list, if there is one that is in force: a seeded file, as the island shows it. `None`: follow the taskbar.
pub fn effective(path: &Path, exists: impl Fn(&Path) -> bool) -> Option<Vec<PinnedApp>> {
    match load_checked(path) {
        Loaded::Found(f) if f.seeded => Some(f.alive(exists)),
        _ => None,
    }
}

/// Moves an unreadable pin file out of the way (`dock-pins.json.broken`, then `.broken2` ... never over an earlier one).
pub fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    for n in 1..100 {
        let name = if n == 1 { "broken".to_string() } else { format!("broken{n}") };
        let to = path.with_extension(format!("json.{name}"));
        if !to.exists() {
            std::fs::rename(path, &to)?;
            return Ok(to);
        }
    }
    Err(std::io::Error::other("too many set-aside dock pin files"))
}

/// Writes the file whole to a temporary file next to it and renames it over the old one, so a crash in the middle leaves the old list.
pub fn save(path: &Path, file: &PinFile) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(file.to_json().as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Why a change was not made.
#[derive(Debug, PartialEq, Eq)]
pub enum ChangeError {
    /// The list is not seeded yet and the taskbar's pins could not be read in time: nothing is written, the user can try again.
    TaskbarUnavailable,
    /// The file could not be set aside or written.
    Disk(String),
}

/// What a change did.
#[derive(Debug, PartialEq, Eq)]
pub enum Changed {
    /// The list changed and was written.
    Written,
    /// There was nothing to change (already pinned, or not on the list).
    Same,
}

/// Changes are made one at a time: each reads the file again under this lock, so two quick clicks cannot write over each other.
static LOCK: Mutex<()> = Mutex::new(());

/// Whether an EMPTY answer of the taskbar read can be believed as "the user has no pins". `folder_listed`: the pin folder could be listed
/// (or does not exist); `unreadable_links`: shortcuts in it that could not be read at all (the shell did not answer, a lock). An empty
/// answer that comes with a folder that could not be listed, or with shortcuts that could not be read, is a failed read passed off as
/// "no pins": seeding the dock's list from it would make an empty list the user's list for good.
pub fn empty_read_is_believable(folder_listed: bool, unreadable_links: usize) -> bool {
    folder_listed && unreadable_links == 0
}

/// Applies `edit` to the list (seeding it first from `taskbar`, which is asked for only when the list is not seeded yet) and writes the
/// result. When the list was seeded by this very change, the shortcuts of the seeded pins are COPIED by `copy_links` (see
/// [`localize_links`]) after `edit` has said it changed something: a change that changes nothing copies and writes nothing. Blocks (disk,
/// and whatever `taskbar` and `copy_links` do): a worker thread.
pub fn change(
    path: &Path,
    taskbar: impl FnOnce() -> Option<Vec<PinnedApp>>,
    copy_links: impl FnOnce(Vec<PinnedApp>) -> Vec<PinnedApp>,
    edit: impl FnOnce(&mut PinFile) -> bool,
) -> Result<Changed, ChangeError> {
    let _guard = LOCK.lock().unwrap_or_else(|p| p.into_inner());
    let loaded = load_checked(path);
    // the pins this change takes from the taskbar (empty when the list was seeded before)
    let mut seeded_now: Vec<OwnPin> = Vec::new();
    let mut file = match &loaded {
        Loaded::Found(f) if f.seeded => f.clone(),
        other => {
            // not seeded: the taskbar's pins first (if they cannot be read, nothing is touched at all)
            let taskbar = taskbar().ok_or(ChangeError::TaskbarUnavailable)?;
            let mut f = match other {
                Loaded::Found(f) => f.clone(),
                _ => PinFile::default(),
            };
            let was_empty = f.pins.is_empty();
            f.seed(&taskbar);
            if was_empty {
                seeded_now = f.pins.clone();
            }
            f
        }
    };
    if !edit(&mut file) {
        // nothing to change: nothing is written, not even the seeding, and no shortcut is copied
        return Ok(Changed::Same);
    }
    // the seeded shortcuts become the dock's own copies (a pin the edit removed is not copied; one it added is left as it is)
    let at: Vec<usize> = file.pins.iter().enumerate().filter(|(_, p)| seeded_now.contains(p)).map(|(i, _)| i).collect();
    if !at.is_empty() {
        let local = copy_links(at.iter().map(|&i| file.pins[i].to_pinned()).collect());
        for (&i, l) in at.iter().zip(local) {
            file.pins[i] = OwnPin::from_pinned(&l);
        }
    }
    if loaded == Loaded::Unreadable {
        set_aside(path).map_err(|e| ChangeError::Disk(e.to_string()))?;
    }
    save(path, &file).map_err(|e| ChangeError::Disk(e.to_string()))?;
    Ok(Changed::Written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinned(name: &str, aumid: Option<&str>, exe: Option<&str>) -> PinnedApp {
        PinnedApp {
            name: name.into(),
            aumid: aumid.map(str::to_lowercase),
            exe: exe.map(str::to_lowercase),
            launch: Launch::Link(PathBuf::from(format!(r"C:\pins\{name}.lnk"))),
            icon: IconSource::File(PathBuf::from(exe.unwrap_or("x.exe"))),
        }
    }

    fn dock_app(name: &str, key: &str, launch: Option<Launch>, pinned: bool) -> DockApp {
        DockApp { key: key.into(), name: name.into(), pinned, launch, icon: IconSource::Window(7), windows: vec![7], active: false }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-dockpins-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_pin_survives_the_round_trip_through_json() {
        let p = pinned("Paint", Some("Microsoft.Paint_8wekyb3d8bbwe!App"), None);
        let mut p2 = pinned("Chrome", None, Some(r"C:\a\chrome.exe"));
        p2.launch = Launch::Exe { path: PathBuf::from(r"C:\a\chrome.exe"), args: "--x".into() };
        p2.icon = IconSource::Resource { file: PathBuf::from(r"C:\a\chrome.exe"), index: 3 };
        let mut f = PinFile::default();
        f.seed(&[p.clone(), p2.clone()]);
        let back = PinFile::parse(&f.to_json()).unwrap();
        assert_eq!(back, f);
        assert_eq!(back.pins[0].to_pinned(), p);
        assert_eq!(back.pins[1].to_pinned(), p2);
    }

    #[test]
    fn seeded_shortcuts_are_copies_so_that_unpinning_in_windows_does_not_take_the_docks_pin_away() {
        let mut exe_pin = pinned("Moved", None, Some("m.exe"));
        exe_pin.launch = Launch::Exe { path: PathBuf::from("m.exe"), args: "--a".into() };
        let mut pkg = pinned("Store", Some("a_b!c"), None);
        pkg.launch = Launch::Packaged("a_b!c".into());
        let taskbar = vec![pinned("A", None, Some("a.exe")), exe_pin.clone(), pkg.clone(), pinned("NoCopy", None, Some("n.exe"))];
        let mut asked: Vec<PathBuf> = Vec::new();
        let local = localize_links(taskbar, |f| {
            asked.push(f.to_path_buf());
            (!f.to_string_lossy().contains("NoCopy")).then(|| PathBuf::from(r"D:\cfg\dock-pins").join(f.file_name().unwrap()))
        });
        assert_eq!(asked.len(), 2, "only shortcuts are copied");
        assert_eq!(local[0].launch, Launch::Link(PathBuf::from(r"D:\cfg\dock-pins\A.lnk")));
        assert_eq!(local[1], exe_pin, "a program pin is left alone");
        assert_eq!(local[2], pkg, "a package pin is left alone");
        assert_eq!(local[3].launch, Launch::Link(PathBuf::from(r"C:\pins\NoCopy.lnk")), "a failed copy keeps the original");
        // and the copies are what the island checks for existence
        let mut f = PinFile::default();
        f.seed(&local);
        let shown = f.alive(|p| p.starts_with(r"D:\cfg\dock-pins"));
        assert_eq!(shown.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["A", "Store"], "the copy exists; the original and the program pin do not; a package pin always does");
    }

    #[test]
    fn a_packaged_pin_and_a_shell_place_icon_keep_their_kind() {
        let mut p = pinned("Paint", Some("a_b!c"), None);
        p.launch = Launch::Packaged("A_b!c".into());
        p.icon = IconSource::Packaged("A_b!c".into());
        assert_eq!(OwnPin::from_pinned(&p).to_pinned(), p);
        p.icon = IconSource::Shell("shell:RecycleBinFolder".into());
        assert_eq!(OwnPin::from_pinned(&p).to_pinned(), p);
    }

    #[test]
    fn an_entry_that_cannot_be_read_costs_only_itself_and_a_wrong_file_is_no_file() {
        let text = r#"{"version":1,"seeded":true,"pins":[{"name":"Ok","launch":{"kind":"link","path":"a.lnk"},"icon":{"kind":"file","path":"a.exe"}},{"name":"Bad","launch":{"kind":"nonsense"},"icon":{"kind":"file","path":"x"}},{"name":"  ","launch":{"kind":"link","path":"b.lnk"},"icon":{"kind":"file","path":"b"}}]}"#;
        let f = PinFile::parse(text).unwrap();
        assert!(f.seeded);
        assert_eq!(f.pins.len(), 1);
        assert_eq!(f.pins[0].name, "Ok");
        assert!(PinFile::parse("{ not json").is_none());
        assert!(PinFile::parse("[1, 2]").is_none());
    }

    #[test]
    fn seeding_happens_once_and_an_empty_list_after_it_is_the_users_choice() {
        let mut f = PinFile::default();
        f.seed(&[pinned("A", None, Some("a.exe")), pinned("B", None, Some("b.exe"))]);
        assert!(f.seeded);
        assert_eq!(f.pins.len(), 2);
        f.pins.clear();
        f.seed(&[pinned("C", None, Some("c.exe"))]);
        assert!(f.pins.is_empty(), "the user took them all off; the taskbar does not refill the list");
    }

    #[test]
    fn the_seed_is_capped() {
        let many: Vec<PinnedApp> = (0..SEED_MAX + 10).map(|i| pinned(&format!("A{i}"), None, Some(&format!("a{i}.exe")))).collect();
        let mut f = PinFile::default();
        f.seed(&many);
        assert_eq!(f.pins.len(), SEED_MAX);
    }

    #[test]
    fn a_running_app_is_found_by_the_surest_identity_first() {
        let mut f = PinFile::default();
        f.seed(&[
            pinned("Chrome", None, Some(r"C:\a\chrome.exe")),
            pinned("Code", Some("Microsoft.VisualStudioCode"), Some(r"C:\b\code.exe")),
            pinned("Notepad", None, Some(r"C:\n\notepad.exe")),
        ]);
        // by AppUserModelID (the group key of a running window)
        assert_eq!(f.find(&dock_app("x", "microsoft.visualstudiocode", None, true)), Some(1));
        // by program (the group key is the program path)
        assert_eq!(f.find(&dock_app("x", r"c:\a\chrome.exe", None, true)), Some(0));
        // by the program the app starts (a window that names itself at run time)
        let a = dock_app("x", "some.other.id", Some(Launch::Exe { path: PathBuf::from(r"C:\N\Notepad.exe"), args: String::new() }), true);
        assert_eq!(f.find(&a), Some(2));
        // by name, last, and only for an app the dock shows as pinned
        assert_eq!(f.find(&dock_app("chrome", "unknown", None, true)), Some(0));
        assert_eq!(f.find(&dock_app("chrome", "unknown", None, false)), None, "another program with the same name is not the pinned one");
        assert_eq!(f.find(&dock_app("Unknown", "unknown", None, false)), None);
        assert_eq!(f.find(&dock_app("", "", None, false)), None, "an empty name matches nothing");
    }

    #[test]
    fn pinning_a_running_app_makes_a_pin_that_starts_it_and_a_second_pin_does_nothing() {
        let mut f = PinFile { seeded: true, ..PinFile::default() };
        let app = dock_app("Notepad", r"c:\n\notepad.exe", Some(Launch::Exe { path: PathBuf::from(r"C:\N\notepad.exe"), args: String::new() }), false);
        assert!(f.pin(&app));
        assert!(!f.pin(&app), "already on the list");
        let p = f.pins[0].to_pinned();
        assert_eq!(p.name, "Notepad");
        assert_eq!(p.exe.as_deref(), Some(r"c:\n\notepad.exe"));
        assert_eq!(p.launch, Launch::Exe { path: PathBuf::from(r"C:\N\notepad.exe"), args: String::new() });
        // a window's own icon is not kept: the program's is
        assert_eq!(p.icon, IconSource::File(PathBuf::from(r"C:\N\notepad.exe")));
        // and it is found again as a running app with the same key
        assert!(f.is_pinned(&app));
    }

    #[test]
    fn a_packaged_app_is_pinned_by_its_package_id() {
        let mut f = PinFile { seeded: true, ..PinFile::default() };
        let id = "Microsoft.Paint_8wekyb3d8bbwe!App";
        let mut app = dock_app("Paint", &id.to_lowercase(), Some(Launch::Packaged(id.into())), false);
        app.icon = IconSource::Packaged(id.into());
        assert!(f.pin(&app));
        let p = &f.pins[0];
        assert_eq!(p.aumid.as_deref(), Some("microsoft.paint_8wekyb3d8bbwe!app"));
        assert_eq!(p.launch, LaunchRec::Packaged { id: id.into() });
        assert!(f.unpin(&app));
        assert!(f.pins.is_empty());
    }

    #[test]
    fn an_app_that_cannot_be_started_is_not_pinned() {
        let mut f = PinFile { seeded: true, ..PinFile::default() };
        assert!(!f.pin(&dock_app("Mystery", "pid:5", None, false)));
        assert!(f.pins.is_empty());
        assert!(pin_from_app(&dock_app("Mystery", "pid:5", None, false)).is_none());
    }

    #[test]
    fn unpinning_takes_the_one_pin_and_says_when_there_was_none() {
        let mut f = PinFile::default();
        f.seed(&[pinned("A", None, Some("a.exe")), pinned("B", None, Some("b.exe"))]);
        assert!(f.unpin(&dock_app("B", "b.exe", None, true)));
        assert_eq!(f.pins.len(), 1);
        assert!(!f.unpin(&dock_app("B", "b.exe", None, true)));
    }

    #[test]
    fn pins_whose_program_is_gone_stay_in_the_file_and_out_of_the_island() {
        let mut f = PinFile::default();
        f.seed(&[pinned("Here", None, Some("here.exe")), pinned("Gone", None, Some("gone.exe"))]);
        f.pins.push(OwnPin { name: "Store".into(), aumid: Some("a_b!c".into()), exe: None, launch: LaunchRec::Packaged { id: "a_b!c".into() }, icon: IconRec::Packaged { id: "a_b!c".into() } });
        let shown = f.alive(|p| p.to_string_lossy().contains("Here"));
        assert_eq!(shown.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Here", "Store"]);
        assert_eq!(f.pins.len(), 3);
    }

    #[test]
    fn the_island_follows_the_taskbar_until_the_file_is_seeded() {
        let dir = tmp("effective");
        let path = dir.join(FILE);
        assert_eq!(effective(&path, |_| true), None, "no file");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{ broken").unwrap();
        assert_eq!(effective(&path, |_| true), None, "an unreadable file is not the list");
        let mut f = PinFile::default();
        f.pins.push(OwnPin::from_pinned(&pinned("A", None, Some("a.exe"))));
        save(&path, &f).unwrap();
        assert_eq!(effective(&path, |_| true), None, "a file that was never seeded is not in force");
        f.seeded = true;
        save(&path, &f).unwrap();
        assert_eq!(effective(&path, |_| true).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_first_change_seeds_from_the_taskbar_and_applies_the_edit() {
        let dir = tmp("first");
        let path = dir.join(FILE);
        let taskbar = vec![pinned("A", None, Some("a.exe")), pinned("B", None, Some("b.exe"))];
        let app = dock_app("Notepad", "n.exe", Some(Launch::Exe { path: PathBuf::from("n.exe"), args: String::new() }), false);
        let out = change(&path, || Some(taskbar.clone()), |p| p, |f| f.pin(&app));
        assert_eq!(out, Ok(Changed::Written));
        let f = match load_checked(&path) {
            Loaded::Found(f) => f,
            other => panic!("{other:?}"),
        };
        assert!(f.seeded);
        assert_eq!(f.pins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["A", "B", "Notepad"]);
        // the second change does not ask the taskbar again
        let out = change(&path, || panic!("seeded already"), |p| p, |f| f.unpin(&dock_app("A", "a.exe", None, true)));
        assert_eq!(out, Ok(Changed::Written));
        assert_eq!(effective(&path, |_| true).unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_whose_taskbar_read_failed_writes_nothing() {
        let dir = tmp("failed");
        let path = dir.join(FILE);
        let out = change(&path, || None, |p| p, |f| f.pin(&dock_app("X", "x.exe", Some(Launch::Exe { path: PathBuf::from("x.exe"), args: String::new() }), false)));
        assert_eq!(out, Err(ChangeError::TaskbarUnavailable));
        assert!(!path.exists() && !dir.exists(), "not even the folder");
    }

    #[test]
    fn an_unreadable_file_is_set_aside_not_written_over() {
        let dir = tmp("broken");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, "{ the user's own words, damaged").unwrap();
        let out = change(&path, || Some(vec![pinned("A", None, Some("a.exe"))]), |p| p, |f| f.unpin(&dock_app("A", "a.exe", None, true)));
        assert_eq!(out, Ok(Changed::Written));
        let kept = std::fs::read_to_string(dir.join(format!("{FILE}.broken"))).unwrap();
        assert!(kept.contains("damaged"), "the old file is still there, under another name");
        assert!(effective(&path, |_| true).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpinning_what_is_not_pinned_changes_nothing_on_disk() {
        let dir = tmp("same");
        let path = dir.join(FILE);
        change(&path, || Some(vec![pinned("A", None, Some("a.exe"))]), |p| p, |_| true).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let out = change(&path, || None, |p| p, |f| f.unpin(&dock_app("Nope", "nope.exe", None, false)));
        assert_eq!(out, Ok(Changed::Same));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_that_changes_nothing_copies_no_shortcut() {
        // the first change seeds from the taskbar; its edit finds nothing to do (an unpin of an app that is on no pin): the seeding is
        // not written and the shortcuts are not copied (a copy would be left in the config folder for nothing)
        let dir = tmp("nocopy");
        let path = dir.join(FILE);
        let copies = std::cell::Cell::new(0usize);
        let out = change(
            &path,
            || Some(vec![pinned("A", None, Some("a.exe")), pinned("B", None, Some("b.exe"))]),
            |p| {
                copies.set(copies.get() + p.len());
                p
            },
            |f| f.unpin(&dock_app("Nope", "nope.exe", None, false)),
        );
        assert_eq!(out, Ok(Changed::Same));
        assert_eq!(copies.get(), 0, "no shortcut was copied");
        assert!(!path.exists() && !dir.exists());
    }

    #[test]
    fn the_seeded_shortcuts_are_copied_once_the_change_is_made_and_a_removed_pin_is_not() {
        let dir = tmp("copyafter");
        let path = dir.join(FILE);
        let asked: std::cell::RefCell<Vec<String>> = Default::default();
        let out = change(
            &path,
            || Some(vec![pinned("A", None, Some("a.exe")), pinned("B", None, Some("b.exe"))]),
            |p| {
                asked.borrow_mut().extend(p.iter().map(|x| x.name.clone()));
                localize_links(p, |f| Some(PathBuf::from(r"D:\cfg\dock-pins").join(f.file_name().unwrap())))
            },
            |f| f.unpin(&dock_app("A", "a.exe", None, true)),
        );
        assert_eq!(out, Ok(Changed::Written));
        assert_eq!(*asked.borrow(), vec!["B".to_string()], "only the pin that stays is copied");
        let shown = effective(&path, |_| true).unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].launch, Launch::Link(PathBuf::from(r"D:\cfg\dock-pins\B.lnk")));
        // a later change does not copy again
        let again = change(&path, || panic!("seeded already"), |_| panic!("not seeded by this change"), |f| f.pin(&dock_app("C", "c.exe", Some(Launch::Exe { path: PathBuf::from("c.exe"), args: String::new() }), false)));
        assert_eq!(again, Ok(Changed::Written));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_empty_taskbar_read_is_believed_only_when_the_folder_was_listed_and_no_shortcut_was_unreadable() {
        assert!(empty_read_is_believable(true, 0), "a folder with no pins in it, or none at all");
        assert!(!empty_read_is_believable(false, 0), "the folder could not be listed: not 'no pins'");
        assert!(!empty_read_is_believable(true, 2), "shortcuts were there and could not be read");
    }

    #[test]
    fn writes_leave_no_temporary_file_behind() {
        let dir = tmp("tmp");
        let path = dir.join(FILE);
        save(&path, &PinFile::default()).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE.to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
