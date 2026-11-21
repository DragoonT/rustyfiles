//! EVERY DISPLAY ([`open_all`], what the control centre uses): the same for each display Windows lists, the primary one first. A
//! display that does not answer is not in the panel; the others are. ([`open`] is the one display this module began with, kept for
//! the probe and the tests.)
//!
//! WHICH DISPLAYS ANSWER (the user asked, 2026-11-15: "monitör parlaklığı ayarı sadece benim monitördemi çoğu genel monitörde mi
//! çalışır?"). DDC/CI is a standard nearly every desktop monitor of the last fifteen years has, over DisplayPort, HDMI and DVI. It
//! is not there when the monitor's own menu has "DDC/CI" switched off, behind some KVM switches, docks and cheap adapters that do
//! not pass it on, on most televisions, and on a LAPTOP'S OWN PANEL, which has no such channel at all. A laptop's panel is asked
//! the way Windows' own brightness slider asks it instead: the display driver's backlight requests on the device `\\.\LCD`
//! ([`Lcd`]). It is the first in the list when it answers. That path is written from the documented requests and has never been
//! run on a laptop (the development PC is a desktop, where the device does not answer and nothing is listed for it).
//!
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

use windows::Win32::Devices::Display::{DISPLAY_BRIGHTNESS, IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS, IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS};
use windows::Win32::Devices::Display::{DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR, GetPhysicalMonitorsFromHMONITOR, PHYSICAL_MONITOR, SetMonitorBrightness};
use windows::Win32::Devices::Display::{GetVCPFeatureAndVCPFeatureReply, SetVCPFeature};
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, LPARAM, POINT, RECT};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR, MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::core::BOOL;

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

// ------------------------------------------------------------------ every display

/// The open displays. Dropping it ends the thread.
pub struct Screens {
    /// The newest value asked for each display (by its place in the list the callback was given).
    queue: Arc<std::sync::Mutex<Vec<Option<u32>>>>,
    wake: std::sync::mpsc::Sender<Msg>,
    _end: Subscription,
}

impl Screens {
    /// The user moved the slider of display `index` to this raw value. Returns at once; the newest value of a drag is the one written.
    pub fn set(&self, index: usize, value: u32) {
        if let Ok(mut q) = self.queue.lock() {
            if q.len() <= index {
                q.resize(index + 1, None);
            }
            q[index] = Some(value);
        }
        let _ = self.wake.send(Msg::Write);
    }
}

/// The displays in the order of the panel's rows: the primary one first, the others as Windows lists them.
pub fn primary_first<T: PartialEq + Copy>(all: Vec<T>, primary: T) -> Vec<T> {
    let mut out: Vec<T> = all.iter().copied().filter(|m| *m == primary).take(1).collect();
    out.extend(all.into_iter().filter(|m| *m != primary));
    out
}

unsafe extern "system" fn collect_display(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
    // SAFETY: `list` is the vector `displays` passes for the time of the call
    unsafe { (*(list.0 as *mut Vec<isize>)).push(monitor.0 as isize) };
    BOOL(1)
}

fn displays() -> Vec<isize> {
    let mut all: Vec<isize> = Vec::new();
    let primary = unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect_display), LPARAM(&mut all as *mut Vec<isize> as isize));
        MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY).0 as isize
    };
    primary_first(all, primary)
}

/// A laptop's own panel: its backlight, through the display driver (see the module docs). The level is a percent.
struct Lcd(HANDLE);

/// The level a panel is at: it keeps one for running on mains and one for running on the battery, and shows the one of now.
pub fn lcd_level(on_mains: u8, on_battery: u8, mains_now: bool) -> u32 {
    u32::from(if mains_now { on_mains } else { on_battery }.min(100))
}

impl Lcd {
    fn open() -> Option<Lcd> {
        unsafe { CreateFileW(windows::core::w!(r"\\.\LCD"), (GENERIC_READ | GENERIC_WRITE).0, FILE_SHARE_READ | FILE_SHARE_WRITE, None, OPEN_EXISTING, FILE_FLAGS_AND_ATTRIBUTES(0), None) }.ok().map(Lcd)
    }

    fn read(&self) -> Option<BrightnessSnapshot> {
        let (mut levels, mut got) = (DISPLAY_BRIGHTNESS::default(), 0u32);
        unsafe { DeviceIoControl(self.0, IOCTL_VIDEO_QUERY_DISPLAY_BRIGHTNESS, None, 0, Some(&mut levels as *mut _ as *mut _), size_of::<DISPLAY_BRIGHTNESS>() as u32, Some(&mut got), None) }.ok()?;
        let mut power = SYSTEM_POWER_STATUS::default();
        let mains = unsafe { GetSystemPowerStatus(&mut power) }.is_ok() && power.ACLineStatus == 1;
        Some(BrightnessSnapshot { available: true, min: 0, value: lcd_level(levels.ucACBrightness, levels.ucDCBrightness, mains), max: 100 })
    }

    /// Sets the level for both ways of running (as Windows' own slider does).
    fn write(&self, value: u32) {
        const BOTH: u8 = 3;
        let level = value.min(100) as u8;
        let (levels, mut got) = (DISPLAY_BRIGHTNESS { ucDisplayPolicy: BOTH, ucACBrightness: level, ucDCBrightness: level }, 0u32);
        unsafe {
            let _ = DeviceIoControl(self.0, IOCTL_VIDEO_SET_DISPLAY_BRIGHTNESS, Some(&levels as *const _ as *const _), size_of::<DISPLAY_BRIGHTNESS>() as u32, None, 0, Some(&mut got), None);
        }
    }
}

impl Drop for Lcd {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Reads every display's brightness on a thread of its own and tells `callback` once (one snapshot per display, `available: false`
/// for one that does not answer), then keeps the displays for [`Screens::set`].
pub fn open_all(callback: impl Fn(&[BrightnessSnapshot]) + Send + 'static) -> Screens {
    let queue = Arc::new(std::sync::Mutex::new(Vec::new()));
    let (tx, rx) = channel::<Msg>();
    let q = queue.clone();
    let spawned = std::thread::Builder::new().name("rf-brightness".into()).spawn(move || {
        init_mta();
        // (each display gets the tries of the first reading: `TRIES`)
        // (a laptop's own panel first, when it answers; then the displays that are asked over DDC/CI)
        let lcd = Lcd::open().and_then(|l| l.read().map(|s| (l, s)));
        let opened: Vec<(Option<Monitors>, BrightnessSnapshot)> = displays().into_iter().map(|d| reading_of(|| Monitors::of(HMONITOR(d as *mut _)))).collect();
        let snaps: Vec<BrightnessSnapshot> = lcd.iter().map(|(_, s)| *s).chain(opened.iter().map(|(_, s)| *s)).collect();
        let first_ddc = usize::from(lcd.is_some());
        callback(&snaps);
        if !snaps.iter().any(|s| s.available) {
            return;
        }
        while let Ok(msg) = rx.recv() {
            if matches!(msg, Msg::Quit) {
                break;
            }
            let asked: Vec<Option<u32>> = q.lock().map(|mut q| q.iter_mut().map(Option::take).collect()).unwrap_or_default();
            for (i, v) in asked.into_iter().enumerate() {
                let Some(v) = v else { continue };
                if i < first_ddc {
                    if let Some((panel, snap)) = &lcd {
                        panel.write(v.clamp(snap.min, snap.max));
                    }
                } else if let Some((Some(m), snap)) = opened.get(i - first_ddc)
                    && snap.available
                {
                    m.write(v.clamp(snap.min, snap.max));
                }
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("brightness: the thread did not start: {e}");
    }
    let quit = tx.clone();
    Screens { queue, wake: tx, _end: Subscription::new(move || { let _ = quit.send(Msg::Quit); }) }
}

/// A display and its brightness, with the tries of a first reading.
fn reading_of(open: impl Fn() -> Option<Monitors>) -> (Option<Monitors>, BrightnessSnapshot) {
    let mut monitors = open();
    let mut snap = BrightnessSnapshot::default();
    for attempt in 0..TRIES {
        if attempt > 0 {
            std::thread::sleep(RETRY_AFTER);
            monitors = open();
        }
        snap = monitors.as_ref().and_then(|m| m.read()).unwrap_or_default();
        if snap.available {
            break;
        }
    }
    (monitors, snap)
}

/// The physical monitors behind a display, released when dropped.
///
/// HOW A MONITOR IS ASKED (issue 154: a Samsung Odyssey G5 restarted itself a few seconds after the slider moved, and its brightness
/// did not change). The monitor is asked for its luminance control (VCP code 0x10) directly, one short transfer each way, as
/// Monitorian and Twinkle Tray do. Windows' "high-level" brightness calls are only the fallback: they first read the monitor's whole
/// capabilities string, a long transfer some monitors' firmware does not survive when it comes again and again. Only the monitor that
/// answered is written to, never faster than one transfer per [`WRITE_GAP`] (the DDC/CI standard asks a host to wait 50 ms after a
/// write; some monitors need more).
struct Monitors {
    all: Vec<PHYSICAL_MONITOR>,
    /// The monitor that answered the reading, and how (`true`: the luminance control itself).
    answered: std::cell::Cell<Option<(usize, bool)>>,
    /// When the last write was sent.
    last_write: std::cell::Cell<Option<std::time::Instant>>,
}

/// The shortest time between two writes to a monitor.
const WRITE_GAP: std::time::Duration = std::time::Duration::from_millis(150);

/// The luminance control of the DDC/CI command set.
const VCP_LUMINANCE: u8 = 0x10;

impl Monitors {
    fn primary() -> Option<Monitors> {
        Monitors::of(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
    }

    fn of(hmon: HMONITOR) -> Option<Monitors> {
        unsafe {
            let mut n = 0u32;
            GetNumberOfPhysicalMonitorsFromHMONITOR(hmon, &mut n).ok()?;
            if n == 0 {
                return None;
            }
            let mut all = vec![PHYSICAL_MONITOR::default(); n as usize];
            GetPhysicalMonitorsFromHMONITOR(hmon, &mut all).ok()?;
            Some(Monitors { all, answered: Default::default(), last_write: Default::default() })
        }
    }

    /// The first monitor that answers: its luminance control, else Windows' high-level call. (`GetMonitorCapabilities` is not asked:
    /// it fails on monitors whose brightness works.)
    fn read(&self) -> Option<BrightnessSnapshot> {
        for (i, m) in self.all.iter().enumerate() {
            let handle = m.hPhysicalMonitor;
            let (mut cur, mut max) = (0u32, 0u32);
            // (a nonzero return is success)
            if unsafe { GetVCPFeatureAndVCPFeatureReply(handle, VCP_LUMINANCE, None, &mut cur, Some(&mut max)) } != 0 && max > 0 {
                self.answered.set(Some((i, true)));
                return Some(BrightnessSnapshot { available: true, min: 0, value: cur.min(max), max });
            }
        }
        for (i, m) in self.all.iter().enumerate() {
            let handle = m.hPhysicalMonitor;
            let (mut min, mut cur, mut max) = (0u32, 0u32, 0u32);
            if unsafe { GetMonitorBrightness(handle, &mut min, &mut cur, &mut max) } != 0 && max > min {
                self.answered.set(Some((i, false)));
                return Some(BrightnessSnapshot { available: true, min, value: cur, max });
            }
        }
        None
    }

    /// Sets the monitor that answered the reading, the way it answered, after the gap since the last write.
    fn write(&self, value: u32) {
        let Some((i, direct)) = self.answered.get() else { return };
        let Some(m) = self.all.get(i) else { return };
        if let Some(wait) = self.last_write.get().and_then(|t| WRITE_GAP.checked_sub(t.elapsed())) {
            std::thread::sleep(wait);
        }
        let handle = m.hPhysicalMonitor;
        unsafe {
            if direct {
                let _ = SetVCPFeature(handle, VCP_LUMINANCE, value);
            } else {
                let _ = SetMonitorBrightness(handle, value);
            }
        }
        self.last_write.set(Some(std::time::Instant::now()));
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
    fn a_laptops_panel_shows_the_level_of_how_it_runs_now() {
        assert_eq!(lcd_level(80, 40, true), 80);
        assert_eq!(lcd_level(80, 40, false), 40);
        assert_eq!(lcd_level(250, 0, true), 100, "a level past the top is the top");
    }

    #[test]
    fn the_primary_display_is_the_first_row_and_the_others_keep_their_order() {
        assert_eq!(primary_first(vec![7, 3, 9], 3), [3, 7, 9]);
        assert_eq!(primary_first(vec![3, 7, 9], 3), [3, 7, 9]);
        assert_eq!(primary_first(vec![7, 9], 3), [7, 9], "a primary that is not listed adds nothing");
        assert_eq!(primary_first(Vec::<i32>::new(), 3), Vec::<i32>::new());
    }

    /// On the PC it is run on, by hand: how many displays there are and how many answer DDC/CI. Reads only.
    #[test]
    #[ignore = "asks the monitors of the PC it runs on for their brightness; run by hand"]
    fn what_this_pcs_displays_say() {
        let (tx, rx) = channel();
        let screens = open_all(move |list| {
            let _ = tx.send(list.to_vec());
        });
        let list = rx.recv_timeout(std::time::Duration::from_secs(8)).expect("a reading");
        drop(screens);
        println!("displays {} answering {}", list.len(), list.iter().filter(|s| s.available).count());
    }

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
