//! "Tell me when something under these registry keys changes": `RegNotifyChangeKeyValue` on a list of current-user keys, one thread
//! for all of them, no polling. It only listens; it opens the keys with notify access and never writes.
//!
//! The keys are given WITHOUT the hive. A key that does not exist (yet) is skipped. The thread ends when the [`RegWatch`] is dropped.

use std::sync::Arc;

use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_NOTIFY, REG_NOTIFY_CHANGE_LAST_SET, REG_NOTIFY_CHANGE_NAME, REG_NOTIFY_THREAD_AGNOSTIC, RegCloseKey, RegNotifyChangeKeyValue,
    RegOpenKeyExW,
};
use windows::Win32::System::Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects};
use windows::core::PCWSTR;

pub struct RegWatch {
    stop: HANDLE,
    thread: Option<std::thread::JoinHandle<()>>,
}

// the event handle is only signalled from here and closed after the thread has ended
unsafe impl Send for RegWatch {}

struct Armed {
    key: &'static str,
    hkey: HKEY,
    event: HANDLE,
}

/// Calls `on_change(key)` from the watcher thread whenever a value or a sub-key under `key` or anywhere below it is set, created or
/// deleted.
pub fn watch_keys(keys: &[&'static str], on_change: Arc<dyn Fn(&'static str) + Send + Sync>) -> RegWatch {
    watch_keys_with(keys, true, on_change)
}

/// One step of the loop: the notification is armed AGAIN before the callback runs, so a change made while the callback is busy (it
/// may take seconds) is reported at the next turn instead of being lost. Returns whether the key could be armed again.
fn dispatch(i: usize, arm: &mut dyn FnMut(usize) -> bool, call: &mut dyn FnMut(usize)) -> bool {
    let armed_again = arm(i);
    call(i);
    armed_again
}

/// [`watch_keys`] with the choice of watching the sub-keys too (`subtree`). A watch of one key's own values (`false`) is quiet where
/// the subtree of a busy key, like Explorer's `Advanced`, is not.
pub fn watch_keys_with(keys: &[&'static str], subtree: bool, on_change: Arc<dyn Fn(&'static str) + Send + Sync>) -> RegWatch {
    let stop = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.unwrap_or_default();
    let keys: Vec<&'static str> = keys.to_vec();
    // (a HANDLE is a raw pointer and does not cross threads by itself; this one is only waited on there)
    let stop_raw = stop.0 as isize;
    let thread = std::thread::Builder::new()
        .name("rf-regnotify".into())
        .spawn(move || {
            let stop = HANDLE(stop_raw as *mut _);
            let mut armed: Vec<Armed> = Vec::new();
            for key in keys {
                let w = crate::wide::wide(key);
                let mut hkey = HKEY::default();
                if unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(w.as_ptr()), None, KEY_NOTIFY, &mut hkey) }.is_err() {
                    continue;
                }
                let Ok(event) = (unsafe { CreateEventW(None, false, false, PCWSTR::null()) }) else {
                    unsafe {
                        let _ = RegCloseKey(hkey);
                    }
                    continue;
                };
                armed.push(Armed { key, hkey, event });
            }
            let arm = |a: &Armed| unsafe {
                // THREAD_AGNOSTIC: the notification does not die with the thread that asked for it
                let r = RegNotifyChangeKeyValue(a.hkey, subtree, REG_NOTIFY_CHANGE_NAME | REG_NOTIFY_CHANGE_LAST_SET | REG_NOTIFY_THREAD_AGNOSTIC, Some(a.event), true);
                r.is_ok()
            };
            armed.retain(|a| arm(a));
            if !armed.is_empty() && !stop.is_invalid() {
                let mut handles: Vec<HANDLE> = armed.iter().map(|a| a.event).collect();
                handles.push(stop);
                loop {
                    let r = unsafe { WaitForMultipleObjects(&handles, false, INFINITE) };
                    let i = r.0.wrapping_sub(WAIT_OBJECT_0.0) as usize;
                    if i >= armed.len() {
                        break; // the stop event, or a failure
                    }
                    let key = armed[i].key;
                    let again = dispatch(i, &mut |i| arm(&armed[i]), &mut |_| on_change(key));
                    if !again {
                        break;
                    }
                }
            }
            for a in armed {
                unsafe {
                    let _ = RegCloseKey(a.hkey);
                    let _ = CloseHandle(a.event);
                }
            }
        })
        .ok();
    RegWatch { stop, thread }
}

impl Drop for RegWatch {
    fn drop(&mut self) {
        if !self.stop.is_invalid() {
            unsafe {
                let _ = SetEvent(self.stop);
            }
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if !self.stop.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.stop);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Starting and stopping the watch works and does not fire by itself. (Making a change fire would need a registry write, which
    /// nothing in this crate's tests does.)
    #[test]
    fn starts_and_stops_without_firing() {
        let hits = Arc::new(AtomicUsize::new(0));
        let h2 = hits.clone();
        let w = watch_keys(
            &[r"Control Panel\Colors", r"Control Panel\Cursors", r"Software\No\Such\Key\Anywhere"],
            Arc::new(move |_| {
                h2.fetch_add(1, Ordering::Relaxed);
            }),
        );
        std::thread::sleep(std::time::Duration::from_millis(150));
        let t = std::time::Instant::now();
        drop(w);
        assert!(t.elapsed() < std::time::Duration::from_secs(2), "the thread stops promptly");
        assert_eq!(hits.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn the_key_is_armed_again_before_the_callback_runs() {
        let log = std::cell::RefCell::new(Vec::new());
        let again = dispatch(3, &mut |i| { log.borrow_mut().push(format!("arm {i}")); true }, &mut |i| log.borrow_mut().push(format!("call {i}")));
        assert!(again);
        assert_eq!(*log.borrow(), ["arm 3", "call 3"], "a change made while the callback runs must still be seen");
        // a key that cannot be armed again still reports this change, and tells the loop to stop
        let called = std::cell::Cell::new(false);
        assert!(!dispatch(0, &mut |_| false, &mut |_| called.set(true)));
        assert!(called.get());
    }

    #[test]
    fn no_keys_at_all_is_fine() {
        drop(watch_keys(&[], Arc::new(|_| {})));
    }
}
