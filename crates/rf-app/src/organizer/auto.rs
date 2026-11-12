//! Rules that run by themselves: a folder watcher per watched folder, a plan for the files that
//! changed (not the whole folder), a run without a dialog.
//!
//! Costs nothing while it is not wanted: with the master switch off or no automatic rule there
//! is no global, no watcher, no thread and no timer here. With it on there is one watcher per
//! folder (a recursive one when a rule of that folder includes subfolders) and, while files
//! wait to be looked at again, one timer.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui_kit::*;
use rf_core::organize::safety::norm;
use rf_core::organize::store::RulesFile;
use rf_core::organize::{Dest, ProtectedPaths, Rule};
use rf_win::tree_watch::TreeWatch;
use rf_win::watch::DirWatch;

use super::{RunSpec, Source};
use crate::t;

/// Changes must pause this long before the rules look at the files.
const SETTLE: Duration = Duration::from_secs(3);
/// A file that was in use or too new is looked at again after this, up to [`ATTEMPTS`] times.
const RETRY_AFTER: Duration = Duration::from_secs(12);
const ATTEMPTS: u8 = 20;
/// What a capped run left is looked at again after this.
const CONTINUE_AFTER: Duration = Duration::from_secs(2);
/// Files a run made are left alone by the watcher for this long (a chain of rules cannot go
/// round in circles).
const PRODUCED_FOR: Duration = Duration::from_secs(600);

/// One folder to watch: its normalized path (the key), the path, and whether the tree below is
/// watched too.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Want {
    pub key: String,
    pub folder: PathBuf,
    pub tree: bool,
}

/// [`wanted_and_missing`] without the missing ones (the tests use it).
#[cfg(test)]
pub fn wanted(file: &RulesFile, protected: &ProtectedPaths, folder_exists: &dyn Fn(&std::path::Path) -> bool) -> Vec<Want> {
    wanted_and_missing(file, protected, folder_exists).0
}

/// The folders to watch, and the automatic rules (id, name) whose folder is not there (renamed, a drive that is
/// not plugged in): they cannot be watched and the person should hear that.
pub fn wanted_and_missing(file: &RulesFile, protected: &ProtectedPaths, folder_exists: &dyn Fn(&std::path::Path) -> bool) -> (Vec<Want>, Vec<(u32, String)>) {
    if !file.auto_enabled {
        return (Vec::new(), Vec::new());
    }
    let mut out: Vec<Want> = Vec::new();
    let mut missing: Vec<(u32, String)> = Vec::new();
    // (a rule that looks back in time cannot run by itself: see `Rule::needs_manual_run`)
    for r in file.rules.iter().filter(|r| r.enabled && r.auto && !r.needs_manual_run()) {
        if r.validate().is_err() || super::refusal(protected, &r.folder, r.subfolders).is_some() {
            continue;
        }
        if !folder_exists(&r.folder) {
            missing.push((r.id, r.name.clone()));
            continue;
        }
        let key = norm(&r.folder);
        match out.iter_mut().find(|w| w.key == key) {
            Some(w) => w.tree |= r.subfolders,
            None => out.push(Want { key, folder: r.folder.clone(), tree: r.subfolders }),
        }
    }
    out.sort_by(|a, b| a.key.cmp(&b.key));
    (out, missing)
}

struct Msg {
    key: String,
    paths: Vec<PathBuf>,
    rescan: bool,
    attempt: u8,
}

/// What waits to be looked at in one folder.
#[derive(Default, Debug)]
struct Pending {
    paths: BTreeSet<PathBuf>,
    rescan: bool,
    attempt: u8,
}

impl Pending {
    fn merge(&mut self, m: Msg) {
        self.rescan |= m.rescan;
        self.paths.extend(m.paths);
        self.attempt = self.attempt.max(m.attempt);
    }
    fn is_empty(&self) -> bool {
        !self.rescan && self.paths.is_empty()
    }
}

struct Watch {
    _dir: Option<DirWatch>,
    _tree: Option<TreeWatch>,
}

#[derive(Default)]
struct Auto {
    wants: Vec<Want>,
    watches: Vec<Watch>,
    pending: HashMap<String, Pending>,
    running: HashSet<String>,
    produced: VecDeque<(Instant, String)>,
    /// Rules already complained about this session (a loop, a broken rule).
    warned: HashSet<u32>,
    tx: Option<UnboundedSender<Msg>>,
}
impl Global for Auto {}

/// Brings the watchers in line with the rules. Cheap to call after every change of the rules:
/// nothing is restarted unless the set of watched folders changed.
pub fn sync(cx: &mut App) {
    // helpers (dock, file dialogs) never watch; neither does a second copy running beside the real one
    // on the real rules (a test copy has a settings folder of its own: its rules are its own)
    if crate::background::side_process() || (std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_some() && std::env::var_os("RUSTYFILES_CONFIG_DIR").is_none()) {
        return;
    }
    let (wants, missing) = wanted_and_missing(&super::state(cx).file, &super::protected(), &|p| p.is_dir());
    // once per rule and session: the folder is gone, so nothing is watched and nothing would say so
    static TOLD: Mutex<Vec<u32>> = Mutex::new(Vec::new());
    for (id, name) in missing {
        if !TOLD.lock().unwrap().contains(&id) {
            TOLD.lock().unwrap().push(id);
            let msg = format!("«{name}»: {}", t!("Klasör bulunamadı ya da açılamadı; kural kendiliğinden çalışmıyor. Klasör yerine gelince kuralı kaydedin ya da rustyfiles'ı yeniden başlatın."));
            log::warn!("organizer: {msg}");
            crate::status_center::start(cx, t!("Dosya düzenleyici").to_string(), move |_| anyhow::bail!("{msg}"));
        }
    }
    if !cx.has_global::<Auto>() {
        if wants.is_empty() {
            return;
        }
        cx.set_global(Auto::default());
    }
    if cx.global::<Auto>().wants == wants {
        return;
    }
    // the receiving task starts with the first watcher and stays (an idle await costs nothing)
    if cx.global::<Auto>().tx.is_none() && !wants.is_empty() {
        let (tx, mut rx) = unbounded::<Msg>();
        cx.global_mut::<Auto>().tx = Some(tx);
        cx.spawn(async move |cx| {
            while let Some(msg) = rx.next().await {
                cx.update(|cx| on_msg(msg, cx));
            }
        })
        .detach();
    }
    let tx = cx.global::<Auto>().tx.clone();
    let mut watches = Vec::new();
    for w in &wants {
        let Some(tx) = tx.clone() else { break };
        let tx = Mutex::new(tx);
        let (key, folder) = (w.key.clone(), w.folder.clone());
        let send = move |paths: Vec<PathBuf>, rescan: bool| {
            let _ = tx.lock().unwrap().unbounded_send(Msg { key: key.clone(), paths, rescan, attempt: 0 });
        };
        if w.tree {
            match rf_win::tree_watch::watch_tree(&w.folder, SETTLE, move |c| send(c.paths, c.rescan)) {
                Ok(t) => watches.push(Watch { _dir: None, _tree: Some(t) }),
                Err(e) => log::warn!("organizer: cannot watch {}: {e:#}", w.folder.display()),
            }
        } else {
            match rf_win::watch::watch_dir_changes(&w.folder, SETTLE, move |c| send(c.names.iter().map(|n| folder.join(n)).collect(), c.rescan)) {
                Ok(d) => watches.push(Watch { _dir: Some(d), _tree: None }),
                Err(e) => log::warn!("organizer: cannot watch {}: {e:#}", w.folder.display()),
            }
        }
    }
    cx.update_global::<Auto, _>(|a, _| {
        a.wants = wants;
        a.watches = watches;
        a.pending.retain(|k, _| a.wants.iter().any(|w| &w.key == k));
    });
}

fn on_msg(msg: Msg, cx: &mut App) {
    if !cx.has_global::<Auto>() {
        return;
    }
    let key = msg.key.clone();
    let a = cx.global_mut::<Auto>();
    if !a.wants.iter().any(|w| w.key == key) {
        return;
    }
    a.pending.entry(key.clone()).or_default().merge(msg);
    if !a.running.contains(&key) {
        kick(&key, cx);
    }
}

/// Plans what is waiting in this folder (off the UI thread), then runs it.
fn kick(key: &str, cx: &mut App) {
    let a = cx.global_mut::<Auto>();
    let Some(want) = a.wants.iter().find(|w| w.key == key).cloned() else { return };
    let Some(p) = a.pending.remove(key).filter(|p| !p.is_empty()) else { return };
    a.running.insert(key.to_string());
    a.produced.retain(|(at, _)| at.elapsed() < PRODUCED_FOR);
    let skip: HashSet<String> = a.produced.iter().map(|(_, n)| n.clone()).collect();
    let rules: Vec<Rule> = super::rules_of_folder(&super::state(cx).file, &want.folder);
    let source = if p.rescan { Source::Scan { root: want.folder.clone(), subfolders: want.tree } } else { Source::Paths(p.paths.iter().cloned().collect()) };
    let attempt = p.attempt;
    let protected = super::protected();
    let task = cx.background_spawn({
        let rules = rules.clone();
        async move { super::make_plan(&rules, &source, &skip, &protected, &|| false) }
    });
    let key = key.to_string();
    cx.spawn(async move |cx| {
        let plan = task.await;
        cx.update(|cx| after_plan(key, want, rules, plan, attempt, cx));
    })
    .detach();
}

fn after_plan(key: String, want: Want, rules: Vec<Rule>, plan: rf_core::organize::Plan, attempt: u8, cx: &mut App) {
    // a rule the user did not set to run by itself only takes part in deciding who wins
    let auto_ids: HashSet<u32> = rules.iter().filter(|r| r.auto && !r.needs_manual_run()).map(|r| r.id).collect();
    let mut steps = plan.steps;
    steps.retain(|s| auto_ids.contains(&s.rule_id));
    // a rule that cannot work is said once per session, in the Status Center
    let mut complaints: Vec<(u32, String)> = Vec::new();
    for (id, name) in &plan.loops {
        if auto_ids.contains(id) {
            complaints.push((*id, format!("«{name}»: {}", t!("kural kendi sonucunu yeniden yakalıyor; çalıştırılmadı. Koşulu daraltın ya da dosyaları başka bir klasöre gönderin."))));
        }
    }
    for (id, name, why) in &plan.unusable {
        if auto_ids.contains(id) {
            let why = match why {
                rf_core::organize::Unusable::Problem(p) => crate::i18n::td(p.text()),
                rf_core::organize::Unusable::Refused(r) => crate::i18n::td(r.text()),
            };
            complaints.push((*id, format!("«{name}»: {why}")));
        }
    }
    {
        let a = cx.global_mut::<Auto>();
        complaints.retain(|(id, _)| a.warned.insert(*id));
        let now = Instant::now();
        for s in &steps {
            if let Dest::Move(d) = &s.dest {
                a.produced.push_back((now, norm(d)));
            }
        }
    }
    for (_, msg) in complaints {
        log::warn!("organizer: {msg}");
        crate::status_center::start(cx, t!("Dosya düzenleyici").to_string(), move |_| anyhow::bail!("{msg}"));
    }
    if steps.is_empty() {
        done(&key, cx);
        return;
    }
    let label = rf_core::history::display_name(&want.folder);
    let key2 = key.clone();
    super::start_run(
        RunSpec {
            steps,
            automatic: true,
            label,
            on_finished: Some(Box::new(move |rep, cx| {
                let later = rep.retry_later();
                if !later.is_empty() && attempt < ATTEMPTS {
                    retry_later(key2.clone(), later, false, RETRY_AFTER, attempt + 1, cx);
                }
                // more steps than one run does: the folder is looked at again for the rest (only while
                // runs get somewhere, so files that cannot move do not make it go round for ever)
                if rep.deferred_by_cap > 0 && rep.record.count() > 0 {
                    retry_later(key2.clone(), Vec::new(), true, CONTINUE_AFTER, 0, cx);
                }
                done(&key2, cx);
            })),
        },
        cx,
    );
}

/// The run is over: whatever arrived meanwhile is looked at next.
fn done(key: &str, cx: &mut App) {
    if !cx.has_global::<Auto>() {
        return;
    }
    let a = cx.global_mut::<Auto>();
    a.running.remove(key);
    if a.pending.get(key).is_some_and(|p| !p.is_empty()) {
        kick(key, cx);
    }
}

/// A look at these files (or, with `rescan`, the whole folder) again after a while: files that were in
/// use or changed a moment ago, or what a capped run left.
fn retry_later(key: String, paths: Vec<PathBuf>, rescan: bool, after: Duration, attempt: u8, cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(after).await;
        cx.update(|cx| on_msg(Msg { key, paths, rescan, attempt }, cx));
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // (not `use super::*`: gpui's own `test` attribute would come with it)
    use super::{Msg, Pending, wanted, wanted_and_missing};
    use rf_core::organize::store::RulesFile;
    use rf_core::organize::{Action, Condition, ProtectedPaths, Rule};
    use std::path::PathBuf;

    fn rule(id: u32, folder: &str, auto: bool, sub: bool) -> Rule {
        Rule {
            id,
            name: format!("r{id}"),
            folder: PathBuf::from(folder),
            subfolders: sub,
            auto,
            conditions: vec![Condition::Extension { list: vec!["jpg".into()] }],
            action: Action::MoveTo { folder: PathBuf::from(r"D:\Resimler") },
            ..Rule::default()
        }
    }

    fn file(auto_enabled: bool, rules: Vec<Rule>) -> RulesFile {
        let mut f = RulesFile { auto_enabled, ..RulesFile::default() };
        for r in rules {
            f.add(r);
        }
        f
    }

    fn prot() -> ProtectedPaths {
        ProtectedPaths { user_profile: Some(PathBuf::from(r"C:\Users\Ali")), windows: Some(PathBuf::from(r"C:\Windows")), ..ProtectedPaths::default() }
    }

    #[test]
    fn nothing_is_wanted_unless_the_master_switch_and_an_automatic_rule_say_so() {
        let yes = |_: &std::path::Path| true;
        let rules = vec![rule(1, r"C:\Users\Ali\Downloads", true, false)];
        assert!(wanted(&file(false, rules.clone()), &prot(), &yes).is_empty(), "master switch off");
        assert_eq!(wanted(&file(true, rules), &prot(), &yes).len(), 1);
        assert!(wanted(&file(true, vec![rule(1, r"C:\Users\Ali\Downloads", false, false)]), &prot(), &yes).is_empty(), "no rule runs by itself");
        assert!(wanted(&file(true, vec![]), &prot(), &yes).is_empty());
        let mut off = rule(1, r"C:\Users\Ali\Downloads", true, false);
        off.enabled = false;
        assert!(wanted(&file(true, vec![off]), &prot(), &yes).is_empty(), "a switched-off rule does not watch");
    }

    #[test]
    fn rules_that_cannot_work_do_not_watch() {
        let yes = |_: &std::path::Path| true;
        let f = file(
            true,
            vec![rule(1, r"C:\", true, false), rule(2, r"C:\Windows\Temp", true, false), rule(3, r"C:\Users\Ali", true, false), rule(4, "Downloads", true, false), rule(5, r"C:\Users\Ali\Desktop", true, false)],
        );
        let w = wanted(&f, &prot(), &yes);
        assert_eq!(w.iter().map(|w| w.folder.to_string_lossy().into_owned()).collect::<Vec<_>>(), [r"C:\Users\Ali\Desktop"]);
        // a folder that is not there cannot be watched
        let no = |_: &std::path::Path| false;
        assert!(wanted(&f, &prot(), &no).is_empty());
        // an invalid rule (no destination)
        let mut bad = rule(1, r"C:\Users\Ali\Downloads", true, false);
        bad.action = Action::MoveTo { folder: PathBuf::new() };
        assert!(wanted(&file(true, vec![bad]), &prot(), &yes).is_empty());
    }

    #[test]
    fn one_watcher_per_folder_and_a_tree_when_any_rule_wants_subfolders() {
        let yes = |_: &std::path::Path| true;
        let f = file(
            true,
            vec![
                rule(1, r"C:\Users\Ali\Downloads", true, false),
                rule(2, r"c:\users\ali\downloads\", true, true),
                rule(3, r"D:\Gelen", true, false),
                rule(4, r"D:\Gelen", false, true),
            ],
        );
        let w = wanted(&f, &prot(), &yes);
        assert_eq!(w.len(), 2);
        assert_eq!(w.iter().map(|w| (w.key.as_str(), w.tree)).collect::<Vec<_>>(), [(r"c:\users\ali\downloads", true), (r"d:\gelen", false)]);
    }

    #[test]
    fn a_rule_that_looks_back_in_time_never_watches() {
        use rf_core::organize::{Cmp, TimeField};
        let yes = |_: &std::path::Path| true;
        let age = |cmp| Condition::Age { field: TimeField::Modified, cmp, days: 30 };
        let mut old = rule(1, r"C:\Users\Ali\Downloads", true, false);
        old.conditions = vec![age(Cmp::Greater)];
        // files unpacked or copied in keep their old dates: the watcher must not take them for old files
        assert!(wanted(&file(true, vec![old.clone()]), &prot(), &yes).is_empty());
        let mut fresh = old;
        fresh.conditions = vec![age(Cmp::Less)];
        assert_eq!(wanted(&file(true, vec![fresh]), &prot(), &yes).len(), 1);
    }

    #[test]
    fn a_folder_that_is_gone_is_reported_not_silently_dropped() {
        let no = |_: &std::path::Path| false;
        let f = file(true, vec![rule(7, r"E:\Gelen", true, false), rule(8, r"C:\Windows\Temp", true, false)]);
        let (wants, missing) = wanted_and_missing(&f, &prot(), &no);
        assert!(wants.is_empty());
        // (the protected folder is refused for its own reason, not reported as missing)
        assert_eq!(missing, [(f.rules[0].id, "r7".to_string())]);
        // the master switch off: nothing is said either
        let off = file(false, vec![rule(7, r"E:\Gelen", true, false)]);
        assert_eq!(wanted_and_missing(&off, &prot(), &no), (vec![], vec![]));
    }

    #[test]
    fn pending_files_are_merged() {
        let mut p = Pending::default();
        assert!(p.is_empty());
        p.merge(Msg { key: "k".into(), paths: vec![PathBuf::from("a"), PathBuf::from("b")], rescan: false, attempt: 2 });
        p.merge(Msg { key: "k".into(), paths: vec![PathBuf::from("b"), PathBuf::from("c")], rescan: false, attempt: 0 });
        assert_eq!(p.paths.len(), 3);
        assert_eq!(p.attempt, 2);
        assert!(!p.rescan);
        p.merge(Msg { key: "k".into(), paths: vec![], rescan: true, attempt: 0 });
        assert!(p.rescan && !p.is_empty());
    }
}
