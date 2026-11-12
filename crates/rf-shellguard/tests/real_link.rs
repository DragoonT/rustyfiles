//! The guard's real side of the host's two events, against events THIS TEST creates (names under `Local\` with the test's process id
//! in them). Nothing of Windows' or of Explorer's is touched: two event objects live for the length of the test.

use rf_shellguard::sys::HostLink;
use rf_shellguard::win::link::RealLink;
use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject};
use windows::core::PCWSTR;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn create(name: &str) -> HANDLE {
    let w = wide(name);
    // SAFETY: NUL-terminated name; manual-reset, not signalled.
    unsafe { CreateEventW(None, true, false, PCWSTR(w.as_ptr())) }.expect("event")
}

#[test]
fn the_guard_sets_ready_and_hears_done_through_the_hosts_events() {
    let pid = std::process::id();
    let (rn, dn) = (format!("Local\\rf-shellguard-test-link-ready-{pid}"), format!("Local\\rf-shellguard-test-link-done-{pid}"));
    let (ready, done) = (create(&rn), create(&dn));
    let mut link = RealLink::new(&rn, &dn);
    assert_eq!(link.done_waitable(), 0, "not armed yet");
    assert!(link.confirm_ready().is_err(), "cannot confirm before it is armed");
    link.arm().expect("arm");
    assert_ne!(link.done_waitable(), 0);
    // SAFETY: handles created above.
    unsafe {
        assert_eq!(WaitForSingleObject(ready, 0), WAIT_TIMEOUT);
        link.confirm_ready().expect("confirm");
        assert_eq!(WaitForSingleObject(ready, 0), WAIT_OBJECT_0, "the host sees the guard's confirmation");
        assert_eq!(WaitForSingleObject(HANDLE(link.done_waitable() as *mut _), 0), WAIT_TIMEOUT);
        SetEvent(done).expect("set done");
        assert_eq!(WaitForSingleObject(HANDLE(link.done_waitable() as *mut _), 0), WAIT_OBJECT_0, "the guard sees the host's word");
    }
    drop(link);
    // SAFETY: closing what was created above.
    unsafe {
        let _ = CloseHandle(ready);
        let _ = CloseHandle(done);
    }
}

#[test]
fn events_that_do_not_exist_cannot_be_armed() {
    let pid = std::process::id();
    let mut link = RealLink::new(&format!("Local\\rf-shellguard-test-nolink-a-{pid}"), &format!("Local\\rf-shellguard-test-nolink-b-{pid}"));
    assert!(link.arm().is_err());
    assert_eq!(link.done_waitable(), 0);
}
