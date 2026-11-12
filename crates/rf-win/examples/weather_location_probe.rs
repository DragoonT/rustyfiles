//! A windowless look at the weather-from-position route, with NOTHING real behind it: a fake position source and a fake network. It
//! prints the order of the sources for every (switch, city, permission, position) combination, how a raw coordinate is cut to two
//! decimals and what forecast address it makes, and what a few refreshes ask of the fake network. It never asks Windows for the
//! position (that may put a consent prompt on the screen), never contacts a server, shows no window, writes nothing.
//! The one real thing it reads is Windows' location privacy switches from the registry (read only; the answer is Allowed / Denied /
//! Unknown, no place). `cargo run --offline -p rf-win --example weather_location_probe`

use rf_win::weather::Outcome;
use rf_win::weather_location::{Fix, LocatedWeather, Permission, Position, PositionSource, choose_route, forecast_url_for, place_changed, should_ask};

struct Fake(Fix);

impl PositionSource for Fake {
    fn permission(&mut self) -> Permission {
        Permission::Allowed
    }
    fn position(&mut self) -> Fix {
        self.0
    }
}

fn main() {
    println!("-- the order: switch, city typed, Windows' permission, what Windows gave -> where the weather comes from");
    let here = Position::from_degrees(39.754_321, 39.498_76).unwrap();
    for switch in [false, true] {
        for city in [false, true] {
            for permission in [Permission::Allowed, Permission::Unknown, Permission::Denied] {
                for fix in [Fix::Found(here), Fix::Denied, Fix::Unavailable] {
                    // (what the dock does: Windows is asked only when `should_ask` says so)
                    let asked = should_ask(switch, permission).then_some(fix);
                    let fix_text = if asked.is_some() { format!("{fix:?}") } else { "not asked".to_string() };
                    println!("switch {switch:<5} city {city:<5} {permission:<8?} {fix_text:<28} -> {:?}", choose_route(switch, city, permission, asked));
                }
            }
        }
    }

    println!("\n-- rounding to two decimals (the exact numbers never leave Position::from_degrees)");
    for (lat, lon) in [(39.754_321, 39.498_76), (-33.8698, 151.2099), (0.004, -0.004), (90.0, 180.0), (91.0, 0.0), (f64::NAN, 0.0)] {
        match Position::from_degrees(lat, lon) {
            Some(p) => println!("({lat}, {lon}) -> ({}, {})  {}", p.latitude(), p.longitude(), forecast_url_for(p)),
            None => println!("({lat}, {lon}) -> not a coordinate"),
        }
    }
    let held = Position::from_degrees(39.7512, 39.4988);
    println!("same rounded place after a small move: {}", !place_changed(held, Position::from_degrees(39.7549, 39.5011).unwrap()));
    println!("another place after a move of 0.01 degrees: {}", place_changed(held, Position::from_degrees(39.7612, 39.4988).unwrap()));

    println!("\n-- a few refreshes against a fake network");
    let forecast = r#"{"current":{"temperature_2m":6.4,"weather_code":3,"is_day":1}}"#;
    let geocode = r#"{"results":[{"name":"X","latitude":39.75,"longitude":39.5}]}"#;
    for (label, city, switch, fix) in [
        ("switch on, position found, city typed", "Ankara", true, Fix::Found(here)),
        ("switch on, Windows refuses, city typed", "Ankara", true, Fix::Denied),
        ("switch on, Windows refuses, no city", "", true, Fix::Denied),
        ("switch on, no position in time, no city", "", true, Fix::Unavailable),
        ("switch off, city typed", "Ankara", false, Fix::Found(here)),
        ("switch off, no city", "", false, Fix::Found(here)),
    ] {
        let mut asked: Vec<String> = Vec::new();
        let mut w = LocatedWeather::new(city, switch);
        let out = w.refresh_with(&mut Fake(fix), &mut |u| {
            asked.push(u.split('?').next().unwrap_or("").to_string() + &u.split('?').nth(1).map(|q| format!("?{}", q.split('&').take(2).collect::<Vec<_>>().join("&"))).unwrap_or_default());
            Ok(if u.contains("geocoding") { geocode.to_string() } else { forecast.to_string() })
        });
        let shown = match out {
            Outcome::Weather(o) => format!("weather {}", o.text()),
            Outcome::UnknownPlace => "nothing (ask again in an hour)".to_string(),
            Outcome::Failed => "nothing (ask again soon)".to_string(),
        };
        println!("{label}: {shown}; {} request(s){}", asked.len(), if asked.is_empty() { String::new() } else { format!(" {asked:?}") });
    }

    println!("\n-- Windows' privacy switches for location (read from the registry, nothing written)");
    println!("{:?}", rf_win::weather_location::windows_permission());
}
