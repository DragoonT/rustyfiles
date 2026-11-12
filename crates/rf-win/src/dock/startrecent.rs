//! "Önerilenler" in the Start panel: what Windows recently opened, as a short list the panel can offer. The choice is pure (a list of
//! candidates, a clock and a test for "is it still there" in; the entries in the order they are shown, each with how long ago, out);
//! reading the candidates is the few functions at the end, for a worker thread.
//!
//! WHERE THEY COME FROM. The recent files Windows keeps are the `.lnk` files in `%APPDATA%\Microsoft\Windows\Recent`: a shortcut per
//! document, whose modification time is when the document was last opened (this is also what the file manager's Home page lists,
//! `places.rs`). Nothing here asks for a permission, and nothing is ever written. RECENTLY INSTALLED APPS: the list of installed apps
//! the panel has (`apps::list`, the AppsFolder) gives a name and an id and no date of installation, so there is no source for them
//! today; [`Source::App`] and [`When::Added`] are what an app entry would be, if a source is found, and are tested, but nothing makes one.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::toasts::{self, Ago};

/// How many entries the section shows at most.
pub const LIMIT: usize = 6;
/// How many of the newest shortcuts are looked at to find that many files that still exist (a folder, a deleted file and a file on a
/// network share are skipped).
pub const SCAN: usize = 40;
/// How long the shell thread may take to read the shortcuts before the section is left out this time.
const RESOLVE_LIMIT: Duration = Duration::from_secs(3);

/// What an entry opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A document: its path.
    File(PathBuf),
    /// An installed app: its id in the AppsFolder.
    App(String),
}

/// A thing that could be recommended, as a source reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    pub source: Source,
    /// When it was last opened (a document) or installed (an app): Unix time, ms.
    pub at_ms: i64,
}

/// The second line of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum When {
    /// A document: how long ago it was opened (`Ago::Date`: the panel writes the date).
    Ago(Ago),
    /// An app: "Son eklenen".
    Added,
}

/// An entry of the section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pick {
    pub name: String,
    pub source: Source,
    pub at_ms: i64,
    pub when: When,
}

/// The same thing under another spelling of its path or id: Windows compares paths and ids without regard to case.
fn same(a: &Source, b: &Source) -> bool {
    match (a, b) {
        (Source::File(x), Source::File(y)) => x.as_os_str().eq_ignore_ascii_case(y.as_os_str()),
        (Source::App(x), Source::App(y)) => x.eq_ignore_ascii_case(y),
        _ => false,
    }
}

/// The entries to show: the newest first, each thing once, only what `exists`, at most `limit`. `now_ms` is the clock, `day_of` turns a
/// time (Unix ms) into a local calendar day count (so that "dün" is the day before today, not the last 24 to 48 hours).
pub fn choose(mut candidates: Vec<Candidate>, now_ms: i64, limit: usize, day_of: impl Fn(i64) -> i64, exists: impl Fn(&Candidate) -> bool) -> Vec<Pick> {
    // newest first; the same moment is told apart by the name so that the order does not change from one reading to the next
    candidates.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    let today = day_of(now_ms);
    let mut out: Vec<Pick> = Vec::new();
    for c in candidates {
        if out.len() >= limit {
            break;
        }
        // (a moment that was never recorded is no time at all: it is not "1 January 1970")
        if c.name.trim().is_empty() || c.at_ms <= 0 || out.iter().any(|p| same(&p.source, &c.source)) || !exists(&c) {
            continue;
        }
        let when = match c.source {
            Source::App(_) => When::Added,
            Source::File(_) => When::Ago(toasts::ago((now_ms - c.at_ms) / 1000, today, day_of(c.at_ms))),
        };
        out.push(Pick { name: c.name, source: c.source, at_ms: c.at_ms, when });
    }
    out
}

// ---------------------------------------------------------------- reading the shortcuts

/// The newest `.lnk` files of `dir` with their modification times (Unix ms), newest first, at most `scan`. Disk: a worker thread.
pub fn list_links(dir: &Path, scan: usize) -> Vec<(i64, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut links: Vec<(i64, PathBuf)> = rd
        .filter_map(Result::ok)
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))
        .filter_map(|e| {
            let t = e.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some((t.as_millis() as i64, e.path()))
        })
        .collect();
    links.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    links.truncate(scan);
    links
}

/// The candidates behind `links`, as far as `resolve` (a shortcut's target) can tell: each target once, named by its file name.
pub fn candidates_from(links: &[(i64, PathBuf)], mut resolve: impl FnMut(&Path) -> Option<PathBuf>) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for (at_ms, link) in links {
        let Some(target) = resolve(link).filter(|t| !t.as_os_str().is_empty()) else { continue };
        let Some(name) = target.file_name().map(|n| n.to_string_lossy().into_owned()) else { continue };
        let source = Source::File(target);
        if out.iter().any(|c| same(&c.source, &source)) {
            continue;
        }
        out.push(Candidate { name, source, at_ms: *at_ms });
    }
    out
}

/// Whether the file can be looked at without waiting for a network: not a UNC path and not on a network drive. A recommendation must
/// not make the panel wait for a share that is asleep.
pub fn is_local(path: &Path) -> bool {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    let s = path.to_string_lossy();
    if s.starts_with(r"\\") {
        return false;
    }
    let b = s.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' {
        return false;
    }
    let root = crate::wide::wide(format!("{}:\\", b[0] as char));
    // DRIVE_FIXED 3, DRIVE_REMOVABLE 2, DRIVE_RAMDISK 6 are local and quick; a network (4), an optical drive (5) and the unknown (0, 1) are not
    matches!(unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) }, 2 | 3 | 6)
}

/// Whether the candidate is a file that is still there (a folder is not recommended).
pub fn still_there(c: &Candidate) -> bool {
    match &c.source {
        Source::File(p) => is_local(p) && std::fs::metadata(p).is_ok_and(|m| m.is_file()),
        Source::App(_) => true,
    }
}

/// The folder Windows keeps the recent-file shortcuts in.
pub fn recent_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Microsoft").join("Windows").join("Recent"))
}

/// How many shortcuts one job on the shell thread resolves. That thread serves every shell request of the dock, the icon extraction
/// among them: a batch of 40 resolutions in one job made the first icons of an opening queue behind it. With a few at a time the other
/// requests get their turn between the batches, and the reading stops as soon as there are enough files.
pub const BATCH: usize = 6;

/// Resolves `links` batch by batch until `want` of the candidates pass `exists`. `resolve_batch` resolves one batch of shortcuts (it
/// runs on the shell thread; `None`: it did not answer in time, which ends the reading with what there is). A target is listed once,
/// the newest shortcut's reading kept. Returns the candidates found, newest first.
pub fn gather(links: &[(i64, PathBuf)], batch: usize, want: usize, mut resolve_batch: impl FnMut(&[(i64, PathBuf)]) -> Option<Vec<Candidate>>, exists: impl Fn(&Candidate) -> bool) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = Vec::new();
    let mut there = 0usize;
    for chunk in links.chunks(batch.max(1)) {
        let Some(got) = resolve_batch(chunk) else { break };
        for c in got {
            if found.iter().any(|f| same(&f.source, &c.source)) {
                continue;
            }
            if exists(&c) {
                there += 1;
            }
            found.push(c);
        }
        if there >= want {
            break;
        }
    }
    found
}

/// The recent documents as candidates (not yet checked for being there). Blocks (the disk, the shell thread): a worker thread.
/// The shortcuts are read on the one shell thread (`sta::call`) a few at a time (see [`BATCH`]), within [`RESOLVE_LIMIT`] in all; a
/// reading that does not come in time leaves what was found, or nothing, which hides the section.
pub fn recent_candidates() -> Vec<Candidate> {
    let Some(dir) = recent_dir() else { return Vec::new() };
    let links = list_links(&dir, SCAN);
    if links.is_empty() {
        return Vec::new();
    }
    let started = std::time::Instant::now();
    gather(
        &links,
        BATCH,
        LIMIT,
        |chunk| {
            let left = RESOLVE_LIMIT.checked_sub(started.elapsed())?;
            let chunk = chunk.to_vec();
            crate::sta::call(left, move || candidates_from(&chunk, |p| crate::shortcut::resolve(p).ok().map(|i| i.target)))
        },
        still_there,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400_000;
    /// A made-up clock: the "day" of a time is its number of whole days since the epoch, as if the user lived at UTC.
    fn day_of(ms: i64) -> i64 {
        ms.div_euclid(DAY)
    }

    fn file(name: &str, at_ms: i64) -> Candidate {
        Candidate { name: name.into(), source: Source::File(PathBuf::from(format!(r"C:\Docs\{name}"))), at_ms }
    }

    fn app(name: &str, at_ms: i64) -> Candidate {
        Candidate { name: name.into(), source: Source::App(format!("{name}!App")), at_ms }
    }

    /// 20 days in, noon.
    const NOW: i64 = 20 * DAY + 12 * 3_600_000;

    fn pick(c: Vec<Candidate>) -> Vec<Pick> {
        choose(c, NOW, LIMIT, day_of, |_| true)
    }

    #[test]
    fn the_newest_come_first_and_at_most_six_are_chosen() {
        let c: Vec<Candidate> = (0..10).map(|i| file(&format!("f{i}.txt"), NOW - 1_000 * i)).collect();
        let got = pick(c);
        assert_eq!(got.len(), LIMIT);
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["f0.txt", "f1.txt", "f2.txt", "f3.txt", "f4.txt", "f5.txt"]);
        // a different limit
        assert_eq!(choose((0..10).map(|i| file(&format!("f{i}"), NOW - i)).collect(), NOW, 2, day_of, |_| true).len(), 2);
        assert!(pick(Vec::new()).is_empty(), "nothing to recommend: an empty list, which hides the section");
    }

    #[test]
    fn a_file_that_is_gone_is_not_recommended_and_the_next_one_takes_its_place() {
        let c: Vec<Candidate> = (0..8).map(|i| file(&format!("f{i}"), NOW - 1_000 * i)).collect();
        let got = choose(c, NOW, LIMIT, day_of, |c| c.name != "f1" && c.name != "f2");
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["f0", "f3", "f4", "f5", "f6", "f7"]);
    }

    #[test]
    fn each_thing_once_whatever_the_case_of_its_path() {
        let a = file("Report.docx", NOW - 10_000);
        let b = Candidate { source: Source::File(PathBuf::from(r"c:\docs\REPORT.DOCX")), ..file("REPORT.DOCX", NOW - 5_000) };
        let got = pick(vec![a, b, file("other.txt", NOW - 20_000)]);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "REPORT.DOCX", "the newer reading is the one kept");
    }

    #[test]
    fn a_time_that_was_never_recorded_and_a_blank_name_are_left_out() {
        let got = pick(vec![file("zero.txt", 0), file("negative.txt", -5), file("  ", NOW - 1), file("fine.txt", NOW - 1)]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "fine.txt");
    }

    #[test]
    fn the_same_moment_keeps_a_steady_order() {
        let a = pick(vec![file("b.txt", NOW - 5), file("A.txt", NOW - 5), file("c.txt", NOW - 5)]);
        let b = pick(vec![file("c.txt", NOW - 5), file("b.txt", NOW - 5), file("A.txt", NOW - 5)]);
        assert_eq!(a, b);
        assert_eq!(a.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A.txt", "b.txt", "c.txt"]);
    }

    #[test]
    fn the_second_line_says_how_long_ago_and_yesterday_is_the_day_before_not_the_last_day() {
        let min = 60_000;
        let hour = 60 * min;
        let got = pick(vec![
            file("now", NOW - 20_000),
            file("min", NOW - 5 * min),
            file("hours", NOW - 3 * hour),
            // 15 hours ago is 21:00 yesterday: less than 24 hours, so the hours say it
            file("evening", NOW - 15 * hour),
            // 28 hours ago is 08:00 yesterday: more than a day, and the day before today
            file("yesterday", NOW - 28 * hour),
            file("older", NOW - 3 * DAY),
        ]);
        let when = |n: &str| got.iter().find(|p| p.name == n).unwrap().when;
        assert_eq!(when("now"), When::Ago(Ago::Now));
        assert_eq!(when("min"), When::Ago(Ago::Minutes(5)));
        assert_eq!(when("hours"), When::Ago(Ago::Hours(3)));
        assert_eq!(when("evening"), When::Ago(Ago::Hours(15)));
        assert_eq!(when("yesterday"), When::Ago(Ago::Yesterday));
        assert_eq!(when("older"), When::Ago(Ago::Date));
    }

    #[test]
    fn a_clock_that_is_behind_the_file_says_now_and_an_app_says_it_was_added() {
        let got = pick(vec![file("future", NOW + 90_000), app("Paint", NOW - 2 * DAY)]);
        assert_eq!(got[0].name, "future");
        assert_eq!(got[0].when, When::Ago(Ago::Now));
        assert_eq!(got[1].when, When::Added, "an app's second line is 'Son eklenen', whatever its age");
        assert_eq!(got[1].source, Source::App("Paint!App".into()));
    }

    #[test]
    fn files_and_apps_are_ordered_together_by_their_time() {
        let got = pick(vec![file("old.txt", NOW - 3 * DAY), app("New App", NOW - 1_000), file("mid.txt", NOW - DAY)]);
        assert_eq!(got.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["New App", "mid.txt", "old.txt"]);
    }

    // ---- reading

    #[test]
    fn the_shortcuts_are_listed_newest_first_and_only_the_lnk_files() {
        let dir = std::env::temp_dir().join(format!("rf-startrecent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let t0 = std::time::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        for (i, name) in ["old.lnk", "new.LNK", "mid.lnk", "notes.txt", "another.lnk"].iter().enumerate() {
            let f = std::fs::File::create(dir.join(name)).unwrap();
            f.set_modified(t0 + Duration::from_secs([10u64, 300, 100, 999, 200][i])).unwrap();
        }
        let got = list_links(&dir, 10);
        let names: Vec<String> = got.iter().map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["new.LNK", "another.lnk", "mid.lnk", "old.lnk"]);
        assert_eq!(got[0].0, 1_700_000_300_000);
        assert_eq!(list_links(&dir, 2).len(), 2, "the scan is cut");
        assert!(list_links(&dir.join("nothing here"), 10).is_empty(), "a folder that is not there is an empty list");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_shortcut_that_cannot_be_read_or_points_at_nothing_is_skipped_and_a_target_is_listed_once() {
        let links = vec![
            (300, PathBuf::from("a.lnk")),
            (200, PathBuf::from("broken.lnk")),
            (150, PathBuf::from("empty.lnk")),
            (100, PathBuf::from("b.lnk")),
            (50, PathBuf::from("dir.lnk")),
        ];
        let got = candidates_from(&links, |p| match p.to_str().unwrap() {
            "a.lnk" => Some(PathBuf::from(r"C:\Docs\Plan.docx")),
            "broken.lnk" => None,
            "empty.lnk" => Some(PathBuf::new()),
            "b.lnk" => Some(PathBuf::from(r"c:\docs\PLAN.DOCX")),
            _ => Some(PathBuf::from(r"C:\")),
        });
        assert_eq!(got.len(), 1, "{got:?}");
        assert_eq!(got[0].name, "Plan.docx");
        assert_eq!(got[0].at_ms, 300, "the newer shortcut's time");
    }

    #[test]
    fn the_reading_goes_a_batch_at_a_time_and_stops_when_there_are_enough_files() {
        let links: Vec<(i64, PathBuf)> = (0..40).map(|i| (1_000 - i, PathBuf::from(format!("l{i}.lnk")))).collect();
        let mut batches = 0;
        let got = gather(
            &links,
            6,
            LIMIT,
            |chunk| {
                batches += 1;
                assert!(chunk.len() <= 6, "a job on the shell thread is a few shortcuts, not forty");
                Some(candidates_from(chunk, |p| Some(PathBuf::from(format!(r"C:\Docs\{}.txt", p.file_stem().unwrap().to_string_lossy())))))
            },
            |_| true,
        );
        assert_eq!(batches, 1, "six files are there after the first batch of six");
        assert_eq!(got.len(), 6);
        // some are gone: more batches are read until six are there
        let mut batches = 0;
        let got = gather(
            &links,
            6,
            LIMIT,
            |chunk| {
                batches += 1;
                Some(candidates_from(chunk, |p| Some(PathBuf::from(format!(r"C:\Docs\{}.txt", p.file_stem().unwrap().to_string_lossy())))))
            },
            still_there_fake,
        );
        assert!(batches > 1 && batches < 7, "{batches}: not all forty at once");
        assert!(got.len() < 40);
        assert!(got.iter().filter(|c| still_there_fake(c)).count() >= LIMIT);
    }

    fn still_there_fake(c: &Candidate) -> bool {
        matches!(&c.source, Source::File(p) if ["3.txt", "7.txt", "9.txt", "1.txt", "5.txt", "0.txt"].iter().any(|e| p.to_string_lossy().ends_with(e)))
    }

    #[test]
    fn a_batch_that_does_not_answer_ends_the_reading_with_what_there_is_and_a_target_is_kept_once_across_batches() {
        let links: Vec<(i64, PathBuf)> = (0..12).map(|i| (100 - i, PathBuf::from(format!("l{i}.lnk")))).collect();
        let mut n = 0;
        let got = gather(
            &links,
            4,
            LIMIT,
            |chunk| {
                n += 1;
                if n == 3 {
                    return None;
                }
                // every batch points at the same two documents plus one of its own
                Some(candidates_from(chunk, |p| {
                    let i: usize = p.file_stem().unwrap().to_string_lossy()[1..].parse().unwrap();
                    Some(PathBuf::from(if i.is_multiple_of(4) { r"C:\Docs\same-a.txt".to_string() } else if i % 4 == 1 { r"c:\docs\SAME-B.txt".to_string() } else { format!(r"C:\Docs\own{i}.txt") }))
                }))
            },
            |_| false,
        );
        assert_eq!(n, 3, "the third batch did not answer: nothing after it is asked");
        // batch 1: same-a, same-b, own2, own3; batch 2: (same-a, same-b again, dropped), own6, own7
        assert_eq!(got.len(), 6, "{got:?}");
        assert_eq!(got.iter().filter(|c| c.name.eq_ignore_ascii_case("same-a.txt")).count(), 1);
        assert_eq!(got[0].at_ms, 100, "the newest shortcut's reading is the one kept");
    }

    #[test]
    fn network_paths_and_unreadable_roots_are_not_touched() {
        assert!(!is_local(Path::new(r"\\server\share\a.txt")));
        assert!(!is_local(Path::new("relative.txt")));
        assert!(!is_local(Path::new("")));
        assert!(!still_there(&file("x", NOW)), "a file that does not exist");
        assert!(still_there(&app("Paint", NOW)), "an app is checked by whoever knows it");
    }

    #[test]
    fn a_real_file_in_a_temp_folder_is_there_and_a_folder_is_not_recommended() {
        let dir = std::env::temp_dir().join(format!("rf-startrecent-there-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("doc.txt"), b"x").unwrap();
        let c = |p: PathBuf| Candidate { name: "x".into(), source: Source::File(p), at_ms: 1 };
        assert!(still_there(&c(dir.join("doc.txt"))));
        assert!(!still_there(&c(dir.join("sub"))));
        assert!(!still_there(&c(dir.join("gone.txt"))));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reads this PC's own recent files and says how many there are; changes nothing. Needs a desktop session.
    #[test]
    fn reads_this_pcs_recent_documents_without_changing_anything() {
        let t = std::time::Instant::now();
        let candidates = recent_candidates();
        let n = candidates.len();
        let there = choose(candidates, 1_000_000_000_000_000, LIMIT, |_| 0, still_there).len();
        eprintln!("recent documents: {n} candidates, {there} chosen, in {} ms", t.elapsed().as_millis());
        assert!(there <= LIMIT);
    }
}
