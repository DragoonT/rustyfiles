//! The Start panel's own pinned apps: a small JSON file in the config folder (`start-pins.json`), seeded once from the apps pinned to
//! the Windows taskbar and after that the user's own list (right click an app in the panel: pin / unpin). Windows' own stores are
//! never touched. Pure apart from [`load`] and [`save`], which only take a path.
//!
//! An entry is the app's id in `shell:AppsFolder` (exactly what [`crate::apps::App::id`] holds), so a pin survives a rename or a
//! re-install; the name is only a cache for showing something when the app is gone. An entry whose app is not installed is kept in
//! the file (a package can come back) and not shown.

use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use super::model::PinnedApp;
use crate::apps::App;

/// The file's name in the config folder.
pub const FILE: &str = "start-pins.json";

/// How many apps the first seeding takes from the taskbar.
pub const SEED_MAX: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    pub id: String,
    /// The app's name when it was pinned.
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PinFile {
    pub version: u32,
    /// The taskbar's pins have been copied in (once: an empty list after that is the user's choice).
    pub seeded: bool,
    pub pins: Vec<Pin>,
}

impl Default for PinFile {
    fn default() -> Self {
        PinFile { version: 1, seeded: false, pins: Vec::new() }
    }
}

/// Two ids name the same app (the AppUserModelIDs of packaged apps differ only in case between sources).
pub fn same_id(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
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
            .map(|a| a.iter().filter_map(|e| serde_json::from_value::<Pin>(e.clone()).ok()).filter(|p| !p.id.trim().is_empty()).collect())
            .unwrap_or_default();
        Some(PinFile {
            version: obj.get("version").and_then(|x| x.as_u64()).unwrap_or(1) as u32,
            seeded: obj.get("seeded").and_then(|x| x.as_bool()).unwrap_or(false),
            pins: dedupe(pins),
        })
    }

    pub fn is_pinned(&self, id: &str) -> bool {
        self.pins.iter().any(|p| same_id(&p.id, id))
    }

    /// Pins an app at the end of the list. False when it already is.
    pub fn pin(&mut self, app: &App) -> bool {
        if self.is_pinned(&app.id) {
            return false;
        }
        self.pins.push(Pin { id: app.id.clone(), name: app.name.clone() });
        true
    }

    /// Removes a pin. False when there was none.
    pub fn unpin(&mut self, id: &str) -> bool {
        let before = self.pins.len();
        self.pins.retain(|p| !same_id(&p.id, id));
        self.pins.len() != before
    }

    /// The pins whose app is installed, in the file's order: `(index in `apps`, pin)`.
    pub fn resolve<'a>(&'a self, apps: &[App]) -> Vec<(usize, &'a Pin)> {
        self.pins.iter().filter_map(|p| apps.iter().position(|a| same_id(&a.id, &p.id)).map(|i| (i, p))).collect()
    }
}

fn dedupe(pins: Vec<Pin>) -> Vec<Pin> {
    let mut out: Vec<Pin> = Vec::with_capacity(pins.len());
    for p in pins {
        if !out.iter().any(|o| same_id(&o.id, &p.id)) {
            out.push(p);
        }
    }
    out
}

/// The apps among `apps` that the Windows taskbar has pinned, in the taskbar's order. A pin is matched to an app by its
/// AppUserModelID, else by its program (the last two parts of the program's path end the app's id), else by its name.
pub fn seed(taskbar: &[PinnedApp], apps: &[App]) -> Vec<Pin> {
    let mut out: Vec<Pin> = Vec::new();
    for t in taskbar {
        let found = find_app(t, apps);
        if let Some(a) = found
            && !out.iter().any(|o| same_id(&o.id, &a.id))
        {
            out.push(Pin { id: a.id.clone(), name: a.name.clone() });
        }
        if out.len() >= SEED_MAX {
            break;
        }
    }
    out
}

fn find_app<'a>(t: &PinnedApp, apps: &'a [App]) -> Option<&'a App> {
    if let Some(aumid) = t.aumid.as_deref().filter(|a| !a.is_empty())
        && let Some(a) = apps.iter().find(|a| same_id(&a.id, aumid))
    {
        return Some(a);
    }
    if let Some(exe) = t.exe.as_deref() {
        let tail = last_two_parts(exe);
        if !tail.is_empty()
            && let Some(a) = apps.iter().find(|a| a.id.to_lowercase().replace('/', "\\").ends_with(&tail))
        {
            return Some(a);
        }
    }
    let name = fold(&t.name);
    if name.is_empty() {
        return None;
    }
    apps.iter().find(|a| fold(&a.name) == name)
}

/// `\folder\program.exe` of a path, lower case ("" for a bare file name: too weak to match by).
fn last_two_parts(path: &str) -> String {
    let p = path.to_lowercase().replace('/', "\\");
    let parts: Vec<&str> = p.split('\\').filter(|s| !s.is_empty()).collect();
    if parts.len() < 2 {
        return String::new();
    }
    format!("\\{}\\{}", parts[parts.len() - 2], parts[parts.len() - 1])
}

fn fold(s: &str) -> String {
    s.trim().chars().flat_map(char::to_lowercase).filter(|c| *c != '\u{307}').collect()
}

/// Reads the file. `None`: there is none, or it is not readable as one (see [`load_checked`] to tell the two apart).
pub fn load(path: &Path) -> Option<PinFile> {
    match load_checked(path) {
        Loaded::Found(f) => Some(f),
        _ => None,
    }
}

/// What the pin file turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// There is no file: a first run.
    Absent,
    /// There is one and it could not be read (locked for a moment, half written, damaged): it may hold the user's own pins, so it is
    /// never written over without being set aside first.
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

/// What to do with a loaded file at the start of the Start panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Plan {
    /// The user's own list: use it as it is.
    Use(PinFile),
    /// Not seeded yet: copy the taskbar's pins in, starting from this file. `set_aside`: the file on disk could not be read and must
    /// be moved out of the way first (and the seeding does not happen if that fails).
    Seed { base: PinFile, set_aside: bool },
    /// Cannot decide now (the app list is not known yet): ask again at the next opening.
    Wait,
}

pub fn plan(loaded: Loaded, apps_known: bool) -> Plan {
    match loaded {
        Loaded::Found(f) if f.seeded => Plan::Use(f),
        _ if !apps_known => Plan::Wait,
        Loaded::Found(base) => Plan::Seed { base, set_aside: false },
        Loaded::Absent => Plan::Seed { base: PinFile::default(), set_aside: false },
        Loaded::Unreadable => Plan::Seed { base: PinFile::default(), set_aside: true },
    }
}

/// The file after seeding. `taskbar` is `None` when the taskbar's pins could not be read (the shell thread did not answer): then
/// there is no seeding and the file is NOT marked as seeded, so the next opening tries again; an empty list that came from a failed
/// read must not become the user's choice for good.
pub fn seeded_file(mut base: PinFile, taskbar: Option<&[PinnedApp]>, apps: &[App]) -> Option<PinFile> {
    let taskbar = taskbar?;
    if base.pins.is_empty() {
        base.pins = seed(taskbar, apps);
    }
    base.seeded = true;
    Some(base)
}

/// Moves an unreadable pin file out of the way (`start-pins.json.broken`, then `.broken2` ... never over an earlier one).
pub fn set_aside(path: &Path) -> std::io::Result<std::path::PathBuf> {
    for n in 1..100 {
        let name = if n == 1 { "broken".to_string() } else { format!("broken{n}") };
        let to = path.with_extension(format!("json.{name}"));
        if !to.exists() {
            std::fs::rename(path, &to)?;
            return Ok(to);
        }
    }
    Err(std::io::Error::other("too many set-aside pin files"))
}

/// Writes snapshots of the pin list in the order they were taken, whatever order the writing threads run in: a snapshot older than
/// one already written is dropped (two quick pin / unpin clicks must leave the later list on disk).
pub struct OrderedWriter {
    next: std::sync::atomic::AtomicU64,
    done: std::sync::Mutex<u64>,
}

impl OrderedWriter {
    pub const fn new() -> Self {
        OrderedWriter { next: std::sync::atomic::AtomicU64::new(0), done: std::sync::Mutex::new(0) }
    }

    /// The number of the snapshot being taken now (call it where the snapshot is cloned, on the thread that decides the order).
    pub fn ticket(&self) -> u64 {
        self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
    }

    /// Writes the snapshot unless a newer one has been written. `Ok(false)`: dropped as stale.
    pub fn write(&self, ticket: u64, path: &Path, file: &PinFile) -> std::io::Result<bool> {
        let mut done = self.done.lock().unwrap_or_else(|p| p.into_inner());
        if ticket < *done {
            return Ok(false);
        }
        save(path, file)?;
        *done = ticket;
        Ok(true)
    }
}

impl Default for OrderedWriter {
    fn default() -> Self {
        Self::new()
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::model::{IconSource, Launch};
    use std::path::PathBuf;

    fn app(name: &str, id: &str) -> App {
        App { name: name.into(), id: id.into() }
    }

    fn taskbar(name: &str, aumid: Option<&str>, exe: Option<&str>) -> PinnedApp {
        PinnedApp {
            name: name.into(),
            aumid: aumid.map(str::to_lowercase),
            exe: exe.map(str::to_lowercase),
            launch: Launch::Link(PathBuf::from(format!("{name}.lnk"))),
            icon: IconSource::File(PathBuf::from("x")),
        }
    }

    fn apps() -> Vec<App> {
        vec![
            app("Hesap Makinesi", "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"),
            app("Google Chrome", r"{6D809377-6AF0-444B-8957-A3773F02200E}\Google\Chrome\Application\chrome.exe"),
            app("Not Defteri", r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe"),
            app("Paint", "Microsoft.Paint_8wekyb3d8bbwe!App"),
            app("Işık Ayarları", "isik"),
        ]
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-startpins-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_missing_file_is_a_first_run_and_an_unreadable_one_is_not() {
        let dir = tmp("loaded");
        let path = dir.join(FILE);
        assert_eq!(load_checked(&path), Loaded::Absent);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, "{ this is not json").unwrap();
        assert_eq!(load_checked(&path), Loaded::Unreadable);
        assert_eq!(load(&path), None);
        let mut f = PinFile::default();
        f.pin(&app("Paint", "p!App"));
        save(&path, &f).unwrap();
        assert_eq!(load_checked(&path), Loaded::Found(f));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_plan_never_seeds_over_a_users_list_and_sets_an_unreadable_file_aside() {
        let mut mine = PinFile::default();
        mine.pin(&app("Paint", "p!App"));
        mine.seeded = true;
        // the user's own list is used as it is, whether or not the app list is known
        assert_eq!(plan(Loaded::Found(mine.clone()), false), Plan::Use(mine.clone()));
        assert_eq!(plan(Loaded::Found(mine.clone()), true), Plan::Use(mine));
        // nothing is decided before the apps are known
        for l in [Loaded::Absent, Loaded::Unreadable, Loaded::Found(PinFile::default())] {
            assert_eq!(plan(l, false), Plan::Wait);
        }
        assert_eq!(plan(Loaded::Absent, true), Plan::Seed { base: PinFile::default(), set_aside: false });
        assert_eq!(plan(Loaded::Found(PinFile::default()), true), Plan::Seed { base: PinFile::default(), set_aside: false });
        // a file that is there and cannot be read may hold pins: moved aside before anything is written
        assert_eq!(plan(Loaded::Unreadable, true), Plan::Seed { base: PinFile::default(), set_aside: true });
    }

    #[test]
    fn a_seeding_whose_taskbar_read_failed_is_not_marked_as_done() {
        let pins = vec![taskbar("Paint", None, None)];
        let done = seeded_file(PinFile::default(), Some(&pins), &apps()).unwrap();
        assert!(done.seeded);
        assert_eq!(done.pins.len(), 1);
        // the shell thread did not answer: no file to write, so the next opening seeds for real
        assert_eq!(seeded_file(PinFile::default(), None, &apps()), None);
        // a taskbar with no pins at all is a real answer: seeded, empty
        let empty = seeded_file(PinFile::default(), Some(&[]), &apps()).unwrap();
        assert!(empty.seeded && empty.pins.is_empty());
    }

    #[test]
    fn an_unreadable_file_is_moved_aside_without_overwriting_an_earlier_one() {
        let dir = tmp("aside");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        std::fs::write(&path, "one").unwrap();
        let a = set_aside(&path).unwrap();
        assert!(!path.exists() && a.exists());
        std::fs::write(&path, "two").unwrap();
        let b = set_aside(&path).unwrap();
        assert_ne!(a, b);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "two");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_quick_saves_leave_the_later_list_even_when_the_older_thread_runs_last() {
        let dir = tmp("ordered");
        let path = dir.join(FILE);
        let w = OrderedWriter::new();
        let (first, second) = (w.ticket(), w.ticket());
        let mut a = PinFile::default();
        a.pin(&app("A", "a!App"));
        let mut b = a.clone();
        b.pin(&app("B", "b!App"));
        // the newer snapshot is written first, then the older thread gets its turn
        assert!(w.write(second, &path, &b).unwrap());
        assert!(!w.write(first, &path, &a).unwrap(), "the stale snapshot is dropped");
        assert_eq!(load(&path).unwrap().pins.len(), 2);
        // the usual order works too
        let third = w.ticket();
        assert!(w.write(third, &path, &a).unwrap());
        assert_eq!(load(&path).unwrap().pins.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_pin_file_survives_a_round_trip_through_text() {
        let f = PinFile {
            version: 1,
            seeded: true,
            pins: vec![Pin { id: "Microsoft.Paint_8wekyb3d8bbwe!App".into(), name: "Paint".into() }, Pin { id: r"{6D809377}\Notepad++\notepad++.exe".into(), name: "Notepad++".into() }, Pin { id: "isik".into(), name: "Işık Ayarları".into() }],
        };
        assert_eq!(PinFile::parse(&f.to_json()), Some(f));
    }

    #[test]
    fn the_file_on_disk_is_written_whole_and_read_back() {
        let dir = tmp("disk");
        let path = dir.join(FILE);
        assert_eq!(load(&path), None, "no file yet");
        let mut f = PinFile::default();
        f.pin(&app("Paint", "p!App"));
        f.seeded = true;
        save(&path, &f).unwrap();
        assert_eq!(load(&path), Some(f.clone()));
        // a second save replaces it, and no temporary file is left behind
        f.pin(&app("Calc", "c!App"));
        save(&path, &f).unwrap();
        assert_eq!(load(&path).unwrap().pins.len(), 2);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec![FILE.to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn text_that_is_not_a_pin_file_is_not_one_and_a_bad_entry_costs_only_itself() {
        assert_eq!(PinFile::parse("not json"), None);
        assert_eq!(PinFile::parse("[1,2]"), None);
        let f = PinFile::parse(r#"{"version":1,"seeded":true,"pins":[{"id":"a","name":"A"},{"id":5},{"name":"no id"},{"id":"  ","name":"blank"},{"id":"A","name":"again"},{"id":"b","name":"B"}]}"#).unwrap();
        assert!(f.seeded);
        assert_eq!(f.pins.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a", "b"], "the unreadable, blank and repeated ones are gone");
        // a file from the future with no pins key at all
        let g = PinFile::parse(r#"{"version":9}"#).unwrap();
        assert!(g.pins.is_empty() && !g.seeded && g.version == 9);
    }

    #[test]
    fn pinning_and_unpinning_keep_the_order_and_never_repeat() {
        let mut f = PinFile::default();
        assert!(f.pin(&app("A", "a!App")));
        assert!(f.pin(&app("B", "b!App")));
        assert!(!f.pin(&app("A again", "A!APP")), "the same app in another case");
        assert_eq!(f.pins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A", "B"]);
        assert!(f.is_pinned("B!app"));
        assert!(f.unpin("a!app"));
        assert!(!f.unpin("a!app"));
        assert_eq!(f.pins.len(), 1);
    }

    #[test]
    fn an_uninstalled_app_stays_in_the_file_and_out_of_the_grid() {
        let mut f = PinFile::default();
        f.pin(&app("Paint", "Microsoft.Paint_8wekyb3d8bbwe!App"));
        f.pin(&app("Gone", "gone!App"));
        f.pin(&app("Hesap", "microsoft.windowscalculator_8wekyb3d8bbwe!app"));
        let shown = f.resolve(&apps());
        assert_eq!(shown.iter().map(|(i, _)| *i).collect::<Vec<_>>(), [3, 0], "in the file's order, the missing one skipped");
        assert_eq!(f.pins.len(), 3, "nothing is deleted for being missing");
    }

    #[test]
    fn the_first_seeding_takes_the_taskbars_pins_in_its_order_by_id_program_or_name() {
        let pins = vec![
            // by AppUserModelID (the pin keeps it lower case)
            taskbar("Hesap Makinesi", Some("microsoft.windowscalculator_8wekyb3d8bbwe!app"), None),
            // by program: the tail of the path ends the app's id
            taskbar("Chrome", None, Some(r"C:\Program Files\Google\Chrome\Application\chrome.exe")),
            // by name, in Turkish letters
            taskbar("işık ayarları", None, None),
            // something that is not in the list: skipped
            taskbar("Unknown", Some("nothing"), Some(r"C:\Nowhere\none.exe")),
            // the same app again (a second shortcut): once
            taskbar("Hesap Makinesi", None, None),
            taskbar("Paint", None, None),
        ];
        let got = seed(&pins, &apps());
        assert_eq!(got.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Hesap Makinesi", "Google Chrome", "Işık Ayarları", "Paint"]);
        assert_eq!(got[1].id, r"{6D809377-6AF0-444B-8957-A3773F02200E}\Google\Chrome\Application\chrome.exe");
    }

    #[test]
    fn a_bare_file_name_is_too_weak_to_match_a_program_by() {
        let pins = vec![taskbar("Whatever", None, Some("notepad.exe"))];
        assert!(seed(&pins, &apps()).is_empty(), "only its name could match, and it does not");
        assert_eq!(last_two_parts("notepad.exe"), "");
        assert_eq!(last_two_parts(r"C:\A\b.exe"), r"\a\b.exe");
    }

    #[test]
    fn seeding_stops_at_the_cap() {
        let many: Vec<App> = (0..40).map(|i| app(&format!("App{i}"), &format!("id{i}"))).collect();
        let pins: Vec<PinnedApp> = (0..40).map(|i| taskbar(&format!("App{i}"), None, None)).collect();
        assert_eq!(seed(&pins, &many).len(), SEED_MAX);
    }
}
