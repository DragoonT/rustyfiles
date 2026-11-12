//! `dwData = 1`: `Shell_NotifyIcon`. The block is shell32's `TRAYNOTIFYDATAW` (Chappell's name):
//!
//! ```text
//! 0x000  4    signature 0x34753423
//! 0x004  4    dwMessage  NIM_ADD 0, NIM_MODIFY 1, NIM_DELETE 2, NIM_SETFOCUS 3, NIM_SETVERSION 4
//! 0x008  956  NOTIFYICONDATAW with 32-bit handles (offsets below are relative to 0x008):
//!   +0x000  4   cbSize           (not trusted: shell32 rewrites it; the block's length is what counts)
//!   +0x004  4   hWnd             (low 32 bits)
//!   +0x008  4   uID
//!   +0x00C  4   uFlags           NIF_*
//!   +0x010  4   uCallbackMessage
//!   +0x014  4   hIcon            (low 32 bits)
//!   +0x018  256 szTip[128]       UTF-16
//!   +0x118  4   dwState
//!   +0x11C  4   dwStateMask
//!   +0x120  512 szInfo[256]      UTF-16
//!   +0x320  4   uTimeout / uVersion (a union)
//!   +0x324  128 szInfoTitle[64]  UTF-16
//!   +0x3A4  4   dwInfoFlags      NIIF_*
//!   +0x3A8  16  guidItem
//!   +0x3B8  4   hBalloonIcon     (low 32 bits)
//! 0x3C4  (4)  a trailing word that one implementation names `version`; whether it exists on 26H2 is UNVERIFIED
//! ```
//!
//! shell32 builds the block in the sender's process, always in the newest layout, whichever `NOTIFYICONDATA` size the program
//! passed; a 32-bit and a 64-bit sender therefore send the same bytes (consistent with three implementations that read both with one
//! struct; UNVERIFIED by a direct capture). The inner `cbSize` may therefore say nothing about the sender (one source says shell32
//! rewrites it; whether it keeps the sender's own size is exactly what the first spy log shows, see
//! [`NotifyIconData::sender_layouts`], which reports what the field implies and no more). What the sender really passed shows in
//! which `NIF_*` flags it set: **a field is read only when its flag is set**, the rest may be
//! stale. The block can be shorter than the newest layout (an old sender, a hostile one): fields that are not wholly inside are
//! zero / empty, and flags that need them are ignored (see [`DataTier`]).

use super::bytes::{array16_at, set_u32, set_wide_field, u32_at, wide_field};
use super::{IconKey, IconVersion, WireError, WireGuid, NOTIFY_SIGNATURE, sign_extend_handle};

/// `dwMessage` of the block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Nim {
    Add,
    Modify,
    Delete,
    SetFocus,
    SetVersion,
}

impl Nim {
    pub fn from_u32(v: u32) -> Option<Nim> {
        match v {
            0 => Some(Nim::Add),
            1 => Some(Nim::Modify),
            2 => Some(Nim::Delete),
            3 => Some(Nim::SetFocus),
            4 => Some(Nim::SetVersion),
            _ => None,
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            Nim::Add => 0,
            Nim::Modify => 1,
            Nim::Delete => 2,
            Nim::SetFocus => 3,
            Nim::SetVersion => 4,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Nim::Add => "NIM_ADD",
            Nim::Modify => "NIM_MODIFY",
            Nim::Delete => "NIM_DELETE",
            Nim::SetFocus => "NIM_SETFOCUS",
            Nim::SetVersion => "NIM_SETVERSION",
        }
    }
}

/// `uFlags`.
pub mod nif {
    pub const MESSAGE: u32 = 0x01;
    pub const ICON: u32 = 0x02;
    pub const TIP: u32 = 0x04;
    pub const STATE: u32 = 0x08;
    pub const INFO: u32 = 0x10;
    pub const GUID: u32 = 0x20;
    pub const REALTIME: u32 = 0x40;
    pub const SHOWTIP: u32 = 0x80;
}

/// `dwState` / `dwStateMask`.
pub mod nis {
    pub const HIDDEN: u32 = 1;
    pub const SHAREDICON: u32 = 2;
}

/// `dwInfoFlags`.
pub mod niif {
    pub const NONE: u32 = 0;
    pub const INFO: u32 = 1;
    pub const WARNING: u32 = 2;
    pub const ERROR: u32 = 3;
    pub const USER: u32 = 4;
    pub const ICON_MASK: u32 = 0x0F;
    pub const NOSOUND: u32 = 0x10;
    pub const LARGE_ICON: u32 = 0x20;
    pub const RESPECT_QUIET_TIME: u32 = 0x80;
}

/// Field offsets inside the `NOTIFYICONDATAW` part (the block minus its 8-byte header).
pub mod off {
    pub const CB_SIZE: usize = 0x000;
    pub const HWND: usize = 0x004;
    pub const UID: usize = 0x008;
    pub const FLAGS: usize = 0x00C;
    pub const CALLBACK: usize = 0x010;
    pub const ICON: usize = 0x014;
    pub const TIP: usize = 0x018;
    pub const STATE: usize = 0x118;
    pub const STATE_MASK: usize = 0x11C;
    pub const INFO: usize = 0x120;
    pub const TIMEOUT_VERSION: usize = 0x320;
    pub const INFO_TITLE: usize = 0x324;
    pub const INFO_FLAGS: usize = 0x3A4;
    pub const GUID: usize = 0x3A8;
    pub const BALLOON_ICON: usize = 0x3B8;
    /// Size of the whole `NOTIFYICONDATAW` part (956).
    pub const END: usize = 0x3BC;
}

pub const TIP_UNITS: usize = 128;
pub const INFO_UNITS: usize = 256;
pub const TITLE_UNITS: usize = 64;

/// The two words before the `NOTIFYICONDATAW` part.
pub const HEADER_LEN: usize = 8;
/// The block as shell32 builds it (without the doubtful trailing word): 8 + 956.
pub const PAYLOAD_LEN: usize = HEADER_LEN + off::END;
/// Smallest block that is still parsed: the header and the fields up to `hIcon`.
pub const MIN_PAYLOAD_LEN: usize = HEADER_LEN + off::TIP;

/// How much of `NOTIFYICONDATAW` a block carries; decides which fields exist. Ordered from least to most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DataTier {
    /// Up to `hIcon` only (0x18 bytes): the least that is parsed.
    Core,
    /// Through a 64-unit `szTip` (0x98): the Windows 95 layout. The tip is read from whatever the block holds, up to 128 units.
    V1,
    /// Through `dwInfoFlags` (0x3A8): state and balloon fields exist.
    V2,
    /// Through `guidItem` (0x3B8): `NIF_GUID` is honoured.
    V3,
    /// Through `hBalloonIcon` (0x3BC): the whole structure.
    Full,
}

impl DataTier {
    /// The tier of a `NOTIFYICONDATAW` part of `n` bytes; `None` when it is shorter than [`DataTier::Core`].
    pub fn from_len(n: usize) -> Option<DataTier> {
        if n >= off::END {
            Some(DataTier::Full)
        } else if n >= off::BALLOON_ICON {
            Some(DataTier::V3)
        } else if n >= off::INFO_FLAGS + 4 {
            Some(DataTier::V2)
        } else if n >= off::TIP + 64 * 2 {
            Some(DataTier::V1)
        } else if n >= off::TIP {
            Some(DataTier::Core)
        } else {
            None
        }
    }

    /// The `NIF_*` bits that make sense for this much data. `NIF_REALTIME` and `NIF_SHOWTIP` carry no field and always pass.
    pub fn allowed_flags(self) -> u32 {
        let mut f = nif::MESSAGE | nif::ICON | nif::TIP | nif::REALTIME | nif::SHOWTIP;
        if self >= DataTier::V2 {
            f |= nif::STATE | nif::INFO;
        }
        if self >= DataTier::V3 {
            f |= nif::GUID;
        }
        f
    }
}

/// What the inner `cbSize` implies the sender compiled against. Informational only, and possibly no information at all: the research
/// note says shell32 rewrites the field; the length of the block is what counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SenderLayout {
    /// 32 or 64: the pointer width of the sender's `NOTIFYICONDATAW`.
    pub bits: u8,
    pub variant: SenderVariant,
}

/// Which `NOTIFYICONDATAW_V*_SIZE` the sender used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderVariant {
    /// `NOTIFYICONDATAW_V1_SIZE`: through `szTip[64]`.
    V1,
    /// `NOTIFYICONDATAW_V2_SIZE`: through `dwInfoFlags`.
    V2,
    /// `NOTIFYICONDATAW_V3_SIZE`: through `guidItem`.
    V3,
    /// `sizeof(NOTIFYICONDATAW)`: with `hBalloonIcon`.
    Full,
}

/// The `uTimeout / uVersion` union, plus the flags, read out of a [`NotifyIconData`] for an icon with `NIF_INFO`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BalloonRequest {
    pub title: String,
    pub text: String,
    /// `dwInfoFlags` as sent.
    pub info_flags: u32,
    pub kind: BalloonKind,
    pub no_sound: bool,
    pub large_icon: bool,
    pub respect_quiet_time: bool,
    /// `NIF_REALTIME`: drop the balloon if it cannot be shown now.
    pub realtime: bool,
    /// `hBalloonIcon` (sign-extended) when it is present and non-zero. For `NIIF_USER` the shell falls back to the main icon when this
    /// is `None` (Chappell).
    pub user_icon: Option<isize>,
    /// The union as a timeout in milliseconds. Ignored since Vista; the host clamps its own display time.
    pub timeout_ms: u32,
}

impl BalloonRequest {
    /// `NIF_INFO` with an empty text removes the balloon on show.
    pub fn is_removal(&self) -> bool {
        self.text.is_empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalloonKind {
    None,
    Info,
    Warning,
    Error,
    /// `NIIF_USER`: the icon is `hBalloonIcon` (else `hIcon`).
    User,
    /// An `NIIF_*` icon value the header does not define.
    Other(u32),
}

/// The `(dwState, dwStateMask)` pair of an icon with `NIF_STATE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateUpdate {
    pub state: u32,
    pub mask: u32,
}

impl StateUpdate {
    /// `Some(hidden)` when the mask covers `NIS_HIDDEN`.
    pub fn hidden(&self) -> Option<bool> {
        (self.mask & nis::HIDDEN != 0).then_some(self.state & nis::HIDDEN != 0)
    }

    /// `Some(shared)` when the mask covers `NIS_SHAREDICON`.
    pub fn shared_icon(&self) -> Option<bool> {
        (self.mask & nis::SHAREDICON != 0).then_some(self.state & nis::SHAREDICON != 0)
    }

    /// The stored state after this update: only the bits in the mask change.
    pub fn merge(&self, stored: u32) -> u32 {
        (stored & !self.mask) | (self.state & self.mask)
    }
}

/// The `NOTIFYICONDATAW` part, every field as sent (handles as the raw 32 bits). Use the accessor methods: they apply the flags and the
/// tier, the plain fields do not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyIconData {
    /// The inner `cbSize` as sent (not trusted).
    pub cb_size: u32,
    pub hwnd: u32,
    pub uid: u32,
    pub flags: u32,
    pub callback_message: u32,
    pub icon: u32,
    /// At most 127 units, up to the first NUL.
    pub tip: String,
    pub state: u32,
    pub state_mask: u32,
    /// At most 255 units.
    pub info: String,
    /// `uTimeout` for a balloon, `uVersion` for `NIM_SETVERSION`: one field, two meanings.
    pub timeout_or_version: u32,
    /// At most 63 units.
    pub info_title: String,
    pub info_flags: u32,
    pub guid: WireGuid,
    pub balloon_icon: u32,
    pub tier: DataTier,
}

impl NotifyIconData {
    /// Parses the `NOTIFYICONDATAW` part. Fields not wholly inside `data` (or beyond its [`DataTier`]) are zero / empty.
    pub fn parse(data: &[u8]) -> Result<NotifyIconData, WireError> {
        let tier = DataTier::from_len(data.len()).ok_or(WireError::Truncated {
            what: "NOTIFYICONDATAW",
            need: off::TIP,
            have: data.len(),
        })?;
        // only the structure itself is read; whatever follows is somebody else's
        let data = &data[..data.len().min(off::END)];
        let scalar = |at: usize, min: DataTier| if tier >= min { u32_at(data, at).unwrap_or(0) } else { 0 };
        // the tip exists in every tier; the rest of the text fields only from V2 on
        let tip = wide_field(data, off::TIP, TIP_UNITS);
        let (info, info_title) = if tier >= DataTier::V2 {
            (wide_field(data, off::INFO, INFO_UNITS), wide_field(data, off::INFO_TITLE, TITLE_UNITS))
        } else {
            (String::new(), String::new())
        };
        let guid = if tier >= DataTier::V3 { array16_at(data, off::GUID).map(WireGuid).unwrap_or_default() } else { WireGuid::default() };
        Ok(NotifyIconData {
            cb_size: scalar(off::CB_SIZE, DataTier::Core),
            hwnd: scalar(off::HWND, DataTier::Core),
            uid: scalar(off::UID, DataTier::Core),
            flags: scalar(off::FLAGS, DataTier::Core),
            callback_message: scalar(off::CALLBACK, DataTier::Core),
            icon: scalar(off::ICON, DataTier::Core),
            tip,
            state: scalar(off::STATE, DataTier::V2),
            state_mask: scalar(off::STATE_MASK, DataTier::V2),
            info,
            timeout_or_version: scalar(off::TIMEOUT_VERSION, DataTier::V2),
            info_title,
            info_flags: scalar(off::INFO_FLAGS, DataTier::V2),
            guid,
            balloon_icon: scalar(off::BALLOON_ICON, DataTier::Full),
            tier,
        })
    }

    /// The 956-byte `NOTIFYICONDATAW` part, every field written (strings cut to fit and NUL-terminated).
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; off::END];
        set_u32(&mut b, off::CB_SIZE, self.cb_size);
        set_u32(&mut b, off::HWND, self.hwnd);
        set_u32(&mut b, off::UID, self.uid);
        set_u32(&mut b, off::FLAGS, self.flags);
        set_u32(&mut b, off::CALLBACK, self.callback_message);
        set_u32(&mut b, off::ICON, self.icon);
        set_wide_field(&mut b, off::TIP, TIP_UNITS, &self.tip);
        set_u32(&mut b, off::STATE, self.state);
        set_u32(&mut b, off::STATE_MASK, self.state_mask);
        set_wide_field(&mut b, off::INFO, INFO_UNITS, &self.info);
        set_u32(&mut b, off::TIMEOUT_VERSION, self.timeout_or_version);
        set_wide_field(&mut b, off::INFO_TITLE, TITLE_UNITS, &self.info_title);
        set_u32(&mut b, off::INFO_FLAGS, self.info_flags);
        b[off::GUID..off::GUID + 16].copy_from_slice(&self.guid.0);
        set_u32(&mut b, off::BALLOON_ICON, self.balloon_icon);
        b
    }

    /// `uFlags` with the bits this much data cannot back removed.
    pub fn effective_flags(&self) -> u32 {
        self.flags & self.tier.allowed_flags()
    }

    fn has(&self, flag: u32) -> bool {
        self.effective_flags() & flag != 0
    }

    /// The window as a Win32 handle (sign-extended).
    pub fn hwnd_handle(&self) -> isize {
        sign_extend_handle(self.hwnd)
    }

    /// The key that identifies the icon: the GUID when `NIF_GUID` is set and the GUID is not zero, else window and id.
    pub fn key(&self) -> IconKey {
        if self.has(nif::GUID) && !self.guid.is_zero() {
            IconKey::Guid(self.guid)
        } else {
            IconKey::Wnd { hwnd: self.hwnd, uid: self.uid }
        }
    }

    /// `uCallbackMessage`, when `NIF_MESSAGE` is set.
    pub fn callback(&self) -> Option<u32> {
        self.has(nif::MESSAGE).then_some(self.callback_message)
    }

    /// The icon handle (sign-extended), when `NIF_ICON` is set; `Some(0)` means "clear the icon". The sender may destroy the icon as
    /// soon as the call returns: copy the pixels inside the `WM_COPYDATA` handler.
    pub fn icon_handle(&self) -> Option<isize> {
        self.has(nif::ICON).then(|| sign_extend_handle(self.icon))
    }

    /// The tip text, when `NIF_TIP` is set.
    pub fn tip_text(&self) -> Option<&str> {
        self.has(nif::TIP).then_some(self.tip.as_str())
    }

    /// `(dwState, dwStateMask)`, when `NIF_STATE` is set.
    pub fn state_update(&self) -> Option<StateUpdate> {
        self.has(nif::STATE).then_some(StateUpdate { state: self.state, mask: self.state_mask })
    }

    /// The balloon, when `NIF_INFO` is set.
    pub fn balloon(&self) -> Option<BalloonRequest> {
        if !self.has(nif::INFO) {
            return None;
        }
        let kind = match self.info_flags & niif::ICON_MASK {
            niif::NONE => BalloonKind::None,
            niif::INFO => BalloonKind::Info,
            niif::WARNING => BalloonKind::Warning,
            niif::ERROR => BalloonKind::Error,
            niif::USER => BalloonKind::User,
            other => BalloonKind::Other(other),
        };
        Some(BalloonRequest {
            title: self.info_title.clone(),
            text: self.info.clone(),
            info_flags: self.info_flags,
            kind,
            no_sound: self.info_flags & niif::NOSOUND != 0,
            large_icon: self.info_flags & niif::LARGE_ICON != 0,
            respect_quiet_time: self.info_flags & niif::RESPECT_QUIET_TIME != 0,
            realtime: self.flags & nif::REALTIME != 0,
            user_icon: (self.tier >= DataTier::Full && self.balloon_icon != 0).then(|| sign_extend_handle(self.balloon_icon)),
            timeout_ms: self.timeout_or_version,
        })
    }

    /// The requested version of a `NIM_SETVERSION` block (the union word). `None` for anything but 0, 3 and 4, and `None` when the
    /// block ends before the word (an absent word is not a request for version 0). Read it only for `NIM_SETVERSION`: in every other
    /// message the word is a balloon timeout.
    pub fn requested_version(&self) -> Option<IconVersion> {
        if self.tier < DataTier::V2 {
            return None;
        }
        IconVersion::from_raw(self.timeout_or_version)
    }

    /// What the inner `cbSize` IMPLIES about the sender (see [`SenderLayout`]): the layouts whose `NOTIFYICONDATAW_V*_SIZE` equals it.
    /// This is a guess, not a fact: the research note says shell32 repackages the block into the newest layout and that the field must
    /// not be trusted, so if it is rewritten every block reads as 32-bit `Full` whoever sent it. The first spy log decides.
    /// Empty for a size that matches none; two entries for 952 (a 32-bit V3 and a 64-bit V2 are the same size). The 32-bit sizes are
    /// the SDK header's (152, 936, 952, 956); the 64-bit ones are my arithmetic from the same header (168, 952, 968, 976) and only the
    /// last is checked against `size_of`.
    pub fn sender_layouts(&self) -> Vec<SenderLayout> {
        const TABLE: [(u32, u8, SenderVariant); 8] = [
            (152, 32, SenderVariant::V1),
            (936, 32, SenderVariant::V2),
            (952, 32, SenderVariant::V3),
            (956, 32, SenderVariant::Full),
            (168, 64, SenderVariant::V1),
            (952, 64, SenderVariant::V2),
            (968, 64, SenderVariant::V3),
            (976, 64, SenderVariant::Full),
        ];
        TABLE.iter().filter(|(cb, _, _)| *cb == self.cb_size).map(|&(_, bits, variant)| SenderLayout { bits, variant }).collect()
    }
}

/// One `dwData = 1` block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotifyIconRequest {
    pub message: Nim,
    pub data: NotifyIconData,
    /// Bytes after the 964 (zero for a block of exactly [`PAYLOAD_LEN`]).
    pub trailing_len: usize,
    /// The first word after the 964 bytes, when there are four: the `version` one implementation names there. UNVERIFIED.
    pub trailing_word: Option<u32>,
}

impl NotifyIconRequest {
    /// Parses a whole `dwData = 1` block (`cbData` bytes). Needs at least [`MIN_PAYLOAD_LEN`] (32) bytes; checks the signature and that
    /// `dwMessage` is one of the five `NIM_*` values.
    pub fn parse(payload: &[u8]) -> Result<NotifyIconRequest, WireError> {
        let sig = u32_at(payload, 0).ok_or(WireError::Truncated { what: "tray notify header", need: HEADER_LEN, have: payload.len() })?;
        let msg = u32_at(payload, 4).ok_or(WireError::Truncated { what: "tray notify header", need: HEADER_LEN, have: payload.len() })?;
        if sig != NOTIFY_SIGNATURE {
            return Err(WireError::BadSignature { found: sig });
        }
        let message = Nim::from_u32(msg).ok_or(WireError::UnknownNotifyMessage(msg))?;
        if payload.len() < MIN_PAYLOAD_LEN {
            return Err(WireError::Truncated { what: "tray notify data", need: MIN_PAYLOAD_LEN, have: payload.len() });
        }
        let data = NotifyIconData::parse(&payload[HEADER_LEN..])?;
        let trailing_len = payload.len().saturating_sub(PAYLOAD_LEN);
        let trailing_word = if trailing_len >= 4 { u32_at(payload, PAYLOAD_LEN) } else { None };
        Ok(NotifyIconRequest { message, data, trailing_len, trailing_word })
    }

    /// The block as shell32 builds it: signature, message, the 956 bytes, and the trailing word when one was parsed.
    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; HEADER_LEN];
        set_u32(&mut b, 0, NOTIFY_SIGNATURE);
        set_u32(&mut b, 4, self.message.as_u32());
        b.extend_from_slice(&self.data.encode());
        if let Some(w) = self.trailing_word {
            b.extend_from_slice(&w.to_le_bytes());
        }
        b
    }
}

/// Human-readable list of the `NIF_*` bits in `flags` (`MESSAGE|ICON|TIP`), for logs.
pub fn nif_names(flags: u32) -> String {
    let names = [
        (nif::MESSAGE, "MESSAGE"),
        (nif::ICON, "ICON"),
        (nif::TIP, "TIP"),
        (nif::STATE, "STATE"),
        (nif::INFO, "INFO"),
        (nif::GUID, "GUID"),
        (nif::REALTIME, "REALTIME"),
        (nif::SHOWTIP, "SHOWTIP"),
    ];
    let mut out: Vec<String> = names.iter().filter(|(b, _)| flags & b != 0).map(|(_, n)| (*n).to_string()).collect();
    let unknown = flags & !0xFF;
    if unknown != 0 {
        out.push(format!("{unknown:#x}"));
    }
    if out.is_empty() {
        "-".into()
    } else {
        out.join("|")
    }
}

/// Golden byte blocks for tests, written with absolute literal offsets of the block (header 8 + the offsets of the note), on purpose
/// not through the constants above, so that a wrong constant is caught.
#[cfg(test)]
pub(crate) mod tests_support {
    pub fn put32(b: &mut [u8], at: usize, v: u32) {
        b[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }

    pub fn put_wide(b: &mut [u8], at: usize, s: &str) {
        for (i, u) in s.encode_utf16().enumerate() {
            b[at + i * 2..at + i * 2 + 2].copy_from_slice(&u.to_le_bytes());
        }
    }

    /// A full 964-byte block (no trailing word) with `NIM_ADD`, all flags, and distinctive values in every field.
    pub fn golden_full() -> Vec<u8> {
        let mut b = vec![0u8; 964];
        put32(&mut b, 0x000, 0x3475_3423);
        put32(&mut b, 0x004, 0); // NIM_ADD
        put32(&mut b, 0x008, 956); // cbSize
        put32(&mut b, 0x00C, 0x0001_0ABC); // hWnd
        put32(&mut b, 0x010, 7); // uID
        put32(&mut b, 0x014, 0x3F); // uFlags: MESSAGE|ICON|TIP|STATE|INFO|GUID
        put32(&mut b, 0x018, 0x8000 + 5); // uCallbackMessage
        put32(&mut b, 0x01C, 0xFFFF_FF00); // hIcon (negative when sign-extended)
        put_wide(&mut b, 0x020, "Tip text");
        put32(&mut b, 0x120, 1); // dwState NIS_HIDDEN
        put32(&mut b, 0x124, 3); // dwStateMask
        put_wide(&mut b, 0x128, "Balloon body");
        put32(&mut b, 0x328, 5000); // uTimeout
        put_wide(&mut b, 0x32C, "Balloon title");
        put32(&mut b, 0x3AC, 1 | 0x10); // dwInfoFlags NIIF_INFO|NOSOUND
        b[0x3B0..0x3C0].copy_from_slice(&[0x73, 0xAE, 0x20, 0x78, 0xE3, 0x23, 0x29, 0x42, 0x82, 0xC1, 0xE4, 0x1C, 0xB6, 0x7D, 0x5B, 0x9C]);
        put32(&mut b, 0x3C0, 0x0002_0003); // hBalloonIcon
        b
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::tray_wire::XorShift;

    fn full() -> NotifyIconRequest {
        NotifyIconRequest::parse(&golden_full()).expect("golden block parses")
    }

    #[test]
    fn constants_match_the_sdk_header() {
        use windows::Win32::UI::Shell as sh;
        assert_eq!(nif::MESSAGE, sh::NIF_MESSAGE.0);
        assert_eq!(nif::ICON, sh::NIF_ICON.0);
        assert_eq!(nif::TIP, sh::NIF_TIP.0);
        assert_eq!(nif::STATE, sh::NIF_STATE.0);
        assert_eq!(nif::INFO, sh::NIF_INFO.0);
        assert_eq!(nif::GUID, sh::NIF_GUID.0);
        assert_eq!(nif::REALTIME, sh::NIF_REALTIME.0);
        assert_eq!(nif::SHOWTIP, sh::NIF_SHOWTIP.0);
        assert_eq!(nis::HIDDEN, sh::NIS_HIDDEN.0);
        assert_eq!(nis::SHAREDICON, sh::NIS_SHAREDICON.0);
        assert_eq!(niif::INFO, sh::NIIF_INFO.0);
        assert_eq!(niif::WARNING, sh::NIIF_WARNING.0);
        assert_eq!(niif::ERROR, sh::NIIF_ERROR.0);
        assert_eq!(niif::USER, sh::NIIF_USER.0);
        assert_eq!(niif::ICON_MASK, sh::NIIF_ICON_MASK.0);
        assert_eq!(niif::NOSOUND, sh::NIIF_NOSOUND.0);
        assert_eq!(niif::LARGE_ICON, sh::NIIF_LARGE_ICON.0);
        assert_eq!(niif::RESPECT_QUIET_TIME, sh::NIIF_RESPECT_QUIET_TIME.0);
        assert_eq!(Nim::Add.as_u32(), sh::NIM_ADD.0);
        assert_eq!(Nim::Modify.as_u32(), sh::NIM_MODIFY.0);
        assert_eq!(Nim::Delete.as_u32(), sh::NIM_DELETE.0);
        assert_eq!(Nim::SetFocus.as_u32(), sh::NIM_SETFOCUS.0);
        assert_eq!(Nim::SetVersion.as_u32(), sh::NIM_SETVERSION.0);
    }

    #[test]
    fn layout_sizes_match_the_real_structure() {
        // the 32-bit handle layout is not the native one on this (64-bit) build, but its pieces are checkable:
        assert_eq!(off::TIP + TIP_UNITS * 2, off::STATE);
        assert_eq!(off::INFO + INFO_UNITS * 2, off::TIMEOUT_VERSION);
        assert_eq!(off::INFO_TITLE + TITLE_UNITS * 2, off::INFO_FLAGS);
        assert_eq!(off::GUID + 16, off::BALLOON_ICON);
        assert_eq!(off::BALLOON_ICON + 4, off::END);
        assert_eq!(off::END, 956);
        assert_eq!(PAYLOAD_LEN, 964);
        // and against the 64-bit native struct: strings and the fixed words keep their sizes
        assert_eq!(std::mem::size_of::<windows::Win32::UI::Shell::NOTIFYICONDATAW>(), 976);
    }

    #[test]
    fn golden_block_every_field() {
        let r = full();
        assert_eq!(r.message, Nim::Add);
        assert_eq!(r.trailing_len, 0);
        assert_eq!(r.trailing_word, None);
        let d = &r.data;
        assert_eq!(d.tier, DataTier::Full);
        assert_eq!(d.cb_size, 956);
        assert_eq!(d.hwnd, 0x0001_0ABC);
        assert_eq!(d.uid, 7);
        assert_eq!(d.flags, 0x3F);
        assert_eq!(d.callback_message, 0x8005);
        assert_eq!(d.icon, 0xFFFF_FF00);
        assert_eq!(d.tip, "Tip text");
        assert_eq!(d.state, 1);
        assert_eq!(d.state_mask, 3);
        assert_eq!(d.info, "Balloon body");
        assert_eq!(d.timeout_or_version, 5000);
        assert_eq!(d.info_title, "Balloon title");
        assert_eq!(d.info_flags, 0x11);
        assert_eq!(d.guid.to_string(), "{7820AE73-23E3-4229-82C1-E41CB67D5B9C}");
        assert_eq!(d.balloon_icon, 0x0002_0003);
    }

    #[test]
    fn accessors_apply_the_flags() {
        let r = full();
        let d = &r.data;
        assert_eq!(d.hwnd_handle(), 0x0001_0ABC);
        assert_eq!(d.callback(), Some(0x8005));
        assert_eq!(d.icon_handle(), Some(-256), "hIcon is sign-extended");
        assert_eq!(d.tip_text(), Some("Tip text"));
        let s = d.state_update().unwrap();
        assert_eq!(s.hidden(), Some(true));
        assert_eq!(s.shared_icon(), Some(false));
        let b = d.balloon().unwrap();
        assert_eq!(b.title, "Balloon title");
        assert_eq!(b.text, "Balloon body");
        assert_eq!(b.kind, BalloonKind::Info);
        assert!(b.no_sound && !b.large_icon && !b.respect_quiet_time && !b.realtime);
        assert_eq!(b.user_icon, Some(0x0002_0003));
        assert_eq!(b.timeout_ms, 5000);
        assert!(!b.is_removal());
        assert_eq!(d.key(), IconKey::Guid(d.guid));

        // with no flags set every accessor is empty, whatever the stale fields hold
        let mut raw = golden_full();
        put32(&mut raw, 0x014, 0);
        let r = NotifyIconRequest::parse(&raw).unwrap();
        let d = &r.data;
        assert_eq!(d.callback(), None);
        assert_eq!(d.icon_handle(), None);
        assert_eq!(d.tip_text(), None);
        assert_eq!(d.state_update(), None);
        assert_eq!(d.balloon(), None);
        assert_eq!(d.key(), IconKey::Wnd { hwnd: 0x0001_0ABC, uid: 7 });
    }

    #[test]
    fn single_flags_each_enable_only_their_field() {
        for (flag, which) in [(nif::MESSAGE, 0), (nif::ICON, 1), (nif::TIP, 2), (nif::STATE, 3), (nif::INFO, 4), (nif::GUID, 5)] {
            let mut raw = golden_full();
            put32(&mut raw, 0x014, flag);
            let d = NotifyIconRequest::parse(&raw).unwrap().data;
            let present = [
                d.callback().is_some(),
                d.icon_handle().is_some(),
                d.tip_text().is_some(),
                d.state_update().is_some(),
                d.balloon().is_some(),
                matches!(d.key(), IconKey::Guid(_)),
            ];
            for (i, p) in present.iter().enumerate() {
                assert_eq!(*p, i == which, "flag {flag:#x}, field {i}");
            }
        }
    }

    #[test]
    fn nif_guid_with_a_zero_guid_keys_by_window() {
        let mut raw = golden_full();
        raw[0x3B0..0x3C0].fill(0);
        let d = NotifyIconRequest::parse(&raw).unwrap().data;
        assert_eq!(d.key(), IconKey::Wnd { hwnd: 0x0001_0ABC, uid: 7 });
    }

    #[test]
    fn icon_clear_and_removal_balloon() {
        let mut raw = golden_full();
        put32(&mut raw, 0x01C, 0); // hIcon = 0
        put_wide(&mut raw, 0x128, "");
        raw[0x128..0x128 + 24].fill(0);
        let d = NotifyIconRequest::parse(&raw).unwrap().data;
        assert_eq!(d.icon_handle(), Some(0));
        assert!(d.balloon().unwrap().is_removal());
    }

    #[test]
    fn balloon_kinds_and_flags() {
        for (flags, kind) in [
            (0, BalloonKind::None),
            (1, BalloonKind::Info),
            (2, BalloonKind::Warning),
            (3, BalloonKind::Error),
            (4, BalloonKind::User),
            (5, BalloonKind::Other(5)),
            (0x0F, BalloonKind::Other(0x0F)),
        ] {
            let mut raw = golden_full();
            put32(&mut raw, 0x3AC, flags | 0x20 | 0x80);
            let b = NotifyIconRequest::parse(&raw).unwrap().data.balloon().unwrap();
            assert_eq!(b.kind, kind);
            assert!(b.large_icon && b.respect_quiet_time && !b.no_sound);
        }
        let mut raw = golden_full();
        put32(&mut raw, 0x014, 0x10 | 0x40);
        let b = NotifyIconRequest::parse(&raw).unwrap().data.balloon().unwrap();
        assert!(b.realtime);
        put32(&mut raw, 0x3C0, 0);
        assert_eq!(NotifyIconRequest::parse(&raw).unwrap().data.balloon().unwrap().user_icon, None);
    }

    #[test]
    fn state_merge_changes_only_masked_bits() {
        let s = StateUpdate { state: 0b01, mask: 0b01 };
        assert_eq!(s.merge(0b10), 0b11);
        let s = StateUpdate { state: 0b00, mask: 0b01 };
        assert_eq!(s.merge(0b11), 0b10);
        let s = StateUpdate { state: 0b11, mask: 0b00 };
        assert_eq!(s.merge(0b10), 0b10, "an empty mask changes nothing");
        assert_eq!(StateUpdate { state: 2, mask: 2 }.shared_icon(), Some(true));
        assert_eq!(StateUpdate { state: 1, mask: 2 }.hidden(), None);
    }

    #[test]
    fn every_nim_message_parses_and_others_do_not() {
        for m in 0..5u32 {
            let mut raw = golden_full();
            put32(&mut raw, 4, m);
            assert_eq!(NotifyIconRequest::parse(&raw).unwrap().message.as_u32(), m);
        }
        for m in [5u32, 6, 0xFF, u32::MAX] {
            let mut raw = golden_full();
            put32(&mut raw, 4, m);
            assert_eq!(NotifyIconRequest::parse(&raw), Err(WireError::UnknownNotifyMessage(m)));
        }
    }

    #[test]
    fn set_version_reads_the_union_word() {
        for (v, want) in [(0, Some(IconVersion::V0)), (3, Some(IconVersion::V3)), (4, Some(IconVersion::V4)), (1, None), (5, None), (5000, None)] {
            let mut raw = golden_full();
            put32(&mut raw, 4, 4);
            put32(&mut raw, 0x328, v);
            let r = NotifyIconRequest::parse(&raw).unwrap();
            assert_eq!(r.message, Nim::SetVersion);
            assert_eq!(r.data.requested_version(), want, "version {v}");
        }
    }

    #[test]
    fn a_short_set_version_block_asks_for_nothing() {
        // the union word lies at 0x328 of the block; a block that ends before it must not read as "version 0"
        let mut raw = golden_full();
        put32(&mut raw, 4, 4);
        put32(&mut raw, 0x328, 4);
        for n in MIN_PAYLOAD_LEN..=raw.len() {
            let r = NotifyIconRequest::parse(&raw[..n]).unwrap();
            let whole = r.data.tier >= DataTier::V2;
            assert_eq!(r.data.requested_version(), whole.then_some(IconVersion::V4), "len {n}");
        }
    }

    #[test]
    fn wrong_signature_is_rejected() {
        for sig in [0u32, 0x3475_3422, 0x2334_7534, u32::MAX] {
            let mut raw = golden_full();
            put32(&mut raw, 0, sig);
            assert_eq!(NotifyIconRequest::parse(&raw), Err(WireError::BadSignature { found: sig }));
        }
    }

    #[test]
    fn every_truncation_is_handled() {
        let raw = golden_full();
        for n in 0..=raw.len() {
            let r = NotifyIconRequest::parse(&raw[..n]);
            if n < MIN_PAYLOAD_LEN {
                assert!(matches!(r, Err(WireError::Truncated { .. })), "len {n}: {r:?}");
            } else {
                let r = r.unwrap_or_else(|e| panic!("len {n}: {e}"));
                // the tier follows the length
                let want = DataTier::from_len(n - 8).unwrap();
                assert_eq!(r.data.tier, want, "len {n}");
                assert_eq!(r.data.hwnd, 0x0001_0ABC);
                // text and state beyond the data are empty, never garbage
                if want < DataTier::V2 {
                    assert!(r.data.info.is_empty() && r.data.info_title.is_empty());
                    assert_eq!(r.data.state_mask, 0);
                    assert_eq!(r.data.state_update(), None);
                    assert_eq!(r.data.balloon(), None);
                }
                if want < DataTier::V3 {
                    assert!(r.data.guid.is_zero());
                    assert!(matches!(r.data.key(), IconKey::Wnd { .. }));
                }
                if want < DataTier::Full {
                    assert_eq!(r.data.balloon_icon, 0);
                }
            }
        }
        // short wrong-signature block: the header check comes first
        assert!(matches!(NotifyIconRequest::parse(&[1, 2, 3]), Err(WireError::Truncated { .. })));
        assert_eq!(NotifyIconRequest::parse(&[0; 8]), Err(WireError::BadSignature { found: 0 }));
    }

    #[test]
    fn tiers_follow_the_documented_sizes() {
        assert_eq!(DataTier::from_len(0), None);
        assert_eq!(DataTier::from_len(0x17), None);
        assert_eq!(DataTier::from_len(0x18), Some(DataTier::Core));
        assert_eq!(DataTier::from_len(0x97), Some(DataTier::Core));
        assert_eq!(DataTier::from_len(0x98), Some(DataTier::V1));
        assert_eq!(DataTier::from_len(0x3A7), Some(DataTier::V1));
        assert_eq!(DataTier::from_len(0x3A8), Some(DataTier::V2));
        assert_eq!(DataTier::from_len(0x3B7), Some(DataTier::V2));
        assert_eq!(DataTier::from_len(0x3B8), Some(DataTier::V3));
        assert_eq!(DataTier::from_len(0x3BB), Some(DataTier::V3));
        assert_eq!(DataTier::from_len(0x3BC), Some(DataTier::Full));
        assert_eq!(DataTier::from_len(100_000), Some(DataTier::Full));
        // the NIF bits a tier admits
        assert_eq!(DataTier::Core.allowed_flags() & (nif::STATE | nif::INFO | nif::GUID), 0);
        assert_eq!(DataTier::V2.allowed_flags() & nif::GUID, 0);
        assert_ne!(DataTier::V2.allowed_flags() & nif::INFO, 0);
        assert_ne!(DataTier::V3.allowed_flags() & nif::GUID, 0);
    }

    #[test]
    fn old_sender_blocks_ignore_flags_their_size_cannot_back() {
        // an old (V1-sized) block that still has NIF_INFO|NIF_GUID|NIF_STATE set in stale flags
        let raw = &golden_full()[..8 + 0x98];
        let d = NotifyIconRequest::parse(raw).unwrap().data;
        assert_eq!(d.tier, DataTier::V1);
        assert_eq!(d.effective_flags(), nif::MESSAGE | nif::ICON | nif::TIP);
        assert_eq!(d.balloon(), None);
        assert_eq!(d.state_update(), None);
        assert!(matches!(d.key(), IconKey::Wnd { .. }));
        assert_eq!(d.tip_text(), Some("Tip text"));
    }

    #[test]
    fn v2_and_v3_sized_blocks() {
        let raw = golden_full();
        let v2 = NotifyIconRequest::parse(&raw[..8 + 0x3A8]).unwrap().data;
        assert_eq!(v2.tier, DataTier::V2);
        assert!(v2.balloon().is_some() && v2.state_update().is_some());
        assert!(v2.guid.is_zero() && matches!(v2.key(), IconKey::Wnd { .. }));
        let v3 = NotifyIconRequest::parse(&raw[..8 + 0x3B8]).unwrap().data;
        assert_eq!(v3.tier, DataTier::V3);
        assert!(matches!(v3.key(), IconKey::Guid(_)));
        assert_eq!(v3.balloon().unwrap().user_icon, None, "hBalloonIcon is not inside a V3-sized block");
    }

    #[test]
    fn strings_that_fill_their_field_lose_the_last_unit() {
        let mut raw = golden_full();
        // tip: fill all 128 units with 'x' (no NUL at all)
        for i in 0..128 {
            raw[0x20 + i * 2] = b'x';
            raw[0x20 + i * 2 + 1] = 0;
        }
        for i in 0..256 {
            raw[0x128 + i * 2] = b'y';
            raw[0x128 + i * 2 + 1] = 0;
        }
        for i in 0..64 {
            raw[0x32C + i * 2] = b'z';
            raw[0x32C + i * 2 + 1] = 0;
        }
        let d = NotifyIconRequest::parse(&raw).unwrap().data;
        assert_eq!(d.tip, "x".repeat(127));
        assert_eq!(d.info, "y".repeat(255));
        assert_eq!(d.info_title, "z".repeat(63));
    }

    #[test]
    fn strings_stop_at_the_first_nul_and_keep_unicode() {
        let mut raw = golden_full();
        raw[0x20..0x20 + 256].fill(0);
        put_wide(&mut raw, 0x20, "Sürücü \u{1F600}");
        raw[0x20 + 2 * 20..0x20 + 2 * 20 + 2].copy_from_slice(&(b'Q' as u16).to_le_bytes()); // junk after the NUL stays unread
        let d = NotifyIconRequest::parse(&raw).unwrap().data;
        assert_eq!(d.tip, "Sürücü \u{1F600}");
    }

    #[test]
    fn round_trip_full_block() {
        let raw = golden_full();
        let r = NotifyIconRequest::parse(&raw).unwrap();
        assert_eq!(r.encode(), raw, "bytes -> request -> bytes");
        assert_eq!(NotifyIconRequest::parse(&r.encode()).unwrap(), r, "request -> bytes -> request");
    }

    #[test]
    fn round_trip_with_trailing_word() {
        let mut raw = golden_full();
        raw.extend_from_slice(&4u32.to_le_bytes());
        let r = NotifyIconRequest::parse(&raw).unwrap();
        assert_eq!(r.trailing_len, 4);
        assert_eq!(r.trailing_word, Some(4));
        assert_eq!(r.encode(), raw);
        // longer trailing data is counted and its first word kept, the rest is opaque
        raw.extend_from_slice(&[9; 30]);
        let r = NotifyIconRequest::parse(&raw).unwrap();
        assert_eq!(r.trailing_len, 34);
        assert_eq!(r.trailing_word, Some(4));
        // fewer than four bytes: counted, no word
        let mut raw = golden_full();
        raw.extend_from_slice(&[1, 2, 3]);
        let r = NotifyIconRequest::parse(&raw).unwrap();
        assert_eq!((r.trailing_len, r.trailing_word), (3, None));
    }

    #[test]
    fn round_trip_built_from_fields() {
        let d = NotifyIconData {
            cb_size: 976,
            hwnd: 0xFFFF_0001,
            uid: 0xFFFF_FFFF,
            flags: nif::MESSAGE | nif::TIP | nif::GUID | nif::SHOWTIP,
            callback_message: 0x8123,
            icon: 0,
            tip: "x".repeat(127),
            state: 0,
            state_mask: 0,
            info: String::new(),
            timeout_or_version: 0,
            info_title: "t".repeat(63),
            info_flags: 0,
            guid: WireGuid::from_parts(1, 2, 3, [4; 8]),
            balloon_icon: 0,
            tier: DataTier::Full,
        };
        for msg in [Nim::Add, Nim::Modify, Nim::Delete, Nim::SetFocus, Nim::SetVersion] {
            let r = NotifyIconRequest { message: msg, data: d.clone(), trailing_len: 0, trailing_word: None };
            assert_eq!(NotifyIconRequest::parse(&r.encode()).unwrap(), r);
        }
        // over-long text is cut at encode, never overflows into the next field
        let mut long = d.clone();
        long.tip = "a".repeat(500);
        long.info = "b".repeat(500);
        long.info_title = "c".repeat(500);
        let back = NotifyIconRequest::parse(&NotifyIconRequest { message: Nim::Add, data: long, trailing_len: 0, trailing_word: None }.encode()).unwrap();
        assert_eq!(back.data.tip.len(), 127);
        assert_eq!(back.data.info.len(), 255);
        assert_eq!(back.data.info_title.len(), 63);
        assert_eq!(back.data.state, 0, "the field after the tip is untouched");
    }

    #[test]
    fn sixty_four_bit_and_thirty_two_bit_senders_send_the_same_layout() {
        // only the inner cbSize differs; the parse of the rest is identical
        let mut a = golden_full();
        let mut b = golden_full();
        put32(&mut a, 8, 956);
        put32(&mut b, 8, 976);
        let (ra, rb) = (NotifyIconRequest::parse(&a).unwrap(), NotifyIconRequest::parse(&b).unwrap());
        assert_eq!(ra.data.sender_layouts(), vec![SenderLayout { bits: 32, variant: SenderVariant::Full }]);
        assert_eq!(rb.data.sender_layouts(), vec![SenderLayout { bits: 64, variant: SenderVariant::Full }]);
        let mut rb_fixed = rb.clone();
        rb_fixed.data.cb_size = 956;
        assert_eq!(ra, rb_fixed);
    }

    #[test]
    fn sender_layout_table() {
        for (cb, bits, variant) in [
            (152, 32, SenderVariant::V1),
            (936, 32, SenderVariant::V2),
            (956, 32, SenderVariant::Full),
            (168, 64, SenderVariant::V1),
            (968, 64, SenderVariant::V3),
            (976, 64, SenderVariant::Full),
        ] {
            let mut raw = golden_full();
            put32(&mut raw, 8, cb);
            let l = NotifyIconRequest::parse(&raw).unwrap().data.sender_layouts();
            assert_eq!(l, vec![SenderLayout { bits, variant }], "cbSize {cb}");
        }
        // 952 is both the 32-bit V3 and the 64-bit V2 size
        let mut raw = golden_full();
        put32(&mut raw, 8, 952);
        let l = NotifyIconRequest::parse(&raw).unwrap().data.sender_layouts();
        assert_eq!(l, vec![SenderLayout { bits: 32, variant: SenderVariant::V3 }, SenderLayout { bits: 64, variant: SenderVariant::V2 }]);
        for cb in [0u32, 1, 100, 957, 4000, u32::MAX] {
            let mut raw = golden_full();
            put32(&mut raw, 8, cb);
            assert!(NotifyIconRequest::parse(&raw).unwrap().data.sender_layouts().is_empty(), "cbSize {cb}");
        }
    }

    #[test]
    fn handles_are_sign_extended_for_win32() {
        let mut raw = golden_full();
        put32(&mut raw, 0x00C, 0xFFFF_FFFE);
        let d = NotifyIconRequest::parse(&raw).unwrap().data;
        assert_eq!(d.hwnd_handle(), -2);
        assert_eq!(d.hwnd, 0xFFFF_FFFE, "the raw 32 bits are kept for the key");
    }

    #[test]
    fn nif_names_lists_bits() {
        assert_eq!(nif_names(0), "-");
        assert_eq!(nif_names(nif::MESSAGE | nif::ICON | nif::TIP), "MESSAGE|ICON|TIP");
        assert_eq!(nif_names(0xFF), "MESSAGE|ICON|TIP|STATE|INFO|GUID|REALTIME|SHOWTIP");
        assert_eq!(nif_names(0x100), "0x100");
    }

    #[test]
    fn huge_block_is_fine() {
        let mut raw = golden_full();
        raw.extend(std::iter::repeat_n(0xAB, 1_000_000));
        let r = NotifyIconRequest::parse(&raw).unwrap();
        assert_eq!(r.trailing_len, 1_000_000);
        assert_eq!(r.data.tip, "Tip text");
    }

    #[test]
    fn random_blocks_with_a_good_header_never_panic() {
        let mut rng = XorShift(0xDEAD_BEEF_CAFE_F00D);
        for _ in 0..20_000 {
            let n = rng.below(1100) as usize;
            let mut b = NOTIFY_SIGNATURE.to_le_bytes().to_vec();
            b.extend_from_slice(&(rng.below(5) as u32).to_le_bytes());
            b.extend(rng.bytes(n));
            if let Ok(r) = NotifyIconRequest::parse(&b) {
                // exercise every accessor on whatever came out
                let d = &r.data;
                let _ = (d.key(), d.callback(), d.icon_handle(), d.tip_text(), d.state_update(), d.balloon(), d.requested_version(), d.sender_layouts());
                let _ = r.encode();
            }
        }
    }
}
