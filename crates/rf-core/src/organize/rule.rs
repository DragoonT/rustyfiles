//! The rule: what it watches, which files it matches, what it does with them.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::entry::{contains_ignore_case, fold_case, matches_patterns};
use crate::rename;

pub const DAY_MS: i64 = 86_400_000;

/// The variables a destination or rename template may use.
pub const TEMPLATE_VARS: [&str; 6] = ["name", "ext", "kind", "year", "month", "day"];

/// The coarse kind of a file. The caller decides it from the extension (the app uses its own
/// file categories, so the organizer agrees with the icons).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileKind {
    Image,
    Video,
    Audio,
    Document,
    Archive,
    Program,
    Other,
}

impl FileKind {
    pub const ALL: [FileKind; 7] = [FileKind::Image, FileKind::Video, FileKind::Audio, FileKind::Document, FileKind::Archive, FileKind::Program, FileKind::Other];

    pub fn index(self) -> usize {
        self as usize
    }

    /// The folder name `{kind}` expands to when the caller gives no translated names.
    pub fn english_label(self) -> &'static str {
        match self {
            FileKind::Image => "Images",
            FileKind::Video => "Videos",
            FileKind::Audio => "Audio",
            FileKind::Document => "Documents",
            FileKind::Archive => "Archives",
            FileKind::Program => "Programs",
            FileKind::Other => "Other",
        }
    }
}

/// Do all the conditions have to hold, or any one of them?
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Join {
    #[default]
    All,
    Any,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameOp {
    Contains,
    StartsWith,
    EndsWith,
    /// `*` any run, `?` one character; several patterns may be separated by `;`.
    Wildcard,
}

/// "More than" / "less than" for sizes and ages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cmp {
    Greater,
    Less,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeField {
    Modified,
    Created,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// On the whole file name, case-insensitive (Turkish i / ı / İ / I are one letter here).
    Name { op: NameOp, text: String },
    /// Extensions without the dot ("jpg", "png").
    Extension { list: Vec<String> },
    Kind { kinds: Vec<FileKind> },
    /// Bytes.
    Size { cmp: Cmp, bytes: u64 },
    /// The time field is more / less than `days` days ago.
    Age { field: TimeField, cmp: Cmp, days: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Into this folder (a relative one is taken from the watched folder).
    MoveTo { folder: PathBuf },
    /// Into a folder built from a template: `Pictures\{year}\{month}`.
    MoveToTemplate { template: String },
    /// A new file name from a template: `{year}-{month}-{day} {name}.{ext}`.
    Rename { template: String },
    /// To the Recycle Bin (never a permanent delete).
    Recycle,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    /// Stable inside the rules file; the rule's name may change.
    pub id: u32,
    pub name: String,
    pub enabled: bool,
    pub folder: PathBuf,
    /// Also the files in the folders below `folder` (off by default).
    pub subfolders: bool,
    pub join: Join,
    pub conditions: Vec<Condition>,
    pub action: Action,
    /// Run by itself when files arrive (off by default; needs the master switch too).
    pub auto: bool,
}

impl Default for Rule {
    fn default() -> Self {
        Self {
            id: 0,
            name: String::new(),
            enabled: true,
            folder: PathBuf::new(),
            subfolders: false,
            join: Join::All,
            conditions: Vec::new(),
            action: Action::Recycle,
            auto: false,
        }
    }
}

/// What a file looks like to the rules. The caller collects these (one `stat` per file); the
/// engine never touches the disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFact {
    pub path: PathBuf,
    /// The last component of `path`.
    pub name: String,
    pub size: u64,
    /// Unix milliseconds; 0 = unknown.
    pub modified_ms: i64,
    pub created_ms: i64,
    pub kind: FileKind,
    /// Hidden or system: never touched.
    pub hidden: bool,
    /// For a shortcut (.lnk): the folder or file it points to. A shortcut to a rule's own
    /// destination stays where it is.
    pub link_target: Option<PathBuf>,
}

impl FileFact {
    pub fn new(path: impl Into<PathBuf>, size: u64, modified_ms: i64, created_ms: i64, kind: FileKind) -> Self {
        let path = path.into();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self { path, name, size, modified_ms, created_ms, kind, hidden: false, link_target: None }
    }
}

/// The extension of a file name without the dot ("" for none); `.gitignore` and `name.` have
/// none, as in the folder listing.
pub fn ext_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[i + 1..],
        _ => "",
    }
}

/// The name without its extension.
pub fn stem_of(name: &str) -> &str {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => &name[..i],
        _ => name,
    }
}

/// One entry of an extension list as typed: "JPG", ".jpg", "*.jpg" all mean "jpg".
pub fn clean_ext(raw: &str) -> String {
    fold_case(raw.trim().trim_start_matches('*').trim_start_matches('.'))
}

/// An extension list typed in one box: "jpg, png; .gif" -> ["jpg", "png", "gif"].
pub fn parse_ext_list(text: &str) -> Vec<String> {
    text.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).map(clean_ext).filter(|e| !e.is_empty()).collect()
}

impl Condition {
    /// Is the condition left empty (nothing to match against)? Such a condition never matches.
    pub fn is_empty(&self) -> bool {
        match self {
            Condition::Name { text, .. } => text.trim().is_empty(),
            Condition::Extension { list } => list.iter().all(|e| clean_ext(e).is_empty()),
            Condition::Kind { kinds } => kinds.is_empty(),
            Condition::Size { .. } | Condition::Age { .. } => false,
        }
    }

    pub fn matches(&self, f: &FileFact, now_ms: i64) -> bool {
        if self.is_empty() {
            return false;
        }
        match self {
            Condition::Name { op, text } => {
                let text = text.trim();
                match op {
                    NameOp::Contains => contains_ignore_case(&f.name, text),
                    NameOp::StartsWith => fold_case(&f.name).starts_with(&fold_case(text)),
                    NameOp::EndsWith => fold_case(&f.name).ends_with(&fold_case(text)),
                    NameOp::Wildcard => matches_patterns(text, &f.name),
                }
            }
            Condition::Extension { list } => {
                let ext = fold_case(ext_of(&f.name));
                !ext.is_empty() && list.iter().any(|e| clean_ext(e) == ext)
            }
            Condition::Kind { kinds } => kinds.contains(&f.kind),
            Condition::Size { cmp, bytes } => match cmp {
                Cmp::Greater => f.size > *bytes,
                Cmp::Less => f.size < *bytes,
            },
            Condition::Age { field, cmp, days } => {
                let t = match field {
                    TimeField::Modified => f.modified_ms,
                    TimeField::Created => f.created_ms,
                };
                if t <= 0 {
                    return false;
                }
                let age = now_ms - t;
                let limit = *days as i64 * DAY_MS;
                match cmp {
                    Cmp::Greater => age > limit,
                    Cmp::Less => age < limit,
                }
            }
        }
    }
}

/// Why a rule cannot be used as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleProblem {
    NoName,
    FolderNotAbsolute,
    /// The condition (0-based) has nothing to compare with.
    EmptyCondition(usize),
    NoDestination,
    UnknownVariable(String),
    BadTemplate,
    /// A rename template is a file name: no folders in it.
    RenameNeedsName,
    /// "To the Recycle Bin" for every file of a folder is never what anyone means.
    RecycleNeedsCondition,
}

impl RuleProblem {
    /// The message (a Turkish text; the app translates it).
    pub fn text(&self) -> &'static str {
        match self {
            RuleProblem::NoName => T!("Kurala bir ad verin"),
            RuleProblem::FolderNotAbsolute => T!("İzlenecek klasörü seçin"),
            RuleProblem::EmptyCondition(_) => T!("Boş bir koşul var: değerini yazın ya da koşulu kaldırın"),
            RuleProblem::NoDestination => T!("Hedefi belirtin"),
            RuleProblem::UnknownVariable(_) => T!("Bilinmeyen değişken"),
            RuleProblem::BadTemplate => T!("Hedef şablonu geçersiz: yasak karakter ya da \"..\" var"),
            RuleProblem::RenameNeedsName => T!("Yeni ad şablonu bir dosya adı olmalı: klasör ayracı içeremez"),
            RuleProblem::RecycleNeedsCondition => T!("Geri Dönüşüm Kutusu'na göndermek için en az bir koşul ekleyin"),
        }
    }
}

/// Characters a file name cannot hold become `_` (values that go into a template).
pub fn clean_value(s: &str) -> String {
    s.chars().map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || (c as u32) < 32 { '_' } else { c }).collect()
}

/// Windows drops trailing dots and spaces from a name; so do we, before checking it.
pub fn trim_name(s: &str) -> &str {
    s.trim_end_matches(['.', ' '])
}

/// Checks a template without a file: every variable known, no stray braces, and what comes
/// out is a valid file name (`path == false`) or folder path (`path == true`).
pub fn check_template(template: &str, path: bool) -> Result<(), RuleProblem> {
    let mut unknown: Option<String> = None;
    let out = rename::expand_template(template, |tok| {
        if TEMPLATE_VARS.contains(&tok) {
            Some("x".to_string())
        } else {
            unknown.get_or_insert_with(|| tok.to_string());
            None
        }
    });
    if let Some(u) = unknown {
        return Err(RuleProblem::UnknownVariable(u));
    }
    if out.contains('{') || out.contains('}') || out.trim().is_empty() {
        return Err(RuleProblem::BadTemplate);
    }
    if !path {
        if out.contains(['\\', '/']) {
            return Err(RuleProblem::RenameNeedsName);
        }
        return rename::validate(trim_name(&out)).map_err(|_| RuleProblem::BadTemplate);
    }
    for c in Path::new(&out).components() {
        match c {
            Component::ParentDir => return Err(RuleProblem::BadTemplate),
            Component::Normal(n) => {
                let n = n.to_string_lossy();
                if rename::validate(trim_name(&n)).is_err() {
                    return Err(RuleProblem::BadTemplate);
                }
            }
            _ => {}
        }
    }
    Ok(())
}

impl Rule {
    /// All conditions (or any, see `join`); a rule without conditions matches every file.
    pub fn matches(&self, f: &FileFact, now_ms: i64) -> bool {
        if self.conditions.is_empty() {
            return true;
        }
        match self.join {
            Join::All => self.conditions.iter().all(|c| c.matches(f, now_ms)),
            Join::Any => self.conditions.iter().any(|c| c.matches(f, now_ms)),
        }
    }

    /// A rule with an "older than N days" condition cannot run by itself: it is the folder
    /// watcher that would judge it, and a file that has just arrived (unpacked from a zip, copied
    /// from another drive, moved in) keeps its old dates, so the rule would take files the user
    /// has only just put there. Such a rule runs by hand only.
    pub fn needs_manual_run(&self) -> bool {
        self.conditions.iter().any(|c| matches!(c, Condition::Age { cmp: Cmp::Greater, .. }))
    }

    pub fn validate(&self) -> Result<(), RuleProblem> {
        if self.name.trim().is_empty() {
            return Err(RuleProblem::NoName);
        }
        if !self.folder.is_absolute() {
            return Err(RuleProblem::FolderNotAbsolute);
        }
        if let Some(i) = self.conditions.iter().position(Condition::is_empty) {
            return Err(RuleProblem::EmptyCondition(i));
        }
        match &self.action {
            Action::MoveTo { folder } => {
                if folder.as_os_str().is_empty() {
                    return Err(RuleProblem::NoDestination);
                }
                if folder.components().any(|c| matches!(c, Component::ParentDir)) {
                    return Err(RuleProblem::BadTemplate);
                }
            }
            Action::MoveToTemplate { template } => {
                if template.trim().is_empty() {
                    return Err(RuleProblem::NoDestination);
                }
                check_template(template, true)?;
            }
            Action::Rename { template } => {
                if template.trim().is_empty() {
                    return Err(RuleProblem::NoDestination);
                }
                check_template(template, false)?;
            }
            Action::Recycle => {
                if self.conditions.is_empty() {
                    return Err(RuleProblem::RecycleNeedsCondition);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fact(name: &str) -> FileFact {
        FileFact::new(PathBuf::from(r"C:\Downloads").join(name), 1000, 0, 0, FileKind::Other)
    }
    fn name(op: NameOp, text: &str) -> Condition {
        Condition::Name { op, text: text.into() }
    }

    #[test]
    fn age_rules_that_look_back_do_not_run_by_themselves() {
        let age = |cmp| Condition::Age { field: TimeField::Modified, cmp, days: 30 };
        let mut r = Rule { conditions: vec![age(Cmp::Greater)], ..Rule::default() };
        assert!(r.needs_manual_run());
        r.conditions = vec![age(Cmp::Less)];
        assert!(!r.needs_manual_run(), "files that are new are what a watcher sees");
        r.conditions = vec![name(NameOp::Contains, "x"), age(Cmp::Greater)];
        assert!(r.needs_manual_run());
        r.conditions = vec![];
        assert!(!r.needs_manual_run());
    }

    #[test]
    fn extension_helpers() {
        assert_eq!(ext_of("a.JPG"), "JPG");
        assert_eq!(ext_of(".gitignore"), "");
        assert_eq!(ext_of("trailing."), "");
        assert_eq!(ext_of("a.tar.gz"), "gz");
        assert_eq!(stem_of("a.tar.gz"), "a.tar");
        assert_eq!(stem_of(".gitignore"), ".gitignore");
        assert_eq!(parse_ext_list("JPG, .png;*.gif  webp"), ["jpg", "png", "gif", "webp"]);
    }

    #[test]
    fn name_conditions_are_case_blind_with_turkish_letters() {
        assert!(name(NameOp::Contains, "istanbul").matches(&fact("İSTANBUL gezi.txt"), 0));
        assert!(name(NameOp::Contains, "ILIK").matches(&fact("ılık hava.txt"), 0));
        assert!(name(NameOp::Contains, "ılık").matches(&fact("ILIK.txt"), 0));
        assert!(name(NameOp::StartsWith, "ımg").matches(&fact("IMG_001.jpg"), 0));
        assert!(name(NameOp::StartsWith, "IMG").matches(&fact("img_001.jpg"), 0));
        assert!(!name(NameOp::StartsWith, "001").matches(&fact("img_001.jpg"), 0));
        assert!(name(NameOp::EndsWith, ".JPG").matches(&fact("a.jpg"), 0));
        assert!(name(NameOp::EndsWith, "şeker.txt").matches(&fact("TATLI ŞEKER.TXT"), 0));
        assert!(!name(NameOp::Contains, "seker").matches(&fact("ŞEKER.txt"), 0));
        assert!(name(NameOp::Wildcard, "rapor-??.pdf").matches(&fact("RAPOR-01.pdf"), 0));
        assert!(name(NameOp::Wildcard, "*.bak;*.old").matches(&fact("x.OLD"), 0));
        assert!(!name(NameOp::Wildcard, "*.bak").matches(&fact("x.baks"), 0));
    }

    #[test]
    fn empty_conditions_never_match() {
        assert!(!name(NameOp::Contains, "  ").matches(&fact("a"), 0));
        assert!(!Condition::Extension { list: vec![] }.matches(&fact("a.txt"), 0));
        assert!(!Condition::Extension { list: vec!["".into(), " . ".into()] }.matches(&fact("a.txt"), 0));
        assert!(!Condition::Kind { kinds: vec![] }.matches(&fact("a"), 0));
    }

    #[test]
    fn extension_condition() {
        let c = Condition::Extension { list: vec!["jpg".into(), ".PNG".into()] };
        assert!(c.matches(&fact("a.JPG"), 0));
        assert!(c.matches(&fact("a.png"), 0));
        assert!(!c.matches(&fact("a.gif"), 0));
        // no extension: never "one of"
        assert!(!c.matches(&fact("jpg"), 0));
        assert!(!c.matches(&fact(".jpg"), 0));
    }

    #[test]
    fn size_and_age() {
        let mut f = fact("a.bin");
        f.size = 5_000_000;
        f.modified_ms = 1_000_000_000_000;
        f.created_ms = 1_000_000_000_000 + 20 * DAY_MS;
        let now = 1_000_000_000_000 + 40 * DAY_MS;
        assert!(Condition::Size { cmp: Cmp::Greater, bytes: 4_000_000 }.matches(&f, now));
        assert!(!Condition::Size { cmp: Cmp::Greater, bytes: 5_000_000 }.matches(&f, now));
        assert!(Condition::Size { cmp: Cmp::Less, bytes: 5_000_001 }.matches(&f, now));
        // modified 40 days ago, created 20
        assert!(Condition::Age { field: TimeField::Modified, cmp: Cmp::Greater, days: 30 }.matches(&f, now));
        assert!(!Condition::Age { field: TimeField::Modified, cmp: Cmp::Less, days: 30 }.matches(&f, now));
        assert!(Condition::Age { field: TimeField::Created, cmp: Cmp::Less, days: 30 }.matches(&f, now));
        assert!(!Condition::Age { field: TimeField::Created, cmp: Cmp::Greater, days: 30 }.matches(&f, now));
        // exactly on the limit is neither more nor less
        assert!(!Condition::Age { field: TimeField::Modified, cmp: Cmp::Greater, days: 40 }.matches(&f, now));
        assert!(!Condition::Age { field: TimeField::Modified, cmp: Cmp::Less, days: 40 }.matches(&f, now));
        // unknown time: no answer is a "no"
        f.modified_ms = 0;
        assert!(!Condition::Age { field: TimeField::Modified, cmp: Cmp::Greater, days: 1 }.matches(&f, now));
        assert!(!Condition::Age { field: TimeField::Modified, cmp: Cmp::Less, days: 1 }.matches(&f, now));
    }

    #[test]
    fn all_and_any() {
        let mut r = Rule {
            conditions: vec![name(NameOp::Contains, "rapor"), Condition::Extension { list: vec!["pdf".into()] }],
            ..Rule::default()
        };
        assert!(r.matches(&fact("rapor.pdf"), 0));
        assert!(!r.matches(&fact("rapor.docx"), 0));
        r.join = Join::Any;
        assert!(r.matches(&fact("rapor.docx"), 0));
        assert!(r.matches(&fact("x.pdf"), 0));
        assert!(!r.matches(&fact("x.txt"), 0));
        r.conditions.clear();
        assert!(r.matches(&fact("x.txt"), 0));
    }

    #[test]
    fn templates_are_checked() {
        assert!(check_template(r"C:\Users\ali\Pictures\{year}\{month}", true).is_ok());
        assert!(check_template(r"Sıralı\{kind}", true).is_ok());
        assert_eq!(check_template(r"C:\x\{yıl}", true), Err(RuleProblem::UnknownVariable("yıl".into())));
        assert_eq!(check_template(r"C:\x\..\y", true), Err(RuleProblem::BadTemplate));
        assert_eq!(check_template(r"C:\x\a|b", true), Err(RuleProblem::BadTemplate));
        // a brace that never closes
        assert_eq!(check_template(r"C:\x\{year", true), Err(RuleProblem::BadTemplate));
        assert_eq!(check_template(r"C:\x\con", true), Err(RuleProblem::BadTemplate));
        assert_eq!(check_template("{year}-{name}.{ext}", false), Ok(()));
        assert_eq!(check_template(r"a\{name}", false), Err(RuleProblem::RenameNeedsName));
        assert_eq!(check_template("a/{name}", false), Err(RuleProblem::RenameNeedsName));
        assert_eq!(check_template("", false), Err(RuleProblem::BadTemplate));
        assert_eq!(check_template("   ", false), Err(RuleProblem::BadTemplate));
        assert_eq!(check_template("nul", false), Err(RuleProblem::BadTemplate));
        // a trailing dot comes from an empty {ext}: fine, it is trimmed
        assert_eq!(check_template("{name}.{ext}", false), Ok(()));
    }

    #[test]
    fn rules_are_validated() {
        let ok = Rule {
            name: "Resimler".into(),
            folder: PathBuf::from(r"C:\Users\ali\Downloads"),
            conditions: vec![Condition::Extension { list: vec!["jpg".into()] }],
            action: Action::MoveTo { folder: PathBuf::from(r"C:\Users\ali\Pictures") },
            ..Rule::default()
        };
        assert_eq!(ok.validate(), Ok(()));
        let mut r = ok.clone();
        r.name = "  ".into();
        assert_eq!(r.validate(), Err(RuleProblem::NoName));
        let mut r = ok.clone();
        r.folder = PathBuf::from("Downloads");
        assert_eq!(r.validate(), Err(RuleProblem::FolderNotAbsolute));
        let mut r = ok.clone();
        r.conditions.push(Condition::Kind { kinds: vec![] });
        assert_eq!(r.validate(), Err(RuleProblem::EmptyCondition(1)));
        let mut r = ok.clone();
        r.action = Action::Recycle;
        r.conditions.clear();
        assert_eq!(r.validate(), Err(RuleProblem::RecycleNeedsCondition));
        let mut r = ok.clone();
        r.action = Action::MoveTo { folder: PathBuf::new() };
        assert_eq!(r.validate(), Err(RuleProblem::NoDestination));
        let mut r = ok;
        r.action = Action::Rename { template: "{foo}".into() };
        assert_eq!(r.validate(), Err(RuleProblem::UnknownVariable("foo".into())));
    }
}
