//! Read-only look at the Recycle Bin and its settings; prints counts only (no names, paths or values), creates no window.
//!
//!     cargo run --offline -p rf-win --example recycle_probe
//!
//! * the bin's items: how many, how many the shell calls folders / folders and streams (compressed folders), how many
//!   are directories for the file system, and how many are "folder" for the shell but a file for the file system;
//! * what `rf_win::recycle::list` makes of that (its `is_dir`);
//! * the bin's settings as the registry holds them: drives, how many have a custom size / remove-immediately, the
//!   delete confirmation (and whether a policy fixes it), and a cross-check of the confirmation against the raw ShellState byte.
//!
//! Nothing is deleted, restored, emptied or written.

use rf_core::recycle_bin::BinSettingsStore;
use windows_registry::CURRENT_USER;

fn main() {
    rf_win::shell::ensure_com();
    match rf_win::recycle::attribute_survey() {
        Ok((items, folder, folder_stream, fs_dir, disagree)) => {
            println!("bin items: {items}");
            println!("  shell says folder: {folder}; folder and stream: {folder_stream}; file system says directory: {fs_dir}; shell folder but file-system file: {disagree}");
        }
        Err(e) => println!("bin survey failed: {e:#}"),
    }
    match rf_win::recycle::list() {
        Ok(v) => {
            let dirs = v.iter().filter(|i| i.is_dir).count();
            println!("list(): {} items, {} drawn as folders, {} as files", v.len(), dirs, v.len() - dirs);
        }
        Err(e) => println!("list failed: {e:#}"),
    }
    match rf_win::recycle_settings::SystemBinSettings.read() {
        Ok(s) => {
            let custom = s.volumes.iter().filter(|v| v.max_mb.is_some()).count();
            let nuke = s.volumes.iter().filter(|v| v.remove_immediately).count();
            println!("settings: {} drives, {custom} with a custom size, {nuke} remove-immediately, confirmation {}, fixed by policy {}", s.volumes.len(), s.confirm_delete, s.confirm_locked);
        }
        Err(e) => println!("settings failed: {e:#}"),
    }
    // the raw ShellState value (read only): byte 4 holds the first bit field, bit 0x04 = fNoConfirmRecycle
    let raw = CURRENT_USER.open(r"Software\Microsoft\Windows\CurrentVersion\Explorer").ok().and_then(|k| k.get_value("ShellState").ok());
    match raw {
        Some(v) => {
            let b = v.to_vec();
            println!("ShellState: {} bytes; fNoConfirmRecycle bit {}", b.len(), b.get(4).map(|x| x & 0x04 != 0).map_or("unknown".to_string(), |x| x.to_string()));
        }
        None => println!("ShellState: not present"),
    }
}
