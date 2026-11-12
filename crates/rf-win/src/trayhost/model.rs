//! The model of the notification area: the icons programs have put there, as the tray host learned them from `WM_COPYDATA`. Pure: a
//! parsed request and the things the handler read beforehand (pixels, owner) go in, the new state and the answer a tray would give
//! come out. No handle is ever kept except the raw 32 bits of the owner's window; pictures are pixels, copied while the sender was
//! still waiting.
//!
//! Bounds (they are the host's, whatever a program sends): at most [`MAX_ICONS`] icons (a 257th `NIM_ADD` is refused in the model),
//! pictures at most [`MAX_ICON_PX`] square, tips at most [`MAX_TIP_CHARS`] characters, balloon texts at most their wire sizes.
//!
//! GENERATIONS. Every change of the model bumps one counter ([`TrayModel::generation`]) and stamps the icon it changed
//! ([`TrayIcon::generation`], and [`TrayIcon::image_generation`] for the picture alone), so an interface that remembers the last
//! number it drew can tell what changed and skip the rest. A request that changes nothing bumps nothing.
//!
//! The answers ([`ApplyOutcome::answer`]) are what a tray returns to `Shell_NotifyIcon` when nobody else answers (research note 2,
//! section 5.3): `NIM_ADD` of a new icon true, of a known one false (and the fields are merged all the same, so a program that adds
//! again after `TaskbarCreated` refreshes the icon), `NIM_MODIFY` / `NIM_DELETE` true for a known icon, `NIM_SETVERSION` true for
//! 0, 3 and 4 on a known icon, `NIM_SETFOCUS` true.

use std::sync::Arc;

use crate::tray_wire::notify::{BalloonKind, Nim, NotifyIconRequest, nis};
use crate::tray_wire::{IconKey, IconVersion, WireGuid};

pub const MAX_ICONS: usize = 256;
pub const MAX_ICON_PX: u32 = 64;
pub const MAX_TIP_CHARS: usize = 256;

/// An icon's number in this model. Never reused within a run, so a click that was queued for an icon which has gone since cannot
/// land on another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IconId(pub u64);

/// An icon's picture: `width` x `height` pixels, BGRA with STRAIGHT alpha (the convention of `rf_win::dock::icons::Bgra`), at most
/// [`MAX_ICON_PX`] on each side.
#[derive(Clone, PartialEq, Eq)]
pub struct IconBitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl IconBitmap {
    /// `None` when the size is zero, beyond [`MAX_ICON_PX`], or does not match the data.
    pub fn new(width: u32, height: u32, bgra: Vec<u8>) -> Option<IconBitmap> {
        let ok = (1..=MAX_ICON_PX).contains(&width) && (1..=MAX_ICON_PX).contains(&height) && bgra.len() == (width * height * 4) as usize;
        ok.then_some(IconBitmap { width, height, bgra })
    }
}

impl std::fmt::Debug for IconBitmap {
    // (the pixels are the user's; a log line gets the size)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "IconBitmap({}x{})", self.width, self.height)
    }
}

/// What changed on an icon, as a bit set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changes(pub u8);

impl Changes {
    pub const IMAGE: u8 = 1;
    pub const TIP: u8 = 2;
    pub const STATE: u8 = 4;
    pub const VERSION: u8 = 8;
    pub const CALLBACK: u8 = 16;
    pub const OWNER: u8 = 32;
    pub fn has(self, bit: u8) -> bool {
        self.0 & bit != 0
    }
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
    fn set(&mut self, bit: u8) {
        self.0 |= bit;
    }
}

/// A balloon (`NIF_INFO`) as the sender asked for it. The model keeps the latest one of each icon; the queue, the timing and the
/// drawing are the notification increment's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Balloon {
    /// Numbers the balloons of the whole model, so a consumer can tell a new one from one it has seen.
    pub seq: u64,
    pub title: String,
    pub text: String,
    pub kind: BalloonKind,
    pub info_flags: u32,
    pub no_sound: bool,
    pub large_icon: bool,
    pub respect_quiet_time: bool,
    pub realtime: bool,
    /// The `NIIF_USER` picture (`hBalloonIcon`), when there was one that could be read; otherwise the icon's own picture is meant.
    pub user_icon: Option<Arc<IconBitmap>>,
    pub at_ms: u64,
}

/// One icon of the notification area.
#[derive(Clone, Debug)]
pub struct TrayIcon {
    pub id: IconId,
    pub key: IconKey,
    /// The owner window, the raw 32 bits as sent (widen with `sign_extend_handle`).
    pub hwnd: u32,
    pub uid: u32,
    /// `uCallbackMessage`, once `NIF_MESSAGE` was seen. Without it nothing can be sent to the owner.
    pub callback: Option<u32>,
    pub version: IconVersion,
    pub tip: String,
    /// `dwState` merged under the masks seen so far.
    pub state: u32,
    pub image: Option<Arc<IconBitmap>>,
    /// The owner window's process when the icon was added (0: unknown). A window whose owner is another process now is not this
    /// icon's owner any more.
    pub owner_pid: u32,
    /// The owner's program file name (`chrome.exe`), empty if unknown.
    pub owner_exe: String,
    /// Order of arrival, for a stable default order.
    pub added_seq: u64,
    pub generation: u64,
    pub image_generation: u64,
    pub balloon: Option<Balloon>,
}

impl TrayIcon {
    pub fn hidden(&self) -> bool {
        self.state & nis::HIDDEN != 0
    }

    pub fn shared_icon(&self) -> bool {
        self.state & nis::SHAREDICON != 0
    }

    pub fn hwnd_handle(&self) -> isize {
        crate::tray_wire::sign_extend_handle(self.hwnd)
    }
}

/// What the handler read before taking the model, from the sender's process.
#[derive(Clone, Debug, Default)]
pub enum ImageUpdate {
    /// The request does not touch the picture, or it could not be read: the old picture stays.
    #[default]
    Keep,
    /// `NIF_ICON` with a null handle.
    Clear,
    Set(Arc<IconBitmap>),
}

#[derive(Clone, Debug, Default)]
pub struct Prepared {
    pub image: ImageUpdate,
    /// The `NIIF_USER` picture of a balloon.
    pub balloon_icon: Option<Arc<IconBitmap>>,
    /// `(pid, program file name)` of the owner window. Read for `NIM_ADD` only.
    pub owner: Option<(u32, String)>,
    pub now_ms: u64,
    /// Explorer's answer to the same request, when it gave one: `Some(false)` is a refusal. It matters for one thing here: a
    /// `NIM_ADD` of an icon the model already holds starts a new life for the icon (version back to 0) only if Explorer did not
    /// refuse it. Explorer refuses the add of an icon it already has and keeps that icon's version, and the model must do the same
    /// or a version 4 program would get the packing of version 0.
    pub explorer_accepted: Option<bool>,
}

/// Something the interface may want to hear about. A HINT: the model (with its generation) is the truth, and a full queue drops events.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayEvent {
    Added(IconId),
    Changed(IconId, Changes),
    Removed(IconId),
    /// A balloon arrived for this icon (not a removal).
    Balloon(IconId),
}

/// What happened to the model, for the counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyNote {
    Added,
    /// `NIM_ADD` of an icon that was there: merged.
    ReAdded,
    Modified,
    ModifyUnknown,
    Deleted,
    DeleteUnknown,
    VersionSet,
    VersionRefused,
    Focus,
    /// A 257th icon.
    Full,
}

#[derive(Clone, Debug)]
pub struct ApplyOutcome {
    /// What a tray answers the sender when nobody else does.
    pub answer: bool,
    pub events: Vec<TrayEvent>,
    pub note: ApplyNote,
    /// Set when the request removed an icon.
    pub removed: Option<IconId>,
}

/// A snapshot for the interface: cheap to clone (pictures are shared).
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub generation: u64,
    pub icons: Vec<TrayIcon>,
}

#[derive(Default)]
pub struct TrayModel {
    icons: Vec<TrayIcon>,
    next_id: u64,
    generation: u64,
    seq: u64,
    balloon_seq: u64,
}

fn clamp_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => s[..at].to_string(),
        None => s.to_string(),
    }
}

impl TrayModel {
    pub fn new() -> TrayModel {
        TrayModel::default()
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn len(&self) -> usize {
        self.icons.len()
    }

    pub fn is_empty(&self) -> bool {
        self.icons.is_empty()
    }

    pub fn icons(&self) -> &[TrayIcon] {
        &self.icons
    }

    pub fn get(&self, id: IconId) -> Option<&TrayIcon> {
        self.icons.iter().find(|i| i.id == id)
    }

    pub fn find(&self, key: &IconKey) -> Option<&TrayIcon> {
        self.icons.iter().find(|i| i.key == *key)
    }

    /// An icon by window and id, whichever way it is keyed (a program may ask about a GUID icon by its window).
    pub fn find_wnd(&self, hwnd: u32, uid: u32) -> Option<&TrayIcon> {
        self.icons.iter().find(|i| i.hwnd == hwnd && i.uid == uid)
    }

    /// An icon by the key a rectangle query names (GUID, or window and id).
    pub fn find_query(&self, key: &IconKey) -> Option<&TrayIcon> {
        match key {
            IconKey::Guid(g) => self.icons.iter().find(|i| matches!(i.key, IconKey::Guid(k) if k == *g)),
            IconKey::Wnd { hwnd, uid } => self.find_wnd(*hwnd, *uid),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { generation: self.generation, icons: self.icons.clone() }
    }

    fn bump(&mut self) -> u64 {
        self.generation += 1;
        self.generation
    }

    /// Removes an icon (it was deleted, or its owner is gone). False when there was none.
    pub fn remove(&mut self, id: IconId) -> bool {
        let Some(at) = self.icons.iter().position(|i| i.id == id) else { return false };
        self.icons.remove(at);
        self.bump();
        true
    }

    /// Applies one parsed `dwData = 1` request. `prep` is what the handler read from the sender's process beforehand.
    pub fn apply(&mut self, req: &NotifyIconRequest, prep: Prepared) -> ApplyOutcome {
        let d = &req.data;
        let key = d.key();
        let pos = self.icons.iter().position(|i| i.key == key);
        let outcome = |answer, note, events: Vec<TrayEvent>, removed| ApplyOutcome { answer, events, note, removed };
        match req.message {
            Nim::Add => {
                let explorer_accepted = prep.explorer_accepted;
                let (at, fresh) = match pos {
                    Some(at) => (at, false),
                    None => {
                        if self.icons.len() >= MAX_ICONS {
                            return outcome(false, ApplyNote::Full, vec![], None);
                        }
                        self.seq += 1;
                        self.next_id += 1;
                        self.icons.push(TrayIcon {
                            id: IconId(self.next_id),
                            key,
                            hwnd: d.hwnd,
                            uid: d.uid,
                            callback: None,
                            version: IconVersion::V0,
                            tip: String::new(),
                            state: 0,
                            image: None,
                            owner_pid: 0,
                            owner_exe: String::new(),
                            added_seq: self.seq,
                            generation: 0,
                            image_generation: 0,
                            balloon: None,
                        });
                        (self.icons.len() - 1, true)
                    }
                };
                let (mut changes, new_balloon) = self.merge(at, req, prep.clone());
                if !fresh {
                    // a program that adds again starts a new life for the icon: Windows forgets the version at every add and the
                    // program sends `NIM_SETVERSION` again. Not when Explorer refused the add: it kept the icon and its version.
                    let icon = &mut self.icons[at];
                    if icon.version != IconVersion::V0 && explorer_accepted != Some(false) {
                        icon.version = IconVersion::V0;
                        changes.set(Changes::VERSION);
                    }
                    if icon.hwnd != d.hwnd || icon.uid != d.uid {
                        icon.hwnd = d.hwnd;
                        icon.uid = d.uid;
                        changes.set(Changes::OWNER);
                    }
                }
                if let Some((pid, exe)) = prep.owner {
                    let icon = &mut self.icons[at];
                    if icon.owner_pid != pid || icon.owner_exe != exe {
                        icon.owner_pid = pid;
                        icon.owner_exe = exe;
                        changes.set(Changes::OWNER);
                    }
                }
                let id = self.icons[at].id;
                let mut events = Vec::new();
                if fresh {
                    events.push(TrayEvent::Added(id));
                } else if !changes.is_empty() {
                    events.push(TrayEvent::Changed(id, changes));
                }
                if fresh || !changes.is_empty() {
                    let g = self.bump();
                    self.icons[at].generation = g;
                }
                self.push_balloon_event(at, new_balloon, &mut events);
                outcome(fresh, if fresh { ApplyNote::Added } else { ApplyNote::ReAdded }, events, None)
            }
            Nim::Modify => {
                let Some(at) = pos else { return outcome(false, ApplyNote::ModifyUnknown, vec![], None) };
                let (changes, new_balloon) = self.merge(at, req, prep);
                let id = self.icons[at].id;
                let mut events = Vec::new();
                if !changes.is_empty() {
                    let g = self.bump();
                    self.icons[at].generation = g;
                    events.push(TrayEvent::Changed(id, changes));
                }
                self.push_balloon_event(at, new_balloon, &mut events);
                outcome(true, ApplyNote::Modified, events, None)
            }
            Nim::Delete => match pos {
                Some(at) => {
                    let id = self.icons[at].id;
                    self.icons.remove(at);
                    self.bump();
                    outcome(true, ApplyNote::Deleted, vec![TrayEvent::Removed(id)], Some(id))
                }
                None => outcome(false, ApplyNote::DeleteUnknown, vec![], None),
            },
            Nim::SetVersion => {
                let (Some(at), Some(v)) = (pos, d.requested_version()) else { return outcome(false, ApplyNote::VersionRefused, vec![], None) };
                let mut events = Vec::new();
                if self.icons[at].version != v {
                    self.icons[at].version = v;
                    let g = self.bump();
                    self.icons[at].generation = g;
                    events.push(TrayEvent::Changed(self.icons[at].id, Changes(Changes::VERSION)));
                }
                outcome(true, ApplyNote::VersionSet, events, None)
            }
            // the sender asks for the keyboard focus to return to the tray; the model has nothing to change
            Nim::SetFocus => outcome(true, ApplyNote::Focus, vec![], None),
        }
    }

    /// Merges the fields a request carries into the icon at `at` (only the fields whose `NIF_*` flag is set). Returns what changed;
    /// does not bump the generation (the caller does, once).
    fn merge(&mut self, at: usize, req: &NotifyIconRequest, prep: Prepared) -> (Changes, bool) {
        let d = &req.data;
        let mut ch = Changes::default();
        let mut new_balloon = false;
        if let Some(cb) = d.callback()
            && self.icons[at].callback != Some(cb)
        {
            self.icons[at].callback = Some(cb);
            ch.set(Changes::CALLBACK);
        }
        if let Some(tip) = d.tip_text() {
            let tip = clamp_chars(tip, MAX_TIP_CHARS);
            if self.icons[at].tip != tip {
                self.icons[at].tip = tip;
                ch.set(Changes::TIP);
            }
        }
        if let Some(st) = d.state_update() {
            let merged = st.merge(self.icons[at].state);
            if self.icons[at].state != merged {
                self.icons[at].state = merged;
                ch.set(Changes::STATE);
            }
        }
        match prep.image {
            ImageUpdate::Keep => {}
            ImageUpdate::Clear => {
                if self.icons[at].image.is_some() {
                    self.icons[at].image = None;
                    ch.set(Changes::IMAGE);
                }
            }
            ImageUpdate::Set(img) => {
                // (the same picture again is not a change: animations that resend one frame cost the interface nothing)
                if self.icons[at].image.as_deref() != Some(&*img) {
                    self.icons[at].image = Some(img);
                    ch.set(Changes::IMAGE);
                }
            }
        }
        if ch.has(Changes::IMAGE) {
            // (the stamp is taken now; the caller bumps the generation right after, so the two stay ordered)
            self.icons[at].image_generation = self.generation + 1;
        }
        if let Some(b) = d.balloon() {
            if b.is_removal() {
                self.icons[at].balloon = None;
            } else {
                self.balloon_seq += 1;
                new_balloon = true;
                self.icons[at].balloon = Some(Balloon {
                    seq: self.balloon_seq,
                    title: b.title,
                    text: b.text,
                    kind: b.kind,
                    info_flags: b.info_flags,
                    no_sound: b.no_sound,
                    large_icon: b.large_icon,
                    respect_quiet_time: b.respect_quiet_time,
                    realtime: b.realtime,
                    user_icon: prep.balloon_icon,
                    at_ms: prep.now_ms,
                });
            }
        }
        (ch, new_balloon)
    }

    /// A balloon the request just stored is an event of its own (and a generation of its own).
    fn push_balloon_event(&mut self, at: usize, stored: bool, events: &mut Vec<TrayEvent>) {
        if stored {
            let g = self.bump();
            self.icons[at].generation = g;
            events.push(TrayEvent::Balloon(self.icons[at].id));
        }
    }

    /// `(id, window, owner pid)` of every icon, for the sweep that looks for owners that are gone.
    pub fn owners(&self) -> Vec<(IconId, isize, u32)> {
        self.icons.iter().map(|i| (i.id, i.hwnd_handle(), i.owner_pid)).collect()
    }
}

/// How an icon is told apart in the user's own order file: by GUID when it has one, else by program and id. Never by window handle
/// (that changes at every start of the program). Used by `order`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum StableKey {
    Guid(WireGuid),
    ExeUid { exe: String, uid: u32 },
}

impl TrayIcon {
    pub fn stable_key(&self) -> StableKey {
        match self.key {
            IconKey::Guid(g) => StableKey::Guid(g),
            IconKey::Wnd { uid, .. } => StableKey::ExeUid { exe: self.owner_exe.to_ascii_lowercase(), uid },
        }
    }
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::tray_wire::notify::{NotifyIconData, DataTier, nif};

    pub fn data(hwnd: u32, uid: u32, flags: u32) -> NotifyIconData {
        NotifyIconData {
            cb_size: 956,
            hwnd,
            uid,
            flags,
            callback_message: 0x8005,
            icon: 0,
            tip: String::new(),
            state: 0,
            state_mask: 0,
            info: String::new(),
            timeout_or_version: 0,
            info_title: String::new(),
            info_flags: 0,
            guid: WireGuid::default(),
            balloon_icon: 0,
            tier: DataTier::Full,
        }
    }

    pub fn req(m: Nim, d: NotifyIconData) -> NotifyIconRequest {
        NotifyIconRequest { message: m, data: d, trailing_len: 0, trailing_word: None }
    }

    pub fn add(hwnd: u32, uid: u32) -> NotifyIconRequest {
        req(Nim::Add, data(hwnd, uid, nif::MESSAGE | nif::TIP))
    }

    pub fn bitmap(w: u32, h: u32, fill: u8) -> Arc<IconBitmap> {
        Arc::new(IconBitmap::new(w, h, vec![fill; (w * h * 4) as usize]).unwrap())
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use crate::tray_wire::notify::nif;

    fn none() -> Prepared {
        Prepared::default()
    }

    #[test]
    fn add_modify_delete_answers_follow_the_table() {
        let mut m = TrayModel::new();
        let o = m.apply(&add(1, 1), none());
        assert!(o.answer);
        assert_eq!(o.note, ApplyNote::Added);
        assert_eq!(m.len(), 1);
        // adding again: false, and still one icon
        let o = m.apply(&add(1, 1), none());
        assert!(!o.answer);
        assert_eq!(o.note, ApplyNote::ReAdded);
        assert_eq!(m.len(), 1);
        // modify known / unknown
        assert!(m.apply(&req(Nim::Modify, data(1, 1, nif::TIP)), none()).answer);
        let o = m.apply(&req(Nim::Modify, data(9, 9, nif::TIP)), none());
        assert!(!o.answer);
        assert_eq!(o.note, ApplyNote::ModifyUnknown);
        assert_eq!(m.len(), 1, "an unknown modify is not an implicit add");
        // delete known / unknown
        let o = m.apply(&req(Nim::Delete, data(9, 9, 0)), none());
        assert!(!o.answer);
        assert_eq!(o.note, ApplyNote::DeleteUnknown);
        let o = m.apply(&req(Nim::Delete, data(1, 1, 0)), none());
        assert!(o.answer);
        assert!(o.removed.is_some());
        assert!(m.is_empty());
        // focus is always true
        assert!(m.apply(&req(Nim::SetFocus, data(7, 7, 0)), none()).answer);
    }

    #[test]
    fn set_version_takes_only_0_3_4_on_a_known_icon() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), none());
        for (v, ok) in [(4u32, true), (3, true), (0, true), (1, false), (2, false), (5, false), (99, false)] {
            let mut d = data(1, 1, 0);
            d.timeout_or_version = v;
            let o = m.apply(&req(Nim::SetVersion, d), none());
            assert_eq!(o.answer, ok, "version {v}");
        }
        let mut d = data(1, 1, 0);
        d.timeout_or_version = 4;
        m.apply(&req(Nim::SetVersion, d.clone()), none());
        assert_eq!(m.icons()[0].version, IconVersion::V4);
        // unknown icon
        d.hwnd = 8;
        assert!(!m.apply(&req(Nim::SetVersion, d), none()).answer);
        // a block too short to hold the word is not a request for version 0
        let mut short = data(1, 1, 0);
        short.tier = crate::tray_wire::notify::DataTier::V1;
        short.timeout_or_version = 0;
        assert!(!m.apply(&req(Nim::SetVersion, short), none()).answer);
        assert_eq!(m.icons()[0].version, IconVersion::V4);
    }

    #[test]
    fn a_new_add_starts_the_version_over() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), none());
        let mut d = data(1, 1, 0);
        d.timeout_or_version = 4;
        m.apply(&req(Nim::SetVersion, d), none());
        assert_eq!(m.icons()[0].version, IconVersion::V4);
        let o = m.apply(&add(1, 1), none());
        assert_eq!(m.icons()[0].version, IconVersion::V0, "Windows forgets the version at every add");
        assert!(matches!(o.events.as_slice(), [TrayEvent::Changed(_, c)] if c.has(Changes::VERSION)));
    }

    #[test]
    fn only_the_fields_whose_flag_is_set_are_merged() {
        let mut m = TrayModel::new();
        let mut d = data(1, 1, nif::MESSAGE | nif::TIP);
        d.tip = "first".into();
        m.apply(&req(Nim::Add, d), none());
        // a modify with NIF_ICON only: tip and callback stay, even though the stale fields say otherwise
        let mut d = data(1, 1, nif::ICON);
        d.tip = "stale".into();
        d.callback_message = 0x9999;
        m.apply(&req(Nim::Modify, d), Prepared { image: ImageUpdate::Set(bitmap(16, 16, 7)), ..none() });
        let i = &m.icons()[0];
        assert_eq!(i.tip, "first");
        assert_eq!(i.callback, Some(0x8005));
        assert!(i.image.is_some());
    }

    #[test]
    fn state_merges_under_the_mask() {
        let mut m = TrayModel::new();
        let mut d = data(1, 1, nif::STATE);
        d.state = 3;
        d.state_mask = 3;
        m.apply(&req(Nim::Add, d), none());
        assert!(m.icons()[0].hidden() && m.icons()[0].shared_icon());
        // unhide, leave the shared bit alone
        let mut d = data(1, 1, nif::STATE);
        d.state = 0;
        d.state_mask = 1;
        m.apply(&req(Nim::Modify, d), none());
        assert!(!m.icons()[0].hidden() && m.icons()[0].shared_icon());
    }

    #[test]
    fn icon_clear_keeps_and_replaces() {
        let mut m = TrayModel::new();
        m.apply(&req(Nim::Add, data(1, 1, nif::MESSAGE | nif::ICON)), Prepared { image: ImageUpdate::Set(bitmap(16, 16, 1)), ..none() });
        assert!(m.icons()[0].image.is_some());
        m.apply(&req(Nim::Modify, data(1, 1, nif::ICON)), Prepared { image: ImageUpdate::Keep, ..none() });
        assert!(m.icons()[0].image.is_some(), "a picture that could not be read leaves the old one");
        m.apply(&req(Nim::Modify, data(1, 1, nif::ICON)), Prepared { image: ImageUpdate::Clear, ..none() });
        assert!(m.icons()[0].image.is_none());
    }

    #[test]
    fn a_guid_names_the_icon_whatever_the_window() {
        let mut m = TrayModel::new();
        let g = WireGuid::from_parts(1, 2, 3, [4; 8]);
        let mut d = data(1, 1, nif::GUID | nif::MESSAGE);
        d.guid = g;
        m.apply(&req(Nim::Add, d.clone()), none());
        // the program restarted: another window, same GUID: one icon, now owned by the new window
        d.hwnd = 77;
        d.uid = 5;
        let o = m.apply(&req(Nim::Add, d), none());
        assert_eq!(o.note, ApplyNote::ReAdded);
        assert_eq!(m.len(), 1);
        assert_eq!(m.icons()[0].hwnd, 77);
        assert_eq!(m.icons()[0].uid, 5);
        // a flagless block with the same GUID bytes is NOT the GUID icon (NIF_GUID not set)
        let mut d = data(1, 1, nif::MESSAGE);
        d.guid = g;
        m.apply(&req(Nim::Add, d), none());
        assert_eq!(m.len(), 2);
        // a rectangle query by window and id finds the GUID icon too
        assert!(m.find_query(&IconKey::Wnd { hwnd: 77, uid: 5 }).is_some());
        assert!(m.find_query(&IconKey::Guid(g)).is_some());
    }

    #[test]
    fn at_most_256_icons() {
        let mut m = TrayModel::new();
        for i in 0..MAX_ICONS as u32 {
            assert!(m.apply(&add(100 + i, 1), none()).answer);
        }
        let g = m.generation();
        let o = m.apply(&add(9999, 1), none());
        assert!(!o.answer);
        assert_eq!(o.note, ApplyNote::Full);
        assert_eq!(m.len(), MAX_ICONS);
        assert_eq!(m.generation(), g, "a refused add changes nothing");
        // room again after a delete
        m.apply(&req(Nim::Delete, data(100, 1, 0)), none());
        assert!(m.apply(&add(9999, 1), none()).answer);
    }

    #[test]
    fn tips_are_cut_at_256_characters_not_bytes() {
        let mut m = TrayModel::new();
        let mut d = data(1, 1, nif::TIP);
        d.tip = "ş".repeat(400);
        m.apply(&req(Nim::Add, d), none());
        assert_eq!(m.icons()[0].tip.chars().count(), MAX_TIP_CHARS);
        assert_eq!(clamp_chars("abc", 10), "abc");
        assert_eq!(clamp_chars("abc", 3), "abc");
        assert_eq!(clamp_chars("abc", 2), "ab");
    }

    #[test]
    fn generations_move_only_when_something_changed() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), none());
        let g1 = m.generation();
        assert_eq!(m.icons()[0].generation, g1);
        // the same modify twice: the second changes nothing
        let mut d = data(1, 1, nif::TIP);
        d.tip = "x".into();
        m.apply(&req(Nim::Modify, d.clone()), none());
        let g2 = m.generation();
        assert!(g2 > g1);
        let o = m.apply(&req(Nim::Modify, d), none());
        assert_eq!(m.generation(), g2);
        assert!(o.events.is_empty());
        // the same picture again is no change; a different one is, and stamps image_generation
        let o = m.apply(&req(Nim::Modify, data(1, 1, nif::ICON)), Prepared { image: ImageUpdate::Set(bitmap(16, 16, 5)), ..none() });
        let g3 = m.generation();
        assert!(g3 > g2 && !o.events.is_empty());
        assert_eq!(m.icons()[0].image_generation, g3, "the picture's stamp is the generation of the change");
        m.apply(&req(Nim::Modify, data(1, 1, nif::ICON)), Prepared { image: ImageUpdate::Set(bitmap(16, 16, 5)), ..none() });
        assert_eq!(m.generation(), g3);
        m.apply(&req(Nim::Modify, data(1, 1, nif::ICON)), Prepared { image: ImageUpdate::Set(bitmap(16, 16, 6)), ..none() });
        assert!(m.generation() > g3);
        // a second icon: the first one's stamp is untouched
        let stamp = m.icons()[0].generation;
        m.apply(&add(2, 1), none());
        assert_eq!(m.icons()[0].generation, stamp);
        assert_eq!(m.icons()[1].generation, m.generation());
        // a delete moves the model's generation
        let before = m.generation();
        m.apply(&req(Nim::Delete, data(2, 1, 0)), none());
        assert!(m.generation() > before);
    }

    #[test]
    fn events_say_what_happened() {
        let mut m = TrayModel::new();
        let o = m.apply(&add(1, 1), none());
        let id = m.icons()[0].id;
        assert_eq!(o.events, vec![TrayEvent::Added(id)]);
        let mut d = data(1, 1, nif::TIP);
        d.tip = "t".into();
        let o = m.apply(&req(Nim::Modify, d), none());
        assert_eq!(o.events, vec![TrayEvent::Changed(id, Changes(Changes::TIP))]);
        let o = m.apply(&req(Nim::Delete, data(1, 1, 0)), none());
        assert_eq!(o.events, vec![TrayEvent::Removed(id)]);
        assert_eq!(o.removed, Some(id));
    }

    #[test]
    fn ids_are_not_reused() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), none());
        let a = m.icons()[0].id;
        m.apply(&req(Nim::Delete, data(1, 1, 0)), none());
        m.apply(&add(1, 1), none());
        assert_ne!(m.icons()[0].id, a);
        assert!(m.get(a).is_none());
    }

    #[test]
    fn balloons_are_kept_and_removed_and_announced_once() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), none());
        let mut d = data(1, 1, nif::INFO);
        d.info = "body".into();
        d.info_title = "title".into();
        d.info_flags = 1;
        let o = m.apply(&req(Nim::Modify, d.clone()), Prepared { now_ms: 42, ..none() });
        let id = m.icons()[0].id;
        assert_eq!(o.events, vec![TrayEvent::Balloon(id)]);
        let b = m.icons()[0].balloon.clone().unwrap();
        assert_eq!((b.title.as_str(), b.text.as_str(), b.seq, b.at_ms), ("title", "body", 1, 42));
        // an empty text removes the balloon, and says nothing
        d.info.clear();
        let o = m.apply(&req(Nim::Modify, d), none());
        assert!(m.icons()[0].balloon.is_none());
        assert!(o.events.is_empty());
        // a modify without NIF_INFO leaves a balloon alone and announces nothing new
        let mut d2 = data(1, 1, nif::INFO);
        d2.info = "again".into();
        m.apply(&req(Nim::Modify, d2), none());
        let o = m.apply(&req(Nim::Modify, data(1, 1, nif::TIP)), none());
        assert!(m.icons()[0].balloon.is_some());
        assert!(!o.events.iter().any(|e| matches!(e, TrayEvent::Balloon(_))));
    }

    #[test]
    fn owner_is_taken_at_add_and_a_re_add_refreshes_it() {
        let mut m = TrayModel::new();
        m.apply(&add(1, 1), Prepared { owner: Some((10, "a.exe".into())), ..none() });
        assert_eq!((m.icons()[0].owner_pid, m.icons()[0].owner_exe.as_str()), (10, "a.exe"));
        m.apply(&req(Nim::Modify, data(1, 1, nif::TIP)), none());
        assert_eq!(m.icons()[0].owner_pid, 10, "a modify keeps the owner");
        m.apply(&add(1, 1), Prepared { owner: Some((11, "b.exe".into())), ..none() });
        assert_eq!(m.icons()[0].owner_pid, 11);
    }

    #[test]
    fn stable_keys_do_not_use_the_window() {
        let mut m = TrayModel::new();
        m.apply(&add(5, 3), Prepared { owner: Some((1, "Chrome.EXE".into())), ..none() });
        let k1 = m.icons()[0].stable_key();
        assert_eq!(k1, StableKey::ExeUid { exe: "chrome.exe".into(), uid: 3 });
        let mut m2 = TrayModel::new();
        m2.apply(&add(999, 3), Prepared { owner: Some((77, "chrome.exe".into())), ..none() });
        assert_eq!(m2.icons()[0].stable_key(), k1, "another window, another pid, the same icon for the user's order");
    }

    #[test]
    fn the_bitmap_is_bounded_and_does_not_print_its_pixels() {
        assert!(IconBitmap::new(64, 64, vec![0; 64 * 64 * 4]).is_some());
        assert!(IconBitmap::new(65, 64, vec![0; 65 * 64 * 4]).is_none());
        assert!(IconBitmap::new(0, 4, vec![]).is_none());
        assert!(IconBitmap::new(2, 2, vec![0; 15]).is_none());
        assert_eq!(format!("{:?}", bitmap(16, 16, 9)), "IconBitmap(16x16)");
    }
}
