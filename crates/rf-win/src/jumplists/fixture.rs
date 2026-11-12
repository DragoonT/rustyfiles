//! Byte fixtures for the tests, built by hand from the file formats' layouts (MS-SHLLINK for the shell link, MS-CFB through the `cfb`
//! crate's writer for the compound file). Nothing here is a copy of a real jump-list file.

use std::io::{Cursor, Write};

use super::custom::{CategoryKind, RawCategory};
use super::destlist::{self, DestEntry};

/// What a fixture shell link says.
#[derive(Clone, Default)]
pub struct L {
    /// A local file path (stored in the link info, as Windows does for a plain file).
    pub target: Option<String>,
    pub args: Option<String>,
    pub working_dir: Option<String>,
    pub icon: Option<(String, i32)>,
    pub title: Option<String>,
    pub aumid: Option<String>,
    pub separator: bool,
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

fn counted(s: &str) -> Vec<u8> {
    let mut v = (s.encode_utf16().count() as u16).to_le_bytes().to_vec();
    v.extend(utf16(s));
    v
}

/// A serialized property store block (extra data signature 0xA0000009) holding string / bool values of the shell's `{GUID}, pid` keys.
fn property_block(strings: &[(u128, u32, &str)], bools: &[(u128, u32, bool)]) -> Vec<u8> {
    let mut storages: Vec<u8> = Vec::new();
    let mut all: Vec<(u128, Vec<u8>)> = Vec::new();
    for (g, pid, s) in strings {
        let mut val = Vec::new();
        val.extend_from_slice(&pid.to_le_bytes());
        val.push(0); // reserved
        val.extend_from_slice(&0x001Fu16.to_le_bytes()); // VT_LPWSTR
        val.extend_from_slice(&0u16.to_le_bytes());
        val.extend_from_slice(&((s.encode_utf16().count() + 1) as u32).to_le_bytes());
        val.extend(utf16(s));
        val.extend_from_slice(&[0, 0]);
        // the string part (not the whole value) is padded to four bytes, as the shell writes it
        val.resize(val.len() + (4 - (s.encode_utf16().count() * 2 + 2) % 4) % 4, 0);
        all.push((*g, val));
    }
    for (g, pid, b) in bools {
        let mut val = Vec::new();
        val.extend_from_slice(&pid.to_le_bytes());
        val.push(0);
        val.extend_from_slice(&0x000Bu16.to_le_bytes()); // VT_BOOL
        val.extend_from_slice(&0u16.to_le_bytes());
        val.extend_from_slice(&(if *b { 0xFFFFu16 } else { 0 }).to_le_bytes());
        val.extend_from_slice(&[0, 0]); // padded to four bytes
        all.push((*g, val));
    }
    // one storage per format id
    let mut ids: Vec<u128> = Vec::new();
    for (g, _) in &all {
        if !ids.contains(g) {
            ids.push(*g);
        }
    }
    for g in ids {
        let mut body = Vec::new();
        for (gg, v) in &all {
            if *gg == g {
                body.extend_from_slice(&((v.len() + 4) as u32).to_le_bytes());
                body.extend_from_slice(v);
            }
        }
        body.extend_from_slice(&0u32.to_le_bytes()); // terminal value
        let total = 4 + 4 + 16 + body.len();
        storages.extend_from_slice(&(total as u32).to_le_bytes());
        storages.extend_from_slice(&0x5350_5331u32.to_le_bytes()); // "1SPS"
        storages.extend_from_slice(&guid_bytes(g));
        storages.extend(body);
    }
    storages.extend_from_slice(&0u32.to_le_bytes()); // terminal storage
    let mut block = Vec::new();
    block.extend_from_slice(&((8 + storages.len()) as u32).to_le_bytes());
    block.extend_from_slice(&0xA000_0009u32.to_le_bytes());
    block.extend(storages);
    block
}

fn guid_bytes(g: u128) -> [u8; 16] {
    let b = g.to_be_bytes();
    let mut o = [0u8; 16];
    o[0..4].copy_from_slice(&u32::from_be_bytes([b[0], b[1], b[2], b[3]]).to_le_bytes());
    o[4..6].copy_from_slice(&u16::from_be_bytes([b[4], b[5]]).to_le_bytes());
    o[6..8].copy_from_slice(&u16::from_be_bytes([b[6], b[7]]).to_le_bytes());
    o[8..16].copy_from_slice(&b[8..16]);
    o
}

const SUMMARY_INFO: u128 = 0xF29F85E0_4FF9_1068_AB91_08002B27B3D9; // System.Title is {F29F85E0-...}, 2
const APP_USER_MODEL: u128 = 0x9F4C2855_9F79_4B39_A8D0_E1D42DE1D5F3; // System.AppUserModel.* is {9F4C2855-...}, 5 / 6

/// The item ID list of a local path `X:\a\b.txt`: My Computer, the drive, then one simple file-entry item per component (the short
/// ANSI name only, which is enough for the shell to rebuild the path; `IShellLink::GetPath` reads the target from this list).
fn id_list(path: &str) -> Vec<u8> {
    let mut items: Vec<Vec<u8>> = Vec::new();
    // My Computer {20D04FE0-3AEA-1069-A2D8-08002B30309D}
    let mut computer = vec![0x1F, 0x50];
    computer.extend_from_slice(&guid_bytes(0x20D04FE0_3AEA_1069_A2D8_08002B30309D));
    items.push(computer);
    let mut parts = path.split('\\');
    let drive = parts.next().unwrap_or("C:");
    let mut d = vec![0x2F];
    d.extend_from_slice(drive.as_bytes());
    d.push(b'\\');
    d.resize(1 + 22, 0);
    items.push(d);
    let comps: Vec<&str> = parts.collect();
    for (i, c) in comps.iter().enumerate() {
        let last = i + 1 == comps.len();
        let mut it = vec![if last && c.contains('.') { 0x32 } else { 0x31 }, 0];
        it.extend_from_slice(&0u32.to_le_bytes()); // size
        it.extend_from_slice(&0x5A21_5A21u32.to_le_bytes()); // a DOS date and time
        it.extend_from_slice(&0x20u16.to_le_bytes()); // attributes
        it.extend_from_slice(c.as_bytes());
        it.push(0);
        if (it.len() + 2) % 2 != 0 {
            it.push(0);
        }
        items.push(it);
    }
    let mut out = Vec::new();
    for it in items {
        out.extend_from_slice(&((it.len() + 2) as u16).to_le_bytes());
        out.extend(it);
    }
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

/// A shell link in its file format.
pub fn lnk(l: &L) -> Vec<u8> {
    let mut flags = 0u32;
    let mut body: Vec<u8> = Vec::new();
    if let Some(t) = &l.target {
        flags |= 1; // HasLinkTargetIDList
        let ids = id_list(t);
        body.extend_from_slice(&(ids.len() as u16).to_le_bytes());
        body.extend(ids);
        flags |= 1 << 1; // HasLinkInfo
        // LinkInfo: header 0x1C, local base path flag, a volume id, the path (ANSI), an empty common suffix
        let vol_label = b"\0";
        let mut vol = Vec::new();
        vol.extend_from_slice(&((0x10 + vol_label.len()) as u32).to_le_bytes());
        vol.extend_from_slice(&3u32.to_le_bytes()); // DRIVE_FIXED
        vol.extend_from_slice(&0x1234_5678u32.to_le_bytes());
        vol.extend_from_slice(&0x10u32.to_le_bytes());
        vol.extend_from_slice(vol_label);
        let path = [t.as_bytes(), &[0]].concat();
        let vol_off = 0x1Cu32;
        let path_off = vol_off + vol.len() as u32;
        let suffix_off = path_off + path.len() as u32;
        let size = suffix_off + 1;
        let mut info = Vec::new();
        info.extend_from_slice(&size.to_le_bytes());
        info.extend_from_slice(&0x1Cu32.to_le_bytes());
        info.extend_from_slice(&1u32.to_le_bytes()); // VolumeIDAndLocalBasePath
        info.extend_from_slice(&vol_off.to_le_bytes());
        info.extend_from_slice(&path_off.to_le_bytes());
        info.extend_from_slice(&0u32.to_le_bytes());
        info.extend_from_slice(&suffix_off.to_le_bytes());
        info.extend(vol);
        info.extend(path);
        info.push(0);
        body.extend(info);
    }
    if let Some(w) = &l.working_dir {
        flags |= 1 << 4;
        body.extend(counted(w));
    }
    if let Some(a) = &l.args {
        flags |= 1 << 5;
        body.extend(counted(a));
    }
    if let Some((f, _)) = &l.icon {
        flags |= 1 << 6;
        body.extend(counted(f));
    }
    flags |= 1 << 7; // IsUnicode
    let mut strings: Vec<(u128, u32, &str)> = Vec::new();
    if let Some(t) = &l.title {
        strings.push((SUMMARY_INFO, 2, t));
    }
    if let Some(a) = &l.aumid {
        strings.push((APP_USER_MODEL, 5, a));
    }
    let bools: Vec<(u128, u32, bool)> = if l.separator { vec![(APP_USER_MODEL, 6, true)] } else { vec![] };
    if !strings.is_empty() || !bools.is_empty() {
        body.extend(property_block(&strings, &bools));
    }
    body.extend_from_slice(&0u32.to_le_bytes()); // terminal block
    let mut v = Vec::new();
    v.extend_from_slice(&0x4Cu32.to_le_bytes());
    v.extend_from_slice(&super::custom::LINK_CLSID);
    v.extend_from_slice(&flags.to_le_bytes());
    v.extend_from_slice(&0x20u32.to_le_bytes()); // FILE_ATTRIBUTE_ARCHIVE
    v.extend_from_slice(&[0u8; 24]); // three FILETIMEs
    v.extend_from_slice(&0u32.to_le_bytes()); // file size
    v.extend_from_slice(&l.icon.as_ref().map_or(0, |(_, i)| *i).to_le_bytes());
    v.extend_from_slice(&1u32.to_le_bytes()); // SW_SHOWNORMAL
    v.extend_from_slice(&[0u8; 12]); // hot key, two reserved words, one reserved dword
    assert_eq!(v.len(), 0x4C);
    v.extend(body);
    v
}

/// A custom-destinations file.
pub fn custom_file(categories: &[(CategoryKind, Vec<L>)]) -> Vec<u8> {
    let cats: Vec<RawCategory> = categories.iter().map(|(k, ls)| RawCategory { kind: k.clone(), links: ls.iter().map(lnk).collect() }).collect();
    super::custom::build(&cats)
}

/// An automatic-destinations file (a compound file): a `DestList` and one stream per entry, named by the entry number in hexadecimal.
/// An entry with `None` has a `DestList` row and no stream.
pub fn automatic_file(version: u32, entries: &[(DestEntry, Option<L>)]) -> Vec<u8> {
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).unwrap();
    let rows: Vec<DestEntry> = entries.iter().map(|(e, _)| e.clone()).collect();
    cf.create_stream("/DestList").unwrap().write_all(&destlist::build(version, &rows)).unwrap();
    for (e, l) in entries {
        if let Some(l) = l {
            cf.create_stream(format!("/{:x}", e.number)).unwrap().write_all(&lnk(l)).unwrap();
        }
    }
    cf.flush().unwrap();
    cf.into_inner().into_inner()
}
