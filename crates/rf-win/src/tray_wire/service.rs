//! `dwData = 2`: `SHLoadInProc` / `SHEnableServiceObject`. The block is a CLSID and a flag word (20 bytes). Chappell's account of the
//! flag, the only source: 1 = `SHLoadInProc`, 2 = `SHEnableServiceObject(FALSE)`, 3 = `SHEnableServiceObject(TRUE)`. UNVERIFIED on
//! 24H2 and later; a relay forwards the block untouched.
//!
//! (One README labels dwData 2 as "tray updates"; its code and ManagedShell's use 1 for those. The code wins.)

use super::bytes::{array16_at, set_u32, u32_at};
use super::{WireError, WireGuid};

pub const LEN: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceAction {
    LoadInProc,
    Disable,
    Enable,
    Unknown(u32),
}

impl ServiceAction {
    pub fn from_u32(v: u32) -> ServiceAction {
        match v {
            1 => ServiceAction::LoadInProc,
            2 => ServiceAction::Disable,
            3 => ServiceAction::Enable,
            other => ServiceAction::Unknown(other),
        }
    }

    pub fn as_u32(self) -> u32 {
        match self {
            ServiceAction::LoadInProc => 1,
            ServiceAction::Disable => 2,
            ServiceAction::Enable => 3,
            ServiceAction::Unknown(v) => v,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceObjectRequest {
    pub clsid: WireGuid,
    pub action: ServiceAction,
    pub trailing_len: usize,
}

impl ServiceObjectRequest {
    /// Needs [`LEN`] bytes; more are counted and ignored.
    pub fn parse(b: &[u8]) -> Result<ServiceObjectRequest, WireError> {
        let truncated = || WireError::Truncated { what: "service object request", need: LEN, have: b.len() };
        let clsid = WireGuid(array16_at(b, 0).ok_or_else(truncated)?);
        let flag = u32_at(b, 16).ok_or_else(truncated)?;
        Ok(ServiceObjectRequest { clsid, action: ServiceAction::from_u32(flag), trailing_len: b.len() - LEN })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut b = vec![0u8; LEN];
        b[0..16].copy_from_slice(&self.clsid.0);
        set_u32(&mut b, 16, self.action.as_u32());
        b
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    /// CLSID `{35CEC8A3-2BE6-11D2-8773-92E220524153}` (the classic SysTray service object) with flag 3.
    pub fn golden() -> Vec<u8> {
        let mut b = vec![0u8; 20];
        b[0..16].copy_from_slice(&[0xA3, 0xC8, 0xCE, 0x35, 0xE6, 0x2B, 0xD2, 0x11, 0x87, 0x73, 0x92, 0xE2, 0x20, 0x52, 0x41, 0x53]);
        b[16..20].copy_from_slice(&3u32.to_le_bytes());
        b
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn golden_parses() {
        let r = ServiceObjectRequest::parse(&golden()).unwrap();
        assert_eq!(r.clsid.to_string(), "{35CEC8A3-2BE6-11D2-8773-92E220524153}");
        assert_eq!(r.action, ServiceAction::Enable);
        assert_eq!(r.trailing_len, 0);
        assert_eq!(r.encode(), golden());
    }

    #[test]
    fn actions() {
        for (v, a) in [(1, ServiceAction::LoadInProc), (2, ServiceAction::Disable), (3, ServiceAction::Enable), (0, ServiceAction::Unknown(0)), (9, ServiceAction::Unknown(9))] {
            let mut b = golden();
            b[16..20].copy_from_slice(&(v as u32).to_le_bytes());
            let r = ServiceObjectRequest::parse(&b).unwrap();
            assert_eq!(r.action, a);
            assert_eq!(r.encode(), b);
        }
    }

    #[test]
    fn every_truncation_is_an_error() {
        let g = golden();
        for n in 0..LEN {
            assert!(matches!(ServiceObjectRequest::parse(&g[..n]), Err(WireError::Truncated { need: 20, .. })), "len {n}");
        }
        let mut longer = g.clone();
        longer.extend_from_slice(&[1; 11]);
        assert_eq!(ServiceObjectRequest::parse(&longer).unwrap().trailing_len, 11);
    }
}
