//! Shared Windows launch path for toolbar commands and file drops.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

use gpui_kit::{App, Window};

use crate::i18n::tx;

pub(crate) fn split_arguments(text: &str) -> anyhow::Result<Vec<OsString>> {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::UI::Shell::CommandLineToArgvW;
    use windows::core::PCWSTR;

    // Windows parses argv[0] differently, so supply a dummy executable name.
    anyhow::ensure!(!text.contains('\0'), "Invalid command arguments");
    let line = format!("program {text}");
    let wide: Vec<u16> = OsStr::new(&line).encode_wide().chain([0]).collect();
    let mut count = 0;
    unsafe {
        let argv = CommandLineToArgvW(PCWSTR(wide.as_ptr()), &mut count);
        if argv.is_null() {
            return Err(std::io::Error::last_os_error().into());
        }
        let args = std::slice::from_raw_parts(argv, count as usize).iter().skip(1)
            .map(|p| OsString::from_wide(p.as_wide())).collect();
        let _ = LocalFree(Some(HLOCAL(argv.cast())));
        Ok(args)
    }
}

pub(crate) fn accepts_program(path: &Path) -> bool {
    path.extension().is_some_and(|e| ["exe", "com", "bat", "cmd", "lnk"].iter().any(|ext| e.eq_ignore_ascii_case(ext)))
}

/// Starts an executable or a batch script, or returns the directory a shortcut points to.
/// Run off the UI thread. Paths are passed as individual arguments.
pub(crate) fn launch(program: PathBuf, arguments: Vec<OsString>, uses_paths: bool, folder: Option<PathBuf>) -> anyhow::Result<Option<PathBuf>> {
    let (exe, existing, working_dir) = if program.extension().is_some_and(|e| e.eq_ignore_ascii_case("lnk")) {
        let shortcut = rf_win::shortcut::resolve(&program)?;
        if shortcut.target.is_dir() {
            return Ok(Some(shortcut.target));
        }
        (shortcut.target, split_arguments(&shortcut.args)?, Some(shortcut.working_dir))
    } else {
        (program, Vec::new(), None)
    };
    anyhow::ensure!(exe.is_absolute() && accepts_program(&exe) && rf_win::runfind::is_program_file(&exe),
        "{}", tx("Program bulunamadı.", "Program not found."));
    // Command can quote paths for an executable or a batch file. Explicit shell command
    // evaluation has its own syntax and must not interpret characters from filenames.
    if uses_paths && exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("cmd")) {
        anyhow::ensure!(!arguments.iter().any(|a| a.to_string_lossy().contains(['&', '|', '<', '>', '^', '%', '!', '(', ')', '"', '\r', '\n'])),
            "{}", tx("Bu bağımsız değişkenleri doğrudan bir .bat veya .cmd dosyasına verin.", "Pass these arguments directly to a .bat or .cmd file."));
    }
    if uses_paths && exe.file_stem().is_some_and(|s| s.eq_ignore_ascii_case("powershell") || s.eq_ignore_ascii_case("pwsh")) {
        anyhow::ensure!(!existing.iter().chain(&arguments).any(|a| {
            let a = a.to_string_lossy().to_ascii_lowercase();
            a.starts_with('-') && (a == "-c" || "-command".starts_with(&a) && a.len() > 2 || a == "-commandwithargs")
        }), "{}", tx("Dosya yollarını geçirmek için PowerShell -File kullanın.", "Use PowerShell -File to pass file paths."));
    }
    let mut command = std::process::Command::new(&exe);
    command.args(existing).args(arguments);
    if let Some(dir) = working_dir.filter(|p| p.is_dir()).or(folder).or_else(|| exe.parent().map(Path::to_path_buf)) {
        command.current_dir(dir);
    }
    command.spawn()?;
    Ok(None)
}

pub(crate) fn launch_error(error: anyhow::Error, window: &mut Window, cx: &mut App) {
    crate::prompt::notice(tx("Program başlatılamadı", "Could not start the program"), error.to_string(), window, cx);
}

#[cfg(test)]
mod tests {
    use super::launch;

    #[test]
    fn batch_shortcut_receives_a_file_path_with_spaces_and_ampersand() {
        let dir = std::env::temp_dir().join(format!("rf-quick-command-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let input = dir.join("file & name.txt");
        let output = dir.join("copied.txt");
        let batch = dir.join("receive.bat");
        let shortcut = dir.join("receive.lnk");
        std::fs::write(&input, "test data").unwrap();
        std::fs::write(&batch, b"@copy /Y \"%~1\" \"%~dp0copied.txt\" >nul\r\n").unwrap();
        rf_win::shortcut::create(&shortcut, &batch).unwrap();
        assert_eq!(launch(shortcut, vec![input.into_os_string()], true, None).unwrap(), None);
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        let copied = loop {
            if let Ok(contents) = std::fs::read_to_string(&output)
                && !contents.is_empty()
            {
                break contents;
            }
            assert!(std::time::Instant::now() < until, "the batch file did not copy the argument");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!(copied, "test data");

        // cmd.exe can still hold the batch file open after writing the output.
        let until = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => break,
                Err(err) if err.raw_os_error() == Some(32) && std::time::Instant::now() < until => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                Err(err) => panic!("could not clean up the batch test: {err}"),
            }
        }
    }
}
