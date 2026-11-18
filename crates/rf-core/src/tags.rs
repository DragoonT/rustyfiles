//! File tags (Files' "Tags"): colored labels assigned to paths, persisted as JSON.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TagDef {
    pub id: u32,
    pub name: String,
    /// 0xRRGGBB
    pub color: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tagged {
    /// Original-case path (keys are lower-cased for case-insensitive lookup).
    pub path: PathBuf,
    pub tags: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TagStore {
    pub defs: Vec<TagDef>,
    /// Key: lower-cased absolute path.
    pub items: BTreeMap<String, Tagged>,
    pub next_id: u32,
}

impl Default for TagStore {
    fn default() -> Self {
        let defaults = [
            ("Kırmızı", 0xFF453A),
            ("Turuncu", 0xFF9F0A),
            ("Sarı", 0xFFD60A),
            ("Yeşil", 0x30D158),
            ("Mavi", 0x0A84FF),
            ("Mor", 0xBF5AF2),
            ("Gri", 0x8E8E93),
        ];
        Self {
            defs: defaults
                .iter()
                .enumerate()
                .map(|(i, (n, c))| TagDef { id: i as u32 + 1, name: n.to_string(), color: *c })
                .collect(),
            items: BTreeMap::new(),
            next_id: defaults.len() as u32 + 1,
        }
    }
}

fn key(p: &Path) -> String {
    let s = p.to_string_lossy().to_lowercase();
    if s.len() > 3 { s.trim_end_matches(['\\', '/']).to_string() } else { s }
}

impl TagStore {
    pub fn def(&self, id: u32) -> Option<&TagDef> {
        self.defs.iter().find(|d| d.id == id)
    }

    /// Creates a tag; returns its id.
    pub fn add(&mut self, name: &str, color: u32) -> u32 {
        let id = self.next_id.max(self.defs.iter().map(|d| d.id + 1).max().unwrap_or(1));
        self.next_id = id + 1;
        self.defs.push(TagDef { id, name: name.trim().to_string(), color });
        id
    }

    pub fn rename(&mut self, id: u32, name: &str) {
        if let Some(d) = self.defs.iter_mut().find(|d| d.id == id) {
            d.name = name.trim().to_string();
        }
    }

    pub fn recolor(&mut self, id: u32, color: u32) {
        if let Some(d) = self.defs.iter_mut().find(|d| d.id == id) {
            d.color = color;
        }
    }

    /// Deletes a tag and takes it off every item.
    pub fn remove(&mut self, id: u32) {
        self.defs.retain(|d| d.id != id);
        for v in self.items.values_mut() {
            v.tags.retain(|t| *t != id);
        }
        self.items.retain(|_, v| !v.tags.is_empty());
    }

    pub fn tags_of(&self, p: &Path) -> &[u32] {
        self.items.get(&key(p)).map(|v| v.tags.as_slice()).unwrap_or(&[])
    }

    pub fn has(&self, p: &Path, id: u32) -> bool {
        self.tags_of(p).contains(&id)
    }

    /// Color (`0xRRGGBB`) of the first defined tag assigned to `p`, if any.
    pub fn first_color(&self, p: &Path) -> Option<u32> {
        if self.items.is_empty() {
            return None;
        }
        self.tags_of(p).iter().find_map(|id| self.def(*id)).map(|d| d.color)
    }

    /// Adds `id` to all paths, or removes it from all when every path already has it.
    pub fn toggle(&mut self, paths: &[PathBuf], id: u32) {
        let all = !paths.is_empty() && paths.iter().all(|p| self.has(p, id));
        for p in paths {
            let k = key(p);
            let v = self.items.entry(k.clone()).or_insert_with(|| Tagged { path: p.clone(), tags: Vec::new() });
            if all {
                v.tags.retain(|t| *t != id);
            } else if !v.tags.contains(&id) {
                v.tags.push(id);
            }
            if v.tags.is_empty() {
                self.items.remove(&k);
            }
        }
    }

    /// The tagged items that lie directly in `dir` (not deeper), without looking at the others: for a list that wants to know what
    /// its tag column will show. `dir` in any letter case, with or without a trailing separator.
    pub fn tagged_in(&self, dir: &Path) -> impl Iterator<Item = &Tagged> + '_ {
        let mut prefix = key(dir);
        if !prefix.ends_with('\\') {
            prefix.push('\\');
        }
        let n = prefix.len();
        self.items.range(prefix.clone()..).take_while(move |(k, _)| k.starts_with(&prefix)).filter(move |(k, _)| !k[n..].contains('\\')).map(|(_, v)| v)
    }

    pub fn paths_with(&self, id: u32) -> Vec<PathBuf> {
        self.items.values().filter(|v| v.tags.contains(&id)).map(|v| v.path.clone()).collect()
    }

    /// Keep tags when a file is renamed/moved inside the app.
    pub fn moved(&mut self, from: &Path, to: &Path) {
        if let Some(mut v) = self.items.remove(&key(from)) {
            v.path = to.to_path_buf();
            self.items.insert(key(to), v);
        }
    }

    pub fn load(path: &Path) -> Self {
        let text = match crate::read_config(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(_) => {
                crate::backup_corrupt(path);
                return Self::default();
            }
        };
        match serde_json::from_str(text.trim_start_matches('\u{feff}')) {
            Ok(store) => store,
            Err(_) => {
                // Unreadable: keep it aside instead of letting the next save overwrite the
                // user's tags with an empty store.
                crate::backup_corrupt(path);
                Self::default()
            }
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        static LOCK: parking_lot::Mutex<u64> = parking_lot::Mutex::new(0);
        let mut n = LOCK.lock();
        *n += 1;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension(format!("json.{}.{}.tmp", std::process::id(), *n));
        // never replace the real file with an empty one if serialization fails
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]

    #[cfg_attr(not(windows), ignore = "Windows paths and APIs; a Unix test comes when the Mac port uses this")]
    fn the_tagged_items_of_one_folder() {
        let mut s = TagStore::default();
        let p = |x: &str| PathBuf::from(x);
        s.toggle(&[p(r"C:\Work\a.txt"), p(r"C:\Work\Sub\deep.txt"), p(r"C:\Workshop\b.txt"), p(r"C:\work\Z.txt"), p(r"D:\Work\a.txt")], 1);
        s.toggle(&[p(r"C:\Work\a.txt")], 3);
        let names = |dir: &str| -> Vec<String> { s.tagged_in(Path::new(dir)).map(|t| t.path.file_name().unwrap().to_string_lossy().into_owned()).collect() };
        assert_eq!(names(r"C:\Work"), ["a.txt", "Z.txt"], "its own items, not a subfolder's and not a folder's whose name only begins the same");
        assert_eq!(names(r"c:\WORK\"), ["a.txt", "Z.txt"], "any letter case, a trailing separator");
        assert_eq!(s.tagged_in(Path::new(r"C:\Work")).next().unwrap().tags, [1, 3]);
        assert_eq!(names(r"C:\Work\Sub"), ["deep.txt"]);
        assert!(names(r"C:\Nothing").is_empty() && names(r"E:\").is_empty());
    }

    #[test]
    fn toggle_and_query() {
        let mut s = TagStore::default();
        let a = PathBuf::from(r"C:\A.txt");
        let b = PathBuf::from(r"C:\b.txt");
        s.toggle(&[a.clone()], 1);
        assert!(s.has(Path::new(r"c:\a.TXT"), 1));
        s.toggle(&[a.clone(), b.clone()], 1); // b lacks it → add to both
        assert!(s.has(&b, 1));
        s.toggle(&[a.clone(), b.clone()], 1); // all have → remove
        assert!(!s.has(&a, 1) && !s.has(&b, 1));
        assert!(s.items.is_empty());
        s.toggle(&[a.clone()], 2);
        s.moved(&a, Path::new(r"C:\x.txt"));
        assert_eq!(s.paths_with(2), vec![PathBuf::from(r"C:\x.txt")]);
        let json = serde_json::to_string(&s).unwrap();
        let back: TagStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.defs.len(), 7);
    }

    #[test]
    fn folder_first_color_and_trailing_slash() {
        let mut s = TagStore::default();
        let folder = PathBuf::from(r"C:\Work\Sub");
        assert_eq!(s.first_color(&folder), None);

        s.toggle(&[folder.clone()], 3);
        assert_eq!(s.first_color(&folder), Some(0xFFD60A));
        assert_eq!(s.first_color(Path::new(r"c:\work\sub\")), Some(0xFFD60A));

        s.toggle(&[folder.clone()], 5);
        assert_eq!(s.first_color(&folder), Some(0xFFD60A));

        s.toggle(&[folder.clone()], 3);
        assert_eq!(s.first_color(&folder), Some(0x0A84FF));
    }
}
