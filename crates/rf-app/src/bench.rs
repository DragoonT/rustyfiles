//! The speed test in Settings: rustyfiles against the other file managers on this machine, and
//! its own engines against Windows'. Blocking: run [`run`] on a background thread.
//!
//! Test folders live in `<app dir>/bench` and are kept between runs (making 100 000 files takes a
//! while); copies and extractions go to a scratch folder that is emptied after each run.
//! rustyfiles is measured as a fresh process with its own settings folder, the way the others
//! start: the user's settings are neither read nor changed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use rf_win::bench::{App, AppKind, Machine, OpenResult};

/// Each app opens each folder this many times (after one warm-up run that isn't counted).
pub const RUNS: usize = 3;
const OPEN_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OpenTest {
    /// An empty folder: how long the app takes to start.
    Start,
    Files1k,
    Files10k,
    Files100k,
    /// 2 000 pictures: thumbnails.
    Images,
}

impl OpenTest {
    pub const ALL: [OpenTest; 5] = [OpenTest::Start, OpenTest::Files1k, OpenTest::Files10k, OpenTest::Files100k, OpenTest::Images];

    /// The test folder's name (also what the window titles are matched by).
    pub fn folder(self) -> &'static str {
        match self {
            OpenTest::Start => "rf-bench empty",
            OpenTest::Files1k => "rf-bench 1000",
            OpenTest::Files10k => "rf-bench 10000",
            OpenTest::Files100k => "rf-bench 100000",
            OpenTest::Images => "rf-bench images",
        }
    }
}

/// What to run.
#[derive(Debug, Clone)]
pub struct Plan {
    /// rustyfiles first, then the apps to compare with.
    pub apps: Vec<App>,
    pub open: Vec<OpenTest>,
    pub copy: bool,
    pub engines: bool,
}

/// One app's runs of one open test.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppRuns {
    pub app: AppKind,
    pub runs: Vec<OpenResult>,
    /// Why it couldn't be measured, if it couldn't.
    pub error: Option<String>,
}

impl AppRuns {
    /// The run with the median settle time.
    pub fn median(&self) -> Option<&OpenResult> {
        let mut v: Vec<&OpenResult> = self.runs.iter().collect();
        v.sort_by(|a, b| a.settled_ms.total_cmp(&b.settled_ms));
        v.get(v.len() / 2).copied()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct OpenResults {
    pub test: OpenTest,
    pub apps: Vec<AppRuns>,
}

/// Ours against Windows' engine, in ms (median of the runs); `None` when it failed.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Versus {
    pub ours_ms: Option<f64>,
    pub windows_ms: Option<f64>,
    pub bytes: u64,
    pub files: u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Engines {
    /// 10 000 small files in 100 folders.
    pub copy_small: Option<Versus>,
    /// One 1 GB file.
    pub copy_big: Option<Versus>,
    /// Extracting a ZIP of 10 000 files: rarchive against Windows' compressed folders.
    pub unzip: Option<Versus>,
    /// Searching the test folders by name (ms, entries looked at).
    pub search_ms: Option<f64>,
    pub search_entries: u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Results {
    /// Unix ms.
    pub when: i64,
    pub version: String,
    pub machine: Machine,
    pub open: Vec<OpenResults>,
    pub engines: Engines,
}

/// Progress for the UI.
#[derive(Debug, Clone)]
pub enum Progress {
    /// Making the test folders (what, done 0..1).
    Preparing(String, f32),
    /// Step `i` of `n`.
    Step { i: usize, n: usize, label: String },
}

/// `%LOCALAPPDATA%/rustyfiles/bench`: local, never the roaming settings folder (a roaming
/// profile would carry 100 000 test files around).
pub fn bench_dir() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map_or_else(std::env::temp_dir, PathBuf::from).join("rustyfiles").join("bench")
}

/// rustyfiles itself, started fresh with its own settings folder.
pub fn rustyfiles_app() -> App {
    let exe = std::env::current_exe().map(|p| p.with_file_name("rustyfiles.exe")).unwrap_or_default();
    let cfg = bench_dir().join("rustyfiles-settings");
    App {
        kind: AppKind::Rustyfiles,
        exe,
        env: vec![
            ("RUSTYFILES_MULTI_INSTANCE".into(), "1".into()),
            ("RUSTYFILES_CONFIG_DIR".into(), cfg.to_string_lossy().into_owned()),
            ("RUSTYFILES_MUTE".into(), "1".into()),
        ],
    }
}

// ------------------------------------------------------------------ test folders

const EXTS: [&str; 10] = ["txt", "pdf", "docx", "zip", "mp3", "rs", "md", "json", "csv", "log"];

fn make_files(dir: &Path, n: usize, report: &dyn Fn(f32)) -> std::io::Result<()> {
    let done = dir.join(".rf-bench-done");
    if done.exists() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let body = vec![b'x'; 600];
    for i in 0..n {
        // small enough to stay inside the file record: no clusters used
        let len = (i * 37) % 600;
        std::fs::write(dir.join(format!("file {i:06}.{}", EXTS[i % EXTS.len()])), &body[..len])?;
        if i % 2000 == 0 {
            report(i as f32 / n as f32);
        }
    }
    hide(&done)?;
    Ok(())
}

/// The "done" marker, hidden so it doesn't show in the listings.
fn hide(p: &Path) -> std::io::Result<()> {
    std::fs::write(p, b"")?;
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    unsafe {
        let _ = windows::Win32::Storage::FileSystem::SetFileAttributesW(
            windows::core::PCWSTR(w.as_ptr()),
            windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_HIDDEN,
        );
    }
    Ok(())
}

fn make_images(dir: &Path, n: usize, report: &dyn Fn(f32)) -> anyhow::Result<()> {
    let done = dir.join(".rf-bench-done");
    if done.exists() {
        return Ok(());
    }
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    for i in 0..n {
        let (w, h) = (320u32, 240u32);
        let hue = (i * 47 % 360) as f32;
        let img = image::RgbImage::from_fn(w, h, |x, y| {
            let t = (x + y) as f32 / (w + h) as f32;
            let c = |o: f32| (((hue + o) / 360.0 * std::f32::consts::TAU).sin() * 0.5 + 0.5) * 200.0 * (0.5 + t * 0.5) + 30.0;
            image::Rgb([c(0.0) as u8, c(120.0) as u8, c(240.0) as u8])
        });
        img.save(dir.join(format!("picture {i:04}.png")))?;
        if i % 50 == 0 {
            report(i as f32 / n as f32);
        }
    }
    hide(&done)?;
    Ok(())
}

/// 10 000 files of 4 KB in 100 folders, and one 1 GB file.
fn make_copy_sources(root: &Path, report: &dyn Fn(f32)) -> std::io::Result<(PathBuf, PathBuf)> {
    let small = root.join("copy small");
    let big = root.join("copy big.bin");
    let done = root.join(".rf-bench-copy-done");
    if done.exists() && big.exists() {
        return Ok((small, big));
    }
    let _ = std::fs::remove_dir_all(&small);
    for d in 0..100 {
        let dir = small.join(format!("folder {d:03}"));
        std::fs::create_dir_all(&dir)?;
        for f in 0..100u32 {
            let body: Vec<u8> = (0..4096u32).map(|i| ((i + f * 7 + d) % 251) as u8).collect();
            std::fs::write(dir.join(format!("file {f:03}.dat")), body)?;
        }
        report(d as f32 / 200.0);
    }
    use std::io::Write;
    let chunk: Vec<u8> = (0..(4u32 << 20)).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
    let mut f = std::io::BufWriter::new(std::fs::File::create(&big)?);
    for k in 0..256 {
        f.write_all(&chunk)?;
        if k % 16 == 0 {
            report(0.5 + k as f32 / 512.0);
        }
    }
    f.flush()?;
    hide(&done)?;
    Ok((small, big))
}

/// Writes the benchmark instance's settings: nothing that would stay running or reach out.
fn prepare_rustyfiles_settings() -> std::io::Result<()> {
    let cfg = bench_dir().join("rustyfiles-settings");
    std::fs::create_dir_all(&cfg)?;
    std::fs::write(
        cfg.join("settings.toml"),
        "restore_session = false\nrun_in_background = false\nauto_update = false\nstar_asked = true\nstarts = 1\n",
    )
}

/// Removes the test folders.
pub fn delete_test_files() -> std::io::Result<()> {
    let d = bench_dir();
    if d.exists() { std::fs::remove_dir_all(d) } else { Ok(()) }
}

fn free_space(p: &Path) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    let w: Vec<u16> = p.as_os_str().encode_wide().chain([0]).collect();
    let mut free = 0u64;
    unsafe {
        let _ = windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(windows::core::PCWSTR(w.as_ptr()), Some(&mut free), None, None);
    }
    free
}

// ------------------------------------------------------------------ running

fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let n = v.len();
    // an even count averages the middle two (taking v[n / 2] of two runs took the slower one)
    Some(if n % 2 == 0 { (v[n / 2 - 1] + v[n / 2]) / 2.0 } else { v[n / 2] })
}

fn time<T>(f: impl FnOnce() -> anyhow::Result<T>) -> anyhow::Result<f64> {
    let t = Instant::now();
    f()?;
    Ok(t.elapsed().as_secs_f64() * 1000.0)
}

/// Our engine exactly as a paste uses it (thread count chosen for the disks).
fn our_copy(sources: &[PathBuf], dest: &Path) -> anyhow::Result<()> {
    let tracker = Arc::new(rf_win::fileops::Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))));
    anyhow::ensure!(rf_win::fastcopy::transfer(rf_win::fileops::Transfer::Copy, sources, dest, tracker)?, "the copy stopped");
    Ok(())
}

fn versus(runs: usize, scratch: &Path, ours: &dyn Fn(&Path) -> anyhow::Result<()>, windows: &dyn Fn(&Path) -> anyhow::Result<()>, cancel: &AtomicBool) -> Versus {
    let (mut a, mut b) = (Vec::new(), Vec::new());
    for r in 0..runs {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        for (which, f) in [(0, ours), (1, windows)] {
            let dest = scratch.join(format!("run {r} {which}"));
            let _ = std::fs::remove_dir_all(&dest);
            if std::fs::create_dir_all(&dest).is_err() {
                continue;
            }
            match time(|| f(&dest)) {
                Ok(ms) => [&mut a, &mut b][which].push(ms),
                Err(e) => log::warn!("bench: {} failed: {e:#}", if which == 0 { "ours" } else { "windows" }),
            }
            let _ = std::fs::remove_dir_all(&dest);
        }
        // a side that takes this long is measured once (Windows' ZIP and small-file copy run
        // for half a minute each)
        if a.iter().chain(&b).any(|&ms| ms > 8000.0) {
            break;
        }
    }
    Versus { ours_ms: median(a), windows_ms: median(b), ..Default::default() }
}

/// Runs `plan`. `progress` is called from this thread; `cancel` stops between steps.
pub fn run(plan: &Plan, progress: &dyn Fn(Progress), cancel: &AtomicBool) -> anyhow::Result<Results> {
    let root = bench_dir();
    std::fs::create_dir_all(&root)?;
    prepare_rustyfiles_settings()?;
    let machine = rf_win::bench::machine(&root);

    // test folders
    for t in &plan.open {
        let dir = root.join(t.folder());
        let label = t.folder().to_string();
        let report = |f: f32| progress(Progress::Preparing(label.clone(), f));
        match t {
            OpenTest::Start => std::fs::create_dir_all(&dir)?,
            OpenTest::Files1k => make_files(&dir, 1_000, &report)?,
            OpenTest::Files10k => make_files(&dir, 10_000, &report)?,
            OpenTest::Files100k => make_files(&dir, 100_000, &report)?,
            OpenTest::Images => make_images(&dir, 2_000, &report)?,
        }
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("cancelled");
        }
    }
    let copy_ok = plan.copy && free_space(&root) > 4 << 30;
    let copy_src = if copy_ok || plan.engines {
        Some(make_copy_sources(&root, &|f| progress(Progress::Preparing("copy".into(), f)))?)
    } else {
        None
    };

    let n_open = plan.open.len() * plan.apps.len() * (RUNS + 1);
    let n = n_open + if copy_ok { 2 } else { 0 } + if plan.engines { 2 } else { 0 };
    let mut i = 0usize;
    let mut step = |label: String| {
        i += 1;
        progress(Progress::Step { i, n, label });
    };

    let mut open = Vec::new();
    for &t in &plan.open {
        let dir = root.join(t.folder());
        let mut per_app: Vec<AppRuns> = plan.apps.iter().map(|a| AppRuns { app: a.kind, runs: Vec::new(), error: None }).collect();
        // run 0 warms up (caches, the app's own first start); apps take turns each round
        for r in 0..=RUNS {
            for (k, app) in plan.apps.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    anyhow::bail!("cancelled");
                }
                step(format!("{} · {}", app.kind.name(), t.folder()));
                if per_app[k].error.is_some() {
                    continue;
                }
                match rf_win::bench::measure_open(app, &dir, OPEN_TIMEOUT, cancel) {
                    Ok(res) if r > 0 => per_app[k].runs.push(res),
                    Ok(_) => {}
                    Err(e) => {
                        log::warn!("bench: {} on {}: {e:#}", app.kind.name(), t.folder());
                        per_app[k].error = Some(format!("{e:#}"));
                    }
                }
                // let the closed window's process settle before the next start
                std::thread::sleep(Duration::from_millis(400));
            }
        }
        open.push(OpenResults { test: t, apps: per_app });
    }

    let mut engines = Engines::default();
    let scratch = root.join("scratch");
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch)?;
    if let (true, Some((small, big))) = (copy_ok, &copy_src) {
        step("copy: 10 000 small files".into());
        let s = [small.clone()];
        let mut v = versus(2, &scratch, &|d| our_copy(&s, d), &|d| rf_win::bench::shell_copy_quiet(&s, d), cancel);
        v.files = 10_000;
        v.bytes = 10_000 * 4096;
        engines.copy_small = Some(v);
        step("copy: one 1 GB file".into());
        let b = [big.clone()];
        let mut v = versus(2, &scratch, &|d| our_copy(&b, d), &|d| rf_win::bench::shell_copy_quiet(&b, d), cancel);
        v.files = 1;
        v.bytes = 1 << 30;
        engines.copy_big = Some(v);
    }
    if plan.engines && !cancel.load(Ordering::Relaxed) {
        step("ZIP".into());
        let src = root.join(OpenTest::Files10k.folder());
        let zip = root.join("rf-bench 10000.zip");
        if !zip.exists() && src.exists() {
            let _ = rf_archive::compress(&[src.clone()], &zip, rf_archive::Format::Zip, 5, &mut |_| {}, &|| false);
        }
        if zip.exists() {
            // Windows: copying out of the compressed folder (what Explorer's Extract does)
            let inner = zip.join(OpenTest::Files10k.folder());
            let mut v = versus(
                2,
                &scratch,
                &|d| rf_archive::extract(&zip, d, &mut |_| {}, &|| false).map(|_| ()).map_err(|e| anyhow::anyhow!("{e}")),
                &|d| rf_win::bench::shell_copy_quiet(std::slice::from_ref(&inner), d),
                cancel,
            );
            v.files = 10_000;
            engines.unzip = Some(v);
        }
        step("search".into());
        let q = rf_win::search::SearchQuery { root: root.clone(), pattern: "file 0999".into(), include_hidden: false, max_results: 50_000 };
        let t = Instant::now();
        if let Ok(s) = rf_win::search::search(&q, &|| false, &mut |_| {}) {
            engines.search_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
            engines.search_entries = s.entries_scanned as u64;
        }
    }
    let _ = std::fs::remove_dir_all(&scratch);

    let when = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let r = Results { when, version: crate::installer::version(), machine, open, engines };
    let _ = std::fs::write(root.join("results.json"), serde_json::to_vec_pretty(&r).unwrap_or_default());
    Ok(r)
}

/// The last results, if a test was run before.
pub fn last_results() -> Option<Results> {
    serde_json::from_slice(&std::fs::read(bench_dir().join("results.json")).ok()?).ok()
}
