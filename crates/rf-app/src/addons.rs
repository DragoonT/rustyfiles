//! Add-ons: separate programs rustyfiles can install on the user's click and talk to (the first: HyperDownload, the download
//! manager, `gelenmedya/hyperdownload`). An add-on is never code loaded into rustyfiles: it is a program of its own in its own
//! folder, so its licence and its crashes stay apart.
//!
//! WHERE THEY COME FROM. The list is `addons.json` in rustyfiles' own repository: each add-on's version and, for each processor,
//! the URL of its zip (a GitHub release of the add-on's repository) and that zip's SHA-256. A zip whose SHA-256 is not the listed one
//! is not installed. Nothing is downloaded until the user clicks Install or Update.
//!
//! WHERE THEY GO. `<local data>\addons\<id>\<version>\` (local data: `%LOCALAPPDATA%\rustyfiles`, or the portable copy's own
//! `data` folder), with `<local data>\addons\<id>\installed.json` naming the version that is in use. An update unpacks the new
//! version beside the old one and then switches `installed.json`, so a failed update leaves the old one working.
//!
//! `RUSTYFILES_ADDONS_MANIFEST` names another list (a URL, or a file path) for testing.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MANIFEST_URL: &str = "https://raw.githubusercontent.com/gelenmedya/rustyfiles/main/addons.json";

#[derive(Clone, Debug, Deserialize)]
pub struct Manifest {
    pub addons: Vec<AddonInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub license: String,
    /// The source code (shown as a link).
    pub source: String,
    /// One per processor: "x64", "arm64".
    #[serde(default)]
    pub files: std::collections::HashMap<String, FileRef>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FileRef {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: u64,
}

/// What `installed.json` says.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Installed {
    pub version: String,
    /// The program to start, relative to the version's folder.
    pub exe: String,
}

/// This machine's processor as the list names it.
pub fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" }
}

impl AddonInfo {
    /// The zip for this machine, if the add-on has one.
    pub fn file(&self) -> Option<&FileRef> {
        self.files.get(arch())
    }
}

/// `<local data>\addons`.
pub fn root() -> Option<PathBuf> {
    crate::settings::local_data_dir().map(|d| d.join("addons"))
}

fn dir_of(id: &str) -> Option<PathBuf> {
    // (an id from the list becomes a folder name: only plain names)
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')).then(|| root().map(|r| r.join(id))).flatten()
}

/// The version of `id` that is installed, if one is.
pub fn installed(id: &str) -> Option<Installed> {
    let d = dir_of(id)?;
    let i: Installed = serde_json::from_slice(&std::fs::read(d.join("installed.json")).ok()?).ok()?;
    d.join(&i.version).join(&i.exe).is_file().then_some(i)
}

/// The installed program of `id`.
pub fn program(id: &str) -> Option<PathBuf> {
    let i = installed(id)?;
    Some(dir_of(id)?.join(&i.version).join(&i.exe))
}

/// Whether `a` is a newer version than `b` (dotted numbers; a part that is not a number counts as 0).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(['.', '-']).take(3).map(|p| p.parse().unwrap_or(0)).collect() };
    parts(a) > parts(b)
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(15)))
        .timeout_recv_body(Some(std::time::Duration::from_secs(60)))
        .http_status_as_error(false)
        .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
        .build()
        .into()
}

/// Reads the list (blocking: call it off the UI thread).
pub fn fetch_manifest() -> Result<Manifest, String> {
    let from = std::env::var("RUSTYFILES_ADDONS_MANIFEST").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| MANIFEST_URL.to_string());
    let bytes = if from.starts_with("http://") || from.starts_with("https://") {
        let resp = agent().get(&from).call().map_err(|e| e.to_string())?;
        if resp.status().as_u16() != 200 {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        let mut v = Vec::new();
        resp.into_body().into_reader().take(1 << 20).read_to_end(&mut v).map_err(|e| e.to_string())?;
        v
    } else {
        std::fs::read(&from).map_err(|e| format!("{from}: {e}"))?
    };
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

/// The bytes of `url` (a URL, or for testing a file path) into `to`, with the bytes so far told to `progress`; the SHA-256 of what
/// was written.
fn download(url: &str, to: &Path, progress: &dyn Fn(u64), cancelled: &dyn Fn() -> bool) -> Result<String, String> {
    let mut reader: Box<dyn Read> = if url.starts_with("http://") || url.starts_with("https://") {
        let resp = agent().get(url).call().map_err(|e| e.to_string())?;
        if resp.status().as_u16() != 200 {
            return Err(format!("HTTP {}", resp.status().as_u16()));
        }
        Box::new(resp.into_body().into_reader())
    } else {
        Box::new(std::fs::File::open(url).map_err(|e| format!("{url}: {e}"))?)
    };
    let mut out = std::fs::File::create(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut done = 0u64;
    loop {
        if cancelled() {
            return Err("cancelled".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
        hash.update(&buf[..n]);
        done += n as u64;
        progress(done);
    }
    Ok(hash.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Installs (or updates to) `info`'s version for this machine: download, SHA-256 check, unpack beside any older version, switch
/// `installed.json`, remove the older versions. Blocking.
pub fn install(info: &AddonInfo, progress: &dyn Fn(u64, u64), cancelled: &dyn Fn() -> bool) -> Result<Installed, String> {
    let file = info.file().ok_or_else(|| format!("no {} build of {}", arch(), info.name))?;
    let dir = dir_of(&info.id).ok_or("no folder for add-ons")?;
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let zip = dir.join(".download.zip");
    let total = file.size;
    let got = download(&file.url, &zip, &|n| progress(n, total), cancelled);
    let sha = match got {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&zip);
            return Err(e);
        }
    };
    if !sha.eq_ignore_ascii_case(file.sha256.trim()) {
        let _ = std::fs::remove_file(&zip);
        return Err("the download is not the file the list names (its SHA-256 differs): it was not installed".into());
    }
    let staging = dir.join(format!(".{}.tmp", info.version));
    let _ = std::fs::remove_dir_all(&staging);
    let unpacked = rf_archive::extract(&zip, &staging, &mut |_| {}, cancelled);
    let _ = std::fs::remove_file(&zip);
    if let Err(e) = unpacked {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e.to_string());
    }
    let exe = format!("{}.exe", info.id);
    if !staging.join(&exe).is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("{exe} is not in the zip"));
    }
    let target = dir.join(&info.version);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| e.to_string())?;
    let now = Installed { version: info.version.clone(), exe };
    std::fs::write(dir.join("installed.json"), serde_json::to_vec_pretty(&now).unwrap_or_default()).map_err(|e| e.to_string())?;
    // the older versions (one still running keeps its files until it ends: what cannot be removed now is left)
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() && n != info.version && !n.starts_with('.') {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    Ok(now)
}

/// Removes `id` (its folder; its own data elsewhere stays).
pub fn remove(id: &str) -> Result<(), String> {
    let dir = dir_of(id).ok_or("no folder for add-ons")?;
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(newer("0.10.0", "0.9.9"));
        assert!(newer("1.0.0", "0.99.0"));
        assert!(!newer("0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
        assert!(newer("v0.2.0", "0.1.9"));
    }

    #[test]
    fn a_list_is_read_and_the_machines_file_is_picked() {
        let m: Manifest = serde_json::from_str(
            r#"{"addons":[{"id":"hyperdownload","name":"HyperDownload","version":"0.1.0","license":"MIT","source":"https://github.com/gelenmedya/hyperdownload",
               "files":{"x64":{"url":"https://x/a-x64.zip","sha256":"ab","size":10},"arm64":{"url":"https://x/a-arm64.zip","sha256":"cd"}}}]}"#,
        )
        .unwrap();
        let a = &m.addons[0];
        assert_eq!(a.file().map(|f| f.url.ends_with(&format!("-{}.zip", arch()))), Some(true));
        // a list from a later version with fields this one does not know is still read
        let later: Manifest = serde_json::from_str(r#"{"addons":[{"id":"x","name":"X","version":"1","license":"MIT","source":"s","new":1}],"more":true}"#).unwrap();
        assert!(later.addons[0].file().is_none());
    }

    #[test]
    fn an_install_checks_the_sha256_and_switches_only_when_it_is_right() {
        let tmp = std::env::temp_dir().join(format!("rf-addons-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        // SAFETY of the env change: this test is the only one that reads RUSTYFILES_CONFIG_DIR in this module's tests
        unsafe { std::env::set_var("RUSTYFILES_CONFIG_DIR", tmp.join("cfg")) };
        // a zip with the program in it
        let src = tmp.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("demo.exe"), b"MZ demo").unwrap();
        let zip = tmp.join("demo.zip");
        rf_archive::compress(&[src.join("demo.exe")], &zip, rf_archive::Format::Zip, 5, &mut |_| {}, &|| false).unwrap();
        let sha: String = Sha256::digest(std::fs::read(&zip).unwrap()).iter().map(|b| format!("{b:02x}")).collect();
        let mut info = AddonInfo { id: "demo".into(), name: "Demo".into(), version: "0.1.0".into(), license: "MIT".into(), source: String::new(), files: Default::default() };
        info.files.insert(arch().into(), FileRef { url: zip.display().to_string(), sha256: "00".into(), size: 0 });
        let err = install(&info, &|_, _| {}, &|| false).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(installed("demo").is_none(), "nothing installed from a wrong file");
        info.files.get_mut(arch()).unwrap().sha256 = sha.to_uppercase();
        let got = install(&info, &|_, _| {}, &|| false).unwrap();
        assert_eq!(installed("demo"), Some(got));
        assert!(program("demo").unwrap().ends_with("0.1.0/demo.exe") || program("demo").unwrap().ends_with(r"0.1.0\demo.exe"));
        // an update replaces it and the old version's folder goes
        info.version = "0.2.0".into();
        install(&info, &|_, _| {}, &|| false).unwrap();
        assert_eq!(installed("demo").unwrap().version, "0.2.0");
        assert!(!root().unwrap().join("demo").join("0.1.0").exists());
        remove("demo").unwrap();
        assert!(installed("demo").is_none());
        unsafe { std::env::remove_var("RUSTYFILES_CONFIG_DIR") };
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
