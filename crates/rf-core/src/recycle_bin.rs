//! Recycle Bin logic that needs no Windows: which bin items a command applies to, what the
//! "delete for good" confirmation says, how a deletion is checked afterwards, and the model of
//! the bin's own settings (per drive: custom maximum size or "remove immediately", and the
//! delete confirmation) with its validation. The shell and the registry sit behind the two
//! traits [`BinBackend`] and [`BinSettingsStore`]; the tests here use fakes.

use std::collections::HashSet;

/// How an item of the bin is told apart from the others: its path inside the bin's storage, lower-cased
/// (the same key `rf_win::recycle` uses to find an item again).
pub fn key_of(parsing_path: &str) -> String {
    parsing_path.to_lowercase()
}

/// The rows a command (restore, delete for good) applies to: the selected ones that are shown, in the order they are shown.
/// A selected row the search filter hides is never acted on.
pub fn command_targets(shown: &[usize], selected: &HashSet<usize>) -> Vec<usize> {
    shown.iter().copied().filter(|i| selected.contains(i)).collect()
}

/// Whether an item of the bin is a folder. The shell calls a compressed (zip) folder a folder (SFGAO_FOLDER) and marks it as a stream
/// too (SFGAO_STREAM, "it is also a file"); a file system that is asked directly says what it is. The file system's answer wins when
/// there is one; without it, a folder that is also a stream is a file.
pub fn is_real_folder(shell_folder: bool, shell_stream: bool, fs_is_dir: Option<bool>) -> bool {
    fs_is_dir.unwrap_or(shell_folder && !shell_stream)
}

/// What the file system's own look at an item (without following links) tells [`is_real_folder`]. A junction or a directory symlink
/// is not "a directory" to a look that does not follow links, yet it is a folder to the shell and to the user; for such a
/// link the file system gives no answer and the shell's own (folder and not stream) decides.
pub fn fs_dir_answer(is_dir: bool, is_link: bool) -> Option<bool> {
    if is_link { None } else { Some(is_dir) }
}

/// What the confirmation tells: how many items, how many of them folders, how much data.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeleteFacts {
    pub count: usize,
    pub folders: usize,
    pub bytes: u64,
}

/// Counts for `(is_folder, size)` pairs.
pub fn delete_facts(items: impl IntoIterator<Item = (bool, u64)>) -> DeleteFacts {
    let mut f = DeleteFacts::default();
    for (dir, size) in items {
        f.count += 1;
        f.folders += usize::from(dir);
        f.bytes = f.bytes.saturating_add(size);
    }
    f
}

/// The shell's side of the bin, as the deletion needs it.
pub trait BinBackend {
    /// The keys ([`key_of`]) of everything in the bin now.
    fn list_keys(&self) -> anyhow::Result<Vec<String>>;
    /// Deletes exactly these items for good. May delete fewer (an item in use); an error says why nothing, or not everything, went.
    fn delete_keys(&self, keys: &[String]) -> anyhow::Result<()>;
}

/// What a deletion did, found by listing the bin before and after (the shell's own answer is not trusted alone).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    pub requested: usize,
    /// Requested items that are gone from the bin.
    pub deleted: usize,
    /// Requested items still in the bin.
    pub left: usize,
    /// Items that were NOT requested and are gone from the bin. Must be 0; anything else is reported loudly.
    pub collateral: usize,
    /// The shell's error, or the failure to list the bin, in one line.
    pub error: Option<String>,
    /// The deletion was sent and the answer never came (the shell thread did not answer in time, or the bin could not be listed
    /// afterwards): the request may still run, or may have run, so `deleted` and `left` are not claimed.
    pub unknown: bool,
}

impl DeleteReport {
    pub fn all_done(&self) -> bool {
        !self.unknown && self.error.is_none() && self.left == 0 && self.collateral == 0 && self.deleted == self.requested
    }
}

/// The error a [`BinBackend`] gives when the shell did not answer within its time limit. The job it was given may still run later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimedOut;

impl std::fmt::Display for TimedOut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Geri Dönüşüm Kutusu zamanında yanıt vermedi")
    }
}

impl std::error::Error for TimedOut {}

/// Deletes `keys` through `backend` and checks the result against the bin's listing before and after.
/// With no keys nothing is asked of the backend. Keys the bin does not have (gone already) are not counted as requested.
pub fn delete_through(backend: &dyn BinBackend, keys: &[String]) -> DeleteReport {
    let mut asked: Vec<String> = keys.iter().map(|k| key_of(k)).collect();
    asked.sort();
    asked.dedup();
    let mut r = DeleteReport::default();
    if asked.is_empty() {
        return r;
    }
    let before: HashSet<String> = match backend.list_keys() {
        Ok(v) => v.into_iter().map(|k| key_of(&k)).collect(),
        Err(e) => {
            r.requested = asked.len();
            r.left = asked.len();
            r.error = Some(format!("{e:#}"));
            return r;
        }
    };
    asked.retain(|k| before.contains(k));
    r.requested = asked.len();
    if asked.is_empty() {
        return r;
    }
    if let Err(e) = backend.delete_keys(&asked) {
        r.error = Some(format!("{e:#}"));
        if e.downcast_ref::<TimedOut>().is_some() {
            // The request is still queued on the shell thread, which is the same thread a listing would wait on: do not wait for
            // it again, and do not say anything was or was not deleted.
            r.unknown = true;
            return r;
        }
    }
    match backend.list_keys() {
        Ok(v) => {
            let after: HashSet<String> = v.into_iter().map(|k| key_of(&k)).collect();
            let asked_set: HashSet<&String> = asked.iter().collect();
            r.left = asked.iter().filter(|k| after.contains(*k)).count();
            r.deleted = r.requested - r.left;
            r.collateral = before.iter().filter(|k| !asked_set.contains(k) && !after.contains(*k)).count();
        }
        Err(e) => {
            // the outcome is unknown: nothing is claimed
            r.unknown = true;
            r.error.get_or_insert(format!("{e:#}"));
        }
    }
    r
}

// ---------------------------------------------------------------------------------------------------------------------
// The bin's own settings (Explorer: Recycle Bin Properties)
// ---------------------------------------------------------------------------------------------------------------------

/// One drive's bin settings as they are now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeBin {
    /// The volume's key under the bin's settings (its GUID in braces); what a write goes to.
    pub id: String,
    /// "C:"
    pub label: String,
    /// The drive's size in MB; 0 when not known.
    pub capacity_mb: u64,
    /// The custom maximum size in MB; `None` while the drive has no custom size (the bin's default applies).
    pub max_mb: Option<u32>,
    /// "Do not move files to the Recycle Bin. Remove files immediately when deleted."
    pub remove_immediately: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinSettings {
    pub volumes: Vec<VolumeBin>,
    /// "Display delete confirmation dialog".
    pub confirm_delete: bool,
    /// A policy fixes the confirmation: it is shown but not written.
    pub confirm_locked: bool,
}

/// What the dialog's controls hold for one drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeEdit {
    pub remove_immediately: bool,
    /// The text of "Maximum size (MB)"; empty = no custom size.
    pub size_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinSettingsEdit {
    pub volumes: Vec<VolumeEdit>,
    pub confirm_delete: bool,
}

/// The controls as they start out.
pub fn initial_edit(s: &BinSettings) -> BinSettingsEdit {
    BinSettingsEdit {
        volumes: s.volumes.iter().map(|v| VolumeEdit { remove_immediately: v.remove_immediately, size_text: v.max_mb.map(|m| m.to_string()).unwrap_or_default() }).collect(),
        confirm_delete: s.confirm_delete,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Problem {
    /// Not a whole number of megabytes (or empty where a custom size was set before).
    NotANumber,
    /// Below 1 MB.
    TooSmall,
    /// More than the drive holds (the drive's size in MB).
    TooBig(u64),
}

/// What to write for one drive; a `None` is left as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeChange {
    pub id: String,
    pub max_mb: Option<u32>,
    pub remove_immediately: Option<bool>,
}

/// Only what differs from the settings read; nothing else is ever written.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub volumes: Vec<VolumeChange>,
    pub confirm_delete: Option<bool>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.volumes.is_empty() && self.confirm_delete.is_none()
    }
}

/// Checks `edit` against `now` and says what would be written. Problems are listed per drive index (the first for each drive).
/// The size of a drive whose "remove immediately" is on is not looked at (its box is off in Explorer too).
pub fn plan(now: &BinSettings, edit: &BinSettingsEdit) -> Result<Changes, Vec<(usize, Problem)>> {
    let mut changes = Changes::default();
    let mut problems = Vec::new();
    for (i, (v, e)) in now.volumes.iter().zip(&edit.volumes).enumerate() {
        let mut c = VolumeChange { id: v.id.clone(), max_mb: None, remove_immediately: None };
        if e.remove_immediately != v.remove_immediately {
            c.remove_immediately = Some(e.remove_immediately);
        }
        // (a size the user did not touch is not checked: a stale value above the drive's size, e.g. after the partition shrank,
        // must not stand in the way of saving the rest)
        let untouched = e.size_text.trim() == v.max_mb.map(|m| m.to_string()).unwrap_or_default();
        if !e.remove_immediately && !untouched {
            let text = e.size_text.trim();
            match parse_size(text, v.capacity_mb) {
                Ok(Some(mb)) if Some(mb) != v.max_mb => c.max_mb = Some(mb),
                Ok(Some(_)) => {}
                // empty: no custom size wanted; one that was set before cannot be taken away from here
                Ok(None) if v.max_mb.is_none() => {}
                Ok(None) => problems.push((i, Problem::NotANumber)),
                Err(p) => problems.push((i, p)),
            }
        }
        if c.max_mb.is_some() || c.remove_immediately.is_some() {
            changes.volumes.push(c);
        }
    }
    if !now.confirm_locked && edit.confirm_delete != now.confirm_delete {
        changes.confirm_delete = Some(edit.confirm_delete);
    }
    if problems.is_empty() { Ok(changes) } else { Err(problems) }
}

fn parse_size(text: &str, capacity_mb: u64) -> Result<Option<u32>, Problem> {
    if text.is_empty() {
        return Ok(None);
    }
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(Problem::NotANumber);
    }
    let n: u64 = text.parse().map_err(|_| Problem::TooBig(capacity_mb))?;
    if n < 1 {
        return Err(Problem::TooSmall);
    }
    if (capacity_mb > 0 && n > capacity_mb) || n > u32::MAX as u64 {
        return Err(Problem::TooBig(capacity_mb));
    }
    Ok(Some(n as u32))
}

/// Where the settings live. The real one reads and writes the registry (`rf_win::recycle_settings`); reading is harmless,
/// writing is only ever called by [`save`].
pub trait BinSettingsStore {
    fn read(&self) -> anyhow::Result<BinSettings>;
    fn write(&self, changes: &Changes) -> anyhow::Result<()>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Nothing differed; nothing was written.
    Unchanged,
    /// Written: how many drives, and whether the confirmation too.
    Saved { volumes: usize, confirm: bool },
}

/// The Save button: validates, and writes only the differences. A problem is returned before anything is written.
pub fn save(store: &dyn BinSettingsStore, now: &BinSettings, edit: &BinSettingsEdit) -> Result<SaveOutcome, SaveError> {
    let changes = plan(now, edit).map_err(SaveError::Invalid)?;
    if changes.is_empty() {
        return Ok(SaveOutcome::Unchanged);
    }
    store.write(&changes).map_err(|e| SaveError::Failed(format!("{e:#}")))?;
    Ok(SaveOutcome::Saved { volumes: changes.volumes.len(), confirm: changes.confirm_delete.is_some() })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    Invalid(Vec<(usize, Problem)>),
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct FakeBin {
        items: RefCell<Vec<String>>,
        /// Items the "shell" refuses to delete (in use).
        stuck: Vec<String>,
        /// A bug of the shell: deletes this one too.
        also_deletes: Option<String>,
        fail_with: Option<&'static str>,
        calls: RefCell<Vec<Vec<String>>>,
    }

    impl FakeBin {
        fn new(items: &[&str]) -> Self {
            FakeBin { items: RefCell::new(items.iter().map(|s| s.to_string()).collect()), stuck: vec![], also_deletes: None, fail_with: None, calls: RefCell::new(vec![]) }
        }
    }

    impl BinBackend for FakeBin {
        fn list_keys(&self) -> anyhow::Result<Vec<String>> {
            Ok(self.items.borrow().clone())
        }
        fn delete_keys(&self, keys: &[String]) -> anyhow::Result<()> {
            self.calls.borrow_mut().push(keys.to_vec());
            let mut items = self.items.borrow_mut();
            items.retain(|i| !keys.contains(i) || self.stuck.contains(i));
            if let Some(x) = &self.also_deletes {
                items.retain(|i| i != x);
            }
            match self.fail_with {
                Some(m) => Err(anyhow::anyhow!(m)),
                None => Ok(()),
            }
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_command_applies_to_the_selected_rows_that_are_shown_in_the_order_shown() {
        let shown = [4, 2, 7, 1];
        let selected: HashSet<usize> = [1, 4, 9].into_iter().collect();
        // 9 is selected but filtered away
        assert_eq!(command_targets(&shown, &selected), vec![4, 1]);
        assert!(command_targets(&shown, &HashSet::new()).is_empty());
        assert!(command_targets(&[], &selected).is_empty());
    }

    #[test]
    fn a_compressed_folder_is_a_file_and_a_real_folder_is_a_folder() {
        // what the shell says about a zip: folder and stream; the file system says it is a file
        assert!(!is_real_folder(true, true, None));
        assert!(!is_real_folder(true, true, Some(false)));
        assert!(!is_real_folder(true, false, Some(false)));
        assert!(is_real_folder(true, false, None));
        assert!(is_real_folder(true, false, Some(true)));
        assert!(!is_real_folder(false, false, None));
        // the file system wins over the shell
        assert!(is_real_folder(false, false, Some(true)));
    }

    #[test]
    fn the_confirmation_counts_items_folders_and_bytes() {
        let f = delete_facts([(true, 0), (false, 10), (false, 32)]);
        assert_eq!(f, DeleteFacts { count: 3, folders: 1, bytes: 42 });
        assert_eq!(delete_facts([]), DeleteFacts::default());
        assert_eq!(delete_facts([(false, u64::MAX), (false, 5)]).bytes, u64::MAX);
    }

    #[test]
    fn exactly_the_asked_items_are_deleted() {
        let bin = FakeBin::new(&["a", "b", "c", "d"]);
        let r = delete_through(&bin, &s(&["B", "c"]));
        assert_eq!(r, DeleteReport { requested: 2, deleted: 2, left: 0, collateral: 0, error: None, unknown: false });
        assert!(r.all_done());
        assert_eq!(*bin.items.borrow(), s(&["a", "d"]));
        // the backend was asked for the two keys, once
        assert_eq!(*bin.calls.borrow(), vec![s(&["b", "c"])]);
    }

    #[test]
    fn nothing_is_asked_of_the_backend_for_no_keys_or_keys_the_bin_does_not_have() {
        let bin = FakeBin::new(&["a"]);
        assert_eq!(delete_through(&bin, &[]), DeleteReport::default());
        let r = delete_through(&bin, &s(&["zzz"]));
        assert_eq!((r.requested, r.deleted, r.left), (0, 0, 0));
        assert!(bin.calls.borrow().is_empty());
        assert_eq!(*bin.items.borrow(), s(&["a"]));
    }

    #[test]
    fn duplicate_keys_count_once() {
        let bin = FakeBin::new(&["a", "b"]);
        let r = delete_through(&bin, &s(&["a", "A", "a"]));
        assert_eq!((r.requested, r.deleted), (1, 1));
    }

    #[test]
    fn an_item_that_would_not_go_is_counted_as_left() {
        let mut bin = FakeBin::new(&["a", "b", "c"]);
        bin.stuck = s(&["b"]);
        let r = delete_through(&bin, &s(&["a", "b"]));
        assert_eq!((r.requested, r.deleted, r.left, r.collateral), (2, 1, 1, 0));
        assert!(!r.all_done());
    }

    #[test]
    fn an_error_is_reported_with_what_happened_anyway() {
        let mut bin = FakeBin::new(&["a", "b"]);
        bin.fail_with = Some("access denied");
        let r = delete_through(&bin, &s(&["a"]));
        assert_eq!(r.error.as_deref(), Some("access denied"));
        assert_eq!((r.deleted, r.left), (1, 0));
        assert!(!r.all_done());
    }

    #[test]
    fn an_item_that_went_without_being_asked_for_is_reported_as_collateral() {
        let mut bin = FakeBin::new(&["a", "b", "c"]);
        bin.also_deletes = Some("c".into());
        let r = delete_through(&bin, &s(&["a"]));
        assert_eq!((r.deleted, r.collateral), (1, 1));
        assert!(!r.all_done());
    }

    #[test]
    fn a_shell_that_did_not_answer_in_time_is_reported_as_unknown_and_is_not_asked_again() {
        struct Stuck {
            lists: RefCell<usize>,
        }
        impl BinBackend for Stuck {
            fn list_keys(&self) -> anyhow::Result<Vec<String>> {
                *self.lists.borrow_mut() += 1;
                Ok(vec!["a".into(), "b".into()])
            }
            fn delete_keys(&self, _: &[String]) -> anyhow::Result<()> {
                Err(anyhow::Error::new(TimedOut))
            }
        }
        let bin = Stuck { lists: RefCell::new(0) };
        let r = delete_through(&bin, &s(&["a"]));
        assert!(r.unknown && !r.all_done());
        // neither "deleted" nor "left" is claimed, and the (blocked) shell thread was not asked for a second listing
        assert_eq!((r.requested, r.deleted, r.left), (1, 0, 0));
        assert_eq!(*bin.lists.borrow(), 1);
        assert!(r.error.is_some());
    }

    #[test]
    fn a_bin_that_cannot_be_listed_afterwards_claims_nothing() {
        struct DiesAfter {
            lists: RefCell<usize>,
        }
        impl BinBackend for DiesAfter {
            fn list_keys(&self) -> anyhow::Result<Vec<String>> {
                *self.lists.borrow_mut() += 1;
                if *self.lists.borrow() == 1 { Ok(vec!["a".into()]) } else { Err(anyhow::anyhow!("gone")) }
            }
            fn delete_keys(&self, _: &[String]) -> anyhow::Result<()> {
                Ok(())
            }
        }
        let r = delete_through(&DiesAfter { lists: RefCell::new(0) }, &s(&["a"]));
        assert!(r.unknown && !r.all_done());
        assert_eq!((r.deleted, r.left), (0, 0));
    }

    #[test]
    fn a_link_to_a_folder_is_left_to_the_shell_to_judge() {
        // a junction: not a directory to a look that does not follow links, but a link
        assert_eq!(fs_dir_answer(false, true), None);
        assert!(is_real_folder(true, false, fs_dir_answer(false, true)));
        // a plain file (a zip) and a plain directory are answered by the file system
        assert_eq!(fs_dir_answer(false, false), Some(false));
        assert_eq!(fs_dir_answer(true, false), Some(true));
        // a link that the shell also calls a stream is still a file
        assert!(!is_real_folder(true, true, fs_dir_answer(false, true)));
    }

    #[test]
    fn a_size_the_user_did_not_touch_is_not_checked() {
        // the drive shrank (or the value is stale): 10000 MB on a 500 MB drive
        let mut now = settings();
        now.volumes[0].capacity_mb = 500;
        let mut e = initial_edit(&now);
        // nothing touched: nothing planned and no problem; the confirmation can still be saved
        assert_eq!(plan(&now, &e), Ok(Changes::default()));
        e.confirm_delete = false;
        let c = plan(&now, &e).unwrap();
        assert_eq!(c.confirm_delete, Some(false));
        assert!(c.volumes.is_empty());
        // a size the user typed is checked
        e.volumes[0].size_text = "10001".into();
        assert_eq!(plan(&now, &e), Err(vec![(0, Problem::TooBig(500))]));
    }

    #[test]
    fn a_bin_that_cannot_be_listed_claims_nothing() {
        struct Broken;
        impl BinBackend for Broken {
            fn list_keys(&self) -> anyhow::Result<Vec<String>> {
                Err(anyhow::anyhow!("no bin"))
            }
            fn delete_keys(&self, _: &[String]) -> anyhow::Result<()> {
                panic!("must not be called when the bin cannot be listed");
            }
        }
        let r = delete_through(&Broken, &s(&["a"]));
        assert_eq!((r.requested, r.deleted, r.left), (1, 0, 1));
        assert_eq!(r.error.as_deref(), Some("no bin"));
    }

    fn settings() -> BinSettings {
        BinSettings {
            volumes: vec![
                VolumeBin { id: "{AAA}".into(), label: "C:".into(), capacity_mb: 500_000, max_mb: Some(10_000), remove_immediately: false },
                VolumeBin { id: "{BBB}".into(), label: "D:".into(), capacity_mb: 1_000, max_mb: None, remove_immediately: false },
            ],
            confirm_delete: true,
            confirm_locked: false,
        }
    }

    struct FakeStore {
        now: BinSettings,
        written: RefCell<Vec<Changes>>,
        fail: bool,
    }
    impl BinSettingsStore for FakeStore {
        fn read(&self) -> anyhow::Result<BinSettings> {
            Ok(self.now.clone())
        }
        fn write(&self, c: &Changes) -> anyhow::Result<()> {
            if self.fail {
                anyhow::bail!("registry refused");
            }
            self.written.borrow_mut().push(c.clone());
            Ok(())
        }
    }

    #[test]
    fn the_controls_start_as_the_settings_read() {
        let e = initial_edit(&settings());
        assert_eq!(e.volumes[0], VolumeEdit { remove_immediately: false, size_text: "10000".into() });
        assert_eq!(e.volumes[1], VolumeEdit { remove_immediately: false, size_text: String::new() });
        assert!(e.confirm_delete);
        // untouched controls plan nothing
        assert_eq!(plan(&settings(), &e), Ok(Changes::default()));
    }

    #[test]
    fn only_what_differs_is_planned() {
        let now = settings();
        let mut e = initial_edit(&now);
        e.volumes[0].size_text = " 2048 ".into();
        e.confirm_delete = false;
        let c = plan(&now, &e).unwrap();
        assert_eq!(c.volumes, vec![VolumeChange { id: "{AAA}".into(), max_mb: Some(2048), remove_immediately: None }]);
        assert_eq!(c.confirm_delete, Some(false));
        // a first custom size on the drive that had none, and the immediate removal of the first
        let mut e = initial_edit(&now);
        e.volumes[1].size_text = "500".into();
        e.volumes[0].remove_immediately = true;
        e.volumes[0].size_text = "not looked at".into();
        let c = plan(&now, &e).unwrap();
        assert_eq!(c.volumes.len(), 2);
        assert_eq!(c.volumes[0], VolumeChange { id: "{AAA}".into(), max_mb: None, remove_immediately: Some(true) });
        assert_eq!(c.volumes[1], VolumeChange { id: "{BBB}".into(), max_mb: Some(500), remove_immediately: None });
        assert_eq!(c.confirm_delete, None);
    }

    #[test]
    fn sizes_are_checked() {
        let now = settings();
        let with = |t: &str| {
            let mut e = initial_edit(&now);
            e.volumes[1].size_text = t.into();
            plan(&now, &e)
        };
        assert_eq!(with("0"), Err(vec![(1, Problem::TooSmall)]));
        assert_eq!(with("1001"), Err(vec![(1, Problem::TooBig(1_000))]));
        assert_eq!(with("1000").map(|c| c.volumes.len()), Ok(1));
        assert_eq!(with("12a"), Err(vec![(1, Problem::NotANumber)]));
        assert_eq!(with("-5"), Err(vec![(1, Problem::NotANumber)]));
        assert_eq!(with("1.5"), Err(vec![(1, Problem::NotANumber)]));
        assert_eq!(with("99999999999999999999"), Err(vec![(1, Problem::TooBig(1_000))]));
        // empty on a drive without a custom size: nothing to do
        assert_eq!(with(""), Ok(Changes::default()));
        // a custom size cannot be emptied away
        let mut e = initial_edit(&now);
        e.volumes[0].size_text.clear();
        assert_eq!(plan(&now, &e), Err(vec![(0, Problem::NotANumber)]));
        // an unknown drive size accepts any size that fits the registry value
        let mut unknown = settings();
        unknown.volumes[1].capacity_mb = 0;
        let mut e = initial_edit(&unknown);
        e.volumes[1].size_text = "4000000".into();
        assert!(plan(&unknown, &e).is_ok());
        e.volumes[1].size_text = "4294967296".into();
        assert!(plan(&unknown, &e).is_err());
    }

    #[test]
    fn a_confirmation_fixed_by_policy_is_not_written() {
        let mut now = settings();
        now.confirm_locked = true;
        let mut e = initial_edit(&now);
        e.confirm_delete = false;
        assert_eq!(plan(&now, &e), Ok(Changes::default()));
    }

    #[test]
    fn save_writes_only_on_a_valid_change() {
        let now = settings();
        let store = FakeStore { now: now.clone(), written: RefCell::new(vec![]), fail: false };
        // nothing changed: nothing written
        assert_eq!(save(&store, &now, &initial_edit(&now)), Ok(SaveOutcome::Unchanged));
        assert!(store.written.borrow().is_empty());
        // a problem: nothing written
        let mut bad = initial_edit(&now);
        bad.volumes[0].size_text = "0".into();
        bad.confirm_delete = false;
        assert_eq!(save(&store, &now, &bad), Err(SaveError::Invalid(vec![(0, Problem::TooSmall)])));
        assert!(store.written.borrow().is_empty());
        // a good change: written once, with only the differences
        let mut good = initial_edit(&now);
        good.confirm_delete = false;
        assert_eq!(save(&store, &now, &good), Ok(SaveOutcome::Saved { volumes: 0, confirm: true }));
        assert_eq!(store.written.borrow().len(), 1);
        assert_eq!(store.written.borrow()[0].confirm_delete, Some(false));
        // the store failing is an error, not a success
        let broken = FakeStore { now: now.clone(), written: RefCell::new(vec![]), fail: true };
        assert_eq!(save(&broken, &now, &good), Err(SaveError::Failed("registry refused".into())));
    }
}
