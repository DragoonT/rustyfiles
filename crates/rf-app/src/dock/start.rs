//! The Start panel: the view that opens above the left island when its logo (or its search field) is clicked. About 620 x 680 logical
//! px, on HyperGlass (`panel.rs` opens, places and closes it; this is what is inside).
//!
//! * Top: a search field with the keyboard in it. Typing is the search; Escape clears it first and closes the panel second.
//! * Empty query: "Sabitlenenler" (the user's pinned apps, a grid; right click any app to pin or unpin it) and "Tüm uygulamalar" (every
//!   installed app in the alphabet of the user's language with letter headers, in the app's virtual list: only the rows in view are
//!   built and only they ask for an icon).
//! * With a query: the result list of the quick search (`quick_search::search_local`, and the files the running rustyfiles answers
//!   over a pipe: `search_pipe`). Local rows appear within a frame of the keystroke; the files arrive after them without moving the row
//!   the user is on (`search.rs`).
//! * Bottom: the user's name, Settings (rustyfiles' own) and the power button (Kilitle, Uyku, Oturumu kapat, Yeniden başlat, Bilgisayarı
//!   kapat; the last two ask "Emin misiniz?" first, in the panel).
//!
//! Keyboard: typing goes to the field wherever the pointer was last clicked; the arrows move through the grid and the list (or the
//! results), Enter starts the one under the cursor, Ctrl+Enter shows it in its folder in rustyfiles, Escape steps back.
//!
//! Menus (right click on an app, the power button) are drawn inside this window: a separate menu window would take the activation and
//! the panel would close under it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::future::{Either, select};
use gpui_kit::assets::IconName as L;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_win::apps::App as AppEntry;
use rf_win::dock::IconSource;
use rf_win::dock::panel::{self as geo, CloseCause};
use rf_win::dock::power::{self, Action as PowerAction, Menu as PowerMenu, Step as PowerStep};
use rf_win::dock::startapps;
use rf_win::dock::startlist::{self, Cursor, Move, Row};
use rf_win::dock::startpins::{self, Loaded, PinFile, Plan};

use super::backdrop::IslandGlass;
use super::icons::{self, IconCache};
use super::look::{Env, Palette};
use super::search::{self, Entry, Files, Note, Selection};
use crate::glass_watch::{self, PopupGlass};
use crate::quick_search::{self, Item, Section, Target};
use crate::search_pipe::{self, Reply};
use crate::settings::Settings;
use crate::t;
use crate::theme::{self, Palette as ThemePalette};

const PAD: f32 = 22.0;
const SEARCH_H: f32 = 56.0;
const BOTTOM_H: f32 = 56.0;
const CELL_W: f32 = 96.0;
const CELL_H: f32 = 86.0;
/// The pinned grid shows three rows (the taskbar seed of a typical PC is 13 apps: six, six and one) and scrolls beyond them, keeping
/// the keyboard cursor in view.
const GRID_MAX_H: f32 = 3.0 * CELL_H + 4.0;
/// A start, a sign-out or a lock is waited for this long before the panel closes without knowing how it went; an answer that comes
/// sooner and says "failed" keeps the panel open with a note (a sleep call returns only when the PC wakes: it is never waited for).
const OUTCOME_WAIT: Duration = Duration::from_millis(300);
/// How long clicks and Enter are ignored after a click removed a menu or answered the question.
const QUIET: Duration = Duration::from_millis(350);
/// A row of "Tüm uygulamalar" and a letter header have the same height (the virtual list needs equal rows).
const ROW_H: f32 = 40.0;
const RESULT_H: f32 = 46.0;
const MENU_W: f32 = 236.0;
const MENU_ROW: f32 = 34.0;
#[cfg(test)]
const PANEL_W: f32 = super::panel::START_SIZE.0;
const PANEL_H: f32 = super::panel::START_SIZE.1;
/// The main app is waited for this long (and asked every 1.5 s) after the user chose to start it.
const MAIN_WAIT_TRIES: usize = 16;
/// A file answer later than this is given up on: the rows are shown without files.
const FILES_PATIENCE: Duration = Duration::from_millis(1500);

#[derive(Clone, Copy, PartialEq, Eq)]
enum How {
    Open,
    ShowInFolder,
}

/// The menu of an app (right click).
struct AppMenu {
    app: AppEntry,
    /// Where it was asked for, in the panel's coordinates.
    at: Point<Pixels>,
    /// The program the app starts, once the shell has said (the item is shown only when there is one).
    location: Option<PathBuf>,
}

/// The rows of the app menu, top to bottom.
/// Whether a click or Enter does anything: the panel is shown, it is not on its way out (a choice was made: a double click or a held
/// Enter must not choose twice) and no menu was dismissed a moment ago (the second click of a double click on a menu item must not
/// fall through to what lies under the place the menu was).
fn acts(shown: bool, closing: bool, now: Instant, quiet_until: Instant) -> bool {
    shown && !closing && now >= quiet_until
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppMenuItem {
    Pin,
    Admin,
    Location,
}

impl AppMenu {
    fn items(&self) -> Vec<AppMenuItem> {
        let mut v = vec![AppMenuItem::Pin, AppMenuItem::Admin];
        if self.location.is_some() {
            v.push(AppMenuItem::Location);
        }
        v
    }
}

pub struct StartView {
    island: Rc<RefCell<IslandGlass>>,
    hwnd: isize,
    popup: Option<PopupGlass>,
    _watch: Option<Task<()>>,
    /// The panel is on the screen or easing out (false while it is hidden: nothing is built then).
    shown: bool,
    /// The panel is on its way out (a choice was made, or it is easing out): clicks and keys do nothing, so a double click or a held
    /// Enter cannot start a thing twice, and the second click of a double click on a menu item cannot fall through to whatever lies
    /// under the menu once it is gone.
    closing: bool,
    /// Started at the open or the close; `true` is opening.
    anim: Option<(Instant, bool)>,
    /// Said above the bottom row when something that was asked for did not work.
    notice: Option<SharedString>,
    /// Clicks and Enter do nothing before this moment (set when a click removed a menu or answered the question).
    quiet_until: Instant,

    // the data
    apps: Arc<Vec<AppEntry>>,
    rows: Vec<Row>,
    /// The list `apps` was made from (a newer list from the shell is sorted again).
    source: Option<Arc<Vec<AppEntry>>>,
    apps_ready: bool,
    /// The last reading of the app list came back empty (the shell was busy or enumeration failed).
    apps_failed: bool,
    loading: bool,
    turkish: bool,
    pins: PinFile,
    pins_loaded: bool,
    /// The pinned apps that are installed: indexes into `apps`.
    pin_cells: Vec<usize>,
    user: String,

    // the interface
    input: Entity<InputState>,
    query: String,
    cursor: Option<Cursor>,
    list_scroll: UniformListScrollHandle,
    results_scroll: ScrollHandle,
    /// The pinned grid's scroll (the keyboard cursor is kept in view in it).
    pins_scroll: ScrollHandle,
    reveal_results: bool,
    app_menu: Option<AppMenu>,
    /// The row of the app menu the keyboard is on.
    app_hi: usize,
    power: PowerMenu,
    power_hi: usize,
    /// When the "Emin misiniz?" question appeared (an "Evet" in the first moments is not an answer: `power::yes_counts`).
    power_since: Option<Instant>,
    /// The keyboard is on "Evet" (it starts on "Hayır").
    confirm_yes: bool,
    _grace: Option<Task<()>>,
    /// Keyboard stops of the bottom row (Tab goes field, Settings, power).
    settings_focus: FocusHandle,
    power_focus: FocusHandle,

    // the search
    generation: u64,
    local_for: u64,
    local: Vec<Item>,
    files: Vec<Item>,
    files_state: Files,
    files_req: u64,
    entries: Vec<Entry>,
    selection: Selection,
    sel_ix: usize,
    pending: Option<How>,
    _search: Option<Task<()>>,
    _files: Option<Task<()>>,
    _starter: Option<Task<()>>,

    icons: IconCache,
    icon_scale: f32,
    _subs: Vec<Subscription>,
}

use std::cell::RefCell;
use std::rc::Rc;

static WRITER: startpins::OrderedWriter = startpins::OrderedWriter::new();

fn pins_path() -> Option<PathBuf> {
    crate::settings::app_dir().map(|d| d.join(startpins::FILE))
}

/// Reads the pin file; the first time (no file, or one that was never seeded) the taskbar's pins are copied in. `None`: nothing can
/// be decided now (the app list is not known yet, the taskbar's pins could not be read in time, or the file cannot be read and could
/// not be set aside), so nothing is written and the next opening asks again. A file that exists and cannot be read is moved aside,
/// never written over. Blocks (disk, the shell): a worker thread.
fn load_or_seed_pins(apps: &[AppEntry]) -> Option<PinFile> {
    let path = pins_path();
    let loaded = path.as_ref().map_or(Loaded::Absent, |p| startpins::load_checked(p));
    match startpins::plan(loaded, !apps.is_empty()) {
        Plan::Use(f) => Some(f),
        Plan::Wait => None,
        Plan::Seed { base, set_aside } => {
            if set_aside && let Some(p) = &path {
                match startpins::set_aside(p) {
                    Ok(to) => log::warn!("start pins: {} could not be read, kept as {}", p.display(), to.display()),
                    Err(e) => {
                        log::warn!("start pins: {} could not be read nor set aside ({e}); left alone", p.display());
                        return None;
                    }
                }
            }
            let taskbar = rf_win::dock::pins::read_pins_checked();
            let f = startpins::seeded_file(base, taskbar.as_deref(), apps)?;
            save_pins(f.clone());
            Some(f)
        }
    }
}

/// Writes the pin file on a thread of its own, in the order the snapshots were taken (a stale one is dropped).
fn save_pins(file: PinFile) {
    let Some(path) = pins_path() else { return };
    let ticket = WRITER.ticket();
    std::thread::spawn(move || {
        if let Err(e) = WRITER.write(ticket, &path, &file) {
            log::warn!("start pins: cannot save {}: {e}", path.display());
        }
    });
}

impl StartView {
    pub fn new(island: Rc<RefCell<IslandGlass>>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t!("Uygulama, dosya ya da web'de ara")));
        let subs = vec![cx.subscribe_in(&input, window, |this: &mut Self, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                this.on_query_changed(cx);
            }
        })];
        let me = StartView {
            island,
            hwnd: crate::look::window::hwnd_of(window),
            popup: None,
            _watch: None,
            shown: false,
            closing: false,
            anim: None,
            notice: None,
            quiet_until: Instant::now(),
            apps: Arc::new(Vec::new()),
            rows: Vec::new(),
            source: None,
            apps_ready: false,
            apps_failed: false,
            loading: false,
            turkish: rf_win::encoding::user_locale_is_turkish(),
            pins: PinFile::default(),
            pins_loaded: false,
            pin_cells: Vec::new(),
            user: String::new(),
            input,
            query: String::new(),
            cursor: None,
            list_scroll: UniformListScrollHandle::new(),
            results_scroll: ScrollHandle::new(),
            pins_scroll: ScrollHandle::new(),
            reveal_results: false,
            app_menu: None,
            app_hi: 0,
            power: PowerMenu::Closed,
            power_hi: 0,
            power_since: None,
            confirm_yes: false,
            _grace: None,
            settings_focus: cx.focus_handle(),
            power_focus: cx.focus_handle(),
            generation: 0,
            local_for: 0,
            local: Vec::new(),
            files: Vec::new(),
            files_state: Files::Idle,
            files_req: 0,
            entries: Vec::new(),
            selection: Selection::First,
            sel_ix: 0,
            pending: None,
            _search: None,
            _files: None,
            _starter: None,
            icons: IconCache::default(),
            icon_scale: 0.0,
            _subs: subs,
        };
        // (the list of apps, the pins and the user's name are read when the panel opens, `on_open`: the view is made by the first click
        // on the Start button, so there is nothing to warm up before it, and a timer started here would only read it all a second time)
        me
    }

    // ------------------------------------------------------------------ opening and closing (called by panel.rs)

    /// The panel is about to be shown: start from a clean state and begin easing in.
    pub fn on_open(&mut self, _dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = true;
        self.closing = false;
        self.notice = None;
        self.anim = Some((Instant::now(), true));
        self.app_menu = None;
        self.power = PowerMenu::Closed;
        self.power_since = None;
        self.confirm_yes = false;
        self._grace = None;
        self.cursor = None;
        self.pending = None;
        self.reset_query(window, cx);
        // what was opened since this dock started is in the file now
        quick_search::reload_usage();
        self.load_data(cx);
        cx.notify();
    }

    /// The window is on the screen: the glass goes on, the keyboard goes to the field.
    pub fn on_shown(&mut self, dark: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.popup.is_none() && self.shown {
            self.popup = Some(PopupGlass::start_panel(self.hwnd, dark));
            self._watch = Some(glass_watch::watch(cx, |this: &mut Self| this.popup.as_mut()));
        }
        self.focus_field(window, cx);
        cx.notify();
    }

    /// The panel is being put away: ease out, stop what was running for it.
    pub fn on_close(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.closing = true;
        self.anim = Some((Instant::now(), false));
        self._search = None;
        self._files = None;
        self._starter = None;
        self._grace = None;
        self.pending = None;
        self.files_req = 0;
        self.app_menu = None;
        self.power = PowerMenu::Closed;
        cx.notify();
    }

    /// The window is hidden: the glass comes off, the field is emptied, nothing is kept alive. The pictures of files that were found
    /// are let go (they would pile up over weeks of searches); the apps' own pictures stay.
    pub fn on_hidden(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = false;
        self.closing = false;
        self.anim = None;
        self.popup = None;
        self._watch = None;
        self.icons.retain(|k| k.starts_with("app:"));
        self.reset_query(window, cx);
        cx.notify();
    }

    /// The panel is open and not on its way out.
    fn open(&self) -> bool {
        self.shown && !self.closing
    }

    /// Whether a click or a key may DO something now: the panel is open, and no menu has just been dismissed by a click (see `quiet`).
    fn live(&self) -> bool {
        acts(self.shown, self.closing, Instant::now(), self.quiet_until)
    }

    fn say(&mut self, text: &'static str, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        cx.notify();
    }

    fn reset_query(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.query.clear();
        self.generation += 1;
        self.local.clear();
        self.files.clear();
        self.files_state = Files::Idle;
        self.entries.clear();
        self.selection = Selection::First;
        self.sel_ix = 0;
        self.input.update(cx, |s, cx| s.set_value("", window, cx));
        super::panel::set_typed("", cx);
    }

    fn focus_field(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |s, cx| s.focus(window, cx));
    }

    // ------------------------------------------------------------------ the data

    /// Reads the app list (cached by the quick search after the first time) and the pins in the background; sorts again only when the
    /// list is a new one.
    fn load_data(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        self.loading = true;
        let turkish = self.turkish;
        let have_pins = self.pins_loaded;
        let have_user = !self.user.is_empty();
        let last = self.source.clone();
        cx.spawn(async move |this, cx| {
            let work = cx.background_executor().spawn(async move {
                let list = quick_search::apps();
                let fresh = last.as_ref().is_none_or(|l| !Arc::ptr_eq(l, &list));
                let sorted = fresh.then(|| {
                    let mut v: Vec<AppEntry> = (*list).clone();
                    startlist::sort_apps(&mut v, turkish);
                    let rows = startlist::rows(&v, turkish);
                    (v, rows)
                });
                let pins = if have_pins { None } else { load_or_seed_pins(&list) };
                // (a domain account's display name can take a moment to come from the server: not on the UI thread)
                let user = (!have_user).then(rf_win::dock::power::display_name);
                (list, sorted, pins, user)
            });
            let (list, sorted, pins, user) = work.await;
            let _ = this.update(cx, |me, cx| {
                me.loading = false;
                if let Some(u) = user {
                    me.user = u;
                }
                me.source = Some(list);
                if let Some((v, rows)) = sorted {
                    me.apps_ready = !v.is_empty();
                    me.apps_failed = v.is_empty();
                    me.apps = Arc::new(v);
                    me.rows = rows;
                }
                if let Some(p) = pins {
                    me.pins = p;
                    me.pins_loaded = true;
                }
                me.rebuild_pin_cells();
                cx.notify();
            });
        })
        .detach();
    }

    fn rebuild_pin_cells(&mut self) {
        self.pin_cells = self.pins.resolve(&self.apps).into_iter().map(|(i, _)| i).collect();
        if let Some(Cursor::Pin(i)) = self.cursor
            && i >= self.pin_cells.len()
        {
            self.cursor = None;
        }
    }

    // ------------------------------------------------------------------ the search

    fn on_query_changed(&mut self, cx: &mut Context<Self>) {
        let q = self.input.read(cx).value().to_string();
        if q == self.query {
            return;
        }
        self.query = q.clone();
        super::panel::set_typed(&q, cx);
        self.cursor = None;
        self.generation += 1;
        self.selection = Selection::First;
        self.sel_ix = 0;
        self.pending = None;
        self.app_menu = None;
        if !search::wants_files(&q) {
            self._search = None;
            self._files = None;
            self.local.clear();
            self.files.clear();
            self.files_state = Files::Idle;
            self.entries.clear();
            cx.notify();
            return;
        }
        self.reveal_results = true;
        self.run_local(cx);
        self.request_files(cx);
        cx.notify();
    }

    /// Apps, bookmarks and the web: on a worker thread, back within a frame or two. The rows of the previous query stay until then.
    fn run_local(&mut self, cx: &mut Context<Self>) {
        let generation = self.generation;
        let query = self.query.clone();
        let s = cx.global::<Settings>();
        let (engine, bookmarks) = (s.web_search, s.quick_search_bookmarks);
        let work = cx.background_spawn(async move { quick_search::search_local(&query, engine, bookmarks) });
        self._search = Some(cx.spawn(async move |this, cx| {
            let local = work.await;
            let _ = this.update(cx, |me, cx| {
                if me.generation != generation {
                    return;
                }
                me.local = local;
                me.local_for = generation;
                me.rebuild_entries();
                me.reveal_results = true;
                cx.notify();
                me.run_pending(cx);
            });
        }));
    }

    /// Asks the running rustyfiles for files; its answer comes to `on_files_reply`. No answer to the pipe write means no rustyfiles.
    fn request_files(&mut self, cx: &mut Context<Self>) {
        let id = search_pipe::next_id();
        self.files_req = id;
        // (the rows of the previous query's files stay until this query's answer replaces them: files that vanish at every
        // keystroke and come back a moment later make the list jump under the user's hand)
        if self.files_state != Files::Starting {
            self.files_state = Files::Pending;
        }
        let req = search_pipe::Request { id, query: self.query.clone() };
        let ask = cx.background_spawn(async move { search_pipe::ask(&req) });
        self._files = Some(cx.spawn(async move |this, cx| {
            let sent = ask.await;
            let waiting = this.update(cx, |me, cx| {
                if me.files_req != id {
                    return false;
                }
                if sent {
                    me.files_state = Files::Pending;
                    return true;
                }
                // nobody is listening: no rustyfiles is running (unless the user has just asked for one, which is still on its way)
                if me.files_state != Files::Starting {
                    me.files_state = Files::NoMain;
                }
                me.rebuild_entries();
                cx.notify();
                me.run_pending(cx);
                false
            });
            if !matches!(waiting, Ok(true)) {
                return;
            }
            cx.background_executor().timer(FILES_PATIENCE).await;
            let _ = this.update(cx, |me, cx| {
                if me.files_req == id && me.files_state == Files::Pending {
                    me.files_state = Files::Silent;
                    me.rebuild_entries();
                    cx.notify();
                    me.run_pending(cx);
                }
            });
        }));
    }

    /// The main app answered.
    pub fn files_arrived(&mut self, reply: Reply, cx: &mut Context<Self>) {
        if reply.id != self.files_req || !self.shown {
            return;
        }
        self.files = reply.hits.iter().map(search::item_of).collect();
        self.files_state = Files::Done(reply.index);
        self.rebuild_entries();
        cx.notify();
        self.run_pending(cx);
    }

    fn rebuild_entries(&mut self) {
        self.entries = search::entries(&self.local, &self.files, self.files_state);
        self.sel_ix = self.selection.index(&self.entries);
    }

    /// An Enter that had to wait for the files (or the local rows) goes ahead when it no longer has to.
    fn run_pending(&mut self, cx: &mut Context<Self>) {
        if self.pending.is_some() && !self.must_wait() {
            let how = self.pending.take().unwrap_or(How::Open);
            self.activate(how, cx);
        }
    }

    fn must_wait(&self) -> bool {
        self.local_for != self.generation || search::enter_must_wait(&self.selection, &self.entries, self.files_state)
    }

    fn move_selection(&mut self, up: bool) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        let ix = startlist::step_results(self.sel_ix, up, n);
        self.selection = Selection::at(&self.entries, ix);
        self.sel_ix = ix;
        self.reveal_results = true;
    }

    // ------------------------------------------------------------------ doing things

    fn finish(&mut self, cx: &mut Context<Self>) {
        // (at once, not when the deferred close runs: a second click in the same frame must find the panel already closing)
        self.closing = true;
        cx.defer(|cx| super::panel::close(CloseCause::Chosen, cx));
    }

    /// After a click that removed a menu or answered a question, the next moments' clicks are not the user's: the second click of a
    /// double click would land on whatever lies under the place the menu was.
    fn quiet(&mut self) {
        self.quiet_until = Instant::now() + QUIET;
    }

    /// Runs a start, a sign-out or the like off the UI thread and closes the panel when it went through. A failure that is reported
    /// within `OUTCOME_WAIT` keeps the panel open with `failure` said in it; a call that takes longer (a sleep returns only when the PC
    /// wakes, Windows' consent prompt waits for the user) is not waited for. Declining the consent prompt is not a failure.
    fn run_then_close(&mut self, failure: &'static str, job: impl FnOnce() -> anyhow::Result<()> + Send + 'static, cx: &mut Context<Self>) {
        self.closing = true;
        self.notice = None;
        let work = cx.background_spawn(async move { job() });
        let timer = cx.background_executor().timer(OUTCOME_WAIT);
        cx.spawn(async move |this, cx| {
            let result = match select(work, timer).await {
                Either::Left((result, _)) => Some(result),
                // still going: it goes on without us, and the panel closes
                Either::Right((_, work)) => {
                    work.detach();
                    None
                }
            };
            let _ = this.update(cx, |me, cx| match result {
                Some(Err(e)) if !startapps::is_cancelled(&e) => {
                    log::warn!("start: {failure}: {e:#}");
                    me.closing = false;
                    me.say(failure, cx);
                }
                Some(Err(_)) => {
                    me.closing = false;
                    cx.notify();
                }
                _ => me.finish(cx),
            });
        })
        .detach();
        cx.notify();
    }

    fn launch_app(&mut self, app: AppEntry, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        quick_search::remember(&Item { target: Target::App { id: app.id.clone(), name: app.name.clone() }, title: app.name.clone(), detail: String::new(), section: Section::Apps, score: 0 });
        self.run_then_close(t!("Başlatılamadı. Kaldırılmış ya da taşınmış olabilir."), move || startapps::launch(&app), cx);
    }

    fn launch_app_as_admin(&mut self, app: AppEntry, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        // Windows shows its consent prompt: the user asked for it with this click
        self.run_then_close(t!("Başlatılamadı. Kaldırılmış ya da taşınmış olabilir."), move || startapps::launch_as_admin(&app), cx);
    }

    fn show_app_location(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        super::actions::run_rustyfiles(super::actions::show_in_folder_args(&path));
        self.finish(cx);
    }

    fn open_in_shell(path: PathBuf) {
        std::thread::spawn(move || {
            let p = path.clone();
            if rf_win::sta::call(Duration::from_secs(15), move || rf_win::shell::open(&p)).and_then(Result::ok).is_none() {
                log::warn!("start: cannot open {}", path.display());
            }
        });
    }

    fn toggle_pin(&mut self, app: &AppEntry, cx: &mut Context<Self>) {
        // (before the file has been read, a write would replace a list that is still to be loaded)
        if !self.live() || !self.pins_loaded {
            return;
        }
        if !self.pins.unpin(&app.id) {
            self.pins.pin(app);
        }
        self.rebuild_pin_cells();
        save_pins(self.pins.clone());
        cx.notify();
    }

    fn open_menu(&mut self, app: AppEntry, at: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        self.power = PowerMenu::Closed;
        self.app_hi = 0;
        let id = app.id.clone();
        let a = app.clone();
        self.app_menu = Some(AppMenu { app, at, location: None });
        cx.spawn(async move |this, cx| {
            let loc = cx.background_executor().spawn(async move { startapps::location(&a) }).await;
            let _ = this.update(cx, |me, cx| {
                if let Some(m) = me.app_menu.as_mut()
                    && m.app.id == id
                {
                    m.location = loc;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// A row of the app menu was chosen (clicked, or Enter on it).
    fn app_menu_do(&mut self, item: AppMenuItem, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let Some(m) = self.app_menu.take() else { return };
        self.quiet();
        match item {
            AppMenuItem::Pin => self.toggle_pin(&m.app, cx),
            AppMenuItem::Admin => self.launch_app_as_admin(m.app, cx),
            AppMenuItem::Location => {
                if let Some(p) = m.location {
                    self.show_app_location(p, cx);
                }
            }
        }
        cx.notify();
    }

    /// Where the app menu opens for the keyboard (Menu key, Shift+F10): by the thing the cursor is on, in the panel's coordinates.
    /// Roughly: the menu clamps itself inside the panel.
    fn keyboard_menu_target(&self) -> Option<(AppEntry, Point<Pixels>)> {
        let top = SEARCH_H + 1.0 + 34.0;
        if !self.query.trim().is_empty() {
            return match self.entries.get(self.sel_ix) {
                Some(Entry::Item(Item { target: Target::App { id, name }, .. })) => {
                    Some((AppEntry { id: id.clone(), name: name.clone() }, point(px(PAD + 60.0), px(SEARCH_H + 24.0 + self.sel_ix.min(8) as f32 * RESULT_H))))
                }
                _ => None,
            };
        }
        match self.cursor? {
            Cursor::Pin(i) => {
                let app = self.pin_cells.get(i).and_then(|&a| self.apps.get(a)).cloned()?;
                let (col, row) = (i % startlist::GRID_COLS, (i / startlist::GRID_COLS).min(2));
                Some((app, point(px(PAD + col as f32 * CELL_W + CELL_W / 2.0), px(top + row as f32 * CELL_H + CELL_H / 2.0))))
            }
            Cursor::App(j) => {
                let app = self.apps.get(j).cloned()?;
                Some((app, point(px(PAD + 80.0), px(top + GRID_MAX_H.min(PANEL_H / 3.0) + 120.0))))
            }
        }
    }

    /// Enter (`Open`) or Ctrl+Enter (`ShowInFolder`) on what the cursor or the selection is on.
    fn activate(&mut self, how: How, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        if self.query.trim().is_empty() {
            let app = match self.cursor {
                Some(Cursor::Pin(i)) => self.pin_cells.get(i).and_then(|&a| self.apps.get(a)).cloned(),
                Some(Cursor::App(j)) => self.apps.get(j).cloned(),
                None => None,
            };
            if let Some(app) = app {
                self.activate_app(app, how, cx);
            }
            return;
        }
        if self.must_wait() {
            self.pending = Some(how);
            return;
        }
        let Some(entry) = self.entries.get(self.sel_ix).cloned() else { return };
        match entry {
            Entry::StartMain => self.start_main(cx),
            Entry::Item(item) => {
                let engine = cx.global::<Settings>().web_search;
                match (&item.target, how) {
                    (Target::App { id, name }, how) => {
                        self.activate_app(AppEntry { id: id.clone(), name: name.clone() }, how, cx);
                        return;
                    }
                    (Target::Path { path, .. }, How::ShowInFolder) => self.show_app_location(path.clone(), cx),
                    (Target::Path { path, dir: true }, How::Open) => {
                        super::actions::run_rustyfiles(vec![path.display().to_string()]);
                        self.finish(cx);
                    }
                    (Target::Path { path, dir: false }, How::Open) => {
                        Self::open_in_shell(path.clone());
                        self.finish(cx);
                    }
                    (Target::Url { url }, _) => {
                        Self::open_in_shell(PathBuf::from(url));
                        self.finish(cx);
                    }
                    (Target::Web { query }, _) => {
                        Self::open_in_shell(PathBuf::from(quick_search::web_url(engine, query)));
                        self.finish(cx);
                    }
                }
                quick_search::remember(&item);
            }
        }
    }

    fn activate_app(&mut self, app: AppEntry, how: How, cx: &mut Context<Self>) {
        match how {
            How::Open => self.launch_app(app, cx),
            How::ShowInFolder => {
                // the program's place is found on the shell thread; the panel closes when it is known
                cx.spawn(async move |this, cx| {
                    let a = app.clone();
                    let loc = cx.background_executor().spawn(async move { startapps::location(&a) }).await;
                    let _ = this.update(cx, |me, cx| match loc {
                        Some(p) => me.show_app_location(p, cx),
                        // (a Store app has no program to show; starting it would not be what was asked)
                        None => me.say(t!("Bu uygulamanın klasörü yok"), cx),
                    });
                })
                .detach();
            }
        }
    }

    /// The row "start rustyfiles in the background" was chosen: start it and ask again when it answers on its pipe.
    fn start_main(&mut self, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        super::actions::start_main_in_background();
        self.files_state = Files::Starting;
        self.rebuild_entries();
        self._starter = Some(cx.spawn(async move |this, cx| {
            for _ in 0..MAIN_WAIT_TRIES {
                cx.background_executor().timer(Duration::from_millis(1500)).await;
                let up = cx.background_executor().spawn(async { search_pipe::main_is_running() }).await;
                if up {
                    let _ = this.update(cx, |me, cx| {
                        if me.shown && !me.query.trim().is_empty() {
                            me.request_files(cx);
                            cx.notify();
                        }
                    });
                    return;
                }
            }
            let _ = this.update(cx, |me, cx| {
                if me.files_state == Files::Starting {
                    me.files_state = Files::NoMain;
                    me.rebuild_entries();
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    // ------------------------------------------------------------------ the power menu

    /// Moves the menu to a new state; a question that has just appeared starts its grace period (see `power::yes_counts`).
    fn set_power(&mut self, menu: PowerMenu, cx: &mut Context<Self>) {
        let before = self.power;
        self.power = menu;
        if matches!(menu, PowerMenu::Confirming(_)) {
            if before != menu {
                self.power_since = Some(Instant::now());
                self.confirm_yes = false;
                // "Evet" is drawn faint until it counts: drawn again when it starts to
                self._grace = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_millis(power::CONFIRM_GRACE_MS + 20)).await;
                    let _ = this.update(cx, |_, cx| cx.notify());
                }));
            }
        } else {
            self.power_since = None;
            self._grace = None;
        }
        cx.notify();
    }

    /// Whether an "Evet" counts yet.
    fn yes_counts(&self) -> bool {
        self.power_since.is_some_and(|t| power::yes_counts(t.elapsed().as_millis() as u64))
    }

    fn choose_power(&mut self, action: PowerAction, cx: &mut Context<Self>) {
        if !self.live() {
            return;
        }
        let (menu, step) = self.power.choose(action);
        self.quiet();
        self.set_power(menu, cx);
        self.run_power(step, cx);
    }

    fn answer_power(&mut self, yes: bool, cx: &mut Context<Self>) {
        if !self.live() || (yes && !self.yes_counts()) {
            return;
        }
        let (menu, step) = self.power.answer(yes);
        self.quiet();
        self.set_power(menu, cx);
        self.run_power(step, cx);
    }

    fn run_power(&mut self, step: PowerStep, cx: &mut Context<Self>) {
        if let PowerStep::Run(action) = step {
            // the real call, and only here: the user's click (or Enter) on the item, after the question where it has one
            self.run_then_close(t!("Bu işlem yapılamadı. Yeniden deneyin."), move || power::perform(action), cx);
        }
        cx.notify();
    }

    // ------------------------------------------------------------------ keys

    /// Tab: the field, then Settings, then the power button.
    fn tab(&mut self, back: bool, window: &mut Window, cx: &mut Context<Self>) {
        let at = if self.settings_focus.is_focused(window) {
            1
        } else if self.power_focus.is_focused(window) {
            2
        } else {
            0
        };
        match if back { (at + 2) % 3 } else { (at + 1) % 3 } {
            0 => self.focus_field(window, cx),
            1 => window.focus(&self.settings_focus, cx),
            _ => window.focus(&self.power_focus, cx),
        }
        cx.notify();
    }

    fn on_key(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        if !self.open() {
            // a panel on its way out swallows its keys: a second Enter must not start a second thing, a letter must not be typed
            cx.stop_propagation();
            return;
        }
        let m = ev.keystroke.modifiers;
        if ev.is_held && key == "enter" {
            cx.stop_propagation();
            return;
        }
        self.notice = None;
        let footer = self.settings_focus.is_focused(window) || self.power_focus.is_focused(window);

        // the power menu and the app menu take the keys while they are open
        if self.power != PowerMenu::Closed {
            match (self.power, key) {
                (_, "escape") => {
                    let (menu, _) = self.power.escape();
                    self.set_power(menu, cx);
                    cx.stop_propagation();
                }
                (PowerMenu::Open, "up" | "down") => {
                    self.power_hi = startlist::step_results(self.power_hi, key == "up", PowerAction::ALL.len());
                    cx.stop_propagation();
                    cx.notify();
                }
                (PowerMenu::Open, "enter") => {
                    cx.stop_propagation();
                    self.choose_power(PowerAction::ALL[self.power_hi.min(PowerAction::ALL.len() - 1)], cx);
                }
                // the question: the keyboard starts on "Hayır"; Enter answers what it is on
                (PowerMenu::Confirming(_), "left" | "right" | "tab") => {
                    self.confirm_yes = !self.confirm_yes;
                    cx.stop_propagation();
                    cx.notify();
                }
                (PowerMenu::Confirming(_), "enter") => {
                    cx.stop_propagation();
                    self.answer_power(self.confirm_yes, cx);
                }
                _ => {}
            }
            return;
        }
        if let Some(menu) = &self.app_menu {
            let items = menu.items();
            let n = items.len();
            match key {
                "escape" => {
                    self.app_menu = None;
                    cx.stop_propagation();
                    cx.notify();
                }
                "up" | "down" => {
                    self.app_hi = startlist::step_results(self.app_hi.min(n - 1), key == "up", n);
                    cx.stop_propagation();
                    cx.notify();
                }
                "enter" => {
                    cx.stop_propagation();
                    let item = items[self.app_hi.min(n - 1)];
                    self.app_menu_do(item, cx);
                }
                _ => {}
            }
            return;
        }
        if key == "tab" {
            cx.stop_propagation();
            self.tab(m.shift, window, cx);
            return;
        }
        // typing from the bottom row's buttons goes to the field: the field takes the focus, and the character that follows is typed
        if footer && ev.keystroke.key_char.is_some() && !m.control && !m.alt && !m.platform && key != "space" && key != "enter" {
            self.focus_field(window, cx);
        }
        let searching = !self.query.trim().is_empty();
        match key {
            "escape" => {
                cx.stop_propagation();
                if !self.query.is_empty() {
                    self.reset_query(window, cx);
                    cx.notify();
                } else {
                    cx.defer(|cx| super::panel::close(CloseCause::Escape, cx));
                }
            }
            "menu" | "f10" if key == "menu" || m.shift => {
                cx.stop_propagation();
                if let Some((app, at)) = self.keyboard_menu_target() {
                    self.open_menu(app, at, cx);
                }
            }
            "down" | "up" => {
                cx.stop_propagation();
                if searching {
                    self.move_selection(key == "up");
                } else {
                    self.move_cursor(if key == "up" { Move::Up } else { Move::Down });
                }
                cx.notify();
            }
            "pagedown" | "pageup" if !searching => {
                cx.stop_propagation();
                self.move_cursor(if key == "pageup" { Move::PageUp } else { Move::PageDown });
                cx.notify();
            }
            // left and right are the text field's, except while the cursor is in the pinned grid
            "left" | "right" if !searching && matches!(self.cursor, Some(Cursor::Pin(_))) => {
                cx.stop_propagation();
                self.move_cursor(if key == "left" { Move::Left } else { Move::Right });
                cx.notify();
            }
            // (on a button of the bottom row Enter is the button's: it is clicked by the key's release)
            "enter" if !footer => {
                cx.stop_propagation();
                self.activate(if m.control { How::ShowInFolder } else { How::Open }, cx);
            }
            _ => {}
        }
    }

    fn move_cursor(&mut self, mv: Move) {
        self.cursor = startlist::step(self.cursor, mv, self.pin_cells.len(), self.apps.len());
        match self.cursor {
            Some(Cursor::App(j)) => {
                if let Some(row) = startlist::row_of_app(&self.rows, j) {
                    self.list_scroll.scroll_to_item(row, ScrollStrategy::Nearest);
                }
            }
            // (a pin in a row that is scrolled out of the grid is brought into it)
            Some(Cursor::Pin(i)) => self.pins_scroll.scroll_to_item(i),
            None => {}
        }
    }

    // ------------------------------------------------------------------ icons

    fn icon_px(&self, logical: f32) -> u32 {
        (logical * self.icon_scale.max(1.0)).ceil() as u32
    }

    /// The picture for `key` if it has arrived (a first-letter box until then); starts loading it when nobody has.
    fn picture(&mut self, key: String, src: IconSource, name: &str, logical: f32, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        let px_size = self.icon_px(logical);
        let key = format!("{key}@{px_size}");
        if let Some(img) = self.icons.get(&key) {
            return gpui_kit::img(img).size(px(logical)).into_any_element();
        }
        if self.icons.start_loading(&key, Instant::now()) {
            let k = key.clone();
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
        div()
            .size(px(logical))
            .rounded(px(logical * 0.24))
            .bg(pal.placeholder_fill().hsla())
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(logical * 0.45))
            .child(super::plan::initial(name))
            .into_any_element()
    }

    fn app_picture(&mut self, app: &AppEntry, logical: f32, pal: Palette, cx: &mut Context<Self>) -> AnyElement {
        self.picture(format!("app:{}", app.id), IconSource::Packaged(app.id.clone()), &app.name, logical, pal, cx)
    }
}

/// The answer of the main app, from the dock's pipe (called by the host's message loop).
pub fn on_files_reply(line: &str, cx: &mut App) {
    let Some(reply) = search_pipe::decode_reply(line) else { return };
    if let Some(view) = super::panel::view(cx) {
        view.update(cx, |v, cx| v.files_arrived(reply, cx));
    }
}

// ---------------------------------------------------------------------- drawing

impl Render for StartView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // hidden: nothing to build
        if !self.shown {
            return div().size_full().into_any_element();
        }
        let scale = cx.global::<Env>().scale;
        if (scale - self.icon_scale).abs() > 0.001 {
            self.icon_scale = scale;
            self.icons.clear();
        }
        let (pal, _) = super::host::frame_palette(&self.island, window);
        let dark = pal.mix < 0.5;
        let fill = glass_watch::panel_fill(dark, self.popup.as_ref().is_none_or(|g| g.fallback()));
        let frame = match self.anim {
            Some((t, opening)) => geo::frame(t.elapsed().as_millis() as u32, opening, crate::settings::animations()),
            None => geo::Frame { opacity: 1.0, drop_px: 0.0, done: true },
        };
        if !frame.done {
            window.request_animation_frame();
        }
        let searching = !self.query.trim().is_empty();
        let theme = theme_of(pal);

        let body = if searching { self.render_results(pal, theme, cx) } else { self.render_start(pal, theme, window, cx) };
        let notice = self.notice.clone();
        let content = v_flex()
            .size_full()
            .opacity(frame.opacity)
            .mt(px(frame.drop_px))
            .child(self.render_search_bar(pal, cx))
            .child(div().h(px(1.)).w_full().bg(pal.separator().hsla()))
            .child(div().flex_1().min_h_0().child(body))
            .when_some(notice, |d, n| d.child(div().px(px(PAD)).py(px(6.)).text_xs().text_color(pal.text_secondary().hsla()).child(n)))
            .child(div().h(px(1.)).w_full().bg(pal.separator().hsla()))
            .child(self.render_bottom(pal, theme, cx));

        div()
            .id("start-panel")
            .relative()
            .size_full()
            .bg(fill)
            .text_color(pal.text().hsla())
            .text_size(px(13.))
            .capture_key_down(cx.listener(|this, ev: &KeyDownEvent, window, cx| this.on_key(ev, window, cx)))
            // a click anywhere in the panel leaves the keyboard in the field: typing goes there wherever the click was
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| this.focus_field(window, cx)))
            .child(content)
            .children(self.render_overlays(pal, window, cx))
            .into_any_element()
    }
}

/// The app's palette for the panel's tone (its selection fill, accent and accent pill are used for the keyboard cursor, like every list).
fn theme_of(pal: Palette) -> ThemePalette {
    if pal.mix < 0.5 { ThemePalette::dark() } else { ThemePalette::light() }
}

impl StartView {
    fn render_search_bar(&mut self, pal: Palette, _cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h(px(SEARCH_H))
            .flex_shrink_0()
            .px(px(PAD))
            .gap_3()
            .items_center()
            .child(Icon::new(L::Search).size(px(18.)).text_color(pal.text_secondary().hsla()))
            .child(div().flex_1().text_lg().child(Input::new(&self.input).appearance(false)))
    }

    /// The empty query: pinned grid, then all apps.
    fn render_start(&mut self, pal: Palette, theme: ThemePalette, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.apps_ready {
            let text = if self.apps_failed && !self.loading { t!("Uygulama listesi okunamadı. Yeniden denemek için paneli kapatıp açın.") } else { t!("Uygulamalar yükleniyor…") };
            return div().p(px(PAD)).text_color(pal.text_secondary().hsla()).child(text).into_any_element();
        }
        let hover = pal.hover_fill().hsla();
        let secondary = pal.text_secondary().hsla();
        let sel_fill = theme.selection;

        // the pinned grid: the scroll container itself wraps its cells, so a cell can be scrolled to by its number
        let cells: Vec<usize> = self.pin_cells.clone();
        let mut grid = h_flex().id("pins").flex_wrap().w_full().max_h(px(GRID_MAX_H)).overflow_y_scroll().track_scroll(&self.pins_scroll);
        for (i, &ai) in cells.iter().enumerate() {
            let Some(app) = self.apps.get(ai).cloned() else { continue };
            let selected = self.cursor == Some(Cursor::Pin(i));
            let icon = self.app_picture(&app, 40.0, pal, cx);
            let (a1, a2) = (app.clone(), app.clone());
            grid = grid.child(
                v_flex()
                    .id(("pin", i))
                    .relative()
                    .w(px(CELL_W))
                    .h(px(CELL_H))
                    .flex_shrink_0()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .when(selected, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
                    .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                    .child(icon)
                    .child(div().w_full().px(px(6.)).text_xs().text_center().truncate().child(app.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.launch_app(a1.clone(), cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(a2.clone(), ev.position, cx))),
            );
        }
        let pinned_block = if cells.is_empty() {
            div().h(px(44.)).flex().items_center().text_color(secondary).child(t!("Henüz sabitlenmiş uygulama yok. Bir uygulamaya sağ tıklayıp (ya da Menü tuşuna basıp) “Başlat'a sabitle”yi seçin.")).into_any_element()
        } else {
            grid.into_any_element()
        };

        // all apps
        let n = self.rows.len();
        let list = uniform_list(
            "start-all-apps",
            n,
            cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                let pal = this.island.borrow().palette(super::backdrop::now_ms());
                let theme = theme_of(pal);
                let (hover, sel_fill) = (pal.hover_fill().hsla(), theme.selection);
                let wanted: Vec<(usize, Row)> = range.filter_map(|r| this.rows.get(r).cloned().map(|row| (r, row))).collect();
                wanted
                    .into_iter()
                    .map(|(r, row)| match row {
                        Row::Header(h) => div()
                            .id(("start-header", r))
                            .h(px(ROW_H))
                            .flex()
                            .items_center()
                            .px(px(10.))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(pal.text_secondary().hsla())
                            .child(h)
                            .into_any_element(),
                        Row::App(ai) => {
                            let Some(app) = this.apps.get(ai).cloned() else { return div().h(px(ROW_H)).into_any_element() };
                            let selected = this.cursor == Some(Cursor::App(ai));
                            let icon = this.app_picture(&app, 24.0, pal, cx);
                            let (a1, a2) = (app.clone(), app.clone());
                            h_flex()
                                .id(("start-app", r))
                                .relative()
                                .h(px(ROW_H))
                                .px(px(10.))
                                .gap_3()
                                .items_center()
                                .rounded(px(8.))
                                .cursor_pointer()
                                .when(selected, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
                                .when(!selected, |d| d.hover(move |s| s.bg(hover)))
                                .child(icon)
                                .child(div().flex_1().min_w_0().truncate().text_sm().child(app.name.clone()))
                                .on_click(cx.listener(move |this, _, _, cx| this.launch_app(a1.clone(), cx)))
                                .on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(a2.clone(), ev.position, cx)))
                                .into_any_element()
                        }
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .track_scroll(&self.list_scroll)
        .flex_1()
        .min_h_0();

        v_flex()
            .size_full()
            .px(px(PAD))
            .child(self.section_label(t!("Sabitlenenler"), secondary))
            .child(pinned_block)
            .child(self.section_label(t!("Tüm uygulamalar"), secondary))
            .child(list)
            .into_any_element()
    }

    fn section_label(&self, text: &'static str, color: Hsla) -> impl IntoElement {
        div().h(px(34.)).flex().items_end().pb(px(4.)).px(px(10.)).text_sm().font_weight(FontWeight::SEMIBOLD).text_color(color).child(text)
    }

    /// With a query: the rows of the quick search, sectioned.
    fn render_results(&mut self, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> AnyElement {
        let (hover, sel_fill) = (pal.hover_fill().hsla(), theme.selection);
        let (secondary, tertiary) = (pal.text_secondary().hsla(), pal.text_secondary().hsla());
        let entries = self.entries.clone();
        let mut list = v_flex().px(px(12.)).pb(px(8.));
        let mut last: Option<Section> = None;
        let (mut child, mut selected_child) = (0usize, 0usize);
        for (ix, entry) in entries.iter().enumerate() {
            let section = match entry {
                Entry::Item(i) => i.section,
                Entry::StartMain => Section::Files,
            };
            if last != Some(section) {
                last = Some(section);
                list = list.child(div().px(px(10.)).pt(px(10.)).pb(px(4.)).text_xs().text_color(tertiary).child(quick_search::section_title(section)));
                child += 1;
            }
            if ix == self.sel_ix {
                selected_child = child;
            }
            child += 1;
            let sel = ix == self.sel_ix;
            let (title, detail, icon): (SharedString, String, AnyElement) = match entry {
                Entry::StartMain => (
                    t!("Dosyaları aramak için rustyfiles'ı arka planda başlat").into(),
                    t!("Dosya dizini ana uygulamada tutulur; 10 dakika aranmazsa kendiliğinden kapanır").to_string(),
                    Icon::new(L::HardDrive).size(px(20.)).text_color(secondary).into_any_element(),
                ),
                Entry::Item(item) => {
                    let title: SharedString = match &item.target {
                        Target::Web { query } => format!("\u{201c}{query}\u{201d} {}", t!("için webde ara")).into(),
                        _ => item.title.clone().into(),
                    };
                    let icon = match &item.target {
                        Target::App { id, name } => self.app_picture(&AppEntry { id: id.clone(), name: name.clone() }, 28.0, pal, cx),
                        Target::Path { path, .. } => self.picture(format!("file:{}", path.to_string_lossy().to_lowercase()), IconSource::File(path.clone()), &item.title, 28.0, pal, cx),
                        Target::Url { .. } => Icon::new(L::Globe).size(px(20.)).text_color(secondary).into_any_element(),
                        Target::Web { .. } => Icon::new(L::Search).size(px(20.)).text_color(secondary).into_any_element(),
                    };
                    (title, item.detail.clone(), icon)
                }
            };
            let hint = if sel { "Enter" } else { "" };
            let app_for_menu = match entry {
                Entry::Item(Item { target: Target::App { id, name }, .. }) => Some(AppEntry { id: id.clone(), name: name.clone() }),
                _ => None,
            };
            let mut row = h_flex()
                .id(("start-result", ix))
                .relative()
                .h(px(RESULT_H))
                .px(px(10.))
                .gap_3()
                .items_center()
                .rounded(px(8.))
                .cursor_pointer()
                .when(sel, |d| d.bg(sel_fill).child(theme::selection_pill(&theme)))
                .when(!sel, |d| d.hover(move |s| s.bg(hover)))
                .child(div().size(px(28.)).flex_shrink_0().flex().items_center().justify_center().child(icon))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().text_sm().truncate().child(title))
                        .when(!detail.is_empty(), |d| d.child(div().text_xs().text_color(tertiary).truncate().child(detail))),
                )
                .when(!hint.is_empty(), |d| d.child(div().flex_shrink_0().text_xs().text_color(tertiary).child(hint)))
                .on_click(cx.listener(move |this, ev: &ClickEvent, _, cx| {
                    this.selection = Selection::at(&this.entries, ix);
                    this.sel_ix = ix;
                    this.pending = None;
                    this.activate(if ev.modifiers().control { How::ShowInFolder } else { How::Open }, cx);
                }));
            if let Some(app) = app_for_menu {
                row = row.on_mouse_down(MouseButton::Right, cx.listener(move |this, ev: &MouseDownEvent, _, cx| this.open_menu(app.clone(), ev.position, cx)));
            }
            list = list.child(row);
        }
        if std::mem::take(&mut self.reveal_results) {
            self.results_scroll.scroll_to_item(selected_child);
        }
        // Ctrl+Enter does something for a file or folder, and for an app that has a program to show; for the rest it is not offered
        let can_show = matches!(self.entries.get(self.sel_ix), Some(Entry::Item(Item { target: Target::Path { .. } | Target::App { .. }, .. })));
        let note = search::note(self.files_state).map(|n| match n {
            Note::IndexBuilding => t!("Dosya dizini hazırlanıyor: bazı dosyalar henüz çıkmayabilir"),
            Note::IndexOff => t!("Dosya araması kapalı: Ayarlar'da ad dizinini açın"),
            Note::MainStarting => t!("Ana uygulama başlatılıyor: dosyalar birazdan gelir"),
        });
        v_flex()
            .size_full()
            .when_some(note, |d, n| d.child(div().px(px(PAD)).pt(px(8.)).text_xs().text_color(tertiary).child(n)))
            .child(div().id("start-results").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.results_scroll).child(list))
            .when(!entries.is_empty(), |d| {
                d.child(
                    h_flex()
                        .h(px(30.))
                        .px(px(PAD))
                        .gap_4()
                        .items_center()
                        .border_t_1()
                        .border_color(pal.separator().hsla())
                        .text_xs()
                        .text_color(tertiary)
                        .child(t!("Enter: aç"))
                        .when(can_show, |d| d.child(t!("Ctrl+Enter: klasörde göster")))
                        .child(t!("Esc: temizle")),
                )
            })
            .into_any_element()
    }

    fn render_bottom(&mut self, pal: Palette, theme: ThemePalette, cx: &mut Context<Self>) -> impl IntoElement {
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
        let secondary = pal.text_secondary().hsla();
        let ring = theme.accent;
        let initial = self.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let button = |id: &'static str, icon: L, on: bool, focus: &FocusHandle, tip: &'static str| {
            div()
                .id(id)
                .size(px(36.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                // (room for the keyboard ring, which is the border's colour)
                .border_2()
                .border_color(transparent_black())
                .cursor_pointer()
                .track_focus(focus)
                .focus_visible(move |s| s.border_color(ring))
                .when(on, |d| d.bg(pressed))
                .when(!on, |d| d.hover(move |s| s.bg(hover)))
                .active(move |s| s.bg(pressed))
                .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(tip).build(w, cx))
                .child(Icon::new(icon).size(px(18.)).text_color(pal.text().hsla()))
        };
        let (settings_focus, power_focus) = (self.settings_focus.clone(), self.power_focus.clone());
        h_flex()
            .h(px(BOTTOM_H))
            .flex_shrink_0()
            .px(px(PAD))
            .gap_2()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .items_center()
                    .child(
                        div()
                            .size(px(32.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(pal.field_fill().hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(secondary)
                            .child(initial),
                    )
                    .child(div().min_w_0().truncate().text_sm().child(self.user.clone())),
            )
            .child(button("start-settings", L::Settings, false, &settings_focus, t!("Ayarlar")).on_click(cx.listener(|this, _, _, cx| {
                if !this.live() {
                    return;
                }
                // (Windows' Settings, as the gear of Windows' own Start does; rustyfiles' settings are in its own window)
                super::panel_parts::open_uri("ms-settings:");
                this.finish(cx);
            })))
            .child(button("start-power", L::Power, self.power != PowerMenu::Closed, &power_focus, t!("Güç")).on_click(cx.listener(|this, _, _, cx| {
                if !this.live() {
                    return;
                }
                this.app_menu = None;
                this.power_hi = 0;
                let toggled = this.power.toggled();
                this.set_power(toggled, cx);
            })))
    }

    /// The power menu and the app menu, drawn over the panel with a backdrop that closes them. They are placed from the panel's real
    /// size (its window is shorter than the design size on a small screen), the power menu from the bottom edge.
    fn render_overlays(&mut self, pal: Palette, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        let dark = pal.mix < 0.5;
        let menu_fill = glass_watch::panel_fill(dark, true);
        let (hover, pressed) = (pal.hover_fill().hsla(), pal.pressed_fill().hsla());
        let text = pal.text().hsla();
        let border = pal.separator().hsla();
        let size = window.viewport_size();
        let (view_w, view_h) = (f32::from(size.width), f32::from(size.height));
        let item = move |id: (&'static str, usize), icon: L, label: &'static str, hot: bool| {
            h_flex()
                .id(id)
                .h(px(MENU_ROW))
                .px(px(10.))
                .gap_3()
                .items_center()
                .rounded(theme::RADIUS_SM)
                .cursor_pointer()
                .text_sm()
                .when(hot, |d| d.bg(pressed))
                .when(!hot, |d| d.hover(move |s| s.bg(hover)))
                .child(Icon::new(icon).size(px(16.)).text_color(text))
                .child(label)
        };
        let dismiss = dismiss_listener;
        let backdrop = backdrop_of;

        // ---- the app menu
        if let Some(m) = &self.app_menu {
            let pinned = self.pins.is_pinned(&m.app.id);
            let items = m.items();
            let h = items.len() as f32 * MENU_ROW + 12.0;
            let x = f32::from(m.at.x).min(view_w - MENU_W - 8.0).max(8.0);
            let y = f32::from(m.at.y).min(view_h - h - 8.0).max(8.0);
            let hi = self.app_hi.min(items.len().saturating_sub(1));
            out.push(backdrop(cx).into_any_element());
            let mut col = v_flex()
                .id("start-app-menu")
                .absolute()
                .left(px(x))
                .top(px(y))
                .w(px(MENU_W))
                .p(px(6.))
                .rounded(theme::RADIUS)
                .bg(menu_fill)
                .border_1()
                .border_color(border)
                .shadow_lg()
                .occlude();
            for (i, it) in items.into_iter().enumerate() {
                let (icon, label) = match it {
                    AppMenuItem::Pin if pinned => (L::PinOff, t!("Sabitlemeyi kaldır")),
                    AppMenuItem::Pin => (L::Pin, t!("Başlat'a sabitle")),
                    AppMenuItem::Admin => (L::ShieldCheck, t!("Yönetici olarak çalıştır")),
                    AppMenuItem::Location => (L::FolderOpen, t!("Dosya konumunu aç")),
                };
                col = col.child(item(("m", i), icon, label, i == hi).on_click(cx.listener(move |this, _, _, cx| this.app_menu_do(it, cx))));
            }
            out.push(col.into_any_element());
        }

        // ---- the power menu, above the power button at the bottom right
        match self.power {
            PowerMenu::Closed => {}
            PowerMenu::Confirming(action) => {
                // The question is in the middle of the panel, not where the menu's rows were: it appears under the pointer that chose
                // "Yeniden başlat" or "Bilgisayarı kapat", and a second click of a double click must not land on its "Evet".
                // (And "Evet" does not count for the first moments in any case: `power::yes_counts`.)
                let counts = self.yes_counts();
                let yes_on = self.confirm_yes;
                let field = pal.field_fill().hsla();
                let answer = |id: &'static str, label: &'static str, yes: bool, hot: bool, faint: bool, cx: &mut Context<Self>| {
                    div()
                        .id(id)
                        .flex_1()
                        .h(px(34.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(theme::RADIUS_SM)
                        .bg(if hot { pressed } else { field })
                        .when(faint, |d| d.opacity(0.45))
                        .when(!faint, |d| d.cursor_pointer().hover(move |s| s.bg(pressed)))
                        .child(label)
                        .on_click(cx.listener(move |this, _, _, cx| this.answer_power(yes, cx)))
                };
                let no_button = answer("power-no", t!("Hayır"), false, !yes_on, false, cx);
                let yes_button = answer("power-yes", t!("Evet"), true, yes_on, !counts, cx);
                out.push(
                    div()
                        .id("start-power-confirm")
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .on_mouse_down(MouseButton::Left, dismiss(cx))
                        .on_mouse_down(MouseButton::Right, dismiss(cx))
                        .child(
                            v_flex()
                                .id("start-power-menu")
                                .w(px(MENU_W + 24.0))
                                .p(px(14.))
                                .gap_2()
                                .rounded(theme::RADIUS)
                                .bg(menu_fill)
                                .border_1()
                                .border_color(border)
                                .shadow_lg()
                                .occlude()
                                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(power_label(action)))
                                .child(div().text_sm().child(t!("Emin misiniz?")))
                                .child(h_flex().gap_2().mt(px(4.)).child(no_button).child(yes_button)),
                        )
                        .into_any_element(),
                );
            }
            PowerMenu::Open => {
                out.push(backdrop(cx).into_any_element());
                let mut col = v_flex()
                    .id("start-power-menu")
                    .absolute()
                    .right(px(PAD))
                    .bottom(px(BOTTOM_H + 6.0))
                    .w(px(MENU_W))
                    .p(px(6.))
                    .rounded(theme::RADIUS)
                    .bg(menu_fill)
                    .border_1()
                    .border_color(border)
                    .shadow_lg()
                    .occlude();
                for (i, action) in PowerAction::ALL.into_iter().enumerate() {
                    col = col.child(item(("p", i), power_icon(action), power_label(action), i == self.power_hi).on_click(cx.listener(move |this, _, _, cx| this.choose_power(action, cx))));
                }
                out.push(col.into_any_element());
            }
        }
        out
    }
}

/// A mouse press outside a menu or the question: it goes away (and the click is not passed on to what lies under it).
fn dismiss_listener(cx: &mut Context<StartView>) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static {
    cx.listener(|this: &mut StartView, _: &MouseDownEvent, _, cx| {
        this.app_menu = None;
        this.quiet();
        this.set_power(PowerMenu::Closed, cx);
        cx.stop_propagation();
        cx.notify();
    })
}

fn backdrop_of(cx: &mut Context<StartView>) -> Stateful<Div> {
    div().id("start-overlay-backdrop").absolute().inset_0().on_mouse_down(MouseButton::Left, dismiss_listener(cx)).on_mouse_down(MouseButton::Right, dismiss_listener(cx))
}

fn power_label(a: PowerAction) -> &'static str {
    match a {
        PowerAction::Lock => t!("Kilitle"),
        PowerAction::Sleep => t!("Uyku"),
        PowerAction::SignOut => t!("Oturumu kapat"),
        PowerAction::Restart => t!("Yeniden başlat"),
        PowerAction::Shutdown => t!("Bilgisayarı kapat"),
    }
}

fn power_icon(a: PowerAction) -> L {
    match a {
        PowerAction::Lock => L::Lock,
        PowerAction::Sleep => L::Moon,
        PowerAction::SignOut => L::LogOut,
        PowerAction::Restart => L::RotateCw,
        PowerAction::Shutdown => L::Power,
    }
}

#[cfg(test)]
mod tests {
    // (not `super::*`: gpui's own `test` attribute would come with it)
    use super::{AppMenu, AppMenuItem, CELL_W, PAD, PANEL_H, PANEL_W, PowerAction, QUIET, acts, pins_path, power_icon, power_label, startpins};
    use std::time::{Duration, Instant};

    fn menu(location: Option<&str>) -> AppMenu {
        AppMenu { app: rf_win::apps::App { name: "Paint".into(), id: "p!App".into() }, at: gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)), location: location.map(Into::into) }
    }

    #[test]
    fn the_app_menu_offers_the_folder_only_when_the_shell_found_a_program() {
        assert_eq!(menu(None).items(), [AppMenuItem::Pin, AppMenuItem::Admin]);
        assert_eq!(menu(Some(r"C:\Windows\mspaint.exe")).items(), [AppMenuItem::Pin, AppMenuItem::Admin, AppMenuItem::Location]);
    }

    #[test]
    fn a_panel_on_its_way_out_or_just_after_a_menu_click_does_nothing_on_a_second_click() {
        let now = Instant::now();
        // an ordinary click
        assert!(acts(true, false, now, now));
        // the panel is hidden, or a choice was made and it is closing: a double click starts nothing twice
        assert!(!acts(false, false, now, now));
        assert!(!acts(true, true, now, now));
        // a menu item was clicked a moment ago: the second click of the double click is not a click on what lies under the menu
        let quiet = now + QUIET;
        assert!(!acts(true, false, now, quiet));
        assert!(!acts(true, false, now + QUIET - Duration::from_millis(1), quiet));
        assert!(acts(true, false, quiet, quiet));
    }

    #[test]
    fn every_power_item_has_its_own_label_and_picture() {
        let labels: std::collections::HashSet<_> = PowerAction::ALL.iter().map(|a| power_label(*a)).collect();
        assert_eq!(labels.len(), PowerAction::ALL.len());
        let icons: std::collections::HashSet<_> = PowerAction::ALL.iter().map(|a| format!("{:?}", power_icon(*a))).collect();
        assert_eq!(icons.len(), PowerAction::ALL.len());
    }

    #[test]
    fn the_pin_file_lives_in_the_config_folder() {
        let p = pins_path().unwrap();
        assert_eq!(p.file_name().unwrap(), startpins::FILE);
    }

    #[test]
    fn the_panels_size_is_the_design_size() {
        assert_eq!((PANEL_W, PANEL_H), (620.0, 680.0));
        // the grid is six cells wide inside the padding
        assert_eq!(CELL_W * 6.0 + 2.0 * PAD, PANEL_W - 0.0);
    }
}
