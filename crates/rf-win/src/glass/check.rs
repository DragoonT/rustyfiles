//! A windowless manual check of the capture session backend, for `examples/glass_capture_check.rs`. It is not a test and not part of the
//! app: it opens the engine's own session code (`wgc.rs`) on one monitor, puts the windows it is given into the exclusion list,
//! waits for the first picture made with that list and hands the mirror back. It creates no window, no swap chain, no picker, and asks
//! the system for nothing: the border-free permission is only READ, and nothing is opened unless it says Allowed.

use std::time::{Duration, Instant};

use windows::Win32::Graphics::Direct3D11::{D3D11_CPU_ACCESS_READ, D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_STAGING};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTOPRIMARY, MonitorFromPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetThreadDpiAwarenessContext};

use super::capture::{MonError, Monitor, Pumped, locate};
use super::engine::Apartment;
use super::gpu::Gpu;
use super::wgc::{self, Exclusion};

/// What one run of the check saw.
pub struct Report {
    pub width: u32,
    pub height: u32,
    /// The mirror: `width` x `height` BGRA, tightly packed.
    pub bgra: Vec<u8>,
    /// The captured monitor in desktop coordinates: left, top, right, bottom.
    pub monitor: (i32, i32, i32, i32),
    /// Making the session (device, pool, session, start).
    pub open: Duration,
    /// The call that sent the exclusion list.
    pub list_call: Duration,
    /// From the start of the capture to the first picture made with the list.
    pub first_image: Duration,
    /// Frames the pool delivered meanwhile (stale ones included).
    pub frames: u32,
    /// How many window ids the session reports back as its exclusion list.
    pub reported: Option<usize>,
}

fn why(e: MonError) -> String {
    match e {
        MonError::Device(s) | MonError::Monitor(s) | MonError::Backend(s) => s,
    }
}

/// The WinRT apartment the check needs: held by the caller for the whole check, like the engine thread holds it for its life (leaving it
/// between two runs tears the system's capture machinery down while it is still releasing the first session's objects).
pub struct ApartmentGuard(#[allow(dead_code)] Apartment);

pub fn enter_apartment() -> ApartmentGuard {
    ApartmentGuard(Apartment::enter())
}

/// Captures the monitor that contains the desktop point `monitor_at` (the primary one when it is on none) through a session that leaves `excluded` (window handles) out, and returns the picture. `Err` when the
/// permission is not Allowed (nothing was opened then) or anything fails.
pub fn run(excluded: &[isize], max_wait: Duration, monitor_at: (i32, i32)) -> Result<Report, String> {
    unsafe {
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        if let Some(w) = wgc::access().why_not() {
            return Err(format!("not opened: {w}"));
        }
        let hmon = MonitorFromPoint(POINT { x: monitor_at.0, y: monitor_at.1 }, MONITOR_DEFAULTTOPRIMARY).0 as isize;
        let loc = locate(hmon).map_err(|e| format!("the monitor is not among the DXGI outputs: {e}"))?;
        let g = Gpu::on_adapter(&loc.adapter).map_err(|e| format!("the D3D11 device: {e}"))?;

        let t0 = Instant::now();
        let mut mon = Monitor::open_session(&g, hmon)?;
        let open = t0.elapsed();
        log::info!("glass: screen capture through a Windows.Graphics.Capture session (manual check)");

        let mut ex = Exclusion::default();
        for &h in excluded {
            ex.attach(h);
            ex.set_visible(h, true);
        }
        let start = Instant::now();
        let (mut frames, mut first_image) = (0u32, None);
        while start.elapsed() < max_wait && first_image.is_none() {
            match mon.pump(&g, 50, &ex, None).map_err(why)? {
                Pumped::Waited { frame, image, .. } => {
                    frames += frame as u32;
                    if image {
                        first_image = Some(start.elapsed());
                    }
                }
                Pumped::NoWait => {}
            }
        }
        let first_image = first_image.ok_or("no picture made with the exclusion list arrived in time (a static screen sends no frame by itself)")?;
        let (list_call, reported) = match &mon {
            Monitor::Session(s) => (s.last_set_time, s.reported_list().map(|l| l.len())),
            Monitor::Dup(_) => (Duration::ZERO, None),
        };
        let m = mon.mirror().ok_or("no mirror")?;
        let mut d = D3D11_TEXTURE2D_DESC::default();
        m.tex.GetDesc(&mut d);
        d.Usage = D3D11_USAGE_STAGING;
        d.BindFlags = 0;
        d.MiscFlags = 0;
        d.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        g.dev.CreateTexture2D(&d, None, Some(&mut staging)).map_err(|e| format!("staging texture: {e}"))?;
        let staging = staging.ok_or("no staging texture")?;
        g.ctx.CopyResource(&staging, &m.tex);
        let mut map = D3D11_MAPPED_SUBRESOURCE::default();
        g.ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut map)).map_err(|e| format!("reading the mirror back: {e}"))?;
        let (w, h) = (d.Width as usize, d.Height as usize);
        let mut bgra = vec![0u8; w * h * 4];
        for y in 0..h {
            let row = std::slice::from_raw_parts((map.pData as *const u8).add(y * map.RowPitch as usize), w * 4);
            bgra[y * w * 4..(y + 1) * w * 4].copy_from_slice(row);
        }
        g.ctx.Unmap(&staging, 0);
        let r = mon.rect();
        // the session is closed here, when `mon` is dropped
        Ok(Report { width: d.Width, height: d.Height, bgra, monitor: (r.left, r.top, r.right, r.bottom), open, list_call, first_image, frames, reported })
    }
}
