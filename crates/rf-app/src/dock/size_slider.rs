//! The Dock page's size slider: the app's own slider control (`settings_view::slider_for`, the one the HyperGlass sliders use) over the
//! positions 0..=15 (%75 to %150 in steps of %5), the value as a percentage beside it and a small "Sıfırla" that goes back to %100.
//!
//! WHAT A STEP DOES. It sets the size in the settings in memory (so the page and everything else in this process shows it at once) and
//! lets `settings::update_live` write the file once the steps have settled, as every slider does. Besides, it sends the number itself to
//! the running dock on its pipe (`size <stamp> <permille> <0|1>`, see `rf_win::dock::size_live`), so the dock follows the slider while
//! it is dragged instead of jumping when the file lands. When the pointer is let go (or a key made its single step) a last message with
//! the release flag tells the dock to do what it held back: the reserved screen band and the icons' new resolution. A key press sends
//! its step WITHOUT the release: a held arrow key is a run of steps, and the dock's rest check (`size_live::IDLE`) settles it once.
//!
//! THE UI THREAD NEVER WAITS FOR THE PIPE. Messages go to an outbox that keeps only the newest step; one worker thread writes them to
//! the pipe. A step that finds the pipe busy is dropped (the next one carries the newer value anyway); a release is tried a few times.

use std::rc::Rc;
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gpui_kit::component::h_flex;
use gpui_kit::*;
use rf_win::dock::size_live::{self as live, Msg, Outbox};

use super::size_pref::DockSizePref;
use crate::settings::{self, Settings};
use crate::settings_view::{SliderSpec, button, hg_focus, slider_dragged, slider_for};
use crate::t;
use crate::theme::Palette;

/// The slider's id among the app's sliders (the HyperGlass ones are 0..=4).
pub(crate) const SLIDER_ID: usize = 5;

/// The control of the size row: slider, percentage, "Sıfırla".
pub(crate) fn control(p: &Palette, s: &Settings, cx: &mut App) -> AnyElement {
    let value = s.dock_size;
    let focus = hg_focus(cx, SLIDER_ID);
    let dragging = slider_dragged(cx, SLIDER_ID);
    let slider = slider_for(spec(), p, value.position(), focus, dragging);
    h_flex()
        .gap_3()
        .items_center()
        .child(slider)
        .child(div().w(px(44.)).flex_shrink_0().text_right().text_sm().text_color(p.text_secondary).child(value.percent_text(crate::i18n::is_tr())))
        .child(button("dock-size-reset", p, t!("Sıfırla"), |_, cx| reset(cx)))
        .into_any_element()
}

/// What the slider is bound to.
fn spec() -> SliderSpec {
    SliderSpec {
        id: SLIDER_ID,
        max: live::POSITIONS,
        // Shift and Page Up / Down move by %20
        big: 4,
        get: Rc::new(|cx| cx.global::<Settings>().dock_size.position()),
        apply: Rc::new(|position, cx| step(DockSizePref::at_position(position), cx)),
        release: Some(Rc::new(released)),
    }
}

/// One step of the slider: the size is set (the file follows once the steps settle) and the dock is told at once.
fn step(size: DockSizePref, cx: &mut App) {
    if cx.global::<Settings>().dock_size == size {
        return;
    }
    settings::update_live(cx, |s| s.dock_size = size);
    send(size.permille(), false, cx);
}

/// The slider was let go: whatever size it rests at is final.
fn released(cx: &mut App) {
    let size = cx.global::<Settings>().dock_size;
    send(size.permille(), true, cx);
}

/// "Sıfırla": back to %100 (and final at once). At %100 already there is nothing to go back to: a click then sends nothing, so
/// repeated clicks do not make the dock redo its band and its icons each time.
fn reset(cx: &mut App) {
    if !reset_does_anything(cx.global::<Settings>().dock_size) {
        return;
    }
    step(DockSizePref::default(), cx);
    released(cx);
}

fn reset_does_anything(current: DockSizePref) -> bool {
    current != DockSizePref::default()
}

// ---------------------------------------------------------------- the pipe

fn send(permille: u16, released: bool, cx: &App) {
    // (a dock that is off has nothing to follow; a test copy must leave the real dock alone, see `may_send_size`)
    if !cx.global::<Settings>().dock_enabled || !super::control::may_send_size() {
        return;
    }
    enqueue(Msg { stamp: next_stamp(), permille, released });
}

/// A stamp that is later than every stamp this process made before: the time in milliseconds, or one more than the last when the clock
/// has not moved (or went back).
fn next_stamp() -> u64 {
    static LAST: Mutex<u64> = Mutex::new(0);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    *last = now.max(*last + 1);
    *last
}

static OUTBOX: (Mutex<Outbox>, Condvar) = (Mutex::new(Outbox::new()), Condvar::new());
static WORKER: Once = Once::new();

fn enqueue(m: Msg) {
    WORKER.call_once(|| {
        let _ = std::thread::Builder::new().name("rf-dock-size".into()).spawn(worker);
    });
    OUTBOX.0.lock().unwrap_or_else(|p| p.into_inner()).push(m);
    OUTBOX.1.notify_one();
}

fn worker() {
    loop {
        let m = {
            let mut queue = OUTBOX.0.lock().unwrap_or_else(|p| p.into_inner());
            loop {
                if let Some(m) = queue.pop() {
                    break m;
                }
                queue = OUTBOX.1.wait(queue).unwrap_or_else(|p| p.into_inner());
            }
        };
        deliver(m);
    }
}

/// Writes one message to the dock's pipe. The server is between two pipe instances for a moment after every client, so a write can find
/// nothing there: a step is let go (the next one carries a newer value), a release is tried again.
fn deliver(m: Msg) {
    let line = live::message(m);
    let attempts = if m.released { 6 } else { 1 };
    for attempt in 0..attempts {
        if rf_win::single_instance::send_to(super::control::APP, &line) {
            return;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(Duration::from_millis(15));
        }
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: the glob of gpui_kit up there brings a `test` of its own that would shadow `#[test]`)
    use super::{DockSizePref, SLIDER_ID, next_stamp, reset_does_anything, spec};

    #[test]
    fn a_reset_at_the_default_sends_nothing() {
        assert!(!reset_does_anything(DockSizePref::default()));
        assert!(reset_does_anything(DockSizePref::from_permille(1050)));
        assert!(reset_does_anything(DockSizePref::at_position(0)));
    }

    #[test]
    fn stamps_only_grow() {
        let mut last = next_stamp();
        for _ in 0..10_000 {
            let now = next_stamp();
            assert!(now > last);
            last = now;
        }
    }

    #[test]
    fn the_slider_covers_the_whole_range_in_steps_of_five_percent() {
        let spec = spec();
        assert_eq!(spec.max, 15);
        assert_eq!(DockSizePref::at_position(0).permille(), 750);
        assert_eq!(DockSizePref::at_position(spec.max).permille(), 1500);
        assert_eq!(DockSizePref::at_position(5), DockSizePref::default());
        assert!(spec.release.is_some(), "the end of a drag is what the dock waits for");
        assert!(spec.big > 0 && spec.big <= spec.max);
        assert!(SLIDER_ID > 4, "the HyperGlass sliders are 0..=4");
    }
}
