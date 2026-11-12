//! The apps pinned to the Windows taskbar, in the taskbar's order. Read-only: nothing is written, nothing is registered.
//!
//! Two places hold the pins. The shortcuts are in `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar`; the
//! order, and the pins that have no shortcut (a packaged app is pinned by its AppUserModelID alone), are in the binary value
//! `Favorites` of `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband`. Its layout, as found on Windows 11: a list of
//! records, each a `0x00` byte, a little-endian `u32` length and that many bytes (an item ID list that holds, as UTF-16 text, the
//! shortcut's file name or the package's AppUserModelID), ended by a `0xFF` byte. We do not parse the item ID list: the text in it
//! is enough to tell which shortcut or app the record stands for, and a record that cannot be understood is skipped.
//!
//! A shortcut is not always what it says: a program may have been replaced by its next version (Adobe's folders carry the year, the
//! shortcut keeps naming the old one). `fate_of` decides, `read_pins` acts on it.

use std::path::{Path, PathBuf};

use super::model::{IconSource, Launch, PinnedApp, looks_packaged};

/// What one record of the `Favorites` value says, as far as we need it: the runs of text found in it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub texts: Vec<String>,
}

/// Splits the `Favorites` value into its records. A value that does not follow the layout gives what could be read before the
/// first thing that did not fit (possibly nothing).
pub fn parse_taskband(blob: &[u8]) -> Vec<Record> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < blob.len() {
        match blob[pos] {
            0x00 => {
                let Some(len) = blob.get(pos + 1..pos + 5).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize) else { break };
                let Some(rec) = blob.get(pos + 5..pos + 5 + len) else { break };
                out.push(Record { texts: utf16_runs(rec, 4) });
                pos += 5 + len;
            }
            _ => break, // 0xFF ends the list; anything else is not a layout we know
        }
    }
    out
}

/// The UTF-16 text runs (of at least `min` characters) in some bytes, looked for at both byte alignments (the text inside an item ID
/// list starts wherever the preceding fields end).
fn utf16_runs(bytes: &[u8], min: usize) -> Vec<String> {
    let mut runs = Vec::new();
    for align in 0..2 {
        let mut cur: Vec<u16> = Vec::new();
        let mut flush = |cur: &mut Vec<u16>| {
            if cur.len() >= min {
                runs.push(String::from_utf16_lossy(cur));
            }
            cur.clear();
        };
        let mut i = align;
        while i + 1 < bytes.len() {
            let u = u16::from_le_bytes([bytes[i], bytes[i + 1]]);
            // printable: not a control character, not a surrogate half, not a non-character
            if u >= 0x20 && !(0xD800..0xE000).contains(&u) && u < 0xFFFE {
                cur.push(u);
            } else {
                flush(&mut cur);
            }
            i += 2;
        }
        flush(&mut cur);
    }
    runs
}

/// A package's AppUserModelID found in some text (`Package.Name_publisherid!AppId`), with whatever precedes it cut off.
pub fn find_aumid(text: &str) -> Option<String> {
    let bang = text.find('!')?;
    let (head, tail) = (&text[..bang], &text[bang + 1..]);
    let app: String = tail.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')).collect();
    if app.is_empty() {
        return None;
    }
    // the publisher id: 13 characters of [a-z0-9]; before it an underscore and the package name
    let head_chars: Vec<char> = head.chars().collect();
    if head_chars.len() < 15 {
        return None;
    }
    let publisher: String = head_chars[head_chars.len() - 13..].iter().collect();
    if !publisher.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()) || head_chars[head_chars.len() - 14] != '_' {
        return None;
    }
    let mut start = head_chars.len() - 14;
    while start > 0 && (head_chars[start - 1].is_ascii_alphanumeric() || matches!(head_chars[start - 1], '.' | '-')) {
        start -= 1;
    }
    if start == head_chars.len() - 14 {
        return None;
    }
    let id: String = head_chars[start..].iter().collect::<String>() + "!" + &app;
    looks_packaged(&id).then_some(id)
}

/// One pin of the taskbar, in the order of the `Favorites` value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Slot {
    /// A shortcut of the pin folder, by its index in the list the caller gave.
    Link(usize),
    /// A packaged app with no shortcut.
    Packaged(String),
}

/// Puts the pins in the taskbar's order: for every record the shortcut whose file name ends one of its texts (the longest name wins,
/// so `Google Chrome.lnk` is not taken for `Chrome.lnk`), or else the AppUserModelID it holds; then, at the end and by name, the
/// shortcuts no record named (a pin the shell has not written into the value yet). A shortcut is used once.
pub fn order_pins(link_names: &[String], records: &[Record]) -> Vec<Slot> {
    let lower: Vec<String> = link_names.iter().map(|n| n.to_lowercase()).collect();
    let mut used = vec![false; link_names.len()];
    let mut out = Vec::new();
    for rec in records {
        let texts: Vec<String> = rec.texts.iter().map(|t| t.to_lowercase()).collect();
        let best = (0..lower.len()).filter(|&i| !used[i] && texts.iter().any(|t| t.ends_with(&lower[i]))).max_by_key(|&i| lower[i].len());
        if let Some(i) = best {
            used[i] = true;
            out.push(Slot::Link(i));
        } else if let Some(id) = rec.texts.iter().find_map(|t| find_aumid(t)) {
            let again = out.iter().any(|s| matches!(s, Slot::Packaged(p) if p.eq_ignore_ascii_case(&id)));
            if !again {
                out.push(Slot::Packaged(id));
            }
        }
    }
    let mut rest: Vec<usize> = (0..lower.len()).filter(|&i| !used[i]).collect();
    rest.sort_by(|&a, &b| lower[a].cmp(&lower[b]));
    out.extend(rest.into_iter().map(Slot::Link));
    out
}

/// The folder that holds the taskbar's pinned shortcuts.
pub fn pin_folder() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar"))
}

/// The `Favorites` value, if it can be read.
fn read_favorites() -> Option<Vec<u8>> {
    let key = windows_registry::CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband").ok()?;
    let v = key.get_value("Favorites").ok()?;
    Some(v.to_vec())
}

/// The AppUserModelID a shortcut carries in its property store.
fn link_app_id(link: &windows::Win32::UI::Shell::IShellLinkW) -> Option<String> {
    use windows::Win32::Storage::EnhancedStorage::PKEY_AppUserModel_ID;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::core::Interface;
    unsafe {
        let store: IPropertyStore = link.cast().ok()?;
        let v = store.GetValue(&PKEY_AppUserModel_ID).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

/// Fills in `%NAME%` variables from `lookup` (case-insensitive names are the lookup's business); an unknown one stays as written.
pub fn expand_vars(s: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) if j > 0 && !after[..j].contains(char::is_whitespace) => match lookup(&after[..j]) {
                Some(v) => {
                    out.push_str(&v);
                    rest = &after[j + 1..];
                }
                None => {
                    out.push('%');
                    rest = after;
                }
            },
            _ => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// What a shortcut says: its target, its AppUserModelID, its arguments and its icon location.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct LinkFacts {
    pub target: PathBuf,
    pub aumid: Option<String>,
    pub args: String,
    /// The icon location with `%variables%` expanded; empty when the shortcut names none.
    pub icon_file: String,
    pub icon_index: i32,
}

fn read_link(path: &Path) -> Option<LinkFacts> {
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistFile, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    use windows::core::{Interface, PCWSTR};
    crate::shell::ensure_com();
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        let file: IPersistFile = link.cast().ok()?;
        let w = crate::wide::wide(path);
        file.Load(PCWSTR(w.as_ptr()), STGM_READ).ok()?;
        let mut buf = vec![0u16; 32768];
        let mut fd = WIN32_FIND_DATAW::default();
        let _ = link.GetPath(&mut buf, &mut fd, 0);
        let target = PathBuf::from(crate::wide::from_wide(&buf));
        buf.fill(0);
        let _ = link.GetArguments(&mut buf);
        let args = crate::wide::from_wide(&buf);
        buf.fill(0);
        let mut icon_index = 0i32;
        let _ = link.GetIconLocation(&mut buf, &mut icon_index);
        // a shortcut keeps its icon location with %variables% unexpanded (Task Manager's is `%windir%\system32\Taskmgr.exe`)
        let icon_file = expand_vars(&crate::wide::from_wide(&buf), |n| std::env::var(n).ok());
        Some(LinkFacts { target, aumid: link_app_id(&link), args, icon_file, icon_index })
    }
}

/// What the shell calls the app with this AppUserModelID, or `None` when no such app is installed (a pin whose app was removed) or
/// the shell did not answer in a few seconds.
///
/// Asked on the shell thread, like every shell call of the dock (see [`crate::sta::call`]).
pub fn packaged_name(aumid: &str) -> Option<String> {
    let aumid = aumid.to_string();
    crate::sta::call(std::time::Duration::from_secs(4), move || packaged_name_here(&aumid)).flatten()
}

fn packaged_name_here(aumid: &str) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_NORMALDISPLAY};
    use windows::core::PCWSTR;
    crate::shell::ensure_com();
    let w = crate::wide::wide(format!("shell:AppsFolder\\{aumid}"));
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(w.as_ptr()), None).ok()?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

// ---------------------------------------------------------------------------------------------------------------------------------
// Which pins are alive, which moved, which are dead
// ---------------------------------------------------------------------------------------------------------------------------------

/// The questions the pin rules ask of the machine, so the rules themselves are pure and tested with a made-up disk.
pub trait Disk {
    fn exists(&self, p: &Path) -> bool;
    /// The names of the folders in `dir` (empty when it cannot be read).
    fn folders(&self, dir: &Path) -> Vec<String>;
    /// Where Windows registered a program of this file name (`App Paths`), if it did.
    fn registered(&self, exe_name: &str) -> Option<PathBuf>;
    /// The programs the Start menu's shortcuts point at, whose file name is `exe_name`.
    fn start_menu(&self, exe_name: &str) -> Vec<PathBuf>;
}

/// Why a pin is left out of the dock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dead {
    /// The shortcut points at nothing at all and carries no AppUserModelID that could start something.
    NoTarget,
    /// The program is gone and nothing that looks like its newer version was found.
    TargetGone,
}

/// What a shortcut of the taskbar amounts to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fate {
    /// As the shortcut says.
    Alive,
    /// The shortcut's program was replaced by a newer install: the pin stands for this one (Adobe's programs carry their year in the
    /// folder name, and the shortcut keeps naming last year's).
    Moved(PathBuf),
    Dead(Dead),
}

/// `Adobe Photoshop 2026` -> `adobe photoshop #`: the name with every run of digits replaced, so two versions of a program have the
/// same skeleton.
fn skeleton(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut in_digits = false;
    for c in name.chars() {
        if c.is_ascii_digit() {
            if !in_digits {
                out.push('#');
            }
            in_digits = true;
        } else {
            in_digits = false;
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// The numbers in a name, in order (`Adobe Photoshop 2027` -> `[2027]`): what "newer" is measured with.
fn version_key(name: &str) -> Vec<u64> {
    let mut v = Vec::new();
    let mut cur: Option<u64> = None;
    for c in name.chars() {
        match (c.to_digit(10), cur) {
            (Some(d), c0) => cur = Some(c0.unwrap_or(0).saturating_mul(10).saturating_add(u64::from(d))),
            (None, Some(n)) => {
                v.push(n);
                cur = None;
            }
            (None, None) => {}
        }
    }
    v.extend(cur);
    v
}

/// The same program in the newest sibling version folder: the deepest folder of the path that has digits in its name is looked for
/// next to itself under names of the same shape (`Adobe Photoshop 2026` -> `Adobe Photoshop 2027`), the newest first, and the rest of
/// the path must exist under it.
pub fn newer_install(target: &Path, disk: &dyn Disk) -> Option<PathBuf> {
    let comps: Vec<_> = target.components().collect();
    // never the file name itself (the last component) and never the drive or the root (the first two)
    for idx in (2..comps.len().saturating_sub(1)).rev() {
        let name = comps[idx].as_os_str().to_string_lossy().into_owned();
        if !name.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        let parent: PathBuf = comps[..idx].iter().collect();
        let rest: PathBuf = comps[idx + 1..].iter().collect();
        let mut siblings: Vec<String> = disk.folders(&parent).into_iter().filter(|s| !s.eq_ignore_ascii_case(&name) && skeleton(s) == skeleton(&name)).collect();
        siblings.sort_by_key(|s| std::cmp::Reverse(version_key(s)));
        for s in siblings {
            let candidate = parent.join(&s).join(&rest);
            if disk.exists(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

/// How many leading components two paths share (the closer a replacement is to where the program used to be, the better).
fn shared_prefix(a: &Path, b: &Path) -> usize {
    a.components().zip(b.components()).take_while(|(x, y)| x.as_os_str().eq_ignore_ascii_case(y.as_os_str())).count()
}

/// What happened to a program whose shortcut names a file that is not there any more, if anything can be found: the newer install
/// next to it, the place Windows registered a program of that name at, a Start menu shortcut to a program of that name (the closest
/// to the old place first).
pub fn find_replacement(target: &Path, disk: &dyn Disk) -> Option<PathBuf> {
    if let Some(p) = newer_install(target, disk) {
        return Some(p);
    }
    let name = target.file_name()?.to_string_lossy().into_owned();
    if let Some(p) = disk.registered(&name).filter(|p| disk.exists(p)) {
        return Some(p);
    }
    // The Start menu holds shortcuts to many programs with the same generic file name (the `Update.exe` of every Squirrel app,
    // `launcher.exe`, `setup.exe`): only one that stands in a folder of the same shape as the old one is taken for the same program,
    // or a click on a pin would start somebody else's program.
    let shape = |p: &Path| p.parent().and_then(Path::file_name).map(|n| skeleton(&n.to_string_lossy()));
    let old_shape = shape(target);
    let mut starts: Vec<PathBuf> = disk.start_menu(&name).into_iter().filter(|p| disk.exists(p) && shape(p) == old_shape).collect();
    starts.sort_by_key(|p| std::cmp::Reverse(shared_prefix(p, target)));
    starts.into_iter().next()
}

/// The rules for one shortcut that has a file-system target:
/// 1. the program is there: alive. A pin the user made on the taskbar and that still starts is never taken out of the dock because
///    the icon file its shortcut names is gone (an installer's cache that was cleaned): `pinned_from_link` then draws the program's own
///    icon, and the taskbar shows the pin too;
/// 2. the program is gone: moved when [`find_replacement`] knows where to, else dead.
pub fn fate_of_target(f: &LinkFacts, disk: &dyn Disk) -> Fate {
    if disk.exists(&f.target) {
        return Fate::Alive;
    }
    match find_replacement(&f.target, disk) {
        Some(p) => Fate::Moved(p),
        None => Fate::Dead(Dead::TargetGone),
    }
}

/// The rules for any shortcut: no target means an AppUserModelID is all there is (kept when there is one).
pub fn fate_of(f: &LinkFacts, disk: &dyn Disk) -> Fate {
    if f.target.as_os_str().is_empty() {
        return if f.aumid.is_some() { Fate::Alive } else { Fate::Dead(Dead::NoTarget) };
    }
    fate_of_target(f, disk)
}

/// The real machine for [`Disk`].
pub struct RealDisk;

impl Disk for RealDisk {
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }

    fn folders(&self, dir: &Path) -> Vec<String> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.file_name().to_string_lossy().into_owned()).collect()
    }

    fn registered(&self, exe_name: &str) -> Option<PathBuf> {
        let sub = format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\{exe_name}");
        let wow = format!(r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\App Paths\{exe_name}");
        for (root, path) in [(windows_registry::CURRENT_USER, &sub), (windows_registry::LOCAL_MACHINE, &sub), (windows_registry::LOCAL_MACHINE, &wow)] {
            // read-only: the key is only opened for reading
            let Ok(key) = root.open(path) else { continue };
            // the default value is the program itself; some installers give only `Path`, the folder it is in
            let exe = key.get_string("").ok().map(|s| PathBuf::from(s.trim_matches('"'))).filter(|p| p.exists());
            let in_dir = || key.get_string("Path").ok().and_then(|s| s.split(';').map(|d| Path::new(d.trim().trim_matches('"')).join(exe_name)).find(|p| p.exists()));
            if let Some(p) = exe.or_else(in_dir) {
                return Some(p);
            }
        }
        None
    }

    fn start_menu(&self, exe_name: &str) -> Vec<PathBuf> {
        start_menu_targets().iter().filter(|t| t.file_name().is_some_and(|n| n.eq_ignore_ascii_case(exe_name))).cloned().collect()
    }
}

/// The programs the Start menu's shortcuts point at. Reading every shortcut is slow (COM, one by one), the list is needed only for a
/// pin whose program is gone, and it changes only when something is installed: it is kept for ten minutes.
fn start_menu_targets() -> Vec<PathBuf> {
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static CACHE: Mutex<Option<(Instant, Vec<PathBuf>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((at, v)) = c.as_ref()
        && at.elapsed() < Duration::from_secs(600)
    {
        return v.clone();
    }
    let mut out = Vec::new();
    for var in ["ProgramData", "APPDATA"] {
        if let Some(d) = std::env::var_os(var) {
            collect_link_targets(&PathBuf::from(d).join(r"Microsoft\Windows\Start Menu\Programs"), 3, &mut out);
        }
    }
    *c = Some((Instant::now(), out.clone()));
    out
}

fn collect_link_targets(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => collect_link_targets(&p, depth - 1, out),
            Ok(t) if t.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) => {
                if let Some(f) = read_link(&p).filter(|f| !f.target.as_os_str().is_empty()) {
                    out.push(f.target);
                }
            }
            _ => {}
        }
    }
}

/// What the dock does with one pin, for the dock and for the probe.
#[derive(Clone, Debug)]
pub struct PinReport {
    /// The shortcut's file name, or the package's AppUserModelID.
    pub file: String,
    pub facts: Option<LinkFacts>,
    pub fate: Fate,
    /// The app the dock shows for it, `None` when the pin is left out.
    pub app: Option<PinnedApp>,
}

fn pinned_from_link(path: &Path, name: &str, f: &LinkFacts, fate: &Fate) -> PinnedApp {
    let aumid = f.aumid.as_deref().map(str::to_lowercase);
    let packaged_id = f.aumid.as_deref().filter(|id| looks_packaged(id) && f.target.as_os_str().is_empty());
    // the program the pin starts: where the shortcut says, or where the program went
    let target = match fate {
        Fate::Moved(p) => p.clone(),
        _ => f.target.clone(),
    };
    // an icon of its own: another file, or another icon of the same program than its first (a file the shortcut names that does
    // not exist is not one: the program's own icon is the best left)
    let icon_there = !f.icon_file.is_empty() && Path::new(&f.icon_file).exists();
    let own_icon = icon_there && (f.icon_index != 0 || !f.icon_file.eq_ignore_ascii_case(&f.target.to_string_lossy()));
    let icon = if let Some(id) = packaged_id {
        IconSource::Packaged(id.to_string())
    } else if own_icon {
        IconSource::Resource { file: PathBuf::from(&f.icon_file), index: f.icon_index }
    } else if !target.as_os_str().is_empty() {
        IconSource::File(target.clone())
    } else {
        IconSource::File(path.to_path_buf())
    };
    let exe = (!target.as_os_str().is_empty()).then(|| target.to_string_lossy().to_lowercase());
    // a moved program cannot be started through the old shortcut: it is started where it is, with the shortcut's arguments
    let launch = match fate {
        Fate::Moved(p) => Launch::Exe { path: p.clone(), args: f.args.clone() },
        _ => Launch::Link(path.to_path_buf()),
    };
    PinnedApp { name: name.to_string(), aumid, exe, launch, icon }
}

/// Looks at every pin of the Windows taskbar, in its order, and says what the dock makes of each. Blocks (COM, the shell, the disk).
pub fn inspect_pins(disk: &dyn Disk) -> Vec<PinReport> {
    let Some(dir) = pin_folder() else { return Vec::new() };
    let mut links: Vec<(String, PathBuf)> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let p = e.path();
            let is_lnk = p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
            if is_lnk && p.is_file() && let Some(n) = p.file_name() {
                links.push((n.to_string_lossy().into_owned(), p));
            }
        }
    }
    let names: Vec<String> = links.iter().map(|(n, _)| n.clone()).collect();
    let records = read_favorites().map(|b| parse_taskband(&b)).unwrap_or_default();
    let mut out = Vec::new();
    for slot in order_pins(&names, &records) {
        match slot {
            Slot::Link(i) => {
                let (file, path) = &links[i];
                let stem = Path::new(file).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                let Some(facts) = read_link(path) else {
                    out.push(PinReport { file: file.clone(), facts: None, fate: Fate::Dead(Dead::NoTarget), app: None });
                    continue;
                };
                let fate = fate_of(&facts, disk);
                let app = (!matches!(fate, Fate::Dead(_))).then(|| pinned_from_link(path, &stem, &facts, &fate));
                match &fate {
                    Fate::Dead(why) => log::debug!("dock: pin {file} left out: {why:?}"),
                    Fate::Moved(p) => log::debug!("dock: pin {file} stands for {} now", p.display()),
                    Fate::Alive => {}
                }
                out.push(PinReport { file: file.clone(), facts: Some(facts), fate, app });
            }
            Slot::Packaged(id) => {
                let app = packaged_name(&id).map(|name| PinnedApp {
                    name,
                    aumid: Some(id.to_lowercase()),
                    exe: None,
                    launch: Launch::Packaged(id.clone()),
                    icon: IconSource::Packaged(id.clone()),
                });
                let fate = if app.is_some() { Fate::Alive } else { Fate::Dead(Dead::TargetGone) };
                out.push(PinReport { file: id, facts: None, fate, app });
            }
        }
    }
    out
}

/// The apps pinned to the Windows taskbar, in its order. Dead pins (a program that is gone and has no newer install, an icon the
/// shortcut names that is gone, a package that was removed) are left out; a pin whose program was replaced by a newer install stands
/// for that one. Blocks (COM, the shell, the disk): call it from a worker thread, and again when the pin folder changes.
pub fn read_pins() -> Vec<PinnedApp> {
    read_pins_checked().unwrap_or_default()
}

/// [`read_pins`] that says when the shell thread did not answer (`None`) instead of passing it off as "no pins".
pub fn read_pins_checked() -> Option<Vec<PinnedApp>> {
    // On the shell thread, like every other shell call of the dock (see `crate::sta::call`). No answer in time is "no pins for now":
    // the caller reads them again on its next turn.
    crate::sta::call(std::time::Duration::from_secs(15), || inspect_pins(&RealDisk).into_iter().filter_map(|r| r.app).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A record as the shell writes it: some binary, the text as UTF-16, some more binary.
    fn record_with(texts: &[&str]) -> Vec<u8> {
        let mut body = vec![0x14, 0x00, 0x1F, 0x80, 0x9B, 0xD4, 0x34, 0x42, 0x45];
        for t in texts {
            body.extend_from_slice(&[0x31, 0x00, 0x00, 0x00]);
            for u in t.encode_utf16() {
                body.extend_from_slice(&u.to_le_bytes());
            }
            body.extend_from_slice(&[0, 0, 0x11, 0, 0, 0]);
        }
        let mut v = vec![0x00];
        v.extend_from_slice(&(body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }

    fn blob(records: &[&[&str]]) -> Vec<u8> {
        let mut v: Vec<u8> = records.iter().flat_map(|r| record_with(r)).collect();
        v.push(0xFF);
        v
    }

    #[test]
    fn records_are_split_and_their_text_is_found() {
        let b = blob(&[&["rustyfiles.lnk"], &["Microsoft.WindowsStore_8wekyb3d8bbwe!App"], &["Google Chrome.lnk"]]);
        let recs = parse_taskband(&b);
        assert_eq!(recs.len(), 3);
        assert!(recs[0].texts.iter().any(|t| t.ends_with("rustyfiles.lnk")));
        assert!(recs[1].texts.iter().any(|t| t.contains("WindowsStore_8wekyb3d8bbwe!App")));
        assert!(recs[2].texts.iter().any(|t| t.ends_with("Google Chrome.lnk")));
    }

    #[test]
    fn a_damaged_value_gives_what_came_before_the_damage() {
        let mut b = blob(&[&["a.lnk"], &["b.lnk"]]);
        b.truncate(b.len() - 12); // the second record is cut off
        assert_eq!(parse_taskband(&b).len(), 1);
        assert!(parse_taskband(&[]).is_empty());
        assert!(parse_taskband(&[0x07, 1, 2, 3]).is_empty(), "an unknown layout");
        assert!(parse_taskband(&[0x00, 0xFF, 0xFF, 0xFF, 0x7F]).is_empty(), "a length beyond the value");
    }

    #[test]
    fn aumids_are_cut_out_of_noisy_text() {
        assert_eq!(find_aumid("Microsoft.WindowsStore_8wekyb3d8bbwe!App").as_deref(), Some("Microsoft.WindowsStore_8wekyb3d8bbwe!App"));
        assert_eq!(find_aumid("%Microsoft.Windows.Photos_8wekyb3d8bbwe!App").as_deref(), Some("Microsoft.Windows.Photos_8wekyb3d8bbwe!App"));
        assert_eq!(find_aumid("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify").as_deref(), Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"));
        assert_eq!(
            find_aumid("TelegramMessengerLLP.TelegramDesktop_t4vj0pshhgkwm!Telegram.TelegramDesktop.Store").as_deref(),
            Some("TelegramMessengerLLP.TelegramDesktop_t4vj0pshhgkwm!Telegram.TelegramDesktop.Store")
        );
        assert_eq!(find_aumid("Chrome"), None);
        assert_eq!(find_aumid("Hello! World"), None);
        assert_eq!(find_aumid("Pkg_short!App"), None, "the publisher id is 13 characters");
        assert_eq!(find_aumid("!App"), None);
    }

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn order_follows_the_records() {
        let links = names(&["Firefox.lnk", "Google Chrome.lnk", "rustyfiles.lnk", "Task Manager.lnk"]);
        let b = blob(&[&["rustyfiles.lnk"], &["Microsoft.WindowsStore_8wekyb3d8bbwe!App"], &["Google Chrome.lnk"], &["Task Manager.lnk"], &["Firefox.lnk"]]);
        let slots = order_pins(&links, &parse_taskband(&b));
        assert_eq!(slots, vec![Slot::Link(2), Slot::Packaged("Microsoft.WindowsStore_8wekyb3d8bbwe!App".into()), Slot::Link(1), Slot::Link(3), Slot::Link(0)]);
    }

    #[test]
    fn the_longest_name_wins_and_a_shortcut_is_used_once() {
        let links = names(&["Chrome.lnk", "Google Chrome.lnk"]);
        let b = blob(&[&["Google Chrome.lnk"], &["Chrome.lnk"]]);
        assert_eq!(order_pins(&links, &parse_taskband(&b)), vec![Slot::Link(1), Slot::Link(0)]);
        // the same record twice does not pin it twice
        let b = blob(&[&["Chrome.lnk"], &["Chrome.lnk"]]);
        assert_eq!(order_pins(&links, &parse_taskband(&b)), vec![Slot::Link(0), Slot::Link(1)], "the second record has nothing left to name; the rest comes last by name");
    }

    #[test]
    fn shortcuts_no_record_names_come_last_by_name_and_without_a_value_all_are_by_name() {
        let links = names(&["b.lnk", "a.lnk", "c.lnk"]);
        let b = blob(&[&["c.lnk"]]);
        assert_eq!(order_pins(&links, &parse_taskband(&b)), vec![Slot::Link(2), Slot::Link(1), Slot::Link(0)]);
        assert_eq!(order_pins(&links, &[]), vec![Slot::Link(1), Slot::Link(0), Slot::Link(2)]);
    }

    #[test]
    fn matching_ignores_case_and_a_package_pinned_twice_counts_once() {
        let links = names(&["Notepad++.lnk"]);
        let b = blob(&[&["notepad++.LNK"], &["Pkg.Name_abcdefghijklm!App"], &["Pkg.Name_abcdefghijklm!App"]]);
        assert_eq!(order_pins(&links, &parse_taskband(&b)), vec![Slot::Link(0), Slot::Packaged("Pkg.Name_abcdefghijklm!App".into())]);
    }

    #[test]
    fn variables_in_an_icon_location_are_expanded() {
        let look = |n: &str| match n.to_ascii_lowercase().as_str() {
            "windir" => Some(r"C:\WINDOWS".to_string()),
            _ => None,
        };
        assert_eq!(expand_vars(r"%windir%\system32\Taskmgr.exe", look), r"C:\WINDOWS\system32\Taskmgr.exe");
        assert_eq!(expand_vars(r"%nothing%\x.exe", look), r"%nothing%\x.exe", "unknown: as written");
        assert_eq!(expand_vars("100% sure", look), "100% sure");
        assert_eq!(expand_vars("a%%b", look), "a%%b");
        assert_eq!(expand_vars("%windir%%windir%", look), r"C:\WINDOWSC:\WINDOWS");
        assert_eq!(expand_vars("", look), "");
    }

    // ---- which pins are alive, moved or dead: the shapes found on this machine, run against a made-up disk ----

    use std::collections::{BTreeMap, BTreeSet};

    #[derive(Default)]
    struct FakeDisk {
        files: BTreeSet<String>,
        registered: BTreeMap<String, String>,
        start: Vec<String>,
    }

    impl FakeDisk {
        fn with(files: &[&str]) -> FakeDisk {
            FakeDisk { files: files.iter().map(|f| f.to_string()).collect(), ..Default::default() }
        }
    }

    impl Disk for FakeDisk {
        fn exists(&self, p: &Path) -> bool {
            let s = p.to_string_lossy().to_lowercase();
            self.files.iter().any(|f| {
                let f = f.to_lowercase();
                f == s || f.starts_with(&format!("{s}\\"))
            })
        }

        fn folders(&self, dir: &Path) -> Vec<String> {
            let prefix = format!("{}\\", dir.to_string_lossy().trim_end_matches('\\')).to_lowercase();
            let mut v: Vec<String> = Vec::new();
            for f in &self.files {
                // the folder names keep their case; only the comparison ignores it
                if f.to_lowercase().starts_with(&prefix)
                    && let Some((first, _)) = f[prefix.len()..].split_once('\\')
                    && !v.iter().any(|x: &String| x == first)
                {
                    v.push(first.to_string());
                }
            }
            v
        }

        fn registered(&self, exe_name: &str) -> Option<PathBuf> {
            self.registered.get(&exe_name.to_lowercase()).map(PathBuf::from)
        }

        fn start_menu(&self, exe_name: &str) -> Vec<PathBuf> {
            self.start.iter().filter(|s| s.to_lowercase().ends_with(&format!("\\{}", exe_name.to_lowercase()))).map(PathBuf::from).collect()
        }
    }

    fn facts(target: &str) -> LinkFacts {
        LinkFacts { target: PathBuf::from(target), ..Default::default() }
    }

    const PS26: &str = r"C:\Program Files\Adobe\Adobe Photoshop 2026\Photoshop.exe";
    const PS27: &str = r"C:\Program Files\Adobe\Adobe Photoshop 2027\Photoshop.exe";
    const AI26: &str = r"C:\Program Files\Adobe\Adobe Illustrator 2026\Support Files\Contents\Windows\Illustrator.exe";
    const AI27: &str = r"C:\Program Files\Adobe\Adobe Illustrator 2027\Support Files\Contents\Windows\Illustrator.exe";
    const AE26: &str = r"C:\Program Files\Adobe\Adobe After Effects 2026\Support Files\AfterFX.exe";
    const AE27: &str = r"C:\Program Files\Adobe\Adobe After Effects 2027\Support Files\AfterFX.exe";

    #[test]
    fn a_program_that_moved_to_the_next_years_folder_is_found_there() {
        // Photoshop and Illustrator 2026 are gone, 2027 is installed; After Effects 2026 left its folder behind without the program
        let disk = FakeDisk::with(&[PS27, AI27, AE27, r"C:\Program Files\Adobe\Adobe After Effects 2026\Support Files\readme.txt", r"C:\Program Files\Adobe\Adobe Lightroom CC\lightroom.exe"]);
        assert_eq!(fate_of(&facts(PS26), &disk), Fate::Moved(PathBuf::from(PS27)));
        assert_eq!(fate_of(&facts(AI26), &disk), Fate::Moved(PathBuf::from(AI27)), "the folder that changes is above a deeper path");
        assert_eq!(fate_of(&facts(AE26), &disk), Fate::Moved(PathBuf::from(AE27)), "an old folder that is still there without the program does not hold the pin back");
        // a program that has not moved stays as it is
        assert_eq!(fate_of(&facts(r"C:\Program Files\Adobe\Adobe Lightroom CC\lightroom.exe"), &disk), Fate::Alive);
    }

    #[test]
    fn the_newest_version_wins_and_its_path_must_exist() {
        let disk = FakeDisk::with(&[PS27, r"C:\Program Files\Adobe\Adobe Photoshop 2025\Photoshop.exe", r"C:\Program Files\Adobe\Adobe Photoshop 2028\Other.exe"]);
        assert_eq!(newer_install(Path::new(PS26), &disk), Some(PathBuf::from(PS27)), "2028 has no Photoshop.exe, 2025 is older");
        // nothing of that shape beside it
        let none = FakeDisk::with(&[r"C:\Program Files\Adobe\Adobe Lightroom CC\lightroom.exe"]);
        assert_eq!(newer_install(Path::new(PS26), &none), None);
        // numbers in a folder name that are not the same shape are not versions of each other
        assert_eq!(newer_install(Path::new(r"C:\Tools\x64\tool.exe"), &FakeDisk::with(&[r"C:\Tools\tool32\tool.exe"])), None);
        // numbers compare as numbers: 10 beats 9
        let d = FakeDisk::with(&[r"C:\a\App 9\x.exe", r"C:\a\App 10\x.exe"]);
        assert_eq!(newer_install(Path::new(r"C:\a\App 8\x.exe"), &d), Some(PathBuf::from(r"C:\a\App 10\x.exe")));
    }

    #[test]
    fn a_program_registered_with_windows_or_in_the_start_menu_is_found_when_no_folder_has_it() {
        let mut disk = FakeDisk::with(&[r"D:\Apps\Photo Tool\ptool.exe", r"E:\Other\Photo Tool\ptool.exe"]);
        disk.registered.insert("ptool.exe".into(), r"D:\Apps\Photo Tool\ptool.exe".into());
        assert_eq!(fate_of(&facts(r"C:\Old\Photo Tool\ptool.exe"), &disk), Fate::Moved(PathBuf::from(r"D:\Apps\Photo Tool\ptool.exe")), "App Paths first");
        disk.registered.clear();
        disk.start = vec![r"D:\Apps\Photo Tool\ptool.exe".into(), r"E:\Other\Photo Tool\ptool.exe".into(), r"E:\Other\Photo Tool\another.exe".into()];
        assert_eq!(fate_of(&facts(r"E:\Gone\Photo Tool\ptool.exe"), &disk), Fate::Moved(PathBuf::from(r"E:\Other\Photo Tool\ptool.exe")), "the Start menu entry closest to the old place");
        // a registration that points at a file that is not there either is no help
        disk.start.clear();
        disk.registered.insert("ptool.exe".into(), r"Z:\nowhere\ptool.exe".into());
        assert_eq!(fate_of(&facts(r"C:\Old\Photo Tool\ptool.exe"), &disk), Fate::Dead(Dead::TargetGone));
    }

    #[test]
    fn a_program_of_the_same_file_name_in_another_folder_is_another_program() {
        // Squirrel apps all have an `Update.exe`; the Start menu has a shortcut to Slack's, the pin was Discord's
        let mut disk = FakeDisk::with(&[r"C:\Users\x\AppData\Local\Slack\Update.exe"]);
        disk.start = vec![r"C:\Users\x\AppData\Local\Slack\Update.exe".into()];
        assert_eq!(fate_of(&facts(r"C:\Users\x\AppData\Local\Discord\Update.exe"), &disk), Fate::Dead(Dead::TargetGone));
        // the same folder name on another drive is the same program
        let mut disk = FakeDisk::with(&[r"D:\Apps\Discord\Update.exe"]);
        disk.start = vec![r"D:\Apps\Discord\Update.exe".into()];
        assert_eq!(fate_of(&facts(r"C:\Users\x\AppData\Local\Discord\Update.exe"), &disk), Fate::Moved(PathBuf::from(r"D:\Apps\Discord\Update.exe")));
    }

    #[test]
    fn a_program_that_is_gone_without_a_trace_is_dead() {
        assert_eq!(fate_of(&facts(PS26), &FakeDisk::default()), Fate::Dead(Dead::TargetGone));
    }

    #[test]
    fn a_program_whose_icon_file_is_gone_stays_pinned_with_its_own_icon() {
        // DaVinci Resolve's shortcut: the program is there and starts, the icon it names (in the installer's cache) is not. The taskbar
        // still shows the pin, so the dock must not lose it; it draws the program's own icon.
        let target = r"C:\Program Files\Blackmagic Design\DaVinci Resolve\Resolve.exe";
        let icon = r"C:\WINDOWS\Installer\{B304CB42-340D-45FA-9769-2F229011B682}\ResolveIcon.exe";
        let davinci = LinkFacts { target: PathBuf::from(target), icon_file: icon.into(), ..Default::default() };
        assert_eq!(fate_of(&davinci, &FakeDisk::with(&[target])), Fate::Alive);
        assert_eq!(fate_of(&davinci, &FakeDisk::with(&[target, icon])), Fate::Alive);
        // the icon file the shortcut names is not on this machine's disk, so the program's own icon is the one asked for
        let p = pinned_from_link(Path::new(r"C:\pins\DaVinci Resolve.lnk"), "DaVinci Resolve", &davinci, &Fate::Alive);
        assert_eq!(p.icon, IconSource::File(PathBuf::from(target)));
        for loc in ["", r"%windir%\system32\Taskmgr.exe", "shell32.dll"] {
            let f = LinkFacts { icon_file: loc.into(), ..davinci.clone() };
            assert_eq!(fate_of(&f, &FakeDisk::with(&[target])), Fate::Alive, "{loc:?}");
        }
        // Task Manager's expanded location exists
        let tm = LinkFacts { target: PathBuf::from(r"C:\WINDOWS\system32\taskmgr.exe"), icon_file: r"C:\WINDOWS\system32\Taskmgr.exe".into(), icon_index: -30651, ..Default::default() };
        assert_eq!(fate_of(&tm, &FakeDisk::with(&[r"C:\WINDOWS\system32\taskmgr.exe"])), Fate::Alive);
    }

    #[test]
    fn a_shortcut_without_a_target_lives_by_its_app_id() {
        let packaged = LinkFacts { aumid: Some("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify".into()), ..Default::default() };
        assert_eq!(fate_of(&packaged, &FakeDisk::default()), Fate::Alive);
        assert_eq!(fate_of(&LinkFacts::default(), &FakeDisk::default()), Fate::Dead(Dead::NoTarget));
    }

    #[test]
    fn a_moved_pin_is_started_where_it_is_and_matches_the_running_program() {
        let f = LinkFacts { target: PathBuf::from(PS26), args: "--safe".into(), ..Default::default() };
        let fate = Fate::Moved(PathBuf::from(PS27));
        let p = pinned_from_link(Path::new(r"C:\pins\Adobe Photoshop 2026.lnk"), "Adobe Photoshop 2026", &f, &fate);
        assert_eq!(p.exe.as_deref(), Some(PS27.to_lowercase().as_str()), "the running Photoshop 2027 is this pin's window");
        assert_eq!(p.launch, Launch::Exe { path: PathBuf::from(PS27), args: "--safe".into() });
        assert_eq!(p.icon, IconSource::File(PathBuf::from(PS27)));
        // an alive pin is started through its shortcut
        let f = LinkFacts { target: PathBuf::from(PS27), ..Default::default() };
        let p = pinned_from_link(Path::new(r"C:\pins\p.lnk"), "P", &f, &Fate::Alive);
        assert_eq!(p.launch, Launch::Link(PathBuf::from(r"C:\pins\p.lnk")));
    }

    #[test]
    fn versions_are_numbers_and_shapes_ignore_case() {
        assert_eq!(skeleton("Adobe Photoshop 2026"), skeleton("adobe photoshop 2027"));
        assert_ne!(skeleton("Adobe Photoshop 2026"), skeleton("Adobe Illustrator 2026"));
        assert_eq!(version_key("Adobe Photoshop 2027"), vec![2027]);
        assert_eq!(version_key("App 1.10.2"), vec![1, 10, 2]);
        assert!(version_key("App 10") > version_key("App 9"));
    }

    /// Read-only, on this machine: what the dock makes of every pin. A pin is shown or left out, never both.
    #[test]
    fn the_real_pins_each_have_a_fate() {
        for r in inspect_pins(&RealDisk) {
            assert_eq!(r.app.is_some(), !matches!(r.fate, Fate::Dead(_)), "{}: {:?}", r.file, r.fate);
        }
    }

    /// Read-only: the real pin folder and value of this machine. Whatever is pinned, every pin has a name and a way to start it.
    #[test]
    fn reads_the_real_pins() {
        let pins = read_pins();
        eprintln!("{} pins: {:?}", pins.len(), pins.iter().map(|p| p.name.as_str()).collect::<Vec<_>>());
        for p in &pins {
            assert!(!p.name.is_empty());
            if let Launch::Link(l) = &p.launch {
                assert!(l.exists());
            }
        }
    }
}
