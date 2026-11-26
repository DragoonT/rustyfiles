//! Whether the server behind a network folder answers at all, asked quickly before the folder is read.
//!
//! A share on a server that is asleep, switched off or gone keeps a listing waiting for Windows' own SMB timeout: about 21 s here
//! (measured on 2026-11-23 against an address nobody answers), up to a minute elsewhere. A TCP connection to the server's SMB port
//! answers or not within a couple of seconds, so the folder says "the server does not answer" at once instead.
//!
//! Only plain servers are asked: `\\wsl$`, `\\wsl.localhost`, `\\tsclient` (redirectors, not TCP servers), WebDAV names
//! (`\\host@SSL\...`) and the local machine go straight to the listing. A name that does not resolve goes to the listing too: Windows
//! then gives its own error. A server reached only over SMB over QUIC (UDP) would read as silent; pressing F5 a few seconds later
//! asks again.
//!
//! A network drive that is not an SMB share is never asked. Cloud drives mounted through WinFsp (Filen, rclone) are network drives
//! of a server name that exists nowhere (`\\Filen\Filen`): no machine has that name, Windows spends 2 to 4 seconds finding that
//! out (measured here: 2.3 s and 3.7 s), the limit passed meanwhile and the drive read as "the server does not answer" (0.9.9, a
//! Discord report: "I can't open my network drive with rustyfiles"). Two things keep that from happening: a drive letter is asked
//! only when the SMB redirector serves it ([`smb_device`]), and a lookup that is still running at the limit is not an answer of
//! "silent" ([`ask`]).

use std::collections::HashMap;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long the whole question (name lookup and connection) may take.
const ASK_FOR: Duration = Duration::from_secs(2);
/// How long an answer is kept: a silent server briefly (F5 soon asks again), one that answered longer.
const KEEP_SILENT: Duration = Duration::from_secs(5);
const KEEP_ANSWERED: Duration = Duration::from_secs(60);
/// SMB, and NetBIOS session service for old servers.
const PORTS: [u16; 2] = [445, 139];

static SEEN: Mutex<Option<HashMap<String, (Instant, bool)>>> = Mutex::new(None);

/// The server of a UNC path (`\\server\share\...`, `\\?\UNC\server\share\...`) or of a network drive letter, when it is one to ask.
pub fn server_of(p: &Path) -> Option<String> {
    let s = p.to_string_lossy();
    let unc = if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        rest.to_string()
    } else if s.starts_with(r"\\?\") || s.starts_with(r"\\.\") {
        drive_target(s.get(4..)?)?
    } else if let Some(rest) = s.strip_prefix(r"\\") {
        rest.to_string()
    } else {
        drive_target(&s)?
    };
    let server = unc.split(['\\', '/']).next()?.to_string();
    worth_asking(&server).then_some(server)
}

fn worth_asking(server: &str) -> bool {
    let low = server.to_ascii_lowercase();
    !(low.is_empty()
        || low.contains('@')
        || matches!(low.as_str(), "wsl$" | "wsl.localhost" | "tsclient" | "localhost" | "127.0.0.1" | "::1" | "." | "?"))
}

/// `X:...` on a network drive: the `server\share` it is connected to (without the leading backslashes).
fn drive_target(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 2 || b[1] != b':' || !b[0].is_ascii_alphabetic() {
        return None;
    }
    use windows::Win32::NetworkManagement::WNet::WNetGetConnectionW;
    use windows::core::PCWSTR;
    let local = crate::wide::wide(format!("{}:", b[0] as char));
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    // (only a share of the SMB redirector has a server to ask: a WinFsp, WebDAV or NFS drive is listed straight away)
    if !smb_drive(b[0] as char) {
        return None;
    }
    // (a local drive letter answers "not a network drive" without any network traffic)
    let r = unsafe { WNetGetConnectionW(PCWSTR(local.as_ptr()), Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut len) };
    if r.0 != 0 {
        return None;
    }
    let remote = crate::wide::from_wide(&buf);
    remote.strip_prefix(r"\\").map(str::to_string)
}

/// Whether the drive letter is a share of Windows' SMB redirector (read from the object namespace: nothing goes over the network).
fn smb_drive(letter: char) -> bool {
    use windows::Win32::Storage::FileSystem::QueryDosDeviceW;
    use windows::core::PCWSTR;
    let name = crate::wide::wide(format!("{letter}:"));
    let mut buf = vec![0u16; 1024];
    let n = unsafe { QueryDosDeviceW(PCWSTR(name.as_ptr()), Some(&mut buf)) } as usize;
    // (not readable: asked as before, the lookup's own rule still protects a made-up name)
    n == 0 || smb_device(&crate::wide::from_wide(&buf[..n.min(buf.len())]))
}

/// Whether a drive letter's device (`\Device\LanmanRedirector\;Z:0000000000012345\nas\media`) is the SMB redirector's.
fn smb_device(target: &str) -> bool {
    target.to_ascii_lowercase().starts_with(r"\device\lanmanredirector")
}

/// Whether `server` answers on its SMB port within [`ASK_FOR`] (kept for a while, see [`KEEP_SILENT`]).
pub fn answers(server: &str) -> bool {
    let key = server.to_ascii_lowercase();
    if let Some((at, ok)) = SEEN.lock().unwrap().as_ref().and_then(|m| m.get(&key).copied()) {
        if at.elapsed() < if ok { KEEP_ANSWERED } else { KEEP_SILENT } {
            return ok;
        }
    }
    let ok = ask(server);
    SEEN.lock().unwrap().get_or_insert_with(HashMap::new).insert(key, (Instant::now(), ok));
    if !ok {
        log::info!("netprobe: {server} did not answer within {ASK_FOR:?}");
    }
    ok
}

/// The question itself, on a thread of its own so a name lookup that hangs cannot hold the caller past [`ASK_FOR`].
fn ask(server: &str) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let host = server.to_string();
    let started = Instant::now();
    // (set once the name is known to be a machine's: only then can "no answer in time" mean a silent server)
    let named = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let named_there = named.clone();
    std::thread::spawn(move || {
        let addrs: Vec<_> = match (host.as_str(), PORTS[0]).to_socket_addrs() {
            Ok(a) => a.collect(),
            // not a name this lookup knows: the listing goes ahead and Windows says what is wrong
            Err(_) => {
                let _ = tx.send(true);
                return;
            }
        };
        named_there.store(true, std::sync::atomic::Ordering::Relaxed);
        for port in PORTS {
            for a in &addrs {
                let left = ASK_FOR.saturating_sub(started.elapsed());
                if left.is_zero() {
                    let _ = tx.send(false);
                    return;
                }
                let mut a = *a;
                a.set_port(port);
                if TcpStream::connect_timeout(&a, left).is_ok() {
                    let _ = tx.send(true);
                    return;
                }
            }
        }
        let _ = tx.send(false);
    });
    // The limit passed. With the name still being looked up nothing is known about any server: Windows takes seconds to give up
    // on a name no machine has, and that is what a cloud drive's made-up server name is. The listing goes ahead.
    rx.recv_timeout(ASK_FOR + Duration::from_millis(200)).unwrap_or_else(|_| !named.load(std::sync::atomic::Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn servers_are_taken_from_unc_paths_and_redirectors_are_left_alone() {
        assert_eq!(server_of(Path::new(r"\\nas\media\films")), Some("nas".into()));
        assert_eq!(server_of(Path::new(r"\\?\UNC\192.168.1.20\share")), Some("192.168.1.20".into()));
        assert_eq!(server_of(Path::new(r"\\wsl$\Ubuntu\home")), None);
        assert_eq!(server_of(Path::new(r"\\wsl.localhost\Ubuntu")), None);
        assert_eq!(server_of(Path::new(r"\\tsclient\C")), None);
        assert_eq!(server_of(Path::new(r"\\files.example.com@SSL\DavWWWRoot")), None);
        assert_eq!(server_of(Path::new(r"\\localhost\c$")), None);
        assert_eq!(server_of(Path::new(r"\\?\C:\Windows")), None);
        assert_eq!(server_of(Path::new(r"C:\Windows")), None);
    }

    #[test]
    fn only_a_drive_of_the_smb_redirector_has_a_server_to_ask() {
        assert!(smb_device(r"\Device\LanmanRedirector\;Z:0000000000012345\nas\media"));
        assert!(!smb_device(r"\Device\WinFsp.Mup\;X:\Filen\Filen"));
        assert!(!smb_device(r"\Device\WebDavRedirector\;W:0000000000012345\files.example.com@SSL\DavWWWRoot"));
        assert!(!smb_device(r"\Device\HarddiskVolume3"));
        // this machine's system drive is a disk, not a share
        assert!(!smb_drive('C'));
    }

    #[test]
    fn a_name_no_machine_has_is_not_a_silent_server() {
        // (a cloud drive's made-up server name: Windows needs longer than the limit to give up on it, and the folder must be listed)
        let t = Instant::now();
        assert!(answers("rf-no-such-machine-zq7"));
        assert!(t.elapsed() < ASK_FOR + Duration::from_millis(800), "took {:?}", t.elapsed());
    }

    #[test]
    fn a_server_nobody_answers_is_told_within_the_limit() {
        // (a documentation address, TEST-NET-1: nothing answers there)
        let t = Instant::now();
        assert!(!answers("192.0.2.1"));
        assert!(t.elapsed() < ASK_FOR + Duration::from_millis(500), "took {:?}", t.elapsed());
        // the answer is kept: asked again, it comes at once
        let t = Instant::now();
        assert!(!answers("192.0.2.1"));
        assert!(t.elapsed() < Duration::from_millis(50));
    }
}
