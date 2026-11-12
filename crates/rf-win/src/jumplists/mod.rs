//! What Windows' taskbar shows when you right-click an app's button, for the dock and the Start panel to show too: the app's Tasks, the
//! categories it wrote itself, the items pinned to it, and its Recent or Frequent items. READ-ONLY: Windows' stores are never written
//! (no pin, no removal, no clearing; see `docs/jumplists.md`).
//!
//! ```text
//! read(&AppKey, limit) -> JumpList          one worker thread; empty on any failure; never waits longer than BUDGET for the shell
//! open(&JumpItem)      -> Result            starts an item through rf_win::dock::launch (behind the Starter trait)
//! ```
//!
//! Where each part comes from:
//!
//! | part | source |
//! |---|---|
//! | `recent`, `frequent` | `IApplicationDocumentLists` (in-process, `windows.storage.dll`), on the shell thread (`sta::call`) |
//! | `pinned` | the `DestList` stream of `<AppID>.automaticDestinations-ms` (the pin flag and order) and the stream of each pinned entry |
//! | `tasks`, `custom`, `which` | `<AppID>.customDestinations-ms` |
//! | the files' name | [`appid`]: a CRC-64 of the app's identity |
//!
//! The files are read into memory with the ordinary shared read (Explorer holds them open and keeps writing them; nothing of its is
//! disturbed, no copy is made on disk) and everything after that is pure. The shell links inside are loaded from memory with
//! `IShellLink` (never resolved). Dead targets are dropped (a target on a network share, a removable drive or an optical drive is kept
//! without being asked about, so that a sleeping drive cannot hold the dock up).
//!
//! Every shell call goes through `sta::call` with the time that is left of the read's budget; a read that runs out of time returns
//! what it has.

pub mod appid;
pub mod custom;
pub mod destlist;
pub mod lnk;

#[cfg(test)]
mod fixture;

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::dock::model::{DockApp, IconSource, Launch, PinnedApp};
use custom::CategoryKind;
use lnk::{AutoList, Disk, LinkFacts, Role};

/// An app's identity as the dock knows it: the explicit AppUserModelID when the pin, the window or the package has one, and the program.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct AppKey {
    pub aumid: Option<String>,
    pub exe: Option<PathBuf>,
}

impl AppKey {
    pub fn from_aumid(aumid: impl Into<String>) -> Self {
        AppKey { aumid: Some(aumid.into()), exe: None }
    }

    pub fn from_exe(exe: impl Into<PathBuf>) -> Self {
        AppKey { aumid: None, exe: Some(exe.into()) }
    }

    /// A pin of the Windows taskbar: both its AppUserModelID and its program, when it has them.
    pub fn from_pin(p: &PinnedApp) -> Self {
        AppKey { aumid: p.aumid.clone(), exe: p.exe.as_ref().map(PathBuf::from) }
    }

    /// An app of the middle island. Its `key` is the AppUserModelID, else the program path (lower case; the hash upper-cases anyway),
    /// else `pid:..` for a program with nothing the shell could be asked about.
    pub fn from_dock_app(a: &DockApp) -> Self {
        let mut k = AppKey::default();
        let key = a.key.trim();
        if key.starts_with("pid:") || key.is_empty() {
            // nothing to go by
        } else if key.contains('\\') || key.contains('/') {
            k.exe = Some(PathBuf::from(key));
        } else {
            k.aumid = Some(key.to_string());
        }
        // the dock's key is the path in lower case; the launch path, when it is the same path, has the program's own spelling, which is
        // what the hash is made of (a lower-cased "İ" is not the letter it came from)
        if let Some(Launch::Exe { path, .. }) = &a.launch
            && k.exe.as_ref().is_none_or(|e| e.to_string_lossy().to_lowercase() == path.to_string_lossy().to_lowercase())
        {
            k.exe = Some(path.clone());
        }
        k
    }

    pub fn is_empty(&self) -> bool {
        self.aumid.is_none() && self.exe.is_none()
    }
}

/// Which of the two automatic categories the app asked Windows to show (its custom file says so by listing the category).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Which {
    /// The app has no custom list of its own (no custom file, or an empty one): Windows shows its default, Recent.
    #[default]
    Unknown,
    /// The app has a custom list (Tasks, categories of its own) and names neither Recent nor Frequent in it: the taskbar shows the
    /// automatic items of neither. [`JumpList::recent`] and `frequent` can still hold what the shell listed.
    Neither,
    Recent,
    Frequent,
    Both,
}

/// What a jump-list entry does when chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JumpTarget {
    /// Open this file or folder with its default handler.
    File(PathBuf),
    /// Run a program (or, with no program, a packaged app by its AppUserModelID) with these arguments.
    Link { program: PathBuf, arguments: String, working_dir: Option<PathBuf>, aumid: Option<String> },
    /// A line between groups of tasks.
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpItem {
    pub title: String,
    pub icon: Option<IconSource>,
    pub target: JumpTarget,
}

/// A category the app wrote itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JumpCategory {
    pub name: String,
    pub items: Vec<JumpItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JumpList {
    pub tasks: Vec<JumpItem>,
    pub custom: Vec<JumpCategory>,
    pub pinned: Vec<JumpItem>,
    pub recent: Vec<JumpItem>,
    pub frequent: Vec<JumpItem>,
    pub which: Which,
}

impl JumpList {
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty() && self.custom.iter().all(|c| c.items.is_empty()) && self.pinned.is_empty() && self.recent.is_empty() && self.frequent.is_empty()
    }

    /// The automatic items the taskbar would show for the app: its Frequent items when it asked for those alone, none when its own
    /// custom list names neither category, else its Recent. (`Both` gives Recent here; the caller reads `frequent` for the other.)
    pub fn automatic(&self) -> &[JumpItem] {
        match self.which {
            Which::Frequent => &self.frequent,
            Which::Neither => &[],
            Which::Unknown | Which::Recent | Which::Both => &self.recent,
        }
    }
}

/// How long a read may wait for the shell in all.
pub const BUDGET: Duration = Duration::from_secs(5);

/// The largest jump-list file read (the real ones are a few kilobytes up to a few megabytes).
const MAX_FILE: u64 = 16 * 1024 * 1024;
/// The largest single stream (one shell link) read out of a compound file.
const MAX_STREAM: u64 = 1024 * 1024;
/// The most shell links of the custom file handed to the shell thread in one read, over all its categories together. A timed-out job
/// still runs to its end on the one shell thread (`sta::call`), so what one read can queue there is bounded.
const MAX_CUSTOM_LINKS: usize = 200;
/// How long to wait before reading a file a second time when what came did not parse (Explorer rewrites these files in place, so a read
/// can land in the middle of a write).
const TORN_READ_PAUSE: Duration = Duration::from_millis(15);

/// What a read found out besides the list itself, for the probe and for the day something does not show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// The identity texts tried and the one that named a file.
    pub candidates: usize,
    pub matched: Option<appid::Form>,
    pub automatic_file: bool,
    pub custom_file: bool,
    pub destlist_version: Option<u32>,
    pub destlist_entries: usize,
    pub custom_complete: Option<bool>,
    /// `IApplicationDocumentLists` answered for Recent / Frequent (an empty list is an answer).
    pub recent_answered: bool,
    pub frequent_answered: bool,
    /// The budget ran out before everything was read.
    pub out_of_time: bool,
    pub elapsed: Duration,
}

/// What a read needs from outside: the files, the shell and the disk. The real one is [`Real`]; tests give their own.
pub trait Backend {
    /// The bytes of `<stem>.<extension>` (`appid::AUTOMATIC_EXT` or `CUSTOM_EXT`), `None` when there is no such file.
    fn file(&self, extension: &str, stem: &str) -> Option<Vec<u8>>;
    /// Loads each blob as a shell link; `None` for one that is not (or when the shell did not answer in `within`).
    fn links(&self, blobs: Vec<Vec<u8>>, within: Duration) -> Option<Vec<Option<LinkFacts>>>;
    /// The app's automatic list from the shell; `None` when the shell did not answer in `within`, `Err` when it cannot be had.
    fn documents(&self, app_id: &str, which: AutoList, wanted: u32, within: Duration) -> Option<anyhow::Result<Vec<LinkFacts>>>;
    fn disk(&self) -> &dyn Disk;
    fn known_folders(&self) -> Vec<(String, PathBuf)>;
}

/// The real thing: the user's Recent folder, the shell thread, the disk.
pub struct Real;

impl Backend for Real {
    fn file(&self, extension: &str, stem: &str) -> Option<Vec<u8>> {
        let dir = appid::recent_root()?.join(if extension == appid::AUTOMATIC_EXT { "AutomaticDestinations" } else { "CustomDestinations" });
        read_shared(&dir.join(format!("{stem}.{extension}")))
    }

    fn links(&self, blobs: Vec<Vec<u8>>, within: Duration) -> Option<Vec<Option<LinkFacts>>> {
        crate::sta::call(within, move || lnk::read_links(&blobs))
    }

    fn documents(&self, app_id: &str, which: AutoList, wanted: u32, within: Duration) -> Option<anyhow::Result<Vec<LinkFacts>>> {
        let id = app_id.to_string();
        crate::sta::call(within, move || lnk::read_documents(&id, which, wanted))
    }

    fn disk(&self) -> &dyn Disk {
        &lnk::RealDisk
    }

    fn known_folders(&self) -> Vec<(String, PathBuf)> {
        // the known folders do not move while the process runs: asked once, so that a read does not make a dozen shell calls of its own
        // on the caller's thread each time
        static FOLDERS: std::sync::OnceLock<Vec<(String, PathBuf)>> = std::sync::OnceLock::new();
        FOLDERS.get_or_init(appid::known_folders_now).clone()
    }
}

/// A file read whole with the ordinary shared read: Rust opens with read, write and delete sharing, so a file Explorer holds open (and
/// rewrites) is neither blocked nor disturbed. Nothing is created or written.
fn read_shared(path: &Path) -> Option<Vec<u8>> {
    let f = std::fs::File::open(path).ok()?;
    if f.metadata().ok()?.len() > MAX_FILE {
        return None;
    }
    let mut v = Vec::new();
    f.take(MAX_FILE).read_to_end(&mut v).ok()?;
    Some(v)
}

/// The jump list of an app: at most `limit` items in each part. Empty on any failure; waits for the shell at most [`BUDGET`] in all.
/// Blocks (disk, shell thread): call it from a worker thread.
pub fn read(app: &AppKey, limit: usize) -> JumpList {
    read_detailed(&Real, app, limit, BUDGET).0
}

/// [`read`] with a backend and a budget of the caller's choosing, and the [`Report`].
pub fn read_detailed(be: &dyn Backend, app: &AppKey, limit: usize, budget: Duration) -> (JumpList, Report) {
    let started = Instant::now();
    let mut rep = Report::default();
    let mut list = JumpList::default();
    if limit == 0 || app.is_empty() {
        return (list, rep);
    }
    let left = || budget.saturating_sub(started.elapsed());

    // 1. which identity names a file
    let cands = appid::candidates(app.aumid.as_deref(), app.exe.as_deref(), &be.known_folders());
    rep.candidates = cands.len();
    let mut chosen = None;
    let (mut auto_bytes, mut custom_bytes) = (None, None);
    for c in &cands {
        let stem = appid::file_stem(c.id);
        let a = be.file(appid::AUTOMATIC_EXT, &stem);
        let u = be.file(appid::CUSTOM_EXT, &stem);
        if a.is_some() || u.is_some() {
            chosen = Some(c);
            (auto_bytes, custom_bytes) = (a, u);
            break;
        }
    }
    let Some(chosen) = chosen else { return (list, finish(rep, started)) };
    // a file that does not parse may have been caught in the middle of Explorer's rewrite: once more after a short pause
    let stem = appid::file_stem(chosen.id);
    let read_again = |ext: &str| {
        std::thread::sleep(TORN_READ_PAUSE);
        be.file(ext, &stem)
    };
    if auto_bytes.as_deref().is_some_and(automatic_file_looks_torn)
        && let Some(again) = read_again(appid::AUTOMATIC_EXT)
    {
        auto_bytes = Some(again);
    }
    if custom_bytes.as_deref().is_some_and(|b| b.len() > 24 && !custom::parse(b).complete)
        && let Some(again) = read_again(appid::CUSTOM_EXT)
    {
        custom_bytes = Some(again);
    }
    rep.matched = Some(chosen.form);
    rep.automatic_file = auto_bytes.is_some();
    rep.custom_file = custom_bytes.is_some();
    let disk = be.disk();

    // 2. the custom file: tasks, named categories, which automatic category
    let mut custom_blobs: Vec<(usize, Vec<u8>)> = Vec::new(); // (category index, link)
    let mut cats: Vec<custom::RawCategory> = Vec::new();
    if let Some(b) = &custom_bytes {
        let f = custom::parse(b);
        rep.custom_complete = Some(f.complete);
        let (r, q) = (f.categories.iter().any(|c| c.kind == CategoryKind::Recent), f.categories.iter().any(|c| c.kind == CategoryKind::Frequent));
        list.which = match (r, q) {
            (true, true) => Which::Both,
            (true, false) => Which::Recent,
            (false, true) => Which::Frequent,
            // a custom list that names neither is the app saying "no automatic items"; no list at all is the default (Recent)
            (false, false) if f.complete && !f.categories.is_empty() => Which::Neither,
            (false, false) => Which::Unknown,
        };
        cats = f.categories;
        for (i, c) in cats.iter().enumerate() {
            if matches!(c.kind, CategoryKind::Tasks | CategoryKind::Named(_)) {
                // twice the limit per category (repeats, separators and dead entries are dropped later), and a cap over all of them
                let room = MAX_CUSTOM_LINKS.saturating_sub(custom_blobs.len());
                for l in c.links.iter().take(limit.saturating_mul(2).max(limit).min(room)) {
                    custom_blobs.push((i, l.clone()));
                }
            }
        }
    }

    // 3. the automatic file: pins, and the newest entries as a stand-in for Recent should the shell not answer
    let mut pinned_entries: Vec<destlist::DestEntry> = Vec::new();
    let mut pinned_blobs: Vec<Vec<u8>> = Vec::new();
    let mut spare_entries: Vec<destlist::DestEntry> = Vec::new();
    let mut spare_blobs: Vec<Vec<u8>> = Vec::new();
    if let Some(b) = &auto_bytes
        && let Some((dl, mut streams)) = read_destlist(b)
    {
        rep.destlist_version = Some(dl.version);
        rep.destlist_entries = dl.entries.len();
        let pins: Vec<destlist::DestEntry> = dl.pinned().into_iter().take(limit).cloned().collect();
        let recent: Vec<destlist::DestEntry> = dl.recent().into_iter().take(limit).cloned().collect();
        for e in pins {
            pinned_blobs.push(streams.get(e.number).unwrap_or_default());
            pinned_entries.push(e);
        }
        for e in recent {
            spare_blobs.push(streams.get(e.number).unwrap_or_default());
            spare_entries.push(e);
        }
    }

    // 4. the shell links (one call to the shell thread for all of them)
    let mut blobs: Vec<Vec<u8>> = custom_blobs.iter().map(|(_, b)| b.clone()).collect();
    let n_custom = blobs.len();
    blobs.extend(pinned_blobs.iter().cloned());
    let facts: Vec<Option<LinkFacts>> = if blobs.is_empty() {
        Vec::new()
    } else {
        match be.links(blobs, left()) {
            Some(f) => f,
            None => {
                rep.out_of_time = true;
                Vec::new()
            }
        }
    };
    let fact = |i: usize| facts.get(i).cloned().flatten();

    // tasks and the app's own categories
    let mut seen_cat: Vec<(usize, Vec<JumpItem>)> = Vec::new();
    for (k, (ci, _)) in custom_blobs.iter().enumerate() {
        let role = Role::Task;
        let Some(item) = fact(k).and_then(|f| lnk::to_item(&f, role, disk)) else { continue };
        match seen_cat.iter_mut().find(|(i, _)| i == ci) {
            Some((_, v)) => v.push(item),
            None => seen_cat.push((*ci, vec![item])),
        }
    }
    for (ci, items) in seen_cat {
        let items = tidy(items, limit);
        if items.is_empty() {
            continue;
        }
        match &cats[ci].kind {
            CategoryKind::Tasks => list.tasks = items,
            CategoryKind::Named(n) => list.custom.push(JumpCategory { name: n.clone(), items }),
            _ => {}
        }
    }

    // pins
    for (k, e) in pinned_entries.iter().enumerate() {
        let f = fact(n_custom + k).or_else(|| path_only(e));
        if let Some(item) = f.and_then(|f| lnk::to_item(&f, Role::Document, disk)) {
            list.pinned.push(item);
        }
    }
    list.pinned = tidy(std::mem::take(&mut list.pinned), limit);

    // 5. Recent and Frequent from the shell
    let app_id = chosen.text.as_str();
    if rep.automatic_file && !rep.out_of_time {
        for which in [AutoList::Recent, AutoList::Frequent] {
            if left().is_zero() {
                rep.out_of_time = true;
                break;
            }
            match be.documents(app_id, which, limit as u32, left()) {
                None => rep.out_of_time = true,
                Some(Err(e)) => log::debug!("jumplists: the shell's {which:?} list could not be had ({e})"),
                Some(Ok(v)) => {
                    let items: Vec<JumpItem> = v.iter().filter_map(|f| lnk::to_item(f, Role::Document, disk)).collect();
                    if which == AutoList::Recent {
                        rep.recent_answered = true;
                        list.recent = items;
                    } else {
                        rep.frequent_answered = true;
                        list.frequent = items;
                    }
                }
            }
        }
    }
    // the shell did not give Recent: the file's own newest entries stand in for it
    if !rep.recent_answered && !spare_entries.is_empty() && !left().is_zero() {
        let loaded = be.links(spare_blobs, left()).unwrap_or_default();
        for (k, e) in spare_entries.iter().enumerate() {
            let f = loaded.get(k).cloned().flatten().or_else(|| path_only(e));
            if let Some(item) = f.and_then(|f| lnk::to_item(&f, Role::Document, disk)) {
                list.recent.push(item);
            }
        }
    }
    // a pinned item is not also recent
    let pinned_keys: HashSet<String> = list.pinned.iter().map(key_of).collect();
    list.recent.retain(|i| !pinned_keys.contains(&key_of(i)));
    list.frequent.retain(|i| !pinned_keys.contains(&key_of(i)));
    list.recent = tidy(std::mem::take(&mut list.recent), limit);
    list.frequent = tidy(std::mem::take(&mut list.frequent), limit);
    (list, finish(rep, started))
}

fn finish(mut rep: Report, started: Instant) -> Report {
    rep.elapsed = started.elapsed();
    rep
}

/// An entry the shell link of which could not be had still names a path.
fn path_only(e: &destlist::DestEntry) -> Option<LinkFacts> {
    (!e.path.is_empty()).then(|| LinkFacts { target: Some(PathBuf::from(&e.path)), ..Default::default() })
}

fn key_of(i: &JumpItem) -> String {
    match &i.target {
        JumpTarget::File(p) => format!("f:{}", p.to_string_lossy().to_lowercase()),
        JumpTarget::Link { program, arguments, aumid, .. } => format!("l:{}|{}|{}", program.to_string_lossy().to_lowercase(), arguments, aumid.as_deref().unwrap_or("")),
        JumpTarget::Separator => "sep".into(),
    }
}

/// Removes repeats (the same target twice), a separator at the start, the end or doubled, and cuts to `limit` items (separators do not
/// count against it).
fn tidy(items: Vec<JumpItem>, limit: usize) -> Vec<JumpItem> {
    let mut seen = HashSet::new();
    let mut out: Vec<JumpItem> = Vec::new();
    let mut count = 0;
    for it in items {
        let sep = it.target == JumpTarget::Separator;
        if sep {
            if out.is_empty() || out.last().is_some_and(|l| l.target == JumpTarget::Separator) {
                continue;
            }
        } else {
            if count == limit || !seen.insert(key_of(&it)) {
                continue;
            }
            count += 1;
        }
        out.push(it);
    }
    while out.last().is_some_and(|l| l.target == JumpTarget::Separator) {
        out.pop();
    }
    out
}

/// The streams of a compound file, read on request.
pub struct Streams<'a> {
    cf: cfb::CompoundFile<std::io::Cursor<&'a [u8]>>,
}

impl Streams<'_> {
    /// The bytes of the stream of entry number `n` (named in hexadecimal), `None` when there is none.
    pub fn get(&mut self, n: u32) -> Option<Vec<u8>> {
        let mut s = self.cf.open_stream(format!("/{n:x}")).ok()?;
        let mut v = Vec::new();
        (&mut s).take(MAX_STREAM).read_to_end(&mut v).ok()?;
        Some(v)
    }
}

/// The file is a compound file that does not open, or one whose `DestList` stream has bytes that do not parse: what a read caught in the
/// middle of a write looks like. A compound file with no `DestList` stream or an empty one is an ordinary state and not torn.
fn automatic_file_looks_torn(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return false;
    }
    let Ok(mut cf) = cfb::CompoundFile::open(std::io::Cursor::new(bytes)) else { return true };
    let Ok(s) = cf.open_stream("/DestList") else { return false };
    let mut raw = Vec::new();
    if s.take(MAX_FILE).read_to_end(&mut raw).is_err() {
        return true;
    }
    !raw.is_empty() && destlist::parse(&raw).is_none()
}

/// The `DestList` of an automatic-destinations file (a compound file in memory) and a reader of its other streams.
pub fn read_destlist(bytes: &[u8]) -> Option<(destlist::DestList, Streams<'_>)> {
    let mut cf = cfb::CompoundFile::open(std::io::Cursor::new(bytes)).ok()?;
    let mut raw = Vec::new();
    cf.open_stream("/DestList").ok()?.take(MAX_FILE).read_to_end(&mut raw).ok()?;
    let dl = destlist::parse(&raw)?;
    Some((dl, Streams { cf }))
}

// ---------------------------------------------------------------------------------------------------------------------------
// opening an item

/// Starts things; the real one is the dock's own launcher. A trait so that the decisions are tested without starting anything.
pub trait Starter {
    fn start(&self, launch: &Launch) -> anyhow::Result<()>;
    /// A packaged app (by its AppUserModelID) with an argument string; `Launch::Packaged` has no place for one.
    fn start_packaged_with(&self, aumid: &str, arguments: &str) -> anyhow::Result<()>;
}

/// [`crate::dock::launch::start`]: the shell's "open", or the application activation manager for a packaged app.
pub struct ShellStarter;

impl Starter for ShellStarter {
    fn start(&self, launch: &Launch) -> anyhow::Result<()> {
        crate::dock::launch::start(launch)
    }

    fn start_packaged_with(&self, aumid: &str, arguments: &str) -> anyhow::Result<()> {
        crate::dock::launch::start_packaged_with(aumid, arguments)
    }
}

/// What starting an item means for the dock's launcher. `None` for a separator and for a link with nothing to run.
///
/// The launcher starts a program in the program's own folder: a task's working directory is not passed on (it is read and kept in the
/// item, but `dock::launch` has no place for it). The arguments of a packaged app's task are not in the `Launch` either: [`open_with`]
/// hands those to the starter itself.
pub fn launch_for(item: &JumpItem) -> Option<Launch> {
    match &item.target {
        JumpTarget::File(p) => Some(Launch::Link(p.clone())),
        JumpTarget::Link { program, arguments, aumid, .. } => {
            if !program.as_os_str().is_empty() {
                Some(Launch::Exe { path: program.clone(), args: arguments.clone() })
            } else {
                aumid.clone().map(Launch::Packaged)
            }
        }
        JumpTarget::Separator => None,
    }
}

pub fn open_with(starter: &dyn Starter, item: &JumpItem) -> anyhow::Result<()> {
    // a packaged app's task with arguments: the activation manager takes them, the dock's `Launch` does not carry them
    if let JumpTarget::Link { program, arguments, aumid: Some(aumid), .. } = &item.target
        && program.as_os_str().is_empty()
        && !arguments.is_empty()
    {
        return starter.start_packaged_with(aumid, arguments);
    }
    match launch_for(item) {
        Some(l) => starter.start(&l),
        None => anyhow::bail!("this jump-list entry has nothing to start"),
    }
}

/// Starts an item: opens the file, runs the task. Can wait on another program: a worker thread. Never run by the tests of this crate.
pub fn open(item: &JumpItem) -> anyhow::Result<()> {
    open_with(&ShellStarter, item)
}

#[cfg(test)]
mod tests;
