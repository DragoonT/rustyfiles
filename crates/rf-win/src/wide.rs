use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

/// NUL-terminated UTF-16.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// [`wide`] for a name given to the shell (`SHParseDisplayName`, `SHCreateItemFromParsingName`): an FTP address with its backslashes
/// turned into slashes. `Path::join` puts a backslash before each name it adds, and the shell refuses `ftp://host:port\DCIM` with
/// "The parameter is incorrect" (0x80070057), so dragging, copying and the menu failed on FTP while the list worked (issue 223).
pub fn shell_name(s: impl AsRef<OsStr>) -> Vec<u16> {
    let mut w: Vec<u16> = s.as_ref().encode_wide().collect();
    let ftp: Vec<u16> = "ftp://".encode_utf16().collect();
    let is_ftp = w.len() > ftp.len() && w[..ftp.len()].iter().zip(&ftp).all(|(a, b)| char::from_u32(u32::from(*a)).is_some_and(|c| c.to_ascii_lowercase() as u32 == u32::from(*b)));
    if is_ftp {
        for c in w.iter_mut() {
            if *c == u16::from(b'\\') {
                *c = u16::from(b'/');
            }
        }
    }
    w.push(0);
    w
}

/// UTF-16 buffer (possibly NUL-terminated) → String.
pub fn from_wide(buf: &[u16]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..len])
}

/// Adds the `\\?\` prefix so paths longer than MAX_PATH work. With that prefix Windows
/// takes the path literally, so `/` separators are turned into backslashes first.
pub fn long_path(p: &Path) -> Vec<u16> {
    let s = p.as_os_str();
    let mut raw: Vec<u16> = s.encode_wide().collect();
    for c in raw.iter_mut() {
        if *c == u16::from(b'/') {
            *c = u16::from(b'\\');
        }
    }
    let has_prefix = raw.starts_with(&[b'\\' as u16, b'\\' as u16, b'?' as u16, b'\\' as u16]);
    let is_unc = raw.starts_with(&[b'\\' as u16, b'\\' as u16]);
    let mut out: Vec<u16> = Vec::with_capacity(raw.len() + 9);
    if !has_prefix {
        if is_unc {
            out.extend(r"\\?\UNC".encode_utf16());
            out.extend_from_slice(&raw[1..]); // keep one leading backslash
        } else {
            out.extend(r"\\?\".encode_utf16());
            out.extend_from_slice(&raw);
        }
    } else {
        out.extend_from_slice(&raw);
    }
    out.push(0);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ftp_address_reaches_the_shell_with_slashes_and_a_path_as_it_is() {
        let text = |w: Vec<u16>| from_wide(&w);
        assert_eq!(text(shell_name(r"ftp://192.168.1.5:2221\DCIM\a.jpg")), "ftp://192.168.1.5:2221/DCIM/a.jpg");
        assert_eq!(text(shell_name(r"FTP://h\x")), "FTP://h/x");
        assert_eq!(text(shell_name(r"C:\Users\x")), r"C:\Users\x");
        assert_eq!(text(shell_name(r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\x")), r"::{20D04FE0-3AEA-1069-A2D8-08002B30309D}\x");
    }
}
