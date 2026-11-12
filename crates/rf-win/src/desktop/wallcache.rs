//! The on-disk cache of rendered wallpaper bitmaps: `%LOCALAPPDATA%\rustyfiles\wallcache\` (shell plan 4.5). A file is named by the
//! SHA-256 of the cache key's canonical text (`rf_core::desktop::wallpaper::CacheKey`: path, modified time, size, target size, mode,
//! colour, arrangement), so a changed picture, mode or monitor size simply finds no file, and stale files are pruned by age.
//!
//! FILE: `RFWC`, format version (u32), width, height, CRC-32 of the pixels, then width x height x 4 bytes of BGRA. A file that is
//! short, long, from another version or fails its CRC is a miss and is deleted: a half-written or damaged cache never paints a wrong
//! desktop. Writing goes to a temporary name and is renamed over the real one.
//!
//! The folder is the caller's choice (the app's local data folder, a temp folder in tests); this module never invents a path.

use std::io::Write;
use std::path::PathBuf;

use rf_core::desktop::wallpaper::CacheKey;
use sha2::{Digest, Sha256};

use crate::thumbs::Bgra;

const MAGIC: &[u8; 4] = b"RFWC";
const FORMAT: u32 = 1;
const HEADER: usize = 20;

#[derive(Debug, Clone)]
pub struct DiskCache {
    dir: PathBuf,
    max_entries: usize,
}

impl DiskCache {
    /// `max_entries`: how many bitmaps are kept (a 2560 x 1440 one is 14 MB); the least recently used are deleted by [`prune`](Self::prune).
    pub fn new(dir: PathBuf, max_entries: usize) -> Self {
        DiskCache { dir, max_entries: max_entries.max(1) }
    }

    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }

    pub fn file_for(&self, key: &CacheKey) -> PathBuf {
        let h = Sha256::digest(key.canonical().as_bytes());
        let name: String = h.iter().take(16).map(|b| format!("{b:02x}")).collect();
        self.dir.join(format!("{name}.rfwall"))
    }

    /// The bitmap for `key`, or `None` (also for a damaged file, which is deleted). A hit refreshes the file's time (for pruning).
    pub fn get(&self, key: &CacheKey) -> Option<Bgra> {
        let path = self.file_for(key);
        let bytes = std::fs::read(&path).ok()?;
        match parse(&bytes) {
            Some(b) => {
                if let Ok(f) = std::fs::OpenOptions::new().write(true).open(&path) {
                    let _ = f.set_modified(std::time::SystemTime::now());
                }
                Some(b)
            }
            None => {
                let _ = std::fs::remove_file(&path);
                None
            }
        }
    }

    pub fn put(&self, key: &CacheKey, b: &Bgra) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        let path = self.file_for(key);
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let r = (|| {
            let mut f = std::fs::File::create(&tmp)?;
            let mut head = Vec::with_capacity(HEADER);
            head.extend_from_slice(MAGIC);
            for v in [FORMAT, b.width, b.height, crc32fast::hash(&b.data)] {
                head.extend_from_slice(&v.to_le_bytes());
            }
            f.write_all(&head)?;
            f.write_all(&b.data)?;
            f.flush()?;
            drop(f);
            std::fs::rename(&tmp, &path)
        })();
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r?;
        self.prune();
        Ok(())
    }

    /// Keeps the `max_entries` most recently used bitmaps; removes the rest and any leftover temporary files.
    pub fn prune(&self) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return };
        let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            match p.extension().and_then(|x| x.to_str()) {
                Some("rfwall") => files.push((e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH), p)),
                Some(x) if x.starts_with("tmp") => {
                    // a temporary file of a writer that died; one being written right now is younger than a minute
                    let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|d| d.as_secs() > 60);
                    if old {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                _ => {}
            }
        }
        files.sort_by(|a, b| b.cmp(a)); // newest first
        for (_, p) in files.into_iter().skip(self.max_entries) {
            let _ = std::fs::remove_file(p);
        }
    }
}

fn parse(bytes: &[u8]) -> Option<Bgra> {
    if bytes.len() < HEADER || &bytes[..4] != MAGIC {
        return None;
    }
    let u = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let (format, w, h, crc) = (u(4), u(8), u(12), u(16));
    let len = (w as usize).checked_mul(h as usize)?.checked_mul(4)?;
    if format != FORMAT || w == 0 || h == 0 || bytes.len() != HEADER + len {
        return None;
    }
    let data = bytes[HEADER..].to_vec();
    (crc32fast::hash(&data) == crc).then_some(Bgra { width: w, height: h, data })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::geom::{Rect, Rgb};
    use rf_core::desktop::placement::{Position, TileAnchor};
    use rf_core::desktop::wallpaper::FileStamp;
    use std::path::Path;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-wallcache-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }
    fn key(path: &str, mtime: i64, size: u64, mode: Position) -> CacheKey {
        let m = Rect::new(0, 0, 4, 2);
        CacheKey::new(Path::new(path), FileStamp { modified_ms: mtime, size }, m, m, mode, Rgb(1, 2, 3), TileAnchor::Monitor)
    }
    fn bmp(seed: u8) -> Bgra {
        Bgra { width: 4, height: 2, data: (0..32).map(|i| (i as u8).wrapping_mul(seed)).collect() }
    }

    #[test]
    fn a_stored_bitmap_comes_back_identical() {
        let dir = temp("rt");
        let c = DiskCache::new(dir.clone(), 4);
        let k = key(r"D:\a.png", 1, 2, Position::Fill);
        assert!(c.get(&k).is_none());
        c.put(&k, &bmp(7)).unwrap();
        let b = c.get(&k).unwrap();
        assert_eq!((b.width, b.height, b.data), (4, 2, bmp(7).data));
        // the file name comes from the key and nothing else is left behind
        assert_eq!(c.file_for(&k).extension().unwrap(), "rfwall");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_changes_the_key_misses() {
        let dir = temp("inval");
        let c = DiskCache::new(dir.clone(), 16);
        let k = key(r"D:\a.png", 1, 2, Position::Fill);
        c.put(&k, &bmp(1)).unwrap();
        for other in [key(r"D:\b.png", 1, 2, Position::Fill), key(r"D:\a.png", 9, 2, Position::Fill), key(r"D:\a.png", 1, 3, Position::Fill), key(r"D:\a.png", 1, 2, Position::Fit)] {
            assert!(c.get(&other).is_none());
            assert_ne!(c.file_for(&other), c.file_for(&k));
        }
        // the same key spelt differently is the same file
        assert_eq!(c.file_for(&key(r"d:/A.PNG", 1, 2, Position::Fill)), c.file_for(&k));
        assert!(c.get(&key(r"d:/A.PNG", 1, 2, Position::Fill)).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn damaged_files_are_misses_and_are_deleted() {
        let dir = temp("bad");
        let c = DiskCache::new(dir.clone(), 16);
        let k = key(r"D:\a.png", 1, 2, Position::Fill);
        c.put(&k, &bmp(3)).unwrap();
        let path = c.file_for(&k);
        let good = std::fs::read(&path).unwrap();
        let cases: Vec<(&str, Vec<u8>)> = vec![
            ("truncated", good[..good.len() - 1].to_vec()),
            ("too long", [good.clone(), vec![0]].concat()),
            ("one flipped pixel byte", {
                let mut v = good.clone();
                v[HEADER + 5] ^= 0xFF;
                v
            }),
            ("wrong magic", {
                let mut v = good.clone();
                v[0] = b'X';
                v
            }),
            ("other format version", {
                let mut v = good.clone();
                v[4] = 99;
                v
            }),
            ("empty", vec![]),
            ("zero size", {
                let mut v = good.clone();
                v[8..12].copy_from_slice(&0u32.to_le_bytes());
                v
            }),
            ("absurd size", {
                let mut v = good.clone();
                v[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
                v
            }),
        ];
        for (what, bytes) in cases {
            std::fs::write(&path, bytes).unwrap();
            assert!(c.get(&k).is_none(), "{what}");
            assert!(!path.exists(), "{what}: the damaged file is removed");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_the_most_recently_used_entries_are_kept() {
        let dir = temp("prune");
        let c = DiskCache::new(dir.clone(), 2);
        let keys: Vec<CacheKey> = (0..4).map(|i| key(&format!(r"D:\{i}.png"), 1, 1, Position::Fill)).collect();
        let base = std::time::SystemTime::now() - std::time::Duration::from_secs(1000);
        for (i, k) in keys.iter().enumerate() {
            c.put(k, &bmp(i as u8 + 1)).unwrap();
            // spread the times out so "newest" is unambiguous (file systems round)
            let f = std::fs::OpenOptions::new().write(true).open(c.file_for(k)).unwrap();
            f.set_modified(base + std::time::Duration::from_secs(i as u64 * 10)).unwrap();
        }
        // put() pruned as it went; make the order explicit and prune once more
        c.prune();
        let left: Vec<usize> = (0..4).filter(|i| c.file_for(&keys[*i]).exists()).collect();
        assert_eq!(left, [2, 3], "the two newest");
        // a hit makes an entry the newest
        assert!(c.get(&keys[2]).is_some());
        c.put(&keys[0], &bmp(9)).unwrap();
        let left: Vec<usize> = (0..4).filter(|i| c.file_for(&keys[*i]).exists()).collect();
        assert_eq!(left, [0, 2], "3 was the least recently used");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_leftover_temporary_file_is_swept_when_old() {
        let dir = temp("tmp");
        let c = DiskCache::new(dir.clone(), 2);
        let old = dir.join("x.tmp123");
        std::fs::write(&old, b"half").unwrap();
        std::fs::OpenOptions::new().write(true).open(&old).unwrap().set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(3600)).unwrap();
        let fresh = dir.join("y.tmp456");
        std::fs::write(&fresh, b"being written").unwrap();
        c.prune();
        assert!(!old.exists() && fresh.exists());
        // an unrelated file is never touched
        let other = dir.join("notes.txt");
        std::fs::write(&other, b"mine").unwrap();
        c.prune();
        assert!(other.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
