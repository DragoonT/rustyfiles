//! The sort keys that need more than the entry's own fields: by tag, by the folder an item lies in
//! (its path) and by sync state. Everything here is pure; the values a comparison needs are made
//! ONCE before the sort (a batch read off the UI thread by the folder view) and handed over as one
//! `u32` rank per entry, so a comparison never reads anything.
//!
//! - Tag: the rank is the lowest position, in the user's tag list (Settings), among the tags an
//!   item has; no tag at all is [`NO_RANK`], which sorts last in both directions.
//! - Path: the rank is the place of the item's folder among all the folders present (compared
//!   component by component, digits as numbers), so a comparison is one integer compare.
//! - Sync: no side table, the state is in the entry's attributes ([`sync_rank`]).
//!
//! The per-folder layout file keeps the key in two places ([`SortKey::stored`]) so that a build
//! that does not know the new keys still reads the file: see `rf-app/src/layouts.rs`.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::entry::Entry;
use crate::sort::{SortKey, natural_cmp};

/// "No value": an item without a tag. Sorts after every real rank whatever the direction.
pub const NO_RANK: u32 = u32::MAX;

/// FILE_ATTRIBUTE_PINNED (OneDrive & co.: "always keep on this device").
pub const ATTR_PINNED: u32 = 0x0008_0000;

/// The states of the sync column, in the order a sort by it puts them (ascending): what is only in
/// the cloud first, what is here, what is kept here on purpose last.
pub const SYNC_ONLINE_ONLY: u8 = 0;
pub const SYNC_AVAILABLE: u8 = 1;
pub const SYNC_ALWAYS_HERE: u8 = 2;

/// The sync column's state of an entry: the same three cases the column draws.
pub fn sync_rank(e: &Entry) -> u8 {
    if e.is_cloud_placeholder() {
        SYNC_ONLINE_ONLY
    } else if e.attrs & ATTR_PINNED != 0 {
        SYNC_ALWAYS_HERE
    } else {
        SYNC_AVAILABLE
    }
}

/// The tag rank of an item: the lowest `order_of(tag)` among its tags (the position of the tag in
/// the user's list), [`NO_RANK`] when it has none. A tag the list no longer knows counts as none.
pub fn tag_rank(tags: &[u32], order_of: &dyn Fn(u32) -> Option<u32>) -> u32 {
    tags.iter().filter_map(|&t| order_of(t)).min().unwrap_or(NO_RANK)
}

/// Compares two folder paths component by component (`\` or `/`), each with the file manager's
/// natural order, so `C:\a\z` comes before `C:\a b` although ' ' sorts before '\\' as a character,
/// and a folder comes right before its subfolders.
pub fn path_cmp(a: &str, b: &str) -> Ordering {
    let mut ai = a.split(['\\', '/']).filter(|c| !c.is_empty());
    let mut bi = b.split(['\\', '/']).filter(|c| !c.is_empty());
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let o = natural_cmp(x, y);
                if o != Ordering::Equal {
                    return o;
                }
            }
        }
    }
}

/// One rank per input folder: the position of its folder among the distinct folders in
/// [`path_cmp`] order (equal folders share a rank, the first rank is 0). Costs one sort of the
/// DISTINCT folders, not of the items.
pub fn rank_paths<'a>(folders: impl IntoIterator<Item = &'a str>) -> Vec<u32> {
    // Windows paths ignore case, so spellings that differ only in case are ONE folder: they are
    // compared by `folder_key`. (With path_cmp's byte tie-break alone, another folder could sort
    // between two spellings and split the hits of one folder into two runs.)
    let keys: Vec<String> = folders.into_iter().map(folder_key).collect();
    let mut distinct: Vec<&str> = {
        let mut seen: HashMap<&str, ()> = HashMap::new();
        keys.iter().map(|k| k.as_str()).filter(|k| seen.insert(*k, ()).is_none()).collect()
    };
    distinct.sort_by(|x, y| path_cmp(x, y));
    // (two keys that still compare equal, such as "01" and "1", share a rank)
    let mut rank_of: HashMap<&str, u32> = HashMap::with_capacity(distinct.len());
    let mut next = 0u32;
    for (i, f) in distinct.iter().enumerate() {
        if i > 0 && path_cmp(distinct[i - 1], f) != Ordering::Equal {
            next += 1;
        }
        rank_of.insert(*f, next);
    }
    keys.iter().map(|k| rank_of[k.as_str()]).collect()
}

/// The folder as Windows sees it: components lower-cased and joined with one separator, so that
/// `C:\Users`, `c:\users\` and `C:/Users` are one folder.
fn folder_key(f: &str) -> String {
    let mut out = String::with_capacity(f.len());
    for c in f.split(['\\', '/']).filter(|c| !c.is_empty()) {
        if !out.is_empty() {
            out.push('\\');
        }
        out.push_str(&c.to_lowercase());
    }
    out
}

/// The sorted order of a ranked key (Tag, Path) after a few entries changed, without sorting again:
/// the old order with the entries that are gone taken out and the changed or new ones put in by
/// binary search (the watcher case). `fresh` is the new entry list, its first `kept` entries are
/// the old ones that stayed (`remap[old index]` = their new index, `u32::MAX` = gone) and the rest
/// are new or changed ones, whose rank `rank_of_new` gives (a changed file keeps the rank it had).
/// `old_ranks` has one rank per old entry. Costs O(n + k log n) for k changes.
pub fn patch_ranked(
    spec: &crate::sort::SortSpec,
    fresh: &[Entry],
    kept: usize,
    sorted_old: &[u32],
    remap: &[u32],
    old_ranks: &[u32],
    rank_of_new: impl Fn(&Entry) -> u32,
) -> Vec<u32> {
    let mut ranks = vec![NO_RANK; fresh.len()];
    for (old, &new) in remap.iter().enumerate() {
        if new != u32::MAX {
            ranks[new as usize] = old_ranks.get(old).copied().unwrap_or(NO_RANK);
        }
    }
    for j in kept..fresh.len() {
        ranks[j] = rank_of_new(&fresh[j]);
    }
    let mut v: Vec<u32> = sorted_old.iter().filter_map(|&i| remap.get(i as usize).copied().filter(|&j| j != u32::MAX)).collect();
    for j in kept..fresh.len() {
        let pos = v.partition_point(|&x| spec.compare_ranked(&fresh[x as usize], ranks[x as usize], &fresh[j], ranks[j]) == Ordering::Less);
        v.insert(pos, j as u32);
    }
    v
}

impl SortKey {
    /// Keys beyond the five a column of the file list always had. They are never written to the
    /// Settings file (see `make_layout_default`), and the layout file keeps them apart.
    pub fn is_extra(self) -> bool {
        matches!(self, SortKey::Tag | SortKey::Path | SortKey::Sync)
    }

    /// The key needs one rank per entry made before the sort ([`NO_RANK`], [`rank_paths`]).
    pub fn needs_ranks(self) -> bool {
        matches!(self, SortKey::Tag | SortKey::Path)
    }

    /// How a layout file stores the key: `(key an older build knows, name of the newer key)`. A
    /// newer key is written as `Name` (the order an older build can give) plus its own name, so
    /// the older build still parses the file and just sorts by name.
    pub fn stored(self) -> (SortKey, Option<&'static str>) {
        match self {
            SortKey::Tag => (SortKey::Name, Some("tag")),
            SortKey::Path => (SortKey::Name, Some("path")),
            SortKey::Sync => (SortKey::Name, Some("sync")),
            old => (old, None),
        }
    }

    /// The inverse of [`stored`](Self::stored). A name this build does not know (a layout written
    /// by a later build) leaves the key an older build would have read.
    pub fn from_stored(base: SortKey, extra: Option<&str>) -> SortKey {
        match extra {
            Some("tag") => SortKey::Tag,
            Some("path") => SortKey::Path,
            Some("sync") => SortKey::Sync,
            _ => base,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryKind, attrs};
    use crate::sort::{SortDir, SortSpec};

    fn file(name: &str, a: u32) -> Entry {
        Entry::new(name, EntryKind::File, 1, 1, 1, a)
    }

    #[test]
    fn sync_states_follow_the_columns_three_cases_in_the_stated_order() {
        let online = file("a", attrs::OFFLINE);
        let recall = file("b", attrs::RECALL_ON_DATA_ACCESS);
        let here = file("c", attrs::ARCHIVE);
        let pinned = file("d", attrs::ARCHIVE | ATTR_PINNED);
        // a placeholder that is also pinned is still only online (the column draws the cloud first)
        let both = file("e", attrs::OFFLINE | ATTR_PINNED);
        assert_eq!([&online, &recall, &here, &pinned, &both].map(sync_rank), [0, 0, 1, 2, 0]);
        let es = vec![pinned, here, online, recall];
        let mut idx: Vec<u32> = (0..4).collect();
        SortSpec { key: SortKey::Sync, ..SortSpec::default() }.sort_indices(&es, &mut idx);
        // online-only (a, b by name), available, always here
        assert_eq!(idx, vec![2, 3, 1, 0]);
        SortSpec { key: SortKey::Sync, dir: SortDir::Desc, ..SortSpec::default() }.sort_indices(&es, &mut idx);
        assert_eq!(idx, vec![0, 1, 3, 2]);
    }

    #[test]
    fn a_tag_rank_is_the_lowest_position_in_the_users_list() {
        let order = |t: u32| match t {
            10 => Some(0),
            20 => Some(1),
            30 => Some(2),
            _ => None,
        };
        assert_eq!(tag_rank(&[30, 20], &order), 1);
        assert_eq!(tag_rank(&[20, 10], &order), 0);
        assert_eq!(tag_rank(&[], &order), NO_RANK);
        // a tag that was deleted from the list counts as none
        assert_eq!(tag_rank(&[99], &order), NO_RANK);
        assert_eq!(tag_rank(&[99, 30], &order), 2);
    }

    #[test]
    fn tagged_items_come_first_by_tag_order_then_name_and_untagged_stay_last_both_ways() {
        let es = vec![
            file("untagged-b", 0),  // 0
            file("blue-a", 0),      // 1
            file("red-z", 0),       // 2
            file("red-a", 0),       // 3
            file("untagged-a", 0),  // 4
            Entry::new("a-dir", EntryKind::Dir, 0, 1, 1, 0), // 5, red
        ];
        // positions in the user's list: red 0, blue 1
        let ranks = vec![NO_RANK, 1, 0, 0, NO_RANK, 0];
        let mut idx: Vec<u32> = (0..6).collect();
        let asc = SortSpec { key: SortKey::Tag, ..SortSpec::default() };
        asc.sort_indices_ranked(&es, &ranks, &mut idx);
        // folders first (the default), red by name, blue, then untagged by name
        assert_eq!(idx, vec![5, 3, 2, 1, 4, 0]);
        let desc = SortSpec { dir: SortDir::Desc, ..asc };
        desc.sort_indices_ranked(&es, &ranks, &mut idx);
        // direction flips the tag order and the names, the untagged are STILL last
        assert_eq!(idx, vec![5, 1, 2, 3, 0, 4]);
        // no folder preference: the folder is just a red item
        let mixed = SortSpec { dirs_first: false, ..asc };
        mixed.sort_indices_ranked(&es, &ranks, &mut idx);
        assert_eq!(idx, vec![5, 3, 2, 1, 4, 0]);
    }

    #[test]
    fn a_rank_vector_that_is_too_short_does_not_panic_and_counts_as_none() {
        let es = vec![file("b", 0), file("a", 0), file("c", 0)];
        let mut idx: Vec<u32> = (0..3).collect();
        SortSpec { key: SortKey::Tag, ..SortSpec::default() }.sort_indices_ranked(&es, &[0], &mut idx);
        // entry 0 has rank 0, the others (no rank given) are untagged
        assert_eq!(idx, vec![0, 1, 2]);
    }

    #[test]
    fn folder_paths_compare_component_by_component() {
        assert_eq!(path_cmp(r"C:\a\z", r"C:\a b"), Ordering::Less);
        assert_eq!(path_cmp(r"C:\a", r"C:\a\b"), Ordering::Less);
        assert_eq!(path_cmp(r"C:\d2", r"C:\d10"), Ordering::Less);
        // spellings that differ only in case are different but stay next to each other, both ways round
        assert_ne!(path_cmp(r"C:\A", r"c:\a"), Ordering::Equal);
        assert_eq!(path_cmp(r"C:\A", r"c:\a"), path_cmp(r"c:\a", r"C:\A").reverse());
        assert_eq!(path_cmp(r"C:\x\", r"C:\x"), Ordering::Equal);
        assert_eq!(path_cmp("C:/x/y", r"C:\x\y"), Ordering::Equal);
    }

    #[test]
    fn paths_are_ranked_by_folder_and_equal_folders_share_a_rank() {
        let folders = [r"D:\b", r"C:\z", r"C:\a\sub", r"D:\b", r"C:\a", r"C:\z"];
        // distinct, in order: C: 0, C:\sub 1, C:\z 2, D: 3
        assert_eq!(rank_paths(folders), vec![3, 2, 1, 3, 0, 2]);
        assert!(rank_paths(std::iter::empty::<&str>()).is_empty());
    }

    #[test]
    fn a_sort_by_path_groups_by_folder_then_name() {
        let es = vec![file("b.txt", 0), file("a.txt", 0), file("z.txt", 0), file("a.txt", 0)];
        let folders = [r"C:\two", r"C:\two", r"C:\one", r"C:\one"];
        let ranks = rank_paths(folders);
        let mut idx: Vec<u32> = (0..4).collect();
        let spec = SortSpec { key: SortKey::Path, ..SortSpec::default() };
        spec.sort_indices_ranked(&es, &ranks, &mut idx);
        // one: a.txt(3), z.txt(2); two: a.txt(1), b.txt(0)
        assert_eq!(idx, vec![3, 2, 1, 0]);
        let desc = SortSpec { dir: SortDir::Desc, ..spec };
        desc.sort_indices_ranked(&es, &ranks, &mut idx);
        assert_eq!(idx, vec![0, 1, 2, 3]);
    }

    #[test]
    fn spellings_of_one_folder_that_differ_in_case_share_a_rank_and_stay_one_block() {
        // Windows paths ignore case: a root typed as c:\users and one returned as C:\Users are one folder,
        // and a different folder that sorts between the two spellings must not split them
        let folders = [r"C:\Users\Foo", r"C:\Users\Zed", r"c:\users\foo"];
        assert_eq!(rank_paths(folders), vec![0, 1, 0]);
        let es = vec![file("b.txt", 0), file("a.txt", 0), file("c.txt", 0)];
        let ranks = rank_paths(folders);
        let mut idx: Vec<u32> = (0..3).collect();
        SortSpec { key: SortKey::Path, ..SortSpec::default() }.sort_indices_ranked(&es, &ranks, &mut idx);
        // both hits of the one folder (b.txt, c.txt by name), then the other folder's a.txt
        assert_eq!(idx, vec![0, 2, 1]);
    }

    #[test]
    fn a_sort_by_path_keeps_each_folders_hits_together_with_folders_first_only_inside_a_folder() {
        let dir = |n: &str| Entry::new(n, EntryKind::Dir, 0, 1, 1, 0);
        // x is a directory hit in C:\two, a a file hit in C:\one, y a directory hit in C:\one
        let es = vec![dir("x"), file("a", 0), dir("y"), file("b", 0)];
        let folders = [r"C:\two", r"C:\one", r"C:\one", r"C:\two"];
        let ranks = rank_paths(folders);
        let mut idx: Vec<u32> = (0..4).collect();
        let spec = SortSpec { key: SortKey::Path, ..SortSpec::default() };
        spec.sort_indices_ranked(&es, &ranks, &mut idx);
        // one: y (directory first), a; two: x (directory first), b. Not "all directories, then all files".
        assert_eq!(idx, vec![2, 1, 0, 3]);
        // descending flips the folders and the names but a directory still leads inside its folder
        let desc = SortSpec { dir: SortDir::Desc, ..spec };
        desc.sort_indices_ranked(&es, &ranks, &mut idx);
        assert_eq!(idx, vec![0, 3, 2, 1]);
        // folders not preferred: the folder, then the name alone
        let mixed = SortSpec { dirs_first: false, ..spec };
        mixed.sort_indices_ranked(&es, &ranks, &mut idx);
        assert_eq!(idx, vec![1, 2, 3, 0]);
    }

    #[test]
    fn patching_a_ranked_order_removes_the_gone_and_places_the_new_by_their_rank() {
        // old entries: a(tag 0), b(untagged), c(tag 1), d(untagged); sorted: a c b d
        let old_ranks = vec![0, NO_RANK, 1, NO_RANK];
        let old_sorted = vec![0u32, 2, 1, 3];
        // b is gone and c was changed (it comes back at the end, carrying its rank); e is new.
        // fresh = a, d, e, c; kept = 2 (a, d)
        let fresh = vec![file("a", 0), file("d", 0), file("e", 0), file("c", 0)];
        let remap = vec![0, u32::MAX, u32::MAX, 1];
        let spec = SortSpec { key: SortKey::Tag, ..SortSpec::default() };
        let v = patch_ranked(&spec, &fresh, 2, &old_sorted, &remap, &old_ranks, |e| if &*e.name == "c" { 1 } else { NO_RANK });
        // a (0), c (1), then the untagged by name: d, e
        assert_eq!(v, vec![0, 3, 1, 2]);
        // the result is what a full sort with the same ranks gives
        let ranks = vec![0, NO_RANK, NO_RANK, 1];
        let mut full: Vec<u32> = (0..4).collect();
        spec.sort_indices_ranked(&fresh, &ranks, &mut full);
        assert_eq!(v, full);
    }

    #[test]
    fn the_new_keys_are_stored_as_name_plus_their_own_name_and_read_back() {
        for k in [SortKey::Tag, SortKey::Path, SortKey::Sync] {
            let (base, extra) = k.stored();
            assert_eq!(base, SortKey::Name, "an older build must be able to read {k:?}");
            assert_eq!(SortKey::from_stored(base, extra), k);
            assert!(k.is_extra());
        }
        for k in [SortKey::Name, SortKey::Modified, SortKey::Created, SortKey::Size, SortKey::Type] {
            assert_eq!(k.stored(), (k, None));
            assert_eq!(SortKey::from_stored(k, None), k);
            assert!(!k.is_extra());
        }
        // a name from a later build: the key an older build would have read
        assert_eq!(SortKey::from_stored(SortKey::Size, Some("owner")), SortKey::Size);
        assert!(SortKey::Tag.needs_ranks() && SortKey::Path.needs_ranks());
        assert!(!SortKey::Sync.needs_ranks() && !SortKey::Name.needs_ranks());
    }
}
