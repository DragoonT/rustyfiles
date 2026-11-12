//! Cutting a Run value into the program and its arguments, the way Windows does it for a command line.
//!
//! A Run value is a command line, not a path. `CreateProcess` (documented behaviour, MSDN "CreateProcessW", `lpCommandLine`) reads the
//! first token as the program: when the line starts with a quote, the program is what is inside the quotes; when it does not, the
//! program is tried at every white-space boundary, shortest first, and the first candidate that is a file wins:
//! `C:\Program Files\Sub Dir\prog name.exe -x` is tried as `C:\Program.exe`, `C:\Program Files\Sub.exe`,
//! `C:\Program Files\Sub Dir\prog.exe`, `C:\Program Files\Sub Dir\prog name.exe`. A candidate whose last part has no extension gets
//! `.exe` added. A candidate without a folder is looked for in the system folders and `PATH`.
//! (The first two candidates are why an unquoted path with spaces is a known security hole: a file `C:\Program.exe` would win.
//! We reproduce the rule and do not repair it, so that what we say matches what Windows would start.)
//!
//! Not copied: the application's own folder and the current folder come first in the real search; neither means anything for a
//! line read at sign-in. For the same reason a program with a folder part that is not absolute (`sub\x.exe`, `\x.exe`, `C:x.exe`) is
//! not found: it would be looked for next to whatever the host's current folder happens to be, and the launcher passes none.
//!
//! The documented 260-character limit of a Run value (research note 1, section 5.1 item 3) is NOT enforced here: what Windows does
//! with a longer value is not documented, so a longer line is split like any other (up to the 32,767 of `CreateProcess`).
//!
//! What exists on disk, and what `%VARIABLES%` mean, are told to the splitter through [`Files`] and [`Environment`], so that it is
//! tested on plain strings.

/// The longest command line `CreateProcess` accepts (characters, with the terminating NUL it is 32,767).
pub const MAX_COMMAND_CHARS: usize = 32_767;
/// How many white-space boundaries are tried as the end of an unquoted program. Real paths have a handful of spaces; a hostile line of
/// thousands of spaces must not turn into thousands of disk probes.
const MAX_CANDIDATES: usize = 32;

/// What is on disk.
pub trait Files {
    /// A file (not a folder) exists at this path.
    fn is_file(&self, path: &str) -> bool;
    /// A file or a folder exists at this path.
    fn exists(&self, path: &str) -> bool;
    /// The long form of an existing path (8.3 names written out), so that two spellings of one file compare equal. The input when
    /// there is nothing to improve.
    fn long_path(&self, path: &str) -> String {
        path.to_string()
    }
}

/// The environment of the user.
pub trait Environment {
    /// `%VARIABLES%` replaced by their values (what `ExpandEnvironmentStrings` does; unknown variables stay as written).
    fn expand(&self, s: &str) -> String;
    /// The folders a program name without a folder is looked for in, in order.
    fn search_dirs(&self) -> Vec<String>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SplitHow {
    /// The line starts with a quote and the program is what is inside.
    Quoted,
    /// The quote is never closed: the rest of the line is taken as the program.
    UnterminatedQuote,
    /// Unquoted: the candidate with this index (0 = up to the first white space) is the file.
    Candidate(usize),
    /// No candidate is a file: the first token is reported as the program.
    NotFound,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Split {
    /// The program: the file that was found (with `.exe` added or the folder searched when that is what found it), else the text as
    /// written.
    pub program: String,
    pub args: String,
    /// `program` is a file on disk.
    pub found: bool,
    pub how: SplitHow,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SplitError {
    /// Nothing but white space, or an empty pair of quotes.
    Empty,
    /// Longer than `CreateProcess` takes.
    TooLong,
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn has_dir_part(s: &str) -> bool {
    s.contains(['\\', '/', ':'])
}

fn last_part(s: &str) -> &str {
    s.rsplit(['\\', '/']).next().unwrap_or(s)
}

/// The file name `CreateProcess` looks for when it is given `candidate`.
fn with_default_extension(candidate: &str) -> String {
    let last = last_part(candidate);
    if last.contains('.') { candidate.to_string() } else { format!("{candidate}.exe") }
}

fn join(dir: &str, name: &str) -> String {
    format!("{}\\{}", dir.trim_end_matches(['\\', '/']), name)
}

/// `X:\...` or `X:/...`, or a UNC / `\\?\` path. Anything else with a folder part is relative to a current folder or drive.
fn is_absolute(p: &str) -> bool {
    let b = p.as_bytes();
    (b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && matches!(b[2], b'\\' | b'/')) || p.starts_with(r"\\")
}

fn resolve(candidate: &str, files: &dyn Files, dirs: &mut Option<Vec<String>>, env: &dyn Environment) -> Option<String> {
    if candidate.is_empty() {
        return None;
    }
    let name = with_default_extension(candidate);
    if has_dir_part(&name) {
        return (is_absolute(&name) && files.is_file(&name)).then_some(name);
    }
    let dirs = dirs.get_or_insert_with(|| env.search_dirs());
    // a relative entry in PATH ("." or "bin") would again mean the current folder
    dirs.iter().map(|d| join(d, &name)).find(|p| is_absolute(p) && files.is_file(p))
}

/// Splits a command line (already expanded) into program and arguments. See the module comment for the rules.
pub fn split_command(command: &str, files: &dyn Files, env: &dyn Environment) -> Result<Split, SplitError> {
    // A string in the registry ends at its first NUL for everything that reads it.
    let command = command.split('\0').next().unwrap_or("");
    if command.len() > 4 * MAX_COMMAND_CHARS || command.encode_utf16().count() > MAX_COMMAND_CHARS {
        return Err(SplitError::TooLong);
    }
    let s = command.trim_matches(is_ws);
    if s.is_empty() {
        return Err(SplitError::Empty);
    }
    let mut dirs: Option<Vec<String>> = None;

    if let Some(rest) = s.strip_prefix('"') {
        let (inner, args, how) = match rest.find('"') {
            Some(i) => (&rest[..i], rest[i + 1..].trim_matches(is_ws), SplitHow::Quoted),
            None => (rest, "", SplitHow::UnterminatedQuote),
        };
        // Not trimmed: CreateProcess takes what is inside the quotes as the file name, spaces included (`" C:\a.exe"` names a file
        // with a leading space, not `C:\a.exe`). A quoted blank is "no program at all".
        if inner.trim_matches(is_ws).is_empty() {
            return Err(SplitError::Empty);
        }
        return Ok(match resolve(inner, files, &mut dirs, env) {
            Some(p) => Split { program: p, args: args.to_string(), found: true, how },
            None => Split { program: inner.to_string(), args: args.to_string(), found: false, how },
        });
    }

    // Unquoted: the ends the program may have are the first white space of every run of white space, then the end of the line.
    // At most MAX_CANDIDATES candidates, the whole line always the last of them: a path with many spaces still gets its full
    // spelling tried, and only the boundaries in between are dropped.
    let mut ends: Vec<usize> = Vec::new();
    let mut prev_ws = false;
    for (i, c) in s.char_indices() {
        let ws = is_ws(c);
        if ws && !prev_ws {
            ends.push(i);
            if ends.len() >= MAX_CANDIDATES - 1 {
                break;
            }
        }
        prev_ws = ws;
    }
    ends.push(s.len());
    ends.dedup();
    for (n, &end) in ends.iter().enumerate() {
        if let Some(p) = resolve(&s[..end], files, &mut dirs, env) {
            return Ok(Split { program: p, args: s[end..].trim_start_matches(is_ws).to_string(), found: true, how: SplitHow::Candidate(n) });
        }
    }
    let first = ends[0];
    Ok(Split { program: s[..first].to_string(), args: s[first..].trim_start_matches(is_ws).to_string(), found: false, how: SplitHow::NotFound })
}

/// Replaces `%NAME%` by what `lookup` says, like `ExpandEnvironmentStrings`: a name that `lookup` does not know stays as written
/// (percent signs included), `%%` and a lone `%` stay as they are.
pub fn expand_with(s: &str, lookup: &dyn Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('%') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find('%') {
            Some(j) => {
                let name = &after[..j];
                match if name.is_empty() { None } else { lookup(name) } {
                    Some(v) => out.push_str(&v),
                    None => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[j + 1..];
            }
            None => {
                out.push('%');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// A key to compare two spellings of one path: lower case, one kind of separator, no `\\?\` prefix, no doubled or trailing
/// separators.
pub fn path_key(p: &str) -> String {
    let mut s = p.trim().replace('/', "\\");
    if let Some(r) = s.strip_prefix(r"\\?\") {
        s = r.to_string();
    }
    let unc = s.starts_with(r"\\");
    let mut out = String::with_capacity(s.len());
    let mut prev_sep = false;
    for c in s.chars() {
        let sep = c == '\\';
        if sep && prev_sep && !(unc && out.len() == 1) {
            continue;
        }
        out.push(c);
        prev_sep = sep;
    }
    while out.len() > 3 && out.ends_with('\\') {
        out.pop();
    }
    out.to_lowercase()
}

/// File names that stand for many different jobs: whether a process of that image runs says nothing about whether THIS line is
/// running (a `rundll32.exe` runs for a dozen reasons).
const GENERIC_HOSTS: &[&str] = &[
    "rundll32.exe",
    "cmd.exe",
    "powershell.exe",
    "pwsh.exe",
    "wscript.exe",
    "cscript.exe",
    "mshta.exe",
    "regsvr32.exe",
    "msiexec.exe",
    "explorer.exe",
    "conhost.exe",
    "schtasks.exe",
    "reg.exe",
    "wmic.exe",
    "java.exe",
    "javaw.exe",
    "node.exe",
    "python.exe",
    "pythonw.exe",
    "dllhost.exe",
    "svchost.exe",
];

/// Whether "a process with this image path is running" is a useful answer for `program`: it is an `.exe` (a `.bat`, a `.vbs`, a
/// document run through its association has no image of its own) and not a generic host.
pub fn running_check_is_meaningful(program: &str) -> bool {
    let name = last_part(program).to_lowercase();
    name.ends_with(".exe") && !GENERIC_HOSTS.contains(&name.as_str())
}

/// The file name of a program, lower case ("c:\\x\\Ctfmon.EXE" -> "ctfmon.exe").
pub fn file_name_lower(program: &str) -> String {
    last_part(program.trim_end_matches(['\\', '/'])).to_lowercase()
}

#[cfg(test)]
pub(crate) mod fakes {
    use super::*;
    use std::collections::HashSet;

    /// A disk of the files it is given (compared by `path_key`). Counts the probes.
    #[derive(Default)]
    pub struct FakeFiles {
        pub files: HashSet<String>,
        pub dirs: HashSet<String>,
        pub probes: std::cell::Cell<usize>,
    }

    impl FakeFiles {
        pub fn with(files: &[&str]) -> FakeFiles {
            FakeFiles { files: files.iter().map(|f| path_key(f)).collect(), ..Default::default() }
        }
    }

    impl Files for FakeFiles {
        fn is_file(&self, path: &str) -> bool {
            self.probes.set(self.probes.get() + 1);
            self.files.contains(&path_key(path))
        }
        fn exists(&self, path: &str) -> bool {
            self.is_file(path) || self.dirs.contains(&path_key(path))
        }
    }

    /// `%NAME%` replaced from a small table (case-insensitive names, like Windows); the search folders are given.
    pub struct FakeEnv {
        pub vars: Vec<(String, String)>,
        pub dirs: Vec<String>,
    }

    impl FakeEnv {
        pub fn standard() -> FakeEnv {
            FakeEnv {
                vars: vec![
                    ("ProgramFiles".into(), r"C:\Program Files".into()),
                    ("SystemRoot".into(), r"C:\WINDOWS".into()),
                    ("APPDATA".into(), r"C:\Users\u\AppData\Roaming".into()),
                ],
                dirs: vec![r"C:\WINDOWS\system32".into(), r"C:\WINDOWS".into(), r"C:\Tools".into()],
            }
        }
    }

    impl Environment for FakeEnv {
        fn expand(&self, s: &str) -> String {
            expand_with(s, &|name| self.vars.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone()))
        }
        fn search_dirs(&self) -> Vec<String> {
            self.dirs.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fakes::*;
    use super::*;

    fn split(cmd: &str, files: &[&str]) -> Result<Split, SplitError> {
        split_command(cmd, &FakeFiles::with(files), &FakeEnv::standard())
    }

    #[test]
    fn a_quoted_program_with_arguments() {
        let s = split(r#""C:\Program Files\App\app.exe" --minimized /x"#, &[r"C:\Program Files\App\app.exe"]).unwrap();
        assert_eq!(s.program, r"C:\Program Files\App\app.exe");
        assert_eq!(s.args, "--minimized /x");
        assert!(s.found);
        assert_eq!(s.how, SplitHow::Quoted);
    }

    #[test]
    fn a_quoted_program_followed_at_once_by_its_arguments() {
        let s = split(r#""C:\a\b.exe"/s"#, &[r"C:\a\b.exe"]).unwrap();
        assert_eq!((s.program.as_str(), s.args.as_str()), (r"C:\a\b.exe", "/s"));
    }

    #[test]
    fn an_unquoted_path_with_spaces_is_found_by_trying_the_candidates() {
        let s = split(r"C:\Program Files\App\app.exe -x -y", &[r"C:\Program Files\App\app.exe"]).unwrap();
        assert_eq!(s.program, r"C:\Program Files\App\app.exe");
        assert_eq!(s.args, "-x -y");
        assert_eq!(s.how, SplitHow::Candidate(1), "C:\\Program(.exe) was candidate 0, the whole path is candidate 1");
    }

    #[test]
    fn several_spaces_in_a_path_take_several_candidates() {
        let p = r"C:\Program Files\Sub Dir\prog name.exe";
        let s = split(&format!("{p} a b"), &[p]).unwrap();
        assert_eq!(s.program, p);
        assert_eq!(s.args, "a b");
        assert_eq!(s.how, SplitHow::Candidate(3));
    }

    #[test]
    fn createprocess_takes_the_shortest_candidate_that_is_a_file() {
        // the well-known hole: with a file C:\Program.exe on the disk, the unquoted line starts that, with the rest as arguments
        let s = split(r"C:\Program Files\App\app.exe", &[r"C:\Program.exe", r"C:\Program Files\App\app.exe"]).unwrap();
        assert_eq!(s.program, r"C:\Program.exe");
        assert_eq!(s.args, r"Files\App\app.exe");
        assert_eq!(s.how, SplitHow::Candidate(0));
    }

    #[test]
    fn exe_is_added_to_a_candidate_without_an_extension() {
        let s = split(r"C:\Tools\tool /s", &[r"C:\Tools\tool.exe"]).unwrap();
        assert_eq!((s.program.as_str(), s.args.as_str()), (r"C:\Tools\tool.exe", "/s"));
        // ... and to a quoted one
        let s = split(r#""C:\Tools\tool" /s"#, &[r"C:\Tools\tool.exe"]).unwrap();
        assert_eq!(s.program, r"C:\Tools\tool.exe");
        // a name that has an extension is taken as it is: no tool.txt.exe
        let s = split(r"C:\Tools\tool.txt", &[r"C:\Tools\tool.txt.exe"]).unwrap();
        assert!(!s.found);
    }

    #[test]
    fn a_name_without_a_folder_is_looked_for_in_the_search_folders_in_order() {
        let s = split(r"rundll32.exe shell32.dll,Control_RunDLL", &[r"C:\WINDOWS\system32\rundll32.exe", r"C:\Tools\rundll32.exe"]).unwrap();
        assert_eq!(s.program, r"C:\WINDOWS\system32\rundll32.exe");
        assert_eq!(s.args, "shell32.dll,Control_RunDLL");
        // without the extension, and not in the first folder
        let s = split("ctfmon", &[r"C:\Tools\ctfmon.exe"]).unwrap();
        assert_eq!(s.program, r"C:\Tools\ctfmon.exe");
        assert_eq!(s.args, "");
    }

    #[test]
    fn rundll32_lines() {
        let rd = r"C:\WINDOWS\system32\rundll32.exe";
        let s = split(&format!(r#"{rd} C:\WINDOWS\system32\mshtml.dll,PrintHTML "%1""#), &[rd]).unwrap();
        assert_eq!(s.program, rd);
        assert_eq!(s.args, r#"C:\WINDOWS\system32\mshtml.dll,PrintHTML "%1""#);
        // quoted rundll32 with a quoted dll that has spaces in its path
        let s = split(&format!(r#""{rd}" "C:\Program Files\X\x.dll",Start"#), &[rd]).unwrap();
        assert_eq!(s.program, rd);
        assert_eq!(s.args, r#""C:\Program Files\X\x.dll",Start"#);
    }

    #[test]
    fn a_program_that_is_not_there_is_reported_as_the_first_token() {
        let s = split(r"C:\Gone\app.exe -x -y", &[]).unwrap();
        assert!(!s.found);
        assert_eq!(s.how, SplitHow::NotFound);
        assert_eq!(s.program, r"C:\Gone\app.exe");
        assert_eq!(s.args, "-x -y");
        let s = split(r#""C:\Gone\app.exe" -x"#, &[]).unwrap();
        assert!(!s.found);
        assert_eq!((s.program.as_str(), s.args.as_str()), (r"C:\Gone\app.exe", "-x"));
    }

    #[test]
    fn white_space_inside_quotes_belongs_to_the_file_name() {
        // a file named " a.exe" is not C:\a.exe, and the other way round
        let s = split(r#"" C:\a.exe" x"#, &[r"C:\a.exe"]).unwrap();
        assert!(!s.found, "{s:?}");
        assert_eq!(split(r#""  ""#, &[]), Err(SplitError::Empty));
        // the unquoted line is trimmed at its ends, as before
        assert!(split(r"   C:\a.exe", &[r"C:\a.exe"]).unwrap().found);
    }

    #[test]
    fn a_program_with_a_relative_folder_part_is_not_found() {
        let on_disk = [r"sub\x.exe", r"\x.exe", "C:x.exe", "./x.exe", r"..\x.exe", r"C:\WINDOWS\system32\sub\x.exe"];
        for line in [r"sub\x.exe", r"\x.exe", "C:x.exe", "./x.exe", r"..\x.exe", r#""sub\x.exe" /s"#] {
            assert!(!split(line, &on_disk).unwrap().found, "{line}");
        }
        // a UNC path is absolute
        assert!(split(r"\\srv\share\x.exe", &[r"\\srv\share\x.exe"]).unwrap().found);
        // a relative entry of PATH is not searched
        let env = FakeEnv { vars: vec![], dirs: vec![".".into(), r"C:\Tools".into()] };
        let f = FakeFiles::with(&[r".\t.exe", r"C:\Tools\t.exe"]);
        assert_eq!(split_command("t", &f, &env).unwrap().program, r"C:\Tools\t.exe");
    }

    #[test]
    fn a_path_with_around_the_candidate_limit_of_white_space_runs_is_still_found_whole() {
        // the candidate count is bounded, but the full spelling of the line is always among the candidates
        for runs in [MAX_CANDIDATES - 2, MAX_CANDIDATES - 1, MAX_CANDIDATES, MAX_CANDIDATES + 1, MAX_CANDIDATES + 40] {
            let p = format!(r"C:\{}\app.exe", vec!["d"; runs + 1].join(" "));
            let s = split(&p, &[&p]).unwrap_or_else(|e| panic!("{runs}: {e:?}"));
            assert!(s.found, "{runs} runs of white space inside the path");
            assert_eq!((s.program.as_str(), s.args.as_str()), (p.as_str(), ""));
        }
    }

    #[test]
    fn a_folder_is_not_a_program() {
        let mut f = FakeFiles::default();
        f.dirs.insert(path_key(r"C:\Tools\thing.exe"));
        let s = split_command(r"C:\Tools\thing.exe", &f, &FakeEnv::standard()).unwrap();
        assert!(!s.found);
    }

    #[test]
    fn an_unclosed_quote_takes_the_rest_as_the_program() {
        let s = split(r#""C:\Program Files\App\app.exe --x"#, &[]).unwrap();
        assert_eq!(s.how, SplitHow::UnterminatedQuote);
        assert_eq!(s.program, r"C:\Program Files\App\app.exe --x");
        assert_eq!(s.args, "");
        assert!(!s.found);
    }

    #[test]
    fn variables_are_expanded_by_the_environment_trait_before_the_split() {
        let env = FakeEnv::standard();
        let files = FakeFiles::with(&[r"C:\Program Files\Foo\foo.exe"]);
        let expanded = env.expand(r#""%ProgramFiles%\Foo\foo.exe" -s %UNKNOWN%"#);
        assert_eq!(expanded, r#""C:\Program Files\Foo\foo.exe" -s %UNKNOWN%"#, "unknown variables stay as written");
        let s = split_command(&expanded, &files, &env).unwrap();
        assert_eq!(s.program, r"C:\Program Files\Foo\foo.exe");
        assert_eq!(s.args, "-s %UNKNOWN%");
        // names are not case-sensitive
        assert_eq!(env.expand("%systemroot%\\x"), r"C:\WINDOWS\x");
    }

    #[test]
    fn no_program_at_all() {
        for cmd in ["", "   ", "\t\r\n", "\"\"", "\"   \"", "\"", "\0", "\0C:\\a.exe"] {
            assert_eq!(split(cmd, &[]), Err(SplitError::Empty), "{cmd:?}");
        }
    }

    #[test]
    fn an_embedded_nul_ends_the_line() {
        let s = split("C:\\a.exe\0 --evil", &[r"C:\a.exe"]).unwrap();
        assert_eq!(s.program, r"C:\a.exe");
        assert_eq!(s.args, "", "what follows the NUL is not part of the command");
        let s = split("\"C:\\a b.exe\0\" x", &[]).unwrap();
        assert!(!s.args.contains("x"));
    }

    #[test]
    fn a_line_longer_than_createprocess_takes_is_refused_without_probing_the_disk() {
        let files = FakeFiles::default();
        let long = format!("C:\\{}.exe", "a".repeat(40_000));
        assert_eq!(split_command(&long, &files, &FakeEnv::standard()), Err(SplitError::TooLong));
        let wide_bytes_many = "𝄞".repeat(20_000); // 40,000 UTF-16 units
        assert_eq!(split_command(&wide_bytes_many, &files, &FakeEnv::standard()), Err(SplitError::TooLong));
        assert_eq!(files.probes.get(), 0);
        // 20,000 characters of two bytes each is within the limit
        let wide = "é".repeat(20_000);
        assert!(split_command(&wide, &FakeFiles::default(), &FakeEnv::standard()).is_ok());
    }

    #[test]
    fn thousands_of_spaces_cost_a_bounded_number_of_probes() {
        let files = FakeFiles::default();
        let hostile = "a ".repeat(15_000);
        let s = split_command(&hostile, &files, &FakeEnv::standard()).unwrap();
        assert!(!s.found);
        assert_eq!(s.program, "a");
        // at most MAX_CANDIDATES candidates, each looked for in at most the three search folders
        assert!(files.probes.get() <= MAX_CANDIDATES * 3, "{} probes", files.probes.get());
    }

    #[test]
    fn the_longest_legal_line_still_splits() {
        let files = FakeFiles::default();
        let line = format!("C:\\a.exe {}", "x".repeat(MAX_COMMAND_CHARS - 10));
        let s = split_command(&line, &files, &FakeEnv::standard()).unwrap();
        assert_eq!(s.program, r"C:\a.exe");
    }

    #[test]
    fn white_space_around_and_between_is_not_part_of_the_pieces() {
        let s = split("   \"C:\\a.exe\"   \t -x  ", &[r"C:\a.exe"]).unwrap();
        assert_eq!((s.program.as_str(), s.args.as_str()), (r"C:\a.exe", "-x"));
    }

    #[test]
    fn expansion_leaves_what_it_does_not_know_and_odd_percent_signs_alone() {
        let lookup = |n: &str| (n.eq_ignore_ascii_case("a")).then(|| "X".to_string());
        assert_eq!(expand_with("%a%%A%", &lookup), "XX");
        assert_eq!(expand_with("%b% %a%", &lookup), "%b% X");
        assert_eq!(expand_with("100%", &lookup), "100%");
        assert_eq!(expand_with("%%", &lookup), "%%");
        // percent signs pair up from the left, so the first two here make a name " of " that nothing knows
        assert_eq!(expand_with("50% of %a% and 5%", &lookup), "50% of %a% and 5%");
        assert_eq!(expand_with("", &lookup), "");
        // a value that itself holds a percent sign is not expanded again
        let again = |n: &str| (n == "a").then(|| "%a%".to_string());
        assert_eq!(expand_with("%a%", &again), "%a%");
        // a million lone percent signs cost a linear scan
        assert_eq!(expand_with(&"%".repeat(1_000_000), &lookup).len(), 1_000_000);
    }

    #[test]
    fn path_keys_compare_spellings() {
        assert_eq!(path_key(r"C:\Program Files\A\"), path_key(r"c:/program files//a"));
        assert_eq!(path_key(r"\\?\C:\X\y.EXE"), path_key(r"c:\x\y.exe"));
        assert_ne!(path_key(r"C:\x\y.exe"), path_key(r"C:\x\z.exe"));
        assert_eq!(path_key(r"\\server\share\f"), r"\\server\share\f");
        assert_eq!(path_key("C:\\"), "c:\\");
    }

    #[test]
    fn the_running_check_is_meaningful_for_ordinary_programs_only() {
        assert!(running_check_is_meaningful(r"C:\Program Files\Discord\Update.exe"));
        assert!(!running_check_is_meaningful(r"C:\WINDOWS\system32\RunDLL32.EXE"));
        assert!(!running_check_is_meaningful(r"C:\x\start.bat"));
        assert!(!running_check_is_meaningful(r"C:\x\a.lnk"));
        assert!(!running_check_is_meaningful(""));
        assert_eq!(file_name_lower(r"C:\X\CtfMon.EXE"), "ctfmon.exe");
    }
}
