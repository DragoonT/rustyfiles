//! One app's own sound level: read it, and turn it up or down by the mouse wheel's notches.
//!
//! WHY (issue 21, a user of the dock: "will be good control volume level per app by scrolling over app icon"). Windows keeps a level
//! for every program that plays sound (the "volume mixer"); the dock's icon is the nearest handle on a program there is.
//!
//! WHICH SOUNDS ARE AN APP'S. Windows names a sound session by the PROCESS that plays it, and that is often not the process of the
//! app's window: a browser plays from a helper process, an Electron app from a child. So a session belongs to the nearest process, going
//! up from the one that plays through its parents, that has a window on the dock ([`owner_of`]). "Nearest" matters: a player started
//! from a terminal has its own icon, and its sound is its own, not the terminal's. Three things end the walk with no owner: the shell's
//! process (nearly every program descends from it), this process (apps started from the dock are its children), and a "parent" that was
//! created AFTER its child: process numbers are reused, and such a parent is a newer program that only has the dead parent's number.
//!
//! WHAT IS CHANGED: the session's own level (`ISimpleAudioVolume`), the very value the volume mixer shows; the system's master level
//! is not touched. Every session of the app gets the same new level. Turning up takes the mute off, as the volume keys do.
//!
//! Everything here blocks for a few milliseconds (the audio service is asked across processes): call it off the UI thread. Two calls at
//! once would each read the old level and write the same new one; a lock makes each notch count.

use std::collections::HashMap;
use std::sync::Mutex;

use windows::Win32::Foundation::{CloseHandle, FILETIME, S_OK};
use windows::Win32::Media::Audio::{
    AudioSessionStateExpired, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{SPI_GETWHEELSCROLLLINES, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, SystemParametersInfoW};
use windows::core::Interface;

use crate::dock::winops;

/// One notch of the wheel, in hundredths of the full level.
pub const STEP: i32 = 5;
/// How far up the parents a sound's owner is looked for.
const DEPTH: usize = 24;

/// An app's level: 0.0 to 1.0, and whether it is muted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
}

impl Level {
    /// The level as the number the volume mixer shows (0 to 100).
    pub fn percent(&self) -> i32 {
        (self.volume.clamp(0.0, 1.0) * 100.0).round() as i32
    }
}

/// The level after `notches` of the wheel (up is positive): to the next multiple of [`STEP`] in that direction, then a step for each
/// further notch, never below 0 or above 100. Turning up takes the mute off; turning down leaves it.
pub fn stepped(now: Level, notches: i32) -> Level {
    if notches == 0 {
        return now;
    }
    let cur = now.percent();
    let grid = if notches > 0 { cur.div_euclid(STEP) } else { (cur + STEP - 1).div_euclid(STEP) };
    let next = ((grid + notches) * STEP).clamp(0, 100);
    Level { volume: next as f32 / 100.0, muted: now.muted && notches < 0 }
}

/// One level for an app with several sessions: the loudest of them, and muted only when all are.
pub fn combined(levels: &[Level]) -> Option<Level> {
    let first = *levels.first()?;
    Some(levels.iter().skip(1).fold(first, |a, l| Level { volume: a.volume.max(l.volume), muted: a.muted && l.muted }))
}

/// The whole notches in what the wheel has turned so far: `turned` (in notches, a part of one for a fine wheel or a touchpad) is added
/// to `rest`, the whole notches are taken out and returned, the part stays for the next event.
pub fn take_notches(rest: &mut f32, turned: f32) -> i32 {
    if !turned.is_finite() {
        return 0;
    }
    *rest += turned;
    let whole = rest.trunc();
    *rest -= whole;
    whole.clamp(-20.0, 20.0) as i32
}

/// The notches in a wheel event the toolkit reports in lines (it multiplies each notch by the system's "lines per notch").
pub fn notches_of_lines(lines: f32, lines_per_notch: u32) -> f32 {
    // (0 lines and "a page at a time", which Windows writes as the largest number, are no divisor: three, the usual value)
    let per = if (1..=100).contains(&lines_per_notch) { lines_per_notch } else { 3 };
    lines / per as f32
}

/// The process a sound belongs to for the dock: the nearest of `pid` and its parents that has a window on the dock (`has_window`).
/// `None` when the walk reaches one of `stop` (the shell, this process), the system's processes, a parent that is gone, or a parent
/// that was created after its child (`created`: a time that only has to order two processes; 0 when it is not known, which is taken
/// on trust).
pub fn owner_of(pid: u32, parent_of: &dyn Fn(u32) -> Option<u32>, created: &dyn Fn(u32) -> u64, has_window: &dyn Fn(u32) -> bool, stop: &[u32]) -> Option<u32> {
    let mut at = pid;
    for _ in 0..DEPTH {
        if at <= 4 || stop.contains(&at) {
            return None;
        }
        if has_window(at) {
            return Some(at);
        }
        let parent = parent_of(at)?;
        if parent == at {
            return None;
        }
        let (mine, theirs) = (created(at), created(parent));
        if mine != 0 && theirs != 0 && theirs > mine {
            return None;
        }
        at = parent;
    }
    None
}

// ---------------------------------------------------------------- the system

/// Every process and its parent, as the system lists them now.
fn parents() -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    // SAFETY: the snapshot handle is used and closed here; `dwSize` is set as the API asks.
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else { return out };
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap, &mut e).is_ok();
        while more {
            out.insert(e.th32ProcessID, e.th32ParentProcessID);
            more = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    out
}

/// When a process was created (the system's 100 ns count); 0 when it cannot be asked (a program with more rights, or gone).
fn created_at(pid: u32) -> u64 {
    // SAFETY: the handle is closed here; the four times are plain out values.
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return 0 };
        let (mut made, mut ended, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let ok = GetProcessTimes(handle, &mut made, &mut ended, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(handle);
        if ok { (u64::from(made.dwHighDateTime) << 32) | u64::from(made.dwLowDateTime) } else { 0 }
    }
}

/// The system's "lines per notch" of the wheel (what the toolkit multiplies a notch by).
pub fn wheel_lines() -> u32 {
    let mut lines = 3u32;
    // SAFETY: the out value is a u32, as this parameter asks.
    let _ = unsafe { SystemParametersInfoW(SPI_GETWHEELSCROLLLINES, 0, Some(&mut lines as *mut u32 as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0)) };
    lines
}

/// The processes behind windows (a Store app's frame counts as the app inside it), each once.
fn pids_of(windows: &[isize]) -> Vec<u32> {
    let mut out: Vec<u32> = Vec::new();
    for w in winops::refs(windows) {
        if let Some(pid) = winops::task_owner(w).and_then(|o| o.app_pid())
            && !out.contains(&pid)
        {
            out.push(pid);
        }
    }
    out
}

/// A sound session that is in use: the process that plays it and the handle on its level.
struct Session {
    pid: u32,
    volume: ISimpleAudioVolume,
}

/// Every program's session on every output device that is in use (an app can be sent to a device that is not the default one). The
/// system's own sounds are not a program's.
fn sessions() -> Vec<Session> {
    let mut out = Vec::new();
    // SAFETY: COM calls on interfaces this function owns; each result is checked.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return out };
        let Ok(devices) = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) else { return out };
        for d in 0..devices.GetCount().unwrap_or(0) {
            let Ok(dev) = devices.Item(d) else { continue };
            let Ok(manager) = dev.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else { continue };
            let Ok(list) = manager.GetSessionEnumerator() else { continue };
            for i in 0..list.GetCount().unwrap_or(0) {
                let Ok(control) = list.GetSession(i) else { continue };
                if control.GetState().is_ok_and(|s| s == AudioSessionStateExpired) {
                    continue;
                }
                let Ok(two) = control.cast::<IAudioSessionControl2>() else { continue };
                if two.IsSystemSoundsSession() == S_OK {
                    continue;
                }
                let Ok(pid) = two.GetProcessId() else { continue };
                let Ok(volume) = control.cast::<ISimpleAudioVolume>() else { continue };
                out.push(Session { pid, volume });
            }
        }
    }
    out
}

/// The sessions that are the app's, when its windows are `app_windows` and the windows of everything on the dock are `all_windows`.
fn sessions_of(app_windows: &[isize], all_windows: &[isize]) -> Vec<Session> {
    let app = pids_of(app_windows);
    if app.is_empty() {
        return Vec::new();
    }
    let mut windowed = pids_of(all_windows);
    for p in &app {
        if !windowed.contains(p) {
            windowed.push(*p);
        }
    }
    let all = sessions();
    if all.is_empty() {
        return all;
    }
    let tree = parents();
    let mut stop = vec![unsafe { GetCurrentProcessId() }];
    stop.extend(winops::shell_pid());
    // (an app whose window IS this process's or the shell's has no sessions by this rule: the walk ends at once)
    let owner = |pid: u32| owner_of(pid, &|p| tree.get(&p).copied(), &created_at, &|p| windowed.contains(&p), &stop);
    all.into_iter().filter(|s| owner(s.pid).is_some_and(|o| app.contains(&o))).collect()
}

/// Every program's sound session as the system lists it now, for a probe: the process that plays, the process the dock would give
/// the sound to (when the windows of everything on the dock are `all_windows`), and its level. Reads only.
pub fn probe(all_windows: &[isize]) -> Vec<(u32, Option<u32>, Option<Level>)> {
    let windowed = pids_of(all_windows);
    let tree = parents();
    let mut stop = vec![unsafe { GetCurrentProcessId() }];
    stop.extend(winops::shell_pid());
    sessions().iter().map(|s| (s.pid, owner_of(s.pid, &|p| tree.get(&p).copied(), &created_at, &|p| windowed.contains(&p), &stop), level_of(s))).collect()
}

fn level_of(s: &Session) -> Option<Level> {
    // SAFETY: plain getters of an interface this module holds.
    unsafe { Some(Level { volume: s.volume.GetMasterVolume().ok()?, muted: s.volume.GetMute().map(|b| b.as_bool()).unwrap_or(false) }) }
}

/// One change at a time (see the module's note).
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

/// How many sessions the app has now and their level; `(0, None)` when it plays through none. Reads only.
pub fn read(app_windows: &[isize], all_windows: &[isize]) -> (usize, Option<Level>) {
    let mine = sessions_of(app_windows, all_windows);
    let levels: Vec<Level> = mine.iter().filter_map(level_of).collect();
    (mine.len(), combined(&levels))
}

/// Turns the app's level by `notches` of the wheel and returns the level it has now; `None` when the app has no sound session (it
/// has played nothing yet): then nothing is changed.
pub fn nudge(app_windows: &[isize], all_windows: &[isize], notches: i32) -> Option<Level> {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let mine = sessions_of(app_windows, all_windows);
    let levels: Vec<Level> = mine.iter().filter_map(level_of).collect();
    let now = combined(&levels)?;
    let next = stepped(now, notches);
    for s in &mine {
        // SAFETY: plain setters; a session that ended meanwhile refuses, which changes nothing.
        unsafe {
            let _ = s.volume.SetMasterVolume(next.volume, std::ptr::null());
            // (turning up takes the mute off every session; turning down touches no session's mute)
            if notches > 0 {
                let _ = s.volume.SetMute(false, std::ptr::null());
            }
        }
    }
    Some(next)
}

// ------------------------------------------------------------------ the mixer: every program that plays, by name

/// One program in the mixer.
#[derive(Clone, Debug, PartialEq)]
pub struct MixerApp {
    /// The program's name (its file's, without ".exe"): what its row is found by again.
    pub name: String,
    pub level: Level,
}

/// A program's name in the mixer, from the file of the process that plays ("C:\...\chrome.exe": "chrome"); a process whose file
/// cannot be asked (a protected one) is named by its number.
pub fn mixer_name(exe: Option<&std::path::Path>, pid: u32) -> String {
    exe.and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().into_owned()).filter(|s| !s.trim().is_empty()).unwrap_or_else(|| format!("#{pid}"))
}

/// The mixer's rows from the sessions' names and levels: a program once, however many processes and sessions it plays through (a
/// browser plays from helper processes that carry its file's name), with the level `combined` gives them; by name.
pub fn mixer_rows(sessions: Vec<(String, Level)>) -> Vec<MixerApp> {
    let mut by_name: Vec<(String, Vec<Level>)> = Vec::new();
    for (name, level) in sessions {
        match by_name.iter_mut().find(|(n, _)| *n == name) {
            Some((_, levels)) => levels.push(level),
            None => by_name.push((name, vec![level])),
        }
    }
    let mut out: Vec<MixerApp> = by_name.into_iter().filter_map(|(name, levels)| combined(&levels).map(|level| MixerApp { name, level })).collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

fn named_sessions() -> Vec<(String, Session)> {
    sessions().into_iter().map(|s| (mixer_name(crate::tasklist::process_exe_path(s.pid).as_deref(), s.pid), s)).collect()
}

/// The programs that have a sound session now (the system's own sounds are not a program's). Reads only; blocks a few ms.
pub fn mixer() -> Vec<MixerApp> {
    mixer_rows(named_sessions().iter().filter_map(|(name, s)| level_of(s).map(|l| (name.clone(), l))).collect())
}

fn apply_mixer_level(name: &str, volume: f32) {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let volume = volume.clamp(0.0, 1.0);
    for (_, s) in named_sessions().iter().filter(|(n, _)| n == name) {
        // SAFETY: plain setters; a session that ended meanwhile refuses, which changes nothing.
        unsafe {
            let _ = s.volume.SetMasterVolume(volume, std::ptr::null());
            // (a level above nothing takes the mute off, as turning the wheel up does)
            if volume > 0.0 {
                let _ = s.volume.SetMute(false, std::ptr::null());
            }
        }
    }
}

/// The user dragged a program's slider in the mixer: every session of that program gets the level. Returns at once: a thread of its
/// own applies the NEWEST level asked for each program (a drag asks for one at every pixel).
pub fn set_mixer_level(name: &str, volume: f32) {
    static QUEUE: std::sync::OnceLock<std::sync::mpsc::Sender<(String, f32)>> = std::sync::OnceLock::new();
    let tx = QUEUE.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<(String, f32)>();
        let spawned = std::thread::Builder::new().name("rf-mixer".into()).spawn(move || {
            while let Ok(first) = rx.recv() {
                let mut latest: HashMap<String, f32> = HashMap::from([first]);
                while let Ok((name, v)) = rx.try_recv() {
                    latest.insert(name, v);
                }
                for (name, v) in latest {
                    apply_mixer_level(&name, v);
                }
            }
        });
        if let Err(e) = spawned {
            log::warn!("mixer: the thread did not start: {e}");
        }
        tx
    });
    let _ = tx.send((name.to_string(), volume));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mixer_names_a_program_by_its_file_and_lists_it_once() {
        use std::path::Path;
        assert_eq!(mixer_name(Some(Path::new(r"C:\Program Files\Google\Chrome\Application\chrome.exe")), 7), "chrome");
        assert_eq!(mixer_name(Some(Path::new(r"D:\Oyun\Forza Horizon.exe")), 7), "Forza Horizon");
        assert_eq!(mixer_name(None, 4812), "#4812");
        let l = |percent: i32, muted: bool| Level { volume: percent as f32 / 100.0, muted };
        let rows = mixer_rows(vec![("Spotify".into(), l(80, false)), ("chrome".into(), l(40, false)), ("chrome".into(), l(40, false)), ("discord".into(), l(100, true))]);
        let shown: Vec<(&str, i32, bool)> = rows.iter().map(|a| (a.name.as_str(), a.level.percent(), a.level.muted)).collect();
        assert_eq!(shown, [("chrome", 40, false), ("discord", 100, true), ("Spotify", 80, false)]);
        assert!(mixer_rows(Vec::new()).is_empty());
    }

    /// On the PC it is run on, by hand: how many programs play now (not which). Reads only.
    #[test]
    #[ignore = "reads the sound sessions of the PC it runs on; run by hand"]
    fn what_this_pc_plays() {
        println!("programs in the mixer: {}", mixer().len());
    }

    fn at(percent: i32) -> Level {
        Level { volume: percent as f32 / 100.0, muted: false }
    }

    #[test]
    fn a_notch_goes_to_the_next_multiple_of_the_step() {
        assert_eq!(stepped(at(50), 1).percent(), 55);
        assert_eq!(stepped(at(50), -1).percent(), 45);
        // from between two steps: to the next one in that direction, not a whole step further
        assert_eq!(stepped(at(63), 1).percent(), 65);
        assert_eq!(stepped(at(63), -1).percent(), 60);
        assert_eq!(stepped(at(63), 3).percent(), 75);
        assert_eq!(stepped(at(63), -2).percent(), 55);
        // the ends
        assert_eq!(stepped(at(98), 1).percent(), 100);
        assert_eq!(stepped(at(100), 4).percent(), 100);
        assert_eq!(stepped(at(3), -1).percent(), 0);
        assert_eq!(stepped(at(0), -5).percent(), 0);
        assert_eq!(stepped(at(0), 1).percent(), 5);
        // nothing turned: as it was, to the last digit
        assert_eq!(stepped(Level { volume: 0.634, muted: true }, 0), Level { volume: 0.634, muted: true });
        // a level out of range is read as the nearest end
        assert_eq!(stepped(Level { volume: 1.7, muted: false }, -1).percent(), 95);
    }

    #[test]
    fn turning_up_takes_the_mute_off_and_turning_down_leaves_it() {
        let muted = Level { volume: 0.4, muted: true };
        assert_eq!(stepped(muted, 1), Level { volume: 0.45, muted: false });
        assert_eq!(stepped(muted, -1), Level { volume: 0.35, muted: true });
        assert!(!stepped(at(40), -1).muted && !stepped(at(40), 1).muted);
    }

    #[test]
    fn several_sessions_read_as_the_loudest_and_are_muted_only_together() {
        assert_eq!(combined(&[]), None);
        assert_eq!(combined(&[at(30)]), Some(at(30)));
        assert_eq!(combined(&[at(30), at(80), at(10)]), Some(at(80)));
        let m = |p: i32| Level { volume: p as f32 / 100.0, muted: true };
        assert_eq!(combined(&[m(30), at(20)]), Some(at(30)), "one of them still sounds");
        assert_eq!(combined(&[m(30), m(60)]), Some(m(60)));
    }

    #[test]
    fn the_wheel_gives_whole_notches_and_keeps_the_rest() {
        let mut rest = 0.0;
        assert_eq!(take_notches(&mut rest, 1.0), 1);
        assert_eq!(take_notches(&mut rest, -2.0), -2);
        // a fine wheel: four quarter turns are one notch
        assert_eq!((take_notches(&mut rest, 0.25), take_notches(&mut rest, 0.25), take_notches(&mut rest, 0.25)), (0, 0, 0));
        assert_eq!(take_notches(&mut rest, 0.25), 1);
        assert!(rest.abs() < 1e-6);
        // back and forth cancels
        assert_eq!((take_notches(&mut rest, 0.6), take_notches(&mut rest, -0.6)), (0, 0));
        assert_eq!(take_notches(&mut rest, f32::NAN), 0);
        assert_eq!(take_notches(&mut rest, 500.0), 20, "one event never turns the level through more than its range");
        // the toolkit's lines: three to a notch by default, whatever the system says
        assert_eq!(notches_of_lines(3.0, 3), 1.0);
        assert_eq!(notches_of_lines(-6.0, 3), -2.0);
        assert_eq!(notches_of_lines(5.0, 5), 1.0);
        assert_eq!(notches_of_lines(3.0, 0), 1.0);
        assert_eq!(notches_of_lines(3.0, u32::MAX), 1.0, "a page at a time");
    }

    /// A small world: pid -> (parent, created).
    fn owner(pid: u32, world: &[(u32, u32, u64)], windows: &[u32], stop: &[u32]) -> Option<u32> {
        let parent = |p: u32| world.iter().find(|w| w.0 == p).map(|w| w.1);
        let created = |p: u32| world.iter().find(|w| w.0 == p).map_or(0, |w| w.2);
        owner_of(pid, &parent, &created, &|p| windows.contains(&p), stop)
    }

    #[test]
    fn a_sound_belongs_to_the_nearest_process_with_a_window() {
        // shell 100 -> browser 200 (window) -> audio helper 210;  shell -> terminal 300 (window) -> shell 310 -> player 320 (window) -> helper 330
        let world = [(100, 4, 10), (200, 100, 20), (210, 200, 30), (300, 100, 40), (310, 300, 50), (320, 310, 60), (330, 320, 70), (400, 100, 80)];
        let (windows, stop) = ([200, 300, 320], [100, 999]);
        assert_eq!(owner(200, &world, &windows, &stop), Some(200), "the app's own process");
        assert_eq!(owner(210, &world, &windows, &stop), Some(200), "its helper");
        assert_eq!(owner(310, &world, &windows, &stop), Some(300));
        assert_eq!(owner(320, &world, &windows, &stop), Some(320), "a player started from a terminal is its own, not the terminal's");
        assert_eq!(owner(330, &world, &windows, &stop), Some(320));
        // a program without a window, started by the shell: nobody's (the shell is not an owner, though everything descends from it)
        assert_eq!(owner(400, &world, &windows, &stop), None);
        assert_eq!(owner(100, &world, &[100], &stop), None, "the shell itself is never an owner");
        // the system's processes, a process that is not listed, a parent that is gone
        assert_eq!(owner(0, &world, &windows, &stop), None);
        assert_eq!(owner(4, &world, &windows, &stop), None);
        assert_eq!(owner(5000, &world, &windows, &stop), None);
        assert_eq!(owner(600, &[(600, 601, 5)], &[601], &stop), Some(601), "a parent the list no longer has is still asked for its window");
        assert_eq!(owner(600, &[(600, 601, 5)], &[], &stop), None);
    }

    #[test]
    fn a_parent_number_that_a_newer_process_took_is_not_a_parent() {
        // helper 210 was started by a process 200 that ended; a new program got the number 200 later and has a window
        let world = [(200, 100, 90), (210, 200, 30)];
        assert_eq!(owner(210, &world, &[200], &[100]), None);
        // the true parent is older
        let world = [(200, 100, 20), (210, 200, 30)];
        assert_eq!(owner(210, &world, &[200], &[100]), Some(200));
        // a time that is not known is taken on trust (a program with more rights cannot be asked)
        let world = [(200, 100, 0), (210, 200, 30)];
        assert_eq!(owner(210, &world, &[200], &[100]), Some(200));
        // a list that goes round in a circle ends
        let world = [(700, 701, 1), (701, 700, 1)];
        assert_eq!(owner(700, &world, &[], &[]), None);
        assert_eq!(owner(800, &[(800, 800, 1)], &[], &[]), None);
    }

    /// On the PC the tests run on: reading never fails, an app without windows has no sound, and nothing is changed for it.
    #[test]
    fn an_app_without_windows_has_no_sound_and_nothing_is_changed() {
        assert_eq!(read(&[], &[]), (0, None));
        assert_eq!(read(&[0x7FFF_FFF1], &[0x7FFF_FFF1]), (0, None));
        assert_eq!(nudge(&[], &[], 3), None);
        assert_eq!(nudge(&[0x7FFF_FFF1], &[], -3), None);
        assert!((1..=100).contains(&wheel_lines()) || wheel_lines() == u32::MAX || wheel_lines() == 0);
        // (the tree and the times are read without failing; this process has a parent and a time)
        let me = std::process::id();
        assert!(parents().contains_key(&me));
        assert_ne!(created_at(me), 0);
    }
}
