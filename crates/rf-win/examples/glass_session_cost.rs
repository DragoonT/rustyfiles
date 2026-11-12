//! Windowless measurement of what the glass's screen capture costs, per thread of THIS process. No window is created and nothing is
//! shown or changed: it opens the engine's own capture session on a monitor (the code of `wgc.rs`, through the `rf_win::glass::check`
//! module), runs the engine's turn for a while (wait for a frame, test what it touched, prepare + read back + draw one emulated glass
//! window into a private texture) and prints, for every thread of its own process, the context switches a second and the CPU time,
//! the way `thread_wakes` and `thread_cpu` read another process (NtQuerySystemInformation, read only).
//!
//!   cargo run --release -p rf-win --example glass_session_cost -- run  [--secs 10] [--legacy | --interval refresh|battery|MS] [--window X,Y,W,H]
//!                                                                      [--wait 50] [--buffers 2] [--switch] [--drive MS] [--monitor X,Y]
//!   cargo run --release -p rf-win --example glass_session_cost -- force [--trials 10]
//!   cargo run --release -p rf-win --example glass_session_cost -- resume [--pause 3000]  (a session used again after a pause: stale frames?)
//!   cargo run --release -p rf-win --example glass_session_cost -- dirty [--secs 30] [--interval refresh|battery|MS]   [--dup [--slow]]   (do the dirty rectangles cover every change? --dup: Desktop Duplication, --slow: with the gap of the not-in-front pace)
//!   cargo run --release -p rf-win --example glass_session_cost -- device [--secs 10]    (a D3D device with the renderer, no session)
//!   cargo run --release -p rf-win --example glass_session_cost -- idle  [--secs 10]     (a device and nothing else: the baseline)
//!   cargo run --release -p rf-win --example glass_session_cost -- copy                  (GPU time of a whole-monitor copy against a rectangle's)
//! `--drive MS`: asks for a frame every MS milliseconds (the exclusion list is sent again), a source of frames on a still desktop.
//! `--monitor X,Y`: capture the monitor that holds this desktop point (default 0,0: the primary one).
//!
//! `--legacy`: what the engine did before the cap: one refresh period as the interval. Otherwise the engine's present policy (60 frames
//! a second, 30 on battery). Every frame that is taken is copied whole into the mirror in both.
//! `--interval`: `refresh` (the old interval), `battery` (the policy's battery interval), or a number of milliseconds.
//! `--switch`: half way through the session is set to the battery interval, to see that a RUNNING session takes a new interval.
//! `--window`: the emulated glass window in the primary monitor's pixels (default 150,60,1100,640).

use std::time::{Duration, Instant};

use rf_win::glass::check::{self, ProbeInterval, ProbeOpts};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::LibraryLoader::{GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW, GetModuleHandleExW};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetThreadDescription, OpenThread, THREAD_QUERY_INFORMATION, THREAD_QUERY_LIMITED_INFORMATION};
use windows::core::PCWSTR;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtQuerySystemInformation(class: u32, buf: *mut u8, len: u32, ret: *mut u32) -> i32;
    fn NtQueryInformationThread(h: HANDLE, class: u32, info: *mut usize, len: u32, ret: *mut u32) -> i32;
}

#[derive(Clone, Copy)]
struct Row {
    tid: u64,
    switches: u32,
    cpu_100ns: u64,
}

/// Every thread of this process: (thread id, context switches, kernel + user time).
fn sample() -> Vec<Row> {
    let pid = unsafe { GetCurrentProcessId() } as u64;
    let mut buf = vec![0u8; 4 << 20];
    loop {
        let mut ret = 0u32;
        let st = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr(), buf.len() as u32, &mut ret) };
        if st == 0xC0000004u32 as i32 {
            buf = vec![0u8; ret as usize + (1 << 20)];
            continue;
        }
        assert!(st >= 0, "NtQuerySystemInformation failed: {st:#x}");
        break;
    }
    let rd32 = |o: usize| u32::from_le_bytes(buf[o..o + 4].try_into().unwrap());
    let rd64 = |o: usize| u64::from_le_bytes(buf[o..o + 8].try_into().unwrap());
    let mut off = 0usize;
    let mut out = vec![];
    loop {
        let next = rd32(off) as usize;
        let n = rd32(off + 4) as usize;
        if rd64(off + 80) == pid {
            for i in 0..n {
                let t = off + 256 + i * 80;
                out.push(Row { tid: rd64(t + 48), switches: rd32(t + 64), cpu_100ns: rd64(t) + rd64(t + 8) });
            }
            break;
        }
        if next == 0 {
            break;
        }
        off += next;
    }
    out
}

/// The name a thread gave itself and the module its start address is in.
fn describe(tid: u64) -> (String, String) {
    unsafe {
        let Ok(h) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION | THREAD_QUERY_INFORMATION, false, tid as u32) else { return ("(gone)".into(), String::new()) };
        let name = GetThreadDescription(h).ok().and_then(|p| p.to_string().ok()).unwrap_or_default();
        let mut addr = 0usize;
        NtQueryInformationThread(h, 9, &mut addr, 8, std::ptr::null_mut());
        let _ = CloseHandle(h);
        let mut module = String::new();
        if addr != 0 {
            let mut hm = Default::default();
            if GetModuleHandleExW(GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, PCWSTR(addr as *const u16), &mut hm).is_ok() {
                let mut path = [0u16; 260];
                let n = GetModuleFileNameW(Some(hm), &mut path) as usize;
                module = String::from_utf16_lossy(&path[..n]).rsplit('\\').next().unwrap_or("").to_string();
            }
        }
        (name, module)
    }
}

static mut BEFORE: Option<(Vec<Row>, Instant)> = None;
static mut AFTER: Option<(Vec<Row>, Instant)> = None;
/// Names and start modules of the threads, read while the session is still open (the threads of the capture service end with it).
static mut NAMES: Vec<(u64, String, String)> = Vec::new();

fn hook(stage: &'static str) {
    // single threaded use: the hook is called on the thread that runs the example's main
    unsafe {
        match stage {
            "opened" => BEFORE = Some((sample(), Instant::now())),
            "finished" => {
                let rows = sample();
                #[allow(static_mut_refs)]
                {
                    NAMES = rows.iter().map(|r| {
                        let (n, m) = describe(r.tid);
                        (r.tid, n, m)
                    }).collect();
                }
                AFTER = Some((rows, Instant::now()));
            }
            _ => {}
        }
    }
}

fn print_threads(label: &str) {
    #[allow(static_mut_refs)]
    let (Some((a, t0)), Some((b, t1))) = (unsafe { BEFORE.clone() }, unsafe { AFTER.clone() }) else { return };
    let el = (t1 - t0).as_secs_f64();
    let mut rows: Vec<(f64, f64, u64)> = b
        .iter()
        .map(|r| {
            let (s0, c0) = a.iter().find(|x| x.tid == r.tid).map_or((0, 0), |x| (x.switches, x.cpu_100ns));
            (r.switches.wrapping_sub(s0) as f64 / el, r.cpu_100ns.saturating_sub(c0) as f64 / 10_000.0, r.tid)
        })
        .collect();
    rows.sort_by(|x, y| y.0.total_cmp(&x.0));
    let total_sw: f64 = rows.iter().map(|r| r.0).sum();
    let total_cpu: f64 = rows.iter().map(|r| r.1).sum();
    println!("[{label}] threads over {el:.1} s (after the session was open): {} threads, all together {total_sw:.0} context switches/s and {total_cpu:.1} ms CPU ({:.2} ms/s)", rows.len(), total_cpu / el);
    println!("  ctx/s     cpu ms   thread   name / start module");
    for (sw, cpu, tid) in rows.iter().filter(|r| r.0 >= 3.0 || r.1 >= 3.0).take(14) {
        #[allow(static_mut_refs)]
        let (name, module) = unsafe { NAMES.iter().find(|n| n.0 == *tid).map(|n| (n.1.clone(), n.2.clone())).unwrap_or_default() };
        println!("{sw:>7.1} {cpu:>10.1} {tid:>8}   {} {}", if name.is_empty() { "(unnamed)" } else { &name }, module);
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("run");
    let secs: f64 = arg(&args, "--secs").and_then(|s| s.parse().ok()).unwrap_or(10.0);
    let _apartment = check::enter_apartment();
    match cmd {
        "copy" => {
            for (name, r) in [("the window's region (1536 x 1024)", (150u32, 60u32, 1536u32, 1024u32)), ("a dock island's region (1024 x 512)", (700, 1100, 1024, 512)), ("a clock (300 x 48)", (2200, 1380, 300, 48))] {
                match check::bench_copy((0, 0), r, 4000) {
                    Ok((whole, part)) => println!("copy of {name}: {part:.1} us each, against {whole:.1} us for the whole monitor ({:.1}x)", whole / part),
                    Err(e) => println!("not run: {e}"),
                }
            }
        }
        "device" => {
            if let Err(e) = check::hold_device(secs, (0, 0), Some(hook)) {
                println!("not run: {e}");
            }
            print_threads("a D3D11 device and the renderer, no session");
        }
        "idle" => {
            // a device and a WinRT apartment, no session
            std::thread::sleep(Duration::from_millis(200));
            unsafe {
                BEFORE = Some((sample(), Instant::now()));
            }
            std::thread::sleep(Duration::from_secs_f64(secs));
            unsafe {
                AFTER = Some((sample(), Instant::now()));
            }
            print_threads("no session");
        }
        "resume" => {
            let pause: u64 = arg(&args, "--pause").and_then(|s| s.parse().ok()).unwrap_or(3000);
            match check::probe_resume(Duration::from_millis(pause), Duration::from_millis(1500), (0, 0)) {
                Ok(r) => {
                    println!(
                        "session left alone for {pause} ms with a frame in its pool (queued: {}); after the resume: ready right away {} (must be false), the old frame was accepted {} (must be false), the first picture made after it arrived after {:?} ms",
                        r.frame_was_queued, r.ready_right_after, r.old_frame_accepted, r.fresh_ms.map(|m| (m * 10.0).round() / 10.0)
                    );
                }
                Err(e) => println!("not run: {e}"),
            }
        }
        "dirty" => {
            let secs: f64 = arg(&args, "--secs").and_then(|s| s.parse().ok()).unwrap_or(30.0);
            let interval = match arg(&args, "--interval").as_deref() {
                Some("refresh") => ProbeInterval::Refresh,
                Some("battery") => ProbeInterval::Policy { on_battery: true },
                Some(ms) => ProbeInterval::Millis(ms.parse().unwrap_or(16.7)),
                None => ProbeInterval::Policy { on_battery: false },
            };
            // --dup: Desktop Duplication (the fallback backend) instead of a capture session; --slow adds the gap of the "not in front" pace
            let dup = args.iter().any(|a| a == "--dup");
            let result = if dup { check::probe_dirty_dup(secs, args.iter().any(|a| a == "--slow"), (0, 0)) } else { check::probe_dirty(secs, interval, (0, 0)) };
            match result {
                Ok(r) => println!(
                    "{:.1} s at an interval of {:?} ms: {} pictures, {} differed from the one before, {} of those reported a bounded list of rectangles; changed pixels inside the reported rectangles {}, OUTSIDE {} (in {} pictures; must be 0)",
                    r.secs, r.interval_ms, r.images, r.changed, r.bounded, r.inside, r.outside, r.violations
                ),
                Err(e) => println!("not run: {e}"),
            }
        }
        "force" => {
            let trials: u32 = arg(&args, "--trials").and_then(|s| s.parse().ok()).unwrap_or(10);
            match check::probe_force(trials * 2, Duration::from_millis(1500), (0, 0)) {
                Ok(r) => {
                    let fmt = |v: &Vec<Option<f64>>| v.iter().map(|x| x.map_or("none".to_string(), |m| format!("{m:.1}"))).collect::<Vec<_>>().join(" ");
                    println!("frame after the list was sent again (ms): {}", fmt(&r.forced_ms));
                    println!("frame with nothing sent, same wait    (ms): {}", fmt(&r.control_ms));
                }
                Err(e) => println!("not run: {e}"),
            }
        }
        _ => {
            let legacy = args.iter().any(|a| a == "--legacy");
            let interval = match arg(&args, "--interval").as_deref() {
                _ if legacy => ProbeInterval::Refresh,
                Some("refresh") => ProbeInterval::Refresh,
                Some("battery") => ProbeInterval::Policy { on_battery: true },
                Some(ms) => ProbeInterval::Millis(ms.parse().unwrap_or(16.7)),
                None => ProbeInterval::Policy { on_battery: false },
            };
            let window = match arg(&args, "--window") {
                Some(w) => {
                    let v: Vec<i32> = w.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    (v.len() == 4).then(|| (v[0], v[1], v[2] as u32, v[3] as u32))
                }
                None => Some((150, 60, 1100, 640)),
            };
            let opts = ProbeOpts {
                secs,
                wait_ms: arg(&args, "--wait").and_then(|s| s.parse().ok()).unwrap_or(50),
                interval,
                buffers: arg(&args, "--buffers").and_then(|s| s.parse().ok()).unwrap_or(2),
                window,
                drive_ms: arg(&args, "--drive").and_then(|s| s.parse().ok()),
                switch_power_at_half: args.iter().any(|a| a == "--switch"),
                monitor_at: arg(&args, "--monitor").and_then(|m| {
                    let v: Vec<i32> = m.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                    (v.len() == 2).then(|| (v[0], v[1]))
                }).unwrap_or((0, 0)),
                hook: Some(hook),
            };
            let cps = check::cycles_per_second(200);
            match check::probe(&opts) {
                Ok(r) => {
                    let label = if legacy { "legacy: refresh-rate interval" } else { "now" };
                    println!(
                        "[{label}] monitor {:?}; interval {:?} ms (after switch {:?}); {:.1} s wall: {} turns, {} frames announced (handler ran), {} taken, {} refreshed the mirror ({:.1}/s)",
                        r.monitor, r.interval_ms, r.interval_ms_after, r.wall.as_secs_f64(), r.turns, r.arrived, r.frames, r.images, r.images as f64 / r.wall.as_secs_f64()
                    );
                    if opts.switch_power_at_half {
                        println!("  frames taken in the first half {}, in the second half (battery interval) {}", r.frames_first_half, r.frames_second_half);
                    }
                    println!(
                        "  emulated window: {} draws prepared ({} presented, {} skipped by the loop guard); the loop thread used {:.1} ms CPU ({:.2} ms/s)",
                        r.prepared, r.presented, r.skipped, r.thread_cpu.as_secs_f64() * 1000.0, r.thread_cpu.as_secs_f64() * 1000.0 / r.wall.as_secs_f64()
                    );
                    let ms = |c: u64| c as f64 / cps * 1000.0;
                    println!(
                        "  exact CPU by cycle counts ({:.2} GHz): the loop thread {:.1} ms ({:.2} ms/s), the whole process {:.1} ms ({:.2} ms/s), {:.0} us per frame taken",
                        cps / 1e9, ms(r.thread_cycles), ms(r.thread_cycles) / r.wall.as_secs_f64(), ms(r.process_cycles), ms(r.process_cycles) / r.wall.as_secs_f64(),
                        ms(r.thread_cycles) * 1000.0 / r.frames.max(1) as f64
                    );
                    println!("  video memory of this process on the adapter: {:.1} MiB before, {:.1} MiB with the session open", r.vram[0] as f64 / 1048576.0, r.vram[1] as f64 / 1048576.0);
                    print_threads(label);
                }
                Err(e) => println!("not run: {e}"),
            }
        }
    }
}
