//! The weather for the dock's left island: the current temperature and a weather code from Open-Meteo (https://open-meteo.com, no
//! key), for the city the user typed in Settings.
//!
//! PRIVACY. The only thing that ever leaves the machine is the city name the user typed, in one geocoding request, and the
//! coordinates it gave back, in the forecast requests that follow. No location API is asked. NO REQUEST IS MADE while the city is
//! empty (not even a failed one): [`WeatherFeed::refresh`] returns before it touches the network.
//!
//! The HTTP is Windows' own (WinHTTP, through the `windows` crate), with timeouts. Everything here blocks: call it from a worker
//! thread, not the UI thread. Parsing and the weather-code mapping are pure and tested with canned answers.

use std::time::Duration;

use serde::Deserialize;

/// How often the app should ask again.
pub const REFRESH_EVERY: Duration = Duration::from_secs(30 * 60);
/// One request's limit for connecting, sending and receiving, each.
pub const TIMEOUT_MS: u32 = 8_000;
/// An answer bigger than this is not an Open-Meteo answer.
const MAX_BODY: usize = 256 * 1024;

const GEOCODING: &str = "https://geocoding-api.open-meteo.com/v1/search";
const FORECAST: &str = "https://api.open-meteo.com/v1/forecast";

/// A place the geocoder found.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
}

/// The weather right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Observation {
    /// Degrees Celsius.
    pub temp_c: f32,
    /// WMO weather code (see [`condition`]).
    pub code: u8,
    pub is_day: bool,
}

impl Observation {
    /// "12°".
    pub fn text(&self) -> String {
        temperature_text(self.temp_c)
    }

    /// The glyph for the condition, in Segoe Fluent Icons.
    pub fn glyph(&self) -> char {
        glyph(self.code, self.is_day)
    }
}

/// The condition families the WMO codes fall into.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    Clear,
    MainlyClear,
    PartlyCloudy,
    Overcast,
    Fog,
    Drizzle,
    Rain,
    FreezingRain,
    RainShowers,
    Snow,
    SnowGrains,
    SnowShowers,
    Thunderstorm,
    ThunderstormHail,
    Unknown,
}

/// WMO weather interpretation codes, as Open-Meteo documents them.
pub fn condition(code: u8) -> Condition {
    match code {
        0 => Condition::Clear,
        1 => Condition::MainlyClear,
        2 => Condition::PartlyCloudy,
        3 => Condition::Overcast,
        45 | 48 => Condition::Fog,
        51 | 53 | 55 => Condition::Drizzle,
        56 | 57 | 66 | 67 => Condition::FreezingRain,
        61 | 63 | 65 => Condition::Rain,
        71 | 73 | 75 => Condition::Snow,
        77 => Condition::SnowGrains,
        80..=82 => Condition::RainShowers,
        85 | 86 => Condition::SnowShowers,
        95 => Condition::Thunderstorm,
        96 | 99 => Condition::ThunderstormHail,
        _ => Condition::Unknown,
    }
}

/// The Segoe Fluent Icons code point for a condition. The font has no weather set of its own, only a sun, a moon, a cloud, a drop,
/// a bolt, a snowflake-like asterisk and a "lines" glyph, so conditions share: sun by day and moon by night for clear skies, the
/// cloud for partly cloudy and overcast (and for an unknown code), the lines for fog, the drop for every kind of rain, the asterisk
/// for every kind of snow, the bolt for thunder.
pub fn glyph(code: u8, is_day: bool) -> char {
    match condition(code) {
        Condition::Clear | Condition::MainlyClear => {
            if is_day {
                '\u{E706}'
            } else {
                '\u{EC46}'
            }
        }
        Condition::PartlyCloudy | Condition::Overcast | Condition::Unknown => '\u{E753}',
        Condition::Fog => '\u{EA37}',
        Condition::Drizzle | Condition::Rain | Condition::FreezingRain | Condition::RainShowers => '\u{EB42}',
        Condition::Snow | Condition::SnowGrains | Condition::SnowShowers => '\u{EA38}',
        Condition::Thunderstorm | Condition::ThunderstormHail => '\u{E945}',
    }
}

/// "12°", "-3°", "0°" (a temperature that rounds to zero has no sign).
pub fn temperature_text(t: f32) -> String {
    if !t.is_finite() {
        return String::new();
    }
    format!("{}°", t.round() as i32)
}

/// Percent-encodes a query value (everything but the unreserved characters, UTF-8 bytes).
pub fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The geocoding request for a city name (the best hit only).
pub fn geocode_url(city: &str) -> String {
    format!("{GEOCODING}?name={}&count=1&language=en&format=json", percent_encode(city.trim()))
}

/// The forecast request for a place: only the current values we show.
pub fn forecast_url(latitude: f64, longitude: f64) -> String {
    format!("{FORECAST}?latitude={latitude:.4}&longitude={longitude:.4}&current=temperature_2m,weather_code,is_day&timezone=auto")
}

#[derive(Deserialize)]
struct GeoResponse {
    results: Option<Vec<GeoHit>>,
}

#[derive(Deserialize)]
struct GeoHit {
    name: Option<String>,
    latitude: f64,
    longitude: f64,
}

#[derive(Deserialize)]
struct ForecastResponse {
    current: Option<Current>,
}

#[derive(Deserialize)]
struct Current {
    temperature_2m: Option<f64>,
    weather_code: Option<i64>,
    is_day: Option<i64>,
}

/// The geocoder's answer: the first hit, `None` when the city is unknown (the answer then has no `results`).
pub fn parse_geocode(json: &str) -> anyhow::Result<Option<Place>> {
    let r: GeoResponse = serde_json::from_str(json)?;
    Ok(r.results.and_then(|v| v.into_iter().next()).filter(|h| h.latitude.is_finite() && h.longitude.is_finite() && h.latitude.abs() <= 90.0 && h.longitude.abs() <= 180.0).map(
        |h| Place { name: h.name.unwrap_or_default(), latitude: h.latitude, longitude: h.longitude },
    ))
}

/// The forecast's `current` block.
pub fn parse_current(json: &str) -> anyhow::Result<Observation> {
    let r: ForecastResponse = serde_json::from_str(json)?;
    let c = r.current.ok_or_else(|| anyhow::anyhow!("the answer has no current weather"))?;
    let t = c.temperature_2m.ok_or_else(|| anyhow::anyhow!("the answer has no temperature"))?;
    let code = c.weather_code.ok_or_else(|| anyhow::anyhow!("the answer has no weather code"))?;
    anyhow::ensure!(t.is_finite() && (-100.0..=70.0).contains(&t), "a temperature of {t} degrees is not believable");
    Ok(Observation { temp_c: t as f32, code: u8::try_from(code).unwrap_or(u8::MAX), is_day: c.is_day.unwrap_or(1) != 0 })
}

/// The host and the path with its query of an `https://` URL.
fn split_https(url: &str) -> Option<(&str, &str)> {
    let rest = url.strip_prefix("https://")?;
    let slash = rest.find('/')?;
    Some((&rest[..slash], &rest[slash..]))
}

/// A closed-on-drop WinHTTP handle.
struct Handle(*mut std::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = windows::Win32::Networking::WinHttp::WinHttpCloseHandle(self.0);
            }
        }
    }
}

fn nonnull(h: *mut std::ffi::c_void, what: &str) -> anyhow::Result<Handle> {
    if h.is_null() {
        anyhow::bail!("{what}: {}", windows::core::Error::from_thread());
    }
    Ok(Handle(h))
}

/// GET of an `https://` URL with WinHTTP; the body as text. Errors for a status other than 200, a body over the limit, a timeout.
pub fn http_get(url: &str, timeout_ms: u32) -> anyhow::Result<String> {
    use windows::Win32::Networking::WinHttp::{
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, WINHTTP_FLAG_SECURE, WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE, WinHttpConnect,
        WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable, WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
    };
    use windows::core::{PCWSTR, w};
    let (host, path) = split_https(url).ok_or_else(|| anyhow::anyhow!("not an https URL: {url}"))?;
    let (host_w, path_w) = (crate::wide::wide(host), crate::wide::wide(path));
    let started = std::time::Instant::now();
    unsafe {
        let agent = w!("rustyfiles-dock");
        let mut session = WinHttpOpen(agent, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        if session.is_null() {
            // before Windows 8.1 there is no automatic proxy mode
            session = WinHttpOpen(agent, WINHTTP_ACCESS_TYPE_DEFAULT_PROXY, PCWSTR::null(), PCWSTR::null(), 0);
        }
        let session = nonnull(session, "WinHttpOpen")?;
        let t = timeout_ms.min(i32::MAX as u32) as i32;
        WinHttpSetTimeouts(session.0, t, t, t, t)?;
        let connection = nonnull(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), 443, 0), "WinHttpConnect")?;
        let request = nonnull(
            WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path_w.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE),
            "WinHttpOpenRequest",
        )?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0)?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())?;
        let mut status = 0u32;
        let mut len = size_of::<u32>() as u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut len,
            std::ptr::null_mut(),
        )?;
        anyhow::ensure!(status == 200, "{host} answered {status}");
        let mut body: Vec<u8> = Vec::new();
        let total_limit = Duration::from_millis(timeout_ms as u64 * 3);
        loop {
            anyhow::ensure!(started.elapsed() < total_limit, "{host} answered too slowly");
            let mut avail = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut avail)?;
            if avail == 0 {
                break;
            }
            let at = body.len();
            anyhow::ensure!(at + avail as usize <= MAX_BODY, "the answer of {host} is too big");
            body.resize(at + avail as usize, 0);
            let mut got = 0u32;
            WinHttpReadData(request.0, body[at..].as_mut_ptr() as *mut _, avail, &mut got)?;
            body.truncate(at + got as usize);
            if got == 0 {
                break;
            }
        }
        Ok(String::from_utf8(body)?)
    }
}

/// What the dock holds for the weather: the city the user typed and the place it was found at (asked once per city, not every 30
/// minutes).
#[derive(Clone, Debug, Default)]
pub struct WeatherFeed {
    city: String,
    place: Option<Place>,
}

impl WeatherFeed {
    pub fn new(city: &str) -> WeatherFeed {
        WeatherFeed { city: city.trim().to_string(), place: None }
    }

    pub fn city(&self) -> &str {
        &self.city
    }

    /// The user changed the city: the old place is forgotten.
    pub fn set_city(&mut self, city: &str) {
        let city = city.trim();
        if city != self.city {
            self.city = city.to_string();
            self.place = None;
        }
    }

    /// Whether a refresh would ask anything at all: only with a city.
    pub fn active(&self) -> bool {
        !self.city.is_empty()
    }

    /// One refresh with `get` as the HTTP (so a test can count the requests). `Ok(None)` for an empty city (no request was made) or
    /// a city the geocoder does not know.
    pub fn refresh_with(&mut self, get: &mut dyn FnMut(&str) -> anyhow::Result<String>) -> anyhow::Result<Option<Observation>> {
        if !self.active() {
            return Ok(None);
        }
        if self.place.is_none() {
            match parse_geocode(&get(&geocode_url(&self.city))?)? {
                Some(p) => self.place = Some(p),
                None => return Ok(None),
            }
        }
        let Some(p) = &self.place else { return Ok(None) };
        let obs = parse_current(&get(&forecast_url(p.latitude, p.longitude))?)?;
        Ok(Some(obs))
    }

    /// One refresh over the network. Blocks.
    pub fn refresh(&mut self) -> anyhow::Result<Option<Observation>> {
        self.refresh_with(&mut |u| http_get(u, TIMEOUT_MS))
    }

    /// [`WeatherFeed::refresh`] for the dock: a failure shows nothing and is logged (and the place is asked again next time, in
    /// case it was the geocoding that failed). The outcome tells the caller how soon to try again.
    pub fn poll(&mut self) -> Outcome {
        let result = self.refresh();
        let outcome = outcome_of(&result);
        match &result {
            Ok(None) if self.active() => log::info!("weather: no place named {:?}", self.city),
            Err(e) => log::warn!("weather: {e:#}"),
            _ => {}
        }
        outcome
    }
}

/// How one refresh ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome {
    /// The weather right now.
    Weather(Observation),
    /// The geocoder does not know the city. Asking again soon will not change that: only a different city (or a long wait) will.
    UnknownPlace,
    /// The network, the service or its answer failed.
    Failed,
}

impl Outcome {
    /// The observation, if there is one.
    pub fn observation(&self) -> Option<Observation> {
        match self {
            Outcome::Weather(o) => Some(*o),
            _ => None,
        }
    }
}

/// What a refresh's result means for the caller. (An empty city also gives `Ok(None)`; the caller never polls without a city.)
pub fn outcome_of(result: &anyhow::Result<Option<Observation>>) -> Outcome {
    match result {
        Ok(Some(o)) => Outcome::Weather(*o),
        Ok(None) => Outcome::UnknownPlace,
        Err(_) => Outcome::Failed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GEO: &str = r#"{"results":[{"id":315202,"name":"Erzincan","latitude":39.75,"longitude":39.5,"elevation":1180.0,"feature_code":"PPLA","country_code":"TR","admin1":"Erzincan","country":"Türkiye"},{"id":1,"name":"Other","latitude":1.0,"longitude":2.0}],"generationtime_ms":0.6}"#;
    const FORECAST_JSON: &str = r#"{"latitude":39.75,"longitude":39.5,"timezone":"Europe/Istanbul","current_units":{"temperature_2m":"°C"},"current":{"time":"2026-11-12T14:00","interval":900,"temperature_2m":12.3,"weather_code":3,"is_day":1}}"#;

    #[test]
    fn geocode_answer_gives_the_first_place() {
        let p = parse_geocode(GEO).unwrap().unwrap();
        assert_eq!(p, Place { name: "Erzincan".into(), latitude: 39.75, longitude: 39.5 });
    }

    #[test]
    fn an_unknown_city_has_no_results_key() {
        assert_eq!(parse_geocode(r#"{"generationtime_ms":0.4}"#).unwrap(), None);
        assert_eq!(parse_geocode(r#"{"results":[]}"#).unwrap(), None);
        // a place with coordinates that cannot exist is not a place
        assert_eq!(parse_geocode(r#"{"results":[{"name":"x","latitude":123.0,"longitude":0.0}]}"#).unwrap(), None);
        assert!(parse_geocode("<html>").is_err());
    }

    #[test]
    fn forecast_answer_gives_temperature_code_and_daylight() {
        let o = parse_current(FORECAST_JSON).unwrap();
        assert_eq!(o, Observation { temp_c: 12.3, code: 3, is_day: true });
        assert_eq!(o.text(), "12°");
        let night = parse_current(r#"{"current":{"temperature_2m":-3.6,"weather_code":71,"is_day":0}}"#).unwrap();
        assert!(!night.is_day);
        assert_eq!(night.text(), "-4°");
        // a missing is_day means day, not a failure
        assert!(parse_current(r#"{"current":{"temperature_2m":5.0,"weather_code":0}}"#).unwrap().is_day);
    }

    #[test]
    fn broken_forecasts_are_errors() {
        assert!(parse_current(r#"{"latitude":1}"#).is_err());
        assert!(parse_current(r#"{"current":{"weather_code":1}}"#).is_err());
        assert!(parse_current(r#"{"current":{"temperature_2m":null,"weather_code":1}}"#).is_err());
        assert!(parse_current(r#"{"current":{"temperature_2m":12.0}}"#).is_err());
        assert!(parse_current(r#"{"current":{"temperature_2m":999.0,"weather_code":1}}"#).is_err());
        assert!(parse_current("").is_err());
        // a code that is not a WMO code still parses; the mapping falls back to the cloud
        assert_eq!(parse_current(r#"{"current":{"temperature_2m":1.0,"weather_code":1000}}"#).unwrap().code, 255);
    }

    #[test]
    fn temperatures_read_like_the_taskbar_weather() {
        assert_eq!(temperature_text(12.0), "12°");
        assert_eq!(temperature_text(12.5), "13°");
        assert_eq!(temperature_text(-3.0), "-3°");
        assert_eq!(temperature_text(-0.4), "0°");
        assert_eq!(temperature_text(0.0), "0°");
        assert_eq!(temperature_text(f32::NAN), "");
    }

    #[test]
    fn wmo_codes_map_to_conditions() {
        use Condition::*;
        let cases = [
            (0, Clear), (1, MainlyClear), (2, PartlyCloudy), (3, Overcast), (45, Fog), (48, Fog), (51, Drizzle), (53, Drizzle), (55, Drizzle), (56, FreezingRain),
            (57, FreezingRain), (61, Rain), (63, Rain), (65, Rain), (66, FreezingRain), (67, FreezingRain), (71, Snow), (73, Snow), (75, Snow), (77, SnowGrains),
            (80, RainShowers), (81, RainShowers), (82, RainShowers), (85, SnowShowers), (86, SnowShowers), (95, Thunderstorm), (96, ThunderstormHail),
            (99, ThunderstormHail), (4, Unknown), (44, Unknown), (100, Unknown),
        ];
        for (code, want) in cases {
            assert_eq!(condition(code), want, "code {code}");
        }
    }

    #[test]
    fn every_code_has_a_glyph_from_the_icon_font_and_sun_becomes_moon_at_night() {
        for code in 0..=255u8 {
            for day in [true, false] {
                let g = glyph(code, day);
                assert!(('\u{E000}'..='\u{F8FF}').contains(&g), "code {code} day {day}: {g:?} is not a private-use code point");
            }
        }
        assert_eq!(glyph(0, true), '\u{E706}');
        assert_eq!(glyph(0, false), '\u{EC46}');
        assert_eq!(glyph(1, false), '\u{EC46}');
        assert_eq!(glyph(3, true), glyph(3, false), "clouds do not change at night");
        assert_eq!(glyph(61, true), '\u{EB42}');
        assert_eq!(glyph(95, true), '\u{E945}');
        assert_ne!(glyph(61, true), glyph(71, true), "rain and snow differ");
        assert_ne!(glyph(45, true), glyph(3, true), "fog and cloud differ");
    }

    #[test]
    fn urls_are_encoded() {
        assert_eq!(percent_encode("Erzincan"), "Erzincan");
        assert_eq!(percent_encode("İstanbul"), "%C4%B0stanbul");
        assert_eq!(percent_encode("New York"), "New%20York");
        assert_eq!(percent_encode("a&b=c"), "a%26b%3Dc");
        assert_eq!(geocode_url("  Şanlıurfa "), "https://geocoding-api.open-meteo.com/v1/search?name=%C5%9Eanl%C4%B1urfa&count=1&language=en&format=json");
        assert_eq!(
            forecast_url(39.75, 39.5),
            "https://api.open-meteo.com/v1/forecast?latitude=39.7500&longitude=39.5000&current=temperature_2m,weather_code,is_day&timezone=auto"
        );
        assert_eq!(split_https("https://api.open-meteo.com/v1/forecast?x=1"), Some(("api.open-meteo.com", "/v1/forecast?x=1")));
        assert_eq!(split_https("http://api.open-meteo.com/v1"), None);
    }

    #[test]
    fn a_refresh_ends_as_weather_an_unknown_place_or_a_failure() {
        let obs = Observation { temp_c: 12.0, code: 3, is_day: true };
        assert_eq!(outcome_of(&Ok(Some(obs))), Outcome::Weather(obs));
        assert_eq!(outcome_of(&Ok(None)), Outcome::UnknownPlace);
        assert_eq!(outcome_of(&Err(anyhow::anyhow!("offline"))), Outcome::Failed);
        assert_eq!(Outcome::Weather(obs).observation(), Some(obs));
        assert_eq!(Outcome::UnknownPlace.observation(), None);
        assert_eq!(Outcome::Failed.observation(), None);
    }

    #[test]
    fn an_empty_city_makes_no_request_at_all() {
        let mut calls = 0;
        for city in ["", "   ", "\t\n"] {
            let mut feed = WeatherFeed::new(city);
            assert!(!feed.active());
            let r = feed.refresh_with(&mut |_| {
                calls += 1;
                anyhow::bail!("must not be asked")
            });
            assert!(matches!(r, Ok(None)));
        }
        assert_eq!(calls, 0);
    }

    #[test]
    fn the_place_is_asked_once_and_the_forecast_every_time() {
        let mut urls: Vec<String> = Vec::new();
        let mut feed = WeatherFeed::new(" Erzincan ");
        let mut get = |u: &str| -> anyhow::Result<String> {
            urls.push(u.to_string());
            Ok(if u.contains("geocoding") { GEO.to_string() } else { FORECAST_JSON.to_string() })
        };
        let a = feed.refresh_with(&mut get).unwrap().unwrap();
        let b = feed.refresh_with(&mut get).unwrap().unwrap();
        assert_eq!(a, b);
        assert_eq!(urls.len(), 3, "{urls:?}");
        assert!(urls[0].starts_with(GEOCODING) && urls[0].contains("name=Erzincan&"));
        assert!(urls[1].starts_with(FORECAST) && urls[1].contains("latitude=39.7500"));
        assert_eq!(feed.city(), "Erzincan");
        // a new city is a new geocoding
        feed.set_city("Ankara");
        let mut n = 0;
        let _ = feed.refresh_with(&mut |u| {
            n += 1;
            Ok(if u.contains("geocoding") { GEO.to_string() } else { FORECAST_JSON.to_string() })
        });
        assert_eq!(n, 2);
        // the same city again changes nothing
        feed.set_city(" Ankara ");
        let mut n = 0;
        let _ = feed.refresh_with(&mut |_| {
            n += 1;
            Ok(FORECAST_JSON.to_string())
        });
        assert_eq!(n, 1);
    }

    #[test]
    fn an_unknown_city_and_a_failing_network_show_nothing() {
        let mut feed = WeatherFeed::new("Nowhereville");
        assert!(matches!(feed.refresh_with(&mut |_| Ok(r#"{"generationtime_ms":0.2}"#.into())), Ok(None)));
        let mut feed = WeatherFeed::new("Erzincan");
        assert!(feed.refresh_with(&mut |_| anyhow::bail!("offline")).is_err());
        // after a geocoding failure the place is still unknown, so the next refresh asks for it again
        let mut n = 0;
        let r = feed.refresh_with(&mut |u| {
            n += 1;
            Ok(if u.contains("geocoding") { GEO.to_string() } else { FORECAST_JSON.to_string() })
        });
        assert!(r.unwrap().is_some());
        assert_eq!(n, 2);
    }

    #[test]
    fn a_non_https_url_is_refused_before_any_connection() {
        assert!(http_get("http://api.open-meteo.com/v1/forecast", 100).is_err());
        assert!(http_get("ftp://x/y", 100).is_err());
        assert!(http_get("https://", 100).is_err());
    }
}
