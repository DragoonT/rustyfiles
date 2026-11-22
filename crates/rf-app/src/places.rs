//! Shared sidebar/home data: drives and pinned folders, loaded off the UI thread.

use std::path::PathBuf;

use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_win::{Drive, DriveKind, KnownFolder};

/// Drive display name with the generic fallback labels translated ("Yerel Disk (C:)").
pub fn drive_name(d: &Drive) -> String {
    if d.label.is_empty() {
        let kind = match d.kind {
            DriveKind::Removable => crate::t!("USB Sürücü"),
            DriveKind::Network => crate::t!("Ağ Sürücüsü"),
            DriveKind::Optical => crate::t!("DVD Sürücü"),
            _ => crate::t!("Yerel Disk"),
        };
        format!("{kind} ({}:)", d.letter)
    } else {
        d.display_name()
    }
}

#[derive(Clone)]
pub struct Pinned {
    pub title: String,
    pub path: PathBuf,
    pub icon: L,
}

#[derive(Clone)]
pub struct Recent {
    pub name: String,
    pub target: PathBuf,
    pub used_ms: i64,
    pub is_dir: bool,
}

/// Windows' own "recent items" (%APPDATA%\Microsoft\Windows\Recent\*.lnk), newest first.
fn load_recent(limit: usize) -> Vec<Recent> {
    let Some(dir) = dirs::data_dir().map(|d| d.join("Microsoft").join("Windows").join("Recent")) else {
        return Vec::new();
    };
    let Ok(rd) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut links: Vec<(std::time::SystemTime, PathBuf)> = rd
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    links.sort_by(|a, b| b.0.cmp(&a.0));
    let mut out: Vec<Recent> = Vec::new();
    for (t, link) in links.into_iter().take(limit * 3) {
        let Ok(info) = rf_win::shortcut::resolve(&link) else { continue };
        if info.target.as_os_str().is_empty() || out.iter().any(|r| r.target == info.target) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(&info.target) else { continue };
        out.push(Recent {
            name: info.target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            target: info.target,
            used_ms: t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0),
            is_dir: meta.is_dir(),
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

/// Cloud folders (OneDrive, Dropbox, iCloud, Google Drive) and WSL distributions.
fn load_cloud_and_wsl() -> Vec<Pinned> {
    let mut out = Vec::new();
    let mut add = |title: String, path: PathBuf, icon: L| {
        let same = |p: &Pinned| p.path.as_os_str().eq_ignore_ascii_case(path.as_os_str());
        if path.is_dir() && !out.iter().any(same) {
            out.push(Pinned { title, path, icon });
        }
    };
    for (var, title) in [("OneDriveConsumer", "OneDrive"), ("OneDriveCommercial", "OneDrive (İş)"), ("OneDrive", "OneDrive")] {
        if let Some(v) = std::env::var_os(var) {
            add(title.into(), PathBuf::from(v), L::Cloud);
        }
    }
    if let Some(home) = dirs::home_dir() {
        add("iCloud Drive".into(), home.join("iCloudDrive"), L::Cloud);
        add("Dropbox".into(), home.join("Dropbox"), L::Cloud);
    }
    if let Some(local) = dirs::data_local_dir() {
        // Dropbox records its real location in info.json
        if let Ok(t) = std::fs::read_to_string(local.join("Dropbox").join("info.json")) {
            if let Some(i) = t.find("\"path\": \"") {
                let rest = &t[i + 9..];
                if let Some(j) = rest.find('"') {
                    add("Dropbox".into(), PathBuf::from(rest[..j].replace("\\\\", "\\")), L::Cloud);
                }
            }
        }
    }
    for letter in ['G', 'H'] {
        add("Google Drive".into(), PathBuf::from(format!("{letter}:\\My Drive")), L::Cloud);
    }
    // every other sync root Windows knows (issue 141: Proton Drive, pCloud, Nextcloud, Box, MEGA ...), as Explorer lists them; one
    // that is already in the list above (OneDrive) stays as it is
    for r in rf_win::syncroots::list() {
        add(r.name, r.path, L::Cloud);
    }
    // WSL distributions registered for this user
    if let Ok(lxss) = windows_registry::CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Lxss") {
        if let Ok(keys) = lxss.keys() {
            for k in keys {
                if let Ok(d) = lxss.open(&k) {
                    if let Ok(name) = d.get_string("DistributionName") {
                        let path = PathBuf::from(format!(r"\\wsl.localhost\{name}"));
                        out.push(Pinned { title: format!("WSL: {name}"), path, icon: L::SquareTerminal });
                    }
                }
            }
        }
    }
    out
}

pub struct Places {
    pub drives: Vec<Drive>,
    pub cloud: Vec<Pinned>,
    pub pinned: Vec<Pinned>,
    pub recent: Vec<Recent>,
    pub libraries: Vec<rf_win::libraries::Library>,
    /// Network locations ("Add a network location" shortcuts): (name, target).
    pub network: Vec<(String, std::path::PathBuf)>,
    /// Devices under This PC that are not drives (MTP phones, cameras): browsed as shell
    /// folders.
    pub devices: Vec<rf_win::shellfolder::ShellChild>,
    /// Whether the Windows Recycle Bin contains items; `None` until the shell has answered.
    pub recycle_full: Option<bool>,
    recycle_icon_hovered: bool,
    pub loaded: bool,
    /// The recent items and the cloud / WSL entries are in too (they come a little after `loaded`: see `refresh`).
    pub extras_loaded: bool,
    _task: Option<Task<()>>,
    _recycle_task: Option<Task<()>>,
}

impl Places {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this =
            Self {
                drives: Vec::new(),
                cloud: Vec::new(),
                pinned: Vec::new(),
                recent: Vec::new(),
                libraries: Vec::new(),
                network: Vec::new(),
                devices: Vec::new(),
                recycle_full: None,
                recycle_icon_hovered: false,
                loaded: false,
                extras_loaded: false,
                _task: None,
                _recycle_task: None,
            };
        this.refresh(cx);
        // the drives the user hides (Settings > Files and folders, a drive's own menu) leave and come back at once
        let mut hidden = rf_win::drives::hidden();
        // and the pins, when they change anywhere (a pinned folder deleted or renamed: `pins_follow`)
        let mut pins = cx.global::<crate::settings::Settings>().pinned.clone();
        let mut recycle_theme = cx.global::<crate::settings::Settings>().windows_file_explorer_theme;
        cx.observe_global::<crate::settings::Settings>(move |p, cx| {
            let now = rf_win::drives::hidden();
            let settings = cx.global::<crate::settings::Settings>();
            let now_pins = settings.pinned.clone();
            let now_recycle_theme = settings.windows_file_explorer_theme;
            if recycle_theme && !now_recycle_theme {
                p.set_recycle_icon_hovered(false, cx);
            }
            recycle_theme = now_recycle_theme;
            if now != hidden || now_pins != pins {
                hidden = now;
                pins = now_pins;
                p.refresh(cx);
            }
        })
        .detach();
        // Notice drives appearing/disappearing (USB, network mappings) without any shell hooks.
        cx.spawn(async move |this, cx| {
            let mut mask = rf_win::drives::drive_mask();
            loop {
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
                let now = rf_win::drives::drive_mask();
                if now != mask {
                    mask = now;
                    if this.update(cx, |p, cx| p.refresh(cx)).is_err() {
                        break;
                    }
                }
            }
        })
        .detach();
        // Phones come and go without a drive letter: ask the shell now and then. Off the UI
        // thread, and the list is only replaced when it changed. Not while no window is in front
        // of the user (minimised, or another app active): enumerating This PC loads shell
        // extensions and can touch the drives; the look happens again when one comes back.
        cx.spawn(async move |this, cx| loop {
            if !rf_win::window::app_in_front() {
                cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
                if this.upgrade().is_none() {
                    break;
                }
                continue;
            }
            let found = cx.background_spawn(async move { rf_win::shellfolder::devices() }).await;
            let alive = this
                .update(cx, |p, cx| {
                    if p.devices != found {
                        p.devices = found;
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
            cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
        })
        .detach();
        this
    }

    pub fn set_recycle_icon_hovered(&mut self, hovered: bool, cx: &mut Context<Self>) {
        if self.recycle_icon_hovered == hovered {
            return;
        }
        self.recycle_icon_hovered = hovered;
        if !hovered {
            self._recycle_task = None;
            return;
        }

        self._recycle_task = Some(cx.spawn(async move |this, cx| {
            let mut error_logged = false;
            loop {
                match cx.background_spawn(async { rf_win::recycle::info() }).await {
                    Ok((item_count, _)) => {
                        error_logged = false;
                        if this
                            .update(cx, |places, cx| {
                                let full = item_count > 0;
                                if places.recycle_full != Some(full) {
                                    places.recycle_full = Some(full);
                                    cx.notify();
                                }
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if !error_logged => {
                        log::warn!("could not read Recycle Bin state for its icon: {error:#}");
                        error_logged = true;
                    }
                    Err(_) => {}
                }
                cx.background_executor().timer(std::time::Duration::from_secs(3)).await;
            }
        }));
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let user_pins = cx.try_global::<crate::settings::Settings>().and_then(|s| s.pinned.clone());
        // Two pieces of work, so that the sidebar does not wait for the slow one. The recent items ask the disk about the place each
        // one names (a stat each), and so do the cloud entries (OneDrive, Dropbox, Google Drive...); a place that has to wake up first
        // (an offline share, a sleeping disk, a WSL folder opened lately) answers after seconds. The WSL distributions are read from
        // the registry and cost nothing. The pinned folders and the drives need none of that.
        // The lists are not complete until the second piece is in, so the jump list waits for it again after a refresh.
        self.extras_loaded = false;
        let work = cx.background_spawn(async move {
            let known = rf_win::known_folders();
            let pinned = match user_pins {
                None => known
                    .into_iter()
                    .map(|(k, path)| Pinned { title: k.title().to_string(), path, icon: known_icon(k) })
                    .collect::<Vec<_>>(),
                Some(paths) => paths
                    .into_iter()
                    .map(|path| match known.iter().find(|(_, p)| *p == path) {
                        Some((k, _)) => Pinned { title: k.title().to_string(), path, icon: known_icon(*k) },
                        None => Pinned {
                            title: rf_core::history::display_name(&path),
                            path,
                            icon: L::Folder,
                        },
                    })
                    .collect(),
            };
            // (less the drives the user hides and the ones Explorer hides)
            let drives = rf_win::drives::visible_drives();
            let libraries = rf_win::libraries::list();
            let network = rf_win::network::network_shortcuts();
            (pinned, drives, libraries, network)
        });
        let extras = cx.background_spawn(async move {
            let recent = load_recent(14);
            // the localized names the jump list and the Home view show for these folders are read here, on this thread: the
            // drawing only looks in the cache (see rf_win::localized::label_if_known)
            rf_win::localized::warm(recent.iter().filter(|r| r.is_dir).flat_map(|r| crate::nav_touch::label_paths(&r.target)));
            (recent, load_cloud_and_wsl())
        });
        self._task = Some(cx.spawn(async move |this, cx| {
            let (pinned, drives, libraries, network) = work.await;
            // the names of the pinned folders and of the folders above them (the breadcrumb of a click on a pin), read off the UI
            // thread and not in the piece the sidebar waits for
            let pin_paths: Vec<PathBuf> = pinned.iter().flat_map(|p| crate::nav_touch::label_paths(&p.path)).collect();
            let pin_names = cx.background_spawn(async move { rf_win::localized::warm(pin_paths) });
            let _ = this.update(cx, |p, cx| {
                p.pinned = pinned;
                p.drives = drives;
                p.libraries = libraries;
                p.network = network;
                p.loaded = true;
                cx.notify();
            });
            let (recent, cloud) = extras.await;
            let _ = this.update(cx, |p, cx| {
                p.recent = recent;
                p.cloud = cloud;
                p.extras_loaded = true;
                cx.notify();
            });
            if pin_names.await > 0 {
                // a pinned folder has a localized name: the views that show it are drawn again
                let _ = this.update(cx, |_, cx| cx.notify());
            }
        }));
    }
}

pub fn known_icon(k: KnownFolder) -> L {
    match k {
        KnownFolder::Desktop => L::Monitor,
        KnownFolder::Downloads => L::Download,
        KnownFolder::Documents => L::FileText,
        KnownFolder::Pictures => L::Image,
        KnownFolder::Music => L::Music,
        KnownFolder::Videos => L::Video,
        KnownFolder::Profile => L::House,
    }
}

pub fn drive_icon(d: &Drive) -> L {
    match d.kind {
        DriveKind::Removable => L::Usb,
        DriveKind::Network => L::Network,
        DriveKind::Optical => L::Disc3,
        _ => L::HardDrive,
    }
}

/// The user's pins after `from` was deleted (`to` None) or moved or renamed to `to`: a pin on `from` or inside it goes, or follows it.
/// `None` when no pin is touched.
///
/// WHY (the owner, trying 0.8.8's sidebar menu on 2026-11-17: "sidebarda siliyorum. dosya siliniyor ama sidebarda görünmeye devam
/// ediyor"): the pins are a list in the settings, and deleting or renaming the folder did not touch it.
pub fn pins_after(pins: &[std::path::PathBuf], from: &std::path::Path, to: Option<&std::path::Path>) -> Option<Vec<std::path::PathBuf>> {
    let touched = |p: &std::path::PathBuf| p.starts_with(from);
    if !pins.iter().any(touched) {
        return None;
    }
    Some(
        pins.iter()
            .filter_map(|p| match (p.strip_prefix(from), to) {
                (Ok(_), None) => None,
                (Ok(rest), Some(to)) => Some(if rest.as_os_str().is_empty() { to.to_path_buf() } else { to.join(rest) }),
                (Err(_), _) => Some(p.clone()),
            })
            .collect(),
    )
}

/// Applies [`pins_after`] to the user's own pin list (the default pins, when the user never changed them, are the known folders and
/// stay as they are).
pub fn pins_follow(cx: &mut App, from: &std::path::Path, to: Option<&std::path::Path>) {
    let Some(list) = cx.global::<crate::settings::Settings>().pinned.clone() else { return };
    if let Some(next) = pins_after(&list, from, to) {
        crate::settings::update(cx, move |s| s.pinned = Some(next));
    }
}

#[cfg(test)]
mod pins_follow_tests {
    use super::pins_after;
    use std::path::{Path, PathBuf};

    #[test]
    fn a_pin_goes_with_its_folder_or_follows_it() {
        let pins = vec![PathBuf::from(r"C:\a\Work"), PathBuf::from(r"C:\a\Work\inner"), PathBuf::from(r"D:\Photos")];
        assert_eq!(pins_after(&pins, Path::new(r"C:\a\Work"), None), Some(vec![PathBuf::from(r"D:\Photos")]), "deleted: it and what was inside go");
        assert_eq!(
            pins_after(&pins, Path::new(r"C:\a\Work"), Some(Path::new(r"C:\a\Job"))),
            Some(vec![PathBuf::from(r"C:\a\Job"), PathBuf::from(r"C:\a\Job\inner"), PathBuf::from(r"D:\Photos")])
        );
        assert_eq!(pins_after(&pins, Path::new(r"C:\a\Wor"), None), None, "a name that only starts the same is another folder");
        assert_eq!(pins_after(&pins, Path::new(r"E:\x"), None), None);
    }
}
