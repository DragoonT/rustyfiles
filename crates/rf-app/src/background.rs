//! Running in the background (tray icon after the last window closes) and notifications for
//! long operations — Files' "leave app running in the background" and its toasts.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui_kit::*;

use crate::status_center::{OpPhase, Ops};
use crate::t;

/// Set while "Quit" from the tray closes the windows (so closing them really exits).
static QUITTING: AtomicBool = AtomicBool::new(false);
static TRAY_ON: AtomicBool = AtomicBool::new(false);

/// Operations shorter than this never notify.
const NOTIFY_AFTER: Duration = Duration::from_secs(10);

pub fn quitting() -> bool {
    QUITTING.load(Ordering::Relaxed)
}

fn labels() -> rf_win::tray::Labels {
    rf_win::tray::Labels {
        tooltip: "rustyfiles".into(),
        open: t!("rustyfiles'ı aç").into(),
        new_window: t!("Yeni pencere").into(),
        settings: t!("Ayarlar").into(),
        quit: t!("Çıkış").into(),
    }
}

/// Bu süreç DirectLook: rustyfiles'ın bildirim alanı simgesi gösterilmez (DirectLook'un kendisi var).
pub static DIRECTLOOK_PROCESS: AtomicBool = AtomicBool::new(false);

/// This process is the dock (`rustyfiles --dock`): a side process like DirectLook, so it owns no tray icon, no name index and no
/// hotkey, and does not start or stop the dock itself.
pub static DOCK_PROCESS: AtomicBool = AtomicBool::new(false);

/// A process that only helps the main rustyfiles (DirectLook, the dock) and must leave the machine-wide things to it.
pub fn side_process() -> bool {
    DIRECTLOOK_PROCESS.load(Ordering::Relaxed) || DOCK_PROCESS.load(Ordering::Relaxed)
}

/// Settings changed: Explorer windows that other programs open are taken over while the setting is on (issue 37). Only the file
/// manager's own process watches (not the Dock's, DirectLook's or an Open / Save window's); a window taken over is shown by
/// starting this program with its place, which a running copy receives.
pub fn sync_explorer_catch(on: bool) {
    if side_process() || std::env::args().nth(1).as_deref() == Some("--pick") {
        return;
    }
    rf_win::explorer_catch::set(on, |c| {
        if let Ok(exe) = std::env::current_exe() {
            if let Err(e) = std::process::Command::new(exe).args(rf_win::explorer_catch::args_for(&c)).spawn() {
                log::warn!("explorer catch: {e}");
            }
        }
    });
}

/// Settings changed: the tray icon is there exactly while background mode is on.
pub fn sync_tray(on: bool) {
    if side_process() {
        return;
    }
    if TRAY_ON.swap(on, Ordering::Relaxed) == on {
        return;
    }
    if on {
        rf_win::tray::show(labels());
    } else {
        rf_win::tray::hide();
    }
    // One icon in the notification area: while ours is there DirectLook leaves its own out. That ours IS there it sees from this
    // sign, not from the setting alone: with background mode on and rustyfiles quit (or never started) there was no icon at all,
    // DirectLook's settings and its Quit out of reach. The sign goes with this process however it ends.
    thread_local! {
        static HERE: std::cell::RefCell<Option<rf_win::single_instance::Presence>> = const { std::cell::RefCell::new(None) };
    }
    HERE.with(|h| *h.borrow_mut() = on.then(|| rf_win::single_instance::announce(&tray_sign())).flatten());
    if on {
        // (a DirectLook that runs already shows its own: it looks again. Taking ours away needs no word, it waits on the sign.)
        std::thread::spawn(|| {
            let _ = rf_win::single_instance::send_to(&crate::look::ipc::app_name(), &crate::look::ipc::Msg::Reload.encode());
        });
    }
}

/// The name of the sign "rustyfiles has its icon in the notification area" (see [`sync_tray`]).
pub fn tray_sign() -> String {
    format!("{}-tray", rf_win::single_instance::instance_name())
}

/// Routes tray clicks to the app (call once at startup).
pub fn init(cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<rf_win::tray::TrayEvent>();
    rf_win::tray::set_handler(move |e| {
        let _ = tx.unbounded_send(e);
    });
    cx.spawn(async move |cx| {
        use futures::StreamExt;
        while let Some(e) = rx.next().await {
            cx.update(|cx| on_tray(e, cx));
        }
    })
    .detach();
}

fn on_tray(e: rf_win::tray::TrayEvent, cx: &mut App) {
    use rf_win::tray::TrayEvent;
    match e {
        TrayEvent::Open => {
            // bring an open main window forward (not whichever window comes first: that can be a pop-up), or open one (restoring
            // the last session)
            let first = crate::workspace::main_windows(cx).into_iter().next();
            match first {
                Some(w) => {
                    let _ = w.update(cx, |_, window, _| window.activate_window());
                }
                None => crate::open_main_window(None, None, None, cx),
            }
        }
        TrayEvent::NewWindow => crate::open_main_window(Some(rf_core::Location::Home), None, None, cx),
        TrayEvent::Settings => {
            crate::workspace::show_settings(cx);
        }
        TrayEvent::Quit => quit_all(cx),
    }
}

/// "Çıkış" (the tray icon's, and the toolbar's "..." menu's, issue 410): the sessions are saved and every window closes, so the
/// process ends whatever keeps it running otherwise (background mode, the Dock).
pub fn quit_all(cx: &mut App) {
    QUITTING.store(true, Ordering::Relaxed);
    crate::workspace::save_all_sessions(cx);
    let windows = cx.windows();
    if !crate::workspace::any_main_window(cx) {
        crate::quit_when_idle(cx);
    }
    for w in windows {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
}

/// An operation ended: notify when it was long and rustyfiles is not in front.
pub fn op_finished(ops: &Entity<Ops>, id: u64, cx: &mut App) {
    if !cx.global::<crate::settings::Settings>().notify_long_ops {
        return;
    }
    let Some((title, phase, took)) = ops.read(cx).summary(id) else { return };
    if took < NOTIFY_AFTER || rf_win::window::app_in_front() {
        return;
    }
    let (head, body) = match phase {
        OpPhase::Done => (t!("İşlem tamamlandı"), title),
        OpPhase::Failed(e) => (t!("İşlem başarısız oldu"), format!("{title}\n{e}")),
        OpPhase::Cancelled | OpPhase::Running => return,
    };
    rf_win::tray::notify(head, &body);
}
