//! Per-folder layout memory (Settings → Layout → "Sync layout across folders" off): each
//! folder remembers its view, sort and grouping, like Files. Stored in
//! `%APPDATA%\rustyfiles\layouts.json`, written in the background after a change.

use std::collections::HashMap;
use std::path::Path;

use gpui_kit::*;
use serde::{Deserialize, Serialize};

/// In memory a layout holds any sort key. On disk (`FolderLayoutFile`) the keys added later (tag,
/// path, sync) are written as `sort = "Name"` plus `sort_x = "tag"` etc., so that an OLDER build,
/// which has no such variants, still parses the whole file and sorts that folder by name; a
/// layout file written before `sort_x` existed has none and reads as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "FolderLayoutFile", into = "FolderLayoutFile")]
pub struct FolderLayout {
    /// 0 details, 1 grid, 2 columns
    pub view: u8,
    pub sort: rf_core::SortKey,
    pub desc: bool,
    pub dirs_first: bool,
    #[serde(default)]
    pub files_first: bool,
    /// "Size" slider: Details row height / grid tile size (indices).
    #[serde(default = "default_row")]
    pub row_size: u8,
    #[serde(default = "default_grid")]
    pub grid_size: u8,
    /// 0 none, 1 name, 2 type, 3 modified, 4 size
    pub group: u8,
}

/// The shape in the file. `sort` is always a key every build knows; `sort_x` names a newer one.
#[derive(Serialize, Deserialize)]
struct FolderLayoutFile {
    view: u8,
    sort: rf_core::SortKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sort_x: Option<String>,
    desc: bool,
    dirs_first: bool,
    #[serde(default)]
    files_first: bool,
    #[serde(default = "default_row")]
    row_size: u8,
    #[serde(default = "default_grid")]
    grid_size: u8,
    group: u8,
}

impl From<FolderLayout> for FolderLayoutFile {
    fn from(l: FolderLayout) -> Self {
        let (sort, sort_x) = l.sort.stored();
        FolderLayoutFile {
            view: l.view,
            sort,
            sort_x: sort_x.map(str::to_string),
            desc: l.desc,
            dirs_first: l.dirs_first,
            files_first: l.files_first,
            row_size: l.row_size,
            grid_size: l.grid_size,
            group: l.group,
        }
    }
}

impl From<FolderLayoutFile> for FolderLayout {
    fn from(f: FolderLayoutFile) -> Self {
        FolderLayout {
            view: f.view,
            sort: rf_core::SortKey::from_stored(f.sort, f.sort_x.as_deref()),
            desc: f.desc,
            dirs_first: f.dirs_first,
            files_first: f.files_first,
            row_size: f.row_size,
            grid_size: f.grid_size,
            group: f.group,
        }
    }
}

fn default_row() -> u8 {
    1
}
fn default_grid() -> u8 {
    2
}

#[derive(Default)]
struct Layouts {
    map: HashMap<String, FolderLayout>,
    loaded: bool,
    dirty_gen: u64,
}
impl Global for Layouts {}

const MAX: usize = 4000;

fn file() -> Option<std::path::PathBuf> {
    crate::settings::app_dir().map(|d| d.join("layouts.json"))
}

fn key(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

fn store(cx: &mut App) -> &mut Layouts {
    if !cx.has_global::<Layouts>() {
        cx.set_global(Layouts::default());
    }
    let l = cx.global_mut::<Layouts>();
    if !l.loaded {
        l.loaded = true;
        if let Some(text) = file().and_then(|f| std::fs::read_to_string(f).ok()) {
            l.map = serde_json::from_str(&text).unwrap_or_default();
        }
    }
    l
}

pub fn get(path: &Path, cx: &mut App) -> Option<FolderLayout> {
    store(cx).map.get(&key(path)).copied()
}

pub fn set(path: &Path, layout: FolderLayout, cx: &mut App) {
    let s = store(cx);
    if s.map.get(&key(path)) == Some(&layout) {
        return;
    }
    if s.map.len() >= MAX {
        // forget an arbitrary half; layouts are a convenience, not data
        let drop: Vec<String> = s.map.keys().take(MAX / 2).cloned().collect();
        for k in drop {
            s.map.remove(&k);
        }
    }
    s.map.insert(key(path), layout);
    s.dirty_gen += 1;
    let gen_ = s.dirty_gen;
    // coalesce bursts (sort clicks) into one write
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(800)).await;
        let _ = cx.update(|cx| {
            let s = store(cx);
            if s.dirty_gen != gen_ {
                return;
            }
            let Ok(text) = serde_json::to_string(&s.map) else { return };
            if let Some(f) = file() {
                std::thread::spawn(move || {
                    if let Some(d) = f.parent() {
                        let _ = std::fs::create_dir_all(d);
                    }
                    let tmp = f.with_extension("json.tmp");
                    if std::fs::write(&tmp, text).is_ok() {
                        let _ = std::fs::rename(&tmp, &f);
                    }
                });
            }
        });
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use rf_core::SortKey;
    use serde::Deserialize;

    // (not `use super::*`: that brings gpui's `test` attribute macro into scope and #[test] then loops)
    use super::{FolderLayout, default_grid, default_row};

    fn layout(sort: SortKey) -> FolderLayout {
        FolderLayout { view: 0, sort, desc: true, dirs_first: true, files_first: false, row_size: 1, grid_size: 2, group: 0 }
    }

    /// What an older build's layout looked like: five sort keys, no `sort_x`, unknown fields ignored.
    #[derive(Deserialize, Debug, PartialEq)]
    enum OldKey {
        Name,
        Modified,
        Created,
        Size,
        Type,
    }
    #[derive(Deserialize, Debug)]
    #[allow(dead_code)]
    struct OldLayout {
        view: u8,
        sort: OldKey,
        desc: bool,
        dirs_first: bool,
        #[serde(default)]
        files_first: bool,
        #[serde(default = "default_row")]
        row_size: u8,
        #[serde(default = "default_grid")]
        grid_size: u8,
        group: u8,
    }

    #[test]
    fn a_layout_file_written_before_the_new_keys_still_loads() {
        let old = r#"{"c:\\a":{"view":0,"sort":"Size","desc":true,"dirs_first":true,"group":3},
                      "c:\\b":{"view":1,"sort":"Type","desc":false,"dirs_first":false,"files_first":true,"row_size":2,"grid_size":3,"group":0}}"#;
        let map: HashMap<String, FolderLayout> = serde_json::from_str(old).expect("old file parses");
        assert_eq!(map["c:\\a"].sort, SortKey::Size);
        assert_eq!(map["c:\\a"].row_size, 1, "fields missing in the old file take their defaults");
        assert!(map["c:\\b"].files_first);
        assert_eq!(map["c:\\b"].sort, SortKey::Type);
    }

    #[test]
    fn a_new_key_survives_a_round_trip() {
        for k in [SortKey::Tag, SortKey::Path, SortKey::Sync, SortKey::Modified] {
            let text = serde_json::to_string(&layout(k)).unwrap();
            let back: FolderLayout = serde_json::from_str(&text).unwrap();
            assert_eq!(back, layout(k), "{text}");
        }
    }

    #[test]
    fn a_new_key_is_written_as_name_so_that_an_older_build_reads_the_file() {
        let mut map = HashMap::new();
        map.insert("c:\\t".to_string(), layout(SortKey::Tag));
        map.insert("c:\\s".to_string(), layout(SortKey::Size));
        let text = serde_json::to_string(&map).unwrap();
        assert!(text.contains(r#""sort_x":"tag""#), "{text}");
        // an older build: every folder's layout reads, the new key's folder sorts by name
        let old: HashMap<String, OldLayout> = serde_json::from_str(&text).expect("an older build parses the file");
        assert_eq!(old["c:\\t"].sort, OldKey::Name);
        assert!(old["c:\\t"].desc);
        assert_eq!(old["c:\\s"].sort, OldKey::Size);
        // and the file of an older key carries no new field at all
        assert!(!serde_json::to_string(&layout(SortKey::Size)).unwrap().contains("sort_x"));
    }

    #[test]
    fn a_key_name_from_a_later_build_leaves_the_key_an_older_build_would_read() {
        let text = r#"{"view":0,"sort":"Modified","sort_x":"owner","desc":false,"dirs_first":true,"group":0}"#;
        let l: FolderLayout = serde_json::from_str(text).unwrap();
        assert_eq!(l.sort, SortKey::Modified);
    }
}
