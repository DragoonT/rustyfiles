//! Switching a radio on and off (Wi-Fi, Bluetooth): WinRT `Radio`, which is what Windows' own quick settings and its Settings app
//! use, so the switch here and the switch there are the same switch.
//!
//! WAS NOT DONE at first. The notes of `network` and `bluetooth` said `Radio.SetStateAsync` was "not found solid for a program with
//! no package identity", and it had not been tried, because trying it changes the user's radios. Then the user asked for the
//! control centre's buttons to work (2026-11-15: "bluetooth ve ağ butonu da işlevli olsun tıklayınca sadece ayarları açmak
//! yerine"), and it was tried on a PC whose Wi-Fi was off and unused: see the test at the end, run by hand.
//!
//! A switch is the user's click and nothing else. It is made on a thread of its own ([`set`]): `RequestAccessAsync` and
//! `SetStateAsync` answer after a second or so, and waiting for a WinRT answer is not something the interface's thread may do.
//! Whoever follows the radio (`bluetooth`, `wifi`) hears of the new state from Windows (`Radio.StateChanged`).

use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind, RadioState};

use super::init_mta;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    WiFi,
    Bluetooth,
}

/// Where a radio is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Power {
    #[default]
    Off,
    On,
    /// Turned off by a hardware switch or a policy: it cannot be turned on from here.
    Disabled,
}

/// What a press on a radio's button asks for: on when it is off, off when it is on, nothing when it cannot be switched.
pub fn toggled(now: Power) -> Option<bool> {
    match now {
        Power::Off => Some(true),
        Power::On => Some(false),
        Power::Disabled => None,
    }
}

/// The machine's radio of that kind (blocks a moment; an MTA thread).
pub(crate) fn find(kind: Kind) -> Option<Radio> {
    let want = match kind {
        Kind::WiFi => RadioKind::WiFi,
        Kind::Bluetooth => RadioKind::Bluetooth,
    };
    Radio::GetRadiosAsync().ok()?.join().ok()?.into_iter().find(|r| r.Kind().ok() == Some(want))
}

pub(crate) fn power_of(radio: &Radio) -> Power {
    match radio.State().unwrap_or(RadioState::Unknown) {
        RadioState::On => Power::On,
        RadioState::Disabled => Power::Disabled,
        _ => Power::Off,
    }
}

/// Reads the radio once on the calling thread (`None`: the machine has none). Blocks.
pub fn read_once(kind: Kind) -> Option<Power> {
    init_mta();
    find(kind).map(|r| power_of(&r))
}

/// Switches the radio on the calling thread and waits for Windows' answer (an MTA thread; a second or two).
pub fn set_blocking(kind: Kind, on: bool) -> Result<(), String> {
    let allowed = |s: RadioAccessStatus, what: &str| if s == RadioAccessStatus::Allowed { Ok(()) } else { Err(format!("{what}: Windows said no ({})", s.0)) };
    let access = Radio::RequestAccessAsync().and_then(|op| op.join()).map_err(|e| e.to_string())?;
    allowed(access, "access to the radios")?;
    let radio = find(kind).ok_or("the machine has no such radio")?;
    let answer = radio.SetStateAsync(if on { RadioState::On } else { RadioState::Off }).and_then(|op| op.join()).map_err(|e| e.to_string())?;
    allowed(answer, "the switch")
}

/// The user pressed a radio's button: switches it on a thread of its own and returns at once.
pub fn set(kind: Kind, on: bool) {
    let spawned = std::thread::Builder::new().name("rf-radio".into()).spawn(move || {
        init_mta();
        if let Err(e) = set_blocking(kind, on) {
            log::warn!("radio: {kind:?} was not switched {}: {e}", if on { "on" } else { "off" });
        }
    });
    if let Err(e) = spawned {
        log::warn!("radio: the thread did not start: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_asks_for_the_other_state_and_for_nothing_when_the_radio_cannot_be_switched() {
        assert_eq!(toggled(Power::Off), Some(true));
        assert_eq!(toggled(Power::On), Some(false));
        assert_eq!(toggled(Power::Disabled), None);
    }

    /// On the PC it is run on: the Wi-Fi radio is switched to its other state and back. ONLY for a PC whose Wi-Fi is not what
    /// connects it (it goes off for a moment if it was on). Bluetooth is never tried this way: a keyboard and a mouse hang on it.
    #[test]
    #[ignore = "switches the Wi-Fi radio of the PC it runs on, and back; run by hand"]
    fn the_wifi_radio_is_switched_and_put_back() {
        let Some(was) = read_once(Kind::WiFi) else { return };
        let Some(other) = toggled(was) else { return };
        set_blocking(Kind::WiFi, other).expect("the switch");
        let now = read_once(Kind::WiFi);
        set_blocking(Kind::WiFi, !other).expect("the switch back");
        assert_eq!(now, Some(if other { Power::On } else { Power::Off }));
        assert_eq!(read_once(Kind::WiFi), Some(was));
    }
}
