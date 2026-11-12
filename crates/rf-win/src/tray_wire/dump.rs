//! Text forms of a block for logs: a hex dump and a one-line description of what the parser made of it. Used by the `tray_spy` example;
//! pure so the format is tested. The descriptions contain the icons' tips and balloon texts (whatever the programs send), so a log made
//! from them is the user's data: keep it local.

use super::notify::{nif_names, Nim};
use super::{TrayRequest, WireError, parse_copydata};
use std::fmt::Write;

/// First line of every log the tray spy writes. A log path that already holds something else is refused: the spy appends to its own
/// logs (several runs, one file) and must never append to somebody's document because of a mistyped path.
pub const LOG_HEADER: &str = "# rf-win tray_spy log";

/// Whether a file whose first bytes are `head` may be appended to as a spy log: empty (or new), or starting with [`LOG_HEADER`].
pub fn existing_log_acceptable(head: &[u8]) -> bool {
    head.is_empty() || head.starts_with(LOG_HEADER.as_bytes())
}

/// Classic hex dump, 16 bytes per line (`offset  hex  |ascii|`), at most `max_bytes` bytes; the rest is summarised in one line.
pub fn hex_dump(bytes: &[u8], max_bytes: usize) -> String {
    let shown = &bytes[..bytes.len().min(max_bytes)];
    let mut out = String::new();
    for (i, chunk) in shown.chunks(16).enumerate() {
        let _ = write!(out, "{:04x} ", i * 16);
        for j in 0..16 {
            match chunk.get(j) {
                Some(b) => {
                    let _ = write!(out, " {b:02x}");
                }
                None => out.push_str("   "),
            }
            if j == 7 {
                out.push(' ');
            }
        }
        out.push_str("  |");
        out.extend(chunk.iter().map(|&b| if (0x20..0x7F).contains(&b) { b as char } else { '.' }));
        out.push_str("|\n");
    }
    if bytes.len() > shown.len() {
        let _ = writeln!(out, "... {} more bytes not shown", bytes.len() - shown.len());
    }
    out
}

/// Quotes a text for a log line: escapes quotes, backslashes and control characters, cuts at `max_chars`.
fn quoted(s: &str, max_chars: usize) -> String {
    let mut out = String::from("\"");
    for (n, c) in s.chars().enumerate() {
        if n >= max_chars {
            out.push_str("...");
            break;
        }
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{{{:x}}}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One line saying what a parsed block is.
pub fn describe(req: &TrayRequest) -> String {
    match req {
        TrayRequest::Icon(r) => {
            let d = &r.data;
            let mut s = format!(
                "{} hwnd={:#x} uid={} flags={} eff={} cb={} tier={:?}",
                r.message.name(),
                d.hwnd,
                d.uid,
                nif_names(d.flags),
                nif_names(d.effective_flags()),
                d.cb_size,
                d.tier
            );
            let layouts = d.sender_layouts();
            if !layouts.is_empty() {
                let _ = write!(s, " cbSize-implies={}", layouts.iter().map(|l| format!("{}bit/{:?}", l.bits, l.variant)).collect::<Vec<_>>().join("|"));
            }
            if let Some(cb) = d.callback() {
                let _ = write!(s, " callback={cb:#x}");
            }
            if let Some(i) = d.icon_handle() {
                let _ = write!(s, " icon={i:#x}");
            }
            if let Some(t) = d.tip_text() {
                let _ = write!(s, " tip={}", quoted(t, 80));
            }
            if let Some(st) = d.state_update() {
                let _ = write!(s, " state={:#x}/mask={:#x}", st.state, st.mask);
            }
            if let Some(b) = d.balloon() {
                let _ = write!(s, " balloon[{:?} title={} text={}]", b.kind, quoted(&b.title, 40), quoted(&b.text, 80));
            }
            if let super::IconKey::Guid(g) = d.key() {
                let _ = write!(s, " guid={g}");
            }
            if r.message == Nim::SetVersion {
                let _ = write!(s, " version={}", d.timeout_or_version);
            }
            if r.trailing_len > 0 {
                let _ = write!(s, " trailing={}B", r.trailing_len);
                if let Some(w) = r.trailing_word {
                    let _ = write!(s, " (first word {w})");
                }
            }
            s
        }
        TrayRequest::AppBar(a) => format!(
            "{} hwnd={:#x} edge={:?} rect=({},{},{},{}) lparam={:#x} callback={:#x} pid={} shared={:#x} layout={:?} cbSize={:#x}",
            a.message.name(),
            a.hwnd,
            a.edge_kind(),
            a.rect.left,
            a.rect.top,
            a.rect.right,
            a.rect.bottom,
            a.lparam,
            a.callback_message,
            a.source_pid,
            a.shared_memory,
            a.layout,
            a.abd_cb_size
        ),
        TrayRequest::ServiceObject(r) => format!("service-object clsid={} action={:?}", r.clsid, r.action),
        TrayRequest::IconRect(q) => format!(
            "icon-rect message={} key={:?} magic={:#x} cbSize={}",
            q.message,
            q.key(),
            q.magic,
            q.cb_size
        ),
    }
}

/// Parses `bytes` as `dw_data` and describes the result, or the reason it was not understood.
pub fn describe_block(dw_data: usize, bytes: &[u8]) -> String {
    match parse_copydata(dw_data, bytes) {
        Ok(r) => describe(&r),
        Err(e) => describe_error(dw_data, &e),
    }
}

fn describe_error(dw_data: usize, e: &WireError) -> String {
    format!("UNPARSED dwData={dw_data}: {e}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_empty_or_own_logs_are_appended_to() {
        assert!(existing_log_acceptable(b""));
        assert!(existing_log_acceptable(LOG_HEADER.as_bytes()));
        assert!(existing_log_acceptable(format!("{LOG_HEADER}
[+0.000s] started
").as_bytes()));
        assert!(!existing_log_acceptable(b"Dear all,
"));
        assert!(!existing_log_acceptable(b"PK"));
        assert!(!existing_log_acceptable(&LOG_HEADER.as_bytes()[..LOG_HEADER.len() - 1]), "a prefix of the header is not the header");
        assert!(!existing_log_acceptable(b" # rf-win tray_spy log"));
    }
    use crate::tray_wire::notify::tests_support::golden_full;
    use crate::tray_wire::{appbar, rect, service, XorShift};

    #[test]
    fn hex_dump_format() {
        let d = hex_dump(&[0x23, 0x34, 0x75, 0x34, b'A', b'z', 0x00, 0x7F, 0x80], 100);
        assert_eq!(d, "0000  23 34 75 34 41 7a 00 7f  80                       |#4u4Az...|\n");
    }

    #[test]
    fn hex_dump_wraps_at_16_and_caps() {
        let bytes: Vec<u8> = (0..40u8).collect();
        let d = hex_dump(&bytes, 100);
        assert_eq!(d.lines().count(), 3);
        assert!(d.lines().nth(1).unwrap().starts_with("0010  10 11"));
        let capped = hex_dump(&bytes, 20);
        assert_eq!(capped.lines().count(), 3, "two rows and the summary");
        assert!(capped.ends_with("... 20 more bytes not shown\n"));
        assert_eq!(hex_dump(&[], 10), "");
        assert_eq!(hex_dump(&bytes, 0), "... 40 more bytes not shown\n");
    }

    #[test]
    fn descriptions_of_each_kind() {
        let d = describe_block(1, &golden_full());
        assert!(d.starts_with("NIM_ADD hwnd=0x10abc uid=7 flags=MESSAGE|ICON|TIP|STATE|INFO|GUID"), "{d}");
        assert!(d.contains("tip=\"Tip text\""), "{d}");
        assert!(d.contains("guid={7820AE73-23E3-4229-82C1-E41CB67D5B9C}"), "{d}");
        assert!(d.contains("cbSize-implies=32bit/Full"), "{d}");
        assert!(d.contains("balloon[Info"), "{d}");
        let d = describe_block(0, &appbar::tests_support::golden_v3());
        assert!(d.starts_with("ABM_SETPOS hwnd=0x10def edge=Bottom rect=(-1920,1392,0,1440)"), "{d}");
        let d = describe_block(2, &service::tests_support::golden());
        assert!(d.contains("{35CEC8A3-2BE6-11D2-8773-92E220524153}") && d.contains("Enable"), "{d}");
        let d = describe_block(3, &rect::tests_support::golden());
        assert!(d.starts_with("icon-rect message=1"), "{d}");
    }

    #[test]
    fn errors_are_described_not_hidden() {
        assert!(describe_block(1, &[1, 2, 3]).starts_with("UNPARSED dwData=1: "));
        assert!(describe_block(9, &[]).contains("unknown dwData 9"));
        assert!(describe_block(1, &[0; 40]).contains("bad signature"));
    }

    #[test]
    fn hostile_text_is_escaped() {
        let mut raw = golden_full();
        let evil = "a\"b\\c\nd\u{7}";
        raw[0x20..0x20 + 256].fill(0);
        crate::tray_wire::notify::tests_support::put_wide(&mut raw, 0x20, evil);
        let d = describe_block(1, &raw);
        assert!(d.contains("tip=\"a\\\"b\\\\c\\u{a}d\\u{7}\""), "{d}");
        assert_eq!(d.lines().count(), 1, "one line per block, whatever the text holds");
    }

    #[test]
    fn long_text_is_cut() {
        let mut raw = golden_full();
        raw[0x20..0x20 + 256].fill(0);
        crate::tray_wire::notify::tests_support::put_wide(&mut raw, 0x20, &"x".repeat(120));
        let d = describe_block(1, &raw);
        assert!(d.contains(&format!("tip=\"{}...\"", "x".repeat(80))), "{d}");
    }

    #[test]
    fn describing_random_blocks_never_panics_and_stays_one_line() {
        let mut rng = XorShift(31337);
        for _ in 0..5000 {
            let n = rng.below(1100) as usize;
            let mut b = 0x3475_3423u32.to_le_bytes().to_vec();
            b.extend_from_slice(&(rng.below(5) as u32).to_le_bytes());
            b.extend(rng.bytes(n));
            let d = describe_block(rng.below(5) as usize, &b);
            assert_eq!(d.lines().count(), 1, "{d:?}");
            let _ = hex_dump(&b, 64);
        }
    }
}
