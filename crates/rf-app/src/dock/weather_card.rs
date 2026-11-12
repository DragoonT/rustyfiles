//! Settings' cards for the dock's weather from the PC's position: the switch "Konumumu kullan" with the one sentence that says what
//! happens, and, when the switch is on and Windows refuses location to desktop apps, one line saying so with a button that opens
//! Windows' location page. Drawn next to the weather city's row (`settings_view::dock_cards`); the rest of the Settings page does not
//! know about it.
//!
//! Nothing here asks Windows for the position (that would be a consent prompt on the user's screen): the line is from the privacy
//! switches in the registry, read-only (`rf_win::weather_location::windows_permission_cached`).

use gpui_kit::assets::IconName as L;
use gpui_kit::*;
use rf_win::weather_location::{Permission, SETTINGS_PAGE};

use crate::settings::{self, Settings};
use crate::settings_view::{button, card, switch};
use crate::t;
use crate::theme::Palette;

/// Whether the "location is off in Windows" line is shown: only for a switch that is on and a refusal that is known.
pub fn refusal_line_shown(switch_on: bool, permission: Permission) -> bool {
    switch_on && permission == Permission::Denied
}

/// The cards: the switch, and the refusal line after it when it applies.
pub fn cards(p: &Palette, s: &Settings) -> Vec<AnyElement> {
    let on = s.dock_weather_location;
    let mut out = vec![card(
        p,
        L::MapPin,
        t!("Konumumu kullan"),
        Some(t!("Bilgisayarın yaklaşık konumu (yaklaşık bir kilometreye yuvarlanmış) Windows'tan istenir ve hava durumunu almak için Open-Meteo'ya gönderilir; başka hiçbir yere gönderilmez.")),
        switch("dock-weather-location", p, on, move |_, cx| settings::update(cx, |s| s.dock_weather_location = !on)),
    )];
    if refusal_line_shown(on, rf_win::weather_location::windows_permission_cached()) {
        out.push(card(
            p,
            L::TriangleAlert,
            t!("Konum Windows'ta kapalı ya da masaüstü uygulamalara izin verilmemiş; şehir adı yazılmışsa o kullanılır."),
            None,
            button("dock-weather-location-open", p, t!("Konum ayarlarını aç"), |_, _| {
                std::thread::spawn(|| {
                    let _ = rf_win::shell::open(std::path::Path::new(SETTINGS_PAGE));
                });
            }),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the toolkit's glob brings its own `test` attribute along)
    use super::{Permission, refusal_line_shown};

    #[test]
    fn the_refusal_line_needs_the_switch_on_and_a_known_refusal() {
        assert!(refusal_line_shown(true, Permission::Denied));
        assert!(!refusal_line_shown(true, Permission::Allowed));
        assert!(!refusal_line_shown(true, Permission::Unknown), "an unreadable setting is not a refusal");
        assert!(!refusal_line_shown(false, Permission::Denied), "nothing is said to a user who did not ask");
    }
}
