//! Live pictures of the windows on the switcher's cards, through DWM (`DwmRegisterThumbnail`).
//!
//! THE RULES OF DWM THUMBNAILS (docs; the live behaviour with a GPUI window is checked in the first live run):
//! * the DESTINATION must be a top-level window of this process (here: the switcher's own window) and the source a top-level window;
//! * DWM draws the picture on top of everything in the destination's client area, so nothing of ours can be painted over it and it
//!   cannot be clipped to a rounded corner: the cards leave a box for it and put their text around the box;
//! * a thumbnail belongs to the process that registered it and must be unregistered by it: [`ThumbSet`] does so for every picture
//!   it made when it is cleared or dropped, which is when the switcher closes. Nothing is registered while it is closed.
//! * a MINIMIZED window is registered like any other: DWM keeps the last picture of a window that was minimized, which is what
//!   Windows' own taskbar previews and Alt+Tab show. The first version gave every minimized window the app's icon instead, and the
//!   user, hovering a minimized browser in the dock, got a card with only an icon. Only when DWM has no usable picture of it (it
//!   reports a source smaller than [`MIN_SOURCE`] a side: a window that never drew, or only its minimized stub) does the card show
//!   the icon, and that registration is given back at once. A window DWM refuses (an elevated window for a normal process, one that
//!   was just destroyed) gets the icon too.
//!
//! The DWM calls are behind [`ThumbApi`] so that the order of calls, the fitting and the cleanup are tested with a fake.

use super::layout::fit_picture;
use crate::dock::layout::Rect;

/// The calls into DWM.
pub trait ThumbApi {
    /// Registers `source` to be drawn into `dest`; the handle of the thumbnail.
    fn register(&mut self, dest: isize, source: isize) -> anyhow::Result<isize>;
    /// The source window's size, device px.
    fn source_size(&mut self, thumb: isize) -> Option<(i32, i32)>;
    /// Draws the thumbnail into `rect` (the destination window's client coordinates).
    fn place(&mut self, thumb: isize, rect: Rect) -> anyhow::Result<()>;
    fn unregister(&mut self, thumb: isize);
    /// The source window's outer size, device px, read without DWM (the fallback when a size cannot be had from a registration).
    fn window_size(&mut self, _source: isize) -> Option<(i32, i32)> {
        None
    }
}

/// The size of a source that is worth fitting a picture to: a known size, and for a minimized window not only its stub (see
/// [`MIN_SOURCE`]). `None` means "not known", which the layout treats as 16:10 until it is.
pub fn usable_size(size: Option<(i32, i32)>, minimized: bool) -> Option<(i32, i32)> {
    let (w, h) = size?;
    if w <= 0 || h <= 0 || (minimized && (w < MIN_SOURCE || h < MIN_SOURCE)) {
        return None;
    }
    Some((w, h))
}

/// The size DWM would draw `source` at, found BEFORE the preview's window is sized (the card's height follows the window's shape, so
/// the shape must be known first): the source is registered to `dest` for a moment, asked, and given back. Falls back to the window's
/// own rectangle when DWM will not register it (a minimized window has no such fallback: its rectangle is the stub). Nothing stays
/// registered and nothing is drawn.
pub fn probe_size<A: ThumbApi>(api: &mut A, dest: isize, source: isize, minimized: bool) -> Option<(i32, i32)> {
    if source == 0 {
        return None;
    }
    let via_dwm = api.register(dest, source).ok().and_then(|t| {
        let size = api.source_size(t);
        api.unregister(t);
        size
    });
    usable_size(via_dwm.or_else(|| if minimized { None } else { api.window_size(source) }), minimized)
}

/// What a card shows in its picture box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picture {
    /// DWM draws the window live, here (panel client coordinates).
    Live(Rect),
    /// No live picture: the card shows the app's icon large.
    Icon,
}

/// The pictures of one switcher's cards.
pub struct ThumbSet<A: ThumbApi> {
    api: A,
    dest: isize,
    /// Added to every destination rectangle before DWM gets it (see [`ThumbSet::with_origin`]).
    origin: (i32, i32),
    registered: Vec<isize>,
}

/// The smallest source, a side, that counts as a picture of a minimized window (its stub on a desktop without a taskbar is about
/// 160 x 28; a window that never drew reports nothing).
pub const MIN_SOURCE: i32 = 64;

impl<A: ThumbApi> ThumbSet<A> {
    pub fn new(api: A, dest: isize) -> ThumbSet<A> {
        ThumbSet { api, dest, origin: (0, 0), registered: Vec::new() }
    }

    /// A set whose destination rectangles are moved by `origin` (device px) on their way to DWM, for a destination window whose client
    /// area does not start at its own top left corner (the dock's pop-ups keep the system's invisible resize frame: 8 px left, right
    /// and below at 100 %, and no caption). The pictures in [`Picture::Live`] stay in CLIENT coordinates, the ones the view draws its
    /// boxes in; only the call to DWM is shifted. Seen with a real mouse: every live picture stood exactly the frame's left inset
    /// (8 px) left of its box, whatever its proportions, with no vertical error (the top inset is 0), which is DWM taking the
    /// destination from the window's own origin and not from the client area's.
    pub fn with_origin(mut self, origin: (i32, i32)) -> ThumbSet<A> {
        self.origin = origin;
        self
    }

    /// Registers a picture for each `(source window, minimized, picture box)` and returns what each card shows, in the same
    /// order. A minimized window shows the last picture DWM has of it, or its icon when DWM has none. A failure on one card is
    /// that card's icon, not an error.
    pub fn show(&mut self, cards: &[(isize, bool, Rect)]) -> Vec<Picture> {
        cards
            .iter()
            .map(|&(source, minimized, boxr)| {
                if source == 0 {
                    return Picture::Icon;
                }
                let thumb = match self.api.register(self.dest, source) {
                    Ok(t) => t,
                    Err(e) => {
                        log::debug!("switcher: no picture of {source:#x}: {e:#}");
                        return Picture::Icon;
                    }
                };
                let size = self.api.source_size(thumb).unwrap_or((0, 0));
                if minimized && (size.0 < MIN_SOURCE || size.1 < MIN_SOURCE) {
                    // (nothing worth showing: the registration is given back now, not kept invisible until the cards go)
                    self.api.unregister(thumb);
                    return Picture::Icon;
                }
                self.registered.push(thumb);
                let at = fit_picture(size, boxr);
                let sent = Rect::new(at.left + self.origin.0, at.top + self.origin.1, at.right + self.origin.0, at.bottom + self.origin.1);
                match self.api.place(thumb, sent) {
                    Ok(()) => Picture::Live(at),
                    Err(e) => {
                        log::debug!("switcher: cannot place the picture of {source:#x}: {e:#}");
                        // (it stays registered, invisible, until clear: DWM draws nothing for a thumbnail that was never made visible)
                        Picture::Icon
                    }
                }
            })
            .collect()
    }

    /// How many pictures are registered now.
    pub fn count(&self) -> usize {
        self.registered.len()
    }

    /// Unregisters every picture. The switcher calls it when it closes; dropping the set does the same.
    pub fn clear(&mut self) {
        for t in self.registered.drain(..) {
            self.api.unregister(t);
        }
    }
}

impl<A: ThumbApi> Drop for ThumbSet<A> {
    fn drop(&mut self) {
        self.clear();
    }
}

/// The real DWM calls.
pub struct Dwm;

impl ThumbApi for Dwm {
    fn register(&mut self, dest: isize, source: isize) -> anyhow::Result<isize> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::Graphics::Dwm::DwmRegisterThumbnail;
        Ok(unsafe { DwmRegisterThumbnail(HWND(dest as *mut _), HWND(source as *mut _))? })
    }

    fn source_size(&mut self, thumb: isize) -> Option<(i32, i32)> {
        use windows::Win32::Graphics::Dwm::DwmQueryThumbnailSourceSize;
        unsafe { DwmQueryThumbnailSourceSize(thumb) }.ok().map(|s| (s.cx, s.cy))
    }

    fn place(&mut self, thumb: isize, r: Rect) -> anyhow::Result<()> {
        use windows::Win32::Foundation::RECT;
        use windows::Win32::Graphics::Dwm::{DWM_THUMBNAIL_PROPERTIES, DwmUpdateThumbnailProperties};
        // RECTDESTINATION 0x1 | OPACITY 0x4 | VISIBLE 0x8 (the whole window, not only its client area: SOURCECLIENTAREAONLY stays off)
        let props = DWM_THUMBNAIL_PROPERTIES {
            dwFlags: 0x1 | 0x4 | 0x8,
            rcDestination: RECT { left: r.left, top: r.top, right: r.right, bottom: r.bottom },
            opacity: 255,
            fVisible: true.into(),
            ..Default::default()
        };
        unsafe { DwmUpdateThumbnailProperties(thumb, &props)? };
        Ok(())
    }

    fn unregister(&mut self, thumb: isize) {
        use windows::Win32::Graphics::Dwm::DwmUnregisterThumbnail;
        unsafe {
            let _ = DwmUnregisterThumbnail(thumb);
        }
    }

    fn window_size(&mut self, source: isize) -> Option<(i32, i32)> {
        use windows::Win32::Foundation::{HWND, RECT};
        use windows::Win32::UI::WindowsAndMessaging::GetWindowRect;
        let mut r = RECT::default();
        unsafe { GetWindowRect(HWND(source as *mut _), &mut r) }.ok()?;
        Some((r.right - r.left, r.bottom - r.top))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        next: isize,
        live: Vec<isize>,
    }

    struct Fake {
        log: Rc<RefCell<Log>>,
        refuse: Vec<isize>,
        size: (i32, i32),
        /// The size DWM reports for these source windows (the others: `size`).
        sizes: Vec<(isize, (i32, i32))>,
        /// Which source each thumbnail was registered for.
        of: Vec<(isize, isize)>,
        fail_place: bool,
        /// What `window_size` answers.
        window: Option<(i32, i32)>,
        /// The rectangles DWM was given.
        placed: Rc<RefCell<Vec<Rect>>>,
    }

    impl Fake {
        fn new(log: &Rc<RefCell<Log>>) -> Fake {
            Fake {
                log: log.clone(),
                refuse: vec![],
                size: (1920, 1080),
                sizes: vec![],
                of: vec![],
                fail_place: false,
                window: None,
                placed: Rc::new(RefCell::new(Vec::new())),
            }
        }
    }

    impl ThumbApi for Fake {
        fn register(&mut self, dest: isize, source: isize) -> anyhow::Result<isize> {
            let mut l = self.log.borrow_mut();
            l.calls.push(format!("register {dest:#x} {source:#x}"));
            if self.refuse.contains(&source) {
                anyhow::bail!("access denied");
            }
            l.next += 1;
            let t = l.next;
            l.live.push(t);
            self.of.push((t, source));
            Ok(t)
        }
        fn source_size(&mut self, thumb: isize) -> Option<(i32, i32)> {
            self.log.borrow_mut().calls.push(format!("size {thumb}"));
            let source = self.of.iter().find(|(t, _)| *t == thumb).map(|(_, s)| *s);
            Some(self.sizes.iter().find(|(s, _)| Some(*s) == source).map_or(self.size, |(_, z)| *z))
        }
        fn place(&mut self, thumb: isize, r: Rect) -> anyhow::Result<()> {
            self.log.borrow_mut().calls.push(format!("place {thumb} {}x{}", r.width(), r.height()));
            self.placed.borrow_mut().push(r);
            if self.fail_place {
                anyhow::bail!("bad rect");
            }
            Ok(())
        }
        fn unregister(&mut self, thumb: isize) {
            let mut l = self.log.borrow_mut();
            l.calls.push(format!("unregister {thumb}"));
            l.live.retain(|t| *t != thumb);
        }
        fn window_size(&mut self, _source: isize) -> Option<(i32, i32)> {
            self.window
        }
    }

    #[test]
    fn an_origin_moves_what_dwm_is_given_and_not_what_the_card_keeps() {
        let log = Rc::new(RefCell::new(Log::default()));
        let api = Fake::new(&log);
        let placed = api.placed.clone();
        // the dock's pop-up: its client area starts 8 px right of the window's own left edge
        let mut set = ThumbSet::new(api, 1).with_origin((8, 0));
        let pics = set.show(&[(0x10, false, BOX)]);
        let Picture::Live(kept) = pics[0] else { panic!("live") };
        assert!(BOX.covers(&kept), "the card keeps the client rectangle {kept:?}");
        let sent = placed.borrow()[0];
        assert_eq!((sent.left, sent.top), (kept.left + 8, kept.top), "DWM gets it moved by the origin");
        assert_eq!((sent.width(), sent.height()), (kept.width(), kept.height()), "moved, not resized");
        // no origin: DWM gets exactly the client rectangle
        let api = Fake::new(&log);
        let placed = api.placed.clone();
        let mut set = ThumbSet::new(api, 1);
        let pics = set.show(&[(0x10, false, BOX)]);
        assert_eq!(Picture::Live(placed.borrow()[0]), pics[0]);
    }

    #[test]
    fn a_size_is_found_before_any_picture_is_drawn_and_nothing_stays_registered() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        api.sizes = vec![(0x10, (1000, 400))];
        assert_eq!(probe_size(&mut api, 7, 0x10, false), Some((1000, 400)));
        assert_eq!(log.borrow().calls, vec!["register 0x7 0x10", "size 1", "unregister 1"]);
        assert!(log.borrow().live.is_empty());
        assert!(api.placed.borrow().is_empty(), "nothing was placed");
    }

    #[test]
    fn a_window_dwm_will_not_register_is_sized_by_its_rectangle_unless_it_is_minimized() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        api.refuse = vec![0x10];
        api.window = Some((800, 600));
        assert_eq!(probe_size(&mut api, 7, 0x10, false), Some((800, 600)));
        assert_eq!(probe_size(&mut api, 7, 0x10, true), None, "a minimized window's rectangle is its stub");
        assert_eq!(probe_size(&mut api, 7, 0, false), None, "no window");
    }

    #[test]
    fn only_a_size_worth_a_picture_counts() {
        assert_eq!(usable_size(Some((1920, 1080)), false), Some((1920, 1080)));
        assert_eq!(usable_size(None, false), None);
        assert_eq!(usable_size(Some((0, 0)), false), None);
        assert_eq!(usable_size(Some((-3, 20)), false), None);
        assert_eq!(usable_size(Some((160, 28)), true), None, "a minimized stub");
        assert_eq!(usable_size(Some((160, 28)), false), Some((160, 28)), "a small tool window is a picture");
        assert_eq!(usable_size(Some((800, 600)), true), Some((800, 600)), "the last picture DWM keeps");
    }

    const BOX: Rect = Rect::new(10, 10, 214, 134);

    #[test]
    fn each_live_card_is_registered_sized_and_placed_in_that_order() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut set = ThumbSet::new(Fake::new(&log), 0xd);
        let pics = set.show(&[(0x10, false, BOX), (0x20, false, BOX)]);
        assert!(matches!(pics[0], Picture::Live(r) if r.width() == 204 && BOX.covers(&r)));
        assert_eq!(
            log.borrow().calls,
            vec!["register 0xd 0x10", "size 1", "place 1 204x115", "register 0xd 0x20", "size 2", "place 2 204x115"]
        );
        assert_eq!(set.count(), 2);
    }

    #[test]
    fn a_minimized_window_shows_the_last_picture_dwm_has_of_it() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut set = ThumbSet::new(Fake::new(&log), 1);
        let pics = set.show(&[(0x10, true, BOX), (0x20, false, BOX)]);
        assert!(matches!(pics[0], Picture::Live(_)), "what the taskbar's own preview shows: {:?}", pics[0]);
        assert!(matches!(pics[1], Picture::Live(_)));
        assert_eq!(set.count(), 2);
    }

    #[test]
    fn a_minimized_window_dwm_has_no_picture_of_shows_its_icon_and_keeps_no_registration() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        // only the minimized stub, and a window that never drew
        api.sizes = vec![(0x10, (160, 28)), (0x30, (0, 0))];
        let mut set = ThumbSet::new(api, 1);
        let pics = set.show(&[(0x10, true, BOX), (0x20, true, BOX), (0x30, true, BOX)]);
        assert_eq!(pics[0], Picture::Icon);
        assert!(matches!(pics[1], Picture::Live(_)));
        assert_eq!(pics[2], Picture::Icon);
        assert_eq!(set.count(), 1, "the two without a picture were given back at once");
        assert_eq!(log.borrow().calls.iter().filter(|c| c.starts_with("unregister")).count(), 2);
        // a window that is NOT minimized is shown whatever its size (a small tool window is a picture too)
        let log2 = Rc::new(RefCell::new(Log::default()));
        let mut api2 = Fake::new(&log2);
        api2.sizes = vec![(0x10, (40, 40))];
        let mut set2 = ThumbSet::new(api2, 1);
        assert!(matches!(set2.show(&[(0x10, false, BOX)])[0], Picture::Live(_)));
    }

    #[test]
    fn a_window_dwm_refuses_shows_its_icon_and_the_others_still_work() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        api.refuse = vec![0x10];
        let mut set = ThumbSet::new(api, 1);
        let pics = set.show(&[(0x10, false, BOX), (0x20, false, BOX)]);
        assert_eq!(pics[0], Picture::Icon);
        assert!(matches!(pics[1], Picture::Live(_)));
        assert_eq!(set.count(), 1);
    }

    #[test]
    fn a_placement_that_fails_shows_the_icon_but_the_thumbnail_is_still_cleaned_up() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        api.fail_place = true;
        let mut set = ThumbSet::new(api, 1);
        let pics = set.show(&[(0x10, false, BOX)]);
        assert_eq!(pics[0], Picture::Icon);
        drop(set);
        assert!(log.borrow().live.is_empty(), "nothing is left registered");
    }

    #[test]
    fn closing_unregisters_every_picture_it_made() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut set = ThumbSet::new(Fake::new(&log), 1);
        set.show(&[(0x10, false, BOX), (0x20, false, BOX), (0x30, false, BOX)]);
        assert_eq!(log.borrow().live.len(), 3);
        set.clear();
        assert!(log.borrow().live.is_empty());
        assert_eq!(set.count(), 0);
        // and clearing again does nothing
        let before = log.borrow().calls.len();
        set.clear();
        assert_eq!(log.borrow().calls.len(), before);
    }

    #[test]
    fn dropping_the_set_unregisters_too() {
        let log = Rc::new(RefCell::new(Log::default()));
        {
            let mut set = ThumbSet::new(Fake::new(&log), 1);
            set.show(&[(0x10, false, BOX), (0x20, false, BOX)]);
            assert_eq!(log.borrow().live.len(), 2);
        }
        assert!(log.borrow().live.is_empty());
    }

    #[test]
    fn nothing_is_registered_until_asked() {
        let log = Rc::new(RefCell::new(Log::default()));
        let set = ThumbSet::new(Fake::new(&log), 1);
        assert_eq!(set.count(), 0);
        assert!(log.borrow().calls.is_empty());
    }

    #[test]
    fn a_source_with_no_known_size_fills_the_box() {
        let log = Rc::new(RefCell::new(Log::default()));
        let mut api = Fake::new(&log);
        api.size = (0, 0);
        let mut set = ThumbSet::new(api, 1);
        let pics = set.show(&[(0x10, false, BOX)]);
        assert_eq!(pics[0], Picture::Live(BOX));
    }
}
