//! Run commands for the search, and the second name an app is found by.
//!
//! A user's report: typing `cmd` into the dock's search found no Command Prompt (its display name on a Turkish Windows is
//! "Komut İstemi", and apps were matched by their display name only), and `dxdiag` found nothing at all (it is no Start menu app).
//! Windows' own search finds both. So:
//!
//! * an app is also found by the file name of its program ([`app_file_stem`]: the AppsFolder id of a classic app ends in the
//!   program's path), and
//! * a query that is the bare name of a program the Run box would start gets a row for that program ([`resolve`]): `dxdiag`,
//!   `regedit`, `msconfig`, `services.msc`, `appwiz.cpl`. The name is looked for the way the address field's commands are
//!   (`rf_win::runfind`: System32, the Windows folder, `PATH`, the app execution aliases, then `App Paths`), read-only.
//!
//! The query is only ever COMPARED with file names. Nothing typed is run as a command line: a query with a space, a path separator,
//! a quote or any other sign is no name ([`is_bare_name`]), and the row starts the file that was found, with no arguments, when the
//! user chooses it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// How long an answer is remembered (found or not): a search asks for the same name at every keystroke after it.
const REMEMBER: Duration = Duration::from_secs(300);
/// The most answers remembered; past this the memory is emptied (names are short, the bound is for sanity).
const MOST: usize = 256;

/// A bare program name: 2 to 64 letters, digits, dots, dashes and underscores, beginning with a letter or a digit. No space, no
/// path, no quote, no wildcard: anything else is not a name and gets no row.
pub fn is_bare_name(q: &str) -> bool {
    let n = q.chars().count();
    (2..=64).contains(&n) && q.chars().next().is_some_and(|c| c.is_ascii_alphanumeric()) && q.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')) && !q.ends_with('.')
}

/// The program's file name without its extension, lower case, from the AppsFolder id of a classic app (`{GUID}\cmd.exe`,
/// `C:\Program Files\App\app.exe`). `None` for a packaged app's id and for anything that does not end in `.exe`.
pub fn app_file_stem(id: &str) -> Option<String> {
    let file = id.rsplit(['\\', '/']).next()?;
    let lower = file.to_lowercase();
    let stem = lower.strip_suffix(".exe")?;
    (id.contains(['\\', '/']) && !stem.is_empty()).then(|| stem.to_string())
}

/// The file names a bare name stands for, lower case: the name itself and, when it has no extension of its own, the name with
/// `.exe` (what an app's program is compared with, so that `cmd` and `cmd.exe` both mean the app whose program is `cmd.exe`).
pub fn stem_of(q: &str) -> String {
    let lower = q.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

static MEMO: Mutex<Option<HashMap<String, (Instant, Option<PathBuf>)>>> = Mutex::new(None);

/// The program file a bare name stands for, the way the Run box would find it, or `None`. Blocks (file system and registry reads):
/// call it off the UI thread. Remembered for five minutes.
pub fn resolve(q: &str) -> Option<PathBuf> {
    let q = q.trim();
    if !is_bare_name(q) {
        return None;
    }
    let key = q.to_ascii_lowercase();
    if let Some((at, found)) = MEMO.lock().unwrap_or_else(|p| p.into_inner()).as_ref().and_then(|m| m.get(&key).cloned())
        && at.elapsed() < REMEMBER
    {
        return found;
    }
    let found = look(q);
    let mut memo = MEMO.lock().unwrap_or_else(|p| p.into_inner());
    let map = memo.get_or_insert_with(HashMap::new);
    if map.len() >= MOST {
        map.clear();
    }
    map.insert(key, (Instant::now(), found.clone()));
    found
}

fn look(name: &str) -> Option<PathBuf> {
    let lower = name.to_ascii_lowercase();
    // a management console or a Control Panel applet by its full file name: only in the two system folders
    if lower.ends_with(".msc") || lower.ends_with(".cpl") {
        return rf_win::runfind::search_dirs().into_iter().take(2).map(|d| d.join(name)).find(|p| rf_win::runfind::is_program_file(p));
    }
    let line = rf_core::addrcmd::CommandLine { program: name.to_string(), args: String::new() };
    rf_win::runfind::find(&[line], Path::new("")).map(|f| f.exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bare_name_is_a_name() {
        for yes in ["cmd", "dxdiag", "services.msc", "appwiz.cpl", "python3.11", "wt", "7z", "my-tool_2"] {
            assert!(is_bare_name(yes), "{yes}");
        }
        for no in ["", "c", "cmd /c del x", "cmd.exe /k", r"C:\Windows\cmd.exe", "..\\cmd", "tools/x", "\"cmd\"", "cmd|more", "a b", "*.exe", "cmd.", ".cmd", "çmd", &"x".repeat(65), "http://x", "a&b", "%comspec%"] {
            assert!(!is_bare_name(no), "{no:?}");
        }
    }

    #[test]
    fn the_program_of_a_classic_app_is_read_from_its_id() {
        assert_eq!(app_file_stem(r"{D65231B0-B2F1-4857-A4CE-A8E7C6EA7D27}\cmd.exe").as_deref(), Some("cmd"));
        assert_eq!(app_file_stem(r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\PowerShell.exe").as_deref(), Some("powershell"));
        assert_eq!(app_file_stem(r"C:\Program Files\Mozilla Firefox\firefox.exe").as_deref(), Some("firefox"));
        // a packaged app's id, a document, a bare word
        assert_eq!(app_file_stem("Microsoft.WindowsCalculator_8wekyb3d8bbwe!App"), None);
        assert_eq!(app_file_stem(r"{GUID}\readme.txt"), None);
        assert_eq!(app_file_stem("cmd.exe"), None);
        assert_eq!(app_file_stem(r"{GUID}\.exe"), None);
    }

    #[test]
    fn a_name_with_and_without_exe_is_one_program() {
        assert_eq!(stem_of("CMD"), "cmd");
        assert_eq!(stem_of("cmd.exe"), "cmd");
        assert_eq!(stem_of("services.msc"), "services.msc");
    }

    #[test]
    fn what_is_no_name_is_never_looked_for() {
        assert_eq!(resolve(r"C:\Windows\System32\cmd.exe"), None);
        assert_eq!(resolve("cmd /c echo"), None);
        assert_eq!(resolve(""), None);
    }

    /// Read-only, on the real system folders: the command prompt is a program every Windows has, and a made-up name is none.
    #[test]
    fn the_command_prompt_is_found_and_a_made_up_name_is_not() {
        let cmd = resolve("cmd").expect("cmd.exe is in System32");
        assert!(cmd.to_string_lossy().to_lowercase().ends_with(r"\cmd.exe"));
        assert_eq!(resolve("cmd"), Some(cmd), "the remembered answer is the same");
        assert_eq!(resolve("no-such-program-rustyfiles-test"), None);
        assert!(resolve("services.msc").is_some_and(|p| p.to_string_lossy().to_lowercase().ends_with(r"\services.msc")));
    }
}
