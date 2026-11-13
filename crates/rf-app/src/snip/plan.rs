//! The decisions of the snip's interface that need no window, pure and tested: where the toolbar and the notice go, what is dimmed and
//! what is left clear, what the size label says, what the notice says and offers, how long it stays, and the edge of the Escape key.
//!
//! Lengths with `_px` or in a `Rect` are DEVICE pixels of the virtual screen; the ones named `logical` are the toolkit's.

use std::path::PathBuf;

use rf_core::desktop::geom::Rect;
use rf_win::snip::clip::Written;
use rf_win::snip::geom::Monitor;

/// The toolbar's size, logical px.
pub const TOOLBAR_W: f32 = 404.0;
pub const TOOLBAR_H: f32 = 52.0;
/// The gap between the top of the monitor and the toolbar, logical px.
pub const TOOLBAR_TOP: f32 = 20.0;

/// The notice's size, logical px.
pub const NOTICE_W: f32 = 344.0;
pub const NOTICE_H: f32 = 60.0;
/// The gap between the bottom of the work area and the notice, logical px (clear of the dock's islands, which sit at the bottom).
pub const NOTICE_BOTTOM: f32 = 96.0;
/// How long the notice stays when nobody points at it.
pub const NOTICE_MS: u64 = 6_000;

fn scale_of(m: &Monitor) -> f32 {
    if m.scale.is_finite() && m.scale > 0.0 { m.scale } else { 1.0 }
}

/// A rectangle of `logical` size centred on `across` (left to right) and `top` device px from `y`.
fn centred(across: &Rect, y: i32, w: f32, h: f32, scale: f32) -> Rect {
    let (dw, dh) = ((w * scale).round() as i32, (h * scale).round() as i32);
    let left = across.left + (across.width() - dw) / 2;
    Rect::new(left, y, left + dw, y + dh)
}

/// The toolbar: centred at the top of the monitor.
pub fn toolbar_rect(m: &Monitor) -> Rect {
    let s = scale_of(m);
    centred(&m.rect, m.rect.top + (TOOLBAR_TOP * s).round() as i32, TOOLBAR_W, TOOLBAR_H, s)
}

/// The notice: centred above the bottom of the monitor's work area. A work area that is missing or empty falls back to the monitor.
pub fn notice_rect(m: &Monitor, work: &Rect) -> Rect {
    let s = scale_of(m);
    let area = if work.is_empty() { m.rect } else { work.intersect(&m.rect) };
    let area = if area.is_empty() { m.rect } else { area };
    let h = (NOTICE_H * s).round() as i32;
    let y = (area.bottom - (NOTICE_BOTTOM * s).round() as i32 - h).max(area.top);
    centred(&area, y, NOTICE_W, NOTICE_H, s)
}

/// The monitor the toolbar goes to: the one under the pointer when the snip began, else the first (the primary).
pub fn toolbar_monitor(pointer: Option<(i32, i32)>, monitors: &[Monitor]) -> usize {
    pointer.and_then(|p| rf_win::snip::geom::nearest_monitor(p, monitors)).unwrap_or(0)
}

/// The monitor a finished snip's notice goes to: the one under the middle of the region.
pub fn notice_monitor(region: &Rect, monitors: &[Monitor]) -> usize {
    let mid = ((region.left + region.right) / 2, (region.top + region.bottom) / 2);
    rf_win::snip::geom::nearest_monitor(mid, monitors).unwrap_or(0)
}

/// A rectangle in a window's logical px: (x, y, width, height).
pub type LRect = (f32, f32, f32, f32);

/// The hole (clear part) cut to the window `(0, 0, w, h)`, or `None` when none of it is inside.
pub fn clip_to(hole: LRect, w: f32, h: f32) -> Option<LRect> {
    let (l, t) = (hole.0.max(0.0), hole.1.max(0.0));
    let (r, b) = ((hole.0 + hole.2).min(w), (hole.1 + hole.3).min(h));
    (r > l && b > t).then_some((l, t, r - l, b - t))
}

/// The dimmed parts of a window of `w` by `h` logical px with the clear `hole` in it: the whole window when there is no hole (or it is
/// outside), else the four strips around it (above, below, left and right of it), none of them empty. They never overlap, so the dim is
/// even and no pixel is darkened twice.
pub fn dim_strips(w: f32, h: f32, hole: Option<LRect>) -> Vec<LRect> {
    let Some((hx, hy, hw, hh)) = hole.and_then(|r| clip_to(r, w, h)) else {
        return vec![(0.0, 0.0, w, h)];
    };
    let mut v = Vec::with_capacity(4);
    if hy > 0.0 {
        v.push((0.0, 0.0, w, hy));
    }
    if hy + hh < h {
        v.push((0.0, hy + hh, w, h - (hy + hh)));
    }
    if hx > 0.0 {
        v.push((0.0, hy, hx, hh));
    }
    if hx + hw < w {
        v.push((hx + hw, hy, w - (hx + hw), hh));
    }
    v
}

/// What the size label says: "1280 × 720".
pub fn size_text(w: i32, h: i32) -> String {
    format!("{w} \u{d7} {h}")
}

/// The width of a label of `chars` characters, logical px (the label is sized from this: text is not measured before it is drawn).
pub fn label_width(chars: usize) -> f32 {
    chars as f32 * 7.6 + 22.0
}

/// The label's height, logical px.
pub const LABEL_H: f32 = 26.0;

// ---------------------------------------------------------------- the notice

/// What became of one snip.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// The picture's size in pixels; `None` when no picture could be made.
    pub size: Option<(u32, u32)>,
    pub copied: Result<Written, String>,
    /// The file the snip was saved to without asking (the setting), or why it was not.
    pub saved: Option<Result<PathBuf, String>>,
    /// The PNG is kept so that `Kaydet` writes the same bytes without making them again.
    pub has_png: bool,
}

/// The first line of the notice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Headline {
    /// "Panoya kopyalandı"
    Copied,
    /// "Panoya kopyalandı ve kaydedildi"
    CopiedAndSaved,
    /// "Kaydedildi" (the clipboard failed, the file did not)
    SavedOnly,
    /// "Panoya kopyalanamadı"
    NotCopied,
    /// "Alıntı alınamadı"
    Failed,
}

pub fn headline(o: &Outcome) -> Headline {
    let copied = o.copied.is_ok();
    let saved = matches!(o.saved, Some(Ok(_)));
    match (o.size.is_some(), copied, saved) {
        (false, ..) => Headline::Failed,
        (true, true, true) => Headline::CopiedAndSaved,
        (true, true, false) => Headline::Copied,
        (true, false, true) => Headline::SavedOnly,
        (true, false, false) => Headline::NotCopied,
    }
}

/// Whether the notice offers `Kaydet`: there is a picture to write and it has not been written (a snip saved without asking is not
/// offered again).
pub fn offers_save(o: &Outcome) -> bool {
    o.has_png && !matches!(o.saved, Some(Ok(_)))
}

/// Whether the notice goes away now. It stays while the pointer is on it and while a save is going on.
pub fn notice_expired(elapsed_ms: u64, hovered: bool, saving: bool) -> bool {
    !hovered && !saving && elapsed_ms >= NOTICE_MS
}

// ---------------------------------------------------------------- how long an overlay may stand, and when it must go

/// An overlay that is still up after this long is taken away: nothing else ends one that nobody is looking at (the user walked off,
/// the screen was locked behind it).
pub const SESSION_MAX_MS: u64 = 120_000;
/// How long the picture may take. The copy of a screen is tens of milliseconds; a call that has not returned after this is stuck.
pub const CAPTURE_MAX_MS: u64 = 10_000;
/// For this long after the overlay appears the foreground window is not yet judged: it is still settling (a key's release, a panel
/// that closes) and what it is at the end of it is the program the user is in.
pub const FOREGROUND_GRACE_MS: u64 = 400;

/// Why an overlay that nobody closed is closed anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    /// Another program came to the front (Alt+Tab, a call, a lock screen, a pop-up): the frozen picture would be stale and in the way.
    ForegroundChanged,
    /// A monitor was plugged, unplugged or moved: the pictures no longer fit the screen.
    DisplaysChanged,
    /// [`SESSION_MAX_MS`].
    TimedOut,
}

/// What the running overlay is watched against. Pure: the caller reads the system and feeds the values in.
#[derive(Clone, Copy, Debug)]
pub struct SessionWatch {
    /// The window that was in front when the overlay appeared.
    foreground: isize,
    /// The screen's shape then (`rf_win::snip::place::screen_signature`).
    screen: [i32; 5],
}

impl SessionWatch {
    pub fn new(foreground: isize, screen: [i32; 5]) -> SessionWatch {
        SessionWatch { foreground, screen }
    }

    /// `foreground_now`: 0 when there is none at this moment (it happens while the foreground changes hands, and is not a change);
    /// `ours`: that window belongs to this process (the overlay's own windows never take the foreground, but if the toolkit gives one
    /// to one of them it is not another program coming in front).
    pub fn check(&mut self, elapsed_ms: u64, foreground_now: isize, ours: bool, screen_now: [i32; 5]) -> Option<EndReason> {
        if screen_now != self.screen {
            return Some(EndReason::DisplaysChanged);
        }
        if elapsed_ms >= SESSION_MAX_MS {
            return Some(EndReason::TimedOut);
        }
        if foreground_now == 0 || ours {
            return None;
        }
        if elapsed_ms < FOREGROUND_GRACE_MS {
            self.foreground = foreground_now;
            return None;
        }
        (foreground_now != self.foreground).then_some(EndReason::ForegroundChanged)
    }
}

/// Whether a `--snip` process has nothing left to do and ends: nothing is being captured, nothing is being copied or saved, no overlay is
/// up and no notice is showing. (A save the user started with `Kaydet` counts as working: closing the notice does not end the process
/// under the write.)
pub fn nothing_left(capturing: bool, working: usize, session: bool, notice: bool) -> bool {
    !capturing && working == 0 && !session && !notice
}

// ---------------------------------------------------------------- Escape

/// Reads Escape's state at a steady rate and reports each PRESS once. An Escape that is already down when the overlay appears (the key
/// that was used to cancel the last one, still held) does not count: it has to be seen up first.
#[derive(Clone, Copy, Debug, Default)]
pub struct EscWatch {
    seen_up: bool,
    was_down: bool,
}

impl EscWatch {
    pub fn feed(&mut self, down: bool) -> bool {
        let pressed = down && self.seen_up && !self.was_down;
        if !down {
            self.seen_up = true;
        }
        self.was_down = down;
        pressed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(l: i32, t: i32, r: i32, b: i32, scale: f32) -> Monitor {
        Monitor { rect: Rect::new(l, t, r, b), scale }
    }

    #[test]
    fn the_toolbar_is_centred_at_the_top_in_device_pixels() {
        let r = toolbar_rect(&mon(0, 0, 1920, 1080, 1.0));
        assert_eq!((r.width(), r.height()), (404, 52));
        assert_eq!(r.left, (1920 - 404) / 2);
        assert_eq!(r.top, 20);
        // 150 %: everything is 1.5 times larger in device pixels, and the gap too
        let r = toolbar_rect(&mon(-2560, -400, 0, 1040, 1.5));
        assert_eq!((r.width(), r.height()), (606, 78));
        assert_eq!(r.top, -400 + 30);
        assert_eq!(r.left, -2560 + (2560 - 606) / 2);
        assert!(r.right <= 0 && r.left >= -2560, "inside its own monitor, which has a negative origin");
    }

    #[test]
    fn the_notice_sits_above_the_bottom_of_the_work_area_and_inside_the_monitor() {
        let m = mon(0, 0, 1920, 1080, 1.0);
        let work = Rect::new(0, 0, 1920, 1040);
        let r = notice_rect(&m, &work);
        assert_eq!((r.width(), r.height()), (344, 60));
        assert_eq!(r.bottom, 1040 - 96);
        assert_eq!(r.left, (1920 - 344) / 2);
        // no work area known: the monitor stands in for it
        assert_eq!(notice_rect(&m, &Rect::default()).bottom, 1080 - 96);
        // a work area that is not on this monitor at all is not believed
        assert_eq!(notice_rect(&m, &Rect::new(5000, 0, 6000, 500)).bottom, 1080 - 96);
        // a very short monitor keeps the notice on it
        let tiny = mon(0, 0, 800, 120, 1.0);
        assert!(notice_rect(&tiny, &tiny.rect).top >= 0);
    }

    #[test]
    fn a_nonsense_scale_is_one() {
        let r = toolbar_rect(&mon(0, 0, 1000, 500, 0.0));
        assert_eq!(r.width(), 404);
    }

    #[test]
    fn the_toolbar_goes_to_the_monitor_under_the_pointer_and_the_notice_to_the_one_with_the_region() {
        let ms = [mon(0, 0, 1920, 1080, 1.0), mon(-1440, -560, 0, 2000, 1.5)];
        assert_eq!(toolbar_monitor(Some((100, 100)), &ms), 0);
        assert_eq!(toolbar_monitor(Some((-100, 100)), &ms), 1);
        assert_eq!(toolbar_monitor(None, &ms), 0);
        assert_eq!(toolbar_monitor(Some((500, 1500)), &ms), 0, "a gap between monitors is the nearest one's");
        assert_eq!(notice_monitor(&Rect::new(-1000, 0, -800, 100), &ms), 1);
        assert_eq!(notice_monitor(&Rect::new(10, 10, 110, 60), &ms), 0);
        // a region across both: the middle decides
        assert_eq!(notice_monitor(&Rect::new(-90, -110, 400, 200), &ms), 0);
    }

    #[test]
    fn with_no_hole_everything_is_dimmed() {
        assert_eq!(dim_strips(100.0, 50.0, None), vec![(0.0, 0.0, 100.0, 50.0)]);
        // a hole wholly outside the window (it is on another monitor) is no hole here
        assert_eq!(dim_strips(100.0, 50.0, Some((200.0, 10.0, 30.0, 30.0))), vec![(0.0, 0.0, 100.0, 50.0)]);
    }

    /// The area covered by the strips plus the hole equals the window, and no two strips overlap.
    fn assert_partition(w: f32, h: f32, hole: LRect) {
        let strips = dim_strips(w, h, Some(hole));
        let clipped = clip_to(hole, w, h);
        let area = |r: &LRect| r.2 * r.3;
        let total: f32 = strips.iter().map(area).sum::<f32>() + clipped.map_or(0.0, |c| area(&c));
        assert!((total - w * h).abs() < 0.01, "{strips:?} + {clipped:?} do not make {w}x{h}");
        for (i, a) in strips.iter().enumerate() {
            assert!(a.2 > 0.0 && a.3 > 0.0, "empty strip {a:?}");
            for b in &strips[i + 1..] {
                let (ox, oy) = ((a.0 + a.2).min(b.0 + b.2) - a.0.max(b.0), (a.1 + a.3).min(b.1 + b.3) - a.1.max(b.1));
                assert!(ox <= 0.0 || oy <= 0.0, "{a:?} overlaps {b:?}");
            }
        }
    }

    #[test]
    fn the_four_strips_and_the_hole_make_the_window_exactly() {
        assert_partition(200.0, 100.0, (50.0, 20.0, 80.0, 40.0));
        // touching an edge or two
        assert_partition(200.0, 100.0, (0.0, 0.0, 80.0, 40.0));
        assert_partition(200.0, 100.0, (120.0, 60.0, 80.0, 40.0));
        // the whole window
        assert_partition(200.0, 100.0, (0.0, 0.0, 200.0, 100.0));
        assert!(dim_strips(200.0, 100.0, Some((0.0, 0.0, 200.0, 100.0))).is_empty(), "a clear window needs no dim");
        // a hole that runs off the window on the left and the top (the drag came from another monitor)
        assert_partition(200.0, 100.0, (-40.0, -10.0, 100.0, 50.0));
        assert_eq!(clip_to((-40.0, -10.0, 100.0, 50.0), 200.0, 100.0), Some((0.0, 0.0, 60.0, 40.0)));
        // and beyond the right and bottom
        assert_partition(200.0, 100.0, (150.0, 80.0, 500.0, 500.0));
    }

    #[test]
    fn the_size_label_reads_width_times_height() {
        assert_eq!(size_text(1280, 720), "1280 \u{d7} 720");
        assert!(label_width(size_text(1280, 720).chars().count()) > label_width(size_text(9, 9).chars().count()));
    }

    fn outcome(size: bool, copied: bool, saved: Option<bool>, png: bool) -> Outcome {
        Outcome {
            size: size.then_some((10, 10)),
            copied: if copied { Ok(Written::All) } else { Err("busy".into()) },
            saved: saved.map(|ok| if ok { Ok(PathBuf::from("x.png")) } else { Err("disk full".into()) }),
            has_png: png,
        }
    }

    #[test]
    fn the_notice_says_what_became_of_the_snip() {
        assert_eq!(headline(&outcome(true, true, None, true)), Headline::Copied);
        assert_eq!(headline(&outcome(true, true, Some(true), true)), Headline::CopiedAndSaved);
        assert_eq!(headline(&outcome(true, true, Some(false), true)), Headline::Copied, "a file that failed to write does not make the copy worse");
        assert_eq!(headline(&outcome(true, false, Some(true), true)), Headline::SavedOnly);
        assert_eq!(headline(&outcome(true, false, None, true)), Headline::NotCopied);
        assert_eq!(headline(&outcome(false, false, None, false)), Headline::Failed);
    }

    #[test]
    fn kaydet_is_offered_only_while_there_is_a_picture_not_yet_written() {
        assert!(offers_save(&outcome(true, true, None, true)));
        assert!(offers_save(&outcome(true, false, None, true)), "the clipboard failed: the file is still a way to keep it");
        assert!(offers_save(&outcome(true, true, Some(false), true)), "an automatic save that failed can be tried by hand");
        assert!(!offers_save(&outcome(true, true, Some(true), true)), "already saved");
        assert!(!offers_save(&outcome(false, false, None, false)), "no picture");
    }

    #[test]
    fn the_notice_stays_while_pointed_at_or_saving_and_goes_after_its_time() {
        assert!(!notice_expired(NOTICE_MS - 1, false, false));
        assert!(notice_expired(NOTICE_MS, false, false));
        assert!(!notice_expired(NOTICE_MS * 10, true, false));
        assert!(!notice_expired(NOTICE_MS * 10, false, true));
    }

    #[test]
    fn an_overlay_nobody_closed_goes_when_another_program_comes_to_the_front_or_the_screen_changes_or_it_is_too_old() {
        let screen = [0, 0, 2560, 1440, 1];
        let mut w = SessionWatch::new(0x1000, screen);
        // the same program in front, the same screen, a minute in: left alone
        assert_eq!(w.check(60_000, 0x1000, false, screen), None);
        // another program in front (Alt+Tab, a pop-up, a lock screen)
        assert_eq!(w.check(60_000, 0x2000, false, screen), Some(EndReason::ForegroundChanged));
        // no foreground window for a moment while it changes hands is not a change
        assert_eq!(w.check(60_000, 0, false, screen), None);
        // a window of our own in front is not another program
        assert_eq!(w.check(60_000, 0x3000, true, screen), None);
        // a monitor plugged or unplugged
        assert_eq!(w.check(1_000, 0x1000, false, [0, 0, 4480, 1440, 2]), Some(EndReason::DisplaysChanged));
        // too old
        assert_eq!(w.check(SESSION_MAX_MS - 1, 0x1000, false, screen), None);
        assert_eq!(w.check(SESSION_MAX_MS, 0x1000, false, screen), Some(EndReason::TimedOut));
    }

    #[test]
    fn the_foreground_is_not_judged_while_it_is_still_settling() {
        let screen = [0, 0, 1920, 1080, 1];
        // the program in front when the overlay appeared was not yet the one the user is in (a panel was closing)
        let mut w = SessionWatch::new(0x10, screen);
        assert_eq!(w.check(50, 0x20, false, screen), None, "inside the grace time the newer window is taken as the baseline");
        assert_eq!(w.check(FOREGROUND_GRACE_MS - 1, 0x20, false, screen), None);
        assert_eq!(w.check(FOREGROUND_GRACE_MS, 0x20, false, screen), None, "and it is the one to stay with");
        assert_eq!(w.check(FOREGROUND_GRACE_MS + 10, 0x10, false, screen), Some(EndReason::ForegroundChanged));
        // a screen change counts even inside the grace time
        let mut w = SessionWatch::new(0x10, screen);
        assert_eq!(w.check(10, 0x10, false, [0, 0, 1280, 720, 1]), Some(EndReason::DisplaysChanged));
    }

    #[test]
    fn a_snip_process_does_not_end_under_work_that_is_still_going() {
        assert!(nothing_left(false, 0, false, false));
        assert!(!nothing_left(true, 0, false, false), "the picture is being taken");
        assert!(!nothing_left(false, 1, false, false), "a copy or a Kaydet is writing: closing the notice must not end the process under it");
        assert!(!nothing_left(false, 0, true, false), "the overlay is up");
        assert!(!nothing_left(false, 0, false, true), "the notice is up");
    }

    #[test]
    fn escape_counts_once_per_press_and_never_for_a_key_already_down() {
        let mut e = EscWatch::default();
        // held from before the overlay appeared
        assert!(!e.feed(true));
        assert!(!e.feed(true));
        assert!(!e.feed(false));
        assert!(e.feed(true), "a fresh press");
        assert!(!e.feed(true), "still down: not another press");
        assert!(!e.feed(false));
        assert!(e.feed(true));
        // never down: never reported
        let mut idle = EscWatch::default();
        assert!(!idle.feed(false) && !idle.feed(false));
    }
}
