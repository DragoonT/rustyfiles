//! Archives opened like folders (ZIP, 7z, RAR, the tar family, ISO), through rf-archive.
//!
//! An archive is listed once; each folder inside it becomes a [`Folder`] the table shows like
//! any other. Files are written out only when they are needed (opened, previewed, copied or
//! extracted), and only those files.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rf_core::entry::{Entry, EntryKind, attrs};

use crate::{Folder, SortColumn};

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ArchiveError {
    /// The archive is encrypted and no password (or a wrong one) was given.
    #[error("password needed")]
    PasswordNeeded,
    #[error("{message}")]
    Failed { message: String },
}

impl ArchiveError {
    fn from_anyhow(e: anyhow::Error) -> Self {
        if e.downcast_ref::<rf_archive::PasswordError>().is_some() {
            ArchiveError::PasswordNeeded
        } else {
            ArchiveError::Failed { message: format!("{e:#}") }
        }
    }
}

/// Whether rustyfiles can open `path` as a folder (by its name).
#[uniffi::export]
pub fn is_archive(path: String) -> bool {
    rf_archive::is_supported(Path::new(&path))
}

/// A listed archive.
#[derive(uniffi::Object)]
pub struct Archive {
    path: PathBuf,
    password: Option<String>,
    /// Every folder inside the archive (by its path, "" for the top) and what is directly in it.
    /// Folders the archive implies without storing them ("a/b/c.txt" alone) are made up here.
    tree: BTreeMap<String, Vec<Entry>>,
}

fn norm(p: &str) -> String {
    p.replace('\\', "/").trim_matches('/').to_string()
}

#[uniffi::export]
impl Archive {
    /// Lists `path`. Blocks on the disk (and, for tar streams, decompression): call it off the
    /// main thread.
    #[uniffi::constructor]
    pub fn open(path: String, password: Option<String>) -> Result<Arc<Self>, ArchiveError> {
        let p = PathBuf::from(&path);
        let opts = rf_archive::Options { password: password.clone(), only: Vec::new() };
        let entries = rf_archive::list_with(&p, &opts).map_err(ArchiveError::from_anyhow)?;
        let mut tree: BTreeMap<String, BTreeMap<String, Entry>> = BTreeMap::new();
        tree.entry(String::new()).or_default();
        for e in &entries {
            let full = norm(&e.path);
            if full.is_empty() {
                continue;
            }
            let parts: Vec<&str> = full.split('/').filter(|s| !s.is_empty() && *s != "." && *s != "..").collect();
            // every folder on the way, made up when the archive does not store it
            for depth in 0..parts.len() {
                let parent = parts[..depth].join("/");
                let name = parts[depth];
                let last = depth + 1 == parts.len();
                let children = tree.entry(parent).or_default();
                if last && !e.is_dir {
                    let mut entry = Entry::new(name, EntryKind::File, e.size, e.modified_ms, 0, 0);
                    if name.starts_with('.') {
                        entry.attrs |= attrs::HIDDEN;
                    }
                    children.insert(name.to_string(), entry);
                } else {
                    let ms = if last { e.modified_ms } else { 0 };
                    let dir = children
                        .entry(name.to_string())
                        .or_insert_with(|| Entry::new(name, EntryKind::Dir, 0, ms, 0, attrs::DIRECTORY));
                    if last && ms != 0 {
                        dir.modified_ms = ms;
                    }
                    tree.entry(parts[..=depth].join("/")).or_default();
                }
            }
        }
        let tree = tree.into_iter().map(|(k, v)| (k, v.into_values().collect())).collect();
        Ok(Arc::new(Archive { path: p, password, tree }))
    }

    /// Whether `inner` ("" for the top, "a/b") is a folder in the archive.
    pub fn has_folder(&self, inner: String) -> bool {
        self.tree.contains_key(&norm(&inner))
    }

    /// The folder `inner` as a listing (its rows' paths are `<archive>/<inner>/<name>`).
    pub fn folder(&self, inner: String, column: SortColumn, ascending: bool, folders_first: bool, show_hidden: bool) -> Option<Arc<Folder>> {
        let inner = norm(&inner);
        let entries = self.tree.get(&inner)?.clone();
        let path = if inner.is_empty() { self.path.clone() } else { self.path.join(&inner) };
        let folder = Folder::from_entries(path, entries);
        folder.arrange(column, ascending, folders_first, show_hidden, String::new());
        Some(Arc::new(folder))
    }

    /// Writes the members at `inner_paths` (a folder brings everything under it) into `dest`,
    /// keeping their paths inside the archive. Returns how many files were written.
    pub fn extract(&self, inner_paths: Vec<String>, dest: String) -> Result<u64, ArchiveError> {
        let only = inner_paths.iter().map(|p| norm(p)).collect();
        let opts = rf_archive::Options { password: self.password.clone(), only };
        let stats = rf_archive::extract_with(&self.path, Path::new(&dest), &opts, &mut |_| {}, &|| false)
            .map_err(ArchiveError::from_anyhow)?;
        Ok(stats.files)
    }

    /// Everything into `dest` (created if missing).
    pub fn extract_all(&self, dest: String) -> Result<u64, ArchiveError> {
        self.extract(Vec::new(), dest)
    }

    pub fn needs_password(&self) -> bool {
        self.password.is_none() && rf_archive::needs_password(&self.path).unwrap_or(false)
    }
}

/// Where "Extract here" writes for `archive` inside `into`, as on Windows: an archive whose
/// members are all under one folder is written as is, anything else into a new folder named
/// after the archive. Returns (where to extract, the folder that then holds the result).
#[uniffi::export]
pub fn smart_extract_target(archive: String, into: String) -> Vec<String> {
    let a = Path::new(&archive);
    let entries = rf_archive::list(a).unwrap_or_default();
    let (dest, shown) = rf_archive::smart_dest(a, Path::new(&into), &entries);
    vec![dest.display().to_string(), shown.display().to_string()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_zip_is_browsed_like_folders() {
        let d = std::env::temp_dir().join(format!("rf-mac-zip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let zip_path = d.join("t.zip");
        {
            let f = std::fs::File::create(&zip_path).unwrap();
            let mut z = zip::ZipWriter::new(f);
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("top.txt", o).unwrap();
            z.write_all(b"hello").unwrap();
            // a folder the archive only implies
            z.start_file("photos/2024/a.jpg", o).unwrap();
            z.write_all(b"12345678").unwrap();
            z.finish().unwrap();
        }
        let a = Archive::open(zip_path.display().to_string(), None).unwrap();
        let top = a.folder(String::new(), SortColumn::Name, true, true, false).unwrap();
        let names: Vec<String> = (0..top.count()).map(|r| top.item(r).unwrap().name).collect();
        assert_eq!(names, ["photos", "top.txt"]);
        assert!(a.has_folder("photos/2024".into()));
        let inner = a.folder("photos/2024".into(), SortColumn::Name, true, true, false).unwrap();
        assert_eq!(inner.item(0).unwrap().size, 8);
        assert!(inner.child_path(0).unwrap().ends_with("t.zip/photos/2024/a.jpg"));
        let out = d.join("out");
        assert_eq!(a.extract(vec!["photos/2024/a.jpg".into()], out.display().to_string()).unwrap(), 1);
        assert_eq!(std::fs::read(out.join("photos/2024/a.jpg")).unwrap(), b"12345678");
        assert!(!out.join("top.txt").exists());
        // and back: what came out packs into a 7z that lists the same file
        let packed = d.join("again.7z");
        compress(vec![out.join("photos").display().to_string()], packed.display().to_string(), ArchiveFormat::SevenZ).unwrap();
        let again = Archive::open(packed.display().to_string(), None).unwrap();
        assert!(again.has_folder("photos/2024".into()));
        std::fs::remove_dir_all(&d).unwrap();
    }
}

/// What "Compress to" makes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ArchiveFormat {
    Zip,
    SevenZ,
    Rar,
}

/// Packs `sources` into the archive `dest` (which must not exist), at the normal level as the
/// Windows app's "Compress to ZIP / 7z / RAR" does. RAR needs RARLAB's `rar` ([`can_make_rar`]).
/// Blocks: call it off the main thread.
#[uniffi::export]
pub fn compress(sources: Vec<String>, dest: String, format: ArchiveFormat) -> Result<(), ArchiveError> {
    let sources: Vec<PathBuf> = sources.into_iter().map(PathBuf::from).collect();
    let dest = Path::new(&dest);
    // "Normal" in the Windows app's level choice (None / Fast / Normal / Maximum)
    let level = 5;
    let result = match format {
        ArchiveFormat::Zip => rf_archive::compress(&sources, dest, rf_archive::Format::Zip, level, &mut |_| {}, &|| false),
        ArchiveFormat::SevenZ => rf_archive::compress(&sources, dest, rf_archive::Format::SevenZ, level, &mut |_| {}, &|| false),
        ArchiveFormat::Rar => rf_archive::compress_rar(&sources, dest, level, None, &mut |_| {}, &|| false),
    };
    result.map(|_| ()).map_err(ArchiveError::from_anyhow)
}

/// Whether RAR archives can be made here (RARLAB's `rar` is installed).
#[uniffi::export]
pub fn can_make_rar() -> bool {
    rf_archive::rar_exe().is_some()
}
