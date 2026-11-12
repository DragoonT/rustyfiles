//! Rules + file facts -> a plan. Pure: nothing here reads or writes a file. Whether a name
//! exists is asked through `PlanContext::exists`, so the tests can answer from a set.
//!
//! What a plan guarantees:
//! * the first enabled rule that matches a file decides what happens to it;
//! * nothing is ever overwritten: a name that is taken (on disk, or by an earlier step of the
//!   same plan) gets " (2)", " (3)" ... before the extension;
//! * a file that already is where the rule wants it makes no step;
//! * a rule whose result would be matched by a rule of the same folder again (a loop) makes no
//!   steps at all and is reported in [`Plan::loops`].

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use chrono::{Datelike, FixedOffset, Local};

use super::rule::{Action, FileFact, FileKind, Rule, RuleProblem, clean_value, ext_of, stem_of, trim_name};
use super::safety::{ProtectedPaths, Refusal, is_inside, is_partial_download, norm};
use crate::rename;

/// A path longer than this (in UTF-16 units) is not produced: Explorer, the shell's copy engine
/// and most programs cannot work with it.
pub const MAX_PATH_UNITS: usize = 259;
/// The longest file name NTFS takes (UTF-16 units).
pub const MAX_NAME_UNITS: usize = 255;
/// How far the numbering of a name goes before the file is given up.
const MAX_CLASH: u32 = 9999;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dest {
    /// To this full path (a different folder, a different name, or both).
    Move(PathBuf),
    Recycle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub source: PathBuf,
    pub dest: Dest,
    pub rule_id: u32,
    pub rule_name: String,
    /// The folder the rule watches: the source must be inside it when the step runs.
    pub root: PathBuf,
    pub subfolders: bool,
    /// What the file looked like when it was planned. The runner leaves a file alone when it is
    /// not the same any more (a preview can stay open for minutes; something else may have been
    /// written to that name since).
    pub size: u64,
    pub modified_ms: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// The name the template makes is not a valid Windows name (reserved name, empty...).
    InvalidName,
    PathTooLong,
    /// The destination uses the file's date and the file has none.
    NoDate,
    /// Thousands of files already have the name.
    TooManyClashes,
    /// The rule sends files into a place that is kept free of them (Windows, Program Files,
    /// AppData, the app's own folders).
    ProtectedDest,
}

impl SkipReason {
    pub fn text(self) -> &'static str {
        match self {
            SkipReason::InvalidName => T!("Şablonun ürettiği ad geçerli bir Windows adı değil"),
            SkipReason::PathTooLong => T!("Yeni yol çok uzun"),
            SkipReason::NoDate => T!("Dosyanın tarihi bilinmiyor"),
            SkipReason::TooManyClashes => T!("Aynı adlı çok fazla dosya var"),
            SkipReason::ProtectedDest => T!("Hedef klasör korunuyor (Windows, Program Files, AppData ya da uygulamanın klasörü)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub source: PathBuf,
    pub rule_id: u32,
    pub rule_name: String,
    pub reason: SkipReason,
}

/// A rule that was left out of the run, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unusable {
    Problem(RuleProblem),
    Refused(Refusal),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub steps: Vec<Step>,
    pub skipped: Vec<Skipped>,
    /// Rules (id, name) whose result a rule of the same folder would match again: no steps.
    pub loops: Vec<(u32, String)>,
    /// Enabled rules that cannot be used (id, name, why).
    pub unusable: Vec<(u32, String, Unusable)>,
    /// Matched files that already are where the rule wants them.
    pub in_place: usize,
    /// Files left alone without a word: hidden or system, partial downloads, the app's own
    /// files, shortcuts to a destination.
    pub ignored: usize,
}

pub struct PlanContext<'a> {
    pub now_ms: i64,
    /// The offset from UTC the dates of `{year}` `{month}` `{day}` are taken in; `None` = the
    /// computer's time zone at that moment.
    pub offset_secs: Option<i32>,
    /// Kind from a lower-case extension (to judge the result of a rename).
    pub classify: fn(&str) -> FileKind,
    /// What `{kind}` expands to, by [`FileKind::index`].
    pub kind_labels: [String; 7],
    /// Does a file or folder with this path exist?
    pub exists: &'a dyn Fn(&Path) -> bool,
    pub protected: &'a ProtectedPaths,
}

impl<'a> PlanContext<'a> {
    pub fn new(now_ms: i64, classify: fn(&str) -> FileKind, exists: &'a dyn Fn(&Path) -> bool, protected: &'a ProtectedPaths) -> Self {
        Self { now_ms, offset_secs: None, classify, kind_labels: FileKind::ALL.map(|k| k.english_label().to_string()), exists, protected }
    }
}

fn units(s: &str) -> usize {
    s.encode_utf16().count()
}

fn truncate_units(s: &str, budget: usize) -> &str {
    let mut used = 0;
    for (i, c) in s.char_indices() {
        used += c.len_utf16();
        if used > budget {
            return &s[..i];
        }
    }
    s
}

/// `name` with " (n)" before the extension, shortened to fit a file name if it has to:
/// `foto.jpg`, 2 -> `foto (2).jpg`.
pub fn numbered_name(name: &str, n: u32) -> String {
    let (stem, ext) = (stem_of(name), ext_of(name));
    let suffix = format!(" ({n})");
    let ext_part = if ext.is_empty() { String::new() } else { format!(".{ext}") };
    let budget = MAX_NAME_UNITS.saturating_sub(units(&suffix) + units(&ext_part));
    let stem = trim_name(truncate_units(stem, budget));
    format!("{stem}{suffix}{ext_part}")
}

/// The date parts of `ms` in the wanted time zone.
fn ymd(ms: i64, offset: Option<i32>) -> Option<(i32, u32, u32)> {
    let utc = chrono::DateTime::from_timestamp_millis(ms)?;
    let d = match offset {
        Some(s) => utc.with_timezone(&FixedOffset::east_opt(s)?).date_naive(),
        None => utc.with_timezone(&Local).date_naive(),
    };
    Some((d.year(), d.month(), d.day()))
}

struct Vars<'a> {
    stem: &'a str,
    ext: &'a str,
    kind: &'a str,
    ymd: Option<(i32, u32, u32)>,
}

impl Vars<'_> {
    fn get(&self, tok: &str) -> Option<String> {
        match tok {
            "name" => Some(clean_value(self.stem)),
            "ext" => Some(clean_value(self.ext)),
            "kind" => Some(clean_value(self.kind)),
            "year" => self.ymd.map(|(y, _, _)| format!("{y:04}")),
            "month" => self.ymd.map(|(_, m, _)| format!("{m:02}")),
            "day" => self.ymd.map(|(_, _, d)| format!("{d:02}")),
            _ => None,
        }
    }
}

fn uses_date(template: &str) -> bool {
    let mut used = false;
    rename::expand_template(template, |tok| {
        used |= matches!(tok, "year" | "month" | "day");
        None
    });
    used
}

fn vars_for<'a>(f: &'a FileFact, ctx: &'a PlanContext, template: &str) -> Result<Vars<'a>, SkipReason> {
    let t = if f.modified_ms > 0 { f.modified_ms } else { f.created_ms };
    let date = if t > 0 { ymd(t, ctx.offset_secs) } else { None };
    if date.is_none() && uses_date(template) {
        return Err(SkipReason::NoDate);
    }
    Ok(Vars { stem: stem_of(&f.name), ext: ext_of(&f.name), kind: &ctx.kind_labels[f.kind.index()], ymd: date })
}

fn absolute(folder: &Path, base: &Path) -> PathBuf {
    if folder.is_absolute() { folder.to_path_buf() } else { base.join(folder) }
}

/// The folder a template makes for a file: every part is expanded and checked on its own, so
/// nothing a file name holds can add a folder level.
fn expand_dir(template: &str, rule: &Rule, f: &FileFact, ctx: &PlanContext) -> Result<PathBuf, SkipReason> {
    let vars = vars_for(f, ctx, template)?;
    let mut out = PathBuf::new();
    for c in Path::new(template).components() {
        match c {
            Component::Normal(n) => {
                let seg = rename::expand_template(&n.to_string_lossy(), |t| vars.get(t));
                let seg = trim_name(&seg);
                if rename::validate(seg).is_err() {
                    return Err(SkipReason::InvalidName);
                }
                out.push(seg);
            }
            Component::ParentDir => return Err(SkipReason::InvalidName),
            other => out.push(other.as_os_str()),
        }
    }
    Ok(absolute(&out, &rule.folder))
}

fn expand_name(template: &str, f: &FileFact, ctx: &PlanContext) -> Result<String, SkipReason> {
    let vars = vars_for(f, ctx, template)?;
    let name = rename::expand_template(template, |t| vars.get(t));
    let name = trim_name(&name);
    if rename::validate(name).is_err() || units(name) > MAX_NAME_UNITS {
        return Err(SkipReason::InvalidName);
    }
    Ok(name.to_string())
}

/// Where the rule wants the file, before any name is made unique.
fn wanted(rule: &Rule, f: &FileFact, ctx: &PlanContext) -> Result<Dest, SkipReason> {
    match &rule.action {
        Action::Recycle => Ok(Dest::Recycle),
        Action::MoveTo { folder } => Ok(Dest::Move(absolute(folder, &rule.folder).join(&f.name))),
        Action::MoveToTemplate { template } => Ok(Dest::Move(expand_dir(template, rule, f, ctx)?.join(&f.name))),
        Action::Rename { template } => Ok(Dest::Move(f.path.with_file_name(expand_name(template, f, ctx)?))),
    }
}

/// Where `rule` would put this file (or what it would call it), without looking at the
/// conditions, at names that exist or at loops: the live example of the rule editor.
pub fn preview_dest(rule: &Rule, f: &FileFact, ctx: &PlanContext) -> Result<Dest, SkipReason> {
    let d = wanted(rule, f, ctx)?;
    if let Dest::Move(p) = &d
        && units(&p.to_string_lossy()) > MAX_PATH_UNITS
    {
        return Err(SkipReason::PathTooLong);
    }
    Ok(d)
}

/// The folder a rule moves files into, as far as it is fixed (the part of a template before the
/// first variable). Shortcuts to it are left alone.
fn dest_root(rule: &Rule) -> Option<PathBuf> {
    match &rule.action {
        Action::MoveTo { folder } => Some(absolute(folder, &rule.folder)),
        Action::MoveToTemplate { template } => {
            let mut out = PathBuf::new();
            for c in Path::new(template).components() {
                if matches!(c, Component::Normal(n) if n.to_string_lossy().contains('{')) {
                    break;
                }
                out.push(c.as_os_str());
            }
            (!out.as_os_str().is_empty()).then(|| absolute(&out, &rule.folder))
        }
        Action::Rename { .. } | Action::Recycle => None,
    }
}

struct Usable {
    idx: usize,
    root: String,
}

struct Cand {
    fact: usize,
    rule: usize,
    dest: Result<Dest, SkipReason>,
}

/// Does `rule` see this file: it lies in the rule's folder (or below it, with subfolders on)?
fn covers(u: &Usable, rule: &Rule, parent: &str) -> bool {
    parent == u.root || (rule.subfolders && is_inside(parent, &u.root))
}

pub fn plan(rules: &[Rule], facts: &[FileFact], ctx: &PlanContext) -> Plan {
    let mut out = Plan::default();

    let mut usable: Vec<Usable> = Vec::new();
    for (idx, r) in rules.iter().enumerate() {
        if !r.enabled {
            continue;
        }
        if let Err(p) = r.validate() {
            out.unusable.push((r.id, r.name.clone(), Unusable::Problem(p)));
        } else if let Some(why) = ctx.protected.refuse(&r.folder, r.subfolders) {
            out.unusable.push((r.id, r.name.clone(), Unusable::Refused(why)));
        } else {
            usable.push(Usable { idx, root: norm(&r.folder) });
        }
    }

    // 1. which rule takes which file, and where it wants it
    let mut cands: Vec<Cand> = Vec::new();
    for (fi, f) in facts.iter().enumerate() {
        if f.hidden || is_partial_download(&f.name) || ctx.protected.is_app_file(&f.path) {
            out.ignored += 1;
            continue;
        }
        let parent = f.path.parent().map(norm).unwrap_or_default();
        let Some(u) = usable.iter().find(|u| covers(u, &rules[u.idx], &parent) && rules[u.idx].matches(f, ctx.now_ms)) else { continue };
        let rule = &rules[u.idx];
        if let (Some(target), Some(root)) = (&f.link_target, dest_root(rule))
            && is_inside(&norm(target), &norm(&root))
        {
            out.ignored += 1;
            continue;
        }
        cands.push(Cand { fact: fi, rule: u.idx, dest: wanted(rule, f, ctx) });
    }

    // 2. loops: would a rule of the same folder take the result again?
    let mut looping: HashSet<usize> = HashSet::new();
    for c in &cands {
        let (rule, f) = (&rules[c.rule], &facts[c.fact]);
        let Ok(Dest::Move(dest)) = &c.dest else { continue };
        if norm(dest) == norm(&f.path) {
            continue;
        }
        let root = norm(&rule.folder);
        let dest_dir = dest.parent().map(norm).unwrap_or_default();
        if !is_inside(&dest_dir, &root) {
            continue;
        }
        let mut result = f.clone();
        result.path = dest.clone();
        result.name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        result.kind = (ctx.classify)(&ext_of(&result.name).to_lowercase());
        let again = usable.iter().filter(|x| x.root == root).any(|x| {
            let xr = &rules[x.idx];
            (xr.subfolders || dest_dir == root) && xr.matches(&result, ctx.now_ms)
        });
        if again {
            looping.insert(c.rule);
        }
    }
    let mut looping_sorted: Vec<usize> = looping.iter().copied().collect();
    looping_sorted.sort_unstable();
    for i in looping_sorted {
        out.loops.push((rules[i].id, rules[i].name.clone()));
    }

    // 3. names that are free, in the order of the facts
    let mut taken: HashSet<String> = HashSet::new();
    for c in cands {
        if looping.contains(&c.rule) {
            continue;
        }
        let (rule, f) = (&rules[c.rule], &facts[c.fact]);
        let step = |dest: Dest| Step { source: f.path.clone(), dest, rule_id: rule.id, rule_name: rule.name.clone(), root: rule.folder.clone(), subfolders: rule.subfolders, size: f.size, modified_ms: f.modified_ms };
        let skip = |reason: SkipReason| Skipped { source: f.path.clone(), rule_id: rule.id, rule_name: rule.name.clone(), reason };
        let dest = match c.dest {
            Ok(d) => d,
            Err(reason) => {
                out.skipped.push(skip(reason));
                continue;
            }
        };
        let base = match dest {
            Dest::Recycle => {
                out.steps.push(step(Dest::Recycle));
                continue;
            }
            Dest::Move(p) => p,
        };
        if norm(&base) == norm(&f.path) {
            out.in_place += 1;
            continue;
        }
        let dir = base.parent().map(Path::to_path_buf).unwrap_or_default();
        if ctx.protected.refuse_dest(&dir).is_some() {
            out.skipped.push(skip(SkipReason::ProtectedDest));
            continue;
        }
        let name = base.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let mut dest = base;
        let mut n = 1u32;
        let ok = loop {
            if !taken.contains(&norm(&dest)) && !(ctx.exists)(&dest) {
                break true;
            }
            n += 1;
            if n > MAX_CLASH {
                break false;
            }
            dest = dir.join(numbered_name(&name, n));
        };
        if !ok {
            out.skipped.push(skip(SkipReason::TooManyClashes));
            continue;
        }
        if units(&dest.to_string_lossy()) > MAX_PATH_UNITS {
            out.skipped.push(skip(SkipReason::PathTooLong));
            continue;
        }
        taken.insert(norm(&dest));
        out.steps.push(step(Dest::Move(dest)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organize::rule::{Cmp, Condition, DAY_MS, Join, NameOp};

    const DL: &str = r"C:\Users\Ali\Downloads";
    const PICS: &str = r"C:\Users\Ali\Pictures";

    fn classify(ext: &str) -> FileKind {
        match ext {
            "jpg" | "jpeg" | "png" => FileKind::Image,
            "mp4" | "mkv" => FileKind::Video,
            "pdf" | "docx" | "txt" => FileKind::Document,
            "zip" => FileKind::Archive,
            _ => FileKind::Other,
        }
    }

    fn prot() -> ProtectedPaths {
        ProtectedPaths {
            windows: Some(PathBuf::from(r"C:\Windows")),
            program_files: vec![PathBuf::from(r"C:\Program Files")],
            user_profile: Some(PathBuf::from(r"C:\Users\Ali")),
            app_data: vec![PathBuf::from(r"C:\Users\Ali\AppData")],
            app_dirs: vec![PathBuf::from(r"C:\Users\Ali\AppData\Roaming\rustyfiles")],
            app_files: vec![PathBuf::from(r"C:\Users\Ali\Downloads\rustyfiles.exe")],
        }
    }

    /// 2024-03-07 23:30 UTC.
    const T0: i64 = 1_709_854_200_000;

    fn file(dir: &str, name: &str) -> FileFact {
        let mut f = FileFact::new(PathBuf::from(dir).join(name), 1000, T0, T0, classify(&ext_of(name).to_lowercase()));
        f.link_target = None;
        f
    }

    fn rule(id: u32, name: &str, conditions: Vec<Condition>, action: Action) -> Rule {
        Rule { id, name: name.into(), folder: PathBuf::from(DL), conditions, action, ..Rule::default() }
    }

    fn ext(list: &[&str]) -> Condition {
        Condition::Extension { list: list.iter().map(|s| s.to_string()).collect() }
    }

    fn run_with(rules: &[Rule], facts: &[FileFact], existing: &[&str], offset: Option<i32>) -> Plan {
        let existing: HashSet<String> = existing.iter().map(|p| norm(Path::new(p))).collect();
        let exists = move |p: &Path| existing.contains(&norm(p));
        let protected = prot();
        let mut ctx = PlanContext::new(T0 + 40 * DAY_MS, classify, &exists, &protected);
        ctx.offset_secs = offset.or(Some(0));
        plan(rules, facts, &ctx)
    }

    fn run(rules: &[Rule], facts: &[FileFact], existing: &[&str]) -> Plan {
        run_with(rules, facts, existing, None)
    }

    fn moved(p: &Plan) -> Vec<(String, String)> {
        p.steps
            .iter()
            .map(|s| {
                (
                    s.source.to_string_lossy().into_owned(),
                    match &s.dest {
                        Dest::Move(d) => d.to_string_lossy().into_owned(),
                        Dest::Recycle => "<bin>".into(),
                    },
                )
            })
            .collect()
    }

    fn to_pics() -> Action {
        Action::MoveTo { folder: PathBuf::from(PICS) }
    }

    #[test]
    fn first_matching_enabled_rule_wins() {
        let mut off = rule(1, "kapalı", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from(r"D:\x") });
        off.enabled = false;
        let rules = [
            off,
            rule(2, "resimler", vec![ext(&["jpg", "png"])], to_pics()),
            rule(3, "hepsi", vec![], Action::MoveTo { folder: PathBuf::from(r"D:\Hepsi") }),
        ];
        let p = run(&rules, &[file(DL, "a.JPG"), file(DL, "b.txt")], &[]);
        assert_eq!(
            moved(&p),
            [(format!(r"{DL}\a.JPG"), format!(r"{PICS}\a.JPG")), (format!(r"{DL}\b.txt"), r"D:\Hepsi\b.txt".to_string())]
        );
        assert_eq!(p.steps[0].rule_id, 2);
        assert_eq!(p.steps[1].rule_name, "hepsi");
    }

    #[test]
    fn a_relative_destination_is_taken_from_the_watched_folder() {
        let r = rule(1, "r", vec![ext(&["pdf"])], Action::MoveTo { folder: PathBuf::from("Belgeler") });
        let p = run(&[r], &[file(DL, "a.pdf")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\a.pdf"), format!(r"{DL}\Belgeler\a.pdf"))]);
    }

    #[test]
    fn templates_use_the_modified_date_in_the_chosen_time_zone() {
        let r = rule(1, "r", vec![ext(&["jpg"])], Action::MoveToTemplate { template: format!(r"{PICS}\{{year}}\{{month}}\{{day}}") });
        // 23:30 UTC on the 7th is 02:30 on the 8th in Turkey (+3)
        let p = run_with(std::slice::from_ref(&r), &[file(DL, "a.jpg")], &[], Some(0));
        assert_eq!(moved(&p), [(format!(r"{DL}\a.jpg"), format!(r"{PICS}\2024\03\07\a.jpg"))]);
        let p = run_with(&[r], &[file(DL, "a.jpg")], &[], Some(3 * 3600));
        assert_eq!(moved(&p), [(format!(r"{DL}\a.jpg"), format!(r"{PICS}\2024\03\08\a.jpg"))]);
    }

    #[test]
    fn template_kind_name_and_ext() {
        let mut r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\Arşiv\{kind}\{ext}\{name}".into() });
        r.conditions = vec![ext(&["jpg", "pdf"])];
        let p = run(&[r], &[file(DL, "tatil.jpg"), file(DL, "rapor.pdf")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\tatil.jpg"), r"D:\Arşiv\Images\jpg\tatil\tatil.jpg".to_string()), (format!(r"{DL}\rapor.pdf"), r"D:\Arşiv\Documents\pdf\rapor\rapor.pdf".to_string())]);
        // translated kind names come from the context
        let existing: HashSet<String> = HashSet::new();
        let exists = move |p: &Path| existing.contains(&norm(p));
        let protected = prot();
        let mut ctx = PlanContext::new(0, classify, &exists, &protected);
        ctx.offset_secs = Some(0);
        ctx.kind_labels[FileKind::Image.index()] = "Resimler".into();
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\{kind}".into() });
        let p = plan(&[r], &[file(DL, "a.jpg")], &ctx);
        assert_eq!(moved(&p), [(format!(r"{DL}\a.jpg"), r"D:\Resimler\a.jpg".to_string())]);
    }

    #[test]
    fn rename_by_template() {
        // (the new name must fall out of the rule's reach, or it is a loop: see below)
        let starts = |s: &str| Condition::Name { op: NameOp::StartsWith, text: s.into() };
        let r = rule(1, "r", vec![starts("tatil")], Action::Rename { template: "{year}-{month}-{day} {name}.{ext}".into() });
        let p = run(&[r], &[file(DL, "tatil.JPG")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\tatil.JPG"), format!(r"{DL}\2024-03-07 tatil.JPG"))]);
        // a file in a subfolder is renamed where it is
        let mut r = rule(1, "r", vec![starts("a")], Action::Rename { template: "{year}-{name}.{ext}".into() });
        r.subfolders = true;
        let sub = format!(r"{DL}\tatil");
        let p = run(&[r], &[file(&sub, "a.jpg")], &[]);
        assert_eq!(moved(&p), [(format!(r"{sub}\a.jpg"), format!(r"{sub}\2024-a.jpg"))]);
    }

    #[test]
    fn recycle_is_a_step_without_a_name() {
        let r = rule(1, "eski", vec![Condition::Age { field: crate::organize::rule::TimeField::Modified, cmp: Cmp::Greater, days: 30 }], Action::Recycle);
        let p = run(&[r], &[file(DL, "a.tmpl")], &[]);
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.steps[0].dest, Dest::Recycle);
    }

    #[test]
    fn names_are_made_unique_against_the_disk_and_the_plan() {
        let r = rule(1, "r", vec![ext(&["jpg"])], to_pics());
        let facts = [file(DL, "a.jpg"), file(&format!(r"{DL}\eski"), "a.jpg"), file(DL, "b.jpg"), file(&format!(r"{DL}\eski"), "b.jpg"), file(DL, "c.jpg")];
        let mut r = r;
        r.subfolders = true;
        let existing = [format!(r"{PICS}\a.jpg"), format!(r"{PICS}\a (2).jpg"), format!(r"{PICS}\c (2).jpg")];
        let existing: Vec<&str> = existing.iter().map(String::as_str).collect();
        let p = run(&[r], &facts, &existing);
        let dest: Vec<String> = moved(&p).into_iter().map(|(_, d)| d).collect();
        assert_eq!(
            dest,
            [
                format!(r"{PICS}\a (3).jpg"),
                format!(r"{PICS}\a (4).jpg"),
                format!(r"{PICS}\b.jpg"),
                format!(r"{PICS}\b (2).jpg"),
                format!(r"{PICS}\c.jpg"),
            ]
        );
    }

    #[test]
    fn extensionless_and_dotted_names_are_numbered_sensibly() {
        assert_eq!(numbered_name("foto.jpg", 2), "foto (2).jpg");
        assert_eq!(numbered_name("LICENSE", 3), "LICENSE (3)");
        assert_eq!(numbered_name("a.tar.gz", 2), "a.tar (2).gz");
        assert_eq!(numbered_name(".gitignore", 2), ".gitignore (2)");
        assert_eq!(numbered_name("İlk Ödev.docx", 12), "İlk Ödev (12).docx");
    }

    #[test]
    fn turkish_letters_clash_like_the_file_system_does() {
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(r"D:\Hedef") });
        // "ILIK.txt" and "ılık.txt" and "ilik.txt" are one name for the comparison
        let facts = [file(DL, "ILIK.txt"), file(DL, "ılık.TXT")];
        let p = run(&[r], &facts, &[r"D:\Hedef\ilik.txt"]);
        let dest: Vec<String> = moved(&p).into_iter().map(|(_, d)| d).collect();
        assert_eq!(dest, [r"D:\Hedef\ILIK (2).txt".to_string(), r"D:\Hedef\ılık (3).TXT".to_string()]);
    }

    #[test]
    fn a_file_already_in_place_makes_no_step() {
        let r = rule(1, "r", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from(r"C:\Users\Ali\Downloads") });
        let p = run(&[r], &[file(DL, "a.jpg")], &[&format!(r"{DL}\a.jpg")]);
        assert!(p.steps.is_empty() && p.skipped.is_empty());
        assert_eq!(p.in_place, 1);
        // same place spelled differently
        let r = rule(1, "r", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from("c:/users/ali/DOWNLOADS/") });
        let p = run(&[r], &[file(DL, "a.jpg")], &[&format!(r"{DL}\a.jpg")]);
        assert!(p.steps.is_empty());
        assert_eq!(p.in_place, 1);
    }

    #[test]
    fn files_outside_the_folder_and_in_subfolders() {
        let r = rule(1, "r", vec![], to_pics());
        let facts = [file(DL, "a.jpg"), file(&format!(r"{DL}\alt"), "b.jpg"), file(r"C:\Users\Ali\Documents", "c.jpg"), file(r"C:\Users\Ali\Downloads2", "d.jpg")];
        let p = run(std::slice::from_ref(&r), &facts, &[]);
        assert_eq!(moved(&p).len(), 1, "{:?}", moved(&p));
        let mut sub = r;
        sub.subfolders = true;
        let p = run(&[sub], &facts, &[]);
        assert_eq!(moved(&p).len(), 2);
    }

    #[test]
    fn hidden_partial_and_app_files_are_never_touched() {
        let r = rule(1, "hepsi", vec![], to_pics());
        let mut hidden = file(DL, "desktop.ini");
        hidden.hidden = true;
        let facts = [hidden, file(DL, "film.mp4.crdownload"), file(DL, "x.PART"), file(DL, "~$rapor.docx"), file(DL, "iyi.txt")];
        let p = run(&[r], &facts, &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\iyi.txt"), format!(r"{PICS}\iyi.txt"))]);
        assert_eq!(p.ignored, 4);
        // the app's own folder cannot even be watched
        let mut r = rule(1, "hepsi", vec![], to_pics());
        r.folder = PathBuf::from(r"C:\Users\Ali\AppData\Roaming\rustyfiles");
        let p = run(&[r], &[file(r"C:\Users\Ali\AppData\Roaming\rustyfiles", "settings.toml")], &[]);
        assert!(p.steps.is_empty());
        assert!(matches!(p.unusable[0].2, Unusable::Refused(Refusal::AppData)));
    }

    #[test]
    fn shortcuts_to_the_destination_stay() {
        let r = rule(1, "r", vec![], to_pics());
        let mut link = file(DL, "Resimler.lnk");
        link.link_target = Some(PathBuf::from(r"C:\Users\Ali\Pictures"));
        let mut inner = file(DL, "Yıl.lnk");
        inner.link_target = Some(PathBuf::from(r"c:\users\ali\pictures\2024"));
        let mut other = file(DL, "Baska.lnk");
        other.link_target = Some(PathBuf::from(r"C:\Users\Ali\Music"));
        let p = run(&[r], &[link, inner, other], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\Baska.lnk"), format!(r"{PICS}\Baska.lnk"))]);
        assert_eq!(p.ignored, 2);
        // a template: the fixed part in front of the first variable is the destination
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: format!(r"{PICS}\{{year}}") });
        let mut link = file(DL, "Resimler.lnk");
        link.link_target = Some(PathBuf::from(PICS));
        let p = run(&[r], &[link], &[]);
        assert!(p.steps.is_empty());
    }

    #[test]
    fn a_rename_that_matches_its_own_rule_loops() {
        // IMG_1.jpg -> Foto_IMG_1.jpg still contains "img"
        let r = rule(1, "önek", vec![Condition::Name { op: NameOp::Contains, text: "img".into() }], Action::Rename { template: "Foto_{name}.{ext}".into() });
        let p = run(&[r], &[file(DL, "IMG_1.jpg")], &[]);
        assert!(p.steps.is_empty());
        assert_eq!(p.loops, [(1, "önek".to_string())]);
        // a rename that leaves the rule's reach is fine
        let r = rule(1, "önek", vec![Condition::Name { op: NameOp::StartsWith, text: "img".into() }], Action::Rename { template: "Foto_{name}.{ext}".into() });
        let p = run(&[r], &[file(DL, "IMG_1.jpg")], &[]);
        assert_eq!(p.steps.len(), 1);
        assert!(p.loops.is_empty());
    }

    #[test]
    fn a_destination_inside_the_folder_loops_only_when_it_would_be_seen_again() {
        let inside = Action::MoveTo { folder: PathBuf::from("Resimler") };
        // subfolders off: Downloads\Resimler is not looked at
        let r = rule(1, "r", vec![ext(&["jpg"])], inside.clone());
        let p = run(&[r], &[file(DL, "a.jpg")], &[]);
        assert_eq!(p.steps.len(), 1);
        assert!(p.loops.is_empty());
        // subfolders on: it is, and the rule matches it again
        let mut r = rule(1, "r", vec![ext(&["jpg"])], inside.clone());
        r.subfolders = true;
        let p = run(&[r], &[file(DL, "a.jpg"), file(DL, "b.jpg")], &[]);
        assert!(p.steps.is_empty());
        assert_eq!(p.loops.len(), 1);
        // subfolders on, but the result matches no rule of the folder: allowed (the new name no
        // longer starts with "a")
        let mut r = rule(1, "r", vec![Condition::Name { op: NameOp::StartsWith, text: "a".into() }], Action::Rename { template: "b_{name}.{ext}".into() });
        r.subfolders = true;
        let mut other = rule(2, "başka", vec![ext(&["zip"])], Action::Recycle);
        other.subfolders = true;
        let p = run(&[r.clone(), other], &[file(DL, "a.jpg")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\a.jpg"), format!(r"{DL}\b_a.jpg"))]);
        // a different rule of the same folder that sees the result is a loop too
        let mut other = rule(2, "başka", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from(r"D:\x") });
        other.subfolders = true;
        let p = run(&[r, other], &[file(DL, "a.jpg")], &[]);
        assert_eq!(p.loops, [(1, "r".to_string())]);
        assert!(p.steps.is_empty());
        // a rule that lands outside the folder never loops
        let mut r = rule(1, "r", vec![ext(&["jpg"])], to_pics());
        r.subfolders = true;
        let p = run(&[r], &[file(DL, "a.jpg")], &[]);
        assert_eq!(p.steps.len(), 1);
    }

    #[test]
    fn a_rule_in_another_folder_does_not_count_for_the_loop_check() {
        let r = rule(1, "r", vec![ext(&["jpg"])], to_pics());
        let mut other = rule(2, "resim klasörü", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from(r"D:\x") });
        other.folder = PathBuf::from(PICS);
        other.subfolders = true;
        let p = run(&[r, other], &[file(DL, "a.jpg")], &[]);
        assert_eq!(p.steps.len(), 1);
        assert!(p.loops.is_empty());
    }

    #[test]
    fn very_long_names_keep_their_extension_when_numbered() {
        let long = "ğ".repeat(250);
        let name = format!("{long}.jpg");
        let n2 = numbered_name(&name, 2);
        assert!(n2.encode_utf16().count() <= MAX_NAME_UNITS, "{}", n2.encode_utf16().count());
        assert!(n2.ends_with(" (2).jpg"));
        assert!(n2.starts_with("ğğğ"));
        // through the plan
        let name = format!("{}.jpg", "ğ".repeat(240));
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(r"D:\H") });
        let dest_existing = format!(r"D:\H\{name}");
        let p = run(&[r], &[file(DL, &name)], &[&dest_existing]);
        let Dest::Move(d) = &p.steps[0].dest else { panic!() };
        let n = d.file_name().unwrap().to_string_lossy().into_owned();
        assert!(n.encode_utf16().count() <= MAX_NAME_UNITS && n.ends_with(" (2).jpg"), "{n}");
        // names made of surrogate pairs are cut between characters, never inside one
        let emoji = format!("{}.txt", "😀".repeat(130));
        let n3 = numbered_name(&emoji, 2);
        assert!(n3.encode_utf16().count() <= MAX_NAME_UNITS);
        assert!(n3.ends_with(" (2).txt"));
    }

    #[test]
    fn a_path_that_is_too_long_is_skipped_not_made() {
        let deep = format!(r"D:\{}", "klasör\\".repeat(45));
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(deep) });
        let p = run(&[r], &[file(DL, "a.jpg")], &[]);
        assert!(p.steps.is_empty());
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].reason, SkipReason::PathTooLong);
        // just inside the limit is fine
        let ok_dir = format!(r"D:\{}", "a".repeat(200));
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(ok_dir) });
        assert_eq!(run(&[r], &[file(DL, "a.jpg")], &[]).steps.len(), 1);
    }

    #[test]
    fn trailing_dots_and_spaces_from_templates_are_trimmed() {
        // no extension: "{name}.{ext}" would end with a dot
        let r = rule(1, "r", vec![Condition::Name { op: NameOp::StartsWith, text: "read".into() }], Action::Rename { template: "yeni {name}.{ext}".into() });
        let p = run(&[r], &[file(DL, "README")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\README"), format!(r"{DL}\yeni README"))]);
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\Arşiv\{name}. \x".into() });
        let p = run(&[r], &[file(DL, "tatil.jpg")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\tatil.jpg"), r"D:\Arşiv\tatil\x\tatil.jpg".to_string())]);
        // a name with a trailing dot that already exists stays as it is when only moved
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(r"D:\H") });
        let p = run(&[r], &[file(DL, "garip.")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\garip."), r"D:\H\garip.".to_string())]);
        // values that go into a name never bring a separator or a forbidden character
        let r = rule(1, "r", vec![], Action::Rename { template: "{name}.{ext}".into() });
        assert_eq!(run(&[r], &[file(DL, "a.jpg")], &[]).in_place, 1);
    }

    #[test]
    fn reserved_device_names_are_skipped() {
        // a file called nul.txt (it can exist through a \\?\ path) must not make a folder "nul"
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\{name}".into() });
        let p = run(&[r], &[file(DL, "nul.txt"), file(DL, "ok.txt"), file(DL, "COM1")], &[]);
        assert_eq!(moved(&p), [(format!(r"{DL}\ok.txt"), r"D:\ok\ok.txt".to_string())]);
        assert_eq!(p.skipped.len(), 2);
        assert!(p.skipped.iter().all(|s| s.reason == SkipReason::InvalidName));
        let r = rule(1, "r", vec![ext(&["txt"])], Action::Rename { template: "{ext}".into() });
        // "txt" is fine; "nul" as a template is refused when the rule is checked
        assert!(run(&[r], &[file(DL, "a.txt")], &[]).steps.len() == 1);
        let r = rule(1, "r", vec![], Action::Rename { template: "con".into() });
        let p = run(&[r], &[file(DL, "a.txt")], &[]);
        assert!(p.steps.is_empty());
        assert!(matches!(p.unusable[0].2, Unusable::Problem(RuleProblem::BadTemplate)));
    }

    #[test]
    fn a_file_without_a_date_cannot_go_to_a_dated_folder() {
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\{year}".into() });
        let mut f = file(DL, "a.jpg");
        f.modified_ms = 0;
        f.created_ms = 0;
        let p = run(std::slice::from_ref(&r), &[f.clone()], &[]);
        assert_eq!(p.skipped[0].reason, SkipReason::NoDate);
        // the created time stands in for a missing modified time
        f.created_ms = T0;
        assert_eq!(moved(&run(&[r], &[f], &[])), [(format!(r"{DL}\a.jpg"), r"D:\2024\a.jpg".to_string())]);
    }

    #[test]
    fn unusable_rules_are_reported_and_left_out() {
        let mut bad = rule(1, "adsız dizin", vec![], Action::MoveTo { folder: PathBuf::new() });
        bad.name = "kötü".into();
        let mut root = rule(2, "kök", vec![], to_pics());
        root.folder = PathBuf::from(r"C:\");
        let good = rule(3, "iyi", vec![], to_pics());
        let p = run(&[bad, root, good], &[file(DL, "a.jpg")], &[]);
        assert_eq!(p.steps.len(), 1);
        assert_eq!(p.steps[0].rule_id, 3);
        assert_eq!(p.unusable.len(), 2);
        assert!(matches!(p.unusable[0].2, Unusable::Problem(RuleProblem::NoDestination)));
        assert!(matches!(p.unusable[1].2, Unusable::Refused(Refusal::DriveRoot)));
        // a disabled broken rule is not reported
        let mut off = rule(4, "kapalı", vec![], Action::MoveTo { folder: PathBuf::new() });
        off.enabled = false;
        assert!(run(&[off], &[], &[]).unusable.is_empty());
    }

    #[test]
    fn any_join_and_kind_conditions() {
        let mut r = rule(1, "medya", vec![Condition::Kind { kinds: vec![FileKind::Image, FileKind::Video] }, Condition::Name { op: NameOp::EndsWith, text: ".zip".into() }], to_pics());
        r.join = Join::Any;
        let p = run(&[r], &[file(DL, "a.png"), file(DL, "b.mkv"), file(DL, "c.zip"), file(DL, "d.txt")], &[]);
        assert_eq!(p.steps.len(), 3);
    }

    #[test]
    fn the_live_example_shows_the_destination_without_conditions_or_loops() {
        let exists = |_: &Path| true;
        let protected = prot();
        let mut ctx = PlanContext::new(T0, classify, &exists, &protected);
        ctx.offset_secs = Some(0);
        // conditions that would not match, a rename that would loop, names that exist: none of it matters here
        let r = rule(1, "r", vec![ext(&["zip"])], Action::Rename { template: "{year}-{name}.{ext}".into() });
        assert_eq!(preview_dest(&r, &file(DL, "a.jpg"), &ctx), Ok(Dest::Move(PathBuf::from(format!(r"{DL}\2024-a.jpg")))));
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: format!(r"{PICS}\{{year}}\{{month}}") });
        assert_eq!(preview_dest(&r, &file(DL, "a.jpg"), &ctx), Ok(Dest::Move(PathBuf::from(format!(r"{PICS}\2024\03\a.jpg")))));
        let r = rule(1, "r", vec![], Action::Recycle);
        assert_eq!(preview_dest(&r, &file(DL, "a.jpg"), &ctx), Ok(Dest::Recycle));
        let deep = format!(r"D:\{}", "klasör\\".repeat(45));
        let r = rule(1, "r", vec![], Action::MoveTo { folder: PathBuf::from(deep) });
        assert_eq!(preview_dest(&r, &file(DL, "a.jpg"), &ctx), Err(SkipReason::PathTooLong));
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"D:\{name}".into() });
        assert_eq!(preview_dest(&r, &file(DL, "nul.txt"), &ctx), Err(SkipReason::InvalidName));
    }

    #[test]
    fn files_are_not_sent_into_protected_places() {
        for dest in [r"C:\Windows\Temp", r"C:\Program Files\Foo", r"C:\Users\Ali\AppData\Roaming\Foo", r"C:\Users\Ali\AppData\Roaming\rustyfiles\x"] {
            let r = rule(1, "r", vec![ext(&["jpg"])], Action::MoveTo { folder: PathBuf::from(dest) });
            let p = run(&[r], &[file(DL, "a.jpg")], &[]);
            assert!(p.steps.is_empty(), "{dest}");
            assert_eq!(p.skipped.len(), 1, "{dest}");
            assert_eq!(p.skipped[0].reason, SkipReason::ProtectedDest);
        }
        // a template that lands there is caught too
        let r = rule(1, "r", vec![], Action::MoveToTemplate { template: r"C:\Windows\{year}".into() });
        assert_eq!(run(&[r], &[file(DL, "a.jpg")], &[]).skipped[0].reason, SkipReason::ProtectedDest);
    }

    #[test]
    fn a_step_remembers_how_the_file_looked() {
        let r = rule(1, "r", vec![], to_pics());
        let mut f = file(DL, "a.jpg");
        f.size = 4321;
        let p = run(&[r], &[f], &[]);
        assert_eq!((p.steps[0].size, p.steps[0].modified_ms), (4321, T0));
    }

    #[test]
    fn many_files_stay_cheap() {
        let r = rule(1, "r", vec![ext(&["jpg"])], Action::MoveToTemplate { template: format!(r"{PICS}\{{year}}\{{month}}") });
        let facts: Vec<FileFact> = (0..20_000).map(|i| file(DL, &format!("foto {i}.jpg"))).collect();
        let t = std::time::Instant::now();
        let p = run(&[r], &facts, &[]);
        assert_eq!(p.steps.len(), 20_000);
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "{:?}", t.elapsed());
    }
}
