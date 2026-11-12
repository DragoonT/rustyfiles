//! The owner of the one hook: installs it when a setting goes on, removes it when both are off, keeps the heartbeat, notices a hook
//! that Windows dropped, and carries the layer's posts to the interface. Runs on the interface thread (the settings and the
//! heartbeat) plus one small dispatcher thread; the hook callback itself is `layer.rs`.
//!
//! WHEN THE HOOK EXISTS. Only inside the shell host, only through this type, and only while `shell_win_key` or `shell_own_switcher`
//! is on: [`KeyService::set_wanted`] installs it when the first one goes on and removes it when both are off; [`KeyService::stop`]
//! (called at quit) and `Drop` remove it too. A process that is killed loses its hooks with it: Windows' own behaviour is back at once.
//!
//! HOW A DROPPED HOOK IS NOTICED. Windows removes a low-level hook without a word when its callback is too slow; ours never waits, so
//! the usual causes are a stall of the whole process (it was suspended, the machine slept, the process was starved) - or a bug. Three
//! checks, all made by [`KeyService::heartbeat`] on the interface thread, none of which sends a key:
//! 1. THE THREAD ENDED: the feed says it is not running (the hook thread died, or the hook could not be installed in the first place).
//! 2. A STALL: the heartbeat itself came more than [`STALL_MS`] after the previous one, so the process was away for a long moment and
//!    the hook may have been dropped meanwhile.
//! 3. A DISAGREEMENT THAT LASTS: the layer keeps a belief about the Windows keys and Alt (never swallowed, so what reached the system)
//!    and the heartbeat reads the keyboard's own state of them; two beats in a row (about half a second) that disagree mean the hook
//!    is no longer being called. Only a key held that long shows it: a hook dropped for no visible reason is found the first time Alt
//!    or the Windows key is held for half a second, and until then the keys simply do what Windows does. Tab is left out on purpose
//!    (whether the keyboard's state shows a key the hook swallowed is not known). A release the hook cannot see (Win+L: the key comes
//!    up on the secure desktop) also looks like a disagreement and installs the hook again; that is wanted, a fresh layer starts from
//!    the keyboard's state and forgets a Windows key it believes is still down.
//!
//! A SECOND, SEPARATE CHECK at the same beats: a chord that is open as a HELD one (the layer thinks Alt is down) while the keyboard
//! says Alt is up for two beats in a row. This happens when Alt came up where the hook cannot see (a window of a higher integrity level
//! gets no keys from a hook of ours). The chord is ended on the spot and the interface is told (`Lost`), so Tab, the arrows, Enter and
//! Escape are not swallowed for a switcher nobody is steering.
//!
//! A dropped hook is installed again from scratch (a fresh layer, seeded from the keyboard's state), and the interface is told the layer
//! lost track (`Lost`), which closes a switcher that was open. Installing again is limited: not twice within [`MIN_GAP_MS`], and after
//! [`MAX_REINSTALLS`] within a minute the layer gives up. It does not stay off for good: after [`GIVE_UP_COOLDOWN_MS`] the next
//! heartbeat tries the hook again from scratch (a shell host with no Explorer has no other Windows key, and a hook that kept being
//! lost because of a passing cause, a sleeping machine or a remote session that came and went, should come back by itself); if it is
//! lost again it gives up again, for another pause. Switching the settings off and on starts afresh at once.

use std::sync::Arc;
use std::thread::JoinHandle;

use super::layer::KeyLayer;
use super::machine::{VK_LWIN, VK_RWIN};
use super::mask::{MaskSender, SendInputMask};
use super::queue::Posted;
use super::shared::{Clock, Shared, Tracked};
use crate::switcher::hook::{KeyFeed, LowLevelHook, VK_LMENU, VK_RMENU};

/// The heartbeat is considered late, and the process to have been away, after this long since the previous one.
pub const STALL_MS: u64 = 3_000;
/// Two installations of the hook are at least this far apart.
pub const MIN_GAP_MS: u64 = 2_000;
/// After this many installations within [`WINDOW_MS`] the layer gives up.
pub const MAX_REINSTALLS: usize = 5;
pub const WINDOW_MS: u64 = 60_000;
/// After giving up, the layer waits this long and then tries the hook again.
pub const GIVE_UP_COOLDOWN_MS: u64 = 300_000;

/// What the interface does with a post. Called on the dispatcher thread; it must be quick and must not call back into the service.
pub type Deliver = Arc<dyn Fn(Posted) + Send + Sync>;

/// The keyboard's own state, asked of the system.
pub trait KeyProbe: Send + Sync {
    /// Whether the key is down right now.
    fn down(&self, vk: u32) -> bool;
}

/// `GetAsyncKeyState`.
pub struct SystemProbe;

impl KeyProbe for SystemProbe {
    fn down(&self, vk: u32) -> bool {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
        // SAFETY: a plain query of the keyboard state.
        (unsafe { GetAsyncKeyState(vk as i32) } as u16 & 0x8000) != 0
    }
}

fn read_tracked(p: &dyn KeyProbe) -> Tracked {
    Tracked { win: p.down(VK_LWIN) || p.down(VK_RWIN), alt: p.down(VK_LMENU) || p.down(VK_RMENU) }
}

/// Counts consecutive beats at which the layer's belief and the keyboard disagree.
#[derive(Clone, Copy, Debug, Default)]
pub struct Watchdog {
    strikes: u8,
}

impl Watchdog {
    /// True when the hook has to be installed again.
    pub fn step(&mut self, believed: Tracked, actual: Tracked) -> bool {
        if believed == actual {
            self.strikes = 0;
            return false;
        }
        self.strikes = self.strikes.saturating_add(1);
        self.strikes >= 2
    }

    pub fn clear(&mut self) {
        self.strikes = 0;
    }
}

/// Why the hook was installed again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    ThreadEnded,
    Stall,
    Disagreement,
    /// The pause after giving up is over.
    Pause,
}

/// What a heartbeat found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Beat {
    /// Not installed (both settings off).
    Idle,
    Fine,
    /// The hook was installed again for this reason.
    Reinstalled(Why),
    /// A reinstallation was wanted and refused (too soon after the last).
    Deferred(Why),
    /// Too many reinstallations in a minute: the layer is off for [`GIVE_UP_COOLDOWN_MS`] (or until the settings are switched off and on).
    GaveUp,
}

struct Running {
    shared: Arc<Shared>,
    dispatcher: Option<JoinHandle<()>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

pub struct KeyService<F: KeyFeed> {
    feed: F,
    mask: Arc<dyn MaskSender>,
    probe: Arc<dyn KeyProbe>,
    deliver: Deliver,
    clock: Arc<Clock>,
    log_only: bool,
    running: Option<Running>,
    watchdog: Watchdog,
    last_beat: Option<u64>,
    installs: Vec<u64>,
    gave_up: bool,
    /// When the layer gave up (the clock's reading).
    gave_up_at: Option<u64>,
    /// Win+E and Win+D are to be taken (set by the host while no Explorer is the shell).
    combos: bool,
    mask_failures_logged: u32,
    /// Consecutive beats at which the chord was held and the keyboard said Alt was up.
    chord_strikes: u8,
}

impl KeyService<LowLevelHook> {
    /// The real service: the system's hook, the system's `SendInput`, the keyboard's own state. Nothing is installed until
    /// [`KeyService::set_wanted`] asks for it.
    pub fn system(deliver: Deliver, log_only: bool) -> KeyService<LowLevelHook> {
        KeyService::new(LowLevelHook::new(), Arc::new(SendInputMask), Box::new(SystemProbe), deliver, Arc::new(Clock::real()), log_only)
    }
}

impl<F: KeyFeed> KeyService<F> {
    pub fn new(feed: F, mask: Arc<dyn MaskSender>, probe: Box<dyn KeyProbe>, deliver: Deliver, clock: Arc<Clock>, log_only: bool) -> KeyService<F> {
        KeyService {
            feed,
            mask,
            probe: probe.into(),
            deliver,
            clock,
            log_only,
            running: None,
            watchdog: Watchdog::default(),
            last_beat: None,
            installs: Vec::new(),
            gave_up: false,
            gave_up_at: None,
            combos: false,
            mask_failures_logged: 0,
            chord_strikes: 0,
        }
    }

    /// Whether any setting is on (the hook is wanted, installed or not).
    pub fn wanted(&self) -> bool {
        self.running.is_some()
    }

    /// Whether the hook is running now.
    pub fn installed(&self) -> bool {
        self.running.is_some() && self.feed.running()
    }

    /// The state shared with the hook (for the interface's `on_closed` and `give_up` calls); `None` when both settings are off.
    pub fn shared(&self) -> Option<Arc<Shared>> {
        self.running.as_ref().map(|r| r.shared.clone())
    }

    /// The feed (tests push key events through it).
    pub fn feed_mut(&mut self) -> &mut F {
        &mut self.feed
    }

    pub fn log_only(&self) -> bool {
        self.log_only
    }

    /// The settings changed. Installs the hook when the first one goes on, tells the running layer about the change otherwise, and
    /// removes everything when both are off. An error is a hook that could not be installed; the service keeps trying at its beats
    /// (within the limits above) as long as a setting is on.
    pub fn set_wanted(&mut self, win_key: bool, own_switcher: bool) -> anyhow::Result<()> {
        if !win_key && !own_switcher {
            self.stop_because("both settings are off");
            return Ok(());
        }
        if self.running.is_none() {
            self.gave_up = false;
            self.installs.clear();
            self.watchdog.clear();
            self.chord_strikes = 0;
            self.last_beat = None;
            let shared = Arc::new(Shared::new(self.clock.clone(), self.log_only));
            shared.set_wanted(win_key, own_switcher);
            shared.set_combos(self.combos);
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let dispatcher = spawn_dispatcher(shared.clone(), stop.clone(), self.deliver.clone());
            // (the first beat, so that the hook does not start out failing open for a quarter of a second)
            shared.beat(false);
            self.running = Some(Running { shared, dispatcher, stop });
            log::info!("keys: layer on (windows key: {}, switcher: {}, log only: {})", yn(win_key), yn(own_switcher), yn(self.log_only));
            return self.install();
        }
        if let Some(r) = &self.running {
            r.shared.set_wanted(win_key, own_switcher);
        }
        Ok(())
    }

    /// Win+E and Win+D are taken (true) or left to whoever answers them (false). Takes effect at once on a running layer, and is
    /// remembered for one that starts later. The host sets it from who owns the shell window.
    pub fn set_combos(&mut self, on: bool) {
        self.combos = on;
        if let Some(r) = &self.running {
            r.shared.set_combos(on);
        }
    }

    /// The interface thread is alive: stamps the heartbeat, hands over whether a full-screen program is in front, and checks that the
    /// hook is still being called. Call it every [`HEARTBEAT_EVERY_MS`](super::shared::HEARTBEAT_EVERY_MS).
    pub fn heartbeat(&mut self, fullscreen: bool) -> Beat {
        let Some(shared) = self.running.as_ref().map(|r| r.shared.clone()) else { return Beat::Idle };
        let now = self.clock.now_ms();
        let stalled = self.last_beat.is_some_and(|l| now.saturating_sub(l) > STALL_MS);
        self.last_beat = Some(now);
        shared.beat(fullscreen);
        let failures = shared.mask_failures();
        if failures > self.mask_failures_logged {
            self.mask_failures_logged = failures;
            log::warn!("keys: the system did not take the mask key {failures} time(s); Windows' own Start may open as well");
        }
        if self.gave_up {
            let waited = now.saturating_sub(self.gave_up_at.unwrap_or(now));
            if waited < GIVE_UP_COOLDOWN_MS {
                return Beat::GaveUp;
            }
            // the pause is over: from scratch, as after a switch off and on
            log::warn!("keys: the layer was off for {} s after the hook kept getting lost; trying the hook again", waited / 1000);
            self.gave_up = false;
            self.gave_up_at = None;
            self.installs.clear();
            self.watchdog.clear();
            self.chord_strikes = 0;
            if let Err(e) = self.install() {
                log::error!("keys: the hook could not be installed after the pause: {e:#}");
            }
            return Beat::Reinstalled(Why::Pause);
        }
        let actual = read_tracked(self.probe.as_ref());
        self.watch_chord(&shared, actual);
        let why = if !self.feed.running() {
            Some(Why::ThreadEnded)
        } else if stalled {
            Some(Why::Stall)
        } else if self.watchdog.step(shared.believed(), actual) {
            Some(Why::Disagreement)
        } else {
            None
        };
        let Some(why) = why else { return Beat::Fine };
        self.reinstall(why, now, &shared)
    }

    /// A chord held open with Alt up on the keyboard for two beats is ended here (see the module's second check). Fires once per
    /// episode. `deliver` is called from this thread: it only queues the post, as it does for the dispatcher.
    fn watch_chord(&mut self, shared: &Arc<Shared>, actual: Tracked) {
        if !(shared.chord_held() && !actual.alt) {
            self.chord_strikes = 0;
            return;
        }
        self.chord_strikes = self.chord_strikes.saturating_add(1);
        if self.chord_strikes == 2 {
            log::warn!("keys: Alt was let go where the hook could not see it; the switcher is closed");
            shared.close_current_chord();
            (self.deliver)(Posted::Lost);
        }
    }

    fn reinstall(&mut self, why: Why, now: u64, shared: &Arc<Shared>) -> Beat {
        self.installs.retain(|t| now.saturating_sub(*t) <= WINDOW_MS);
        if self.installs.len() >= MAX_REINSTALLS {
            self.gave_up = true;
            self.gave_up_at = Some(now);
            self.feed.stop();
            log::error!("keys: the hook keeps getting lost; the keyboard layer is off for {} minutes (or until the settings are switched off and on again)", GIVE_UP_COOLDOWN_MS / 60_000);
            return Beat::GaveUp;
        }
        if self.installs.last().is_some_and(|t| now.saturating_sub(*t) < MIN_GAP_MS) {
            return Beat::Deferred(why);
        }
        log::warn!("keys: installing the hook again ({})", match why {
            Why::ThreadEnded => "the hook's thread had ended",
            Why::Stall => "the process was away for a long moment",
            Why::Disagreement => "the hook stopped seeing the keys",
            Why::Pause => "the pause after giving up is over",
        });
        self.feed.stop();
        self.watchdog.clear();
        self.chord_strikes = 0;
        // whatever chord was open is over; the interface closes a switcher that is still up
        shared.post(Posted::Lost);
        if let Err(e) = self.install() {
            log::error!("keys: the hook could not be installed again: {e:#}");
        }
        Beat::Reinstalled(why)
    }

    /// Installs the hook with a fresh layer, seeded from the keyboard's state.
    fn install(&mut self) -> anyhow::Result<()> {
        let Some(shared) = self.running.as_ref().map(|r| r.shared.clone()) else { return Ok(()) };
        let now = self.clock.now_ms();
        self.installs.push(now);
        let (l, r) = (self.probe.down(VK_LWIN), self.probe.down(VK_RWIN));
        let mut layer = KeyLayer::new(shared.clone(), self.mask.clone(), l, r).with_probe(self.probe.clone());
        shared.set_believed(read_tracked(self.probe.as_ref()));
        match self.feed.start_raw(Box::new(move |k| layer.on_event(k))) {
            Ok(()) => {
                log::info!("keys: hook installed");
                Ok(())
            }
            Err(e) => {
                log::error!("keys: the hook could not be installed: {e:#}");
                Err(e)
            }
        }
    }

    /// Removes the hook and everything that served it. Safe to call twice.
    pub fn stop(&mut self) {
        self.stop_because("stopped");
    }

    /// [`KeyService::stop`] with the reason that goes into the log line (`keys: layer off, hook removed (<why>)`). A reason is a short
    /// phrase of the host's, never a key.
    pub fn stop_because(&mut self, why: &str) {
        let Some(mut r) = self.running.take() else { return };
        self.feed.stop();
        r.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(d) = r.dispatcher.take() {
            d.thread().unpark();
            let _ = d.join();
        }
        self.gave_up = false;
        self.gave_up_at = None;
        self.last_beat = None;
        log::info!("keys: layer off, hook removed ({why})");
    }
}

impl<F: KeyFeed> Drop for KeyService<F> {
    fn drop(&mut self) {
        self.stop_because("the service was dropped");
    }
}

fn yn(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}

/// The thread that carries posts from the ring to the interface. Wakes when the hook posts; ends when told.
fn spawn_dispatcher(shared: Arc<Shared>, stop: Arc<std::sync::atomic::AtomicBool>, deliver: Deliver) -> Option<JoinHandle<()>> {
    let for_waker = shared.clone();
    let handle = std::thread::Builder::new()
        .name("rf-keys-post".into())
        .spawn(move || {
            loop {
                while let Some(p) = shared.pop() {
                    deliver(p);
                }
                if shared.take_lost() {
                    deliver(Posted::Lost);
                }
                if stop.load(std::sync::atomic::Ordering::Acquire) {
                    break;
                }
                std::thread::park();
            }
        })
        .ok()?;
    // (before the hook exists: every post from now on wakes this thread)
    for_waker.set_waker(handle.thread().clone());
    Some(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::switcher::hook::{FakeFeed, LLKHF_ALTDOWN, VK_TAB};
    use crate::winkey::fakes::{FakeMask, FakeProbe, RefusingFeed};
    use crate::winkey::shared::STALE_AFTER_MS;
    use parking_lot::Mutex;

    struct Rig {
        svc: KeyService<FakeFeed>,
        clock: Arc<Clock>,
        mask: Arc<FakeMask>,
        probe: Arc<FakeProbe>,
        got: Arc<Mutex<Vec<Posted>>>,
    }

    struct SharedProbe(Arc<FakeProbe>);
    impl KeyProbe for SharedProbe {
        fn down(&self, vk: u32) -> bool {
            self.0.down(vk)
        }
    }

    fn rig_with(log_only: bool) -> Rig {
        let clock = Arc::new(Clock::manual());
        clock.set(1_000);
        let mask = Arc::new(FakeMask::default());
        let probe = Arc::new(FakeProbe::default());
        let got: Arc<Mutex<Vec<Posted>>> = Default::default();
        let g = got.clone();
        let deliver: Deliver = Arc::new(move |p| g.lock().push(p));
        let svc = KeyService::new(FakeFeed::default(), mask.clone(), Box::new(SharedProbe(probe.clone())), deliver, clock.clone(), log_only);
        Rig { svc, clock, mask, probe, got }
    }

    fn rig() -> Rig {
        rig_with(false)
    }

    fn wait_for(got: &Arc<Mutex<Vec<Posted>>>, n: usize) -> Vec<Posted> {
        for _ in 0..400 {
            if got.lock().len() >= n {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        got.lock().clone()
    }

    #[test]
    fn nothing_is_installed_until_a_setting_is_on() {
        let mut r = rig();
        assert!(!r.svc.wanted() && !r.svc.installed());
        assert_eq!(r.svc.heartbeat(false), Beat::Idle);
        assert!(r.svc.shared().is_none());
    }

    #[test]
    fn the_hook_goes_in_when_the_first_setting_goes_on_and_out_when_both_are_off() {
        let mut r = rig();
        r.svc.set_wanted(true, false).unwrap();
        assert!(r.svc.installed());
        r.svc.set_wanted(true, true).unwrap();
        assert!(r.svc.installed(), "a second setting changes nothing about the hook");
        r.svc.set_wanted(false, true).unwrap();
        assert!(r.svc.installed(), "one is still on");
        r.svc.set_wanted(false, false).unwrap();
        assert!(!r.svc.installed() && !r.svc.wanted(), "both off: the hook is removed");
        assert!(r.svc.shared().is_none());
    }

    #[test]
    fn stop_removes_the_hook_and_is_safe_twice() {
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.stop();
        assert!(!r.svc.installed());
        r.svc.stop();
    }

    #[test]
    fn dropping_the_service_removes_the_hook() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        let shared = r.svc.shared().unwrap();
        drop(r.svc);
        // (the feed went with the service; the dispatcher ended: the shared state has no waiting thread left to wake)
        shared.post(Posted::StartTap);
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert!(r.got.lock().is_empty(), "nothing is delivered after the service is gone");
    }

    #[test]
    fn a_setting_flipping_while_installed_reaches_the_running_layer() {
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.heartbeat(false);
        // Win tap with the Windows-key setting off: nothing
        r.svc.feed_mut().push_raw(VK_LWIN, 0x5B, 0, true);
        r.svc.feed_mut().push_raw(VK_LWIN, 0x5B, 0, false);
        assert_eq!(r.mask.sent(), 0);
        r.svc.set_wanted(true, true).unwrap();
        r.svc.feed_mut().push_raw(VK_LWIN, 0x5B, 0, true);
        r.svc.feed_mut().push_raw(VK_LWIN, 0x5B, 0, false);
        assert_eq!(r.mask.sent(), 1);
        assert_eq!(wait_for(&r.got, 1), vec![Posted::StartTap]);
    }

    #[test]
    fn the_layers_posts_reach_the_interface_through_the_dispatcher_thread() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        r.svc.heartbeat(false);
        let f = r.svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true));
        f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, false);
        f.push_raw(VK_LMENU, 0x38, 0, false);
        let got = wait_for(&r.got, 2);
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(matches!(got[0], Posted::Chord { effect: crate::switcher::chord::Effect::Open { .. }, .. }));
        assert!(matches!(got[1], Posted::Chord { effect: crate::switcher::chord::Effect::Choose, .. }));
    }

    #[test]
    fn the_first_beat_is_made_at_install_so_the_layer_does_not_start_out_failing_open() {
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        let f = r.svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true));
    }

    #[test]
    fn the_layer_fails_open_when_the_interface_stops_beating() {
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.heartbeat(false);
        r.clock.set(1_000 + STALE_AFTER_MS + 10);
        let f = r.svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(!f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true), "stuck interface: Alt+Tab is Windows'");
    }

    #[test]
    fn log_only_mode_is_handed_to_the_layer() {
        let mut r = rig_with(true);
        assert!(r.svc.log_only());
        r.svc.set_wanted(true, true).unwrap();
        r.svc.heartbeat(false);
        let f = r.svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        assert!(!f.push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true));
    }

    // ---------------------------------------------------------------- the watchdog and the reinstallation

    #[test]
    fn the_watchdog_needs_two_beats_in_a_row_to_disagree() {
        let mut w = Watchdog::default();
        let down = Tracked { win: true, alt: false };
        assert!(!w.step(Tracked::default(), down), "one beat: a key may have gone down a moment ago");
        assert!(w.step(Tracked::default(), down), "two beats: the hook is not seeing it");
        let mut w = Watchdog::default();
        assert!(!w.step(Tracked::default(), down));
        assert!(!w.step(down, down), "agreement in between clears it");
        assert!(!w.step(Tracked::default(), down));
    }

    #[test]
    fn a_dead_hook_thread_is_installed_again_and_the_interface_is_told_the_layer_lost_track() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        r.svc.feed_mut().stop(); // the hook went away behind the service's back
        r.clock.set(1_000 + MIN_GAP_MS + 10);
        assert_eq!(r.svc.heartbeat(false), Beat::Reinstalled(Why::ThreadEnded));
        assert!(r.svc.installed());
        assert_eq!(wait_for(&r.got, 1), vec![Posted::Lost]);
    }

    #[test]
    fn a_long_stall_installs_the_hook_again() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        r.clock.set(1_100);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        r.clock.set(1_100 + STALL_MS + 5);
        assert_eq!(r.svc.heartbeat(false), Beat::Reinstalled(Why::Stall));
    }

    #[test]
    fn a_keyboard_that_shows_a_held_key_the_hook_never_saw_installs_the_hook_again_at_the_second_beat() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        r.clock.set(1_300);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        r.probe.set(VK_LWIN, true); // held, and the layer saw nothing
        r.clock.set(3_500);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine, "the first disagreement");
        r.clock.set(3_750);
        assert_eq!(r.svc.heartbeat(false), Beat::Reinstalled(Why::Disagreement));
        // the new layer starts from the keyboard's state: the Windows key is down, so its release is no tap
        r.svc.feed_mut().push_raw(VK_LWIN, 0x5B, 0, false);
        assert_eq!(r.mask.sent(), 0);
    }

    #[test]
    fn a_key_the_hook_did_see_is_no_disagreement() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        r.svc.heartbeat(false);
        r.svc.feed_mut().push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        r.probe.set(VK_LMENU, true);
        for t in [1_300u64, 1_550, 1_800, 2_050] {
            r.clock.set(t);
            assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        }
    }

    #[test]
    fn a_tab_the_hook_swallowed_is_not_compared_with_the_keyboard_so_cycling_does_not_reinstall_the_hook() {
        // Whether GetAsyncKeyState shows a key that a low-level hook swallowed is not known (not run here). If it does, holding Tab
        // during Alt+Tab would look like a hook that stopped seeing the keys: reinstalled mid-chord, the switcher closed. Tab is
        // therefore not among the compared keys: Alt and the Windows keys tell a dropped hook just as well.
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.heartbeat(false);
        let f = r.svc.feed_mut();
        f.push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        r.probe.set(VK_LMENU, true);
        assert!(r.svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true), "swallowed");
        r.probe.set(VK_TAB, true); // the case in doubt: the keyboard shows the swallowed Tab as held
        for t in [1_300u64, 1_550, 1_800, 2_050, 2_300] {
            r.clock.set(t);
            assert_eq!(r.svc.heartbeat(false), Beat::Fine, "at {t}");
        }
        let got = wait_for(&r.got, 1);
        assert_eq!(got.len(), 1, "only the opening was posted: no Lost, the switcher stays: {got:?}");
    }

    #[test]
    fn a_chord_left_open_with_alt_up_on_the_keyboard_is_closed_after_two_beats_and_stops_swallowing() {
        // Alt went down and came up where the hook cannot see (a window of a higher integrity level), and a Tab was seen in between
        // with the "Alt is down" flag: the chord opens as held and its Alt release never comes. The keyboard says Alt is up.
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.heartbeat(false);
        assert!(r.svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true), "opens");
        r.svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, false);
        r.clock.set(1_300);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        assert_eq!(wait_for(&r.got, 1).len(), 1, "one beat is not yet a verdict");
        r.clock.set(1_550);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        let got = wait_for(&r.got, 2);
        assert_eq!(got.last(), Some(&Posted::Lost), "{got:?}");
        // the chord is over without waiting for the interface: the arrow now passes
        assert!(!r.svc.feed_mut().push_raw(0x25, 0x4B, LLKHF_ALTDOWN, true));
    }

    #[test]
    fn a_chord_with_alt_really_held_is_never_closed_by_the_beats() {
        let mut r = rig();
        r.svc.set_wanted(false, true).unwrap();
        r.svc.heartbeat(false);
        r.svc.feed_mut().push_raw(VK_LMENU, 0x38, LLKHF_ALTDOWN, true);
        r.probe.set(VK_LMENU, true);
        assert!(r.svc.feed_mut().push_raw(VK_TAB, 0x0F, LLKHF_ALTDOWN, true));
        for t in (1..=8).map(|i| 1_000 + i * 250) {
            r.clock.set(t);
            assert_eq!(r.svc.heartbeat(false), Beat::Fine);
        }
        assert!(r.svc.feed_mut().push_raw(0x25, 0x4B, LLKHF_ALTDOWN, true), "still ours: the arrow moves in the switcher");
        std::thread::sleep(std::time::Duration::from_millis(30));
        assert_eq!(r.got.lock().iter().filter(|p| **p == Posted::Lost).count(), 0);
    }

    #[test]
    fn reinstalling_is_limited_in_rate_and_in_number() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        // the feed keeps dying
        let mut t = 1_000u64;
        r.svc.feed_mut().stop();
        t += 100;
        r.clock.set(t);
        assert!(matches!(r.svc.heartbeat(false), Beat::Deferred(Why::ThreadEnded)), "less than the minimum gap since the install");
        let mut reinstalled = 0;
        for _ in 0..20 {
            t += MIN_GAP_MS + 100;
            r.clock.set(t);
            r.svc.feed_mut().stop();
            match r.svc.heartbeat(false) {
                Beat::Reinstalled(_) => reinstalled += 1,
                Beat::GaveUp => break,
                other => panic!("{other:?}"),
            }
        }
        assert!(reinstalled < MAX_REINSTALLS, "{reinstalled}");
        assert_eq!(r.svc.heartbeat(false), Beat::GaveUp);
        assert!(!r.svc.feed_mut().running(), "gave up: the hook is out and stays out");
        // switching the settings off and on starts afresh
        r.svc.set_wanted(false, false).unwrap();
        r.svc.set_wanted(true, false).unwrap();
        assert!(r.svc.installed());
    }

    #[test]
    fn a_hook_that_cannot_be_installed_is_reported_and_tried_again_at_the_beats_within_the_limits() {
        let clock = Arc::new(Clock::manual());
        clock.set(500);
        let got: Arc<Mutex<Vec<Posted>>> = Default::default();
        let g = got.clone();
        let deliver: Deliver = Arc::new(move |p| g.lock().push(p));
        let mut svc = KeyService::new(RefusingFeed::default(), Arc::new(FakeMask::default()), Box::new(SharedProbe(Arc::new(FakeProbe::default()))), deliver, clock.clone(), false);
        assert!(svc.set_wanted(true, true).is_err());
        assert!(svc.wanted() && !svc.installed());
        clock.set(500 + MIN_GAP_MS + 1);
        assert_eq!(svc.heartbeat(false), Beat::Reinstalled(Why::ThreadEnded));
        assert_eq!(svc.feed_mut().attempts, 2);
        svc.stop();
    }

    #[test]
    fn after_giving_up_the_layer_comes_back_by_itself_when_the_pause_is_over() {
        let mut r = rig();
        r.svc.set_wanted(true, true).unwrap();
        let mut t = 1_000u64;
        for _ in 0..20 {
            t += MIN_GAP_MS + 100;
            r.clock.set(t);
            r.svc.feed_mut().stop();
            if r.svc.heartbeat(false) == Beat::GaveUp {
                break;
            }
        }
        assert!(!r.svc.feed_mut().running(), "gave up: the hook is out");
        let gave_up_at = t;
        // inside the pause: still off, however many beats come
        for dt in [250, 5_000, 120_000, GIVE_UP_COOLDOWN_MS - 1] {
            r.clock.set(gave_up_at + dt);
            assert_eq!(r.svc.heartbeat(false), Beat::GaveUp, "dt = {dt}");
            assert!(!r.svc.feed_mut().running());
        }
        // the pause is over: the hook is installed again, from scratch
        r.clock.set(gave_up_at + GIVE_UP_COOLDOWN_MS);
        assert_eq!(r.svc.heartbeat(false), Beat::Reinstalled(Why::Pause));
        assert!(r.svc.feed_mut().running() && r.svc.installed());
        // and the counting starts afresh: the next beats are quiet
        r.clock.set(gave_up_at + GIVE_UP_COOLDOWN_MS + 250);
        assert_eq!(r.svc.heartbeat(false), Beat::Fine);
    }

    #[test]
    fn a_layer_that_is_lost_again_after_the_pause_gives_up_again_for_another_pause() {
        let mut r = rig();
        r.svc.set_wanted(true, false).unwrap();
        let mut t = 1_000u64;
        let mut gave_up = 0;
        for _ in 0..60 {
            t += MIN_GAP_MS + 100;
            r.clock.set(t);
            r.svc.feed_mut().stop();
            if r.svc.heartbeat(false) == Beat::GaveUp {
                gave_up += 1;
                if gave_up == 2 {
                    break;
                }
                t += GIVE_UP_COOLDOWN_MS;
            }
        }
        assert_eq!(gave_up, 2);
    }

    #[test]
    fn the_chords_setting_reaches_a_running_layer_and_one_that_starts_later() {
        let mut r = rig();
        r.svc.set_combos(true);
        r.svc.set_wanted(true, false).unwrap();
        assert!(r.svc.shared().unwrap().combos_on(), "remembered from before the layer started");
        r.svc.set_combos(false);
        assert!(!r.svc.shared().unwrap().combos_on(), "applied to the running layer");
        r.svc.set_wanted(false, false).unwrap();
        r.svc.set_combos(true);
        r.svc.set_wanted(true, false).unwrap();
        assert!(r.svc.shared().unwrap().combos_on());
    }

    #[test]
    fn the_service_never_asks_for_the_real_hook_in_tests() {
        // The real service is constructed (nothing is installed by that) and dropped without a setting having been on.
        let svc = KeyService::system(Arc::new(|_: Posted| {}), true);
        assert!(!svc.installed() && !svc.wanted());
    }

}
