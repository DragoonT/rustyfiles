//! The control centre's decisions, with no window in them: which tiles and sections a machine gets, how tall the panel is for them,
//! the words on each tile for a snapshot (Turkish or English), which symbol a state gets, and the slider's pointer arithmetic. The
//! view (`control_centre.rs`) only puts these on the screen.
//!
//! The words are passed a translator (`Words`) instead of reading the app's language, so a test can ask for both languages in the
//! same run without touching the global one; every Turkish text used here has its English pair in `i18n_en` (a test checks that).

use gpui_kit::assets::IconName as L;
use rf_win::sysstatus::audio::{AudioSnapshot, VolumeGlyph};
use rf_win::sysstatus::bluetooth::{BluetoothSnapshot, Power};
use rf_win::sysstatus::inuse::InUse;
use rf_win::sysstatus::toggles::PowerMode;
use rf_win::sysstatus::media::{MediaSession, Timeline};
use rf_win::sysstatus::network::{DnsOwner, Link, NetworkGlyph, NetworkSnapshot, Reach, dns_owner, router_page, wifi_steps};
use rf_win::sysstatus::power::BatterySnapshot;
use rf_win::sysstatus::wifi::{Listing, WifiSnapshot};

// ------------------------------------------------------------------ measures (logical px)

pub const WIDTH: f32 = 360.0;
/// The space between the window's edge and the cards.
pub const PAD: f32 = 12.0;
pub const GAP: f32 = 10.0;
pub const TILE_H: f32 = 64.0;
pub const MEDIA_H: f32 = 88.0;
pub const SOUND_H: f32 = 72.0;
pub const BRIGHTNESS_H: f32 = 72.0;
pub const BOTTOM_H: f32 = 36.0;
/// A round button at the head of a slider (the speaker, the microphone, a display's number): what an item of a row that opens one
/// item at a time is while it is not the open one.
pub const HEAD: f32 = 28.0;
/// The gap between two things of a card's row.
pub const ROW_GAP: f32 = 8.0;
/// The inner width of a card (the panel less its padding and the card's own).
pub const CARD_INNER: f32 = WIDTH - 2.0 * PAD - 24.0;
/// The line that says who is using the microphone or the camera.
pub const IN_USE_H: f32 = 20.0;
/// A detail (the outputs, a tile's list) is one card: this padding round it, a head (its name, and a radio's switch), rows.
pub const DETAIL_PAD: f32 = 8.0;
pub const DETAIL_HEAD_H: f32 = 32.0;
pub const DETAIL_ROW_H: f32 = 30.0;
/// The rows a detail's card shows without the panel growing for it, when it has that many: see [`sections`].
pub const DETAIL_USABLE_ROWS: usize = 5;
/// A press on a radio's button is the state its tile shows for this long, while Windows has not yet said so.
pub const RADIO_PENDING_MS: u64 = 4_000;
/// The spring a detail opens out and folds back with: the time the motion is felt to take, and how far it swings past its end
/// (`rf_win::dock::spring`; a little bounce, so that it is seen to be a spring and comes to rest at once).
pub const UNFOLD_MS: f32 = 380.0;
pub const UNFOLD_BOUNCE: f32 = 0.25;
/// A click on the speaker is the "latest" state for this long, while the audio thread has not yet answered with a snapshot.
pub const MUTE_PENDING_MS: u64 = 1_000;
/// A slider's width in its card (the card's inner width less the buttons at its ends).
pub const SLIDER_W: f32 = 240.0;

// ------------------------------------------------------------------ what is shown

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Wifi,
    Network,
    Bluetooth,
    Battery,
}

/// The connectivity tiles this machine gets, in their order. A tile whose source is not there (no Wi-Fi or Bluetooth radio, no
/// battery, no network source) is left out, not shown dead; a source that has not answered yet (`None`) gets none either.
///
/// A machine with a Wi-Fi radio has a Wi-Fi tile (the user, 2026-11-15, of a PC on a cable whose Wi-Fi was off: "wifi tuşu yok
/// nedense"): the two radios first, then the network tile for a connection that is NOT Wi-Fi (the cable). There the network tile
/// has nothing to say while the machine is on Wi-Fi or on nothing, and is left out. Without a Wi-Fi radio the network tile is the
/// network's one tile and comes first, as before.
pub fn tiles(net: Option<&NetworkSnapshot>, wifi: Option<&WifiSnapshot>, bt: Option<&BluetoothSnapshot>, battery: Option<&BatterySnapshot>) -> Vec<Tile> {
    let mut v = Vec::new();
    let has_wifi = wifi.is_some_and(|w| w.available);
    let has_bt = bt.is_some_and(|b| b.available);
    let net = net.filter(|n| n.available);
    if has_wifi {
        v.push(Tile::Wifi);
        if has_bt {
            v.push(Tile::Bluetooth);
        }
        if net.is_some_and(|n| !matches!(n.link, Link::WiFi { .. } | Link::Disconnected)) {
            v.push(Tile::Network);
        }
    } else {
        if net.is_some() {
            v.push(Tile::Network);
        }
        if has_bt {
            v.push(Tile::Bluetooth);
        }
    }
    if battery.is_some_and(|b| b.present) {
        v.push(Tile::Battery);
    }
    v
}

/// A part of the panel that opens out, one at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Detail {
    /// The output devices: opens out UPWARDS from the sound card.
    Outputs,
    /// The input devices: opens out UPWARDS from the microphone's card.
    Inputs,
    /// The programs that play now, each with a slider for its own level: opens out UPWARDS from the sound card, by a button of its
    /// own (issue 22: under the output devices it was two clicks and a scroll away).
    Mixer,
    /// A tile's own list: opens out DOWNWARDS from the tiles.
    Wifi,
    Bluetooth,
    Network,
}

impl Detail {
    /// It opens out upwards, from a card under it (the others open downwards, from the tiles).
    pub fn upwards(self) -> bool {
        matches!(self, Detail::Outputs | Detail::Inputs | Detail::Mixer)
    }
}

/// The detail a tile opens (the battery's has none: it opens Windows' page).
pub fn detail_of(tile: Tile) -> Option<Detail> {
    match tile {
        Tile::Wifi => Some(Detail::Wifi),
        Tile::Bluetooth => Some(Detail::Bluetooth),
        Tile::Network => Some(Detail::Network),
        Tile::Battery => None,
    }
}

/// How tall a detail's card is for `rows` rows under its head.
pub fn detail_height(rows: usize) -> f32 {
    2.0 * DETAIL_PAD + DETAIL_HEAD_H + rows as f32 * DETAIL_ROW_H
}

/// What decides the sections of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Shown {
    /// A program is using the microphone or the camera: the line that says so.
    pub in_use: bool,
    pub tiles: usize,
    /// A media session exists.
    pub media: bool,
    /// A sound device exists, an output or a microphone: the one sound card (they share its row: [`Swap`]).
    pub sound: bool,
    /// How many displays answer: they share the one brightness card's row (0: no card).
    pub brightness: usize,
    /// What is opened out, and how many rows it has; `None` while nothing is.
    pub detail: Option<(Detail, usize)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Section {
    InUse,
    /// The tiles, two to a row: the number of rows.
    Tiles(usize),
    Media,
    /// What is opened out, as one card of this height, IN THE PLACE of the sections it covers. (Made by [`sections`] alone, which
    /// is the layout in the form the tests read; the view draws from [`layout`].)
    #[cfg_attr(not(test), allow(dead_code))]
    Detail(Detail, f32),
    /// The speaker and the microphone, in one row.
    Sound,
    /// The brightness card: how many displays share its row.
    Brightness(usize),
    /// The quick buttons and the panel's own three, in one row.
    Bottom,
}

/// The place a detail opens out in.
#[derive(Clone, Debug, PartialEq)]
pub struct Area {
    pub kind: Detail,
    /// The card's height when it is fully opened out.
    pub height: f32,
    /// The sections it takes the place of (top to bottom) and the room they have, gaps included.
    pub covered: Vec<Section>,
    pub room: f32,
    /// It opens out upwards, its bottom edge staying (the outputs, from the sound card); else downwards, its top edge staying (a
    /// tile's list, from the tiles).
    pub upwards: bool,
}

impl Area {
    /// The height of the area's place in the panel while the detail is opened out by `t` (0 folded, 1 open): the room of what it
    /// covers, growing to the card's height where that is more. Never past its ends, whatever the spring does.
    pub fn place(&self, t: f32) -> f32 {
        self.room + (self.height - self.room) * t.clamp(0.0, 1.0)
    }

    /// The card's own height then: it GROWS from nothing at the edge that stays, and the spring may carry it a little past.
    pub fn card(&self, t: f32) -> f32 {
        (self.height * t).max(0.0)
    }

    /// The part of a gap this place does not need yet. A detail that takes the place of NOTHING (a machine where nothing stands
    /// above the sound card, or nothing under the tiles) is a section more than the panel had, and with it a gap more: the gap
    /// comes with the card as it opens, so that the folded panel is exactly the panel without a detail.
    pub fn spare_gap(&self, t: f32) -> f32 {
        if self.room > 0.0 { 0.0 } else { GAP * (1.0 - t.clamp(0.0, 1.0)) }
    }
}

/// The panel from top to bottom: sections, the place of the detail that is opened out (if one is), sections.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub before: Vec<Section>,
    pub area: Option<Area>,
    pub after: Vec<Section>,
}

/// The sections a machine gets, top to bottom, without the bottom row (which is always there) and with nothing opened out: who
/// uses the microphone, the tiles, what plays, the sound, the brightness.
///
/// ONE ROW FOR THE THINGS OF ONE KIND (the user, 2026-11-15, of the panel that had a card for the speaker, one for the microphone
/// and a slider for each display: "mesela mikrofonla hoparlör aynı satırda olur ama hangisine tıklarsan o genişler. monitörde de
/// aynı. hem 3-4 monitörü olan adamın yeri çok dolmaz."): the speaker and the microphone are one card, the displays are one card,
/// and in each the one that was pressed has the slider while the others are a round button ([`Swap`]). The quick buttons stand in
/// the bottom row, in the room beside the panel's own three ("burayı da değerlendirelim").
fn base(s: &Shown) -> Vec<Section> {
    [
        s.in_use.then_some(Section::InUse),
        (s.tiles > 0).then(|| Section::Tiles(s.tiles.div_ceil(2))),
        s.media.then_some(Section::Media),
        s.sound.then_some(Section::Sound),
        (s.brightness > 0).then_some(Section::Brightness(s.brightness)),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The panel's layout.
///
/// A DETAIL TAKES THE PLACE OF OTHER SECTIONS, it is not added to them (the user, 2026-11-15: "ses aygıtlarına tıklayınca panelin
/// genişlemesi yerine üstteki player ethernet ve blutooth alanlarının yerine genişlesin", and of the tiles: "onlar da aynı şekilde
/// alta doğru genişleyebilir"). A list of devices opens out UPWARDS from its card (the outputs from the sound card, the inputs
/// from the microphone's): its card stands where everything above that card was, and the card that was pressed stays where it
/// is. A tile's list opens out DOWNWARDS: its card stands where everything under the tiles was, and the tiles stay. The card is as
/// tall as the room it takes, so the panel keeps its height; its rows scroll when they are more than fit. ONLY when that room is
/// too small to be a list at all is the card taller than the room: as tall as its rows want, up to `DETAIL_USABLE_ROWS` of them,
/// and the panel grows by the difference while the card opens. A detail whose owner is not there (no sound device, no tiles) is
/// not laid out.
pub fn layout(s: &Shown) -> Layout {
    let mut all = base(s);
    let owner = s.detail.and_then(|(d, rows)| {
        // (the outputs and the inputs both open out of the sound card, which has the speaker and the microphone)
        let at = all.iter().position(|sec| if d.upwards() { *sec == Section::Sound } else { matches!(sec, Section::Tiles(_)) })?;
        Some((d, rows, at))
    });
    let Some((kind, rows, at)) = owner else {
        return Layout { before: all, area: None, after: vec![Section::Bottom] };
    };
    let upwards = kind.upwards();
    // (upwards: everything above the owner is covered and the owner leads what stays; downwards: the owner stays above and
    // everything under it is covered)
    let cut = if upwards { at } else { at + 1 };
    let tail = all.split_off(cut);
    let (before, covered, mut after) = if upwards { (Vec::new(), all, tail) } else { (all, tail, Vec::new()) };
    after.push(Section::Bottom);
    let room = covered.iter().map(section_height).sum::<f32>() + GAP * covered.len().saturating_sub(1) as f32;
    let area = Area { kind, height: room.max(detail_height(rows.min(DETAIL_USABLE_ROWS))), covered, room, upwards };
    Layout { before, area: Some(area), after }
}

/// The sections from top to bottom with the detail fully opened out (its card as one section). The layout as the tests read it.
#[cfg(test)]
pub fn sections(s: &Shown) -> Vec<Section> {
    let l = layout(s);
    l.before.into_iter().chain(l.area.map(|a| Section::Detail(a.kind, a.height))).chain(l.after).collect()
}

pub fn section_height(s: &Section) -> f32 {
    match s {
        Section::InUse => IN_USE_H,
        Section::Tiles(rows) => *rows as f32 * TILE_H + rows.saturating_sub(1) as f32 * GAP,
        Section::Media => MEDIA_H,
        Section::Detail(_, h) => *h,
        Section::Sound => SOUND_H,
        Section::Brightness(_) => BRIGHTNESS_H,
        Section::Bottom => BOTTOM_H,
    }
}

/// The panel's height: the sections, the gaps between them and the padding round them. (The bottom row is always there.) The
/// view asks [`height_at`]; this is its end, as the tests read it.
#[cfg(test)]
pub fn height(s: &Shown) -> f32 {
    height_at(s, 1.0)
}

/// The panel's height while the detail is opened out by `t` (see [`Area::place`]): the panel grows and shrinks WITH the card where
/// it has to grow at all.
pub fn height_at(s: &Shown, t: f32) -> f32 {
    let l = layout(s);
    let parts = l.before.len() + l.after.len() + usize::from(l.area.is_some());
    let sum: f32 = l.before.iter().chain(&l.after).map(section_height).sum::<f32>() + l.area.as_ref().map_or(0.0, |a| a.place(t) - a.spare_gap(t));
    2.0 * PAD + sum + GAP * parts.saturating_sub(1) as f32
}

/// How far a detail is opened out, moving on a spring: 0 folded, 1 open, a little past either on the way (the bounce). A motion
/// that is turned round in mid flight keeps its speed. Pure: the time is the caller's, in ms.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Unfold {
    from: f32,
    vel: f32,
    to: f32,
    started_ms: u64,
    settle_ms: u32,
}

impl Unfold {
    /// At rest, folded.
    pub const fn folded() -> Unfold {
        Unfold { from: 0.0, vel: 0.0, to: 0.0, started_ms: 0, settle_ms: 0 }
    }

    /// At rest, open (a detail that is there when the panel opens does not open out a second time).
    pub const fn open() -> Unfold {
        Unfold { from: 1.0, vel: 0.0, to: 1.0, started_ms: 0, settle_ms: 0 }
    }

    fn spring() -> rf_win::dock::spring::Spring {
        rf_win::dock::spring::Spring::new(UNFOLD_MS, UNFOLD_BOUNCE)
    }

    fn state(&self, now_ms: u64) -> (f32, f32) {
        let elapsed = now_ms.saturating_sub(self.started_ms);
        if elapsed >= u64::from(self.settle_ms) {
            return (self.to, 0.0);
        }
        let (e, v) = Self::spring().at(self.from - self.to, self.vel, elapsed as f32);
        (self.to + e, v)
    }

    /// Sets it going towards open or folded from where it is and as fast as it is; without animations it is there at once.
    pub fn go(&mut self, open: bool, now_ms: u64, animate: bool) {
        let to = if open { 1.0 } else { 0.0 };
        if !animate {
            *self = Unfold { from: to, vel: 0.0, to, started_ms: now_ms, settle_ms: 0 };
            return;
        }
        if self.to == to {
            return;
        }
        let (x, v) = self.state(now_ms);
        let settle = Self::spring().settle_ms(x - to, v, 0.002).ceil().min(3_000.0) as u32;
        *self = Unfold { from: x, vel: v, to, started_ms: now_ms, settle_ms: settle };
    }

    pub fn value(&self, now_ms: u64) -> f32 {
        self.state(now_ms).0
    }

    /// Where it is going: open.
    pub fn opening(&self) -> bool {
        self.to == 1.0
    }

    /// It has come to rest.
    pub fn settled(&self, now_ms: u64) -> bool {
        now_ms.saturating_sub(self.started_ms) >= u64::from(self.settle_ms)
    }
}

/// How many rows a detail has under its head, for what the sources say now (the view builds the same rows: a line that says why
/// there is no list, the list, and the way to Windows' page as the last row).
pub fn detail_rows(d: Detail, audio: Option<&AudioSnapshot>, mic: Option<&AudioSnapshot>, wifi: Option<&WifiSnapshot>, bt: Option<&BluetoothSnapshot>, net: Option<&NetworkSnapshot>) -> usize {
    match d {
        Detail::Outputs => audio.map_or(0, |a| a.devices.len()) + 1,
        Detail::Inputs => mic.map_or(0, |a| a.devices.len()) + 1,
        // (the programs' rows are the panel's own to count: `mixer_rows`)
        Detail::Mixer => 0,
        Detail::Wifi => match wifi {
            Some(w) if w.power == Power::On && w.listing == Listing::Denied => 3,
            Some(w) if w.power == Power::On && !w.networks.is_empty() => w.networks.len() + 1,
            _ => 2,
        },
        Detail::Bluetooth => match bt {
            Some(b) if b.power == Power::On && !b.devices.is_empty() => b.devices.len() + 1,
            _ => 2,
        },
        Detail::Network => net.map_or(0, |n| network_rows(n, Words { tr: |s| s, pct: |_| String::new() }).len()) + 1,
    }
}

/// The state a radio's button shows: what the last press asked for (`ask`: on or off, and how many ms ago) while Windows has not
/// yet said so, for at most `RADIO_PENDING_MS`; else what Windows says. A radio that cannot be switched shows that, whatever was
/// pressed.
pub fn shown_power(actual: Power, ask: Option<(bool, u64)>) -> Power {
    match (actual, ask) {
        (Power::Disabled, _) => Power::Disabled,
        (_, Some((on, age))) if age < RADIO_PENDING_MS => {
            if on {
                Power::On
            } else {
                Power::Off
            }
        }
        (p, _) => p,
    }
}

// ------------------------------------------------------------------ a row that opens one item at a time

/// Which item of a row has the slider, and the motion while another takes its place: the one that was pressed grows from a round
/// button to the row's width while the one that was open shrinks to a button, on the spring a detail opens with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Swap {
    from: usize,
    to: usize,
    motion: Unfold,
}

impl Swap {
    /// At rest, with item `i` open.
    pub const fn at(i: usize) -> Swap {
        Swap { from: i, to: i, motion: Unfold::open() }
    }

    /// The item that is open, or opening.
    pub fn open(&self) -> usize {
        self.to
    }

    /// Opens item `to` in the place of the open one. The same item again changes nothing.
    pub fn go(&mut self, to: usize, now_ms: u64, animate: bool) {
        if to == self.to {
            return;
        }
        (self.from, self.to) = (self.to, to);
        self.motion = Unfold::folded();
        self.motion.go(true, now_ms, animate);
    }

    /// How far item `i` is opened out: 1 for the open one, 0 for the others, in between (and a little past, as a spring goes) for
    /// the two that are changing places. The two shares always add up to 1.
    pub fn share(&self, i: usize, now_ms: u64) -> f32 {
        let t = self.motion.value(now_ms);
        if i == self.to {
            t
        } else if i == self.from {
            1.0 - t
        } else {
            0.0
        }
    }

    pub fn settled(&self, now_ms: u64) -> bool {
        self.motion.settled(now_ms)
    }
}

/// The width of an item whose share is `share`: a round button when 0, `open` when 1.
pub fn swap_width(share: f32, open: f32) -> f32 {
    (HEAD + (open - HEAD) * share).max(0.0)
}

/// The width the open item of a row of `items` has, when the row is `room` wide: what the others, as round buttons, leave it.
/// The rows of the mixer's detail for `apps` programs that play: one each, or one line that says there is none, and the way to
/// Windows' own page.
pub fn mixer_rows(apps: usize) -> usize {
    apps.max(1) + 1
}

pub fn open_width(items: usize, room: f32) -> f32 {
    room - items.saturating_sub(1) as f32 * (HEAD + ROW_GAP)
}

/// The slider's width in an open item: the item less its round button, and no wider than a card's own slider.
pub fn open_slider(open: f32) -> f32 {
    (open - HEAD - ROW_GAP).min(SLIDER_W)
}

/// The size of the round buttons of the bottom row and the gap between two of them, for `count` buttons in a row `room` wide: the
/// full size while they fit, smaller and closer when the user shows more than fit.
pub fn bottom_buttons(count: usize, room: f32) -> (f32, f32) {
    let fits = |size: f32, gap: f32| count as f32 * size + count.saturating_sub(1) as f32 * gap <= room;
    if fits(BOTTOM_H, 6.0) {
        return (BOTTOM_H, 6.0);
    }
    let gap = 4.0;
    (((room - count.saturating_sub(1) as f32 * gap) / count.max(1) as f32).floor().clamp(20.0, BOTTOM_H), gap)
}

// ------------------------------------------------------------------ the quick buttons, and what can be hidden

/// One of the quick buttons of the bottom row. Three switch something themselves (Windows' dark mode, keeping the PC awake, the power mode);
/// the others open the page or the panel of Windows that does (Windows has no switch a program may press for them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quick {
    Dark,
    Awake,
    Power,
    NightLight,
    Focus,
    Airplane,
    Project,
    Cast,
}

/// What the three switching buttons are at now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct QuickState {
    pub dark: bool,
    pub awake: bool,
    /// `None`: Windows tells no power mode here, and there is no such button.
    pub power: Option<PowerMode>,
}

impl Quick {
    pub const ALL: [Quick; 8] = [Quick::Dark, Quick::Awake, Quick::Power, Quick::NightLight, Quick::Focus, Quick::Airplane, Quick::Project, Quick::Cast];

    /// Its name in the settings' list of what is hidden.
    pub fn key(self) -> &'static str {
        match self {
            Quick::Dark => "quick.dark",
            Quick::Awake => "quick.awake",
            Quick::Power => "quick.power",
            Quick::NightLight => "quick.nightlight",
            Quick::Focus => "quick.focus",
            Quick::Airplane => "quick.airplane",
            Quick::Project => "quick.project",
            Quick::Cast => "quick.cast",
        }
    }

    pub fn name(self, w: Words) -> &'static str {
        (w.tr)(match self {
            Quick::Dark => "Koyu tema",
            Quick::Awake => "Uyanık tut",
            Quick::Power => "Güç modu",
            Quick::NightLight => "Gece ışığı",
            Quick::Focus => "Rahatsız etmeyin",
            Quick::Airplane => "Uçak modu",
            Quick::Project => "Yansıt",
            Quick::Cast => "Yayınla",
        })
    }

    /// The page of Windows it opens; `None` for the three that switch something themselves and for "project", which is a panel of
    /// Windows started as a program.
    pub fn page(self) -> Option<&'static str> {
        match self {
            Quick::NightLight => Some("ms-settings:nightlight"),
            Quick::Focus => Some("ms-settings:notifications"),
            Quick::Airplane => Some("ms-settings:network-airplanemode"),
            Quick::Cast => Some("ms-settings-connectabledevices:devicediscovery"),
            Quick::Dark | Quick::Awake | Quick::Power | Quick::Project => None,
        }
    }

    pub fn icon(self, s: &QuickState) -> L {
        match self {
            Quick::Dark => L::Moon,
            Quick::Awake => L::Coffee,
            Quick::Power => match s.power {
                Some(PowerMode::Efficiency) => L::Leaf,
                Some(PowerMode::Performance) => L::Zap,
                _ => L::Gauge,
            },
            Quick::NightLight => L::Sunset,
            Quick::Focus => L::BellOff,
            Quick::Airplane => L::Plane,
            Quick::Project => L::Projector,
            Quick::Cast => L::Cast,
        }
    }

    /// Lit: the thing is on (a power mode that is not the balanced one counts as on).
    pub fn lit(self, s: &QuickState) -> bool {
        match self {
            Quick::Dark => s.dark,
            Quick::Awake => s.awake,
            Quick::Power => s.power.is_some_and(|m| m != PowerMode::Balanced),
            _ => false,
        }
    }

    /// What the button says under the pointer: its name, and for the power mode which one it is at.
    pub fn tip(self, s: &QuickState, w: Words) -> &'static str {
        match (self, s.power) {
            (Quick::Power, Some(PowerMode::Efficiency)) => (w.tr)("Güç modu: en iyi verimlilik"),
            (Quick::Power, Some(PowerMode::Performance)) => (w.tr)("Güç modu: en iyi performans"),
            (Quick::Power, _) => (w.tr)("Güç modu: dengeli"),
            _ => self.name(w),
        }
    }
}

/// The parts of the panel that can be hidden (Settings > Dock), by their names in the settings' list. The quick buttons are hidden
/// one by one with [`Quick::key`], or all at once with "quick".
pub const PARTS: [&str; 10] = ["wifi", "bluetooth", "network", "battery", "quick", "inuse", "media", "sound", "mic", "brightness"];

pub fn part_name(key: &str, w: Words) -> &'static str {
    (w.tr)(match key {
        // (the two that are names, the same in every language)
        "wifi" => return "Wi-Fi",
        "bluetooth" => return "Bluetooth",
        "network" => "Ağ",
        "battery" => "Pil",
        "quick" => "Hızlı düğmeler",
        "inuse" => "Mikrofon ve kamera kullanımı",
        "media" => "Şimdi çalıyor",
        "sound" => "Ses",
        "mic" => "Mikrofon",
        _ => "Parlaklık",
    })
}

/// The quick buttons the bottom row shows: the ones that are not hidden, and the power mode's only where Windows tells one.
pub fn quick_buttons(hidden: &dyn Fn(&str) -> bool, s: &QuickState) -> Vec<Quick> {
    if hidden("quick") {
        return Vec::new();
    }
    Quick::ALL.into_iter().filter(|q| !hidden(q.key()) && (*q != Quick::Power || s.power.is_some())).collect()
}

/// The settings' list of what is hidden with `key` hidden or shown again: each name once, the parts first and then the buttons,
/// in their own order; a name this version does not know is kept at the end.
pub fn with_hidden(list: &[String], key: &str, hide: bool) -> Vec<String> {
    let order = |k: &str| PARTS.iter().position(|p| *p == k).or_else(|| Quick::ALL.iter().position(|q| q.key() == k).map(|i| PARTS.len() + i)).unwrap_or(usize::MAX);
    let mut out: Vec<String> = list.iter().filter(|k| k.as_str() != key).cloned().collect();
    if hide {
        out.push(key.to_string());
    }
    out.sort_by_key(|k| order(k));
    out.dedup();
    out
}

/// The line that says who is using the microphone or the camera: "Mikrofon: chrome, Discord · Kamera: Zoom".
pub fn in_use_text(u: &InUse, w: Words) -> String {
    let part = |label: &str, apps: &[String]| (!apps.is_empty()).then(|| format!("{label}: {}", apps.join(", ")));
    [part((w.tr)("Mikrofon"), &u.microphone), part((w.tr)("Kamera"), &u.camera)].into_iter().flatten().collect::<Vec<_>>().join(" · ")
}

// ------------------------------------------------------------------ words

/// The translator and the percent format of the language in use.
#[derive(Clone, Copy)]
pub struct Words {
    pub tr: fn(&'static str) -> &'static str,
    pub pct: fn(u8) -> String,
}

impl Words {
    /// The app's language.
    pub fn current() -> Words {
        Words { tr: crate::i18n::t, pct: |p| crate::i18n::pct(f32::from(p)) }
    }
}

/// What a tile says: a bold line, a quiet one under it, and whether its button is lit (the thing is on).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileText {
    pub title: String,
    pub subtitle: String,
    pub active: bool,
}

/// How strong a Wi-Fi signal is, in a word (Windows' 0 to 5 bars).
pub fn signal_word(bars: u8, w: Words) -> &'static str {
    match bars {
        0..=1 => (w.tr)("Zayıf"),
        2..=3 => (w.tr)("Orta"),
        _ => (w.tr)("Güçlü"),
    }
}

pub fn network_text(s: &NetworkSnapshot, w: Words) -> TileText {
    let tr = w.tr;
    let t = |title: String, subtitle: String, active: bool| TileText { title, subtitle, active };
    match &s.link {
        Link::Disconnected => t(tr("Ağ").into(), tr("Bağlı değil").into(), false),
        Link::Ethernet => t("Ethernet".into(), if s.reach == Reach::Internet { tr("Bağlı") } else { tr("İnternet yok") }.into(), true),
        Link::WiFi { name, bars } => {
            let title = if name.trim().is_empty() { "Wi-Fi".to_string() } else { name.clone() };
            let state = if s.reach == Reach::Internet { signal_word(*bars, w) } else { tr("İnternet yok") };
            t(title, format!("Wi-Fi · {state}"), true)
        }
        Link::Cellular => t(tr("Hücresel").into(), if s.reach == Reach::Internet { tr("Bağlı") } else { tr("İnternet yok") }.into(), true),
        Link::Other => t(tr("Ağ").into(), tr("Bağlı").into(), true),
    }
}

/// The network tile of a PC with a wired card (`rf_win::ethernet`): the card turned off, on and in use, or on with something else in
/// use (no cable, or Wi-Fi in front). Its button turns the card on and off; it is lit while the card is on.
pub fn wired_text(enabled: bool, net: Option<&NetworkSnapshot>, w: Words) -> TileText {
    let tr = w.tr;
    let t = |subtitle: &str, active: bool| TileText { title: "Ethernet".into(), subtitle: subtitle.into(), active };
    if !enabled {
        return t(tr("Kapalı"), false);
    }
    match net.map(|n| (&n.link, n.reach)) {
        Some((Link::Ethernet, Reach::Internet)) => t(tr("Bağlı"), true),
        Some((Link::Ethernet, _)) => t(tr("İnternet yok"), true),
        _ => t(tr("Bağlı değil"), true),
    }
}

/// The Wi-Fi tile: off, on with nothing connected, or the network it is on. The name comes from the list of networks, or, when
/// Windows does not give that list (location is off), from the connection in use.
pub fn wifi_text(s: &WifiSnapshot, net: Option<&NetworkSnapshot>, w: Words) -> TileText {
    let tr = w.tr;
    let name = s.connected().map(|n| n.ssid.clone()).or_else(|| match net.map(|n| &n.link) {
        Some(Link::WiFi { name, .. }) if !name.trim().is_empty() => Some(name.clone()),
        _ => None,
    });
    let subtitle = match s.power {
        Power::Disabled => tr("Kullanılamıyor").to_string(),
        Power::Off => tr("Kapalı").to_string(),
        Power::On => name.unwrap_or_else(|| tr("Bağlı değil").to_string()),
    };
    TileText { title: "Wi-Fi".into(), subtitle, active: s.power == Power::On }
}

/// The Wi-Fi version, short ("6E", "7"): the standard of the connection in use while there is one, else the highest the card
/// supports. (The owner, 2026-11-26: "wifi ve bluetooth'un yanına sürümlerini yazabilir miyiz? wi-fi 6E gibi"; at first it was the
/// connection's alone, as Windows' network properties and the quick settings of phones show it, and on a PC on Ethernet the tile then
/// said nothing: "wifi sürümü yazmıyor". macOS's detailed Wi-Fi menu also tells the card's supported modes while not connected.)
pub fn wifi_tag(s: &WifiSnapshot) -> Option<String> {
    match s.standard {
        Some(st) if s.power == Power::On => Some(st.short()),
        _ => s.card_generation.map(|g| g.to_string()),
    }
}

/// "Wi-Fi 6E", or "Wi-Fi" when no standard is known.
pub fn wifi_title(s: &WifiSnapshot) -> String {
    wifi_tag(s).map_or_else(|| "Wi-Fi".into(), |t| format!("Wi-Fi {t}"))
}

/// The Bluetooth adapter's version, when Windows tells it: "5.4".
pub fn bluetooth_tag(s: &BluetoothSnapshot) -> Option<String> {
    s.version.map(str::to_string)
}

/// "Bluetooth 5.4", or "Bluetooth".
pub fn bluetooth_title(s: &BluetoothSnapshot) -> String {
    bluetooth_tag(s).map_or_else(|| "Bluetooth".into(), |t| format!("Bluetooth {t}"))
}

/// One row of the network tile's detail: a name, what it is, and for the modem the address of its own pages (a click opens them).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRow {
    pub name: String,
    pub value: String,
    pub open: Option<String>,
}

/// Whose the connection's name servers are, in a line: "Modem (İSS)", "Cloudflare", "Cloudflare, Modem (İSS)", or the address of
/// one nobody here knows. The modem counts as the provider's: asked for a name, a modem asks the servers its provider gave it,
/// unless somebody changed that in the modem itself, which this cannot see.
pub fn dns_text(servers: &[String], gateway: Option<&str>, w: Words) -> String {
    let mut out: Vec<String> = Vec::new();
    for s in servers {
        let name = match dns_owner(s, gateway) {
            DnsOwner::Router => (w.tr)("Modem (İSS)").to_string(),
            DnsOwner::Known(name) => name.to_string(),
            DnsOwner::Local => (w.tr)("Yerel ağ").to_string(),
            DnsOwner::Other => s.clone(),
        };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out.join(", ")
}

/// A link's speed as it is said: "100 Mb/sn", "1 Gb/sn", "2.5 Gb/sn".
pub fn speed_text(mbps: u64, w: Words) -> String {
    if mbps >= 1000 {
        let gb = mbps as f64 / 1000.0;
        let number = if (gb - gb.round()).abs() < 0.05 { format!("{}", gb.round() as u64) } else { format!("{gb:.1}") };
        format!("{number} {}", (w.tr)("Gb/sn"))
    } else {
        format!("{mbps} {}", (w.tr)("Mb/sn"))
    }
}

/// The rows of the network tile's detail: the state, the machine's address, the modem (with the address of its own pages when it
/// has an IPv4 address: a click opens them), whose the name servers are, the link's speed. A thing Windows does not tell has no row.
pub fn network_rows(s: &NetworkSnapshot, w: Words) -> Vec<NetRow> {
    let tr = w.tr;
    let row = |name: &'static str, value: String, open: Option<String>| NetRow { name: tr(name).to_string(), value, open };
    let state = match (&s.link, s.reach) {
        (Link::Disconnected, _) => tr("Bağlı değil"),
        (_, Reach::Internet) => tr("Bağlı"),
        _ => tr("İnternet yok"),
    };
    let mut rows = vec![row("Durum", state.to_string(), None)];
    if let Some(a) = s.address.as_ref().filter(|a| !a.trim().is_empty()) {
        rows.push(row("IP adresi", a.clone(), None));
    }
    if let Some(g) = s.gateway.as_ref().filter(|g| !g.trim().is_empty()) {
        match router_page(g) {
            Some(page) => rows.push(row("Modem arayüzü", g.clone(), Some(page))),
            None => rows.push(row("Ağ geçidi", g.clone(), None)),
        }
    }
    if !s.dns.is_empty() {
        rows.push(row("DNS", dns_text(&s.dns, s.gateway.as_deref(), w), None));
    }
    if let Some(speed) = s.speed_mbps {
        rows.push(row("Hız", speed_text(speed, w), None));
    }
    rows
}

pub fn bluetooth_text(s: &BluetoothSnapshot, w: Words) -> TileText {
    let tr = w.tr;
    let subtitle = match s.power {
        Power::Disabled => tr("Kullanılamıyor").to_string(),
        Power::Off => tr("Kapalı").to_string(),
        Power::On => match s.connected.as_slice() {
            [] => tr("Açık").to_string(),
            [one] => one.clone(),
            [first, rest @ ..] => format!("{first} +{}", rest.len()),
        },
    };
    TileText { title: "Bluetooth".into(), subtitle, active: s.power == Power::On }
}

pub fn battery_text(s: &BatterySnapshot, w: Words) -> TileText {
    let tr = w.tr;
    let level = s.percent.map(|p| (w.pct)(p));
    let state = if s.saver {
        Some(tr("Pil tasarrufu"))
    } else if s.charging {
        Some(tr("Şarj oluyor"))
    } else if s.on_ac {
        Some(tr("Prize takılı"))
    } else {
        None
    };
    let subtitle = match (level, state) {
        (Some(l), Some(st)) => format!("{l} · {st}"),
        (Some(l), None) => l,
        (None, Some(st)) => st.to_string(),
        (None, None) => String::new(),
    };
    TileText { title: tr("Pil").into(), subtitle, active: s.charging || s.on_ac }
}

/// The line under the sound slider: the default output device's name.
pub fn sound_device_text(s: &AudioSnapshot, w: Words) -> String {
    if s.device.trim().is_empty() { (w.tr)("Ses aygıtı").to_string() } else { s.device.clone() }
}

/// The three lines of the media card: the program, the title, the artist. A session without a title shows the program's name
/// there and nothing under it.
pub fn media_lines(s: &MediaSession, w: Words) -> (String, String, String) {
    let app = if s.app.is_empty() { (w.tr)("Şimdi çalıyor").to_string() } else { s.app.clone() };
    if s.title.trim().is_empty() { (app.clone(), app, String::new()) } else { (app, s.title.clone(), s.artist.clone()) }
}

/// The first line of the media card: the program, and which of the programs that play it is when there are more ("Chrome · 1/2";
/// a click on it turns the card to the next one).
pub fn media_source_line(app: &str, source: usize, sources: usize) -> String {
    if sources > 1 { format!("{app} · {}/{sources}", source.min(sources - 1) + 1) } else { app.to_string() }
}

/// Where the track is, as a player writes it: "1:05 / 3:20".
pub fn media_time(t: &Timeline, playing: bool, now: std::time::Instant) -> String {
    format!("{} / {}", rf_win::sysstatus::media::clock(t.position_at(now, playing)), rf_win::sysstatus::media::clock(t.duration_ms))
}

// ------------------------------------------------------------------ symbols

pub fn network_icon(g: NetworkGlyph) -> L {
    match g {
        NetworkGlyph::Off => L::WifiOff,
        NetworkGlyph::Ethernet => L::EthernetPort,
        NetworkGlyph::WiFi(0) => L::WifiZero,
        NetworkGlyph::WiFi(1) => L::WifiLow,
        NetworkGlyph::WiFi(2) => L::WifiHigh,
        NetworkGlyph::WiFi(_) => L::Wifi,
        NetworkGlyph::Other => L::Network,
    }
}

/// The symbol of a Wi-Fi signal of that many of Windows' bars (0 to 5).
pub fn signal_icon(bars: u8) -> L {
    network_icon(NetworkGlyph::WiFi(wifi_steps(bars)))
}

/// The Wi-Fi tile's symbol: off, on, or the strength of the network it is on.
pub fn wifi_icon(s: &WifiSnapshot) -> L {
    if s.power != Power::On {
        return L::WifiOff;
    }
    s.connected().map_or(L::Wifi, |n| signal_icon(n.bars))
}

pub fn volume_icon(g: VolumeGlyph) -> L {
    match g {
        VolumeGlyph::Unavailable => L::VolumeOff,
        VolumeGlyph::Muted => L::VolumeX,
        VolumeGlyph::Silent => L::Volume,
        VolumeGlyph::Low => L::Volume1,
        VolumeGlyph::High => L::Volume2,
    }
}

pub fn bluetooth_icon(s: &BluetoothSnapshot) -> L {
    match s.power {
        Power::On if !s.connected.is_empty() => L::BluetoothConnected,
        Power::On => L::Bluetooth,
        Power::Off | Power::Disabled => L::BluetoothOff,
    }
}

pub fn battery_icon(s: &BatterySnapshot) -> L {
    if s.charging {
        return L::BatteryCharging;
    }
    match s.percent {
        None => L::Battery,
        Some(0..=10) => L::BatteryWarning,
        Some(11..=35) => L::BatteryLow,
        Some(36..=70) => L::BatteryMedium,
        Some(_) => L::BatteryFull,
    }
}

// ------------------------------------------------------------------ the slider

/// What a click on the speaker asks for. `pending` is what the previous click asked for and how many ms ago: until the audio thread
/// has answered with a new snapshot the snapshot is out of date, and flipping it would make a quick double click ask for the same
/// thing twice (the second click lost). After `MUTE_PENDING_MS` the snapshot is the truth again.
pub fn mute_target(snapshot_muted: bool, pending: Option<(bool, u64)>) -> bool {
    match pending {
        Some((asked, age_ms)) if age_ms < MUTE_PENDING_MS => !asked,
        _ => !snapshot_muted,
    }
}

/// How many equal parts a volume slider has (the same as the write rounds to: `audio::slider_to_volume`).
pub fn slider_parts(steps: u32) -> u32 {
    if steps >= 2 { (steps - 1).min(100) } else { 100 }
}

/// The slider's share after a key, or `None` when the key is not the slider's: the arrows move one part, Shift and the page keys ten,
/// Home and End go to the ends.
pub fn slider_key(frac: f32, key: &str, shift: bool, parts: u32) -> Option<f32> {
    let one = 1.0 / parts.max(1) as f32;
    let ten = (one * 10.0).min(1.0);
    let next = match key {
        "right" | "up" => frac + if shift { ten } else { one },
        "left" | "down" => frac - if shift { ten } else { one },
        "pageup" => frac + ten,
        "pagedown" => frac - ten,
        "home" => 0.0,
        "end" => 1.0,
        _ => return None,
    };
    Some(next.clamp(0.0, 1.0))
}

/// The slider's share (0.0 to 1.0) for a pointer at `x`: the slider's box starts at `left` and is `width` wide; the thumb's centre
/// travels from half a resting thumb inside the left edge to half inside the right one (`liquid::slider_layout` is the inverse), and
/// a thumb that grows while it is held keeps its centre, so the mapping does not move under the pointer.
pub fn slider_fraction_at(x: f32, left: f32, width: f32) -> f32 {
    let thumb = crate::liquid::SL_REST_W;
    let run = (width - thumb).max(1.0);
    ((x - left - thumb / 2.0) / run).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rf_win::sysstatus::network::Reach;

    fn tr_tr(s: &'static str) -> &'static str {
        s
    }

    fn tr_en(s: &'static str) -> &'static str {
        crate::i18n_en::EN.iter().find(|(k, _)| *k == s).map(|(_, v)| *v).unwrap_or_else(|| panic!("no English pair for {s:?}"))
    }

    const TR: Words = Words { tr: tr_tr, pct: |p| format!("%{p}") };
    const EN: Words = Words { tr: tr_en, pct: |p| format!("{p}%") };

    fn net(link: Link, reach: Reach) -> NetworkSnapshot {
        NetworkSnapshot { available: true, link, reach, ..Default::default() }
    }

    fn bt(power: Power, connected: &[&str]) -> BluetoothSnapshot {
        let connected: Vec<String> = connected.iter().map(|s| s.to_string()).collect();
        BluetoothSnapshot { available: true, power, devices: rf_win::sysstatus::bluetooth::devices_of(Vec::new(), &connected), connected, version: None }
    }

    fn bat(percent: Option<u8>, charging: bool, on_ac: bool, saver: bool) -> BatterySnapshot {
        BatterySnapshot { present: true, percent, charging, on_ac, saver, minutes_left: None }
    }

    fn wifi(power: Power, nets: &[(&str, u8, bool)]) -> WifiSnapshot {
        let networks = nets.iter().map(|(ssid, bars, connected)| rf_win::sysstatus::wifi::Network { ssid: ssid.to_string(), bars: *bars, secured: true, profile: None, connected: *connected, auth: rf_win::sysstatus::wifi::Auth::Wpa2 }).collect();
        WifiSnapshot { available: true, power, networks, listing: Listing::Ok, standard: None, card_generation: None }
    }

    /// The tiles name the Wi-Fi standard of the connection in use and the Bluetooth adapter's version (the owner, 2026-11-26).
    #[test]
    fn the_titles_name_the_wifi_standard_in_use_and_the_bluetooth_version() {
        use rf_win::sysstatus::wifi::Standard;
        let mut w = wifi(Power::On, &[("Ev", 5, true)]);
        assert_eq!((wifi_tag(&w), wifi_title(&w)), (None, "Wi-Fi".into()), "the standard not known: the plain name");
        w.standard = Some(Standard { generation: 6, six_ghz: true });
        assert_eq!((wifi_tag(&w), wifi_title(&w)), (Some("6E".into()), "Wi-Fi 6E".into()));
        assert_eq!(wifi_text(&w, None, TR).title, "Wi-Fi", "the tile's name stays short: the tag stands beside it");
        w.power = Power::Off;
        assert_eq!(wifi_tag(&w), None, "off: no standard, whatever was last read");
        w.card_generation = Some(7);
        assert_eq!((wifi_tag(&w), wifi_title(&w)), (Some("7".into()), "Wi-Fi 7".into()), "off or not connected: the card's best");
        w.power = Power::On;
        assert_eq!(wifi_tag(&w), Some("6E".into()), "connected: the connection's, not the card's");
        let mut b = bt(Power::On, &[]);
        assert_eq!((bluetooth_tag(&b), bluetooth_title(&b)), (None, "Bluetooth".into()));
        b.version = Some("5.4");
        assert_eq!((bluetooth_tag(&b), bluetooth_title(&b)), (Some("5.4".into()), "Bluetooth 5.4".into()));
        b.power = Power::Off;
        assert_eq!(bluetooth_tag(&b), Some("5.4".into()), "the adapter's version, whether it is on or not");
    }

    #[test]
    fn a_desktop_with_a_network_and_a_bluetooth_radio_gets_two_tiles_and_a_laptop_three() {
        let n = net(Link::Ethernet, Reach::Internet);
        let b = bt(Power::Off, &[]);
        let none = BatterySnapshot::default();
        assert_eq!(tiles(Some(&n), None, Some(&b), Some(&none)), vec![Tile::Network, Tile::Bluetooth], "no battery: no battery tile");
        assert_eq!(tiles(Some(&n), None, Some(&b), Some(&bat(Some(50), false, false, false))), vec![Tile::Network, Tile::Bluetooth, Tile::Battery]);
        // no Bluetooth radio on this machine: no tile, not a dead one
        assert_eq!(tiles(Some(&n), None, Some(&BluetoothSnapshot::default()), None), vec![Tile::Network]);
        // a source that has not answered yet gives nothing; no network source gives no network tile
        assert_eq!(tiles(Some(&n), None, None, None), vec![Tile::Network]);
        assert_eq!(tiles(Some(&NetworkSnapshot::default()), None, Some(&b), None), vec![Tile::Bluetooth]);
        assert!(tiles(None, None, None, None).is_empty());
        // a machine without a Wi-Fi radio is as before, whatever the Wi-Fi source says about there being none
        assert_eq!(tiles(Some(&n), Some(&WifiSnapshot::default()), Some(&b), None), vec![Tile::Network, Tile::Bluetooth]);
    }

    #[test]
    fn a_machine_with_a_wifi_radio_has_a_wifi_tile_and_the_cable_keeps_its_own() {
        let b = bt(Power::On, &[]);
        let off = wifi(Power::Off, &[]);
        // on a cable with Wi-Fi off (the user's PC): the two radios, then the cable
        assert_eq!(tiles(Some(&net(Link::Ethernet, Reach::Internet)), Some(&off), Some(&b), None), vec![Tile::Wifi, Tile::Bluetooth, Tile::Network]);
        // on Wi-Fi, or on nothing: the network tile would repeat the Wi-Fi tile or say nothing, and is left out
        let on = wifi(Power::On, &[("Ev", 4, true)]);
        assert_eq!(tiles(Some(&net(Link::WiFi { name: "Ev".into(), bars: 4 }, Reach::Internet)), Some(&on), Some(&b), None), vec![Tile::Wifi, Tile::Bluetooth]);
        assert_eq!(tiles(Some(&net(Link::Disconnected, Reach::None)), Some(&on), Some(&b), None), vec![Tile::Wifi, Tile::Bluetooth]);
        // a laptop on Wi-Fi: Wi-Fi, Bluetooth, the battery; no Bluetooth radio: Wi-Fi and the cable
        assert_eq!(tiles(Some(&net(Link::Disconnected, Reach::None)), Some(&on), Some(&b), Some(&bat(Some(50), false, false, false))), vec![Tile::Wifi, Tile::Bluetooth, Tile::Battery]);
        assert_eq!(tiles(Some(&net(Link::Ethernet, Reach::Internet)), Some(&off), None, None), vec![Tile::Wifi, Tile::Network]);
        assert_eq!(tiles(Some(&net(Link::Other, Reach::Internet)), Some(&off), None, None), vec![Tile::Wifi, Tile::Network], "a VPN adapter is a connection of its own too");
        // which detail a tile opens
        assert_eq!([Tile::Wifi, Tile::Bluetooth, Tile::Network, Tile::Battery].map(detail_of), [Some(Detail::Wifi), Some(Detail::Bluetooth), Some(Detail::Network), None]);
    }

    #[test]
    fn the_sections_follow_what_exists_and_the_bottom_row_is_always_there() {
        assert_eq!(sections(&Shown::default()), vec![Section::Bottom]);
        let all = Shown { tiles: 3, media: true, sound: true, brightness: 1, detail: None, ..Default::default() };
        assert_eq!(sections(&all), vec![Section::Tiles(2), Section::Media, Section::Sound, Section::Brightness(1), Section::Bottom]);
        // tiles go two to a row
        assert_eq!(sections(&Shown { tiles: 1, ..Default::default() })[0], Section::Tiles(1));
        assert_eq!(sections(&Shown { tiles: 4, ..Default::default() })[0], Section::Tiles(2));
    }

    #[test]
    fn a_detail_takes_the_place_of_what_it_opens_over_and_the_panel_keeps_its_height() {
        // the user's PC: three tiles (two rows), something playing, sound, brightness
        let all = Shown { tiles: 3, media: true, sound: true, brightness: 1, detail: None, ..Default::default() };
        let above = 2.0 * TILE_H + GAP + GAP + MEDIA_H;
        let below = MEDIA_H + GAP + SOUND_H + GAP + BRIGHTNESS_H;
        // THE OUTPUTS: where the tiles and the media card were; the sound card and what is under it stay
        let outputs = Shown { detail: Some((Detail::Outputs, 6)), ..all };
        assert_eq!(sections(&outputs), vec![Section::Detail(Detail::Outputs, above), Section::Sound, Section::Brightness(1), Section::Bottom]);
        assert_eq!(height(&outputs), height(&all), "the panel does not grow");
        assert!(detail_height(6) <= above, "five devices and the link fit there without scrolling");
        // a list longer than the room scrolls in the same room
        assert_eq!(height(&Shown { detail: Some((Detail::Outputs, 40)), ..all }), height(&all));
        // A TILE'S LIST: where the media, sound and brightness cards were; the tiles stay
        for d in [Detail::Wifi, Detail::Bluetooth, Detail::Network] {
            let open = Shown { detail: Some((d, 12)), ..all };
            assert_eq!(sections(&open), vec![Section::Tiles(2), Section::Detail(d, below), Section::Bottom]);
            assert_eq!(height(&open), height(&all));
        }
        // TOO LITTLE ROOM to be a list (nothing is playing, one row of tiles): as tall as the rows want, up to the usable five
        let quiet = Shown { tiles: 2, media: false, sound: true, brightness: 1, detail: None, ..Default::default() };
        let small = Shown { detail: Some((Detail::Outputs, 6)), ..quiet };
        assert_eq!(sections(&small)[0], Section::Detail(Detail::Outputs, detail_height(DETAIL_USABLE_ROWS)));
        assert_eq!(height(&small), height(&quiet) + detail_height(DETAIL_USABLE_ROWS) - TILE_H);
        let two = Shown { detail: Some((Detail::Outputs, 2)), ..quiet };
        assert_eq!(sections(&two)[0], Section::Detail(Detail::Outputs, detail_height(2)), "a short list is as tall as it is");
        assert!(detail_height(2) > TILE_H);
        // nothing above the sound card at all: the card is added (there is nothing to take the place of)
        let bare = Shown { tiles: 0, media: false, sound: true, brightness: 0, detail: Some((Detail::Outputs, 3)), ..Default::default() };
        assert_eq!(sections(&bare), vec![Section::Detail(Detail::Outputs, detail_height(3)), Section::Sound, Section::Bottom]);
        // a detail whose owner is not there is not shown: no sound device, no tiles
        assert_eq!(sections(&Shown { sound: false, detail: Some((Detail::Outputs, 3)), ..all }), sections(&Shown { sound: false, ..all }));
        assert_eq!(sections(&Shown { tiles: 0, detail: Some((Detail::Wifi, 3)), ..all }), sections(&Shown { tiles: 0, ..all }));
    }

    #[test]
    fn a_detail_grows_from_the_edge_that_stays_and_the_panel_grows_only_where_it_must() {
        let all = Shown { tiles: 3, media: true, sound: true, brightness: 1, detail: None, ..Default::default() };
        assert_eq!(layout(&all), Layout { before: vec![Section::Tiles(2), Section::Media, Section::Sound, Section::Brightness(1)], area: None, after: vec![Section::Bottom] });
        // the outputs: upwards over the tiles and the media card, in their room; the panel is as tall all the way
        let outputs = Shown { detail: Some((Detail::Outputs, 6)), ..all };
        let l = layout(&outputs);
        let a = l.area.clone().unwrap();
        assert_eq!((a.kind, a.upwards, a.covered.clone()), (Detail::Outputs, true, vec![Section::Tiles(2), Section::Media]));
        assert_eq!(a.height, a.room);
        assert_eq!((l.before, l.after), (vec![], vec![Section::Sound, Section::Brightness(1), Section::Bottom]));
        for t in [0.0, 0.3, 1.0, 1.07, -0.05] {
            assert_eq!(a.place(t), a.room, "t {t}");
            assert_eq!(height_at(&outputs, t), height(&all), "t {t}");
        }
        // the card grows from nothing, and the spring may carry it a little past its place
        assert_eq!(a.card(0.0), 0.0);
        assert_eq!(a.card(0.5), a.height / 2.0);
        assert_eq!(a.card(1.0), a.height);
        assert!(a.card(1.04) > a.height);
        assert_eq!(a.card(-0.03), 0.0, "folding back past nothing is nothing");
        // a tile's list: downwards over the cards under the tiles
        let a = layout(&Shown { detail: Some((Detail::Bluetooth, 3)), ..all }).area.unwrap();
        assert_eq!((a.upwards, a.covered), (false, vec![Section::Media, Section::Sound, Section::Brightness(1)]));
        // too little room: the place grows from the room to the card's height while it opens, and the panel with it, never past
        let quiet = Shown { tiles: 2, media: false, sound: true, brightness: 1, detail: None, ..Default::default() };
        let small = Shown { detail: Some((Detail::Outputs, 6)), ..quiet };
        let a = layout(&small).area.unwrap();
        assert_eq!((a.room, a.height), (TILE_H, detail_height(DETAIL_USABLE_ROWS)));
        assert_eq!(a.place(0.0), TILE_H);
        assert_eq!(a.place(0.5), (TILE_H + a.height) / 2.0);
        assert_eq!(a.place(1.2), a.height);
        assert_eq!(height_at(&small, 0.0), height(&quiet), "folded: the panel as it was");
        assert_eq!(height_at(&small, 1.0), height(&small));
        assert_eq!(height_at(&small, 1.3), height(&small));
        assert!(height_at(&small, 0.5) > height(&quiet) && height_at(&small, 0.5) < height(&small));
    }

    /// "o panelde birinden biri veya daha fazlası eksik olabilir. orası dinamik çalışması lazım" (the user, 2026-11-15): whatever
    /// a machine has and has not, a detail has a place, a folded detail is the panel without one, and an open one never makes the
    /// panel shorter.
    #[test]
    fn a_detail_has_its_place_on_every_machine() {
        for bits in 0u32..(1 << 8) {
            let on = |bit: u32| bits & (1 << bit) != 0;
            let base = Shown { tiles: [0, 1, 2, 3][(bits & 3) as usize], media: on(2), sound: on(3), brightness: [0, 1, 2, 4][((bits >> 6) & 3) as usize] * usize::from(on(4)), in_use: on(5), detail: None };
            for d in [Detail::Outputs, Detail::Inputs, Detail::Wifi, Detail::Bluetooth, Detail::Network] {
                for rows in [1usize, 2, 6, 30] {
                    let open = Shown { detail: Some((d, rows)), ..base };
                    let l = layout(&open);
                    let owner = if d.upwards() { base.sound } else { base.tiles > 0 };
                    assert_eq!(l.area.is_some(), owner, "{open:?}");
                    assert_eq!(height_at(&open, 0.0), height(&base), "folded is the panel without it: {open:?}");
                    assert!(height(&open) >= height(&base), "{open:?}");
                    let Some(a) = l.area else { continue };
                    // every section is somewhere exactly once: beside the place, or under it
                    let mut all: Vec<Section> = l.before.iter().chain(&a.covered).chain(&l.after).copied().collect();
                    let mut want = sections(&base);
                    let key = |s: &Section| format!("{s:?}");
                    all.sort_by_key(key);
                    want.sort_by_key(key);
                    assert_eq!(all, want, "{open:?}");
                    // the card is a list: never shorter than its rows want, up to the usable ones; and as tall as its place
                    assert!(a.height >= detail_height(rows.min(DETAIL_USABLE_ROWS)), "{open:?}");
                    assert!(a.height >= a.room);
                    assert_eq!(a.place(1.0), a.height);
                    // the height moves one way while it opens
                    let hs: Vec<f32> = (0..=10).map(|i| height_at(&open, i as f32 / 10.0)).collect();
                    assert!(hs.windows(2).all(|w| w[1] >= w[0]), "{open:?}: {hs:?}");
                    assert_eq!(height_at(&open, 1.0), height(&open));
                }
            }
        }
        // the one case with nothing to take the place of: the gap comes with the card
        let bare = Shown { tiles: 0, media: false, sound: true, brightness: 0, detail: Some((Detail::Outputs, 3)), ..Default::default() };
        let a = layout(&bare).area.unwrap();
        assert_eq!((a.room, a.spare_gap(0.0), a.spare_gap(0.5), a.spare_gap(1.0)), (0.0, GAP, GAP / 2.0, 0.0));
        let roomy = layout(&Shown { tiles: 2, ..bare }).area.unwrap();
        assert_eq!(roomy.spare_gap(0.0), 0.0);
    }

    #[test]
    fn a_detail_opens_on_a_spring_that_swings_a_little_past_and_keeps_its_speed_when_turned_round() {
        let mut u = Unfold::folded();
        assert_eq!((u.value(0), u.settled(0), u.opening()), (0.0, true, false));
        u.go(true, 1_000, true);
        assert!(u.opening() && !u.settled(1_000));
        assert_eq!(u.value(1_000), 0.0, "it starts where it was");
        // it rises, passes 1 on the way (the bounce), and comes to rest exactly on 1
        let rising: Vec<f32> = (0..=12).map(|i| u.value(1_000 + i * 10)).collect();
        assert!(rising.windows(2).all(|w| w[1] > w[0]), "{rising:?}");
        let top = (0..800).map(|i| u.value(1_000 + i)).fold(0.0f32, f32::max);
        assert!(top > 1.005 && top < 1.08, "the swing past the end is small: {top}");
        let end = (0..3_000u64).find(|i| u.settled(1_000 + i)).expect("it comes to rest");
        assert!(end > 200 && end < 1_500, "{end} ms");
        assert_eq!(u.value(1_000 + end), 1.0);
        // the same order again changes nothing
        let same = u;
        u.go(true, 1_050, true);
        assert_eq!(u, same);
        // turned round in mid flight: from where it is, still moving the way it was for a moment
        let mut u = Unfold::folded();
        u.go(true, 0, true);
        let at = u.value(90);
        u.go(false, 90, true);
        assert_eq!(u.value(90), at);
        assert!(u.value(100) > at, "it still carries its speed");
        assert!(!u.opening());
        let end = (0..3_000u64).find(|i| u.settled(90 + i)).unwrap();
        assert_eq!(u.value(90 + end), 0.0);
        // without animations it is there at once, and a detail that is there when the panel opens is at rest, open
        let mut u = Unfold::folded();
        u.go(true, 5, false);
        assert_eq!((u.value(5), u.settled(5)), (1.0, true));
        u.go(false, 6, false);
        assert_eq!((u.value(6), u.settled(6), u.opening()), (0.0, true, false));
        assert_eq!((Unfold::open().value(0), Unfold::open().settled(0), Unfold::open().opening()), (1.0, true, true));
    }

    #[test]
    fn the_height_is_the_sections_the_gaps_and_the_padding() {
        // nothing but the bottom row
        assert_eq!(height(&Shown::default()), 2.0 * PAD + BOTTOM_H);
        // two tiles (one row), media, sound, brightness, bottom: 12 + 64 + 88 + 72 + 72 + 36 + 4 gaps of 10 + 12
        let all = Shown { tiles: 2, media: true, sound: true, brightness: 1, detail: None, ..Default::default() };
        assert_eq!(height(&all), 2.0 * PAD + TILE_H + MEDIA_H + SOUND_H + BRIGHTNESS_H + BOTTOM_H + 4.0 * GAP);
        // three tiles are two rows: one more tile height and a gap
        let three = Shown { tiles: 3, ..all };
        assert_eq!(height(&three), height(&all) + TILE_H + GAP);
        // a media card that comes and goes moves the height by its own and a gap
        assert_eq!(height(&Shown { media: false, ..all }), height(&all) - MEDIA_H - GAP);
    }

    #[test]
    fn a_details_rows_are_counted_from_what_the_sources_say() {
        let dev = |id: &str| rf_win::sysstatus::audio::Device { id: id.into(), name: id.into(), default: false };
        let audio = AudioSnapshot { available: true, devices: vec![dev("a"), dev("b"), dev("c")], ..Default::default() };
        assert_eq!(detail_rows(Detail::Outputs, Some(&audio), None, None, None, None), 4, "the devices and the link");
        // Wi-Fi: a line and the link while there is no list; the networks and the link; the location text, its link and the link
        assert_eq!(detail_rows(Detail::Wifi, None, None, Some(&wifi(Power::Off, &[])), None, None), 2);
        assert_eq!(detail_rows(Detail::Wifi, None, None, Some(&wifi(Power::On, &[])), None, None), 2);
        assert_eq!(detail_rows(Detail::Wifi, None, None, Some(&wifi(Power::On, &[("a", 1, false), ("b", 2, false), ("c", 3, true)])), None, None), 4);
        let denied = WifiSnapshot { listing: Listing::Denied, ..wifi(Power::On, &[]) };
        assert_eq!(detail_rows(Detail::Wifi, None, None, Some(&denied), None, None), 3);
        assert_eq!(detail_rows(Detail::Wifi, None, None, None, None, None), 2);
        // Bluetooth: the same shape
        assert_eq!(detail_rows(Detail::Bluetooth, None, None, None, Some(&bt(Power::Off, &[])), None), 2);
        assert_eq!(detail_rows(Detail::Bluetooth, None, None, None, Some(&bt(Power::On, &[])), None), 2);
        assert_eq!(detail_rows(Detail::Bluetooth, None, None, None, Some(&bt(Power::On, &["Fare", "Klavye"])), None), 3);
        // the cable: its state, its address when there is one, the link
        let mut eth = net(Link::Ethernet, Reach::Internet);
        assert_eq!(detail_rows(Detail::Network, None, None, None, None, Some(&eth)), 2);
        eth.address = Some("192.168.1.20".into());
        assert_eq!(detail_rows(Detail::Network, None, None, None, None, Some(&eth)), 3);
        let pairs = |s: &NetworkSnapshot, w: Words| network_rows(s, w).into_iter().map(|r| (r.name, r.value)).collect::<Vec<_>>();
        let pair = |a: &str, b: &str| (a.to_string(), b.to_string());
        assert_eq!(pairs(&eth, TR), [pair("Durum", "Bağlı"), pair("IP adresi", "192.168.1.20")]);
        assert_eq!(pairs(&eth, EN), [pair("Status", "Connected"), pair("IP address", "192.168.1.20")]);
        assert_eq!(network_rows(&net(Link::Ethernet, Reach::Local), EN)[0].value, "No internet");
        assert_eq!(network_rows(&net(Link::Disconnected, Reach::None), TR)[0].value, "Bağlı değil");
        // the modem (a click opens its own pages), whose the name servers are, the speed
        eth.gateway = Some("192.168.1.1".into());
        eth.dns = vec!["fe80::1".into(), "1.1.1.1".into(), "1.0.0.1".into(), "192.168.1.1".into()];
        eth.speed_mbps = Some(1000);
        assert_eq!(pairs(&eth, TR), [pair("Durum", "Bağlı"), pair("IP adresi", "192.168.1.20"), pair("Modem arayüzü", "192.168.1.1"), pair("DNS", "Modem (İSS), Cloudflare"), pair("Hız", "1 Gb/sn")]);
        assert_eq!(pairs(&eth, EN)[2..], [pair("Router page", "192.168.1.1"), pair("DNS", "Router (ISP), Cloudflare"), pair("Speed", "1 Gbps")]);
        let opens: Vec<Option<String>> = network_rows(&eth, TR).into_iter().map(|r| r.open).collect();
        assert_eq!(opens, [None, None, Some("http://192.168.1.1".to_string()), None, None], "only the modem's row is a way somewhere");
        assert_eq!(detail_rows(Detail::Network, None, None, None, None, Some(&eth)), 6);
        // a gateway that has only an IPv6 address is told, not offered as a page
        eth.gateway = Some("fe80::1".into());
        assert_eq!(network_rows(&eth, TR)[2], NetRow { name: "Ağ geçidi".into(), value: "fe80::1".into(), open: None });
        // name servers: a home one that is not the modem, one nobody knows (its number), each owner once
        assert_eq!(dns_text(&["192.168.1.53".into(), "8.8.8.8".into(), "8.8.4.4".into(), "212.156.4.4".into()], Some("192.168.1.1"), TR), "Yerel ağ, Google, 212.156.4.4");
        assert_eq!(dns_text(&[], None, TR), "");
        assert_eq!([speed_text(100, TR), speed_text(1000, TR), speed_text(2500, EN), speed_text(10_000, EN), speed_text(866, EN)], ["100 Mb/sn", "1 Gb/sn", "2.5 Gbps", "10 Gbps", "866 Mbps"]);
    }

    #[test]
    fn the_sections_stand_in_their_order_and_a_list_of_devices_covers_all_that_is_above_the_sound_card() {
        let all = Shown { in_use: true, tiles: 3, media: true, sound: true, brightness: 4, detail: None };
        assert_eq!(sections(&all), vec![Section::InUse, Section::Tiles(2), Section::Media, Section::Sound, Section::Brightness(4), Section::Bottom]);
        // one card for the speaker and the microphone, one for the displays however many: the panel is no taller for them
        assert_eq!(height(&all), 2.0 * PAD + IN_USE_H + 2.0 * TILE_H + GAP + MEDIA_H + SOUND_H + BRIGHTNESS_H + BOTTOM_H + 5.0 * GAP);
        assert_eq!(height(&Shown { brightness: 1, ..all }), height(&all));
        // the outputs and the inputs both open out of the sound card, over everything above it
        for d in [Detail::Outputs, Detail::Inputs] {
            let l = layout(&Shown { detail: Some((d, 6)), ..all });
            let a = l.area.unwrap();
            assert!(a.upwards);
            assert_eq!(a.covered, vec![Section::InUse, Section::Tiles(2), Section::Media]);
            assert_eq!(l.after, vec![Section::Sound, Section::Brightness(4), Section::Bottom]);
        }
        // a tile's list: the line above the tiles and the tiles stay, everything under them is covered
        let l = layout(&Shown { detail: Some((Detail::Wifi, 9)), ..all });
        assert_eq!(l.before, vec![Section::InUse, Section::Tiles(2)]);
        assert_eq!(l.area.unwrap().covered, vec![Section::Media, Section::Sound, Section::Brightness(4)]);
        assert_eq!(l.after, vec![Section::Bottom]);
        assert!(Detail::Outputs.upwards() && Detail::Inputs.upwards() && !Detail::Wifi.upwards() && !Detail::Bluetooth.upwards() && !Detail::Network.upwards());
    }

    #[test]
    fn a_row_opens_one_item_at_a_time_and_the_pressed_one_grows_as_the_open_one_shrinks() {
        let mut s = Swap::at(0);
        assert_eq!((s.open(), s.share(0, 0), s.share(1, 0), s.settled(0)), (0, 1.0, 0.0, true));
        // the microphone is pressed: it grows while the speaker shrinks, and the two always fill the same room
        s.go(1, 1_000, true);
        assert_eq!((s.open(), s.share(0, 1_000), s.share(1, 1_000)), (1, 1.0, 0.0), "it starts where it was");
        for at in [1_010u64, 1_060, 1_120, 1_300, 1_900] {
            assert!((s.share(0, at) + s.share(1, at) - 1.0).abs() < 1e-5, "at {at}");
            assert_eq!(s.share(2, at), 0.0, "an item that is not in it stays a button");
        }
        assert!(s.share(1, 1_100) > s.share(1, 1_030));
        let end = (0..3_000u64).find(|i| s.settled(1_000 + i)).unwrap();
        assert_eq!((s.share(1, 1_000 + end), s.share(0, 1_000 + end)), (1.0, 0.0));
        // pressed again: nothing; without animations: at once
        let same = s;
        s.go(1, 5_000, true);
        assert_eq!(s, same);
        s.go(0, 5_000, false);
        assert_eq!((s.share(0, 5_000), s.share(1, 5_000), s.settled(5_000)), (1.0, 0.0, true));
        // the widths: a round button, the open item, what is between
        assert_eq!(swap_width(0.0, 240.0), HEAD);
        assert_eq!(swap_width(1.0, 240.0), 240.0);
        assert_eq!(swap_width(0.5, 240.0), (HEAD + 240.0) / 2.0);
        assert!(swap_width(1.05, 240.0) > 240.0 && swap_width(-0.05, 240.0) < HEAD && swap_width(-9.0, 240.0) == 0.0);
        // the speaker and the microphone beside the list's button: the open one is as wide as a card's own slider with its button
        let sound_room = CARD_INNER - HEAD - ROW_GAP;
        assert_eq!(open_width(2, sound_room), CARD_INNER - 2.0 * (HEAD + ROW_GAP));
        assert_eq!(open_slider(open_width(1, sound_room)), SLIDER_W, "one alone keeps the slider it had");
        assert!(open_slider(open_width(2, sound_room)) < SLIDER_W);
        // displays: two keep the full slider, each further one takes a button's room from it, and four still have a slider to drag
        assert_eq!(open_slider(open_width(2, CARD_INNER)), SLIDER_W);
        assert_eq!(open_slider(open_width(3, CARD_INNER)), SLIDER_W - (HEAD + ROW_GAP));
        assert!(open_slider(open_width(4, CARD_INNER)) > 150.0);
    }

    #[test]
    fn the_bottom_rows_buttons_are_full_size_while_they_fit_and_smaller_when_more_are_shown() {
        let room = WIDTH - 2.0 * PAD;
        // the panel's own three, and up to five quick buttons beside them: as they were
        for count in [1usize, 3, 8] {
            assert_eq!(bottom_buttons(count, room), (BOTTOM_H, 6.0), "{count}");
        }
        // everything shown (eight quick buttons and the three): smaller, closer, still in one row
        let (size, gap) = bottom_buttons(11, room);
        assert!(size < BOTTOM_H && size >= 24.0, "{size}");
        assert!(11.0 * size + 10.0 * gap <= room);
        // never nothing, never more than the full size
        assert_eq!(bottom_buttons(0, room).0, BOTTOM_H);
        assert!(bottom_buttons(40, room).0 >= 20.0);
    }

    #[test]
    fn the_quick_buttons_are_the_ones_not_hidden_and_say_what_they_are_at() {
        let none = |_: &str| false;
        let state = QuickState { dark: true, awake: false, power: Some(PowerMode::Balanced) };
        assert_eq!(quick_buttons(&none, &state), Quick::ALL);
        // no power mode told by Windows: no such button
        let no_power = QuickState { power: None, ..state };
        assert!(!quick_buttons(&none, &no_power).contains(&Quick::Power));
        assert_eq!(quick_buttons(&none, &no_power).len(), 7);
        // hidden one by one, or the whole row
        let some = |k: &str| k == "quick.cast" || k == "quick.dark";
        assert_eq!(quick_buttons(&some, &state), [Quick::Awake, Quick::Power, Quick::NightLight, Quick::Focus, Quick::Airplane, Quick::Project]);
        assert!(quick_buttons(&|k| k == "quick", &state).is_empty());
        // lit, symbol and words
        assert!(Quick::Dark.lit(&state) && !Quick::Awake.lit(&state) && !Quick::Power.lit(&state) && !Quick::Cast.lit(&state));
        let fast = QuickState { power: Some(PowerMode::Performance), awake: true, dark: false };
        assert!(Quick::Power.lit(&fast) && Quick::Awake.lit(&fast) && !Quick::Dark.lit(&fast));
        assert_eq!([Quick::Power.icon(&state), Quick::Power.icon(&fast), Quick::Power.icon(&QuickState { power: Some(PowerMode::Efficiency), ..state })], [L::Gauge, L::Zap, L::Leaf]);
        assert_eq!(Quick::Power.tip(&state, TR), "Güç modu: dengeli");
        assert_eq!(Quick::Power.tip(&fast, EN), "Power mode: best performance");
        assert_eq!(Quick::Power.tip(&QuickState { power: Some(PowerMode::Efficiency), ..state }, EN), "Power mode: best power efficiency");
        // every button has a name in both languages, a key of its own, and either switches something itself or opens a page
        let mut keys = std::collections::HashSet::new();
        for q in Quick::ALL {
            assert!(!q.name(TR).is_empty() && !q.name(EN).is_empty() && q.name(TR) != q.name(EN), "{q:?}");
            assert!(q.key().starts_with("quick.") && keys.insert(q.key()), "{q:?}");
            assert_eq!(q.tip(&no_power, TR).is_empty(), false);
        }
        assert_eq!(Quick::ALL.iter().filter(|q| q.page().is_some()).count(), 4);
        assert_eq!(Quick::NightLight.page(), Some("ms-settings:nightlight"));
        // the parts that can be hidden have names in both languages too
        for part in PARTS {
            assert!(!part_name(part, TR).is_empty() && !part_name(part, EN).is_empty(), "{part}");
        }
    }

    #[test]
    fn what_is_hidden_is_kept_once_and_in_order() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(with_hidden(&[], "media", true), list(&["media"]));
        assert_eq!(with_hidden(&list(&["media"]), "wifi", true), list(&["wifi", "media"]));
        assert_eq!(with_hidden(&list(&["wifi", "media"]), "quick.cast", true), list(&["wifi", "media", "quick.cast"]));
        assert_eq!(with_hidden(&list(&["wifi", "media", "quick.cast"]), "quick.dark", true), list(&["wifi", "media", "quick.dark", "quick.cast"]));
        assert_eq!(with_hidden(&list(&["wifi", "media"]), "media", true), list(&["wifi", "media"]), "hidden twice is hidden once");
        assert_eq!(with_hidden(&list(&["wifi", "media"]), "wifi", false), list(&["media"]));
        assert_eq!(with_hidden(&list(&["media"]), "wifi", false), list(&["media"]), "shown again when it was not hidden: nothing");
        assert_eq!(with_hidden(&list(&["gelecek", "media", "media"]), "sound", true), list(&["media", "sound", "gelecek"]), "a name of a later version is kept");
    }

    #[test]
    fn the_line_names_who_uses_the_microphone_and_the_camera() {
        let u = InUse { microphone: vec!["chrome".into(), "Discord".into()], camera: vec!["Zoom".into()] };
        assert_eq!(in_use_text(&u, TR), "Mikrofon: chrome, Discord · Kamera: Zoom");
        assert_eq!(in_use_text(&u, EN), "Microphone: chrome, Discord · Camera: Zoom");
        assert_eq!(in_use_text(&InUse { microphone: Vec::new(), camera: vec!["Zoom".into()] }, TR), "Kamera: Zoom");
        assert_eq!(in_use_text(&InUse::default(), TR), "");
    }

    #[test]
    fn the_brightness_card_is_one_row_for_every_display_and_the_media_card_says_where_the_track_is() {
        let one = Shown { brightness: 1, ..Default::default() };
        assert_eq!(sections(&one), vec![Section::Brightness(1), Section::Bottom]);
        assert_eq!(section_height(&Section::Brightness(1)), BRIGHTNESS_H);
        assert_eq!(section_height(&Section::Brightness(4)), BRIGHTNESS_H, "the displays share one row");
        assert_eq!(height(&Shown { brightness: 3, ..one }), height(&one));
        assert_eq!(sections(&Shown { brightness: 0, ..one }), vec![Section::Bottom]);
        // the source's line, and the time
        assert_eq!(media_source_line("Chrome", 0, 1), "Chrome");
        assert_eq!(media_source_line("Chrome", 0, 2), "Chrome · 1/2");
        assert_eq!(media_source_line("Spotify", 2, 3), "Spotify · 3/3");
        assert_eq!(media_source_line("Spotify", 9, 3), "Spotify · 3/3", "a place past the end is the last");
        let now = std::time::Instant::now();
        let t = Timeline { duration_ms: 200_000, position_ms: 65_000, at: now };
        assert_eq!(media_time(&t, true, now), "1:05 / 3:20");
        assert_eq!(media_time(&t, true, now + std::time::Duration::from_secs(5)), "1:10 / 3:20");
        assert_eq!(media_time(&t, false, now + std::time::Duration::from_secs(5)), "1:05 / 3:20");
    }

    #[test]
    fn a_radios_button_shows_what_was_pressed_until_windows_says_so() {
        // nothing pressed: what Windows says
        assert_eq!(shown_power(Power::Off, None), Power::Off);
        assert_eq!(shown_power(Power::On, None), Power::On);
        // pressed on a moment ago, Windows still says off: on (and the other way round)
        assert_eq!(shown_power(Power::Off, Some((true, 300))), Power::On);
        assert_eq!(shown_power(Power::On, Some((false, 300))), Power::Off);
        // Windows never followed: back to what it says
        assert_eq!(shown_power(Power::Off, Some((true, RADIO_PENDING_MS))), Power::Off);
        // a radio that cannot be switched is not shown as switched
        assert_eq!(shown_power(Power::Disabled, Some((true, 0))), Power::Disabled);
    }

    #[test]
    fn the_wifi_tile_says_off_on_or_the_network_it_is_on() {
        assert_eq!(wifi_text(&wifi(Power::Off, &[]), None, TR), TileText { title: "Wi-Fi".into(), subtitle: "Kapalı".into(), active: false });
        assert_eq!(wifi_text(&wifi(Power::Off, &[]), None, EN).subtitle, "Off");
        assert_eq!(wifi_text(&wifi(Power::On, &[("Komşu", 3, false)]), None, TR), TileText { title: "Wi-Fi".into(), subtitle: "Bağlı değil".into(), active: true });
        assert_eq!(wifi_text(&wifi(Power::On, &[("Komşu", 3, false)]), None, EN).subtitle, "Not connected");
        assert_eq!(wifi_text(&wifi(Power::On, &[("Komşu", 3, false), ("Ev", 5, true)]), None, TR).subtitle, "Ev");
        assert_eq!(wifi_text(&wifi(Power::Disabled, &[]), None, EN).subtitle, "Unavailable");
        // Windows does not give the list (location is off): the name of the connection in use
        let on_wifi = net(Link::WiFi { name: "Ev".into(), bars: 4 }, Reach::Internet);
        assert_eq!(wifi_text(&wifi(Power::On, &[]), Some(&on_wifi), TR).subtitle, "Ev");
        assert_eq!(wifi_text(&wifi(Power::On, &[]), Some(&net(Link::Ethernet, Reach::Internet)), TR).subtitle, "Bağlı değil");
        // the symbol: off, on, the strength of the network it is on
        assert_eq!(wifi_icon(&wifi(Power::Off, &[])), L::WifiOff);
        assert_eq!(wifi_icon(&wifi(Power::On, &[("Komşu", 5, false)])), L::Wifi);
        assert_eq!(wifi_icon(&wifi(Power::On, &[("Ev", 2, true)])), L::WifiLow);
        assert_eq!(wifi_icon(&wifi(Power::On, &[("Ev", 5, true)])), L::Wifi);
        assert_eq!([0u8, 1, 3, 5].map(signal_icon), [L::WifiZero, L::WifiLow, L::WifiHigh, L::Wifi]);
    }

    #[test]
    fn the_network_tile_says_what_the_machine_is_connected_through_in_both_languages() {
        let eth = net(Link::Ethernet, Reach::Internet);
        assert_eq!(network_text(&eth, TR), TileText { title: "Ethernet".into(), subtitle: "Bağlı".into(), active: true });
        // a PC with a wired card: its tile, which turns the card on and off (the user, 2026-11-27)
        assert_eq!(wired_text(false, Some(&eth), TR), TileText { title: "Ethernet".into(), subtitle: "Kapalı".into(), active: false });
        assert_eq!(wired_text(true, Some(&eth), TR).subtitle, "Bağlı");
        assert!(wired_text(true, Some(&eth), TR).active);
        assert_eq!(wired_text(true, Some(&net(Link::Ethernet, Reach::Local)), TR).subtitle, "İnternet yok");
        assert_eq!(wired_text(true, None, TR).subtitle, "Bağlı değil", "on, with nothing on the cable");
        assert_eq!(network_text(&eth, EN), TileText { title: "Ethernet".into(), subtitle: "Connected".into(), active: true });
        let none = net(Link::Disconnected, Reach::None);
        assert_eq!(network_text(&none, TR), TileText { title: "Ağ".into(), subtitle: "Bağlı değil".into(), active: false });
        assert_eq!(network_text(&none, EN), TileText { title: "Network".into(), subtitle: "Not connected".into(), active: false });
        let wifi = net(Link::WiFi { name: "Evim".into(), bars: 5 }, Reach::Internet);
        assert_eq!(network_text(&wifi, TR), TileText { title: "Evim".into(), subtitle: "Wi-Fi · Güçlü".into(), active: true });
        assert_eq!(network_text(&wifi, EN).subtitle, "Wi-Fi · Strong");
        let weak = net(Link::WiFi { name: "Evim".into(), bars: 1 }, Reach::Internet);
        assert_eq!(network_text(&weak, EN).subtitle, "Wi-Fi · Weak");
        assert_eq!(network_text(&net(Link::WiFi { name: "Evim".into(), bars: 3 }, Reach::Internet), TR).subtitle, "Wi-Fi · Orta");
        // a Wi-Fi without internet says so instead of a strength; a nameless one is called Wi-Fi
        let local = net(Link::WiFi { name: "Evim".into(), bars: 5 }, Reach::Local);
        assert_eq!(network_text(&local, TR).subtitle, "Wi-Fi · İnternet yok");
        assert_eq!(network_text(&local, EN).subtitle, "Wi-Fi · No internet");
        assert_eq!(network_text(&net(Link::WiFi { name: "  ".into(), bars: 2 }, Reach::Internet), TR).title, "Wi-Fi");
        assert_eq!(network_text(&net(Link::Ethernet, Reach::Local), EN).subtitle, "No internet");
        assert_eq!(network_text(&net(Link::Cellular, Reach::Internet), EN).title, "Mobile data");
        assert_eq!(network_text(&net(Link::Other, Reach::Internet), TR).subtitle, "Bağlı");
    }

    #[test]
    fn the_bluetooth_tile_says_off_on_or_who_is_connected() {
        assert_eq!(bluetooth_text(&bt(Power::Off, &[]), TR), TileText { title: "Bluetooth".into(), subtitle: "Kapalı".into(), active: false });
        assert_eq!(bluetooth_text(&bt(Power::Off, &[]), EN).subtitle, "Off");
        assert_eq!(bluetooth_text(&bt(Power::On, &[]), TR), TileText { title: "Bluetooth".into(), subtitle: "Açık".into(), active: true });
        assert_eq!(bluetooth_text(&bt(Power::On, &[]), EN).subtitle, "On");
        assert_eq!(bluetooth_text(&bt(Power::On, &["Kulaklık"]), TR).subtitle, "Kulaklık");
        assert_eq!(bluetooth_text(&bt(Power::On, &["Kulaklık", "Fare", "Klavye"]), EN).subtitle, "Kulaklık +2");
        assert_eq!(bluetooth_text(&bt(Power::Disabled, &[]), TR).subtitle, "Kullanılamıyor");
        assert_eq!(bluetooth_text(&bt(Power::Disabled, &[]), EN).subtitle, "Unavailable");
        assert!(!bluetooth_text(&bt(Power::Disabled, &[]), TR).active);
        // a radio that is off lists nobody even if a stale name were there
        assert_eq!(bluetooth_text(&bt(Power::Off, &["Kulaklık"]), TR).subtitle, "Kapalı");
    }

    #[test]
    fn the_battery_tile_says_the_percent_and_what_it_is_doing_in_the_languages_own_order() {
        assert_eq!(battery_text(&bat(Some(63), false, false, false), TR), TileText { title: "Pil".into(), subtitle: "%63".into(), active: false });
        assert_eq!(battery_text(&bat(Some(63), false, false, false), EN), TileText { title: "Battery".into(), subtitle: "63%".into(), active: false });
        assert_eq!(battery_text(&bat(Some(80), true, true, false), TR).subtitle, "%80 · Şarj oluyor");
        assert_eq!(battery_text(&bat(Some(80), true, true, false), EN).subtitle, "80% · Charging");
        assert!(battery_text(&bat(Some(80), true, true, false), TR).active);
        assert_eq!(battery_text(&bat(Some(100), false, true, false), EN).subtitle, "100% · Plugged in");
        assert_eq!(battery_text(&bat(Some(18), false, false, true), TR).subtitle, "%18 · Pil tasarrufu");
        assert_eq!(battery_text(&bat(Some(18), false, false, true), EN).subtitle, "18% · Battery saver");
        assert_eq!(battery_text(&bat(None, false, true, false), TR).subtitle, "Prize takılı");
        assert_eq!(battery_text(&bat(None, false, false, false), TR).subtitle, "");
    }

    #[test]
    fn the_sound_card_and_the_media_card_have_their_lines() {
        let s = AudioSnapshot { available: true, volume: 0.5, device: "Hoparlör".into(), ..Default::default() };
        assert_eq!(sound_device_text(&s, TR), "Hoparlör");
        assert_eq!(sound_device_text(&AudioSnapshot { available: true, ..Default::default() }, TR), "Ses aygıtı");
        assert_eq!(sound_device_text(&AudioSnapshot { available: true, ..Default::default() }, EN), "Sound device");
        let m = MediaSession { app: "Spotify".into(), title: "Şarkı".into(), artist: "Sanatçı".into(), playing: true, can_previous: true, can_next: true, can_toggle: true, artwork: None };
        assert_eq!(media_lines(&m, TR), ("Spotify".into(), "Şarkı".into(), "Sanatçı".into()));
        let untitled = MediaSession { title: " ".into(), artist: "x".into(), ..m.clone() };
        assert_eq!(media_lines(&untitled, TR), ("Spotify".into(), "Spotify".into(), String::new()));
        let nameless = MediaSession { app: String::new(), title: String::new(), ..m };
        assert_eq!(media_lines(&nameless, TR).1, "Şimdi çalıyor");
        assert_eq!(media_lines(&nameless, EN).1, "Now playing");
    }

    #[test]
    fn every_symbol_follows_its_state() {
        assert_eq!(network_icon(NetworkGlyph::Off), L::WifiOff);
        assert_eq!(network_icon(NetworkGlyph::Ethernet), L::EthernetPort);
        let steps: Vec<L> = (0..=3).map(|s| network_icon(NetworkGlyph::WiFi(s))).collect();
        assert_eq!(steps, vec![L::WifiZero, L::WifiLow, L::WifiHigh, L::Wifi]);
        assert_eq!(volume_icon(VolumeGlyph::Muted), L::VolumeX);
        assert_eq!(volume_icon(VolumeGlyph::Unavailable), L::VolumeOff);
        let levels = [VolumeGlyph::Silent, VolumeGlyph::Low, VolumeGlyph::High].map(volume_icon);
        assert_eq!(levels, [L::Volume, L::Volume1, L::Volume2]);
        assert_eq!(bluetooth_icon(&bt(Power::Off, &[])), L::BluetoothOff);
        assert_eq!(bluetooth_icon(&bt(Power::Disabled, &[])), L::BluetoothOff);
        assert_eq!(bluetooth_icon(&bt(Power::On, &[])), L::Bluetooth);
        assert_eq!(bluetooth_icon(&bt(Power::On, &["x"])), L::BluetoothConnected);
        assert_eq!(battery_icon(&bat(Some(5), false, false, false)), L::BatteryWarning);
        assert_eq!(battery_icon(&bat(Some(30), false, false, false)), L::BatteryLow);
        assert_eq!(battery_icon(&bat(Some(50), false, false, false)), L::BatteryMedium);
        assert_eq!(battery_icon(&bat(Some(95), false, false, false)), L::BatteryFull);
        assert_eq!(battery_icon(&bat(Some(5), true, true, false)), L::BatteryCharging, "charging wins over low");
        assert_eq!(battery_icon(&bat(None, false, false, false)), L::Battery);
    }

    #[test]
    fn the_speaker_flips_the_state_the_previous_click_asked_for_until_windows_has_answered() {
        // nothing pending: flip what the snapshot says
        assert!(mute_target(false, None));
        assert!(!mute_target(true, None));
        // two quick clicks: the first asked for mute and the snapshot still says "not muted"; the second must ask for unmute
        assert!(!mute_target(false, Some((true, 80))));
        // and a third flips again
        assert!(mute_target(false, Some((false, 150))));
        // long after, the snapshot is the truth again (the write may have been refused)
        assert!(mute_target(false, Some((true, MUTE_PENDING_MS))));
        assert!(!mute_target(true, Some((false, MUTE_PENDING_MS + 500))));
    }

    #[test]
    fn the_arrow_keys_move_a_slider_by_one_step_and_shift_or_the_page_keys_by_ten() {
        // a volume with 51 positions (50 parts of 2 %)
        let close = |got: Option<f32>, want: f32| got.is_some_and(|g| (g - want).abs() < 1e-5);
        let parts = slider_parts(51);
        assert_eq!(parts, 50);
        let one = 1.0 / 50.0;
        assert!(close(slider_key(0.5, "right", false, parts), 0.5 + one));
        assert!(close(slider_key(0.5, "up", false, parts), 0.5 + one));
        assert!(close(slider_key(0.5, "left", false, parts), 0.5 - one));
        assert!(close(slider_key(0.5, "down", false, parts), 0.5 - one));
        let ten = 10.0 / 50.0;
        assert!(close(slider_key(0.5, "right", true, parts), 0.5 + ten));
        assert!(close(slider_key(0.5, "pagedown", false, parts), 0.5 - ten));
        assert!(close(slider_key(0.5, "pageup", false, parts), 0.5 + ten));
        assert!(close(slider_key(0.5, "home", false, parts), 0.0));
        assert!(close(slider_key(0.5, "end", false, parts), 1.0));
        // the ends hold
        assert!(close(slider_key(1.0, "right", false, parts), 1.0));
        assert!(close(slider_key(0.0, "left", true, parts), 0.0));
        // other keys are not the slider's
        assert_eq!(slider_key(0.5, "tab", false, parts), None);
        assert_eq!(slider_key(0.5, "a", false, parts), None);
        // an endpoint that did not say how many positions it has is taken in whole percent, like the write does
        assert_eq!(slider_parts(0), 100);
        assert_eq!(slider_parts(1), 100);
        assert_eq!(slider_parts(1000), 100);
        // a coarse endpoint (11 positions) moves by its own step
        assert!(close(slider_key(0.3, "right", false, slider_parts(11)), 0.4));
    }

    #[test]
    fn the_pointer_maps_to_the_slider_the_way_the_thumb_is_placed() {
        // the inverse of liquid::slider_layout: the thumb's centre travels half a resting thumb in from both edges
        let (left, w) = (100.0, 240.0);
        let half = crate::liquid::SL_REST_W / 2.0;
        assert_eq!(slider_fraction_at(left + half, left, w), 0.0);
        assert_eq!(slider_fraction_at(left + w - half, left, w), 1.0);
        assert_eq!(slider_fraction_at(left - 50.0, left, w), 0.0, "left of the box");
        assert_eq!(slider_fraction_at(left + w + 50.0, left, w), 1.0, "right of the box");
        for frac in [0.0f32, 0.25, 0.5, 0.99, 1.0] {
            let centre = left + crate::liquid::slider_layout(w, frac).centre(false);
            assert!((slider_fraction_at(centre, left, w) - frac).abs() < 1e-4, "{frac}");
        }
        // a box narrower than the thumb does not divide by zero
        assert!(slider_fraction_at(5.0, 0.0, 0.0).is_finite());
    }

    #[test]
    fn every_turkish_text_the_model_uses_has_an_english_pair() {
        // (tr_en panics on a missing pair; this walks every state)
        for link in [Link::Disconnected, Link::Ethernet, Link::WiFi { name: "a".into(), bars: 4 }, Link::Cellular, Link::Other] {
            for reach in [Reach::None, Reach::Local, Reach::Internet] {
                let _ = network_text(&net(link.clone(), reach), EN);
            }
        }
        for p in [Power::Off, Power::On, Power::Disabled] {
            let _ = bluetooth_text(&bt(p, &[]), EN);
        }
        for (c, a, s) in [(false, false, false), (true, true, false), (false, true, false), (false, false, true)] {
            let _ = battery_text(&bat(Some(40), c, a, s), EN);
        }
        for bars in 0..=5 {
            let _ = signal_word(bars, EN);
        }
        let _ = sound_device_text(&AudioSnapshot::default(), EN);
        let _ = media_lines(&MediaSession { app: String::new(), title: String::new(), artist: String::new(), playing: false, can_previous: false, can_next: false, can_toggle: false, artwork: None }, EN);
    }
}

#[cfg(test)]
mod mixer_tests {
    use super::{Detail, detail_rows, mixer_rows};

    /// The mixer opens from the sound card like the lists of devices, and its rows are the programs that play.
    #[test]
    fn the_mixer_is_a_detail_of_its_own_above_the_sound_card() {
        assert!(Detail::Mixer.upwards());
        assert_eq!(detail_rows(Detail::Mixer, None, None, None, None, None), 0, "its rows are counted by the panel, which holds the programs");
        assert_eq!(mixer_rows(0), 2, "nothing plays: a line that says so, and the way to Windows' page");
        assert_eq!(mixer_rows(1), 2);
        assert_eq!(mixer_rows(5), 6);
    }
}
