//! Directory change notifications (ReadDirectoryChangesW via `notify`), debounced.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Keeps the watch alive; drop to stop.
pub struct DirWatch {
    _w: RecommendedWatcher,
}

/// Calls `on_change` (from a background thread) at most once per `debounce` after changes in `dir`.
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
    std::thread::Builder::new().name("rf-watch".into()).spawn(move || {
        while rx.recv().is_ok() {
            // swallow the burst
            while rx.recv_timeout(debounce).is_ok() {}
            on_change();
        }
    })?;
    Ok(DirWatch { _w: w })
}
