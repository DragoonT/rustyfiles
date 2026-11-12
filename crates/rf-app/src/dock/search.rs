//! The dock's search: what the result list holds and how the answers of its sources are put together.
//!
//! The sources and their ranking are the quick search's own (`quick_search::search_local` for apps, bookmarks and the web,
//! `quick_search::search_files` for files). The dock process does not hold the file index, so the files come from the running
//! rustyfiles over a pipe (`search_pipe`) and arrive after the rest. This module is the pure part: the rows for a state of the
//! answers, which row is selected when the rows change under it, and whether Enter has to wait.

use crate::quick_search::{Item, Section, Target, merge};
use crate::search_pipe::Index;

/// What is known about the files for the current query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Files {
    /// No query yet.
    Idle,
    /// Asked, no answer yet.
    Pending,
    /// The main app answered; this is the state of its index.
    Done(Index),
    /// No main rustyfiles is running, so there is no file index at all.
    NoMain,
    /// The user chose to start the main app in the background; it is being waited for.
    Starting,
    /// The main app did not answer in time (busy or hung): the rows are shown without files.
    Silent,
}

/// A row of the result list.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(Item),
    /// "The file index is in the main app": choosing it starts the main app in the background.
    StartMain,
}

impl Entry {
    /// What a row is known by, to keep the selection on it when the rows change.
    pub fn key(&self) -> String {
        match self {
            Entry::Item(i) => i.target.key(),
            Entry::StartMain => "start-main".to_string(),
        }
    }

    pub fn is_app(&self) -> bool {
        matches!(self, Entry::Item(Item { target: Target::App { .. }, .. }))
    }
}

/// The rows for the answers so far: the quick search's sections in its order (apps, files, bookmarks, web); where there is no main
/// app a row that offers to start it stands where the files would be.
pub fn entries(local: &[Item], files: &[Item], state: Files) -> Vec<Entry> {
    let merged = merge(local.to_vec(), files.to_vec());
    let mut out: Vec<Entry> = merged.into_iter().map(Entry::Item).collect();
    if state == Files::NoMain {
        let at = out.iter().position(|e| matches!(e, Entry::Item(i) if i.section > Section::Files)).unwrap_or(out.len());
        out.insert(at, Entry::StartMain);
    }
    out
}

/// Where the selection is: on the first row until the user moves it, then on the row they chose.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Selection {
    #[default]
    First,
    /// A row, by its [`Entry::key`].
    Row(String),
}

impl Selection {
    /// The index of the selected row in `rows`. Unmoved, it is the first row that is a result (the offer to start the main app is
    /// never the default: Enter on a query must not start a program the user did not ask for). A chosen row that is no longer there
    /// (the query changed) falls back to that too.
    pub fn index(&self, rows: &[Entry]) -> usize {
        let default = rows.iter().position(|e| !matches!(e, Entry::StartMain)).unwrap_or(0);
        match self {
            Selection::First => default,
            Selection::Row(k) => rows.iter().position(|e| e.key() == *k).unwrap_or(default),
        }
    }

    /// The selection after the user moved to index `ix` of `rows`.
    pub fn at(rows: &[Entry], ix: usize) -> Selection {
        rows.get(ix).map_or(Selection::First, |e| Selection::Row(e.key()))
    }
}

/// Whether Enter has to wait for the files before it opens anything: the user has not moved the selection, the first row is not an
/// app (an app is a sure thing that does not need the files to be known) and files are still on their way. Without this, Enter in
/// the few milliseconds before the file answer arrives would open the web search for "rapor.docx".
pub fn enter_must_wait(sel: &Selection, rows: &[Entry], files: Files) -> bool {
    *sel == Selection::First && files == Files::Pending && !rows.get(sel.index(rows)).is_some_and(Entry::is_app)
}

/// A file the main app found, as a row: the name, and the folder it is in under it.
pub fn item_of(h: &crate::search_pipe::Hit) -> Item {
    Item {
        target: Target::Path { path: h.path.clone(), dir: h.dir },
        title: crate::quick_search::file_name(&h.path),
        detail: h.path.parent().map(|d| d.display().to_string()).unwrap_or_default(),
        section: Section::Files,
        score: h.score,
    }
}

/// Whether the request for files should be sent: a query with something in it.
pub fn wants_files(query: &str) -> bool {
    !query.trim().is_empty()
}

/// The note under the list for the state of the file index, if there is one to give.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Note {
    IndexBuilding,
    IndexOff,
    MainStarting,
}

pub fn note(files: Files) -> Option<Note> {
    match files {
        Files::Done(Index::Building) => Some(Note::IndexBuilding),
        Files::Done(Index::Off) => Some(Note::IndexOff),
        Files::Starting => Some(Note::MainStarting),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(section: Section, target: Target, title: &str) -> Item {
        Item { target, title: title.into(), detail: String::new(), section, score: 0 }
    }

    fn app(name: &str) -> Item {
        item(Section::Apps, Target::App { id: format!("id-{name}"), name: name.into() }, name)
    }

    fn file(path: &str) -> Item {
        item(Section::Files, Target::Path { path: path.into(), dir: false }, path)
    }

    fn web(q: &str) -> Item {
        item(Section::Web, Target::Web { query: q.into() }, q)
    }

    fn mark(url: &str) -> Item {
        item(Section::Bookmarks, Target::Url { url: url.into() }, url)
    }

    fn titles(rows: &[Entry]) -> Vec<String> {
        rows.iter().map(|e| match e {
            Entry::Item(i) => i.title.clone(),
            Entry::StartMain => "<start main>".into(),
        }).collect()
    }

    #[test]
    fn local_rows_come_at_once_and_the_files_slot_in_between_apps_and_the_rest() {
        let local = vec![app("Notepad"), mark("https://a.example/"), web("not")];
        // before the files: the local rows alone, the web row last
        assert_eq!(titles(&entries(&local, &[], Files::Pending)), ["Notepad", "https://a.example/", "not"]);
        // the files arrive: after the apps, before the bookmarks and the web
        let got = entries(&local, &[file("C:\\notes.txt"), file("C:\\notebook.md")], Files::Done(Index::Ready));
        assert_eq!(titles(&got), ["Notepad", "C:\\notes.txt", "C:\\notebook.md", "https://a.example/", "not"]);
    }

    #[test]
    fn with_no_main_app_one_row_offers_to_start_it_where_the_files_would_be() {
        let local = vec![app("Notepad"), web("not")];
        assert_eq!(titles(&entries(&local, &[], Files::NoMain)), ["Notepad", "<start main>", "not"]);
        // no web row (nothing but apps): the offer is last
        assert_eq!(titles(&entries(&[app("A")], &[], Files::NoMain)), ["A", "<start main>"]);
        // and never when a main app has answered, or is still being asked
        for s in [Files::Idle, Files::Pending, Files::Silent, Files::Starting, Files::Done(Index::Building), Files::Done(Index::Off), Files::Done(Index::Ready)] {
            assert!(!entries(&local, &[], s).contains(&Entry::StartMain), "{s:?}");
        }
    }

    #[test]
    fn the_offer_to_start_the_main_app_is_never_the_default_row() {
        // no app matched and no main app: the offer would be the first row, the web search is the one Enter opens
        let rows = entries(&[web("rapor")], &[], Files::NoMain);
        assert_eq!(titles(&rows), ["<start main>", "rapor"]);
        assert_eq!(Selection::First.index(&rows), 1);
        // but the user can move to it
        let chosen = Selection::at(&rows, 0);
        assert_eq!(chosen.index(&rows), 0);
        // Enter does not wait for files that are not coming
        assert!(!enter_must_wait(&Selection::First, &rows, Files::NoMain));
        // only the offer, nothing else: it is the row there is
        assert_eq!(Selection::First.index(&[Entry::StartMain]), 0);
    }

    #[test]
    fn the_selection_stays_on_the_first_row_until_the_user_moves_it() {
        let a = entries(&[app("A"), web("q")], &[], Files::Pending);
        let sel = Selection::First;
        assert_eq!(sel.index(&a), 0);
        // files arrive: the first row is still the first row
        let b = entries(&[app("A"), web("q")], &[file("C:\\q.txt")], Files::Done(Index::Ready));
        assert_eq!(sel.index(&b), 0);
    }

    #[test]
    fn files_arriving_do_not_move_the_selection_from_the_row_the_user_chose() {
        let local = vec![app("A"), mark("https://b/"), web("q")];
        let before = entries(&local, &[], Files::Pending);
        // the user pressed Down twice: on the web row
        let chosen = Selection::at(&before, 2);
        assert_eq!(titles(&before)[chosen.index(&before)], "q");
        // two files arrive above it: it is now the fifth row, and still the selected one
        let after = entries(&local, &[file("C:\\q.txt"), file("C:\\q2.txt")], Files::Done(Index::Ready));
        assert_eq!(chosen.index(&after), 4);
        assert_eq!(titles(&after)[chosen.index(&after)], "q");
        // a chosen row that went away (the query changed) falls back to the first
        let gone = entries(&[app("Z")], &[], Files::Pending);
        assert_eq!(chosen.index(&gone), 0);
        // an empty list has an index 0 that nothing is at
        assert_eq!(Selection::at(&[], 3), Selection::First);
    }

    #[test]
    fn a_selected_file_stays_selected_when_a_better_file_arrives_before_it() {
        let local = vec![web("q")];
        let first = entries(&local, &[file("C:\\b.txt")], Files::Done(Index::Ready));
        let chosen = Selection::at(&first, 0);
        let second = entries(&local, &[file("C:\\a.txt"), file("C:\\b.txt")], Files::Done(Index::Ready));
        assert_eq!(titles(&second)[chosen.index(&second)], "C:\\b.txt");
    }

    #[test]
    fn enter_waits_for_the_files_only_when_the_best_row_could_still_change() {
        let apps_first = entries(&[app("Notepad"), web("note")], &[], Files::Pending);
        let web_first = entries(&[web("rapor.docx")], &[], Files::Pending);
        // an app is a sure thing: no wait
        assert!(!enter_must_wait(&Selection::First, &apps_first, Files::Pending));
        // only the web row: the files may be better
        assert!(enter_must_wait(&Selection::First, &web_first, Files::Pending));
        // the user chose a row: it is theirs
        assert!(!enter_must_wait(&Selection::at(&web_first, 0), &web_first, Files::Pending));
        // the answer is in (or will never come): open now
        for s in [Files::Done(Index::Ready), Files::NoMain, Files::Silent, Files::Idle, Files::Starting] {
            assert!(!enter_must_wait(&Selection::First, &web_first, s), "{s:?}");
        }
        assert!(enter_must_wait(&Selection::First, &[], Files::Pending), "no rows at all yet");
    }

    #[test]
    fn a_piped_hit_becomes_a_file_row_with_its_folder_under_the_name() {
        let h = crate::search_pipe::Hit { score: 640, dir: true, path: r"C:\Users\Özgür\Belgeler\Çalışma".into() };
        let i = item_of(&h);
        assert_eq!(i.title, "Çalışma");
        assert_eq!(i.detail, r"C:\Users\Özgür\Belgeler");
        assert_eq!((i.section, i.score), (Section::Files, 640));
        assert_eq!(i.target, Target::Path { path: h.path.clone(), dir: true });
        // the local rows and the piped one merge by section whatever order they arrive in
        let rows = entries(&[web("x"), app("A")], &[i], Files::Done(Index::Ready));
        assert_eq!(titles(&rows), ["A", "Çalışma", "x"]);
    }

    #[test]
    fn a_blank_query_asks_for_no_files() {
        assert!(!wants_files(""));
        assert!(!wants_files("   "));
        assert!(wants_files(" a"));
    }

    #[test]
    fn the_index_gives_a_note_when_it_is_not_ready() {
        assert_eq!(note(Files::Done(Index::Building)), Some(Note::IndexBuilding));
        assert_eq!(note(Files::Done(Index::Off)), Some(Note::IndexOff));
        assert_eq!(note(Files::Done(Index::Ready)), None);
        assert_eq!(note(Files::Pending), None);
        assert_eq!(note(Files::NoMain), None, "the offer row says it");
        assert_eq!(note(Files::Starting), Some(Note::MainStarting));
    }

    /// Reads the apps of this PC and prints what the dock's local search costs per keystroke. Windowless and read-only; run with
    /// `cargo test -p rf-app --offline dock::search::tests::query_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn query_cost() {
        crate::background::DOCK_PROCESS.store(true, std::sync::atomic::Ordering::Relaxed);
        let t = std::time::Instant::now();
        let n = crate::quick_search::apps().len();
        eprintln!("apps list (cold): {n} apps, {} ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let _ = crate::quick_search::apps();
        eprintln!("apps list (cached): {} us", t.elapsed().as_micros());
        for q in ["c", "ch", "chr", "chrome", "not", "vsc", "xyzq", "hesap"] {
            let runs = 20;
            let t = std::time::Instant::now();
            let mut rows = 0;
            for _ in 0..runs {
                rows = crate::quick_search::search_local(q, crate::settings::WebSearch::Google, true).len();
            }
            eprintln!("search_local({q:?}) with bookmarks: {} rows, {} us each", rows, t.elapsed().as_micros() / runs as u128);
        }
        let t = std::time::Instant::now();
        let rows = crate::quick_search::search_local("not", crate::settings::WebSearch::Google, false);
        let merged = entries(&rows, &[], Files::Pending);
        eprintln!("entries: {} rows ({} us, first call incl. nothing cached)", merged.len(), t.elapsed().as_micros());
    }
}
