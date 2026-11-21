//! Opening a file the way the user set it up: with their own program for its type when there is one (`rf_core::openrules`), else
//! with Windows' default. One table for the process, set from the settings; the opening itself happens on whatever thread asks
//! (never the UI thread: starting a program can take a moment).

use std::path::Path;
use std::sync::{Arc, RwLock};

use rf_core::openrules::{self, OpenRule};

static RULES: RwLock<Option<Arc<Vec<OpenRule>>>> = RwLock::new(None);

/// The user's lists, as the settings have them now.
pub fn set(rules: Vec<OpenRule>) {
    if let Ok(mut r) = RULES.write() {
        *r = Some(Arc::new(rules));
    }
}

/// The lists the process follows.
pub fn rules() -> Arc<Vec<OpenRule>> {
    RULES.read().ok().and_then(|r| r.clone()).unwrap_or_default()
}

/// The user's programs for this file ("Open with"), in their order; empty when its type has no list.
pub fn programs_for(path: &Path) -> Vec<String> {
    match openrules::ext_of(path) {
        Some(ext) => openrules::programs_for(&rules(), &ext).to_vec(),
        None => Vec::new(),
    }
}

/// How a file was opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Opened {
    /// Windows' default for the type.
    Default,
    /// The user's program for the type.
    With(String),
    /// The user's program could not be started (moved, removed): Windows' default opened the file instead.
    Fallback { program: String, error: String },
}

/// What a double click on this file opens it with: the user's program, or `None` for Windows' default. A folder is never a rule's
/// (a folder can be called "notes.js").
pub fn default_for(path: &Path, rules: &[OpenRule], is_dir: impl Fn(&Path) -> bool) -> Option<String> {
    let ext = openrules::ext_of(path)?;
    let program = openrules::default_program(rules, &ext)?;
    (!is_dir(path)).then(|| program.to_string())
}

/// Opens a file as a double click does: with the user's program for its type when there is one, else with Windows' default. A
/// program that cannot be started does not leave the file unopened: Windows' default is used and the answer says so.
pub fn open(path: &Path) -> anyhow::Result<Opened> {
    open_in_order(path, rf_core::sort::SortKey::Name, true)
}

/// [`open`], for a file shown in a folder sorted this way: a packaged app (Photos, Media Player) is told about the folder's other
/// files in this order, so its Previous and Next walk the folder as the list does (`neighbours`, issue 70).
pub fn open_in_order(path: &Path, key: rf_core::sort::SortKey, ascending: bool) -> anyhow::Result<Opened> {
    let Some(program) = default_for(path, &rules(), |p| p.is_dir()) else {
        if crate::neighbours::opens_in_packaged_app(path) {
            match crate::neighbours::launch(path, crate::neighbours::sort_property(key), ascending) {
                Ok(()) => return Ok(Opened::Default),
                Err(e) => log::warn!("open {path:?} with its folder: {e:#}; opened alone"),
            }
        }
        crate::shell::open(path)?;
        return Ok(Opened::Default);
    };
    match open_in(&program, path) {
        Ok(()) => Ok(Opened::With(program)),
        Err(e) => {
            log::warn!("open rules: {program:?} could not be started for {path:?}: {e:#}");
            crate::shell::open(path)?;
            Ok(Opened::Fallback { program, error: format!("{e:#}") })
        }
    }
}

/// Starts `program` with the file as its argument, in the file's folder (what "Open with" does for a program the user chose).
pub fn open_in(program: &str, file: &Path) -> anyhow::Result<()> {
    crate::shell::run_program(Path::new(program), &openrules::file_argument(file), file.parent())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(exts: &[&str], programs: &[&str], double_click: bool) -> OpenRule {
        OpenRule { exts: exts.iter().map(|s| s.to_string()).collect(), programs: programs.iter().map(|s| s.to_string()).collect(), double_click }
    }

    #[test]
    fn a_double_click_takes_the_users_program_only_for_a_file_of_a_listed_type() {
        let rules = [rule(&["dll"], &[r"C:\t\dnSpy.exe", r"C:\t\ghidra.bat"], true), rule(&["exe"], &[r"C:\t\ghidra.bat"], false)];
        let file = |_: &Path| false;
        assert_eq!(default_for(Path::new(r"C:\a\Lib.DLL"), &rules, file), Some(r"C:\t\dnSpy.exe".into()));
        assert_eq!(default_for(Path::new(r"C:\a\notes.txt"), &rules, file), None);
        assert_eq!(default_for(Path::new(r"C:\a\setup.exe"), &rules, file), None, "the list leaves the double click to Windows");
        assert_eq!(default_for(Path::new(r"C:\a\README"), &rules, file), None);
        // a folder whose name ends like a listed type is still a folder
        assert_eq!(default_for(Path::new(r"C:\a\backup.dll"), &rules, |_| true), None);
        assert_eq!(default_for(Path::new(r"C:\a\Lib.dll"), &[], file), None);
    }

    /// A program that is not there cannot be started: the error comes back and nothing else happens (the fallback to Windows'
    /// default is `open`'s, which would open a real file and is not run by a test).
    #[test]
    fn a_program_that_is_not_there_is_an_error() {
        let missing = std::env::temp_dir().join(format!("rf-openrules-{}", std::process::id())).join("no-such-program.exe");
        assert!(open_in(&missing.to_string_lossy(), Path::new(r"C:\Windows\win.ini")).is_err());
    }
}
