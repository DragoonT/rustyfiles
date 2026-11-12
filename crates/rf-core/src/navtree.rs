//! The folder tree of the navigation pane as a pure model: nodes, the expanded set, the flat list of
//! visible rows with their depth, what a folder listing or a change event does to the tree, the
//! rule that opens the branch to the open folder, and the keyboard.
//!
//! Nothing here touches the disk, a clock or a window. The caller (the sidebar's tree view) asks the
//! model what to list ([`LoadRequest`]), lists it off the UI thread, and hands the answer back; the
//! time (`now_ms`, any monotonic millisecond counter) is passed in wherever a rule depends on it.
//!
//! The expanded set lives in the model only, so it is kept for the session and lost with the process.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::sort::natural_cmp;

pub type NodeId = u32;

/// A first listing that takes longer than this shows a spinner at the node.
pub const SLOW_LOAD_MS: u64 = 300;
/// A branch the user closed by hand stays closed for this long against the follow rule.
pub const COLLAPSE_GRACE_MS: u64 = 30_000;
/// At most this many folders are watched for changes at once; the rest are listed again when opened.
pub const MAX_WATCHES: usize = 32;
/// Above this many nodes, closing a branch frees what other closed branches hold (oldest opened first). Open branches and the
/// branch of the open folder are never freed; a freed branch is listed again when it is opened.
pub const NODE_BUDGET: usize = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootKind {
    Profile,
    Fixed,
    Removable,
    Network,
    Optical,
}

#[derive(Clone, Debug)]
pub struct RootSpec {
    pub path: PathBuf,
    pub label: String,
    pub kind: RootKind,
}

/// One folder of a listing. Files are not tree nodes: the caller leaves them out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirItem {
    pub name: String,
    /// The localized name Explorer shows ("Downloads" is "İndirilenler"); sorting uses it too.
    pub label: Option<String>,
    pub hidden: bool,
    pub system: bool,
}

/// The same visibility rules as the file list ([`crate::entry::Filter`]), for folders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TreeFilter {
    pub show_hidden: bool,
    pub show_system: bool,
    /// Names starting with "." count as hidden.
    pub dot_hidden: bool,
}

impl TreeFilter {
    fn accepts(&self, name: &str, hidden: bool, system: bool) -> bool {
        if !self.show_hidden && (hidden || (self.dot_hidden && name.starts_with('.'))) {
            return false;
        }
        if !self.show_system && system && hidden {
            return false;
        }
        true
    }
}

/// Ask: list this folder and give the answer back with the same `id` and `ver`. An answer whose
/// `ver` is not the node's current one (the node was listed again since, or freed) is dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoadRequest {
    pub id: NodeId,
    pub ver: u32,
    pub path: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListFailure {
    /// The folder is gone: its node goes away with it.
    NotFound,
    /// Anything else (no access, drive not ready…), in words for the error row.
    Other(String),
}

/// What the watcher found out about one name in a watched folder.
#[derive(Clone, Debug)]
pub struct Change {
    pub name: String,
    /// The folder as it is now, or `None` when the name is gone or is a file.
    pub item: Option<DirItem>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Load {
    Unloaded,
    Loading(u64),
    Loaded,
    Failed(Fail),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Fail {
    NotFound,
    Other(String),
}

struct Node {
    live: bool,
    key: String,
    path: PathBuf,
    name: String,
    label: Option<String>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    expanded: bool,
    load: Load,
    hidden: bool,
    system: bool,
    root: Option<RootKind>,
    ver: u32,
    /// When the user closed it by hand (the follow rule leaves it closed for a while).
    user_collapsed_ms: Option<u64>,
    /// Order of the last opening: the newest are watched first.
    seq: u64,
    /// A change was reported while a listing of this node was still on its way: list once more after it.
    rescan_after: bool,
    /// A listing requested for this node (its current `ver`) has not been answered yet.
    inflight: bool,
}

impl Node {
    fn shown(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.name)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Node,
    /// The quiet line under an open node whose listing failed.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Row {
    pub id: NodeId,
    pub depth: u16,
    pub kind: RowKind,
}

/// What a row shows.
pub struct NodeInfo<'a> {
    pub path: &'a Path,
    pub label: &'a str,
    pub expanded: bool,
    /// Shows a chevron: the folder may have folders (not listed yet) or has some.
    pub can_expand: bool,
    pub loading: bool,
    /// Loading for longer than [`SLOW_LOAD_MS`].
    pub slow: bool,
    pub selected: bool,
    pub cursor: bool,
    pub root: Option<RootKind>,
    pub hidden: bool,
    /// The failure text of an Error row (`None` for a node that did not fail).
    pub error: Option<&'a str>,
    pub not_found: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeKey {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    Enter,
}

#[derive(Debug, Default)]
pub struct KeyResult {
    pub loads: Vec<LoadRequest>,
    pub navigate: Option<(NodeId, PathBuf)>,
    /// The cursor moved to this row: scroll it into view.
    pub cursor_row: Option<usize>,
    pub changed: bool,
}

#[derive(Default)]
pub struct NavTree {
    nodes: Vec<Node>,
    free: Vec<NodeId>,
    roots: Vec<NodeId>,
    /// Several nodes can stand for one folder (the profile folder is a root and also lies under its drive).
    index: HashMap<String, Vec<NodeId>>,
    rows: Vec<Row>,
    filter: TreeFilter,
    next_gen: u32,
    expand_seq: u64,
    selected: Option<NodeId>,
    selected_key: Option<String>,
    /// The key the follow rule is still on its way to (waiting for listings).
    reveal: Option<String>,
    /// The walk may open closed nodes and list unlisted ones (the follow setting); off, it only marks what is already in the tree.
    reveal_opens: bool,
    scroll_pending: bool,
    cursor: Option<NodeId>,
    /// Row position of every node that has a row (`u32::MAX`: none), rebuilt with the rows, so that finding a node's row is not a scan.
    row_of: Vec<u32>,
    /// Tests lower the node budget to see the freeing work on a small tree.
    budget_override: Option<usize>,
}

/// The key a path is compared by: lower case, backslashes, no verbatim prefix, no trailing
/// separator (a drive root keeps one: `c:\`).
pub fn norm(p: &Path) -> String {
    let s = p.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").filter(|r| !r.starts_with("UNC\\")).unwrap_or(&s);
    let t = s.replace('/', "\\").to_lowercase();
    let t = t.trim_end_matches('\\');
    if t.len() == 2 && t.ends_with(':') {
        format!("{t}\\")
    } else {
        t.to_string()
    }
}

fn child_key(parent_key: &str, name: &str) -> String {
    format!("{}\\{}", parent_key.trim_end_matches('\\'), name.to_lowercase())
}

/// Is `key` the folder `root` or somewhere below it?
fn is_under(root: &str, key: &str) -> bool {
    let r = root.trim_end_matches('\\');
    key == root || (key.len() > r.len() && key.starts_with(r) && key.as_bytes()[r.len()] == b'\\')
}

/// Which rows of `n` are built, given where the view is in the tree: `into_tree` pixels of the tree are above the view, the view
/// is `view_h` tall. Rows `overscan` above and below the view are built too. Before the view is measured (`view_h <= 1`) the
/// first `first_frame_rows` are built.
pub fn visible_range(n: usize, into_tree: f32, view_h: f32, row_h: f32, overscan: usize, first_frame_rows: usize) -> (usize, usize) {
    if view_h <= 1. {
        return (0, n.min(first_frame_rows));
    }
    let first = ((into_tree / row_h).floor().max(0.) as usize).saturating_sub(overscan).min(n);
    let last = (((into_tree + view_h) / row_h).ceil().max(0.) as usize + overscan).min(n);
    (first.min(last), last)
}

/// The scroll position (pixels from the top of the content) that shows a row whose top is `row_top`, given the current position
/// `cur`. `centre`: the row goes to the upper third unless it is comfortably in view already. Otherwise: the least movement that
/// shows it.
pub fn scroll_goal(cur: f32, view_h: f32, row_top: f32, row_h: f32, centre: bool) -> f32 {
    if centre {
        if row_top >= cur + row_h && row_top + 2. * row_h <= cur + view_h { cur } else { (row_top - view_h / 3.).max(0.) }
    } else if row_top < cur + 4. {
        (row_top - 4.).max(0.)
    } else if row_top + row_h > cur + view_h - 4. {
        (row_top + row_h - view_h + 4.).max(0.)
    } else {
        cur
    }
}

impl NavTree {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    pub fn filter(&self) -> TreeFilter {
        self.filter
    }

    pub fn set_filter(&mut self, f: TreeFilter) -> bool {
        if f == self.filter {
            return false;
        }
        self.filter = f;
        self.rebuild();
        true
    }

    fn live(&self, id: NodeId) -> bool {
        self.nodes.get(id as usize).is_some_and(|n| n.live)
    }

    fn bump_gen(&mut self) -> u32 {
        self.next_gen = self.next_gen.wrapping_add(1);
        self.next_gen
    }

    fn alloc(&mut self, key: String, path: PathBuf, name: String, parent: Option<NodeId>) -> NodeId {
        let ver = self.bump_gen();
        let node = Node {
            live: true,
            key: key.clone(),
            path,
            name,
            label: None,
            parent,
            children: Vec::new(),
            expanded: false,
            load: Load::Unloaded,
            hidden: false,
            system: false,
            root: None,
            ver,
            user_collapsed_ms: None,
            seq: 0,
            rescan_after: false,
            inflight: false,
        };
        let id = match self.free.pop() {
            Some(id) => {
                self.nodes[id as usize] = node;
                id
            }
            None => {
                self.nodes.push(node);
                (self.nodes.len() - 1) as NodeId
            }
        };
        self.index.entry(key).or_default().push(id);
        id
    }

    /// Frees a node and everything below it. The node must already be out of its parent's list.
    fn free_subtree(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            if !self.live(n) {
                continue;
            }
            let (key, kids) = {
                let node = &mut self.nodes[n as usize];
                node.live = false;
                (std::mem::take(&mut node.key), std::mem::take(&mut node.children))
            };
            if let Some(v) = self.index.get_mut(&key) {
                v.retain(|&x| x != n);
                if v.is_empty() {
                    self.index.remove(&key);
                }
            }
            if self.selected == Some(n) {
                self.selected = None;
            }
            if self.cursor == Some(n) {
                self.cursor = None;
            }
            self.free.push(n);
            stack.extend(kids);
        }
    }

    fn sort_children(&mut self, id: NodeId) {
        let mut kids = std::mem::take(&mut self.nodes[id as usize].children);
        kids.sort_by(|&a, &b| natural_cmp(self.nodes[a as usize].shown(), self.nodes[b as usize].shown()));
        self.nodes[id as usize].children = kids;
    }

    /// Replaces the roots (the profile folder first, then the drives). A root that stays keeps
    /// whatever was open below it.
    pub fn set_roots(&mut self, specs: Vec<RootSpec>, now_ms: u64) -> Vec<LoadRequest> {
        let mut next: Vec<NodeId> = Vec::new();
        let mut seen = HashSet::new();
        for spec in specs {
            let key = norm(&spec.path);
            if !seen.insert(key.clone()) {
                continue;
            }
            let existing = self.roots.iter().copied().find(|&r| self.nodes[r as usize].key == key);
            let id = match existing {
                Some(id) => id,
                None => {
                    let name = spec.path.to_string_lossy().trim_end_matches('\\').to_string();
                    self.alloc(key, spec.path.clone(), name, None)
                }
            };
            let n = &mut self.nodes[id as usize];
            n.label = Some(spec.label);
            n.root = Some(spec.kind);
            next.push(id);
        }
        let old = std::mem::take(&mut self.roots);
        for r in old {
            if !next.contains(&r) {
                self.free_subtree(r);
            }
        }
        self.roots = next;
        self.rebuild();
        self.pump(now_ms)
    }

    pub fn root_count(&self) -> usize {
        self.roots.len()
    }

    fn visible(&self, id: NodeId) -> bool {
        let n = &self.nodes[id as usize];
        n.root.is_some() || self.filter.accepts(&n.name, n.hidden, n.system)
    }

    fn rebuild(&mut self) {
        let mut rows = Vec::with_capacity(self.rows.len().max(64));
        let mut stack: Vec<(NodeId, u16)> = self.roots.iter().rev().map(|&r| (r, 0)).collect();
        while let Some((id, depth)) = stack.pop() {
            rows.push(Row { id, depth, kind: RowKind::Node });
            let n = &self.nodes[id as usize];
            if !n.expanded {
                continue;
            }
            if matches!(n.load, Load::Failed(_)) {
                rows.push(Row { id, depth: depth + 1, kind: RowKind::Error });
                continue;
            }
            for &c in n.children.iter().rev() {
                if self.visible(c) {
                    stack.push((c, depth + 1));
                }
            }
        }
        self.row_of.clear();
        self.row_of.resize(self.nodes.len(), u32::MAX);
        for (i, r) in rows.iter().enumerate() {
            if r.kind == RowKind::Node {
                self.row_of[r.id as usize] = i as u32;
            }
        }
        self.rows = rows;
        if let Some(c) = self.cursor
            && !self.live(c)
        {
            self.cursor = None;
        }
    }

    /// The row of a node, if it has one. O(1): this runs for the cursor and the open folder at every frame.
    pub fn row_index(&self, id: NodeId) -> Option<usize> {
        let p = *self.row_of.get(id as usize)? as usize;
        self.rows.get(p).filter(|r| r.id == id && r.kind == RowKind::Node).map(|_| p)
    }

    pub fn info(&self, row: &Row, now_ms: u64) -> NodeInfo<'_> {
        let n = &self.nodes[row.id as usize];
        let (error, not_found) = match (&n.load, row.kind) {
            (Load::Failed(Fail::Other(m)), RowKind::Error) => (Some(m.as_str()), false),
            (Load::Failed(Fail::NotFound), RowKind::Error) => (Some(""), true),
            _ => (None, false),
        };
        NodeInfo {
            path: &n.path,
            label: n.shown(),
            expanded: n.expanded,
            can_expand: self.can_expand(row.id),
            loading: matches!(n.load, Load::Loading(_)),
            slow: matches!(n.load, Load::Loading(t) if now_ms.saturating_sub(t) >= SLOW_LOAD_MS),
            selected: self.selected == Some(row.id),
            cursor: self.cursor == Some(row.id),
            root: n.root,
            hidden: n.hidden,
            error,
            not_found,
        }
    }

    pub fn path_of(&self, id: NodeId) -> Option<&Path> {
        self.live(id).then(|| self.nodes[id as usize].path.as_path())
    }

    /// Does the node get a chevron? Folders not listed yet might have folders, so they do.
    pub fn can_expand(&self, id: NodeId) -> bool {
        let n = &self.nodes[id as usize];
        match n.load {
            Load::Loaded => n.children.iter().any(|&c| self.visible(c)),
            _ => true,
        }
    }

    /// When a node is Loading, the moment it started (for the caller's "wake up when it turns slow" timer).
    pub fn loading_since(&self, id: NodeId) -> Option<u64> {
        match self.nodes.get(id as usize).map(|n| &n.load) {
            Some(Load::Loading(t)) => Some(*t),
            _ => None,
        }
    }

    pub fn any_loading(&self) -> bool {
        self.rows.iter().any(|r| r.kind == RowKind::Node && matches!(self.nodes[r.id as usize].load, Load::Loading(_)))
    }

    fn request(&mut self, id: NodeId) -> LoadRequest {
        let ver = self.bump_gen();
        let n = &mut self.nodes[id as usize];
        n.ver = ver;
        n.inflight = true;
        // this listing starts now: it sees everything that happened before it
        n.rescan_after = false;
        LoadRequest { id, ver, path: n.path.clone() }
    }

    /// Opens a node: the first time (or after a failure) it is listed with a spinner; one that has
    /// been listed before shows what it has and is listed again quietly, together with the open
    /// folders below it, because nothing watched them while the branch was closed.
    fn expand_node(&mut self, id: NodeId, now_ms: u64, by_user: bool) -> Vec<LoadRequest> {
        let was = self.nodes[id as usize].expanded;
        self.expand_seq += 1;
        let seq = self.expand_seq;
        {
            let n = &mut self.nodes[id as usize];
            n.expanded = true;
            n.seq = seq;
            if by_user {
                n.user_collapsed_ms = None;
            }
        }
        let mut reqs = Vec::new();
        match self.nodes[id as usize].load {
            Load::Unloaded | Load::Failed(_) => {
                self.nodes[id as usize].load = Load::Loading(now_ms);
                reqs.push(self.request(id));
            }
            Load::Loading(_) => {}
            Load::Loaded => {
                if !was {
                    reqs.push(self.request(id));
                    let mut stack: Vec<NodeId> = self.nodes[id as usize].children.clone();
                    while let Some(c) = stack.pop() {
                        let (open, loaded) = {
                            let n = &self.nodes[c as usize];
                            (n.expanded, n.load == Load::Loaded)
                        };
                        if open && loaded {
                            reqs.push(self.request(c));
                            stack.extend(self.nodes[c as usize].children.iter().copied());
                        }
                    }
                }
            }
        }
        reqs
    }

    fn collapse_node(&mut self, id: NodeId, now_ms: u64, by_user: bool) {
        let n = &mut self.nodes[id as usize];
        n.expanded = false;
        if by_user {
            n.user_collapsed_ms = Some(now_ms);
        }
        if let Some(c) = self.cursor
            && c != id
            && self.is_below(c, id)
        {
            self.cursor = Some(id);
        }
    }

    fn is_below(&self, mut node: NodeId, ancestor: NodeId) -> bool {
        while let Some(p) = self.nodes[node as usize].parent {
            if p == ancestor {
                return true;
            }
            node = p;
        }
        false
    }

    /// The user opens or closes a node (chevron click).
    pub fn toggle(&mut self, id: NodeId, now_ms: u64) -> Vec<LoadRequest> {
        if !self.live(id) {
            return Vec::new();
        }
        let reqs = if self.nodes[id as usize].expanded {
            self.collapse_node(id, now_ms, true);
            self.trim();
            Vec::new()
        } else {
            self.expand_node(id, now_ms, true)
        };
        self.rebuild();
        reqs
    }

    /// List a failed node again (the error row was clicked).
    pub fn retry(&mut self, id: NodeId, now_ms: u64) -> Vec<LoadRequest> {
        if !self.live(id) || !matches!(self.nodes[id as usize].load, Load::Failed(_)) {
            return Vec::new();
        }
        self.nodes[id as usize].load = Load::Loading(now_ms);
        let r = self.request(id);
        self.rebuild();
        vec![r]
    }

    /// The answer to a [`LoadRequest`]. Returns the listings the answer makes necessary (the next step of the follow rule).
    pub fn apply_listing(&mut self, req: &LoadRequest, result: Result<Vec<DirItem>, ListFailure>, now_ms: u64) -> Vec<LoadRequest> {
        let id = req.id;
        if !self.live(id) || self.nodes[id as usize].ver != req.ver {
            return Vec::new();
        }
        self.nodes[id as usize].inflight = false;
        let mut reqs = Vec::new();
        match result {
            Err(ListFailure::NotFound) => match self.nodes[id as usize].parent {
                Some(p) => {
                    self.nodes[p as usize].children.retain(|&c| c != id);
                    self.free_subtree(id);
                }
                None => self.nodes[id as usize].load = Load::Failed(Fail::NotFound),
            },
            Err(ListFailure::Other(m)) => {
                // a quiet refresh that fails leaves what is shown; a first listing shows the error
                if self.nodes[id as usize].load != Load::Loaded {
                    self.nodes[id as usize].load = Load::Failed(Fail::Other(m));
                }
                // no second try on its own: a failing folder must not be listed in a loop
                self.nodes[id as usize].rescan_after = false;
            }
            Ok(items) => {
                self.merge(id, items);
                self.nodes[id as usize].load = Load::Loaded;
                if std::mem::take(&mut self.nodes[id as usize].rescan_after) {
                    reqs.push(self.request(id));
                }
            }
        }
        self.rebuild();
        reqs.extend(self.pump(now_ms));
        reqs
    }

    fn apply_item(&mut self, id: NodeId, it: &DirItem) {
        let n = &mut self.nodes[id as usize];
        if n.name != it.name {
            // the disk spells the name differently now (case only): keep what the disk says
            n.path.set_file_name(&it.name);
            n.name = it.name.clone();
            self.repath_below(id);
        }
        let n = &mut self.nodes[id as usize];
        n.label = it.label.clone();
        n.hidden = it.hidden;
        n.system = it.system;
    }

    /// A folder's name changed its spelling: the paths of everything below it follow (the keys are lower case and stay).
    fn repath_below(&mut self, id: NodeId) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let base = self.nodes[n as usize].path.clone();
            let kids = self.nodes[n as usize].children.clone();
            for c in kids {
                let name = self.nodes[c as usize].name.clone();
                self.nodes[c as usize].path = base.join(name);
                stack.push(c);
            }
        }
    }

    fn add_child(&mut self, parent: NodeId, it: &DirItem) -> NodeId {
        let key = child_key(&self.nodes[parent as usize].key, &it.name);
        let path = self.nodes[parent as usize].path.join(&it.name);
        let id = self.alloc(key, path, it.name.clone(), Some(parent));
        self.apply_item(id, it);
        id
    }

    /// Makes the children of `id` the given items: the ones that were there keep their state
    /// (open, listed), new ones appear, missing ones go with everything below them.
    fn merge(&mut self, id: NodeId, items: Vec<DirItem>) {
        let old = std::mem::take(&mut self.nodes[id as usize].children);
        let mut by_name: HashMap<String, NodeId> = old.iter().map(|&c| (self.nodes[c as usize].name.to_lowercase(), c)).collect();
        let mut kids = Vec::with_capacity(items.len());
        let mut seen = HashSet::new();
        for it in &items {
            let lk = it.name.to_lowercase();
            if !seen.insert(lk.clone()) {
                continue;
            }
            match by_name.remove(&lk) {
                Some(c) => {
                    self.apply_item(c, it);
                    kids.push(c);
                }
                None => {
                    let c = self.add_child(id, it);
                    kids.push(c);
                }
            }
        }
        for (_, gone) in by_name {
            self.free_subtree(gone);
        }
        self.nodes[id as usize].children = kids;
        self.sort_children(id);
    }

    /// What the watcher saw in folder `dir`. Returns whether the rows changed.
    pub fn apply_changes(&mut self, dir: &Path, changes: Vec<Change>) -> bool {
        let Some(ids) = self.index.get(&norm(dir)).cloned() else { return false };
        let mut touched = false;
        for id in ids {
            if !self.live(id) {
                continue;
            }
            match self.nodes[id as usize].load {
                Load::Loaded => {
                    // a listing that was started before this change may not have seen it, and would take it away again
                    // when it is applied: list once more after it (the change itself shows at once)
                    if self.nodes[id as usize].inflight {
                        self.nodes[id as usize].rescan_after = true;
                    }
                }
                Load::Loading(_) => {
                    self.nodes[id as usize].rescan_after = true;
                    continue;
                }
                _ => continue,
            }
            let mut resort = false;
            for ch in &changes {
                // by the index, not by walking the children: a folder with 100 000 folders gets bursts of changes too
                let ckey = child_key(&self.nodes[id as usize].key, &ch.name);
                let existing = self.index.get(&ckey).and_then(|v| v.iter().copied().find(|&c| self.nodes[c as usize].parent == Some(id)));
                match (existing, &ch.item) {
                    (Some(c), Some(it)) => {
                        let before = (self.nodes[c as usize].name.clone(), self.nodes[c as usize].label.clone(), self.nodes[c as usize].hidden, self.nodes[c as usize].system);
                        self.apply_item(c, it);
                        let n = &self.nodes[c as usize];
                        if before != (n.name.clone(), n.label.clone(), n.hidden, n.system) {
                            resort = true;
                            touched = true;
                        }
                    }
                    (Some(c), None) => {
                        self.nodes[id as usize].children.retain(|&x| x != c);
                        self.free_subtree(c);
                        touched = true;
                    }
                    (None, Some(it)) => {
                        let c = self.add_child(id, it);
                        self.nodes[id as usize].children.push(c);
                        resort = true;
                        touched = true;
                    }
                    (None, None) => {}
                }
            }
            if resort {
                self.sort_children(id);
            }
        }
        if touched {
            self.rebuild();
        }
        touched
    }

    /// The watcher lost track (events dropped, too many names): list every node of that folder again.
    pub fn rescan(&mut self, dir: &Path) -> Vec<LoadRequest> {
        let ids = self.index.get(&norm(dir)).cloned().unwrap_or_default();
        let mut reqs = Vec::new();
        for id in ids {
            if !self.live(id) {
                continue;
            }
            // One listing of a folder at a time: a burst of "too many names" reports while a big folder is being listed
            // asks for one more listing after it, not for one listing per report.
            if self.nodes[id as usize].inflight || matches!(self.nodes[id as usize].load, Load::Loading(_)) {
                self.nodes[id as usize].rescan_after = true;
            } else if self.nodes[id as usize].load == Load::Loaded {
                reqs.push(self.request(id));
            }
        }
        reqs
    }

    /// Over [`NODE_BUDGET`] nodes: frees the children of closed branches, the ones opened longest ago first, until a quarter of the
    /// budget is free. Open nodes, the branch of the open folder and a node that is being listed are left alone. A freed branch
    /// is `Unloaded` again: it shows its chevron and is listed (with the spinner) when it is opened.
    fn trim(&mut self) {
        let budget = self.budget_override.unwrap_or(NODE_BUDGET);
        let live = self.nodes.len() - self.free.len();
        if live <= budget {
            return;
        }
        let target = budget - budget / 4;
        let sel = self.selected_key.as_deref();
        let mut cands: Vec<(u64, NodeId)> = self
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.live && !n.expanded && n.load == Load::Loaded && !n.children.is_empty() && !n.inflight && !sel.is_some_and(|k| is_under(&n.key, k)))
            .map(|(i, n)| (n.seq, i as NodeId))
            .collect();
        cands.sort_unstable();
        let mut now = live;
        for (_, id) in cands {
            if now <= target {
                break;
            }
            if !self.live(id) {
                continue; // went with the branch of an earlier candidate
            }
            let before = self.free.len();
            for c in std::mem::take(&mut self.nodes[id as usize].children) {
                self.free_subtree(c);
            }
            now -= self.free.len() - before;
            let n = &mut self.nodes[id as usize];
            n.load = Load::Unloaded;
            n.rescan_after = false;
        }
    }

    /// The folders to watch: the open ones whose every parent is open too, the branch of the open
    /// folder first, then the most recently opened. At most [`MAX_WATCHES`], one per folder.
    pub fn watch_set(&self) -> Vec<PathBuf> {
        let mut found: Vec<(bool, u64, NodeId)> = Vec::new();
        let mut stack: Vec<NodeId> = self.roots.clone();
        while let Some(id) = stack.pop() {
            let n = &self.nodes[id as usize];
            if !n.expanded {
                continue;
            }
            if matches!(n.load, Load::Loaded | Load::Loading(_)) {
                let on_branch = self.selected_key.as_deref().is_some_and(|k| is_under(&n.key, k));
                found.push((on_branch, n.seq, id));
            }
            // only the open children are looked at (a node can have 100 000 closed ones)
            stack.extend(n.children.iter().copied().filter(|&c| self.nodes[c as usize].expanded));
        }
        found.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for (_, _, id) in found {
            let n = &self.nodes[id as usize];
            if seen.insert(n.key.clone()) {
                out.push(n.path.clone());
                if out.len() >= MAX_WATCHES {
                    break;
                }
            }
        }
        out
    }

    // ---- the follow rule -------------------------------------------------------------------

    /// The open folder changed. Marks it selected and opens the branch down to it, one listing at a
    /// time (the returned requests; each answer continues the walk). `None`: the open place is not a
    /// folder (Home, Settings, a tag…): nothing is selected.
    ///
    /// The rule: only closed nodes ON THE WAY to the open folder are opened, and nothing is ever
    /// closed. A node the user closed by hand less than [`COLLAPSE_GRACE_MS`] ago is left closed and the walk stops there (the
    /// closed node then carries the mark, see [`Self::marked_row`]); after that time the next navigation opens it again. A
    /// folder that is not in the listing (hidden by the settings, or a name that is not a folder) ends the walk the same way.
    ///
    /// With `open_branch` off (the follow setting is off) nothing is opened or listed and the view is not scrolled: the folder
    /// is marked only if its row is in the tree already.
    pub fn follow(&mut self, target: Option<&Path>, now_ms: u64, open_branch: bool) -> Vec<LoadRequest> {
        match target {
            None => {
                self.selected = None;
                self.selected_key = None;
                self.reveal = None;
                self.scroll_pending = false;
                Vec::new()
            }
            Some(p) => {
                let key = norm(p);
                self.selected = None;
                self.selected_key = Some(key.clone());
                self.reveal = Some(key);
                self.reveal_opens = open_branch;
                self.scroll_pending = open_branch;
                self.pump(now_ms)
            }
        }
    }

    /// The user clicked this row: it is the selected one, and nothing is opened or scrolled (the click came from the row itself, so the
    /// branch is open already, and the same folder may stand in the tree twice: the profile folder is a root and also lies under its drive).
    pub fn select_node(&mut self, id: NodeId) {
        if !self.live(id) {
            return;
        }
        self.selected = Some(id);
        self.selected_key = Some(self.nodes[id as usize].key.clone());
        self.reveal = None;
        self.scroll_pending = false;
        self.cursor = Some(id);
    }

    /// Is `path` what the tree has marked as the open folder?
    pub fn is_marked(&self, path: &Path) -> bool {
        self.selected_key.as_deref() == Some(norm(path).as_str())
    }

    fn pump(&mut self, now_ms: u64) -> Vec<LoadRequest> {
        let Some(key) = self.reveal.clone() else { return Vec::new() };
        let mut reqs = Vec::new();
        // the root closest to the target (the profile folder wins over its drive)
        let best = self.roots.iter().copied().filter(|&r| is_under(&self.nodes[r as usize].key, &key)).max_by_key(|&r| self.nodes[r as usize].key.len());
        let Some(mut cur) = best else {
            // roots that have not arrived yet: wait; a target outside every root: nothing to show
            if !self.roots.is_empty() {
                self.reveal = None;
            }
            return reqs;
        };
        let mut structural = false;
        loop {
            let (nk, state, expanded, blocked) = {
                let n = &self.nodes[cur as usize];
                (
                    n.key.clone(),
                    n.load.clone(),
                    n.expanded,
                    n.user_collapsed_ms.is_some_and(|t| now_ms.saturating_sub(t) < COLLAPSE_GRACE_MS),
                )
            };
            if nk == key {
                self.selected = Some(cur);
                self.reveal = None;
                break;
            }
            if matches!(state, Load::Failed(_)) || (!expanded && (blocked || !self.reveal_opens)) {
                self.reveal = None;
                break;
            }
            if !expanded {
                reqs.extend(self.expand_node(cur, now_ms, false));
                structural = true;
            }
            if !matches!(state, Load::Loaded) {
                // Unloaded: the request is out now; Loading: it is on its way. Either way the answer continues the walk
                // (unless the walk may not list anything).
                if !self.reveal_opens {
                    self.reveal = None;
                }
                break;
            }
            // the next folder on the way
            let rest = key[nk.trim_end_matches('\\').len()..].trim_start_matches('\\');
            let Some(comp) = rest.split('\\').next().filter(|c| !c.is_empty()) else {
                self.reveal = None;
                break;
            };
            let want = child_key(&nk, comp);
            let next = self.index.get(&want).and_then(|v| v.iter().copied().find(|&c| self.nodes[c as usize].parent == Some(cur)));
            match next {
                Some(c) if self.visible(c) => cur = c,
                _ => {
                    self.reveal = None;
                    break;
                }
            }
        }
        if structural {
            self.rebuild();
        }
        reqs
    }

    /// The row to scroll to once the walk of the follow rule is over, once.
    pub fn take_scroll_target(&mut self) -> Option<usize> {
        if !self.scroll_pending || self.reveal.is_some() {
            return None;
        }
        self.scroll_pending = false;
        self.marked_row().map(|(i, _)| i)
    }

    /// The row that stands for the open folder: its own (`true`), or, when the branch is closed
    /// or the folder is not in the tree, the closest open folder above it (`false`).
    pub fn marked_row(&self) -> Option<(usize, bool)> {
        if let Some(id) = self.selected
            && let Some(i) = self.row_index(id)
        {
            return Some((i, true));
        }
        let mut key = self.selected_key.clone()?;
        loop {
            if let Some(ids) = self.index.get(&key) {
                for &id in ids {
                    if let Some(i) = self.row_index(id) {
                        return Some((i, false));
                    }
                }
            }
            let cut = key.rfind('\\')?;
            key.truncate(cut);
            if key.is_empty() || key.ends_with(':') {
                // the drive root's key keeps its separator
                key.push('\\');
                if let Some(ids) = self.index.get(&key) {
                    for &id in ids {
                        if let Some(i) = self.row_index(id) {
                            return Some((i, false));
                        }
                    }
                }
                return None;
            }
        }
    }

    pub fn selected_id(&self) -> Option<NodeId> {
        self.selected.filter(|&i| self.live(i))
    }

    // ---- keyboard --------------------------------------------------------------------------

    pub fn set_cursor(&mut self, id: NodeId) {
        if self.live(id) {
            self.cursor = Some(id);
        }
    }

    pub fn cursor_row(&self) -> Option<usize> {
        self.cursor.and_then(|c| self.row_index(c))
    }

    fn node_rows_from(&self, start: usize, forward: bool) -> Option<usize> {
        if forward {
            (start..self.rows.len()).find(|&i| self.rows[i].kind == RowKind::Node)
        } else {
            (0..=start.min(self.rows.len().saturating_sub(1))).rev().find(|&i| self.rows[i].kind == RowKind::Node)
        }
    }

    /// One key of the tree having the focus. Up/Down move the cursor, Right opens or goes to the first child, Left closes or goes to the parent,
    /// Enter asks to navigate to the cursor's folder.
    pub fn key(&mut self, k: TreeKey, now_ms: u64) -> KeyResult {
        let mut out = KeyResult::default();
        if self.rows.is_empty() {
            return out;
        }
        // a cursor that is not on a row yet starts at the open folder's row, or the first
        let cur = self.cursor_row().or_else(|| self.marked_row().map(|(i, _)| i));
        let move_to = |this: &mut Self, out: &mut KeyResult, row: usize| {
            this.cursor = Some(this.rows[row].id);
            out.cursor_row = Some(row);
            out.changed = true;
        };
        match k {
            TreeKey::Up | TreeKey::Down => {
                let target = match (cur, k) {
                    (None, _) => self.node_rows_from(0, true),
                    (Some(c), TreeKey::Down) => self.node_rows_from(c + 1, true),
                    (Some(c), _) => c.checked_sub(1).and_then(|p| self.node_rows_from(p, false)),
                };
                if let Some(t) = target {
                    move_to(self, &mut out, t);
                } else if self.cursor.is_none()
                    && let Some(c) = cur
                {
                    move_to(self, &mut out, c);
                }
            }
            TreeKey::Home => {
                if let Some(t) = self.node_rows_from(0, true) {
                    move_to(self, &mut out, t);
                }
            }
            TreeKey::End => {
                if let Some(t) = self.node_rows_from(self.rows.len() - 1, false) {
                    move_to(self, &mut out, t);
                }
            }
            TreeKey::Right => {
                let Some(c) = cur else {
                    if let Some(t) = self.node_rows_from(0, true) {
                        move_to(self, &mut out, t);
                    }
                    return out;
                };
                let id = self.rows[c].id;
                if self.rows[c].kind != RowKind::Node {
                    return out;
                }
                if !self.nodes[id as usize].expanded {
                    if self.can_expand(id) {
                        out.loads = self.expand_node(id, now_ms, true);
                        self.rebuild();
                        self.cursor = Some(id);
                        out.cursor_row = self.row_index(id);
                        out.changed = true;
                    }
                } else if let Some(next) = self.rows.get(c + 1)
                    && next.kind == RowKind::Node
                    && next.depth == self.rows[c].depth + 1
                {
                    move_to(self, &mut out, c + 1);
                }
            }
            TreeKey::Left => {
                let Some(c) = cur else { return out };
                let id = self.rows[c].id;
                if self.rows[c].kind != RowKind::Node {
                    return out;
                }
                if self.nodes[id as usize].expanded {
                    self.collapse_node(id, now_ms, true);
                    self.trim();
                    self.rebuild();
                    self.cursor = Some(id);
                    out.cursor_row = self.row_index(id);
                    out.changed = true;
                } else if let Some(p) = self.nodes[id as usize].parent
                    && let Some(row) = self.row_index(p)
                {
                    move_to(self, &mut out, row);
                }
            }
            TreeKey::Enter => {
                if let Some(c) = cur
                    && self.rows[c].kind == RowKind::Node
                {
                    self.cursor = Some(self.rows[c].id);
                    out.navigate = Some((self.rows[c].id, self.nodes[self.rows[c].id as usize].path.clone()));
                    out.changed = true;
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str) -> DirItem {
        DirItem { name: name.into(), label: None, hidden: false, system: false }
    }

    fn items(names: &[&str]) -> Vec<DirItem> {
        names.iter().map(|n| item(n)).collect()
    }

    fn root(path: &str, label: &str, kind: RootKind) -> RootSpec {
        RootSpec { path: PathBuf::from(path), label: label.into(), kind }
    }

    fn tree() -> NavTree {
        let mut t = NavTree::new();
        t.set_roots(vec![root(r"C:\Users\ali", "ali", RootKind::Profile), root(r"C:\", "Yerel Disk (C:)", RootKind::Fixed), root(r"D:\", "Veri (D:)", RootKind::Fixed)], 0);
        t
    }

    fn labels(t: &NavTree) -> Vec<(u16, String)> {
        t.rows().iter().map(|r| (r.depth, if r.kind == RowKind::Node { t.info(r, 0).label.to_string() } else { "!".into() })).collect()
    }

    fn id_of(t: &NavTree, path: &str) -> NodeId {
        let ids = &t.index[&norm(Path::new(path))];
        ids[0]
    }

    /// Answers a request with the folders of a made-up disk.
    fn answer(t: &mut NavTree, req: &LoadRequest, disk: &HashMap<String, Vec<DirItem>>, now: u64) -> Vec<LoadRequest> {
        let r = match disk.get(&norm(&req.path)) {
            Some(v) => Ok(v.clone()),
            None => Err(ListFailure::NotFound),
        };
        t.apply_listing(req, r, now)
    }

    fn disk() -> HashMap<String, Vec<DirItem>> {
        let mut d = HashMap::new();
        d.insert(norm(Path::new(r"C:\")), items(&["Users", "Windows", "Program Files"]));
        d.insert(norm(Path::new(r"C:\Users")), items(&["ali", "Public"]));
        d.insert(norm(Path::new(r"C:\Users\ali")), items(&["Documents", "Downloads", "dir10", "dir2"]));
        d.insert(norm(Path::new(r"C:\Users\ali\Documents")), items(&["a", "b"]));
        d.insert(norm(Path::new(r"C:\Windows")), items(&["System32"]));
        d.insert(norm(Path::new(r"D:\")), items(&[]));
        d
    }

    /// Runs the follow walk to its end against the made-up disk.
    fn follow_all(t: &mut NavTree, path: &str, now: u64) {
        let mut todo = t.follow(Some(Path::new(path)), now, true);
        let d = disk();
        let mut guard = 0;
        while let Some(r) = todo.pop() {
            todo.extend(answer(t, &r, &d, now));
            guard += 1;
            assert!(guard < 50);
        }
    }

    #[test]
    fn keys_ignore_case_slashes_prefixes_and_trailing_separators() {
        assert_eq!(norm(Path::new(r"C:\Users\Ali\")), r"c:\users\ali");
        assert_eq!(norm(Path::new("C:/Users/Ali")), r"c:\users\ali");
        assert_eq!(norm(Path::new(r"\\?\C:\Users")), r"c:\users");
        assert_eq!(norm(Path::new(r"C:")), r"c:\");
        assert_eq!(norm(Path::new(r"c:\")), r"c:\");
        assert!(is_under(r"c:\", r"c:\users"));
        assert!(is_under(r"c:\users", r"c:\users\ali"));
        assert!(!is_under(r"c:\users", r"c:\users2"));
        assert_eq!(child_key(r"c:\", "Users"), r"c:\users");
        assert_eq!(child_key(r"c:\users", "Ali"), r"c:\users\ali");
    }

    #[test]
    fn roots_come_in_the_given_order_closed() {
        let t = tree();
        assert_eq!(labels(&t), vec![(0, "ali".to_string()), (0, "Yerel Disk (C:)".to_string()), (0, "Veri (D:)".to_string())]);
        assert!(t.can_expand(t.rows()[1].id), "a folder that was not listed yet shows a chevron");
    }

    #[test]
    fn opening_a_node_lists_it_once_and_the_children_sort_like_the_file_list() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let reqs = t.toggle(c, 10);
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].path, PathBuf::from(r"C:\"));
        let row = t.rows()[1];
        assert!(t.info(&row, 10).loading && !t.info(&row, 10).slow);
        assert!(t.info(&row, 10 + SLOW_LOAD_MS).slow, "a listing that takes long shows the spinner");
        assert_eq!(t.loading_since(c), Some(10));
        assert!(t.any_loading());
        t.apply_listing(&reqs[0], Ok(items(&["Windows", "dir10", "dir2", "Users"])), 20);
        let names: Vec<String> = labels(&t).into_iter().filter(|(d, _)| *d == 1).map(|(_, n)| n).collect();
        assert_eq!(names, ["dir2", "dir10", "Users", "Windows"]);
        assert!(!t.info(&t.rows()[1], 20).loading);
        assert!(!t.any_loading());
    }

    #[test]
    fn a_localized_name_is_what_is_shown_and_sorted_by() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        let mut dl = item("Downloads");
        dl.label = Some("İndirilenler".into());
        t.apply_listing(&r, Ok(vec![item("Zeta"), dl, item("Alfa")]), 0);
        let names: Vec<String> = labels(&t).into_iter().filter(|(d, _)| *d == 1).map(|(_, n)| n).collect();
        assert_eq!(names, ["Alfa", "İndirilenler", "Zeta"]);
    }

    #[test]
    fn hidden_and_system_folders_follow_the_settings_without_listing_again() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        let mut hid = item("gizli");
        hid.hidden = true;
        let mut sys = item("$Recycle.Bin");
        sys.hidden = true;
        sys.system = true;
        t.apply_listing(&r, Ok(vec![item("a"), hid, sys, item(".git")]), 0);
        let count = |t: &NavTree| t.rows().iter().filter(|r| r.depth == 1).count();
        assert_eq!(count(&t), 2, "a and .git (dot files are shown unless asked)");
        t.set_filter(TreeFilter { show_hidden: true, show_system: false, dot_hidden: false });
        assert_eq!(count(&t), 3, "the hidden one shows, the hidden system one does not");
        t.set_filter(TreeFilter { show_hidden: true, show_system: true, dot_hidden: false });
        assert_eq!(count(&t), 4);
        t.set_filter(TreeFilter { show_hidden: false, show_system: false, dot_hidden: true });
        assert_eq!(count(&t), 1);
        assert!(!t.set_filter(TreeFilter { show_hidden: false, show_system: false, dot_hidden: true }), "no change, no work");
    }

    #[test]
    fn an_answer_for_a_node_that_was_listed_again_or_freed_is_dropped() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let first = t.toggle(c, 0).remove(0);
        // the same node is listed again (a failure retried): the first answer is stale
        t.nodes[c as usize].load = Load::Failed(Fail::Other("x".into()));
        let again = t.retry(c, 3).remove(0);
        assert!(t.apply_listing(&first, Ok(items(&["old"])), 4).is_empty());
        assert!(labels(&t).iter().all(|(_, n)| n != "old"), "the stale answer changed nothing");
        t.apply_listing(&again, Ok(items(&["new"])), 5);
        assert_eq!(labels(&t)[2], (1, "new".to_string()));
        // a folder that is gone (the listing says so) leaves the tree and frees its slot
        let n = id_of(&t, r"C:\new");
        let req = t.toggle(n, 6).remove(0);
        t.apply_listing(&req, Err(ListFailure::NotFound), 8);
        assert!(labels(&t).iter().all(|(_, n)| n != "new"));
        assert!(!t.live(n));
        // the slot is reused by another folder: the old answer must not land in it
        let reuse = t.add_child(c, &item("other"));
        assert_eq!(reuse, n, "the slot is reused");
        assert!(t.apply_listing(&req, Ok(items(&["ghost"])), 9).is_empty());
        assert!(t.nodes[reuse as usize].children.is_empty());
    }

    #[test]
    fn a_failed_listing_is_a_quiet_row_and_a_click_tries_again() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        t.apply_listing(&r, Err(ListFailure::Other("Erişim reddedildi".into())), 1);
        let rows = t.rows().to_vec();
        assert_eq!(rows[2].kind, RowKind::Error);
        assert_eq!(rows[2].depth, 1);
        assert_eq!(t.info(&rows[2], 1).error, Some("Erişim reddedildi"));
        let retry = t.retry(c, 2);
        assert_eq!(retry.len(), 1);
        assert!(t.info(&t.rows()[1], 2).loading);
        t.apply_listing(&retry[0], Ok(items(&["x"])), 3);
        assert_eq!(t.rows().len(), 4);
        // a refresh that fails once the node is listed leaves what is shown
        let refresh = t.rescan(Path::new(r"C:\")).remove(0);
        t.apply_listing(&refresh, Err(ListFailure::Other("x".into())), 4);
        assert_eq!(t.rows().len(), 4);
        assert!(t.rows().iter().all(|r| r.kind == RowKind::Node));
    }

    #[test]
    fn following_opens_the_branch_one_listing_at_a_time_and_marks_the_folder() {
        let mut t = tree();
        let d = disk();
        // the profile folder is a root of its own: the walk goes down that one
        let todo = t.follow(Some(Path::new(r"C:\Users\ali\Documents\a")), 100, true);
        assert_eq!(todo.len(), 1);
        assert_eq!(todo[0].path, PathBuf::from(r"C:\Users\ali"));
        let next = answer(&mut t, &todo[0], &d, 100);
        assert_eq!(next.len(), 1, "Documents is the next one to list");
        assert_eq!(next[0].path, PathBuf::from(r"C:\Users\ali\Documents"));
        assert!(t.take_scroll_target().is_none(), "not there yet");
        let end = answer(&mut t, &next[0], &d, 100);
        assert!(end.is_empty());
        let row = t.take_scroll_target().expect("the row to scroll to");
        assert_eq!(t.info(&t.rows()[row], 0).label, "a");
        assert!(t.info(&t.rows()[row], 0).selected);
        assert_eq!(t.rows()[row].depth, 2);
        assert!(t.take_scroll_target().is_none(), "scrolled once");
        // the drive's own branch was not touched
        assert!(!t.nodes[id_of(&t, r"C:\") as usize].expanded);
        // following a folder in the same branch lists nothing new
        assert!(t.follow(Some(Path::new(r"C:\Users\ali\Documents\b")), 200, true).is_empty());
        assert_eq!(t.marked_row().map(|m| m.1), Some(true));
    }

    #[test]
    fn following_a_drive_branch_opens_each_level_and_keeps_siblings_closed() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows\System32", 0);
        let l = labels(&t);
        assert!(l.contains(&(2, "System32".to_string())));
        // nothing else was opened: Users, Program Files stay closed
        assert!(!t.nodes[id_of(&t, r"C:\Program Files") as usize].expanded);
        assert!(t.nodes[id_of(&t, r"C:\Windows") as usize].expanded);
        assert!(t.marked_row().unwrap().1);
    }

    #[test]
    fn a_branch_the_user_just_closed_stays_closed_and_the_closed_node_carries_the_mark() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents", 1_000);
        let ali = id_of(&t, r"C:\Users\ali");
        t.toggle(ali, 2_000); // the user closes it by hand
        let before = t.rows().len();
        // 10 s later the open folder changes to something inside the closed branch
        let reqs = t.follow(Some(Path::new(r"C:\Users\ali\Documents\a")), 12_000, true);
        assert!(reqs.is_empty(), "the walk does not reopen what was just closed");
        assert_eq!(t.rows().len(), before);
        let (row, exact) = t.marked_row().unwrap();
        assert!(!exact);
        assert_eq!(t.info(&t.rows()[row], 0).label, "ali", "the closed node carries the mark");
        assert!(t.take_scroll_target().is_some_and(|r| r == row));
        // after the grace time the next navigation opens it again, without a new listing of what is known
        let again = t.follow(Some(Path::new(r"C:\Users\ali\Documents\b")), 2_000 + COLLAPSE_GRACE_MS + 1, true);
        assert!(again.iter().any(|r| r.path == Path::new(r"C:\Users\ali")), "the branch is listed again quietly when it opens");
        assert!(t.nodes[ali as usize].expanded);
        // and opening by hand clears the stamp
        t.toggle(ali, 500_000);
        t.toggle(ali, 500_001);
        assert!(t.nodes[ali as usize].user_collapsed_ms.is_none());
    }

    #[test]
    fn a_folder_hidden_by_the_settings_ends_the_walk_at_its_parent() {
        let mut t = tree();
        let mut d = disk();
        let mut hid = item("gizli");
        hid.hidden = true;
        d.get_mut(&norm(Path::new(r"C:\Windows"))).unwrap().push(hid);
        let mut todo = t.follow(Some(Path::new(r"C:\Windows\gizli")), 0, true);
        while let Some(r) = todo.pop() {
            todo.extend(answer(&mut t, &r, &d, 0));
        }
        let (row, exact) = t.marked_row().unwrap();
        assert!(!exact);
        assert_eq!(t.info(&t.rows()[row], 0).label, "Windows");
    }

    #[test]
    fn a_place_outside_the_tree_selects_nothing() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows", 0);
        assert!(t.follow(None, 1, true).is_empty());
        assert!(t.marked_row().is_none());
        assert!(t.follow(Some(Path::new(r"\\sunucu\paylaşım")), 2, true).is_empty());
        assert!(t.marked_row().is_none());
        assert!(t.take_scroll_target().is_none());
    }

    #[test]
    fn a_follow_that_comes_before_the_roots_waits_for_them() {
        let mut t = NavTree::new();
        assert!(t.follow(Some(Path::new(r"C:\Users")), 0, true).is_empty());
        let reqs = t.set_roots(vec![root(r"C:\", "C", RootKind::Fixed)], 5);
        assert_eq!(reqs.len(), 1, "the roots arrived: the walk starts");
        assert_eq!(reqs[0].path, PathBuf::from(r"C:\"));
    }

    #[test]
    fn a_folder_made_removed_or_renamed_inside_an_open_node_shows_up_and_goes() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents\a", 0);
        let docs = id_of(&t, r"C:\Users\ali\Documents");
        let before: Vec<String> = labels(&t).into_iter().map(|(_, n)| n).collect();
        assert!(before.contains(&"a".to_string()));
        let parent = Path::new(r"C:\Users\ali");
        // made
        assert!(t.apply_changes(parent, vec![Change { name: "Yeni".into(), item: Some(item("Yeni")) }]));
        assert!(labels(&t).iter().any(|(_, n)| n == "Yeni"));
        // renamed: the old name is gone, the new one is there; the open folder below keeps its state
        assert!(t.apply_changes(parent, vec![Change { name: "dir2".into(), item: None }, Change { name: "iki".into(), item: Some(item("iki")) }]));
        let l: Vec<String> = labels(&t).into_iter().map(|(_, n)| n).collect();
        assert!(!l.contains(&"dir2".to_string()) && l.contains(&"iki".to_string()));
        assert!(t.nodes[docs as usize].expanded);
        // removed: an open folder with open folders below it goes with all of them
        assert!(t.apply_changes(parent, vec![Change { name: "documents".into(), item: None }]));
        assert!(!labels(&t).iter().any(|(_, n)| n == "Documents" || n == "a"));
        assert!(!t.live(docs));
        assert!(!t.index.contains_key(&norm(Path::new(r"C:\Users\ali\Documents"))));
        // a file or an unknown name changes nothing
        assert!(!t.apply_changes(parent, vec![Change { name: "x.txt".into(), item: None }]));
        // a change in a folder that is not open is nobody's business
        assert!(!t.apply_changes(Path::new(r"C:\Windows"), vec![Change { name: "q".into(), item: Some(item("q")) }]));
        // a case-only rename updates the shown name in place
        assert!(t.apply_changes(parent, vec![Change { name: "downloads".into(), item: Some(item("DOWNLOADS")) }]));
        assert!(labels(&t).iter().any(|(_, n)| n == "DOWNLOADS"));
        assert_eq!(t.path_of(id_of(&t, r"C:\Users\ali\downloads")), Some(Path::new(r"C:\Users\ali\DOWNLOADS")));
    }

    #[test]
    fn a_listing_again_keeps_what_was_open_and_drops_what_is_gone() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents\a", 0);
        let docs = id_of(&t, r"C:\Users\ali\Documents");
        let req = t.rescan(Path::new(r"C:\Users\ali")).remove(0);
        t.apply_listing(&req, Ok(items(&["documents", "yeni"])), 5);
        assert!(t.live(docs), "same folder, same node");
        assert!(t.nodes[docs as usize].expanded);
        let l: Vec<String> = labels(&t).into_iter().map(|(_, n)| n).collect();
        assert!(l.contains(&"yeni".to_string()) && !l.contains(&"Downloads".to_string()));
        // a change reported while the first listing was on its way is answered by one more listing
        let n = id_of(&t, r"C:\Users\ali\yeni");
        let r1 = t.toggle(n, 6).remove(0);
        assert!(!t.apply_changes(Path::new(r"C:\Users\ali\yeni"), vec![Change { name: "k".into(), item: Some(item("k")) }]));
        let more = t.apply_listing(&r1, Ok(items(&[])), 7);
        assert_eq!(more.len(), 1);
    }

    #[test]
    fn only_open_nodes_are_watched_the_open_folders_branch_first_and_never_more_than_the_cap() {
        let mut t = tree();
        assert!(t.watch_set().is_empty(), "nothing open, nothing watched");
        follow_all(&mut t, r"C:\Users\ali\Documents", 0);
        let set = t.watch_set();
        assert_eq!(set, [PathBuf::from(r"C:\Users\ali")], "only the profile folder is open (Documents is the open folder, not opened)");
        // open Documents too: the newest first
        let docs = id_of(&t, r"C:\Users\ali\Documents");
        let rq = t.toggle(docs, 5).remove(0);
        answer(&mut t, &rq, &disk(), 5);
        assert_eq!(t.watch_set(), [PathBuf::from(r"C:\Users\ali\Documents"), PathBuf::from(r"C:\Users\ali")]);
        // a node open under a closed parent is not watched, and is again when the parent opens
        let ali = id_of(&t, r"C:\Users\ali");
        t.toggle(ali, 6);
        assert!(t.watch_set().is_empty());
        t.toggle(ali, 7);
        assert_eq!(t.watch_set().len(), 2);
        // the profile folder (a root) and the same folder under its drive are one watch
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 8).remove(0);
        answer(&mut t, &r, &disk(), 8);
        let users = id_of(&t, r"C:\Users");
        let r = t.toggle(users, 9).remove(0);
        answer(&mut t, &r, &disk(), 9);
        let second_ali = t.index[&norm(Path::new(r"C:\Users\ali"))][1];
        let r = t.toggle(second_ali, 10).remove(0);
        answer(&mut t, &r, &disk(), 10);
        let w = t.watch_set();
        assert_eq!(w.iter().filter(|p| p.as_path() == Path::new(r"C:\Users\ali")).count(), 1);
        assert_eq!(w.len(), 4, "C:\\, Users, ali and Documents: {w:?}");
        // a change in that folder is applied to both nodes
        assert!(t.apply_changes(Path::new(r"C:\Users\ali"), vec![Change { name: "Yeni".into(), item: Some(item("Yeni")) }]));
        assert_eq!(labels(&t).iter().filter(|(_, n)| n == "Yeni").count(), 2);
        // the cap
        let mut many = NavTree::new();
        many.set_roots(vec![root(r"C:\", "C", RootKind::Fixed)], 0);
        let c = id_of(&many, r"C:\");
        let r = many.toggle(c, 0).remove(0);
        let names: Vec<String> = (0..100).map(|i| format!("f{i}")).collect();
        many.apply_listing(&r, Ok(names.iter().map(|n| item(n)).collect()), 0);
        for (i, n) in names.iter().enumerate() {
            let id = id_of(&many, &format!(r"C:\{n}"));
            let rq = many.toggle(id, i as u64 + 1).remove(0);
            many.apply_listing(&rq, Ok(items(&["k"])), 0);
        }
        let w = many.watch_set();
        assert_eq!(w.len(), MAX_WATCHES);
        assert_eq!(w[0], PathBuf::from(r"C:\f99"), "the newest first");
    }

    #[test]
    fn reopening_a_branch_lists_it_and_the_open_folders_below_it_again() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows\System32", 0);
        let win = id_of(&t, r"C:\Windows");
        let c = id_of(&t, r"C:\");
        t.toggle(c, 2); // close the whole branch by its top
        let reqs = t.toggle(c, 3);
        let paths: Vec<PathBuf> = reqs.iter().map(|r| r.path.clone()).collect();
        assert!(paths.contains(&PathBuf::from(r"C:\")), "{paths:?}");
        assert!(paths.contains(&PathBuf::from(r"C:\Windows")), "{paths:?}");
        assert!(t.nodes[win as usize].expanded);
        // the rows are there at once, from what was known, and there is no spinner for a refresh
        assert!(labels(&t).iter().any(|(_, n)| n == "System32"));
        assert!(!t.any_loading());
    }

    #[test]
    fn with_the_follow_setting_off_the_open_folder_is_marked_only_when_its_row_is_there() {
        let mut t = tree();
        // nothing is open: no listing, no opening, no scrolling
        assert!(t.follow(Some(Path::new(r"C:\Users\ali\Documents")), 0, false).is_empty());
        assert!(!t.marked_row().unwrap().1, "the closest open folder above carries the mark");
        assert!(t.take_scroll_target().is_none());
        assert_eq!(t.rows().len(), 3);
        // the user opens the branch by hand; the same folder is marked on the next navigation
        let ali = id_of(&t, r"C:\Users\ali");
        let r = t.toggle(ali, 1).remove(0);
        answer(&mut t, &r, &disk(), 1);
        assert!(t.follow(Some(Path::new(r"C:\Users\ali\Documents")), 2, false).is_empty());
        let (row, exact) = t.marked_row().unwrap();
        assert!(exact);
        assert_eq!(t.info(&t.rows()[row], 0).label, "Documents");
        assert!(t.take_scroll_target().is_none());
    }

    #[test]
    fn a_click_selects_that_very_row_even_when_the_folder_stands_in_the_tree_twice() {
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        answer(&mut t, &r, &disk(), 0);
        let users = id_of(&t, r"C:\Users");
        let r = t.toggle(users, 1).remove(0);
        answer(&mut t, &r, &disk(), 1);
        let under_drive = t.index[&norm(Path::new(r"C:\Users\ali"))][1];
        t.select_node(under_drive);
        assert!(t.is_marked(Path::new(r"c:\users\ali\")));
        assert_eq!(t.selected_id(), Some(under_drive));
        let (row, exact) = t.marked_row().unwrap();
        assert!(exact);
        assert_eq!(t.rows()[row].id, under_drive);
        assert!(t.take_scroll_target().is_none(), "no jump to the other copy");
        assert!(!t.is_marked(Path::new(r"C:\Users")));
    }

    #[test]
    fn the_keyboard_walks_opens_closes_and_navigates() {
        let mut t = tree();
        let d = disk();
        // nothing focused yet: Down lands on the first row
        let k = t.key(TreeKey::Down, 0);
        assert_eq!(k.cursor_row, Some(0));
        // Right on the second root opens it
        t.key(TreeKey::Down, 0);
        let k = t.key(TreeKey::Right, 1);
        assert_eq!(k.loads.len(), 1);
        assert_eq!(k.loads[0].path, PathBuf::from(r"C:\"));
        // Right while it loads does nothing more; the listing arrives
        assert!(t.key(TreeKey::Right, 1).cursor_row.is_none());
        answer(&mut t, &k.loads[0], &d, 2);
        // Right on an open node goes to its first child
        let k = t.key(TreeKey::Right, 3);
        assert_eq!(k.cursor_row, Some(2));
        assert_eq!(t.info(&t.rows()[2], 0).label, "Program Files");
        // Left goes to the parent; Left again closes it
        let k = t.key(TreeKey::Left, 3);
        assert_eq!(k.cursor_row, Some(1));
        let k = t.key(TreeKey::Left, 4);
        assert!(k.changed);
        assert_eq!(t.rows().len(), 3);
        assert!(t.nodes[id_of(&t, r"C:\") as usize].user_collapsed_ms.is_some());
        // Enter asks to navigate to the cursor's folder
        let k = t.key(TreeKey::Enter, 5);
        assert_eq!(k.navigate.map(|(_, p)| p), Some(PathBuf::from(r"C:\")));
        // Up from the second row goes to the first; Up on the first stays
        assert_eq!(t.key(TreeKey::Up, 5).cursor_row, Some(0));
        assert_eq!(t.key(TreeKey::Up, 5).cursor_row, None);
        assert_eq!(t.key(TreeKey::End, 5).cursor_row, Some(2));
        assert_eq!(t.key(TreeKey::Down, 5).cursor_row, None);
        assert_eq!(t.key(TreeKey::Home, 5).cursor_row, Some(0));
        // the cursor sits under a node that closes: it moves up to that node
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows\System32", 0);
        let sys = id_of(&t, r"C:\Windows\System32");
        t.set_cursor(sys);
        t.toggle(id_of(&t, r"C:\"), 9);
        assert_eq!(t.cursor_row(), Some(1));
    }

    #[test]
    fn the_cursor_skips_error_rows_and_starts_at_the_open_folder() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows", 0);
        let k = t.key(TreeKey::Down, 0);
        let row = k.cursor_row.unwrap();
        // started at the open folder's row, then moved one down
        assert!(row > t.marked_row().unwrap().0);
        // a failed node: Down passes its error row
        let mut t = tree();
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        t.apply_listing(&r, Err(ListFailure::Other("x".into())), 0);
        t.set_cursor(c);
        let k = t.key(TreeKey::Down, 0);
        assert_eq!(t.rows()[k.cursor_row.unwrap()].kind, RowKind::Node);
        assert_eq!(t.info(&t.rows()[k.cursor_row.unwrap()], 0).label, "Veri (D:)");
    }

    #[test]
    fn a_drive_that_goes_away_takes_its_branch_and_one_that_stays_keeps_its_state() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Windows", 0);
        let keep = id_of(&t, r"C:\");
        t.set_roots(vec![root(r"C:\Users\ali", "ali", RootKind::Profile), root(r"C:\", "Yerel Disk (C:)", RootKind::Fixed)], 5);
        assert_eq!(t.root_count(), 2);
        assert!(t.live(keep) && t.nodes[keep as usize].expanded);
        assert!(!labels(&t).iter().any(|(_, n)| n == "Veri (D:)"));
        t.set_roots(vec![root(r"C:\Users\ali", "ali", RootKind::Profile)], 6);
        assert!(!t.live(keep));
        assert_eq!(labels(&t), vec![(0, "ali".to_string())]);
    }

    #[test]
    fn thousands_of_rows_stay_cheap_to_rebuild() {
        let mut t = NavTree::new();
        t.set_roots(vec![root(r"C:\", "C", RootKind::Fixed)], 0);
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        let names: Vec<String> = (0..20_000).map(|i| format!("klasör {i}")).collect();
        let start = std::time::Instant::now();
        t.apply_listing(&r, Ok(names.iter().map(|n| item(n)).collect()), 0);
        let took = start.elapsed();
        assert_eq!(t.rows().len(), 20_001);
        assert_eq!(t.info(&t.rows()[1], 0).label, "klasör 0");
        assert_eq!(t.info(&t.rows()[20_000], 0).label, "klasör 19999");
        // a debug build of the sort and the rebuild for 20 000 folders is far below a frame budget times a hundred
        assert!(took < std::time::Duration::from_secs(2), "{took:?}");
    }

    #[test]
    fn a_change_that_comes_while_a_quiet_relist_is_on_its_way_is_not_taken_away_again() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents", 0);
        let ali = id_of(&t, r"C:\Users\ali");
        t.toggle(ali, 1); // closed by hand
        let relist = t.toggle(ali, 2).into_iter().find(|r| r.path == Path::new(r"C:\Users\ali")).expect("the node is listed again, quietly");
        // The listing had passed the place where "Yeni" is made before it was made; the watcher reports it and the tree shows it.
        assert!(t.apply_changes(Path::new(r"C:\Users\ali"), vec![Change { name: "Yeni".into(), item: Some(item("Yeni")) }]));
        assert!(labels(&t).iter().any(|(_, n)| n == "Yeni"));
        // The answer comes without it. It must not leave the tree wrong: one more listing is asked for.
        let more = t.apply_listing(&relist, Ok(items(&["Documents", "Downloads", "dir10", "dir2"])), 3);
        assert_eq!(more.len(), 1, "listed once more after it");
        t.apply_listing(&more[0], Ok(items(&["Documents", "Downloads", "dir10", "dir2", "Yeni"])), 4);
        assert!(labels(&t).iter().any(|(_, n)| n == "Yeni"), "the new folder is there");
        // and a listing nobody changed anything during asks for nothing more
        let quiet = t.rescan(Path::new(r"C:\Users\ali")).remove(0);
        assert!(t.apply_listing(&quiet, Ok(items(&["Documents", "Yeni"])), 5).is_empty());
    }

    #[test]
    fn reports_that_come_while_a_folder_is_being_listed_ask_for_one_more_listing_not_one_each() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents", 0);
        let ali = Path::new(r"C:\Users\ali");
        let first = t.rescan(ali).remove(0);
        assert!(t.rescan(ali).is_empty(), "one listing at a time");
        assert!(t.rescan(ali).is_empty());
        let more = t.apply_listing(&first, Ok(items(&["Documents"])), 5);
        assert_eq!(more.len(), 1, "one more, for all the reports");
        assert!(t.apply_listing(&more[0], Ok(items(&["Documents"])), 6).is_empty());
        // a failed listing does not start a loop of its own
        let fail = t.rescan(ali).remove(0);
        assert!(t.rescan(ali).is_empty());
        assert!(t.apply_listing(&fail, Err(ListFailure::Other("x".into())), 7).is_empty());
    }

    #[test]
    fn closed_branches_are_freed_above_the_budget_but_never_the_open_folders_branch() {
        let mut t = NavTree::new();
        t.budget_override = Some(20);
        t.set_roots(vec![root(r"C:\", "C", RootKind::Fixed)], 0);
        let c = id_of(&t, r"C:\");
        let r = t.toggle(c, 0).remove(0);
        let folders: Vec<String> = (0..10).map(|i| format!("f{i}")).collect();
        t.apply_listing(&r, Ok(folders.iter().map(|n| item(n)).collect()), 0);
        for n in &folders {
            let id = id_of(&t, &format!(r"C:\{n}"));
            let rq = t.toggle(id, 1).remove(0);
            t.apply_listing(&rq, Ok(items(&["k0", "k1", "k2", "k3", "k4"])), 1);
        }
        let live = |t: &NavTree| t.nodes.len() - t.free.len();
        assert_eq!(live(&t), 1 + 10 + 50);
        assert!(t.follow(Some(Path::new(r"C:\f3\k1")), 2, true).is_empty());
        for (i, n) in folders.iter().enumerate() {
            t.toggle(id_of(&t, &format!(r"C:\{n}")), 100 + i as u64); // the user closes them one by one
        }
        let f3 = id_of(&t, r"C:\f3");
        assert_eq!(t.nodes[f3 as usize].children.len(), 5, "the open folder's branch is kept");
        assert_eq!(live(&t), 1 + 10 + 5);
        let f5 = id_of(&t, r"C:\f5");
        assert!(t.nodes[f5 as usize].children.is_empty() && t.can_expand(f5), "a freed branch still shows its chevron");
        assert!(t.marked_row().is_some());
        // it is listed again, with the spinner, when it is opened
        let again = t.toggle(f5, 200);
        assert_eq!(again.len(), 1);
        assert!(t.info(&t.rows().iter().copied().find(|r| r.id == f5).unwrap(), 200).loading);
        t.apply_listing(&again[0], Ok(items(&["k0"])), 201);
        assert_eq!(t.nodes[f5 as usize].children.len(), 1);
        // with the budget not reached nothing is freed
        let mut small = tree();
        follow_all(&mut small, r"C:\Windows\System32", 0);
        let win = id_of(&small, r"C:\Windows");
        small.toggle(win, 5);
        assert!(!small.nodes[win as usize].children.is_empty());
    }

    #[test]
    fn a_case_only_rename_gives_what_is_open_below_it_the_new_spelling() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents\a", 0);
        let a = id_of(&t, r"C:\Users\ali\Documents\a");
        assert!(t.apply_changes(Path::new(r"C:\Users\ali"), vec![Change { name: "documents".into(), item: Some(item("DOCUMENTS")) }]));
        assert_eq!(t.path_of(a), Some(Path::new(r"C:\Users\ali\DOCUMENTS\a")));
        assert!(t.nodes[id_of(&t, r"C:\Users\ali\Documents") as usize].expanded, "still open");
        assert!(t.marked_row().unwrap().1, "still the open folder");
    }

    #[test]
    fn a_link_that_leads_back_up_cannot_make_the_walk_or_the_watches_loop() {
        let mut t = NavTree::new();
        t.set_roots(vec![root(r"C:\", "C", RootKind::Fixed)], 0);
        // every folder under C:\a lists a folder "loop" (a junction to C:\a), the deepest asked-for one lists "x"
        let lister = |p: &Path| -> Vec<DirItem> {
            match norm(p).as_str() {
                r"c:\" => items(&["a"]),
                r"c:\a\loop\loop\loop" => items(&["x"]),
                _ => items(&["loop"]),
            }
        };
        let mut todo = t.follow(Some(Path::new(r"C:\a\loop\loop\loop\x")), 0, true);
        let mut guard = 0;
        while let Some(r) = todo.pop() {
            let l = lister(&r.path);
            todo.extend(t.apply_listing(&r, Ok(l), 0));
            guard += 1;
            assert!(guard < 20, "the walk stops");
        }
        let (row, exact) = t.marked_row().unwrap();
        assert!(exact);
        assert_eq!(t.rows()[row].depth, 5);
        assert_eq!(t.watch_set().len(), 5, "every level is a folder of its own: C:\\, a, and three loops");
    }

    #[test]
    fn a_root_that_stays_takes_the_new_label_and_kind() {
        let mut t = tree();
        t.set_roots(vec![root(r"C:\Users\ali", "ali", RootKind::Profile), root(r"C:\", "Sistem (C:)", RootKind::Removable)], 1);
        let rows = t.rows().to_vec();
        assert_eq!(t.info(&rows[1], 1).label, "Sistem (C:)");
        assert_eq!(t.info(&rows[1], 1).root, Some(RootKind::Removable));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn the_row_of_a_node_is_found_without_a_scan_and_never_wrongly() {
        let mut t = tree();
        follow_all(&mut t, r"C:\Users\ali\Documents\a", 0);
        for (i, r) in t.rows().to_vec().iter().enumerate() {
            assert_eq!(t.row_index(r.id), Some(i));
        }
        // a node that has no row (closed branch) and a node that was freed have none
        let b = id_of(&t, r"C:\Users\ali\Documents\b");
        let docs = id_of(&t, r"C:\Users\ali\Documents");
        assert!(t.row_index(b).is_some());
        t.toggle(docs, 9);
        assert_eq!(t.row_index(b), None);
        t.toggle(docs, 10);
        t.apply_changes(Path::new(r"C:\Users\ali\Documents"), vec![Change { name: "b".into(), item: None }]);
        assert_eq!(t.row_index(b), None, "freed");
        assert_eq!(t.row_index(10_000), None);
    }

    #[test]
    fn the_rows_that_are_built_follow_the_scroll_position() {
        // not measured yet: the first rows
        assert_eq!(visible_range(1000, 0., 0., 30., 6, 40), (0, 40));
        assert_eq!(visible_range(10, 0., 0., 30., 6, 40), (0, 10));
        // row 100 at the top of a 300 px view: 10 rows in view and 6 more each side
        assert_eq!(visible_range(1000, 3000., 300., 30., 6, 40), (94, 116));
        // the tree starts below the top of the view
        assert_eq!(visible_range(1000, -50., 300., 30., 6, 40), (0, 15));
        // scrolled past the end: nothing, and no panic
        assert_eq!(visible_range(10, 5000., 300., 30., 6, 40), (10, 10));
        // the last rows
        assert_eq!(visible_range(1000, 29_800., 300., 30., 6, 40), (987, 1000));
    }

    #[test]
    fn the_scroll_goal_shows_the_row_with_the_least_movement_or_puts_it_in_the_upper_third() {
        // centre: comfortably in view stays, otherwise upper third, never above the top
        assert_eq!(scroll_goal(100., 300., 150., 30., true), 100.);
        assert_eq!(scroll_goal(100., 300., 1000., 30., true), 900.);
        assert_eq!(scroll_goal(500., 300., 10., 30., true), 0.);
        assert_eq!(scroll_goal(0., 300., 280., 30., true), 180., "too close to the lower edge");
        // key: least movement
        assert_eq!(scroll_goal(100., 300., 150., 30., false), 100.);
        assert_eq!(scroll_goal(100., 300., 90., 30., false), 86.);
        assert_eq!(scroll_goal(100., 300., 390., 30., false), 124.);
        assert_eq!(scroll_goal(100., 300., 2., 30., false), 0.);
    }
}
