//! The dock's weather without a typed city: the PC's approximate position from Windows' location service, and the order in which the
//! weather's sources are tried.
//!
//! THE ORDER (this is the contract, the pure [`choose_route`] holds it and its tests pin it):
//! 1. the switch is on, Windows has not refused location to desktop apps, and a position came back: the forecast for that position;
//! 2. otherwise (the switch is off, or Windows refused, or no position could be had in time): the typed city, if there is one;
//! 3. otherwise nothing: no weather, and no request of any kind.
//!
//! A refusal is never retried in a loop: the dock asks again only when the weather is refreshed anyway (every 30 minutes, or after the
//! back-off for a failure, see `dock/plan.rs`), and a user who turns the switch off and on asks at once.
//!
//! PRIVACY. The coordinates are cut to two decimals (about a kilometre) the moment they come from Windows, in [`Position::from_degrees`]:
//! the exact ones never travel further than that function. The rounded pair goes into the forecast address (Open-Meteo, the same
//! host as for a typed city) and nowhere else: it is not written to the settings file, not logged, and no place name is looked up (no
//! reverse geocoding, no IP-based guess). Nothing here asks the network when the switch is off and the city is empty.
//!
//! PERMISSION. Windows has two switches in Settings > Privacy > Location: "Location services" and "Let desktop apps access your
//! location". [`windows_permission`] reads them from the registry (read only; nothing is written, nothing is shown) so that Settings can
//! say "off in Windows" and so that the dock does not even ask when it is known to be refused. If the answer is unknown the dock asks
//! Windows, and Windows' own answer (access denied) counts as a refusal too.
//!
//! Everything that blocks (the Windows request, the forecast) must run on a worker thread. Tests use a fake [`PositionSource`]; none
//! of them asks Windows for a position or touches the network.

use std::time::Duration;

use crate::weather::{self, Outcome, WeatherFeed};

/// Windows' own limit for one position request.
pub const LOCATE_TIMEOUT: Duration = Duration::from_secs(10);
/// A position Windows computed this recently may be reused (saves waking the radio and sensors at every refresh).
pub const MAX_POSITION_AGE: Duration = Duration::from_secs(15 * 60);

/// A place on Earth to the nearest hundredth of a degree (about 1.1 km), kept as whole hundredths so that two positions are the same
/// place exactly when their numbers are equal (no float comparison).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Position {
    lat_centi: i32,
    lon_centi: i32,
}

impl Position {
    /// Rounds degrees to two decimals. `None` for numbers that are not coordinates (not finite, latitude over 90, longitude over 180).
    pub fn from_degrees(latitude: f64, longitude: f64) -> Option<Position> {
        if !latitude.is_finite() || !longitude.is_finite() || latitude.abs() > 90.0 || longitude.abs() > 180.0 {
            return None;
        }
        Some(Position { lat_centi: (latitude * 100.0).round() as i32, lon_centi: (longitude * 100.0).round() as i32 })
    }

    pub fn latitude(&self) -> f64 {
        f64::from(self.lat_centi) / 100.0
    }

    pub fn longitude(&self) -> f64 {
        f64::from(self.lon_centi) / 100.0
    }
}

/// Whether the place is another one than the one held (nothing held: it is new).
pub fn place_changed(held: Option<Position>, now: Position) -> bool {
    held != Some(now)
}

/// The forecast request for a position: the weather module's own address with the rounded numbers (`39.7500`: two decimals and zeros).
pub fn forecast_url_for(p: Position) -> String {
    weather::forecast_url(p.latitude(), p.longitude())
}

/// What Windows' privacy settings say about location for desktop apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Permission {
    /// Windows' settings allow it.
    Allowed,
    /// Location services are off for this user or this PC, or desktop apps are not allowed to use it.
    Denied,
    /// The settings could not be read (a value is missing): ask Windows and take its answer.
    Unknown,
}

/// The privacy switches from their registry values (`"Allow"` / `"Deny"`): the user's "Location services", the PC's, and "Let desktop
/// apps access your location". Any "Deny" wins; "Allow" needs the user's own value to say so.
pub fn permission_from_values(user: Option<&str>, machine: Option<&str>, desktop_apps: Option<&str>) -> Permission {
    let is = |v: Option<&str>, word: &str| v.is_some_and(|s| s.trim().eq_ignore_ascii_case(word));
    if is(user, "Deny") || is(machine, "Deny") || is(desktop_apps, "Deny") {
        Permission::Denied
    } else if is(user, "Allow") {
        Permission::Allowed
    } else {
        Permission::Unknown
    }
}

const CONSENT: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\location";

fn consent_value(root: &windows_registry::Key, sub: &str) -> Option<String> {
    let path = if sub.is_empty() { CONSENT.to_string() } else { format!(r"{CONSENT}\{sub}") };
    root.open(path).ok()?.get_string("Value").ok()
}

/// Reads Windows' location privacy switches. Read only: a few registry values, no prompt, no window, nothing written.
pub fn windows_permission() -> Permission {
    let user = consent_value(windows_registry::CURRENT_USER, "");
    let machine = consent_value(windows_registry::LOCAL_MACHINE, "");
    let desktop = consent_value(windows_registry::CURRENT_USER, "NonPackaged");
    permission_from_values(user.as_deref(), machine.as_deref(), desktop.as_deref())
}

/// How long a read of the privacy switches is reused by [`windows_permission_cached`].
const PERMISSION_FRESH: Duration = Duration::from_secs(3);

/// The memory behind [`windows_permission_cached`], apart from the clock and the registry so that a test can drive it: what was read
/// last and whether a read is already under way.
#[derive(Default)]
struct PermissionCache {
    last: Option<(std::time::Instant, Permission)>,
    reading: bool,
}

impl PermissionCache {
    /// What to answer right now, and whether the caller must start a read in the background. Never waits: a screen that draws often
    /// asks this on the UI thread. Before the first read finishes the answer is [`Permission::Unknown`] (no line is shown); an old
    /// value is answered while the new one is read; only one read runs at a time.
    fn ask(&mut self, now: std::time::Instant) -> (Permission, bool) {
        let answer = self.last.map_or(Permission::Unknown, |(_, p)| p);
        let fresh = self.last.is_some_and(|(at, _)| now.saturating_duration_since(at) < PERMISSION_FRESH);
        let start = !fresh && !self.reading;
        if start {
            self.reading = true;
        }
        (answer, start)
    }

    fn done(&mut self, now: std::time::Instant, p: Permission) {
        self.last = Some((now, p));
        self.reading = false;
    }
}

/// [`windows_permission`] for a screen that draws often (Settings), without ever touching the registry on the caller's thread: the
/// last answer comes back at once and a worker thread reads the switches again when that answer is more than a few seconds old.
pub fn windows_permission_cached() -> Permission {
    use std::sync::Mutex;
    use std::time::Instant;
    static CACHE: Mutex<Option<PermissionCache>> = Mutex::new(None);
    let (answer, start) = CACHE.lock().unwrap_or_else(|p| p.into_inner()).get_or_insert_with(PermissionCache::default).ask(Instant::now());
    if start {
        let spawned = std::thread::Builder::new().name("rf-location-consent".into()).spawn(|| {
            let p = windows_permission();
            CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(PermissionCache::default).done(Instant::now(), p);
        });
        if spawned.is_err() {
            CACHE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(PermissionCache::default).reading = false;
        }
    }
    answer
}

/// The page of Windows' Settings where location is switched on.
pub const SETTINGS_PAGE: &str = "ms-settings:privacy-location";

/// What asking for the position came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    Found(Position),
    /// Windows refused (access denied, or location services reported as disabled).
    Denied,
    /// No position in time, no sensor, an error.
    Unavailable,
}

/// Where the position comes from. The real one asks Windows; tests give a fake.
pub trait PositionSource {
    /// What Windows' settings say (cheap, no prompt).
    fn permission(&mut self) -> Permission;
    /// Asks for the position. Blocks, up to [`LOCATE_TIMEOUT`] and a little more.
    fn position(&mut self) -> Fix;
}

/// Why there is nothing to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// Neither a source switched on nor a city typed.
    NoSource,
    /// Windows refuses location and there is no city to fall back to.
    Denied,
    /// The switch is on, Windows did not refuse, but no position came and there is no city.
    NoPosition,
}

/// Where the weather comes from this time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Route {
    Location(Position),
    City,
    Nothing(Reason),
}

/// Whether Windows should be asked for the position at all: only with the switch on and no known refusal.
pub fn should_ask(switch: bool, permission: Permission) -> bool {
    switch && permission != Permission::Denied
}

/// THE ORDER of the module's doc comment. `fix` is `None` when [`should_ask`] said no (the position was not asked).
pub fn choose_route(switch: bool, city_given: bool, permission: Permission, fix: Option<Fix>) -> Route {
    if should_ask(switch, permission)
        && let Some(Fix::Found(p)) = fix
    {
        return Route::Location(p);
    }
    if city_given {
        return Route::City;
    }
    if !switch {
        return Route::Nothing(Reason::NoSource);
    }
    if permission == Permission::Denied || fix == Some(Fix::Denied) {
        Route::Nothing(Reason::Denied)
    } else {
        Route::Nothing(Reason::NoPosition)
    }
}

/// The dock's weather: a typed city and/or the PC's position. [`LocatedWeather::poll`] is one refresh; call it from a worker thread.
#[derive(Clone, Debug)]
pub struct LocatedWeather {
    feed: WeatherFeed,
    switch: bool,
    /// The place the last forecast was for (rounded), to tell a move from a repeat. Cleared when the city was used.
    held: Option<Position>,
}

impl LocatedWeather {
    pub fn new(city: &str, use_location: bool) -> LocatedWeather {
        LocatedWeather { feed: WeatherFeed::new(city), switch: use_location, held: None }
    }

    /// Whether a refresh could ask anything at all. With neither the switch nor a city it cannot, and makes no request.
    pub fn active(&self) -> bool {
        self.switch || self.feed.active()
    }

    /// One refresh with the given position source and HTTP (so a test can count what is asked). Never returns an error: the outcome says
    /// how it ended and so how soon to try again.
    pub fn refresh_with(&mut self, source: &mut dyn PositionSource, get: &mut dyn FnMut(&str) -> anyhow::Result<String>) -> Outcome {
        if !self.active() {
            return Outcome::UnknownPlace;
        }
        let permission = if self.switch { source.permission() } else { Permission::Unknown };
        let fix = should_ask(self.switch, permission).then(|| source.position());
        match choose_route(self.switch, self.feed.active(), permission, fix) {
            Route::Location(p) => {
                // The same rounded place as before needs no new decision about the place; the forecast itself is asked at every refresh,
                // because the weather moves even when the PC does not.
                if place_changed(self.held, p) {
                    log::debug!("weather: the place of the forecast changed");
                }
                self.held = Some(p);
                let result = get(&forecast_url_for(p)).and_then(|body| weather::parse_current(&body));
                match result {
                    Ok(o) => Outcome::Weather(o),
                    Err(e) => {
                        log::warn!("weather: {e:#}");
                        Outcome::Failed
                    }
                }
            }
            Route::City => {
                self.held = None;
                let result = self.feed.refresh_with(get);
                match &result {
                    Ok(None) => log::info!("weather: no place named {:?}", self.feed.city()),
                    Err(e) => log::warn!("weather: {e:#}"),
                    _ => {}
                }
                weather::outcome_of(&result)
            }
            Route::Nothing(Reason::NoSource) => Outcome::UnknownPlace,
            Route::Nothing(Reason::Denied) => {
                log::info!("weather: Windows does not let desktop apps use the location");
                Outcome::UnknownPlace
            }
            Route::Nothing(Reason::NoPosition) => {
                log::info!("weather: Windows gave no position");
                Outcome::Failed
            }
        }
    }

    /// One refresh over the network and Windows' location service. Blocks.
    pub fn poll(&mut self) -> Outcome {
        self.refresh_with(&mut WindowsLocator, &mut |u| weather::http_get(u, weather::TIMEOUT_MS))
    }
}

/// The real position source: Windows.Devices.Geolocation at its lowest accuracy ("Default": Wi-Fi / IP based, a few hundred metres to
/// kilometres, which is all a forecast needs).
pub struct WindowsLocator;

impl PositionSource for WindowsLocator {
    fn permission(&mut self) -> Permission {
        windows_permission()
    }

    fn position(&mut self) -> Fix {
        // On a thread of its own: the request is a WinRT operation that wants a multithreaded apartment, and the wait for it is bounded
        // here as well as by Windows' own timeout, so a service that never answers cannot hold the caller.
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("rf-locate".into()).spawn(move || {
            let _ = tx.send(ask_windows());
        });
        if spawned.is_err() {
            return Fix::Unavailable;
        }
        rx.recv_timeout(LOCATE_TIMEOUT + Duration::from_secs(5)).unwrap_or(Fix::Unavailable)
    }
}

fn ask_windows() -> Fix {
    use windows::Devices::Geolocation::{Geolocator, PositionAccuracy, PositionStatus};
    use windows::Foundation::TimeSpan;
    use windows::Win32::Foundation::E_ACCESSDENIED;
    crate::sysstatus::init_mta();
    let ticks = |d: Duration| TimeSpan { Duration: (d.as_nanos() / 100) as i64 };
    let Ok(locator) = Geolocator::new() else { return Fix::Unavailable };
    // the lowest accuracy that is offered
    let _ = locator.SetDesiredAccuracy(PositionAccuracy::Default);
    let answer = locator.GetGeopositionAsyncWithAgeAndTimeout(ticks(MAX_POSITION_AGE), ticks(LOCATE_TIMEOUT)).and_then(|op| op.join());
    match answer {
        Ok(position) => {
            let point = position.Coordinate().and_then(|c| c.Point()).and_then(|p| p.Position());
            match point.ok().and_then(|b| Position::from_degrees(b.Latitude, b.Longitude)) {
                Some(p) => Fix::Found(p),
                None => Fix::Unavailable,
            }
        }
        Err(e) if e.code() == E_ACCESSDENIED => Fix::Denied,
        Err(_) => {
            if locator.LocationStatus().is_ok_and(|s| s == PositionStatus::Disabled) {
                Fix::Denied
            } else {
                Fix::Unavailable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FORECAST_JSON: &str = r#"{"current":{"temperature_2m":12.3,"weather_code":3,"is_day":1}}"#;
    const GEO: &str = r#"{"results":[{"name":"Erzincan","latitude":39.75,"longitude":39.5}]}"#;

    /// A position source that never asks Windows: it says what it was told and counts how often it was asked.
    struct Fake {
        permission: Permission,
        fix: Fix,
        asked_permission: u32,
        asked_position: u32,
    }

    impl Fake {
        fn new(permission: Permission, fix: Fix) -> Fake {
            Fake { permission, fix, asked_permission: 0, asked_position: 0 }
        }
    }

    impl PositionSource for Fake {
        fn permission(&mut self) -> Permission {
            self.asked_permission += 1;
            self.permission
        }
        fn position(&mut self) -> Fix {
            self.asked_position += 1;
            self.fix
        }
    }

    fn pos(lat: f64, lon: f64) -> Position {
        Position::from_degrees(lat, lon).unwrap()
    }

    #[test]
    fn coordinates_are_cut_to_two_decimals() {
        let p = pos(39.754_321, 39.498_76);
        assert_eq!((p.latitude(), p.longitude()), (39.75, 39.5));
        let q = pos(-33.869_8, 151.209_9);
        assert_eq!((q.latitude(), q.longitude()), (-33.87, 151.21));
        // halves round away from zero, a tiny negative is not "-0"
        assert_eq!(pos(0.125, 0.0).latitude(), 0.13);
        assert_eq!(pos(-0.125, 0.0).latitude(), -0.13);
        assert_eq!(pos(-0.001, 0.0).latitude(), 0.0);
        assert!(pos(-0.001, 0.0).latitude().is_sign_positive());
        // the ends of the Earth
        assert_eq!(pos(90.0, 180.0).longitude(), 180.0);
        assert_eq!(pos(-90.0, -180.0).latitude(), -90.0);
    }

    #[test]
    fn numbers_that_are_not_coordinates_are_refused() {
        for (lat, lon) in [(f64::NAN, 0.0), (0.0, f64::NAN), (f64::INFINITY, 0.0), (0.0, f64::NEG_INFINITY), (90.01, 0.0), (0.0, 180.01), (-91.0, 0.0), (0.0, -181.0)] {
            assert_eq!(Position::from_degrees(lat, lon), None, "{lat} {lon}");
        }
    }

    #[test]
    fn the_place_changes_only_when_the_rounded_numbers_do() {
        let held = pos(39.7512, 39.4988);
        // a few hundred metres on is the same rounded place
        assert!(!place_changed(Some(held), pos(39.7549, 39.5011)));
        // a hundredth of a degree is another
        assert!(place_changed(Some(held), pos(39.7612, 39.4988)));
        assert!(place_changed(Some(held), pos(39.7512, 39.5112)));
        // nothing held: new
        assert!(place_changed(None, held));
    }

    #[test]
    fn the_forecast_address_carries_the_rounded_numbers_only() {
        let url = forecast_url_for(pos(39.754_321, 39.498_76));
        assert!(url.starts_with("https://api.open-meteo.com/v1/forecast?"), "{url}");
        assert!(url.contains("latitude=39.7500&longitude=39.5000&"), "{url}");
        // no digit beyond the second decimal is anything but a zero, for any input
        for (lat, lon) in [(12.345_678_9, -98.765_432_1), (0.000_4, 0.000_6), (-45.678_9, 170.123_4)] {
            let url = forecast_url_for(pos(lat, lon));
            for key in ["latitude=", "longitude="] {
                let value = url.split(key).nth(1).unwrap().split('&').next().unwrap();
                let decimals = value.split('.').nth(1).unwrap();
                assert_eq!(decimals.len(), 4, "{value}");
                assert!(decimals[2..].chars().all(|c| c == '0'), "{value} has more than two real decimals");
            }
        }
    }

    #[test]
    fn windows_privacy_values_mean_what_they_say() {
        use Permission::*;
        assert_eq!(permission_from_values(Some("Allow"), Some("Allow"), Some("Allow")), Allowed);
        assert_eq!(permission_from_values(Some("Allow"), None, None), Allowed);
        assert_eq!(permission_from_values(Some("Allow"), Some("Allow"), Some("Deny")), Denied, "desktop apps refused");
        assert_eq!(permission_from_values(Some("Deny"), Some("Allow"), Some("Allow")), Denied, "location services off for the user");
        assert_eq!(permission_from_values(Some("Allow"), Some("Deny"), Some("Allow")), Denied, "location services off for the PC");
        assert_eq!(permission_from_values(Some(" deny "), None, None), Denied, "case and spaces do not matter");
        // nothing readable: ask Windows
        assert_eq!(permission_from_values(None, None, None), Unknown);
        assert_eq!(permission_from_values(None, Some("Allow"), Some("Allow")), Unknown);
        assert_eq!(permission_from_values(Some("Prompt"), None, None), Unknown);
    }

    /// Every combination of (switch, city, permission, position) and what the order says it comes to.
    #[test]
    fn the_order_is_location_then_city_then_nothing() {
        let here = pos(39.75, 39.5);
        use Permission::*;
        let found = Some(Fix::Found(here));
        // switch on, allowed or unknown, position found: the position, with or without a city
        for permission in [Allowed, Unknown] {
            for city in [false, true] {
                assert_eq!(choose_route(true, city, permission, found), Route::Location(here), "{permission:?} city {city}");
            }
        }
        // switch on and the position cannot be had: the city if there is one
        for fix in [Some(Fix::Denied), Some(Fix::Unavailable)] {
            assert_eq!(choose_route(true, true, Allowed, fix), Route::City);
            assert_eq!(choose_route(true, true, Unknown, fix), Route::City);
        }
        // switch on, refused by the settings (the position is not even asked): the city
        assert_eq!(choose_route(true, true, Denied, None), Route::City);
        // and a position that came although the settings say no is not used
        assert_eq!(choose_route(true, true, Denied, found), Route::City);
        assert_eq!(choose_route(true, false, Denied, found), Route::Nothing(Reason::Denied));
        // switch off: the city, whatever the rest says
        for permission in [Allowed, Denied, Unknown] {
            assert_eq!(choose_route(false, true, permission, None), Route::City);
            assert_eq!(choose_route(false, true, permission, found), Route::City, "a position is never used with the switch off");
        }
        // nothing at all
        assert_eq!(choose_route(false, false, Allowed, None), Route::Nothing(Reason::NoSource));
        assert_eq!(choose_route(true, false, Denied, None), Route::Nothing(Reason::Denied));
        assert_eq!(choose_route(true, false, Allowed, Some(Fix::Denied)), Route::Nothing(Reason::Denied));
        assert_eq!(choose_route(true, false, Allowed, Some(Fix::Unavailable)), Route::Nothing(Reason::NoPosition));
        assert_eq!(choose_route(true, false, Unknown, Some(Fix::Unavailable)), Route::Nothing(Reason::NoPosition));
    }

    #[test]
    fn windows_is_asked_only_with_the_switch_on_and_no_known_refusal() {
        assert!(should_ask(true, Permission::Allowed));
        assert!(should_ask(true, Permission::Unknown));
        assert!(!should_ask(true, Permission::Denied));
        assert!(!should_ask(false, Permission::Allowed));
        assert!(!should_ask(false, Permission::Unknown));
    }

    fn http_for(urls: &mut Vec<String>) -> impl FnMut(&str) -> anyhow::Result<String> + '_ {
        move |u: &str| {
            urls.push(u.to_string());
            Ok(if u.contains("geocoding") { GEO.to_string() } else { FORECAST_JSON.to_string() })
        }
    }

    #[test]
    fn with_a_position_one_forecast_for_the_rounded_place_and_no_geocoding() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Allowed, Fix::Found(pos(39.754_321, 39.498_76)));
        let mut w = LocatedWeather::new("Ankara", true);
        let out = w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert!(matches!(out, Outcome::Weather(o) if o.code == 3), "{out:?}");
        assert_eq!(urls.len(), 1, "{urls:?}");
        assert!(urls[0].contains("latitude=39.7500&longitude=39.5000"), "{urls:?}");
        assert!(!urls.iter().any(|u| u.contains("geocoding") || u.contains("Ankara")), "the typed city is not sent when the position is used");
        assert_eq!((source.asked_permission, source.asked_position), (1, 1));
    }

    #[test]
    fn a_refresh_asks_for_the_position_again_and_the_forecast_again_for_the_same_place() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Allowed, Fix::Found(pos(39.75, 39.5)));
        let mut w = LocatedWeather::new("", true);
        w.refresh_with(&mut source, &mut http_for(&mut urls));
        w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert_eq!(source.asked_position, 2);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], urls[1]);
        // the PC moved a few streets: the same rounded place, the same address
        source.fix = Fix::Found(pos(39.7541, 39.4962));
        w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert_eq!(urls[2], urls[0]);
        // the PC moved a city: another address
        source.fix = Fix::Found(pos(41.01, 28.97));
        w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert_ne!(urls[3], urls[0]);
        assert!(urls[3].contains("latitude=41.0100&longitude=28.9700"));
    }

    #[test]
    fn a_refusal_from_the_settings_is_not_even_asked_and_the_city_takes_over() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Denied, Fix::Found(pos(1.0, 2.0)));
        let mut w = LocatedWeather::new("Erzincan", true);
        let out = w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert!(matches!(out, Outcome::Weather(_)));
        assert_eq!(source.asked_position, 0, "Windows was not asked for the position");
        assert_eq!(urls.len(), 2, "geocoding and forecast for the typed city: {urls:?}");
        assert!(urls[0].contains("geocoding") && urls[0].contains("name=Erzincan"));
    }

    #[test]
    fn a_refusal_without_a_city_is_no_weather_and_no_request_and_a_long_wait() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Denied, Fix::Found(pos(1.0, 2.0)));
        let mut w = LocatedWeather::new("", true);
        assert_eq!(w.refresh_with(&mut source, &mut http_for(&mut urls)), Outcome::UnknownPlace);
        // Windows itself says access denied
        let mut source = Fake::new(Permission::Unknown, Fix::Denied);
        let mut w = LocatedWeather::new("   ", true);
        assert_eq!(w.refresh_with(&mut source, &mut http_for(&mut urls)), Outcome::UnknownPlace);
        assert!(urls.is_empty(), "nothing was sent: {urls:?}");
    }

    #[test]
    fn no_position_in_time_falls_back_to_the_city_or_fails_softly() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Allowed, Fix::Unavailable);
        let mut with_city = LocatedWeather::new("Erzincan", true);
        assert!(matches!(with_city.refresh_with(&mut source, &mut http_for(&mut urls)), Outcome::Weather(_)));
        let before = urls.len();
        let mut no_city = LocatedWeather::new("", true);
        assert_eq!(no_city.refresh_with(&mut source, &mut http_for(&mut urls)), Outcome::Failed);
        assert_eq!(urls.len(), before, "a missing position sends nothing");
    }

    #[test]
    fn with_the_switch_off_nothing_changes_from_before() {
        let mut urls = Vec::new();
        let mut source = Fake::new(Permission::Allowed, Fix::Found(pos(1.0, 2.0)));
        let mut w = LocatedWeather::new(" Erzincan ", false);
        assert!(w.active());
        let out = w.refresh_with(&mut source, &mut http_for(&mut urls));
        assert!(matches!(out, Outcome::Weather(_)));
        assert_eq!((source.asked_permission, source.asked_position), (0, 0), "Windows' location is never touched with the switch off");
        assert!(urls[0].contains("geocoding") && urls[0].contains("name=Erzincan&"));
        // the place of a city is asked once
        let mut more = Vec::new();
        w.refresh_with(&mut source, &mut http_for(&mut more));
        assert_eq!(more.len(), 1);
        // and neither switch nor city: no request, no question
        let mut idle = LocatedWeather::new("", false);
        assert!(!idle.active());
        let mut none = Vec::new();
        assert_eq!(idle.refresh_with(&mut source, &mut http_for(&mut none)), Outcome::UnknownPlace);
        assert!(none.is_empty());
        assert_eq!((source.asked_permission, source.asked_position), (0, 0));
    }

    #[test]
    fn a_failing_network_with_a_position_is_a_failure_and_not_a_fallback() {
        let mut source = Fake::new(Permission::Allowed, Fix::Found(pos(39.75, 39.5)));
        let mut w = LocatedWeather::new("Erzincan", true);
        assert_eq!(w.refresh_with(&mut source, &mut |_| anyhow::bail!("offline")), Outcome::Failed);
        // an answer that is not a forecast
        assert_eq!(w.refresh_with(&mut source, &mut |_| Ok("<html>".into())), Outcome::Failed);
    }

    /// The reviewer's finding: the screen's call must never read the registry itself. The cache answers at once, asks for ONE read in
    /// the background, and hands out the old value while the new one is read.
    #[test]
    fn the_cached_permission_never_waits_and_starts_one_read_at_a_time() {
        use std::time::Instant;
        let t0 = Instant::now();
        let mut c = PermissionCache::default();
        // nothing read yet: Unknown (no line) at once, and a read is asked for
        assert_eq!(c.ask(t0), (Permission::Unknown, true));
        // while it runs nobody else starts one
        assert_eq!(c.ask(t0), (Permission::Unknown, false));
        c.done(t0, Permission::Denied);
        // fresh: the value, no read
        assert_eq!(c.ask(t0 + Duration::from_secs(1)), (Permission::Denied, false));
        // stale: the old value at once, and one new read
        let later = t0 + PERMISSION_FRESH + Duration::from_secs(1);
        assert_eq!(c.ask(later), (Permission::Denied, true));
        assert_eq!(c.ask(later), (Permission::Denied, false));
        c.done(later, Permission::Allowed);
        assert_eq!(c.ask(later), (Permission::Allowed, false));
    }

    #[test]
    fn the_settings_page_is_the_privacy_location_page() {
        assert_eq!(SETTINGS_PAGE, "ms-settings:privacy-location");
    }
}
