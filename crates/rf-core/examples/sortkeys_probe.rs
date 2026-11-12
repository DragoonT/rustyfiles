//! Windowless probe: what the new sort keys and the thumbnail cache's eviction rule cost on a
//! folder of 100 000 entries. Reads and writes nothing on disk, shows no window: it makes the
//! entries in memory and prints timings.
//!
//!   cargo run --offline --release -p rf-core --example sortkeys_probe

use std::time::Instant;

use rf_core::sortkeys::{self, NO_RANK};
use rf_core::thumbcache;
use rf_core::{Entry, EntryKind, SortDir, SortKey, SortSpec, attrs};

fn main() {
    const N: usize = 100_000;
    let entries: Vec<Entry> = (0..N)
        .map(|i| {
            // a third of the files are cloud placeholders, a tenth are kept here on purpose
            let a = match i % 10 {
                0 => sortkeys::ATTR_PINNED,
                1..=3 => attrs::OFFLINE,
                _ => attrs::ARCHIVE,
            };
            Entry::new(&format!("file{:06}.txt", (i * 7919) % N), EntryKind::File, i as u64, i as i64, i as i64, a)
        })
        .collect();
    // 1 000 tagged items among them, in 7 tags; 500 folders for the path sort
    let tag_ranks: Vec<u32> = (0..N).map(|i| if i % 100 == 0 { (i / 100 % 7) as u32 } else { NO_RANK }).collect();
    let folders: Vec<String> = (0..N).map(|i| format!(r"C:\Users\x\Projects\folder{:03}\sub{}", i % 500, i % 3)).collect();

    let time = |label: &str, f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        println!("{label:<44} {:>8.1} ms", t.elapsed().as_secs_f64() * 1000.0);
    };

    let mut idx: Vec<u32> = (0..N as u32).collect();
    println!("{N} entries (release build)");
    time("sort by name (the default)", &mut || {
        SortSpec::default().sort_indices(&entries, &mut idx);
    });
    idx = (0..N as u32).collect();
    time("sort by size", &mut || {
        SortSpec { key: SortKey::Size, ..SortSpec::default() }.sort_indices(&entries, &mut idx);
    });
    idx = (0..N as u32).collect();
    time("sort by sync state (from attributes)", &mut || {
        SortSpec { key: SortKey::Sync, ..SortSpec::default() }.sort_indices(&entries, &mut idx);
    });
    let mut ranks = Vec::new();
    time("path ranks (500 x 3 distinct folders)", &mut || {
        ranks = sortkeys::rank_paths(folders.iter().map(String::as_str));
    });
    idx = (0..N as u32).collect();
    time("sort by path, given the ranks", &mut || {
        SortSpec { key: SortKey::Path, ..SortSpec::default() }.sort_indices_ranked(&entries, &ranks, &mut idx);
    });
    idx = (0..N as u32).collect();
    time("sort by tag (1 000 tagged), given the ranks", &mut || {
        SortSpec { key: SortKey::Tag, dir: SortDir::Desc, ..SortSpec::default() }.sort_indices_ranked(&entries, &tag_ranks, &mut idx);
    });
    let untagged_last = idx.iter().rev().take(N - 1000).all(|&i| tag_ranks[i as usize] == NO_RANK);
    println!("descending tag sort keeps the {} untagged last: {untagged_last}", N - 1000);

    // the thumbnail cache's eviction rule over 100 000 tracked pictures of 300 KB against 64 MB
    let items: Vec<(u64, u64, usize)> = (0..N as u64).map(|k| (k, k, 300 * 1024)).collect();
    let total: usize = items.iter().map(|&(_, _, b)| thumbcache::charge(b)).sum();
    let mut gone = 0;
    time("eviction plan, 100 000 tracked, 64 MB budget", &mut || {
        gone = thumbcache::eviction_victims(&items, total, thumbcache::budget_bytes(None)).len();
    });
    println!("pictures dropped: {gone} of {N}");
}
