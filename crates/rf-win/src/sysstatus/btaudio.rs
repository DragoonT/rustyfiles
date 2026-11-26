//! Bluetooth devices: where a device's address stands in the names Windows gives things, its battery level, and connecting or
//! disconnecting the SOUND of a headset or a speaker.
//!
//! WHY HERE AND NOT IN `bluetooth`. That module is WinRT and tells who is paired and who is connected. What is here Windows tells
//! through two older doors, and both go by the device's ADDRESS (twelve hex digits), which is the one thing all the names share:
//!
//! * THE BATTERY is a property of the device's node in the device tree (the same number Settings shows beside a mouse or a
//!   headset), read with SetupAPI from the nodes of the Bluetooth class. Only devices that report one have it.
//! * CONNECT / DISCONNECT. Windows has no call that connects "a Bluetooth device"; a device is connected while one of its services
//!   is in use. For sound, the Bluetooth audio driver takes two requests on the audio endpoint's own filter, "reconnect" and
//!   "disconnect" (the kernel streaming property set `KSPROPSETID_BtAudio`): the Connect button of Windows' sound panel sends the
//!   first. So a headset or a speaker can be connected and let go from here; a mouse or a keyboard connects by itself and has no
//!   such door. An audio endpoint belongs to a device when the id of the filter it hangs on carries the device's address.
//!
//! Everything here blocks for a moment: off the interface's thread. The two requests are the user's click and nothing else.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    DIGCF_PRESENT, GUID_DEVCLASS_BLUETOOTH, HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
    SetupDiGetDevicePropertyW,
};
use windows::Win32::Devices::Properties::{DEVPROP_TYPE_BYTE, DEVPROPTYPE};
use windows::Win32::Foundation::DEVPROPKEY;
use windows::Win32::Media::Audio::{DEVICE_STATE, DEVICE_STATE_ACTIVE, DEVICE_STATE_UNPLUGGED, IDeviceTopology, IMMDeviceEnumerator, MMDeviceEnumerator, eAll};
use windows::Win32::Media::KernelStreaming::{IKsControl, KSIDENTIFIER, KSIDENTIFIER_0, KSIDENTIFIER_0_0, KSPROPERTY_TYPE_GET};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::core::{GUID, PCWSTR};

use super::init_mta;

// ------------------------------------------------------------------ pure: the address in a name

fn is_hex12(s: &[u8]) -> bool {
    s.len() == 12 && s.iter().all(u8::is_ascii_hexdigit)
}

/// A device's address from the id WinRT gives it ("Bluetooth#Bluetooth00:1a:7d:da:71:13-aa:bb:cc:dd:ee:ff": the part after the
/// dash), as twelve capital hex digits.
pub fn address_of_winrt_id(id: &str) -> Option<String> {
    let tail = id.rsplit('-').next()?;
    let hex: String = tail.chars().filter(|c| *c != ':').collect();
    (tail.len() == 17 && is_hex12(hex.as_bytes())).then(|| hex.to_ascii_uppercase())
}

/// A device's address in a device-tree id or an audio filter's id. It is the twelve hex digits that stand after "DEV_" (a low
/// energy device: `BTHLE\DEV_AABBCCDDEEFF\...`) or after the last "&" of the instance part (a classic one:
/// `BTHENUM\{service}_VID&..\7&1ebf1d05&0&AABBCCDDEEFF_C00000000`), and are followed by something that is no letter or digit. The
/// twelve digits that end a GUID are not it (a dash stands before them).
pub fn address_in(id: &str) -> Option<String> {
    let b = id.as_bytes();
    let mut found = None;
    for i in 0..b.len().saturating_sub(11) {
        if !is_hex12(&b[i..i + 12]) || b.get(i + 12).is_some_and(u8::is_ascii_alphanumeric) {
            continue;
        }
        let before = &b[..i];
        if before.last() == Some(&b'&') || (before.len() >= 4 && before[before.len() - 4..].eq_ignore_ascii_case(b"dev_")) {
            found = Some(id[i..i + 12].to_ascii_uppercase());
        }
    }
    found
}

// ------------------------------------------------------------------ the battery

/// The battery level of a Bluetooth device's node, in percent (a byte).
const BATTERY: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0x104ea319_6ee2_4701_bd47_8ddbf425bbe5), pid: 2 };

struct DevInfo(HDEVINFO);

impl Drop for DevInfo {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

/// The battery levels the Bluetooth devices that are there report: (address, percent). A device with several nodes that report
/// one is listed once.
pub fn battery_levels() -> Vec<(String, u8)> {
    let mut out: Vec<(String, u8)> = Vec::new();
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_BLUETOOTH), PCWSTR::null(), None, DIGCF_PRESENT) else { return out };
        let set = DevInfo(set);
        for index in 0.. {
            let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set.0, index, &mut dev).is_err() {
                break;
            }
            let (mut kind, mut level) = (DEVPROPTYPE::default(), [0u8; 1]);
            if SetupDiGetDevicePropertyW(set.0, &dev, &BATTERY, &mut kind, Some(&mut level), None, 0).is_err() || kind != DEVPROP_TYPE_BYTE {
                continue;
            }
            let mut id = [0u16; 512];
            if SetupDiGetDeviceInstanceIdW(set.0, &dev, Some(&mut id), None).is_err() {
                continue;
            }
            let id = String::from_utf16_lossy(&id[..id.iter().position(|c| *c == 0).unwrap_or(id.len())]);
            if let Some(address) = address_in(&id)
                && !out.iter().any(|(a, _)| *a == address)
            {
                out.push((address, level[0].min(100)));
            }
        }
    }
    out
}

// ------------------------------------------------------------------ the version of the adapter

/// The radio's LMP version (a byte), on the node of the Bluetooth adapter itself. It is not in Windows' published headers: it is the
/// value Device Manager shows on the adapter's Advanced tab, and the one tools that tell a PC's Bluetooth version read (checked on the
/// owner's PC, 2026-11-26: 13 on a Qualcomm FastConnect 7800, a Bluetooth 5.4 adapter).
const LMP_VERSION: DEVPROPKEY = DEVPROPKEY { fmtid: GUID::from_u128(0xa92f26ca_eda7_4b1d_9db2_27b68aa5a2eb), pid: 4 };

/// The LMP version of the Bluetooth adapter that is there: the first node of the Bluetooth class that carries one.
pub fn adapter_lmp_version() -> Option<u8> {
    unsafe {
        let set = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_BLUETOOTH), PCWSTR::null(), None, DIGCF_PRESENT).ok()?;
        let set = DevInfo(set);
        for index in 0.. {
            let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set.0, index, &mut dev).is_err() {
                break;
            }
            let (mut kind, mut v) = (DEVPROPTYPE::default(), [0u8; 1]);
            if SetupDiGetDevicePropertyW(set.0, &dev, &LMP_VERSION, &mut kind, Some(&mut v), None, 0).is_ok() && kind == DEVPROP_TYPE_BYTE {
                return Some(v[0]);
            }
        }
    }
    None
}

/// The Bluetooth version an LMP version stands for (the core specification's own table): 6 is 4.0, 9 is 5.0, 13 is 5.4, 14 is 6.0.
/// Older and unknown numbers give `None`.
pub fn version_of_lmp(lmp: u8) -> Option<&'static str> {
    Some(match lmp {
        3 => "2.0",
        4 => "2.1",
        5 => "3.0",
        6 => "4.0",
        7 => "4.1",
        8 => "4.2",
        9 => "5.0",
        10 => "5.1",
        11 => "5.2",
        12 => "5.3",
        13 => "5.4",
        14 => "6.0",
        15 => "6.1",
        _ => return None,
    })
}

// ------------------------------------------------------------------ the sound of a device

const BT_AUDIO: GUID = GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
const ONESHOT_RECONNECT: u32 = 0;
const ONESHOT_DISCONNECT: u32 = 1;

/// The Bluetooth audio endpoints (there or unplugged): the address of the device each belongs to, and the filter that takes the
/// two requests.
fn endpoints() -> Vec<(String, IKsControl)> {
    let mut out = Vec::new();
    init_mta();
    unsafe {
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(list) = en.EnumAudioEndpoints(eAll, DEVICE_STATE(DEVICE_STATE_ACTIVE.0 | DEVICE_STATE_UNPLUGGED.0)) else { return out };
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(endpoint) = list.Item(i) else { continue };
            let Ok(topology) = endpoint.Activate::<IDeviceTopology>(CLSCTX_ALL, None) else { continue };
            let Ok(connector) = topology.GetConnector(0) else { continue };
            let Ok(raw) = connector.GetDeviceIdConnectedTo() else { continue };
            let id = raw.to_string().unwrap_or_default();
            let filter = en.GetDevice(PCWSTR(raw.0));
            CoTaskMemFree(Some(raw.0 as *const _));
            // (only a filter of the Bluetooth audio drivers: its id names the Bluetooth enumerator and carries the address)
            if !id.to_ascii_lowercase().contains("bth") {
                continue;
            }
            let Some(address) = address_in(&id) else { continue };
            let Ok(control) = filter.and_then(|f| f.Activate::<IKsControl>(CLSCTX_ALL, None)) else { continue };
            out.push((address, control));
        }
    }
    out
}

/// The addresses of the devices that have sound here (a headset, a speaker): the ones [`set_connected`] can act on. Each once.
pub fn audio_addresses() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for (address, _) in endpoints() {
        if !out.contains(&address) {
            out.push(address);
        }
    }
    out
}

/// The user clicked a headset or a speaker in the list: asks the Bluetooth audio driver to connect its sound, or to let it go. How
/// many of the device's endpoints took the request (a headset has two: music, and calls); an error when none did.
pub fn set_connected(address: &str, connect: bool) -> Result<usize, String> {
    let request = KSIDENTIFIER { Anonymous: KSIDENTIFIER_0 { Anonymous: KSIDENTIFIER_0_0 { Set: BT_AUDIO, Id: if connect { ONESHOT_RECONNECT } else { ONESHOT_DISCONNECT }, Flags: KSPROPERTY_TYPE_GET } } };
    let (mut taken, mut refused) = (0usize, None);
    for (_, control) in endpoints().into_iter().filter(|(a, _)| a.eq_ignore_ascii_case(address)) {
        let mut returned = 0u32;
        match unsafe { control.KsProperty(&request, size_of::<KSIDENTIFIER>() as u32, std::ptr::null_mut(), 0, &mut returned) } {
            Ok(()) => taken += 1,
            Err(e) => refused = Some(e.to_string()),
        }
    }
    if taken > 0 { Ok(taken) } else { Err(refused.unwrap_or_else(|| "the device has no sound endpoint here".to_string())) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The core specification's table of LMP versions (the control centre's Bluetooth title, 2026-11-26).
    #[test]
    fn an_lmp_version_names_its_bluetooth_version() {
        use super::version_of_lmp;
        assert_eq!([6, 9, 12, 13, 14].map(version_of_lmp), [Some("4.0"), Some("5.0"), Some("5.3"), Some("5.4"), Some("6.0")]);
        assert_eq!([0, 2, 16, 255].map(version_of_lmp), [None; 4]);
    }

    #[test]
    fn the_address_is_found_in_every_name_windows_gives_a_device() {
        // WinRT's id: adapter, dash, device
        assert_eq!(address_of_winrt_id("Bluetooth#Bluetooth00:1a:7d:da:71:13-94:db:56:ab:cd:ef").as_deref(), Some("94DB56ABCDEF"));
        assert_eq!(address_of_winrt_id("BluetoothLE#BluetoothLE00:1a:7d:da:71:13-f0:0d:ca:fe:00:01").as_deref(), Some("F00DCAFE0001"));
        assert_eq!(address_of_winrt_id("Bluetooth#Bluetooth00:1a:7d:da:71:13"), None);
        assert_eq!(address_of_winrt_id(""), None);
        assert_eq!(address_of_winrt_id("x-94:db:56:ab:cd:zz"), None);
        // the device tree: a low energy device, a classic device's service, its hands-free service
        assert_eq!(address_in(r"BTHLE\DEV_F00DCAFE0001\8&2A3B4C5D&0&F00DCAFE0001").as_deref(), Some("F00DCAFE0001"));
        assert_eq!(address_in(r"BTHLE\DEV_f00dcafe0001\8&2a3b4c5d&0").as_deref(), Some("F00DCAFE0001"));
        assert_eq!(address_in(r"BTHENUM\{0000110B-0000-1000-8000-00805F9B34FB}_VID&0002054C_PID&0D58\7&1EBF1D05&0&94DB56ABCDEF_C00000000").as_deref(), Some("94DB56ABCDEF"));
        assert_eq!(address_in(r"BTHENUM\{0000111E-0000-1000-8000-00805F9B34FB}_VID&0001004C_PID&792D\7&3A1&0&AABBCCDDEEFF_C00000000").as_deref(), Some("AABBCCDDEEFF"));
        // an audio filter's id: the GUIDs' own twelve digits are not the address, before it or after it
        assert_eq!(
            address_in(r"{2}.\\?\bthenum#{0000110b-0000-1000-8000-00805f9b34fb}_vid&0002054c_pid&0d58#7&1ebf1d05&0&94db56abcdef_c00000000#{6994ad04-93ef-11d0-a3cc-00a0c9223196}\snk").as_deref(),
            Some("94DB56ABCDEF")
        );
        // things that are no Bluetooth device's id
        for none in [r"BTH\MS_BTHLE\7&1EBF1D05&0&4", r"SWD\MMDEVAPI\{0.0.0.00000000}.{6994ad04-93ef-11d0-a3cc-00a0c9223196}", r"USB\VID_046D&PID_C52B\5&1234", "", "DEV_12345"] {
            assert_eq!(address_in(none), None, "{none}");
        }
        // thirteen digits are not twelve
        assert_eq!(address_in(r"X\DEV_F00DCAFE00012\1"), None);
    }

    /// On the PC it is run on, by hand: how many devices report a battery level and how many have sound. Reads only.
    #[test]
    #[ignore = "reads the Bluetooth devices of the PC it runs on; run by hand"]
    fn what_this_pcs_devices_say() {
        let batteries = battery_levels();
        println!("with a battery level: {} (the levels: {:?}); with sound: {}", batteries.len(), batteries.iter().map(|(_, l)| *l).collect::<Vec<_>>(), audio_addresses().len());
    }
}
