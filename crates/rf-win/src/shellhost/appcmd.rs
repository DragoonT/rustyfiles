//! Media and volume keys when no Explorer is there to answer them.
//!
//! WHAT HAPPENS TO A MEDIA KEY (Microsoft's documentation, read 2026-11-12): the key makes `WM_APPCOMMAND`. "If a top level window does
//! not process this message and instead calls DefWindowProc, DefWindowProc will call a shell hook with the hook code equal to
//! HSHELL_APPCOMMAND" (WM_APPCOMMAND, Remarks). A window registered with `RegisterShellHookWindow` receives that as the registered
//! message "SHELLHOOK" with `wParam = HSHELL_APPCOMMAND` and `lParam` = the application command, "unhandled by the application or other
//! hooks" (`GET_APPCOMMAND_LPARAM`). So a program that handles the key itself (Spotify with its window in front) answers it; every other
//! case goes to the shell hook, and the shell is Explorer: with Explorer away nobody takes it. That is the trial's "fn+F7 does nothing
//! unless Spotify's window is open".
//!
//! WHAT THIS FILE DOES. [`decide`] is the whole decision: which command, shell present or not, what is to be done. With Explorer owning
//! the shell window it answers [`Do::Nothing`] for EVERY command - beside a running Explorer the hook is never even registered (see
//! `shellhook.rs`), and this is the second lock: each key would act twice otherwise (Explorer's volume key and ours). [`Executor`] carries
//! a decision out through [`Outputs`] (the real ones are [`SystemOutputs`]: the same WinRT media session the control centre's card
//! talks to, and the same `audio::set_volume` / `set_muted` its slider uses). Volume keys step by 2 % like Windows' own
//! (the endpoint's own step when it reports one that is finer, see [`step_of`]).
//!
//! No on-screen feedback is added: the dock has none for volume today; the right island's speaker symbol follows the endpoint by itself
//! (`sysstate`), which is what shows the key worked.

use std::time::{Duration, Instant};

use super::mode::{self, Owner, ShellMode};

/// The application commands this handles (`APPCOMMAND_*` values from winuser.h, as in the WM_APPCOMMAND documentation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppCommand {
    VolumeMute,
    VolumeDown,
    VolumeUp,
    MediaNext,
    MediaPrevious,
    MediaStop,
    MediaPlayPause,
    /// Any other command (browser keys, mail, launch keys ...): never touched.
    Other(u16),
}

impl AppCommand {
    pub fn from_code(code: u16) -> AppCommand {
        match code {
            8 => AppCommand::VolumeMute,
            9 => AppCommand::VolumeDown,
            10 => AppCommand::VolumeUp,
            11 => AppCommand::MediaNext,
            12 => AppCommand::MediaPrevious,
            13 => AppCommand::MediaStop,
            14 => AppCommand::MediaPlayPause,
            n => AppCommand::Other(n),
        }
    }
}

/// `GET_APPCOMMAND_LPARAM(lParam)`: the command is the high word with the device bits (0xF000) taken off.
pub fn command_of_lparam(lparam: isize) -> u16 {
    (((lparam as usize >> 16) & 0xFFFF) as u16) & !0xF000
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaAction {
    Next,
    Previous,
    PlayPause,
    Stop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VolumeAction {
    Up,
    Down,
    ToggleMute,
}

/// What is to be done for one command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    Media(MediaAction),
    Volume(VolumeAction),
}

/// Who answers the keys right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// Explorer owns the shell window: it answers.
    Explorer,
    /// Another program owns it (an alternative shell): it is its business, and a second answer would double every key.
    Foreign,
    /// Nobody owns the shell window (or it is this very process): nobody answers.
    Nobody,
}

/// [`Shell`] from the owner of the shell window (`None`: there is none). `windows_dir` as in [`mode::decide`].
pub fn shell_of(owner: Option<&Owner>, windows_dir: &str) -> Shell {
    match owner {
        None => Shell::Nobody,
        Some(o) if o.own_process => Shell::Nobody,
        Some(o) => match mode::decide(Some(o), windows_dir) {
            ShellMode::Beside => Shell::Explorer,
            ShellMode::Shell => Shell::Foreign,
        },
    }
}

/// [`Shell`] by looking now (read-only).
pub fn shell_now() -> Shell {
    shell_of(mode::shell_window_owner().as_ref(), &crate::startup::system::windows_dir())
}

/// The decision. `shell`: who owns the shell window NOW (a fresh look, not the one from the host's start).
pub fn decide(command: AppCommand, shell: Shell) -> Do {
    if shell != Shell::Nobody {
        // Explorer (or another shell) is there and answers these itself
        return Do::Nothing;
    }
    match command {
        AppCommand::MediaNext => Do::Media(MediaAction::Next),
        AppCommand::MediaPrevious => Do::Media(MediaAction::Previous),
        AppCommand::MediaPlayPause => Do::Media(MediaAction::PlayPause),
        AppCommand::MediaStop => Do::Media(MediaAction::Stop),
        AppCommand::VolumeUp => Do::Volume(VolumeAction::Up),
        AppCommand::VolumeDown => Do::Volume(VolumeAction::Down),
        AppCommand::VolumeMute => Do::Volume(VolumeAction::ToggleMute),
        AppCommand::Other(_) => Do::Nothing,
    }
}

/// What the output device is now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Level {
    pub volume: f32,
    pub muted: bool,
    /// The endpoint's number of positions (`GetVolumeStepInfo`; 0: not told).
    pub steps: u32,
}

/// Windows' step for a volume key: 2 %.
pub const WINDOWS_STEP: f32 = 0.02;

/// The size of one key press. An endpoint with up to 101 positions moves by its own position (51 positions are the 2 % steps
/// Windows shows); anything else (no answer, a finer endpoint) moves by 2 %.
pub fn step_of(steps: u32) -> f32 {
    if (2..=101).contains(&steps) { 1.0 / (steps - 1) as f32 } else { WINDOWS_STEP }
}

/// What a volume command asks the endpoint for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumePlan {
    pub volume: Option<f32>,
    pub muted: Option<bool>,
}

/// The new state for a volume command. `pending`: the volume a press a moment ago asked for and that the endpoint may not have
/// reported yet (a held key repeats faster than the answers come back; stepping from the old reading would lose steps).
pub fn plan_volume(action: VolumeAction, level: Level, pending: Option<f32>) -> VolumePlan {
    match action {
        VolumeAction::ToggleMute => VolumePlan { volume: None, muted: Some(!level.muted) },
        VolumeAction::Up | VolumeAction::Down => {
            let base = pending.unwrap_or(level.volume);
            let delta = if action == VolumeAction::Up { step_of(level.steps) } else { -step_of(level.steps) };
            let v = (base + delta).clamp(0.0, 1.0);
            // a press of a volume key on a muted device is heard (Windows' own keys unmute)
            VolumePlan { volume: Some(v), muted: level.muted.then_some(false) }
        }
    }
}

/// How long a volume asked for is remembered as "the level" before the endpoint's own reading is trusted again.
pub const PENDING_MS: u64 = 600;

/// The outputs of a decision.
pub trait Outputs {
    fn media(&mut self, action: MediaAction);
    fn level(&mut self) -> Option<Level>;
    fn set_volume(&mut self, fraction: f32);
    fn set_muted(&mut self, muted: bool);
}

/// Carries decisions out; remembers the last volume it asked for.
#[derive(Default)]
pub struct Executor {
    pending: Option<(Instant, f32)>,
}

impl Executor {
    pub fn new() -> Executor {
        Executor::default()
    }

    /// Does what `d` says. Returns whether anything was done.
    pub fn run(&mut self, d: Do, now: Instant, out: &mut dyn Outputs) -> bool {
        match d {
            Do::Nothing => false,
            Do::Media(a) => {
                out.media(a);
                true
            }
            Do::Volume(a) => {
                let Some(level) = out.level() else { return false };
                let pending = self.pending.filter(|(t, _)| now.saturating_duration_since(*t) < Duration::from_millis(PENDING_MS)).map(|(_, v)| v);
                let plan = plan_volume(a, level, pending);
                if let Some(v) = plan.volume {
                    out.set_volume(v);
                    self.pending = Some((now, v));
                }
                if let Some(m) = plan.muted {
                    out.set_muted(m);
                }
                true
            }
        }
    }
}

/// One key, start to end: decides for the shell as it is now and carries the decision out. The hook's worker thread calls this.
pub fn act(code: u16, shell: Shell, ex: &mut Executor, now: Instant, out: &mut dyn Outputs) -> Do {
    let d = decide(AppCommand::from_code(code), shell);
    ex.run(d, now, out);
    d
}

/// The real outputs. Blocks (WinRT, Core Audio): call from a worker thread, never from a window procedure.
pub struct SystemOutputs;

impl Outputs for SystemOutputs {
    fn media(&mut self, action: MediaAction) {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
        crate::sysstatus::init_mta();
        let session = GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok().and_then(|op| op.join().ok()).and_then(|m| m.GetCurrentSession().ok());
        let Some(s) = session else {
            log::debug!("media key: there is no media session to send {action:?} to");
            return;
        };
        // (the answer is a bool: a program may refuse)
        let _ = match action {
            MediaAction::Next => s.TrySkipNextAsync(),
            MediaAction::Previous => s.TrySkipPreviousAsync(),
            MediaAction::PlayPause => s.TryTogglePlayPauseAsync(),
            MediaAction::Stop => s.TryStopAsync(),
        };
    }

    fn level(&mut self) -> Option<Level> {
        use crate::sysstatus::audio;
        // the dock's right island follows the sound all the time, so there is a picture; if not (a host without that island), read once
        let s = audio::snapshot().filter(|s| s.available).unwrap_or_else(audio::read_once);
        s.available.then_some(Level { volume: s.volume, muted: s.muted, steps: s.steps })
    }

    fn set_volume(&mut self, fraction: f32) {
        crate::sysstatus::audio::set_volume(fraction);
    }

    fn set_muted(&mut self, muted: bool) {
        crate::sysstatus::audio::set_muted(muted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        media: Vec<MediaAction>,
        level: Option<Level>,
        volumes: Vec<f32>,
        mutes: Vec<bool>,
    }

    impl Outputs for Fake {
        fn media(&mut self, a: MediaAction) {
            self.media.push(a);
        }
        fn level(&mut self) -> Option<Level> {
            self.level
        }
        fn set_volume(&mut self, f: f32) {
            self.volumes.push(f);
            if let Some(l) = &mut self.level {
                l.volume = f;
            }
        }
        fn set_muted(&mut self, m: bool) {
            self.mutes.push(m);
        }
    }

    const ALL: [u16; 7] = [8, 9, 10, 11, 12, 13, 14];

    #[test]
    fn the_codes_are_the_documented_values() {
        use AppCommand::*;
        assert_eq!(ALL.map(AppCommand::from_code), [VolumeMute, VolumeDown, VolumeUp, MediaNext, MediaPrevious, MediaStop, MediaPlayPause]);
        assert_eq!(AppCommand::from_code(7), Other(7), "browser home");
        assert_eq!(AppCommand::from_code(46), Other(46), "direct play is left alone");
    }

    #[test]
    fn the_command_is_taken_out_of_the_high_word_without_the_device_bits() {
        // MAKELPARAM(0, MAKELONG(cmd | FAPPCOMMAND_OEM, keys))
        assert_eq!(command_of_lparam((11isize) << 16), 11);
        assert_eq!(command_of_lparam((((11u32 | 0x1000) as isize) << 16) | 0x0004), 11, "OEM key, shift down");
        assert_eq!(command_of_lparam((((14u32 | 0x8000) as isize) << 16) | 0x0001), 14, "mouse");
    }

    #[test]
    fn beside_a_running_explorer_nothing_is_done_with_any_command() {
        for code in 0..=60u16 {
            assert_eq!(decide(AppCommand::from_code(code), Shell::Explorer), Do::Nothing, "command {code}");
            assert_eq!(decide(AppCommand::from_code(code), Shell::Foreign), Do::Nothing, "command {code} (another shell answers)");
        }
    }

    #[test]
    fn beside_a_running_explorer_no_key_reaches_any_output() {
        let mut ex = Executor::new();
        let mut out = Fake { level: Some(Level { volume: 0.5, muted: false, steps: 51 }), ..Default::default() };
        for code in ALL {
            assert_eq!(act(code, Shell::Explorer, &mut ex, Instant::now(), &mut out), Do::Nothing);
        }
        assert!(out.media.is_empty() && out.volumes.is_empty() && out.mutes.is_empty(), "Explorer answers; we would double every key");
    }

    #[test]
    fn who_answers_is_worked_out_from_the_owner_of_the_shell_window() {
        let owner = |program: &str, path: &str, own: bool| Owner { class: "Progman".into(), program: program.into(), path: Some(path.into()), pid: 7, own_process: own };
        assert_eq!(shell_of(None, r"C:\Windows"), Shell::Nobody);
        assert_eq!(shell_of(Some(&owner("explorer.exe", r"C:\Windows\explorer.exe", false)), r"C:\Windows"), Shell::Explorer);
        assert_eq!(shell_of(Some(&owner("cairodesktop.exe", r"C:\Cairo\CairoDesktop.exe", false)), r"C:\Windows"), Shell::Foreign);
        assert_eq!(shell_of(Some(&owner("rustyfiles.exe", r"C:\x\rustyfiles.exe", true)), r"C:\Windows"), Shell::Nobody);
    }

    #[test]
    fn with_nobody_answering_a_key_reaches_the_outputs_through_act() {
        let mut ex = Executor::new();
        let mut out = Fake { level: Some(Level { volume: 0.5, muted: false, steps: 51 }), ..Default::default() };
        assert_eq!(act(11, Shell::Nobody, &mut ex, Instant::now(), &mut out), Do::Media(MediaAction::Next));
        assert_eq!(act(10, Shell::Nobody, &mut ex, Instant::now(), &mut out), Do::Volume(VolumeAction::Up));
        assert_eq!(act(7, Shell::Nobody, &mut ex, Instant::now(), &mut out), Do::Nothing);
        assert_eq!(out.media, [MediaAction::Next]);
        assert_eq!(out.volumes.len(), 1);
    }

    #[test]
    fn without_explorer_the_seven_commands_are_taken_and_all_others_left() {
        use {MediaAction as M, VolumeAction as V};
        let want = [Do::Volume(V::ToggleMute), Do::Volume(V::Down), Do::Volume(V::Up), Do::Media(M::Next), Do::Media(M::Previous), Do::Media(M::Stop), Do::Media(M::PlayPause)];
        for (code, d) in ALL.iter().zip(want) {
            assert_eq!(decide(AppCommand::from_code(*code), Shell::Nobody), d, "command {code}");
        }
        for code in (0..=60u16).filter(|c| !ALL.contains(c)) {
            assert_eq!(decide(AppCommand::from_code(code), Shell::Nobody), Do::Nothing, "command {code}");
        }
    }

    #[test]
    fn a_volume_key_steps_by_two_percent_or_the_endpoints_own_step() {
        assert_eq!(step_of(0), 0.02);
        assert!((step_of(51) - 0.02).abs() < 1e-6, "Windows' usual 51 positions");
        assert!((step_of(101) - 0.01).abs() < 1e-6);
        assert_eq!(step_of(256), 0.02, "a finer endpoint is not stepped one tick at a time");
        assert_eq!(step_of(1), 0.02);
    }

    #[test]
    fn volume_up_and_down_are_clamped_and_unmute() {
        let l = Level { volume: 0.5, muted: false, steps: 51 };
        assert!((plan_volume(VolumeAction::Up, l, None).volume.unwrap() - 0.52).abs() < 1e-6);
        assert!((plan_volume(VolumeAction::Down, l, None).volume.unwrap() - 0.48).abs() < 1e-6);
        assert_eq!(plan_volume(VolumeAction::Up, l, None).muted, None);
        let top = Level { volume: 0.99, ..l };
        assert_eq!(plan_volume(VolumeAction::Up, top, None).volume, Some(1.0));
        let bottom = Level { volume: 0.01, ..l };
        assert_eq!(plan_volume(VolumeAction::Down, bottom, None).volume, Some(0.0));
        let muted = Level { muted: true, ..l };
        assert_eq!(plan_volume(VolumeAction::Up, muted, None).muted, Some(false));
        assert_eq!(plan_volume(VolumeAction::ToggleMute, muted, None), VolumePlan { volume: None, muted: Some(false) });
        assert_eq!(plan_volume(VolumeAction::ToggleMute, l, None), VolumePlan { volume: None, muted: Some(true) });
    }

    #[test]
    fn a_held_key_steps_from_what_it_asked_for_not_from_a_reading_that_is_behind() {
        let mut ex = Executor::new();
        // the endpoint's reading never catches up in this test: it stays at 0.50
        struct Slow(Fake);
        impl Outputs for Slow {
            fn media(&mut self, a: MediaAction) {
                self.0.media(a)
            }
            fn level(&mut self) -> Option<Level> {
                Some(Level { volume: 0.5, muted: false, steps: 51 })
            }
            fn set_volume(&mut self, f: f32) {
                self.0.volumes.push(f)
            }
            fn set_muted(&mut self, m: bool) {
                self.0.mutes.push(m)
            }
        }
        let mut out = Slow(Fake::default());
        let t0 = Instant::now();
        for i in 0..3u64 {
            assert!(ex.run(Do::Volume(VolumeAction::Up), t0 + Duration::from_millis(i * 30), &mut out));
        }
        let v = &out.0.volumes;
        assert!((v[0] - 0.52).abs() < 1e-6 && (v[1] - 0.54).abs() < 1e-6 && (v[2] - 0.56).abs() < 1e-6, "{v:?}");
        // a press long after: the reading is trusted again
        ex.run(Do::Volume(VolumeAction::Up), t0 + Duration::from_secs(5), &mut out);
        assert!((out.0.volumes[3] - 0.52).abs() < 1e-6);
    }

    #[test]
    fn media_commands_go_to_the_media_session_and_nothing_else_is_touched() {
        let mut ex = Executor::new();
        let mut out = Fake::default();
        for a in [MediaAction::Next, MediaAction::Previous, MediaAction::PlayPause, MediaAction::Stop] {
            assert!(ex.run(Do::Media(a), Instant::now(), &mut out));
        }
        assert_eq!(out.media, [MediaAction::Next, MediaAction::Previous, MediaAction::PlayPause, MediaAction::Stop]);
        assert!(out.volumes.is_empty() && out.mutes.is_empty());
        assert!(!ex.run(Do::Nothing, Instant::now(), &mut out));
    }

    #[test]
    fn with_no_sound_device_a_volume_key_does_nothing() {
        let mut ex = Executor::new();
        let mut out = Fake::default(); // level None
        assert!(!ex.run(Do::Volume(VolumeAction::Up), Instant::now(), &mut out));
        assert!(out.volumes.is_empty() && out.mutes.is_empty());
    }

    #[test]
    fn mute_toggles_from_the_reading_and_a_muted_volume_up_unmutes() {
        let mut ex = Executor::new();
        let mut out = Fake { level: Some(Level { volume: 0.3, muted: true, steps: 51 }), ..Default::default() };
        ex.run(Do::Volume(VolumeAction::ToggleMute), Instant::now(), &mut out);
        assert_eq!(out.mutes, [false]);
        ex.run(Do::Volume(VolumeAction::Up), Instant::now(), &mut out);
        assert_eq!(out.mutes, [false, false], "volume up on a muted device unmutes it");
        assert!((out.volumes[0] - 0.32).abs() < 1e-6);
    }
}
