//! Which programs are using the microphone or the camera right now: what macOS shows at the top of its Control Centre and Windows
//! as a small symbol in the tray.
//!
//! SOURCE. Windows keeps, per user, when each program last began and stopped using a capability (the "consent store" under
//! `CapabilityAccessManager`; it is what Settings > Privacy > Microphone lists as "last accessed"). A program whose start is set and
//! whose stop is zero is using it now. Read only, from the user's own registry; nothing is asked of the programs.
//!
//! The control centre reads it when it opens and every few seconds while it is open (there is no event worth a thread for a line of
//! text).

/// The programs using each of the two now, by a short name, in the order Windows lists them.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct InUse {
    pub microphone: Vec<String>,
    pub camera: Vec<String>,
}

impl InUse {
    pub fn any(&self) -> bool {
        !self.microphone.is_empty() || !self.camera.is_empty()
    }
}

/// A program's short name from its key in the consent store. A classic program's key is its path with `#` for the separators
/// ("C:#Program Files#App#app.exe": "app"); a packaged one's is its package family name ("Microsoft.WindowsCamera_8wekyb3d8bbwe":
/// "WindowsCamera").
pub fn app_name(key: &str, packaged: bool) -> String {
    if packaged {
        let family = key.split('_').next().unwrap_or(key);
        return family.rsplit('.').next().unwrap_or(family).to_string();
    }
    let file = key.rsplit('#').next().unwrap_or(key);
    let stem = if file.len() > 4 && file.to_ascii_lowercase().ends_with(".exe") { &file[..file.len() - 4] } else { file };
    stem.to_string()
}

/// In use now: it began and has not stopped.
pub fn is_in_use(start: u64, stop: u64) -> bool {
    start != 0 && stop == 0
}

fn users_of(capability: &str) -> Vec<String> {
    let path = format!(r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\{capability}");
    let Ok(root) = windows_registry::CURRENT_USER.open(&path) else { return Vec::new() };
    let using = |key: &windows_registry::Key| is_in_use(key.get_u64("LastUsedTimeStart").unwrap_or(0), key.get_u64("LastUsedTimeStop").unwrap_or(1));
    let mut out: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if !name.trim().is_empty() && !out.contains(&name) {
            out.push(name);
        }
    };
    let Ok(names) = root.keys() else { return Vec::new() };
    for name in names {
        let Ok(key) = root.open(&name) else { continue };
        if name == "NonPackaged" {
            let Ok(apps) = key.keys() else { continue };
            for app in apps {
                if key.open(&app).is_ok_and(|k| using(&k)) {
                    add(app_name(&app, false));
                }
            }
        } else if using(&key) {
            add(app_name(&name, true));
        }
    }
    out
}

/// Reads who is using the microphone and the camera now. A few registry reads: off the interface's thread.
pub fn read() -> InUse {
    InUse { microphone: users_of("microphone"), camera: users_of("webcam") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_named_by_its_file_or_its_package() {
        assert_eq!(app_name(r"C:#Program Files#Google#Chrome#Application#chrome.exe", false), "chrome");
        assert_eq!(app_name("C:#Users#o#AppData#Local#Discord#app-1.0.9#Discord.EXE", false), "Discord");
        assert_eq!(app_name("tool", false), "tool");
        assert_eq!(app_name(".exe", false), ".exe");
        assert_eq!(app_name("Microsoft.WindowsCamera_8wekyb3d8bbwe", true), "WindowsCamera");
        assert_eq!(app_name("MSTeams_8wekyb3d8bbwe", true), "MSTeams");
        assert_eq!(app_name("5319275A.WhatsAppDesktop_cv1g1gvanyjgm", true), "WhatsAppDesktop");
    }

    #[test]
    fn in_use_is_begun_and_not_stopped() {
        assert!(is_in_use(133_000_000_000_000_000, 0));
        assert!(!is_in_use(133_000_000_000_000_000, 133_000_000_100_000_000));
        assert!(!is_in_use(0, 0), "never used");
        assert!(!InUse::default().any());
        assert!(InUse { microphone: vec!["chrome".into()], camera: Vec::new() }.any());
    }

    /// On the PC it is run on, by hand: how many programs use each now (not which).
    #[test]
    #[ignore = "reads the PC's own privacy records; run by hand"]
    fn what_this_pc_says() {
        let now = read();
        println!("microphone {} camera {}", now.microphone.len(), now.camera.len());
    }
}
