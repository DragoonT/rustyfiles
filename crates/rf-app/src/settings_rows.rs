//! Windows' settings and tools as rows of the search (the dock's Start search and the quick search window share them).
//!
//! The catalogue itself (every Settings page, the settings on the pages, the Control Panel's tasks and tools, in the display
//! language) is Windows' own and is read by `rf_win::settingscat`, once, in the background; until it has been read a search simply
//! has no such rows. This file ranks its entries for a query: pure, with tests, and nothing here opens anything.

use std::sync::{Arc, Mutex};

use rf_win::settingscat::{Entry, Launch};

use crate::quick_search::{Item, Section, Target, fold, score_folded};
use crate::t;

/// The most rows of this kind in a result list.
pub const MAX_SETTINGS: usize = 4;
/// What a Settings page's own entry is worth more than a setting on a page with the same kind of match.
const PAGE_FIRST: i32 = 45;
/// A query shorter than this is matched with names only (one letter is at the start of a hundred search words).
const KEYWORD_QUERY: usize = 3;

/// An entry with its name and words folded once (lower case, no marks), so a keystroke only compares.
struct Folded {
    entry: Entry,
    name: String,
    words: Vec<String>,
}

static FOLDED: Mutex<Option<(usize, Arc<Vec<Folded>>)>> = Mutex::new(None);

/// The catalogue, folded; `None` until Windows' files have been read (the first call starts that and does not wait).
fn folded() -> Option<Arc<Vec<Folded>>> {
    let dir = crate::settings::app_dir()?;
    let cat = rf_win::settingscat::get_or_start(&dir)?;
    let mut held = FOLDED.lock().unwrap_or_else(|p| p.into_inner());
    // (the catalogue is read once in a process: its address is its identity)
    let id = Arc::as_ptr(&cat) as usize;
    if let Some((was, f)) = held.as_ref()
        && *was == id
    {
        return Some(f.clone());
    }
    let f = Arc::new(cat.iter().map(fold_entry).collect::<Vec<_>>());
    *held = Some((id, f.clone()));
    Some(f)
}

fn fold_entry(e: &Entry) -> Folded {
    Folded { name: fold(&e.name), words: e.keywords.split(';').map(|w| fold(w.trim())).filter(|w| !w.is_empty()).collect(), entry: e.clone() }
}

/// How well an entry answers the folded query `q`: its name as every other name is matched; else one of its search words, when the
/// query is that word (or, from three letters on, begins it). A name always ranks above a word.
fn rank(f: &Folded, q: &str) -> Option<i32> {
    if let Some(s) = score_folded(&f.name, q, false) {
        // a Settings page itself before the settings on the pages (seen on the user's copy: "fare" gave "Fare tuşları", a setting
        // of the accessibility page, above "Fare ayarları", the page the word means)
        let page = f.entry.place.is_empty() && !f.entry.launch.is_classic();
        return Some(s + if page { PAGE_FIRST } else { 0 });
    }
    if q.chars().count() < KEYWORD_QUERY {
        return None;
    }
    let mut best = None;
    for w in &f.words {
        let s = if w == q {
            Some(350)
        } else if w.starts_with(q) {
            Some(300)
        } else {
            None
        };
        best = best.max(s);
    }
    best
}

fn rows_of(all: &[Folded], q: &str, lift: &dyn Fn(&Target) -> i32) -> Vec<Item> {
    let mut hits: Vec<(i32, &Folded)> = all.iter().filter_map(|f| rank(f, q).map(|s| (s, f))).collect();
    // the best first; of equals a Settings page before a Control Panel entry, then the shorter name
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.entry.launch.is_classic().cmp(&b.1.entry.launch.is_classic())).then(a.1.name.len().cmp(&b.1.name.len())).then(a.1.name.cmp(&b.1.name)));
    let mut out: Vec<Item> = Vec::new();
    for (s, f) in hits {
        // (the same name twice, a page and a setting on it, or the modern and the classic one: the first, which ranks higher, stands)
        if out.iter().any(|i| fold(&i.title) == f.name) {
            continue;
        }
        let target = Target::Setting { launch: f.entry.launch.encode(), name: f.entry.name.clone() };
        let detail = match where_of(&f.entry) {
            Where::Page(page) => page,
            Where::ControlPanel => t!("Denetim Masası").to_string(),
            Where::Settings => t!("Ayarlar").to_string(),
        };
        out.push(Item { score: s + lift(&target), title: f.entry.name.clone(), detail, section: Section::Settings, target });
        if out.len() == MAX_SETTINGS {
            break;
        }
    }
    out.sort_by_key(|i| std::cmp::Reverse(i.score));
    out
}

/// What stands under a row's name: the page a setting is on, or where the entry lives.
#[derive(Debug, PartialEq, Eq)]
enum Where {
    Page(String),
    ControlPanel,
    Settings,
}

fn where_of(e: &Entry) -> Where {
    if !e.place.is_empty() {
        Where::Page(e.place.clone())
    } else if e.launch.is_classic() {
        Where::ControlPanel
    } else {
        Where::Settings
    }
}

/// The settings rows for the folded query `q` (not blank). `lift` is what the user's own use adds to a row.
pub(crate) fn find(q: &str, lift: &dyn Fn(&Target) -> i32) -> Vec<Item> {
    match folded() {
        Some(all) => rows_of(&all, q, lift),
        None => Vec::new(),
    }
}

/// Starts reading the catalogue (on a low-priority thread of its own; from the small file it takes milliseconds) so that the first
/// search has its rows: without this the first query started the reading and showed nothing of it. Returns at once.
pub(crate) fn warm() {
    if let Some(dir) = crate::settings::app_dir() {
        let _ = rf_win::settingscat::get_or_start(&dir);
    }
}

/// Opens what a settings row stands for (blocks while the system starts it: off the UI thread).
pub(crate) fn open(launch: &str) -> anyhow::Result<()> {
    let launch = Launch::decode(launch).ok_or_else(|| anyhow::anyhow!("not a settings row"))?;
    rf_win::settingscat::open(&launch)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, place: &str, words: &str, launch: Launch) -> Folded {
        fold_entry(&Entry { name: name.into(), place: place.into(), keywords: words.into(), launch })
    }

    fn sample() -> Vec<Folded> {
        vec![
            entry("Ses", "", "ses ayarları;hoparlör;mikrofon", Launch::Page("SettingsPageSound".into())),
            entry("Ses düzeyi karıştırıcısı", "Ses", "ses seviyesi", Launch::Page("SettingsPageSound".into())),
            entry("Ses", "", "ses;hoparlör", Launch::Control("Microsoft.Sound".into())),
            entry("Aygıt Yöneticisi", "", "donanım;sürücü;device", Launch::Command("%windir%\\system32\\mmc.exe devmgmt.msc".into())),
            entry("Yüklü uygulamalar", "", "uygulama kaldır;program kaldır", Launch::Page("SettingsPageInstalledApps".into())),
            entry("Windows Update", "", "güncelleştirme;güncelle", Launch::Page("SettingsPageWindowsUpdate".into())),
            entry("Fare", "", "fare ayarları;imleç hızı", Launch::Page("SettingsPageMouse".into())),
            entry("Fare işaretçisi hızı", "Fare", "", Launch::Page("SettingsPageMouse".into())),
            entry("Ekran", "", "çözünürlük", Launch::Page("SettingsPageDisplay".into())),
            entry("Gece ışığı", "Ekran", "mavi ışık", Launch::Page("SettingsPageDisplay".into())),
            entry("Çevrimdışı dosyaları yönetin", "", "ses", Launch::Control("Microsoft.OfflineFiles".into())),
        ]
    }

    /// The rows' names and how each is opened (the text under a name is in the display language of the moment, which other tests
    /// change while this one runs: it is not compared).
    fn titles(q: &str) -> Vec<(String, String)> {
        rows_of(&sample(), &fold(q), &|_| 0)
            .into_iter()
            .map(|i| match i.target {
                Target::Setting { launch, .. } => (i.title, launch),
                _ => (i.title, String::new()),
            })
            .collect()
    }

    #[test]
    fn what_the_user_typed_is_found_by_name_in_any_spelling() {
        assert_eq!(titles("aygıt yöneticisi")[0].0, "Aygıt Yöneticisi");
        assert_eq!(titles("aygit yon")[0].0, "Aygıt Yöneticisi", "without the Turkish letters");
        assert_eq!(titles("windows update")[0].0, "Windows Update");
        assert_eq!(titles("yüklü uygulamalar")[0].0, "Yüklü uygulamalar");
        assert_eq!(titles("fare")[0], ("Fare".to_string(), "page:SettingsPageMouse".to_string()));
        assert_eq!(titles("gece")[0], ("Gece ışığı".to_string(), "page:SettingsPageDisplay".to_string()), "a setting opens its page");
        let s = sample();
        assert_eq!(where_of(&s[9].entry), Where::Page("Ekran".into()), "and says which page it is on");
        assert_eq!(where_of(&s[3].entry), Where::ControlPanel);
        assert_eq!(where_of(&s[0].entry), Where::Settings);
    }

    #[test]
    fn a_name_ranks_above_a_search_word_and_the_same_name_stands_once() {
        let ses = titles("ses");
        assert_eq!(ses[0], ("Ses".to_string(), "page:SettingsPageSound".to_string()), "the Settings page before the Control Panel's entry of the same name");
        assert_eq!(ses.iter().filter(|(t, _)| t == "Ses").count(), 1);
        assert_eq!(ses[1].0, "Ses düzeyi karıştırıcısı");
        assert!(ses.iter().any(|(t, _)| t == "Çevrimdışı dosyaları yönetin"), "found by its search word, after the names");
        assert_eq!(ses.last().unwrap().0, "Çevrimdışı dosyaları yönetin");
    }

    #[test]
    fn search_words_find_what_the_name_does_not_say() {
        assert_eq!(titles("güncelle")[0].0, "Windows Update");
        assert_eq!(titles("sürücü")[0].0, "Aygıt Yöneticisi");
        assert_eq!(titles("device")[0].0, "Aygıt Yöneticisi", "the English word finds it on a Turkish Windows");
        assert_eq!(titles("mavi ışık")[0].0, "Gece ışığı");
        assert_eq!(titles("hop")[0].0, "Ses", "from three letters on a search word's beginning finds its entry");
        // two letters match names only (a word list would answer almost anything)
        assert!(titles("ho").is_empty());
        assert!(titles("zzzz").is_empty());
    }

    #[test]
    fn a_page_comes_before_the_settings_on_pages() {
        let all = vec![
            entry("Fare tuşları", "Erişilebilirlik fare ayarları", "", Launch::Page("SettingsPageAccessMouse".into())),
            entry("Fare ayarları", "", "", Launch::Page("SettingsPageMouse".into())),
            entry("Fare işaretçisi stili", "Fare işaretçisi ve dokunma", "", Launch::Page("SettingsPagePointer".into())),
        ];
        let rows = rows_of(&all, &fold("fare"), &|_| 0);
        assert_eq!(rows[0].title, "Fare ayarları");
        // but an exact name of a setting still beats a page that only begins with the word
        let rows = rows_of(&all, &fold("fare tuşları"), &|_| 0);
        assert_eq!(rows[0].title, "Fare tuşları");
    }

    #[test]
    fn there_are_at_most_four_rows() {
        let many: Vec<Folded> = (0..30).map(|i| entry(&format!("Ekran ayarı {i}"), "Ekran", "", Launch::Page(format!("P{i}")))).collect();
        assert_eq!(rows_of(&many, &fold("ekran"), &|_| 0).len(), MAX_SETTINGS);
    }

    #[test]
    fn what_the_user_opens_often_comes_first() {
        let rows = rows_of(&sample(), &fold("ses"), &|t| if matches!(t, Target::Setting { name, .. } if name == "Ses düzeyi karıştırıcısı") { 500 } else { 0 });
        assert_eq!(rows[0].title, "Ses düzeyi karıştırıcısı");
    }
}
