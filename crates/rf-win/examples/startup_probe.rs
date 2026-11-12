//! Windowless, read-only probe of the start-up list and the dry run of its plan (shell plan increment H1). Prints, for this machine:
//! the COUNTS per source (enabled / disabled / setting of unknown layout / program missing / already running), the plan a run would
//! make, with every entry named by its source and number only ("HKCU Run #3: exists, enabled, already running -> skip"), so that the
//! counts can be held against Task Manager's Startup tab without this output naming anybody's programs.
//!
//! Nothing is started and nothing is deleted or written: the registry is opened for reading only, the Startup folders are listed,
//! `.lnk` files are read on the shell thread, the process list is a snapshot. No window is created.
//!
//!   cargo run -p rf-win --example startup_probe
//!   cargo run -p rf-win --example startup_probe -- --names   (the plan WITH the entries' names: for your own eyes only)

use rf_win::startup::collect::RegistryRead;
use rf_win::startup::model::{Hive, Presence, Running, ShortcutState, View};
use rf_win::startup::plan;
use rf_win::startup::system::SystemRegistry;
use rf_win::startup::{Names, Source, counts_by_source, format_plan, format_summary};

fn main() {
    let names = if std::env::args().any(|a| a == "--names") { Names::Show } else { Names::Hide };
    let t = std::time::Instant::now();
    let entries = rf_win::startup::list();
    let took = t.elapsed();
    println!("== START-UP LIST (read in {took:?}; safe mode: {})", rf_win::startup::is_safe_mode());
    println!("{:<26} {:>5} {:>7} {:>8} {:>8} {:>7} {:>8}", "source", "total", "enabled", "disabled", "unknown", "missing", "running");
    let counts = counts_by_source(&entries);
    for (s, c) in &counts {
        println!("{:<26} {:>5} {:>7} {:>8} {:>8} {:>7} {:>8}", s.label(), c.total, c.enabled, c.disabled, c.approval_unknown, c.missing_program, c.already_running);
    }
    let total: usize = counts.iter().map(|(_, c)| c.total).sum();
    println!("{:<26} {:>5}", "all sources", total);

    // what Task Manager's Startup tab shows: Run and the Startup folders (no policy keys, no RunOnce, and it also lists packaged apps' tasks)
    let tm = |s: &Source| matches!(s, Source::RunMachine | Source::RunMachine32 | Source::RunUser | Source::StartupFolderUser | Source::StartupFolderCommon);
    let (mut n, mut on, mut off) = (0, 0, 0);
    for (s, c) in counts.iter().filter(|(s, _)| tm(s)) {
        let _ = s;
        n += c.total;
        on += c.enabled;
        off += c.disabled;
    }
    println!("\nTask Manager comparison (Run keys + Startup folders only; packaged apps' start-up tasks are not listed by this module): {n} entries, {on} enabled, {off} disabled");

    println!("\n== DIAGNOSTICS");
    let reg = SystemRegistry;
    if let Some((hive, _, key)) = Source::RunUser.registry_key() {
        let native = reg.values(hive, View::Native, key).map(|v| v.len());
        let wow = reg.values(hive, View::Wow32, key).map(|v| v.len());
        println!("  HKCU Run values: native view {native:?}, 32-bit view {wow:?} (equal = the same key, so only the native view is listed)");
    }
    println!("  HKCU Policies Run exists: {}", reg.values(Hive::CurrentUser, View::Native, Source::PolicyRunUser.registry_key().map_or("", |k| k.2)).is_some());
    println!("  shortcuts that could not be read: {}", entries.iter().filter(|e| e.shortcut == ShortcutState::Unreadable).count());
    println!("  programs whose existence is unknown: {}", entries.iter().filter(|e| e.presence == Presence::Unknown).count());
    println!("  entries that name no program: {}", entries.iter().filter(|e| e.file_path.is_none() && e.program.is_empty()).count());
    println!("  entries whose program has a same-named process with an unreadable path (counted as running by the plan): {}", entries.iter().filter(|e| e.running == Running::Maybe).count());
    println!("  shortcuts that ask to run as administrator (skipped by the plan): {}", entries.iter().filter(|e| e.run_as_admin).count());

    println!("\n== PLAN (dry run: nothing is started, nothing is deleted)");
    let opts = rf_win::startup::session_options();
    let p = plan::build(&entries, &opts);
    println!("  {}", format_summary(&p.summary()));
    print!("{}", format_plan(&entries, &p, names));
}
