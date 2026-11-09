//! Settings → Speed test: runs [`crate::bench`] on a background thread and shows the results:
//! a race played back at real speed (like rustyfiles.com), a table with time, CPU and memory per
//! app, our engines against Windows', then a star request and sharing.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use parking_lot::Mutex;
use rf_win::bench::{App as BenchApp, AppKind, Machine, OpenResult};

use crate::bench::{OpenTest, Progress, Results};
use crate::settings::{self, Settings};
use crate::t;
use crate::theme::{self, Palette};

const SITE: &str = "https://rustyfiles.com";
const REPO: &str = "https://github.com/gelenmedya/rustyfiles";
/// One dot of the race track stands for this long.
const DOT_MS: f64 = 20.0;

/// What the background run reports.
#[derive(Default)]
struct Shared {
    progress: Option<Progress>,
    done: Option<Result<Results, String>>,
}

struct State {
    detected: Vec<BenchApp>,
    machine: Option<Machine>,
    off: HashSet<AppKind>,
    running: Option<(Arc<Mutex<Shared>>, Arc<AtomicBool>)>,
    progress: Option<Progress>,
    results: Option<Results>,
    error: Option<String>,
    race: OpenTest,
    replay: Instant,
    /// "Copied" feedback on the share buttons.
    note: Option<(String, Instant)>,
}
impl Global for State {}

fn state(cx: &mut App) -> &mut State {
    if !cx.has_global::<State>() {
        let detected = rf_win::bench::detect();
        cx.set_global(State {
            detected,
            machine: None,
            off: HashSet::new(),
            running: None,
            progress: None,
            results: crate::bench::last_results(),
            error: None,
            race: OpenTest::Files100k,
            replay: Instant::now() - Duration::from_secs(600),
            note: None,
        });
        // the machine line reads the registry and asks the disk: not on the UI thread
        cx.spawn(async move |cx| {
            let m = cx.background_spawn(async { rf_win::bench::machine(&crate::bench::bench_dir()) }).await;
            cx.update(|cx| {
                cx.global_mut::<State>().machine = Some(m);
                cx.refresh_windows();
            });
        })
        .detach();
    }
    cx.global_mut::<State>()
}

// ---------------------------------------------------------------- formatting

fn ms(v: f64) -> String {
    if v >= 10_000.0 {
        format!("{:.1} s", v / 1000.0)
    } else if v >= 1000.0 {
        format!("{:.2} s", v / 1000.0)
    } else {
        format!("{v:.0} ms")
    }
}

fn mb(bytes: u64) -> String {
    format!("{} MB", (bytes as f64 / 1_048_576.0).round() as u64)
}

/// 2330750 → "2.330.750" (Turkish) / "2,330,750".
pub(crate) fn count(n: u64) -> String {
    let sep = if crate::i18n::t("1.000 dosya").starts_with("1.000") { '.' } else { ',' };
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(sep);
        }
        out.push(c);
    }
    out
}

fn times(slow: f64, fast: f64) -> String {
    let x = slow / fast.max(1.0);
    if x >= 10.0 { format!("{x:.0}×") } else { format!("{x:.1}×") }
}

fn test_label(t: OpenTest) -> &'static str {
    match t {
        OpenTest::Start => t!("Açılış (boş klasör)"),
        OpenTest::Files1k => t!("1.000 dosya"),
        OpenTest::Files10k => t!("10.000 dosya"),
        OpenTest::Files100k => t!("100.000 dosya"),
        OpenTest::Images => t!("2.000 resim"),
    }
}

fn app_label(k: AppKind) -> &'static str {
    match k {
        AppKind::Rustyfiles => "rustyfiles",
        AppKind::Explorer => t!("Dosya Gezgini"),
        AppKind::Files => "Files",
        AppKind::FilePilot => "File Pilot",
    }
}

fn machine_line(m: &Machine) -> String {
    let mut parts = vec![m.cpu.clone()];
    if m.ram_gb > 0 {
        parts.push(format!("{} GB RAM", m.ram_gb));
    }
    if !m.disk.is_empty() {
        parts.push(m.disk.clone());
    }
    parts.push(m.windows.clone());
    parts.join(" · ")
}

/// rustyfiles' median and the slowest other app's on `test`, for the headline.
fn speedup(r: &Results, test: OpenTest, other: AppKind) -> Option<(f64, f64)> {
    let o = r.open.iter().find(|o| o.test == test)?;
    let ours = o.apps.iter().find(|a| a.app == AppKind::Rustyfiles)?.median()?.settled_ms;
    let theirs = o.apps.iter().find(|a| a.app == other)?.median()?.settled_ms;
    Some((ours, theirs))
}

// ---------------------------------------------------------------- running

fn start(cx: &mut App) {
    let st = state(cx);
    if st.running.is_some() {
        return;
    }
    let mut apps = vec![crate::bench::rustyfiles_app()];
    apps.extend(st.detected.iter().filter(|a| !st.off.contains(&a.kind)).cloned());
    let plan = crate::bench::Plan { apps, open: OpenTest::ALL.to_vec(), copy: true, engines: true };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let cancel = Arc::new(AtomicBool::new(false));
    st.running = Some((shared.clone(), cancel.clone()));
    st.error = None;
    st.progress = None;
    {
        let shared = shared.clone();
        std::thread::Builder::new()
            .name("rf-bench".into())
            .spawn(move || {
                let report = |p: Progress| shared.lock().progress = Some(p);
                let r = crate::bench::run(&plan, &report, &cancel).map_err(|e| format!("{e:#}"));
                shared.lock().done = Some(r);
            })
            .ok();
    }
    // mirror the thread's progress into the page until it finishes
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(200)).await;
            let (p, done) = {
                let mut s = shared.lock();
                (s.progress.clone(), s.done.take())
            };
            let finished = cx.update(|cx| {
                let st = cx.global_mut::<State>();
                st.progress = p;
                if let Some(d) = done {
                    st.running = None;
                    match d {
                        Ok(r) => {
                            st.results = Some(r);
                            st.replay = Instant::now();
                        }
                        Err(e) if e == "cancelled" => {}
                        Err(e) => st.error = Some(e),
                    }
                    cx.refresh_windows();
                    return true;
                }
                cx.refresh_windows();
                false
            });
            if finished {
                break;
            }
        }
        replay_frames(cx).await;
    })
    .detach();
}

/// Redraws while the race replays.
async fn replay_frames(cx: &mut AsyncApp) {
    loop {
        let more = cx.update(|cx| {
            let st = cx.global::<State>();
            let longest = st.results.as_ref().map_or(0.0, |r| race_longest(r, st.race));
            let going = st.replay.elapsed().as_secs_f64() * 1000.0 < longest + 300.0;
            cx.refresh_windows();
            going
        });
        if !more {
            break;
        }
        cx.background_executor().timer(Duration::from_millis(33)).await;
    }
}

fn race_longest(r: &Results, test: OpenTest) -> f64 {
    r.open
        .iter()
        .find(|o| o.test == test)
        .map_or(0.0, |o| o.apps.iter().filter_map(|a| a.median()).map(|m| m.settled_ms).fold(0.0, f64::max))
}

fn replay(test: OpenTest, cx: &mut App) {
    let st = state(cx);
    st.race = test;
    st.replay = Instant::now();
    cx.spawn(async move |cx| replay_frames(cx).await).detach();
}

// ---------------------------------------------------------------- sharing

/// base64url without padding.
fn b64url(data: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for k in 0..c.len() + 1 {
            out.push(A[(n >> (18 - 6 * k) & 63) as usize] as char);
        }
    }
    out
}

fn app_key(k: AppKind) -> &'static str {
    match k {
        AppKind::Rustyfiles => "rf",
        AppKind::Explorer => "ex",
        AppKind::Files => "fi",
        AppKind::FilePilot => "fp",
    }
}

/// The results as the site reads them (link format v1, agreed with the site): a small JSON,
/// base64url, in the path of rustyfiles.com/r/…. Measurements and the PC model only.
pub(crate) fn share_payload(r: &Results) -> serde_json::Value {
    use serde_json::{Map, Value, json};
    let mut open = Vec::new();
    let mut shared: Vec<&str> = Vec::new();
    for o in &r.open {
        let mut row = Map::new();
        let n = match o.test {
            OpenTest::Start => 0,
            OpenTest::Files1k => 1000,
            OpenTest::Files10k => 10000,
            OpenTest::Files100k => 100000,
            OpenTest::Images => {
                row.insert("img".into(), json!(true));
                2000
            }
        };
        row.insert("n".into(), json!(n));
        for a in &o.apps {
            let m = a.median();
            row.insert(app_key(a.app).into(), m.map_or(Value::Null, |m| json!(m.settled_ms.round() as i64)));
            if m.is_some_and(|m| m.shared) && !shared.contains(&app_key(a.app)) {
                shared.push(app_key(a.app));
            }
        }
        open.push(Value::Object(row));
    }
    let mut out = json!({
        "v": 1,
        "app": r.version,
        "t": r.when / 1000,
        "pc": {"cpu": r.machine.cpu, "ram": r.machine.ram_gb, "disk": r.machine.disk, "os": r.machine.windows, "cores": r.machine.cores},
        "open": open,
        "shared": shared,
    });
    let o = out.as_object_mut().expect("object");
    // CPU and memory from the largest folder that was opened
    let size = |t: OpenTest| match t {
        OpenTest::Files100k => 100000,
        OpenTest::Files10k => 10000,
        OpenTest::Files1k => 1000,
        _ => 0,
    };
    if let Some(l) = r.open.iter().filter(|x| size(x.test) > 0).max_by_key(|x| size(x.test)) {
        let (mut cpu, mut mem) = (Map::new(), Map::new());
        for a in &l.apps {
            let m = a.median();
            cpu.insert(app_key(a.app).into(), m.map_or(Value::Null, |m| json!(m.cpu_ms.round() as i64)));
            mem.insert(app_key(a.app).into(), m.map_or(Value::Null, |m| json!(((if m.private > 0 { m.private } else { m.ram }) as f64 / 1_048_576.0).round() as i64)));
        }
        o.insert("of".into(), json!(size(l.test)));
        o.insert("cpu".into(), Value::Object(cpu));
        o.insert("mem".into(), Value::Object(mem));
    }
    let e = &r.engines;
    let vs = |v: &Option<crate::bench::Versus>| v.as_ref().and_then(|v| Some((v.ours_ms?.round() as i64, v.windows_ms?.round() as i64)));
    if let Some((a, b)) = vs(&e.copy_small) {
        o.insert("copy".into(), json!({"n": 10000, "mb": 39, "rf": a, "win": b}));
    }
    if let Some((a, b)) = vs(&e.copy_big) {
        o.insert("big".into(), json!({"mb": 1024, "rf": a, "win": b}));
    }
    if let Some((a, b)) = vs(&e.unzip) {
        o.insert("zip".into(), json!({"n": 10000, "rf": a, "win": b}));
    }
    if let Some(ms) = e.search_ms {
        o.insert("search".into(), json!({"n": e.search_entries, "rf": ms.round() as i64}));
    }
    out
}

/// rustyfiles.com/r/… for these results.
pub(crate) fn share_link(r: &Results) -> String {
    let json = serde_json::to_vec(&share_payload(r)).unwrap_or_default();
    format!("{SITE}/r/{}", b64url(&json))
}

fn summary_text(r: &Results) -> String {
    let mut lines = vec![format!("rustyfiles · {}", machine_line(&r.machine))];
    for o in &r.open {
        if !matches!(o.test, OpenTest::Files100k | OpenTest::Start) {
            continue;
        }
        let row: Vec<String> = o.apps.iter().filter_map(|a| a.median().map(|m| format!("{} {}", app_label(a.app), ms(m.settled_ms)))).collect();
        if !row.is_empty() {
            lines.push(format!("{}: {}", test_label(o.test), row.join(", ")));
        }
    }
    if let Some(v) = &r.engines.copy_small {
        if let (Some(a), Some(b)) = (v.ours_ms, v.windows_ms) {
            lines.push(format!("{}: rustyfiles {} · Windows {}", t!("10.000 küçük dosya kopyalama"), ms(a), ms(b)));
        }
    }
    lines.push(format!("{} {}", t!("Sonuçlar:"), share_link(r)));
    lines.push(format!("{} {SITE}", t!("Ücretsiz ve açık kaynak:")));
    lines.join("\n")
}

/// The short line for X / WhatsApp / Reddit titles.
fn headline(r: &Results) -> String {
    match speedup(r, OpenTest::Files100k, AppKind::Explorer) {
        Some((ours, theirs)) => t!("rustyfiles bilgisayarımda 100.000 dosyalık klasörü {ours} sürede açtı, Dosya Gezgini {theirs}.")
            .replace("{ours}", &ms(ours))
            .replace("{theirs}", &ms(theirs)),
        None => t!("rustyfiles: Windows için hızlı, ücretsiz ve açık kaynak dosya yöneticisi.").to_string(),
    }
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn open_url(url: String) {
    std::thread::spawn(move || {
        let _ = rf_win::shell::open(std::path::Path::new(&url));
    });
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The result card as SVG (1200 × 675, the size social sites show whole).
fn card_svg(r: &Results) -> String {
    // 1200 × 675, the size social sites show whole; the same look as rustyfiles' posts:
    // two-tone wordmark, one big multiplier, bars, three figures, a warm glow
    let (w, h) = (1200.0, 675.0);
    let orange = "#ff6a1a";
    let test = if r.open.iter().any(|o| o.test == OpenTest::Files100k) { OpenTest::Files100k } else { r.open.iter().map(|o| o.test).find(|t| *t != OpenTest::Start).unwrap_or(OpenTest::Files10k) };
    let row = r.open.iter().find(|o| o.test == test);
    let rows: Vec<(AppKind, f64, bool)> = row.map(|o| o.apps.iter().filter_map(|a| a.median().map(|m| (a.app, m.settled_ms, m.shared))).collect()).unwrap_or_default();
    let ours = rows.iter().find(|x| x.0 == AppKind::Rustyfiles).map(|x| x.1);
    let explorer = rows.iter().find(|x| x.0 == AppKind::Explorer).map(|x| x.1);
    let mut s = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs>
<radialGradient id="glow" cx="0.82" cy="0.42" r="0.55"><stop offset="0" stop-color="{orange}" stop-opacity="0.16"/><stop offset="1" stop-color="{orange}" stop-opacity="0"/></radialGradient>
<linearGradient id="hot" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="{orange}"/><stop offset="1" stop-color="#ff8a45"/></linearGradient>
</defs>
<rect width="{w}" height="{h}" fill="#0f1013"/>
<rect width="{w}" height="{h}" fill="url(#glow)"/>
<text x="64" y="104" font-size="66" font-weight="800" letter-spacing="-2" fill="#f2f2f4">rusty<tspan fill="{orange}">files</tspan></text>
<text x="66" y="146" font-size="24" fill="#a3a6b0">{}</text>
<text x="66" y="178" font-size="18" fill="#6e717b">{}</text>
"##,
        esc(t!("Bilgisayarımdaki hız testi")),
        esc(&machine_line(&r.machine)),
    );
    // the headline: how much faster than File Explorer at opening the folder
    if let (Some(a), Some(b)) = (ours, explorer) {
        let headline = if b > a * 1.05 { times(b, a) } else { "=".into() };
        s.push_str(&format!(
            r##"<text x="62" y="330" font-size="132" font-weight="800" letter-spacing="-5" fill="{orange}">{}</text>
<text x="66" y="378" font-size="28" fill="#f2f2f4">{}</text>
<text x="66" y="410" font-size="20" fill="#6e717b">{}</text>
"##,
            esc(&headline),
            esc(if b > a * 1.05 { t!("Dosya Gezgini'nden hızlı") } else { t!("Dosya Gezgini kadar hızlı") }),
            esc(&t!("{test}: klasörün açılması").replace("{test}", test_label(test))),
        ));
    }
    // bars on the right
    let longest = rows.iter().map(|x| x.1).fold(1.0, f64::max);
    let (bx, bw) = (640.0, 420.0);
    s.push_str(&format!(r##"<text x="{bx}" y="236" font-size="18" fill="#6e717b">{}</text>"##, esc(t!("Klasörün açılması, gerçek hızda"))));
    for (i, (app, v, shared)) in rows.iter().enumerate() {
        let y = 262.0 + i as f64 * 62.0;
        let len = (v / longest * bw).max(10.0);
        let mine = *app == AppKind::Rustyfiles;
        s.push_str(&format!(
            r##"<text x="{bx}" y="{}" font-size="20" fill="{}">{}</text><text x="{}" y="{}" font-size="20" font-weight="600" fill="{}" text-anchor="end">{}{}</text><rect x="{bx}" y="{}" width="{len:.0}" height="16" rx="5" fill="{}"/>"##,
            y + 18.0,
            if mine { "#f2f2f4" } else { "#a3a6b0" },
            esc(app_label(*app)),
            bx + bw + 76.0,
            y + 18.0,
            if mine { "#ff8a45" } else { "#f2f2f4" },
            esc(&ms(*v)),
            if *shared { " *" } else { "" },
            y + 28.0,
            if mine { "url(#hot)" } else { "#444955" },
        ));
    }
    // three figures
    let mut cells: Vec<(String, String)> = Vec::new();
    if let Some(v) = &r.engines.copy_small {
        if let (Some(a), Some(b)) = (v.ours_ms, v.windows_ms) {
            cells.push((times(b, a), t!("10.000 küçük dosya kopyalama").to_string()));
        }
    }
    if let Some(v) = &r.engines.unzip {
        if let (Some(a), Some(b)) = (v.ours_ms, v.windows_ms) {
            cells.push((times(b, a), t!("ZIP çıkarma: 10.000 dosya").to_string()));
        }
    }
    if let Some(m) = row.and_then(|o| o.apps.iter().find(|a| a.app == AppKind::Rustyfiles)).and_then(|a| a.median()) {
        cells.push((ms(m.cpu_ms), t!("işlemci süresi").to_string()));
    }
    let n = cells.len().max(1) as f64;
    let (cx0, cw, cy) = (64.0, (w - 128.0) / n, 470.0);
    if !cells.is_empty() {
        s.push_str(&format!(r##"<rect x="{cx0}" y="{cy}" width="{}" height="104" rx="16" fill="#17191e" stroke="#262932"/>"##, w - 128.0));
    }
    for (i, (big, small)) in cells.iter().enumerate() {
        let x = cx0 + i as f64 * cw;
        if i > 0 {
            s.push_str(&format!(r##"<line x1="{x}" y1="{}" x2="{x}" y2="{}" stroke="#262932"/>"##, cy, cy + 104.0));
        }
        s.push_str(&format!(
            r##"<text x="{}" y="{}" font-size="44" font-weight="800" letter-spacing="-1" fill="#f2f2f4">{}</text><text x="{}" y="{}" font-size="18" fill="#a3a6b0">{}</text>"##,
            x + 28.0,
            cy + 56.0,
            esc(big),
            x + 30.0,
            cy + 86.0,
            esc(small)
        ));
    }
    s.push_str(&format!(
        r##"<text x="64" y="{}" font-size="22" font-weight="600" fill="#ff8a45">rustyfiles.com</text><text x="{}" y="{}" font-size="17" fill="#6e717b" text-anchor="end">{}</text></svg>"##,
        h - 40.0,
        w - 64.0,
        h - 40.0,
        esc(t!("Kendi bildirdiği sonuç · ücretsiz ve açık kaynak · Windows"))
    ));
    s
}

fn hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get()),
        _ => None,
    }
}

fn note(msg: &str, cx: &mut App) {
    state(cx).note = Some((msg.to_string(), Instant::now()));
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(2600)).await;
        cx.update(|cx| cx.refresh_windows());
    })
    .detach();
}

fn copy_picture(window: &Window, cx: &mut App) {
    let Some(r) = state(cx).results.clone() else { return };
    let owner = hwnd(window);
    let svg = card_svg(&r);
    match rf_win::bench::render_card(&svg, 1.0) {
        Ok(pic) => {
            // also kept as a file next to the results, for attaching
            let path = crate::bench::bench_dir().join("rustyfiles-speed-test.png");
            let _ = std::fs::write(&path, &pic.png);
            let ok = owner.is_some_and(|h| rf_win::clipboard::set_image(windows::Win32::Foundation::HWND(h as *mut _), pic.width, pic.height, &pic.rgba, &pic.png).is_ok());
            note(if ok { t!("Sonuç kartı panoya kopyalandı") } else { t!("Panoya kopyalanamadı") }, cx);
        }
        Err(e) => {
            log::warn!("bench card: {e:#}");
            note(t!("Kart çizilemedi"), cx);
        }
    }
}

/// Saves the card to Downloads and opens it.
fn save_picture(cx: &mut App) {
    let Some(r) = state(cx).results.clone() else { return };
    let svg = card_svg(&r);
    std::thread::spawn(move || {
        let Ok(pic) = rf_win::bench::render_card(&svg, 1.0) else { return };
        let dir = rf_win::known::KnownFolder::Downloads.path().unwrap_or_else(std::env::temp_dir);
        let path = rf_win::fileops::unique_name(&dir, "rustyfiles speed test", ".png");
        if std::fs::write(&path, &pic.png).is_ok() {
            let _ = rf_win::shell::open(&path);
        }
    });
    note(t!("Kart İndirilenler klasörüne kaydedildi"), cx);
}

fn copy_text(window: &Window, cx: &mut App) {
    let Some(r) = state(cx).results.clone() else { return };
    let text = summary_text(&r);
    cx.write_to_clipboard(ClipboardItem::new_string(text));
    let _ = window;
    note(t!("Sonuçlar panoya kopyalandı"), cx);
}

// ---------------------------------------------------------------- page

fn button(id: impl Into<ElementId>, p: &Palette, icon: L, label: impl Into<SharedString>, primary: bool, run: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    h_flex()
        .id(id)
        .h(px(32.))
        .px_3()
        .gap_2()
        .items_center()
        .rounded(theme::RADIUS_SM)
        .text_sm()
        .when(primary, |d| d.bg(p.accent).text_color(p.accent_text).hover(|s| s.opacity(0.9)))
        .when(!primary, |d| d.bg(p.hover).border_1().border_color(p.border).hover(|s| s.bg(p.pressed)))
        .active(|s| s.opacity(0.8))
        .child(Icon::new(icon).size(px(14.)))
        .child(label.into())
        .on_click(move |_, window, cx| run(window, cx))
        .into_any_element()
}

fn shell(p: &Palette) -> Div {
    v_flex().w_full().rounded(theme::RADIUS).bg(p.hover).border_1().border_color(p.border)
}

pub fn page(p: &Palette, cx: &mut App) -> Vec<AnyElement> {
    let st = state(cx);
    let running = st.running.is_some();
    let mut out = Vec::new();

    // intro: what it does, on which machine, with which apps
    let machine = st.machine.as_ref().map(machine_line).unwrap_or_default();
    let mut chips = h_flex().gap_2().flex_wrap().child(chip(p, "bench-app-rf", "rustyfiles", true, true, |_| {}));
    let installed: Vec<AppKind> = st.detected.iter().map(|a| a.kind).collect();
    for kind in [AppKind::Explorer, AppKind::Files, AppKind::FilePilot] {
        let here = installed.contains(&kind);
        let on = here && !st.off.contains(&kind);
        let label = if here { app_label(kind).to_string() } else { format!("{} · {}", app_label(kind), t!("kurulu değil")) };
        chips = chips.child(chip(p, ("bench-app", kind as usize), &label, on, !here || running, move |cx| {
            let off = &mut state(cx).off;
            if !off.remove(&kind) {
                off.insert(kind);
            }
        }));
    }
    let action = if running {
        button("bench-stop", p, L::X, t!("Durdur"), false, |_, cx| {
            if let Some((_, c)) = &state(cx).running {
                c.store(true, Ordering::Relaxed);
            }
        })
    } else {
        button("bench-start", p, L::Play, if st.results.is_some() { t!("Yeniden çalıştır") } else { t!("Testi başlat") }, true, |_, cx| start(cx))
    };
    let mut intro = v_flex()
        .p_4()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Icon::new(L::Gauge).size(px(22.)).text_color(p.accent))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(t!("Bu bilgisayarda hız testi")))
                        .child(div().text_xs().text_color(p.text_secondary).child(if machine.is_empty() { t!("Bilgisayar okunuyor…").to_string() } else { machine })),
                )
                .child(action),
        )
        .child(chips)
        .child(div().text_xs().text_color(p.text_secondary).child(t!(
            "Her uygulama aynı klasörleri sırayla açar; süre, işlemci ve bellek ölçülür. Test sırasında diğer uygulamaların pencereleri açılıp kapanır: fareye ve klavyeye dokunmayın. Yaklaşık 5–10 dakika sürer; test dosyaları ilk seferde hazırlanır (~1,3 GB, yerel uygulama verisinde)."
        )));
    if running {
        let (frac, label) = match &st.progress {
            Some(Progress::Step { i, n, label }) => (*i as f32 / (*n).max(1) as f32, format!("{} {i}/{n} · {label}", t!("Adım"))),
            Some(Progress::Preparing(what, f)) => (*f * 0.05, format!("{} {what}", t!("Test dosyaları hazırlanıyor:"))),
            None => (0.0, t!("Başlıyor…").to_string()),
        };
        intro = intro.child(
            v_flex()
                .gap_1p5()
                .child(div().h(px(4.)).w_full().rounded_full().bg(p.divider).child(div().h_full().w(relative(frac.clamp(0.01, 1.0))).rounded_full().bg(p.accent)))
                .child(div().text_xs().text_color(p.text_secondary).child(label)),
        );
    }
    if let Some(e) = &st.error {
        intro = intro.child(div().text_sm().text_color(p.danger).child(format!("{} {e}", t!("Test tamamlanamadı:"))));
    }
    out.push(shell(p).child(intro).into_any_element());

    let Some(r) = st.results.clone() else { return out };
    let race = st.race;
    let elapsed = st.replay.elapsed().as_secs_f64() * 1000.0;
    let note_text = st.note.as_ref().filter(|n| n.1.elapsed() < Duration::from_millis(2500)).map(|n| n.0.clone());
    out.push(race_card(p, &r, race, elapsed));
    out.push(table_card(p, &r));
    if let Some(e) = engines_card(p, &r) {
        out.push(e);
    }
    let star_asked = cx.global::<Settings>().star_asked;
    if !star_asked {
        out.push(star_card(p, &r));
    }
    out.push(share_card(p, note_text));
    out.push(
        h_flex()
            .pt_1()
            .gap_2()
            .text_xs()
            .text_color(p.text_tertiary)
            .child(div().flex_1().min_w_0().child(t!(
                "Ortanca değerler (3 çalıştırma, ısınma turu sayılmaz). Süre: uygulama başlatıldığından pencere içeriği 500 ms boyunca değişmeyene dek. * işaretli değerlerde uygulama zaten çalışıyordu: işlemci pencere açıldıktan sonrasını, bellek tüm süreci gösterir."
            )))
            .child(
                div()
                    .id("bench-delete")
                    .cursor_pointer()
                    .text_color(p.accent)
                    .hover(|s| s.underline())
                    .child(t!("Test dosyalarını sil"))
                    .on_click(|_, _, cx| {
                        std::thread::spawn(|| {
                            let _ = crate::bench::delete_test_files();
                        });
                        state(cx).results = None;
                        cx.refresh_windows();
                    }),
            )
            .into_any_element(),
    );
    out
}

fn chip(p: &Palette, id: impl Into<ElementId>, label: &str, on: bool, locked: bool, toggle: impl Fn(&mut App) + 'static) -> AnyElement {
    h_flex()
        .id(id)
        .h(px(28.))
        .px_3()
        .gap_1p5()
        .items_center()
        .rounded_full()
        .border_1()
        .text_sm()
        .when(on, |d| d.bg(p.selection).border_color(p.accent))
        .when(!on, |d| d.border_color(p.border).text_color(p.text_secondary))
        .when(!locked, |d| d.hover(|s| s.bg(p.pressed)).on_click(move |_, _, cx| {
            toggle(cx);
            cx.refresh_windows();
        }))
        .when(on, |d| d.child(Icon::new(L::Check).size(px(12.)).text_color(p.accent)))
        .child(label.to_string())
        .into_any_element()
}

fn race_card(p: &Palette, r: &Results, race: OpenTest, elapsed: f64) -> AnyElement {
    let mut tabs = h_flex().gap_1();
    for (i, o) in r.open.iter().enumerate() {
        let t = o.test;
        let on = t == race;
        tabs = tabs.child(
            div()
                .id(("bench-tab", i))
                .px_3()
                .h(px(28.))
                .flex()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .text_sm()
                .when(on, |d| d.bg(p.selection).text_color(p.text))
                .when(!on, |d| d.text_color(p.text_secondary).hover(|s| s.bg(p.pressed)))
                .child(test_label(t))
                .on_click(move |_, _, cx| replay(t, cx)),
        );
    }
    let o = r.open.iter().find(|o| o.test == race);
    let rows: Vec<(AppKind, Option<OpenResult>, Option<String>)> =
        o.map(|o| o.apps.iter().map(|a| (a.app, a.median().cloned(), a.error.clone())).collect()).unwrap_or_default();
    let longest = rows.iter().filter_map(|r| r.1.as_ref().map(|m| m.settled_ms)).fold(1.0, f64::max);
    let best = rows.iter().filter_map(|r| r.1.as_ref().map(|m| m.settled_ms)).fold(f64::MAX, f64::min);
    let mut lanes = v_flex().gap_3();
    for (app, m, err) in rows {
        let (track, value): (AnyElement, String) = match (&m, err) {
            (Some(m), _) => {
                let shown = elapsed.min(m.settled_ms);
                let done = elapsed >= m.settled_ms;
                let total_dots = (longest / DOT_MS).ceil().max(1.0);
                let lit = (shown / DOT_MS).ceil();
                let frac = (lit / total_dots).clamp(0.0, 1.0) as f32;
                let color = if app == AppKind::Rustyfiles { p.accent } else { p.text_secondary };
                (
                    div().h(px(8.)).w_full().rounded_full().bg(p.divider).child(div().h_full().w(relative(frac.max(0.005))).rounded_full().bg(color)).into_any_element(),
                    if done { format!("{}{}", ms(m.settled_ms), if m.shared { " *" } else { "" }) } else { t!("Listeleniyor…").to_string() },
                )
            }
            (None, Some(_)) => (div().text_xs().text_color(p.text_tertiary).child(t!("ölçülemedi")).into_any_element(), "—".into()),
            _ => (div().into_any_element(), "—".into()),
        };
        let winner = m.as_ref().is_some_and(|m| m.settled_ms == best && elapsed >= m.settled_ms);
        lanes = lanes.child(
            h_flex()
                .gap_3()
                .items_center()
                .child(div().w(px(120.)).flex_shrink_0().text_sm().child(app_label(app)))
                .child(div().flex_1().min_w_0().child(track))
                .child(
                    div()
                        .w(px(96.))
                        .flex_shrink_0()
                        .text_sm()
                        .text_right()
                        .font_weight(if winner { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                        .text_color(if winner { p.accent } else { p.text })
                        .child(value),
                ),
        );
    }
    shell(p)
        .p_4()
        .gap_4()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(t!("Klasörün açılması, gerçek hızda")))
                .child(button("bench-replay", p, L::RotateCcw, t!("Tekrar oynat"), false, move |_, cx| replay(race, cx))),
        )
        .child(tabs)
        .child(lanes)
        .into_any_element()
}

fn table_card(p: &Palette, r: &Results) -> AnyElement {
    let apps: Vec<AppKind> = r.open.first().map(|o| o.apps.iter().map(|a| a.app).collect()).unwrap_or_default();
    let col = || div().flex_1().min_w(px(110.));
    let mut head = h_flex().px_4().py_2().gap_3().text_xs().text_color(p.text_secondary).child(div().w(px(150.)).flex_shrink_0().child(t!("Test")));
    for a in &apps {
        head = head.child(col().child(app_label(*a)));
    }
    let mut table = v_flex().child(head);
    for o in &r.open {
        let best = o.apps.iter().filter_map(|a| a.median().map(|m| m.settled_ms)).fold(f64::MAX, f64::min);
        let mut row = h_flex().px_4().py_2p5().gap_3().border_t_1().border_color(p.divider).items_start().child(div().w(px(150.)).flex_shrink_0().text_sm().child(test_label(o.test)));
        for a in &o.apps {
            let cell = match (a.median(), &a.error) {
                (Some(m), _) => {
                    let winner = m.settled_ms == best;
                    let mem = if m.private > 0 { mb(m.private) } else { mb(m.ram) };
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(if winner { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
                                .text_color(if winner { p.accent } else { p.text })
                                .child(format!("{}{}{}", ms(m.settled_ms), if m.shared { " *" } else { "" }, if m.timed_out { " ⏱" } else { "" })),
                        )
                        .child(div().text_xs().text_color(p.text_secondary).child(format!("{} {} · {} {}", t!("işlemci"), ms(m.cpu_ms), t!("bellek"), mem)))
                        .into_any_element()
                }
                (None, Some(e)) => div().text_xs().text_color(p.text_tertiary).child(format!("{} ({})", t!("ölçülemedi"), e.chars().take(60).collect::<String>())).into_any_element(),
                _ => div().child("—").into_any_element(),
            };
            row = row.child(col().child(cell));
        }
        table = table.child(row);
    }
    shell(p)
        .child(h_flex().px_4().pt_4().pb_2().child(div().font_weight(FontWeight::SEMIBOLD).child(t!("Ayrıntılar: süre, işlemci ve bellek"))))
        .child(table)
        .into_any_element()
}

fn engines_card(p: &Palette, r: &Results) -> Option<AnyElement> {
    let e = &r.engines;
    let mut rows = Vec::new();
    let mut versus = |label: &str, v: &Option<crate::bench::Versus>, windows_name: &str| {
        let Some(v) = v else { return };
        let (Some(a), Some(b)) = (v.ours_ms, v.windows_ms) else {
            if let Some(a) = v.ours_ms {
                rows.push((label.to_string(), format!("rustyfiles {}", ms(a)), String::new()));
            }
            return;
        };
        // within 10 %: the disk sets the pace for both
        let verdict = if (a - b).abs() <= a.max(b) * 0.1 {
            t!("aynı hızda").to_string()
        } else if b > a {
            t!("{x} daha hızlı").replace("{x}", &times(b, a))
        } else {
            t!("{x} daha yavaş").replace("{x}", &times(a, b))
        };
        rows.push((label.to_string(), format!("rustyfiles {} · {windows_name} {}", ms(a), ms(b)), verdict));
    };
    versus(t!("Kopyalama: 10.000 küçük dosya"), &e.copy_small, "Windows");
    versus(t!("Kopyalama: 1 GB'lık tek dosya"), &e.copy_big, "Windows");
    versus(t!("ZIP çıkarma: 10.000 dosya"), &e.unzip, "Windows");
    if let Some(s) = e.search_ms {
        let rate = e.search_entries as f64 / (s / 1000.0).max(0.001);
        rows.push((t!("Alt klasörlerde arama").to_string(), format!("{} {} · {}", count(e.search_entries), t!("öğe"), ms(s)), t!("saniyede {n} öğe").replace("{n}", &count(rate as u64))));
    }
    if rows.is_empty() {
        return None;
    }
    let mut list = v_flex();
    for (i, (label, detail, verdict)) in rows.into_iter().enumerate() {
        list = list.child(
            h_flex()
                .px_4()
                .py_2p5()
                .gap_3()
                .items_center()
                .when(i > 0, |d| d.border_t_1().border_color(p.divider))
                .child(div().w(px(230.)).flex_shrink_0().text_sm().child(label))
                .child(div().flex_1().min_w_0().text_sm().text_color(p.text_secondary).child(detail))
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(p.accent).child(verdict)),
        );
    }
    Some(
        shell(p)
            .child(h_flex().px_4().pt_4().pb_2().child(div().font_weight(FontWeight::SEMIBOLD).child(t!("rustyfiles motorları ve Windows"))))
            .child(list)
            .into_any_element(),
    )
}

fn star_card(p: &Palette, r: &Results) -> AnyElement {
    let line = match speedup(r, OpenTest::Files100k, AppKind::Explorer) {
        Some((ours, theirs)) if theirs > ours => {
            t!("rustyfiles bu bilgisayarda büyük klasörü Dosya Gezgini'nden {x} hızlı açtı.").replace("{x}", &times(theirs, ours))
        }
        _ => t!("Test bitti.").to_string(),
    };
    shell(p)
        .p_4()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Icon::new(L::Star).size(px(20.)).text_color(p.accent))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(line))
                        .child(div().text_sm().text_color(p.text_secondary).child(t!(
                            "rustyfiles ücretsiz ve tek kişinin emeği. Beğendiysen GitHub'da bir yıldız ver: başkalarının da bulmasına yardım eder."
                        ))),
                )
                .child(button("bench-star", p, L::Star, t!("Yıldız ver"), true, |_, cx| {
                    open_url(REPO.into());
                    settings::update(cx, |s| s.star_asked = true);
                }))
                .child(button("bench-starred", p, L::Check, t!("Verdim"), false, |_, cx| settings::update(cx, |s| s.star_asked = true))),
        )
        .into_any_element()
}

fn share_card(p: &Palette, note_text: Option<String>) -> AnyElement {
    let social = |id: &'static str, label: &'static str, make: fn(&Results) -> String| {
        button(id, p, L::Send, label, false, move |_, cx| {
            if let Some(r) = state(cx).results.clone() {
                open_url(make(&r));
            }
        })
    };
    shell(p)
        .p_4()
        .gap_3()
        .child(
            h_flex()
                .gap_3()
                .items_center()
                .child(Icon::new(L::Share2).size(px(20.)).text_color(p.accent))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(t!("Arkadaşlarına öner")))
                        .child(div().text_sm().text_color(p.text_secondary).child(t!("Sonuç kartını paylaş: bilgisayarının özellikleri ve süreler tek bir görselde."))),
                )
                .children(note_text.map(|n| div().text_sm().text_color(p.accent).child(n))),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(button("bench-copy-card", p, L::Image, t!("Kartı kopyala"), true, |w, cx| copy_picture(w, cx)))
                .child(button("bench-save-card", p, L::Download, t!("Kartı kaydet"), false, |_, cx| save_picture(cx)))
                .child(button("bench-copy-text", p, L::Copy, t!("Metni kopyala"), false, |w, cx| copy_text(w, cx)))
                .child(button("bench-copy-link", p, L::Link, t!("Bağlantıyı kopyala"), false, |_, cx| {
                    if let Some(r) = state(cx).results.clone() {
                        cx.write_to_clipboard(ClipboardItem::new_string(share_link(&r)));
                        note(t!("Bağlantı kopyalandı"), cx);
                    }
                }))
                .child(social("bench-x", "X", |r| format!("https://twitter.com/intent/tweet?text={}&url={}", enc(&headline(r)), enc(&share_link(r)))))
                .child(social("bench-wa", "WhatsApp", |r| format!("https://wa.me/?text={}", enc(&format!("{} {}", headline(r), share_link(r))))))
                .child(social("bench-reddit", "Reddit", |r| format!("https://www.reddit.com/submit?url={}&title={}", enc(&share_link(r)), enc(&headline(r)))))
                .child(social("bench-li", "LinkedIn", |r| format!("https://www.linkedin.com/sharing/share-offsite/?url={}", enc(&share_link(r))))),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    /// Draws the card from the last results on this machine (when there are any) to %TEMP%.
    #[test]
    fn card_renders() {
        let Some(r) = crate::bench::last_results() else { return };
        let pic = rf_win::bench::render_card(&super::card_svg(&r), 1.0).unwrap();
        assert_eq!((pic.width, pic.height), (1200, 675));
        std::fs::write(std::env::temp_dir().join("rf-bench-card.png"), &pic.png).unwrap();
    }

    #[test]
    fn base64url_matches_the_standard() {
        // RFC 4648 test vectors, url alphabet, no padding
        for (i, o) in [("", ""), ("f", "Zg"), ("fo", "Zm8"), ("foo", "Zm9v"), ("foob", "Zm9vYg"), ("fooba", "Zm9vYmE"), ("foobar", "Zm9vYmFy")] {
            assert_eq!(super::b64url(i.as_bytes()), o);
        }
        assert_eq!(super::b64url(&[0xfb, 0xff]), "-_8");
    }

    #[test]
    fn payload_follows_the_site_format() {
        let Some(r) = crate::bench::last_results() else { return };
        let v = super::share_payload(&r);
        assert_eq!(v["v"], 1);
        assert!(v["pc"]["cpu"].is_string() && v["open"].is_array());
        let link = super::share_link(&r);
        assert!(link.starts_with("https://rustyfiles.com/r/") && link.len() < 2900, "{link}");
    }

    #[test]
    fn counts_get_separators() {
        let c = super::count(2_330_750);
        assert!(c == "2.330.750" || c == "2,330,750", "{c}");
        assert_eq!(super::count(999), "999");
    }
}
