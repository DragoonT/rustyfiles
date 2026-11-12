//! The condition in words under the dock's temperature ("Çok bulutlu", as Windows' own taskbar shows it): one text for every group of
//! WMO codes `rf_win::weather` knows, Turkish with an English pair. A code outside every group has no text (the picture and the
//! temperature are shown, as before).

use rf_win::weather::{Condition, condition};

use crate::t;

/// The words for a condition group; `None` for a code that belongs to none.
pub fn condition_label(c: Condition) -> Option<&'static str> {
    Some(match c {
        // ("Açık" is also the word for a switch that is on, so its English is given here and not through the shared table)
        Condition::Clear => crate::i18n::tx("Açık", "Clear"),
        Condition::MainlyClear => t!("Çoğunlukla açık"),
        Condition::PartlyCloudy => t!("Parçalı bulutlu"),
        Condition::Overcast => t!("Çok bulutlu"),
        Condition::Fog => t!("Sisli"),
        Condition::Drizzle => t!("Çisenti"),
        Condition::Rain => t!("Yağmurlu"),
        Condition::FreezingRain => t!("Buzlu yağmur"),
        Condition::RainShowers => t!("Sağanak"),
        Condition::Snow => t!("Karlı"),
        Condition::SnowGrains => t!("Kar taneleri"),
        Condition::SnowShowers => t!("Kar sağanağı"),
        Condition::Thunderstorm => t!("Gök gürültülü"),
        Condition::ThunderstormHail => t!("Dolulu fırtına"),
        Condition::Unknown => return None,
    })
}

/// The words for a WMO weather code.
pub fn code_label(code: u8) -> Option<&'static str> {
    condition_label(condition(code))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUPS: [Condition; 14] = [
        Condition::Clear,
        Condition::MainlyClear,
        Condition::PartlyCloudy,
        Condition::Overcast,
        Condition::Fog,
        Condition::Drizzle,
        Condition::Rain,
        Condition::FreezingRain,
        Condition::RainShowers,
        Condition::Snow,
        Condition::SnowGrains,
        Condition::SnowShowers,
        Condition::Thunderstorm,
        Condition::ThunderstormHail,
    ];

    #[test]
    fn every_group_has_its_own_words() {
        let labels: Vec<&str> = GROUPS.iter().map(|c| condition_label(*c).unwrap_or_else(|| panic!("{c:?} has no text"))).collect();
        assert!(labels.iter().all(|l| !l.trim().is_empty() && l.chars().count() <= 24), "{labels:?}");
        let distinct: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(distinct.len(), GROUPS.len(), "two groups share a text: {labels:?}");
        assert_eq!(condition_label(Condition::Unknown), None);
    }

    /// Every WMO code Open-Meteo documents has words, and a code nobody knows has none.
    #[test]
    fn every_documented_code_has_words() {
        for code in [0u8, 1, 2, 3, 45, 48, 51, 53, 55, 56, 57, 61, 63, 65, 66, 67, 71, 73, 75, 77, 80, 81, 82, 85, 86, 95, 96, 99] {
            assert!(code_label(code).is_some(), "code {code}");
        }
        for code in 0..=255u8 {
            assert_eq!(code_label(code).is_some(), condition(code) != Condition::Unknown, "code {code}");
        }
        for code in [4u8, 10, 44, 100, 255] {
            assert_eq!(code_label(code), None, "code {code}");
        }
        // codes of one group read alike
        assert_eq!(code_label(61), code_label(65));
        assert_eq!(code_label(45), code_label(48));
        assert_ne!(code_label(61), code_label(80), "rain and showers differ");
    }
}
