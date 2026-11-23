//! FTP browsing and transfers through WinINet (built into Windows: no extra dependency, uses
//! the system proxy settings). Locations are `ftp://[user[:pass]@]host[:port]/path` strings;
//! inside the app they travel as `PathBuf`s, so backslashes that `Path::join` adds are
//! normalized here. Blocking: call off the UI thread.

use std::path::Path;

use rf_core::Entry;
use windows::Win32::Networking::WinInet::{
    FTP_TRANSFER_TYPE_BINARY, FtpCreateDirectoryW, FtpDeleteFileW, FtpFindFirstFileW, FtpGetFileW, FtpPutFileW, FtpRemoveDirectoryW,
    FtpRenameFileW, INTERNET_DEFAULT_FTP_PORT,
    INTERNET_FLAG_NO_CACHE_WRITE, INTERNET_FLAG_PASSIVE, INTERNET_FLAG_RELOAD, INTERNET_OPEN_TYPE_PRECONFIG,
    INTERNET_SERVICE_FTP, InternetCloseHandle, InternetConnectW, InternetFindNextFileW, InternetOpenW,
};
use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::core::PCWSTR;

use crate::wide::wide;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FtpUrl {
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    pub pass: Option<String>,
    /// Always starts with '/', no trailing '/' except for the root.
    pub path: String,
}

pub fn is_ftp(p: &Path) -> bool {
    // byte-safe: never slice inside a multi-byte character (C:\Öğrenci)
    p.to_str().is_some_and(|s| s.len() > 6 && s.as_bytes()[..6].eq_ignore_ascii_case(b"ftp://"))
}

impl FtpUrl {
    pub fn parse(s: &str) -> Option<FtpUrl> {
        let s = s.replace('\\', "/");
        let rest = s.get(..6).filter(|p| p.eq_ignore_ascii_case("ftp://")).map(|_| &s[6..])?;
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let (cred, hostport) = match authority.rfind('@') {
            Some(i) => (Some(&authority[..i]), &authority[i + 1..]),
            None => (None, authority),
        };
        let (user, pass) = match cred {
            Some(c) => match c.split_once(':') {
                Some((u, p)) => (Some(u.to_string()), Some(p.to_string())),
                None => (Some(c.to_string()), None),
            },
            None => (None, None),
        };
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => (h, p.parse().ok()?),
            _ => (hostport, INTERNET_DEFAULT_FTP_PORT),
        };
        if host.is_empty() {
            return None;
        }
        let mut path = path.replace("//", "/");
        while path.len() > 1 && path.ends_with('/') {
            path.pop();
        }
        Some(FtpUrl { host: host.to_string(), port, user, pass, path })
    }
}

struct Handle(*mut core::ffi::c_void);
impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = InternetCloseHandle(self.0);
            }
        }
    }
}

/// WinINet codes live in wininet.dll's message table, which `io::Error` cannot format.
fn wininet_message(code: i32) -> Option<&'static str> {
    Some(match code {
        12002 => "zaman aşımı",
        12007 => "sunucu adı çözülemedi",
        12013 => "kullanıcı adı yanlış",
        12014 => "parola yanlış",
        12015 => "oturum açılamadı",
        12029 => "sunucuya bağlanılamadı",
        12030 => "bağlantı kesildi",
        12031 => "bağlantı sunucu tarafından sıfırlandı",
        12003 => "sunucu isteği reddetti",
        12111 => "FTP oturumu iptal edildi",
        _ => return None,
    })
}

fn describe(e: std::io::Error) -> String {
    match e.raw_os_error().and_then(wininet_message) {
        Some(m) => m.to_string(),
        None => e.to_string(),
    }
}

fn last_err(what: &str) -> std::io::Error {
    let e = std::io::Error::last_os_error();
    std::io::Error::new(e.kind(), format!("{what}: {}", describe(e)))
}

/// (session, connection) — the session must outlive the connection; keep both.
fn connect(u: &FtpUrl) -> std::io::Result<(Handle, Handle)> {
    let agent = wide("rustyfiles");
    let session = Handle(unsafe { InternetOpenW(PCWSTR(agent.as_ptr()), INTERNET_OPEN_TYPE_PRECONFIG.0, PCWSTR::null(), PCWSTR::null(), 0) });
    if session.0.is_null() {
        return Err(last_err("InternetOpen"));
    }
    let host = wide(&u.host);
    let user = u.user.as_deref().map(wide);
    let pass = u.pass.as_deref().map(wide);
    let p = |w: &Option<Vec<u16>>| w.as_ref().map_or(PCWSTR::null(), |w| PCWSTR(w.as_ptr()));
    let conn = Handle(unsafe {
        InternetConnectW(session.0, PCWSTR(host.as_ptr()), u.port, p(&user), p(&pass), INTERNET_SERVICE_FTP, INTERNET_FLAG_PASSIVE, None)
    });
    if conn.0.is_null() {
        return Err(last_err("FTP bağlantısı"));
    }
    Ok((session, conn))
}

/// Lists a remote directory.
pub fn list(url: &str) -> std::io::Result<Vec<Entry>> {
    let u = parsed(url)?;
    let (_s, conn) = connect(&u)?;
    list_on(&conn, &u.path)
}

fn parsed(url: &str) -> std::io::Result<FtpUrl> {
    FtpUrl::parse(url).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "geçersiz FTP adresi"))
}

/// `name` inside the remote directory `dir` (a path on the server).
fn join(dir: &str, name: &str) -> String {
    if dir == "/" { format!("/{name}") } else { format!("{dir}/{name}") }
}

/// The address of `name` inside the directory at `dir_url`, with slashes whatever `dir_url` had (`Path::join` adds backslashes).
pub fn child_url(dir_url: &str, name: &str) -> String {
    format!("{}/{name}", dir_url.replace('\\', "/").trim_end_matches('/'))
}

fn failed(what: &str, e: windows::core::Error) -> std::io::Error {
    std::io::Error::other(format!("{what}: {}", describe(std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF))))
}

/// Deletes `url`: a file, or a directory with all it holds (the server's own delete; Windows' FTP folder cannot be asked: its delete
/// through the shell's engine answers "No such interface supported", issue 223).
///
/// Only what the listing said is a directory is emptied: a server that answers a file's listing with its folder's must not have the
/// file's neighbours deleted.
pub fn delete(url: &str, is_dir: bool) -> std::io::Result<()> {
    let u = parsed(url)?;
    let (_s, conn) = connect(&u)?;
    delete_on(&conn, &u.path, is_dir, 0)
}

fn delete_on(conn: &Handle, path: &str, is_dir: bool, depth: u32) -> std::io::Result<()> {
    let w = wide(path);
    if !is_dir {
        return unsafe { FtpDeleteFileW(conn.0, PCWSTR(w.as_ptr())) }.map_err(|e| failed("FTP silme", e));
    }
    // emptied first (the listing is read whole before anything is deleted: one search at a time on a connection)
    if depth > 64 {
        return Err(std::io::Error::other("FTP silme: klasörler çok derin"));
    }
    for e in list_on(conn, path)? {
        delete_on(conn, &join(path, &e.name), e.is_dir(), depth + 1)?;
    }
    unsafe { FtpRemoveDirectoryW(conn.0, PCWSTR(w.as_ptr())) }.map_err(|e| failed("FTP silme", e))
}

/// Renames `url` to `new_name` in the same directory.
pub fn rename(url: &str, new_name: &str) -> std::io::Result<()> {
    let u = parsed(url)?;
    let (_s, conn) = connect(&u)?;
    let parent = u.path.rsplit_once('/').map_or("/", |(p, _)| if p.is_empty() { "/" } else { p }).to_string();
    let (from, to) = (wide(&u.path), wide(join(&parent, new_name)));
    unsafe { FtpRenameFileW(conn.0, PCWSTR(from.as_ptr()), PCWSTR(to.as_ptr())) }.map_err(|e| failed("FTP yeniden adlandırma", e))
}

/// Makes the directory `name` inside `dir_url`.
pub fn make_dir(dir_url: &str, name: &str) -> std::io::Result<()> {
    let u = parsed(dir_url)?;
    let (_s, conn) = connect(&u)?;
    let w = wide(join(&u.path, name));
    unsafe { FtpCreateDirectoryW(conn.0, PCWSTR(w.as_ptr())) }.map_err(|e| failed("FTP klasör oluşturma", e))
}

/// Downloads `url` into the local folder `dest_dir` under its own name: a file, or a directory with all it holds. Returns what was made.
pub fn download_into(url: &str, is_dir: bool, dest_dir: &Path) -> std::io::Result<std::path::PathBuf> {
    let u = parsed(url)?;
    let name = u.path.rsplit('/').next().filter(|n| !n.is_empty()).unwrap_or("ftp").to_string();
    let (_s, conn) = connect(&u)?;
    let local = dest_dir.join(&name);
    fetch(&conn, &u.path, is_dir, &local, 0)?;
    Ok(local)
}

fn fetch(conn: &Handle, path: &str, is_dir: bool, local: &Path, depth: u32) -> std::io::Result<()> {
    if !is_dir {
        let (remote, l) = (wide(path), wide(local));
        return unsafe { FtpGetFileW(conn.0, PCWSTR(remote.as_ptr()), PCWSTR(l.as_ptr()), false, 0, FTP_TRANSFER_TYPE_BINARY.0 | INTERNET_FLAG_RELOAD, None) }
            .map_err(|e| failed("FTP indirme", e));
    }
    if depth > 64 {
        return Err(std::io::Error::other("FTP indirme: klasörler çok derin"));
    }
    std::fs::create_dir_all(local)?;
    for e in list_on(conn, path)? {
        fetch(conn, &join(path, &e.name), e.is_dir(), &local.join(&*e.name), depth + 1)?;
    }
    Ok(())
}

/// Uploads the local file or folder `local` into the remote directory `dir_url` (a folder with all it holds; one that is already
/// there is written into).
pub fn upload_tree(local: &Path, dir_url: &str) -> std::io::Result<()> {
    if !local.is_dir() {
        return upload(local, dir_url);
    }
    let name = local.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "dosya adı yok"))?.to_string_lossy().into_owned();
    if let Err(e) = make_dir(dir_url, &name) {
        // (already there: written into; anything else shows when its files fail)
        log::debug!("ftp mkdir {name}: {e}");
    }
    let inner = child_url(dir_url, &name);
    for e in std::fs::read_dir(local)? {
        upload_tree(&e?.path(), &inner)?;
    }
    Ok(())
}

fn list_on(conn: &Handle, path: &str) -> std::io::Result<Vec<Entry>> {
    let pattern = wide(if path == "/" { "/*".to_string() } else { format!("{path}/*") });
    let mut data = WIN32_FIND_DATAW::default();
    let find = Handle(unsafe {
        FtpFindFirstFileW(conn.0, PCWSTR(pattern.as_ptr()), Some(&mut data), INTERNET_FLAG_RELOAD | INTERNET_FLAG_NO_CACHE_WRITE, None)
    });
    let mut out = Vec::new();
    if find.0.is_null() {
        let e = std::io::Error::last_os_error();
        // ERROR_NO_MORE_FILES (18): empty directory
        if e.raw_os_error() == Some(18) {
            return Ok(out);
        }
        return Err(std::io::Error::new(e.kind(), format!("FTP listeleme: {}", describe(e))));
    }
    loop {
        if let Some(e) = crate::listing::to_entry(&data) {
            out.push(e);
        }
        if unsafe { InternetFindNextFileW(find.0, &mut data as *mut _ as *mut _) }.is_err() {
            break;
        }
    }
    Ok(out)
}

/// Downloads `url` (a file) to `dest`.
pub fn download(url: &str, dest: &Path) -> std::io::Result<()> {
    let u = FtpUrl::parse(url).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "geçersiz FTP adresi"))?;
    let (_s, conn) = connect(&u)?;
    let remote = wide(&u.path);
    let local = wide(dest);
    unsafe {
        FtpGetFileW(
            conn.0,
            PCWSTR(remote.as_ptr()),
            PCWSTR(local.as_ptr()),
            false,
            0,
            FTP_TRANSFER_TYPE_BINARY.0 | INTERNET_FLAG_RELOAD,
            None,
        )
    }
    .map_err(|e| std::io::Error::other(format!("FTP indirme: {}", describe(std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF)))))
}

/// Uploads a local file into the remote directory `dir_url`.
pub fn upload(local: &Path, dir_url: &str) -> std::io::Result<()> {
    let u = FtpUrl::parse(dir_url).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "geçersiz FTP adresi"))?;
    let name = local.file_name().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "dosya adı yok"))?;
    let (_s, conn) = connect(&u)?;
    let remote_path = if u.path == "/" { format!("/{}", name.to_string_lossy()) } else { format!("{}/{}", u.path, name.to_string_lossy()) };
    let remote = wide(&remote_path);
    let l = wide(local);
    unsafe { FtpPutFileW(conn.0, PCWSTR(l.as_ptr()), PCWSTR(remote.as_ptr()), FTP_TRANSFER_TYPE_BINARY, None) }
        .map_err(|e| std::io::Error::other(format!("FTP yükleme: {}", describe(std::io::Error::from_raw_os_error(e.code().0 & 0xFFFF)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_child_address_has_slashes_whatever_the_folder_had() {
        assert_eq!(child_url(r"ftp://h:21\DCIM", "a b.jpg"), "ftp://h:21/DCIM/a b.jpg");
        assert_eq!(child_url("ftp://h/", "x"), "ftp://h/x");
        assert_eq!(join("/", "x"), "/x");
        assert_eq!(join("/a", "x"), "/a/x");
    }

    #[test]
    fn parse_urls() {
        let u = FtpUrl::parse("ftp://user:pw@example.com:2121/pub/dir/").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.user.as_deref(), u.pass.as_deref(), u.path.as_str()), ("example.com", 2121, Some("user"), Some("pw"), "/pub/dir"));
        let u = FtpUrl::parse(r"ftp://example.com\pub\x").unwrap();
        assert_eq!((u.port, u.path.as_str(), u.user), (21, "/pub/x", None));
        assert_eq!(FtpUrl::parse("ftp://h").unwrap().path, "/");
        assert!(FtpUrl::parse("http://h").is_none());
        assert!(is_ftp(Path::new("FTP://h/x")));
        assert!(!is_ftp(Path::new(r"C:\Öğrenci")));
        assert!(!is_ftp(Path::new(r"C:\aaç")));
    }

    /// Needs a local server: `python -m pyftpdlib -p 2121 -d <dir>` and RF_FTP_TEST=ftp://127.0.0.1:2121
    #[test]
    fn live_server() {
        let Ok(base) = std::env::var("RF_FTP_TEST") else { return };
        let entries = list(&base).expect("list");
        eprintln!("{:?}", entries.iter().map(|e| e.name.to_string()).collect::<Vec<_>>());
        assert!(!entries.is_empty());
        let file = entries.iter().find(|e| !e.is_dir()).expect("a file");
        let dest = std::env::temp_dir().join(format!("rf-ftp-{}", file.name));
        download(&format!("{base}/{}", file.name), &dest).expect("download");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), file.size);
        let _ = std::fs::remove_file(dest);
    }
}
