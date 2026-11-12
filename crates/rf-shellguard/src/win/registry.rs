//! The real registry side. The guard reads the Shell value and, in the fall-back, deletes it. It
//! never writes it: turning the switch on is the Settings page's decision, in another program.
//! NOT RUN by the author of this crate (the delete is exercised only through `MemRegistry`).

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, RegCloseKey, RegDeleteValueW, RegOpenKeyExW};
use windows::core::w;

use crate::sys::Registry;
use crate::winro;

pub struct RealRegistry;

impl Registry for RealRegistry {
    fn read_shell_value(&mut self) -> Result<Option<String>, String> {
        winro::read_shell_value()
    }

    fn read_override_policy(&mut self) -> Result<Option<u32>, String> {
        winro::read_override_policy()
    }

    /// Deletes `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Winlogon\Shell`, that one value and nothing else.
    fn delete_shell_value(&mut self) -> Result<bool, String> {
        let mut key = HKEY::default();
        // SAFETY: constant NUL-terminated strings; `key` is a valid out-pointer.
        let opened = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, w!(r"Software\Microsoft\Windows NT\CurrentVersion\Winlogon"), None, KEY_SET_VALUE, &mut key) };
        if opened == ERROR_FILE_NOT_FOUND {
            // No Winlogon key for this user: no Shell value either.
            return Ok(false);
        }
        if opened != ERROR_SUCCESS {
            return Err(format!("opening the Winlogon key failed ({})", opened.0));
        }
        // SAFETY: `key` is open with the set-value right.
        let deleted = unsafe { RegDeleteValueW(key, w!("Shell")) };
        // SAFETY: `key` was opened above.
        unsafe {
            let _ = RegCloseKey(key);
        }
        if deleted == ERROR_SUCCESS {
            Ok(true)
        } else if deleted == ERROR_FILE_NOT_FOUND {
            Ok(false)
        } else {
            Err(format!("deleting the Shell value failed ({})", deleted.0))
        }
    }
}
