//! Directory change notifications (ReadDirectoryChangesW via `notify`), debounced.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Keeps the watch alive; drop to stop.
pub struct DirWatch {
    _w: RecommendedWatcher,
}

/// Calls `on_change` (from a background thread) once changes in `dir` pause for `debounce`, and
/// during a burst that never pauses (files renamed in the background without end) at least every
/// `debounce` × 5, so the view never falls behind for long.
pub fn watch_dir(dir: &Path, debounce: Duration, on_change: impl Fn() + Send + Sync + 'static) -> anyhow::Result<DirWatch> {
    let (tx, rx) = crossbeam_channel::unbounded::<()>();
    let mut w = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if !matches!(ev.kind, notify::EventKind::Access(_)) {
                let _ = tx.send(());
            }
        }
    })?;
    // notify opens the directory without the `\\?\` prefix: past MAX_PATH that fails and the
    // view would silently stop updating. Give it the long form there.
    let target = if dir.as_os_str().len() >= 240 {
        use std::os::windows::ffi::OsStringExt;
        let mut long = crate::wide::long_path(dir);
        long.pop(); // NUL
        std::path::PathBuf::from(std::ffi::OsString::from_wide(&long))
    } else {
        dir.to_path_buf()
    };
    w.watch(&target, RecursiveMode::NonRecursive)?;
    let on_change = Arc::new(on_change);
    let max_wait = debounce * 5;
    std::thread::Builder::new().name("rf-watch".into()).spawn(move || {
        while rx.recv().is_ok() {
            // swallow the burst, but not for longer than max_wait
            let burst = std::time::Instant::now();
            loop {
                let left = max_wait.saturating_sub(burst.elapsed());
                if left.is_zero() {
                    break;
                }
                match rx.recv_timeout(debounce.min(left)) {
                    Ok(()) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => break,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                }
            }
            on_change();
        }
    })?;
    Ok(DirWatch { _w: w })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    #[test]
    fn a_burst_without_pause_still_reports() {
        let dir = std::env::temp_dir().join(format!("rf-watch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let c2 = calls.clone();
        let _w = watch_dir(&dir, Duration::from_millis(150), move || {
            c2.fetch_add(1, Ordering::Relaxed);
        })
        .unwrap();
        // a change every 30 ms for 2 s: never 150 ms of quiet
        let start = Instant::now();
        let mut i = 0;
        while start.elapsed() < Duration::from_secs(2) {
            std::fs::write(dir.join(format!("f{}.txt", i % 50)), format!("{i}")).unwrap();
            i += 1;
            std::thread::sleep(Duration::from_millis(30));
        }
        let during = calls.load(Ordering::Relaxed);
        assert!(during >= 2, "reported {during} times during a 2 s burst");
        // and once more after it settles
        std::thread::sleep(Duration::from_millis(400));
        assert!(calls.load(Ordering::Relaxed) > during);
        drop(_w);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
