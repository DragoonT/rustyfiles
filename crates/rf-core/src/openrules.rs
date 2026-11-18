//! The user's own programs for a kind of file: which program opens `.dll` on a double click, and which programs stand first under
//! "Open with". Pure rules; the launching is `rf_win::openrules`, the page is the app's.
//!
//! WHY (a user on Discord, 2026-11-14, who opens the same file in several reverse-engineering tools: "i want to be able to fully
//! customize the default opening and open with behavior for different files ... i can set the default to my vscodium path ... and i
//! can set first priority to notepad++"). Windows keeps one default per type and guards it; a program it does not know has to be
//! browsed for every time. Here a type has an ordered list of programs, each given by its path: the first opens the file on a double
//! click, all of them are the first rows of "Open with". Only rustyfiles follows the list: Windows' own defaults are not touched.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// One list: the extensions it is for and the programs, in the user's order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenRule {
    /// Lower-case extensions without the dot ("dll", "js").
    pub exts: Vec<String>,
    /// The programs' paths. The first is the default.
    pub programs: Vec<String>,
    /// A double click (and Enter) opens the file with the first program. Off: the programs are only offered under "Open with", and a
    /// double click does what Windows does (the choice for `.exe`, which should still run).
    pub double_click: bool,
}

impl Default for OpenRule {
    fn default() -> Self {
        OpenRule { exts: Vec::new(), programs: Vec::new(), double_click: true }
    }
}

/// An extension as the rules keep it: without dots at its front, lower case. `None` for what is no extension (nothing, a space, a
/// path, a pattern).
pub fn clean_ext(raw: &str) -> Option<String> {
    let e = raw.trim().trim_start_matches(['*', '.']).to_lowercase();
    let ok = !e.is_empty() && e.len() <= 32 && !e.chars().any(|c| c.is_whitespace() || matches!(c, '.' | '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ','));
    ok.then_some(e)
}

/// The extensions in what the user typed ("dll, .exe; *.sys"): separated by commas, semicolons or spaces, each once, in the order
/// typed. `Err` names the first piece that is no extension.
pub fn parse_exts(text: &str) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for piece in text.split([',', ';', ' ', '\t', '\n']).map(str::trim).filter(|p| !p.is_empty()) {
        let e = clean_ext(piece).ok_or_else(|| piece.to_string())?;
        if !out.contains(&e) {
            out.push(e);
        }
    }
    Ok(out)
}

/// A file's extension as the rules keep it; `None` for a name without one (and for a name that is only an extension, ".gitignore").
pub fn ext_of(path: &Path) -> Option<String> {
    path.extension().and_then(|e| clean_ext(&e.to_string_lossy()))
}

/// The list for an extension: the first that names it.
pub fn rule_for<'a>(rules: &'a [OpenRule], ext: &str) -> Option<&'a OpenRule> {
    let ext = ext.to_lowercase();
    rules.iter().find(|r| r.exts.iter().any(|e| *e == ext))
}

/// The programs offered under "Open with" for an extension, in the user's order.
pub fn programs_for<'a>(rules: &'a [OpenRule], ext: &str) -> &'a [String] {
    rule_for(rules, ext).map_or(&[], |r| r.programs.as_slice())
}

/// The program a double click opens a file of this extension with; `None`: Windows' own default.
pub fn default_program<'a>(rules: &'a [OpenRule], ext: &str) -> Option<&'a str> {
    rule_for(rules, ext).filter(|r| r.double_click).and_then(|r| r.programs.first()).map(String::as_str)
}

/// The ones of `exts` that another list already names (`except`: the index of the list being changed). An extension belongs to one
/// list: two lists for it would leave the second one never looked at.
pub fn taken(rules: &[OpenRule], except: Option<usize>, exts: &[String]) -> Vec<String> {
    exts.iter().filter(|e| rules.iter().enumerate().any(|(i, r)| Some(i) != except && r.exts.contains(e))).cloned().collect()
}

/// A program's path as the rules keep it: what was typed or pasted without the quotes "Copy as path" puts around it. `None` when
/// nothing is left.
pub fn clean_program(raw: &str) -> Option<String> {
    let p = raw.trim().trim_matches('"').trim();
    (!p.is_empty()).then(|| p.to_string())
}

/// The name a program goes by in the list and in the menu: its file's name without the extension ("dnSpy" for `...\dnSpy.exe`).
pub fn program_label(path: &str) -> String {
    let stem = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    if stem.is_empty() { path.to_string() } else { stem }
}

/// Whether a file of this extension does something by itself when it is opened (a program, a script, an installer): a list for it
/// starts with the double click left to Windows, so that making "Open with" entries for `.exe` does not stop programs from running.
pub fn runs_by_itself(ext: &str) -> bool {
    matches!(ext.to_lowercase().as_str(), "exe" | "com" | "bat" | "cmd" | "msi" | "scr" | "lnk" | "ps1" | "vbs" | "appref-ms")
}

/// A new list for these extensions.
pub fn new_rule(exts: Vec<String>) -> OpenRule {
    let double_click = !exts.iter().any(|e| runs_by_itself(e));
    OpenRule { exts, programs: Vec::new(), double_click }
}

/// The list with the item at `ix` moved `by` places (clamped to the ends); the same list when `ix` is not in it.
pub fn moved<T: Clone>(list: &[T], ix: usize, by: i32) -> Vec<T> {
    let mut out = list.to_vec();
    if ix >= out.len() {
        return out;
    }
    let to = (ix as i64 + i64::from(by)).clamp(0, out.len() as i64 - 1) as usize;
    let item = out.remove(ix);
    out.insert(to, item);
    out
}

/// The list's extensions as one line: ".dll, .sys".
pub fn exts_line(rule: &OpenRule) -> String {
    rule.exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(", ")
}

/// The command line's part that names the file for a program: the path in quotes (a path has no quote in it; one that ends with a
/// backslash would swallow the closing quote, so that backslash is doubled).
pub fn file_argument(file: &Path) -> String {
    let s = file.to_string_lossy();
    if s.ends_with('\\') { format!("\"{s}\\\"") } else { format!("\"{s}\"") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(exts: &[&str], programs: &[&str], double_click: bool) -> OpenRule {
        OpenRule { exts: exts.iter().map(|s| s.to_string()).collect(), programs: programs.iter().map(|s| s.to_string()).collect(), double_click }
    }

    #[test]
    fn an_extension_is_kept_without_its_dot_and_in_lower_case() {
        assert_eq!(clean_ext(".DLL"), Some("dll".into()));
        assert_eq!(clean_ext("  js "), Some("js".into()));
        assert_eq!(clean_ext("*.sys"), Some("sys".into()));
        assert_eq!(clean_ext("7z"), Some("7z".into()));
        assert_eq!(clean_ext("appref-ms"), Some("appref-ms".into()));
        for bad in ["", ".", "  ", "a b", "tar.gz", r"c:\x", "a/b", "a*", "x?", "\"dll\""] {
            assert_eq!(clean_ext(bad), None, "{bad:?}");
        }
        assert_eq!(clean_ext(&"x".repeat(33)), None, "no extension is that long");
    }

    #[test]
    fn what_the_user_types_is_read_as_a_list_of_extensions() {
        assert_eq!(parse_exts("dll, .exe; *.sys  js"), Ok(vec!["dll".into(), "exe".into(), "sys".into(), "js".into()]));
        assert_eq!(parse_exts("DLL, dll, .Dll"), Ok(vec!["dll".into()]), "each once");
        assert_eq!(parse_exts("   "), Ok(Vec::new()));
        assert_eq!(parse_exts("dll, tar.gz"), Err("tar.gz".into()), "the piece that is no extension is named");
        assert_eq!(parse_exts(r"C:\x"), Err(r"C:\x".into()));
    }

    #[test]

    #[cfg_attr(not(windows), ignore = "Windows paths and APIs; a Unix test comes when the Mac port uses this")]
    fn a_files_extension() {
        assert_eq!(ext_of(Path::new(r"C:\a\Lib.DLL")), Some("dll".into()));
        assert_eq!(ext_of(Path::new(r"C:\a\arşiv.tar.gz")), Some("gz".into()));
        assert_eq!(ext_of(Path::new(r"C:\a\README")), None);
        assert_eq!(ext_of(Path::new(r"C:\a\.gitignore")), None);
    }

    #[test]
    fn the_first_list_that_names_an_extension_is_its_list() {
        let rules = [rule(&["dll", "sys"], &[r"C:\t\dnSpy.exe", r"C:\t\ghidraRun.bat"], true), rule(&["js"], &[r"C:\t\codium.exe"], true), rule(&["dll"], &[r"C:\never.exe"], true)];
        assert_eq!(programs_for(&rules, "dll"), [r"C:\t\dnSpy.exe".to_string(), r"C:\t\ghidraRun.bat".to_string()]);
        assert_eq!(programs_for(&rules, "DLL").len(), 2, "cases are not told apart");
        assert_eq!(programs_for(&rules, "sys").len(), 2);
        assert_eq!(default_program(&rules, "js"), Some(r"C:\t\codium.exe"));
        assert!(programs_for(&rules, "txt").is_empty());
        assert_eq!(default_program(&rules, "txt"), None);
        assert_eq!(default_program(&[], "dll"), None);
    }

    #[test]
    fn a_list_can_leave_the_double_click_to_windows() {
        let rules = [rule(&["exe"], &[r"C:\t\ghidraRun.bat"], false), rule(&["bin"], &[], true)];
        assert_eq!(default_program(&rules, "exe"), None, "a double click still runs the program");
        assert_eq!(programs_for(&rules, "exe").len(), 1, "and the tool is offered under Open with");
        assert_eq!(default_program(&rules, "bin"), None, "a list without programs opens nothing");
        // a new list for something that runs by itself starts that way; any other starts with the double click on
        assert!(!new_rule(vec!["exe".into()]).double_click);
        assert!(!new_rule(vec!["txt".into(), "bat".into()]).double_click);
        assert!(new_rule(vec!["dll".into(), "js".into()]).double_click);
        assert!(runs_by_itself("EXE") && runs_by_itself("lnk") && !runs_by_itself("dll") && !runs_by_itself(""));
        assert!(OpenRule::default().double_click, "a list read from a file without the key");
    }

    #[test]
    fn an_extension_belongs_to_one_list() {
        let rules = [rule(&["dll", "sys"], &[], true), rule(&["js"], &[], true)];
        let typed: Vec<String> = vec!["js".into(), "ts".into(), "dll".into()];
        assert_eq!(taken(&rules, None, &typed), ["js".to_string(), "dll".to_string()]);
        assert_eq!(taken(&rules, Some(1), &typed), ["dll".to_string()], "its own extensions are not taken from the list being changed");
        assert!(taken(&rules, Some(0), &["dll".into(), "sys".into(), "bin".into()]).is_empty());
        assert!(taken(&[], None, &typed).is_empty());
    }

    #[test]

    #[cfg_attr(not(windows), ignore = "Windows paths and APIs; a Unix test comes when the Mac port uses this")]
    fn a_programs_path_and_name() {
        assert_eq!(clean_program(r#"  "C:\Program Files\dnSpy\dnSpy.exe"  "#), Some(r"C:\Program Files\dnSpy\dnSpy.exe".into()));
        assert_eq!(clean_program(r"C:\t\x.exe"), Some(r"C:\t\x.exe".into()));
        assert_eq!(clean_program("  \"\"  "), None);
        assert_eq!(clean_program(""), None);
        assert_eq!(program_label(r"C:\Program Files\dnSpy\dnSpy.exe"), "dnSpy");
        assert_eq!(program_label(r"C:\ghidra\ghidraRun.bat"), "ghidraRun");
        assert_eq!(program_label("notepad"), "notepad");
        assert_eq!(program_label(""), "");
    }

    #[test]
    fn a_program_moves_one_place_and_stays_at_an_end() {
        let l = ["a", "b", "c"];
        assert_eq!(moved(&l, 2, -1), ["a", "c", "b"]);
        assert_eq!(moved(&l, 0, 1), ["b", "a", "c"]);
        assert_eq!(moved(&l, 0, -1), l, "the first stays first");
        assert_eq!(moved(&l, 2, 1), l, "the last stays last");
        assert_eq!(moved(&l, 2, -9), ["c", "a", "b"], "a long way is the end");
        assert_eq!(moved(&l, 7, -1), l, "an index that is not there changes nothing");
        assert_eq!(moved::<&str>(&[], 0, 1), Vec::<&str>::new());
    }

    #[test]
    fn the_file_is_handed_to_the_program_in_quotes() {
        assert_eq!(file_argument(Path::new(r"C:\my files\a b.dll")), r#""C:\my files\a b.dll""#);
        assert_eq!(file_argument(Path::new(r"D:\")), r#""D:\\""#, "a closing backslash does not swallow the quote");
        assert_eq!(exts_line(&rule(&["dll", "sys"], &[], true)), ".dll, .sys");
        assert_eq!(exts_line(&rule(&[], &[], true)), "");
    }

    #[test]
    fn a_list_survives_the_settings_file() {
        let r = rule(&["dll"], &[r"C:\t\dnSpy.exe"], false);
        let text = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<OpenRule>(&text).unwrap(), r);
        // a list written by hand without the switch: the double click is on
        let by_hand: OpenRule = serde_json::from_str(r#"{"exts":["js"],"programs":["C:\\x.exe"]}"#).unwrap();
        assert!(by_hand.double_click);
    }
}
