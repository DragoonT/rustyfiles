//! The dock (experimental): `rustyfiles.exe --dock` puts three floating islands above the bottom of the primary monitor, in the
//! spirit of a taskbar of our own. Version 1 does not hide or replace the Windows taskbar and is not the shell; Explorer keeps the
//! desktop, the Start menu and the notifications, nothing is injected into anything and nothing needs elevation. Everything it does
//! is undone when it exits.
//!
//! * LEFT: the Windows logo (opens our own Start panel, `panel.rs` / `start.rs`; the Windows key when "rustyfiles Başlat menüsünü kullan" is off), a
//!   search field (the same panel with the keyboard in its field, results from the quick search's sources: `search.rs`) and the
//!   weather of the city set in Settings. `left.rs`
//! * MIDDLE: the dock. The apps pinned to the Windows taskbar, the other running apps, a separator, the Recycle Bin and the
//!   Downloads folder; the icon under the pointer grows. `middle.rs`
//! * RIGHT: network and volume symbols that show the real state and open our own control centre (`control_centre.rs`; Win+A when
//!   "rustyfiles denetim merkezini kullan" is off), the time and date (our own calendar panel, `cal_view.rs`; Win+N when
//!   "rustyfiles takvimi kullan" is off), a bell (our own notification panel, `notif_view.rs`, with a dot while there are notifications;
//!   Win+N when "rustyfiles bildirim panelini kullan" is off). `right.rs`
//!
//! Each island is a window of its own (a GPUI pop-up with a transparent background) that is never activated, has no taskbar
//! button and is always on top; `rf_win::dock` makes it so and places it. Each has HyperGlass behind it with its own corner radius
//! (`backdrop.rs`), and paints its own translucent fill while the glass is not running (`host::island_box`).
//!
//! THE PROCESS. `--dock` is a process of its own, like `--bar-preview`: it never hands over to a running rustyfiles, never starts
//! DirectLook, owns no tray icon, hotkey or name index, and never writes the settings. It is a single instance of its own group
//! (`control::APP`) whose pipe also carries `stop` and `reload`. The main rustyfiles starts it with the "Show the dock" setting, at
//! its own start-up and when the switch goes on, and stops it when the switch goes off (`control::sync_process`). The dock keeps
//! running when rustyfiles' windows close: it is the user's taskbar. `RUSTYFILES_NO_DOCK=1` keeps a rustyfiles from starting or
//! stopping one.
//!
//! WHAT IS PURE (and tested): the widths and the icon layout (`plan.rs`), the glass state machine (`backdrop.rs`), the colours
//! (`look.rs`), the light or dark foreground that follows the brightness behind the glass (`tone.rs`), the process decisions (`control.rs`), the settings differences (`host.rs`), the icon cache (`icons.rs`) and the
//! click commands (`actions.rs`). The rectangles, the hover animation, the click decision, the clock, the weather parsing and the
//! full-screen decision are pure and tested in `rf_win::dock` / `rf_win::weather`.

pub mod actions;
mod backdrop;
mod cal_view;
mod cc_model;
pub mod control;
mod control_centre;
mod host;
mod icons;
mod left;
mod look;
mod middle;
mod notif_model;
mod notif_state;
mod notif_view;
mod panel;
mod panel_parts;
mod plan;
mod right;
mod search;
mod start;
mod sysstate;
mod tone;

use futures::channel::mpsc::UnboundedReceiver;
use gpui_kit::App;

/// The command-line argument that makes this process the dock.
pub const ARG: &str = "--dock";

/// Takes the dock's single-instance place. `None`: a dock is already running (it has been greeted) and this process should end.
/// The guard must be kept for the life of the process; the receiver carries the messages other processes send (`stop`, `reload`).
pub fn acquire() -> Option<(rf_win::single_instance::InstanceGuard, UnboundedReceiver<String>)> {
    match rf_win::single_instance::acquire_app(control::APP, "hello") {
        rf_win::single_instance::Role::Forwarded => None,
        rf_win::single_instance::Role::Primary(guard) => {
            let (tx, rx) = futures::channel::mpsc::unbounded::<String>();
            rf_win::single_instance::serve_app(control::APP, move |m| {
                let _ = tx.unbounded_send(m);
            });
            Some((guard, rx))
        }
    }
}

/// Runs the dock in this (already started) application: opens the islands and keeps them up to date until told to stop.
pub fn run(messages: Option<UnboundedReceiver<String>>, cx: &mut App) {
    host::run(messages, cx);
}
