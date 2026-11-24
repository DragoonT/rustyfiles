//! Says one line to the running dock on its pipe: `cargo run -p rf-win --example dock_say -- stop` (or `reload`, `ping`, and in a
//! test run with made-up notifications `notifications`). Start it with the same `RUSTYFILES_CONFIG_DIR` as the dock it is meant
//! for: the pipe's name comes from it. Prints whether a dock took the line.

fn main() {
    let line = std::env::args().nth(1).unwrap_or_else(|| "ping".into());
    let taken = rf_win::single_instance::send_to("rustyfiles-dock", &line);
    println!("{line}: {}", if taken { "taken" } else { "no dock answered" });
}
