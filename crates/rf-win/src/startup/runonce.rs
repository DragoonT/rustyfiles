//! The name prefixes of a RunOnce value, as Microsoft documents them (research note 1, section 5.1 item 3, from the Win32 page
//! "Run and RunOnce Registry Keys"):
//!
//! * by default the value is deleted BEFORE the command runs (a command that fails is not tried again);
//! * `!` in front of the name: the deletion waits until the command has run;
//! * `*` in front of the name: the command runs in Safe Mode too (without it a RunOnce value does not run there);
//! * the two may be combined, in either order.
//!
//! Nothing else is read into a prefix: any other leading character belongs to the name.

use super::model::RunOnceFlags;

/// Splits a RunOnce value name into its flags and the name without the prefix characters.
pub fn parse_name(name: &str) -> (RunOnceFlags, &str) {
    let mut flags = RunOnceFlags::default();
    let mut rest = name;
    loop {
        match rest.as_bytes().first() {
            Some(b'!') => flags.delete_after = true,
            Some(b'*') => flags.run_in_safe_mode = true,
            _ => break,
        }
        rest = &rest[1..];
    }
    (flags, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_names_have_no_flags() {
        assert_eq!(parse_name("msedge_cleanup"), (RunOnceFlags::default(), "msedge_cleanup"));
        assert_eq!(parse_name(""), (RunOnceFlags::default(), ""));
    }

    #[test]
    fn the_documented_prefixes() {
        assert_eq!(parse_name("!job"), (RunOnceFlags { delete_after: true, run_in_safe_mode: false }, "job"));
        assert_eq!(parse_name("*job"), (RunOnceFlags { delete_after: false, run_in_safe_mode: true }, "job"));
        assert_eq!(parse_name("*!job"), (RunOnceFlags { delete_after: true, run_in_safe_mode: true }, "job"));
        assert_eq!(parse_name("!*job"), (RunOnceFlags { delete_after: true, run_in_safe_mode: true }, "job"));
        assert_eq!(parse_name("!!*job").1, "job");
    }

    #[test]
    fn a_prefix_character_inside_the_name_is_part_of_the_name() {
        assert_eq!(parse_name("job!"), (RunOnceFlags::default(), "job!"));
        assert_eq!(parse_name("a*b").1, "a*b");
        assert_eq!(parse_name("!").1, "");
        // multi-byte characters after the prefixes are not cut in the middle
        assert_eq!(parse_name("*İş").1, "İş");
    }
}
