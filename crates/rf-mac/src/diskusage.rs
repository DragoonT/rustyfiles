//! Disk usage for the Mac app (the Windows app's `rf_win::diskusage` + disk_usage.rs): where
//! the space under a folder goes. The walk counts the blocks each file takes on disk, stays on
//! the folder's own disk and does not follow links. The tree is kept here; the app asks for a
//! folder's children (biggest first) and for treemap rectangles (`rf_core::treemap`).

use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use rayon::prelude::*;
use rf_core::treemap::{self, Rect};

/// Files kept by name per folder; the smaller rest of a folder's files become one item.
const FILES_KEPT: usize = 300;

struct Node {
    name: String,
    size: u64,
    files: u64,
    is_dir: bool,
    /// "N smaller files" standing for the files not kept.
    is_rest: bool,
    children: Vec<Node>,
}

/// One item of the tree as the app sees it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DiskItem {
    /// Where it is in the tree: the indices from the root down (empty: the root).
    pub id: Vec<u32>,
    pub name: String,
    pub path: String,
    /// Bytes on disk.
    pub size: u64,
    /// Files inside (1 for a file).
    pub files: u64,
    pub is_dir: bool,
    /// The smaller files of a folder, counted together.
    pub is_rest: bool,
    pub has_children: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct DiskRect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// A disk usage walk; dropped or cancelled, it stops.
#[derive(uniffi::Object)]
pub struct DiskScan {
    root: PathBuf,
    tree: Arc<Mutex<Option<Node>>>,
    done: Arc<AtomicBool>,
    cancelled: Arc<AtomicBool>,
    entries: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
}

#[uniffi::export]
impl DiskScan {
    #[uniffi::constructor]
    pub fn start(root: String) -> Arc<Self> {
        let scan = Arc::new(DiskScan {
            root: PathBuf::from(&root),
            tree: Arc::new(Mutex::new(None)),
            done: Arc::new(AtomicBool::new(false)),
            cancelled: Arc::new(AtomicBool::new(false)),
            entries: Arc::new(AtomicU64::new(0)),
            bytes: Arc::new(AtomicU64::new(0)),
        });
        let (tree, done, cancelled, entries, bytes) = (scan.tree.clone(), scan.done.clone(), scan.cancelled.clone(), scan.entries.clone(), scan.bytes.clone());
        std::thread::Builder::new()
            .name("rf-diskusage".into())
            .spawn(move || {
                let root = Path::new(&root);
                let dev = std::fs::metadata(root).map(|m| m.dev()).unwrap_or(0);
                let w = Walk { dev, cancelled, entries, bytes };
                let name = root.file_name().map_or_else(|| root.display().to_string(), |n| n.to_string_lossy().into_owned());
                let mut node = w.dir(root, name);
                if !w.cancelled.load(Ordering::Relaxed) {
                    sort(&mut node);
                    *tree.lock().unwrap() = Some(node);
                }
                done.store(true, Ordering::Release);
            })
            .ok();
        scan
    }

    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }

    /// Items and bytes counted so far.
    pub fn entries(&self) -> u64 {
        self.entries.load(Ordering::Relaxed)
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// The item at `id` (once done).
    pub fn item(&self, id: Vec<u32>) -> Option<DiskItem> {
        let tree = self.tree.lock().unwrap();
        let (node, path) = find(tree.as_ref()?, &self.root, &id)?;
        Some(describe(node, id, path))
    }

    /// The children of the item at `id`, biggest first.
    pub fn children(&self, id: Vec<u32>) -> Vec<DiskItem> {
        let tree = self.tree.lock().unwrap();
        let Some((node, path)) = tree.as_ref().and_then(|t| find(t, &self.root, &id)) else { return Vec::new() };
        node.children
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut cid = id.clone();
                cid.push(i as u32);
                describe(c, cid, if c.is_rest { path.clone() } else { path.join(&c.name) })
            })
            .collect()
    }
}

/// Squarified treemap rectangles for `weights` (biggest first) in a `w` × `h` box.
#[uniffi::export]
pub fn treemap_layout(weights: Vec<u64>, w: f64, h: f64) -> Vec<DiskRect> {
    treemap::squarify(&weights, Rect { x: 0., y: 0., w: w as f32, h: h as f32 })
        .into_iter()
        .map(|r| DiskRect { x: r.x as f64, y: r.y as f64, w: r.w as f64, h: r.h as f64 })
        .collect()
}

fn describe(n: &Node, id: Vec<u32>, path: PathBuf) -> DiskItem {
    DiskItem {
        id,
        name: n.name.clone(),
        path: path.display().to_string(),
        size: n.size,
        files: n.files,
        is_dir: n.is_dir,
        is_rest: n.is_rest,
        has_children: !n.children.is_empty(),
    }
}

fn find<'a>(root: &'a Node, root_path: &Path, id: &[u32]) -> Option<(&'a Node, PathBuf)> {
    let mut n = root;
    let mut p = root_path.to_path_buf();
    for &i in id {
        n = n.children.get(i as usize)?;
        if !n.is_rest {
            p.push(&n.name);
        }
    }
    Some((n, p))
}

fn sort(n: &mut Node) {
    n.children.sort_by(|a, b| b.size.cmp(&a.size));
    n.children.par_iter_mut().for_each(sort);
}

struct Walk {
    dev: u64,
    cancelled: Arc<AtomicBool>,
    entries: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
}

impl Walk {
    fn dir(&self, path: &Path, name: String) -> Node {
        let mut node = Node { name, size: 0, files: 0, is_dir: true, is_rest: false, children: Vec::new() };
        if self.cancelled.load(Ordering::Relaxed) {
            return node;
        }
        let Ok(rd) = std::fs::read_dir(path) else { return node };
        let mut files = Vec::new();
        let mut dirs = Vec::new();
        for e in rd.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            let n = e.file_name().to_string_lossy().into_owned();
            if m.is_dir() {
                if m.dev() == self.dev {
                    dirs.push((e.path(), n));
                }
            } else {
                let size = m.blocks() * 512;
                self.entries.fetch_add(1, Ordering::Relaxed);
                self.bytes.fetch_add(size, Ordering::Relaxed);
                files.push(Node { name: n, size, files: 1, is_dir: false, is_rest: false, children: Vec::new() });
            }
        }
        let sub: Vec<Node> = dirs.par_iter().map(|(p, n)| self.dir(p, n.clone())).collect();
        node.size = files.iter().map(|f| f.size).sum::<u64>() + sub.iter().map(|d| d.size).sum::<u64>();
        node.files = files.len() as u64 + sub.iter().map(|d| d.files).sum::<u64>();
        if files.len() > FILES_KEPT {
            files.sort_by(|a, b| b.size.cmp(&a.size));
            let rest: Vec<Node> = files.split_off(FILES_KEPT);
            let size = rest.iter().map(|f| f.size).sum();
            files.push(Node { name: format!("{} smaller files", rest.len()), size, files: rest.len() as u64, is_dir: false, is_rest: true, children: Vec::new() });
        }
        node.children = sub;
        node.children.extend(files);
        node
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_add_up_and_children_come_biggest_first() {
        let d = std::env::temp_dir().join(format!("rf-mac-du-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("big")).unwrap();
        std::fs::write(d.join("big/a.bin"), vec![1u8; 200_000]).unwrap();
        std::fs::write(d.join("small.txt"), b"hi").unwrap();
        let s = DiskScan::start(d.display().to_string());
        while !s.is_done() {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let root = s.item(vec![]).unwrap();
        let kids = s.children(vec![]);
        assert_eq!(kids[0].name, "big");
        assert!(kids[0].is_dir && kids[0].has_children);
        assert_eq!(root.size, kids.iter().map(|k| k.size).sum::<u64>());
        assert_eq!(root.files, 2);
        assert!(s.children(vec![0])[0].path.ends_with("big/a.bin"));
        let rects = treemap_layout(kids.iter().map(|k| k.size).collect(), 100., 50.);
        assert_eq!(rects.len(), 2);
        let _ = std::fs::remove_dir_all(&d);
    }
}
