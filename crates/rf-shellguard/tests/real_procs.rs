//! The real process side against harmless windowless children (`cmd.exe /c exit 3`, `ping`), never
//! against Explorer: `start_explorer` is only called with a stand-in that does not exist.
//! Children are created with CREATE_NO_WINDOW, so no window or console appears.

use std::path::PathBuf;

use rf_shellguard::sys::{HostLaunch, PidWait, Processes, Wait};
use rf_shellguard::win::procs::RealProcesses;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{EVENT_MODIFY_STATE, OpenEventW, SetEvent};
use windows::core::PCWSTR;

fn system32(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32").join(name)
}

fn launch(exe: PathBuf, args: &[&str], tag: &str) -> HostLaunch {
    HostLaunch { exe, args: args.iter().map(|s| s.to_string()).collect(), ready_event: format!("Local\\rf-shellguard-test-ready-{}-{tag}", std::process::id()) }
}

fn procs() -> RealProcesses {
    RealProcesses::new(None).without_windows()
}

#[test]
fn the_exit_code_of_a_host_comes_back_with_its_pid() {
    let mut p = procs();
    let pid = p.spawn_host(&launch(system32("cmd.exe"), &["/c", "exit 3"], "exit")).unwrap();
    assert_eq!(p.wait(0, Some(15_000)), Wait::HostExited { pid, code: 3 });
    // The host is gone: waiting again only times out.
    assert_eq!(p.wait(0, Some(50)), Wait::Timeout);
}

#[test]
fn a_running_host_times_out_is_not_hung_and_can_be_killed() {
    let mut p = procs();
    let pid = p.spawn_host(&launch(system32("PING.EXE"), &["-n", "20", "127.0.0.1"], "kill")).unwrap();
    assert_eq!(p.wait(0, Some(200)), Wait::Timeout);
    assert!(!p.host_hung(), "a console program has no window to hang");
    assert_eq!(p.wait_pid(pid, 0, Some(100)), PidWait::Timeout, "alive");
    p.kill_host();
    assert_eq!(p.wait_pid(pid, 0, Some(5_000)), PidWait::Gone);
    assert_eq!(p.wait(0, Some(50)), Wait::Timeout, "no host, nothing to report");
}

#[test]
fn a_process_id_that_does_not_exist_counts_as_gone_and_zero_is_a_plain_sleep() {
    let mut p = procs();
    assert_eq!(p.wait_pid(0xFFFF_FF00, 0, Some(10)), PidWait::Gone);
    let t = std::time::Instant::now();
    assert_eq!(p.wait_pid(0, 0, Some(120)), PidWait::Timeout);
    assert!(t.elapsed().as_millis() >= 100);
}

#[test]
fn the_ready_event_wakes_the_wait_once() {
    let mut p = procs();
    let l = launch(system32("PING.EXE"), &["-n", "20", "127.0.0.1"], "ready");
    p.spawn_host(&l).unwrap();
    assert_eq!(p.wait(0, Some(100)), Wait::Timeout);
    // The host would do this; the test does it by name.
    let name: Vec<u16> = l.ready_event.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: opens the event this test's own spawn just created, signals it, closes it.
    unsafe {
        let h = OpenEventW(EVENT_MODIFY_STATE, false, PCWSTR(name.as_ptr())).unwrap();
        SetEvent(h).unwrap();
        let _ = CloseHandle(h);
    }
    assert_eq!(p.wait(0, Some(5_000)), Wait::HostReady);
    // Manual-reset and already seen: it must not fire again (it would spin the loop).
    assert_eq!(p.wait(0, Some(100)), Wait::Timeout);
    p.kill_host();
}

#[test]
fn a_host_that_does_not_exist_is_an_error_not_a_crash() {
    let mut p = procs();
    let err = p.spawn_host(&launch(system32("no-such-host.exe"), &[], "missing")).unwrap_err();
    assert!(err.contains("no-such-host.exe"), "{err}");
}

#[test]
fn the_self_test_wants_a_program_not_a_text_file() {
    let mut p = procs();
    assert!(p.host_self_test(&launch(system32("cmd.exe"), &[], "st")));
    assert!(!p.host_self_test(&launch(system32("no-such-host.exe"), &[], "st")));
    let txt = std::env::temp_dir().join(format!("rf-shellguard-test-{}-selftest.txt", std::process::id()));
    std::fs::write(&txt, b"hello").unwrap();
    assert!(!p.host_self_test(&launch(txt.clone(), &[], "st")));
    let _ = std::fs::remove_file(&txt);
}

#[test]
fn a_missing_explorer_stand_in_is_reported_and_nothing_is_started() {
    let mut p = RealProcesses::new(Some(PathBuf::from("Z:\\no\\such\\stand-in.exe"))).without_windows();
    assert!(p.start_explorer().is_err());
    // With a stand-in configured, "up" means "the stand-in runs": it never was started.
    assert!(!p.explorer_shell_up(0));
}

#[test]
fn the_read_only_queries_run() {
    // These only read: two metrics, a key state, and registry values with the query right.
    assert!(rf_shellguard::winro::read_shell_value().is_ok());
    assert!(rf_shellguard::winro::read_override_policy().is_ok());
    assert!(rf_shellguard::winro::build().is_some());
    let _ = rf_shellguard::winro::safe_mode();
    let _ = rf_shellguard::winro::shift_held();
}

// ---- ending a host this process did not start (the standalone --panic) --------------------------

fn spawn_ping() -> std::process::Child {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(system32("PING.EXE"))
        .args(["-n", "30", "127.0.0.1"])
        .creation_flags(0x0800_0000)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

#[test]
fn an_adopted_host_with_the_right_image_is_ended() {
    let mut orphan = spawn_ping();
    let mut p = procs();
    // Without adoption, kill_host does not reach a process this one did not start.
    p.kill_host();
    assert!(orphan.try_wait().unwrap().is_none(), "not adopted: untouched");
    p.adopt_host(orphan.id(), &system32("PING.EXE"));
    p.kill_host();
    assert!(orphan.try_wait().unwrap().is_some(), "adopted and ended");
}

#[test]
fn an_adopted_process_running_some_other_program_is_left_alone() {
    // A process id can be reused: the id from the state file now belongs to another program.
    let mut other = spawn_ping();
    let mut p = procs();
    p.adopt_host(other.id(), &system32("cmd.exe"));
    p.kill_host();
    assert!(other.try_wait().unwrap().is_none(), "wrong image: not ended");
    let _ = other.kill();
    let _ = other.wait();
}

#[test]
fn adopting_this_process_or_a_dead_id_ends_nothing() {
    let mut p = procs();
    p.adopt_host(std::process::id(), &std::env::current_exe().unwrap());
    p.kill_host(); // would end the test run if it were honoured
    p.adopt_host(0xFFFF_FF00, &system32("PING.EXE"));
    p.kill_host();
    p.adopt_host(0, &system32("PING.EXE"));
    p.kill_host();
}

// ---- Explorer is started once while the first one is still coming up ----------------------------

fn temp_cmd(tag: &str, body: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("rf-shellguard-test-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("standin.cmd");
    std::fs::write(&file, body).unwrap();
    (dir, file)
}

#[test]
fn a_second_start_while_the_first_explorer_still_runs_starts_nothing() {
    // The stand-in runs for about two seconds, like an Explorer that is slow to show its desktop.
    let (dir, file) = temp_cmd("slow-explorer", "@ping -n 6 127.0.0.1 >nul\r\n");
    let mut p = RealProcesses::new(Some(file)).without_windows();
    p.start_explorer().unwrap();
    assert_eq!(p.explorer_starts(), 1);
    assert!(p.explorer_shell_up(0));
    // The fall-back's second try after a ten-second wait that ran out: no second program.
    p.start_explorer().unwrap();
    p.start_explorer().unwrap();
    assert_eq!(p.explorer_starts(), 1, "still the first one");
    // Let it finish before its folder goes (it holds the script open while it runs).
    let t = std::time::Instant::now();
    while p.explorer_shell_up(0) && t.elapsed().as_secs() < 15 {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_second_start_after_the_first_explorer_ended_starts_another() {
    let (dir, file) = temp_cmd("dead-explorer", "@exit /b 0\r\n");
    let mut p = RealProcesses::new(Some(file)).without_windows();
    p.start_explorer().unwrap();
    // It exits at once; wait until the process side notices.
    let t = std::time::Instant::now();
    while p.explorer_shell_up(0) && t.elapsed().as_secs() < 10 {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert!(!p.explorer_shell_up(0), "the stand-in ended");
    p.start_explorer().unwrap();
    assert_eq!(p.explorer_starts(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_test_run_never_looks_at_the_real_explorer() {
    let mut p = RealProcesses::new(Some(PathBuf::from(r"Z:\no\such\stand-in.exe"))).without_windows();
    assert!(!p.explorer_owns_shell_window());
}
