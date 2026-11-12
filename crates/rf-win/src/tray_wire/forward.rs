//! The relay's decisions, pure. A window of class `Shell_TrayWnd` that sits in front of Explorer's (the tray spy, and later the tray host
//! beside Explorer) must pass every message on to the real one, unchanged, and must never be the cause of a message being lost.
//! Rules, each with tests:
//!
//! * **Which window is the real one**: a top-level window of class `Shell_TrayWnd` (checked by class name: a Windows 11 21H2 bug made
//!   `FindWindow` return it for classes that do not exist) that is not ours, not owned by our process and not owned by a program with
//!   our own program name (two copies of the spy must not forward to each other in a loop). Preferred: the one owned by
//!   `explorer.exe`; second choice: one whose program could not be determined (delivering beats dropping, and the class and
//!   ownership checks above already hold); other tray hosts only when the caller says so.
//! * **If there is none**: the message is refused with reply 0 (what the sender would have got with no taskbar at all) and the caller
//!   logs it. Nothing pretends to have added an icon.
//! * **A parse error never drops a message**: [`plan`] parses only to describe the message; the action it returns depends on the
//!   candidates alone.
//! * **What is forwarded and how**: `WM_COPYDATA`, `WM_COMMAND`, `WM_ACTIVATEAPP` and the `WM_USER` and `WM_APP` ranges (0x400..0xBFFF)
//!   are passed on (`WM_USER + 372` posted, as other hosts do, the rest sent with a timeout); registered messages such as
//!   `TaskbarCreated` (broadcasts that Explorer's window gets by itself; the spy logs each distinct one once) and the window's own
//!   housekeeping messages are not.

use super::{TrayRequest, WireError, parse_copydata, TRAY_CLASS};

/// A top-level window as the caller enumerated it (in z-order, top first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub hwnd: isize,
    /// Its class name as `GetClassNameW` returned it, not as the search asked for it.
    pub class: String,
    pub pid: u32,
    /// The file name of its program (`explorer.exe`), `None` when it could not be queried.
    pub exe: Option<String>,
}

/// Who we are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelfIdentity {
    pub hwnd: isize,
    pub pid: u32,
    /// Our program's file name (`tray_spy.exe`).
    pub exe: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PickPolicy {
    /// Also accept a tray window owned by some other program than Explorer (another shell, another relay). Off by default.
    pub accept_other_hosts: bool,
}

/// How a window was judged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Explorer's own tray window.
    Real,
    /// A tray window whose program is unknown.
    Unknown,
    /// A tray window of some other program.
    OtherHost,
    /// Our window, or a window of our process or our program.
    Ours,
    /// Not of class `Shell_TrayWnd`.
    WrongClass,
}

fn same_exe(a: &str, b: &str) -> bool {
    let name = |s: &str| s.rsplit(['\\', '/']).next().unwrap_or("").to_ascii_lowercase();
    !name(a).is_empty() && name(a) == name(b)
}

pub fn judge(w: &WindowInfo, me: &SelfIdentity) -> Verdict {
    if w.class != TRAY_CLASS {
        return Verdict::WrongClass;
    }
    if w.hwnd == me.hwnd || w.pid == me.pid || w.exe.as_deref().is_some_and(|e| same_exe(e, &me.exe)) {
        return Verdict::Ours;
    }
    match w.exe.as_deref() {
        Some(e) if same_exe(e, "explorer.exe") => Verdict::Real,
        Some(_) => Verdict::OtherHost,
        None => Verdict::Unknown,
    }
}

/// What to do with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Pass it to this window, unchanged.
    Forward { target: isize },
    /// There is nobody to pass it to: answer with `reply` (0).
    Refuse { reply: isize },
}

/// The best window to forward to, from `candidates` in z-order (first is topmost). The first window of the best verdict wins.
pub fn pick_real_tray(candidates: &[WindowInfo], me: &SelfIdentity, policy: PickPolicy) -> Option<isize> {
    let mut best: Option<(u8, isize)> = None;
    for w in candidates {
        let rank = match judge(w, me) {
            Verdict::Real => 0,
            Verdict::Unknown => 1,
            Verdict::OtherHost if policy.accept_other_hosts => 2,
            _ => continue,
        };
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, w.hwnd));
        }
    }
    best.map(|(_, h)| h)
}

pub fn decide(candidates: &[WindowInfo], me: &SelfIdentity, policy: PickPolicy) -> Action {
    match pick_real_tray(candidates, me, policy) {
        Some(target) => Action::Forward { target },
        None => Action::Refuse { reply: 0 },
    }
}

/// What the relay does with one `WM_COPYDATA`: the parse (for the log only) and the action (from the candidates only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub parsed: Result<TrayRequest, WireError>,
    pub action: Action,
}

pub fn plan(dw_data: usize, bytes: &[u8], candidates: &[WindowInfo], me: &SelfIdentity, policy: PickPolicy) -> Plan {
    Plan { parsed: parse_copydata(dw_data, bytes), action: decide(candidates, me, policy) }
}

/// How a message other than `WM_COPYDATA` is passed on, by number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `SendMessageTimeout` with `SMTO_ABORTIFHUNG`.
    Send,
    /// `PostMessage` (the message is a notification nobody waits on).
    Post,
    /// Handled by the window itself, not passed on.
    Local,
}

const WM_ACTIVATEAPP: u32 = 0x001C;
const WM_COPYDATA: u32 = 0x004A;
const WM_COMMAND: u32 = 0x0111;
const WM_USER: u32 = 0x0400;
/// The one message ManagedShell posts instead of sending (undocumented; UNVERIFIED what it is for).
const WM_USER_POSTED: u32 = WM_USER + 372;
/// First registered message (`RegisterWindowMessage`). Below it lies the `WM_APP` range (0x8000..0xBFFF), which is private to a window
/// class and never broadcast, so a program that sends one to the tray window means it for Explorer's window and it is passed on.
const REGISTERED_FIRST: u32 = 0xC000;
/// Registered message ids are 0xC000..=0xFFFF; the most distinct ones the spy writes to its log (a hostile sender cannot grow it).
pub const LOCAL_LOG_LIMIT: usize = 256;

/// Whether `msg` is in the registered-message range (0xC000..=0xFFFF).
pub fn is_registered_message(msg: u32) -> bool {
    (REGISTERED_FIRST..=0xFFFF).contains(&msg)
}

/// How the window procedure of a relay treats message `msg`. `taskbar_created` is the id `RegisterWindowMessage("TaskbarCreated")`
/// returned. Everything from `WM_USER` up to the end of the `WM_APP` range is passed on (the reference relays pass every message from
/// `WM_USER` up). Registered messages (0xC000 and up) are the exception: most are broadcasts that Explorer's tray window receives by
/// itself, so passing them on would deliver them twice. They stay local, and the spy writes each distinct one to its log once
/// ([`should_log_local`]) so that a message that was meant for Explorer alone would still show up in the probe.
pub fn method_for(msg: u32, taskbar_created: u32) -> Method {
    if msg == taskbar_created && taskbar_created != 0 {
        return Method::Local;
    }
    match msg {
        WM_COPYDATA | WM_COMMAND | WM_ACTIVATEAPP => Method::Send,
        WM_USER_POSTED => Method::Post,
        m if (WM_USER..REGISTERED_FIRST).contains(&m) => Method::Send,
        _ => Method::Local,
    }
}

/// Whether a message that stayed local should be written to the log now: a registered message other than `TaskbarCreated` that has not
/// been logged yet. Records it in `seen` (at most [`LOCAL_LOG_LIMIT`] ids).
pub fn should_log_local(msg: u32, taskbar_created: u32, seen: &mut std::collections::HashSet<u32>) -> bool {
    if !is_registered_message(msg) || (msg == taskbar_created && taskbar_created != 0) || seen.len() >= LOCAL_LOG_LIMIT {
        return false;
    }
    seen.insert(msg)
}

/// Whether a start-up may go ahead: Explorer's own tray window exists. [`pick_real_tray`] also accepts a window of unknown owner while
/// running (delivering beats dropping), but starting because of one could pair two relays with each other.
pub fn real_tray_present(candidates: &[WindowInfo], me: &SelfIdentity) -> bool {
    candidates.iter().any(|w| judge(w, me) == Verdict::Real)
}

/// How a forward ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// The target answered with this value.
    Delivered(isize),
    /// The target did not answer within the limit (or is hung).
    TimedOut,
    /// The target window is gone or the call failed.
    TargetGone,
    /// The message was posted; there is no answer.
    Posted,
}

/// The value the relay's window procedure returns to the sender for a forwarded message: the target's own answer when there is one, 0
/// (failure) otherwise.
pub fn reply_for(outcome: Outcome) -> isize {
    match outcome {
        Outcome::Delivered(r) => r,
        Outcome::TimedOut | Outcome::TargetGone | Outcome::Posted => 0,
    }
}

/// Whether the relay should look for the real tray again and retry once: the target vanished (Explorer restarted).
pub fn should_retry(outcome: Outcome) -> bool {
    matches!(outcome, Outcome::TargetGone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray_wire::XorShift;
    use crate::tray_wire::notify::tests_support::golden_full;

    fn me() -> SelfIdentity {
        SelfIdentity { hwnd: 100, pid: 5000, exe: "tray_spy.exe".into() }
    }

    fn win(hwnd: isize, pid: u32, exe: Option<&str>) -> WindowInfo {
        WindowInfo { hwnd, class: TRAY_CLASS.into(), pid, exe: exe.map(str::to_string) }
    }

    const P: PickPolicy = PickPolicy { accept_other_hosts: false };

    #[test]
    fn explorers_window_is_picked() {
        let c = [win(100, 5000, Some("tray_spy.exe")), win(200, 1234, Some("explorer.exe"))];
        assert_eq!(pick_real_tray(&c, &me(), P), Some(200));
        assert_eq!(decide(&c, &me(), P), Action::Forward { target: 200 });
    }

    #[test]
    fn never_forwards_to_ourselves() {
        // only our own window: nothing to forward to
        let c = [win(100, 5000, Some("tray_spy.exe"))];
        assert_eq!(pick_real_tray(&c, &me(), P), None);
        assert_eq!(decide(&c, &me(), P), Action::Refuse { reply: 0 });
        // our hwnd, whatever the pid/exe say
        let c = [win(100, 1, Some("explorer.exe"))];
        assert_eq!(pick_real_tray(&c, &me(), P), None);
        // another window of our process
        let c = [win(101, 5000, None), win(102, 5000, Some("explorer.exe"))];
        assert_eq!(pick_real_tray(&c, &me(), P), None);
    }

    #[test]
    fn a_second_copy_of_the_same_program_is_not_a_target() {
        // two spies stacked in front of Explorer would otherwise forward to each other forever
        let c = [win(300, 6000, Some(r"C:\x\TRAY_SPY.EXE")), win(200, 1234, Some("explorer.exe"))];
        assert_eq!(judge(&c[0], &me()), Verdict::Ours);
        assert_eq!(pick_real_tray(&c, &me(), P), Some(200));
        let only = [win(300, 6000, Some("tray_spy.exe"))];
        assert_eq!(pick_real_tray(&only, &me(), PickPolicy { accept_other_hosts: true }), None);
    }

    #[test]
    fn explorer_beats_unknown_beats_other_hosts_regardless_of_z_order() {
        let other = win(10, 1, Some("retrobar.exe"));
        let unknown = win(20, 2, None);
        let real = win(30, 3, Some("Explorer.EXE"));
        let all = PickPolicy { accept_other_hosts: true };
        assert_eq!(pick_real_tray(&[other.clone(), unknown.clone(), real.clone()], &me(), all), Some(30));
        assert_eq!(pick_real_tray(&[other.clone(), unknown.clone()], &me(), all), Some(20));
        assert_eq!(pick_real_tray(std::slice::from_ref(&other), &me(), all), Some(10));
        // without the policy other hosts are refused
        assert_eq!(pick_real_tray(std::slice::from_ref(&other), &me(), P), None);
        assert_eq!(pick_real_tray(&[other, unknown.clone()], &me(), P), Some(20));
        // the first of equal rank wins
        let real2 = win(31, 4, Some("explorer.exe"));
        assert_eq!(pick_real_tray(&[real, real2], &me(), P), Some(30));
    }

    #[test]
    fn the_class_is_checked_not_assumed() {
        let mut w = win(200, 1234, Some("explorer.exe"));
        w.class = "Shell_SecondaryTrayWnd".into();
        assert_eq!(judge(&w, &me()), Verdict::WrongClass);
        w.class = "shell_traywnd".into();
        assert_eq!(judge(&w, &me()), Verdict::WrongClass, "class names compare exactly");
        assert_eq!(pick_real_tray(&[w], &me(), P), None);
    }

    #[test]
    fn exe_paths_and_cases_compare_by_file_name() {
        assert!(same_exe(r"C:\Windows\explorer.exe", "EXPLORER.EXE"));
        assert!(same_exe("C:/Windows/explorer.exe", "explorer.exe"));
        assert!(!same_exe("explorer.exe", "explorer.exe.bak"));
        assert!(!same_exe("", ""), "empty names never match");
        assert!(!same_exe(r"C:\x\", r"C:\y\"));
    }

    #[test]
    fn nobody_to_forward_to_refuses_with_zero() {
        assert_eq!(decide(&[], &me(), P), Action::Refuse { reply: 0 });
    }

    #[test]
    fn a_parse_error_never_changes_the_action() {
        let c = [win(200, 1234, Some("explorer.exe"))];
        let want = Action::Forward { target: 200 };
        // malformed bytes of every kind, and a bad kind
        for (dw, bytes) in [
            (1usize, vec![]),
            (1, vec![1, 2, 3]),
            (1, vec![0; 964]),
            (0, vec![9; 10]),
            (2, vec![]),
            (3, vec![0xFF; 5]),
            (77, vec![1; 40]),
            (usize::MAX, vec![]),
        ] {
            let p = plan(dw, &bytes, &c, &me(), P);
            assert!(p.parsed.is_err(), "{dw} {bytes:?}");
            assert_eq!(p.action, want);
        }
        // and with nobody to forward to the parse still runs and the action is the refusal
        let p = plan(1, &golden_full(), &[], &me(), P);
        assert!(p.parsed.is_ok());
        assert_eq!(p.action, Action::Refuse { reply: 0 });
    }

    #[test]
    fn random_input_always_gets_an_action() {
        let mut rng = XorShift(0x1234_5678_9ABC_DEF1);
        let c = [win(200, 1234, Some("explorer.exe"))];
        for _ in 0..5_000 {
            let n = rng.below(1500) as usize;
            let bytes = rng.bytes(n);
            let dw = rng.below(8) as usize;
            assert_eq!(plan(dw, &bytes, &c, &me(), P).action, Action::Forward { target: 200 });
        }
    }

    const TC: u32 = 0xC0BE;

    #[test]
    fn forwarding_methods() {
        assert_eq!(method_for(0x004A, TC), Method::Send, "WM_COPYDATA");
        assert_eq!(method_for(0x0111, TC), Method::Send, "WM_COMMAND");
        assert_eq!(method_for(0x001C, TC), Method::Send, "WM_ACTIVATEAPP");
        assert_eq!(method_for(0x0400 + 50, TC), Method::Send, "taskbar button messages");
        assert_eq!(method_for(0x0400 + 372, TC), Method::Post);
        assert_eq!(method_for(0x0400, TC), Method::Send);
        assert_eq!(method_for(0x7FFF, TC), Method::Send, "the last WM_USER-range message");
    }

    #[test]
    fn the_wm_app_range_is_private_to_the_window_and_is_passed_on() {
        // WM_APP messages are never broadcast, so a program that sends one to the tray window means it for Explorer's
        assert_eq!(method_for(0x8000, TC), Method::Send);
        assert_eq!(method_for(0xBFFF, TC), Method::Send);
        assert_eq!(method_for(0x9123, 0), Method::Send, "also when TaskbarCreated could not be registered");
    }

    #[test]
    fn registered_messages_stay_local_but_are_reported_once() {
        // passing a broadcast on would deliver it twice to Explorer's own tray window
        assert_eq!(method_for(0xC000, TC), Method::Local, "registered messages");
        assert_eq!(method_for(0xFFFF, TC), Method::Local);
        assert_eq!(method_for(u32::MAX, TC), Method::Local);
        assert!(is_registered_message(0xC000) && is_registered_message(0xFFFF));
        assert!(!is_registered_message(0xBFFF) && !is_registered_message(0x0400) && !is_registered_message(u32::MAX));
        // every distinct one is written to the log the first time it arrives (the probe must show what was not relayed), not again
        let mut seen = std::collections::HashSet::new();
        assert!(should_log_local(0xC123, TC, &mut seen));
        assert!(!should_log_local(0xC123, TC, &mut seen));
        assert!(should_log_local(0xC124, TC, &mut seen));
        // TaskbarCreated is the one broadcast everybody knows; housekeeping and out-of-range ids are not interesting
        assert!(!should_log_local(TC, TC, &mut seen));
        assert!(!should_log_local(0x0014, TC, &mut seen));
        assert!(!should_log_local(0x0400, TC, &mut seen));
        // a flood of distinct ids cannot grow the set without limit
        for m in 0xC000..=0xFFFFu32 {
            should_log_local(m, TC, &mut seen);
        }
        assert!(seen.len() <= LOCAL_LOG_LIMIT);
    }

    #[test]
    fn only_explorers_own_window_opens_the_gate() {
        let unknown = win(20, 2, None);
        let other = win(10, 1, Some("retrobar.exe"));
        let real = win(30, 3, Some("explorer.exe"));
        let mine = win(100, 5000, Some("tray_spy.exe"));
        assert!(real_tray_present(&[mine.clone(), real], &me()));
        // an unidentified window is an acceptable target while running, not a reason to start: it could be another relay
        assert!(!real_tray_present(&[unknown, other, mine], &me()));
        assert!(!real_tray_present(&[], &me()));
    }

    #[test]
    fn taskbar_created_and_housekeeping_stay_local() {
        assert_eq!(method_for(TC, TC), Method::Local);
        for m in [0x0001u32, 0x0002, 0x0005, 0x000F, 0x0010, 0x0014, 0x001A, 0x0046, 0x0047, 0x0113, 0x007E, 0x0214, 0x0081, 0x0083, 0x0085, 0x00FF] {
            // WM_CREATE, DESTROY, SIZE, PAINT, CLOSE, ERASEBKGND, SETTINGCHANGE, WINDOWPOSCHANGING/ED, TIMER, DISPLAYCHANGE, SIZING, NCCREATE, NCCALCSIZE, NCPAINT, INPUT
            assert_eq!(method_for(m, TC), Method::Local, "{m:#x}");
        }
        // an unregistered TaskbarCreated (0) must not turn message 0 into a local-only special case by accident
        assert_eq!(method_for(0, 0), Method::Local);
        assert_eq!(method_for(0xC123, 0), Method::Local);
    }

    #[test]
    fn replies_come_from_the_target_or_are_zero() {
        assert_eq!(reply_for(Outcome::Delivered(1)), 1);
        assert_eq!(reply_for(Outcome::Delivered(0)), 0);
        assert_eq!(reply_for(Outcome::Delivered(-5)), -5);
        assert_eq!(reply_for(Outcome::Delivered(0x0003_0001)), 0x0003_0001);
        assert_eq!(reply_for(Outcome::TimedOut), 0);
        assert_eq!(reply_for(Outcome::TargetGone), 0);
        assert_eq!(reply_for(Outcome::Posted), 0);
        assert!(should_retry(Outcome::TargetGone));
        assert!(!should_retry(Outcome::TimedOut), "a hung target is not asked twice: the sender is waiting");
        assert!(!should_retry(Outcome::Delivered(0)));
    }
}
