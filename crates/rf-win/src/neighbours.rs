//! Opening a file in a packaged app (Photos, Media Player) together with the files beside it, so the app's Previous and Next walk the
//! folder as they do when the file is opened from Explorer.
//!
//! WHY (issue 70, and a user on Discord, 2026-11-21, on 0.9.4: "in Windows native photos app I can't navigate to the next/previous
//! media in the same folder"). A packaged app started through `ShellExecuteEx` gets the one file. Explorer tells it the rest through
//! WinRT's launcher: `LauncherOptions.NeighboringFilesQuery`, a query over the file's folder. Files does the same. A desktop program
//! (IrfanView, VLC) reads its folder on its own and keeps going through the shell as before.

use std::path::Path;

use windows::Storage::Search::{FolderDepth, QueryOptions};
use windows::Storage::{StorageFile, StorageFolder};
use windows::System::{Launcher, LauncherOptions};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::Win32::UI::Shell::{ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_PROGID, AssocQueryStringW};
use windows::core::{HSTRING, PCWSTR, PWSTR, w};
use windows_collections::IIterable;

use crate::wide::{from_wide, wide};

/// Whether the program Windows opens files of this kind with is one of Windows' own media apps (Photos, Media Player): a packaged
/// app (its ProgID is an `AppX…` one) of a family in [`walks_its_neighbours`].
///
/// WHY only those (issue 189: "Double clicking on an image opens the version info page of Pictureflect, not the image", on 0.9.5;
/// 0.9.4 opened it). Another packaged app may do anything with the launcher's activation, so it is started through the shell as
/// before.
pub fn opens_in_packaged_app(path: &Path) -> bool {
    let Some(ext) = path.extension() else { return false };
    let ext = wide(format!(".{}", ext.to_string_lossy()));
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    let ok = unsafe { AssocQueryStringW(ASSOCF_INIT_IGNOREUNKNOWN, ASSOCSTR_PROGID, PCWSTR(ext.as_ptr()), w!("open"), Some(PWSTR(buf.as_mut_ptr())), &mut len) };
    let progid = from_wide(&buf);
    ok.is_ok()
        && is_packaged_progid(&progid)
        && crate::bench::reg_string(windows::Win32::System::Registry::HKEY_CLASSES_ROOT, &format!("{progid}\\Application"), "AppUserModelID")
            .is_some_and(|id| walks_its_neighbours(&id))
}

/// The packaged apps that are told about a file's neighbours: Windows' Photos (and its older version), Media Player and Movies & TV,
/// by the family of their AppUserModelID. Pure.
pub fn walks_its_neighbours(aumid: &str) -> bool {
    let family = aumid.split('!').next().unwrap_or("").to_ascii_lowercase();
    ["microsoft.windows.photos_", "microsoft.photoslegacy_", "microsoft.zunemusic_", "microsoft.zunevideo_"].iter().any(|f| family.starts_with(f))
}

/// `AppX43hnxtbyyps62jhe9sqpdzxn1790zetc` is a packaged app's file type; `jpegfile` or `IrfanView.jpg` are not. Pure.
pub fn is_packaged_progid(progid: &str) -> bool {
    progid.len() > 4 && progid[..4].eq_ignore_ascii_case("appx")
}

/// The Windows property a folder's sort column is (for the order the app walks the files in). The columns Windows has no property
/// for walk by name.
pub fn sort_property(key: rf_core::sort::SortKey) -> &'static str {
    use rf_core::sort::SortKey;
    match key {
        SortKey::Modified => "System.DateModified",
        SortKey::Created => "System.DateCreated",
        SortKey::Size => "System.Size",
        SortKey::Type => "System.ItemTypeText",
        _ => "System.ItemNameDisplay",
    }
}

/// The helper process's argument: `rustyfiles.exe --open-with-folder <file> <sort property> <1|0>`.
pub const ARG: &str = "--open-with-folder";

/// Opens `path` with its default app, telling the app about the other files of its folder in this order. `Err` leaves the opening
/// to the caller.
///
/// IN A HELPER PROCESS THAT ENDS AT ONCE, found by trying (Photos' film strip read through UI Automation): the query handed to the
/// app is an object of the process that made it, and Photos, which runs apart, could not read it from rustyfiles while rustyfiles
/// was running (one picture in its strip, from the app and from a probe kept alive 25 s after the launch). When the process that
/// made it is gone, Photos lists the folder itself, and all the pictures are there (three of three, from a probe that ended 0.1 s
/// after the launch). Explorer's own process lets Photos in; ours does not, and opening COM's door for every packaged app in the
/// whole file manager is a bigger change than this. So a copy of the program is started for the launch and ends after it.
pub fn launch(path: &Path, property: &str, ascending: bool) -> anyhow::Result<()> {
    use std::os::windows::process::CommandExt;
    let exe = std::env::current_exe()?;
    // (only the file manager's exe answers the argument: DirectLook's would show a window)
    if !exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case("rustyfiles.exe")) {
        anyhow::bail!("not the file manager's process");
    }
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut child = std::process::Command::new(exe)
        .arg(ARG)
        .arg(path)
        .arg(property)
        .arg(if ascending { "1" } else { "0" })
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() { Ok(()) } else { Err(anyhow::anyhow!("the helper ended with {status}")) };
        }
        if std::time::Instant::now() > until {
            let _ = child.kill();
            anyhow::bail!("the helper did not end");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// The helper process (`ARG`): launches and returns its exit code; the process must end right after.
pub fn helper_main() -> i32 {
    let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let (Some(path), Some(property), Some(asc)) = (args.get(2), args.get(3), args.get(4)) else { return 2 };
    let property = property.to_string_lossy();
    match launch_in_this_process(Path::new(path), &property, asc.to_string_lossy() == "1") {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// The launch itself, from this process (its query lives here: see [`launch`]).
pub fn launch_in_this_process(path: &Path, property: &str, ascending: bool) -> anyhow::Result<()> {
    let (path, property) = (path.to_path_buf(), property.to_string());
    std::thread::spawn(move || {
        let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let r = launch_here(&path, &property, ascending).map(|_| ());
        unsafe { CoUninitialize() };
        r
    })
    .join()
    .map_err(|_| anyhow::anyhow!("the launcher's thread panicked"))?
}

fn launch_here(path: &Path, property: &str, ascending: bool) -> anyhow::Result<(LauncherOptions, windows::Storage::Search::StorageFileQueryResult)> {
    let dir = path.parent().ok_or_else(|| anyhow::anyhow!("no folder"))?;
    let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(path.as_os_str()))?.join()?;
    let folder = StorageFolder::GetFolderFromPathAsync(&HSTRING::from(dir.as_os_str()))?.join()?;
    let no_filter: IIterable<HSTRING> = Vec::<HSTRING>::new().into();
    let options = QueryOptions::CreateCommonFileQuery(windows::Storage::Search::CommonFileQuery::DefaultQuery, &no_filter)?;
    options.SetFolderDepth(FolderDepth::Shallow)?;
    let order = options.SortOrder()?;
    order.Clear()?;
    order.Append(&windows::Storage::Search::SortEntry { PropertyName: HSTRING::from(property), AscendingOrder: ascending })?;
    let query = folder.CreateFileQueryWithOptions(&options)?;
    let launch = LauncherOptions::new()?;
    launch.SetNeighboringFilesQuery(&query)?;
    if Launcher::LaunchFileWithOptionsAsync(&file, &launch)?.join()? {
        Ok((launch, query))
    } else {
        anyhow::bail!("the launcher said no")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_an_appx_progid_is_a_packaged_app() {
        assert!(is_packaged_progid("AppX43hnxtbyyps62jhe9sqpdzxn1790zetc"));
        assert!(is_packaged_progid("appx6eg8h5sxqq90pv53845wmnbewywdqq5h"));
        assert!(!is_packaged_progid("jpegfile"));
        assert!(!is_packaged_progid("IrfanView.jpg"));
        assert!(!is_packaged_progid("AppX"));
        assert!(!is_packaged_progid(""));
        assert!(walks_its_neighbours("Microsoft.Windows.Photos_8wekyb3d8bbwe!App"));
        assert!(walks_its_neighbours("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"));
        assert!(!walks_its_neighbours("57469Pictureflect.PictureflectPhotoViewer_xxxx!App"), "another app opens through the shell");
        assert!(!walks_its_neighbours(""));
    }

    /// Opens a picture in its default app with its folder from this process, which ends right after (as the helper does)
    /// (`RF_NEIGHBOUR_FILE=<a picture>`), for a look at the app's film strip.
    /// `cargo test -p rf-win --lib neighbours::tests::open_with_folder -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn open_with_folder() {
        let Ok(f) = std::env::var("RF_NEIGHBOUR_FILE") else { return };
        let p = Path::new(&f);
        println!("packaged: {}", opens_in_packaged_app(p));
        println!("{:?}", launch_in_this_process(p, sort_property(rf_core::sort::SortKey::Name), true));
    }

    /// Which file types this PC opens in an app that is told about the neighbours. A look: `-- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn which_types_walk_their_folder_here() {
        for ext in ["jpg", "png", "heic", "mp4", "mp3", "pdf", "txt"] {
            println!("{ext}: {}", opens_in_packaged_app(Path::new(&format!("x.{ext}"))));
        }
    }
}
