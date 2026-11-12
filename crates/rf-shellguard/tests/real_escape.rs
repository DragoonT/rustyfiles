//! The escape chord's event against the real Windows wait, without registering a hotkey, installing a
//! hook or creating a window: `RealHotkey::create_event_only` makes just the named event, and the test
//! signals it by name the way the hook thread and a `--panic` process do.
//!
//! The first test is the regression for a lost press: the runner waits on the event
//! (`WaitForMultipleObjects`) and then asks `take_pressed`; with an auto-reset event the wait itself used the
//! press up and `take_pressed` saw nothing, so the chord and `--panic` did nothing.

use std::time::{Duration, Instant};

use rf_shellguard::sys::{Hotkey, Processes, Session, Wait};
use rf_shellguard::win::hotkey::RealHotkey;
use rf_shellguard::win::procs::RealProcesses;
use rf_shellguard::win::session::{Names, RealSession};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
use windows::core::PCWSTR;

fn name(tag: &str) -> String {
    format!("Local\\rf-shellguard-test-escape-{}-{tag}", std::process::id())
}

fn session_for(escape: &str, ack_ms: u64) -> RealSession {
    RealSession::new(Names { shell_ready: format!("{escape}-unused-ready"), escape: escape.to_string() }).with_panic_ack_ms(ack_ms)
}

/// What the hook thread does on a press: signal the named event.
fn press(event_name: &str) {
    let wide: Vec<u16> = event_name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: opens the event this test created, signals it, closes it.
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(wide.as_ptr())).unwrap();
        SetEvent(h).unwrap();
        let _ = CloseHandle(h);
    }
}

#[test]
fn a_press_survives_the_wait_that_woke_the_runner() {
    let n = name("press");
    let mut hk = RealHotkey::new(&n);
    hk.create_event_only().unwrap();
    let mut procs = RealProcesses::new(None).without_windows();

    // Nothing pressed: the wait times out and there is nothing to take.
    assert_eq!(procs.wait(hk.waitable(), Some(50)), Wait::Timeout);
    assert!(!hk.take_pressed());

    press(&n);
    // The runner's order of events: the wait returns first ...
    assert_eq!(procs.wait(hk.waitable(), Some(5_000)), Wait::Extra);
    // ... and only then asks. The press must still be there.
    assert!(hk.take_pressed(), "the wait must not use the press up");
    // Taken once: gone.
    assert!(!hk.take_pressed());
    assert_eq!(procs.wait(hk.waitable(), Some(50)), Wait::Timeout, "and the wait does not fire again for the same press");
}

#[test]
fn two_presses_before_the_runner_looks_are_one_escape() {
    // The hook and the hotkey backup both fire for one real press.
    let n = name("double");
    let mut hk = RealHotkey::new(&n);
    hk.create_event_only().unwrap();
    press(&n);
    press(&n);
    assert!(hk.take_pressed());
    assert!(!hk.take_pressed());
}

#[test]
fn a_running_guard_that_takes_the_request_up_answers_the_panic_process() {
    let n = name("ack");
    let mut hk = RealHotkey::new(&n);
    hk.create_event_only().unwrap();
    // The "guard": takes the press the way the runner does.
    let guard = std::thread::spawn(move || {
        let until = Instant::now() + Duration::from_secs(10);
        while Instant::now() < until {
            if hk.take_pressed() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    });
    assert!(session_for(&n, 5_000).signal_running_guard_panic(), "handed over");
    assert!(guard.join().unwrap(), "and the guard saw it");
}

#[test]
fn a_hung_guard_does_not_count_as_handed_over() {
    // The event exists (the guard is alive) but nobody takes the request: the panic process must do the fall-back itself.
    let n = name("hung");
    let mut hk = RealHotkey::new(&n);
    hk.create_event_only().unwrap();
    let t = Instant::now();
    assert!(!session_for(&n, 300).signal_running_guard_panic());
    assert!(t.elapsed() >= Duration::from_millis(250), "it waited for an answer first");
    // The request is still pending for a guard that wakes up later.
    assert!(hk.take_pressed());
}

#[test]
fn no_guard_at_all_is_not_handed_over() {
    assert!(!session_for(&name("none"), 300).signal_running_guard_panic());
}
