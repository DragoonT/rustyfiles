//! Read-only probe of the property columns: how many properties the system lists, and for the first
//! 200 files of a folder how many values each of five common properties returned and how long the
//! batch took. Prints counts and times only (no names, paths or values). Opens no window.
//!
//! `cargo run --release -p rf-win --example propcols_probe -- <folder> [--compare]`
//!
//! `--compare` repeats the batch with other flag sets of the property store (to choose the flags).
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use rf_win::propcols::{self, Listing, PropSource, ReadFail, SystemSource};

const WANTED: [&str; 5] = ["System.Media.Duration", "System.Image.Dimensions", "System.Photo.DateTaken", "System.Music.Artist", "System.Title"];

fn batch(label: &str, flags: Option<i32>, files: &[PathBuf]) {
    let src: Arc<dyn PropSource> = match flags {
        Some(f) => Arc::new(SystemSource::with_flags(Duration::from_secs(5), f)),
        None => Arc::new(SystemSource::new(Duration::from_secs(5))),
    };
    let names: Vec<Arc<str>> = WANTED.iter().map(|s| Arc::from(*s)).collect();
    let mut with_value = [0usize; 5];
    let (mut ok, mut online, mut unavailable, mut timeout) = (0, 0, 0, 0);
    let t = Instant::now();
    let end = propcols::read_batch(&*src, files.iter().cloned().enumerate(), &names, &AtomicBool::new(false), |_, r| match r {
        Ok(cells) => {
            ok += 1;
            for (i, c) in cells.iter().enumerate() {
                with_value[i] += usize::from(!c.value.is_empty());
            }
        }
        Err(ReadFail::OnlineOnly) => online += 1,
        Err(ReadFail::Unavailable) => unavailable += 1,
        Err(ReadFail::Timeout) => timeout += 1,
    });
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    println!("{label}: {end:?}; {ok} files read, {online} online-only skipped, {unavailable} unavailable, {timeout} timed out");
    for (n, c) in WANTED.iter().zip(with_value) {
        println!("  {n}: {c} values");
    }
    println!("  batch: {ms:.1} ms ({:.2} ms a file)", if files.is_empty() { 0.0 } else { ms / files.len() as f64 });
}

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(dir) = args.next() else {
        eprintln!("usage: propcols_probe <folder> [--compare]");
        std::process::exit(2);
    };
    let compare = args.any(|a| a == "--compare");

    let t = Instant::now();
    let viewable = propcols::list_properties(Listing::Viewable);
    let t_viewable = t.elapsed();
    let t = Instant::now();
    let columns = propcols::list_properties(Listing::Column);
    println!(
        "properties the system lists: {} viewable ({:.1} ms the first time, which loads the schema), {} with a column definition ({:.1} ms)",
        viewable.len(),
        t_viewable.as_secs_f64() * 1000.0,
        columns.len(),
        t.elapsed().as_secs_f64() * 1000.0
    );
    let known = WANTED.iter().filter(|w| viewable.iter().any(|p| p.canonical == **w)).count();
    println!("of the five common ones, {known} are in the list");

    // the first 200 files, in the order the file system gives them
    let mut files: Vec<PathBuf> = Vec::new();
    let mut folders = 0usize;
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if files.len() >= 200 {
                break;
            }
            match e.file_type() {
                Ok(ft) if ft.is_file() => files.push(e.path()),
                Ok(_) => folders += 1,
                Err(_) => {}
            }
        }
    }
    println!("folder: {} files taken, {folders} folders passed over", files.len());
    if files.is_empty() {
        return;
    }
    batch("default flags (best effort, no oplock)", None, &files);
    if compare {
        // best effort + no oplock + prefer the search index's copy
        batch("plus prefer-query-properties", Some(64 | 128 | 256), &files);
        // best effort + no oplock + only properties that are quick to get
        batch("plus fast-properties-only", Some(64 | 128 | 8), &files);
        // a second run of the default set: the system's own caches are warm
        batch("default flags, second run", None, &files);
    }
}
