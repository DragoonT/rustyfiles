//! The folder organizer's hands: collecting file facts, and running a plan.
//!
//! Safety, in one place (the plan itself is made by `rf_core::organize`):
//! * nothing is ever deleted for good: the only way a file leaves is the Recycle Bin (through
//!   the shell, [`Recycler`]); there is no delete call in this file;
//! * nothing is ever overwritten: a move that meets an existing name is numbered;
//! * a file that is still being written is left alone: partial-download names, a file another
//!   program holds open for writing, and (in automatic runs) one that changed in the last 10 s;
//! * hidden and system files, links, folders and anything outside the watched folder are
//!   skipped again when the step runs, whatever the plan said;
//! * one failure never stops the rest; every failure is listed;
//! * everything that happened is in one [`Record`], so one undo puts it all back.
//!
//! Blocking: call from a worker thread.

use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rf_core::organize::safety::{ProtectedPaths, is_inside, is_partial_download, norm};
use rf_core::organize::{Dest, FileFact, FileKind, Step, numbered_name};
use rf_core::{Entry, attrs};
use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW, GetVolumeNameForVolumeMountPointW, MoveFileExW};
use windows::core::PCWSTR;

use crate::wide::{long_path, wide};

/// Marks a user-facing Turkish text (the app translates it; a test there checks every one has an
/// English pair).
fn tx(s: &'static str) -> &'static str {
    s
}

/// A file that changed more recently than this is not touched by an automatic run.
pub const SETTLE_MS: i64 = 10_000;
/// The most steps an automatic run does; the rest wait for the next run.
pub const AUTO_CAP: usize = 500;

/// Why a step was not done (not an error: the file stays as it is).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    Gone,
    NotAFile,
    Hidden,
    Partial,
    OutsideRoot,
    Link,
    InUse,
    /// Changed in the last [`SETTLE_MS`] (automatic runs only): tried again later.
    TooYoung,
    NoRecycleBin,
    /// The drive's Recycle Bin is switched off or would not take a file this big: the shell
    /// would ask whether to delete it for good, which a rule never does.
    BinRefuses,
    /// Not the file that was planned any more (a different size or time since the plan).
    Changed,
}

impl Why {
    pub fn text(self) -> &'static str {
        match self {
            Why::Gone => tx("Dosya artık yok"),
            Why::NotAFile => tx("Dosya değil"),
            Why::Hidden => tx("Gizli ya da sistem dosyası"),
            Why::Partial => tx("Henüz yazılıyor (kısmi indirme)"),
            Why::OutsideRoot => tx("İzlenen klasörün dışında"),
            Why::Link => tx("Bağlantı (sembolik) dosya"),
            Why::InUse => tx("Başka bir uygulama kullanıyor"),
            Why::TooYoung => tx("Az önce değişti: biraz beklenecek"),
            Why::NoRecycleBin => tx("Bu sürücüde Geri Dönüşüm Kutusu yok"),
            Why::BinRefuses => tx("Geri Dönüşüm Kutusu kapalı ya da bu dosya için fazla büyük"),
            Why::Changed => tx("Planlandıktan sonra değişti"),
        }
    }

    /// Worth trying again after a while.
    pub fn retry_later(self) -> bool {
        matches!(self, Why::InUse | Why::TooYoung | Why::Changed)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip {
    pub path: PathBuf,
    pub why: Why,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub path: PathBuf,
    pub message: String,
}

/// Everything one run did: the journal entry of the undo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Record {
    /// Tells this run from others in the undo stack.
    pub token: u64,
    /// (from, to) of every file moved or renamed, in order.
    pub moves: Vec<(PathBuf, PathBuf)>,
    /// (original path, path inside the Recycle Bin) of every file sent there.
    pub recycled: Vec<(PathBuf, PathBuf)>,
    /// Folders the run had to create, outermost first.
    pub created_dirs: Vec<PathBuf>,
}

impl Record {
    pub fn count(&self) -> usize {
        self.moves.len() + self.recycled.len()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub record: Record,
    pub failed: Vec<Failure>,
    pub skipped: Vec<Skip>,
    /// Steps left for the next run because of the cap.
    pub deferred_by_cap: usize,
    pub cancelled: bool,
}

impl Report {
    /// Files to look at again later (in use, changed a moment ago).
    pub fn retry_later(&self) -> Vec<PathBuf> {
        self.skipped.iter().filter(|s| s.why.retry_later()).map(|s| s.path.clone()).collect()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct RunOptions {
    /// A run started by the folder watcher (not by the user's click): files that changed a
    /// moment ago wait.
    pub automatic: bool,
    /// At most this many steps; the rest are counted in [`Report::deferred_by_cap`].
    pub cap: usize,
    /// Now, Unix milliseconds (passed in so tests can say).
    pub now_ms: i64,
}

impl RunOptions {
    pub fn manual(now_ms: i64) -> Self {
        Self { automatic: false, cap: usize::MAX, now_ms }
    }
    pub fn automatic(now_ms: i64) -> Self {
        Self { automatic: true, cap: AUTO_CAP, now_ms }
    }
}

/// Sends files to the Recycle Bin, and only there.
pub trait Recycler {
    /// Can this file go to a Recycle Bin? (Removable and network drives have none: the shell
    /// would delete for good there, so those files are left alone.)
    fn can_recycle(&self, path: &Path) -> bool;
    /// Will the drive's bin take this very file? (It can be switched off for the drive, or the
    /// file can be bigger than the bin: the shell would then ask whether to delete for good.)
    fn bin_takes(&self, _path: &Path) -> bool {
        true
    }
    /// Recycles the files; returns (original, path in the bin) for each that went.
    fn recycle(&self, paths: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>>;
}

/// The shell's Recycle Bin, with the progress of a Status Center job.
pub struct ShellRecycler {
    pub tracker: Arc<crate::fileops::Tracker>,
}

impl Recycler for ShellRecycler {
    fn can_recycle(&self, path: &Path) -> bool {
        on_fixed_drive(path)
    }

    fn bin_takes(&self, path: &Path) -> bool {
        bin_accepts(bin_settings(path), std::fs::metadata(verbatim(path)).map_or(0, |m| m.len()))
    }

    fn recycle(&self, paths: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
        let before = self.tracker.results.lock().unwrap().len();
        // `false`: to the Recycle Bin, never permanently
        crate::fileops::delete_tracked(paths, false, self.tracker.clone())?;
        let results = self.tracker.results.lock().unwrap();
        Ok(results[before.min(results.len())..].to_vec())
    }
}

fn on_fixed_drive(path: &Path) -> bool {
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return false;
    }
    let root = wide(format!("{}:\\", b[0] as char));
    // DRIVE_FIXED
    unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) == 3 }
}

/// What a drive's Recycle Bin is set to take.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BinSettings {
    /// "Don't move files to the Recycle Bin. Remove files immediately when deleted."
    pub disabled: bool,
    /// The bin's maximum size in bytes, when the drive has a custom one.
    pub max_bytes: Option<u64>,
    /// The size of the drive (0 = unknown).
    pub drive_bytes: u64,
}

/// Would the bin take a file of this size? When the drive has no custom maximum the shell's
/// default is about a tenth of the drive; a bigger file is not recycled either, to be safe.
pub fn bin_accepts(s: BinSettings, size: u64) -> bool {
    if s.disabled {
        return false;
    }
    match (s.max_bytes, s.drive_bytes) {
        (Some(max), _) => size <= max,
        (None, 0) => true,
        (None, drive) => size <= drive / 10,
    }
}

/// Reads the Recycle Bin settings of the drive `path` is on (read only: nothing is written).
pub fn bin_settings(path: &Path) -> BinSettings {
    use windows_registry::{CURRENT_USER, LOCAL_MACHINE};
    let mut out = BinSettings::default();
    let s = path.to_string_lossy();
    let s = s.strip_prefix(r"\\?\").unwrap_or(&s);
    let b = s.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() {
        return out;
    }
    let root = wide(format!("{}:\\", b[0] as char));
    let (mut free, mut total) = (0u64, 0u64);
    unsafe {
        let _ = GetDiskFreeSpaceExW(PCWSTR(root.as_ptr()), Some(&mut free), Some(&mut total), None);
    }
    out.drive_bytes = total;
    // the group policy "Do not move deleted files to the Recycle Bin"
    const POLICY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    let policy_on = |k: windows_registry::Key| k.get_u32("NoRecycleFiles").is_ok_and(|v| v != 0);
    if CURRENT_USER.open(POLICY).is_ok_and(policy_on) || LOCAL_MACHINE.open(POLICY).is_ok_and(policy_on) {
        out.disabled = true;
        return out;
    }
    // the per-drive settings live under the volume's GUID
    let mut name = [0u16; 64];
    if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(root.as_ptr()), &mut name) }.is_ok() {
        let n = String::from_utf16_lossy(&name[..name.iter().position(|&c| c == 0).unwrap_or(name.len())]);
        if let Some(guid) = n.split_once('{').and_then(|(_, r)| r.split_once('}')).map(|(g, _)| g) {
            let key = format!(r"Software\Microsoft\Windows\CurrentVersion\Explorer\BitBucket\Volume\{{{guid}}}");
            if let Ok(k) = CURRENT_USER.open(&key) {
                out.disabled = k.get_u32("NukeOnDelete").is_ok_and(|v| v != 0);
                out.max_bytes = k.get_u32("MaxCapacity").ok().map(|mb| mb as u64 * 1024 * 1024);
            }
        }
    }
    out
}

/// The path in the `\\?\` form: names with a trailing dot or space, and long paths, mean what
/// they say there.
fn verbatim(p: &Path) -> PathBuf {
    let mut w = long_path(p);
    w.pop(); // NUL
    PathBuf::from(OsString::from_wide(&w))
}

const ERROR_NOT_SAME_DEVICE: i32 = 17;

/// `std::fs::rename` replaces an existing target; this fails instead (and moves across drives).
pub fn move_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    let (a, b) = (long_path(from), long_path(to));
    // (no flags: MoveFileEx never replaces without MOVEFILE_REPLACE_EXISTING, and without
    // MOVEFILE_COPY_ALLOWED it does not copy to another drive; that is done below, carefully)
    match unsafe { MoveFileExW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), Default::default()) } {
        Ok(()) => Ok(()),
        Err(e) => {
            let code = e.code().0 & 0xFFFF;
            if code == ERROR_NOT_SAME_DEVICE { copy_then_remove(from, to) } else { Err(std::io::Error::from_raw_os_error(code)) }
        }
    }
}

static COPY_TOKENS: AtomicU64 = AtomicU64::new(1);

/// A move to another drive. The shell's own copy-and-delete writes straight to the final name, so
/// a crash leaves a half file there and a failed delete of the source leaves a second copy. This
/// copies to a name nobody else uses (a `.part` file, which every organizer pass ignores), renames it
/// into place without replacing anything, and only then removes the source; if that last step is
/// refused the copy is taken away again and the file stays where it was.
pub(crate) fn copy_then_remove(from: &Path, to: &Path) -> std::io::Result<()> {
    if exists(to) {
        return Err(std::io::ErrorKind::AlreadyExists.into());
    }
    let dir = to.parent().unwrap_or(Path::new(""));
    let tmp = dir.join(format!(".rf-moving-{}-{}.part", std::process::id(), COPY_TOKENS.fetch_add(1, Ordering::Relaxed)));
    if let Err(e) = std::fs::copy(verbatim(from), verbatim(&tmp)) {
        remove_own_file(&tmp);
        return Err(e);
    }
    // (the same drive now: a plain rename that fails if the name is taken)
    if let Err(e) = move_no_replace(&tmp, to) {
        remove_own_file(&tmp);
        return Err(e);
    }
    if let Err(e) = remove_source(from) {
        remove_own_file(to);
        return Err(e);
    }
    Ok(())
}

/// Removes the source of a move once its copy is in place. The one call that takes a user's file
/// away: only ever after the same bytes are at their new name.
fn remove_source(from: &Path) -> std::io::Result<()> {
    std::fs::remove_file(verbatim(from))
}

/// Removes a file this module made itself a moment ago (a half copy, a copy that has to be taken
/// back).
fn remove_own_file(p: &Path) {
    let _ = std::fs::remove_file(verbatim(p));
}

/// Removes a folder, but only while it is empty (it fails otherwise, which is wanted): used for
/// the folders a run made itself.
fn remove_empty_dir(p: &Path) {
    let _ = std::fs::remove_dir(verbatim(p));
}

/// A real link (symbolic link or junction): something that points somewhere else. The bare
/// "reparse point" attribute is not that: OneDrive and other cloud-sync placeholders carry it
/// too, hydrated or not, and are ordinary files. Rust asks for the reparse tag and counts only
/// the name-surrogate ones (symlinks, junctions) as links.
fn is_link_meta(m: &std::fs::Metadata) -> bool {
    m.file_type().is_symlink()
}

/// Does this listing entry need a second look to be called a link (only reparse points do)?
fn entry_is_link(dir: &Path, e: &Entry) -> bool {
    e.is_symlink() && std::fs::symlink_metadata(verbatim(&dir.join(&*e.name))).map_or(true, |m| is_link_meta(&m))
}

/// Opens the file for reading while letting others only read: this fails when another program
/// has it open for writing, i.e. while it is still being written.
fn free_to_take(path: &Path, a: u32) -> bool {
    // an online-only cloud file: opening it would download it
    if a & (attrs::OFFLINE | attrs::RECALL_ON_OPEN | attrs::RECALL_ON_DATA_ACCESS) != 0 {
        return true;
    }
    // FILE_SHARE_READ
    std::fs::OpenOptions::new().read(true).share_mode(1).open(verbatim(path)).is_ok()
}

fn time_ms(t: std::io::Result<std::time::SystemTime>) -> i64 {
    t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as i64)
}

/// The checks every step passes just before it runs (the plan may be minutes old).
fn inspect(step: &Step, opts: &RunOptions) -> Result<(), Why> {
    use std::os::windows::fs::MetadataExt;
    let src = &step.source;
    let meta = std::fs::symlink_metadata(verbatim(src)).map_err(|_| Why::Gone)?;
    let a = meta.file_attributes();
    if is_link_meta(&meta) {
        return Err(Why::Link);
    }
    if meta.is_dir() || a & attrs::DIRECTORY != 0 {
        return Err(Why::NotAFile);
    }
    if a & (attrs::HIDDEN | attrs::SYSTEM) != 0 {
        return Err(Why::Hidden);
    }
    let name = src.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if is_partial_download(&name) {
        return Err(Why::Partial);
    }
    let parent = src.parent().map(norm).unwrap_or_default();
    let root = norm(&step.root);
    if !(parent == root || (step.subfolders && is_inside(&parent, &root))) {
        return Err(Why::OutsideRoot);
    }
    // a hidden, system or linked folder between the watched folder and the file (.git, .stversions ...)
    // puts the file out of bounds, as it does in a scan
    if let Some(why) = bad_ancestor(src, &root) {
        return Err(why);
    }
    // not the file that was planned any more
    if step.modified_ms > 0 && (meta.len() != step.size || (time_ms(meta.modified()) - step.modified_ms).abs() > 1000) {
        return Err(Why::Changed);
    }
    if !free_to_take(src, a) {
        return Err(Why::InUse);
    }
    if opts.automatic {
        let newest = time_ms(meta.modified()).max(time_ms(meta.created()));
        if opts.now_ms - newest < SETTLE_MS {
            return Err(Why::TooYoung);
        }
    }
    Ok(())
}

/// The folders from the file's own up to (not including) the watched folder: any that is hidden,
/// a system folder or a link makes the file off limits.
fn bad_ancestor(src: &Path, root_norm: &str) -> Option<Why> {
    use std::os::windows::fs::MetadataExt;
    let mut cur = src.parent();
    while let Some(dir) = cur {
        let n = norm(dir);
        if n == root_norm || !is_inside(&n, root_norm) {
            break;
        }
        match std::fs::symlink_metadata(verbatim(dir)) {
            Ok(m) => {
                if is_link_meta(&m) {
                    return Some(Why::Link);
                }
                if m.file_attributes() & (attrs::HIDDEN | attrs::SYSTEM) != 0 {
                    return Some(Why::Hidden);
                }
            }
            Err(_) => return Some(Why::Gone),
        }
        cur = dir.parent();
    }
    None
}

/// Creates `dir` and its missing parents; the ones that were missing go into `created`.
fn ensure_dir(dir: &Path, created: &mut Vec<PathBuf>) -> std::io::Result<()> {
    let mut missing = Vec::new();
    let mut cur = dir;
    loop {
        match std::fs::symlink_metadata(verbatim(cur)) {
            Ok(m) => {
                // a file where a folder is needed
                if !m.is_dir() && missing.is_empty() {
                    return Err(std::io::Error::other(tx("Hedef bir klasör değil, aynı adlı bir dosya var")));
                }
                break;
            }
            // (any other error, access denied say, is not "missing": making the folder would only fail worse)
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            Err(_) => {
                missing.push(cur.to_path_buf());
                match cur.parent() {
                    Some(p) if !p.as_os_str().is_empty() => cur = p,
                    _ => break,
                }
            }
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(verbatim(dir))?;
    created.extend(missing.into_iter().rev());
    Ok(())
}

static TOKENS: AtomicU64 = AtomicU64::new(1);

/// Runs the steps. `progress(done, total, name)` is called before each one; `cancel` is looked
/// at between steps.
pub fn run(steps: &[Step], opts: &RunOptions, recycler: &dyn Recycler, progress: &dyn Fn(usize, usize, &str), cancel: &AtomicBool) -> Report {
    let mut rep = Report { record: Record { token: TOKENS.fetch_add(1, Ordering::Relaxed), ..Record::default() }, ..Report::default() };
    let take = steps.len().min(opts.cap);
    rep.deferred_by_cap = steps.len() - take;
    let mut bin: Vec<&Step> = Vec::new();
    for (i, st) in steps[..take].iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            rep.cancelled = true;
            break;
        }
        let name = st.source.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        progress(i, take, &name);
        if let Err(why) = inspect(st, opts) {
            rep.skipped.push(Skip { path: st.source.clone(), why });
            continue;
        }
        match &st.dest {
            Dest::Recycle => {
                if !recycler.can_recycle(&st.source) {
                    rep.skipped.push(Skip { path: st.source.clone(), why: Why::NoRecycleBin });
                } else if !recycler.bin_takes(&st.source) {
                    rep.skipped.push(Skip { path: st.source.clone(), why: Why::BinRefuses });
                } else {
                    bin.push(st);
                }
            }
            Dest::Move(dest) => match do_move(&st.source, dest, &mut rep.record.created_dirs) {
                Ok(done) => rep.record.moves.push((st.source.clone(), done)),
                Err(e) => rep.failed.push(Failure { path: st.source.clone(), message: e }),
            },
        }
    }
    // The bin goes last, in one go, which can be long after the first check (cross-drive copies in
    // between): every file is looked at once more right before it is sent.
    let mut checked: Vec<&Step> = Vec::new();
    if !rep.cancelled {
        for st in bin {
            match inspect(st, opts) {
                Ok(()) => checked.push(st),
                Err(why) => rep.skipped.push(Skip { path: st.source.clone(), why }),
            }
        }
    }
    let bin = checked;
    if !bin.is_empty() && !rep.cancelled {
        let paths: Vec<PathBuf> = bin.iter().map(|s| s.source.clone()).collect();
        progress(take, take, "");
        match recycler.recycle(&paths) {
            Ok(pairs) => {
                for p in &paths {
                    if !pairs.iter().any(|(orig, _)| norm(orig) == norm(p)) {
                        rep.failed.push(Failure { path: p.clone(), message: tx("Geri Dönüşüm Kutusu'na gönderilemedi").to_string() });
                    }
                }
                rep.record.recycled = pairs;
            }
            Err(e) => {
                for p in paths {
                    rep.failed.push(Failure { path: p, message: format!("{e:#}") });
                }
            }
        }
    }
    progress(take, take, "");
    rep
}

/// Moves one file; a name that turns out to be taken (someone else was faster than the plan)
/// gets a number. Returns the path it ended up at.
fn do_move(src: &Path, dest: &Path, created: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    let made_before = created.len();
    let r = move_into(src, dest, created);
    if r.is_err() {
        // folders made for a move that did not happen would only be left behind (and no undo entry
        // exists for a run that moved nothing): take them away again, innermost first
        for d in created.drain(made_before..).rev() {
            remove_empty_dir(&d);
        }
    }
    r
}

fn move_into(src: &Path, dest: &Path, created: &mut Vec<PathBuf>) -> Result<PathBuf, String> {
    if let Some(dir) = dest.parent() {
        ensure_dir(dir, created).map_err(|e| format!("{}: {e}", tx("Hedef klasör oluşturulamadı")))?;
    }
    let dir = dest.parent().map(Path::to_path_buf).unwrap_or_default();
    let name = dest.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut target = dest.to_path_buf();
    for n in 2..=60u32 {
        match move_no_replace(src, &target) {
            Ok(()) => return Ok(target),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => target = dir.join(numbered_name(&name, n)),
            Err(e) => return Err(e.to_string()),
        }
    }
    Err(tx("Hedefte aynı ad bulundu").to_string())
}

#[derive(Debug, Clone, Default)]
pub struct UndoReport {
    pub restored: usize,
    pub failed: Vec<Failure>,
}

/// Puts the moved files back (newest first) and removes the folders the run made, when they are
/// empty again. A file that is gone, or whose old place has been taken, is left and listed.
/// The Recycle Bin part of the record is for the caller (the shell restores it).
pub fn undo_moves(rec: &Record) -> UndoReport {
    let mut out = UndoReport::default();
    for (from, to) in rec.moves.iter().rev() {
        let fail = |out: &mut UndoReport, m: &'static str| out.failed.push(Failure { path: to.clone(), message: m.to_string() });
        if std::fs::symlink_metadata(verbatim(to)).is_err() {
            fail(&mut out, tx("Dosya artık yok"));
            continue;
        }
        if std::fs::symlink_metadata(verbatim(from)).is_ok() {
            fail(&mut out, tx("Eski yeri başka bir dosya tutuyor"));
            continue;
        }
        if let Some(dir) = from.parent() {
            let _ = std::fs::create_dir_all(verbatim(dir));
        }
        match move_no_replace(to, from) {
            Ok(()) => out.restored += 1,
            Err(e) => out.failed.push(Failure { path: to.clone(), message: e.to_string() }),
        }
    }
    for d in rec.created_dirs.iter().rev() {
        // (fails while anything is inside: that is wanted)
        remove_empty_dir(d);
    }
    out
}

/// Does the moves of the record again (after an undo).
pub fn redo_moves(rec: &Record) -> UndoReport {
    let mut out = UndoReport::default();
    let mut created = Vec::new();
    for (from, to) in &rec.moves {
        if std::fs::symlink_metadata(verbatim(from)).is_err() || std::fs::symlink_metadata(verbatim(to)).is_ok() {
            out.failed.push(Failure { path: from.clone(), message: tx("Dosya taşınamadı").to_string() });
            continue;
        }
        let r = match to.parent() {
            Some(dir) => ensure_dir(dir, &mut created).and_then(|_| move_no_replace(from, to)),
            None => move_no_replace(from, to),
        };
        match r {
            Ok(()) => out.restored += 1,
            Err(e) => out.failed.push(Failure { path: from.clone(), message: e.to_string() }),
        }
    }
    out
}

fn fact_of_entry(dir: &Path, e: &Entry, classify: fn(&str) -> FileKind) -> FileFact {
    let path = dir.join(&*e.name);
    let mut f = FileFact::new(path.clone(), e.size, e.modified_ms, e.created_ms, classify(&e.ext));
    f.hidden = e.is_hidden() || e.is_system();
    if &*e.ext == "lnk" {
        f.link_target = crate::shortcut::resolve(&path).ok().map(|i| i.target).filter(|t| !t.as_os_str().is_empty());
    }
    f
}

/// Every file of `root` (and of the folders below it, with `subfolders`) as facts. Links to
/// folders are not entered; hidden and system folders and the app's own folders are skipped.
/// `cancelled` is polled between batches.
pub fn scan(root: &Path, subfolders: bool, classify: fn(&str) -> FileKind, protected: &ProtectedPaths, cancelled: &dyn Fn() -> bool) -> Vec<FileFact> {
    let mut facts = Vec::new();
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        if cancelled() {
            break;
        }
        let mut subdirs = Vec::new();
        let r = crate::list_dir(&dir, 4096, cancelled, &mut |batch| {
            for e in batch {
                if e.is_dir() {
                    if subfolders && depth < 24 && !e.is_hidden() && !e.is_system() && !entry_is_link(&dir, &e) {
                        subdirs.push(dir.join(&*e.name));
                    }
                } else if !entry_is_link(&dir, &e) {
                    facts.push(fact_of_entry(&dir, &e, classify));
                }
            }
        });
        if let Err(e) = r {
            log::debug!("organize scan {}: {e}", dir.display());
        }
        for d in subdirs {
            if !protected.is_app_file(&d) {
                stack.push((d, depth + 1));
            }
        }
    }
    facts
}

/// Facts for some names of one folder (what a folder watcher reported): one `stat` each, names
/// that are gone or are folders drop out. The cost does not depend on the folder's size.
pub fn facts_for_names(dir: &Path, names: &[String], classify: fn(&str) -> FileKind) -> Vec<FileFact> {
    let mut out = Vec::new();
    for n in names {
        if let Ok(Some(e)) = crate::stat_entry(dir, n)
            && !e.is_dir()
            && !entry_is_link(dir, &e)
        {
            out.push(fact_of_entry(dir, &e, classify));
        }
    }
    out
}

/// The same for full paths (the watcher of a folder with its subfolders).
pub fn facts_for_paths(paths: &[PathBuf], classify: fn(&str) -> FileKind) -> Vec<FileFact> {
    let mut out = Vec::new();
    for p in paths {
        let (Some(dir), Some(name)) = (p.parent(), p.file_name()) else { continue };
        out.extend(facts_for_names(dir, &[name.to_string_lossy().into_owned()], classify));
    }
    out
}

/// Can the folder be opened to list its files? `None`: yes. Otherwise the reason, from the system
/// (a folder that was renamed, a drive that is not plugged in, no access): a run over such a folder
/// must not pass for "nothing matches".
pub fn folder_problem(root: &Path) -> Option<String> {
    std::fs::read_dir(verbatim(root)).err().map(|e| e.to_string())
}

/// Does something exist at `p` (a file, a folder, a broken link)?
pub fn exists(p: &Path) -> bool {
    std::fs::symlink_metadata(verbatim(p)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::organize::{Action, Condition, Join, NameOp, PlanContext, Rule, plan};
    use std::cell::RefCell;

    fn classify(ext: &str) -> FileKind {
        match ext {
            "jpg" | "png" => FileKind::Image,
            "txt" | "pdf" => FileKind::Document,
            _ => FileKind::Other,
        }
    }

    struct Dir(PathBuf);
    impl Dir {
        fn new(name: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("rf-organize-{}-{name}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn p(&self, rel: &str) -> PathBuf {
            self.0.join(rel)
        }
        fn write(&self, rel: &str, body: &str) -> PathBuf {
            let p = self.p(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
            p
        }
        /// Backdated so an automatic run does not call it new.
        fn old(&self, rel: &str, body: &str) -> PathBuf {
            let p = self.write(rel, body);
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            let t = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            // (the creation time counts too: a file made a moment ago is new whatever its modified time says)
            use std::os::windows::fs::FileTimesExt;
            f.set_times(std::fs::FileTimes::new().set_modified(t).set_accessed(t).set_created(t)).unwrap();
            p
        }
    }
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(verbatim(&self.0));
        }
    }

    /// A bin that is a folder inside the test's own folder: no real Recycle Bin is touched.
    struct FakeBin {
        dir: PathBuf,
        sent: RefCell<Vec<PathBuf>>,
        fail: bool,
    }
    impl FakeBin {
        fn new(dir: &Dir) -> Self {
            FakeBin { dir: dir.p("$bin"), sent: RefCell::new(Vec::new()), fail: false }
        }
    }
    impl Recycler for FakeBin {
        fn can_recycle(&self, _: &Path) -> bool {
            true
        }
        fn recycle(&self, paths: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
            anyhow::ensure!(!self.fail, "no bin");
            std::fs::create_dir_all(&self.dir)?;
            let mut out = Vec::new();
            for (i, p) in paths.iter().enumerate() {
                let to = self.dir.join(format!("{i}-{}", p.file_name().unwrap().to_string_lossy()));
                std::fs::rename(p, &to)?;
                self.sent.borrow_mut().push(p.clone());
                out.push((p.clone(), to));
            }
            Ok(out)
        }
    }

    fn now() -> i64 {
        time_ms(Ok(std::time::SystemTime::now()))
    }

    fn rule(dir: &Path, name: &str, conditions: Vec<Condition>, action: Action) -> Rule {
        Rule { id: 1, name: name.into(), folder: dir.to_path_buf(), conditions, action, ..Rule::default() }
    }

    fn ext(list: &[&str]) -> Condition {
        Condition::Extension { list: list.iter().map(|s| s.to_string()).collect() }
    }

    fn make_plan(rules: &[Rule], root: &Path, subfolders: bool) -> rf_core::organize::Plan {
        let protected = ProtectedPaths::default();
        let facts = scan(root, subfolders, classify, &protected, &|| false);
        let ex = |p: &Path| exists(p);
        let ctx = PlanContext::new(now(), classify, &ex, &protected);
        plan(rules, &facts, &ctx)
    }

    fn go(steps: &[Step], opts: &RunOptions, bin: &dyn Recycler) -> Report {
        run(steps, opts, bin, &|_, _, _| {}, &AtomicBool::new(false))
    }

    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }

    #[test]
    fn files_move_and_the_destination_folders_are_made() {
        let d = Dir::new("move");
        d.write("a.jpg", "A");
        d.write("b.JPG", "B");
        d.write("c.txt", "C");
        let r = rule(&d.p(""), "resimler", vec![ext(&["jpg"])], Action::MoveToTemplate { template: format!(r"{}\Resimler\{{kind}}\{{ext}}", d.p("hedef").display()) });
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps.len(), 2, "{p:?}");
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert!(rep.failed.is_empty() && rep.skipped.is_empty(), "{rep:?}");
        assert_eq!(rep.record.moves.len(), 2);
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\Resimler\Images\jpg\a.jpg")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\Resimler\Images\JPG\b.JPG")).unwrap(), "B");
        assert_eq!(names(&d.0), ["c.txt", "hedef"]);
        // the folders it made, outermost first ("JPG" is the same folder as "jpg" on NTFS)
        let made: Vec<String> = rep.record.created_dirs.iter().map(|p| p.strip_prefix(&d.0).unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(made, ["hedef", r"hedef\Resimler", r"hedef\Resimler\Images", r"hedef\Resimler\Images\jpg"]);
    }

    #[test]
    fn a_taken_name_is_numbered_never_overwritten() {
        let d = Dir::new("clash");
        d.write("a.txt", "yeni");
        d.write(r"hedef\a.txt", "eski");
        d.write(r"hedef\a (2).txt", "eski2");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves, [(d.p("a.txt"), d.p(r"hedef\a (3).txt"))]);
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\a.txt")).unwrap(), "eski");
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\a (2).txt")).unwrap(), "eski2");
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\a (3).txt")).unwrap(), "yeni");
    }

    #[test]
    fn a_name_taken_after_the_plan_is_numbered_when_the_step_runs() {
        let d = Dir::new("late-clash");
        d.write("a.txt", "yeni");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps[0].dest, Dest::Move(d.p(r"hedef\a.txt")));
        // somebody else puts a file there before "Uygula"
        d.write(r"hedef\a.txt", "baska");
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\a.txt")).unwrap(), "baska");
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\a (2).txt")).unwrap(), "yeni");
        assert_eq!(rep.record.moves[0].1, d.p(r"hedef\a (2).txt"));
    }

    #[test]
    fn clashes_inside_one_plan_are_numbered() {
        let d = Dir::new("plan-clash");
        d.write(r"x\a.txt", "1");
        d.write(r"y\a.txt", "2");
        d.write(r"z\a.txt", "3");
        let mut r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: PathBuf::from("hedef") });
        r.subfolders = true;
        // (the destination is inside the watched folder and subfolders are on: a loop, so no steps)
        let p = make_plan(std::slice::from_ref(&r), &d.0, true);
        assert!(p.steps.is_empty() && p.loops.len() == 1);
        // from outside the folder it is fine
        let out = Dir::new("plan-clash-out");
        r.action = Action::MoveTo { folder: out.p("hedef") };
        let p = make_plan(&[r], &d.0, true);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert!(rep.failed.is_empty(), "{rep:?}");
        assert_eq!(names(&out.p("hedef")), ["a (2).txt", "a (3).txt", "a.txt"]);
    }

    #[test]
    fn what_must_not_be_touched_is_skipped_when_the_step_runs() {
        let d = Dir::new("skip");
        let hidden = d.write("gizli.txt", "x");
        let sys = d.write("sistem.txt", "x");
        let part = d.write("film.mp4.crdownload", "x");
        let ok = d.write("iyi.txt", "x");
        let locked = d.write("kilitli.txt", "x");
        let folder = d.p("klasor.txt");
        std::fs::create_dir_all(&folder).unwrap();
        let set = |p: &Path, a: u32| {
            use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, SetFileAttributesW};
            let w = wide(p);
            unsafe { SetFileAttributesW(PCWSTR(w.as_ptr()), FILE_FLAGS_AND_ATTRIBUTES(a)).unwrap() };
        };
        set(&hidden, attrs::HIDDEN);
        set(&sys, attrs::SYSTEM);
        // a writer holds this one open, as a browser would
        let _writer = std::fs::OpenOptions::new().write(true).share_mode(1 | 2 | 4).open(&locked).unwrap();
        let step = |p: &PathBuf| Step {
            source: p.clone(),
            dest: Dest::Move(d.p("hedef").join(p.file_name().unwrap())),
            rule_id: 1,
            rule_name: "r".into(),
            root: d.0.clone(),
            subfolders: false,
            size: 0,
            modified_ms: 0,
        };
        let outside = Dir::new("skip-outside");
        let stray = outside.write("disarida.txt", "x");
        let steps: Vec<Step> = [&hidden, &sys, &part, &ok, &locked, &folder, &stray, &d.p("yok.txt")].iter().map(|p| step(p)).collect();
        let rep = go(&steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        let why: Vec<(String, Why)> = rep.skipped.iter().map(|s| (s.path.file_name().unwrap().to_string_lossy().into_owned(), s.why)).collect();
        assert_eq!(
            why,
            [
                ("gizli.txt".to_string(), Why::Hidden),
                ("sistem.txt".to_string(), Why::Hidden),
                ("film.mp4.crdownload".to_string(), Why::Partial),
                ("kilitli.txt".to_string(), Why::InUse),
                ("klasor.txt".to_string(), Why::NotAFile),
                ("disarida.txt".to_string(), Why::OutsideRoot),
                ("yok.txt".to_string(), Why::Gone),
            ]
        );
        assert_eq!(rep.record.moves.len(), 1);
        assert!(!ok.exists() && d.p(r"hedef\iyi.txt").exists());
        assert!(hidden.exists() && sys.exists() && part.exists() && locked.exists() && stray.exists());
        assert!(rep.failed.is_empty());
    }

    #[test]
    fn an_automatic_run_waits_for_files_that_just_changed() {
        let d = Dir::new("young");
        let fresh = d.write("yeni.txt", "x");
        let old = d.old("eski.txt", "x");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps.len(), 2);
        let rep = go(&p.steps, &RunOptions::automatic(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), 1);
        assert!(!old.exists() && fresh.exists());
        assert_eq!(rep.skipped, [Skip { path: fresh.clone(), why: Why::TooYoung }]);
        assert_eq!(rep.retry_later(), [fresh.clone()]);
        // a run the user asked for does not wait
        let p = make_plan(&[rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") })], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), 1);
        // and once ten seconds have passed, the automatic one takes it too
        let d2 = Dir::new("young2");
        let fresh = d2.write("yeni.txt", "x");
        let r = rule(&d2.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d2.p("hedef") });
        let p = make_plan(&[r], &d2.0, false);
        let rep = go(&p.steps, &RunOptions::automatic(now() + 11_000), &FakeBin::new(&d2));
        assert_eq!(rep.record.moves.len(), 1);
        assert!(!fresh.exists());
    }

    #[test]
    fn at_most_the_cap_per_run_the_rest_waits() {
        let d = Dir::new("cap");
        for i in 0..520 {
            d.old(&format!("f{i:03}.txt"), "x");
        }
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(std::slice::from_ref(&r), &d.0, false);
        assert_eq!(p.steps.len(), 520);
        let rep = go(&p.steps, &RunOptions::automatic(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), AUTO_CAP);
        assert_eq!(rep.deferred_by_cap, 20);
        assert_eq!(std::fs::read_dir(d.p("hedef")).unwrap().count(), 500);
        // the next run plans the 20 that are left
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps.len(), 20);
        let rep = go(&p.steps, &RunOptions::automatic(now()), &FakeBin::new(&d));
        assert_eq!((rep.record.moves.len(), rep.deferred_by_cap), (20, 0));
        // a user's own run has no cap
        let d2 = Dir::new("cap2");
        for i in 0..520 {
            d2.write(&format!("f{i:03}.txt"), "x");
        }
        let r = rule(&d2.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d2.p("hedef") });
        let p = make_plan(&[r], &d2.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d2));
        assert_eq!(rep.record.moves.len(), 520);
    }

    #[test]
    fn one_failure_does_not_stop_the_others() {
        let d = Dir::new("fail");
        d.write("a.txt", "1");
        d.write("b.txt", "2");
        d.write("c.txt", "3");
        // a FILE where b's destination folder would have to be
        let r = rule(
            &d.p(""),
            "r",
            vec![ext(&["txt"])],
            Action::MoveToTemplate { template: format!(r"{}\{{name}}", d.p("hedef").display()) },
        );
        d.write("hedef", "ben bir dosyayım");
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps.len(), 3);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.failed.len(), 3, "the folder cannot be made for any of them");
        assert!(rep.failed.iter().all(|f| !f.message.is_empty()));
        // now only one of them fails
        std::fs::remove_file(d.p("hedef")).unwrap();
        d.write(r"hedef\b", "ben de bir dosyayım");
        let r = rule(
            &d.p(""),
            "r",
            vec![ext(&["txt"])],
            Action::MoveToTemplate { template: format!(r"{}\{{name}}", d.p("hedef").display()) },
        );
        let p = make_plan(&[r], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.failed.len(), 1);
        assert_eq!(rep.failed[0].path, d.p("b.txt"));
        assert_eq!(rep.record.moves.len(), 2);
        assert!(d.p("b.txt").exists() && !d.p("a.txt").exists() && !d.p("c.txt").exists());
    }

    #[test]
    fn recycling_goes_through_the_bin_only() {
        let d = Dir::new("bin");
        d.write("a.tmpx", "1");
        d.write("b.tmpx", "2");
        d.write("c.txt", "3");
        let r = rule(&d.p(""), "çöp", vec![ext(&["tmpx"])], Action::Recycle);
        let p = make_plan(&[r], &d.0, false);
        let bin = FakeBin::new(&d);
        let rep = go(&p.steps, &RunOptions::manual(now()), &bin);
        assert_eq!(rep.record.recycled.len(), 2);
        assert_eq!(bin.sent.borrow().len(), 2);
        assert!(!d.p("a.tmpx").exists() && !d.p("b.tmpx").exists() && d.p("c.txt").exists());
        // the bin says no: the files stay, the failures are listed, nothing is deleted
        let d2 = Dir::new("bin2");
        d2.write("a.tmpx", "1");
        let r = rule(&d2.p(""), "çöp", vec![ext(&["tmpx"])], Action::Recycle);
        let p = make_plan(&[r], &d2.0, false);
        let mut bin = FakeBin::new(&d2);
        bin.fail = true;
        let rep = go(&p.steps, &RunOptions::manual(now()), &bin);
        assert_eq!(rep.failed.len(), 1);
        assert!(d2.p("a.tmpx").exists());
        // a drive without a bin
        struct NoBin;
        impl Recycler for NoBin {
            fn can_recycle(&self, _: &Path) -> bool {
                false
            }
            fn recycle(&self, _: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
                panic!("must not be asked")
            }
        }
        let rep = go(&p.steps, &RunOptions::manual(now()), &NoBin);
        assert_eq!(rep.skipped, [Skip { path: d2.p("a.tmpx"), why: Why::NoRecycleBin }]);
        assert!(d2.p("a.tmpx").exists());
    }

    #[test]
    fn this_file_cannot_delete_for_good() {
        // The runner's only way to remove a file is the Recycler. Nothing above the tests may
        // call a delete: look at the source.
        let src = include_str!("organize.rs");
        let code = src.split("#[cfg(test)]").next().unwrap();
        for forbidden in ["remove_dir_all", "fastdelete", "DeleteFile", "delete_permanent", "delete_tracked(paths, true", "permanent: true"] {
            assert!(!code.contains(forbidden), "found {forbidden}");
        }
        // remove_dir only for the folders the run itself made, and only while empty
        assert_eq!(code.matches("remove_dir(").count(), 1);
        // remove_file: the source of a move to another drive once its copy is in place, and a copy
        // this module made itself (each in a function of its own)
        assert_eq!(code.matches("remove_file(").count(), 2);
        assert!(code.contains("fn remove_source(") && code.contains("fn remove_own_file("));
        assert!(code.contains("delete_tracked(paths, false"));
    }

    #[test]
    fn one_undo_puts_the_whole_run_back() {
        let d = Dir::new("undo");
        d.write("a.jpg", "A");
        d.write("b.jpg", "B");
        d.write("c.pdf", "C");
        d.write("d.tmpx", "D");
        let rules = [
            rule(&d.p(""), "resimler", vec![ext(&["jpg"])], Action::MoveToTemplate { template: format!(r"{}\Arşiv\{{ext}}", d.p("hedef").display()) }),
            Rule { id: 2, ..rule(&d.p(""), "pdf", vec![Condition::Name { op: NameOp::StartsWith, text: "c".into() }], Action::Rename { template: "yeni {name}.{ext}".into() }) },
            Rule { id: 3, ..rule(&d.p(""), "çöp", vec![ext(&["tmpx"])], Action::Recycle) },
        ];
        let before = names(&d.0);
        let p = make_plan(&rules, &d.0, false);
        let bin = FakeBin::new(&d);
        let rep = go(&p.steps, &RunOptions::manual(now()), &bin);
        assert_eq!((rep.record.moves.len(), rep.record.recycled.len()), (3, 1), "{rep:?}");
        assert!(rep.failed.is_empty());
        assert_eq!(std::fs::read_to_string(d.p("yeni c.pdf")).unwrap(), "C");
        // the moves go back and the folders the run made go away
        let u = undo_moves(&rep.record);
        assert_eq!((u.restored, u.failed.len()), (3, 0), "{u:?}");
        assert_eq!(std::fs::read_to_string(d.p("a.jpg")).unwrap(), "A");
        assert_eq!(std::fs::read_to_string(d.p("b.jpg")).unwrap(), "B");
        assert_eq!(std::fs::read_to_string(d.p("c.pdf")).unwrap(), "C");
        assert!(!d.p("hedef").exists(), "the folders the run made are removed");
        // (the bin part is the shell's to restore: here it is still in the fake bin)
        assert_eq!(rep.record.recycled.len(), 1);
        assert!(rep.record.recycled[0].1.exists());
        let mut now_names = names(&d.0);
        now_names.retain(|n| n != "$bin");
        let mut want = before.clone();
        want.retain(|n| n != "d.tmpx");
        assert_eq!(now_names, want);
        // and again (redo)
        let r = redo_moves(&rep.record);
        assert_eq!((r.restored, r.failed.len()), (3, 0), "{r:?}");
        assert!(d.p(r"hedef\Arşiv\jpg\a.jpg").exists() && d.p("yeni c.pdf").exists());
    }

    #[test]
    fn undo_leaves_what_it_cannot_put_back_and_says_so() {
        let d = Dir::new("undo-conflict");
        d.write("a.txt", "A");
        d.write("b.txt", "B");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), 2);
        // a new a.txt took the old place; b.txt was deleted by the user meanwhile
        d.write("a.txt", "YENİ");
        std::fs::remove_file(d.p(r"hedef\b.txt")).unwrap();
        let u = undo_moves(&rep.record);
        assert_eq!(u.restored, 0);
        assert_eq!(u.failed.len(), 2);
        assert_eq!(std::fs::read_to_string(d.p("a.txt")).unwrap(), "YENİ");
        assert!(d.p(r"hedef\a.txt").exists(), "kept, not lost");
        // the folder is not empty, so it stays
        assert!(d.p("hedef").exists());
    }

    #[test]
    fn scan_lists_files_below_when_asked_and_skips_hidden_folders() {
        let d = Dir::new("scan");
        d.write("a.txt", "1");
        d.write(r"alt\b.txt", "2");
        d.write(r"alt\derin\c.txt", "3");
        d.write(r"gizli\d.txt", "4");
        let w = wide(d.p("gizli"));
        unsafe {
            windows::Win32::Storage::FileSystem::SetFileAttributesW(PCWSTR(w.as_ptr()), windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attrs::HIDDEN)).unwrap();
        }
        let protected = ProtectedPaths::default();
        let flat = scan(&d.0, false, classify, &protected, &|| false);
        assert_eq!(flat.len(), 1);
        let mut deep: Vec<String> = scan(&d.0, true, classify, &protected, &|| false).iter().map(|f| f.name.clone()).collect();
        deep.sort();
        assert_eq!(deep, ["a.txt", "b.txt", "c.txt"]);
        // only the named files are looked at
        let some = facts_for_names(&d.0, &["a.txt".to_string(), "yok.txt".to_string(), "alt".to_string()], classify);
        assert_eq!(some.len(), 1);
        assert_eq!(some[0].name, "a.txt");
        assert_eq!(some[0].size, 1);
        assert!(some[0].modified_ms > 0 && some[0].created_ms > 0);
        let by_path = facts_for_paths(&[d.p(r"alt\derin\c.txt")], classify);
        assert_eq!(by_path.len(), 1);
        // an app folder is not entered
        let mut protected = ProtectedPaths::default();
        protected.app_dirs.push(d.p("alt"));
        let mut deep: Vec<String> = scan(&d.0, true, classify, &protected, &|| false).iter().map(|f| f.name.clone()).collect();
        deep.sort();
        assert_eq!(deep, ["a.txt"]);
    }

    fn hold_open_without_delete_share(p: &Path) -> std::fs::File {
        // like an indexer or a virus scanner: reading, and nobody may rename or delete it meanwhile
        std::fs::OpenOptions::new().read(true).share_mode(1).open(p).unwrap()
    }

    #[test]
    fn a_failed_move_leaves_no_empty_folders_behind() {
        let d = Dir::new("fail-dirs");
        let a = d.write("a.txt", "1");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveToTemplate { template: format!(r"{}\yeni\{{ext}}", d.p("hedef").display()) });
        let p = make_plan(&[r], &d.0, false);
        let _holder = hold_open_without_delete_share(&a);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.failed.len(), 1, "{rep:?}");
        assert!(a.exists());
        assert!(!d.p("hedef").exists(), "the folders made for the move that failed are gone again");
        assert!(rep.record.created_dirs.is_empty());
    }

    #[test]
    fn a_hidden_or_linked_folder_on_the_way_puts_a_file_out_of_bounds() {
        let d = Dir::new("ancestors");
        let inner = d.write(r"gizli\derin\x.txt", "x");
        let plain = d.write(r"acik\y.txt", "y");
        let w = wide(d.p("gizli"));
        unsafe {
            windows::Win32::Storage::FileSystem::SetFileAttributesW(PCWSTR(w.as_ptr()), windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES(attrs::HIDDEN)).unwrap();
        }
        // a junction to somewhere else (needs no privilege)
        let elsewhere = Dir::new("ancestors-elsewhere");
        let through = elsewhere.write("z.txt", "z");
        let junction = d.p("kisayol");
        let ok = std::process::Command::new("cmd").args(["/C", "mklink", "/J"]).arg(&junction).arg(&elsewhere.0).stdout(std::process::Stdio::null()).status().is_ok_and(|s| s.success());
        assert!(ok, "mklink /J (a junction needs no privilege) failed");
        let step = |p: &Path| Step {
            source: p.to_path_buf(),
            dest: Dest::Move(d.p("hedef").join(p.file_name().unwrap())),
            rule_id: 1,
            rule_name: "r".into(),
            root: d.0.clone(),
            subfolders: true,
            size: 0,
            modified_ms: 0,
        };
        let steps = vec![step(&inner), step(&plain), step(&junction.join("z.txt"))];
        let rep = go(&steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), 1, "{rep:?}");
        assert!(rep.skipped.contains(&Skip { path: inner.clone(), why: Why::Hidden }));
        assert!(inner.exists());
        assert!(rep.skipped.contains(&Skip { path: junction.join("z.txt"), why: Why::Link }));
        assert!(through.exists());
        // a scan does not go through the junction either
        let seen: Vec<String> = scan(&d.0, true, classify, &ProtectedPaths::default(), &|| false).iter().map(|f| f.name.clone()).collect();
        assert!(!seen.contains(&"z.txt".to_string()), "{seen:?}");
        // (remove the junction itself before the folders go: removing the tree must not reach through it)
        std::fs::remove_dir(&junction).unwrap();
    }

    #[test]
    fn a_reparse_point_that_is_not_a_link_is_an_ordinary_file() {
        // Cloud-sync placeholders (OneDrive) have the reparse-point attribute but are not links: only
        // symlinks and junctions are. The decision is the reparse tag's, not the attribute's.
        let d = Dir::new("plain");
        let f = d.write("a.txt", "1");
        let m = std::fs::symlink_metadata(&f).unwrap();
        assert!(!is_link_meta(&m));
        let e = Entry::new("a.txt", rf_core::entry::EntryKind::File, 1, 0, 0, attrs::REPARSE_POINT);
        // the listing says "reparse point", the second look says "not a link": the file is kept
        assert!(e.is_symlink());
        assert!(!entry_is_link(&d.0, &e));
        // an entry that cannot be looked at again is left out (the safe way round)
        let gone = Entry::new("yok.txt", rf_core::entry::EntryKind::File, 1, 0, 0, attrs::REPARSE_POINT);
        assert!(entry_is_link(&d.0, &gone));
        // no reparse attribute: no second look needed
        let plain = Entry::new("a.txt", rf_core::entry::EntryKind::File, 1, 0, 0, 0);
        assert!(!entry_is_link(&d.0, &plain));
    }

    #[test]
    fn a_file_that_is_not_the_planned_one_any_more_stays() {
        let d = Dir::new("changed");
        let a = d.old("a.txt", "kisa");
        let b = d.old("b.txt", "kisa");
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        assert_eq!(p.steps.len(), 2);
        assert!(p.steps.iter().all(|s| s.modified_ms > 0 && s.size == 4));
        // a preview stays open; meanwhile a is overwritten with something else
        std::fs::write(&a, "cok daha uzun bir icerik").unwrap();
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.skipped, [Skip { path: a.clone(), why: Why::Changed }]);
        assert_eq!(rep.record.moves.len(), 1);
        assert!(a.exists() && !b.exists());
        // the recycle batch is looked at again right before it is sent
        let d2 = Dir::new("changed-bin");
        let c = d2.write("c.tmpx", "1");
        let r = rule(&d2.p(""), "çöp", vec![ext(&["tmpx"])], Action::Recycle);
        let p = make_plan(&[r], &d2.0, false);
        std::fs::write(&c, "iki baytlik degil").unwrap();
        let bin = FakeBin::new(&d2);
        let rep = go(&p.steps, &RunOptions::manual(now()), &bin);
        assert_eq!(rep.skipped, [Skip { path: c.clone(), why: Why::Changed }]);
        assert!(bin.sent.borrow().is_empty() && c.exists());
    }

    #[test]
    fn the_bin_must_be_able_to_take_the_file_or_it_stays() {
        let on = BinSettings { disabled: false, max_bytes: Some(100 << 20), drive_bytes: 1 << 40 };
        assert!(bin_accepts(on, 100 << 20));
        assert!(!bin_accepts(on, (100 << 20) + 1), "bigger than the bin");
        assert!(!bin_accepts(BinSettings { disabled: true, ..on }, 1), "the bin is off for this drive");
        // no custom size: a tenth of the drive is the most that is trusted
        let dflt = BinSettings { disabled: false, max_bytes: None, drive_bytes: 1000 };
        assert!(bin_accepts(dflt, 100) && !bin_accepts(dflt, 101));
        assert!(bin_accepts(BinSettings::default(), u64::MAX), "nothing is known: the drive check decides alone");
        // a file the recycler says no to is skipped with its own reason and not handed to the shell
        struct Refuses;
        impl Recycler for Refuses {
            fn can_recycle(&self, _: &Path) -> bool {
                true
            }
            fn bin_takes(&self, _: &Path) -> bool {
                false
            }
            fn recycle(&self, _: &[PathBuf]) -> anyhow::Result<Vec<(PathBuf, PathBuf)>> {
                panic!("must not be asked")
            }
        }
        let d = Dir::new("bin-refuses");
        d.write("a.tmpx", "1");
        let r = rule(&d.p(""), "çöp", vec![ext(&["tmpx"])], Action::Recycle);
        let p = make_plan(&[r], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &Refuses);
        assert_eq!(rep.skipped, [Skip { path: d.p("a.tmpx"), why: Why::BinRefuses }]);
        assert!(d.p("a.tmpx").exists());
        // reading the real settings of the temp folder's drive works (read only) and knows the drive's size
        let s = bin_settings(&d.0);
        assert!(s.drive_bytes > 0, "{s:?}");
    }

    #[test]
    fn a_move_to_another_drive_never_leaves_a_half_or_a_second_copy() {
        // (the copy path is the same whatever the drives: it is called directly here, on one drive)
        let d = Dir::new("xcopy");
        let a = d.write("a.txt", "icerik");
        let target = d.p(r"hedef\a.txt");
        std::fs::create_dir_all(d.p("hedef")).unwrap();
        copy_then_remove(&a, &target).unwrap();
        assert!(!a.exists());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "icerik");
        assert_eq!(names(&d.p("hedef")), ["a.txt"], "no temporary file is left");
        // a name that is taken: nothing is overwritten, nothing is left over
        let b = d.write("b.txt", "yeni");
        d.write(r"hedef\b.txt", "eski");
        let e = copy_then_remove(&b, &d.p(r"hedef\b.txt")).unwrap_err();
        assert_eq!(e.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(std::fs::read_to_string(d.p(r"hedef\b.txt")).unwrap(), "eski");
        assert!(b.exists());
        // the source cannot be removed (somebody holds it without delete sharing): the copy is taken
        // back, the file stays where it was, and there is no duplicate
        let c = d.write("c.txt", "uc");
        let _holder = hold_open_without_delete_share(&c);
        assert!(copy_then_remove(&c, &d.p(r"hedef\c.txt")).is_err());
        assert!(c.exists());
        assert_eq!(names(&d.p("hedef")), ["a.txt", "b.txt"]);
    }

    #[test]
    fn a_folder_that_cannot_be_listed_says_why() {
        let d = Dir::new("problem");
        assert_eq!(folder_problem(&d.0), None);
        assert!(folder_problem(&d.p("yok")).is_some());
        // a file is not a folder either
        let f = d.write("a.txt", "x");
        assert!(folder_problem(&f).is_some());
    }

    #[test]
    fn a_cancelled_run_stops_between_steps() {
        let d = Dir::new("cancel");
        for i in 0..5 {
            d.write(&format!("f{i}.txt"), "x");
        }
        let r = rule(&d.p(""), "r", vec![ext(&["txt"])], Action::MoveTo { folder: d.p("hedef") });
        let p = make_plan(&[r], &d.0, false);
        let cancel = AtomicBool::new(false);
        let n = std::cell::Cell::new(0);
        let rep = run(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d), &|_, _, _| {
            n.set(n.get() + 1);
            if n.get() == 3 {
                cancel.store(true, Ordering::Relaxed);
            }
        }, &cancel);
        assert!(rep.cancelled);
        // (asked to stop while the third step was announced: that one is still done, the next not)
        assert_eq!(rep.record.moves.len(), 3);
        // what was done can still be undone
        assert_eq!(undo_moves(&rep.record).restored, 3);
    }

    #[test]
    fn names_with_a_trailing_dot_or_space_are_found_where_they_are() {
        let d = Dir::new("dots");
        // only a \\?\ path can make such a name; Windows' normal path rules would cut it
        let weird = verbatim(&d.p("garip. "));
        std::fs::write(&weird, "x").unwrap();
        let facts = scan(&d.0, false, classify, &ProtectedPaths::default(), &|| false);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].name, "garip. ");
        let r = rule(&d.p(""), "r", vec![], Action::MoveTo { folder: d.p("hedef") });
        let ex = |p: &Path| exists(p);
        let protected = ProtectedPaths::default();
        let ctx = PlanContext::new(now(), classify, &ex, &protected);
        let p = plan(&[r], &facts, &ctx);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert!(rep.failed.is_empty() && rep.skipped.is_empty(), "{rep:?}");
        assert!(exists(&d.p("hedef").join("garip. ")));
        assert!(!exists(&d.p("garip. ")));
    }

    #[test]
    fn the_plan_and_the_runner_agree_on_rules_with_any() {
        let d = Dir::new("any");
        d.write("a.jpg", "1");
        d.write("rapor-final.docx", "2");
        d.write("c.bin", "3");
        let mut r = rule(&d.p(""), "r", vec![ext(&["jpg"]), Condition::Name { op: NameOp::StartsWith, text: "RAPOR".into() }], Action::MoveTo { folder: d.p("hedef") });
        r.join = Join::Any;
        let p = make_plan(&[r], &d.0, false);
        let rep = go(&p.steps, &RunOptions::manual(now()), &FakeBin::new(&d));
        assert_eq!(rep.record.moves.len(), 2);
        assert_eq!(names(&d.0), ["c.bin", "hedef"]);
    }
}
