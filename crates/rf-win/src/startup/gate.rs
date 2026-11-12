//! The gate in front of the parts that act. Starting the user's start-up programs and deleting RunOnce values is the job of the shell
//! (when rustyfiles replaces Explorer, nobody else does it). Beside Explorer, Explorer has already done both, and doing it again
//! would start every program twice and consume RunOnce jobs. So the real launcher and the real RunOnce store cannot be built
//! without a [`ShellModeToken`], and the only way to get one is [`ShellModeToken::detect`], which says no while a real Explorer
//! runs in this session.
//!
//! This is a guard against a wrong call from a later part of the program, not against anything hostile: the token proves that
//! somebody asked the question, and the answer was yes. The pure check is separate, so that it is tested on plain lists.

use super::collect::ProcessSnapshot;
use super::command::path_key;

/// Why no token was given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A process runs from `<Windows>\explorer.exe` in this session: Explorer is the shell, and it has started the start-up programs.
    ExplorerRunning,
    /// The process list came back empty (it always holds at least this process): the question cannot be answered, so the answer is no.
    ProcessListUnreadable,
}

/// Proof that the process list was looked at and no Explorer runs. Cannot be made any other way than [`ShellModeToken::detect`].
#[derive(Debug)]
pub struct ShellModeToken(());

impl ShellModeToken {
    /// Reads this session's processes (read-only).
    pub fn detect() -> Result<ShellModeToken, Refusal> {
        use super::collect::RunningImages;
        let windows = super::system::windows_dir();
        Self::check(&super::system::SystemProcesses.snapshot(), &windows)
    }

    /// The decision, on a list.
    pub(crate) fn check(snap: &ProcessSnapshot, windows_dir: &str) -> Result<ShellModeToken, Refusal> {
        if snap.images.is_empty() && snap.unreadable_names.is_empty() {
            return Err(Refusal::ProcessListUnreadable);
        }
        let explorer = path_key(&format!("{}\\explorer.exe", windows_dir.trim_end_matches(['\\', '/'])));
        // An Explorer whose path could not be read is still an Explorer: refuse on the name alone.
        if snap.images.iter().any(|p| path_key(p) == explorer) || snap.unreadable_names.iter().any(|n| n.eq_ignore_ascii_case("explorer.exe")) {
            return Err(Refusal::ExplorerRunning);
        }
        Ok(ShellModeToken(()))
    }

    /// For the tests of the acting types, which are run over fakes and never touch the machine.
    #[cfg(test)]
    pub(crate) fn for_tests() -> ShellModeToken {
        ShellModeToken(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(images: &[&str], unreadable: &[&str]) -> ProcessSnapshot {
        ProcessSnapshot { images: images.iter().map(|s| s.to_string()).collect(), unreadable_names: unreadable.iter().map(|s| s.to_string()).collect() }
    }

    #[test]
    fn no_explorer_in_the_session_gives_a_token() {
        let s = snap(&[r"C:\Windows\System32\sihost.exe", r"C:\x\rustyfiles.exe"], &[]);
        assert!(ShellModeToken::check(&s, r"C:\Windows").is_ok());
    }

    #[test]
    fn the_windows_explorer_in_any_spelling_is_refused() {
        for p in [r"C:\Windows\explorer.exe", r"c:\WINDOWS\Explorer.EXE", r"C:/Windows//explorer.exe", r"\\?\C:\Windows\explorer.exe"] {
            let s = snap(&[p, r"C:\x\rustyfiles.exe"], &[]);
            assert_eq!(ShellModeToken::check(&s, r"C:\Windows\").unwrap_err(), Refusal::ExplorerRunning, "{p}");
        }
    }

    #[test]
    fn an_explorer_whose_path_cannot_be_read_is_refused_by_its_name() {
        let s = snap(&[r"C:\x\rustyfiles.exe"], &["Explorer.EXE"]);
        assert_eq!(ShellModeToken::check(&s, r"C:\Windows").unwrap_err(), Refusal::ExplorerRunning);
    }

    #[test]
    fn another_program_called_explorer_in_another_folder_is_not_the_windows_one() {
        let s = snap(&[r"D:\tools\explorer.exe"], &[]);
        assert!(ShellModeToken::check(&s, r"C:\Windows").is_ok());
    }

    #[test]
    fn an_empty_process_list_is_no_answer_and_so_a_refusal() {
        assert_eq!(ShellModeToken::check(&snap(&[], &[]), r"C:\Windows").unwrap_err(), Refusal::ProcessListUnreadable);
    }

    #[test]
    fn on_this_machine_the_real_question_can_be_asked_without_side_effects() {
        // The answer depends on the machine (a development PC runs Explorer: Err); the call must simply return.
        let _ = ShellModeToken::detect();
    }
}
