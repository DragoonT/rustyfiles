//! Video ve ses görüntüleyici (rf-player: FilmCraft çözücüleri, WASAPI ses).
//! Video: kareler pencere boyutuna ölçeklenip arka planda BGRA'ya çevrilir; başlık çubuğu ve
//! kontroller içeriğin üstünde durur, fare durunca gizlenir. Tıklama oynat/duraklat, çift tık
//! tam ekran, tekerlek ses düzeyi (0,04). Ses: 500×300 kart (kapak, başlık/sanatçı/albüm,
//! süre, eşzamanlı şarkı sözü). FilmCraft'ın açamadığı dosyalar Media Foundation ile oynatılır.

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_player::{AudioSink, Player, PlayerState};

use crate::look::viewer::{Prepared, Probe, Viewer};
use crate::look::config::LookSettings;
use crate::t;

/// Kontroller fare durunca bu kadar sonra gizlenir.
const HIDE_AFTER: Duration = Duration::from_millis(1500);
/// Oynatma döngüsünün yoklama aralığı.
const TICK: Duration = Duration::from_millis(8);

#[derive(Default)]
pub struct VideoViewer {
    view: Option<Entity<MediaView>>,
}

impl Viewer for VideoViewer {
    fn name(&self) -> &'static str {
        "video"
    }
    fn priority(&self) -> i32 {
        -3
    }
    fn can_handle(&self, f: &Probe) -> bool {
        // `.ts` çoğunlukla TypeScript: yalnız MPEG-TS paket eşitlemesi varsa video
        if f.ext == "ts" && !is_transport_stream(&f.head) {
            return false;
        }
        f.is_file() && (rf_player::can_open(&f.path) || rf_win::preview_host::is_playable(&f.path))
    }
    fn prepare(&mut self, f: &Probe, cx: &mut App) -> Prepared {
        let name = super::file_name(&f.path);
        // yalnız dizin okunur (hızlı); ilk karenin çözülebildiği arka planda denenir (MediaView::new)
        let opened = rf_player::open(&f.path);
        let (kind, prepared) = match opened {
            Ok(media) => match media.info().video.clone() {
                Some(v) => {
                    let par = if v.par.1 > 0 { v.par.0 as f32 / v.par.1 as f32 } else { 1.0 };
                    let size = ((v.width as f32 * par).max(160.0), (v.height as f32).max(90.0));
                    let mut p = Prepared::new(size, name);
                    p.overlay_titlebar = true;
                    p.drag_by_body = false;
                    (Kind::Video(media), p)
                }
                None => {
                    let mut p = Prepared::new((500.0, 300.0), name);
                    p.resizable = false;
                    (Kind::Audio(media), p)
                }
            },
            Err(e) if rf_win::preview_host::is_playable(&f.path) => {
                log::info!("look: rf-player cannot open {}, Media Foundation: {e:#}", f.path.display());
                (Kind::Mf, Prepared::new((960.0, 540.0), name))
            }
            Err(e) => (Kind::Failed(format!("{e:#}")), Prepared::new((520.0, 220.0), name)),
        };
        let path = f.path.clone();
        self.view = Some(cx.new(|cx| MediaView::new(path, kind, cx)));
        prepared
    }
    fn render(&mut self, _window: &mut Window, _cx: &mut App) -> AnyElement {
        self.view.as_ref().map_or_else(|| div().into_any_element(), |v| v.clone().into_any_element())
    }
    fn cleanup(&mut self) {
        self.view = None;
    }
}

/// MPEG-TS: 188 baytlık (ya da 4 bayt zaman damgalı 192 baytlık) paketlerin başında 0x47.
pub(crate) fn is_transport_stream(head: &[u8]) -> bool {
    let synced = |start: usize, step: usize| (0..3).all(|k| head.get(start + k * step) == Some(&0x47));
    synced(0, 188) || synced(4, 192)
}

enum Kind {
    Video(rf_player::Media),
    Audio(rf_player::Media),
    Mf,
    Failed(String),
}

fn sink() -> Box<dyn AudioSink> {
    if std::env::var_os("RUSTYFILES_MUTE").is_some() {
        return Box::new(rf_player::NullSink::new(48000, 2));
    }
    rf_player::wasapi_sink().unwrap_or_else(|e| {
        log::warn!("look audio: {e:#}");
        Box::new(rf_player::NullSink::new(48000, 2))
    })
}

fn clock(t: f64) -> String {
    let t = t.max(0.0) as u64;
    if t >= 3600 { format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60) } else { format!("{}:{:02}", t / 60, t % 60) }
}

fn bgra_image(w: u32, h: u32, data: Vec<u8>) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(w, h, data)?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

pub struct MediaView {
    path: PathBuf,
    video: bool,
    mf: bool,
    failed: Option<String>,
    player: Option<Player>,
    state: PlayerState,
    image: Option<Arc<RenderImage>>,
    /// Bir önceki kare: GPU dokusu bir kare sonra bırakılır (o an çizimde olabilir).
    previous: Option<Arc<RenderImage>>,
    shown_pts: f64,
    converting: bool,
    /// Kare çevrilecek boyut (aygıt pikseli).
    target: (u32, u32),
    volume: f32,
    muted: bool,
    looping: bool,
    controls: bool,
    hovered_controls: bool,
    bar: Rc<Cell<Option<Bounds<Pixels>>>>,
    meta: Vec<(&'static str, String)>,
    cover: Option<Arc<RenderImage>>,
    images: Option<Entity<crate::image_cache::ImageCache>>,
    lyrics: Vec<(f64, String)>,
    opened_at: Instant,
    first_frame_logged: bool,
    mf_rect: Option<[i32; 4]>,
    mf_owner: isize,
    /// Görüntü boyutu (piksel en-boyu uygulanmış).
    dims: (f32, f32),
    mf_state: Option<rf_win::preview_host::MediaState>,
    _hide: Option<Task<()>>,
    _tasks: Vec<Task<()>>,
}

impl MediaView {
    fn new(path: PathBuf, kind: Kind, cx: &mut Context<Self>) -> Self {
        let s = cx.global::<LookSettings>();
        let (volume, looping) = (s.look_volume.clamp(0.0, 1.0), s.look_loop);
        let mut v = Self {
            path: path.clone(),
            video: false,
            mf: false,
            failed: None,
            player: None,
            state: PlayerState::default(),
            image: None,
            previous: None,
            shown_pts: f64::NAN,
            converting: false,
            target: (0, 0),
            volume,
            muted: false,
            looping,
            controls: true,
            hovered_controls: false,
            bar: Default::default(),
            meta: Vec::new(),
            cover: None,
            images: None,
            lyrics: Vec::new(),
            opened_at: Instant::now(),
            first_frame_logged: false,
            mf_rect: None,
            mf_owner: 0,
            dims: (16.0, 9.0),
            mf_state: None,
            _hide: None,
            _tasks: Vec::new(),
        };
        match kind {
            Kind::Video(media) | Kind::Audio(media) => {
                v.video = media.info().has_video();
                if let Some(i) = media.info().video.as_ref() {
                    let par = if i.par.1 > 0 { i.par.0 as f32 / i.par.1 as f32 } else { 1.0 };
                    v.dims = ((i.width as f32 * par).max(1.0), (i.height as f32).max(1.0));
                }
                if v.video {
                    v.probe_first_frame(media.clone(), cx);
                }
                let mut p = Player::new(media, sink());
                p.set_volume(volume);
                p.set_loop(looping);
                p.play();
                v.player = Some(p);
                v.tick(cx);
            }
            Kind::Mf => {
                v.mf = true;
                v.video = true;
                v.controls = true;
                v.poll_mf(cx);
            }
            Kind::Failed(e) => v.failed = Some(e),
        }
        if !v.video && v.failed.is_none() {
            v.load_audio_info(cx);
        }
        // pencere kapanınca: ses durur, GPU dokuları bırakılır
        cx.on_release(|v: &mut Self, cx| {
            v.player = None;
            if v.mf {
                rf_win::sta::run(|_| rf_win::preview_host::hide());
            }
            for img in [v.image.take(), v.previous.take(), v.cover.take()].into_iter().flatten() {
                cx.drop_image(img, None);
            }
        })
        .detach();
        v.arm_hide(cx);
        v
    }

    /// FilmCraft bazı biçimleri (ör. 4:2:2 HEVC) açar ama karesini çözemez: ilk kare arka planda
    /// denenir, olmazsa Media Foundation'a (o da yoksa açıklamaya) geçilir.
    fn probe_first_frame(&mut self, media: rf_player::Media, cx: &mut Context<Self>) {
        let probe = cx.background_spawn(async move { media.frame_at(0.0).err().map(|e| format!("{e:#}")) });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let Some(err) = probe.await else { return };
            let _ = this.update(cx, |v, cx| {
                v.player = None;
                if rf_win::preview_host::is_playable(&v.path) {
                    log::info!("look: rf-player cannot decode {}, Media Foundation: {err}", v.path.display());
                    v.mf = true;
                    v.controls = true;
                    v.opened_at = Instant::now();
                    v.poll_mf(cx);
                } else {
                    v.failed = Some(err);
                }
                cx.notify();
            });
        }));
    }

    fn load_audio_info(&mut self, cx: &mut Context<Self>) {
        let p = self.path.clone();
        let work = cx.background_spawn(async move {
            let meta = rf_win::props::media_props(&p);
            let lyrics = crate::look::lyrics::sidecar(&p).unwrap_or_default();
            (meta, lyrics)
        });
        self._tasks.push(cx.spawn(async move |this, cx| {
            let (meta, lyrics) = work.await;
            if std::env::var_os("RUSTYFILES_LOOK_TRACE").is_some() {
                eprintln!("look: ses bilgisi {meta:?}, {} söz satırı", lyrics.len());
            }
            let _ = this.update(cx, |v, cx| {
                v.meta = meta;
                v.lyrics = lyrics;
                cx.notify();
            });
        }));
        // kapak: kabuk küçük resmi, paylaşılan önbellekten (kendi iş parçacıklarında, mesaj döngülü)
        let images = crate::look::LookManager::images(cx);
        cx.observe(&images, |_, _, cx| cx.notify()).detach();
        self.images = Some(images);
    }

    /// Oynatma döngüsü: yeni kare varsa arka planda çevrilir; durum güncellenir.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self._tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                let job = this.update(cx, |v, cx| {
                    let p = v.player.as_ref()?;
                    let st = p.state();
                    let coarse = |s: &PlayerState| ((s.position * 10.0) as i64, s.playing, s.ended, (s.duration * 10.0) as i64);
                    if coarse(&st) != coarse(&v.state) {
                        cx.notify();
                    }
                    v.state = st;
                    if !v.video || v.converting || v.target.0 == 0 {
                        return None;
                    }
                    let f = p.current_frame()?;
                    if f.pts == v.shown_pts {
                        return None;
                    }
                    v.converting = true;
                    Some((f, v.target))
                });
                let Ok(job) = job else { break };
                let Some((frame, (w, h))) = job else { continue };
                let pts = frame.pts;
                let data = cx.background_spawn(async move { rf_player::to_bgra(&frame, w, h) }).await;
                let img = bgra_image(w, h, data);
                if this
                    .update(cx, |v, cx| {
                        v.converting = false;
                        v.shown_pts = pts;
                        if let Some(old) = v.previous.take() {
                            cx.drop_image(old, None);
                        }
                        v.previous = std::mem::replace(&mut v.image, img);
                        if !v.first_frame_logged {
                            v.first_frame_logged = true;
                            crate::look::trace::mark("ilk video karesi");
                        }
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
    }

    fn poll_mf(&mut self, cx: &mut Context<Self>) {
        self._tasks.push(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            let (tx, rx) = futures::channel::oneshot::channel();
            rf_win::sta::run(move |_| {
                let _ = tx.send(rf_win::preview_host::media_state());
            });
            let st = rx.await.ok().flatten();
            let gone = this.update(cx, |v, cx| {
                v.mf_state = st;
                // Media Foundation de açamadıysa (süre gelmedi): oynatıcı kapanır, açıklama gösterilir
                if v.failed.is_none() && v.opened_at.elapsed() > Duration::from_secs(2) && st.is_none_or(|s| s.duration <= 0.0) {
                    v.failed = Some(t!("Bu biçim desteklenmiyor (ne FilmCraft ne de Windows çözebildi)").to_string());
                    v.mf = false;
                    v.video = false;
                    rf_win::sta::run(|_| rf_win::preview_host::hide());
                }
                cx.notify();
                v.failed.is_some()
            });
            if gone.unwrap_or(true) {
                break;
            }
        }));
    }

    fn arm_hide(&mut self, cx: &mut Context<Self>) {
        self._hide = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HIDE_AFTER).await;
            let _ = this.update(cx, |v, cx| {
                if !v.hovered_controls && v.video && !v.mf {
                    v.controls = false;
                    cx.notify();
                }
            });
        }));
    }

    fn show_controls(&mut self, cx: &mut Context<Self>) {
        if !self.controls {
            self.controls = true;
            cx.notify();
        }
        self.arm_hide(cx);
    }

    fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.mf {
            let playing = self.mf_state.is_some_and(|s| s.playing);
            let (owner, path) = (self.mf_owner, self.path.clone());
            rf_win::sta::run(move |_| {
                if playing {
                    rf_win::preview_host::media_pause()
                } else {
                    rf_win::preview_host::media_play(owner, &path, None)
                }
            });
            return;
        }
        if let Some(p) = &mut self.player {
            if p.state().playing {
                p.pause();
            } else {
                p.play();
            }
        }
        cx.notify();
    }

    fn seek_fraction(&mut self, f: f64) {
        let f = f.clamp(0.0, 1.0);
        if self.mf {
            rf_win::sta::run(move |_| rf_win::preview_host::media_seek(f));
        } else if let Some(p) = &mut self.player {
            let d = p.state().duration;
            p.seek(f * d);
        }
    }

    fn set_volume(&mut self, v: f32, cx: &mut Context<Self>) {
        self.volume = v.clamp(0.0, 1.0);
        if let Some(p) = &mut self.player {
            p.set_volume(self.volume);
        }
        crate::look::config::update(cx, |s| s.look_volume = self.volume);
        cx.notify();
    }

    fn render_controls(&self, light_text: bool, cx: &mut Context<Self>) -> AnyElement {
        let (playing, pos, dur) = if self.mf {
            self.mf_state.map_or((false, 0.0, 0.0), |s| (s.playing, s.position, s.duration))
        } else {
            (self.state.playing, self.state.position, self.state.duration)
        };
        let frac = if dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let fg: Hsla = if light_text { white().into() } else { rgb(0x1a1a1a).into() };
        let track: Hsla = if light_text { hsla(0., 0., 1., 0.3) } else { hsla(0., 0., 0., 0.2) };
        let accent = cx.global::<gpui_kit::component::Theme>().primary;
        let bar = self.bar.clone();
        let btn = |id: &'static str, icon: L, active: bool| {
            div()
                .id(id)
                .size(px(30.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .when(active, |d| d.bg(hsla(0., 0., 1., 0.18)))
                .hover(|s| s.bg(hsla(0., 0., 1., 0.12)))
                .child(Icon::new(icon).size(px(16.)).text_color(fg))
        };
        h_flex()
            .id("look-media-controls")
            .w_full()
            .gap_2()
            .px_3()
            .py_2()
            .items_center()
            .text_color(fg)
            .text_xs()
            .on_hover(cx.listener(|v, h: &bool, _, _| v.hovered_controls = *h))
            .child(btn("media-play", if playing { L::Pause } else { L::Play }, false).on_click(cx.listener(|v, _, _, cx| v.toggle_play(cx))))
            .child(div().w(px(84.)).flex_shrink_0().child(format!("{} / {}", clock(pos), clock(dur))))
            .child(
                div()
                    .id("media-seek")
                    .flex_1()
                    .h(px(18.))
                    .flex()
                    .items_center()
                    .cursor_pointer()
                    .child({
                        let bar = bar.clone();
                        canvas(move |b, _, _| bar.set(Some(b)), |_, _, _, _| {}).absolute().size_full()
                    })
                    .child(div().relative().w_full().h(px(4.)).rounded(px(2.)).bg(track).child(div().h_full().w(relative(frac)).rounded(px(2.)).bg(accent)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |v, ev: &MouseDownEvent, _, cx| {
                        if let Some(b) = bar.get() {
                            let f = (f32::from(ev.position.x - b.origin.x) / f32::from(b.size.width).max(1.0)) as f64;
                            v.seek_fraction(f);
                            cx.notify();
                        }
                        cx.stop_propagation();
                    })),
            )
            .when(!self.mf, |d| {
                d.child(btn("media-loop", L::RotateCw, self.looping).on_click(cx.listener(|v, _, _, cx| {
                    v.looping = !v.looping;
                    if let Some(p) = &mut v.player {
                        p.set_loop(v.looping);
                    }
                    let on = v.looping;
                    crate::look::config::update(cx, |s| s.look_loop = on);
                    cx.notify();
                })))
                .child(btn("media-mute", if self.muted { L::Minus } else { L::Music }, self.muted).on_click(cx.listener(|v, _, _, cx| {
                    v.muted = !v.muted;
                    let m = v.muted;
                    if let Some(p) = &mut v.player {
                        p.set_mute(m);
                    }
                    cx.notify();
                })))
                .child(div().w(px(34.)).text_right().child(format!("{:.0}%", self.volume * 100.0)))
            })
            .into_any_element()
    }
}

impl Render for MediaView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = crate::theme::Palette::for_window(window);
        if let Some(e) = &self.failed {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .child(div().text_color(p.text).child(t!("Bu dosya oynatılamıyor")))
                .child(div().text_xs().text_color(p.text_secondary).child(e.clone()))
                .into_any_element();
        }
        if !self.video {
            return self.render_audio(window, cx);
        }
        // video: alana sığan dikdörtgen
        let area = window.viewport_size();
        let (aw, ah) = (f32::from(area.width), f32::from(area.height));
        let (vw, vh) = self.dims;
        let s = (aw / vw).min(ah / vh);
        let (dw, dh) = ((vw * s).floor(), (vh * s).floor());
        let scale = window.scale_factor();
        self.target = (((dw * scale) as u32).max(2), ((dh * scale) as u32).max(2));
        let controls = self.controls || self.mf;
        let mut root = div()
            .id("look-media")
            .relative()
            .size_full()
            .bg(black())
            .on_mouse_move(cx.listener(|v, _: &MouseMoveEvent, _, cx| v.show_controls(cx)))
            .on_scroll(cx.listener(|v, ev: &ScrollEvent, _, cx| {
                let dy = match ev.delta {
                    ScrollDelta::Pixels(p) => f32::from(p.y) / 100.0,
                    ScrollDelta::Lines(l) => l.y / 3.0,
                };
                v.set_volume(v.volume + 0.04 * dy, cx);
                v.show_controls(cx);
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(|v, ev: &MouseDownEvent, window, cx| {
                if ev.click_count == 2 {
                    window.toggle_fullscreen();
                } else if ev.click_count == 1 {
                    v.toggle_play(cx);
                }
            }));
        if self.mf {
            // Media Foundation: video kendi penceresinde; kontroller altta sabit
            let rect = [
                ((f32::from(window.bounds().origin.x) + (aw - dw) / 2.0) * scale) as i32,
                ((f32::from(window.bounds().origin.y) + (ah - 48.0 - dh.min(ah - 48.0)) / 2.0) * scale) as i32,
                ((f32::from(window.bounds().origin.x) + (aw + dw) / 2.0) * scale) as i32,
                ((f32::from(window.bounds().origin.y) + (ah - 48.0 + dh.min(ah - 48.0)) / 2.0) * scale) as i32,
            ];
            if self.mf_rect != Some(rect) {
                let first = self.mf_rect.is_none();
                self.mf_rect = Some(rect);
                let owner = crate::look::window::hwnd_of(window);
                self.mf_owner = owner;
                let path = self.path.clone();
                let r = windows::Win32::Foundation::RECT { left: rect[0], top: rect[1], right: rect[2], bottom: rect[3] };
                rf_win::sta::run(move |_| {
                    if first {
                        rf_win::preview_host::media_play(owner, &path, Some(r));
                    } else {
                        rf_win::preview_host::media_reposition(r);
                    }
                });
            }
        } else if let Some(img) = self.image.clone() {
            root = root.child(
                gpui_kit::img(img)
                    .absolute()
                    .left(px(((aw - dw) / 2.0).floor()))
                    .top(px(((ah - dh) / 2.0).floor()))
                    .w(px(dw))
                    .h(px(dh))
                    .object_fit(ObjectFit::Fill),
            );
        }
        if controls {
            root = root.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom_0()
                    .bg(linear_gradient(0., linear_color_stop(hsla(0., 0., 0., 0.7), 0.), linear_color_stop(hsla(0., 0., 0., 0.0), 1.)))
                    .pt_6()
                    .child(self.render_controls(true, cx)),
            );
        }
        root.into_any_element()
    }
}

impl MediaView {
    fn render_audio(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let lyric = crate::look::lyrics::line_at(&self.lyrics, self.state.position).map(str::to_string);
        if self.cover.is_none() {
            if let (Some(images), Some(dir), Ok(m)) = (self.images.clone(), self.path.parent(), std::fs::metadata(&self.path)) {
                let name = super::file_name(&self.path);
                let e = rf_core::Entry::new(&name, rf_core::EntryKind::File, m.len(), 0, 0, 0);
                let dir = dir.to_path_buf();
                // önbellek kendi dokusunu yönetir: burada bırakılmaz
                let thumb = images.update(cx, |c, cx| c.thumbnail(&dir, &e, 256, cx));
                if let Some(t) = thumb {
                    return self.render_audio_with(t, window, cx, lyric);
                }
            }
        }
        let cover = self.cover.clone();
        self.render_audio_inner(cover, window, cx, lyric)
    }

    fn render_audio_with(&mut self, cover: Arc<RenderImage>, window: &mut Window, cx: &mut Context<Self>, lyric: Option<String>) -> AnyElement {
        self.render_audio_inner(Some(cover), window, cx, lyric)
    }

    fn render_audio_inner(&mut self, cover: Option<Arc<RenderImage>>, window: &mut Window, cx: &mut Context<Self>, lyric: Option<String>) -> AnyElement {
        let p = crate::theme::Palette::for_window(window);
        let field = |k: &str| self.meta.iter().find(|(l, _)| *l == k).map(|(_, v)| v.clone());
        let title = field("Başlık").unwrap_or_else(|| super::file_name(&self.path));
        let artist = field("Sanatçı");
        let album = field("Albüm");
        h_flex()
            .size_full()
            .p_4()
            .gap_4()
            .items_center()
            .child(
                div()
                    .size(px(150.))
                    .flex_shrink_0()
                    .rounded(px(8.))
                    .overflow_hidden()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(p.card_bg)
                    .map(|d| match cover {
                        Some(c) => d.child(gpui_kit::img(c).size_full().object_fit(ObjectFit::Cover)).into_any_element(),
                        None => d.child(Icon::new(L::Music).size(px(56.)).text_color(p.text_secondary)).into_any_element(),
                    }),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(div().text_lg().font_weight(FontWeight::SEMIBOLD).text_color(p.text).truncate().child(title))
                    .when_some(artist, |d, a| d.child(div().text_sm().text_color(p.text_secondary).truncate().child(a)))
                    .when_some(album, |d, a| d.child(div().text_xs().text_color(p.text_tertiary).truncate().child(a)))
                    .when_some(lyric, |d, l| d.child(div().pt_2().text_sm().text_color(p.text).child(l)))
                    .child(div().pt_2().child(self.render_controls(p.dark, cx))),
            )
            .into_any_element()
    }
}
