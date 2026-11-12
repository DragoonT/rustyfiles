//! Folder organizer (Settings -> Düzenleyici): rules that sort the files of a folder.
//!
//! The pure engine is `rf_core::organize` (rules, plans, safety), the hands are
//! `rf_win::organize` (scan, run, undo). This module is the glue and the screens:
//!
//! * `mod.rs`: the rules as a global, saving, running a plan as a Status Center job, undo;
//! * `auto.rs`: the folder watchers of the rules that run by themselves (nothing exists
//!   while no rule is automatic or the master switch is off);
//! * `page.rs`: the Settings page; `editor.rs`: the rule editor; `preview.rs`: the preview of
//!   a run ("Uygula" / "Vazgeç").

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gpui_kit::*;
use rf_core::organize::safety::norm;
use rf_core::organize::store::{LoadOutcome, RulesFile};
use rf_core::organize::{FileFact, FileKind, Plan, PlanContext, ProtectedPaths, Rule, Step};

use crate::t;

mod auto;
mod editor;
pub mod page;
mod preview;

pub use rf_win::organize::Record;

/// What went wrong with the rules file at start-up (shown on the page until the user has seen it).
#[derive(Debug, Clone)]
pub enum Problem {
    /// Renamed aside to `moved_to` (None: that failed too); nothing was loaded.
    Corrupt { moved_to: Option<PathBuf>, reason: String },
    /// Locked or unreadable: left alone, and nothing is saved over it.
    Unreadable(String),
}

pub struct Organizer {
    pub file: RulesFile,
    pub problem: Option<Problem>,
    /// The file could not be read: rules are not written until the app is restarted.
    blocked: bool,
}
impl Global for Organizer {}

fn file_path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("organizer.json"))
}

/// What the start-up load leaves us with: the rules, the problem to tell the user about, and
/// whether saving is switched off for this session.
fn from_outcome(outcome: Option<LoadOutcome>) -> (RulesFile, Option<Problem>, bool) {
    match outcome {
        Some(LoadOutcome::Loaded(f)) => (f, None, false),
        Some(LoadOutcome::Corrupt { moved_to, reason }) => {
            log::warn!("organizer.json is unreadable ({reason}); kept as {moved_to:?}");
            // A damaged file that could not be set aside is still lying where the rules are saved:
            // the next save would overwrite it, and with it whatever could still be recovered by hand.
            let blocked = moved_to.is_none();
            (RulesFile::default(), Some(Problem::Corrupt { moved_to, reason }), blocked)
        }
        Some(LoadOutcome::Unreadable { reason }) => {
            log::warn!("organizer.json cannot be read: {reason}");
            (RulesFile::default(), Some(Problem::Unreadable(reason)), true)
        }
        Some(LoadOutcome::Missing) | None => (RulesFile::default(), None, false),
    }
}

/// Loads the rules once (a small file, read on first use). Nothing else starts here.
pub fn ensure(cx: &mut App) {
    if cx.has_global::<Organizer>() {
        return;
    }
    let (file, problem, blocked) = from_outcome(file_path().map(|p| RulesFile::load(&p)));
    cx.set_global(Organizer { file, problem, blocked });
}

/// The app is up: start the watchers of the automatic rules, if any. Called once from the
/// main process (not from helpers such as the dock or a file dialog).
pub fn init(cx: &mut App) {
    ensure(cx);
    // A rules file that could not be read is said in the Status Center right away: somebody whose rules
    // run by themselves never opens the Settings page and would not learn that they stopped.
    if let Some(pr) = cx.global::<Organizer>().problem.clone() {
        let msg = problem_text(&pr);
        crate::status_center::start(cx, t!("Dosya düzenleyici").to_string(), move |_| anyhow::bail!("{msg}"));
    }
    auto::sync(cx);
}

/// The problem with the rules file, in words.
pub fn problem_text(p: &Problem) -> String {
    match p {
        Problem::Corrupt { moved_to: Some(to), reason } => format!(
            "{} {} ({}) {}",
            t!("Kural dosyası okunamadı; silinmedi, şuraya taşındı:"),
            to.display(),
            reason,
            t!("Kurallar boş bir listeyle başlıyor.")
        ),
        Problem::Corrupt { moved_to: None, reason } => format!("{} ({reason})", t!("Kural dosyası okunamadı ve yan yana alınamadı; kurallar kaydedilmeyecek. rustyfiles'ı yeniden başlatın.")),
        Problem::Unreadable(reason) => format!("{} ({reason})", t!("Kural dosyası şu an okunamıyor; yazmak onu ezebileceğinden kurallar kaydedilmeyecek. rustyfiles'ı yeniden başlatın.")),
    }
}

pub fn state(cx: &mut App) -> &Organizer {
    ensure(cx);
    cx.global::<Organizer>()
}

/// Changes the rules, saves them in the background and brings the watchers in line.
pub fn update(cx: &mut App, f: impl FnOnce(&mut RulesFile)) {
    ensure(cx);
    cx.update_global::<Organizer, _>(|o, _| f(&mut o.file));
    save(cx);
    auto::sync(cx);
    cx.refresh_windows();
}

/// The page has shown the problem: it goes away. Not while rules cannot be saved: that has to
/// stay in sight, or the rules a person adds from then on would be lost at the next start without a sign.
pub fn dismiss_problem(cx: &mut App) {
    ensure(cx);
    cx.update_global::<Organizer, _>(|o, _| {
        if !o.blocked {
            o.problem = None;
        }
    });
    cx.refresh_windows();
}

fn save(cx: &mut App) {
    static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static WRITTEN: Mutex<u64> = Mutex::new(0);
    let o = cx.global::<Organizer>();
    if o.blocked {
        return;
    }
    let snapshot = o.file.clone();
    let mine = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        // saves finish in the order they were asked for: an older one never lands on a newer
        let mut last = WRITTEN.lock().unwrap();
        if *last > mine {
            return;
        }
        *last = mine;
        if let Some(p) = file_path()
            && let Err(e) = snapshot.save(&p)
        {
            log::warn!("organizer.json save: {e}");
        }
    });
}

// ---------------------------------------------------------------- helpers shared by the screens

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// The kind of a file from its (lower-case) extension: the same categories the icons use.
pub fn classify(ext: &str) -> FileKind {
    use crate::icons::Category as C;
    match crate::icons::category_for_ext(ext) {
        C::Image => FileKind::Image,
        C::Video => FileKind::Video,
        C::Audio => FileKind::Audio,
        C::Archive => FileKind::Archive,
        C::Executable => FileKind::Program,
        C::Document | C::Spreadsheet | C::Pdf | C::Text => FileKind::Document,
        C::Folder | C::Code | C::Other => FileKind::Other,
    }
}

/// What `{kind}` becomes in a folder name.
pub fn kind_labels() -> [String; 7] {
    [t!("Resimler"), t!("Videolar"), t!("Sesler"), t!("Belgeler"), t!("Arşivler"), t!("Programlar"), t!("Diğer dosyalar")].map(String::from)
}

/// The Windows folders and the app's own folders the organizer keeps away from.
pub fn protected() -> ProtectedPaths {
    let mut p = ProtectedPaths::from_env();
    for d in [crate::settings::app_dir(), crate::settings::local_data_dir()].into_iter().flatten() {
        if !p.app_dirs.contains(&d) {
            p.app_dirs.push(d);
        }
    }
    // only the program file: running a copy from Downloads must not make Downloads off limits
    if let Ok(exe) = std::env::current_exe() {
        p.app_files.push(exe);
    }
    p
}

/// Why this folder may not be organized: the text of the path is checked, and so is the folder
/// the path really leads to (a junction or symbolic link, a `subst` drive, 8.3 short names such as
/// `C:\PROGRA~1`), which the text alone would not give away.
pub fn refusal(protected: &ProtectedPaths, folder: &Path, subfolders: bool) -> Option<rf_core::organize::Refusal> {
    protected.refuse(folder, subfolders).or_else(|| std::fs::canonicalize(folder).ok().and_then(|real| protected.refuse(&real, subfolders)))
}

pub fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

/// A Turkish text of the engine, in the UI language.
pub fn tr(s: &'static str) -> String {
    crate::i18n::t(s).to_string()
}

/// Where the files are looked for.
pub enum Source {
    /// The whole folder (and the folders below it).
    Scan { root: PathBuf, subfolders: bool },
    /// Only these full paths (what a watcher reported): one `stat` each, whatever the size of the folder.
    Paths(Vec<PathBuf>),
}

/// Collects the facts and makes the plan. Blocking: run it off the UI thread.
pub fn make_plan(rules: &[Rule], source: &Source, skip: &std::collections::HashSet<String>, protected: &ProtectedPaths, cancelled: &dyn Fn() -> bool) -> Plan {
    // A rule whose folder is protected only once the path is resolved is left out here and reported
    // like the others the engine refuses.
    let mut rules: Vec<Rule> = rules.to_vec();
    let mut resolved: Vec<(u32, String, rf_core::organize::Unusable)> = Vec::new();
    for r in rules.iter_mut().filter(|r| r.enabled) {
        if protected.refuse(&r.folder, r.subfolders).is_none()
            && let Some(why) = refusal(protected, &r.folder, r.subfolders)
        {
            resolved.push((r.id, r.name.clone(), rf_core::organize::Unusable::Refused(why)));
            r.enabled = false;
        }
    }
    let mut facts: Vec<FileFact> = match source {
        Source::Scan { root, subfolders } if refusal(protected, root, *subfolders).is_none() => rf_win::organize::scan(root, *subfolders, classify, protected, cancelled),
        Source::Scan { .. } => Vec::new(),
        Source::Paths(paths) => rf_win::organize::facts_for_paths(paths, classify),
    };
    if !skip.is_empty() {
        facts.retain(|f| !skip.contains(&norm(&f.path)));
    }
    let mut plan = plan_facts(&rules, &facts, protected);
    plan.unusable.extend(resolved);
    plan
}

pub fn plan_facts(rules: &[Rule], facts: &[FileFact], protected: &ProtectedPaths) -> Plan {
    let exists = |p: &Path| rf_win::organize::exists(p);
    let mut ctx = PlanContext::new(now_ms(), classify, &exists, protected);
    ctx.kind_labels = kind_labels();
    rf_core::organize::plan(rules, facts, &ctx)
}

/// A new rule for the editor to start from: nothing chosen yet but the folder.
pub fn blank_rule(folder: PathBuf) -> Rule {
    Rule { folder, action: rf_core::organize::Action::MoveTo { folder: PathBuf::new() }, ..Rule::default() }
}

/// The example the empty page offers: images of Downloads go to Pictures\{year}\{month}. It is only
/// a starting point for the editor; nothing is created until the user saves it.
pub fn starter_rule() -> Rule {
    use rf_core::organize::{Action, Condition};
    let pictures = dirs::picture_dir().or_else(|| home().map(|h| h.join("Pictures"))).unwrap_or_default();
    Rule {
        name: t!("İndirilenler: resimler Resimler klasörüne").to_string(),
        folder: dirs::download_dir().or_else(|| home().map(|h| h.join("Downloads"))).unwrap_or_default(),
        conditions: vec![Condition::Kind { kinds: vec![FileKind::Image] }],
        action: Action::MoveToTemplate { template: pictures.join(r"{year}\{month}").to_string_lossy().into_owned() },
        ..Rule::default()
    }
}

/// The enabled rules of one folder (the rules that decide among themselves, first one wins).
pub fn rules_of_folder(file: &RulesFile, dir: &Path) -> Vec<Rule> {
    let d = norm(dir);
    file.rules.iter().filter(|r| r.enabled && norm(&r.folder) == d).cloned().collect()
}

/// Does the folder have rules? (The folder background menu offers "Klasörü düzenle" if so.)
pub fn has_rules_for(dir: &Path, cx: &mut App) -> bool {
    !rules_of_folder(&state(cx).file, dir).is_empty()
}

// ---------------------------------------------------------------- the screens

#[derive(Clone)]
enum Layer {
    Preview(Entity<preview::PreviewView>),
    Editor(Entity<editor::EditorView>),
}

/// The one layer that is open (preview or editor), the window it is in, and what had the focus.
#[derive(Clone)]
struct Host(Option<(AnyWindowHandle, Layer, Option<FocusHandle>)>);
impl Global for Host {}

/// `restore`: what had the focus before the layer was made (the new view takes the focus in its
/// constructor, so it has to be asked first), to give it back when the layer closes.
fn open_layer(layer: Layer, restore: Option<FocusHandle>, window: &mut Window, cx: &mut App) {
    cx.set_global(Host(Some((window.window_handle(), layer, restore))));
    window.refresh();
}

pub fn close(window: &mut Window, cx: &mut App) {
    let Some(Some((_, _, restore))) = cx.try_global::<Host>().map(|h| h.0.clone()) else { return };
    cx.set_global(Host(None));
    if let Some(f) = restore {
        window.focus(&f, cx);
    }
    window.refresh();
}

/// The layer to paint over the window (the workspace puts it last), if one is open in it.
pub fn overlay(window: &Window, cx: &App) -> Option<AnyElement> {
    let (handle, layer, _) = cx.try_global::<Host>()?.0.as_ref()?;
    if *handle != window.window_handle() {
        return None;
    }
    Some(match layer {
        Layer::Preview(e) => e.clone().into_any_element(),
        Layer::Editor(e) => e.clone().into_any_element(),
    })
}

pub fn is_open(window: &Window, cx: &App) -> bool {
    cx.try_global::<Host>().and_then(|h| h.0.as_ref()).is_some_and(|(handle, _, _)| *handle == window.window_handle())
}

/// Opens the rule editor: a new rule (`rule` is its starting point, e.g. the starter) or the
/// rule with this id.
pub fn edit_rule(id: Option<u32>, start: Option<Rule>, window: &mut Window, cx: &mut App) {
    ensure(cx);
    let restore = window.focused(cx);
    let rule = id.and_then(|id| cx.global::<Organizer>().file.rule(id).cloned()).or(start);
    let view = cx.new(|cx| editor::EditorView::new(rule, window, cx));
    open_layer(Layer::Editor(view), restore, window, cx);
}

/// "Klasörü düzenle": the plan for the folder's rules, in a preview.
pub fn organize_folder(dir: PathBuf, window: &mut Window, cx: &mut App) {
    ensure(cx);
    let rules = rules_of_folder(&cx.global::<Organizer>().file, &dir);
    if rules.is_empty() {
        let d = dir.clone();
        // rules that are only switched off are not "no rule": adding another might double one
        let key = norm(&dir);
        let switched_off = cx.global::<Organizer>().file.rules.iter().any(|r| norm(&r.folder) == key);
        let (title, body) = if switched_off {
            (t!("Bu klasörün açık kuralı yok"), t!("Ayarlar'dan bu klasörün kurallarından birini açın ya da yeni bir kural ekleyin."))
        } else {
            (t!("Bu klasör için kural yok"), t!("Dosyalarını kurallara göre düzenlemek için bu klasöre bir kural ekleyin."))
        };
        crate::prompt::show(
            title,
            Some(body.to_string()),
            None,
            vec![
                crate::prompt::Button::new(t!("Kural ekle"), crate::prompt::Kind::Primary, move |_, window, cx| {
                    let starter = blank_rule(d.clone());
                    // (the prompt closes itself after this returns; the editor opens on top of it)
                    window.defer(cx, move |window, cx| edit_rule(None, Some(starter), window, cx));
                    Ok(())
                }),
                crate::prompt::Button::cancel(),
            ],
            window,
            cx,
        );
        return;
    }
    let subfolders = rules.iter().any(|r| r.subfolders);
    let title = rf_core::history::display_name(&dir);
    open_preview(title, rules, Source::Scan { root: dir, subfolders }, window, cx);
}

/// "Şimdi çalıştır" of one rule (even a switched-off one: the click is the user's word).
pub fn run_rule_now(id: u32, window: &mut Window, cx: &mut App) {
    ensure(cx);
    let Some(mut rule) = cx.global::<Organizer>().file.rule(id).cloned() else { return };
    rule.enabled = true;
    let (root, sub, title) = (rule.folder.clone(), rule.subfolders, rule.name.clone());
    open_preview(title, vec![rule], Source::Scan { root, subfolders: sub }, window, cx);
}

fn open_preview(title: String, rules: Vec<Rule>, source: Source, window: &mut Window, cx: &mut App) {
    let restore = window.focused(cx);
    let view = cx.new(|cx| preview::PreviewView::new(title, rules, source, window, cx));
    open_layer(Layer::Preview(view), restore, window, cx);
}

/// "Klasörü düzenle" where there is no folder (This PC, the Recycle Bin, a search, a settings tab).
pub fn not_a_folder(window: &mut Window, cx: &mut App) {
    crate::prompt::show(
        t!("Bu konum bir klasör değil"),
        Some(t!("Dosyalarını kurallara göre düzenlemek için bir klasör açın.").to_string()),
        None,
        vec![crate::prompt::Button::new(t!("Tamam"), crate::prompt::Kind::Primary, |_, _, _| Ok(()))],
        window,
        cx,
    );
}

// ---------------------------------------------------------------- running

/// What is done on the UI thread when a run is over.
pub type Finished = Box<dyn FnOnce(&rf_win::organize::Report, &mut App)>;

pub struct RunSpec {
    pub steps: Vec<Step>,
    /// A run the folder watcher started (waits for files that just changed, at most 500 steps).
    pub automatic: bool,
    /// Said in the Status Center title (the rule or the folder).
    pub label: String,
    /// Called on the UI thread when the run is over.
    pub on_finished: Option<Finished>,
}

/// Runs the steps as a Status Center job. When it ends, the job's row says what happened
/// ("12 dosya düzenlendi") and carries "Geri al"; the whole run is one entry of the undo journal.
pub fn start_run(spec: RunSpec, cx: &mut App) {
    let RunSpec { steps, automatic, label, on_finished } = spec;
    if steps.is_empty() {
        return;
    }
    let result: Arc<Mutex<Option<rf_win::organize::Report>>> = Arc::default();
    let shared = result.clone();
    let id_cell = Rc::new(Cell::new(0u64));
    let id_for_done = id_cell.clone();
    let title = format!("{} {label}", t!("Dosyalar düzenleniyor:"));
    let id = crate::status_center::start_with_id(
        cx,
        title,
        move |t| {
            let recycler = rf_win::organize::ShellRecycler { tracker: t.clone() };
            let opts = if automatic { rf_win::organize::RunOptions::automatic(now_ms()) } else { rf_win::organize::RunOptions::manual(now_ms()) };
            let progress = |done: usize, total: usize, name: &str| (t.on_progress)(total as u32, done as u32, (!name.is_empty()).then(|| name.to_string()));
            let rep = rf_win::organize::run(&steps, &opts, &recycler, &progress, &t.cancel);
            // Only a run the user started goes on the Ctrl+Z stack. One that ran by itself in the
            // background would be undone by the next Ctrl+Z instead of what the user just did, and would
            // wipe the Ctrl+Y history; it has its "Geri al" on its Status Center row.
            if rep.record.count() > 0 && !automatic {
                crate::undo::push(crate::undo::UndoOp::Organized { rec: rep.record.clone() });
            }
            let cancelled = rep.cancelled;
            let failed = rep.failed.clone();
            *shared.lock().unwrap() = Some(rep);
            if let Some(first) = failed.first() {
                for f in &failed {
                    log::warn!("organizer: {}: {}", f.path.display(), f.message);
                }
                let count = plural(failed.len(), t!("1 dosya düzenlenemedi"), t!("{n} dosya düzenlenemedi"));
                anyhow::bail!("{count} · {}: {}", first.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), first.message);
            }
            Ok(!cancelled)
        },
        Some(Box::new(move |_ok, cx| {
            let Some(rep) = result.lock().unwrap().take() else { return };
            finish(id_for_done.get(), &rep, automatic, cx);
            if let Some(f) = on_finished {
                f(&rep, cx);
            }
        })),
    );
    id_cell.set(id);
}

/// "12 dosya düzenlendi · 3 dosya atlandı · 40 dosya sonraki çalıştırmada"
fn result_title(rep: &rf_win::organize::Report) -> String {
    let n = rep.record.count();
    let mut parts = vec![plural(n, t!("1 dosya düzenlendi"), t!("{n} dosya düzenlendi"))];
    let skipped = rep.skipped.iter().filter(|s| !s.why.retry_later()).count();
    if skipped > 0 {
        parts.push(plural(skipped, t!("1 dosya atlandı"), t!("{n} dosya atlandı")));
    }
    let waiting = rep.deferred_by_cap + rep.retry_later().len();
    if waiting > 0 {
        parts.push(plural(waiting, t!("1 dosya sonraki çalıştırmada"), t!("{n} dosya sonraki çalıştırmada")));
    }
    parts.join(" · ")
}

/// "1 file ..." or "{n} files ...": the app's counted texts come as a pair of keys, one for a single
/// item and one with `{n}` (English has a plural form, Turkish does not).
pub fn plural(n: usize, one: impl AsRef<str>, many: impl AsRef<str>) -> String {
    if n == 1 { one.as_ref().to_string() } else { many.as_ref().replace("{n}", &n.to_string()) }
}

fn finish(id: u64, rep: &rf_win::organize::Report, automatic: bool, cx: &mut App) {
    // a run by itself that did nothing (every file was too new, say) leaves no row
    if automatic && rep.record.count() == 0 && rep.failed.is_empty() {
        crate::status_center::remove(cx, id);
        return;
    }
    let title = result_title(rep);
    // (the row keeps the run's record itself: the button works whatever has happened to the Ctrl+Z stack)
    let rec = rep.record.clone();
    let action: Option<crate::status_center::ResultAction> = (rec.count() > 0).then(|| -> crate::status_center::ResultAction {
        let title = title.clone();
        let run: Rc<dyn Fn(&mut App)> = Rc::new({
            let rec = rec.clone();
            move |cx: &mut App| undo_from_row(rec.clone(), id, title.clone(), cx)
        });
        (t!("Geri al").to_string(), run)
    });
    crate::status_center::set_result(cx, id, Some(title), action);
    // Failures and skipped files of a run the user started are listed (a run by itself leaves them in
    // the row and the log: nobody is looking).
    if !automatic && (!rep.failed.is_empty() || !rep.skipped.is_empty()) {
        let list: Vec<String> = rep.failed.iter().take(12).map(|f| format!("{}: {}", f.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), crate::i18n::td(&f.message))).collect();
        let more = rep.failed.len().saturating_sub(12);
        let mut text = list.join("\n");
        if more > 0 {
            text.push_str(&format!("\n+{more}"));
        }
        for line in skipped_lines(&rep.skipped) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&line);
        }
        let title = if rep.failed.is_empty() { t!("Bazı dosyalar atlandı") } else { t!("Bazı dosyalar düzenlenemedi") };
        if let Some(w) = cx.active_window() {
            let _ = w.update(cx, |_, window, cx| {
                crate::prompt::show(title, Some(text), None, vec![crate::prompt::Button::new(t!("Tamam"), crate::prompt::Kind::Primary, |_, _, _| Ok(()))], window, cx);
            });
        }
    }
}

/// The skipped files of a run, one line per reason with the first names: "Başka bir uygulama kullanıyor: a.zip, b.zip +3".
pub fn skipped_lines(skipped: &[rf_win::organize::Skip]) -> Vec<String> {
    let mut groups: Vec<(rf_win::organize::Why, Vec<String>)> = Vec::new();
    for s in skipped {
        let name = s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match groups.iter_mut().find(|(w, _)| *w == s.why) {
            Some((_, names)) => names.push(name),
            None => groups.push((s.why, vec![name])),
        }
    }
    groups
        .into_iter()
        .map(|(why, names)| {
            let mut shown = names.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
            if names.len() > 3 {
                shown.push_str(&format!(" +{}", names.len() - 3));
            }
            format!("{}: {shown}", crate::i18n::td(why.text()))
        })
        .collect()
}

// ---------------------------------------------------------------- undo and redo of a run

/// Tokens of the runs that are undone right now (and not redone since): a second undo of the same
/// run, from the row after Ctrl+Z or the other way round, must not run.
static UNDONE: Mutex<Vec<u64>> = Mutex::new(Vec::new());

fn was_undone(token: u64) -> bool {
    UNDONE.lock().unwrap().contains(&token)
}

/// Marks a run undone; false when it already was.
fn mark_undone(token: u64) -> bool {
    let mut u = UNDONE.lock().unwrap();
    if u.contains(&token) {
        return false;
    }
    if u.len() >= 200 {
        u.remove(0);
    }
    u.push(token);
    true
}

/// The "Geri al" button of a finished run's Status Center row. The row's words change only when
/// the undo has really finished, and say so if it did not.
fn undo_from_row(rec: Record, id: u64, title: String, cx: &mut App) {
    if was_undone(rec.token) {
        crate::status_center::set_result(cx, id, Some(format!("{title} ({})", t!("zaten geri alınmıştı"))), None);
        return;
    }
    // (the button goes away at once: a second click cannot start a second undo)
    crate::status_center::set_result(cx, id, Some(format!("{title} ({})", t!("geri alınıyor…"))), None);
    // a run the user started is on the Ctrl+Z stack too: it comes off there, so Ctrl+Z does not undo it again
    let _ = crate::undo::forget_organized(rec.token);
    undo_run(
        rec,
        Some(Box::new(move |ok, cx| {
            let word = if ok { t!("geri alındı") } else { t!("geri alınamadı: ayrıntı Durum Merkezi'nde") };
            crate::status_center::set_result(cx, id, Some(format!("{title} ({word})")), None);
        })),
        cx,
    );
}

/// Undo of a run: the moved files go back, the folders it made go away, the recycled files are
/// restored from the Recycle Bin (the shell does that; a file that is no longer in the bin is
/// reported).
pub fn undo_run(rec: Record, on_done: Option<crate::status_center::OnDone>, cx: &mut App) {
    if !mark_undone(rec.token) {
        return;
    }
    crate::status_center::start_with(cx, t!("Geri alınıyor: dosya düzenleme").to_string(), move |_| {
        let u = rf_win::organize::undo_moves(&rec);
        let mut problems = u.failed.len();
        for f in &u.failed {
            log::warn!("organizer undo: {}: {}", f.path.display(), f.message);
        }
        if !rec.recycled.is_empty() {
            let wanted: Vec<String> = rec.recycled.iter().map(|(_, bin)| bin.to_string_lossy().to_lowercase()).collect();
            let items: Vec<_> = rf_win::recycle::list()?.into_iter().filter(|it| wanted.contains(&it.parsing_path.to_lowercase())).collect();
            if items.is_empty() {
                problems += rec.recycled.len();
            } else {
                problems += rec.recycled.len().saturating_sub(items.len());
                rf_win::recycle::restore(&items)?;
            }
        }
        anyhow::ensure!(problems == 0, "{}", plural(problems, t!("1 dosya geri alınamadı"), t!("{n} dosya geri alınamadı")));
        Ok(true)
    }, on_done);
}

/// Redo of a run (after an undo): the same moves, the same files to the Recycle Bin.
pub fn redo_run(rec: Record, cx: &mut App) {
    UNDONE.lock().unwrap().retain(|t| *t != rec.token);
    crate::status_center::start(cx, t!("Yineleniyor: dosya düzenleme").to_string(), move |tracker| {
        let r = rf_win::organize::redo_moves(&rec);
        let mut problems = r.failed.len();
        let present: Vec<PathBuf> = rec.recycled.iter().map(|(orig, _)| orig.clone()).filter(|p| rf_win::organize::exists(p)).collect();
        problems += rec.recycled.len() - present.len();
        if !present.is_empty() {
            // (`false`: to the Recycle Bin)
            rf_win::fileops::delete_tracked(&present, false, tracker)?;
        }
        anyhow::ensure!(problems == 0, "{}", plural(problems, t!("1 dosya taşınamadı"), t!("{n} dosya taşınamadı")));
        Ok(true)
    });
}

#[cfg(test)]
mod tests;
