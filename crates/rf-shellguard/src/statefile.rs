//! `shell-state.json` (plan 4.5): the one file the guard and the shell host share. The guard reads
//! it on every start, so a crash in the middle of a change (work areas moved, the Windows taskbar
//! hidden) is undone, and writes the crash log and the reason of a fall-back into it.
//!
//! Rules this file keeps:
//! * Only the guard's own keys are edited; every other key of the document is written back as it was
//!   (the host's keys, keys of a later version).
//! * A file that does not parse is moved aside as `*.corrupt` and replaced by defaults; the guard
//!   then treats a restore as pending, because it cannot know whether one was.
//! * A file from a newer version is read for the keys that still make sense but never rewritten.
//! * Writes are atomic: a temp file in the same folder, then a rename.
//! * `peek` reads and never writes, so `--dry-run` can use it.

use std::io;
use std::path::{Path, PathBuf};

use crate::json::Json;
use crate::machine::{Crash, Persist, Reason};
use crate::tbmodel::{HideRecord, Target};

pub const STATE_VERSION: u64 = 1;
/// A state file larger than this is not ours.
pub const MAX_BYTES: u64 = 256 * 1024;
pub const FILE_NAME: &str = "shell-state.json";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    Missing,
    Ok,
    /// Did not parse, or was not an object, or was too large; the text says why.
    Corrupt(String),
    /// `version` is higher than this build understands.
    FromNewer(u64),
}

/// One saved work area: the monitor's device name and its rectangle (left, top, right, bottom).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkAreaRec {
    pub monitor: String,
    pub rect: [i32; 4],
}

#[derive(Clone, Debug)]
pub struct ShellStateFile {
    doc: Json,
    pub status: LoadStatus,
    /// False for a file from a newer version: `save` refuses.
    pub writable: bool,
}

pub fn path_in(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

impl ShellStateFile {
    pub fn empty() -> ShellStateFile {
        ShellStateFile { doc: Json::Obj(vec![("version".into(), Json::num(STATE_VERSION))]), status: LoadStatus::Missing, writable: true }
    }

    /// Reads the file and never changes anything on disk.
    pub fn peek(path: &Path) -> ShellStateFile {
        let mut f = ShellStateFile::empty();
        let meta = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return f,
            Err(e) => {
                f.status = LoadStatus::Corrupt(format!("cannot stat: {e}"));
                return f;
            }
        };
        if meta.len() > MAX_BYTES {
            f.status = LoadStatus::Corrupt(format!("{} bytes is too large", meta.len()));
            return f;
        }
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                f.status = LoadStatus::Corrupt(format!("cannot read: {e}"));
                return f;
            }
        };
        let Ok(text) = String::from_utf8(bytes) else {
            f.status = LoadStatus::Corrupt("not UTF-8".into());
            return f;
        };
        match Json::parse(text.trim_start_matches('\u{feff}')) {
            Ok(doc) if doc.is_obj() => {
                let version = doc.get("version").and_then(Json::as_u64).unwrap_or(STATE_VERSION);
                f.doc = doc;
                if version > STATE_VERSION {
                    f.status = LoadStatus::FromNewer(version);
                    f.writable = false;
                } else {
                    f.status = LoadStatus::Ok;
                }
            }
            Ok(_) => f.status = LoadStatus::Corrupt("not a JSON object".into()),
            Err(e) => f.status = LoadStatus::Corrupt(e),
        }
        f
    }

    /// `peek`, and a corrupt file is moved aside (`shell-state.json.corrupt`) so the next write starts clean.
    pub fn load(path: &Path) -> ShellStateFile {
        let f = ShellStateFile::peek(path);
        if matches!(f.status, LoadStatus::Corrupt(_)) {
            let mut aside = path.as_os_str().to_owned();
            aside.push(".corrupt");
            // Best effort: if this fails the next atomic write replaces the file anyway.
            let _ = std::fs::rename(path, PathBuf::from(aside));
        }
        f
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if !self.writable {
            return Err(io::Error::other("the state file is from a newer version; not rewritten"));
        }
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        let result = std::fs::write(&tmp, self.doc.to_pretty()).and_then(|()| std::fs::rename(&tmp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
    }

    pub fn is_corrupt(&self) -> bool {
        matches!(self.status, LoadStatus::Corrupt(_))
    }

    pub fn mode(&self) -> Option<&str> {
        self.doc.get("mode").and_then(Json::as_str)
    }

    /// The shell host program, written by the Settings switch next to the Shell value.
    pub fn host_exe(&self) -> Option<&str> {
        self.doc.get("host_exe").and_then(Json::as_str).filter(|s| !s.is_empty())
    }

    pub fn set_host_exe(&mut self, path: &str) {
        self.doc.set("host_exe", Json::Str(path.to_string()));
    }

    /// The process id of the host this guard started last and has not seen end. A `--panic` process that finds no
    /// guard to hand the job to uses it to end a host that outlived its guard (checked against the host's image
    /// before anything is ended, because a process id can be reused).
    pub fn host_pid(&self) -> Option<u32> {
        self.doc.get("host_pid").and_then(Json::as_u64).and_then(|p| u32::try_from(p).ok()).filter(|p| *p != 0)
    }

    pub fn set_host_pid(&mut self, pid: Option<u32>) {
        match pid {
            Some(p) => self.doc.set("host_pid", Json::num(u64::from(p))),
            None => self.doc.remove("host_pid"),
        }
    }

    pub fn last_known_good_build(&self) -> Option<&str> {
        self.doc.get("last_known_good_build").and_then(Json::as_str)
    }

    /// Plain numbers (the plan's first layout) and `{"t":..,"up":..}` objects are both read.
    pub fn crashes(&self) -> Vec<Crash> {
        let Some(list) = self.doc.get("crashes").and_then(Json::as_arr) else { return Vec::new() };
        list.iter()
            .filter_map(|v| match v {
                Json::Num(_) => v.as_u64().map(|at| Crash { at, uptime_ms: u64::MAX }),
                Json::Obj(_) => {
                    let at = v.get("t").and_then(Json::as_u64)?;
                    Some(Crash { at, uptime_ms: v.get("up").and_then(Json::as_u64).unwrap_or(u64::MAX) })
                }
                _ => None,
            })
            .collect()
    }

    pub fn fallback_reason(&self) -> Option<Reason> {
        self.doc.get("fallback_reason").and_then(Json::as_str).and_then(Reason::parse)
    }

    pub fn value_cleared(&self) -> Option<bool> {
        self.doc.get("value_cleared").and_then(Json::as_bool)
    }

    /// A fall-back wrote its reason and never recorded that the Shell value was gone.
    pub fn unresolved(&self) -> Option<Reason> {
        match (self.fallback_reason(), self.value_cleared()) {
            (Some(r), Some(false)) if r.deletes_value() => Some(r),
            _ => None,
        }
    }

    /// The host sets this before it changes work areas or hides the taskbar, and clears it after it put them back.
    pub fn pending_restore(&self) -> bool {
        self.doc.get("pending_restore").and_then(Json::as_bool).unwrap_or(false)
    }

    pub fn original_work_areas(&self) -> Vec<WorkAreaRec> {
        let Some(list) = self.doc.get("original_work_areas").and_then(Json::as_arr) else { return Vec::new() };
        list.iter()
            .filter_map(|v| {
                let monitor = v.get("monitor")?.as_str()?.to_string();
                let r = v.get("rect")?.as_arr()?;
                if r.len() != 4 {
                    return None;
                }
                let n = |i: usize| r[i].as_i64().and_then(|x| i32::try_from(x).ok());
                Some(WorkAreaRec { monitor, rect: [n(0)?, n(1)?, n(2)?, n(3)?] })
            })
            .collect()
    }

    /// What the host writes before it changes work areas (the guard only reads them back).
    pub fn set_original_work_areas(&mut self, areas: &[WorkAreaRec]) {
        let list = areas
            .iter()
            .map(|a| {
                let rect = a.rect.iter().map(|n| Json::Num(n.to_string())).collect();
                Json::Obj(vec![("monitor".to_string(), Json::Str(a.monitor.clone())), ("rect".to_string(), Json::Arr(rect))])
            })
            .collect();
        self.doc.set("original_work_areas", Json::Arr(list));
    }

    pub fn set_pending_restore(&mut self, on: bool) {
        self.doc.set("pending_restore", Json::Bool(on));
    }

    /// `hidden_taskbar` (shell plan E1): which Explorer taskbar windows the dock host hid, written BEFORE the first one is hidden.
    /// A record with ANY entry that is not understood, without a usable host identity or without any window is `None` as a whole
    /// (the restore then falls back to showing every hidden taskbar window of Explorer). Trimming it instead would show the entries
    /// that parse, remove the key, and leave the others hidden with nothing left that remembers them.
    pub fn hidden_taskbar(&self) -> Option<HideRecord> {
        let o = self.doc.get("hidden_taskbar")?;
        let host_pid = o.get("host_pid").and_then(Json::as_u64).and_then(|p| u32::try_from(p).ok()).filter(|p| *p != 0)?;
        let host_start = o.get("host_start").and_then(Json::as_u64).unwrap_or(0);
        let at_ms = o.get("at").and_then(Json::as_u64).unwrap_or(0);
        let windows: Vec<Target> = o
            .get("windows")
            .and_then(Json::as_arr)?
            .iter()
            .map(|v| {
                Some(Target {
                    handle: isize::try_from(v.get("hwnd")?.as_i64()?).ok()?,
                    class: v.get("class")?.as_str()?.to_string(),
                    pid: u32::try_from(v.get("pid")?.as_u64()?).ok()?,
                    start: v.get("start").and_then(Json::as_u64).unwrap_or(0),
                })
            })
            .collect::<Option<Vec<Target>>>()?;
        if windows.is_empty() { None } else { Some(HideRecord { host_pid, host_start, at_ms, windows }) }
    }

    pub fn set_hidden_taskbar(&mut self, rec: &HideRecord) {
        let windows = rec
            .windows
            .iter()
            .map(|t| {
                Json::Obj(vec![
                    ("hwnd".to_string(), Json::Num((t.handle as i64).to_string())),
                    ("class".to_string(), Json::Str(t.class.clone())),
                    ("pid".to_string(), Json::num(u64::from(t.pid))),
                    ("start".to_string(), Json::num(t.start)),
                ])
            })
            .collect();
        let o = Json::Obj(vec![
            ("host_pid".to_string(), Json::num(u64::from(rec.host_pid))),
            ("host_start".to_string(), Json::num(rec.host_start)),
            ("at".to_string(), Json::num(rec.at_ms)),
            ("windows".to_string(), Json::Arr(windows)),
        ]);
        self.doc.set("hidden_taskbar", o);
    }

    pub fn clear_hidden_taskbar(&mut self) {
        self.doc.remove("hidden_taskbar");
    }

    /// The key is there, whether or not it can be understood.
    pub fn has_hidden_taskbar_key(&self) -> bool {
        self.doc.get("hidden_taskbar").is_some()
    }

    fn set_crashes(&mut self, crashes: &[Crash]) {
        let list = crashes
            .iter()
            .map(|c| {
                let mut o = vec![("t".to_string(), Json::num(c.at))];
                if c.uptime_ms != u64::MAX {
                    o.push(("up".to_string(), Json::num(c.uptime_ms)));
                }
                Json::Obj(o)
            })
            .collect();
        self.doc.set("crashes", Json::Arr(list));
    }

    /// Records what the machine asked to be written.
    pub fn apply(&mut self, p: &Persist) {
        match p {
            Persist::Crashes(c) => self.set_crashes(c),
            Persist::Ready { build, crashes } => {
                self.set_crashes(crashes);
                if let Some(b) = build {
                    self.doc.set("last_known_good_build", Json::Str(b.clone()));
                }
            }
            Persist::FallbackStart { reason, crashes } => {
                self.set_crashes(crashes);
                self.doc.set("fallback_reason", Json::Str(reason.as_str().into()));
                // Pessimistic until the value is really gone: a guard that dies in the middle is finished next start.
                self.doc.set("value_cleared", Json::Bool(!reason.deletes_value()));
            }
            Persist::FallbackEnd { reason, cleared, explorer_up } => {
                self.doc.set("fallback_reason", Json::Str(reason.as_str().into()));
                self.doc.set("value_cleared", Json::Bool(*cleared));
                self.doc.set("fallback_explorer_up", Json::Bool(*explorer_up));
                if reason.deletes_value() && *cleared {
                    // The switch is off now; a later enable starts with a clean log.
                    self.set_crashes(&[]);
                }
            }
        }
    }

    /// For the Settings switch: turning it on again forgets the last fall-back.
    pub fn clear_fallback(&mut self) {
        for k in ["fallback_reason", "value_cleared", "fallback_explorer_up"] {
            self.doc.remove(k);
        }
        self.set_crashes(&[]);
    }
}

/// The state file on disk, behind the `StateStore` trait.
pub struct FsStore {
    pub path: PathBuf,
}

impl crate::sys::StateStore for FsStore {
    fn load(&mut self) -> ShellStateFile {
        ShellStateFile::load(&self.path)
    }
    fn save(&mut self, file: &ShellStateFile) -> Result<(), String> {
        file.save(&self.path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-shellguard-test-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn cleanup(d: &Path) {
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn missing_file_gives_defaults_and_creates_nothing() {
        let d = temp("missing");
        let f = ShellStateFile::load(&path_in(&d));
        assert_eq!(f.status, LoadStatus::Missing);
        assert!(f.crashes().is_empty() && !f.pending_restore() && f.unresolved().is_none());
        assert!(!path_in(&d).exists());
        cleanup(&d);
    }

    #[test]
    fn round_trip_and_atomic_write_leaves_no_temp() {
        let d = temp("roundtrip");
        let p = path_in(&d);
        let mut f = ShellStateFile::empty();
        f.apply(&Persist::Ready { build: Some("26300.9974".into()), crashes: vec![Crash { at: 5, uptime_ms: 7 }, Crash { at: 9, uptime_ms: u64::MAX }] });
        f.set_pending_restore(true);
        f.save(&p).unwrap();
        let back = ShellStateFile::load(&p);
        assert_eq!(back.status, LoadStatus::Ok);
        assert_eq!(back.last_known_good_build(), Some("26300.9974"));
        assert_eq!(back.crashes(), vec![Crash { at: 5, uptime_ms: 7 }, Crash { at: 9, uptime_ms: u64::MAX }]);
        assert!(back.pending_restore());
        let names: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, vec![FILE_NAME.to_string()]);
        cleanup(&d);
    }

    #[test]
    fn unknown_keys_survive_a_rewrite() {
        let d = temp("unknown");
        let p = path_in(&d);
        std::fs::write(&p, r#"{"version":1,"mode":"shell","host_key":{"x":[1,2,3]},"original_work_areas":[{"monitor":"\\\\.\\DISPLAY1","rect":[0,0,1920,1040]}]}"#).unwrap();
        let mut f = ShellStateFile::load(&p);
        assert_eq!(f.original_work_areas(), vec![WorkAreaRec { monitor: "\\\\.\\DISPLAY1".into(), rect: [0, 0, 1920, 1040] }]);
        f.apply(&Persist::Crashes(vec![Crash { at: 1, uptime_ms: 2 }]));
        f.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        let doc = Json::parse(&text).unwrap();
        assert_eq!(doc.get("host_key").unwrap().get("x").unwrap().as_arr().unwrap().len(), 3);
        assert_eq!(doc.get("mode").unwrap().as_str(), Some("shell"));
        assert_eq!(ShellStateFile::peek(&p).crashes().len(), 1);
        cleanup(&d);
    }

    #[test]
    fn corrupt_files_are_moved_aside_and_replaced_by_defaults() {
        let d = temp("corrupt");
        let p = path_in(&d);
        let mut cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"{\"version\":1,\"crashes\":[".to_vec(),
            b"[1,2,3]".to_vec(),
            b"\"hello\"".to_vec(),
            vec![0xff, 0xfe, 0x00, 0x01],
            b"{\"version\":1}{\"version\":1}".to_vec(),
        ];
        cases.push(vec![b'x'; (MAX_BYTES + 1) as usize]);
        for (i, bytes) in cases.iter().enumerate() {
            std::fs::write(&p, bytes).unwrap();
            let f = ShellStateFile::load(&p);
            assert!(f.is_corrupt(), "case {i}: {:?}", f.status);
            assert!(f.crashes().is_empty() && f.unresolved().is_none());
            assert!(!p.exists(), "case {i}: the corrupt file must be moved aside");
            assert!(d.join("shell-state.json.corrupt").exists());
            // The next save writes a good file.
            f.save(&p).unwrap();
            assert_eq!(ShellStateFile::peek(&p).status, LoadStatus::Ok);
        }
        cleanup(&d);
    }

    #[test]
    fn peek_never_touches_a_corrupt_file() {
        let d = temp("peek");
        let p = path_in(&d);
        std::fs::write(&p, b"{{{").unwrap();
        let f = ShellStateFile::peek(&p);
        assert!(f.is_corrupt());
        assert!(p.exists() && !d.join("shell-state.json.corrupt").exists());
        cleanup(&d);
    }

    #[test]
    fn newer_version_is_read_but_never_rewritten() {
        let d = temp("newer");
        let p = path_in(&d);
        let original = r#"{"version":2,"pending_restore":true,"crashes":[{"t":10,"up":20}],"shiny":true}"#;
        std::fs::write(&p, original).unwrap();
        let mut f = ShellStateFile::load(&p);
        assert_eq!(f.status, LoadStatus::FromNewer(2));
        assert!(!f.writable);
        assert!(f.pending_restore());
        assert_eq!(f.crashes().len(), 1);
        f.apply(&Persist::Crashes(vec![]));
        assert!(f.save(&p).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), original);
        cleanup(&d);
    }

    #[test]
    fn wrong_types_of_single_keys_are_ignored_not_fatal() {
        let d = temp("types");
        let p = path_in(&d);
        std::fs::write(&p, r#"{"version":1,"crashes":"many","pending_restore":"yes","fallback_reason":7,"original_work_areas":[{"monitor":1},{"monitor":"a","rect":[1,2,3]},{"monitor":"b","rect":[1,2,3,4]}]}"#).unwrap();
        let f = ShellStateFile::load(&p);
        assert_eq!(f.status, LoadStatus::Ok);
        assert!(f.crashes().is_empty() && !f.pending_restore() && f.fallback_reason().is_none());
        assert_eq!(f.original_work_areas().len(), 1);
        cleanup(&d);
    }

    #[test]
    fn plain_number_crash_lists_of_the_plan_are_read() {
        let d = temp("plain");
        let p = path_in(&d);
        std::fs::write(&p, r#"{"version":1,"crashes":[100,200,"x",300]}"#).unwrap();
        let f = ShellStateFile::load(&p);
        assert_eq!(f.crashes().iter().map(|c| c.at).collect::<Vec<_>>(), vec![100, 200, 300]);
        assert!(f.crashes().iter().all(|c| c.uptime_ms == u64::MAX));
        cleanup(&d);
    }

    #[test]
    fn fallback_bookkeeping_and_unresolved_detection() {
        let mut f = ShellStateFile::empty();
        f.apply(&Persist::FallbackStart { reason: Reason::CrashLoop, crashes: vec![Crash { at: 1, uptime_ms: 1 }] });
        assert_eq!(f.unresolved(), Some(Reason::CrashLoop), "pessimistic until the value is gone");
        f.apply(&Persist::FallbackEnd { reason: Reason::CrashLoop, cleared: true, explorer_up: true });
        assert_eq!(f.unresolved(), None);
        assert!(f.crashes().is_empty(), "a cleared value resets the crash log");

        let mut g = ShellStateFile::empty();
        g.apply(&Persist::FallbackStart { reason: Reason::SafeMode, crashes: vec![] });
        assert_eq!(g.unresolved(), None, "safe mode leaves the value on purpose");

        let mut h = ShellStateFile::empty();
        h.apply(&Persist::FallbackStart { reason: Reason::Escape, crashes: vec![] });
        h.apply(&Persist::FallbackEnd { reason: Reason::Escape, cleared: false, explorer_up: true });
        assert_eq!(h.unresolved(), Some(Reason::Escape));
        h.clear_fallback();
        assert_eq!(h.unresolved(), None);
    }

    #[test]
    fn work_areas_round_trip_with_negative_origins() {
        let areas = vec![WorkAreaRec { monitor: "A".into(), rect: [0, 0, 1920, 1040] }, WorkAreaRec { monitor: "B".into(), rect: [-1080, -200, 0, 1720] }];
        let mut f = ShellStateFile::empty();
        f.set_original_work_areas(&areas);
        let d = temp("areas");
        f.save(&path_in(&d)).unwrap();
        assert_eq!(ShellStateFile::load(&path_in(&d)).original_work_areas(), areas);
        cleanup(&d);
    }

    #[test]
    fn the_host_pid_round_trips_and_is_removed() {
        let d = temp("hostpid");
        let p = path_in(&d);
        let mut f = ShellStateFile::empty();
        assert_eq!(f.host_pid(), None);
        f.set_host_pid(Some(4242));
        f.save(&p).unwrap();
        let mut back = ShellStateFile::load(&p);
        assert_eq!(back.host_pid(), Some(4242));
        back.set_host_pid(None);
        back.save(&p).unwrap();
        assert_eq!(ShellStateFile::peek(&p).host_pid(), None);
        // Nonsense is no pid: zero, negative, too large, a string.
        for text in [r#"{"version":1,"host_pid":0}"#, r#"{"version":1,"host_pid":-5}"#, r#"{"version":1,"host_pid":99999999999}"#, r#"{"version":1,"host_pid":"7"}"#] {
            std::fs::write(&p, text).unwrap();
            assert_eq!(ShellStateFile::peek(&p).host_pid(), None, "{text}");
        }
        cleanup(&d);
    }

    fn rec() -> HideRecord {
        HideRecord {
            host_pid: 4242,
            host_start: 133_700_000_000_000_000,
            at_ms: 1_731_000_000_000,
            windows: vec![
                Target { handle: 0x10_0A2E, class: "Shell_TrayWnd".into(), pid: 8800, start: 133_699_000_000_000_001 },
                // handles are sign-extended 32-bit values: a negative one must survive
                Target { handle: -65_538, class: "Shell_SecondaryTrayWnd".into(), pid: 8800, start: 133_699_000_000_000_001 },
            ],
        }
    }

    #[test]
    fn the_hidden_taskbar_record_round_trips_with_64_bit_times_and_negative_handles() {
        let d = temp("hidden");
        let p = path_in(&d);
        let mut f = ShellStateFile::empty();
        assert!(f.hidden_taskbar().is_none() && !f.has_hidden_taskbar_key());
        f.set_hidden_taskbar(&rec());
        f.set_pending_restore(true);
        f.save(&p).unwrap();
        let back = ShellStateFile::load(&p);
        assert_eq!(back.hidden_taskbar(), Some(rec()));
        assert!(back.pending_restore());
        let mut back = back;
        back.clear_hidden_taskbar();
        assert!(back.hidden_taskbar().is_none() && !back.has_hidden_taskbar_key());
        cleanup(&d);
    }

    #[test]
    fn a_garbled_hidden_taskbar_record_is_none_but_the_key_is_still_seen() {
        let d = temp("hidden-bad");
        let p = path_in(&d);
        for text in [
            r#"{"version":1,"hidden_taskbar":"yes"}"#,
            r#"{"version":1,"hidden_taskbar":{"host_pid":0,"windows":[{"hwnd":1,"class":"Shell_TrayWnd","pid":2}]}}"#,
            r#"{"version":1,"hidden_taskbar":{"host_pid":5,"windows":[]}}"#,
            r#"{"version":1,"hidden_taskbar":{"host_pid":5,"windows":[{"hwnd":"x","class":1,"pid":-2}]}}"#,
            r#"{"version":1,"hidden_taskbar":{"host_pid":5}}"#,
        ] {
            std::fs::write(&p, text).unwrap();
            let f = ShellStateFile::peek(&p);
            assert!(f.hidden_taskbar().is_none(), "{text}");
            assert!(f.has_hidden_taskbar_key(), "{text}");
        }
        // one good entry among bad ones is NOT enough: a record that is only partly understood is garbled as a whole, so that the
        // sweep shows every hidden window and none is orphaned by a trimmed record being deleted
        std::fs::write(&p, r#"{"version":1,"hidden_taskbar":{"host_pid":5,"windows":[{"hwnd":"x"},{"hwnd":7,"class":"Shell_TrayWnd","pid":9}]}}"#).unwrap();
        let f = ShellStateFile::peek(&p);
        assert!(f.hidden_taskbar().is_none() && f.has_hidden_taskbar_key(), "a partly unreadable record is garbled as a whole");
        // the host's start time may be missing from an otherwise good record
        std::fs::write(&p, r#"{"version":1,"hidden_taskbar":{"host_pid":5,"windows":[{"hwnd":7,"class":"Shell_TrayWnd","pid":9}]}}"#).unwrap();
        let r = ShellStateFile::peek(&p).hidden_taskbar().unwrap();
        assert_eq!((r.host_pid, r.host_start, r.windows.len(), r.windows[0].start), (5, 0, 1, 0));
        cleanup(&d);
    }

    #[test]
    fn the_hidden_taskbar_record_survives_the_guards_own_rewrites() {
        let d = temp("hidden-keep");
        let p = path_in(&d);
        let mut f = ShellStateFile::empty();
        f.set_hidden_taskbar(&rec());
        f.save(&p).unwrap();
        let mut g = ShellStateFile::load(&p);
        g.apply(&Persist::Crashes(vec![Crash { at: 1, uptime_ms: 2 }]));
        g.save(&p).unwrap();
        assert_eq!(ShellStateFile::peek(&p).hidden_taskbar(), Some(rec()));
        cleanup(&d);
    }

    #[test]
    fn save_creates_missing_folders() {
        let d = temp("mkdir");
        let p = d.join("a").join("b").join(FILE_NAME);
        ShellStateFile::empty().save(&p).unwrap();
        assert!(p.exists());
        cleanup(&d);
    }

    #[test]
    fn a_leftover_temp_file_is_harmless() {
        let d = temp("tmp");
        let p = path_in(&d);
        std::fs::write(d.join("shell-state.json.tmp"), b"half a wri").unwrap();
        assert_eq!(ShellStateFile::load(&p).status, LoadStatus::Missing);
        ShellStateFile::empty().save(&p).unwrap();
        assert_eq!(ShellStateFile::peek(&p).status, LoadStatus::Ok);
        cleanup(&d);
    }
}
