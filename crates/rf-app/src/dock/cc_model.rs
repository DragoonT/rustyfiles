//! The control centre's decisions, with no window in them: which tiles and sections a machine gets, how tall the panel is for them,
//! the words on each tile for a snapshot (Turkish or English), which symbol a state gets, and the slider's pointer arithmetic. The
//! view (`control_centre.rs`) only puts these on the screen.
//!
//! The words are passed a translator (`Words`) instead of reading the app's language, so a test can ask for both languages in the
//! same run without touching the global one; every Turkish text used here has its English pair in `i18n_en` (a test checks that).

use gpui_kit::assets::IconName as L;
use rf_win::sysstatus::audio::{AudioSnapshot, VolumeGlyph};
use rf_win::sysstatus::bluetooth::{BluetoothSnapshot, Power};
use rf_win::sysstatus::media::MediaSession;
use rf_win::sysstatus::network::{Link, NetworkGlyph, NetworkSnapshot, Reach};
use rf_win::sysstatus::power::BatterySnapshot;

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
/// One output device in the list.
pub const DEVICE_ROW_H: f32 = 30.0;
/// The list is a card of its own above the sound card (so that opening it moves nothing the user has just clicked); this is the
/// padding above and below its rows.
pub const DEVICE_CARD_PAD: f32 = 8.0;
/// A click on the speaker is the "latest" state for this long, while the audio thread has not yet answered with a snapshot.
pub const MUTE_PENDING_MS: u64 = 1_000;
/// A slider's width in its card (the card's inner width less the buttons at its ends).
pub const SLIDER_W: f32 = 240.0;

// ------------------------------------------------------------------ what is shown

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tile {
    Network,
    Bluetooth,
    Battery,
}

/// The connectivity tiles this machine gets, in their order. A tile whose source is not there (no Bluetooth radio, no battery, no
/// network source) is left out, not shown dead; a source that has not answered yet (`None`) gets none either.
pub fn tiles(net: Option<&NetworkSnapshot>, bt: Option<&BluetoothSnapshot>, battery: Option<&BatterySnapshot>) -> Vec<Tile> {
    let mut v = Vec::new();
    if net.is_some_and(|n| n.available) {
        v.push(Tile::Network);
    }
    if bt.is_some_and(|b| b.available) {
        v.push(Tile::Bluetooth);
    }
    if battery.is_some_and(|b| b.present) {
        v.push(Tile::Battery);
    }
    v
}

/// What decides the sections of the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Shown {
    pub tiles: usize,
    /// A media session exists.
    pub media: bool,
    /// A sound device exists.
    pub sound: bool,
    /// The list of outputs is open: how many rows it has (devices and the link under them); `None` while closed.
    pub devices: Option<usize>,
    /// The monitor answered DDC/CI.
    pub brightness: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section {
    /// The tiles, two to a row: the number of rows.
    Tiles(usize),
    Media,
    /// The list of output devices, a card above the sound card: the number of its rows (devices and the link under them).
    Devices(usize),
    Sound,
    Brightness,
    Bottom,
}

pub fn sections(s: &Shown) -> Vec<Section> {
    let mut v = Vec::new();
    if s.tiles > 0 {
        v.push(Section::Tiles(s.tiles.div_ceil(2)));
    }
    if s.media {
        v.push(Section::Media);
    }
    if s.sound {
        // (above the sound card: the panel grows upwards, so what is under the pointer when the list opens stays where it is)
        if let Some(rows) = s.devices {
            v.push(Section::Devices(rows));
        }
        v.push(Section::Sound);
    }
    if s.brightness {
        v.push(Section::Brightness);
    }
    v.push(Section::Bottom);
    v
}

pub fn section_height(s: &Section) -> f32 {
    match s {
        Section::Tiles(rows) => *rows as f32 * TILE_H + rows.saturating_sub(1) as f32 * GAP,
        Section::Media => MEDIA_H,
        Section::Devices(rows) => 2.0 * DEVICE_CARD_PAD + *rows as f32 * DEVICE_ROW_H,
        Section::Sound => SOUND_H,
        Section::Brightness => BRIGHTNESS_H,
        Section::Bottom => BOTTOM_H,
    }
}

/// The panel's height: the sections, the gaps between them and the padding round them. (The bottom row is always there.)
pub fn height(s: &Shown) -> f32 {
    let secs = sections(s);
    let sum: f32 = secs.iter().map(section_height).sum();
    2.0 * PAD + sum + GAP * (secs.len().saturating_sub(1)) as f32
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
        NetworkSnapshot { available: true, link, reach }
    }

    fn bt(power: Power, connected: &[&str]) -> BluetoothSnapshot {
        BluetoothSnapshot { available: true, power, connected: connected.iter().map(|s| s.to_string()).collect() }
    }

    fn bat(percent: Option<u8>, charging: bool, on_ac: bool, saver: bool) -> BatterySnapshot {
        BatterySnapshot { present: true, percent, charging, on_ac, saver, minutes_left: None }
    }

    #[test]
    fn a_desktop_with_a_network_and_a_bluetooth_radio_gets_two_tiles_and_a_laptop_three() {
        let n = net(Link::Ethernet, Reach::Internet);
        let b = bt(Power::Off, &[]);
        let none = BatterySnapshot::default();
        assert_eq!(tiles(Some(&n), Some(&b), Some(&none)), vec![Tile::Network, Tile::Bluetooth], "no battery: no battery tile");
        assert_eq!(tiles(Some(&n), Some(&b), Some(&bat(Some(50), false, false, false))), vec![Tile::Network, Tile::Bluetooth, Tile::Battery]);
        // no Bluetooth radio on this machine: no tile, not a dead one
        assert_eq!(tiles(Some(&n), Some(&BluetoothSnapshot::default()), None), vec![Tile::Network]);
        // a source that has not answered yet gives nothing; no network source gives no network tile
        assert_eq!(tiles(Some(&n), None, None), vec![Tile::Network]);
        assert_eq!(tiles(Some(&NetworkSnapshot::default()), Some(&b), None), vec![Tile::Bluetooth]);
        assert!(tiles(None, None, None).is_empty());
    }

    #[test]
    fn the_sections_follow_what_exists_and_the_bottom_row_is_always_there() {
        assert_eq!(sections(&Shown::default()), vec![Section::Bottom]);
        let all = Shown { tiles: 3, media: true, sound: true, devices: None, brightness: true };
        assert_eq!(sections(&all), vec![Section::Tiles(2), Section::Media, Section::Sound, Section::Brightness, Section::Bottom]);
        // the list of outputs is a card ABOVE the sound card
        let some = Shown { tiles: 2, media: false, sound: true, devices: Some(6), brightness: false };
        assert_eq!(sections(&some), vec![Section::Tiles(1), Section::Devices(6), Section::Sound, Section::Bottom]);
        // tiles go two to a row
        assert_eq!(sections(&Shown { tiles: 1, ..Default::default() })[0], Section::Tiles(1));
        assert_eq!(sections(&Shown { tiles: 4, ..Default::default() })[0], Section::Tiles(2));
    }

    #[test]
    fn the_height_is_the_sections_the_gaps_and_the_padding() {
        // nothing but the bottom row
        assert_eq!(height(&Shown::default()), 2.0 * PAD + BOTTOM_H);
        // two tiles (one row), media, sound, brightness, bottom: 12 + 64 + 88 + 72 + 72 + 36 + 4 gaps of 10 + 12
        let all = Shown { tiles: 2, media: true, sound: true, devices: None, brightness: true };
        assert_eq!(height(&all), 2.0 * PAD + TILE_H + MEDIA_H + SOUND_H + BRIGHTNESS_H + BOTTOM_H + 4.0 * GAP);
        // three tiles are two rows: one more tile height and a gap
        let three = Shown { tiles: 3, ..all };
        assert_eq!(height(&three), height(&all) + TILE_H + GAP);
        // an open list of 5 outputs and the link is a card of its own: a gap, its padding and its rows
        let open = Shown { devices: Some(6), ..all };
        assert_eq!(height(&open), height(&all) + GAP + 2.0 * DEVICE_CARD_PAD + 6.0 * DEVICE_ROW_H);
        // closing it gives the height back; a media card that comes and goes moves the height by its own and a gap
        assert_eq!(height(&Shown { devices: None, ..open }), height(&all));
        assert_eq!(height(&Shown { media: false, ..all }), height(&all) - MEDIA_H - GAP);
    }

    #[test]
    fn the_network_tile_says_what_the_machine_is_connected_through_in_both_languages() {
        let eth = net(Link::Ethernet, Reach::Internet);
        assert_eq!(network_text(&eth, TR), TileText { title: "Ethernet".into(), subtitle: "Bağlı".into(), active: true });
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
