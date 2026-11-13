//! The dock's parts and the ONE switch that starts them (Settings -> Dock, `dock_enabled`).
//!
//! WHY ONE SWITCH. The dock used to be five switches (the desktop, Windows' taskbar, the Windows key, Alt+Tab, the tray icons) beside the
//! dock's own, and the dock only worked properly with all of them: with the taskbar hidden and no tray host there were no tray icons
//! anywhere, with the dock and Windows' taskbar there were two bars, without the keyboard layer the Windows key opened Windows' Start
//! under the hidden taskbar, without our desktop the dock floated over Explorer's. So the switch is one: the dock with everything it
//! needs.
//!
//! THE RULE. A part runs when the dock switch is on AND the part's own value is not false ([`runs`]). The own values stay in the
//! settings file under the names they always had (`shell_desktop_cover`, `shell_hide_taskbar`, `shell_win_key`, `shell_own_switcher`,
//! `shell_tray_host`) as TROUBLESHOOTING OVERRIDES: the Dock page shows no switch for them. Their default is true.
//!
//! FILES FROM BEFORE. The old default was false, but a file only holds `= false` when it was written whole (the first run, or a build
//! from before the settings were saved key by key); a file that existed earlier and was saved since holds `dock_enabled = true` and no
//! part at all. So the file cannot tell "the user chose this" from "the old default", and the rule is made so that an update never
//! gives anyone a keyboard hook or a hidden taskbar they have not seen: a false is read as "held off", and so is a part that a file
//! with the dock on does not name, as long as the file has no `dock_one_switch` marker (`Settings::migrate`; the first save writes
//! the marker and the held values, so the hold outlives the marker). The Dock page says in one line which parts the file holds off
//! ([`held_off`]), and one button clears them ([`clear_overrides`]). A file with the dock off, or a new install, gives every part with
//! the dock when it is turned on: that is the user turning on the one switch, with its description in front of them. A part's own
//! value that cannot be read is a false too (`Settings::parse_lenient`).
//!
//! This file is the pure rules and the texts; the file the dock process writes to say that a part could not start, and the page's way
//! of reading it, are in `parts_status.rs`.

use crate::settings::Settings;
use crate::t;

/// One of the things the one switch starts besides the islands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Part {
    /// Our desktop over Explorer's (`shell/cover.rs`).
    Desktop,
    /// Explorer's taskbar hidden while the dock runs, the dock taking its edge (`shell/taskbar_hide.rs`).
    Taskbar,
    /// A tap of the Windows key opens our Start panel (`shell/keylayer.rs`).
    WinKey,
    /// Alt+Tab opens our switcher (`shell/keylayer.rs`).
    Switcher,
    /// The programs' tray icons, taken by our own tray host and shown in the dock (`shell/trayhost.rs`).
    TrayHost,
}

impl Part {
    /// In the order the dock has always listed them (the order of `Keys::shell`).
    pub const ALL: [Part; 5] = [Part::Desktop, Part::Taskbar, Part::WinKey, Part::Switcher, Part::TrayHost];

    /// The name of the part's own value in the settings file (the tests tie it to the struct).
    pub fn field(self) -> &'static str {
        match self {
            Part::Desktop => "shell_desktop_cover",
            Part::Taskbar => "shell_hide_taskbar",
            Part::WinKey => "shell_win_key",
            Part::Switcher => "shell_own_switcher",
            Part::TrayHost => "shell_tray_host",
        }
    }

    /// A short word for the status file (never shown).
    pub fn token(self) -> &'static str {
        match self {
            Part::Desktop => "desktop",
            Part::Taskbar => "taskbar",
            Part::WinKey => "winkey",
            Part::Switcher => "switcher",
            Part::TrayHost => "tray",
        }
    }

    pub fn from_token(token: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.token() == token)
    }

    /// The part whose own value is called `name` in the settings file.
    pub fn from_field(name: &str) -> Option<Part> {
        Part::ALL.into_iter().find(|p| p.field() == name)
    }

    /// The part's own value: what the file says about it (true when it says nothing).
    pub fn own_value(self, s: &Settings) -> bool {
        match self {
            Part::Desktop => s.shell_desktop_cover,
            Part::Taskbar => s.shell_hide_taskbar,
            Part::WinKey => s.shell_win_key,
            Part::Switcher => s.shell_own_switcher,
            Part::TrayHost => s.shell_tray_host,
        }
    }

    pub fn set_own_value(self, s: &mut Settings, on: bool) {
        match self {
            Part::Desktop => s.shell_desktop_cover = on,
            Part::Taskbar => s.shell_hide_taskbar = on,
            Part::WinKey => s.shell_win_key = on,
            Part::Switcher => s.shell_own_switcher = on,
            Part::TrayHost => s.shell_tray_host = on,
        }
    }

    /// What the part is called in a sentence ("... holds off: Windows tuşu, Alt+Tab").
    pub fn label(self) -> &'static str {
        match self {
            Part::Desktop => t!("masaüstü"),
            Part::Taskbar => t!("görev çubuğunu gizleme"),
            Part::WinKey => t!("Windows tuşu"),
            Part::Switcher => t!("Alt+Tab"),
            Part::TrayHost => t!("tepsi simgeleri"),
        }
    }
}

/// Whether the part runs with these settings: the one switch is on and the part is not held off by its own value.
pub fn runs(s: &Settings, part: Part) -> bool {
    s.dock_enabled && part.own_value(s)
}

/// [`runs`] for every part, in [`Part::ALL`] order.
pub fn run_flags(s: &Settings) -> [bool; 5] {
    Part::ALL.map(|p| runs(s, p))
}

/// The parts the file holds off (own value false), whether or not the switch is on.
pub fn held_off(s: &Settings) -> Vec<Part> {
    Part::ALL.into_iter().filter(|p| !p.own_value(s)).collect()
}

/// "Hepsini aç": every override cleared, so every part follows the switch.
pub fn clear_overrides(s: &mut Settings) {
    for p in Part::ALL {
        p.set_own_value(s, true);
    }
}

/// The one line the Dock page shows when the file holds parts off, or `None` when it holds none. Shown only while the switch is on:
/// with the dock off nothing runs either way, and the line is for the moment the user turns it on.
pub fn held_off_line(s: &Settings) -> Option<String> {
    let held = held_off(s);
    if !s.dock_enabled || held.is_empty() {
        return None;
    }
    let labels: Vec<&str> = held.iter().map(|p| p.label()).collect();
    Some(format!("{} {}", t!("Ayar dosyası şunları kapalı tutuyor, bu yüzden çalışmıyor:"), labels.join(", ")))
}

// ---------------------------------------------------------------- the texts of the one switch

pub fn switch_title() -> &'static str {
    t!("rustyfiles Dock'unu kullan (deneysel)")
}

/// What the one switch does, one short line each (the Settings search reads it, so the old names are in it: masaüstü, görev çubuğu,
/// tepsi, Windows tuşu, Alt+Tab).
pub fn switch_description() -> &'static str {
    t!("Başlat, arama ve denetim merkeziyle ekranın altında üç yüzen ada.\nKendi masaüstümüz (duvar kâğıdı ve simgeler), Explorer'ınkinin üstünde.\nProgramların tepsi simgeleri Dock'ta.\nDock çalıştığı sürece Windows görev çubuğu gizlenir; Dock durunca geri gelir.\nWindows tuşu ve Alt+Tab, Windows'unkini değil rustyfiles'ınkini açar.")
}

/// The warning about the keyboard hook, under the switch.
pub fn keyboard_warning() -> &'static str {
    t!("Windows tuşu ve Alt+Tab için klavye, sistem genelinde bir kancayla dinlenir; yalnızca Windows tuşuna tek başına basmaya (ve Windows tuşuyla E ya da D'ye) ve Alt+Tab'a (açıkken Sekme, oklar, Enter, Esc ve Alt'ı bırakmaya da) bakılır, başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez. Windows'un kendi Başlat menüsü ve öndeki programın menü çubuğu açılmasın diye tek bir görünmez, hiçbir şey yapmayan tuş gönderilir. Explorer kabuk değilken Win+E bir rustyfiles penceresi açar, Win+D masaüstünü gösterir; Windows tuşlu diğer kısayollara dokunulmaz. rustyfiles durursa ya da takılırsa Windows eski davranışına döner.")
}

/// The warning about the taskbar, under the switch.
pub fn taskbar_warning() -> &'static str {
    t!("Windows'un hiçbir görev çubuğu ayarı değiştirilmez. Görev çubuğu, Dock durunca geri gelir. Dock beklenmedik biçimde ölürse küçük bir yardımcı program (rustyfiles-shellguard.exe) onu bir iki saniye içinde geri getirir; yardımcı çalışmıyorsa görev çubuğu hiç gizlenmez.")
}

/// The line under the switch while the parts' real path has not been run on a PC (the lead removes it once it has been).
pub fn untried_line() -> &'static str {
    t!("Henüz hiç çalıştırılmadı: bu özelliğin gerçek yolu bir bilgisayarda denenmedi, yalnızca sahte girdilerle sınandı.")
}

/// The line on the Masaüstü page: the desktop has no switch of its own.
pub fn desktop_comes_with_the_dock() -> &'static str {
    t!("Masaüstü Dock'la birlikte gelir: açıp kapatmak için Dock sayfasındaki Dock anahtarı kullanılır. Burada yalnızca masaüstünün kendi seçimleri vardır.")
}

pub fn clear_button() -> &'static str {
    t!("Hepsini aç")
}

// ---------------------------------------------------------------- a part that could not start

/// Why a part is not running although it should be. Only faults: a situation ("Explorer is not the shell, so there is no taskbar to
/// hide") is not one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// `rustyfiles-shellguard.exe` is not next to the program.
    GuardMissing,
    /// The guard did not confirm in time.
    GuardNotConfirmed,
    /// The guard confirmed and then ended.
    GuardLost,
    /// The record that lets the guard undo the hide could not be written.
    RecordNotSaved,
    /// Windows refused the keyboard hook.
    HookRefused,
    /// The part could not be started for another reason (the log has the details).
    NotStarted,
}

impl Why {
    pub fn token(self) -> &'static str {
        match self {
            Why::GuardMissing => "guard-missing",
            Why::GuardNotConfirmed => "guard-not-confirmed",
            Why::GuardLost => "guard-lost",
            Why::RecordNotSaved => "record-not-saved",
            Why::HookRefused => "hook-refused",
            Why::NotStarted => "not-started",
        }
    }

    pub fn from_token(token: &str) -> Option<Why> {
        [Why::GuardMissing, Why::GuardNotConfirmed, Why::GuardLost, Why::RecordNotSaved, Why::HookRefused, Why::NotStarted].into_iter().find(|w| w.token() == token)
    }

    /// The one honest line for a part that could not start. It always says the rest keep running.
    pub fn line(self, part: Part) -> String {
        let what = match (self, part) {
            (Why::GuardMissing, _) => t!("Görev çubuğu gizlenemedi: rustyfiles-shellguard.exe program dosyasının yanında bulunamadı."),
            (Why::GuardNotConfirmed, _) => t!("Görev çubuğu gizlenemedi: yardımcı program (rustyfiles-shellguard.exe) çalıştığını bildirmedi."),
            (Why::GuardLost, _) => t!("Görev çubuğu geri getirildi: yardımcı program (rustyfiles-shellguard.exe) durdu."),
            (Why::RecordNotSaved, _) => t!("Görev çubuğu gizlenemedi: geri getirme kaydı yazılamadığı için hiçbir şey gizlenmez."),
            (Why::HookRefused, _) => t!("Windows tuşu ve Alt+Tab çalışmıyor: Windows klavye kancasını vermedi."),
            (Why::NotStarted, Part::Desktop) => t!("Masaüstü başlatılamadı."),
            (Why::NotStarted, Part::Taskbar) => t!("Görev çubuğunu gizleme başlatılamadı."),
            (Why::NotStarted, Part::WinKey | Part::Switcher) => t!("Windows tuşu ve Alt+Tab başlatılamadı."),
            (Why::NotStarted, Part::TrayHost) => t!("Tepsi simgeleri toplanamıyor: tepsi penceresi kurulamadı."),
        };
        format!("{what} {}", t!("Diğer parçalar çalışmaya devam ediyor."))
    }
}

/// The failures a status file or a status list holds, one line each, a pair of parts that share a cause (the Windows key and Alt+Tab
/// share the one hook) said once.
pub fn failure_lines(failures: &[(Part, Why)]) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for &(part, why) in failures {
        let line = why.line(part);
        if !lines.contains(&line) {
            lines.push(line);
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock_page::while_turkish;

    fn on() -> Settings {
        Settings { dock_enabled: true, ..Default::default() }
    }

    fn with(part: Part, value: bool) -> Settings {
        let mut s = on();
        part.set_own_value(&mut s, value);
        s
    }

    #[test]
    fn the_defaults_bring_every_part_with_the_dock_and_none_without_it() {
        let d = Settings::default();
        assert!(!d.dock_enabled, "the dock is opt-in");
        for p in Part::ALL {
            assert!(p.own_value(&d), "{p:?}: the own value defaults to true");
            assert!(!runs(&d, p), "{p:?} must not run while the switch is off");
            assert!(runs(&on(), p), "{p:?} runs with the switch");
        }
        assert_eq!(run_flags(&d), [false; 5]);
        assert_eq!(run_flags(&on()), [true; 5]);
    }

    #[test]
    fn a_part_runs_for_the_switch_and_its_own_value_and_each_override_holds_off_only_its_part() {
        for held in Part::ALL {
            let s = with(held, false);
            for p in Part::ALL {
                assert_eq!(runs(&s, p), p != held, "{p:?} with {held:?} held off");
            }
            // and with the switch off nothing runs whatever the overrides say
            let off = Settings { dock_enabled: false, ..s };
            assert!(Part::ALL.iter().all(|&p| !runs(&off, p)));
        }
        // the order of the flags is the order of the parts (the dock's Keys rely on it)
        let s = with(Part::WinKey, false);
        assert_eq!(run_flags(&s), [true, true, false, true, true]);
    }

    #[test]
    fn every_part_reads_and_writes_its_own_field_only() {
        let before = serde_json::to_value(Settings::default()).unwrap();
        for p in Part::ALL {
            let mut s = Settings::default();
            p.set_own_value(&mut s, false);
            let after = serde_json::to_value(&s).unwrap();
            let changed: Vec<_> = after.as_object().unwrap().iter().filter(|(k, v)| before.get(k.as_str()) != Some(*v)).map(|(k, _)| k.clone()).collect();
            assert_eq!(changed, [p.field()], "{p:?}");
            assert!(!p.own_value(&s));
        }
        let mut names: Vec<_> = Part::ALL.iter().map(|p| p.field()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 5);
    }

    // ---- what an old settings file becomes

    #[test]
    fn a_file_from_before_the_dock_gets_the_dock_off_and_every_part_ready_for_when_it_is_turned_on() {
        let s = Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap();
        assert!(!s.dock_enabled);
        assert!(held_off(&s).is_empty());
        assert_eq!(run_flags(&s), [false; 5], "nothing runs without the switch");
    }

    #[test]
    fn a_file_from_before_with_the_dock_on_that_names_no_part_holds_them_all_off_until_the_user_says_otherwise() {
        // the case a save-by-key produces: an old file, the dock turned on later, no part ever written. The update must not give it a hook.
        let s = Settings::parse_migrated("dock_enabled = true\n").unwrap();
        assert!(s.dock_enabled && s.dock_one_switch);
        assert_eq!(run_flags(&s), [false; 5]);
        assert_eq!(held_off(&s), Part::ALL.to_vec());
        while_turkish(|| assert!(held_off_line(&s).is_some()));
        // the button clears it
        let mut s = s;
        clear_overrides(&mut s);
        assert_eq!(run_flags(&s), [true; 5]);
    }

    #[test]
    fn only_the_parts_the_file_does_not_name_are_held_and_a_named_true_is_kept() {
        let s = Settings::parse_migrated("dock_enabled = true\nshell_win_key = true\nshell_tray_host = true\n").unwrap();
        assert_eq!(run_flags(&s), [false, false, true, false, true]);
    }

    #[test]
    fn a_file_with_the_marker_gives_the_parts_it_does_not_name_with_the_dock() {
        // the marker says a build with the one switch has read this file: an absent part is the new default
        let s = Settings::parse_migrated("dock_enabled = true\ndock_one_switch = true\n").unwrap();
        assert_eq!(run_flags(&s), [true; 5]);
        assert!(held_off_line(&s).is_none());
    }

    #[test]
    fn a_file_with_the_dock_off_gives_the_parts_when_the_user_turns_the_one_switch_on() {
        let s = Settings::parse_migrated("theme = \"dark\"\n").unwrap();
        assert_eq!(run_flags(&s), [false; 5]);
        // (the user turning the switch on: the file then has the dock on AND the marker, since the first save wrote it)
        let on = Settings { dock_enabled: true, ..s };
        assert_eq!(run_flags(&on), [true; 5]);
        let saved = toml::to_string_pretty(&on).unwrap();
        assert_eq!(run_flags(&Settings::parse_migrated(&saved).unwrap()), [true; 5]);
    }

    #[test]
    fn the_hold_survives_the_first_save_because_the_save_writes_the_held_values_with_the_marker() {
        let text = "dock_enabled = true\n";
        // what a save works out: the keys whose value differs between what the file gave (the baseline, before the migration) and what
        // the process holds after it
        let baseline = toml::Table::try_from(Settings::parse_lenient_pub(text).unwrap()).unwrap();
        let mine = toml::Table::try_from(Settings::parse_migrated(text).unwrap()).unwrap();
        // (`adaptive_offered` is another migration's own change, not part of this story)
        let mut changed: Vec<&str> = mine.iter().filter(|(k, v)| baseline.get(k.as_str()) != Some(*v) && k.as_str() != "adaptive_offered").map(|(k, _)| k.as_str()).collect();
        changed.sort_unstable();
        let mut expect: Vec<&str> = Part::ALL.iter().map(|p| p.field()).chain(["dock_one_switch"]).collect();
        expect.sort_unstable();
        assert_eq!(changed, expect);
        // and the file after that save (the old text with those keys put on top) still holds every part off
        let mut file = toml::from_str::<toml::Table>(text).unwrap();
        for k in changed {
            file.insert(k.to_string(), mine[k].clone());
        }
        let again = Settings::parse_migrated(&toml::to_string(&file).unwrap()).unwrap();
        assert_eq!(run_flags(&again), [false; 5]);
        assert_eq!(held_off(&again), Part::ALL.to_vec());
    }

    #[test]
    fn the_marker_and_the_transient_list_are_not_confused_with_a_part() {
        assert_eq!(Part::from_field("shell_win_key"), Some(Part::WinKey));
        assert_eq!(Part::from_field("dock_one_switch"), None);
        assert_eq!(Part::from_field("dock_enabled"), None);
        // the list of unnamed parts is never written to the file
        let s = Settings::parse_lenient_pub("dock_enabled = true\n").unwrap();
        assert_eq!(s.parts_unnamed_in_old_file.len(), 5);
        assert!(!toml::to_string(&s).unwrap().contains("parts_unnamed"));
    }

    #[test]
    fn a_file_that_names_a_part_as_true_keeps_it_and_one_that_names_it_false_holds_it_off() {
        let s = Settings::parse_migrated("dock_enabled = true\nshell_hide_taskbar = true\nshell_win_key = false\nshell_own_switcher = false\nshell_tray_host = false\nshell_desktop_cover = false\n").unwrap();
        assert_eq!(run_flags(&s), [false, true, false, false, false]);
        assert_eq!(held_off(&s), [Part::Desktop, Part::WinKey, Part::Switcher, Part::TrayHost]);
    }

    #[test]
    fn the_file_an_older_build_saved_with_the_old_defaults_holds_every_part_off_and_nobody_is_given_a_hook_by_an_update() {
        // what the old Settings::default() wrote: every part false, the dock on because the user had turned it on
        let text = "dock_enabled = true\nshell_desktop_cover = false\nshell_win_key = false\nshell_own_switcher = false\nshell_tray_host = false\nshell_hide_taskbar = false\n";
        let s = Settings::parse_migrated(text).unwrap();
        assert!(s.dock_enabled, "the dock stays on");
        assert_eq!(run_flags(&s), [false; 5], "no hook, no hidden taskbar, no cover, no tray host for a file that says false");
        assert_eq!(held_off(&s), Part::ALL.to_vec());
        while_turkish(|| {
            let line = held_off_line(&s).expect("the page says so");
            for p in Part::ALL {
                assert!(line.contains(p.label()), "{line} lacks {p:?}");
            }
        });
    }

    #[test]
    fn a_wrong_value_in_the_file_holds_only_that_part_off_and_the_page_lists_it() {
        // (a part's own value that cannot be read is a false: the default of these is true, and a typo must not install a keyboard hook)
        let s = Settings::parse_migrated("dock_enabled = true\ndock_one_switch = true\nshell_win_key = \"evet\"\nshell_hide_taskbar = false\nshell_own_switcher = 3\n").unwrap();
        assert_eq!(run_flags(&s), [true, false, false, false, true]);
        assert_eq!(held_off(&s), [Part::Taskbar, Part::WinKey, Part::Switcher]);
        // a wrong value of a setting that is not a part's own is still the default
        assert!(!Settings::parse_lenient_pub("dock_one_switch = 3\n").unwrap().dock_one_switch);
        // (a value that is not even TOML makes the whole file unreadable, which is the old behaviour: the defaults, the dock off)
        assert!(Settings::parse_migrated("dock_enabled = true\nshell_tray_host = ture\n").is_err());
    }

    #[test]
    fn a_wrong_dock_value_means_the_dock_is_off_and_with_it_every_part() {
        let s = Settings::parse_lenient_pub("dock_enabled = \"evet\"\nshell_win_key = true\n").unwrap();
        assert!(!s.dock_enabled);
        assert_eq!(run_flags(&s), [false; 5]);
    }

    // ---- the page's line and its button

    #[test]
    fn the_page_lists_the_parts_held_off_only_while_the_switch_is_on_and_only_when_there_are_any() {
        while_turkish(|| {
            assert!(held_off_line(&on()).is_none(), "none held off");
            let held = Settings { dock_enabled: true, shell_win_key: false, shell_own_switcher: false, ..Default::default() };
            let line = held_off_line(&held).unwrap();
            assert!(line.contains("Windows tuşu") && line.contains("Alt+Tab"), "{line}");
            assert!(!line.contains("masaüstü") && !line.contains("tepsi") && !line.contains("görev çubuğunu"), "only the held ones: {line}");
            let off = Settings { dock_enabled: false, ..held };
            assert!(held_off_line(&off).is_none(), "the switch is off: the line is for the moment it is turned on");
        });
    }

    #[test]
    fn the_button_clears_every_override_and_touches_nothing_else() {
        let mut s = Settings { dock_enabled: true, shell_win_key: false, shell_hide_taskbar: false, shell_tray_host: false, dock_reserve: false, show_hidden: true, ..Default::default() };
        let before = serde_json::to_value(&s).unwrap();
        clear_overrides(&mut s);
        assert!(held_off(&s).is_empty());
        assert_eq!(run_flags(&s), [true; 5]);
        let after = serde_json::to_value(&s).unwrap();
        let changed: Vec<_> = after.as_object().unwrap().iter().filter(|(k, v)| before.get(k.as_str()) != Some(*v)).map(|(k, _)| k.as_str().to_string()).collect();
        let mut expect = vec!["shell_win_key", "shell_hide_taskbar", "shell_tray_host"];
        expect.sort_unstable();
        let mut changed = changed;
        changed.sort_unstable();
        assert_eq!(changed, expect, "the button clears the three overrides that were set and nothing else");
        assert!(s.dock_enabled && !s.dock_reserve && s.show_hidden);
        // and it does not turn the dock on
        let mut off = Settings { dock_enabled: false, shell_win_key: false, ..Default::default() };
        clear_overrides(&mut off);
        assert!(!off.dock_enabled);
    }

    // ---- the texts

    #[test]
    fn the_switch_is_found_by_the_old_names() {
        while_turkish(|| {
            let hay = rf_core::entry::fold_case(&format!("{} {}", switch_title(), switch_description()));
            for word in ["masaüstü", "görev çubuğu", "tepsi", "Windows tuşu", "Alt+Tab", "Başlat", "arama", "denetim merkezi"] {
                assert!(hay.contains(&rf_core::entry::fold_case(word)), "the one switch's texts do not contain `{word}`");
            }
            assert!(switch_title().contains("(deneysel)"));
            assert!(switch_description().lines().count() >= 4 && switch_description().lines().all(|l| l.chars().count() < 110), "a few short lines");
        });
    }

    #[test]
    fn the_warnings_of_the_risky_parts_are_kept() {
        while_turkish(|| {
            assert!(keyboard_warning().contains("başka hiçbir tuşu saklamaz, kaydetmez ya da başka yere göndermez"));
            assert!(keyboard_warning().contains("rustyfiles durursa ya da takılırsa Windows eski davranışına döner"));
            assert!(taskbar_warning().contains("Dock durunca geri gelir"));
            assert!(taskbar_warning().contains("rustyfiles-shellguard.exe"));
            assert!(taskbar_warning().contains("yardımcı çalışmıyorsa görev çubuğu hiç gizlenmez"));
        });
    }

    #[test]
    fn every_text_has_its_english_pair() {
        while_turkish(|| {
            let en: std::collections::HashSet<&str> = crate::i18n_en::EN.iter().map(|(k, _)| *k).collect();
            for text in [switch_title(), switch_description(), keyboard_warning(), taskbar_warning(), desktop_comes_with_the_dock(), clear_button()] {
                assert!(en.contains(text), "no English for {text}");
            }
            for p in Part::ALL {
                assert!(en.contains(p.label()), "{p:?}");
            }
            for w in [Why::GuardMissing, Why::GuardNotConfirmed, Why::GuardLost, Why::RecordNotSaved, Why::HookRefused, Why::NotStarted] {
                for p in Part::ALL {
                    let line = w.line(p);
                    // (the two halves are separate keys)
                    let (first, _) = line.rsplit_once(". ").unwrap();
                    assert!(en.contains(format!("{first}.").as_str()), "no English for {first}");
                }
            }
            assert!(en.contains("Diğer parçalar çalışmaya devam ediyor."));
            assert!(en.contains("Ayar dosyası şunları kapalı tutuyor, bu yüzden çalışmıyor:"));
        });
    }

    #[test]
    fn a_failure_line_names_the_fault_and_says_the_rest_keep_running() {
        while_turkish(|| {
            let line = Why::GuardMissing.line(Part::Taskbar);
            assert!(line.contains("rustyfiles-shellguard.exe") && line.ends_with("Diğer parçalar çalışmaya devam ediyor."), "{line}");
            assert!(Why::HookRefused.line(Part::WinKey).contains("klavye kancasını"));
            // the Windows key and Alt+Tab share one hook: one fault, said once
            let lines = failure_lines(&[(Part::WinKey, Why::HookRefused), (Part::Switcher, Why::HookRefused), (Part::Taskbar, Why::GuardMissing)]);
            assert_eq!(lines.len(), 2, "{lines:?}");
            assert!(failure_lines(&[]).is_empty());
        });
    }

    #[test]
    fn the_tokens_round_trip() {
        for p in Part::ALL {
            assert_eq!(Part::from_token(p.token()), Some(p));
        }
        for w in [Why::GuardMissing, Why::GuardNotConfirmed, Why::GuardLost, Why::RecordNotSaved, Why::HookRefused, Why::NotStarted] {
            assert_eq!(Why::from_token(w.token()), Some(w));
        }
        assert_eq!(Part::from_token("nope"), None);
        assert_eq!(Why::from_token(""), None);
    }
}
