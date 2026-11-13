//! Hızlı Bakış penceresi: dinamik başlık çubuğu (görsel içerikte içeriğin üstünde durur, fare
//! durunca gizlenir), eylem düğmeleri, tam ekran, sabitleme.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, TitleBar, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::geometry::{self, Rect};
use super::viewer::{Prepared, Probe, Viewer, pick};
use crate::look::config::LookSettings;
use crate::t;
use crate::theme::{self, Palette};

/// The bar that lies over the content (a picture, a video) is shown only while the pointer is in the window's top strip, where the
/// bar is (the user: "sadece üst bar hoverda görünsün"). While it is shown, the pointer's place is looked at this often ...
const BAR_WATCH: Duration = Duration::from_millis(120);
/// ... and the bar goes when this many looks in a row found the pointer away: about a third of a second, so a pointer that slips
/// under the bar and comes back does not make it blink.
const BAR_AWAY_LOOKS: u8 = 3;

/// How many looks in a row found the pointer away from the bar's strip.
#[derive(Default)]
struct BarAway(u8);

impl BarAway {
    /// One look. True when the bar is to go now. The bar's own menu keeps it: the pointer is in the menu then, under the strip.
    fn hide_now(&mut self, over: bool, menu_open: bool) -> bool {
        if over || menu_open {
            self.0 = 0;
            return false;
        }
        self.0 = self.0.saturating_add(1);
        self.0 >= BAR_AWAY_LOOKS
    }
}

pub(crate) fn hwnd_of(window: &Window) -> isize {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
        Ok(RawWindowHandle::Win32(h)) => h.hwnd.get(),
        _ => 0,
    }
}

pub struct LookWindow {
    pub(crate) path: PathBuf,
    pub(crate) title: SharedString,
    pub(crate) pinned: bool,
    pub(crate) topmost: bool,
    /// Başlık çubuğu içeriğin üstünde durur ve kendiliğinden gizlenir (resim, video).
    pub(crate) overlay_titlebar: bool,
    /// The bar over the content is on the screen (only for `overlay_titlebar`; the other bar is always there).
    chrome_shown: bool,
    more_open: bool,
    pub(crate) busy: bool,
    /// Gövdeden sürükleme dosyayı dışarı sürükler (kendi sürüklemesi olmayan içerikte).
    pub(crate) drag_by_body: bool,
    drag_from: Option<Point<Pixels>>,
    pub(crate) hwnd: isize,
    /// Her yüklemede artar: içerik geçiş animasyonu yeniden başlar.
    load_gen: u64,
    /// Otomatik yeniden yükleme: dosyanın klasörü izlenir.
    _watch: Option<rf_win::watch::DirWatch>,
    _watch_task: Option<Task<()>>,
    focus: FocusHandle,
    _hide: Option<Task<()>>,
    /// The file being opened off the UI thread (a slow place, see `load_in_background`).
    _loading: Option<Task<()>>,
    _subs: Vec<Subscription>,
    viewer: Box<dyn Viewer>,
    /// Pencerenin bizim verdiğimiz son boyutu (pencerenin bildirdiği haliyle).
    programmatic: (f32, f32),
    /// Kullanıcı pencereyi elle boyutladıysa o boyut (bu pencere açık kaldıkça korunur).
    user_size: Option<(f32, f32)>,
    /// Az önce biz boyutladık: bu süre içindeki boyut değişikliği bizimdir, kullanıcının değil.
    resizing_until: Option<std::time::Instant>,
}

/// Dosyanın klasöründe değişiklik oldu: yeniden yüklensin mi? Silinme ya da geri gelme her zaman
/// (Bilgi kartına "dosya yok" ile geçilir); içerik değişikliği yalnız otomatik yeniden yükleme açıksa.
fn should_reload(auto_reload: bool, existed: bool, exists: bool, changed: bool) -> bool {
    existed != exists || (auto_reload && exists && changed)
}

/// The same off the UI thread, for a file on a network drive or another slow place: the file is
/// looked at and opened by the background executor (seconds there for a big video's index or a
/// PDF), the viewer picked here and prepared by the caller.
pub(crate) async fn load_viewer(path: PathBuf, cx: &mut AsyncApp) -> (Box<dyn Viewer>, Probe) {
    let probe = cx.background_executor().spawn(async move { Probe::read(&path) }).await;
    let mut viewer = pick(&probe);
    log::debug!("look: {} → {} (opened off the UI thread)", probe.path.display(), viewer.name());
    if let Some(load) = viewer.loader() {
        let p = probe.clone();
        let loaded = cx.background_executor().spawn(async move { load(&p) }).await;
        viewer.take_loaded(loaded);
    }
    (viewer, probe)
}

/// What the window shows while a file in a slow place opens.
pub(crate) struct Opening;

impl Opening {
    pub(crate) fn prepared(path: &std::path::Path) -> Prepared {
        let mut p = Prepared::new((520.0, 220.0), file_title(path).to_string());
        p.drag_by_body = false;
        p
    }
}

impl Viewer for Opening {
    fn name(&self) -> &'static str {
        "opening"
    }
    fn priority(&self) -> i32 {
        i32::MIN
    }
    fn can_handle(&self, _f: &Probe) -> bool {
        false
    }
    fn prepare(&mut self, f: &Probe, _cx: &mut App) -> Prepared {
        Opening::prepared(&f.path)
    }
    fn render(&mut self, window: &mut Window, _cx: &mut App) -> AnyElement {
        let p = Palette::for_window(window);
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_2()
            .child(Icon::new(L::LoaderCircle).size(px(20.)).text_color(p.text_secondary))
            .child(div().text_sm().text_color(p.text_secondary).child(t!("Açılıyor…")))
            .into_any_element()
    }
}

/// Dosyaya bakar, görüntüleyiciyi seçer ve hazırlar.
pub(crate) fn prepare_viewer(path: &std::path::Path, cx: &mut App) -> (Box<dyn Viewer>, Prepared) {
    let probe = Probe::read(path);
    let mut viewer = pick(&probe);
    log::debug!("look: {} → {}", path.display(), viewer.name());
    let prepared = viewer.prepare(&probe, cx);
    (viewer, prepared)
}

impl LookWindow {
    pub(crate) fn new(
        path: PathBuf,
        topmost: bool,
        viewer: Box<dyn Viewer>,
        prepared: Prepared,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut subs = vec![cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                // tıklanınca ya da öne gelince tuşlar (Esc, Boşluk, F11…) bu pencereye gelsin
                window.focus(&this.focus, cx);
                return;
            }
            if this.pinned {
                return;
            }
            if cx.global::<LookSettings>().look_close_on_lost_focus {
                window.remove_window();
            }
        })];
        // Who changed the size? Right after our own resize it is us (the window may settle on a
        // size a pixel or a frame off the one asked for); any other change is the user's, and
        // that size is then kept for the next files.
        subs.push(cx.observe_window_bounds(window, |this: &mut Self, window, _| {
            let b = window.bounds();
            let now = (f32::from(b.size.width), f32::from(b.size.height));
            let ours = this.resizing_until.is_some_and(|t| std::time::Instant::now() < t);
            if ours {
                this.programmatic = now;
            } else if !window.is_fullscreen()
                && !window.is_maximized()
                && ((now.0 - this.programmatic.0).abs() > 2.0 || (now.1 - this.programmatic.1).abs() > 2.0)
            {
                this.user_size = Some(now);
                this.programmatic = now;
            }
        }));
        let focus = cx.focus_handle();
        if cx.global::<LookSettings>().look_focus_on_open {
            window.focus(&focus, cx);
        }
        let hwnd = hwnd_of(window);
        let title = file_title(&path);
        let mut v = Self {
            path,
            title,
            pinned: false,
            topmost,
            overlay_titlebar: false,
            chrome_shown: false,
            more_open: false,
            busy: false,
            drag_by_body: true,
            drag_from: None,
            hwnd,
            load_gen: 0,
            _watch: None,
            _watch_task: None,
            focus,
            _hide: None,
            _loading: None,
            _subs: subs,
            viewer,
            programmatic: (0.0, 0.0),
            user_size: None,
            resizing_until: None,
        };
        let b = window.bounds();
        v.programmatic = (f32::from(b.size.width), f32::from(b.size.height));
        v.apply(prepared, false, window, cx);
        v
    }

    /// Dosyayı yeniden okur ve gösterir. `resize`: yeni dosya, pencere içeriğe göre boyutlanır.
    fn reload_viewer(&mut self, resize: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer.cleanup();
        self._loading = None;
        self.busy = false;
        if rf_win::drives::slow_to_read(&self.path) {
            self.load_in_background(resize, window, cx);
            return;
        }
        let (viewer, prepared) = prepare_viewer(&self.path, cx);
        self.viewer = viewer;
        self.apply(prepared, resize, window, cx);
    }

    /// A file on a network drive (or a USB disk, a cloud file not on this PC) is opened off the UI
    /// thread: reading a video's index or a PDF there can take seconds, and the window has to
    /// keep answering meanwhile. It says it is opening until the viewer is ready.
    pub(crate) fn load_in_background(&mut self, resize: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.viewer.cleanup();
        self.viewer = Box::new(Opening);
        self.busy = true;
        self.title = file_title(&self.path);
        window.set_window_title(&self.title);
        cx.notify();
        let path = self.path.clone();
        self._loading = Some(cx.spawn_in(window, async move |this, cx| {
            let (mut viewer, probe) = load_viewer(path, cx).await;
            let _ = this.update_in(cx, |this, window, cx| {
                if this.path != probe.path {
                    return;
                }
                this.busy = false;
                let prepared = viewer.prepare(&probe, cx);
                this.viewer = viewer;
                this.apply(prepared, resize, window, cx);
            });
        }));
    }

    fn apply(&mut self, prepared: Prepared, resize: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay_titlebar = prepared.overlay_titlebar;
        self.drag_by_body = prepared.drag_by_body;
        // a size the user chose stays exactly as it is (re-applying it would lose the few pixels
        // between the window's reported size and its frame)
        let keep_user_size = self.user_size.is_some() && prepared.resizable;
        if resize && !window.is_fullscreen() && !keep_user_size {
            let b = window.bounds();
            let cur = (f32::from(b.size.width), f32::from(b.size.height));
            // (a size the user dragged to is recorded by the bounds observer, see `new`)
            let w = rf_win::look::win::work_area_of(self.hwnd);
            let work = Rect { x: w[0], y: w[1], w: w[2], h: w[3] };
            let size = geometry::fit(prepared.preferred, work, prepared.fit_ratio);
            let old = Rect { x: f32::from(b.origin.x), y: f32::from(b.origin.y), w: cur.0, h: cur.1 };
            let r = geometry::relocate(old, size, work);
            self.resizing_until = Some(std::time::Instant::now() + Duration::from_millis(400));
            // Not from inside this update: SetWindowPos delivers the new size at once, while the
            // app is still borrowed, and GPUI drops it ("handling resize: RefCell already
            // borrowed"); the window then grows but the content stays at the old size. A task runs
            // after the update, when the size change reaches GPUI normally.
            let (hwnd, rect, scale) = (self.hwnd, [r.x, r.y, r.w, r.h], window.scale_factor());
            cx.spawn(async move |_, _| rf_win::look::win::set_rect(hwnd, rect, scale)).detach();
            self.programmatic = size;
        }
        self.title = prepared.title.into();
        self.load(window, cx);
    }

    /// Yeni dosyayı gösterir (aynı pencerede).
    fn load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.set_window_title(&self.title);
        self.more_open = false;
        self.load_gen += 1;
        self.watch(window, cx);
        cx.notify();
    }

    /// Ayar açıksa dosya değişince yeniden yükler (klasör izlenir, dosyanın zamanı ve boyutu karşılaştırılır).
    fn watch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self._watch = None;
        self._watch_task = None;
        let Some(dir) = self.path.parent().map(|d| d.to_path_buf()) else { return };
        let stamp = |p: &std::path::Path| std::fs::metadata(p).ok().map(|m| (m.modified().ok(), m.len()));
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        let Ok(w) = rf_win::watch::watch_dir(&dir, Duration::from_millis(300), move || {
            let _ = tx.unbounded_send(());
        }) else {
            return;
        };
        self._watch = Some(w);
        let path = self.path.clone();
        let mut last = stamp(&path);
        self._watch_task = Some(cx.spawn_in(window, async move |this, cx| {
            use futures::StreamExt;
            while rx.next().await.is_some() {
                let now = stamp(&path);
                let auto = this.update(cx, |_, cx| cx.global::<LookSettings>().look_auto_reload).unwrap_or(false);
                let reload = should_reload(auto, last.is_some(), now.is_some(), now != last);
                last = now;
                if reload {
                    let _ = this.update_in(cx, |v, window, cx| {
                        if v.path == path {
                            v.reload_viewer(false, window, cx);
                        }
                    });
                }
            }
        }));
    }

    pub(crate) fn set_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.path = path;
        self.reload_viewer(true, window, cx);
    }

    /// F5 / Yeniden yükle: aynı dosya, aynı boyut.
    pub(crate) fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_viewer(false, window, cx);
    }

    pub(crate) fn set_topmost(&mut self, on: bool, cx: &mut Context<Self>) {
        self.topmost = on;
        rf_win::look::win::set_topmost(self.hwnd, on);
        cx.notify();
    }

    /// The pointer moved over the content. The bar that lies over a picture or a video comes when the pointer enters the window's top
    /// strip, where the bar is; it used to come with every movement anywhere in the window and stay for a second and a half.
    fn pointer_moved(&mut self, at: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if !self.overlay_titlebar || self.chrome_shown || at.y < px(0.) || at.y >= theme::TITLEBAR_H {
            return;
        }
        self.chrome_shown = true;
        cx.notify();
        self.watch_bar(window, cx);
    }

    /// While the bar is shown: looks where the pointer is a few times a second and takes the bar away when it has left the strip.
    /// The pointer itself is asked, not the toolkit's events: over the bar the window gets no ordinary mouse messages (it is a
    /// caption to Windows), and a pointer that leaves the window sends none at all.
    fn watch_bar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let hwnd = self.hwnd;
        let strip = (f32::from(theme::TITLEBAR_H) * window.scale_factor()).round() as i32;
        self._hide = Some(cx.spawn_in(window, async move |this, cx| {
            let mut away = BarAway::default();
            loop {
                cx.background_executor().timer(BAR_WATCH).await;
                let over = rf_win::look::win::pointer_in_top_strip(hwnd, strip);
                let go_on = this.update(cx, |v, cx| {
                    if !v.chrome_shown {
                        return false;
                    }
                    if away.hide_now(over, v.more_open) {
                        v.chrome_shown = false;
                        cx.notify();
                        return false;
                    }
                    true
                });
                if !matches!(go_on, Ok(true)) {
                    break;
                }
            }
        }));
    }

    fn open_file(&self) {
        let p = self.path.clone();
        rf_win::sta::run(move |_| {
            if let Err(e) = rf_win::shell::open(&p) {
                log::warn!("look open: {e:#}");
            }
        });
    }

    fn share(&self) {
        let p = self.path.clone();
        rf_win::sta::run(move |owner| {
            let mut pt = windows::Win32::Foundation::POINT::default();
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt);
            }
            let target = rf_win::ctxmenu::ShellTarget::Items(vec![p]);
            if let Err(e) = rf_win::ctxmenu::invoke_verb(owner, &target, "Windows.ModernShare", (pt.x, pt.y)) {
                log::warn!("look share: {e:#}");
            }
        });
    }

    fn open_with(&self) {
        let p = self.path.clone();
        rf_win::sta::run(move |_| {
            if let Err(e) = rf_win::shell::open_with(&p) {
                log::warn!("look open with: {e:#}");
            }
        });
    }

    fn drag_out(&self) {
        let items = vec![self.path.clone()];
        let ui_thread = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture();
        }
        rf_win::sta::run(move |owner| {
            if let Err(e) = rf_win::dnd::drag_out(owner, ui_thread, &items) {
                log::warn!("look drag: {e:#}");
            }
        });
    }

    fn is_executable(&self) -> bool {
        let ext = self.path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        matches!(ext.as_str(), "exe" | "bat" | "cmd" | "msi" | "lnk" | "ps1" | "com")
    }

    fn button(&self, id: &'static str, icon: L, tip: SharedString, active: bool, p: &Palette) -> Stateful<Div> {
        div()
            .id(id)
            .size(px(32.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(theme::RADIUS_SM)
            .when(active, |d| d.bg(p.selection))
            .hover(|s| s.bg(p.hover))
            .active(|s| s.bg(p.pressed))
            // The button lies on the bar's drag area, which is a caption to Windows. A press there that nobody stops goes to Windows'
            // own caption handling, which waits to see whether the window is being dragged and swallows the release: the click never
            // came (the user: "üstte tut seçeneği kapatılmıyor"). Stopped here, the press is the button's alone and the release
            // arrives; the rest of the bar still drags the window.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(Icon::new(icon).size(px(16.)))
            .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(w, cx))
    }

    fn render_bar(&mut self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let show_reload = cx.global::<LookSettings>().look_show_reload;
        let run_tip: SharedString = if self.is_executable() { t!("Çalıştır").into() } else { t!("Aç").into() };
        let overlay = self.overlay_titlebar;
        let buttons = h_flex()
            .gap_0p5()
            .items_center()
            .child(self.button("look-pin", if self.pinned { L::PinOff } else { L::Pin }, t!("Sabitle").into(), self.pinned, p).on_click(
                cx.listener(|_, _, _, cx| {
                    cx.defer(super::LookManager::pin_current);
                }),
            ))
            .child(self.button("look-top", L::Layers, t!("Üstte tut").into(), self.topmost, p).on_click(cx.listener(|v, _, _, cx| {
                let on = !v.topmost;
                v.set_topmost(on, cx);
            })))
            .child(self.button("look-share", L::Share, t!("Paylaş").into(), false, p).on_click(cx.listener(|v, _, _, _| v.share())))
            .child(
                self.button("look-openwith", L::SquareArrowOutUpRight, t!("Birlikte aç").into(), false, p)
                    .on_click(cx.listener(|v, _, _, _| v.open_with())),
            )
            .child(self.button("look-open", L::ExternalLink, run_tip, false, p).on_click(cx.listener(|v, _, window, _| {
                v.open_file();
                window.remove_window();
            })))
            .when(show_reload, |d| {
                d.child(self.button("look-reload", L::RefreshCw, t!("Yeniden yükle").into(), false, p).on_click(cx.listener(|v, _, window, cx| {
                    v.reload(window, cx);
                })))
            })
            .child(self.button("look-more", L::Ellipsis, t!("Diğer").into(), self.more_open, p).on_click(cx.listener(|v, _, _, cx| {
                v.more_open = !v.more_open;
                cx.notify();
            })));
        TitleBar::new()
            .h(theme::TITLEBAR_H)
            .bg(if overlay { hsla(0., 0., 0., 0.55) } else { p.titlebar_bg })
            .border_color(gpui_kit::transparent_black())
            .when(overlay, |b| b.text_color(white()))
            .child(
                h_flex()
                    .id("look-bar")
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .items_center()
                            .text_xs()
                            .when(self.busy, |d| d.child(Icon::new(L::LoaderCircle).size(px(14.))))
                            .child(div().min_w_0().truncate().child(self.title.clone())),
                    )
                    .child(buttons),
            )
    }

    fn render_more(&self, p: &Palette, cx: &mut Context<Self>) -> impl IntoElement {
        let show_reload = cx.global::<LookSettings>().look_show_reload;
        let item = |id: &'static str, label: &'static str| {
            div().id(id).px_3().py_1p5().rounded(theme::RADIUS_SM).text_sm().hover(|s| s.bg(p.hover)).child(label)
        };
        v_flex()
            .absolute()
            .top(theme::TITLEBAR_H)
            .right(px(150.))
            .w(px(220.))
            .p_1()
            // a press on the menu is not also a press on the content under it (a video would pause)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .bg(p.card_bg)
            .border_1()
            .border_color(p.border)
            .rounded(theme::RADIUS_SM)
            .shadow_lg()
            .text_color(p.text)
            .when(!show_reload, |d| {
                d.child(item("more-reload", t!("Yeniden yükle")).on_click(cx.listener(|v, _, window, cx| {
                    v.more_open = false;
                    v.reload(window, cx);
                })))
            })
            .child(item("more-copy", t!("Yolu kopyala")).on_click(cx.listener(|v, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(v.path.to_string_lossy().into_owned()));
                v.more_open = false;
                cx.notify();
            })))
            // görüntüleyicinin kendi eylemleri
            .children(self.viewer.more_menu().into_iter().enumerate().map(|(i, m)| {
                let action = m.action.clone();
                div()
                    .id(("more-viewer", i))
                    .px_3()
                    .py_1p5()
                    .rounded(theme::RADIUS_SM)
                    .text_sm()
                    .hover(|s| s.bg(p.hover))
                    .child(m.label.clone())
                    .on_click(cx.listener(move |v, _, window, cx| {
                        v.more_open = false;
                        action(window, cx);
                        cx.notify();
                    }))
            }))
    }

    /// Görüntüleyicinin içeriği; gövdeden sürükleme dosyayı dışarı sürükler.
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let inner = self.viewer.render(window, cx);
        v_flex()
            .id("look-content")
            .size_full()
            // dosyayı pencereden sürükleyip bırakma (Explorer'a, masaüstüne…)
            .on_mouse_down(MouseButton::Left, cx.listener(|v, ev: &MouseDownEvent, _, _| {
                v.drag_from = (v.drag_by_body && ev.click_count == 1).then_some(ev.position);
            }))
            .on_mouse_up(MouseButton::Left, cx.listener(|v, _: &MouseUpEvent, _, _| v.drag_from = None))
            .on_mouse_move(cx.listener(|v, ev: &MouseMoveEvent, _, _| {
                if let Some(from) = v.drag_from {
                    if ev.pressed_button == Some(MouseButton::Left) && (ev.position - from).magnitude() > 6.0 {
                        v.drag_from = None;
                        v.drag_out();
                    }
                }
            }))
            .child(inner)
            .into_any_element()
    }
}

fn file_title(p: &std::path::Path) -> SharedString {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned()).into()
}

impl Render for LookWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::trace::mark("pencere çizildi");
        let p = Palette::for_window(window);
        let overlay = self.overlay_titlebar;
        let show_bar = !overlay || self.chrome_shown || self.more_open;
        let content = self.render_content(window, cx);
        // geçiş: yeni içerik kısa bir solmayla gelir
        let content = if cx.global::<LookSettings>().look_transition {
            div()
                .size_full()
                .child(content)
                .with_animation(("look-fade", self.load_gen), Animation::new(Duration::from_millis(140)), |d, t| d.opacity(t))
                .into_any_element()
        } else {
            content
        };
        let bar = show_bar.then(|| self.render_bar(&p, cx).into_any_element());
        let more = self.more_open.then(|| self.render_more(&p, cx).into_any_element());
        div()
            .id("look-root")
            .relative()
            .size_full()
            .track_focus(&self.focus)
            .text_color(p.text)
            .when(!overlay, |d| d.bg(p.window_bg))
            .when_some(cx.global::<LookSettings>().look_backdrop_color, |d, c| d.bg(rgb(c)))
            .on_mouse_move(cx.listener(|v, ev: &MouseMoveEvent, window, cx| v.pointer_moved(ev.position, window, cx)))
            .on_key_down(cx.listener(|v, ev: &KeyDownEvent, window, cx| {
                let k = ev.keystroke.key.as_str();
                let m = &ev.keystroke.modifiers;
                if m.control || m.alt || m.shift || m.platform {
                    if v.viewer.on_key(k, m, window, cx) {
                        cx.notify();
                    }
                    return;
                }
                match k {
                    "escape" | "space" => window.remove_window(),
                    "enter" => {
                        v.open_file();
                        window.remove_window();
                    }
                    "f11" => window.toggle_fullscreen(),
                    "f5" => v.reload(window, cx),
                    _ => {}
                }
            }))
            .map(|d| {
                if overlay {
                    // içerik tüm pencereyi kaplar, çubuk üstünde
                    // A press on the bar is not also a press on the content under it (the pin button paused the video). The bar takes
                    // the mouse by OCCLUDING the content, not by stopping the press: a press on the bar's drag area reaches Windows as
                    // a press on a caption only when nobody stopped it (the toolkit answers the message itself otherwise), and with
                    // the press stopped the window of a picture or a video could not be moved by its bar (the user's report).
                    // (When the bar comes and goes is `pointer_moved` and `watch_bar`.)
                    d.child(div().absolute().inset_0().child(content)).when_some(bar, |d, bar| d.child(div().absolute().top_0().left_0().right_0().occlude().child(bar)))
                } else {
                    d.child(
                        v_flex()
                            .size_full()
                            .when_some(bar, |d, bar| d.child(bar))
                            .child(div().flex_1().min_h_0().child(content)),
                    )
                }
            })
            .when_some(more, |d, m| d.child(m))
    }
}

#[cfg(test)]
mod tests {
    use super::{BAR_AWAY_LOOKS, BarAway, should_reload};

    #[test]
    fn the_bar_goes_after_the_pointer_was_away_a_few_looks_in_a_row() {
        let mut a = BarAway::default();
        assert!(!a.hide_now(true, false), "over the bar: it stays");
        for _ in 1..BAR_AWAY_LOOKS {
            assert!(!a.hide_now(false, false), "away for a moment: not yet");
        }
        assert!(a.hide_now(false, false), "away long enough");
        // a pointer that slips under the bar and comes back starts the count again
        let mut b = BarAway::default();
        assert!(!b.hide_now(false, false));
        assert!(!b.hide_now(true, false));
        for _ in 1..BAR_AWAY_LOOKS {
            assert!(!b.hide_now(false, false));
        }
        assert!(b.hide_now(false, false));
        // the bar's menu is open: the pointer is in the menu, and the bar stays however long
        let mut c = BarAway::default();
        for _ in 0..20 {
            assert!(!c.hide_now(false, true));
        }
        assert!(!c.hide_now(false, false), "the menu closed just now: the count starts here");
    }

    #[test]
    fn deleted_or_restored_file_always_reloads() {
        // dosya silindi: ayar kapalı olsa da Bilgi kartına geçilir
        assert!(should_reload(false, true, false, true));
        // geri geldi
        assert!(should_reload(false, false, true, true));
        // içerik değişti: yalnız otomatik yeniden yükleme açıksa
        assert!(!should_reload(false, true, true, true));
        assert!(should_reload(true, true, true, true));
        // klasörde başka bir dosya değişti
        assert!(!should_reload(true, true, true, false));
    }
}
