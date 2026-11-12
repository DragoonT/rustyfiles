//! The control centre: the panel that opens above the right island when its network or volume symbol is clicked (`panel.rs` opens,
//! places and closes it; this is what is inside). About 360 logical px wide, as tall as what it shows, on HyperGlass, with the app's
//! liquid controls. Top to bottom, in the layout of the mock-up the user liked:
//!
//! 1. Connectivity tiles, two to a row: the network (Wi-Fi name and strength, Ethernet, not connected), Bluetooth (off, on, or who is
//!    connected) and, on a machine with a battery, the battery. A tile opens Windows' page for it; none of them switches anything
//!    (see the notes in `rf_win::sysstatus`). A tile whose source is not on the machine is left out.
//! 2. "Now playing", only while the system has a media session: artwork, title, artist, the program, previous / play-pause / next.
//! 3. Sound: a liquid slider for the master volume with a speaker at its left that mutes, the default output device's name under it,
//!    and a chevron that lists the output devices (read-only: switching the default device is undocumented, so the list ends with a
//!    link to Windows' sound settings).
//! 4. Brightness, only when the primary monitor answers DDC/CI.
//! 5. A gear that opens Windows' Settings.
//!
//! WHAT RUNS. The sound and the network are followed all the time by the dock (the island shows them: `sysstate.rs`); this view only
//! reads them (and is redrawn when they change). Media, Bluetooth, the battery and the brightness are followed only while the panel
//! is open: each is a thread of `rf_win::sysstatus` that ends when the panel is hidden, and what they last said is kept so that the
//! next opening starts with the same cards and corrects them within a moment. The panel's height follows what is shown; when a card
//! appears or goes while it is open the window is placed again (`panel::relayout`), growing upwards.
//!
//! THE KEYBOARD. Every control is a tab stop (Tab and Shift+Tab: the tiles, the speaker, the volume slider, the outputs button, the
//! media buttons, the brightness slider, the gear), with a ring in the accent colour while it has the focus by key. Enter and Space
//! press a button; a slider takes the arrows (one step), Shift or the page keys (ten steps), Home and End; Escape closes the panel.
//!
//! WRITES. The slider's drag, the speaker, the transport buttons and the brightness slider are the user's own clicks and drags; each
//! call is made from that event and nowhere else.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::panel::{self as geo, CloseCause, PanelId};
use rf_win::sysstatus::Subscription;
use rf_win::sysstatus::audio;
use rf_win::sysstatus::bluetooth::{self, BluetoothSnapshot};
use rf_win::sysstatus::brightness::{self, Brightness, BrightnessSnapshot};
use rf_win::sysstatus::media::{self, Artwork, Media, MediaSnapshot, Transport};
use rf_win::sysstatus::network::{self, Link};
use rf_win::sysstatus::power::{self, BatterySnapshot};

use super::backdrop::IslandGlass;
use super::cc_model::{self, Shown, Tile, Words};
use super::look::Palette;
use super::sysstate::{self, SysState};
use crate::glass_watch::{self, PopupGlass};
use crate::t;
use crate::theme::Palette as ThemePalette;

/// The two sliders' ids (for the drag).
const SL_VOLUME: usize = 0;
const SL_BRIGHTNESS: usize = 1;
/// The corner radius of a card, logical px (the window's own corners are the system's 8; a card sits 12 px inside them).
const CARD_R: f32 = 12.0;
/// How often the Wi-Fi bars are read again while the panel is open (Windows has no event for a signal that only changes).
const WIFI_POLL: Duration = Duration::from_secs(5);

/// What the live sources tell the view.
enum Ev {
    Media(MediaSnapshot),
    Bluetooth(BluetoothSnapshot),
    Battery(BatterySnapshot),
    Brightness(BrightnessSnapshot),
}

/// The sources that run while the panel is open. Dropping it stops them.
struct Live {
    media: Media,
    _bluetooth: Subscription,
    _battery: Option<Subscription>,
    brightness: Brightness,
}

#[derive(Clone)]
struct CcDrag(usize);

struct Ghost;
impl Render for Ghost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

pub struct CcView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// The panel is on the screen or easing out (false while hidden: nothing is built then).
    shown: bool,
    /// A choice was made (a Windows page is opening) or the panel is easing out: a second click does nothing.
    closing: bool,
    /// Started at the open or the close; `true` is opening.
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    /// The keyboard stops, made when first drawn and kept (a handle that is made again at every frame would never keep the focus).
    stops: RefCell<HashMap<(&'static str, usize), FocusHandle>>,
    /// What the last click on the speaker asked for, and when (see `cc_model::mute_target`).
    mute_ask: Option<(bool, Instant)>,

    // what the live sources last said
    media: Option<MediaSnapshot>,
    bluetooth: Option<BluetoothSnapshot>,
    battery: Option<BatterySnapshot>,
    brightness: Option<BrightnessSnapshot>,
    /// The decoded artwork turned into a picture GPUI draws, and the number of the artwork it is from.
    art: Option<(u64, Arc<RenderImage>)>,
    live: Option<Live>,
    _tasks: Vec<Task<()>>,

    // the interface
    devices_open: bool,
    /// The slider being dragged right now.
    drag: Option<usize>,
    /// The volume the slider shows while the user drags it and until Windows has answered (the snapshot may lag a moment).
    vol_drag: Option<f32>,
    /// The height last given to the window (logical px).
    reported: f32,
    _subs: Vec<Subscription2>,
}

/// GPUI's observer handle (named apart from `rf_win::sysstatus::Subscription`).
type Subscription2 = gpui_kit::Subscription;

impl CcView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // the sound and the network change under the open panel (the keyboard's volume keys, a cable pulled): redraw
        let subs = vec![cx.observe_global::<SysState>(|this, cx| {
            if this.drag != Some(SL_VOLUME) {
                this.vol_drag = None;
            }
            if this.shown {
                this.refit(cx);
                cx.notify();
            }
        })];
        CcView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            stops: RefCell::new(HashMap::new()),
            mute_ask: None,
            media: None,
            bluetooth: None,
            battery: None,
            brightness: None,
            art: None,
            live: None,
            _tasks: Vec::new(),
            devices_open: false,
            drag: None,
            vol_drag: None,
            reported: 0.0,
            _subs: subs,
        }
    }

    /// The keyboard stop `(key, i)`: made the first time it is asked for.
    fn stop(&self, key: &'static str, i: usize, cx: &mut Context<Self>) -> FocusHandle {
        self.stops.borrow_mut().entry((key, i)).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    // ------------------------------------------------------------------ what is shown

    fn shown_now(&self, cx: &App) -> Shown {
        let net = sysstate::network(cx);
        let audio = sysstate::audio(cx);
        let tiles = cc_model::tiles(net.as_ref(), self.bluetooth.as_ref(), self.battery.as_ref()).len();
        let sound = audio.as_ref().is_some_and(|a| a.available);
        Shown {
            tiles,
            media: self.media.as_ref().is_some_and(|m| m.session.is_some()),
            sound,
            devices: (self.devices_open && sound).then(|| audio.as_ref().map_or(0, |a| a.devices.len()) + 1),
            brightness: self.brightness.is_some_and(|b| b.available),
        }
    }

    /// The window's height now, logical px.
    pub fn height(&self, cx: &App) -> f32 {
        cc_model::height(&self.shown_now(cx))
    }

    /// Tells the panel when what is shown wants another height.
    fn refit(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let h = self.height(cx);
        if (h - self.reported).abs() > 0.5 {
            self.reported = h;
            super::panel::relayout(PanelId::CONTROL_CENTRE, h, cx);
        }
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    /// The panel is about to be shown: start the sources that run while it is open and begin easing in.
    pub fn on_open(&mut self, _dark: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = Some((Instant::now(), true));
        self.devices_open = false;
        self.drag = None;
        self.vol_drag = None;
        self.mute_ask = None;
        self.reported = self.height(cx);
        self.start_live(cx);
        cx.notify();
    }

    /// The window is on the screen: the glass goes on, the keyboard goes to the panel.
    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The panel is being put away: ease out.
    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        self.devices_open = false;
        self.drag = None;
        cx.notify();
    }

    /// The window is hidden: the glass comes off and the sources stop.
    pub fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self.live = None;
        self._tasks.clear();
        cx.notify();
    }

    /// Starts what runs while the panel is open: the media session, Bluetooth, the battery (if there is one), the brightness, and the
    /// 5 s look at the Wi-Fi bars.
    fn start_live(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        let (t_media, t_bt, t_bat, t_br) = (tx.clone(), tx.clone(), tx.clone(), tx);
        let media = media::subscribe(move |s| {
            let _ = t_media.unbounded_send(Ev::Media(s.clone()));
        });
        let bluetooth = bluetooth::subscribe(move |s| {
            let _ = t_bt.unbounded_send(Ev::Bluetooth(s.clone()));
        });
        // the battery is cheap to read: read it now, follow it only on a machine that has one
        let now = power::read();
        self.battery = Some(now);
        let battery = now.present.then(|| {
            power::subscribe(move |b| {
                let _ = t_bat.unbounded_send(Ev::Battery(b));
            })
        });
        let brightness = brightness::open(move |b| {
            let _ = t_br.unbounded_send(Ev::Brightness(*b));
        });
        self.live = Some(Live { media, _bluetooth: bluetooth, _battery: battery, brightness });
        network::refresh();
        let events = cx.spawn(async move |this, cx| {
            while let Some(ev) = rx.next().await {
                if this.update(cx, |me, cx| me.apply(ev, cx)).is_err() {
                    break;
                }
            }
        });
        let poll = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(WIFI_POLL).await;
                let Ok(wifi) = this.update(cx, |me, cx| me.shown && matches!(sysstate::network(cx).map(|n| n.link), Some(Link::WiFi { .. }))) else { break };
                if wifi {
                    network::refresh();
                }
            }
        });
        self._tasks = vec![events, poll];
    }

    fn apply(&mut self, ev: Ev, cx: &mut Context<Self>) {
        match ev {
            Ev::Media(m) => {
                // the picture GPUI draws is made once per artwork, here (the pixels were decoded and cut down on the media thread)
                let id = m.session.as_ref().and_then(|s| s.artwork.as_ref()).map(|a| a.id);
                match (id, self.art.as_ref().map(|a| a.0)) {
                    (None, _) => self.art = None,
                    (Some(id), cur) if cur != Some(id) => {
                        self.art = m.session.as_ref().and_then(|s| s.artwork.as_deref()).and_then(art_image).map(|i| (id, i));
                    }
                    _ => {}
                }
                self.media = Some(m);
            }
            Ev::Bluetooth(b) => self.bluetooth = Some(b),
            Ev::Battery(b) => self.battery = Some(b),
            // a value the user has just dragged to is not taken back by the monitor's old answer
            // (DDC/CI misses a transfer now and then even after the retries: a monitor that answered before and does not now keeps its
            // row rather than losing it for one opening)
            Ev::Brightness(b) => {
                if self.drag != Some(SL_BRIGHTNESS) && (b.available || !self.brightness.is_some_and(|old| old.available)) {
                    self.brightness = Some(b);
                }
            }
        }
        self.refit(cx);
        cx.notify();
    }

    // ------------------------------------------------------------------ doing things (each is the user's own click)

    /// Opens a Windows settings page and puts the panel away.
    fn open_settings(&mut self, uri: &'static str, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.closing = true;
        open_uri(uri);
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    fn drag_to(&mut self, which: usize, fraction: f32, cx: &mut Context<Self>) {
        match which {
            SL_VOLUME => {
                let a = sysstate::audio(cx);
                let steps = a.as_ref().map_or(0, |a| a.steps);
                let v = audio::slider_to_volume(fraction, steps);
                if self.vol_drag != Some(v) {
                    self.vol_drag = Some(v);
                    audio::set_volume(fraction);
                    // like Windows' own slider: raising the volume of a muted speaker unmutes it
                    if a.is_some_and(|a| a.muted) && v > 0.0 {
                        audio::set_muted(false);
                    }
                }
            }
            SL_BRIGHTNESS => {
                if let (Some(b), Some(live)) = (self.brightness.as_mut(), self.live.as_ref()) {
                    let raw = b.value_at(fraction);
                    if raw != b.value {
                        b.value = raw;
                        live.brightness.set(raw);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// A slider's share now and how many parts it has, for the keyboard.
    fn slider_state(&self, which: usize, cx: &App) -> Option<(f32, u32)> {
        match which {
            SL_VOLUME => {
                let a = sysstate::audio(cx)?;
                Some((audio::volume_to_slider(self.vol_drag.unwrap_or(a.volume)), cc_model::slider_parts(a.steps)))
            }
            SL_BRIGHTNESS => self.brightness.filter(|b| b.available).map(|b| (b.fraction(), 20)),
            _ => None,
        }
    }

    fn transport(&self, t: Transport) {
        if let Some(live) = &self.live {
            live.media.transport(t);
        }
    }

    // ------------------------------------------------------------------ drawing the parts

    /// A round button with a glyph; `lit` fills it with the accent (the thing is on). `focus` makes it a keyboard stop (it shows a ring
    /// while it has the focus by key), `tip` names it for the pointer.
    fn round_button(&self, id: impl Into<ElementId>, icon: L, size: f32, lit: bool, look: &Look, (focus, tip): (Option<&FocusHandle>, Option<&'static str>)) -> Stateful<Div> {
        let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
        let glyph = if lit { look.on_accent } else { look.pal.text().hsla() };
        let ring = if lit { look.on_accent } else { look.accent };
        div()
            .id(id)
            .size(px(size))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .border_2()
            .border_color(transparent_black())
            .cursor_pointer()
            .when_some(focus, |d, f| d.track_focus(f).focus_visible(move |s| s.border_color(ring)))
            .when_some(tip, |d, tip| d.tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)))
            .when(lit, |d| d.bg(look.accent))
            .when(!lit, |d| d.bg(look.pal.field_fill().hsla()).hover(move |s| s.bg(hover)).active(move |s| s.bg(pressed)))
            .child(Icon::new(icon).size(px(size * 0.45)).text_color(glyph))
    }

    fn card(&self, look: &Look) -> Div {
        div().rounded(px(CARD_R)).bg(look.pal.field_fill().hsla())
    }

    fn tile(&self, id: &'static str, icon: L, text: cc_model::TileText, look: &Look, uri: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let pressed = look.pal.pressed_fill().hsla();
        let ring = look.accent;
        let focus = self.stop(id, 0, cx);
        // (the round button inside is the tile's picture; the tile is the one keyboard stop)
        let button = self.round_button((id, 1usize), icon, 40.0, text.active, look, (None, None));
        h_flex()
            .id(id)
            .flex_1()
            .min_w_0()
            .h(px(cc_model::TILE_H))
            .px(px(10.))
            .gap(px(10.))
            .items_center()
            .rounded(px(CARD_R))
            .border_2()
            .border_color(transparent_black())
            .bg(look.pal.field_fill().hsla())
            .cursor_pointer()
            .track_focus(&focus)
            .focus_visible(move |s| s.border_color(ring))
            .hover(move |s| s.bg(pressed))
            .child(button.cursor_pointer())
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(text.title))
                    .child(div().truncate().text_xs().text_color(look.pal.text_secondary().hsla()).child(text.subtitle)),
            )
            .on_click(cx.listener(move |this, _, _, cx| this.open_settings(uri, cx)))
            .into_any_element()
    }

    fn render_tiles(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let net = sysstate::network(cx);
        let tiles = cc_model::tiles(net.as_ref(), self.bluetooth.as_ref(), self.battery.as_ref());
        let mut items: Vec<AnyElement> = Vec::new();
        for t in tiles {
            items.push(match t {
                Tile::Network => {
                    let n = net.clone().unwrap_or_default();
                    self.tile("cc-network", cc_model::network_icon(network::glyph(&n)), cc_model::network_text(&n, words), look, "ms-settings:network", cx)
                }
                Tile::Bluetooth => {
                    let b = self.bluetooth.clone().unwrap_or_default();
                    self.tile("cc-bluetooth", cc_model::bluetooth_icon(&b), cc_model::bluetooth_text(&b, words), look, "ms-settings:bluetooth", cx)
                }
                Tile::Battery => {
                    let b = self.battery.unwrap_or_default();
                    self.tile("cc-battery", cc_model::battery_icon(&b), cc_model::battery_text(&b, words), look, "ms-settings:powersleep", cx)
                }
            });
        }
        // two to a row; a lone tile keeps its half
        let mut rows = Vec::new();
        let mut it = items.into_iter();
        while let Some(first) = it.next() {
            let second = it.next().unwrap_or_else(|| div().flex_1().into_any_element());
            rows.push(h_flex().gap(px(cc_model::GAP)).child(first).child(second).into_any_element());
        }
        rows
    }

    fn render_media(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = self.media.as_ref()?.session.as_ref()?;
        let (app, title, artist) = cc_model::media_lines(s, words);
        let secondary = look.pal.text_secondary().hsla();
        let art = match &self.art {
            Some((_, img)) => gpui_kit::img(img.clone()).size(px(56.)).rounded(px(10.)).into_any_element(),
            None => div()
                .size(px(56.))
                .rounded(px(10.))
                .bg(look.pal.placeholder_fill().hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(Icon::new(L::Music).size(px(24.)).text_color(secondary))
                .into_any_element(),
        };
        let button = |id: &'static str, icon: L, on: bool, t: Transport, tip: &'static str, cx: &mut Context<Self>| {
            let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
            let c = if on { look.pal.text().hsla() } else { secondary.opacity(0.45) };
            let ring = look.accent;
            let focus = self.stop(id, 0, cx);
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_2()
                .border_color(transparent_black())
                // (a button that cannot be pressed is not a keyboard stop either)
                .when(on, |d| d.track_focus(&focus).focus_visible(move |s| s.border_color(ring)).tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx)))
                .when(on, |d| d.cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(pressed)))
                .child(Icon::new(icon).size(px(16.)).text_color(c))
                .when(on, |d| d.on_click(cx.listener(move |this, _, _, _| this.transport(t))))
        };
        Some(
            self.card(look)
                .h(px(cc_model::MEDIA_H))
                .flex()
                .items_center()
                .px(px(12.))
                .gap(px(12.))
                .child(art)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(div().truncate().text_xs().text_color(secondary).child(app))
                        .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(div().truncate().text_xs().text_color(secondary).child(artist)),
                )
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .child(button("cc-prev", L::SkipBack, s.can_previous, Transport::Previous, t!("Önceki"), cx))
                        .child(button("cc-play", if s.playing { L::Pause } else { L::Play }, s.can_toggle, Transport::PlayPause, if s.playing { t!("Duraklat") } else { t!("Oynat") }, cx))
                        .child(button("cc-next", L::SkipForward, s.can_next, Transport::Next, t!("Sonraki"), cx)),
                )
                .into_any_element(),
        )
    }

    /// A liquid slider: the app's own control (`liquid::slider_parts`), driven by the pointer.
    fn slider(&self, which: usize, frac: f32, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let group: SharedString = format!("cc-slider-{which}").into();
        let dragging = self.drag == Some(which);
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let probe = bounds.clone();
        let (b_down, b_drag) = (bounds.clone(), bounds);
        let (track, lane) = crate::liquid::slider_parts(which + 40, group.clone(), &look.theme, cc_model::SLIDER_W - 2.0 * crate::liquid::SL_BORDER, frac, dragging);
        let focus = self.stop("cc-slider", which, cx);
        let ring = look.accent;
        div()
            .id(("cc-slider", which))
            .group(group)
            .relative()
            .w(px(cc_model::SLIDER_W))
            .h(px(crate::liquid::SL_BOX_H))
            .flex_shrink_0()
            .cursor_pointer()
            .rounded(px(12.))
            .border_1()
            .border_color(transparent_black())
            .track_focus(&focus)
            .focus_visible(move |s| s.border_color(ring))
            .on_key_down(cx.listener(move |this, ev: &KeyDownEvent, _, cx| {
                let Some((frac, parts)) = this.slider_state(which, cx) else { return };
                if let Some(f) = cc_model::slider_key(frac, ev.keystroke.key.as_str(), ev.keystroke.modifiers.shift, parts) {
                    this.drag_to(which, f, cx);
                    cx.stop_propagation();
                }
            }))
            // (the probe records where the slider is: the mouse events do not carry the element's box)
            .child(canvas(move |b, _, _| probe.set(b), |_, _, _, _| {}).absolute().top_0().left_0().size_full())
            .child(track)
            .child(lane)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                    let b = b_down.get();
                    this.drag = Some(which);
                    let f = cc_model::slider_fraction_at(f32::from(e.position.x), f32::from(b.left()), f32::from(b.size.width));
                    this.drag_to(which, f, cx);
                    cx.stop_propagation();
                }),
            )
            .on_drag(CcDrag(which), |_, _, _, cx| cx.new(|_| Ghost))
            .on_drag_move(cx.listener(move |this, e: &DragMoveEvent<CcDrag>, _, cx| {
                if e.drag(cx).0 == which {
                    this.drag = Some(which);
                    let b = b_drag.get();
                    let f = cc_model::slider_fraction_at(f32::from(e.event.position.x), f32::from(b.left()), f32::from(b.size.width));
                    this.drag_to(which, f, cx);
                }
            }))
            // the thumb goes back to rest when the pointer is let go, on the slider or off it
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, cx| this.drag_ended(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _, _, cx| this.drag_ended(cx)))
            .into_any_element()
    }

    fn drag_ended(&mut self, cx: &mut Context<Self>) {
        if self.drag.take().is_some() {
            cx.notify();
        }
    }

    fn render_sound(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> Option<AnyElement> {
        let a = sysstate::audio(cx).filter(|a| a.available)?;
        let secondary = look.pal.text_secondary().hsla();
        let frac = audio::volume_to_slider(self.vol_drag.unwrap_or(a.volume));
        let (mute_focus, devices_focus) = (self.stop("cc-mute", 0, cx), self.stop("cc-devices", 0, cx));
        let mute = self
            .round_button("cc-mute", cc_model::volume_icon(audio::glyph(&a)), 28.0, false, look, (Some(&mute_focus), Some(if a.muted { t!("Sesi aç") } else { t!("Sessize al") })))
            .on_click(cx.listener(|this, _, _, cx| {
                // (the state this click flips is the one the previous click asked for while Windows has not answered yet)
                let muted = sysstate::audio(cx).is_some_and(|a| a.muted);
                let pending = this.mute_ask.map(|(v, at)| (v, at.elapsed().as_millis() as u64));
                let want = cc_model::mute_target(muted, pending);
                this.mute_ask = Some((want, Instant::now()));
                audio::set_muted(want);
            }));
        // (the list opens ABOVE this card, so the chevron points up while it is closed)
        let chevron = self
            .round_button("cc-devices", if self.devices_open { L::ChevronDown } else { L::ChevronUp }, 28.0, false, look, (Some(&devices_focus), Some(t!("Çıkış aygıtları"))))
            .on_click(cx.listener(|this, _, _, cx| {
                this.devices_open = !this.devices_open;
                this.refit(cx);
                cx.notify();
            }));
        Some(
            self.card(look)
                .px(px(12.))
                .py(px(12.))
                .child(h_flex().h(px(28.)).gap(px(8.)).items_center().child(mute).child(self.slider(SL_VOLUME, frac, look, cx)).child(chevron))
                .child(div().mt(px(4.)).h(px(16.)).pl(px(4.)).truncate().text_xs().text_color(secondary).child(cc_model::sound_device_text(&a, words)))
                .into_any_element(),
        )
    }

    /// The output devices, read-only (the default device cannot be switched through a documented API), and the way to do it: Windows'
    /// own sound page. A card of its own above the sound card: opening it moves nothing that is under the pointer.
    fn render_devices(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.devices_open {
            return None;
        }
        let a = sysstate::audio(cx).filter(|a| a.available)?;
        let secondary = look.pal.text_secondary().hsla();
        let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
        let (ring, focus) = (look.accent, self.stop("cc-sound-settings", 0, cx));
        let mut rows = v_flex();
        for (i, d) in a.devices.iter().enumerate() {
            // (plain lines: they do nothing when clicked, so they do not look like choices)
            rows = rows.child(
                h_flex()
                    .id(("cc-device", i))
                    .h(px(cc_model::DEVICE_ROW_H))
                    .px(px(8.))
                    .gap(px(8.))
                    .items_center()
                    .text_sm()
                    .child(div().w(px(14.)).flex_shrink_0().when(d.default, |dv| dv.child(Icon::new(L::Check).size(px(14.)).text_color(look.pal.text().hsla()))))
                    .child(div().flex_1().min_w_0().truncate().child(d.name.clone())),
            );
        }
        rows = rows.child(
            h_flex()
                .id("cc-sound-settings")
                .h(px(cc_model::DEVICE_ROW_H))
                .px(px(8.))
                .gap(px(8.))
                .items_center()
                .rounded(px(8.))
                .text_sm()
                .cursor_pointer()
                .border_1()
                .border_color(transparent_black())
                .track_focus(&focus)
                .focus_visible(move |s| s.border_color(ring))
                .hover(move |s| s.bg(hover))
                .active(move |s| s.bg(pressed))
                .child(div().w(px(14.)).flex_shrink_0().child(Icon::new(L::Settings).size(px(14.)).text_color(secondary)))
                .child(div().flex_1().min_w_0().truncate().child(t!("Çıkışı değiştir: Ses ayarları")))
                .on_click(cx.listener(|this, _, _, cx| this.open_settings("ms-settings:sound", cx))),
        );
        Some(self.card(look).flex_shrink_0().px(px(12.)).py(px(cc_model::DEVICE_CARD_PAD)).child(rows).into_any_element())
    }

    fn render_brightness(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let b = self.brightness.filter(|b| b.available)?;
        let secondary = look.pal.text_secondary().hsla();
        Some(
            self.card(look)
                .px(px(12.))
                .py(px(12.))
                .child(
                    h_flex()
                        .h(px(28.))
                        .gap(px(8.))
                        .items_center()
                        .child(div().size(px(28.)).flex_shrink_0().flex().items_center().justify_center().child(Icon::new(L::Sun).size(px(16.)).text_color(look.pal.text().hsla())))
                        .child(self.slider(SL_BRIGHTNESS, b.fraction(), look, cx))
                        .child(div().size(px(28.)).flex_shrink_0()),
                )
                .child(div().mt(px(4.)).h(px(16.)).pl(px(4.)).text_xs().text_color(secondary).child(t!("Parlaklık")))
                .into_any_element(),
        )
    }

    fn render_bottom(&self, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.stop("cc-gear", 0, cx);
        h_flex()
            .h(px(cc_model::BOTTOM_H))
            .flex_shrink_0()
            .justify_end()
            .child(
                self.round_button("cc-gear", L::Settings, cc_model::BOTTOM_H, false, look, (Some(&focus), Some(t!("Ayarlar"))))
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings("ms-settings:", cx))),
            )
            .into_any_element()
    }
}

/// The colours of one frame.
struct Look {
    pal: Palette,
    theme: ThemePalette,
    accent: Hsla,
    /// The glyph on a lit button: dark on a light accent, white on a deep one.
    on_accent: Hsla,
}

/// The picture GPUI draws from decoded BGRA pixels (the bytes were cut down on the media thread; this only wraps them).
fn art_image(a: &Artwork) -> Option<Arc<RenderImage>> {
    let buf = image::RgbaImage::from_raw(a.size, a.size, a.bgra.clone())?;
    Some(Arc::new(RenderImage::new(vec![image::Frame::new(buf)])))
}

/// Opens a Windows page (`ms-settings:...`) through the shell, on the one shell thread and from a thread that may wait for it.
fn open_uri(uri: &'static str) {
    std::thread::spawn(move || {
        let ok = rf_win::sta::call(Duration::from_secs(15), move || rf_win::shell::open(Path::new(uri))).and_then(Result::ok);
        if ok.is_none() {
            log::warn!("control centre: cannot open {uri}");
        }
    });
}

impl Render for CcView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let dark = pal.mix < 0.5;
        // the app's own palette for the tone: its accent (lifted on dark, as everywhere in the app) and the text colour that goes on it
        let theme = if dark { ThemePalette::dark() } else { ThemePalette::light() };
        let look = Look { pal, theme, accent: theme.accent, on_accent: theme.accent_text };
        let words = Words::current();
        let fill = glass_watch::panel_fill(dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let frame = match self.anim {
            Some((t, opening)) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
            None => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
        };
        if !frame.done {
            window.request_animation_frame();
        }

        let mut body = v_flex().size_full().p(px(cc_model::PAD)).gap(px(cc_model::GAP)).opacity(frame.opacity).mt(px(frame.drop_px));
        for row in self.render_tiles(&look, words, cx) {
            body = body.child(row);
        }
        body = body
            .children(self.render_media(&look, words, cx))
            .children(self.render_devices(&look, cx))
            .children(self.render_sound(&look, words, cx))
            .children(self.render_brightness(&look, cx))
            .child(self.render_bottom(&look, cx));
        div()
            .id("control-centre")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(look.pal.text().hsla())
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|_, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
                "escape" => {
                    cx.stop_propagation();
                    cx.defer(|cx| super::panel::close(CloseCause::Escape, cx));
                }
                "tab" => {
                    cx.stop_propagation();
                    if ev.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                }
                _ => {}
            }))
            .child(body)
            .into_any_element()
    }
}
