//! Extracting archives to disk: rustyfiles' engine against NanaZip / 7-Zip (`7z x`) and Windows'
//! own `tar.exe` (libarchive, what Explorer uses for RAR and 7z), taking turns, best of `rounds`.
//! `cargo run --release -p rf-archive --example extract_vs -- <dest-dir> <rounds> <archive>...`

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

fn timed(f: impl FnOnce() -> bool) -> Option<f64> {
    let t = Instant::now();
    f().then(|| t.elapsed().as_secs_f64())
}

fn files_in(d: &Path) -> usize {
    std::fs::read_dir(d).map_or(0, |rd| {
        rd.flatten().map(|e| if e.file_type().is_ok_and(|t| t.is_dir()) { files_in(&e.path()) } else { 1 }).sum()
    })
}

fn main() {
    let mut a = std::env::args().skip(1);
    let dest = PathBuf::from(a.next().expect("dest dir"));
    let rounds: usize = a.next().expect("rounds").parse().unwrap();
    let archives: Vec<PathBuf> = a.map(PathBuf::from).collect();
    let seven = std::env::var("SEVENZIP").unwrap_or_else(|_| "7z".into());
    for arc in &archives {
        let name = arc.file_name().unwrap().to_string_lossy().into_owned();
        let mut best: [Option<f64>; 3] = [None; 3];
        let mut counts = [0usize; 3];
        for r in 0..rounds {
            // a different one goes first each round
            for k in 0..3 {
                let which = (k + r) % 3;
                let d = dest.join(format!("{which}"));
                let _ = std::fs::remove_dir_all(&d);
                std::fs::create_dir_all(&d).unwrap();
                let t = match which {
                    0 => timed(|| rf_archive::extract(arc, &d, &mut |_| {}, &|| false).is_ok()),
                    1 => timed(|| Command::new(&seven).args(["x", "-y", "-bso0", "-bsp0"]).arg(arc).arg(format!("-o{}", d.display())).status().is_ok_and(|s| s.success())),
                    _ => timed(|| Command::new(r"C:\Windows\System32\tar.exe").arg("-xf").arg(arc).arg("-C").arg(&d).status().is_ok_and(|s| s.success())),
                };
                counts[which] = files_in(&d);
                if let Some(t) = t {
                    best[which] = Some(best[which].map_or(t, |b: f64| b.min(t)));
                }
                let _ = std::fs::remove_dir_all(&d);
            }
        }
        let s = |v: Option<f64>| v.map_or("failed".to_string(), |v| format!("{v:.2} s"));
        println!("{name}: rustyfiles {} ({} files) | 7z {} ({}) | Windows tar {} ({})", s(best[0]), counts[0], s(best[1]), counts[1], s(best[2]), counts[2]);
    }
}
