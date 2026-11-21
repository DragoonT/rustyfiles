//! A file being downloaded by HyperDownload, shown by DirectLook from the add-on's local stream. It travels as a "path" of its own
//! (DirectLook's messages carry paths): `hdstream:\<the stream's URL in hex>\<the file's name>`. The name gives the window its title
//! and the viewer the file's kind; the URL is where the bytes come from.

use std::path::{Path, PathBuf};

const PREFIX: &str = "hdstream:\\";

/// The path DirectLook is given for the stream at `url` of the file `name`.
pub fn path(url: &str, name: &str) -> PathBuf {
    let hex: String = url.bytes().map(|b| format!("{b:02x}")).collect();
    // (a name with a path separator in it keeps only its last part)
    let name = name.rsplit(['\\', '/']).next().unwrap_or(name);
    PathBuf::from(format!("{PREFIX}{hex}\\{name}"))
}

/// The stream's URL and the file's name, when `path` is one.
pub fn parse(path: &Path) -> Option<(String, String)> {
    let s = path.to_str()?.strip_prefix(PREFIX)?;
    let (hex, name) = s.split_once('\\')?;
    if hex.len() % 2 != 0 {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok()).collect();
    let url = String::from_utf8(bytes?).ok()?;
    // only the add-on's own stream on this machine
    url.starts_with("http://127.0.0.1:").then(|| (url, name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stream_travels_as_a_path_and_comes_back() {
        let url = "http://127.0.0.1:5123/api/torrents/3/stream/0?token=abc";
        let p = path(url, "Film (2026)/Film.mkv");
        assert_eq!(p.file_name().unwrap().to_string_lossy(), "Film.mkv");
        assert_eq!(parse(&p), Some((url.to_string(), "Film.mkv".to_string())));
        // a plain path is not one, nor a stream of another host
        assert_eq!(parse(Path::new(r"C:\Videos\a.mkv")), None);
        assert_eq!(parse(&path("http://example.com/a.mkv", "a.mkv")), None);
    }
}
