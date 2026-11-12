//! The click engine: a left / right / middle click, a double click, a hover or a key on one of OUR pictures of a tray icon, turned into
//! the messages that icon's owner expects for its version (the table is `tray_wire::click`), with the foreground handed to the owner
//! first as Explorer does. Delivery is by `SendNotifyMessage` (queued when the owner is on another thread, so a hung program cannot
//! freeze the caller) and happens on the CALLER's thread (the interface's), which is the one that just received the click and so the
//! one that may hand the foreground on.
//!
//! An icon whose owner window is gone, or whose window now belongs to another process (the handle was reused), is dropped from the
//! model and NEVER sent to.

use super::model::IconId;
use super::shared::{Shared, bump};
use super::traits::OwnerSend;
use crate::tray_wire::Point;
use crate::tray_wire::click::{ClickOptions, HostEvent, IconRef, anchor_for, callbacks, grants_foreground};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClickRequest {
    pub id: IconId,
    pub event: HostEvent,
    /// The pointer, screen coordinates.
    pub cursor: Point,
    /// The icon's upper-left corner on our strip, screen coordinates (where keyboard events are anchored).
    pub icon_top_left: Point,
    pub opts: ClickOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickOutcome {
    /// This many messages were sent.
    Sent(usize),
    /// No such icon (it was deleted meanwhile).
    UnknownIcon,
    /// The icon never gave a callback message: there is nothing to send.
    NoCallback,
    /// The owner is gone: the icon was dropped and nothing was sent.
    OwnerGone,
    /// The icon is there, but its owner window or process is not the one the caller expected (see [`deliver_expecting`]): it was
    /// taken over by another program since the caller drew it. Nothing was sent, the icon was left as it is.
    OwnerChanged,
    /// A message could not be sent; `sent` went out before it.
    Failed { sent: usize },
}

/// Whether the window `hwnd` is still the window of the program that added the icon: it exists and it belongs to that process.
/// An icon whose process was never known (`pid` 0: its window was not a window when the icon was added) is NOT alive: nothing can
/// tell whether a window that has that handle value now is the one the program meant, and a click sends it the callback messages
/// and hands its process the foreground.
pub fn owner_alive(owners: &dyn OwnerSend, hwnd: isize, pid: u32) -> bool {
    pid != 0 && owners.is_window(hwnd) && owners.owner_pid(hwnd) == Some(pid)
}

/// An icon's owner as a caller saw it: `(window handle, process id)`.
pub type OwnerRef = (isize, u32);

pub fn deliver(shared: &Shared, owners: &dyn OwnerSend, req: &ClickRequest) -> ClickOutcome {
    deliver_expecting(shared, owners, req, None)
}

/// [`deliver`] for a caller that drew the icon at some time and clicked on that picture later. A program that adds an icon again
/// under a key the model already holds (the same GUID from another process, the same window handle and id) takes the model's icon
/// over: same [`IconId`], new owner. `expect` is the owner the caller drew; when the model's icon has another one now, nothing is
/// sent and no foreground is granted (`OwnerChanged`), so a click meant for the first program never reaches the second.
pub fn deliver_expecting(shared: &Shared, owners: &dyn OwnerSend, req: &ClickRequest, expect: Option<OwnerRef>) -> ClickOutcome {
    // a copy of what is needed, under the lock; nothing is sent while it is held
    let found = {
        let st = shared.state.lock();
        st.model.get(req.id).map(|i| (i.hwnd_handle(), i.uid, i.callback, i.version, i.owner_pid))
    };
    let Some((hwnd, uid, callback, version, pid)) = found else { return ClickOutcome::UnknownIcon };
    if expect.is_some_and(|e| e != (hwnd, pid)) {
        return ClickOutcome::OwnerChanged;
    }
    let Some(callback) = callback else { return ClickOutcome::NoCallback };
    if !owner_alive(owners, hwnd, pid) {
        shared.drop_icon(req.id);
        bump(&shared.counters.clicks_dropped);
        return ClickOutcome::OwnerGone;
    }
    if grants_foreground(req.event) {
        // the process that now owns the window (it is the one that was checked above; a program with several processes lets the
        // window's own process take the foreground)
        owners.allow_foreground(owners.owner_pid(hwnd).unwrap_or(pid));
    }
    let anchor = anchor_for(req.event, req.cursor, req.icon_top_left);
    let mut sent = 0;
    for cb in callbacks(IconRef { uid, version }, req.event, anchor, req.opts) {
        if !owners.send_notify(hwnd, callback, cb.wparam, cb.lparam) {
            return ClickOutcome::Failed { sent };
        }
        sent += 1;
        bump(&shared.counters.clicks_sent);
    }
    ClickOutcome::Sent(sent)
}

/// Drops every icon whose owner is gone. Returns how many.
pub fn sweep(shared: &Shared, owners: &dyn OwnerSend) -> usize {
    let list = shared.state.lock().model.owners();
    // (the questions are asked with no lock held)
    let dead: Vec<IconId> = list.into_iter().filter(|(_, hwnd, pid)| !owner_alive(owners, *hwnd, *pid)).map(|(id, _, _)| id).collect();
    dead.into_iter().filter(|id| shared.drop_icon(*id)).count()
}

#[cfg(test)]
mod tests {
    use super::super::fakes::FakeOwners;
    use super::super::model::Prepared;
    use super::super::model::test_util::*;
    use super::*;
    use crate::tray_wire::IconVersion;
    use crate::tray_wire::click::ev;
    use crate::tray_wire::notify::{Nim, nif};

    fn shared_with(version: IconVersion, hwnd: u32, uid: u32, pid: u32) -> (Shared, IconId) {
        let (s, _rx) = Shared::new();
        s.apply(&req(Nim::Add, data(hwnd, uid, nif::MESSAGE)), Prepared { owner: Some((pid, "a.exe".into())), ..Default::default() });
        if version != IconVersion::V0 {
            let mut d = data(hwnd, uid, 0);
            d.timeout_or_version = version.raw();
            s.apply(&req(Nim::SetVersion, d), Prepared::default());
        }
        let id = s.snapshot().icons[0].id;
        (s, id)
    }

    fn click(id: IconId, event: HostEvent) -> ClickRequest {
        ClickRequest { id, event, cursor: Point { x: 500, y: 600 }, icon_top_left: Point { x: 490, y: 590 }, opts: ClickOptions::default() }
    }

    #[test]
    fn a_left_click_on_a_version_3_icon_is_down_up_select_with_the_foreground_first() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::Sent(3));
        let log = o.log();
        assert_eq!(log[0], "allow_foreground 50", "the foreground is handed over before the first message");
        assert_eq!(
            &log[1..],
            &[
                format!("send {:#x} {:#x} {:#x} {:#x}", 0x1234, 0x8005, 7, ev::WM_LBUTTONDOWN),
                format!("send {:#x} {:#x} {:#x} {:#x}", 0x1234, 0x8005, 7, ev::WM_LBUTTONUP),
                format!("send {:#x} {:#x} {:#x} {:#x}", 0x1234, 0x8005, 7, ev::NIN_SELECT),
            ]
        );
    }

    #[test]
    fn a_right_click_on_a_version_4_icon_packs_the_anchor_and_the_id() {
        let (s, id) = shared_with(IconVersion::V4, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::RightClick)), ClickOutcome::Sent(3));
        let sent = o.sent();
        // MAKEWPARAM(500, 600) and MAKELPARAM(event, 7)
        assert_eq!(sent[0], (0x1234, 0x8005, (600usize << 16) | 500, ((7isize) << 16) | ev::WM_RBUTTONDOWN as isize));
        assert_eq!(sent[2], (0x1234, 0x8005, (600usize << 16) | 500, ((7isize) << 16) | ev::WM_CONTEXTMENU as isize));
    }

    #[test]
    fn a_keyboard_activation_anchors_at_the_icon_corner() {
        let (s, id) = shared_with(IconVersion::V4, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::KeyboardSelect)), ClickOutcome::Sent(1));
        assert_eq!(o.sent()[0], (0x1234, 0x8005, (590usize << 16) | 490, ((7isize) << 16) | ev::NIN_KEYSELECT as isize));
        // version 0 gets the plain button pair
        let (s0, id0) = shared_with(IconVersion::V0, 0x1234, 7, 50);
        let o0 = FakeOwners::new();
        o0.add_window(0x1234, 50);
        assert_eq!(deliver(&s0, &o0, &click(id0, HostEvent::KeyboardSelect)), ClickOutcome::Sent(2));
        assert_eq!(o0.sent()[0].2, 7);
        assert_eq!(o0.sent()[0].3, ev::WM_LBUTTONDOWN as isize);
    }

    #[test]
    fn a_double_click_and_a_middle_click() {
        let (s, id) = shared_with(IconVersion::V0, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::DoubleClick)), ClickOutcome::Sent(2));
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::MiddleClick)), ClickOutcome::Sent(2));
        let kinds: Vec<isize> = o.sent().iter().map(|s| s.3).collect();
        assert_eq!(kinds, vec![ev::WM_LBUTTONDBLCLK as isize, ev::WM_LBUTTONUP as isize, ev::WM_MBUTTONDOWN as isize, ev::WM_MBUTTONUP as isize]);
    }

    #[test]
    fn hover_does_not_hand_over_the_foreground() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::HoverMove)), ClickOutcome::Sent(1));
        assert!(!o.log().iter().any(|l| l.starts_with("allow_foreground")));
    }

    #[test]
    fn an_owner_that_is_gone_is_dropped_and_never_sent_to() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new(); // no windows at all
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::OwnerGone);
        assert!(o.log().is_empty(), "no message, no foreground grant: {:?}", o.log());
        assert!(s.snapshot().icons.is_empty());
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::UnknownIcon);
        assert_eq!(s.counters.snapshot().clicks_dropped, 1);
    }

    #[test]
    fn a_click_expecting_one_owner_is_refused_when_the_icon_has_another_now() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        // the owner the caller drew: delivered
        assert_eq!(deliver_expecting(&s, &o, &click(id, HostEvent::LeftClick), Some((0x1234, 50))), ClickOutcome::Sent(3));
        let before = o.log().len();
        // another process, another window: refused, nothing sent, no foreground granted, and the icon is left alone
        for other in [(0x1234, 99), (0x4321, 50)] {
            assert_eq!(deliver_expecting(&s, &o, &click(id, HostEvent::LeftClick), Some(other)), ClickOutcome::OwnerChanged);
        }
        assert_eq!(o.log().len(), before);
        assert_eq!(s.snapshot().icons.len(), 1);
        // no expectation: as `deliver`
        assert_eq!(deliver_expecting(&s, &o, &click(id, HostEvent::LeftClick), None), ClickOutcome::Sent(3));
    }

    #[test]
    fn a_window_that_belongs_to_another_process_now_is_not_the_owner() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 99); // the handle was reused by another program
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::RightClick)), ClickOutcome::OwnerGone);
        assert!(o.sent().is_empty());
        assert!(s.snapshot().icons.is_empty());
    }

    #[test]
    fn an_owner_of_unknown_process_is_never_sent_to() {
        // the window was not a window when the icon was added; a window with that handle value now may be anybody's
        let (s, id) = shared_with(IconVersion::V0, 0x1234, 7, 0);
        let o = FakeOwners::new();
        o.add_window(0x1234, 12345);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::OwnerGone);
        assert!(o.log().is_empty(), "no foreground grant, no message: {:?}", o.log());
        assert!(s.snapshot().icons.is_empty());
    }

    #[test]
    fn no_callback_means_nothing_to_send() {
        let (s, _rx) = Shared::new();
        s.apply(&req(Nim::Add, data(1, 1, nif::TIP)), Prepared::default());
        let id = s.snapshot().icons[0].id;
        let o = FakeOwners::new();
        o.add_window(1, 0);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::NoCallback);
        assert!(o.log().is_empty());
        assert_eq!(s.snapshot().icons.len(), 1, "an icon without a callback is not dropped for that");
    }

    #[test]
    fn a_failed_send_stops_the_sequence() {
        let (s, id) = shared_with(IconVersion::V3, 0x1234, 7, 50);
        let o = FakeOwners::new();
        o.add_window(0x1234, 50);
        o.fail_sends_after(1);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::Failed { sent: 1 });
        assert_eq!(s.snapshot().icons.len(), 1, "a refused send (an elevated owner) does not drop the icon");
    }

    #[test]
    fn the_sweep_drops_the_dead_and_keeps_the_living() {
        let (s, _rx) = Shared::new();
        for (h, pid) in [(10u32, 1u32), (11, 2), (12, 3)] {
            s.apply(&req(Nim::Add, data(h, 1, nif::MESSAGE)), Prepared { owner: Some((pid, "x.exe".into())), ..Default::default() });
        }
        let o = FakeOwners::new();
        o.add_window(10, 1);
        o.add_window(12, 77); // reused by another process
        assert_eq!(sweep(&s, &o), 2);
        let left: Vec<u32> = s.snapshot().icons.iter().map(|i| i.hwnd).collect();
        assert_eq!(left, vec![10]);
        assert_eq!(sweep(&s, &o), 0);
        assert!(o.sent().is_empty(), "the sweep sends nothing");
    }

    #[test]
    fn a_negative_window_handle_is_widened_with_its_sign() {
        let (s, _rx) = Shared::new();
        s.apply(&req(Nim::Add, data(0xFFFF_FFF0, 1, nif::MESSAGE)), Prepared { owner: Some((8, "a.exe".into())), ..Default::default() });
        let id = s.snapshot().icons[0].id;
        let o = FakeOwners::new();
        o.add_window(-16, 8);
        assert_eq!(deliver(&s, &o, &click(id, HostEvent::LeftClick)), ClickOutcome::Sent(2));
        assert_eq!(o.sent()[0].0, -16);
    }
}
