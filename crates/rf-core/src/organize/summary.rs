//! A rule said in words: "uzantısı jpg, png olan dosyalar → Pictures\{year}".
//!
//! The texts are Turkish literals marked `T!`; the caller gives the translating function (the
//! app's own `t`), so this stays pure and testable with the identity.

use std::path::Path;

use super::rule::{Action, Cmp, Condition, FileKind, Join, NameOp, Rule, TimeField, clean_ext};
use super::safety::{is_inside, norm};
use crate::entry::fold_case;

/// The unit a size is entered in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SizeUnit {
    B,
    Kb,
    Mb,
    Gb,
}

impl SizeUnit {
    pub const ALL: [SizeUnit; 4] = [SizeUnit::B, SizeUnit::Kb, SizeUnit::Mb, SizeUnit::Gb];

    pub fn bytes(self) -> u64 {
        match self {
            SizeUnit::B => 1,
            SizeUnit::Kb => 1024,
            SizeUnit::Mb => 1024 * 1024,
            SizeUnit::Gb => 1024 * 1024 * 1024,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SizeUnit::B => "B",
            SizeUnit::Kb => "KB",
            SizeUnit::Mb => "MB",
            SizeUnit::Gb => "GB",
        }
    }

    /// The biggest unit that divides `bytes` evenly: 5 MB, 1536 KB, 700 B.
    pub fn split(bytes: u64) -> (u64, SizeUnit) {
        for u in [SizeUnit::Gb, SizeUnit::Mb, SizeUnit::Kb] {
            if bytes >= u.bytes() && bytes.is_multiple_of(u.bytes()) {
                return (bytes / u.bytes(), u);
            }
        }
        (bytes, SizeUnit::B)
    }
}

/// "5 MB".
pub fn size_text(bytes: u64) -> String {
    let (n, u) = SizeUnit::split(bytes);
    format!("{n} {}", u.label())
}

/// The kind as a short lower-case word ("resim").
pub fn kind_word(k: FileKind, tr: &dyn Fn(&'static str) -> String) -> String {
    match k {
        FileKind::Image => tr(T!("resim")),
        FileKind::Video => tr(T!("video")),
        FileKind::Audio => tr(T!("ses")),
        FileKind::Document => tr(T!("belge")),
        FileKind::Archive => tr(T!("arşiv")),
        FileKind::Program => tr(T!("program")),
        FileKind::Other => tr(T!("diğer")),
    }
}

/// A path as the user knows it: below the user's folder it is shown without that part.
pub fn show_path(p: &Path, home: Option<&Path>) -> String {
    if let Some(h) = home
        && norm(p) != norm(h)
        && is_inside(&norm(p), &norm(h))
    {
        // part by part, case-blind like the file system (Path::strip_prefix is not)
        let mut rest = p.components();
        let same = h.components().all(|hc| rest.next().is_some_and(|pc| fold_case(&pc.as_os_str().to_string_lossy()) == fold_case(&hc.as_os_str().to_string_lossy())));
        if same {
            return rest.as_path().to_string_lossy().into_owned();
        }
    }
    p.to_string_lossy().into_owned()
}

fn condition_words(c: &Condition, tr: &dyn Fn(&'static str) -> String) -> String {
    let with = |key: String, x: String| key.replace("{x}", &x);
    match c {
        Condition::Name { op, text } => {
            let key = match op {
                NameOp::Contains => tr(T!("adında “{x}” geçen")),
                NameOp::StartsWith => tr(T!("adı “{x}” ile başlayan")),
                NameOp::EndsWith => tr(T!("adı “{x}” ile biten")),
                NameOp::Wildcard => tr(T!("adı “{x}” desenine uyan")),
            };
            with(key, text.trim().to_string())
        }
        Condition::Extension { list } => {
            let exts: Vec<String> = list.iter().map(|e| clean_ext(e)).filter(|e| !e.is_empty()).collect();
            with(tr(T!("uzantısı {x} olan")), exts.join(", "))
        }
        Condition::Kind { kinds } => {
            let words: Vec<String> = kinds.iter().map(|k| kind_word(*k, tr)).collect();
            with(tr(T!("türü {x} olan")), words.join(", "))
        }
        Condition::Size { cmp, bytes } => {
            let key = match cmp {
                Cmp::Greater => tr(T!("boyutu {x}'den büyük olan")),
                Cmp::Less => tr(T!("boyutu {x}'den küçük olan")),
            };
            with(key, size_text(*bytes))
        }
        Condition::Age { field, cmp, days } => {
            let key = match (field, cmp) {
                (TimeField::Modified, Cmp::Greater) => tr(T!("değiştirilme tarihi {x} günden eski olan")),
                (TimeField::Modified, Cmp::Less) => tr(T!("değiştirilme tarihi {x} günden yeni olan")),
                (TimeField::Created, Cmp::Greater) => tr(T!("oluşturulma tarihi {x} günden eski olan")),
                (TimeField::Created, Cmp::Less) => tr(T!("oluşturulma tarihi {x} günden yeni olan")),
            };
            with(key, days.to_string())
        }
    }
}

/// What the rule does, the part after the arrow.
pub fn action_words(a: &Action, tr: &dyn Fn(&'static str) -> String, home: Option<&Path>) -> String {
    match a {
        Action::MoveTo { folder } => show_path(folder, home),
        Action::MoveToTemplate { template } => show_path(Path::new(template), home),
        Action::Rename { template } => tr(T!("yeniden adlandır: {x}")).replace("{x}", template),
        Action::Recycle => tr(T!("Geri Dönüşüm Kutusu'na gönder")),
    }
}

/// The whole rule in one line.
pub fn summary(rule: &Rule, tr: &dyn Fn(&'static str) -> String, home: Option<&Path>) -> String {
    let who = if rule.conditions.is_empty() {
        tr(T!("tüm dosyalar"))
    } else {
        let joiner = match rule.join {
            Join::All => tr(T!("ve")),
            Join::Any => tr(T!("veya")),
        };
        let parts: Vec<String> = rule.conditions.iter().map(|c| condition_words(c, tr)).collect();
        tr(T!("{c} dosyalar")).replace("{c}", &parts.join(&format!(" {joiner} ")))
    };
    format!("{who} → {}", action_words(&rule.action, tr, home))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn id(s: &'static str) -> String {
        s.to_string()
    }

    fn rule(conditions: Vec<Condition>, action: Action) -> Rule {
        Rule { conditions, action, ..Rule::default() }
    }

    #[test]
    fn the_starter_rule_reads_like_the_example() {
        let r = rule(
            vec![Condition::Extension { list: vec!["jpg".into(), ".PNG".into()] }],
            Action::MoveToTemplate { template: r"C:\Users\Ali\Pictures\{year}\{month}".into() },
        );
        let home = PathBuf::from(r"C:\Users\Ali");
        assert_eq!(summary(&r, &id, Some(&home)), r"uzantısı jpg, png olan dosyalar → Pictures\{year}\{month}");
        assert_eq!(summary(&r, &id, None), r"uzantısı jpg, png olan dosyalar → C:\Users\Ali\Pictures\{year}\{month}");
    }

    #[test]
    fn conditions_are_joined_by_the_rules_join() {
        let mut r = rule(
            vec![
                Condition::Name { op: NameOp::StartsWith, text: " IMG ".into() },
                Condition::Size { cmp: Cmp::Greater, bytes: 5 * 1024 * 1024 },
                Condition::Age { field: TimeField::Modified, cmp: Cmp::Greater, days: 30 },
            ],
            Action::Recycle,
        );
        assert_eq!(
            summary(&r, &id, None),
            "adı “IMG” ile başlayan ve boyutu 5 MB'den büyük olan ve değiştirilme tarihi 30 günden eski olan dosyalar → Geri Dönüşüm Kutusu'na gönder"
        );
        r.join = Join::Any;
        assert!(summary(&r, &id, None).contains(" veya "));
    }

    #[test]
    fn every_condition_and_action_has_words() {
        let r = rule(
            vec![
                Condition::Name { op: NameOp::Contains, text: "rapor".into() },
                Condition::Name { op: NameOp::EndsWith, text: ".bak".into() },
                Condition::Name { op: NameOp::Wildcard, text: "*.tmp".into() },
                Condition::Kind { kinds: vec![FileKind::Image, FileKind::Video] },
                Condition::Size { cmp: Cmp::Less, bytes: 1536 },
                Condition::Age { field: TimeField::Created, cmp: Cmp::Less, days: 7 },
            ],
            Action::Rename { template: "{year}-{name}.{ext}".into() },
        );
        assert_eq!(
            summary(&r, &id, None),
            "adında “rapor” geçen ve adı “.bak” ile biten ve adı “*.tmp” desenine uyan ve türü resim, video olan ve boyutu 1536 B'den küçük olan ve oluşturulma tarihi 7 günden yeni olan dosyalar → yeniden adlandır: {year}-{name}.{ext}"
        );
        assert_eq!(summary(&rule(vec![], Action::MoveTo { folder: PathBuf::from(r"D:\Arşiv") }), &id, None), r"tüm dosyalar → D:\Arşiv");
    }

    #[test]
    fn sizes_use_the_biggest_even_unit() {
        assert_eq!(size_text(5 * 1024 * 1024), "5 MB");
        assert_eq!(size_text(1536 * 1024), "1536 KB");
        assert_eq!(size_text(3 * 1024 * 1024 * 1024), "3 GB");
        assert_eq!(size_text(700), "700 B");
        assert_eq!(size_text(0), "0 B");
        assert_eq!(SizeUnit::split(1024), (1, SizeUnit::Kb));
        assert_eq!(SizeUnit::split(1025), (1025, SizeUnit::B));
    }

    #[test]
    fn a_translator_is_used_for_every_text() {
        let marked = |s: &'static str| format!("[{s}]");
        let r = rule(vec![Condition::Kind { kinds: vec![FileKind::Audio] }], Action::Recycle);
        assert_eq!(summary(&r, &marked, None), "[[türü [ses] olan] dosyalar] → [Geri Dönüşüm Kutusu'na gönder]");
    }

    #[test]
    fn paths_outside_the_home_are_shown_whole() {
        let home = PathBuf::from(r"C:\Users\Ali");
        assert_eq!(show_path(Path::new(r"D:\x"), Some(&home)), r"D:\x");
        assert_eq!(show_path(Path::new(r"C:\Users\Ali"), Some(&home)), r"C:\Users\Ali");
        assert_eq!(show_path(Path::new(r"C:\Users\Alice\x"), Some(&home)), r"C:\Users\Alice\x");
        assert_eq!(show_path(Path::new(r"c:\users\ali\Belgeler"), Some(&home)), r"Belgeler");
    }
}
