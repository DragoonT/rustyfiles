//! Quick search (Alt+Space by default): a small window over whatever app is in front that finds
//! installed apps, files and folders (rustyfiles' name index), Chrome / Edge bookmarks, and
//! offers a web search. Enter opens, Ctrl+Enter shows the item in its folder, Shift+Enter
//! previews it with DirectLook, Esc closes. What is opened often comes first, and what was chosen
//! for the same typed letters before comes first of all ("En iyi eşleşme": see `Picks`).
//!
//! The hotkey is registered by the running rustyfiles (`rf_win::hotkey`); with "keep running in
//! the background" on it works while no window is open.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use serde::{Deserialize, Serialize};

use crate::image_cache::ImageCache;
use crate::settings::{Settings, WebSearch};
use crate::t;
use crate::theme::{self, Palette};

/// The shortcut editor records into the quick-search hotkey under this name (keymap_editor).
pub const CAPTURE_ACTION: &str = "@quick-search";
/// `rustyfiles.exe --quick-search` opens it too (in the running rustyfiles).
pub const LAUNCH_ARG: &str = "--quick-search";

const WIDTH: f32 = 680.;
const ROW_H: f32 = 46.;
/// What a match on an app's program file name is worth less than the same match on its display name.
const FILE_NAME_MATCH: i32 = 60;
/// A run command's rank: under an app whose own name is exactly the query (1000), above every other app.
const RUN_COMMAND: i32 = 990;
/// Results per kind.
const MAX_APPS: usize = 4;
const MAX_FILES: usize = 7;
const MAX_BOOKMARKS: usize = 3;
/// An app list older than this is refreshed (in the background) when the window opens.
const APPS_STALE: Duration = Duration::from_secs(300);

// ---------------------------------------------------------------------------------------------
// The hotkey

/// The shortcut held now (None: none) and whether registering it failed.
static HELD: Mutex<Option<(Option<String>, Option<rf_win::hotkey::HotkeyError>)>> = Mutex::new(None);

static NO_HOTKEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A side copy of rustyfiles (the taskbar preview) must not take the user's hotkey.
pub fn disable_hotkey() {
    NO_HOTKEY.store(true, std::sync::atomic::Ordering::Relaxed);
}

/// This process registers the hotkey: the one rustyfiles (not DirectLook, not a test copy). `RUSTYFILES_QUICK_HOTKEY=1` lets a test
/// copy hold it like the real one (a copy the user tries the next version with, while the installed app is not running); without it a
/// test copy never takes the user's hotkey.
fn hotkey_owner() -> bool {
    let test_copy = std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_some() || std::env::var_os("RUSTYFILES_INSTANCE").is_some();
    !crate::background::DIRECTLOOK_PROCESS.load(std::sync::atomic::Ordering::Relaxed)
        && !NO_HOTKEY.load(std::sync::atomic::Ordering::Relaxed)
        && (!test_copy || std::env::var_os("RUSTYFILES_QUICK_HOTKEY").is_some())
}

/// Apps, files and pages being started on other threads: the app does not quit under them
/// (started only for the quick search, it quits when its window closes).
static LAUNCHING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub fn launching() -> usize {
    LAUNCHING.load(std::sync::atomic::Ordering::Relaxed)
}

/// Runs `f` on a thread of its own, counted in `launching`.
fn launch(f: impl FnOnce() + Send + 'static) {
    LAUNCHING.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::thread::spawn(move || {
        f();
        LAUNCHING.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    });
}

/// Settings applied: holds the configured hotkey (or none). Registering waits for the hotkey
/// thread, a few microseconds.
pub fn sync(s: &Settings) {
    if !hotkey_owner() {
        return;
    }
    let want = s.quick_search.then(|| s.quick_search_keys.clone()).filter(|k| !k.trim().is_empty());
    let mut held = HELD.lock().unwrap();
    // (a shortcut another app held is tried again: that app may have let it go)
    if held.as_ref().is_some_and(|(k, e)| *k == want && e.is_none()) {
        return;
    }
    let r = rf_win::hotkey::set(want.as_deref());
    if let Err(e) = &r {
        log::warn!("quick search hotkey {want:?}: {e:?}");
    }
    *held = Some((want, r.err()));
}

/// Lets the next `sync` register again (after the shortcut editor let go of the keys).
pub fn resync(cx: &App) {
    *HELD.lock().unwrap() = None;
    sync(cx.global::<Settings>());
}

/// The shortcut editor is recording a new hotkey: the old one must not swallow the keys.
pub fn release_for_capture() {
    if !hotkey_owner() {
        return;
    }
    let _ = rf_win::hotkey::set(None);
    *HELD.lock().unwrap() = Some((None, None));
}

/// Why the configured hotkey does not work, if it doesn't.
pub fn problem() -> Option<rf_win::hotkey::HotkeyError> {
    HELD.lock().unwrap().as_ref().and_then(|(_, e)| e.clone())
}

/// Routes hotkey presses to the app and warms the app list (call once at startup).
pub fn init(cx: &mut App) {
    if !hotkey_owner() {
        return;
    }
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
    rf_win::hotkey::set_handler(move || {
        let _ = tx.unbounded_send(());
    });
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while rx.next().await.is_some() {
            log::info!("quick search: the hotkey was pressed");
            cx.update(toggle);
        }
    })
    .detach();
    // the first press should not wait for "All apps" (about a second cold)
    if cx.global::<Settings>().quick_search {
        std::thread::Builder::new()
            .name("quick-search-warm".into())
            .spawn(|| {
                std::thread::sleep(Duration::from_secs(8));
                let _ = apps();
                // (and Windows' settings catalogue: the first search then has its rows)
                crate::settings_rows::warm();
            })
            .ok();
    }
}

// ---------------------------------------------------------------------------------------------
// Sources

static APPS: Mutex<Option<(Instant, Arc<Vec<rf_win::apps::App>>)>> = Mutex::new(None);
/// One enumeration at a time (each keystroke's search would start its own).
static LISTING: Mutex<()> = Mutex::new(());

/// Reads "All apps" from the shell. In the dock process this runs on the one shell thread that pumps messages: a worker thread that
/// asks the shell becomes an apartment that never pumps, and several of those asking at once hang the shell (see `rf_win::sta::call`).
/// The main app has always asked from its own worker threads and keeps doing so.
fn read_apps() -> anyhow::Result<Vec<rf_win::apps::App>> {
    if crate::background::DOCK_PROCESS.load(std::sync::atomic::Ordering::Relaxed) {
        let limit = Duration::from_secs(20);
        return rf_win::sta::call(limit, rf_win::apps::list).unwrap_or_else(|| anyhow::bail!("the shell did not list the apps in {limit:?}"));
    }
    rf_win::apps::list()
}

/// "All apps": cached; a stale list is served while a fresh one is read. Blocks while the very first list is read (a second to
/// start with): not on the UI thread.
pub(crate) fn apps() -> Arc<Vec<rf_win::apps::App>> {
    let cached = APPS.lock().unwrap().clone();
    match cached {
        Some((at, list)) => {
            if at.elapsed() > APPS_STALE {
                *APPS.lock().unwrap() = Some((Instant::now(), list.clone()));
                std::thread::spawn(|| {
                    if let Ok(l) = read_apps() {
                        *APPS.lock().unwrap() = Some((Instant::now(), Arc::new(l)));
                    }
                });
            }
            list
        }
        None => {
            let _one = LISTING.lock().unwrap();
            if let Some((_, list)) = APPS.lock().unwrap().clone() {
                return list;
            }
            match read_apps() {
                Ok(l) if !l.is_empty() => {
                    let list = Arc::new(l);
                    *APPS.lock().unwrap() = Some((Instant::now(), list.clone()));
                    list
                }
                // not cached: the shell may not be ready yet, the next search asks again
                _ => Arc::new(Vec::new()),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Bookmark {
    pub name: String,
    pub url: String,
}

/// Chrome's and Edge's bookmark files (every profile), with their change times: re-read only
/// when one changes.
static BOOKMARKS: Mutex<Option<(Vec<(PathBuf, Option<SystemTime>)>, Arc<Vec<Bookmark>>)>> = Mutex::new(None);

/// The browsers' profile list changes rarely: read at most every 30 s (not per keystroke).
fn bookmark_files() -> Vec<PathBuf> {
    static FILES: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    if let Some((at, f)) = FILES.lock().unwrap().as_ref() {
        if at.elapsed() < Duration::from_secs(30) {
            return f.clone();
        }
    }
    let f = find_bookmark_files();
    *FILES.lock().unwrap() = Some((Instant::now(), f.clone()));
    f
}

fn find_bookmark_files() -> Vec<PathBuf> {
    let Some(local) = dirs::data_local_dir() else { return Vec::new() };
    let mut out = Vec::new();
    for browser in ["Google\\Chrome", "Microsoft\\Edge"] {
        let root = local.join(browser).join("User Data");
        // the profiles the browser knows (Local State → profile.info_cache), else Default
        let profiles: Vec<String> = std::fs::read(root.join("Local State"))
            .ok()
            .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
            .and_then(|v| v.pointer("/profile/info_cache").and_then(|c| c.as_object()).map(|o| o.keys().cloned().collect()))
            .unwrap_or_else(|| vec!["Default".to_string()]);
        for p in profiles {
            let f = root.join(p).join("Bookmarks");
            if f.is_file() {
                out.push(f);
            }
        }
    }
    out
}

fn collect_urls(node: &serde_json::Value, out: &mut Vec<Bookmark>) {
    match node.get("type").and_then(|t| t.as_str()) {
        Some("url") => {
            let url = node.get("url").and_then(|u| u.as_str()).unwrap_or("");
            if url.starts_with("https://") || url.starts_with("http://") {
                let name = node.get("name").and_then(|n| n.as_str()).unwrap_or("").trim();
                out.push(Bookmark { name: if name.is_empty() { url.to_string() } else { name.to_string() }, url: url.to_string() });
            }
        }
        _ => {
            if let Some(children) = node.get("children").and_then(|c| c.as_array()) {
                for c in children {
                    collect_urls(c, out);
                }
            }
        }
    }
}

/// The bookmarks of one browser profile (`Bookmarks`, the browser's JSON).
pub fn parse_bookmarks(json: &[u8]) -> Vec<Bookmark> {
    let mut out = Vec::new();
    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(json) {
        if let Some(roots) = v.get("roots").and_then(|r| r.as_object()) {
            for root in roots.values().filter(|r| r.is_object()) {
                collect_urls(root, &mut out);
            }
        }
    }
    out
}

fn bookmarks() -> Arc<Vec<Bookmark>> {
    let files: Vec<(PathBuf, Option<SystemTime>)> =
        bookmark_files().into_iter().map(|f| { let m = std::fs::metadata(&f).ok().and_then(|m| m.modified().ok()); (f, m) }).collect();
    if let Some((stamp, list)) = BOOKMARKS.lock().unwrap().as_ref() {
        if *stamp == files {
            return list.clone();
        }
    }
    let mut all = Vec::new();
    for (f, _) in &files {
        if let Ok(b) = std::fs::read(f) {
            all.extend(parse_bookmarks(&b));
        }
    }
    // the same page saved in two browsers or profiles shows once
    let mut seen = std::collections::HashSet::new();
    all.retain(|b| seen.insert(b.url.clone()));
    let list = Arc::new(all);
    *BOOKMARKS.lock().unwrap() = Some((files, list.clone()));
    list
}

// ---------------------------------------------------------------------------------------------
// What was opened, and how often (`quicksearch.json` next to settings.toml)

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Use {
    count: u32,
    /// Seconds since 1970.
    last: u64,
    title: String,
    detail: String,
    target: Target,
}

#[derive(Default)]
struct Usage {
    map: HashMap<String, Use>,
    /// What was chosen for what was typed.
    picks: Picks,
    loaded: bool,
    /// The dates of the two files when they were read: a file that another rustyfiles process has written since is read again.
    stamps: [Option<SystemTime>; 2],
}

static USAGE: std::sync::LazyLock<Mutex<Usage>> = std::sync::LazyLock::new(|| Mutex::new(Usage::default()));

fn usage_path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("quicksearch.json"))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Reads the use counts from the file again at the next search. The dock calls it when its panel opens: the main app writes the same
/// file, and a process that loaded it once would otherwise rank by what it saw at its own start.
pub(crate) fn reload_usage() {
    USAGE.lock().unwrap().loaded = false;
}

fn file_stamp(path: Option<PathBuf>) -> Option<SystemTime> {
    path.and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}

/// The use counts and the picks, read from their files when they were not read yet or when a file's date is not the one they were
/// read at: the dock and the main app are two processes that both write them (a choice made in the dock's Start must count in the
/// main app's file search at the next keystroke, not at the next start). Two looks at a file's date for every search.
fn with_store<R>(f: impl FnOnce(&mut Usage) -> R) -> R {
    let mut u = USAGE.lock().unwrap();
    let stamps = [file_stamp(usage_path()), file_stamp(picks_path())];
    if !u.loaded || u.stamps != stamps {
        u.loaded = true;
        u.stamps = stamps;
        u.map.clear();
        if let Some(text) = usage_path().and_then(|p| std::fs::read(p).ok()) {
            // entry by entry: one this version can't read does not cost the others
            let raw: HashMap<String, serde_json::Value> = serde_json::from_slice(&text).unwrap_or_default();
            u.map = raw.into_iter().filter_map(|(k, v)| serde_json::from_value(v).ok().map(|u| (k, u))).collect();
        }
        u.picks = picks_path().and_then(|p| std::fs::read(p).ok()).map(|text| parse_picks(&text)).unwrap_or_default();
    }
    f(&mut u)
}

fn with_usage<R>(f: impl FnOnce(&mut HashMap<String, Use>) -> R) -> R {
    with_store(|u| f(&mut u.map))
}

/// How much use lifts an item: grows with the count, fades over weeks without use.
fn boost(u: &Use) -> i32 {
    let days = now_secs().saturating_sub(u.last) as f64 / 86_400.0;
    let recency = 1.0 / (1.0 + days / 14.0);
    ((u.count as f64).ln_1p() * 60.0 * recency).min(240.0) as i32
}

/// The item was opened after `query` was typed: its use is counted ([`remember`]) and the choice is kept for those letters
/// ([`remember_pick`]).
pub(crate) fn remember_for(query: &str, item: &Item) {
    remember(item);
    remember_pick(query, &item.target);
}

pub(crate) fn remember(item: &Item) {
    // a typed web search is not something to offer again (and stays off the disk)
    if matches!(item.target, Target::Web { .. }) {
        return;
    }
    let key = item.target.key();
    let snapshot = with_usage(|m| {
        let e = m.entry(key).or_insert_with(|| Use { count: 0, last: 0, title: item.title.clone(), detail: item.detail.clone(), target: item.target.clone() });
        e.count += 1;
        e.last = now_secs();
        e.title = item.title.clone();
        e.detail = item.detail.clone();
        // a long tail of one-off items is not worth keeping
        if m.len() > 600 {
            let mut v: Vec<(String, i32, u64)> = m.iter().map(|(k, u)| (k.clone(), boost(u), u.last)).collect();
            v.sort_by_key(|(_, b, last)| (*b, *last));
            for (k, _, _) in v.into_iter().take(m.len() - 500) {
                m.remove(&k);
            }
        }
        serde_json::to_vec(m).ok()
    });
    if let (Some(bytes), Some(path)) = (snapshot, usage_path()) {
        std::thread::spawn(move || {
            static WRITE: Mutex<()> = Mutex::new(());
            let _g = WRITE.lock().unwrap();
            let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// What was chosen for what was typed (`quicksearch-picks.json` next to settings.toml)
//
// The use counts above say what is opened often. They do not say what the user MEANS by two letters: "ph" matches Phone Link by its
// first letters and Photoshop by a word, and the one the user opens after typing "ph" is the answer. So every choice made with a
// query in the box is kept under that query (folded, as the matching folds it), and at the next search:
// * an item chosen for these letters is lifted in its own section (`pick_points`, up to 800: above a better match by name), and
// * the item the picks speak for most becomes the first row, whatever its kind, under "En iyi eşleşme" (`merge_with`): Enter opens
//   what was opened the last times.
// A choice counts in full for the very letters it was made with, less for a shorter or a longer query that begins the same way
// (typed "pho" then, "ph" now), and fades over months without use. A web search is never kept (as its use is not): what was typed
// to be looked up on the web stays off the disk.

/// One choice: how often, and when last (seconds since 1970).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct Pick {
    count: u32,
    last: u64,
}

/// Folded query -> the key of what was opened (`Target::key`) -> the choice.
pub(crate) type Picks = HashMap<String, HashMap<String, Pick>>;

/// How many typed queries are kept, and how many choices under each.
const MAX_PICK_QUERIES: usize = 400;
const MAX_PICKS_PER_QUERY: usize = 6;
/// A query is kept up to this many characters (a longer one is a sentence, not a habit).
const MAX_PICK_QUERY_CHARS: usize = 48;
/// What a choice is worth for a query that is not the one it was made with: the user typed more then (and less now), or less then.
const PICK_TYPED_MORE: f64 = 0.6;
const PICK_TYPED_LESS: f64 = 0.4;
/// A choice loses half its weight in this many days without being made again.
const PICK_HALF_DAYS: f64 = 60.0;
/// The weight from which an item is the best match: one choice for these very letters within about two months, or more than one
/// for letters that begin the same way.
const BEST_MIN: f64 = 0.5;

fn picks_path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("quicksearch-picks.json"))
}

/// The picks file (a query whose entry this version cannot read does not cost the others).
pub(crate) fn parse_picks(json: &[u8]) -> Picks {
    let raw: HashMap<String, serde_json::Value> = serde_json::from_slice(json).unwrap_or_default();
    raw.into_iter().filter_map(|(q, v)| serde_json::from_value::<HashMap<String, Pick>>(v).ok().map(|m| (q, m))).filter(|(_, m)| !m.is_empty()).collect()
}

/// The query as the picks know it: folded like the matching folds it, without the spaces around it, not longer than a habit is.
pub(crate) fn pick_query(query: &str) -> String {
    fold(query.trim()).chars().take(MAX_PICK_QUERY_CHARS).collect()
}

/// Notes that `key` was opened after `query` was typed, at `now`. False when there is nothing to note (no query). Keeps the list
/// small: the weakest choice of a query goes when it has more than [`MAX_PICKS_PER_QUERY`], and the queries not chosen from for the
/// longest go when there are more than [`MAX_PICK_QUERIES`].
pub(crate) fn note_pick(picks: &mut Picks, query: &str, key: &str, now: u64) -> bool {
    let q = pick_query(query);
    if q.is_empty() {
        return false;
    }
    let chosen = picks.entry(q).or_default();
    let p = chosen.entry(key.to_string()).or_insert(Pick { count: 0, last: 0 });
    p.count = p.count.saturating_add(1);
    p.last = now;
    while chosen.len() > MAX_PICKS_PER_QUERY {
        let Some(weakest) = chosen.iter().filter(|(k, _)| k.as_str() != key).min_by_key(|(_, p)| (p.count, p.last)).map(|(k, _)| k.clone()) else { break };
        chosen.remove(&weakest);
    }
    if picks.len() > MAX_PICK_QUERIES {
        let mut by_age: Vec<(u64, String)> = picks.iter().map(|(q, m)| (m.values().map(|p| p.last).max().unwrap_or(0), q.clone())).collect();
        by_age.sort();
        for (_, q) in by_age.into_iter().take(picks.len() - MAX_PICK_QUERIES * 9 / 10) {
            picks.remove(&q);
        }
    }
    true
}

/// What the picks say for the folded query `q` at `now`: the key of each item that was chosen for these letters, or for letters that
/// begin the same way, and how much speaks for it. One pass over the picks for a search, not one for every candidate.
pub(crate) fn pick_weights(picks: &Picks, q: &str, now: u64) -> HashMap<String, f64> {
    let mut out: HashMap<String, f64> = HashMap::new();
    if q.is_empty() {
        return out;
    }
    for (typed, chosen) in picks {
        let closeness = if typed == q {
            1.0
        } else if typed.starts_with(q) {
            PICK_TYPED_MORE
        } else if q.starts_with(typed.as_str()) {
            PICK_TYPED_LESS
        } else {
            continue;
        };
        for (key, p) in chosen {
            let days = now.saturating_sub(p.last) as f64 / 86_400.0;
            *out.entry(key.clone()).or_default() += closeness * p.count as f64 * 0.5f64.powf(days / PICK_HALF_DAYS);
        }
    }
    out
}

/// What a weight lifts an item by in its own section: 277 for one fresh choice (more than the step from a match at a word's start to
/// a match at the name's start, so one choice is enough to put the chosen one first), up to 800.
pub(crate) fn pick_points(weight: f64) -> i32 {
    (weight.max(0.0).ln_1p() * 400.0).min(800.0) as i32
}

/// The key of the item that would be the best match for these weights, if any is: the heaviest from [`BEST_MIN`] on.
fn best_key(weights: &HashMap<String, f64>) -> Option<&String> {
    weights.iter().filter(|(_, w)| **w >= BEST_MIN).max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal).then_with(|| b.0.cmp(a.0))).map(|(k, _)| k)
}

/// What the picks say for a query as typed (read from the store).
fn weights_for(query: &str) -> HashMap<String, f64> {
    let q = pick_query(query);
    with_store(|u| pick_weights(&u.picks, &q, now_secs()))
}

/// Whether the item the picks speak for most, for this query, is a file or a folder: the dock's search gets its files a moment after
/// the rest, and Enter in that moment must wait for them (the first row is about to change).
pub(crate) fn best_is_a_file(query: &str) -> bool {
    best_key(&weights_for(query)).is_some_and(|k| k.starts_with("path:"))
}

/// Keeps the choice of `target` for the typed `query` (not a web search, not without a query). The use count is another matter
/// ([`remember`]).
pub(crate) fn remember_pick(query: &str, target: &Target) {
    if matches!(target, Target::Web { .. }) {
        return;
    }
    let key = target.key();
    let snapshot = with_store(|u| note_pick(&mut u.picks, query, &key, now_secs()).then(|| serde_json::to_vec(&u.picks).ok()).flatten());
    if let (Some(bytes), Some(path)) = (snapshot, picks_path()) {
        std::thread::spawn(move || {
            static WRITE: Mutex<()> = Mutex::new(());
            let _g = WRITE.lock().unwrap();
            let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Matching and ranking (pure)

/// Lower case without Turkish / accented letters' marks: "Şarkı" and "sarki" match.
pub fn fold(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .filter(|c| *c != '\u{307}')
        .map(|c| match c {
            'ş' => 's',
            'ğ' => 'g',
            'ü' | 'û' => 'u',
            'ö' => 'o',
            'ç' => 'c',
            'ı' | 'î' => 'i',
            'â' | 'á' | 'à' | 'ä' => 'a',
            'é' | 'è' | 'ê' | 'ë' => 'e',
            c => c,
        })
        .collect()
}

/// How well `name` matches the folded query `q`: whole name, start, start of a word, anywhere.
/// `None`: no match.
pub fn score(name: &str, q: &str) -> Option<i32> {
    score_with(name, q, false)
}

/// `score`, and for app names also the initials of their words ("vsc" → Visual Studio Code;
/// for page titles and file names that matched too much by chance).
pub(crate) fn score_with(name: &str, q: &str, initials: bool) -> Option<i32> {
    score_folded(&fold(name), q, initials)
}

/// [`score_with`] for a name that is folded already (a list of many names folds them once and compares at every keystroke).
pub(crate) fn score_folded(n: &str, q: &str, initials: bool) -> Option<i32> {
    if q.is_empty() {
        return Some(0);
    }
    let short = (40 - n.chars().count() as i32).max(0);
    if n == q {
        return Some(1000);
    }
    if n.starts_with(q) {
        return Some(800 + short);
    }
    let is_start = |i: usize| i == 0 || !n[..i].chars().last().is_some_and(char::is_alphanumeric);
    if let Some(i) = n.match_indices(q).map(|(i, _)| i).find(|&i| is_start(i)) {
        let _ = i;
        return Some(600 + short);
    }
    if let Some(i) = n.find(q) {
        return Some(400 - (i as i32).min(100) + short / 2);
    }
    if !initials {
        return None;
    }
    // initials: each query letter starts a word, in order
    let starts: Vec<char> = n.char_indices().filter(|(i, _)| is_start(*i)).map(|(_, c)| c).collect();
    let mut it = starts.iter();
    if q.chars().all(|qc| it.any(|&c| c == qc)) && q.chars().count() >= 2 {
        return Some(300 + short / 2);
    }
    None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Target {
    App { id: String, name: String },
    Path { path: PathBuf, dir: bool },
    Url { url: String },
    Web { query: String },
    /// A page of Windows' Settings, a setting on one, or a Control Panel task or tool (`settings_rows.rs`): how it is opened, as
    /// `rf_win::settingscat::Launch` writes it, and its name.
    Setting { launch: String, name: String },
}

impl Target {
    pub(crate) fn key(&self) -> String {
        match self {
            Target::App { id, .. } => format!("app:{id}"),
            Target::Path { path, .. } => format!("path:{}", path.to_string_lossy().to_lowercase()),
            Target::Url { url } => format!("url:{url}"),
            Target::Web { query } => format!("web:{query}"),
            Target::Setting { launch, name } => format!("setting:{launch}:{}", name.to_lowercase()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Section {
    Recent,
    /// The one item the picks speak for most for this query (`merge_with`): first, whatever its kind.
    Best,
    Apps,
    /// Windows' settings and tools: before the files, as Windows' own search has them (seven files named like the query would push
    /// the page the user wants out of sight).
    Settings,
    Files,
    Bookmarks,
    Web,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub target: Target,
    pub title: String,
    pub detail: String,
    pub section: Section,
    pub score: i32,
}

/// Everything found for one query, in the order shown.
pub struct Found {
    pub items: Vec<Item>,
    /// The file index is still being built: files may be missing.
    pub index_building: bool,
    /// No file index (turned off in Settings): no files at all.
    pub index_off: bool,
}

pub(crate) fn web_url(engine: WebSearch, q: &str) -> String {
    let enc: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect();
    match engine {
        WebSearch::Google => format!("https://www.google.com/search?q={enc}"),
        WebSearch::Bing => format!("https://www.bing.com/search?q={enc}"),
        WebSearch::DuckDuckGo => format!("https://duckduckgo.com/?q={enc}"),
        WebSearch::Yandex => format!("https://yandex.com/search/?text={enc}"),
    }
}

pub(crate) fn engine_name(engine: WebSearch) -> &'static str {
    match engine {
        WebSearch::Google => "Google",
        WebSearch::Bing => "Bing",
        WebSearch::DuckDuckGo => "DuckDuckGo",
        WebSearch::Yandex => "Yandex",
    }
}

pub(crate) fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// Searches every source for `query` (blocks: run off the UI thread).
fn search(query: &str, engine: WebSearch, with_bookmarks: bool, index_on: bool) -> Found {
    if fold(query.trim()).is_empty() {
        // nothing typed: what was opened most lately
        return Found { items: recent(), index_building: false, index_off: false };
    }
    let local = search_local(query, engine, with_bookmarks);
    let files = search_files(query, index_on);
    Found { items: merge(query, local, files.items), index_building: files.index_building, index_off: files.index_off }
}

/// What was opened most lately, for an empty query.
pub(crate) fn recent() -> Vec<Item> {
    let used: HashMap<String, Use> = with_usage(|m| m.clone());
    let mut v: Vec<&Use> = used.values().filter(|u| !matches!(u.target, Target::Web { .. })).collect();
    v.sort_by_key(|u| std::cmp::Reverse(boost(u)));
    // a file deleted since is left out
    v.into_iter()
        .filter(|u| !matches!(&u.target, Target::Path { path, .. } if !path.exists()))
        .take(8)
        .map(|u| Item { target: u.target.clone(), title: u.title.clone(), detail: u.detail.clone(), section: Section::Recent, score: 0 })
        .collect()
}

/// The sources that need no file index and answer at once: installed apps, bookmarks (when asked for) and the web search, in that
/// order. The dock's search shows these on every keystroke and the files when they arrive. `query` must not be blank.
pub(crate) fn search_local(query: &str, engine: WebSearch, with_bookmarks: bool) -> Vec<Item> {
    let q = fold(query.trim());
    let (_, lift) = lifts(query);
    let mut items = Vec::new();
    // apps: by their display name, and by the file name of their program (`cmd` finds "Komut İstemi"; see `run_names.rs`). A match on
    // the program's name ranks a little below the same kind of match on the display name.
    let listed = apps();
    let mut a: Vec<Item> = listed
        .iter()
        .filter_map(|app| {
            let t = Target::App { id: app.id.clone(), name: app.name.clone() };
            let by_name = score_with(&app.name, &q, true);
            let by_file = crate::run_names::app_file_stem(&app.id).and_then(|stem| score(&stem, &q)).map(|s| s - FILE_NAME_MATCH);
            let s = by_name.max(by_file)?;
            Some(Item { score: s + lift(&t), title: app.name.clone(), detail: String::new(), section: Section::Apps, target: t })
        })
        .collect();
    // a run command, as Windows' search and the Run box have it: the query IS the name of a program (`dxdiag`, `regedit`,
    // `services.msc`). Not offered when an app in the list is that very program (the app's own row stands for it).
    if let Some(exe) = crate::run_names::resolve(query.trim()) {
        let wanted = crate::run_names::stem_of(query.trim());
        let an_app_is_it = listed.iter().any(|app| crate::run_names::app_file_stem(&app.id).is_some_and(|s| s == wanted));
        if !an_app_is_it {
            let folder = exe.parent().map(|d| d.display().to_string()).unwrap_or_default();
            let t = Target::Path { path: exe.clone(), dir: false };
            a.push(Item { score: RUN_COMMAND + lift(&t), title: file_name(&exe), detail: format!("{} · {folder}", t!("Komut")), section: Section::Apps, target: t });
        }
    }
    a.sort_by_key(|i| std::cmp::Reverse(i.score));
    items.extend(a.into_iter().take(MAX_APPS));
    // Windows' settings and tools, from Windows' own catalogue (read in the background the first time: no rows until it is there)
    items.extend(crate::settings_rows::find(&q, &lift));
    // bookmarks
    if with_bookmarks {
        let mut b: Vec<Item> = bookmarks()
            .iter()
            .filter_map(|bm| {
                let host = bm.url.split("://").nth(1).unwrap_or("").split('/').next().unwrap_or("").trim_start_matches("www.");
                let s = score(&bm.name, &q).or_else(|| score(host, &q).map(|s| s - 100))?;
                let t = Target::Url { url: bm.url.clone() };
                Some(Item { score: s + lift(&t), title: bm.name.clone(), detail: host.to_string(), section: Section::Bookmarks, target: t })
            })
            .collect();
        b.sort_by_key(|i| std::cmp::Reverse(i.score));
        items.extend(b.into_iter().take(MAX_BOOKMARKS));
    }
    // the web, always last
    items.push(Item {
        target: Target::Web { query: query.trim().to_string() },
        title: query.trim().to_string(),
        detail: engine_name(engine).to_string(),
        section: Section::Web,
        score: 0,
    });
    items
}

/// What the file index found for a query.
pub(crate) struct FilesFound {
    pub items: Vec<Item>,
    /// The file index is still being built: files may be missing.
    pub index_building: bool,
    /// No file index (turned off in Settings): no files at all.
    pub index_off: bool,
}

/// Files and folders from the name index, every indexed drive, ranked (blocks: off the UI thread). Needs the index, which only the
/// main app loads. `query` must not be blank.
pub(crate) fn search_files(query: &str, index_on: bool) -> FilesFound {
    let q = fold(query.trim());
    let (used, lift) = lifts(query);
    let mut index_building = false;
    let status = rf_win::indexer::status();
    // on in Settings but not started yet (it starts a few seconds after rustyfiles): coming
    let index_off = !index_on;
    if index_on && status.is_empty() {
        index_building = true;
    }
    // a wildcard query ("*.psd") is the index's glob: its hits all match equally
    let glob = query.contains(['*', '?']);
    let mut hits: Vec<(i32, PathBuf)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |p: PathBuf, hits: &mut Vec<(i32, PathBuf)>| {
        if !seen.insert(p.clone()) {
            return;
        }
        let s = if glob { Some(500) } else { score(&file_name(&p), &q) };
        if let Some(s) = s {
            // a shallower path is likelier what is meant
            let depth = p.components().count() as i32;
            hits.push((s - depth * 4, p));
        }
    };
    for (root, state, _, _) in status {
        match state {
            rf_win::indexer::State::Ready => {}
            rf_win::indexer::State::Building => {
                index_building = true;
                continue;
            }
            _ => continue,
        }
        // the best names first, whatever their place in the index (a common fragment matches
        // tens of thousands of files)
        let rank = |name: &str| if glob { Some(500) } else { score(name, &q) };
        if let Some(found) = rf_win::indexer::search_ranked(query.trim(), &root, false, 60, &rank) {
            for (_, p) in found {
                add(p, &mut hits);
            }
        }
    }
    // what was opened before is always a candidate (the cap above could leave it out); only a
    // name that matches is looked for on the disk
    for u in used.values() {
        if let Target::Path { path, .. } = &u.target
            && (glob || score(&file_name(path), &q).is_some())
            && path.exists()
        {
            add(path.clone(), &mut hits);
        }
    }
    let mut f: Vec<Item> = hits
        .into_iter()
        .map(|(s, p)| {
            let target = Target::Path { path: p.clone(), dir: false };
            Item { score: s + lift(&target), title: file_name(&p), detail: p.parent().map(|d| d.display().to_string()).unwrap_or_default(), section: Section::Files, target }
        })
        .collect();
    f.sort_by_key(|i| std::cmp::Reverse(i.score));
    f.truncate(MAX_FILES);
    for i in &mut f {
        if let Target::Path { path, dir } = &mut i.target {
            *dir = std::fs::metadata(&*path).is_ok_and(|m| m.is_dir());
        }
    }
    FilesFound { items: f, index_building, index_off }
}

/// What was opened before (for the candidates it adds) and what use lifts an item by for this query: how often it is opened
/// ([`boost`]) and whether it was chosen for these letters ([`pick_points`]).
fn lifts(query: &str) -> (HashMap<String, Use>, impl Fn(&Target) -> i32) {
    let q = pick_query(query);
    let (used, weights) = with_store(|u| (u.map.clone(), pick_weights(&u.picks, &q, now_secs())));
    let counts = used.clone();
    (used, move |t: &Target| {
        let key = t.key();
        counts.get(&key).map_or(0, boost) + weights.get(&key).map_or(0, |w| pick_points(*w))
    })
}

/// Puts the sources' answers in the order shown: the best match for the typed `query` first if the picks know one, then by section
/// (apps, settings, files, bookmarks, web), each section keeping its own ranking.
pub(crate) fn merge(query: &str, local: Vec<Item>, files: Vec<Item>) -> Vec<Item> {
    merge_with(&weights_for(query), local, files)
}

/// [`merge`] for weights that are known ([`pick_weights`]). The item the picks speak for most (from [`BEST_MIN`] on; never the web
/// search) leaves its section and becomes the first row, under "En iyi eşleşme".
pub(crate) fn merge_with(weights: &HashMap<String, f64>, local: Vec<Item>, files: Vec<Item>) -> Vec<Item> {
    let mut all = local;
    all.extend(files);
    let best = all
        .iter()
        .enumerate()
        .filter(|(_, i)| !matches!(i.target, Target::Web { .. }))
        .filter_map(|(ix, i)| weights.get(&i.target.key()).map(|w| (ix, *w, i.score)))
        .filter(|(_, w, _)| *w >= BEST_MIN)
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal).then(a.2.cmp(&b.2)).then(b.0.cmp(&a.0)))
        .map(|(ix, _, _)| ix);
    if let Some(ix) = best {
        all[ix].section = Section::Best;
    }
    all.sort_by_key(|i| i.section);
    all
}

// ---------------------------------------------------------------------------------------------
// The window

struct Host {
    window: Option<WindowHandle<QuickSearch>>,
    images: Option<Entity<ImageCache>>,
}
impl Global for Host {}

fn host(cx: &mut App) -> &mut Host {
    if !cx.has_global::<Host>() {
        cx.set_global(Host { window: None, images: None });
    }
    cx.global_mut::<Host>()
}

pub fn close(cx: &mut App) {
    if let Some(w) = host(cx).window.take() {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
}

/// The hotkey: opens the window, or closes it when it is open.
pub fn toggle(cx: &mut App) {
    if let Some(w) = host(cx).window {
        // (closed some other way, Alt+F4: the press opens a new one)
        if w.update(cx, |_, _, _| ()).is_ok() {
            close(cx);
            return;
        }
        host(cx).window = None;
    }
    let images = match host(cx).images.clone() {
        Some(i) => i,
        None => {
            let i = cx.new(ImageCache::new);
            host(cx).images = Some(i.clone());
            i
        }
    };
    // centered on the screen under the pointer, in its upper part (where the eyes are)
    let work = rf_win::look::win::work_area_at_cursor();
    let origin = point(px(work[0] + (work[2] - WIDTH) / 2.), px(work[1] + work[3] * 0.22));
    let max_h = (work[3] * 0.78 - 24.).max(200.);
    let display_id = cx
        .displays()
        .into_iter()
        .find(|d| {
            let b = d.visible_bounds();
            (f32::from(b.origin.x) - work[0]).abs() < 2.0 && (f32::from(b.origin.y) - work[1]).abs() < 2.0
        })
        .map(|d| d.id());
    // automated UI tests: shown without taking the foreground from the user
    let test = std::env::var_os("RUSTYFILES_QUICK_SEARCH_NOACTIVATE").is_some();
    // with HyperGlass as the app's backdrop the search is on the same glass (the user asked); else Windows' acrylic as before
    let glassy = cx.try_global::<Settings>().is_some_and(|s| s.backdrop == crate::settings::BackdropPref::Glass);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(WIDTH), px(64.))))),
        display_id,
        titlebar: None,
        focus: !test,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: if glassy { WindowBackgroundAppearance::Transparent } else { WindowBackgroundAppearance::Blurred },
        ..Default::default()
    };
    match cx.open_window(opts, move |window, cx| {
        let hwnd = crate::look::window::hwnd_of(window);
        rf_win::window::set_dark_mode(hwnd, Palette::for_window(window).dark);
        crate::glass::keep_out_of_capture(hwnd);
        // summoned from another app: take the keyboard (the hotkey press allows it)
        if !test {
            rf_win::window::bring_to_front(hwnd);
        }
        cx.new(|cx| QuickSearch::new(images, max_h, glassy, window, cx))
    }) {
        Ok(w) => host(cx).window = Some(w),
        Err(e) => log::warn!("quick search window: {e:#}"),
    }
}

pub struct QuickSearch {
    input: Entity<InputState>,
    images: Entity<ImageCache>,
    items: Vec<Item>,
    index_building: bool,
    index_off: bool,
    selected: usize,
    generation: u64,
    /// The search `items` came from: Enter pressed before the newest results arrive waits
    /// for them (it would open the previous query's result).
    items_generation: u64,
    pending: Option<How>,
    /// The window's tallest size (from its top to the screen's bottom): the list scrolls past it.
    max_h: f32,
    scroll: ScrollHandle,
    /// The keyboard moved the selection: bring it into view.
    reveal: bool,
    /// The glass behind the window, when HyperGlass is the app's backdrop (`None`: Windows' acrylic).
    glass: Option<crate::glass_watch::PopupGlass>,
    _glass_watch: Option<Task<()>>,
    _search: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl QuickSearch {
    fn new(images: Entity<ImageCache>, max_h: f32, glassy: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Uygulama, dosya ya da web'de ara")));
        let mut subs = vec![cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.run_search(cx);
            }
        })];
        subs.push(cx.observe_window_activation(window, |_, window, cx| {
            if !window.is_window_active() {
                log::info!("quick search: the window lost the foreground and closes");
                cx.defer(close);
            }
        }));
        subs.push(cx.observe(&images, |_, _, cx| cx.notify()));
        input.update(cx, |s, cx| s.focus(window, cx));
        let glass = glassy.then(|| crate::glass_watch::PopupGlass::start_panel(crate::look::window::hwnd_of(window), Palette::for_window(window).dark));
        let glass_watch = glass.is_some().then(|| crate::glass_watch::watch(cx, |this: &mut Self| this.glass.as_mut()));
        let mut this = Self {
            input,
            images,
            items: Vec::new(),
            index_building: false,
            index_off: false,
            selected: 0,
            generation: 0,
            items_generation: 0,
            pending: None,
            max_h,
            scroll: ScrollHandle::new(),
            reveal: false,
            glass,
            _glass_watch: glass_watch,
            _search: None,
            _subs: subs,
        };
        this.run_search(cx);
        this
    }

    fn run_search(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        let generation = self.generation;
        let query = self.input.read(cx).value().to_string();
        let s = cx.global::<Settings>();
        let (engine, with_bookmarks, index_on) = (s.web_search, s.quick_search_bookmarks, s.name_index);
        let work = cx.background_spawn(async move { search(&query, engine, with_bookmarks, index_on) });
        self._search = Some(cx.spawn(async move |this, cx| {
            let found = work.await;
            let _ = this.update(cx, |this, cx| {
                if this.generation != generation {
                    return;
                }
                this.items = found.items;
                this.index_building = found.index_building;
                this.index_off = found.index_off;
                this.items_generation = generation;
                this.selected = 0;
                this.reveal = true;
                cx.notify();
                if let Some(how) = this.pending.take() {
                    this.activate(how, cx);
                }
            });
        }));
    }

    /// Enter (`how` = Open), Ctrl+Enter (ShowInFolder), Shift+Enter (Preview).
    fn activate(&mut self, how: How, cx: &mut Context<Self>) {
        if self.items_generation != self.generation {
            self.pending = Some(how);
            return;
        }
        let Some(item) = self.items.get(self.selected).cloned() else { return };
        let engine = cx.global::<Settings>().web_search;
        match (&item.target, how) {
            (Target::App { id, name }, _) => {
                let app = rf_win::apps::App { id: id.clone(), name: name.clone() };
                launch(move || {
                    if let Err(e) = rf_win::apps::launch(&app) {
                        log::warn!("quick search: start {}: {e:#}", app.name);
                    }
                });
            }
            (Target::Path { path, .. }, How::ShowInFolder) => {
                let msg = format!("{}{}", crate::workspace::SELECT_HERE_PREFIX, path.display());
                cx.defer(move |cx| crate::workspace::route_ipc(msg, cx));
            }
            (Target::Path { path, .. }, How::Preview) => {
                rf_win::look::win::allow_any_foreground();
                crate::look::ipc::send(crate::look::ipc::Msg::Show(path.clone(), crate::look::ShowOpts::default()), true);
            }
            (Target::Path { path, dir: true }, How::Open) => {
                // a folder opens in rustyfiles
                let msg = path.display().to_string();
                cx.defer(move |cx| crate::workspace::route_ipc(msg, cx));
            }
            (Target::Path { path, dir: false }, How::Open) => {
                let p = path.clone();
                launch(move || {
                    if let Err(e) = rf_win::shell::open(&p) {
                        log::warn!("quick search: open {}: {e:#}", p.display());
                    }
                });
            }
            (Target::Url { url }, _) => open_url(url.clone()),
            (Target::Web { query }, _) => open_url(web_url(engine, query)),
            (Target::Setting { launch: what, .. }, _) => {
                let what = what.clone();
                launch(move || {
                    if let Err(e) = crate::settings_rows::open(&what) {
                        log::warn!("quick search: a settings page could not be opened: {e:#}");
                    }
                });
            }
        }
        if !matches!(how, How::Preview) {
            let query = self.input.read(cx).value().to_string();
            remember_for(&query, &item);
            close_later(cx);
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum How {
    Open,
    ShowInFolder,
    Preview,
}

fn open_url(url: String) {
    launch(move || {
        if let Err(e) = rf_win::shell::open(Path::new(&url)) {
            log::warn!("quick search: open url: {e:#}");
        }
    });
}

fn close_later<T>(cx: &mut Context<T>) {
    cx.defer(close);
}

pub(crate) fn section_title(s: Section) -> &'static str {
    match s {
        Section::Recent => t!("Son kullanılanlar"),
        Section::Best => t!("En iyi eşleşme"),
        Section::Apps => t!("Uygulamalar"),
        Section::Files => t!("Dosyalar ve klasörler"),
        Section::Settings => t!("Ayarlar"),
        Section::Bookmarks => t!("Yer imleri"),
        Section::Web => t!("Web"),
    }
}

impl QuickSearch {
    fn icon(&self, item: &Item, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let img = match &item.target {
            Target::App { id, name } => {
                let parsing = rf_win::apps::App { id: id.clone(), name: name.clone() }.parsing();
                self.images.update(cx, |c, cx| c.shell_icon(&parsing, crate::image_cache::ICON_PX, cx))
            }
            Target::Path { path, dir } => {
                let name = file_name(path);
                let e = rf_core::Entry::new(&name, if *dir { rf_core::EntryKind::Dir } else { rf_core::EntryKind::File }, 0, 0, 0, 0);
                let parent = path.parent().map(Path::to_path_buf).unwrap_or_default();
                self.images.update(cx, |c, cx| c.icon(&parent, &e, cx))
            }
            _ => None,
        };
        let fallback = match &item.target {
            Target::App { .. } => L::AppWindow,
            Target::Path { dir: true, .. } => L::Folder,
            Target::Path { .. } => L::File,
            Target::Url { .. } => L::Globe,
            Target::Web { .. } => L::Search,
            Target::Setting { .. } => L::Settings,
        };
        div()
            .size(px(28.))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .map(|d| match img {
                Some(i) => d.child(img_el(i)).into_any_element(),
                None => d.child(Icon::new(fallback).size(px(20.)).text_color(p.text_secondary)).into_any_element(),
            })
    }
}

fn img_el(i: Arc<RenderImage>) -> impl IntoElement {
    gpui_kit::img(i).size(px(28.)).object_fit(ObjectFit::Contain)
}

impl Render for QuickSearch {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let n = self.items.len();
        let mut list = v_flex().px_2().pb_2();
        let mut last: Option<Section> = None;
        let items = self.items.clone();
        // the selected row's place among the list's children (section headers in between)
        let mut child = 0;
        let mut selected_child = 0;
        for (ix, item) in items.iter().enumerate() {
            if last != Some(item.section) {
                last = Some(item.section);
                list = list.child(div().px_2().pt_2().pb_1().text_xs().text_color(p.text_tertiary).child(section_title(item.section)));
                child += 1;
            }
            if ix == self.selected {
                selected_child = child;
            }
            child += 1;
            let sel = ix == self.selected;
            let title: SharedString = match &item.target {
                Target::Web { query } => format!("\u{201c}{query}\u{201d} {}", t!("için webde ara")).into(),
                _ => item.title.clone().into(),
            };
            let hint = if sel {
                match &item.target {
                    Target::Path { .. } => t!("Ctrl+Enter: klasörde göster"),
                    _ => "Enter",
                }
            } else {
                ""
            };
            let icon = self.icon(item, &p, cx);
            list = list.child(
                h_flex()
                    .id(("qs", ix))
                    .relative()
                    .h(px(ROW_H))
                    .px_2()
                    .gap_3()
                    .items_center()
                    .rounded(theme::RADIUS_SM)
                    .when(sel, |d| d.bg(p.selection).child(theme::selection_pill(&p)))
                    .when(!sel, |d| d.hover(|s| s.bg(p.hover)))
                    .child(icon)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().truncate().child(title))
                            .when(!item.detail.is_empty(), |d| d.child(div().text_xs().text_color(p.text_tertiary).truncate().child(item.detail.clone()))),
                    )
                    .when(!hint.is_empty(), |d| d.child(div().flex_shrink_0().text_xs().text_color(p.text_tertiary).child(hint)))
                    .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                        this.selected = ix;
                        let m = ev.modifiers();
                        this.activate(if m.control { How::ShowInFolder } else if m.shift { How::Preview } else { How::Open }, cx);
                    })),
            );
        }
        let width = WIDTH;
        let vh = window.viewport_size().height;
        let max_h = self.max_h;
        // what is left for the list under the search box, the notes and the key hints
        let notes = if self.index_building || self.index_off { 26. } else { 0. };
        let list_max = (max_h - 61. - 31. - notes).max(ROW_H * 2.);
        if std::mem::take(&mut self.reveal) {
            self.scroll.scroll_to_item(selected_child);
        }
        // over glass a wash, or the plain fill until the glass runs (never a see-through window with nothing behind it)
        let fill = self.glass.as_ref().map(|g| crate::glass_watch::panel_fill(p.dark, g.fallback()));
        v_flex()
            .size_full()
            .when_some(fill, |d, f| d.bg(f))
            .text_color(p.text)
            .capture_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                let m = &ev.keystroke.modifiers;
                match ev.keystroke.key.as_str() {
                    "down" if n > 0 => {
                        this.selected = (this.selected + 1) % n;
                        this.reveal = true;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "up" if n > 0 => {
                        this.selected = (this.selected + n - 1) % n;
                        this.reveal = true;
                        cx.stop_propagation();
                        cx.notify();
                    }
                    "enter" => {
                        cx.stop_propagation();
                        this.activate(if m.control { How::ShowInFolder } else if m.shift { How::Preview } else { How::Open }, cx);
                    }
                    "escape" => {
                        cx.stop_propagation();
                        close_later(cx);
                    }
                    _ => {}
                }
            }))
            .child(
                div().relative().w_full().child(
                    v_flex()
                        .w_full()
                        .child(
                            h_flex()
                                .h(px(60.))
                                .px_4()
                                .gap_3()
                                .items_center()
                                .child(Icon::new(L::Search).size(px(20.)).text_color(p.text_secondary))
                                .child(div().flex_1().text_lg().child(Input::new(&self.input).appearance(false))),
                        )
                        .when(n > 0 || self.index_building, |d| d.child(div().h(px(1.)).w_full().bg(p.divider)))
                        .when(self.index_building, |d| {
                            d.child(div().px_4().pt_2().text_xs().text_color(p.text_tertiary).child(t!("Dosya dizini hazırlanıyor: bazı dosyalar henüz çıkmayabilir")))
                        })
                        .when(self.index_off && n > 0, |d| {
                            d.child(div().px_4().pt_2().text_xs().text_color(p.text_tertiary).child(t!("Dosya araması kapalı: Ayarlar'da ad dizinini açın")))
                        })
                        .child(div().id("qs-list").max_h(px(list_max)).overflow_y_scroll().track_scroll(&self.scroll).child(list))
                        .when(n > 0, |d| {
                            d.child(
                                h_flex()
                                    .h(px(30.))
                                    .px_4()
                                    .gap_4()
                                    .items_center()
                                    .border_t_1()
                                    .border_color(p.divider)
                                    .text_xs()
                                    .text_color(p.text_tertiary)
                                    .child(t!("Enter: aç"))
                                    .child(t!("Ctrl+Enter: klasörde göster"))
                                    .child(t!("Shift+Enter: önizle"))
                                    .child(t!("Esc: kapat")),
                            )
                        }),
                )
                // the window follows the content's height
                .child(
                    canvas(
                        move |b, window, _| {
                            let h = b.size.height.ceil().min(px(max_h));
                            if (f32::from(h) - f32::from(vh)).abs() > 1. {
                                window.on_next_frame(move |w, _| w.resize(size(px(width), h)));
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's own `test` attribute would come with it)
    use super::{Bookmark, WebSearch, fold, parse_bookmarks, score, score_with, web_url};

    #[test]
    fn folding_and_scores() {
        assert_eq!(fold("Şarkılar İçin"), "sarkilar icin");
        // whole > start > word start > anywhere > initials
        let whole = score("Chrome", "chrome").unwrap();
        let start = score("Chrome Remote Desktop", "chrome").unwrap();
        let word = score("Google Chrome", "chrome").unwrap();
        let inside = score("Mychromeapp", "chrome").unwrap();
        let initials = score_with("Visual Studio Code", "vsc", true).unwrap();
        assert_eq!(score("Visual Studio Code", "vsc"), None);
        assert!(whole > start && start > word && word > inside && inside > initials, "{whole} {start} {word} {inside} {initials}");
        assert_eq!(score("Notepad", "xyz"), None);
        assert!(score("Şarkı listesi", "sarki").is_some());
    }

    #[test]
    fn bookmarks_from_browser_json() {
        let json = br#"{"checksum":"x","roots":{"bookmark_bar":{"type":"folder","name":"Bar","children":[
            {"type":"url","name":"GitHub","url":"https://github.com/"},
            {"type":"folder","name":"Dev","children":[{"type":"url","name":"","url":"https://docs.rs/"}]},
            {"type":"url","name":"Bookmarklet","url":"javascript:alert(1)"}]},
            "other":{"type":"folder","children":[]},"synced":{"type":"folder","children":[]}},"version":1}"#;
        let b = parse_bookmarks(json);
        assert_eq!(b, vec![Bookmark { name: "GitHub".into(), url: "https://github.com/".into() }, Bookmark { name: "https://docs.rs/".into(), url: "https://docs.rs/".into() }]);
    }

    #[test]
    fn web_search_urls() {
        assert_eq!(web_url(WebSearch::Google, "rust gpui"), "https://www.google.com/search?q=rust+gpui");
        assert_eq!(web_url(WebSearch::DuckDuckGo, "ç&="), "https://duckduckgo.com/?q=%C3%A7%26%3D");
    }
}

#[cfg(test)]
mod pick_tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use super::{BEST_MIN, Item, MAX_PICK_QUERIES, MAX_PICKS_PER_QUERY, Pick, Picks, Section, Target, best_key, merge_with, note_pick, parse_picks, pick_points, pick_query, pick_weights, score_with};

    const DAY: u64 = 86_400;
    const NOW: u64 = 1_800_000_000;

    fn app(name: &str, score: i32) -> Item {
        Item { target: Target::App { id: format!("id-{name}"), name: name.into() }, title: name.into(), detail: String::new(), section: Section::Apps, score }
    }
    fn file(path: &str, score: i32) -> Item {
        Item { target: Target::Path { path: PathBuf::from(path), dir: false }, title: path.into(), detail: String::new(), section: Section::Files, score }
    }
    fn web(q: &str) -> Item {
        Item { target: Target::Web { query: q.into() }, title: q.into(), detail: String::new(), section: Section::Web, score: 0 }
    }

    #[test]
    fn a_choice_is_kept_under_the_letters_it_was_made_with_folded_like_the_matching() {
        let mut p = Picks::new();
        assert!(note_pick(&mut p, "  PH ", "app:photoshop", NOW));
        assert!(note_pick(&mut p, "ph", "app:photoshop", NOW + 5));
        assert!(note_pick(&mut p, "Şarkı", "path:c:\\sarki.mp3", NOW));
        assert!(!note_pick(&mut p, "   ", "app:x", NOW), "nothing typed: nothing to learn");
        assert_eq!(p.len(), 2);
        assert_eq!(p["ph"]["app:photoshop"], Pick { count: 2, last: NOW + 5 });
        assert!(p.contains_key("sarki"));
        assert_eq!(pick_query(&"x".repeat(200)).chars().count(), 48, "a sentence is not a habit");
    }

    #[test]
    fn one_choice_puts_the_chosen_app_above_a_better_match_by_name() {
        // "ph": Phone Link begins with it, Photoshop only has a word that does
        let (phone, shop) = (score_with("Phone Link", "ph", true).unwrap(), score_with("Adobe Photoshop 2026", "ph", true).unwrap());
        assert!(phone > shop);
        let mut p = Picks::new();
        note_pick(&mut p, "ph", "app:shop", NOW);
        let w = pick_weights(&p, "ph", NOW);
        assert!(shop + pick_points(w["app:shop"]) > phone, "one choice is enough");
        assert_eq!(pick_points(0.0), 0);
        assert!(pick_points(1.0) >= 270);
        assert_eq!(pick_points(1_000.0), 800, "never more than a whole step of the matching and a half");
    }

    #[test]
    fn a_choice_counts_in_full_for_its_own_letters_less_for_shorter_or_longer_ones_and_not_for_others() {
        let mut p = Picks::new();
        note_pick(&mut p, "pho", "app:shop", NOW);
        let at = |q: &str| pick_weights(&p, q, NOW).get("app:shop").copied().unwrap_or(0.0);
        assert!((at("pho") - 1.0).abs() < 1e-9);
        assert!((at("ph") - 0.6).abs() < 1e-9, "typed more then, less now");
        assert!((at("p") - 0.6).abs() < 1e-9);
        assert!((at("photo") - 0.4).abs() < 1e-9, "typed less then, more now");
        assert_eq!(at("x"), 0.0);
        assert_eq!(at("hop"), 0.0, "not the same beginning");
        assert!(pick_weights(&p, "", NOW).is_empty(), "no query: nothing");
    }

    #[test]
    fn choices_add_up_and_fade_without_use() {
        let mut p = Picks::new();
        for i in 0..3 {
            note_pick(&mut p, "ra", "path:c:\\rapor.xlsx", NOW + i);
        }
        let w = |now: u64| pick_weights(&p, "ra", now)["path:c:\\rapor.xlsx"];
        assert!((w(NOW + 2) - 3.0).abs() < 1e-3);
        assert!((w(NOW + 60 * DAY) - 1.5).abs() < 0.01, "half after two months");
        assert!(w(NOW + 120 * DAY) >= BEST_MIN, "three choices are remembered for a season");
        assert!(w(NOW + 365 * DAY) < BEST_MIN, "and not for ever");
    }

    #[test]
    fn the_item_the_picks_speak_for_most_becomes_the_first_row_whatever_its_kind() {
        let local = vec![app("Rapor Aracı", 840), app("Radyo", 830), web("ra")];
        let files = vec![file("C:\\rapor.xlsx", 700), file("C:\\raf.txt", 650)];
        // nothing learned: the sections in their order, as before
        let plain = merge_with(&HashMap::new(), local.clone(), files.clone());
        assert_eq!(plain.iter().map(|i| i.section).collect::<Vec<_>>(), [Section::Apps, Section::Apps, Section::Files, Section::Files, Section::Web]);
        // the file was opened for these letters: it is the first row, under a heading of its own, and the rest keeps its order
        let mut p = Picks::new();
        note_pick(&mut p, "ra", "path:c:\\rapor.xlsx", NOW);
        let got = merge_with(&pick_weights(&p, "ra", NOW), local.clone(), files.clone());
        assert_eq!((got[0].title.as_str(), got[0].section), ("C:\\rapor.xlsx", Section::Best));
        assert_eq!(got.iter().skip(1).map(|i| i.title.as_str()).collect::<Vec<_>>(), ["Rapor Aracı", "Radyo", "C:\\raf.txt", "ra"]);
        // an app chosen more often for the same letters is the best match instead; there is only ever one
        note_pick(&mut p, "ra", "app:id-Radyo", NOW);
        note_pick(&mut p, "ra", "app:id-Radyo", NOW);
        let got = merge_with(&pick_weights(&p, "ra", NOW), local.clone(), files.clone());
        assert_eq!((got[0].title.as_str(), got[0].section), ("Radyo", Section::Best));
        assert_eq!(got.iter().filter(|i| i.section == Section::Best).count(), 1);
        assert_eq!(got.len(), 5, "nothing is lost and nothing shown twice");
        // a weak hint is not a best match (one choice for a single letter, long ago), and what is not among the rows cannot be one
        let mut weak = Picks::new();
        note_pick(&mut weak, "r", "app:id-Radyo", NOW - 200 * DAY);
        assert!(merge_with(&pick_weights(&weak, "ra", NOW), local.clone(), files.clone()).iter().all(|i| i.section != Section::Best));
        let mut gone = Picks::new();
        note_pick(&mut gone, "ra", "app:id-Silinmiş", NOW);
        assert!(merge_with(&pick_weights(&gone, "ra", NOW), local, files).iter().all(|i| i.section != Section::Best));
    }

    #[test]
    fn the_web_search_is_never_the_best_match() {
        let mut w = HashMap::new();
        w.insert("web:ra".to_string(), 9.0);
        let got = merge_with(&w, vec![app("Radyo", 830), web("ra")], Vec::new());
        assert!(got.iter().all(|i| i.section != Section::Best));
    }

    #[test]
    fn the_best_key_is_the_heaviest_from_the_threshold_on() {
        let mut w = HashMap::new();
        assert_eq!(best_key(&w), None);
        w.insert("app:a".to_string(), BEST_MIN - 0.01);
        assert_eq!(best_key(&w), None);
        w.insert("path:c:\\x".to_string(), 2.0);
        w.insert("app:b".to_string(), 1.0);
        assert_eq!(best_key(&w).map(String::as_str), Some("path:c:\\x"));
    }

    #[test]
    fn the_list_of_choices_stays_small() {
        let mut p = Picks::new();
        for i in 0..20u64 {
            note_pick(&mut p, "a", &format!("app:{i}"), NOW + i);
        }
        assert_eq!(p["a"].len(), MAX_PICKS_PER_QUERY);
        assert!(p["a"].contains_key("app:19"), "the newest choice stays");
        for i in 0..(MAX_PICK_QUERIES as u64 + 50) {
            note_pick(&mut p, &format!("q{i}"), "app:x", NOW + 100 + i);
        }
        assert!(p.len() <= MAX_PICK_QUERIES);
        assert!(p.contains_key(&format!("q{}", MAX_PICK_QUERIES + 49)), "the newest query stays");
        assert!(!p.contains_key("a"), "the oldest went");
    }

    #[test]
    fn a_picks_file_is_read_entry_by_entry_and_comes_back_as_it_was_written() {
        let mut p = Picks::new();
        note_pick(&mut p, "ph", "app:shop", NOW);
        assert_eq!(parse_picks(&serde_json::to_vec(&p).unwrap()), p);
        // an entry of another shape does not cost the others; rubbish is an empty list
        let mixed = br#"{"ph":{"app:shop":{"count":2,"last":5}},"x":"not a map","y":{"k":{"count":"many"}}}"#;
        let got = parse_picks(mixed);
        assert_eq!(got.len(), 1);
        assert_eq!(got["ph"]["app:shop"], Pick { count: 2, last: 5 });
        assert!(parse_picks(b"not json").is_empty());
    }
}
