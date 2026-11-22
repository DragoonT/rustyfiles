//! Settings > Appearance > "App icon": the user's icon on rustyfiles' own shortcuts (the work is `rf_win::app_icon`).

use gpui_kit::*;

use crate::t;

/// The Choose… button: a file dialog for an icon, then [`set`].
pub fn pick(window: &mut Window, cx: &mut App) {
    let _ = window;
    let (send, receive) = futures::channel::oneshot::channel();
    let (title, files) = (t!("Simge seçin"), t!("Simge dosyaları"));
    rf_win::sta::run(move |owner| {
        let _ = send.send(rf_win::dialogs::pick(owner, title, rf_win::dialogs::Pick::File { filters: &[(files, "*.ico;*.exe;*.dll")] }));
    });
    cx.spawn(async move |cx| {
        if let Ok(Some(path)) = receive.await {
            let _ = cx.update(|cx| set(Some(path.to_string_lossy().into_owned()), cx));
        }
    })
    .detach();
}

/// Keeps `icon` (`None`: the program's own) and writes it into the shortcuts.
pub fn set(icon: Option<String>, cx: &mut App) {
    let wanted = icon.clone();
    crate::settings::update(cx, move |s| s.app_icon = wanted);
    write(icon);
}

/// At start: the chosen icon goes back on shortcuts that an update wrote again (nothing when none is chosen).
pub fn reapply(icon: Option<String>) {
    if icon.is_some() {
        write(icon);
    }
}

fn write(icon: Option<String>) {
    std::thread::spawn(move || {
        rf_win::shell::ensure_com();
        let n = rf_win::app_icon::apply(icon.as_deref());
        if n > 0 {
            log::info!("app icon: {n} shortcut(s) changed");
        }
    });
}
