//! Loads a dock icon from the best source for the size it is really drawn at, and returns it as the set of pictures the dock keeps
//! (`iconpick::level_sizes`), every one made by our own resampling (`resample`).
//!
//! THE SOURCES, best first for an item whose chain (`DockApp::icon_chain`) holds several:
//! 1. a packaged app's own logo FILES: the package's manifest names the logo, the folder holds its sizes (`scale-` and `targetsize-`
//!    variants, the unplated form); [`iconpick::choose_asset`] takes the smallest that has enough pixels. (The shell's route for a
//!    package, `shell:AppsFolder\id`, scales whichever file it likes to the size asked and is the fallback.)
//! 2. a program's or library's icon, FRAME by frame: the group directory lists the sizes, [`iconpick::choose_frame`] takes the
//!    smallest that is big enough (the 256 px frame above 48 px), and that frame is decoded as it is. (The shell, asked for 69 px, gives
//!    a frame it has already scaled.) A `.ico` file likewise. Used when more than 48 px are needed; up to 48 the shell's answer is a
//!    frame as it is and stays what it was.
//! 3. the shell, for places and for everything the above cannot read.
//! 4. the icon a window has set itself, LAST: it is the 32 px `WM_GETICON` picture of a program that may have a big one in its file.
//!
//! Blocking (the disk, the shell): a worker thread only.

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{EnumResourceNamesW, FindResourceW, LOAD_LIBRARY_AS_DATAFILE, LoadLibraryExW, LoadResource, LockResource, SizeofResource};
use windows::Win32::UI::WindowsAndMessaging::{CreateIconFromResourceEx, DestroyIcon, IMAGE_FLAGS, RT_GROUP_ICON, RT_ICON};
use windows::core::{BOOL, PCWSTR};

use super::iconpick::{self, AssetVariant, FrameInfo};
use super::model::IconSource;
use crate::thumbs::Bgra;

/// Up to this many device px the shell's answer for a program is used as it always was (its frames are 16, 24, 32, 48 px).
const SHELL_OK_PX: u32 = 48;

/// What a picture came from; the dock does not care, the probe counts them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Used {
    /// A logo file of the app's package.
    PackageAsset,
    /// One frame of the icon group of a program or library, chosen by us.
    ProgramFrame,
    /// An `.ico` file.
    IconFile,
    /// The shell's own answer (a place, a package it was left to, a program up to 48 px).
    Shell,
    /// `PrivateExtractIconsW` (a shortcut's icon location, up to 48 px).
    ResourceApi,
    /// What a running window has set itself.
    WindowIcon,
}

/// The pictures of one icon, smallest first (the sizes asked for), and where they came from.
pub struct Loaded {
    pub levels: Vec<Bgra>,
    pub used: Used,
    /// The side, in px, of the picture they were made from. For a frame, a logo file or a window's icon this is its own size; for the
    /// shell's answer it is the size the shell returned, which may itself have been scaled by the shell.
    pub native_px: u32,
    /// Which of the chain's sources gave it (an index into the slice given to [`load_levels`]).
    pub source: usize,
}

/// The order a chain is tried in: a package's logo first, then the item's own icon in the order given, the windows' icons last.
pub fn rank(srcs: &[IconSource]) -> Vec<usize> {
    let class = |s: &IconSource| match s {
        IconSource::Packaged(_) => 0,
        IconSource::Window(_) => 2,
        _ => 1,
    };
    let mut order: Vec<usize> = (0..srcs.len()).collect();
    order.sort_by_key(|&i| class(&srcs[i]));
    order
}

/// Loads the icon of the first source of `srcs` (in [`rank`] order) that gives a picture, for the biggest of `sizes` (ascending), and
/// returns one picture per size.
pub fn load_levels(srcs: &[IconSource], sizes: &[u32]) -> anyhow::Result<Loaded> {
    load_levels_with(srcs, sizes, fetch)
}

/// [`load_levels`] with the reading of one source given (a test passes its own, so the order the sources are tried in is checked
/// without a window, a disk or the shell).
fn load_levels_with(srcs: &[IconSource], sizes: &[u32], get: impl Fn(&IconSource, u32) -> anyhow::Result<(Bgra, Used)>) -> anyhow::Result<Loaded> {
    let need = sizes.iter().copied().max().ok_or_else(|| anyhow::anyhow!("no size asked for"))?;
    let mut first_error = None;
    for i in rank(srcs) {
        match get(&srcs[i], need) {
            Ok((raw, used)) => {
                let native_px = raw.width.max(raw.height);
                return Ok(Loaded { levels: super::resample::levels(&raw, sizes), used, native_px, source: i });
            }
            Err(e) => {
                log::debug!("dock icon: {:?} failed: {e:#}", srcs[i]);
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| anyhow::anyhow!("no icon source")))
}

/// One picture at exactly `px`, from the best source (the Start panel's tiles and the app menu, which draw an icon at one size).
pub fn load_one(srcs: &[IconSource], px: u32) -> anyhow::Result<Bgra> {
    Ok(load_levels(srcs, &[px])?.levels.remove(0))
}

/// What to ask the shell for when an icon needs `need` px: above 48 px the biggest it has (256), so that the only reduction the picture
/// goes through is ours (`resample`); the shell's own, from its next size up, would be a first filtering by a filter we do not know.
/// Up to 48 px it is asked for as it always was.
fn shell_ask(need: u32) -> u32 {
    if need > SHELL_OK_PX { 256 } else { need }
}

/// The best picture one source has for an icon that needs `need` px, as it is stored (not resampled).
pub fn fetch(src: &IconSource, need: u32) -> anyhow::Result<(Bgra, Used)> {
    let shell = |used: Used| super::icons::raw_icon(src, shell_ask(need)).map(|b| (b, used));
    match src {
        IconSource::Packaged(id) => match package_logo(id, need) {
            Some(logo) => Ok((logo.bgra, Used::PackageAsset)),
            None => shell(Used::Shell),
        },
        IconSource::File(p) if need > SHELL_OK_PX => match from_file(p, 0, need) {
            Some(r) => Ok(r),
            None => shell(Used::Shell),
        },
        IconSource::Resource { file, index } if need > SHELL_OK_PX => match from_file(file, *index, need) {
            Some(r) => Ok(r),
            None => shell(Used::Shell),
        },
        IconSource::Resource { .. } => shell(Used::ResourceApi),
        IconSource::Window(_) => shell(Used::WindowIcon),
        IconSource::File(_) | IconSource::Shell(_) => shell(Used::Shell),
    }
}

/// What [`fetch`] would use for `src` and how many px of the picture's own detail it has, asked WITHOUT the shell: `None` when only the
/// shell could answer (a place, a program up to 48 px, a package whose files cannot be read). The probe counts with it, because the
/// shell route needs the shell thread's window and is not worth starting to count. The size is the picture's own side.
pub fn fetch_without_shell(src: &IconSource, need: u32) -> Option<(Used, u32)> {
    match src {
        IconSource::Packaged(id) => package_logo(id, need).map(|l| (Used::PackageAsset, l.bgra.width)),
        IconSource::File(p) if need > SHELL_OK_PX => from_file(p, 0, need).map(|(b, u)| (u, b.width.max(b.height))),
        IconSource::Resource { file, index } if need > SHELL_OK_PX => from_file(file, *index, need).map(|(b, u)| (u, b.width.max(b.height))),
        IconSource::Window(_) => super::icons::raw_icon(src, need).ok().map(|b| (Used::WindowIcon, b.width.max(b.height))),
        _ => None,
    }
}

fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// The icon of a program, a library or an `.ico` file at the frame that fits `need`; `None` when the file has no icon of its own that
/// we can read (the shell is asked then).
fn from_file(p: &Path, index: i32, need: u32) -> Option<(Bgra, Used)> {
    match ext_of(p).as_str() {
        "ico" => ico_file(p).map(|b| (b, Used::IconFile)),
        "exe" | "dll" | "scr" | "cpl" | "ocx" | "mui" => program_frame(p, index, need).map(|b| (b, Used::ProgramFrame)),
        _ => None,
    }
}

fn ico_file(p: &Path) -> Option<Bgra> {
    // (the decoder takes the biggest frame of the file)
    let img = image::ImageReader::open(p).ok()?.with_guessed_format().ok()?.decode().ok()?;
    Some(rgba_to_bgra(img.to_rgba8()))
}

fn rgba_to_bgra(img: image::RgbaImage) -> Bgra {
    let (width, height) = img.dimensions();
    let mut data = img.into_raw();
    data.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p.swap(0, 2));
    Bgra { width, height, data }
}

// ---- the frames of a program

/// The name of a resource: a number or a string.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ResName {
    Id(u16),
    Text(Vec<u16>),
}

impl ResName {
    fn pcwstr(&self, text: &mut Vec<u16>) -> PCWSTR {
        match self {
            ResName::Id(id) => PCWSTR(*id as usize as *const u16),
            ResName::Text(t) => {
                text.clear();
                text.extend_from_slice(t);
                text.push(0);
                PCWSTR(text.as_ptr())
            }
        }
    }
}

unsafe extern "system" fn collect_name(_: HMODULE, _: PCWSTR, name: PCWSTR, lparam: isize) -> BOOL {
    // SAFETY: `lparam` is the address of the vector `group_names` passes, alive for the whole enumeration.
    let names = unsafe { &mut *(lparam as *mut Vec<ResName>) };
    let raw = name.0 as usize;
    names.push(if raw >> 16 == 0 { ResName::Id(raw as u16) } else { ResName::Text(unsafe { name.as_wide() }.to_vec()) });
    BOOL(1)
}

fn group_names(module: HMODULE) -> Vec<ResName> {
    let mut names: Vec<ResName> = Vec::new();
    unsafe {
        let _ = EnumResourceNamesW(Some(module), RT_GROUP_ICON, Some(collect_name), &mut names as *mut Vec<ResName> as isize);
    }
    names
}

/// The bytes of a resource of `kind` named `name`, copied out of the mapped file.
fn resource_bytes(module: HMODULE, name: &ResName, kind: PCWSTR) -> Option<Vec<u8>> {
    let mut text = Vec::new();
    unsafe {
        let res = FindResourceW(Some(module), name.pcwstr(&mut text), kind);
        if res.is_invalid() {
            return None;
        }
        let size = SizeofResource(Some(module), res) as usize;
        let global = LoadResource(Some(module), res).ok()?;
        let ptr = LockResource(global) as *const u8;
        (!ptr.is_null() && size > 0 && size < 64 << 20).then(|| std::slice::from_raw_parts(ptr, size).to_vec())
    }
}

/// One entry of a group directory (`GRPICONDIRENTRY`): the frame's description and the id of its `RT_ICON` resource.
struct GroupEntry {
    info: FrameInfo,
    id: u16,
}

/// The entries of a group directory (`GRPICONDIR`: a 6-byte header, then 14 bytes each). Pure.
fn parse_group(bytes: &[u8]) -> Vec<GroupEntry> {
    if bytes.len() < 6 || u16::from_le_bytes([bytes[2], bytes[3]]) != 1 {
        return Vec::new();
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    (0..count)
        .filter_map(|i| {
            let e = bytes.get(6 + i * 14..6 + i * 14 + 14)?;
            Some(GroupEntry {
                info: FrameInfo { width: e[0] as u32, height: e[1] as u32, bpp: u16::from_le_bytes([e[6], e[7]]) },
                id: u16::from_le_bytes([e[12], e[13]]),
            })
        })
        .collect()
}

/// Which group of the file `index` means, as `ExtractIcon` reads it: a number from 0 is the position in the file's order, a negative
/// one is minus a resource id.
fn pick_group(names: &[ResName], index: i32) -> Option<ResName> {
    if index >= 0 {
        names.get(index as usize).cloned()
    } else {
        let id = index.unsigned_abs();
        u16::try_from(id).ok().map(ResName::Id).filter(|n| names.contains(n))
    }
}

/// The frame sizes of the icon at `index` of a program or library (the probe's question: what could the dock draw from this file).
pub fn program_frames(p: &Path, index: i32) -> Option<Vec<FrameInfo>> {
    with_module(p, |module| {
        let group = pick_group(&group_names(module), index)?;
        Some(parse_group(&resource_bytes(module, &group, RT_GROUP_ICON)?).into_iter().map(|e| e.info).collect())
    })
}

fn with_module<T>(p: &Path, f: impl FnOnce(HMODULE) -> Option<T>) -> Option<T> {
    let w = crate::wide::wide(p);
    let module = unsafe { LoadLibraryExW(PCWSTR(w.as_ptr()), None, LOAD_LIBRARY_AS_DATAFILE) }.ok()?;
    let r = f(module);
    unsafe {
        let _ = FreeLibrary(module);
    }
    r
}

/// The frame of a program's icon that fits `need`, decoded at its own size.
fn program_frame(p: &Path, index: i32, need: u32) -> Option<Bgra> {
    with_module(p, |module| {
        let group = pick_group(&group_names(module), index)?;
        let entries = parse_group(&resource_bytes(module, &group, RT_GROUP_ICON)?);
        let infos: Vec<FrameInfo> = entries.iter().map(|e| e.info).collect();
        let chosen = &entries[iconpick::choose_frame(&infos, need)?];
        let bytes = resource_bytes(module, &ResName::Id(chosen.id), RT_ICON)?;
        decode_frame(&bytes, chosen.info.side())
    })
}

/// A frame as stored in an `RT_ICON` resource: a PNG (the 256 px frames of recent programs) or a DIB with its mask.
fn decode_frame(bytes: &[u8], side: u32) -> Option<Bgra> {
    if bytes.starts_with(b"\x89PNG") {
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).ok()?;
        return Some(rgba_to_bgra(img.to_rgba8()));
    }
    unsafe {
        let icon = CreateIconFromResourceEx(bytes, true, 0x0003_0000, side as i32, side as i32, IMAGE_FLAGS(0)).ok()?;
        let r = super::icons::hicon_to_bgra(icon);
        let _ = DestroyIcon(icon);
        r
    }
}

// ---- a packaged app's logo files

// kernel32 has had these since Windows 8, the same way as `identity`'s call: the `windows` crate feature that wraps them is not
// enabled here. Both only read.
unsafe extern "system" {
    fn GetPackagesByPackageFamily(family: *const u16, count: *mut u32, full_names: *mut *mut u16, buffer_len: *mut u32, buffer: *mut u16) -> u32;
    fn GetPackagePathByFullName(full_name: *const u16, len: *mut u32, path: *mut u16) -> u32;
}

const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

fn wide_nul(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The folder a package family is installed in (its newest installed version), or `None` for an app that is not packaged or cannot
/// be asked.
pub fn package_dir(family: &str) -> Option<PathBuf> {
    let family = wide_nul(family);
    let (mut count, mut buf_len) = (0u32, 0u32);
    let r = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, std::ptr::null_mut(), &mut buf_len, std::ptr::null_mut()) };
    if r != ERROR_INSUFFICIENT_BUFFER || count == 0 || buf_len == 0 || count > 64 || buf_len > 1 << 16 {
        return None;
    }
    let mut names: Vec<*mut u16> = vec![std::ptr::null_mut(); count as usize];
    let mut buffer = vec![0u16; buf_len as usize];
    let r = unsafe { GetPackagesByPackageFamily(family.as_ptr(), &mut count, names.as_mut_ptr(), &mut buf_len, buffer.as_mut_ptr()) };
    if r != 0 {
        return None;
    }
    let full: Vec<String> = names
        .iter()
        .take(count as usize)
        .filter(|p| !p.is_null())
        // SAFETY: each pointer is into `buffer`, which is NUL separated and alive here.
        .map(|&p| {
            let s = PCWSTR(p);
            String::from_utf16_lossy(unsafe { s.as_wide() })
        })
        .collect();
    let full = &full[iconpick::newest_package(&full)?];
    let wide = wide_nul(full);
    let mut len = 0u32;
    let r = unsafe { GetPackagePathByFullName(wide.as_ptr(), &mut len, std::ptr::null_mut()) };
    if r != ERROR_INSUFFICIENT_BUFFER || len == 0 || len > 1 << 15 {
        return None;
    }
    let mut path = vec![0u16; len as usize];
    let r = unsafe { GetPackagePathByFullName(wide.as_ptr(), &mut len, path.as_mut_ptr()) };
    (r == 0 && len > 1).then(|| PathBuf::from(String::from_utf16_lossy(&path[..(len - 1) as usize])))
}

/// What the dock found of a package's logo files.
pub struct PackageAssets {
    pub dir: PathBuf,
    /// The folder the logo files are in.
    pub assets: PathBuf,
    pub files: Vec<(PathBuf, AssetVariant)>,
}

/// The logo files of the application `aumid` names (`Family!AppId`): found through the installed package's manifest. `None` when the
/// package, its manifest or its folder cannot be read.
pub fn package_assets(aumid: &str) -> Option<PackageAssets> {
    let (family, app_id) = aumid.split_once('!')?;
    let dir = package_dir(family)?;
    let xml = std::fs::read_to_string(dir.join("AppxManifest.xml")).ok()?;
    let logo = iconpick::manifest_logo(&xml, app_id)?;
    let (folder, stem, ext) = iconpick::split_logo_path(&logo.relative)?;
    let assets = if folder.is_empty() { dir.clone() } else { dir.join(folder.replace('/', "\\")) };
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&assets).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(v) = iconpick::parse_asset(&name, &stem, &ext, logo.base_px) {
            files.push((entry.path(), v));
        }
    }
    (!files.is_empty()).then_some(PackageAssets { dir, assets, files })
}

/// The logo of a packaged app.
pub struct PackageLogo {
    pub bgra: Bgra,
    /// The size the chosen file's name stands for, and whether it is the unplated form.
    pub variant: AssetVariant,
}

/// The package logo file that fits an icon needing `need` px (see [`iconpick::choose_asset`]), decoded. `None` when the package's
/// files cannot be read (the shell is asked then).
pub fn package_logo(aumid: &str, need: u32) -> Option<PackageLogo> {
    let found = package_assets(aumid)?;
    let variants: Vec<AssetVariant> = found.files.iter().map(|(_, v)| v.clone()).collect();
    // (a file that does not decode - a stub, a damaged one - is dropped and the next best is tried)
    let mut left: Vec<usize> = (0..variants.len()).collect();
    while !left.is_empty() {
        let pool: Vec<AssetVariant> = left.iter().map(|&i| variants[i].clone()).collect();
        let pick = left[iconpick::choose_asset(&pool, need)?];
        let (path, variant) = &found.files[pick];
        if let Some(img) = std::fs::read(path).ok().and_then(|b| image::load_from_memory_with_format(&b, image::ImageFormat::Png).ok()) {
            return Some(PackageLogo { bgra: rgba_to_bgra(img.to_rgba8()), variant: variant.clone() });
        }
        left.retain(|&i| i != pick);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_chain_is_tried_with_the_package_first_and_the_windows_last() {
        let srcs = [IconSource::File("a.exe".into()), IconSource::Window(1), IconSource::Window(2), IconSource::Packaged("p_x!a".into()), IconSource::Shell("s".into())];
        assert_eq!(rank(&srcs), vec![3, 0, 4, 1, 2], "stable inside each class");
        assert_eq!(rank(&[]), Vec::<usize>::new());
        // an item with only its own icon keeps it
        assert_eq!(rank(&[IconSource::File("a.exe".into())]), vec![0]);
    }

    fn group_bytes(entries: &[(u8, u8, u16, u16)]) -> Vec<u8> {
        let mut b = vec![0, 0, 1, 0];
        b.extend((entries.len() as u16).to_le_bytes());
        for &(w, h, bpp, id) in entries {
            b.extend([w, h, 0, 0, 1, 0]);
            b.extend(bpp.to_le_bytes());
            b.extend(1234u32.to_le_bytes());
            b.extend(id.to_le_bytes());
        }
        b
    }

    #[test]
    fn a_group_directory_is_read_as_the_system_writes_it() {
        let g = parse_group(&group_bytes(&[(16, 16, 32, 1), (48, 48, 32, 2), (0, 0, 32, 3)]));
        assert_eq!(g.len(), 3);
        assert_eq!((g[1].info.side(), g[1].id), (48, 2));
        assert_eq!((g[2].info.side(), g[2].id, g[2].info.bpp), (256, 3, 32));
        let infos: Vec<FrameInfo> = g.iter().map(|e| e.info).collect();
        assert_eq!(iconpick::choose_frame(&infos, 69), Some(2));
        // a header that is not an icon group, a cut-off list and nothing
        assert!(parse_group(&[0, 0, 2, 0, 1, 0]).is_empty());
        assert_eq!(parse_group(&group_bytes(&[(16, 16, 32, 1)])[..10]).len(), 0);
        assert!(parse_group(&[]).is_empty());
    }

    #[test]
    fn an_icon_index_is_a_position_or_minus_a_resource_id() {
        let names = [ResName::Id(1), ResName::Id(100), ResName::Text(vec![65])];
        assert_eq!(pick_group(&names, 0), Some(ResName::Id(1)));
        assert_eq!(pick_group(&names, 2), Some(ResName::Text(vec![65])));
        assert_eq!(pick_group(&names, 3), None);
        assert_eq!(pick_group(&names, -100), Some(ResName::Id(100)));
        assert_eq!(pick_group(&names, -7), None, "no such resource");
        assert_eq!(pick_group(&[], 0), None);
    }

    #[test]
    fn nothing_to_ask_for_is_an_error() {
        assert!(load_levels(&[IconSource::File("x".into())], &[]).is_err());
        assert!(load_levels(&[], &[44]).is_err());
    }

    // The tests below only read: the files of every Windows.

    #[test]
    fn a_program_gives_its_biggest_frame_above_48_px_and_every_level_has_its_size() {
        let src = IconSource::File(r"C:\Windows\explorer.exe".into());
        let sizes = [44, 55, 69];
        let got = load_levels(&[src], &sizes).expect("explorer has an icon");
        assert_eq!(got.levels.iter().map(|b| (b.width, b.height)).collect::<Vec<_>>(), vec![(44, 44), (55, 55), (69, 69)]);
        assert_eq!(got.used, Used::ProgramFrame, "above 48 px the frame is chosen by us");
        assert!(got.native_px >= 69, "a frame with enough pixels of its own: {}", got.native_px);
        assert!(got.levels[2].data.as_chunks::<4>().0.iter().any(|p| p[3] == 255));
    }

    #[test]
    fn the_frames_of_a_program_are_listed_and_a_missing_file_has_none() {
        let frames = program_frames(Path::new(r"C:\Windows\explorer.exe"), 0).expect("explorer's icon group");
        assert!(!frames.is_empty() && frames.iter().all(|f| f.side() >= 16));
        assert!(program_frames(Path::new(r"C:\no\such\file.exe"), 0).is_none());
        assert!(program_frames(Path::new(r"C:\Windows\explorer.exe"), 9999).is_none());
    }

    #[test]
    fn up_to_48_px_a_program_keeps_the_shells_answer() {
        let src = IconSource::File(r"C:\Windows\explorer.exe".into());
        let got = load_levels(&[src], &[44]).unwrap();
        assert_eq!(got.used, Used::Shell);
        assert_eq!((got.levels[0].width, got.levels[0].height), (44, 44));
    }

    /// A bitmap of `side` px, to stand for what a source gives.
    fn bitmap(side: u32) -> Bgra {
        Bgra { width: side, height: side, data: vec![255; (side * side * 4) as usize] }
    }

    #[test]
    fn a_window_icon_is_the_last_resort_not_the_first() {
        use std::cell::RefCell;
        // a window that DOES give a (small) icon, listed before the program's own file that has a big one
        let srcs = [IconSource::Window(7), IconSource::File("a.exe".into())];
        let asked = RefCell::new(Vec::new());
        let got = load_levels_with(&srcs, &[44, 69], |src, need| {
            asked.borrow_mut().push((src.clone(), need));
            Ok(match src {
                IconSource::Window(_) => (bitmap(32), Used::WindowIcon),
                _ => (bitmap(256), Used::ProgramFrame),
            })
        })
        .unwrap();
        assert_eq!(got.source, 1, "the file, not the window");
        assert_eq!((got.used, got.native_px), (Used::ProgramFrame, 256));
        assert_eq!(asked.borrow().len(), 1, "the window is not even asked while a better source gives a picture");
        assert_eq!(asked.borrow()[0].1, 69, "the biggest size is what the source is asked for");
        // the file fails: the window's small icon is better than nothing
        let got = load_levels_with(&srcs, &[44], |src, _| match src {
            IconSource::Window(_) => Ok((bitmap(32), Used::WindowIcon)),
            _ => Err(anyhow::anyhow!("unreadable")),
        })
        .unwrap();
        assert_eq!((got.source, got.used, got.native_px), (0, Used::WindowIcon, 32));
        assert_eq!(got.levels[0].width, 44, "enlarged to the size by us");
        // nothing gives one: the first source's error
        let err = load_levels_with(&srcs, &[44], |src, _| Err(anyhow::anyhow!("{src:?} failed"))).err().unwrap();
        assert!(err.to_string().contains("File"), "{err}");
    }

    #[test]
    fn the_shell_is_asked_for_its_biggest_picture_above_48_px_and_for_the_size_up_to_it() {
        assert_eq!(shell_ask(44), 44);
        assert_eq!(shell_ask(48), 48);
        assert_eq!(shell_ask(49), 256);
        assert_eq!(shell_ask(69), 256);
        assert_eq!(shell_ask(137), 256);
    }

    #[test]
    fn a_package_that_is_not_installed_has_no_logo() {
        assert!(package_dir("No.Such.Package_0000000000000").is_none());
        assert!(package_logo("No.Such.Package_0000000000000!App", 69).is_none());
        assert!(package_assets("not a package id").is_none());
    }
}
