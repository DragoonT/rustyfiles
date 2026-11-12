//! Tests of the whole read, on fixtures built by hand and a backend that never touches the user's Windows state.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::destlist::DestEntry;
use super::fixture::{L, automatic_file, custom_file, lnk};
use super::lnk::{AutoList, Disk, LinkFacts};
use super::*;

struct FakeDisk(HashSet<PathBuf>);
impl Disk for FakeDisk {
    fn exists(&self, p: &Path) -> bool {
        self.0.contains(p)
    }
}

/// Files by (extension, stem); the shell links are really loaded (the shell thread), the shell's own lists are made up.
struct Fake {
    files: HashMap<(String, String), Vec<u8>>,
    disk: FakeDisk,
    docs_recent: Option<Vec<LinkFacts>>,
    docs_frequent: Option<Vec<LinkFacts>>,
    asked: RefCell<Vec<String>>,
    slow_links: bool,
    /// The shell thread does not answer: `links` waits out the time it is given, as `sta::call` does, and says `None`.
    hang_links: bool,
    /// The `within` of every call to `links`, and the number of blobs it was given.
    links_calls: RefCell<Vec<(Duration, usize)>>,
    /// The first read of these (extension, stem) is cut short, as a read in the middle of Explorer's rewrite would be.
    torn_once: RefCell<HashSet<(String, String)>>,
    file_reads: RefCell<usize>,
}

impl Fake {
    fn new(existing: &[&str]) -> Self {
        Fake {
            files: HashMap::new(),
            disk: FakeDisk(existing.iter().map(PathBuf::from).collect()),
            docs_recent: None,
            docs_frequent: None,
            asked: RefCell::new(Vec::new()),
            slow_links: false,
            hang_links: false,
            links_calls: RefCell::new(Vec::new()),
            torn_once: RefCell::new(HashSet::new()),
            file_reads: RefCell::new(0),
        }
    }
    fn put(&mut self, ext: &str, id_text: &str, bytes: Vec<u8>) {
        self.files.insert((ext.to_string(), appid::file_stem(appid::app_id(id_text))), bytes);
    }
}

impl Backend for Fake {
    fn file(&self, extension: &str, stem: &str) -> Option<Vec<u8>> {
        *self.file_reads.borrow_mut() += 1;
        let key = (extension.to_string(), stem.to_string());
        let whole = self.files.get(&key).cloned()?;
        if self.torn_once.borrow_mut().remove(&key) {
            return Some(whole[..whole.len().min(200)].to_vec());
        }
        Some(whole)
    }
    fn links(&self, blobs: Vec<Vec<u8>>, within: Duration) -> Option<Vec<Option<LinkFacts>>> {
        self.links_calls.borrow_mut().push((within, blobs.len()));
        if self.hang_links {
            std::thread::sleep(within);
            return None;
        }
        if self.slow_links {
            return None;
        }
        crate::sta::call(within.max(Duration::from_secs(10)), move || lnk::read_links(&blobs))
    }
    fn documents(&self, app_id: &str, which: AutoList, _wanted: u32, _within: Duration) -> Option<anyhow::Result<Vec<LinkFacts>>> {
        self.asked.borrow_mut().push(app_id.to_string());
        let v = if which == AutoList::Recent { &self.docs_recent } else { &self.docs_frequent };
        Some(v.clone().ok_or_else(|| anyhow::anyhow!("no list")))
    }
    fn disk(&self) -> &dyn Disk {
        &self.disk
    }
    fn known_folders(&self) -> Vec<(String, PathBuf)> {
        vec![("{6D809377-6AF0-444B-8957-A3773F02200E}".into(), PathBuf::from(r"C:\Program Files"))]
    }
}

fn e(number: u32, pin: Option<u32>, last_used: u64, path: &str) -> DestEntry {
    DestEntry { number, pin, last_used, uses: 1, path: path.into() }
}

fn doc(path: &str) -> L {
    L { target: Some(path.into()), ..Default::default() }
}

fn facts_of(path: &str) -> LinkFacts {
    LinkFacts { target: Some(path.into()), ..Default::default() }
}

#[test]
fn shell_links_built_by_hand_load_from_memory_with_everything_the_item_needs() {
    let full = L {
        target: Some(r"C:\Windows\System32\notepad.exe".into()),
        args: Some("--new-window".into()),
        working_dir: Some(r"C:\Work".into()),
        icon: Some((r"%SystemRoot%\System32\shell32.dll".into(), 5)),
        title: Some("Yeni pencere".into()),
        aumid: Some("Vendor.App".into()),
        separator: false,
    };
    let sep = L { separator: true, ..Default::default() };
    // built here, not on the shell thread: a panic there would take the whole test process down
    let blobs = vec![lnk(&full), lnk(&sep), vec![1, 2, 3], lnk(&doc(r"C:\docs\a.txt"))];
    let facts = crate::sta::call(Duration::from_secs(20), move || lnk::read_links(&blobs)).unwrap();
    assert_eq!(facts.len(), 4);
    let f = facts[0].clone().expect("the full link loads");
    assert_eq!(f.target.as_deref(), Some(Path::new(r"C:\Windows\System32\notepad.exe")));
    assert_eq!(f.arguments, "--new-window");
    assert_eq!(f.working_dir.as_deref(), Some(Path::new(r"C:\Work")));
    assert_eq!(f.title.as_deref(), Some("Yeni pencere"));
    assert_eq!(f.aumid.as_deref(), Some("Vendor.App"));
    let (icon, index) = f.icon.expect("an icon location");
    assert_eq!(index, 5);
    assert!(icon.to_string_lossy().to_lowercase().ends_with(r"system32\shell32.dll") && !icon.to_string_lossy().contains('%'), "variables expanded: {icon:?}");
    assert!(facts[1].as_ref().expect("the separator loads").separator);
    assert!(facts[2].is_none(), "three bytes are not a shell link");
    assert_eq!(facts[3].as_ref().unwrap().target.as_deref(), Some(Path::new(r"C:\docs\a.txt")));
}

#[test]
fn an_app_with_an_aumid_reads_its_pins_tasks_and_categories_from_its_two_files() {
    let id = "Vendor.App";
    let mut be = Fake::new(&[r"C:\docs\pin2.txt", r"C:\docs\pin1.txt", r"C:\Apps\app.exe", r"C:\docs\fold"]);
    be.put(
        appid::AUTOMATIC_EXT,
        id,
        automatic_file(3, &[
            (e(1, None, 900, r"C:\docs\recent.txt"), Some(doc(r"C:\docs\recent.txt"))),
            (e(2, Some(1), 100, r"C:\docs\pin2.txt"), Some(doc(r"C:\docs\pin2.txt"))),
            (e(3, Some(0), 100, r"C:\docs\pin1.txt"), Some(doc(r"C:\docs\pin1.txt"))),
            (e(4, Some(2), 100, r"C:\docs\gone.txt"), Some(doc(r"C:\docs\gone.txt"))),
            (e(5, Some(3), 100, r"C:\docs\fold"), None), // no stream: the DestList path stands in
        ]),
    );
    let task = |t: &str, a: &str| L { target: Some(r"C:\Apps\app.exe".into()), args: Some(a.into()), title: Some(t.into()), ..Default::default() };
    be.put(
        appid::CUSTOM_EXT,
        id,
        custom_file(&[
            (CategoryKind::Recent, vec![]),
            (CategoryKind::Named("Sık kullanılanlar".into()), vec![task("Birinci", "--one"), L { separator: true, ..Default::default() }, task("İkinci", "--two"), task("Birinci again", "--one")]),
            (CategoryKind::Tasks, vec![L { separator: true, ..Default::default() }, task("Yeni pencere", "--new-window"), task("Gizli", "--incognito")]),
        ]),
    );
    be.docs_recent = Some(vec![facts_of(r"C:\docs\pin1.txt")]);
    be.docs_frequent = Some(vec![]);
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));

    assert_eq!(rep.matched, Some(appid::Form::Aumid));
    assert!(rep.automatic_file && rep.custom_file);
    assert_eq!((rep.destlist_version, rep.destlist_entries, rep.custom_complete), (Some(3), 5, Some(true)));
    assert_eq!(list.which, Which::Recent);

    // pins in the pins' order; the dead one is dropped; the one with no stream comes from the DestList path
    let titles: Vec<&str> = list.pinned.iter().map(|i| i.title.as_str()).collect();
    assert_eq!(titles, vec!["pin1.txt", "pin2.txt", "fold"]);
    assert_eq!(list.pinned[0].target, JumpTarget::File(r"C:\docs\pin1.txt".into()));

    // tasks: the leading separator is dropped, the order kept
    assert_eq!(list.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), vec!["Yeni pencere", "Gizli"]);
    assert_eq!(list.tasks[0].target, JumpTarget::Link { program: r"C:\Apps\app.exe".into(), arguments: "--new-window".into(), working_dir: None, aumid: None });

    // the app's own category: the separator kept between two items, the repeat of the first dropped
    assert_eq!(list.custom.len(), 1);
    assert_eq!(list.custom[0].name, "Sık kullanılanlar");
    let kinds: Vec<&str> = list.custom[0].items.iter().map(|i| if i.target == JumpTarget::Separator { "-" } else { i.title.as_str() }).collect();
    assert_eq!(kinds, vec!["Birinci", "-", "İkinci"]);

    // the shell's Recent list had the first pin: it is not listed twice
    assert!(list.recent.is_empty(), "{:?}", list.recent);
    assert!(rep.recent_answered && rep.frequent_answered);
    assert_eq!(be.asked.borrow().as_slice(), [id, id], "the shell was asked with the explicit id as it is written");
}

#[test]
fn recent_and_frequent_come_from_the_shell_and_the_limit_cuts_each_part() {
    let id = "Vendor.Viewer";
    let mut be = Fake::new(&["C:\\a.txt", "C:\\b.txt", "C:\\c.txt", "C:\\d.txt", "C:\\e.txt"]);
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[]));
    be.put(appid::CUSTOM_EXT, id, custom_file(&[(CategoryKind::Frequent, vec![])]));
    be.docs_recent = Some(["C:\\a.txt", "C:\\b.txt", "C:\\c.txt", "C:\\gone.txt", "C:\\d.txt"].iter().map(|p| facts_of(p)).collect());
    be.docs_frequent = Some(vec![facts_of("C:\\e.txt"), facts_of("C:\\e.txt")]);
    let (list, _) = read_detailed(&be, &AppKey::from_aumid(id), 3, Duration::from_secs(10));
    assert_eq!(list.which, Which::Frequent);
    assert_eq!(list.recent.len(), 3, "cut to the limit; the dead one does not count");
    assert_eq!(list.frequent.len(), 1, "a repeat is one item");
    // an app that asked for Frequent alone is shown Frequent
    assert_eq!(list.automatic(), &list.frequent[..]);
}

#[test]
fn when_the_shell_cannot_give_recent_the_files_newest_entries_stand_in() {
    let id = "Vendor.Notes";
    let mut be = Fake::new(&["C:\\old.txt", "C:\\new.txt"]);
    be.put(
        appid::AUTOMATIC_EXT,
        id,
        automatic_file(3, &[(e(1, None, 100, "C:\\old.txt"), Some(doc("C:\\old.txt"))), (e(2, None, 900, "C:\\new.txt"), Some(doc("C:\\new.txt")))]),
    );
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));
    assert!(!rep.recent_answered);
    assert_eq!(list.recent.iter().map(|i| i.title.as_str()).collect::<Vec<_>>(), vec!["new.txt", "old.txt"], "newest first");
    assert!(list.frequent.is_empty());
    assert_eq!(list.which, Which::Unknown);
}

#[test]
fn a_program_without_an_aumid_is_found_by_its_known_folder_path_and_an_unknown_app_is_empty() {
    let mut be = Fake::new(&[r"C:\Program Files\V\v.exe", "C:\\x.txt"]);
    be.put(appid::AUTOMATIC_EXT, r"{6D809377-6AF0-444B-8957-A3773F02200E}\V\v.exe", automatic_file(3, &[(e(1, Some(0), 5, "C:\\x.txt"), Some(doc("C:\\x.txt")))]));
    let key = AppKey::from_exe(r"c:\program files\v\v.exe");
    let (list, rep) = read_detailed(&be, &key, 10, Duration::from_secs(10));
    assert_eq!(rep.matched, Some(appid::Form::KnownFolderPath), "the lower-case spelling of the path is the same app");
    assert_eq!(list.pinned.len(), 1);
    // an app nothing is known about, an empty key, and a zero limit
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid("Nobody.Home"), 10, Duration::from_secs(10));
    assert!(list.is_empty() && rep.matched.is_none() && !rep.automatic_file);
    assert!(read_detailed(&be, &AppKey::default(), 10, Duration::from_secs(10)).0.is_empty());
    assert!(read_detailed(&be, &key, 0, Duration::from_secs(10)).0.is_empty());
}

#[test]
fn a_shell_that_does_not_answer_in_time_gives_what_is_known_and_says_so() {
    let id = "Vendor.Slow";
    let mut be = Fake::new(&["C:\\x.txt"]);
    be.slow_links = true;
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[(e(1, Some(0), 5, "C:\\x.txt"), Some(doc("C:\\x.txt")))]));
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));
    assert!(rep.out_of_time);
    // the link could not be loaded, but the DestList still names the pin's path
    assert_eq!(list.pinned.len(), 1);
    assert_eq!(list.pinned[0].title, "x.txt");
}

#[test]
fn damaged_files_give_an_empty_list_and_never_panic() {
    let id = "Vendor.Broken";
    let mut be = Fake::new(&[]);
    be.put(appid::AUTOMATIC_EXT, id, vec![0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 1, 2, 3]);
    be.put(appid::CUSTOM_EXT, id, vec![2, 0, 0, 0, 9, 0, 0, 0, 0, 0, 0, 0, 7]);
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));
    assert!(list.is_empty());
    assert_eq!(rep.destlist_version, None);
    assert_eq!(rep.custom_complete, Some(false));
}

#[test]
fn the_dock_s_identities_make_keys() {
    let app = |key: &str, launch: Option<Launch>| DockApp { key: key.into(), name: String::new(), pinned: false, launch, icon: IconSource::Window(0), windows: vec![], active: false };
    assert_eq!(AppKey::from_dock_app(&app("vendor.app", None)), AppKey::from_aumid("vendor.app"));
    assert_eq!(AppKey::from_dock_app(&app(r"c:\apps\a.exe", None)), AppKey::from_exe(r"c:\apps\a.exe"));
    assert!(AppKey::from_dock_app(&app("pid:42", None)).is_empty());
    let with_launch = AppKey::from_dock_app(&app("vendor.app", Some(Launch::Exe { path: r"C:\a\b.exe".into(), args: String::new() })));
    assert_eq!(with_launch, AppKey { aumid: Some("vendor.app".into()), exe: Some(r"C:\a\b.exe".into()) });
}

// ---- opening ----

struct Recording(RefCell<Vec<Launch>>, RefCell<Vec<(String, String)>>);
impl Starter for Recording {
    fn start(&self, l: &Launch) -> anyhow::Result<()> {
        self.0.borrow_mut().push(l.clone());
        Ok(())
    }
    fn start_packaged_with(&self, aumid: &str, arguments: &str) -> anyhow::Result<()> {
        self.1.borrow_mut().push((aumid.to_string(), arguments.to_string()));
        Ok(())
    }
}

#[test]
fn choosing_an_item_starts_what_the_dock_s_launcher_understands_and_a_separator_starts_nothing() {
    let item = |target| JumpItem { title: "t".into(), icon: None, target };
    let rec = Recording(RefCell::new(Vec::new()), RefCell::new(Vec::new()));
    open_with(&rec, &item(JumpTarget::File(r"C:\a.txt".into()))).unwrap();
    open_with(&rec, &item(JumpTarget::Link { program: r"C:\p.exe".into(), arguments: "-x".into(), working_dir: Some(r"C:\w".into()), aumid: None })).unwrap();
    open_with(&rec, &item(JumpTarget::Link { program: PathBuf::new(), arguments: String::new(), working_dir: None, aumid: Some("V.A_x!App".into()) })).unwrap();
    assert!(open_with(&rec, &item(JumpTarget::Separator)).is_err());
    assert!(open_with(&rec, &item(JumpTarget::Link { program: PathBuf::new(), arguments: String::new(), working_dir: None, aumid: None })).is_err());
    assert_eq!(
        rec.0.borrow().as_slice(),
        [Launch::Link(r"C:\a.txt".into()), Launch::Exe { path: r"C:\p.exe".into(), args: "-x".into() }, Launch::Packaged("V.A_x!App".into())]
    );
}

#[test]
fn tidy_drops_repeats_and_stray_separators_and_counts_only_items() {
    let it = |t: &str| JumpItem { title: t.into(), icon: None, target: JumpTarget::File(t.into()) };
    let sep = || JumpItem { title: String::new(), icon: None, target: JumpTarget::Separator };
    let out = tidy(vec![sep(), it("a"), sep(), sep(), it("b"), it("a"), sep(), it("c"), it("d"), sep()], 3);
    let shape: Vec<&str> = out.iter().map(|i| if i.target == JumpTarget::Separator { "-" } else { i.title.as_str() }).collect();
    assert_eq!(shape, vec!["a", "-", "b", "-", "c"]);
}

// ---- what the review found ----

#[test]
fn a_packaged_task_with_arguments_hands_them_to_the_activation_call() {
    let rec = Recording(RefCell::new(Vec::new()), RefCell::new(Vec::new()));
    let task = |arguments: &str| JumpItem {
        title: "New window".into(),
        icon: None,
        target: JumpTarget::Link { program: PathBuf::new(), arguments: arguments.into(), working_dir: None, aumid: Some("V.A_x!App".into()) },
    };
    open_with(&rec, &task("--new-window")).unwrap();
    assert_eq!(rec.1.borrow().as_slice(), [("V.A_x!App".to_string(), "--new-window".to_string())]);
    assert!(rec.0.borrow().is_empty(), "not started a second time without them");
    // no arguments: the dock's own Packaged launch, as before
    open_with(&rec, &task("")).unwrap();
    assert_eq!(rec.0.borrow().as_slice(), [Launch::Packaged("V.A_x!App".into())]);
    // a program with an id and arguments is still run as the program
    let both = JumpItem { title: "t".into(), icon: None, target: JumpTarget::Link { program: r"C:\p.exe".into(), arguments: "-x".into(), working_dir: None, aumid: Some("V.A_x!App".into()) } };
    open_with(&rec, &both).unwrap();
    assert_eq!(rec.0.borrow().last(), Some(&Launch::Exe { path: r"C:\p.exe".into(), args: "-x".into() }));
    assert_eq!(rec.1.borrow().len(), 1);
}

#[test]
fn a_custom_list_that_names_neither_category_shows_no_automatic_items_and_no_custom_list_shows_recent() {
    let task = L { target: Some(r"C:\Apps\app.exe".into()), title: Some("Yeni".into()), ..Default::default() };
    let recent_doc = e(1, None, 900, r"C:\docs\r.txt");
    let mut be = Fake::new(&[r"C:\Apps\app.exe", r"C:\docs\r.txt"]);
    be.docs_recent = Some(vec![facts_of(r"C:\docs\r.txt")]);
    be.docs_frequent = Some(vec![]);
    // tasks only: the app wrote its own list and asked for neither category
    be.put(appid::AUTOMATIC_EXT, "Vendor.Tasks", automatic_file(3, &[(recent_doc.clone(), Some(doc(r"C:\docs\r.txt")))]));
    be.put(appid::CUSTOM_EXT, "Vendor.Tasks", custom_file(&[(CategoryKind::Tasks, vec![task.clone()])]));
    let (list, _) = read_detailed(&be, &AppKey::from_aumid("Vendor.Tasks"), 10, Duration::from_secs(10));
    assert_eq!(list.which, Which::Neither);
    assert_eq!(list.tasks.len(), 1);
    assert_eq!(list.recent.len(), 1, "what the shell listed is still in the list");
    assert!(list.automatic().is_empty(), "but the taskbar would not show it");
    // no custom file at all: the default, Recent
    be.put(appid::AUTOMATIC_EXT, "Vendor.Plain", automatic_file(3, &[(recent_doc.clone(), Some(doc(r"C:\docs\r.txt")))]));
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid("Vendor.Plain"), 10, Duration::from_secs(10));
    assert!(!rep.custom_file);
    assert_eq!(list.which, Which::Unknown);
    assert_eq!(list.automatic().len(), 1);
    // an empty custom file (24 zero bytes, what an app that cleared its list leaves) is no custom list either
    be.put(appid::AUTOMATIC_EXT, "Vendor.Cleared", automatic_file(3, &[(recent_doc, Some(doc(r"C:\docs\r.txt")))]));
    be.put(appid::CUSTOM_EXT, "Vendor.Cleared", vec![0; 24]);
    let (list, _) = read_detailed(&be, &AppKey::from_aumid("Vendor.Cleared"), 10, Duration::from_secs(10));
    assert_eq!(list.which, Which::Unknown);
    assert_eq!(list.automatic().len(), 1);
    // naming both among its own categories: both
    be.put(appid::AUTOMATIC_EXT, "Vendor.Both", automatic_file(3, &[]));
    be.put(appid::CUSTOM_EXT, "Vendor.Both", custom_file(&[(CategoryKind::Tasks, vec![task]), (CategoryKind::Recent, vec![]), (CategoryKind::Frequent, vec![])]));
    let (list, _) = read_detailed(&be, &AppKey::from_aumid("Vendor.Both"), 10, Duration::from_secs(10));
    assert_eq!(list.which, Which::Both);
}

#[test]
fn a_shell_that_hangs_costs_the_budget_and_not_much_more_and_nothing_else_is_queued_behind_it() {
    let id = "Vendor.Hang";
    let mut be = Fake::new(&["C:\\x.txt", "C:\\y.txt"]);
    be.hang_links = true;
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[(e(1, Some(0), 5, "C:\\x.txt"), Some(doc("C:\\x.txt"))), (e(2, None, 9, "C:\\y.txt"), Some(doc("C:\\y.txt")))]));
    be.put(appid::CUSTOM_EXT, id, custom_file(&[(CategoryKind::Tasks, vec![L { target: Some("C:\\x.txt".into()), ..Default::default() }])]));
    let budget = Duration::from_millis(150);
    let t = std::time::Instant::now();
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, budget);
    let took = t.elapsed();
    assert!(rep.out_of_time);
    assert!(took >= budget && took < budget + Duration::from_millis(500), "took {took:?}");
    let calls = be.links_calls.borrow();
    assert_eq!(calls.len(), 1, "the one call for the custom links and the pins; the stand-in for Recent is not tried with no time left: {calls:?}");
    assert!(calls[0].0 <= budget, "the shell was given what was left of the budget, not more: {:?}", calls[0].0);
    assert!(be.asked.borrow().is_empty(), "the shell's own lists are not asked after the budget is spent");
    // what is known without the shell still comes back: the pin's path from the DestList
    assert_eq!(list.pinned.len(), 1);
}

#[test]
fn the_links_handed_to_the_shell_thread_are_capped_over_all_the_categories() {
    let id = "Vendor.Many";
    let mut be = Fake::new(&[]);
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[]));
    // 20 categories of 25 tasks: 20 per category are taken at a limit of 10 (400), and only MAX_CUSTOM_LINKS of those go on
    let cats: Vec<(CategoryKind, Vec<L>)> =
        (0..20).map(|c| (CategoryKind::Named(format!("Category {c}")), (0..25).map(|k| L { target: Some(format!("C:\\Apps\\app{c}_{k}.exe")), ..Default::default() }).collect())).collect();
    be.put(appid::CUSTOM_EXT, id, custom_file(&cats));
    let _ = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(20));
    let calls = be.links_calls.borrow();
    assert_eq!(calls.first().map(|c| c.1), Some(MAX_CUSTOM_LINKS), "{calls:?}");
}

#[test]
fn a_file_caught_in_the_middle_of_a_rewrite_is_read_once_more() {
    let id = "Vendor.Torn";
    let mut be = Fake::new(&["C:\\x.txt", "C:\\t.txt"]);
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[(e(1, Some(0), 5, "C:\\x.txt"), Some(doc("C:\\x.txt")))]));
    be.put(appid::CUSTOM_EXT, id, custom_file(&[(CategoryKind::Tasks, vec![L { target: Some("C:\\t.txt".into()), title: Some("Task".into()), ..Default::default() }])]));
    for ext in [appid::AUTOMATIC_EXT, appid::CUSTOM_EXT] {
        be.torn_once.borrow_mut().insert((ext.to_string(), appid::file_stem(appid::app_id(id))));
    }
    let (list, rep) = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));
    assert_eq!(list.pinned.len(), 1, "the pin of the second read of the automatic file");
    assert_eq!(list.tasks.len(), 1, "the task of the second read of the custom file");
    assert_eq!((rep.destlist_version, rep.custom_complete), (Some(3), Some(true)));
    assert_eq!(*be.file_reads.borrow(), 2 + 2, "two files asked for, each once more");
    // a whole file with an empty list is not torn: no second read
    let mut be = Fake::new(&[]);
    be.put(appid::AUTOMATIC_EXT, id, automatic_file(3, &[]));
    let _ = read_detailed(&be, &AppKey::from_aumid(id), 10, Duration::from_secs(10));
    assert_eq!(*be.file_reads.borrow(), 2, "the pair of reads that found the file, and no more");
}

#[test]
fn the_dock_s_lower_case_key_gives_way_to_the_launch_path_with_the_program_s_own_spelling() {
    let app = |key: &str, launch: Option<Launch>| DockApp { key: key.into(), name: String::new(), pinned: false, launch, icon: IconSource::Window(0), windows: vec![], active: false };
    let real = "C:\\Users\\\u{130}brahim\\AppData\\Local\\Programs\\app\\app.exe";
    let key = real.to_lowercase();
    assert_ne!(key, real.to_ascii_lowercase(), "the lower-cased spelling is not the one the hash was made of");
    let with_launch = AppKey::from_dock_app(&app(&key, Some(Launch::Exe { path: real.into(), args: String::new() })));
    assert_eq!(with_launch.exe.as_deref(), Some(Path::new(real)));
    // a launch path of another program does not replace the key's
    let other = AppKey::from_dock_app(&app(r"c:\a\a.exe", Some(Launch::Exe { path: r"C:\b\b.exe".into(), args: String::new() })));
    assert_eq!(other.exe.as_deref(), Some(Path::new(r"c:\a\a.exe")));
}
