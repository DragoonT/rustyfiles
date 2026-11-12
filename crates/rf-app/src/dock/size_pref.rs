//! The dock's size as the settings file keeps it ("Dock boyutu"): a factor from 0.75 to 1.5 that the slider on the Dock page sets in
//! steps of 0.05. The range, the old names and the arithmetic are `rf_win::dock::size_live`; this is the value that is saved.
//!
//! THE FILE. The setting is still called `dock_size`. It used to be one of four names (small, medium, large, xlarge); it is now a number
//! (`dock_size = 1.25`), and a number that is exactly one of the four old sizes is still written as its name, so a file this version
//! saves at the default is the file the old version wrote, and an older build still reads the four sizes it knew. Reading: a name loads
//! as the factor it meant, a number as a number cut to the range (zero, negative and huge ones too), and anything else (a name nobody
//! knows, NaN, a list) as the default.

use std::fmt;

use rf_win::dock::size_live as live;
use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Settings -> Dock: how big the whole dock is, in thousandths of the default (1000 is the dock as it always was).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockSizePref(u16);

impl Default for DockSizePref {
    fn default() -> Self {
        DockSizePref(live::DEFAULT)
    }
}

impl DockSizePref {
    /// A size from its thousandths, cut to the range.
    pub fn from_permille(permille: u16) -> DockSizePref {
        DockSizePref(live::clamp(permille))
    }

    /// A size from a number as a person or a file wrote it (1.25 is %125), cut to the range; NaN is the default.
    pub fn from_number(v: f64) -> DockSizePref {
        DockSizePref(live::from_number(v))
    }

    pub fn permille(self) -> u16 {
        self.0
    }

    /// The factor that multiplies every length of the dock.
    pub fn factor(self) -> f32 {
        live::factor(self.0)
    }

    /// The slider position (0..=15) the value is shown at.
    pub fn position(self) -> u8 {
        live::position_of(self.0)
    }

    /// The value at a slider position.
    pub fn at_position(position: u8) -> DockSizePref {
        DockSizePref(live::at_position(position))
    }

    /// "%100" or "100%".
    pub fn percent_text(self, turkish: bool) -> String {
        live::percent_text(self.0, turkish)
    }
}

impl Serialize for DockSizePref {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match live::legacy_name(self.0) {
            Some(name) => s.serialize_str(name),
            // (thousandths over 1000 in f64 is the shortest decimal for the value: 1230 -> 1.23)
            None => s.serialize_f64(f64::from(self.0) / 1000.0),
        }
    }
}

struct SizeVisitor;

impl Visitor<'_> for SizeVisitor {
    type Value = DockSizePref;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a number from 0.75 to 1.5, or small, medium, large or xlarge")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<DockSizePref, E> {
        // a name nobody knows is the default, as it was when the setting was an enum read leniently
        Ok(live::from_legacy_name(v).map_or_else(DockSizePref::default, DockSizePref))
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<DockSizePref, E> {
        Ok(DockSizePref::from_number(v))
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<DockSizePref, E> {
        Ok(DockSizePref::from_number(v as f64))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<DockSizePref, E> {
        Ok(DockSizePref::from_number(v as f64))
    }

    // (a list, a table, a boolean: not a size; the field falls back to the default like any value this version does not understand)
}

impl<'de> Deserialize<'de> for DockSizePref {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(SizeVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::DockSizePref;
    use crate::settings::Settings;

    fn read(line: &str) -> Settings {
        Settings::parse_lenient_pub(line).unwrap()
    }

    #[test]
    fn the_dock_is_100_percent_by_default_and_a_file_from_before_has_no_size() {
        assert_eq!(Settings::default().dock_size, DockSizePref::default());
        assert_eq!(read("theme = \"dark\"\n").dock_size.permille(), 1000);
        assert_eq!(DockSizePref::default().factor(), 1.0, "exactly today's Orta");
    }

    #[test]
    fn the_four_old_names_load_as_the_factor_they_meant_and_round_trip_as_names() {
        for (name, factor) in [("small", 0.85f32), ("medium", 1.0), ("large", 1.2), ("xlarge", 1.4)] {
            let s = read(&format!("dock_size = \"{name}\"\n"));
            assert_eq!(s.dock_size.factor(), factor, "{name}");
            let text = toml::to_string_pretty(&s).unwrap();
            assert!(text.contains(&format!("dock_size = \"{name}\"")), "{text}");
            assert_eq!(read(&text).dock_size, s.dock_size);
        }
        // the default saved is the line the old version wrote
        let text = toml::to_string_pretty(&Settings::default()).unwrap();
        assert!(text.contains("dock_size = \"medium\""), "{text}");
    }

    #[test]
    fn a_number_loads_as_a_number_and_is_saved_as_one() {
        for (text, permille) in [("1.25", 1250), ("0.75", 750), ("1.5", 1500), ("1.23", 1230), ("1.05", 1050), ("1.0", 1000), ("1", 1000)] {
            let s = read(&format!("dock_size = {text}\n"));
            assert_eq!(s.dock_size.permille(), permille, "{text}");
        }
        for permille in (750..=1500).step_by(5) {
            let s = Settings { dock_size: DockSizePref::from_permille(permille), ..Default::default() };
            let text = toml::to_string_pretty(&s).unwrap();
            assert_eq!(read(&text).dock_size.permille(), permille, "{text}");
            let named = matches!(permille, 850 | 1000 | 1200 | 1400);
            assert_eq!(text.contains("dock_size = \""), named, "{text}");
        }
        let text = toml::to_string_pretty(&Settings { dock_size: DockSizePref::from_permille(1230), ..Default::default() }).unwrap();
        assert!(text.contains("dock_size = 1.23\n"), "{text}");
    }

    #[test]
    fn a_number_outside_the_range_is_cut_to_it() {
        for (text, permille) in [("0", 750), ("0.0", 750), ("-2", 750), ("-0.5", 750), ("0.1", 750), ("1.51", 1500), ("2", 1500), ("100", 1500), ("1e300", 1500), ("inf", 1500), ("-inf", 750)] {
            assert_eq!(read(&format!("dock_size = {text}\n")).dock_size.permille(), permille, "{text}");
        }
    }

    #[test]
    fn nan_and_anything_that_is_not_a_size_load_as_the_default_and_cost_nothing_else() {
        for text in ["nan", "+nan", "-nan", "\"gigantic\"", "\"\"", "\"1.25\"", "true", "[1.2]", "{ a = 1 }"] {
            let s = read(&format!("dock_size = {text}\nshow_hidden = true\n"));
            assert_eq!(s.dock_size.permille(), 1000, "{text}");
            assert!(s.show_hidden, "{text}");
        }
    }

    #[test]
    fn the_value_at_a_slider_position_and_back() {
        for p in 0..=15u8 {
            let v = DockSizePref::at_position(p);
            assert_eq!(v.position(), p);
            assert_eq!(v.permille(), 750 + 50 * u16::from(p));
        }
        assert_eq!(DockSizePref::at_position(5), DockSizePref::default());
        assert_eq!(DockSizePref::from_permille(1230).position(), 10);
    }

    #[test]
    fn the_percent_text_follows_the_value_in_both_languages() {
        let v = DockSizePref::default();
        assert_eq!(v.percent_text(true), "%100");
        assert_eq!(v.percent_text(false), "100%");
        assert_eq!(DockSizePref::from_permille(1250).percent_text(true), "%125");
    }
}
