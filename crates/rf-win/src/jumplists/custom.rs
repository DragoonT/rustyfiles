//! An `<AppID>.customDestinations-ms` file: the Tasks an app wrote (`AddUserTasks`), its own named categories (`AppendCategory`) and
//! the markers for Windows' Recent and Frequent categories (`AppendKnownCategory`). Not a compound file. Pure: bytes in, values out.
//!
//! Layout, as the libyal notes give it (note 3-start-search.md section 1.6) and as all 453 non-empty files on this machine parse
//! (`jumplist_probe` prints the count of files that parse to their end):
//!
//! ```text
//! u32 version (2) | u32 category count | u32 0
//! per category:
//!   u32 type: 0 = the app's own category | 1 = a known category | 2 = the Tasks
//!   type 0: u16 name length in characters | UTF-16LE name | u32 entry count | entries
//!   type 1: u32 known category (1 = Frequent, 2 = Recent)
//!   type 2: u32 entry count | entries
//!   u32 0xBABFFBAB    <- after EVERY category, not only the last
//! entry: the 16-byte CLSID of a shell link (00021401-0000-0000-C000-000000000046), then the shell link in its file format
//! ```
//!
//! The shell link's own size fields are not a reliable boundary for the entry, so an entry runs from its CLSID to the next entry's
//! CLSID-and-link-header; the LAST entry of a category runs to the footer signature, but only to one that is followed by the end of the
//! file or by a category type (0 to 2): the four footer bytes inside a link's property data (a title or an icon path can hold any
//! bytes) do not end an entry. The link bytes handed on may carry trailing bytes; the shell link loader ignores what it does not need.

/// What a category is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CategoryKind {
    /// A category the app named.
    Named(String),
    /// Windows' Recent category (the app asked for it to be shown).
    Recent,
    /// Windows' Frequent category.
    Frequent,
    /// A known category this code has no name for.
    OtherKnown(u32),
    /// The Tasks.
    Tasks,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawCategory {
    pub kind: CategoryKind,
    /// The shell link of each entry, in the file's link format (no leading CLSID).
    pub links: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct CustomFile {
    pub categories: Vec<RawCategory>,
    /// The file was read to its last footer with every category whole.
    pub complete: bool,
}

/// The most categories read from one file (the real ones have up to a dozen).
const MAX_CATEGORIES: u32 = 256;

pub const FOOTER: [u8; 4] = [0xAB, 0xFB, 0xBF, 0xBA];
/// The CLSID of a shell link, as it is stored.
pub const LINK_CLSID: [u8; 16] = [0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46];

/// The start of the next entry: its CLSID, the link's header size (0x4C) and the link's own CLSID.
fn entry_start_at(b: &[u8], at: usize) -> bool {
    b.get(at..at + 36).is_some_and(|w| w[..16] == LINK_CLSID && w[16..20] == [0x4C, 0, 0, 0] && w[20..36] == LINK_CLSID)
}

/// Where the entry that starts its link at `from - 0x4C` ends: at the next entry; the last entry of a category also at a footer that
/// is followed by the end of the file or by a category type.
fn find_boundary(b: &[u8], from: usize, last: bool) -> Option<usize> {
    (from..b.len().saturating_sub(3)).find(|&i| entry_start_at(b, i) || (last && b[i..i + 4] == FOOTER && (i + 4 == b.len() || u32_at(b, i + 4).is_some_and(|t| t <= 2))))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Parses the file. A damaged file gives the categories read before the damage and `complete == false`; an empty or foreign one gives
/// no categories.
pub fn parse(b: &[u8]) -> CustomFile {
    let mut out = CustomFile::default();
    let (Some(version), Some(count)) = (u32_at(b, 0), u32_at(b, 4)) else { return out };
    if version != 2 || b.len() < 12 {
        return out;
    }
    let mut at = 12usize;
    for _ in 0..count.min(MAX_CATEGORIES) {
        match category(b, at) {
            Some((cat, next)) => {
                out.categories.push(cat);
                at = next;
            }
            None => return out,
        }
    }
    // more categories than are read is a file that was not read whole
    out.complete = count <= MAX_CATEGORIES;
    out
}

fn category(b: &[u8], mut at: usize) -> Option<(RawCategory, usize)> {
    let ty = u32_at(b, at)?;
    at += 4;
    let (kind, entries) = match ty {
        0 => {
            let chars = u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize;
            at += 2;
            let raw = b.get(at..at + chars * 2)?;
            at += chars * 2;
            let name = super::destlist::utf16_le(raw);
            let n = u32_at(b, at)?;
            at += 4;
            (CategoryKind::Named(name), n)
        }
        1 => {
            let id = u32_at(b, at)?;
            at += 4;
            (
                match id {
                    1 => CategoryKind::Frequent,
                    2 => CategoryKind::Recent,
                    other => CategoryKind::OtherKnown(other),
                },
                0,
            )
        }
        2 => {
            let n = u32_at(b, at)?;
            at += 4;
            (CategoryKind::Tasks, n)
        }
        _ => return None,
    };
    let mut links = Vec::new();
    let n = entries.min(4096);
    for k in 0..n {
        if b.get(at..at + 16)? != LINK_CLSID {
            return None;
        }
        let start = at + 16;
        // the link header is at least 0x4C bytes: the next boundary is looked for beyond it
        let end = find_boundary(b, start + 0x4C, k + 1 == n)?;
        links.push(b[start..end].to_vec());
        at = end;
    }
    if b.get(at..at + 4)? != FOOTER {
        return None;
    }
    Some((RawCategory { kind, links }, at + 4))
}

/// Builds a file from categories (for tests): the inverse of [`parse`].
#[cfg(test)]
pub fn build(categories: &[RawCategory]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(categories.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    for c in categories {
        match &c.kind {
            CategoryKind::Named(n) => {
                out.extend_from_slice(&0u32.to_le_bytes());
                let u: Vec<u16> = n.encode_utf16().collect();
                out.extend_from_slice(&(u.len() as u16).to_le_bytes());
                for x in u {
                    out.extend_from_slice(&x.to_le_bytes());
                }
                out.extend_from_slice(&(c.links.len() as u32).to_le_bytes());
            }
            CategoryKind::Frequent | CategoryKind::Recent | CategoryKind::OtherKnown(_) => {
                out.extend_from_slice(&1u32.to_le_bytes());
                let id = match c.kind {
                    CategoryKind::Frequent => 1,
                    CategoryKind::Recent => 2,
                    CategoryKind::OtherKnown(x) => x,
                    _ => unreachable!(),
                };
                out.extend_from_slice(&id.to_le_bytes());
            }
            CategoryKind::Tasks => {
                out.extend_from_slice(&2u32.to_le_bytes());
                out.extend_from_slice(&(c.links.len() as u32).to_le_bytes());
            }
        }
        for l in &c.links {
            out.extend_from_slice(&LINK_CLSID);
            out.extend_from_slice(l);
        }
        out.extend_from_slice(&FOOTER);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Something with a link's header (size 0x4C, the link CLSID) and a body, standing for a shell link; the body is varied.
    pub fn fake_link(body: &[u8]) -> Vec<u8> {
        let mut v = vec![0x4C, 0, 0, 0];
        v.extend_from_slice(&LINK_CLSID);
        v.resize(0x4C, 0);
        v.extend_from_slice(body);
        v
    }

    fn cats() -> Vec<RawCategory> {
        vec![
            RawCategory { kind: CategoryKind::Recent, links: vec![] },
            RawCategory { kind: CategoryKind::Named("Sık kullanılan klasörler".into()), links: vec![fake_link(b"one"), fake_link(b"two two")] },
            RawCategory { kind: CategoryKind::Frequent, links: vec![] },
            RawCategory { kind: CategoryKind::Tasks, links: vec![fake_link(b"new window"), fake_link(b""), fake_link(b"third")] },
        ]
    }

    #[test]
    fn a_file_with_every_kind_of_category_round_trips() {
        let f = parse(&build(&cats()));
        assert!(f.complete);
        assert_eq!(f.categories, cats());
    }

    #[test]
    fn an_empty_tasks_file_and_a_known_only_file_are_whole() {
        let f = parse(&build(&[RawCategory { kind: CategoryKind::Tasks, links: vec![] }]));
        assert!(f.complete);
        assert_eq!(f.categories.len(), 1);
        let f = parse(&build(&[RawCategory { kind: CategoryKind::Recent, links: vec![] }, RawCategory { kind: CategoryKind::OtherKnown(7), links: vec![] }]));
        assert!(f.complete);
        assert_eq!(f.categories[1].kind, CategoryKind::OtherKnown(7));
    }

    #[test]
    fn a_link_does_not_end_where_its_own_size_field_says() {
        // the size field of the link says 0x4C; the entry still runs to the next entry
        let f = parse(&build(&[RawCategory { kind: CategoryKind::Tasks, links: vec![fake_link(&[9; 300]), fake_link(b"x")] }]));
        assert_eq!(f.categories[0].links[0].len(), 0x4C + 300);
        assert_eq!(f.categories[0].links[1].len(), 0x4C + 1);
    }

    #[test]
    fn the_footer_bytes_inside_a_link_do_not_end_its_entry() {
        // a title or an icon path may hold any bytes, these four included; an entry that is not the last of its category runs to the
        // next entry, and the last one to a footer that is followed by a category type or the end of the file
        let mut with_footer = b"title ".to_vec();
        with_footer.extend_from_slice(&FOOTER);
        with_footer.extend_from_slice(b" and more");
        let cats = vec![
            RawCategory { kind: CategoryKind::Tasks, links: vec![fake_link(&with_footer), fake_link(b"second"), fake_link(&with_footer)] },
            RawCategory { kind: CategoryKind::Named("Own".into()), links: vec![fake_link(&with_footer)] },
            RawCategory { kind: CategoryKind::Recent, links: vec![] },
        ];
        let f = parse(&build(&cats));
        assert!(f.complete);
        assert_eq!(f.categories, cats);
    }

    #[test]
    fn more_categories_than_are_read_is_not_a_whole_file() {
        let many: Vec<RawCategory> = (0..300).map(|_| RawCategory { kind: CategoryKind::Recent, links: vec![] }).collect();
        let f = parse(&build(&many));
        assert_eq!(f.categories.len(), 256);
        assert!(!f.complete, "300 categories declared, 256 read");
        let exactly: Vec<RawCategory> = (0..256).map(|_| RawCategory { kind: CategoryKind::Recent, links: vec![] }).collect();
        assert!(parse(&build(&exactly)).complete);
    }

    #[test]
    fn foreign_empty_and_damaged_files_are_not_whole_and_never_panic() {
        assert_eq!(parse(&[]), CustomFile::default());
        assert_eq!(parse(&[0; 24]), CustomFile::default(), "version 0: the 24 zero bytes of an empty file");
        let good = build(&cats());
        assert!(parse(&good).complete);
        for n in 0..good.len() {
            let f = parse(&good[..n]);
            assert!(!f.complete, "cut at {n}");
        }
        // an unknown category type stops the file there, keeping what came before
        let mut bad = build(&cats());
        let second = 12 + 4 + 4 + 4; // after the first category (type, id, footer) the second's type word
        bad[second..second + 4].copy_from_slice(&9u32.to_le_bytes());
        let f = parse(&bad);
        assert!(!f.complete);
        assert_eq!(f.categories.len(), 1);
        // a count that promises entries that are not there
        let mut lie = build(&[RawCategory { kind: CategoryKind::Tasks, links: vec![fake_link(b"a")] }]);
        lie[16..20].copy_from_slice(&5u32.to_le_bytes());
        assert!(!parse(&lie).complete);
    }
}
