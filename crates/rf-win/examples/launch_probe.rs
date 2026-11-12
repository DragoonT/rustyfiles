//! What a Start entry is and whether it can be started: `launch_probe <part of the name> [go]`. Without `go` nothing is started.
fn main() -> anyhow::Result<()> {
    let want = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let go = std::env::args().nth(2).as_deref() == Some("go");
    let apps = rf_win::apps::list()?;
    println!("{} apps", apps.len());
    for app in apps.iter().filter(|a| !want.is_empty() && a.name.to_lowercase().contains(&want)).take(4) {
        let target = rf_win::dock::startapps::location(app);
        // (the direct way is read on the shell thread: it finds the entry's shortcut through COM)
        let app1 = app.clone();
        let way = rf_win::sta::call(std::time::Duration::from_secs(10), move || rf_win::apps::direct_way_of(&app1));
        println!("{:?}: id {:?}, file behind it {:?}, direct way {:?}", app.name, app.id, target, way);
        // `shell` / `direct`: one way alone, timed (to see which one is slow or refuses without Explorer)
        if let Some(way) = std::env::args().nth(2).filter(|w| w == "shell" || w == "direct") {
            let app2 = app.clone();
            let t = std::time::Instant::now();
            let r = rf_win::sta::call(std::time::Duration::from_secs(40), move || if way == "shell" { rf_win::shellfolder::open(&app2.parsing()) } else { rf_win::apps::launch_directly_for_probe(&app2) });
            println!("  {:?} after {} ms", r.map(|x| x.map_err(|e| e.to_string())), t.elapsed().as_millis());
            break;
        }
        if go {
            println!("  start: {:?}", rf_win::dock::startapps::launch(app).map_err(|e| e.to_string()));
            break;
        }
    }
    Ok(())
}
