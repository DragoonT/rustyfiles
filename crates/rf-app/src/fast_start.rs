//! Whether the window opens on the CPU renderer first (the windowing fork's "fast start"), decided per machine.
//!
//! WHAT IT IS. The first window needs a graphics device. A discrete card's takes about 110 ms to make (measured here: an RTX 5070
//! Ti, 105 to 115 ms), Windows' software rasterizer's one millisecond. With the fast start the window opens on the software one
//! and the card's device, made meanwhile on another thread, takes over about a tenth of a second later; nothing of the switch
//! shows. It brought the first window from 310 to 199 ms after the process's start on that machine, and costs CPU time while the
//! software rasterizer draws (0.74 s -> 1.25 s of CPU time in the first 1.8 s).
//!
//! WHY PER MACHINE. It pays only where the card's device is slow to make. Where it takes a few ms (we have measured one machine,
//! so this is the careful reading, not a finding) the software frames would cost CPU time and buy nothing. So the time the device
//! took is remembered between starts, and the fast start is asked for when that was [`SLOW_DEVICE_MS`] or more. A machine's first
//! start therefore opens on the card, as every start did before.

use std::path::PathBuf;

/// A device that took this long to make is worth not waiting for.
const SLOW_DEVICE_MS: u32 = 60;

/// Whether to ask for the fast start, for what is remembered of this machine's device (`None`: nothing yet).
fn wanted(last_device_ms: Option<u32>) -> bool {
    last_device_ms.is_some_and(|ms| ms >= SLOW_DEVICE_MS)
}

/// What the file says: a number of milliseconds, and nothing else.
fn parse(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|ms| (1..=60_000).contains(ms))
}

/// Whether a new measurement is worth writing: there was none, or it differs by more than a fifth (the device's time varies by a
/// few ms from start to start; the file is not written at every start).
fn worth_writing(remembered: Option<u32>, now: u32) -> bool {
    remembered.is_none_or(|old| now.abs_diff(old) * 5 > old.max(1))
}

fn file() -> Option<PathBuf> {
    Some(crate::settings::local_data_dir()?.join("gpu-device-ms"))
}

fn remembered() -> Option<u32> {
    parse(&std::fs::read_to_string(file()?).ok()?)
}

/// Before the platform is created: tells the windowing layer what this machine is like. One small file read; a machine that has
/// none yet opens on the card.
pub fn before_app() {
    gpui_windows::hybrid::set_fast_start(wanted(remembered()));
}

/// A while after the start (the device exists by then, whichever way it was made): remembers how long it took, off the
/// interface's thread.
pub fn remember(cx: &mut gpui_kit::App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(std::time::Duration::from_secs(5)).await;
        let Some(now) = gpui_windows::hybrid::gpu_device_ms() else { return };
        cx.background_executor()
            .spawn(async move {
                if worth_writing(remembered(), now)
                    && let Some(path) = file()
                {
                    if let Some(dir) = path.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(path, now.to_string());
                }
            })
            .await;
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fast_start_is_asked_for_where_the_device_is_slow_to_make() {
        assert!(!wanted(None), "a machine's first start: nothing is known, it opens on the card");
        assert!(!wanted(Some(8)) && !wanted(Some(59)), "a device made in a few ms is waited for");
        assert!(wanted(Some(60)) && wanted(Some(110)) && wanted(Some(400)));
    }

    #[test]
    fn the_file_is_a_number_of_milliseconds_or_nothing() {
        assert_eq!((parse("110"), parse(" 7\r\n"), parse("60000")), (Some(110), Some(7), Some(60_000)));
        for bad in ["", "0", "abc", "-5", "110 ms", "60001", "1e3"] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn a_measurement_is_written_when_there_was_none_or_it_is_another_one() {
        assert!(worth_writing(None, 110));
        assert!(!worth_writing(Some(110), 110) && !worth_writing(Some(110), 118) && !worth_writing(Some(110), 95));
        assert!(worth_writing(Some(110), 140) && worth_writing(Some(110), 40) && worth_writing(Some(8), 110));
        assert!(!worth_writing(Some(5), 5) && worth_writing(Some(5), 7));
    }
}
