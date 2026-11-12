//! Where the dock's tray strip gets its icons from, and where it sends its clicks. The strip (`tray_strip.rs`) knows nothing of how the
//! icons arrive: this file is the one seam between it and the tray host (`rf_win::trayhost`, built by another team at the same time).
//!
//! THE SEAM is five things, and nothing else of the host is touched from the strip:
//!
//! * [`available`]: whether there is a tray host in this build at all (the strip makes nothing when there is none);
//! * [`snapshot`]: the icons now, or `None` when the host is off (the island is then exactly what it was before the strip existed);
//! * [`subscribe`]: a call back, from any thread, whenever the snapshot may have changed (the strip reads [`snapshot`] again);
//! * [`deliver_click`]: a click on an icon (which button, where the pointer is) for the host's click engine to hand to the program;
//! * [`report_icon_rects`]: where each icon is on the screen now, for the host to answer `Shell_NotifyIconGetRect` with.
//!
//! THE REAL SOURCE is `tray_feed.rs`: it takes the running tray host's model (`rf_win::trayhost`, owned by `shell::trayhost` while the
//! setting `shell_tray_host` is on) and gives the strip what these five promise. With no host attached (the setting off, the host in
//! its log-only mode, or not yet started) there is no snapshot, and the island is exactly what it was. What each body promises:
//!
//! * `snapshot()` is cheap and does not block (it copies what the host's model holds; pictures are `Arc`s). Icons are in the host's order.
//!   `TrayIcon::picture` is already RGBA pixels (the host copies the program's `HICON` inside its message handler, where the sender is
//!   still blocked; the strip never sees a handle). `TrayIcon::generation` changes whenever the picture or the tip does, so the strip
//!   converts a picture again only then. `TrayIcon::key` is opaque to the strip, only handed back; `TrayIcon::id` is the identity the order
//!   file remembers the icon by (`StableId::of(guid, exe, uid)`), never made from the tip. Two icons with one identity (a program started
//!   twice with the same id) are one icon to the order file: they are placed and hidden together. An identity without a program path
//!   (`exe:|n`) is shared by every icon whose path could not be read and has the same id: the host should resolve the path (or give a
//!   GUID) and not leave it empty.
//! * `subscribe(f)` may call `f` from any thread, any number of times; `f` must be cheap (it posts to the app's thread). Dropping the
//!   [`Subscription`] stops the calls.
//! * `deliver_click` must not block (the host's click engine sends the program's messages with `SendNotifyMessage`). It returns whether
//!   the icon was still there AND still has the identity the click names (`TrayClick::id`). A click from the keyboard ([`ClickKind::KeySelect`], [`ClickKind::KeyContext`]) has the icon's top left
//!   corner as `at`; the engine makes `NIN_KEYSELECT` and `WM_CONTEXTMENU` of them for a program that asked for version 3 or 4.
//! * `report_icon_rects` is called with the current rectangles (device px, screen coordinates) of the icons that are in the strip, and for
//!   an icon in the overflow panel with the rectangle of its cell while the panel is open and of the chevron otherwise (what Explorer's
//!   own overflow does). It is called again only when something moved.
//!
//! `RUSTYFILES_TRAY_FAKE=1` makes all five work on six invented icons (coloured squares with invented tips) so that the strip, the overflow
//! panel, the tooltip, the reordering and the keyboard can be looked at with no tray host and no real program. Off unless set. Clicks on
//! the invented icons are only logged (counts, never anything of a user's), except that a left click on the first one counts a message
//! and so changes its tip and colour, which shows that an update from the host reaches the strip. `RUSTYFILES_TRAY_FAKE_FOCUS_MS=<n>`
//! (fake mode only) moves the keyboard to the strip `n` ms after the dock started, so that the keyboard can be tried before a key is
//! wired to do it.

use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use rf_win::dock::layout::Rect;
use rf_win::dock::tray_order::StableId;
use rf_win::dock::tray_strip::ClickKind;

/// The host's own name for an icon. Opaque here: only handed back to the host.
pub type IconKey = u64;

/// A program's picture: straight alpha, red first, `width * height * 4` bytes.
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bitmap({}x{})", self.width, self.height)
    }
}

impl Bitmap {
    /// Whether the bytes are a whole picture of the size it claims.
    pub fn is_whole(&self) -> bool {
        self.width > 0 && self.height > 0 && self.rgba.len() == self.width as usize * self.height as usize * 4
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayIcon {
    pub key: IconKey,
    /// What the order file remembers the icon by: its GUID, or its program and id. Never made from the tip.
    pub id: StableId,
    /// The program's tip, as given (the strip cleans it). May be empty.
    pub tip: String,
    pub picture: Option<Arc<Bitmap>>,
    /// Changes whenever the picture or the tip changes.
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TraySnapshot {
    /// Changes whenever anything in the snapshot does.
    pub generation: u64,
    pub icons: Vec<TrayIcon>,
}

/// One click for the host to deliver.
///
/// `id` is the identity of the icon the strip saw when the user pressed or pointed (the one its snapshot carried for the key). The host
/// MUST refuse the click (return false) when the icon it holds under `key` has another identity now: a program that exits while a button
/// is held, and another that registers and takes the same key (a table slot, a window handle and id), would otherwise be sent a click
/// the user meant for the first. Keys should not be reused for a different icon within one run of the host in the first place; this is
/// the second line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayClick {
    pub kind: ClickKind,
    pub id: StableId,
    /// The pointer, device px, screen coordinates (for a click from the keyboard: the icon's top left corner).
    pub at: (i32, i32),
}

/// Keeps a [`subscribe`] alive; dropping it stops the calls.
pub struct Subscription(Option<Box<dyn FnOnce() + Send>>);

impl Subscription {
    /// A subscription that runs `on_drop` when it is dropped (the source removes the callback there).
    pub fn new(on_drop: impl FnOnce() + Send + 'static) -> Subscription {
        Subscription(Some(Box::new(on_drop)))
    }
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f();
        }
    }
}

impl fmt::Debug for Subscription {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Subscription")
    }
}

// ---------------------------------------------------------------- the five

/// Whether this build has a tray host (it always has) or the made-up source is on. The host being off is [`snapshot`]'s `None`, not
/// this: the strip is made once at the dock's start and follows the host when the setting turns it on or off later.
pub fn available() -> bool {
    true
}

/// The icons now, or `None` when the tray host is off (or in log-only mode). Cheap: a copy of what `tray_feed`'s thread stored. The
/// made-up source wins when `RUSTYFILES_TRAY_FAKE=1`.
pub fn snapshot() -> Option<TraySnapshot> {
    if fake::on() { Some(fake::snapshot()) } else { super::tray_feed::snapshot() }
}

/// Calls `on_change` whenever [`snapshot`] may have changed. Works while the host is off: the calls start when it starts.
pub fn subscribe(on_change: impl Fn() + Send + Sync + 'static) -> Option<Subscription> {
    if fake::on() { Some(fake::subscribe(Arc::new(on_change))) } else { Some(super::tray_feed::subscribe(Arc::new(on_change))) }
}

/// Hands a click on an icon to the tray host. False: the icon is not there any more (or there is no host).
pub fn deliver_click(key: IconKey, click: TrayClick) -> bool {
    if fake::on() { fake::click(key, click) } else { super::tray_feed::deliver_click(key, click) }
}

/// Tells the tray host where the icons are on the screen now.
pub fn report_icon_rects(rects: &[(IconKey, Rect)]) {
    if fake::on() {
        fake::rects(rects.len());
    } else {
        super::tray_feed::report_icon_rects(rects);
    }
}

/// The milliseconds after which the fake source moves the keyboard to the strip (`None` when not asked, or not in fake mode).
pub fn fake_focus_after_ms() -> Option<u64> {
    if !fake::on() {
        return None;
    }
    std::env::var("RUSTYFILES_TRAY_FAKE_FOCUS_MS").ok().and_then(|v| v.trim().parse().ok())
}

// ---------------------------------------------------------------- the made-up source

mod fake {
    use super::*;

    pub fn on() -> bool {
        static ON: OnceLock<bool> = OnceLock::new();
        *ON.get_or_init(|| std::env::var("RUSTYFILES_TRAY_FAKE").is_ok_and(|v| v.trim() == "1"))
    }

    /// A 16 x 16 coloured square with rounded corners and a lighter top, so that two of them are told apart at a glance.
    pub fn square(rgb: [u8; 3]) -> Bitmap {
        const N: i32 = 16;
        let mut rgba = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                // the outer ring is empty and the corners are rounded off
                let (dx, dy) = (x.min(N - 1 - x), y.min(N - 1 - y));
                let inside = dx + dy >= 3 && x >= 1 && y >= 1 && x < N - 1 && y < N - 1;
                let lift = if y < N / 2 { 36u8 } else { 0 };
                let c = |v: u8| v.saturating_add(lift);
                if inside {
                    rgba.extend_from_slice(&[c(rgb[0]), c(rgb[1]), c(rgb[2]), 255]);
                } else {
                    rgba.extend_from_slice(&[0, 0, 0, 0]);
                }
            }
        }
        Bitmap { width: N as u32, height: N as u32, rgba }
    }

    /// The made-up icons. Three are known by a GUID and three by a program and an id, one has no tip (a program that gave none), one has
    /// a tip of two lines.
    pub struct FakeIcon {
        pub key: IconKey,
        pub id: StableId,
        pub tip: &'static str,
        pub rgb: [u8; 3],
    }

    pub fn icons() -> Vec<FakeIcon> {
        let guid = |n: u8| StableId::from_guid(&format!("{{fa4e0000-0000-4000-8000-0000000000{n:02x}}}")).expect("a valid guid");
        vec![
            FakeIcon { key: 101, id: guid(1), tip: "Posta", rgb: [0x2b, 0x7c, 0xd6] },
            FakeIcon { key: 102, id: StableId::from_program("C:\\Fake\\Bulut.exe", 1), tip: "Bulut eşitleme: güncel", rgb: [0x2f, 0x9e, 0x5b] },
            FakeIcon { key: 103, id: StableId::from_program("C:\\Fake\\Ses.exe", 2), tip: "Ses karıştırıcı", rgb: [0xe0, 0x8a, 0x1e] },
            FakeIcon { key: 104, id: guid(4), tip: "VPN: bağlı (Frankfurt)\nGönderilen: 1,2 MB", rgb: [0xc8, 0x3c, 0x3c] },
            FakeIcon { key: 105, id: StableId::from_program("C:\\Fake\\Guncelle.exe", 7), tip: "Güncelleme var", rgb: [0x8a, 0x4f, 0xc9] },
            FakeIcon { key: 106, id: guid(6), tip: "", rgb: [0x1c, 0x9c, 0xa8] },
        ]
    }

    /// The state of the made-up source: how many messages "Posta" counts. Pure, so that it is tested.
    #[derive(Default)]
    pub struct Model {
        pub messages: u32,
        pub generation: u64,
        pub clicks: u64,
    }

    impl Model {
        pub fn snapshot(&self) -> TraySnapshot {
            let icons = icons()
                .into_iter()
                .map(|f| {
                    let mail = f.key == 101;
                    let tip = if mail { format!("Posta - {} yeni ileti", 3 + self.messages) } else { f.tip.to_string() };
                    // each message makes the mail square a little lighter
                    let rgb = if mail { f.rgb.map(|c| c.saturating_add((self.messages * 24).min(120) as u8)) } else { f.rgb };
                    let generation = if mail { 1 + u64::from(self.messages) } else { 1 };
                    TrayIcon { key: f.key, id: f.id, tip, picture: Some(Arc::new(square(rgb))), generation }
                })
                .collect();
            TraySnapshot { generation: self.generation + 1, icons }
        }

        /// A click arrived. True when the snapshot changed (the subscribers are told).
        pub fn click(&mut self, key: IconKey, click: TrayClick) -> bool {
            // (the key and the identity the click names must still belong together)
            if !icons().iter().any(|f| f.key == key && f.id == click.id) {
                return false;
            }
            self.clicks += 1;
            if key == 101 && matches!(click.kind, ClickKind::Left | ClickKind::KeySelect) {
                self.messages += 1;
                self.generation += 1;
                return true;
            }
            false
        }
    }

    type Callback = Arc<dyn Fn() + Send + Sync>;

    #[derive(Default)]
    struct Shared {
        model: Model,
        subs: Vec<(u64, Callback)>,
        next: u64,
        rects: usize,
    }

    static SHARED: Mutex<Option<Shared>> = Mutex::new(None);

    fn with<R>(f: impl FnOnce(&mut Shared) -> R) -> R {
        let mut g = SHARED.lock().unwrap_or_else(|p| p.into_inner());
        f(g.get_or_insert_with(Shared::default))
    }

    pub fn snapshot() -> TraySnapshot {
        with(|s| s.model.snapshot())
    }

    pub fn subscribe(cb: Callback) -> Subscription {
        let id = with(|s| {
            s.next += 1;
            let id = s.next;
            s.subs.push((id, cb));
            id
        });
        Subscription(Some(Box::new(move || with(|s| s.subs.retain(|(i, _)| *i != id)))))
    }

    pub fn click(key: IconKey, click: TrayClick) -> bool {
        let (known, changed, callbacks, n) = with(|s| {
            let known = icons().iter().any(|f| f.key == key && f.id == click.id);
            let changed = s.model.click(key, click.clone());
            let cbs: Vec<Callback> = if changed { s.subs.iter().map(|(_, c)| c.clone()).collect() } else { Vec::new() };
            (known, changed, cbs, s.model.clicks)
        });
        log::info!("tray fake: click #{n} {:?} on icon {key} at {:?}{}", click.kind, click.at, if changed { " (its picture and tip changed)" } else { "" });
        // (called with the lock let go: a callback may ask for the snapshot)
        for c in callbacks {
            c();
        }
        known
    }

    pub fn rects(n: usize) {
        let changed = with(|s| std::mem::replace(&mut s.rects, n) != n);
        if changed {
            log::debug!("tray fake: {n} icon rectangles reported");
        }
    }
}

/// The made-up source's snapshot after `messages` messages of "Posta", for the strip's own tests.
#[cfg(test)]
pub fn test_snapshot(messages: u32) -> TraySnapshot {
    fake::Model { messages, ..Default::default() }.snapshot()
}

#[cfg(test)]
mod tests {
    use super::fake::{Model, icons, square};
    use super::*;

    #[test]
    fn six_icons_with_distinct_keys_and_identities() {
        let list = icons();
        assert_eq!(list.len(), 6);
        let mut keys: Vec<_> = list.iter().map(|f| f.key).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), 6);
        let mut ids: Vec<_> = list.iter().map(|f| f.id.clone()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 6, "no two icons share an identity");
        assert_eq!(list.iter().filter(|f| f.id.as_str().starts_with("guid:")).count(), 3);
        assert_eq!(list.iter().filter(|f| f.id.as_str().starts_with("exe:")).count(), 3);
        assert_eq!(list.iter().filter(|f| f.tip.is_empty()).count(), 1, "one icon without a tip");
        assert_eq!(list.iter().filter(|f| f.tip.contains('\n')).count(), 1, "one tip of two lines");
    }

    #[test]
    fn a_square_is_a_whole_sixteen_pixel_picture_with_cut_corners() {
        let b = square([200, 100, 50]);
        assert!(b.is_whole());
        assert_eq!((b.width, b.height), (16, 16));
        let alpha = |x: usize, y: usize| b.rgba[(y * 16 + x) * 4 + 3];
        assert_eq!(alpha(0, 0), 0, "the corner is cut");
        assert_eq!(alpha(8, 8), 255);
        assert_eq!(alpha(15, 15), 0);
        // the top half is lighter than the bottom half
        assert!(b.rgba[(2 * 16 + 8) * 4] > b.rgba[(12 * 16 + 8) * 4]);
    }

    #[test]
    fn a_bitmap_with_the_wrong_number_of_bytes_is_not_whole() {
        assert!(!Bitmap { width: 2, height: 2, rgba: vec![0; 15] }.is_whole());
        assert!(!Bitmap { width: 0, height: 2, rgba: vec![] }.is_whole());
        assert!(Bitmap { width: 2, height: 2, rgba: vec![0; 16] }.is_whole());
    }

    #[test]
    fn a_left_click_on_the_mail_icon_counts_a_message_and_changes_what_the_strip_would_convert() {
        let mut m = Model::default();
        let before = m.snapshot();
        let click = |kind| TrayClick { kind, at: (1, 2), id: icons()[0].id.clone() };
        assert!(m.click(101, click(ClickKind::Left)));
        let after = m.snapshot();
        assert_ne!(before.generation, after.generation);
        let (a, b) = (&before.icons[0], &after.icons[0]);
        assert_ne!(a.generation, b.generation, "the icon's generation moves with its picture and tip");
        assert_ne!(a.tip, b.tip);
        assert_ne!(a.picture, b.picture);
        assert_eq!(a.id, b.id, "the identity does not follow the tip");
        // the others did not change
        for (x, y) in before.icons.iter().zip(&after.icons).skip(1) {
            assert_eq!(x, y);
        }
        // other buttons and other icons change nothing, and an unknown icon is not there
        assert!(!m.click(101, click(ClickKind::Right)));
        assert!(!m.click(102, click(ClickKind::Left)), "the identity is the mail icon's, the key is the cloud icon's");
        assert!(!m.click(999, click(ClickKind::Left)));
        assert!(m.click(101, click(ClickKind::KeySelect)), "Enter on it counts as a left click");
    }

    #[test]
    fn a_click_for_a_key_that_now_belongs_to_another_icon_is_refused() {
        let mut m = Model::default();
        let list = icons();
        // pressed on the mail icon (key 101); that program went and another took the key: its identity is not the one the click names
        let stale = TrayClick { kind: ClickKind::Left, at: (0, 0), id: list[1].id.clone() };
        assert!(!m.click(101, stale));
        assert_eq!(m.clicks, 0, "nothing was delivered");
        assert!(m.click(101, TrayClick { kind: ClickKind::Left, at: (0, 0), id: list[0].id.clone() }));
        assert_eq!(m.clicks, 1);
    }

    #[test]
    fn a_subscription_is_called_until_it_is_dropped() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        // (the shared made-up source: only this test subscribes to it, and it clicks an icon no other test clicks)
        let n = Arc::new(AtomicUsize::new(0));
        let n2 = n.clone();
        let sub = fake::subscribe(Arc::new(move || {
            n2.fetch_add(1, Ordering::SeqCst);
        }));
        let mail = || TrayClick { kind: ClickKind::Left, at: (0, 0), id: icons()[0].id.clone() };
        assert!(fake::click(101, mail()));
        assert_eq!(n.load(Ordering::SeqCst), 1);
        drop(sub);
        assert!(fake::click(101, mail()));
        assert_eq!(n.load(Ordering::SeqCst), 1, "no call after the subscription is gone");
    }

    #[test]
    fn without_the_switch_and_with_no_host_there_are_no_icons() {
        // (the test run does not set RUSTYFILES_TRAY_FAKE, and no test attaches a host to the process's one feed)
        if std::env::var_os("RUSTYFILES_TRAY_FAKE").is_none() {
            assert!(available(), "the strip is made, and follows the host when it comes");
            assert!(snapshot().is_none(), "no host: the island is what it was");
            assert!(subscribe(|| {}).is_some(), "a subscription works while the host is off");
            assert!(!deliver_click(1, TrayClick { kind: ClickKind::Left, at: (0, 0), id: StableId::from_program("x", 1) }));
            assert_eq!(fake_focus_after_ms(), None);
        }
    }
}
