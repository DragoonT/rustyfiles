//! Bulk rename for the Mac app: rf-core's name computation (`rf_core::rename`) and its
//! swap-safe plan. The app does the renames (with its undo).

use rf_core::rename::{self, CaseKind, Mode, RenameError, RenameItem, RenameSpec};

use crate::organize::tr;

/// What the names become.
#[derive(Debug, Clone, uniffi::Enum)]
pub enum RenameMode {
    FindReplace { find: String, replace: String, case_insensitive: bool, regex: bool },
    /// `{n}` / `{n:03}` counter, `{name}`, `{ext}`, `{date}`.
    Pattern { pattern: String, start: i64, step: i64 },
    /// 0 lower, 1 UPPER, 2 Title.
    Case { kind: u8 },
    PrefixSuffix { prefix: String, suffix: String },
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenameInput {
    pub name: String,
    pub is_dir: bool,
    pub modified_ms: i64,
}

/// One item's new name, or why it cannot have it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct RenameOutcome {
    pub name: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct RenameStep {
    pub from: String,
    pub to: String,
}

fn message(e: &RenameError) -> String {
    match e {
        RenameError::Empty => tr("Ad boş olamaz"),
        RenameError::InvalidChars => tr("Geçersiz karakter: < > : \" / \\ | ? * veya denetim karakteri"),
        RenameError::TrailingDotSpace => tr("Ad nokta veya boşlukla bitemez"),
        RenameError::Reserved => tr("Ayrılmış Windows adı"),
        RenameError::TooLong => tr("Ad çok uzun"),
        RenameError::Duplicate => tr("Aynı ada çıkan başka bir öğe var"),
        RenameError::Exists => tr("Klasörde bu ada sahip başka bir öğe var"),
        RenameError::BadRegex(m) => format!("{}: {m}", tr("Geçersiz düzenli ifade")),
    }
}

/// The new names of `items` (same order). `existing`: the folder's other names, which the new
/// ones must not take. `include_ext`: the mode works on the whole name, extension too.
#[uniffi::export]
pub fn rename_preview(items: Vec<RenameInput>, mode: RenameMode, include_ext: bool, existing: Vec<String>) -> Vec<RenameOutcome> {
    let items: Vec<RenameItem> = items.into_iter().map(|i| RenameItem::new(i.name, i.is_dir, i.modified_ms)).collect();
    let mode = match mode {
        RenameMode::FindReplace { find, replace, case_insensitive, regex } => Mode::FindReplace { find, replace, case_insensitive, regex },
        RenameMode::Pattern { pattern, start, step } => Mode::Pattern { pattern, start, step },
        RenameMode::Case { kind } => Mode::Case(match kind {
            0 => CaseKind::Lower,
            1 => CaseKind::Upper,
            _ => CaseKind::Title,
        }),
        RenameMode::PrefixSuffix { prefix, suffix } => Mode::PrefixSuffix { prefix, suffix },
    };
    let spec = RenameSpec { mode, include_ext, existing };
    rename::compute(&items, &spec)
        .into_iter()
        .zip(&items)
        .map(|(r, item)| match r {
            Ok(name) => RenameOutcome { name, error: None },
            Err(e) => RenameOutcome { name: item.name.clone(), error: Some(message(&e)) },
        })
        .collect()
}

/// The renames to make, in an order that never overwrites (a name another item still has goes
/// through a temporary name first). Names only: all in one folder.
#[uniffi::export]
pub fn rename_plan(pairs: Vec<RenameStep>) -> Vec<RenameStep> {
    let pairs: Vec<(String, String)> = pairs.into_iter().map(|p| (p.from, p.to)).collect();
    let pid = std::process::id();
    rename::plan(&pairs, |i| format!(".rf-rename-{pid}-{i}.tmp")).into_iter().map(|s| RenameStep { from: s.from, to: s.to }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_counter_pattern_and_a_swap() {
        let items = vec![
            RenameInput { name: "b.jpg".into(), is_dir: false, modified_ms: 0 },
            RenameInput { name: "a.jpg".into(), is_dir: false, modified_ms: 0 },
        ];
        let out = rename_preview(items, RenameMode::Pattern { pattern: "Trip {n:02}".into(), start: 1, step: 1 }, false, vec![]);
        assert_eq!(out.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["Trip 01.jpg", "Trip 02.jpg"]);
        assert!(out.iter().all(|o| o.error.is_none()));
        // a.jpg <-> b.jpg goes through a temporary name
        let steps = rename_plan(vec![RenameStep { from: "a.jpg".into(), to: "b.jpg".into() }, RenameStep { from: "b.jpg".into(), to: "a.jpg".into() }]);
        assert_eq!(steps.len(), 4);
        assert!(steps[0].to.starts_with(".rf-rename-"));
        let clash = rename_preview(
            vec![RenameInput { name: "x.txt".into(), is_dir: false, modified_ms: 0 }],
            RenameMode::PrefixSuffix { prefix: "".into(), suffix: "".into() },
            false,
            vec![],
        );
        assert_eq!(clash[0].name, "x.txt");
    }
}
