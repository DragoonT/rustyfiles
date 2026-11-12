//! `StartupApproved`: the binary value Task Manager and Settings write when the user switches a start-up entry off or on.
//!
//! What is known (research note 1, section 5.1, and the values read on the development machine, counted by form):
//! the value is 12 bytes: a little-endian DWORD, then an 8-byte FILETIME.
//!
//! | first DWORD | meaning | the FILETIME |
//! |---|---|---|
//! | 2 | enabled | zero, or the moment it was switched on again (both seen) |
//! | 6 | enabled | zero (seen) |
//! | 3 | disabled | the moment it was switched off (seen) |
//!
//! Anything else (another first DWORD, another length) is `Unknown`. The research note's rule "an odd first byte means disabled"
//! (a different project's rule) was NOT adopted: with 7, 1 or 5 nobody here has seen what Windows writes, so those stay unknown and
//! the plan (`plan.rs`) says out loud what it does with them.

use super::model::Approval;

const LEN: usize = 12;

/// Decodes one StartupApproved value.
pub fn decode(bytes: &[u8]) -> Approval {
    if bytes.len() != LEN {
        return Approval::Unknown;
    }
    let first = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let ft = u64::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9], bytes[10], bytes[11]]);
    let ft = (ft != 0).then_some(ft);
    match first {
        2 | 6 => Approval::Enabled { changed: ft },
        3 => Approval::Disabled { since: ft },
        _ => Approval::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(first: u32, ft: u64) -> Vec<u8> {
        let mut v = first.to_le_bytes().to_vec();
        v.extend_from_slice(&ft.to_le_bytes());
        v
    }

    #[test]
    fn the_known_forms() {
        assert_eq!(decode(&value(2, 0)), Approval::Enabled { changed: None });
        assert_eq!(decode(&value(6, 0)), Approval::Enabled { changed: None });
        // an enabled value that carries the time it was switched back on (seen on a real machine)
        assert_eq!(decode(&value(2, 0x01dd_2918_20f7_f7f1)), Approval::Enabled { changed: Some(0x01dd_2918_20f7_f7f1) });
        assert_eq!(decode(&value(3, 0x01dc_b048_e34a_99c1)), Approval::Disabled { since: Some(0x01dc_b048_e34a_99c1) });
        assert_eq!(decode(&value(3, 0)), Approval::Disabled { since: None });
    }

    #[test]
    fn the_bytes_of_a_real_value_decode_as_stored() {
        // 03 00 00 00 + FILETIME, bytes as the registry shows them
        let raw = [0x03, 0, 0, 0, 0xc1, 0x99, 0x4a, 0xda, 0x48, 0xb0, 0xdc, 0x01];
        assert_eq!(decode(&raw), Approval::Disabled { since: Some(0x01dc_b048_da4a_99c1) });
    }

    #[test]
    fn unknown_layouts_are_unknown_and_never_guessed() {
        for first in [0u32, 1, 4, 5, 7, 8, 0x0100_0003, u32::MAX] {
            assert_eq!(decode(&value(first, 0)), Approval::Unknown, "first dword {first}");
        }
        assert_eq!(decode(&[]), Approval::Unknown);
        assert_eq!(decode(&[3]), Approval::Unknown);
        assert_eq!(decode(&[3, 0, 0, 0]), Approval::Unknown, "only the DWORD: not a layout we know");
        let mut longer = value(3, 1);
        longer.push(0);
        assert_eq!(decode(&longer), Approval::Unknown);
        assert_eq!(decode(&vec![3u8; 100_000]), Approval::Unknown);
    }
}
