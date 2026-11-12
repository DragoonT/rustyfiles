//! `dwData = 0`: `SHAppBarMessage`. shell32 wraps the `APPBARDATA` the program passed in an enhanced block that also carries the
//! message number and a shared-memory handle, so the taskbar can write results back (a `WM_COPYDATA` is one way).
//!
//! Two layouts are known; which one 26H2 sends is **UNVERIFIED** (the `tray_spy` example settles it). A parse picks by the length of
//! the block (`cbData`):
//!
//! ```text
//! APPBARDATAV2 (both layouts)                       V3 (ManagedShell, 0x40 bytes)     Vista (Chappell, 0x38 bytes)
//! 0x00 4  cbSize = 0x28                              0x28 4  dwMessage                  0x28 4  dwMessage
//! 0x04 4  hWnd (32 bits)                             0x2C 4  padding   (UNSURE)         0x2C 4  hSharedMemory (32-bit handle)
//! 0x08 4  uCallbackMessage                           0x30 8  hSharedMemory (i64)        0x30 4  dwSourceProcessId
//! 0x0C 4  uEdge                                      0x38 4  dwSourceProcessId          0x34 4  padding   (UNSURE)
//! 0x10 16 rc (left, top, right, bottom as i32)       0x3C 4  padding   (UNSURE)
//! 0x20 8  lParam (i64)
//! ```
//!
//! Unsure, marked in the fields below: the padding words, whether the shared-memory handle is 32 or 64 bits wide in the block, the
//! meaning of `lParam` per message (the `APPBARDATA.lParam` of `ABM_SETAUTOHIDEBAR`, `ABM_SETSTATE`, `ABM_WINDOWPOSCHANGED`), and the
//! shape of the replies (the taskbar writes them into the shared memory, and `ABM_GETTASKBARPOS` fills the rect). A relay forwards the
//! block untouched and never needs any of this; only an appbar *server* (shell mode) does.

use super::bytes::{i32_at, i64_at, set_i32, set_i64, set_u32, u32_at};
use super::{Rect, WireError, sign_extend_handle};

/// Size of `APPBARDATAV2`, the part both layouts share.
pub const ABD_LEN: usize = 0x28;
/// The ManagedShell layout.
pub const V3_LEN: usize = 0x40;
/// Chappell's Vista layout (`0x34` bytes of data, 8-aligned to `0x38`). Anything from here to [`V3_LEN`] - 1 is read as Vista.
pub const VISTA_MIN_LEN: usize = 0x34;
pub const VISTA_LEN: usize = 0x38;

/// `ABM_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbMessage {
    New,
    Remove,
    QueryPos,
    SetPos,
    GetState,
    GetTaskbarPos,
    Activate,
    GetAutoHideBar,
    SetAutoHideBar,
    WindowPosChanged,
    SetState,
    /// A number the header does not define (newer Windows may add some). Forward, do not guess.
    Unknown(u32),
}

impl AbMessage {
    pub fn from_u32(v: u32) -> AbMessage {
        match v {
            0 => AbMessage::New,
            1 => AbMessage::Remove,
            2 => AbMessage::QueryPos,
            3 => AbMessage::SetPos,
            4 => AbMessage::GetState,
            5 => AbMessage::GetTaskbarPos,
            6 => AbMessage::Activate,
            7 => AbMessage::GetAutoHideBar,
            8 => AbMessage::SetAutoHideBar,
            9 => AbMessage::WindowPosChanged,
            10 => AbMessage::SetState,
            other => AbMessage::Unknown(other),
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            AbMessage::New => 0,
            AbMessage::Remove => 1,
            AbMessage::QueryPos => 2,
            AbMessage::SetPos => 3,
            AbMessage::GetState => 4,
            AbMessage::GetTaskbarPos => 5,
            AbMessage::Activate => 6,
            AbMessage::GetAutoHideBar => 7,
            AbMessage::SetAutoHideBar => 8,
            AbMessage::WindowPosChanged => 9,
            AbMessage::SetState => 10,
            AbMessage::Unknown(v) => v,
        }
    }

    pub fn name(self) -> String {
        match self {
            AbMessage::New => "ABM_NEW".into(),
            AbMessage::Remove => "ABM_REMOVE".into(),
            AbMessage::QueryPos => "ABM_QUERYPOS".into(),
            AbMessage::SetPos => "ABM_SETPOS".into(),
            AbMessage::GetState => "ABM_GETSTATE".into(),
            AbMessage::GetTaskbarPos => "ABM_GETTASKBARPOS".into(),
            AbMessage::Activate => "ABM_ACTIVATE".into(),
            AbMessage::GetAutoHideBar => "ABM_GETAUTOHIDEBAR".into(),
            AbMessage::SetAutoHideBar => "ABM_SETAUTOHIDEBAR".into(),
            AbMessage::WindowPosChanged => "ABM_WINDOWPOSCHANGED".into(),
            AbMessage::SetState => "ABM_SETSTATE".into(),
            AbMessage::Unknown(v) => format!("ABM_{v}"),
        }
    }
}

/// `ABE_*`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbEdge {
    Left,
    Top,
    Right,
    Bottom,
    Other(u32),
}

impl AbEdge {
    pub fn from_u32(v: u32) -> AbEdge {
        match v {
            0 => AbEdge::Left,
            1 => AbEdge::Top,
            2 => AbEdge::Right,
            3 => AbEdge::Bottom,
            other => AbEdge::Other(other),
        }
    }
}

/// Which of the two known layouts a block was read as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppBarLayout {
    /// Chappell's, `0x38` bytes: shared memory is a 32-bit handle at `0x2C`, the pid at `0x30`.
    Vista,
    /// ManagedShell's `APPBARMSGDATAV3`, `0x40` bytes: shared memory is an `i64` at `0x30`, the pid at `0x38`.
    V3,
}

/// One `dwData = 0` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppBarRequest {
    pub layout: AppBarLayout,
    /// `APPBARDATA.cbSize` as sent (0x28 expected; not trusted).
    pub abd_cb_size: u32,
    /// `APPBARDATA.hWnd`, the raw 32 bits.
    pub hwnd: u32,
    pub callback_message: u32,
    pub edge: u32,
    pub rect: Rect,
    /// `APPBARDATA.lParam`. Meaning per message UNSURE (see the module docs).
    pub lparam: i64,
    pub message: AbMessage,
    /// The sender's shared-memory handle for the reply, sign-extended when the layout holds only 32 bits. Valid only in the sender's
    /// process (open it with `SHLockShared(handle, source_pid)`).
    pub shared_memory: i64,
    pub source_pid: u32,
    /// Bytes after the layout's end (opaque).
    pub trailing_len: usize,
}

impl AppBarRequest {
    /// Parses a `dwData = 0` block. Needs at least [`VISTA_MIN_LEN`] bytes; a block of [`V3_LEN`] or more is read as V3, a shorter one
    /// as Vista.
    pub fn parse(b: &[u8]) -> Result<AppBarRequest, WireError> {
        if b.len() < VISTA_MIN_LEN {
            return Err(WireError::Truncated { what: "appbar message", need: VISTA_MIN_LEN, have: b.len() });
        }
        // after the length check every read below is inside the block; `?`-free reads return a default only if that were false
        let rd = |o: usize| u32_at(b, o).unwrap_or(0);
        let ri = |o: usize| i32_at(b, o).unwrap_or(0);
        let message = AbMessage::from_u32(rd(0x28));
        let (layout, shared_memory, source_pid, used) = if b.len() >= V3_LEN {
            (AppBarLayout::V3, i64_at(b, 0x30).unwrap_or(0), rd(0x38), V3_LEN)
        } else {
            (AppBarLayout::Vista, sign_extend_handle(rd(0x2C)) as i64, rd(0x30), b.len().min(VISTA_LEN))
        };
        Ok(AppBarRequest {
            layout,
            abd_cb_size: rd(0),
            hwnd: rd(4),
            callback_message: rd(8),
            edge: rd(0x0C),
            rect: Rect { left: ri(0x10), top: ri(0x14), right: ri(0x18), bottom: ri(0x1C) },
            lparam: i64_at(b, 0x20).unwrap_or(0),
            message,
            shared_memory,
            source_pid,
            trailing_len: b.len() - used,
        })
    }

    /// `APPBARDATA.hWnd` widened for Win32.
    pub fn hwnd_handle(&self) -> isize {
        sign_extend_handle(self.hwnd)
    }

    pub fn edge_kind(&self) -> AbEdge {
        AbEdge::from_u32(self.edge)
    }

    /// Whether the embedded `cbSize` is the 0x28 of `APPBARDATAV2`. A different value is worth a log line, not a refusal.
    pub fn abd_size_is_expected(&self) -> bool {
        self.abd_cb_size as usize == ABD_LEN
    }

    /// Builds the block in the layout this request was read as (V3: 0x40 bytes, Vista: 0x38). Padding is written as zero. A shared
    /// memory handle that does not fit 32 bits is cut in the Vista layout.
    pub fn encode(&self) -> Vec<u8> {
        let len = match self.layout {
            AppBarLayout::V3 => V3_LEN,
            AppBarLayout::Vista => VISTA_LEN,
        };
        let mut b = vec![0u8; len];
        set_u32(&mut b, 0, self.abd_cb_size);
        set_u32(&mut b, 4, self.hwnd);
        set_u32(&mut b, 8, self.callback_message);
        set_u32(&mut b, 0x0C, self.edge);
        set_i32(&mut b, 0x10, self.rect.left);
        set_i32(&mut b, 0x14, self.rect.top);
        set_i32(&mut b, 0x18, self.rect.right);
        set_i32(&mut b, 0x1C, self.rect.bottom);
        set_i64(&mut b, 0x20, self.lparam);
        set_u32(&mut b, 0x28, self.message.as_u32());
        match self.layout {
            AppBarLayout::V3 => {
                set_i64(&mut b, 0x30, self.shared_memory);
                set_u32(&mut b, 0x38, self.source_pid);
            }
            AppBarLayout::Vista => {
                set_u32(&mut b, 0x2C, self.shared_memory as u32);
                set_u32(&mut b, 0x30, self.source_pid);
            }
        }
        b
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::super::notify::tests_support::put32;

    fn base(len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        put32(&mut b, 0x00, 0x28);
        put32(&mut b, 0x04, 0x0001_0DEF); // hWnd
        put32(&mut b, 0x08, 0x8100); // callback
        put32(&mut b, 0x0C, 3); // ABE_BOTTOM
        put32(&mut b, 0x10, (-1920i32) as u32); // left (a monitor to the left of the primary)
        put32(&mut b, 0x14, 1392);
        put32(&mut b, 0x18, 0);
        put32(&mut b, 0x1C, 1440);
        b[0x20..0x28].copy_from_slice(&0x1_0000_0001i64.to_le_bytes()); // lParam
        b
    }

    /// 0x40 bytes: message ABM_SETPOS (3), shared memory 0x1234_5678_9ABC, pid 4242.
    pub fn golden_v3() -> Vec<u8> {
        let mut b = base(0x40);
        put32(&mut b, 0x28, 3);
        b[0x30..0x38].copy_from_slice(&0x1234_5678_9ABCi64.to_le_bytes());
        put32(&mut b, 0x38, 4242);
        b
    }

    /// 0x38 bytes: message ABM_NEW (0), shared memory 0xFFFF_FFF0 (a negative 32-bit handle), pid 777.
    pub fn golden_vista() -> Vec<u8> {
        let mut b = base(0x38);
        put32(&mut b, 0x28, 0);
        put32(&mut b, 0x2C, 0xFFFF_FFF0);
        put32(&mut b, 0x30, 777);
        b
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn message_numbers_match_the_sdk_header() {
        use windows::Win32::UI::Shell as sh;
        for (m, v) in [
            (AbMessage::New, sh::ABM_NEW),
            (AbMessage::Remove, sh::ABM_REMOVE),
            (AbMessage::QueryPos, sh::ABM_QUERYPOS),
            (AbMessage::SetPos, sh::ABM_SETPOS),
            (AbMessage::GetState, sh::ABM_GETSTATE),
            (AbMessage::GetTaskbarPos, sh::ABM_GETTASKBARPOS),
            (AbMessage::Activate, sh::ABM_ACTIVATE),
            (AbMessage::GetAutoHideBar, sh::ABM_GETAUTOHIDEBAR),
            (AbMessage::SetAutoHideBar, sh::ABM_SETAUTOHIDEBAR),
            (AbMessage::WindowPosChanged, sh::ABM_WINDOWPOSCHANGED),
            (AbMessage::SetState, sh::ABM_SETSTATE),
        ] {
            assert_eq!(m.as_u32(), v, "{}", m.name());
            assert_eq!(AbMessage::from_u32(v), m);
        }
        assert_eq!(AbMessage::from_u32(11), AbMessage::Unknown(11));
        assert_eq!(AbMessage::Unknown(99).as_u32(), 99);
        assert_eq!(AbMessage::Unknown(99).name(), "ABM_99");
    }

    #[test]
    fn golden_v3_every_field() {
        let r = AppBarRequest::parse(&golden_v3()).unwrap();
        assert_eq!(r.layout, AppBarLayout::V3);
        assert!(r.abd_size_is_expected());
        assert_eq!(r.hwnd, 0x0001_0DEF);
        assert_eq!(r.hwnd_handle(), 0x0001_0DEF);
        assert_eq!(r.callback_message, 0x8100);
        assert_eq!(r.edge_kind(), AbEdge::Bottom);
        assert_eq!(r.rect, Rect { left: -1920, top: 1392, right: 0, bottom: 1440 });
        assert_eq!(r.lparam, 0x1_0000_0001);
        assert_eq!(r.message, AbMessage::SetPos);
        assert_eq!(r.shared_memory, 0x1234_5678_9ABC);
        assert_eq!(r.source_pid, 4242);
        assert_eq!(r.trailing_len, 0);
    }

    #[test]
    fn golden_vista_every_field() {
        let r = AppBarRequest::parse(&golden_vista()).unwrap();
        assert_eq!(r.layout, AppBarLayout::Vista);
        assert_eq!(r.message, AbMessage::New);
        assert_eq!(r.shared_memory, -16, "a 32-bit handle is sign-extended");
        assert_eq!(r.source_pid, 777);
        assert_eq!(r.rect.left, -1920);
        assert_eq!(r.trailing_len, 0);
    }

    #[test]
    fn layout_is_chosen_by_length() {
        let v3 = golden_v3();
        for n in 0..=v3.len() + 20 {
            let mut b = v3.clone();
            b.resize(n.max(v3.len()), 0);
            let b = &b[..n.min(b.len())];
            let r = AppBarRequest::parse(b);
            if n < VISTA_MIN_LEN {
                assert!(matches!(r, Err(WireError::Truncated { .. })), "len {n}");
            } else if n < V3_LEN {
                let r = r.unwrap();
                assert_eq!(r.layout, AppBarLayout::Vista, "len {n}");
                assert_eq!(r.trailing_len, n.saturating_sub(VISTA_LEN), "len {n}");
            } else {
                let r = r.unwrap();
                assert_eq!(r.layout, AppBarLayout::V3, "len {n}");
                assert_eq!(r.trailing_len, n - V3_LEN, "len {n}");
            }
        }
    }

    #[test]
    fn round_trips() {
        for raw in [golden_v3(), golden_vista()] {
            let r = AppBarRequest::parse(&raw).unwrap();
            assert_eq!(r.encode(), raw);
            assert_eq!(AppBarRequest::parse(&r.encode()).unwrap(), r);
        }
    }

    #[test]
    fn padding_is_ignored_on_read() {
        let mut a = golden_v3();
        let mut b = golden_v3();
        a[0x2C..0x30].fill(0);
        b[0x2C..0x30].fill(0xFF);
        b[0x3C..0x40].fill(0xEE);
        assert_eq!(AppBarRequest::parse(&a).unwrap(), AppBarRequest::parse(&b).unwrap());
    }

    #[test]
    fn unexpected_cb_size_is_reported_not_refused() {
        let mut b = golden_v3();
        b[0..4].copy_from_slice(&0x30u32.to_le_bytes());
        let r = AppBarRequest::parse(&b).unwrap();
        assert!(!r.abd_size_is_expected());
        assert_eq!(r.abd_cb_size, 0x30);
    }

    #[test]
    fn unknown_message_and_edge_are_kept() {
        let mut b = golden_v3();
        b[0x28..0x2C].copy_from_slice(&0xDEAD_u32.to_le_bytes());
        b[0x0C..0x10].copy_from_slice(&9u32.to_le_bytes());
        let r = AppBarRequest::parse(&b).unwrap();
        assert_eq!(r.message, AbMessage::Unknown(0xDEAD));
        assert_eq!(r.edge_kind(), AbEdge::Other(9));
        assert_eq!(AppBarRequest::parse(&r.encode()).unwrap(), r);
    }

    #[test]
    fn vista_encode_cuts_a_wide_shared_handle() {
        let mut r = AppBarRequest::parse(&golden_vista()).unwrap();
        r.shared_memory = 0x1_0000_0005;
        let back = AppBarRequest::parse(&r.encode()).unwrap();
        assert_eq!(back.shared_memory, 5);
    }

    #[test]
    fn huge_block_is_fine() {
        let mut b = golden_v3();
        b.extend(std::iter::repeat_n(7u8, 500_000));
        let r = AppBarRequest::parse(&b).unwrap();
        assert_eq!(r.trailing_len, 500_000);
        assert_eq!(r.message, AbMessage::SetPos);
    }
}
