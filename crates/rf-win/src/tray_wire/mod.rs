//! The wire format of the notification area: what shell32 sends to a window of class `Shell_TrayWnd` in `WM_COPYDATA`, and what a
//! tray host sends back to the icons' owners. Pure: bytes in, typed values out, and the other way round. No window, no handle, no
//! Win32 call, so everything here is tested with byte blocks.
//!
//! These bytes come from other processes (any program that calls `Shell_NotifyIcon` or `SHAppBarMessage`). Every length is checked
//! before a read, no function indexes with an offset taken from the data, and no function panics on any input (a fuzz test with a
//! fixed seed runs over all kinds). A parse failure is an `Err`, never a reason to drop a message: a relay forwards the raw bytes
//! whatever the parser says (see [`forward`]).
//!
//! Where the layouts come from: the research note `2-tray-notifications.md` (sections 1.2 and 1.4), which collects three independent
//! open implementations and Geoff Chappell's pages. **None of it has been captured on Windows 11 24H2 or 26H2 yet** (probe P1, the
//! `tray_spy` example, settles it). What is sure and what is not is stated in the docs of each type:
//!
//! * sure, from several sources and the SDK header: the signature, the `NIM_*` numbers, the `NIF_*` / `NIS_*` / `NIIF_*` / `NIN_*`
//!   values, the field order and the sizes of `NOTIFYICONDATAW` (956 bytes with 32-bit handles), the callback packing for versions
//!   0, 3 and 4;
//! * not sure: a trailing `version` word after the 964 bytes, the `dwData = 3` magic and reply packing, the padding in the
//!   `APPBARMSGDATAV3`, which of the two appbar layouts 26H2 sends, the meaning of the appbar `lParam`.
//!
//! Handles in these blocks are 32 bits wide even on 64-bit Windows (Microsoft's rule for interprocess handles): keep the raw `u32`
//! and widen it with [`sign_extend_handle`] only when calling Win32.

pub mod appbar;
pub(crate) mod bytes;
pub mod click;
pub mod dump;
pub mod forward;
pub mod notify;
pub mod rect;
pub mod service;

use std::fmt;

pub use appbar::AppBarRequest;
pub use notify::NotifyIconRequest;
pub use rect::IconRectQuery;
pub use service::ServiceObjectRequest;

/// First four bytes of every `dwData = 1` block (`0x34753423`). Named "dwUnknown" in ManagedShell, a magic in Chappell's pages.
pub const NOTIFY_SIGNATURE: u32 = 0x3475_3423;

/// `COPYDATASTRUCT.dwData` values shell32 uses toward the taskbar window.
pub const DW_APPBAR: usize = 0;
pub const DW_NOTIFY_ICON: usize = 1;
pub const DW_SERVICE_OBJECT: usize = 2;
pub const DW_ICON_RECT: usize = 3;

/// The class every tray sender looks for.
pub const TRAY_CLASS: &str = "Shell_TrayWnd";

/// Why a block was not understood. Never a reason to drop the message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    /// The block is shorter than the part being read needs.
    Truncated { what: &'static str, need: usize, have: usize },
    /// A `dwData = 1` block that does not start with [`NOTIFY_SIGNATURE`].
    BadSignature { found: u32 },
    /// A `dwData` value that is none of 0..=3.
    UnknownKind(usize),
    /// `dwMessage` of a notify-icon block that is none of `NIM_ADD..=NIM_SETVERSION`.
    UnknownNotifyMessage(u32),
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WireError::Truncated { what, need, have } => write!(f, "{what}: need {need} bytes, have {have}"),
            WireError::BadSignature { found } => write!(f, "bad signature {found:#010x} (want {NOTIFY_SIGNATURE:#010x})"),
            WireError::UnknownKind(k) => write!(f, "unknown dwData {k}"),
            WireError::UnknownNotifyMessage(m) => write!(f, "unknown NIM message {m}"),
        }
    }
}

impl std::error::Error for WireError {}

/// Which function of shell32 sent the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayKind {
    /// `SHAppBarMessage`.
    AppBar,
    /// `Shell_NotifyIcon`.
    NotifyIcon,
    /// `SHLoadInProc` / `SHEnableServiceObject`.
    ServiceObject,
    /// `Shell_NotifyIconGetRect`.
    IconRect,
}

impl TrayKind {
    pub fn from_dw_data(dw: usize) -> Option<TrayKind> {
        match dw {
            DW_APPBAR => Some(TrayKind::AppBar),
            DW_NOTIFY_ICON => Some(TrayKind::NotifyIcon),
            DW_SERVICE_OBJECT => Some(TrayKind::ServiceObject),
            DW_ICON_RECT => Some(TrayKind::IconRect),
            _ => None,
        }
    }

    pub fn dw_data(self) -> usize {
        match self {
            TrayKind::AppBar => DW_APPBAR,
            TrayKind::NotifyIcon => DW_NOTIFY_ICON,
            TrayKind::ServiceObject => DW_SERVICE_OBJECT,
            TrayKind::IconRect => DW_ICON_RECT,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TrayKind::AppBar => "appbar",
            TrayKind::NotifyIcon => "notify-icon",
            TrayKind::ServiceObject => "service-object",
            TrayKind::IconRect => "icon-rect",
        }
    }
}

/// A parsed `WM_COPYDATA` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrayRequest {
    AppBar(AppBarRequest),
    Icon(NotifyIconRequest),
    ServiceObject(ServiceObjectRequest),
    IconRect(IconRectQuery),
}

impl TrayRequest {
    pub fn kind(&self) -> TrayKind {
        match self {
            TrayRequest::AppBar(_) => TrayKind::AppBar,
            TrayRequest::Icon(_) => TrayKind::NotifyIcon,
            TrayRequest::ServiceObject(_) => TrayKind::ServiceObject,
            TrayRequest::IconRect(_) => TrayKind::IconRect,
        }
    }
}

/// Parses the block of one `WM_COPYDATA`: `dw_data` is `COPYDATASTRUCT.dwData`, `bytes` the `cbData` bytes at `lpData` (trust
/// `cbData`, not any size field inside the block). Total: returns `Err` for anything it does not understand and never panics.
pub fn parse_copydata(dw_data: usize, bytes: &[u8]) -> Result<TrayRequest, WireError> {
    match TrayKind::from_dw_data(dw_data).ok_or(WireError::UnknownKind(dw_data))? {
        TrayKind::AppBar => AppBarRequest::parse(bytes).map(TrayRequest::AppBar),
        TrayKind::NotifyIcon => NotifyIconRequest::parse(bytes).map(TrayRequest::Icon),
        TrayKind::ServiceObject => ServiceObjectRequest::parse(bytes).map(TrayRequest::ServiceObject),
        TrayKind::IconRect => IconRectQuery::parse(bytes).map(TrayRequest::IconRect),
    }
}

/// A 32-bit handle from a block, widened for Win32. Microsoft's rule for interprocess handles on 64-bit Windows is sign extension
/// (`0xFFFF_FFFF` is `-1`, not `0x0000_0000_FFFF_FFFF`); widening with `as isize` straight from a `u32` loses it.
pub fn sign_extend_handle(h: u32) -> isize {
    h as i32 as isize
}

/// The low 32 bits of a handle, the form that goes into a block.
pub fn narrow_handle(h: isize) -> u32 {
    h as u32
}

/// The icon versions a tray host serves. `NIM_SETVERSION` with any other value is refused (returns 0 to the sender), as Windows does:
/// the SDK header says "values 0, 3 and 4".
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum IconVersion {
    /// Never sent `NIM_SETVERSION` (or sent 0): the pre-Windows-2000 behaviour.
    #[default]
    V0,
    /// `NOTIFYICON_VERSION`: adds `WM_CONTEXTMENU`, `NIN_KEYSELECT`, `NIN_SELECT`.
    V3,
    /// `NOTIFYICON_VERSION_4`: adds the anchor point in `wParam` and the 16-bit icon id in `HIWORD(lParam)`.
    V4,
}

impl IconVersion {
    /// `None` for anything but 0, 3 and 4.
    pub fn from_raw(v: u32) -> Option<IconVersion> {
        match v {
            0 => Some(IconVersion::V0),
            3 => Some(IconVersion::V3),
            4 => Some(IconVersion::V4),
            _ => None,
        }
    }

    pub fn raw(self) -> u32 {
        match self {
            IconVersion::V0 => 0,
            IconVersion::V3 => 3,
            IconVersion::V4 => 4,
        }
    }

    /// Versions 3 and 4 get the keyboard and "select" messages.
    pub fn at_least_v3(self) -> bool {
        self >= IconVersion::V3
    }
}

/// A rectangle in screen coordinates (left, top, right, bottom), as in `RECT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// A point in screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// A GUID in its in-memory form (`Data1` little endian, `Data2`, `Data3`, then eight bytes), as it is in the blocks.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct WireGuid(pub [u8; 16]);

impl WireGuid {
    pub fn is_zero(&self) -> bool {
        self.0 == [0u8; 16]
    }

    pub fn from_parts(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> WireGuid {
        let mut b = [0u8; 16];
        b[0..4].copy_from_slice(&d1.to_le_bytes());
        b[4..6].copy_from_slice(&d2.to_le_bytes());
        b[6..8].copy_from_slice(&d3.to_le_bytes());
        b[8..16].copy_from_slice(&d4);
        WireGuid(b)
    }
}

impl fmt::Display for WireGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        write!(
            f,
            "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
            u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            u16::from_le_bytes([b[4], b[5]]),
            u16::from_le_bytes([b[6], b[7]]),
            b[8],
            b[9],
            b[10],
            b[11],
            b[12],
            b[13],
            b[14],
            b[15]
        )
    }
}

impl fmt::Debug for WireGuid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// How an icon is told apart from the others: by GUID when the sender gave one (`NIF_GUID` and a non-zero GUID; `uID` is ignored
/// then), else by window and id. The window is kept as the raw 32 bits of the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IconKey {
    Guid(WireGuid),
    Wnd { hwnd: u32, uid: u32 },
}

/// Shared by the tests of this module family: a deterministic random source (xorshift64*), so no crate is needed and a failing run can
/// be repeated.
#[cfg(test)]
pub(crate) struct XorShift(pub u64);

#[cfg(test)]
impl XorShift {
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_sign_extend_and_narrow() {
        assert_eq!(sign_extend_handle(0), 0);
        assert_eq!(sign_extend_handle(0x0001_0002), 0x0001_0002);
        assert_eq!(sign_extend_handle(0x7FFF_FFFF), 0x7FFF_FFFF);
        assert_eq!(sign_extend_handle(0x8000_0000), -0x8000_0000);
        assert_eq!(sign_extend_handle(0xFFFF_FFFF), -1);
        for h in [0u32, 1, 0x1234_5678, 0x8000_0001, 0xFFFF_FFFF] {
            assert_eq!(narrow_handle(sign_extend_handle(h)), h);
        }
        // a real 64-bit value keeps only its low 32 bits
        assert_eq!(narrow_handle(0x1_0000_0042), 0x42);
    }

    #[test]
    fn kinds_round_trip() {
        for k in [TrayKind::AppBar, TrayKind::NotifyIcon, TrayKind::ServiceObject, TrayKind::IconRect] {
            assert_eq!(TrayKind::from_dw_data(k.dw_data()), Some(k));
            assert!(!k.name().is_empty());
        }
        assert_eq!(TrayKind::from_dw_data(4), None);
        assert_eq!(TrayKind::from_dw_data(usize::MAX), None);
    }

    #[test]
    fn unknown_kind_is_an_error_not_a_panic() {
        assert_eq!(parse_copydata(4, &[0; 100]), Err(WireError::UnknownKind(4)));
        assert_eq!(parse_copydata(usize::MAX, &[]), Err(WireError::UnknownKind(usize::MAX)));
    }

    #[test]
    fn versions_are_0_3_4_only() {
        assert_eq!(IconVersion::from_raw(0), Some(IconVersion::V0));
        assert_eq!(IconVersion::from_raw(3), Some(IconVersion::V3));
        assert_eq!(IconVersion::from_raw(4), Some(IconVersion::V4));
        for bad in [1, 2, 5, 6, 0x100, u32::MAX] {
            assert_eq!(IconVersion::from_raw(bad), None, "{bad}");
        }
        assert!(!IconVersion::V0.at_least_v3());
        assert!(IconVersion::V3.at_least_v3());
        assert!(IconVersion::V4.at_least_v3());
        assert_eq!(IconVersion::default(), IconVersion::V0);
        for v in [IconVersion::V0, IconVersion::V3, IconVersion::V4] {
            assert_eq!(IconVersion::from_raw(v.raw()), Some(v));
        }
    }

    #[test]
    fn guid_displays_in_registry_form() {
        let g = WireGuid::from_parts(0x7820AE73, 0x23E3, 0x4229, [0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]);
        assert_eq!(g.to_string(), "{7820AE73-23E3-4229-82C1-E41CB67D5B9C}");
        assert_eq!(&g.0[0..4], &[0x73, 0xAE, 0x20, 0x78], "Data1 is little endian in memory");
        assert!(!g.is_zero());
        assert!(WireGuid::default().is_zero());
        assert_eq!(WireGuid::default().to_string(), "{00000000-0000-0000-0000-000000000000}");
    }

    /// The fuzz loop over every kind. Fixed seed, so a failure repeats. Random blocks of random size, plus blocks that start with the
    /// right signature and message so the parsers get past their first check, plus mutations of valid blocks.
    #[test]
    fn fuzz_never_panics() {
        let mut rng = XorShift(0x9E37_79B9_7F4A_7C15);
        let valid_notify = notify::tests_support::golden_full();
        let valid_appbar = appbar::tests_support::golden_v3();
        let valid_service = service::tests_support::golden();
        let valid_rect = rect::tests_support::golden();
        for round in 0..40_000u32 {
            let dw = rng.below(6) as usize;
            let mut b = match round % 4 {
                0 => {
                    let n = rng.below(1200) as usize;
                    rng.bytes(n)
                }
                1 => {
                    // signature + a message number, then noise
                    let n = rng.below(1100) as usize;
                    let mut v = NOTIFY_SIGNATURE.to_le_bytes().to_vec();
                    v.extend_from_slice(&(rng.below(7) as u32).to_le_bytes());
                    v.extend(rng.bytes(n));
                    v
                }
                2 => {
                    // a valid block of some kind, cut to a random length
                    let src = match rng.below(4) {
                        0 => &valid_notify,
                        1 => &valid_appbar,
                        2 => &valid_service,
                        _ => &valid_rect,
                    };
                    let cut = rng.below(src.len() as u64 + 8) as usize;
                    src[..cut.min(src.len())].to_vec()
                }
                _ => {
                    // a valid block with a few random bytes changed
                    let mut v = match rng.below(4) {
                        0 => valid_notify.clone(),
                        1 => valid_appbar.clone(),
                        2 => valid_service.clone(),
                        _ => valid_rect.clone(),
                    };
                    for _ in 0..1 + rng.below(8) {
                        let i = rng.below(v.len() as u64) as usize;
                        v[i] = rng.next() as u8;
                    }
                    v
                }
            };
            // sometimes a very large block
            if round % 4000 == 0 {
                b.extend(rng.bytes(200_000));
            }
            if let Ok(req) = parse_copydata(dw, &b) {
                // invariants of an accepted block
                match req {
                    TrayRequest::Icon(i) => {
                        assert!(i.data.tip.encode_utf16().count() <= 127);
                        assert!(i.data.info.encode_utf16().count() <= 255);
                        assert!(i.data.info_title.encode_utf16().count() <= 63);
                    }
                    TrayRequest::AppBar(a) => assert!(a.trailing_len <= b.len()),
                    _ => {}
                }
            }
        }
    }
}
