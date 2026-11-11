//! Tags as a GPUI global, persisted to %APPDATA%\rustyfiles\tags.json.

use std::path::PathBuf;

use gpui_kit::*;
use rf_core::tags::TagStore;

pub struct Tags(pub TagStore);
impl Global for Tags {}

fn path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join("tags.json"))
}

pub fn load() -> Tags {
    Tags(path().map(|p| TagStore::load(&p)).unwrap_or_default())
}

pub fn update(cx: &mut App, f: impl FnOnce(&mut TagStore)) {
    cx.update_global::<Tags, _>(|t, _| f(&mut t.0));
    let snapshot = cx.global::<Tags>().0.clone();
    std::thread::spawn(move || {
        if let Some(p) = path() {
            if let Err(e) = snapshot.save(&p) {
                log::warn!("tags save: {e}");
            }
        }
    });
    cx.refresh_windows();
}

pub fn color(c: u32) -> Hsla {
    rgb(c).into()
}

/// How a tag joins the result of a tag query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Join {
    /// Items that have this tag too.
    And,
    /// Also the items that have this tag.
    Or,
    /// Without the items that have this tag.
    Not,
}

/// The tags of a query like `kırmızı AND iş`, `kırmızı OR mavi NOT bitti` (after `tag:`), in
/// order. Operators are AND / OR / NOT, VE / VEYA / DEĞİL (any case) or `&`, `|`, `!`; a tag
/// name may have spaces. The first tag joins with OR unless the query starts with NOT.
/// `text` is case-folded already.
pub fn parse_query(text: &str) -> Vec<(Join, String)> {
    let mut out: Vec<(Join, String)> = Vec::new();
    let mut join = Join::Or;
    let mut name = String::new();
    let flush = |join: Join, name: &mut String, out: &mut Vec<(Join, String)>| {
        if !name.is_empty() {
            out.push((join, std::mem::take(name)));
        }
    };
    for word in text.split_whitespace() {
        let op = match word {
            "and" | "ve" | "&" | "&&" => Some(Join::And),
            "or" | "veya" | "|" | "||" => Some(Join::Or),
            "not" | "değil" | "degil" | "!" => Some(Join::Not),
            _ => None,
        };
        match op {
            Some(op) => {
                flush(join, &mut name, &mut out);
                join = op;
            }
            None => {
                if !name.is_empty() {
                    name.push(' ');
                }
                name.push_str(word);
            }
        }
    }
    flush(join, &mut name, &mut out);
    out
}

/// The paths a tag query gives: the terms are applied left to right (no brackets). `with`
/// gives the paths of a tag by its (case-folded) name, empty for a name no tag has; `all` is
/// every tagged path (what a query starting with NOT takes from).
pub fn eval_query(terms: &[(Join, String)], with: &dyn Fn(&str) -> Vec<PathBuf>, all: &dyn Fn() -> Vec<PathBuf>) -> Vec<PathBuf> {
    use std::collections::BTreeSet;
    let mut set: BTreeSet<PathBuf> = BTreeSet::new();
    for (i, (join, name)) in terms.iter().enumerate() {
        let tagged: BTreeSet<PathBuf> = with(name).into_iter().collect();
        match join {
            Join::Or => set.extend(tagged),
            Join::And => set.retain(|p| tagged.contains(p)),
            Join::Not => {
                if i == 0 {
                    set = all().into_iter().collect();
                }
                set.retain(|p| !tagged.contains(p));
            }
        }
    }
    set.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::{Join, eval_query, parse_query};
    use std::path::PathBuf;

    #[test]
    fn tag_queries_with_and_or_not() {
        assert_eq!(parse_query("kırmızı"), vec![(Join::Or, "kırmızı".into())]);
        assert_eq!(parse_query("kırmızı and iş dosyası"), vec![(Join::Or, "kırmızı".into()), (Join::And, "iş dosyası".into())]);
        assert_eq!(parse_query("kırmızı veya mavi değil bitti"), vec![(Join::Or, "kırmızı".into()), (Join::Or, "mavi".into()), (Join::Not, "bitti".into())]);
        assert_eq!(parse_query("not bitti"), vec![(Join::Not, "bitti".into())]);
        assert_eq!(parse_query("  "), vec![]);

        let p = |s: &str| PathBuf::from(s);
        let with = |name: &str| -> Vec<PathBuf> {
            match name {
                "kırmızı" => vec![p("a"), p("b")],
                "mavi" => vec![p("b"), p("c")],
                "bitti" => vec![p("c")],
                _ => Vec::new(),
            }
        };
        let all = || vec![p("a"), p("b"), p("c")];
        let run = |q: &str| eval_query(&parse_query(q), &with, &all);
        assert_eq!(run("kırmızı"), vec![p("a"), p("b")]);
        assert_eq!(run("kırmızı and mavi"), vec![p("b")]);
        assert_eq!(run("kırmızı or mavi"), vec![p("a"), p("b"), p("c")]);
        assert_eq!(run("kırmızı or mavi not bitti"), vec![p("a"), p("b")]);
        assert_eq!(run("not kırmızı"), vec![p("c")]);
        // a name no tag has: nothing with AND, no change with OR / NOT
        assert!(run("kırmızı and yok").is_empty());
        assert_eq!(run("kırmızı or yok"), vec![p("a"), p("b")]);
    }
}
