//! Playing a file while it downloads (HyperDownload, the download manager add-on): the file is read from the add-on's local
//! stream (`http://127.0.0.1:<port>/api/torrents/<id>/stream/<file>`, which answers byte ranges and waits for the pieces it is asked
//! for, putting them first). Reads go in blocks of 1 MiB, the last few kept, so the decoder's many small reads do not each become a
//! request.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, anyhow};
use filmcraft_media::reader::{ByteReader, open_reader_within};

use crate::source::Media;

const BLOCK: u64 = 1 << 20;
/// Blocks kept (16 MiB).
const KEPT: usize = 16;

pub struct HttpReader {
    url: String,
    len: u64,
    agent: ureq::Agent,
    blocks: Mutex<VecDeque<(u64, Arc<Vec<u8>>)>>,
}

fn agent() -> ureq::Agent {
    // (a piece not downloaded yet is waited for by the server: a read can take a while on a slow torrent)
    ureq::Agent::config_builder().timeout_connect(Some(Duration::from_secs(5))).timeout_recv_body(Some(Duration::from_secs(120))).http_status_as_error(false).build().into()
}

/// `bytes 0-0/12345` → 12345.
fn total_of(content_range: &str) -> Option<u64> {
    content_range.rsplit('/').next()?.trim().parse().ok()
}

impl HttpReader {
    /// Opens the stream: its length from the answer to a one-byte range.
    pub fn open(url: &str) -> anyhow::Result<HttpReader> {
        let agent = agent();
        let resp = agent.get(url).header("Range", "bytes=0-0").call().with_context(|| url.to_string())?;
        if resp.status().as_u16() != 206 {
            return Err(anyhow!("the stream did not answer a range (HTTP {})", resp.status().as_u16()));
        }
        let len = resp.headers().get("content-range").and_then(|v| v.to_str().ok()).and_then(total_of).ok_or_else(|| anyhow!("the stream did not say its length"))?;
        Ok(HttpReader { url: url.to_string(), len, agent, blocks: Mutex::new(VecDeque::new()) })
    }

    fn block(&self, start: u64) -> io::Result<Arc<Vec<u8>>> {
        if let Some(b) = self.blocks.lock().unwrap().iter().find(|(s, _)| *s == start).map(|(_, b)| b.clone()) {
            return Ok(b);
        }
        let end = (start + BLOCK).min(self.len) - 1;
        let resp = self.agent.get(&self.url).header("Range", format!("bytes={start}-{end}")).call().map_err(io::Error::other)?;
        if resp.status().as_u16() != 206 {
            return Err(io::Error::other(format!("HTTP {}", resp.status().as_u16())));
        }
        let mut data = Vec::with_capacity((end + 1 - start) as usize);
        io::Read::read_to_end(&mut resp.into_body().into_reader(), &mut data)?;
        if data.len() as u64 != end + 1 - start {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the stream sent less than asked"));
        }
        let b = Arc::new(data);
        let mut kept = self.blocks.lock().unwrap();
        kept.push_back((start, b.clone()));
        while kept.len() > KEPT {
            kept.pop_front();
        }
        Ok(b)
    }
}

impl ByteReader for HttpReader {
    fn len(&self) -> u64 {
        self.len
    }

    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if offset + buf.len() as u64 > self.len {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "read past the end of the stream"));
        }
        let mut done = 0usize;
        while done < buf.len() {
            let at = offset + done as u64;
            let start = at - at % BLOCK;
            let b = self.block(start)?;
            let from = (at - start) as usize;
            let n = (b.len() - from).min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&b[from..from + n]);
            done += n;
        }
        Ok(())
    }
}

/// Opens a file being downloaded from its local stream; `name` gives its kind (its extension).
pub fn open_stream(url: &str, name: &str) -> anyhow::Result<Media> {
    let reader = HttpReader::open(url)?;
    let src = open_reader_within(name, Arc::new(reader), &filmcraft_codecs::reader_openers(), &filmcraft_codecs::openers(), crate::source::WHOLE_FILE_MAX)
        .map_err(|e| anyhow!("{name}: {e}"))?;
    Ok(Media::from_source(src))
}

/// The first `n` bytes of the stream (what a viewer looks at to tell the kind of file).
pub fn head(url: &str, n: usize) -> Vec<u8> {
    let Ok(r) = HttpReader::open(url) else { return Vec::new() };
    let n = (n as u64).min(r.len) as usize;
    let mut buf = vec![0u8; n];
    if r.read_at(0, &mut buf).is_ok() { buf } else { Vec::new() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};

    #[test]
    fn a_stream_is_read_in_blocks_across_their_edges() {
        let body: Vec<u8> = (0..(3 * BLOCK + 123) as u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let served = Arc::new(body.clone());
        let asked = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = asked.clone();
        std::thread::spawn(move || {
            for c in listener.incoming().flatten() {
                let (b, n) = (served.clone(), counted.clone());
                std::thread::spawn(move || {
                    let mut r = BufReader::new(c.try_clone().unwrap());
                    let (mut range, mut line) = ((0usize, 0usize), String::new());
                    loop {
                        line.clear();
                        if r.read_line(&mut line).unwrap_or(0) == 0 || line.trim().is_empty() {
                            break;
                        }
                        if let Some(v) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                            let (a, z) = v.trim().split_once('-').unwrap();
                            range = (a.parse().unwrap(), z.parse().unwrap());
                        }
                    }
                    n.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    let (a, z) = range;
                    let mut c = c;
                    let _ = c.write_all(format!("HTTP/1.1 206 P\r\nContent-Length: {}\r\nContent-Range: bytes {a}-{z}/{}\r\nConnection: close\r\n\r\n", z + 1 - a, b.len()).as_bytes());
                    let _ = c.write_all(&b[a..=z]);
                });
            }
        });
        let r = HttpReader::open(&format!("http://127.0.0.1:{port}/s")).unwrap();
        assert_eq!(r.len(), body.len() as u64);
        // across the first block's end, then back inside it (kept: no new request)
        let mut buf = vec![0u8; 5000];
        r.read_at(BLOCK - 2000, &mut buf).unwrap();
        assert_eq!(buf, body[(BLOCK - 2000) as usize..(BLOCK + 3000) as usize]);
        let before = asked.load(std::sync::atomic::Ordering::SeqCst);
        let mut small = vec![0u8; 10];
        r.read_at(100, &mut small).unwrap();
        assert_eq!(small, body[100..110]);
        assert_eq!(asked.load(std::sync::atomic::Ordering::SeqCst), before, "a kept block is not asked again");
        // the short last block, and past the end
        let mut tail = vec![0u8; 123];
        r.read_at(3 * BLOCK, &mut tail).unwrap();
        assert_eq!(tail, body[(3 * BLOCK) as usize..]);
        assert!(r.read_at(body.len() as u64 - 5, &mut [0u8; 10]).is_err());
    }
}
