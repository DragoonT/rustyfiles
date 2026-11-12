//! The user's wallpaper as plain data, read-only (shell plan F1, note 4 section 4).
//!
//! Two sources, tried in this order by `rf_win::desktop::wallpaper::current`:
//! 1. `IDesktopWallpaper` (documented, Windows 8 and later): a picture PER MONITOR, one position for the whole system, one background
//!    colour, the slideshow state. Whether it answers with no Explorer is UNVERIFIED (test T3), so
//! 2. the registry, which this file parses: `Control Panel\Desktop` (`WallPaper`, `WallpaperStyle`, `TileWallpaper`), `Control
//!    Panel\Colors\Background`, `Explorer\Wallpapers\BackgroundType`, and the per-virtual-desktop `Wallpaper` values.
//!
//! Also here: the change tracker (what to compare when "the wallpaper might have changed"), the cache key of a rendered bitmap, and the
//! test of a `WM_SETTINGCHANGE`.

use std::path::{Path, PathBuf};

use super::geom::{Rect, Rgb, Size};
use super::placement::{Position, TileAnchor};
use super::registry::{Hive, RegRead};

pub const KEY_DESKTOP: &str = r"Control Panel\Desktop";
pub const KEY_COLORS: &str = r"Control Panel\Colors";
pub const KEY_WALLPAPERS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\Wallpapers";
pub const KEY_VIRTUAL_DESKTOPS: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\VirtualDesktops";

/// Where the facts came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    DesktopWallpaperCom,
    Registry,
}

/// `Explorer\Wallpapers\BackgroundType`. 0 is a picture (seen on a real machine); 1 solid colour, 2 slideshow and 3 Spotlight are the
/// commonly reported numbers and UNVERIFIED.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackgroundKind {
    Picture,
    SolidColor,
    Slideshow,
    Spotlight,
    Unknown(u32),
}

impl BackgroundKind {
    pub fn from_registry(v: Option<u32>) -> Self {
        match v {
            None | Some(0) => BackgroundKind::Picture,
            Some(1) => BackgroundKind::SolidColor,
            Some(2) => BackgroundKind::Slideshow,
            Some(3) => BackgroundKind::Spotlight,
            Some(n) => BackgroundKind::Unknown(n),
        }
    }
}

/// `DESKTOP_SLIDESHOW_STATE` (1 enabled, 2 slideshow, 4 disabled by a remote session) and the options (1 = shuffle).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SlideshowInfo {
    pub enabled: bool,
    pub running: bool,
    pub disabled_by_remote_session: bool,
    pub shuffle: bool,
    pub tick_ms: u32,
}

impl SlideshowInfo {
    pub fn from_com(state: i32, options: i32, tick_ms: u32) -> Self {
        SlideshowInfo { enabled: state & 1 != 0, running: state & 2 != 0, disabled_by_remote_session: state & 4 != 0, shuffle: options & 1 != 0, tick_ms }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonitorWallpaper {
    /// `IDesktopWallpaper::GetMonitorDevicePathAt` (stable across sessions), or the device name when that was not available.
    pub device: String,
    /// Virtual-screen coordinates, physical pixels.
    pub rect: Rect,
    /// `None`: no picture on this monitor (a solid colour, or `GetWallpaper` gave an empty string).
    pub picture: Option<PathBuf>,
}

/// One virtual desktop's own wallpaper value (Windows 11, "different wallpaper per desktop").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopWallpaperValue {
    /// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}`
    pub desktop: String,
    pub current: bool,
    pub picture: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Wallpaper {
    pub source: Source,
    pub monitors: Vec<MonitorWallpaper>,
    /// One position for the whole system.
    pub position: Position,
    /// The registry held a style number this code does not know and `Fill` was assumed.
    pub position_assumed: bool,
    pub background: Rgb,
    pub kind: BackgroundKind,
    pub slideshow: Option<SlideshowInfo>,
    /// The per-virtual-desktop values, in the order Windows keeps the desktops (empty when there is only one desktop).
    pub per_desktop: Vec<DesktopWallpaperValue>,
}

impl Wallpaper {
    /// The picture each monitor shows, with a virtual desktop's own value in front when the source is the registry: the registry's
    /// `WallPaper` is Windows' transcoded copy of whatever was applied last, while the desktop's own value is the original picture the
    /// user chose for it. (The COM source already answers for the current desktop.)
    pub fn pictures(&self) -> Vec<Option<PathBuf>> {
        let own = (self.source == Source::Registry)
            .then(|| self.per_desktop.iter().find(|d| d.current).and_then(|d| d.picture.clone()))
            .flatten();
        self.monitors
            .iter()
            .map(|m| if self.kind == BackgroundKind::SolidColor { None } else { own.clone().or_else(|| m.picture.clone()) })
            .collect()
    }
}

/// The monitors the registry fallback paints: `(device, rect)`, the same picture on each.
pub fn read_registry(reg: &dyn RegRead, monitors: &[(String, Rect)]) -> Wallpaper {
    let cu = Hive::CurrentUser;
    let path = reg.string(cu, KEY_DESKTOP, "WallPaper").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from);
    let style = reg.string(cu, KEY_DESKTOP, "WallpaperStyle").unwrap_or_default();
    let tile = reg.string(cu, KEY_DESKTOP, "TileWallpaper").unwrap_or_default();
    let parsed = Position::from_registry(&style, &tile);
    let background = reg.string(cu, KEY_COLORS, "Background").and_then(|s| Rgb::parse_triple(&s)).unwrap_or_default();
    Wallpaper {
        source: Source::Registry,
        monitors: monitors.iter().map(|(d, r)| MonitorWallpaper { device: d.clone(), rect: *r, picture: path.clone() }).collect(),
        position: parsed.unwrap_or(Position::Fill),
        position_assumed: parsed.is_none(),
        background,
        kind: BackgroundKind::from_registry(reg.dword(cu, KEY_WALLPAPERS, "BackgroundType")),
        slideshow: None,
        per_desktop: virtual_desktops(reg),
    }
}

/// `{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}` of 16 registry bytes (a Windows GUID: the first three groups are little-endian).
pub fn guid_string(b: &[u8]) -> Option<String> {
    let b: &[u8; 16] = b.try_into().ok()?;
    let d1 = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let d2 = u16::from_le_bytes([b[4], b[5]]);
    let d3 = u16::from_le_bytes([b[6], b[7]]);
    Some(format!(
        "{{{d1:08x}-{d2:04x}-{d3:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}}}",
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    ))
}

/// Every virtual desktop with its `Wallpaper` value: `VirtualDesktopIDs` (16 bytes per desktop), `CurrentVirtualDesktop` (16 bytes),
/// and `VirtualDesktops\Desktops\{guid}\Wallpaper`. All of it is read-only; whether the current-desktop value follows a switch live
/// is UNVERIFIED (test T2), which is why a change of that key is one of the wallpaper change causes.
pub fn virtual_desktops(reg: &dyn RegRead) -> Vec<DesktopWallpaperValue> {
    let cu = Hive::CurrentUser;
    let Some(ids) = reg.binary(cu, KEY_VIRTUAL_DESKTOPS, "VirtualDesktopIDs") else { return Vec::new() };
    let current = reg.binary(cu, KEY_VIRTUAL_DESKTOPS, "CurrentVirtualDesktop").and_then(|b| guid_string(&b));
    ids.as_chunks::<16>()
        .0
        .iter()
        .filter_map(|c| guid_string(c))
        .map(|guid| {
            let key = format!(r"{KEY_VIRTUAL_DESKTOPS}\Desktops\{guid}");
            let picture = reg.string(cu, &key, "Wallpaper").map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from);
            DesktopWallpaperValue { current: current.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(&guid)), desktop: guid, picture }
        })
        .collect()
}

/// `HKCU\Control Panel\Desktop\WallPaper` points at `TranscodedWallpaper`, a copy with no extension (PNG for a PNG, JPEG for a JPEG).
/// Decoding must sniff the bytes ([`sniff_format`]) and never trust the name.
pub fn is_transcoded_copy(p: &Path) -> bool {
    p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase().starts_with("transcodedwallpaper"))
}

/// The file type from the first bytes, for the picture formats a wallpaper can be. `None` for anything else.
pub fn sniff_format(head: &[u8]) -> Option<&'static str> {
    if head.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpeg")
    } else if head.starts_with(b"BM") {
        Some("bmp")
    } else if head.len() >= 12 && &head[..4] == b"RIFF" && &head[8..12] == b"WEBP" {
        Some("webp")
    } else if head.starts_with(b"GIF8") {
        Some("gif")
    } else if head.starts_with(&[0x49, 0x49, 0xBC]) {
        Some("jxr")
    } else if head.starts_with(&[b'I', b'I', 0x2A, 0]) || head.starts_with(&[b'M', b'M', 0, 0x2A]) {
        Some("tiff")
    } else {
        None
    }
}

/// `WM_SETTINGCHANGE` carries `SPI_SETDESKWALLPAPER` (0x14) in `wParam` when the wallpaper was set through the system parameters call
/// (standard behaviour, UNVERIFIED on this build). A window that gets the message asks for a fresh read; it does not trust the
/// message to say what changed.
pub fn is_wallpaper_setting_change(wparam: usize) -> bool {
    wparam == 0x14
}

/// Why a fresh read was asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// `WM_SETTINGCHANGE` with `SPI_SETDESKWALLPAPER`.
    SettingChange,
    /// `RegNotifyChangeKeyValue` on one of the keys in [`WATCHED_KEYS`].
    RegistryKey(&'static str),
    /// A file in the Themes folder was written (the transcoded copy is rewritten in place).
    ThemesFolder,
    /// A start-up read or a manual refresh.
    Manual,
}

/// The keys whose change means the wallpaper may have changed. `RegNotifyChangeKeyValue` watches a key and (with the sub-tree flag)
/// everything under it.
pub const WATCHED_KEYS: [&str; 3] = [KEY_DESKTOP, KEY_WALLPAPERS, KEY_VIRTUAL_DESKTOPS];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileStamp {
    pub modified_ms: i64,
    pub size: u64,
}

pub trait Stamper {
    fn stamp(&self, path: &Path) -> Option<FileStamp>;
}

/// What differs between two reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Changed {
    pub first: bool,
    pub monitors: bool,
    pub pictures: bool,
    /// The same path with another size or modification time: the file was rewritten in place.
    pub rewritten: bool,
    pub position: bool,
    pub background: bool,
    pub kind: bool,
}

impl Changed {
    pub fn any(&self) -> bool {
        self.first || self.monitors || self.pictures || self.rewritten || self.position || self.background || self.kind
    }
}

/// Remembers the last read and says what is different in the next one. It never reads anything itself.
#[derive(Default)]
pub struct Tracker {
    last: Option<(Wallpaper, Vec<Option<FileStamp>>)>,
}

impl Tracker {
    pub fn update(&mut self, now: Wallpaper, stamper: &dyn Stamper) -> Option<Changed> {
        let stamps: Vec<Option<FileStamp>> = now.pictures().iter().map(|p| p.as_deref().and_then(|p| stamper.stamp(p))).collect();
        let changed = match &self.last {
            None => Changed { first: true, ..Default::default() },
            Some((old, old_stamps)) => Changed {
                first: false,
                monitors: old.monitors.iter().map(|m| (&m.device, m.rect)).ne(now.monitors.iter().map(|m| (&m.device, m.rect))),
                pictures: old.pictures() != now.pictures(),
                rewritten: old.pictures() == now.pictures() && *old_stamps != stamps,
                position: old.position != now.position,
                background: old.background != now.background,
                kind: old.kind != now.kind,
            },
        };
        let any = changed.any();
        self.last = Some((now, stamps));
        any.then_some(changed)
    }
}

/// Bumped whenever the placement maths or the scaling filter changes, so cached bitmaps made by the old code are not reused.
pub const RENDER_VERSION: u32 = 1;

/// Everything that decides how a rendered bitmap looks. Two equal keys mean the same pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheKey {
    pub path: String,
    pub modified_ms: i64,
    pub size: u64,
    pub target: Size,
    pub position: Position,
    pub background: Rgb,
    pub anchor: TileAnchor,
    /// Where this monitor sits in the virtual screen and how big that is: part of the key only for span (and tile anchored at the
    /// virtual screen), where the monitor's slice of the picture depends on it. Moving monitors around does not invalidate the others.
    pub placement: Option<(Rect, Rect)>,
}

impl CacheKey {
    pub fn new(
        path: &Path,
        stamp: FileStamp,
        monitor: Rect,
        virtual_screen: Rect,
        position: Position,
        background: Rgb,
        anchor: TileAnchor,
    ) -> Self {
        let uses_arrangement = position == Position::Span || (position == Position::Tile && anchor == TileAnchor::VirtualScreen);
        CacheKey {
            path: normalise_path(path),
            modified_ms: stamp.modified_ms,
            size: stamp.size,
            target: monitor.size(),
            position,
            background,
            anchor: if position == Position::Tile { anchor } else { TileAnchor::default() },
            placement: uses_arrangement.then_some((monitor, virtual_screen)),
        }
    }

    /// A line that holds every field; the disk cache hashes it for the file name.
    pub fn canonical(&self) -> String {
        let place = match self.placement {
            Some((m, v)) => format!("{},{},{},{}|{},{},{},{}", m.left, m.top, m.right, m.bottom, v.left, v.top, v.right, v.bottom),
            None => "-".into(),
        };
        format!(
            "rfwall{RENDER_VERSION}|{}|{}|{}|{}x{}|{}|{},{},{}|{:?}|{place}",
            self.path,
            self.modified_ms,
            self.size,
            self.target.w,
            self.target.h,
            self.position.to_dwpos(),
            self.background.0,
            self.background.1,
            self.background.2,
            self.anchor,
        )
    }
}

/// Windows paths compare without case and with either slash; a `\\?\` prefix is not part of the name.
pub fn normalise_path(p: &Path) -> String {
    let s = p.to_string_lossy().replace('/', "\\");
    s.strip_prefix(r"\\?\").unwrap_or(&s).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::desktop::registry::MemRegistry;

    fn mons() -> Vec<(String, Rect)> {
        vec![("A".into(), Rect::new(0, 0, 1920, 1080)), ("B".into(), Rect::new(-1440, -560, 0, 2000))]
    }

    fn base() -> MemRegistry {
        MemRegistry::new()
            .cu_string(KEY_DESKTOP, "WallPaper", r"C:\Users\x\AppData\Roaming\Microsoft\Windows\Themes\TranscodedWallpaper")
            .cu_string(KEY_DESKTOP, "WallpaperStyle", "10")
            .cu_string(KEY_DESKTOP, "TileWallpaper", "0")
            .cu_string(KEY_COLORS, "Background", "195 0 82")
            .cu_dword(KEY_WALLPAPERS, "BackgroundType", 0)
    }

    #[test]
    fn registry_fallback_reads_the_observed_values() {
        let w = read_registry(&base(), &mons());
        assert_eq!(w.source, Source::Registry);
        assert_eq!(w.position, Position::Fill);
        assert!(!w.position_assumed);
        assert_eq!(w.background, Rgb(195, 0, 82));
        assert_eq!(w.kind, BackgroundKind::Picture);
        assert_eq!(w.monitors.len(), 2);
        assert!(w.monitors.iter().all(|m| m.picture.as_deref().is_some_and(is_transcoded_copy)));
        assert!(w.per_desktop.is_empty());
    }

    #[test]
    fn registry_fallback_empty_and_odd_cases() {
        // nothing at all: a black desktop with Fill assumed
        let w = read_registry(&MemRegistry::new(), &mons());
        assert!(w.monitors.iter().all(|m| m.picture.is_none()));
        assert_eq!(w.background, Rgb(0, 0, 0));
        assert!(w.position_assumed && w.position == Position::Fill);
        // an empty path string is no picture
        let r = MemRegistry::new().cu_string(KEY_DESKTOP, "WallPaper", "  ");
        assert!(read_registry(&r, &mons()).monitors[0].picture.is_none());
        // tile wins over the style number
        let r = base().cu_string(KEY_DESKTOP, "WallpaperStyle", "0").cu_string(KEY_DESKTOP, "TileWallpaper", "1");
        assert_eq!(read_registry(&r, &mons()).position, Position::Tile);
        // a style this code does not know is assumed Fill, and says so
        let r = base().cu_string(KEY_DESKTOP, "WallpaperStyle", "99");
        let w = read_registry(&r, &mons());
        assert!(w.position_assumed && w.position == Position::Fill);
        // a broken colour is black, not a crash
        let r = base().cu_string(KEY_COLORS, "Background", "red");
        assert_eq!(read_registry(&r, &mons()).background, Rgb(0, 0, 0));
        // solid colour means no picture is drawn even though the path is still there
        let r = base().cu_dword(KEY_WALLPAPERS, "BackgroundType", 1);
        let w = read_registry(&r, &mons());
        assert_eq!(w.kind, BackgroundKind::SolidColor);
        assert!(w.pictures().iter().all(|p| p.is_none()));
        assert_eq!(BackgroundKind::from_registry(Some(7)), BackgroundKind::Unknown(7));
        assert_eq!(BackgroundKind::from_registry(Some(3)), BackgroundKind::Spotlight);
        assert_eq!(BackgroundKind::from_registry(Some(2)), BackgroundKind::Slideshow);
    }

    fn guid_bytes(d1: u32, d2: u16, d3: u16, d4: [u8; 8]) -> Vec<u8> {
        let mut v = d1.to_le_bytes().to_vec();
        v.extend(d2.to_le_bytes());
        v.extend(d3.to_le_bytes());
        v.extend(d4);
        v
    }

    #[test]
    fn guid_strings_use_windows_byte_order() {
        let b = guid_bytes(0x1122_3344, 0x5566, 0x7788, [0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF, 0x00]);
        assert_eq!(guid_string(&b).as_deref(), Some("{11223344-5566-7788-99aa-bbccddeeff00}"));
        assert_eq!(guid_string(&b[..15]), None);
    }

    #[test]
    fn per_virtual_desktop_values() {
        let a = guid_bytes(0xAAAA_0001, 1, 2, [3; 8]);
        let b = guid_bytes(0xBBBB_0002, 4, 5, [6; 8]);
        let (ga, gb) = (guid_string(&a).unwrap(), guid_string(&b).unwrap());
        let both: Vec<u8> = a.iter().chain(b.iter()).copied().collect();
        let r = base()
            .cu_binary(KEY_VIRTUAL_DESKTOPS, "VirtualDesktopIDs", &both)
            .cu_binary(KEY_VIRTUAL_DESKTOPS, "CurrentVirtualDesktop", &b)
            .cu_string(&format!(r"{KEY_VIRTUAL_DESKTOPS}\Desktops\{ga}"), "Wallpaper", r"D:\one.jpg")
            .cu_string(&format!(r"{KEY_VIRTUAL_DESKTOPS}\Desktops\{}", gb.to_uppercase()), "Wallpaper", r"D:\two.png");
        let w = read_registry(&r, &mons());
        assert_eq!(w.per_desktop.len(), 2);
        assert_eq!(w.per_desktop[0].desktop, ga);
        assert!(!w.per_desktop[0].current && w.per_desktop[1].current);
        assert_eq!(w.per_desktop[1].picture.as_deref(), Some(Path::new(r"D:\two.png")));
        // the registry source prefers the current desktop's own (original) picture over the transcoded copy
        assert!(w.pictures().iter().all(|p| p.as_deref() == Some(Path::new(r"D:\two.png"))));
        // ... the COM source does not (it already answers for the current desktop)
        let mut c = w.clone();
        c.source = Source::DesktopWallpaperCom;
        assert!(c.pictures().iter().all(|p| p.as_deref().is_some_and(is_transcoded_copy)));
        // a desktop without a value falls back to the registry's WallPaper
        let r2 = base().cu_binary(KEY_VIRTUAL_DESKTOPS, "VirtualDesktopIDs", &a).cu_binary(KEY_VIRTUAL_DESKTOPS, "CurrentVirtualDesktop", &a);
        let w2 = read_registry(&r2, &mons());
        assert!(w2.per_desktop[0].current && w2.per_desktop[0].picture.is_none());
        assert!(w2.pictures()[0].as_deref().is_some_and(is_transcoded_copy));
    }

    #[test]
    fn slideshow_flags() {
        let s = SlideshowInfo::from_com(3, 1, 600_000);
        assert!(s.enabled && s.running && s.shuffle && !s.disabled_by_remote_session);
        assert!(SlideshowInfo::from_com(4, 0, 0).disabled_by_remote_session);
        assert_eq!(SlideshowInfo::from_com(0, 0, 0), SlideshowInfo::default());
    }

    #[test]
    fn file_types_are_sniffed_from_bytes() {
        assert_eq!(sniff_format(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10]), Some("png"));
        assert_eq!(sniff_format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpeg"));
        assert_eq!(sniff_format(b"BM\x00\x00"), Some("bmp"));
        assert_eq!(sniff_format(b"RIFF\x10\0\0\0WEBPVP8 "), Some("webp"));
        assert_eq!(sniff_format(b"RIFF\x10\0\0\0WAVEfmt "), None);
        assert_eq!(sniff_format(b"GIF89a"), Some("gif"));
        assert_eq!(sniff_format(&[0x49, 0x49, 0xBC, 0x01]), Some("jxr"));
        assert_eq!(sniff_format(b"%PDF-1.7"), None);
        assert_eq!(sniff_format(&[]), None);
        assert!(is_transcoded_copy(Path::new(r"C:\a\Themes\TranscodedWallpaper")));
        assert!(is_transcoded_copy(Path::new(r"C:\a\Themes\TranscodedWallpaperCache\x\transcodedwallpaper_1f")));
        assert!(!is_transcoded_copy(Path::new(r"C:\a\photo.jpg")));
    }

    #[test]
    fn setting_change_message() {
        assert!(is_wallpaper_setting_change(0x14));
        assert!(!is_wallpaper_setting_change(0x2F)); // some other SPI value
        assert!(!is_wallpaper_setting_change(0));
    }

    struct FakeStamps(std::cell::RefCell<Vec<(PathBuf, FileStamp)>>);
    impl Stamper for FakeStamps {
        fn stamp(&self, p: &Path) -> Option<FileStamp> {
            self.0.borrow().iter().find(|(q, _)| q == p).map(|(_, s)| *s)
        }
    }

    #[test]
    fn the_tracker_tells_what_changed() {
        let stamps = FakeStamps(std::cell::RefCell::new(vec![(PathBuf::from(r"D:\a.png"), FileStamp { modified_ms: 10, size: 5 })]));
        let mk = |pic: &str, pos: Position, bg: Rgb| {
            let mut w = read_registry(&MemRegistry::new(), &mons());
            for m in &mut w.monitors {
                m.picture = Some(PathBuf::from(pic));
            }
            w.position = pos;
            w.background = bg;
            w
        };
        let mut t = Tracker::default();
        assert!(t.update(mk(r"D:\a.png", Position::Fill, Rgb(0, 0, 0)), &stamps).unwrap().first);
        // the same again: nothing
        assert_eq!(t.update(mk(r"D:\a.png", Position::Fill, Rgb(0, 0, 0)), &stamps), None);
        // position only
        let c = t.update(mk(r"D:\a.png", Position::Fit, Rgb(0, 0, 0)), &stamps).unwrap();
        assert!(c.position && !c.pictures && !c.rewritten && !c.background);
        // background only
        let c = t.update(mk(r"D:\a.png", Position::Fit, Rgb(1, 2, 3)), &stamps).unwrap();
        assert!(c.background && !c.position);
        // the same path, the file rewritten in place (the transcoded copy)
        stamps.0.borrow_mut()[0].1 = FileStamp { modified_ms: 99, size: 5 };
        let c = t.update(mk(r"D:\a.png", Position::Fit, Rgb(1, 2, 3)), &stamps).unwrap();
        assert!(c.rewritten && !c.pictures);
        assert_eq!(t.update(mk(r"D:\a.png", Position::Fit, Rgb(1, 2, 3)), &stamps), None);
        // another picture
        let c = t.update(mk(r"D:\b.png", Position::Fit, Rgb(1, 2, 3)), &stamps).unwrap();
        assert!(c.pictures && !c.rewritten);
        // a monitor arrives
        let mut w = mk(r"D:\b.png", Position::Fit, Rgb(1, 2, 3));
        w.monitors.push(MonitorWallpaper { device: "C".into(), rect: Rect::new(1920, 0, 3840, 1080), picture: Some(PathBuf::from(r"D:\b.png")) });
        assert!(t.update(w, &stamps).unwrap().monitors);
    }

    #[allow(clippy::too_many_arguments)]
    fn key(path: &str, mtime: i64, size: u64, mon: Rect, v: Rect, pos: Position, bg: Rgb, anchor: TileAnchor) -> CacheKey {
        CacheKey::new(Path::new(path), FileStamp { modified_ms: mtime, size }, mon, v, pos, bg, anchor)
    }

    #[test]
    fn the_cache_key_changes_with_what_changes_the_pixels() {
        let m = Rect::new(0, 0, 1920, 1080);
        let v = Rect::new(-1440, -560, 1920, 2000);
        let base = key(r"D:\a.png", 10, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen);
        let same = |k: &CacheKey| k.canonical() == base.canonical();
        assert!(same(&key(r"d:/A.PNG", 10, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "case and slash do not matter");
        assert!(same(&key(r"\\?\D:\a.png", 10, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "nor does the long-path prefix");
        // moving the monitors around does not matter for fill
        assert!(same(&key(r"D:\a.png", 10, 5, m, Rect::new(0, 0, 1920, 1080), Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)));
        assert!(same(&key(r"D:\a.png", 10, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::Monitor)), "anchor is only for tile");
        // but all of these do
        assert!(!same(&key(r"D:\b.png", 10, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "path");
        assert!(!same(&key(r"D:\a.png", 11, 5, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "modified time (rewritten in place)");
        assert!(!same(&key(r"D:\a.png", 10, 6, m, v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "size");
        assert!(!same(&key(r"D:\a.png", 10, 5, Rect::new(0, 0, 2560, 1440), v, Position::Fill, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "target size");
        assert!(!same(&key(r"D:\a.png", 10, 5, m, v, Position::Fit, Rgb(0, 0, 0), TileAnchor::VirtualScreen)), "mode");
        assert!(!same(&key(r"D:\a.png", 10, 5, m, v, Position::Fill, Rgb(0, 0, 1), TileAnchor::VirtualScreen)), "bars are baked in");
        // span depends on where the monitor is
        let s1 = key(r"D:\a.png", 10, 5, m, v, Position::Span, Rgb(0, 0, 0), TileAnchor::VirtualScreen);
        let s2 = key(r"D:\a.png", 10, 5, m, Rect::new(0, 0, 3840, 1080), Position::Span, Rgb(0, 0, 0), TileAnchor::VirtualScreen);
        assert_ne!(s1.canonical(), s2.canonical());
        // tile depends on the arrangement only when anchored at the virtual screen
        let t1 = key(r"D:\a.png", 10, 5, m, v, Position::Tile, Rgb(0, 0, 0), TileAnchor::VirtualScreen);
        let t2 = key(r"D:\a.png", 10, 5, m, Rect::new(0, 0, 3840, 1080), Position::Tile, Rgb(0, 0, 0), TileAnchor::VirtualScreen);
        let t3 = key(r"D:\a.png", 10, 5, m, v, Position::Tile, Rgb(0, 0, 0), TileAnchor::Monitor);
        let t4 = key(r"D:\a.png", 10, 5, m, Rect::new(0, 0, 3840, 1080), Position::Tile, Rgb(0, 0, 0), TileAnchor::Monitor);
        assert_ne!(t1.canonical(), t2.canonical());
        assert_eq!(t3.canonical(), t4.canonical());
        assert!(base.canonical().starts_with("rfwall1|d:\\a.png|10|5|1920x1080|4|"));
    }
}
