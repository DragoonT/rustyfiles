//! rustyfiles ve DirectLook'un ortak kodu; iki program da bu kütüphaneyi kullanır.

mod actions;
mod aliases;
mod background;
pub mod bench;
mod bench_view;
mod donate;
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
mod folder_view;
mod icons;
mod keymap_editor;
mod last_window;
mod logfile;
mod layouts;
pub mod look;
mod image_cache;
mod installer;
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
mod taskbar_preview;
mod update_splash;
mod shelf_view;
mod status_center;
mod transfer_view;
mod tags;
mod undo;
mod preview;
mod preview_trace;
mod properties_view;
mod quick_search;
mod run_names;
mod settings_rows;
mod search_pipe;
mod hotkey_field;
mod recycle_settings_view;
mod recycle_text;
mod recycle_view;
mod theme;
mod thumb_cache;
mod workspace;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;
use rf_core::Location;

use crate::workspace::Workspace;

/// Window placement from the last session (ignored if it is implausibly small).
pub(crate) fn restored_bounds(cx: &App) -> Option<WindowBounds> {
    let [x, y, w, h, max] = cx.global::<settings::Settings>().window?;
    if w < 500. || h < 350. {
        return None;
    }
    let b = Bounds { origin: point(px(x), px(y)), size: size(px(w), px(h)) };
    Some(if max > 0.5 { WindowBounds::Maximized(b) } else { WindowBounds::Windowed(b) })
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

/// The Open/Save dialog DLL to register: a copy of the one next to this exe, outside the
/// install folder (`%LocalAppData%\rustyfiles\dialog\<build>\`). Apps keep the registered DLL
/// loaded, and an update can't replace an install folder with a loaded DLL in it. Next to the
/// copy, `rustyfiles-exe.txt` tells the DLL which rustyfiles to start. Older copies go once no
/// app has them loaded.
pub(crate) fn dialog_dll() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let src = exe.with_file_name(rf_win::dialog_reg::DLL_NAME);
    let meta = std::fs::metadata(&src).ok()?;
    let stamp = meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    let root = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("rustyfiles").join("dialog");
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
    if let Ok(rd) = std::fs::read_dir(&root) {
        for e in rd.flatten() {
            if e.path() != dir {
                let _ = std::fs::remove_dir_all(e.path()); // fails while an app has it loaded
            }
        }
    }
    Some(dst.display().to_string())
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
    let options = WindowOptions {
        window_bounds: Some(bounds.unwrap_or_else(|| WindowBounds::centered(size(px(1280.), px(800.)), cx))),
        window_min_size: Some(size(px(720.), px(420.))),
        window_background: WindowBackgroundAppearance::MicaBackdrop,
        ..TitleBar::window_options()
    };
    if let Err(e) = gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| Workspace::new(start, ipc, window, cx))) {
        log::error!("open window: {e:#}");
        rf_win::handover::trace("open window: failed");
        return false;
    }
    true
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
    let arg = arg.filter(|_| !new_window);
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
            let server = rf_win::network::server_root(std::path::Path::new(&arg)).is_some();
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
        // Open/Save dialogs: a registration whose DLL moved is pointed here (not from tests)
        if registers {
            if rf_win::dialog_reg::is_enabled(rf_win::dialog_reg::CLASSES) {
                if let Some(dll) = dialog_dll() {
                    if rf_win::dialog_reg::point_to(&dll, rf_win::dialog_reg::CLASSES) {
                        log::warn!("file dialogs now use {dll}");
                    }
                }
            }
        }
        FILE_DIALOGS.store(rf_win::dialog_reg::is_enabled(rf_win::dialog_reg::CLASSES), std::sync::atomic::Ordering::Relaxed);
        DEFAULT_FM.store(
            rf_win::default_fm::is_enabled(rf_win::default_fm::CLASSES),
            std::sync::atomic::Ordering::Relaxed,
        )
    });

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
        _ if quick_search => quick_search::LAUNCH_ARG.to_string(),
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
                return;
            }
            rf_win::single_instance::Role::Primary(guard) => Some(guard),
        }
    };
    let primary_instance = _instance.is_some() && pick.is_none() && !quick_search && !bar_preview && !dock;
    let (ipc_tx, ipc_rx) = futures::channel::mpsc::unbounded::<String>();
    // second launches: each comes with the acknowledgement it waits for (the other senders of ipc_tx need none)
    let (launch_tx, launch_rx) = futures::channel::mpsc::unbounded::<(String, rf_win::handover::Ack)>();
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

    start_mark("before the app");
    gpui_kit::application().with_assets(gpui_kit::assets::AllAssets).run(move |cx| {
        start_mark("app running");
        gpui_kit::init(cx);
        ui_stall::start(cx);
        cx.set_global(settings::Settings::load());
        start_mark("settings loaded");
        rf_core::sort::set_turkish_collation(rf_win::encoding::user_locale_is_turkish());
        // eski kod sayfalı metinlerin tahmini: Türkçe olmayan sistemde Türkçe ipucu verilmez
        if !rf_win::encoding::user_locale_is_turkish() {
            rf_core::textload::set_locale_hint(None);
        }
        settings::apply_component_theme(cx);
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
                    "dock" => settings_view::Page::Dock,
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
        let bounds = restored_bounds(cx);
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
                open_main_window(if open_look_settings { Some(Location::Settings) } else { start }, None, bounds, cx);
                start_mark("window opened");
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
        }
        // GPUI would quit by itself when the last window closes (QuitMode::Default on Windows):
        // that cut running copies in half and makes background mode impossible. We decide.
        cx.set_quit_mode(gpui_kit::QuitMode::Explicit);
        cx.on_window_closed(move |cx, _| last_window::settle(cx, switcher_preview))
        .detach();
        cx.activate(true);
    });
    rf_win::handover::trace("main: event loop ended");
    // (the host's process says why it ended when no earlier line did)
    shell::exits::after_run();
}
