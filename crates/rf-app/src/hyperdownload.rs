//! rustyfiles' side of HyperDownload, the download manager add-on (`addons.rs` installs it; its own repository is
//! `gelenmedya/hyperdownload`). The add-on is a program of its own: rustyfiles starts it when a download is asked for (`serve`,
//! with its data in rustyfiles' local data, so a portable copy or a test copy keeps it apart), finds its port and secret in its
//! `rpc.json`, and speaks aria2's JSON-RPC to it on 127.0.0.1. A download is a card of the Status Center, like a copy: its bytes, its
//! speed, pause and cancel.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const ID: &str = "hyperdownload";

/// The message a second launch (`rustyfiles.exe --hd-follow=<gid>`, started by the add-on's browser host) sends to the running
/// rustyfiles: show this download as a card of the Status Center.
pub const FOLLOW_PREFIX: &str = "hd-follow:";

/// The browser extension's id while it is loaded unpacked (from the key in its manifest); the stores' ids join it once it is published.
const EXTENSION_IDS: &[&str] = &["pbnficneolckhnkmpagjimjbhjahcepa"];
const HOST_NAME: &str = "com.gelenmedya.hyperdownload";

/// Where the browsers look for native messaging hosts (current user only).
const HOST_KEYS: &[&str] = &[
    r"Software\Google\Chrome\NativeMessagingHosts",
    r"Software\Microsoft\Edge\NativeMessagingHosts",
    r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts",
    r"Software\Chromium\NativeMessagingHosts",
];

/// After the add-on is installed (and at each start of it): the browser host's files beside the program (`host.json`: this
/// rustyfiles' data folder, the Downloads folder and its own exe, so that the host started by a browser finds the same running
/// program and shows its downloads here) and the browsers' registration of the host (current user's registry, its own keys only).
pub fn register_host() {
    let (Some(exe), Some(data)) = (crate::addons::program(ID), data_dir()) else { return };
    let Some(dir) = exe.parent() else { return };
    let downloads = dirs::download_dir().unwrap_or_else(|| data.clone());
    let me = std::env::current_exe().ok();
    let host = json!({ "data": data, "downloads": downloads, "rustyfiles": me });
    let _ = std::fs::write(dir.join("host.json"), serde_json::to_vec_pretty(&host).unwrap_or_default());
    let origins: Vec<String> = EXTENSION_IDS.iter().map(|id| format!("chrome-extension://{id}/")).collect();
    let manifest = json!({ "name": HOST_NAME, "description": "HyperDownload, the download manager of rustyfiles", "path": exe, "type": "stdio", "allowed_origins": origins });
    let manifest_path = dir.join("native-host.json");
    if std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap_or_default()).is_err() {
        return;
    }
    // a copy with settings of its own (a test copy) never points the user's browsers at its add-on
    if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some_and(|d| !d.is_empty()) {
        return;
    }
    for key in HOST_KEYS {
        if let Ok(k) = windows_registry::CURRENT_USER.create(format!("{key}\\{HOST_NAME}")) {
            let _ = k.set_string("", manifest_path.to_string_lossy().as_ref());
        }
    }
}

/// Before the add-on is removed: the browsers forget the host.
pub fn unregister_host() {
    if std::env::var_os("RUSTYFILES_CONFIG_DIR").is_some_and(|d| !d.is_empty()) {
        return;
    }
    for key in HOST_KEYS {
        if let Ok(k) = windows_registry::CURRENT_USER.open(key) {
            let _ = k.remove_tree(HOST_NAME);
        }
    }
}

/// Shows a download HyperDownload already has (handed over by the browser) as a card, into the Downloads folder.
pub fn follow(gid: String, cx: &mut gpui_kit::App) {
    let dir = dirs::download_dir().unwrap_or_default();
    download_source(Source::Follow(gid), dir, cx)
}

/// Where the add-on keeps its lists, its torrent session and `rpc.json`.
fn data_dir() -> Option<PathBuf> {
    crate::settings::local_data_dir().map(|d| d.join("hyperdownload"))
}

/// A running HyperDownload: its address and secret.
#[derive(Clone)]
pub struct Rpc {
    port: u16,
    token: String,
}

/// Why nothing can be downloaded.
#[derive(Debug)]
pub enum Unavailable {
    /// The add-on is not installed (Settings > Eklentiler).
    NotInstalled,
    /// It was started but did not answer.
    NoAnswer(String),
}

impl Rpc {
    /// One JSON-RPC call; the secret goes first, as aria2 wants it.
    pub fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, String> {
        let mut p = vec![json!(format!("token:{}", self.token))];
        p.extend(params);
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": p });
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(10))).http_status_as_error(false).build().into();
        let resp = agent
            .post(&format!("http://127.0.0.1:{}/jsonrpc", self.port))
            .header("Content-Type", "application/json")
            .send(body.to_string())
            .map_err(|e| e.to_string())?;
        let text = resp.into_body().read_to_string().map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
        match v.get("error") {
            Some(e) => Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        }
    }
}

fn read_rpc(dir: &Path) -> Option<Rpc> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join("rpc.json")).ok()?).ok()?;
    Some(Rpc { port: u16::try_from(v.get("port")?.as_u64()?).ok()?, token: v.get("token")?.as_str()?.to_string() })
}

fn answers(r: &Rpc) -> bool {
    r.call("aria2.getVersion", vec![]).is_ok()
}

/// The running add-on, started first when it is not running. Blocking (up to ~15 s the first time): call it off the UI thread.
pub fn connect() -> Result<Rpc, Unavailable> {
    let dir = data_dir().ok_or(Unavailable::NoAnswer("no data folder".into()))?;
    if let Some(r) = read_rpc(&dir).filter(answers) {
        return Ok(r);
    }
    let exe = crate::addons::program(ID).ok_or(Unavailable::NotInstalled)?;
    register_host();
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::remove_file(dir.join("rpc.json"));
    let downloads = dirs::download_dir().unwrap_or_else(|| dir.clone());
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("serve").arg("--data").arg(&dir).arg("--downloads").arg(&downloads);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window; not tied to this process (it goes on with the downloads when rustyfiles closes)
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS);
    }
    cmd.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    // (the variables of a test copy are not handed on: the add-on has its own folders by its arguments)
    cmd.env_remove("RUSTYFILES_CONFIG_DIR");
    cmd.spawn().map_err(|e| Unavailable::NoAnswer(format!("{}: {e}", exe.display())))?;
    let until = Instant::now() + Duration::from_secs(15);
    while Instant::now() < until {
        std::thread::sleep(Duration::from_millis(150));
        if let Some(r) = read_rpc(&dir).filter(answers) {
            return Ok(r);
        }
    }
    Err(Unavailable::NoAnswer("HyperDownload did not start".into()))
}

/// What a download's status says, read from aria2's `tellStatus`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Seen {
    pub status: String,
    pub total: u64,
    pub done: u64,
    pub speed: u64,
    pub error: String,
    /// The file (an HTTP download's), once known.
    pub path: Option<PathBuf>,
}

pub fn seen(v: &Value) -> Seen {
    let num = |k: &str| v.get(k).and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
    let path = v.get("files").and_then(Value::as_array).and_then(|f| f.first()).and_then(|f| f.get("path")).and_then(Value::as_str).filter(|p| !p.is_empty()).map(PathBuf::from);
    Seen {
        status: v.get("status").and_then(Value::as_str).unwrap_or_default().to_string(),
        total: num("totalLength"),
        done: num("completedLength"),
        speed: num("downloadSpeed"),
        error: v.get("errorMessage").and_then(Value::as_str).unwrap_or_default().to_string(),
        path,
    }
}

/// The name a link's download is shown under before the server has said (the URL's last part, or the magnet's name).
pub fn label_of(url: &str) -> String {
    if url.starts_with("magnet:") {
        if let Some(dn) = url.split(['?', '&']).find_map(|kv| kv.strip_prefix("dn=")) {
            return dn.replace('+', " ");
        }
        return "magnet".into();
    }
    let no_query = url.split(['?', '#']).next().unwrap_or(url);
    no_query.rsplit('/').find(|s| !s.is_empty()).unwrap_or(url).to_string()
}

/// Whether a text is a link HyperDownload takes (http, https, magnet).
pub fn is_link(text: &str) -> bool {
    let t = text.trim().to_ascii_lowercase();
    (t.starts_with("http://") || t.starts_with("https://") || t.starts_with("magnet:?")) && !t.contains(char::is_whitespace)
}

/// Downloads `url` into `dir` as a card of the Status Center. When the add-on is missing, says so and offers its page.
/// A torrent's file list, sent from the download's thread to the window to be chosen from: its name, the files (path, bytes), the
/// ones suggested, and where the answer goes (`None`: cancelled).
type Ask = (String, Vec<(String, u64)>, Vec<usize>, std::sync::mpsc::Sender<Option<Vec<usize>>>);

/// Shows the file list with check boxes in the window in front (the suggested files ticked); "İndir" sends the ticked ones.
fn ask_files(name: String, files: Vec<(String, u64)>, suggested: Vec<usize>, reply: std::sync::mpsc::Sender<Option<Vec<usize>>>, cx: &mut gpui_kit::App) {
    use crate::prompt::{Button, Check, Kind};
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    let Some(win) = cx.active_window().or_else(|| cx.windows().first().copied()) else {
        let _ = reply.send(Some(suggested));
        return;
    };
    let cells: Vec<Rc<Cell<bool>>> = (0..files.len()).map(|i| Rc::new(Cell::new(suggested.contains(&i)))).collect();
    let checks: Vec<Check> = files
        .iter()
        .zip(&cells)
        .map(|((path, len), on)| Check { label: path.clone().into(), note: rf_core::format::size(*len).into(), on: on.clone() })
        .collect();
    let reply = Rc::new(RefCell::new(Some(reply)));
    let (r1, r2) = (reply.clone(), reply.clone());
    let picked = cells.clone();
    let _ = win.update(cx, move |_, window, cx| {
        crate::prompt::show_checks(
            name,
            Some(crate::t!("İndirilecek dosyaları seçin").to_string()),
            checks,
            vec![
                Button::new(crate::t!("İndir"), Kind::Primary, move |_, _, _| {
                    let chosen: Vec<usize> = picked.iter().enumerate().filter(|(_, c)| c.get()).map(|(i, _)| i).collect();
                    if chosen.is_empty() {
                        return Err(crate::t!("En az bir dosya seçin").to_string());
                    }
                    if let Some(tx) = r1.borrow_mut().take() {
                        let _ = tx.send(Some(chosen));
                    }
                    Ok(())
                }),
                Button::new(crate::t!("İptal"), Kind::Normal, move |_, _, _| {
                    if let Some(tx) = r2.borrow_mut().take() {
                        let _ = tx.send(None);
                    }
                    Ok(())
                }),
            ],
            window,
            cx,
        );
    });
}

/// What is downloaded: a link (http, https, magnet) or a `.torrent` file.
#[derive(Clone, Debug)]
pub enum Source {
    Link(String),
    TorrentFile(PathBuf),
    /// A download HyperDownload already has (one the browser extension handed it), by its GID.
    Follow(String),
}

/// Standard base64 (with padding), as aria2's `addTorrent` takes the torrent file.
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16) | (u32::from(*c.get(1).unwrap_or(&0)) << 8) | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

/// A link pasted into a folder (Ctrl+V with a link and no files on the clipboard): asks whether to download it there.
pub fn offer_pasted_link(url: String, dir: PathBuf, cx: &mut gpui_kit::App) {
    use crate::prompt::{Button, Kind};
    let Some(win) = cx.active_window() else { return };
    let _ = win.update(cx, move |_, window, cx| {
        crate::prompt::show(
            crate::t!("Bağlantı bu klasöre indirilsin mi?"),
            Some(url.clone()),
            None,
            vec![
                Button::new(crate::t!("İndir"), Kind::Primary, move |_, _, cx| {
                    download(url.clone(), dir.clone(), cx);
                    Ok(())
                }),
                Button::cancel(),
            ],
            window,
            cx,
        );
    });
}

/// Downloads `url` into `dir` as a card of the Status Center. When the add-on is missing, says so and offers its page.
pub fn download(url: String, dir: PathBuf, cx: &mut gpui_kit::App) {
    download_source(Source::Link(url), dir, cx)
}

/// A `.torrent` file's download, into `dir`.
pub fn download_torrent(file: PathBuf, dir: PathBuf, cx: &mut gpui_kit::App) {
    download_source(Source::TorrentFile(file), dir, cx)
}

pub fn download_source(source: Source, dir: PathBuf, cx: &mut gpui_kit::App) {
    let label = match &source {
        Source::Link(u) => label_of(u),
        Source::TorrentFile(p) => p.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        Source::Follow(_) => crate::t!("tarayıcıdan").to_string(),
    };
    let url = match &source {
        Source::Link(u) => u.clone(),
        Source::TorrentFile(p) => p.display().to_string(),
        Source::Follow(g) => g.clone(),
    };
    let title = format!("{} {}", crate::t!("İndiriliyor:"), label);
    let shown_dir = dir.clone();
    // a torrent's file list is chosen in a window: the download's thread asks the window through this
    let (ask_tx, mut ask_rx) = futures::channel::mpsc::unbounded::<Ask>();
    // a video of a torrent can be watched while it downloads: the card gets an "İzle" button (its id is known once it is made)
    let card = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let (watch_tx, mut watch_rx) = futures::channel::mpsc::unbounded::<PathBuf>();
    {
        let card = card.clone();
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(path) = watch_rx.next().await {
                let id = card.load(std::sync::atomic::Ordering::Relaxed);
                let _ = cx.update(|cx| {
                    let p = path.clone();
                    let run: std::rc::Rc<dyn Fn(&mut gpui_kit::App)> = std::rc::Rc::new(move |_cx| {
                        crate::look::ipc::send(crate::look::ipc::Msg::Show(p.clone(), crate::look::ShowOpts::default()), true);
                    });
                    crate::status_center::set_live_action(cx, id, (crate::t!("İzle").to_string(), run));
                });
            }
        })
        .detach();
    }
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while let Some((name, files, suggested, reply)) = ask_rx.next().await {
            let _ = cx.update(|cx| ask_files(name, files, suggested, reply, cx));
        }
    })
    .detach();
    let id = crate::status_center::start_with_id(
        cx,
        title,
        move |t: Arc<rf_win::fileops::Tracker>| {
            let rpc = match connect() {
                Ok(r) => r,
                Err(Unavailable::NotInstalled) => anyhow::bail!("{}", crate::t!("HyperDownload kurulu değil: Ayarlar › Eklentiler'den kurabilirsiniz")),
                Err(Unavailable::NoAnswer(e)) => anyhow::bail!("{e}"),
            };
            let options = json!({ "dir": dir.display().to_string() });
            let gid = match &source {
                Source::Link(u) => rpc.call("aria2.addUri", vec![json!([u]), options]),
                Source::TorrentFile(p) => {
                    let bytes = std::fs::read(p).map_err(|e| anyhow::anyhow!("{}: {e}", p.display()))?;
                    rpc.call("aria2.addTorrent", vec![json!(base64(&bytes)), json!([]), options])
                }
                Source::Follow(g) => Ok(json!(g)),
            }
            .map_err(anyhow::Error::msg)?;
            let gid = gid.as_str().unwrap_or_default().to_string();
            let mut paused = false;
            let mut asked = false;
            let mut offered = false;
            loop {
                std::thread::sleep(Duration::from_millis(300));
                if t.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                    let _ = rpc.call("aria2.remove", vec![json!(gid)]);
                    let _ = rpc.call("aria2.removeDownloadResult", vec![json!(gid)]);
                    return Ok(false);
                }
                let want_pause = t.paused.load(std::sync::atomic::Ordering::Relaxed);
                if want_pause != paused {
                    let _ = rpc.call(if want_pause { "aria2.pause" } else { "aria2.unpause" }, vec![json!(gid)]);
                    paused = want_pause;
                }
                let Ok(v) = rpc.call("aria2.tellStatus", vec![json!(gid)]) else { continue };
                let s = seen(&v);
                t.bytes.set(s.done, s.total.max(s.done));
                let name = s
                    .path
                    .as_ref()
                    .filter(|_| v.get("hdKind").and_then(Value::as_str) != Some("torrent"))
                    .and_then(|p| p.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
                    .or_else(|| v.pointer("/bittorrent/info/name").and_then(Value::as_str).map(str::to_string));
                (t.on_progress)(1, 0, name);
                // a torrent whose file list waits for the user's choice
                if s.status == "waiting" && v.get("hdChoice").and_then(Value::as_bool) == Some(true) && !asked {
                    asked = true;
                    let choices = rpc.call("hd.choices", vec![]).unwrap_or(Value::Null);
                    let mine = choices.as_array().and_then(|a| a.iter().find(|c| c.get("gid").and_then(Value::as_str) == Some(gid.as_str()))).cloned();
                    if let Some(c) = mine {
                        let n = |v: &Value| v.as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
                        let files: Vec<(String, u64)> = c["files"].as_array().map(|a| a.iter().map(|f| (f["path"].as_str().unwrap_or_default().to_string(), n(&f["length"]))).collect()).unwrap_or_default();
                        let suggested: Vec<usize> = c["suggested"].as_array().map(|a| a.iter().map(|i| n(i) as usize).filter(|i| *i > 0).map(|i| i - 1).collect()).unwrap_or_default();
                        let (tx, rx) = std::sync::mpsc::channel();
                        let name = c["name"].as_str().unwrap_or_default().to_string();
                        let _ = ask_tx.unbounded_send((name, files, suggested, tx));
                        let answer = loop {
                            if t.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                                break None;
                            }
                            match rx.recv_timeout(Duration::from_millis(300)) {
                                Ok(a) => break a,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                                Err(_) => break None,
                            }
                        };
                        match answer {
                            Some(files) => {
                                let list: Vec<Value> = files.iter().map(|i| json!((i + 1).to_string())).collect();
                                rpc.call("hd.choose", vec![json!(gid), json!(list)]).map_err(anyhow::Error::msg)?;
                            }
                            None => {
                                let _ = rpc.call("aria2.remove", vec![json!(gid)]);
                                return Ok(false);
                            }
                        }
                    }
                    continue;
                }
                // a torrent's video: offered for watching while it downloads (from the add-on's local stream)
                if !offered && s.status == "active" && v.get("hdKind").and_then(Value::as_str) == Some("torrent") {
                    let main = v.get("hdMainFile").and_then(Value::as_str).and_then(|m| m.parse::<usize>().ok());
                    let file = main.and_then(|m| v["files"].as_array().and_then(|f| f.get(m - 1)).and_then(|f| f["path"].as_str()).map(str::to_string));
                    if let (Some(m), Some(file)) = (main, file)
                        && rf_player::can_open(Path::new(&file))
                        && let Ok(url) = rpc.call("hd.streamUrl", vec![json!(gid), json!(m.to_string())])
                        && let Some(url) = url.as_str()
                    {
                        let name = Path::new(&file).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                        let _ = watch_tx.unbounded_send(crate::look::stream::path(url, &name));
                        offered = true;
                    }
                }
                match s.status.as_str() {
                    "complete" => {
                        let _ = rpc.call("aria2.removeDownloadResult", vec![json!(gid)]);
                        if let Some(p) = s.path {
                            t.results.lock().unwrap().push((PathBuf::from(&url), p));
                        }
                        (t.on_progress)(1, 1, None);
                        return Ok(true);
                    }
                    "error" => anyhow::bail!("{}", s.error),
                    "removed" => return Ok(false),
                    _ => {}
                }
            }
        },
        Some(Box::new(move |_ok, cx| {
            // (the folder the file came into shows it at once: it is watched, as after a copy)
            let _ = (&shown_dir, cx);
        })),
    );
    card.store(id, std::sync::atomic::Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_status_is_read_from_arias_strings() {
        let v = json!({ "status": "active", "totalLength": "1000", "completedLength": "250", "downloadSpeed": "50",
                        "files": [{ "path": "C:\\d\\a.zip" }], "errorMessage": "" });
        let s = seen(&v);
        assert_eq!((s.status.as_str(), s.total, s.done, s.speed), ("active", 1000, 250, 50));
        assert_eq!(s.path, Some(PathBuf::from("C:\\d\\a.zip")));
        assert_eq!(seen(&json!({ "files": [{ "path": "" }] })).path, None);
    }

    #[test]
    fn base64_is_the_standard_one() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64(&[0xff, 0xfe, 0xfd]), "//79");
    }

    #[test]
    fn links_and_their_names() {
        assert!(is_link("https://example.com/a.zip"));
        assert!(is_link(" magnet:?xt=urn:btih:abc&dn=Big+Buck+Bunny "));
        assert!(!is_link("C:\\folder"));
        assert!(!is_link("https://a b"));
        assert_eq!(label_of("https://example.com/files/a.zip?x=1"), "a.zip");
        assert_eq!(label_of("magnet:?xt=urn:btih:abc&dn=Big+Buck+Bunny"), "Big Buck Bunny");
    }
}

/// Manual, end to end: the add-on installed from a zip (`HD_TEST_ZIP`, a zip with hyperdownload.exe), started by `connect`, a file
/// downloaded from a server of the test's own through the JSON-RPC, then the add-on asked to end.
/// `HD_TEST_ZIP=<zip> cargo test -p rf-app --lib hyperdownload::e2e -- --ignored --nocapture`
#[cfg(test)]
mod e2e {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    #[ignore]
    fn a_link_is_downloaded_through_the_installed_add_on() {
        let zip = PathBuf::from(std::env::var("HD_TEST_ZIP").expect("HD_TEST_ZIP"));
        let tmp = std::env::temp_dir().join(format!("rf-hd-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("out")).unwrap();
        unsafe { std::env::set_var("RUSTYFILES_CONFIG_DIR", tmp.join("cfg")) };
        // the add-on, installed as the Add-ons page does it
        use sha2::Digest;
        let sha: String = sha2::Sha256::digest(std::fs::read(&zip).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
        let mut info = crate::addons::AddonInfo { id: ID.into(), name: "HyperDownload".into(), version: "0.1.0".into(), license: "MIT".into(), source: String::new(), files: Default::default() };
        info.files.insert(crate::addons::arch().into(), crate::addons::FileRef { url: zip.display().to_string(), sha256: sha, size: 0 });
        crate::addons::install(&info, &|_, _| {}, &|| false).unwrap();
        // a server: 3 MB with ranges
        let body: Vec<u8> = (0..3_000_000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = std::sync::Arc::new(body.clone());
        std::thread::spawn(move || {
            for c in listener.incoming().flatten() {
                let b = served.clone();
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().unwrap());
                    let (mut range, mut line) = (None, String::new());
                    loop {
                        line.clear();
                        if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                            let (a, z) = v.trim().split_once('-').unwrap();
                            range = Some((a.parse::<usize>().unwrap(), z.parse::<usize>().ok()));
                        }
                    }
                    let mut c = c;
                    let n = b.len();
                    let (a, z) = range.map_or((0, n - 1), |(a, z)| (a, z.unwrap_or(n - 1).min(n - 1)));
                    let head = if range.is_some() {
                        format!("HTTP/1.1 206 P\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{z}/{n}\r\nConnection: close\r\n\r\n", z + 1 - a)
                    } else {
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {n}\r\nConnection: close\r\n\r\n")
                    };
                    let _ = c.write_all(head.as_bytes());
                    let _ = c.write_all(&b[a..=z]);
                });
            }
        });
        let started = Instant::now();
        let rpc = connect().unwrap_or_else(|e| panic!("{e:?}"));
        eprintln!("the add-on answered after {:?}", started.elapsed());
        let gid = rpc.call("aria2.addUri", vec![json!([format!("http://127.0.0.1:{port}/e2e.bin")]), json!({ "dir": tmp.join("out").display().to_string() })]).unwrap();
        let mut last = Seen::default();
        for _ in 0..200 {
            std::thread::sleep(Duration::from_millis(100));
            last = seen(&rpc.call("aria2.tellStatus", vec![gid.clone()]).unwrap());
            if last.status == "complete" || last.status == "error" {
                break;
            }
        }
        assert_eq!(last.status, "complete", "{last:?}");
        assert_eq!(std::fs::read(tmp.join("out").join("e2e.bin")).unwrap(), body);
        // a second connect finds the running one
        assert!(connect().is_ok());
        let _ = rpc.call("aria2.shutdown", vec![]);
        std::thread::sleep(Duration::from_millis(800));
        unsafe { std::env::remove_var("RUSTYFILES_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
