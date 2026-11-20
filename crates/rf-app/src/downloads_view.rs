//! HyperDownload's page (`Location::Downloads`, the sidebar's row above the Recycle Bin) and its window: what is downloading, with
//! the molten bar, and what was downloaded, with the space it takes. The design is in `notes/hyperdownload-ui.md`.
//!
//! THE MOLTEN BAR. One bar a download, split into the pieces its connections fetch (each as wide as its share of the file). The
//! written part of a piece is ember (HyperDownload's colour); where a connection is writing at this moment its fill ends in a hot cap.
//! A torrent's bar is its piece map: 120 cells, each as ember as the share of that stretch that has arrived.
//!
//! THE DATA. A watch (every [`EVERY`]) asks the running add-on for its active and waiting downloads (it is never started from here: a
//! page that only looks must not start it) and, for each torrent, its piece map. The finished list is `hd_history`, read again when it
//! changes, with which files are still there and the free space of the Downloads folder's drive looked at off the UI thread.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::format;
use serde_json::{Value, json};

use crate::hd_history::{self, Entry};
use crate::hyperdownload::Rpc;
use crate::t;
use crate::theme::{self, Palette};

/// The start argument (and the message to a running rustyfiles) that opens HyperDownload's window.
pub const LAUNCH_ARG: &str = "--downloads";

/// How often the add-on is asked.
const EVERY: Duration = Duration::from_millis(600);
/// How many speed readings the skyline keeps (a minute).
const HISTORY: usize = 100;
/// The piece map's cells.
const MAP_CELLS: usize = 120;

/// HyperDownload's ember (its logo's colour) and the hot cap of a writing connection.
pub fn ember() -> Hsla {
    rgb(0xff6a1a).into()
}
fn ember_hot() -> Hsla {
    rgb(0xffc59a).into()
}

/// A download the add-on is working on.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub gid: String,
    pub torrent: bool,
    pub name: String,
    /// "active", "paused", "waiting", "error".
    pub status: String,
    pub total: u64,
    pub done: u64,
    pub speed: u64,
    /// Connections (HTTP) or peers (torrent).
    pub links: u64,
    /// [start, end, written, a connection on it].
    pub segments: Vec<(u64, u64, u64, bool)>,
    /// '0'..='9' a cell (a torrent's).
    pub map: String,
    pub error: String,
    /// The file list waits for the user's choice.
    pub choosing: bool,
    pub eta: Option<u64>,
}

fn num(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or(x.as_u64())).unwrap_or(0)
}

/// One download from aria2's status object (with HyperDownload's own fields).
pub fn live_of(v: &Value) -> Live {
    let torrent = v.get("hdKind").and_then(Value::as_str) == Some("torrent");
    let file = v.pointer("/files/0/path").and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
    let name = if torrent {
        v.get("hdTitle").and_then(Value::as_str).filter(|s| !s.is_empty()).or_else(|| v.pointer("/bittorrent/info/name").and_then(Value::as_str)).unwrap_or_default().to_string()
    } else {
        file.as_ref().and_then(|f| f.file_name()).map(|n| n.to_string_lossy().into_owned()).or_else(|| v.pointer("/files/0/uris/0/uri").and_then(Value::as_str).map(crate::hyperdownload::label_of)).unwrap_or_default()
    };
    let segments = v
        .get("hdSegments")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|s| Some((s.get(0)?.as_u64()?, s.get(1)?.as_u64()?, s.get(2)?.as_u64()?, s.get(3)?.as_bool()?))).collect())
        .unwrap_or_default();
    let (total, done, speed) = (num(v, "totalLength"), num(v, "completedLength"), num(v, "downloadSpeed"));
    let eta = v.get("hdEta").and_then(Value::as_u64).or_else(|| (speed > 0 && total > done).then(|| (total - done) / speed));
    Live {
        gid: v.get("gid").and_then(Value::as_str).unwrap_or_default().to_string(),
        torrent,
        name,
        status: v.get("status").and_then(Value::as_str).unwrap_or_default().to_string(),
        total,
        done,
        speed,
        links: num(v, "connections"),
        segments,
        map: v.get("hdMap").and_then(Value::as_str).unwrap_or_default().to_string(),
        error: v.get("errorMessage").and_then(Value::as_str).unwrap_or_default().to_string(),
        choosing: v.get("hdChoice").and_then(Value::as_bool) == Some(true),
        eta,
    }
}

/// One look at the add-on: its downloads (torrents with their piece maps), or `None` when it is not running.
fn fetch(rpc: &Option<Rpc>) -> (Option<Rpc>, Option<Vec<Live>>) {
    let rpc = rpc.clone().filter(|r| r.call("aria2.getVersion", vec![]).is_ok()).or_else(crate::hyperdownload::running);
    let Some(r) = rpc else { return (None, None) };
    let mut all: Vec<Value> = Vec::new();
    for (m, p) in [("aria2.tellActive", vec![]), ("aria2.tellWaiting", vec![json!(0), json!(1000)])] {
        if let Ok(Value::Array(a)) = r.call(m, p) {
            all.extend(a);
        }
    }
    let list = all
        .iter()
        .map(|v| {
            let mut l = live_of(v);
            if l.torrent
                && let Ok(full) = r.call("aria2.tellStatus", vec![json!(l.gid)])
            {
                l.map = full.get("hdMap").and_then(Value::as_str).unwrap_or_default().to_string();
            }
            l
        })
        .collect();
    (Some(r), Some(list))
}

/// What the finished list needs from the disk: whether each file is still there, and the Downloads folder's free space.
fn look_at_disk(list: &[Entry]) -> (Vec<bool>, Option<u64>) {
    let there = list.iter().map(|e| e.path.exists()).collect();
    let free = dirs::download_dir().and_then(|d| {
        let root: PathBuf = d.components().take(1).collect::<PathBuf>().join("\\");
        rf_win::volume::volume_info(&root).ok().map(|v| v.free)
    });
    (there, free)
}

pub struct DownloadsView {
    rpc: Option<Rpc>,
    /// `None`: the add-on is not running.
    live: Option<Vec<Live>>,
    speeds: VecDeque<u64>,
    history: Vec<Entry>,
    there: Vec<bool>,
    free: Option<u64>,
    history_gen: u64,
    /// In a window of its own (no "open in window" button then).
    windowed: bool,
    _watch: Task<()>,
}

impl DownloadsView {
    pub fn new(windowed: bool, cx: &mut Context<Self>) -> Self {
        let watch = cx.spawn(async move |this, cx| {
            loop {
                let Ok(rpc) = this.update(cx, |v, _| v.rpc.clone()) else { break };
                let (rpc, live) = cx.background_executor().spawn(async move { fetch(&rpc) }).await;
                let reload = this.update(cx, |v, cx| {
                    v.rpc = rpc;
                    let total: u64 = live.as_ref().map_or(0, |l| l.iter().map(|d| d.speed).sum());
                    v.speeds.push_back(total);
                    while v.speeds.len() > HISTORY {
                        v.speeds.pop_front();
                    }
                    if live != v.live || total > 0 {
                        v.live = live;
                        cx.notify();
                    }
                    (v.history_gen != hd_history::generation()).then(hd_history::generation)
                });
                match reload {
                    Ok(Some(generation)) => {
                        let (list, (there, free)) = cx
                            .background_executor()
                            .spawn(async move {
                                let list = hd_history::load();
                                let disk = look_at_disk(&list);
                                (list, disk)
                            })
                            .await;
                        let _ = this.update(cx, |v, cx| {
                            v.history = list;
                            v.there = there;
                            v.free = free;
                            v.history_gen = generation;
                            cx.notify();
                        });
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
                cx.background_executor().timer(EVERY).await;
            }
        });
        DownloadsView { rpc: None, live: None, speeds: VecDeque::new(), history: Vec::new(), there: Vec::new(), free: None, history_gen: 0, windowed, _watch: watch }
    }

    /// Pauses, resumes or cancels one download (off the UI thread).
    fn command(&mut self, method: &'static str, gid: String, cx: &mut Context<Self>) {
        let Some(rpc) = self.rpc.clone() else { return };
        cx.background_executor()
            .spawn(async move {
                let _ = rpc.call(method, vec![json!(gid)]);
            })
            .detach();
    }

    /// The last minute's total speed: thin ember columns over one hairline, the newest at the right and the brightest. The scale has
    /// room above the fastest reading (a steady download is a calm plateau, not a wall), and never less than 1 MB/s (a trickle stays
    /// low).
    fn render_skyline(&self, p: &Palette) -> impl IntoElement {
        let max = (self.speeds.iter().copied().max().unwrap_or(0) as f32 * 1.35).max(1_048_576.0);
        let pad = HISTORY.saturating_sub(self.speeds.len());
        let n = self.speeds.len();
        let mut row = h_flex().size_full().items_end().gap(px(2.));
        for _ in 0..pad {
            row = row.child(div().flex_1());
        }
        for (i, &s) in self.speeds.iter().enumerate() {
            let f = (s as f32 / max).clamp(0.0, 1.0);
            let newest = i + 1 == n;
            row = row.child(if s == 0 {
                div().flex_1()
            } else {
                div().flex_1().h(relative(f.max(0.03))).rounded_t(px(1.5)).bg(ember().opacity(if newest { 0.95 } else { 0.22 + 0.38 * (i as f32 / n as f32) }))
            });
        }
        div().relative().h(px(36.)).w_full().child(div().absolute().bottom_0().left_0().right_0().h(px(1.)).bg(p.divider)).child(row)
    }
}

/// A small square button with an icon and a tooltip.
fn icon_button(id: impl Into<ElementId>, icon: L, tip: &'static str, p: &Palette) -> Stateful<Div> {
    let hover = p.hover;
    div()
        .id(id.into())
        .size(px(28.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
        .child(Icon::new(icon).size(px(15.)).text_color(p.text_secondary))
}

/// The molten bar of an HTTP download: its pieces side by side, each as wide as its share of the file.
pub fn segment_bar(segs: &[(u64, u64, u64, bool)], total: u64, done: u64, p: &Palette) -> AnyElement {
    let track = p.divider;
    if segs.is_empty() || total == 0 {
        // (no size known: one bar of what came, or nothing yet)
        let f = if total > 0 { done as f32 / total as f32 } else { 0.0 };
        return div().h(px(6.)).w_full().rounded_full().bg(track).child(div().h_full().w(relative(f.clamp(0.0, 1.0))).rounded_full().bg(ember())).into_any_element();
    }
    let span_all: u64 = segs.iter().map(|s| s.1.saturating_sub(s.0)).sum::<u64>().max(1);
    let mut row = h_flex().h(px(6.)).w_full();
    let last = segs.len() - 1;
    for (i, &(start, end, written, active)) in segs.iter().enumerate() {
        let span = end.saturating_sub(start).max(1);
        let share = span as f32 / span_all as f32;
        let f = (written as f32 / span as f32).clamp(0.0, 1.0);
        let fill = h_flex()
            .h_full()
            .w(relative(f))
            .justify_end()
            .rounded_full()
            .bg(ember())
            .when(active && f < 1.0, |d| d.child(div().h_full().w(px(3.)).rounded_full().bg(ember_hot())));
        row = row.child(div().h_full().w(relative(share)).when(i < last, |d| d.pr(px(2.))).child(div().size_full().rounded_full().bg(track).child(fill)));
    }
    row.into_any_element()
}

/// A torrent's bar: its piece map, each cell as ember as the share of that stretch that has arrived.
pub fn map_bar(map: &str, total: u64, done: u64, p: &Palette) -> AnyElement {
    if map.is_empty() {
        return segment_bar(&[], total, done, p);
    }
    let cells: Vec<f32> = map.bytes().map(|b| f32::from(b.saturating_sub(b'0').min(9)) / 9.0).collect();
    let n = cells.len().min(MAP_CELLS).max(1);
    let mut row = h_flex().h(px(6.)).w_full().rounded_full().overflow_hidden().bg(p.divider);
    for f in cells.into_iter().take(n) {
        row = row.child(div().h_full().w(relative(1.0 / n as f32)).when(f > 0.0, |d| d.bg(ember().opacity(0.2 + 0.8 * f))));
    }
    row.into_any_element()
}

// ---------------------------------------------------------------------------------------------
// The molten bar on a download's Status Center card
// ---------------------------------------------------------------------------------------------

/// What a download's bar is drawn from: an HTTP download's pieces, or a torrent's piece map (with the bytes done and in all).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Segments(Vec<(u64, u64, u64, bool)>, u64, u64),
    Map(String, u64, u64),
}

static SHAPES: std::sync::Mutex<Option<std::collections::HashMap<u64, Shape>>> = std::sync::Mutex::new(None);

/// The bar of the download behind the Status Center card `card` (`None`: the card is done with it). From the download's thread.
pub fn set_shape(card: u64, shape: Option<Shape>) {
    let mut g = SHAPES.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(Default::default);
    match shape {
        Some(s) => {
            map.insert(card, s);
        }
        None => {
            map.remove(&card);
        }
    }
}

/// A status object's bar.
pub fn shape_of(v: &Value) -> Shape {
    let l = live_of(v);
    if l.torrent { Shape::Map(v.get("hdMap").and_then(Value::as_str).unwrap_or_default().to_string(), l.total, l.done) } else { Shape::Segments(l.segments, l.total, l.done) }
}

/// The molten bar for a card, when the card is a download's.
pub fn card_bar(card: u64, p: &Palette) -> Option<AnyElement> {
    let shape = SHAPES.lock().ok()?.as_ref()?.get(&card).cloned()?;
    Some(match shape {
        Shape::Segments(s, total, done) => segment_bar(&s, total, done, p),
        Shape::Map(m, total, done) => map_bar(&m, total, done, p),
    })
}

/// "1 dk 20 sn" (from the Status Center's wording).
fn eta_text(secs: u64) -> String {
    crate::status_center::fmt_duration(secs as f32)
}

impl EventEmitter<()> for DownloadsView {}

impl Render for DownloadsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let me = cx.entity().downgrade();
        let mut list = v_flex().id("dl-list").flex_1().min_h_0().overflow_y_scroll().px_3().pb_4();

        // --- what is downloading
        let live = self.live.clone().unwrap_or_default();
        list = list.child(section_title(t!("İndiriliyor"), Some(live.len().to_string()), None, &p));
        if live.is_empty() {
            let line = if self.live.is_none() { t!("HyperDownload şu an çalışmıyor: bir indirme başlayınca kendiliğinden açılır.") } else { t!("Şu an inen bir şey yok. Bir bağlantıyı herhangi bir klasöre yapıştırın ya da buradan ekleyin.") };
            list = list.child(div().px_2().py_3().text_sm().text_color(p.text_secondary).child(line));
        }
        for d in &live {
            list = list.child(live_row(d, &p, me.clone()));
        }

        // --- what was downloaded
        let total_size: u64 = self.history.iter().map(|e| e.size).sum();
        let summary = (!self.history.is_empty()).then(|| format!("{} {}, {}", self.history.len(), t!("dosya"), format::size(total_size)));
        let free = self.free.map(|f| format!("{} {}", format::size(f), t!("boş (İndirilenler)")));
        let clear = (!self.history.is_empty()).then(|| {
            div()
                .id("dl-clear")
                .px_2()
                .py_1()
                .rounded(theme::RADIUS_SM)
                .text_xs()
                .text_color(p.text_secondary)
                .cursor_pointer()
                .hover(|s| s.bg(p.hover).text_color(p.text))
                .child(t!("Listeyi temizle"))
                .on_click(|_, _, _| hd_history::clear())
                .into_any_element()
        });
        list = list.child(div().h(px(1.)).mx_2().mt_3().bg(p.divider));
        list = list.child(section_title(t!("Bitenler"), summary, Some((free, clear)), &p));
        if self.history.is_empty() {
            list = list.child(div().px_2().py_3().text_sm().text_color(p.text_secondary).child(t!("Biten indirmeler burada, kapladıkları yerle birlikte listelenir.")));
        }
        for (i, e) in self.history.iter().enumerate() {
            let there = self.there.get(i).copied().unwrap_or(true);
            list = list.child(done_row(i, e, there, &p));
        }

        v_flex().size_full().text_color(p.text).child(self.render_header_el(&p, cx)).child(div().h(px(1.)).bg(p.divider)).child(list)
    }
}

impl DownloadsView {
    fn render_header_el(&self, p: &Palette, cx: &mut Context<Self>) -> AnyElement {
        let total: u64 = self.live.as_ref().map_or(0, |l| l.iter().map(|d| d.speed).sum());
        let add = crate::settings_view::button("dl-add", p, t!("Bağlantı ekle"), |window, cx| {
            if let Some(dir) = dirs::download_dir() {
                crate::hyperdownload::ask_link(dir, window, cx);
            }
        });
        let pop = (!self.windowed).then(|| icon_button("dl-window", L::PictureInPicture2, t!("Ayrı pencerede aç"), p).on_click(|_, _, cx| open_window(cx)));
        let _ = cx;
        v_flex()
            .gap_2()
            .px_5()
            .pt_4()
            .pb_3()
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(div().size(px(28.)).flex().items_center().justify_center().rounded(px(7.)).bg(ember().opacity(0.14)).child(Icon::new(L::ArrowDownToLine).size(px(16.)).text_color(ember())))
                    .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child("HyperDownload"))
                    .child(div().flex_1())
                    .when(total > 0, |d| d.child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(ember()).child(crate::transfer_view::speed_text(total as f64))))
                    .child(add)
                    .children(pop),
            )
            .child(self.render_skyline(p))
            .into_any_element()
    }
}

/// A section's title line: its name, a count or summary beside it, and at the right what it offers.
fn section_title(name: &'static str, beside: Option<String>, right: Option<(Option<String>, Option<AnyElement>)>, p: &Palette) -> AnyElement {
    let (note, action) = right.unwrap_or((None, None));
    h_flex()
        .px_2()
        .pt_3()
        .pb_1()
        .gap_2()
        .items_center()
        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(name))
        .children(beside.map(|b| div().text_xs().text_color(p.text_tertiary).child(b)))
        .child(div().flex_1())
        .children(note.map(|n| div().text_xs().text_color(p.text_tertiary).child(n)))
        .children(action)
        .into_any_element()
}

/// The numbers' columns: fixed widths, right-aligned, so they do not dance while they change.
fn col(w: f32, text: impl Into<SharedString>, color: Hsla) -> Div {
    div().w(px(w)).flex_shrink_0().text_right().text_xs().text_color(color).whitespace_nowrap().child(text.into())
}

fn live_row(d: &Live, p: &Palette, me: WeakEntity<DownloadsView>) -> AnyElement {
    let paused = d.status == "paused";
    let failed = d.status == "error";
    let sizes = if d.total > 0 { format!("{} / {}", format::size(d.done), format::size(d.total)) } else { format::size(d.done) };
    let speed = if paused { t!("Duraklatıldı").to_string() } else if d.speed > 0 { crate::transfer_view::speed_text(d.speed as f64) } else { String::new() };
    let eta = d.eta.filter(|_| !paused && d.speed > 0).map(eta_text).unwrap_or_default();
    let caption = if d.choosing {
        t!("Dosyaların seçilmesi bekleniyor").to_string()
    } else if failed {
        d.error.clone()
    } else if d.torrent {
        format!("{} {}", d.links, t!("eş"))
    } else {
        format!("{} {}", d.links, t!("bağlantı"))
    };
    let bar = if d.torrent { map_bar(&d.map, d.total, d.done, p) } else { segment_bar(&d.segments, d.total, d.done, p) };
    let (g1, g2, m1, m2) = (d.gid.clone(), d.gid.clone(), me.clone(), me);
    let toggle = icon_button(("dl-toggle", hash_of(&d.gid)), if paused || failed { L::Play } else { L::Pause }, if paused || failed { t!("Sürdür") } else { t!("Duraklat") }, p).on_click(move |_, _, cx| {
        let method = if paused || failed { "aria2.unpause" } else { "aria2.pause" };
        let _ = m1.update(cx, |v, cx| v.command(method, g1.clone(), cx));
    });
    let cancel = icon_button(("dl-cancel", hash_of(&d.gid)), L::X, t!("İptal"), p).on_click(move |_, _, cx| {
        let _ = m2.update(cx, |v, cx| v.command("aria2.remove", g2.clone(), cx));
    });
    h_flex()
        .px_2()
        .py_2()
        .gap_3()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .hover(|s| s.bg(p.hover))
        .child(div().size(px(32.)).flex_shrink_0().flex().items_center().justify_center().rounded(px(8.)).bg(p.card_bg).child(Icon::new(if d.torrent { L::Magnet } else { L::FileDown }).size(px(16.)).text_color(p.text_secondary)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1p5()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::MEDIUM).child(d.name.clone()))
                        .child(col(150., sizes, p.text_secondary))
                        .child(col(90., speed, if paused { p.text_tertiary } else { p.text }))
                        .child(col(70., eta, p.text_secondary)),
                )
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(div().flex_1().min_w_0().child(bar))
                        .child(col(90., caption, if failed { p.danger } else { p.text_tertiary })),
                ),
        )
        .child(h_flex().flex_shrink_0().gap_1().child(toggle).child(cancel))
        .into_any_element()
}

fn done_row(i: usize, e: &Entry, there: bool, p: &Palette) -> AnyElement {
    let (open, show, forget) = (e.path.clone(), e.path.clone(), e.path.clone());
    let when = format::datetime(e.finished_ms);
    h_flex()
        .id(("dl-done", i))
        .px_2()
        .py_1p5()
        .gap_3()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .hover(|s| s.bg(p.hover))
        .when(!e.source.is_empty(), |d| {
            let src: SharedString = e.source.clone().into();
            d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(src.clone()).build(w, cx))
        })
        .when(there, |d| d.cursor_pointer().on_click(move |ev, _, cx| {
            if ev.click_count() >= 2 {
                crate::folder_view::open_in_background(open.clone(), cx);
            }
        }))
        .child(div().size(px(32.)).flex_shrink_0().flex().items_center().justify_center().rounded(px(8.)).bg(p.card_bg).child(Icon::new(if e.torrent { L::FolderDown } else { L::FileCheck }).size(px(16.)).text_color(if there { ember() } else { p.text_tertiary })))
        .child(div().flex_1().min_w_0().truncate().text_sm().when(!there, |d| d.text_color(p.text_tertiary).line_through()).child(e.name.clone()))
        .when(!there, |d| d.child(col(80., t!("Silinmiş"), p.text_tertiary)))
        .child(col(90., format::size(e.size), p.text_secondary))
        .child(col(130., when, p.text_tertiary))
        .child(
            h_flex()
                .flex_shrink_0()
                .gap_1()
                .when(there, |d| {
                    d.child(icon_button(("dl-show", i), L::FolderOpen, t!("Klasörde göster"), p).on_click(move |_, _, cx| {
                        crate::workspace::route_ipc(format!("{}{}", crate::workspace::SELECT_PREFIX, show.display()), cx);
                    }))
                })
                .child(icon_button(("dl-forget", i), L::X, t!("Listeden kaldır"), p).on_click(move |_, _, _| hd_history::forget(&forget))),
        )
        .into_any_element()
}

fn hash_of(s: &str) -> usize {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish() as usize
}

// ---------------------------------------------------------------------------------------------
// The window of its own
// ---------------------------------------------------------------------------------------------

struct DownloadsWindow {
    view: Entity<DownloadsView>,
}

impl Render for DownloadsWindow {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        v_flex()
            .size_full()
            .bg(p.content_bg)
            .child(
                gpui_kit::component::TitleBar::new()
                    .h(theme::TITLEBAR_H)
                    .bg(p.titlebar_bg)
                    .border_color(gpui_kit::transparent_black())
                    .child(h_flex().gap_2().items_center().text_xs().text_color(p.text_secondary).child("HyperDownload")),
            )
            .child(div().flex_1().min_h_0().child(self.view.clone()))
    }
}

/// HyperDownload's own window (the page's button); one at a time: a second ask brings the first forward.
pub fn open_window(cx: &mut App) {
    if let Some(h) = cx.try_global::<OpenWindow>().and_then(|w| w.0)
        && h.update(cx, |_, window, _| window.activate_window()).is_ok()
    {
        return;
    }
    let bounds = WindowBounds::centered(size(px(820.), px(600.)), cx);
    let options = WindowOptions {
        window_bounds: Some(bounds),
        window_min_size: Some(size(px(560.), px(360.))),
        window_background: crate::theme::window_material(WindowBackgroundAppearance::MicaBackdrop),
        ..gpui_kit::component::TitleBar::window_options()
    };
    match cx.open_window(options, |window, cx| {
        window.set_window_title("HyperDownload");
        let view = cx.new(|cx| DownloadsView::new(true, cx));
        cx.new(|_| DownloadsWindow { view })
    }) {
        Ok(h) => cx.set_global(OpenWindow(Some(h))),
        Err(e) => log::warn!("downloads window: {e:#}"),
    }
}

struct OpenWindow(Option<WindowHandle<DownloadsWindow>>);
impl Global for OpenWindow {}

#[cfg(test)]
mod tests {
    // (not `super::*`: the toolkit's glob import brings a `test` attribute of its own)
    use super::live_of;
    use serde_json::json;

    #[test]
    fn a_status_becomes_a_row() {
        let v = json!({ "gid": "00000000000000a1", "status": "active", "totalLength": "1000", "completedLength": "400", "downloadSpeed": "100",
                        "connections": "8", "files": [{ "path": "C:\\d\\ubuntu.iso" }], "hdKind": "http",
                        "hdSegments": [[0, 500, 300, true], [500, 1000, 100, false]] });
        let l = live_of(&v);
        assert_eq!(l.name, "ubuntu.iso");
        assert_eq!((l.total, l.done, l.speed, l.links), (1000, 400, 100, 8));
        assert_eq!(l.segments, vec![(0, 500, 300, true), (500, 1000, 100, false)]);
        assert_eq!(l.eta, Some(6));
        let t = live_of(&json!({ "gid": "x", "status": "paused", "hdKind": "torrent", "hdTitle": "Film (2026)", "bittorrent": { "info": { "name": "Film.2026.1080p" } }, "hdEta": 40 }));
        assert!(t.torrent);
        assert_eq!(t.name, "Film (2026)");
        assert_eq!(t.eta, Some(40));
    }
}
