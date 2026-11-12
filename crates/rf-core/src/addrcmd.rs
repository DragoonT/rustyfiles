//! Commands typed into the address field (GitHub issue 14): the pure rules.
//!
//! Explorer takes the text of its address bar as a place first; only when it names no place that exists is it tried as a program
//! (`cmd`, `powershell`, `wt`, `"C:\Tools\a b\x.exe" /s`), started in the folder that is open. Nothing here touches the disk, the
//! registry or a process: the app passes in what exists, and the Windows side (`rf_win::runcmd`) looks for the program and starts
//! it. That keeps the decision testable and keeps the text -> place path (`parse_location` and friends) unable to start anything.

use std::path::{Path, PathBuf};

/// The extensions a typed program name is completed with, in the order Windows' `PATHEXT` lists them by default. Fixed on purpose
/// (not read from the environment): scripts such as `.vbs` or `.js` are not started from a typo in the address field.
pub const EXEC_EXTS: [&str; 4] = ["com", "exe", "bat", "cmd"];

/// A program and the arguments that follow it, both as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandLine {
    pub program: String,
    pub args: String,
}

/// What the text means.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// A place that exists: the tab goes there.
    Place(PathBuf),
    /// No place of that name exists and the text reads like a command. Several readings are possible when the program is a full
    /// path with spaces that was not quoted; the first one that names a program wins.
    Command(Vec<CommandLine>),
    /// Neither (empty text, a web address): the field does what it always did with the text.
    Neither,
}

/// Decides place / command / neither.
///
/// `places` are the readings of the whole text as a place (the typed path, the same name inside the open folder, ...), in the
/// order they are tried; `exists` says whether a path exists (a folder or a file: a file or folder with the typed name always wins
/// over a program of that name).
pub fn decide(text: &str, places: &[PathBuf], exists: &dyn Fn(&Path) -> bool) -> Intent {
    let t = text.trim();
    if t.is_empty() {
        return Intent::Neither;
    }
    if let Some(p) = places.iter().find(|p| exists(p)) {
        return Intent::Place(p.clone());
    }
    if looks_like_url(t) {
        return Intent::Neither;
    }
    let readings = split_command(t);
    if readings.is_empty() { Intent::Neither } else { Intent::Command(readings) }
}

/// `https://example.com`, `mailto:x`-style text is never a program. (The field has no web behaviour of its own: such text goes the
/// way every other missing path goes.)
pub fn looks_like_url(t: &str) -> bool {
    let first = t.split_whitespace().next().unwrap_or("");
    let Some(i) = first.find("://") else { return false };
    let scheme = &first[..i];
    scheme.len() >= 2 && scheme.starts_with(|c: char| c.is_ascii_alphabetic()) && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Text that can only be a place, so the field may go there at once without asking the disk whether anything is there: a
/// `shell:` place, "home", an `@alias`, a network path, a bare drive, a single path that ends with a separator (a folder is meant)
/// or a web address. Everything else (a name, a path, a path with spaces and more words) needs a look at the disk first.
///
/// A trailing separator only counts when the text is one token (no spaces, or all of it inside one pair of quotes): in
/// `cmd /k cd C:\` or `wt -d C:\` the separator ends an argument, and the text is a command.
pub fn is_plain_place(text: &str) -> bool {
    let t = text.trim().trim_matches('"');
    if t.is_empty() {
        return true;
    }
    let lower = t.to_lowercase();
    lower == "home"
        || lower == "ana sayfa"
        || lower.starts_with("shell:")
        || t.starts_with('@')
        || t.starts_with("\\\\")
        || t.starts_with("//")
        || (t.ends_with(['\\', '/']) && is_single_token(text.trim()))
        || (t.len() == 2 && t.ends_with(':'))
        || looks_like_url(t)
}

/// No whitespace in the text, or all of it inside one pair of quotes.
fn is_single_token(raw: &str) -> bool {
    if !raw.contains(char::is_whitespace) {
        return true;
    }
    raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') && !raw[1..raw.len() - 1].contains('"')
}

/// The path with `.` and `..` folded away by reading it, without asking the disk (a `..` after a folder name removes that name; a
/// `..` with nothing to remove stays when the path is relative and is dropped at a root). Verbatim (`\\?\`) paths are returned as
/// they are: Windows takes those literally.
pub fn normalize_lexical(p: &Path) -> PathBuf {
    use std::path::Component;
    if p.components().any(|c| matches!(c, Component::Prefix(pc) if pc.kind().is_verbatim())) {
        return p.to_path_buf();
    }
    let mut out: Vec<Component> = Vec::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => match out.last() {
                Some(Component::Normal(_)) => {
                    out.pop();
                }
                Some(Component::RootDir) | Some(Component::Prefix(_)) => {}
                _ => out.push(c),
            },
            _ => out.push(c),
        }
    }
    if out.is_empty() { PathBuf::from(".") } else { out.iter().collect() }
}

/// Splits typed text into the program and its arguments.
///
/// * A quoted first part is the program (`"C:\Program Files\x\a.exe" /s` -> `C:\Program Files\x\a.exe`, `/s`).
/// * Otherwise the first word is the program (`cmd /k dir` -> `cmd`, `/k dir`).
/// * A full path (`C:\...`) that was not quoted may contain spaces, as CreateProcess allows: the readings that stop at each later
///   space follow the first word, and the whole text as a program with no arguments comes last.
///
/// The arguments are left exactly as typed (nothing is expanded or re-quoted).
pub fn split_command(text: &str) -> Vec<CommandLine> {
    let t = text.trim();
    if t.is_empty() {
        return Vec::new();
    }
    if let Some(rest) = t.strip_prefix('"') {
        let (program, args) = match rest.find('"') {
            Some(i) => (&rest[..i], rest[i + 1..].trim_start()),
            None => (rest, ""),
        };
        let program = program.trim();
        return if program.is_empty() { Vec::new() } else { vec![CommandLine { program: program.to_string(), args: args.to_string() }] };
    }
    let mut out = Vec::new();
    let mut push = |end: usize| {
        let (program, args) = (&t[..end], t[end..].trim_start());
        out.push(CommandLine { program: program.to_string(), args: args.to_string() });
    };
    let first_end = t.find(char::is_whitespace).unwrap_or(t.len());
    push(first_end);
    if is_drive_path(&t[..first_end]) {
        let mut at = first_end;
        while let Some(off) = t[at..].find(char::is_whitespace) {
            let sp = at + off;
            // a run of spaces counts as one boundary
            let next = t[sp..].find(|c: char| !c.is_whitespace()).map_or(t.len(), |n| sp + n);
            if next >= t.len() {
                break;
            }
            let end = t[next..].find(char::is_whitespace).map_or(t.len(), |n| next + n);
            if end > first_end {
                push(end);
            }
            at = next;
        }
    }
    out.dedup();
    out
}

/// `C:\...` or `C:/...`
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// True when the program names a place on the disk (it has a separator or a drive), so it is not looked for on the search path.
pub fn has_dir_part(program: &str) -> bool {
    program.contains(['\\', '/', ':'])
}

/// True for `C:\...` / `\\server\...`, i.e. a program that needs no folder to be found.
pub fn is_absolute(program: &str) -> bool {
    is_drive_path(program) || program.starts_with("\\\\")
}

/// The file names tried for a program name: as typed when it already ends in an executable extension, else with each extension
/// added (`git` -> `git.com`, `git.exe`, ...; `python3.11` -> `python3.11.exe`, ...).
pub fn candidate_names(program: &str) -> Vec<String> {
    let name = program.rsplit(['\\', '/']).next().unwrap_or(program);
    let ext = name.rfind('.').map(|i| name[i + 1..].to_ascii_lowercase());
    if ext.as_deref().is_some_and(|e| EXEC_EXTS.contains(&e)) {
        return vec![program.to_string()];
    }
    EXEC_EXTS.iter().map(|e| format!("{program}.{e}")).collect()
}

/// The key under `App Paths` for a program name: `wt` -> `wt.exe` (the registry lists programs by their `.exe` name).
pub fn app_paths_name(program: &str) -> String {
    let lower = program.to_ascii_lowercase();
    if lower.ends_with(".exe") { program.to_string() } else { format!("{program}.exe") }
}

/// `%NAME%` replaced by `lookup(NAME)`; an unknown name stays as typed.
pub fn expand_vars(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('%') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        match after.find('%') {
            Some(end) => {
                let name = &after[..end];
                match lookup(name).filter(|_| !name.is_empty()) {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cl(p: &str, a: &str) -> CommandLine {
        CommandLine { program: p.into(), args: a.into() }
    }

    #[test]
    fn splits_the_first_word_from_the_arguments() {
        assert_eq!(split_command("cmd"), vec![cl("cmd", "")]);
        assert_eq!(split_command("  cmd /k dir  "), vec![cl("cmd", "/k dir")]);
        assert_eq!(split_command("powershell   -NoExit -Command \"ls\""), vec![cl("powershell", "-NoExit -Command \"ls\"")]);
        assert_eq!(split_command(""), vec![]);
        assert_eq!(split_command("   "), vec![]);
    }

    #[test]
    fn a_quoted_first_part_is_the_program() {
        assert_eq!(split_command(r#""C:\Program Files\Git\git.exe" status -s"#), vec![cl(r"C:\Program Files\Git\git.exe", "status -s")]);
        assert_eq!(split_command(r#""C:\a b\x.exe""#), vec![cl(r"C:\a b\x.exe", "")]);
        // an unterminated quote takes the rest as the program
        assert_eq!(split_command(r#""C:\a b\x.exe /s"#), vec![cl(r"C:\a b\x.exe /s", "")]);
        assert_eq!(split_command(r#""" arg"#), vec![]);
        // quotes inside the arguments are left alone
        assert_eq!(split_command(r#"cmd /c "echo hi""#), vec![cl("cmd", r#"/c "echo hi""#)]);
    }

    #[test]
    fn an_unquoted_full_path_with_spaces_gives_longer_readings_after_the_first_word() {
        let r = split_command(r"C:\Program Files\Git\git.exe status -s");
        assert_eq!(
            r,
            vec![
                cl(r"C:\Program", r"Files\Git\git.exe status -s"),
                cl(r"C:\Program Files\Git\git.exe", "status -s"),
                cl(r"C:\Program Files\Git\git.exe status", "-s"),
                cl(r"C:\Program Files\Git\git.exe status -s", ""),
            ]
        );
        // a name that is not a full path has one reading only
        assert_eq!(split_command("my program here"), vec![cl("my", "program here")]);
        assert_eq!(split_command(r"tools\x.exe a b"), vec![cl(r"tools\x.exe", "a b")]);
        // several spaces in a row, trailing space
        assert_eq!(split_command("C:\\a  b\\x.exe  /s").first(), Some(&cl("C:\\a", "b\\x.exe  /s")));
        assert!(split_command("C:\\a  b\\x.exe  /s").contains(&cl("C:\\a  b\\x.exe", "/s")));
    }

    #[test]
    fn candidate_file_names() {
        assert_eq!(candidate_names("cmd"), ["cmd.com", "cmd.exe", "cmd.bat", "cmd.cmd"]);
        assert_eq!(candidate_names("cmd.exe"), ["cmd.exe"]);
        assert_eq!(candidate_names("CMD.EXE"), ["CMD.EXE"]);
        assert_eq!(candidate_names("run.Bat"), ["run.Bat"]);
        assert_eq!(candidate_names(r"C:\a b\tool"), [r"C:\a b\tool.com", r"C:\a b\tool.exe", r"C:\a b\tool.bat", r"C:\a b\tool.cmd"]);
        // a dot in the name is not an executable extension
        assert_eq!(candidate_names("python3.11")[1], "python3.11.exe");
        // a dot in a folder name is not the file's extension
        assert_eq!(candidate_names(r"C:\v1.exe\tool")[1], r"C:\v1.exe\tool.exe");
        // a document is not a program
        assert_eq!(candidate_names("notes.txt")[1], "notes.txt.exe");
        assert_eq!(app_paths_name("wt"), "wt.exe");
        assert_eq!(app_paths_name("Chrome.EXE"), "Chrome.EXE");
    }

    #[test]
    fn place_command_or_neither() {
        let folder = PathBuf::from(r"D:\Work");
        let exists = |p: &Path| p == Path::new(r"D:\Work") || p == Path::new(r"D:\Work\cmd") || p == Path::new(r"C:\Users\x");
        // an existing place wins
        assert_eq!(decide(r"C:\Users\x", &[PathBuf::from(r"C:\Users\x")], &exists), Intent::Place(PathBuf::from(r"C:\Users\x")));
        // the same name in the open folder wins over the program of that name
        assert_eq!(decide("cmd", &[PathBuf::from("cmd"), folder.join("cmd")], &exists), Intent::Place(folder.join("cmd")));
        // no such place: a command
        assert_eq!(decide("cmd /k dir", &[PathBuf::from("cmd /k dir"), folder.join("cmd /k dir")], &exists), Intent::Command(vec![cl("cmd", "/k dir")]));
        assert_eq!(decide("wt", &[PathBuf::from("wt")], &|_| false), Intent::Command(vec![cl("wt", "")]));
        // a place is tried in order
        assert_eq!(decide("x", &[PathBuf::from("first"), PathBuf::from("second")], &|p| p == Path::new("second")), Intent::Place(PathBuf::from("second")));
        // empty and web addresses are neither
        assert_eq!(decide("", &[], &exists), Intent::Neither);
        assert_eq!(decide("   ", &[], &exists), Intent::Neither);
        assert_eq!(decide("https://example.com/a b", &[], &exists), Intent::Neither);
        assert_eq!(decide("HTTP://example.com", &[], &exists), Intent::Neither);
        // a quoted empty program is neither
        assert_eq!(decide("\"\"", &[], &exists), Intent::Neither);
    }

    #[test]
    fn urls_are_told_from_paths() {
        assert!(looks_like_url("https://rustyfiles.com"));
        assert!(looks_like_url("ftp://host/dir"));
        assert!(looks_like_url("git+ssh://host/x y"));
        assert!(!looks_like_url(r"C:\a\b"));
        assert!(!looks_like_url("://x"));
        assert!(!looks_like_url("1http://x"));
        assert!(!looks_like_url("cmd /c echo ://"));
    }

    #[test]
    fn what_can_only_be_a_place() {
        for t in ["home", "Ana Sayfa", "shell:Downloads", "@proje", "@proje\\src", r"\\server\share", "//server/share", r"C:\Users\", "D:", "c:", "https://x.y", r#""C:\Program Files\""#, ""] {
            assert!(is_plain_place(t), "{t}");
        }
        for t in ["cmd", "cmd /k dir", r"C:\Windows", r"C:\Windows\notepad", "docs", "%USERPROFILE%", "..", r"..\x", "wt -d ."] {
            assert!(!is_plain_place(t), "{t}");
        }
    }

    #[test]
    fn a_command_whose_text_ends_with_a_separator_is_still_a_command() {
        for t in [r"explorer C:\Users\", r"cmd /k cd C:\", r"wt -d C:\", r"code .\", r"cmd /c dir C:/", r#"  wt -d "C:\a b\"  "#] {
            assert!(!is_plain_place(t), "{t}");
        }
        // a single folder path, quoted or not, is still a place
        assert!(is_plain_place(r"  C:\Users\  "));
        assert!(is_plain_place(r#""C:\Program Files\""#));
        assert!(!is_plain_place(r#""C:\Program Files\" /s"#));
        // and the text really is a command when no place exists
        assert_eq!(decide(r"cmd /k cd C:\", &[PathBuf::from(r"cmd /k cd C:\")], &|_| false), Intent::Command(vec![cl("cmd", r"/k cd C:\")]));
    }

    #[test]
    fn lexical_normalising() {
        let n = |s: &str| normalize_lexical(Path::new(s));
        assert_eq!(n(r"C:\a\b\.."), PathBuf::from(r"C:\a"));
        assert_eq!(n(r"C:\a\.\b\..\c"), PathBuf::from(r"C:\a\c"));
        assert_eq!(n(r"C:\a\..\..\.."), PathBuf::from(r"C:\"));
        assert_eq!(n(r"..\x"), PathBuf::from(r"..\x"));
        assert_eq!(n(r"a\..\..\x"), PathBuf::from(r"..\x"));
        assert_eq!(n(r"a\.."), PathBuf::from("."));
        assert_eq!(n(r"\\server\share\a\..\b"), PathBuf::from(r"\\server\share\b"));
        // verbatim paths are literal
        assert_eq!(n(r"\\?\C:\a\..\b"), PathBuf::from(r"\\?\C:\a\..\b"));
    }

    #[test]
    fn variables_are_expanded_and_unknown_ones_stay() {
        let lookup = |n: &str| (n == "WINDIR").then(|| r"C:\Windows".to_string());
        assert_eq!(expand_vars(r"%WINDIR%\system32\cmd.exe", &lookup), r"C:\Windows\system32\cmd.exe");
        assert_eq!(expand_vars(r"%NOPE%\x", &lookup), r"%NOPE%\x");
        assert_eq!(expand_vars("100% sure", &lookup), "100% sure");
        assert_eq!(expand_vars("%%", &lookup), "%%");
        assert_eq!(expand_vars("a%WINDIR", &lookup), "a%WINDIR");
    }

    #[test]
    fn dir_parts() {
        assert!(has_dir_part(r"tools\x.exe"));
        assert!(has_dir_part("C:x"));
        assert!(!has_dir_part("cmd"));
        assert!(is_absolute(r"C:\a"));
        assert!(is_absolute(r"\\s\a"));
        assert!(!is_absolute(r"tools\a"));
    }
}
