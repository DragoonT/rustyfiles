//! Building the list from what the machine holds, through traits: the registry, the Startup folders, the shortcuts, the running
//! processes, the disk and the environment. The real ones are in `system.rs`; the tests use fakes. Read-only throughout.

use std::collections::{HashMap, HashSet};

use super::approved;
use super::command::{Environment, Files, file_name_lower, path_key, running_check_is_meaningful, split_command};
use super::model::{
    Approval, Hive, Presence, RegData, RegValue, Running, ShortcutState, Source, SourceKind, StartupEntry, View,
};
use super::runonce;

/// The registry, read only.
pub trait RegistryRead {
    /// The values of a key (`None`: the key is not there or cannot be opened), in the order the registry gives them.
    fn values(&self, hive: Hive, view: View, key: &str) -> Option<Vec<RegValue>>;
}

/// One file in a Startup folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderFile {
    pub name: String,
    pub path: String,
    /// Has the hidden attribute (Explorer does not start hidden files: `desktop.ini` is one).
    pub hidden: bool,
}

pub trait StartupFolders {
    /// The files directly in the folder of a folder source (`None`: the folder could not be found or read). Sub-folders are not
    /// listed: Windows does not look into them.
    fn files(&self, source: Source) -> Option<Vec<FolderFile>>;
}

/// What a shortcut says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortcutData {
    /// As stored (variables not expanded); "" when the shortcut points at something that is not a file.
    pub target: String,
    pub args: String,
    pub working_dir: String,
    /// `SW_*`.
    pub show: i32,
    /// `SLDF_RUNAS_USER`: the shortcut asks to run as administrator.
    pub run_as_admin: bool,
}

pub trait Shortcuts {
    /// `None`: the shortcut could not be read.
    fn read(&self, path: &str) -> Option<ShortcutData>;
}

/// The processes of this session, as image paths.
pub trait RunningImages {
    fn images(&self) -> Vec<String>;

    /// The image paths, and the file names of the processes whose path could not be read (an elevated or protected process is not
    /// opened by the user's token). The default knows no unreadable ones.
    fn snapshot(&self) -> ProcessSnapshot {
        ProcessSnapshot { images: self.images(), unreadable_names: Vec::new() }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessSnapshot {
    pub images: Vec<String>,
    /// File names (`szExeFile`) of processes of this session whose image path was not readable.
    pub unreadable_names: Vec<String>,
}

/// The snapshot, prepared for lookups.
pub struct RunningSet {
    paths: HashSet<String>,
    unreadable_names: HashSet<String>,
}

impl RunningSet {
    pub fn new(snap: &ProcessSnapshot) -> RunningSet {
        RunningSet { paths: snap.images.iter().map(|p| path_key(p)).collect(), unreadable_names: snap.unreadable_names.iter().map(|n| n.to_lowercase()).collect() }
    }

    /// Whether a process runs for `program` (a path): `Yes` by image path, `Maybe` when only a process of that file name whose path
    /// could not be read is there, `No` otherwise. `NotChecked` when the question has no useful answer.
    pub fn state_of(&self, program: &str, found: bool) -> Running {
        if !found || program.is_empty() || !running_check_is_meaningful(program) {
            return Running::NotChecked;
        }
        if self.paths.contains(&path_key(program)) {
            Running::Yes
        } else if self.unreadable_names.contains(&file_name_lower(program)) {
            Running::Maybe
        } else {
            Running::No
        }
    }
}

/// Everything the list is built from.
pub struct Machine<'a> {
    pub registry: &'a dyn RegistryRead,
    pub folders: &'a dyn StartupFolders,
    pub shortcuts: &'a dyn Shortcuts,
    pub processes: &'a dyn RunningImages,
    pub files: &'a dyn Files,
    pub env: &'a dyn Environment,
}

fn cut_nul(s: &str) -> &str {
    s.split('\0').next().unwrap_or("")
}

/// Whether a text still holds a `%NAME%` pair: two percent signs with a name between them that has no white space or quote in it
/// (`-p 50% -q 20%` is arguments with percent signs, not a variable).
fn has_variable(s: &str) -> bool {
    let mut parts = s.split('%');
    parts.next();
    // every piece after a '%' that is followed by another '%' is a candidate name
    let rest: Vec<&str> = parts.collect();
    rest.iter().take(rest.len().saturating_sub(1)).any(|name| !name.is_empty() && !name.contains(|c: char| c.is_whitespace() || c == '"'))
}

/// The approval values of one key, by lower-case name (registry value names do not care about case).
fn approvals(m: &Machine, source: Source) -> Option<HashMap<String, Approval>> {
    let (hive, key) = source.approval_key()?;
    let values = m.registry.values(hive, View::Native, &key).unwrap_or_default();
    Some(
        values
            .into_iter()
            .map(|v| {
                let a = match &v.data {
                    RegData::Binary(b) => approved::decode(b),
                    _ => Approval::Unknown,
                };
                (cut_nul(&v.name).to_lowercase(), a)
            })
            .collect(),
    )
}

fn approval_of(table: &Option<HashMap<String, Approval>>, name: &str) -> Approval {
    match table {
        None => Approval::NotApplicable,
        Some(t) => t.get(&name.to_lowercase()).copied().unwrap_or(Approval::NoRecord),
    }
}

/// Reads every start-up source. Entries come in source order (`Source::ALL`), and within a source in the order they were read
/// (registry order; folders by name).
pub fn collect(m: &Machine) -> Vec<StartupEntry> {
    let running = RunningSet::new(&m.processes.snapshot());
    let mut out = Vec::new();
    for source in Source::ALL {
        let approvals = approvals(m, source);
        let mut ordinal = 0usize;
        match source.kind() {
            SourceKind::Run | SourceKind::RunOnce => {
                let Some((hive, view, key)) = source.registry_key() else { continue };
                for v in m.registry.values(hive, view, key).unwrap_or_default() {
                    let (raw, expand) = match &v.data {
                        RegData::Sz(s) => (cut_nul(s).to_string(), false),
                        RegData::ExpandSz(s) => (cut_nul(s).to_string(), true),
                        // not a command line
                        _ => continue,
                    };
                    // A RunOnce value is deleted by its name. A name that holds a NUL is cut here, and the cut name could be another
                    // value's: such a value is neither started nor deleted (the plan never sees it).
                    if source.kind() == SourceKind::RunOnce && v.name.contains('\0') {
                        continue;
                    }
                    ordinal += 1;
                    let name = cut_nul(&v.name).to_string();
                    out.push(registry_entry(m, &running, source, ordinal, &name, &raw, expand, approval_of(&approvals, &name)));
                }
            }
            SourceKind::Folder => {
                let mut files = m.folders.files(source).unwrap_or_default();
                files.sort_by_key(|f| f.name.to_lowercase());
                for f in files {
                    if f.hidden || f.name.eq_ignore_ascii_case("desktop.ini") {
                        continue;
                    }
                    ordinal += 1;
                    let approval = approval_of(&approvals, &f.name);
                    out.push(folder_entry(m, &running, source, ordinal, &f, approval));
                }
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn registry_entry(
    m: &Machine,
    running: &RunningSet,
    source: Source,
    ordinal: usize,
    name: &str,
    raw: &str,
    expand: bool,
    approval: Approval,
) -> StartupEntry {
    // Only a REG_EXPAND_SZ value is expanded: whether Explorer also expands a plain REG_SZ is not documented, so such a value is
    // read as written and, if it still holds a variable, its program's existence is "unknown" instead of "missing".
    let expanded = if expand { m.env.expand(raw) } else { raw.to_string() };
    let (program, args, found, split_ok) = match split_command(&expanded, m.files, m.env) {
        Ok(s) => (s.program, s.args, s.found, true),
        Err(_) => (String::new(), String::new(), false, false),
    };
    let presence = if found {
        Presence::Found
    } else if split_ok && !expand && has_variable(raw) {
        Presence::Unknown
    } else {
        Presence::Missing
    };
    let program = if found { m.files.long_path(&program) } else { program };
    let running = running.state_of(&program, found);
    let runonce = (source.kind() == SourceKind::RunOnce).then(|| runonce::parse_name(name).0);
    StartupEntry {
        source,
        ordinal,
        name: name.to_string(),
        command: raw.to_string(),
        expanded,
        program,
        args,
        working_dir: String::new(),
        show: None,
        file_path: None,
        runonce,
        shortcut: ShortcutState::NotShortcut,
        run_as_admin: false,
        approval,
        presence,
        running,
    }
}

fn quote_if_needed(s: &str) -> String {
    if s.contains(' ') { format!("\"{s}\"") } else { s.to_string() }
}

fn folder_entry(m: &Machine, running: &RunningSet, source: Source, ordinal: usize, f: &FolderFile, approval: Approval) -> StartupEntry {
    let mut e = StartupEntry {
        source,
        ordinal,
        name: f.name.clone(),
        command: quote_if_needed(&f.path),
        expanded: quote_if_needed(&f.path),
        program: f.path.clone(),
        args: String::new(),
        working_dir: String::new(),
        show: None,
        file_path: Some(f.path.clone()),
        runonce: None,
        shortcut: ShortcutState::NotShortcut,
        run_as_admin: false,
        approval,
        presence: Presence::Found,
        running: Running::NotChecked,
    };
    let is_lnk = f.name.to_lowercase().ends_with(".lnk");
    if !is_lnk {
        // any other file is started as it is: an .exe by itself, a .bat or .url through its association
        e.running = running.state_of(&e.program, true);
        return e;
    }
    match m.shortcuts.read(&f.path) {
        None => {
            // the file is there, but we cannot say where it leads
            e.shortcut = ShortcutState::Unreadable;
            e.program = String::new();
            e.presence = Presence::Unknown;
        }
        Some(d) => {
            e.shortcut = ShortcutState::Resolved;
            let target = m.env.expand(cut_nul(&d.target));
            let args = m.env.expand(cut_nul(&d.args));
            e.working_dir = m.env.expand(cut_nul(&d.working_dir));
            e.show = Some(d.show);
            e.run_as_admin = d.run_as_admin;
            if target.is_empty() {
                // a shortcut to a packaged app or a Control Panel item: opened as the shortcut, nothing to check on disk
                e.program = String::new();
                e.presence = Presence::Unknown;
                e.command = quote_if_needed(&f.path);
            } else {
                let there = m.files.exists(&target);
                e.presence = if there { Presence::Found } else { Presence::Missing };
                e.command = if args.is_empty() { quote_if_needed(&target) } else { format!("{} {}", quote_if_needed(&target), args) };
                e.expanded = e.command.clone();
                let target = if there { m.files.long_path(&target) } else { target };
                e.running = running.state_of(&target, there);
                e.program = target;
                e.args = args;
            }
        }
    }
    e
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::cell::RefCell;

    /// An in-memory registry keyed by (hive, view, lower-case key).
    #[derive(Default)]
    pub struct FakeRegistry {
        pub keys: HashMap<(Hive, View, String), Vec<RegValue>>,
    }

    impl FakeRegistry {
        pub fn put(&mut self, hive: Hive, view: View, key: &str, values: Vec<RegValue>) {
            self.keys.insert((hive, view, key.to_lowercase()), values);
        }
    }

    impl RegistryRead for FakeRegistry {
        fn values(&self, hive: Hive, view: View, key: &str) -> Option<Vec<RegValue>> {
            self.keys.get(&(hive, view, key.to_lowercase())).cloned()
        }
    }

    pub fn sz(name: &str, data: &str) -> RegValue {
        RegValue { name: name.into(), data: RegData::Sz(data.into()) }
    }
    pub fn expand_sz(name: &str, data: &str) -> RegValue {
        RegValue { name: name.into(), data: RegData::ExpandSz(data.into()) }
    }
    pub fn bin(name: &str, first: u32, ft: u64) -> RegValue {
        let mut b = first.to_le_bytes().to_vec();
        b.extend_from_slice(&ft.to_le_bytes());
        RegValue { name: name.into(), data: RegData::Binary(b) }
    }

    #[derive(Default)]
    pub struct FakeFolders {
        pub user: Option<Vec<FolderFile>>,
        pub common: Option<Vec<FolderFile>>,
    }

    impl StartupFolders for FakeFolders {
        fn files(&self, source: Source) -> Option<Vec<FolderFile>> {
            match source {
                Source::StartupFolderUser => self.user.clone(),
                Source::StartupFolderCommon => self.common.clone(),
                _ => None,
            }
        }
    }

    pub fn file(name: &str) -> FolderFile {
        FolderFile { name: name.into(), path: format!(r"C:\Startup\{name}"), hidden: false }
    }

    #[derive(Default)]
    pub struct FakeShortcuts {
        pub by_path: HashMap<String, ShortcutData>,
        pub asked: RefCell<Vec<String>>,
    }

    impl Shortcuts for FakeShortcuts {
        fn read(&self, path: &str) -> Option<ShortcutData> {
            self.asked.borrow_mut().push(path.to_string());
            self.by_path.get(path).cloned()
        }
    }

    /// Image paths, and file names of processes whose path cannot be read.
    #[derive(Default)]
    pub struct FakeProcesses(pub Vec<String>, pub Vec<String>);

    impl RunningImages for FakeProcesses {
        fn images(&self) -> Vec<String> {
            self.0.clone()
        }
        fn snapshot(&self) -> ProcessSnapshot {
            ProcessSnapshot { images: self.0.clone(), unreadable_names: self.1.clone() }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::command::fakes::{FakeEnv, FakeFiles};
    use super::fakes::*;
    use super::*;

    const RUN: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Run";
    const RUNONCE: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\RunOnce";
    const POLICY: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\Explorer\Run";
    const APPROVED: &str = r"SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved";

    struct World {
        reg: FakeRegistry,
        folders: FakeFolders,
        shortcuts: FakeShortcuts,
        processes: FakeProcesses,
        files: FakeFiles,
        env: FakeEnv,
    }

    impl World {
        fn new(files: &[&str]) -> World {
            World {
                reg: FakeRegistry::default(),
                folders: FakeFolders::default(),
                shortcuts: FakeShortcuts::default(),
                processes: FakeProcesses::default(),
                files: FakeFiles::with(files),
                env: FakeEnv::standard(),
            }
        }
        fn collect(&self) -> Vec<StartupEntry> {
            collect(&Machine {
                registry: &self.reg,
                folders: &self.folders,
                shortcuts: &self.shortcuts,
                processes: &self.processes,
                files: &self.files,
                env: &self.env,
            })
        }
    }

    #[test]
    fn sources_come_in_the_order_of_the_notes_and_ordinals_count_per_source() {
        let mut w = World::new(&[r"C:\a.exe", r"C:\b.exe", r"C:\c.exe", r"C:\d.exe", r"C:\e.exe"]);
        w.reg.put(Hive::CurrentUser, View::Native, RUN, vec![sz("u1", r"C:\a.exe"), sz("u2", r"C:\b.exe")]);
        w.reg.put(Hive::LocalMachine, View::Native, RUN, vec![sz("m1", r"C:\c.exe")]);
        w.reg.put(Hive::LocalMachine, View::Wow32, RUN, vec![sz("w1", r"C:\d.exe")]);
        w.reg.put(Hive::LocalMachine, View::Native, POLICY, vec![sz("p1", r"C:\e.exe")]);
        w.reg.put(Hive::CurrentUser, View::Native, RUNONCE, vec![sz("o1", r"C:\a.exe")]);
        w.folders.user = Some(vec![file("z.exe"), file("A.lnk")]);
        w.folders.common = Some(vec![file("c.exe")]);
        let list = w.collect();
        let order: Vec<(Source, usize, &str)> = list.iter().map(|e| (e.source, e.ordinal, e.name.as_str())).collect();
        assert_eq!(
            order,
            vec![
                (Source::PolicyRunMachine, 1, "p1"),
                (Source::RunMachine, 1, "m1"),
                (Source::RunMachine32, 1, "w1"),
                (Source::RunUser, 1, "u1"),
                (Source::RunUser, 2, "u2"),
                (Source::RunOnceUser, 1, "o1"),
                (Source::StartupFolderUser, 1, "A.lnk"),
                (Source::StartupFolderUser, 2, "z.exe"),
                (Source::StartupFolderCommon, 1, "c.exe"),
            ]
        );
        assert!(Source::ALL.windows(2).all(|p| p[0] < p[1]), "Source::ALL is in source order");
    }

    #[test]
    fn an_entry_has_its_command_split_its_program_checked_and_its_running_state_read() {
        let mut w = World::new(&[r"C:\Program Files\Foo\foo.exe", r"C:\Program Files\Bar\bar.exe"]);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![
                expand_sz("Foo", r#""%ProgramFiles%\Foo\foo.exe" --min"#),
                sz("Bar", r"C:\Program Files\Bar\bar.exe"),
                sz("Gone", r"C:\Gone\gone.exe /s"),
            ],
        );
        w.processes.0 = vec![r"c:\program files\foo\FOO.EXE".into()];
        let list = w.collect();
        let foo = &list[0];
        assert_eq!(foo.program, r"C:\Program Files\Foo\foo.exe");
        assert_eq!(foo.args, "--min");
        assert_eq!(foo.expanded, r#""C:\Program Files\Foo\foo.exe" --min"#);
        assert_eq!(foo.command, r#""%ProgramFiles%\Foo\foo.exe" --min"#, "the command is kept as stored");
        assert_eq!((foo.presence, foo.running), (Presence::Found, Running::Yes), "the same file under another spelling");
        let bar = &list[1];
        assert_eq!((bar.presence, bar.running), (Presence::Found, Running::No));
        let gone = &list[2];
        assert_eq!((gone.presence, gone.running), (Presence::Missing, Running::NotChecked));
        assert_eq!(gone.args, "/s");
        assert!(foo.enabled() && !foo.missing() && foo.already_running());
        assert!(gone.missing());
    }

    #[test]
    fn only_an_expand_value_is_expanded_a_plain_one_with_a_variable_is_unknown_not_missing() {
        let mut w = World::new(&[r"C:\Program Files\Foo\foo.exe"]);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![sz("Plain", r"%ProgramFiles%\Foo\foo.exe"), sz("PlainGone", r"C:\Gone\gone.exe"), expand_sz("ExpandGone", r"%ProgramFiles%\Nope\n.exe")],
        );
        let list = w.collect();
        assert_eq!(list[0].presence, Presence::Unknown);
        assert_eq!(list[0].expanded, r"%ProgramFiles%\Foo\foo.exe", "a REG_SZ is read as written");
        assert_eq!(list[1].presence, Presence::Missing);
        assert_eq!(list[2].presence, Presence::Missing, "an expanded variable that leads nowhere is a missing program");
    }

    #[test]
    fn values_that_are_not_command_lines_are_not_entries() {
        let mut w = World::new(&[]);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![RegValue { name: "n".into(), data: RegData::Dword(1) }, RegValue { name: "b".into(), data: RegData::Binary(vec![1]) }, sz("real", "C:\\x.exe")],
        );
        let list = w.collect();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].ordinal, 1);
    }

    #[test]
    fn startup_approved_decides_per_source_and_name_case_insensitively() {
        let mut w = World::new(&[r"C:\a.exe", r"C:\b.exe", r"C:\c.exe", r"C:\d.exe", r"C:\e.exe"]);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![sz("Off", r"C:\a.exe"), sz("On", r"C:\b.exe"), sz("Absent", r"C:\c.exe"), sz("Odd", r"C:\d.exe"), sz("WrongKind", r"C:\e.exe")],
        );
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            &format!(r"{APPROVED}\Run"),
            vec![
                bin("OFF", 3, 77),
                bin("on", 2, 0),
                bin("Odd", 7, 0),
                RegValue { name: "WrongKind".into(), data: RegData::Dword(3) },
            ],
        );
        // a same-named value in the 32-bit approval key must not leak into the 64-bit Run entries
        w.reg.put(Hive::LocalMachine, View::Native, &format!(r"{APPROVED}\Run32"), vec![bin("Absent", 3, 1)]);
        let list = w.collect();
        assert_eq!(list[0].approval, Approval::Disabled { since: Some(77) });
        assert_eq!(list[1].approval, Approval::Enabled { changed: None });
        assert_eq!(list[2].approval, Approval::NoRecord);
        assert_eq!(list[3].approval, Approval::Unknown);
        assert_eq!(list[4].approval, Approval::Unknown, "a value of another kind is not a layout we know");
        assert!(list[0].disabled() && !list[0].enabled());
        assert!(list[2].enabled());
        assert!(list[3].approval_unknown() && !list[3].enabled() && !list[3].disabled());
    }

    #[test]
    fn the_32_bit_run_key_uses_run32_and_hklm_uses_the_hklm_approval_key() {
        let mut w = World::new(&[r"C:\a.exe", r"C:\b.exe"]);
        w.reg.put(Hive::LocalMachine, View::Wow32, RUN, vec![sz("W", r"C:\a.exe")]);
        w.reg.put(Hive::LocalMachine, View::Native, RUN, vec![sz("N", r"C:\b.exe")]);
        w.reg.put(Hive::LocalMachine, View::Native, &format!(r"{APPROVED}\Run32"), vec![bin("W", 3, 5)]);
        w.reg.put(Hive::LocalMachine, View::Native, &format!(r"{APPROVED}\Run"), vec![bin("N", 6, 0)]);
        // The user's hive says the opposite for the same names: it is not consulted for HKLM entries. This PINS AN UNVERIFIED
        // ASSUMPTION: on the development machine every record of a machine-wide entry is in HKLM (an administrator account), but where
        // Windows writes the record when a standard user switches such an entry off is not known (docs/shell-startup.md, "To verify").
        w.reg.put(Hive::CurrentUser, View::Native, &format!(r"{APPROVED}\Run"), vec![bin("N", 3, 9), bin("W", 3, 9)]);
        let list = w.collect();
        let n = list.iter().find(|e| e.name == "N").unwrap();
        let wow = list.iter().find(|e| e.name == "W").unwrap();
        assert_eq!(n.approval, Approval::Enabled { changed: None });
        assert_eq!(wow.approval, Approval::Disabled { since: Some(5) });
    }

    #[test]
    fn policy_and_runonce_entries_have_no_approval_and_runonce_names_carry_their_flags() {
        let mut w = World::new(&[r"C:\a.exe"]);
        w.reg.put(Hive::LocalMachine, View::Native, POLICY, vec![sz("P", r"C:\a.exe")]);
        w.reg.put(Hive::CurrentUser, View::Native, RUNONCE, vec![sz("!*job", r"C:\a.exe"), sz("plain", r"C:\a.exe")]);
        let list = w.collect();
        assert_eq!(list[0].approval, Approval::NotApplicable);
        assert_eq!(list[0].runonce, None);
        assert_eq!(list[1].name, "!*job", "the stored name is kept for deleting the value later");
        let f = list[1].runonce.unwrap();
        assert!(f.delete_after && f.run_in_safe_mode);
        assert_eq!(list[2].runonce, Some(Default::default()));
        assert!(list[0].enabled());
    }

    #[test]
    fn startup_folder_files_hidden_ones_and_desktop_ini_are_left_out() {
        let mut w = World::new(&[r"C:\Startup\prog.exe"]);
        w.folders.user = Some(vec![
            file("prog.exe"),
            FolderFile { name: "hidden.exe".into(), path: r"C:\Startup\hidden.exe".into(), hidden: true },
            file("desktop.ini"),
            file("Desktop.INI"),
            file("notes.bat"),
        ]);
        w.reg.put(Hive::CurrentUser, View::Native, &format!(r"{APPROVED}\StartupFolder"), vec![bin("NOTES.BAT", 3, 1)]);
        let list = w.collect();
        let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["notes.bat", "prog.exe"]);
        assert!(list[0].disabled());
        assert_eq!(list[0].presence, Presence::Found);
        assert_eq!(list[0].running, Running::NotChecked, "a .bat has no image of its own");
        assert_eq!(list[1].file_path.as_deref(), Some(r"C:\Startup\prog.exe"));
    }

    #[test]
    fn common_folder_entries_take_their_approval_from_hklm() {
        let mut w = World::new(&[]);
        w.folders.common = Some(vec![file("c.lnk")]);
        w.shortcuts.by_path.insert(r"C:\Startup\c.lnk".into(), ShortcutData { target: r"C:\x.exe".into(), args: String::new(), working_dir: String::new(), show: 1, run_as_admin: false });
        w.reg.put(Hive::LocalMachine, View::Native, &format!(r"{APPROVED}\StartupFolder"), vec![bin("c.lnk", 3, 4)]);
        w.reg.put(Hive::CurrentUser, View::Native, &format!(r"{APPROVED}\StartupFolder"), vec![bin("c.lnk", 2, 0)]);
        assert_eq!(w.collect()[0].approval, Approval::Disabled { since: Some(4) });
    }

    #[test]
    fn a_shortcut_is_resolved_on_target_arguments_working_folder_and_show_command() {
        let mut w = World::new(&[r"C:\Program Files\App\app.exe"]);
        w.folders.user = Some(vec![file("App.lnk")]);
        w.shortcuts.by_path.insert(
            r"C:\Startup\App.lnk".into(),
            ShortcutData {
                target: r"%ProgramFiles%\App\app.exe".into(),
                args: "--tray %APPDATA%".into(),
                working_dir: r"%ProgramFiles%\App".into(),
                show: 7,
                run_as_admin: false,
            },
        );
        w.processes.0 = vec![r"C:\Program Files\App\app.exe".into()];
        let e = &w.collect()[0];
        assert_eq!(e.shortcut, ShortcutState::Resolved);
        assert_eq!(e.program, r"C:\Program Files\App\app.exe");
        assert_eq!(e.args, r"--tray C:\Users\u\AppData\Roaming");
        assert_eq!(e.working_dir, r"C:\Program Files\App");
        assert_eq!(e.show, Some(7));
        assert_eq!((e.presence, e.running), (Presence::Found, Running::Yes));
        assert_eq!(e.file_path.as_deref(), Some(r"C:\Startup\App.lnk"), "it is the shortcut that is opened to start it");
        assert_eq!(e.command, r#""C:\Program Files\App\app.exe" --tray C:\Users\u\AppData\Roaming"#);
    }

    #[test]
    fn a_shortcut_that_cannot_be_resolved_is_marked_and_its_program_is_unknown() {
        let mut w = World::new(&[]);
        w.folders.user = Some(vec![file("Broken.lnk")]);
        let e = &w.collect()[0];
        assert_eq!(e.shortcut, ShortcutState::Unreadable);
        assert_eq!(e.presence, Presence::Unknown);
        assert_eq!(e.running, Running::NotChecked);
        assert_eq!(e.program, "");
        assert!(e.file_path.is_some());
    }

    #[test]
    fn a_shortcut_to_a_missing_file_and_one_to_a_non_file_target() {
        let mut w = World::new(&[]);
        w.folders.user = Some(vec![file("Gone.lnk"), file("Uwp.lnk")]);
        w.shortcuts.by_path.insert(r"C:\Startup\Gone.lnk".into(), ShortcutData { target: r"C:\Gone\g.exe".into(), args: "-x".into(), working_dir: String::new(), show: 1, run_as_admin: false });
        w.shortcuts.by_path.insert(r"C:\Startup\Uwp.lnk".into(), ShortcutData { target: String::new(), args: String::new(), working_dir: String::new(), show: 1, run_as_admin: false });
        let list = w.collect();
        assert_eq!(list[0].presence, Presence::Missing);
        assert_eq!(list[1].presence, Presence::Unknown);
        assert_eq!(list[1].shortcut, ShortcutState::Resolved);
    }

    #[test]
    fn a_shortcut_to_a_folder_is_not_a_missing_program() {
        let mut w = World::new(&[]);
        w.files.dirs.insert(path_key(r"C:\Some\Folder"));
        w.folders.user = Some(vec![file("Dir.lnk")]);
        w.shortcuts.by_path.insert(r"C:\Startup\Dir.lnk".into(), ShortcutData { target: r"C:\Some\Folder".into(), args: String::new(), working_dir: String::new(), show: 1, run_as_admin: false });
        assert_eq!(w.collect()[0].presence, Presence::Found);
    }

    #[test]
    fn generic_hosts_and_broken_commands_are_not_checked_for_running() {
        let rd = r"C:\WINDOWS\system32\rundll32.exe";
        let mut w = World::new(&[rd]);
        w.reg.put(Hive::LocalMachine, View::Native, RUN, vec![sz("Rd", &format!("{rd} shell32.dll,Foo")), sz("None", ""), sz("Quotes", "\"\"")]);
        w.processes.0 = vec![rd.into()];
        let list = w.collect();
        assert_eq!((list[0].presence, list[0].running), (Presence::Found, Running::NotChecked));
        for e in &list[1..] {
            assert_eq!((e.program.as_str(), e.presence, e.running), ("", Presence::Missing, Running::NotChecked));
        }
    }

    #[test]
    fn hostile_strings_do_not_break_the_list() {
        let mut w = World::new(&[]);
        let long = "x".repeat(200_000);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![
                sz("a\0hidden", "C:\\a.exe\0 --evil"),
                sz(&long, &long),
                sz("spaces", &" ".repeat(50_000)),
                sz("quote", "\""),
                expand_sz("percent", "%%%%"),
            ],
        );
        let list = w.collect();
        assert_eq!(list.len(), 5);
        assert_eq!(list[0].name, "a");
        assert_eq!(list[0].program, r"C:\a.exe");
        assert_eq!(list[0].args, "");
        assert_eq!(list[1].program, "", "too long for CreateProcess: no program");
        assert_eq!(list[2].program, "");
        assert_eq!(list[3].program, "");
        assert_eq!(list[4].presence, Presence::Missing);
    }

    #[test]
    fn a_shortcut_that_asks_for_administrator_carries_the_flag() {
        let mut w = World::new(&[r"C:\Tools\t.exe"]);
        w.folders.user = Some(vec![file("Admin.lnk"), file("Plain.lnk")]);
        let mk = |admin| ShortcutData { target: r"C:\Tools\t.exe".into(), args: String::new(), working_dir: String::new(), show: 1, run_as_admin: admin };
        w.shortcuts.by_path.insert(r"C:\Startup\Admin.lnk".into(), mk(true));
        w.shortcuts.by_path.insert(r"C:\Startup\Plain.lnk".into(), mk(false));
        let list = w.collect();
        assert!(list[0].run_as_admin && !list[1].run_as_admin);
        // a registry command never carries it
        w.reg.put(Hive::CurrentUser, View::Native, RUN, vec![sz("r", r"C:\Tools\t.exe")]);
        assert!(w.collect().iter().filter(|e| e.source == Source::RunUser).all(|e| !e.run_as_admin));
    }

    #[test]
    fn a_process_whose_path_cannot_be_read_makes_the_same_file_name_a_maybe() {
        let mut w = World::new(&[r"C:\Apps\elev.exe", r"C:\Apps\other.exe", r"C:\Apps\seen.exe"]);
        w.reg.put(Hive::CurrentUser, View::Native, RUN, vec![sz("elev", r"C:\Apps\elev.exe"), sz("other", r"C:\Apps\other.exe"), sz("seen", r"C:\Apps\seen.exe")]);
        w.processes.0 = vec![r"C:\Apps\seen.exe".into()];
        w.processes.1 = vec!["ELEV.exe".into()];
        let list = w.collect();
        assert_eq!(list[0].running, Running::Maybe);
        assert_eq!(list[1].running, Running::No);
        assert_eq!(list[2].running, Running::Yes);
        assert!(list[0].probably_running() && !list[0].already_running());
        assert!(!list[1].probably_running());
    }

    #[test]
    fn an_unreadable_process_name_says_nothing_about_a_generic_host() {
        let rd = r"C:\WINDOWS\system32\rundll32.exe";
        let mut w = World::new(&[rd]);
        w.reg.put(Hive::LocalMachine, View::Native, RUN, vec![sz("Rd", &format!("{rd} x.dll,Go"))]);
        w.processes.1 = vec!["rundll32.exe".into()];
        assert_eq!(w.collect()[0].running, Running::NotChecked);
    }

    #[test]
    fn a_runonce_value_whose_name_holds_a_nul_is_not_listed_so_it_is_never_deleted_by_the_wrong_name() {
        let mut w = World::new(&[r"C:\a.exe", r"C:\b.exe"]);
        w.reg.put(Hive::CurrentUser, View::Native, RUNONCE, vec![sz("job\0x", r"C:\a.exe"), sz("job", r"C:\b.exe")]);
        let list = w.collect();
        assert_eq!(list.len(), 1);
        assert_eq!((list[0].name.as_str(), list[0].program.as_str(), list[0].ordinal), ("job", r"C:\b.exe", 1));
    }

    #[test]
    fn only_a_real_variable_pair_counts_as_one() {
        assert!(has_variable(r"%ProgramFiles%\x.exe"));
        assert!(has_variable(r"C:\x.exe -d %APPDATA%"));
        assert!(!has_variable(r"C:\a.exe -p 50% -q 20%"), "percent signs around white space are arguments");
        assert!(!has_variable("100%"));
        assert!(!has_variable("%%"));
        assert!(!has_variable(""));
        assert!(has_variable("a%b%c %d"));
        // and the classification: an unfound program with such arguments is missing, not "unknown"
        let mut w = World::new(&[]);
        w.reg.put(Hive::CurrentUser, View::Native, RUN, vec![sz("p", r"C:\Gone\a.exe -p 50% -q 20%")]);
        assert_eq!(w.collect()[0].presence, Presence::Missing);
    }

    #[test]
    fn an_empty_machine_gives_an_empty_list() {
        assert!(World::new(&[]).collect().is_empty());
    }

    #[test]
    fn counts_per_source_cover_every_source_and_match_the_entries() {
        let mut w = World::new(&[r"C:\a.exe", r"C:\b.exe", r"C:\c.exe"]);
        w.reg.put(
            Hive::CurrentUser,
            View::Native,
            RUN,
            vec![sz("A", r"C:\a.exe"), sz("B", r"C:\b.exe"), sz("C", r"C:\c.exe"), sz("D", r"C:\gone.exe"), sz("E", r"C:\a.exe")],
        );
        w.reg.put(Hive::CurrentUser, View::Native, &format!(r"{APPROVED}\Run"), vec![bin("B", 3, 1), bin("C", 9, 0)]);
        w.processes.0 = vec![r"C:\a.exe".into()];
        let counts = super::super::model::counts_by_source(&w.collect());
        assert_eq!(counts.len(), Source::ALL.len());
        let user = counts.iter().find(|(s, _)| *s == Source::RunUser).unwrap().1;
        assert_eq!(user.total, 5);
        assert_eq!(user.enabled, 3, "A, D and E have no record");
        assert_eq!(user.disabled, 1);
        assert_eq!(user.approval_unknown, 1);
        assert_eq!(user.missing_program, 1);
        assert_eq!(user.already_running, 2);
        assert_eq!(counts.iter().map(|(_, c)| c.total).sum::<usize>(), 5);
    }
}
