//! Our own window switcher (shell plan increment G1), the app's side: a centred glass panel with one card per window, a live picture
//! through DWM on each card, opened by a command or by a function the shell host calls. The model, the rules, the chord, the layout
//! and the activation sequence are `rf_win::switcher` (pure where they can be, tested there); this module makes the window.
//!
//! HOW IT IS OPENED IN THIS TRACK. Only `rustyfiles --switcher-preview` (see [`open_preview`]): the switcher is shown once, as if
//! the chord had been pressed, and stays until a window is chosen (Enter, a click), Escape, Alt+F4, or the window loses the
//! foreground (a click outside it, or a foreground that stays somebody else's, also put it away: `rf_win::switcher::watch`).
//! Tab / Shift+Tab step, the arrows move over the grid. NO global hotkey and NO keyboard hook is registered by anything
//! that runs here. The shell host, later, calls [`open`] when its chord fires and feeds the chord's effects to [`apply_effect`]
//! (the chord and the hook are `rf_win::switcher::{chord, hook}`).
//!
//! WHAT EXISTS WHEN. Nothing while the switcher is closed: no window, no thumbnails, no timer, no thread of its own. [`open`] makes
//! the window (a transparent pop-up, created when needed, not kept hidden: creating it is the cost, measured live and logged at
//! info level as "switcher: opened in ..."), lists the windows once, and registers one DWM picture per live card; [`close`] removes
//! all of it. Only a few dozen small icon bitmaps are kept in memory between two openings (`view::forget_icons` lets go of them).
//!
//! WHERE IT OPENS. On the monitor under the pointer, centred in its work area, sized by `rf_win::switcher::layout` for the number of
//! windows (up to about 12 at full size, more scale the cards down). It takes the foreground (the click or the chord that opened
//! it was ours), so choosing a window can bring that window forward by the ordinary rules; Escape and losing the foreground give
//! the window that was in front before its place back.

pub mod keys;
mod view;

use std::rc::Rc;
use std::time::Instant;

use gpui_kit::*;
use rf_win::dock::panel as geo;
use rf_win::switcher::activate::{self, Outcome, Win32Ops};
use rf_win::switcher::chord::{Effect, Scope};
use rf_win::switcher::layout;
use rf_win::switcher::session::Session;
use rf_win::switcher::{ComRoute, Options};

pub use view::{SwitcherView, forget_icons};

/// The command-line argument that shows the switcher once.
pub const ARG: &str = "--switcher-preview";

/// What the switcher is asked to open with.
#[derive(Clone)]
pub struct OpenRequest {
    pub scope: Scope,
    /// Shift+Tab opened it: start on the oldest window instead of the previous one.
    pub backwards: bool,
    /// The keys come from the host's keyboard hook ([`apply_effect`]), so the switcher's window is not expected to be the one in
    /// front: only a click outside puts it away on its own (the host ends it on Alt's release, Escape and so on). False for a
    /// switcher opened by a command, which also goes when the foreground stays somebody else's.
    pub hooked: bool,
    /// Called once, after the switcher has closed for ANY reason (a card chosen, Escape, a click, the foreground lost), so that a
    /// host can tell its chord (`Chord::closed_elsewhere`): otherwise its chord would stay open and go on swallowing Tab, the
    /// arrows, Enter and Escape for a switcher that is gone.
    pub on_closed: Option<Rc<dyn Fn(Closing, &mut App)>>,
}

impl Default for OpenRequest {
    fn default() -> Self {
        OpenRequest { scope: Scope::AllWindows, backwards: false, hooked: false, on_closed: None }
    }
}

/// Why the switcher closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closing {
    /// A window was chosen: it is brought to the front.
    Chosen(isize),
    /// Escape, Alt+F4: the window that was in front gets its place back.
    Cancelled,
    /// Another window took the foreground: the foreground is left alone.
    LostForeground,
}

struct Active {
    handle: WindowHandle<SwitcherView>,
    on_closed: Option<Rc<dyn Fn(Closing, &mut App)>>,
}
impl Global for Active {}

/// Whether the switcher is open.
pub fn is_open(cx: &App) -> bool {
    cx.has_global::<Active>()
}

/// `--switcher-preview`: the switcher once, as if the chord had been pressed. False when no window could be made (the caller then
/// ends the process: a preview without a window would stay in memory with nothing to see and nothing to click).
pub fn open_preview(cx: &mut App) -> bool {
    open(OpenRequest::default(), cx)
}

/// Opens the switcher (does nothing when it is open). The windows are listed now, the panel is made on the monitor under the
/// pointer and shown, and the pictures are registered once it is on the screen. False when it could not be opened (no monitor
/// under the pointer, the window could not be made); true when it is open, already or now.
pub fn open(req: OpenRequest, cx: &mut App) -> bool {
    if is_open(cx) {
        return true;
    }
    let started = Instant::now();
    let Some(mon) = rf_win::switcher::monitor_at_cursor() else {
        log::warn!("switcher: no monitor under the pointer");
        return false;
    };
    let listing = rf_win::switcher::list(ComRoute::ShellThread, Options::default());
    let listed_at = started.elapsed();
    let previous = rf_win::dock::foreground_root();
    let mut session = Session::open_from(listing.items, req.scope, req.backwards, listing.front_listed);
    let plan = layout::plan(session.items().len(), mon.work, mon.scale());
    session.fit(&plan);
    let dark = rf_win::accent::system_is_dark();

    let display = DisplayId::new(mon.handle as u64);
    let (w, h) = plan.window_logical();
    let bounds = Bounds::centered(Some(display), size(px(w), px(h)), cx);
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        display_id: Some(display),
        titlebar: None,
        focus: false,
        show: false,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        ..Default::default()
    };
    let hooked = req.hooked;
    let opened = cx.open_window(opts, move |window, cx| cx.new(|cx| SwitcherView::new(session, plan, previous, dark, hooked, window, cx)));
    let handle = match opened {
        Ok(h) => h,
        Err(e) => {
            log::error!("switcher: cannot make the window: {e:#}");
            return false;
        }
    };
    let created_at = started.elapsed();
    let Ok(hwnd) = handle.update(cx, |v, _, _| v.hwnd()) else { return false };
    cx.set_global(Active { handle, on_closed: req.on_closed });
    let counts = listing.counts;
    let took = listing.took;
    // (showing, ordering and focusing a window send it messages: from a task, with nothing borrowed, like the dock's panels)
    cx.spawn(async move |cx| {
        rf_win::window::set_dark_mode(hwnd, dark);
        rf_win::switcher::mark_shell_window(hwnd);
        geo::show_at(hwnd, plan.window);
        // (the quiet kind: no tap of the Alt key into the program that is in front)
        rf_win::window::bring_to_front_quietly(hwnd);
        let shown_at = started.elapsed();
        let (cards, live) = cx.update(|cx| handle.update(cx, |v, window, cx| {
            v.on_shown(window, cx);
            v.counts()
        })
        .unwrap_or((0, 0)));
        log::info!(
            "switcher: opened in {:?}: listing {:?} (facts {:?}, desktops {:?}, items {:?}; {} windows seen, {} listed, {} minimized, {} suspended, {} on other desktops), window made at {:?}, shown at {:?}, pictures and glass done at {:?}; {cards} cards, {live} live pictures",
            started.elapsed(),
            listed_at,
            took.facts,
            took.desktops,
            took.items,
            counts.windows_seen,
            counts.listed,
            counts.minimized,
            counts.suspended,
            counts.on_other_desktop,
            created_at,
            shown_at,
            started.elapsed(),
        );
    })
    .detach();
    true
}

/// An effect of the chord, from the shell host's keyboard feed (the keys never reach the switcher's window then: the hook swallowed
/// them). Does nothing when the switcher is closed.
pub fn apply_effect(effect: Effect, cx: &mut App) {
    let Some(active) = cx.try_global::<Active>().map(|a| a.handle) else { return };
    let _ = active.update(cx, |v, _, cx| v.apply(effect, cx));
}

/// Closes the switcher: a chosen window is brought to the front (while the switcher still holds the foreground), the pictures are
/// unregistered, the glass is let go and the window removed; then the request's `on_closed` is called. Does nothing when it is
/// closed.
pub fn close(how: Closing, cx: &mut App) {
    let Some((active, on_closed)) = cx.try_global::<Active>().map(|a| (a.handle, a.on_closed.clone())) else { return };
    // from here on the switcher counts as closed: a second close, a key or a click does nothing
    cx.remove_global::<Active>();
    cx.spawn(async move |cx| {
        let outcome = match how {
            Closing::Chosen(hwnd) => Some(activate::activate(&mut Win32Ops, hwnd)),
            _ => None,
        };
        let gone = cx.update(|cx| {
            active
                .update(cx, |v, window, cx| {
                    let ids = (v.hwnd(), v.previous());
                    v.teardown(cx);
                    window.remove_window();
                    ids
                })
                .ok()
        });
        if let (Some((hwnd, previous)), Closing::Cancelled) = (gone, how) {
            // Escape: the window that was in front is in front again (a click elsewhere or another program's window is left alone)
            let fg = rf_win::dock::foreground_root();
            if let Some(target) = geo::restore_to(previous, fg, hwnd, geo::exists(previous)) {
                geo::give_back(target);
            }
        }
        match outcome {
            Some(Outcome::Done) | None => {}
            Some(Outcome::Gone) => log::debug!("switcher: the chosen window is gone"),
            Some(Outcome::Flashed) => log::info!("switcher: the chosen window would not come forward; its button flashes"),
        }
        if let Some(f) = on_closed {
            cx.update(|cx| f(how, cx));
        }
    })
    .detach();
}
