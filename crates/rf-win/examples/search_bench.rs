//! Subfolder search, index against our own walk:
//! `cargo run --release -p rf-win --example search_bench -- <folder> <query> [runs]`.
//! Prints, per method, the time to the first hit and to the last, and the hit count.

use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    let mut a = std::env::args().skip(1);
    let root = PathBuf::from(a.next().expect("folder"));
    let query = a.next().expect("query");
    let runs: usize = a.next().map_or(3, |s| s.parse().unwrap());
    let indexed = rf_win::wsearch::is_indexed(&root);
    println!("{} \"{query}\" (indexed: {indexed})", root.display());
    for run in 0..runs {
        if indexed {
            let t = Instant::now();
            let mut first: Option<Duration> = None;
            let n = rf_win::wsearch::search(&root, &query, 50_000, &|| false, &mut |h| {
                if !h.is_empty() && first.is_none() {
                    first = Some(t.elapsed());
                }
            })?;
            println!("  run {run} index : first {:>8.1?}  all {:>8.1?}  {n} hits", first.unwrap_or_default(), t.elapsed());
        }
        let t = Instant::now();
        let mut first: Option<Duration> = None;
        let q = rf_win::search::SearchQuery { root: root.clone(), pattern: query.clone(), include_hidden: false, max_results: 50_000 };
        let s = rf_win::search::search(&q, &|| false, &mut |h| {
            if !h.is_empty() && first.is_none() {
                first = Some(t.elapsed());
            }
        })?;
        println!(
            "  run {run} walk  : first {:>8.1?}  all {:>8.1?}  {} hits, {} folders, {} entries",
            first.unwrap_or_default(),
            t.elapsed(),
            s.hits,
            s.dirs_scanned,
            s.entries_scanned
        );
    }
    Ok(())
}
