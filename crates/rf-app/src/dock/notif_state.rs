//! What the dock keeps of the notification list while the bell's panel is CLOSED: a count, nothing else (for the bell's dot).
//!
//! It is read once when the dock starts, again when the panel closes (the panel tells what it last saw) and, when the notification
//! folder of Windows changes, once per burst of changes (`rf_win::toasts::hint`: a plain directory-change wait, no polling). So the
//! dot is as fresh as the last signal; if the folder cannot be watched it is as fresh as the last time the panel was open. The
//! texts of the notifications are never kept here: they are read for the count and dropped.
//!
//! While the panel is open the folder's changes are not read at all ([`panel_open`]): the panel polls the list itself.
//!
//! Everything starts only with the setting "rustyfiles bildirim panelini kullan" (`dock_own_notifications`, on by default); switched
//! off, the worker and the folder watch end and the bell shows no dot (it opens Windows' own centre as before).

use futures::StreamExt;
use gpui_kit::*;
use rf_win::toasts::hint::{Dot, DotCount};
use rf_win::toasts::{self, Access};

use super::right::RightIsland;

pub struct NotifState {
    pub access: Access,
    pub count: usize,
    dot: Option<Dot>,
    /// The bell's panel is open (it reads the list itself, so the worker's hint reads are paused).
    panel_open: bool,
    right: Entity<RightIsland>,
    /// The channel the worker's thread tells through (kept so that a worker started later uses the same task).
    tx: futures::channel::mpsc::UnboundedSender<DotCount>,
}

impl Global for NotifState {}

/// Sets the state up (nothing runs yet): called once, when the right island exists.
pub fn init(right: Entity<RightIsland>, cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded::<DotCount>();
    cx.set_global(NotifState { access: Access::NotAsked, count: 0, dot: None, panel_open: false, right, tx });
    cx.spawn(async move |cx| {
        while let Some(c) = rx.next().await {
            cx.update(|cx| apply(c, cx));
        }
    })
    .detach();
    sync(cx);
}

fn apply(c: DotCount, cx: &mut App) {
    if !cx.has_global::<NotifState>() {
        return;
    }
    let right = {
        let s = cx.global_mut::<NotifState>();
        // (a count that arrives after the setting went off is not shown)
        if s.dot.is_none() || (s.access == c.access && s.count == c.count) {
            return;
        }
        s.access = c.access;
        s.count = c.count;
        s.right.clone()
    };
    right.update(cx, |_, cx| cx.notify());
}

/// Starts or stops the worker to match the setting.
pub fn sync(cx: &mut App) {
    if !cx.has_global::<NotifState>() {
        return;
    }
    let on = cx.try_global::<crate::settings::Settings>().is_none_or(|s| s.dock_own_notifications);
    let running = cx.global::<NotifState>().dot.is_some();
    if on == running {
        return;
    }
    if on {
        let tx = cx.global::<NotifState>().tx.clone();
        let dot = toasts::hint::start(move |c| {
            let _ = tx.unbounded_send(c);
        });
        log::debug!("notifications: the dot is followed (the folder is watched: {})", dot.hinted());
        let s = cx.global_mut::<NotifState>();
        // (switched on while the panel is open: the worker starts paused)
        dot.pause(s.panel_open);
        s.dot = Some(dot);
    } else {
        let right = {
            let s = cx.global_mut::<NotifState>();
            s.dot = None;
            s.count = 0;
            s.right.clone()
        };
        right.update(cx, |_, cx| cx.notify());
    }
}

/// The bell's panel opened or went away. While it is open the worker reads nothing for the folder's changes (the panel polls the list
/// every 2 s); when it goes away the worker makes one read for what changed meanwhile.
pub fn panel_open(open: bool, cx: &mut App) {
    if !cx.has_global::<NotifState>() {
        return;
    }
    let s = cx.global_mut::<NotifState>();
    s.panel_open = open;
    if let Some(dot) = s.dot.as_ref() {
        dot.pause(open);
    }
}

/// The bell wears its dot: access is there and the list is not empty.
pub fn dot_visible(cx: &App) -> bool {
    cx.try_global::<NotifState>().is_some_and(|s| s.dot.is_some() && toasts::dot_visible(s.access, s.count))
}

/// The open panel found this: the dot follows at once, and the worker takes it as the count.
pub fn told_by_panel(access: Access, count: usize, cx: &mut App) {
    if !cx.has_global::<NotifState>() {
        return;
    }
    let c = DotCount { access, count };
    if let Some(dot) = cx.global::<NotifState>().dot.as_ref() {
        dot.set(c);
    }
    apply(c, cx);
}

/// The last count (for the panel's first guess at its height before the list has been read).
pub fn last_count(cx: &App) -> usize {
    cx.try_global::<NotifState>().map_or(0, |s| s.count)
}
