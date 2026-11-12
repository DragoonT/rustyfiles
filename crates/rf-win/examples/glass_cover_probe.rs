//! Read-only look at what the glass engine's cover walk (`glass/cover.rs`) makes of the windows on this desktop: for a rectangle of the
//! desktop it lists the visible top-level windows that meet it, top of the z order first, with their extended style, layered
//! attributes, region and cloak state, whether the walk counts each as hiding what lies under it, and whether together they hide the
//! rectangle. No window is created or changed; nothing is sent to any window.
//!
//!   cargo run --release -p rf-win --example glass_cover_probe -- X Y W H
//!
//! (default: the middle of the primary monitor, 400 x 300). Window titles are not read.

fn main() {
    let a: Vec<i32> = std::env::args().skip(1).filter_map(|s| s.parse().ok()).collect();
    let rect = if a.len() == 4 { (a[0], a[1], a[2].max(1) as u32, a[3].max(1) as u32) } else { (1080, 570, 400, 300) };
    unsafe {
        let _ = windows::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(windows::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let (lines, covered) = rf_win::glass::check::cover_survey(rect);
    println!("windows that meet the rectangle {:?}, top of the z order first:", rect);
    for l in &lines {
        println!("  {l}");
    }
    println!("{} windows; together they hide the rectangle completely: {}", lines.len(), if covered { "YES" } else { "no" });
}
