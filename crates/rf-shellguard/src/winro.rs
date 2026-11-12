//! READ-ONLY Windows queries. This file is all that `--dry-run` may reach: it opens registry keys
//! with the query right only, reads two metrics and a key state, and nothing else. It creates no
//! window, no process, no event and no hook, and it has no code path that writes. The test
//! `tests/dryrun_proof.rs` reads this file and `dryrun.rs` and fails if a write or a window API
//! name appears in either.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_QUERY_VALUE, REG_DWORD, REG_EXPAND_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegOpenKeyExW, RegQueryValueExW};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SHIFT};
use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CLEANBOOT};
use windows::core::PCWSTR;

const WINLOGON: &str = r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon";
const POLICY: &str = r"SOFTWARE\Microsoft\PolicyManager\current\device\WindowsLogon";
const CURRENT_VERSION: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Reads one value with the query right only. `Ok(None)`: the key or the value is not there.
fn read_value(root: HKEY, sub: &str, name: &str) -> Result<Option<(REG_VALUE_TYPE, Vec<u8>)>, String> {
    let sub_w = wide(sub);
    let name_w = wide(name);
    let mut key = HKEY::default();
    // SAFETY: both strings are NUL-terminated and outlive the calls; `key` is a valid out-pointer.
    let opened = unsafe { RegOpenKeyExW(root, PCWSTR(sub_w.as_ptr()), None, KEY_QUERY_VALUE, &mut key) };
    if opened == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    if opened != ERROR_SUCCESS {
        return Err(format!("opening the key failed ({})", opened.0));
    }
    let mut ty = REG_VALUE_TYPE::default();
    let mut size = 0u32;
    // SAFETY: first call only asks for type and size.
    let probe = unsafe { RegQueryValueExW(key, PCWSTR(name_w.as_ptr()), None, Some(&mut ty), None, Some(&mut size)) };
    let result = if probe == ERROR_FILE_NOT_FOUND {
        Ok(None)
    } else if probe != ERROR_SUCCESS {
        Err(format!("reading the value failed ({})", probe.0))
    } else if size > 64 * 1024 {
        Err("the value is unreasonably large".to_string())
    } else {
        let mut buf = vec![0u8; size as usize];
        let mut got = size;
        // SAFETY: `buf` holds `got` bytes.
        let r = unsafe { RegQueryValueExW(key, PCWSTR(name_w.as_ptr()), None, Some(&mut ty), Some(buf.as_mut_ptr()), Some(&mut got)) };
        if r != ERROR_SUCCESS {
            Err(format!("reading the value failed ({})", r.0))
        } else {
            buf.truncate(got as usize);
            Ok(Some((ty, buf)))
        }
    };
    // SAFETY: `key` was opened above.
    unsafe {
        let _ = RegCloseKey(key);
    }
    result
}

fn as_string(ty: REG_VALUE_TYPE, bytes: &[u8]) -> Option<String> {
    if ty != REG_SZ && ty != REG_EXPAND_SZ {
        return None;
    }
    let (pairs, _) = bytes.as_chunks::<2>();
    let units: Vec<u16> = pairs.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    Some(String::from_utf16_lossy(&units[..end]))
}

fn as_dword(ty: REG_VALUE_TYPE, bytes: &[u8]) -> Option<u32> {
    if ty != REG_DWORD || bytes.len() < 4 {
        return None;
    }
    Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

/// `HKCU\...\Winlogon\Shell`.
pub fn read_shell_value() -> Result<Option<String>, String> {
    Ok(read_value(HKEY_CURRENT_USER, WINLOGON, "Shell")?.and_then(|(t, b)| as_string(t, &b)))
}

/// `WindowsLogon/OverrideShellProgram` from the policy store.
pub fn read_override_policy() -> Result<Option<u32>, String> {
    Ok(read_value(HKEY_LOCAL_MACHINE, POLICY, "OverrideShellProgram")?.and_then(|(t, b)| as_dword(t, &b)))
}

/// "26300.9974": the build number and the update revision.
pub fn build() -> Option<String> {
    let (t, b) = read_value(HKEY_LOCAL_MACHINE, CURRENT_VERSION, "CurrentBuildNumber").ok()??;
    let number = as_string(t, &b)?;
    let ubr = read_value(HKEY_LOCAL_MACHINE, CURRENT_VERSION, "UBR").ok().flatten().and_then(|(t, b)| as_dword(t, &b));
    Some(match ubr {
        Some(u) => format!("{number}.{u}"),
        None => number,
    })
}

pub fn safe_mode() -> bool {
    // SAFETY: a plain metric query.
    unsafe { GetSystemMetrics(SM_CLEANBOOT) != 0 }
}

pub fn shift_held() -> bool {
    // SAFETY: reads the key state; sends no input.
    unsafe { (GetAsyncKeyState(i32::from(VK_SHIFT.0)) as u16) & 0x8000 != 0 }
}
