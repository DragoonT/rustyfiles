//! Windows' own "always show scrollbars" choice: Settings › Accessibility › Visual effects › "Always show scrollbars" on
//! Windows 11, Ease of Access › Display › "Automatically hide scroll bars in Windows" (off) on Windows 10. Both write
//! `HKCU\Control Panel\Accessibility\DynamicScrollbars`, 0 when the bars are to stay. Asked on every frame, so the registry is
//! read at most every few seconds.

use std::sync::Mutex;
use std::time::{Duration, Instant};

static CACHE: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// Whether the user asked Windows to show scrollbars all the time instead of only while scrolling.
pub fn always_shown() -> bool {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, shown)) = *cache
        && at.elapsed() < Duration::from_secs(5)
    {
        return shown;
    }
    let shown = windows_registry::CURRENT_USER
        .open(r"Control Panel\Accessibility")
        .and_then(|k| k.get_u32("DynamicScrollbars"))
        .is_ok_and(|v| v == 0);
    *cache = Some((Instant::now(), shown));
    shown
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_answer_is_read_and_kept() {
        let first = super::always_shown();
        assert_eq!(super::always_shown(), first);
    }
}
