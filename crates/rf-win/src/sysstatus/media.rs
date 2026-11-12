//! "Now playing": the system's media session (WinRT `GlobalSystemMediaTransportControlsSessionManager`), the same one Windows' own
//! volume flyout shows. It activates for a program with no package identity (checked on this build: the manager answered, one session
//! was listed, its properties and thumbnail were readable), so no package is needed.
//!
//! RUNS only while the panel is open ([`subscribe`] starts a thread of its own, MTA; dropping the returned [`Media`] ends it and takes
//! every handler off). EVENTS, not polling: the manager's `SessionsChanged` and `CurrentSessionChanged`, and on the current session
//! `PlaybackInfoChanged` and `MediaPropertiesChanged`. They arrive in bursts with half-filled properties, so the thread waits 100 ms
//! for a burst to end and then reads.
//!
//! THE ARTWORK is decoded here, on the media thread, and cut down to what the panel draws (a 128 px square, 56 px at 2x and a bit
//! more): the UI thread only gets pixels. Windows leaves the thumbnail empty for a moment after a track change, so a new track whose
//! picture is not there yet keeps the previous picture of the same app for one more look (1.5 s later, or when Windows says the
//! properties changed, whichever comes first) and is read again until a picture has really been decoded FOR THIS TRACK; if none
//! comes, the old cover is dropped rather than left on the wrong song. Another app's session starts without one. ([`ArtState`] is
//! that rule, pure and tested.)
//!
//! Only the session Windows calls current is shown (the one the user would most likely want to control). Choosing between several
//! sessions is not offered.
//!
//! WRITES: [`Media::transport`] is the user's click on previous, play / pause or next, sent to the session that is shown.

use std::sync::Arc;
use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager, GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference};

use super::{Subscription, init_mta};

/// The side of the square the artwork is cut down to, in px.
pub const ART_PX: u32 = 128;

/// A decoded picture: BGRA bytes, straight alpha, `size` x `size`.
#[derive(Debug)]
pub struct Artwork {
    pub size: u32,
    pub bgra: Vec<u8>,
    /// Which picture this is (a new one has a new number: the interface compares these, not the bytes).
    pub id: u64,
}

#[derive(Clone, Debug)]
pub struct MediaSession {
    /// The program that plays, as a short name (`app_name` of the session's AppUserModelID).
    pub app: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub can_previous: bool,
    pub can_next: bool,
    pub can_toggle: bool,
    pub artwork: Option<Arc<Artwork>>,
}

impl PartialEq for MediaSession {
    fn eq(&self, o: &Self) -> bool {
        self.app == o.app
            && self.title == o.title
            && self.artist == o.artist
            && self.playing == o.playing
            && self.can_previous == o.can_previous
            && self.can_next == o.can_next
            && self.can_toggle == o.can_toggle
            && self.artwork.as_ref().map(|a| a.id) == o.artwork.as_ref().map(|a| a.id)
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MediaSnapshot {
    /// The session manager could be had. False: this machine has no such service (the card is left out for good).
    pub available: bool,
    /// The session the system calls current, if any.
    pub session: Option<MediaSession>,
}

/// What the three buttons do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Previous,
    PlayPause,
    Next,
}

// ------------------------------------------------------------------ pure

/// What a picture was decoded for: (program, title, artist).
pub type ArtKey = (String, String, String);

/// What the reader does after offering a decoding result to [`ArtState`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArtNext {
    /// Nothing more to do for this track.
    Done,
    /// The old cover is still shown and the new one may yet arrive: read again soon.
    RetryLater,
}

/// The cover the card shows and which track it belongs to.
#[derive(Debug, Default)]
pub struct ArtState {
    key: Option<ArtKey>,
    art: Option<Arc<Artwork>>,
    /// `art` was decoded for `key` (not left over from the track before).
    fresh: bool,
    /// Looks at `key` that found no picture.
    misses: u8,
    next_id: u64,
}

impl ArtState {
    pub fn new() -> Self {
        ArtState { next_id: 1, ..Default::default() }
    }

    /// The picture to show now.
    pub fn picture(&self) -> Option<Arc<Artwork>> {
        self.art.clone()
    }

    /// Whether the thumbnail has to be decoded again for this track: a different track, or the same one whose own picture has not
    /// been found yet.
    pub fn needs_decode(&self, key: &ArtKey) -> bool {
        self.key.as_ref() != Some(key) || !self.fresh
    }

    /// No session: nothing is shown and nothing is remembered.
    pub fn clear(&mut self) {
        self.key = None;
        self.art = None;
        self.fresh = false;
        self.misses = 0;
    }

    /// The result of decoding the thumbnail of `key` (`None`: there was none, or it was not a picture).
    pub fn offer(&mut self, key: ArtKey, decoded: Option<(Vec<u8>, u32)>) -> ArtNext {
        let same_app = self.key.as_ref().is_some_and(|k| k.0 == key.0);
        if self.key.as_ref() != Some(&key) {
            self.misses = 0;
        }
        self.key = Some(key);
        match decoded {
            Some((bgra, size)) => {
                let id = self.next_id;
                self.next_id += 1;
                self.art = Some(Arc::new(Artwork { size, bgra, id }));
                self.fresh = true;
                self.misses = 0;
                ArtNext::Done
            }
            None => {
                self.fresh = false;
                self.misses = self.misses.saturating_add(1);
                if same_app && self.art.is_some() && self.misses < 2 {
                    // the previous cover stays for one more look: the new one is usually a moment behind the title
                    ArtNext::RetryLater
                } else {
                    // the track really has no picture (or another app's stale one): no cover rather than the wrong one
                    self.art = None;
                    ArtNext::Done
                }
            }
        }
    }
}

/// A short name for the program of a session from its AppUserModelID: `Spotify.exe` is "Spotify"; a packaged app's
/// `SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify` is "Spotify" (the part after the `!`), and when that is a dotted name its last
/// part (`Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic` is "ZuneMusic"). The first letter is a capital.
pub fn app_name(aumid: &str) -> String {
    let s = aumid.trim();
    let tail = s.rsplit_once('!').map_or(s, |(_, app)| app);
    // a path ("C:\Program Files\...\app.exe")
    let tail = tail.rsplit(['\\', '/']).next().unwrap_or(tail);
    let lower = tail.to_ascii_lowercase();
    let tail = if lower.ends_with(".exe") { &tail[..tail.len() - 4] } else { tail };
    // a family name with no app id ("Name_hash"): drop the hash
    let tail = if s.contains('!') { tail } else { tail.split('_').next().unwrap_or(tail) };
    let name = tail.rsplit('.').find(|p| !p.is_empty()).unwrap_or(tail);
    let mut chars = name.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// BGRA (straight alpha) from RGBA, in place.
pub fn rgba_to_bgra(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        p.swap(0, 2);
    }
}

/// Decodes a picture file's bytes to a square `size` x `size` BGRA bitmap (cut to the middle, not stretched). `None` for bytes that
/// are no picture.
pub fn decode_artwork(bytes: &[u8], size: u32, id: u64) -> Option<Artwork> {
    let img = image::load_from_memory(bytes).ok()?;
    let fitted = img.resize_to_fill(size, size, image::imageops::FilterType::Triangle);
    let mut rgba = fitted.to_rgba8().into_raw();
    rgba_to_bgra(&mut rgba);
    Some(Artwork { size, bgra: rgba, id })
}

// ------------------------------------------------------------------ the thread

enum Msg {
    Read,
    Transport(Transport),
    Quit,
}

const BURST: Duration = Duration::from_millis(100);

/// The follower's end: dropping it stops the thread. Also takes the user's clicks on the buttons.
pub struct Media {
    tx: Sender<Msg>,
    _end: Subscription,
}

impl Media {
    /// The user clicked previous, play / pause or next.
    pub fn transport(&self, t: Transport) {
        let _ = self.tx.send(Msg::Transport(t));
    }
}

/// Follows the media session (see the module docs). `callback` runs on the media thread: at once with the first reading, then at
/// every change.
pub fn subscribe(callback: impl Fn(&MediaSnapshot) + Send + Sync + 'static) -> Media {
    let (tx, rx) = channel::<Msg>();
    let wake = tx.clone();
    let spawned = std::thread::Builder::new().name("rf-media".into()).spawn(move || {
        init_mta();
        let Some(mgr) = manager() else {
            callback(&MediaSnapshot::default());
            return;
        };
        let poke = || {
            let tx = wake.clone();
            move || {
                let _ = tx.send(Msg::Read);
            }
        };
        let (a, b) = (poke(), poke());
        let t_sessions = mgr
            .SessionsChanged(&TypedEventHandler::new(move |_, _| {
                a();
                Ok(())
            }))
            .ok();
        let t_current = mgr
            .CurrentSessionChanged(&TypedEventHandler::new(move |_, _| {
                b();
                Ok(())
            }))
            .ok();
        let mut state = Reader { wake: wake.clone(), current: None, art: ArtState::new() };
        let mut last: Option<MediaSnapshot> = None;
        let tell = |s: MediaSnapshot, last: &mut Option<MediaSnapshot>| {
            if last.as_ref() != Some(&s) {
                callback(&s);
                *last = Some(s);
            }
        };
        tell(state.read(&mgr), &mut last);
        'run: while let Ok(msg) = rx.recv() {
            match msg {
                Msg::Quit => break,
                Msg::Transport(t) => {
                    state.send(t);
                    continue;
                }
                Msg::Read => {}
            }
            loop {
                match rx.recv_timeout(BURST) {
                    Ok(Msg::Quit) => break 'run,
                    Ok(Msg::Transport(t)) => state.send(t),
                    Ok(Msg::Read) => continue,
                    Err(_) => break,
                }
            }
            tell(state.read(&mgr), &mut last);
        }
        state.leave();
        take_off(&mgr, t_sessions, t_current);
    });
    if let Err(e) = spawned {
        log::warn!("media: the thread did not start: {e}");
    }
    let quit = tx.clone();
    Media { tx, _end: Subscription::new(move || { let _ = quit.send(Msg::Quit); }) }
}

fn take_off(mgr: &GlobalSystemMediaTransportControlsSessionManager, sessions: Option<i64>, current: Option<i64>) {
    if let Some(t) = sessions {
        let _ = mgr.RemoveSessionsChanged(t);
    }
    if let Some(t) = current {
        let _ = mgr.RemoveCurrentSessionChanged(t);
    }
}

fn manager() -> Option<GlobalSystemMediaTransportControlsSessionManager> {
    GlobalSystemMediaTransportControlsSessionManager::RequestAsync().ok()?.join().ok()
}

/// Reads once on the calling thread (a probe). Blocks.
pub fn read_once() -> MediaSnapshot {
    init_mta();
    let Some(mgr) = manager() else { return MediaSnapshot::default() };
    let (tx, _rx) = channel::<Msg>();
    Reader { wake: tx, current: None, art: ArtState::new() }.read(&mgr)
}

/// What the thread keeps between two readings: the session its handlers are on, and the artwork it has decoded.
struct Reader {
    wake: Sender<Msg>,
    current: Option<(GlobalSystemMediaTransportControlsSession, String, i64, i64)>,
    art: ArtState,
}

/// How long after a track change that came without its picture the thumbnail is looked at again.
const ART_RETRY: Duration = Duration::from_millis(1500);

impl Reader {
    /// Reads the current session, moving the handlers onto it when it is another one than before.
    fn read(&mut self, mgr: &GlobalSystemMediaTransportControlsSessionManager) -> MediaSnapshot {
        let Ok(session) = mgr.GetCurrentSession() else {
            self.leave();
            self.art.clear();
            return MediaSnapshot { available: true, session: None };
        };
        let id = session.SourceAppUserModelId().map(|s| s.to_string()).unwrap_or_default();
        if self.current.as_ref().is_none_or(|(s, ..)| *s != session) {
            self.leave();
            let (a, b) = (self.wake.clone(), self.wake.clone());
            let t1 = session
                .PlaybackInfoChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = a.send(Msg::Read);
                    Ok(())
                }))
                .unwrap_or(0);
            let t2 = session
                .MediaPropertiesChanged(&TypedEventHandler::new(move |_, _| {
                    let _ = b.send(Msg::Read);
                    Ok(())
                }))
                .unwrap_or(0);
            self.current = Some((session.clone(), id.clone(), t1, t2));
        }
        let app = app_name(&id);
        let (playing, can_previous, can_next, can_toggle) = match session.GetPlaybackInfo() {
            Ok(info) => {
                let c = info.Controls().ok();
                let flag = |f: fn(&windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackControls) -> windows::core::Result<bool>| c.as_ref().and_then(|c| f(c).ok()).unwrap_or(false);
                (
                    info.PlaybackStatus().ok() == Some(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing),
                    flag(|c| c.IsPreviousEnabled()),
                    flag(|c| c.IsNextEnabled()),
                    flag(|c| c.IsPlayPauseToggleEnabled()) || flag(|c| c.IsPlayEnabled()) || flag(|c| c.IsPauseEnabled()),
                )
            }
            Err(_) => (false, false, false, false),
        };
        let (mut title, mut artist) = (String::new(), String::new());
        if let Some(props) = session.TryGetMediaPropertiesAsync().ok().and_then(|op| op.join().ok()) {
            title = props.Title().map(|s| s.to_string()).unwrap_or_default();
            artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();
            let key = (app.clone(), title.clone(), artist.clone());
            if self.art.needs_decode(&key) {
                let decoded = props.Thumbnail().ok().and_then(|t| stream_bytes(&t)).and_then(|b: Vec<u8>| decode_artwork(&b, ART_PX, 0)).map(|a| (a.bgra, a.size));
                if self.art.offer(key, decoded) == ArtNext::RetryLater {
                    // Windows fills the thumbnail in after the title; it usually says so with an event, and this look is the net under it
                    let wake = self.wake.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(ART_RETRY);
                        let _ = wake.send(Msg::Read);
                    });
                }
            }
        }
        MediaSnapshot { available: true, session: Some(MediaSession { app, title, artist, playing, can_previous, can_next, can_toggle, artwork: self.art.picture() }) }
    }

    fn send(&self, t: Transport) {
        let Some((s, ..)) = self.current.as_ref() else { return };
        // (the answer is a bool: a program may refuse; the events say what happened)
        let _ = match t {
            Transport::Previous => s.TrySkipPreviousAsync(),
            Transport::PlayPause => s.TryTogglePlayPauseAsync(),
            Transport::Next => s.TrySkipNextAsync(),
        };
    }

    /// Takes the handlers off the session they are on.
    fn leave(&mut self) {
        if let Some((s, _, t1, t2)) = self.current.take() {
            let _ = s.RemovePlaybackInfoChanged(t1);
            let _ = s.RemoveMediaPropertiesChanged(t2);
        }
    }
}

/// A picture's bytes from a WinRT stream reference (at most 8 MB).
fn stream_bytes(r: &IRandomAccessStreamReference) -> Option<Vec<u8>> {
    let stream = r.OpenReadAsync().ok()?.join().ok()?;
    let size = u32::try_from(stream.Size().ok()?).ok().filter(|&n| n > 0 && n <= 8 * 1024 * 1024)?;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0).ok()?).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_name_from_an_app_user_model_id() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "ZuneMusic");
        assert_eq!(app_name("chrome.exe"), "Chrome");
        assert_eq!(app_name(r"C:\Program Files\VideoLAN\VLC\vlc.exe"), "Vlc");
        assert_eq!(app_name("  MSEdge  "), "MSEdge");
        assert_eq!(app_name(""), "");
        // a family name with no app id: no hash
        assert_eq!(app_name("Microsoft.WindowsMediaPlayer_8wekyb3d8bbwe"), "WindowsMediaPlayer");
    }

    #[test]
    fn red_and_blue_swap_and_green_and_alpha_stay() {
        let mut px = [1u8, 2, 3, 4, 10, 20, 30, 40];
        rgba_to_bgra(&mut px);
        assert_eq!(px, [3, 2, 1, 4, 30, 20, 10, 40]);
        // a half pixel at the end is left alone
        let mut odd = [1u8, 2, 3, 4, 5];
        rgba_to_bgra(&mut odd);
        assert_eq!(odd, [3, 2, 1, 4, 5]);
    }

    #[test]
    fn a_picture_is_cut_to_a_square_of_the_asked_size_and_bytes_that_are_no_picture_give_none() {
        // a 4 x 2 red PNG
        let mut png = Vec::new();
        {
            let img = image::RgbaImage::from_pixel(4, 2, image::Rgba([255, 0, 0, 255]));
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).unwrap();
        }
        let a = decode_artwork(&png, 8, 7).expect("decodes");
        assert_eq!((a.size, a.id, a.bgra.len()), (8, 7, 8 * 8 * 4));
        // red in BGRA is B=0 G=0 R=255
        assert_eq!(&a.bgra[0..4], &[0, 0, 255, 255]);
        assert!(decode_artwork(b"not a picture", 8, 1).is_none());
        assert!(decode_artwork(&[], 8, 1).is_none());
    }

    fn key(app: &str, title: &str) -> ArtKey {
        (app.into(), title.into(), "A".into())
    }

    fn pic(n: u8) -> Option<(Vec<u8>, u32)> {
        Some((vec![n; 4], 1))
    }

    #[test]
    fn a_new_track_whose_picture_comes_late_gets_that_picture_not_the_old_one_for_good() {
        let mut a = ArtState::new();
        // first track, with its cover
        assert!(a.needs_decode(&key("Spotify", "Bir")));
        assert_eq!(a.offer(key("Spotify", "Bir"), pic(1)), ArtNext::Done);
        let first = a.picture().unwrap().id;
        assert!(!a.needs_decode(&key("Spotify", "Bir")), "nothing to do again for the same track");
        // the next track: the title is there and the thumbnail is not yet; the old cover stays for one look
        assert!(a.needs_decode(&key("Spotify", "Iki")));
        assert_eq!(a.offer(key("Spotify", "Iki"), None), ArtNext::RetryLater);
        assert_eq!(a.picture().unwrap().id, first);
        // it is still not settled: the next read decodes again (this is the bug: it used to count as served)
        assert!(a.needs_decode(&key("Spotify", "Iki")));
        // Windows says the properties changed, now with the thumbnail
        assert_eq!(a.offer(key("Spotify", "Iki"), pic(2)), ArtNext::Done);
        let second = a.picture().unwrap();
        assert_ne!(second.id, first);
        assert_eq!(second.bgra, vec![2; 4]);
        assert!(!a.needs_decode(&key("Spotify", "Iki")));
    }

    #[test]
    fn a_track_that_never_gets_a_picture_loses_the_old_cover_instead_of_wearing_it() {
        let mut a = ArtState::new();
        a.offer(key("Spotify", "Bir"), pic(1));
        assert_eq!(a.offer(key("Spotify", "Iki"), None), ArtNext::RetryLater);
        // the second look finds nothing either: no cover at all
        assert_eq!(a.offer(key("Spotify", "Iki"), None), ArtNext::Done);
        assert!(a.picture().is_none());
        // and a track after it with a picture is fine
        assert_eq!(a.offer(key("Spotify", "Uc"), pic(3)), ArtNext::Done);
        assert!(a.picture().is_some());
    }

    #[test]
    fn another_app_starts_without_the_cover_of_the_first_and_no_session_forgets_everything() {
        let mut a = ArtState::new();
        a.offer(key("Spotify", "Bir"), pic(1));
        assert_eq!(a.offer(key("Chrome", "Video"), None), ArtNext::Done, "no retry for a stranger cover");
        assert!(a.picture().is_none());
        a.offer(key("Chrome", "Video"), pic(5));
        a.clear();
        assert!(a.picture().is_none());
        assert!(a.needs_decode(&key("Chrome", "Video")));
    }

    #[test]
    fn two_sessions_are_the_same_when_what_is_shown_is_the_same() {
        let art = |id| Some(Arc::new(Artwork { size: 1, bgra: vec![0; 4], id }));
        let base = MediaSession { app: "A".into(), title: "T".into(), artist: "R".into(), playing: true, can_previous: true, can_next: true, can_toggle: true, artwork: art(1) };
        assert_eq!(base, MediaSession { artwork: art(1), ..base.clone() }, "the same picture number is the same picture");
        assert_ne!(base, MediaSession { artwork: art(2), ..base.clone() });
        assert_ne!(base, MediaSession { artwork: None, ..base.clone() });
        assert_ne!(base, MediaSession { playing: false, ..base.clone() });
    }
}
