//! `RUSTYFILES_TRACE_STALL=1` (or a number of milliseconds): logs every time the UI thread was blocked for longer than that
//! (default 50 ms), with how long, and the longest block so far.
//!
//! How: a task on the UI thread asks the background timer for a wake-up in 10 ms. The wake-up can only run once the UI thread is
//! free again, so the time it comes late is the time the UI thread was held. Nothing is read from the window or the disk. Off
//! (the default) this costs one environment lookup at start and no task.

use std::time::{Duration, Instant};

use gpui_kit::App;

const TICK: Duration = Duration::from_millis(10);
const DEFAULT_LIMIT_MS: u64 = 50;

/// The limit asked for in the environment variable's text: a number of milliseconds, or the default for "1", "true", "on" and the like.
fn limit_from(text: &str) -> Duration {
    match text.trim().parse::<u64>() {
        Ok(ms) if ms >= 10 => Duration::from_millis(ms),
        _ => Duration::from_millis(DEFAULT_LIMIT_MS),
    }
}

/// How long the thread was held, given the time a tick of `TICK` really took.
fn held(took: Duration) -> Duration {
    took.saturating_sub(TICK)
}

/// Starts the watch when the environment asks for it.
pub fn start(cx: &mut App) {
    let Some(text) = std::env::var_os("RUSTYFILES_TRACE_STALL") else { return };
    let limit = limit_from(&text.to_string_lossy());
    log::warn!("stall trace on: the UI thread held for more than {} ms is logged", limit.as_millis());
    cx.spawn(async move |cx| {
        let mut longest = Duration::ZERO;
        loop {
            let t = Instant::now();
            cx.background_executor().timer(TICK).await;
            let late = held(t.elapsed());
            if late >= limit {
                longest = longest.max(late);
                log::warn!("ui stall: the UI thread was held for {} ms (longest so far {} ms)", late.as_millis(), longest.as_millis());
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_limit_reads_a_number_or_falls_back() {
        assert_eq!(limit_from("1"), Duration::from_millis(50));
        assert_eq!(limit_from("true"), Duration::from_millis(50));
        assert_eq!(limit_from(""), Duration::from_millis(50));
        assert_eq!(limit_from(" 200 "), Duration::from_millis(200));
        assert_eq!(limit_from("10"), Duration::from_millis(10));
    }

    #[test]
    fn a_tick_that_came_on_time_held_nothing() {
        assert_eq!(held(Duration::from_millis(9)), Duration::ZERO);
        assert_eq!(held(Duration::from_millis(10)), Duration::ZERO);
        assert_eq!(held(Duration::from_millis(1510)), Duration::from_millis(1500));
    }
}
