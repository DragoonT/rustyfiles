//! The screen band the dock keeps (the application desktop toolbar), as a state machine over a trait of the AppBar calls.
//!
//! WHY IT IS HERE. The band lives in EXPLORER: `ABM_NEW`, `ABM_QUERYPOS` and `ABM_SETPOS` are messages to Explorer's taskbar window,
//! and the work area that maximised windows stop at is computed there. Two things go wrong when Explorer goes away and comes back
//! (seen on the user's PC, 2026-11-12: the work area was 2560x1314 with the taskbar and our band, 2560x1392 after the restart, and
//! maximised windows went under the dock):
//!
//! 1. The new Explorer has forgotten every toolbar. It announces itself with the broadcast message `TaskbarCreated`; [`Band::announced`]
//!    is what the window procedure does with it, and the next [`Band::apply`] registers again even though the rectangle is the same.
//! 2. The new taskbar sets the work area again AFTER our registration went through, from its own rectangle alone. The band we were
//!    granted is the same rectangle as before, so the old code saw "nothing changed" and sent no `ABM_SETPOS`, and the work area stayed
//!    without our band for good. [`Band::apply`] therefore also looks at the work area it is given: when it does not leave the band
//!    free, `ABM_SETPOS` is sent again, at most [`MAX_REASSERT`] times in a row (something that keeps taking the band away on purpose,
//!    such as a taskbar that is being hidden, must not be fought for ever; a new announcement or a changed rectangle starts over).
//!
//! Registering can also fail right after the announcement (the taskbar window exists, its toolbar list may not yet): [`Applied::RegisterFailed`]
//! lets the caller try again a little later, [`MAX_REGISTER_TRIES`] times at most.
//!
//! Everything here is pure; the real calls are in `system.rs` behind [`AppBarCalls`], and the tests use a fake that models Explorer.

use super::layout::Rect;

/// How many times in a row `ABM_SETPOS` is sent again for a band that was granted unchanged but is not in the work area.
pub const MAX_REASSERT: u8 = 3;
/// How many times a failed registration is tried again after one announcement.
pub const MAX_REGISTER_TRIES: u8 = 5;

/// The four toolbar calls. The real implementation sends them to Explorer's taskbar window (they block until it answers).
pub trait AppBarCalls {
    /// `ABM_NEW`. False: Explorer did not take the registration.
    fn register(&mut self) -> bool;
    /// `ABM_QUERYPOS` for `edge` (the whole bottom edge of the monitor): the rectangle after the system took away what other toolbars hold.
    fn query(&mut self, edge: Rect) -> Rect;
    /// `ABM_SETPOS`.
    fn set_pos(&mut self, rc: Rect);
    /// `ABM_REMOVE`.
    fn remove(&mut self);
}

/// What one [`Band::apply`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Nothing to send (or nothing could be asked: no monitor).
    Unchanged,
    /// The band was set to this rectangle.
    Set(Rect),
    /// The band was given back.
    Released,
    /// Explorer refused the registration: try again later.
    RegisterFailed,
}

/// Whether the work area leaves the band free: the band sits at the bottom edge, so the work area must end where it starts.
pub fn in_effect(band: Rect, work: Rect) -> bool {
    work.bottom <= band.top
}

#[derive(Debug, Default)]
pub struct Band {
    registered: bool,
    granted: Option<Rect>,
    reasserts: u8,
    /// An announcement arrived and no registration has gone through since. The announcement does not say WHO sent it: Explorer
    /// after a restart (it holds nothing of ours), or any program - our own tray host broadcasts `TaskbarCreated` when it starts
    /// and stops - while Explorer still holds our registration. Both must end up with exactly one registration.
    announced: bool,
}

impl Band {
    pub fn new() -> Band {
        Band::default()
    }

    /// Whether Explorer holds our registration as far as we know.
    pub fn registered(&self) -> bool {
        self.registered
    }

    /// The band set last.
    pub fn granted(&self) -> Option<Rect> {
        self.granted
    }

    /// `TaskbarCreated` arrived: the registration and the grant are gone with the old taskbar. Forget them, so that the next
    /// [`Band::apply`] registers and sets again instead of finding "nothing changed".
    pub fn announced(&mut self) {
        self.registered = false;
        self.granted = None;
        self.reasserts = 0;
        self.announced = true;
    }

    /// Brings the band in line with `want` (its thickness in device px; `None`: no band). `view`: the primary monitor's rectangle and
    /// its work area now (`None`: they could not be read; nothing is asked then, except giving the band back).
    pub fn apply(&mut self, api: &mut dyn AppBarCalls, want: Option<i32>, view: Option<(Rect, Rect)>) -> Applied {
        let Some(thickness) = want else {
            if self.registered {
                api.remove();
                self.registered = false;
            }
            self.reasserts = 0;
            return if self.granted.take().is_some() { Applied::Released } else { Applied::Unchanged };
        };
        let Some((monitor, work)) = view else { return Applied::Unchanged };
        if !self.registered {
            if !api.register() {
                // After an announcement a refusal most likely means "this window is registered already": the broadcast was not from a
                // new Explorer (see `announced`). Give the old registration back and take a new one, so that the band is neither lost nor
                // doubled. Without an announcement nothing is held that is ours, and a refusal is a plain refusal.
                if !self.announced {
                    return Applied::RegisterFailed;
                }
                api.remove();
                if !api.register() {
                    return Applied::RegisterFailed;
                }
            }
            self.registered = true;
            self.announced = false;
            self.reasserts = 0;
        }
        // the whole bottom edge is asked for; the system takes away what the Windows taskbar (and any toolbar that came first) holds,
        // then the band is cut to its thickness at the bottom of what is left
        let mut rc = api.query(monitor);
        rc.top = rc.bottom - thickness;
        if self.granted != Some(rc) {
            self.reasserts = 0;
        } else if in_effect(rc, work) {
            self.reasserts = 0;
            return Applied::Unchanged;
        } else if self.reasserts >= MAX_REASSERT {
            return Applied::Unchanged;
        } else {
            self.reasserts += 1;
        }
        api.set_pos(rc);
        self.granted = Some(rc);
        Applied::Set(rc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MON: Rect = Rect::new(0, 0, 2560, 1440);
    const TASKBAR: i32 = 48;
    const BAND: i32 = 78;

    /// Explorer, as far as the band is concerned: it holds a registration and the work area.
    struct Fake {
        explorer_has_us: bool,
        accept_new: bool,
        work: Rect,
        calls: Vec<&'static str>,
        /// Whether `ABM_SETPOS` takes effect on the work area (false: something takes the band away again at once).
        honours: bool,
    }

    impl Fake {
        fn new() -> Fake {
            Fake { explorer_has_us: false, accept_new: true, work: Rect::new(0, 0, 2560, 1440 - TASKBAR), calls: Vec::new(), honours: true }
        }
        /// Explorer restarted: it forgot our registration and set the work area from its taskbar alone.
        fn restart(&mut self) {
            self.explorer_has_us = false;
            self.work = Rect::new(0, 0, 2560, 1440 - TASKBAR);
        }
        fn count(&self, what: &str) -> usize {
            self.calls.iter().filter(|c| **c == what).count()
        }
    }

    impl AppBarCalls for Fake {
        fn register(&mut self) -> bool {
            self.calls.push("new");
            if self.explorer_has_us {
                return false; // (a window can be registered once)
            }
            if self.accept_new {
                self.explorer_has_us = true;
            }
            self.accept_new
        }
        fn query(&mut self, edge: Rect) -> Rect {
            self.calls.push("query");
            assert!(self.explorer_has_us, "a query without a registration");
            Rect { bottom: edge.bottom - TASKBAR, ..edge }
        }
        fn set_pos(&mut self, rc: Rect) {
            self.calls.push("setpos");
            assert!(self.explorer_has_us, "a setpos without a registration");
            if self.honours {
                self.work.bottom = rc.top;
            }
        }
        fn remove(&mut self) {
            self.calls.push("remove");
            self.explorer_has_us = false;
            self.work = Rect::new(0, 0, 2560, 1440 - TASKBAR);
        }
    }

    /// One apply with the work area as it is now.
    fn go(b: &mut Band, f: &mut Fake, want: Option<i32>) -> Applied {
        let view = Some((MON, f.work));
        b.apply(f, want, view)
    }

    #[test]
    fn the_first_apply_registers_asks_and_sets() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        let r = go(&mut b, &mut f, Some(BAND));
        assert_eq!(r, Applied::Set(Rect::new(0, 1440 - TASKBAR - BAND, 2560, 1440 - TASKBAR)));
        assert_eq!(f.calls, ["new", "query", "setpos"]);
        assert_eq!(f.work.bottom, 1440 - TASKBAR - BAND, "the work area is 2560x1314 as in the trial's before-picture");
        assert!(b.registered());
    }

    #[test]
    fn a_second_apply_with_the_band_in_effect_sends_nothing() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        go(&mut b, &mut f, Some(BAND));
        f.calls.clear();
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::Unchanged);
        assert_eq!(f.calls, ["query"], "one question, no new SETPOS (it would only cause another notification)");
    }

    #[test]
    fn a_taskbar_announcement_makes_it_register_and_set_again_once() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        go(&mut b, &mut f, Some(BAND));
        // the trial: Explorer went away and came back
        f.restart();
        f.calls.clear();
        b.announced();
        assert_eq!(b.granted(), None);
        let r = go(&mut b, &mut f, Some(BAND));
        assert!(matches!(r, Applied::Set(_)));
        assert_eq!(f.calls, ["new", "query", "setpos"], "one registration and one SETPOS per announcement");
        assert_eq!(f.work.bottom, 1440 - TASKBAR - BAND, "back to 2560x1314");
    }

    #[test]
    fn an_announcement_that_is_not_an_explorer_restart_keeps_exactly_one_registration() {
        // our own tray host (or any program) broadcasts TaskbarCreated while Explorer still holds our registration: ABM_NEW is refused
        // for a window that is registered, so the old registration is given back first and taken anew
        let (mut b, mut f) = (Band::new(), Fake::new());
        go(&mut b, &mut f, Some(BAND));
        f.calls.clear();
        b.announced();
        let r = go(&mut b, &mut f, Some(BAND));
        assert!(matches!(r, Applied::Set(_)), "{r:?}: the band is not left forgotten");
        assert_eq!(f.calls, ["new", "remove", "new", "query", "setpos"]);
        assert!(f.explorer_has_us && b.registered());
        assert_eq!(f.work.bottom, 1440 - TASKBAR - BAND, "the work area keeps the band");
        // and it was one announcement: the next apply is quiet
        f.calls.clear();
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::Unchanged);
        assert_eq!(f.calls, ["query"]);
    }

    #[test]
    fn a_refusal_without_an_announcement_does_not_remove_anything() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        f.accept_new = false;
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::RegisterFailed);
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::RegisterFailed);
        assert_eq!(f.calls, ["new", "new"], "no ABM_REMOVE for a registration that was never ours");
    }

    #[test]
    fn a_work_area_that_lost_the_band_after_our_registration_is_set_again() {
        // the cause of "never asked again": the new taskbar set the work area after we had registered; the grant is the same rectangle
        let (mut b, mut f) = (Band::new(), Fake::new());
        go(&mut b, &mut f, Some(BAND));
        f.work = Rect::new(0, 0, 2560, 1440 - TASKBAR); // 2560x1392: only the taskbar
        f.calls.clear();
        let r = go(&mut b, &mut f, Some(BAND));
        assert!(matches!(r, Applied::Set(_)));
        assert_eq!(f.calls, ["query", "setpos"], "no new registration (we still have one), but the position is sent again");
        assert_eq!(f.work.bottom, 1440 - TASKBAR - BAND);
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::Unchanged, "and then it is left alone");
    }

    #[test]
    fn something_that_keeps_taking_the_band_away_is_not_fought_for_ever() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        f.honours = false;
        assert!(matches!(go(&mut b, &mut f, Some(BAND)), Applied::Set(_)));
        for _ in 0..MAX_REASSERT {
            assert!(matches!(go(&mut b, &mut f, Some(BAND)), Applied::Set(_)));
        }
        let before = f.count("setpos");
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::Unchanged);
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::Unchanged);
        assert_eq!(f.count("setpos"), before, "no more after the limit");
        // a new announcement starts over
        b.announced();
        f.restart();
        f.honours = true;
        assert!(matches!(go(&mut b, &mut f, Some(BAND)), Applied::Set(_)));
        assert_eq!(f.work.bottom, 1440 - TASKBAR - BAND);
    }

    #[test]
    fn a_refused_registration_is_reported_and_tried_again_on_the_next_apply() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        f.accept_new = false;
        assert_eq!(go(&mut b, &mut f, Some(BAND)), Applied::RegisterFailed);
        assert_eq!(f.calls, ["new"], "nothing is asked of a toolbar that is not registered");
        assert!(!b.registered());
        f.accept_new = true;
        assert!(matches!(go(&mut b, &mut f, Some(BAND)), Applied::Set(_)));
    }

    #[test]
    fn a_changed_thickness_or_edge_is_set_without_a_new_registration() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        go(&mut b, &mut f, Some(BAND));
        f.calls.clear();
        assert!(matches!(go(&mut b, &mut f, Some(BAND + 10)), Applied::Set(r) if r.height() == BAND + 10));
        assert_eq!(f.calls, ["query", "setpos"]);
    }

    #[test]
    fn giving_the_band_back_removes_the_registration_once() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        assert_eq!(b.apply(&mut f, None, None), Applied::Unchanged, "nothing to give back");
        assert!(f.calls.is_empty());
        go(&mut b, &mut f, Some(BAND));
        f.calls.clear();
        assert_eq!(b.apply(&mut f, None, None), Applied::Released);
        assert_eq!(f.calls, ["remove"]);
        assert_eq!(b.apply(&mut f, None, None), Applied::Unchanged);
        assert_eq!(f.calls, ["remove"]);
        assert_eq!(f.work.bottom, 1440 - TASKBAR);
    }

    #[test]
    fn without_a_readable_monitor_nothing_is_asked() {
        let (mut b, mut f) = (Band::new(), Fake::new());
        assert_eq!(b.apply(&mut f, Some(BAND), None), Applied::Unchanged);
        assert!(f.calls.is_empty());
    }

    #[test]
    fn the_band_counts_as_in_effect_only_when_the_work_area_ends_above_it() {
        let band = Rect::new(0, 1300, 2560, 1392);
        assert!(in_effect(band, Rect::new(0, 0, 2560, 1300)));
        assert!(in_effect(band, Rect::new(0, 0, 2560, 1200)));
        assert!(!in_effect(band, Rect::new(0, 0, 2560, 1392)));
    }
}
