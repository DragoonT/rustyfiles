//! The control centre: the panel that opens above the right island when its network or volume symbol is clicked (`panel.rs` opens,
//! places and closes it; this is what is inside). About 360 logical px wide, as tall as what it shows, on HyperGlass, with the app's
//! liquid controls. Top to bottom, in the layout of the mock-up the user liked:
//!
//! 1. Connectivity tiles, two to a row (a last one alone takes the row): Wi-Fi and Bluetooth on a machine that has the radio, the
//!    network that is not Wi-Fi (the cable) and, on a machine with a battery, the battery. A tile whose source is not on the
//!    machine is left out. THE ROUND BUTTON of the Wi-Fi and the Bluetooth tile switches the radio on and off
//!    (`rf_win::sysstatus::radio`); THE TILE opens its detail: the networks in range (a click connects to one Windows has saved or
//!    to an open one, leaves the connected one, and sends a network that wants a new password to Windows' own page), the paired
//!    Bluetooth devices, the cable's state and address. The battery's tile opens Windows' page.
//! 2. "Now playing", only while the system has a media session: artwork, title, artist, the program, previous / play-pause / next.
//! 3. Sound: a liquid slider for the master volume with a speaker at its left that mutes, the default output device's name under it,
//!    and a chevron that lists the output devices. A click on one makes it Windows' default output
//!    (`rf_win::sysstatus::audio::set_default_output`, which says how); the list ends with a link to Windows' sound settings. A
//!    right click on the island's volume symbol opens the panel with this list open ([`ask_devices`]).
//!
//! AND, since the user asked what the panel still lacked beside Windows' quick settings and macOS's Control Centre and then for all
//! of it ("hepsini ekleyelim. ama isteyen orayı minimalize edebilsin de."): a line at the top that says which programs are using
//! the microphone or the camera now (`rf_win::sysstatus::inuse`; nothing while nobody is); a row of QUICK BUTTONS under the tiles
//! (`cc_model::Quick`: dark mode, keep awake and the power mode switch something themselves, the others open the page or panel of
//! Windows that does); a MICROPHONE card under the sound card (level, mute, and a list of the input devices that opens out like
//! the outputs'). EVERY PART CAN BE HIDDEN (Settings > Dock, `dock_cc_hidden`; the button with the sliders in the bottom row
//! leads there), so the panel can be as small as one card.
//!
//! ONE ROW FOR THE THINGS OF ONE KIND (`cc_model::base` has the user's words): the speaker and the microphone share the sound card's
//! row, the displays share the brightness card's, and in each the one that was pressed has the slider while the others are a round
//! button; they change places on the spring a detail opens with (`cc_model::Swap`). The quick buttons stand in the bottom row,
//! beside the panel's own three, and get smaller when the user shows more of them than fit at full size.
//!
//! A DETAIL TAKES THE PLACE of other cards, it does not make the panel taller (`cc_model::layout` says which and why): the
//! outputs open out upwards from the sound card over the tiles and the media card, a tile's list downwards over the media, sound
//! and brightness cards. One is open at a time; the button that opened it closes it. IT GROWS: the card comes out of the edge
//! that stays (the sound card's top, the tiles' bottom) on a spring (`cc_model::Unfold`), over the cards it covers, which fade
//! under it, and folds back the same way (the user, of a first version that changed at once: "büyümesi animasyonlu değil?? spring
//! animasyonlu olmalı. büyümeli yani."). With the animations setting off it is there at once.
//!
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
//! WRITES. The slider's drag, the speaker, the choice of an output device, a radio's button, the choice of a network, the transport
//! buttons and the brightness slider are the user's own clicks and drags; each call is made from that event and nowhere else.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::dock::panel::{self as geo, CloseCause, PanelId};
use rf_win::sysstatus::Subscription;
use rf_win::sysstatus::audio;
use rf_win::sysstatus::bluetooth::{self, BluetoothSnapshot};
use rf_win::sysstatus::btaudio;
use rf_win::sysstatus::brightness::{self, BrightnessSnapshot};
use rf_win::sysstatus::media::{self, Artwork, Media, MediaSnapshot, Transport};
use rf_win::sysstatus::inuse::{self, InUse};
use rf_win::sysstatus::network::{self, Link};
use rf_win::sysstatus::power::{self, BatterySnapshot};
use rf_win::sysstatus::radio::{self, Kind as RadioKind, Power as RadioPower};

/// What a tile's round button turns on and off: a radio (Wi-Fi, Bluetooth), or the PC's wired card (`rf_win::ethernet`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Switch {
    Radio(RadioKind),
    Wired,
}
use rf_win::sysstatus::toggles::{self, PowerMode};
use rf_win::sysstatus::wifi::{self, Listing, WifiSnapshot};

use super::backdrop::IslandGlass;
use super::cc_model::{self, Area, Detail, Quick, QuickState, Section, Shown, Swap, Tile, Unfold, Words};
use super::look::Palette;
use super::sysstate::{self, SysState};
use crate::glass_watch::{self, PopupGlass};
use crate::t;
use crate::theme::Palette as ThemePalette;

/// The two sliders' ids (for the drag).
const SL_VOLUME: usize = 0;
const SL_MIC: usize = 2;
/// The brightness sliders: this plus the display's place in the list (the primary one is 0).
const SL_SCREEN: usize = 50;
/// The mixer's sliders: this plus the program's place in the list.
const SL_APP: usize = 100;
/// A mixer row: the program's name, then its slider.
const MIXER_NAME_W: f32 = 104.0;
const MIXER_SLIDER_W: f32 = 184.0;
/// How often the line about the microphone and the camera is read again while the panel is open.
const IN_USE_POLL: Duration = Duration::from_secs(3);
/// The corner radius of a card, logical px (the window's own corners are the system's 8; a card sits 12 px inside them).
const CARD_R: f32 = 12.0;
/// How often the Wi-Fi bars are read again while the panel is open (Windows has no event for a signal that only changes).
const WIFI_POLL: Duration = Duration::from_secs(5);

/// A developer's switch, for a look at the panel of a machine that lacks something on a machine that has everything: the parts
/// named in `RUSTYFILES_CC_WITHOUT` (a comma list of media, sound, brightness, wifi, bluetooth, network) are treated as not there.
fn without(part: &str) -> bool {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    NAMES.get_or_init(|| std::env::var("RUSTYFILES_CC_WITHOUT").map(|v| v.split(',').map(|s| s.trim().to_lowercase()).collect()).unwrap_or_default()).iter().any(|p| p == part)
}

/// Whether a part of the panel is hidden: by the user (Settings > Dock: `dock_cc_hidden`, the names of `cc_model::PARTS` and of the
/// quick buttons), or by the developer's switch above.
fn hidden(part: &str, cx: &App) -> bool {
    without(part) || cx.try_global::<crate::settings::Settings>().is_some_and(|s| s.dock_cc_hidden.iter().any(|h| h == part))
}

/// How long a network's row says "connecting" after it was chosen, when Windows never says it is connected.
const JOIN_WAIT: Duration = Duration::from_secs(16);
/// How long a Bluetooth device's row says "connecting" or "disconnecting" after its sound was asked to.
const BT_WAIT: Duration = Duration::from_secs(8);

/// What a click on a tile does: opens its detail in the panel, or a page of Windows.
#[derive(Clone, Copy)]
enum TileAct {
    Detail(Detail),
    Page(&'static str),
}

/// What a click on a row of a detail does.
type RowClick = Rc<dyn Fn(&mut CcView, &mut Window, &mut Context<CcView>)>;

/// What the live sources tell the view.
enum Ev {
    Media(MediaSnapshot),
    Mic(audio::AudioSnapshot),
    /// The programs that play now, with their levels (read while the list of outputs is out).
    Mixer(Vec<rf_win::appvolume::MixerApp>),
    /// What was read off the interface's thread: who uses the microphone and the camera, Windows' dark mode, the power mode.
    Seen(InUse, bool, Option<PowerMode>),
    Wifi(WifiSnapshot),
    Bluetooth(BluetoothSnapshot),
    Battery(BatterySnapshot),
    Brightness(Vec<BrightnessSnapshot>),
}

/// The sources that run while the panel is open. Dropping it stops them.
struct Live {
    media: Media,
    _mic: Subscription,
    wifi: wifi::Wifi,
    _bluetooth: Subscription,
    _battery: Option<Subscription>,
    brightness: brightness::Screens,
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
    /// The default input device (the microphone), as the sound card's snapshot is the default output's.
    mic: Option<audio::AudioSnapshot>,
    in_use: InUse,
    /// Windows' dark mode and power mode as last read (a press on their buttons changes them here at once).
    dark: bool,
    power: Option<PowerMode>,
    wifi: Option<WifiSnapshot>,
    bluetooth: Option<BluetoothSnapshot>,
    battery: Option<BatterySnapshot>,
    /// Every display's brightness, the primary one first (one that does not answer DDC/CI is there too, without a row).
    brightness: Vec<BrightnessSnapshot>,
    /// The decoded artwork turned into a picture GPUI draws, and the number of the artwork it is from.
    art: Option<(u64, Arc<RenderImage>)>,
    live: Option<Live>,
    _tasks: Vec<Task<()>>,

    // the interface
    /// The part that is opened out, or is folding back, one at a time: the outputs above the sound card, or a tile's list under
    /// the tiles. How far it is out is `unfold`.
    detail: Option<Detail>,
    unfold: Unfold,
    /// Which side of the sound card has the slider (0 the speaker, 1 the microphone), and which display of the brightness card
    /// (its place in `brightness`), with the motion while two change places.
    sound_swap: Swap,
    bright_swap: Swap,
    /// The clock the motions are asked with.
    born: Instant,
    /// Draws the frames of a detail's motion (and moves the window where the panel grows with it).
    _unfold: Option<Task<()>>,
    /// What the last press on a radio's button asked for, and when (`cc_model::shown_power`).
    wifi_ask: Option<(bool, Instant)>,
    bt_ask: Option<(bool, Instant)>,
    /// The PC's wired cards (`rf_win::ethernet`, read when the panel opens and after a change), and what a press on the Ethernet
    /// tile's button asked for while Windows asks the user and `netsh` runs.
    wired: Vec<rf_win::ethernet::Adapter>,
    wired_ask: Option<bool>,
    /// The network the user chose to connect to, and when: its row says so until Windows has connected (or `JOIN_WAIT` is over).
    joining: Option<(String, Instant)>,
    /// The network whose password is being asked for (a row with a field under its own), the field, and whether what was typed
    /// last cannot be a password (too short or too long).
    wifi_asking: Option<String>,
    wifi_input: Option<Entity<InputState>>,
    wifi_bad: bool,
    /// The Bluetooth device (its address) whose sound was asked to connect (`true`) or to let go, and when.
    bt_busy: Option<(String, bool, Instant)>,
    /// The slider being dragged right now.
    drag: Option<usize>,
    /// The volume the slider shows while the user drags it and until Windows has answered (the snapshot may lag a moment).
    vol_drag: Option<f32>,
    /// What a wheel turned over a slider left over that is less than a notch (a touchpad's small moves add up).
    wheel_rest: f32,
    /// The same two for the microphone: its level while dragged, and what the last click on it asked for.
    mic_drag: Option<f32>,
    mic_mute_ask: Option<(bool, Instant)>,
    /// The mixer: the programs that play now, and the level a row shows while its slider is dragged (the list is read again only
    /// every few seconds).
    mixer: Vec<rf_win::appvolume::MixerApp>,
    mixer_drag: Option<(String, f32)>,
    /// The height last given to the window (logical px).
    reported: f32,
    _subs: Vec<Subscription2>,
}

/// GPUI's observer handle (named apart from `rf_win::sysstatus::Subscription`).
type Subscription2 = gpui_kit::Subscription;

/// The wish for the list of output devices (a right click on the island's volume symbol), with when it was made: the control centre
/// that opens next, or the one that is open, shows the list. A wish nobody took (the panel could not be opened) is forgotten after
/// `DEVICES_ASK_LIFE`, so that an ordinary opening later does not come with the list.
#[derive(Default)]
struct DevicesAsk(Option<Instant>);

impl gpui::Global for DevicesAsk {}

const DEVICES_ASK_LIFE: std::time::Duration = std::time::Duration::from_secs(2);

/// Asks for the list of output devices: see [`DevicesAsk`]. The caller opens the control centre when it is not open.
pub fn ask_devices(cx: &mut App) {
    cx.set_global(DevicesAsk(Some(Instant::now())));
}

impl CcView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // the sound and the network change under the open panel (the keyboard's volume keys, a cable pulled): redraw
        let mut subs = vec![cx.observe_global::<SysState>(|this, cx| {
            if this.drag != Some(SL_VOLUME) {
                this.vol_drag = None;
            }
            if this.shown {
                this.drop_orphan_detail(cx);
                this.refit(cx);
                cx.notify();
            }
        })];
        // the list of output devices asked for while the panel is open (a right click on the volume symbol): it opens where it is
        subs.push(cx.observe_global::<DevicesAsk>(|this, cx| {
            if this.shown && !this.closing && this.take_devices_ask(cx) {
                this.refit(cx);
                cx.notify();
            }
        }));
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
            mic: None,
            in_use: InUse::default(),
            dark: false,
            power: None,
            wifi: None,
            bluetooth: None,
            battery: None,
            brightness: Vec::new(),
            art: None,
            live: None,
            _tasks: Vec::new(),
            detail: None,
            unfold: Unfold::folded(),
            sound_swap: Swap::at(0),
            bright_swap: Swap::at(0),
            born: Instant::now(),
            _unfold: None,
            wifi_ask: None,
            bt_ask: None,
            wired: Vec::new(),
            wired_ask: None,
            joining: None,
            wifi_asking: None,
            wifi_input: None,
            wifi_bad: false,
            bt_busy: None,
            drag: None,
            vol_drag: None,
            wheel_rest: 0.0,
            mic_drag: None,
            mic_mute_ask: None,
            mixer: Vec::new(),
            mixer_drag: None,
            reported: 0.0,
            _subs: subs,
        }
    }

    /// Takes the wish for the list of output devices, when there is a fresh one: the list is open. True when it was taken.
    fn take_devices_ask(&mut self, cx: &mut Context<Self>) -> bool {
        if !cx.try_global::<DevicesAsk>().is_some_and(|a| a.0.is_some_and(|at| at.elapsed() < DEVICES_ASK_LIFE)) {
            return false;
        }
        cx.set_global(DevicesAsk(None));
        // (asked for from outside: it is there, it does not open out)
        self.detail = Some(Detail::Outputs);
        self.unfold = Unfold::open();
        true
    }

    /// The keyboard stop `(key, i)`: made the first time it is asked for.
    fn stop(&self, key: &'static str, i: usize, cx: &mut Context<Self>) -> FocusHandle {
        self.stops.borrow_mut().entry((key, i)).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    // ------------------------------------------------------------------ what is shown

    /// The tiles this machine gets now (`cc_model::tiles`).
    fn tiles_now(&self, cx: &App) -> Vec<Tile> {
        let net = sysstate::network(cx).filter(|_| !hidden("network", cx));
        let wifi = self.wifi.as_ref().filter(|_| !hidden("wifi", cx));
        let bt = self.bluetooth.as_ref().filter(|_| !hidden("bluetooth", cx));
        let mut tiles = cc_model::tiles(net.as_ref(), wifi, bt, self.battery.as_ref().filter(|_| !hidden("battery", cx)));
        // a PC with a wired card always has the network tile: its button turns the card on and off, also while the PC is on Wi-Fi or
        // the card is off (when it would have no tile of its own)
        if !self.wired.is_empty() && !hidden("network", cx) && !tiles.contains(&Tile::Network) {
            let at = tiles.iter().position(|t| *t == Tile::Battery).unwrap_or(tiles.len());
            tiles.insert(at, Tile::Network);
        }
        tiles
    }

    /// The wired card the Ethernet tile stands for: the first one that is on, else the first one.
    fn wired_card(&self) -> Option<&rf_win::ethernet::Adapter> {
        self.wired.iter().find(|a| a.enabled).or_else(|| self.wired.first())
    }

    /// Reads the wired cards off the interface's thread; the panel is laid out again when they changed.
    fn read_wired(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let cards = cx.background_spawn(async { rf_win::ethernet::adapters() }).await;
            let _ = this.update(cx, |cc, cx| {
                if cc.wired != cards {
                    cc.wired = cards;
                    cc.refit(cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The Ethernet tile's button: the card is turned the other way. Windows asks the user first (UAC); the tile shows what was asked
    /// for until `netsh` is done, then what the card really is.
    fn press_wired(&mut self, cx: &mut Context<Self>) {
        if !self.shown || self.closing || self.wired_ask.is_some() {
            return;
        }
        let Some(card) = self.wired_card().cloned() else { return };
        let want = !card.enabled;
        self.wired_ask = Some(want);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let name = card.name.clone();
            let outcome = cx.background_spawn(async move { rf_win::ethernet::set_enabled(&name, want) }).await;
            if outcome == rf_win::ethernet::Outcome::Failed {
                log::warn!("ethernet: turning {:?} {} failed", card.name, if want { "on" } else { "off" });
            }
            let cards = cx.background_spawn(async { rf_win::ethernet::adapters() }).await;
            let _ = this.update(cx, |cc, cx| {
                cc.wired_ask = None;
                cc.wired = cards;
                cc.refit(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// The sides of the sound card that are there and shown: the speaker (0), the microphone (1).
    fn sound_sides(&self, cx: &App) -> Vec<usize> {
        let speaker = sysstate::audio(cx).is_some_and(|a| a.available) && !hidden("sound", cx);
        let mic = self.mic.as_ref().is_some_and(|m| m.available) && !hidden("mic", cx);
        [speaker.then_some(0), mic.then_some(1)].into_iter().flatten().collect()
    }

    /// The side that has the slider: the one last pressed while it is there, else the one that is.
    fn sound_open(&self, cx: &App) -> Option<usize> {
        let sides = self.sound_sides(cx);
        let want = self.sound_swap.open();
        if sides.contains(&want) { Some(want) } else { sides.first().copied() }
    }

    fn quick_state(&self) -> QuickState {
        QuickState { dark: self.dark, awake: toggles::is_awake(), power: self.power }
    }

    /// The quick buttons the row shows now (`cc_model::quick_buttons`).
    fn quick_now(&self, cx: &App) -> Vec<Quick> {
        cc_model::quick_buttons(&|key| hidden(key, cx), &self.quick_state())
    }

    fn shown_now(&self, cx: &App) -> Shown {
        let net = sysstate::network(cx);
        let audio = sysstate::audio(cx);
        let tiles = self.tiles_now(cx).len();
        let (wifi, bt) = (self.wifi_now(), self.bt_now());
        Shown {
            in_use: self.in_use.any() && !hidden("inuse", cx),
            tiles,
            media: self.media.as_ref().is_some_and(|m| m.session.is_some()) && !hidden("media", cx),
            sound: !self.sound_sides(cx).is_empty(),
            brightness: if hidden("brightness", cx) { 0 } else { self.brightness.iter().filter(|b| b.available).count() },
            // (the mixer's rows are the programs that play now)
            // (and the password's row in Wi-Fi's, while one is asked for)
            detail: self.detail_now().map(|d| {
                let mixer = if d == Detail::Mixer { cc_model::mixer_rows(self.mixer.len()) } else { 0 };
                let asking = usize::from(d == Detail::Wifi && self.wifi_asking.is_some());
                (d, cc_model::detail_rows(d, audio.as_ref(), self.mic.as_ref(), wifi.as_ref(), bt.as_ref(), net.as_ref()) + mixer + asking)
            }),
        }
    }

    fn now_ms(&self) -> u64 {
        self.born.elapsed().as_millis() as u64
    }

    /// The detail that is out or on its way (in or out): `None` once it has folded back.
    fn detail_now(&self) -> Option<Detail> {
        self.detail.filter(|_| self.unfold.opening() || !self.unfold.settled(self.now_ms()))
    }

    /// A detail whose owner has gone while it was out (the Bluetooth adapter was pulled, the last sound device went) goes with it:
    /// it has no button left to fold it back with.
    fn drop_orphan_detail(&mut self, cx: &App) {
        let Some(d) = self.detail else { return };
        let owned = match d {
            Detail::Outputs | Detail::Mixer => self.sound_sides(cx).contains(&0),
            Detail::Inputs => self.sound_sides(cx).contains(&1),
            _ => self.tiles_now(cx).into_iter().any(|t| cc_model::detail_of(t) == Some(d)),
        };
        if !owned {
            self.detail = None;
            self.unfold = Unfold::folded();
            self._unfold = None;
        }
    }

    /// That detail is open, or opening (what its button shows).
    fn is_open(&self, d: Detail) -> bool {
        self.detail == Some(d) && self.unfold.opening()
    }

    /// Wi-Fi as it is shown: the radio's state is what the user has just pressed for while Windows has not yet said so.
    fn wifi_now(&self) -> Option<WifiSnapshot> {
        let mut w = self.wifi.clone()?;
        w.power = cc_model::shown_power(w.power, self.wifi_ask.map(|(on, at)| (on, at.elapsed().as_millis() as u64)));
        Some(w)
    }

    /// Bluetooth as it is shown (see `wifi_now`).
    fn bt_now(&self) -> Option<BluetoothSnapshot> {
        let mut b = self.bluetooth.clone()?;
        b.power = cc_model::shown_power(b.power, self.bt_ask.map(|(on, at)| (on, at.elapsed().as_millis() as u64)));
        Some(b)
    }

    /// The window's height now, logical px (it follows a detail that has to make the panel taller while it opens out).
    pub fn height(&self, cx: &App) -> f32 {
        cc_model::height_at(&self.shown_now(cx), self.unfold.value(self.now_ms()))
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
        self.detail = None;
        self.unfold = Unfold::folded();
        self._unfold = None;
        self.wifi_ask = None;
        self.bt_ask = None;
        self.read_wired(cx);
        self.joining = None;
        self.wifi_asking = None;
        self.wifi_bad = false;
        self.bt_busy = None;
        self.take_devices_ask(cx);
        self.drag = None;
        self.vol_drag = None;
        self.mute_ask = None;
        self.mic_drag = None;
        self.mic_mute_ask = None;
        self.reported = self.height(cx);
        self.start_live(cx);
        cx.notify();
    }

    /// The button was pressed again while the panel was shrinking back into its island (`grow_host.rs`): the motion turns round and the
    /// panel is open again AS IT WAS (its sources never stopped), not as a new one.
    pub fn on_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = None;
        if self.take_devices_ask(cx) {
            self.refit(cx);
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// A click on an output device of the list: it becomes Windows' default output (what plays; the device for calls is left as it
    /// is). The list stays open: the check mark moves when Windows has made the change.
    fn choose_output(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        audio::set_default_output(id);
        // (the row that was pressed is about to be the default one, which is no stop: the keyboard goes to the list's own button)
        let stop = self.stop("cc-devices", 0, cx);
        window.focus(&stop, cx);
    }

    /// The window is on the screen: the glass goes on, the keyboard goes to the panel.
    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            // an island growing into this panel: its glass is a shape in the window that the growth moves (grow_host.rs)
            self.popup = Some(match super::grow_host::shape_for_attach(PanelId::CONTROL_CENTRE, cx) {
                Some((shape, radius)) => PopupGlass::start_panel_shaped(self.hwnd, dark, shape, radius),
                None => PopupGlass::start_panel(self.hwnd, dark),
            });
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// The panel is being put away: ease out.
    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        self.detail = None;
        self.unfold = Unfold::folded();
        self._unfold = None;
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

    /// Opens a detail out, or folds it back when it is the open one. (A detail takes the place of other cards and grows out of the
    /// edge that stays: `cc_model::layout`.)
    fn toggle_detail(&mut self, kind: Detail, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        let (now, animate) = (self.now_ms(), crate::settings::animations());
        self.wifi_asking = None;
        match self.detail_now() {
            // the one that is out folds back; one that is folding back and is pressed again turns round
            Some(d) if d == kind => self.unfold.go(!self.unfold.opening(), now, animate),
            // another tile's list is out in the same place: the list changes, the place stays
            Some(d) if d != Detail::Outputs && kind != Detail::Outputs && d != Detail::Mixer && kind != Detail::Mixer => {
                self.detail = Some(kind);
                self.unfold.go(true, now, animate);
            }
            _ => {
                self.detail = Some(kind);
                self.unfold = Unfold::folded();
                self.unfold.go(true, now, animate);
            }
        }
        // the list of networks is asked for afresh when it is opened
        if self.is_open(Detail::Wifi)
            && let Some(live) = &self.live
        {
            live.wifi.scan();
        }
        // and the mixer is read when it opens out
        if self.is_open(Detail::Mixer) {
            self.read_mixer(cx);
        }
        self.run_unfold(cx);
        self.refit(cx);
        cx.notify();
    }

    /// Reads the mixer off this thread (the audio service is asked across processes) and takes it when it has come.
    fn read_mixer(&self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let mixer = cx.background_executor().spawn(async { rf_win::appvolume::mixer() }).await;
            let _ = this.update(cx, |me, cx| me.apply(Ev::Mixer(mixer), cx));
        })
        .detach();
    }

    /// Draws the frames of the detail's motion until it has come to rest (about 7 ms apart: every frame of a fast display), and
    /// lets the window follow where the panel grows with the card.
    fn run_unfold(&mut self, cx: &mut Context<Self>) {
        self._unfold = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_millis(7)).await;
                let Ok(done) = this.update(cx, |me, cx| {
                    let now = me.now_ms();
                    if me.unfold.settled(now) && !me.unfold.opening() {
                        me.detail = None;
                    }
                    // (the same frames draw the rows whose items are changing places)
                    let done = me.unfold.settled(now) && me.sound_swap.settled(now) && me.bright_swap.settled(now);
                    me.refit(cx);
                    cx.notify();
                    done
                }) else {
                    break;
                };
                if done {
                    break;
                }
            }
        }));
    }

    /// Draws again after `after` (something shown for a while is due to end: a pressed radio's state, a row's "connecting").
    fn redraw_in(&self, after: Duration, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(after).await;
            let _ = this.update(cx, |me, cx| {
                me.refit(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// A press on a radio's button (the tile's round one, or the switch in its detail): asks Windows for the other state. The
    /// button shows it at once and Windows' own word follows (`cc_model::shown_power`).
    fn press_radio(&mut self, kind: RadioKind, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        let now = match kind {
            RadioKind::WiFi => self.wifi_now().map(|w| w.power),
            RadioKind::Bluetooth => self.bt_now().map(|b| b.power),
        };
        let Some(want) = now.and_then(radio::toggled) else { return };
        radio::set(kind, want);
        let ask = Some((want, Instant::now()));
        match kind {
            RadioKind::WiFi => self.wifi_ask = ask,
            RadioKind::Bluetooth => self.bt_ask = ask,
        }
        self.redraw_in(Duration::from_millis(cc_model::RADIO_PENDING_MS + 100), cx);
        self.refit(cx);
        cx.notify();
    }

    /// A click on a network of the list: the connected one is left; one Windows can connect by itself (saved, or open) is
    /// connected; one that wants a password gets a field under its row when it is a home network (`Network::takes_typed_password`),
    /// and goes to Windows' own Wi-Fi page when it is another kind.
    fn choose_network(&mut self, ssid: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        let Some(n) = self.wifi.as_ref().and_then(|w| w.networks.iter().find(|n| n.ssid == ssid)).cloned() else { return };
        if n.connected {
            self.joining = None;
            self.wifi_asking = None;
            wifi::disconnect();
        } else if wifi::way_to(&n).is_some() {
            wifi::connect(ssid);
            self.wifi_asking = None;
            self.joining = Some((ssid.to_string(), Instant::now()));
            self.redraw_in(JOIN_WAIT + Duration::from_millis(100), cx);
        } else if n.takes_typed_password() {
            // (pressed again: the field goes)
            if self.wifi_asking.as_deref() == Some(ssid) {
                self.wifi_asking = None;
            } else {
                let input = match &self.wifi_input {
                    Some(input) => input.clone(),
                    None => {
                        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Şifre")).masked(true));
                        self._subs.push(cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, window, cx| match ev {
                            InputEvent::PressEnter { .. } => this.submit_wifi_password(window, cx),
                            InputEvent::Change => {
                                if this.wifi_bad {
                                    this.wifi_bad = false;
                                    cx.notify();
                                }
                            }
                            _ => {}
                        }));
                        self.wifi_input = Some(input.clone());
                        input
                    }
                };
                input.update(cx, |s, cx| {
                    s.set_value("", window, cx);
                    s.focus(window, cx);
                });
                self.wifi_asking = Some(ssid.to_string());
                self.wifi_bad = false;
            }
            self.refit(cx);
        } else {
            return self.open_settings("ms-settings:network-wifi", cx);
        }
        cx.notify();
    }

    /// Enter in the password's field, or its button: Windows is asked to connect with what was typed (`wifi::connect_with_password`
    /// says what happens to a wrong one). Something that cannot be a password stays in the field, marked.
    fn submit_wifi_password(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        let (Some(ssid), Some(input)) = (self.wifi_asking.clone(), self.wifi_input.clone()) else { return };
        let password = input.read(cx).value().to_string();
        if !wifi::valid_password(&password) {
            self.wifi_bad = true;
            return cx.notify();
        }
        wifi::connect_with_password(&ssid, &password);
        input.update(cx, |s, cx| s.set_value("", window, cx));
        self.wifi_asking = None;
        self.joining = Some((ssid, Instant::now()));
        self.redraw_in(JOIN_WAIT + Duration::from_millis(100), cx);
        window.focus(&self.focus, cx);
        self.refit(cx);
        cx.notify();
    }

    /// A click on a headset or a speaker of the Bluetooth list: its sound is connected, or let go (`btaudio::set_connected`).
    fn choose_bt(&mut self, address: &str, connected: bool, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        let (address, connect) = (address.to_string(), !connected);
        let asked = address.clone();
        std::thread::spawn(move || {
            if let Err(e) = btaudio::set_connected(&asked, connect) {
                log::warn!("control centre: the Bluetooth device's sound was not {}: {e}", if connect { "connected" } else { "let go" });
            }
        });
        self.bt_busy = Some((address, connect, Instant::now()));
        self.redraw_in(BT_WAIT + Duration::from_millis(100), cx);
        cx.notify();
    }

    /// Starts what runs while the panel is open: the media session, Wi-Fi, Bluetooth, the battery (if there is one), the brightness,
    /// and the 5 s look at the Wi-Fi bars.
    fn start_live(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Ev>();
        let t_mic = tx.clone();
        let mic = audio::subscribe_input(move |s| {
            let _ = t_mic.unbounded_send(Ev::Mic(s.clone()));
        });
        let (t_media, t_wifi, t_bt, t_bat, t_br) = (tx.clone(), tx.clone(), tx.clone(), tx.clone(), tx);
        let wifi = wifi::subscribe(move |s| {
            let _ = t_wifi.unbounded_send(Ev::Wifi(s.clone()));
        });
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
        let brightness = brightness::open_all(move |list| {
            let _ = t_br.unbounded_send(Ev::Brightness(list.to_vec()));
        });
        self.live = Some(Live { media, _mic: mic, wifi, _bluetooth: bluetooth, _battery: battery, brightness });
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
        // who uses the microphone and the camera, Windows' dark mode and the power mode: read off this thread now, and the first
        // of them again every few seconds while the panel is open
        let seen = cx.spawn(async move |this, cx| {
            loop {
                let read = cx.background_executor().spawn(async { (inuse::read(), toggles::is_dark(), toggles::power_mode()) }).await;
                if this.update(cx, |me, cx| me.apply(Ev::Seen(read.0, read.1, read.2), cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(IN_USE_POLL).await;
                // (the mixer too, while the list of outputs is out: a program that began to play gets its row)
                let Ok(shown) = this.update(cx, |me, cx| {
                    if me.shown && me.is_open(Detail::Mixer) {
                        me.read_mixer(cx);
                    }
                    me.shown
                }) else {
                    break;
                };
                if !shown {
                    break;
                }
            }
        });
        // the track's time and its line move on while it plays: one look a second, and only then
        let tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let Ok(shown) = this.update(cx, |me, cx| {
                    if me.shown && me.media.as_ref().is_some_and(|m| m.timeline.is_some() && m.session.as_ref().is_some_and(|s| s.playing)) {
                        cx.notify();
                    }
                    me.shown
                }) else {
                    break;
                };
                if !shown {
                    break;
                }
            }
        });
        self._tasks = vec![events, poll, seen, tick];
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
            Ev::Mic(m) => {
                if self.drag != Some(SL_MIC) {
                    self.mic_drag = None;
                }
                self.mic = Some(m);
            }
            Ev::Mixer(mixer) => {
                // (a level being dragged is not taken back by an older reading)
                if !self.drag.is_some_and(|d| d >= SL_APP) {
                    self.mixer_drag = None;
                    self.mixer = mixer;
                }
            }
            Ev::Seen(using, dark, power) => {
                self.in_use = using;
                self.dark = dark;
                self.power = power;
            }
            // (Windows has said what was pressed for: the pressed state has done its work)
            Ev::Wifi(w) => {
                if self.wifi_ask.is_some_and(|(on, _)| (w.power == RadioPower::On) == on) {
                    self.wifi_ask = None;
                }
                if self.joining.as_ref().is_some_and(|(ssid, _)| w.connected().is_some_and(|n| n.ssid == *ssid)) {
                    self.joining = None;
                }
                self.wifi = Some(w);
            }
            Ev::Bluetooth(b) => {
                if self.bt_ask.is_some_and(|(on, _)| (b.power == RadioPower::On) == on) {
                    self.bt_ask = None;
                }
                self.bluetooth = Some(b);
            }
            Ev::Battery(b) => self.battery = Some(b),
            // a value the user has just dragged to is not taken back by the monitor's old answer
            // (DDC/CI misses a transfer now and then even after the retries: a monitor that answered before and does not now keeps its
            // row rather than losing it for one opening)
            Ev::Brightness(list) => {
                for (i, b) in list.into_iter().enumerate() {
                    let old = self.brightness.get(i).copied();
                    if self.drag != Some(SL_SCREEN + i) && (b.available || !old.is_some_and(|o| o.available)) {
                        if i < self.brightness.len() {
                            self.brightness[i] = b;
                        } else {
                            self.brightness.push(b);
                        }
                    }
                }
            }
        }
        self.drop_orphan_detail(cx);
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

    /// A press on a quick button: the three that switch something do it (and show it at once); the others open their page or
    /// panel of Windows, and the panel goes.
    fn press_quick(&mut self, q: Quick, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        match q {
            Quick::Dark => {
                self.dark = !self.dark;
                toggles::set_dark(self.dark);
            }
            Quick::Awake => toggles::set_awake(!toggles::is_awake()),
            Quick::Power => {
                if let Some(next) = self.power.map(toggles::next_mode)
                    && toggles::set_power_mode(next)
                {
                    self.power = Some(next);
                }
            }
            // (Windows' own "project" panel is a program of Windows: started, and ours gets out of its way)
            Quick::Project => {
                self.closing = true;
                std::thread::spawn(|| {
                    if let Err(e) = std::process::Command::new("DisplaySwitch.exe").spawn() {
                        log::warn!("control centre: Windows' project panel did not start: {e}");
                    }
                });
                cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
                return;
            }
            Quick::NightLight | Quick::Focus | Quick::Airplane | Quick::Cast => {
                if let Some(page) = q.page() {
                    self.open_settings(page, cx);
                }
                return;
            }
        }
        cx.notify();
    }

    /// The button with the sliders: Settings > Dock, where the panel's parts are shown and hidden. The panel goes.
    fn open_edit(&mut self, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.closing = true;
        super::actions::run_rustyfiles(vec!["--settings=dock".to_string()]);
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    /// A press on the round button of a side of the sound card. The side that has the slider: its button mutes it. The other: it
    /// takes the slider (and a list of devices that is out turns to its devices).
    fn press_sound_head(&mut self, side: usize, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        if self.sound_open(cx) != Some(side) {
            self.sound_swap.go(side, self.now_ms(), crate::settings::animations());
            // (a list of devices that is out turns to the other side's devices; the mixer, which is no side's list, stays)
            if self.detail.is_some_and(|d| d.upwards() && d != Detail::Mixer) && self.unfold.opening() {
                self.detail = Some(if side == 0 { Detail::Outputs } else { Detail::Inputs });
            }
            self.run_unfold(cx);
            self.refit(cx);
            return cx.notify();
        }
        // (the state a click flips is the one the previous click asked for while Windows has not answered yet: `mute_target`)
        if side == 0 {
            let muted = sysstate::audio(cx).is_some_and(|a| a.muted);
            let want = cc_model::mute_target(muted, self.mute_ask.map(|(v, at)| (v, at.elapsed().as_millis() as u64)));
            self.mute_ask = Some((want, Instant::now()));
            audio::set_muted(want);
        } else {
            let muted = self.mic.as_ref().is_some_and(|a| a.muted);
            let want = cc_model::mute_target(muted, self.mic_mute_ask.map(|(v, at)| (v, at.elapsed().as_millis() as u64)));
            self.mic_mute_ask = Some((want, Instant::now()));
            audio::set_input_muted(want);
        }
    }

    /// A press on a display's number in the brightness card: that display takes the slider.
    fn press_screen(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.bright_swap.go(index, self.now_ms(), crate::settings::animations());
        self.run_unfold(cx);
        cx.notify();
    }

    /// A click on an input device of the list: it becomes Windows' default input (see `choose_output`).
    fn choose_input(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        audio::set_default_input(id);
        let stop = self.stop("cc-devices", 0, cx);
        window.focus(&stop, cx);
    }

    /// Opens an address (the modem's own pages) with the program Windows opens such with, and puts the panel away.
    fn open_link(&mut self, url: String, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.closing = true;
        std::thread::spawn(move || {
            let shown = url.clone();
            let ok = rf_win::sta::call(Duration::from_secs(15), move || rf_win::shell::open(Path::new(&url))).and_then(Result::ok);
            if ok.is_none() {
                log::warn!("control centre: cannot open {shown}");
            }
        });
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    /// Puts the panel away and starts the screen snip once it is gone (the panel eases out in about 120 ms and must not be in the picture).
    fn start_snip(&mut self, cx: &mut Context<Self>) {
        if !self.shown || self.closing {
            return;
        }
        self.closing = true;
        cx.defer(|cx| {
            super::panel::close(CloseCause::Chosen, cx);
            crate::snip::start_after(Duration::from_millis(350), cx);
        });
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
            app if app >= SL_APP => {
                if let Some(name) = self.mixer.get(app - SL_APP).map(|a| a.name.clone()) {
                    let level = fraction.clamp(0.0, 1.0);
                    rf_win::appvolume::set_mixer_level(&name, level);
                    self.mixer_drag = Some((name, level));
                }
            }
            SL_MIC => {
                let steps = self.mic.as_ref().map_or(0, |a| a.steps);
                let v = audio::slider_to_volume(fraction, steps);
                if self.mic_drag != Some(v) {
                    self.mic_drag = Some(v);
                    audio::set_input_volume(fraction);
                }
            }
            screen if (SL_SCREEN..SL_APP).contains(&screen) => {
                let index = screen - SL_SCREEN;
                if let (Some(b), Some(live)) = (self.brightness.get_mut(index), self.live.as_ref()) {
                    let raw = b.value_at(fraction);
                    if raw != b.value {
                        b.value = raw;
                        live.brightness.set(index, raw);
                    }
                }
            }
            _ => {}
        }
        cx.notify();
    }

    /// The wheel turned over a slider: one part of it per notch, up raises (issue 216), as the arrow keys do.
    fn slider_wheel(&mut self, which: usize, ev: &ScrollEvent, cx: &mut Context<Self>) {
        let lines = match ev.delta {
            ScrollDelta::Lines(p) => if p.y != 0.0 { p.y } else { p.x },
            ScrollDelta::Pixels(p) => f32::from(p.y) / 16.0,
        };
        let notches = rf_win::appvolume::take_notches(&mut self.wheel_rest, rf_win::appvolume::notches_of_lines(lines, rf_win::appvolume::wheel_lines()));
        cx.stop_propagation();
        if notches == 0 {
            return;
        }
        let Some((mut frac, parts)) = self.slider_state(which, cx) else { return };
        let key = if notches > 0 { "up" } else { "down" };
        for _ in 0..notches.unsigned_abs() {
            frac = cc_model::slider_key(frac, key, false, parts).unwrap_or(frac);
        }
        self.drag_to(which, frac, cx);
    }

    /// A slider's share now and how many parts it has, for the keyboard.
    fn slider_state(&self, which: usize, cx: &App) -> Option<(f32, u32)> {
        match which {
            SL_VOLUME => {
                let a = sysstate::audio(cx)?;
                Some((audio::volume_to_slider(self.vol_drag.unwrap_or(a.volume)), cc_model::slider_parts(a.steps)))
            }
            SL_MIC => self.mic.as_ref().map(|a| (audio::volume_to_slider(self.mic_drag.unwrap_or(a.volume)), cc_model::slider_parts(a.steps))),
            app if app >= SL_APP => self.mixer.get(app - SL_APP).map(|a| (self.mixer_level(a), 20)),
            screen if (SL_SCREEN..SL_APP).contains(&screen) => self.brightness.get(screen - SL_SCREEN).filter(|b| b.available).map(|b| (b.fraction(), 20)),
            _ => None,
        }
    }

    /// A click on the source's line: the card turns to the next program that plays.
    fn next_source(&self) {
        if let Some(live) = &self.live {
            live.media.next_source();
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

    /// One tile. Its round button is the radio's switch when the tile has a radio (`radio`), else only the tile's picture; the tile
    /// itself opens its detail or a page of Windows (`act`). Two keyboard stops when there is a switch: the tile, then the button.
    ///
    /// `tag` is the version that belongs to the name: "Wi-Fi 7", "Bluetooth 5.4" are what these are called. It is set as part of the
    /// name, in the same size and a lighter weight and colour, so that the name reads first and the number as its qualifier. It never
    /// gives way: when the two do not fit, the name is the one cut short. (On 2026-11-26 it stood small in the round button's corner
    /// first, at the owner's word; at 8.5 px it was hard to read on the accent, and the glyph had to shrink off centre to make room.
    /// The owner then asked for a better place: "bence sürüm yerlerini daha iyi yapabiliriz".)
    #[allow(clippy::too_many_arguments)]
    fn tile(&self, id: &'static str, icon: L, text: cc_model::TileText, tag: Option<String>, look: &Look, act: TileAct, radio: Option<Switch>, cx: &mut Context<Self>) -> AnyElement {
        let pressed = look.pal.pressed_fill().hsla();
        let ring = look.accent;
        let secondary = look.pal.text_secondary().hsla();
        let focus = self.stop(id, 0, cx);
        let open = matches!(act, TileAct::Detail(d) if self.is_open(d));
        let button = match radio {
            Some(kind) => {
                let stop = self.stop(id, 1, cx);
                let tip = if text.active { t!("Kapat") } else { t!("Aç") };
                self.round_button((id, 1usize), icon, 40.0, text.active, look, (Some(&stop), Some(tip))).on_click(cx.listener(move |this, _, _, cx| {
                    // (the press is the button's: the tile under it does not open)
                    cx.stop_propagation();
                    match kind {
                        Switch::Radio(kind) => this.press_radio(kind, cx),
                        Switch::Wired => this.press_wired(cx),
                    }
                }))
            }
            None => self.round_button((id, 1usize), icon, 40.0, text.active, look, (None, None)),
        };
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
            .border_color(if open { ring } else { transparent_black() })
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
                    .child(
                        h_flex()
                            .items_baseline()
                            .gap(px(5.))
                            .min_w_0()
                            .text_sm()
                            .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(text.title))
                            .children(tag.map(|t| div().flex_shrink_0().font_weight(FontWeight::NORMAL).text_color(secondary).child(t))),
                    )
                    .child(div().truncate().text_xs().text_color(secondary).child(text.subtitle)),
            )
            // (a tile that opens out says so, and which way it is now)
            .when(matches!(act, TileAct::Detail(_)), |d| d.child(Icon::new(if open { L::ChevronUp } else { L::ChevronDown }).size(px(14.)).flex_shrink_0().text_color(secondary)))
            .on_click(cx.listener(move |this, _, _, cx| match act {
                TileAct::Detail(d) => this.toggle_detail(d, cx),
                TileAct::Page(uri) => this.open_settings(uri, cx),
            }))
            .into_any_element()
    }

    fn render_tiles(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let net = sysstate::network(cx);
        let tiles = self.tiles_now(cx);
        let mut items: Vec<AnyElement> = Vec::new();
        for t in tiles {
            let act = cc_model::detail_of(t).map_or(TileAct::Page("ms-settings:powersleep"), TileAct::Detail);
            items.push(match t {
                Tile::Wifi => {
                    let w = self.wifi_now().unwrap_or_default();
                    self.tile("cc-wifi", cc_model::wifi_icon(&w), cc_model::wifi_text(&w, net.as_ref(), words), cc_model::wifi_tag(&w), look, act, Some(Switch::Radio(RadioKind::WiFi)), cx)
                }
                Tile::Network => match self.wired_card() {
                    // a wired card: the Ethernet tile, whose button turns it on and off
                    Some(card) => {
                        let on = self.wired_ask.unwrap_or(card.enabled);
                        let text = cc_model::wired_text(on, net.as_ref(), words);
                        self.tile("cc-network", L::EthernetPort, text, None, look, act, Some(Switch::Wired), cx)
                    }
                    None => {
                        let n = net.clone().unwrap_or_default();
                        self.tile("cc-network", cc_model::network_icon(network::glyph(&n)), cc_model::network_text(&n, words), None, look, act, None, cx)
                    }
                },
                Tile::Bluetooth => {
                    let b = self.bt_now().unwrap_or_default();
                    self.tile("cc-bluetooth", cc_model::bluetooth_icon(&b), cc_model::bluetooth_text(&b, words), cc_model::bluetooth_tag(&b), look, act, Some(Switch::Radio(RadioKind::Bluetooth)), cx)
                }
                Tile::Battery => {
                    let b = self.battery.unwrap_or_default();
                    self.tile("cc-battery", cc_model::battery_icon(&b), cc_model::battery_text(&b, words), None, look, act, None, cx)
                }
            });
        }
        // two to a row; a last one alone takes the row
        let mut rows = Vec::new();
        let mut it = items.into_iter();
        while let Some(first) = it.next() {
            rows.push(h_flex().flex_shrink_0().gap(px(cc_model::GAP)).child(first).children(it.next()).into_any_element());
        }
        rows
    }

    fn render_media(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> Option<AnyElement> {
        let media = self.media.as_ref()?;
        let s = media.session.as_ref()?;
        let (app, title, artist) = cc_model::media_lines(s, words);
        let secondary = look.pal.text_secondary().hsla();
        let now = Instant::now();
        // more than one program plays: the first line says which this is, and a click on it turns to the next
        let several = media.sources > 1;
        let source = div().id("cc-source").truncate().text_xs().text_color(secondary).child(cc_model::media_source_line(&app, media.source, media.sources)).when(several, |d| {
            let (stop, ring, hover) = (self.stop("cc-source", 0, cx), look.accent, look.pal.text().hsla());
            d.cursor_pointer()
                .track_focus(&stop)
                .rounded(px(4.))
                .border_1()
                .border_color(transparent_black())
                .focus_visible(move |s| s.border_color(ring))
                .hover(move |s| s.text_color(hover))
                .tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Sonraki kaynak")).build(w, cx))
                .on_click(cx.listener(|this, _, _, _| this.next_source()))
        });
        let time = media.timeline.map(|t| cc_model::media_time(&t, s.playing, now));
        let progress = media.timeline.map(|t| {
            let (track, fill) = (look.pal.hover_fill().hsla(), look.accent);
            div()
                .absolute()
                .left(px(12.))
                .right(px(12.))
                .bottom(px(5.))
                .h(px(3.))
                .rounded_full()
                .bg(track)
                .child(div().h_full().w(relative(t.fraction_at(now, s.playing).clamp(0.0, 1.0))).rounded_full().bg(fill))
        });
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
                .relative()
                .h(px(cc_model::MEDIA_H))
                .flex()
                .items_center()
                .px(px(12.))
                .gap(px(12.))
                .children(progress)
                .child(art)
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(1.))
                        .child(source)
                        .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(
                            h_flex()
                                .gap(px(6.))
                                .text_xs()
                                .text_color(secondary)
                                .child(div().flex_1().min_w_0().truncate().child(artist))
                                .children(time.map(|t| div().flex_shrink_0().child(t))),
                        ),
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


    /// The level a mixer row shows: the one being dragged to, else the program's own.
    fn mixer_level(&self, app: &rf_win::appvolume::MixerApp) -> f32 {
        self.mixer_drag.as_ref().filter(|(name, _)| *name == app.name).map_or(app.level.volume, |(_, v)| *v)
    }

    /// A slider `width` wide (the cards' are `SLIDER_W`; a mixer row's is narrower).
    fn slider_of(&self, which: usize, frac: f32, width: f32, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let group: SharedString = format!("cc-slider-{which}").into();
        let dragging = self.drag == Some(which);
        let bounds = Rc::new(Cell::new(Bounds::<Pixels>::default()));
        let probe = bounds.clone();
        let (b_down, b_drag) = (bounds.clone(), bounds);
        let (track, lane) = crate::liquid::slider_parts(which + 40, group.clone(), &look.theme, width - 2.0 * crate::liquid::SL_BORDER, frac, dragging);
        let focus = self.stop("cc-slider", which, cx);
        let ring = look.accent;
        div()
            .id(("cc-slider", which))
            .group(group)
            .relative()
            .w(px(width))
            .h(px(crate::liquid::SL_BOX_H))
            .flex_shrink_0()
            .cursor_pointer()
            .rounded(px(12.))
            .border_1()
            .border_color(transparent_black())
            .track_focus(&focus)
            .focus_visible(move |s| s.border_color(ring))
            .on_scroll(cx.listener(move |this, ev: &ScrollEvent, _, cx| this.slider_wheel(which, ev, cx)))
            // its level in per cent (issue 216)
            .tooltip({
                // (read as it is drawn: it follows the wheel and a drag while it is up)
                let me = cx.entity().downgrade();
                move |w, cx| {
                    let me = me.clone();
                    gpui_kit::component::tooltip::Tooltip::element(move |_, cx| {
                        let now = me.upgrade().and_then(|e| e.read(cx).slider_state(which, cx)).map_or(frac, |(f, _)| f);
                        format!("{} %", (now * 100.0).round() as i32)
                    })
                    .build(w, cx)
                }
            })
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
        let sides = self.sound_sides(cx);
        let open = self.sound_open(cx)?;
        let now = self.now_ms();
        let secondary = look.pal.text_secondary().hsla();
        let speaker = sysstate::audio(cx).unwrap_or_default();
        let mic = self.mic.clone().unwrap_or_default();
        // The row: each side is its round button, and the one that has the slider has it after the button. The two change places
        // by their widths (`cc_model::Swap`); what does not fit in an item's width is cut off, so a slider is seen coming and going.
        // (the row ends with the chevron of the devices, and before it the mixer's button where there is a speaker)
        let has_mixer = sides.contains(&0);
        let ends = if has_mixer { 2.0 } else { 1.0 } * (cc_model::HEAD + cc_model::ROW_GAP);
        let open_w = cc_model::open_width(sides.len(), cc_model::CARD_INNER - ends);
        let slider_w = cc_model::open_slider(open_w);
        let moving = self.sound_swap.open() == open;
        let mut row = h_flex().h(px(cc_model::HEAD)).gap(px(cc_model::ROW_GAP)).items_center();
        for side in sides.iter().copied() {
            let is_open = side == open;
            let share = if sides.len() > 1 && moving { self.sound_swap.share(side, now) } else { f32::from(u8::from(is_open)) };
            let (id, icon, tip, frac, which) = if side == 0 {
                let tip = if !is_open { t!("Hoparlör") } else if speaker.muted { t!("Sesi aç") } else { t!("Sessize al") };
                ("cc-mute", cc_model::volume_icon(audio::glyph(&speaker)), tip, audio::volume_to_slider(self.vol_drag.unwrap_or(speaker.volume)), SL_VOLUME)
            } else {
                let tip = if !is_open { t!("Mikrofon") } else if mic.muted { t!("Mikrofonu aç") } else { t!("Mikrofonu kapat") };
                ("cc-mic-mute", if mic.muted { L::MicOff } else { L::Mic }, tip, audio::volume_to_slider(self.mic_drag.unwrap_or(mic.volume)), SL_MIC)
            };
            let stop = self.stop(id, 0, cx);
            let head = self.round_button(id, icon, cc_model::HEAD, false, look, (Some(&stop), Some(tip))).on_click(cx.listener(move |this, _, _, cx| this.press_sound_head(side, cx)));
            let mut item = h_flex().w(px(cc_model::swap_width(share, open_w))).h_full().flex_shrink_0().gap(px(cc_model::ROW_GAP)).items_center().overflow_hidden().child(head);
            // (a side that is only its button has no slider to stop at with the keyboard)
            if share > 0.02 {
                item = item.child(self.slider_of(which, frac, slider_w, look, cx));
            }
            row = row.child(item);
        }
        // the list of the devices of the side that has the slider (it opens ABOVE this card, so the chevron points up while it is closed)
        let (detail, list_tip) = if open == 0 { (Detail::Outputs, t!("Çıkış aygıtları")) } else { (Detail::Inputs, t!("Giriş aygıtları")) };
        let list_stop = self.stop("cc-devices", 0, cx);
        let chevron = self
            .round_button("cc-devices", if self.is_open(detail) { L::ChevronDown } else { L::ChevronUp }, cc_model::HEAD, false, look, (Some(&list_stop), Some(list_tip)))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_detail(detail, cx)));
        let name = if open == 0 {
            cc_model::sound_device_text(&speaker, words)
        } else if mic.device.trim().is_empty() {
            t!("Mikrofon").to_string()
        } else {
            mic.device.clone()
        };
        Some(
            self.card(look)
                .flex_shrink_0()
                .px(px(12.))
                .py(px(12.))
                .child(
                    row.children(has_mixer.then(|| {
                        let stop = self.stop("cc-mixer", 0, cx);
                        self.round_button("cc-mixer", L::AppWindow, cc_model::HEAD, self.is_open(Detail::Mixer), look, (Some(&stop), Some(t!("Uygulama sesleri"))))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_detail(Detail::Mixer, cx)))
                    }))
                    .child(chevron),
                )
                .child(div().mt(px(4.)).h(px(16.)).pl(px(4.)).truncate().text_xs().text_color(secondary).child(name))
                .into_any_element(),
        )
    }

    /// The line at the top while a program uses the microphone or the camera: which, and who.
    fn render_in_use(&self, look: &Look, words: Words) -> AnyElement {
        let icon = if self.in_use.microphone.is_empty() { L::Camera } else { L::Mic };
        h_flex()
            .h(px(cc_model::IN_USE_H))
            .flex_shrink_0()
            .px(px(6.))
            .gap(px(6.))
            .items_center()
            .text_xs()
            .text_color(look.pal.text_secondary().hsla())
            .child(Icon::new(icon).size(px(12.)).flex_shrink_0().text_color(look.accent))
            .child(div().truncate().child(cc_model::in_use_text(&self.in_use, words)))
            .into_any_element()
    }

    /// The input devices: as the outputs (`detail_outputs`), for the microphone.
    fn detail_inputs(&self, look: &Look, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let devices = self.mic.as_ref().map(|a| a.devices.clone()).unwrap_or_default();
        let mut rows = Vec::new();
        for (i, d) in devices.iter().enumerate() {
            let id = d.id.clone();
            let click: Option<RowClick> = (!d.default).then(|| Rc::new(move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| this.choose_input(&id, window, cx)) as RowClick);
            rows.push(self.detail_row(("cc-input", i), d.default.then_some(L::Check), d.name.clone(), None, None, look, click, cx));
        }
        rows.push(self.detail_link("cc-mic-settings", t!("Ses ayarları"), "ms-settings:sound", look, cx));
        (self.detail_head(t!("Giriş aygıtları").to_string(), None, look, cx), rows)
    }

    /// A detail's head: its name, and a radio's switch at the right when it has one.
    fn detail_head(&self, title: String, radio: Option<(RadioKind, RadioPower)>, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let switch = radio.map(|(kind, power)| {
            let me = cx.entity().downgrade();
            let id: ElementId = match kind {
                RadioKind::WiFi => "cc-wifi-switch".into(),
                RadioKind::Bluetooth => "cc-bluetooth-switch".into(),
            };
            crate::liquid::switch_track(id, &look.theme, power == RadioPower::On, power == RadioPower::Disabled, move |_, cx| {
                let _ = me.update(cx, |this, cx| this.press_radio(kind, cx));
            })
        });
        h_flex()
            .h(px(cc_model::DETAIL_HEAD_H))
            .flex_shrink_0()
            .px(px(8.))
            .items_center()
            .justify_between()
            .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
            .children(switch)
            .into_any_element()
    }

    /// One row of a detail: a picture (or its empty place), a text, and at the right a quiet note and a small picture. With `click`
    /// it is a choice (a keyboard stop, lit under the pointer); without, a line to read.
    #[allow(clippy::too_many_arguments)]
    fn detail_row(&self, id: (&'static str, usize), icon: Option<L>, text: String, note: Option<String>, mark: Option<L>, look: &Look, click: Option<RowClick>, cx: &mut Context<Self>) -> AnyElement {
        let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
        let secondary = look.pal.text_secondary().hsla();
        let ring = look.accent;
        let row = h_flex()
            .id(id)
            .h(px(cc_model::DETAIL_ROW_H))
            .flex_shrink_0()
            .px(px(8.))
            .gap(px(8.))
            .items_center()
            .rounded(px(8.))
            .border_1()
            .border_color(transparent_black())
            .text_sm()
            .child(div().w(px(14.)).flex_shrink_0().children(icon.map(|i| Icon::new(i).size(px(14.)).text_color(look.pal.text().hsla()))))
            .child(div().flex_1().min_w_0().truncate().child(text))
            .children(note.map(|n| div().flex_shrink_0().text_xs().text_color(secondary).child(n)))
            .children(mark.map(|m| Icon::new(m).size(px(12.)).flex_shrink_0().text_color(secondary)));
        match click {
            Some(click) => {
                let stop = self.stop(id.0, id.1, cx);
                row.cursor_pointer()
                    .track_focus(&stop)
                    .focus_visible(move |s| s.border_color(ring))
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .on_click(cx.listener(move |this, _, window, cx| click(this, window, cx)))
                    .into_any_element()
            }
            None => row.into_any_element(),
        }
    }

    /// A line of a detail that says why there is no list (the radio is off, nothing was found).
    fn detail_line(&self, text: &'static str, look: &Look) -> AnyElement {
        div().h(px(cc_model::DETAIL_ROW_H)).flex_shrink_0().px(px(8.)).flex().items_center().text_sm().text_color(look.pal.text_secondary().hsla()).child(div().truncate().child(text)).into_any_element()
    }

    /// The last row of a detail: the way to Windows' own page for it.
    fn detail_link(&self, id: &'static str, text: &'static str, uri: &'static str, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        self.detail_row((id, 0), Some(L::Settings), text.to_string(), None, None, look, Some(Rc::new(move |this, _, cx| this.open_settings(uri, cx))), cx)
    }

    /// The output devices: the one Windows plays to now with a check mark (a line to read), every other as a choice (a click makes
    /// it the default output), then Windows' own sound page.
    fn detail_outputs(&self, look: &Look, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let devices = sysstate::audio(cx).map(|a| a.devices).unwrap_or_default();
        let mut rows = Vec::new();
        for (i, d) in devices.iter().enumerate() {
            let id = d.id.clone();
            let click: Option<RowClick> = (!d.default).then(|| Rc::new(move |this: &mut Self, window: &mut Window, cx: &mut Context<Self>| this.choose_output(&id, window, cx)) as RowClick);
            rows.push(self.detail_row(("cc-device", i), d.default.then_some(L::Check), d.name.clone(), None, None, look, click, cx));
        }
        rows.push(self.detail_link("cc-sound-settings", t!("Ses ayarları"), "ms-settings:sound", look, cx));
        (self.detail_head(t!("Çıkış aygıtları").to_string(), None, look, cx), rows)
    }

    /// The mixer: every program that plays now, with a slider for its own level (Windows' volume mixer, here), then Windows' own
    /// page for it. It has a button of its own on the sound card: under the output devices, where it first stood, it was two clicks
    /// and a scroll away (issue 22: "Is it possible to make selection of sound apps more comfortable? with one click").
    fn detail_mixer(&self, look: &Look, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let mut rows = Vec::new();
        if self.mixer.is_empty() {
            rows.push(self.detail_line(t!("Şu an ses çalan uygulama yok"), look));
        }
        for (i, app) in self.mixer.iter().enumerate() {
            rows.push(
                h_flex()
                    .h(px(cc_model::DETAIL_ROW_H))
                    .flex_shrink_0()
                    .px(px(8.))
                    .gap(px(8.))
                    .items_center()
                    .text_sm()
                    .child(div().w(px(MIXER_NAME_W)).flex_shrink_0().truncate().child(app.name.clone()))
                    .child(self.slider_of(SL_APP + i, self.mixer_level(app), MIXER_SLIDER_W, look, cx))
                    .into_any_element(),
            );
        }
        rows.push(self.detail_link("cc-mixer-settings", t!("Ses ayarları"), "ms-settings:apps-volume", look, cx));
        (self.detail_head(t!("Uygulama sesleri").to_string(), None, look, cx), rows)
    }

    /// Wi-Fi: its switch, the networks in range (see `choose_network` for what a click does), Windows' Wi-Fi page.
    fn detail_wifi(&self, look: &Look, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let w = self.wifi_now().unwrap_or_default();
        let mut rows = Vec::new();
        match w.power {
            RadioPower::Disabled => rows.push(self.detail_line(t!("Kullanılamıyor"), look)),
            RadioPower::Off => rows.push(self.detail_line(t!("Wi-Fi kapalı"), look)),
            RadioPower::On if w.listing == Listing::Denied => {
                // (the names of the networks in range are location data to Windows: with location off it does not give them)
                rows.push(self.detail_line(t!("Ağları listelemek için Windows'ta konum izni gerekiyor"), look));
                rows.push(self.detail_link("cc-wifi-location", t!("Konum ayarları"), "ms-settings:privacy-location", look, cx));
            }
            RadioPower::On if w.networks.is_empty() => rows.push(self.detail_line(t!("Ağlar aranıyor…"), look)),
            RadioPower::On => {
                for (i, n) in w.networks.iter().enumerate() {
                    let joining = self.joining.as_ref().is_some_and(|(s, at)| *s == n.ssid && at.elapsed() < JOIN_WAIT);
                    let note = if n.connected {
                        Some(t!("Bağlı"))
                    } else if joining {
                        Some(t!("Bağlanıyor…"))
                    } else if n.known() {
                        Some(t!("Kayıtlı"))
                    } else {
                        None
                    };
                    let ssid = n.ssid.clone();
                    let click: RowClick = Rc::new(move |this, window, cx| this.choose_network(&ssid, window, cx));
                    rows.push(self.detail_row(("cc-wifi-net", i), Some(cc_model::signal_icon(n.bars)), n.ssid.clone(), note.map(str::to_string), n.needs_password().then_some(L::Lock), look, Some(click), cx));
                    // the network whose password is asked for: the field, right under its row
                    if self.wifi_asking.as_deref() == Some(n.ssid.as_str()) {
                        rows.extend(self.wifi_password_row(look, cx));
                    }
                }
            }
        }
        rows.push(self.detail_link("cc-wifi-settings", t!("Wi-Fi ayarları"), "ms-settings:network-wifi", look, cx));
        (self.detail_head(cc_model::wifi_title(&w), Some((RadioKind::WiFi, w.power)), look, cx), rows)
    }

    /// The row with the field for a network's password, and the button that connects with it.
    fn wifi_password_row(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let input = self.wifi_input.as_ref()?;
        let stop = self.stop("cc-wifi-join", 0, cx);
        let (tip, mark) = if self.wifi_bad { (t!("Şifre 8-63 karakter olmalı"), gpui_kit::rgb(0xE81123).into()) } else { (t!("Bağlan"), transparent_black()) };
        Some(
            h_flex()
                .h(px(cc_model::DETAIL_ROW_H))
                .flex_shrink_0()
                .pl(px(30.))
                .pr(px(4.))
                .gap(px(6.))
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h(px(26.))
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .border_1()
                        .border_color(mark)
                        .bg(look.pal.hover_fill().hsla())
                        .text_sm()
                        .child(Input::new(input).appearance(false)),
                )
                .child(self.round_button("cc-wifi-join", L::ArrowRight, 26.0, true, look, (Some(&stop), Some(tip))).on_click(cx.listener(|this, _, window, cx| this.submit_wifi_password(window, cx))))
                .into_any_element(),
        )
    }

    /// Bluetooth: its switch, the paired devices (which are connected, the battery of the ones that report one; a headset or a
    /// speaker is a choice: a click connects its sound or lets it go), Windows' Bluetooth page.
    fn detail_bluetooth(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let b = self.bt_now().unwrap_or_default();
        let mut rows = Vec::new();
        match b.power {
            RadioPower::Disabled => rows.push(self.detail_line(t!("Kullanılamıyor"), look)),
            RadioPower::Off => rows.push(self.detail_line(t!("Bluetooth kapalı"), look)),
            RadioPower::On if b.devices.is_empty() => rows.push(self.detail_line(t!("Eşleşmiş cihaz yok"), look)),
            RadioPower::On => {
                for (i, d) in b.devices.iter().enumerate() {
                    let icon = if d.audio {
                        L::Headphones
                    } else if d.connected {
                        L::BluetoothConnected
                    } else {
                        L::Bluetooth
                    };
                    // what was asked of its sound a moment ago, while Bluetooth has not yet said so
                    let busy = self.bt_busy.as_ref().filter(|(a, connect, at)| d.address.as_deref() == Some(a.as_str()) && at.elapsed() < BT_WAIT && *connect != d.connected).map(|(_, connect, _)| *connect);
                    let state = match busy {
                        Some(true) => Some(t!("Bağlanıyor…")),
                        Some(false) => Some(t!("Bağlantı kesiliyor…")),
                        None => d.connected.then_some(t!("Bağlı")),
                    };
                    let note: Vec<String> = d.battery.map(|level| (words.pct)(level)).into_iter().chain(state.map(str::to_string)).collect();
                    let click: Option<RowClick> = d.address.clone().filter(|_| d.audio).map(|address| {
                        let connected = d.connected;
                        Rc::new(move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| this.choose_bt(&address, connected, cx)) as RowClick
                    });
                    rows.push(self.detail_row(("cc-bt-device", i), Some(icon), d.name.clone(), (!note.is_empty()).then(|| note.join(" · ")), None, look, click, cx));
                }
            }
        }
        rows.push(self.detail_link("cc-bt-settings", t!("Bluetooth ayarları"), "ms-settings:bluetooth", look, cx));
        (self.detail_head(cc_model::bluetooth_title(&b), Some((RadioKind::Bluetooth, b.power)), look, cx), rows)
    }

    /// The connection that is not Wi-Fi (the cable): its state and the machine's address on it, Windows' network page.
    fn detail_network(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> (AnyElement, Vec<AnyElement>) {
        let n = sysstate::network(cx).unwrap_or_default();
        let mut rows = Vec::new();
        for (i, r) in cc_model::network_rows(&n, words).into_iter().enumerate() {
            // (the modem's row is a way to its own pages: a click opens them in the browser)
            let (mark, click) = match r.open {
                Some(page) => (Some(L::ExternalLink), Some(Rc::new(move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| this.open_link(page.clone(), cx)) as RowClick)),
                None => (None, None),
            };
            rows.push(self.detail_row(("cc-net-row", i), None, r.name, Some(r.value), mark, look, click, cx));
        }
        rows.push(self.detail_link("cc-net-settings", t!("Ağ ayarları"), "ms-settings:network", look, cx));
        (self.detail_head(cc_model::network_text(&n, words).title, None, look, cx), rows)
    }

    /// The elements of one section (a detail's card is not one of them: `render_area`).
    fn section(&self, s: Section, look: &Look, words: Words, cx: &mut Context<Self>) -> Vec<AnyElement> {
        match s {
            Section::InUse => vec![self.render_in_use(look, words)],
            Section::Tiles(_) => self.render_tiles(look, words, cx),
            Section::Media => self.render_media(look, words, cx).into_iter().collect(),
            Section::Sound => self.render_sound(look, words, cx).into_iter().collect(),
            Section::Brightness(_) => self.render_brightness(look, cx).into_iter().collect(),
            Section::Bottom => vec![self.render_bottom(look, words, cx)],
            Section::Detail(..) => Vec::new(),
        }
    }

    /// The place a detail opens out in, at `t` of its way (0 folded, 1 open, a little past either while the spring swings): the
    /// cards it takes the place of, fading, and over them its card, growing out of the edge that stays.
    fn render_area(&self, area: &Area, t: f32, look: &Look, words: Words, cx: &mut Context<Self>) -> AnyElement {
        let (place_h, card_h) = (area.place(t), area.card(t));
        // (a place that takes the place of nothing brings its gap with it as it opens: `Area::spare_gap`)
        let mut place = div().relative().w_full().h(px(place_h)).mt(px(-area.spare_gap(t))).flex_shrink_0();
        // (once it is fully out the cards under it are not built at all: nothing of them is seen, and nothing of them is a stop)
        if !(self.unfold.opening() && self.unfold.settled(self.now_ms())) {
            // The cards it takes the place of stand where they stood (measured from the edge that stays) and are seen only in the
            // part of the place the card has not reached: the card wipes over them, the two are never drawn through each other
            // (a card is a sheet of glass, not a lid). They fade as they go.
            let top = if area.upwards { place_h - area.room } else { -card_h.min(place_h) };
            let mut under = v_flex().absolute().left_0().right_0().top(px(top)).gap(px(cc_model::GAP)).opacity((1.0 - t).clamp(0.0, 1.0));
            for s in &area.covered {
                under = under.children(self.section(*s, look, words, cx));
            }
            let mut seen = div().absolute().left_0().right_0().h(px((place_h - card_h).max(0.0))).overflow_hidden();
            seen = if area.upwards { seen.top_0() } else { seen.bottom_0() };
            place = place.child(seen.child(under));
        }
        // (the card takes the pointer: what is under it is not pressed through it)
        let mut card = div().absolute().left_0().right_0().occlude().opacity((t * 2.5).clamp(0.0, 1.0));
        card = if area.upwards { card.bottom_0() } else { card.top_0() };
        place.child(card.child(self.render_detail(area.kind, area.card(t), look, words, cx))).into_any_element()
    }

    /// What is opened out, as one card of `height`: its head, and its rows, which scroll when they are more than fit.
    fn render_detail(&self, kind: Detail, height: f32, look: &Look, words: Words, cx: &mut Context<Self>) -> AnyElement {
        let (head, rows) = match kind {
            Detail::Outputs => self.detail_outputs(look, cx),
            Detail::Inputs => self.detail_inputs(look, cx),
            Detail::Mixer => self.detail_mixer(look, cx),
            Detail::Wifi => self.detail_wifi(look, cx),
            Detail::Bluetooth => self.detail_bluetooth(look, words, cx),
            Detail::Network => self.detail_network(look, words, cx),
        };
        self.card(look)
            .h(px(height))
            .flex_shrink_0()
            .p(px(cc_model::DETAIL_PAD))
            .flex()
            .flex_col()
            .overflow_hidden()
            .child(head)
            .child(v_flex().id(("cc-detail", kind as usize)).flex_1().min_h_0().overflow_y_scroll().children(rows))
            .into_any_element()
    }

    fn render_brightness(&self, look: &Look, cx: &mut Context<Self>) -> Option<AnyElement> {
        let screens: Vec<(usize, BrightnessSnapshot)> = self.brightness.iter().copied().enumerate().filter(|(_, b)| b.available).collect();
        if screens.is_empty() {
            return None;
        }
        let now = self.now_ms();
        let secondary = look.pal.text_secondary().hsla();
        let several = screens.len() > 1;
        // the display that has the slider: the one last pressed while it answers, else the first
        let open = if screens.iter().any(|(i, _)| *i == self.bright_swap.open()) { self.bright_swap.open() } else { screens[0].0 };
        let moving = self.bright_swap.open() == open;
        let open_w = cc_model::open_width(screens.len(), cc_model::CARD_INNER);
        let slider_w = cc_model::open_slider(open_w);
        let mut row = h_flex().h(px(cc_model::HEAD)).gap(px(cc_model::ROW_GAP)).items_center();
        for (i, b) in screens {
            let is_open = i == open;
            let share = if several && moving { self.bright_swap.share(i, now) } else { f32::from(u8::from(is_open)) };
            // its head: the sun for a lone display; with several, each display's number (the primary one is 1), lit on the one that
            // has the slider, and a press on another's gives it the slider
            let head = if several {
                let (stop, ring) = (self.stop("cc-screen", i, cx), look.accent);
                let (hover, pressed) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla());
                div()
                    .id(("cc-screen", i))
                    .size(px(cc_model::HEAD))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .border_2()
                    .border_color(transparent_black())
                    .cursor_pointer()
                    .track_focus(&stop)
                    .focus_visible(move |s| s.border_color(ring))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(is_open, |d| d.bg(look.accent).text_color(look.on_accent))
                    .when(!is_open, |d| d.bg(look.pal.field_fill().hsla()).hover(move |s| s.bg(hover)).active(move |s| s.bg(pressed)))
                    .child((i + 1).to_string())
                    .on_click(cx.listener(move |this, _, _, cx| this.press_screen(i, cx)))
                    .into_any_element()
            } else {
                div().size(px(cc_model::HEAD)).flex_shrink_0().flex().items_center().justify_center().child(Icon::new(L::Sun).size(px(16.)).text_color(look.pal.text().hsla())).into_any_element()
            };
            let mut item = h_flex().w(px(cc_model::swap_width(share, open_w))).h_full().flex_shrink_0().gap(px(cc_model::ROW_GAP)).items_center().overflow_hidden().child(head);
            if share > 0.02 {
                item = item.child(self.slider_of(SL_SCREEN + i, b.fraction(), slider_w, look, cx));
            }
            row = row.child(item);
        }
        Some(self.card(look).flex_shrink_0().px(px(12.)).py(px(12.)).child(row).child(div().mt(px(4.)).h(px(16.)).pl(px(4.)).text_xs().text_color(secondary).child(t!("Parlaklık"))).into_any_element())
    }

    /// The bottom row: the quick buttons at the left (lit while the thing is on, each with its name under the pointer), the panel's
    /// own three at the right. All of one size, which gets smaller when the user shows more buttons than fit at the full one.
    fn render_bottom(&self, look: &Look, words: Words, cx: &mut Context<Self>) -> AnyElement {
        let (state, quick) = (self.quick_state(), self.quick_now(cx));
        let (size, gap) = cc_model::bottom_buttons(quick.len() + 3, cc_model::WIDTH - 2.0 * cc_model::PAD);
        let mut left = h_flex().gap(px(gap)).items_center();
        for q in quick {
            let stop = self.stop("cc-quick", q as usize, cx);
            left = left.child(
                self.round_button(("cc-quick", q as usize), q.icon(&state), size, q.lit(&state), look, (Some(&stop), Some(q.tip(&state, words))))
                    .on_click(cx.listener(move |this, _, _, cx| this.press_quick(q, cx))),
            );
        }
        let (focus, snip_focus, edit_focus) = (self.stop("cc-gear", 0, cx), self.stop("cc-snip", 0, cx), self.stop("cc-edit", 0, cx));
        let right = h_flex()
            .gap(px(gap))
            .items_center()
            .child(
                self.round_button("cc-edit", L::SlidersHorizontal, size, false, look, (Some(&edit_focus), Some(t!("Paneli düzenle"))))
                    .on_click(cx.listener(|this, _, _, cx| this.open_edit(cx))),
            )
            .child(self.round_button("cc-snip", L::Scissors, size, false, look, (Some(&snip_focus), Some(t!("Ekran alıntısı")))).on_click(cx.listener(|this, _, _, cx| this.start_snip(cx))))
            .child(self.round_button("cc-gear", L::Settings, size, false, look, (Some(&focus), Some(t!("Ayarlar")))).on_click(cx.listener(|this, _, _, cx| this.open_settings("ms-settings:", cx))));
        h_flex().h(px(cc_model::BOTTOM_H)).flex_shrink_0().items_center().justify_between().child(left).child(right).into_any_element()
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
        // The island's glass is growing into this panel (the setting "Paneller adalardan büyüyerek açılsın"): the motion decides what is
        // seen (the glass's shape, the content's opacity) and the panel's own 120 ms easing stays out of it (`grow_host.rs`).
        let grown = super::grow_host::view_frame(PanelId::CONTROL_CENTRE, cx);
        let fill = glass_watch::panel_fill(dark, grown.map_or_else(|| self.popup.as_ref().is_none_or(|g| g.fallback()), |g| g.plain));
        let frame = match (self.anim, grown) {
            (_, Some(_)) | (None, None) => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
            (Some((t, opening)), None) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
        };
        if !frame.done || grown.is_some_and(|g| g.moving) {
            window.request_animation_frame();
        }

        let mut body = v_flex().size_full().p(px(cc_model::PAD)).gap(px(cc_model::GAP)).opacity(frame.opacity).mt(px(frame.drop_px));
        let layout = cc_model::layout(&self.shown_now(cx));
        for s in layout.before {
            body = body.children(self.section(s, &look, words, cx));
        }
        if let Some(area) = &layout.area {
            body = body.child(self.render_area(area, self.unfold.value(self.now_ms()), &look, words, cx));
        }
        for s in layout.after {
            body = body.children(self.section(s, &look, words, cx));
        }
        div()
            .id("control-centre")
            .relative()
            .size_full()
            // (an ordinary panel paints its fill over its whole window; a growing one over the glass's rectangle of the moment)
            .when(grown.is_none(), |d| d.bg(fill))
            .children(grown.map(|g| super::grow_view::wash(&g, fill)))
            .children(grown.and_then(|g| super::grow_view::hairline(&g, look.pal)))
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
            .child(match grown {
                Some(g) => super::grow_view::body(&g, body).into_any_element(),
                None => body.into_any_element(),
            })
            .into_any_element()
    }
}
