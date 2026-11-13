//! What the Dock page can say about a part that could not start.
//!
//! The parts run in the dock's process and the Settings page lives in the main rustyfiles, so the news has to cross a process border.
//! It crosses as a small text file in the settings folder, `dock-parts.txt`, one `part reason` pair per line (words, never a path or a
//! name). THE DOCK'S SIDE ([`start`]): once every few seconds the host works out which parts are not running although they should
//! (the taskbar's own status, and the faults the other parts noted when they were started, see [`note`]) and writes the file when the
//! set changed; no fault: no file. THE PAGE'S SIDE ([`poll`], [`cached`]): the page asks for a look now and then while it is on screen;
//! the look (the file, and one write to the dock's pipe to see whether a dock answers at all, since a dock that died leaves its last
//! word behind) happens on a background thread, and the page draws what the last look found.
//!
//! COST. A dock that is not running does nothing. A running dock does a lock, a comparison and a few reads of in-memory state every
//! three seconds, and a file write only when the set of faults changed. The page does nothing while it is not shown.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::*;

use super::parts::{Part, Why};

/// The file's name in the settings folder.
pub const FILE: &str = "dock-parts.txt";

/// How often the dock looks at its parts.
const HOST_EVERY: Duration = Duration::from_secs(3);
/// How often the page looks at the file while it is shown.
const PAGE_EVERY: Duration = Duration::from_secs(2);
/// The page stops looking when it has not been drawn for this long (a page nobody sees needs no news).
const PAGE_GIVES_UP: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------- the file's format

/// The text of the file for these faults (empty for none).
pub fn encode(faults: &[(Part, Why)]) -> String {
    faults.iter().map(|(p, w)| format!("{} {}\n", p.token(), w.token())).collect()
}

/// The faults a file holds. A line that is not understood is skipped (a newer build may write words this one does not know), and a
/// pair is not listed twice.
pub fn decode(text: &str) -> Vec<(Part, Why)> {
    let mut out: Vec<(Part, Why)> = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(p), Some(w)) = (words.next(), words.next()) else { continue };
        if let (Some(p), Some(w)) = (Part::from_token(p), Why::from_token(w))
            && !out.contains(&(p, w))
        {
            out.push((p, w));
        }
    }
    out
}

/// `rf_win::taskbar_hide::Fault` as the reason the page tells.
pub fn why_of_fault(f: rf_win::taskbar_hide::Fault) -> Why {
    use rf_win::taskbar_hide::Fault;
    match f {
        Fault::GuardMissing => Why::GuardMissing,
        Fault::GuardNotConfirmed => Why::GuardNotConfirmed,
        Fault::GuardLost => Why::GuardLost,
        Fault::RecordNotSaved => Why::RecordNotSaved,
    }
}

/// The faults to write: what the parts noted, plus the taskbar's, in the order of [`Part::ALL`], each pair once.
pub fn merge(noted: &[(Part, Why)], taskbar: Option<Why>) -> Vec<(Part, Why)> {
    let mut all: Vec<(Part, Why)> = noted.to_vec();
    if let Some(w) = taskbar {
        all.retain(|(p, _)| *p != Part::Taskbar);
        all.push((Part::Taskbar, w));
    }
    all.sort_by_key(|(p, _)| Part::ALL.iter().position(|x| x == p));
    all.dedup();
    all
}

fn file_path() -> Option<std::path::PathBuf> {
    crate::settings::path().and_then(|p| p.parent().map(|d| d.join(FILE)))
}

// ---------------------------------------------------------------- the dock's side

/// What the parts noted when they were started (or stopped): a part that could not start puts its fault here, a part that started or
/// stopped takes it away.
static NOTED: Mutex<Vec<(Part, Why)>> = Mutex::new(Vec::new());

/// A part could not start (`Some`), or it started or stopped properly (`None`). Cheap: a lock and a small list.
pub fn note(part: Part, why: Option<Why>) {
    let mut g = NOTED.lock().unwrap_or_else(|p| p.into_inner());
    g.retain(|(p, _)| *p != part);
    if let Some(w) = why {
        g.push((part, w));
    }
}

/// What the host last wrote (`None` before the first look, so that the first look always writes: a file left by an earlier dock goes).
static WRITTEN: Mutex<Option<Vec<(Part, Why)>>> = Mutex::new(None);

/// One look at the parts. Returns the faults that are to be on file now, and writes them (on a thread of its own) when they are not
/// what was written last.
fn look(taskbar: Option<Why>) -> Vec<(Part, Why)> {
    let noted = NOTED.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let now = merge(&noted, taskbar);
    let mut written = WRITTEN.lock().unwrap_or_else(|p| p.into_inner());
    if written.as_ref() != Some(&now) {
        *written = Some(now.clone());
        // (the number is taken under the lock above, so the writes are numbered in the order the faults changed)
        let seq = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
        write_in_background(now.clone(), seq);
    }
    now
}

/// The number of the newest write asked for. A write thread that finds a newer number than its own has been overtaken and writes
/// nothing: a fault written and cleared within a few seconds must not leave the old file behind because its two threads finished in the
/// wrong order.
static LATEST: AtomicU64 = AtomicU64::new(0);
/// One file write at a time (with [`LATEST`] this makes the newest write the last).
static WRITING: Mutex<()> = Mutex::new(());

/// Writes `faults` to `path` (removes the file for none) unless a newer write has been asked for meanwhile. Returns whether it wrote.
fn write_ordered(path: &std::path::Path, faults: &[(Part, Why)], seq: u64, latest: &AtomicU64, writing: &Mutex<()>) -> bool {
    let _one = writing.lock().unwrap_or_else(|p| p.into_inner());
    if latest.load(Ordering::SeqCst) != seq {
        return false;
    }
    if faults.is_empty() {
        let _ = std::fs::remove_file(path);
    } else if let Err(e) = std::fs::write(path, encode(faults)) {
        log::warn!("dock parts: the status file could not be written: {e}");
        return false;
    }
    true
}

fn write_in_background(faults: Vec<(Part, Why)>, seq: u64) {
    let Some(path) = file_path() else { return };
    let spawned = std::thread::Builder::new().name("rf-parts-status".into()).spawn(move || {
        write_ordered(&path, &faults, seq, &LATEST, &WRITING);
    });
    if let Err(e) = spawned {
        log::warn!("dock parts: the status writer could not start: {e}");
    }
}

/// The dock is ending: its word goes with it (the file of a dock that is gone must not be read as news by the next one), and no write
/// that is still on its way can bring it back. Called from `shutdown`, which already waits for the taskbar; one small file.
pub fn stop() {
    let seq = LATEST.fetch_add(1, Ordering::SeqCst) + 1;
    if let Some(path) = file_path() {
        let _one = WRITING.lock().unwrap_or_else(|p| p.into_inner());
        if LATEST.load(Ordering::SeqCst) == seq {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// The host started: looks at its parts now and every few seconds for as long as it runs.
pub fn start(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let alive = cx.update(|cx| {
                if !cx.has_global::<super::host::Host>() {
                    return false;
                }
                look(crate::shell::taskbar_hide::fault(cx).map(why_of_fault));
                true
            });
            if !alive {
                break;
            }
            cx.background_executor().timer(HOST_EVERY).await;
        }
    })
    .detach();
}

// ---------------------------------------------------------------- the page's side

struct Seen {
    faults: Vec<(Part, Why)>,
    /// When the page last asked to be kept up to date.
    asked: Option<Instant>,
}

static SEEN: Mutex<Seen> = Mutex::new(Seen { faults: Vec::new(), asked: None });
static LOOPING: AtomicBool = AtomicBool::new(false);

/// What the last look found.
pub fn cached() -> Vec<(Part, Why)> {
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).faults.clone()
}

/// What a look finds: nothing when no dock answers, else the file's faults. Reads a file and writes to a pipe: never on the UI thread.
fn read_now() -> Vec<(Part, Why)> {
    if !super::control::is_running() {
        return Vec::new();
    }
    file_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| decode(&t)).unwrap_or_default()
}

/// Whether the page should keep looking: it was drawn lately, and the switch is on.
fn wants_looking(asked: Option<Instant>, now: Instant, dock_on: bool) -> bool {
    dock_on && asked.is_some_and(|t| now.saturating_duration_since(t) < PAGE_GIVES_UP)
}

/// The Dock page was built: keeps the faults up to date for a while (one background look every two seconds, a redraw only when the
/// answer changed). Called from the page's build, so it costs a lock and a flag.
pub fn poll(cx: &mut App) {
    let on = cx.try_global::<crate::settings::Settings>().is_some_and(|s| s.dock_enabled);
    SEEN.lock().unwrap_or_else(|p| p.into_inner()).asked = Some(Instant::now());
    if !on || LOOPING.swap(true, Ordering::SeqCst) {
        return;
    }
    cx.spawn(async move |cx| {
        loop {
            let found = cx.background_executor().spawn(async { read_now() }).await;
            let again = cx.update(|cx| {
                let changed = {
                    let mut g = SEEN.lock().unwrap_or_else(|p| p.into_inner());
                    let changed = g.faults != found;
                    g.faults = found;
                    changed
                };
                if changed {
                    cx.refresh_windows();
                }
                let on = cx.try_global::<crate::settings::Settings>().is_some_and(|s| s.dock_enabled);
                let asked = SEEN.lock().unwrap_or_else(|p| p.into_inner()).asked;
                wants_looking(asked, Instant::now(), on)
            });
            if !again {
                break;
            }
            cx.background_executor().timer(PAGE_EVERY).await;
        }
        LOOPING.store(false, Ordering::SeqCst);
    })
    .detach();
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the glob of gpui_kit up there brings a `test` of its own that would shadow `#[test]`)
    use super::{AtomicU64, Duration, Instant, Mutex, NOTED, PAGE_GIVES_UP, FILE, Part, Why, decode, encode, merge, note, why_of_fault, wants_looking, write_ordered};
    use std::sync::atomic::Ordering;

    #[test]
    fn the_file_round_trips_and_an_empty_set_is_an_empty_file() {
        let faults = vec![(Part::Taskbar, Why::GuardMissing), (Part::WinKey, Why::HookRefused), (Part::Switcher, Why::HookRefused)];
        assert_eq!(decode(&encode(&faults)), faults);
        assert_eq!(encode(&[]), "");
        assert!(decode("").is_empty());
    }

    #[test]
    fn a_line_that_is_not_understood_is_skipped_and_a_pair_is_listed_once() {
        let text = "taskbar guard-missing\nnonsense words\ntaskbar something-new\n\nwinkey\ntaskbar guard-missing\r\n  tray   not-started  \n";
        assert_eq!(decode(text), [(Part::Taskbar, Why::GuardMissing), (Part::TrayHost, Why::NotStarted)]);
    }

    #[test]
    fn the_faults_to_write_are_the_noted_ones_and_the_taskbars_in_the_order_of_the_parts() {
        assert!(merge(&[], None).is_empty());
        let noted = [(Part::TrayHost, Why::NotStarted), (Part::WinKey, Why::HookRefused)];
        assert_eq!(merge(&noted, Some(Why::GuardLost)), [(Part::Taskbar, Why::GuardLost), (Part::WinKey, Why::HookRefused), (Part::TrayHost, Why::NotStarted)]);
        // the taskbar's own word replaces an older note of the taskbar
        assert_eq!(merge(&[(Part::Taskbar, Why::NotStarted)], Some(Why::GuardMissing)), [(Part::Taskbar, Why::GuardMissing)]);
        // and with no word of its own, a note stays
        assert_eq!(merge(&[(Part::Taskbar, Why::NotStarted)], None), [(Part::Taskbar, Why::NotStarted)]);
    }

    #[test]
    fn the_fault_of_the_taskbar_becomes_the_reason_the_page_tells() {
        use rf_win::taskbar_hide::Fault;
        assert_eq!(why_of_fault(Fault::GuardMissing), Why::GuardMissing);
        assert_eq!(why_of_fault(Fault::GuardNotConfirmed), Why::GuardNotConfirmed);
        assert_eq!(why_of_fault(Fault::GuardLost), Why::GuardLost);
        assert_eq!(why_of_fault(Fault::RecordNotSaved), Why::RecordNotSaved);
    }

    #[test]
    fn a_part_that_started_takes_its_fault_away() {
        // (the notes are process-wide: this test uses a part the other tests of this file do not)
        note(Part::Desktop, Some(Why::NotStarted));
        assert!(NOTED.lock().unwrap().contains(&(Part::Desktop, Why::NotStarted)));
        note(Part::Desktop, Some(Why::NotStarted));
        assert_eq!(NOTED.lock().unwrap().iter().filter(|(p, _)| *p == Part::Desktop).count(), 1, "noted twice is noted once");
        note(Part::Desktop, None);
        assert!(!NOTED.lock().unwrap().iter().any(|(p, _)| *p == Part::Desktop));
    }

    #[test]
    fn a_write_that_was_overtaken_writes_nothing_so_the_newest_word_is_the_one_on_file() {
        let dir = std::env::temp_dir().join(format!("rf-parts-status-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let (latest, lock) = (AtomicU64::new(0), Mutex::new(()));
        let fault = [(Part::Taskbar, Why::GuardMissing)];
        // the fault is written (1) and then cleared (2); the clear's thread happens to run first, the fault's second
        latest.store(2, Ordering::SeqCst);
        assert!(write_ordered(&path, &[], 2, &latest, &lock), "the newest write goes through (nothing to remove here)");
        assert!(!write_ordered(&path, &fault, 1, &latest, &lock), "the older write finds it was overtaken");
        assert!(!path.exists(), "so the file the clear left is not brought back");
        // in the right order the file holds the fault and then the clear removes it
        latest.store(3, Ordering::SeqCst);
        assert!(write_ordered(&path, &fault, 3, &latest, &lock));
        assert_eq!(decode(&std::fs::read_to_string(&path).unwrap()), fault);
        latest.store(4, Ordering::SeqCst);
        assert!(write_ordered(&path, &[], 4, &latest, &lock));
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_page_looks_only_while_it_is_drawn_and_the_switch_is_on() {
        let t0 = Instant::now();
        assert!(wants_looking(Some(t0), t0 + Duration::from_secs(5), true));
        assert!(!wants_looking(Some(t0), t0 + Duration::from_secs(5), false), "the switch is off: nothing to look for");
        assert!(!wants_looking(Some(t0), t0 + PAGE_GIVES_UP + Duration::from_secs(1), true), "not drawn for a long time");
        assert!(!wants_looking(None, t0, true));
    }

    #[test]
    fn the_status_file_lives_in_the_settings_folder_and_carries_words_only() {
        assert_eq!(FILE, "dock-parts.txt");
        let text = encode(&[(Part::Taskbar, Why::GuardMissing)]);
        assert!(!text.contains('\\') && !text.contains(':'), "no path, no name: {text}");
    }
}
