//! Lossless URL editing for Windows Internet shortcut files.

use std::path::Path;

#[derive(Clone)]
pub struct InternetShortcut {
    original: Vec<u8>,
    pub url: String,
}

#[derive(Clone, Copy)]
enum Encoding {
    Utf8(bool),
    Utf16(bool),
    Ansi(&'static encoding_rs::Encoding),
}

fn decode(data: &[u8]) -> Result<(String, Encoding), String> {
    if data.starts_with(&[0xff, 0xfe]) || data.starts_with(&[0xfe, 0xff]) {
        let le = data[0] == 0xff;
        let bytes = &data[2..];
        if bytes.len() % 2 != 0 {
            return Err("Invalid UTF-16 shortcut".into());
        }
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|b| {
                if le {
                    u16::from_le_bytes([b[0], b[1]])
                } else {
                    u16::from_be_bytes([b[0], b[1]])
                }
            })
            .collect();
        return String::from_utf16(&units)
            .map(|s| (s, Encoding::Utf16(le)))
            .map_err(|_| "Invalid UTF-16 shortcut".into());
    }
    if let Some(bytes) = data.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return std::str::from_utf8(bytes)
            .map(|s| (s.to_owned(), Encoding::Utf8(true)))
            .map_err(|_| "Invalid UTF-8 shortcut".into());
    }
    if !data.is_ascii() {
        if let Ok(s) = std::str::from_utf8(data) {
            return Ok((s.to_owned(), Encoding::Utf8(false)));
        }
    }
    let cp = unsafe { windows::Win32::Globalization::GetACP() };
    let label = match cp {
        932 => "shift_jis".to_string(),
        936 => "gbk".into(),
        949 => "euc-kr".into(),
        950 => "big5".into(),
        65001 => "utf-8".into(),
        _ => format!("windows-{cp}"),
    };
    let enc = encoding_rs::Encoding::for_label(label.as_bytes())
        .ok_or_else(|| format!("Unsupported Windows code page {cp}"))?;
    let (s, _, invalid) = enc.decode(data);
    if invalid {
        return Err("Invalid shortcut encoding".into());
    }
    let (roundtrip, _, invalid) = enc.encode(&s);
    if invalid || roundtrip.as_ref() != data {
        return Err("Shortcut encoding cannot be preserved".into());
    }
    Ok((s.into_owned(), Encoding::Ansi(enc)))
}

fn encode(s: &str, enc: Encoding) -> Result<Vec<u8>, String> {
    match enc {
        Encoding::Utf8(bom) => {
            let mut v = if bom { vec![0xef, 0xbb, 0xbf] } else { vec![] };
            v.extend_from_slice(s.as_bytes());
            Ok(v)
        }
        Encoding::Utf16(le) => {
            let mut v = if le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for c in s.encode_utf16() {
                v.extend_from_slice(&if le { c.to_le_bytes() } else { c.to_be_bytes() });
            }
            Ok(v)
        }
        Encoding::Ansi(enc) => {
            let (bytes, _, invalid) = enc.encode(s);
            if invalid {
                Err("URL cannot be represented in the shortcut's encoding".into())
            } else {
                Ok(bytes.into_owned())
            }
        }
    }
}

fn url_range(s: &str) -> Result<std::ops::Range<usize>, String> {
    let mut section = false;
    let mut found = None;
    let mut offset = 0;
    for line in s.split_inclusive('\n') {
        let text = line.trim_end_matches(['\r', '\n']);
        let trimmed = text.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            section = trimmed[1..trimmed.len() - 1].eq_ignore_ascii_case("InternetShortcut");
        } else if section {
            if let Some(eq) = text.find('=') {
                if text[..eq].trim().eq_ignore_ascii_case("URL") {
                    if found.is_some() {
                        return Err("Shortcut contains multiple URL entries".into());
                    }
                    found = Some(offset + eq + 1..offset + text.len());
                }
            }
        }
        offset += line.len();
    }
    found.ok_or_else(|| "Internet shortcut has no URL entry".into())
}

fn replace_url(data: &[u8], new_url: &str) -> Result<Vec<u8>, String> {
    if new_url.contains(['\r', '\n', '\0']) {
        return Err("URL must be a single line".into());
    }
    let (mut s, enc) = decode(data)?;
    let range = url_range(&s)?;
    s.replace_range(range, new_url);
    encode(&s, enc)
}

pub fn read(path: &Path) -> Result<InternetShortcut, String> {
    let original = std::fs::read(path).map_err(|e| e.to_string())?;
    if original.len() > 1024 * 1024 {
        return Err("Internet shortcut is too large".into());
    }
    let (s, _) = decode(&original)?;
    let url = s[url_range(&s)?].to_owned();
    Ok(InternetShortcut { original, url })
}

pub fn save(
    path: &Path,
    shortcut: &InternetShortcut,
    url: &str,
) -> Result<InternetShortcut, String> {
    let bytes = replace_url(&shortcut.original, url)?;
    if std::fs::read(path).map_err(|e| e.to_string())? != shortcut.original {
        return Err("Internet shortcut changed on disk; reopen Properties".into());
    }
    // The shell caches Internet shortcuts independently of file contents. Saving through
    // IUniformResourceLocator tells Windows to invalidate that cache; restore the exact edited
    // text afterwards because the shell may add or rewrite unrelated INI entries.
    crate::shell::ensure_com();
    unsafe {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READWRITE,
        };
        use windows::Win32::UI::Shell::{CLSID_InternetShortcut, IUniformResourceLocatorW};
        use windows::core::{Interface, PCWSTR};
        let locator: IUniformResourceLocatorW =
            CoCreateInstance(&CLSID_InternetShortcut, None, CLSCTX_INPROC_SERVER)
                .map_err(|e| e.to_string())?;
        let file: IPersistFile = locator.cast().map_err(|e| e.to_string())?;
        let name = crate::wide::wide(path);
        let value = crate::wide::wide(url);
        file.Load(PCWSTR(name.as_ptr()), STGM_READWRITE)
            .map_err(|e| e.to_string())?;
        locator
            .SetURL(PCWSTR(value.as_ptr()), 0)
            .map_err(|e| e.to_string())?;
        file.Save(PCWSTR(name.as_ptr()), true)
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(path, &bytes).map_err(|e| {
        format!("Windows may already have saved the URL, but restoring the shortcut's other data failed: {e}. Reopen Properties and check the file.")
    })?;
    Ok(InternetShortcut {
        original: bytes,
        url: url.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changes_only_the_internet_shortcut_url() {
        let source = b"[Other]\r\nURL=leave\r\n[InternetShortcut]\r\nIconFile=abc.ico\r\nURL=https://old.test\r\nIDList=123\r\n";
        let expected = b"[Other]\r\nURL=leave\r\n[InternetShortcut]\r\nIconFile=abc.ico\r\nURL=https://new.test\r\nIDList=123\r\n";
        assert_eq!(replace_url(source, "https://new.test").unwrap(), expected);
    }
    #[test]
    fn rejects_newline_injection() {
        assert!(
            replace_url(
                b"[InternetShortcut]\nURL=https://old\n",
                "https://new\nIconFile=bad"
            )
            .is_err()
        );
    }
    #[test]
    fn preserves_utf16_and_other_fields() {
        for little in [true, false] {
            let input = "[InternetShortcut]\r\nURL=https://old\r\nIconFile=іконка.ico\r\n";
            let output = "[InternetShortcut]\r\nURL=https://новий\r\nIconFile=іконка.ico\r\n";
            let source = encode(input, Encoding::Utf16(little)).unwrap();
            let expected = encode(output, Encoding::Utf16(little)).unwrap();
            assert_eq!(replace_url(&source, "https://новий").unwrap(), expected);
        }
    }
    #[test]
    fn refuses_duplicate_or_missing_url() {
        assert!(replace_url(b"[InternetShortcut]\nURL=one\nURL=two\n", "new").is_err());
        assert!(replace_url(b"[Other]\nURL=one\n", "new").is_err());
    }
    #[test]
    fn edits_a_real_file_and_refuses_external_changes() {
        let dir = std::env::temp_dir().join(format!(
            "rustyfiles-url-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("shortcut.url");
        std::fs::write(
            &file,
            b"[InternetShortcut]\r\nURL=https://old.test\r\nIconIndex=4\r\n",
        )
        .unwrap();
        let shortcut = read(&file).unwrap();
        let saved = save(&file, &shortcut, "https://new.test").unwrap();
        assert_eq!(read(&file).unwrap().url, "https://new.test");
        assert_eq!(
            std::fs::read(&file).unwrap(),
            b"[InternetShortcut]\r\nURL=https://new.test\r\nIconIndex=4\r\n"
        );
        std::fs::write(&file, b"changed externally").unwrap();
        assert!(save(&file, &saved, "https://third.test").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"changed externally");
        std::fs::remove_dir_all(dir).unwrap();
    }
    fn shell_url(path: &Path) -> String {
        use windows::Win32::System::Com::{
            CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ,
        };
        use windows::Win32::UI::Shell::{CLSID_InternetShortcut, IUniformResourceLocatorW};
        use windows::core::{Interface, PCWSTR};
        crate::shell::ensure_com();
        unsafe {
            let locator: IUniformResourceLocatorW =
                CoCreateInstance(&CLSID_InternetShortcut, None, CLSCTX_INPROC_SERVER).unwrap();
            let file: IPersistFile = locator.cast().unwrap();
            let name = crate::wide::wide(path);
            file.Load(PCWSTR(name.as_ptr()), STGM_READ).unwrap();
            let ptr = locator.GetURL().unwrap();
            let url = ptr.to_string().unwrap();
            CoTaskMemFree(Some(ptr.0.cast()));
            url
        }
    }
    #[test]
    fn windows_reloads_new_url_after_save() {
        let dir = std::env::temp_dir().join(format!(
            "rustyfiles-url-shell-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shortcut.url");
        let original = b"[InternetShortcut]\r\nURL=https://example.com/before\r\nIconFile=custom.ico\r\nCustomKey=keep\r\n";
        std::fs::write(&path, original).unwrap();
        assert_eq!(shell_url(&path), "https://example.com/before");
        let shortcut = read(&path).unwrap();
        let saved = save(&path, &shortcut, "https://example.com/after").unwrap();
        assert_eq!(shell_url(&path), "https://example.com/after");
        assert_eq!(std::fs::read(&path).unwrap(), b"[InternetShortcut]\r\nURL=https://example.com/after\r\nIconFile=custom.ico\r\nCustomKey=keep\r\n");
        save(&path, &saved, "https://example.com/again").unwrap();
        assert_eq!(shell_url(&path), "https://example.com/again");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn shell_cache_reader_child() {
        let Some(dir) = std::env::var_os("RUSTYFILES_URL_TEST_READER") else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        let path = dir.join("cross-process.url");
        assert_eq!(shell_url(&path), "https://example.com/before");
        std::fs::write(dir.join("ready"), b"ready").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !dir.join("go").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(dir.join("go").exists(), "writer did not finish");
        assert_eq!(shell_url(&path), "https://example.com/after");
    }

    #[test]
    fn saving_refreshes_windows_reader_in_another_process() {
        let dir = std::env::temp_dir().join(format!(
            "rustyfiles-url-cross-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("cross-process.url");
        let old =
            b"[InternetShortcut]\r\nURL=https://example.com/before\r\nIconFile=custom.ico\r\n";
        std::fs::write(&path, old).unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "internet_shortcut::tests::shell_cache_reader_child",
                "--nocapture",
            ])
            .env("RUSTYFILES_URL_TEST_READER", &dir)
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !dir.join("ready").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(dir.join("ready").exists(), "reader did not start");
        let shortcut = read(&path).unwrap();
        save(&path, &shortcut, "https://example.com/after").unwrap();
        std::fs::write(dir.join("go"), b"go").unwrap();
        assert!(child.wait().unwrap().success());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"[InternetShortcut]\r\nURL=https://example.com/after\r\nIconFile=custom.ico\r\n"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn saving_utf16_preserves_original_encoding_and_metadata() {
        let dir = std::env::temp_dir().join(format!(
            "rustyfiles-url-utf16-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shortcut.url");
        let original = encode(
            "[InternetShortcut]\r\nURL=https://example.com/before\r\nIconFile=іконка.ico\r\n",
            Encoding::Utf16(true),
        )
        .unwrap();
        let expected = encode(
            "[InternetShortcut]\r\nURL=https://example.com/after\r\nIconFile=іконка.ico\r\n",
            Encoding::Utf16(true),
        )
        .unwrap();
        std::fs::write(&path, &original).unwrap();
        assert_eq!(shell_url(&path), "https://example.com/before");
        let shortcut = read(&path).unwrap();
        save(&path, &shortcut, "https://example.com/after").unwrap();
        assert_eq!(shell_url(&path), "https://example.com/after");
        assert_eq!(std::fs::read(&path).unwrap(), expected);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
