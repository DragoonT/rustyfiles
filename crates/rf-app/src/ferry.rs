//! Ferry, the Windows side: sending files to rustyfiles on the network's other devices, and taking theirs. The network part is
//! `crates/rf-nearby` (shared with the macOS app); this is the service in the main process, the Status Center cards for the transfers,
//! the small window that asks about an offer or a pairing code, the "Send with Ferry" menu and the Settings page.
//!
//! Off until the user turns it on (Settings > Ferry): it listens on a network port, and Windows' firewall may ask about that. It runs
//! while rustyfiles runs (a window open, or in the notification area).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use futures::StreamExt as _;
use futures::channel::mpsc;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_nearby::{Config, EstimateSource, LinkKind, NearbyListener, NetStatus, Nearby, Offer, Peer, SpeedResult, TransferResult, Visibility};

use crate::fluent_menu::{Item, MenuIcon};
use crate::settings::{self, FerryVisibility, Settings};
use crate::t;
use crate::theme::{self, Palette};

/// "Everyone" lasts this long, then the crate falls back to trusted devices.
const EVERYONE_MINUTES: u32 = 10;
/// An unanswered question goes away a little after the other side stops waiting (the crate waits three minutes).
const QUESTION_LIFE: Duration = Duration::from_secs(185);
const WINDOW_W: f32 = 380.;
const WINDOW_H: f32 = 176.;

/// What the crate says, carried to the UI thread.
enum Ev {
    Peers(Vec<Peer>),
    Offer(Offer),
    Receiving(Offer),
    Progress(u64, u64, u64),
    Finished(u64, TransferResult),
    Code(String, String),
    Paired(String),
    Visibility(Visibility),
    Status(NetStatus),
    /// A click on one of our toasts: its argument (`ferry:yes:<key>`, `ferry:no:<key>`, `ferry:show:<key>`, `ferry:open:<path>`).
    Toast(String),
    Speed(String, SpeedResult),
}

struct Listener(mpsc::UnboundedSender<Ev>);

impl NearbyListener for Listener {
    fn peers_changed(&self, peers: Vec<Peer>) {
        let _ = self.0.unbounded_send(Ev::Peers(peers));
    }
    fn offer(&self, offer: Offer) {
        let _ = self.0.unbounded_send(Ev::Offer(offer));
    }
    fn receiving(&self, offer: Offer) {
        let _ = self.0.unbounded_send(Ev::Receiving(offer));
    }
    fn progress(&self, transfer: u64, done: u64, total: u64) {
        let _ = self.0.unbounded_send(Ev::Progress(transfer, done, total));
    }
    fn finished(&self, transfer: u64, result: TransferResult) {
        let _ = self.0.unbounded_send(Ev::Finished(transfer, result));
    }
    fn pairing_code(&self, peer: String, code: String) {
        let _ = self.0.unbounded_send(Ev::Code(peer, code));
    }
    fn paired(&self, peer: String, _ok: bool) {
        let _ = self.0.unbounded_send(Ev::Paired(peer));
    }
    fn speed_measured(&self, peer: String, result: SpeedResult) {
        let _ = self.0.unbounded_send(Ev::Speed(peer, result));
    }
    fn visibility_changed(&self, visibility: Visibility) {
        let _ = self.0.unbounded_send(Ev::Visibility(visibility));
    }
    fn status(&self, status: NetStatus) {
        let _ = self.0.unbounded_send(Ev::Status(status));
    }
}

/// A transfer's news for its Status Center card's worker thread.
enum Step {
    Progress(u64, u64),
    End(TransferResult),
}

/// Something the user is asked about.
#[derive(Clone)]
enum Question {
    Offer(Offer),
    Pair { peer: String, name: String, code: String },
}

impl Question {
    fn key(&self) -> String {
        match self {
            Question::Offer(o) => format!("offer-{}", o.id),
            Question::Pair { peer, .. } => format!("pair-{peer}"),
        }
    }
}

/// What it was started with: a change of any of these starts it anew (the crate takes them once).
#[derive(Clone, PartialEq)]
struct Started {
    name: String,
    inbox: String,
    auto_accept_trusted: bool,
}

#[derive(Default)]
pub struct Ferry {
    nearby: Option<Arc<Nearby>>,
    started: Option<Started>,
    peers: Vec<Peer>,
    /// Why discovery or the whole service could not start, for the Settings page.
    problem: Option<String>,
    transfers: HashMap<u64, std::sync::mpsc::Sender<Step>>,
    /// The transfers coming in, with the name of the device they come from (for the toast when they arrive).
    incoming: HashMap<u64, String>,
    questions: Vec<Question>,
    /// The questions asked in a toast (by key); the others are asked in the window.
    in_toast: std::collections::HashSet<String>,
    window: Option<WindowHandle<QuestionView>>,
    events: Option<mpsc::UnboundedSender<Ev>>,
    /// Settings > Ferry is on screen: see [`page_seen`].
    viewing: Option<Viewing>,
    /// The speed tests running now (by device), and the last result with each device in this run.
    measuring: std::collections::HashSet<String>,
    speeds: HashMap<String, SpeedResult>,
}

/// Settings > Ferry on screen, in `window`, last drawn at `seen`; `restore` is the visibility to go back to when it goes.
struct Viewing {
    window: AnyWindowHandle,
    seen: std::time::Instant,
    restore: Option<FerryVisibility>,
}

/// The user picked a visibility in the list: a choice that stays when the page goes.
static CHOSEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The page counts as gone when it was not drawn for this long (its window is redrawn every two seconds while it is open).
const PAGE_GONE: Duration = Duration::from_secs(5);

impl Global for Ferry {}

/// At start, in the main process only: Ferry follows the settings from here on ([`apply`], called by every settings change).
pub fn init(cx: &mut App) {
    let (tx, mut rx) = mpsc::unbounded::<Ev>();
    cx.set_global(Ferry { events: Some(tx), ..Default::default() });
    cx.spawn(async move |cx| {
        while let Some(ev) = rx.next().await {
            cx.update(|cx| on_event(ev, cx));
        }
    })
    .detach();
    // "everyone" never outlives a run: it is a ten-minute state
    if cx.global::<Settings>().ferry_visibility == FerryVisibility::Everyone {
        settings::update(cx, |s| s.ferry_visibility = FerryVisibility::Trusted);
    }
    apply(cx);
}

/// Brings the service in line with the settings: starts or stops it, starts it anew when its name or folder changed, sets who sees
/// this PC. Cheap when nothing changed.
pub fn apply(cx: &mut App) {
    if cx.try_global::<Ferry>().is_none() {
        return;
    }
    let s = cx.global::<Settings>();
    let want = s.ferry_enabled.then(|| Started { name: name_of(s), inbox: inbox_of(s).to_string_lossy().into_owned(), auto_accept_trusted: s.ferry_auto_accept_trusted });
    let vis = match s.ferry_visibility {
        FerryVisibility::Off => Visibility::Off,
        FerryVisibility::Trusted => Visibility::Trusted,
        FerryVisibility::Everyone => Visibility::Everyone,
    };
    if cx.global::<Ferry>().started != want {
        stop(cx);
        if let Some(w) = want {
            start(w, cx);
        }
        cx.refresh_windows();
    }
    if let Some(n) = cx.global::<Ferry>().nearby.clone()
        && n.visibility() != vis
    {
        n.set_visibility(vis);
    }
}

fn start(w: Started, cx: &mut App) {
    let Some(tx) = cx.global::<Ferry>().events.clone() else { return };
    let Some(dir) = settings::app_dir().map(|d| d.join("ferry")) else { return };
    let config = Config {
        name: w.name.clone(),
        os: "windows".into(),
        state_dir: dir.to_string_lossy().into_owned(),
        inbox: w.inbox.clone(),
        auto_accept_trusted: w.auto_accept_trusted,
        port: 0,
        discovery: true,
        everyone_minutes: EVERYONE_MINUTES,
        local_only: false,
    };
    let f = cx.global_mut::<Ferry>();
    f.started = Some(w);
    match Nearby::start(config, Arc::new(Listener(tx))) {
        Ok(n) => {
            f.nearby = Some(n);
            f.problem = None;
            watch_link(cx);
        }
        Err(e) => {
            log::warn!("ferry: cannot start: {e}");
            f.problem = Some(e.to_string());
        }
    }
}

fn stop(cx: &mut App) {
    let f = cx.global_mut::<Ferry>();
    f.started = None;
    f.peers.clear();
    f.problem = None;
    f.questions.clear();
    for key in f.in_toast.drain() {
        rf_win::toast_post::remove(crate::installer::APP_ID, &key);
    }
    f.incoming.clear();
    f.viewing = None;
    // (the cards' workers see their channel go and end as cancelled)
    f.transfers.clear();
    let window = f.window.take();
    if let Some(n) = f.nearby.take() {
        // stopping waits for the runtime: not on the UI thread
        std::thread::spawn(move || n.stop());
    }
    if let Some(w) = window {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
}

fn name_of(s: &Settings) -> String {
    let name = s.ferry_name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into())
}

fn inbox_of(s: &Settings) -> PathBuf {
    let custom = s.ferry_inbox.trim();
    if !custom.is_empty() {
        return PathBuf::from(custom);
    }
    dirs::download_dir().or_else(|| dirs::home_dir().map(|h| h.join("Downloads"))).unwrap_or_default()
}

/// Whether Ferry runs (the menu offers it only then).
pub(crate) fn running(cx: &App) -> bool {
    cx.try_global::<Ferry>().is_some_and(|f| f.nearby.is_some())
}

fn on_event(ev: Ev, cx: &mut App) {
    match ev {
        Ev::Peers(peers) => {
            cx.global_mut::<Ferry>().peers = peers;
            cx.refresh_windows();
        }
        Ev::Offer(offer) => ask(Question::Offer(offer), cx),
        Ev::Receiving(offer) => {
            let dest = inbox_of(cx.global::<Settings>());
            cx.global_mut::<Ferry>().incoming.insert(offer.transfer, offer.from_name.clone());
            incoming_started(&offer, cx);
            track(cx, offer.transfer, format!("{}: {}", t!("Ferry ile alınıyor"), offer.from_name), Some(dest));
        }
        Ev::Progress(transfer, done, total) => {
            if let Some(tx) = cx.global::<Ferry>().transfers.get(&transfer) {
                let _ = tx.send(Step::Progress(done, total));
            }
        }
        Ev::Finished(transfer, result) => {
            let from = cx.global_mut::<Ferry>().incoming.remove(&transfer);
            if let Some(from) = from
                && result.ok
            {
                arrived(transfer, from, result.paths.clone(), cx);
            }
            if let Some(tx) = cx.global_mut::<Ferry>().transfers.remove(&transfer) {
                let _ = tx.send(Step::End(result));
            }
        }
        Ev::Toast(arg) => on_toast(&arg, cx),
        Ev::Speed(peer, result) => {
            let f = cx.global_mut::<Ferry>();
            f.measuring.remove(&peer);
            f.speeds.insert(peer, result);
            cx.refresh_windows();
        }
        Ev::Code(peer, code) => {
            let f = cx.global::<Ferry>();
            let name = f.peers.iter().find(|p| p.id == peer).map(|p| p.name.clone()).unwrap_or_else(|| peer.clone());
            ask(Question::Pair { peer, name, code }, cx);
        }
        Ev::Paired(peer) => {
            forget_question(&Question::Pair { peer, name: String::new(), code: String::new() }.key(), cx);
            cx.refresh_windows();
        }
        Ev::Visibility(v) => {
            // (the ten-minute fall-back is not undone when the page goes)
            if let Some(view) = cx.global_mut::<Ferry>().viewing.as_mut() {
                view.restore = None;
            }
            let vis = match v {
                Visibility::Off => FerryVisibility::Off,
                Visibility::Trusted => FerryVisibility::Trusted,
                Visibility::Everyone => FerryVisibility::Everyone,
            };
            settings::update(cx, |s| s.ferry_visibility = vis);
        }
        Ev::Status(status) => {
            cx.global_mut::<Ferry>().problem = match status {
                NetStatus::Ready => None,
                NetStatus::DiscoveryUnavailable { reason } => Some(reason),
            };
            cx.refresh_windows();
        }
    }
}

/// A Status Center card for a transfer: its progress, its end, and its cancel button stopping the transfer.
fn track(cx: &mut App, transfer: u64, title: String, dest: Option<PathBuf>) {
    let Some(nearby) = cx.global::<Ferry>().nearby.clone() else { return };
    let (tx, rx) = std::sync::mpsc::channel::<Step>();
    cx.global_mut::<Ferry>().transfers.insert(transfer, tx);
    let id = crate::status_center::start_with_id(
        cx,
        title,
        move |tracker| {
            let mut cancelled = false;
            loop {
                match rx.recv_timeout(Duration::from_millis(150)) {
                    Ok(Step::Progress(done, total)) => {
                        tracker.bytes.set(done, total);
                        let part = if total == 0 { 0 } else { (done.min(total) * 1000 / total) as u32 };
                        (tracker.on_progress)(1000, part, None);
                    }
                    Ok(Step::End(r)) if r.ok => return Ok(true),
                    Ok(Step::End(r)) if r.cancelled || cancelled => return Ok(false),
                    Ok(Step::End(r)) => return Err(anyhow::anyhow!("{}", failure_text(r.error.as_deref()))),
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if !cancelled && tracker.cancel.load(Ordering::Relaxed) {
                            cancelled = true;
                            nearby.cancel(transfer);
                        }
                    }
                    // (Ferry stopped meanwhile)
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(false),
                }
            }
        },
        None,
    );
    if dest.is_some() {
        crate::status_center::set_extras(cx, id, None, dest, None);
    }
}

/// The crate's reasons are English sentences for logs: the ones a user meets get words of the interface.
fn failure_text(error: Option<&str>) -> String {
    let e = error.unwrap_or_default();
    let has = |s: &str| e.contains(s);
    if has("only takes files from its trusted devices") {
        t!("O cihaz yalnızca güvenilir cihazlarından dosya alıyor. Önce eşleştirin.").to_string()
    } else if has("declined") {
        t!("Karşı taraf reddetti.").to_string()
    } else if has("did not answer") || has("no answer") || has("timed out") {
        t!("Cihaz yanıt vermedi.").to_string()
    } else if has("busy with another speed test") {
        t!("O cihazla az önce hız ölçüldü; birkaç saniye sonra yeniden deneyin.").to_string()
    } else if has("busy with other offers") {
        t!("O cihazda yanıt bekleyen bir teklifiniz zaten var.").to_string()
    } else if has("arrived damaged") {
        t!("Bir dosya bozuk geldi. Yeniden gönderin: kalan kısım kaldığı yerden gelir.").to_string()
    } else if e.is_empty() {
        t!("Aktarım yarıda kaldı.").to_string()
    } else {
        e.to_string()
    }
}

// ------------------------------------------------------------------------------------------------------------------ questions

/// Asks: in a toast with the answers as its buttons (the installed app), else in the window.
fn ask(q: Question, cx: &mut App) {
    let key = q.key();
    let toasted = toast_question(&q, cx);
    let f = cx.global_mut::<Ferry>();
    f.questions.retain(|x| x.key() != key);
    f.questions.push(q);
    if toasted {
        f.in_toast.insert(key.clone());
    }
    sync_window(cx);
    // the other side stops waiting after three minutes: the question goes with it
    cx.spawn(async move |cx| {
        cx.background_executor().timer(QUESTION_LIFE).await;
        cx.update(|cx| forget_question(&key, cx));
    })
    .detach();
}

fn answer(q: &Question, yes: bool, cx: &mut App) {
    forget_question(&q.key(), cx);
    let Some(n) = cx.global::<Ferry>().nearby.clone() else { return };
    match q {
        Question::Offer(o) if yes => {
            let dest = inbox_of(cx.global::<Settings>());
            n.accept(o.id, dest.to_string_lossy().into_owned());
            cx.global_mut::<Ferry>().incoming.insert(o.transfer, o.from_name.clone());
            track(cx, o.transfer, format!("{}: {}", t!("Ferry ile alınıyor"), o.from_name), Some(dest));
        }
        Question::Offer(o) => n.reject(o.id),
        Question::Pair { peer, .. } => n.confirm_pairing(peer.clone(), yes),
    }
}

/// A question is no longer asked (answered, given up, or out of time): out of the window, and its toast taken away.
fn forget_question(key: &str, cx: &mut App) {
    if !cx.has_global::<Ferry>() {
        return;
    }
    let f = cx.global_mut::<Ferry>();
    f.questions.retain(|x| x.key() != key);
    if f.in_toast.remove(key) {
        rf_win::toast_post::remove(crate::installer::APP_ID, key);
    }
    sync_window(cx);
}

/// What a question says: its title, its lines, a pairing's code, and its two answers.
fn texts(q: &Question) -> (String, Vec<String>, Option<String>, &'static str, &'static str) {
    match q {
        Question::Offer(o) => {
            // "photo.jpg", or "photo.jpg and 3 items" (the top-level names: a folder counts once)
            let what = match o.names.len() {
                0 => crate::i18n::items(o.file_count as usize),
                1 => o.names[0].clone(),
                n => format!("{} {} {}", o.names[0], t!("ve"), crate::i18n::items(n - 1)),
            };
            let mut lines = vec![format!("{} · {}", what, rf_core::format::size(o.total_bytes))];
            // (a device that was never paired can call itself anything)
            if !o.trusted {
                lines.push(t!("Eşleşmemiş cihaz: adı doğrulanamaz").to_string());
            }
            (format!("{} {}", o.from_name, t!("dosya göndermek istiyor")), lines, None, t!("Reddet"), t!("Kabul et"))
        }
        Question::Pair { name, code, .. } => (
            format!("{}: {name}", t!("Eşleştir")),
            vec![t!("İki cihazda da aynı kodun göründüğünden emin olun. Eşleşen cihazlar birbirine sormadan dosya gönderebilir.").to_string()],
            Some(format!("{} {}", &code[..3.min(code.len())], &code[3.min(code.len())..])),
            t!("İptal"),
            t!("Eşleştir"),
        ),
    }
}

/// Windows shows our toasts only for the installed app: its Start menu shortcut carries the AppUserModelID. A build from the source
/// tree or the portable zip has none, and a toast would vanish without a word: those ask in the window.
fn toasts_usable() -> bool {
    dirs::data_dir().is_some_and(|d| d.join(r"Microsoft\Windows\Start Menu\Programs\rustyfiles.lnk").exists())
}

fn toast_question(q: &Question, cx: &App) -> bool {
    if !toasts_usable() {
        return false;
    }
    let Some(tx) = cx.global::<Ferry>().events.clone() else { return false };
    let key = q.key();
    let (title, lines, code, no, yes) = texts(q);
    let mut all: Vec<String> = code.map(|c| format!("{} {c}", t!("Kod:"))).into_iter().collect();
    all.extend(lines);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    let buttons = [
        rf_win::toast_post::Button { label: no.to_string(), argument: format!("ferry:no:{key}") },
        rf_win::toast_post::Button { label: yes.to_string(), argument: format!("ferry:yes:{key}") },
    ];
    let toast = rf_win::toast_post::Toast { tag: &key, title: &title, lines: &refs, launch: &format!("ferry:show:{key}"), buttons: &buttons, stays: true };
    match rf_win::toast_post::show(crate::installer::APP_ID, &toast, move |arg| {
        let _ = tx.unbounded_send(Ev::Toast(arg));
    }) {
        Ok(()) => true,
        Err(e) => {
            log::info!("ferry: no toast ({e}); asking in the window");
            false
        }
    }
}

fn on_toast(arg: &str, cx: &mut App) {
    let mut parts = arg.splitn(3, ':');
    let (Some("ferry"), Some(kind), Some(rest)) = (parts.next(), parts.next(), parts.next()) else { return };
    match kind {
        "yes" | "no" => {
            let q = cx.global::<Ferry>().questions.iter().find(|q| q.key() == rest).cloned();
            if let Some(q) = q {
                answer(&q, kind == "yes", cx);
            }
        }
        // a click on the question's toast itself: it goes away, so the question moves to the window
        "show" => {
            cx.global_mut::<Ferry>().in_toast.remove(rest);
            sync_window(cx);
        }
        "open" => {
            crate::workspace::route_ipc_shown(format!("{}{rest}", crate::workspace::SELECT_PREFIX), cx);
        }
        _ => {}
    }
}

/// A trusted device's files started coming in without a question: said in a toast, with Windows' notification sound, so the user knows
/// (the user asked for it). Without toasts, the sound alone.
fn incoming_started(offer: &Offer, cx: &mut App) {
    let what = match offer.names.len() {
        0 => crate::i18n::items(offer.file_count as usize),
        1 => offer.names[0].clone(),
        n => format!("{} {} {}", offer.names[0], t!("ve"), crate::i18n::items(n - 1)),
    };
    let line = format!("{what} · {}", rf_core::format::size(offer.total_bytes));
    let shown = toasts_usable()
        && cx.global::<Ferry>().events.clone().is_some_and(|tx| {
            let title = format!("{} {}", offer.from_name, t!("dosya gönderiyor"));
            let lines = [line.as_str()];
            let toast = rf_win::toast_post::Toast { tag: &format!("in-{}", offer.transfer), title: &title, lines: &lines, launch: "ferry:none:", buttons: &[], stays: false };
            rf_win::toast_post::show(crate::installer::APP_ID, &toast, move |arg| {
                let _ = tx.unbounded_send(Ev::Toast(arg));
            })
            .is_ok()
        });
    if !shown {
        rf_win::sound::notify();
    }
}

/// Files came in: each is marked as come from another computer (SmartScreen asks before a program among them runs), and the installed
/// app says so in a toast that opens them.
fn arrived(transfer: u64, from: String, paths: Vec<String>, cx: &mut App) {
    let referrer = format!("ferry://{from}");
    let marked = paths.clone();
    std::thread::spawn(move || {
        for p in &marked {
            mark_tree(std::path::Path::new(p), &referrer);
        }
    });
    let Some(first) = paths.first().cloned() else { return };
    if !toasts_usable() {
        return;
    }
    let Some(tx) = cx.global::<Ferry>().events.clone() else { return };
    let name = std::path::Path::new(&first).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let what = if paths.len() == 1 { name } else { format!("{name} {} {}", t!("ve"), crate::i18n::items(paths.len() - 1)) };
    let title = format!("{from} {}", t!("dosya gönderdi"));
    let lines = [what.as_str(), t!("Görmek için tıklayın")];
    let toast = rf_win::toast_post::Toast { tag: &format!("got-{transfer}"), title: &title, lines: &lines, launch: &format!("ferry:open:{first}"), buttons: &[], stays: false };
    if let Err(e) = rf_win::toast_post::show(crate::installer::APP_ID, &toast, move |arg| {
        let _ = tx.unbounded_send(Ev::Toast(arg));
    }) {
        log::info!("ferry: no toast for an arrival: {e}");
    }
}

/// Marks a received file, or every file in a received folder (links are not followed).
fn mark_tree(path: &std::path::Path, from: &str) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if meta.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                mark_tree(&e.path(), from);
            }
        }
    } else if meta.is_file()
        && let Err(e) = rf_win::fileinfo::mark_downloaded(path, from)
    {
        log::info!("ferry: cannot mark {}: {e}", path.display());
    }
}

/// The question window: open while a question is asked in it (not in a toast), closed when none is, redrawn when they change.
fn sync_window(cx: &mut App) {
    let Some(f) = cx.try_global::<Ferry>() else { return };
    let empty = f.questions.iter().all(|q| f.in_toast.contains(&q.key()));
    let window = f.window;
    match (empty, window) {
        (true, Some(w)) => {
            cx.global_mut::<Ferry>().window = None;
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
        (false, Some(w)) => {
            if w.update(cx, |_, _, cx| cx.notify()).is_err() {
                cx.global_mut::<Ferry>().window = None;
                open_window(cx);
            }
        }
        (false, None) => open_window(cx),
        (true, None) => {}
    }
}

/// At the bottom right of the main screen, over the other windows, without taking the keyboard from the app in front.
fn open_window(cx: &mut App) {
    let Some(display) = cx.primary_display() else { return };
    let area = display.visible_bounds();
    let origin = point(area.origin.x + area.size.width - px(WINDOW_W + 16.), area.origin.y + area.size.height - px(WINDOW_H + 16.));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::new(origin, size(px(WINDOW_W), px(WINDOW_H))))),
        display_id: Some(display.id()),
        titlebar: None,
        focus: false,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        // the app's own popup look: on HyperGlass when that is the backdrop (as the quick search), else Windows' acrylic
        window_background: crate::theme::popup_material(glassy(cx)),
        ..Default::default()
    };
    let glass = glassy(cx);
    match cx.open_window(opts, move |window, cx| {
        let hwnd = crate::look::window::hwnd_of(window);
        rf_win::window::set_dark_mode(hwnd, Palette::for_window(window).dark);
        cx.new(|cx| QuestionView::new(glass, window, cx))
    }) {
        Ok(w) => cx.global_mut::<Ferry>().window = Some(w),
        Err(e) => log::warn!("ferry: question window: {e:#}"),
    }
}

/// HyperGlass is the app's backdrop (and this Windows has the materials it is drawn with).
fn glassy(cx: &App) -> bool {
    crate::settings::materials_available() && cx.try_global::<Settings>().is_some_and(|s| s.backdrop_in_use() == crate::settings::BackdropPref::Glass)
}

struct QuestionView {
    glass: Option<crate::glass_watch::PopupGlass>,
    _glass_watch: Option<Task<()>>,
}

impl QuestionView {
    fn new(glassy: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let glass = glassy.then(|| crate::glass_watch::PopupGlass::start_panel(crate::look::window::hwnd_of(window), Palette::for_window(window).dark));
        let watch = glass.is_some().then(|| crate::glass_watch::watch(cx, |this: &mut Self| this.glass.as_mut()));
        QuestionView { glass, _glass_watch: watch }
    }
}

impl Render for QuestionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        // (the questions asked in a toast are not asked here too)
        let questions: Vec<Question> = cx
            .try_global::<Ferry>()
            .map(|f| f.questions.iter().filter(|q| !f.in_toast.contains(&q.key())).cloned().collect())
            .unwrap_or_default();
        let Some(q) = questions.first().cloned() else { return div().into_any_element() };
        let waiting = questions.len() - 1;
        let (title, lines, code, no, yes) = texts(&q);
        let (qn, qy) = (q.clone(), q);
        // over glass a wash, or the plain fill until the glass runs (never a see-through window with nothing behind it)
        let fill = self.glass.as_ref().map(|g| crate::glass_watch::panel_fill(p.dark, g.fallback())).or_else(|| crate::theme::popup_fill(&p));
        let content = v_flex()
            .w_full()
            .p_4()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(L::Send).size(px(16.)).text_color(p.accent))
                    .child(div().text_xs().text_color(p.text_secondary).child("Ferry"))
                    .children((waiting > 0).then(|| div().ml_auto().text_xs().text_color(p.text_secondary).child(format!("+{waiting}")))),
            )
            .child(div().mt_1().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(title))
            .children(lines.into_iter().map(|line| div().text_xs().text_color(p.text_secondary).whitespace_normal().child(line)))
            .children(code.map(|c| div().mt_1().text_2xl().font_weight(FontWeight::BOLD).child(c)))
            .child(
                h_flex()
                    .mt_3()
                    .gap_2()
                    .justify_end()
                    .child(crate::settings_view::button("ferry-no", &p, no, move |_, cx| answer(&qn, false, cx)))
                    .child(primary("ferry-yes", &p, yes, move |_, cx| answer(&qy, true, cx))),
            );
        // the window is as tall as what it says, growing upwards (its bottom stays above the taskbar)
        let vh = window.viewport_size().height;
        div()
            .size_full()
            .when_some(fill, |d, f| d.bg(f))
            .text_color(p.text)
            .child(
                div().relative().w_full().child(content).child(
                    canvas(
                        move |b, window, _| {
                            let h = b.size.height.ceil();
                            if (f32::from(h) - f32::from(vh)).abs() > 1. {
                                window.on_next_frame(move |w, _| {
                                    let hwnd = crate::look::window::hwnd_of(w);
                                    let new_h = (f32::from(h) * w.scale_factor()).round() as i32;
                                    if let Some((r, _)) = rf_win::window::rect(hwnd) {
                                        rf_win::window::place_quietly(hwnd, [r[0], r[3] - new_h, r[2], r[3]]);
                                    }
                                });
                            }
                        },
                        |_, _, _, _| {},
                    )
                    .absolute()
                    .inset_0(),
                ),
            )
            .into_any_element()
    }
}

fn primary(id: &'static str, p: &Palette, label: &str, run: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .id(id)
        .h(px(32.))
        .px_4()
        .flex()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .bg(p.accent)
        .text_color(p.accent_text)
        .text_sm()
        .cursor_pointer()
        .tab_index(0)
        .hover(|s| s.opacity(0.9))
        .active(|s| s.opacity(0.8))
        .child(label.to_string())
        .on_click(move |_, window, cx| run(window, cx))
        .into_any_element()
}

// ------------------------------------------------------------------------------------------------------------------ sending

/// The "Send with Ferry" submenu for `paths`: the devices seen now (the trusted ones first), and the way to the settings.
pub(crate) fn menu_items(paths: &[PathBuf], cx: &App) -> Vec<Item> {
    let peers = cx.try_global::<Ferry>().map(|f| f.peers.clone()).unwrap_or_default();
    let mut v: Vec<Item> = peers
        .into_iter()
        .map(|peer| {
            let paths = paths.to_vec();
            let icon = MenuIcon::Lucide(if peer.trusted { L::ShieldCheck } else { L::Monitor });
            Item::action(peer.name.clone(), icon, move |_, cx| send(&peer, paths.clone(), cx))
        })
        .collect();
    if v.is_empty() {
        v.push(Item::action(t!("Yakında cihaz yok"), MenuIcon::Lucide(L::Wifi), |_, _| {}).enabled(false));
    }
    v.push(Item::Separator);
    v.push(Item::action(t!("Ferry ayarları…"), MenuIcon::Lucide(L::Settings), |_, cx| open_settings(cx)));
    v
}

pub(crate) fn open_settings(cx: &mut App) {
    crate::settings_view::open_page(crate::settings_view::Page::Ferry, cx);
    crate::workspace::show_settings(cx);
}

fn send(peer: &Peer, paths: Vec<PathBuf>, cx: &mut App) {
    let Some(n) = cx.global::<Ferry>().nearby.clone() else { return };
    match n.send(peer.id.clone(), paths.iter().map(|p| p.to_string_lossy().into_owned()).collect()) {
        Ok(transfer) => track(cx, transfer, format!("{}: {}", t!("Ferry ile gönderiliyor"), peer.name), None),
        Err(e) => log::warn!("ferry: cannot send to {}: {e}", peer.name),
    }
}

// ------------------------------------------------------------------------------------------------------------------ settings

/// Settings > Ferry.
pub(crate) fn page(p: &Palette, s: &Settings, cx: &mut App) -> Vec<AnyElement> {
    use crate::settings_view::{button, card, dropdown, section, switch};
    let on = s.ferry_enabled;
    let mut v = vec![card(
        p,
        L::Send,
        t!("Ferry'yi kullan"),
        Some(t!("Aynı ağdaki rustyfiles'lar birbirini bulur; dosyalar şifreli olarak doğrudan cihazdan cihaza gider, sunucu ya da hesap yok. Açınca Windows Güvenlik Duvarı rustyfiles'ın ağı kullanmasına izin vermenizi isteyebilir: özel ağlar için izin verin.")),
        switch("ferry-on", p, on, move |_, cx| settings::update(cx, |s| s.ferry_enabled = !on)),
    )];
    if !on {
        return v;
    }
    let (nearby, peers, problem) = cx.try_global::<Ferry>().map(|f| (f.nearby.clone(), f.peers.clone(), f.problem.clone())).unwrap_or_default();
    if let Some(problem) = problem {
        v.push(card(p, L::TriangleAlert, t!("Yakındaki cihazlar aranamıyor"), Some(problem.as_str()), div().into_any_element()));
    }
    v.push(card(
        p,
        L::Eye,
        t!("Bu bilgisayarı kimler görebilir"),
        Some(t!("Herkes seçeneği 10 dakika sürer, sonra yalnız güvenilir cihazlara döner. Eşleşmemiş bir cihazdan dosya almak ya da onunla eşleşmek için Herkes seçili olmalı.")),
        dropdown(
            "ferry-vis",
            p,
            s.ferry_visibility,
            vec![
                (FerryVisibility::Off, t!("Hiç kimse").to_string()),
                (FerryVisibility::Trusted, t!("Yalnız güvenilir cihazlar").to_string()),
                (FerryVisibility::Everyone, t!("Herkes (10 dakika)").to_string()),
            ],
            |s, v| {
                s.ferry_visibility = v;
                CHOSEN.store(true, Ordering::Relaxed);
            },
        ),
    ));
    let name = name_of(s);
    v.push(card(
        p,
        L::Pencil,
        t!("Cihaz adı"),
        Some(name.as_str()),
        button("ferry-name", p, t!("Değiştir…"), move |window, cx| {
            let ok = crate::prompt::Button::new(t!("Kaydet"), crate::prompt::Kind::Primary, |text, _, cx| {
                let text = text.unwrap_or_default().trim().to_string();
                settings::update(cx, |s| s.ferry_name = text);
                Ok(())
            });
            let current = cx.global::<Settings>().ferry_name.clone();
            crate::prompt::show(
                t!("Cihaz adı"),
                Some(t!("Diğer cihazlar bu bilgisayarı bu adla görür. Boş bırakırsanız bilgisayarın adı kullanılır.").to_string()),
                Some((current, std::env::var("COMPUTERNAME").unwrap_or_default())),
                vec![ok, crate::prompt::Button::cancel()],
                window,
                cx,
            );
        }),
    ));
    let inbox = inbox_of(s).to_string_lossy().into_owned();
    let custom = !s.ferry_inbox.trim().is_empty();
    v.push(card(
        p,
        L::FolderDown,
        t!("Gelen dosyalar"),
        Some(inbox.as_str()),
        h_flex()
            .gap_2()
            .children(custom.then(|| button("ferry-inbox-reset", p, t!("Varsayılan"), |_, cx| settings::update(cx, |s| s.ferry_inbox.clear()))))
            .child(button("ferry-inbox", p, t!("Değiştir…"), |_, cx| {
                let (tx, rx) = futures::channel::oneshot::channel::<Option<PathBuf>>();
                let title = t!("Gelen dosyaların klasörünü seçin");
                rf_win::sta::run(move |owner| {
                    let _ = tx.send(rf_win::dialogs::pick(owner, title, rf_win::dialogs::Pick::Folder));
                });
                cx.spawn(async move |cx| {
                    if let Ok(Some(path)) = rx.await {
                        let _ = cx.update(|cx| settings::update(cx, |s| s.ferry_inbox = path.to_string_lossy().into_owned()));
                    }
                })
                .detach();
            }))
            .into_any_element(),
    ));
    let auto = s.ferry_auto_accept_trusted;
    v.push(card(
        p,
        L::ShieldCheck,
        t!("Güvenilir cihazlardan sormadan al"),
        Some(t!("Eşleştirdiğiniz cihazların gönderdikleri doğrudan Gelen dosyalar klasörüne iner; diğer cihazlar her seferinde sorulur.")),
        switch("ferry-auto", p, auto, move |_, cx| settings::update(cx, |s| s.ferry_auto_accept_trusted = !auto)),
    ));

    v.push(section(p, t!("Yakındaki cihazlar")));
    let everyone = s.ferry_visibility == FerryVisibility::Everyone;
    v.push(card(
        p,
        L::RefreshCw,
        t!("Cihazları yeniden ara"),
        Some(t!("Ağdaki cihazlar kendiliğinden bulunur; yeni açılan bir cihaz görünmüyorsa yeniden arayın.")),
        h_flex()
            .gap_2()
            .children((!everyone).then(|| button("ferry-everyone", p, t!("10 dakika herkese görün"), |_, cx| settings::update(cx, |s| s.ferry_visibility = FerryVisibility::Everyone))))
            .child(button("ferry-refresh", p, t!("Yenile"), |_, cx| {
                if let Some(n) = cx.global::<Ferry>().nearby.clone() {
                    n.refresh();
                }
            }))
            .into_any_element(),
    ));
    if peers.is_empty() {
        v.push(card(
            p,
            L::Wifi,
            t!("Henüz cihaz yok"),
            Some(t!("Diğer cihazda rustyfiles açık ve Ferry'si açık olmalı. İki cihaz henüz eşleşmediyse en az birinin görünürlüğü Herkes olmalı.")),
            div().into_any_element(),
        ));
    }
    for (i, peer) in peers.iter().enumerate() {
        let (measuring, last) = cx.try_global::<Ferry>().map(|f| (f.measuring.contains(&peer.id), f.speeds.get(&peer.id).cloned())).unwrap_or_default();
        let mut desc = os_name(&peer.os).to_string();
        if peer.trusted {
            desc = format!("{desc} · {}", t!("Güvenilir"));
        }
        if let Some(speed) = speed_text(peer, measuring, last.as_ref()) {
            desc = format!("{desc} · {speed}");
        }
        let id = peer.id.clone();
        let control = h_flex()
            .gap_2()
            .child(button(("ferry-speed", i), p, if measuring { t!("Ölçülüyor…") } else { t!("Hızı ölç") }, move |_, cx| measure(id.clone(), cx)))
            .children((!peer.trusted).then(|| {
                let id = peer.id.clone();
                button(("ferry-pair", i), p, t!("Eşleştir"), move |_, cx| pair(id.clone(), cx))
            }))
            .into_any_element();
        v.push(card(p, if peer.trusted { L::ShieldCheck } else { L::Monitor }, &peer.name, Some(desc.as_str()), control));
    }

    let trusted = nearby.map(|n| n.trusted()).unwrap_or_default();
    if !trusted.is_empty() {
        v.push(section(p, t!("Güvenilir cihazlar")));
        for (i, peer) in trusted.iter().enumerate() {
            let id = peer.id.clone();
            v.push(card(
                p,
                L::ShieldCheck,
                &peer.name,
                None,
                button(("ferry-forget", i), p, t!("Unut"), move |_, cx| {
                    if let Some(n) = cx.global::<Ferry>().nearby.clone()
                        && let Err(e) = n.forget(id.clone())
                    {
                        log::warn!("ferry: cannot forget a device: {e}");
                    }
                    cx.refresh_windows();
                }),
            ));
        }
    }
    v
}

/// Settings > Ferry was drawn in `window`. While it is on screen this PC is seen by everyone, the way a Mac is while Finder's AirDrop
/// window is open (the user asked for both platforms): the first drawing remembers the visibility and sets Everyone, and when the
/// page has not been drawn for [`PAGE_GONE`] (left, another tab, the window closed or minimized) the remembered value comes back, if
/// it is still Everyone. A choice in the list, or the ten-minute fall-back, is kept instead.
pub(crate) fn page_seen(window: AnyWindowHandle, cx: &mut App) {
    if !cx.has_global::<Ferry>() || cx.global::<Ferry>().nearby.is_none() {
        return;
    }
    let vis = cx.global::<Settings>().ferry_visibility;
    let f = cx.global_mut::<Ferry>();
    if let Some(view) = f.viewing.as_mut() {
        view.seen = std::time::Instant::now();
        view.window = window;
        if CHOSEN.swap(false, Ordering::Relaxed) {
            view.restore = None;
        }
        return;
    }
    CHOSEN.store(false, Ordering::Relaxed);
    let restore = (vis != FerryVisibility::Everyone).then_some(vis);
    f.viewing = Some(Viewing { window, seen: std::time::Instant::now(), restore });
    if restore.is_some() {
        // (not while the page is drawn)
        cx.defer(|cx| settings::update(cx, |s| s.ferry_visibility = FerryVisibility::Everyone));
    }
    // the heartbeat: the page's window is redrawn now and then, and the page that is no longer drawn is gone
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let over = cx.update(|cx| {
                let Some(view) = cx.has_global::<Ferry>().then(|| cx.global::<Ferry>().viewing.as_ref().map(|v| (v.window, v.seen))).flatten() else {
                    return true;
                };
                let minimized = view.0.update(cx, |_, w, _| rf_win::window::is_minimized(crate::look::window::hwnd_of(w))).unwrap_or(true);
                if minimized || view.1.elapsed() > PAGE_GONE {
                    page_gone(cx);
                    return true;
                }
                let _ = view.0.update(cx, |_, w, _| w.refresh());
                false
            });
            if over {
                break;
            }
        }
    })
    .detach();
}

fn page_gone(cx: &mut App) {
    let Some(view) = cx.global_mut::<Ferry>().viewing.take() else { return };
    let chosen = CHOSEN.swap(false, Ordering::Relaxed);
    if let Some(back) = view.restore
        && !chosen
        && cx.global::<Settings>().ferry_visibility == FerryVisibility::Everyone
    {
        settings::update(cx, |s| s.ferry_visibility = back);
    }
}

fn os_name(os: &str) -> &str {
    match os {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

/// The speed part of a device's line: the test running, its result, or the estimate (measured or from a transfer: "~48 MB/s"; from
/// the two links: "≈ 52 MB/s (Wi-Fi)"), else the round trip.
fn speed_text(peer: &Peer, measuring: bool, last: Option<&SpeedResult>) -> Option<String> {
    let per_s = |bps: u64| format!("{}/s", rf_core::format::size(bps));
    if measuring {
        return Some(t!("hız ölçülüyor…").to_string());
    }
    match last {
        Some(r) if r.ok => return Some(format!("↑ {} · ↓ {} · {} ms", per_s(r.up_bps), per_s(r.down_bps), r.rtt_ms)),
        Some(r) => return Some(format!("{}: {}", t!("Ölçülemedi"), failure_text(r.error.as_deref()))),
        None => {}
    }
    match peer.estimate_source {
        EstimateSource::Measured | EstimateSource::Transfer => Some(format!("~{}", per_s(peer.estimate_bps))),
        EstimateSource::Link => {
            let link = match peer.link_kind {
                LinkKind::Wifi => "Wi-Fi",
                LinkKind::Ethernet => "Ethernet",
                _ => t!("ağ"),
            };
            Some(format!("≈ {} ({link})", per_s(peer.estimate_bps)))
        }
        EstimateSource::Unknown => (peer.rtt_ms > 0).then(|| format!("{} ms", peer.rtt_ms)),
    }
}

fn measure(peer_id: String, cx: &mut App) {
    let Some(n) = cx.global::<Ferry>().nearby.clone() else { return };
    if cx.global::<Ferry>().measuring.contains(&peer_id) {
        return;
    }
    match n.measure_speed(peer_id.clone()) {
        Ok(_) => {
            let f = cx.global_mut::<Ferry>();
            f.speeds.remove(&peer_id);
            f.measuring.insert(peer_id);
            cx.refresh_windows();
        }
        Err(e) => log::warn!("ferry: cannot measure: {e}"),
    }
}

/// Tells the crate this PC's network link and keeps telling it while Ferry runs (a Wi-Fi rate drifts; the crate announces a real
/// change only). Read every 30 seconds, off the UI thread: one routing-table look and one interface row.
fn watch_link(cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            let link = cx.background_executor().spawn(async { rf_win::network::lan_link() }).await;
            let running = cx.update(|cx| {
                let Some(n) = cx.has_global::<Ferry>().then(|| cx.global::<Ferry>().nearby.clone()).flatten() else { return false };
                let (kind, mbps) = match link {
                    Some((rf_win::network::LinkType::Wifi, m)) => (LinkKind::Wifi, m),
                    Some((rf_win::network::LinkType::Ethernet, m)) => (LinkKind::Ethernet, m),
                    Some((rf_win::network::LinkType::Other, m)) => (LinkKind::Other, m),
                    None => (LinkKind::Unknown, 0),
                };
                n.set_link(kind, mbps);
                true
            });
            if !running {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(30)).await;
        }
    })
    .detach();
}

fn pair(peer_id: String, cx: &mut App) {
    let Some(n) = cx.global::<Ferry>().nearby.clone() else { return };
    if let Err(e) = n.pair(peer_id) {
        log::warn!("ferry: cannot pair: {e}");
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's own `test` attribute would come in with it)
    use super::{Question, failure_text, texts};
    use rf_nearby::Offer;

    fn offer(trusted: bool, names: &[&str]) -> Offer {
        Offer {
            id: 7,
            transfer: 8,
            from_id: "abc".into(),
            from_name: "Mac".into(),
            from_os: "macos".into(),
            trusted,
            names: names.iter().map(|n| n.to_string()).collect(),
            file_count: 3,
            total_bytes: 2048,
        }
    }

    #[test]
    fn an_unpaired_device_is_said_to_be_unverified() {
        let (_, lines, code, _, _) = texts(&Question::Offer(offer(false, &["photo.jpg"])));
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].starts_with("photo.jpg"));
        assert!(code.is_none());
        let (_, lines, _, _, _) = texts(&Question::Offer(offer(true, &["photo.jpg", "docs"])));
        assert_eq!(lines.len(), 1, "a paired device needs no warning: {lines:?}");
        assert!(lines[0].starts_with("photo.jpg "), "{lines:?}");
    }

    #[test]
    fn a_pairing_shows_its_code_in_two_halves() {
        let q = Question::Pair { peer: "p".into(), name: "Mac".into(), code: "012345".into() };
        let (_, _, code, _, _) = texts(&q);
        assert_eq!(code.as_deref(), Some("012 345"));
        assert_eq!(q.key(), "pair-p");
    }

    #[test]
    fn crate_reasons_get_interface_words() {
        for reason in ["the device is busy with other offers", "the device only takes files from its trusted devices", "declined", "x did not answer"] {
            assert_ne!(failure_text(Some(reason)), reason, "{reason}");
        }
        assert_eq!(failure_text(Some("something new")), "something new");
    }
}
