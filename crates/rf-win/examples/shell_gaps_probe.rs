//! Windowless, read-only probe of the "shell's gaps without Explorer" parts (docs/design/shell-gaps.md).
//!
//! `cargo run --offline -p rf-win --example shell_gaps_probe`
//!
//! It creates no window, sends no key, writes no registry value and no Windows setting. It reads: whether a shell window exists, the
//! minimized-windows metrics, and what each file operation would do with and without a shell. The no-shell paths of the file
//! operations are run FOR REAL with the calling thread told there is no shell window, on files this probe creates in a temp folder of its
//! own (and removes); the Recycle Bin is never touched. The activation manager is asked for an id that is no app (nothing starts).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use rf_win::engine_guard::{self, NoShellEngine, Op, Plan, with_no_shell};
use rf_win::fileops::{self, Tracker, Transfer};
use rf_win::shellhost::minimized::{MetricsApi, SystemMetrics};

fn main() {
    println!("shell window exists: {}", engine_guard::shell_present());
    println!("shell now: {:?}", rf_win::shellhost::appcmd::shell_now());

    let m = SystemMetrics.get();
    println!("minimized-windows metrics readable: {}; hide bit set (a shell, or something else, hides them): {:?}", m.is_some(), m.map(|m| m.hidden()));

    println!("-- what each operation does (shell present / no shell):");
    for op in [Op::Rename, Op::PermanentDelete, Op::RecycleDelete, Op::Copy, Op::Move, Op::RestoreFromBin, Op::DeleteFromBin] {
        println!("   {op:?}: {:?} / {:?}", engine_guard::plan(op, true), engine_guard::plan(op, false));
    }
    assert_eq!(engine_guard::plan(Op::RecycleDelete, false), Plan::Refuse);

    println!("-- the no-shell paths, run for real in a temp folder (the thread is told there is no shell window):");
    let dir: PathBuf = std::env::temp_dir().join(format!("rf-shell-gaps-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("dst")).unwrap();
    std::fs::write(dir.join("a.txt"), b"a").unwrap();
    std::fs::create_dir_all(dir.join("tree/sub")).unwrap();
    std::fs::write(dir.join("tree/sub/f.txt"), b"f").unwrap();
    let tracker = Arc::new(Tracker::new(Box::new(|_, _, _| {}), Arc::new(AtomicBool::new(false))));
    with_no_shell(|| {
        let t = std::time::Instant::now();
        let r = fileops::rename(&dir.join("a.txt"), "b.txt");
        println!("   rename a.txt -> b.txt: {r:?} in {:?}; b.txt exists: {}", t.elapsed(), dir.join("b.txt").exists());

        let r = fileops::delete_tracked(&[dir.join("b.txt")], false, tracker.clone());
        let refused = r.as_ref().err().and_then(|e| e.downcast_ref::<NoShellEngine>()).map(|n| n.op);
        println!("   delete to the Recycle Bin: refused = {refused:?}; the file is still there: {}", dir.join("b.txt").exists());
        if let Err(e) = &r {
            println!("   message: {e}");
        }

        let r = fileops::transfer_tracked(Transfer::Copy, &[dir.join("b.txt")], &dir.join("dst"), tracker.clone());
        println!("   copy through the shell's engine: refused = {}", r.as_ref().err().and_then(|e| e.downcast_ref::<NoShellEngine>()).is_some());

        let t = std::time::Instant::now();
        let r = fileops::delete_tracked(&[dir.join("tree"), dir.join("b.txt")], true, tracker.clone());
        println!("   permanent delete (our own): {r:?} in {:?}; tree gone: {}, b.txt gone: {}", t.elapsed(), !dir.join("tree").exists(), !dir.join("b.txt").exists());
    });
    let _ = std::fs::remove_dir_all(&dir);

    println!("-- the activation manager asked for an id that is no app (nothing starts):");
    let t = std::time::Instant::now();
    let r = rf_win::activate::packaged("rustyfiles.probe.NoSuchPackage_0000000000000!App", "");
    println!("   {:?} in {:?}", r.map_err(|e| e.to_string()), t.elapsed());
}
