//! What happens when the last main window is gone: the app ends, or stays in the notification area. Also looked at again when a
//! second launch's request (see `rf_win::handover`) held back a quit and then left no window open (GitHub issue 15).

use gpui_kit::App;

/// What to do once no main window is open any more.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    /// A main window (or the desktop preview's window) is open: nothing to do.
    Nothing,
    /// The app stays in the notification area: the settings are saved, the process lives on.
    StayInBackground,
    /// The app ends (once running copies and moves are done).
    Quit,
}

/// The decision, from the facts only. The switcher preview is one window shown once: it never stays in the notification area.
pub(crate) fn next(main_window_open: bool, desktop_preview_open: bool, run_in_background: bool, quitting: bool, switcher_preview: bool) -> Next {
    if main_window_open || desktop_preview_open {
        Next::Nothing
    } else if run_in_background && !quitting && !switcher_preview {
        Next::StayInBackground
    } else {
        Next::Quit
    }
}

/// Run when a window closes, and again when a request from a second launch has been acted on while a quit was held back. Windows
/// left behind (menus, flyouts) go with the last main window, so the process cannot linger with nothing on the screen.
pub(crate) fn settle(cx: &mut App, switcher_preview: bool) {
    let main = crate::workspace::any_main_window(cx);
    let desktop = crate::shell::desktop::window_open(cx);
    let background = cx.global::<crate::settings::Settings>().run_in_background;
    let what = next(main, desktop, background, crate::background::quitting(), switcher_preview);
    if what == Next::Nothing {
        return;
    }
    // menus / flyouts left behind go too
    for w in cx.windows() {
        let _ = w.update(cx, |_, window, _| window.remove_window());
    }
    match what {
        Next::StayInBackground => {
            let s = cx.global::<crate::settings::Settings>().clone();
            std::thread::spawn(move || s.save());
        }
        _ => crate::quit_when_idle(cx),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_open_window_keeps_the_app_whatever_else_is_set() {
        for background in [false, true] {
            for quitting in [false, true] {
                assert_eq!(next(true, false, background, quitting, false), Next::Nothing);
                assert_eq!(next(false, true, background, quitting, false), Next::Nothing);
            }
        }
    }

    #[test]
    fn no_window_ends_the_app_unless_it_is_to_stay_in_the_notification_area() {
        assert_eq!(next(false, false, false, false, false), Next::Quit);
        assert_eq!(next(false, false, true, false, false), Next::StayInBackground);
        // the tray's own "quit" is on its way: the app ends
        assert_eq!(next(false, false, true, true, false), Next::Quit);
        // the switcher preview is shown once and ends
        assert_eq!(next(false, false, true, false, true), Next::Quit);
    }
}
