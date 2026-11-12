//! Brightness of the primary monitor through DDC/CI (`dxva2`: `GetMonitorBrightness` / `SetMonitorBrightness`). It is the one
//! documented way for a desktop monitor; it needs no administrator rights. A laptop's own panel does not answer DDC/CI (it is driven
//! through WMI), which is NOT implemented here: on such a machine the call fails, [`BrightnessSnapshot::available`] is false and the
//! panel leaves the row out.
//!
//! Slow and fussy: a DDC/CI transfer takes tens to hundreds of milliseconds and some monitors misbehave when asked too often. So
//! nothing here runs while the panel is closed and there is no change event to wait for (a monitor's own buttons cannot be heard):
//! [`open`] starts a thread of its own (MTA) that reads once when the panel opens, and writes only what the user's slider asks for,
//! one transfer at a time, the newest value of a drag (see [`Latest`](super::Latest)). Dropping the handle ends the thread and
//! releases the monitor.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};

use windows::Win32::Devices::Display::{DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness};
use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};

use super::{Shared, Subscription, init_mta};

/// What the slider draws the brightness from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BrightnessSnapshot {
    /// The monitor answered DDC/CI. False: no row.
    pub available: bool,
    pub min: u32,
    pub value: u32,
    pub max: u32,
}

impl BrightnessSnapshot {
    /// The slider's position, 0.0 to 1.0.
    pub fn fraction(&self) -> f32 {
        to_fraction(self.min, self.value, self.max)
    }

    /// The monitor's own value for the slider at `fraction`.
    pub fn value_at(&self, fraction: f32) -> u32 {
        from_fraction(self.min, self.max, fraction)
    }
}

/// The slider position of `value` in the monitor's range (0.0 when the range is empty).
pub fn to_fraction(min: u32, value: u32, max: u32) -> f32 {
    if max <= min {
        return 0.0;
    }
    (value.clamp(min, max) - min) as f32 / (max - min) as f32
}

/// The monitor's value for a slider position, rounded to a whole step and kept inside the range.
pub fn from_fraction(min: u32, max: u32, fraction: f32) -> u32 {
    if max <= min {
        return min;
    }
    let f = if fraction.is_nan() { 0.0 } else { fraction.clamp(0.0, 1.0) };
    min + (f * (max - min) as f32).round() as u32
}

/// The open monitor. Dropping it ends the thread.
pub struct Brightness {
    queue: Arc<Shared<u32>>,
    wake: std::sync::mpsc::Sender<Msg>,
    _end: Subscription,
}

enum Msg {
    Write,
    Quit,
}

impl Brightness {
    /// The user moved the slider to this raw value (`BrightnessSnapshot::value_at`). Returns at once.
    pub fn set(&self, value: u32) {
        if self.queue.push(value) {
            let _ = self.wake.send(Msg::Write);
        }
    }
}

/// Reads the primary monitor's brightness on a thread of its own and tells `callback` (once: the reading, `available: false` when
/// the monitor does not answer), then keeps the monitor for [`Brightness::set`].
pub fn open(callback: impl Fn(&BrightnessSnapshot) + Send + 'static) -> Brightness {
    let queue = Shared::<u32>::new();
    let (tx, rx) = channel::<Msg>();
    let q = queue.clone();
    let spawned = std::thread::Builder::new().name("rf-brightness".into()).spawn(move || work(rx, q, callback));
    if let Err(e) = spawned {
        log::warn!("brightness: the thread did not start: {e}");
    }
    let quit = tx.clone();
    Brightness { queue, wake: tx, _end: Subscription::new(move || { let _ = quit.send(Msg::Quit); }) }
}

/// Reads once on the calling thread (a probe). Blocks for as long as the monitor takes (and a few tries, see [`TRIES`]).
pub fn read_once() -> BrightnessSnapshot {
    init_mta();
    first_reading().1
}

/// How many times the first reading is tried, and the wait between tries. A monitor that has not been spoken to for a while can
/// miss a DDC/CI transfer (seen on the development machine: a read now and then fails and the next one works), so a monitor is only
/// called unavailable after a few tries. Only the first reading of a panel opening is repeated, and only when it fails.
const TRIES: u32 = 3;
const RETRY_AFTER: std::time::Duration = std::time::Duration::from_millis(250);

/// The primary monitor and its brightness, with the tries.
fn first_reading() -> (Option<Monitors>, BrightnessSnapshot) {
    let mut monitors = Monitors::primary();
    let mut snap = BrightnessSnapshot::default();
    for attempt in 0..TRIES {
        if attempt > 0 {
            std::thread::sleep(RETRY_AFTER);
            monitors = Monitors::primary();
        }
        snap = monitors.as_ref().and_then(|m| m.read()).unwrap_or_default();
        if snap.available {
            break;
        }
        log::debug!("brightness: no answer from the monitor (try {} of {TRIES})", attempt + 1);
    }
    (monitors, snap)
}

fn work(rx: Receiver<Msg>, queue: Arc<Shared<u32>>, callback: impl Fn(&BrightnessSnapshot)) {
    init_mta();
    let (monitors, snap) = first_reading();
    callback(&snap);
    let Some(m) = monitors.filter(|_| snap.available) else { return };
    while let Ok(msg) = rx.recv() {
        if matches!(msg, Msg::Quit) {
            break;
        }
        if let Some(v) = queue.take() {
            m.write(v.clamp(snap.min, snap.max));
        }
    }
}

/// The physical monitors behind the primary display, released when dropped.
struct Monitors {
    all: Vec<PHYSICAL_MONITOR>,
}

impl Monitors {
    fn primary() -> Option<Monitors> {
        unsafe {
            let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
            let mut n = 0u32;
            GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n).ok()?;
            if n == 0 {
                return None;
            }
            let mut all = vec![PHYSICAL_MONITOR::default(); n as usize];
            GetPhysicalMonitorsFromHMONITOR(hmon, &mut all).ok()?;
            Some(Monitors { all })
        }
    }

    /// The first monitor that answers. (`GetMonitorCapabilities` is not asked: it fails on monitors whose brightness works.)
    fn read(&self) -> Option<BrightnessSnapshot> {
        for m in &self.all {
            let handle = m.hPhysicalMonitor;
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            // (a nonzero return is success)
            if unsafe { GetMonitorBrightness(handle, &mut min, &mut cur, &mut max) } != 0 && max > min {
                return Some(BrightnessSnapshot { available: true, min, value: cur, max });
            }
        }
        None
    }

    /// Sets every monitor that answered (there is normally one).
    fn write(&self, value: u32) {
        for m in &self.all {
            let handle = m.hPhysicalMonitor;
            unsafe {
                let _ = SetMonitorBrightness(handle, value);
            }
        }
    }
}

impl Drop for Monitors {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyPhysicalMonitors(&self.all);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_slider_and_the_monitors_range_map_both_ways() {
        // the two monitors measured on the development machine answered 0..=100; others use other ranges
        for (min, max) in [(0u32, 100u32), (10, 90), (0, 255), (5, 6)] {
            let s = |value| BrightnessSnapshot { available: true, min, value, max };
            assert_eq!(s(min).fraction(), 0.0);
            assert_eq!(s(max).fraction(), 1.0);
            for v in min..=max {
                assert_eq!(s(0).value_at(s(v).fraction()), v, "range {min}..={max}, value {v}");
            }
        }
    }

    #[test]
    fn a_value_out_of_range_is_cut_and_an_empty_range_does_not_divide_by_zero() {
        assert_eq!(to_fraction(0, 150, 100), 1.0);
        assert_eq!(to_fraction(10, 3, 100), 0.0);
        assert_eq!(to_fraction(50, 50, 50), 0.0);
        assert_eq!(from_fraction(0, 100, 2.0), 100);
        assert_eq!(from_fraction(0, 100, -1.0), 0);
        assert_eq!(from_fraction(0, 100, f32::NAN), 0);
        assert_eq!(from_fraction(50, 50, 0.5), 50);
        assert_eq!(from_fraction(20, 80, 0.5), 50);
    }

    #[test]
    fn a_monitor_that_does_not_answer_is_not_available() {
        assert!(!BrightnessSnapshot::default().available);
    }
}
