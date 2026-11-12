//! The `DestList` stream of an `<AppID>.automaticDestinations-ms` file: which entries the file has, which of them are pinned and in
//! what order, when each was last used, and the path Windows noted for it. Pure: bytes in, values out.
//!
//! Layout (note 3-start-search.md section 1.6, from the libyal format notes; the offsets that matter here were checked against this
//! machine's files by `jumplist_probe`, which prints how many entries parse and how many name a stream that exists):
//!
//! * header, 32 bytes: u32 format version (1 = Windows 7, 3 and up = Windows 10 and later), u32 entries, u32 pinned entries, ...
//! * entry, both versions: entry number at 88, FILETIME of the last use at 100, pin status (i32, -1 = not pinned, otherwise the
//!   pin's position) at 108;
//!   * version 1: u16 path length in characters at 112, the UTF-16LE path from 114;
//!   * version 3 and up: a u32 at 112, the use count at 116, u16 path length at 128, the path from 130, then a u32 holding the size of
//!     an extra block that follows (a serialized property store, `1SPS`; 0 in almost every entry, 2988 bytes in some of this
//!     machine's) and the block itself. This machine's files are all version 6 and this layout parses all but a few entries of 6580.

/// One entry of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestEntry {
    /// The number of the stream (named in hexadecimal) that holds the entry's shell link.
    pub number: u32,
    /// The pin's position when the entry is pinned.
    pub pin: Option<u32>,
    /// When it was last used, as a FILETIME (100 ns since 1601).
    pub last_used: u64,
    /// The times it was used (version 3 and up; 0 before).
    pub uses: u32,
    /// The path Windows noted (may be empty).
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DestList {
    pub version: u32,
    /// The entry count and the pinned count the header declares (the parsed `entries` can be fewer when the stream is damaged).
    pub declared_entries: u32,
    pub declared_pinned: u32,
    pub entries: Vec<DestEntry>,
}

impl DestList {
    /// The pinned entries in the pins' order.
    pub fn pinned(&self) -> Vec<&DestEntry> {
        let mut p: Vec<&DestEntry> = self.entries.iter().filter(|e| e.pin.is_some()).collect();
        p.sort_by_key(|e| e.pin);
        p
    }

    /// The entries that are not pinned, the most recently used first.
    pub fn recent(&self) -> Vec<&DestEntry> {
        let mut r: Vec<&DestEntry> = self.entries.iter().filter(|e| e.pin.is_none()).collect();
        r.sort_by_key(|e| std::cmp::Reverse(e.last_used));
        r
    }
}

/// UTF-16LE bytes (an odd last byte is ignored) as text.
pub(super) fn utf16_le(raw: &[u8]) -> String {
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    String::from_utf16_lossy(&units)
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}
fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Parses a `DestList` stream. `None` when the header is not one (too short, an unknown version, an absurd entry count); an entry
/// that is cut short ends the list there and the entries before it are kept.
pub fn parse(bytes: &[u8]) -> Option<DestList> {
    let version = u32_at(bytes, 0)?;
    let declared = u32_at(bytes, 4)?;
    let declared_pinned = u32_at(bytes, 8)?;
    if !(1..=16).contains(&version) || bytes.len() < 32 {
        return None;
    }
    // an entry is at least 114 bytes; a header that promises more entries than the bytes can hold is not trusted past the bytes
    let cap = (bytes.len() - 32) / 114;
    let mut entries = Vec::new();
    let mut at = 32usize;
    for _ in 0..(declared as usize).min(cap) {
        let Some(e) = entry(bytes, at, version) else { break };
        at = e.1;
        entries.push(e.0);
    }
    Some(DestList { version, declared_entries: declared, declared_pinned, entries })
}

/// One entry at `at`, and where the next one starts.
fn entry(b: &[u8], at: usize, version: u32) -> Option<(DestEntry, usize)> {
    let number = u32_at(b, at + 88)?;
    let last_used = u64_at(b, at + 100)?;
    let pin_raw = u32_at(b, at + 108)? as i32;
    let (uses, len_at, path_at) = if version == 1 { (0, at + 112, at + 114) } else { (u32_at(b, at + 116)?, at + 128, at + 130) };
    let chars = u16_at(b, len_at)? as usize;
    let raw = b.get(path_at..path_at + chars * 2)?;
    let path = utf16_le(raw);
    let path_end = path_at + chars * 2;
    let next = if version == 1 {
        path_end
    } else {
        // the extra block's size; a size that runs past the stream ends the list here
        let extra = u32_at(b, path_end).map_or(0, |n| n as usize);
        let end = path_end + 4 + extra;
        if end > b.len() && extra != 0 {
            return None;
        }
        // the size field is not required of the last entry (a stream cut exactly at its path is still read)
        end.min(b.len())
    };
    Some((DestEntry { number, pin: (pin_raw >= 0).then_some(pin_raw as u32), last_used, uses, path }, next))
}

/// Builds a `DestList` stream (for tests, here and elsewhere): the inverse of [`parse`], by the layout above.
#[cfg(test)]
pub fn build(version: u32, entries: &[DestEntry]) -> Vec<u8> {
    build_with_extra(version, entries, &[])
}

/// [`build`] with an extra block of `extra[i]` bytes after entry `i`'s path (version 3 and up).
#[cfg(test)]
pub fn build_with_extra(version: u32, entries: &[DestEntry], extra: &[usize]) -> Vec<u8> {
    let mut out = vec![0u8; 32];
    out[0..4].copy_from_slice(&version.to_le_bytes());
    out[4..8].copy_from_slice(&(entries.len() as u32).to_le_bytes());
    let pinned = entries.iter().filter(|e| e.pin.is_some()).count() as u32;
    out[8..12].copy_from_slice(&pinned.to_le_bytes());
    for (i, e) in entries.iter().enumerate() {
        let units: Vec<u16> = e.path.encode_utf16().collect();
        let fixed = if version == 1 { 114 } else { 130 };
        let mut v = vec![0u8; fixed];
        // a recognisable filler in the fields nobody reads, so a parser that reads a wrong offset fails a test
        for (i, b) in v.iter_mut().enumerate().take(88) {
            *b = (i as u8).wrapping_mul(7).wrapping_add(1);
        }
        v[88..92].copy_from_slice(&e.number.to_le_bytes());
        v[100..108].copy_from_slice(&e.last_used.to_le_bytes());
        let pin = e.pin.map_or(-1i32, |p| p as i32);
        v[108..112].copy_from_slice(&pin.to_le_bytes());
        if version == 1 {
            v[112..114].copy_from_slice(&(units.len() as u16).to_le_bytes());
        } else {
            v[112..116].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
            v[116..120].copy_from_slice(&e.uses.to_le_bytes());
            v[128..130].copy_from_slice(&(units.len() as u16).to_le_bytes());
        }
        for u in &units {
            v.extend_from_slice(&u.to_le_bytes());
        }
        if version != 1 {
            let n = extra.get(i).copied().unwrap_or(0);
            v.extend_from_slice(&(n as u32).to_le_bytes());
            v.extend((0..n).map(|k| 0x31u8.wrapping_add(k as u8)));
        }
        out.extend(v);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(number: u32, pin: Option<u32>, last_used: u64, path: &str) -> DestEntry {
        DestEntry { number, pin, last_used, uses: number * 3, path: path.into() }
    }

    fn sample() -> Vec<DestEntry> {
        vec![
            e(5, None, 1_000, r"C:\docs\old.txt"),
            e(2, Some(1), 500, r"C:\docs\second pin.txt"),
            e(9, None, 3_000, r"C:\docs\newest ünïcode.txt"),
            e(1, Some(0), 200, r"C:\docs\first pin.txt"),
            e(7, None, 2_000, ""),
        ]
    }

    #[test]
    fn a_windows_10_list_round_trips_with_its_pins_in_order_and_the_rest_newest_first() {
        let d = parse(&build(3, &sample())).unwrap();
        assert_eq!((d.version, d.declared_pinned, d.entries.len()), (3, 2, 5));
        assert_eq!(d.entries[2], sample()[2], "every field survives, the non-ASCII path too");
        assert_eq!(d.pinned().iter().map(|e| e.number).collect::<Vec<_>>(), vec![1, 2], "pin 0 before pin 1, not file order");
        assert_eq!(d.recent().iter().map(|e| e.number).collect::<Vec<_>>(), vec![9, 7, 5]);
    }

    #[test]
    fn an_entry_with_an_extra_block_after_its_path_does_not_swallow_the_next_entries() {
        let rows = sample();
        let d = parse(&build_with_extra(6, &rows, &[2988, 0, 40, 0, 7])).unwrap();
        assert_eq!(d.entries, rows, "all five entries, with the block sizes 2988, 40 and 7 skipped");
        // a block that claims more than the stream holds ends the list there
        let mut b = build_with_extra(6, &rows[..1], &[10]);
        let at = 32 + 130 + rows[0].path.encode_utf16().count() * 2;
        b[at..at + 4].copy_from_slice(&5_000_000u32.to_le_bytes());
        assert_eq!(parse(&b).unwrap().entries.len(), 0);
    }

    #[test]
    fn a_windows_7_list_has_its_path_two_fields_earlier_and_no_use_count() {
        let d = parse(&build(1, &sample())).unwrap();
        assert_eq!(d.entries.len(), 5);
        assert_eq!(d.entries[0].path, r"C:\docs\old.txt");
        assert_eq!(d.entries[0].uses, 0);
        assert_eq!(d.entries[1].pin, Some(1));
        assert_eq!(d.entries[3].number, 1);
    }

    #[test]
    fn a_pin_status_of_minus_one_is_not_pinned_and_zero_is_the_first_pin() {
        let d = parse(&build(3, &[e(1, None, 1, "a"), e(2, Some(0), 1, "b")])).unwrap();
        assert_eq!(d.entries[0].pin, None);
        assert_eq!(d.entries[1].pin, Some(0));
    }

    #[test]
    fn damaged_input_gives_what_is_whole_or_nothing_and_never_panics() {
        let full = build(3, &sample());
        // cut inside the last entries: the whole ones before the cut remain
        let cut = &full[..full.len() - 40];
        let d = parse(cut).unwrap();
        assert!(d.entries.len() < 5 && !d.entries.is_empty());
        assert_eq!(d.entries[0], sample()[0]);
        // a header promising more than the bytes hold
        let mut lie = build(3, &sample()[..1]);
        lie[4..8].copy_from_slice(&9_999_999u32.to_le_bytes());
        assert_eq!(parse(&lie).unwrap().entries.len(), 1);
        // not a DestList at all
        assert_eq!(parse(&[]), None);
        assert_eq!(parse(&[0; 31]), None);
        assert_eq!(parse(&[0xFF; 200]), None, "a version of 0xFFFFFFFF");
        assert_eq!(parse(&[0u8; 200]), None, "version 0");
        // every truncation of a good list is survived
        for n in 0..full.len() {
            let _ = parse(&full[..n]);
        }
    }

    #[test]
    fn a_path_length_that_runs_past_the_end_stops_the_list_there() {
        let mut b = build(3, &[e(1, None, 1, "abc")]);
        b[32 + 128..32 + 130].copy_from_slice(&5000u16.to_le_bytes());
        assert_eq!(parse(&b).unwrap().entries.len(), 0);
    }
}
