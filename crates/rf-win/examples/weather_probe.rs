//! A live look at the weather route with a city given on the command line: the same geocode and forecast requests the dock makes.
fn main() -> anyhow::Result<()> {
    let city = std::env::args().nth(1).unwrap_or_else(|| "London".into());
    let geo = rf_win::weather::http_get(&rf_win::weather::geocode_url(&city), 8000)?;
    let Some(place) = rf_win::weather::parse_geocode(&geo)? else {
        println!("no such place: {city}");
        return Ok(());
    };
    let cur = rf_win::weather::http_get(&rf_win::weather::forecast_url(place.latitude, place.longitude), 8000)?;
    let obs = rf_win::weather::parse_current(&cur)?;
    println!("{place:?}\n{obs:?}");
    Ok(())
}
