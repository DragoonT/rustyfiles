//! Starting a packaged (Store) app by its AppUserModelID, the one way every part of the app uses (the dock's pinned and running apps,
//! jump-list tasks, the Start panel's All apps entries when the shell refuses).
//!
//! The application activation manager (`IApplicationActivationManager::ActivateApplication`) is what the shell itself calls for such an
//! app. Measured on the user's PC with NO Explorer: the call starts the app and then does not return (the app's window was up, the call
//! still out after 40 s). So the call is made on a thread of its own, never on the caller's (the dock's click handlers and the shared
//! shell thread must not hang), and the answer is waited for only [`ACTIVATION_WAIT`]:
//!
//! * an answer in time is the answer (an error is an error: the app is not installed, the id is wrong);
//! * silence is taken as "started", and logged; an error that arrives after the wait is logged as a warning, and for a caller that has
//!   a second step of its own (the dock: the shell's open) that step is run from the activation's thread ([`Late::Shell`]), so that a
//!   slow activation that fails late does not lose it;
//! * with no Explorer each such call stays out for good: one parked thread per launch, until the process ends. That is the price of not
//!   being able to cut a COM call short; a launch is a click, so the count stays small.
//!
//! With Explorer running the call returns within a few hundred milliseconds, so the behaviour there is the one callers had before: the
//! same call, the same error, the caller continues with its shell fallback when it gets an error.
//!
//! The activation manager is created as a local server first (the usual way). Our own process is tried second only when that is
//! not a matter of the app: the manager could not be made as a local server, or the server was not there / died under the call
//! ([`server_trouble`]). An answer about the app (not installed, a bad id) is final: our process would say the same, and it would only
//! delay the error.

use std::time::Duration;

use windows::core::PCWSTR;

use crate::shell::ensure_com;
use crate::wide::wide;

/// How long the activation's answer is waited for.
pub const ACTIVATION_WAIT: Duration = Duration::from_millis(600);

/// What the wait for the activation's answer means.
pub fn activation_outcome(answer: Option<Result<(), String>>) -> Result<(), String> {
    match answer {
        Some(r) => r,
        // no answer in time: the app is starting (or started) and the call is still out
        None => Ok(()),
    }
}

/// What a caller wants done when the activation fails AFTER its answer was waited for (the caller has been told "started" by then).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Late {
    /// Only a log line: the caller has its own next step already (the Start panel asked the shell first).
    Log,
    /// Try the shell's open of the app's All-apps entry from the activation's thread: the dock's pinned and running apps tried the
    /// activation first and the shell second, and a slow activation that fails late must not lose that second step.
    Shell,
}

/// The two ways the activation manager is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ctx {
    /// The system's own server (the usual way).
    LocalServer,
    /// In our process.
    InProcess,
}

/// How one try failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fail {
    /// The manager could not be made this way.
    Create(String),
    /// The manager was made and `ActivateApplication` answered with this error (`code` is the HRESULT).
    Activate { code: i32, text: String },
}

/// An `ActivateApplication` error that says the SERVER was not there or died (so another way of making the manager may do), as opposed to
/// the manager answering about the app (not installed, bad id, bad arguments), which another process would answer the same.
pub fn server_trouble(code: i32) -> bool {
    matches!(
        code as u32,
        0x8004_0154 // class not registered
        | 0x8008_0005 // server execution failed
        | 0x8007_06BA // the RPC server is unavailable
        | 0x8007_06BE // the RPC call failed
        | 0x8001_0007 // the server died
        | 0x8001_0108 // the object was disconnected from its clients
    )
}

/// Runs `attempt` for the manager made as a local server and, when that is a different problem from the app's, in our process: the
/// second way is tried after a creation error or a server-trouble error only. An error about the app ends the matter at once (the same
/// answer would come twice, and a start that half worked must not be started again).
pub fn run_contexts(mut attempt: impl FnMut(Ctx) -> Result<(), Fail>) -> Result<(), String> {
    let mut last = String::new();
    for ctx in [Ctx::LocalServer, Ctx::InProcess] {
        match attempt(ctx) {
            Ok(()) => return Ok(()),
            Err(Fail::Create(e)) => last = e,
            Err(Fail::Activate { code, text }) => {
                if !server_trouble(code) {
                    return Err(format!("activation: {text}"));
                }
                last = text;
            }
        }
    }
    Err(format!("activation: {last}"))
}

/// Runs `job` on a thread of its own and waits `wait` for its answer: `Some(answer)` in time, `None` for silence. An error that arrives
/// after the wait is handed to `late` on that same thread (nobody is waiting for it any more).
fn answered_in_time(
    wait: Duration,
    job: impl FnOnce() -> Result<(), String> + Send + 'static,
    late: impl FnOnce(String) + Send + 'static,
) -> anyhow::Result<Option<Result<(), String>>> {
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    std::thread::Builder::new().name("rf-activate-app".into()).spawn(move || {
        let answer = job();
        // (the receiver is gone once the wait is over: then the send fails and the answer comes back inside the error)
        if let Err(std::sync::mpsc::SendError(Err(e))) = tx.send(answer) {
            late(e);
        }
    })?;
    Ok(rx.recv_timeout(wait).ok())
}

/// Starts the packaged app `aumid` (`family!app`), with `args` as its activation argument string when it is not empty. An error that
/// arrives after [`ACTIVATION_WAIT`] is only logged ([`Late::Log`]).
///
/// Returns when the activation answered, or [`ACTIVATION_WAIT`] after the call (see the module). Safe to call from any thread.
pub fn packaged(aumid: &str, args: &str) -> anyhow::Result<()> {
    packaged_with(aumid, args, Late::Log)
}

/// [`packaged`] with a choice of what a late error does.
pub fn packaged_with(aumid: &str, args: &str, late: Late) -> anyhow::Result<()> {
    let (id_text, args_text) = (aumid.to_string(), args.to_string());
    let id_for_late = id_text.clone();
    let answer = answered_in_time(
        ACTIVATION_WAIT,
        move || run_contexts(|ctx| try_once(ctx, &id_text, &args_text)),
        move |e| {
            log::warn!("activate: {id_for_late} did not start in time: {e}");
            if late == Late::Shell {
                // (shell calls from a thread of ours go through the one shell thread, see `sta::call`)
                let target = format!("shell:AppsFolder\\{id_for_late}");
                match crate::sta::call(Duration::from_secs(15), move || crate::shellfolder::open(&target)) {
                    Some(Ok(())) => log::info!("activate: {id_for_late} started by the shell after the activation failed"),
                    Some(Err(e)) => log::warn!("activate: the shell could not start {id_for_late} either: {e:#}"),
                    None => log::warn!("activate: the shell did not answer for {id_for_late}"),
                }
            }
        },
    )?;
    if answer.is_none() {
        log::info!("activate: {aumid} has not answered in {ACTIVATION_WAIT:?}; taken as started");
    }
    activation_outcome(answer).map_err(|e| anyhow::anyhow!(e))
}

/// One try: the manager made the given way, then the activation.
fn try_once(ctx: Ctx, aumid: &str, args: &str) -> Result<(), Fail> {
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, CoCreateInstance};
    use windows::Win32::UI::Shell::{AO_NONE, ApplicationActivationManager, IApplicationActivationManager};
    ensure_com();
    let id = wide(aumid);
    let arguments = wide(args);
    let arguments = if args.is_empty() { PCWSTR::null() } else { PCWSTR(arguments.as_ptr()) };
    let clsctx = match ctx {
        Ctx::LocalServer => CLSCTX_LOCAL_SERVER,
        Ctx::InProcess => CLSCTX_INPROC_SERVER,
    };
    let manager = unsafe { CoCreateInstance::<_, IApplicationActivationManager>(&ApplicationActivationManager, None, clsctx) }.map_err(|e| Fail::Create(e.to_string()))?;
    unsafe { manager.ActivateApplication(PCWSTR(id.as_ptr()), arguments, AO_NONE) }.map(|_| ()).map_err(|e| Fail::Activate { code: e.code().0, text: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn an_activation_that_does_not_answer_is_taken_as_started_and_an_error_in_time_is_an_error() {
        assert_eq!(activation_outcome(Some(Ok(()))), Ok(()));
        assert_eq!(activation_outcome(Some(Err("activation: no such app".into()))), Err("activation: no such app".into()));
        assert_eq!(activation_outcome(None), Ok(()), "with no Explorer the call starts the app and stays out");
    }

    /// An id that is no app: the manager answers with an error well inside the wait, and the error reaches the caller. Starts nothing.
    #[test]
    fn an_id_that_is_no_app_comes_back_as_an_error() {
        let r = packaged("rustyfiles.test.NoSuchPackage_0000000000000!App", "");
        let e = r.expect_err("nothing is installed under this id");
        assert!(e.to_string().starts_with("activation:"), "{e}");
    }

    #[test]
    fn a_call_that_never_returns_does_not_hold_the_caller_past_the_wait() {
        // the no-Explorer case: the activation manager starts the app and the call stays out
        let (park_tx, park_rx) = std::sync::mpsc::channel::<()>();
        let t = Instant::now();
        let r = answered_in_time(
            Duration::from_millis(100),
            move || {
                let _ = park_rx.recv_timeout(Duration::from_secs(5));
                Ok(())
            },
            |_| {},
        );
        assert!(t.elapsed() < Duration::from_secs(2), "the caller was held for {:?}", t.elapsed());
        assert_eq!(r.unwrap(), None, "silence");
        let _ = park_tx.send(());
    }

    #[test]
    fn an_error_in_time_reaches_the_caller_and_is_not_also_handed_to_the_late_step() {
        let (late_tx, late_rx) = std::sync::mpsc::channel::<String>();
        let r = answered_in_time(Duration::from_secs(5), || Err("activation: no such app".into()), move |e| {
            let _ = late_tx.send(e);
        });
        assert_eq!(r.unwrap(), Some(Err("activation: no such app".into())));
        assert!(late_rx.recv_timeout(Duration::from_millis(200)).is_err(), "the caller already has this error");
    }

    #[test]
    fn an_error_after_the_wait_is_handed_to_the_late_step_so_the_shell_second_step_is_not_lost() {
        let (late_tx, late_rx) = std::sync::mpsc::channel::<String>();
        let r = answered_in_time(
            Duration::from_millis(50),
            || {
                std::thread::sleep(Duration::from_millis(300));
                Err("activation: the app crashed while starting".into())
            },
            move |e| {
                let _ = late_tx.send(e);
            },
        );
        assert_eq!(r.unwrap(), None, "the caller was told 'started' after the wait");
        assert_eq!(late_rx.recv_timeout(Duration::from_secs(3)).unwrap(), "activation: the app crashed while starting");
    }

    #[test]
    fn a_late_success_is_not_an_error() {
        let (late_tx, late_rx) = std::sync::mpsc::channel::<String>();
        let r = answered_in_time(
            Duration::from_millis(50),
            || {
                std::thread::sleep(Duration::from_millis(200));
                Ok(())
            },
            move |e| {
                let _ = late_tx.send(e);
            },
        );
        assert_eq!(r.unwrap(), None);
        assert!(late_rx.recv_timeout(Duration::from_millis(600)).is_err());
    }

    #[test]
    fn an_error_about_the_app_is_not_tried_a_second_time_but_a_missing_server_is() {
        // the manager says the app is not installed: the same answer would come from our process
        let mut tried = Vec::new();
        let r = run_contexts(|c| {
            tried.push(c);
            Err(Fail::Activate { code: 0x8007_0490_u32 as i32, text: "element not found".into() })
        });
        assert_eq!(r, Err("activation: element not found".into()));
        assert_eq!(tried, vec![Ctx::LocalServer]);

        // the manager could not be made as a local server: our process is tried
        let mut tried = Vec::new();
        let r = run_contexts(|c| {
            tried.push(c);
            match c {
                Ctx::LocalServer => Err(Fail::Create("class not registered".into())),
                Ctx::InProcess => Ok(()),
            }
        });
        assert_eq!(r, Ok(()));
        assert_eq!(tried, vec![Ctx::LocalServer, Ctx::InProcess]);

        // the server was there but is gone under the call (no Explorer to serve it): our process is tried
        let mut tried = Vec::new();
        let r = run_contexts(|c| {
            tried.push(c);
            match c {
                Ctx::LocalServer => Err(Fail::Activate { code: 0x8007_06BA_u32 as i32, text: "the RPC server is unavailable".into() }),
                Ctx::InProcess => Ok(()),
            }
        });
        assert_eq!(r, Ok(()));
        assert_eq!(tried, vec![Ctx::LocalServer, Ctx::InProcess]);

        // both fail: the last reason is what the caller reads
        let r = run_contexts(|c| match c {
            Ctx::LocalServer => Err(Fail::Create("a".into())),
            Ctx::InProcess => Err(Fail::Create("b".into())),
        });
        assert_eq!(r, Err("activation: b".into()));
    }

    #[test]
    fn only_the_servers_own_trouble_counts_as_trouble() {
        for code in [0x8004_0154_u32, 0x8008_0005, 0x8007_06BA, 0x8007_06BE, 0x8001_0007, 0x8001_0108] {
            assert!(server_trouble(code as i32), "{code:#x}");
        }
        for code in [0x8007_0490_u32, 0x8007_0057, 0x8007_0005, 0x8007_0002] {
            assert!(!server_trouble(code as i32), "{code:#x}");
        }
    }
}
