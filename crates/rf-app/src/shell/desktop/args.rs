//! The command line of `rustyfiles --desktop-preview`. Pure.
//!
//! ```text
//! rustyfiles --desktop-preview                       the real Desktop, READ-ONLY (selecting, opening and menus work; no file of the
//!                                                    Desktop changes. The only thing written is our own layout file, and only after
//!                                                    an icon was moved or the size / sort / alignment was changed)
//! rustyfiles --desktop-preview --writable            the real Desktop, operations enabled (NOT when a --folder was given and could
//!                                                    not be used: then the real Desktop is shown and stays read-only)
//! rustyfiles --desktop-preview --folder <path>       SCRATCH: that folder pretends to be the Desktop (only its contents, no Public
//!                                                    Desktop, no system icons unless Settings -> Masaüstü says "Göster" for them), its own
//!                                                    layout file, every operation acts on it
//! RUSTYFILES_DESKTOP_DIR=<path>                      the same as --folder (the argument wins)
//! rustyfiles --desktop-preview ... --monitor <n>     show the n-th monitor (1 = primary, the default)
//! ```

use std::path::PathBuf;

pub const ARG: &str = "--desktop-preview";
pub const ENV_DIR: &str = "RUSTYFILES_DESKTOP_DIR";

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PreviewArgs {
    /// The scratch folder, if one was asked for.
    pub folder: Option<PathBuf>,
    /// `--writable` was given (only meaningful for the real Desktop: a scratch folder is always writable).
    pub writable: bool,
    /// 1-based, as typed.
    pub monitor: Option<usize>,
    /// Words that were not understood, for the log.
    pub ignored: Vec<String>,
}

impl PreviewArgs {
    pub fn scratch(&self) -> bool {
        self.folder.is_some()
    }
    /// The Desktop folders are not to be changed, going by the arguments alone: the real Desktop without `--writable`. (The loader
    /// also keeps a scratch folder that could not be used read-only: see `source::read_only_for`.)
    pub fn read_only(&self) -> bool {
        !self.scratch() && !self.writable
    }
}

/// `args`: the command line after the program name; `env_dir`: the value of `RUSTYFILES_DESKTOP_DIR`.
pub fn parse(args: &[String], env_dir: Option<&str>) -> PreviewArgs {
    let mut out = PreviewArgs::default();
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        let low = a.to_ascii_lowercase();
        if low == ARG {
            continue;
        }
        if let Some(v) = low.strip_prefix("--folder=").map(|_| a["--folder=".len()..].to_string()) {
            out.folder = Some(clean(&v)).filter(|p| !p.as_os_str().is_empty());
        } else if low == "--folder" {
            match it.next_if(|n| !n.starts_with("--")) {
                Some(v) => out.folder = Some(clean(v)).filter(|p| !p.as_os_str().is_empty()),
                None => out.ignored.push(a.clone()),
            }
        } else if low == "--writable" {
            out.writable = true;
        } else if let Some(v) = low.strip_prefix("--monitor=").map(|_| a["--monitor=".len()..].to_string()) {
            out.monitor = v.trim().parse().ok().filter(|n| *n >= 1);
        } else if low == "--monitor" {
            out.monitor = it.next_if(|n| !n.starts_with("--")).and_then(|v| v.trim().parse().ok()).filter(|n| *n >= 1);
        } else {
            out.ignored.push(a.clone());
        }
    }
    if out.folder.is_none() {
        out.folder = env_dir.map(clean).filter(|p| !p.as_os_str().is_empty());
    }
    out
}

fn clean(s: &str) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str], env: Option<&str>) -> PreviewArgs {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>(), env)
    }

    #[test]
    fn by_default_it_is_the_real_desktop_and_read_only() {
        let a = p(&["--desktop-preview"], None);
        assert_eq!(a, PreviewArgs::default());
        assert!(!a.scratch() && a.read_only());
    }

    #[test]
    fn writable_opens_the_real_desktop_for_changes() {
        let a = p(&["--desktop-preview", "--writable"], None);
        assert!(!a.scratch() && !a.read_only());
        let a = p(&["--desktop-preview", "--WRITABLE"], None);
        assert!(a.writable);
    }

    #[test]
    fn a_folder_makes_it_a_scratch_desktop_that_is_writable() {
        for args in [
            vec!["--desktop-preview", "--folder", r"C:\Temp\fake desktop"],
            vec!["--desktop-preview", r"--folder=C:\Temp\fake desktop"],
            vec!["--desktop-preview", "--folder", r#""C:\Temp\fake desktop""#],
        ] {
            let a = p(&args, None);
            assert_eq!(a.folder, Some(PathBuf::from(r"C:\Temp\fake desktop")), "{args:?}");
            assert!(a.scratch() && !a.read_only());
        }
    }

    #[test]
    fn the_environment_variable_does_the_same_and_the_argument_wins() {
        let a = p(&["--desktop-preview"], Some(r"C:\Temp\env desktop"));
        assert_eq!(a.folder, Some(PathBuf::from(r"C:\Temp\env desktop")));
        assert!(!a.read_only());
        let a = p(&["--desktop-preview", "--folder", r"C:\arg"], Some(r"C:\env"));
        assert_eq!(a.folder, Some(PathBuf::from(r"C:\arg")));
        // an empty variable means no scratch folder
        assert_eq!(p(&["--desktop-preview"], Some("  ")).folder, None);
        assert!(p(&["--desktop-preview"], Some("")).read_only());
    }

    #[test]
    fn a_missing_value_is_ignored_not_swallowing_the_next_option() {
        let a = p(&["--desktop-preview", "--folder", "--writable"], None);
        assert_eq!(a.folder, None);
        assert!(a.writable, "--writable was not taken as the folder");
        assert_eq!(a.ignored, ["--folder"]);
    }

    #[test]
    fn monitors_count_from_one() {
        assert_eq!(p(&["--desktop-preview", "--monitor", "2"], None).monitor, Some(2));
        assert_eq!(p(&["--desktop-preview", "--monitor=3"], None).monitor, Some(3));
        assert_eq!(p(&["--desktop-preview", "--monitor", "0"], None).monitor, None);
        assert_eq!(p(&["--desktop-preview", "--monitor", "x"], None).monitor, None);
    }

    #[test]
    fn unknown_words_are_kept_for_the_log() {
        let a = p(&["--desktop-preview", "--frobnicate", "x"], None);
        assert_eq!(a.ignored, ["--frobnicate", "x"]);
    }
}
