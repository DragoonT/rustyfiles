//! Show Desktop and Minimize All, carried out. The decisions (which windows, what to remember, in which order to restore) are
//! `rf_core::desktop::cover::showdesktop`; this file asks the system what windows there are, minimizes and restores them, and raises
//! or lowers the cover windows. The system is behind the [`WindowOps`] trait, so that the whole sequence is tested over a fake.
//!
//! NOT BOUND TO A KEY in this increment: nothing calls [`Driver`] yet but the tests. The real [`SystemOps`] is compiled and never run
//! against the user's windows.

use rf_core::desktop::cover::Hwnd;
use rf_core::desktop::cover::showdesktop::{Action, Now, State, Win};

/// What the driver needs of the system.
pub trait WindowOps {
    /// The top-level windows from the top of the z-order to the bottom.
    fn z_order(&self) -> Vec<Win>;
    /// What is true of these windows now.
    fn now(&self, windows: &[Hwnd]) -> Vec<Now>;
    /// Minimizes without activating anything.
    fn minimize(&self, hwnd: Hwnd);
    /// Brings a minimized window back.
    fn restore(&self, hwnd: Hwnd);
    /// The cover windows go above the windows that stayed (`true`) or back to their place (`false`).
    fn raise_desktop(&self, raised: bool);
}

/// Show Desktop / Minimize All over some [`WindowOps`].
pub struct Driver<O: WindowOps> {
    pub ops: O,
    state: State,
}

impl<O: WindowOps> Driver<O> {
    pub fn new(ops: O) -> Driver<O> {
        Driver { ops, state: State::default() }
    }

    /// Something we minimized has not been brought back.
    pub fn showing(&self) -> bool {
        self.state.showing()
    }

    fn run(&self, action: Action) {
        match action {
            Action::Minimize { windows, raise_desktop } => {
                for h in windows {
                    self.ops.minimize(h);
                }
                if raise_desktop {
                    self.ops.raise_desktop(true);
                }
            }
            Action::Restore { windows, lower_desktop } => {
                // (the desktop goes down first, so that the windows come back above it and not under it)
                if lower_desktop {
                    self.ops.raise_desktop(false);
                }
                for h in windows {
                    self.ops.restore(h);
                }
            }
            Action::Nothing => {}
        }
    }

    /// Win+D.
    pub fn toggle_show_desktop(&mut self) {
        let z = self.ops.z_order();
        let now = self.ops.now(self.state.remembered());
        let action = self.state.toggle_show_desktop(&z, &now);
        self.run(action);
    }

    /// Win+M.
    pub fn minimize_all(&mut self) {
        let z = self.ops.z_order();
        let action = self.state.minimize_all(&z);
        self.run(action);
    }

    /// Shift+Win+M.
    pub fn undo_minimize_all(&mut self) {
        let now = self.ops.now(self.state.remembered());
        let action = self.state.undo(&now);
        self.run(action);
    }
}

/// The real system. COMPILED, NEVER RUN: it would minimize and restore the user's windows.
pub struct SystemOps;

impl WindowOps for SystemOps {
    fn z_order(&self) -> Vec<Win> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
        use windows::Win32::System::Threading::GetCurrentProcessId;
        use windows::Win32::UI::WindowsAndMessaging::{
            GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GetClassNameW, GetWindow, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW, WS_MINIMIZEBOX,
        };
        let own_pid = unsafe { GetCurrentProcessId() };
        super::cover::visible_order()
            .into_iter()
            .map(|h| {
                let w = HWND(h as *mut _);
                let (ex, style) = unsafe { (GetWindowLongPtrW(w, GWL_EXSTYLE) as u32, GetWindowLongPtrW(w, GWL_STYLE) as u32) };
                let mut cloaked = 0u32;
                let _ = unsafe { DwmGetWindowAttribute(w, DWMWA_CLOAKED, &mut cloaked as *mut u32 as *mut _, size_of::<u32>() as u32) };
                let mut pid = 0u32;
                unsafe { GetWindowThreadProcessId(w, Some(&mut pid)) };
                let mut class = [0u16; 128];
                let n = unsafe { GetClassNameW(w, &mut class) };
                let class = String::from_utf16_lossy(&class[..n.max(0) as usize]);
                let owned = unsafe { GetWindow(w, GW_OWNER) }.is_ok_and(|o| !o.0.is_null());
                let app = ex & WS_EX_APPWINDOW.0 != 0;
                Win {
                    hwnd: h,
                    visible: unsafe { IsWindowVisible(w) }.as_bool(),
                    cloaked: cloaked != 0,
                    tool_window: !app && (ex & WS_EX_TOOLWINDOW.0 != 0 || owned),
                    minimizable: style & WS_MINIMIZEBOX.0 != 0,
                    minimized: unsafe { IsIconic(w) }.as_bool(),
                    ours: pid == own_pid || super::cover::is_cover(h),
                    shell: crate::dock::fullscreen::is_shell_class(&class),
                }
            })
            .collect()
    }

    fn now(&self, windows: &[Hwnd]) -> Vec<Now> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow};
        windows
            .iter()
            .map(|&h| {
                let w = HWND(h as *mut _);
                let exists = unsafe { IsWindow(Some(w)) }.as_bool();
                Now { hwnd: h, exists, minimized: exists && unsafe { IsIconic(w) }.as_bool() }
            })
            .collect()
    }

    fn minimize(&self, hwnd: Hwnd) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SW_SHOWMINNOACTIVE, ShowWindowAsync};
        // (asynchronous: a window that does not answer must not hold the others up)
        let _ = unsafe { ShowWindowAsync(HWND(hwnd as *mut _), SW_SHOWMINNOACTIVE) };
    }

    fn restore(&self, hwnd: Hwnd) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SW_RESTORE, ShowWindowAsync};
        let _ = unsafe { ShowWindowAsync(HWND(hwnd as *mut _), SW_RESTORE) };
    }

    fn raise_desktop(&self, raised: bool) {
        super::cover::set_raised(raised);
        super::cover::reinsert_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A fake desktop: windows with a state, and a log of what was done to them.
    struct Fake {
        z: RefCell<Vec<Win>>,
        log: RefCell<Vec<String>>,
    }

    impl Fake {
        fn new(handles: &[Hwnd]) -> Fake {
            let z = handles.iter().map(|&h| Win { hwnd: h, visible: true, cloaked: false, tool_window: false, minimizable: true, minimized: false, ours: false, shell: false }).collect();
            Fake { z: RefCell::new(z), log: RefCell::new(Vec::new()) }
        }
        fn set(&self, h: Hwnd, f: impl FnOnce(&mut Win)) {
            if let Some(w) = self.z.borrow_mut().iter_mut().find(|w| w.hwnd == h) {
                f(w);
            }
        }
        fn log(&self) -> Vec<String> {
            self.log.borrow().clone()
        }
    }

    impl WindowOps for Fake {
        fn z_order(&self) -> Vec<Win> {
            self.z.borrow().clone()
        }
        fn now(&self, windows: &[Hwnd]) -> Vec<Now> {
            let z = self.z.borrow();
            windows.iter().map(|&h| z.iter().find(|w| w.hwnd == h).map_or(Now { hwnd: h, exists: false, minimized: false }, |w| Now { hwnd: h, exists: true, minimized: w.minimized })).collect()
        }
        fn minimize(&self, hwnd: Hwnd) {
            self.log.borrow_mut().push(format!("min {hwnd}"));
            self.set(hwnd, |w| w.minimized = true);
        }
        fn restore(&self, hwnd: Hwnd) {
            self.log.borrow_mut().push(format!("restore {hwnd}"));
            self.set(hwnd, |w| w.minimized = false);
        }
        fn raise_desktop(&self, raised: bool) {
            self.log.borrow_mut().push(format!("desktop {}", if raised { "up" } else { "down" }));
        }
    }

    #[test]
    fn show_desktop_then_show_desktop_again_puts_everything_back_in_reverse_order() {
        let mut d = Driver::new(Fake::new(&[10, 20, 30]));
        d.toggle_show_desktop();
        assert_eq!(d.ops.log(), ["min 10", "min 20", "min 30", "desktop up"]);
        assert!(d.showing());
        d.toggle_show_desktop();
        assert_eq!(d.ops.log()[4..], ["desktop down", "restore 30", "restore 20", "restore 10"]);
        assert!(!d.showing());
        assert!(d.ops.z_order().iter().all(|w| !w.minimized));
    }

    #[test]
    fn minimize_all_leaves_the_desktop_alone_and_its_undo_restores() {
        let mut d = Driver::new(Fake::new(&[1, 2]));
        d.minimize_all();
        assert_eq!(d.ops.log(), ["min 1", "min 2"]);
        d.undo_minimize_all();
        assert_eq!(d.ops.log()[2..], ["restore 2", "restore 1"]);
    }

    #[test]
    fn a_window_that_was_restored_by_hand_or_closed_in_between_is_not_touched_again() {
        let mut d = Driver::new(Fake::new(&[1, 2, 3]));
        d.minimize_all();
        d.ops.set(2, |w| w.minimized = false); // the user brought it back
        d.ops.z.borrow_mut().retain(|w| w.hwnd != 3); // the user closed it
        d.undo_minimize_all();
        assert_eq!(d.ops.log()[3..], ["restore 1"]);
    }

    #[test]
    fn windows_that_cannot_or_should_not_be_minimized_are_left_out() {
        let f = Fake::new(&[1, 2, 3, 4]);
        f.set(2, |w| w.minimizable = false);
        f.set(3, |w| w.ours = true);
        f.set(4, |w| w.shell = true);
        let mut d = Driver::new(f);
        d.minimize_all();
        assert_eq!(d.ops.log(), ["min 1"]);
    }

    #[test]
    fn the_real_system_can_be_asked_what_windows_there_are() {
        // read-only: nothing is minimized or restored here
        let z = SystemOps.z_order();
        assert!(!z.is_empty());
        let _ = SystemOps.now(&z.iter().map(|w| w.hwnd).take(3).collect::<Vec<_>>());
    }
}
