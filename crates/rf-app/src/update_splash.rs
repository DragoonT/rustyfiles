//! The window shown while an update is applied: the installer's 3-D logo animation (the scene of
//! tools/splash, as `splash.webp`: full colour, 2x, 30 frames a second) with a running bar,
//! instead of Velopack's plain "Installing update" box (its updater shows a splash image only
//! when installing, not when applying an update).
//!
//! `restart_to_update` copies this exe to a temporary folder (the installed one is being
//! replaced) and starts the copy with [`ARG`]; the update is applied silently. The window
//! closes when the new version's window is on the taskbar, or after two minutes at most.
//!
//! The frames are decoded one at a time on a helper thread, at their own pace: the window holds
//! one or two of them (2.4 MB each), never the whole animation.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::*;

pub const ARG: &str = "--update-splash";

const ANIMATION: &[u8] = include_bytes!("../assets/splash.webp");
const W: f32 = 520.;
const H: f32 = 300.;
/// The bar's colour, as Setup.exe's (`--splashProgressColor`).
const ORANGE: u32 = 0xff6a1a;
const GIVE_UP: Duration = Duration::from_secs(120);

struct Splash {
    frame: Option<Arc<RenderImage>>,
    /// Frames shown before: their textures are released at the next draw.
    old: Vec<Arc<RenderImage>>,
}

impl Render for Splash {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        for f in self.old.drain(..) {
            cx.drop_image(f, Some(window));
        }
        div()
            .size_full()
            .relative()
            .bg(rgb(0x0b0b0d))
            .children(self.frame.clone().map(|f| img(f).size_full().object_fit(ObjectFit::Cover)))
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

/// Plays the animation forever on this thread: each frame, decoded and turned BGRA (as GPUI
/// draws), goes to `send` when it is due. Stops when `send` says the window is gone.
fn play(send: impl Fn(Arc<RenderImage>) -> bool) {
    use image::AnimationDecoder as _;
    loop {
        let Ok(dec) = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(ANIMATION)) else { return };
        let mut due = Instant::now();
        let mut any = false;
        for f in dec.into_frames() {
            let Ok(f) = f else { break };
            any = true;
            let delay = Duration::from(f.delay()).max(Duration::from_millis(15));
            let mut buf = f.into_buffer();
            for px in buf.chunks_exact_mut(4) {
                px.swap(0, 2);
            }
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            }
            if !send(Arc::new(RenderImage::new(vec![image::Frame::new(buf)]))) {
                return;
            }
            due += delay;
        }
        if !any {
            return;
        }
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
        let view = match cx.open_window(options, |_, cx| cx.new(|_| Splash { frame: None, old: Vec::new() })) {
            Ok(w) => w,
            Err(e) => {
                log::warn!("update splash: {e:#}");
                cx.quit();
                return;
            }
        };
        // the frames, from the decoding thread
        let (tx, mut rx) = futures::channel::mpsc::channel::<Arc<RenderImage>>(2);
        std::thread::Builder::new()
            .name("rf-update-splash".into())
            .spawn(move || {
                let tx = std::sync::Mutex::new(tx);
                play(|f| {
                    let mut tx = tx.lock().unwrap_or_else(|e| e.into_inner());
                    // a full channel (the window is behind): this frame is skipped
                    match tx.try_send(f) {
                        Ok(()) => true,
                        Err(e) => !e.is_disconnected(),
                    }
                })
            })
            .ok();
        cx.spawn(async move |cx| {
            use futures::StreamExt;
            while let Some(f) = rx.next().await {
                let ok = view
                    .update(cx, |v, _, cx| {
                        if let Some(old) = v.frame.replace(f) {
                            v.old.push(old);
                        }
                        cx.notify();
                    })
                    .is_ok();
                if !ok {
                    break;
                }
            }
        })
        .detach();
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
    // The copy's working directory is the temporary folder, said here and not inherited: a copy that stood in the installed folder
    // kept the updater from renaming it, and the update gave up after ten tries (`rf_win::ownfolder` has the whole story; the
    // versions 0.7.0-beta.1 to 0.7.3 did exactly that whenever the program had been started from its shortcut).
    std::process::Command::new(&copy).arg(ARG).env("RUSTYFILES_MULTI_INSTANCE", "1").current_dir(std::env::temp_dir()).spawn().ok()
}

#[cfg(test)]
mod tests {
    /// The animation in the exe decodes: frames at 2x, a frame rate near 30 a second.
    #[test]
    fn animation_decodes() {
        use image::AnimationDecoder as _;
        let dec = image::codecs::webp::WebPDecoder::new(std::io::Cursor::new(super::ANIMATION)).unwrap();
        let frames: Vec<_> = dec.into_frames().collect::<Result<_, _>>().unwrap();
        assert!(frames.len() > 60, "{} frames", frames.len());
        let f = &frames[0];
        assert_eq!((f.buffer().width(), f.buffer().height()), (1040, 600));
        let ms = std::time::Duration::from(f.delay()).as_millis();
        assert!((25..=40).contains(&ms), "{ms} ms a frame");
    }
}
