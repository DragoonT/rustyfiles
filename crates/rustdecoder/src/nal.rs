//! NAL units: the length-prefixed form MP4 stores them in, the decoder configuration records
//! (`avcC`, `hvcC`) that carry the parameter sets, and emulation prevention.

use crate::{Error, Result};

/// Removes emulation prevention bytes (`00 00 03` → `00 00`), giving the raw payload.
pub fn unescape(nal: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(nal.len());
    let mut zeros = 0;
    for &b in nal {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
    }
    out
}

/// The NAL units of one sample, each prefixed by its length in `len_size` bytes (1, 2 or 4).
pub fn units(sample: &[u8], len_size: usize) -> impl Iterator<Item = Result<&[u8]>> {
    let mut p = 0usize;
    std::iter::from_fn(move || {
        if p >= sample.len() {
            return None;
        }
        if p + len_size > sample.len() {
            p = sample.len();
            return Some(Err(Error::Invalid("NAL length cut short")));
        }
        let n = sample[p..p + len_size].iter().fold(0usize, |a, &b| (a << 8) | b as usize);
        p += len_size;
        if n > sample.len() - p {
            p = sample.len();
            return Some(Err(Error::Invalid("NAL unit runs past its sample")));
        }
        let u = &sample[p..p + n];
        p += n;
        Some(Ok(u))
    })
}

/// What a decoder configuration record holds.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// Bytes in each NAL unit's length prefix.
    pub len_size: usize,
    /// Parameter sets (and any SEI) in the order stored, escaped as in the stream.
    pub units: Vec<Vec<u8>>,
}

/// The `hvcC` record (ISO/IEC 14496-15, 8.3.3).
pub fn hvcc(b: &[u8]) -> Result<Config> {
    if b.len() < 23 {
        return Err(Error::Invalid("hvcC too short"));
    }
    let len_size = (b[21] & 3) as usize + 1;
    let arrays = b[22];
    let mut p = 23usize;
    let mut units = Vec::new();
    for _ in 0..arrays {
        if p + 3 > b.len() {
            return Err(Error::Invalid("hvcC array cut short"));
        }
        let n = u16::from_be_bytes([b[p + 1], b[p + 2]]);
        p += 3;
        for _ in 0..n {
            if p + 2 > b.len() {
                return Err(Error::Invalid("hvcC unit cut short"));
            }
            let len = u16::from_be_bytes([b[p], b[p + 1]]) as usize;
            p += 2;
            let u = b.get(p..p + len).ok_or(Error::Invalid("hvcC unit cut short"))?;
            units.push(u.to_vec());
            p += len;
        }
    }
    Ok(Config { len_size, units })
}

/// The `avcC` record (ISO/IEC 14496-15, 5.3.3).
pub fn avcc(b: &[u8]) -> Result<Config> {
    if b.len() < 7 {
        return Err(Error::Invalid("avcC too short"));
    }
    let len_size = (b[4] & 3) as usize + 1;
    let mut p = 5usize;
    let mut units = Vec::new();
    for group in 0..2 {
        let n = if group == 0 { b[p] & 0x1f } else { *b.get(p).ok_or(Error::Invalid("avcC cut short"))? };
        p += 1;
        for _ in 0..n {
            if p + 2 > b.len() {
                return Err(Error::Invalid("avcC unit cut short"));
            }
            let len = u16::from_be_bytes([b[p], b[p + 1]]) as usize;
            p += 2;
            let u = b.get(p..p + len).ok_or(Error::Invalid("avcC unit cut short"))?;
            units.push(u.to_vec());
            p += len;
        }
    }
    Ok(Config { len_size, units })
}

#[cfg(test)]
mod tests {
    #[test]
    fn unescape_removes_prevention_bytes() {
        assert_eq!(super::unescape(&[0, 0, 3, 1, 0, 0, 3, 0, 0, 3]), [0, 0, 1, 0, 0, 0, 0]);
        // a 03 not after two zeros stays
        assert_eq!(super::unescape(&[0, 3, 0, 0, 2, 3]), [0, 3, 0, 0, 2, 3]);
    }

    #[test]
    fn length_prefixed_units() {
        let s = [0, 0, 0, 2, 0xaa, 0xbb, 0, 0, 0, 1, 0xcc];
        let u: Vec<&[u8]> = super::units(&s, 4).map(|u| u.unwrap()).collect();
        assert_eq!(u, [&[0xaa, 0xbb][..], &[0xcc][..]]);
        assert!(super::units(&[0, 0, 0, 9, 1], 4).next().unwrap().is_err());
    }
}
