//! The other direction: what a tray host sends to an icon's owner window when the user does something with the icon. Pure: a host event
//! and the icon's version go in, the list of callback messages with their `wParam` / `lParam` come out. Delivery (posting them, the
//! `AllowSetForegroundWindow` before a button-down) is the host's business; [`grants_foreground`] says when it is due.
//!
//! The message that is sent is always the icon's own `uCallbackMessage` to its `hWnd`; what changes by version is the packing:
//!
//! | version | `wParam` | `lParam` |
//! |---|---|---|
//! | 0 and 3 | `uID` | the event (a mouse message, `WM_CONTEXTMENU`, `NIN_*`) |
//! | 4 | `MAKEWPARAM(x, y)`: the anchor point, signed 16-bit halves, screen coordinates | `MAKELPARAM(event, uID)`: the event in the low word, the 16-bit icon id in the high word |
//!
//! Version 4 documents the anchor for `NIN_POPUPOPEN`, `NIN_SELECT`, `NIN_KEYSELECT` and the mouse messages; the Microsoft sample also
//! reads it for `WM_CONTEXTMENU`, so it is set there too. For the other events (`NIN_POPUPCLOSE`, the balloon events) `wParam` is
//! documented as undefined and is 0 here.
//!
//! What Explorer-compatible hosts send (note 2, section 1.4): a left click is button-down, button-up, then `NIN_SELECT` from version
//! 3 on; a right click is button-down, button-up, then `WM_CONTEXTMENU` from version 3 on; a double click is the double-click message
//! and the button-up (and `NIN_SELECT` from 3); the keyboard gives `NIN_KEYSELECT` / `WM_CONTEXTMENU` from version 3 and the
//! button-message pairs below it; hovering sends `WM_MOUSEMOVE`, and `NIN_POPUPOPEN` / `NIN_POPUPCLOSE` only for version 4 and only
//! when the caller asks for rich popups (Windows 11's own tray never sends them: UNVERIFIED on 26H2). ManagedShell also sends
//! `WM_MOUSEHOVER` / `WM_MOUSELEAVE`; this table leaves them out until the spy shows that Explorer sends them (UNVERIFIED either way).

use super::{IconVersion, Point};

/// Message / notification numbers used as callback events.
pub mod ev {
    pub const WM_MOUSEMOVE: u32 = 0x0200;
    pub const WM_LBUTTONDOWN: u32 = 0x0201;
    pub const WM_LBUTTONUP: u32 = 0x0202;
    pub const WM_LBUTTONDBLCLK: u32 = 0x0203;
    pub const WM_RBUTTONDOWN: u32 = 0x0204;
    pub const WM_RBUTTONUP: u32 = 0x0205;
    pub const WM_MBUTTONDOWN: u32 = 0x0207;
    pub const WM_MBUTTONUP: u32 = 0x0208;
    /// `WM_MOUSEFIRST` .. `WM_MOUSELAST` (the range the version-4 documentation lists for the anchor).
    pub const WM_MOUSEFIRST: u32 = 0x0200;
    pub const WM_MOUSELAST: u32 = 0x020E;
    pub const WM_CONTEXTMENU: u32 = 0x007B;
    pub const WM_USER: u32 = 0x0400;
    pub const NIN_SELECT: u32 = WM_USER;
    pub const NINF_KEY: u32 = 1;
    pub const NIN_KEYSELECT: u32 = NIN_SELECT | NINF_KEY;
    pub const NIN_BALLOONSHOW: u32 = WM_USER + 2;
    pub const NIN_BALLOONHIDE: u32 = WM_USER + 3;
    pub const NIN_BALLOONTIMEOUT: u32 = WM_USER + 4;
    pub const NIN_BALLOONUSERCLICK: u32 = WM_USER + 5;
    pub const NIN_POPUPOPEN: u32 = WM_USER + 6;
    pub const NIN_POPUPCLOSE: u32 = WM_USER + 7;
}

/// Something that happened to an icon, from the host's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostEvent {
    LeftClick,
    DoubleClick,
    RightClick,
    MiddleClick,
    /// The pointer entered the icon.
    HoverIn,
    /// The pointer moved inside the icon.
    HoverMove,
    /// The pointer left the icon.
    HoverOut,
    /// Space / Enter on the focused icon.
    KeyboardSelect,
    /// The menu key / Shift+F10 on the focused icon.
    ContextMenuKey,
    BalloonShown,
    BalloonHidden,
    BalloonTimedOut,
    BalloonClicked,
}

/// The icon a message is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconRef {
    /// `uID` as the sender gave it (32 bits). Version 4 can carry only the low 16 bits (see [`uid_fits_v4`]).
    pub uid: u32,
    pub version: IconVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClickOptions {
    /// Send `NIN_POPUPOPEN` / `NIN_POPUPCLOSE` to version-4 icons on hover. Off by default: Windows 11's tray does not.
    pub rich_popups: bool,
}

/// One message for the callback: the event it carries and the packed words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Callback {
    pub event: u32,
    pub wparam: usize,
    pub lparam: isize,
}

/// The events (in order) a host event turns into for an icon of `version`.
pub fn events_for(ev: HostEvent, version: IconVersion, opts: ClickOptions) -> Vec<u32> {
    use self::ev as e;
    let v3 = version.at_least_v3();
    let mut out = Vec::new();
    match ev {
        HostEvent::LeftClick => {
            out.extend([e::WM_LBUTTONDOWN, e::WM_LBUTTONUP]);
            if v3 {
                out.push(e::NIN_SELECT);
            }
        }
        HostEvent::DoubleClick => {
            out.extend([e::WM_LBUTTONDBLCLK, e::WM_LBUTTONUP]);
            if v3 {
                out.push(e::NIN_SELECT);
            }
        }
        HostEvent::RightClick => {
            out.extend([e::WM_RBUTTONDOWN, e::WM_RBUTTONUP]);
            if v3 {
                out.push(e::WM_CONTEXTMENU);
            }
        }
        HostEvent::MiddleClick => out.extend([e::WM_MBUTTONDOWN, e::WM_MBUTTONUP]),
        HostEvent::HoverIn => {
            out.push(e::WM_MOUSEMOVE);
            if version == IconVersion::V4 && opts.rich_popups {
                out.push(e::NIN_POPUPOPEN);
            }
        }
        HostEvent::HoverMove => out.push(e::WM_MOUSEMOVE),
        HostEvent::HoverOut => {
            if version == IconVersion::V4 && opts.rich_popups {
                out.push(e::NIN_POPUPCLOSE);
            }
        }
        HostEvent::KeyboardSelect => {
            if v3 {
                out.push(e::NIN_KEYSELECT);
            } else {
                out.extend([e::WM_LBUTTONDOWN, e::WM_LBUTTONUP]);
            }
        }
        HostEvent::ContextMenuKey => {
            if v3 {
                out.push(e::WM_CONTEXTMENU);
            } else {
                out.extend([e::WM_RBUTTONDOWN, e::WM_RBUTTONUP]);
            }
        }
        HostEvent::BalloonShown => out.push(e::NIN_BALLOONSHOW),
        HostEvent::BalloonHidden => out.push(e::NIN_BALLOONHIDE),
        HostEvent::BalloonTimedOut => out.push(e::NIN_BALLOONTIMEOUT),
        HostEvent::BalloonClicked => out.push(e::NIN_BALLOONUSERCLICK),
    }
    out
}

/// Whether a version-4 `wParam` carries the anchor point for this event.
pub fn event_has_anchor(event: u32) -> bool {
    use self::ev as e;
    (e::WM_MOUSEFIRST..=e::WM_MOUSELAST).contains(&event)
        || matches!(event, e::WM_CONTEXTMENU | e::NIN_SELECT | e::NIN_KEYSELECT | e::NIN_POPUPOPEN)
}

/// Clamps a coordinate to the signed 16-bit range a version-4 anchor can hold (an anchor beyond +-32767 cannot be encoded; the
/// nearest representable point is sent rather than a wrapped one).
fn clamp16(v: i32) -> i16 {
    v.clamp(i16::MIN as i32, i16::MAX as i32) as i16
}

/// `MAKEWPARAM(x, y)` for an anchor: low word x, high word y, as a `WPARAM` with the upper bits zero.
pub fn pack_anchor(p: Point) -> usize {
    (clamp16(p.x) as u16 as usize) | ((clamp16(p.y) as u16 as usize) << 16)
}

/// `GET_X_LPARAM` / `GET_Y_LPARAM` of a version-4 `wParam`.
pub fn unpack_anchor(wparam: usize) -> Point {
    Point { x: (wparam as u16 as i16) as i32, y: ((wparam >> 16) as u16 as i16) as i32 }
}

/// `MAKELPARAM(event, uID)` of version 4: both truncated to 16 bits, the result zero-extended (as `MAKELPARAM` does).
pub fn pack_lparam_v4(event: u32, uid: u32) -> isize {
    (((event & 0xFFFF) | ((uid & 0xFFFF) << 16)) as usize) as isize
}

/// `LOWORD` / `HIWORD` of a version-4 `lParam`: `(event, icon id)`.
pub fn unpack_lparam_v4(lparam: isize) -> (u32, u32) {
    let v = lparam as usize as u32;
    (v & 0xFFFF, v >> 16)
}

/// Version 4 holds only the low 16 bits of `uID`. A sender that uses larger ids and asks for version 4 gets ambiguous messages; the
/// host may want to refuse the `NIM_SETVERSION` for such an icon.
pub fn uid_fits_v4(uid: u32) -> bool {
    uid <= 0xFFFF
}

/// Packs one event for an icon. `anchor` is used only for version 4 and only for events that carry it.
pub fn pack(icon: IconRef, event: u32, anchor: Point) -> Callback {
    if icon.version == IconVersion::V4 {
        let wparam = if event_has_anchor(event) { pack_anchor(anchor) } else { 0 };
        Callback { event, wparam, lparam: pack_lparam_v4(event, icon.uid) }
    } else {
        Callback { event, wparam: icon.uid as usize, lparam: event as isize }
    }
}

/// The callbacks for a host event, in the order they must be sent.
pub fn callbacks(icon: IconRef, ev: HostEvent, anchor: Point, opts: ClickOptions) -> Vec<Callback> {
    events_for(ev, icon.version, opts).into_iter().map(|e| pack(icon, e, anchor)).collect()
}

/// The anchor for a host event: keyboard events anchor at the icon's upper-left corner (the documented rule), the others at the
/// pointer.
pub fn anchor_for(ev: HostEvent, cursor: Point, icon_top_left: Point) -> Point {
    match ev {
        HostEvent::KeyboardSelect | HostEvent::ContextMenuKey => icon_top_left,
        _ => cursor,
    }
}

/// Whether the owner must be given the right to take the foreground (`AllowSetForegroundWindow(pid)`) just before the first message
/// of this event: the events after which an app opens a menu or a window. Without it a popup menu often does not take focus or close
/// on an outside click.
pub fn grants_foreground(ev: HostEvent) -> bool {
    matches!(
        ev,
        HostEvent::LeftClick
            | HostEvent::DoubleClick
            | HostEvent::RightClick
            | HostEvent::MiddleClick
            | HostEvent::KeyboardSelect
            | HostEvent::ContextMenuKey
            | HostEvent::BalloonClicked
    )
}

/// What the receiving side reads out of a callback (the inverse of [`pack`]): used to test the packing and by a tray host that talks to
/// its own icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Decoded {
    pub event: u32,
    pub uid: u32,
    /// Only for version 4 and only for events that carry one.
    pub anchor: Option<Point>,
}

pub fn decode(version: IconVersion, wparam: usize, lparam: isize) -> Decoded {
    if version == IconVersion::V4 {
        let (event, uid) = unpack_lparam_v4(lparam);
        Decoded { event, uid, anchor: event_has_anchor(event).then(|| unpack_anchor(wparam)) }
    } else {
        Decoded { event: lparam as usize as u32, uid: wparam as u32, anchor: None }
    }
}

#[cfg(test)]
mod tests {
    use super::ev as e;
    use super::*;
    use crate::tray_wire::XorShift;

    const V0: IconVersion = IconVersion::V0;
    const V3: IconVersion = IconVersion::V3;
    const V4: IconVersion = IconVersion::V4;
    const OFF: ClickOptions = ClickOptions { rich_popups: false };
    const RICH: ClickOptions = ClickOptions { rich_popups: true };

    #[test]
    fn numbers_match_windows() {
        use windows::Win32::UI::Shell as sh;
        use windows::Win32::UI::WindowsAndMessaging as wm;
        assert_eq!(e::WM_MOUSEMOVE, wm::WM_MOUSEMOVE);
        assert_eq!(e::WM_LBUTTONDOWN, wm::WM_LBUTTONDOWN);
        assert_eq!(e::WM_LBUTTONUP, wm::WM_LBUTTONUP);
        assert_eq!(e::WM_LBUTTONDBLCLK, wm::WM_LBUTTONDBLCLK);
        assert_eq!(e::WM_RBUTTONDOWN, wm::WM_RBUTTONDOWN);
        assert_eq!(e::WM_RBUTTONUP, wm::WM_RBUTTONUP);
        assert_eq!(e::WM_MBUTTONDOWN, wm::WM_MBUTTONDOWN);
        assert_eq!(e::WM_MBUTTONUP, wm::WM_MBUTTONUP);
        assert_eq!(e::WM_MOUSEFIRST, wm::WM_MOUSEFIRST);
        assert_eq!(e::WM_CONTEXTMENU, wm::WM_CONTEXTMENU);
        assert_eq!(e::WM_USER, wm::WM_USER);
        assert_eq!(e::NIN_SELECT, sh::NIN_SELECT);
        assert_eq!(e::NINF_KEY, sh::NINF_KEY);
        assert_eq!(e::NIN_KEYSELECT, sh::NIN_SELECT | sh::NINF_KEY, "the header defines it as NIN_SELECT | NINF_KEY");
        assert_eq!(e::NIN_BALLOONSHOW, sh::NIN_BALLOONSHOW);
        assert_eq!(e::NIN_BALLOONHIDE, sh::NIN_BALLOONHIDE);
        assert_eq!(e::NIN_BALLOONTIMEOUT, sh::NIN_BALLOONTIMEOUT);
        assert_eq!(e::NIN_BALLOONUSERCLICK, sh::NIN_BALLOONUSERCLICK);
        assert_eq!(e::NIN_POPUPOPEN, sh::NIN_POPUPOPEN);
        assert_eq!(e::NIN_POPUPCLOSE, sh::NIN_POPUPCLOSE);
    }

    #[test]
    fn left_click_table() {
        assert_eq!(events_for(HostEvent::LeftClick, V0, OFF), [e::WM_LBUTTONDOWN, e::WM_LBUTTONUP]);
        for v in [V3, V4] {
            assert_eq!(events_for(HostEvent::LeftClick, v, OFF), [e::WM_LBUTTONDOWN, e::WM_LBUTTONUP, e::NIN_SELECT]);
        }
    }

    #[test]
    fn double_click_table() {
        assert_eq!(events_for(HostEvent::DoubleClick, V0, OFF), [e::WM_LBUTTONDBLCLK, e::WM_LBUTTONUP]);
        assert_eq!(events_for(HostEvent::DoubleClick, V3, OFF), [e::WM_LBUTTONDBLCLK, e::WM_LBUTTONUP, e::NIN_SELECT]);
        assert_eq!(events_for(HostEvent::DoubleClick, V4, OFF), [e::WM_LBUTTONDBLCLK, e::WM_LBUTTONUP, e::NIN_SELECT]);
    }

    #[test]
    fn right_and_middle_click_table() {
        assert_eq!(events_for(HostEvent::RightClick, V0, OFF), [e::WM_RBUTTONDOWN, e::WM_RBUTTONUP]);
        for v in [V3, V4] {
            assert_eq!(events_for(HostEvent::RightClick, v, OFF), [e::WM_RBUTTONDOWN, e::WM_RBUTTONUP, e::WM_CONTEXTMENU]);
        }
        for v in [V0, V3, V4] {
            assert_eq!(events_for(HostEvent::MiddleClick, v, OFF), [e::WM_MBUTTONDOWN, e::WM_MBUTTONUP]);
        }
    }

    #[test]
    fn hover_table() {
        for v in [V0, V3] {
            for o in [OFF, RICH] {
                assert_eq!(events_for(HostEvent::HoverIn, v, o), [e::WM_MOUSEMOVE]);
                assert!(events_for(HostEvent::HoverOut, v, o).is_empty(), "only version 4 has popup close");
            }
        }
        assert_eq!(events_for(HostEvent::HoverIn, V4, OFF), [e::WM_MOUSEMOVE]);
        assert!(events_for(HostEvent::HoverOut, V4, OFF).is_empty());
        assert_eq!(events_for(HostEvent::HoverIn, V4, RICH), [e::WM_MOUSEMOVE, e::NIN_POPUPOPEN]);
        assert_eq!(events_for(HostEvent::HoverOut, V4, RICH), [e::NIN_POPUPCLOSE]);
        for v in [V0, V3, V4] {
            assert_eq!(events_for(HostEvent::HoverMove, v, RICH), [e::WM_MOUSEMOVE]);
        }
    }

    #[test]
    fn keyboard_table() {
        assert_eq!(events_for(HostEvent::KeyboardSelect, V0, OFF), [e::WM_LBUTTONDOWN, e::WM_LBUTTONUP]);
        assert_eq!(events_for(HostEvent::KeyboardSelect, V3, OFF), [e::NIN_KEYSELECT]);
        assert_eq!(events_for(HostEvent::KeyboardSelect, V4, OFF), [e::NIN_KEYSELECT]);
        assert_eq!(events_for(HostEvent::ContextMenuKey, V0, OFF), [e::WM_RBUTTONDOWN, e::WM_RBUTTONUP]);
        assert_eq!(events_for(HostEvent::ContextMenuKey, V3, OFF), [e::WM_CONTEXTMENU]);
        assert_eq!(events_for(HostEvent::ContextMenuKey, V4, OFF), [e::WM_CONTEXTMENU]);
    }

    #[test]
    fn balloon_events_are_the_same_in_every_version() {
        for v in [V0, V3, V4] {
            assert_eq!(events_for(HostEvent::BalloonShown, v, OFF), [e::NIN_BALLOONSHOW]);
            assert_eq!(events_for(HostEvent::BalloonHidden, v, OFF), [e::NIN_BALLOONHIDE]);
            assert_eq!(events_for(HostEvent::BalloonTimedOut, v, OFF), [e::NIN_BALLOONTIMEOUT]);
            assert_eq!(events_for(HostEvent::BalloonClicked, v, OFF), [e::NIN_BALLOONUSERCLICK]);
        }
    }

    #[test]
    fn packing_for_versions_0_and_3_is_uid_and_event() {
        for v in [V0, V3] {
            let icon = IconRef { uid: 0x1234_5678, version: v };
            let cb = callbacks(icon, HostEvent::LeftClick, Point { x: 500, y: 600 }, OFF);
            assert_eq!(cb[0], Callback { event: e::WM_LBUTTONDOWN, wparam: 0x1234_5678, lparam: 0x201 });
            assert!(cb.iter().all(|c| c.wparam == 0x1234_5678), "the anchor is not sent below version 4");
            assert_eq!(cb.last().unwrap().lparam, if v == V3 { e::NIN_SELECT as isize } else { e::WM_LBUTTONUP as isize });
        }
    }

    #[test]
    fn packing_for_version_4_is_anchor_and_event_plus_id() {
        let icon = IconRef { uid: 0x0007, version: V4 };
        let cb = callbacks(icon, HostEvent::LeftClick, Point { x: 500, y: 600 }, OFF);
        assert_eq!(cb.len(), 3);
        // MAKEWPARAM(500, 600) = 600 << 16 | 500
        assert_eq!(cb[0].wparam, (600usize << 16) | 500);
        // MAKELPARAM(WM_LBUTTONDOWN, 7) = 7 << 16 | 0x201
        assert_eq!(cb[0].lparam, ((7isize) << 16) | 0x201);
        assert_eq!(cb[2].lparam, (7isize << 16) | e::NIN_SELECT as isize);
        assert_eq!(cb[2].wparam, (600usize << 16) | 500, "NIN_SELECT carries the anchor");
        // HIWORD(lParam) is the icon id, LOWORD the event
        let (event, id) = unpack_lparam_v4(cb[1].lparam);
        assert_eq!((event, id), (e::WM_LBUTTONUP, 7));
    }

    #[test]
    fn negative_anchors_on_a_monitor_left_of_the_primary() {
        let icon = IconRef { uid: 1, version: V4 };
        let p = Point { x: -1500, y: 20 };
        let cb = pack(icon, e::WM_RBUTTONUP, p);
        assert_eq!(cb.wparam, ((20usize) << 16) | (-1500i16 as u16 as usize));
        assert_eq!(unpack_anchor(cb.wparam), p, "GET_X_LPARAM sign-extends");
        // a negative y too
        let p = Point { x: 10, y: -700 };
        assert_eq!(unpack_anchor(pack_anchor(p)), p);
        assert_eq!(pack_anchor(Point { x: -1, y: -1 }), 0xFFFF_FFFF, "upper bits stay zero (MAKEWPARAM is a DWORD)");
    }

    #[test]
    fn anchors_beyond_sixteen_bits_are_clamped_not_wrapped() {
        assert_eq!(unpack_anchor(pack_anchor(Point { x: 40_000, y: -40_000 })), Point { x: 32767, y: -32768 });
        assert_eq!(unpack_anchor(pack_anchor(Point { x: i32::MAX, y: i32::MIN })), Point { x: 32767, y: -32768 });
        assert_eq!(unpack_anchor(pack_anchor(Point { x: 32767, y: -32768 })), Point { x: 32767, y: -32768 });
    }

    #[test]
    fn version_4_icon_id_keeps_the_low_sixteen_bits() {
        let cb = pack(IconRef { uid: 0x0001_2345, version: V4 }, e::WM_LBUTTONUP, Point::default());
        assert_eq!(unpack_lparam_v4(cb.lparam).1, 0x2345);
        assert!(!uid_fits_v4(0x0001_2345));
        assert!(uid_fits_v4(0xFFFF));
        assert!(uid_fits_v4(0));
        // a high id never leaks into the event word
        assert_eq!(unpack_lparam_v4(pack_lparam_v4(0xFFFF_FFFF, 0xFFFF_FFFF)), (0xFFFF, 0xFFFF));
        // and the result is non-negative, like MAKELPARAM
        assert!(pack_lparam_v4(0xFFFF, 0xFFFF) > 0);
    }

    #[test]
    fn events_without_an_anchor_send_zero_in_wparam_for_version_4() {
        let icon = IconRef { uid: 3, version: V4 };
        let p = Point { x: 11, y: 22 };
        for event in [e::NIN_BALLOONSHOW, e::NIN_BALLOONHIDE, e::NIN_BALLOONTIMEOUT, e::NIN_BALLOONUSERCLICK, e::NIN_POPUPCLOSE] {
            assert_eq!(pack(icon, event, p).wparam, 0, "event {event:#x}");
        }
        for event in [e::WM_MOUSEMOVE, e::WM_LBUTTONDOWN, e::WM_RBUTTONUP, e::WM_CONTEXTMENU, e::NIN_SELECT, e::NIN_KEYSELECT, e::NIN_POPUPOPEN] {
            assert_eq!(unpack_anchor(pack(icon, event, p).wparam), p, "event {event:#x}");
        }
    }

    #[test]
    fn balloon_events_for_old_versions_carry_the_uid() {
        let icon = IconRef { uid: 99, version: V0 };
        let cb = callbacks(icon, HostEvent::BalloonClicked, Point::default(), OFF);
        assert_eq!(cb, vec![Callback { event: e::NIN_BALLOONUSERCLICK, wparam: 99, lparam: e::NIN_BALLOONUSERCLICK as isize }]);
    }

    #[test]
    fn decode_inverts_pack() {
        let p = Point { x: -300, y: 1200 };
        for v in [V0, V3, V4] {
            for uid in [0u32, 1, 7, 0xFFFF] {
                for ev in [HostEvent::LeftClick, HostEvent::RightClick, HostEvent::DoubleClick, HostEvent::KeyboardSelect, HostEvent::BalloonShown, HostEvent::HoverIn] {
                    let icon = IconRef { uid, version: v };
                    for (cb, event) in callbacks(icon, ev, p, RICH).iter().zip(events_for(ev, v, RICH)) {
                        let d = decode(v, cb.wparam, cb.lparam);
                        assert_eq!(d.event, event);
                        assert_eq!(d.uid, uid);
                        match (v, event_has_anchor(event)) {
                            (IconVersion::V4, true) => assert_eq!(d.anchor, Some(p)),
                            _ => assert_eq!(d.anchor, None),
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn keyboard_anchors_at_the_icon_corner() {
        let cursor = Point { x: 900, y: 900 };
        let corner = Point { x: 2300, y: 1400 };
        assert_eq!(anchor_for(HostEvent::KeyboardSelect, cursor, corner), corner);
        assert_eq!(anchor_for(HostEvent::ContextMenuKey, cursor, corner), corner);
        for ev in [HostEvent::LeftClick, HostEvent::RightClick, HostEvent::HoverIn, HostEvent::DoubleClick, HostEvent::BalloonClicked] {
            assert_eq!(anchor_for(ev, cursor, corner), cursor);
        }
    }

    #[test]
    fn foreground_is_granted_for_the_events_that_open_things() {
        for ev in [HostEvent::LeftClick, HostEvent::RightClick, HostEvent::DoubleClick, HostEvent::MiddleClick, HostEvent::KeyboardSelect, HostEvent::ContextMenuKey, HostEvent::BalloonClicked] {
            assert!(grants_foreground(ev), "{ev:?}");
        }
        for ev in [HostEvent::HoverIn, HostEvent::HoverMove, HostEvent::HoverOut, HostEvent::BalloonShown, HostEvent::BalloonHidden, HostEvent::BalloonTimedOut] {
            assert!(!grants_foreground(ev), "{ev:?}");
        }
    }

    #[test]
    fn every_event_for_every_version_produces_a_list_without_panicking() {
        let evs = [
            HostEvent::LeftClick,
            HostEvent::DoubleClick,
            HostEvent::RightClick,
            HostEvent::MiddleClick,
            HostEvent::HoverIn,
            HostEvent::HoverMove,
            HostEvent::HoverOut,
            HostEvent::KeyboardSelect,
            HostEvent::ContextMenuKey,
            HostEvent::BalloonShown,
            HostEvent::BalloonHidden,
            HostEvent::BalloonTimedOut,
            HostEvent::BalloonClicked,
        ];
        let mut rng = XorShift(99);
        for _ in 0..5000 {
            let v = [V0, V3, V4][rng.below(3) as usize];
            let icon = IconRef { uid: rng.next() as u32, version: v };
            let p = Point { x: rng.next() as i32, y: rng.next() as i32 };
            let o = ClickOptions { rich_popups: rng.below(2) == 1 };
            for ev in evs {
                let cbs = callbacks(icon, ev, p, o);
                assert_eq!(cbs.len(), events_for(ev, v, o).len());
            }
        }
        // every host event of every version except a V0/V3 hover-out and a non-rich V4 hover-out sends something
        for ev in evs {
            if ev != HostEvent::HoverOut {
                for v in [V0, V3, V4] {
                    assert!(!events_for(ev, v, OFF).is_empty(), "{ev:?} {v:?}");
                }
            }
        }
    }
}
