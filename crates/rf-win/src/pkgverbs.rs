//! The commands packaged apps add to Windows 11's new right-click menu (`IExplorerCommand`): "Open in Terminal", "Edit in Notepad",
//! "Edit with Notepad++", NanaZip, PowerToys, VS Code, WhatsApp's "Send with", ... Explorer shows them; the classic menu that
//! rustyfiles reads (`ctxmenu`) never had them.
//!
//! WHY (issue 107: "Edit with Notepad++ ... is one of the commands that newer apps add only to Windows 11's new right-click menu";
//! PR 123 added one such app by hand). An app declares them in its package manifest, `AppxManifest.xml`:
//!
//! ```xml
//! <desktop4:Extension Category="windows.fileExplorerContextMenus">
//!   <desktop4:FileExplorerContextMenus>
//!     <desktop5:ItemType Type="Directory">
//!       <desktop5:Verb Id="OpenTerminalHere" Clsid="9f156763-7844-4dc4-b2b1-901f640f5155" />
//! ```
//!
//! The class is a packaged COM class, which any process can create (Windows starts it in its package). Its `IExplorerCommand` gives
//! the title, the icon and whether it shows for the items (`GetState`), may have commands under it (`EnumSubCommands`), and runs
//! with `Invoke`. The packages of this account and their folders are listed under
//! `HKCU\Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages`
//! (`PackageRootFolder`); only the newest version of each package family is read.
//!
//! THREAD. Everything that touches a command runs on the shell STA (`crate::sta`): [`for_items`] keeps the commands it made for
//! the open menu there, and [`invoke`] runs one of them by its number.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{IExplorerCommand, IShellItemArray, ILCreateFromPathW, ILFree, SHCreateShellItemArrayFromIDLists};
use windows::core::{GUID, PCWSTR};

const PACKAGES: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";

/// One command an app declared: its class and the kinds of items it is for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declared {
    pub clsid: GUID,
    /// The manifest's `ItemType` values: `*` (every file), `.txt`, `Directory`, `Directory\Background`, `Drive`, ...
    pub types: Vec<String>,
}

// ------------------------------------------------------------------ pure: the manifest, the package names, the item kinds

/// The value of attribute `name` in the tag text `tag` (`Type="Directory"`).
fn attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let mut rest = tag;
    while let Some(i) = rest.find(name) {
        let before = rest[..i].chars().last();
        let after = &rest[i + name.len()..];
        if before.is_some_and(|c| c.is_whitespace()) && let Some(v) = after.trim_start().strip_prefix('=') {
            let v = v.trim_start();
            let q = v.chars().next()?;
            if q == '"' || q == '\'' {
                let v = &v[1..];
                return v.find(q).map(|end| &v[..end]);
            }
        }
        rest = after;
    }
    None
}

/// A tag's local name (`desktop5:Verb` -> `Verb`) and whether it closes (`</...>`).
fn tag_name(tag: &str) -> (&str, bool) {
    let closing = tag.starts_with('/');
    let t = tag.trim_start_matches('/');
    let name = t.split(|c: char| c.is_whitespace() || c == '/' || c == '>').next().unwrap_or("");
    (name.rsplit(':').next().unwrap_or(name), closing)
}

/// The commands of a manifest's `windows.fileExplorerContextMenus` extensions, one per class (a class declared for several item
/// kinds is one command with all of them).
pub fn parse_manifest(xml: &str) -> Vec<Declared> {
    let mut out: Vec<Declared> = Vec::new();
    let mut in_menus = false;
    let mut item_type: Option<String> = None;
    let mut rest = xml;
    while let Some(start) = rest.find('<') {
        let Some(len) = rest[start..].find('>') else { break };
        let tag = &rest[start + 1..start + len];
        rest = &rest[start + len + 1..];
        let (name, closing) = tag_name(tag);
        match (name, closing) {
            ("Extension", false) if attr(tag, "Category") == Some("windows.fileExplorerContextMenus") => in_menus = true,
            ("Extension", true) => {
                in_menus = false;
                item_type = None;
            }
            ("ItemType", false) if in_menus => item_type = attr(tag, "Type").map(str::to_string),
            ("ItemType", true) => item_type = None,
            ("Verb", false) if in_menus => {
                let (Some(t), Some(id)) = (item_type.clone(), attr(tag, "Clsid")) else { continue };
                let Ok(clsid) = GUID::try_from(id.trim_matches(|c| c == '{' || c == '}')) else { continue };
                match out.iter_mut().find(|d| d.clsid == clsid) {
                    Some(d) if !d.types.contains(&t) => d.types.push(t),
                    Some(_) => {}
                    None => out.push(Declared { clsid, types: vec![t] }),
                }
            }
            _ => {}
        }
    }
    out
}

/// A package's full name split: (family name, version). `Name_1.2.3.4_x64__publisherid` -> (`Name_publisherid`, [1, 2, 3, 4]).
pub fn family_and_version(full: &str) -> Option<(String, Vec<u32>)> {
    let parts: Vec<&str> = full.split('_').collect();
    if parts.len() != 5 {
        return None;
    }
    let version = parts[1].split('.').map(|n| n.parse().unwrap_or(0)).collect();
    Some((format!("{}_{}", parts[0], parts[4]), version))
}

/// The newest version of each package family among `full_names`.
pub fn newest_of_each(full_names: Vec<String>) -> Vec<String> {
    let mut best: Vec<(String, Vec<u32>, String)> = Vec::new();
    for f in full_names {
        let Some((family, version)) = family_and_version(&f) else { continue };
        match best.iter_mut().find(|(fam, ..)| *fam == family) {
            Some(b) if version > b.1 => *b = (family, version, f),
            Some(_) => {}
            None => best.push((family, version, f)),
        }
    }
    best.into_iter().map(|(.., f)| f).collect()
}

/// Whether a command declared for `types` is for these items: the folder's background, or every item (a file by `*` or its
/// extension, a folder by `Directory`, a drive's root by `Drive` or `Directory`).
pub fn applies(types: &[String], items: &[PathBuf], background: bool) -> bool {
    let has = |t: &str| types.iter().any(|x| x.eq_ignore_ascii_case(t));
    if background {
        return has("Directory\\Background");
    }
    !items.is_empty()
        && items.iter().all(|p| {
            let root = p.parent().is_none();
            if root {
                return has("Drive") || has("Directory");
            }
            if p.is_dir() {
                return has("Directory") || has("Folder");
            }
            has("*") || p.extension().is_some_and(|e| has(&format!(".{}", e.to_string_lossy())))
        })
}

// ------------------------------------------------------------------ the packages of this account

struct Cache {
    at: Instant,
    declared: Vec<Declared>,
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);
/// The packages are read again after this long (an app installed meanwhile shows up in the next menu after it).
const FRESH: Duration = Duration::from_secs(60);

/// Every command the newest packages of this account declare.
pub fn declared() -> Vec<Declared> {
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = c.as_ref().filter(|c| c.at.elapsed() < FRESH) {
        return c.declared.clone();
    }
    let declared = read_packages();
    *c = Some(Cache { at: Instant::now(), declared: declared.clone() });
    declared
}

fn read_packages() -> Vec<Declared> {
    let Ok(key) = windows_registry::CURRENT_USER.open(PACKAGES) else { return Vec::new() };
    let Ok(names) = key.keys() else { return Vec::new() };
    let mut out: Vec<Declared> = Vec::new();
    for full in newest_of_each(names.collect()) {
        let Ok(root) = key.open(&full).and_then(|k| k.get_string("PackageRootFolder")) else { continue };
        let Ok(xml) = std::fs::read_to_string(Path::new(&root).join("AppxManifest.xml")) else { continue };
        if !xml.contains("windows.fileExplorerContextMenus") {
            continue;
        }
        for d in parse_manifest(&xml) {
            if !out.iter().any(|o| o.clsid == d.clsid) {
                out.push(d);
            }
        }
    }
    out
}

// ------------------------------------------------------------------ the commands (shell STA)

/// One command as the menu shows it.
#[derive(Clone, Debug)]
pub struct Shown {
    /// The number [`invoke`] takes.
    pub id: usize,
    pub title: String,
    /// The icon's place as the command gives it (`C:\...\app.exe,-101`), when it gives a file.
    pub icon: Option<PathBuf>,
    pub enabled: bool,
    /// The commands under it (a submenu).
    pub sub: Vec<Shown>,
}

thread_local! {
    /// The commands of the menu that is open, by number, and the items they are for.
    static SESSION: RefCell<(Vec<IExplorerCommand>, Vec<PathBuf>)> = const { RefCell::new((Vec::new(), Vec::new())) };
}

/// The items as the shell's array (real item ids, so the command sees what Explorer would give it).
fn item_array(items: &[PathBuf]) -> windows::core::Result<IShellItemArray> {
    let mut ids: Vec<*mut ITEMIDLIST> = Vec::new();
    for p in items {
        let w = crate::wide::wide(p);
        let id = unsafe { ILCreateFromPathW(PCWSTR(w.as_ptr())) };
        if !id.is_null() {
            ids.push(id);
        }
    }
    let consts: Vec<*const ITEMIDLIST> = ids.iter().map(|p| *p as *const _).collect();
    let r = unsafe { SHCreateShellItemArrayFromIDLists(&consts) };
    for id in ids {
        unsafe { ILFree(Some(id)) };
    }
    r
}

fn take_string(p: windows::core::PWSTR) -> Option<String> {
    if p.is_null() {
        return None;
    }
    let s = unsafe { p.to_string() }.ok();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s.filter(|s| !s.is_empty())
}

/// The icon's file, when the command names one (`path,index` or a plain path; not a package resource).
fn icon_file(s: &str) -> Option<PathBuf> {
    let s = s.trim().trim_start_matches('@');
    let path = match s.rfind(',') {
        Some(i) if s[i + 1..].trim().parse::<i32>().is_ok() => &s[..i],
        _ => s,
    };
    let p = PathBuf::from(crate::localized::expand_env(path.trim().trim_matches('"')));
    (p.is_absolute() && p.exists()).then_some(p)
}

const ECS_HIDDEN: u32 = 0x2;
const ECS_DISABLED: u32 = 0x1;
const ECF_HASSUBCOMMANDS: u32 = 0x1;
const ECF_HIDELABEL: u32 = 0x2;
const ECF_ISSEPARATOR: u32 = 0x8;

/// What `cmd` shows for `array` (None: it hides itself, is a separator, or has no title), with its commands below, numbered into
/// `keep`.
fn shown(cmd: &IExplorerCommand, array: &IShellItemArray, keep: &mut Vec<IExplorerCommand>, depth: u8) -> Option<Shown> {
    let state = unsafe { cmd.GetState(array, true) }.unwrap_or(0);
    if state & ECS_HIDDEN != 0 {
        return None;
    }
    let flags = unsafe { cmd.GetFlags() }.unwrap_or(0);
    if flags & (ECF_ISSEPARATOR | ECF_HIDELABEL) != 0 {
        return None;
    }
    let title = take_string(unsafe { cmd.GetTitle(array) }.ok()?)?;
    let icon = unsafe { cmd.GetIcon(array) }.ok().and_then(take_string).and_then(|s| icon_file(&s));
    let mut sub = Vec::new();
    if flags & ECF_HASSUBCOMMANDS != 0 && depth < 2 && let Ok(e) = unsafe { cmd.EnumSubCommands() } {
        loop {
            let mut got = [None];
            let mut n = 0u32;
            if unsafe { e.Next(&mut got, Some(&mut n)) }.is_err() || n == 0 {
                break;
            }
            if let Some(c) = got[0].take() && let Some(s) = shown(&c, array, keep, depth + 1) {
                sub.push(s);
            }
        }
        if sub.is_empty() {
            return None;
        }
    }
    keep.push(cmd.clone());
    Some(Shown { id: keep.len() - 1, title, icon, enabled: state & ECS_DISABLED == 0, sub })
}

/// The packaged apps' commands for `items` (the folder itself with `background`), as the menu shows them. On the shell STA.
pub fn for_items(items: &[PathBuf], background: bool) -> Vec<Shown> {
    let wanted: Vec<GUID> = declared().into_iter().filter(|d| applies(&d.types, items, background)).map(|d| d.clsid).collect();
    let mut keep = Vec::new();
    let mut out = Vec::new();
    if !wanted.is_empty() && let Ok(array) = item_array(items) {
        for clsid in wanted {
            let made: windows::core::Result<IExplorerCommand> = unsafe { CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER | CLSCTX_LOCAL_SERVER) };
            match made {
                Ok(cmd) => out.extend(shown(&cmd, &array, &mut keep, 0)),
                Err(e) => log::debug!("packaged menu command {clsid:?}: {e}"),
            }
        }
    }
    SESSION.with(|s| *s.borrow_mut() = (keep, items.to_vec()));
    out
}

/// Runs command `id` of the menu [`for_items`] was last asked for. On the shell STA.
pub fn invoke(id: usize) -> anyhow::Result<()> {
    let (cmd, items) = SESSION.with(|s| {
        let s = s.borrow();
        (s.0.get(id).cloned(), s.1.clone())
    });
    let cmd = cmd.ok_or_else(|| anyhow::anyhow!("the menu's commands are gone"))?;
    let array = item_array(&items)?;
    unsafe { cmd.Invoke(&array, None) }?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TERMINAL: &str = r#"<Package><Extensions>
      <desktop4:Extension Category="windows.fileExplorerContextMenus">
        <desktop4:FileExplorerContextMenus>
          <desktop5:ItemType Type="Directory">
            <desktop5:Verb Id="OpenTerminalHere" Clsid="9f156763-7844-4dc4-b2b1-901f640f5155" />
          </desktop5:ItemType>
          <desktop5:ItemType Type="Directory\Background">
            <desktop5:Verb Id="OpenTerminalHere" Clsid="9f156763-7844-4dc4-b2b1-901f640f5155" />
          </desktop5:ItemType>
        </desktop4:FileExplorerContextMenus>
      </desktop4:Extension>
      <uap:Extension Category="windows.fileTypeAssociation"><uap:FileTypeAssociation Name="x"><desktop5:Verb Id="No" Clsid="11111111-1111-1111-1111-111111111111"/></uap:FileTypeAssociation></uap:Extension>
      <desktop4:Extension Category='windows.fileExplorerContextMenus'><desktop4:FileExplorerContextMenus>
          <desktop5:ItemType Type="*"><desktop5:Verb Id="Edit" Clsid="{B298D29A-A6ED-11DE-BA8C-A68E55D89593}"/></desktop5:ItemType>
      </desktop4:FileExplorerContextMenus></desktop4:Extension>
    </Extensions></Package>"#;

    #[test]
    fn a_manifests_menu_commands_are_read_with_the_kinds_they_are_for() {
        let d = parse_manifest(TERMINAL);
        assert_eq!(d.len(), 2, "{d:?}");
        assert_eq!(d[0].clsid, GUID::try_from("9f156763-7844-4dc4-b2b1-901f640f5155").unwrap());
        assert_eq!(d[0].types, ["Directory", "Directory\\Background"]);
        assert_eq!(d[1].types, ["*"], "single quotes and braces around the class are fine");
        assert!(!d.iter().any(|x| x.clsid == GUID::try_from("11111111-1111-1111-1111-111111111111").unwrap()), "only the menu extension counts");
    }

    #[test]
    fn only_the_newest_version_of_a_package_is_read() {
        let names = ["5319275A.WhatsAppDesktop_2.2644.101.0_x64__cv1g1gvanyjgm", "5319275A.WhatsAppDesktop_2.2645.101.0_x64__cv1g1gvanyjgm", "Microsoft.Paint_11.2609.441.0_x64__8wekyb3d8bbwe", "odd"];
        let newest = newest_of_each(names.map(String::from).to_vec());
        assert_eq!(newest, ["5319275A.WhatsAppDesktop_2.2645.101.0_x64__cv1g1gvanyjgm", "Microsoft.Paint_11.2609.441.0_x64__8wekyb3d8bbwe"]);
    }

    #[test]
    fn a_command_shows_for_the_items_it_was_declared_for() {
        let dir = std::env::temp_dir();
        let file = dir.join("rf-pkgverbs-test.TXT");
        std::fs::write(&file, b"x").unwrap();
        let t = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(applies(&t(&["*"]), std::slice::from_ref(&file), false));
        assert!(applies(&t(&[".txt"]), std::slice::from_ref(&file), false), "an extension, whatever its case");
        assert!(!applies(&t(&[".md"]), std::slice::from_ref(&file), false));
        assert!(!applies(&t(&["*"]), std::slice::from_ref(&dir), false), "a folder is not a file");
        assert!(applies(&t(&["Directory"]), std::slice::from_ref(&dir), false));
        assert!(applies(&t(&["Directory\\Background"]), &[], true));
        assert!(!applies(&t(&["Directory"]), &[], true), "the background is its own kind");
        assert!(!applies(&t(&["*"]), &[file.clone(), dir.clone()], false), "every item must fit");
        assert!(applies(&t(&["Drive"]), &[PathBuf::from("C:\\")], false));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn an_icon_is_a_file_when_it_names_one() {
        let exe = std::env::current_exe().unwrap();
        assert_eq!(icon_file(&format!("{},-101", exe.display())), Some(exe.clone()));
        assert_eq!(icon_file(&format!("\"{}\"", exe.display())), Some(exe));
        assert_eq!(icon_file("ms-resource://x/y"), None);
    }
}

#[cfg(test)]
mod manual {
    /// This account's real packages: the commands for a folder, a text file and a folder's background (titles only; nothing runs).
    #[test]
    #[ignore]
    fn manual_list_this_pcs_packaged_commands() {
        use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        let dir = std::env::temp_dir().join("rf-pkgverbs-manual");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("note.txt");
        std::fs::write(&file, b"x").unwrap();
        println!("declared: {}", super::declared().len());
        for (what, items, bg) in [("folder", vec![dir.clone()], false), ("text file", vec![file.clone()], false), ("background", vec![dir.clone()], true)] {
            let shown = super::for_items(&items, bg);
            let line: Vec<String> = shown.iter().map(|s| format!("{} (icon {}, {} below)", s.title, s.icon.is_some(), s.sub.len())).collect();
            println!("{what}: {line:?}");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}
