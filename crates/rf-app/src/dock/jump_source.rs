//! Where the app menu gets its jump-list sections (Tasks, an app's own categories, Pinned, Recent or Frequent) and how an entry is run.
//!
//! THIS FILE IS THE SEAM between the menu and the jump-list reader (`rf_win::jumplists`): the menu is written against this small model
//! (a section is a title and entries, an entry is a title, an optional icon and an ACTION the menu never looks into), and [`sections`]
//! and [`run`] are the only two functions it calls. The order is the Windows taskbar's, top to bottom: Pinned, the app's own
//! categories, Recent or Frequent (whichever the app asked Windows to show), Tasks. An entry's action carries its whole target as
//! text, so running it needs no second read of the list and cannot run another entry than the one that was shown.
//!
//! Both BLOCK (the disk, the shell, COM): the menu calls them from a worker thread, and never waits for [`sections`]: the menu is open
//! with its fixed part before the sections are asked for, and they appear when they arrive.

use std::path::PathBuf;

use rf_win::dock::{DockApp, IconSource, Launch};
use rf_win::jumplists::{AppKey, JumpItem, JumpTarget, Which};

use crate::t;

/// Which app's jump list: everything a reader may need to find it. Plain data, so it can go to a worker thread.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpApp {
    /// The dock's identity of the app (lower case): its AppUserModelID, else its program.
    pub key: String,
    /// The name the dock shows.
    pub name: String,
    /// The AppUserModelID, when the app has one that a jump-list file is named by.
    pub aumid: Option<String>,
    /// The program, when the app has one on disk.
    pub exe: Option<PathBuf>,
}

impl JumpApp {
    pub fn of(app: &DockApp) -> JumpApp {
        let aumid = match &app.launch {
            Some(Launch::Packaged(id)) => Some(id.clone()),
            _ if rf_win::dock::model::looks_packaged(&app.key) => Some(app.key.clone()),
            _ => None,
        };
        let exe = match &app.launch {
            Some(Launch::Exe { path, .. }) => Some(path.clone()),
            _ => match &app.icon {
                IconSource::File(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) => Some(p.clone()),
                _ => None,
            },
        };
        JumpApp { key: app.key.clone(), name: app.name.clone(), aumid, exe }
    }
}

/// What an entry does when chosen. The menu keeps it and hands it back to [`run`]; it never looks inside.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action(pub String);

/// One line of a section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub title: String,
    /// The line's own icon, when it has one (a task's, a recent document's).
    pub icon: Option<IconSource>,
    pub action: Action,
}

/// One block of the menu: its title (Tasks, Recent ...; `None` for a block with no heading) and its lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Section {
    pub title: Option<String>,
    pub entries: Vec<Entry>,
}

/// The jump list of an app, in the order the menu shows it. Empty when the app has none (or nothing reads them yet). Blocks: a worker
/// thread only.
pub fn sections(app: &JumpApp) -> Vec<Section> {
    let key = app_key(app);
    if key.is_empty() {
        return Vec::new();
    }
    let list = rf_win::jumplists::read(&key, PER_PART);
    let mut out: Vec<Section> = Vec::new();
    let mut push = |title: String, items: &[JumpItem]| {
        // (a line between groups of tasks starts a new block without a heading)
        let mut title = Some(title);
        for group in items.split(|i| matches!(i.target, JumpTarget::Separator)) {
            let entries: Vec<Entry> = group.iter().filter_map(|i| Some(Entry { title: i.title.clone(), icon: i.icon.clone(), action: encode(&i.target)? })).collect();
            if !entries.is_empty() {
                out.push(Section { title: title.take(), entries });
            }
        }
    };
    push(t!("Sabitlenenler").to_string(), &list.pinned);
    for c in &list.custom {
        push(c.name.clone(), &c.items);
    }
    let automatic = if list.which == Which::Frequent { t!("Sık kullanılanlar") } else { t!("Son kullanılanlar") };
    push(automatic.to_string(), list.automatic());
    push(t!("Görevler").to_string(), &list.tasks);
    out
}

/// The most entries of one part of a jump list (Windows shows ten recent items by default).
const PER_PART: usize = 10;

/// Between the fields of an action's text: a character no path, argument string or AppUserModelID holds.
const SEP: char = '\u{1f}';

/// The dock's identity of the app as the reader wants it: the key is an AppUserModelID unless it is a path (or `pid:..`, which names
/// nothing a jump list could be found by); the program keeps its own spelling, which the file's name is made of.
fn app_key(app: &JumpApp) -> AppKey {
    let key = app.key.trim();
    let is_path = key.contains('\\') || key.contains('/');
    let aumid = app.aumid.clone().or_else(|| (!key.is_empty() && !is_path && !key.starts_with("pid:")).then(|| key.to_string()));
    let exe = app.exe.clone().or_else(|| is_path.then(|| PathBuf::from(key)));
    AppKey { aumid, exe }
}

/// An entry's target as text (`None` for a separator, which is no entry).
fn encode(t: &JumpTarget) -> Option<Action> {
    let path = |p: &std::path::Path| p.to_string_lossy().into_owned();
    match t {
        JumpTarget::File(p) => Some(Action(format!("file{SEP}{}", path(p)))),
        JumpTarget::Link { program, arguments, working_dir, aumid } => Some(Action(
            ["link", &path(program), arguments, &working_dir.as_deref().map(path).unwrap_or_default(), aumid.as_deref().unwrap_or_default()].join(&SEP.to_string()),
        )),
        JumpTarget::Separator => None,
    }
}

/// The target an action's text names (`None` for text this file did not write).
fn decode(a: &Action) -> Option<JumpTarget> {
    let parts: Vec<&str> = a.0.split(SEP).collect();
    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
    match parts.as_slice() {
        ["file", p] if !p.is_empty() => Some(JumpTarget::File(PathBuf::from(p))),
        ["link", program, arguments, dir, aumid] if !program.is_empty() || !aumid.is_empty() => {
            Some(JumpTarget::Link { program: PathBuf::from(program), arguments: arguments.to_string(), working_dir: some(dir).map(PathBuf::from), aumid: some(aumid) })
        }
        _ => None,
    }
}

/// Runs an entry's action (starts the task, opens the document). Blocks: a worker thread only.
pub fn run(_app: &JumpApp, action: &Action) -> anyhow::Result<()> {
    let target = decode(action).ok_or_else(|| anyhow::anyhow!("not an entry of a jump list"))?;
    rf_win::jumplists::open(&JumpItem { title: String::new(), icon: None, target })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(key: &str, launch: Option<Launch>, icon: IconSource) -> DockApp {
        DockApp { key: key.into(), name: "x".into(), pinned: true, launch, icon, windows: vec![], active: false }
    }

    #[test]
    fn a_packaged_app_is_found_by_its_package_id_and_a_classic_one_by_its_program() {
        let p = JumpApp::of(&app("microsoft.paint_8wekyb3d8bbwe!app", Some(Launch::Packaged("Microsoft.Paint_8wekyb3d8bbwe!App".into())), IconSource::Packaged("x".into())));
        assert_eq!(p.aumid.as_deref(), Some("Microsoft.Paint_8wekyb3d8bbwe!App"));
        assert_eq!(p.exe, None);
        let c = JumpApp::of(&app(r"c:\a\code.exe", Some(Launch::Exe { path: PathBuf::from(r"C:\a\Code.exe"), args: String::new() }), IconSource::File(PathBuf::from(r"C:\a\Code.exe"))));
        assert_eq!(c.aumid, None);
        assert_eq!(c.exe, Some(PathBuf::from(r"C:\a\Code.exe")));
    }

    #[test]
    fn a_pinned_shortcut_app_still_gives_its_program_when_the_icon_names_it() {
        let a = JumpApp::of(&app("k", Some(Launch::Link(PathBuf::from(r"C:\pins\X.lnk"))), IconSource::File(PathBuf::from(r"C:\a\x.exe"))));
        assert_eq!(a.exe, Some(PathBuf::from(r"C:\a\x.exe")));
        let none = JumpApp::of(&app("k", Some(Launch::Link(PathBuf::from(r"C:\pins\X.lnk"))), IconSource::Window(5)));
        assert_eq!(none.exe, None);
    }

    #[test]
    fn an_app_with_nothing_to_go_by_has_no_jump_list_and_a_strange_action_runs_nothing() {
        let a = JumpApp { key: "pid:42".into(), name: "x".into(), aumid: None, exe: None };
        assert!(app_key(&a).is_empty());
        assert!(sections(&a).is_empty());
        assert!(run(&a, &Action("anything".into())).is_err());
        assert!(run(&a, &Action(String::new())).is_err());
    }

    #[test]
    fn the_dock_s_key_becomes_the_reader_s_identity() {
        // an explicit AppUserModelID is the key itself
        let k = app_key(&JumpApp { key: "microsoft.visualstudiocode".into(), name: "x".into(), aumid: None, exe: Some(PathBuf::from(r"C:\a\Code.exe")) });
        assert_eq!((k.aumid.as_deref(), k.exe), (Some("microsoft.visualstudiocode"), Some(PathBuf::from(r"C:\a\Code.exe"))));
        // a path key is the program; the program's own spelling wins when it is known
        let k = app_key(&JumpApp { key: r"c:\a\code.exe".into(), name: "x".into(), aumid: None, exe: Some(PathBuf::from(r"C:\a\Code.exe")) });
        assert_eq!((k.aumid, k.exe), (None, Some(PathBuf::from(r"C:\a\Code.exe"))));
        let k = app_key(&JumpApp { key: r"c:\a\code.exe".into(), name: "x".into(), aumid: None, exe: None });
        assert_eq!(k.exe, Some(PathBuf::from(r"c:\a\code.exe")));
        // a packaged app's id as the launcher has it
        let k = app_key(&JumpApp { key: "microsoft.paint_8wekyb3d8bbwe!app".into(), name: "x".into(), aumid: Some("Microsoft.Paint_8wekyb3d8bbwe!App".into()), exe: None });
        assert_eq!(k.aumid.as_deref(), Some("Microsoft.Paint_8wekyb3d8bbwe!App"));
    }

    #[test]
    fn an_action_carries_its_whole_target_and_comes_back_the_same() {
        let targets = [
            JumpTarget::File(PathBuf::from(r"C:\Belgeler\rapor 2026.docx")),
            JumpTarget::Link { program: PathBuf::from(r"C:\a\app.exe"), arguments: "--new-window \"a b\"".into(), working_dir: Some(PathBuf::from(r"C:\a")), aumid: None },
            JumpTarget::Link { program: PathBuf::from(r"C:\a\app.exe"), arguments: String::new(), working_dir: None, aumid: None },
            JumpTarget::Link { program: PathBuf::new(), arguments: "x".into(), working_dir: None, aumid: Some("Pkg_1!App".into()) },
        ];
        for t in targets {
            let a = encode(&t).unwrap();
            assert_eq!(decode(&a), Some(t));
        }
        assert_eq!(encode(&JumpTarget::Separator), None);
        // text this file did not write names nothing: no program and no id, a missing field, another word
        let sep = SEP.to_string();
        assert_eq!(decode(&Action(["link", "", "args", "", ""].join(&sep))), None);
        assert_eq!(decode(&Action(["link", "a.exe"].join(&sep))), None);
        assert_eq!(decode(&Action(["file", ""].join(&sep))), None);
        assert_eq!(decode(&Action(["run", "a.exe"].join(&sep))), None);
    }
}
