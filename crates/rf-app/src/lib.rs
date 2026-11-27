//! rustyfiles ve DirectLook'un ortak kodu; iki program da bu kütüphaneyi kullanır.

mod actions;
mod aliases;
mod app_icon;
mod background;
mod cut_marks;
pub mod bench;
mod fast_start;
mod bench_view;
mod donate;
mod ferry;
mod addons;
mod addons_page;
mod hyperdownload;
mod hd_history;
mod downloads_view;
mod archive_view;
mod bulk_rename;
mod command_palette;
mod conflict_dialog;
mod ctx_menu;
mod disk_usage;
pub mod dock;
mod desktop_page;
mod dock_page;
mod fluent_menu;
mod flyout;
mod glass;
mod glass_watch;
mod liquid;
mod layout_panel;
mod git_ui;
mod prompt;
mod prop_chooser;
mod hash_panel;
mod i18n;
mod i18n_en;
mod folder_history;
mod folder_view;
mod icons;
mod icon_theme;
mod keymap_editor;
mod last_window;
mod logfile;
mod layouts;
pub mod look;
mod image_cache;
mod installer;
mod open_rules_page;
mod ops;
mod organizer;
mod picker;
mod nav_touch;
mod ui_stall;
mod places;
mod settings;
mod settings_merge;
mod shellless;
pub mod shell;
mod snip;
mod shell_view;
mod select_box;
mod settings_view;
mod share;
mod taskbar_preview;
mod update_splash;
mod shelf_view;
mod status_center;
mod transfer_view;
mod tags;
mod undo;
mod preview;
mod preview_trace;
mod program_launch;
mod properties_view;
mod quick_commands;
mod quick_search;
mod quicklook;
mod run_names;
mod settings_rows;
mod search_kinds;
mod search_pipe;
mod hotkey_field;
mod recycle_settings_view;
mod recycle_text;
mod recycle_view;
mod rtl;
pub(crate) mod toolbar_layout;
mod theme;
mod themes;
mod thumb_cache;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use rf_core::Location;

use crate::workspace::Workspace;

/// Restores the window on its original display, even when displays use different scales.
fn restored_bounds(cx: &App) -> Option<(WindowBounds, Option<DisplayId>)> {
    let prefs = cx.global::<settings::Settings>();
    let values @ [x, y, w, h, max] = prefs.window?;
    if !values.iter().all(|v| v.is_finite()) || w < 500. || h < 350. {
        return None;
    }
    let mut b = Bounds::new(point(px(x), px(y)), size(px(w), px(h)));
    let display = if let Some((key, [dx, dy])) = &prefs.window_display {
        let saved = cx.displays().into_iter().find(|d| d.uuid().is_ok_and(|id| id.to_string() == *key));
        if let Some(d) = saved.as_ref().cloned().or_else(|| cx.primary_display()) {
            let origin = saved.filter(|_| dx.is_finite() && dy.is_finite())
                .map(|d| d.bounds().origin + point(px(*dx), px(*dy)));
            b = fit_window_bounds(b.size, origin, d.visible_bounds());
            Some(d.id())
        } else {
            None
        }
    } else {
        b = on_primary(b.size, cx);
        None
    };
    Some((if max > 0.5 { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) }, display))
}

/// A window of `want` size in the middle of the main display's free area, made smaller when the display is smaller.
///
/// WHY (issue 176: "the RustyFiles window can only be resized from the top"): the first window was 1280 x 800 whatever the screen,
/// so on a 1366 x 768 screen (or a larger one at a larger scale) its edges lay beyond the screen and under the taskbar, where they
/// cannot be grabbed.
fn on_primary(want: Size<Pixels>, cx: &App) -> Bounds<Pixels> {
    match cx.primary_display() {
        Some(d) => fit_window_bounds(want, None, d.visible_bounds()),
        None => Bounds::centered(None, want, cx),
    }
}

fn fit_window_bounds(size: Size<Pixels>, origin: Option<Point<Pixels>>, area: Bounds<Pixels>) -> Bounds<Pixels> {
    let size = gpui_kit::size(size.width.min(area.size.width), size.height.min(area.size.height));
    let center = area.center() - point(size.width / 2., size.height / 2.);
    let origin = origin.filter(|&p| area.contains(&Bounds::new(p, size).center())).unwrap_or(center);
    Bounds::new(origin, size)
}

#[cfg(test)]
mod placement_tests {
    use super::fit_window_bounds;
    use gpui_kit::{Bounds, point, px, size};

    #[test]
    fn restores_negative_coordinates_and_centers_on_the_fallback_screen() {
        let secondary = Bounds::new(point(px(-1920.), px(0.)), size(px(1920.), px(1040.)));
        let original = Bounds::new(point(px(-1750.), px(40.)), size(px(1000.), px(700.)));
        assert_eq!(fit_window_bounds(original.size, Some(original.origin), secondary), original);
        let primary = Bounds::new(point(px(0.), px(0.)), size(px(1280.), px(720.)));
        let centered = fit_window_bounds(original.size, None, primary);
        assert_eq!(centered.center(), primary.center());
        assert_eq!(centered.size, original.size);
    }
}

/// Panics are written to %APPDATA%\rustyfiles\crash.log (the app has no console in release).
pub(crate) fn install_panic_log() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        rf_win::handover::trace("panic (see crash.log)");
        if let Some(dir) = crate::settings::app_dir() {
            let _ = std::fs::create_dir_all(&dir);
            let bt = std::backtrace::Backtrace::force_capture();
            let line = format!(
                "[{}] {info}\n{bt}\n\n",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crash.log")) {
                let _ = f.write_all(line.as_bytes());
            }
        }
        // (the shell host's process also says it in the log, where the other exit reasons are)
        shell::exits::on_panic(info);
        prev(info);
    }));
}

/// Whether rustyfiles is registered as the default file manager (HKCU).
pub static DEFAULT_FM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// rustyfiles answers other apps' Open/Save dialogs (registry state, read off the UI thread).
pub static FILE_DIALOGS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// What [`dialog_registration`] does with the Open/Save dialog registration.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum DialogReg {
    /// Leave it on or off, but while it is on, point it at this build's copy (start-up, after an update).
    Follow,
    /// Turn it on (a fresh install, the setting).
    Enable,
}

/// Brings the Open/Save dialog registration in line with this build. The registered DLL is a copy of the one next to this exe,
/// outside the install folder (`%LocalAppData%\rustyfiles\dialog\<build>\`): apps keep it loaded, and an update can't replace an
/// install folder with a loaded DLL in it. Next to the copy, `rustyfiles-exe.txt` tells the DLL which rustyfiles to start.
///
/// The registration must never point at a missing DLL: every app's Open/Save dialog then fails and some apps crash (issue 82). So
/// the copy registered now is never deleted, all of it runs under a lock every rustyfiles process takes (two builds starting
/// together, as at an update, deleted each other's copy), and a registration of ours whose DLL is gone, with no copy to point it at
/// instead, is removed. Returns whether the registration changed.
pub(crate) fn dialog_registration(mode: DialogReg) -> anyhow::Result<bool> {
    use rf_win::dialog_reg as reg;
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA").ok_or_else(|| anyhow::anyhow!("no LOCALAPPDATA"))?).join("rustyfiles").join("dialog");
    let _ = std::fs::create_dir_all(&root);
    let _lock = FileLock::take(&root.join(".lock"));
    let dll = if mode == DialogReg::Enable || reg::any_ours(reg::CLASSES) { dialog_copy(&root) } else { None };
    let mut changed = match (mode, &dll) {
        (DialogReg::Enable, Some(dll)) => {
            reg::enable(dll, reg::CLASSES)?;
            true
        }
        (DialogReg::Enable, None) => anyhow::bail!("{} could not be copied from next to rustyfiles.exe", reg::DLL_NAME),
        (DialogReg::Follow, Some(dll)) => reg::point_to(dll, reg::CLASSES),
        (DialogReg::Follow, None) => false,
    };
    if reg::drop_dangling(reg::CLASSES) {
        log::warn!("file dialogs: the registered DLL is gone; Windows' own dialog is back");
        changed = true;
    }
    let mut keep: Vec<PathBuf> = reg::registered_paths(reg::CLASSES).iter().filter_map(|p| std::path::Path::new(p).parent().map(PathBuf::from)).collect();
    keep.extend(dll.as_deref().and_then(|d| std::path::Path::new(d).parent().map(PathBuf::from)));
    delete_old_copies(&root, &keep);
    Ok(changed)
}

/// Deletes the copies under `root` except those in `keep` (one registered now: another build may have just registered it). A copy
/// whose DLL an app has loaded stays whole, so the loaded DLL still finds rustyfiles-exe.txt.
fn delete_old_copies(root: &std::path::Path, keep: &[PathBuf]) {
    let same = |a: &std::path::Path, b: &std::path::Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if !p.is_dir() || keep.iter().any(|k| same(k, &p)) {
            continue;
        }
        let lib = p.join(rf_win::dialog_reg::DLL_NAME);
        if std::fs::remove_file(&lib).is_ok() || !lib.exists() {
            let _ = std::fs::remove_dir_all(&p);
        }
    }
}

#[cfg(test)]
mod dialog_copy_tests {
    use std::os::windows::fs::OpenOptionsExt as _;

    #[test]
    fn the_registered_copy_and_loaded_copies_survive_the_clean_up() {
        let root = std::env::temp_dir().join(format!("rf-dlgcopies-{}", std::process::id()));
        let make = |name: &str| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join(rf_win::dialog_reg::DLL_NAME), b"x").unwrap();
            std::fs::write(d.join("rustyfiles-exe.txt"), b"x").unwrap();
            d
        };
        let (registered, old, loaded) = (make("0.8.6-a"), make("0.8.5-b"), make("0.8.4-c"));
        std::fs::write(root.join(".lock"), b"").unwrap();
        // (an app holding the DLL: no delete sharing)
        let held = std::fs::OpenOptions::new().read(true).share_mode(1).open(loaded.join(rf_win::dialog_reg::DLL_NAME)).unwrap();
        let upper = std::path::PathBuf::from(registered.to_string_lossy().to_uppercase());
        super::delete_old_copies(&root, &[upper]);
        assert!(registered.join(rf_win::dialog_reg::DLL_NAME).exists(), "the registered copy was deleted");
        assert!(!old.exists());
        assert!(loaded.join(rf_win::dialog_reg::DLL_NAME).exists() && loaded.join("rustyfiles-exe.txt").exists());
        assert!(root.join(".lock").exists());
        drop(held);
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// This build's copy of the dialog DLL under `root` (made when missing), with `rustyfiles-exe.txt` beside it.
fn dialog_copy(root: &std::path::Path) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let src = exe.with_file_name(rf_win::dialog_reg::DLL_NAME);
    let meta = std::fs::metadata(&src).ok()?;
    let stamp = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let dir = root.join(format!("{}-{:x}-{:x}", env!("CARGO_PKG_VERSION"), meta.len(), stamp));
    let dst = dir.join(rf_win::dialog_reg::DLL_NAME);
    if !dst.exists() {
        std::fs::create_dir_all(&dir).ok()?;
        grant_app_packages(&dir);
        let part = dir.join("rustyfiles_dialog.dll.part");
        std::fs::copy(&src, &part).ok()?;
        std::fs::rename(&part, &dst).ok()?;
    }
    let _ = std::fs::write(dir.join("rustyfiles-exe.txt"), exe.display().to_string());
    Some(dst.display().to_string())
}

/// A lock across processes: a file held open with no sharing. Waits up to three seconds for it, then goes on without it (better
/// than never registering).
struct FileLock(#[allow(dead_code)] Option<std::fs::File>);

impl FileLock {
    fn take(path: &std::path::Path) -> FileLock {
        use std::os::windows::fs::OpenOptionsExt as _;
        for _ in 0..60 {
            if let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).write(true).share_mode(0).open(path) {
                return FileLock(Some(f));
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        FileLock(None)
    }
}

/// Packaged apps' file pickers run in PickerHost.exe, which can only load the registered DLL if
/// app packages may read it; LocalAppData gives them no access, and the failed load makes the
/// picker fail (Snipping Tool crashed on Save as). Read and execute for ALL APPLICATION PACKAGES
/// and ALL RESTRICTED APPLICATION PACKAGES, inherited by the files copied in after.
fn grant_app_packages(dir: &std::path::Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let _ = std::process::Command::new("icacls")
        .arg(dir)
        .args(["/grant", "*S-1-15-2-1:(OI)(CI)(RX)", "*S-1-15-2-2:(OI)(CI)(RX)"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(test)]
mod grant_tests {
    #[test]
    fn app_packages_can_read_the_dialog_folder() {
        let dir = std::env::temp_dir().join(format!("rf-grant-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        super::grant_app_packages(&dir);
        let file = dir.join("rustyfiles_dialog.dll");
        std::fs::write(&file, b"x").unwrap();
        // the file copied in after inherits the grant; SDDL (SIDs, not localized names):
        // AC is ALL APPLICATION PACKAGES, S-1-15-2-2 ALL RESTRICTED APPLICATION PACKAGES
        let saved = std::env::temp_dir().join(format!("rf-grant-{}.acl", std::process::id()));
        std::process::Command::new("icacls").arg(&file).arg("/save").arg(&saved).output().unwrap();
        let raw = std::fs::read(&saved).unwrap();
        let words: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        let sddl = String::from_utf16_lossy(&words);
        let _ = std::fs::remove_file(&saved);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(sddl.contains("(A;ID;0x1200a9;;;AC)"), "{sddl}");
        assert!(sddl.contains("(A;ID;0x1200a9;;;S-1-15-2-2)"), "{sddl}");
    }
}

/// Opens a rustyfiles window. `start = None` restores the session (first window) or opens Home.
pub fn open_main_window(
    start: Option<Location>,
    ipc: Option<futures::channel::mpsc::UnboundedReceiver<String>>,
    bounds: Option<WindowBounds>,
    cx: &mut App,
) {
    open_main_window_made(start, ipc, bounds, cx);
}

/// [`open_main_window`], saying whether the window was made (a second launch waits for that answer: see `rf_win::handover`).
pub fn open_main_window_made(
    start: Option<Location>,
    ipc: Option<futures::channel::mpsc::UnboundedReceiver<String>>,
    bounds: Option<WindowBounds>,
    cx: &mut App,
) -> bool {
    open_main_window_handle(start, ipc, bounds, cx).is_some()
}

/// A new main window for a key the running program caught itself (Win+E while rustyfiles is the default file manager), brought to
/// the front.
///
/// A window opened like that is opened by a program that is NOT in front (it may be minimized, or behind another program), and
/// Windows then shows the window without giving it the keyboard: it opened behind what was in front, and Win+E seemed to do
/// nothing (issue 367: "when an instance is already open, pressing Win + E spawns an additional window without giving it focus").
/// A second process started by Explorer does not have this trouble: it hands its right to come forward to the first
/// (`rf_win::handover`). Here the window is taken to the front with the sequence the Dock's window switcher uses
/// (`rf_win::switcher::activate`): the plain call, then the thread-input route, and a flash of its taskbar button when Windows
/// still refuses. No key is sent.
pub fn open_main_window_in_front(start: Option<Location>, cx: &mut App) -> Option<rf_win::switcher::activate::Outcome> {
    let handle = open_main_window_handle(start, None, None, cx)?;
    let hwnd = handle.update(cx, |_, window, _| look::window::hwnd_of(window)).ok().filter(|h| *h != 0)?;
    Some(rf_win::switcher::activate::activate(&mut rf_win::switcher::activate::Win32Ops, hwnd))
}

fn open_main_window_handle(
    start: Option<Location>,
    ipc: Option<futures::channel::mpsc::UnboundedReceiver<String>>,
    bounds: Option<WindowBounds>,
    cx: &mut App,
) -> Option<AnyWindowHandle> {
    let (bounds, display_id) = match bounds {
        Some(bounds) => (bounds, None),
        None => {
            let restored = if workspace::any_main_window(cx) { None } else { restored_bounds(cx) };
            restored.unwrap_or_else(|| (WindowBounds::Windowed(on_primary(size(px(1280.), px(800.)), cx)), None))
        }
    };
    let options = WindowOptions {
        window_bounds: Some(bounds),
        display_id,
        window_min_size: Some(size(px(720.), px(420.))),
        window_background: crate::theme::window_material(WindowBackgroundAppearance::MicaBackdrop),
        ..TitleBar::window_options()
    };
    match gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| Workspace::new(start, ipc, window, cx))) {
        Ok((handle, _)) => Some(handle),
        Err(e) => {
            log::error!("open window: {e:#}");
            rf_win::handover::trace("open window: failed");
            None
        }
    }
}

/// Quits now, or once running copy/move/delete operations finish (closing the window must not
/// cut a copy in half). A window opened meanwhile (second launch) cancels the quit.
pub(crate) fn quit_when_idle(cx: &mut App) {
    // (an app or file the quick search is still starting counts as running work)
    let running = |cx: &App| cx.try_global::<status_center::OpsGlobal>().map_or(0, |g| g.0.read(cx).running()) + quick_search::launching();
    if running(cx) == 0 {
        save_and_quit(cx);
        return;
    }
    cx.spawn(async move |cx| loop {
        cx.background_executor().timer(std::time::Duration::from_millis(500)).await;
        let done = cx.update(|cx| {
            if workspace::any_main_window(cx) {
                return Some(false);
            }
            (running(cx) == 0).then_some(true)
        });
        match done {
            Some(true) => {
                cx.update(save_and_quit);
                break;
            }
            Some(false) => break,
            None => {}
        }
    })
    .detach();
}

/// The session/placement save on window close runs on a helper thread; write once more here,
/// synchronously, so quitting cannot cut it off (a small file, only at exit).
fn save_and_quit(cx: &mut App) {
    // The quit and a second launch's request meet here (GitHub issue 15): a request that was taken and is not acted on yet stops the
    // quit (it opens a window, and that keeps the process); once the quit goes ahead, requests are refused with `leaving` and the
    // second launch shows its folder itself. Without this a folder opened while the app was quitting was forwarded to a process
    // that closed the window it opened, or that never looked at it.
    if !rf_win::handover::GATE.try_leave() {
        rf_win::handover::trace("quit: held back, a request is in flight");
        return;
    }
    rf_win::handover::trace("quit: leaving, requests are refused from now on");
    cx.global::<settings::Settings>().clone().save();
    // the search index keeps its place in the change journal (or its changes): the next start
    // then catches up instead of walking the drives; bounded, quitting must not hang on it
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        rf_win::indexer::stop();
        let _ = tx.send(());
    });
    // (kept on the UI thread and before the process ends on purpose: the index must be at rest when the next process opens it, and
    // a launch meanwhile is told `leaving` and waits for the mutex; the trace gives the real time of this wait)
    let stopped = rx.recv_timeout(std::time::Duration::from_secs(2)).is_ok();
    rf_win::handover::trace(&format!("quit: index stopped {}, ending", if stopped { "in time" } else { "late (2 s limit)" }));
    cx.quit();
}

/// The variable a restarted rustyfiles is started with: the process id of the one that is ending (see [`restart`]).
const AFTER_EXIT_ENV: &str = "RUSTYFILES_AFTER_EXIT";

/// Restarts rustyfiles: DirectLook and the dock are asked to stop, a new rustyfiles is started that waits for this one to end, and
/// this one quits the usual way (settings and session written). The new one opens its last session and brings the dock and
/// DirectLook back as the settings say. WHY (the owner, 2026-11-17, after a morning where the dock came up empty: "bir de bir
/// kısayol ekleyelim. rustyfiles'ı restart etsin"): there was no way to start everything afresh short of ending the processes by
/// hand. Ctrl+Alt+Shift+R and the command palette.
pub(crate) fn restart(cx: &mut App) {
    let Ok(me) = std::env::current_exe() else { return };
    let pid = std::process::id().to_string();
    std::thread::spawn(move || {
        rf_win::single_instance::send_to(&look::ipc::app_name(), &look::ipc::Msg::Quit.encode());
        dock::control::stop_for_update();
        // (no folder: the new process opens the last session; its working directory is never the installation folder)
        let dir = std::env::temp_dir();
        if let Err(e) = std::process::Command::new(&me).env(AFTER_EXIT_ENV, &pid).current_dir(dir).spawn() {
            log::warn!("restart: the new rustyfiles did not start: {e}");
        }
    });
    // (give the stops their moment, then quit; the new process waits for this one to be gone)
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_millis(600)).await;
        cx.update(|cx| {
            rf_win::handover::GATE.leave_now();
            workspace::save_all_sessions(cx);
            save_and_quit(cx)
        });
    })
    .detach();
}

/// A restarted rustyfiles: waits (up to ten seconds) for the one that started it to end, so that it does not hand its start to
/// that one as a second launch does.
fn wait_for_the_old_one() {
    let Some(pid) = std::env::var(AFTER_EXIT_ENV).ok().and_then(|p| p.trim().parse::<u32>().ok()) else { return };
    // (not inherited by what this process starts)
    unsafe { std::env::remove_var(AFTER_EXIT_ENV) };
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
    // SAFETY: a handle to wait on, closed right after
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, 10_000);
            let _ = windows::Win32::Foundation::CloseHandle(h);
        }
    }
}

/// `Path::is_dir` with a time limit (it can block for the SMB timeout on an offline share).
pub(crate) fn is_dir_bounded(p: PathBuf, limit: std::time::Duration) -> Option<bool> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(p.is_dir());
    });
    rx.recv_timeout(limit).ok()
}

/// A shell namespace argument resolved to a folder we list ourselves that is not on the file
/// system (see `rf_win::shellfolder::browsable`), within `limit`.
fn shell_place_bounded(arg: &str, limit: std::time::Duration) -> Option<rf_win::shellfolder::Place> {
    let (tx, rx) = std::sync::mpsc::channel();
    let arg = arg.to_string();
    std::thread::spawn(move || {
        let _ = tx.send(rf_win::shellfolder::browsable(&arg));
    });
    rx.recv_timeout(limit).ok().flatten().filter(|p| p.fs_path.is_none())
}

/// rustyfiles'ın girişi.
/// Start-up timing (`RUSTYFILES_TRACE_START=1`, logged at info): milliseconds since the process
/// was created, at the steps before the first window.
pub(crate) fn start_mark(step: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *ON.get_or_init(|| std::env::var_os("RUSTYFILES_TRACE_START").is_some()) {
        eprintln!("start: {step} at {} ms", rf_win::process_age_ms());
    }
}

pub fn main() {
    start_mark("main");
    wait_for_the_old_one();
    // Not in the installation folder: the updater renames it, and Windows refuses while it is a working directory of this process
    // or of anything this process starts (`rf_win::ownfolder`). Before everything else, so that every child inherits the way out.
    rf_win::ownfolder::leave();
    // `--open-with-folder <file> <property> <1|0>`: a file opened in a packaged app with its folder, from a process that ends at once
    // (see rf_win::neighbours::launch)
    if std::env::args().nth(1).as_deref() == Some(rf_win::neighbours::ARG) {
        std::process::exit(rf_win::neighbours::helper_main());
    }
    // `--shell-probe`: what the shell host finds, windowless and read-only, before anything else is done (see shell::probe)
    if std::env::args().nth(1).as_deref() == Some(shell::probe::ARG) {
        std::process::exit(shell::probe::run());
    }
    // Velopack (installer / updater): first, it may run an install hook and exit
    installer::startup();
    // the id Velopack gives the Start menu / taskbar shortcuts: a pin survives updates
    rf_win::window::set_app_id(installer::APP_ID);
    install_panic_log();
    rf_win::shell::suppress_critical_error_dialogs();
    // before any shell icon/thumbnail work: see keep_gdiplus_alive (loader-lock deadlock)
    rf_win::thumbs::keep_gdiplus_alive();
    // Logs go to stderr and to rustyfiles.log in the settings folder; override the level with
    // RUST_LOG. The hybrid renderer (gpui_windows) logs its GPU/CPU switches at info level.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn,gpui_windows=info"))
        .target(env_logger::Target::Pipe(Box::new(logfile::Tee::new())))
        .format_timestamp_millis()
        .init();
    start_mark("logging");
    // a copy of this exe showing the update window while the installed one is replaced
    if std::env::args().nth(1).as_deref() == Some(update_splash::ARG) {
        update_splash::run();
        return;
    }

    // As default file manager we also receive shell namespaces (Control Panel = "::{…}") and
    // non-folders: hand those back to Explorer so nothing becomes unreachable.
    // The folder check is bounded: an offline network path must not keep the window from
    // appearing for the whole SMB timeout. Undecided after 2 s = treat as a (slow) folder.
    // `--settings=directlook`: DirectLook'un simge menüsünden "DirectLook ayarları"
    // `--settings=speed`: straight to Settings → Speed test
    // `--settings=dock`: straight to Settings → Dock
    let settings_arg = std::env::args().nth(1).and_then(|a| a.strip_prefix("--settings=").map(str::to_string));
    let open_look_settings = settings_arg.is_some();
    // `--pick <pipe>`: an app's Open/Save dialog (rustyfiles_dialog.dll) asks for a file
    let pick = (std::env::args().nth(1).as_deref() == Some("--pick")).then(|| std::env::args().nth(2)).flatten();
    // `--bar-preview`: our taskbar, as an ordinary window (see taskbar_preview)
    // `--desktop-preview`: our desktop, as an ordinary window (see shell::desktop); treated like the bar preview everywhere below
    let desktop_preview = std::env::args().nth(1).as_deref() == Some(shell::desktop::args::ARG);
    // `--switcher-preview`: our window switcher, shown once (see shell::switcher); also treated like the bar preview below
    let switcher_preview = std::env::args().nth(1).as_deref() == Some(shell::switcher::ARG);
    let bar_preview = desktop_preview || switcher_preview || std::env::args().nth(1).as_deref() == Some("--bar-preview");
    // `--quick-search`: the quick search window, as its hotkey opens it (a shortcut of the
    // user's own can run this)
    let quick_search = std::env::args().nth(1).as_deref() == Some(quick_search::LAUNCH_ARG);
    // `--hd-follow=<gid>`: started by HyperDownload's browser host: show that download (in the running window, or in a new one)
    let hd_follow = std::env::args().nth(1).and_then(|a| a.strip_prefix("--hd-follow=").map(str::to_string));
    // `--dock`: the dock (see dock/), a process of its own that lives beside the main rustyfiles. `--shell` is the same process under
    // its new name, the shell host (see shell/): the one with the desktop cover, the shell mode and the probe.
    let dock = shell::is_host_arg(std::env::args().nth(1).as_deref());
    // `--snip`: our own screen snip and nothing else (see snip/): no window of the app, no forwarding to a running copy
    let snip_only = snip::is_arg(std::env::args().nth(1).as_deref());
    // `--no-window`: the main app with no window, started by the dock so that its search has the file index (see search_pipe)
    let no_window = std::env::args().nth(1).as_deref() == Some(search_pipe::START_ARG);
    if bar_preview || dock {
        quick_search::disable_hotkey();
    }
    if dock {
        // before the settings are loaded: the dock owns no tray icon, no name index and no hotkey
        background::DOCK_PROCESS.store(true, std::sync::atomic::Ordering::Relaxed);
        // which mode it runs in: found by looking at the shell window (read-only); only logged for now
        shell::note_host_start(&std::env::args().skip(1).collect::<Vec<_>>());
    }
    let arg = std::env::args().nth(1).map(|a| a.trim().trim_matches('"').to_string()).filter(|a| !a.is_empty());
    let arg = arg.filter(|_| !open_look_settings && pick.is_none() && !bar_preview && !quick_search && !dock && !no_window && !snip_only);
    // jump list task "New window"
    let new_window = arg.as_deref() == Some(rf_win::jumplist::NEW_WINDOW_ARG);
    // `--downloads`: HyperDownload's window (a running rustyfiles opens it; a first start opens it beside its own window)
    let downloads_window = arg.as_deref() == Some(downloads_view::LAUNCH_ARG);
    // (`--hd-follow=` is not a folder either: it was handed to Explorer, which opened Documents, and the download was never shown)
    let arg = arg.filter(|_| !new_window && !downloads_window && hd_follow.is_none());
    // several folders on the command line (issue 125): a first one that is gone (moved, renamed) is passed over, not handed to Explorer
    // with its error, and the first one that is there opens instead; `first_taken` is how many arguments that used up
    let mut first_taken = 1;
    let arg = match arg {
        Some(a) if std::env::args().nth(2).is_some() && !a.starts_with("::") && !a.to_ascii_lowercase().starts_with("shell:") && !a.starts_with("/") && !a.starts_with('-') && !std::path::Path::new(&a).exists() => {
            let later = std::env::args()
                .enumerate()
                .skip(2)
                .map(|(i, x)| (i, x.trim().trim_matches('"').to_string()))
                .find(|(_, x)| !x.is_empty() && !x.starts_with("--") && is_dir_bounded(PathBuf::from(x), std::time::Duration::from_secs(2)) == Some(true));
            match later {
                Some((i, x)) => {
                    first_taken = i;
                    Some(x)
                }
                None => Some(a),
            }
        }
        other => other,
    };
    let mut start: Option<Location> = None;
    // Explorer's `/select,<file>`: its folder, with the file selected
    // (callers also write `/select, <file>`: the path then comes as the next argument)
    let all_args = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let mut select = {
        let lower = all_args.to_ascii_lowercase();
        (lower.starts_with("/select,") || lower.starts_with("-select,"))
            .then(|| all_args[8..].trim().trim_matches('"').to_string())
            // bounded like the folder check below: an offline share must not hold the window
            .filter(|file| {
                PathBuf::from(file)
                    .parent()
                    .is_some_and(|d| is_dir_bounded(d.to_path_buf(), std::time::Duration::from_secs(2)) != Some(false))
            })
    };
    let arg = if select.is_some() { None } else { arg };
    if let Some(file) = &select {
        let p = PathBuf::from(file);
        start = p.parent().map(|d| Location::Path(d.to_path_buf()));
        *workspace::START_SELECT.lock().unwrap() = p.file_name().map(|n| n.to_string_lossy().into_owned());
    }
    // desktop icons (This PC, Recycle Bin) arrive as shell namespaces: those we show ourselves
    if let Some(loc) = arg.as_deref().and_then(Location::from_shell_name) {
        start = Some(loc);
    } else if let Some(arg) = arg {
        let shell_namespace = arg.starts_with("::") || arg.to_ascii_lowercase().starts_with("shell:");
        // a namespace folder we can list ourselves (a phone, Network, a library…) opens here;
        // Control Panel and other applets (and anything that does not enumerate) stay with Explorer.
        // Bounded like the folder check: a device that hangs must not keep the window away.
        let shell_place = if shell_namespace { shell_place_bounded(&arg, std::time::Duration::from_secs(3)) } else { None };
        if let Some(place) = shell_place {
            start = Some(Location::Shell { parsing: place.parsing, name: place.name });
        } else {
            // a network server root (\\nas) isn't a "directory" to Rust, but we browse its shares
            // (an FTP address is browsed here too: it went to Explorer)
            let server = rf_win::network::server_root(std::path::Path::new(&arg)).is_some() || rf_win::ftp::is_ftp(std::path::Path::new(&arg));
            let is_dir = if shell_namespace {
                Some(false)
            } else if server {
                Some(true)
            } else {
                is_dir_bounded(PathBuf::from(&arg), std::time::Duration::from_secs(2))
            };
            // a browsable archive (DirectLook's "rustyfiles'ta aç") opens like a folder, here
            let archive = !shell_namespace && archive_view::is_browsable(std::path::Path::new(&arg));
            if is_dir == Some(false) && !archive {
                // Explorer opens folders through the verb we took over: what we just handed it may
                // come straight back. Never ping-pong; a path is then shown here (with its error).
                rf_win::handover::trace("main: not a folder for us (or a namespace we do not list): to Explorer unless it just came from there");
                if !rf_win::default_fm::forwarded_recently(&arg) {
                    rf_win::default_fm::note_forward(&arg);
                    rf_win::handover::trace("main: handed to Explorer, this process ends");
                    rf_win::explorer_catch::quiet_for(std::time::Duration::from_secs(5));
                    let _ = std::process::Command::new("explorer.exe").arg(&arg).spawn();
                    return;
                }
                if shell_namespace {
                    return;
                }
                // a file that came back (an archive type Windows browses but we don't, e.g. .cpio):
                // its folder, with the file selected
                let p = PathBuf::from(&arg);
                if let (Some(dir), Some(name)) = (p.parent().filter(|_| p.is_file()), p.file_name()) {
                    start = Some(Location::Path(dir.to_path_buf()));
                    *workspace::START_SELECT.lock().unwrap() = Some(name.to_string_lossy().into_owned());
                    select = Some(arg.clone());
                }
            }
            if start.is_none() {
                start = Some(Location::Path(PathBuf::from(arg)));
            }
        }
    }
    // Cache the registry state for the settings page (read once, off the hot path).
    // A picker started for another app's Open/Save (--pick) only reads: whichever rustyfiles.exe
    // the dialog DLL started (a test build too) must not re-point the user's registrations.
    // The dock is a side process that only reads: it neither re-points the registrations nor cleans the main app's temp copies.
    // (nor from the switcher preview, a try-out of one window: it re-points nothing in the registry, config directory or not)
    let registers = pick.is_none() && !dock && !switcher_preview && std::env::var_os("RUSTYFILES_CONFIG_DIR").is_none();
    std::thread::spawn(move || {
        // copies opened/copied out of archives days ago
        if !dock {
            crate::archive_view::clean_temp();
        }
        // default file manager whose registered exe vanished (moved / rebuilt elsewhere):
        // re-point it here, or Windows can't open any folder. Not from test instances.
        if registers {
            if let Ok(me) = std::env::current_exe() {
                if rf_win::default_fm::heal(&me.display().to_string(), rf_win::default_fm::CLASSES) {
                    log::warn!("default file manager re-registered to {}", me.display());
                }
                if rf_win::default_fm::add_archives(&me.display().to_string(), rf_win::default_fm::CLASSES) {
                    log::warn!("default file manager: archive registration updated");
                }
            }
        }
        // Open/Save dialogs: a registration of another build is pointed at this one's copy, one whose DLL is gone is removed
        // (not from tests)
        if registers {
            match dialog_registration(DialogReg::Follow) {
                Ok(true) => log::warn!("file dialogs: registration updated"),
                Ok(false) => {}
                Err(e) => log::warn!("file dialogs: {e:#}"),
            }
        }
        FILE_DIALOGS.store(rf_win::dialog_reg::is_enabled(rf_win::dialog_reg::CLASSES), std::sync::atomic::Ordering::Relaxed);
        DEFAULT_FM.store(
            rf_win::default_fm::is_enabled(rf_win::default_fm::CLASSES),
            std::sync::atomic::Ordering::Relaxed,
        )
    });

    // more folders on the command line (`rustyfiles.exe "C:\My Pictures" "D:\My Videos"`): each one a tab of the same window, after the
    // first (issue 125). Only plain folders that are there (one that is gone is passed over without a word), after a first argument that
    // was one too.
    let more_folders: Vec<String> = if matches!(start, Some(Location::Path(_))) && select.is_none() {
        std::env::args()
            .skip(first_taken + 1)
            .map(|a| a.trim().trim_matches('"').to_string())
            .filter(|a| !a.is_empty() && !a.starts_with("--") && is_dir_bounded(PathBuf::from(a), std::time::Duration::from_secs(2)) == Some(true))
            .collect()
    } else {
        Vec::new()
    };
    // "Show in folder" from other apps (SHOpenFolderAndSelectItems: Telegram, browsers) while we
    // are the default file manager: Windows opens the folder through our verb and names the
    // file afterwards, to a window it finds in its window list (see rf_win::shell_window). The
    // folder opens at once; the file, if one comes, is selected in it.
    const SHELL_SELECT_WAIT: std::time::Duration = std::time::Duration::from_secs(4);
    let shell_select = match &start {
        Some(Location::Path(dir)) if select.is_none() && std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_none() && std::env::var_os("RUSTYFILES_INSTANCE").is_none() && rf_win::default_fm::is_enabled(rf_win::default_fm::CLASSES) => {
            let (tx, rx) = std::sync::mpsc::channel();
            let dir = dir.clone();
            std::thread::spawn(move || {
                let _ = tx.send(rf_win::shell_window::wait_for_select(&dir, SHELL_SELECT_WAIT));
            });
            Some(rx)
        }
        _ => None,
    };
    let shell_selected = move |rx: std::sync::mpsc::Receiver<Option<PathBuf>>| {
        rx.recv_timeout(SHELL_SELECT_WAIT + std::time::Duration::from_secs(1)).ok().flatten().map(|f| format!("{}{}", workspace::SELECT_HERE_PREFIX, f.display()))
    };

    // Single instance: a second launch opens its folder as a tab in the running window.
    let message = match &start {
        _ if open_look_settings => look::ipc::ToApp::OpenSettings.encode(),
        _ if pick.is_some() => format!("{}{}", picker::PICK_PREFIX, pick.as_deref().unwrap_or_default()),
        _ if new_window => rf_win::jumplist::NEW_WINDOW_ARG.to_string(),
        _ if downloads_window => downloads_view::LAUNCH_ARG.to_string(),
        _ if quick_search => quick_search::LAUNCH_ARG.to_string(),
        _ if hd_follow.is_some() => format!("{}{}", hyperdownload::FOLLOW_PREFIX, hd_follow.as_deref().unwrap_or_default()),
        _ if no_window => search_pipe::START_MSG.to_string(),
        _ if select.is_some() => format!("{}{}", workspace::SELECT_PREFIX, select.as_deref().unwrap_or_default()),
        Some(Location::Path(p)) => p.to_string_lossy().into_owned(),
        Some(Location::RecycleBin) => "shell:RecycleBinFolder".to_string(),
        Some(Location::Shell { parsing, .. }) => format!("{}{parsing}", workspace::SHELL_PREFIX),
        // a search link goes on to the running window as a search link
        Some(Location::Search { root, query }) => {
            let enc = |s: &str| s.replace('%', "%25").replace('&', "%26");
            format!("search-ms:query={}&crumb=location:{}", enc(query), enc(&root.to_string_lossy()))
        }
        _ => String::new(),
    };
    // RUSTYFILES_MULTI_INSTANCE=1 skips forwarding (automated tests run several copies).
    let multi = std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_some() || bar_preview || dock || snip_only;
    // the dock is a single instance of its own group: a second `--dock` greets the first and ends; the guard lives to the end
    let dock_instance = if dock {
        match dock::acquire() {
            Some(held) => Some(held),
            None => {
                // (this used to end the process without a word)
                shell::exits::log_exit(shell::exits::Exit::AnotherHost, "");
                return;
            }
        }
    } else {
        None
    };
    let (_dock_guard, mut dock_messages) = match dock_instance {
        Some((guard, rx)) => (Some(guard), Some(rx)),
        None => (None, None),
    };
    rf_win::handover::trace(&format!("main: launch, request of {} bytes, multi {multi}", message.len()));
    let _instance = if multi {
        None
    } else {
        // (the hand-over waits for the running instance's answer, and shows the folder here when that one is leaving: see rf_win::handover)
        match rf_win::handover::acquire(&message) {
            rf_win::single_instance::Role::Forwarded => {
                // the folder is on its way to the running window; the file to select follows
                if let Some(msg) = shell_select.and_then(shell_selected) {
                    rf_win::handover::send_followup(&msg);
                }
                for folder in &more_folders {
                    rf_win::handover::send_followup(folder);
                }
                return;
            }
            rf_win::single_instance::Role::Primary(guard) => Some(guard),
        }
    };
    let primary_instance = _instance.is_some() && pick.is_none() && !quick_search && !bar_preview && !dock;
    let (ipc_tx, ipc_rx) = futures::channel::mpsc::unbounded::<String>();
    // second launches: each comes with the acknowledgement it waits for (the other senders of ipc_tx need none)
    let (launch_tx, launch_rx) = futures::channel::mpsc::unbounded::<(String, rf_win::handover::Ack)>();
    // (this process is the first: its own extra folders open as tabs once the window is there, like second launches do)
    // (this process is the first one and was started for a browser download: it is shown once the window is there)
    if let Some(gid) = &hd_follow {
        let _ = ipc_tx.unbounded_send(format!("{}{gid}", hyperdownload::FOLLOW_PREFIX));
    }
    for folder in more_folders {
        let _ = ipc_tx.unbounded_send(folder);
    }
    if downloads_window {
        let _ = ipc_tx.unbounded_send(downloads_view::LAUNCH_ARG.to_string());
    }
    if let Some(rx) = shell_select {
        let tx = ipc_tx.clone();
        std::thread::spawn(move || {
            if let Some(msg) = shell_selected(rx) {
                let _ = tx.unbounded_send(msg);
            }
        });
    }
    if !multi {
        // the dock's file search has a pipe of its own (an older main app never sees its requests as launch arguments)
        search_pipe::serve(ipc_tx.clone());
        rf_win::handover::serve(move |m, ack| {
            let _ = launch_tx.unbounded_send((m, ack));
        });
    }

    // a drag of files that live inside the source (a mail's attachment, a browser's image) is dropped as files (see rf_win::vfiles)
    gpui_windows::dnd_hint::set_virtual_reader(rf_win::vfiles::drag_paths);
    // (the first window on the CPU renderer or on the card: what this machine was like the last time, see `fast_start`)
    fast_start::before_app(!dock && !bar_preview && pick.is_none() && !quick_search && !snip_only && !no_window && !open_look_settings);
    start_mark("before the app");
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        start_mark("app running");
        gpui_kit::init(cx);
        ui_stall::start(cx);
        cx.set_global(settings::Settings::load());
        start_mark("settings loaded");
        // (an update may have written the shortcuts again: the chosen icon goes back on them)
        if !dock {
            app_icon::reapply(cx.global::<settings::Settings>().app_icon.clone());
        }
        rf_core::sort::set_turkish_collation(rf_win::encoding::user_locale_is_turkish());
        // eski kod sayfalı metinlerin tahmini: Türkçe olmayan sistemde Türkçe ipucu verilmez
        if !rf_win::encoding::user_locale_is_turkish() {
            rf_core::textload::set_locale_hint(None);
        }
        settings::apply_component_theme(cx);
        settings::prewarm_icon_pack(cx);
        cx.set_global(tags::load());
        let ops = cx.new(status_center::Ops::new);
        // the Status Center flyout is its own Acrylic window, opened/closed with `open`
        cx.observe(&ops, |ops, cx| {
            status_center::sync_popup(&ops, cx);
            status_center::sync_taskbar(&ops, cx);
        })
        .detach();
        cx.set_global(status_center::OpsGlobal(ops));
        actions::bind(cx);
        actions::bind_user(cx);
        keymap_editor::init(cx);
        if let Some(page) = &settings_arg {
            settings_view::open_page(
                match page.as_str() {
                    "speed" => settings_view::Page::Speed,
                    "general" => settings_view::Page::General,
                    "appearance" => settings_view::Page::Appearance,
                    "dock" => settings_view::Page::Dock,
                    "filetypes" => settings_view::Page::FileTypes,
                    "hypersearch" => settings_view::Page::Search,
                    "advanced" => settings_view::Page::Advanced,
                    "addons" => settings_view::Page::Addons,
                    _ => settings_view::Page::Look,
                },
                cx,
            );
        }
        // copies and moves keep a crash-safe journal there (see rf_win::fastcopy)
        if let Some(d) = settings::app_dir() {
            rf_win::fastcopy::set_journal_dir(d.join("jobs"));
        }
        // folder sizes survive a restart (shown at once, measured again when asked for)
        if let Some(d) = settings::local_data_dir() {
            rf_win::foldersize::shared_cache().persist(d.join("foldersizes.bin"));
        }
        match pick.clone() {
            // started only to answer a dialog: no main window
            Some(pipe) => picker::begin(pipe, cx),
            None if desktop_preview => {
                let args: Vec<String> = std::env::args().skip(1).collect();
                shell::desktop::open_preview(shell::desktop::args::parse(&args, std::env::var(shell::desktop::args::ENV_DIR).ok().as_deref()), cx)
            }
            // (when no window could be made the process would stay with nothing on the screen and no tray icon: it ends here)
            None if switcher_preview => {
                if !shell::switcher::open_preview(cx) {
                    cx.quit();
                }
            }
            None if bar_preview => taskbar_preview::open(cx),
            // the dock: three islands, no main window
            None if dock => dock::run(dock_messages.take(), cx),
            // the screen snip alone: it ends by itself when it is over
            None if snip_only => snip::run_standalone(cx),
            // started for the quick search only: no main window (it quits when that closes,
            // unless running in the background)
            None if quick_search => quick_search::toggle(cx),
            // started by the dock for its file search: no window, no tray icon; it ends by itself when nobody searches for a while
            None if no_window => search_pipe::watch_idle(cx),
            None => {
                installer::count_start(cx);
                donate::refresh();
                start_mark("opening the window");
                open_main_window(if open_look_settings { Some(Location::Settings) } else { start }, None, None, cx);
                start_mark("window opened");
                fast_start::remember(cx);
                // a copy or move cut off last time: ask to carry it on
                status_center::offer_resume(cx);
            }
        }
        if snip_only {
            // nothing else of the app runs in this process; the snip quits it when it is over
            cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
            return;
        }
        if dock {
            // a side process: no updates, no DirectLook, no tray, no drive-eject handler, and no "last window closed" saving of
            // the settings (it only ever reads them); it ends when the dock module quits it
            cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
            return;
        }
        // the dock, when the setting is on (a separate process, started off the UI thread; it keeps running without us)
        // (not for a picker or the quick search started on their own, not for a test copy)
        if pick.is_none() && !quick_search && !multi {
            dock::control::start_if_wanted(cx.global::<settings::Settings>());
            // Win+E for the default file manager while no dock takes it (issue 44: Windows' newer Win+E skips the registry)
            shell::keylayer::watch_main(cx);
        }
        // updates from GitHub Releases (installed copies only)
        installer::set_beta_updates(cx.global::<settings::Settings>().beta_updates);
        installer::spawn_checks(cx, |cx| cx.refresh_windows());
        // DirectLook ayrı süreç: önceden başlatılır ki ilk Boşluk beklemesin (testlerde değil)
        if cx.global::<settings::Settings>().directlook_enabled && std::env::var_os("RUSTYFILES_MULTI_INSTANCE").is_none() && !bar_preview {
            look::ipc::send(look::ipc::Msg::Wake, true);
        }
        // "Safely remove" / eject: let go of a drive when Windows asks (see rf_win::removal)
        {
            let (tx, mut rx) = futures::channel::mpsc::unbounded::<(char, std::sync::mpsc::Sender<()>)>();
            rf_win::removal::start(move |letter, ack| {
                let _ = tx.unbounded_send((letter, ack));
            });
            cx.spawn(async move |cx| {
                use futures::StreamExt;
                while let Some((letter, ack)) = rx.next().await {
                    cx.update(|cx| workspace::release_drive(letter, cx));
                    let _ = ack.send(());
                }
            })
            .detach();
        }
        // Second launches arrive here and go to whichever window is still open.
        let mut ipc_rx = ipc_rx;
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(msg) = ipc_rx.next().await {
                cx.update(|cx| workspace::route_ipc(msg, cx));
            }
        })
        .detach();
        // The launches of second processes, which wait for this answer: told once the tab or window is there. A quit that was held
        // back for the sake of a request (see save_and_quit) is looked at again when the last one is acted on, and goes ahead if the
        // request left nothing open.
        let mut launch_rx = launch_rx;
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some((msg, ack)) = launch_rx.next().await {
                rf_win::handover::trace("first: acting on a request");
                let shown = cx.update(|cx| workspace::route_ipc_shown(msg, cx));
                // (no window could be made: the second launch is told, and shows the folder itself)
                let quit_held_back = if shown { ack.done() } else { ack.refuse() };
                if quit_held_back {
                    cx.update(|cx| last_window::settle(cx, switcher_preview));
                }
            }
        })
        .detach();
        // The last window closing ends the app (GPUI does not on its own: the process would
        // linger invisibly and swallow every later launch through the single-instance pipe).
        // With "run in the background" on, the app stays in the notification area instead.
        background::init(cx);
        quick_search::init(cx);
        // the folder organizer's watchers (only if a rule runs by itself); not in helper processes
        if primary_instance {
            organizer::init(cx);
            // Ferry (off until the user turns it on): only the main process listens
            ferry::init(cx);
        }
        // GPUI would quit by itself when the last window closes (QuitMode::Default on Windows):
        // that cut running copies in half and makes background mode impossible. We decide.
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        cx.on_window_closed(move |cx, id| {
            workspace::save_closed_placement(id, cx);
            last_window::settle(cx, switcher_preview);
        })
        .detach();
        cx.activate(true);
    });
    rf_win::handover::trace("main: event loop ended");
    // (the host's process says why it ended when no earlier line did)
    shell::exits::after_run();
}
