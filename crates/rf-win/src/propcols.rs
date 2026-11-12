//! The Windows property system as columns: the list of properties the system knows (canonical and
//! display names in the UI language), and a reader that fetches the values of chosen properties for
//! files, in batches, cancellable, with a time limit per file.
//!
//! Why `IPropertyStore` and not `IShellFolder2::GetDetailsEx`: one store per file gives every chosen
//! property for the price of one open (a handler is created once, not once per column), the values
//! come typed (a `PROPVARIANT`, so numbers and dates can be ordered as numbers and dates) and
//! `PSFormatForDisplayAlloc` turns the same variant into the text Explorer shows for it. `GetDetailsEx`
//! needs the parent folder bound and a child ID list built per file, hands back text for the column
//! provider's own idea of the property and a variant that is not always the typed value, and it is a
//! property of that one folder (a plain folder's columns, not a property of the file).
//!
//! Online-only files (OneDrive and other placeholders) are never opened: opening a property handler
//! reads the file, and a read downloads it. They get no value at all. The store is opened with
//! `GPS_BESTEFFORT` (a handler that fails still gives the file system's own properties), `GPS_NO_OPLOCK`
//! (reading does not take the file's opportunistic lock, so a program writing the file is not made to
//! wait) and without `GPS_OPENSLOWITEM` (a slow item, such as one on a slow share, is not forced open).
//!
//! All shell calls go through the shell thread ([`crate::sta`]): a worker thread that called the shell
//! itself would become an apartment that never pumps messages (see `sta::call`'s doc comment). The
//! wait for the thread's turn and the time a file takes are limited separately ([`call_timed`]).
//!
//! Nothing here runs unless the caller asks: no thread is started until the first job is queued.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Condvar, Mutex};
use rf_core::attrs;
use rf_core::propcols::{PropCell, PropKind, PropValue};
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PropVariantToBoolean, PropVariantToDouble, PropVariantToFileTime, PropVariantToInt64, PropVariantToUInt64,
};
use windows::Win32::System::Variant::{
    PSTF_UTC, VT_BOOL, VT_FILETIME, VT_I1, VT_I2, VT_I4, VT_I8, VT_INT, VT_NULL, VT_R4, VT_R8, VT_UI1, VT_UI2, VT_UI4, VT_UI8, VT_UINT,
};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GETPROPERTYSTOREFLAGS, GPS_BESTEFFORT, GPS_NO_OPLOCK, IPropertyDescription, IPropertyDescriptionList, IPropertyStore, PDEF_COLUMN,
    PDEF_VIEWABLE, PDFF_DEFAULT, PDTF_MASK_ALL, PDTF_MULTIPLEVALUES, PDVF_HIDDEN, PROPDESC_ENUMFILTER, PSEnumeratePropertyDescriptions,
    PSFormatForDisplayAlloc, PSGetPropertyDescriptionByName, PSGetPropertyKeyFromName, SHGetPropertyStoreFromParsingName,
};
use windows::core::{Interface, PCWSTR, PWSTR};

use crate::wide::wide;

// ------------------------------------------------------------------ pure helpers

/// The kind of value a property holds, from the `VARTYPE` its description gives.
pub fn kind_of_vartype(vt: u16) -> PropKind {
    // lists and arrays are shown as text ("a; b")
    if vt & 0xf000 != 0 {
        return PropKind::Text;
    }
    match vt {
        2..=6 | 16..=23 => PropKind::Number,
        11 => PropKind::Bool,
        64 => PropKind::DateTime,
        _ => PropKind::Text,
    }
}

/// A file Windows keeps in the cloud (or on a slow medium): reading its properties would read the file.
pub fn is_online_only(attributes: u32) -> bool {
    attributes & (attrs::OFFLINE | attrs::RECALL_ON_OPEN | attrs::RECALL_ON_DATA_ACCESS) != 0
}

/// 100-nanosecond ticks since 1601 -> Unix milliseconds (0 stays 0: no time).
pub fn filetime_to_unix_ms(ticks: u64) -> i64 {
    if ticks == 0 {
        return 0;
    }
    (ticks / 10_000) as i64 - 11_644_473_600_000
}

// ------------------------------------------------------------------ the list of properties

/// One property of the system's schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropInfo {
    pub canonical: String,
    /// The name in the UI language ("Uzunluk", "Çekim tarihi").
    pub display: String,
    pub kind: PropKind,
    /// A width that suits the property, in pixels.
    pub default_width: u16,
    /// The property holds a list (authors, tags).
    pub multiple: bool,
}

/// Which properties to list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Listing {
    /// Everything the system shows anywhere (Explorer's Details tab).
    Viewable,
    /// Only the ones that carry a column definition.
    Column,
}

fn take_pwstr(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string().unwrap_or_default() };
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn info_of(d: &IPropertyDescription) -> Option<PropInfo> {
    unsafe {
        let canonical = take_pwstr(d.GetCanonicalName().ok()?);
        let display = take_pwstr(d.GetDisplayName().ok()?);
        if canonical.is_empty() || display.trim().is_empty() {
            return None;
        }
        if d.GetViewFlags().is_ok_and(|f| f.0 & PDVF_HIDDEN.0 != 0) {
            return None;
        }
        let kind = kind_of_vartype(d.GetPropertyType().unwrap_or(0));
        let multiple = d.GetTypeFlags(PDTF_MASK_ALL).is_ok_and(|f| f.0 & PDTF_MULTIPLEVALUES.0 != 0);
        // the schema gives characters; seven pixels a character and some padding suits the 14 px text
        let chars = d.GetDefaultColumnWidth().unwrap_or(0);
        let default_width = if chars == 0 { rf_core::propcols::DEFAULT_WIDTH } else { (chars * 7 + 16).clamp(90, 260) as u16 };
        Some(PropInfo { canonical, display: display.trim().to_string(), kind, default_width, multiple })
    }
}

fn enumerate(filter: PROPDESC_ENUMFILTER) -> Vec<PropInfo> {
    let mut raw = std::ptr::null_mut();
    let list: IPropertyDescriptionList = unsafe {
        if PSEnumeratePropertyDescriptions(filter, &IPropertyDescriptionList::IID, &mut raw).is_err() || raw.is_null() {
            return Vec::new();
        }
        IPropertyDescriptionList::from_raw(raw)
    };
    let n = unsafe { list.GetCount().unwrap_or(0) };
    let mut out: Vec<PropInfo> = (0..n)
        .filter_map(|i| {
            let d: IPropertyDescription = unsafe { list.GetAt(i).ok()? };
            info_of(&d)
        })
        .collect();
    // by display name, case folded; a property listed twice keeps one row
    out.sort_by_cached_key(|p| (rf_core::sort::lower(&p.display), p.canonical.clone()));
    out.dedup_by(|a, b| a.canonical == b.canonical);
    out
}

/// The properties the system lists, by display name. Blocks (the schema is loaded on first use):
/// never call it on the UI thread.
pub fn list_properties(which: Listing) -> Vec<PropInfo> {
    let filter = match which {
        Listing::Viewable => PDEF_VIEWABLE,
        Listing::Column => PDEF_COLUMN,
    };
    crate::sta::call(Duration::from_secs(30), move || enumerate(filter)).unwrap_or_default()
}

/// The description of each named property (None: the system does not know the name).
pub fn describe(names: &[Arc<str>]) -> Vec<Option<PropInfo>> {
    let n = names.len();
    let names: Vec<Arc<str>> = names.to_vec();
    crate::sta::call(Duration::from_secs(15), move || {
        names
            .iter()
            .map(|n| {
                let w = wide(&**n);
                let mut raw = std::ptr::null_mut();
                unsafe {
                    PSGetPropertyDescriptionByName(PCWSTR(w.as_ptr()), &IPropertyDescription::IID, &mut raw).ok()?;
                    if raw.is_null() {
                        return None;
                    }
                    info_of(&IPropertyDescription::from_raw(raw))
                }
            })
            .collect()
    })
    .unwrap_or_else(|| vec![None; n])
}

// ------------------------------------------------------------------ reading values

/// Why a file has no values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadFail {
    /// The shell did not answer in time (a slow or broken handler).
    Timeout,
    /// The file is not there, is a folder, or its handler refused to open.
    Unavailable,
    /// The file is in the cloud and was not opened.
    OnlineOnly,
}

/// A place the values come from. The real one asks Windows; tests use [`FakeSource`].
pub trait PropSource: Send + Sync {
    /// The cells of `names` for the file, in the same order. Blocks, but not longer than the source's
    /// own limit.
    fn read(&self, path: &Path, names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail>;
}

type KeyMap = Mutex<HashMap<Arc<str>, Option<PROPERTYKEY>>>;

/// The Windows property system.
pub struct SystemSource {
    limit: Duration,
    flags: GETPROPERTYSTOREFLAGS,
    keys: Arc<KeyMap>,
}

impl SystemSource {
    /// `limit`: the longest one file may take once the shell thread has started on it.
    pub fn new(limit: Duration) -> Self {
        Self { limit, flags: GPS_BESTEFFORT | GPS_NO_OPLOCK, keys: Arc::new(Mutex::new(HashMap::new())) }
    }

    /// The same with other `GETPROPERTYSTOREFLAGS` (the probe compares flag sets).
    pub fn with_flags(limit: Duration, flags: i32) -> Self {
        Self { flags: GETPROPERTYSTOREFLAGS(flags), ..Self::new(limit) }
    }
}

fn key_of(keys: &KeyMap, name: &Arc<str>) -> Option<PROPERTYKEY> {
    if let Some(k) = keys.lock().get(name) {
        return *k;
    }
    let w = wide(&**name);
    let mut key = PROPERTYKEY::default();
    let k = unsafe { PSGetPropertyKeyFromName(PCWSTR(w.as_ptr()), &mut key) }.ok().map(|_| key);
    keys.lock().insert(name.clone(), k);
    k
}

/// A date property's value; a zero time (a handler that has no date but answers with an empty
/// FILETIME) is no value, so it sorts with the files that have none and not before every real date.
pub fn date_value(ticks: u64) -> PropValue {
    if ticks == 0 { PropValue::Empty } else { PropValue::Int(filetime_to_unix_ms(ticks)) }
}

fn value_of(pv: &PROPVARIANT, text: &str) -> PropValue {
    let vt = pv.vt();
    // lists and arrays order by their text
    if vt.0 & 0xf000 != 0 {
        return PropValue::text(text);
    }
    unsafe {
        match vt {
            VT_I1 | VT_I2 | VT_I4 | VT_I8 | VT_INT | VT_UI1 | VT_UI2 | VT_UI4 | VT_UINT => PropVariantToInt64(pv).map(PropValue::Int).unwrap_or_default(),
            VT_UI8 => PropVariantToUInt64(pv).map(|v| PropValue::Int(v.min(i64::MAX as u64) as i64)).unwrap_or_default(),
            VT_R4 | VT_R8 => PropVariantToDouble(pv).map(PropValue::float).unwrap_or_default(),
            VT_BOOL => PropVariantToBoolean(pv).map(|b| PropValue::Int(i64::from(b.as_bool()))).unwrap_or_default(),
            VT_FILETIME => PropVariantToFileTime(pv, PSTF_UTC)
                .map(|ft| date_value((u64::from(ft.dwHighDateTime) << 32) | u64::from(ft.dwLowDateTime)))
                .unwrap_or_default(),
            _ => PropValue::text(text),
        }
    }
}

fn read_cell(store: &IPropertyStore, key: &PROPERTYKEY) -> PropCell {
    let Ok(pv) = (unsafe { store.GetValue(key) }) else { return PropCell::empty() };
    if pv.is_empty() || pv.vt() == VT_NULL {
        return PropCell::empty();
    }
    // the text Explorer shows for this value (units, localized dates, "a; b" lists)
    let text = unsafe {
        match PSFormatForDisplayAlloc(key, &pv, PDFF_DEFAULT) {
            Ok(s) => take_pwstr(s).trim().to_string(),
            Err(_) => String::new(),
        }
    };
    let value = value_of(&pv, &text);
    if text.is_empty() && value.is_empty() {
        return PropCell::empty();
    }
    PropCell::new(&text, value)
}

fn read_on_sta(path: &Path, names: &[Arc<str>], keys: &KeyMap, flags: GETPROPERTYSTOREFLAGS) -> Result<Vec<PropCell>, ReadFail> {
    // (the parsing name wants backslashes: a path built with a slash somewhere is not found)
    let mut w = wide(path);
    for c in w.iter_mut().filter(|c| **c == u16::from(b'/')) {
        *c = u16::from(b'\\');
    }
    let store: IPropertyStore =
        unsafe { SHGetPropertyStoreFromParsingName(PCWSTR(w.as_ptr()), None, flags) }.map_err(|_| ReadFail::Unavailable)?;
    Ok(names
        .iter()
        .map(|n| match key_of(keys, n) {
            Some(k) => read_cell(&store, &k),
            None => PropCell::empty(),
        })
        .collect())
}

/// What the file's attributes say about reading it: online-only files are never opened (a read would
/// download them) and folders have no values.
pub fn precheck(attributes: u32) -> Result<(), ReadFail> {
    if is_online_only(attributes) {
        Err(ReadFail::OnlineOnly)
    } else if attributes & attrs::DIRECTORY != 0 {
        Err(ReadFail::Unavailable)
    } else {
        Ok(())
    }
}

/// Runs `f` on the shell thread. The two limits are kept apart: `queue_wait` is how long the job may
/// wait for its turn (the thread may be busy with a context menu, an icon or another file's handler),
/// `run_limit` how long it may take once it has started. `rf_win::sta::call` has one limit for both,
/// which charged a file for the time other shell work spent in front of it. A job that did not get
/// its turn in time is skipped when its turn comes (the file is not read for nobody). `None`: out of
/// time. Never call it from the shell thread itself (the job would wait for its own turn).
fn call_timed<R: Send + 'static>(queue_wait: Duration, run_limit: Duration, f: impl FnOnce() -> R + Send + 'static) -> Option<R> {
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel::<()>(1);
    let (tx, rx) = std::sync::mpsc::sync_channel::<R>(1);
    let skip = Arc::new(AtomicBool::new(false));
    let skip_there = skip.clone();
    crate::sta::run(move |_| {
        if skip_there.load(Ordering::SeqCst) {
            return;
        }
        let _ = started_tx.send(());
        let _ = tx.send(f());
    });
    if started_rx.recv_timeout(queue_wait).is_err() {
        skip.store(true, Ordering::SeqCst);
        return None;
    }
    rx.recv_timeout(run_limit).ok()
}

impl PropSource for SystemSource {
    fn read(&self, path: &Path, names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> {
        use std::os::windows::fs::MetadataExt;
        // (asked on the caller's own thread: a placeholder's attributes are read without recalling it.
        // The caller only reads plain local folders: network shares are left out by the view, because
        // this call has no time limit of its own.)
        match std::fs::symlink_metadata(path) {
            Ok(md) => precheck(md.file_attributes())?,
            Err(_) => return Err(ReadFail::Unavailable),
        }
        let (p, n, keys, flags) = (path.to_path_buf(), names.to_vec(), self.keys.clone(), self.flags);
        match call_timed(QUEUE_WAIT, self.limit, move || read_on_sta(&p, &n, &keys, flags)) {
            Some(r) => r,
            None => Err(ReadFail::Timeout),
        }
    }
}

/// How long a read may wait for the shell thread's turn.
pub const QUEUE_WAIT: Duration = Duration::from_secs(10);

type FakeAnswer = dyn Fn(&Path, &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> + Send + Sync;

/// A source made of a closure, for tests: counts its calls.
pub struct FakeSource {
    answer: Box<FakeAnswer>,
    pub calls: AtomicUsize,
}

impl FakeSource {
    pub fn new(answer: impl Fn(&Path, &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> + Send + Sync + 'static) -> Self {
        Self { answer: Box::new(answer), calls: AtomicUsize::new(0) }
    }
}

impl PropSource for FakeSource {
    fn read(&self, path: &Path, names: &[Arc<str>]) -> Result<Vec<PropCell>, ReadFail> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.answer)(path, names)
    }
}

// ------------------------------------------------------------------ a batch

/// How a batch ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchEnd {
    Finished,
    Cancelled,
    /// Several files in a row did not answer in time: the rest was not tried.
    Stalled,
}

/// This many files in a row that run out of time stop a batch (the shell is stuck on something; asking
/// on would only queue more work behind it).
pub const STRIKES: usize = 3;

/// Reads `names` for each job's file, one file after another, and hands every answer to `on_result`
/// (the job's tag first). Stops between files when `cancel` is set. A file that runs out of time is
/// reported as such and the batch goes on, unless [`STRIKES`] files in a row did.
pub fn read_batch<T>(
    src: &dyn PropSource,
    jobs: impl IntoIterator<Item = (T, PathBuf)>,
    names: &[Arc<str>],
    cancel: &AtomicBool,
    mut on_result: impl FnMut(T, Result<Vec<PropCell>, ReadFail>),
) -> BatchEnd {
    let mut strikes = 0;
    for (tag, path) in jobs {
        if cancel.load(Ordering::Relaxed) {
            return BatchEnd::Cancelled;
        }
        let r = src.read(&path, names);
        if r == Err(ReadFail::Timeout) {
            strikes += 1;
            on_result(tag, r);
            if strikes >= STRIKES {
                return BatchEnd::Stalled;
            }
        } else {
            strikes = 0;
            on_result(tag, r);
        }
    }
    BatchEnd::Finished
}

// ------------------------------------------------------------------ the queue behind a list's rows

/// A file to read: where it is and the size and time it has now (the cache key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueJob {
    pub path: PathBuf,
    pub size: u64,
    pub modified_ms: i64,
}

/// What came of a queued job.
pub struct Done {
    pub job: QueueJob,
    pub names: Arc<[Arc<str>]>,
    pub result: Result<Vec<PropCell>, ReadFail>,
}

struct QueueState {
    jobs: VecDeque<QueueJob>,
    names: Arc<[Arc<str>]>,
    quit: bool,
    /// After a stall nothing is read until this time.
    paused_until: Option<Instant>,
    running: bool,
    /// The file being read now (a new request does not queue it a second time).
    current: Option<PathBuf>,
}

struct QueueShared {
    state: Mutex<QueueState>,
    wake: Condvar,
}

/// The files the visible rows of a list want, read one after another on a thread of its own. A new
/// request replaces what has not started yet (rows that scrolled away are not read); the file being
/// read is finished. The thread starts with the first job and ends when the queue is dropped.
pub struct ReadQueue {
    src: Arc<dyn PropSource>,
    sink: Arc<dyn Fn(Done) + Send + Sync>,
    shared: Arc<QueueShared>,
}

/// How long a stalled queue reads nothing.
pub const STALL_PAUSE: Duration = Duration::from_secs(10);

impl ReadQueue {
    /// `sink` gets every answer, on the queue's thread.
    pub fn new(src: Arc<dyn PropSource>, sink: impl Fn(Done) + Send + Sync + 'static) -> Self {
        let shared = Arc::new(QueueShared {
            state: Mutex::new(QueueState { jobs: VecDeque::new(), names: Arc::from(Vec::new()), quit: false, paused_until: None, running: false, current: None }),
            wake: Condvar::new(),
        });
        Self { src, sink: Arc::new(sink), shared }
    }

    /// Replaces the waiting jobs. Does nothing while the queue is paused after a stall.
    pub fn replace(&self, names: Arc<[Arc<str>]>, jobs: Vec<QueueJob>) {
        let mut st = self.shared.state.lock();
        if st.paused_until.is_some_and(|t| Instant::now() < t) {
            return;
        }
        st.paused_until = None;
        st.names = names;
        let current = st.current.clone();
        st.jobs = jobs.into_iter().filter(|j| Some(&j.path) != current.as_ref()).collect();
        let start = !st.running && !st.jobs.is_empty();
        if start {
            st.running = true;
        }
        drop(st);
        if start {
            let (src, sink, shared) = (self.src.clone(), self.sink.clone(), self.shared.clone());
            let spawned = std::thread::Builder::new().name("rf-propread".into()).spawn(move || run_queue(src, sink, shared));
            if spawned.is_err() {
                self.shared.state.lock().running = false;
            }
        } else {
            self.shared.wake.notify_one();
        }
    }

    /// Drops every waiting job.
    pub fn clear(&self) {
        self.shared.state.lock().jobs.clear();
    }

    /// Jobs waiting (the one being read is not counted).
    pub fn waiting(&self) -> usize {
        self.shared.state.lock().jobs.len()
    }

    /// Whether the queue is paused after a stall.
    pub fn paused(&self) -> bool {
        self.shared.state.lock().paused_until.is_some_and(|t| Instant::now() < t)
    }
}

impl Drop for ReadQueue {
    fn drop(&mut self) {
        let mut st = self.shared.state.lock();
        st.quit = true;
        st.jobs.clear();
        drop(st);
        self.shared.wake.notify_all();
    }
}

fn run_queue(src: Arc<dyn PropSource>, sink: Arc<dyn Fn(Done) + Send + Sync>, shared: Arc<QueueShared>) {
    let mut strikes = 0;
    loop {
        let (job, names) = {
            let mut st = shared.state.lock();
            loop {
                if st.quit {
                    st.running = false;
                    return;
                }
                if let Some(j) = st.jobs.pop_front() {
                    st.current = Some(j.path.clone());
                    break (j, st.names.clone());
                }
                // idle: wait for work, and leave for good when nobody asks for a minute
                if shared.wake.wait_for(&mut st, Duration::from_secs(60)).timed_out() && st.jobs.is_empty() {
                    st.running = false;
                    return;
                }
            }
        };
        let result = src.read(&job.path, &names);
        if result == Err(ReadFail::Timeout) {
            strikes += 1;
        } else {
            strikes = 0;
        }
        sink(Done { job, names, result });
        let mut st = shared.state.lock();
        st.current = None;
        if strikes >= STRIKES {
            strikes = 0;
            st.jobs.clear();
            st.paused_until = Some(Instant::now() + STALL_PAUSE);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn names(items: &[&str]) -> Vec<Arc<str>> {
        items.iter().map(|s| Arc::from(*s)).collect()
    }

    fn cell(s: &str) -> PropCell {
        PropCell::new(s, PropValue::text(s))
    }

    #[test]
    fn a_vartype_decides_the_kind() {
        assert_eq!(kind_of_vartype(31), PropKind::Text); // LPWSTR
        assert_eq!(kind_of_vartype(19), PropKind::Number); // UI4
        assert_eq!(kind_of_vartype(21), PropKind::Number); // UI8
        assert_eq!(kind_of_vartype(5), PropKind::Number); // R8
        assert_eq!(kind_of_vartype(64), PropKind::DateTime);
        assert_eq!(kind_of_vartype(11), PropKind::Bool);
        assert_eq!(kind_of_vartype(0x1000 | 31), PropKind::Text, "a list of strings");
        assert_eq!(kind_of_vartype(0x1000 | 19), PropKind::Text, "a list of numbers is shown as text");
    }

    #[test]
    fn online_only_files_are_the_ones_a_read_would_download() {
        assert!(is_online_only(attrs::RECALL_ON_DATA_ACCESS));
        assert!(is_online_only(attrs::RECALL_ON_OPEN | attrs::ARCHIVE));
        assert!(is_online_only(attrs::OFFLINE));
        assert!(!is_online_only(attrs::ARCHIVE | attrs::HIDDEN));
        assert!(!is_online_only(0));
    }

    #[test]
    fn file_times_become_unix_milliseconds() {
        assert_eq!(filetime_to_unix_ms(116_444_736_000_000_000), 0, "1970");
        assert_eq!(filetime_to_unix_ms(116_444_736_000_000_000 + 10_000_000), 1000);
        assert_eq!(filetime_to_unix_ms(0), 0);
    }

    #[test]
    fn a_batch_reads_in_order_and_stops_when_cancelled() {
        let src = FakeSource::new(|p, n| Ok(n.iter().map(|_| cell(&p.to_string_lossy())).collect()));
        let jobs: Vec<(usize, PathBuf)> = (0..5).map(|i| (i, PathBuf::from(format!("f{i}")))).collect();
        let cancel = AtomicBool::new(false);
        let mut got = Vec::new();
        let end = read_batch(&src, jobs.clone(), &names(&["a", "b"]), &cancel, |t, r| got.push((t, r.unwrap().len())));
        assert_eq!(end, BatchEnd::Finished);
        assert_eq!(got, vec![(0, 2), (1, 2), (2, 2), (3, 2), (4, 2)]);

        let mut seen = 0;
        let end = read_batch(&src, jobs, &names(&["a"]), &cancel, |t, _| {
            seen += 1;
            if t == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert_eq!(end, BatchEnd::Cancelled);
        assert_eq!(seen, 2, "the file after the cancel was not read");
    }

    #[test]
    fn one_slow_file_does_not_stop_the_batch_but_three_in_a_row_do_on_a_fake_source() {
        let src = FakeSource::new(|p, _| if p.to_string_lossy().starts_with("slow") { Err(ReadFail::Timeout) } else { Ok(vec![cell("x")]) });
        let cancel = AtomicBool::new(false);
        let run = |files: &[&str]| {
            let mut fails = 0;
            let end = read_batch(&src, files.iter().enumerate().map(|(i, f)| (i, PathBuf::from(f))), &names(&["a"]), &cancel, |_, r| {
                fails += usize::from(r.is_err());
            });
            (end, fails)
        };
        assert_eq!(run(&["a", "slow1", "b", "slow2", "c", "slow3", "d"]), (BatchEnd::Finished, 3), "answers in between reset the count");
        assert_eq!(run(&["a", "slow1", "slow2", "slow3", "b", "c"]), (BatchEnd::Stalled, 3));
    }

    #[test]
    fn a_zero_date_is_no_value_and_the_attributes_decide_what_is_read() {
        assert_eq!(date_value(0), PropValue::Empty, "an empty FILETIME sorts with the files that have no date");
        assert_eq!(date_value(116_444_736_000_000_000 + 10_000_000), PropValue::Int(1000));
        assert_eq!(precheck(attrs::RECALL_ON_DATA_ACCESS), Err(ReadFail::OnlineOnly));
        assert_eq!(precheck(attrs::OFFLINE | attrs::ARCHIVE), Err(ReadFail::OnlineOnly));
        assert_eq!(precheck(attrs::DIRECTORY), Err(ReadFail::Unavailable));
        assert_eq!(precheck(attrs::ARCHIVE | attrs::HIDDEN), Ok(()));
        // a placeholder folder is still never opened
        assert_eq!(precheck(attrs::DIRECTORY | attrs::RECALL_ON_OPEN), Err(ReadFail::OnlineOnly));
    }

    #[test]
    fn work_in_front_of_a_job_on_the_shell_thread_is_not_charged_to_it() {
        // the shell thread is busy for 300 ms with something else (a menu, an icon)
        crate::sta::run(|_| std::thread::sleep(Duration::from_millis(300)));
        // the job itself takes no time: it must be answered although it waited longer than its run limit
        assert_eq!(call_timed(Duration::from_secs(10), Duration::from_millis(100), || 7), Some(7));
        // a job that really is slow once it has started is out of time
        assert_eq!(call_timed(Duration::from_secs(10), Duration::from_millis(50), || std::thread::sleep(Duration::from_millis(400))), None);
        assert_eq!(call_timed(Duration::from_secs(10), Duration::from_secs(10), || 1), Some(1), "the thread lives on");
    }

    #[test]
    fn a_job_that_never_got_its_turn_is_skipped_when_the_turn_comes() {
        let ran = Arc::new(AtomicBool::new(false));
        crate::sta::run(|_| std::thread::sleep(Duration::from_millis(300)));
        let r = ran.clone();
        assert_eq!(call_timed(Duration::from_millis(50), Duration::from_secs(10), move || r.store(true, Ordering::SeqCst)), None);
        // a later job goes behind the sleeping one and the abandoned one; when it is answered the abandoned one has had its turn
        assert_eq!(call_timed(Duration::from_secs(10), Duration::from_secs(10), || 1), Some(1));
        assert!(!ran.load(Ordering::SeqCst), "nobody was waiting for that file any more, so it was not read");
    }

    #[test]
    fn online_only_and_missing_files_are_reported_not_hidden() {
        let src = FakeSource::new(|p, _| match p.to_string_lossy().as_ref() {
            "cloud" => Err(ReadFail::OnlineOnly),
            "gone" => Err(ReadFail::Unavailable),
            _ => Ok(vec![cell("v")]),
        });
        let mut out = Vec::new();
        read_batch(&src, [("cloud", PathBuf::from("cloud")), ("gone", PathBuf::from("gone")), ("ok", PathBuf::from("ok"))], &names(&["a"]), &AtomicBool::new(false), |t, r| {
            out.push((t, r.map(|v| v.len())))
        });
        assert_eq!(out, vec![("cloud", Err(ReadFail::OnlineOnly)), ("gone", Err(ReadFail::Unavailable)), ("ok", Ok(1))]);
    }

    fn job(name: &str) -> QueueJob {
        QueueJob { path: PathBuf::from(name), size: 1, modified_ms: 1 }
    }

    #[test]
    fn a_new_request_replaces_the_rows_that_have_not_started() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let first_started = Arc::new(AtomicBool::new(false));
        let fs = first_started.clone();
        let src = Arc::new(FakeSource::new(move |p, _| {
            if p.to_string_lossy() == "first" {
                fs.store(true, Ordering::SeqCst);
                let _ = gate_rx.lock().recv_timeout(Duration::from_secs(10));
            }
            Ok(vec![cell("v")])
        }));
        let (tx, rx) = mpsc::channel::<String>();
        let tx = Mutex::new(tx);
        let q = ReadQueue::new(src.clone(), move |d| {
            let _ = tx.lock().send(d.job.path.to_string_lossy().into_owned());
        });
        q.replace(Arc::from(names(&["a"])), vec![job("first"), job("old1"), job("old2")]);
        while !first_started.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(2));
        }
        // the rows scrolled away: the old ones are dropped, the new ones wait behind the file being read
        q.replace(Arc::from(names(&["a"])), vec![job("new1"), job("new2")]);
        assert_eq!(q.waiting(), 2);
        gate_tx.send(()).unwrap();
        let got: Vec<String> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(10)).unwrap()).collect();
        assert_eq!(got, vec!["first", "new1", "new2"]);
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err(), "old1 and old2 were never read");
        assert_eq!(src.calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_queue_that_stalls_pauses_and_nothing_is_read_while_it_does() {
        let src = Arc::new(FakeSource::new(|_, _| Err(ReadFail::Timeout)));
        let (tx, rx) = mpsc::channel::<()>();
        let tx = Mutex::new(tx);
        let q = ReadQueue::new(src.clone(), move |_| {
            let _ = tx.lock().send(());
        });
        q.replace(Arc::from(names(&["a"])), (0..10).map(|i| job(&format!("f{i}"))).collect());
        for _ in 0..STRIKES {
            rx.recv_timeout(Duration::from_secs(10)).unwrap();
        }
        // wait until the queue has noticed
        let t = Instant::now();
        while !q.paused() && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(q.paused());
        assert_eq!(src.calls.load(Ordering::SeqCst), STRIKES, "the rest of the rows was not tried");
        q.replace(Arc::from(names(&["a"])), vec![job("again")]);
        assert_eq!(q.waiting(), 0, "a request during the pause is not queued");
    }

    #[test]
    fn dropping_the_queue_ends_its_thread() {
        let src = Arc::new(FakeSource::new(|_, _| Ok(vec![cell("v")])));
        let count = Arc::new(AtomicUsize::new(0));
        let c = count.clone();
        let q = ReadQueue::new(src, move |_| {
            c.fetch_add(1, Ordering::SeqCst);
        });
        q.replace(Arc::from(names(&["a"])), vec![job("x")]);
        let t = Instant::now();
        while count.load(Ordering::SeqCst) < 1 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        let shared = q.shared.clone();
        drop(q);
        let t = Instant::now();
        while shared.state.lock().running && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!shared.state.lock().running);
    }

    /// The real property system on files made here: sizes and names are always there.
    #[test]
    fn the_system_source_reads_a_file_made_here_and_skips_a_folder() {
        let dir = std::env::temp_dir().join(format!("rf-propcols-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a.txt");
        std::fs::write(&file, vec![b'x'; 1536]).unwrap();
        let src = SystemSource::new(Duration::from_secs(20));
        let wanted = names(&["System.Size", "System.ItemTypeText", "System.Media.Duration", "No.Such.Property"]);
        let r = src.read(&file, &wanted);
        let folder = src.read(&dir, &wanted);
        let missing = src.read(&dir.join("nope.txt"), &wanted);
        let _ = std::fs::remove_dir_all(&dir);
        let cells = r.expect("the file is readable");
        assert_eq!(cells.len(), 4);
        assert_eq!(cells[0].value, PropValue::Int(1536), "the size is a number");
        assert!(!cells[0].text.is_empty(), "and has the text Explorer shows");
        assert!(!cells[1].text.is_empty());
        assert!(cells[2].value.is_empty(), "a text file has no duration");
        assert!(cells[3].value.is_empty(), "an unknown name is an empty cell, not an error");
        assert_eq!(folder, Err(ReadFail::Unavailable));
        assert_eq!(missing, Err(ReadFail::Unavailable));
    }

    #[test]
    fn the_system_lists_properties_with_display_names_and_describes_them_by_name() {
        let all = list_properties(Listing::Viewable);
        assert!(all.len() > 100, "{} properties", all.len());
        let dur = all.iter().find(|p| p.canonical == "System.Media.Duration").expect("duration is listed");
        assert!(!dur.display.is_empty());
        assert_eq!(dur.kind, PropKind::Number);
        let distinct: std::collections::HashSet<&str> = all.iter().map(|p| p.canonical.as_str()).collect();
        assert_eq!(distinct.len(), all.len(), "no property twice");
        let d = describe(&names(&["System.Photo.DateTaken", "No.Such.Property"]));
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].as_ref().map(|p| p.kind), Some(PropKind::DateTime));
        assert!(d[1].is_none());
    }
}
