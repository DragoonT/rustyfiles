//! The window shown while an update is applied: the installer's animated logo (the same GIF
//! Setup.exe shows) with a running bar, instead of Velopack's plain "Installing update" box
//! (its updater shows the splash image only when installing, not when applying an update).
//!
//! `restart_to_update` copies this exe to a temporary folder (the installed one is being
//! replaced) and starts the copy with [`ARG`]; the update is applied silently. The window
//! closes when the new version's window is on the taskbar, or after two minutes at most.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

pub const ARG: &str = "--update-splash";

const W: f32 = 520.;
const H: f32 = 300.;
/// The bar's colour, as Setup.exe's (`--splashProgressColor`).
const ORANGE: u32 = 0xff6a1a;
const GIVE_UP: Duration = Duration::from_secs(120);

struct Splash {
    /// The GIF's frames (BGRA, as GPUI draws them) and how long each shows.
    frames: Vec<(Arc<RenderImage>, Duration)>,
    start: Instant,
}

/// The splash GIF's frames (GPUI shows only the first frame of a GIF given as bytes).
fn frames() -> Vec<(Arc<RenderImage>, Duration)> {
    use image::AnimationDecoder as _;
    let Ok(dec) = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(&include_bytes!("../assets/splash.gif")[..])) else { return Vec::new() };
    dec.into_frames()
        .filter_map(Result::ok)
        .map(|f| {
            let delay = Duration::from(f.delay()).max(Duration::from_millis(20));
            let mut buf = f.into_buffer();
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            (Arc::new(RenderImage::new(vec![image::Frame::new(buf)])), delay)
        })
        .collect()
}

impl Render for Splash {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        // the frame for now, looping
        let total: Duration = self.frames.iter().map(|f| f.1).sum();
        let frame = (!total.is_zero()).then(|| {
            let mut t = Duration::from_nanos((self.start.elapsed().as_nanos() % total.as_nanos()) as u64);
            self.frames.iter().find(|f| {
                if t < f.1 {
                    return true;
                }
                t -= f.1;
                false
            })
        });
        window.request_animation_frame();
        div()
            .size_full()
            .relative()
            .bg(rgb(0x0b0b0d))
            .children(frame.flatten().map(|f| img(f.0.clone()).size_full().object_fit(ObjectFit::Cover)))
            // how far is not known: a stretch of orange runs along the bottom
            .child(
                div().absolute().left_0().right_0().bottom_0().h(px(4.)).bg(hsla(0., 0., 1., 0.08)).child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .w(px(W * 0.3))
                        .bg(rgb(ORANGE))
                        .with_animation("run", Animation::new(Duration::from_millis(1400)).repeat(), |d, t| d.left(px(-W * 0.3 + t * W * 1.3))),
                ),
            )
    }
}

/// The installed rustyfiles.exe (the one the update replaces and restarts).
fn installed_exe() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("rustyfiles").join("current").join("rustyfiles.exe"))
}

/// Taskbar windows of the installed rustyfiles, by process.
fn installed_windows() -> Vec<u32> {
    let Some(exe) = installed_exe() else { return Vec::new() };
    let exe = exe.to_string_lossy().to_lowercase();
    rf_win::tasklist::tasks()
        .into_iter()
        .filter(|t| t.exe.as_ref().is_some_and(|e| e.to_string_lossy().to_lowercase() == exe))
        .map(|t| t.pid)
        .collect()
}

/// Shows the window until the updated rustyfiles has one of its own.
pub fn run() {
    let before = installed_windows();
    let started = Instant::now();
    gpui_kit::application().run(move |cx| {
        let frames = frames();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(W), px(H)), cx))),
            titlebar: None,
            focus: false,
            show: true,
            kind: WindowKind::PopUp,
            is_movable: false,
            is_resizable: false,
            is_minimizable: false,
            ..Default::default()
        };
        if let Err(e) = cx.open_window(options, |_, cx| cx.new(|_| Splash { frames, start: Instant::now() })) {
            log::warn!("update splash: {e:#}");
            cx.quit();
            return;
        }
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let before = before.clone();
                let done = cx
                    .background_executor()
                    .spawn(async move { installed_windows().into_iter().any(|pid| !before.contains(&pid)) })
                    .await;
                if done || started.elapsed() > GIVE_UP {
                    // a moment for the new window to draw before this one goes
                    cx.background_executor().timer(Duration::from_millis(400)).await;
                    let _ = cx.update(|cx| cx.quit());
                    break;
                }
            }
        })
        .detach();
    });
}

/// Starts a copy of this exe showing the window (the installed exe is about to be replaced).
/// None when it could not be started: the update then shows Velopack's own progress box.
pub fn launch() -> Option<std::process::Child> {
    let me = std::env::current_exe().ok()?;
    let dir = std::env::temp_dir().join("rustyfiles-update");
    let _ = std::fs::create_dir_all(&dir);
    let copy = dir.join("rustyfiles-update.exe");
    std::fs::copy(&me, &copy).ok()?;
    std::process::Command::new(&copy).arg(ARG).env("RUSTYFILES_MULTI_INSTANCE", "1").spawn().ok()
}
