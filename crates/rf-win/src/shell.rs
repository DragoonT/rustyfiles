//! Shell actions (open, properties). Each call initializes COM on the calling thread,
//! so run them on a worker thread — shell extensions can block for seconds.

use std::path::Path;

use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx};
use windows::Win32::UI::Shell::{
    SEE_MASK_FLAG_NO_UI, SEE_MASK_INVOKEIDLIST, SEE_MASK_NOASYNC, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
use windows::core::PCWSTR;

use crate::wide::wide;

/// Initializes an STA on the current thread (idempotent; errors ignored when already initialized).
pub fn ensure_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE);
    }
}

fn exec(path: &Path, verb: Option<&str>, args: Option<&str>) -> anyhow::Result<()> {
    ensure_com();
    let file = wide(path);
    let verb_w = verb.map(wide);
    let args_w = args.map(wide);
    let dir = path.parent().map(wide);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_INVOKEIDLIST | SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpVerb: verb_w.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: args_w.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        lpDirectory: dir.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

/// Starts a program (an `.exe`, a script, a shortcut) with a command line of its own and a working directory: what "Open with" does
/// for a program the user named. No error box of the shell's: the caller says what went wrong.
pub fn run_program(program: &Path, args: &str, dir: Option<&Path>) -> anyhow::Result<()> {
    ensure_com();
    let (file, args_w, dir_w) = (wide(program), wide(args), dir.map(wide));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(args_w.as_ptr()),
        lpDirectory: dir_w.as_ref().map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        nShow: SW_SHOWNORMAL.0,
        ..Default::default()
    };
    unsafe { ShellExecuteExW(&mut info)? };
    Ok(())
}

/// Tells Windows that the user opened this file: it goes to Windows' recent items (Home, Start, the jump lists), as a file opened
/// from Explorer does. Windows keeps that list for what Explorer and the file dialogs open; a file opened by another program is in it
/// only when that program says so (issue 341). Nothing happens when the user switched the recent items off.
pub fn note_recent(path: &Path) {
    use windows::Win32::UI::Shell::{SHARD_PATHW, SHAddToRecentDocs};
    let file = wide(path);
    unsafe { SHAddToRecentDocs(SHARD_PATHW.0 as u32, Some(file.as_ptr().cast())) };
}

/// Opens a file with its default handler (same as double-click in Explorer).
pub fn open(path: &Path) -> anyhow::Result<()> {
    exec(path, None, None)
}

/// Shows the shell's "Open with" chooser for `path`, and opens the file with the app picked there (with "Always" the app becomes the
/// type's default). Modal: run it on the STA shell thread.
///
/// Through `SHOpenWithDialog`, not the `openas` verb: types such as `.cmd`, `.bat` and `.exe` have no `openas` verb, and asking for
/// it failed without a word (issue 207: "When I do a right click on a .cmd file and select open with, nothing appear"). The verb is
/// still tried when the chooser cannot be shown at all. Cancelling the chooser is not an error.
pub fn open_with(path: &Path) -> anyhow::Result<()> {
    use windows::Win32::Foundation::ERROR_CANCELLED;
    use windows::Win32::UI::Shell::{OAIF_ALLOW_REGISTRATION, OAIF_EXEC, OPENASINFO, SHOpenWithDialog};
    ensure_com();
    let file = wide(path);
    let info = OPENASINFO { pcszFile: PCWSTR(file.as_ptr()), pcszClass: PCWSTR::null(), oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_EXEC };
    // SAFETY: info and the path buffer outlive the modal call
    match unsafe { SHOpenWithDialog(None, &info) } {
        Ok(()) => Ok(()),
        Err(e) if e.code() == ERROR_CANCELLED.to_hresult() => Ok(()),
        Err(e) => {
            log::warn!("open with: the chooser could not be shown ({e}); trying the openas verb");
            exec(path, Some("openas"), None)
        }
    }
}

/// Shows the Windows properties dialog for the item.
pub fn properties(path: &Path) -> anyhow::Result<()> {
    exec(path, Some("properties"), None)
}

/// Settings > Developer tools > "Terminal": the program "Open in Terminal" starts (issue 224). Empty: Windows Terminal, else PowerShell.
static TERMINAL: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Sets the terminal [`open_terminal`] starts: a program's path or name (`cmd.exe`, `pwsh.exe`, `C:\Tools\Cmder\Cmder.exe`), or empty
/// for the automatic one.
pub fn set_terminal(program: &str) {
    *TERMINAL.lock().unwrap_or_else(|e| e.into_inner()) = program.trim().trim_matches('"').to_string();
}

/// The arguments that make a terminal `program` (its file name, lower case) start in `dir`, for the ones that do not take the folder
/// they are started in (Cmder and ConEmu keep their own start folder). Others are only started in `dir`.
fn terminal_dir_args(program: &str, dir: &str) -> Vec<String> {
    match program {
        "cmder.exe" => vec!["/START".into(), dir.into()],
        "conemu.exe" | "conemu64.exe" => vec!["-Dir".into(), dir.into()],
        "wt.exe" | "windowsterminal.exe" => vec!["-d".into(), dir.into()],
        "wezterm.exe" | "wezterm-gui.exe" => vec!["start".into(), "--cwd".into(), dir.into()],
        "alacritty.exe" => vec!["--working-directory".into(), dir.into()],
        "pwsh.exe" | "powershell.exe" => vec!["-NoExit".into()],
        _ => Vec::new(),
    }
}

/// Opens a terminal in `dir`: the one chosen in Settings, else Windows Terminal if present, else PowerShell.
pub fn open_terminal(dir: &Path) -> anyhow::Result<()> {
    let chosen = TERMINAL.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if !chosen.is_empty() {
        let name = Path::new(&chosen).file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let name = if name.ends_with(".exe") || name.contains('.') { name } else { format!("{name}.exe") };
        let args = terminal_dir_args(&name, &dir.to_string_lossy());
        match std::process::Command::new(&chosen).args(&args).current_dir(dir).spawn() {
            Ok(_) => return Ok(()),
            // (a terminal that is gone: the automatic one rather than nothing)
            Err(e) => log::warn!("terminal {chosen}: {e}"),
        }
    }
    let d = dir.to_string_lossy().to_string();
    let wt = std::process::Command::new("wt.exe").args(["-d", &d]).spawn();
    if wt.is_err() {
        std::process::Command::new("powershell.exe").current_dir(dir).spawn()?;
    }
    Ok(())
}

/// Opens an elevated terminal in `dir` (Windows asks for permission: UAC). Windows Terminal if
/// present, else PowerShell. Declining the permission prompt is not an error.
pub fn open_terminal_admin(dir: &Path) -> anyhow::Result<()> {
    let run = |file: &str, args: &str| -> windows::core::Result<()> {
        ensure_com();
        let (file, args, verb, cwd) = (wide(file), wide(args), wide("runas"), wide(dir));
        let mut info = SHELLEXECUTEINFOW {
            cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
            fMask: SEE_MASK_NOASYNC | SEE_MASK_FLAG_NO_UI,
            lpVerb: PCWSTR(verb.as_ptr()),
            lpFile: PCWSTR(file.as_ptr()),
            lpParameters: PCWSTR(args.as_ptr()),
            lpDirectory: PCWSTR(cwd.as_ptr()),
            nShow: SW_SHOWNORMAL.0,
            ..Default::default()
        };
        unsafe { ShellExecuteExW(&mut info) }
    };
    // ERROR_CANCELLED: the user said no at the prompt (asking again with PowerShell would nag)
    let declined = |e: &windows::core::Error| e.code().0 as u32 == 0x8007_04C7;
    match run("wt.exe", &terminal_args(dir)) {
        Ok(()) => Ok(()),
        Err(e) if declined(&e) => Ok(()),
        Err(_) => match run("powershell.exe", &powershell_args(dir)) {
            Ok(()) => Ok(()),
            Err(e) if declined(&e) => Ok(()),
            Err(e) => Err(e.into()),
        },
    }
}

/// `-d "C:\some dir"` for Windows Terminal. A trailing backslash would escape the closing
/// quote (`"C:\"`), so it is doubled.
fn terminal_args(dir: &Path) -> String {
    let d = dir.to_string_lossy();
    let tail = if d.ends_with('\\') { "\\" } else { "" };
    format!("-d \"{d}{tail}\"")
}

/// An elevated process starts in System32 whatever directory is asked for: PowerShell is told
/// to go to the folder itself (single quotes doubled, as PowerShell escapes them).
fn powershell_args(dir: &Path) -> String {
    format!("-NoExit -Command \"Set-Location -LiteralPath '{}'\"", dir.to_string_lossy().replace('\'', "''"))
}

/// `shell:Downloads`, `shell:startup`, `shell:sendto`… → file-system path (None for virtual
/// folders like `shell:RecycleBinFolder`). Blocking (shell namespace): off the UI thread.
pub fn resolve_shell_path(s: &str) -> Option<std::path::PathBuf> {
    use windows::Win32::UI::Shell::{IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH};
    ensure_com();
    let w = crate::wide::wide(s.trim());
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(windows::core::PCWSTR(w.as_ptr()), None).ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let out = p.to_string().ok();
        windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
        out.map(std::path::PathBuf::from)
    }
}

#[cfg(test)]
mod shell_path_tests {
    #[test]
    fn resolves_known_shell_paths() {
        let p = super::resolve_shell_path("shell:Windows").expect("shell:Windows");
        assert!(p.to_string_lossy().to_lowercase().ends_with("windows"), "{p:?}");
        assert!(super::resolve_shell_path("shell:RecycleBinFolder").is_none());
    }
}

/// No "There is no disk in the drive" / "Insert a disk" system boxes when touching an empty
/// card reader or an ejected disc: such calls just fail (and the app shows "drive not ready").
/// Process-wide; call once at startup.
pub fn suppress_critical_error_dialogs() {
    use windows::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SEM_NOOPENFILEERRORBOX, SetErrorMode};
    unsafe {
        SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOOPENFILEERRORBOX);
    }
}

/// Tells the shell (Explorer windows, desktop, file dialogs) that `dir`'s contents changed,
/// the way the shell's own copy engine does when it finishes.
pub fn notify_updated(dir: &std::path::Path) {
    use windows::Win32::UI::Shell::{SHCNE_UPDATEDIR, SHCNF_PATHW, SHChangeNotify};
    let w = crate::wide::wide(dir);
    // SAFETY: a NUL-terminated wide path, read during the call
    unsafe { SHChangeNotify(SHCNE_UPDATEDIR, SHCNF_PATHW, Some(w.as_ptr() as *const _), None) };
}

/// Explorer's "Change..." next to "Opens with": the shell's app chooser for `path`, which on OK makes the
/// chosen app the default for the extension (OAIF_REGISTER_EXT + OAIF_FORCE_REGISTRATION) without opening the
/// file (no OAIF_EXEC). Modal: run it on the STA shell thread.
pub fn open_with_dialog(owner: windows::Win32::Foundation::HWND, path: &Path) -> anyhow::Result<()> {
    use windows::Win32::UI::Shell::{OAIF_ALLOW_REGISTRATION, OAIF_FORCE_REGISTRATION, OAIF_REGISTER_EXT, OPENASINFO, SHOpenWithDialog};
    ensure_com();
    let file = wide(path);
    let info = OPENASINFO { pcszFile: PCWSTR(file.as_ptr()), pcszClass: PCWSTR::null(), oaifInFlags: OAIF_ALLOW_REGISTRATION | OAIF_REGISTER_EXT | OAIF_FORCE_REGISTRATION };
    // SAFETY: info and the path buffer outlive the modal call
    unsafe { SHOpenWithDialog(Some(owner), &info)? };
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn elevated_terminal_arguments() {
        assert_eq!(super::terminal_args(Path::new(r"C:\Users\a b")), r#"-d "C:\Users\a b""#);
        // a drive root: the backslash must not escape the closing quote
        assert_eq!(super::terminal_args(Path::new(r"D:\")), r#"-d "D:\\""#);
        assert_eq!(super::powershell_args(Path::new(r"C:\it's")), r#"-NoExit -Command "Set-Location -LiteralPath 'C:\it''s'""#);
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::terminal_dir_args;

    #[test]
    fn terminals_that_keep_their_own_start_folder_are_told_the_folder() {
        assert_eq!(terminal_dir_args("cmder.exe", r"C:\work"), ["/START", r"C:\work"]);
        assert_eq!(terminal_dir_args("conemu64.exe", r"C:\work"), ["-Dir", r"C:\work"]);
        assert_eq!(terminal_dir_args("wt.exe", r"C:\work"), ["-d", r"C:\work"]);
        assert!(terminal_dir_args("cmd.exe", r"C:\work").is_empty());
    }
}

#[cfg(test)]
mod recent_tests {
    /// Live: one shortcut is written to this PC's recent items and taken out again. Windows leaves out files in the temporary
    /// folder, so the file stands beside the test program.
    #[test]
    #[ignore = "writes one entry to this PC's recent items and removes it again"]
    fn a_noted_file_is_one_of_windows_recent_items() {
        let dir = std::env::current_exe().unwrap().parent().unwrap().join("rf-recent-probe");
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rf-recent-probe-341.txt");
        std::fs::write(&file, "probe").unwrap();
        let link = crate::dock::startrecent::recent_dir().unwrap().join("rf-recent-probe-341.txt.lnk");
        let _ = std::fs::remove_file(&link);
        super::note_recent(&file);
        let t = std::time::Instant::now();
        while !link.exists() && t.elapsed() < std::time::Duration::from_secs(5) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let there = link.exists();
        let _ = std::fs::remove_file(&link);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(there, "no shortcut in the recent items after {:?}", t.elapsed());
    }
}
