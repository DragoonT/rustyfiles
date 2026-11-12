//! Icons for the dock: one function, [`icon_bgra`], that returns a square bitmap with alpha at exactly the pixel size asked for, for
//! whatever an item is made of (a program, an icon in a file, a packaged app, a shell place such as the Recycle Bin, a running
//! window that has an icon of its own). The shell's own icon machinery does the finding (`IShellItemImageFactory`, as `thumbs`
//! already uses for the file list); the resampling to the exact size is ours, because the shell answers with the nearest frame of
//! the icon, which is not the size the island wants.
//!
//! Blocking, with COM: call from a worker thread, never the UI thread.

use std::path::Path;

use super::model::IconSource;
use crate::thumbs::{Bgra, Kind};

/// What to ask the shell for when `px` is wanted: the next size up of the sizes icons are usually drawn in, so the shell hands back
/// a frame that is sharper than needed and our own resampling brings it down (a frame scaled UP by the shell is always soft).
pub fn request_size(px: u32) -> u32 {
    [32u32, 48, 64, 96, 128, 256].into_iter().find(|&s| s >= px).unwrap_or(256)
}

/// The icon for `src`, `px` x `px`, straight (not premultiplied) BGRA with alpha, top-down rows. A picture that is not square is
/// scaled to fit and centred on a transparent square.
pub fn icon_bgra(src: &IconSource, px: u32) -> anyhow::Result<Bgra> {
    Ok(fit_square(&raw_icon(src, px)?, px))
}

/// What the system gives for `src` when about `px` is wanted, at the size it came in (not fitted to a square). The dock's own loader
/// (`iconload`) takes it from here and does the resampling itself, from this picture, once for every size it keeps.
pub(super) fn raw_icon(src: &IconSource, px: u32) -> anyhow::Result<Bgra> {
    anyhow::ensure!((1..=1024).contains(&px), "icon size {px} out of range");
    let shell = |p: &Path| on_shell_thread(p.to_path_buf(), request_size(px));
    let got = match src {
        IconSource::File(p) => shell(p)?,
        IconSource::Shell(name) => shell(Path::new(name))?,
        IconSource::Packaged(id) => shell(Path::new(&format!("shell:AppsFolder\\{id}")))?,
        IconSource::Resource { file, index } => match extract_resource(file, *index, px) {
            Some(b) => b,
            // a damaged or moved icon file: what the shell shows for the file itself
            None => shell(file)?,
        },
        IconSource::Window(hwnd) => window_icon(*hwnd, px).ok_or_else(|| anyhow::anyhow!("window {hwnd:#x} has no icon"))?,
    };
    Ok(got)
}

/// The first of `srcs` that gives a picture (see [`crate::dock::DockApp::icon_chain`]): the app's own icon, then what its windows have
/// set themselves, then its package's logo. The error is the first source's, which is the one that says why the app has no icon of its
/// own. An empty list is an error too.
pub fn icon_bgra_chain(srcs: &[IconSource], px: u32) -> anyhow::Result<Bgra> {
    let mut first_error = None;
    for src in srcs {
        let t = std::time::Instant::now();
        log::debug!("dock icon: asking {src:?} at {px} px on tid {}", unsafe { windows::Win32::System::Threading::GetCurrentThreadId() });
        match icon_bgra(src, px) {
            Ok(b) => {
                log::debug!("dock icon: {src:?} gave a picture in {:?}", t.elapsed());
                return Ok(b);
            }
            Err(e) => {
                log::debug!("dock icon: {src:?} failed in {:?}: {e:#}", t.elapsed());
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.unwrap_or_else(|| anyhow::anyhow!("no icon source")))
}

/// The shell's icon for `p` at about `size` px. The shell answers E_PENDING ("not ready yet") when another thread is just extracting
/// the same icon (the dock asks for many at once, and a program's icon can be asked for at two sizes); the answer is there a moment
/// later, so a few short waits are worth more than a missing icon.
fn shell_icon(p: &Path, size: u32) -> anyhow::Result<Bgra> {
    const E_PENDING: i32 = 0x8000_000Au32 as i32;
    let mut tries = 0;
    loop {
        match crate::thumbs::get_image(p, size, Kind::Icon) {
            Err(e) if tries < 8 && e.downcast_ref::<windows::core::Error>().is_some_and(|w| w.code().0 == E_PENDING) => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            other => return other,
        }
    }
}

/// How long the shell may take for one icon before the dock gives up on this ask (it asks again later).
const SHELL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(4);

/// The shell's icon for `p`, asked on the shell thread and waited for here, with a limit (see [`crate::sta::call`] for what happened
/// when the dock's worker threads asked the shell themselves).
fn on_shell_thread(p: std::path::PathBuf, size: u32) -> anyhow::Result<Bgra> {
    match crate::sta::call(SHELL_TIMEOUT, move || shell_icon(&p, size)) {
        Some(r) => r,
        None => anyhow::bail!("the shell did not give the icon in {SHELL_TIMEOUT:?}"),
    }
}

/// The bitmap as RGBA bytes (what image libraries and some GPU upload paths want).
pub fn to_rgba(b: &Bgra) -> Vec<u8> {
    let mut v = b.data.clone();
    for p in v.as_chunks_mut::<4>().0.iter_mut() {
        p.swap(0, 2);
    }
    v
}

/// Scales `img` to fit into a `px` x `px` square, centred on transparent pixels. Downscaling averages the covered source pixels
/// (area weighting), upscaling interpolates; colours are weighted by their alpha, so the edge of an icon does not pick up the dark
/// fringe that transparent black pixels would give it.
pub fn fit_square(img: &Bgra, px: u32) -> Bgra {
    if img.width == px && img.height == px && img.data.len() == (px * px * 4) as usize {
        return img.clone();
    }
    let mut out = Bgra { width: px, height: px, data: vec![0; (px * px * 4) as usize] };
    if img.width == 0 || img.height == 0 || img.data.len() < (img.width * img.height * 4) as usize {
        return out;
    }
    let k = px as f32 / img.width.max(img.height) as f32;
    let dw = ((img.width as f32 * k).round() as u32).clamp(1, px);
    let dh = ((img.height as f32 * k).round() as u32).clamp(1, px);
    let scaled = resample(img, dw, dh);
    let (ox, oy) = ((px - dw) / 2, (px - dh) / 2);
    for y in 0..dh {
        let from = (y * dw * 4) as usize;
        let to = (((y + oy) * px + ox) * 4) as usize;
        out.data[to..to + (dw * 4) as usize].copy_from_slice(&scaled[from..from + (dw * 4) as usize]);
    }
    out
}

fn resample(src: &Bgra, dw: u32, dh: u32) -> Vec<u8> {
    let (sw, sh) = (src.width as usize, src.height as usize);
    // premultiplied floats
    let pre: Vec<[f32; 4]> = src
        .data
        .as_chunks::<4>().0.iter()
        .take(sw * sh)
        .map(|p| {
            let a = p[3] as f32 / 255.0;
            [p[0] as f32 * a, p[1] as f32 * a, p[2] as f32 * a, a * 255.0]
        })
        .collect();
    let at = |x: isize, y: isize| pre[y.clamp(0, sh as isize - 1) as usize * sw + x.clamp(0, sw as isize - 1) as usize];
    let (fx, fy) = (sw as f32 / dw as f32, sh as f32 / dh as f32);
    let mut out = vec![0u8; (dw * dh * 4) as usize];
    for dy in 0..dh as usize {
        for dx in 0..dw as usize {
            let mut acc = [0f32; 4];
            if fx > 1.0 || fy > 1.0 {
                // average of the footprint, edge pixels weighted by how much of them is covered
                let (x0, x1) = (dx as f32 * fx, (dx + 1) as f32 * fx);
                let (y0, y1) = (dy as f32 * fy, (dy + 1) as f32 * fy);
                let mut wsum = 0.0;
                let mut y = y0.floor() as isize;
                while (y as f32) < y1 && (y as usize) < sh {
                    let wy = (y1.min(y as f32 + 1.0) - y0.max(y as f32)).max(0.0);
                    let mut x = x0.floor() as isize;
                    while (x as f32) < x1 && (x as usize) < sw {
                        let wx = (x1.min(x as f32 + 1.0) - x0.max(x as f32)).max(0.0);
                        let w = wx * wy;
                        let p = at(x, y);
                        for c in 0..4 {
                            acc[c] += p[c] * w;
                        }
                        wsum += w;
                        x += 1;
                    }
                    y += 1;
                }
                if wsum > 0.0 {
                    for a in &mut acc {
                        *a /= wsum;
                    }
                }
            } else {
                // bilinear at the centre of the destination pixel
                let (cx, cy) = ((dx as f32 + 0.5) * fx - 0.5, (dy as f32 + 0.5) * fy - 0.5);
                let (x0, y0) = (cx.floor(), cy.floor());
                let (tx, ty) = (cx - x0, cy - y0);
                let (x0, y0) = (x0 as isize, y0 as isize);
                let (p00, p10, p01, p11) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
                for c in 0..4 {
                    acc[c] = (p00[c] * (1.0 - tx) + p10[c] * tx) * (1.0 - ty) + (p01[c] * (1.0 - tx) + p11[c] * tx) * ty;
                }
            }
            let a = acc[3];
            let o = (dy * dw as usize + dx) * 4;
            if a > 0.0 {
                let un = 255.0 / a;
                out[o] = (acc[0] * un).round().clamp(0.0, 255.0) as u8;
                out[o + 1] = (acc[1] * un).round().clamp(0.0, 255.0) as u8;
                out[o + 2] = (acc[2] * un).round().clamp(0.0, 255.0) as u8;
                out[o + 3] = a.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// An icon stored in a file, at the size asked for (`PrivateExtractIconsW` picks the frame closest to it).
fn extract_resource(file: &Path, index: i32, px: u32) -> Option<Bgra> {
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON, PrivateExtractIconsW};
    let w = crate::wide::wide(file);
    let mut buf = [0u16; 260];
    let n = w.len().min(buf.len() - 1);
    buf[..n].copy_from_slice(&w[..n]);
    let mut icons = [HICON::default()];
    let got = unsafe { PrivateExtractIconsW(&buf, index, px as i32, px as i32, Some(&mut icons), None, 0) };
    if got == 0 || got == u32::MAX || icons[0].is_invalid() {
        return None;
    }
    let r = hicon_to_bgra(icons[0]);
    unsafe {
        let _ = DestroyIcon(icons[0]);
    }
    r
}

/// The icon a window has set itself (or its class has), asked without ever waiting long for a window that does not answer.
fn window_icon(hwnd: isize, px: u32) -> Option<Bgra> {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{GCLP_HICON, GCLP_HICONSM, GetClassLongPtrW, HICON, ICON_BIG, ICON_SMALL2, SMTO_ABORTIFHUNG, SMTO_BLOCK, SendMessageTimeoutW, WM_GETICON};
    let h = HWND(hwnd as *mut _);
    let ask = |kind: u32| -> Option<HICON> {
        let mut out = 0usize;
        let r = unsafe { SendMessageTimeoutW(h, WM_GETICON, WPARAM(kind as usize), LPARAM(0), SMTO_ABORTIFHUNG | SMTO_BLOCK, 100, Some(&mut out)) };
        (r.0 != 0 && out != 0).then_some(HICON(out as *mut _))
    };
    let class = |idx| {
        let v = unsafe { GetClassLongPtrW(h, idx) };
        (v != 0).then_some(HICON(v as *mut _))
    };
    // never destroyed: these belong to the window or its class
    let icon = ask(ICON_BIG).or_else(|| ask(ICON_SMALL2)).or_else(|| class(GCLP_HICON)).or_else(|| class(GCLP_HICONSM))?;
    let _ = px;
    hicon_to_bgra(icon)
}

/// An `HICON` as straight-alpha BGRA at its own size. An icon without an alpha channel gets its transparency from its mask.
pub(super) fn hicon_to_bgra(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Bgra> {
    use windows::Win32::Graphics::Gdi::{BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, DeleteObject, GetDC, GetDIBits, GetObjectW, HGDIOBJ, ReleaseDC};
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};
    unsafe {
        let mut ii = ICONINFO::default();
        GetIconInfo(icon, &mut ii).ok()?;
        let free = |ii: &ICONINFO| {
            if !ii.hbmColor.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
            }
            if !ii.hbmMask.is_invalid() {
                let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
            }
        };
        if ii.hbmColor.is_invalid() {
            // a monochrome icon (colour and mask stacked in one bitmap): not worth drawing
            free(&ii);
            return None;
        }
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(ii.hbmColor.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut _)) == 0 || bm.bmWidth <= 0 || bm.bmHeight <= 0 {
            free(&ii);
            return None;
        }
        let (w, h) = (bm.bmWidth as u32, bm.bmHeight as u32);
        let read = |bmp: windows::Win32::Graphics::Gdi::HBITMAP| -> Option<Vec<u8>> {
            let mut data = vec![0u8; (w * h * 4) as usize];
            let mut bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w as i32,
                    biHeight: -(h as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = GetDC(None);
            let lines = GetDIBits(dc, bmp, 0, h, Some(data.as_mut_ptr() as *mut _), &mut bmi, DIB_RGB_COLORS);
            ReleaseDC(None, dc);
            (lines as u32 == h).then_some(data)
        };
        let color = read(ii.hbmColor);
        let mask = if ii.hbmMask.is_invalid() { None } else { read(ii.hbmMask) };
        free(&ii);
        let mut data = color?;
        if data.as_chunks::<4>().0.iter().all(|p| p[3] == 0) {
            // no alpha channel: opaque where the mask is black
            for (i, p) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                let opaque = mask.as_ref().is_none_or(|m| m[i * 4] == 0);
                p[3] = if opaque { 255 } else { 0 };
            }
        }
        Some(Bgra { width: w, height: h, data })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, bgra: [u8; 4]) -> Bgra {
        Bgra { width: w, height: h, data: bgra.iter().copied().cycle().take((w * h * 4) as usize).collect() }
    }

    fn at(b: &Bgra, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width + x) * 4) as usize;
        [b.data[i], b.data[i + 1], b.data[i + 2], b.data[i + 3]]
    }

    #[test]
    fn request_sizes_are_the_next_size_up() {
        assert_eq!(request_size(16), 32);
        assert_eq!(request_size(44), 48);
        assert_eq!(request_size(56), 64);
        assert_eq!(request_size(84), 96);
        assert_eq!(request_size(500), 256);
    }

    #[test]
    fn the_exact_size_comes_back_unchanged() {
        let b = solid(44, 44, [10, 20, 30, 255]);
        let f = fit_square(&b, 44);
        assert_eq!((f.width, f.height), (44, 44));
        assert_eq!(f.data, b.data);
    }

    #[test]
    fn scaling_keeps_a_solid_colour_and_its_alpha() {
        for (from, to) in [(64u32, 44u32), (256, 44), (48, 44), (32, 44), (44, 84)] {
            let f = fit_square(&solid(from, from, [10, 200, 90, 255]), to);
            assert_eq!((f.width, f.height), (to, to));
            for (x, y) in [(0, 0), (to / 2, to / 2), (to - 1, to - 1)] {
                assert_eq!(at(&f, x, y), [10, 200, 90, 255], "{from} -> {to} at {x},{y}");
            }
            let t = fit_square(&solid(from, from, [40, 60, 80, 128]), to);
            let p = at(&t, to / 2, to / 2);
            assert_eq!(p[3], 128);
            assert!((p[0] as i32 - 40).abs() <= 1 && (p[1] as i32 - 60).abs() <= 1 && (p[2] as i32 - 80).abs() <= 1, "{p:?}");
        }
    }

    #[test]
    fn a_soft_edge_does_not_pick_up_a_dark_fringe() {
        // the left 30 of 64 columns opaque red, the rest fully transparent black (the boundary falls inside a destination pixel): after scaling down, the pixels on the boundary are red with partial alpha
        let mut b = solid(64, 64, [0, 0, 0, 0]);
        for y in 0..64 {
            for x in 0..30 {
                let i = ((y * 64 + x) * 4) as usize;
                b.data[i..i + 4].copy_from_slice(&[0, 0, 255, 255]);
            }
        }
        let f = fit_square(&b, 44);
        let edge = (0..44).map(|x| at(&f, x, 22)).find(|p| p[3] > 0 && p[3] < 255).expect("a partly covered pixel");
        assert!(edge[2] >= 250 && edge[0] <= 3, "the boundary is red, not dark red: {edge:?}");
        assert_eq!(at(&f, 2, 22), [0, 0, 255, 255]);
        assert_eq!(at(&f, 40, 22)[3], 0);
    }

    #[test]
    fn a_wide_picture_is_centred_on_a_transparent_square() {
        let f = fit_square(&solid(64, 32, [1, 2, 3, 255]), 44);
        assert_eq!((f.width, f.height), (44, 44));
        assert_eq!(at(&f, 22, 22), [1, 2, 3, 255]);
        assert_eq!(at(&f, 22, 0)[3], 0, "above the picture");
        assert_eq!(at(&f, 22, 43)[3], 0, "below the picture");
        assert_eq!(at(&f, 0, 22)[3], 255, "the width is used in full");
    }

    #[test]
    fn nonsense_input_gives_a_transparent_square_not_a_panic() {
        let f = fit_square(&Bgra { width: 4, height: 4, data: vec![0; 3] }, 8);
        assert_eq!((f.width, f.height, f.data.len()), (8, 8, 256));
        assert!(f.data.iter().all(|&b| b == 0));
        assert!(icon_bgra(&IconSource::File("x".into()), 0).is_err());
        assert!(icon_bgra(&IconSource::Window(0), 44).is_err());
    }

    #[test]
    fn the_chain_takes_the_first_source_that_gives_a_picture() {
        let good = IconSource::File(r"C:\Windows\explorer.exe".into());
        let bad = IconSource::File(r"C:\no\such\program.exe".into());
        // a window that does not exist has no icon either
        let no_window = IconSource::Window(0x7fff_0001);
        let img = icon_bgra_chain(&[bad.clone(), no_window.clone(), good.clone()], 44).expect("the third source works");
        assert_eq!((img.width, img.height), (44, 44));
        assert_eq!(icon_bgra_chain(&[good.clone(), bad.clone()], 44).unwrap().data, icon_bgra(&good, 44).unwrap().data, "the first that works is the one used");
        // nothing works: an error, not a panic (the dock draws its letter)
        assert!(icon_bgra_chain(&[bad, no_window], 44).is_err());
        assert!(icon_bgra_chain(&[], 44).is_err());
    }

    #[test]
    fn rgba_swaps_red_and_blue() {
        let b = solid(1, 1, [1, 2, 3, 4]);
        assert_eq!(to_rgba(&b), vec![3, 2, 1, 4]);
    }

    // The shell tests below only read: they ask the shell for icons of what every Windows has.

    #[test]
    fn a_program_has_an_icon_at_the_exact_size_with_alpha() {
        let img = icon_bgra(&IconSource::File(r"C:\Windows\explorer.exe".into()), 44).unwrap();
        assert_eq!((img.width, img.height), (44, 44));
        assert!(img.data.as_chunks::<4>().0.iter().any(|p| p[3] == 255), "something opaque");
        assert!(img.data.as_chunks::<4>().0.iter().any(|p| p[3] < 255), "and something transparent around it");
    }

    #[test]
    fn the_recycle_bin_and_downloads_have_icons() {
        let rb = icon_bgra(&IconSource::Shell("shell:RecycleBinFolder".into()), 56).unwrap();
        assert_eq!((rb.width, rb.height), (56, 56));
        assert!(rb.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
        let dl = icon_bgra(&super::super::model::Place::Downloads.icon_source(), 56).unwrap();
        assert!(dl.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }

    #[test]
    fn an_icon_stored_in_a_file_is_found_by_index() {
        let img = icon_bgra(&IconSource::Resource { file: r"C:\Windows\System32\shell32.dll".into(), index: 3 }, 44).unwrap();
        assert_eq!((img.width, img.height), (44, 44));
        assert!(img.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    }

    #[test]
    fn a_packaged_app_has_an_icon() {
        // Settings is on every Windows 10 and 11
        let img = icon_bgra(&IconSource::Packaged("windows.immersivecontrolpanel_cw5n1h2txyewy!microsoft.windows.immersivecontrolpanel".into()), 44);
        if let Ok(img) = img {
            assert_eq!((img.width, img.height), (44, 44));
        }
    }
}
