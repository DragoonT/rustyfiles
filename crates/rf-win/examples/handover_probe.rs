//! Windowless probe of the hand-over of a launch to the running rustyfiles (GitHub issue 15). Nothing here creates a window.
//!
//! `handover_probe [old|new] [N]` runs the scenarios below with the real pipe and mutex code of `rf_win::single_instance`
//! (`old`: what 0.6.5 does) or `rf_win::handover` (`new`), against a "first instance" that is a child process of this probe
//! (a real process: its mutex and pipe go when it ends) whose "UI thread" is modelled on the app's: a loop that acts on the
//! requests that arrive and, on "the last window closed", goes through the quit the app goes through (settings save and name
//! index stop block the UI thread, GPUI's quit is a task queued behind the tasks already queued, the process ends after that).
//! The group is private (RUSTYFILES_INSTANCE and RUSTYFILES_CONFIG_DIR are set here), so nothing of the user's is touched.
//!
//! N second launches are started at evenly spread moments around the event of the scenario. Each ends as one of
//!   SHOWN    a window or tab with its folder is there and stays (acted on by a first instance that lives on, or shown by the
//!            second launch itself, or by a process that took over)
//!   FLICKER  acted on by a first instance that was already quitting: the window opens and the quit closes it
//!   LOST     "forwarded" and never acted on by anybody
//!   DOUBLE   acted on by the first instance AND shown by the second launch itself: two windows for one request
//!
//! What this does NOT show: GPUI itself. The quit's timing is a model (the numbers are arguments); that GPUI's quit is a queued
//! task is read from its source (`quit` spawns a foreground task that calls PostQuitMessage).

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rf_win::handover;
use rf_win::single_instance::{self, Role};

fn epoch_ms() -> u128 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn log_path() -> PathBuf {
    PathBuf::from(std::env::var_os("HANDOVER_PROBE_LOG").expect("HANDOVER_PROBE_LOG"))
}

fn log(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let _ = f.write_all(format!("{} {line}\n", epoch_ms()).as_bytes());
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Proto {
    Old,
    New,
}

/// What happens in the first instance, and when (ms after it is ready).
#[derive(Clone, Copy)]
struct Plan {
    /// The last window closes: the quit starts.
    close_at: Option<u64>,
    /// The UI thread is blocked this long by the quit (settings save, name index stop).
    save_ms: u64,
    /// The process lives this long after GPUI's quit task before it is gone.
    teardown_ms: u64,
    /// The UI thread is busy (a frozen window) from this moment for this long, no quit.
    busy: Option<(u64, u64)>,
    /// No window at all and none expected to close (the tray, or the dock's search helper): requests open one.
    windowless: bool,
}

// ---------------------------------------------------------------------------------------------------------------------------
// the first instance (child process)
// ---------------------------------------------------------------------------------------------------------------------------

struct Req {
    text: String,
    ack: Option<handover::Ack>,
}

fn run_first(proto: Proto, plan: Plan) {
    let role = match proto {
        Proto::Old => single_instance::acquire("first"),
        Proto::New => handover::acquire("first"),
    };
    let Role::Primary(_guard) = role else { std::process::exit(3) };
    let (tx, rx) = mpsc::channel::<Req>();
    let tx = Mutex::new(tx);
    match proto {
        Proto::Old => single_instance::serve(move |m| {
            let _ = tx.lock().unwrap().send(Req { text: m, ack: None });
        }),
        Proto::New => handover::serve(move |m, ack| {
            let _ = tx.lock().unwrap().send(Req { text: m, ack: Some(ack) });
        }),
    }
    let start = Instant::now();
    log(&format!("READY {}", epoch_ms()));
    let mut closed = false;
    let mut busy_done = plan.busy.is_none();
    // whether a main window is open; a request acted on while there is none opens one (and then nothing closes it in the scenarios)
    let mut window_open = !plan.windowless;
    let mut stays = plan.windowless;
    let handle = |r: Req, dying: bool, window_open: &mut bool, stays: &mut bool| {
        log(&format!("HANDLED {} {}", r.text, if dying { "dying" } else { "alive" }));
        if !dying && !*window_open {
            *window_open = true;
            *stays = true;
        }
        if let Some(ack) = r.ack
            && ack.done()
        {
            log("quit looked at again");
        }
    };
    loop {
        let now = start.elapsed().as_millis() as u64;
        if let Some((at, len)) = plan.busy
            && !busy_done
            && now >= at
        {
            log("BUSY begins");
            std::thread::sleep(Duration::from_millis(len));
            busy_done = true;
            log("BUSY ends");
        }
        if let Some(at) = plan.close_at
            && !closed
            && !stays
            && now >= at
        {
            closed = true;
            window_open = false;
            log("LAST WINDOW CLOSED");
            let go = match proto {
                Proto::Old => true,
                Proto::New => handover::GATE.try_leave(),
            };
            if !go {
                // the request in flight is acted on by the next turn of the loop: it opens a window, which keeps the process
                log("quit deferred: a request is in flight");
            } else {
                log("QUIT begins (UI thread blocked)");
                std::thread::sleep(Duration::from_millis(plan.save_ms));
                // GPUI's quit is a task behind the tasks already queued: those run first
                while let Ok(r) = rx.try_recv() {
                    handle(r, true, &mut window_open, &mut stays);
                }
                log("QUIT task queued, process tearing down");
                std::thread::sleep(Duration::from_millis(plan.teardown_ms));
                log("EXIT");
                std::process::exit(0);
            }
        }
        if let Ok(r) = rx.recv_timeout(Duration::from_millis(2)) {
            handle(r, false, &mut window_open, &mut stays);
        }
        if start.elapsed() > Duration::from_secs(60) {
            std::process::exit(4);
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------------
// the second launches (this process)
// ---------------------------------------------------------------------------------------------------------------------------

/// What the second launches of this process that became a first instance keep: their guards, and the serving started once.
static TAKEN: Mutex<Vec<single_instance::InstanceGuard>> = Mutex::new(Vec::new());
static SERVING: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// The requests the taker has acted on.
static TAKER_HANDLED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn serve_as_taker(proto: Proto) {
    // once per group (a pipe thread of an earlier scenario goes on serving its own group's name; groups differ per scenario)
    let group = std::env::var("RUSTYFILES_INSTANCE").unwrap_or_default();
    {
        let mut served = SERVING.lock().unwrap();
        if served.contains(&group) {
            return;
        }
        served.push(group);
    }
    match proto {
        Proto::Old => single_instance::serve(|m| TAKER_HANDLED.lock().unwrap().push(m)),
        Proto::New => handover::serve(|m, ack| {
            TAKER_HANDLED.lock().unwrap().push(m);
            ack.done();
        }),
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Outcome {
    Shown,
    Flicker,
    Lost,
    Double,
}

struct Row {
    scenario: String,
    shown: usize,
    flicker: usize,
    lost: usize,
    double: usize,
    took_ms: Vec<u128>,
}

fn run_scenario(exe: &Path, proto: Proto, name: &str, plan: Plan, n: usize, spread: (i64, i64)) -> Row {
    let log = std::env::temp_dir().join(format!("rf-handover-probe-{}-{}.log", std::process::id(), name.replace(|c: char| !c.is_ascii_alphanumeric(), "_")));
    let _ = std::fs::remove_file(&log);
    // SAFETY: set before the scenario's threads exist (the scenarios run one after the other)
    unsafe {
        std::env::set_var("HANDOVER_PROBE_LOG", &log);
        // a group of its own for each scenario: nothing of an earlier scenario (a taker still serving its pipe) can answer in this one
        let base = std::env::var("HANDOVER_PROBE_GROUP").unwrap();
        std::env::set_var("RUSTYFILES_INSTANCE", format!("{base}-{}", name.replace(|c: char| !c.is_ascii_alphanumeric(), "")));
    }
    TAKER_HANDLED.lock().unwrap().clear();
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("first")
        .arg(if proto == Proto::Old { "old" } else { "new" })
        .arg(plan.close_at.map_or("-".into(), |v| v.to_string()))
        .arg(plan.save_ms.to_string())
        .arg(plan.teardown_ms.to_string())
        .arg(plan.busy.map_or("-".into(), |b| format!("{},{}", b.0, b.1)))
        .arg(if plan.windowless { "w" } else { "-" });
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut child = cmd.spawn().expect("spawn the first instance");
    // ready?
    let begun = Instant::now();
    let ready_at = loop {
        if let Some(l) = std::fs::read_to_string(&log).ok().and_then(|t| t.lines().find(|l| l.contains(" READY ")).map(str::to_string)) {
            break l.split_whitespace().next().and_then(|v| v.parse::<u128>().ok()).unwrap_or_else(epoch_ms);
        }
        assert!(begun.elapsed() < Duration::from_secs(10), "the first instance never got ready (it ended with {:?}; log {})", child.try_wait(), log.display());
        std::thread::sleep(Duration::from_millis(5));
    };
    // second launches, evenly spread over `spread` ms around the first instance's close (or from ready when there is none)
    let base = plan.close_at.unwrap_or(plan.busy.map_or(0, |b| b.0)) as i64;
    let (results_tx, results_rx) = mpsc::channel::<(usize, bool, u128)>();
    let mut threads = vec![];
    for i in 0..n {
        let offset = base + spread.0 + (spread.1 - spread.0) * i as i64 / n.max(1) as i64;
        let at = ready_at as i64 + offset.max(0);
        let tx = results_tx.clone();
        threads.push(std::thread::spawn(move || {
            let now = epoch_ms() as i64;
            if at > now {
                std::thread::sleep(Duration::from_millis((at - now) as u64));
            }
            let t = Instant::now();
            let msg = format!("c{i}");
            let role = match proto {
                Proto::Old => single_instance::acquire(&msg),
                Proto::New => handover::acquire(&msg),
            };
            let (became_first, ms) = (matches!(role, Role::Primary(_)), t.elapsed().as_millis());
            if let Role::Primary(g) = role {
                // what the app does: shows the folder (counted as acted on), and serves
                TAKER_HANDLED.lock().unwrap().push(msg);
                TAKEN.lock().unwrap().push(g);
                serve_as_taker(proto);
            }
            let _ = tx.send((i, became_first, ms));
        }));
    }
    drop(results_tx);
    for t in threads {
        let _ = t.join();
    }
    let results: Vec<(usize, bool, u128)> = results_rx.try_iter().collect();
    // the first instance ends by itself in the scenarios that close it; the others are ended here
    let waited = Instant::now();
    while waited.elapsed() < Duration::from_secs(8) && child.try_wait().ok().flatten().is_none() {
        // (a first instance that never closes is left alone for a moment after the last launch, long enough for a busy UI thread
        // to catch up with what was sent while it was busy)
        let busy_end = plan.busy.map_or(0, |b| b.0 + b.1);
        if plan.close_at.is_none() && epoch_ms() as i64 > ready_at as i64 + busy_end as i64 + 600 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    if std::env::var_os("HANDOVER_PROBE_SHOW").is_some_and(|v| name.contains(v.to_string_lossy().as_ref())) {
        let t0 = ready_at;
        eprintln!("--- {name}: the first instance's log (ms after ready)");
        for l in text.lines() {
            let mut p = l.splitn(2, ' ');
            let at: u128 = p.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            if !l.contains("HANDLED") {
                eprintln!("{:>6} {}", at as i128 - t0 as i128, p.next().unwrap_or(""));
            }
        }
        let mut t = results.iter().map(|r| r.2).collect::<Vec<_>>();
        t.sort();
        eprintln!("durations of the second launches: {t:?}");
    }
    let mut row = Row { scenario: name.to_string(), shown: 0, flicker: 0, lost: 0, double: 0, took_ms: vec![] };
    let taker = TAKER_HANDLED.lock().unwrap().clone();
    for (i, _, ms) in &results {
        let id = format!("c{i}");
        let by_first = text.lines().find_map(|l| {
            let mut p = l.split_whitespace();
            (p.nth(1) == Some("HANDLED") && p.next() == Some(id.as_str())).then(|| l.ends_with("dying"))
        });
        let outcome = match by_first {
            Some(_) if taker.contains(&id) => Outcome::Double,
            Some(true) => Outcome::Flicker,
            Some(false) => Outcome::Shown,
            None if taker.contains(&id) => Outcome::Shown,
            None => Outcome::Lost,
        };
        match outcome {
            Outcome::Shown => row.shown += 1,
            Outcome::Flicker => row.flicker += 1,
            Outcome::Lost => row.lost += 1,
            Outcome::Double => row.double += 1,
        }
        row.took_ms.push(*ms);
    }
    // a taker of an earlier scenario must not hold this group's mutex into the next one
    TAKEN.lock().unwrap().clear();
    std::thread::sleep(Duration::from_millis(150));
    let _ = std::fs::remove_file(&log);
    row
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // the private group: nothing of the user's (or of another test's) is in it
    // SAFETY: first thing in main, before any thread exists
    unsafe {
        // (the first instance, a child, inherits its scenario's group from the parent)
        if args.first().map(String::as_str) != Some("first") {
            std::env::set_var("RUSTYFILES_INSTANCE", format!("probe{}", std::process::id()));
        }
        if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_none() {
            std::env::set_var("RUSTYFILES_CONFIG_DIR", std::env::temp_dir().join("rf-handover-probe-config"));
        }
    }
    if args.first().map(String::as_str) == Some("first") {
        let proto = if args[1] == "old" { Proto::Old } else { Proto::New };
        let num = |s: &str| s.parse::<u64>().ok();
        let plan = Plan {
            close_at: num(&args[2]),
            save_ms: num(&args[3]).unwrap_or(0),
            teardown_ms: num(&args[4]).unwrap_or(0),
            busy: args[5].split_once(',').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))),
            windowless: args[6] == "w",
        };
        run_first(proto, plan);
        return;
    }
    let proto = if args.first().map(String::as_str) == Some("old") { Proto::Old } else { Proto::New };
    let n: usize = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(40);
    // the group is the same for the parent and the children it starts
    unsafe { std::env::set_var("HANDOVER_PROBE_GROUP", std::env::var("RUSTYFILES_INSTANCE").unwrap()) };
    let exe = std::env::current_exe().unwrap();
    println!("protocol: {}, {n} second launches per scenario", if proto == Proto::Old { "old (0.6.5: write and leave)" } else { "new (acknowledged hand-over)" });
    let scenarios: Vec<(&str, Plan, (i64, i64))> = vec![
        ("idle, window open", Plan { close_at: None, save_ms: 0, teardown_ms: 0, busy: None, windowless: false }, (50, 450)),
        ("no window (tray / helper)", Plan { close_at: None, save_ms: 0, teardown_ms: 0, busy: None, windowless: true }, (50, 450)),
        ("busy 1.5 s (UI blocked)", Plan { close_at: None, save_ms: 0, teardown_ms: 0, busy: Some((200, 1500)), windowless: false }, (-100, 1000)),
        ("busy 10 s (past the old 8 s limit)", Plan { close_at: None, save_ms: 0, teardown_ms: 0, busy: Some((200, 10_000)), windowless: false }, (-100, 1000)),
        ("closing: quit 30+150 ms", Plan { close_at: Some(400), save_ms: 30, teardown_ms: 150, busy: None, windowless: false }, (-150, 450)),
        ("closing: quit 400+300 ms", Plan { close_at: Some(400), save_ms: 400, teardown_ms: 300, busy: None, windowless: false }, (-150, 900)),
        ("closing: index stop 2000+300 ms", Plan { close_at: Some(400), save_ms: 2000, teardown_ms: 300, busy: None, windowless: false }, (-150, 2500)),
    ];
    let mut rows = vec![];
    for (name, plan, spread) in scenarios {
        rows.push(run_scenario(&exe, proto, name, plan, n, spread));
    }
    // two launches at the same moment, nobody running
    println!();
    println!("{:<34} {:>6} {:>8} {:>6} {:>7}   second launch lasted (ms): median / max", "scenario", "SHOWN", "FLICKER", "LOST", "DOUBLE");
    for r in &rows {
        let mut t = r.took_ms.clone();
        t.sort();
        println!("{:<34} {:>6} {:>8} {:>6} {:>7}   {} / {}", r.scenario, r.shown, r.flicker, r.lost, r.double, t.get(t.len() / 2).copied().unwrap_or(0), t.last().copied().unwrap_or(0));
    }
}
