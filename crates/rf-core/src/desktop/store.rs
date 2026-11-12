//! The layout file: the app's OWN record of where every desktop icon sits (shell plan 4.5, `desktop-layout.json`). Windows' own store
//! (`Shell\Bags\1\Desktop\IconLayouts`) is never written and, with Explorer gone, nothing would read it.
//!
//! FORMAT (version 1), written pretty so a person can read it:
//! ```json
//! { "version": 1, "icon_size": 48, "auto_arrange": false, "align_to_grid": true,
//!   "sort": { "key": "name", "descending": false }, "show_icons": true, "clock": 7, "imported_from_explorer": false,
//!   "layouts": { "<monitor device path>": { "1920x1080@100": {
//!       "items":   { "u:notes.txt": { "col": 0, "row": 2, "dx": 0, "dy": 0, "at": 5 } },
//!       "removed": { "u:old.txt":   { "col": 1, "row": 0, "dx": 0, "dy": 0, "at": 3, "gone_ms": 1700000000000 } } } } } }
//! ```
//! A cell is a (column, row) in the grid of that monitor at that resolution and scale (so a layout survives a DPI change as a place
//! in a grid, not as pixels); `dx`, `dy` are the offset inside the cell when "align to grid" is off. `at` is a counter that goes up
//! each time an item is placed or moved, so when two layouts both claim an item the more recent one wins. `removed` remembers where a
//! deleted item was for a day, so one restored from the Recycle Bin returns to its place. The monitor key is the device path
//! `IDesktopWallpaper` gives (stable across sessions), the layout key is `"{width}x{height}@{scale percent}"`.
//!
//! WHAT LOADING NEVER DOES: replace a file it could not use by an empty layout. Unreadable JSON is renamed aside (`.bozuk-<ms>`, the
//! same rule as the settings file) and the outcome says so. A file written by a NEWER version is left exactly as it is: the store comes
//! back empty and BLOCKED, and `save` refuses until the file is gone. A file that could not be read at all (a lock) blocks saving
//! the same way. The caller shows the outcome; it does not have to guess.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::sort::{SortDir, SortKey};

use super::grid::IconSize;

pub const STORE_VERSION: u32 = 1;

/// The highest counter a loaded file may have (2^62: it would take longer than the machine lives to count past it).
pub const CLOCK_LIMIT: u64 = 1 << 62;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Slot {
    pub col: u32,
    pub row: u32,
    #[serde(default)]
    pub dx: i32,
    #[serde(default)]
    pub dy: i32,
    #[serde(default)]
    pub at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Removed {
    pub col: u32,
    pub row: u32,
    #[serde(default)]
    pub dx: i32,
    #[serde(default)]
    pub dy: i32,
    #[serde(default)]
    pub at: u64,
    pub gone_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct MonitorLayout {
    /// Where the user (or the first placement, or the import) put each item. Only these are claims: a place that a pass of
    /// `resolve` made up because a claim could not be honoured is never written here, so a docked taskbar, a monitor that is
    /// asleep or an icon size that does not fit cannot overwrite what the user arranged.
    #[serde(default)]
    pub items: BTreeMap<String, Slot>,
    #[serde(default)]
    pub removed: BTreeMap<String, Removed>,
    /// Where an item that could not be put at its claimed cell is shown for now (outside the grid, a collision, its monitor not
    /// connected). Kept so that it does not jump about from one pass to the next; dropped as soon as the claim can be honoured.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub provisional: BTreeMap<String, Slot>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortName {
    #[default]
    Name,
    Size,
    Type,
    Modified,
}

impl SortName {
    pub fn key(self) -> SortKey {
        match self {
            SortName::Name => SortKey::Name,
            SortName::Size => SortKey::Size,
            SortName::Type => SortKey::Type,
            SortName::Modified => SortKey::Modified,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct SortPref {
    pub key: SortName,
    pub descending: bool,
}

impl SortPref {
    pub fn dir(self) -> SortDir {
        if self.descending { SortDir::Desc } else { SortDir::Asc }
    }
}

/// Why the store will not be written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// The file on disk was written by a newer version of rustyfiles.
    NewerVersion(u32),
    /// The file exists and could not be read (a lock, a sharing violation, a failing disk).
    Unreadable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// No file yet: a fresh store.
    NoFile,
    Loaded,
    /// The file could not be parsed. It was renamed to `aside` (when the rename worked) and a fresh store is in use.
    Recovered { aside: Option<PathBuf>, reason: String },
    /// A newer version wrote the file; it is untouched and the store is blocked.
    Newer { found: u32 },
    /// The file could not be read; it is untouched and the store is blocked.
    Unreadable { reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("the layout file is not written: {0:?}")]
    Blocked(Blocked),
    #[error("could not write the layout file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not serialise the layout: {0}")]
    Json(#[from] serde_json::Error),
}

fn default_true() -> bool {
    true
}
fn default_icon_size() -> u32 {
    IconSize::Medium.px()
}
fn default_version() -> u32 {
    STORE_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Store {
    #[serde(default = "default_version")]
    pub version: u32,
    /// The icon's edge at 100 %: 32, 48 or 96 (see `IconSize`).
    #[serde(default = "default_icon_size")]
    pub icon_size: u32,
    #[serde(default)]
    pub auto_arrange: bool,
    #[serde(default = "default_true")]
    pub align_to_grid: bool,
    #[serde(default)]
    pub sort: SortPref,
    #[serde(default = "default_true")]
    pub show_icons: bool,
    /// Goes up whenever an item is placed or moved; see `Slot::at`.
    #[serde(default)]
    pub clock: u64,
    /// The one-time import of Explorer's positions has been done (or declined).
    #[serde(default)]
    pub imported_from_explorer: bool,
    /// monitor device path -> "WxH@pct" -> that layout
    #[serde(default)]
    pub layouts: BTreeMap<String, BTreeMap<String, MonitorLayout>>,
    #[serde(skip)]
    pub(crate) dirty: bool,
    #[serde(skip)]
    pub(crate) blocked: Option<Blocked>,
}

impl Default for Store {
    fn default() -> Self {
        Store {
            version: STORE_VERSION,
            icon_size: IconSize::Medium.px(),
            auto_arrange: false,
            align_to_grid: true,
            sort: SortPref::default(),
            show_icons: true,
            clock: 0,
            imported_from_explorer: false,
            layouts: BTreeMap::new(),
            dirty: false,
            blocked: None,
        }
    }
}

impl Store {
    pub fn icon_size_step(&self) -> IconSize {
        IconSize::from_px(self.icon_size)
    }
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }
    pub fn blocked(&self) -> Option<&Blocked> {
        self.blocked.as_ref()
    }
    pub(crate) fn mark(&mut self) {
        self.dirty = true;
    }
    /// The next value of the placement counter. It saturates: a file with a huge counter must not panic (debug) or wrap round and turn
    /// "the later placement wins" upside down (release).
    pub(crate) fn tick(&mut self) -> u64 {
        self.clock = self.clock.saturating_add(1);
        self.dirty = true;
        self.clock
    }

    pub fn set_auto_arrange(&mut self, on: bool) {
        if self.auto_arrange != on {
            self.auto_arrange = on;
            self.dirty = true;
        }
    }
    pub fn set_align_to_grid(&mut self, on: bool) {
        if self.align_to_grid != on {
            self.align_to_grid = on;
            self.dirty = true;
        }
    }
    pub fn set_sort(&mut self, sort: SortPref) {
        if self.sort != sort {
            self.sort = sort;
            self.dirty = true;
        }
    }
    pub fn set_show_icons(&mut self, on: bool) {
        if self.show_icons != on {
            self.show_icons = on;
            self.dirty = true;
        }
    }
    pub fn mark_imported(&mut self) {
        if !self.imported_from_explorer {
            self.imported_from_explorer = true;
            self.dirty = true;
        }
    }

    /// Reads the file. See the top of this file for what happens to a file that cannot be used.
    pub fn load(path: &Path) -> (Store, LoadOutcome) {
        let text = match crate::read_config(path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (Store::default(), LoadOutcome::NoFile),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData => return Self::corrupt(path, format!("not text: {e}")),
            Err(e) => {
                let reason = e.to_string();
                let s = Store { blocked: Some(Blocked::Unreadable(reason.clone())), ..Store::default() };
                return (s, LoadOutcome::Unreadable { reason });
            }
        };
        let value: serde_json::Value = match serde_json::from_str(&text) {
            Ok(v) => v,
            Err(e) => return Self::corrupt(path, format!("not JSON: {e}")),
        };
        let Some(version) = value.get("version").and_then(|v| v.as_u64()) else {
            return Self::corrupt(path, "no version number".into());
        };
        if version > STORE_VERSION as u64 {
            let found = version.min(u32::MAX as u64) as u32;
            let s = Store { blocked: Some(Blocked::NewerVersion(found)), ..Store::default() };
            return (s, LoadOutcome::Newer { found });
        }
        if version == 0 {
            return Self::corrupt(path, "version 0".into());
        }
        match serde_json::from_value::<Store>(value) {
            Ok(mut s) => {
                s.sanitise();
                (s, LoadOutcome::Loaded)
            }
            Err(e) => Self::corrupt(path, format!("wrong shape: {e}")),
        }
    }

    /// Brings numbers from a file nobody checked into a range the rules can work with: the counter cannot be so high that the next
    /// ticks saturate, and no slot claims to be newer than the counter.
    fn sanitise(&mut self) {
        self.clock = self.clock.min(CLOCK_LIMIT);
        let clock = self.clock;
        for l in self.layouts.values_mut().flat_map(|by_key| by_key.values_mut()) {
            for s in l.items.values_mut().chain(l.provisional.values_mut()) {
                s.at = s.at.min(clock);
            }
            for r in l.removed.values_mut() {
                r.at = r.at.min(clock);
            }
        }
    }

    fn corrupt(path: &Path, reason: String) -> (Store, LoadOutcome) {
        let before = aside_names(path);
        crate::backup_corrupt(path);
        let aside = aside_names(path).into_iter().find(|p| !before.contains(p));
        (Store::default(), LoadOutcome::Recovered { aside, reason })
    }

    /// Writes the file atomically (a temporary file next to it, then a rename over it). Refuses while the store is blocked.
    pub fn save(&mut self, path: &Path) -> Result<(), SaveError> {
        if let Some(b) = &self.blocked {
            return Err(SaveError::Blocked(b.clone()));
        }
        let text = serde_json::to_string_pretty(self)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(format!(".tmp-{}", std::process::id()));
        let tmp = PathBuf::from(tmp);
        let r = std::fs::write(&tmp, text).and_then(|_| std::fs::rename(&tmp, path));
        if r.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        r?;
        self.dirty = false;
        Ok(())
    }

    /// `save` when something changed since the last save or load; the app calls this from a timer a short while after a change.
    pub fn save_if_dirty(&mut self, path: &Path) -> Result<bool, SaveError> {
        if !self.dirty {
            return Ok(false);
        }
        self.save(path).map(|_| true)
    }
}

fn aside_names(path: &Path) -> Vec<PathBuf> {
    let (Some(dir), Some(name)) = (path.parent(), path.file_name().and_then(|n| n.to_str())) else { return Vec::new() };
    let prefix = format!("{name}.bozuk-");
    std::fs::read_dir(dir)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&prefix)).map(|e| e.path()).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-layout-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0)));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sample() -> Store {
        let mut s = Store { icon_size: 96, auto_arrange: true, align_to_grid: false, show_icons: false, clock: 7, imported_from_explorer: true, ..Store::default() };
        s.sort = SortPref { key: SortName::Modified, descending: true };
        let mut l = MonitorLayout::default();
        l.items.insert("u:notes.txt".into(), Slot { col: 3, row: 4, dx: 5, dy: 6, at: 7 });
        l.items.insert("::{20D04FE0-3AEA-1069-A2D8-08002B30309D}".into(), Slot { col: 0, row: 0, dx: 0, dy: 0, at: 1 });
        l.removed.insert("p:old.lnk".into(), Removed { col: 1, row: 1, dx: 0, dy: 0, at: 2, gone_ms: 1_700_000_000_000 });
        s.layouts.entry(r"\\?\DISPLAY#X#1&2".into()).or_default().insert("2560x1440@125".into(), l);
        s
    }

    #[test]
    fn round_trip() {
        let dir = temp("rt");
        let path = dir.join("desktop-layout.json");
        let mut s = sample();
        s.save(&path).unwrap();
        assert!(!s.is_dirty());
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"version\": 1") && text.contains("\"key\": \"modified\"") && text.contains("2560x1440@125"), "{text}");
        let (back, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
        // no temporary file is left next to it
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["desktop-layout.json"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_file_is_a_fresh_store_and_the_first_save_creates_the_folder() {
        let dir = temp("fresh");
        let path = dir.join("deeper").join("desktop-layout.json");
        let (mut s, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::NoFile);
        assert_eq!(s, Store::default());
        assert!(!s.save_if_dirty(&path).unwrap(), "nothing to write");
        s.set_auto_arrange(true);
        assert!(s.save_if_dirty(&path).unwrap());
        assert!(path.exists());
        assert!(!s.save_if_dirty(&path).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_fields_take_their_defaults_and_unknown_ones_are_ignored() {
        let dir = temp("lenient");
        let path = dir.join("l.json");
        std::fs::write(&path, r#"{ "version": 1, "future_field": [1,2,3], "layouts": { "m": { "1x1@100": { "items": { "u:a": { "col": 1, "row": 2 } }, "extra": 1 } } } }"#).unwrap();
        let (s, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!((s.icon_size, s.align_to_grid, s.show_icons, s.auto_arrange), (48, true, true, false));
        assert_eq!(s.layouts["m"]["1x1@100"].items["u:a"], Slot { col: 1, row: 2, dx: 0, dy: 0, at: 0 });
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_is_renamed_aside_never_replaced_by_an_empty_layout() {
        for (tag, junk) in [("garbage", "this is not json {{{"), ("noversion", r#"{"layouts":{}}"#), ("wrongshape", r#"{"version":1,"layouts":[1,2]}"#), ("array", "[1,2,3]"), ("zero", r#"{"version":0}"#)] {
            let dir = temp(tag);
            let path = dir.join("desktop-layout.json");
            std::fs::write(&path, junk).unwrap();
            let (mut s, outcome) = Store::load(&path);
            let LoadOutcome::Recovered { aside, reason } = outcome else { panic!("{tag}: {outcome:?}") };
            assert!(!reason.is_empty());
            let aside = aside.unwrap_or_else(|| panic!("{tag}: no aside path"));
            assert_eq!(std::fs::read_to_string(&aside).unwrap(), junk, "{tag}: the user's bytes are kept");
            assert!(aside.file_name().unwrap().to_string_lossy().starts_with("desktop-layout.json.bozuk-"));
            assert!(!path.exists(), "{tag}: the broken file no longer sits where the next save would overwrite it");
            assert_eq!(s, Store::default());
            // the fresh store can be saved, and the aside copy is still there afterwards
            s.set_show_icons(false);
            s.save(&path).unwrap();
            assert!(aside.exists());
            assert_eq!(Store::load(&path).1, LoadOutcome::Loaded);
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn a_file_from_a_newer_version_is_left_alone() {
        let dir = temp("newer");
        let path = dir.join("desktop-layout.json");
        let newer = r#"{ "version": 2, "something_new": { "layouts_v2": [] } }"#;
        std::fs::write(&path, newer).unwrap();
        let (mut s, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::Newer { found: 2 });
        assert_eq!(s.blocked(), Some(&Blocked::NewerVersion(2)));
        // using the empty store in memory is fine, but it can not be written over the newer file
        s.set_auto_arrange(true);
        assert!(matches!(s.save(&path), Err(SaveError::Blocked(Blocked::NewerVersion(2)))));
        assert!(matches!(s.save_if_dirty(&path), Err(SaveError::Blocked(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), newer, "byte for byte as the newer version left it");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1, "nothing renamed aside either");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unreadable_file_blocks_saving_it_does_not_count_as_empty() {
        // a folder where the file should be: reading it fails with something other than "not found"
        let dir = temp("unreadable");
        let path = dir.join("desktop-layout.json");
        std::fs::create_dir_all(&path).unwrap();
        let (mut s, outcome) = Store::load(&path);
        assert!(matches!(outcome, LoadOutcome::Unreadable { .. }), "{outcome:?}");
        assert!(matches!(s.blocked(), Some(Blocked::Unreadable(_))));
        assert!(matches!(s.save(&path), Err(SaveError::Blocked(_))));
        assert!(path.is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn setters_mark_dirty_only_on_a_real_change() {
        let mut s = Store::default();
        s.set_align_to_grid(true);
        s.set_auto_arrange(false);
        s.set_show_icons(true);
        s.set_sort(SortPref::default());
        assert!(!s.is_dirty());
        s.set_sort(SortPref { key: SortName::Size, descending: false });
        assert!(s.is_dirty());
        assert_eq!(s.sort.key.key(), SortKey::Size);
        assert_eq!(s.sort.dir(), SortDir::Asc);
        assert_eq!(Store { icon_size: 96, ..Store::default() }.icon_size_step(), IconSize::Large);
    }

    #[test]
    fn numbers_from_a_file_nobody_checked_are_brought_into_range() {
        let dir = temp("clock");
        let path = dir.join("desktop-layout.json");
        std::fs::write(&path, r#"{ "version": 1, "clock": 18446744073709551615, "layouts": { "m": { "1x1@100": { "items": { "u:a": { "col": 0, "row": 0, "at": 18446744073709551615 } }, "removed": { "u:b": { "col": 0, "row": 0, "at": 18446744073709551615, "gone_ms": 5 } } } } } }"#).unwrap();
        let (mut s, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(s.clock, CLOCK_LIMIT);
        assert_eq!(s.layouts["m"]["1x1@100"].items["u:a"].at, CLOCK_LIMIT, "no slot is newer than the counter");
        assert_eq!(s.layouts["m"]["1x1@100"].removed["u:b"].at, CLOCK_LIMIT);
        assert_eq!(s.tick(), CLOCK_LIMIT + 1, "and the next placement is newer than all of them");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn provisional_places_round_trip_and_are_left_out_when_empty() {
        let dir = temp("prov");
        let path = dir.join("desktop-layout.json");
        let mut s = Store { clock: 1, ..Store::default() };
        let mut l = MonitorLayout::default();
        l.items.insert("u:a".into(), Slot { col: 1, row: 2, dx: 0, dy: 0, at: 1 });
        s.layouts.entry("m".into()).or_default().insert("100x100@100".into(), l.clone());
        s.set_show_icons(false);
        s.save(&path).unwrap();
        assert!(!std::fs::read_to_string(&path).unwrap().contains("provisional"));
        l.provisional.insert("u:b".into(), Slot { col: 0, row: 0, dx: 0, dy: 0, at: 0 });
        s.layouts.get_mut("m").unwrap().insert("100x100@100".into(), l);
        s.set_show_icons(true);
        s.save(&path).unwrap();
        let (back, outcome) = Store::load(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(back, s);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
