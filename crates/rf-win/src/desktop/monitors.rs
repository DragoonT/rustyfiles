//! The connected monitors, for the wallpaper and the icon layout. Read-only.
//!
//! NEEDS A PER-MONITOR-DPI-AWARE PROCESS to see physical pixels: a process that is not aware is shown virtualised rectangles and a
//! 96 dpi everywhere. The app and the dock are aware; a windowless probe calls [`make_process_per_monitor_aware`] first.
//!
//! THE DEVICE KEY. A layout is remembered per monitor, so the monitor needs a name that does not change between runs, between modes
//! (beside Explorer or instead of it) or when another monitor is plugged in. `IDesktopWallpaper::GetMonitorDevicePathAt` gives such a
//! path but whether that object answers with no Explorer is unverified (shell plan test T3), and a key that changes with the mode would
//! throw every layout away. So the key is asked of the display driver instead: `EnumDisplayDevices` with
//! `EDD_GET_DEVICE_INTERFACE_NAME` returns the monitor's device-interface path (`\\?\DISPLAY#...`), which is the same kind of string.
//! (Whether it equals the COM one is printed by `desktop_probe`.) When a monitor has no such path the adapter's name (`\\.\DISPLAY2`)
//! is used, which can change when monitors are re-plugged.

use rf_core::desktop::geom::{Rect, Size};
use rf_core::desktop::layout::MonitorSpec;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    DISPLAY_DEVICEW, EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForMonitor, MDT_EFFECTIVE_DPI, SetProcessDpiAwarenessContext};
use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
use windows::core::{BOOL, PCWSTR};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorInfo {
    /// Stable key for layouts, see the top of this file.
    pub device_key: String,
    /// `\\.\DISPLAY1`
    pub adapter_name: String,
    /// Virtual-screen coordinates, physical pixels (negative left of / above the primary monitor).
    pub rect: Rect,
    /// The part not taken by the taskbar / appbars, virtual-screen coordinates.
    pub work: Rect,
    pub primary: bool,
    pub dpi: u32,
    pub scale_percent: u32,
}

impl MonitorInfo {
    /// What the layout rules need to know.
    pub fn spec(&self) -> MonitorSpec {
        MonitorSpec {
            device: self.device_key.clone(),
            size: Size::new(self.rect.width().max(0) as u32, self.rect.height().max(0) as u32),
            scale_percent: self.scale_percent,
            work: self.work.offset(-self.rect.left, -self.rect.top).intersect(&Rect::from_size(0, 0, self.rect.width().max(0) as u32, self.rect.height().max(0) as u32)),
            primary: self.primary,
            rect: self.rect,
        }
    }
}

/// Makes the process per-monitor-v2 aware (for windowless probes; a process that already has an awareness keeps it and this returns
/// false). Changes nothing outside this process.
pub fn make_process_per_monitor_aware() -> bool {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2).is_ok() }
}

fn rect(r: RECT) -> Rect {
    Rect::new(r.left, r.top, r.right, r.bottom)
}

fn interface_path(adapter: &[u16; 32]) -> Option<String> {
    let mut dd = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
    // monitor 0 of the adapter (a cloned display has more than one; the first is the one that stands for the rectangle)
    if !unsafe { EnumDisplayDevicesW(PCWSTR(adapter.as_ptr()), 0, &mut dd, EDD_GET_DEVICE_INTERFACE_NAME) }.as_bool() {
        return None;
    }
    let s = crate::wide::from_wide(&dd.DeviceID);
    (!s.is_empty()).then_some(s)
}

unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    let out = unsafe { &mut *(data.0 as *mut Vec<MonitorInfo>) };
    let mut mi = MONITORINFOEXW::default();
    mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(h, &mut mi as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dx, &mut dy) };
        let dpi = dx.max(96);
        let adapter_name = crate::wide::from_wide(&mi.szDevice);
        out.push(MonitorInfo {
            device_key: interface_path(&mi.szDevice).unwrap_or_else(|| adapter_name.clone()),
            adapter_name,
            rect: rect(mi.monitorInfo.rcMonitor),
            work: rect(mi.monitorInfo.rcWork),
            primary: mi.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
            dpi,
            scale_percent: (dpi * 100 + 48) / 96,
        });
    }
    BOOL(1)
}

/// The monitors, primary first, then left to right.
pub fn list() -> Vec<MonitorInfo> {
    let mut v: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(collect), LPARAM(&mut v as *mut Vec<MonitorInfo> as isize));
    }
    v.sort_by_key(|m| (!m.primary, m.rect.left, m.rect.top));
    v
}

/// `(device key, rect)` pairs, which is what the wallpaper's registry fallback needs.
pub fn device_rects(ms: &[MonitorInfo]) -> Vec<(String, Rect)> {
    ms.iter().map(|m| (m.device_key.clone(), m.rect)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_is_at_least_one_monitor_and_exactly_one_primary_first() {
        let ms = list();
        assert!(!ms.is_empty());
        assert!(ms[0].primary);
        assert_eq!(ms.iter().filter(|m| m.primary).count(), 1);
        for m in &ms {
            assert!(!m.rect.is_empty() && !m.device_key.is_empty() && m.scale_percent >= 100);
            assert!(m.work.intersect(&m.rect) == m.work, "the work area is inside the monitor");
            let s = m.spec();
            assert_eq!(s.work, m.work.offset(-m.rect.left, -m.rect.top), "work in monitor-local coordinates");
        }
    }

    #[test]
    fn spec_translates_the_work_area_to_monitor_coordinates() {
        let m = MonitorInfo {
            device_key: "K".into(),
            adapter_name: "\\\\.\\DISPLAY2".into(),
            rect: Rect::new(-1440, -560, 0, 2000),
            work: Rect::new(-1440, -560, 0, 1900),
            primary: false,
            dpi: 120,
            scale_percent: 125,
        };
        let s = m.spec();
        assert_eq!((s.size, s.work, s.scale_percent, s.rect), (Size::new(1440, 2560), Rect::new(0, 0, 1440, 2460), 125, m.rect));
    }
}
