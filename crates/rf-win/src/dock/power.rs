//! The Start panel's power button: lock, sleep, sign out, restart, shut down, and the name of the user shown beside it.
//!
//! Nothing here runs by itself. [`perform`] makes the real call and is reached only from the user's click on the item (restart and
//! shut down after the "Emin misiniz?" step has been answered with yes); every decision around it is in [`Menu`], which is pure and
//! tested with no call at all. None of the calls needs administrator rights.
//!
//! Notes from the research: `ExitWindowsEx` and `SetSuspendState` need `SeShutdownPrivilege`, which the user's token holds but
//! disabled, so it is enabled first (a token privilege, not elevation). `ExitWindowsEx` is asynchronous (a true answer only means
//! the request was accepted) and gets a planned reason code, never 0.

/// What the power menu can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Lock,
    Sleep,
    SignOut,
    Restart,
    Shutdown,
}

impl Action {
    /// The items of the menu, top to bottom.
    pub const ALL: [Action; 5] = [Action::Lock, Action::Sleep, Action::SignOut, Action::Restart, Action::Shutdown];

    /// Restart and shut down close every program: they ask first. Lock, sleep and sign out are what the user asked for with the
    /// click (a sign out asks for unsaved work through the programs themselves).
    pub fn needs_confirmation(self) -> bool {
        matches!(self, Action::Restart | Action::Shutdown)
    }
}

/// What the panel shows of the power button.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Menu {
    #[default]
    Closed,
    /// The five items.
    Open,
    /// "Emin misiniz?" for this action.
    Confirming(Action),
}

/// What the click on an item leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Make the call now.
    Run(Action),
    /// Nothing happens yet; the menu changed.
    Asking,
}

/// An "Evet" that comes this soon after the question appeared is not an answer. The question appears under the pointer that chose
/// "Yeniden başlat" or "Bilgisayarı kapat", and the second click of a double click would otherwise answer a question nobody has read.
pub const CONFIRM_GRACE_MS: u64 = 600;

/// Whether an "Evet" (a click or Enter on it) counts, `shown_for_ms` after the question appeared. "Hayır" always counts.
pub fn yes_counts(shown_for_ms: u64) -> bool {
    shown_for_ms >= CONFIRM_GRACE_MS
}

impl Menu {
    /// The power button itself: opens the menu, or closes it (also from the question).
    pub fn toggled(self) -> Menu {
        match self {
            Menu::Closed => Menu::Open,
            Menu::Open | Menu::Confirming(_) => Menu::Closed,
        }
    }

    /// An item was clicked (or chosen with Enter). Returns the new state of the menu and what to do.
    pub fn choose(self, action: Action) -> (Menu, Step) {
        match self {
            Menu::Closed => (Menu::Closed, Step::Asking),
            Menu::Open if action.needs_confirmation() => (Menu::Confirming(action), Step::Asking),
            Menu::Open => (Menu::Closed, Step::Run(action)),
            // the question is only answered by its own buttons; a click on an item meanwhile asks about that one
            Menu::Confirming(_) if action.needs_confirmation() => (Menu::Confirming(action), Step::Asking),
            Menu::Confirming(_) => (Menu::Closed, Step::Run(action)),
        }
    }

    /// The answer to the question: yes runs the action, no goes back to the items.
    pub fn answer(self, yes: bool) -> (Menu, Step) {
        match self {
            Menu::Confirming(a) if yes => (Menu::Closed, Step::Run(a)),
            Menu::Confirming(_) => (Menu::Open, Step::Asking),
            other => (other, Step::Asking),
        }
    }

    /// Escape: leaves the question for the items, the items for nothing. False when the menu was already closed (Escape then belongs
    /// to the panel).
    pub fn escape(self) -> (Menu, bool) {
        match self {
            Menu::Closed => (Menu::Closed, false),
            Menu::Open => (Menu::Closed, true),
            Menu::Confirming(_) => (Menu::Open, true),
        }
    }
}

use windows::Win32::Foundation::{CloseHandle, HANDLE, LUID};
use windows::Win32::Security::{AdjustTokenPrivileges, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW, SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_PRIVILEGES, TOKEN_QUERY};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::w;

/// Switches on `SeShutdownPrivilege` in this process's token (the user's token has it, switched off).
fn enable_shutdown_privilege() -> anyhow::Result<()> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token)?;
        let mut luid = LUID::default();
        let looked = LookupPrivilegeValueW(None, w!("SeShutdownPrivilege"), &mut luid);
        let result = looked.and_then(|_| {
            let tp = TOKEN_PRIVILEGES { PrivilegeCount: 1, Privileges: [LUID_AND_ATTRIBUTES { Luid: luid, Attributes: SE_PRIVILEGE_ENABLED }] };
            AdjustTokenPrivileges(token, false, Some(&tp), 0, None, None)
        });
        let _ = CloseHandle(token);
        result?;
    }
    Ok(())
}

/// Makes the call. ONLY from the user's click (see the module docs); never from a test or a probe. Blocks briefly: a worker thread.
pub fn perform(action: Action) -> anyhow::Result<()> {
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{EWX_LOGOFF, EWX_REBOOT, EWX_SHUTDOWN, EXIT_WINDOWS_FLAGS, ExitWindowsEx, LockWorkStation, SHUTDOWN_REASON};
    // SHTDN_REASON_FLAG_PLANNED | major "other" | minor "other": a planned, ordinary request
    let planned = SHUTDOWN_REASON(0x8000_0000);
    match action {
        Action::Lock => unsafe { LockWorkStation()? },
        Action::Sleep => {
            enable_shutdown_privilege()?;
            anyhow::ensure!(unsafe { SetSuspendState(false, false, false) }, "the system refused to sleep");
        }
        Action::SignOut => unsafe { ExitWindowsEx(EWX_LOGOFF, planned)? },
        Action::Restart => {
            enable_shutdown_privilege()?;
            unsafe { ExitWindowsEx(EWX_REBOOT, planned)? }
        }
        Action::Shutdown => {
            enable_shutdown_privilege()?;
            // EWX_HYBRID_SHUTDOWN: what Windows' own button does (fast start-up where it is on)
            unsafe { ExitWindowsEx(EXIT_WINDOWS_FLAGS(EWX_SHUTDOWN.0 | 0x0040_0000), planned)? }
        }
    }
    Ok(())
}

/// The name of the signed-in user as Windows shows it ("Ad Soyad"), else the account name.
pub fn display_name() -> String {
    use windows::Win32::Security::Authentication::Identity::{GetUserNameExW, NameDisplay};
    let mut buf = [0u16; 256];
    let mut n = buf.len() as u32;
    let got = unsafe { GetUserNameExW(NameDisplay, Some(windows::core::PWSTR(buf.as_mut_ptr())), &mut n) };
    if got && n > 0 {
        let s = String::from_utf16_lossy(&buf[..n as usize]);
        if !s.trim().is_empty() {
            return s;
        }
    }
    std::env::var("USERNAME").unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(m: Menu, a: Action) -> (Menu, Step) {
        m.choose(a)
    }

    #[test]
    fn lock_sleep_and_sign_out_run_at_once_restart_and_shutdown_ask_first() {
        for a in [Action::Lock, Action::Sleep, Action::SignOut] {
            assert!(!a.needs_confirmation());
            assert_eq!(run(Menu::Open, a), (Menu::Closed, Step::Run(a)));
        }
        for a in [Action::Restart, Action::Shutdown] {
            assert!(a.needs_confirmation());
            assert_eq!(run(Menu::Open, a), (Menu::Confirming(a), Step::Asking), "nothing runs before the answer");
        }
    }

    #[test]
    fn the_question_runs_the_action_only_on_yes() {
        let asking = Menu::Confirming(Action::Shutdown);
        assert_eq!(asking.answer(true), (Menu::Closed, Step::Run(Action::Shutdown)));
        assert_eq!(asking.answer(false), (Menu::Open, Step::Asking));
        // an answer with no question runs nothing
        assert_eq!(Menu::Open.answer(true), (Menu::Open, Step::Asking));
        assert_eq!(Menu::Closed.answer(true), (Menu::Closed, Step::Asking));
    }

    #[test]
    fn a_click_on_another_item_while_asking_asks_about_that_one_and_never_runs_the_old_one() {
        let asking = Menu::Confirming(Action::Restart);
        assert_eq!(asking.choose(Action::Shutdown), (Menu::Confirming(Action::Shutdown), Step::Asking));
        assert_eq!(asking.choose(Action::Lock), (Menu::Closed, Step::Run(Action::Lock)));
        // a menu that is closed runs nothing whatever arrives
        for a in Action::ALL {
            assert_eq!(Menu::Closed.choose(a).1, Step::Asking);
        }
    }

    #[test]
    fn a_yes_in_the_moment_after_the_question_appears_is_not_an_answer() {
        // the second click of a double click on "Yeniden başlat" lands on the question that the first click opened
        assert!(!yes_counts(0));
        assert!(!yes_counts(120));
        assert!(!yes_counts(CONFIRM_GRACE_MS - 1));
        assert!(yes_counts(CONFIRM_GRACE_MS));
        assert!(yes_counts(5_000));
    }

    #[test]
    fn escape_steps_back_one_level_and_leaves_the_panel_alone_until_the_menu_is_closed() {
        assert_eq!(Menu::Confirming(Action::Restart).escape(), (Menu::Open, true));
        assert_eq!(Menu::Open.escape(), (Menu::Closed, true));
        assert_eq!(Menu::Closed.escape(), (Menu::Closed, false));
    }

    #[test]
    fn the_button_opens_and_closes_the_menu() {
        assert_eq!(Menu::Closed.toggled(), Menu::Open);
        assert_eq!(Menu::Open.toggled(), Menu::Closed);
        assert_eq!(Menu::Confirming(Action::Restart).toggled(), Menu::Closed);
    }

    #[test]
    fn five_items_in_the_order_of_the_menu() {
        assert_eq!(Action::ALL, [Action::Lock, Action::Sleep, Action::SignOut, Action::Restart, Action::Shutdown]);
    }

    /// Read-only: the name Windows shows for the user.
    #[test]
    fn the_user_has_a_display_name() {
        let n = display_name();
        eprintln!("display name: {} characters", n.chars().count());
        assert!(!n.trim().is_empty());
    }
}
