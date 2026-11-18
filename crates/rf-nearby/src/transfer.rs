//! Sending, receiving and pairing over one QUIC connection each (see `proto`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::proto::{self, Answer, Done, FileHeader, Hello, Item, Outcome, PairConfirm, Purpose};
use crate::{Inner, Offer, TransferResult, Visibility};

/// Files sent at once, each on its own stream.
const PARALLEL: usize = 4;
const CHUNK: usize = 1 << 20;
/// How long the other side may take to say yes to an offer or a pairing code.
const DECISION: Duration = Duration::from_secs(180);
const CONNECT: Duration = Duration::from_secs(6);
/// The suffix of a file being received: renamed away only once its BLAKE3 matches.
pub const PART: &str = ".rfpart";

// ------------------------------------------------------------------------------------------------------------------------- sending

/// The offer's items for `paths`: files as they are, folders walked (an empty folder is an item of its own). Blocking.
pub fn collect(paths: &[PathBuf]) -> anyhow::Result<(Vec<Item>, Vec<Option<PathBuf>>)> {
    let mut items = Vec::new();
    let mut sources = Vec::new();
    for p in paths {
        let name = p.file_name().ok_or_else(|| anyhow!("{} has no name", p.display()))?.to_string_lossy().to_string();
        walk(p, &name, &mut items, &mut sources)?;
    }
    Ok((items, sources))
}

fn walk(path: &Path, rel: &str, items: &mut Vec<Item>, sources: &mut Vec<Option<PathBuf>>) -> anyhow::Result<()> {
    use unicode_normalization::UnicodeNormalization as _;
    // (names go on the wire in NFC: macOS hands out NFD, the others expect NFC)
    let rel: String = rel.nfc().collect();
    // (a link is sent as a link, never followed: a bundle's links must arrive as links, and following one could send half the disk)
    let meta = std::fs::symlink_metadata(path).with_context(|| format!("cannot read {}", path.display()))?;
    let mtime = meta.modified().ok().and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis() as i64);
    if meta.file_type().is_symlink() {
        let target = std::fs::read_link(path)?.to_string_lossy().replace('\\', "/");
        items.push(Item { path: rel, link: Some(target), mtime, ..Default::default() });
        sources.push(None);
    } else if meta.is_dir() {
        let mut children: Vec<_> = std::fs::read_dir(path)?.flatten().collect();
        children.sort_by_key(|e| e.file_name());
        if children.is_empty() {
            items.push(Item { path: rel.clone(), dir: true, mtime, ..Default::default() });
            sources.push(None);
        }
        for c in children {
            let child_rel = format!("{rel}/{}", c.file_name().to_string_lossy());
            walk(&c.path(), &child_rel, items, sources)?;
        }
    } else {
        items.push(Item { path: rel, size: meta.len(), mode: mode_of(&meta), mtime, ..Default::default() });
        sources.push(Some(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(unix)]
fn mode_of(meta: &std::fs::Metadata) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(meta.permissions().mode() & 0o7777)
}

#[cfg(not(unix))]
fn mode_of(_: &std::fs::Metadata) -> Option<u32> {
    None
}

pub async fn connect(inner: &Inner, addrs: &[SocketAddr]) -> anyhow::Result<quinn::Connection> {
    let mut last = anyhow!("the device has no address");
    for addr in addrs {
        let attempt = async {
            let connecting = inner.endpoint.connect_with(inner.client.clone(), *addr, "rustyfiles")?;
            anyhow::Ok(connecting.await?)
        };
        match tokio::time::timeout(CONNECT, attempt).await {
            Ok(Ok(c)) => return Ok(c),
            Ok(Err(e)) => last = e,
            Err(_) => last = anyhow!("{addr} did not answer"),
        }
    }
    Err(last)
}

pub async fn send(inner: Arc<Inner>, tid: u64, addrs: Vec<SocketAddr>, peer_id: String, paths: Vec<PathBuf>) -> TransferResult {
    let r = send_inner(&inner, tid, &addrs, &peer_id, paths).await;
    inner.forget_transfer(tid);
    result_of(&inner, tid, r.map(|_| Vec::new()))
}

async fn send_inner(inner: &Arc<Inner>, tid: u64, addrs: &[SocketAddr], peer_id: &str, paths: Vec<PathBuf>) -> anyhow::Result<()> {
    let (items, sources) = tokio::task::spawn_blocking(move || collect(&paths)).await??;
    let conn = connect(inner, addrs).await?;
    let fp = crate::tls::peer_fingerprint(&conn).ok_or_else(|| anyhow!("the device showed no certificate"))?;
    // (the device must be the one that was chosen: its certificate proves its id)
    if !peer_id.is_empty() && crate::identity::id_of(&fp) != peer_id {
        conn.close(0u32.into(), b"wrong device");
        bail!("another device answered at that address");
    }
    inner.track(tid, conn.clone());
    let (mut ctl_send, mut ctl_recv) = conn.open_bi().await?;
    let hello = Hello { proto: proto::PROTOCOL, id: inner.me.id.clone(), name: inner.cfg.name.clone(), os: inner.cfg.os.clone(), purpose: Purpose::Send { items: items.clone() } };
    proto::write_frame(&mut ctl_send, &hello).await?;
    let answer: Answer = tokio::time::timeout(DECISION, proto::read_frame(&mut ctl_recv)).await.map_err(|_| anyhow!("no answer from the device"))??;
    if !answer.accept {
        bail!("{}", answer.reason.unwrap_or_else(|| "the device declined".into()));
    }
    let total: u64 = items.iter().map(|i| i.size).sum();
    let done = Arc::new(AtomicU64::new(answer.resume.iter().sum::<u64>().min(total)));
    let ticker = progress_ticker(inner.clone(), tid, done.clone(), total);

    let sem = Arc::new(tokio::sync::Semaphore::new(PARALLEL));
    let mut tasks = Vec::new();
    for (index, (item, source)) in items.iter().zip(&sources).enumerate() {
        let Some(source) = source.clone() else { continue };
        let offset = answer.resume.get(index).copied().unwrap_or(0).min(item.size);
        let (conn, sem, done) = (conn.clone(), sem.clone(), done.clone());
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire_owned().await?;
            send_file(&conn, index as u32, &source, offset, &done).await
        }));
    }
    let mut hashes = vec![String::new(); items.len()];
    let mut file_i = sources.iter().enumerate().filter(|(_, s)| s.is_some()).map(|(i, _)| i);
    for t in tasks {
        let h = t.await??;
        if let Some(i) = file_i.next() {
            hashes[i] = h;
        }
    }
    ticker.store(true, Ordering::Relaxed);
    proto::write_frame(&mut ctl_send, &Done { hashes }).await?;
    // (the other side checks every file before it answers: give it time for a big transfer)
    let outcome: Outcome = tokio::time::timeout(Duration::from_secs(600), proto::read_frame(&mut ctl_recv)).await.map_err(|_| anyhow!("the device did not confirm"))??;
    let _ = ctl_send.finish();
    conn.close(0u32.into(), b"done");
    inner.listener.progress(tid, total, total);
    if outcome.ok { Ok(()) } else { Err(anyhow!("{}", outcome.error.unwrap_or_else(|| "the device could not keep the files".into()))) }
}

/// Sends one file from `offset` on a stream of its own; returns its BLAKE3 (of the whole file).
async fn send_file(conn: &quinn::Connection, index: u32, path: &Path, offset: u64, done: &AtomicU64) -> anyhow::Result<String> {
    let mut stream = conn.open_uni().await?;
    proto::write_frame(&mut stream, &FileHeader { index, offset }).await?;
    let mut file = tokio::fs::File::open(path).await.with_context(|| format!("cannot open {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    if offset > 0 {
        // (the part the other side already has still counts in the hash)
        let mut head = tokio::fs::File::open(path).await?.take(offset);
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = head.read(&mut buf).await?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        file.seek(std::io::SeekFrom::Start(offset)).await?;
    }
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        stream.write_all(&buf[..n]).await?;
        done.fetch_add(n as u64, Ordering::Relaxed);
    }
    stream.finish()?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// Reports `done / total` to the listener at most ten times a second while the transfer runs; stops once the flag it returns is set.
fn progress_ticker(inner: Arc<Inner>, tid: u64, done: Arc<AtomicU64>, total: u64) -> Arc<AtomicBool> {
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    tokio::spawn(async move {
        let mut last = u64::MAX;
        while !s.load(Ordering::Relaxed) {
            let d = done.load(Ordering::Relaxed);
            if d != last {
                inner.listener.progress(tid, d, total);
                last = d;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });
    stop
}

fn result_of(inner: &Inner, tid: u64, r: anyhow::Result<Vec<String>>) -> TransferResult {
    let cancelled = inner.was_cancelled(tid);
    match r {
        Ok(paths) => TransferResult { ok: true, cancelled: false, error: None, paths },
        Err(e) => {
            log::warn!("nearby: transfer {tid}: {e:#}");
            TransferResult { ok: false, cancelled, error: (!cancelled).then(|| format!("{e:#}")), paths: Vec::new() }
        }
    }
}

// ------------------------------------------------------------------------------------------------------------------------- incoming

/// One incoming connection: its hello says whether it is an offer or a pairing.
pub async fn incoming(inner: Arc<Inner>, incoming: quinn::Incoming) -> anyhow::Result<()> {
    let conn = incoming.await?;
    let fp = crate::tls::peer_fingerprint(&conn).ok_or_else(|| anyhow!("no certificate"))?;
    let (mut ctl_send, mut ctl_recv) = tokio::time::timeout(Duration::from_secs(10), conn.accept_bi()).await??;
    let hello: Hello = proto::read_frame(&mut ctl_recv).await?;
    if hello.proto != proto::PROTOCOL || hello.id != crate::identity::id_of(&fp) {
        conn.close(1u32.into(), b"bad hello");
        bail!("a peer sent a hello that does not match its certificate");
    }
    match hello.purpose.clone() {
        Purpose::Send { items } => {
            let tid = inner.next_id();
            inner.track(tid, conn.clone());
            let r = receive(&inner, tid, &conn, &mut ctl_send, &mut ctl_recv, &hello, &fp, items).await;
            inner.forget_transfer(tid);
            let declined = matches!(&r, Err(e) if e.downcast_ref::<Declined>().is_some());
            if !declined {
                inner.listener.finished(tid, result_of(&inner, tid, r));
            }
            Ok(())
        }
        Purpose::Pair => pair(&inner, &conn, &mut ctl_send, &mut ctl_recv, &hello.id, &hello.name, &fp).await,
    }
}

/// An offer this side said no to (or the user did, or nobody answered): nothing to report as a failed transfer.
#[derive(Debug)]
struct Declined;
impl std::fmt::Display for Declined {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("declined")
    }
}
impl std::error::Error for Declined {}

#[allow(clippy::too_many_arguments)]
async fn receive(
    inner: &Arc<Inner>,
    tid: u64,
    conn: &quinn::Connection,
    ctl_send: &mut quinn::SendStream,
    ctl_recv: &mut quinn::RecvStream,
    hello: &Hello,
    fp: &str,
    items: Vec<Item>,
) -> anyhow::Result<Vec<String>> {
    let decline = |reason: &str| Answer { accept: false, reason: Some(reason.to_string()), resume: Vec::new() };
    if items.iter().any(|i| proto::safe_parts(&i.path).is_none()) {
        proto::write_frame(ctl_send, &decline("a file name is not allowed")).await?;
        return Err(Declined.into());
    }
    let trusted = inner.trusts(fp);
    if inner.visibility() == Visibility::Trusted && !trusted {
        proto::write_frame(ctl_send, &decline("the device only takes files from its trusted devices")).await?;
        return Err(Declined.into());
    }
    // the destination: a trusted device's files go straight to the inbox when the app says so; everything else is asked
    let dest = if trusted && inner.cfg.auto_accept_trusted {
        Some(PathBuf::from(&inner.cfg.inbox))
    } else {
        let mut names: Vec<String> = Vec::new();
        for i in &items {
            let top = i.path.split('/').next().unwrap_or_default().to_string();
            if !names.contains(&top) {
                names.push(top);
            }
        }
        let offer = Offer {
            id: inner.next_id(),
            transfer: tid,
            from_id: hello.id.clone(),
            from_name: hello.name.clone(),
            from_os: hello.os.clone(),
            trusted,
            names,
            file_count: items.iter().filter(|i| i.is_file()).count() as u64,
            total_bytes: items.iter().map(|i| i.size).sum(),
        };
        let rx = inner.ask(offer);
        tokio::time::timeout(DECISION, rx).await.ok().and_then(|r| r.ok()).flatten()
    };
    let Some(dest) = dest else {
        proto::write_frame(ctl_send, &decline("declined")).await?;
        return Err(Declined.into());
    };
    tokio::fs::create_dir_all(&dest).await.with_context(|| format!("cannot make {}", dest.display()))?;
    let targets = targets(&dest, &items);
    let resume: Vec<u64> = items
        .iter()
        .zip(&targets)
        .map(|(i, t)| if !i.is_file() { 0 } else { std::fs::metadata(part(t)).map(|m| m.len()).ok().filter(|&l| l <= i.size).unwrap_or(0) })
        .collect();
    proto::write_frame(ctl_send, &Answer { accept: true, reason: None, resume: resume.clone() }).await?;

    let total: u64 = items.iter().map(|i| i.size).sum();
    let done = Arc::new(AtomicU64::new(resume.iter().sum()));
    let ticker = progress_ticker(inner.clone(), tid, done.clone(), total);
    let files = items.iter().filter(|i| i.is_file()).count();
    let targets = Arc::new(targets);
    let mut tasks = Vec::new();
    for _ in 0..files {
        let mut stream = conn.accept_uni().await?;
        let (targets, done) = (targets.clone(), done.clone());
        let sizes: Vec<u64> = items.iter().map(|i| i.size).collect();
        tasks.push(tokio::spawn(async move {
            let header: FileHeader = proto::read_frame(&mut stream).await?;
            let index = header.index as usize;
            let target = targets.get(index).ok_or_else(|| anyhow!("no item {index}"))?;
            anyhow::ensure!(header.offset <= sizes[index], "offset past the end");
            if let Some(dir) = target.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            let mut file = tokio::fs::OpenOptions::new().create(true).write(true).truncate(false).open(part(target)).await?;
            file.set_len(header.offset).await?;
            file.seek(std::io::SeekFrom::Start(header.offset)).await?;
            let mut written = header.offset;
            while let Some(chunk) = stream.read_chunk(CHUNK, true).await? {
                written += chunk.bytes.len() as u64;
                anyhow::ensure!(written <= sizes[index], "more bytes than offered");
                file.write_all(&chunk.bytes).await?;
                done.fetch_add(chunk.bytes.len() as u64, Ordering::Relaxed);
            }
            file.flush().await?;
            anyhow::Ok(())
        }));
    }
    for t in tasks {
        t.await??;
    }
    ticker.store(true, Ordering::Relaxed);
    let Done { hashes } = proto::read_frame(ctl_recv).await?;
    // every file is checked before it takes its real name; a mismatch leaves the .rfpart for the next attempt to resume or replace
    let mut failure = None;
    for (index, item) in items.iter().enumerate() {
        let target = &targets[index];
        if item.dir {
            tokio::fs::create_dir_all(target).await?;
            continue;
        }
        if !item.is_file() {
            continue;
        }
        let p = part(target);
        let want = hashes.get(index).cloned().unwrap_or_default();
        let got = tokio::task::spawn_blocking(move || hash_file(&p)).await??;
        if got != want {
            failure = Some(format!("{} arrived damaged", item.path));
            continue;
        }
        tokio::fs::rename(part(target), target).await?;
        keep_attributes(target, item);
    }
    // links last: none exists while files are written, so no file can be written through one
    if failure.is_none() {
        for (index, item) in items.iter().enumerate() {
            if let Some(to) = &item.link {
                make_link(&targets[index], to, &item.path);
            }
        }
    }
    let outcome = Outcome { ok: failure.is_none(), error: failure.clone() };
    proto::write_frame(ctl_send, &outcome).await?;
    let _ = ctl_send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    inner.listener.progress(tid, total, total);
    if let Some(f) = failure {
        bail!(f);
    }
    // the top-level things that arrived, each once
    let mut tops: Vec<String> = Vec::new();
    for (item, target) in items.iter().zip(targets.iter()) {
        let depth = item.path.split('/').count();
        let mut top = target.clone();
        for _ in 1..depth {
            top.pop();
        }
        let s = top.to_string_lossy().to_string();
        if !tops.contains(&s) {
            tops.push(s);
        }
    }
    Ok(tops)
}

/// Where each item goes under `dest`, each name as this platform can store it ([`proto::local_name`]). A top-level name that is
/// already taken there (by a finished file or folder, not by an unfinished `.rfpart`) gets " (2)", " (3)"... before its extension, and
/// so does an item whose path differs from an earlier one only in letter case (they would be one file on NTFS and APFS).
pub fn targets(dest: &Path, items: &[Item]) -> Vec<PathBuf> {
    let mut renamed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut taken: std::collections::HashSet<String> = std::collections::HashSet::new();
    items
        .iter()
        .map(|i| {
            let parts: Vec<String> = proto::safe_parts(&i.path).unwrap_or_default().into_iter().map(proto::local_name).collect();
            let top = parts.first().cloned().unwrap_or_default();
            let top_new = renamed.entry(top.to_lowercase()).or_insert_with(|| free_name(dest, &top)).clone();
            let mut p = dest.join(top_new);
            for rest in parts.iter().skip(1) {
                p.push(rest);
            }
            if !taken.insert(p.to_string_lossy().to_lowercase()) {
                // (an item whose path only differs in case from an earlier one: number its own name)
                let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                let parent = p.parent().map(Path::to_path_buf).unwrap_or_default();
                for n in 2.. {
                    let candidate = parent.join(numbered(&name, n));
                    if (parts.len() > 1 || !candidate.exists()) && taken.insert(candidate.to_string_lossy().to_lowercase()) {
                        p = candidate;
                        break;
                    }
                }
            }
            p
        })
        .collect()
}

fn numbered(name: &str, n: u32) -> String {
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    format!("{stem} ({n}){ext}")
}

fn free_name(dest: &Path, name: &str) -> String {
    if !dest.join(name).exists() {
        return name.to_string();
    }
    (2..).map(|n| numbered(name, n)).find(|n| !dest.join(n).exists()).unwrap_or_else(|| name.to_string())
}

/// A received file keeps the sender's modification time and, on Unix, its permission bits. Failures are only logged.
fn keep_attributes(target: &Path, item: &Item) {
    if let Some(ms) = item.mtime
        && let Ok(f) = std::fs::OpenOptions::new().write(true).open(target)
    {
        let when = std::time::UNIX_EPOCH + Duration::from_millis(ms.max(0) as u64);
        if let Err(e) = f.set_modified(when) {
            log::info!("nearby: cannot set the time of {}: {e}", target.display());
        }
    }
    #[cfg(unix)]
    if let Some(mode) = item.mode {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode & 0o7777));
    }
}

/// Makes a received symbolic link, when its target stays inside the item it came with (a bundle's own links do); one that points
/// elsewhere, or absolutely, is left out. On Windows links are left out (making one needs a privilege most users don't have).
fn make_link(target: &Path, to: &str, rel: &str) {
    if !link_stays_inside(rel, to) {
        log::warn!("nearby: the link {rel} -> {to} points outside what was sent; left out");
        return;
    }
    #[cfg(unix)]
    {
        if let Some(dir) = target.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::os::unix::fs::symlink(to, target) {
            log::warn!("nearby: cannot make the link {}: {e}", target.display());
        }
    }
    #[cfg(not(unix))]
    log::info!("nearby: the link {rel} -> {to} was left out (links are not made on this platform); target {}", target.display());
}

/// Whether a link at `rel` (a path of the offer) pointing at `to` lands inside the top-level item it came with. A link sent on its
/// own (at the top level) never does: its target is outside what was sent.
pub fn link_stays_inside(rel: &str, to: &str) -> bool {
    if to.is_empty() || to.starts_with('/') || to.contains([':', '\\']) {
        return false;
    }
    let mut at: Vec<&str> = rel.split('/').collect();
    at.pop();
    if at.is_empty() {
        return false;
    }
    for part in to.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if at.len() <= 1 {
                    return false;
                }
                at.pop();
            }
            p => at.push(p),
        }
    }
    true
}

fn part(target: &Path) -> PathBuf {
    let mut s = target.as_os_str().to_owned();
    s.push(PART);
    PathBuf::from(s)
}

fn hash_file(path: &Path) -> anyhow::Result<String> {
    use std::io::Read as _;
    let mut f = std::fs::File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

// ------------------------------------------------------------------------------------------------------------------------- pairing

/// Starts pairing with a device: both sides show the same code and both users confirm it.
pub async fn pair_with(inner: Arc<Inner>, addrs: Vec<SocketAddr>, peer_id: String, peer_name: String) {
    let r = async {
        let conn = connect(&inner, &addrs).await?;
        let fp = crate::tls::peer_fingerprint(&conn).ok_or_else(|| anyhow!("no certificate"))?;
        anyhow::ensure!(crate::identity::id_of(&fp) == peer_id, "another device answered at that address");
        let (mut send, mut recv) = conn.open_bi().await?;
        let hello = Hello { proto: proto::PROTOCOL, id: inner.me.id.clone(), name: inner.cfg.name.clone(), os: inner.cfg.os.clone(), purpose: Purpose::Pair };
        proto::write_frame(&mut send, &hello).await?;
        pair(&inner, &conn, &mut send, &mut recv, &peer_id, &peer_name, &fp).await
    }
    .await;
    if let Err(e) = r {
        log::warn!("nearby: pairing with {peer_id}: {e:#}");
        inner.listener.paired(peer_id, false);
    }
}

/// Both sides of a pairing run this: show the code, wait for this side's user, tell the other side, hear its answer; both yes: trusted.
async fn pair(inner: &Arc<Inner>, conn: &quinn::Connection, send: &mut quinn::SendStream, recv: &mut quinn::RecvStream, peer_id: &str, peer_name: &str, fp: &str) -> anyhow::Result<()> {
    if inner.visibility() == Visibility::Off {
        conn.close(2u32.into(), b"not visible");
        bail!("not visible");
    }
    let code = crate::tls::pairing_code(conn).ok_or_else(|| anyhow!("no pairing code"))?;
    let rx = inner.ask_pairing(peer_id);
    inner.listener.pairing_code(peer_id.to_string(), code);
    let mine = tokio::time::timeout(DECISION, rx).await.ok().and_then(|r| r.ok()).unwrap_or(false);
    proto::write_frame(send, &PairConfirm { ok: mine }).await?;
    let theirs: PairConfirm = tokio::time::timeout(DECISION, proto::read_frame(recv)).await.map_err(|_| anyhow!("no answer"))??;
    let _ = send.finish();
    let ok = mine && theirs.ok;
    if ok {
        inner.trust_device(peer_id, peer_name, fp);
    }
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    inner.listener.paired(peer_id.to_string(), ok);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taken_names_get_a_number_and_folders_keep_their_inside() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("pics")).unwrap();
        let items = vec![
            Item { path: "a.txt".into(), size: 1, ..Default::default() },
            Item { path: "pics/1.jpg".into(), size: 1, ..Default::default() },
            Item { path: "pics/sub/2.jpg".into(), size: 1, ..Default::default() },
            Item { path: "new.bin".into(), size: 1, ..Default::default() },
        ];
        let t = targets(dir.path(), &items);
        assert_eq!(t[0], dir.path().join("a (2).txt"));
        assert_eq!(t[1], dir.path().join("pics (2)").join("1.jpg"));
        assert_eq!(t[2], dir.path().join("pics (2)").join("sub").join("2.jpg"));
        assert_eq!(t[3], dir.path().join("new.bin"));
    }

    #[test]
    fn paths_that_differ_only_in_case_do_not_land_on_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let items = vec![
            Item { path: "Docs/Report.txt".into(), size: 1, ..Default::default() },
            Item { path: "Docs/report.txt".into(), size: 1, ..Default::default() },
            Item { path: "docs/other.txt".into(), size: 1, ..Default::default() },
        ];
        let t = targets(dir.path(), &items);
        assert_eq!(t[0], dir.path().join("Docs").join("Report.txt"));
        assert_eq!(t[1], dir.path().join("Docs").join("report (2).txt"));
        // (one top-level folder for both spellings of it)
        assert_eq!(t[2], dir.path().join("Docs").join("other.txt"));
        let top = vec![Item { path: "A.txt".into(), ..Default::default() }, Item { path: "a.txt".into(), ..Default::default() }];
        let t = targets(dir.path(), &top);
        assert_eq!(t, vec![dir.path().join("A.txt"), dir.path().join("A (2).txt")]);
    }

    #[test]
    fn links_must_stay_inside_what_was_sent() {
        assert!(link_stays_inside("App.app/Contents/Frameworks/Lib", "Versions/Current/Lib"));
        assert!(link_stays_inside("App.app/Contents/x", "../Resources/y"));
        assert!(!link_stays_inside("App.app/x", "../../etc/passwd"));
        assert!(!link_stays_inside("App.app/x", "../Other.app"));
        assert!(!link_stays_inside("lonely-link", "target"));
        assert!(!link_stays_inside("App.app/x", "/etc/passwd"));
        assert!(!link_stays_inside("App.app/x", "C:/Windows"));
    }
}
