//! The apps pinned to the Windows taskbar, in the taskbar's order. Read-only: nothing is written, nothing is registered.
//!
//! Two places hold the pins. The shortcuts are in `%APPDATA%\Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar`; the
//! order, and the pins that have no shortcut (a packaged app is pinned by its AppUserModelID alone), are in the binary value
//! `Favorites` of `HKCU\Software\Microsoft\Windows\CurrentVersion\Explorer\Taskband`. Its layout, as found on Windows 11: a list of
//! records, each a `0x00` byte, a little-endian `u32` length and that many bytes (an item ID list that holds, as UTF-16 text, the
//! shortcut's file name or the package's AppUserModelID), ended by a `0xFF` byte. We do not parse the item ID list: the text in it
//! is enough to tell which shortcut or app the record stands for, and a record that cannot be understood is skipped.

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

/// What a shortcut says: its target, its AppUserModelID and its icon location.
struct LinkFacts {
    target: PathBuf,
    aumid: Option<String>,
    icon_file: String,
    icon_index: i32,
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
        let mut icon_index = 0i32;
        let _ = link.GetIconLocation(&mut buf, &mut icon_index);
        // a shortcut keeps its icon location with %variables% unexpanded (Task Manager's is `%windir%\system32\Taskmgr.exe`)
        let icon_file = expand_vars(&crate::wide::from_wide(&buf), |n| std::env::var(n).ok());
        Some(LinkFacts { target, aumid: link_app_id(&link), icon_file, icon_index })
    }
}

/// What the shell calls the app with this AppUserModelID, or `None` when no such app is installed (a pin whose app was removed).
pub fn packaged_name(aumid: &str) -> Option<String> {
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

/// Whether a shortcut's target still exists. A shortcut with no file-system target (a packaged app's shortcut, a shell item) is
/// kept when it carries an AppUserModelID, which is enough to start it; one with neither cannot be told dead from alive and is
/// dropped.
fn alive(f: &LinkFacts) -> bool {
    if f.target.as_os_str().is_empty() { f.aumid.is_some() } else { f.target.exists() }
}

fn pinned_from_link(path: &Path, name: &str, f: LinkFacts) -> PinnedApp {
    let aumid = f.aumid.as_deref().map(str::to_lowercase);
    let packaged_id = f.aumid.as_deref().filter(|id| looks_packaged(id) && f.target.as_os_str().is_empty());
    // an icon of its own: another file, or another icon of the same program than its first
    let own_icon = !f.icon_file.is_empty() && (f.icon_index != 0 || !f.icon_file.eq_ignore_ascii_case(&f.target.to_string_lossy()));
    let icon = if let Some(id) = packaged_id {
        IconSource::Packaged(id.to_string())
    } else if own_icon {
        IconSource::Resource { file: PathBuf::from(&f.icon_file), index: f.icon_index }
    } else if !f.target.as_os_str().is_empty() {
        IconSource::File(f.target.clone())
    } else {
        IconSource::File(path.to_path_buf())
    };
    let exe = (!f.target.as_os_str().is_empty()).then(|| f.target.to_string_lossy().to_lowercase());
    PinnedApp { name: name.to_string(), aumid, exe, launch: Launch::Link(path.to_path_buf()), icon }
}

/// The apps pinned to the Windows taskbar, in its order. Dead pins (a target that no longer exists, a package that was removed) are
/// skipped. Blocks (COM, the shell, the disk): call it from a worker thread, and again when the pin folder changes.
pub fn read_pins() -> Vec<PinnedApp> {
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
                let Some(facts) = read_link(path) else { continue };
                if !alive(&facts) {
                    log::debug!("dock: pin {file} skipped, its target is gone");
                    continue;
                }
                out.push(pinned_from_link(path, &stem, facts));
            }
            Slot::Packaged(id) => {
                let Some(name) = packaged_name(&id) else {
                    log::debug!("dock: pin {id} skipped, the app is not installed");
                    continue;
                };
                out.push(PinnedApp { name, aumid: Some(id.to_lowercase()), exe: None, launch: Launch::Packaged(id.clone()), icon: IconSource::Packaged(id) });
            }
        }
    }
    out
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
