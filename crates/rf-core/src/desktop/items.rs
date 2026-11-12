//! The desktop's items as plain data (shell plan F2): the user's Desktop folder and the Public one merged, the virtual items the user
//! has enabled, the "show hidden" rules, and the diff that turns two snapshots into added / removed / renamed / changed. The folders are
//! read by `rf_win::desktop::items`; this file never touches the disk.
//!
//! IDENTITY. An item is named by where it comes from, because the same file name can exist in both folders and then shows twice (that
//! is what Explorer does): `u:<name>` for the user's Desktop, `p:<name>` for the Public one, `::{CLSID}` for a virtual item. The name
//! is lower-cased (Windows compares file names without case), so renaming `Foo` to `foo` keeps the item, and with it its cell.
//!
//! RENAMES. A folder watcher says which names changed, not which one became which. A removed name and an added name in the same
//! folder are one rename when exactly one of each has the same kind, size, modification time and attributes (a rename changes none of
//! them); with two candidates for one fingerprint nothing is guessed and they stay a removal and an addition.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::entry::{Entry, EntryKind, Filter, attrs};
use crate::sort::{SortDir, SortKey, SortSpec, natural_cmp};

use super::layout::OwnedListing;
use super::virtuals::VirtualKind;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ItemId(String);

impl ItemId {
    pub fn user(name: &str) -> Self {
        ItemId(format!("u:{}", name.to_lowercase()))
    }
    pub fn public(name: &str) -> Self {
        ItemId(format!("p:{}", name.to_lowercase()))
    }
    pub fn virtual_item(k: VirtualKind) -> Self {
        ItemId(k.parsing_name())
    }
    pub fn file(origin: FolderOrigin, name: &str) -> Self {
        match origin {
            FolderOrigin::User => Self::user(name),
            FolderOrigin::Public => Self::public(name),
        }
    }
    /// An id read back from the layout file.
    pub fn from_stored(s: &str) -> Self {
        ItemId(s.to_string())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn origin(&self) -> Option<Origin> {
        if self.0.starts_with("u:") {
            Some(Origin::User)
        } else if self.0.starts_with("p:") {
            Some(Origin::Public)
        } else {
            VirtualKind::from_clsid(&self.0).map(Origin::Virtual)
        }
    }
}

impl std::fmt::Display for ItemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The two folders on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FolderOrigin {
    User,
    Public,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Origin {
    User,
    Public,
    Virtual(VirtualKind),
}

/// One file or folder as the listing of a Desktop folder gives it. Its content is never read (a OneDrive placeholder would be
/// downloaded by that); the attributes say whether it is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopFile {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: i64,
    pub attrs: u32,
}

impl DesktopFile {
    pub fn from_entry(e: &Entry) -> Self {
        DesktopFile { name: e.name.to_string(), is_dir: e.is_dir(), size: e.size, modified_ms: e.modified_ms, attrs: e.attrs }
    }
    fn to_entry(&self) -> Entry {
        Entry::new(&self.name, if self.is_dir { EntryKind::Dir } else { EntryKind::File }, self.size, self.modified_ms, 0, self.attrs)
    }
}

/// The app's "show hidden" settings, used exactly as the file lists use them (`rf_core::Filter`): hidden items are shown dimmed when
/// `show_hidden` is on; hidden items that are also system files (desktop.ini) need `show_system` as well; names starting with a dot
/// count as hidden when `dot_hidden` is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Visibility {
    pub show_hidden: bool,
    pub show_system: bool,
    pub dot_hidden: bool,
}

impl Visibility {
    fn accepts(&self, f: &DesktopFile) -> bool {
        Filter { show_hidden: self.show_hidden, show_system: self.show_system, dot_hidden: self.dot_hidden, ..Default::default() }.accepts(&f.to_entry())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopItem {
    pub id: ItemId,
    pub origin: Origin,
    /// The file name, or for a virtual item its label (`VirtualKind::english_name` until the shell's own is filled in).
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: i64,
    pub attrs: u32,
    /// Hidden by attribute but shown because the setting says so: drawn faded.
    pub dimmed: bool,
    /// The other Desktop folder has an item with this name too.
    pub same_name_elsewhere: bool,
    /// An online-only placeholder: use the icon, never read the content.
    pub cloud_placeholder: bool,
    /// The Recycle Bin: has something in it.
    pub recycle_full: Option<bool>,
}

impl DesktopItem {
    pub fn is_virtual(&self) -> bool {
        matches!(self.origin, Origin::Virtual(_))
    }
    fn to_entry(&self) -> Entry {
        Entry::new(&self.name, if self.is_dir { EntryKind::Dir } else { EntryKind::File }, self.size, self.modified_ms, 0, self.attrs)
    }
    fn fingerprint(&self) -> (bool, u64, i64, u32) {
        (self.is_dir, self.size, self.modified_ms, self.attrs)
    }
}

/// Everything the model needs to know about the virtual items.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct VirtualState {
    pub shown: Vec<VirtualKind>,
    pub recycle_full: bool,
    /// "Show desktop icons" is off, or a policy removes the desktop: no items at all.
    pub disabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemChange {
    Added(ItemId),
    Removed(ItemId),
    Renamed { from: ItemId, to: ItemId },
    /// Same item, something about it differs (size, time, attributes, the Recycle Bin's state, the case of its name).
    Changed(ItemId),
}

#[derive(Debug, Clone, Default)]
pub struct DesktopModel {
    vis: Visibility,
    user: BTreeMap<String, DesktopFile>,
    public: BTreeMap<String, DesktopFile>,
    virt: VirtualState,
    /// Which folders have been listed successfully at least once. A folder that never was holds nothing because nothing is known,
    /// not because it is empty.
    listed: (bool, bool),
}

impl DesktopModel {
    pub fn new(vis: Visibility) -> Self {
        DesktopModel { vis, ..Default::default() }
    }

    /// Whether a full listing of this folder has arrived.
    pub fn listed(&self, o: FolderOrigin) -> bool {
        match o {
            FolderOrigin::User => self.listed.0,
            FolderOrigin::Public => self.listed.1,
        }
    }

    /// What the layout needs to know (see `layout::Listing`): the items drawn, the ones that exist but are not drawn (hidden by the
    /// "show hidden" rules, virtual items that are off, everything when the desktop is switched off), and whether both folders have
    /// been listed. Pass this to `Store::resolve`, never `items()` alone: the list of what is drawn says nothing about what is gone.
    pub fn listing(&self) -> OwnedListing {
        let shown: Vec<ItemId> = self.items().into_iter().map(|i| i.id).collect();
        let on: std::collections::BTreeSet<&ItemId> = shown.iter().collect();
        let mut kept: Vec<ItemId> = Vec::new();
        for k in VirtualKind::ALL {
            let id = ItemId::virtual_item(k);
            if !on.contains(&id) {
                kept.push(id);
            }
        }
        for (origin, files) in [(FolderOrigin::User, &self.user), (FolderOrigin::Public, &self.public)] {
            for f in files.values() {
                let id = ItemId::file(origin, &f.name);
                if !on.contains(&id) {
                    kept.push(id);
                }
            }
        }
        OwnedListing { shown, kept, complete: self.listed.0 && self.listed.1 }
    }

    fn folder(&mut self, o: FolderOrigin) -> &mut BTreeMap<String, DesktopFile> {
        match o {
            FolderOrigin::User => &mut self.user,
            FolderOrigin::Public => &mut self.public,
        }
    }

    /// The visible items: the enabled virtual items first (in `VirtualKind::ALL` order), then the user's Desktop, then the Public one,
    /// each in natural name order with folders first. A name in both folders is two items.
    pub fn items(&self) -> Vec<DesktopItem> {
        if self.virt.disabled {
            return Vec::new();
        }
        let mut out = Vec::new();
        for k in VirtualKind::ALL {
            if self.virt.shown.contains(&k) {
                out.push(DesktopItem {
                    id: ItemId::virtual_item(k),
                    origin: Origin::Virtual(k),
                    name: k.english_name().to_string(),
                    is_dir: true,
                    size: 0,
                    modified_ms: 0,
                    attrs: 0,
                    dimmed: false,
                    same_name_elsewhere: false,
                    cloud_placeholder: false,
                    recycle_full: (k == VirtualKind::RecycleBin).then_some(self.virt.recycle_full),
                });
            }
        }
        for (origin, files, other) in [(FolderOrigin::User, &self.user, &self.public), (FolderOrigin::Public, &self.public, &self.user)] {
            let mut shown: Vec<&DesktopFile> = files.values().filter(|f| self.vis.accepts(f)).collect();
            shown.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| natural_cmp(&a.name, &b.name)));
            for f in shown {
                out.push(DesktopItem {
                    id: ItemId::file(origin, &f.name),
                    origin: match origin {
                        FolderOrigin::User => Origin::User,
                        FolderOrigin::Public => Origin::Public,
                    },
                    name: f.name.clone(),
                    is_dir: f.is_dir,
                    size: f.size,
                    modified_ms: f.modified_ms,
                    attrs: f.attrs,
                    dimmed: f.attrs & attrs::HIDDEN != 0,
                    same_name_elsewhere: other.contains_key(&f.name.to_lowercase()),
                    cloud_placeholder: f.attrs & (attrs::OFFLINE | attrs::RECALL_ON_OPEN | attrs::RECALL_ON_DATA_ACCESS) != 0,
                    recycle_full: None,
                });
            }
        }
        out
    }

    fn changed_by(&mut self, f: impl FnOnce(&mut Self)) -> Vec<ItemChange> {
        let before = self.items();
        f(self);
        diff(&before, &self.items())
    }

    /// A full listing of one folder (start-up, or the watcher said "list it again").
    pub fn set_files(&mut self, origin: FolderOrigin, files: Vec<DesktopFile>) -> Vec<ItemChange> {
        self.changed_by(|m| {
            match origin {
                FolderOrigin::User => m.listed.0 = true,
                FolderOrigin::Public => m.listed.1 = true,
            }
            let map = m.folder(origin);
            map.clear();
            for f in files {
                map.insert(f.name.to_lowercase(), f);
            }
        })
    }

    /// The answer to "these names changed": each name with what it is now (`None`: it is gone).
    pub fn apply_stats(&mut self, origin: FolderOrigin, stats: Vec<(String, Option<DesktopFile>)>) -> Vec<ItemChange> {
        self.changed_by(|m| {
            let map = m.folder(origin);
            for (name, now) in stats {
                match now {
                    Some(f) => {
                        map.insert(f.name.to_lowercase(), f);
                    }
                    None => {
                        map.remove(&name.to_lowercase());
                    }
                }
            }
        })
    }

    pub fn set_virtuals(&mut self, v: VirtualState) -> Vec<ItemChange> {
        self.changed_by(|m| m.virt = v)
    }

    pub fn set_visibility(&mut self, vis: Visibility) -> Vec<ItemChange> {
        self.changed_by(|m| m.vis = vis)
    }

    /// The names the model knows in a folder (for the watcher to re-check against the disk).
    pub fn known_names(&self, origin: FolderOrigin) -> Vec<String> {
        match origin {
            FolderOrigin::User => self.user.values().map(|f| f.name.clone()).collect(),
            FolderOrigin::Public => self.public.values().map(|f| f.name.clone()).collect(),
        }
    }
}

/// What differs between two snapshots, renames paired up as described at the top of this file. Sorted: renames, removals, additions,
/// changes, each by id.
pub fn diff(before: &[DesktopItem], after: &[DesktopItem]) -> Vec<ItemChange> {
    let b: BTreeMap<&ItemId, &DesktopItem> = before.iter().map(|i| (&i.id, i)).collect();
    let a: BTreeMap<&ItemId, &DesktopItem> = after.iter().map(|i| (&i.id, i)).collect();
    let mut removed: Vec<&DesktopItem> = b.iter().filter(|(id, _)| !a.contains_key(*id)).map(|(_, i)| *i).collect();
    let mut added: Vec<&DesktopItem> = a.iter().filter(|(id, _)| !b.contains_key(*id)).map(|(_, i)| *i).collect();
    let changed: Vec<ItemChange> = a.iter().filter(|(id, i)| b.get(*id).is_some_and(|old| old != *i)).map(|(id, _)| ItemChange::Changed((*id).clone())).collect();

    let mut out = Vec::new();
    // pair renames: same folder, a fingerprint that exactly one removed and exactly one added item have
    let mut paired_from = Vec::new();
    for r in &removed {
        if r.is_virtual() {
            continue;
        }
        let same_r = removed.iter().filter(|x| x.origin == r.origin && x.fingerprint() == r.fingerprint()).count();
        let cands: Vec<&&DesktopItem> = added.iter().filter(|x| x.origin == r.origin && x.fingerprint() == r.fingerprint()).collect();
        if same_r == 1 && cands.len() == 1 {
            let to = cands[0].id.clone();
            // the added item must not be wanted by another removed one: guaranteed by same_r == 1 and equal fingerprints
            out.push(ItemChange::Renamed { from: r.id.clone(), to: to.clone() });
            paired_from.push((r.id.clone(), to));
        }
    }
    removed.retain(|r| !paired_from.iter().any(|(f, _)| *f == r.id));
    added.retain(|x| !paired_from.iter().any(|(_, t)| *t == x.id));
    out.extend(removed.into_iter().map(|i| ItemChange::Removed(i.id.clone())));
    out.extend(added.into_iter().map(|i| ItemChange::Added(i.id.clone())));
    out.extend(changed);
    out
}

/// The desktop's sort commands. Enabled virtual items stay at the front in their fixed order whatever the key; folders come before
/// files; ties fall back to the natural name order, and the user's folder before the Public one.
pub fn sort_items(items: &mut [DesktopItem], key: SortKey, dir: SortDir) {
    let spec = SortSpec { key, dir, dirs_first: true, files_first: false };
    items.sort_by(|a, b| match (a.origin, b.origin) {
        (Origin::Virtual(x), Origin::Virtual(y)) => x.cmp(&y),
        (Origin::Virtual(_), _) => std::cmp::Ordering::Less,
        (_, Origin::Virtual(_)) => std::cmp::Ordering::Greater,
        _ => spec.compare(&a.to_entry(), &b.to_entry()).then_with(|| origin_rank(a.origin).cmp(&origin_rank(b.origin))),
    });
}

fn origin_rank(o: Origin) -> u8 {
    match o {
        Origin::Virtual(_) => 0,
        Origin::User => 1,
        Origin::Public => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> DesktopFile {
        DesktopFile { name: name.into(), is_dir: false, size: name.len() as u64 * 10, modified_ms: 1000 + name.len() as i64, attrs: attrs::ARCHIVE }
    }
    fn dir(name: &str) -> DesktopFile {
        DesktopFile { name: name.into(), is_dir: true, size: 0, modified_ms: 5, attrs: attrs::DIRECTORY }
    }
    fn hidden(name: &str, extra: u32) -> DesktopFile {
        DesktopFile { attrs: attrs::HIDDEN | extra, ..f(name) }
    }
    fn names(m: &DesktopModel) -> Vec<String> {
        m.items().into_iter().map(|i| i.id.to_string()).collect()
    }

    #[test]
    fn identity_includes_the_folder_and_ignores_case() {
        assert_eq!(ItemId::user("Foo.TXT"), ItemId::user("foo.txt"));
        assert_ne!(ItemId::user("a.txt"), ItemId::public("a.txt"));
        assert_eq!(ItemId::user("\u{130}şlem.txt").as_str(), "u:i\u{307}şlem.txt");
        assert_eq!(ItemId::virtual_item(VirtualKind::ThisPc).as_str(), "::{20D04FE0-3AEA-1069-A2D8-08002B30309D}");
        assert_eq!(ItemId::user("x").origin(), Some(Origin::User));
        assert_eq!(ItemId::public("x").origin(), Some(Origin::Public));
        assert_eq!(ItemId::virtual_item(VirtualKind::RecycleBin).origin(), Some(Origin::Virtual(VirtualKind::RecycleBin)));
        assert_eq!(ItemId::from_stored("garbage").origin(), None);
    }

    #[test]
    fn the_two_folders_merge_and_a_shared_name_shows_twice() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("Notes.txt"), dir("Projects"), f("shared.lnk")]);
        m.set_files(FolderOrigin::Public, vec![f("Zoom.lnk"), f("SHARED.lnk")]);
        let items = m.items();
        assert_eq!(names(&m), ["u:projects", "u:notes.txt", "u:shared.lnk", "p:shared.lnk", "p:zoom.lnk"]);
        let shared: Vec<_> = items.iter().filter(|i| i.name.eq_ignore_ascii_case("shared.lnk")).collect();
        assert_eq!(shared.len(), 2);
        assert!(shared.iter().all(|i| i.same_name_elsewhere));
        assert!(!items.iter().find(|i| i.name == "Notes.txt").unwrap().same_name_elsewhere);
        assert_eq!(items[3].name, "SHARED.lnk", "each keeps the case it has on disk");
    }

    #[test]
    fn natural_order_within_a_folder() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("file10.txt"), f("file2.txt"), f("File1.txt"), dir("zeta"), dir("Alpha")]);
        assert_eq!(names(&m), ["u:alpha", "u:zeta", "u:file1.txt", "u:file2.txt", "u:file10.txt"]);
    }

    #[test]
    fn hidden_and_system_files_follow_the_settings() {
        let files = vec![f("a.txt"), hidden("secret.txt", 0), hidden("desktop.ini", attrs::SYSTEM), f(".gitignore")];
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, files);
        assert_eq!(names(&m), ["u:.gitignore", "u:a.txt"], "hidden and system are out by default");
        // show hidden: the hidden file comes in, dimmed; desktop.ini is hidden AND system, so it stays out
        m.set_visibility(Visibility { show_hidden: true, ..Default::default() });
        assert_eq!(names(&m), ["u:.gitignore", "u:a.txt", "u:secret.txt"]);
        assert!(m.items().iter().find(|i| i.name == "secret.txt").unwrap().dimmed);
        assert!(!m.items().iter().find(|i| i.name == "a.txt").unwrap().dimmed);
        // show system as well: desktop.ini appears
        m.set_visibility(Visibility { show_hidden: true, show_system: true, dot_hidden: false });
        assert_eq!(names(&m), ["u:.gitignore", "u:a.txt", "u:desktop.ini", "u:secret.txt"]);
        // dot files count as hidden when asked to
        m.set_visibility(Visibility { show_hidden: false, show_system: false, dot_hidden: true });
        assert_eq!(names(&m), ["u:a.txt"]);
    }

    #[test]
    fn visibility_is_the_same_rule_as_the_file_lists() {
        let samples = [f("a"), hidden("h", 0), hidden("s", attrs::SYSTEM), f(".d"), DesktopFile { attrs: attrs::SYSTEM, ..f("sys") }];
        for sh in [false, true] {
            for ss in [false, true] {
                for dot in [false, true] {
                    let v = Visibility { show_hidden: sh, show_system: ss, dot_hidden: dot };
                    let flt = Filter { show_hidden: sh, show_system: ss, dot_hidden: dot, ..Default::default() };
                    for s in &samples {
                        assert_eq!(v.accepts(s), flt.accepts(&s.to_entry()), "{sh} {ss} {dot} {}", s.name);
                    }
                }
            }
        }
    }

    #[test]
    fn virtual_items_come_first_in_a_fixed_order() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("a.txt")]);
        m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin, VirtualKind::ThisPc], recycle_full: true, disabled: false });
        let items = m.items();
        assert_eq!(items[0].id.as_str(), VirtualKind::ThisPc.parsing_name());
        assert_eq!(items[1].origin, Origin::Virtual(VirtualKind::RecycleBin));
        assert_eq!(items[1].recycle_full, Some(true));
        assert_eq!(items[0].recycle_full, None);
        assert_eq!(items.len(), 3);
        // the Recycle Bin empties: that is a change of the same item
        let ch = m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin, VirtualKind::ThisPc], recycle_full: false, disabled: false });
        assert_eq!(ch, vec![ItemChange::Changed(ItemId::virtual_item(VirtualKind::RecycleBin))]);
        // the user turns This PC off in Settings
        let ch = m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin], recycle_full: false, disabled: false });
        assert_eq!(ch, vec![ItemChange::Removed(ItemId::virtual_item(VirtualKind::ThisPc))]);
        // "Show desktop icons" off: nothing, then everything back
        let ch = m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin], recycle_full: false, disabled: true });
        assert_eq!(ch.len(), 2);
        assert!(m.items().is_empty());
        m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin], recycle_full: false, disabled: false });
        assert_eq!(m.items().len(), 2);
    }

    #[test]
    fn cloud_placeholders_are_flagged() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![DesktopFile { attrs: attrs::RECALL_ON_DATA_ACCESS, ..f("online.docx") }, f("local.docx")]);
        let items = m.items();
        assert!(items.iter().find(|i| i.name == "online.docx").unwrap().cloud_placeholder);
        assert!(!items.iter().find(|i| i.name == "local.docx").unwrap().cloud_placeholder);
    }

    #[test]
    fn a_watcher_burst_becomes_added_removed_and_changed() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("keep.txt"), f("edit.txt"), f("gone.txt")]);
        let mut edited = f("edit.txt");
        edited.size += 1;
        edited.modified_ms += 1;
        let ch = m.apply_stats(
            FolderOrigin::User,
            vec![("new-file.txt".into(), Some(f("new-file.txt"))), ("edit.txt".into(), Some(edited)), ("gone.txt".into(), None), ("never-existed.txt".into(), None)],
        );
        assert_eq!(
            ch,
            vec![ItemChange::Removed(ItemId::user("gone.txt")), ItemChange::Added(ItemId::user("new-file.txt")), ItemChange::Changed(ItemId::user("edit.txt"))]
        );
        assert_eq!(m.known_names(FolderOrigin::User).len(), 3);
    }

    #[test]
    fn a_rename_is_a_removal_and_an_addition_with_the_same_fingerprint() {
        let mut m = DesktopModel::new(Visibility::default());
        let old = DesktopFile { name: "old.txt".into(), size: 77, modified_ms: 5000, ..f("x") };
        let new = DesktopFile { name: "Brand New.txt".into(), ..old.clone() };
        m.set_files(FolderOrigin::User, vec![old.clone(), f("other.txt")]);
        let ch = m.apply_stats(FolderOrigin::User, vec![("old.txt".into(), None), ("Brand New.txt".into(), Some(new.clone()))]);
        assert_eq!(ch, vec![ItemChange::Renamed { from: ItemId::user("old.txt"), to: ItemId::user("brand new.txt") }]);
        // a folder renamed (size 0, same time)
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::Public, vec![dir("Old Dir")]);
        let ch = m.set_files(FolderOrigin::Public, vec![dir("New Dir")]);
        assert_eq!(ch, vec![ItemChange::Renamed { from: ItemId::public("old dir"), to: ItemId::public("new dir") }]);
    }

    #[test]
    fn renames_are_not_guessed_when_ambiguous_or_across_folders() {
        // two identical-looking files renamed at once: nothing is paired
        let twin = |n: &str| DesktopFile { name: n.into(), size: 9, modified_ms: 9, ..f("x") };
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![twin("a1"), twin("a2")]);
        let ch = m.set_files(FolderOrigin::User, vec![twin("b1"), twin("b2")]);
        assert_eq!(ch.len(), 4);
        assert!(ch.iter().all(|c| !matches!(c, ItemChange::Renamed { .. })));
        // moving a file from the user's folder to the Public one is not a rename
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("mv.txt")]);
        m.set_files(FolderOrigin::Public, vec![]);
        let before = m.items();
        m.set_files(FolderOrigin::User, vec![]);
        m.set_files(FolderOrigin::Public, vec![f("mv.txt")]);
        let ch = diff(&before, &m.items());
        assert_eq!(ch, vec![ItemChange::Removed(ItemId::user("mv.txt")), ItemChange::Added(ItemId::public("mv.txt"))]);
        // the same file appearing in the second folder while staying in the first
        assert!(m.items().iter().all(|i| !i.same_name_elsewhere));
    }

    #[test]
    fn a_case_only_rename_is_a_change_of_the_same_item() {
        let mut m = DesktopModel::new(Visibility::default());
        m.set_files(FolderOrigin::User, vec![f("report.txt")]);
        let ch = m.apply_stats(FolderOrigin::User, vec![("Report.txt".into(), Some(DesktopFile { name: "Report.txt".into(), ..f("report.txt") }))]);
        assert_eq!(ch, vec![ItemChange::Changed(ItemId::user("report.txt"))]);
        assert_eq!(m.items()[0].name, "Report.txt");
    }

    #[test]
    fn hiding_by_setting_reads_as_removed_and_added() {
        let mut m = DesktopModel::new(Visibility { show_hidden: true, ..Default::default() });
        m.set_files(FolderOrigin::User, vec![hidden("h.txt", 0), f("v.txt")]);
        let ch = m.set_visibility(Visibility::default());
        assert_eq!(ch, vec![ItemChange::Removed(ItemId::user("h.txt"))]);
        let ch = m.set_visibility(Visibility { show_hidden: true, ..Default::default() });
        assert_eq!(ch, vec![ItemChange::Added(ItemId::user("h.txt"))]);
    }

    #[test]
    fn sort_commands() {
        let mut m = DesktopModel::new(Visibility::default());
        let mk = |n: &str, size: u64, t: i64| DesktopFile { name: n.into(), size, modified_ms: t, ..f(n) };
        m.set_files(FolderOrigin::User, vec![mk("b.txt", 5, 300), mk("a.png", 50, 100), dir("Zdir"), mk("c.txt", 1, 200)]);
        m.set_files(FolderOrigin::Public, vec![mk("a.txt", 5, 300)]);
        m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin, VirtualKind::ThisPc], ..Default::default() });
        let order = |key, dir| {
            let mut it = m.items();
            sort_items(&mut it, key, dir);
            it.into_iter().map(|i| i.name).collect::<Vec<_>>()
        };
        // virtual items stay in front in every sort, folders before files
        assert_eq!(order(SortKey::Name, SortDir::Asc), ["This PC", "Recycle Bin", "Zdir", "a.png", "a.txt", "b.txt", "c.txt"].map(String::from));
        assert_eq!(order(SortKey::Name, SortDir::Desc), ["This PC", "Recycle Bin", "Zdir", "c.txt", "b.txt", "a.txt", "a.png"].map(String::from));
        assert_eq!(order(SortKey::Size, SortDir::Asc)[2..], ["Zdir", "c.txt", "a.txt", "b.txt", "a.png"].map(String::from));
        assert_eq!(order(SortKey::Modified, SortDir::Desc)[2..], ["Zdir", "b.txt", "a.txt", "c.txt", "a.png"].map(String::from));
        assert_eq!(order(SortKey::Type, SortDir::Asc)[2..], ["Zdir", "a.png", "a.txt", "b.txt", "c.txt"].map(String::from));
    }

    #[test]
    fn the_listing_tells_hidden_and_switched_off_from_gone_and_unknown() {
        let mut m = DesktopModel::new(Visibility::default());
        // nothing has been listed: nothing is known, so nothing may be forgotten
        let l = m.listing();
        assert!(!l.complete && l.shown.is_empty());
        assert_eq!(l.kept.len(), VirtualKind::ALL.len(), "the virtual items are known to exist, they are just not shown");
        m.set_files(FolderOrigin::User, vec![f("a.txt"), hidden("h.txt", 0)]);
        assert!(!m.listing().complete, "the Public folder has not been listed");
        m.set_files(FolderOrigin::Public, vec![f("p.lnk")]);
        m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin], ..Default::default() });
        let l = m.listing();
        assert!(l.complete);
        assert_eq!(l.shown, [ItemId::virtual_item(VirtualKind::RecycleBin), ItemId::user("a.txt"), ItemId::public("p.lnk")]);
        assert!(l.kept.contains(&ItemId::user("h.txt")), "hidden by the setting: kept");
        assert!(l.kept.contains(&ItemId::virtual_item(VirtualKind::ThisPc)), "switched off: kept");
        assert!(!l.kept.contains(&ItemId::user("a.txt")));
        // show hidden: the file moves from kept to shown
        m.set_visibility(Visibility { show_hidden: true, ..Default::default() });
        let l = m.listing();
        assert!(l.shown.contains(&ItemId::user("h.txt")) && !l.kept.contains(&ItemId::user("h.txt")));
        // "show desktop icons" off: nothing is drawn and everything is kept
        m.set_virtuals(VirtualState { shown: vec![VirtualKind::RecycleBin], disabled: true, ..Default::default() });
        let l = m.listing();
        assert!(l.shown.is_empty() && l.complete);
        assert_eq!(l.kept.len(), VirtualKind::ALL.len() + 3);
    }
}
