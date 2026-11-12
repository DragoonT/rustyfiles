//! Windowless, read-only probe of the dock's icons: for the apps pinned to the Windows taskbar and the apps running now, how many
//! icons come from which kind of source, and how many of them have fewer pixels of their own than the size the dock draws them at,
//! BEFORE this change (the shell asked first, a window's icon in the chain before a package's logo) and AFTER it (a package's own logo
//! files, a program's icon frame by frame, a window's icon last). Prints COUNTS only: no names, paths or titles.
//!
//! Nothing is written, no window is created, and the shell is not asked: a source that only the shell could answer is counted as
//! such ("shell") with its detail unknown. What a probe cannot see is how the shell scales a picture it hands back, so those are not
//! counted as small or big.
//!
//!   cargo run -p rf-win --example dock_icon_probe
//!   cargo run -p rf-win --example dock_icon_probe -- --units 1,1.5,2        (monitor scale x dock size, default 1,1.25,1.5,2)

use std::collections::BTreeMap;

use rf_win::dock::iconload::{self, Used};
use rf_win::dock::iconpick;
use rf_win::dock::magnify::MAX_PEAK;
use rf_win::dock::model::{self, DockApp, DockItem};
use rf_win::dock::pins::{self, RealDisk};
use rf_win::dock::{IconSource, identity};

/// What the earlier loading started from, as far as it can be told without the shell: the first source of the app's own chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Before {
    /// A package: the shell scaled whichever logo file it chose; how much detail it had is not visible from here.
    PackageViaShell,
    /// A program or library: the shell scaled a frame of its icon, so it had the detail of the largest frame at most.
    ProgramViaShell,
    /// A shortcut's icon location, as above.
    ResourceViaShell,
    /// A place or another file: the shell, detail unknown.
    OtherViaShell,
    /// The icon a window has set itself (nothing else was known of the item).
    WindowIcon,
}

fn is_program(p: &std::path::Path) -> bool {
    p.extension().is_some_and(|e| ["exe", "dll", "scr", "cpl", "ocx"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

fn before(app: &DockApp) -> (Before, Option<u32>) {
    let chain = app.icon_chain();
    match &chain[0] {
        IconSource::Packaged(_) => (Before::PackageViaShell, None),
        IconSource::File(p) if is_program(p) => (Before::ProgramViaShell, iconload::program_frames(p, 0).and_then(|f| f.iter().map(|f| f.side()).max())),
        IconSource::Resource { file, index } if is_program(file) => (Before::ResourceViaShell, iconload::program_frames(file, *index).and_then(|f| f.iter().map(|f| f.side()).max())),
        // (asked for 32 px: the plain ask of the earlier loading, which did not look at the size)
        IconSource::Window(_) => (Before::WindowIcon, iconload::fetch_without_shell(&chain[0], 32).map(|(_, side)| side)),
        _ => (Before::OtherViaShell, None),
    }
}

/// What the new loader starts from for an app, in its order; the shell when only the shell can answer.
fn after(app: &DockApp, need: u32) -> (Option<Used>, Option<u32>) {
    let chain = app.icon_chain();
    for i in iconload::rank(&chain) {
        if let Some((used, side)) = iconload::fetch_without_shell(&chain[i], need) {
            return (Some(used), Some(side));
        }
        if !matches!(chain[i], IconSource::Window(_)) {
            // the shell answers for it (the earlier loading did too)
            return (None, None);
        }
    }
    (None, None)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let units: Vec<f32> = args
        .iter()
        .position(|a| a == "--units")
        .and_then(|i| args.get(i + 1))
        .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
        .unwrap_or_else(|| vec![1.0, 1.25, 1.5, 2.0]);

    let pins_shown: Vec<_> = pins::inspect_pins(&RealDisk).iter().filter_map(|r| r.app.clone()).collect();
    let items = model::merge(&pins_shown, identity::running_groups(), 0);
    let apps: Vec<DockApp> = items.iter().filter_map(|i| if let DockItem::App(a) = i { Some(a.clone()) } else { None }).collect();
    let running = apps.iter().filter(|a| a.running()).count();
    println!("apps in the dock: {} ({} pinned, {} running, {} both); places: 2", apps.len(), apps.iter().filter(|a| a.pinned).count(), running, apps.iter().filter(|a| a.pinned && a.running()).count());
    let by_kind = |f: &dyn Fn(&IconSource) -> bool| apps.iter().filter(|a| f(&a.icon)).count();
    println!(
        "the app's own icon is: a package {}, a program file {}, a shortcut's icon location {}, a window only {}",
        by_kind(&|s| matches!(s, IconSource::Packaged(_))),
        by_kind(&|s| matches!(s, IconSource::File(_))),
        by_kind(&|s| matches!(s, IconSource::Resource { .. })),
        by_kind(&|s| matches!(s, IconSource::Window(_)))
    );

    // the package side: can the files be read at all
    let packaged: Vec<String> = apps.iter().flat_map(|a| a.icon_chain()).filter_map(|s| if let IconSource::Packaged(id) = s { Some(id) } else { None }).collect();
    let mut readable = 0;
    let (mut with_unplated, mut biggest_unplated_256) = (0, 0);
    for id in &packaged {
        if let Some(found) = iconload::package_assets(id) {
            readable += 1;
            let unplated: Vec<_> = found.files.iter().filter(|(_, v)| v.unplated).collect();
            with_unplated += (!unplated.is_empty()) as usize;
            biggest_unplated_256 += unplated.iter().any(|(_, v)| v.px >= 256) as usize;
        }
    }
    println!("packaged apps: {}; their manifest and logo folder could be read: {}; of those with unplated files: {} (up to 256 px: {})", packaged.len(), readable, with_unplated, biggest_unplated_256);

    // the program side: the largest frame of each program icon
    let mut frames_hist: BTreeMap<u32, usize> = BTreeMap::new();
    let mut unreadable_programs = 0;
    for a in &apps {
        let (file, index) = match &a.icon {
            IconSource::File(p) if is_program(p) => (p.clone(), 0),
            IconSource::Resource { file, index } if is_program(file) => (file.clone(), *index),
            _ => continue,
        };
        match iconload::program_frames(&file, index).and_then(|f| f.iter().map(|f| f.side()).max()) {
            Some(side) => *frames_hist.entry(side).or_default() += 1,
            None => unreadable_programs += 1,
        }
    }
    println!("programs: largest icon frame (px: count) {:?}, no icon group readable: {}", frames_hist, unreadable_programs);

    for &unit in &units {
        let (rest, need) = (iconpick::rest_px(unit), iconpick::peak_px(unit, MAX_PEAK));
        let sizes = iconpick::level_sizes(rest, need);
        let per_icon: usize = sizes.iter().map(|s| (s * s * 4) as usize).sum();
        println!("\n== unit {unit}: rest {rest} px, needed at the peak {need} px, kept sizes {sizes:?} = {} KB per icon ({} KB for {} icons + 2 places)", per_icon / 1024, per_icon * (apps.len() + 2) / 1024, apps.len());
        let (mut b_kinds, mut a_kinds): (BTreeMap<Before, usize>, BTreeMap<String, usize>) = (BTreeMap::new(), BTreeMap::new());
        let (mut b_small, mut b_known, mut a_small, mut a_known) = (0, 0, 0, 0);
        for app in &apps {
            let (kind, native) = before(app);
            *b_kinds.entry(kind).or_default() += 1;
            if let Some(n) = native {
                b_known += 1;
                b_small += (n < need) as usize;
            }
            let (used, native) = after(app, need);
            *a_kinds.entry(used.map_or("shell".to_string(), |u| format!("{u:?}"))).or_default() += 1;
            if let Some(n) = native {
                a_known += 1;
                a_small += (n < need) as usize;
            }
        }
        println!("  before, by what the first source was: {b_kinds:?}");
        println!("  before, detail known for {b_known}, of those smaller than {need} px: {b_small}");
        println!("  after,  by what the picture comes from: {a_kinds:?}");
        println!("  after,  detail known for {a_known}, of those smaller than {need} px: {a_small}");

        // what the loading costs: every app whose picture comes from our own routes (not the shell, which this probe does not start),
        // loaded one after the other on this thread, all kept sizes made. (A debug build of rf-win: a release build is several times faster.)
        let t = std::time::Instant::now();
        let (mut done, mut slowest) = (0, std::time::Duration::ZERO);
        for app in &apps {
            let chain = app.icon_chain();
            if after(app, need).0.is_none() {
                continue;
            }
            let one = std::time::Instant::now();
            if iconload::load_levels(&chain, &sizes).is_ok() {
                done += 1;
                slowest = slowest.max(one.elapsed());
            }
        }
        println!("  loading {done} icons (all sizes made): {:?} in all, {:?} for the slowest", t.elapsed(), slowest);
    }
}
