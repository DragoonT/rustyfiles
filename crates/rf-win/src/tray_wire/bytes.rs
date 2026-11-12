//! Bounds-checked reads and writes on byte blocks that come from other processes. Every read goes through `get(range)`, so a short
//! block gives `None` instead of a panic; nothing here indexes a slice with an offset that came from the data.

pub(crate) fn slice_at(b: &[u8], off: usize, len: usize) -> Option<&[u8]> {
    b.get(off..off.checked_add(len)?)
}

pub(crate) fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    let s = slice_at(b, off, 4)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

pub(crate) fn i32_at(b: &[u8], off: usize) -> Option<i32> {
    u32_at(b, off).map(|v| v as i32)
}

pub(crate) fn i64_at(b: &[u8], off: usize) -> Option<i64> {
    let s = slice_at(b, off, 8)?;
    Some(i64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

pub(crate) fn array16_at(b: &[u8], off: usize) -> Option<[u8; 16]> {
    let s = slice_at(b, off, 16)?;
    let mut a = [0u8; 16];
    a.copy_from_slice(s);
    Some(a)
}

/// A fixed-size UTF-16 text field of `max_units` code units starting at `off`.
///
/// Reads at most `max_units - 1` units and stops at the first NUL: shell32 forces a terminator into the last unit, but the bytes come
/// from another process, so the last unit is never trusted. A field cut short by the end of the block yields what is there. Lone
/// surrogates become U+FFFD.
pub(crate) fn wide_field(b: &[u8], off: usize, max_units: usize) -> String {
    let avail = b.len().saturating_sub(off) / 2;
    let mut n = avail.min(max_units);
    if n == max_units {
        n -= 1;
    }
    let mut units = Vec::with_capacity(n);
    for i in 0..n {
        let o = off + i * 2;
        let u = u16::from_le_bytes([b[o], b[o + 1]]);
        if u == 0 {
            break;
        }
        units.push(u);
    }
    String::from_utf16_lossy(&units)
}

// The writers below are used only with constant offsets inside buffers this crate sized itself, so they index directly.

pub(crate) fn set_u32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

pub(crate) fn set_i32(buf: &mut [u8], off: usize, v: i32) {
    set_u32(buf, off, v as u32);
}

pub(crate) fn set_i64(buf: &mut [u8], off: usize, v: i64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// Writes `s` as a NUL-padded UTF-16 field of `units` code units. Text longer than `units - 1` units is cut (a surrogate pair is never
/// split), so the field always ends in a NUL, like shell32's own.
pub(crate) fn set_wide_field(buf: &mut [u8], off: usize, units: usize, s: &str) {
    let mut u: Vec<u16> = s.encode_utf16().collect();
    if u.len() > units - 1 {
        u.truncate(units - 1);
        if matches!(u.last(), Some(0xD800..=0xDBFF)) {
            u.pop();
        }
    }
    for (i, unit) in u.iter().enumerate() {
        buf[off + i * 2..off + i * 2 + 2].copy_from_slice(&unit.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_are_bounds_checked() {
        let b = [1u8, 2, 3, 4, 5];
        assert_eq!(u32_at(&b, 0), Some(0x0403_0201));
        assert_eq!(u32_at(&b, 1), Some(0x0504_0302));
        assert_eq!(u32_at(&b, 2), None);
        assert_eq!(u32_at(&b, usize::MAX), None);
        assert_eq!(u32_at(&b, usize::MAX - 1), None);
        assert_eq!(i64_at(&b, 0), None);
        assert_eq!(array16_at(&b, 0), None);
        assert_eq!(u32_at(&[], 0), None);
        assert_eq!(i32_at(&[0xFF, 0xFF, 0xFF, 0xFF], 0), Some(-1));
    }

    #[test]
    fn wide_field_stops_at_nul_and_never_reads_the_last_unit() {
        // 4-unit field "ab\0c": stops at the NUL
        let mut b = Vec::new();
        for u in [b'a' as u16, b'b' as u16, 0, b'c' as u16] {
            b.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(wide_field(&b, 0, 4), "ab");
        // a field that is full to the last unit: only max-1 units are taken
        let mut full = Vec::new();
        for u in [b'a' as u16, b'b' as u16, b'c' as u16, b'd' as u16] {
            full.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(wide_field(&full, 0, 4), "abc");
        // a field cut short by the end of the block: what is there
        assert_eq!(wide_field(&full[..6], 0, 4), "abc");
        assert_eq!(wide_field(&full[..5], 0, 4), "ab");
        assert_eq!(wide_field(&full, 100, 4), "");
        assert_eq!(wide_field(&full, usize::MAX, 4), "");
    }

    #[test]
    fn wide_field_lone_surrogate_is_replaced() {
        let b = [0x00, 0xD8, b'x', 0x00, 0, 0, 0, 0];
        assert_eq!(wide_field(&b, 0, 4), "\u{FFFD}x");
    }

    #[test]
    fn set_wide_field_cuts_without_splitting_a_pair() {
        let mut buf = vec![0u8; 8]; // 4 units
        set_wide_field(&mut buf, 0, 4, "ab\u{1F600}"); // a, b, then a pair: only 3 units fit, the pair would start at unit 2 and cross
        assert_eq!(wide_field(&buf, 0, 4), "ab");
        let mut buf = vec![0u8; 8];
        set_wide_field(&mut buf, 0, 4, "a\u{1F600}"); // a + pair = 3 units: fits
        assert_eq!(wide_field(&buf, 0, 4), "a\u{1F600}");
        let mut buf = vec![0u8; 8];
        set_wide_field(&mut buf, 0, 4, "abcdef");
        assert_eq!(wide_field(&buf, 0, 4), "abc");
        assert_eq!(&buf[6..8], &[0, 0], "the last unit stays NUL");
    }
}
