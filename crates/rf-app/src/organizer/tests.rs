// (not `use super::*`: gpui's own `test` attribute would come with it)
use super::{Problem, Source, blank_rule, classify, editor, from_outcome, kind_labels, make_plan, mark_undone, plural, refusal, result_title, rules_of_folder, starter_rule, was_undone};
use rf_core::organize::store::RulesFile;
use rf_core::organize::{Action, Condition, FileKind, ProtectedPaths};
use std::path::{Path, PathBuf};

/// Every user-facing Turkish text of the engine and the runner (marked `T!("...")` / `tx("...")`
/// there) has its English pair, so nothing shows up untranslated.
#[test]
fn the_texts_of_the_lower_crates_have_english_pairs() {
    let sources = [
        (include_str!("../../../rf-core/src/organize/rule.rs"), "T!(\""),
        (include_str!("../../../rf-core/src/organize/plan.rs"), "T!(\""),
        (include_str!("../../../rf-core/src/organize/safety.rs"), "T!(\""),
        (include_str!("../../../rf-core/src/organize/summary.rs"), "T!(\""),
        (include_str!("../../../rf-win/src/organize.rs"), "tx(\""),
    ];
    let have: std::collections::HashSet<&str> = crate::i18n_en::EN.iter().map(|(tr, _)| *tr).collect();
    let mut checked = 0;
    for (src, marker) in sources {
        let code = src.split("#[cfg(test)]").next().unwrap();
        let mut rest = code;
        while let Some(i) = rest.find(marker) {
            let after = &rest[i + marker.len()..];
            // up to the closing quote (an escaped quote is part of the text)
            let mut end = 0;
            let b = after.as_bytes();
            while end < b.len() && b[end] != b'"' {
                end += if b[end] == b'\\' { 2 } else { 1 };
            }
            if after[end + 1..].starts_with(')') {
                let key = after[..end].replace("\\\"", "\"").replace("\\\\", "\\");
                assert!(have.contains(key.as_str()), "no English pair for {key:?}");
                checked += 1;
            }
            rest = &after[end..];
        }
    }
    assert!(checked > 50, "only {checked} texts found: the markers moved?");
}

#[test]
fn the_apps_file_categories_decide_the_kind() {
    assert_eq!(classify("jpg"), FileKind::Image);
    assert_eq!(classify("mkv"), FileKind::Video);
    assert_eq!(classify("flac"), FileKind::Audio);
    assert_eq!(classify("zip"), FileKind::Archive);
    assert_eq!(classify("exe"), FileKind::Program);
    assert_eq!(classify("pdf"), FileKind::Document);
    assert_eq!(classify("xlsx"), FileKind::Document);
    assert_eq!(classify("txt"), FileKind::Document);
    assert_eq!(classify("rs"), FileKind::Other);
    assert_eq!(classify(""), FileKind::Other);
}

#[test]
fn the_starter_is_the_downloads_to_pictures_example_and_is_not_saved() {
    let r = starter_rule();
    assert_eq!(r.id, 0, "not in the list until the user saves it");
    assert!(!r.auto && !r.subfolders && r.enabled);
    assert!(matches!(&r.conditions[..], [Condition::Kind { kinds }] if kinds == &[FileKind::Image]));
    let Action::MoveToTemplate { template } = &r.action else { panic!("{:?}", r.action) };
    assert!(template.ends_with(r"{year}\{month}"), "{template}");
    let b = blank_rule(PathBuf::from(r"D:\x"));
    assert_eq!(b.folder, PathBuf::from(r"D:\x"));
    assert!(b.conditions.is_empty() && !b.auto);
}

#[test]
fn rules_are_grouped_by_folder_whatever_the_spelling() {
    let mut f = RulesFile::default();
    let mut a = blank_rule(PathBuf::from(r"C:\Users\Ali\Downloads"));
    a.name = "a".into();
    f.add(a.clone());
    a.folder = PathBuf::from(r"c:/users/ali/DOWNLOADS/");
    a.name = "b".into();
    f.add(a.clone());
    a.folder = PathBuf::from(r"C:\Users\Ali\Desktop");
    a.name = "c".into();
    f.add(a.clone());
    a.folder = PathBuf::from(r"C:\Users\Ali\Downloads");
    a.name = "kapalı".into();
    a.enabled = false;
    f.add(a);
    let names: Vec<String> = rules_of_folder(&f, Path::new(r"C:\USERS\ali\downloads")).into_iter().map(|r| r.name).collect();
    assert_eq!(names, ["a", "b"], "same place spelled differently, switched-off rules left out");
    assert!(rules_of_folder(&f, Path::new(r"C:\Users\Ali\Documents")).is_empty());
}

#[test]
fn the_plan_of_a_real_folder_uses_the_apps_kinds_and_labels() {
    /// Removes the test's own folder even when an assertion fails.
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let dir = std::env::temp_dir().join(format!("rf-organizer-app-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let _cleanup = Cleanup(dir.clone());
    std::fs::write(dir.join("a.jpg"), "1").unwrap();
    std::fs::write(dir.join("b.txt"), "2").unwrap();
    std::fs::write(dir.join("c.crdownload"), "3").unwrap();
    let mut r = blank_rule(dir.clone());
    r.name = "tür".into();
    r.conditions = vec![Condition::Kind { kinds: vec![FileKind::Image, FileKind::Document] }];
    r.action = Action::MoveToTemplate { template: format!(r"{}\{{kind}}", dir.display()) };
    let plan = make_plan(&[r], &Source::Scan { root: dir.clone(), subfolders: false }, &Default::default(), &ProtectedPaths::default(), &|| false);
    let mut dests: Vec<String> = plan
        .steps
        .iter()
        .map(|s| match &s.dest {
            rf_core::organize::Dest::Move(d) => d.strip_prefix(&dir).unwrap().to_string_lossy().into_owned(),
            rf_core::organize::Dest::Recycle => "<bin>".into(),
        })
        .collect();
    dests.sort();
    // (the folder names are in whatever language another test has switched to; what matters is
    // that the two kinds go to two different folders and keep their names)
    assert_eq!(dests.len(), 2, "{dests:?}");
    let split: Vec<(&str, &str)> = dests.iter().filter_map(|d| d.split_once('\\')).collect();
    assert_eq!(split.len(), 2, "{dests:?}");
    assert_ne!(split[0].0, split[1].0, "{dests:?}");
    let mut files: Vec<&str> = split.iter().map(|s| s.1).collect();
    files.sort();
    assert_eq!(files, ["a.jpg", "b.txt"]);
    assert!(kind_labels().iter().all(|l| !l.is_empty()));
    assert_eq!(plan.ignored, 1, "the partial download is left alone");
    // the same files, as paths a watcher reported: one stat each
    let plan = make_plan(&[], &Source::Paths(vec![dir.join("a.jpg"), dir.join("yok.jpg")]), &Default::default(), &ProtectedPaths::default(), &|| false);
    assert!(plan.steps.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_result_line_says_what_happened() {
    use rf_win::organize::{Report, Skip, Why};
    let mut rep = Report::default();
    rep.record.moves = vec![(PathBuf::from("a"), PathBuf::from("b")); 3];
    rep.record.recycled = vec![(PathBuf::from("c"), PathBuf::from("d"))];
    rep.skipped = vec![Skip { path: "x".into(), why: Why::Hidden }, Skip { path: "y".into(), why: Why::TooYoung }];
    rep.deferred_by_cap = 20;
    // (the words depend on the language other tests switch; the counts do not)
    let line = result_title(&rep);
    let parts: Vec<&str> = line.split(" · ").collect();
    assert_eq!(parts.len(), 3, "{line}");
    assert!(parts[0].contains('4') && parts[1].contains('1') && parts[2].contains("21"), "{line}");
    assert!(!result_title(&Report::default()).contains(" · "));
}

#[test]
fn problems_are_told_in_words() {
    use rf_core::organize::RuleProblem;
    assert!(!editor::problem_text(&RuleProblem::NoName).is_empty());
    assert!(editor::problem_text(&RuleProblem::UnknownVariable("yıl".into())).ends_with(": {yıl}"));
    assert!(editor::problem_text(&RuleProblem::EmptyCondition(1)).starts_with("2. "));
}

#[test]
fn a_rules_file_that_could_not_be_set_aside_blocks_saving() {
    use rf_core::organize::store::LoadOutcome;
    // Corrupt, and the damaged file could not be renamed: it still lies where the rules are
    // saved, so the next save would destroy it
    let (rules, problem, blocked) = from_outcome(Some(LoadOutcome::Corrupt { moved_to: None, reason: "locked".into() }));
    assert!(blocked, "saving over a damaged file that is still in place loses what could be recovered");
    assert!(rules.rules.is_empty() && matches!(problem, Some(Problem::Corrupt { moved_to: None, .. })));
    // set aside: the place is free, saving is fine
    let (_, problem, blocked) = from_outcome(Some(LoadOutcome::Corrupt { moved_to: Some(PathBuf::from(r"C:\x\organizer.json.bozuk-1")), reason: "bad".into() }));
    assert!(!blocked && problem.is_some());
    // locked or unreadable: left alone
    let (_, problem, blocked) = from_outcome(Some(LoadOutcome::Unreadable { reason: "denied".into() }));
    assert!(blocked && matches!(problem, Some(Problem::Unreadable(_))));
    // nothing there yet, or fine
    let (_, problem, blocked) = from_outcome(Some(LoadOutcome::Missing));
    assert!(!blocked && problem.is_none());
    let (_, problem, blocked) = from_outcome(None);
    assert!(!blocked && problem.is_none());
}

#[test]
fn counted_texts_say_one_file_and_n_files() {
    assert_eq!(plural(1, "1 file organized", "{n} files organized"), "1 file organized");
    assert_eq!(plural(2, "1 file organized", "{n} files organized"), "2 files organized");
    assert_eq!(plural(0, "1 file organized", "{n} files organized"), "0 files organized");
}

#[test]
fn a_run_is_undone_once_whoever_asks() {
    // the row's button after Ctrl+Z (or twice) must find the run already undone
    let token = 0xfeed_0001_u64;
    assert!(!was_undone(token));
    assert!(mark_undone(token));
    assert!(was_undone(token));
    assert!(!mark_undone(token), "a second undo does not start");
}

#[test]
fn a_run_comes_off_the_ctrl_z_stack_by_its_token_only_once() {
    use crate::undo::{UndoOp, forget_organized, push};
    let rec = |token| rf_win::organize::Record { token, moves: vec![(PathBuf::from("a"), PathBuf::from("b"))], ..Default::default() };
    push(UndoOp::Organized { rec: rec(0xfeed_0002) });
    push(UndoOp::Organized { rec: rec(0xfeed_0003) });
    assert!(forget_organized(0xfeed_0002), "found in the middle of the stack");
    assert!(!forget_organized(0xfeed_0002), "only once");
    assert!(!forget_organized(0xfeed_0099), "a run that ran by itself was never on it");
    assert!(forget_organized(0xfeed_0003));
}

#[test]
fn a_junction_to_a_protected_place_is_refused_like_the_place_itself() {
    struct Cleanup(PathBuf, PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.1); // (the junction itself, not what it points to)
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let base = std::env::temp_dir().join(format!("rf-organizer-junction-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let real = base.join("gercek");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("a.jpg"), "x").unwrap();
    let link = base.join("kisayol");
    let _cleanup = Cleanup(base.clone(), link.clone());
    let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&link).arg(&real).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
    assert!(ok, "mklink /J failed");
    let mut protected = ProtectedPaths::default();
    protected.app_dirs.push(real.clone());
    // the text of the path does not give it away ...
    assert!(protected.refuse(&link, false).is_none());
    // ... the place it leads to does
    assert_eq!(refusal(&protected, &link, false), Some(rf_core::organize::Refusal::AppOwn));
    let mut r = blank_rule(link.clone());
    r.name = "r".into();
    r.action = Action::MoveTo { folder: base.join("hedef") };
    let plan = make_plan(&[r], &Source::Scan { root: link.clone(), subfolders: false }, &Default::default(), &protected, &|| false);
    assert!(plan.steps.is_empty(), "nothing is planned in a protected place");
    assert_eq!(plan.unusable.len(), 1);
    assert!(real.join("a.jpg").exists());
}

#[test]
fn skipped_files_are_named_by_reason() {
    use rf_win::organize::{Skip, Why};
    let skip = |n: &str, why| Skip { path: PathBuf::from(format!(r"C:\x\{n}")), why };
    let lines = super::skipped_lines(&[skip("a.zip", Why::InUse), skip("b.zip", Why::InUse), skip("c.txt", Why::Hidden), skip("d.zip", Why::InUse), skip("e.zip", Why::InUse), skip("f.zip", Why::InUse)]);
    assert_eq!(lines.len(), 2, "one line per reason: {lines:?}");
    assert!(lines[0].contains("a.zip, b.zip, d.zip +2"), "{lines:?}");
    assert!(lines[1].contains("c.txt"), "{lines:?}");
    assert!(super::skipped_lines(&[]).is_empty());
}
