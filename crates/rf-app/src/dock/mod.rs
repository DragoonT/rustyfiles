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
//!   A right click on an app opens the dock's own app menu (`app_menu.rs`: jump-list sections through `jump_source.rs`, the app's name,
//!   pin / unpin on the dock's own list `own_pins.rs`, close), and resting the pointer on a running app shows its windows
//!   (`hover_preview.rs`: live pictures, never activated).
//! * RIGHT: network and volume symbols that show the real state and open our own control centre (`control_centre.rs`; Win+A when
//!   "rustyfiles denetim merkezini kullan" is off), the time and date (our own calendar panel, `cal_view.rs`; Win+N when
//!   "rustyfiles takvimi kullan" is off), a bell (our own notification panel, `notif_view.rs`, with a dot while there are notifications;
//!   Win+N when "rustyfiles bildirim panelini kullan" is off). `right.rs`
//!
//! A DOCK ON EVERY MONITOR (setting "Dock'u her ekranda göster", on by default; see docs/design/dock.md): `host.rs` keeps a list of docks,
//! the home dock (the primary monitor's, the one above) and one for each other monitor, found by the monitor's device name
//! (`rf_win::dock::monitors`). A secondary dock has the logo and the search, the apps, and the clock and the bell. With one monitor, or
//! the switch off, there is the home dock only.
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
mod app_menu;
mod backdrop;
mod cal_view;
mod cc_model;
pub mod control;
mod control_centre;
// the middle island's highlight while an app from the Start panel is dragged over it
mod drop_hint;
mod eject_view;
mod empty;
mod feedback;
mod grow_host;
mod grow_head;
mod grow_view;
mod host;
mod hover_preview;
mod icon_levels;
mod icons;
mod jump_source;
mod left;
pub(crate) mod look;
mod middle;
mod notif_model;
mod notif_state;
mod notif_view;
mod own_pins;
pub(crate) mod parts;
pub(crate) mod parts_status;
mod panel;
pub(crate) mod panel_parts;
mod plan;
mod popup_win;
mod reorder;
mod right;
pub mod size_pref;
pub mod status_card;
pub(crate) mod size_slider;
mod search;
mod start;
mod start_button;
mod sys_menu;
mod sysstate;
mod thin;
mod task_feed;
mod tone;
mod tray_overflow;
pub(crate) mod tray_feed;
mod tray_source;
mod tray_strip;
mod tray_tip;
// the weather's condition in words, and Settings' card for the weather from the PC's position
pub(crate) mod weather_card;
mod weather_text;

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

/// Opens the Start panel, or closes it when it is the open one: what a click on the logo does. The shell host's keyboard layer
/// (`shell/keylayer.rs`) calls it for a tap of the Windows key. False: it could not open (see `panel::toggle_start_reported`).
pub fn toggle_start(cx: &mut App) -> bool {
    panel::toggle_start_reported(cx)
}

/// Moves the keyboard into the right island's tray strip (the arrows walk its icons, Enter and Shift+F10 click them, Escape leaves).
/// False: there is no strip to walk (no tray source, no icons) or a full-screen program is in front. Nothing calls this by itself: a key
/// is wired to it when the tray host is (`RUSTYFILES_TRAY_FAKE_FOCUS_MS` tries it with the made-up source).
pub fn focus_tray_strip(cx: &mut App) -> bool {
    tray_strip::focus_strip(cx)
}

/// Whether a full-screen application is in front (the islands are hidden). The keyboard layer stands down then.
pub fn is_fullscreen(cx: &App) -> bool {
    host::any_fullscreen(cx)
}
