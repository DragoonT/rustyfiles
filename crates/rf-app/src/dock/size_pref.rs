//! The dock's size as the settings file keeps it ("Dock boyutu": small, medium, large, extra large). The numbers and the arithmetic are
//! `rf_win::dock::size`; this is only the value that is saved and the names the settings page shows.

use rf_win::dock::size::DockSize;
use serde::{Deserialize, Serialize};

/// Settings -> Dock: how big the whole dock is. Medium is the dock as it always was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DockSizePref {
    /// About 0.85 of the default.
    Small,
    /// The default: every length as it always was.
    #[default]
    Medium,
    /// About 1.2 times.
    Large,
    /// About 1.4 times.
    #[serde(rename = "xlarge")]
    XLarge,
}

impl DockSizePref {
    /// The size the layout works with.
    pub fn size(self) -> DockSize {
        match self {
            DockSizePref::Small => DockSize::Small,
            DockSizePref::Medium => DockSize::Medium,
            DockSizePref::Large => DockSize::Large,
            DockSizePref::XLarge => DockSize::XLarge,
        }
    }

    /// The factor that multiplies every length of the dock.
    pub fn factor(self) -> f32 {
        self.size().factor()
    }
}

#[cfg(test)]
mod tests {
    use super::DockSizePref;
    use crate::settings::Settings;

    #[test]
    fn the_dock_is_medium_by_default_and_a_file_from_before_has_no_size() {
        assert_eq!(Settings::default().dock_size, DockSizePref::Medium);
        assert_eq!(Settings::parse_lenient_pub("theme = \"dark\"\n").unwrap().dock_size, DockSizePref::Medium);
        assert_eq!(DockSizePref::default().factor(), 1.0);
    }

    #[test]
    fn every_size_round_trips_and_has_its_factor() {
        for (pref, name, factor) in [(DockSizePref::Small, "small", 0.85), (DockSizePref::Medium, "medium", 1.0), (DockSizePref::Large, "large", 1.2), (DockSizePref::XLarge, "xlarge", 1.4)] {
            assert_eq!(pref.factor(), factor);
            let text = toml::to_string_pretty(&Settings { dock_size: pref, ..Default::default() }).unwrap();
            assert!(text.contains(&format!("dock_size = \"{name}\"")), "{text}");
            assert_eq!(Settings::parse_lenient_pub(&text).unwrap().dock_size, pref);
        }
    }

    #[test]
    fn a_size_this_version_does_not_know_falls_back_and_costs_nothing_else() {
        let s = Settings::parse_lenient_pub("dock_size = \"gigantic\"\nshow_hidden = true\n").unwrap();
        assert_eq!(s.dock_size, DockSizePref::Medium);
        assert!(s.show_hidden);
    }
}
