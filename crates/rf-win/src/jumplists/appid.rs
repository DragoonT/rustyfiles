//! The AppID that names an app's jump-list files: `<AppID>.automaticDestinations-ms` and `<AppID>.customDestinations-ms`.
//!
//! The file name is a 64-bit CRC of the app's identity text, written in lower-case hexadecimal WITHOUT leading zeros (15-digit names
//! exist). Note 3-start-search.md (section 1.6) knew the polynomial, `0x92C64265D32139A4`, and not the rest. The parameters below
//! were settled against this machine, read-only (see `docs/jumplists.md` for the counts), and against two published vectors:
//!
//! * the polynomial is used as the REFLECTED polynomial of the classic LSB-first algorithm (equivalently: bit-reversed, MSB-first, with
//!   input and output reflection);
//! * the register starts as all ones and the result is not inverted;
//! * the input is the identity upper-cased and written as UTF-16LE without a terminator.
//!
//! The identity text is the explicit AppUserModelID when the app has one (a packaged app's included), otherwise its program in the
//! shell's known-folder form (`{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe`; the same string the "All apps" list uses as an id),
//! and for a few apps the fully expanded path.
//!
//! [`sweep`] is the pure matcher that found the parameters: every combination of the usual CRC parameters over every text form of every
//! identity, counted against a set of existing file names. It stays so the settling can be repeated on another build.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The polynomial the note gives.
pub const POLY: u64 = 0x92C6_4265_D321_39A4;

/// Which bit order of [`POLY`] is the polynomial of a most-significant-bit-first register.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolyBits {
    /// The constant as written.
    AsWritten,
    /// The constant with its 64 bits reversed.
    Reversed,
}

/// The Rocksoft-model parameters of a CRC-64 over [`POLY`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CrcParams {
    pub poly: PolyBits,
    pub init: u64,
    pub xor_out: u64,
    pub reflect_in: bool,
    pub reflect_out: bool,
}

/// The parameters that name the files (settled on this machine, see the module comment).
pub const SETTLED: CrcParams = CrcParams { poly: PolyBits::Reversed, init: u64::MAX, xor_out: 0, reflect_in: true, reflect_out: true };

/// The generic, bit-at-a-time CRC-64 (slow; for the sweep and as the reference the fast table version is tested against).
pub fn crc64(p: &CrcParams, data: &[u8]) -> u64 {
    let poly = match p.poly {
        PolyBits::AsWritten => POLY,
        PolyBits::Reversed => POLY.reverse_bits(),
    };
    let mut c = p.init;
    for &b in data {
        let b = if p.reflect_in { b.reverse_bits() } else { b };
        c ^= (b as u64) << 56;
        for _ in 0..8 {
            c = if c & (1 << 63) != 0 { (c << 1) ^ poly } else { c << 1 };
        }
    }
    if p.reflect_out {
        c = c.reverse_bits();
    }
    c ^ p.xor_out
}

fn table() -> &'static [u64; 256] {
    static T: OnceLock<[u64; 256]> = OnceLock::new();
    T.get_or_init(|| {
        let mut t = [0u64; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u64;
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ POLY } else { c >> 1 };
            }
            *slot = c;
        }
        t
    })
}

/// Upper-cases the way the identity is hashed: one UTF-16 unit at a time, a unit whose upper case is not a single unit stays.
fn upper_units(s: &str) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut buf = [0u16; 2];
    for c in s.chars() {
        let mut up = c.to_uppercase();
        let u = match (up.next(), up.next()) {
            (Some(u), None) => u,
            _ => c,
        };
        out.extend_from_slice(u.encode_utf16(&mut buf));
    }
    out
}

/// The AppID of an identity text, by the settled parameters.
pub fn app_id(identity: &str) -> u64 {
    let t = table();
    let mut c = u64::MAX;
    for u in upper_units(identity) {
        for b in u.to_le_bytes() {
            c = t[((c ^ b as u64) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c
}

/// The file stem for an AppID: lower-case hexadecimal, no leading zeros (what Windows writes).
pub fn file_stem(id: u64) -> String {
    format!("{id:x}")
}

/// Where an AppID's file is, in `dir` (`AutomaticDestinations` or `CustomDestinations`).
pub fn file_in(dir: &Path, id: u64, extension: &str) -> PathBuf {
    dir.join(format!("{}.{extension}", file_stem(id)))
}

/// The jump-list folder (`%APPDATA%\Microsoft\Windows\Recent`), `None` when `APPDATA` is not set.
pub fn recent_root() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Recent"))
}

pub const AUTOMATIC_EXT: &str = "automaticDestinations-ms";
pub const CUSTOM_EXT: &str = "customDestinations-ms";

/// How an identity text was made, for the counts of the probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Form {
    /// The explicit AppUserModelID of the window, the shortcut or the package.
    Aumid,
    /// The program's path with its leading known folder written as `{GUID}\...`.
    KnownFolderPath,
    /// The program's path as it is on disk.
    ExpandedPath,
}

/// One identity text an app's AppID may be the hash of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub form: Form,
    pub text: String,
    pub id: u64,
}

/// The known folders programs live under, as `({GUID}, path)`; the order does not matter ([`known_folder_form`] takes the longest match).
pub fn known_folders_now() -> Vec<(String, PathBuf)> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{
        FOLDERID_LocalAppData, FOLDERID_Profile, FOLDERID_ProgramData, FOLDERID_ProgramFiles, FOLDERID_ProgramFilesCommon,
        FOLDERID_ProgramFilesCommonX86, FOLDERID_ProgramFilesX64, FOLDERID_ProgramFilesX86, FOLDERID_RoamingAppData, FOLDERID_System,
        FOLDERID_SystemX86, FOLDERID_Windows, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    };
    let ids = [
        FOLDERID_ProgramFilesX64,
        FOLDERID_ProgramFilesX86,
        FOLDERID_ProgramFiles,
        FOLDERID_ProgramFilesCommonX86,
        FOLDERID_ProgramFilesCommon,
        FOLDERID_System,
        FOLDERID_SystemX86,
        FOLDERID_Windows,
        FOLDERID_LocalAppData,
        FOLDERID_RoamingAppData,
        FOLDERID_ProgramData,
        FOLDERID_Profile,
    ];
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for id in ids {
        let Ok(p) = (unsafe { SHGetKnownFolderPath(&id, KF_FLAG_DEFAULT, None) }) else { continue };
        let s = unsafe { p.to_string().ok() };
        unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
        if let Some(s) = s {
            let guid = format!("{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}", id.data1, id.data2, id.data3, id.data4[0], id.data4[1], id.data4[2], id.data4[3], id.data4[4], id.data4[5], id.data4[6], id.data4[7]);
            if !out.iter().any(|(g, _)| *g == guid) {
                out.push((guid, PathBuf::from(s)));
            }
        }
    }
    out
}

/// The rest of `path` after `prefix`, when `path` starts with `prefix` ignoring case. Compared one character at a time and sliced at the
/// ORIGINAL string's offset: lower-casing whole strings and reusing a length from the lower-cased one is wrong for letters whose lower
/// case has another byte length (a capital I with a dot, "İ", becomes "i" and a combining dot: 2 bytes become 3).
fn strip_prefix_ignore_case<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let mut chars = path.char_indices();
    let mut end = 0;
    for pc in prefix.chars() {
        let (i, c) = chars.next()?;
        if !c.to_lowercase().eq(pc.to_lowercase()) {
            return None;
        }
        end = i + c.len_utf8();
    }
    Some(&path[end..])
}

/// `path` with its longest matching known folder replaced by that folder's `{GUID}` (case-insensitive, at a path-component boundary).
/// The rest of the path keeps its own spelling.
pub fn known_folder_form(path: &str, folders: &[(String, PathBuf)]) -> Option<String> {
    let mut best: Option<(usize, &str, &str)> = None; // (bytes of `path` the folder covers, guid, the rest of the path)
    for (guid, dir) in folders {
        let dir = dir.to_string_lossy();
        let dir = dir.trim_end_matches('\\');
        if dir.is_empty() {
            continue;
        }
        let Some(rest) = strip_prefix_ignore_case(path, dir) else { continue };
        if !(rest.is_empty() || rest.starts_with('\\')) {
            continue;
        }
        let covered = path.len() - rest.len();
        if best.is_none_or(|(n, _, _)| covered > n) {
            best = Some((covered, guid, rest));
        }
    }
    let (_, guid, rest) = best?;
    Some(format!("{guid}{rest}"))
}

/// Every identity text an app may be hashed under, the most likely first, without repeats. `aumid` is the explicit AppUserModelID (a
/// packaged app's id included), `exe` the program.
pub fn candidates(aumid: Option<&str>, exe: Option<&Path>, folders: &[(String, PathBuf)]) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut add = |form: Form, text: String| {
        if text.is_empty() {
            return;
        }
        let id = app_id(&text);
        if !out.iter().any(|c| c.id == id) {
            out.push(Candidate { form, text, id });
        }
    };
    if let Some(a) = aumid.map(str::trim).filter(|a| !a.is_empty()) {
        add(Form::Aumid, a.to_string());
    }
    if let Some(e) = exe {
        let s = e.to_string_lossy().into_owned();
        // an id that is already a known-folder form (the "All apps" list's ids) is used as it is
        if let Some(k) = known_folder_form(&s, folders) {
            add(Form::KnownFolderPath, k);
        }
        add(Form::ExpandedPath, s);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------------
// the sweep that settled the parameters

/// How the identity text was turned into bytes in a sweep row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextForm {
    /// Upper-cased UTF-16LE (the settled one).
    UpperUtf16,
    /// As written, UTF-16LE.
    AsWrittenUtf16,
    /// Lower-cased UTF-16LE.
    LowerUtf16,
    /// Upper-cased UTF-16LE with a terminating NUL unit.
    UpperUtf16Nul,
    /// Upper-cased UTF-8.
    UpperUtf8,
}

impl TextForm {
    pub const ALL: [TextForm; 5] = [TextForm::UpperUtf16, TextForm::AsWrittenUtf16, TextForm::LowerUtf16, TextForm::UpperUtf16Nul, TextForm::UpperUtf8];

    pub fn bytes(self, s: &str) -> Vec<u8> {
        let utf16 = |units: Vec<u16>| units.into_iter().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>();
        match self {
            TextForm::UpperUtf16 => utf16(upper_units(s)),
            TextForm::AsWrittenUtf16 => utf16(s.encode_utf16().collect()),
            TextForm::LowerUtf16 => utf16(s.to_lowercase().encode_utf16().collect()),
            TextForm::UpperUtf16Nul => {
                let mut u = upper_units(s);
                u.push(0);
                utf16(u)
            }
            TextForm::UpperUtf8 => s.to_uppercase().into_bytes(),
        }
    }
}

/// One row of a sweep: a parameter set and text form, and how many of the identities named an existing file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SweepRow {
    pub params: CrcParams,
    pub text: TextForm,
    /// Identities whose hash named a file in the first set (automatic destinations).
    pub in_first: usize,
    /// ... in the second set (custom destinations).
    pub in_second: usize,
}

/// Every usual parameter set (both bit orders of the polynomial, init and xor-out of zeros or ones, input and output reflection on
/// or off) over every text form of every identity, counted against two sets of existing file stems (lower-case hex, no padding or
/// with it: both spellings are tried). Best first.
pub fn sweep(identities: &[String], first: &HashSet<String>, second: &HashSet<String>) -> Vec<SweepRow> {
    let mut rows = Vec::new();
    for poly in [PolyBits::AsWritten, PolyBits::Reversed] {
        for init in [0, u64::MAX] {
            for xor_out in [0, u64::MAX] {
                for reflect_in in [false, true] {
                    for reflect_out in [false, true] {
                        let params = CrcParams { poly, init, xor_out, reflect_in, reflect_out };
                        for text in TextForm::ALL {
                            let (mut a, mut b) = (0, 0);
                            for id in identities {
                                let v = crc64(&params, &text.bytes(id));
                                let plain = format!("{v:x}");
                                let padded = format!("{v:016x}");
                                if first.contains(&plain) || first.contains(&padded) {
                                    a += 1;
                                }
                                if second.contains(&plain) || second.contains(&padded) {
                                    b += 1;
                                }
                            }
                            rows.push(SweepRow { params, text, in_first: a, in_second: b });
                        }
                    }
                }
            }
        }
    }
    rows.sort_by_key(|r| std::cmp::Reverse(r.in_first + r.in_second));
    rows
}

/// The file stems in a jump-list folder (`<stem>.<extension>`), lower case.
pub fn stems_in(dir: &Path, extension: &str) -> HashSet<String> {
    let suffix = format!(".{}", extension.to_lowercase());
    let mut out = HashSet::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase();
            if let Some(stem) = name.strip_suffix(&suffix) {
                out.insert(stem.to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_settled_parameters_give_the_published_ids() {
        // Windows Explorer's own AppID (published in many forensic references)
        assert_eq!(file_stem(app_id("Microsoft.Windows.Explorer")), "f01b4d95cf55d32a");
        // 64-bit Notepad, by its known-folder path (published: 9b9cdc69c1c24e2b)
        assert_eq!(file_stem(app_id(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe")), "9b9cdc69c1c24e2b");
        // upper-casing is part of the hash: the same identity in any case names the same file
        assert_eq!(app_id("microsoft.windows.explorer"), app_id("MICROSOFT.WINDOWS.EXPLORER"));
    }

    #[test]
    fn the_fast_table_version_is_the_generic_one() {
        for s in ["", "A", "Microsoft.Windows.Explorer", r"{6D809377-6AF0-444B-8957-A3773F02200E}\Some App\app.exe", "Türkçe Uygulama"] {
            let bytes = TextForm::UpperUtf16.bytes(s);
            assert_eq!(app_id(s), crc64(&SETTLED, &bytes), "{s:?}");
        }
    }

    #[test]
    fn a_leading_zero_is_dropped_from_the_file_name() {
        assert_eq!(file_stem(0x0123_4567_89AB_CDEF), "123456789abcdef");
        assert_eq!(file_stem(0xF01B_4D95_CF55_D32A), "f01b4d95cf55d32a");
        assert_eq!(file_in(Path::new("d"), 0xAB, AUTOMATIC_EXT), Path::new("d").join("ab.automaticDestinations-ms"));
    }

    fn folders() -> Vec<(String, PathBuf)> {
        vec![
            ("{6D809377-6AF0-444B-8957-A3773F02200E}".into(), PathBuf::from(r"C:\Program Files")),
            ("{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}".into(), PathBuf::from(r"C:\Program Files (x86)")),
            ("{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}".into(), PathBuf::from(r"C:\WINDOWS\system32")),
            ("{F38BF404-1D43-42F2-9305-67DE0B28FC23}".into(), PathBuf::from(r"C:\WINDOWS")),
        ]
    }

    #[test]
    fn a_path_takes_its_longest_known_folder_at_a_component_boundary_ignoring_case() {
        let f = folders();
        assert_eq!(known_folder_form(r"C:\Windows\System32\notepad.exe", &f).as_deref(), Some(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe"));
        assert_eq!(known_folder_form(r"c:\program files (x86)\A\a.exe", &f).as_deref(), Some(r"{7C5A40EF-A0FB-4BFC-874A-C0F2E0B9FA8E}\A\a.exe"));
        assert_eq!(known_folder_form(r"C:\Windows\explorer.exe", &f).as_deref(), Some(r"{F38BF404-1D43-42F2-9305-67DE0B28FC23}\explorer.exe"));
        // "C:\Program Files" is not a prefix of "C:\Program Files2\..." as a folder
        assert_eq!(known_folder_form(r"C:\Program Files2\a.exe", &f), None);
        assert_eq!(known_folder_form(r"D:\tools\a.exe", &f), None);
    }

    #[test]
    fn a_profile_name_with_a_dotted_capital_i_keeps_its_backslash_and_its_spelling() {
        // U+0130 lower-cases to "i" plus a combining dot: 2 bytes become 3, so a length taken from the lower-cased text cut the rest wrong
        let guid = "{F1B32785-6FBA-4FCF-9D55-7B8E7F157091}";
        let f = vec![(guid.to_string(), PathBuf::from("C:\\Users\\\u{130}brahim\\AppData\\Local"))];
        let p = "C:\\Users\\\u{130}brahim\\AppData\\Local\\Programs\\app\\app.exe";
        assert_eq!(known_folder_form(p, &f).as_deref(), Some(format!("{guid}\\Programs\\app\\app.exe").as_str()));
        // the same with the dotless small letter in the profile name and a path spelled in lower case
        let f2 = vec![(guid.to_string(), PathBuf::from("C:\\Users\\Ib\u{131}\\AppData\\Local"))];
        assert_eq!(known_folder_form("c:\\users\\ib\u{131}\\appdata\\local\\P\\a.exe", &f2).as_deref(), Some(format!("{guid}\\P\\a.exe").as_str()));
        // the plain "i" is another letter than the dotted capital (Windows does not equate them either)
        assert_eq!(known_folder_form("C:\\Users\\ibrahim\\AppData\\Local\\a.exe", &f), None);
        // a folder written with a trailing backslash, and a path that is the folder itself
        let root = vec![("{G}".to_string(), PathBuf::from("D:\\Apps\\"))];
        assert_eq!(known_folder_form("d:\\apps", &root).as_deref(), Some("{G}"));
        assert_eq!(known_folder_form("d:\\apps\\x\\y.exe", &root).as_deref(), Some("{G}\\x\\y.exe"));
    }

    #[test]
    fn candidates_are_the_aumid_then_the_known_folder_form_then_the_path_without_repeats() {
        let f = folders();
        let c = candidates(Some("Vendor.App"), Some(Path::new(r"C:\Program Files\V\a.exe")), &f);
        assert_eq!(c.iter().map(|c| c.form).collect::<Vec<_>>(), vec![Form::Aumid, Form::KnownFolderPath, Form::ExpandedPath]);
        assert_eq!(c[1].text, r"{6D809377-6AF0-444B-8957-A3773F02200E}\V\a.exe");
        // no aumid, program outside every known folder: the path alone
        let c = candidates(None, Some(Path::new(r"D:\x\a.exe")), &f);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].form, Form::ExpandedPath);
        // an id given twice in two spellings is one candidate
        let c = candidates(Some("vendor.app"), None, &f);
        assert_eq!(c.len(), 1);
        assert!(candidates(Some("  "), None, &f).is_empty());
    }

    #[test]
    fn the_sweep_finds_the_settled_parameters_among_all_the_others() {
        let ids: Vec<String> = ["Microsoft.Windows.Explorer", "Chrome", "MSEdge", r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\notepad.exe"].iter().map(|s| s.to_string()).collect();
        let mut first = HashSet::new();
        for i in &ids {
            first.insert(file_stem(app_id(i)));
        }
        let rows = sweep(&ids, &first, &HashSet::new());
        assert_eq!(rows.len(), 2 * 2 * 2 * 2 * 2 * TextForm::ALL.len());
        let best = rows[0];
        assert_eq!((best.params, best.text, best.in_first), (SETTLED, TextForm::UpperUtf16, ids.len()));
        // nothing else names a single file (an all-upper-case identity is the same bytes as its as-written form; Chrome is not)
        assert!(rows.iter().skip(1).all(|r| r.in_first < ids.len()));
        assert!(rows[1].in_first <= 2, "the runner-up got {}", rows[1].in_first);
    }

    #[test]
    fn stems_are_read_from_a_folder_by_extension() {
        let dir = std::env::temp_dir().join(format!("rf-jl-stems-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("ABC123.automaticDestinations-ms"), b"x").unwrap();
        std::fs::write(dir.join("def.customDestinations-ms"), b"x").unwrap();
        std::fs::write(dir.join("noext"), b"x").unwrap();
        let a = stems_in(&dir, AUTOMATIC_EXT);
        assert_eq!(a.len(), 1);
        assert!(a.contains("abc123"));
        assert_eq!(stems_in(&dir, CUSTOM_EXT).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(stems_in(&dir, CUSTOM_EXT).is_empty(), "a missing folder has no stems");
    }
}
