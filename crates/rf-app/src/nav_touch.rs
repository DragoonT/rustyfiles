//! Touching a path from the UI thread.
//!
//! The rule of the app: the UI thread never waits for a disk, the shell or the network. A path that is slow on purpose is
//! `\\wsl.localhost\<distro>`: the first touch of it makes Windows start the distribution's virtual machine and its file server,
//! which takes one to many seconds; an offline share is the same with the SMB timeout, and a disk that has gone to sleep or a
//! phone does the same for a local-looking path. Issue 16 (the window freezes when a WSL distribution is clicked in the sidebar)
//! was a set of calls on the click path that touch the path: `Path::is_dir`, asked by `archive_view::split` while a folder view
//! started its watcher (measured: 2262 ms for a UNC host that does not answer), and the localized-name lookup
//! (`rf_win::localized::folder_label`: `GetFileAttributesW` and a read of `desktop.ini`) that the breadcrumb and the tab title made
//! for every folder of the path while drawing.
//!
//! What lives here:
//! * [`Disk`]: the two questions the archive code asks of the disk, so that a test can put a slow fake under them;
//! * [`archive_split`]: whether a path lies inside an archive file. It asks the disk only when a NAME in the path looks like an
//!   archive, so a folder on a slow place is never touched to find out it is not one;
//! * [`may_hold_archive`]: the same by name alone (no disk at all);
//! * [`label_paths`]: the folders whose localized names a breadcrumb and a tab title ask for, so that a worker can read them before
//!   the drawing needs them (the drawing looks in the cache only: see `rf_win::localized::label_if_known`).

use std::path::{Path, PathBuf};

/// What the archive code asks of the disk. The real one is [`RealDisk`].
pub trait Disk {
    fn is_dir(&self, p: &Path) -> bool;
    fn is_file(&self, p: &Path) -> bool;
}

pub struct RealDisk;

impl Disk for RealDisk {
    fn is_dir(&self, p: &Path) -> bool {
        p.is_dir()
    }
    fn is_file(&self, p: &Path) -> bool {
        p.is_file()
    }
}

/// Splits a path into (archive file, path inside it) when some ancestor is an archive file; a plain folder, or a path outside any
/// archive, gives `None`.
///
/// The names are read first and cost nothing; the disk is asked (a folder first, then each archive-named ancestor) only for a path
/// that has such a name. Before issue 16 the first question was always "is this a folder?", which on `\\wsl.localhost\<distro>`
/// held the UI thread until the distribution had started.
pub fn archive_split(disk: &dyn Disk, path: &Path) -> Option<(PathBuf, PathBuf)> {
    if !may_hold_archive(path) {
        return None;
    }
    if disk.is_dir(path) {
        return None;
    }
    let a = archive_named(path).find(|a| disk.is_file(a))?;
    let inner = path.strip_prefix(a).ok()?.to_path_buf();
    Some((a.to_path_buf(), inner))
}

/// The ancestors of `path` (itself first) whose name is that of an archive file. A drive root or anything of three characters or
/// fewer ends the walk. Reads the first bytes of a `.001` file (`is_browsable`): call it only for a path that [`may_hold_archive`],
/// and not on the UI thread.
fn archive_named(path: &Path) -> impl Iterator<Item = &Path> {
    path.ancestors().take_while(|a| a.as_os_str().len() > 3).filter(|a| crate::archive_view::is_browsable(a))
}

/// Whether some name in `path` MAY be that of an archive file: only then can the path lead into one. Looks at no disk at all: a
/// name ending in `.001` counts as "maybe", because only the first bytes of that file could tell and the UI thread does not read
/// them. A caller treats a folder that is merely named like an archive as a plain folder (a change in it is listed again) or goes
/// on to the exact check ([`archive_split`]) off the UI thread.
pub fn may_hold_archive(path: &Path) -> bool {
    path.ancestors().take_while(|a| a.as_os_str().len() > 3).any(crate::archive_view::may_be_archive_name)
}

/// The folders of `path` whose localized names the breadcrumb and the tab title ask for: the prefixes the breadcrumb builds
/// (`C:`, `C:\Users`, ...) and the path with all its ancestors. A worker passes them to `rf_win::localized::warm`.
pub fn label_paths(path: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let mut acc = PathBuf::new();
    for comp in path.components() {
        acc.push(comp.as_os_str());
        if !matches!(comp, std::path::Component::RootDir) {
            out.push(acc.clone());
        }
    }
    for a in path.ancestors() {
        if !a.as_os_str().is_empty() && !out.iter().any(|o| o == a) {
            out.push(a.to_path_buf());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A disk that takes `delay` for every question, like the first touch of a WSL distribution, and counts the questions.
    struct SlowDisk {
        delay: Duration,
        asked: AtomicUsize,
        dirs: HashSet<PathBuf>,
        files: HashSet<PathBuf>,
    }

    impl SlowDisk {
        fn new(delay_ms: u64) -> Self {
            Self { delay: Duration::from_millis(delay_ms), asked: AtomicUsize::new(0), dirs: HashSet::new(), files: HashSet::new() }
        }
        fn with_file(mut self, p: &str) -> Self {
            self.files.insert(PathBuf::from(p));
            self
        }
        fn with_dir(mut self, p: &str) -> Self {
            self.dirs.insert(PathBuf::from(p));
            self
        }
        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
        fn touch(&self) {
            self.asked.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(self.delay);
        }
    }

    impl Disk for SlowDisk {
        fn is_dir(&self, p: &Path) -> bool {
            self.touch();
            self.dirs.contains(p)
        }
        fn is_file(&self, p: &Path) -> bool {
            self.touch();
            self.files.contains(p)
        }
    }

    #[test]
    fn a_slow_place_is_not_touched_to_learn_it_is_no_archive() {
        for p in [
            r"\\wsl.localhost\Ubuntu",
            r"\\wsl.localhost\Ubuntu\home\me\src",
            r"\\wsl$\docker-desktop\mnt",
            r"\\nas\share\photos\2024",
            r"C:\Users\me\Documents",
            r"D:\",
        ] {
            let disk = SlowDisk::new(150);
            let began = Instant::now();
            let got = archive_split(&disk, Path::new(p));
            assert_eq!(got, None, "{p}");
            assert_eq!(disk.asked(), 0, "{p}: the disk was asked");
            assert!(began.elapsed() < Duration::from_millis(100), "{p}: took {:?}", began.elapsed());
        }
    }

    #[test]
    fn a_path_into_an_archive_still_splits() {
        let disk = SlowDisk::new(0).with_file(r"C:\data\x.zip");
        let got = archive_split(&disk, Path::new(r"C:\data\x.zip\in\a.txt"));
        assert_eq!(got, Some((PathBuf::from(r"C:\data\x.zip"), PathBuf::from(r"in\a.txt"))));
        // the archive itself splits as its own root
        let got = archive_split(&disk, Path::new(r"C:\data\x.zip"));
        assert_eq!(got, Some((PathBuf::from(r"C:\data\x.zip"), PathBuf::new())));
    }

    #[test]
    fn a_folder_that_is_named_like_an_archive_is_a_folder() {
        let disk = SlowDisk::new(0).with_dir(r"C:\data\x.zip");
        assert_eq!(archive_split(&disk, Path::new(r"C:\data\x.zip")), None);
        // named like one and neither there nor a file: nothing to split
        let disk = SlowDisk::new(0);
        assert_eq!(archive_split(&disk, Path::new(r"C:\data\y.7z\in")), None);
    }

    #[test]
    fn the_names_alone_tell_what_may_hold_an_archive() {
        assert!(!may_hold_archive(Path::new(r"\\wsl.localhost\Ubuntu\home\me")));
        assert!(!may_hold_archive(Path::new(r"C:\")));
        assert!(may_hold_archive(Path::new(r"C:\data\x.zip")));
        assert!(may_hold_archive(Path::new(r"\\wsl.localhost\Ubuntu\home\me\x.tar.gz\inner")));
        // a split set, by its name or (a bare `.001`) as a maybe
        assert!(may_hold_archive(Path::new(r"C:\data\x.7z.001")));
        assert!(may_hold_archive(Path::new(r"C:\data\backup.001\sub")));
    }

    /// The answers did not change with the order of the questions: the same table through a copy of the first version.
    #[test]
    fn the_answers_are_those_of_the_first_version() {
        fn first_version(disk: &dyn Disk, path: &Path) -> Option<(PathBuf, PathBuf)> {
            if disk.is_dir(path) {
                return None;
            }
            for a in path.ancestors() {
                if a.as_os_str().len() <= 3 {
                    break;
                }
                if crate::archive_view::is_browsable(a) && disk.is_file(a) {
                    let inner = path.strip_prefix(a).ok()?.to_path_buf();
                    return Some((a.to_path_buf(), inner));
                }
            }
            None
        }
        let disk = SlowDisk::new(0)
            .with_file(r"C:\a\one.zip")
            .with_file(r"C:\a\two.7z")
            .with_dir(r"C:\a\three.zip")
            .with_dir(r"C:\a\three.zip\deep")
            .with_file(r"C:\a\two.7z\inner.zip")
            .with_file(r"C:\a\plain.txt")
            .with_dir(r"C:\a\plain");
        for p in [
            r"C:\a\one.zip",
            r"C:\a\one.zip\x\y",
            r"C:\a\two.7z\inner.zip\z",
            r"C:\a\two.7z",
            r"C:\a\three.zip",
            r"C:\a\three.zip\deep",
            r"C:\a\three.zip\deep\file.iso",
            r"C:\a\plain.txt",
            r"C:\a\plain",
            r"C:\a\missing",
            r"C:\a\missing.rar\q",
            r"C:\",
            r"C:",
            r"\\nas\share\x.zip\a",
            r"",
        ] {
            let p = Path::new(p);
            assert_eq!(archive_split(&disk, p), first_version(&disk, p), "{p:?}");
        }
    }

    /// A file called `x.001` that is locked against being opened: the first bytes of it cannot be read. What the UI thread asks
    /// (name only) must not depend on that: it did, while `may_hold_archive` and the tab icon went through `is_browsable`, which
    /// opens a `.001` file to see what it is. Reading the file on a WSL path is the first touch of it.
    #[test]
    fn a_split_volume_name_is_never_opened_by_the_name_checks() {
        use std::os::windows::fs::OpenOptionsExt;
        let d = std::env::temp_dir().join(format!("rf-nav-vol-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let bare = d.join("img.001");
        let named = d.join("set.7z.001");
        for f in [&bare, &named] {
            std::fs::write(f, b"PK\x03\x04 and some more bytes").unwrap();
        }
        // open with no sharing: any other open of the file now fails
        let _hold: Vec<std::fs::File> =
            [&bare, &named].iter().map(|f| std::fs::OpenOptions::new().read(true).share_mode(0).open(f).unwrap()).collect();
        assert!(std::fs::File::open(&bare).is_err(), "the lock does not hold: the test would prove nothing");
        // what the exact check says depends on reading the file; the name checks say what they say without it
        assert!(!crate::archive_view::is_browsable(&bare));
        assert!(may_hold_archive(&bare));
        assert!(may_hold_archive(&bare.join("inside")));
        assert!(crate::archive_view::is_browsable_path_by_name(&named));
        assert!(crate::archive_view::is_browsable_path_by_name(&named.join("inside")));
        assert!(!crate::archive_view::is_browsable_path_by_name(&bare));
        drop(_hold);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_folders_a_breadcrumb_asks_for() {
        let got = label_paths(Path::new(r"C:\Users\demo"));
        assert_eq!(got, vec![PathBuf::from("C:"), PathBuf::from(r"C:\Users"), PathBuf::from(r"C:\Users\demo"), PathBuf::from(r"C:\")]);
        let got = label_paths(Path::new(r"\\wsl.localhost\Ubuntu\home"));
        assert!(got.contains(&PathBuf::from(r"\\wsl.localhost\Ubuntu\home")));
        assert_eq!(label_paths(Path::new("")), Vec::<PathBuf>::new());
    }

    /// The tokens by which the text of a function shows that it asks the disk (or the shell) by itself: the plain calls, and the
    /// helpers that are known to do it (`archive_view::split` stats the path; `folder_label` reads attributes and `desktop.ini`;
    /// `is_browsable` opens a `.001` file). The functions of a click are scanned for them below.
    const FORBIDDEN: [&str; 20] = [
        ".is_dir()",
        ".is_file()",
        ".exists()",
        ".metadata()",
        "fs::metadata(",
        "symlink_metadata(",
        "canonicalize(",
        "read_dir(",
        "fs::read(",
        "File::open(",
        "GetFileAttributes",
        "GetDiskFreeSpace",
        "GetVolumeInformation",
        "GetDriveType",
        "archive_view::split(",
        "archive_view::split_inside(",
        "archive_view::is_browsable(",
        "archive_view::is_browsable_path(",
        "folder_label(",
        "is_dir_bounded(",
    ];

    fn offenders(body: &str) -> Vec<&'static str> {
        FORBIDDEN.iter().copied().filter(|bad| body.contains(bad)).collect()
    }

    /// The text of the function that starts at `head`: up to the line that closes it at the same indentation (the source is read
    /// with its line ends made `\n`: the files may be checked out with `\r\n`).
    fn body_of(src: &str, head: &str) -> String {
        let src = src.replace("\r\n", "\n");
        let i = src.find(head).unwrap_or_else(|| panic!("{head:?} is gone: update this test"));
        let line_start = src[..i].rfind('\n').map_or(0, |n| n + 1);
        let line = &src[line_start..];
        let indent = line.len() - line.trim_start().len();
        let close = format!("\n{}}}\n", " ".repeat(indent));
        let end = src[i..].find(&close).map_or(src.len(), |e| i + e + close.len());
        assert!(end < src.len(), "{head:?}: the end of the function was not found");
        src[i..end].to_string()
    }

    /// The detector itself: the text of the two calls that froze the window (the old `start_watch` line and the old breadcrumb
    /// line, from the version before the fix) is found, so the check below is not an empty one.
    #[test]
    fn the_detector_finds_the_calls_that_froze_the_window() {
        let old_start_watch = "let patchable = crate::archive_view::split(&path).is_none();";
        let old_breadcrumb = "let label = match rf_win::localized::folder_label(&acc, None) {";
        let old_title = "if let Some(l) = rf_win::localized::folder_label(p, None) {";
        let old_icon = "Location::Path(p) if crate::archive_view::is_browsable_path(p) => L::FileArchive,";
        for old in [old_start_watch, old_breadcrumb, old_title, old_icon] {
            assert!(!offenders(old).is_empty(), "{old}");
        }
        assert!(offenders("let patchable = !crate::nav_touch::may_hold_archive(&path);").is_empty());
        assert!(offenders("match rf_win::localized::label_if_known(&acc) {").is_empty());
    }

    /// The functions a click on a place walks through before the first frame of the new folder, and the ones that draw that
    /// frame, are read as text: none of them may ask the disk by itself, nor call a helper that does (the listing, the git status,
    /// the watcher's handle and the localized names are made on other threads, and come back as an update).
    #[test]
    fn the_functions_of_a_click_do_not_touch_the_disk() {
        let folder_view = include_str!("folder_view.rs");
        let workspace = include_str!("workspace.rs");
        let heads: [(&str, &str, &str); 14] = [
            ("folder_view", folder_view, "pub fn navigate(&mut self, to: Location"),
            ("folder_view", folder_view, "pub fn back(&mut self"),
            ("folder_view", folder_view, "pub fn up(&mut self"),
            ("folder_view", folder_view, "    fn load(&mut self"),
            ("folder_view", folder_view, "fn start_watch(&mut self"),
            ("folder_view", folder_view, "fn warm_labels(&self"),
            ("folder_view", folder_view, "fn apply_folder_layout(&mut self"),
            ("folder_view", folder_view, "pub fn tab_icon(&self"),
            ("folder_view", folder_view, "pub fn title(&self"),
            ("folder_view", folder_view, "fn render_home(&mut self"),
            ("workspace", workspace, "    fn navigate(&mut self, loc: Location"),
            ("workspace", workspace, "fn render_breadcrumb(&mut self"),
            ("workspace", workspace, "fn breadcrumbs(path: &Path)"),
            ("workspace", workspace, "fn update_jump_list(places"),
        ];
        for (file, src, head) in heads {
            let body = body_of(src, head);
            let bad = offenders(&body);
            assert!(bad.is_empty(), "{file}: `{}` asks the disk on the UI thread with {bad:?}", head.trim());
        }
        // the sidebar's item is a multi-line signature
        let item = body_of(workspace, "    fn sidebar_item(\n");
        assert!(offenders(&item).is_empty(), "workspace: sidebar_item asks the disk on the UI thread with {:?}", offenders(&item));
    }
}

#[cfg(test)]
mod probe {
    use std::path::{Path, PathBuf};
    use std::time::Instant;

    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.0
    }

    /// Windowless, read-only: what the calls on the click-to-first-frame path cost on a place that is slow on purpose (a UNC host
    /// that does not exist: Windows needs seconds to give up, like the first touch of \\wsl.localhost\<distro> that has to start
    /// the distribution), and on a local folder. RF_PROBE_SLOW names another slow place (a WSL path, if a distribution is there).
    /// Run: cargo test -p rf-app --offline --lib nav_touch::probe -- --ignored --nocapture
    #[test]
    #[ignore]
    fn what_the_click_path_costs() {
        let slow = std::env::var("RF_PROBE_SLOW").unwrap_or_else(|_| r"\\rf-nav-probe-no-such-host\share\dir\sub".into());
        let slow = PathBuf::from(slow);
        let local = std::env::temp_dir();
        for (name, p) in [("slow", slow.as_path()), ("local", local.as_path())] {
            for round in ["first", "second"] {
                let t = Instant::now();
                let r = crate::archive_view::split(p);
                println!("{name:5} {round:6} archive_view::split           {:9.2} ms  -> {}", ms(t), r.is_some());
                let t = Instant::now();
                for q in super::label_paths(p) {
                    let _ = rf_win::localized::label_if_known(&q);
                }
                println!("{name:5} {round:6} localized::label_if_known (UI)  {:9.3} ms", ms(t));
                let t = Instant::now();
                let mut acc = PathBuf::new();
                for c in p.components() {
                    acc.push(c.as_os_str());
                    let _ = rf_win::localized::folder_label(&acc, None);
                }
                println!("{name:5} {round:6} localized::folder_label x{}   {:9.2} ms", p.components().count(), ms(t));
                let t = Instant::now();
                let _ = crate::archive_view::is_browsable_path(p);
                let _ = super::may_hold_archive(p);
                let _ = rf_win::network::server_root(p);
                let _ = rf_win::ftp::is_ftp(p);
                let _ = rf_core::history::Location::Path(p.to_path_buf()).cleaned();
                println!("{name:5} {round:6} name-only checks              {:9.2} ms", ms(t));
            }
        }
    }

    /// A folder of N files (RF_PROBE_FILES, default 100 000) in a temp folder this test makes and removes: the listing, and the one
    /// call of the click path that sees the folder.
    #[test]
    #[ignore]
    fn big_local_folder() {
        let n: usize = std::env::var("RF_PROBE_FILES").ok().and_then(|v| v.parse().ok()).unwrap_or(100_000);
        let dir = std::env::temp_dir().join(format!("rf-nav-probe-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let t = Instant::now();
        std::thread::scope(|s| {
            for part in 0..8 {
                let dir = &dir;
                s.spawn(move || {
                    for i in (part..n).step_by(8) {
                        std::fs::write(dir.join(format!("file-{i:06}.txt")), b"").unwrap();
                    }
                });
            }
        });
        println!("made {n} files in {:.0} ms", ms(t));
        for round in 0..3 {
            let t = Instant::now();
            let mut count = 0usize;
            rf_win::list_dir(&dir, 512, &|| false, &mut |b| count += b.len()).unwrap();
            let list = ms(t);
            let t = Instant::now();
            let _ = crate::archive_view::split(&dir);
            let split = ms(t);
            let t = Instant::now();
            let _ = crate::archive_view::split(&dir.join("file-000001.txt"));
            let split_file = ms(t);
            println!("round {round}: listed {count} in {list:.1} ms; split(folder) {split:.3} ms; split(file in it) {split_file:.3} ms");
        }
        std::fs::remove_dir_all(&dir).unwrap();
        let _ = Path::new("");
    }
}
