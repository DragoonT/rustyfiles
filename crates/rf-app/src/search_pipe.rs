//! Files for the dock's search, from the running rustyfiles.
//!
//! The name index (every file name of every drive, tens of MB) is loaded by the main rustyfiles only; the dock process deliberately
//! does not load it (`background::DOCK_PROCESS`). When the dock's search needs files it asks the main app over a pipe of its own
//! (`<instance name>-search`, served by the main app next to its instance pipe), and the answer comes back over the dock's control
//! pipe (these pipes are one-way, so there are two):
//!
//! ```text
//! dock -> main   \x01qs-files <TAB> id <TAB> query
//! main -> dock   \x01qs-files-reply <TAB> id <TAB> ok|building|off <TAB> n <TAB> (score <TAB> d|f <TAB> path){n}
//! ```
//!
//! One line each, the fields separated by tabs; a tab, a line break or a backslash inside a field is written `\t`, `\n`, `\r`, `\\`.
//! WHY A PIPE OF ITS OWN. The ordinary instance pipe turns every line it does not know into a launch argument: a main app from
//! before this protocol would open an error tab and raise its window for every keystroke typed in the dock. Only a main app that
//! knows the protocol serves the search pipe; with an older one running the dock finds no pipe and says "no main app". The ranking
//! is `quick_search::search_files` itself: the same function answers the quick search window and this.
//!
//! Nothing is answered unless asked, a request that a newer one has overtaken is not searched, and no request is sent at all unless
//! the user has typed something into the dock's search.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const REQUEST: &str = "\u{1}qs-files";
const REPLY: &str = "\u{1}qs-files-reply";

/// A longest path worth sending: the whole message must stay well below the pipe reader's 64 KB.
const MAX_PATH_CHARS: usize = 4_000;

/// "Find files for this query."
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub id: u64,
    pub query: String,
}

/// How far the main app's file index is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Index {
    /// At least one drive is indexed and none is being built.
    Ready,
    /// Still being built (or not started yet): files may be missing.
    Building,
    /// Turned off in Settings.
    Off,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    pub score: i32,
    pub dir: bool,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reply {
    pub id: u64,
    pub index: Index,
    pub hits: Vec<Hit>,
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> Option<String> {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        out.push(match it.next()? {
            '\\' => '\\',
            't' => '\t',
            'n' => '\n',
            'r' => '\r',
            _ => return None,
        });
    }
    Some(out)
}

pub fn encode_request(r: &Request) -> String {
    format!("{REQUEST}\t{}\t{}", r.id, escape(&r.query))
}

/// Whether a message on the main app's pipe is this request (cheap: the caller checks it before anything else).
pub fn is_request(msg: &str) -> bool {
    msg.strip_prefix(REQUEST).is_some_and(|rest| rest.starts_with('\t'))
}

pub fn decode_request(msg: &str) -> Option<Request> {
    let rest = msg.strip_prefix(REQUEST)?.strip_prefix('\t')?;
    let (id, query) = rest.split_once('\t')?;
    Some(Request { id: id.parse().ok()?, query: unescape(query)? })
}

pub fn encode_reply(r: &Reply) -> String {
    let state = match r.index {
        Index::Ready => "ok",
        Index::Building => "building",
        Index::Off => "off",
    };
    let hits: Vec<&Hit> = r.hits.iter().filter(|h| h.path.to_string_lossy().chars().count() <= MAX_PATH_CHARS).collect();
    let mut s = format!("{REPLY}\t{}\t{state}\t{}", r.id, hits.len());
    for h in hits {
        s.push_str(&format!("\t{}\t{}\t{}", h.score, if h.dir { 'd' } else { 'f' }, escape(&h.path.to_string_lossy())));
    }
    s
}

/// Whether a message on the dock's pipe is a reply.
pub fn is_reply(msg: &str) -> bool {
    msg.strip_prefix(REPLY).is_some_and(|rest| rest.starts_with('\t'))
}

pub fn decode_reply(msg: &str) -> Option<Reply> {
    let rest = msg.strip_prefix(REPLY)?.strip_prefix('\t')?;
    let mut f = rest.split('\t');
    let id = f.next()?.parse().ok()?;
    let index = match f.next()? {
        "ok" => Index::Ready,
        "building" => Index::Building,
        "off" => Index::Off,
        _ => return None,
    };
    let n: usize = f.next()?.parse().ok()?;
    let mut hits = Vec::with_capacity(n.min(64));
    for _ in 0..n {
        let score = f.next()?.parse().ok()?;
        let dir = match f.next()? {
            "d" => true,
            "f" => false,
            _ => return None,
        };
        let path = PathBuf::from(unescape(f.next()?)?);
        hits.push(Hit { score, dir, path });
    }
    // anything left over is a message from a newer version: ignored, not an error
    Some(Reply { id, index, hits })
}

// ---------------------------------------------------------------- the dock's side

/// The id of the next request: increasing within a dock process, and larger than any a previous dock process used (it starts from the
/// clock), so the main app can tell a newer request from an older one by comparing.
pub fn next_id() -> u64 {
    static LAST: AtomicU64 = AtomicU64::new(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_millis() as u64);
    let mut prev = LAST.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev + 1);
        match LAST.compare_exchange(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(p) => prev = p,
        }
    }
}

/// The name of the pipe the main app serves for this protocol (a test copy with its own instance name has its own).
pub fn app_name() -> String {
    format!("{}-search", rf_win::single_instance::instance_name())
}

/// Asks the running main rustyfiles for files. False when none answers on its pipe (there is no main app, or one too old to know
/// this: the caller says so in the list instead). Blocks for a pipe write at most: a worker thread.
pub fn ask(r: &Request) -> bool {
    rf_win::single_instance::send_to(&app_name(), &encode_request(r))
}

/// Whether a main app is running (its pipe answers). Sends a request with an empty query, which the main app swallows without
/// searching or answering.
pub fn main_is_running() -> bool {
    ask(&Request { id: 0, query: String::new() })
}

/// `rustyfiles.exe --no-window`: the main app without a window, so that the file index is there for the dock's search. Started by the
/// dock when the user chooses the row that offers it. A second one finds the first and greets it with [`START_MSG`] (which the app
/// swallows) instead of opening a window.
pub const START_ARG: &str = "--no-window";
pub const START_MSG: &str = "\u{1}no-window";

// ---------------------------------------------------------------- the main app's side

/// Starts serving the search pipe; its lines go to `tx` (the app's message queue, which `route_ipc` reads on the UI thread).
pub fn serve(tx: futures::channel::mpsc::UnboundedSender<String>) {
    rf_win::single_instance::serve_app(&app_name(), move |m| {
        let _ = tx.unbounded_send(m);
    });
}

/// A main app that was started only for the dock's search ends itself after this long without a request, when it has no window.
pub const IDLE_QUIT_MS: u64 = 10 * 60 * 1000;
/// How often that is looked at (a sleeping task, only in a window-less search-only process).
const IDLE_CHECK: std::time::Duration = std::time::Duration::from_secs(60);

/// When the last request (or greeting) arrived, ms since 1970.
static LAST_ACTIVITY: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Something asked this process for files (or started it): the idle time begins again.
pub fn touch() {
    LAST_ACTIVITY.store(now_ms(), Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    /// Still wanted: look again later.
    Waiting,
    /// A window has been opened in this process: it is an ordinary rustyfiles now and ends the ordinary way.
    Windowed,
    /// Nobody has asked for files for long enough and there is no window: end the process.
    Over,
}

pub fn idle_verdict(has_window: bool, idle_ms: u64) -> Idle {
    if has_window {
        Idle::Windowed
    } else if idle_ms >= IDLE_QUIT_MS {
        Idle::Over
    } else {
        Idle::Waiting
    }
}

/// For `rustyfiles.exe --no-window`: no window, no tray icon and so no way for the user to close it, so it ends by itself after
/// ten minutes without a request for files (the dock starts it again when the user chooses that row). Stops watching as soon as a
/// window opens in the process.
pub fn watch_idle(cx: &mut gpui_kit::App) {
    touch();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(IDLE_CHECK).await;
            let idle = now_ms().saturating_sub(LAST_ACTIVITY.load(Ordering::Relaxed));
            let verdict = cx.update(|cx| idle_verdict(crate::workspace::any_main_window(cx), idle));
            match verdict {
                Idle::Waiting => {}
                Idle::Windowed => break,
                Idle::Over => {
                    cx.update(crate::quit_when_idle);
                    break;
                }
            }
        }
    })
    .detach();
}

/// The newest request seen (a request older than this is not worth searching for).
static NEWEST: AtomicU64 = AtomicU64::new(0);

/// Whether a request is overtaken: a newer one has been seen. Remembers this one as seen.
pub fn overtaken(id: u64) -> bool {
    let newest = NEWEST.fetch_max(id, Ordering::Relaxed);
    newest > id
}

/// The main app's answer to a request message; true when `msg` was one (and so must not be treated as a launch argument). The
/// search runs on a thread of its own: this returns at once.
pub fn handle_request(msg: &str, index_on: bool) -> bool {
    if !is_request(msg) {
        return false;
    }
    let Some(req) = decode_request(msg) else { return true };
    if req.query.trim().is_empty() || overtaken(req.id) {
        return true;
    }
    touch();
    std::thread::Builder::new()
        .name("rf-dock-files".into())
        .spawn(move || {
            // overtaken while waiting for the thread: the dock has asked again and wants that answer
            if overtaken(req.id) {
                return;
            }
            let found = crate::quick_search::search_files(&req.query, index_on);
            let index = if found.index_off {
                Index::Off
            } else if found.index_building {
                Index::Building
            } else {
                Index::Ready
            };
            let hits = found
                .items
                .into_iter()
                .filter_map(|i| match i.target {
                    crate::quick_search::Target::Path { path, dir } => Some(Hit { score: i.score, dir, path }),
                    _ => None,
                })
                .collect();
            let line = encode_reply(&Reply { id: req.id, index, hits });
            if !rf_win::single_instance::send_to(crate::dock::control::APP, &line) {
                log::debug!("dock search: the dock is gone, the answer for {} is dropped", req.id);
            }
        })
        .ok();
    true
}

/// The main app's handling of the private messages of this module (called first by the message router): true when `msg` was one and
/// must not be read as a launch argument.
pub fn handle_private(msg: &str, index_on: bool) -> bool {
    if msg.trim() == START_MSG {
        touch();
        return true;
    }
    handle_request(msg, index_on)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u64, q: &str) -> Request {
        Request { id, query: q.into() }
    }

    #[test]
    fn a_request_survives_the_wire_with_tabs_lines_slashes_and_turkish_letters() {
        for q in ["rapor", "Şirket Çıktıları", "a\tb", "satır\nsonu\r", "C:\\Users\\x\\", "\\t literal", "", "tırnak \"çift\" 'tek'", "*.psd", "emoji 😀"] {
            let r = req(1_700_000_000_123, q);
            let line = encode_request(&r);
            assert!(is_request(&line));
            assert!(!line.contains('\n') && !line.contains('\r'), "one line: {line:?}");
            assert_eq!(decode_request(&line), Some(r), "{q:?}");
        }
    }

    #[test]
    fn a_reply_survives_the_wire_with_every_state_and_odd_paths() {
        let hits = vec![
            Hit { score: 812, dir: false, path: PathBuf::from(r"C:\Users\Özgür\Belgeler\rapor 2026.docx") },
            Hit { score: -3, dir: true, path: PathBuf::from(r"D:\Proje\Çalışma Klasörü") },
            Hit { score: 5, dir: false, path: PathBuf::from("C:\\tab\there\\and\\back\\slash") },
        ];
        for index in [Index::Ready, Index::Building, Index::Off] {
            let r = Reply { id: 42, index, hits: hits.clone() };
            let line = encode_reply(&r);
            assert!(is_reply(&line) && !is_request(&line));
            assert!(!line.contains('\n'));
            assert_eq!(decode_reply(&line), Some(r));
        }
        let empty = Reply { id: 7, index: Index::Building, hits: vec![] };
        assert_eq!(decode_reply(&encode_reply(&empty)), Some(empty));
    }

    #[test]
    fn messages_that_are_not_ours_or_are_damaged_are_not_decoded() {
        assert!(!is_request("C:\\Users\\x"));
        assert!(!is_request("\u{1}select-here:C:\\x"));
        assert!(!is_request("\u{1}qs-files-reply\t1\tok\t0"), "a reply is not a request");
        assert!(!is_reply("\u{1}qs-files\t1\tq"));
        assert_eq!(decode_request("\u{1}qs-files\tx\tq"), None, "id is a number");
        assert_eq!(decode_request("\u{1}qs-files\t5"), None, "no query field");
        assert_eq!(decode_request("\u{1}qs-files\t5\tbad\\q"), None, "unknown escape");
        assert_eq!(decode_reply("\u{1}qs-files-reply\t1\tmaybe\t0"), None);
        // (a backslash is written doubled on the wire)
        assert_eq!(decode_reply("\u{1}qs-files-reply\t1\tok\t2\t5\tf\tC:\\\\a"), None, "two announced, one sent");
        assert_eq!(decode_reply("\u{1}qs-files-reply\t1\tok\t1\t5\tx\tC:\\\\a"), None, "unknown kind");
        assert_eq!(decode_reply("\u{1}qs-files-reply\t1\tok\t1\t5\tf\tC:\\a"), None, "a lone backslash is not an escape");
        assert_eq!(decode_reply("ping"), None);
        // extra fields from a newer version are ignored
        let r = decode_reply("\u{1}qs-files-reply\t1\tok\t1\t5\tf\tC:\\\\a\tmore\tstuff").unwrap();
        assert_eq!(r.hits.len(), 1);
    }

    #[test]
    fn a_path_too_long_for_the_pipe_is_left_out_not_sent() {
        let long = format!("C:\\{}", "x".repeat(MAX_PATH_CHARS + 10));
        let r = Reply { id: 1, index: Index::Ready, hits: vec![Hit { score: 1, dir: false, path: long.into() }, Hit { score: 2, dir: false, path: "C:\\ok".into() }] };
        let back = decode_reply(&encode_reply(&r)).unwrap();
        assert_eq!(back.hits.len(), 1);
        assert_eq!(back.hits[0].path, PathBuf::from("C:\\ok"));
        // seven full-size hits stay far below the 64 KB the pipe reader takes
        let seven = Reply { id: 1, index: Index::Ready, hits: (0..7).map(|i| Hit { score: i, dir: false, path: format!("C:\\{}", "ç".repeat(MAX_PATH_CHARS - 10)).into() }).collect() };
        assert!(encode_reply(&seven).len() < 64 * 1024);
    }

    #[test]
    fn ids_only_go_up() {
        let a = next_id();
        let b = next_id();
        let c = next_id();
        assert!(a < b && b < c);
        // a restarted process starts from the clock: larger than the ids of a process that ended a moment ago
        assert!(a > 1_000_000_000_000, "milliseconds since 1970");
    }

    #[test]
    fn an_older_request_is_overtaken_by_a_newer_one_and_a_newer_one_is_not() {
        // (a process-wide counter: this test uses ids far above the other tests')
        let base = u64::MAX / 2;
        assert!(!overtaken(base + 5));
        assert!(overtaken(base + 3), "5 has been seen");
        assert!(!overtaken(base + 5), "the newest itself is not overtaken");
        assert!(!overtaken(base + 9));
    }

    #[test]
    fn a_main_app_ignores_blank_requests_and_says_it_handled_requests_only() {
        assert!(!handle_request("C:\\a\\b", true), "a launch argument is not ours");
        assert!(handle_request(&encode_request(&req(1, "   ")), true), "ours, nothing to search");
        assert!(handle_request("\u{1}qs-files\tbroken", true), "ours but unreadable: swallowed, not opened as a path");
        assert!(handle_private(START_MSG, true), "the greeting of a second window-less start");
        assert!(!handle_private("shell:RecycleBinFolder", true));
    }

    #[test]
    fn the_search_pipe_is_not_the_instance_pipe_so_an_older_main_app_never_sees_a_request() {
        let name = app_name();
        assert!(name.ends_with("-search"));
        assert_ne!(name, rf_win::single_instance::instance_name());
        assert!(name.starts_with(&rf_win::single_instance::instance_name()));
    }

    #[test]
    fn a_window_less_search_process_ends_after_ten_idle_minutes_and_only_without_a_window() {
        assert_eq!(idle_verdict(false, 0), Idle::Waiting);
        assert_eq!(idle_verdict(false, IDLE_QUIT_MS - 1), Idle::Waiting);
        assert_eq!(idle_verdict(false, IDLE_QUIT_MS), Idle::Over);
        // somebody opened a window in it: it is a normal rustyfiles now, however long ago the last search was
        assert_eq!(idle_verdict(true, IDLE_QUIT_MS * 5), Idle::Windowed);
        touch();
        assert!(now_ms().saturating_sub(LAST_ACTIVITY.load(Ordering::Relaxed)) < 5_000);
    }

    /// The whole trip over real pipes between two serving ends in this process: the dock's request reaches a "main", its reply reaches
    /// a "dock". Prints how long the trip takes (no index search in it).
    #[test]
    fn a_request_and_its_reply_cross_the_pipes() {
        let main_app = format!("rf-sp-main-{}", std::process::id());
        let dock_app = format!("rf-sp-dock-{}", std::process::id());
        let (to_main, main_rx) = std::sync::mpsc::channel::<String>();
        let (to_dock, dock_rx) = std::sync::mpsc::channel::<String>();
        let to_main = std::sync::Mutex::new(to_main);
        let to_dock = std::sync::Mutex::new(to_dock);
        rf_win::single_instance::serve_app(&main_app, move |m| {
            let _ = to_main.lock().unwrap().send(m);
        });
        rf_win::single_instance::serve_app(&dock_app, move |m| {
            let _ = to_dock.lock().unwrap().send(m);
        });
        let t = std::time::Instant::now();
        let r = req(9, "çalışma notları");
        while !rf_win::single_instance::send_to(&main_app, &encode_request(&r)) {
            assert!(t.elapsed().as_secs() < 5, "the main pipe never came up");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let got = main_rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(decode_request(&got), Some(r));
        let reply = Reply { id: 9, index: Index::Ready, hits: vec![Hit { score: 700, dir: true, path: r"C:\Çalışma".into() }] };
        while !rf_win::single_instance::send_to(&dock_app, &encode_reply(&reply)) {
            assert!(t.elapsed().as_secs() < 5, "the dock pipe never came up");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let got = dock_rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert_eq!(decode_reply(&got), Some(reply));
        // the pipes are up now: a second round trip is the cost a keystroke pays
        let t2 = std::time::Instant::now();
        assert!(rf_win::single_instance::send_to(&main_app, &encode_request(&req(10, "x"))));
        main_rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(rf_win::single_instance::send_to(&dock_app, &encode_reply(&Reply { id: 10, index: Index::Off, hits: vec![] })));
        dock_rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        eprintln!("request + reply over two pipes, warm: {:?} (first trip incl. server start-up: {:?})", t2.elapsed(), t.elapsed());
    }
}
