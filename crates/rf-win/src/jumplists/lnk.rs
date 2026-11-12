//! Shell links from memory: what a jump list's entry says (its title, program, arguments, icon, AppUserModelID), read with the shell's
//! own `IShellLink` loaded from a memory stream, and turned into a [`JumpItem`] by pure rules.
//!
//! [`read_links`] and [`read_documents`] use COM (an apartment thread that pumps messages): they run only inside
//! `rf_win::sta::call`, never on a worker. Nothing here resolves a link (`IShellLink::Resolve` can search the disk and show a
//! dialog) and nothing writes one.

use std::path::{Path, PathBuf};

use super::{JumpItem, JumpTarget};
use crate::dock::model::{IconSource, looks_packaged};

/// What one entry says, as read, before the rules decide what it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LinkFacts {
    /// The file system path the link (or shell item) points at; `None` for a target that is not a file (a packaged app, a control
    /// panel page).
    pub target: Option<PathBuf>,
    pub arguments: String,
    pub working_dir: Option<PathBuf>,
    /// The icon location with `%variables%` expanded, and its index.
    pub icon: Option<(PathBuf, i32)>,
    /// `System.Title`, the name an app gives its task or item.
    pub title: Option<String>,
    pub description: String,
    pub aumid: Option<String>,
    /// `System.AppUserModel.IsDestListSeparator`: the entry is a line, not an item.
    pub separator: bool,
}

/// What an entry is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// An app's task or an item of its own category: something to run.
    Task,
    /// A pinned or recent entry: a document (or folder) to open.
    Document,
}

const PROGRAM_EXTENSIONS: [&str; 7] = ["exe", "bat", "cmd", "com", "msc", "cpl", "scr"];

fn is_program(p: &Path) -> bool {
    p.extension().is_some_and(|e| PROGRAM_EXTENSIONS.iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// Whether the disk may be asked about a path without a long wait: only fixed local drives are asked. A network share, a removable
/// or optical drive, or an unknown root can block for seconds, so such a target is kept without a check. A drive letter that does not
/// exist at all is answered at once by Windows and its targets are dead.
pub trait Disk {
    fn exists(&self, p: &Path) -> bool;
}

/// The real disk: [`Disk::exists`] is `true` without asking for anything that is not on a fixed drive.
pub struct RealDisk;

impl Disk for RealDisk {
    fn exists(&self, p: &Path) -> bool {
        match drive_of(p) {
            Drive::Missing => false,
            Drive::Other => true,
            Drive::Fixed => match std::fs::metadata(p) {
                Ok(_) => true,
                // a path the user may not look at is not a dead one
                Err(e) => e.kind() == std::io::ErrorKind::PermissionDenied,
            },
        }
    }
}

/// What a path's root says about asking the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Drive {
    /// A fixed or RAM drive: asking is quick.
    Fixed,
    /// The letter is not a drive at all (`DRIVE_NO_ROOT_DIR`): nothing there can exist.
    Missing,
    /// A share, a removable or optical drive, an unknown type, or not a `X:\` path (relative, a `\\?\` prefix, a resource string): not asked.
    Other,
}

/// The classification of a `GetDriveTypeW` result: 3 = DRIVE_FIXED, 6 = DRIVE_RAMDISK, 1 = DRIVE_NO_ROOT_DIR.
fn drive_from_type(t: u32) -> Drive {
    match t {
        3 | 6 => Drive::Fixed,
        1 => Drive::Missing,
        _ => Drive::Other,
    }
}

fn drive_of(p: &Path) -> Drive {
    use windows::Win32::Storage::FileSystem::GetDriveTypeW;
    let s = p.to_string_lossy();
    let b = s.as_bytes();
    if b.len() < 3 || !b[0].is_ascii_alphabetic() || b[1] != b':' || b[2] != b'\\' {
        return Drive::Other;
    }
    let root: Vec<u16> = format!("{}:\\", b[0] as char).encode_utf16().chain(std::iter::once(0)).collect();
    drive_from_type(unsafe { GetDriveTypeW(windows::core::PCWSTR(root.as_ptr())) })
}

/// Why an entry gives no item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dropped {
    /// Nothing to show or run: no target and no AppUserModelID (a library, a Control Panel page), or no title to give it.
    NothingToOpen,
    /// The target is gone.
    TargetGone,
}

/// Turns what an entry says into an item, or `None` for a dead one (its target is gone) or one with nothing to show or run.
pub fn to_item(f: &LinkFacts, role: Role, disk: &dyn Disk) -> Option<JumpItem> {
    classify(f, role, disk).ok()
}

/// [`to_item`] saying why an entry was dropped.
pub fn classify(f: &LinkFacts, role: Role, disk: &dyn Disk) -> Result<JumpItem, Dropped> {
    if f.separator {
        return Ok(JumpItem { title: String::new(), icon: None, target: JumpTarget::Separator });
    }
    let args = f.arguments.trim();
    let target = f.target.as_deref().filter(|p| !p.as_os_str().is_empty());
    let runs = match (role, target) {
        (Role::Task, _) => true,
        (Role::Document, Some(t)) => is_program(t) || !args.is_empty(),
        (Role::Document, None) => f.aumid.is_some(),
    };
    // an icon location that is a resource string of a package ("@{...?ms-resource:...}") names no file: the program's icon stands in
    let own_icon = f
        .icon
        .as_ref()
        .filter(|(p, _)| !p.as_os_str().is_empty() && !p.to_string_lossy().starts_with('@') && disk.exists(p))
        .map(|(p, i)| IconSource::Resource { file: p.clone(), index: *i });
    if runs {
        // a task needs a program (or a packaged app) to run
        if let Some(t) = target
            && !disk.exists(t)
        {
            return Err(Dropped::TargetGone);
        }
        if target.is_none() && f.aumid.is_none() {
            return Err(Dropped::NothingToOpen);
        }
        let title = f
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .or_else(|| Some(f.description.clone()).filter(|d| !d.trim().is_empty()))
            .or_else(|| target.and_then(|t| t.file_stem()).map(|s| s.to_string_lossy().into_owned()))
            .or_else(|| f.aumid.clone())
            .ok_or(Dropped::NothingToOpen)?;
        let packaged_icon = f.aumid.as_deref().filter(|a| target.is_none() && looks_packaged(a)).map(|a| IconSource::Packaged(a.to_string()));
        let icon = own_icon.or(packaged_icon).or_else(|| target.map(|t| IconSource::File(t.to_path_buf())));
        return Ok(JumpItem {
            title,
            icon,
            target: JumpTarget::Link {
                program: target.map(Path::to_path_buf).unwrap_or_default(),
                arguments: args.to_string(),
                working_dir: f.working_dir.clone().filter(|d| !d.as_os_str().is_empty()),
                aumid: f.aumid.clone(),
            },
        });
    }
    // a document or a folder
    let t = target.ok_or(Dropped::NothingToOpen)?;
    if !disk.exists(t) {
        return Err(Dropped::TargetGone);
    }
    let title = f.title.clone().filter(|t| !t.trim().is_empty()).or_else(|| t.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| t.to_string_lossy().into_owned());
    Ok(JumpItem { title, icon: own_icon.or_else(|| Some(IconSource::File(t.to_path_buf()))), target: JumpTarget::File(t.to_path_buf()) })
}

// ---------------------------------------------------------------------------------------------------------------------------
// COM: only on the shell thread

fn pwstr_to_string(buf: &[u16]) -> String {
    crate::wide::from_wide(buf)
}

fn expand_vars(s: &str) -> String {
    crate::dock::pins::expand_vars(s, |n| std::env::var(n).ok())
}

fn prop_string(store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore, key: &windows::Win32::Foundation::PROPERTYKEY) -> Option<String> {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    unsafe {
        let v = store.GetValue(key).ok()?;
        let p = PropVariantToStringAlloc(&v).ok()?;
        let s = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        s.filter(|s| !s.is_empty())
    }
}

fn prop_bool(store: &windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore, key: &windows::Win32::Foundation::PROPERTYKEY) -> bool {
    use windows::Win32::System::Com::StructuredStorage::PropVariantToBooleanWithDefault;
    unsafe { store.GetValue(key).map(|v| PropVariantToBooleanWithDefault(&v, false).as_bool()).unwrap_or(false) }
}

/// What a loaded shell link says.
fn facts_of(link: &windows::Win32::UI::Shell::IShellLinkW) -> LinkFacts {
    use windows::Win32::Storage::EnhancedStorage::{PKEY_AppUserModel_ID, PKEY_AppUserModel_IsDestListSeparator, PKEY_Title};
    use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
    use windows::Win32::UI::Shell::PropertiesSystem::IPropertyStore;
    use windows::core::Interface;
    let mut f = LinkFacts::default();
    unsafe {
        let mut buf = vec![0u16; 4096];
        let mut fd = WIN32_FIND_DATAW::default();
        if link.GetPath(&mut buf, &mut fd, 0).is_ok() {
            let p = pwstr_to_string(&buf);
            if !p.is_empty() {
                f.target = Some(PathBuf::from(p));
            }
        }
        buf.fill(0);
        let _ = link.GetArguments(&mut buf);
        f.arguments = pwstr_to_string(&buf);
        buf.fill(0);
        let _ = link.GetWorkingDirectory(&mut buf);
        let wd = expand_vars(&pwstr_to_string(&buf));
        f.working_dir = (!wd.is_empty()).then(|| PathBuf::from(wd));
        buf.fill(0);
        let _ = link.GetDescription(&mut buf);
        f.description = pwstr_to_string(&buf);
        buf.fill(0);
        let mut index = 0i32;
        let _ = link.GetIconLocation(&mut buf, &mut index);
        let icon = expand_vars(&pwstr_to_string(&buf));
        f.icon = (!icon.is_empty()).then(|| (PathBuf::from(icon), index));
        if let Ok(store) = link.cast::<IPropertyStore>() {
            f.title = prop_string(&store, &PKEY_Title);
            f.aumid = prop_string(&store, &PKEY_AppUserModel_ID);
            f.separator = prop_bool(&store, &PKEY_AppUserModel_IsDestListSeparator);
        }
    }
    f
}

/// Loads each blob as a shell link from memory and reads what it says. A blob that is not a link gives `None`. COM, on the shell thread.
pub fn read_links(blobs: &[Vec<u8>]) -> Vec<Option<LinkFacts>> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, IPersistStream};
    use windows::Win32::UI::Shell::{IShellLinkW, SHCreateMemStream, ShellLink};
    use windows::core::Interface;
    crate::shell::ensure_com();
    blobs
        .iter()
        .map(|b| unsafe {
            // the shell would accept an empty stream as an empty link; only something with a link's header is one
            if b.len() < 0x4C || b[..4] != [0x4C, 0, 0, 0] || b[4..20] != super::custom::LINK_CLSID {
                return None;
            }
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
            let persist: IPersistStream = link.cast().ok()?;
            let stream = SHCreateMemStream(Some(b))?;
            persist.Load(&stream).ok()?;
            Some(facts_of(&link))
        })
        .collect()
}

/// Which of Windows' automatic lists to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoList {
    Recent,
    Frequent,
}

/// The documents Windows lists for an app (`IApplicationDocumentLists`, in-process). `Err` when the object or the list cannot be had,
/// an empty list when the app has none. COM, on the shell thread.
pub fn read_documents(app_id: &str, which: AutoList, wanted: u32) -> anyhow::Result<Vec<LinkFacts>> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::Common::IObjectArray;
    use windows::Win32::UI::Shell::{ADLT_FREQUENT, ADLT_RECENT, ApplicationDocumentLists, IApplicationDocumentLists, IShellItem, IShellLinkW, SIGDN_FILESYSPATH, SIGDN_NORMALDISPLAY};
    use windows::core::PCWSTR;
    crate::shell::ensure_com();
    let mut out = Vec::new();
    unsafe {
        let lists: IApplicationDocumentLists = CoCreateInstance(&ApplicationDocumentLists, None, CLSCTX_INPROC_SERVER)?;
        let id = crate::wide::wide(app_id);
        lists.SetAppID(PCWSTR(id.as_ptr()))?;
        let array: IObjectArray = lists.GetList(if which == AutoList::Recent { ADLT_RECENT } else { ADLT_FREQUENT }, wanted)?;
        let n = array.GetCount()?;
        for i in 0..n.min(wanted) {
            if let Ok(link) = array.GetAt::<IShellLinkW>(i) {
                out.push(facts_of(&link));
                continue;
            }
            if let Ok(item) = array.GetAt::<IShellItem>(i) {
                let name = |d| {
                    item.GetDisplayName(d).ok().and_then(|p| {
                        let s = p.to_string().ok();
                        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
                        s
                    })
                };
                out.push(LinkFacts { target: name(SIGDN_FILESYSPATH).map(PathBuf::from), title: name(SIGDN_NORMALDISPLAY), ..Default::default() });
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    struct FakeDisk(HashSet<PathBuf>);
    impl Disk for FakeDisk {
        fn exists(&self, p: &Path) -> bool {
            self.0.contains(p)
        }
    }
    fn disk(paths: &[&str]) -> FakeDisk {
        FakeDisk(paths.iter().map(PathBuf::from).collect())
    }

    #[test]
    fn a_recent_document_is_a_file_titled_by_its_name_with_its_own_icon() {
        let f = LinkFacts { target: Some(r"C:\docs\report.docx".into()), ..Default::default() };
        let it = to_item(&f, Role::Document, &disk(&[r"C:\docs\report.docx"])).unwrap();
        assert_eq!(it.title, "report.docx");
        assert_eq!(it.target, JumpTarget::File(r"C:\docs\report.docx".into()));
        assert_eq!(it.icon, Some(IconSource::File(r"C:\docs\report.docx".into())));
    }

    #[test]
    fn a_document_whose_file_is_gone_is_dropped_and_a_folder_is_a_file_target_too() {
        let f = LinkFacts { target: Some(r"C:\gone.txt".into()), ..Default::default() };
        assert_eq!(to_item(&f, Role::Document, &disk(&[])), None);
        let d = LinkFacts { target: Some(r"C:\Users\x\Downloads".into()), title: Some("Downloads".into()), ..Default::default() };
        let it = to_item(&d, Role::Document, &disk(&[r"C:\Users\x\Downloads"])).unwrap();
        assert_eq!(it.title, "Downloads");
        assert!(matches!(it.target, JumpTarget::File(_)));
    }

    #[test]
    fn a_task_is_a_link_with_its_arguments_its_title_and_its_icon_location() {
        let f = LinkFacts {
            target: Some(r"C:\Apps\app.exe".into()),
            arguments: "  --new-window  ".into(),
            working_dir: Some(r"C:\Apps".into()),
            icon: Some((r"C:\Apps\icons.dll".into(), 3)),
            title: Some("Yeni pencere".into()),
            ..Default::default()
        };
        let it = to_item(&f, Role::Task, &disk(&[r"C:\Apps\app.exe", r"C:\Apps\icons.dll"])).unwrap();
        assert_eq!(it.title, "Yeni pencere");
        assert_eq!(it.icon, Some(IconSource::Resource { file: r"C:\Apps\icons.dll".into(), index: 3 }));
        assert_eq!(it.target, JumpTarget::Link { program: r"C:\Apps\app.exe".into(), arguments: "--new-window".into(), working_dir: Some(r"C:\Apps".into()), aumid: None });
        // the icon file is gone: the program's own icon stands in
        let it = to_item(&f, Role::Task, &disk(&[r"C:\Apps\app.exe"])).unwrap();
        assert_eq!(it.icon, Some(IconSource::File(r"C:\Apps\app.exe".into())));
        // the program is gone: a dead task
        assert_eq!(to_item(&f, Role::Task, &disk(&[])), None);
    }

    #[test]
    fn a_document_entry_that_is_a_program_or_has_arguments_is_still_something_to_run() {
        let exe = LinkFacts { target: Some(r"C:\Tools\t.exe".into()), ..Default::default() };
        assert!(matches!(to_item(&exe, Role::Document, &disk(&[r"C:\Tools\t.exe"])).unwrap().target, JumpTarget::Link { .. }));
        let with_args = LinkFacts { target: Some(r"C:\Tools\notes.txt".into()), arguments: "x".into(), ..Default::default() };
        assert!(matches!(to_item(&with_args, Role::Document, &disk(&[r"C:\Tools\notes.txt"])).unwrap().target, JumpTarget::Link { .. }));
    }

    #[test]
    fn a_packaged_app_s_task_has_no_program_and_its_package_logo_for_an_icon() {
        let id = "Vendor.App_abc123!App";
        let f = LinkFacts { aumid: Some(id.into()), title: Some("New chat".into()), ..Default::default() };
        let it = to_item(&f, Role::Task, &disk(&[])).unwrap();
        assert_eq!(it.icon, Some(IconSource::Packaged(id.into())));
        assert_eq!(it.target, JumpTarget::Link { program: PathBuf::new(), arguments: String::new(), working_dir: None, aumid: Some(id.into()) });
        // nothing to run at all
        assert_eq!(to_item(&LinkFacts { title: Some("x".into()), ..Default::default() }, Role::Task, &disk(&[])), None);
    }

    #[test]
    fn a_separator_is_kept_whatever_else_it_says_and_a_missing_title_falls_back() {
        let s = LinkFacts { separator: true, ..Default::default() };
        let it = to_item(&s, Role::Task, &disk(&[])).unwrap();
        assert_eq!((it.title.as_str(), &it.target, &it.icon), ("", &JumpTarget::Separator, &None));
        let no_title = LinkFacts { target: Some(r"C:\Apps\my tool.exe".into()), description: "Open my tool".into(), ..Default::default() };
        assert_eq!(to_item(&no_title, Role::Task, &disk(&[r"C:\Apps\my tool.exe"])).unwrap().title, "Open my tool");
        let bare = LinkFacts { target: Some(r"C:\Apps\my tool.exe".into()), ..Default::default() };
        assert_eq!(to_item(&bare, Role::Task, &disk(&[r"C:\Apps\my tool.exe"])).unwrap().title, "my tool");
    }

    #[test]
    fn the_real_disk_does_not_ask_about_what_is_not_on_a_fixed_drive() {
        // a share and a path with no drive are kept without a check (they could block for seconds)
        assert!(RealDisk.exists(Path::new(r"\\no-such-server\share\file.txt")));
        assert!(RealDisk.exists(Path::new("relative.txt")));
        // a fixed drive is asked
        let here = std::env::current_exe().unwrap();
        assert!(RealDisk.exists(&here));
        assert!(!RealDisk.exists(&here.with_extension("definitely-not-here")));
    }

    #[test]
    fn a_drive_letter_that_is_not_a_drive_is_dead_but_removable_and_unknown_drives_are_not_asked() {
        assert_eq!(drive_from_type(3), Drive::Fixed);
        assert_eq!(drive_from_type(6), Drive::Fixed);
        assert_eq!(drive_from_type(1), Drive::Missing, "DRIVE_NO_ROOT_DIR");
        for t in [0, 2, 4, 5] {
            assert_eq!(drive_from_type(t), Drive::Other, "type {t}: unknown, removable, remote, optical");
        }
        // a letter this machine has no drive for: answered at once by Windows, and a target there is gone
        let used = unsafe { windows::Win32::Storage::FileSystem::GetLogicalDrives() };
        if let Some(free) = (b'A'..=b'Z').rev().find(|l| used & (1 << (l - b'A')) == 0) {
            let p = PathBuf::from(format!("{}:\\no\\such\\file.txt", free as char));
            assert_eq!(drive_of(&p), Drive::Missing);
            assert!(!RealDisk.exists(&p));
        }
    }

    #[test]
    fn a_package_resource_string_is_not_an_icon_file_and_the_reason_for_a_drop_is_told() {
        let f = LinkFacts { target: Some(r"C:\Apps\app.exe".into()), icon: Some((r"@{Vendor.App_1.0_x64__abc?ms-resource://Vendor.App/Files/logo.png}".into(), 0)), ..Default::default() };
        // even a disk that claims every path exists does not make the string a file
        struct Everything;
        impl Disk for Everything {
            fn exists(&self, _: &Path) -> bool {
                true
            }
        }
        let it = to_item(&f, Role::Task, &Everything).unwrap();
        assert_eq!(it.icon, Some(IconSource::File(r"C:\Apps\app.exe".into())));
        assert_eq!(classify(&f, Role::Task, &disk(&[])), Err(Dropped::TargetGone));
        let page = LinkFacts { title: Some("A control panel page".into()), ..Default::default() };
        assert_eq!(classify(&page, Role::Document, &disk(&[])), Err(Dropped::NothingToOpen));
        assert_eq!(classify(&page, Role::Task, &disk(&[])), Err(Dropped::NothingToOpen));
        let gone = LinkFacts { target: Some(r"C:\gone.txt".into()), ..Default::default() };
        assert_eq!(classify(&gone, Role::Document, &disk(&[])), Err(Dropped::TargetGone));
    }
}
