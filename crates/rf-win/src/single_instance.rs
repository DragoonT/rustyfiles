//! Single-instance plumbing: the first process owns a named mutex and serves a named pipe;
//! later launches forward their arguments (paths) through the pipe and exit.

use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GENERIC_WRITE, GetLastError, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_NONE, OPEN_EXISTING, PIPE_ACCESS_INBOUND, ReadFile, WriteFile,
};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::CreateMutexW;
use windows::core::PCWSTR;

use crate::wide::wide;

/// Groups whose names carry the session number as well. The mutex is in the `Local\` namespace, which is per session already, but a
/// named PIPE lives in one namespace for the whole machine: two sessions of one user name (the console and a remote one, fast user
/// switching) would find each other's pipe, and a second `--dock` or `--shell` in session 2 would greet (and a `stop` would end) the
/// first one in session 1. The shell host is the one group that must be right here, because it can be somebody's whole desktop; the
/// others keep their names so that a copy of rustyfiles from before this change is still found by a newer one.
///
/// The FIRST interactive session (0 and 1: the console of a single-user PC) keeps the name it always had, with no session in it, for
/// the same reason: after an update the old main app that is still running sends `stop`, `reload` and `ping` to the name it knows, and
/// a dock that listens under a new one would be deaf to it (and a second dock would be started beside it). Two sessions can still not
/// meet, because every other session carries its number.
const SESSION_SCOPED: &[&str] = &["rustyfiles-dock"];

/// The sessions that keep the old, session-less names of the scoped groups.
const LEGACY_SESSIONS: u32 = 1;

/// The Windows session this process lives in (0 for the services session, 1 for the console on a single-user PC). 0 when it cannot be read.
pub fn session_id() -> u32 {
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    let mut id = 0u32;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut id) }.is_ok() { id } else { 0 }
}

/// (kilit adı, boru adı) bir uygulama için: rustyfiles ve DirectLook ayrı örnek gruplarıdır.
fn names_for(app: &str) -> (String, String) {
    names_in_session(app, session_id())
}

/// [`names_for`] for a given session (the tests need to ask for another one).
fn names_in_session(app: &str, session: u32) -> (String, String) {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    // A separate settings folder (tests, portable copies) is a separate instance group:
    // its second launch must not land in the user's everyday window.
    let suffix = match std::env::var_os("RUSTYFILES_CONFIG_DIR") {
        Some(dir) => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            dir.to_string_lossy().to_lowercase().hash(&mut h);
            format!("-{:08x}", h.finish() as u32)
        }
        None => String::new(),
    };
    let session = if SESSION_SCOPED.contains(&app) && session > LEGACY_SESSIONS { format!("-s{session}") } else { String::new() };
    (format!("Local\\{app}-{user}{suffix}{session}"), format!(r"\\.\pipe\{app}-{user}{suffix}{session}"))
}

pub enum Role {
    /// We are the first instance. Keep the guard alive for the process lifetime.
    Primary(InstanceGuard),
    /// Another instance is running and received our message.
    Forwarded,
}

pub struct InstanceGuard(HANDLE);
unsafe impl Send for InstanceGuard {}
impl Drop for InstanceGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Decides the role. `message` is sent to the primary when one exists ("" = just focus a new window/tab).
pub fn acquire(message: &str) -> Role {
    acquire_app(&instance_name(), message)
}

/// "rustyfiles", or "rustyfiles-NAME" with `RUSTYFILES_INSTANCE=NAME`: a copy that is a single
/// instance of its own (the speed test's "already running" copy) and never forwards to, or
/// receives from, the user's.
pub fn instance_name() -> String {
    match std::env::var("RUSTYFILES_INSTANCE") {
        Ok(n) if !n.is_empty() => format!("rustyfiles-{n}"),
        _ => "rustyfiles".to_string(),
    }
}

/// [`acquire`] for another application of the bundle (e.g. "directlook").
pub fn acquire_app(app: &str, message: &str) -> Role {
    let (mutex_name, pipe_name) = names_for(app);
    let w = wide(&mutex_name);
    let handle = unsafe { CreateMutexW(None, false, PCWSTR(w.as_ptr())) };
    let exists = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    match handle {
        Ok(h) if !exists => Role::Primary(InstanceGuard(h)),
        Ok(h) => {
            // Primary may still be starting its pipe: retry briefly.
            for _ in 0..40 {
                if send(&pipe_name, message) {
                    unsafe {
                        let _ = CloseHandle(h);
                    }
                    return Role::Forwarded;
                }
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            // Primary unresponsive (hung or exiting): run on our own.
            Role::Primary(InstanceGuard(h))
        }
        Err(_) => Role::Primary(InstanceGuard(HANDLE::default())),
    }
}

/// Sends `message` to the running instance of `app` (false if none answers). Never waits long:
/// the pipe either exists right now or it doesn't.
pub fn send_to(app: &str, message: &str) -> bool {
    send(&names_for(app).1, message)
}

fn send(pipe: &str, message: &str) -> bool {
    let w = wide(pipe);
    unsafe {
        let Ok(h) = CreateFileW(
            PCWSTR(w.as_ptr()),
            GENERIC_WRITE.0,
            FILE_SHARE_NONE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        ) else {
            return false;
        };
        let mut bytes = message.as_bytes().to_vec();
        bytes.push(b'\n');
        let ok = WriteFile(h, Some(&bytes), None, None).is_ok();
        let _ = CloseHandle(h);
        ok
    }
}

/// Serves the pipe on a background thread; each received line is passed to `on_message`.
pub fn serve(on_message: impl Fn(String) + Send + Sync + 'static) {
    serve_app("rustyfiles", on_message)
}

/// [`serve`] for another application of the bundle.
pub fn serve_app(app: &str, on_message: impl Fn(String) + Send + Sync + 'static) {
    let on_message = std::sync::Arc::new(on_message);
    let (_, pipe_name) = names_for(app);
    std::thread::Builder::new()
        .name("rf-instance-pipe".into())
        .spawn(move || {
            let w = wide(&pipe_name);
            loop {
                let pipe = unsafe {
                    CreateNamedPipeW(
                        PCWSTR(w.as_ptr()),
                        PIPE_ACCESS_INBOUND,
                        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                        PIPE_UNLIMITED_INSTANCES,
                        0,
                        64 * 1024,
                        0,
                        None,
                    )
                };
                if pipe.is_invalid() {
                    log::warn!("single instance pipe failed");
                    return;
                }
                unsafe {
                    let _ = ConnectNamedPipe(pipe, None);
                }
                // Read each client on its own thread so a stalled client can't block the next one.
                let raw = pipe.0 as isize;
                let cb = on_message.clone();
                std::thread::spawn(move || unsafe {
                    let pipe = HANDLE(raw as *mut _);
                    let mut buf = vec![0u8; 16 * 1024];
                    let mut total = Vec::new();
                    loop {
                        let mut n = 0u32;
                        if ReadFile(pipe, Some(&mut buf), Some(&mut n), None).is_err() || n == 0 {
                            break;
                        }
                        total.extend_from_slice(&buf[..n as usize]);
                        if total.len() > 64 * 1024 {
                            break;
                        }
                    }
                    let _ = DisconnectNamedPipe(pipe);
                    let _ = CloseHandle(pipe);
                    for line in String::from_utf8_lossy(&total).lines() {
                        cb(line.to_string());
                    }
                });
            }
        })
        .ok();
}

#[cfg(test)]
mod app_tests {
    use super::*;

    #[test]
    fn apps_have_separate_names() {
        let (m1, p1) = names_for("rustyfiles");
        let (m2, p2) = names_for("directlook");
        assert_ne!(m1, m2);
        assert_ne!(p1, p2);
        assert!(p2.contains("directlook"));
    }

    #[test]
    fn the_shell_host_group_is_told_apart_by_session_and_the_others_are_not() {
        let (m1, p1) = names_in_session("rustyfiles-dock", 1);
        let (m2, p2) = names_in_session("rustyfiles-dock", 2);
        let (m3, p3) = names_in_session("rustyfiles-dock", 3);
        assert_ne!(p1, p2, "a pipe is machine-wide: it must carry the session");
        assert_ne!(p2, p3);
        assert_ne!(m1, m2);
        assert_ne!(m2, m3);
        assert!(m1.starts_with("Local\\"), "the mutex stays in the per-session namespace");
        // the console session keeps the name an older copy computes, so that an old main app still reaches a newer dock
        assert_eq!(names_in_session("rustyfiles-dock", 0), (m1.clone(), p1.clone()));
        assert!(!p1.contains("-s1"), "{p1}");
        // the same session, the same name: a second --dock or --shell finds the first
        assert_eq!(names_in_session("rustyfiles-dock", 1), (m1, p1));
        // the main app and DirectLook keep the names older copies know
        assert_eq!(names_in_session("rustyfiles", 1), names_in_session("rustyfiles", 2));
        assert_eq!(names_in_session("directlook", 1), names_in_session("directlook", 5));
        assert!(session_id() < 1_000_000);
    }

    #[test]
    fn send_reaches_a_serving_app_and_fails_without_one() {
        let app = format!("rf-test-{}", std::process::id());
        assert!(!send_to(&app, "yok"));
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        serve_app(&app, move |m| {
            let _ = tx.lock().unwrap().send(m);
        });
        let t = std::time::Instant::now();
        while !send_to(&app, "merhaba") {
            assert!(t.elapsed().as_secs() < 3, "pipe never came up");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap(), "merhaba");
    }
}
