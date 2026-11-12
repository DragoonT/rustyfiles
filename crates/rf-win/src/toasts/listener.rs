//! The WinRT side of the notification list: a thread of its own (COM MTA, as the system sources of the control centre run; not the UI
//! thread, not the shell thread) that reads Windows' `UserNotificationListener`.
//!
//! [`watch`] is the open panel's: it reads the list at once and every [`super::POLL_EVERY`] until the returned [`Watch`] is dropped,
//! and tells its callback only when what it read differs from what it told last. It also does what the user asks of it: remove one
//! notification, clear the list. [`count_now`] is the closed bell's one-off read for the dot: it keeps nothing but a number.
//!
//! The access is read with `GetAccessStatus()` and the list is read only when that says `Allowed`. The call that shows a consent
//! prompt is never made (see the notes in `super`). Every property of every item is read with its own error check: a program whose
//! package cannot be described (a known failure of this API for programs without identity) still shows, under its AppUserModelID.

use std::sync::mpsc::{Sender, channel};
use std::time::Duration;

use windows::UI::Notifications::Management::{UserNotificationListener, UserNotificationListenerAccessStatus};
use windows::UI::Notifications::{NotificationKinds, UserNotification};
use windows::core::HSTRING;

use super::{Access, Fake, MAX_BODY, MAX_TITLE, REMOVE_ENV, RawAccess, RemoveWatch, Snapshot, Toast, access_of, fake_from_env, fake_toasts, now_unix, removal_enabled, take_turn, unix_from_ticks};
use crate::sysstatus::{Subscription, init_mta};

enum Msg {
    /// Read again now (the user opened the panel again, or a remove was made).
    Read,
    Remove(u32),
    Clear,
    Quit,
}

/// The open panel's reader. Dropping it ends the thread.
pub struct Watch {
    tx: Sender<Msg>,
    _end: Subscription,
}

impl Watch {
    /// The user clicked the small x of one notification.
    pub fn remove(&self, id: u32) {
        let _ = self.tx.send(Msg::Remove(id));
    }

    /// The user clicked "clear all".
    pub fn clear(&self) {
        let _ = self.tx.send(Msg::Clear);
    }

    /// Read now, without waiting for the next turn.
    pub fn refresh(&self) {
        let _ = self.tx.send(Msg::Read);
    }
}

/// Starts reading. `callback` runs on the reader's thread: with the first reading, then whenever a reading differs from the one
/// before. The texts in it are the user's: the receiver keeps them only while the panel is open.
pub fn watch(poll: Duration, callback: impl Fn(&Snapshot) + Send + 'static) -> Watch {
    let (tx, rx) = channel::<Msg>();
    let spawned = std::thread::Builder::new().name("rf-toasts".into()).spawn(move || {
        init_mta();
        let fake = fake_from_env();
        // (in the made-up mode Windows is not asked at all, so there is no listener to remove anything from)
        let listener = if fake.is_some() { None } else { UserNotificationListener::Current().ok() };
        let mut fake_list = (fake == Some(Fake::Many)).then(|| fake_toasts(now_unix()));
        let mut refused = false;
        // (removing is off in the real mode unless asked for: nobody has seen Windows obey it for a program without identity)
        let removal_on = removal_enabled(fake, std::env::var(REMOVE_ENV).ok().as_deref());
        let mut removal = RemoveWatch::default();
        let mut last: Option<Snapshot> = None;
        loop {
            let reading = match fake {
                Some(f) => Some(fake_snapshot(f, fake_list.as_deref().unwrap_or_default())),
                None => read(listener.as_ref()),
            };
            // (a reading that failed tells nobody and teaches nothing: see `take_turn`)
            if let Some(snap) = take_turn(reading, &mut removal, refused, removal_on, &mut last) {
                callback(snap);
            }
            // Wait for the next turn, or for what the user asks. Every way out of the wait reads again at once, so the panel shows
            // what Windows did with a click without waiting for the next turn.
            match rx.recv_timeout(poll) {
                Ok(Msg::Quit) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(Msg::Read) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Ok(Msg::Remove(id)) if removal_on => {
                    if let Some(list) = fake_list.as_mut() {
                        removal.asked([id]);
                        list.retain(|t| t.id != id);
                    } else if let Some(l) = &listener {
                        // (a documented call; it is made only as the answer to the user's own click)
                        removal.asked([id]);
                        if l.RemoveNotification(id).is_err() {
                            log::warn!("notifications: RemoveNotification failed");
                            refused = true;
                        }
                    }
                }
                Ok(Msg::Clear) if removal_on => {
                    removal.asked(last.iter().flat_map(|s| s.toasts.iter().map(|t| t.id)));
                    if let Some(list) = fake_list.as_mut() {
                        list.clear();
                    } else if let Some(l) = &listener
                        && l.ClearNotifications().is_err()
                    {
                        log::warn!("notifications: ClearNotifications failed");
                        refused = true;
                    }
                }
                // (the panel does not offer them when removing is off; a stray message does nothing)
                Ok(Msg::Remove(_) | Msg::Clear) => {}
            }
        }
    });
    if let Err(e) = spawned {
        log::warn!("notifications: the thread did not start: {e}");
    }
    let quit = tx.clone();
    Watch {
        tx,
        _end: Subscription::new(move || {
            let _ = quit.send(Msg::Quit);
        }),
    }
}

/// The made-up reading of `RUSTYFILES_TOASTS_FAKE`.
fn fake_snapshot(f: Fake, list: &[Toast]) -> Snapshot {
    match f {
        Fake::Many => Snapshot { access: Access::Granted, toasts: list.to_vec(), remove_refused: false },
        Fake::Empty => Snapshot::empty(Access::Granted),
        Fake::Denied => Snapshot::empty(Access::Denied),
        Fake::NotAsked => Snapshot::empty(Access::NotAsked),
    }
}

/// What Windows says about the user's choice. Never asks.
fn raw_access(l: &UserNotificationListener) -> RawAccess {
    match l.GetAccessStatus() {
        Ok(s) if s == UserNotificationListenerAccessStatus::Allowed => RawAccess::Allowed,
        Ok(s) if s == UserNotificationListenerAccessStatus::Denied => RawAccess::Denied,
        Ok(_) => RawAccess::Unspecified,
        Err(_) => RawAccess::Failed,
    }
}

/// One reading of the list. Blocks for the few milliseconds the call takes (this thread is not the UI's). `None` when the list could
/// not be had this time although access is granted (a passing error of Windows' notification service): that is not an empty list.
fn read(listener: Option<&UserNotificationListener>) -> Option<Snapshot> {
    let Some(l) = listener else { return Some(Snapshot::empty(Access::Unavailable)) };
    let access = access_of(raw_access(l));
    if !access.may_read() {
        return Some(Snapshot::empty(access));
    }
    let list = l.GetNotificationsAsync(NotificationKinds::Toast).ok().and_then(|op| op.join().ok())?;
    let n = list.Size().unwrap_or(0);
    let toasts = (0..n).filter_map(|i| list.GetAt(i).ok()).filter_map(|item| read_toast(&item)).collect();
    Some(Snapshot { access, toasts, remove_refused: false })
}

fn read_toast(n: &UserNotification) -> Option<Toast> {
    let id = n.Id().ok()?;
    // (0 = Windows did not say; the panel shows no age for it)
    let created = n.CreationTime().map(|t| unix_from_ticks(t.UniversalTime)).unwrap_or(0).max(0);
    let (app_id, mut app) = match n.AppInfo() {
        Ok(info) => (
            info.AppUserModelId().map(|s| s.to_string()).unwrap_or_default(),
            info.DisplayInfo().and_then(|d| d.DisplayName()).map(|s| s.to_string()).unwrap_or_default(),
        ),
        Err(_) => (String::new(), String::new()),
    };
    if app.trim().is_empty() {
        app = crate::sysstatus::media::app_name(&app_id);
    }
    let texts: Vec<String> = n
        .Notification()
        .and_then(|x| x.Visual())
        .and_then(|v| v.GetBinding(&HSTRING::from("ToastGeneric")))
        .and_then(|b| b.GetTextElements())
        .map(|elements| {
            let count = elements.Size().unwrap_or(0);
            (0..count).filter_map(|i| elements.GetAt(i).ok()).filter_map(|t| t.Text().ok()).map(|s| s.to_string()).collect()
        })
        .unwrap_or_default();
    // the first text of a toast is its title, the rest is the body
    let mut it = texts.into_iter();
    let title = cut(it.next().unwrap_or_default().trim(), MAX_TITLE);
    let body = cut(&it.collect::<Vec<_>>().join("\n"), MAX_BODY);
    Some(Toast { id, app_id, app, title, body, created })
}

fn cut(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// The number of notifications in the list, for the bell's dot, read once on the calling thread (a worker; it starts COM as MTA).
/// `None` when the list cannot be read; `Some((access, n))` otherwise. The notifications themselves are dropped at once.
pub fn count_now() -> Option<(Access, usize)> {
    if let Some(f) = fake_from_env() {
        let s = fake_snapshot(f, &fake_toasts(now_unix()));
        return Some((s.access, s.toasts.len()));
    }
    init_mta();
    let l = UserNotificationListener::Current().ok()?;
    let access = access_of(raw_access(&l));
    if !access.may_read() {
        return Some((access, 0));
    }
    let list = l.GetNotificationsAsync(NotificationKinds::Toast).ok()?.join().ok()?;
    Some((access, list.Size().ok()? as usize))
}
