//! Windowless, read-only cost probe for the dock process (nothing is created on the screen, nothing is written). Prints, for the parts of
//! the dock that run on a timer or on a system event, what one run costs on this machine, and how often the system events that start a
//! task-list refresh arrive. Each part is its own sub-command so one can be run alone.
//!
//!   cargo run --release -p rf-win --example dock_cost -- all
//!   cargo run --release -p rf-win --example dock_cost -- events 30        (counts task-list events for 30 s)
//!
//! Parts: tasks, pins, icons, fullscreen, known, pdh, measure, events <secs>, offscreen.

use std::time::{Duration, Instant};

use windows::Win32::Foundation::{FILETIME, HWND};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

fn ws_mb() -> (f64, f64) {
    let mut m = PROCESS_MEMORY_COUNTERS_EX { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32, ..Default::default() };
    unsafe {
        let _ = GetProcessMemoryInfo(GetCurrentProcess(), &mut m as *mut _ as *mut PROCESS_MEMORY_COUNTERS, m.cb);
    }
    (m.WorkingSetSize as f64 / 1048576.0, m.PrivateUsage as f64 / 1048576.0)
}

fn cpu_ms() -> f64 {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let q = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 10_000.0;
    q(k) + q(u)
}

fn timed<T>(label: &str, n: u32, mut f: impl FnMut() -> T) -> T {
    let mut last = None;
    let c0 = cpu_ms();
    let t0 = Instant::now();
    for _ in 0..n {
        last = Some(f());
    }
    let wall = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let cpu = (cpu_ms() - c0) / n as f64;
    println!("  {label}: {wall:.3} ms wall, {cpu:.3} ms cpu per run (n={n})");
    last.unwrap()
}

unsafe extern "system" fn count(_: HWND, l: windows::Win32::Foundation::LPARAM) -> windows::core::BOOL {
    unsafe { *(l.0 as *mut u32) += 1 };
    windows::core::BOOL(1)
}

fn tasks() {
    println!("== tasklist::tasks() + grouped() (what every task-list refresh runs on a worker thread)");
    let mut all = 0u32;
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(Some(count), windows::Win32::Foundation::LPARAM(&mut all as *mut u32 as isize));
    }
    timed("EnumWindows alone (callback counts)", 200, || unsafe {
        let mut n = 0u32;
        let _ = windows::Win32::UI::WindowsAndMessaging::EnumWindows(Some(count), windows::Win32::Foundation::LPARAM(&mut n as *mut u32 as isize));
        n
    });
    // a thread without COM: the way a GPUI background thread may be
    let no_com = std::thread::spawn(|| {
        let t = rf_win::tasklist::tasks();
        (t.len(), t.iter().filter(|t| t.app_id.is_some()).count())
    })
    .join()
    .unwrap();
    rf_win::shell::ensure_com();
    let with_com = {
        let t = rf_win::tasklist::tasks();
        (t.len(), t.iter().filter(|t| t.app_id.is_some()).count())
    };
    println!("  top-level windows enumerated: {all}; tasks found: {}; with an AppUserModelID: {} (thread without COM) vs {} (after ensure_com)", with_com.0, no_com.1, with_com.1);
    let n = timed("tasks()", 100, rf_win::tasklist::tasks).len();
    timed("tasks() + grouped()", 100, || rf_win::tasklist::grouped(rf_win::tasklist::tasks()).len());
    println!("  ({n} tasks)");
}

fn pins() -> Vec<rf_win::dock::PinnedApp> {
    println!("== pins::read_pins() (runs every 20 s on a worker thread)");
    let ws0 = ws_mb();
    let first = Instant::now();
    let p = rf_win::dock::pins::read_pins();
    println!("  first run (cold): {:.1} ms, {} pins", first.elapsed().as_secs_f64() * 1000.0, p.len());
    let packaged = p.iter().filter(|a| matches!(a.launch, rf_win::dock::Launch::Packaged(_))).count();
    println!("  of them packaged (shell:AppsFolder lookups): {packaged}");
    timed("read_pins()", 20, rf_win::dock::pins::read_pins);
    let ws1 = ws_mb();
    println!("  working set {:.1} -> {:.1} MB, private {:.1} -> {:.1} MB across the pin reads (COM, shell, packaged-app lookups)", ws0.0, ws1.0, ws0.1, ws1.1);
    p
}

fn icons(pins: &[rf_win::dock::PinnedApp]) {
    println!("== icons::icon_bgra(.., 56) for every pin + the two places (once per icon, worker threads)");
    rf_win::shell::ensure_com();
    let ws0 = ws_mb();
    let mut bytes = 0usize;
    let mut srcs: Vec<rf_win::dock::IconSource> = pins.iter().map(|p| p.icon.clone()).collect();
    srcs.push(rf_win::dock::Place::RecycleBin.icon_source());
    srcs.push(rf_win::dock::Place::Downloads.icon_source());
    let mut total = Duration::ZERO;
    let mut worst = Duration::ZERO;
    for s in &srcs {
        let t = Instant::now();
        match rf_win::dock::icons::icon_bgra(s, 56) {
            Ok(b) => bytes += b.data.len(),
            Err(e) => println!("    failed: {e:#}"),
        }
        let d = t.elapsed();
        total += d;
        worst = worst.max(d);
    }
    let n = srcs.len() as f64;
    println!(
        "  {} icons: {:.1} ms in all (worst {:.1} ms), {:.1} KB of BGRA at 56 px, {:.1} KB at 70 px (125 %), {:.1} KB at 84 px (150 %)",
        srcs.len(),
        total.as_secs_f64() * 1000.0,
        worst.as_secs_f64() * 1000.0,
        bytes as f64 / 1024.0,
        n * 70.0 * 70.0 * 4.0 / 1024.0,
        n * 84.0 * 84.0 * 4.0 / 1024.0
    );
    let t = Instant::now();
    for s in &srcs {
        let _ = rf_win::dock::icons::icon_bgra(s, 56);
    }
    println!("  second run (warm shell cache): {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    let ws1 = ws_mb();
    println!("  working set {:.1} -> {:.1} MB, private {:.1} -> {:.1} MB across the icon loads", ws0.0, ws1.0, ws0.1, ws1.1);
    let src = rf_win::thumbs::Bgra { width: 256, height: 256, data: vec![200; 256 * 256 * 4] };
    timed("fit_square 256 -> 56", 200, || rf_win::dock::icons::fit_square(&src, 56));
    let src = rf_win::thumbs::Bgra { width: 64, height: 64, data: vec![200; 64 * 64 * 4] };
    timed("fit_square 64 -> 56", 200, || rf_win::dock::icons::fit_square(&src, 56));
}

fn fullscreen() {
    println!("== the 500 ms poll of the system window");
    timed("fullscreen::on_primary()", 2000, rf_win::dock::fullscreen::on_primary);
    timed("system::primary_monitor() (every 4th tick)", 2000, rf_win::dock::primary_monitor);
}

fn known() {
    println!("== KnownFolder::Downloads.path() (called on the UI thread at every task-list refresh)");
    timed("SHGetKnownFolderPath(Downloads)", 500, || rf_win::known::KnownFolder::Downloads.path());
}

fn measure() {
    // the glass engine per-window measure (engine.rs), on the Windows taskbar window, a window we only look at
    use windows::Win32::Foundation::{POINT, RECT};
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_WINDOW_CORNER_PREFERENCE, DwmGetWindowAttribute};
    use windows::Win32::Graphics::Gdi::{ClientToScreen, GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows::Win32::UI::HiDpi::GetDpiForWindow;
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowW, GetClientRect, IsIconic, IsWindow, IsWindowVisible, IsZoomed};
    println!("== the glass engine per-window measure, replicated read-only on the shell taskbar window");
    let h = unsafe { FindWindowW(windows::core::w!("Shell_TrayWnd"), None) }.unwrap_or_default();
    if h.0.is_null() {
        println!("  no taskbar window found");
        return;
    }
    let one = || unsafe {
        let _ = IsWindow(Some(h));
        let _ = IsWindowVisible(h);
        let _ = IsIconic(h);
        let mut cloaked = 0u32;
        let _ = DwmGetWindowAttribute(h, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, 4);
        let mut rc = RECT::default();
        let mut origin = POINT::default();
        let _ = GetClientRect(h, &mut rc);
        let _ = ClientToScreen(h, &mut origin);
        let _ = GetDpiForWindow(h);
        let hmon = MonitorFromWindow(h, MONITOR_DEFAULTTONEAREST);
        let mut mi = MONITORINFO { cbSize: size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(hmon, &mut mi);
        let mut pref = 0i32;
        let _ = DwmGetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE, &mut pref as *mut i32 as *mut _, 4);
        let _ = IsZoomed(h);
    };
    timed("one measure()", 20000, one);
    println!("  the engine calls measure_all() at least once per turn for every window; the dock has 3 windows");
}

fn pdh() {
    use windows::Win32::System::Performance::{
        PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA, PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhOpenQueryW,
    };
    use windows::core::w;
    println!("== the GPUI fork RendererMonitor tick (every 500 ms, in every rustyfiles process): the PDH GPU Engine query, replicated");
    unsafe {
        let mut q = PDH_HQUERY::default();
        if PdhOpenQueryW(windows::core::PCWSTR::null(), 0, &mut q) != 0 {
            println!("  PdhOpenQuery failed");
            return;
        }
        let mut c = PDH_HCOUNTER::default();
        if PdhAddEnglishCounterW(q, w!(r"\GPU Engine(*engtype_3D)\Utilization Percentage"), 0, &mut c) != 0 {
            println!("  counter not available");
            PdhCloseQuery(q);
            return;
        }
        PdhCollectQueryData(q);
        let before = ws_mb();
        let mut buf: Vec<u8> = Vec::new();
        let mut instances = 0u32;
        let mut walls = Vec::new();
        let mut cpus = 0.0;
        for _ in 0..40 {
            std::thread::sleep(Duration::from_millis(5));
            let t = Instant::now();
            let c0 = cpu_ms();
            PdhCollectQueryData(q);
            let (mut size, mut count) = (0u32, 0u32);
            let st = PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, None);
            if st == PDH_MORE_DATA && size > 0 {
                buf.resize(size as usize, 0);
                let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
                PdhGetFormattedCounterArrayW(c, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items));
            }
            instances = count;
            walls.push(t.elapsed().as_secs_f64() * 1000.0);
            cpus += cpu_ms() - c0;
        }
        walls.sort_by(|a, b| a.partial_cmp(b).unwrap());
        println!(
            "  {instances} GPU engine instances in the answer; per tick: median {:.2} ms wall, max {:.2} ms; mean cpu {:.2} ms (process cpu clock, coarse)",
            walls[walls.len() / 2],
            walls[walls.len() - 1],
            cpus / 40.0
        );
        let after = ws_mb();
        println!("  working set {:.1} -> {:.1} MB, private {:.1} -> {:.1} MB over the 40 ticks", before.0, after.0, before.1, after.1);
        PdhCloseQuery(q);
    }
}

fn events(secs: u64) {
    println!("== task-list events for {secs} s (the same hooks and the same 16 ms coalescing as the middle island of the dock)");
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    let new = || Arc::new(AtomicU32::new(0));
    let (lists, actives, titles, batches, both, differs) = (new(), new(), new(), new(), new(), new());
    let (l2, a2, t2, b2, c2, d2) = (lists.clone(), actives.clone(), titles.clone(), batches.clone(), both.clone(), differs.clone());
    // what the middle island would show: the groups with their windows, and which one is in front
    let signature = || {
        let groups = rf_win::tasklist::grouped(rf_win::tasklist::tasks());
        let fg = rf_win::dock::foreground_root();
        let sig: Vec<(String, Vec<isize>, bool)> = groups.into_iter().map(|(k, t)| { let w: Vec<isize> = t.iter().map(|t| t.hwnd).collect(); let a = w.contains(&fg); (k, w, a) }).collect();
        sig
    };
    let last = std::sync::Mutex::new(signature());
    let _w = rf_win::tasklist::watch(move |c| {
        b2.fetch_add(1, Ordering::Relaxed);
        if c.list {
            l2.fetch_add(1, Ordering::Relaxed);
        }
        if c.active {
            a2.fetch_add(1, Ordering::Relaxed);
        }
        if c.titles {
            t2.fetch_add(1, Ordering::Relaxed);
        }
        if c.list || c.active {
            c2.fetch_add(1, Ordering::Relaxed);
            let now = signature();
            let mut l = last.lock().unwrap();
            if *l != now {
                d2.fetch_add(1, Ordering::Relaxed);
                *l = now;
            }
        }
    })
    .expect("watch");
    std::thread::sleep(Duration::from_secs(secs));
    let (l, a, t, b, r, d) = (lists.load(Ordering::Relaxed), actives.load(Ordering::Relaxed), titles.load(Ordering::Relaxed), batches.load(Ordering::Relaxed), both.load(Ordering::Relaxed), differs.load(Ordering::Relaxed));
    println!("  batches {b}; with list=true {l}; with active=true {a}; with titles {t}");
    println!("  the dock refreshes on list||active: {r} refreshes in {secs} s = {:.2} per second ({:.1} per minute)", r as f64 / secs as f64, r as f64 * 60.0 / secs as f64);
    println!("  of those, the ones whose result (groups, their windows, which is in front) differs from the one before: {d}");
}

fn compile() {
    use windows::Win32::Graphics::Direct3D::Fxc::{D3DCOMPILE_OPTIMIZATION_LEVEL3, D3DCompile};
    use windows::Win32::Graphics::Direct3D::ID3DBlob;
    use windows::core::PCSTR;
    println!("== D3DCompile of glass.hlsl, the four entry points the engine compiles at start (VS, PSCrop, PSDown, PSGlass), OPT3");
    let src = include_str!("../src/glass/glass.hlsl");
    let before = ws_mb();
    let mut total = 0usize;
    for round in 0..3 {
        let t = Instant::now();
        let c0 = cpu_ms();
        total = 0;
        for (entry, target) in [("VSMain ", "vs_5_0 "), ("PSCrop ", "ps_5_0 "), ("PSDown ", "ps_5_0 "), ("PSGlass ", "ps_5_0 ")] {
            let (mut blob, mut err): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
            let r = unsafe {
                D3DCompile(src.as_ptr() as *const _, src.len(), PCSTR(b"glass.hlsl ".as_ptr()), None, None, PCSTR(entry.as_ptr()), PCSTR(target.as_ptr()), D3DCOMPILE_OPTIMIZATION_LEVEL3, 0, &mut blob, Some(&mut err))
            };
            assert!(r.is_ok(), "compile {entry}");
            total += unsafe { blob.unwrap().GetBufferSize() };
        }
        println!("  round {round}: {:.0} ms wall, {:.0} ms cpu, {} bytes of bytecode in all", t.elapsed().as_secs_f64() * 1000.0, cpu_ms() - c0, total);
    }
    let after = ws_mb();
    println!("  working set {:.1} -> {:.1} MB, private {:.1} -> {:.1} MB after 3 rounds ({total} bytes would be embedded)", before.0, after.0, before.1, after.1);
}

fn offscreen() {
    println!("== glass engine fixed cost, windowless: one offscreen picture = a D3D11 device + 4 D3DCompile of glass.hlsl + one tiny draw");
    let before = ws_mb();
    let has = || unsafe { windows::Win32::System::LibraryLoader::GetModuleHandleW(windows::core::w!("d3dcompiler_47.dll")).is_ok() };
    println!("  before: working set {:.1} MB, private {:.1} MB; d3dcompiler_47 loaded: {}", before.0, before.1, has());
    let src = vec![120u8; 512 * 256 * 4];
    let t = Instant::now();
    let c0 = cpu_ms();
    let r = rf_win::glass::render_offscreen(&src, 512, 256, (100, 100, 276, 48), 24.0, 1.0, true);
    println!("  first call: {:.0} ms wall, {:.0} ms cpu, ok={}", t.elapsed().as_secs_f64() * 1000.0, cpu_ms() - c0, r.is_ok());
    let after = ws_mb();
    println!("  after: working set {:.1} MB, private {:.1} MB; d3dcompiler_47 loaded: {}", after.0, after.1, has());
    let t = Instant::now();
    let c0 = cpu_ms();
    let _ = rf_win::glass::render_offscreen(&src, 512, 256, (100, 100, 276, 48), 24.0, 1.0, true);
    println!("  second call (device and shaders are made again on every call of this path): {:.0} ms wall, {:.0} ms cpu", t.elapsed().as_secs_f64() * 1000.0, cpu_ms() - c0);
}

fn copy() {
    use windows::Win32::Foundation::HMODULE;
    use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
    println!("== GPU time of copying the monitor into the mirror (CopyResource of a whole 2560x1440 BGRA texture) against a dirty rectangle, timestamp queries");
    unsafe {
        let (mut dev, mut ctx) = (None, None);
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, HMODULE::default(), D3D11_CREATE_DEVICE_BGRA_SUPPORT, Some(&[D3D_FEATURE_LEVEL_11_0]), D3D11_SDK_VERSION, Some(&mut dev), None, Some(&mut ctx)).expect("device");
        let (dev, ctx) = (dev.unwrap(), ctx.unwrap());
        let mk = |w: u32, h: u32| {
            let mut t = None;
            dev.CreateTexture2D(
                &D3D11_TEXTURE2D_DESC { Width: w, Height: h, MipLevels: 1, ArraySize: 1, Format: DXGI_FORMAT_B8G8R8A8_UNORM, SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 }, Usage: D3D11_USAGE_DEFAULT, BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32, CPUAccessFlags: 0, MiscFlags: 0 },
                None,
                Some(&mut t),
            )
            .unwrap();
            t.unwrap()
        };
        for (w, h) in [(2560u32, 1440u32), (3840, 2160)] {
            let (a, b) = (mk(w, h), mk(w, h));
            let q = |kind| {
                let mut q = None;
                dev.CreateQuery(&D3D11_QUERY_DESC { Query: kind, MiscFlags: 0 }, Some(&mut q)).unwrap();
                q.unwrap()
            };
            let measure = |label: &str, f: &dyn Fn()| {
                let mut us = Vec::new();
                for _ in 0..200 {
                    let (dj, t0, t1) = (q(D3D11_QUERY_TIMESTAMP_DISJOINT), q(D3D11_QUERY_TIMESTAMP), q(D3D11_QUERY_TIMESTAMP));
                    ctx.Begin(&dj);
                    ctx.End(&t0);
                    for _ in 0..10 {
                        f();
                    }
                    ctx.End(&t1);
                    ctx.End(&dj);
                    ctx.Flush();
                    // GetData answers S_FALSE (no error) while the data is not ready: poll until the data is plausible
                    let mut d = D3D11_QUERY_DATA_TIMESTAMP_DISJOINT::default();
                    while d.Frequency == 0 {
                        let _ = ctx.GetData(&dj, Some(&mut d as *mut _ as *mut _), size_of::<D3D11_QUERY_DATA_TIMESTAMP_DISJOINT>() as u32, 0);
                        std::thread::yield_now();
                    }
                    let (mut v0, mut v1) = (0u64, 0u64);
                    while v0 == 0 {
                        let _ = ctx.GetData(&t0, Some(&mut v0 as *mut _ as *mut _), 8, 0);
                    }
                    while v1 == 0 {
                        let _ = ctx.GetData(&t1, Some(&mut v1 as *mut _ as *mut _), 8, 0);
                    }
                    if d.Disjoint.as_bool() || d.Frequency == 0 {
                        continue;
                    }
                    us.push((v1 - v0) as f64 / d.Frequency as f64 * 1e6 / 10.0);
                }
                us.sort_by(|a, b| a.partial_cmp(b).unwrap());
                println!("  {w}x{h} {label}: median {:.1} us GPU per copy ({} samples)", us[us.len() / 2], us.len());
            };
            measure("whole texture (CopyResource)", &|| ctx.CopyResource(&b, &a));
            let bx = D3D11_BOX { left: 100, top: h - 300, front: 0, right: 100 + 1800, bottom: h, back: 1 };
            measure("one rectangle 1800x300 (the dock band, CopySubresourceRegion)", &|| ctx.CopySubresourceRegion(&b, 0, 100, h - 300, 0, &a, 0, Some(&bx)));
            let bx = D3D11_BOX { left: 100, top: h - 100, front: 0, right: 100 + 300, bottom: h - 52, back: 1 };
            measure("one rectangle 300x48 (a clock changing in the tray)", &|| ctx.CopySubresourceRegion(&b, 0, 100, h - 100, 0, &a, 0, Some(&bx)));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let part = args.first().map(String::as_str).unwrap_or("all");
    let all = part == "all";
    if all || part == "tasks" {
        tasks();
    }
    let mut p = Vec::new();
    if all || part == "pins" || part == "icons" {
        p = pins();
    }
    if all || part == "icons" {
        icons(&p);
    }
    if all || part == "fullscreen" {
        fullscreen();
    }
    if all || part == "known" {
        known();
    }
    if all || part == "measure" {
        measure();
    }
    if all || part == "pdh" {
        pdh();
    }
    if part == "events" {
        events(args.get(1).and_then(|s| s.parse().ok()).unwrap_or(20));
    }
    if part == "copy" {
        copy();
    }
    if part == "compile" {
        compile();
    }
    if part == "offscreen" {
        offscreen();
    }
}
