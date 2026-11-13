//! Settings -> Masaüstü (deneysel): the settings of the desktop that rustyfiles draws itself, on a page of their own, built the way the
//! Dock page is (`dock_page.rs`): ONE list, [`entries`], that the page draws and the Settings search reads, so the two cannot drift
//! apart, and plain data that is tested without a window.
//!
//! WHAT IS ON THE PAGE: "show desktop icons", and one choice for each of the five
//! system icons (This PC, the user's folder, Network, Recycle Bin, Control Panel): follow Windows (the default: exactly what Windows'
//! own setting says), show, or hide. Windows' settings are only read, never written; the choices are this app's own.
//!
//! THE DESKTOP HAS NO SWITCH OF ITS OWN. It comes with the dock (the one switch on the Dock page, `dock/parts.rs`): a dock over
//! Explorer's desktop is two desktops. The page says so in one line.
//!
//! WHAT IS NOT, AND WHY: the icon size, "align to grid", auto arrange and the sort. They are the desktop's layout (`desktop-layout.json`),
//! which the process that draws the desktop owns and writes; this page lives in the main app, and a second writer of that file would
//! lose what the first one wrote. They are on the desktop's own right-click menu, and the page says so.

use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_core::desktop::virtuals::{IconChoice, VirtualKind};

use crate::dock::parts::{self, Part};
use crate::settings::Settings;
use crate::settings_view::{card_body, dropdown, section, shell};
use crate::t;
use crate::theme::Palette;

/// The groups of the page, in the order they are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Section {
    General,
    Icons,
}

impl Section {
    pub(crate) const ALL: [Section; 2] = [Section::General, Section::Icons];

    pub(crate) fn title(self) -> &'static str {
        match self {
            Section::General => t!("GENEL"),
            Section::Icons => t!("SİMGELER"),
        }
    }
}

/// The control of an entry, with how it reads and writes its own field (the tests check that it touches that field and no other).
#[derive(Clone, Copy)]
pub(crate) enum Control {
    /// Follow Windows / show / hide.
    Choice(fn(&Settings) -> IconChoice, fn(&mut Settings, IconChoice)),
    /// A corner radius in logical px, one of [`RADII`].
    Radius(fn(&Settings) -> u8, fn(&mut Settings, u8)),
}

/// The radii the wallpaper's corners can have (0: square).
pub(crate) const RADII: [u8; 5] = [0, 8, 12, 16, 24];

/// The radii as the dropdown writes them.
pub(crate) fn radius_labels() -> Vec<(u8, String)> {
    RADII.iter().map(|&r| (r, if r == 0 { t!("Kapalı").to_string() } else { format!("{r} px") })).collect()
}

/// One setting of the page.
#[derive(Clone, Copy)]
pub(crate) struct Entry {
    pub id: &'static str,
    /// The field of `Settings` it changes (the tests compare these with the fields the struct really has).
    #[cfg_attr(not(test), allow(dead_code))]
    pub field: &'static str,
    pub section: Section,
    pub icon: L,
    pub title: &'static str,
    pub desc: &'static str,
    /// For the five system icons: which one.
    #[cfg_attr(not(test), allow(dead_code))]
    pub kind: Option<VirtualKind>,
    pub control: Control,
}

/// The three states of a choice, as the dropdown writes them.
pub(crate) fn choice_labels() -> Vec<(IconChoice, String)> {
    vec![
        (IconChoice::Follow, t!("Windows ayarını izle").into()),
        (IconChoice::Show, t!("Göster").into()),
        (IconChoice::Hide, t!("Gizle").into()),
    ]
}

/// The intro at the top of the page.
pub(crate) fn intro() -> &'static str {
    t!("Masaüstü, Explorer'ın masaüstünün üstüne rustyfiles'ın çizdiği masaüstüdür: duvar kâğıdınız, simgeleriniz, sürükle-bırak ve menüler. Deneyseldir ve varsayılan olarak kapalıdır.")
}

/// What is Explorer's (Windows') and what is ours.
pub(crate) fn whose_line() -> &'static str {
    t!("Windows'un ayarları yalnızca okunur, hiçbir zaman değiştirilmez. \"Windows ayarını izle\" seçiliyken simge Windows'un kendi ayarına göre görünür; \"Göster\" ve \"Gizle\" yalnızca rustyfiles masaüstünü etkiler, Explorer'ın masaüstüne dokunmaz.")
}

/// Where the settings that are not on this page are.
pub(crate) fn layout_line() -> &'static str {
    t!("Simge boyutu, ızgaraya hizalama, otomatik düzenleme ve sıralama masaüstünün yerleşimine aittir: masaüstüne sağ tıklayınca açılan menüden değiştirilir.")
}

/// The line under the icons while the desktop itself is off.
pub(crate) fn kept_line() -> &'static str {
    t!("rustyfiles masaüstü kapalı: seçimleriniz kaydedilir ve masaüstü açılınca uygulanır.")
}

/// Every setting of the page, in the order of the page.
pub(crate) fn entries() -> Vec<Entry> {
    use Section::*;
    #[allow(clippy::too_many_arguments)]
    fn e(id: &'static str, field: &'static str, section: Section, icon: L, title: &'static str, desc: &'static str, kind: Option<VirtualKind>, control: Control) -> Entry {
        Entry { id, field, section, icon, title, desc, kind, control }
    }
    vec![
        e(
            "desktop-show-icons",
            "desktop_show_icons",
            General,
            L::Eye,
            t!("Masaüstü simgelerini göster"),
            t!("\"Gizle\", dosyalar dahil bütün simgeleri saklar. Masaüstüne sağ tıklayınca açılan menüde de aynı anahtar vardır; ikisinden biri gizlediyse simgeler gizlidir."),
            None,
            Control::Choice(|s| s.desktop_show_icons, |s, v| s.desktop_show_icons = v),
        ),
        e(
            "desktop-corner-radius",
            "desktop_corner_radius",
            General,
            L::Square,
            t!("Duvar kâğıdının köşelerini yuvarla (deneysel)"),
            t!("Masaüstünün duvar kâğıdı yuvarlak köşelerle çizilir; köşelerin arkası siyah kalır. Büyütülmüş bir pencere köşeyi kapatır: etki yalnızca masaüstü görünürken vardır."),
            None,
            Control::Radius(|s| s.desktop_corner_radius, |s, v| s.desktop_corner_radius = v),
        ),
        e(
            "desktop-icon-this-pc",
            "desktop_icon_this_pc",
            Icons,
            L::Monitor,
            t!("Bu bilgisayar"),
            t!("Çift tıklayınca rustyfiles'ta açılır; sağ tık menüsünde Özellikler (Sistem) vardır."),
            Some(VirtualKind::ThisPc),
            Control::Choice(|s| s.desktop_icon_this_pc, |s, v| s.desktop_icon_this_pc = v),
        ),
        e(
            "desktop-icon-user-files",
            "desktop_icon_user_files",
            Icons,
            L::User,
            t!("Kullanıcı dosyaları"),
            t!("Kullanıcı klasörünüz (Belgeler, Resimler, İndirilenler ve diğerleri)."),
            Some(VirtualKind::UserFiles),
            Control::Choice(|s| s.desktop_icon_user_files, |s, v| s.desktop_icon_user_files = v),
        ),
        e(
            "desktop-icon-network",
            "desktop_icon_network",
            Icons,
            L::Network,
            t!("Ağ"),
            t!("Ağdaki bilgisayarlar ve paylaşımlar."),
            Some(VirtualKind::Network),
            Control::Choice(|s| s.desktop_icon_network, |s, v| s.desktop_icon_network = v),
        ),
        e(
            "desktop-icon-recycle-bin",
            "desktop_icon_recycle_bin",
            Icons,
            L::Trash,
            t!("Geri Dönüşüm Kutusu"),
            t!("Dolu ya da boş olduğunu resminden gösterir; sağ tık menüsünde kutuyu boşaltma vardır, üzerine bırakılan dosyalar geri dönüşüm kutusuna gider."),
            Some(VirtualKind::RecycleBin),
            Control::Choice(|s| s.desktop_icon_recycle_bin, |s, v| s.desktop_icon_recycle_bin = v),
        ),
        e(
            "desktop-icon-control-panel",
            "desktop_icon_control_panel",
            Icons,
            L::Settings,
            t!("Denetim Masası"),
            t!("Windows'un eski ayar sayfaları; Explorer'ın kendi penceresinde açılır."),
            Some(VirtualKind::ControlPanel),
            Control::Choice(|s| s.desktop_icon_control_panel, |s, v| s.desktop_icon_control_panel = v),
        ),
    ]
}

/// The entries of each section, in the order of the page.
pub(crate) fn grouped(all: &[Entry]) -> Vec<(Section, Vec<Entry>)> {
    Section::ALL.iter().map(|&sec| (sec, all.iter().copied().filter(|e| e.section == sec).collect::<Vec<_>>())).filter(|(_, v)| !v.is_empty()).collect()
}

// ---------------------------------------------------------------- the page

/// The page: the intro, the lines that say whose is whose, then the sections.
pub(crate) fn page(p: &Palette, s: &Settings) -> Vec<AnyElement> {
    let text = |t: &'static str| div().pb_1().text_sm().text_color(p.text_secondary).child(t).into_any_element();
    let mut out: Vec<AnyElement> = vec![text(intro()), text(parts::desktop_comes_with_the_dock()), text(whose_line()), text(layout_line())];
    for (sec, rows) in grouped(&entries()) {
        out.push(section(p, sec.title()));
        for e in rows {
            out.push(row(p, s, &e));
        }
        if sec == Section::Icons && !parts::runs(s, Part::Desktop) {
            out.push(div().pt_1().text_xs().text_color(p.text_secondary).child(kept_line()).into_any_element());
        }
    }
    out
}

fn row(p: &Palette, s: &Settings, e: &Entry) -> AnyElement {
    // (the choices are only settings: they are saved before the desktop runs, so the row is usable with the dock off)
    let control = match e.control {
        Control::Choice(get, set) => dropdown(e.id, p, get(s), choice_labels(), set),
        Control::Radius(get, set) => dropdown(e.id, p, get(s), radius_labels(), set),
    };
    // (the card notes its title and description for the Settings search)
    shell(p).child(card_body(p, e.icon, e.title.into(), Some(e.desc.into()), Some(control))).into_any_element()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use rf_core::desktop::virtuals::{IconChoice, VirtualKind};

    use super::{Control, Entry, Section, choice_labels, entries, grouped, intro, kept_line, layout_line, whose_line};
    use crate::dock::parts;
    use crate::dock_page::while_turkish;
    use crate::settings::Settings;

    #[test]
    fn ids_and_fields_are_unique() {
        let all = entries();
        let ids: HashSet<_> = all.iter().map(|e| e.id).collect();
        let fields: HashSet<_> = all.iter().map(|e| e.field).collect();
        assert_eq!(ids.len(), all.len(), "an element id is used twice");
        assert_eq!(fields.len(), all.len(), "a setting is on the page twice");
    }

    /// The `desktop_*` fields the settings struct has, read from its source, so a setting added later without a place on the page fails
    /// here. (The desktop's on/off is the dock's one switch: `shell_desktop_cover` is an override in the file, no control on any page.)
    fn struct_fields() -> HashSet<String> {
        include_str!("settings.rs")
            .lines()
            .filter_map(|l| {
                let rest = l.trim_start().strip_prefix("pub ")?;
                let name = rest.split(':').next()?.trim();
                let ours = name.starts_with("desktop_");
                (ours && rest.contains(':') && !rest.contains('(')).then(|| name.to_string())
            })
            .collect()
    }

    #[test]
    fn every_desktop_setting_is_on_the_page_and_nothing_else() {
        let on_page: HashSet<String> = entries().iter().map(|e| e.field.to_string()).collect();
        let in_struct = struct_fields();
        assert!(in_struct.len() >= 6, "the source scan found too little: {in_struct:?}");
        let missing: Vec<_> = in_struct.difference(&on_page).collect();
        let unknown: Vec<_> = on_page.difference(&in_struct).collect();
        assert!(missing.is_empty(), "settings with no place on the Masaüstü page: {missing:?}");
        assert!(unknown.is_empty(), "entries for a setting that does not exist: {unknown:?}");
    }

    #[test]
    fn nothing_of_the_desktop_is_left_on_the_dock_the_developer_or_the_advanced_page() {
        // (a read or a write of the field on any binding: `s.field`; the fields named in quotes in other tests are not uses)
        let others = [("settings_view.rs", include_str!("settings_view.rs")), ("dock_page.rs", include_str!("dock_page.rs"))];
        for e in entries() {
            let needle = format!(".{}", e.field);
            for (file, src) in others {
                let used = src.match_indices(&needle).any(|(at, _)| !src[at + needle.len()..].starts_with(|c: char| c.is_alphanumeric() || c == '_'));
                assert!(!used, "{} is still used by {file}", e.field);
            }
        }
    }

    #[test]
    fn the_sections_are_in_order_and_hold_what_the_page_promises() {
        let g = grouped(&entries());
        assert_eq!(g.iter().map(|(s, _)| *s).collect::<Vec<_>>(), Section::ALL.to_vec());
        let fields = |sec: Section| g.iter().find(|(s, _)| *s == sec).map(|(_, v)| v.iter().map(|e| e.field).collect::<Vec<_>>()).unwrap();
        assert_eq!(fields(Section::General), ["desktop_show_icons", "desktop_corner_radius"]);
        assert_eq!(
            fields(Section::Icons),
            ["desktop_icon_this_pc", "desktop_icon_user_files", "desktop_icon_network", "desktop_icon_recycle_bin", "desktop_icon_control_panel"]
        );
    }

    #[test]
    fn each_system_icon_has_one_entry_and_its_control_writes_that_icons_field() {
        let mut seen = Vec::new();
        for e in entries() {
            let Some(kind) = e.kind else { continue };
            seen.push(kind);
            let Control::Choice(get, set) = e.control else { panic!("{}: a system icon has a three-state choice", e.field) };
            let mut s = Settings::default();
            assert_eq!(get(&s), IconChoice::Follow, "{}: the default follows Windows", e.field);
            set(&mut s, IconChoice::Show);
            // it reached the choice the desktop reads for that kind, and no other kind's
            let c = crate::shell::desktop::choices::from_settings(&s);
            for k in VirtualKind::ALL {
                assert_eq!(c.of(k), if k == kind { IconChoice::Show } else { IconChoice::Follow }, "{} changed {k:?}", e.field);
            }
            assert_eq!(c.show_icons, IconChoice::Follow);
        }
        seen.sort();
        assert_eq!(seen, VirtualKind::ALL.to_vec(), "one entry for each of the five");
    }

    #[test]
    fn every_control_changes_its_own_field_and_no_other() {
        let before = serde_json::to_value(Settings::default()).unwrap();
        let before = before.as_object().unwrap();
        for e in entries() {
            let mut s = Settings::default();
            match e.control {
                Control::Choice(_, set) => set(&mut s, IconChoice::Hide),
                Control::Radius(_, set) => set(&mut s, 12),
            }
            let after = serde_json::to_value(&s).unwrap();
            let after = after.as_object().unwrap();
            let all: HashSet<&String> = before.keys().chain(after.keys()).collect();
            let changed: Vec<&String> = all.into_iter().filter(|k| before.get(*k) != after.get(*k)).collect();
            assert_eq!(changed.len(), 1, "{} changed {changed:?}", e.field);
            assert_eq!(changed[0], e.field, "the control of {} changed {}", e.field, changed[0]);
        }
    }

    #[test]
    fn the_defaults_leave_the_desktop_as_it_is() {
        let d = Settings::default();
        assert!(!parts::runs(&d, parts::Part::Desktop), "experimental: off with the dock, which is off until asked for");
        for e in entries() {
            match e.control {
                Control::Choice(get, _) => assert_eq!(get(&d), IconChoice::Follow, "{}", e.field),
                Control::Radius(get, _) => assert_eq!(get(&d), 0, "{}: square corners until asked", e.field),
            }
        }
        assert!(crate::shell::desktop::choices::from_settings(&d).is_default());
    }

    #[test]
    fn the_choices_are_written_as_lowercase_words_and_survive_a_save_and_a_wrong_word_costs_only_that_setting() {
        let s = Settings { desktop_icon_this_pc: IconChoice::Show, desktop_icon_recycle_bin: IconChoice::Hide, show_hidden: true, ..Default::default() };
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("desktop_icon_this_pc = \"show\"") && text.contains("desktop_icon_recycle_bin = \"hide\""), "{text}");
        let back = Settings::parse_lenient_pub(&text).unwrap();
        assert_eq!((back.desktop_icon_this_pc, back.desktop_icon_recycle_bin, back.desktop_icon_network), (IconChoice::Show, IconChoice::Hide, IconChoice::Follow));
        // a file from before these settings existed, and a wrong word
        let old = Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap();
        assert!(crate::shell::desktop::choices::from_settings(&old).is_default());
        let wrong = Settings::parse_lenient_pub("desktop_icon_network = \"maybe\"\nshow_hidden = true\n").unwrap();
        assert_eq!(wrong.desktop_icon_network, IconChoice::Follow);
        assert!(wrong.show_hidden, "the other settings are kept");
    }

    #[test]
    fn the_desktop_has_no_switch_of_its_own_and_every_choice_is_a_dropdown_that_works_with_the_dock_off() {
        // the on/off of the desktop is the dock's one switch (`dock/parts.rs`); what is left here are the desktop's own choices, saved
        // before the desktop runs
        for e in entries() {
            assert!(matches!(e.control, Control::Choice(..) | Control::Radius(..)), "{} is not a dropdown", e.field);
            assert_ne!(e.field, "shell_desktop_cover");
        }
        // the desktop runs with the dock and stops with it
        let on = Settings { dock_enabled: true, ..Default::default() };
        assert!(parts::runs(&on, parts::Part::Desktop));
        assert!(!parts::runs(&Settings { dock_enabled: false, ..on }, parts::Part::Desktop));
    }

    #[test]
    fn every_text_has_its_english_pair() {
        while_turkish(|| {
            let en: HashSet<&str> = crate::i18n_en::EN.iter().map(|(k, _)| *k).collect();
            let has = |text: &str| en.contains(text);
            for e in entries() {
                assert!(has(e.title), "no English for the title of {}", e.field);
                assert!(has(e.desc), "no English for the description of {}", e.field);
            }
            for sec in Section::ALL {
                assert!(has(sec.title()), "{sec:?}");
            }
            for text in [intro(), whose_line(), layout_line(), kept_line(), parts::desktop_comes_with_the_dock()] {
                assert!(has(text), "{text}");
            }
            for (_, l) in choice_labels() {
                assert!(has(&l), "{l}");
            }
        });
    }

    #[test]
    fn the_page_says_what_is_not_on_it_and_where_it_is() {
        while_turkish(|| {
            assert!(layout_line().contains("sağ tıklayınca"));
            for word in ["Simge boyutu", "ızgaraya hizalama", "otomatik düzenleme", "sıralama"] {
                assert!(layout_line().contains(word), "{word}");
            }
            assert!(whose_line().contains("hiçbir zaman değiştirilmez"));
        });
    }

    #[test]
    fn the_page_says_the_desktop_comes_with_the_dock_and_where_its_switch_is() {
        while_turkish(|| {
            let line = parts::desktop_comes_with_the_dock();
            assert!(line.contains("Dock'la birlikte gelir") && line.contains("Dock sayfasındaki Dock anahtarı"), "{line}");
            assert!(!line.contains("sayfanın başındaki"), "the Dock switch is not at the top of THIS page: {line}");
        });
    }

    #[test]
    fn every_entry_has_texts() {
        while_turkish(|| {
            for e in entries() {
                let Entry { id, title, desc, .. } = e;
                assert!(!id.is_empty() && !title.is_empty() && !desc.is_empty(), "{}", e.field);
            }
        });
    }
}
