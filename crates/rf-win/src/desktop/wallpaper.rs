//! Reading the user's wallpaper, read-only (shell plan F1). `IDesktopWallpaper` first, the registry when it does not answer.
//!
//! WHY TWO. The COM object gives a picture per monitor, one position, the background colour and the slideshow state, but its class has
//! no server path in the registry on the machine this was written on, so whether it answers when Explorer is not running is
//! UNVERIFIED (shell plan test T3). The registry fallback (rf-core `read_registry`) has the global picture, style and colour, which is
//! enough to draw the same wallpaper on every monitor. Windows 11's per-virtual-desktop values are read from the registry in both cases.
//!
//! THREADS. COM runs on the shell thread through `sta::call` with a limit; a hung or missing server costs `SHELL_LIMIT` at most and the
//! registry answer is used. Nothing here calls a method that writes: no `SetWallpaper`, `SetPosition`, `SetBackgroundColor`,
//! `SetSlideshow*`, `AdvanceSlideshow`, `Enable`.

use std::path::{Path, PathBuf};

use anyhow::Context;
use rf_core::desktop::geom::{Rect, Rgb};
use rf_core::desktop::placement::Position;
use rf_core::desktop::registry::RegRead;
use rf_core::desktop::wallpaper::{
    BackgroundKind, FileStamp, KEY_WALLPAPERS, MonitorWallpaper, SlideshowInfo, Source, Stamper, Wallpaper, read_registry, virtual_desktops,
};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance, CoTaskMemFree};
use windows::Win32::UI::Shell::{DesktopWallpaper, IDesktopWallpaper};
use windows::core::{PCWSTR, PWSTR};

/// The read half of `IDesktopWallpaper`, so the conversion to data can be tested with a fake.
pub trait DesktopWallpaperApi {
    fn monitor_count(&self) -> anyhow::Result<u32>;
    /// `GetMonitorDevicePathAt`
    fn monitor_path(&self, index: u32) -> anyhow::Result<String>;
    fn monitor_rect(&self, path: &str) -> anyhow::Result<Rect>;
    /// `GetWallpaper`: the empty string for no picture (S_FALSE, or a monitor showing a colour).
    fn wallpaper(&self, path: &str) -> anyhow::Result<String>;
    /// `GetPosition`: `DESKTOP_WALLPAPER_POSITION` as a number.
    fn position(&self) -> anyhow::Result<i32>;
    /// `GetBackgroundColor`: a COLORREF.
    fn background(&self) -> anyhow::Result<u32>;
    /// `GetStatus`, `GetSlideshowOptions`: (state, options, tick in ms).
    fn slideshow(&self) -> anyhow::Result<(i32, i32, u32)>;
}

/// Builds the data from the API. `registry` supplies what the COM object does not know (the kind of background, the per-desktop
/// values) and the position when `GetPosition` fails.
pub fn read_from_api(api: &dyn DesktopWallpaperApi, registry: &dyn RegRead) -> anyhow::Result<Wallpaper> {
    let n = api.monitor_count().context("GetMonitorDevicePathCount")?;
    let mut monitors = Vec::new();
    for i in 0..n {
        let path = api.monitor_path(i).with_context(|| format!("GetMonitorDevicePathAt({i})"))?;
        let rect = api.monitor_rect(&path).with_context(|| format!("GetMonitorRECT({i})"))?;
        // a monitor whose picture cannot be read is shown as having none rather than failing the whole read
        let picture = api.wallpaper(&path).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).map(PathBuf::from);
        monitors.push(MonitorWallpaper { device: path, rect, picture });
    }
    let fallback = read_registry(registry, &[]);
    let (position, position_assumed) = match api.position().ok().and_then(Position::from_dwpos) {
        Some(p) => (p, false),
        None => (fallback.position, fallback.position_assumed),
    };
    let slideshow = api.slideshow().ok().map(|(state, options, tick)| SlideshowInfo::from_com(state, options, tick));
    let mut kind = BackgroundKind::from_registry(registry.dword(rf_core::desktop::registry::Hive::CurrentUser, KEY_WALLPAPERS, "BackgroundType"));
    if kind == BackgroundKind::Picture && slideshow.is_some_and(|s| s.running) {
        kind = BackgroundKind::Slideshow;
    }
    Ok(Wallpaper {
        source: Source::DesktopWallpaperCom,
        monitors,
        position,
        position_assumed,
        background: api.background().map(Rgb::from_colorref).unwrap_or(fallback.background),
        kind,
        slideshow,
        per_desktop: virtual_desktops(registry),
    })
}

/// The real object. Created and used on the shell thread only (`sta::call`).
struct ComApi(IDesktopWallpaper);

fn take_string(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

impl DesktopWallpaperApi for ComApi {
    fn monitor_count(&self) -> anyhow::Result<u32> {
        Ok(unsafe { self.0.GetMonitorDevicePathCount()? })
    }
    fn monitor_path(&self, index: u32) -> anyhow::Result<String> {
        Ok(take_string(unsafe { self.0.GetMonitorDevicePathAt(index)? }))
    }
    fn monitor_rect(&self, path: &str) -> anyhow::Result<Rect> {
        let w = crate::wide::wide(path);
        let r = unsafe { self.0.GetMonitorRECT(PCWSTR(w.as_ptr()))? };
        Ok(Rect::new(r.left, r.top, r.right, r.bottom))
    }
    fn wallpaper(&self, path: &str) -> anyhow::Result<String> {
        let w = crate::wide::wide(path);
        Ok(take_string(unsafe { self.0.GetWallpaper(PCWSTR(w.as_ptr()))? }))
    }
    fn position(&self) -> anyhow::Result<i32> {
        Ok(unsafe { self.0.GetPosition()? }.0)
    }
    fn background(&self) -> anyhow::Result<u32> {
        Ok(unsafe { self.0.GetBackgroundColor()? }.0)
    }
    fn slideshow(&self) -> anyhow::Result<(i32, i32, u32)> {
        let state = unsafe { self.0.GetStatus()? }.0;
        let (mut options, mut tick) = (Default::default(), 0u32);
        unsafe { self.0.GetSlideshowOptions(&mut options, &mut tick)? };
        Ok((state, options.0, tick))
    }
}

/// `IDesktopWallpaper`, read on the shell thread. `Err` when the object cannot be created or the shell thread does not answer. It
/// goes through the COM-activation gate: the class has no server path in the registry here, so with no Explorer the activation may
/// never return (shell plan test T3), and a second read must not queue up behind the first on the one shell thread.
pub fn read_com(registry: std::sync::Arc<dyn RegRead + Send + Sync>) -> anyhow::Result<Wallpaper> {
    super::COM_ACTIVATION
        .call(super::SHELL_LIMIT, move || -> anyhow::Result<Wallpaper> {
            let obj: IDesktopWallpaper = unsafe { CoCreateInstance(&DesktopWallpaper, None, CLSCTX_ALL) }.context("CoCreateInstance(DesktopWallpaper)")?;
            read_from_api(&ComApi(obj), registry.as_ref())
        })
        .map_err(|e| match e {
            super::GateError::Busy => anyhow::anyhow!("an earlier COM activation has not come back yet"),
            super::GateError::TimedOut => anyhow::anyhow!("the shell thread did not answer in time"),
        })?
}

/// The wallpaper: COM when it answers, the registry otherwise. `monitors` are `(device key, rect)` of the connected monitors, used by the
/// registry fallback (see [`super::monitors`]).
pub fn current(registry: std::sync::Arc<dyn RegRead + Send + Sync>, monitors: &[(String, Rect)]) -> Wallpaper {
    match read_com(registry.clone()) {
        Ok(w) if !w.monitors.is_empty() => w,
        Ok(_) => read_registry(registry.as_ref(), monitors),
        Err(e) => {
            log::debug!("IDesktopWallpaper did not answer ({e:#}); reading the registry");
            read_registry(registry.as_ref(), monitors)
        }
    }
}

/// [`current`] for a chosen route (`rf_core::desktop::cover::bare::route_for`): with no shell desktop window the registry alone is read,
/// without waiting for a COM activation that may not come back (see [`read_com`]).
pub fn current_via(route: rf_core::desktop::cover::bare::Route, registry: std::sync::Arc<dyn RegRead + Send + Sync>, monitors: &[(String, Rect)]) -> Wallpaper {
    use rf_core::desktop::cover::bare::Route;
    match route {
        Route::ComThenRegistry => current(registry, monitors),
        Route::RegistryOnly => read_registry(registry.as_ref(), monitors),
    }
}

/// File size and modification time from the disk.
pub struct DiskStamper;

impl Stamper for DiskStamper {
    fn stamp(&self, path: &Path) -> Option<FileStamp> {
        let md = std::fs::metadata(path).ok()?;
        let ms = md.modified().ok()?.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0);
        Some(FileStamp { modified_ms: ms, size: md.len() })
    }
}

/// The picture's file type from its first bytes (`"png"`, `"jpeg"` ...), never from the name: the transcoded copy has none.
pub fn file_type(path: &Path) -> Option<&'static str> {
    use std::io::Read;
    let mut head = [0u8; 16];
    let n = std::fs::File::open(path).ok()?.read(&mut head).ok()?;
    rf_core::desktop::wallpaper::sniff_format(&head[..n])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_core::desktop::registry::MemRegistry;

    struct Fake {
        pics: Vec<(&'static str, Rect, Result<&'static str, ()>)>,
        position: Result<i32, ()>,
        color: Result<u32, ()>,
        slideshow: Result<(i32, i32, u32), ()>,
    }
    impl DesktopWallpaperApi for Fake {
        fn monitor_count(&self) -> anyhow::Result<u32> {
            Ok(self.pics.len() as u32)
        }
        fn monitor_path(&self, i: u32) -> anyhow::Result<String> {
            Ok(self.pics[i as usize].0.to_string())
        }
        fn monitor_rect(&self, p: &str) -> anyhow::Result<Rect> {
            Ok(self.pics.iter().find(|x| x.0 == p).unwrap().1)
        }
        fn wallpaper(&self, p: &str) -> anyhow::Result<String> {
            self.pics.iter().find(|x| x.0 == p).unwrap().2.map(|s| s.to_string()).map_err(|_| anyhow::anyhow!("E_FAIL"))
        }
        fn position(&self) -> anyhow::Result<i32> {
            self.position.map_err(|_| anyhow::anyhow!("E_POINTER"))
        }
        fn background(&self) -> anyhow::Result<u32> {
            self.color.map_err(|_| anyhow::anyhow!("E_FAIL"))
        }
        fn slideshow(&self) -> anyhow::Result<(i32, i32, u32)> {
            self.slideshow.map_err(|_| anyhow::anyhow!("E_FAIL"))
        }
    }

    fn reg() -> MemRegistry {
        MemRegistry::new().cu_string(rf_core::desktop::wallpaper::KEY_DESKTOP, "WallpaperStyle", "6").cu_string(rf_core::desktop::wallpaper::KEY_DESKTOP, "TileWallpaper", "0")
    }

    #[test]
    fn per_monitor_pictures_and_the_empty_string_of_s_false() {
        let api = Fake {
            pics: vec![("\\\\?\\M1", Rect::new(0, 0, 1920, 1080), Ok(r"D:\a.png")), ("\\\\?\\M2", Rect::new(-1440, -560, 0, 2000), Ok("")), ("\\\\?\\M3", Rect::new(1920, 0, 3840, 1080), Err(()))],
            position: Ok(4),
            color: Ok(0x0052_00C3),
            slideshow: Ok((1, 0, 0)),
        };
        let w = read_from_api(&api, &reg()).unwrap();
        assert_eq!(w.source, Source::DesktopWallpaperCom);
        assert_eq!(w.monitors.len(), 3);
        assert_eq!(w.monitors[0].picture.as_deref(), Some(Path::new(r"D:\a.png")));
        assert_eq!(w.monitors[1].picture, None, "S_FALSE / empty string: no picture on this monitor");
        assert_eq!(w.monitors[2].picture, None, "an error for one monitor does not fail the read");
        assert_eq!(w.monitors[1].rect, Rect::new(-1440, -560, 0, 2000));
        assert_eq!(w.position, Position::Fill, "the COM value wins over the registry's style 6 (fit)");
        assert!(!w.position_assumed);
        assert_eq!(w.background, Rgb(195, 0, 82));
        assert_eq!(w.kind, BackgroundKind::Picture);
        assert!(w.slideshow.is_some_and(|s| s.enabled && !s.running));
    }

    #[test]
    fn failures_fall_back_to_the_registry_values() {
        let api = Fake { pics: vec![("M", Rect::new(0, 0, 10, 10), Ok(r"D:\a.png"))], position: Err(()), color: Err(()), slideshow: Err(()) };
        let w = read_from_api(&api, &reg().cu_string(rf_core::desktop::wallpaper::KEY_COLORS, "Background", "1 2 3")).unwrap();
        assert_eq!(w.position, Position::Fit, "GetPosition failed: the registry's 6/0");
        assert_eq!(w.background, Rgb(1, 2, 3));
        assert!(w.slideshow.is_none());
        // an unknown position number from COM is not trusted either
        let api = Fake { pics: vec![("M", Rect::new(0, 0, 10, 10), Ok(""))], position: Ok(77), color: Ok(0), slideshow: Ok((3, 1, 60_000)) };
        let w = read_from_api(&api, &reg()).unwrap();
        assert_eq!(w.position, Position::Fit);
        assert_eq!(w.kind, BackgroundKind::Slideshow, "a running slideshow is reported as such");
        assert!(w.slideshow.unwrap().shuffle);
    }

    #[test]
    fn a_dead_object_is_an_error_not_an_empty_wallpaper() {
        struct Dead;
        impl DesktopWallpaperApi for Dead {
            fn monitor_count(&self) -> anyhow::Result<u32> {
                anyhow::bail!("RPC_E_SERVER_UNAVAILABLE")
            }
            fn monitor_path(&self, _: u32) -> anyhow::Result<String> {
                unreachable!()
            }
            fn monitor_rect(&self, _: &str) -> anyhow::Result<Rect> {
                unreachable!()
            }
            fn wallpaper(&self, _: &str) -> anyhow::Result<String> {
                unreachable!()
            }
            fn position(&self) -> anyhow::Result<i32> {
                unreachable!()
            }
            fn background(&self) -> anyhow::Result<u32> {
                unreachable!()
            }
            fn slideshow(&self) -> anyhow::Result<(i32, i32, u32)> {
                unreachable!()
            }
        }
        assert!(read_from_api(&Dead, &reg()).is_err());
    }

    #[test]
    fn file_types_from_bytes_in_a_temp_folder() {
        let dir = std::env::temp_dir().join(format!("rf-wp-type-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("TranscodedWallpaper"); // no extension, like Windows' copy
        std::fs::write(&png, [0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0]).unwrap();
        let jpg = dir.join("x.dat");
        std::fs::write(&jpg, [0xFF, 0xD8, 0xFF, 0xE0, 0, 0]).unwrap();
        let txt = dir.join("t.png"); // a lying extension
        std::fs::write(&txt, b"hello").unwrap();
        assert_eq!(file_type(&png), Some("png"));
        assert_eq!(file_type(&jpg), Some("jpeg"));
        assert_eq!(file_type(&txt), None);
        assert_eq!(file_type(&dir.join("missing")), None);
        let st = DiskStamper.stamp(&png).unwrap();
        assert_eq!(st.size, 10);
        assert!(DiskStamper.stamp(&dir.join("missing")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
