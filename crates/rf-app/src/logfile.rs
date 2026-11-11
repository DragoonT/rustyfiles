//! The log file: what the app logs (warnings and errors unless `RUST_LOG` says otherwise) goes
//! to `rustyfiles.log` in the settings folder as well as to stderr, which a release build has
//! none of. The file is kept small: at 1 MB it becomes `rustyfiles.log.old` (replacing the
//! previous one) and a new file starts.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

const MAX_BYTES: u64 = 1 << 20;
const NAME: &str = "rustyfiles.log";

/// Where the log is written (None without a settings folder).
pub fn path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join(NAME))
}

/// Writes to stderr and to the log file. Lines are appended whole, so several rustyfiles
/// processes can share the file.
pub struct Tee {
    file: Option<File>,
    path: Option<PathBuf>,
    written: u64,
    /// Also to stderr (off in the test, which writes a megabyte).
    echo: bool,
}

impl Tee {
    pub fn new() -> Tee {
        let path = path();
        let mut t = Tee { file: None, path, written: 0, echo: true };
        t.open();
        t
    }

    fn open(&mut self) {
        let Some(path) = &self.path else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // a full file is put aside first (also one left full by another process)
        if std::fs::metadata(path).is_ok_and(|m| m.len() >= MAX_BYTES) {
            let old = path.with_extension("log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(path, &old);
        }
        self.file = OpenOptions::new().create(true).append(true).open(path).ok();
        self.written = self.file.as_ref().and_then(|f| f.metadata().ok()).map_or(0, |m| m.len());
    }
}

impl Default for Tee {
    fn default() -> Self {
        Self::new()
    }
}

impl Write for Tee {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.echo {
            let _ = std::io::stderr().write_all(buf);
        }
        if let Some(f) = &mut self.file {
            if f.write_all(buf).is_ok() {
                self.written += buf.len() as u64;
            }
        }
        if self.written >= MAX_BYTES && buf.ends_with(b"\n") {
            self.file = None;
            self.open();
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stderr().flush();
        if let Some(f) = &mut self.file {
            let _ = f.flush();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full log is put aside as `.old` and a new one starts; lines are never lost in between.
    #[test]
    fn a_full_log_is_put_aside() {
        let dir = std::env::temp_dir().join(format!("rf-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(NAME);
        let mut t = Tee { file: None, path: Some(path.clone()), written: 0, echo: false };
        t.open();
        let line = format!("{}\n", "x".repeat(1023));
        for _ in 0..1024 {
            t.write_all(line.as_bytes()).unwrap();
        }
        // exactly 1 MB written: the file was put aside, the next line starts a new one
        t.write_all(b"after\n").unwrap();
        t.flush().unwrap();
        assert_eq!(std::fs::metadata(dir.join("rustyfiles.log.old")).unwrap().len(), MAX_BYTES);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "after\n");
        drop(t);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
