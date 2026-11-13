//! A windowless, READ-ONLY look at what the taskbar hiding WOULD act on: the taskbar-class windows (class, visibility, owner is
//! Explorer, rectangle), the monitors with their work areas, what the work-area rule would do, and whether the state file holds a
//! record. Counts and rectangles only: no titles, no names of programs.
//!
//! It calls `list_taskbar_windows` (EnumWindows, GetClassNameW, GetWindowRect, IsWindowVisible, OpenProcess for a query) and nothing
//! that changes or messages a window; `tests/tb_static.rs` keeps examples from naming the functions that do.
//!
//! cargo run --offline -p rf-shellguard --example taskbar_probe

use rf_shellguard::statefile::{self, ShellStateFile};
use rf_shellguard::tbmodel::{self, MonitorGeom};
use rf_shellguard::tbwork;
use rf_shellguard::win::tbwin::list_taskbar_windows;
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};

/// `MONITORINFOF_PRIMARY`.
const PRIMARY_FLAG: u32 = 1;
use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
use windows::core::BOOL;

unsafe extern "system" fn mon_cb(hmon: HMONITOR, _dc: HDC, _r: *mut RECT, lparam: LPARAM) -> BOOL {
    // SAFETY: lparam is the address of the Vec on the caller's stack, alive during EnumDisplayMonitors.
    unsafe {
        let list = &mut *(lparam.0 as *mut Vec<MonitorGeom>);
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if GetMonitorInfoW(hmon, &mut mi.monitorInfo as *mut _ as *mut _).as_bool() {
            let name = String::from_utf16_lossy(&mi.szDevice).trim_end_matches('\0').to_string();
            let (r, w) = (mi.monitorInfo.rcMonitor, mi.monitorInfo.rcWork);
            list.push(MonitorGeom { id: name, rect: [r.left, r.top, r.right, r.bottom], work: [w.left, w.top, w.right, w.bottom], primary: mi.monitorInfo.dwFlags & PRIMARY_FLAG != 0 });
        }
    }
    BOOL(1)
}

fn main() {
    // SAFETY: plain calls; the awareness is for this process only, so that rectangles come back in physical pixels.
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut mons: Vec<MonitorGeom> = Vec::new();
    // SAFETY: `mons` outlives the call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(mon_cb), LPARAM(&mut mons as *mut Vec<MonitorGeom> as isize));
    }
    println!("taskbar_probe (read-only)");
    println!("monitors: {}", mons.len());
    for (i, m) in mons.iter().enumerate() {
        println!("  monitor {i}: rect {:?} work {:?}", m.rect, m.work);
    }
    let live = match list_taskbar_windows() {
        Ok(l) => l,
        Err(e) => {
            println!("could not list the windows: {e}");
            return;
        }
    };
    println!("taskbar-class windows: {}", live.len());
    let on = |r: [i32; 4]| mons.iter().position(|m| r[0] < m.rect[2] && r[2] > m.rect[0] && r[1] < m.rect[3] && r[3] > m.rect[1]);
    for w in &live {
        println!("  class {} visible {} owner is Explorer {} rect {:?} monitor {:?}", w.class, w.visible, w.explorer, w.rect, on(w.rect));
    }
    let would = tbmodel::windows_to_hide(&live);
    println!("would hide: {} window(s)", would.len());
    // The rule is shown for every taskbar window of Explorer's, visible or already hidden (by an earlier run of the dock, say), as if our
    // hide were in force: the rule only looks at where they are.
    let bars: Vec<[i32; 4]> = live.iter().filter(|w| w.explorer && tbmodel::is_taskbar_class(&w.class)).map(|w| w.rect).collect();
    // What the rule would do once our hide is in force: with the dock's band (78 px is the user's size at 100 percent) and without.
    for (label, band) in [("dock_reserve on, a 78 px band", Some(78)), ("dock_reserve off", None)] {
        let plan = tbwork::plan_edge(&mons, &bars, band);
        println!("work-area rule, {label}:");
        for (i, m) in plan.monitors.iter().enumerate() {
            println!("  monitor {i}: {:?}; work area now {:?}, wanted {:?}, the user's own without the dock {:?}", m.verdict, m.work_now, m.target, m.user_work);
        }
        if !plan.any_ours() {
            println!("  no change (no taskbar to hide, or another toolbar reserves room)");
        }
    }
    let dir = rf_shellguard::paths::state_dir(None, &|k| std::env::var_os(k));
    let st = ShellStateFile::peek(&statefile::path_in(&dir));
    println!(
        "state file: {:?}; record of hidden windows: {}; pending_restore: {}",
        st.status,
        st.hidden_taskbar().map_or_else(|| if st.has_hidden_taskbar_key() { "unreadable".to_string() } else { "none".to_string() }, |r| format!("{} window(s)", r.windows.len())),
        st.pending_restore()
    );
}
