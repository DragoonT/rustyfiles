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
use gpui_kit::*;
use rf_nearby::{Config, NearbyListener, NetStatus, Nearby, Offer, Peer, TransferResult, Visibility};

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
    questions: Vec<Question>,
    window: Option<WindowHandle<QuestionView>>,
    events: Option<mpsc::UnboundedSender<Ev>>,
}

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
            track(cx, offer.transfer, format!("{}: {}", t!("Ferry ile alınıyor"), offer.from_name), Some(dest));
        }
        Ev::Progress(transfer, done, total) => {
            if let Some(tx) = cx.global::<Ferry>().transfers.get(&transfer) {
                let _ = tx.send(Step::Progress(done, total));
            }
        }
        Ev::Finished(transfer, result) => {
            if let Some(tx) = cx.global_mut::<Ferry>().transfers.remove(&transfer) {
                let _ = tx.send(Step::End(result));
            }
        }
        Ev::Code(peer, code) => {
            let f = cx.global::<Ferry>();
            let name = f.peers.iter().find(|p| p.id == peer).map(|p| p.name.clone()).unwrap_or_else(|| peer.clone());
            ask(Question::Pair { peer, name, code }, cx);
        }
        Ev::Paired(peer) => {
            cx.global_mut::<Ferry>().questions.retain(|q| !matches!(q, Question::Pair { peer: p, .. } if *p == peer));
            sync_window(cx);
            cx.refresh_windows();
        }
        Ev::Visibility(v) => {
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
    } else if has("arrived damaged") {
        t!("Bir dosya bozuk geldi. Yeniden gönderin: kalan kısım kaldığı yerden gelir.").to_string()
    } else if e.is_empty() {
        t!("Aktarım yarıda kaldı.").to_string()
    } else {
        e.to_string()
    }
}

// ------------------------------------------------------------------------------------------------------------------ questions

fn ask(q: Question, cx: &mut App) {
    let key = q.key();
    let f = cx.global_mut::<Ferry>();
    f.questions.retain(|x| x.key() != key);
    f.questions.push(q);
    sync_window(cx);
    // the other side stops waiting after three minutes: the question goes with it
    cx.spawn(async move |cx| {
        cx.background_executor().timer(QUESTION_LIFE).await;
        let _ = cx.update(|cx| {
            if cx.has_global::<Ferry>() {
                cx.global_mut::<Ferry>().questions.retain(|x| x.key() != key);
            }
            sync_window(cx);
        });
    })
    .detach();
}

fn answer(q: &Question, yes: bool, cx: &mut App) {
    let key = q.key();
    let f = cx.global_mut::<Ferry>();
    f.questions.retain(|x| x.key() != key);
    let nearby = f.nearby.clone();
    if let Some(n) = nearby {
        match q {
            Question::Offer(o) if yes => {
                let dest = inbox_of(cx.global::<Settings>());
                n.accept(o.id, dest.to_string_lossy().into_owned());
                track(cx, o.transfer, format!("{}: {}", t!("Ferry ile alınıyor"), o.from_name), Some(dest));
            }
            Question::Offer(o) => n.reject(o.id),
            Question::Pair { peer, .. } => n.confirm_pairing(peer.clone(), yes),
        }
    }
    sync_window(cx);
}

/// The question window: open while there is a question, closed when there is none, redrawn when they change.
fn sync_window(cx: &mut App) {
    let Some(f) = cx.try_global::<Ferry>() else { return };
    let empty = f.questions.is_empty();
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
        window_background: crate::theme::window_material(WindowBackgroundAppearance::Blurred),
        ..Default::default()
    };
    match cx.open_window(opts, |window, cx| {
        let hwnd = crate::look::window::hwnd_of(window);
        rf_win::window::set_dark_mode(hwnd, Palette::for_window(window).dark);
        cx.new(|_| QuestionView)
    }) {
        Ok(w) => cx.global_mut::<Ferry>().window = Some(w),
        Err(e) => log::warn!("ferry: question window: {e:#}"),
    }
}

struct QuestionView;

impl Render for QuestionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let questions = cx.try_global::<Ferry>().map(|f| f.questions.clone()).unwrap_or_default();
        let Some(q) = questions.first().cloned() else { return div().into_any_element() };
        let waiting = questions.len() - 1;
        let (title, line, code, no, yes) = match &q {
            Question::Offer(o) => {
                // "photo.jpg", or "photo.jpg and 3 items" (the top-level names: a folder counts once)
                let what = match o.names.len() {
                    0 => crate::i18n::items(o.file_count as usize),
                    1 => o.names[0].clone(),
                    n => format!("{} {} {}", o.names[0], t!("ve"), crate::i18n::items(n - 1)),
                };
                (
                    format!("{} {}", o.from_name, t!("dosya göndermek istiyor")),
                    format!("{} · {}", what, rf_core::format::size(o.total_bytes)),
                    None,
                    t!("Reddet"),
                    t!("Kabul et"),
                )
            }
            Question::Pair { name, code, .. } => (
                format!("{}: {name}", t!("Eşleştir")),
                t!("İki cihazda da aynı kodun göründüğünden emin olun. Eşleşen cihazlar birbirine sormadan dosya gönderebilir.").to_string(),
                Some(format!("{} {}", &code[..3.min(code.len())], &code[3.min(code.len())..])),
                t!("İptal"),
                t!("Eşleştir"),
            ),
        };
        let (qn, qy) = (q.clone(), q);
        v_flex()
            .size_full()
            .p_4()
            .gap_1()
            .text_color(p.text)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(L::Send).size(px(16.)).text_color(p.accent))
                    .child(div().text_xs().text_color(p.text_secondary).child("Ferry"))
                    .children((waiting > 0).then(|| div().ml_auto().text_xs().text_color(p.text_secondary).child(format!("+{waiting}")))),
            )
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).truncate().child(title))
            .child(div().text_xs().text_color(p.text_secondary).overflow_hidden().child(line))
            .children(code.map(|c| div().text_2xl().font_weight(FontWeight::BOLD).child(c)))
            .child(
                h_flex()
                    .mt_auto()
                    .gap_2()
                    .justify_end()
                    .child(crate::settings_view::button("ferry-no", &p, no, move |_, cx| answer(&qn, false, cx)))
                    .child(primary("ferry-yes", &p, yes, move |_, cx| answer(&qy, true, cx))),
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
            |s, v| s.ferry_visibility = v,
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
    if peers.is_empty() {
        v.push(card(
            p,
            L::Wifi,
            t!("Henüz cihaz yok"),
            Some(t!("Diğer cihazda rustyfiles açık ve Ferry'si açık olmalı. Eşleşmemiş bir cihaz yalnızca görünürlüğü Herkes iken listelenir.")),
            div().into_any_element(),
        ));
    }
    for (i, peer) in peers.iter().enumerate() {
        let desc = if peer.trusted { format!("{} · {}", os_name(&peer.os), t!("Güvenilir")) } else { os_name(&peer.os).to_string() };
        let control = if peer.trusted {
            div().into_any_element()
        } else {
            let id = peer.id.clone();
            button(("ferry-pair", i), p, t!("Eşleştir"), move |_, cx| pair(id.clone(), cx))
        };
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

fn os_name(os: &str) -> &str {
    match os {
        "windows" => "Windows",
        "macos" => "macOS",
        "linux" => "Linux",
        other => other,
    }
}

fn pair(peer_id: String, cx: &mut App) {
    let Some(n) = cx.global::<Ferry>().nearby.clone() else { return };
    if let Err(e) = n.pair(peer_id) {
        log::warn!("ferry: cannot pair: {e}");
    }
}
