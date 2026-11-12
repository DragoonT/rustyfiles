//! Sorting by tag, by path (the folder an item lies in) and by sync state: the part that lives
//! inside the folder view. The comparisons and the ranks are `rf_core::sortkeys`; the layout file's
//! reading is `crate::layouts`.
//!
//! - Sync needs nothing: the state is in the entry's attributes, so the plain comparison does it.
//! - Tag and Path need one rank per entry. They are made ONCE, on a worker thread, from what the
//!   view already holds (the tag store as it is in memory, the parent folders of search hits), and
//!   the sorted order is made there too. No comparison ever reads anything, and nothing here
//!   touches the disk. Until the ranks are in, the list keeps the order it had (a sort by name is
//!   used if a list has to be built meanwhile).
//! - With none of these keys chosen every hook starts with one check of the sort key: no thread,
//!   no allocation, no change to the sort cache.
//!
//! This is a child module of the folder view so that it can reach the view's private state; the
//! folder view itself only has small hooks.

use std::collections::HashMap;
use std::path::Path;

use rf_core::sortkeys::{self, NO_RANK};

use super::*;

/// The ranks of the entries for a key, for the entries generation they were made from.
struct Ranks {
    key: SortKey,
    entries_gen: u64,
    ranks: Arc<Vec<u32>>,
}

/// The job in flight: a result for anything else is thrown away.
struct Job {
    key: SortKey,
    entries_gen: u64,
    epoch: u64,
    _task: Task<()>,
}

#[derive(Default)]
pub(super) struct SortxState {
    ranks: Option<Ranks>,
    /// The tags changed after the ranks were made: they are made again (the old ones serve meanwhile).
    stale: bool,
    /// The ranks of the entries before the watcher replaced them, found again by name, so that the
    /// list keeps its order while the new ranks are made.
    carry: Option<HashMap<Arc<str>, u32>>,
    job: Option<Job>,
    epoch: u64,
}

/// Which of the added sort keys the menu and the headers offer here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SortOffers {
    pub tag: bool,
    pub path: bool,
    pub sync: bool,
}

/// Where the folder of each entry comes from.
pub(super) enum Folders {
    /// One folder for all of them (a plain listing).
    Same(PathBuf),
    /// Each entry's own (search, a tag's page, a library).
    Each(Vec<Arc<Path>>),
}

impl Folders {
    fn of(&self, i: usize) -> &Path {
        match self {
            Folders::Same(p) => p,
            Folders::Each(v) => v.get(i).map_or(Path::new(""), |p| p),
        }
    }
}

/// The tag store as the sort needs it: the lower-cased path of every tagged item → its tag rank.
/// Only items that have a tag the user's list knows are in it.
pub(super) fn tag_table(store: &rf_core::tags::TagStore) -> HashMap<String, u32> {
    let order: HashMap<u32, u32> = store.defs.iter().enumerate().map(|(i, d)| (d.id, i as u32)).collect();
    store
        .items
        .iter()
        .map(|(k, v)| (k.clone(), sortkeys::tag_rank(&v.tags, &|t| order.get(&t).copied())))
        .filter(|&(_, r)| r != NO_RANK)
        .collect()
}

/// [`tag_table`] for a plain folder: only the tagged items that lie DIRECTLY in `folder`. The
/// store's items are in a sorted map keyed by the lower-cased path, so these are one range of it
/// (a seek and the folder's own tagged items), not a pass over every tagged file on the machine;
/// this runs on the UI thread, before the work goes to a worker.
pub(super) fn tag_table_in(store: &rf_core::tags::TagStore, folder: &Path) -> HashMap<String, u32> {
    let mut prefix = folder.to_string_lossy().to_lowercase();
    if !prefix.ends_with('\\') {
        prefix.push('\\');
    }
    let order: HashMap<u32, u32> = store.defs.iter().enumerate().map(|(i, d)| (d.id, i as u32)).collect();
    store
        .items
        .range::<str, _>((std::ops::Bound::Included(prefix.as_str()), std::ops::Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(&prefix))
        // (deeper items belong to subfolders: no entry of this listing is one of them)
        .filter(|(k, _)| !k[prefix.len()..].contains('\\'))
        .map(|(k, v)| (k.clone(), sortkeys::tag_rank(&v.tags, &|t| order.get(&t).copied())))
        .filter(|&(_, r)| r != NO_RANK)
        .collect()
}

/// Whether the ranks of the key in force order the list now. A sort by a property column leaves
/// the fixed key (`Tag`, `Path`) underneath unchanged and keeps its own order in the same place
/// (the view's sort cache): while it is in force the ranks must not install theirs.
pub(super) fn ranks_order_the_list(key: SortKey, property_sort_active: bool) -> bool {
    key.needs_ranks() && !property_sort_active
}

/// The key a view keeps after it goes to another folder: a sort by path only means something where
/// results from several folders are listed, so it ends in a plain folder.
pub(super) fn key_after_reset(key: SortKey, results_from_many_folders: bool) -> SortKey {
    if key == SortKey::Path && !results_from_many_folders { SortKey::Name } else { key }
}

/// What the sort menu offers beyond the five fixed keys (see `FolderView::sort_offers`). A key that
/// is in force is always listed, so the menu can show it checked.
pub(super) fn offers_for(key: SortKey, is_search: bool, has_path: bool, sync_possible: bool) -> SortOffers {
    SortOffers {
        // anywhere the items have paths (a listing, search hits, a tag's page)
        tag: key == SortKey::Tag || is_search || has_path,
        // only where entries from several folders are listed together
        path: key == SortKey::Path || is_search,
        sync: key == SortKey::Sync || sync_possible,
    }
}

/// One rank per entry for `key` (Tag or Path); runs on a worker thread. Pure: it reads only what
/// it is given.
pub(super) fn make_ranks(key: SortKey, entries: &[Entry], folders: &Folders, tags: &HashMap<String, u32>) -> Vec<u32> {
    match key {
        SortKey::Tag => {
            // (nothing is tagged: no path is built at all)
            if tags.is_empty() {
                return vec![NO_RANK; entries.len()];
            }
            entries
                .iter()
                .enumerate()
                .map(|(i, e)| {
                    // the store's keys are the lower-cased path text: the same text is made here
                    let k = folders.of(i).join(&*e.name).to_string_lossy().to_lowercase();
                    tags.get(&k).copied().unwrap_or(NO_RANK)
                })
                .collect()
        }
        SortKey::Path => match folders {
            Folders::Same(_) => vec![0; entries.len()],
            Folders::Each(v) => {
                let text: Vec<std::borrow::Cow<'_, str>> = v.iter().map(|p| p.to_string_lossy()).collect();
                let mut ranks = sortkeys::rank_paths(text.iter().map(|c| &**c));
                ranks.resize(entries.len(), NO_RANK);
                ranks
            }
        },
        _ => Vec::new(),
    }
}

impl FolderView {
    /// The ranks the list can be sorted with right now: the fresh ones, or the old ones carried over
    /// a watcher refresh by name, or none.
    pub(super) fn sortx_ranks(&self) -> Option<Arc<Vec<u32>>> {
        let key = self.sort.key;
        if !self.sortx_wants() {
            return None;
        }
        match &self.sortx.ranks {
            Some(r) if r.key == key && r.entries_gen == self.entries_gen && r.ranks.len() == self.entries.len() => Some(r.ranks.clone()),
            _ => self.sortx.carry.as_ref().filter(|_| self.sortx.ranks.as_ref().is_some_and(|r| r.key == key)).map(|by_name| {
                Arc::new(self.entries.iter().map(|e| by_name.get(&e.name).copied().unwrap_or(NO_RANK)).collect())
            }),
        }
    }

    /// A rebuild of the list now would sort by name only because the ranks are not in yet, and a job
    /// is making them: callers skip the rebuild, the job's end makes it.
    pub(super) fn sortx_must_wait(&self) -> bool {
        self.sortx_wants() && self.sortx.job.is_some() && self.sortx_ranks().is_none()
    }

    /// The key in force is a ranked one and nothing else (a property column) orders the list.
    fn sortx_wants(&self) -> bool {
        ranks_order_the_list(self.sort.key, self.props.sort_active())
    }

    /// The ranks made for exactly the entries now in the list, if there are such: what a patch of
    /// the sorted order (the watcher) can start from.
    pub(super) fn sortx_fresh_ranks(&self) -> Option<Arc<Vec<u32>>> {
        let key = self.sort.key;
        if !self.sortx_wants() {
            return None;
        }
        self.sortx.ranks.as_ref().filter(|r| r.key == key && r.entries_gen == self.entries_gen && r.ranks.len() == self.entries.len()).map(|r| r.ranks.clone())
    }

    /// Makes the ranks the current sort key needs, if they are not there. Called whenever the key,
    /// the entries or the tags may have changed; a check of the key and nothing else when the key
    /// needs no ranks.
    pub(super) fn sortx_ensure(&mut self, cx: &mut Context<Self>) {
        let key = self.sort.key;
        if !key.needs_ranks() {
            if self.sortx.ranks.is_some() || self.sortx.job.is_some() || self.sortx.carry.is_some() {
                self.sortx_reset();
            }
            return;
        }
        // (a sort by a property column orders the list: the ranks are made when that ends)
        if !self.sortx_wants() {
            return;
        }
        // (the end of the listing asks again)
        if self.loading {
            return;
        }
        let fresh = self.sortx.ranks.as_ref().is_some_and(|r| r.key == key && r.entries_gen == self.entries_gen && r.ranks.len() == self.entries.len());
        if fresh && !self.sortx.stale {
            return;
        }
        if self.sortx.job.as_ref().is_some_and(|j| j.key == key && j.entries_gen == self.entries_gen && !self.sortx.stale) {
            return;
        }
        let folders = if !self.parents.is_empty() && self.parents.len() == self.entries.len() {
            Folders::Each(self.parents.clone())
        } else if let Some(p) = self.location().path() {
            Folders::Same(p.to_path_buf())
        } else {
            return; // a folder without paths (a phone): the list stays sorted by name
        };
        let tags = match (&folders, key) {
            (_, k) if k != SortKey::Tag => HashMap::new(),
            // a plain folder: its own tagged items only
            (Folders::Same(p), _) => tag_table_in(&cx.global::<crate::tags::Tags>().0, p),
            // results from several folders: any tagged item may be among them
            (Folders::Each(_), _) => tag_table(&cx.global::<crate::tags::Tags>().0),
        };
        let entries = self.entries.clone();
        let spec = self.sort;
        let entries_gen = self.entries_gen;
        self.sortx.epoch += 1;
        self.sortx.stale = false;
        let epoch = self.sortx.epoch;
        let work = cx.background_spawn(async move {
            let ranks = make_ranks(key, &entries, &folders, &tags);
            let mut order: Vec<u32> = (0..entries.len() as u32).collect();
            spec.sort_indices_ranked(&entries, &ranks, &mut order);
            (ranks, order)
        });
        let task = cx.spawn(async move |this, cx| {
            let (ranks, order) = work.await;
            let _ = this.update(cx, |v, cx| v.sortx_done(epoch, key, entries_gen, spec, ranks, order, cx));
        });
        self.sortx.job = Some(Job { key, entries_gen, epoch, _task: task });
    }

    #[allow(clippy::too_many_arguments)]
    fn sortx_done(&mut self, epoch: u64, key: SortKey, entries_gen: u64, spec: SortSpec, ranks: Vec<u32>, order: Vec<u32>, cx: &mut Context<Self>) {
        if self.sortx.job.as_ref().is_none_or(|j| j.epoch != epoch) {
            return; // an old job
        }
        self.sortx.job = None;
        if self.sort.key != key || self.entries_gen != entries_gen || ranks.len() != self.entries.len() {
            // the key or the entries moved on while it ran: once more for what is there now
            self.sortx_ensure(cx);
            return;
        }
        self.sortx.ranks = Some(Ranks { key, entries_gen, ranks: Arc::new(ranks) });
        self.sortx.carry = None;
        // a property sort took over while the job ran: its order stays, the ranks wait for its end
        if !self.sortx_wants() {
            return;
        }
        // (the order is only good for the spec it was made with; the direction may have changed since)
        if spec == self.sort {
            self.sorted_cache = Some((spec, entries_gen, Arc::new(order)));
        }
        self.rebuild_visible();
        cx.notify();
    }

    /// The entries are about to be replaced (the watcher): the ranks are remembered by name so that
    /// the list keeps its order until new ones are made.
    pub(super) fn sortx_before_swap(&mut self) {
        let key = self.sort.key;
        if !self.sortx_wants() {
            return;
        }
        // (only ranks made for THESE entries: ranks of an older listing would line up by accident)
        if let Some(r) = self.sortx.ranks.as_ref().filter(|r| r.key == key && r.entries_gen == self.entries_gen && r.ranks.len() == self.entries.len()) {
            self.sortx.carry = Some(self.entries.iter().zip(r.ranks.iter()).map(|(e, &rank)| (e.name.clone(), rank)).collect());
        }
    }

    /// A tag was given or taken (or the tag list changed): a sort by tag is made again.
    pub(super) fn sortx_tags_changed(&mut self, cx: &mut Context<Self>) {
        if self.sort.key != SortKey::Tag {
            return;
        }
        self.sortx.stale = true;
        self.sortx_ensure(cx);
    }

    /// A different folder is being opened: nothing of the old one goes on. A sort by path only means
    /// something where results from several folders are listed, so it ends when the view goes to a
    /// plain folder (it was chosen in a search or on a tag's page).
    pub(super) fn sortx_reset(&mut self) {
        self.sortx = SortxState::default();
        self.sort.key = key_after_reset(self.sort.key, self.is_search());
    }

    /// Whether the sync column can show here: switched on in Settings and a folder with cloud files
    /// (the same test `visible_cols` makes, without the width the pane has).
    fn sync_column_possible(&self, cx: &App) -> bool {
        const PINNED: u32 = sortkeys::ATTR_PINNED;
        if !cx.global::<crate::settings::Settings>().columns.sync || self.location().path().is_none() {
            return false;
        }
        match self.cloud_cache.get() {
            Some((g, any)) if g == self.entries_gen => any,
            _ => {
                let any = self.entries.iter().any(|e| e.is_cloud_placeholder() || e.attrs & PINNED != 0);
                self.cloud_cache.set(Some((self.entries_gen, any)));
                any
            }
        }
    }

    /// What the sort menu offers beyond the five fixed keys. A key that is in force is always
    /// listed, so the menu can show it checked.
    pub(crate) fn sort_offers(&self, cx: &App) -> SortOffers {
        let key = self.sort.key;
        // (the sync test reads the settings and walks the entries once per listing: asked only when it can matter)
        let sync_possible = key != SortKey::Sync && self.sync_column_possible(cx);
        offers_for(key, self.is_search(), self.location().path().is_some(), sync_possible)
    }

    /// A details header cell that sorts by `key` (the columns that did not sort before: tags, sync).
    pub(super) fn sortx_header(&self, id: &'static str, label: &'static str, width: f32, key: SortKey, p: &Palette, cx: &mut Context<Self>) -> Stateful<Div> {
        let active = self.sort.key == key && !self.props.sort_active();
        let arrow = if self.sort.dir == SortDir::Asc { L::ChevronUp } else { L::ChevronDown };
        h_flex()
            .id(id)
            .h_full()
            .px_2()
            .gap_1()
            .items_center()
            .w(px(width))
            .flex_shrink_0()
            .overflow_hidden()
            .rounded(theme::RADIUS_SM)
            .cursor_pointer()
            .hover(|s| s.bg(p.hover))
            .text_color(if active { p.text } else { p.text_secondary })
            .child(div().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label))
            .when(active, |d| d.child(Icon::new(arrow).size(px(12.)).flex_shrink_0().text_color(p.text_secondary)))
            .on_click(cx.listener(move |this, _, _, cx| this.set_sort(key, cx)))
            .relative()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use rf_core::sortkeys::NO_RANK;
    use rf_core::tags::{TagDef, TagStore, Tagged};
    use rf_core::{Entry, EntryKind, SortKey, SortSpec};

    // (not `use super::*`: that brings gpui's `test` attribute macro into scope and #[test] then loops)
    use super::{Folders, SortOffers, key_after_reset, make_ranks, offers_for, ranks_order_the_list, tag_table, tag_table_in};

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File, 1, 1, 1, 0)
    }

    fn store() -> TagStore {
        let mut s = TagStore { defs: Vec::new(), items: Default::default(), next_id: 10 };
        // the user's list: blue first, red second (ids are not positions)
        s.defs.push(TagDef { id: 7, name: "Mavi".into(), color: 1 });
        s.defs.push(TagDef { id: 3, name: "Kırmızı".into(), color: 2 });
        let mut add = |path: &str, tags: &[u32]| {
            s.items.insert(path.to_lowercase(), Tagged { path: path.into(), tags: tags.to_vec() });
        };
        add(r"C:\Docs\Report.txt", &[3]);
        add(r"C:\Docs\both.txt", &[3, 7]);
        add(r"C:\Docs\gone.txt", &[99]);
        s
    }

    #[test]
    fn the_tag_table_holds_the_lowest_position_of_each_tagged_path() {
        let t = tag_table(&store());
        assert_eq!(t.get(r"c:\docs\report.txt"), Some(&1));
        assert_eq!(t.get(r"c:\docs\both.txt"), Some(&0));
        // a tag the list does not know leaves the item out of the table (it counts as untagged)
        assert!(!t.contains_key(r"c:\docs\gone.txt"));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn a_property_sort_in_force_keeps_the_ranks_from_ordering_the_list() {
        // a sort by a property column leaves Tag or Path as the fixed key underneath and keeps its own
        // order in the view's sort cache: a finished tag or path job must not install its order then
        assert!(ranks_order_the_list(SortKey::Tag, false));
        assert!(ranks_order_the_list(SortKey::Path, false));
        assert!(!ranks_order_the_list(SortKey::Tag, true));
        assert!(!ranks_order_the_list(SortKey::Path, true));
        // keys that need no ranks never do, property sort or not
        for k in [SortKey::Name, SortKey::Modified, SortKey::Created, SortKey::Size, SortKey::Type, SortKey::Sync] {
            assert!(!ranks_order_the_list(k, false) && !ranks_order_the_list(k, true), "{k:?}");
        }
    }

    #[test]
    fn a_sort_by_path_ends_in_a_plain_folder_and_nothing_else_changes() {
        assert_eq!(key_after_reset(SortKey::Path, false), SortKey::Name);
        assert_eq!(key_after_reset(SortKey::Path, true), SortKey::Path);
        for k in [SortKey::Name, SortKey::Modified, SortKey::Created, SortKey::Size, SortKey::Type, SortKey::Tag, SortKey::Sync] {
            assert_eq!(key_after_reset(k, false), k);
            assert_eq!(key_after_reset(k, true), k);
        }
    }

    #[test]
    fn the_sort_menu_offers_tag_where_items_have_paths_path_only_in_results_and_sync_only_where_it_shows() {
        // a plain folder without cloud files
        assert_eq!(offers_for(SortKey::Name, false, true, false), SortOffers { tag: true, path: false, sync: false });
        // a plain folder with cloud files and the sync column on
        assert_eq!(offers_for(SortKey::Name, false, true, true), SortOffers { tag: true, path: false, sync: true });
        // search results: every folder
        assert_eq!(offers_for(SortKey::Name, true, false, false), SortOffers { tag: true, path: true, sync: false });
        // a folder with no paths (a phone, an archive listing): none of the three
        assert_eq!(offers_for(SortKey::Name, false, false, false), SortOffers { tag: false, path: false, sync: false });
        // a key in force is always listed so that the menu can show it checked
        assert!(offers_for(SortKey::Path, false, true, false).path);
        assert!(offers_for(SortKey::Sync, false, false, false).sync);
        assert!(offers_for(SortKey::Tag, false, false, false).tag);
    }

    #[test]
    fn the_table_of_a_plain_folder_holds_only_the_tagged_items_directly_in_it() {
        let mut s = store();
        let mut add = |path: &str| {
            s.items.insert(path.to_lowercase(), Tagged { path: path.into(), tags: vec![3] });
        };
        add(r"C:\Docs\Sub\deep.txt"); // a subfolder's item
        add(r"C:\DocsX\near.txt"); // a folder whose name starts the same
        add(r"D:\Docs\other.txt");
        add(r"C:\Doc\before.txt");
        let t = tag_table_in(&s, Path::new(r"C:\Docs"));
        let mut keys: Vec<&String> = t.keys().collect();
        keys.sort();
        // report.txt and both.txt (gone.txt has a tag the list does not know)
        assert_eq!(keys, vec![r"c:\docs\both.txt", r"c:\docs\report.txt"]);
        assert_eq!(t[r"c:\docs\both.txt"], 0);
        // a trailing separator and a drive root work the same
        assert_eq!(tag_table_in(&s, Path::new(r"C:\Docs\")), t);
        assert_eq!(tag_table_in(&s, Path::new(r"C:\")).len(), 0);
        let mut root = store();
        root.items.insert(r"c:\top.txt".into(), Tagged { path: r"C:\top.txt".into(), tags: vec![7] });
        assert_eq!(tag_table_in(&root, Path::new(r"C:\")).keys().collect::<Vec<_>>(), vec![r"c:\top.txt"]);
        // the plain folder's table and the whole table agree on every item in the folder
        for (k, v) in &t {
            assert_eq!(tag_table(&s)[k], *v);
        }
    }

    #[test]
    fn tag_ranks_are_found_by_the_lower_cased_path_of_each_entry() {
        let t = tag_table(&store());
        let es = vec![file("REPORT.txt"), file("other.txt"), file("both.txt"), file("gone.txt")];
        let r = make_ranks(SortKey::Tag, &es, &Folders::Same(PathBuf::from(r"C:\Docs")), &t);
        assert_eq!(r, vec![1, NO_RANK, 0, NO_RANK]);
        // search hits: each entry has its own folder
        let each = Folders::Each(vec![Path::new(r"C:\Docs").into(), Path::new(r"D:\Elsewhere").into(), Path::new(r"C:\Docs").into(), Path::new(r"C:\Docs").into()]);
        assert_eq!(make_ranks(SortKey::Tag, &es, &each, &t), vec![1, NO_RANK, 0, NO_RANK]);
    }

    #[test]
    fn with_nothing_tagged_no_path_is_built_and_every_rank_is_none() {
        let es = vec![file("a"), file("b")];
        let r = make_ranks(SortKey::Tag, &es, &Folders::Same(PathBuf::from(r"C:\x")), &HashMap::new());
        assert_eq!(r, vec![NO_RANK, NO_RANK]);
    }

    #[test]
    fn path_ranks_follow_the_folders_of_the_hits() {
        let es = vec![file("a"), file("b"), file("c")];
        let each = Folders::Each(vec![Path::new(r"C:\z").into(), Path::new(r"C:\a").into(), Path::new(r"C:\z").into()]);
        assert_eq!(make_ranks(SortKey::Path, &es, &each, &HashMap::new()), vec![1, 0, 1]);
        // a plain listing has one folder: one rank
        assert_eq!(make_ranks(SortKey::Path, &es, &Folders::Same(PathBuf::from(r"C:\q")), &HashMap::new()), vec![0, 0, 0]);
    }

    /// Not part of the normal run (timing depends on the machine): `cargo test --release -p rf-app
    /// --lib sort_ext -- --ignored --nocapture`. Makes the ranks of 100 000 entries the way the
    /// worker thread does, 1 000 of them tagged, and checks them.
    #[test]
    #[ignore]
    fn making_the_tag_ranks_of_100000_entries_is_one_pass() {
        let mut s = store();
        for i in 0..1000 {
            let p = format!(r"C:\Big\file{i:06}.txt");
            s.items.insert(p.to_lowercase(), Tagged { path: p.into(), tags: vec![3] });
        }
        let t0 = std::time::Instant::now();
        let table = tag_table(&s);
        let t1 = std::time::Instant::now();
        let es: Vec<Entry> = (0..100_000).map(|i| file(&format!("File{i:06}.txt"))).collect();
        let ranks = make_ranks(SortKey::Tag, &es, &Folders::Same(PathBuf::from(r"C:\Big")), &table);
        let t2 = std::time::Instant::now();
        println!("tag table of {} paths: {:?}; ranks of 100 000 entries: {:?}", table.len(), t1 - t0, t2 - t1);
        assert_eq!(ranks.iter().filter(|&&r| r != NO_RANK).count(), 1000);
    }

    #[test]
    fn a_sort_by_tag_made_from_the_table_puts_the_users_first_tag_first() {
        let t = tag_table(&store());
        let es = vec![file("zeta.txt"), file("report.txt"), file("both.txt"), file("alpha.txt")];
        let ranks = make_ranks(SortKey::Tag, &es, &Folders::Same(PathBuf::from(r"C:\Docs")), &t);
        let mut idx: Vec<u32> = (0..4).collect();
        SortSpec { key: SortKey::Tag, ..SortSpec::default() }.sort_indices_ranked(&es, &ranks, &mut idx);
        // both (blue), report (red), then the untagged by name
        assert_eq!(idx, vec![2, 1, 3, 0]);
    }
}
