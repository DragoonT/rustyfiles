//! A windowless manual check of the capture session backend, for `examples/glass_capture_check.rs`. It is not a test and not part of the
//! app: it opens the engine's own session code (`wgc.rs`) on one monitor, puts the windows it is given into the exclusion list,
//! waits for the first picture made with that list and hands the mirror back. It creates no window, no swap chain, no picker, and asks
//! the system for nothing: the border-free permission is only READ, and nothing is opened unless it says Allowed.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};

use super::capture::{MonError, Monitor, Pumped, locate};
use super::engine::Apartment;
use super::gpu::Gpu;
use super::wgc::{self, Exclusion};

/// What one run of the check saw.
pub struct Report {
    pub width: u32,
    pub height: u32,
    /// The mirror: `width` x `height` BGRA, tightly packed.
    pub bgra: Vec<u8>,
    /// The captured monitor in desktop coordinates: left, top, right, bottom.
    pub monitor: (i32, i32, i32, i32),
    /// Making the session (device, pool, session, start).
    pub open: Duration,
    /// The call that sent the exclusion list.
    pub list_call: Duration,
    /// From the start of the capture to the first picture made with the list.
    pub first_image: Duration,
    /// Frames the pool delivered meanwhile (stale ones included).
    pub frames: u32,
    /// How many window ids the session reports back as its exclusion list.
    pub reported: Option<usize>,
}

fn why(e: MonError) -> String {
    match e {
        MonError::Device(s) | MonError::Monitor(s) | MonError::Backend(s) => s,
    }
}

/// The WinRT apartment the check needs: held by the caller for the whole check, like the engine thread holds it for its life (leaving it
/// between two runs tears the system's capture machinery down while it is still releasing the first session's objects).
pub struct ApartmentGuard(#[allow(dead_code)] Apartment);

pub fn enter_apartment() -> ApartmentGuard {
    ApartmentGuard(Apartment::enter())
}

/// Captures the monitor that contains the desktop point `monitor_at` (the primary one when it is on none) through a session that leaves `excluded` (window handles) out, and returns the picture. `Err` when the
/// permission is not Allowed (nothing was opened then) or anything fails.
pub fn run(excluded: &[isize], max_wait: Duration, monitor_at: (i32, i32)) -> Result<Report, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let g = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;

        let t0 = Instant::now();
        let mut mon = Monitor::open_session(&g, hmon, wgc::Power::Mains)?;
        let open = t0.elapsed();
        log::info!("glass: screen capture through a Windows.Graphics.Capture session (manual check)");

        let mut ex = Exclusion::default();
        for &h in excluded {
            ex.attach(h);
            ex.set_visible(h, true);
        }
        let start = Instant::now();
        let (mut frames, mut first_image) = (0u32, None);
        while start.elapsed() < max_wait && first_image.is_none() {
            match mon.pump(&g, 50, &ex, None, &[]).map_err(why)? {
                Pumped::Waited { frame, image, .. } => {
                    frames += frame as u32;
                    if image {
                        first_image = Some(start.elapsed());
                    }
                }
                Pumped::NoWait => {}
            }
        }
        let first_image = first_image.ok_or("no picture made with the exclusion list arrived in time (a static screen sends no frame by itself)")?;
        let (list_call, reported) = match &mon {
            Monitor::Session(s) => (s.last_set_time, s.reported_list().map(|l| l.len())),
            Monitor::Dup(_) => (Duration::ZERO, None),
        };
        let m = mon.mirror().ok_or("no mirror")?;
        let mut d = D3D11_TEXTURE2D_DESC::default();
        m.tex.GetDesc(&mut d);
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.MiscFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        g.dev.CreateTexture2D(&d, None, Some(&mut staging)).map_err(|e| format!("staging texture: {e}"))?;
        let staging = staging.ok_or("no staging texture")?;
        g.ctx.CopyResource(&staging, &m.tex);
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        g.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).map_err(|e| format!("reading the mirror back: {e}"))?;
        let (w, h) = (d.Width as usize, d.Height as usize);
        let mut bgra = vec![0u8; w * h * 4];
        for y in 0..h {
            let row = std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4);
            bgra[y * w * 4..(y + 1) * w * 4].copy_from_slice(row);
        }
        g.ctx.Unmap(&staging, 0);
        let r = mon.rect();
        // the session is closed here, when `mon` is dropped
        Ok(Report { width: d.Width, height: d.Height, bgra, monitor: (r.left, r.top, r.right, r.bottom), open, list_call, first_image, frames, reported })
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Windowless measurements of the capture (for `examples/glass_session_cost.rs`)
// ---------------------------------------------------------------------------------------------------------------------------------

use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Dxgi::{DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO, IDXGIAdapter3};
use windows::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, GetThreadTimes};
use windows::core::Interface;

use super::geom::Rc;
use super::gpu::{Geometry, Region, Renderer, glass_pass, offscreen_target, prep, region_pad};
use super::wgc::{Power, SessionMonitor};

/// How a measuring run asks the session for frames.
#[derive(Clone, Copy, Debug)]
pub enum ProbeInterval {
    /// What the engine did before the cap: one refresh period of the display.
    Refresh,
    /// What the engine does now, for this power state.
    Policy { on_battery: bool },
    /// A fixed interval in milliseconds.
    Millis(f64),
}

/// What a measuring run does.
#[derive(Clone, Debug)]
pub struct ProbeOpts {
    pub secs: f64,
    /// The engine's wait for a frame on a quiet desktop.
    pub wait_ms: u32,
    pub interval: ProbeInterval,
    /// Frame pool buffers.
    pub buffers: i32,
    /// A glass window as the engine would hold it: x, y, w, h in the monitor's pixels. Frames that touch it are prepared and read back
    /// (and presented when the loop guard says so), into a private texture: no window, no swap chain. `None`: frames are only taken.
    pub window: Option<(i32, i32, u32, u32)>,
    /// Asks for a frame (the exclusion list is sent again) every this many milliseconds, a source of frames for a desktop that is
    /// still. The capture service decides how many of them it delivers: a cap on the rate shows here.
    pub drive_ms: Option<f64>,
    /// Half way through, the session is set to the battery interval (a running session must take it).
    pub switch_power_at_half: bool,
    pub monitor_at: (i32, i32),
    /// Called with `"opened"` when the session is open and set up, and with `"finished"` just before it is closed, so that a measuring
    /// program can read its threads in between.
    pub hook: Option<fn(&'static str)>,
}

/// What a measuring run saw.
#[derive(Clone, Debug, Default)]
pub struct ProbeReport {
    pub wall: Duration,
    /// CPU time (user + kernel) of the thread that ran the loop, which is what the engine thread would have used for the same work.
    pub thread_cpu: Duration,
    /// The same in processor cycles (`QueryThreadCycleTime`, exact where the time above has a resolution of 15.6 ms), for the loop thread
    /// and for every thread of the process together.
    pub thread_cycles: u64,
    pub process_cycles: u64,
    pub turns: u32,
    /// Times the frame handler of the session ran (announced frames).
    pub arrived: u64,
    /// Frames the loop took with the current list, and how many refreshed the mirror.
    pub frames: u32,
    pub images: u32,
    /// Draws prepared for the emulated window, presented and skipped by the loop guard.
    pub prepared: u32,
    pub presented: u32,
    pub skipped: u32,
    pub frames_first_half: u32,
    pub frames_second_half: u32,
    /// The interval the session reports (ms) at the start, after the switch (if any).
    pub interval_ms: Option<f64>,
    pub interval_ms_after: Option<f64>,
    /// Video memory in use by this process on the adapter, bytes: before the session, and with it open at the end.
    pub vram: [u64; 2],
    pub monitor: (i32, i32, i32, i32),
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn QueryThreadCycleTime(thread: *mut std::ffi::c_void, cycles: *mut u64) -> i32;
    fn QueryProcessCycleTime(process: *mut std::ffi::c_void, cycles: *mut u64) -> i32;
}

/// Processor cycles used by the calling thread and by the whole process so far.
fn cycles() -> (u64, u64) {
    unsafe {
        let (mut t, mut p) = (0u64, 0u64);
        QueryThreadCycleTime(GetCurrentThread().0, &mut t);
        QueryProcessCycleTime(GetCurrentProcess().0, &mut p);
        (t, p)
    }
}

/// Processor cycles a second, measured by spinning the calling thread for `ms` milliseconds.
pub fn cycles_per_second(ms: u64) -> f64 {
    let (c0, _) = cycles();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_millis(ms) {
        std::hint::spin_loop();
    }
    let (c1, _) = cycles();
    (c1 - c0) as f64 / t0.elapsed().as_secs_f64()
}

fn thread_cpu() -> Duration {
    unsafe {
        let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
        let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
        Duration::from_nanos((t(k) + t(u)) * 100)
    }
}

unsafe fn vram(adapter: &windows::Win32::Graphics::Dxgi::IDXGIAdapter1) -> u64 {
    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
    match adapter.cast::<IDXGIAdapter3>().and_then(|a| a.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &mut info)) {
        Ok(()) => info.CurrentUsage,
        Err(_) => 0,
    }
}

/// Runs the engine's capture loop for `secs` on one monitor with no window: the same session code, the same turn (a wait for a frame,
/// a test of what the frame touched, the draw of one emulated window), and reports what it cost. Reads the screen only.
pub fn probe(opts: &ProbeOpts) -> Result<ProbeReport, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: opts.monitor_at.0, y: opts.monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let r = Renderer::new(gpu, false)?;
        let mut rep = ProbeReport { vram: [vram(&loc.adapter), 0], ..Default::default() };

        let mut mon = SessionMonitor::open_with(&r.g, hmon, Power::Mains, opts.buffers)?;
        match opts.interval {
            ProbeInterval::Policy { on_battery } => {
                mon.retune(if on_battery { Power::Battery } else { Power::Mains });
            }
            ProbeInterval::Refresh => mon.set_interval_ticks(wgc::refresh_period_ticks(mon.hz())),
            ProbeInterval::Millis(ms) => mon.set_interval_ticks((ms * 10_000.0) as i64),
        }
        rep.interval_ms = mon.reported_interval().map(|t| t as f64 / 1e4);
        rep.monitor = (mon.rect.left, mon.rect.top, mon.rect.right, mon.rect.bottom);

        // the emulated window
        let mut emu = None;
        if let Some((x, y, w, h)) = opts.window {
            let pad = region_pad(1.0);
            let region = Region::new(&r.g, w, h, pad).map_err(|e| format!("region: {e}"))?;
            let rtv = offscreen_target(&r.g, w, h).map_err(|e| format!("target: {e}"))?;
            let geo = Geometry { w, h, origin: (x, y), scale: 1.0, radius_px: 8.0, cut_corners: false, shape: None, shadow: None };
            emu = Some((region, rtv, geo, (x, y, w, h)));
        }
        let look = super::gpu::Look::regular(true, &super::look());

        let mut ex = Exclusion::default();
        let (mut stale, mut last_hash) = (true, None::<u64>);
        if let Some(h) = opts.hook {
            h("opened");
        }
        let start = Instant::now();
        let cpu0 = thread_cpu();
        let cyc0 = cycles();
        let arrived0 = mon.arrived();
        let half = Duration::from_secs_f64(opts.secs / 2.0);
        let mut switched = false;
        let mut next_drive = Instant::now();
        while start.elapsed() < Duration::from_secs_f64(opts.secs) {
            if let Some(ms) = opts.drive_ms
                && Instant::now() >= next_drive
            {
                ex.nudge();
                next_drive = Instant::now() + Duration::from_secs_f64(ms / 1000.0);
            }
            if opts.switch_power_at_half && !switched && start.elapsed() >= half {
                switched = true;
                mon.retune(Power::Battery);
                rep.interval_ms_after = mon.reported_interval().map(|t| t as f64 / 1e4);
            }
            rep.turns += 1;
            match mon.pump(&r.g, opts.wait_ms, &ex, None, &[]).map_err(why)? {
                Pumped::Waited { frame, image, dirty } => {
                    if frame {
                        rep.frames += 1;
                        if switched {
                            rep.frames_second_half += 1;
                        } else {
                            rep.frames_first_half += 1;
                        }
                    }
                    if image {
                        rep.images += 1;
                        if let Some((_, _, _, (x, y, w, h))) = &emu
                            && dirty.touches(*x, *y, *w, *h, region_pad(1.0))
                        {
                            stale = true;
                        }
                    }
                }
                Pumped::NoWait => {}
            }
            if let Some((region, rtv, geo, _)) = &emu
                && stale
                && mon.ready()
            {
                let Some(mirror) = mon.mirror.as_ref() else { continue };
                let prepared = prep(&r, region, mirror, geo, &look, false);
                rep.prepared += 1;
                let read = region.read_back(&r.g, [0.0, 0.0, geo.w as f32, geo.h as f32]).map_err(|e| format!("read back: {e}"))?;
                stale = false;
                if last_hash == Some(read.hash) {
                    rep.skipped += 1;
                } else {
                    glass_pass(&r, region, &prepared, geo, rtv);
                    rep.presented += 1;
                    last_hash = Some(read.hash);
                }
            }
        }
        rep.wall = start.elapsed();
        rep.thread_cpu = thread_cpu() - cpu0;
        let cyc1 = cycles();
        rep.thread_cycles = cyc1.0 - cyc0.0;
        rep.process_cycles = cyc1.1 - cyc0.1;
        rep.arrived = mon.arrived() - arrived0;
        rep.vram[1] = vram(&loc.adapter);
        if let Some(h) = opts.hook {
            h("finished");
        }
        Ok(rep)
    }
}

/// What asking for a frame did on this desktop (see [`probe_force`]).
#[derive(Clone, Debug, Default)]
pub struct ForceReport {
    /// Milliseconds from sending the list again to the first frame made with it, one per trial (`None`: none within the limit).
    pub forced_ms: Vec<Option<f64>>,
    /// Milliseconds to the next frame when nothing was sent, one per control trial (`None`: none within the limit).
    pub control_ms: Vec<Option<f64>>,
}

/// Asks a running session for a frame by sending its exclusion list again (`Exclusion::nudge`), and times the frame that follows; the
/// same number of times it waits without asking. On a still desktop the second kind finds no frame and the first must find one at once:
/// that is what lets a glass that moved onto a part of the screen the mirror does not hold get it.
pub fn probe_force(trials: u32, limit: Duration, monitor_at: (i32, i32)) -> Result<ForceReport, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let mut mon = SessionMonitor::open_with(&gpu, hmon, Power::Mains, 2)?;
        let mut ex = Exclusion::default();
        // the first picture, then quiet
        let t = Instant::now();
        while t.elapsed() < Duration::from_secs(5) {
            if let Pumped::Waited { image: true, .. } = mon.pump(&gpu, 50, &ex, None, &[]).map_err(why)? {
                break;
            }
        }
        let mut rep = ForceReport::default();
        for i in 0..trials {
            // settle: take whatever frames there are for a while, so the trial starts from a quiet pool
            let s = Instant::now();
            while s.elapsed() < Duration::from_millis(300) {
                mon.pump(&gpu, 2, &ex, None, &[]).map_err(why)?;
            }
            let forced = i % 2 == 0;
            if forced {
                ex.nudge();
            }
            let t0 = Instant::now();
            let mut got = None;
            while t0.elapsed() < limit {
                if let Pumped::Waited { frame: true, .. } = mon.pump(&gpu, 2, &ex, None, &[]).map_err(why)? {
                    got = Some(t0.elapsed().as_secs_f64() * 1000.0);
                    break;
                }
            }
            if forced { rep.forced_ms.push(got) } else { rep.control_ms.push(got) }
        }
        Ok(rep)
    }
}

/// A D3D11 device on the monitor's adapter, with the glass's renderer on it, and nothing else, held for `secs` (the baseline for the
/// measurements: what a process costs that has the engine's device but no capture session). `hook` as in [`ProbeOpts`].
pub fn hold_device(secs: f64, monitor_at: (i32, i32), hook: Option<fn(&'static str)>) -> Result<(), String> {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let _r = Renderer::new(gpu, false)?;
        if let Some(h) = hook {
            h("opened");
        }
        std::thread::sleep(Duration::from_secs_f64(secs));
        if let Some(h) = hook {
            h("finished");
        }
        Ok(())
    }
}

/// The cost of copying a whole monitor-sized texture against copying only a rectangle of it, `n` times each, to the end of the GPU's
/// work: (microseconds per whole copy, microseconds per rectangle copy). The two textures are private; nothing is captured.
pub fn bench_copy(monitor_at: (i32, i32), rect: (u32, u32, u32, u32), n: u32) -> Result<(f64, f64), String> {
    use windows::Win32::Graphics::Direct3D11::{D3D11_BOX, D3D11_QUERY_DESC, D3D11_QUERY_EVENT, ID3D11Query};
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let g = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let (w, h) = (loc.rect.right - loc.rect.left, loc.rect.bottom - loc.rect.top);
        let a = super::gpu::Source::new(&g, w as u32, h as u32, None).map_err(|e| format!("texture: {e}"))?;
        let b = super::gpu::Source::new(&g, w as u32, h as u32, None).map_err(|e| format!("texture: {e}"))?;
        let mut q: Option<ID3D11Query> = None;
        g.dev.CreateQuery(&D3D11_QUERY_DESC { Query: D3D11_QUERY_EVENT, MiscFlags: 0 }, Some(&mut q)).map_err(|e| format!("query: {e}"))?;
        let q = q.ok_or("no query")?;
        let finish = || {
            g.ctx.End(&q);
            g.ctx.Flush();
            let mut done = windows::core::BOOL(0);
            while g.ctx.GetData(&q, Some(&mut done as *mut _ as *mut std::ffi::c_void), 4, 0).is_err() || !done.as_bool() {
                std::hint::spin_loop();
            }
        };
        let bx = D3D11_BOX { left: rect.0, top: rect.1, front: 0, right: rect.0 + rect.2, bottom: rect.1 + rect.3, back: 1 };
        let time = |f: &dyn Fn()| {
            for _ in 0..50 {
                f();
            }
            finish();
            let t = Instant::now();
            for _ in 0..n {
                f();
            }
            finish();
            t.elapsed().as_secs_f64() * 1e6 / n as f64
        };
        let whole = time(&|| g.ctx.CopyResource(&a.tex, &b.tex));
        let part = time(&|| g.ctx.CopySubresourceRegion(&a.tex, 0, bx.left, bx.top, 0, &b.tex, 0, Some(&bx)));
        Ok((whole, part))
    }
}

/// The top-level windows that meet a rectangle of the desktop (x, y, w, h), top of the z order first, with how the cover walk treats each
/// (read only), and whether together they hide the rectangle.
pub fn cover_survey(rect: (i32, i32, u32, u32)) -> (Vec<String>, bool) {
    let target = Rc::xywh(rect.0, rect.1, rect.2, rect.3);
    let rows = super::cover::survey(target);
    let mut covers = Vec::new();
    let mut lines = Vec::new();
    for r in &rows {
        let ex = r.ex_style;
        let flags: Vec<&str> = [(0x0008_0000u32, "LAYERED"), (0x0020_0000, "NOREDIRECTIONBITMAP"), (0x20, "TRANSPARENT"), (0x80, "TOOLWINDOW"), (0x8, "TOPMOST")]
            .iter()
            .filter(|(m, _)| ex & m != 0)
            .map(|(_, n)| *n)
            .collect();
        if r.counts {
            covers.extend(super::cover::solid_parts(r.frame, r.zoomed, super::cover::corner_allowance(96)));
        }
        lines.push(format!(
            "{:<34} pid {:<6} style {:#010x} frame ({},{})-({},{}) {}{}{}{}",
            r.class,
            r.pid,
            r.style,
            r.frame.l,
            r.frame.t,
            r.frame.r,
            r.frame.b,
            if flags.is_empty() { "-".to_string() } else { flags.join("|") },
            r.layered.map_or(String::new(), |(a, f)| format!(" layered alpha {a} flags {f}")),
            if r.has_region { " HAS-REGION" } else { "" },
            if r.cloaked { " CLOAKED" } else { "" },
        ) + if r.counts { "  => hides what is under it" } else { "  => does not count" });
    }
    (lines, super::geom::covered_by(target, &covers))
}

// ---------------------------------------------------------------------------------------------------------------------------------
// A session that comes back from a pause (see `SessionMonitor::resume`), windowless
// ---------------------------------------------------------------------------------------------------------------------------------

/// What a session did when it was used again after nobody took its frames for a while.
#[derive(Clone, Debug, Default)]
pub struct ResumeReport {
    /// The session had announced a frame since the first picture (a frame is forced just before the pause, so there is one in the pool).
    pub frame_was_queued: bool,
    /// The mirror was ready right after the resume (it must not be: the frames queued from before the pause are not to be drawn from).
    pub ready_right_after: bool,
    /// A pump right after the resume produced a picture (it must not: the only frame there is is from before the pause).
    pub old_frame_accepted: bool,
    /// Milliseconds from the resume to the first picture made after it (`None`: none within the limit).
    pub fresh_ms: Option<f64>,
}

/// Opens a session, takes its first picture, forces one more frame and leaves it in the pool for `pause`, resumes the session the way
/// the engine does and reports what the first pumps after that did. Reads the screen only.
pub fn probe_resume(pause: Duration, limit: Duration, monitor_at: (i32, i32)) -> Result<ResumeReport, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let mut mon = Monitor::open_session(&gpu, hmon, wgc::Power::Mains)?;
        let mut ex = Exclusion::default();
        // the first picture
        let t = Instant::now();
        let mut got = false;
        while t.elapsed() < Duration::from_secs(5) && !got {
            if let Pumped::Waited { image: true, .. } = mon.pump(&gpu, 50, &ex, None, &[]).map_err(why)? {
                got = true;
            }
        }
        if !got {
            return Err("no first picture".into());
        }
        // quiet, then one frame forced and left in the pool
        let s = Instant::now();
        while s.elapsed() < Duration::from_millis(200) {
            mon.pump(&gpu, 2, &ex, None, &[]).map_err(why)?;
        }
        let arrived0 = match &mon {
            Monitor::Session(m) => m.arrived(),
            Monitor::Dup(_) => 0,
        };
        ex.nudge();
        mon.pump(&gpu, 0, &ex, None, &[]).map_err(why)?; // sends the list
        // (the frame the list forces arrives within a few milliseconds; it is not taken)
        std::thread::sleep(pause);
        let mut rep = ResumeReport::default();
        if let Monitor::Session(m) = &mon {
            rep.frame_was_queued = m.arrived() > arrived0;
        }
        // the engine's resume, and then its first turn: the list is sent again and the frames are taken
        mon.resume();
        rep.ready_right_after = mon.has_image();
        if let Pumped::Waited { image, .. } = mon.pump(&gpu, 0, &ex, None, &[]).map_err(why)? {
            rep.old_frame_accepted = image;
        }
        let t0 = Instant::now();
        while t0.elapsed() < limit {
            if let Pumped::Waited { image: true, .. } = mon.pump(&gpu, 5, &ex, None, &[]).map_err(why)? {
                rep.fresh_ms = Some(t0.elapsed().as_secs_f64() * 1000.0);
                break;
            }
        }
        Ok(rep)
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Do the dirty rectangles of a frame cover what changed since the picture the engine took before it? (windowless)
// ---------------------------------------------------------------------------------------------------------------------------------

/// What `probe_dirty` saw.
#[derive(Clone, Debug, Default)]
pub struct DirtyReport {
    pub secs: f64,
    pub interval_ms: Option<f64>,
    /// Pictures the engine's own `pump` handed back.
    pub images: u32,
    /// Of those, how many differed from the one before (somewhere).
    pub changed: u32,
    /// Of those, how many reported a bounded list of rectangles (not "everywhere").
    pub bounded: u32,
    /// Pixels that differed from the previous picture, inside the reported rectangles and outside them. Outside must be 0: the engine
    /// draws a window again only when a rectangle touches it.
    pub inside: u64,
    pub outside: u64,
    /// Pictures with pixels outside the reported rectangles.
    pub violations: u32,
}

/// Runs the engine's session for `secs` at the given interval, and after every picture reads the mirror back and compares it with the
/// one before: every pixel that differs must lie inside the rectangles the pump reported for the frames it took in between (frames the
/// interval or a full pool made it skip included, that is the point). Only changes on the screen can feed it; a still desktop proves
/// nothing, and the report says how many pictures differed.
pub fn probe_dirty(secs: f64, interval: ProbeInterval, monitor_at: (i32, i32)) -> Result<DirtyReport, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let mut mon = SessionMonitor::open_with(&gpu, hmon, Power::Mains, 2)?;
        match interval {
            ProbeInterval::Policy { on_battery } => {
                mon.retune(if on_battery { Power::Battery } else { Power::Mains });
            }
            ProbeInterval::Refresh => mon.set_interval_ticks(wgc::refresh_period_ticks(mon.hz())),
            ProbeInterval::Millis(ms) => mon.set_interval_ticks((ms * 10_000.0) as i64),
        }
        let ex = Exclusion::default();
        let mut rep = DirtyReport { interval_ms: mon.reported_interval().map(|t| t as f64 / 1e4), ..Default::default() };
        let (w, h) = ((loc.rect.right - loc.rect.left) as usize, (loc.rect.bottom - loc.rect.top) as usize);
        let mut d = D3D11_TEXTURE2D_DESC::default();
        mon.mirror.as_ref().ok_or("no mirror")?.tex.GetDesc(&mut d);
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.MiscFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        gpu.dev.CreateTexture2D(&d, None, Some(&mut staging)).map_err(|e| format!("staging texture: {e}"))?;
        let staging = staging.ok_or("no staging texture")?;
        let read = |mon: &SessionMonitor| -> Result<Vec<u8>, String> {
            gpu.ctx.CopyResource(&staging, &mon.mirror.as_ref().ok_or("no mirror")?.tex);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).map_err(|e| format!("reading the mirror back: {e}"))?;
            let mut out = vec![0u8; w * h * 4];
            for y in 0..h {
                out[y * w * 4..(y + 1) * w * 4].copy_from_slice(std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4));
            }
            gpu.ctx.Unmap(&staging, 0);
            Ok(out)
        };
        let mut prev: Option<Vec<u8>> = None;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs_f64(secs) {
            let Pumped::Waited { image: true, dirty, .. } = mon.pump(&gpu, 20, &ex, None, &[]).map_err(why)? else { continue };
            rep.images += 1;
            let now = read(&mon)?;
            if let Some(before) = &prev {
                let rects = match &dirty {
                    super::capture::Dirty::Rects(r) => Some(r),
                    super::capture::Dirty::All => None,
                };
                if rects.is_some() {
                    rep.bounded += 1;
                }
                let (mut inside, mut outside) = (0u64, 0u64);
                for y in 0..h {
                    let (a, b) = (&before[y * w * 4..(y + 1) * w * 4], &now[y * w * 4..(y + 1) * w * 4]);
                    if a == b {
                        continue;
                    }
                    for x in 0..w {
                        if a[x * 4..x * 4 + 4] != b[x * 4..x * 4 + 4] {
                            let covered = rects.is_none_or(|rs| rs.iter().any(|r| (x as i32) >= r.left && (x as i32) < r.right && (y as i32) >= r.top && (y as i32) < r.bottom));
                            if covered {
                                inside += 1;
                            } else {
                                outside += 1;
                            }
                        }
                    }
                }
                if inside + outside > 0 {
                    rep.changed += 1;
                }
                if outside > 0 {
                    rep.violations += 1;
                }
                rep.inside += inside;
                rep.outside += outside;
            }
            prev = Some(now);
        }
        rep.secs = start.elapsed().as_secs_f64();
        Ok(rep)
    }
}

/// [`probe_dirty`] for Desktop Duplication, the fallback backend: the engine's own `DupMonitor` (the dirty rectangles it reads, and,
/// with `slow`, the gap it leaves between two frames while no glass is in front) on one monitor. After every picture the mirror is read
/// back and compared with the one before: every pixel that differs must lie inside the rectangles the pump reported for the frames it
/// took in between (frames the gap made it leave alone included, that is the point of `slow`). Only changes on the screen can feed it.
pub fn probe_dirty_dup(secs: f64, slow: bool, monitor_at: (i32, i32)) -> Result<DirtyReport, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let luid = loc.adapter.GetDesc1().map_err(|e| format!("adapter description: {e}"))?.AdapterLuid;
        let gpu = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;
        let mut mon = Monitor::open(hmon, luid)?;
        if slow {
            mon.set_pace(super::rest::Pace::Slow, wgc::Power::Mains);
        }
        let ex = Exclusion::default();
        let mut rep = DirtyReport { interval_ms: slow.then(|| super::rest::gap_for(super::rest::behind_fps()).as_secs_f64() * 1000.0), ..Default::default() };
        let (w, h) = ((loc.rect.right - loc.rect.left) as usize, (loc.rect.bottom - loc.rect.top) as usize);
        let mut staging: Option<windows::Win32::Graphics::Direct3D11::ID3D11Texture2D> = None;
        let mut prev: Option<Vec<u8>> = None;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs_f64(secs) {
            if let Some(left) = mon.throttled_for() {
                std::thread::sleep(left.min(Duration::from_millis(50)));
                continue;
            }
            let Pumped::Waited { image: true, dirty, .. } = mon.pump(&gpu, 20, &ex, None, &[]).map_err(why)? else { continue };
            rep.images += 1;
            let mirror = mon.mirror().ok_or("no mirror")?;
            if staging.is_none() {
                let mut d = D3D11_TEXTURE2D_DESC::default();
                mirror.tex.GetDesc(&mut d);
                d.Usage = D3D11_USAGE_STAGING;
                d.BindFlags = 0;
                d.MiscFlags = 0;
                d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                let mut t = None;
                gpu.dev.CreateTexture2D(&d, None, Some(&mut t)).map_err(|e| format!("staging texture: {e}"))?;
                staging = t;
            }
            let staging = staging.as_ref().ok_or("no staging texture")?;
            gpu.ctx.CopyResource(staging, &mirror.tex);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            gpu.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).map_err(|e| format!("reading the mirror back: {e}"))?;
            let mut now = vec![0u8; w * h * 4];
            for y in 0..h {
                now[y * w * 4..(y + 1) * w * 4].copy_from_slice(std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4));
            }
            gpu.ctx.Unmap(staging, 0);
            if let Some(before) = &prev {
                let rects = match &dirty {
                    super::capture::Dirty::Rects(r) => Some(r),
                    super::capture::Dirty::All => None,
                };
                if rects.is_some() {
                    rep.bounded += 1;
                }
                let (mut inside, mut outside) = (0u64, 0u64);
                for y in 0..h {
                    let (a, b) = (&before[y * w * 4..(y + 1) * w * 4], &now[y * w * 4..(y + 1) * w * 4]);
                    if a == b {
                        continue;
                    }
                    for x in 0..w {
                        if a[x * 4..x * 4 + 4] != b[x * 4..x * 4 + 4] {
                            let covered = rects.is_none_or(|rs| rs.iter().any(|r| (x as i32) >= r.left && (x as i32) < r.right && (y as i32) >= r.top && (y as i32) < r.bottom));
                            if covered {
                                inside += 1;
                            } else {
                                outside += 1;
                            }
                        }
                    }
                }
                if inside + outside > 0 {
                    rep.changed += 1;
                }
                if outside > 0 {
                    rep.violations += 1;
                }
                rep.inside += inside;
                rep.outside += outside;
            }
            prev = Some(now);
        }
        rep.secs = start.elapsed().as_secs_f64();
        Ok(rep)
    }
}
