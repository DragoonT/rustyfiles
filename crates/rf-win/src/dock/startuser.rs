//! The signed-in user's account picture, for the round picture at the left of the Start panel's bottom row.
//!
//! HOW WINDOWS KEEPS IT. For each account Windows writes the picture it shows in several sizes as ordinary image files and lists them
//! in `HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\<SID>`, one value per size (`Image32`,
//! `Image40`, `Image48`, `Image64`, `Image96`, `Image192`, `Image208`, `Image240`, `Image424`, `Image448`, `Image1080`), each holding
//! the file's path. Any user may read that key and the files (it is what the sign-in screen and Settings read), so nothing new is
//! asked for, and nothing is written. An account that never chose a picture has no such key; the panel then shows the initial.
//!
//! The size is chosen by [`order`]: the smallest that is at least as big as the picture is drawn (a bigger one is scaled down sharply,
//! a smaller one would be soft), the others after it in case that file is gone. Decoding is the `image` crate's. Blocks (the
//! registry, the disk, decoding): a worker thread.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HLOCAL, LocalFree};
use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::thumbs::Bgra;

/// The sizes Windows writes (the suffix of the `ImageNN` value names).
pub const SIZES: [u32; 11] = [32, 40, 48, 64, 96, 192, 208, 240, 424, 448, 1080];

/// A picture file bigger than this is not read (the biggest size, 1080 px, is a few hundred KB).
const MAX_FILE: u64 = 8 * 1024 * 1024;

/// The key of one account.
pub fn key_path(sid: &str) -> String {
    format!(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\{sid}")
}

/// The value that holds the path of the picture of `size` px.
pub fn value_name(size: u32) -> String {
    format!("Image{size}")
}

/// The sizes of `available` in the order they are tried for a picture drawn `want` px square: the smallest ones that are big enough
/// first (smallest to biggest), then the smaller ones (biggest to smallest).
pub fn order(available: &[u32], want: u32) -> Vec<u32> {
    let mut big: Vec<u32> = available.iter().copied().filter(|&s| s >= want).collect();
    let mut small: Vec<u32> = available.iter().copied().filter(|&s| s < want).collect();
    big.sort_unstable();
    big.dedup();
    small.sort_unstable_by(|a, b| b.cmp(a));
    small.dedup();
    big.extend(small);
    big
}

/// Whether the file named by a registry value is one to read: a picture format we decode, by its extension.
pub fn is_picture_name(path: &str) -> bool {
    let l = path.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".bmp"].iter().any(|e| l.ends_with(e))
}

/// The biggest square that fits in a `w` x `h` picture, centred: (left, top, side). Account pictures are square already; one that is
/// not is cropped, not squeezed.
pub fn square_crop(w: u32, h: u32) -> (u32, u32, u32) {
    let side = w.min(h);
    ((w - side) / 2, (h - side) / 2, side)
}

/// Decodes a picture's bytes to a `px` x `px` straight-alpha BGRA bitmap (cropped to a square first).
pub fn decode_square(bytes: &[u8], px: u32) -> Option<Bgra> {
    if !(1..=1024).contains(&px) {
        return None;
    }
    let rgba = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = rgba.dimensions();
    if w == 0 || h == 0 {
        return None;
    }
    let (x, y, side) = square_crop(w, h);
    let cropped = image::imageops::crop_imm(&rgba, x, y, side, side).to_image();
    let mut data = cropped.into_raw();
    for p in data.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
    Some(crate::dock::icons::fit_square(&Bgra { width: side, height: side, data }, px))
}

/// The text of the signed-in user's SID ("S-1-5-21-..."), from the process's token.
pub fn current_sid() -> Option<String> {
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let sid = sid_of_token(token);
        let _ = CloseHandle(token);
        sid
    }
}

unsafe fn sid_of_token(token: HANDLE) -> Option<String> {
    unsafe {
        let mut need = 0u32;
        // (the first call only says how big the answer is)
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut need);
        if need == 0 || need > 4096 {
            return None;
        }
        // (8-byte aligned: TOKEN_USER holds a pointer)
        let mut buf = vec![0u64; (need as usize).div_ceil(8)];
        GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), need, &mut need).ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut text = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut text).ok()?;
        let s = text.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(text.0.cast())));
        s
    }
}

/// The paths Windows lists for this account, by size (only the sizes it has a value for). Reads the registry.
fn listed(sid: &str) -> Vec<(u32, String)> {
    let Ok(key) = windows_registry::LOCAL_MACHINE.open(key_path(sid)) else { return Vec::new() };
    SIZES.iter().filter_map(|&s| key.get_string(value_name(s)).ok().filter(|p| is_picture_name(p)).map(|p| (s, p))).collect()
}

/// The user's account picture as a `px` x `px` BGRA bitmap, or `None` when the account has none (or it cannot be read). Blocks.
pub fn account_picture(px: u32) -> Option<Bgra> {
    let sid = current_sid()?;
    let files = listed(&sid);
    let sizes: Vec<u32> = files.iter().map(|(s, _)| *s).collect();
    for size in order(&sizes, px) {
        let Some((_, path)) = files.iter().find(|(s, _)| *s == size) else { continue };
        let Ok(meta) = std::fs::metadata(path) else { continue };
        if !meta.is_file() || meta.len() > MAX_FILE {
            continue;
        }
        if let Some(b) = std::fs::read(path).ok().and_then(|bytes| decode_square(&bytes, px)) {
            return Some(b);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_smallest_picture_that_is_big_enough_is_tried_first_then_the_smaller_ones_biggest_first() {
        let all = SIZES;
        assert_eq!(order(&all, 28), [32, 40, 48, 64, 96, 192, 208, 240, 424, 448, 1080, ]);
        assert_eq!(order(&all, 56)[..3], [64, 96, 192]);
        assert_eq!(order(&all, 56)[order(&all, 56).len() - 3..], [48, 40, 32], "the smaller ones last, the biggest of them first");
        assert_eq!(order(&[96, 32], 70), [96, 32]);
        assert_eq!(order(&[32, 48], 70), [48, 32], "all too small: the biggest first");
        assert_eq!(order(&[64], 64), [64], "an exact size");
        assert!(order(&[], 28).is_empty());
        assert_eq!(order(&[96, 96, 32], 28), [32, 96], "no repeats");
    }

    #[test]
    fn the_registry_names_are_the_ones_windows_writes() {
        assert_eq!(value_name(96), "Image96");
        assert_eq!(key_path("S-1-5-21-1-2-3-1001"), r"SOFTWARE\Microsoft\Windows\CurrentVersion\AccountPicture\Users\S-1-5-21-1-2-3-1001");
    }

    #[test]
    fn only_picture_files_are_read() {
        assert!(is_picture_name(r"C:\Users\Public\AccountPictures\S-1\x-Image96.jpg"));
        assert!(is_picture_name(r"C:\a\b.PNG"));
        assert!(is_picture_name(r"C:\a\b.bmp"));
        assert!(!is_picture_name(r"C:\a\b.accountpicture-ms"));
        assert!(!is_picture_name(r"C:\Windows\System32\cmd.exe"));
        assert!(!is_picture_name(""));
    }

    #[test]
    fn a_picture_that_is_not_square_is_cropped_in_the_middle() {
        assert_eq!(square_crop(448, 448), (0, 0, 448));
        assert_eq!(square_crop(300, 200), (50, 0, 200));
        assert_eq!(square_crop(200, 301), (0, 50, 200));
    }

    #[test]
    fn a_picture_is_decoded_to_the_size_asked_with_the_colours_in_bgra_order() {
        // a 4 x 2 picture: the left half red, the right half blue, fully opaque
        let mut img = image::RgbaImage::new(4, 2);
        for (x, _, p) in img.enumerate_pixels_mut() {
            *p = if x < 2 { image::Rgba([255, 0, 0, 255]) } else { image::Rgba([0, 0, 255, 255]) };
        }
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).unwrap();
        let b = decode_square(&bytes, 2).expect("decodes");
        assert_eq!((b.width, b.height, b.data.len()), (2, 2, 16));
        // the square cut from the middle of 4 x 2 is the 2 x 2 in the middle: one red column and one blue column
        assert_eq!(&b.data[0..4], [0, 0, 255, 255], "red, as blue-green-red-alpha");
        assert_eq!(&b.data[4..8], [255, 0, 0, 255], "blue, as blue-green-red-alpha");
        assert!(decode_square(b"not a picture", 32).is_none());
        assert!(decode_square(&bytes, 0).is_none());
        assert!(decode_square(&bytes, 5000).is_none());
    }

    /// Reads this account's picture if it has one and says its size; changes nothing. Needs the account's own session.
    #[test]
    fn reads_this_accounts_picture_if_there_is_one() {
        let sid = current_sid();
        eprintln!("sid read: {}", sid.is_some());
        let sid = sid.expect("the process's token has a user");
        assert!(sid.starts_with("S-1-"));
        let listed_sizes: Vec<u32> = listed(&sid).iter().map(|(s, _)| *s).collect();
        eprintln!("sizes listed by Windows: {}", listed_sizes.len());
        let t = std::time::Instant::now();
        match account_picture(56) {
            Some(b) => {
                eprintln!("account picture read: {} x {} in {} ms", b.width, b.height, t.elapsed().as_millis());
                assert_eq!((b.width, b.height, b.data.len()), (56, 56, 56 * 56 * 4));
            }
            None => eprintln!("no account picture ({} sizes listed)", listed_sizes.len()),
        }
    }
}
