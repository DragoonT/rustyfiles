//! Folder organizer (rule based): the pure part. No file system access in here except the rules
//! file in [`store`]; the planning ([`plan`]) works on lists of file facts the caller collected.
//!
//! * [`rule`]: what a rule is, how its conditions match, the template variables.
//! * [`plan`]: rules + facts -> a list of steps (move / rename / recycle) that never overwrite.
//! * [`safety`]: folders that may not be watched, files that must be left alone.
//! * [`store`]: the versioned rules file.
//! * [`summary`]: a rule said in words.

/// Marks a user-facing Turkish text in this module. It is the identity; a test in rf-app finds
/// every `T!("...")` in these files and checks that the English pair exists (the texts are
/// translated where they are drawn, with the app's own i18n).
macro_rules! T {
    ($s:literal) => {
        $s
    };
}

pub mod plan;
pub mod rule;
pub mod safety;
pub mod store;
pub mod summary;

pub use plan::{Dest, Plan, PlanContext, SkipReason, Skipped, Step, Unusable, numbered_name, plan, preview_dest};
pub use rule::{Action, Cmp, Condition, FileFact, FileKind, Join, NameOp, Rule, RuleProblem, TEMPLATE_VARS, TimeField};
pub use safety::{ProtectedPaths, Refusal, is_partial_download};
pub use store::{LoadOutcome, RulesFile};
