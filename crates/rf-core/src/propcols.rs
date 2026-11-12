//! Details columns for any Windows property (duration, artist, date taken, dimensions ...): the pure
//! part. Nothing here touches the disk or Windows; `rf_win::propcols` reads the values and the
//! folder view shows them.
//!
//! * [`ChosenColumns`]: the list of chosen properties and its settings form.
//! * [`PropValue`] / [`PropCell`]: a value typed for ordering, and the text Explorer would show.
//! * [`sort_order`], [`group_keys`]: sorting and grouping by a property once every value is in.
//! * [`PropCache`]: values per file, valid while the file's size and modified time stay the same.
//! * [`plan`]: which rows to read next for the part of the list that is on screen.
//! * [`chooser_rows`]: the order and filter of the property chooser.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::entry::Entry;
use crate::sort::{SortDir, SortSpec, lower, natural_cmp};

// ------------------------------------------------------------------ the chosen columns

/// More columns than this would leave no room for the name in any window.
pub const MAX_COLUMNS: usize = 8;
pub const DEFAULT_WIDTH: u16 = 130;
pub const MIN_WIDTH: u16 = 70;
pub const MAX_WIDTH: u16 = 600;

/// A property's canonical name looks like `System.Media.Duration`. Anything else in the settings file
/// is dropped instead of being handed to the property system.
pub fn is_canonical_name(s: &str) -> bool {
    (3..=160).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_alphabetic())
        && !s.ends_with('.')
        && !s.contains("..")
        && s.contains('.')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChosenColumn {
    /// Canonical property name.
    pub name: String,
    pub width: u16,
}

/// The properties the user turned into columns, in the order they show.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChosenColumns {
    cols: Vec<ChosenColumn>,
}

impl ChosenColumns {
    /// From the settings form: one string per column, `System.Media.Duration` or `System.Media.Duration:140`.
    /// Bad names, repeats and everything past [`MAX_COLUMNS`] are dropped; widths are kept in range.
    pub fn from_setting(items: &[String]) -> Self {
        let mut cols: Vec<ChosenColumn> = Vec::new();
        for item in items {
            let (name, width) = match item.rsplit_once(':') {
                Some((n, w)) => (n, w.trim().parse::<u16>().unwrap_or(DEFAULT_WIDTH)),
                None => (item.as_str(), DEFAULT_WIDTH),
            };
            let name = name.trim();
            if !is_canonical_name(name) || cols.iter().any(|c| c.name == name) || cols.len() >= MAX_COLUMNS {
                continue;
            }
            cols.push(ChosenColumn { name: name.to_string(), width: width.clamp(MIN_WIDTH, MAX_WIDTH) });
        }
        Self { cols }
    }

    /// The settings form (see [`Self::from_setting`]); the default width is written out too, so a later
    /// change of the default does not move a column the user saw.
    pub fn to_setting(&self) -> Vec<String> {
        self.cols.iter().map(|c| format!("{}:{}", c.name, c.width)).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.cols.is_empty()
    }

    pub fn len(&self) -> usize {
        self.cols.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = &ChosenColumn> {
        self.cols.iter()
    }

    pub fn get(&self, ix: usize) -> Option<&ChosenColumn> {
        self.cols.get(ix)
    }

    pub fn contains(&self, name: &str) -> bool {
        self.cols.iter().any(|c| c.name == name)
    }

    pub fn position(&self, name: &str) -> Option<usize> {
        self.cols.iter().position(|c| c.name == name)
    }

    pub fn names(&self) -> Vec<Arc<str>> {
        self.cols.iter().map(|c| Arc::from(c.name.as_str())).collect()
    }

    /// Adds the column when it is not there, removes it when it is. Returns whether it is chosen now;
    /// adding past [`MAX_COLUMNS`] or a bad name changes nothing and returns false.
    pub fn toggle(&mut self, name: &str, width: u16) -> bool {
        if let Some(i) = self.position(name) {
            self.cols.remove(i);
            return false;
        }
        if self.cols.len() >= MAX_COLUMNS || !is_canonical_name(name) {
            return false;
        }
        self.cols.push(ChosenColumn { name: name.to_string(), width: width.clamp(MIN_WIDTH, MAX_WIDTH) });
        true
    }

    pub fn set_width(&mut self, ix: usize, width: u16) {
        if let Some(c) = self.cols.get_mut(ix) {
            c.width = width.clamp(MIN_WIDTH, MAX_WIDTH);
        }
    }

    /// Moves a column to a new place (the others close up).
    pub fn move_to(&mut self, from: usize, to: usize) {
        if from < self.cols.len() && to < self.cols.len() && from != to {
            let c = self.cols.remove(from);
            self.cols.insert(to, c);
        }
    }
}

/// How many of the columns, taken in order, fit into `room` pixels (a column is shown whole or not at all).
pub fn fit_count(widths: impl IntoIterator<Item = u16>, room: f32) -> usize {
    let mut left = room;
    let mut n = 0;
    for w in widths {
        if f32::from(w) > left {
            break;
        }
        left -= f32::from(w);
        n += 1;
    }
    n
}

/// A readable header for a property whose display name is not known yet: the last part of the canonical
/// name with spaces between the words ("System.Photo.DateTaken" -> "Date Taken").
pub fn fallback_label(canonical: &str) -> String {
    let last = canonical.rsplit('.').next().unwrap_or(canonical);
    let mut out = String::with_capacity(last.len() + 4);
    let mut prev_lower = false;
    for c in last.chars() {
        if c.is_ascii_uppercase() && prev_lower {
            out.push(' ');
        }
        prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        out.push(c);
    }
    out
}

// ------------------------------------------------------------------ values

/// What kind of value a property holds. It decides how a cell is aligned and how groups are cut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PropKind {
    #[default]
    Text,
    Number,
    DateTime,
    Bool,
}

/// A property value in the form that orders correctly. Dates are Unix milliseconds, booleans 0 and 1.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum PropValue {
    /// The file has no value for the property (or it could not be read).
    #[default]
    Empty,
    Int(i64),
    Float(f64),
    Text(Arc<str>),
}

impl PropValue {
    /// A float; NaN is no value.
    pub fn float(f: f64) -> Self {
        if f.is_nan() { PropValue::Empty } else { PropValue::Float(f) }
    }

    /// A text; an empty one is no value.
    pub fn text(s: &str) -> Self {
        if s.is_empty() { PropValue::Empty } else { PropValue::Text(s.into()) }
    }

    pub fn is_empty(&self) -> bool {
        matches!(self, PropValue::Empty)
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            PropValue::Int(i) => Some(*i as f64),
            PropValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    /// Order of two values that are both present: numbers as numbers, text with the app's natural order,
    /// numbers before text when a property mixes the two.
    pub fn cmp_present(&self, other: &Self) -> Ordering {
        match (self, other) {
            (PropValue::Int(a), PropValue::Int(b)) => a.cmp(b),
            (PropValue::Text(a), PropValue::Text(b)) => natural_cmp(a, b),
            (PropValue::Empty, PropValue::Empty) => Ordering::Equal,
            _ => match (self.as_f64(), other.as_f64()) {
                (Some(a), Some(b)) => a.partial_cmp(&b).unwrap_or(Ordering::Equal),
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (None, None) => Ordering::Equal,
            },
        }
    }
}

/// The order of two values in a sort: values by `dir`, files without a value after the ones with one in
/// both directions (a descending sort does not put the empty ones first).
pub fn compare_values(a: &PropValue, b: &PropValue, dir: SortDir) -> Ordering {
    match (a.is_empty(), b.is_empty()) {
        (true, true) => Ordering::Equal,
        (true, false) => Ordering::Greater,
        (false, true) => Ordering::Less,
        (false, false) => {
            let o = a.cmp_present(b);
            if dir == SortDir::Desc { o.reverse() } else { o }
        }
    }
}

/// One file's value for one property: the text Explorer shows and the value to order by.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PropCell {
    pub text: Arc<str>,
    pub value: PropValue,
}

impl PropCell {
    pub fn empty() -> Self {
        Self { text: Arc::from(""), value: PropValue::Empty }
    }

    pub fn new(text: &str, value: PropValue) -> Self {
        Self { text: Arc::from(text), value }
    }
}

// ------------------------------------------------------------------ sorting and grouping

/// Every entry's index in the order of a sort by a property. `values` is aligned with `entries`.
/// Folders and files keep the order of the folder settings (folders first / files first), then the
/// values by `spec.dir` with the empty ones last, then the names.
pub fn sort_order(entries: &[Entry], values: &[PropValue], spec: &SortSpec) -> Vec<u32> {
    let empty = PropValue::Empty;
    let mut idx: Vec<u32> = (0..entries.len() as u32).collect();
    idx.sort_unstable_by(|&x, &y| {
        let (a, b) = (&entries[x as usize], &entries[y as usize]);
        if spec.dirs_first && a.is_dir() != b.is_dir() {
            return if a.is_dir() { Ordering::Less } else { Ordering::Greater };
        }
        if spec.files_first && a.is_dir() != b.is_dir() {
            return if a.is_dir() { Ordering::Greater } else { Ordering::Less };
        }
        let (va, vb) = (values.get(x as usize).unwrap_or(&empty), values.get(y as usize).unwrap_or(&empty));
        compare_values(va, vb, spec.dir).then_with(|| {
            let o = natural_cmp(a.shown(), b.shown());
            if spec.dir == SortDir::Desc { o.reverse() } else { o }
        })
    });
    idx
}

/// The group of files that have no value.
pub const UNSPECIFIED: &str = "Belirtilmemiş";

/// The group (order, label) of every entry for a grouping by a property. `cells` is aligned with the
/// entries. Dates group by calendar month (newest first; `month_name` gives a month's name, 1 to 12);
/// numbers and text group by their value, and when a property has many distinct values the numbers
/// are cut into ten groups of the same size and the text is grouped by its first letter.
pub fn group_keys(cells: &[PropCell], kind: PropKind, month_name: &dyn Fn(u32) -> String) -> Vec<(u32, String)> {
    const MANY: usize = 30;
    const MANY_TEXT: usize = 200;
    let none = (u32::MAX, UNSPECIFIED.to_string());
    if kind == PropKind::DateTime {
        use chrono::{Datelike, TimeZone};
        return cells
            .iter()
            .map(|c| match &c.value {
                PropValue::Int(ms) if *ms > 0 => match chrono::Local.timestamp_millis_opt(*ms).single() {
                    Some(t) => {
                        let (y, m) = (t.year().clamp(0, 9999) as u32, t.month());
                        ((10_000 - y) * 12 + (12 - m), format!("{} {y}", month_name(m)))
                    }
                    None => none.clone(),
                },
                _ => none.clone(),
            })
            .collect();
    }
    // distinct present values in order, each with the rank it takes
    let mut present: Vec<usize> = (0..cells.len()).filter(|&i| !cells[i].value.is_empty()).collect();
    present.sort_by(|&a, &b| cells[a].value.cmp_present(&cells[b].value));
    let mut rank_of = vec![u32::MAX; cells.len()];
    let mut rank = 0u32;
    let mut distinct: Vec<usize> = Vec::new(); // first cell of each rank
    for (k, &i) in present.iter().enumerate() {
        if k > 0 && cells[present[k - 1]].value.cmp_present(&cells[i].value) != Ordering::Equal {
            rank += 1;
        }
        if k == 0 || rank as usize == distinct.len() {
            distinct.push(i);
        }
        rank_of[i] = rank;
    }
    let numeric = cells.iter().any(|c| matches!(c.value, PropValue::Int(_) | PropValue::Float(_)));
    let mut out = vec![none; cells.len()];
    if numeric && distinct.len() > MANY {
        // ten groups with about as many files each: the label is the range the group covers. A group ends
        // only where the value changes, so files with the same value are never split over two groups.
        let n = present.len();
        let step = n.div_ceil(10).max(1);
        let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
        let mut start = 0;
        for k in 1..=n {
            let boundary = k == n || (k - start >= step && rank_of[present[k]] != rank_of[present[k - 1]]);
            if boundary {
                groups.push(start..k);
                start = k;
            }
        }
        for (b, g) in groups.into_iter().enumerate() {
            let chunk = &present[g];
            let (lo, hi) = (&cells[chunk[0]].text, &cells[chunk[chunk.len() - 1]].text);
            let label = if lo == hi { lo.to_string() } else { format!("{lo} – {hi}") };
            for &i in chunk {
                out[i] = (b as u32, label.clone());
            }
        }
    } else if !numeric && distinct.len() > MANY_TEXT {
        for &i in &present {
            let first = cells[i].text.chars().next().unwrap_or(' ');
            let label = if first.is_ascii_digit() { "0–9".to_string() } else { crate::sort::upper(&first.to_string()) };
            // letters in the order of their upper-case form; digits before them
            let order = if first.is_ascii_digit() { 0 } else { 1 + label.chars().next().map_or(0, |c| c as u32) };
            out[i] = (order, label);
        }
    } else {
        for &i in &present {
            out[i] = (rank_of[i], cells[i].text.to_string());
        }
    }
    out
}

// ------------------------------------------------------------------ cache

/// Most files the cache keeps; past it an eighth of them is dropped to make room (a folder of 100 000
/// files read for a sort fits, and a long session does not grow without end). About 300 bytes a file and 100 more for each
/// property read, so at most some 60 MB.
pub const CACHE_CAP: usize = 150_000;

#[derive(Debug)]
struct Record {
    size: u64,
    modified_ms: i64,
    cells: Vec<(Arc<str>, PropCell)>,
}

/// Values per file and property. An entry is valid while the file's size and modified time are the
/// ones it was read with; a changed file reads as never read.
#[derive(Debug)]
pub struct PropCache {
    files: HashMap<PathBuf, Record>,
    cap: usize,
}

impl Default for PropCache {
    fn default() -> Self {
        Self::with_cap(CACHE_CAP)
    }
}

impl PropCache {
    /// A cache that keeps at most `cap` files (tests use a small one).
    pub fn with_cap(cap: usize) -> Self {
        Self { files: HashMap::new(), cap: cap.max(1) }
    }

    pub fn get(&self, path: &Path, size: u64, modified_ms: i64, name: &str) -> Option<&PropCell> {
        let r = self.files.get(path)?;
        if r.size != size || r.modified_ms != modified_ms {
            return None;
        }
        r.cells.iter().find(|(n, _)| &**n == name).map(|(_, c)| c)
    }

    /// Whether every one of `names` is there for this file as it is now.
    pub fn has_all(&self, path: &Path, size: u64, modified_ms: i64, names: &[Arc<str>]) -> bool {
        match self.files.get(path) {
            Some(r) if r.size == size && r.modified_ms == modified_ms => names.iter().all(|n| r.cells.iter().any(|(m, _)| m == n)),
            _ => false,
        }
    }

    /// Stores values read for a file. Values of the same file and stamp are kept next to the new ones;
    /// a file that changed starts over.
    pub fn put(&mut self, path: PathBuf, size: u64, modified_ms: i64, cells: Vec<(Arc<str>, PropCell)>) {
        if self.files.len() >= self.cap && !self.files.contains_key(&path) {
            // an eighth goes, not everything: the rows on screen mostly stay, and a reader that is
            // halfway through a folder loses little. (A sort or grouping job keeps what it read in its
            // own list, so what is evicted here never changes its result.)
            let mut drop_n = (self.cap / 8).max(1);
            self.files.retain(|_, _| {
                if drop_n > 0 {
                    drop_n -= 1;
                    false
                } else {
                    true
                }
            });
        }
        let r = self.files.entry(path).or_insert_with(|| Record { size, modified_ms, cells: Vec::new() });
        if r.size != size || r.modified_ms != modified_ms {
            *r = Record { size, modified_ms, cells: Vec::new() };
        }
        for (name, cell) in cells {
            match r.cells.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 = cell,
                None => r.cells.push((name, cell)),
            }
        }
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }
}

// ------------------------------------------------------------------ what to read next

/// Rows to read for a list showing `visible`: the visible ones first, then `margin` rows after them
/// (the direction of scrolling is down more often than up), then `margin` rows before. At most `limit`
/// rows, and only those `done` says are still missing.
pub fn plan(visible: Range<usize>, margin: usize, len: usize, mut done: impl FnMut(usize) -> bool, limit: usize) -> Vec<usize> {
    let start = visible.start.min(len);
    let end = visible.end.min(len);
    let after_end = (end + margin).min(len);
    let before_start = start.saturating_sub(margin);
    let mut out = Vec::new();
    let order = (start..end).chain(end..after_end).chain((before_start..start).rev());
    for i in order {
        if out.len() >= limit {
            break;
        }
        if !done(i) {
            out.push(i);
        }
    }
    out
}

/// How far a read of every file has come.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
}

impl Progress {
    pub fn percent(self) -> u32 {
        if self.total == 0 { 100 } else { ((self.done.min(self.total) as u64 * 100) / self.total as u64) as u32 }
    }
}

// ------------------------------------------------------------------ the chooser

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Pictures,
    Music,
    Video,
    Documents,
}

/// The properties offered first, per kind of file.
pub const COMMON: &[(Category, &[&str])] = &[
    (
        Category::Pictures,
        &["System.Image.Dimensions", "System.Photo.DateTaken", "System.Photo.CameraModel", "System.Photo.CameraManufacturer", "System.Image.BitDepth"],
    ),
    (
        Category::Music,
        &[
            "System.Music.Artist",
            "System.Music.AlbumTitle",
            "System.Title",
            "System.Music.TrackNumber",
            "System.Media.Duration",
            "System.Audio.EncodingBitrate",
            "System.Music.Genre",
            "System.Media.Year",
        ],
    ),
    (
        Category::Video,
        &["System.Media.Duration", "System.Video.FrameWidth", "System.Video.FrameHeight", "System.Video.FrameRate", "System.Video.TotalBitrate"],
    ),
    (Category::Documents, &["System.Author", "System.Title", "System.Document.PageCount", "System.Document.WordCount"]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserRow {
    /// A heading: a kind of file, or `None` for "all the other properties".
    Heading(Option<Category>),
    /// Index into the list the rows were made from.
    Item(usize),
}

/// The rows of the property chooser. `all` is every property as (canonical name, display name), in the
/// order they should show under "all". With no query: a heading and the common ones per kind of file,
/// then a heading and everything else. With a query: only the matches (display name or canonical name
/// contains it, any case), the common ones first.
pub fn chooser_rows(all: &[(String, String)], query: &str) -> Vec<ChooserRow> {
    let q = lower(query.trim());
    let find = |name: &str| all.iter().position(|(c, _)| c == name);
    if q.is_empty() {
        let mut rows = Vec::new();
        let mut common_seen = std::collections::HashSet::new();
        for (cat, names) in COMMON {
            let items: Vec<usize> = names.iter().filter_map(|n| find(n)).collect();
            if items.is_empty() {
                continue;
            }
            rows.push(ChooserRow::Heading(Some(*cat)));
            for i in items {
                common_seen.insert(i);
                rows.push(ChooserRow::Item(i));
            }
        }
        rows.push(ChooserRow::Heading(None));
        rows.extend((0..all.len()).filter(|i| !common_seen.contains(i)).map(ChooserRow::Item));
        return rows;
    }
    let matches = |i: usize| lower(&all[i].1).contains(&q) || lower(&all[i].0).contains(&q);
    let mut rows = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (_, names) in COMMON {
        for n in *names {
            if let Some(i) = find(n)
                && matches(i)
                && seen.insert(i)
            {
                rows.push(ChooserRow::Item(i));
            }
        }
    }
    rows.extend((0..all.len()).filter(|&i| matches(i) && !seen.contains(&i)).map(ChooserRow::Item));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::EntryKind;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn settings_form_round_trips_and_drops_garbage() {
        let c = ChosenColumns::from_setting(&s(&[
            "System.Media.Duration:140",
            "System.Music.Artist",
            "System.Media.Duration:200", // repeat
            "not a name",
            "System.Author:5000", // width out of range
            "System.Title:abc",    // width not a number
            "..:5",
        ]));
        assert_eq!(c.len(), 4);
        assert_eq!(c.to_setting(), s(&["System.Media.Duration:140", "System.Music.Artist:130", "System.Author:600", "System.Title:130"]));
        let again = ChosenColumns::from_setting(&c.to_setting());
        assert_eq!(again, c);
        assert!(ChosenColumns::from_setting(&[]).is_empty());
    }

    #[test]
    fn columns_are_capped_toggled_and_moved() {
        let mut c = ChosenColumns::default();
        for i in 0..MAX_COLUMNS + 3 {
            c.toggle(&format!("System.Test.P{i}"), 100);
        }
        assert_eq!(c.len(), MAX_COLUMNS);
        assert!(!c.toggle("System.Test.P0", 100), "toggling a chosen column removes it");
        assert_eq!(c.len(), MAX_COLUMNS - 1);
        assert!(c.toggle("System.Test.New", 5), "room again");
        assert_eq!(c.get(c.len() - 1).unwrap().width, MIN_WIDTH, "a width is kept in range");
        c.move_to(0, 2);
        assert_eq!(c.get(2).unwrap().name, "System.Test.P1");
        c.set_width(0, 5000);
        assert_eq!(c.get(0).unwrap().width, MAX_WIDTH);
        assert!(!c.toggle("bad name", 100));
    }

    #[test]
    fn fitting_takes_whole_columns_in_order() {
        assert_eq!(fit_count([100, 100, 100], 250.), 2);
        assert_eq!(fit_count([100, 300, 50], 250.), 1, "a column that does not fit stops the rest");
        assert_eq!(fit_count([], 250.), 0);
        assert_eq!(fit_count([100], 99.), 0);
    }

    #[test]
    fn a_label_is_made_from_the_canonical_name() {
        assert_eq!(fallback_label("System.Photo.DateTaken"), "Date Taken");
        assert_eq!(fallback_label("System.Title"), "Title");
        assert_eq!(fallback_label("System.Video.FrameRate"), "Frame Rate");
    }

    #[test]
    fn numbers_order_as_numbers_text_naturally_and_empty_values_come_last_both_ways() {
        let vals = [PropValue::Int(10), PropValue::Int(9), PropValue::Float(9.5), PropValue::Empty, PropValue::text("b2"), PropValue::text("b10"), PropValue::Empty];
        let mut asc: Vec<usize> = (0..vals.len()).collect();
        asc.sort_by(|&a, &b| compare_values(&vals[a], &vals[b], SortDir::Asc));
        // 9, 9.5, 10, then text "b2" < "b10", then the two empty ones
        assert_eq!(asc, vec![1, 2, 0, 4, 5, 3, 6]);
        let mut desc: Vec<usize> = (0..vals.len()).collect();
        desc.sort_by(|&a, &b| compare_values(&vals[a], &vals[b], SortDir::Desc));
        assert_eq!(&desc[..5], &[5, 4, 0, 2, 1]);
        assert!(vals[desc[5]].is_empty() && vals[desc[6]].is_empty(), "empty values stay last when descending");
        assert_eq!(PropValue::float(f64::NAN), PropValue::Empty);
        assert_eq!(PropValue::text(""), PropValue::Empty);
        // a number is a number: 100 is more than 20 even though "100" < "20" as text
        assert_eq!(PropValue::Int(100).cmp_present(&PropValue::Int(20)), Ordering::Greater);
    }

    fn file(name: &str) -> Entry {
        Entry::new(name, EntryKind::File, 1, 1, 1, 0)
    }
    fn dir(name: &str) -> Entry {
        Entry::new(name, EntryKind::Dir, 0, 1, 1, 0x10)
    }

    #[test]
    fn a_sort_keeps_folders_first_puts_empty_last_and_breaks_ties_by_name() {
        let entries = vec![file("c.mp3"), dir("album"), file("a.mp3"), file("b.mp3"), file("d.mp3")];
        let values = vec![PropValue::Int(200), PropValue::Empty, PropValue::Int(200), PropValue::Int(100), PropValue::Empty];
        let mut spec = SortSpec { dir: SortDir::Asc, ..SortSpec::default() };
        // folder, 100, then the two 200s by name, then the empty one
        assert_eq!(sort_order(&entries, &values, &spec), vec![1, 3, 2, 0, 4]);
        spec.dir = SortDir::Desc;
        assert_eq!(sort_order(&entries, &values, &spec), vec![1, 0, 2, 3, 4]);
        spec.dirs_first = false;
        spec.files_first = true;
        assert_eq!(*sort_order(&entries, &values, &spec).last().unwrap(), 1, "files first puts the folder last");
        // values shorter than the entries: the missing ones are empty
        assert_eq!(sort_order(&entries, &values[..2], &SortSpec::default()).len(), 5);
    }

    fn cell(text: &str, v: PropValue) -> PropCell {
        PropCell::new(text, v)
    }

    #[test]
    fn groups_by_value_with_files_without_a_value_in_their_own_last_group() {
        let cells = vec![cell("Rock", PropValue::text("Rock")), cell("", PropValue::Empty), cell("Jazz", PropValue::text("Jazz")), cell("Rock", PropValue::text("Rock"))];
        let g = group_keys(&cells, PropKind::Text, &|m| m.to_string());
        assert_eq!(g[0], g[3]);
        assert!(g[2].0 < g[0].0, "Jazz sorts before Rock");
        assert_eq!(g[1], (u32::MAX, UNSPECIFIED.to_string()));
    }

    #[test]
    fn many_numbers_are_cut_into_ten_groups_and_many_texts_group_by_first_letter() {
        let nums: Vec<PropCell> = (0..100).map(|i| cell(&format!("{i} s"), PropValue::Int(i))).collect();
        let g = group_keys(&nums, PropKind::Number, &|m| m.to_string());
        let mut labels: Vec<_> = g.iter().map(|x| x.1.clone()).collect();
        labels.dedup();
        assert_eq!(labels.len(), 10);
        assert_eq!(g[0].1, "0 s – 9 s");
        assert!(g.windows(2).all(|w| w[0].0 <= w[1].0), "the groups follow the values");
        let texts: Vec<PropCell> = (0..300).map(|i| cell(&format!("{}{i}", if i % 2 == 0 { "alfa" } else { "Beta" }), PropValue::text(&format!("{}{i}", if i % 2 == 0 { "alfa" } else { "Beta" })))).collect();
        let g = group_keys(&texts, PropKind::Text, &|m| m.to_string());
        let mut labels: Vec<_> = g.iter().map(|x| x.1.clone()).collect();
        labels.sort();
        labels.dedup();
        assert_eq!(labels, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn dates_group_by_month_newest_first() {
        use chrono::TimeZone;
        let ms = |y, m, d| chrono::Local.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().timestamp_millis();
        let cells = vec![
            cell("a", PropValue::Int(ms(2024, 3, 5))),
            cell("b", PropValue::Int(ms(2026, 1, 9))),
            cell("c", PropValue::Int(ms(2024, 3, 28))),
            cell("", PropValue::Empty),
        ];
        let g = group_keys(&cells, PropKind::DateTime, &|m| format!("M{m}"));
        assert_eq!(g[0], g[2]);
        assert_eq!(g[0].1, "M3 2024");
        assert!(g[1].0 < g[0].0, "2026 before 2024");
        assert_eq!(g[3].0, u32::MAX);
    }

    #[test]
    fn the_cache_forgets_a_file_that_changed() {
        let mut c = PropCache::default();
        let p = PathBuf::from("a.mp3");
        let names: Vec<Arc<str>> = vec![Arc::from("System.Title"), Arc::from("System.Music.Artist")];
        c.put(p.clone(), 10, 5, vec![(names[0].clone(), cell("T", PropValue::text("T")))]);
        assert_eq!(c.get(&p, 10, 5, "System.Title").unwrap().text.as_ref(), "T");
        assert!(!c.has_all(&p, 10, 5, &names), "the artist was never read");
        c.put(p.clone(), 10, 5, vec![(names[1].clone(), PropCell::empty())]);
        assert!(c.has_all(&p, 10, 5, &names), "an empty answer counts as read");
        assert!(c.get(&p, 11, 5, "System.Title").is_none(), "another size");
        assert!(c.get(&p, 10, 6, "System.Title").is_none(), "another time");
        c.put(p.clone(), 11, 5, vec![(names[0].clone(), cell("U", PropValue::text("U")))]);
        assert!(c.get(&p, 10, 5, "System.Music.Artist").is_none(), "a changed file starts over");
        assert_eq!(c.get(&p, 11, 5, "System.Title").unwrap().text.as_ref(), "U");
        assert_eq!(c.len(), 1);
        c.clear();
        assert!(c.is_empty());
    }

    #[test]
    fn the_cache_drops_an_eighth_at_its_cap_not_everything() {
        let mut c = PropCache::with_cap(80);
        for i in 0..80 {
            c.put(PathBuf::from(format!("f{i}")), 1, 1, Vec::new());
        }
        assert_eq!(c.len(), 80);
        c.put(PathBuf::from("one more"), 1, 1, Vec::new());
        assert_eq!(c.len(), 80 - 10 + 1, "ten went, the new one is in");
        // a file that is already there never makes room
        let some = c.files.keys().next().unwrap().clone();
        c.put(some, 1, 1, Vec::new());
        assert_eq!(c.len(), 71);
        assert_eq!(PropCache::default().cap, CACHE_CAP);
    }

    #[test]
    fn files_with_the_same_value_are_never_split_over_two_number_groups() {
        // 45 distinct values; the value 20 is there 12 times, across where a cut of 6 would fall
        let mut vals: Vec<i64> = (0..45).collect();
        vals.extend(std::iter::repeat_n(20, 11));
        let cells: Vec<PropCell> = vals.iter().map(|v| cell(&format!("{v} s"), PropValue::Int(*v))).collect();
        let g = group_keys(&cells, PropKind::Number, &|m| m.to_string());
        let of_20: std::collections::HashSet<&(u32, String)> = vals.iter().zip(&g).filter(|(v, _)| **v == 20).map(|(_, k)| k).collect();
        assert_eq!(of_20.len(), 1, "every file with the value 20 is in one group");
        let mut labels: Vec<&String> = g.iter().map(|x| &x.1).collect();
        labels.dedup();
        assert!(labels.len() <= 10 && labels.len() >= 5, "{} groups", labels.len());
        let mut by_value: Vec<usize> = (0..vals.len()).collect();
        by_value.sort_by_key(|&i| vals[i]);
        assert!(by_value.windows(2).all(|w| g[w[0]].0 <= g[w[1]].0), "the groups follow the values");
    }

    #[test]
    fn a_plan_reads_what_is_visible_first_then_ahead_then_behind_and_skips_what_is_done() {
        // list of 100 rows, rows 40..45 visible, margin 3
        let p = plan(40..45, 3, 100, |_| false, 100);
        assert_eq!(p, vec![40, 41, 42, 43, 44, 45, 46, 47, 39, 38, 37]);
        let p = plan(40..45, 3, 100, |i| i % 2 == 0, 100);
        assert_eq!(p, vec![41, 43, 45, 47, 39, 37]);
        assert_eq!(plan(40..45, 3, 100, |_| false, 4), vec![40, 41, 42, 43]);
        // the ends of the list
        assert_eq!(plan(0..2, 2, 3, |_| false, 100), vec![0, 1, 2]);
        assert_eq!(plan(98..120, 5, 100, |_| false, 100), vec![98, 99, 97, 96, 95, 94, 93]);
        assert!(plan(0..0, 5, 0, |_| false, 10).is_empty());
    }

    #[test]
    fn progress_is_a_percentage_that_never_passes_a_hundred() {
        assert_eq!(Progress { done: 0, total: 0 }.percent(), 100);
        assert_eq!(Progress { done: 1, total: 4 }.percent(), 25);
        assert_eq!(Progress { done: 9, total: 4 }.percent(), 100);
    }

    fn props() -> Vec<(String, String)> {
        [
            ("System.Author", "Yazarlar"),
            ("System.Music.Artist", "Katkıda bulunan sanatçılar"),
            ("System.Photo.DateTaken", "Çekim tarihi"),
            ("System.Media.Duration", "Uzunluk"),
            ("System.Zeta.Other", "Başka bir şey"),
            ("System.Title", "Başlık"),
        ]
        .iter()
        .map(|(a, b)| (a.to_string(), b.to_string()))
        .collect()
    }

    #[test]
    fn the_chooser_lists_the_common_ones_first_under_their_kind_then_the_rest() {
        let all = props();
        let rows = chooser_rows(&all, "");
        assert_eq!(rows[0], ChooserRow::Heading(Some(Category::Pictures)));
        assert_eq!(rows[1], ChooserRow::Item(2));
        // the last heading is "all the others", and a common property is not listed again under it
        let at = rows.iter().position(|r| *r == ChooserRow::Heading(None)).unwrap();
        assert_eq!(&rows[at + 1..], &[ChooserRow::Item(4)]);
        // the duration shows under music and under video
        assert_eq!(rows.iter().filter(|r| **r == ChooserRow::Item(3)).count(), 2);
    }

    #[test]
    fn the_chooser_search_matches_either_name_in_any_case_and_keeps_common_ones_first() {
        let all = props();
        assert_eq!(chooser_rows(&all, "BAŞ"), vec![ChooserRow::Item(5), ChooserRow::Item(4)], "Title is common, the other is not");
        assert_eq!(chooser_rows(&all, "system.zeta"), vec![ChooserRow::Item(4)]);
        assert_eq!(chooser_rows(&all, "  "), chooser_rows(&all, ""));
        assert!(chooser_rows(&all, "yok böyle").is_empty());
    }
}
