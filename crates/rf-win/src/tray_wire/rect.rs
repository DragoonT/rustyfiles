//! `dwData = 3`: `Shell_NotifyIconGetRect`. shell32 sends two requests (message 1, then 2) and builds the icon's `RECT` from the two
//! numbers the taskbar returns from `WM_COPYDATA`; a tray host is the one that answers.
//!
//! The request is `WINNOTIFYICONIDENTIFIER` (two implementations agree; 40 bytes):
//!
//! ```text
//! 0x00 4  magic     (value UNVERIFIED: the same word as the notify signature in one implementation's struct; not checked here)
//! 0x04 4  message   1 or 2
//! 0x08 4  cbSize
//! 0x0C 4  padding
//! 0x10 4  hWnd      (32 bits)
//! 0x14 4  uID
//! 0x18 16 guidItem
//! ```
//!
//! **How the two replies are packed is UNVERIFIED and two sources disagree** (note 2, section 1.2), so both readings are kept as
//! [`RectPacking`] and a self-calibrating check ([`matching_packings`]) says which one reproduces a known rectangle (probe P6: call the
//! real `Shell_NotifyIconGetRect` against a host that answers with a known rectangle). The value is returned as the `LRESULT` of the
//! `WM_COPYDATA`; each half is a signed 16-bit coordinate.

use super::bytes::{array16_at, i32_at, set_i32, set_u32, u32_at};
use super::{IconKey, Rect, WireError, WireGuid, NOTIFY_SIGNATURE, sign_extend_handle};

pub const LEN: usize = 40;

/// Which of the two requests of one `Shell_NotifyIconGetRect` call this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectMessage {
    First,
    Second,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconRectQuery {
    pub magic: i32,
    /// As sent; see [`IconRectQuery::part`].
    pub message: i32,
    pub cb_size: i32,
    pub hwnd: u32,
    pub uid: u32,
    pub guid: WireGuid,
    pub trailing_len: usize,
}

impl IconRectQuery {
    /// Needs [`LEN`] bytes; more are counted and ignored. The magic and the message number are not checked here (a host answers
    /// only messages 1 and 2 and forwards the rest).
    pub fn parse(b: &[u8]) -> Result<IconRectQuery, WireError> {
        let truncated = || WireError::Truncated { what: "icon rect query", need: LEN, have: b.len() };
        if b.len() < LEN {
            return Err(truncated());
        }
        Ok(IconRectQuery {
            magic: i32_at(b, 0).ok_or_else(truncated)?,
            message: i32_at(b, 4).ok_or_else(truncated)?,
            cb_size: i32_at(b, 8).ok_or_else(truncated)?,
            hwnd: u32_at(b, 0x10).ok_or_else(truncated)?,
            uid: u32_at(b, 0x14).ok_or_else(truncated)?,
            guid: WireGuid(array16_at(b, 0x18).ok_or_else(truncated)?),
            trailing_len: b.len() - LEN,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; LEN];
        set_i32(&mut b, 0, self.magic);
        set_i32(&mut b, 4, self.message);
        set_i32(&mut b, 8, self.cb_size);
        set_u32(&mut b, 0x10, self.hwnd);
        set_u32(&mut b, 0x14, self.uid);
        b[0x18..0x28].copy_from_slice(&self.guid.0);
        b
    }

    /// Message 1 or 2; `None` for anything else.
    pub fn part(&self) -> Option<RectMessage> {
        match self.message {
            1 => Some(RectMessage::First),
            2 => Some(RectMessage::Second),
            _ => None,
        }
    }

    /// Whether the magic equals the notify-icon signature (one implementation's struct suggests it does; UNVERIFIED).
    pub fn magic_is_notify_signature(&self) -> bool {
        self.magic as u32 == NOTIFY_SIGNATURE
    }

    /// How the icon is named: by GUID when it is not zero (Microsoft recommends that), else by window and id.
    pub fn key(&self) -> IconKey {
        if self.guid.is_zero() {
            IconKey::Wnd { hwnd: self.hwnd, uid: self.uid }
        } else {
            IconKey::Guid(self.guid)
        }
    }

    pub fn hwnd_handle(&self) -> isize {
        sign_extend_handle(self.hwnd)
    }
}

/// The two ways the reply numbers might be packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RectPacking {
    /// Message 1 answers `MAKELONG(left, right)`, message 2 `MAKELONG(top, bottom)` (the reading that makes one implementation's
    /// `MAKELONG(x, x)` / `MAKELONG(y, y + 1)` replies a consistent cursor-sized rectangle).
    Axes,
    /// Message 1 answers `MAKELONG(left, top)`, message 2 `MAKELONG(right, bottom)` (the top-left and bottom-right points).
    Corners,
}

fn make_long(lo: i32, hi: i32) -> Option<u32> {
    let lo = i16::try_from(lo).ok()?;
    let hi = i16::try_from(hi).ok()?;
    Some((lo as u16 as u32) | ((hi as u16 as u32) << 16))
}

fn split_long(v: u32) -> (i32, i32) {
    (v as u16 as i16 as i32, (v >> 16) as u16 as i16 as i32)
}

/// The reply to one request. `None` when a coordinate does not fit a signed 16-bit half (the caller then answers 0 = failure rather than
/// a wrong position).
pub fn pack_reply(packing: RectPacking, which: RectMessage, r: Rect) -> Option<u32> {
    match (packing, which) {
        (RectPacking::Axes, RectMessage::First) => make_long(r.left, r.right),
        (RectPacking::Axes, RectMessage::Second) => make_long(r.top, r.bottom),
        (RectPacking::Corners, RectMessage::First) => make_long(r.left, r.top),
        (RectPacking::Corners, RectMessage::Second) => make_long(r.right, r.bottom),
    }
}

/// The rectangle a sender builds from the two replies.
pub fn unpack_replies(packing: RectPacking, first: u32, second: u32) -> Rect {
    let (a, b) = split_long(first);
    let (c, d) = split_long(second);
    match packing {
        RectPacking::Axes => Rect { left: a, right: b, top: c, bottom: d },
        RectPacking::Corners => Rect { left: a, top: b, right: c, bottom: d },
    }
}

/// The self-calibration of probe P6: given the two numbers the real `Shell_NotifyIconGetRect` produced from a host that answered with
/// the packing under test, which readings turn them back into `known`? One entry when the rectangle tells the readings apart, two
/// when it does not (so pick a rectangle with `left != top` or `right != bottom`), none when the replies are neither.
pub fn matching_packings(first: u32, second: u32, known: Rect) -> Vec<RectPacking> {
    [RectPacking::Axes, RectPacking::Corners].into_iter().filter(|p| unpack_replies(*p, first, second) == known).collect()
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::super::notify::tests_support::put32;

    /// message 1, hWnd 0x10ABC, uID 9, no GUID, cbSize 40.
    pub fn golden() -> Vec<u8> {
        let mut b = vec![0u8; 40];
        put32(&mut b, 0x00, 0x3475_3423);
        put32(&mut b, 0x04, 1);
        put32(&mut b, 0x08, 40);
        put32(&mut b, 0x10, 0x0001_0ABC);
        put32(&mut b, 0x14, 9);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::tray_wire::XorShift;

    #[test]
    fn golden_parses_and_round_trips() {
        let r = IconRectQuery::parse(&golden()).unwrap();
        assert_eq!(r.magic as u32, 0x3475_3423);
        assert!(r.magic_is_notify_signature());
        assert_eq!(r.part(), Some(RectMessage::First));
        assert_eq!(r.cb_size, 40);
        assert_eq!(r.hwnd, 0x0001_0ABC);
        assert_eq!(r.uid, 9);
        assert!(r.guid.is_zero());
        assert_eq!(r.key(), IconKey::Wnd { hwnd: 0x0001_0ABC, uid: 9 });
        assert_eq!(r.trailing_len, 0);
        assert_eq!(r.encode(), golden());
    }

    #[test]
    fn guid_wins_over_window() {
        let mut b = golden();
        b[0x18..0x28].copy_from_slice(&[1; 16]);
        let r = IconRectQuery::parse(&b).unwrap();
        assert_eq!(r.key(), IconKey::Guid(WireGuid([1; 16])));
        assert_eq!(r.encode(), b);
    }

    #[test]
    fn message_numbers() {
        for (m, want) in [(1, Some(RectMessage::First)), (2, Some(RectMessage::Second)), (0, None), (3, None), (-1, None), (i32::MAX, None)] {
            let mut b = golden();
            b[4..8].copy_from_slice(&m.to_le_bytes());
            assert_eq!(IconRectQuery::parse(&b).unwrap().part(), want, "message {m}");
        }
    }

    #[test]
    fn unknown_magic_is_not_refused() {
        let mut b = golden();
        b[0..4].copy_from_slice(&0u32.to_le_bytes());
        let r = IconRectQuery::parse(&b).unwrap();
        assert!(!r.magic_is_notify_signature());
    }

    #[test]
    fn every_truncation_is_an_error() {
        let g = golden();
        for n in 0..LEN {
            assert!(matches!(IconRectQuery::parse(&g[..n]), Err(WireError::Truncated { need: 40, .. })), "len {n}");
        }
        let mut longer = g.clone();
        longer.extend_from_slice(&[0; 9]);
        assert_eq!(IconRectQuery::parse(&longer).unwrap().trailing_len, 9);
    }

    #[test]
    fn negative_handle_widens() {
        let mut b = golden();
        b[0x10..0x14].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        assert_eq!(IconRectQuery::parse(&b).unwrap().hwnd_handle(), -1);
    }

    const RECTS: [Rect; 5] = [
        Rect { left: 2319, top: 1392, right: 2560 - 1, bottom: 1440 },
        Rect { left: -1920, top: 20, right: -1900, bottom: 44 },
        Rect { left: 0, top: 0, right: 16, bottom: 16 },
        Rect { left: -32768, top: -32768, right: 32767, bottom: 32767 },
        Rect { left: 100, top: 100, right: 100, bottom: 100 }, // degenerate: both readings coincide
    ];

    #[test]
    fn replies_round_trip_under_each_packing() {
        for packing in [RectPacking::Axes, RectPacking::Corners] {
            for r in RECTS {
                let a = pack_reply(packing, RectMessage::First, r).unwrap();
                let b = pack_reply(packing, RectMessage::Second, r).unwrap();
                assert_eq!(unpack_replies(packing, a, b), r, "{packing:?} {r:?}");
            }
        }
    }

    #[test]
    fn packing_layouts_are_what_the_docs_say() {
        let r = Rect { left: 1, top: 2, right: 3, bottom: 4 };
        assert_eq!(pack_reply(RectPacking::Axes, RectMessage::First, r), Some(0x0003_0001));
        assert_eq!(pack_reply(RectPacking::Axes, RectMessage::Second, r), Some(0x0004_0002));
        assert_eq!(pack_reply(RectPacking::Corners, RectMessage::First, r), Some(0x0002_0001));
        assert_eq!(pack_reply(RectPacking::Corners, RectMessage::Second, r), Some(0x0004_0003));
        // negative halves are two's complement 16-bit
        let n = Rect { left: -1, top: -2, right: 0, bottom: 0 };
        assert_eq!(pack_reply(RectPacking::Corners, RectMessage::First, n), Some(0xFFFE_FFFF));
    }

    #[test]
    fn coordinates_beyond_sixteen_bits_are_refused_not_wrapped() {
        let r = Rect { left: 40_000, top: 0, right: 40_001, bottom: 10 };
        assert_eq!(pack_reply(RectPacking::Axes, RectMessage::First, r), None);
        assert!(pack_reply(RectPacking::Axes, RectMessage::Second, r).is_some(), "the vertical half still fits");
        assert_eq!(pack_reply(RectPacking::Corners, RectMessage::First, r), None);
        let r = Rect { left: 0, top: -32769, right: 0, bottom: 0 };
        assert_eq!(pack_reply(RectPacking::Corners, RectMessage::First, r), None);
    }

    #[test]
    fn calibration_tells_the_readings_apart() {
        let known = RECTS[0];
        for packing in [RectPacking::Axes, RectPacking::Corners] {
            let a = pack_reply(packing, RectMessage::First, known).unwrap();
            let b = pack_reply(packing, RectMessage::Second, known).unwrap();
            assert_eq!(matching_packings(a, b, known), vec![packing]);
        }
        // a degenerate rectangle cannot tell them apart: both match, which is the cue to pick another one
        let deg = RECTS[4];
        let a = pack_reply(RectPacking::Axes, RectMessage::First, deg).unwrap();
        let b = pack_reply(RectPacking::Axes, RectMessage::Second, deg).unwrap();
        assert_eq!(matching_packings(a, b, deg).len(), 2);
        // replies that match neither
        assert!(matching_packings(0x1234_5678, 0x9ABC_DEF0, known).is_empty());
    }

    #[test]
    fn unpack_never_panics() {
        let mut rng = XorShift(7);
        for _ in 0..10_000 {
            let (a, b) = (rng.next() as u32, rng.next() as u32);
            for p in [RectPacking::Axes, RectPacking::Corners] {
                let _ = unpack_replies(p, a, b);
            }
        }
    }
}
