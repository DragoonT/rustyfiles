//! The Share buttons (the file list's menu and toolbar, DirectLook): Windows' share sheet over the window they were pressed in
//! (`rf_win::share`, issue 48). The items are looked up on a worker; the sheet is shown from a task of the window's thread, which
//! runs outside the app's own work (Windows pumps messages while it shows the sheet).

use std::path::PathBuf;

use gpui_kit::App;

use crate::t;

/// Opens the share sheet for `paths` over the window `hwnd`.
pub fn open(hwnd: isize, paths: Vec<PathBuf>, cx: &mut App) {
    if paths.is_empty() || hwnd == 0 {
        return;
    }
    let title = match paths.as_slice() {
        [one] => one.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        many => format!("{} {}", many.len(), t!("öğe")),
    };
    let ready = cx.background_executor().spawn(async move { rf_win::share::prepare(&paths) });
    cx.spawn(async move |_| match ready.await {
        Ok(items) => {
            if let Err(e) = rf_win::share::show(hwnd, items, &title) {
                log::warn!("share: {e:#}");
            }
        }
        Err(e) => log::warn!("share: {e:#}"),
    })
    .detach();
}
