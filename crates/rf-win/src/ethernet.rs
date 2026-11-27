//! The wired network cards of this PC, on or off, and turning one on or off (the user, 2026-11-27: "sağ alttaki panelde ethernet
//! bağlantısını aktif deaktif edebilelim").
//!
//! WHY NOT THE NETWORK LIST. A card that is turned off is gone from the IP Helper's tables (`GetIfTable2`, `GetAdaptersAddresses`), so
//! a tile built from them would vanish with the card and could never turn it back on. The cards are read from the device tree
//! instead (SetupAPI, class Net): a turned-off card is still there, with the problem "disabled". Which of them are wired cards, and
//! their names as Windows shows them ("Ethernet", "Ethernet 2"), come from their driver keys and the network settings in the
//! registry. Reading needs no rights.
//!
//! TURNING A CARD ON OR OFF needs an administrator, as in Windows' own Network Connections: `netsh interface set interface` is run
//! with "runas", so Windows asks the user each time (UAC). Nothing else here asks for rights. Both block: off the interface's thread.

use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_DEVNODE_STATUS_FLAGS, CM_Get_DevNode_Status, CM_PROB, CM_PROB_DISABLED, CR_SUCCESS, DICS_FLAG_GLOBAL, DIGCF_PRESENT, DIREG_DRV, GUID_DEVCLASS_NET,
    HDEVINFO, SP_DEVINFO_DATA, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo, SetupDiGetClassDevsW, SetupDiOpenDevRegKey,
};
use windows::Win32::System::Registry::{HKEY, HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegCloseKey, RegGetValueW};
use windows::core::{PCWSTR, w};

/// A wired network card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    /// Its name as Windows shows it in Network Connections ("Ethernet"), which `netsh` takes.
    pub name: String,
    /// Turned on (not disabled).
    pub enabled: bool,
}

/// `*IfType` of an Ethernet card (IF_TYPE_ETHERNET_CSMACD), and `Characteristics`' NCF_PHYSICAL: a card in the machine, not a
/// virtual one (Hyper-V, VPN and the like are Ethernet too, to Windows).
const IF_TYPE_ETHERNET: u32 = 6;
const NCF_PHYSICAL: u32 = 0x4;

/// Whether a card's driver values make it a wired card in the machine. Pure.
pub fn is_wired_card(if_type: Option<u32>, characteristics: Option<u32>) -> bool {
    if_type == Some(IF_TYPE_ETHERNET) && characteristics.is_some_and(|c| c & NCF_PHYSICAL != 0)
}

struct DevInfo(HDEVINFO);
impl Drop for DevInfo {
    fn drop(&mut self) {
        unsafe {
            let _ = SetupDiDestroyDeviceInfoList(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn reg_dword(key: HKEY, sub: PCWSTR, name: &str) -> Option<u32> {
    let name = wide(name);
    let mut v = 0u32;
    let mut len = size_of::<u32>() as u32;
    unsafe { RegGetValueW(key, sub, PCWSTR(name.as_ptr()), RRF_RT_REG_DWORD, None, Some((&raw mut v).cast()), Some(&mut len)) }.is_ok().then_some(v)
}

fn reg_string(key: HKEY, sub: PCWSTR, name: &str) -> Option<String> {
    let name = wide(name);
    let mut buf = [0u16; 512];
    let mut len = (buf.len() * 2) as u32;
    unsafe { RegGetValueW(key, sub, PCWSTR(name.as_ptr()), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len)) }.ok().ok()?;
    let n = (len as usize / 2).saturating_sub(1).min(buf.len());
    let s = String::from_utf16_lossy(&buf[..n]);
    (!s.is_empty()).then_some(s)
}

/// The wired cards of this PC, turned on or off, by their names. Empty on a PC without one (or when nothing could be read).
pub fn adapters() -> Vec<Adapter> {
    let mut out = Vec::new();
    unsafe {
        let Ok(set) = SetupDiGetClassDevsW(Some(&GUID_DEVCLASS_NET), PCWSTR::null(), None, DIGCF_PRESENT) else { return out };
        let set = DevInfo(set);
        for index in 0.. {
            let mut dev = SP_DEVINFO_DATA { cbSize: size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiEnumDeviceInfo(set.0, index, &mut dev).is_err() {
                break;
            }
            let Ok(key) = SetupDiOpenDevRegKey(set.0, &dev, DICS_FLAG_GLOBAL.0, 0, DIREG_DRV, KEY_READ.0) else { continue };
            let if_type = reg_dword(key, PCWSTR::null(), "*IfType");
            let chars = reg_dword(key, PCWSTR::null(), "Characteristics");
            let id = reg_string(key, PCWSTR::null(), "NetCfgInstanceId");
            let _ = RegCloseKey(key);
            let Some(id) = id.filter(|_| is_wired_card(if_type, chars)) else { continue };
            let conn = wide(&format!(r"SYSTEM\CurrentControlSet\Control\Network\{{4D36E972-E325-11CE-BFC1-08002BE10318}}\{id}\Connection"));
            let Some(name) = reg_string(HKEY_LOCAL_MACHINE, PCWSTR(conn.as_ptr()), "Name") else { continue };
            let (mut status, mut problem) = (CM_DEVNODE_STATUS_FLAGS::default(), CM_PROB::default());
            let disabled = CM_Get_DevNode_Status(&mut status, &mut problem, dev.DevInst, 0) == CR_SUCCESS && problem == CM_PROB_DISABLED;
            out.push(Adapter { name, enabled: !disabled });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// What a request to turn a card on or off came to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// The user said no to Windows' question (UAC).
    Declined,
    Failed,
}

/// The arguments `netsh` gets to turn the card `name` on or off. Pure.
pub fn netsh_args(name: &str, on: bool) -> String {
    // (a name with a quote in it cannot be one Windows gives a card; it is left out rather than breaking the line)
    let name: String = name.chars().filter(|c| *c != '"').collect();
    format!("interface set interface name=\"{name}\" admin={}", if on { "enabled" } else { "disabled" })
}

/// Turns the card `name` on or off. Windows asks the user first (UAC); this waits for `netsh` to finish (blocks, up to half a minute).
pub fn set_enabled(name: &str, on: bool) -> Outcome {
    use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED, WAIT_OBJECT_0};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let args = wide(&netsh_args(name, on));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: w!("runas"),
        lpFile: w!("netsh.exe"),
        lpParameters: PCWSTR(args.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            return if e.code() == ERROR_CANCELLED.to_hresult() { Outcome::Declined } else { Outcome::Failed };
        }
        if info.hProcess.is_invalid() {
            return Outcome::Failed;
        }
        let waited = WaitForSingleObject(info.hProcess, 30_000);
        let mut code = 1u32;
        let ok = waited == WAIT_OBJECT_0 && GetExitCodeProcess(info.hProcess, &mut code).is_ok() && code == 0;
        let _ = CloseHandle(info.hProcess);
        if ok { Outcome::Done } else { Outcome::Failed }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_wired_card, netsh_args};

    #[test]
    fn only_physical_ethernet_cards_are_wired_cards() {
        assert!(is_wired_card(Some(6), Some(0x84)), "an Ethernet card in the machine");
        assert!(!is_wired_card(Some(6), Some(0x1)), "a virtual one (Hyper-V, VPN)");
        assert!(!is_wired_card(Some(71), Some(0x84)), "a Wi-Fi card");
        assert!(!is_wired_card(None, Some(0x84)));
        assert!(!is_wired_card(Some(6), None));
    }

    #[test]
    fn netsh_gets_the_name_quoted_and_the_state() {
        assert_eq!(netsh_args("Ethernet 2", false), "interface set interface name=\"Ethernet 2\" admin=disabled");
        assert_eq!(netsh_args("Ethernet", true), "interface set interface name=\"Ethernet\" admin=enabled");
        assert_eq!(netsh_args("Et\"h", true), "interface set interface name=\"Eth\" admin=enabled");
    }

    /// Reads this PC's cards (nothing is changed). Run by hand: `cargo test -p rf-win ethernet -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn read_this_pcs_cards() {
        println!("{:?}", super::adapters());
    }
}
