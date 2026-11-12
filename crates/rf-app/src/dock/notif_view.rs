//! The bell's panel: what opens above the right island when its bell is clicked (`panel.rs` opens, places and closes it; this is what
//! is inside). About 380 logical px wide, on HyperGlass, in the look of the control centre's.
//!
//! WHAT IT SHOWS. The notifications other programs have posted, as Windows lists them (`rf_win::toasts`, read through the WinRT
//! `UserNotificationListener`, which works for a program with no package identity): grouped by program, the program with the newest
//! notification first, newest first inside a program. A program's heading has its icon (the dock's icon code, on the shell thread)
//! and its name; each notification its title, up to three lines of text and its age ("şimdi", "5 dk", "2 sa", "dün", or the date). A
//! click on a notification starts its program (by its AppUserModelID, as the Start panel does). A small x removes that one from
//! Windows' list and "Tümünü temizle" clears it, while `rf_win::toasts::RemoveWatch` has not caught Windows ignoring them (see there).
//! With no notification: "Bildirim yok". Without the user's permission for notification access: one line and a button to Windows'
//! privacy page; the panel never asks for the permission itself.
//!
//! WHAT RUNS, AND WHEN. Opening the panel starts one reader thread (`rf_win::toasts::listener::watch`) that reads the list at once and
//! every 2 s (Windows' push event needs package identity: it is not available), and closing it ends the thread. The panel keeps the
//! notifications' TEXTS only while it is open: hiding the window drops them, the pictures of the lists, and the keyboard cursor; what
//! stays is the COUNT (`notif_state.rs`, for the bell's dot). Nothing here is logged or written to a file.
//!
//! NO FLICKER. A poll that finds the same list (`rf_win::toasts::diff` is empty) redraws nothing; one that finds a different list
//! keeps the keyboard cursor on its notification (`rf_win::toasts::keep_cursor`) and the scroll where it was.
//!
//! THE KEYBOARD. Up and Down walk the notifications, Enter starts the program of the one under the cursor, Delete removes it (when
//! removing is offered), Escape closes, Tab visits the buttons.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::StreamExt;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::apps::App as AppEntry;
use rf_win::dock::IconSource;
use rf_win::dock::clock;
use rf_win::dock::panel::{self as geo, CloseCause};
use rf_win::dock::startapps;
use rf_win::toasts::listener::{self, Watch};
use rf_win::toasts::{self, Access, Group, Snapshot, Toast};

use super::backdrop::IslandGlass;
use super::cc_model::Words;
use super::icons::{self, IconCache};
use super::look::Env;
use super::notif_model::{self, Content, GROUP_H};
use super::notif_state;
use super::panel_parts::{self, Look};
use crate::glass_watch::{self, PopupGlass};
use crate::t;

/// The panel's width, logical px.
pub const WIDTH: f32 = notif_model::WIDTH;
/// A group's heading shows its program's icon at this size, logical px.
const ICON: f32 = 22.0;
/// How often the ages ("5 dk") are written again while the panel is open (they change at most once a minute).
const AGE_EVERY: Duration = Duration::from_secs(20);

pub struct NotifView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// On the screen or easing out (false while hidden: nothing is built, nothing is kept).
    shown: bool,
    closing: bool,
    anim: Option<(Instant, bool)>,
    focus: FocusHandle,
    stops: RefCell<HashMap<(&'static str, usize), FocusHandle>>,

    /// The reader; it exists only while the panel is open.
    live: Option<Watch>,
    _tasks: Vec<Task<()>>,
    /// What the last reading found (the user's texts: dropped when the panel is hidden).
    snapshot: Option<Snapshot>,
    groups: Vec<Group>,
    /// The ids in the order they are drawn.
    order: Vec<u32>,
    /// The notification the keyboard is on.
    cursor: Option<u32>,
    scroll: ScrollHandle,
    icons: IconCache,
    /// The user's date pattern and AM / PM words, for the date of an old notification.
    date_pattern: String,
    am: String,
    pm: String,
    /// The height last given to the window (logical px).
    reported: f32,
}

impl NotifView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        NotifView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            focus: cx.focus_handle(),
            stops: RefCell::new(HashMap::new()),
            live: None,
            _tasks: Vec::new(),
            snapshot: None,
            groups: Vec::new(),
            order: Vec::new(),
            cursor: None,
            scroll: ScrollHandle::new(),
            icons: IconCache::default(),
            date_pattern: "dd.MM.yyyy".into(),
            am: "AM".into(),
            pm: "PM".into(),
            reported: 0.0,
        }
    }

    fn stop(&self, key: &'static str, i: usize, cx: &mut Context<Self>) -> FocusHandle {
        self.stops.borrow_mut().entry((key, i)).or_insert_with(|| cx.focus_handle().tab_stop(true)).clone()
    }

    // ------------------------------------------------------------------ what is shown

    fn content(&self, cx: &App) -> Content {
        match &self.snapshot {
            None => Content::Loading { items: notif_state::last_count(cx) },
            Some(s) if !s.access.may_read() => Content::NoAccess,
            Some(s) if s.toasts.is_empty() => Content::Empty,
            Some(s) => Content::List { groups: self.groups.len(), items: s.toasts.len() },
        }
    }

    /// The window's height now, logical px.
    pub fn height(&self, cx: &App) -> f32 {
        notif_model::height(self.content(cx))
    }

    fn refit(&mut self, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let h = self.height(cx);
        if (h - self.reported).abs() > 0.5 {
            self.reported = h;
            super::panel::relayout_sized(super::panel::NOTIFICATIONS, (WIDTH, h), cx);
        }
    }

    /// Whether the x and "clear all" are offered (for the keyboard and clicks, outside drawing: `render` has its own answer from the
    /// snapshot it draws).
    fn can_remove(&self) -> bool {
        self.snapshot.as_ref().is_some_and(Snapshot::removal_offered)
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    /// About to be shown: start the reader and begin easing in.
    pub fn on_open(&mut self, _dark: bool, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.anim = Some((Instant::now(), true));
        self.snapshot = None;
        self.groups.clear();
        self.order.clear();
        self.cursor = None;
        let f = clock::system_formats();
        (self.date_pattern, self.am, self.pm) = (f.date, f.am, f.pm);
        self.reported = self.height(cx);
        self.start_live(cx);
        notif_state::panel_open(true, cx);
        cx.notify();
    }

    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        cx.notify();
    }

    /// Hidden: the glass comes off, the reader ends and the user's texts are dropped. The count was told to the dot at each reading.
    pub fn on_hidden(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self.live = None;
        self._tasks.clear();
        self.snapshot = None;
        self.groups = Vec::new();
        self.order = Vec::new();
        self.cursor = None;
        // (the programs' pictures are small and kept, but not for ever)
        self.icons.retain(|k| k.starts_with("n:"));
        notif_state::panel_open(false, cx);
        cx.notify();
    }

    fn start_live(&mut self, cx: &mut Context<Self>) {
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<Snapshot>();
        self.live = Some(listener::watch(toasts::POLL_EVERY, move |s| {
            let _ = tx.unbounded_send(s.clone());
        }));
        let events = cx.spawn(async move |this, cx| {
            while let Some(s) = rx.next().await {
                if this.update(cx, |me, cx| me.apply(s, cx)).is_err() {
                    break;
                }
            }
        });
        // the ages are written again now and then (a reading that finds the same list tells nobody)
        let ages = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(AGE_EVERY).await;
                if this.update(cx, |me, cx| if me.shown { cx.notify() }).is_err() {
                    break;
                }
            }
        });
        self._tasks = vec![events, ages];
    }

    /// A reading arrived. A list that differs in nothing from the one shown redraws nothing.
    fn apply(&mut self, snap: Snapshot, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let same = self.snapshot.as_ref().is_some_and(|old| old.access == snap.access && old.remove_refused == snap.remove_refused && toasts::diff(&old.toasts, &snap.toasts).is_empty());
        if same {
            return;
        }
        let groups = toasts::group(&snap.toasts);
        let order = toasts::draw_order(&snap.toasts, &groups);
        self.cursor = toasts::keep_cursor(&self.order, self.cursor, &order);
        notif_state::told_by_panel(snap.access, snap.toasts.len(), cx);
        self.groups = groups;
        self.order = order;
        self.snapshot = Some(snap);
        self.refit(cx);
        cx.notify();
    }

    // ------------------------------------------------------------------ doing things (each is the user's own click or key)

    fn live_now(&self) -> bool {
        self.shown && !self.closing
    }

    /// Starts the program of a notification and puts the panel away.
    fn open_app(&mut self, id: u32, cx: &mut Context<Self>) {
        if !self.live_now() {
            return;
        }
        let Some(t) = self.snapshot.as_ref().and_then(|s| s.toasts.iter().find(|t| t.id == id)) else { return };
        if t.app_id.trim().is_empty() {
            return;
        }
        let app = AppEntry { name: t.app.clone(), id: t.app_id.clone() };
        self.closing = true;
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = startapps::launch(&app) {
                    log::debug!("notifications: a program could not be started: {e:#}");
                }
            })
            .detach();
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    fn remove(&mut self, id: u32) {
        if self.live_now() && self.can_remove() && let Some(l) = &self.live {
            l.remove(id);
        }
    }

    fn clear_all(&mut self) {
        if self.live_now() && self.can_remove() && let Some(l) = &self.live {
            l.clear();
        }
    }

    fn open_privacy(&mut self, cx: &mut Context<Self>) {
        if !self.live_now() {
            return;
        }
        self.closing = true;
        panel_parts::open_uri(toasts::PRIVACY_PAGE);
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    fn move_cursor(&mut self, down: bool, cx: &mut Context<Self>) {
        let Some(next) = notif_model::step_cursor(&self.order, self.cursor, down) else { return };
        self.cursor = Some(next);
        if let Some(s) = &self.snapshot
            && let Some(child) = notif_model::child_index(&self.groups, &s.toasts, next)
        {
            self.scroll.scroll_to_item(child);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ drawing the parts

    /// A program's picture at `logical` px, a first-letter box until it has arrived.
    fn picture(&mut self, app_id: &str, name: &str, look: &Look, cx: &mut Context<Self>) -> AnyElement {
        let scale = cx.try_global::<Env>().map_or(1.0, |e| e.scale);
        let px_size = (ICON * scale.max(1.0)).ceil() as u32;
        let key = format!("n:{app_id}@{px_size}");
        if !app_id.is_empty() {
            if let Some(img) = self.icons.get(&key) {
                return gpui_kit::img(img).size(px(ICON)).into_any_element();
            }
            if self.icons.start_loading(&key, Instant::now()) {
                let (k, src) = (key.clone(), IconSource::Packaged(app_id.to_string()));
                cx.spawn(async move |this, cx| {
                    let srcs = vec![src];
                    let bgra = cx.background_executor().spawn(async move { icons::load_bgra(&srcs, px_size) }).await;
                    let retry = this.update(cx, |me, cx| {
                        let retry = me.icons.finish(&k, bgra.and_then(icons::to_image), Instant::now());
                        cx.notify();
                        retry
                    });
                    if matches!(retry, Ok(true)) {
                        cx.background_executor().timer(icons::RETRY_AFTER).await;
                        let _ = this.update(cx, |_, cx| cx.notify());
                    }
                })
                .detach();
            }
        }
        div()
            .size(px(ICON))
            .flex_shrink_0()
            .rounded(px(ICON * 0.24))
            .bg(look.pal.placeholder_fill().hsla())
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(ICON * 0.5))
            .child(super::plan::initial(name))
            .into_any_element()
    }

    /// The date of an old notification in the user's own format.
    fn date_text(&self, created: i64) -> String {
        toasts::local_clock(created).map(|t| clock::format_pattern(&self.date_pattern, &t, &self.am, &self.pm)).unwrap_or_default()
    }

    /// `removable` comes from the snapshot being drawn (which `render` has taken out of `self` for the drawing: asking `self` here would
    /// always say no and the x would never be drawn).
    fn render_item(&self, t: &Toast, removable: bool, (now, today): (i64, i64), look: &Look, words: Words, cx: &mut Context<Self>) -> AnyElement {
        let id = t.id;
        let selected = self.cursor == Some(id);
        let (hover, pressed, ring) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla(), look.accent);
        let secondary = look.pal.text_secondary().hsla();
        // (a notification Windows gave no time has no age; it is not "1 January 1970")
        let when = toasts::age_of(t.created, now, today, || toasts::local_day(t.created)).map(|age| notif_model::time_text(age, &self.date_text(t.created), words)).unwrap_or_default();
        let x = panel_parts::round_button(("notif-x", id as usize), L::X, 24.0, look, None, Some(t!("Bu bildirimi kaldır")), true)
            .on_click(cx.listener(move |this, _, _, cx| {
                // (the click is the x's: the row under it must not start the program)
                cx.stop_propagation();
                this.remove(id);
            }));
        let body = toasts::body_lines(&t.body);
        h_flex()
            .id(("notif", id as usize))
            .group("notif-row")
            .w_full()
            .flex_shrink_0()
            .px(px(10.))
            .py(px(8.))
            .gap(px(8.))
            .items_start()
            .rounded(px(10.))
            .border_2()
            .border_color(if selected { ring } else { transparent_black() })
            .bg(look.pal.field_fill().hsla())
            .cursor_pointer()
            .hover(move |s| s.bg(hover))
            .active(move |s| s.bg(pressed))
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap(px(2.))
                    .child(
                        h_flex()
                            .gap(px(8.))
                            .items_baseline()
                            .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(t.title.clone()))
                            .child(div().flex_shrink_0().text_xs().text_color(secondary).child(when)),
                    )
                    .when(!body.is_empty(), |d| d.child(div().w_full().line_clamp(3).text_ellipsis().text_xs().text_color(secondary).child(body))),
            )
            .when(removable, |d| d.child(div().flex_shrink_0().opacity(if selected { 1.0 } else { 0.0 }).group_hover("notif-row", |s| s.opacity(1.0)).child(x)))
            .on_click(cx.listener(move |this, _, _, cx| this.open_app(id, cx)))
            .into_any_element()
    }

    fn render_list(&mut self, snap: &Snapshot, removable: bool, look: &Look, words: Words, cx: &mut Context<Self>) -> AnyElement {
        let (now, secondary) = (toasts::now_unix(), look.pal.text_secondary().hsla());
        let today = toasts::local_day(now);
        let groups = self.groups.clone();
        let mut column = v_flex().id("notif-list").w_full().flex_1().min_h_0().gap(px(4.)).overflow_y_scroll().track_scroll(&self.scroll);
        for g in &groups {
            let name = if g.app.trim().is_empty() { notif_model::unknown_app(words).to_string() } else { g.app.clone() };
            let picture = self.picture(&g.app_id, &name, look, cx);
            column = column.child(
                h_flex()
                    .h(px(GROUP_H))
                    .flex_shrink_0()
                    .px(px(6.))
                    .gap(px(8.))
                    .items_center()
                    .child(picture)
                    .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(secondary).child(name)),
            );
            for &i in &g.items {
                column = column.child(self.render_item(&snap.toasts[i], removable, (now, today), look, words, cx));
            }
        }
        column.into_any_element()
    }

    fn render_header(&self, look: &Look, show_clear: bool, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.stop("notif-clear", 0, cx);
        let (hover, pressed, ring) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla(), look.accent);
        h_flex()
            .h(px(notif_model::HEADER_H))
            .flex_shrink_0()
            .items_center()
            .px(px(4.))
            .child(div().flex_1().min_w_0().truncate().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(t!("Bildirimler")))
            .when(show_clear, |d| {
                d.child(
                    div()
                        .id("notif-clear")
                        .h(px(30.))
                        .px(px(10.))
                        .flex()
                        .items_center()
                        .rounded(px(8.))
                        .border_2()
                        .border_color(transparent_black())
                        .cursor_pointer()
                        .text_sm()
                        .track_focus(&focus)
                        .focus_visible(move |s| s.border_color(ring))
                        .hover(move |s| s.bg(hover))
                        .active(move |s| s.bg(pressed))
                        .child(t!("Tümünü temizle"))
                        .on_click(cx.listener(|this, _, _, _| this.clear_all())),
                )
            })
            .into_any_element()
    }

    fn render_message(&self, look: &Look, words: Words, access: Option<Access>, cx: &mut Context<Self>) -> AnyElement {
        let secondary = look.pal.text_secondary().hsla();
        let Some(a) = access else {
            // no notification
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .gap(px(8.))
                .child(Icon::new(L::Bell).size(px(28.)).text_color(secondary))
                .child(div().text_sm().text_color(secondary).child(t!("Bildirim yok")))
                .into_any_element();
        };
        let focus = self.stop("notif-privacy", 0, cx);
        let (hover, pressed, ring) = (look.pal.hover_fill().hsla(), look.pal.pressed_fill().hsla(), look.accent);
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .px(px(12.))
            .child(div().text_sm().text_center().text_color(secondary).child(notif_model::access_line(a, words)))
            .child(
                div()
                    .id("notif-privacy")
                    .h(px(34.))
                    .px(px(14.))
                    .flex()
                    .items_center()
                    .rounded(px(10.))
                    .border_2()
                    .border_color(transparent_black())
                    .bg(look.pal.field_fill().hsla())
                    .cursor_pointer()
                    .text_sm()
                    .track_focus(&focus)
                    .focus_visible(move |s| s.border_color(ring))
                    .hover(move |s| s.bg(hover))
                    .active(move |s| s.bg(pressed))
                    .child(t!("Bildirim ayarlarını aç"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_privacy(cx))),
            )
            .into_any_element()
    }
}

impl Render for NotifView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build, nothing of the user's is held
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let look = Look::of(pal);
        let words = Words::current();
        let fill = glass_watch::panel_fill(look.dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let frame = match self.anim {
            Some((t, opening)) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
            None => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
        };
        if !frame.done {
            window.request_animation_frame();
        }

        // (the snapshot is taken out for the drawing, which needs `self` for the pictures, and put back)
        let snap = self.snapshot.take();
        // (one answer for the header's "clear all" and every row's x, from the snapshot in hand)
        let removable = snap.as_ref().is_some_and(Snapshot::removal_offered);
        let header = self.render_header(&look, removable, cx);
        let content = match &snap {
            None => div().flex_1().into_any_element(),
            Some(s) if !s.access.may_read() => self.render_message(&look, words, Some(s.access), cx),
            Some(s) if s.toasts.is_empty() => self.render_message(&look, words, None, cx),
            Some(s) => self.render_list(s, removable, &look, words, cx),
        };
        self.snapshot = snap;

        let body = v_flex().size_full().p(px(notif_model::PAD)).gap(px(4.)).opacity(frame.opacity).mt(px(frame.drop_px)).child(header).child(content);
        div()
            .id("notifications-panel")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(look.pal.text().hsla())
            .text_size(px(13.))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| match ev.keystroke.key.as_str() {
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
                "down" | "up" => {
                    cx.stop_propagation();
                    this.move_cursor(ev.keystroke.key == "down", cx);
                }
                "enter" => {
                    if let Some(id) = this.cursor {
                        cx.stop_propagation();
                        this.open_app(id, cx);
                    }
                }
                "delete" | "backspace" => {
                    if let Some(id) = this.cursor {
                        cx.stop_propagation();
                        this.remove(id);
                    }
                }
                _ => {}
            }))
            .child(body)
            .into_any_element()
    }
}
