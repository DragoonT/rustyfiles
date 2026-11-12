//! Columns for any Windows property in the Details view (duration, artist, date taken ...): the part
//! that lives inside the folder view. The data side is `rf_win::propcols` (reading) and
//! `rf_core::propcols` (the chosen list, typed values, the cache); the chooser window is
//! `crate::prop_chooser`. See docs/design/property-columns.md.
//!
//! With no property column chosen none of this runs: every hook starts with a check of one settings
//! vector, no thread is started and nothing is allocated per row.
//!
//! This is a child module of the folder view so that it can reach the view's private state; the
//! folder view itself only has small hooks (a field, the header and row cells, a status line).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize};

use parking_lot::Mutex;
use rf_core::propcols::{self as pc, ChosenColumn, ChosenColumns, PropCache, PropCell, PropKind, PropValue};
use rf_win::propcols as wp;

use super::*;
use crate::fluent_menu::{self, Item, Menu, MenuIcon};

/// The longest one file may take to answer.
const READ_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);
/// Rows read ahead of and behind the visible ones.
const MARGIN: usize = 30;
/// Most rows planned at once.
const PLAN_LIMIT: usize = 150;
/// How long a notice stays in the status bar.
const NOTICE_SECS: u64 = 8;
/// A file that ran out of time is not asked again for this long (a scroll after it asks again).
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(60);
/// Most files remembered as having run out of time.
const BACKOFF_CAP: usize = 4096;
/// A sort or grouping made again after the watcher's news starts at once, then at most this often.
const REFRESH_GAP: std::time::Duration = std::time::Duration::from_secs(1);

// ------------------------------------------------------------------ process-wide state

static CACHE: OnceLock<Mutex<PropCache>> = OnceLock::new();
/// Changes whenever a value is stored: a view re-plans only when it moved or this did.
static CACHE_VERSION: AtomicU64 = AtomicU64::new(0);
static BACKOFF: OnceLock<Mutex<Backoff>> = OnceLock::new();
static CATALOG: OnceLock<Mutex<HashMap<String, wp::PropInfo>>> = OnceLock::new();
static LIST: OnceLock<Arc<Vec<wp::PropInfo>>> = OnceLock::new();

/// Values per file, shared by every tab and kept while the program runs (a folder visited again shows
/// its columns at once). Bounded: see `rf_core::propcols::CACHE_CAP`.
fn cache() -> &'static Mutex<PropCache> {
    CACHE.get_or_init(Default::default)
}

/// Files that did not answer in time. They are not stored as "no value" (the next scroll over them, a
/// minute later, tries again: one stuck handler must not blank a file for the whole session), and
/// they are not planned again before then (or every answer would start the same request over).
#[derive(Default)]
struct Backoff {
    files: HashMap<std::path::PathBuf, (u64, i64, Instant)>,
}

impl Backoff {
    fn note(&mut self, path: std::path::PathBuf, size: u64, modified_ms: i64, now: Instant) {
        if self.files.len() >= BACKOFF_CAP {
            self.files.retain(|_, (_, _, at)| now.duration_since(*at) < RETRY_AFTER);
            if self.files.len() >= BACKOFF_CAP {
                self.files.clear();
            }
        }
        self.files.insert(path, (size, modified_ms, now));
    }

    fn is_waiting(&self, path: &Path, size: u64, modified_ms: i64, now: Instant) -> bool {
        self.files.get(path).is_some_and(|(s, m, at)| *s == size && *m == modified_ms && now.duration_since(*at) < RETRY_AFTER)
    }
}

fn backoff() -> &'static Mutex<Backoff> {
    BACKOFF.get_or_init(Default::default)
}

/// Display name and kind of the properties seen so far.
fn catalog() -> &'static Mutex<HashMap<String, wp::PropInfo>> {
    CATALOG.get_or_init(Default::default)
}

/// Every property the system lists, once loaded (the chooser loads it).
pub(crate) fn property_list() -> Option<Arc<Vec<wp::PropInfo>>> {
    LIST.get().cloned()
}

/// Keeps the list the chooser loaded, and the names and kinds in it for the headers.
pub(crate) fn set_property_list(list: Vec<wp::PropInfo>) -> Arc<Vec<wp::PropInfo>> {
    {
        let mut c = catalog().lock();
        for p in &list {
            c.insert(p.canonical.clone(), p.clone());
        }
    }
    LIST.get_or_init(|| Arc::new(list)).clone()
}

fn kind_of(name: &str) -> PropKind {
    catalog().lock().get(name).map_or(PropKind::Text, |p| p.kind)
}

fn label_of(name: &str) -> String {
    catalog().lock().get(name).map_or_else(|| pc::fallback_label(name), |p| p.display.clone())
}

// ------------------------------------------------------------------ the view's state

/// Drag payload of a property column's edge.
#[derive(Clone)]
pub(super) struct PropResize;

enum Ev {
    /// A value arrived (repaint).
    Changed,
    /// A background read has moved on (repaint the status line).
    Progress,
    JobEnd(u64, JobOutcome),
}

enum JobOutcome {
    Sorted(Arc<Vec<u32>>),
    Grouped(Arc<Vec<(u32, String)>>),
    /// Several files in a row did not answer: given up.
    Stalled,
    Cancelled,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Purpose {
    Sort,
    Group,
}

/// A background read of every file's value for a sort or a grouping.
struct JobHandle {
    id: u64,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    /// `usize::MAX` until the job has counted what it has to read.
    total: Arc<AtomicUsize>,
    /// The entries it was started with: a result for other entries is thrown away.
    entries_gen: u64,
}

struct PropSort {
    name: Arc<str>,
    dir: SortDir,
}

/// The group (order, label) of each entry, for the entries generation it was made with.
type GroupKeys = (u64, Arc<Vec<(u32, String)>>);

/// The groups of the entries before the last refresh, found again by name: the list keeps its groups
/// (and the folded ones) while the new values are read.
type Carry = (Arc<Vec<(u32, String)>>, HashMap<Arc<str>, u32>);

struct PropGroup {
    name: Arc<str>,
    /// Once computed.
    keys: Option<GroupKeys>,
    carry: Option<Carry>,
}

#[derive(Default)]
pub(super) struct PropState {
    queue: Option<wp::ReadQueue>,
    tx: Option<mpsc::UnboundedSender<Ev>>,
    pump: Option<Task<()>>,
    /// What the last plan was made for; the same again plans nothing.
    plan_key: Option<(usize, usize, u64, u64, u64)>,
    plan_names: Vec<Arc<str>>,
    catalog_asked: HashSet<String>,
    /// Column resize in progress: (column, mouse x at start, width at start), and the live widths.
    drag: Option<(usize, f32, u16)>,
    live: Option<Vec<u16>>,
    save_gen: u64,
    sort: Option<PropSort>,
    group: Option<PropGroup>,
    sort_job: Option<JobHandle>,
    group_job: Option<JobHandle>,
    next_job: u64,
    notice: Option<(SharedString, Instant)>,
    /// Whether the open folder is a plain local one, for the path it was asked about (a drive type is
    /// asked of the system once per folder, not once per frame).
    local: std::cell::RefCell<Option<(std::path::PathBuf, bool)>>,
    last_refresh: Option<Instant>,
    refresh_pending: bool,
}

impl Drop for PropState {
    /// A tab that is closed ends the reads for its sort or grouping (the threads would go on through
    /// every remaining file otherwise, for nobody). The row queue ends with its own Drop.
    fn drop(&mut self) {
        for j in [self.sort_job.take(), self.group_job.take()].into_iter().flatten() {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl PropState {
    /// A sort by a property is the one in force (the fixed columns show no arrow then).
    pub(super) fn sort_active(&self) -> bool {
        self.sort.is_some()
    }
}

// ------------------------------------------------------------------ the columns

impl FolderView {
    /// Property columns belong to the Details view of a plain folder: not archives, FTP, search results,
    /// libraries, tags or the shell's virtual folders (their files are not at a path the property
    /// system opens).
    fn props_applicable(&self) -> bool {
        if self.view_mode != ViewMode::Details {
            return false;
        }
        let Location::Path(p) = self.location() else { return false };
        // (network shares are left out: a handler opening files over the network is slow, and the
        // reader's attribute check has no time limit of its own. Asked once per folder, not per frame.)
        let mut memo = self.props.local.borrow_mut();
        match &*memo {
            Some((path, local)) if path.as_path() == p.as_path() => *local,
            _ => {
                let local = plain_local_path(p);
                *memo = Some((p.clone(), local));
                local
            }
        }
    }

    /// The chosen columns (with the widths of a drag in progress); empty, without any allocation, when
    /// none is chosen.
    fn props_chosen(&self, cx: &App) -> ChosenColumns {
        let s = &cx.global::<crate::settings::Settings>().columns.properties;
        if s.is_empty() || !self.props_applicable() {
            return ChosenColumns::default();
        }
        let mut c = ChosenColumns::from_setting(s);
        if let Some(live) = &self.props.live {
            for (i, w) in live.iter().enumerate() {
                c.set_width(i, *w);
            }
        }
        c
    }

    /// The chosen columns that fit the pane (the name keeps 180 px), with their place in the list.
    fn props_visible(&self, cx: &App) -> Vec<(usize, ChosenColumn)> {
        let chosen = self.props_chosen(cx);
        if chosen.is_empty() {
            return Vec::new();
        }
        let room = f32::from(self.content_width) - 180. - 24.;
        let n = pc::fit_count(chosen.iter().map(|c| c.width), room);
        chosen.iter().take(n).cloned().enumerate().collect()
    }

    /// Pixels the visible property columns take; the fixed columns share what is left.
    pub(super) fn props_used_width(&self, cx: &App) -> f32 {
        if cx.global::<crate::settings::Settings>().columns.properties.is_empty() {
            return 0.;
        }
        self.props_visible(cx).iter().map(|(_, c)| f32::from(c.width)).sum()
    }

    /// The header cells of the property columns: a label, an arrow when the list is sorted by it, a
    /// handle on the left edge to resize, a click to sort.
    pub(super) fn props_header_cells(&self, p: &Palette, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let vis = self.props_visible(cx);
        let mut out = Vec::with_capacity(vis.len());
        for (ix, col) in vis {
            let name: Arc<str> = Arc::from(col.name.as_str());
            let right = kind_of(&col.name) == PropKind::Number;
            let sorted = self.props.sort.as_ref().filter(|s| *s.name == *name).map(|s| s.dir);
            let arrow = if sorted == Some(SortDir::Desc) { L::ChevronDown } else { L::ChevronUp };
            let n2 = name.clone();
            out.push(
                h_flex()
                    .id(("h-prop", ix))
                    .relative()
                    .h_full()
                    .px_2()
                    .gap_1()
                    .items_center()
                    .w(px(f32::from(col.width)))
                    .flex_shrink_0()
                    .rounded(theme::RADIUS_SM)
                    .cursor_pointer()
                    .hover(|s| s.bg(p.hover))
                    .text_color(if sorted.is_some() { p.text } else { p.text_secondary })
                    .when(right, |d| d.justify_end())
                    .overflow_hidden()
                    .child(div().min_w_0().whitespace_nowrap().overflow_hidden().text_ellipsis().child(label_of(&col.name)))
                    .when(sorted.is_some(), |d| d.child(Icon::new(arrow).size(px(12.)).flex_shrink_0().text_color(p.text_secondary)))
                    .on_click(cx.listener(move |this, _, _, cx| this.props_header_click(&n2, cx)))
                    .child(
                        div()
                            .id(("prop-resize", ix))
                            .absolute()
                            .left(px(-4.))
                            .top_0()
                            .h_full()
                            .w(px(8.))
                            .cursor_col_resize()
                            .child(div().mx_auto().w(px(1.)).h_full().bg(p.divider))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, ev: &MouseDownEvent, _, cx| {
                                    cx.stop_propagation();
                                    let cur = this.props_chosen(cx).get(ix).map_or(pc::DEFAULT_WIDTH, |c| c.width);
                                    this.props.drag = Some((ix, f32::from(ev.position.x), cur));
                                }),
                            )
                            .on_drag(PropResize, |_, _, _, cx| cx.new(|_| ColumnGhost)),
                    )
                    .into_any_element(),
            );
        }
        out
    }

    /// The mouse moves while a property column's edge is dragged.
    pub(super) fn props_drag_move(&mut self, e: &DragMoveEvent<PropResize>, cx: &mut Context<Self>) {
        let Some((ix, sx, sw)) = self.props.drag else { return };
        let mut widths: Vec<u16> = self.props_chosen(cx).iter().map(|c| c.width).collect();
        let Some(cur) = widths.get(ix).copied() else { return };
        // no wider than the room left: a column that no longer fits is hidden
        let most = (f32::from(cur) + self.spare_width(cx)).clamp(f32::from(cur).min(600.), 600.);
        widths[ix] = (f32::from(sw) - (f32::from(e.event.position.x) - sx)).clamp(70., most.max(70.)).round() as u16;
        self.props.live = Some(widths);
        self.props.save_gen += 1;
        let generation = self.props.save_gen;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            let _ = this.update(cx, |v, cx| {
                if v.props.save_gen != generation {
                    return;
                }
                v.props.drag = None;
                if let Some(widths) = v.props.live.take() {
                    crate::settings::update(cx, move |s| {
                        let mut c = ChosenColumns::from_setting(&s.columns.properties);
                        for (i, w) in widths.iter().enumerate() {
                            c.set_width(i, *w);
                        }
                        s.columns.properties = c.to_setting();
                    });
                }
            });
        })
        .detach();
        cx.notify();
    }

    // ---------------- cells

    /// What the rows need to draw property cells; None when no column is shown (the common case).
    pub(super) fn props_row_ctx(&self, cx: &App) -> Option<PropRowCtx> {
        let vis = self.props_visible(cx);
        if vis.is_empty() {
            return None;
        }
        Some(PropRowCtx { cols: vis.into_iter().map(|(_, c)| (Arc::from(c.name.as_str()), c.width)).collect() })
    }

    /// The cells of one row: the text of each value that has arrived, nothing for the others.
    pub(super) fn props_cells(&self, ctx: &PropRowCtx, ei: u32, p: &Palette) -> Vec<AnyElement> {
        let e = &self.entries[ei as usize];
        let path = (!e.is_dir() && !e.is_cloud_placeholder()).then(|| self.path_of(ei)).flatten();
        let guard = path.as_ref().map(|_| cache().lock());
        ctx.cols
            .iter()
            .map(|(name, width)| {
                let text: Option<Arc<str>> = match (&path, &guard) {
                    (Some(path), Some(g)) => g.get(path, e.size, e.modified_ms, name).map(|c| c.text.clone()).filter(|t| !t.is_empty()),
                    _ => None,
                };
                let right = kind_of(name) == PropKind::Number;
                div()
                    .w(px(f32::from(*width)))
                    .flex_shrink_0()
                    .px_2()
                    .flex()
                    .when(right, |d| d.justify_end())
                    .text_color(p.text_secondary)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .text_ellipsis()
                    .children(text.map(|t| SharedString::from(t.to_string())))
                    .into_any_element()
            })
            .collect()
    }

    /// After the rows are drawn: asks for the values of the rows on screen and a margin around them, or,
    /// when no column is shown any more, lets go of a sort or grouping that belonged to one.
    pub(super) fn props_after_rows(&mut self, shown: Option<(usize, usize)>, on: bool, cx: &mut Context<Self>) {
        // (a column that is chosen but does not fit the pane is not "gone": its sort stays)
        if self.props_stale(cx) {
            cx.spawn(async move |this, cx| {
                let _ = this.update(cx, |v, cx| v.props_prune(cx));
            })
            .detach();
        }
        if !on {
            return;
        }
        let Some((first, last)) = shown else { return };
        let names = self.props_chosen(cx).names();
        // (the order of the rows is part of the key: a sort, filter or fold puts other files on the same rows)
        let key = (first, last, self.entries_gen, CACHE_VERSION.load(Ordering::Relaxed), visible_hash(&self.visible, first, last));
        if self.props.plan_key == Some(key) && self.props.plan_names == names {
            return;
        }
        self.props.plan_key = Some(key);
        self.props.plan_names = names.clone();
        self.ensure_pump(cx);
        self.props_ensure_catalog(&names, cx);
        // rows whose values are not in yet: the visible ones first, then ahead, then behind
        let wanted: Vec<wp::QueueJob> = {
            let guard = cache().lock();
            let waiting = backoff().lock();
            let now = Instant::now();
            // true: nothing to read for this row (a folder, an online-only file, its values are in, or it
            // ran out of time a moment ago)
            let done = |vi: usize| -> bool {
                let Some(&ei) = self.visible.get(vi) else { return true };
                let e = &self.entries[ei as usize];
                if e.is_dir() || e.is_cloud_placeholder() {
                    return true;
                }
                match self.path_of(ei) {
                    Some(path) => guard.has_all(&path, e.size, e.modified_ms, &names) || waiting.is_waiting(&path, e.size, e.modified_ms, now),
                    None => true,
                }
            };
            pc::plan(first..last + 1, MARGIN, self.visible.len(), done, PLAN_LIMIT)
                .into_iter()
                .filter_map(|vi| {
                    let ei = *self.visible.get(vi)?;
                    let e = &self.entries[ei as usize];
                    Some(wp::QueueJob { path: self.path_of(ei)?, size: e.size, modified_ms: e.modified_ms })
                })
                .collect()
        };
        if wanted.is_empty() && self.props.queue.is_none() {
            return;
        }
        let Some(tx) = self.props.tx.clone() else { return };
        let queue = self.props.queue.get_or_insert_with(|| {
            let src: Arc<dyn wp::PropSource> = Arc::new(wp::SystemSource::new(READ_LIMIT));
            wp::ReadQueue::new(src, move |d: wp::Done| {
                let cells: Vec<(Arc<str>, PropCell)> = match d.result {
                    Ok(c) => d.names.iter().cloned().zip(c).collect(),
                    // too slow: no answer is not "no value". It is noted, and asked again after a while.
                    Err(wp::ReadFail::Timeout) => {
                        backoff().lock().note(d.job.path, d.job.size, d.job.modified_ms, Instant::now());
                        CACHE_VERSION.fetch_add(1, Ordering::Relaxed);
                        let _ = tx.unbounded_send(Ev::Changed);
                        return;
                    }
                    // not readable (cloud, gone, refused): an empty answer, so it is not asked again
                    Err(_) => d.names.iter().map(|n| (n.clone(), PropCell::empty())).collect(),
                };
                cache().lock().put(d.job.path, d.job.size, d.job.modified_ms, cells);
                CACHE_VERSION.fetch_add(1, Ordering::Relaxed);
                let _ = tx.unbounded_send(Ev::Changed);
            })
        });
        queue.replace(Arc::from(names), wanted);
    }

    /// Asks the system for the display names and kinds of the chosen properties not known yet.
    fn props_ensure_catalog(&mut self, names: &[Arc<str>], cx: &mut Context<Self>) {
        let missing: Vec<Arc<str>> = names
            .iter()
            .filter(|n| !catalog().lock().contains_key(&***n) && self.props.catalog_asked.insert(n.to_string()))
            .cloned()
            .collect();
        if missing.is_empty() {
            return;
        }
        cx.spawn(async move |this, cx| {
            let ask = missing.clone();
            let infos = cx.background_executor().spawn(async move { wp::describe(&ask) }).await;
            for (n, info) in missing.iter().zip(infos) {
                if let Some(info) = info {
                    catalog().lock().insert(n.to_string(), info);
                }
            }
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The channel the readers tell the view through, and the task that turns what comes into repaints
    /// (a burst of answers is one repaint).
    fn ensure_pump(&mut self, cx: &mut Context<Self>) {
        if self.props.pump.is_some() {
            return;
        }
        let (tx, mut rx) = mpsc::unbounded::<Ev>();
        self.props.tx = Some(tx);
        self.props.pump = Some(cx.spawn(async move |this, cx| {
            while let Some(first) = rx.next().await {
                cx.background_executor().timer(std::time::Duration::from_millis(30)).await;
                let mut evs = vec![first];
                while let Ok(e) = rx.try_recv() {
                    evs.push(e);
                }
                if this.update(cx, |v, cx| v.props_events(evs, cx)).is_err() {
                    break;
                }
            }
        }));
    }

    fn props_events(&mut self, evs: Vec<Ev>, cx: &mut Context<Self>) {
        for ev in evs {
            match ev {
                Ev::Changed | Ev::Progress => {}
                Ev::JobEnd(id, out) => self.props_job_end(id, out, cx),
            }
        }
        cx.notify();
    }

    // ---------------- sorting and grouping

    /// A click on a property column's header: sort by it (a second click flips the direction). The
    /// list stays as it is while every file's value is read; the new order is applied once.
    fn props_header_click(&mut self, name: &Arc<str>, cx: &mut Context<Self>) {
        if self.loading || !self.props_applicable() {
            return;
        }
        let dir = match &self.props.sort {
            Some(s) if s.name == *name => s.dir.flip(),
            _ => SortDir::Asc,
        };
        self.props.sort = Some(PropSort { name: name.clone(), dir });
        self.props_start_job(Purpose::Sort, cx);
        cx.notify();
    }

    /// Group by a property (the Group menu): the same, with the groups made once every value is in.
    pub(crate) fn props_group_by(&mut self, name: Arc<str>, cx: &mut Context<Self>) {
        if self.loading || !self.props_applicable() {
            return;
        }
        self.props.group = Some(PropGroup { name, keys: None, carry: None });
        self.props_start_job(Purpose::Group, cx);
        cx.notify();
    }

    fn props_start_job(&mut self, purpose: Purpose, cx: &mut Context<Self>) {
        let (name, dir) = match purpose {
            Purpose::Sort => match &self.props.sort {
                Some(s) => (s.name.clone(), s.dir),
                None => return,
            },
            Purpose::Group => match &self.props.group {
                Some(g) => (g.name.clone(), self.sort.dir),
                None => return,
            },
        };
        let slot = if purpose == Purpose::Sort { &mut self.props.sort_job } else { &mut self.props.group_job };
        if let Some(old) = slot.take() {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let Some(base) = self.location().path().map(|p| p.to_path_buf()) else { return };
        self.ensure_pump(cx);
        let Some(tx) = self.props.tx.clone() else { return };
        self.props.next_job += 1;
        let handle = JobHandle {
            id: self.props.next_job,
            cancel: Arc::new(AtomicBool::new(false)),
            done: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(usize::MAX)),
            entries_gen: self.entries_gen,
        };
        let job = JobInput {
            id: handle.id,
            purpose,
            base,
            entries: self.entries.clone(),
            name: name.clone(),
            kind: kind_of(&name),
            spec: SortSpec { dir, ..self.sort },
            cancel: handle.cancel.clone(),
            done: handle.done.clone(),
            total: handle.total.clone(),
        };
        let spawned = std::thread::Builder::new().name("rf-propjob".into()).spawn(move || run_job(job, tx));
        if spawned.is_err() {
            return;
        }
        match purpose {
            Purpose::Sort => self.props.sort_job = Some(handle),
            Purpose::Group => self.props.group_job = Some(handle),
        }
    }

    fn props_job_end(&mut self, id: u64, out: JobOutcome, cx: &mut Context<Self>) {
        let is_sort = self.props.sort_job.as_ref().is_some_and(|j| j.id == id);
        let is_group = self.props.group_job.as_ref().is_some_and(|j| j.id == id);
        if !is_sort && !is_group {
            return; // an old job
        }
        let handle = if is_sort { self.props.sort_job.take() } else { self.props.group_job.take() };
        let Some(handle) = handle else { return };
        match out {
            JobOutcome::Cancelled => {}
            JobOutcome::Stalled => {
                // nothing was changed: the list stays as it was
                if is_sort {
                    self.props.sort = None;
                } else {
                    self.props.group = None;
                }
                self.props.notice = Some((t!("Bir özellik işleyicisi yanıt vermedi, sıralama yapılamadı").into(), Instant::now()));
                self.props_notice_timer(cx);
            }
            JobOutcome::Sorted(order) => {
                if handle.entries_gen != self.entries_gen || order.len() != self.entries.len() {
                    // the folder changed while the values were read: once more (the values are cached)
                    self.props_start_job(Purpose::Sort, cx);
                } else if let Some(s) = self.props.sort.as_ref() {
                    // the sort menu shows the direction of the sort that is in force: the property's
                    // (the list kept the old order, and the old direction, until now)
                    self.sort = spec_of_property_sort(self.sort, s.dir);
                    // the order goes where the view keeps its sort, so every later filter or grouping uses it
                    self.sorted_cache = Some((self.sort, self.entries_gen, order));
                    self.rebuild_visible();
                }
            }
            JobOutcome::Grouped(keys) => {
                if handle.entries_gen != self.entries_gen || keys.len() != self.entries.len() {
                    self.props_start_job(Purpose::Group, cx);
                } else if let Some(g) = self.props.group.as_mut() {
                    // (groups made again after a refresh keep the folds the user made; a first grouping starts open)
                    let first = g.keys.is_none() && g.carry.is_none();
                    g.keys = Some((self.entries_gen, keys));
                    g.carry = None;
                    self.group = GroupBy::Property;
                    if first {
                        self.collapsed.clear();
                    }
                    self.rebuild_visible();
                }
            }
        }
        cx.notify();
    }

    fn props_notice_timer(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_secs(NOTICE_SECS + 1)).await;
            let _ = this.update(cx, |_, cx| cx.notify());
        })
        .detach();
    }

    /// The group of an entry for a grouping by a property (see `rebuild_visible`).
    pub(super) fn props_group_key(&self, i: u32) -> (u32, String) {
        let none = || (u32::MAX, pc::UNSPECIFIED.to_string());
        match &self.props.group {
            Some(PropGroup { keys: Some((generation, keys)), .. }) if *generation == self.entries_gen => {
                keys.get(i as usize).cloned().unwrap_or_else(none)
            }
            // entries replaced and the new groups not made yet: a file seen before keeps its group
            Some(PropGroup { carry: Some((old, by_name)), .. }) => {
                self.entries.get(i as usize).and_then(|e| by_name.get(&e.name)).and_then(|&k| old.get(k as usize)).cloned().unwrap_or_else(none)
            }
            _ => none(),
        }
    }

    /// The entries are about to be replaced (the watcher): a grouping by a property remembers the group
    /// of each file by name, for the list to show until the groups are made again.
    pub(super) fn props_before_swap(&mut self) {
        let Some(g) = self.props.group.as_mut() else { return };
        if let Some((generation, keys)) = &g.keys
            && *generation == self.entries_gen
            && keys.len() == self.entries.len()
        {
            let by_name = self.entries.iter().enumerate().map(|(i, e)| (e.name.clone(), i as u32)).collect();
            g.carry = Some((keys.clone(), by_name));
        }
    }

    /// The folder's entries were replaced (the watcher): a sort or grouping by a property is made again
    /// for the new entries (their values are mostly cached). The first news starts it at once; news
    /// that follows within a second is one more start after it, not one each (a folder that changes
    /// all the time would otherwise read and sort over and over).
    pub(super) fn props_entries_changed(&mut self, cx: &mut Context<Self>) {
        if self.props.sort.is_none() && self.props.group.is_none() {
            return;
        }
        let now = Instant::now();
        let wait = self.props.last_refresh.map_or(std::time::Duration::ZERO, |t| REFRESH_GAP.saturating_sub(now.duration_since(t)));
        if wait.is_zero() {
            self.props.last_refresh = Some(now);
            self.props_refresh_now(cx);
            return;
        }
        if self.props.refresh_pending {
            return; // the start that is waiting will see the newest entries
        }
        self.props.refresh_pending = true;
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(wait).await;
            let _ = this.update(cx, |v, cx| {
                v.props.refresh_pending = false;
                v.props.last_refresh = Some(Instant::now());
                v.props_refresh_now(cx);
            });
        })
        .detach();
    }

    fn props_refresh_now(&mut self, cx: &mut Context<Self>) {
        if self.props.sort.is_some() {
            self.props_start_job(Purpose::Sort, cx);
        }
        if self.props.group.is_some() {
            self.props_start_job(Purpose::Group, cx);
        }
    }

    /// A different folder is being opened: nothing of the old one goes on.
    pub(super) fn props_reset(&mut self) {
        for j in [self.props.sort_job.take(), self.props.group_job.take()].into_iter().flatten() {
            j.cancel.store(true, Ordering::Relaxed);
        }
        if let Some(q) = &self.props.queue {
            q.clear();
        }
        self.props.sort = None;
        self.props.group = None;
        self.props.plan_key = None;
        self.props.notice = None;
        if self.group == GroupBy::Property {
            self.group = GroupBy::None;
        }
    }

    /// The user sorted by a fixed column (or chose a fixed sort): the sort by a property ends.
    pub(super) fn props_drop_sort(&mut self) {
        if self.props.sort.take().is_some() {
            if let Some(j) = self.props.sort_job.take() {
                j.cancel.store(true, Ordering::Relaxed);
            }
            // the order cached for that sort must not be taken for the fixed sort's
            self.sorted_cache = None;
        }
    }

    /// Another grouping was chosen.
    pub(super) fn props_drop_group(&mut self) {
        self.props.group = None;
        if let Some(j) = self.props.group_job.take() {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// The sort menu changed the direction or the folder order while the list is sorted by a property:
    /// the sort by the property follows. True when it did (the caller then leaves the sorting to it).
    pub(super) fn props_sort_spec_changed(&mut self, spec: SortSpec, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.props.sort.as_mut() else { return false };
        // the same spec again is a click on the fixed sort the menu shows as chosen: it ends the property sort
        if !follows_property_sort(spec, self.sort) {
            return false;
        }
        s.dir = spec.dir;
        self.sort = spec;
        self.sorted_cache = None;
        self.props_start_job(Purpose::Sort, cx);
        true
    }

    /// The fixed sort a click on a fixed column's header starts, as `SortSpec::toggled` does for a new key.
    pub(super) fn props_fixed_sort(&self, key: SortKey) -> SortSpec {
        let dir = match key {
            SortKey::Modified | SortKey::Created | SortKey::Size => SortDir::Desc,
            _ => SortDir::Asc,
        };
        SortSpec { key, dir, ..self.sort }
    }

    /// The list is sorted or grouped by a property that is not among the chosen columns any more.
    /// (Chosen but too wide for the pane is not "gone": the sort is kept, the column comes back with room.)
    fn props_stale(&self, cx: &App) -> bool {
        if self.props.sort.is_none() && self.props.group.is_none() {
            return false;
        }
        let chosen = self.props_chosen(cx);
        prune_needed(self.props.sort.as_ref().map(|s| &*s.name), self.props.group.as_ref().map(|g| &*g.name), |n| chosen.contains(n))
    }

    /// A chosen column was removed while the list was sorted or grouped by it. Does nothing (no rebuild,
    /// no repaint) when that is not so: it is asked after every frame.
    fn props_prune(&mut self, cx: &mut Context<Self>) {
        if !self.props_stale(cx) {
            return;
        }
        let chosen = self.props_chosen(cx);
        if self.props.sort.as_ref().is_some_and(|s| !chosen.contains(&s.name)) {
            self.props_drop_sort();
        }
        if self.props.group.as_ref().is_some_and(|g| !chosen.contains(&g.name)) {
            self.props_drop_group();
            if self.group == GroupBy::Property {
                self.group = GroupBy::None;
            }
        }
        self.rebuild_visible();
        cx.notify();
    }

    /// The status bar's line while values are being read for a sort or grouping, or a notice.
    pub(crate) fn prop_status(&self) -> Option<String> {
        if let Some(j) = self.props.sort_job.as_ref().or(self.props.group_job.as_ref()) {
            let total = j.total.load(Ordering::Relaxed);
            return Some(if total == usize::MAX {
                t!("Özellikler okunuyor…").to_string()
            } else {
                format!("{} {} / {}", t!("Özellikler okunuyor…"), j.done.load(Ordering::Relaxed).min(total), total)
            });
        }
        self.props.notice.as_ref().filter(|(_, at)| at.elapsed().as_secs() < NOTICE_SECS).map(|(m, _)| m.to_string())
    }

    // ---------------- menus

    /// A right click on the details header: the fixed columns, the chosen properties, "Diğer sütunlar…".
    pub(super) fn show_header_menu(&mut self, pos: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let c = cx.global::<crate::settings::Settings>().columns.clone();
        let fixed = |label: &str, on: bool, set: fn(&mut crate::settings::Settings)| {
            Item::action(label.to_string(), MenuIcon::None, move |_, cx| crate::settings::update(cx, set)).checked(on)
        };
        let mut items = vec![
            fixed(t!("Değiştirme tarihi"), c.modified, |s| s.columns.modified = !s.columns.modified),
            fixed(t!("Oluşturma tarihi"), c.created, |s| s.columns.created = !s.columns.created),
            fixed(t!("Tür"), c.kind, |s| s.columns.kind = !s.columns.kind),
            fixed(t!("Boyut"), c.size, |s| s.columns.size = !s.columns.size),
        ];
        let chosen = self.props_chosen(cx);
        if !chosen.is_empty() {
            items.push(Item::Separator);
            for col in chosen.iter() {
                let name = col.name.clone();
                items.push(
                    Item::action(label_of(&col.name), MenuIcon::None, move |_, cx| {
                        let name = name.clone();
                        crate::settings::update(cx, move |s| {
                            let mut c = ChosenColumns::from_setting(&s.columns.properties);
                            c.toggle(&name, pc::DEFAULT_WIDTH);
                            s.columns.properties = c.to_setting();
                        })
                    })
                    .checked(true),
                );
            }
        }
        items.push(Item::Separator);
        // (opened after the menu has closed, so the chooser takes the focus the menu gives back)
        items.push(Item::action(t!("Diğer sütunlar…"), MenuIcon::Lucide(L::Columns3), |window, cx| {
            window.defer(cx, crate::prop_chooser::open)
        }));
        fluent_menu::show(Menu { quick: Vec::new(), items }, pos, window, cx);
    }

    /// The property entries of the Group menu: one per chosen property column, nothing when none is chosen.
    pub(crate) fn property_group_items(&self, me: &WeakEntity<FolderView>, cx: &App) -> Vec<Item> {
        let chosen = self.props_chosen(cx);
        if chosen.is_empty() {
            return Vec::new();
        }
        let mut items = vec![Item::Separator];
        for col in chosen.iter() {
            let name: Arc<str> = Arc::from(col.name.as_str());
            let on = self.group == GroupBy::Property && self.props.group.as_ref().is_some_and(|g| g.name == name);
            let me = me.clone();
            items.push(
                Item::action(label_of(&col.name), MenuIcon::None, move |_, cx| {
                    let _ = me.update(cx, |v, cx| v.props_group_by(name.clone(), cx));
                })
                .checked(on),
            );
        }
        items
    }
}

/// What the rows need to draw the property cells: the shown columns (name, width).
pub(super) struct PropRowCtx {
    cols: Vec<(Arc<str>, u16)>,
}

// ------------------------------------------------------------------ the background job

struct JobInput {
    id: u64,
    purpose: Purpose,
    base: PathBuf,
    entries: Vec<Entry>,
    name: Arc<str>,
    kind: PropKind,
    spec: SortSpec,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

/// What the status line and the cancel of one job share.
struct Progress {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

/// Every file's cell for `name`, in the entries' order (folders and online-only files have none, and
/// neither has a file that ran out of time). The job keeps what it reads in this list itself: the shared
/// cache is bounded and may drop entries while the job runs, and a value dropped there must not turn
/// into "no value" in the order that is made.
fn gather(
    entries: &[Entry],
    base: &Path,
    name: &Arc<str>,
    src: &dyn wp::PropSource,
    cache: &Mutex<PropCache>,
    progress: &Progress,
    ping: &dyn Fn(),
) -> (Vec<PropCell>, wp::BatchEnd) {
    let names = [name.clone()];
    let mut cells: Vec<PropCell> = Vec::with_capacity(entries.len());
    // the files whose value is not in the cache yet
    let mut todo: Vec<usize> = Vec::new();
    for (ci, chunk) in entries.chunks(2048).enumerate() {
        let guard = cache.lock();
        for (k, e) in chunk.iter().enumerate() {
            if e.is_dir() || e.is_cloud_placeholder() {
                cells.push(PropCell::empty());
                continue;
            }
            match guard.get(&base.join(&*e.name), e.size, e.modified_ms, name) {
                Some(c) => cells.push(c.clone()),
                None => {
                    cells.push(PropCell::empty());
                    todo.push(ci * 2048 + k);
                }
            }
        }
    }
    progress.total.store(todo.len(), Ordering::Relaxed);
    ping();
    let mut last_ping = Instant::now();
    let jobs = todo.iter().map(|&i| (i, base.join(&*entries[i].name)));
    let end = wp::read_batch(src, jobs, &names, &progress.cancel, |i, r| {
        let e = &entries[i];
        let path = base.join(&*e.name);
        match r {
            Ok(c) => {
                let cell = c.into_iter().next().unwrap_or_else(PropCell::empty);
                cache.lock().put(path, e.size, e.modified_ms, vec![(name.clone(), cell.clone())]);
                cells[i] = cell;
            }
            // ran out of time: no value for this order, but not stored as one either
            Err(wp::ReadFail::Timeout) => backoff().lock().note(path, e.size, e.modified_ms, Instant::now()),
            Err(_) => cache.lock().put(path, e.size, e.modified_ms, vec![(name.clone(), PropCell::empty())]),
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
        if last_ping.elapsed() >= std::time::Duration::from_millis(250) {
            last_ping = Instant::now();
            ping();
        }
    });
    CACHE_VERSION.fetch_add(1, Ordering::Relaxed);
    (cells, end)
}

fn run_job(job: JobInput, tx: mpsc::UnboundedSender<Ev>) {
    let JobInput { id, purpose, base, entries, name, kind, spec, cancel, done, total } = job;
    let src = wp::SystemSource::new(READ_LIMIT);
    let progress = Progress { cancel, done, total };
    let (cells, end) = gather(&entries, &base, &name, &src, cache(), &progress, &|| {
        let _ = tx.unbounded_send(Ev::Progress);
    });
    let out = match end {
        wp::BatchEnd::Cancelled => JobOutcome::Cancelled,
        wp::BatchEnd::Stalled => JobOutcome::Stalled,
        wp::BatchEnd::Finished => match purpose {
            Purpose::Sort => {
                let values: Vec<PropValue> = cells.into_iter().map(|c| c.value).collect();
                JobOutcome::Sorted(Arc::new(pc::sort_order(&entries, &values, &spec)))
            }
            Purpose::Group => {
                let month = |m: u32| crate::i18n::t(MONTHS[(m as usize).saturating_sub(1).min(11)]).to_string();
                JobOutcome::Grouped(Arc::new(pc::group_keys(&cells, kind, &month)))
            }
        },
    };
    let _ = tx.unbounded_send(Ev::JobEnd(id, out));
}

// ------------------------------------------------------------------ small pure rules

/// The sort or grouping by a property has to end: the column it is by is not chosen any more.
fn prune_needed(sort: Option<&str>, group: Option<&str>, is_chosen: impl Fn(&str) -> bool) -> bool {
    sort.is_some_and(|n| !is_chosen(n)) || group.is_some_and(|n| !is_chosen(n))
}

/// A hash of the files on the rows `first..=last` in their order: when another file is on a row (a
/// sort, a filter or a fold changed the list) the plan of what to read has to be made again.
fn visible_hash(visible: &[u32], first: usize, last: usize) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let end = last.saturating_add(1).min(visible.len());
    visible.get(first.min(end)..end).unwrap_or(&[]).hash(&mut h);
    h.finish()
}

/// The folder's sort once a sort by a property is in force: the same sort, in the property's direction.
fn spec_of_property_sort(sort: SortSpec, dir: SortDir) -> SortSpec {
    SortSpec { dir, ..sort }
}

/// Whether a change in the sort menu is for the property sort (it follows) or ends it. The same spec
/// again is a click on the fixed sort the menu shows as chosen; another key is another sort.
fn follows_property_sort(spec: SortSpec, current: SortSpec) -> bool {
    spec != current && spec.key == current.key
}

/// The open folder is one whose files the property system can read without a network round trip.
fn plain_local_path(p: &Path) -> bool {
    !crate::archive_view::is_browsable_path(p) && !rf_win::ftp::is_ftp(p) && !rf_win::localized::is_network(p)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use parking_lot::Mutex;
    use rf_core::propcols::{PropCache, PropCell, PropValue};
    use rf_core::{Entry, EntryKind, SortDir, SortKey, SortSpec};
    use rf_win::propcols::{FakeSource, ReadFail};

    // (not `use super::*`: that brings gpui's `test` attribute macro into scope and #[test] then loops)
    use super::{
        BACKOFF_CAP, Backoff, JobHandle, PropState, Progress, RETRY_AFTER, backoff, follows_property_sort, gather, plain_local_path, pc, prune_needed,
        spec_of_property_sort, visible_hash, wp,
    };
    use crate::folder_view::GroupBy;

    fn files(n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry::new(&format!("f{i}"), EntryKind::File, 10, 5, 5, 0)).collect()
    }

    fn progress() -> Progress {
        Progress { cancel: Arc::new(AtomicBool::new(false)), done: Arc::new(AtomicUsize::new(0)), total: Arc::new(AtomicUsize::new(usize::MAX)) }
    }

    fn index_of(p: &Path) -> i64 {
        p.file_name().unwrap().to_string_lossy()[1..].parse().unwrap()
    }

    #[test]
    fn a_job_keeps_every_value_it_read_although_the_shared_cache_drops_entries() {
        // a cache that holds 8 files, a folder of 40: most of what is read is evicted again while the job runs
        let cache = Mutex::new(PropCache::with_cap(8));
        let entries = files(40);
        let src = FakeSource::new(|p, _| Ok(vec![PropCell::new("v", PropValue::Int(index_of(p)))]));
        let name: Arc<str> = Arc::from("System.Media.Duration");
        let (cells, end) = gather(&entries, Path::new("C:/rf-prop-gather-test-a"), &name, &src, &cache, &progress(), &|| {});
        assert_eq!(end, wp::BatchEnd::Finished);
        assert!(cache.lock().len() <= 8, "the cache really did drop entries");
        let got: Vec<PropValue> = cells.into_iter().map(|c| c.value).collect();
        assert_eq!(got, (0..40).map(PropValue::Int).collect::<Vec<_>>(), "no value turned into none on the way");
        // and the order made from them is by value, whatever the cache did
        let order = pc::sort_order(&entries, &got, &SortSpec { dir: SortDir::Desc, ..SortSpec::default() });
        assert_eq!(order.first(), Some(&39));
        assert_eq!(order.last(), Some(&0));
    }

    #[test]
    fn a_value_already_in_the_cache_is_used_and_not_read_again() {
        let cache = Mutex::new(PropCache::with_cap(100));
        let entries = files(6);
        let name: Arc<str> = Arc::from("System.Media.Duration");
        let base = Path::new("C:/rf-prop-gather-test-b");
        cache.lock().put(base.join("f2"), 10, 5, vec![(name.clone(), PropCell::new("cached", PropValue::Int(99)))]);
        let src = FakeSource::new(|p, _| Ok(vec![PropCell::new("v", PropValue::Int(index_of(p)))]));
        let p = progress();
        let (cells, _) = gather(&entries, base, &name, &src, &cache, &p, &|| {});
        assert_eq!(src.calls.load(Ordering::SeqCst), 5);
        assert_eq!(p.total.load(Ordering::Relaxed), 5);
        assert_eq!(cells[2].value, PropValue::Int(99));
        assert_eq!(cells[3].value, PropValue::Int(3));
    }

    #[test]
    fn a_file_that_ran_out_of_time_has_no_value_but_is_not_stored_as_having_none() {
        let cache = Mutex::new(PropCache::with_cap(100));
        let entries = files(5);
        let name: Arc<str> = Arc::from("System.Media.Duration");
        let base = Path::new("C:/rf-prop-gather-test-c");
        let src = FakeSource::new(|p, _| if index_of(p) == 2 { Err(ReadFail::Timeout) } else { Ok(vec![PropCell::new("v", PropValue::Int(index_of(p)))]) });
        let (cells, end) = gather(&entries, base, &name, &src, &cache, &progress(), &|| {});
        assert_eq!(end, wp::BatchEnd::Finished);
        assert!(cells[2].value.is_empty());
        assert!(cache.lock().get(&base.join("f2"), 10, 5, &name).is_none(), "a slow answer is not a stored no-value");
        assert!(cache.lock().get(&base.join("f3"), 10, 5, &name).is_some());
        let now = Instant::now();
        assert!(backoff().lock().is_waiting(&base.join("f2"), 10, 5, now), "it is not asked again at once");
        assert!(!backoff().lock().is_waiting(&base.join("f2"), 10, 5, now + RETRY_AFTER + Duration::from_secs(1)), "but after a while");
        assert!(!backoff().lock().is_waiting(&base.join("f2"), 11, 5, now), "a changed file is a new file");
        assert!(!backoff().lock().is_waiting(&base.join("f3"), 10, 5, now));
    }

    #[test]
    fn a_job_stops_when_it_is_cancelled() {
        let cache = Mutex::new(PropCache::with_cap(100));
        let entries = files(50);
        let name: Arc<str> = Arc::from("System.Media.Duration");
        let p = progress();
        let cancel = p.cancel.clone();
        let src = FakeSource::new(move |f, _| {
            if index_of(f) == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
            Ok(vec![PropCell::new("v", PropValue::Int(1))])
        });
        let (_, end) = gather(&entries, Path::new("C:/rf-prop-gather-test-d"), &name, &src, &cache, &p, &|| {});
        assert_eq!(end, wp::BatchEnd::Cancelled);
        assert_eq!(src.calls.load(Ordering::SeqCst), 4, "nothing was read after the cancel");
    }

    #[test]
    fn a_sort_by_a_column_that_is_chosen_but_does_not_fit_the_pane_is_not_dropped() {
        // chosen, only not drawn (the pane is narrow): the sort stays, so no prune, no rebuild, no repaint, no loop
        let chosen = |n: &str| n == "System.Media.Duration";
        assert!(!prune_needed(Some("System.Media.Duration"), None, chosen));
        assert!(!prune_needed(Some("System.Media.Duration"), Some("System.Media.Duration"), chosen));
        assert!(!prune_needed(None, None, chosen));
        // the column was unchosen
        assert!(prune_needed(Some("System.Music.Artist"), None, chosen));
        assert!(prune_needed(Some("System.Media.Duration"), Some("System.Music.Artist"), chosen));
        // nothing chosen at all
        assert!(prune_needed(Some("System.Media.Duration"), None, |_| false));
    }

    #[test]
    fn other_files_on_the_same_rows_change_the_plan_key() {
        let a: Vec<u32> = (0..100).collect();
        let mut b = a.clone();
        b.reverse(); // a sort by another column puts other files on rows 0..40
        assert_eq!(visible_hash(&a, 0, 40), visible_hash(&a, 0, 40));
        assert_ne!(visible_hash(&a, 0, 40), visible_hash(&b, 0, 40));
        // rows far below the shown ones are not part of it
        let mut c = a.clone();
        c[90] = 5000;
        assert_eq!(visible_hash(&a, 0, 40), visible_hash(&c, 0, 40));
        // the end of the list and an empty list do not panic
        let _ = visible_hash(&a, 95, 200);
        let _ = visible_hash(&a, 300, 400);
        let _ = visible_hash(&[], 0, 10);
    }

    #[test]
    fn closing_the_view_cancels_its_background_reads() {
        let handle = |id: u64, flag: &Arc<AtomicBool>| JobHandle {
            id,
            cancel: flag.clone(),
            done: Arc::new(AtomicUsize::new(0)),
            total: Arc::new(AtomicUsize::new(0)),
            entries_gen: 0,
        };
        let (sort_flag, group_flag) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
        let mut st = PropState::default();
        st.sort_job = Some(handle(1, &sort_flag));
        st.group_job = Some(handle(2, &group_flag));
        assert!(!sort_flag.load(Ordering::Relaxed));
        drop(st);
        assert!(sort_flag.load(Ordering::Relaxed) && group_flag.load(Ordering::Relaxed));
    }

    #[test]
    fn the_sort_menu_follows_the_direction_of_the_property_sort() {
        // the folder's own sort is Name ascending; a sort by Duration descending has been applied
        let fixed = SortSpec { key: SortKey::Name, dir: SortDir::Asc, ..SortSpec::default() };
        let in_force = spec_of_property_sort(fixed, SortDir::Desc);
        assert_eq!(in_force.dir, SortDir::Desc);
        assert_eq!(in_force.key, SortKey::Name);
        let menu = |dir: SortDir| SortSpec { dir, ..in_force };
        // the menu shows Azalan as chosen; Artan is a real change of the property sort
        assert!(follows_property_sort(menu(SortDir::Asc), in_force), "ascending sorts the property ascending");
        assert!(!follows_property_sort(menu(SortDir::Desc), in_force), "the chosen item ends the property sort");
        // another key is another sort
        assert!(!follows_property_sort(SortSpec { key: SortKey::Size, ..in_force }, in_force));
    }

    #[test]
    fn only_plain_local_folders_get_property_columns() {
        assert!(!plain_local_path(Path::new(r"\\server\share\music")));
        assert!(!plain_local_path(Path::new(r"\\?\UNC\server\share\music")));
        assert!(plain_local_path(&std::env::temp_dir()));
    }

    #[test]
    fn a_backoff_does_not_grow_without_end() {
        let mut b = Backoff::default();
        let now = Instant::now();
        for i in 0..BACKOFF_CAP + 10 {
            b.note(PathBuf::from(format!("f{i}")), 1, 1, now);
        }
        assert!(b.files.len() <= BACKOFF_CAP);
        assert!(b.is_waiting(Path::new(&format!("f{}", BACKOFF_CAP + 9)), 1, 1, now));
    }

    #[test]
    fn the_chosen_properties_are_in_the_settings_file_only_when_there_are_some() {
        let mut s = crate::settings::Settings::default();
        let columns_has_key = |s: &crate::settings::Settings| {
            let table = toml::Table::try_from(s).unwrap();
            table["columns"].as_table().unwrap().contains_key("properties")
        };
        assert!(!columns_has_key(&s), "with none chosen the file is as it was before");
        s.columns.properties = vec!["System.Media.Duration:140".into(), "System.Music.Artist:130".into()];
        assert!(columns_has_key(&s));
        let text = toml::to_string_pretty(&s).unwrap();
        let back = crate::settings::Settings::parse_lenient_pub(&text).unwrap();
        assert_eq!(back.columns.properties, s.columns.properties);
        // a file written before the setting existed has no key and reads as none chosen
        let old = crate::settings::Settings::parse_lenient_pub("[columns]\nsize = false\n").unwrap();
        assert!(old.columns.properties.is_empty() && !old.columns.size);
    }

    #[test]
    fn a_grouping_by_a_property_is_not_stored_with_a_folders_layout() {
        assert_eq!(GroupBy::Property.index(), 0, "it is remembered as no grouping");
        assert!(GroupBy::ALL.iter().all(|(g, _)| *g != GroupBy::Property));
        assert!(GroupBy::Property.needs_extra());
    }
}
