//! The rules file (`organizer.json` in the settings folder): versioned JSON, written whole
//! through a temporary file so a crash cannot leave half of it. A file that cannot be read is
//! never replaced by an empty list: it is renamed aside and the caller is told.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::rule::Rule;

/// The format this build writes. A file with a higher number was written by a newer build.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RulesFile {
    pub version: u32,
    /// The master switch of the automatic runs (off until the user turns it on).
    pub auto_enabled: bool,
    /// The id the next new rule gets.
    pub next_id: u32,
    pub rules: Vec<Rule>,
}

impl Default for RulesFile {
    fn default() -> Self {
        Self { version: VERSION, auto_enabled: false, next_id: 1, rules: Vec::new() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum LoadOutcome {
    /// There is no file yet.
    Missing,
    Loaded(RulesFile),
    /// The file is broken (or from a newer build): it was renamed to `moved_to` (None when the
    /// rename failed too) and nothing was loaded.
    Corrupt { moved_to: Option<PathBuf>, reason: String },
    /// The file could not be read at all (locked, no access): left where it is.
    Unreadable { reason: String },
}

impl RulesFile {
    pub fn load(path: &Path) -> LoadOutcome {
        let text = match crate::read_config(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
            // not valid UTF-8 text: a broken file, not a locked one
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return corrupt(path, e.to_string()),
            Err(e) => return LoadOutcome::Unreadable { reason: e.to_string() },
        };
        let text = text.trim_start_matches('\u{feff}');
        let value: serde_json::Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => return corrupt(path, e.to_string()),
        };
        // (serde would take a bare list for the struct's fields in order)
        if !value.is_object() {
            return corrupt(path, "the file is not a JSON object".into());
        }
        let version = value.get("version").and_then(serde_json::Value::as_u64).unwrap_or(1);
        if version > VERSION as u64 {
            return corrupt(path, format!("version {version}, this build reads up to {VERSION}"));
        }
        match serde_json::from_value::<RulesFile>(value) {
            Ok(mut f) => {
                f.version = VERSION;
                // ids must be unique and below next_id, whatever the file says
                let mut seen = std::collections::HashSet::new();
                let mut next = f.next_id.max(1);
                for r in &mut f.rules {
                    if r.id == 0 || !seen.insert(r.id) {
                        r.id = next;
                        next += 1;
                        seen.insert(r.id);
                    }
                    next = next.max(r.id + 1);
                }
                f.next_id = next;
                LoadOutcome::Loaded(f)
            }
            Err(e) => corrupt(path, e.to_string()),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        static LOCK: parking_lot::Mutex<u64> = parking_lot::Mutex::new(0);
        let mut n = LOCK.lock();
        *n += 1;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), *n));
        // never replace the real file with an empty one if serialization fails
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })
    }

    pub fn rule(&self, id: u32) -> Option<&Rule> {
        self.rules.iter().find(|r| r.id == id)
    }

    /// Adds a rule with a fresh id; returns the id.
    pub fn add(&mut self, mut rule: Rule) -> u32 {
        let id = self.next_id.max(self.rules.iter().map(|r| r.id + 1).max().unwrap_or(1));
        self.next_id = id + 1;
        rule.id = id;
        self.rules.push(rule);
        id
    }

    /// Replaces the rule with the same id, or adds it (with a new id) when `id` is 0.
    pub fn put(&mut self, rule: Rule) -> u32 {
        if rule.id != 0
            && let Some(slot) = self.rules.iter_mut().find(|r| r.id == rule.id)
        {
            let id = rule.id;
            *slot = rule;
            return id;
        }
        self.add(rule)
    }

    pub fn remove(&mut self, id: u32) {
        self.rules.retain(|r| r.id != id);
    }

    /// Moves the rule one place up (-1) or down (+1): the order decides which rule wins.
    pub fn shift(&mut self, id: u32, by: i32) {
        let Some(i) = self.rules.iter().position(|r| r.id == id) else { return };
        let j = i as i64 + by as i64;
        if j >= 0 && (j as usize) < self.rules.len() {
            self.rules.swap(i, j as usize);
        }
    }

    /// Are there rules that run by themselves?
    pub fn has_auto(&self) -> bool {
        self.auto_enabled && self.rules.iter().any(|r| r.enabled && r.auto)
    }
}

/// Renames the broken file to `<name>.bozuk-<ms>`; the user can mend and put it back.
fn corrupt(path: &Path, reason: String) -> LoadOutcome {
    let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".bozuk-{ms}"));
    let aside = path.with_file_name(name);
    let moved_to = std::fs::rename(path, &aside).ok().map(|_| aside);
    LoadOutcome::Corrupt { moved_to, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::organize::rule::{Action, Cmp, Condition, FileKind, Join, NameOp, TimeField};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-organize-store-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> Rule {
        Rule {
            name: "İndirilenler: resimler".into(),
            folder: PathBuf::from(r"C:\Users\Ali\Downloads"),
            subfolders: true,
            join: Join::Any,
            conditions: vec![
                Condition::Name { op: NameOp::Wildcard, text: "IMG_*.jp?".into() },
                Condition::Extension { list: vec!["jpg".into(), "png".into()] },
                Condition::Kind { kinds: vec![FileKind::Image, FileKind::Video] },
                Condition::Size { cmp: Cmp::Greater, bytes: 5 << 20 },
                Condition::Age { field: TimeField::Created, cmp: Cmp::Less, days: 14 },
            ],
            action: Action::MoveToTemplate { template: r"C:\Users\Ali\Pictures\{year}\{month}".into() },
            auto: true,
            ..Rule::default()
        }
    }

    #[test]
    fn a_file_round_trips() {
        let dir = temp("round");
        let path = dir.join("organizer.json");
        assert_eq!(RulesFile::load(&path), LoadOutcome::Missing);
        let mut f = RulesFile { auto_enabled: true, ..RulesFile::default() };
        let a = f.add(sample());
        let mut second = sample();
        second.name = "ikinci".into();
        second.action = Action::Recycle;
        let b = f.add(second);
        let mut third = sample();
        third.action = Action::Rename { template: "{year}-{name}.{ext}".into() };
        third.enabled = false;
        f.add(third);
        let mut fourth = sample();
        fourth.action = Action::MoveTo { folder: PathBuf::from(r"D:\Arşiv") };
        f.add(fourth);
        assert_eq!((a, b), (1, 2));
        f.save(&path).unwrap();
        let LoadOutcome::Loaded(back) = RulesFile::load(&path) else { panic!("not loaded") };
        assert_eq!(back, f);
        // the format is plain JSON with a version
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"auto_enabled\": true"), "{text}");
        // no temporary file stays behind
        let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["organizer.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_file_is_moved_aside_and_reported() {
        let dir = temp("corrupt");
        let path = dir.join("organizer.json");
        std::fs::write(&path, "{ \"rules\": [ {oops").unwrap();
        let LoadOutcome::Corrupt { moved_to, reason } = RulesFile::load(&path) else { panic!("not corrupt") };
        assert!(!reason.is_empty());
        let aside = moved_to.expect("renamed");
        assert!(aside.file_name().unwrap().to_string_lossy().starts_with("organizer.json.bozuk-"));
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&aside).unwrap(), "{ \"rules\": [ {oops");
        // the next load sees no file: the broken one is not read again, not overwritten
        assert_eq!(RulesFile::load(&path), LoadOutcome::Missing);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn wrong_shapes_are_corrupt_too() {
        let dir = temp("shape");
        for (i, text) in ["[]", "42", "{\"rules\": 5}", "{\"rules\": [{\"action\": {\"type\": \"explode\"}}]}", "\u{0}\u{1}"].iter().enumerate() {
            let path = dir.join(format!("o{i}.json"));
            std::fs::write(&path, text).unwrap();
            assert!(matches!(RulesFile::load(&path), LoadOutcome::Corrupt { moved_to: Some(_), .. }), "{text}");
            assert!(!path.exists(), "{text}");
        }
        // not even text
        let path = dir.join("bin.json");
        std::fs::write(&path, [0xff, 0xfe, 0xfd, 0x80]).unwrap();
        assert!(matches!(RulesFile::load(&path), LoadOutcome::Corrupt { .. }));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_newer_version_is_kept_aside_not_dropped() {
        let dir = temp("newer");
        let path = dir.join("organizer.json");
        std::fs::write(&path, "{\"version\": 99, \"rules\": []}").unwrap();
        let LoadOutcome::Corrupt { moved_to, reason } = RulesFile::load(&path) else { panic!() };
        assert!(reason.contains("99"));
        assert!(moved_to.unwrap().exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_fields_take_their_defaults_and_ids_are_repaired() {
        let dir = temp("defaults");
        let path = dir.join("organizer.json");
        std::fs::write(
            &path,
            "\u{feff}{\"version\": 1, \"rules\": [{\"id\": 5, \"name\": \"a\", \"folder\": \"C:\\\\x\"}, {\"id\": 5, \"name\": \"b\"}, {\"name\": \"c\"}]}",
        )
        .unwrap();
        let LoadOutcome::Loaded(f) = RulesFile::load(&path) else { panic!() };
        assert!(!f.auto_enabled, "off unless the file says so");
        let ids: Vec<u32> = f.rules.iter().map(|r| r.id).collect();
        assert_eq!(ids, [5, 6, 7]);
        assert_eq!(f.next_id, 8);
        assert!(f.rules.iter().all(|r| r.enabled && !r.auto && !r.subfolders));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn editing_the_list() {
        let mut f = RulesFile::default();
        let a = f.add(Rule { name: "a".into(), ..Rule::default() });
        let b = f.add(Rule { name: "b".into(), ..Rule::default() });
        let c = f.put(Rule { name: "c".into(), ..Rule::default() });
        assert_eq!([a, b, c], [1, 2, 3]);
        assert_eq!(f.put(Rule { id: b, name: "B".into(), ..Rule::default() }), b);
        assert_eq!(f.rule(b).unwrap().name, "B");
        f.shift(c, -1);
        assert_eq!(f.rules.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3, 2]);
        f.shift(1, -1);
        f.shift(2, 1);
        assert_eq!(f.rules.iter().map(|r| r.id).collect::<Vec<_>>(), [1, 3, 2]);
        f.remove(3);
        // a removed id is not handed out again
        let d = f.add(Rule::default());
        assert_eq!(d, 4);
        assert!(!f.has_auto());
        f.auto_enabled = true;
        assert!(!f.has_auto());
        f.rules[0].auto = true;
        assert!(f.has_auto());
        f.rules[0].enabled = false;
        assert!(!f.has_auto());
    }
}
