//! The battery: percent, charging, battery saver. `GetSystemPowerStatus` for the reading and `RegisterPowerSettingNotification` for
//! the changes, in its CALLBACK form (no window needed; Windows calls a function of ours on a thread of its own). The machine this
//! was written on is a desktop with no battery: `present` is false there and the panel leaves the tile out (and registers nothing).
//!
//! One follower at a time (the panel): the callback has no context pointer to free, it reads the one slot [`SINK`], so a callback
//! that is still running while the follower leaves cannot use freed memory.

use std::ffi::c_void;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, GetSystemPowerStatus, HPOWERNOTIFY, RegisterPowerSettingNotification, SYSTEM_POWER_STATUS, UnregisterPowerSettingNotification,
};
use windows::core::GUID;

use super::Subscription;

/// What the interface draws the battery from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct BatterySnapshot {
    /// The machine has a battery.
    pub present: bool,
    /// 0..=100; `None`: Windows does not know.
    pub percent: Option<u8>,
    pub charging: bool,
    /// Plugged in (it may be full, not charging).
    pub on_ac: bool,
    /// Battery saver is on.
    pub saver: bool,
    /// Minutes left on the battery, when Windows says.
    pub minutes_left: Option<u32>,
}

/// The pure part: a `SYSTEM_POWER_STATUS` as its fields.
pub fn from_status(ac_line: u8, battery_flag: u8, percent: u8, system_flag: u8, seconds_left: u32) -> BatterySnapshot {
    // BatteryFlag: 128 no system battery, 255 unknown; bit 8 (value 8) charging
    let present = battery_flag != 255 && battery_flag & 128 == 0;
    if !present {
        return BatterySnapshot::default();
    }
    BatterySnapshot {
        present: true,
        percent: (percent <= 100).then_some(percent),
        charging: battery_flag & 8 != 0,
        on_ac: ac_line == 1,
        saver: system_flag & 1 != 0,
        // 0xFFFFFFFF: unknown (also while on AC)
        minutes_left: (seconds_left != u32::MAX).then_some(seconds_left / 60),
    }
}

/// Reads the battery now (cheap; no blocking to speak of).
pub fn read() -> BatterySnapshot {
    let mut s = SYSTEM_POWER_STATUS::default();
    if unsafe { GetSystemPowerStatus(&mut s) }.is_err() {
        return BatterySnapshot::default();
    }
    from_status(s.ACLineStatus, s.BatteryFlag, s.BatteryLifePercent, s.SystemStatusFlag, s.BatteryLifeTime)
}

type Sink = Arc<dyn Fn(BatterySnapshot) + Send + Sync>;

static SINK: Mutex<Option<Sink>> = Mutex::new(None);

/// Makes `sink` the one that is told (it replaces an earlier follower's).
fn install(slot: &Mutex<Option<Sink>>, sink: Sink) {
    *slot.lock() = Some(sink);
}

/// A follower going away clears the slot only if the slot still holds ITS sink: a newer follower that has replaced it (the control
/// centre opened again while the old one was still being put away) keeps its own.
fn release(slot: &Mutex<Option<Sink>>, mine: &Sink) {
    let mut g = slot.lock();
    if g.as_ref().is_some_and(|s| Arc::ptr_eq(s, mine)) {
        *g = None;
    }
}

/// Windows' callback. The system calls it on a thread of its own; it reads the battery again and tells the follower.
unsafe extern "system" fn on_power(_context: *const c_void, _kind: u32, _setting: *const c_void) -> u32 {
    let sink = SINK.lock().clone();
    if let Some(f) = sink {
        f(read());
    }
    0
}

/// `GUID_BATTERY_PERCENTAGE_REMAINING`, `GUID_ACDC_POWER_SOURCE`, `GUID_POWER_SAVING_STATUS`.
const SETTINGS: [GUID; 3] = [
    GUID::from_u128(0xa7ad8041_b45a_4cae_87a3_eecbb468a9e1),
    GUID::from_u128(0x5d3e9a59_e9d5_4b00_a6bd_ff34ff516548),
    GUID::from_u128(0xe00958c0_c213_4ace_ac77_fecced2eeea5),
];

/// The notification flag for "the recipient is a callback" (`DEVICE_NOTIFY_CALLBACK`).
const DEVICE_NOTIFY_CALLBACK: u32 = 2;

/// Follows the battery: `callback` is told the reading when the percentage, the power source or battery saver changes (from a thread
/// of Windows'). Not called at once: [`read`] is cheap, the caller reads first. Registers nothing on a machine without a battery.
/// One follower at a time; a second replaces the first.
pub fn subscribe(callback: impl Fn(BatterySnapshot) + Send + Sync + 'static) -> Subscription {
    if !read().present {
        return Subscription::new(|| {});
    }
    let mine: Sink = Arc::new(callback);
    install(&SINK, mine.clone());
    // The parameters are what the "recipient" handle points at; Windows may read them again later, so they live for the process (one
    // small block, made once; they only name the callback and carry no context).
    static PARAMS: OnceLock<usize> = OnceLock::new();
    let params = *PARAMS.get_or_init(|| Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: std::ptr::null_mut() })) as usize);
    let mut handles: Vec<isize> = Vec::new();
    for g in &SETTINGS {
        let recipient = HANDLE(params as *mut c_void);
        match unsafe { RegisterPowerSettingNotification(recipient, g, windows::Win32::UI::WindowsAndMessaging::REGISTER_NOTIFICATION_FLAGS(DEVICE_NOTIFY_CALLBACK)) } {
            Ok(h) => handles.push(h.0),
            Err(e) => log::debug!("power: no notification for {g:?}: {e}"),
        }
    }
    Subscription::new(move || {
        for h in handles {
            unsafe {
                let _ = UnregisterPowerSettingNotification(HPOWERNOTIFY(h));
            }
        }
        release(&SINK, &mine);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_has_no_battery() {
        // AC on, "no system battery" (128), percent unknown (255), no saver, no time
        assert_eq!(from_status(1, 128, 255, 0, u32::MAX), BatterySnapshot::default());
        assert!(!from_status(1, 128, 255, 0, u32::MAX).present);
    }

    #[test]
    fn a_laptop_on_battery_and_on_the_charger() {
        // on battery, 63 %, high (1), 2 h 5 min left
        let a = from_status(0, 1, 63, 0, 7500);
        assert_eq!(a, BatterySnapshot { present: true, percent: Some(63), charging: false, on_ac: false, saver: false, minutes_left: Some(125) });
        // plugged in and charging (flag 8 | 1), no time known
        let b = from_status(1, 9, 80, 0, u32::MAX);
        assert!(b.present && b.charging && b.on_ac);
        assert_eq!(b.minutes_left, None);
        // plugged in, full, not charging
        let c = from_status(1, 1, 100, 0, u32::MAX);
        assert!(c.present && !c.charging && c.on_ac);
    }

    #[test]
    fn an_old_follower_going_away_does_not_silence_the_one_that_replaced_it() {
        let slot: Mutex<Option<Sink>> = Mutex::new(None);
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mk = |h: &Arc<std::sync::atomic::AtomicUsize>| -> Sink {
            let h = h.clone();
            Arc::new(move |_| {
                h.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
        };
        let (old, new) = (mk(&hits), mk(&hits));
        install(&slot, old.clone());
        // the panel is opened again before the first one's subscription is dropped
        install(&slot, new.clone());
        release(&slot, &old);
        assert!(slot.lock().is_some(), "the new follower is still there");
        release(&slot, &new);
        assert!(slot.lock().is_none());
        // releasing twice, or from an empty slot, is harmless
        release(&slot, &new);
    }

    #[test]
    fn battery_saver_is_a_flag() {
        assert!(from_status(0, 2, 18, 1, 600).saver);
        assert!(!from_status(0, 2, 18, 0, 600).saver);
    }

    #[test]
    fn a_battery_that_does_not_know_its_level_has_no_percent() {
        // flag 255 (unknown) with percent 255 is "no battery information": treated as none; a present battery with an unreadable
        // percent is a battery without a number
        assert!(!from_status(1, 255, 255, 0, u32::MAX).present);
        let s = from_status(0, 1, 255, 0, u32::MAX);
        assert!(s.present && s.percent.is_none());
    }
}
