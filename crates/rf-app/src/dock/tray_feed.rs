//! The real source behind `tray_source.rs`: the running tray host's model, as the dock's tray strip wants it (shell plan increment D3,
//! joined to D2). The host (`rf_win::trayhost`, started by `shell::trayhost` only while the setting `shell_tray_host` is on) keeps
//! icons with BGRA pictures, a window handle and a file NAME for the owner; the strip wants RGBA pictures, a tip it can print, an
//! identity the order file can remember across runs, and a hint when anything changed. This file is that translation and nothing else.
//!
//! WHERE THE WORK RUNS. One small thread per attached host (the "pump") waits for the host's event hints, turns the model into the
//! strip's snapshot (path lookups, picture conversion, all cached) and stores the result. The interface thread only copies the stored
//! snapshot ([`Feed::snapshot`]), so nothing the strip does on the interface thread opens a process, converts a picture or waits for
//! the host. A burst of hints is one rebuild (everything queued is drained first) and rebuilds are at least [`GAP`] apart, with the
//! last one always made, so a program that changes its tip a hundred times a second costs the interface at most about 25 wake-ups
//! a second, and a change that alters nothing the strip draws (a balloon, a version) wakes nobody.
//!
//! WHAT RUNS WHEN. With the setting off nothing is attached: no thread, no timer, no lookup; [`Feed::snapshot`] is `None`, so the
//! strip draws nothing and the island is as it was. [`attach`] (called by `shell::trayhost` when the host is up) starts the pump;
//! dropping the [`Attachment`] (host stopped, setting turned off, quit) ends it, joins it, and tells the subscribers the snapshot is
//! gone. A host in log-only mode has no model to read: it is attached with no pump and the snapshot stays `None`.
//!
//! IDENTITY. An icon is known to the order file by its GUID when it gave a usable one, else by its owner program's full path and the
//! id it chose ([`StableId`]). The host keeps only the program's file name, so the path is looked up from the owner's process
//! (`rf_win::tasklist::process_exe_path`, on the pump, once per process and cached). If it cannot be read the file name stands in
//! (`exe:mail.exe|3`, still told apart by id), and only when that is empty too does the identity fall back to `exe:|3`.
//!
//! KEYS. The strip is handed an opaque key per icon. The host numbers its icons from 1 at every start, so the key is the host's number
//! with the ATTACH'S NUMBER in its upper half ([`compose_key`]): a host that was stopped and started again never hands out a key an
//! earlier host did, so the strip's picture cache, its rectangle memo and a click it still holds cannot meet the wrong icon.
//!
//! CLICKS. [`Feed::deliver_click`] checks the icon is in the snapshot the strip drew AND still has the identity the click names, then
//! hands it to the host's click engine ([`HostPort::click`]) together with the owner (window and process) the snapshot was made with.
//! The engine sends the callback messages to the program with `SendNotifyMessage` and refuses an icon whose owner window is gone or
//! belongs to another process now, AND an icon whose owner is not the one that was drawn: a program that adds an icon again under a
//! key the model holds (the same GUID from another process) takes the icon over under the same number and the same identity, so
//! only the owner tells the two apart. The owner is part of what a snapshot is, so the strip is told when it changes.
//!
//! LOGGING: counts and outcomes, never a tip, a name or a path.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use rf_win::dock::layout::Rect;
use rf_win::dock::tray_order::StableId;
use rf_win::dock::tray_strip::ClickKind;
use rf_win::dock::tray_tip as rules;
use rf_win::tray_wire::click::{ClickOptions, HostEvent};
use rf_win::tray_wire::{self as wire, IconKey as WireKey, Point};
use rf_win::trayhost::{ClickOutcome, ClickRequest, IconId, OwnerRef, Snapshot, TrayEvent, TrayView};

use super::tray_source::{Bitmap, IconKey, Subscription, TrayClick, TrayIcon, TraySnapshot};

/// The least time between two rebuilds of the snapshot.
const GAP: Duration = Duration::from_millis(40);

// ---------------------------------------------------------------- the host, as the feed sees it

/// What the feed needs of a running host. [`TrayView`] is the real one; tests use a fake.
pub trait HostPort: Send + Sync + 'static {
    /// The host keeps counts only (no model, nothing to click).
    fn log_only(&self) -> bool;
    /// A copy of the model (pictures are shared, not copied).
    fn snapshot(&self) -> Snapshot;
    /// The click engine, for an icon drawn when its owner was `expect`. Called on the interface's thread: it hands the foreground to the
    /// icon's owner first, which only the thread that received the input may do.
    fn click(&self, req: &ClickRequest, expect: OwnerRef) -> ClickOutcome;
    /// The rectangles (screen coordinates) the interface has drawn the icons at, for `Shell_NotifyIconGetRect`.
    fn set_icon_rects(&self, rects: &[(IconId, wire::Rect)]);
}

impl HostPort for TrayView {
    fn log_only(&self) -> bool {
        TrayView::log_only(self)
    }
    fn snapshot(&self) -> Snapshot {
        TrayView::snapshot(self)
    }
    fn click(&self, req: &ClickRequest, expect: OwnerRef) -> ClickOutcome {
        TrayView::click_expecting(self, req, expect)
    }
    fn set_icon_rects(&self, rects: &[(IconId, wire::Rect)]) {
        TrayView::set_icon_rects(self, rects)
    }
}

/// Where an owner process's program lives. Real: `rf_win::tasklist::process_exe_path`. Called on the pump thread only.
pub trait PathResolver: Send + 'static {
    fn exe_path(&mut self, pid: u32) -> Option<String>;
}

struct RealPaths;

impl PathResolver for RealPaths {
    fn exe_path(&mut self, pid: u32) -> Option<String> {
        rf_win::tasklist::process_exe_path(pid).map(|p| p.to_string_lossy().into_owned())
    }
}

// ---------------------------------------------------------------- the translation (pure)

/// The identity the order file remembers an icon by (see the file's head).
pub fn stable_id(icon: &rf_win::trayhost::TrayIcon, path: Option<&str>) -> StableId {
    let exe = path.filter(|p| !p.trim().is_empty()).unwrap_or(icon.owner_exe.as_str());
    match icon.key {
        WireKey::Guid(g) => StableId::of(Some(&g.to_string()), exe, icon.uid),
        WireKey::Wnd { uid, .. } => StableId::of(None, exe, uid),
    }
}

/// The host's pictures are BGRA, the strip's are RGBA: red and blue change places.
pub fn bgra_to_rgba(bgra: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bgra.len() - bgra.len() % 4);
    for p in bgra.as_chunks::<4>().0 {
        out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    out
}

/// The wire event a click of the strip is. Hover is not here: the strip does not send it.
pub fn host_event(kind: ClickKind) -> HostEvent {
    match kind {
        ClickKind::Left => HostEvent::LeftClick,
        ClickKind::LeftDouble => HostEvent::DoubleClick,
        ClickKind::Right => HostEvent::RightClick,
        ClickKind::Middle => HostEvent::MiddleClick,
        ClickKind::KeySelect => HostEvent::KeyboardSelect,
        ClickKind::KeyContext => HostEvent::ContextMenuKey,
    }
}

/// What a click's outcome means for [`Feed::deliver_click`]: false when the icon is not there (any more), or is another program's now.
/// An icon that never gave a callback message, or whose owner refused a message, is there: the click simply had nothing to do.
pub fn icon_was_there(outcome: ClickOutcome) -> bool {
    !matches!(outcome, ClickOutcome::UnknownIcon | ClickOutcome::OwnerGone | ClickOutcome::OwnerChanged)
}

/// The key the strip gets for the host's icon `id` of attach number `epoch` (see the file's head). `None` for a number that does not
/// fit the lower half (the host never gets near it: it keeps 256 icons and numbers them one by one).
pub fn compose_key(epoch: u32, id: IconId) -> Option<IconKey> {
    u32::try_from(id.0).ok().map(|low| (u64::from(epoch) << 32) | u64::from(low))
}

/// The attach number and the host's icon number a key was made of.
pub fn split_key(key: IconKey) -> (u32, IconId) {
    ((key >> 32) as u32, IconId(key & 0xFFFF_FFFF))
}

/// Whether `path` is a program called `exe` (a file name; case does not matter). An empty `exe` (the host could not read it) matches
/// anything: there is nothing to compare with.
pub fn path_is_program(path: &str, exe: &str) -> bool {
    let exe = exe.trim();
    if exe.is_empty() {
        return true;
    }
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    name.to_lowercase() == exe.to_lowercase()
}

/// How often a path that could not be read is asked for again (once per rebuild) before the file name stands for good.
const PATH_TRIES: u8 = 3;

/// The host's rectangle for the dock's.
pub fn wire_rect(r: Rect) -> wire::Rect {
    wire::Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

/// What the pump remembers of an icon between two rebuilds, so that a picture is converted again only when it changed and the strip's
/// per-icon generation moves exactly when the picture or the tip does.
struct Seen {
    image_generation: u64,
    tip: String,
    picture: Option<Arc<Bitmap>>,
    generation: u64,
}

/// What is known of an owner's program path.
struct PathEntry {
    /// The path, once read AND found to be the program the host recorded.
    path: Option<String>,
    /// Reads that failed. A read that fails may be a moment's trouble (the process is busy or being started), so it is tried again at
    /// the next rebuilds, [`PATH_TRIES`] times in all; a path of another program (the process number was reused) is final at once.
    tries: u8,
}

pub struct Converter {
    paths: Box<dyn PathResolver>,
    /// The number of the attach this converter works for; it is the upper half of every key it makes.
    epoch: u32,
    /// `(owner process, file name)` -> what is known of its path. Kept while the process has an icon.
    path_cache: HashMap<(u32, String), PathEntry>,
    /// The owner (window, process) of each icon of the last [`Converter::convert`], by key.
    owners: HashMap<IconKey, OwnerRef>,
    seen: HashMap<IconId, Seen>,
    /// Counts every change of any icon, so a generation is never used twice, not even by an icon that left and came back.
    next_generation: u64,
}

impl Converter {
    pub fn new(paths: Box<dyn PathResolver>) -> Converter {
        Converter { paths, epoch: 0, path_cache: HashMap::new(), owners: HashMap::new(), seen: HashMap::new(), next_generation: 0 }
    }

    /// The converter of attach number `epoch`.
    pub fn with_epoch(mut self, epoch: u32) -> Converter {
        self.epoch = epoch;
        self
    }

    /// The owner of each icon [`Converter::convert`] last returned, by the key it was given.
    pub fn owners(&self) -> HashMap<IconKey, OwnerRef> {
        self.owners.clone()
    }

    fn path_of(&mut self, icon: &rf_win::trayhost::TrayIcon) -> Option<String> {
        // (a GUID icon is told apart by its GUID: its path is not asked for; an owner of unknown process has nothing to ask)
        if icon.owner_pid == 0 || matches!(icon.key, WireKey::Guid(g) if StableId::from_guid(&g.to_string()).is_some()) {
            return None;
        }
        let key = (icon.owner_pid, icon.owner_exe.clone());
        let tries = match self.path_cache.get(&key) {
            Some(e) if e.path.is_some() || e.tries >= PATH_TRIES => return e.path.clone(),
            Some(e) => e.tries,
            None => 0,
        };
        // (the process number may have been reused by another program since the icon was added: a path that is not the program the host
        // recorded is not this icon's, and the reuse does not go away by asking again)
        let entry = match self.paths.exe_path(icon.owner_pid) {
            Some(p) if path_is_program(&p, &icon.owner_exe) => PathEntry { path: Some(p), tries },
            Some(_) => PathEntry { path: None, tries: PATH_TRIES },
            None => PathEntry { path: None, tries: tries + 1 },
        };
        let found = entry.path.clone();
        self.path_cache.insert(key, entry);
        found
    }

    /// The strip's icons for a model: hidden icons left out (`NIS_HIDDEN` means the program does not want to be shown), the host's
    /// order kept.
    pub fn convert(&mut self, snap: &Snapshot) -> Vec<TrayIcon> {
        let mut out = Vec::with_capacity(snap.icons.len());
        let mut present: HashSet<IconId> = HashSet::with_capacity(snap.icons.len());
        let mut live_paths: HashSet<(u32, String)> = HashSet::new();
        let mut owners: HashMap<IconKey, OwnerRef> = HashMap::with_capacity(snap.icons.len());
        for ic in &snap.icons {
            present.insert(ic.id);
            live_paths.insert((ic.owner_pid, ic.owner_exe.clone()));
            if ic.hidden() {
                continue;
            }
            let Some(key) = compose_key(self.epoch, ic.id) else { continue };
            let tip = rules::clean(&ic.tip);
            let id = {
                let path = self.path_of(ic);
                stable_id(ic, path.as_deref())
            };
            let (picture, generation) = match self.seen.get(&ic.id) {
                Some(s) if s.image_generation == ic.image_generation && s.tip == tip => (s.picture.clone(), s.generation),
                Some(s) if s.image_generation == ic.image_generation => {
                    self.next_generation += 1;
                    (s.picture.clone(), self.next_generation)
                }
                _ => {
                    self.next_generation += 1;
                    let pic = ic.image.as_ref().map(|b| Arc::new(Bitmap { width: b.width, height: b.height, rgba: bgra_to_rgba(&b.bgra) }));
                    (pic, self.next_generation)
                }
            };
            self.seen.insert(ic.id, Seen { image_generation: ic.image_generation, tip: tip.clone(), picture: picture.clone(), generation });
            owners.insert(key, (ic.hwnd_handle(), ic.owner_pid));
            out.push(TrayIcon { key, id, tip, picture, generation });
        }
        self.owners = owners;
        // what left the model is forgotten (its picture goes with it)
        self.seen.retain(|id, _| present.contains(id));
        self.path_cache.retain(|k, _| live_paths.contains(k));
        out
    }
}

// ---------------------------------------------------------------- the feed

type Callback = Arc<dyn Fn() + Send + Sync>;

/// One subscriber. The callback is behind its own lock so that dropping the subscription waits for a call in flight and nothing is
/// called after it returns. (A callback must not drop its own subscription.)
struct Slot(Mutex<Option<Callback>>);

impl Slot {
    fn call(&self) {
        let g = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(cb) = g.as_ref() {
            cb();
        }
    }
}

struct Pump {
    quit: Sender<()>,
    thread: Option<JoinHandle<()>>,
}

/// What the strip was last given, and who owned each icon then.
struct Published {
    snap: Arc<TraySnapshot>,
    owners: HashMap<IconKey, OwnerRef>,
}

#[derive(Default)]
struct Inner {
    port: Option<Arc<dyn HostPort>>,
    published: Option<Published>,
    /// The number of the current attach (the upper half of every key); rises with every attach, never reset.
    epoch: u32,
    /// Numbers every published snapshot; never reset, so None -> Some -> None -> Some keeps rising.
    seq: u64,
    subs: Vec<(u64, Arc<Slot>)>,
    next_sub: u64,
    pump: Option<Pump>,
}

pub struct Feed {
    inner: Mutex<Inner>,
}

impl Feed {
    pub fn new() -> Arc<Feed> {
        Arc::new(Feed { inner: Mutex::new(Inner::default()) })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The icons now, or `None` while no host is attached (or it is in log-only mode). A copy of what the pump stored.
    pub fn snapshot(&self) -> Option<TraySnapshot> {
        self.lock().published.as_ref().map(|p| (*p.snap).clone())
    }

    /// Whether a host is attached (log-only or not).
    #[cfg(test)]
    pub fn attached(&self) -> bool {
        self.lock().port.is_some()
    }

    fn notify(slots: Vec<Arc<Slot>>) {
        for s in slots {
            s.call();
        }
    }

    fn slots(inner: &Inner) -> Vec<Arc<Slot>> {
        inner.subs.iter().map(|(_, s)| s.clone()).collect()
    }

    /// Calls `on_change` (from any thread) whenever [`Feed::snapshot`] changes, until the subscription is dropped.
    pub fn subscribe(self: &Arc<Self>, on_change: Callback) -> Subscription {
        let slot = Arc::new(Slot(Mutex::new(Some(on_change))));
        let id = {
            let mut g = self.lock();
            g.next_sub += 1;
            let id = g.next_sub;
            g.subs.push((id, slot.clone()));
            id
        };
        let feed = Arc::downgrade(self);
        Subscription::new(move || {
            // (stop the calls first: taking the lock waits for a call in flight)
            slot.0.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(feed) = feed.upgrade() {
                feed.lock().subs.retain(|(i, _)| *i != id);
            }
        })
    }

    /// Stores `icons` (and their owners) as the new snapshot when they differ from the last one, and tells the subscribers. True when it
    /// changed. An owner that changed under an icon that looks the same IS a change: the click check works against what was stored.
    fn publish(&self, icons: Vec<TrayIcon>, owners: HashMap<IconKey, OwnerRef>) -> bool {
        let slots = {
            let mut g = self.lock();
            if g.port.is_none() {
                // (detached meanwhile)
                return false;
            }
            if g.published.as_ref().is_some_and(|p| p.snap.icons == icons && p.owners == owners) {
                return false;
            }
            g.seq += 1;
            let generation = g.seq;
            g.published = Some(Published { snap: Arc::new(TraySnapshot { generation, icons }), owners });
            Feed::slots(&g)
        };
        Feed::notify(slots);
        true
    }

    /// Attaches a running host. An earlier one is detached first.
    pub fn attach(self: &Arc<Self>, port: Arc<dyn HostPort>, events: Receiver<TrayEvent>, paths: Box<dyn PathResolver>, gap: Duration) -> Attachment {
        self.detach();
        let log_only = port.log_only();
        let (quit_tx, quit_rx) = crossbeam_channel::bounded::<()>(0);
        let epoch = {
            let mut g = self.lock();
            g.epoch = g.epoch.wrapping_add(1);
            g.port = Some(port.clone());
            g.epoch
        };
        if !log_only {
            let feed = self.clone();
            let spawned = std::thread::Builder::new().name("rf-tray-feed".into()).spawn(move || run_pump(feed, port, events, quit_rx, paths, gap, epoch));
            match spawned {
                Ok(thread) => self.lock().pump = Some(Pump { quit: quit_tx, thread: Some(thread) }),
                Err(e) => {
                    log::warn!("tray feed: the pump thread could not start: {e}");
                    self.lock().port = None;
                }
            }
        }
        Attachment { feed: Some(self.clone()), epoch }
    }

    /// Ends the pump, forgets the host, and tells the subscribers the snapshot is gone. Safe to call at any time and twice.
    pub fn detach(&self) {
        self.detach_if(None);
    }

    /// [`Feed::detach`], but with `Some(epoch)` only if that attach is still the current one: a guard that was replaced by a later
    /// attach must not take the later host away.
    fn detach_if(&self, epoch: Option<u32>) {
        let pump = {
            let mut g = self.lock();
            if epoch.is_some_and(|e| e != g.epoch) {
                return;
            }
            g.port = None;
            g.pump.take()
        };
        if let Some(mut p) = pump {
            drop(p.quit);
            if let Some(t) = p.thread.take() {
                let _ = t.join();
            }
        }
        let slots = {
            let mut g = self.lock();
            g.published.take().map(|_| Feed::slots(&g))
        };
        if let Some(slots) = slots {
            Feed::notify(slots);
        }
    }

    /// A click on the icon the strip knows as `key`. False when there is no host, the icon is not in the snapshot the strip drew, or it
    /// has another identity than the click names.
    pub fn deliver_click(&self, key: IconKey, click: TrayClick) -> bool {
        let (epoch, id) = split_key(key);
        // (what is checked is what the strip drew: its snapshot, and the owner each icon had in it)
        let (port, owner) = {
            let g = self.lock();
            let owner = g
                .published
                .as_ref()
                .filter(|p| epoch == g.epoch && p.snap.icons.iter().any(|i| i.key == key && i.id == click.id))
                .and_then(|p| p.owners.get(&key).copied());
            (g.port.clone(), owner)
        };
        let (Some(port), Some(owner)) = (port, owner) else { return false };
        let at = Point { x: click.at.0, y: click.at.1 };
        // (a click from the keyboard carries the icon's corner as `at`: the engine anchors those events at `icon_top_left`)
        let req = ClickRequest { id, event: host_event(click.kind), cursor: at, icon_top_left: at, opts: ClickOptions::default() };
        let outcome = port.click(&req, owner);
        log::debug!("tray feed: click {:?} -> {:?}", click.kind, outcome);
        icon_was_there(outcome)
    }

    /// Tells the host where the icons are on the screen now.
    pub fn report_icon_rects(&self, rects: &[(IconKey, Rect)]) {
        let (port, current) = {
            let g = self.lock();
            (g.port.clone(), g.epoch)
        };
        let Some(port) = port else { return };
        // (rectangles of icons of an earlier host are not this host's)
        let list: Vec<(IconId, wire::Rect)> = rects
            .iter()
            .filter_map(|(k, r)| {
                let (epoch, id) = split_key(*k);
                (epoch == current).then(|| (id, wire_rect(*r)))
            })
            .collect();
        port.set_icon_rects(&list);
    }
}

impl Default for Feed {
    fn default() -> Self {
        Feed { inner: Mutex::new(Inner::default()) }
    }
}

/// Keeps a host attached; dropping it detaches (and joins the pump).
pub struct Attachment {
    feed: Option<Arc<Feed>>,
    /// Which attach this guard is for (see `Inner::epoch`).
    epoch: u32,
}

impl Drop for Attachment {
    fn drop(&mut self) {
        if let Some(f) = self.feed.take() {
            f.detach_if(Some(self.epoch));
        }
    }
}

fn run_pump(feed: Arc<Feed>, port: Arc<dyn HostPort>, events: Receiver<TrayEvent>, quit: Receiver<()>, paths: Box<dyn PathResolver>, gap: Duration, epoch: u32) {
    let mut conv = Converter::new(paths).with_epoch(epoch);
    let rebuild = |conv: &mut Converter| {
        let icons = conv.convert(&port.snapshot());
        feed.publish(icons, conv.owners());
    };
    rebuild(&mut conv);
    loop {
        // the rate limit, after EVERY rebuild (the first one too): hints that come during the gap wait in the queue and are the next one
        match quit.recv_timeout(gap) {
            Err(RecvTimeoutError::Timeout) => {}
            _ => return,
        }
        crossbeam_channel::select! {
            recv(quit) -> _ => return,
            recv(events) -> msg => {
                if msg.is_err() {
                    // (the host's queue is gone: nothing more will come)
                    return;
                }
            }
        }
        // everything queued is one rebuild; hints are read BEFORE the model so that a change that lands meanwhile wakes us again
        while events.try_recv().is_ok() {}
        rebuild(&mut conv);
    }
}

// ---------------------------------------------------------------- the process's one feed

fn global() -> &'static Arc<Feed> {
    static FEED: std::sync::OnceLock<Arc<Feed>> = std::sync::OnceLock::new();
    FEED.get_or_init(Feed::new)
}

/// The shell host calls this when its tray host is up. Keep the returned guard as long as the host runs; dropping it detaches.
pub fn attach(view: TrayView, events: Receiver<TrayEvent>) -> Attachment {
    global().attach(Arc::new(view), events, Box::new(RealPaths), GAP)
}

pub(super) fn snapshot() -> Option<TraySnapshot> {
    global().snapshot()
}

pub(super) fn subscribe(on_change: Callback) -> Subscription {
    global().subscribe(on_change)
}

pub(super) fn deliver_click(key: IconKey, click: TrayClick) -> bool {
    global().deliver_click(key, click)
}

pub(super) fn report_icon_rects(rects: &[(IconKey, Rect)]) {
    global().report_icon_rects(rects);
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::trayhost::testkit::{FakeHost, IconSpec};
    use rf_win::tray_wire::WireGuid;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Paths from a table; counts how often it was asked.
    struct Table {
        map: HashMap<u32, String>,
        asked: Arc<AtomicUsize>,
    }

    impl PathResolver for Table {
        fn exe_path(&mut self, pid: u32) -> Option<String> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.map.get(&pid).cloned()
        }
    }

    fn table(entries: &[(u32, &str)]) -> (Box<dyn PathResolver>, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        (Box::new(Table { map: entries.iter().map(|(p, s)| (*p, s.to_string())).collect(), asked: asked.clone() }), asked)
    }

    fn pic(w: u32, h: u32, b: u8, g: u8, r: u8, a: u8) -> (u32, u32, Vec<u8>) {
        (w, h, (0..w * h).flat_map(|_| [b, g, r, a]).collect())
    }

    fn spec(hwnd: u32, uid: u32, pid: u32, exe: &str, tip: &str) -> IconSpec {
        IconSpec::new(hwnd, uid, pid, exe, tip)
    }

    fn convert(host: &FakeHost, paths: &[(u32, &str)]) -> Vec<TrayIcon> {
        Converter::new(table(paths).0).convert(&host.view.snapshot())
    }

    #[test]
    fn the_identity_is_the_guid_else_the_program_path_and_id() {
        let h = FakeHost::new(false);
        let mut with_guid = spec(1, 5, 10, "mail.exe", "a");
        with_guid.guid = Some(WireGuid::from_parts(0x7820AE73, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]));
        h.add(&with_guid);
        h.add(&spec(2, 7, 20, "cloud.exe", "b"));
        let icons = convert(&h, &[(10, "C:\\Apps\\Mail.exe"), (20, "C:\\Apps\\Cloud.EXE")]);
        assert_eq!(icons[0].id, StableId::from_guid("{7820ae73-23e3-4229-82c1-e41cb67d5b9c}").unwrap());
        assert_eq!(icons[1].id, StableId::from_program("c:\\apps\\cloud.exe", 7), "the path is resolved, lower-cased by the identity");
        assert_ne!(icons[0].key, icons[1].key);
    }

    #[test]
    fn without_a_readable_path_the_file_name_stands_in_and_only_then_nothing() {
        let h = FakeHost::new(false);
        h.add(&spec(1, 3, 10, "mail.exe", "a"));
        h.add(&spec(2, 3, 0, "", "b"));
        let icons = convert(&h, &[]);
        assert_eq!(icons[0].id, StableId::from_program("mail.exe", 3));
        assert_eq!(icons[1].id, StableId::from_program("", 3));
        // an owner whose process is unknown is not asked about
        let (paths, asked) = table(&[]);
        let mut c = Converter::new(paths);
        c.convert(&h.view.snapshot());
        assert_eq!(asked.load(Ordering::SeqCst), 1, "only the owner with a process was looked up");
    }

    #[test]
    fn a_path_is_looked_up_once_per_process_not_once_per_rebuild() {
        let h = FakeHost::new(false);
        h.add(&spec(1, 1, 10, "a.exe", "a"));
        h.add(&spec(2, 2, 10, "a.exe", "b"));
        let (paths, asked) = table(&[(10, "C:\\a.exe")]);
        let mut c = Converter::new(paths);
        for _ in 0..5 {
            c.convert(&h.view.snapshot());
        }
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        // when the program has no icon left its entry goes; it is looked up again if it returns
        h.delete(spec(1, 1, 10, "a.exe", "").as_ref());
        h.delete(spec(2, 2, 10, "a.exe", "").as_ref());
        c.convert(&h.view.snapshot());
        h.add(&spec(1, 1, 10, "a.exe", "a"));
        c.convert(&h.view.snapshot());
        assert_eq!(asked.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn pictures_are_rgba_and_the_tip_is_cleaned() {
        let h = FakeHost::new(false);
        let mut s = spec(1, 1, 10, "a.exe", "  Posta \r\n  3   yeni\u{7} ileti ");
        s.picture = Some(pic(2, 2, 10, 20, 30, 255)); // blue 10, green 20, red 30
        h.add(&s);
        let icons = convert(&h, &[(10, "C:\\a.exe")]);
        let p = icons[0].picture.as_ref().expect("a picture");
        assert!(p.is_whole());
        assert_eq!(&p.rgba[..4], &[30, 20, 10, 255], "red first");
        assert_eq!(icons[0].tip, "Posta\n3 yeni ileti");
        assert_eq!(bgra_to_rgba(&[1, 2, 3, 4, 9]), vec![3, 2, 1, 4], "a stray byte is dropped, not carried");
    }

    #[test]
    fn a_hidden_icon_is_not_in_the_strip_and_comes_back_with_a_new_generation() {
        let h = FakeHost::new(false);
        let a = spec(1, 1, 10, "a.exe", "a");
        h.add(&a);
        h.add(&spec(2, 2, 11, "b.exe", "b"));
        let mut c = Converter::new(table(&[]).0);
        let first = c.convert(&h.view.snapshot());
        assert_eq!(first.len(), 2);
        h.set_hidden(a.as_ref(), true);
        let hidden = c.convert(&h.view.snapshot());
        assert_eq!(hidden.iter().map(|i| i.tip.as_str()).collect::<Vec<_>>(), vec!["b"]);
        h.set_hidden(a.as_ref(), false);
        let back = c.convert(&h.view.snapshot());
        assert_eq!(back.len(), 2);
        assert_eq!(back[1].key, first[1].key, "the other icon keeps its key");
        assert_eq!(back[1].generation, first[1].generation, "and its generation: nothing about it changed");
    }

    #[test]
    fn an_icons_generation_moves_with_its_picture_or_tip_and_with_nothing_else() {
        let h = FakeHost::new(false);
        let mut s = spec(1, 1, 10, "a.exe", "bir");
        s.picture = Some(pic(2, 2, 1, 1, 1, 255));
        h.add(&s);
        let mut c = Converter::new(table(&[]).0);
        let g0 = c.convert(&h.view.snapshot())[0].generation;
        // a balloon, a version, the same tip again: nothing the strip draws
        h.balloon(s.as_ref());
        h.set_tip(s.as_ref(), "bir");
        let same = c.convert(&h.view.snapshot());
        assert_eq!(same[0].generation, g0);
        // the tip
        h.set_tip(s.as_ref(), "iki");
        let g1 = c.convert(&h.view.snapshot())[0].generation;
        assert_ne!(g1, g0);
        // the picture
        h.set_picture(s.as_ref(), &pic(2, 2, 9, 9, 9, 255));
        let after = c.convert(&h.view.snapshot());
        assert_ne!(after[0].generation, g1);
        assert_eq!(after[0].picture.as_ref().unwrap().rgba[0], 9);
        // the picture Arc is reused while it does not change
        let again = c.convert(&h.view.snapshot());
        assert!(Arc::ptr_eq(after[0].picture.as_ref().unwrap(), again[0].picture.as_ref().unwrap()));
    }

    #[test]
    fn clicks_map_one_to_one_to_the_wire_events() {
        for (k, e) in [
            (ClickKind::Left, HostEvent::LeftClick),
            (ClickKind::LeftDouble, HostEvent::DoubleClick),
            (ClickKind::Right, HostEvent::RightClick),
            (ClickKind::Middle, HostEvent::MiddleClick),
            (ClickKind::KeySelect, HostEvent::KeyboardSelect),
            (ClickKind::KeyContext, HostEvent::ContextMenuKey),
        ] {
            assert_eq!(host_event(k), e);
        }
        assert!(icon_was_there(ClickOutcome::Sent(2)));
        assert!(icon_was_there(ClickOutcome::NoCallback));
        assert!(icon_was_there(ClickOutcome::Failed { sent: 0 }));
        assert!(!icon_was_there(ClickOutcome::UnknownIcon));
        assert!(!icon_was_there(ClickOutcome::OwnerGone));
        assert_eq!(wire_rect(Rect::new(1, 2, 3, 4)), wire::Rect { left: 1, top: 2, right: 3, bottom: 4 });
    }

    // ------------------------------------------------------------ the feed over a fake host

    const WAIT: Duration = Duration::from_secs(5);

    /// A subscriber that counts its calls and can be waited on.
    fn counter(feed: &Arc<Feed>) -> (Subscription, Receiver<()>) {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        let sub = feed.subscribe(Arc::new(move || {
            let _ = tx.send(());
        }));
        (sub, rx)
    }

    fn attach_fake(feed: &Arc<Feed>, host: &FakeHost, paths: Box<dyn PathResolver>) -> Attachment {
        feed.attach(Arc::new(host.view.clone()), host.events.clone(), paths, Duration::from_millis(5))
    }

    /// Waits until the feed's snapshot satisfies `ok` (the pump runs on its own thread).
    fn wait_for(feed: &Arc<Feed>, rx: &Receiver<()>, ok: impl Fn(&Option<TraySnapshot>) -> bool) -> Option<TraySnapshot> {
        let end = std::time::Instant::now() + WAIT;
        loop {
            let s = feed.snapshot();
            if ok(&s) {
                return s;
            }
            assert!(std::time::Instant::now() < end, "the feed never reached the expected state");
            let _ = rx.recv_timeout(Duration::from_millis(50));
        }
    }

    #[test]
    fn with_no_host_attached_there_is_no_snapshot_and_no_click_goes_anywhere() {
        let feed = Feed::new();
        assert!(feed.snapshot().is_none());
        assert!(!feed.attached());
        let id = StableId::from_program("x", 1);
        assert!(!feed.deliver_click(1, TrayClick { kind: ClickKind::Left, at: (0, 0), id }));
        feed.report_icon_rects(&[(1, Rect::new(0, 0, 1, 1))]); // nothing to tell, and no panic
    }

    #[test]
    fn attaching_publishes_what_is_there_and_tells_subscribers_then_follows_changes() {
        let feed = Feed::new();
        let (_sub, rx) = counter(&feed);
        let host = FakeHost::new(false);
        let a = spec(1, 1, 10, "a.exe", "bir");
        host.add(&a);
        let att = attach_fake(&feed, &host, table(&[(10, "C:\\a.exe")]).0);
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap();
        assert_eq!(snap.icons[0].tip, "bir");
        let g0 = snap.generation;
        // a tip change, then an added icon
        host.set_tip(a.as_ref(), "iki");
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons[0].tip == "iki")).unwrap();
        assert!(snap.generation > g0);
        host.add(&spec(2, 2, 11, "b.exe", "uc"));
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 2)).unwrap();
        assert_eq!(snap.icons[1].tip, "uc");
        drop(att);
        assert!(feed.snapshot().is_none(), "the host stopped: the snapshot is gone");
        assert!(!feed.attached());
    }

    #[test]
    fn detaching_tells_the_subscribers_once_and_a_dropped_subscription_hears_nothing() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        host.add(&spec(1, 1, 10, "a.exe", "a"));
        let (sub, rx) = counter(&feed);
        let (_other, rx2) = counter(&feed);
        let att = attach_fake(&feed, &host, table(&[]).0);
        wait_for(&feed, &rx2, |s| s.is_some());
        while rx.try_recv().is_ok() {}
        while rx2.try_recv().is_ok() {}
        drop(sub);
        drop(att);
        assert!(rx.try_recv().is_err(), "silent after its handle is dropped");
        assert_eq!(rx2.try_iter().count(), 1, "the other subscriber was told the snapshot is gone, once");
        // detaching again is harmless and tells nobody
        feed.detach();
        assert!(rx2.try_recv().is_err());
    }

    #[test]
    fn a_change_the_strip_does_not_draw_wakes_nobody() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        let a = spec(1, 1, 10, "a.exe", "a");
        host.add(&a);
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[]).0);
        wait_for(&feed, &rx, |s| s.is_some());
        std::thread::sleep(Duration::from_millis(60));
        while rx.try_recv().is_ok() {}
        host.balloon(a.as_ref());
        host.balloon(a.as_ref());
        // a marker change that does alter the snapshot, to know the pump has been through the balloons
        host.set_tip(a.as_ref(), "marker");
        wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons[0].tip == "marker"));
        assert!(rx.try_iter().count() <= 1, "the balloons alone woke nobody; the tip change woke at most once");
    }

    #[test]
    fn a_burst_of_changes_wakes_the_interface_at_most_once_per_gap_and_the_last_change_is_always_seen() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        let a = spec(1, 1, 10, "a.exe", "0");
        host.add(&a);
        let (_sub, rx) = counter(&feed);
        // every wake-up is stamped on the pump's thread, inside the call
        let stamps: Arc<Mutex<Vec<std::time::Instant>>> = Arc::default();
        let log = stamps.clone();
        let _stamp = feed.subscribe(Arc::new(move || log.lock().unwrap().push(std::time::Instant::now())));
        let gap = Duration::from_millis(40);
        let _att = feed.attach(Arc::new(host.view.clone()), host.events.clone(), table(&[]).0, gap);
        wait_for(&feed, &rx, |s| s.is_some());
        let begun = std::time::Instant::now();
        // a change every ~2 ms for a while: far faster than the gap
        for n in 1..=100 {
            host.set_tip(a.as_ref(), &n.to_string());
            std::thread::sleep(Duration::from_millis(2));
        }
        wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons[0].tip == "100"));
        let stamps = stamps.lock().unwrap().clone();
        assert!(stamps.len() >= 2, "the burst was seen");
        for w in stamps.windows(2) {
            assert!(w[1] - w[0] + Duration::from_millis(2) >= gap, "two rebuilds {:?} apart, the gap is {gap:?}", w[1] - w[0]);
        }
        let span = begun.elapsed();
        let most = (span.as_millis() / gap.as_millis()) as usize + 3;
        assert!(stamps.len() <= most, "{} wake-ups in {span:?} for 100 changes (at most {most} at one per {gap:?})", stamps.len());
    }

    #[test]
    fn a_log_only_host_is_attached_but_gives_no_snapshot() {
        let feed = Feed::new();
        let host = FakeHost::new(true);
        let att = attach_fake(&feed, &host, table(&[]).0);
        assert!(feed.attached());
        assert!(feed.snapshot().is_none());
        assert!(!feed.deliver_click(1, TrayClick { kind: ClickKind::Left, at: (0, 0), id: StableId::from_program("x", 1) }));
        drop(att);
        assert!(!feed.attached());
    }

    #[test]
    fn a_click_reaches_the_owner_only_with_the_right_key_and_identity() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        host.add(&spec(0x40, 7, 50, "a.exe", "a"));
        host.add(&spec(0x41, 8, 51, "b.exe", "b"));
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[(50, "C:\\a.exe"), (51, "C:\\b.exe")]).0);
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 2)).unwrap();
        let (a, b) = (&snap.icons[0], &snap.icons[1]);
        // the identity belongs to the other icon: refused, nothing sent
        assert!(!feed.deliver_click(a.key, TrayClick { kind: ClickKind::Left, at: (5, 6), id: b.id.clone() }));
        // an unknown key
        assert!(!feed.deliver_click(9999, TrayClick { kind: ClickKind::Left, at: (5, 6), id: a.id.clone() }));
        assert!(host.sent().is_empty());
        // the right pair: version 0 gets a button down and up on the callback message
        assert!(feed.deliver_click(a.key, TrayClick { kind: ClickKind::Left, at: (5, 6), id: a.id.clone() }));
        let sent = host.sent();
        assert_eq!(sent.len(), 2);
        assert!(sent.iter().all(|s| s.0 == 0x40 && s.1 == 0x8005 && s.2 == 7), "to the owner of the first icon, with its id: {sent:?}");
    }

    #[test]
    fn a_click_after_the_owner_changed_under_the_same_icon_is_refused() {
        // The pump is not running: the snapshot the strip drew is published by hand and the host changes afterwards, so nothing races.
        let feed = Feed::new();
        let host = FakeHost::new(false);
        let mut a = spec(0x40, 7, 50, "a.exe", "tip");
        a.guid = Some(WireGuid::from_parts(1, 2, 3, [4; 8]));
        host.add(&a);
        feed.lock().port = Some(Arc::new(host.view.clone()));
        let mut conv = Converter::new(table(&[]).0);
        let icons = conv.convert(&host.view.snapshot());
        assert!(feed.publish(icons.clone(), conv.owners()));
        // another program registers the same GUID: the model keeps ONE icon (same id, same identity) with the new owner
        let mut b = spec(0x80, 7, 60, "b.exe", "tip");
        b.guid = a.guid;
        host.add(&b);
        assert_eq!(host.view.snapshot().icons.len(), 1);
        let i = &icons[0];
        assert!(!feed.deliver_click(i.key, TrayClick { kind: ClickKind::Left, at: (1, 2), id: i.id.clone() }), "the user clicked A's picture");
        assert!(host.sent().is_empty(), "nothing went to B: {:?}", host.sent());
    }

    #[test]
    fn a_click_on_an_icon_whose_owner_is_gone_is_false_and_sends_nothing() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        host.add(&spec(0x40, 7, 50, "a.exe", "a"));
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[]).0);
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap();
        host.owners.remove_window(0x40);
        let i = &snap.icons[0];
        assert!(!feed.deliver_click(i.key, TrayClick { kind: ClickKind::Right, at: (0, 0), id: i.id.clone() }));
        assert!(host.sent().is_empty());
    }

    #[test]
    fn a_click_from_the_keyboard_is_anchored_at_the_icons_corner() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        let mut s = spec(0x40, 7, 50, "a.exe", "a");
        s.version = rf_win::tray_wire::IconVersion::V4;
        host.add(&s);
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[]).0);
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap();
        let i = &snap.icons[0];
        assert!(feed.deliver_click(i.key, TrayClick { kind: ClickKind::KeySelect, at: (490, 590), id: i.id.clone() }));
        // version 4: MAKEWPARAM(x, y) is the anchor
        assert_eq!(host.sent()[0].2, (590usize << 16) | 490);
    }

    #[test]
    fn rectangles_reach_the_host_for_icons_it_holds() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        host.add(&spec(0x40, 7, 50, "a.exe", "a"));
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[]).0);
        let snap = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap();
        let key = snap.icons[0].key;
        // (it must not panic or block; the host keeps only rectangles of icons it holds)
        feed.report_icon_rects(&[(key, Rect::new(10, 20, 34, 44)), (4242, Rect::new(0, 0, 1, 1))]);
        assert_eq!(host.rects(), vec![(10, 20, 34, 44)]);
    }

    #[test]
    fn a_second_attach_replaces_the_first() {
        let feed = Feed::new();
        let first = FakeHost::new(false);
        first.add(&spec(1, 1, 10, "a.exe", "one"));
        let second = FakeHost::new(false);
        second.add(&spec(2, 2, 11, "b.exe", "two"));
        let (_sub, rx) = counter(&feed);
        let a1 = attach_fake(&feed, &first, table(&[]).0);
        wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons[0].tip == "one"));
        let a2 = attach_fake(&feed, &second, table(&[]).0);
        wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons[0].tip == "two"));
        // the first guard is stale: dropping it must not take the second host away
        drop(a1);
        assert!(feed.attached());
        assert_eq!(feed.snapshot().expect("the second host's snapshot stays").icons[0].tip, "two");
        drop(a2);
        assert!(!feed.attached());
        assert!(feed.snapshot().is_none());
    }

    #[test]
    fn a_restarted_host_never_reuses_a_key_of_an_earlier_one() {
        let feed = Feed::new();
        let (_sub, rx) = counter(&feed);
        let first = FakeHost::new(false);
        first.add(&spec(1, 1, 10, "a.exe", "one"));
        let att = attach_fake(&feed, &first, table(&[]).0);
        let old = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap().icons.remove(0);
        drop(att);
        // the new host numbers its icons from 1 again, and the program is the same
        let second = FakeHost::new(false);
        second.add(&spec(1, 1, 10, "a.exe", "one"));
        let _att = attach_fake(&feed, &second, table(&[]).0);
        let new = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap().icons.remove(0);
        assert_ne!(old.key, new.key, "a strip that caches by key and generation would show the old picture");
        assert_eq!(old.id, new.id, "(the identity is the same program: the order file still applies)");
        // a click or a rectangle the strip made for the old host's icon does nothing to the new one
        assert!(!feed.deliver_click(old.key, TrayClick { kind: ClickKind::Left, at: (0, 0), id: old.id.clone() }));
        feed.report_icon_rects(&[(old.key, Rect::new(1, 2, 3, 4))]);
        assert!(second.sent().is_empty());
        assert!(second.rects().is_empty());
        assert!(feed.deliver_click(new.key, TrayClick { kind: ClickKind::Left, at: (0, 0), id: new.id.clone() }));
        feed.report_icon_rects(&[(new.key, Rect::new(1, 2, 3, 4))]);
        assert_eq!(second.rects(), vec![(1, 2, 3, 4)]);
    }

    #[test]
    fn an_owner_that_changes_under_the_same_icon_is_a_change_of_the_snapshot_and_clicks_then_go_to_the_new_owner() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        let mut a = spec(0x40, 7, 50, "a.exe", "tip");
        a.guid = Some(WireGuid::from_parts(1, 2, 3, [4; 8]));
        host.add(&a);
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, table(&[]).0);
        let first = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.icons.len() == 1)).unwrap();
        let mut b = spec(0x80, 7, 60, "b.exe", "tip");
        b.guid = a.guid;
        host.add(&b);
        let g0 = first.generation;
        let second = wait_for(&feed, &rx, |s| s.as_ref().is_some_and(|s| s.generation > g0)).unwrap();
        assert_eq!(second.icons, first.icons, "nothing the strip draws differs");
        let i = &second.icons[0];
        assert!(feed.deliver_click(i.key, TrayClick { kind: ClickKind::Left, at: (1, 2), id: i.id.clone() }));
        assert!(host.sent().iter().all(|m| m.0 == 0x80), "to the owner of the snapshot that was drawn last: {:?}", host.sent());
    }

    // ------------------------------------------------------------ paths

    /// Fails `fail` times, then answers `path`; counts the calls.
    struct Flaky {
        fail: usize,
        path: Option<String>,
        asked: Arc<AtomicUsize>,
    }

    impl PathResolver for Flaky {
        fn exe_path(&mut self, _pid: u32) -> Option<String> {
            let n = self.asked.fetch_add(1, Ordering::SeqCst);
            if n < self.fail { None } else { self.path.clone() }
        }
    }

    fn flaky(fail: usize, path: Option<&str>) -> (Box<dyn PathResolver>, Arc<AtomicUsize>) {
        let asked = Arc::new(AtomicUsize::new(0));
        (Box::new(Flaky { fail, path: path.map(str::to_string), asked: asked.clone() }), asked)
    }

    #[test]
    fn a_path_is_the_programs_only_if_its_file_name_is_the_one_the_host_recorded() {
        assert!(path_is_program(r"C:\Apps\Mail.exe", "mail.exe"));
        assert!(path_is_program("C:/Apps/MAIL.EXE", "Mail.exe"));
        assert!(path_is_program(r"C:\Apps\Anything.exe", ""), "no recorded name: nothing to compare");
        assert!(!path_is_program(r"C:\Apps\Other.exe", "mail.exe"));
        assert!(!path_is_program(r"C:\Apps\xmail.exe", "mail.exe"), "a longer name is not the name");
    }

    #[test]
    fn a_process_number_reused_by_another_program_does_not_lend_its_path() {
        let h = FakeHost::new(false);
        h.add(&spec(1, 3, 10, "mail.exe", "a"));
        let (paths, asked) = table(&[(10, r"C:\Apps\Other.exe")]);
        let mut c = Converter::new(paths);
        for _ in 0..4 {
            let icons = c.convert(&h.view.snapshot());
            assert_eq!(icons[0].id, StableId::from_program("mail.exe", 3), "the recorded file name stands in");
        }
        assert_eq!(asked.load(Ordering::SeqCst), 1, "a path that belongs to another program is not asked for again");
    }

    #[test]
    fn a_path_that_could_not_be_read_is_tried_again_a_few_times_and_then_left() {
        let h = FakeHost::new(false);
        h.add(&spec(1, 3, 10, "mail.exe", "a"));
        // one failure: the next rebuild gets the path and the identity moves from the file name to the path
        let (paths, asked) = flaky(1, Some(r"C:\Apps\Mail.exe"));
        let mut c = Converter::new(paths);
        assert_eq!(c.convert(&h.view.snapshot())[0].id, StableId::from_program("mail.exe", 3));
        assert_eq!(c.convert(&h.view.snapshot())[0].id, StableId::from_program(r"c:\apps\mail.exe", 3));
        c.convert(&h.view.snapshot());
        assert_eq!(asked.load(Ordering::SeqCst), 2, "a path that was read is kept");
        // never readable: asked PATH_TRIES times, no more
        let (paths, asked) = flaky(usize::MAX, None);
        let mut c = Converter::new(paths);
        for _ in 0..(PATH_TRIES as usize + 5) {
            c.convert(&h.view.snapshot());
        }
        assert_eq!(asked.load(Ordering::SeqCst), PATH_TRIES as usize);
    }

    #[test]
    fn keys_carry_the_attach_number_in_their_upper_half() {
        let k = compose_key(3, IconId(9)).unwrap();
        assert_eq!(split_key(k), (3, IconId(9)));
        assert_ne!(compose_key(1, IconId(1)), compose_key(2, IconId(1)));
        assert_eq!(compose_key(0, IconId(5)), Some(5), "a converter that was given no number makes the host's own");
        assert_eq!(compose_key(1, IconId(u64::from(u32::MAX) + 1)), None);
    }

    /// The thread (id and name) each lookup ran on.
    type Threads = Arc<Mutex<Vec<(std::thread::ThreadId, Option<String>)>>>;

    /// Records the thread every lookup runs on.
    struct OnThread {
        threads: Threads,
    }

    impl PathResolver for OnThread {
        fn exe_path(&mut self, _pid: u32) -> Option<String> {
            let t = std::thread::current();
            self.threads.lock().unwrap().push((t.id(), t.name().map(str::to_string)));
            Some("C:\\a.exe".into())
        }
    }

    #[test]
    fn the_pump_does_its_work_off_the_interface_thread() {
        let feed = Feed::new();
        let host = FakeHost::new(false);
        host.add(&spec(1, 1, 10, "a.exe", "a"));
        host.add(&spec(2, 2, 11, "a.exe", "b"));
        let threads = Arc::new(Mutex::new(Vec::new()));
        let (_sub, rx) = counter(&feed);
        let _att = attach_fake(&feed, &host, Box::new(OnThread { threads: threads.clone() }));
        wait_for(&feed, &rx, |s| s.is_some());
        let before = threads.lock().unwrap().len();
        assert!(before >= 2, "the lookups were made");
        // (the interface's reads and clicks copy what was stored; they never look a path up)
        for _ in 0..100 {
            let _ = feed.snapshot();
        }
        let i = feed.snapshot().unwrap().icons.remove(0);
        feed.deliver_click(i.key, TrayClick { kind: ClickKind::Left, at: (0, 0), id: i.id.clone() });
        feed.report_icon_rects(&[(i.key, Rect::new(0, 0, 1, 1))]);
        let seen = threads.lock().unwrap().clone();
        assert_eq!(seen.len(), before, "no lookup on the caller's thread");
        let me = std::thread::current().id();
        assert!(seen.iter().all(|(id, name)| *id != me && name.as_deref() == Some("rf-tray-feed")), "{seen:?}");
    }

    fn code() -> String {
        let src = include_str!("tray_feed.rs");
        src.split("#[cfg(test)]
mod tests").next().unwrap_or(src).lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn no_log_line_prints_a_tip_a_name_or_a_path_and_no_window_function_is_called() {
        let code = code();
        let mut statements = 0;
        for stmt in code.split("log::").skip(1) {
            statements += 1;
            let line = stmt.split(");").next().unwrap_or("");
            for banned in ["tip", "title", "name", "path", "exe", "text", "snapshot", "icons", "picture"] {
                assert!(!line.contains(banned), "a log line mentions `{banned}`: {line}");
            }
            // and what it PRINTS is on a short list: an error, or the kind of click and what became of it
            let args: Vec<&str> = line.splitn(3, '"').collect();
            let printed = args.get(2).map_or("", |a| a.trim().trim_start_matches(',').trim());
            assert!(["", "click.kind, outcome"].contains(&printed), "a log line prints `{printed}`: {line}");
            let placeholders: Vec<&str> = args.get(1).map_or(vec![], |f| f.split('{').skip(1).filter_map(|p| p.split('}').next()).collect());
            assert!(placeholders.iter().all(|p| ["", ":?", "e"].contains(p)), "a log line has {placeholders:?}: {line}");
        }
        assert!(statements >= 2, "the scan found the log lines");
        for banned in ["CreateWindow", "SendMessage", "PostMessage", "SetWindowPos", "ShowWindow", "windows::", "TrayHost::start"] {
            assert!(!code.contains(banned), "{banned}");
        }
    }
}
