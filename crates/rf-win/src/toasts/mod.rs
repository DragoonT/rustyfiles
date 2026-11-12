//! The notifications other programs have posted (the toasts in Windows' notification list), read for the dock's bell panel.
//!
//! WHAT WORKS WITHOUT A PACKAGE (checked on this machine, build 26300, from a program with no package identity): the WinRT
//! `UserNotificationListener` answers `GetAccessStatus()` and `GetNotificationsAsync(Toast)` (8 to 10 ms a call), each item gives its
//! program (`AppInfo`: display name and AppUserModelID), its time and the text of its `ToastGeneric` binding. What does NOT work is
//! the push event `NotificationChanged`: it throws `0x80070490` without package identity. So the list is POLLED, and only while the
//! panel is open (`listener::watch`); while it is closed the only thing running is [`hint`], one plain directory-change wait on
//! Windows' notification folder that triggers a single count (the bell's dot).
//!
//! NO CONSENT PROMPT. The access is read with `GetAccessStatus()`, which only reports the user's choice and never asks. The call that
//! asks (`RequestAccessAsync`, which shows a consent prompt and needs a window) is never made by this code, and the list is only read
//! when the status says `Allowed`; otherwise the panel says so in one line and offers a button to Windows' own privacy page
//! (`ms-settings:privacy-notifications`). A test below reads this module's source and fails if that call ever appears in code.
//!
//! PRIVACY. The texts of notifications are the user's. They are never logged and never written to a file ([`Toast`]'s `Debug` prints
//! only an id and lengths); they live in memory while the panel is open and are dropped when it closes. What stays after closing is
//! a count (the dot).
//!
//! REMOVING. `RemoveNotification(id)` and `ClearNotifications()` are documented calls of the same class; whether they take effect for
//! a program without package identity is NOT known (the probe of the research did not call them: they change the user's list). So in
//! the real mode they are OFF until the person running the program has tried them and says so with `RUSTYFILES_TOASTS_REMOVE=1`
//! ([`removal_enabled`]); with it the panel offers the small x and "clear all" only while [`RemoveWatch`] has not caught them doing
//! nothing, and a call that returns an error hides them at once. In the made-up mode (below) they work on the made-up list.
//!
//! THE PURE PARTS, tested: the access decision, grouping and ordering, the three-line clamp, the relative time, the difference between
//! two polls (so the list does not flicker or lose the keyboard cursor), the remove watch, what a poll that failed does (nothing)
//! and the timing of the hint reads ([`HintTimer`]). The Windows
//! calls are [`listener`] and [`hint`]; the clock conversions at the end of this file.

pub mod hint;
pub mod listener;

use std::fmt;
use std::time::{Duration, Instant};

/// How often the open panel reads the list again. Windows has no event for a program without package identity.
pub const POLL_EVERY: Duration = Duration::from_secs(2);

/// The page of Windows' settings where the user allows notification access.
pub const PRIVACY_PAGE: &str = "ms-settings:privacy-notifications";

/// The longest title and body kept (characters). A toast's text can be anything a program sends; the panel shows three lines.
pub const MAX_TITLE: usize = 160;
pub const MAX_BODY: usize = 600;

/// One notification as the panel draws it.
#[derive(Clone, PartialEq, Eq)]
pub struct Toast {
    /// Windows' number of this notification (stable while it is in the list): what is removed by, and what the panel tells two
    /// polls' items apart by.
    pub id: u32,
    /// The program's AppUserModelID (what starts it), empty when Windows would not say.
    pub app_id: String,
    /// The program's name for people.
    pub app: String,
    pub title: String,
    pub body: String,
    /// When it arrived: seconds since 1970-01-01, UTC. 0 when Windows did not say (the panel then shows no age: see [`age_of`]).
    pub created: i64,
}

impl fmt::Debug for Toast {
    /// Never the texts: a stray `{:?}` in a log must not carry the user's notifications.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Toast #{} (title {} chars, body {} chars)", self.id, self.title.chars().count(), self.body.chars().count())
    }
}

// ------------------------------------------------------------------ access

/// What Windows reports about the user's choice for notification access.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RawAccess {
    Allowed,
    Denied,
    /// The user has not been asked.
    Unspecified,
    /// The call itself failed (no such service on this machine).
    Failed,
}

/// What the panel does about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// The list may be read.
    Granted,
    /// The user said no.
    Denied,
    /// The user has not been asked and this code never asks (it would show a consent prompt): the panel points to Windows' page.
    NotAsked,
    /// The listener could not be had.
    Unavailable,
}

/// The decision. Only `Allowed` reads the list; nothing here ever leads to a call that asks.
pub fn access_of(raw: RawAccess) -> Access {
    match raw {
        RawAccess::Allowed => Access::Granted,
        RawAccess::Denied => Access::Denied,
        RawAccess::Unspecified => Access::NotAsked,
        RawAccess::Failed => Access::Unavailable,
    }
}

impl Access {
    pub fn may_read(self) -> bool {
        self == Access::Granted
    }
}

/// Whether the bell wears its dot: access is there and the list is not empty.
pub fn dot_visible(access: Access, count: usize) -> bool {
    access.may_read() && count > 0
}

// ------------------------------------------------------------------ a made-up list, for trying the panel

/// `RUSTYFILES_TOASTS_FAKE` makes the reader answer with a MADE-UP state instead of asking Windows, so the panel can be tried (and its
/// removing, which changes a list) without touching the user's real notifications: `many` (a list of invented notifications that the
/// x and "clear all" really remove, in memory), `empty`, `denied`, `notasked`. Nothing is read from Windows and nothing is changed there
/// in this mode. Off unless the variable is set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fake {
    Many,
    Empty,
    Denied,
    NotAsked,
}

pub fn parse_fake(v: &str) -> Option<Fake> {
    match v.trim().to_ascii_lowercase().as_str() {
        "many" => Some(Fake::Many),
        "empty" => Some(Fake::Empty),
        "denied" => Some(Fake::Denied),
        "notasked" => Some(Fake::NotAsked),
        _ => None,
    }
}

pub fn fake_from_env() -> Option<Fake> {
    std::env::var("RUSTYFILES_TOASTS_FAKE").ok().and_then(|v| parse_fake(&v))
}

/// The variable that turns the x and "clear all" on in the real mode (see the header: nobody has seen Windows obey them yet).
pub const REMOVE_ENV: &str = "RUSTYFILES_TOASTS_REMOVE";

/// Whether removing is offered at all: always on the made-up list (it is in memory), and on the real one only when asked for.
pub fn removal_enabled(fake: Option<Fake>, env: Option<&str>) -> bool {
    fake == Some(Fake::Many) || env.is_some_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"))
}

/// The invented list: three programs without an AppUserModelID (so a click starts nothing), a long title, a body of five lines, and
/// ages of seconds, minutes, hours, a day and several days.
pub fn fake_toasts(now: i64) -> Vec<Toast> {
    let t = |id: u32, app: &str, title: &str, body: &str, age: i64| Toast { id, app_id: String::new(), app: app.into(), title: title.into(), body: body.into(), created: now - age };
    vec![
        t(1001, "Deneme Posta", "Yeni ileti", "Toplantı yarın saat ikide.\nGündem ekte.", 20),
        t(1002, "Deneme Posta", "Başka bir ileti", "", 5 * 60 + 10),
        t(1003, "Deneme Sohbet", "Bir başlık, ekrana sığmayacak kadar uzun bir başlık olsun diye uzatılmış", "Birinci satır\nİkinci satır\nÜçüncü satır\nDördüncü satır\nBeşinci satır", 2 * 3_600 + 30),
        t(1004, "Deneme Sohbet", "Kısa", "Tek satırlık bir metin.", 26 * 3_600),
        t(1005, "Deneme Takvim", "Etkinlik", "Bu bir deneme bildirimidir; gerçek bir bildirim değildir.", 3 * 86_400),
    ]
}

/// What a poll found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub access: Access,
    pub toasts: Vec<Toast>,
    /// A remove call returned an error: the x and "clear all" are not offered any more.
    pub remove_refused: bool,
}

impl Snapshot {
    pub fn empty(access: Access) -> Snapshot {
        Snapshot { access, toasts: Vec::new(), remove_refused: false }
    }

    /// Whether the panel draws the x on its notifications and "clear all": there is a list to remove from and removing is not off.
    /// One answer for the header and for every row, worked out once per drawing from the snapshot being drawn.
    pub fn removal_offered(&self) -> bool {
        self.access.may_read() && !self.toasts.is_empty() && !self.remove_refused
    }
}

// ------------------------------------------------------------------ grouping and order

/// The notifications of one program, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Group {
    pub app_id: String,
    pub app: String,
    /// Indices into the list that was grouped.
    pub items: Vec<usize>,
}

fn group_key(t: &Toast) -> String {
    if t.app_id.trim().is_empty() { t.app.trim().to_lowercase() } else { t.app_id.trim().to_lowercase() }
}

fn newest_first(a: &Toast, b: &Toast) -> std::cmp::Ordering {
    b.created.cmp(&a.created).then(b.id.cmp(&a.id))
}

/// Groups by program: inside a group the newest first, and the group of the newest notification first. Equal times are told apart by
/// the id (the later one is newer), so the order never depends on the order Windows listed them in.
pub fn group(toasts: &[Toast]) -> Vec<Group> {
    let mut order: Vec<usize> = (0..toasts.len()).collect();
    order.sort_by(|&a, &b| newest_first(&toasts[a], &toasts[b]));
    let mut groups: Vec<(String, Group)> = Vec::new();
    for i in order {
        let key = group_key(&toasts[i]);
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, g)) => g.items.push(i),
            None => groups.push((key, Group { app_id: toasts[i].app_id.clone(), app: toasts[i].app.clone(), items: vec![i] })),
        }
    }
    // (the groups were opened in the order of their newest item, which is the order of `order`)
    groups.into_iter().map(|(_, g)| g).collect()
}

/// The ids in the order they are drawn (group after group): what the keyboard walks along.
pub fn draw_order(toasts: &[Toast], groups: &[Group]) -> Vec<u32> {
    groups.iter().flat_map(|g| g.items.iter().map(|&i| toasts[i].id)).collect()
}

// ------------------------------------------------------------------ the three-line clamp

/// A text cut to what the panel shows: at most `max_lines` non-empty lines (the lines of the toast; each is wrapped by the panel,
/// which clamps the wrapped text to the same number of lines again), and at most `max_chars` characters in all. Whatever was cut
/// ends in an ellipsis. Blank lines and the spaces around lines are dropped.
pub fn clamp_text(text: &str, max_lines: usize, max_chars: usize) -> String {
    let mut out = String::new();
    let mut cut = false;
    for (lines, line) in text.split(['\n', '\r']).map(str::trim).filter(|l| !l.is_empty()).enumerate() {
        if lines == max_lines {
            cut = true;
            break;
        }
        if lines > 0 {
            out.push('\n');
        }
        let room = max_chars.saturating_sub(out.chars().count());
        let n = line.chars().count();
        if n > room {
            out.extend(line.chars().take(room.saturating_sub(1)));
            cut = true;
            break;
        }
        out.push_str(line);
    }
    if cut {
        // (no ellipsis after an ellipsis, and none on an empty text)
        let trimmed = out.trim_end().trim_end_matches('…').len();
        out.truncate(trimmed);
        if !out.is_empty() {
            out.push('…');
        }
    }
    out
}

/// The body as the panel shows it: three lines.
pub fn body_lines(body: &str) -> String {
    clamp_text(body, 3, 240)
}

// ------------------------------------------------------------------ time

/// How long ago, as the panel says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ago {
    /// Less than a minute (or a clock that is behind the notification's).
    Now,
    Minutes(u32),
    Hours(u32),
    /// A day older than 24 hours whose date is the day before today.
    Yesterday,
    /// Older: the panel writes the date.
    Date,
}

/// `age_secs` is now minus the notification's time; `today` and `then` are the local calendar days (any consistent day count).
pub fn ago(age_secs: i64, today: i64, then: i64) -> Ago {
    if age_secs < 60 {
        Ago::Now
    } else if age_secs < 3_600 {
        Ago::Minutes((age_secs / 60) as u32)
    } else if age_secs < 86_400 {
        Ago::Hours((age_secs / 3_600) as u32)
    } else if today - then == 1 {
        Ago::Yesterday
    } else {
        Ago::Date
    }
}

/// The age of a notification, or `None` when Windows gave it no time (`created` is 0): it is then shown without an age, not as 1970.
/// `then` is only called for a known time (it converts to a local calendar day).
pub fn age_of(created: i64, now: i64, today: i64, then: impl FnOnce() -> i64) -> Option<Ago> {
    (created > 0).then(|| ago(now - created, today, then()))
}

// ------------------------------------------------------------------ a poll that failed

/// One turn of the reader, after Windows was asked: `read` is the list, or `None` when the list could not be had this time (a failed
/// call is NOT an empty list: it must not empty the panel, clear the dot, or teach [`RemoveWatch`] that a removal worked). Returns the
/// snapshot to tell the panel, or `None` when there is nothing new to tell.
pub fn take_turn<'a>(read: Option<Snapshot>, removal: &mut RemoveWatch, refused: bool, removal_on: bool, last: &'a mut Option<Snapshot>) -> Option<&'a Snapshot> {
    let mut snap = read?;
    // (the reader sees every reading, also the ones that are told to nobody because nothing changed)
    removal.polled(&snap.toasts.iter().map(|t| t.id).collect::<Vec<_>>());
    snap.remove_refused = refused || !removal_on || !removal.offer();
    if last.as_ref() == Some(&snap) {
        return None;
    }
    *last = Some(snap);
    last.as_ref()
}

// ------------------------------------------------------------------ between two polls

/// What differs between two polls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Diff {
    pub added: Vec<u32>,
    pub removed: Vec<u32>,
    /// In both, with other contents.
    pub changed: Vec<u32>,
}

impl Diff {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

/// Compares two lists by id. An empty difference means the panel has nothing to redraw (it does not flicker on a poll that found the
/// same list), whatever order Windows listed the items in.
pub fn diff(old: &[Toast], new: &[Toast]) -> Diff {
    let mut d = Diff::default();
    for n in new {
        match old.iter().find(|o| o.id == n.id) {
            None => d.added.push(n.id),
            Some(o) if o != n => d.changed.push(n.id),
            Some(_) => {}
        }
    }
    d.removed = old.iter().filter(|o| !new.iter().any(|n| n.id == o.id)).map(|o| o.id).collect();
    d
}

/// Where the keyboard cursor is after the list changed. It stays on its notification; when that one is gone it goes to the one that
/// followed it (else the one before it) in the old order that is still there; with nothing left it is gone. No cursor stays none.
pub fn keep_cursor(old_order: &[u32], cursor: Option<u32>, new_order: &[u32]) -> Option<u32> {
    let c = cursor?;
    if new_order.contains(&c) {
        return Some(c);
    }
    let at = old_order.iter().position(|&i| i == c)?;
    old_order[at + 1..].iter().chain(old_order[..at].iter().rev()).copied().find(|i| new_order.contains(i)).or_else(|| new_order.first().copied())
}

// ------------------------------------------------------------------ does removing work

/// Notices a remove call that does nothing. A program without package identity may be allowed to ask and still not be obeyed; the
/// panel must not keep offering an x that does nothing. After a removal was asked, the notification should be gone from the next
/// polls; one that is still listed after [`RemoveWatch::LOOKS`] polls means the call is ignored. One that went settles it the other way.
#[derive(Clone, Debug, Default)]
pub struct RemoveWatch {
    asked: Vec<(u32, u8)>,
    works: bool,
    ignored: bool,
}

impl RemoveWatch {
    /// Polls a notification may still be listed after it was asked to go.
    pub const LOOKS: u8 = 2;

    /// The user asked for these to be removed (one id, or every id listed for "clear all").
    pub fn asked(&mut self, ids: impl IntoIterator<Item = u32>) {
        self.asked.extend(ids.into_iter().map(|i| (i, 0)));
    }

    /// A poll found these ids listed.
    pub fn polled(&mut self, listed: &[u32]) {
        let mut ignored = false;
        let mut works = false;
        self.asked.retain_mut(|(id, looks)| {
            if !listed.contains(id) {
                works = true;
                return false;
            }
            *looks += 1;
            if *looks >= Self::LOOKS {
                ignored = true;
                return false;
            }
            true
        });
        self.works |= works;
        // (once a removal has been seen to work, a late one that did not go is a program posting it again, not a refusal)
        if ignored && !self.works {
            self.ignored = true;
        }
    }

    /// Whether the x and "clear all" are still worth offering.
    pub fn offer(&self) -> bool {
        !self.ignored
    }
}

// ------------------------------------------------------------------ the hint

/// The shortest time between two reads for the dot, how long after the last change in the folder the read waits (one toast touches
/// the folder in several writes), and how long a burst of changes may keep the read waiting.
pub const HINT_MIN_GAP: Duration = Duration::from_secs(3);
pub const HINT_SETTLE: Duration = Duration::from_millis(800);
pub const HINT_BURST_CAP: Duration = Duration::from_secs(3);

/// When the read of a burst of changes is made: [`HINT_SETTLE`] after the last change, but not later than [`HINT_BURST_CAP`] after the
/// first one, and never before [`HINT_MIN_GAP`] has passed since the last read. The last rule wins over the other two, so a burst
/// that comes soon after a read can never make two reads closer than the gap.
pub fn read_at(last_read: Option<Instant>, first_hint: Instant, last_hint: Instant) -> Instant {
    let earliest = last_read.map_or(first_hint, |t| t + HINT_MIN_GAP);
    (last_hint + HINT_SETTLE).min(first_hint + HINT_BURST_CAP).max(earliest)
}

/// What the dot's worker has been told and when it should read. Pure: the worker feeds it the time.
#[derive(Clone, Copy, Debug, Default)]
pub struct HintTimer {
    /// The first and the last change of the burst waiting to be read.
    burst: Option<(Instant, Instant)>,
    /// The bell's panel is open: it reads the list itself, so the folder's changes are not read a second time.
    paused: bool,
}

impl HintTimer {
    /// The folder changed.
    pub fn hint(&mut self, now: Instant) {
        if self.paused {
            return;
        }
        match &mut self.burst {
            Some((_, last)) => *last = now,
            None => self.burst = Some((now, now)),
        }
    }

    /// The panel opened or closed. Opening drops what was waiting (the panel's own reading covers it); closing asks for one read,
    /// because changes made while paused were not noted.
    pub fn set_paused(&mut self, paused: bool, now: Instant) {
        if paused == self.paused {
            return;
        }
        self.paused = paused;
        if paused {
            self.burst = None;
        } else {
            self.hint(now);
        }
    }

    /// When to read, or `None` when nothing is waiting.
    pub fn due(&self, last_read: Option<Instant>) -> Option<Instant> {
        self.burst.map(|(first, last)| read_at(last_read, first, last))
    }

    /// The read was made.
    pub fn read_done(&mut self) {
        self.burst = None;
    }
}

// ------------------------------------------------------------------ the clock (Windows)

/// 1601-01-01 to 1970-01-01, seconds.
const EPOCH_GAP_SECS: i64 = 11_644_473_600;

/// Seconds since 1970 from a Windows tick count (100 ns since 1601), as `DateTime.UniversalTime` and `FILETIME` have it.
pub fn unix_from_ticks(ticks: i64) -> i64 {
    ticks.div_euclid(10_000_000) - EPOCH_GAP_SECS
}

/// Now, seconds since 1970 (UTC).
pub fn now_unix() -> i64 {
    use windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime;
    let ft = unsafe { GetSystemTimeAsFileTime() };
    unix_from_ticks((i64::from(ft.dwHighDateTime) << 32) | i64::from(ft.dwLowDateTime))
}

/// The local date and time of a moment (the user's time zone at that moment, daylight saving included).
pub fn local_clock(unix: i64) -> Option<crate::dock::clock::ClockTime> {
    use windows::Win32::Foundation::{FILETIME, SYSTEMTIME};
    use windows::Win32::System::Time::{FileTimeToSystemTime, SystemTimeToTzSpecificLocalTime};
    let ticks = (unix + EPOCH_GAP_SECS).checked_mul(10_000_000).filter(|t| *t >= 0)?;
    let ft = FILETIME { dwLowDateTime: (ticks & 0xFFFF_FFFF) as u32, dwHighDateTime: (ticks >> 32) as u32 };
    let (mut utc, mut local) = (SYSTEMTIME::default(), SYSTEMTIME::default());
    unsafe {
        FileTimeToSystemTime(&ft, &mut utc).ok()?;
        SystemTimeToTzSpecificLocalTime(None, &utc, &mut local).ok()?;
    }
    Some(crate::dock::clock::ClockTime {
        year: local.wYear,
        month: local.wMonth as u8,
        day: local.wDay as u8,
        hour: local.wHour as u8,
        minute: local.wMinute as u8,
        second: local.wSecond as u8,
        millisecond: local.wMilliseconds,
    })
}

/// The local calendar day of a moment, as a day count (for [`ago`]). 0 when the moment cannot be converted.
pub fn local_day(unix: i64) -> i64 {
    local_clock(unix).map_or(0, |t| crate::dock::calendar::days_from_civil(i32::from(t.year), t.month, t.day))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toast(id: u32, app_id: &str, app: &str, created: i64) -> Toast {
        Toast { id, app_id: app_id.into(), app: app.into(), title: format!("t{id}"), body: String::new(), created }
    }

    #[test]
    fn only_a_granted_access_reads_the_list_and_nothing_asks() {
        assert_eq!(access_of(RawAccess::Allowed), Access::Granted);
        assert_eq!(access_of(RawAccess::Denied), Access::Denied);
        assert_eq!(access_of(RawAccess::Unspecified), Access::NotAsked);
        assert_eq!(access_of(RawAccess::Failed), Access::Unavailable);
        assert!(Access::Granted.may_read());
        for a in [Access::Denied, Access::NotAsked, Access::Unavailable] {
            assert!(!a.may_read(), "{a:?}");
            assert!(!dot_visible(a, 5), "no dot without access: {a:?}");
        }
        assert!(dot_visible(Access::Granted, 1));
        assert!(!dot_visible(Access::Granted, 0));
    }

    /// The call that makes Windows show a consent prompt must not be in this module's code (it may be named in a comment, which is
    /// why the check looks at the call with its opening bracket).
    #[test]
    fn the_call_that_asks_for_consent_is_never_made() {
        for (name, src) in [("mod.rs", include_str!("mod.rs")), ("listener.rs", include_str!("listener.rs")), ("hint.rs", include_str!("hint.rs"))] {
            let code: String = src.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
            // (built from two halves so that this test does not contain the name itself)
            let ask = ["Request", "AccessAsync("].concat();
            assert!(!code.contains(&ask), "{name} calls the consent API");
        }
    }

    #[test]
    fn the_made_up_list_is_a_real_test_of_the_panel_and_is_off_unless_asked_for() {
        assert_eq!(parse_fake("many"), Some(Fake::Many));
        assert_eq!(parse_fake(" Denied "), Some(Fake::Denied));
        assert_eq!(parse_fake("notasked"), Some(Fake::NotAsked));
        assert_eq!(parse_fake("empty"), Some(Fake::Empty));
        assert_eq!(parse_fake(""), None);
        assert_eq!(parse_fake("1"), None);
        let now = 1_800_000_000;
        let list = fake_toasts(now);
        let groups = group(&list);
        assert_eq!(groups.len(), 3, "three programs");
        assert!(list.iter().all(|t| t.app_id.is_empty()), "a click starts nothing");
        let mut ids: Vec<u32> = list.iter().map(|t| t.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), list.len());
        // every age bucket the panel words is in it
        let kinds: Vec<Ago> = list.iter().map(|t| ago(now - t.created, local_day(now), local_day(t.created))).collect();
        assert!(kinds.contains(&Ago::Now) && kinds.iter().any(|k| matches!(k, Ago::Minutes(_))) && kinds.iter().any(|k| matches!(k, Ago::Hours(_))));
        assert!(kinds.contains(&Ago::Yesterday) || kinds.contains(&Ago::Date));
        assert!(list.iter().any(|t| t.body.lines().count() > 3), "a body that gets cut");
        assert!(list.iter().any(|t| t.title.chars().count() > 60), "a title that gets cut");
        assert!(list.iter().any(|t| t.body.is_empty()));
    }

    #[test]
    fn a_toast_never_shows_its_text_in_debug_output() {
        let t = Toast { id: 7, app_id: "a".into(), app: "App".into(), title: "SECRET-TITLE".into(), body: "SECRET-BODY".into(), created: 0 };
        let shown = format!("{t:?} {:?}", Snapshot { access: Access::Granted, toasts: vec![t.clone()], remove_refused: false });
        assert!(!shown.contains("SECRET"), "{shown}");
        assert!(shown.contains("#7"));
    }

    #[test]
    fn notifications_are_grouped_by_program_newest_first() {
        let list = vec![
            toast(1, "chat", "Chat", 100),
            toast(2, "mail", "Mail", 300),
            toast(3, "chat", "Chat", 200),
            toast(4, "news", "News", 250),
            toast(5, "mail", "Mail", 50),
        ];
        let g = group(&list);
        // mail has the newest (300), news next (250), chat last (200)
        assert_eq!(g.iter().map(|g| g.app.as_str()).collect::<Vec<_>>(), ["Mail", "News", "Chat"]);
        assert_eq!(g[0].items, vec![1, 4], "mail: id 2 (300) then id 5 (50)");
        assert_eq!(g[2].items, vec![2, 0], "chat: id 3 (200) then id 1 (100)");
        assert_eq!(draw_order(&list, &g), vec![2, 5, 4, 3, 1]);
        // every item exactly once
        let mut all: Vec<usize> = g.iter().flat_map(|g| g.items.clone()).collect();
        all.sort_unstable();
        assert_eq!(all, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn the_order_does_not_depend_on_how_windows_listed_them() {
        let a = vec![toast(1, "x", "X", 100), toast(2, "x", "X", 100), toast(3, "y", "Y", 100)];
        let mut b = a.clone();
        b.reverse();
        let order = |l: &[Toast]| draw_order(l, &group(l));
        assert_eq!(order(&a), order(&b), "equal times: the later id is the newer");
        assert_eq!(order(&a), vec![3, 2, 1]);
        // a group is its program's id, not its name; without an id, the name (any case) stands in
        let l = vec![toast(1, "", "Photos", 10), toast(2, "", "photos", 20), toast(3, "p!app", "Photos", 30)];
        assert_eq!(group(&l).len(), 2);
        assert_eq!(group(&[]).len(), 0);
    }

    #[test]
    fn the_body_is_cut_to_three_lines_and_a_number_of_characters() {
        assert_eq!(clamp_text("a\nb\nc", 3, 100), "a\nb\nc");
        assert_eq!(clamp_text("a\nb\nc\nd", 3, 100), "a\nb\nc…", "a fourth line is cut and says so");
        assert_eq!(clamp_text("  a  \n\n  \r\nb\r\n", 3, 100), "a\nb", "blank lines and edge spaces go");
        assert_eq!(clamp_text("", 3, 100), "");
        assert_eq!(clamp_text("\n \n", 3, 100), "");
        assert_eq!(clamp_text("abcdefghij", 3, 5), "abcd…", "the length limit counts the ellipsis");
        assert_eq!(clamp_text("abcde", 3, 5), "abcde", "exactly the limit is not cut");
        // characters, not bytes: a Turkish text is not cut inside a letter
        assert_eq!(clamp_text("çğıöşü çğıöşü", 3, 7), "çğıöşü…");
        // no ellipsis after an ellipsis
        assert_eq!(clamp_text("a…\nb\nc\nd", 3, 100), "a…\nb\nc…");
        assert_eq!(clamp_text("a\nb", 1, 100), "a…");
        assert_eq!(body_lines("1\n2\n3\n4"), "1\n2\n3…");
        let long = "x".repeat(1000);
        assert_eq!(body_lines(&long).chars().count(), 240);
    }

    #[test]
    fn how_long_ago() {
        // today is day 100
        assert_eq!(ago(0, 100, 100), Ago::Now);
        assert_eq!(ago(59, 100, 100), Ago::Now);
        assert_eq!(ago(-30, 100, 100), Ago::Now, "a clock behind the notification's");
        assert_eq!(ago(60, 100, 100), Ago::Minutes(1));
        assert_eq!(ago(5 * 60 + 59, 100, 100), Ago::Minutes(5));
        assert_eq!(ago(3_599, 100, 100), Ago::Minutes(59));
        assert_eq!(ago(3_600, 100, 100), Ago::Hours(1));
        assert_eq!(ago(2 * 3_600 + 5, 100, 100), Ago::Hours(2));
        assert_eq!(ago(86_399, 100, 99), Ago::Hours(23), "three hours before midnight is still hours");
        assert_eq!(ago(86_400, 100, 99), Ago::Yesterday);
        assert_eq!(ago(30 * 3_600, 100, 99), Ago::Yesterday);
        assert_eq!(ago(49 * 3_600, 100, 98), Ago::Date);
        assert_eq!(ago(10 * 86_400, 100, 90), Ago::Date);
    }

    #[test]
    fn a_poll_that_found_the_same_list_changes_nothing() {
        let a = vec![toast(1, "x", "X", 1), toast(2, "y", "Y", 2)];
        let mut b = a.clone();
        b.reverse();
        assert!(diff(&a, &a).is_empty());
        assert!(diff(&a, &b).is_empty(), "only the order differs: nothing to redraw");
        assert!(diff(&[], &[]).is_empty());
    }

    #[test]
    fn what_was_added_removed_or_changed_is_told_apart() {
        let old = vec![toast(1, "x", "X", 1), toast(2, "y", "Y", 2), toast(3, "z", "Z", 3)];
        let mut new = vec![toast(2, "y", "Y", 2), toast(3, "z", "Z", 3), toast(4, "w", "W", 4)];
        new[1].title = "other".into();
        let d = diff(&old, &new);
        assert_eq!(d, Diff { added: vec![4], removed: vec![1], changed: vec![3] });
        assert!(!d.is_empty());
    }

    #[test]
    fn the_keyboard_cursor_stays_on_its_notification() {
        let old = [10, 11, 12, 13];
        // the list gained one in front and lost another: the cursor is where it was
        assert_eq!(keep_cursor(&old, Some(12), &[20, 10, 12, 13]), Some(12));
        // its own notification went: the one that followed it
        assert_eq!(keep_cursor(&old, Some(11), &[10, 12, 13]), Some(12));
        // the last went: the one before
        assert_eq!(keep_cursor(&old, Some(13), &[10, 11, 12]), Some(12));
        // the followers are gone too: the nearest earlier one
        assert_eq!(keep_cursor(&old, Some(11), &[10]), Some(10));
        // all of them replaced: the first of the new list
        assert_eq!(keep_cursor(&old, Some(11), &[30, 31]), Some(30));
        // nothing left
        assert_eq!(keep_cursor(&old, Some(11), &[]), None);
        // no cursor stays none (the panel does not take the keyboard on a poll)
        assert_eq!(keep_cursor(&old, None, &[10, 11]), None);
        // a cursor that was never in the list (stale) is dropped
        assert_eq!(keep_cursor(&old, Some(99), &[10]), None);
    }

    #[test]
    fn a_removal_that_is_ignored_takes_the_x_away_and_one_that_works_keeps_it() {
        let mut w = RemoveWatch::default();
        assert!(w.offer(), "before anything is tried");
        w.asked([5]);
        w.polled(&[5, 6]);
        assert!(w.offer(), "one look is not enough: Windows may be a moment behind");
        w.polled(&[5, 6]);
        assert!(!w.offer(), "still listed after two looks: the call is ignored");
        // a removal that goes
        let mut w = RemoveWatch::default();
        w.asked([5]);
        w.polled(&[6]);
        w.polled(&[6]);
        assert!(w.offer());
        // once it has been seen to work, a notification that comes back is a program posting it again
        w.asked([6]);
        w.polled(&[6]);
        w.polled(&[6]);
        assert!(w.offer());
        // clear all asks for every listed id; one that went is proof enough
        let mut w = RemoveWatch::default();
        w.asked([1, 2, 3]);
        w.polled(&[2, 3]);
        w.polled(&[2, 3]);
        assert!(w.offer());
    }

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn the_hint_waits_for_the_folder_to_settle_and_never_reads_more_than_once_in_three_seconds() {
        let b = Instant::now();
        // no read before: settle after the change
        assert_eq!(read_at(None, at(b, 0), at(b, 0)), at(b, 800));
        // a read long ago: the same
        assert_eq!(read_at(Some(b), at(b, 20_000), at(b, 20_000)), at(b, 20_800));
        // a read 1 s ago: the rest of the 3 s
        assert_eq!(read_at(Some(b), at(b, 1_000), at(b, 1_000)), at(b, 3_000));
        // 2.5 s ago: 500 ms are left, less than the settle time
        assert_eq!(read_at(Some(b), at(b, 2_500), at(b, 2_500)), at(b, 3_300));
        // a burst that keeps going is read 3 s after it began, not for ever
        assert_eq!(read_at(None, at(b, 0), at(b, 2_900)), at(b, 3_000));
    }

    /// The case the first version got wrong: read at 0, a hint at 0.5 s, another at 1.0 s. The read was made at 1.8 s.
    #[test]
    fn a_second_hint_inside_the_wait_never_brings_the_read_closer_than_the_gap() {
        let b = Instant::now();
        let mut t = HintTimer::default();
        t.hint(at(b, 500));
        assert_eq!(t.due(Some(b)), Some(at(b, 3_000)));
        t.hint(at(b, 1_000));
        assert_eq!(t.due(Some(b)), Some(at(b, 3_000)), "still not before 3 s since the read");
        for ms in (1_000..2_900).step_by(100) {
            t.hint(at(b, ms));
            assert!(t.due(Some(b)).unwrap() >= at(b, 3_000), "hint at {ms} ms");
        }
        t.read_done();
        assert_eq!(t.due(Some(b)), None, "nothing waits after the read");
    }

    #[test]
    fn the_hint_timer_is_silent_while_the_panel_is_open_and_reads_once_when_it_closes() {
        let b = Instant::now();
        let mut t = HintTimer::default();
        t.hint(at(b, 100));
        t.set_paused(true, at(b, 200));
        assert_eq!(t.due(None), None, "what waited is dropped: the panel reads the list itself");
        t.hint(at(b, 300));
        t.hint(at(b, 400));
        assert_eq!(t.due(None), None, "changes while open read nothing");
        t.set_paused(true, at(b, 500));
        assert_eq!(t.due(None), None, "opening twice changes nothing");
        t.set_paused(false, at(b, 10_000));
        assert_eq!(t.due(Some(b)), Some(at(b, 10_800)), "closed: one read after the settle time");
        t.set_paused(false, at(b, 11_000));
        assert_eq!(t.due(Some(b)), Some(at(b, 10_800)), "closing twice does not ask again");
    }

    #[test]
    fn removing_is_off_in_the_real_mode_until_asked_for() {
        assert!(!removal_enabled(None, None));
        assert!(!removal_enabled(None, Some("")));
        assert!(!removal_enabled(None, Some("0")));
        assert!(removal_enabled(None, Some("1")));
        assert!(removal_enabled(None, Some(" True ")));
        assert!(removal_enabled(Some(Fake::Many), None), "the made-up list is in memory");
        for f in [Fake::Empty, Fake::Denied, Fake::NotAsked] {
            assert!(!removal_enabled(Some(f), None));
        }
    }

    #[test]
    fn the_x_is_offered_only_for_a_readable_list_that_is_not_refused() {
        let list = vec![toast(1, "x", "X", 1)];
        let ok = Snapshot { access: Access::Granted, toasts: list.clone(), remove_refused: false };
        assert!(ok.removal_offered());
        assert!(!Snapshot { remove_refused: true, ..ok.clone() }.removal_offered());
        assert!(!Snapshot { toasts: vec![], ..ok.clone() }.removal_offered());
        assert!(!Snapshot { access: Access::Denied, ..ok }.removal_offered());
    }

    #[test]
    fn a_notification_without_a_time_has_no_age() {
        assert_eq!(age_of(0, 1_000, 100, || panic!("not asked for an unknown time")), None);
        assert_eq!(age_of(-5, 1_000, 100, || panic!("not asked")), None);
        assert_eq!(age_of(940, 1_000, 100, || 100), Some(Ago::Minutes(1)));
    }

    #[test]
    fn a_poll_that_failed_is_not_an_empty_list() {
        let list = Snapshot { access: Access::Granted, toasts: vec![toast(5, "x", "X", 10), toast(6, "y", "Y", 20)], remove_refused: false };
        let (mut w, mut last) = (RemoveWatch::default(), None);
        assert_eq!(take_turn(Some(list.clone()), &mut w, false, true, &mut last).map(|s| s.toasts.len()), Some(2));
        assert_eq!(take_turn(Some(list.clone()), &mut w, false, true, &mut last), None, "the same list tells nobody");
        // the user asks for 5 to go; then reads fail: nothing is told, nothing is learnt
        w.asked([5]);
        for _ in 0..3 {
            assert_eq!(take_turn(None, &mut w, false, true, &mut last), None, "a failed read tells nobody (the panel keeps its list)");
        }
        assert_eq!(last.as_ref().map(|s| s.toasts.len()), Some(2), "the last good reading is kept");
        // so a removal that Windows then ignores is still caught: the failed reads must not have counted as "it is gone"
        assert!(take_turn(Some(list.clone()), &mut w, false, true, &mut last).is_none());
        let told = take_turn(Some(list.clone()), &mut w, false, true, &mut last).map(|s| s.remove_refused);
        assert_eq!(told, Some(true), "listed after two real looks: the x is withdrawn");
        // removing off: always refused
        let (mut w, mut last) = (RemoveWatch::default(), None);
        assert_eq!(take_turn(Some(list), &mut w, false, false, &mut last).map(|s| s.remove_refused), Some(true));
    }

    #[test]
    fn windows_ticks_become_unix_seconds() {
        // 1970-01-01 in ticks
        assert_eq!(unix_from_ticks(116_444_736_000_000_000), 0);
        assert_eq!(unix_from_ticks(116_444_736_000_000_000 + 10_000_000 * 86_400), 86_400);
        assert_eq!(unix_from_ticks(116_444_736_000_000_000 - 10_000_000), -1);
    }

    /// Read-only: the clock functions agree with each other.
    #[test]
    fn the_local_day_of_now_is_the_calendar_day_of_the_local_clock() {
        let now = now_unix();
        assert!(now > 1_700_000_000, "{now}");
        let t = local_clock(now).expect("local time");
        let cal = crate::dock::calendar::today();
        // (the two clocks are read a moment apart; across midnight they may differ by one day)
        let d = local_day(now) - cal.days();
        assert!(d.abs() <= 1, "{t:?} {cal:?}");
        // (24 hours before is the day before, or two days before on the day a clock change makes a day 23 hours long)
        let back = local_day(now) - local_day(now - 86_400);
        assert!((1..=2).contains(&back), "{back}");
    }
}
