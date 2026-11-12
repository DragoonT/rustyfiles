//! Saving the settings file without losing another writer's change.
//!
//! WHAT WAS SEEN (the user's second trial): an older process of the app, on exit, wrote its whole in-memory settings over the file and
//! undid a change another writer had made half an hour earlier (the desktop icons vanished). Every writer used to serialize the WHOLE
//! struct it holds; whoever saved last decided every key, including the ones it never touched and the ones it does not know.
//!
//! THE RULE NOW. A process remembers what the file held when it read it (its BASELINE, as a TOML table in the same form a save
//! produces). A save then:
//!
//! 1. works out what THIS process changed: the keys (at any depth of nested tables) whose value differs from the baseline, and the keys
//!    that were in the baseline and are no longer written (a value set back to its default is not written at all);
//! 2. if that is nothing, writes nothing: a process that changed nothing never touches the file, on exit or otherwise;
//! 3. reads the file as it is NOW (under a lock shared by all processes of the user's session, so two saves do not interleave), puts
//!    those changes on top of it and writes the result by a temporary file and a rename;
//! 4. keeps every key it does not know (a newer build's setting survives an older build's save), and every key of the file it did not
//!    change.
//!
//! TWO WRITERS: different keys, both survive. The same key: the one that saves last wins, as it must. A key in a nested table
//! (`[columns]`, `[keybindings]` ...): the same, per key of the nested table.
//!
//! A FILE THAT CANNOT BE UNDERSTOOD when this process has changes to save: it is set aside (`settings.toml.bozuk-<time>`, the same as
//! at start-up) and the whole of this process's settings are written, so the user's changes are not lost and the broken file is kept
//! for repair. A file that cannot be READ right now (locked by another program): nothing is written and the baseline stays, so that the
//! next save of this process carries the same changes; a save at exit has no next one, which is the price of not overwriting a file
//! that could not be looked at. A file that is missing: the whole of this process's settings are written.
//!
//! This file is the pure rule ([`plan`], tested on tables) and the small amount of file work around it ([`save_with`], tested on temp
//! files with several threads).

use std::path::Path;

use toml::{Table, Value};

/// One change this process made.
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    /// The key and, for a key inside nested tables, the tables above it.
    pub path: Vec<String>,
    /// `Some`: the new value. `None`: the key is no longer written.
    pub value: Option<Value>,
}

/// What this process changed relative to its baseline.
pub fn changes(baseline: &Table, mine: &Table) -> Vec<Change> {
    let mut out = Vec::new();
    diff(baseline, mine, &mut Vec::new(), &mut out);
    out
}

fn diff(base: &Table, mine: &Table, path: &mut Vec<String>, out: &mut Vec<Change>) {
    for (k, v) in mine {
        path.push(k.clone());
        match (base.get(k), v) {
            (Some(Value::Table(b)), Value::Table(m)) => diff(b, m, path, out),
            (Some(b), m) if b == m => {}
            _ => out.push(Change { path: path.clone(), value: Some(v.clone()) }),
        }
        path.pop();
    }
    for k in base.keys().filter(|k| !mine.contains_key(*k)) {
        path.push(k.clone());
        out.push(Change { path: path.clone(), value: None });
        path.pop();
    }
}

/// Puts `changes` on top of `file` (which keeps everything else, unknown keys included).
pub fn apply(mut file: Table, changes: &[Change]) -> Table {
    for c in changes {
        set(&mut file, &c.path, c.value.clone());
    }
    file
}

fn set(table: &mut Table, path: &[String], value: Option<Value>) {
    let Some((first, rest)) = path.split_first() else { return };
    if rest.is_empty() {
        match value {
            Some(v) => {
                table.insert(first.clone(), v);
            }
            None => {
                table.remove(first);
            }
        }
        return;
    }
    // (a file whose shape differs from ours here, a value where we have a table: ours replaces it)
    match table.get_mut(first) {
        Some(Value::Table(t)) => set(t, rest, value),
        _ => {
            if value.is_none() {
                return;
            }
            let mut t = Table::new();
            set(&mut t, rest, value);
            table.insert(first.clone(), Value::Table(t));
        }
    }
}

/// What the file held when it was looked at.
#[derive(Debug)]
pub enum FileState {
    Missing,
    Table(Table),
    /// It is there and cannot be understood (not UTF-8 or not TOML).
    Broken,
    /// It is there and cannot be read now (locked).
    Unreadable,
}

/// What a save does.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Nothing is written.
    Nothing,
    /// This table is written over the file.
    Write(Table),
    /// The file is set aside, then this table (the whole of this process's settings) is written.
    ReplaceBroken(Table),
}

/// The whole decision. `baseline` and `mine` as in [`changes`].
pub fn plan(baseline: &Table, mine: &Table, file: FileState) -> Plan {
    let ch = changes(baseline, mine);
    if ch.is_empty() {
        return Plan::Nothing;
    }
    match file {
        FileState::Missing => Plan::Write(mine.clone()),
        FileState::Broken => Plan::ReplaceBroken(mine.clone()),
        FileState::Unreadable => Plan::Nothing,
        FileState::Table(t) => {
            let merged = apply(t.clone(), &ch);
            // the other writer already put the same values there: no write
            if merged == t { Plan::Nothing } else { Plan::Write(merged) }
        }
    }
}

/// What [`save_with`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    /// Nothing to write.
    Nothing,
    Wrote,
    /// The file was set aside as broken, and a new one written.
    ReplacedBroken,
    /// The file could not be read; nothing written (the baseline stays).
    Unreadable,
    /// The write failed.
    Failed,
}

/// Reads the file at `path`.
pub fn read_state(path: &Path) -> FileState {
    match rf_core::read_config(path) {
        Ok(text) => match toml::from_str::<Table>(text.trim_start_matches('\u{feff}')) {
            Ok(t) => FileState::Table(t),
            Err(e) => {
                log::warn!("settings: {e}");
                FileState::Broken
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileState::Missing,
        // (not UTF-8: as broken as a typo)
        Err(e) if e.kind() == std::io::ErrorKind::InvalidData => FileState::Broken,
        Err(e) => {
            log::warn!("settings: the file cannot be read now ({e}); nothing was saved");
            FileState::Unreadable
        }
    }
}

/// Saves `mine` into the file at `path` by the rule above. Under a lock that all processes of the session share.
pub fn save_with(path: &Path, baseline: &Table, mine: &Table) -> Saved {
    // cheap exit before any file work: nothing changed
    if changes(baseline, mine).is_empty() {
        return Saved::Nothing;
    }
    let _lock = FileLock::acquire(path);
    let state = read_state(path);
    let unreadable = matches!(state, FileState::Unreadable);
    let (table, replaced) = match plan(baseline, mine, state) {
        Plan::Nothing => return if unreadable { Saved::Unreadable } else { Saved::Nothing },
        Plan::Write(t) => (t, false),
        Plan::ReplaceBroken(t) => (t, true),
    };
    let text = match toml::to_string_pretty(&table) {
        Ok(t) => t,
        Err(e) => {
            log::warn!("settings save: {e}");
            return Saved::Failed;
        }
    };
    if replaced {
        log::warn!("settings: the file cannot be understood; it is set aside and this process's settings are written");
        rf_core::backup_corrupt(path);
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // write-then-rename so a crash never leaves a half-written file; a name of its own per write
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("toml.{}.{}.tmp", std::process::id(), n));
    if std::fs::write(&tmp, text).is_ok() && std::fs::rename(&tmp, path).is_ok() {
        if replaced { Saved::ReplacedBroken } else { Saved::Wrote }
    } else {
        let _ = std::fs::remove_file(&tmp);
        Saved::Failed
    }
}

/// What a process last knew the settings file to hold, kept between its reads and its saves.
pub type BaselineCell = parking_lot::Mutex<Option<Table>>;

/// The process has just taken the file's state as its own (`file` is the table the file gave, before this process changed anything of
/// it): from now on only what it changes on top of THIS is its change. A process that follows the file (the dock reloads it when
/// another process writes) must call this each time it replaces its settings with a reload; otherwise its baseline stays at the
/// start-up table, what another writer set since looks like the follower's own change, and its next save writes that old value back
/// over whatever the file has become meanwhile.
pub fn follow(cell: &BaselineCell, file: Table) {
    *cell.lock() = Some(file);
}

/// [`save_with`] against the baseline in `cell` (`default_base` stands in when none was taken yet), and the baseline moves to what was
/// saved when the save went through (or needed no write); a save that did not happen keeps it, so the next one carries the same changes.
pub fn save_tracked(cell: &BaselineCell, path: &Path, mine: Table, default_base: impl FnOnce() -> Table) -> Saved {
    let baseline = cell.lock().clone().unwrap_or_else(default_base);
    let saved = save_with(path, &baseline, &mine);
    match saved {
        Saved::Wrote | Saved::ReplacedBroken | Saved::Nothing => *cell.lock() = Some(mine),
        Saved::Unreadable | Saved::Failed => {}
    }
    saved
}

/// A mutex shared by every process of the user's session that saves this file, so that two saves do not read-modify-write at the
/// same moment. Waits at most three seconds; if it cannot be had the save goes on without it (the window for a lost change is the few
/// milliseconds between a read and a rename, against a settings file that would never be saved at all).
struct FileLock(windows::Win32::Foundation::HANDLE);

impl FileLock {
    fn acquire(path: &Path) -> Option<FileLock> {
        use windows::Win32::Foundation::{CloseHandle, WAIT_ABANDONED, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{CreateMutexW, WaitForSingleObject};
        use windows::core::PCWSTR;
        let key = path.to_string_lossy().to_lowercase();
        let mut h: u32 = 0x811c_9dc5;
        for b in key.bytes() {
            h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
        }
        let name = rf_win::wide::wide(format!("Local\\rustyfiles-settings-{h:08x}"));
        let handle = unsafe { CreateMutexW(None, false, PCWSTR(name.as_ptr())) }.ok()?;
        let got = unsafe { WaitForSingleObject(handle, 3_000) };
        if got == WAIT_OBJECT_0 || got == WAIT_ABANDONED {
            Some(FileLock(handle))
        } else {
            log::warn!("settings: the save lock could not be had in 3 s; saving without it");
            let _ = unsafe { CloseHandle(handle) };
            None
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::ReleaseMutex;
        unsafe {
            let _ = ReleaseMutex(self.0);
            let _ = CloseHandle(self.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> Table {
        toml::from_str(text).unwrap()
    }

    fn written(p: Plan) -> Table {
        match p {
            Plan::Write(t) | Plan::ReplaceBroken(t) => t,
            Plan::Nothing => panic!("nothing was written"),
        }
    }

    #[test]
    fn two_writers_changing_different_keys_both_survive() {
        // both read the same file; A changes `a`, B changes `b`; B saves after A has
        let base = table("a = 1\nb = 1\nc = 1\n");
        let file_after_a = table("a = 2\nb = 1\nc = 1\n");
        let b_mine = table("a = 1\nb = 2\nc = 1\n");
        let merged = written(plan(&base, &b_mine, FileState::Table(file_after_a)));
        assert_eq!(merged, table("a = 2\nb = 2\nc = 1\n"), "B did not undo A's change to `a`");
    }

    #[test]
    fn the_same_key_changed_by_both_the_last_to_save_wins() {
        let base = table("a = 1\n");
        let file_after_a = table("a = 2\n");
        let b_mine = table("a = 3\n");
        assert_eq!(written(plan(&base, &b_mine, FileState::Table(file_after_a))), table("a = 3\n"));
    }

    #[test]
    fn a_key_this_build_does_not_know_survives_its_save() {
        // the file has a key of a newer build, at the top and inside a nested table
        let base = table("a = 1\n[columns]\nsize = true\n");
        let file = table("a = 1\nfuture_switch = true\n[columns]\nsize = true\nfuture_column = \"x\"\n[future_table]\nk = 1\n");
        let mine = table("a = 2\n[columns]\nsize = false\n");
        let merged = written(plan(&base, &mine, FileState::Table(file)));
        assert_eq!(merged, table("a = 2\nfuture_switch = true\n[columns]\nsize = false\nfuture_column = \"x\"\n[future_table]\nk = 1\n"));
    }

    #[test]
    fn a_process_that_changed_nothing_never_writes_whatever_the_file_holds() {
        let base = table("a = 1\nb = 1\n");
        let mine = base.clone();
        for file in [FileState::Table(table("a = 9\nb = 9\n")), FileState::Missing, FileState::Broken, FileState::Unreadable] {
            assert_eq!(plan(&base, &mine, file), Plan::Nothing);
        }
    }

    #[test]
    fn a_value_set_back_to_its_default_is_no_longer_written_and_leaves_the_rest_alone() {
        // the settings leave a default value out of the file; this process set `pos` back to the default
        let base = table("a = 1\npos = \"bottom\"\n");
        let mine = table("a = 1\n");
        let file = table("a = 5\npos = \"bottom\"\nother = 2\n");
        assert_eq!(written(plan(&base, &mine, FileState::Table(file))), table("a = 5\nother = 2\n"));
    }

    #[test]
    fn nested_tables_merge_key_by_key() {
        let base = table("[keys]\nx = \"1\"\n");
        // A added `y`; B added `z`
        let file_after_a = table("[keys]\nx = \"1\"\ny = \"A\"\n");
        let b_mine = table("[keys]\nx = \"1\"\nz = \"B\"\n");
        assert_eq!(written(plan(&base, &b_mine, FileState::Table(file_after_a))), table("[keys]\nx = \"1\"\ny = \"A\"\nz = \"B\"\n"));
        // a table that did not exist in the file is made
        let b_mine = table("[keys]\nz = \"B\"\n");
        assert_eq!(written(plan(&table(""), &b_mine, FileState::Table(table("a = 1\n")))), table("a = 1\n[keys]\nz = \"B\"\n"));
        // a value where we have a table: ours replaces it
        assert_eq!(written(plan(&table(""), &b_mine, FileState::Table(table("keys = 3\n")))), table("[keys]\nz = \"B\"\n"));
    }

    #[test]
    fn a_missing_file_gets_the_whole_of_this_processs_settings_and_a_broken_one_is_set_aside_first() {
        let base = table("a = 1\n");
        let mine = table("a = 2\nb = 3\n");
        assert_eq!(plan(&base, &mine, FileState::Missing), Plan::Write(mine.clone()));
        assert_eq!(plan(&base, &mine, FileState::Broken), Plan::ReplaceBroken(mine.clone()));
        // a file nobody can read now: nothing is written, the caller keeps its baseline
        assert_eq!(plan(&base, &mine, FileState::Unreadable), Plan::Nothing);
    }

    #[test]
    fn a_file_that_already_has_the_values_is_not_written_again() {
        let base = table("a = 1\n");
        let mine = table("a = 2\n");
        assert_eq!(plan(&base, &mine, FileState::Table(table("a = 2\n"))), Plan::Nothing);
    }

    #[test]
    fn the_real_settings_a_desktop_switch_from_one_writer_and_a_session_from_another() {
        use crate::settings::Settings;
        // the old process holds the defaults; another writer turned the desktop cover on (and a newer build added a key)
        let old_base = Table::try_from(Settings::default()).unwrap();
        let mut file = old_base.clone();
        file.insert("shell_desktop_cover".into(), Value::Boolean(true));
        file.insert("a_key_from_the_future".into(), Value::String("kept".into()));
        // the old process, on exit, has only its session
        let old = Settings { session_tabs: vec!["C:\\x".into()], session_active: 0, ..Default::default() };
        let mine = Table::try_from(&old).unwrap();
        let merged = written(plan(&old_base, &mine, FileState::Table(file)));
        assert_eq!(merged.get("shell_desktop_cover"), Some(&Value::Boolean(true)), "the other writer's switch is still on");
        assert_eq!(merged.get("a_key_from_the_future"), Some(&Value::String("kept".into())));
        let text = toml::to_string_pretty(&merged).unwrap();
        let back = Settings::parse_lenient_pub(&text).unwrap();
        assert!(back.shell_desktop_cover);
        assert_eq!(back.session_tabs, vec!["C:\\x".to_string()]);
    }

    #[test]
    fn the_real_settings_write_nothing_when_nothing_changed() {
        use crate::settings::Settings;
        let base = Table::try_from(Settings::default()).unwrap();
        let mine = Table::try_from(Settings::default()).unwrap();
        assert!(changes(&base, &mine).is_empty());
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rf-merge-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("settings.toml")
    }

    #[test]
    fn saving_to_a_file_keeps_the_other_writers_change_and_an_unchanged_process_writes_nothing() {
        let p = scratch("file");
        std::fs::write(&p, "a = 1\nb = 1\nfuture = true\n").unwrap();
        let base = table("a = 1\nb = 1\n");
        // another writer changes `a` after both read
        std::fs::write(&p, "a = 7\nb = 1\nfuture = true\n").unwrap();
        // this process changed `b`
        assert_eq!(save_with(&p, &base, &table("a = 1\nb = 2\n")), Saved::Wrote);
        assert_eq!(table(&std::fs::read_to_string(&p).unwrap()), table("a = 7\nb = 2\nfuture = true\n"));
        // unchanged: not even looked at
        let stamp = std::fs::metadata(&p).unwrap().modified().unwrap();
        assert_eq!(save_with(&p, &base, &base.clone()), Saved::Nothing);
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), stamp);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_broken_file_is_set_aside_and_the_processs_settings_are_written() {
        let p = scratch("broken");
        std::fs::write(&p, "this is = = not toml [").unwrap();
        let base = table("a = 1\n");
        assert_eq!(save_with(&p, &base, &table("a = 2\n")), Saved::ReplacedBroken);
        assert_eq!(table(&std::fs::read_to_string(&p).unwrap()), table("a = 2\n"));
        let kept = std::fs::read_dir(p.parent().unwrap()).unwrap().filter_map(Result::ok).filter(|e| e.file_name().to_string_lossy().contains(".bozuk-")).count();
        assert_eq!(kept, 1, "the broken file is kept for repair");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_follower_that_took_the_reload_as_its_baseline_does_not_write_the_reloaded_values_back() {
        // The dock process: starts with a=0; another writer sets a=1; the dock reloads (memory a=1). Then the file moves on to a=2.
        // The dock's slider (key `size`) is saved afterwards.
        let p = scratch("follow");
        let cell: BaselineCell = parking_lot::Mutex::new(Some(table("a = 0
size = 5
")));
        // the reload: the dock's memory is now what the file held (a = 1)
        follow(&cell, table("a = 1
size = 5
"));
        std::fs::write(&p, "a = 2
size = 5
").unwrap();
        let mine = table("a = 1
size = 9
");
        assert_eq!(save_tracked(&cell, &p, mine, || table("")), Saved::Wrote);
        assert_eq!(table(&std::fs::read_to_string(&p).unwrap()), table("a = 2
size = 9
"), "the file's newer `a` survived the dock's slider");
        // and the baseline moved to what this process holds: the same save again changes nothing
        assert_eq!(save_tracked(&cell, &p, table("a = 1
size = 9
"), || table("")), Saved::Nothing);

        // without following (the old behaviour): the stale baseline made `a` look like the dock's own change
        let stale: BaselineCell = parking_lot::Mutex::new(Some(table("a = 0
size = 5
")));
        std::fs::write(&p, "a = 2
size = 5
").unwrap();
        assert_eq!(save_tracked(&stale, &p, table("a = 1
size = 9
"), || table("")), Saved::Wrote);
        assert_eq!(table(&std::fs::read_to_string(&p).unwrap()), table("a = 1
size = 9
"), "this is the loss `follow` prevents");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn a_save_that_did_not_happen_keeps_the_baseline_so_the_next_one_carries_the_change() {
        let p = scratch("keep");
        let cell: BaselineCell = parking_lot::Mutex::new(Some(table("a = 0
")));
        // a directory where the file should be: the file cannot be read as a file, the write fails
        std::fs::create_dir_all(&p).unwrap();
        let first = save_tracked(&cell, &p, table("a = 1
"), || table(""));
        assert!(matches!(first, Saved::Failed | Saved::Unreadable), "{first:?}");
        assert_eq!(cell.lock().clone(), Some(table("a = 0
")), "the change is still this process's to save");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn many_writers_at_once_each_changing_its_own_key_lose_nothing() {
        let p = scratch("threads");
        std::fs::write(&p, (0..8).map(|i| format!("k{i} = 0\n")).collect::<String>()).unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let mut base = table(&(0..8).map(|j| format!("k{j} = 0\n")).collect::<String>());
                    for round in 1..=25 {
                        let mut mine = base.clone();
                        mine.insert(format!("k{i}"), Value::Integer(round));
                        assert!(matches!(save_with(&p, &base, &mine), Saved::Wrote));
                        base = mine;
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let end = table(&std::fs::read_to_string(&p).unwrap());
        for i in 0..8 {
            assert_eq!(end.get(&format!("k{i}")), Some(&Value::Integer(25)), "writer {i}'s last value");
        }
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
