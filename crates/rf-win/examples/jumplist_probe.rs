//! Windowless, read-only probe of the jump-list reader (shell plan increment B8, data side). Prints COUNTS and timings only: never an
//! app's name, an item's title or a path.
//!
//! 1. THE FILE NAMES. The AppID hash's parameters, settled against this machine: the identities of the "All apps" list (and the
//!    programs they start) hashed under every usual CRC-64 parameter set and text form, counted against the file names that exist in
//!    AutomaticDestinations and CustomDestinations.
//! 2. THE PARSERS against every real file: how many automatic files hold a DestList that parses (by version), whether every entry
//!    names a stream that exists, and how many custom files are read to their last footer.
//! 3. THE READER for the pinned and the running apps: per app (numbered), the count of items in each part and the time it took.
//!
//! Read-only: the jump-list files are read into memory with the ordinary shared read (what Explorer allows), the shell's own lists are
//! asked through IApplicationDocumentLists for apps that already have a file, nothing is written, started, removed or cleared. To show
//! that, every file of the two folders is noted (size and last-write time) before the reader part and after it, and the run prints how
//! many changed, split into the files of the apps the reader asked about and all the others (Explorer and apps write these files on
//! their own, so the others are the background rate: a reader that wrote would show up in the first group alone).
//! A hidden message window of the shell thread is the only window.
//!
//!   cargo run -p rf-win --example jumplist_probe
//!   cargo run -p rf-win --example jumplist_probe -- --no-reader     (parts 1 and 2 only)

use std::collections::HashSet;
use std::time::Instant;

use rf_win::dock::identity;
use rf_win::dock::pins;
use rf_win::jumplists::appid::{self, Form};
use rf_win::jumplists::{self, AppKey, Real};

fn count_files(dir: &std::path::Path) -> usize {
    std::fs::read_dir(dir).map(|r| r.flatten().count()).unwrap_or(0)
}

/// (lower-case file name -> (size, last write)) of the files in `dir`; names are only ever compared, never printed.
type Snapshot = std::collections::HashMap<String, (u64, Option<std::time::SystemTime>)>;

fn snapshot(dir: &std::path::Path) -> Snapshot {
    let mut m = Snapshot::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if let Ok(md) = e.metadata() {
                m.insert(e.file_name().to_string_lossy().to_lowercase(), (md.len(), md.modified().ok()));
            }
        }
    }
    m
}

/// How many files differ between two snapshots (changed, added, removed), split by whether the file's stem is in `asked`.
fn differences(a: &Snapshot, b: &Snapshot, asked: &HashSet<String>) -> [(usize, usize); 3] {
    let stem_asked = |n: &str| n.split('.').next().is_some_and(|s| asked.contains(s));
    let mut out = [(0, 0); 3]; // (asked, others) for changed, added, removed
    let mut bump = |i: usize, name: &str| {
        if stem_asked(name) { out[i].0 += 1 } else { out[i].1 += 1 }
    };
    for (n, v) in b {
        match a.get(n) {
            Some(old) if old != v => bump(0, n),
            None => bump(1, n),
            _ => {}
        }
    }
    for n in a.keys() {
        if !b.contains_key(n) {
            bump(2, n);
        }
    }
    out
}

fn main() {
    let mut asked: HashSet<String> = HashSet::new();
    let Some(root) = appid::recent_root() else {
        println!("APPDATA is not set");
        return;
    };
    let (auto_dir, custom_dir) = (root.join("AutomaticDestinations"), root.join("CustomDestinations"));
    let before = (count_files(&auto_dir), count_files(&custom_dir));
    let auto = appid::stems_in(&auto_dir, appid::AUTOMATIC_EXT);
    let custom = appid::stems_in(&custom_dir, appid::CUSTOM_EXT);
    println!("== FILES: {} automatic, {} custom", before.0, before.1);

    // ---------------------------------------------------------------- 1. the file names
    let t = Instant::now();
    let apps = rf_win::apps::list().unwrap_or_default();
    println!("\n== 1. APPID HASH, settled against {} apps of 'All apps' (listed in {:?})", apps.len(), t.elapsed());
    let ids: Vec<String> = apps.iter().map(|a| a.id.clone()).collect();
    let t = Instant::now();
    let rows = appid::sweep(&ids, &auto, &custom);
    println!("sweep: {} parameter-and-text combinations in {:?}", rows.len(), t.elapsed());
    println!("{:<4} {:<11} {:<18} {:<18} {:<8} {:<10} {:<10} {:>6} {:>6}", "rank", "polynomial", "init", "xor-out", "reflect", "text", "", "auto", "custom");
    for (i, r) in rows.iter().take(4).enumerate() {
        let p = r.params;
        println!(
            "{:<4} {:<11} {:016x}   {:016x}   in={:<5} out={:<5} {:?} {:>6} {:>6}",
            i + 1,
            format!("{:?}", p.poly),
            p.init,
            p.xor_out,
            p.reflect_in,
            p.reflect_out,
            r.text,
            r.in_first,
            r.in_second
        );
    }
    let with_hits = rows.iter().filter(|r| r.in_first + r.in_second > 0).count();
    println!("combinations that name at least one file: {with_hits} of {} (the settled one is rank 1: {})", rows.len(), rows[0].params == appid::SETTLED && rows[0].text == appid::TextForm::UpperUtf16);
    println!("note: a second row with the same parameters and text AS WRITTEN counts only the identities that are already upper case");

    // the three identity forms, per app: the AppsFolder id as written, and (for programs) the path the shell says it starts
    let folders = appid::known_folders_now();
    let t = Instant::now();
    let mut by_form: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let (mut apps_auto, mut apps_custom) = (0usize, 0usize);
    for a in &apps {
        let exe = rf_win::dock::startapps::location(a).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")));
        let cands = appid::candidates(Some(&a.id), exe.as_deref(), &folders);
        let (mut ha, mut hc) = (false, false);
        for c in &cands {
            let s = appid::file_stem(c.id);
            let (ia, ic) = (auto.contains(&s), custom.contains(&s));
            if ia || ic {
                let e = by_form.entry(format!("{:?}", c.form)).or_default();
                e.0 += ia as usize;
                e.1 += ic as usize;
            }
            ha |= ia;
            hc |= ic;
        }
        apps_auto += ha as usize;
        apps_custom += hc as usize;
    }
    println!("\napps with a file under any candidate (ids as written + the program's path forms), {:?}:", t.elapsed());
    println!("  an automatic file: {apps_auto} of {};  a custom file: {apps_custom} of {}", apps.len(), apps.len());
    for (form, (a, c)) in &by_form {
        println!("  candidate form {form:<16} names an automatic file for {a} apps, a custom file for {c}");
    }
    let _ = Form::Aumid;

    // ---------------------------------------------------------------- 2. the parsers against the real files
    println!("\n== 2. PARSERS against every real file");
    let (mut files, mut cfb_ok, mut dl_ok, mut entries, mut with_stream, mut pinned_entries, mut empty_dl) = (0, 0, 0, 0usize, 0usize, 0usize, 0);
    let mut versions: std::collections::BTreeMap<u32, usize> = Default::default();
    let (mut no_destlist, mut zero_destlist, mut not_understood) = (0, 0, 0);
    let mut declared_match = 0;
    let mut count_match = 0;
    let mut path_nonempty = 0usize;
    // every entry's shell link bytes, to load them below
    let mut doc_blobs: Vec<Vec<u8>> = Vec::new();
    let mut pin_blobs: Vec<Vec<u8>> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&auto_dir) {
        for e in rd.flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else { continue };
            files += 1;
            let Some((dl, mut streams)) = jumplists::read_destlist(&bytes) else {
                if let Ok(mut cf) = cfb::CompoundFile::open(std::io::Cursor::new(&bytes[..])) {
                    cfb_ok += 1;
                    // why there is no list: no DestList stream at all, an empty one, or one that is not understood
                    match cf.open_stream("/DestList") {
                        Err(_) => no_destlist += 1,
                        Ok(mut st) => {
                            let mut v = Vec::new();
                            let _ = std::io::Read::read_to_end(&mut st, &mut v);
                            if v.is_empty() { zero_destlist += 1 } else { not_understood += 1 }
                        }
                    }
                }
                continue;
            };
            cfb_ok += 1;
            dl_ok += 1;
            *versions.entry(dl.version).or_default() += 1;
            if dl.entries.is_empty() {
                empty_dl += 1;
            }
            entries += dl.entries.len();
            pinned_entries += dl.entries.iter().filter(|e| e.pin.is_some()).count();
            if dl.declared_pinned as usize == dl.entries.iter().filter(|e| e.pin.is_some()).count() {
                declared_match += 1;
            }
            if dl.declared_entries as usize == dl.entries.len() {
                count_match += 1;
            }
            for en in &dl.entries {
                if let Some(b) = streams.get(en.number) {
                    with_stream += 1;
                    if en.pin.is_some() { pin_blobs.push(b) } else { doc_blobs.push(b) }
                }
                if !en.path.is_empty() {
                    path_nonempty += 1;
                }
            }
        }
    }
    println!("automatic files read: {files}; compound files that open: {cfb_ok}; DestList that parses: {dl_ok} (empty lists: {empty_dl})");
    println!("DestList versions: {versions:?}; compound files with no list: {no_destlist} without a DestList stream, {zero_destlist} with an empty one, {not_understood} with one that is not understood");
    println!("entries parsed: {entries}; with a stream of that number: {with_stream}; with a path: {path_nonempty}; pinned: {pinned_entries}; files where the header's pinned count matches ours: {declared_match} of {dl_ok}; where its entry count matches ours: {count_match} of {dl_ok}");

    // the shell links of every entry, loaded from memory on the shell thread
    println!("
== 2b. SHELL LINKS FROM MEMORY (IShellLink::Load of a memory stream, never resolved)");
    let disk = rf_win::jumplists::lnk::RealDisk;
    let t = Instant::now();
    let report = |label: &str, blobs: Vec<Vec<u8>>, role: rf_win::jumplists::lnk::Role| {
        let total = blobs.len();
        let (mut loaded, mut with_target, mut with_title, mut with_aumid, mut separators, mut items, mut gone, mut nothing) = (0, 0, 0, 0, 0, 0, 0, 0);
        for chunk in blobs.chunks(400) {
            let chunk = chunk.to_vec();
            let facts = rf_win::sta::call(std::time::Duration::from_secs(60), move || rf_win::jumplists::lnk::read_links(&chunk)).unwrap_or_default();
            for f in facts.into_iter().flatten() {
                loaded += 1;
                with_target += f.target.is_some() as usize;
                with_title += f.title.is_some() as usize;
                with_aumid += f.aumid.is_some() as usize;
                separators += f.separator as usize;
                match rf_win::jumplists::lnk::classify(&f, role, &disk) {
                    Ok(_) => items += 1,
                    Err(rf_win::jumplists::lnk::Dropped::TargetGone) => gone += 1,
                    Err(rf_win::jumplists::lnk::Dropped::NothingToOpen) => nothing += 1,
                }
            }
        }
        println!("{label:<26} links: {total:>5}  loaded: {loaded:>5}  with a path: {with_target:>5}  with a title: {with_title:>5}  with an id: {with_aumid:>4}  separators: {separators:>3}  -> items: {items:>5}, dropped: {gone:>4} target gone + {nothing:>4} nothing to open + {:>4} not loadable", total - loaded);
    };
    report("pinned entries", pin_blobs, rf_win::jumplists::lnk::Role::Document);
    report("other automatic entries", doc_blobs, rf_win::jumplists::lnk::Role::Document);
    let mut task_blobs: Vec<Vec<u8>> = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&custom_dir) {
        for e in rd.flatten() {
            if let Ok(bytes) = std::fs::read(e.path()) {
                for c in jumplists::custom::parse(&bytes).categories {
                    task_blobs.extend(c.links);
                }
            }
        }
    }
    report("custom-file entries", task_blobs, rf_win::jumplists::lnk::Role::Task);
    println!("({:?} for all of them)", t.elapsed());
    let (mut cfiles, mut cempty, mut ccomplete, mut cats, mut links, mut tasks_files, mut named_files, mut recent_marks, mut freq_marks) = (0, 0, 0, 0usize, 0usize, 0, 0, 0, 0);
    if let Ok(rd) = std::fs::read_dir(&custom_dir) {
        for e in rd.flatten() {
            let Ok(bytes) = std::fs::read(e.path()) else { continue };
            cfiles += 1;
            if bytes.len() <= 24 {
                cempty += 1;
                continue;
            }
            let f = jumplists::custom::parse(&bytes);
            ccomplete += f.complete as usize;
            cats += f.categories.len();
            links += f.categories.iter().map(|c| c.links.len()).sum::<usize>();
            use jumplists::custom::CategoryKind as K;
            tasks_files += f.categories.iter().any(|c| c.kind == K::Tasks) as usize;
            named_files += f.categories.iter().any(|c| matches!(c.kind, K::Named(_))) as usize;
            recent_marks += f.categories.iter().any(|c| c.kind == K::Recent) as usize;
            freq_marks += f.categories.iter().any(|c| c.kind == K::Frequent) as usize;
        }
    }
    println!("custom files read: {cfiles} (empty: {cempty}); read to the last footer: {ccomplete} of {}; categories: {cats}; shell links: {links}", cfiles - cempty);
    println!("files with Tasks: {tasks_files}; with own categories: {named_files}; asking for Recent: {recent_marks}; for Frequent: {freq_marks}");

    // ---------------------------------------------------------------- 3. the reader
    if !std::env::args().any(|a| a == "--no-reader") {
        println!("\n== 3. THE READER (limit 10 per part), pinned apps then running apps");
        let mut keys: Vec<(String, AppKey)> = Vec::new();
        // the app with the most pins on any machine (Quick Access), as a check of the pinned path with real pins
        keys.push(("file manager".into(), AppKey::from_aumid("Microsoft.Windows.Explorer")));
        for (i, p) in pins::read_pins().iter().enumerate() {
            keys.push((format!("pin #{}", i + 1), AppKey::from_pin(p)));
        }
        for (i, (_, tasks)) in identity::running_groups().iter().enumerate() {
            let t = &tasks[0];
            keys.push((format!("running #{}", i + 1), AppKey { aumid: t.app_id.clone(), exe: t.exe.clone() }));
        }
        let snap_before = (snapshot(&auto_dir), snapshot(&custom_dir));
        println!("{:<12} {:<9} {:>5} {:>6} {:>6} {:>6} {:>7} {:>9} {:<8} {:>9}", "app", "file", "tasks", "cats", "pinned", "recent", "frequent", "which", "shell", "time");
        let mut seen: HashSet<AppKey> = HashSet::new();
        let (mut total, mut with_file, mut with_items) = (0, 0, 0);
        let mut slowest = std::time::Duration::ZERO;
        for (label, key) in keys {
            if !seen.insert(key.clone()) {
                continue;
            }
            total += 1;
            for c in appid::candidates(key.aumid.as_deref(), key.exe.as_deref(), &folders) {
                asked.insert(appid::file_stem(c.id));
            }
            let t = Instant::now();
            let (list, rep) = jumplists::read_detailed(&Real, &key, 10, jumplists::BUDGET);
            let took = t.elapsed();
            slowest = slowest.max(took);
            let file = match (rep.automatic_file, rep.custom_file) {
                (true, true) => "auto+cust",
                (true, false) => "auto",
                (false, true) => "cust",
                (false, false) => "none",
            };
            with_file += (file != "none") as usize;
            with_items += (!list.is_empty()) as usize;
            let shell = match (rep.recent_answered, rep.frequent_answered, rep.out_of_time) {
                (_, _, true) => "timeout",
                (true, true, _) => "R+F",
                (true, false, _) => "R",
                (false, true, _) => "F",
                _ => "-",
            };
            println!(
                "{label:<12} {file:<9} {:>5} {:>6} {:>6} {:>6} {:>7} {:>9} {shell:<8} {:>9.1?}",
                list.tasks.len(),
                list.custom.len(),
                list.pinned.len(),
                list.recent.len(),
                list.frequent.len(),
                format!("{:?}", list.which),
                took
            );
        }
        println!("distinct apps: {total}; with a jump-list file: {with_file}; with at least one item: {with_items}; slowest read: {slowest:.1?}");
        let snap_after = (snapshot(&auto_dir), snapshot(&custom_dir));
        let (da, dc) = (differences(&snap_before.0, &snap_after.0, &asked), differences(&snap_before.1, &snap_after.1, &asked));
        println!("files that differ between the start and the end of part 3 (size or last-write time), as (files of the apps asked about, all other files):");
        for (i, what) in ["changed", "added", "removed"].iter().enumerate() {
            println!("  {what:<8} automatic {:?}   custom {:?}", da[i], dc[i]);
        }
        println!("  (a reader that wrote would show in the first number alone; the second number is what Explorer and apps wrote meanwhile)");
    }

    let after = (count_files(&auto_dir), count_files(&custom_dir));
    println!("\nfiles after: {} automatic, {} custom ({})", after.0, after.1, if after == before { "unchanged: the probe wrote nothing" } else { "CHANGED while the probe ran: Explorer or an app wrote one (the probe writes nothing)" });
}
