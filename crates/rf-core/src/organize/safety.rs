//! What the organizer must never touch: files still being written, hidden and system files,
//! the app's own files, and folders that may not be watched at all.

use std::path::{Path, PathBuf};

use super::rule::ext_of;
use crate::entry::fold_case;

/// Extensions of files a browser, a torrent client or an editor is still writing (or keeps as a
/// lock / swap file). A rule never moves them, whatever it says.
pub const PARTIAL_EXTS: &[&str] = &[
    "crdownload", "part", "partial", "tmp", "temp", "download", "opdownload", "!ut", "!bt", "!qb", "bc!", "aria2", "filepart", "downloading",
    "unconfirmed", "crswap", "swp",
];

/// A file that is not finished: a partial download by its extension, or an office / editor lock
/// file by its name.
pub fn is_partial_download(name: &str) -> bool {
    let lower = name.to_lowercase();
    if lower.starts_with("~$") || lower.starts_with(".~lock.") {
        return true;
    }
    let ext = ext_of(&lower);
    PARTIAL_EXTS.contains(&ext)
}

/// A path as text for comparing: backslashes, no `\\?\` prefix, no trailing separator (except
/// of a drive root), case and Turkish i-letters folded.
pub fn norm(p: &Path) -> String {
    let mut s = p.to_string_lossy().replace('/', "\\");
    if let Some(r) = s.strip_prefix(r"\\?\UNC\") {
        s = format!(r"\\{r}");
    } else if let Some(r) = s.strip_prefix(r"\\?\") {
        s = r.to_string();
    }
    while s.len() > 3 && s.ends_with('\\') {
        s.pop();
    }
    fold_case(&s)
}

/// Is `child` the same place as `parent` or somewhere below it? Both from [`norm`].
pub fn is_inside(child: &str, parent: &str) -> bool {
    if child == parent {
        return true;
    }
    if parent.ends_with('\\') {
        return child.starts_with(parent);
    }
    child.len() > parent.len() && child.starts_with(parent) && child.as_bytes()[parent.len()] == b'\\'
}

/// "C:\", "C:" or the root of a share (`\\server\share`).
fn is_root(n: &str) -> bool {
    let b = n.as_bytes();
    if (b.len() == 2 || (b.len() == 3 && b[2] == b'\\')) && b[1] == b':' && b[0].is_ascii_alphabetic() {
        return true;
    }
    if let Some(rest) = n.strip_prefix(r"\\") {
        return rest.split('\\').filter(|s| !s.is_empty()).count() <= 2;
    }
    false
}

/// Why a folder may not be watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    NotAbsolute,
    DriveRoot,
    WindowsFolder,
    ProgramFiles,
    UserProfile,
    AppData,
    /// Where the app keeps its settings and data (or its own program files).
    AppOwn,
}

impl Refusal {
    /// The reason, in a Turkish text the app translates.
    pub fn text(self) -> &'static str {
        match self {
            Refusal::NotAbsolute => T!("İzlenecek klasörü seçin"),
            Refusal::DriveRoot => T!("Bir sürücünün kökü düzenlenemez: daha belirli bir klasör seçin"),
            Refusal::WindowsFolder => T!("Windows klasörü düzenlenemez"),
            Refusal::ProgramFiles => T!("Program Files klasörleri düzenlenemez"),
            Refusal::UserProfile => T!("Kullanıcı klasörünün kendisi düzenlenemez: İndirilenler gibi bir alt klasör seçin"),
            Refusal::AppData => T!("AppData klasörü düzenlenemez: uygulamaların ayarları orada durur"),
            Refusal::AppOwn => T!("rustyfiles'ın kendi ayar ve veri klasörü düzenlenemez"),
        }
    }
}

/// The places the organizer keeps away from. The caller fills them from the environment
/// ([`ProtectedPaths::from_env`]) and adds the app's own folders; tests fill their own.
#[derive(Debug, Clone, Default)]
pub struct ProtectedPaths {
    pub windows: Option<PathBuf>,
    pub program_files: Vec<PathBuf>,
    pub user_profile: Option<PathBuf>,
    /// `%USERPROFILE%\AppData` (Roaming, Local and LocalLow are below it).
    pub app_data: Vec<PathBuf>,
    /// The settings folder and the data folder.
    pub app_dirs: Vec<PathBuf>,
    /// Single files of the app (the program itself). Only the file is the app's: a folder that
    /// happens to hold a portable copy (Downloads) may still be organized.
    pub app_files: Vec<PathBuf>,
}

impl ProtectedPaths {
    /// The Windows folders from the environment variables of the process.
    pub fn from_env() -> Self {
        let var = |n: &str| std::env::var_os(n).filter(|v| !v.is_empty()).map(PathBuf::from);
        let mut program_files: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData"].iter().filter_map(|n| var(n)).collect();
        program_files.dedup();
        let mut app_data: Vec<PathBuf> = Vec::new();
        for n in ["APPDATA", "LOCALAPPDATA"] {
            // ...\AppData\Roaming -> ...\AppData
            if let Some(p) = var(n).and_then(|p| p.parent().map(Path::to_path_buf))
                && !app_data.contains(&p)
            {
                app_data.push(p);
            }
        }
        Self { windows: var("SystemRoot").or_else(|| var("windir")), program_files, user_profile: var("USERPROFILE"), app_data, app_dirs: Vec::new(), app_files: Vec::new() }
    }

    /// May `folder` be watched? `subfolders`: the rule also takes the folders below it, so a
    /// folder that has a protected place somewhere below it is refused too.
    pub fn refuse(&self, folder: &Path, subfolders: bool) -> Option<Refusal> {
        if !folder.is_absolute() {
            return Some(Refusal::NotAbsolute);
        }
        let f = norm(folder);
        if is_root(&f) {
            return Some(Refusal::DriveRoot);
        }
        let hit = |places: &[PathBuf], r: Refusal| -> Option<Refusal> {
            places.iter().map(|p| norm(p)).any(|p| is_inside(&f, &p) || (subfolders && is_inside(&p, &f))).then_some(r)
        };
        if let Some(w) = &self.windows
            && let Some(r) = hit(std::slice::from_ref(w), Refusal::WindowsFolder)
        {
            return Some(r);
        }
        if let Some(r) = hit(&self.program_files, Refusal::ProgramFiles) {
            return Some(r);
        }
        if let Some(r) = hit(&self.app_data, Refusal::AppData) {
            return Some(r);
        }
        if let Some(r) = hit(&self.app_dirs, Refusal::AppOwn) {
            return Some(r);
        }
        if let Some(u) = &self.user_profile {
            let u = norm(u);
            // the profile root itself; with subfolders also a folder that holds it (C:\Users)
            if f == u || (subfolders && is_inside(&u, &f)) {
                return Some(Refusal::UserProfile);
            }
        }
        None
    }

    /// A file of the app's own (settings, journals, backups of a broken settings file...).
    pub fn is_app_file(&self, p: &Path) -> bool {
        if self.app_dirs.is_empty() && self.app_files.is_empty() {
            return false;
        }
        let n = norm(p);
        self.app_dirs.iter().any(|d| is_inside(&n, &norm(d))) || self.app_files.iter().any(|f| norm(f) == n)
    }

    /// May files be put into `dir`? Not into the Windows folder, Program Files, AppData or the
    /// app's own folders: files sent there vanish from the user's sight (and the organizer
    /// itself would call them the app's files and never look at them again).
    pub fn refuse_dest(&self, dir: &Path) -> Option<Refusal> {
        let f = norm(dir);
        let hit = |places: &[PathBuf], r: Refusal| -> Option<Refusal> { places.iter().map(|p| norm(p)).any(|p| is_inside(&f, &p)).then_some(r) };
        if let Some(w) = &self.windows
            && let Some(r) = hit(std::slice::from_ref(w), Refusal::WindowsFolder)
        {
            return Some(r);
        }
        hit(&self.program_files, Refusal::ProgramFiles).or_else(|| hit(&self.app_data, Refusal::AppData)).or_else(|| hit(&self.app_dirs, Refusal::AppOwn))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prot() -> ProtectedPaths {
        ProtectedPaths {
            windows: Some(PathBuf::from(r"C:\Windows")),
            program_files: vec![PathBuf::from(r"C:\Program Files"), PathBuf::from(r"C:\Program Files (x86)"), PathBuf::from(r"C:\ProgramData")],
            user_profile: Some(PathBuf::from(r"C:\Users\Özer")),
            app_data: vec![PathBuf::from(r"C:\Users\Özer\AppData")],
            app_dirs: vec![PathBuf::from(r"C:\Users\Özer\AppData\Roaming\rustyfiles"), PathBuf::from(r"D:\portable\rustyfiles")],
            app_files: vec![PathBuf::from(r"E:\Indirilenler\rustyfiles.exe")],
        }
    }

    #[test]
    fn partial_downloads() {
        for n in ["a.crdownload", "A.CRDOWNLOAD", "film.mp4.part", "x.partial", "x.tmp", "x.download", "x.opdownload", "x.!ut", "x.!qb", "x.bc!", "~$rapor.docx", ".~lock.a.odt#", "indir.aria2", "a.swp"] {
            assert!(is_partial_download(n), "{n}");
        }
        for n in ["a.jpg", "tmp.jpg", "part.txt", "rapor.docx", "download", "a.tmpl", ".gitignore"] {
            assert!(!is_partial_download(n), "{n}");
        }
    }

    #[test]
    fn norm_and_inside() {
        assert_eq!(norm(Path::new(r"\\?\C:\Users\ALİ\")), norm(Path::new("c:/users/ali")));
        assert_eq!(norm(Path::new(r"\\?\UNC\srv\share\x")), norm(Path::new(r"\\srv\share\x")));
        assert_eq!(norm(Path::new(r"C:\")), r"c:\");
        assert!(is_inside(&norm(Path::new(r"C:\a\b")), &norm(Path::new(r"C:\A"))));
        assert!(is_inside(&norm(Path::new(r"C:\a")), &norm(Path::new(r"C:\a"))));
        assert!(!is_inside(&norm(Path::new(r"C:\ab")), &norm(Path::new(r"C:\a"))));
        assert!(is_inside(&norm(Path::new(r"C:\a")), &norm(Path::new(r"C:\"))));
        // Turkish letters fold: İ, I, ı and i are one letter
        assert!(is_inside(&norm(Path::new(r"C:\Işık\x")), &norm(Path::new(r"C:\İŞİK"))));
        assert_eq!(norm(Path::new(r"C:\İlk")), norm(Path::new(r"C:\ilk")));
        assert_eq!(norm(Path::new(r"C:\ILIK")), norm(Path::new(r"C:\ılık")));
    }

    #[test]
    fn folders_that_are_refused() {
        let p = prot();
        let r = |s: &str, sub: bool| p.refuse(Path::new(s), sub);
        assert_eq!(r(r"C:\", false), Some(Refusal::DriveRoot));
        assert_eq!(r(r"d:\", false), Some(Refusal::DriveRoot));
        assert_eq!(r("d:", false), Some(Refusal::NotAbsolute));
        assert_eq!(r(r"\\srv\share\", false), Some(Refusal::DriveRoot));
        assert_eq!(r(r"\\srv\share\Belgeler", false), None);
        assert_eq!(r("Downloads", false), Some(Refusal::NotAbsolute));
        assert_eq!(r(r"C:\Windows", false), Some(Refusal::WindowsFolder));
        assert_eq!(r(r"c:\windows\Temp", false), Some(Refusal::WindowsFolder));
        assert_eq!(r(r"C:\Program Files", false), Some(Refusal::ProgramFiles));
        assert_eq!(r(r"C:\Program Files (x86)\Foo", false), Some(Refusal::ProgramFiles));
        assert_eq!(r(r"C:\ProgramData", false), Some(Refusal::ProgramFiles));
        assert_eq!(r(r"C:\Users\Özer", false), Some(Refusal::UserProfile));
        assert_eq!(r(r"c:\users\özer\", false), Some(Refusal::UserProfile));
        assert_eq!(r(r"C:\Users\Özer\AppData", false), Some(Refusal::AppData));
        assert_eq!(r(r"C:\Users\Özer\AppData\Local\Temp", false), Some(Refusal::AppData));
        assert_eq!(r(r"C:\Users\Özer\AppData\Roaming\rustyfiles\jobs", false), Some(Refusal::AppData));
        assert_eq!(r(r"D:\portable\rustyfiles", false), Some(Refusal::AppOwn));
        assert_eq!(r(r"D:\portable\rustyfiles\index", false), Some(Refusal::AppOwn));
        // fine
        assert_eq!(r(r"C:\Users\Özer\Downloads", false), None);
        assert_eq!(r(r"C:\Users\Özer\Desktop", true), None);
        assert_eq!(r(r"D:\Belgeler", false), None);
        assert_eq!(r(r"D:\portable", false), None);
        // with subfolders a folder that holds a protected place is refused as well
        assert_eq!(r(r"D:\portable", true), Some(Refusal::AppOwn));
        assert_eq!(r(r"C:\Users", true), Some(Refusal::AppData));
        assert_eq!(r(r"C:\Users", false), None);
        assert_eq!(r(r"C:\Users\Özer", true), Some(Refusal::AppData));
    }

    #[test]
    fn the_apps_own_files() {
        let p = prot();
        assert!(p.is_app_file(Path::new(r"C:\Users\Özer\AppData\Roaming\rustyfiles\settings.toml")));
        assert!(p.is_app_file(Path::new(r"d:\PORTABLE\rustyfiles\x.bozuk-1")));
        assert!(!p.is_app_file(Path::new(r"C:\Users\Özer\Downloads\settings.toml")));
        assert!(!ProtectedPaths::default().is_app_file(Path::new(r"C:\x")));
    }

    #[test]
    fn a_program_run_from_a_folder_does_not_make_the_folder_the_apps_own() {
        let p = prot();
        // only the program file itself is left alone
        assert!(p.is_app_file(Path::new(r"e:\indirilenler\RUSTYFILES.EXE")));
        assert!(!p.is_app_file(Path::new(r"E:\Indirilenler\foto.jpg")));
        assert_eq!(p.refuse(Path::new(r"E:\Indirilenler"), false), None);
        assert_eq!(p.refuse(Path::new(r"E:\Indirilenler"), true), None);
    }

    #[test]
    fn files_may_not_be_sent_into_protected_places() {
        let p = prot();
        let r = |s: &str| p.refuse_dest(Path::new(s));
        assert_eq!(r(r"C:\Windows\System32"), Some(Refusal::WindowsFolder));
        assert_eq!(r(r"C:\Program Files\Foo"), Some(Refusal::ProgramFiles));
        assert_eq!(r(r"C:\Users\Özer\AppData\Roaming\Foo"), Some(Refusal::AppData));
        assert_eq!(r(r"D:\portable\rustyfiles\x"), Some(Refusal::AppOwn));
        // the profile folder itself and its ordinary folders are fine places to send files to
        assert_eq!(r(r"C:\Users\Özer"), None);
        assert_eq!(r(r"C:\Users\Özer\Pictures\2026"), None);
        assert_eq!(r(r"D:\Arşiv"), None);
        assert_eq!(r(r"C:\"), None);
    }

    #[test]
    fn from_env_has_the_windows_folders() {
        let p = ProtectedPaths::from_env();
        assert!(p.windows.is_some());
        assert!(!p.program_files.is_empty());
    }
}
