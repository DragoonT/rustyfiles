//! Windows' own sounds, played by name from the user's sound scheme (Control Panel > Sound > Sounds): what the user hears for the
//! same event in every other program, and nothing when the scheme has no sound for it.

/// The notification sound (the scheme's "Notification" event), asynchronously.
pub fn notify() {
    use windows::Win32::Media::Audio::{PlaySoundW, SND_ALIAS, SND_ASYNC, SND_NODEFAULT};
    let name = crate::wide::wide("Notification.Default");
    unsafe {
        let _ = PlaySoundW(windows::core::PCWSTR(name.as_ptr()), None, SND_ALIAS | SND_ASYNC | SND_NODEFAULT);
    }
}
