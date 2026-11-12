//! "The wallpaper changed": which events tell us (shell plan note 4, 4.3), and what is done with them.
//!
//! THE EVENTS, none of them trusted to say WHAT changed (they only say "look again"):
//! 1. `WM_SETTINGCHANGE` with `wParam == SPI_SETDESKWALLPAPER (0x14)` on a top-level window. Standard, UNVERIFIED on this build. This
//!    needs a window, which this module does not have: the desktop windows call [`Follower::poke`] with
//!    [`Cause::SettingChange`] when [`is_wallpaper_setting_change`] says so.
//! 2. `RegNotifyChangeKeyValue` on `Control Panel\Desktop`, `Explorer\Wallpapers` and `Explorer\VirtualDesktops` (a picture set, a
//!    slideshow tick, a switch of virtual desktop): [`SystemWatch`].
//! 3. A file watch on the Themes folder (`%APPDATA%\Microsoft\Windows\Themes`): the transcoded copy is rewritten IN PLACE, so the path
//!    stays the same and only the modified time moves: [`SystemWatch`] with `rf-win`'s directory watcher.
//!
//! A slideshow tick has no documented event; with Explorer gone the app runs the slideshow itself.
//!
//! WHAT IS DONE. Events arrive in bursts (Windows writes several values for one change). [`Follower`] waits until `debounce` passes
//! with no new event, reads the wallpaper once, and reports it only when `rf_core::desktop::wallpaper::Tracker` says something is
//! different from the last read (another picture, the same path rewritten, position, colour, monitors, kind).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
pub use rf_core::desktop::wallpaper::{Cause, Changed, is_wallpaper_setting_change};
use rf_core::desktop::wallpaper::{Stamper, Tracker, WATCHED_KEYS, Wallpaper};

/// Where events come from. Dropping the returned guard stops them.
pub trait WallpaperWatch {
    fn start(&self, notify: Arc<dyn Fn(Cause) + Send + Sync>) -> Box<dyn Send>;
}

/// Reads the wallpaper (the real one: `wallpaper::current`).
pub trait WallpaperReader: Send + Sync {
    fn read(&self) -> Wallpaper;
}

/// The registry keys and the Themes folder.
pub struct SystemWatch {
    /// `%APPDATA%\Microsoft\Windows\Themes`, or `None` to watch only the registry.
    pub themes_dir: Option<PathBuf>,
}

impl SystemWatch {
    /// The usual Themes folder of the current user.
    pub fn standard() -> Self {
        SystemWatch { themes_dir: std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(r"Microsoft\Windows\Themes")).filter(|p| p.is_dir()) }
    }
}

struct Guards {
    _reg: super::regnotify::RegWatch,
    _dir: Option<crate::watch::DirWatch>,
}

impl WallpaperWatch for SystemWatch {
    fn start(&self, notify: Arc<dyn Fn(Cause) + Send + Sync>) -> Box<dyn Send> {
        let n = notify.clone();
        let reg = super::regnotify::watch_keys(&WATCHED_KEYS, Arc::new(move |k| n(Cause::RegistryKey(k))));
        let dir = self.themes_dir.as_ref().and_then(|d| {
            let n = notify.clone();
            crate::watch::watch_dir(d, Duration::from_millis(150), move || n(Cause::ThemesFolder)).ok()
        });
        Box::new(Guards { _reg: reg, _dir: dir })
    }
}

/// Follows the wallpaper: owns the event source and the thread that debounces and compares.
pub struct Follower {
    tx: Sender<Cause>,
    _guard: Box<dyn Send>,
}

impl Follower {
    /// `on_change` runs on the follower's thread, first with the initial read (`Changed::first`), then for every real change.
    pub fn start(
        watch: &dyn WallpaperWatch,
        reader: Arc<dyn WallpaperReader>,
        stamper: Arc<dyn Stamper + Send + Sync>,
        debounce: Duration,
        on_change: impl Fn(Wallpaper, Changed) + Send + Sync + 'static,
    ) -> Follower {
        let (tx, rx) = crossbeam_channel::unbounded::<Cause>();
        let tx2 = tx.clone();
        let guard = watch.start(Arc::new(move |c| {
            let _ = tx2.send(c);
        }));
        let _ = tx.send(Cause::Manual); // the first read
        std::thread::Builder::new()
            .name("rf-wallfollow".into())
            .spawn(move || run(rx, reader, stamper, debounce, on_change))
            .expect("spawn the wallpaper follower");
        Follower { tx, _guard: guard }
    }

    /// Asks for a fresh read, for events only a window can hear (`WM_SETTINGCHANGE`).
    pub fn poke(&self, cause: Cause) {
        let _ = self.tx.send(cause);
    }
}

fn run(rx: Receiver<Cause>, reader: Arc<dyn WallpaperReader>, stamper: Arc<dyn Stamper + Send + Sync>, debounce: Duration, on_change: impl Fn(Wallpaper, Changed)) {
    let mut tracker = Tracker::default();
    while let Ok(_first) = rx.recv() {
        // a burst: wait for quiet, but not for ever (the watched keys are subtree watches; a slideshow or a chatty tool can write more
        // often than the debounce interval for as long as it likes)
        if !super::settle(&rx, debounce, debounce * 5) {
            return;
        }
        let now = reader.read();
        if let Some(changed) = tracker.update(now.clone(), stamper.as_ref()) {
            on_change(now, changed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use rf_core::desktop::geom::{Rect, Rgb};
    use rf_core::desktop::placement::Position;
    use rf_core::desktop::wallpaper::{BackgroundKind, FileStamp, MonitorWallpaper, Source};
    use std::path::Path;
    use std::sync::atomic::{AtomicUsize, Ordering};

    type Notify = Arc<dyn Fn(Cause) + Send + Sync>;
    type Log = Arc<Mutex<Vec<(String, Changed)>>>;

    /// An event source the test fires by hand.
    struct Hand(Mutex<Option<Notify>>);
    impl WallpaperWatch for Hand {
        fn start(&self, notify: Arc<dyn Fn(Cause) + Send + Sync>) -> Box<dyn Send> {
            *self.0.lock() = Some(notify);
            Box::new(())
        }
    }
    impl Hand {
        fn fire(&self, c: Cause) {
            (self.0.lock().as_ref().unwrap())(c);
        }
    }

    struct Reader {
        pic: Mutex<String>,
        position: Mutex<Position>,
        reads: AtomicUsize,
    }
    impl WallpaperReader for Reader {
        fn read(&self) -> Wallpaper {
            self.reads.fetch_add(1, Ordering::SeqCst);
            Wallpaper {
                source: Source::Registry,
                monitors: vec![MonitorWallpaper { device: "M".into(), rect: Rect::new(0, 0, 10, 10), picture: Some(PathBuf::from(self.pic.lock().clone())) }],
                position: *self.position.lock(),
                position_assumed: false,
                background: Rgb(0, 0, 0),
                kind: BackgroundKind::Picture,
                slideshow: None,
                per_desktop: vec![],
            }
        }
    }
    struct Stamps(Mutex<i64>);
    impl Stamper for Stamps {
        fn stamp(&self, _: &Path) -> Option<FileStamp> {
            Some(FileStamp { modified_ms: *self.0.lock(), size: 1 })
        }
    }

    fn setup() -> (Arc<Hand>, Arc<Reader>, Arc<Stamps>, Log, Follower) {
        let hand = Arc::new(Hand(Mutex::new(None)));
        let reader = Arc::new(Reader { pic: Mutex::new("a".into()), position: Mutex::new(Position::Fill), reads: AtomicUsize::new(0) });
        let stamps = Arc::new(Stamps(Mutex::new(1)));
        let log = Arc::new(Mutex::new(Vec::new()));
        let l2 = log.clone();
        let f = Follower::start(&*hand, reader.clone(), stamps.clone(), Duration::from_millis(60), move |w, c| {
            l2.lock().push((w.monitors[0].picture.clone().unwrap().to_string_lossy().into_owned(), c));
        });
        (hand, reader, stamps, log, f)
    }
    fn wait_for(log: &Mutex<Vec<(String, Changed)>>, n: usize) {
        let t = std::time::Instant::now();
        while log.lock().len() < n && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn the_first_read_then_only_real_changes_and_a_burst_is_one_read() {
        let (hand, reader, stamps, log, f) = setup();
        wait_for(&log, 1);
        assert!(log.lock()[0].1.first);
        assert_eq!(reader.reads.load(Ordering::SeqCst), 1);

        // an event with nothing different: a read, no report
        hand.fire(Cause::RegistryKey("Control Panel\\Desktop"));
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(reader.reads.load(Ordering::SeqCst), 2);
        assert_eq!(log.lock().len(), 1);

        // a burst of five events while the picture changes: one read, one report
        *reader.pic.lock() = "b".into();
        for _ in 0..5 {
            hand.fire(Cause::RegistryKey("Control Panel\\Desktop"));
            std::thread::sleep(Duration::from_millis(10));
        }
        wait_for(&log, 2);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(reader.reads.load(Ordering::SeqCst), 3, "the burst was read once");
        assert_eq!(log.lock().len(), 2);
        assert!(log.lock()[1].1.pictures);
        assert_eq!(log.lock()[1].0, "b");

        // the file is rewritten in place: same path, new modified time (the Themes folder watch fires)
        *stamps.0.lock() = 2;
        hand.fire(Cause::ThemesFolder);
        wait_for(&log, 3);
        assert!(log.lock()[2].1.rewritten && !log.lock()[2].1.pictures);

        // a window heard WM_SETTINGCHANGE and pokes; the position changed
        *reader.position.lock() = Position::Fit;
        assert!(is_wallpaper_setting_change(0x14));
        f.poke(Cause::SettingChange);
        wait_for(&log, 4);
        assert!(log.lock()[3].1.position);
        drop(f);
    }

    #[test]
    fn a_steady_stream_of_events_does_not_hold_the_read_off_for_ever() {
        let (hand, reader, _stamps, log, f) = setup(); // debounce 60 ms, so a burst is read after 300 ms at the latest
        // an event every 15 ms for 0.9 s: never a quiet moment of 60 ms
        let t = std::time::Instant::now();
        while t.elapsed() < Duration::from_millis(900) {
            hand.fire(Cause::RegistryKey("Control Panel\\Desktop"));
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(reader.reads.load(Ordering::SeqCst) >= 1, "the wallpaper was read while the events were still coming");
        wait_for(&log, 1);
        assert!(log.lock()[0].1.first);
        drop(f);
    }

    #[test]
    fn the_real_watch_starts_and_stops() {
        let dir = std::env::temp_dir().join(format!("rf-wallwatch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = SystemWatch { themes_dir: Some(dir.clone()) };
        let hits = Arc::new(AtomicUsize::new(0));
        let h2 = hits.clone();
        let guard = w.start(Arc::new(move |c| {
            if matches!(c, Cause::ThemesFolder) {
                h2.fetch_add(1, Ordering::SeqCst);
            }
        }));
        std::thread::sleep(Duration::from_millis(100));
        // a file written in the (temp) Themes folder is heard
        std::fs::write(dir.join("TranscodedWallpaper"), b"x").unwrap();
        let t = std::time::Instant::now();
        while hits.load(Ordering::SeqCst) == 0 && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(hits.load(Ordering::SeqCst) >= 1, "the folder watch reports a write");
        drop(guard);
        let _ = std::fs::remove_dir_all(&dir);
        // the standard folder exists on a normal profile, and the constructor does not fail without it
        let _ = SystemWatch::standard();
    }
}
