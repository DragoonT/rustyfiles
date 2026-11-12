//! Windowless probe: how long a picture for the preview pane takes from "asked for" to "made", through the
//! same ThumbPool the app's ImageCache uses, with the list's own thumbnails queued at the same time.
//!
//! It creates only files of its own in a temp folder (1920 x 1080 JPEGs of a few hundred KB, one 8000 x 6000
//! picture) and removes the folder at the end. No window, no registry, nothing of the user's is read. The
//! shell may write its own thumbnail cache entries for those files, as it does for any file it is asked about.
//!
//!   preview_latency cold            first picture in a fresh process (run it several times)
//!   preview_latency warm [load]     the scenarios below in one process; `load` keeps every core busy meanwhile
//!   preview_latency walk            arrow keys through many files: how many decodes run
//!   preview_latency busy [load=N] [rounds=N]   a busy machine: 60 list tiles queued, then one click, N times (no waiting for the list)
//!   preview_latency coldlist [load=N] [delay=MS]   a fresh process as the Open dialog is one: the folder's 60 tiles are asked for,
//!                                   and the user clicks a picture MS (150) later; nothing in the process is warm
//!   preview_latency spin [N] [secs]  N busy threads for `secs` seconds (30) and nothing else: the load of another program, for a
//!                                   run of this probe started with `class=below` (its process at below-normal priority)
//!   preview_latency iso bg=shell|image|none fg=shell|image [load=N] [rounds=N]
//!                                   which calls block which: a thread in background mode (the pool's workers) works
//!                                   on the list's pictures all the time while a normal thread makes the pane's
//!
//! `RF_URGENT=1` asks for the pane's pictures the way the fixed app does (see `req`).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, unbounded};
use rf_win::thumbs::{Kind, Request, Response, ThumbPool};

const PANE_PX: u32 = 384;
const TILE_PX: u32 = 128;

static KEY: AtomicU64 = AtomicU64::new(1);
/// (RF_STACKS) the request being waited on has been answered
static ANSWERED: AtomicBool = AtomicBool::new(false);

fn urgent_on() -> bool {
    std::env::var_os("RF_URGENT").is_some()
}

/// The one place a request is built, so that the same scenarios run before and after the fix.
fn req(path: &Path, size: u32, pane: bool) -> Request {
    let key = KEY.fetch_add(1, Ordering::Relaxed);
    let mut r = Request::new(key, path.to_path_buf(), size, Kind::Thumbnail);
    if pane && urgent_on() {
        r.urgent = true;
    }
    r
}

struct Done {
    key: u64,
    at: Instant,
    dropped: bool,
    ok: bool,
    waited: Duration,
    took: Duration,
}

struct Harness {
    pool: ThumbPool,
    rx: Receiver<Done>,
}

impl Harness {
    fn new() -> Self {
        let (tx, rx) = unbounded();
        // the same worker count rule as image_cache.rs
        let workers = std::thread::available_parallelism().map(|n| (n.get() / 4).clamp(2, 4)).unwrap_or(2);
        let pool = ThumbPool::new(workers, move |r: Response| {
            let _ = tx.send(Done { key: r.key, at: Instant::now(), dropped: r.dropped, ok: r.image.is_some(), waited: r.waited, took: r.took });
        });
        Self { pool, rx }
    }

    /// Asks and waits for this request's own answer (other answers are handed to `others`).
    fn ask_wait(&self, r: Request, others: &mut Vec<Done>) -> (Duration, Done) {
        let key = r.key;
        let t0 = Instant::now();
        self.pool.request(r);
        if std::env::var_os("RF_STACKS").is_some() {
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                if !ANSWERED.load(Ordering::SeqCst) {
                    dump_waiting_threads("a pane picture has been waited for 1.5 s");
                }
            });
            ANSWERED.store(false, Ordering::SeqCst);
        }
        loop {
            // (a request starved for 30 s is reported as not answered: the run goes on)
            let Ok(d) = self.rx.recv_timeout(Duration::from_secs(30)) else {
                let at = Instant::now();
                return (at - t0, Done { key, at, dropped: false, ok: false, waited: at - t0, took: Duration::ZERO });
            };
            if d.key == key {
                ANSWERED.store(true, Ordering::SeqCst);
                return (d.at - t0, d);
            }
            others.push(d);
        }
    }

    /// Waits until `n` answers of requests other than the ones already counted have come.
    fn drain(&self, n: usize, others: &mut Vec<Done>) {
        while others.len() < n {
            match self.rx.recv_timeout(Duration::from_secs(120)) {
                Ok(d) => others.push(d),
                Err(_) => break,
            }
        }
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn make_jpeg(path: &Path, w: u32, h: u32, phase: u32) {
    use image::codecs::jpeg::JpegEncoder;
    let img = image::RgbImage::from_fn(w, h, |x, y| {
        // smooth gradients and a few soft shapes plus fine noise: a photo-like few hundred KB at 1920 x 1080
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let hash = (x.wrapping_mul(73856093) ^ y.wrapping_mul(19349663) ^ phase.wrapping_mul(83492791)) % 29;
        let wave = ((fx * 9.0 + phase as f32).sin() * (fy * 7.0).cos() * 40.0) as i32;
        let r = (60.0 + 150.0 * fx) as i32 + wave + hash as i32 - 14;
        let g = (40.0 + 160.0 * fy) as i32 - wave + hash as i32 - 14;
        let b = (200.0 - 120.0 * fx * fy) as i32 + wave / 2 + hash as i32 - 14;
        image::Rgb([r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8])
    });
    let f = std::fs::File::create(path).unwrap();
    let mut enc = JpegEncoder::new_with_quality(std::io::BufWriter::new(f), 88);
    enc.encode_image(&img).unwrap();
}

struct Files {
    dir: PathBuf,
    /// the list's own pictures
    list: Vec<PathBuf>,
    /// pictures the pane is asked for (never in `list`, so no scenario finds one already made)
    pane: Vec<PathBuf>,
    big: PathBuf,
}

fn make_files(list_n: usize, pane_n: usize, with_big: bool) -> Files {
    let dir = std::env::temp_dir().join(format!("rf-prevlat-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("base.jpg");
    make_jpeg(&base, 1920, 1080, 3);
    let size = std::fs::metadata(&base).map(|m| m.len()).unwrap_or(0);
    println!("test picture: 1920 x 1080 JPEG, {} KB", size / 1024);
    let mut list = Vec::new();
    for i in 0..list_n {
        let p = dir.join(format!("list{i:03}.jpg"));
        std::fs::copy(&base, &p).unwrap();
        list.push(p);
    }
    let mut pane = Vec::new();
    for i in 0..pane_n {
        let p = dir.join(format!("pane{i:03}.jpg"));
        std::fs::copy(&base, &p).unwrap();
        pane.push(p);
    }
    let big = dir.join("big.jpg");
    if with_big {
        make_jpeg(&big, 8000, 6000, 5);
        println!("big picture: 8000 x 6000 JPEG, {} KB", std::fs::metadata(&big).map(|m| m.len()).unwrap_or(0) / 1024);
    }
    let _ = std::fs::remove_file(&base);
    Files { dir, list, pane, big }
}

/// Keeps every logical processor busy at normal priority until dropped (a busy machine: the browser
/// that asked for the dialog, an antivirus scan, the build running beside).
struct Load(Arc<AtomicBool>);
impl Load {
    fn start(n: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        for _ in 0..n {
            let s = stop.clone();
            std::thread::spawn(move || {
                let mut x = 1u64;
                while !s.load(Ordering::Relaxed) {
                    for _ in 0..10_000 {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    }
                    std::hint::black_box(x);
                }
            });
        }
        Self(stop)
    }
}
impl Drop for Load {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

fn line(name: &str, vals: &[f64]) {
    let mut v = vals.to_vec();
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = v.get(v.len() / 2).copied().unwrap_or(0.0);
    let all: Vec<String> = vals.iter().map(|x| format!("{x:.0}")).collect();
    println!("{name:<58} median {med:>6.0} ms   max {:>6.0} ms   [{}]", v.last().copied().unwrap_or(0.0), all.join(" "));
}

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "warm".into());
    // `load` = every logical processor busy, `load=N` = N busy threads
    let load = std::env::args().find_map(|a| match a.as_str() {
        "load" => Some(std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)),
        _ => a.strip_prefix("load=").and_then(|n| n.parse().ok()),
    });
    println!("mode {mode}, load {load:?} busy threads, urgent {}, {} logical processors", urgent_on(), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    // `class=below|idle`: this process runs at a lower priority class, as the dialog does when the program that opened it was
    // started that way (a child process inherits its parent's class)
    if let Some(c) = std::env::args().find_map(|a| a.strip_prefix("class=").map(str::to_string)) {
        use windows::Win32::System::Threading::{BELOW_NORMAL_PRIORITY_CLASS, GetCurrentProcess, IDLE_PRIORITY_CLASS, SetPriorityClass};
        let class = if c == "idle" { IDLE_PRIORITY_CLASS } else { BELOW_NORMAL_PRIORITY_CLASS };
        unsafe { SetPriorityClass(GetCurrentProcess(), class).ok() };
        println!("process priority class: {c}");
    }
    match mode.as_str() {
        "spin" => {
            let n: usize = std::env::args().nth(2).and_then(|a| a.parse().ok()).unwrap_or(16);
            let secs: u64 = std::env::args().nth(3).and_then(|a| a.parse().ok()).unwrap_or(30);
            let _l = Load::start(n);
            std::thread::sleep(Duration::from_secs(secs));
        }
        "cold" => cold(load),
        "walk" => walk(),
        "prio" => prio(load),
        "busy" => busy(load),
        "iso" => iso(load),
        "coldlist" => coldlist(load),
        "busy2" => busy2(load),
        _ => warm(load),
    }
}

/// The very first picture of a process: GDI+, COM, the thumbnail handlers all start here.
fn cold(load: Option<usize>) {
    let f = make_files(0, 2, false);
    let _l = load.map(Load::start);
    let h = Harness::new();
    let mut others = Vec::new();
    let (t, d) = h.ask_wait(req(&f.pane[0], PANE_PX, true), &mut others);
    println!("cold: first pane picture {:.0} ms (waited {:.0}, made in {:.0}, ok {})", ms(t), ms(d.waited), ms(d.took), d.ok);
    let (t, d) = h.ask_wait(req(&f.pane[1], PANE_PX, true), &mut others);
    println!("cold: second pane picture {:.0} ms (waited {:.0}, made in {:.0}, ok {})", ms(t), ms(d.waited), ms(d.took), d.ok);
    let _ = std::fs::remove_dir_all(&f.dir);
}

fn warm(load: Option<usize>) {
    let f = make_files(300, 40, true);
    let h = Harness::new();
    let mut others = Vec::new();
    let _l = load.map(Load::start);
    // warm the process (the first picture is the cold case, measured by `cold`)
    let _ = h.ask_wait(req(&f.pane[0], PANE_PX, true), &mut others);
    let mut pane = f.pane[1..].iter();

    // 1. nothing else going on
    let mut v = Vec::new();
    for _ in 0..6 {
        let (t, _) = h.ask_wait(req(pane.next().unwrap(), PANE_PX, true), &mut others);
        v.push(ms(t));
    }
    line("1. idle pool, 1920x1080 at 384 px", &v);

    // 2. the list is making its tiles: N pictures asked at the tile size, then the user clicks one
    for (name, n) in [("2. 60 list tiles queued (a window of tiles)", 60usize), ("3. 300 list tiles queued (scrolled a lot)", 300)] {
        let mut v = Vec::new();
        let mut list_total = Vec::new();
        for round in 0..3 {
            others.clear();
            let t0 = Instant::now();
            let mut asked = 0usize;
            for p in f.list.iter().skip(round * 7).take(n) {
                h.pool.request(req(p, TILE_PX, false));
                asked += 1;
            }
            // the user clicks a picture shortly after the folder appeared
            std::thread::sleep(Duration::from_millis(150));
            let (t, d) = h.ask_wait(req(pane.next().unwrap(), PANE_PX, true), &mut others);
            v.push(ms(t));
            let _ = d;
            h.drain(asked, &mut others);
            list_total.push(ms(t0.elapsed()));
            // let the pool settle and the list's keys be new for the next round
            std::thread::sleep(Duration::from_millis(100));
        }
        line(&format!("{name}: picture in pane"), &v);
        line(&format!("{name}: whole list batch done"), &list_total);
    }

    // 4. one big picture
    others.clear();
    let (t, d) = h.ask_wait(req(&f.big, PANE_PX, true), &mut others);
    println!("4. big 8000x6000 picture alone, 384 px: {:.0} ms (made in {:.0})", ms(t), ms(d.took));
    // ... with the list busy
    others.clear();
    for p in f.list.iter().take(60) {
        h.pool.request(req(p, TILE_PX + 8, false)); // another size: no earlier answer is reused
    }
    std::thread::sleep(Duration::from_millis(150));
    let big2 = f.dir.join("big2.jpg");
    std::fs::copy(&f.big, &big2).unwrap();
    let (t, d) = h.ask_wait(req(&big2, PANE_PX, true), &mut others);
    println!("5. big picture with 60 list tiles queued: {:.0} ms (waited {:.0}, made in {:.0})", ms(t), ms(d.waited), ms(d.took));
    h.drain(60, &mut others);

    // 6. the same file at the sizes the pane and Quick Look ask for
    let mut v = Vec::new();
    for px in [256u32, 384, 768, 1280] {
        let p = pane.next().unwrap();
        let (t, _) = h.ask_wait(req(p, px, true), &mut others);
        v.push(ms(t));
    }
    line("6. idle pool, pane at 256 / 384 / 768 / 1280 px", &v);
    drop(_l);
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// A busy machine (`load=N` threads spinning at normal priority): the pane's picture while the list's tiles are queued.
/// Unlike `warm` it never waits for the list to finish, which on a starved pool takes minutes.
fn busy(load: Option<usize>) {
    // `rounds=N`: how many times the click is repeated (stalls are rare: report the maximum, not only the median)
    let rounds: usize = std::env::args().find_map(|a| a.strip_prefix("rounds=").and_then(|n| n.parse().ok())).unwrap_or(4);
    let f = make_files(240, rounds + 4, false);
    // `age=SECS`: let the files settle first (an antivirus scans files that were just written)
    if let Some(secs) = std::env::args().find_map(|a| a.strip_prefix("age=").and_then(|n| n.parse::<u64>().ok())) {
        std::thread::sleep(Duration::from_secs(secs));
    }
    let h = Harness::new();
    let mut others = Vec::new();
    // one picture first, with nothing busy: the process is warm for what follows
    let _ = h.ask_wait(req(&f.pane[0], PANE_PX, true), &mut others);
    let _l = load.map(Load::start);
    let mut v = Vec::new();
    for round in 0..rounds {
        for p in f.list.iter().cycle().skip(round * 60 % 240).take(60) {
            h.pool.request(req(p, TILE_PX, false));
        }
        std::thread::sleep(Duration::from_millis(150));
        let (t, d) = h.ask_wait(req(&f.pane[1 + round], PANE_PX, true), &mut others);
        if ms(t) > 1000.0 || rounds <= 4 {
            println!("  round {round}: pane picture {:.0} ms (waited {:.0}, made in {:.0}, ok {})", ms(t), ms(d.waited), ms(d.took), d.ok);
        }
        v.push(ms(t));
    }
    line("busy machine, 60 list tiles queued: picture in pane", &v);
    drop(_l);
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// The same file made on a plain thread and on a thread in background mode (what the pool's workers are), one after
/// the other and alternating, with no queue at all: only the thread's priority differs.
fn prio(load: Option<usize>) {
    use rf_win::thumbs::get_image;
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_PRIORITY_BELOW_NORMAL};
    let f = make_files(0, 24, true);
    let _l = load.map(Load::start);
    // (a thread starved for 30 s is reported as 30000: the run goes on without waiting for it)
    let run = |path: PathBuf, size: u32, background: bool| -> f64 {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            if background {
                unsafe {
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
                    let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
                }
            }
            let t = Instant::now();
            let ok = get_image(&path, size, Kind::Thumbnail).is_ok();
            assert!(ok);
            let _ = tx.send(ms(t.elapsed()));
        });
        rx.recv_timeout(Duration::from_secs(30)).unwrap_or(30000.0)
    };
    let _ = run(f.pane[0].clone(), PANE_PX, false); // warm-up
    let (mut normal, mut background) = (Vec::new(), Vec::new());
    let pairs = if load.is_some() { 3 } else { 10 };
    for i in 0..pairs {
        normal.push(run(f.pane[1 + 2 * i].clone(), PANE_PX, false));
        background.push(run(f.pane[2 + 2 * i].clone(), PANE_PX, true));
    }
    line("1920x1080 at 384 px, normal-priority thread", &normal);
    line("1920x1080 at 384 px, background-mode thread (pool worker)", &background);
    let (mut normal, mut background) = (Vec::new(), Vec::new());
    for i in 0..if load.is_some() { 1 } else { 3 } {
        let a = f.dir.join(format!("bigA{i}.jpg"));
        let b = f.dir.join(format!("bigB{i}.jpg"));
        std::fs::copy(&f.big, &a).unwrap();
        std::fs::copy(&f.big, &b).unwrap();
        normal.push(run(a, PANE_PX, false));
        background.push(run(b, PANE_PX, true));
    }
    line("8000x6000 at 384 px, normal-priority thread", &normal);
    line("8000x6000 at 384 px, background-mode thread (pool worker)", &background);
    drop(_l);
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// Arrow keys through many files at 30 ms: every file asks for its picture; how many are really made?
fn walk() {
    // `gap=N`: milliseconds between two files (the keyboard repeat is about 30)
    let gap: u64 = std::env::args().find_map(|a| a.strip_prefix("gap=").and_then(|n| n.parse().ok())).unwrap_or(30);
    let f = make_files(0, 40, false);
    let h = Harness::new();
    let mut others = Vec::new();
    let _ = h.ask_wait(req(&f.pane[0], PANE_PX, true), &mut others);
    others.clear();
    let t0 = Instant::now();
    let mut keys = Vec::new();
    for p in f.pane.iter().skip(1).take(30) {
        let r = req(p, PANE_PX, true);
        keys.push(r.key);
        h.pool.request(r);
        std::thread::sleep(Duration::from_millis(gap));
    }
    let last = *keys.last().unwrap();
    // the last file is the one the user stopped on: when does its picture come?
    let mut last_at = None;
    let mut seen = 0usize;
    let mut made = 0usize;
    let mut dropped = 0usize;
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match h.rx.recv_timeout(Duration::from_millis(500)) {
            Ok(d) => {
                seen += 1;
                if d.dropped {
                    dropped += 1;
                } else if d.ok {
                    made += 1;
                }
                if d.key == last {
                    last_at = Some(d.at - t0);
                }
            }
            Err(_) => {
                if seen >= keys.len() {
                    break;
                }
            }
        }
    }
    println!(
        "walk: 30 files asked {gap} ms apart; answers {seen}, pictures made {made}, dropped {dropped}; the last file's picture {}",
        last_at.map_or("never".to_string(), |d| format!("{:.0} ms after the first ask", ms(d)))
    );
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// One picture made the way `kind` says: `shell` is the pool's path (IShellItemImageFactory), `image` is a plain
/// decode of the file by the `image` crate (no shell, no COM, no GDI+).
fn make_one(kind: &str, path: &Path, size: u32) -> bool {
    match kind {
        "shell" => rf_win::thumbs::get_image(path, size, Kind::Thumbnail).is_ok(),
        "wic" => rf_win::pane_picture::decode(path, size).is_ok(),
        _ => image::open(path).map(|i| i.thumbnail(size, size)).is_ok(),
    }
}

/// Which calls block which. A thread in background mode makes the list's pictures all the time (so that it is
/// always inside a call, holding whatever that call holds) while a thread at normal priority makes the pane's
/// picture, round after round; the normal thread's slowest round is what tells whether the two share a lock.
fn iso(load: Option<usize>) {
    let arg = |k: &str, d: &str| std::env::args().find_map(|a| a.strip_prefix(k).map(str::to_string)).unwrap_or_else(|| d.to_string());
    let (bg, fg) = (arg("bg=", "shell"), arg("fg=", "shell"));
    let rounds: usize = arg("rounds=", "20").parse().unwrap_or(20);
    let f = make_files(120, rounds + 2, false);
    let _ = make_one(&fg, &f.pane[0], PANE_PX);
    let stop = Arc::new(AtomicBool::new(false));
    let list = f.list.clone();
    // `bgn=N` background threads; `late` starts them after the load, as the pool's workers start when the list asks
    let bgn: usize = arg("bgn=", "1").parse().unwrap_or(1);
    let late = std::env::args().any(|a| a == "late");
    let spawn_bg = |list: Vec<PathBuf>, stop: Arc<AtomicBool>, bg: String, from: usize| {
        std::thread::spawn(move || {
            use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_PRIORITY_BELOW_NORMAL};
            unsafe {
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN);
                let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
            }
            for p in list.iter().cycle().skip(from) {
                if stop.load(Ordering::Relaxed) {
                    break;
                }
                let _ = make_one(&bg, p, TILE_PX);
            }
        })
    };
    let mut bg_threads = Vec::new();
    if bg != "none" && !late {
        for k in 0..bgn {
            bg_threads.push(spawn_bg(list.clone(), stop.clone(), bg.clone(), k * 20));
        }
    }
    let _l = load.map(Load::start);
    if bg != "none" && late {
        for k in 0..bgn {
            bg_threads.push(spawn_bg(list.clone(), stop.clone(), bg.clone(), k * 20));
        }
    }
    if !late {
        std::thread::sleep(Duration::from_millis(300));
    }
    let mut v = Vec::new();
    for i in 0..rounds {
        let t = Instant::now();
        let ok = make_one(&fg, &f.pane[1 + i], PANE_PX);
        assert!(ok);
        v.push(ms(t.elapsed()));
    }
    line(&format!("iso: {bgn} background thread(s) make {bg}{}, normal thread makes {fg}", if late { " (started after the load)" } else { "" }), &v);
    stop.store(true, Ordering::Relaxed);
    drop(_l);
    for t in bg_threads {
        let _ = t.join();
    }
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// Where every thread of this process stands (the innermost frames), for a stall that is being waited on: the
/// threads are held for a moment, only their registers and stacks are read without allocating (a held thread may
/// own the heap lock), and the addresses are turned into `module!export+offset` after they are let go.
mod stacks {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Diagnostics::Debug::{CONTEXT, CONTEXT_FULL_AMD64, RtlLookupFunctionEntry};
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next};
    use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
    use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_QUERY_LIMITED_INFORMATION, THREAD_SUSPEND_RESUME, GetThreadDescription};
    use windows::core::PCWSTR;

    // (declared here: the windows crate gates these two behind its Win32_System_Kernel feature, which the workspace does not enable)
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetThreadContext(thread: HANDLE, context: *mut CONTEXT) -> i32;
        fn RtlVirtualUnwind(
            handler_type: u32,
            image_base: u64,
            control_pc: u64,
            function_entry: *const std::ffi::c_void,
            context: *mut CONTEXT,
            handler_data: *mut *mut std::ffi::c_void,
            establisher_frame: *mut u64,
            context_pointers: *mut std::ffi::c_void,
        ) -> *mut std::ffi::c_void;
    }
    const UNW_FLAG_NHANDLER: u32 = 0;

    #[repr(C, align(16))]
    struct Ctx(CONTEXT);

    pub struct Snap {
        pub tid: u32,
        pub name: String,
        pub frames: Vec<usize>,
    }

    pub fn capture(max_frames: usize) -> Vec<Snap> {
        let (pid, me) = unsafe { (GetCurrentProcessId(), GetCurrentThreadId()) };
        let mut tids = Vec::new();
        unsafe {
            let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return Vec::new() };
            let mut e = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
            if Thread32First(snap, &mut e).is_ok() {
                loop {
                    if e.th32OwnerProcessID == pid && e.th32ThreadID != me {
                        tids.push(e.th32ThreadID);
                    }
                    if Thread32Next(snap, &mut e).is_err() {
                        break;
                    }
                }
            }
            let _ = CloseHandle(snap);
        }
        let handles: Vec<(u32, HANDLE)> = tids.iter().filter_map(|&t| unsafe { OpenThread(THREAD_GET_CONTEXT | THREAD_SUSPEND_RESUME | THREAD_QUERY_LIMITED_INFORMATION, false, t).ok().map(|h| (t, h)) }).collect();
        // no allocation between the first suspend and the last resume: the frames go into storage made before
        let mut out: Vec<Snap> = handles
            .iter()
            .map(|&(tid, h)| Snap {
                tid,
                name: unsafe { GetThreadDescription(h).ok().map(|p| p.to_string().unwrap_or_default()).unwrap_or_default() },
                frames: Vec::with_capacity(max_frames),
            })
            .collect();
        unsafe {
            for &(_, h) in &handles {
                SuspendThread(h);
            }
            for (i, &(_, h)) in handles.iter().enumerate() {
                let mut c = Ctx(CONTEXT::default());
                c.0.ContextFlags = CONTEXT_FULL_AMD64;
                if GetThreadContext(h, &mut c.0) == 0 {
                    continue;
                }
                for _ in 0..max_frames {
                    let rip = c.0.Rip as usize;
                    if rip == 0 {
                        break;
                    }
                    out[i].frames.push(rip);
                    let mut base = 0u64;
                    let rf = RtlLookupFunctionEntry(c.0.Rip, &mut base, None);
                    if rf.is_null() {
                        // a leaf without unwind data: the return address is at the top of the stack
                        let sp = c.0.Rsp as *const u64;
                        c.0.Rip = *sp;
                        c.0.Rsp += 8;
                        continue;
                    }
                    let (mut handler, mut frame) = (std::ptr::null_mut(), 0u64);
                    RtlVirtualUnwind(UNW_FLAG_NHANDLER, base, c.0.Rip, rf as *const _, &mut c.0, &mut handler, &mut frame, std::ptr::null_mut());
                }
            }
            for &(_, h) in &handles {
                ResumeThread(h);
            }
            for &(_, h) in &handles {
                let _ = CloseHandle(h);
            }
        }
        out
    }

    /// `module!nearest export+offset` for an address (the export table only: no symbol files, nothing is fetched).
    pub fn name(addr: usize) -> String {
        use windows::Win32::System::Diagnostics::Debug::{SYMBOL_INFO, SYMOPT_DEFERRED_LOADS, SYMOPT_EXACT_SYMBOLS, SymFromAddr, SymInitialize, SymSetOptions};
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| unsafe {
            SymSetOptions(SYMOPT_DEFERRED_LOADS | SYMOPT_EXACT_SYMBOLS);
            let _ = SymInitialize(windows::Win32::System::Threading::GetCurrentProcess(), windows::core::PCSTR::null(), true);
        });
        unsafe {
            let mut buf = [0u8; std::mem::size_of::<SYMBOL_INFO>() + 256];
            let info = buf.as_mut_ptr() as *mut SYMBOL_INFO;
            (*info).SizeOfStruct = std::mem::size_of::<SYMBOL_INFO>() as u32;
            (*info).MaxNameLen = 255;
            let mut disp = 0u64;
            if SymFromAddr(windows::Win32::System::Threading::GetCurrentProcess(), addr as u64, Some(&mut disp), info).is_ok() {
                let n = std::slice::from_raw_parts((*info).Name.as_ptr() as *const u8, (*info).NameLen as usize);
                let mut m = Default::default();
                let module = if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(addr as *const u16), &mut m).is_ok() {
                    let mut b = [0u16; 260];
                    let k = GetModuleFileNameW(Some(m), &mut b) as usize;
                    String::from_utf16_lossy(&b[..k]).rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or("?").to_string()
                } else {
                    "?".to_string()
                };
                return format!("{module}!{}+{disp:x}", String::from_utf8_lossy(n));
            }
        }
        name_by_module(addr)
    }

    fn name_by_module(addr: usize) -> String {
        unsafe {
            let mut m = Default::default();
            if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(addr as *const u16), &mut m).is_err() {
                return format!("?{addr:x}");
            }
            let mut buf = [0u16; 260];
            let n = GetModuleFileNameW(Some(m), &mut buf) as usize;
            let path = String::from_utf16_lossy(&buf[..n]);
            let file = path.rsplit(std::path::MAIN_SEPARATOR).next().unwrap_or("?").to_string();
            format!("{file}+{:x}", addr - m.0 as usize)
        }
    }
}

/// Prints the stacks of the threads that are not busy in this program's own code (the spinners), i.e. the ones that wait.
fn dump_waiting_threads(why: &str) {
    let exe = stacks::name(dump_waiting_threads as *const () as usize);
    let exe_module = exe.split('+').next().unwrap_or("").to_string();
    println!("--- stacks ({why})");
    for s in stacks::capture(40) {
        let names: Vec<String> = s.frames.iter().map(|&a| stacks::name(a)).collect();
        if names.first().is_some_and(|n| n.starts_with(&exe_module)) {
            continue;
        }
        println!("thread {} [{}]: {}", s.tid, s.name, names.join(" < "));
    }
}

/// The Open dialog is a process of its own that has just started: no thread has made a picture yet when the folder's
/// tiles are asked for and the user clicks one.
fn coldlist(load: Option<usize>) {
    let delay: u64 = std::env::args().find_map(|a| a.strip_prefix("delay=").and_then(|n| n.parse().ok())).unwrap_or(150);
    let f = make_files(60, 3, false);
    let _l = load.map(Load::start);
    let h = Harness::new();
    let mut others = Vec::new();
    for p in f.list.iter() {
        h.pool.request(req(p, TILE_PX, false));
    }
    std::thread::sleep(Duration::from_millis(delay));
    let (t, d) = h.ask_wait(req(&f.pane[0], PANE_PX, true), &mut others);
    println!("coldlist: pane picture {:.0} ms (waited {:.0}, made in {:.0}, ok {})", ms(t), ms(d.waited), ms(d.took), d.ok);
    let (t, d) = h.ask_wait(req(&f.pane[1], PANE_PX, true), &mut others);
    println!("coldlist: second pane picture {:.0} ms (waited {:.0}, made in {:.0}, ok {})", ms(t), ms(d.waited), ms(d.took), d.ok);
    drop(_l);
    let _ = std::fs::remove_dir_all(&f.dir);
}

/// Like `busy` (the list's pool at work on 60 tiles through the shell, every core busy) but the pane's picture is made
/// by a thread of its own that stands in for the lane, with the call named by `pane=shell|wic|image`.
/// `fresh` starts that thread new for every picture, `cold` makes the process cold: nothing is warmed first.
fn busy2(load: Option<usize>) {
    let arg = |k: &str, d: &str| std::env::args().find_map(|a| a.strip_prefix(k).map(str::to_string)).unwrap_or_else(|| d.to_string());
    let kind = arg("pane=", "shell");
    let rounds: usize = arg("rounds=", "3").parse().unwrap_or(3);
    let cold = std::env::args().any(|a| a == "cold");
    let f = make_files(240, rounds + 4, false);
    let h = Harness::new();
    let (job_tx, job_rx) = std::sync::mpsc::channel::<(PathBuf, std::sync::mpsc::Sender<(f64, bool)>)>();
    let k2 = kind.clone();
    std::thread::spawn(move || {
        while let Ok((p, back)) = job_rx.recv() {
            let t = Instant::now();
            let ok = make_one(&k2, &p, PANE_PX);
            let _ = back.send((ms(t.elapsed()), ok));
        }
    });
    let ask = |p: &Path| -> (f64, bool) {
        let (tx, rx) = std::sync::mpsc::channel();
        let t = Instant::now();
        job_tx.send((p.to_path_buf(), tx)).unwrap();
        let r = rx.recv_timeout(Duration::from_secs(60)).unwrap_or((60000.0, false));
        (ms(t.elapsed()).max(r.0), r.1)
    };
    if !cold {
        let _ = ask(&f.pane[0]);
    }
    let _l = load.map(Load::start);
    let mut v = Vec::new();
    for round in 0..rounds {
        for p in f.list.iter().cycle().skip(round * 60 % 240).take(60) {
            h.pool.request(req(p, TILE_PX, false));
        }
        std::thread::sleep(Duration::from_millis(150));
        let (t, ok) = ask(&f.pane[1 + round]);
        assert!(ok, "the {kind} call failed");
        v.push(t);
    }
    line(&format!("busy2: pane by {kind}{}", if cold { " (cold)" } else { "" }), &v);
    drop(_l);
    let _ = std::fs::remove_dir_all(&f.dir);
}
