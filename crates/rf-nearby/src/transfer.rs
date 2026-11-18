//! Sending, receiving and pairing over one QUIC connection each (see `proto`).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};

use crate::proto::{self, Answer, Done, FileHeader, Hello, Item, Outcome, PairConfirm, Purpose, SpeedReport};
use crate::{EstimateSource, Inner, Offer, SpeedResult, TransferResult, Visibility};

/// Files sent at once, each on its own stream.
const PARALLEL: usize = 4;
const CHUNK: usize = 1 << 20;
/// What the sender reads and the receiver writes at a time.
const BLOCK: usize = 4 << 20;
/// How long the other side may take to say yes to an offer or a pairing code.
const DECISION: Duration = Duration::from_secs(180);
const CONNECT: Duration = Duration::from_secs(6);
/// The suffix of a file being received: renamed away only once its BLAKE3 matches.
pub const PART: &str = ".ferrying";
/// A transfer that moved less than this tells nothing about the speed.
const SPEED_FROM_TRANSFER_MIN: u64 = 4 << 20;

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
    let resumed = answer.resume.iter().sum::<u64>().min(total);
    let done = Arc::new(AtomicU64::new(resumed));
    let ticker = progress_ticker(inner.clone(), tid, done.clone(), total);
    let started = std::time::Instant::now();

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
    let moved = total - resumed;
    if outcome.ok && moved >= SPEED_FROM_TRANSFER_MIN {
        inner.note_speed(&crate::identity::id_of(&fp), rate(moved, started.elapsed().as_micros() as u64), EstimateSource::Transfer, rtt_ms(&conn));
    }
    let _ = ctl_send.finish();
    conn.close(0u32.into(), b"done");
    inner.listener.progress(tid, total, total);
    if outcome.ok { Ok(()) } else { Err(anyhow!("{}", outcome.error.unwrap_or_else(|| "the device could not keep the files".into()))) }
}

/// Sends one file from `offset` on a stream of its own; returns its BLAKE3 (of the whole file).
/// Sends one file from `offset` on a stream of its own; returns its BLAKE3 (of the whole file). The file is read and hashed on a thread
/// of its own in big blocks, a few ahead of the network: reading it through the runtime a megabyte at a time cost a fifth of the speed.
async fn send_file(conn: &quinn::Connection, index: u32, path: &Path, offset: u64, done: &AtomicU64) -> anyhow::Result<String> {
    let mut stream = conn.open_uni().await?;
    proto::write_frame(&mut stream, &FileHeader { index, offset }).await?;
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(4);
    let path = path.to_path_buf();
    let reader = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        use std::io::Read as _;
        let mut file = std::fs::File::open(&path).with_context(|| format!("cannot open {}", path.display()))?;
        let mut hasher = blake3::Hasher::new();
        // (the part the other side already has still counts in the hash)
        if offset > 0 {
            let mut head = (&mut file).take(offset);
            let mut buf = vec![0u8; BLOCK];
            loop {
                let n = head.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }
        loop {
            let mut buf = vec![0u8; BLOCK];
            let n = read_full(&mut file, &mut buf)?;
            if n == 0 {
                break;
            }
            buf.truncate(n);
            hasher.update(&buf);
            if tx.blocking_send(buf).is_err() {
                // (the sending side gave up: its error is the one that counts)
                break;
            }
        }
        Ok(hasher.finalize().to_hex().to_string())
    });
    while let Some(buf) = rx.recv().await {
        stream.write_all(&buf).await?;
        done.fetch_add(buf.len() as u64, Ordering::Relaxed);
    }
    let hash = reader.await??;
    stream.finish()?;
    Ok(hash)
}

/// Reads until `buf` is full or the file ends.
fn read_full(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    use std::io::Read as _;
    let mut n = 0;
    while n < buf.len() {
        match file.read(&mut buf[n..])? {
            0 => break,
            k => n += k,
        }
    }
    Ok(n)
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
        Purpose::Speed => speed_serve(&inner, &conn, &mut ctl_send, &mut ctl_recv, &hello.id, &fp).await,
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
    let mut names: Vec<String> = Vec::new();
    for i in &items {
        let top = i.path.split('/').next().unwrap_or_default().to_string();
        if !names.contains(&top) {
            names.push(top);
        }
    }
    let dest = if trusted && inner.cfg.auto_accept_trusted {
        let offer = Offer {
            id: 0,
            transfer: tid,
            from_id: hello.id.clone(),
            from_name: hello.name.clone(),
            from_os: hello.os.clone(),
            trusted,
            names,
            file_count: items.iter().filter(|i| i.is_file()).count() as u64,
            total_bytes: items.iter().map(|i| i.size).sum(),
        };
        inner.listener.receiving(offer);
        Some(PathBuf::from(&inner.cfg.inbox))
    } else {
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
        if !inner.begin_asking(&hello.id) {
            proto::write_frame(ctl_send, &decline("the device is busy with other offers")).await?;
            return Err(Declined.into());
        }
        let rx = inner.ask(offer);
        let answer = tokio::time::timeout(DECISION, rx).await.ok().and_then(|r| r.ok()).flatten();
        inner.end_asking(&hello.id);
        answer
    };
    let Some(dest) = dest else {
        proto::write_frame(ctl_send, &decline("declined")).await?;
        return Err(Declined.into());
    };
    tokio::fs::create_dir_all(&dest).await.with_context(|| format!("cannot make {}", dest.display()))?;
    let targets = targets(&dest, &items);
    // everything is written at its staging path and takes its real name only once the whole transfer checks out
    let stages: Vec<PathBuf> = targets.iter().map(|t| staging(&dest, t)).collect();
    let resume: Vec<u64> = items
        .iter()
        .zip(&stages)
        .map(|(i, s)| if !i.is_file() { 0 } else { std::fs::metadata(s).map(|m| m.len()).ok().filter(|&l| l <= i.size).unwrap_or(0) })
        .collect();
    proto::write_frame(ctl_send, &Answer { accept: true, reason: None, resume: resume.clone() }).await?;
    let started = std::time::Instant::now();

    let total: u64 = items.iter().map(|i| i.size).sum();
    let done = Arc::new(AtomicU64::new(resume.iter().sum()));
    let ticker = progress_ticker(inner.clone(), tid, done.clone(), total);
    let files = items.iter().filter(|i| i.is_file()).count();
    let stages = Arc::new(stages);
    let mut tasks = Vec::new();
    for _ in 0..files {
        let mut stream = conn.accept_uni().await?;
        let (stages, done) = (stages.clone(), done.clone());
        let sizes: Vec<u64> = items.iter().map(|i| i.size).collect();
        tasks.push(tokio::spawn(async move {
            let header: FileHeader = proto::read_frame(&mut stream).await?;
            let index = header.index as usize;
            let stage = stages.get(index).ok_or_else(|| anyhow!("no item {index}"))?;
            anyhow::ensure!(header.offset <= sizes[index], "offset past the end");
            if let Some(dir) = stage.parent() {
                tokio::fs::create_dir_all(dir).await?;
            }
            // the file is written, and hashed as it is written, on a thread of its own (the network hands over small pieces: one
            // trip to the runtime's file threads each cost a fifth of the speed, and hashing it all again at the end took minutes)
            let offset = header.offset;
            let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(1024);
            let path = stage.clone();
            let writer = tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
                use std::io::{Read as _, Seek as _, Write as _};
                let mut file = std::fs::OpenOptions::new().create(true).read(true).write(true).truncate(false).open(&path)?;
                file.set_len(offset)?;
                let mut hasher = blake3::Hasher::new();
                // (what an earlier attempt left counts in the hash too)
                if offset > 0 {
                    let mut head = (&mut file).take(offset);
                    let mut buf = vec![0u8; BLOCK];
                    loop {
                        let n = head.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                    }
                }
                file.seek(std::io::SeekFrom::Start(offset))?;
                let mut out = std::io::BufWriter::with_capacity(BLOCK, file);
                while let Some(piece) = rx.blocking_recv() {
                    hasher.update(&piece);
                    out.write_all(&piece)?;
                }
                out.flush()?;
                Ok(hasher.finalize().to_hex().to_string())
            });
            let mut written = offset;
            while let Some(chunk) = stream.read_chunk(CHUNK, true).await? {
                written += chunk.bytes.len() as u64;
                anyhow::ensure!(written <= sizes[index], "more bytes than offered");
                let n = chunk.bytes.len() as u64;
                if tx.send(chunk.bytes.to_vec()).await.is_err() {
                    // (the writer stopped: its error is the one that counts)
                    break;
                }
                done.fetch_add(n, Ordering::Relaxed);
            }
            drop(tx);
            let hash = writer.await??;
            anyhow::Ok((index, hash))
        }));
    }
    let mut got_hashes = std::collections::HashMap::new();
    for t in tasks {
        let (index, hash) = t.await??;
        got_hashes.insert(index, hash);
    }
    ticker.store(true, Ordering::Relaxed);
    let Done { hashes } = proto::read_frame(ctl_recv).await?;
    // every file is checked; a damaged one is thrown away, so the next attempt sends it again instead of going on from its end
    let mut failure = None;
    for (index, item) in items.iter().enumerate() {
        let stage = &stages[index];
        if item.dir {
            tokio::fs::create_dir_all(stage).await?;
            continue;
        }
        if !item.is_file() {
            continue;
        }
        let want = hashes.get(index).cloned().unwrap_or_default();
        let got = got_hashes.get(&index).cloned().unwrap_or_default();
        if got != want {
            let _ = tokio::fs::remove_file(stage).await;
            failure = Some(format!("{} arrived damaged", item.path));
            continue;
        }
        keep_attributes(stage, item);
    }
    // links last: none exists while files are written, so no file can be written through one
    let mut tops: Vec<String> = Vec::new();
    if failure.is_none() {
        for (index, item) in items.iter().enumerate() {
            if let Some(to) = &item.link {
                make_link(&stages[index], to, &item.path);
            }
        }
        // then each top-level item takes its real name (a fresh one, if something took it meanwhile)
        let mut seen = std::collections::HashSet::new();
        for target in targets.iter() {
            let top = top_of(&dest, target);
            if !seen.insert(top.clone()) {
                continue;
            }
            let stage = staging(&dest, &top);
            if std::fs::symlink_metadata(&stage).is_err() {
                continue;
            }
            let name = top.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let to = dest.join(free_name(&dest, &name));
            if let Err(e) = rename_retrying(&stage, &to).await {
                failure = Some(format!("cannot name {}: {e}", to.display()));
                break;
            }
            tops.push(to.to_string_lossy().to_string());
        }
    }
    let outcome = Outcome { ok: failure.is_none(), error: failure.clone() };
    proto::write_frame(ctl_send, &outcome).await?;
    let moved = total.saturating_sub(resume.iter().sum());
    if failure.is_none() && moved >= SPEED_FROM_TRANSFER_MIN {
        inner.note_speed(&hello.id, rate(moved, started.elapsed().as_micros() as u64), EstimateSource::Transfer, rtt_ms(conn));
    }
    let _ = ctl_send.finish();
    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
    inner.listener.progress(tid, total, total);
    if let Some(f) = failure {
        bail!(f);
    }
    Ok(tops)
}

/// Where each item goes under `dest`, each name as this platform can store it ([`proto::local_name`]). A top-level name that is
/// already taken there (by a finished file or folder, not by an unfinished `.ferrying`) gets " (2)", " (3)"... before its extension, and
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

/// Where an item is written while it arrives: a top-level file as `<name>.ferrying`, and everything of a top-level folder under
/// `<folder>.ferrying/`. A broken transfer so leaves nothing that looks finished, and sending the same things again finds the parts
/// at the same place (the real name is still free) and goes on from there.
fn staging(dest: &Path, target: &Path) -> PathBuf {
    let rel = target.strip_prefix(dest).unwrap_or(target);
    let mut parts = rel.components();
    let Some(top) = parts.next() else { return part(target) };
    let mut name = top.as_os_str().to_owned();
    name.push(PART);
    let mut p = dest.join(name);
    p.extend(parts);
    p
}

/// The top-level item `target` belongs to (itself, for a top-level file).
fn top_of(dest: &Path, target: &Path) -> PathBuf {
    match target.strip_prefix(dest).ok().and_then(|r| r.components().next()) {
        Some(top) => dest.join(top),
        None => target.to_path_buf(),
    }
}

/// A rename that tries again for a moment: on Windows a virus scanner or the indexer may still hold a just-written file.
async fn rename_retrying(from: &Path, to: &Path) -> std::io::Result<()> {
    let mut tries = 0;
    loop {
        match tokio::fs::rename(from, to).await {
            Err(e) if tries < 10 && e.kind() == std::io::ErrorKind::PermissionDenied => {
                tries += 1;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            r => return r,
        }
    }
}

fn part(target: &Path) -> PathBuf {
    suffixed(target, PART)
}

fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

// ------------------------------------------------------------------------------------------------------------------------- speed test

/// How long each direction of a speed test is counted (the receiving side stops the stream then) and the most a side sends.
const SPEED_TIME: Duration = Duration::from_secs(2);
const SPEED_MAX: u64 = 512 << 20;
/// The start of each direction is not counted: QUIC's congestion control and Wi-Fi's rate are still ramping up. 200 ms was too short:
/// the direction measured first came out 15-25% lower than the same direction measured second (Windows and Mac, 2026-11-18).
const SPEED_WARMUP: Duration = Duration::from_millis(750);
/// The same device may test this one once in this long, and only this many tests run here at once.
pub(crate) const SPEED_SPACING: Duration = Duration::from_secs(10);
pub(crate) const SPEED_AT_ONCE: usize = 2;

fn rate(bytes: u64, micros: u64) -> u64 {
    if micros == 0 { 0 } else { (u128::from(bytes) * 1_000_000 / u128::from(micros)) as u64 }
}

fn rtt_ms(conn: &quinn::Connection) -> u32 {
    conn.rtt().as_millis().min(u128::from(u32::MAX)) as u32
}

/// Runs a speed test with a device: up (this side sends, the other counts and reports), then down (the other side sends, this
/// one counts).
pub async fn speed_test(inner: Arc<Inner>, addrs: Vec<SocketAddr>, peer_id: String) -> SpeedResult {
    if !inner.begin_measuring(&peer_id) {
        return SpeedResult { error: Some("a speed test with the device is running".into()), ..Default::default() };
    }
    let r = speed_client(&inner, &addrs, &peer_id).await;
    inner.end_measuring(&peer_id);
    match r {
        Ok((up, down, rtt, id)) => {
            inner.note_speed(&id, up.min(down), EstimateSource::Measured, rtt);
            SpeedResult { ok: true, up_bps: up, down_bps: down, rtt_ms: rtt, error: None }
        }
        Err(e) => {
            log::info!("nearby: speed test with {peer_id}: {e:#}");
            SpeedResult { ok: false, error: Some(format!("{e:#}")), ..Default::default() }
        }
    }
}

async fn speed_client(inner: &Arc<Inner>, addrs: &[SocketAddr], peer_id: &str) -> anyhow::Result<(u64, u64, u32, String)> {
    let conn = connect(inner, addrs).await?;
    let fp = crate::tls::peer_fingerprint(&conn).ok_or_else(|| anyhow!("the device showed no certificate"))?;
    let id = crate::identity::id_of(&fp);
    anyhow::ensure!(peer_id.is_empty() || id == peer_id, "another device answered at that address");
    let (mut send, mut recv) = conn.open_bi().await?;
    let hello = Hello { proto: proto::PROTOCOL, id: inner.me.id.clone(), name: inner.cfg.name.clone(), os: inner.cfg.os.clone(), purpose: Purpose::Speed };
    proto::write_frame(&mut send, &hello).await?;
    let answer: Answer = tokio::time::timeout(CONNECT, proto::read_frame(&mut recv)).await.map_err(|_| anyhow!("no answer from the device"))??;
    if !answer.accept {
        bail!("{}", answer.reason.unwrap_or_else(|| "the device declined".into()));
    }
    // the round trip before the load: after it, the queues the test filled make it look several times longer
    let rtt = rtt_ms(&conn);
    blast(conn.open_uni().await?).await;
    let report: SpeedReport = tokio::time::timeout(SPEED_TIME * 3, proto::read_frame(&mut recv)).await.map_err(|_| anyhow!("the device did not report"))??;
    let stream = tokio::time::timeout(SPEED_TIME * 2, conn.accept_uni()).await.map_err(|_| anyhow!("the device sent nothing"))??;
    let (bytes, micros) = measure(stream).await?;
    let _ = send.finish();
    conn.close(0u32.into(), b"done");
    Ok((rate(report.bytes, report.micros), rate(bytes, micros), rtt, id))
}

/// The other side of a speed test: nothing is written to disk and nothing is asked; it is taken from the devices this one's
/// visibility admits, a few at a time.
async fn speed_serve(inner: &Arc<Inner>, conn: &quinn::Connection, send: &mut quinn::SendStream, recv: &mut quinn::RecvStream, peer_id: &str, fp: &str) -> anyhow::Result<()> {
    let _ = recv;
    let reason = if inner.visibility() == Visibility::Trusted && !inner.trusts(fp) {
        Some("the device only takes files from its trusted devices")
    } else if !inner.begin_speed_test(peer_id) {
        Some("the device is busy with another speed test")
    } else {
        None
    };
    if let Some(reason) = reason {
        proto::write_frame(send, &Answer { accept: false, reason: Some(reason.to_string()), resume: Vec::new() }).await?;
        // (the answer must arrive before the connection goes: the other side closes it once it has read it)
        let _ = send.finish();
        let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
        return Ok(());
    }
    let r = async {
        proto::write_frame(send, &Answer { accept: true, reason: None, resume: Vec::new() }).await?;
        inner.note_rtt(peer_id, rtt_ms(conn));
        let stream = tokio::time::timeout(SPEED_TIME * 2, conn.accept_uni()).await.map_err(|_| anyhow!("nothing came"))??;
        let (bytes, micros) = measure(stream).await?;
        proto::write_frame(send, &SpeedReport { bytes, micros }).await?;
        blast(conn.open_uni().await?).await;
        let _ = tokio::time::timeout(SPEED_TIME * 3, conn.closed()).await;
        anyhow::Ok(())
    }
    .await;
    inner.end_speed_test(peer_id);
    r
}

/// Sends bytes that do not compress until the other side stops reading, or for a little longer than it counts, or `SPEED_MAX`.
async fn blast(mut s: quinn::SendStream) {
    let mut buf = vec![0u8; 64 << 10];
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    for b in buf.chunks_mut(8) {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        b.copy_from_slice(&x.to_le_bytes()[..b.len()]);
    }
    let start = std::time::Instant::now();
    let mut sent = 0u64;
    while sent < SPEED_MAX && start.elapsed() < SPEED_WARMUP + SPEED_TIME + Duration::from_millis(500) {
        // (an error: the other side stopped reading, its time is up)
        if s.write_all(&buf).await.is_err() {
            return;
        }
        sent += buf.len() as u64;
    }
    let _ = s.finish();
}

/// Counts what arrives on a stream for `SPEED_TIME` from its first byte (then stops it), leaving out the warm-up: (bytes, µs).
async fn measure(mut s: quinn::RecvStream) -> anyhow::Result<(u64, u64)> {
    let begun = std::time::Instant::now();
    let mut first: Option<std::time::Instant> = None;
    let mut warm: Option<(std::time::Instant, u64)> = None;
    let mut total = 0u64;
    loop {
        let until = match first {
            Some(f) => f + SPEED_WARMUP + SPEED_TIME,
            None => begun + SPEED_TIME * 2,
        };
        match tokio::time::timeout(until.saturating_duration_since(std::time::Instant::now()), s.read_chunk(1 << 20, false)).await {
            Ok(Ok(Some(c))) => {
                let now = std::time::Instant::now();
                let f = *first.get_or_insert(now);
                total += c.bytes.len() as u64;
                if warm.is_none() && now.duration_since(f) >= SPEED_WARMUP {
                    warm = Some((now, total));
                }
            }
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.into()),
            Err(_) => {
                let _ = s.stop(0u32.into());
                break;
            }
        }
    }
    let end = std::time::Instant::now();
    let Some(f) = first else { bail!("nothing arrived") };
    Ok(match warm {
        Some((t, b)) if total > b && end > t => (total - b, end.duration_since(t).as_micros() as u64),
        _ => (total, end.duration_since(f).as_micros().max(1) as u64),
    })
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
