//! Status Center: in-app progress for copy/move/delete with cancel (Files' "StatusCenter").
//! Work runs on its own thread; progress is throttled before it reaches the UI.

use crate::t;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures::StreamExt;
use futures::channel::mpsc;
use gpui_kit::assets::IconName as L;
use gpui_kit::component::{Icon, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use rf_core::transfer::{self as xfer, OpStats, Outcome, Pace, Stage};
use rf_win::fileops::{self, ByteProgress, Tracker, Transfer};

use crate::theme::{self, Palette};

#[derive(Clone, PartialEq)]
pub enum OpPhase {
    Running,
    Done,
    Cancelled,
    Failed(String),
}

/// A button's label and what it does.
pub type ResultAction = (String, std::rc::Rc<dyn Fn(&mut App)>);

pub struct OpState {
    id: u64,
    title: String,
    total: u32,
    done: u32,
    current: Option<String>,
    phase: OpPhase,
    cancel: Arc<AtomicBool>,
    /// Our copy engine waits while set (the shell engine ignores it).
    paused: Arc<AtomicBool>,
    started: Instant,
    /// A button on the finished operation (the folder organizer's "Geri al"), see [`set_result`].
    action: Option<ResultAction>,
    /// When it ended (for how long its card stays).
    ended: Option<Instant>,
    /// Speed, time left and the pace chart, fed from the progress reports.
    stats: OpStats,
    /// Bytes done and in all, when the engine counts them (shared with the worker's tracker).
    bytes: Arc<ByteProgress>,
    /// What the status bar's strip says while it runs ("12 öğe kopyalanıyor: Belgeler"); the title when `None`.
    words: Option<String>,
    /// The user sent its card / strip away (a failure stays up until then).
    dismissed: bool,
    /// Its card is folded to a one-line pill.
    collapsed: bool,
    /// Names that already exist, waiting for an answer.
    question: Option<Question>,
    /// A folder the finished card offers to open.
    open_dest: Option<std::path::PathBuf>,
    /// What the undo stack's top must hold for this operation to be undone from its card: a path it made.
    undo_mark: Option<Arc<std::sync::Mutex<Option<std::path::PathBuf>>>>,
}

/// A question from the copy engine, kept here so every window's strip or card can ask it, not
/// only the dialog in the window the copy began in. Whoever answers first takes the reply.
#[derive(Clone)]
pub struct Question {
    count: usize,
    /// What the dialog lists, kept for "Seç…" on the strip or a card (the dialog is not opened until then).
    list: Vec<rf_win::fastcopy::Conflict>,
    reply: std::rc::Rc<std::cell::RefCell<Option<crate::conflict_dialog::Reply>>>,
}

impl Question {
    /// Still waiting (the dialog may have answered it already).
    fn pending(&self) -> bool {
        self.reply.borrow().is_some()
    }
}

impl OpState {
    fn new(id: u64, title: String, cancel: Arc<AtomicBool>, paused: Arc<AtomicBool>, bytes: Arc<ByteProgress>) -> Self {
        Self {
            id,
            title,
            total: 0,
            done: 0,
            current: None,
            phase: OpPhase::Running,
            cancel,
            paused,
            started: Instant::now(),
            action: None,
            ended: None,
            stats: OpStats::default(),
            bytes,
            words: None,
            dismissed: false,
            collapsed: false,
            question: None,
            open_dest: None,
            undo_mark: None,
        }
    }

    fn secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

pub struct Ops {
    ops: Vec<OpState>,
    next: u64,
    pub open: bool,
    /// A failure the user has not looked at yet (the button turns red until opened).
    unseen_failure: bool,
    /// The window whose button opened the flyout (it is anchored there).
    owner: Option<AnyWindowHandle>,
    /// The Acrylic popup could not be opened: draw the flyout inside the window instead.
    in_window: bool,
    /// When the popup closed itself (focus left it): a click on the button right after must
    /// not reopen it.
    auto_closed: Option<Instant>,
    /// Since when something has been running without a break (None = nothing runs).
    busy_since: Option<Instant>,
    /// The button's attention pulse: when it began, and which one it is (an animation restarts only under a new id).
    pulse: Option<(Instant, u64)>,
    pulse_count: u64,
    /// When the last of the running operations ended after a run long enough to be noticed (the ring closes and fades from here).
    all_done: Option<Instant>,
    /// Which pulse that was (the ring's fade is an animation under this id).
    done_epoch: u64,
    /// A once-a-second wake-up task is running (see [`keep_ticking`]).
    ticking: bool,
}

/// What [`Ops::glance`] says.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Glance {
    /// The newest running operation: its title and how far it is in percent (`None` before the first progress report).
    pub running: Option<(String, Option<u32>)>,
    /// The newest failed operation: its id (the same one is not news twice) and the reason.
    pub failed: Option<(u64, String)>,
}

fn glance_of(ops: &[OpState]) -> Glance {
    let running = ops.iter().rev().find(|o| o.phase == OpPhase::Running).map(|o| (o.title.clone(), (o.total > 0).then(|| (u64::from(o.done) * 100 / u64::from(o.total)).min(100) as u32)));
    let failed = ops.iter().rev().find_map(|o| match &o.phase {
        OpPhase::Failed(why) => Some((o.id, why.clone())),
        _ => None,
    });
    Glance { running, failed }
}

/// Finished operations kept in the list (Files keeps them until "Clear").
const HISTORY: usize = 30;

pub struct OpsGlobal(pub Entity<Ops>);
impl Global for OpsGlobal {}

enum Ev {
    Progress(u32, u32, Option<String>),
    /// The copy thread asks about names that already exist and waits for the reply.
    Ask(Vec<rf_win::fastcopy::Conflict>, crate::conflict_dialog::Reply),
    Done(Result<bool, String>),
}

impl Ops {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self::blank()
    }

    fn blank() -> Self {
        Self { ops: Vec::new(), next: 1, open: false, unseen_failure: false, owner: None, in_window: false, auto_closed: None, busy_since: None, pulse: None, pulse_count: 0, all_done: None, done_epoch: 0, ticking: false }
    }

    pub fn toggle(&mut self, owner: AnyWindowHandle, cx: &mut Context<Self>) {
        if !self.open && self.auto_closed.is_some_and(|t| t.elapsed() < Duration::from_millis(350)) {
            // this click is what closed it (the popup lost focus to the button)
            self.auto_closed = None;
            return;
        }
        self.owner = Some(owner);
        self.open = !self.open;
        if self.open {
            self.unseen_failure = false;
            // the failures are in front of the user now: their strip / card is done
            for o in &mut self.ops {
                if matches!(o.phase, OpPhase::Failed(_)) {
                    o.dismissed = true;
                }
            }
        }
        cx.notify();
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.open = false;
            cx.notify();
        }
    }

    fn clear_finished(&mut self, cx: &mut Context<Self>) {
        self.ops.retain(|o| o.phase == OpPhase::Running);
        self.unseen_failure = false;
        cx.notify();
    }

    /// Title, outcome and duration of an operation (for the "finished" notification).
    pub fn summary(&self, id: u64) -> Option<(String, OpPhase, Duration)> {
        self.ops.iter().find(|o| o.id == id).map(|o| (o.title.clone(), o.phase.clone(), o.started.elapsed()))
    }

    pub fn running(&self) -> usize {
        self.ops.iter().filter(|o| o.phase == OpPhase::Running).count()
    }

    /// For a window that has no Status Center button of its own (the desktop): the newest running operation and the newest failure.
    pub fn glance(&self) -> Glance {
        glance_of(&self.ops)
    }

    /// What the taskbar buttons show: the running operations' progress, yellow when all are
    /// paused, red while a failure is unseen.
    fn taskbar(&self) -> rf_win::taskbar_progress::State {
        use rf_win::taskbar_progress::State;
        let run: Vec<&OpState> = self.ops.iter().filter(|o| o.phase == OpPhase::Running).collect();
        let Some(v) = self.overall() else { return State::None };
        if run.iter().all(|o| o.total == 0) {
            State::Busy
        } else if self.unseen_failure {
            State::Error(v)
        } else if run.iter().all(|o| o.paused.load(Ordering::Relaxed)) {
            State::Paused(v)
        } else {
            State::Normal(v)
        }
    }

    fn overall(&self) -> Option<f32> {
        let run: Vec<&OpState> = self.ops.iter().filter(|o| o.phase == OpPhase::Running).collect();
        if run.is_empty() {
            return None;
        }
        let f: f32 = run.iter().map(|o| if o.total > 0 { o.done as f32 / o.total as f32 } else { 0.0 }).sum();
        Some(f / run.len() as f32)
    }

    /// An operation was added: the attention pulse, and a new run begins when nothing ran before.
    /// True when the caller must start the once-a-second wake-up (none runs yet).
    fn began(&mut self) -> bool {
        self.busy_since.get_or_insert_with(Instant::now);
        self.all_done = None;
        self.pulse_now();
        !std::mem::replace(&mut self.ticking, true)
    }

    /// Something runs that is not waiting: its speed and time left change by the clock, not only
    /// by progress reports (a disk that stops answering sends none).
    fn wants_tick(&self) -> bool {
        self.ops.iter().any(|o| o.phase == OpPhase::Running && !o.paused.load(Ordering::Relaxed))
    }

    /// An operation ended. When it was the last one of a run long enough to be noticed, the ring
    /// closes and fades and the button pulses once; a run that was over before anyone could see
    /// it leaves nothing behind.
    fn ended_one(&mut self) {
        if self.running() > 0 {
            return;
        }
        let long = self.busy_since.take().is_some_and(|t| t.elapsed() >= Duration::from_millis(xfer::CARD_APPEAR_MS));
        if long {
            self.all_done = Some(Instant::now());
            self.pulse_now();
            self.done_epoch = self.pulse_count;
        }
    }

    fn pulse_now(&mut self) {
        self.pulse_count += 1;
        self.pulse = Some((Instant::now(), self.pulse_count));
    }

    /// What the button's ring and pulse need: (something runs, the overall progress when any
    /// total is known, how long since the last run ended, the pulse's id and age in ms).
    fn ring_state(&self) -> RingState {
        let running = self.running() > 0;
        let known = self.ops.iter().any(|o| o.phase == OpPhase::Running && o.total > 0);
        RingState {
            running,
            fraction: if known { self.overall() } else { None },
            since_end_ms: if running { None } else { self.all_done.map(|t| t.elapsed().as_millis() as u64) },
            end_epoch: self.done_epoch,
            pulse: self.pulse.map(|(t, n)| (n, t.elapsed().as_millis() as u64)).filter(|&(_, ms)| ms < xfer::PULSE_MS),
        }
    }

    /// The operations as the strip and the cards see them, oldest first: the running ones, the
    /// ones that ended lately and the failures nobody has looked at.
    pub(crate) fn views(&self) -> Vec<OpView> {
        self.ops
            .iter()
            .filter(|o| match &o.phase {
                OpPhase::Running => true,
                OpPhase::Failed(_) => !o.dismissed,
                _ => o.ended.is_some_and(|e| e.elapsed() < Duration::from_secs(10)),
            })
            .map(OpState::view)
            .collect()
    }

    // The state changes below take no window or context so they can be tested; the methods after
    // them are the thin callers the buttons use.

    fn pause_toggled(&mut self, id: u64) {
        if let Some(op) = self.ops.iter_mut().find(|o| o.id == id) {
            let now_paused = !op.paused.load(Ordering::Relaxed);
            op.paused.store(now_paused, Ordering::Relaxed);
            if now_paused {
                op.stats.pause();
            } else {
                let t = op.secs();
                op.stats.resume(t);
            }
        }
    }

    /// Stops operation `id`. Gives back the reply of the question it was waiting on, if any
    /// (answered "cancel" here), so the dialog showing it can be taken away.
    fn cancelled(&mut self, id: u64) -> Option<std::rc::Rc<std::cell::RefCell<Option<crate::conflict_dialog::Reply>>>> {
        let op = self.ops.iter_mut().find(|o| o.id == id)?;
        op.cancel.store(true, Ordering::Relaxed);
        let q = op.question.take()?;
        let reply = q.reply.borrow_mut().take()?;
        let _ = reply.send(None);
        Some(q.reply)
    }

    fn dismissed(&mut self, id: u64) {
        if let Some(op) = self.ops.iter_mut().find(|o| o.id == id) {
            op.dismissed = true;
        }
        if !self.ops.iter().any(|o| matches!(o.phase, OpPhase::Failed(_)) && !o.dismissed) {
            self.unseen_failure = false;
        }
    }

    fn collapsed_toggled(&mut self, id: u64) {
        if let Some(op) = self.ops.iter_mut().find(|o| o.id == id) {
            op.collapsed = !op.collapsed;
        }
    }

    /// Answers operation `id`'s question for every name. Gives back the reply it took (the
    /// dialog showing it is taken away), or `None` when there was nothing to answer: no such
    /// operation, no question, or somebody answered first.
    fn answered(&mut self, id: u64, r: rf_win::fastcopy::Resolve) -> Option<std::rc::Rc<std::cell::RefCell<Option<crate::conflict_dialog::Reply>>>> {
        let q = self.ops.iter_mut().find(|o| o.id == id)?.question.take()?;
        let reply = q.reply.borrow_mut().take()?;
        let _ = reply.send(Some(vec![r; q.count]));
        Some(q.reply)
    }

    pub(crate) fn toggle_pause(&mut self, id: u64, cx: &mut Context<Self>) {
        self.pause_toggled(id);
        cx.notify();
    }

    pub(crate) fn cancel_op(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(reply) = self.cancelled(id) {
            // (not now: the click that got us here may be in the window holding the dialog)
            cx.defer(move |cx| crate::conflict_dialog::dismiss_if(&reply, cx));
        }
        cx.notify();
    }

    /// Sends a card / strip away (a failure stays until this).
    pub(crate) fn dismiss(&mut self, id: u64, cx: &mut Context<Self>) {
        self.dismissed(id);
        cx.notify();
    }

    pub(crate) fn toggle_collapsed(&mut self, id: u64, cx: &mut Context<Self>) {
        self.collapsed_toggled(id);
        cx.notify();
    }

    /// The same answer for every name the copy asked about, from the strip or a card.
    pub(crate) fn answer(&mut self, id: u64, r: rf_win::fastcopy::Resolve, cx: &mut Context<Self>) {
        if let Some(reply) = self.answered(id, r) {
            // the dialog (if "Seç…" opened one) is answered by this: take it away
            cx.defer(move |cx| crate::conflict_dialog::dismiss_if(&reply, cx));
        }
        cx.notify();
    }

    /// "Seç…" on the strip or a card: the per-name dialog, in the window the click was in.
    pub(crate) fn choose_each(&mut self, id: u64, window: AnyWindowHandle, cx: &mut Context<Self>) {
        let Some(q) = self.ops.iter().find(|o| o.id == id).and_then(|o| o.question.clone()).filter(Question::pending) else { return };
        // (not now: the click is inside that window's update)
        cx.defer(move |cx| crate::conflict_dialog::show_shared(q.list, q.reply, Some(window), cx));
    }

    /// Opens the Status Center from a strip or card in `owner`.
    pub(crate) fn show_center(&mut self, owner: AnyWindowHandle, cx: &mut Context<Self>) {
        if !self.open {
            self.toggle(owner, cx);
        }
    }
}

/// What the Status Center button's ring and pulse are drawn from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RingState {
    pub running: bool,
    pub fraction: Option<f32>,
    pub since_end_ms: Option<u64>,
    /// The id of the run that ended (its fade is an animation under it).
    pub end_epoch: u64,
    /// The pulse's id and how many milliseconds old it is.
    pub pulse: Option<(u64, u64)>,
}

/// An operation as the views draw it: copied out of [`Ops`] so they hold nothing of it.
pub(crate) struct OpView {
    pub id: u64,
    /// The strip's sentence ("12 öğe kopyalanıyor: Belgeler"), the title when there is no better one.
    pub words: String,
    pub title: String,
    pub current: Option<String>,
    pub phase: OpPhase,
    /// `None` while the total is unknown.
    pub frac: Option<f32>,
    /// Bytes done and in all, when the engine counts them.
    pub bytes: Option<(u64, u64)>,
    pub pace: Pace,
    /// The last minute's speed, 0 ..= 1 a bar.
    pub bars: Vec<f32>,
    pub paused: bool,
    /// Names waiting for an answer: how many.
    pub question: Option<usize>,
    pub stage: Stage,
    pub collapsed: bool,
    pub open_dest: Option<std::path::PathBuf>,
    pub can_undo: bool,
    pub action: Option<ResultAction>,
    pub brief: xfer::Brief,
}

impl OpState {
    fn view(&self) -> OpView {
        let running = self.phase == OpPhase::Running;
        let paused = running && self.paused.load(Ordering::Relaxed);
        let b = self.bytes.get();
        let outcome = match &self.phase {
            OpPhase::Running => None,
            OpPhase::Done => Some(Outcome::Done),
            OpPhase::Cancelled => Some(Outcome::Cancelled),
            OpPhase::Failed(_) => Some(Outcome::Failed),
        };
        let ran_ms = self.ended.unwrap_or_else(Instant::now).duration_since(self.started).as_millis() as u64;
        let since_end_ms = self.ended.map_or(0, |e| e.elapsed().as_millis() as u64);
        let question = self.question.as_ref().filter(|q| q.pending()).map(|q| q.count);
        let stage = xfer::card_stage(&xfer::CardIn { outcome, ran_ms, since_end_ms, asking: question.is_some(), dismissed: self.dismissed });
        let pace = if running && !paused { self.stats.pace(self.secs(), (self.done, self.total), b) } else { Pace::default() };
        let can_undo = self.undo_mark.as_ref().and_then(|m| m.lock().ok().and_then(|g| g.clone())).is_some_and(|p| crate::undo::last_made(&p));
        OpView {
            id: self.id,
            words: self.words.clone().unwrap_or_else(|| self.title.clone()),
            title: self.title.clone(),
            current: self.current.clone(),
            phase: self.phase.clone(),
            frac: (self.total > 0).then(|| (self.done as f32 / self.total as f32).clamp(0.0, 1.0)),
            bytes: (b.1 > 0).then_some(b),
            pace,
            bars: if stage == Stage::Running { self.stats.bars(self.secs()) } else { Vec::new() },
            paused,
            question,
            stage,
            collapsed: self.collapsed,
            open_dest: self.open_dest.clone(),
            can_undo,
            action: self.action.clone(),
            brief: xfer::Brief { running, noticed: ran_ms >= xfer::CARD_APPEAR_MS, asking: question.is_some(), failed: matches!(self.phase, OpPhase::Failed(_)), dismissed: self.dismissed },
        }
    }
}

/// Wakes the windows `ms` from now (a card appears or goes by the clock, not by an event).
fn notify_in(cx: &mut App, ops: &Entity<Ops>, ms: u64) {
    let ops = ops.clone();
    cx.spawn(async move |cx| {
        cx.background_executor().timer(Duration::from_millis(ms)).await;
        ops.update(cx, |_, cx| cx.notify());
    })
    .detach();
}

/// How often the windows are woken while something runs.
const TICK_MS: u64 = 1000;

/// While something runs, wakes the windows once a second. The speed and the time left are worked
/// out from the clock each time a window draws, and a window draws on progress reports: when a
/// disk or a share stops answering no report comes, and without this the last "45 MB/sn · ~12 sn"
/// would stay on screen. Stops by itself once nothing runs (nothing wakes anything when idle).
fn keep_ticking(cx: &mut App, ops: &Entity<Ops>) {
    let ops = ops.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(Duration::from_millis(TICK_MS)).await;
            let go = ops.update(cx, |o, cx| {
                if o.running() == 0 {
                    o.ticking = false;
                    return false;
                }
                if o.wants_tick() {
                    cx.notify();
                }
                true
            });
            if !go {
                break;
            }
        }
    })
    .detach();
}

/// Starts a tracked operation. `work` runs on a new thread with COM initialized.
pub fn start(
    cx: &mut App,
    title: String,
    work: impl FnOnce(Arc<Tracker>) -> anyhow::Result<bool> + Send + 'static,
) {
    start_with(cx, title, work, None)
}

/// Called on the UI thread when the operation ends; `true` = it succeeded.
pub type OnDone = Box<dyn FnOnce(bool, &mut App)>;

/// Like [`start`], with a callback when the work is over (e.g. refresh a view after `git pull`).
pub fn start_with(
    cx: &mut App,
    title: String,
    work: impl FnOnce(Arc<Tracker>) -> anyhow::Result<bool> + Send + 'static,
    on_done: Option<OnDone>,
) {
    start_with_id(cx, title, work, on_done);
}

/// [`start_with`] giving the operation's id (0 when there is no Status Center), for
/// [`set_result`].
pub fn start_with_id(
    cx: &mut App,
    title: String,
    work: impl FnOnce(Arc<Tracker>) -> anyhow::Result<bool> + Send + 'static,
    on_done: Option<OnDone>,
) -> u64 {
    let Some(entity) = cx.try_global::<OpsGlobal>().map(|g| g.0.clone()) else { return 0 };
    let cancel = Arc::new(AtomicBool::new(false));
    let paused = Arc::new(AtomicBool::new(false));
    let bytes = Arc::new(ByteProgress::default());
    // questions about the operation go to the window it was started from
    let origin = cx.active_window();
    let (tx, mut rx) = mpsc::unbounded::<Ev>();
    let (id, tick) = entity.update(cx, |ops, cx| {
        let id = ops.next;
        ops.next += 1;
        ops.ops.push(OpState::new(id, title, cancel.clone(), paused.clone(), bytes.clone()));
        let tick = ops.began();
        // ring pictures an earlier colour left behind are given back to the toolkit now and then (see `transfer_view::release_retired`)
        crate::transfer_view::release_retired(cx);
        cx.notify();
        (id, tick)
    });
    if tick {
        keep_ticking(cx, &entity);
    }
    // the card is due once the operation has run a moment, the attention pulse is over a moment after it began
    notify_in(cx, &entity, xfer::CARD_APPEAR_MS + 50);
    notify_in(cx, &entity, xfer::PULSE_MS + 50);

    let ptx = tx.clone();
    let asking_cancel = cancel.clone();
    let last = std::sync::Mutex::new(Instant::now() - Duration::from_secs(1));
    let mut tracker = Tracker::new(
        Box::new(move |t, d, cur| {
            // ~20 updates/s max; item names always go through.
            let mut l = last.lock().unwrap();
            if cur.is_some() || l.elapsed() >= Duration::from_millis(50) || d >= t {
                *l = Instant::now();
                let _ = ptx.unbounded_send(Ev::Progress(t, d, cur));
            }
        }),
        cancel,
    );
    tracker.paused = paused;
    tracker.bytes = bytes;
    let atx = tx.clone();
    tracker.ask = Some(Box::new(move |list| {
        let (rtx, rrx) = std::sync::mpsc::channel();
        atx.unbounded_send(Ev::Ask(list.to_vec(), rtx)).ok()?;
        // no reply (the dialog went away) or the operation cancelled meanwhile = cancel
        loop {
            match rrx.recv_timeout(Duration::from_millis(200)) {
                Ok(answer) => return answer,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) if !asking_cancel.load(Ordering::Relaxed) => {}
                Err(_) => return None,
            }
        }
    }));
    let tracker = Arc::new(tracker);
    std::thread::Builder::new()
        .name("rf-op".into())
        .spawn(move || {
            rf_win::shell::ensure_com();
            // (a failure the shell's absence causes gets a sentence that says so: shellless.rs)
            let r = work(tracker).map_err(|e| crate::shellless::op_failure_text(&e));
            let _ = tx.unbounded_send(Ev::Done(r));
        })
        .ok();

    cx.spawn(async move |cx| {
        let mut ended = false;
        let mut ok = false;
        while let Some(first) = rx.next().await {
            let mut evs = vec![first];
            while let Ok(e) = rx.try_recv() {
                evs.push(e);
            }
            let mut finished = false;
            let mut asks = Vec::new();
            let _ = entity.update(cx, |ops, cx| {
                if let Some(op) = ops.ops.iter_mut().find(|o| o.id == id) {
                    for e in evs {
                        match e {
                            Ev::Ask(list, reply) => {
                                op.current = Some(t!("Hedefte aynı adlı öğeler var").to_string());
                                // shared with the dialog: the strip or card of any window can answer too, the first answer wins
                                let reply = std::rc::Rc::new(std::cell::RefCell::new(Some(reply)));
                                op.question = Some(Question { count: list.len(), list: list.clone(), reply: reply.clone() });
                                asks.push((list, reply));
                            }
                            Ev::Progress(t, d, cur) => {
                                if t > 0 {
                                    op.total = t;
                                    op.done = d;
                                    op.stats.update(op.started.elapsed().as_secs_f64(), (d, t), op.bytes.get());
                                }
                                if cur.is_some() {
                                    op.current = cur;
                                }
                            }
                            Ev::Done(r) => {
                                finished = true;
                                op.ended = Some(Instant::now());
                                op.question = None;
                                ok = matches!(r, Ok(true));
                                op.phase = match r {
                                    Ok(true) => OpPhase::Done,
                                    Ok(false) => OpPhase::Cancelled,
                                    Err(e) => OpPhase::Failed(e),
                                };
                                op.done = op.total;
                            }
                        }
                    }
                }
                if finished {
                    ops.ended_one();
                    crate::transfer_view::release_retired(cx);
                    if ops.ops.iter().any(|o| o.id == id && matches!(o.phase, OpPhase::Failed(_))) && !ops.open {
                        ops.unseen_failure = true;
                    }
                    // a failure that ends while the Status Center is open is in front of the user already: no strip or card for it
                    if ops.open
                        && let Some(op) = ops.ops.iter_mut().find(|o| o.id == id && matches!(o.phase, OpPhase::Failed(_)))
                    {
                        op.dismissed = true;
                    }
                    // keep a bounded history of finished operations (oldest finished go first)
                    while ops.ops.len() > HISTORY {
                        let Some(ix) = ops.ops.iter().position(|o| o.phase != OpPhase::Running) else { break };
                        ops.ops.remove(ix);
                    }
                }
                cx.notify();
            });
            for (list, reply) in asks {
                cx.update(|cx| {
                    let mode = cx.global::<crate::settings::Settings>().transfer_progress;
                    if crate::transfer_view::question_in_dialog(mode, !cx.windows().is_empty()) {
                        crate::conflict_dialog::show_shared(list, reply, origin, cx);
                    } else if let Some(w) = origin.or_else(|| cx.active_window()).or_else(|| cx.windows().first().copied()) {
                        // the question is on the strip or the card (every window's); the app comes forward as the dialog would have brought it
                        let _ = w.update(cx, |_, window, _| window.activate_window());
                    }
                });
            }
            if finished {
                ended = true;
                // the result card goes away by itself, and the ring and the pulse after the last one: wake the windows up then
                cx.update(|cx| {
                    notify_in(cx, &entity, xfer::CARD_CANCELLED_MS + 100);
                    notify_in(cx, &entity, xfer::CARD_DONE_MS + 100);
                    notify_in(cx, &entity, xfer::RING_FADE_MS + 100);
                });
                break;
            }
        }
        if !ended {
            // worker died without reporting (panic / spawn failure): don't spin forever
            let _ = entity.update(cx, |ops, cx| {
                if let Some(op) = ops.ops.iter_mut().find(|o| o.id == id) {
                    op.phase = OpPhase::Failed(t!("İşlem beklenmedik şekilde sonlandı").into());
                    op.ended = Some(Instant::now());
                    op.question = None;
                }
                ops.ended_one();
                ops.unseen_failure |= !ops.open;
                cx.notify();
            });
        }
        let _ = cx.update(|cx| crate::background::op_finished(&entity, id, cx));
        if let Some(done) = on_done {
            let _ = cx.update(|cx| done(ok, cx));
        }
    })
    .detach();
    id
}

/// Gives a finished operation its final words and, optionally, one button (the folder
/// organizer: "12 dosya düzenlendi" and "Geri al"). `title: None` keeps the title.
pub fn set_result(cx: &mut App, id: u64, title: Option<String>, action: Option<ResultAction>) {
    let Some(entity) = cx.try_global::<OpsGlobal>().map(|g| g.0.clone()) else { return };
    entity.update(cx, |ops, cx| {
        if let Some(op) = ops.ops.iter_mut().find(|o| o.id == id) {
            if let Some(t) = title {
                op.title = t;
            }
            op.action = action;
        }
        cx.notify();
    });
}

/// Takes a finished operation off the list (an automatic run that found nothing to do should
/// not leave a row behind).
pub fn remove(cx: &mut App, id: u64) {
    let Some(entity) = cx.try_global::<OpsGlobal>().map(|g| g.0.clone()) else { return };
    entity.update(cx, |ops, cx| {
        ops.ops.retain(|o| o.id != id || o.phase == OpPhase::Running);
        cx.notify();
    });
}

fn short(p: &Path) -> String {
    rf_core::history::display_name(p)
}

/// Copy/move with progress in the Status Center.
pub fn transfer(cx: &mut App, kind: Transfer, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf) {
    transfer_ex(cx, kind, sources, dest, true)
}

/// Like [`transfer`], then `after` on the UI thread with the items that arrived at the top of
/// `dest` (new copies, merged folders), e.g. to select them.
pub fn transfer_then(cx: &mut App, kind: Transfer, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, after: impl FnOnce(Vec<std::path::PathBuf>, &mut App) + 'static) {
    transfer_inner(cx, kind, sources, dest, true, Some(Box::new(after)));
}

/// `record = false` for undo/redo replays (they must not land on the undo stack again).
pub fn transfer_ex(cx: &mut App, kind: Transfer, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, record: bool) {
    transfer_inner(cx, kind, sources, dest, record, None);
}

#[allow(clippy::type_complexity)]
fn transfer_inner(
    cx: &mut App,
    kind: Transfer,
    sources: Vec<std::path::PathBuf>,
    dest: std::path::PathBuf,
    record: bool,
    after: Option<Box<dyn FnOnce(Vec<std::path::PathBuf>, &mut App)>>,
) {
    // into a folder inside an archive: add to the archive (copy semantics; RAR/ISO refuse)
    if let Some((archive, inner)) = crate::archive_view::split(&dest) {
        crate::archive_view::add_into(cx, archive, inner, sources);
        return;
    }
    let verb = match kind {
        Transfer::Copy => t!("Kopyalanıyor"),
        Transfer::Move => t!("Taşınıyor"),
    };
    let title = format!("{verb}: {} → {}", crate::i18n::items(sources.len()), short(&dest));
    let s = cx.global::<crate::settings::Settings>();
    let (fast, verify, limit) = (s.fast_file_ops, s.copy_verify.engine(), u64::from(s.copy_speed_limit) << 20);
    let arrived = Arc::new(std::sync::Mutex::new(Vec::<std::path::PathBuf>::new()));
    let landed = arrived.clone();
    let top = dest.clone();
    let on_done: Option<OnDone> = after.map(|f| -> OnDone { Box::new(move |_, cx: &mut App| f(std::mem::take(&mut *arrived.lock().unwrap()), cx)) });
    // the strip's sentence, what the finished card opens, and the mark that tells its "Geri al" which undo record is its own
    let words = match kind {
        Transfer::Copy => t!("{items} kopyalanıyor: {dest}"),
        Transfer::Move => t!("{items} taşınıyor: {dest}"),
    }
    .replace("{items}", &crate::i18n::items(sources.len()))
    .replace("{dest}", &short(&dest));
    let open_dest = dest.clone();
    let mark = Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let marker = mark.clone();
    let id = start_with_id(cx, title, move |t| {
        t.set_verify(verify);
        t.limit.store(limit, Ordering::Relaxed);
        // our engine (conflicts asked in one list); Windows' engine when it is turned off
        let r = if fast {
            rf_win::fastcopy::transfer(kind, &sources, &dest, t.clone())
        } else {
            fileops::transfer_tracked(kind, &sources, &dest, t.clone())
        };
        let pairs = t.results.lock().unwrap().clone();
        // what now sits directly in the destination: the new item, or the folder a merge went into
        let mut tops: Vec<std::path::PathBuf> = Vec::new();
        for (_, dst) in &pairs {
            if let Some(first) = dst.strip_prefix(&top).ok().and_then(|rel| rel.components().next()) {
                let p = top.join(first);
                if !tops.contains(&p) {
                    tops.push(p);
                }
            }
        }
        *landed.lock().unwrap() = tops;
        if record && !pairs.is_empty() {
            *marker.lock().unwrap() = pairs.first().map(|(_, made)| made.clone());
            crate::undo::push(crate::undo::UndoOp::Transferred { kind, pairs });
        }
        r
    }, on_done);
    set_extras(cx, id, Some(words), Some(open_dest), record.then_some(mark));
}

/// Words for the strip, a folder for the finished card to open, and the mark that tells the
/// card's "Geri al" which undo record is this operation's own (see `undo::last_made`).
pub fn set_extras(cx: &mut App, id: u64, words: Option<String>, open_dest: Option<std::path::PathBuf>, undo_mark: Option<Arc<std::sync::Mutex<Option<std::path::PathBuf>>>>) {
    let Some(entity) = cx.try_global::<OpsGlobal>().map(|g| g.0.clone()) else { return };
    entity.update(cx, |ops, cx| {
        if let Some(op) = ops.ops.iter_mut().find(|o| o.id == id) {
            op.words = words;
            op.open_dest = open_dest;
            op.undo_mark = undo_mark;
        }
        cx.notify();
    });
}

/// A copy or move cut off by a crash or a shutdown: asked about once the main window is up
/// (Resume / Discard / Later), one job at a time.
pub fn offer_resume(cx: &mut App) {
    let found = cx.background_spawn(async {
        // let the window settle first; the scan itself is a directory listing
        std::thread::sleep(std::time::Duration::from_millis(700));
        rf_win::fastcopy::interrupted()
    });
    cx.spawn(async move |cx| {
        let list = found.await;
        let Some(job) = list.into_iter().next() else { return };
        cx.update(|cx| {
            let Some(w) = cx.active_window().or_else(|| cx.windows().first().copied()) else { return };
            let _ = w.update(cx, |_, window, cx| ask_resume(job, window, cx));
        });
    })
    .detach();
}

fn ask_resume(job: rf_win::fastcopy::Interrupted, window: &mut gpui_kit::Window, cx: &mut App) {
    use crate::prompt::{Button, Kind};
    let title = match job.kind {
        Transfer::Copy => t!("Yarıda kalan bir kopyalama var"),
        Transfer::Move => t!("Yarıda kalan bir taşıma var"),
    };
    let what = format!("{} → {}", crate::i18n::items(job.sources.len()), short(&job.dest));
    let msg = format!(
        "{what}\n{} / {} {}\n\n{}",
        job.done.min(job.files),
        job.files,
        t!("dosya tamamlandı"),
        t!("Kaldığı yerden devam edilsin mi? Tamamlanan dosyalar kaynaklarıyla yeniden karşılaştırılır, yarım kalanlar baştan kopyalanır.")
    );
    let (j1, j2) = (job.clone(), job.clone());
    crate::prompt::show(
        title,
        Some(msg),
        None,
        vec![
            Button::new(t!("Devam et"), Kind::Primary, move |_, _, cx| {
                resume_job(cx, j1.clone());
                offer_resume(cx);
                Ok(())
            }),
            Button::new(t!("Vazgeç"), Kind::Normal, move |_, _, cx| {
                // what was copied stays where it is
                rf_win::fastcopy::discard(&j2.journal);
                offer_resume(cx);
                Ok(())
            }),
            Button::new(t!("Sonra"), Kind::Normal, |_, _, _| Ok(())),
        ],
        window,
        cx,
    );
}

/// Carries a cut-off job on in the Status Center, with its original plan.
pub fn resume_job(cx: &mut App, job: rf_win::fastcopy::Interrupted) {
    let verb = match job.kind {
        Transfer::Copy => t!("Kopyalanıyor"),
        Transfer::Move => t!("Taşınıyor"),
    };
    let title = format!("{verb}: {} → {}", crate::i18n::items(job.sources.len()), short(&job.dest));
    let limit = u64::from(cx.global::<crate::settings::Settings>().copy_speed_limit) << 20;
    start(cx, title, move |t| {
        t.limit.store(limit, Ordering::Relaxed);
        rf_win::fastcopy::resume(&job.journal, t)
    });
}

pub fn archive_progress_pub(t: &Tracker) -> impl FnMut(rf_archive::Progress) + '_ {
    archive_progress(t)
}

/// Archive progress (bytes) → tracker units (permille, fits u32).
fn archive_progress(t: &Tracker) -> impl FnMut(rf_archive::Progress) + '_ {
    move |p: rf_archive::Progress| {
        let total = p.total_bytes.max(1);
        let done = (p.done_bytes.min(total) * 1000 / total) as u32;
        // the bytes too, for a speed in MB/s
        t.bytes.set(p.done_bytes, p.total_bytes);
        (t.on_progress)(1000, done, Some(p.current))
    }
}

/// Where an archive from the file list is extracted.
#[derive(Debug, Clone, PartialEq)]
pub enum ExtractTo {
    /// Its own folder, as is.
    Here,
    /// A new folder named after it, next to it.
    Named,
    /// Its own folder when everything in it is under one folder (or it is a single file),
    /// otherwise a new folder named after it.
    Smart,
    /// This folder, as is.
    Into(std::path::PathBuf),
    /// "Smart", inside this folder (several archives extracted to a chosen folder).
    SmartInto(std::path::PathBuf),
}

/// `dest = None`: "extract to <name>\\" (unique folder chosen on the worker thread).
/// A protected archive asks for its password first (probed off the UI thread).
pub fn extract(cx: &mut App, archive: std::path::PathBuf, dest: Option<std::path::PathBuf>) {
    let to = match dest {
        None => ExtractTo::Named,
        Some(d) if archive.parent() == Some(d.as_path()) => ExtractTo::Here,
        Some(d) => ExtractTo::Into(d),
    };
    extract_to(cx, vec![archive], to)
}

/// Extracts each archive as its own job. Protected ones ask for their password one after the
/// other (probed off the UI thread); the others start at once.
pub fn extract_to(cx: &mut App, archives: Vec<std::path::PathBuf>, to: ExtractTo) {
    let probe = cx.background_spawn({
        let list = archives.clone();
        async move { list.iter().map(|a| crate::archive_view::needs_password_now(a)).collect::<Vec<bool>>() }
    });
    cx.spawn(async move |cx| {
        let need = probe.await;
        cx.update(|cx| {
            let mut ask = Vec::new();
            for (a, need) in archives.into_iter().zip(need) {
                if need { ask.push(a) } else { extract_now(cx, a, to.clone()) }
            }
            ask.reverse();
            ask_then_extract(cx, ask, to);
        });
    })
    .detach();
}

/// Asks for a folder (Windows' folder dialog) and extracts the archives there: one archive as
/// is, several each the "smart" way (its own folder, unless it already is one folder).
pub fn extract_pick(cx: &mut App, archives: Vec<std::path::PathBuf>) {
    if archives.is_empty() {
        return;
    }
    let (tx, rx) = futures::channel::oneshot::channel::<Option<std::path::PathBuf>>();
    let title = t!("Çıkarılacak klasör").to_string();
    rf_win::sta::run(move |owner| {
        let _ = tx.send(rf_win::dialogs::pick(owner, &title, rf_win::dialogs::Pick::Folder));
    });
    cx.spawn(async move |cx| {
        let Ok(Some(dir)) = rx.await else { return };
        cx.update(|cx| {
            let to = if archives.len() == 1 { ExtractTo::Into(dir) } else { ExtractTo::SmartInto(dir) };
            extract_to(cx, archives, to);
        });
    })
    .detach();
}

/// Asks for the password of the last archive of `rest`, starts it, and goes on with the others.
fn ask_then_extract(cx: &mut App, mut rest: Vec<std::path::PathBuf>, to: ExtractTo) {
    let Some(archive) = rest.pop() else { return };
    if let Some(w) = cx.active_window() {
        let (a, t, r) = (archive.clone(), to.clone(), rest.clone());
        let asked = w
            .update(cx, |_, window, cx| {
                crate::archive_view::ask_password(archive.clone(), false, window, cx, move |cx| {
                    extract_now(cx, a.clone(), t.clone());
                    // (the prompt is still closing: the next one opens after it)
                    let (r, t) = (r.clone(), t.clone());
                    cx.defer(move |cx| ask_then_extract(cx, r, t));
                })
            })
            .is_ok();
        if asked {
            return;
        }
    }
    extract_now(cx, archive, to.clone());
    ask_then_extract(cx, rest, to);
}

fn extract_now(cx: &mut App, archive: std::path::PathBuf, to: ExtractTo) {
    let title = match &to {
        ExtractTo::Into(d) | ExtractTo::SmartInto(d) => format!("{} {} → {}", t!("Çıkarılıyor:"), short(&archive), short(d)),
        _ => format!("{} {}", t!("Çıkarılıyor:"), short(&archive)),
    };
    // the folder holding the result, when it is a new one: opened afterwards if the user wants
    let made = Arc::new(parking_lot::Mutex::new(None::<std::path::PathBuf>));
    let made2 = made.clone();
    let work = move |t: Arc<Tracker>| {
        let opts = crate::archive_view::options(&archive);
        let parent = archive.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let smart = |into: &std::path::Path| match rf_archive::list_with(&archive, &opts) {
            Ok(entries) => {
                let (d, r) = rf_archive::smart_dest(&archive, into, &entries);
                let r = (r != parent).then_some(r);
                (d, r)
            }
            // (a wrong password and the like: the extraction below reports it)
            Err(_) => {
                let d = rf_archive::named_dir(&archive, into);
                (d.clone(), Some(d))
            }
        };
        let (dest, result) = match &to {
            ExtractTo::Here => (parent.clone(), None),
            ExtractTo::Named => {
                let d = rf_archive::smart_extract_dir(&archive);
                (d.clone(), Some(d))
            }
            ExtractTo::Into(d) => (d.clone(), Some(d.clone())),
            ExtractTo::Smart => smart(&parent),
            ExtractTo::SmartInto(d) => smart(d),
        };
        let cancel = t.cancel.clone();
        let mut prog = archive_progress(&t);
        match rf_archive::extract_with(&archive, &dest, &opts, &mut prog, &|| cancel.load(Ordering::Relaxed)) {
            Ok(_) => {
                *made2.lock() = result;
                Ok(true)
            }
            Err(e) if e.downcast_ref::<rf_archive::Cancelled>().is_some() => Ok(false),
            Err(e) => {
                if crate::archive_view::password_problem(&e) == Some(true) {
                    // forget it: the next attempt asks again
                    crate::archive_view::set_password(&archive, None);
                }
                Err(e)
            }
        }
    };
    let done: OnDone = Box::new(move |ok, cx| {
        let dir = made.lock().take();
        if let (true, Some(dir), true) = (ok, dir, cx.global::<crate::settings::Settings>().open_after_extract) {
            if dir.is_dir() {
                crate::workspace::route_ipc(dir.to_string_lossy().into_owned(), cx);
            }
        }
    });
    start_with(cx, title, work, Some(done));
}

/// Like `compress_level`, encrypted (ZIP: AES-256, 7z: AES-256 incl. names).
pub fn compress_protected(cx: &mut App, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, format: rf_archive::Format, level: u32, password: String) {
    compress_opts(cx, sources, dest, format, level, rf_archive::CompressOptions { password: Some(password), ..Default::default() })
}

/// Creates an archive with options: a password, a 7z dictionary size, parts of a given size.
pub fn compress_opts(cx: &mut App, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, format: rf_archive::Format, level: u32, opts: rf_archive::CompressOptions) {
    let title = format!("{} {} → {}", t!("Sıkıştırılıyor:"), crate::i18n::items(sources.len()), short(&dest));
    start(cx, title, move |t| {
        let cancel = t.cancel.clone();
        let mut prog = archive_progress(&t);
        match rf_archive::compress_with(&sources, &dest, format, level, &opts, &mut prog, &|| cancel.load(Ordering::Relaxed)) {
            Ok(_) => Ok(true),
            Err(e) if e.downcast_ref::<rf_archive::Cancelled>().is_some() => Ok(false),
            Err(e) => Err(e),
        }
    });
}

/// RAR through WinRAR's Rar.exe (`level` 0…9).
pub fn compress_rar(cx: &mut App, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, level: u32, password: Option<String>) {
    let title = format!("{} {} → {}", t!("Sıkıştırılıyor:"), crate::i18n::items(sources.len()), short(&dest));
    start(cx, title, move |t| {
        let cancel = t.cancel.clone();
        let mut prog = archive_progress(&t);
        match rf_archive::compress_rar(&sources, &dest, level, password.as_deref(), &mut prog, &|| cancel.load(Ordering::Relaxed)) {
            Ok(_) => Ok(true),
            Err(e) if e.downcast_ref::<rf_archive::Cancelled>().is_some() => Ok(false),
            Err(e) => Err(e),
        }
    });
}

pub fn compress(cx: &mut App, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, format: rf_archive::Format) {
    compress_level(cx, sources, dest, format, 5)
}

/// `level`: 0 store … 9 smallest.
pub fn compress_level(cx: &mut App, sources: Vec<std::path::PathBuf>, dest: std::path::PathBuf, format: rf_archive::Format, level: u32) {
    let title = format!("{} {} → {}", t!("Sıkıştırılıyor:"), crate::i18n::items(sources.len()), short(&dest));
    start(cx, title, move |t| {
        let cancel = t.cancel.clone();
        let mut prog = archive_progress(&t);
        match rf_archive::compress(&sources, &dest, format, level, &mut prog, &|| cancel.load(Ordering::Relaxed)) {
            Ok(_) => Ok(true),
            Err(e) if e.downcast_ref::<rf_archive::Cancelled>().is_some() => Ok(false),
            Err(e) => Err(e),
        }
    });
}

pub fn delete(cx: &mut App, paths: Vec<std::path::PathBuf>, permanent: bool) {
    delete_ex(cx, paths, permanent, true)
}

pub fn delete_ex(cx: &mut App, paths: Vec<std::path::PathBuf>, permanent: bool, record: bool) {
    let title = if permanent {
        format!("{} {}", t!("Kalıcı olarak siliniyor:"), crate::i18n::items(paths.len()))
    } else {
        format!("{} {}", t!("Geri dönüşüm kutusuna taşınıyor:"), crate::i18n::items(paths.len()))
    };
    let fast = cx.global::<crate::settings::Settings>().fast_file_ops;
    let mark = Arc::new(std::sync::Mutex::new(None::<std::path::PathBuf>));
    let marker = mark.clone();
    let id = start_with_id(cx, title, move |t| {
        // permanent: our parallel delete (the shell takes what it can't); recycle bin: the shell
        let r = if permanent && fast {
            rf_win::fastdelete::delete_permanent(&paths, t.clone(), rf_win::fastdelete::threads_for(&paths))
        } else {
            fileops::delete_tracked(&paths, permanent, t.clone())
        };
        let pairs = t.results.lock().unwrap().clone();
        if record && !permanent && !pairs.is_empty() {
            *marker.lock().unwrap() = pairs.first().map(|(_, bin)| bin.clone());
            crate::undo::push(crate::undo::UndoOp::Recycled { pairs });
        }
        r
    }, None);
    if record && !permanent {
        set_extras(cx, id, None, None, Some(mark));
    }
}

/// Title-bar button (top right, like Files): idle icon, a progress bar under it while work
/// runs, red when a failure has not been looked at. Click toggles the flyout.
/// Puts the operations' progress on every main window's taskbar button (when it changed by a
/// tenth of a percent or more).
pub fn sync_taskbar(ops: &Entity<Ops>, cx: &mut App) {
    use rf_win::taskbar_progress::State;
    thread_local! {
        static LAST: std::cell::Cell<(State, usize)> = const { std::cell::Cell::new((State::None, 0)) };
    }
    let state = match ops.read(cx).taskbar() {
        State::Normal(v) => State::Normal((v * 1000.0).round() / 1000.0),
        State::Paused(v) => State::Paused((v * 1000.0).round() / 1000.0),
        State::Error(v) => State::Error((v * 1000.0).round() / 1000.0),
        s => s,
    };
    let windows = crate::workspace::main_windows(cx);
    // (a window opened meanwhile gets the state too)
    if LAST.with(|l| l.get()) == (state, windows.len()) {
        return;
    }
    LAST.with(|l| l.set((state, windows.len())));
    for w in windows {
        let _ = w.update(cx, |_, window, _| {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(RawWindowHandle::Win32(h)) = HasWindowHandle::window_handle(window).map(|h| h.as_raw()) {
                rf_win::taskbar_progress::set(h.hwnd.get(), state);
            }
        });
    }
}

pub fn render_button(ops: &Entity<Ops>, p: &Palette, cx: &App) -> AnyElement {
    let o = ops.read(cx);
    let running = o.running();
    let ring = o.ring_state();
    let (open, unseen) = (o.open, o.unseen_failure);
    let ops = ops.clone();
    let color = if unseen { p.danger } else if running > 0 { p.accent } else { p.text };
    // the ring around the icon: the overall progress, a turning arc before the first report, the failed look in red
    let animate = crate::settings::animations();
    let ring_color = if unseen { p.danger } else { p.accent };
    div()
        .id("status-center")
        // no tooltip while the flyout is open: it would sit on top of the flyout
        .when(!open, |d| d.tooltip(|w, cx| gpui_kit::component::tooltip::Tooltip::new(t!("Durum Merkezi")).build(w, cx)))
        .relative()
        .size(px(32.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(theme::RADIUS_SM)
        .text_color(color)
        .when(open, |d| d.bg(p.pressed))
        .when(!open, |d| d.hover(|s| s.bg(p.hover)))
        .active(|s| s.bg(p.pressed))
        .children(crate::transfer_view::ring_layer(&ring, ring_color, p.text, animate))
        .children(crate::transfer_view::pulse_layer(&ring, p.accent, animate))
        .child(Icon::new(if unseen { L::CircleAlert } else { L::ListChecks }).size(px(16.)))
        .when(running == 0 && matches!(crate::installer::state(), crate::installer::State::Ready(_)), |d| {
            let dot = div().absolute().top(px(5.)).right(px(5.)).size(px(7.)).rounded_full().bg(p.accent);
            // a few seconds of pulse when it becomes ready (an endless one would keep drawing frames)
            let fresh = crate::installer::ready_since().is_some_and(|t| t.elapsed() < std::time::Duration::from_secs(8));
            if fresh {
                let ring = p.accent;
                d.child(dot).child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(theme::RADIUS_SM)
                        .border_2()
                        .border_color(ring)
                        .with_animation(
                            "sc-pulse",
                            Animation::new(std::time::Duration::from_millis(1300)).repeat().with_easing(pulsating_between(0.0, 0.9)),
                            |el, t| el.opacity(t),
                        ),
                )
            } else {
                d.child(dot)
            }
        })
        .when(running > 1, |d| {
            d.child(
                div()
                    .absolute()
                    .top(px(2.))
                    .right(px(2.))
                    .min_w(px(14.))
                    .h(px(14.))
                    .px(px(3.))
                    .rounded_full()
                    .bg(p.accent)
                    .text_color(p.accent_text)
                    .text_size(px(9.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(running.to_string()),
            )
        })
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| {
            let owner = window.window_handle();
            ops.update(cx, |o, cx| o.toggle(owner, cx))
        })
        .into_any_element()
}

/// The Status Center flyout under the title-bar button, with a click-away layer behind it.
pub fn render_flyout(ops: &Entity<Ops>, p: &Palette, top: Pixels, cx: &App) -> Option<AnyElement> {
    let o = ops.read(cx);
    if !o.open || !o.in_window {
        return None;
    }
    let any_finished = o.ops.iter().any(|x| x.phase != OpPhase::Running);
    let mut list = v_flex().gap_1();
    // newest first
    for op in o.ops.iter().rev() {
        list = list.child(render_op(ops, op, p));
    }
    let (close_e, clear_e) = (ops.clone(), ops.clone());
    let update = update_row(p);
    let body: AnyElement = if o.ops.is_empty() && update.is_some() {
        div().into_any_element()
    } else if o.ops.is_empty() {
        div()
            .py_6()
            .flex()
            .justify_center()
            .text_sm()
            .text_color(p.text_secondary)
            .child(t!("Devam eden dosya işlemleri yok"))
            .into_any_element()
    } else {
        div().id("sc-list").max_h(px(460.)).overflow_y_scroll().child(list).into_any_element()
    };
    Some(
        div()
            .absolute()
            .inset_0()
            // click-away layer
            .child(
                div()
                    .id("sc-away")
                    .absolute()
                    .inset_0()
                    .on_mouse_down(MouseButton::Left, move |_, _, cx| close_e.update(cx, |o, cx| o.close(cx)))
                    .on_mouse_down(MouseButton::Right, {
                        let e = ops.clone();
                        move |_, _, cx| e.update(cx, |o, cx| o.close(cx))
                    }),
            )
            .child(
                v_flex()
                    .id("sc-flyout")
                    .absolute()
                    .top(top)
                    .right(px(8.))
                    .w(px(400.))
                    .p_1()
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(
                        h_flex()
                            .px_3()
                            .pt_2()
                            .pb_2()
                            .items_center()
                            .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!("Durum Merkezi")))
                            .child(
                                div()
                                    .id("sc-clear")
                                    .px_2()
                                    .py_1()
                                    .rounded(theme::RADIUS_SM)
                                    .text_xs()
                                    .border_1()
                                    .border_color(p.border)
                                    .when(!any_finished, |d| d.text_color(p.text_tertiary))
                                    .when(any_finished, |d| {
                                        d.hover(|s| s.bg(p.hover))
                                            .active(|s| s.bg(p.pressed))
                                            .on_click(move |_, _, cx| clear_e.update(cx, |o, cx| o.clear_finished(cx)))
                                    })
                                    .child(t!("Tamamlananları temizle")),
                            ),
                    )
                    .children(update)
                    .child(body),
            )
            .into_any_element(),
    )
}

fn render_op(ops: &Entity<Ops>, op: &OpState, p: &Palette) -> AnyElement {
    let frac = if op.total > 0 { op.done as f32 / op.total as f32 } else { 0.0 };
    let is_paused = op.paused.load(Ordering::Relaxed);
    let running = op.phase == OpPhase::Running;
    let (ops_e, op_id) = (ops.clone(), op.id);
    let (status, color, icon) = match &op.phase {
        OpPhase::Running if is_paused => (t!("Duraklatıldı").to_string(), p.text_secondary, L::Pause),
        OpPhase::Running => {
            let elapsed = op.started.elapsed().as_secs_f32();
            // speed and time left from the moving average of the last few seconds
            let pace = op.stats.pace(op.secs(), (op.done, op.total), op.bytes.get());
            let text = if op.total == 0 {
                format!("{} {}", elapsed as u64, t!("sn"))
            } else {
                let mut text = crate::i18n::pct(frac * 100.0);
                if let Some(v) = pace.speed.filter(|v| *v >= 1.0) {
                    text = format!("{text} · {}", crate::transfer_view::speed_text(v));
                }
                match pace.eta_secs {
                    Some(r) => format!("{text} · {} {}", fmt_duration(r as f32), t!("kaldı")),
                    None => format!("{text} · {} {}", elapsed as u64, t!("sn")),
                }
            };
            (text, p.text_secondary, L::LoaderCircle)
        }
        OpPhase::Done => (t!("Tamamlandı").to_string(), p.accent, L::CircleCheck),
        OpPhase::Cancelled => (t!("İptal edildi").to_string(), p.text_secondary, L::CircleSlash),
        OpPhase::Failed(e) => (format!("{} {}", t!("Hata:"), crate::i18n::td(e)), p.danger, L::CircleAlert),
    };
    let icon_color = match &op.phase {
        OpPhase::Running => p.accent,
        OpPhase::Done => gpui_kit::rgb(0x30D158).into(),
        _ => color,
    };
    h_flex()
        .id(("op", op.id as usize))
        .group("op")
        .gap_3()
        .px_3()
        .py_2()
        .items_start()
        .rounded(theme::RADIUS_SM)
        .hover(|s| s.bg(p.hover))
        .child(div().pt(px(2.)).child(Icon::new(icon).size(px(16.)).text_color(icon_color)))
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_1()
                .child(div().text_sm().overflow_hidden().text_ellipsis().whitespace_nowrap().child(op.title.clone()))
                .when(running, |d| {
                    d.child(
                        div()
                            .h(px(3.))
                            .w_full()
                            .rounded_full()
                            .bg(p.pressed)
                            // unknown total (git, network): a short accent segment
                            .child(div().h_full().rounded_full().bg(p.accent).w(relative(if op.total == 0 { 0.15 } else { frac.max(0.02) }))),
                    )
                })
                .child(
                    h_flex()
                        .gap_2()
                        .text_xs()
                        .text_color(p.text_secondary)
                        .child(div().flex_shrink_0().text_color(color).child(status))
                        .children(op.current.clone().filter(|_| running).map(|c| {
                            div().flex_1().min_w_0().overflow_hidden().text_ellipsis().whitespace_nowrap().child(c)
                        }))
                        .children(op.action.as_ref().filter(|_| !running).map(|(label, run)| {
                            let run = run.clone();
                            div()
                                .id(("op-act", op.id as usize))
                                .flex_shrink_0()
                                .h(px(24.))
                                .flex()
                                .items_center()
                                .px_2()
                                .rounded(theme::RADIUS_SM)
                                .text_color(p.accent)
                                .cursor_pointer()
                                .hover(|s| s.bg(p.pressed))
                                .tab_index(0)
                                .child(label.clone())
                                .on_click(move |_, _, cx| run(cx))
                        })),
                ),
        )
        .when(running, |d| {
            d.child(
                div()
                    .id(("op-pause", op.id as usize))
                    .size(px(24.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(theme::RADIUS_SM)
                    .text_color(p.text_secondary)
                    .tooltip(move |w, cx| gpui_kit::component::tooltip::Tooltip::new(if is_paused { t!("Devam et") } else { t!("Duraklat") }).build(w, cx))
                    .hover(|s| s.bg(p.pressed))
                    .child(Icon::new(if is_paused { L::Play } else { L::Pause }).size(px(12.)))
                    .on_click({
                        let ops_e = ops.clone();
                        move |_, _, cx| ops_e.update(cx, |o, cx| o.toggle_pause(op_id, cx))
                    }),
            )
        })
        .child(
            div()
                .id(("op-x", op.id as usize))
                .size(px(24.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .rounded(theme::RADIUS_SM)
                .text_color(p.text_secondary)
                .tooltip(move |w, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(if running { t!("İptal") } else { t!("Kapat") }).build(w, cx)
                })
                .hover(|s| s.bg(p.pressed))
                .child(Icon::new(L::X).size(px(12.)))
                .on_click(move |_, _, cx| {
                    if running {
                        ops_e.update(cx, |o, cx| o.cancel_op(op_id, cx));
                    } else {
                        ops_e.update(cx, |o, cx| {
                            o.ops.retain(|x| x.id != op_id);
                            cx.notify();
                        })
                    }
                }),
        )
        .into_any_element()
}

/// "~45 sn", "~3 dk", "~1 sa 5 dk" (rounded; it is an estimate).
pub(crate) fn fmt_duration(secs: f32) -> String {
    fmt_duration_with(secs, (crate::t!("sn"), crate::t!("dk"), crate::t!("sa")))
}

/// Unit labels are parameters so the formatting is testable without the global language.
fn fmt_duration_with(secs: f32, (sec, min, hour): (&str, &str, &str)) -> String {
    let s = secs.max(1.0).round() as u64;
    if s < 60 {
        return format!("~{s} {sec}");
    }
    // round to whole minutes first, then roll over (3590 s is "~1 sa", never "~60 dk")
    let total_min = (s + 30) / 60;
    if total_min < 60 {
        format!("~{total_min} {min}")
    } else {
        format!("~{} {hour} {} {min}", total_min / 60, total_min % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::{Glance, OpPhase, OpState, Ops, Ordering, Question, fmt_duration_with, glance_of};
    use rf_core::transfer::{self as xfer, Stage};
    use rf_win::fastcopy::Resolve;
    use std::time::{Duration, Instant};

    fn op(id: u64, title: &str, phase: OpPhase, done: u32, total: u32) -> OpState {
        OpState { total, done, phase, ..OpState::new(id, title.into(), Default::default(), Default::default(), Default::default()) }
    }

    #[test]
    fn a_window_without_a_status_button_sees_the_running_operation_and_the_failure() {
        assert_eq!(glance_of(&[]), Glance::default());
        let ops = [
            op(1, "old", OpPhase::Failed("access denied".into()), 3, 3),
            op(2, "copy", OpPhase::Done, 5, 5),
            op(3, "move", OpPhase::Running, 1, 4),
            op(4, "later", OpPhase::Running, 0, 0),
        ];
        let g = glance_of(&ops);
        assert_eq!(g.running, Some(("later".into(), None)), "the newest running one; no percentage before the first report");
        assert_eq!(g.failed, Some((1, "access denied".into())));
        let g = glance_of(&ops[..3]);
        assert_eq!(g.running, Some(("move".into(), Some(25))));
        // the newest failure wins, and a finished operation is not "running"
        let ops = [op(1, "a", OpPhase::Failed("x".into()), 0, 1), op(2, "b", OpPhase::Failed("y".into()), 0, 1), op(3, "c", OpPhase::Cancelled, 0, 1)];
        assert_eq!(glance_of(&ops), Glance { running: None, failed: Some((2, "y".into())) });
        // done past total does not exceed 100
        assert_eq!(glance_of(&[op(1, "z", OpPhase::Running, 9, 4)]).running, Some(("z".into(), Some(100))));
    }

    #[test]
    fn durations() {
        let u = ("sn", "dk", "sa");
        assert_eq!(fmt_duration_with(0.2, u), "~1 sn");
        assert_eq!(fmt_duration_with(45.0, u), "~45 sn");
        assert_eq!(fmt_duration_with(150.0, u), "~3 dk");
        assert_eq!(fmt_duration_with(3580.0, u), "~1 sa 0 dk");
        assert_eq!(fmt_duration_with(3900.0, u), "~1 sa 5 dk");
    }

    fn ago(ms: u64) -> Instant {
        Instant::now().checked_sub(Duration::from_millis(ms)).expect("the clock is far enough from its start")
    }

    #[test]
    fn a_run_that_was_over_before_anyone_could_see_it_leaves_no_ring_and_no_pulse_at_the_end() {
        let mut o = Ops::blank();
        o.began();
        assert!(o.ring_state().pulse.is_some(), "a start pulses at once");
        o.ops.push(op(1, "a", OpPhase::Done, 1, 1));
        o.ended_one();
        assert_eq!(o.all_done, None);
        assert_eq!(o.ring_state().since_end_ms, None);
        assert_eq!(o.busy_since, None, "the run is over");
    }

    #[test]
    fn a_long_run_closes_its_ring_and_pulses_once_more_when_the_last_operation_ends() {
        let mut o = Ops::blank();
        o.began();
        let first = o.pulse_count;
        o.busy_since = Some(ago(3000));
        o.ops.push(op(1, "a", OpPhase::Running, 1, 2));
        o.began();
        assert_eq!(o.busy_since.map(|t| t.elapsed() >= Duration::from_millis(2900)), Some(true), "a second operation does not restart the run");
        assert!(o.pulse_count > first, "each start pulses");
        // one of the two ends: the other still runs
        o.ops.push(op(2, "b", OpPhase::Running, 0, 0));
        o.ops[0].phase = OpPhase::Done;
        o.ended_one();
        assert_eq!(o.all_done, None);
        assert!(o.ring_state().running);
        // the last one ends
        o.ops[1].phase = OpPhase::Done;
        let before = o.pulse_count;
        o.ended_one();
        assert!(o.all_done.is_some() && o.pulse_count == before + 1 && o.done_epoch == o.pulse_count);
        let r = o.ring_state();
        assert!(!r.running && r.since_end_ms.is_some() && r.end_epoch == o.done_epoch);
        // a new run takes the fade away
        o.began();
        assert_eq!(o.all_done, None);
    }

    #[test]
    fn the_ring_has_no_amount_until_a_total_is_known() {
        let mut o = Ops::blank();
        assert_eq!(o.ring_state().fraction, None);
        o.ops.push(op(1, "git", OpPhase::Running, 0, 0));
        let r = o.ring_state();
        assert!(r.running && r.fraction.is_none(), "an unknown total turns the short arc");
        o.ops.push(op(2, "copy", OpPhase::Running, 1, 4));
        // the overall progress is the mean of the running ones (the unknown counts as nothing done)
        assert_eq!(o.ring_state().fraction, Some(0.125));
    }

    #[test]
    fn the_strip_and_cards_see_what_runs_what_just_ended_and_failures_nobody_looked_at() {
        let mut o = Ops::blank();
        let mut old = op(1, "old", OpPhase::Done, 1, 1);
        old.ended = Some(ago(11_000));
        let mut recent = op(2, "recent", OpPhase::Done, 1, 1);
        recent.ended = Some(ago(500));
        let failed = op(3, "failed", OpPhase::Failed("x".into()), 0, 1);
        let mut seen = op(4, "seen", OpPhase::Failed("y".into()), 0, 1);
        seen.dismissed = true;
        let running = op(5, "running", OpPhase::Running, 1, 3);
        o.ops = vec![old, recent, failed, seen, running];
        let ids: Vec<u64> = o.views().iter().map(|v| v.id).collect();
        assert_eq!(ids, [2, 3, 5]);
        // the real thing the button calls
        o.dismissed(3);
        assert_eq!(o.views().iter().map(|v| v.id).collect::<Vec<_>>(), [2, 5]);
    }

    type Replies = std::sync::mpsc::Receiver<Option<Vec<Resolve>>>;

    /// An operation waiting on a question about `count` names, and where its answer will arrive.
    fn asking(id: u64, count: usize) -> (OpState, Replies) {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut o = op(id, "asking", OpPhase::Running, 0, 1);
        o.question = Some(Question { count, list: Vec::new(), reply: std::rc::Rc::new(std::cell::RefCell::new(Some(tx))) });
        (o, rx)
    }

    #[test]
    fn an_answer_goes_to_the_operation_that_asked_and_the_first_answer_wins() {
        let mut o = Ops::blank();
        let ((a, rx_a), (b, rx_b)) = (asking(1, 2), asking(2, 3));
        o.ops = vec![a, b];
        // the second operation is answered: only its channel hears, with one answer per name
        assert!(o.answered(2, Resolve::Replace).is_some());
        assert_eq!(rx_b.try_recv().unwrap(), Some(vec![Resolve::Replace; 3]));
        assert!(rx_a.try_recv().is_err(), "the first operation's question is still open");
        assert!(o.ops[0].question.is_some() && o.ops[1].question.is_none());
        // a second answer for it finds nothing to answer
        assert!(o.answered(2, Resolve::Skip).is_none());
        assert!(rx_b.try_recv().is_err());
        // the dialog answered the first one before the strip: the strip's answer is not sent
        let taken = o.ops[0].question.as_ref().unwrap().reply.borrow_mut().take();
        taken.unwrap().send(Some(vec![Resolve::KeepBoth; 2])).unwrap();
        assert!(o.answered(1, Resolve::Skip).is_none());
        assert_eq!(rx_a.try_recv().unwrap(), Some(vec![Resolve::KeepBoth; 2]));
        assert!(rx_a.try_recv().is_err());
        // an operation that is not there
        assert!(o.answered(99, Resolve::Skip).is_none());
    }

    #[test]
    fn cancelling_stops_that_operation_and_answers_its_question_cancel() {
        let mut o = Ops::blank();
        let ((a, rx_a), (b, rx_b)) = (asking(1, 2), asking(2, 2));
        o.ops = vec![a, b, op(3, "plain", OpPhase::Running, 1, 4)];
        assert!(o.cancelled(1).is_some());
        assert!(o.ops[0].cancel.load(Ordering::Relaxed));
        assert_eq!(rx_a.try_recv().unwrap(), None, "a cancelled copy is told to stop");
        // the others were not touched
        assert!(!o.ops[1].cancel.load(Ordering::Relaxed) && o.ops[1].question.is_some());
        assert!(rx_b.try_recv().is_err());
        // one without a question is only flagged
        assert!(o.cancelled(3).is_none());
        assert!(o.ops[2].cancel.load(Ordering::Relaxed));
        assert!(o.cancelled(99).is_none());
    }

    #[test]
    fn pausing_flips_the_operation_and_hides_its_pace() {
        let mut o = Ops::blank();
        let mut running = op(1, "copy", OpPhase::Running, 0, 1000);
        let mut t = 0.0;
        while t < 4.0 {
            t += 0.1;
            running.stats.update(t, (0, 1000), ((t * 1_000_000.0) as u64, 8_000_000));
        }
        o.ops = vec![running, op(2, "other", OpPhase::Running, 0, 10)];
        assert!(!o.ops[0].paused.load(Ordering::Relaxed));
        o.pause_toggled(1);
        assert!(o.ops[0].paused.load(Ordering::Relaxed));
        assert!(!o.ops[1].paused.load(Ordering::Relaxed), "only the one asked about");
        let pace = o.ops[0].stats.pace(t, (0, 1000), ((t * 1_000_000.0) as u64, 8_000_000));
        assert_eq!(pace.speed, None, "no speed while it waits");
        o.pause_toggled(1);
        assert!(!o.ops[0].paused.load(Ordering::Relaxed));
        o.pause_toggled(99);
    }

    #[test]
    fn a_failure_stays_unseen_until_the_last_one_is_sent_away() {
        let mut o = Ops::blank();
        o.ops = vec![op(1, "a", OpPhase::Failed("x".into()), 0, 1), op(2, "b", OpPhase::Failed("y".into()), 0, 1)];
        o.unseen_failure = true;
        o.dismissed(1);
        assert!(o.ops[0].dismissed && !o.ops[1].dismissed);
        assert!(o.unseen_failure, "another failure is still waiting");
        o.dismissed(2);
        assert!(!o.unseen_failure);
        o.collapsed_toggled(1);
        assert!(o.ops[0].collapsed && !o.ops[1].collapsed);
        o.collapsed_toggled(1);
        assert!(!o.ops[0].collapsed);
    }

    #[test]
    fn the_wake_up_runs_once_per_run_and_only_for_work_that_is_not_waiting() {
        let mut o = Ops::blank();
        assert!(!o.wants_tick(), "nothing runs");
        assert!(o.began(), "the first operation starts the wake-up");
        assert!(!o.began(), "a second one does not start another");
        o.ops = vec![op(1, "a", OpPhase::Running, 0, 1)];
        assert!(o.wants_tick());
        o.ops[0].paused.store(true, Ordering::Relaxed);
        assert!(!o.wants_tick(), "a paused operation changes nothing by the clock");
        o.ops[0].paused.store(false, Ordering::Relaxed);
        o.ops[0].phase = OpPhase::Done;
        assert!(!o.wants_tick());
    }

    #[test]
    fn a_card_waits_for_the_operation_to_have_run_a_moment_and_a_question_does_not() {
        let mut slow = op(1, "slow", OpPhase::Running, 1, 4);
        slow.started = ago(2000);
        assert_eq!(slow.view().stage, Stage::Running);
        let fresh = op(2, "fresh", OpPhase::Running, 1, 4);
        assert_eq!(fresh.view().stage, Stage::Hidden);
        // a question
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut asking = op(3, "asking", OpPhase::Running, 0, 1);
        asking.question = Some(Question { count: 3, list: Vec::new(), reply: std::rc::Rc::new(std::cell::RefCell::new(Some(tx))) });
        let v = asking.view();
        assert_eq!((v.stage, v.question), (Stage::Asking, Some(3)));
        assert!(v.brief.asking);
        // answered elsewhere (the dialog took the reply): the question is gone
        if let Some(q) = &asking.question {
            q.reply.borrow_mut().take();
        }
        let v = asking.view();
        assert_eq!(v.question, None);
        assert!(!v.brief.asking);
    }

    #[test]
    fn a_finished_operations_card_lives_by_how_long_ago_it_ended() {
        let mut done = op(1, "done", OpPhase::Done, 1, 1);
        done.started = ago(10_000);
        done.ended = Some(ago(1000));
        assert_eq!(done.view().stage, Stage::Result(xfer::Outcome::Done));
        done.ended = Some(ago(5000));
        assert_eq!(done.view().stage, Stage::Hidden);
        // a failure stays however long ago it was, until it is sent away
        let mut bad = op(2, "bad", OpPhase::Failed("disk full".into()), 0, 1);
        bad.started = ago(3_600_000);
        bad.ended = Some(ago(3_000_000));
        assert_eq!(bad.view().stage, Stage::Result(xfer::Outcome::Failed));
        bad.dismissed = true;
        assert_eq!(bad.view().stage, Stage::Hidden);
    }
}

// ---------- Acrylic popup (Windows 11 flyout material) ----------

const FLYOUT_W: f32 = 400.;

struct ScPopup(Option<AnyWindowHandle>);
impl Global for ScPopup {}

/// Keeps the popup window in step with `Ops::open` (call from an observer of the ops entity).
pub fn sync_popup(ops: &Entity<Ops>, cx: &mut App) {
    let (open, owner, in_window) = {
        let o = ops.read(cx);
        (o.open, o.owner, o.in_window)
    };
    let cur = cx.try_global::<ScPopup>().and_then(|g| g.0);
    match (open, cur) {
        (true, None) if !in_window => {
            let Some(owner) = owner else { return };
            // the app window keeps its active look (Mica, caption) while the flyout has focus
            gpui_windows::KEEP_ACTIVE_LOOK.store(true, std::sync::atomic::Ordering::Relaxed);
            let opened = if std::env::var_os("RF_INWINDOW_MENU").is_some() { None } else { open_popup(ops, owner, cx) };
            match opened {
                Some(h) => cx.set_global(ScPopup(Some(h))),
                None => ops.update(cx, |o, cx| {
                    gpui_windows::KEEP_ACTIVE_LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
                    o.in_window = true;
                    cx.notify();
                }),
            }
        }
        (false, Some(h)) => {
            gpui_windows::KEEP_ACTIVE_LOOK.store(false, std::sync::atomic::Ordering::Relaxed);
            cx.set_global(ScPopup(None));
            let _ = h.update(cx, |_, w, _| w.remove_window());
        }
        (false, None) if in_window => ops.update(cx, |o, _| o.in_window = false),
        _ => {}
    }
}

fn open_popup(ops: &Entity<Ops>, owner: AnyWindowHandle, cx: &mut App) -> Option<AnyWindowHandle> {
    // anchored under the title-bar button: right edge 8 px in from the window's
    let (origin, width, dark) = owner
        .update(cx, |_, w, _| (w.bounds().origin, w.viewport_size().width, Palette::for_window(w).dark))
        .ok()?;
    let top = theme::TITLEBAR_H + px(44.);
    let x = origin.x + width - px(8. + FLYOUT_W);
    let bounds = Bounds::new(point(x, origin.y + top), size(px(FLYOUT_W), px(120.)));
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus: true,
        show: true,
        kind: WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Blurred,
        ..Default::default()
    };
    let ops = ops.clone();
    let r = cx.open_window(opts, move |w, cx| {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = w.window_handle() {
            if let RawWindowHandle::Win32(h) = h.as_raw() {
                rf_win::window::set_dark_mode(h.hwnd.get(), dark);
                crate::glass::keep_out_of_capture(h.hwnd.get());
            }
        }
        cx.new(|cx| ScView::new(ops, w, cx))
    });
    match r {
        Ok(h) => Some(h.into()),
        Err(e) => {
            log::warn!("status center popup: {e:#}");
            None
        }
    }
}

/// The Status Center in its own Acrylic window; sizes itself to its content.
struct ScView {
    ops: Entity<Ops>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl ScView {
    fn new(ops: Entity<Ops>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subs = vec![
            cx.observe(&ops, |_, _, cx| cx.notify()),
            // clicking anywhere else closes it, like a Windows 11 flyout
            cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    return;
                }
                let ops = this.ops.clone();
                cx.defer(move |cx| {
                    ops.update(cx, |o, cx| {
                        if o.open {
                            o.open = false;
                            o.auto_closed = Some(Instant::now());
                            cx.notify();
                        }
                    })
                });
            }),
        ];
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self { ops, focus, _subs: subs }
    }
}

impl Render for ScView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = Palette::for_window(window);
        let ops = self.ops.clone();
        let panel = panel(&ops, &p, cx);
        let vh = window.viewport_size().height;
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .text_color(p.text)
            .on_key_down(cx.listener(|this, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    this.ops.update(cx, |o, cx| o.close(cx));
                }
            }))
            .child(
                div()
                    .relative()
                    .w_full()
                    .child(panel)
                    // measure the content; the window follows its height
                    .child(
                        canvas(
                            move |b, window, _| {
                                let h = b.size.height.ceil();
                                if (f32::from(h) - f32::from(vh)).abs() > 1. {
                                    window.on_next_frame(move |w, _| w.resize(size(px(FLYOUT_W), h)));
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .inset_0(),
                    ),
            )
    }
}

/// Header + list for the popup.
fn panel(ops: &Entity<Ops>, p: &Palette, cx: &App) -> Div {
    let o = ops.read(cx);
    let any_finished = o.ops.iter().any(|x| x.phase != OpPhase::Running);
    let mut list = v_flex().gap_1();
    for op in o.ops.iter().rev() {
        list = list.child(render_op(ops, op, p));
    }
    let clear_e = ops.clone();
    let update = update_row(p);
    let body: AnyElement = if o.ops.is_empty() && update.is_some() {
        div().into_any_element()
    } else if o.ops.is_empty() {
        div().py_6().flex().justify_center().text_sm().text_color(p.text_secondary).child(t!("Devam eden dosya işlemleri yok")).into_any_element()
    } else {
        div().id("sc-list").max_h(px(460.)).overflow_y_scroll().child(list).into_any_element()
    };
    v_flex()
        .w_full()
        .p_1()
        .child(
            h_flex()
                .px_3()
                .pt_2()
                .pb_2()
                .items_center()
                .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!("Durum Merkezi")))
                .child(
                    div()
                        .id("sc-clear")
                        .px_2()
                        .py_1()
                        .rounded(theme::RADIUS_SM)
                        .text_xs()
                        .border_1()
                        .border_color(p.border)
                        .when(!any_finished, |d| d.text_color(p.text_tertiary))
                        .when(any_finished, |d| {
                            d.hover(|s| s.bg(p.hover)).active(|s| s.bg(p.pressed)).on_click(move |_, _, cx| clear_e.update(cx, |o, cx| o.clear_finished(cx)))
                        })
                        .child(t!("Tamamlananları temizle")),
                ),
        )
        .children(update)
        .child(body)
}

/// The app's own update, pinned at the top of the Status Center while it downloads and once it
/// is ready (`None` otherwise).
fn update_row(p: &Palette) -> Option<AnyElement> {
    use crate::installer::State;
    let (title, detail, pct, ready) = match crate::installer::state() {
        State::Downloading(v, pct) => (t!("rustyfiles {v} indiriliyor").replace("{v}", &v), format!("%{pct}"), Some(pct), false),
        State::Ready(v) => (t!("rustyfiles {v} hazır").replace("{v}", &v), t!("Yeniden başlatınca kurulur").to_string(), None, true),
        _ => return None,
    };
    Some(
        v_flex()
            .mx_1()
            .mb_1()
            .px_3()
            .py_2p5()
            .gap_2()
            .rounded(theme::RADIUS_SM)
            .bg(p.selection)
            .border_1()
            .border_color(p.accent)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(Icon::new(if ready { L::RefreshCw } else { L::Download }).size(px(16.)).text_color(p.accent))
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(div().text_xs().text_color(p.text_secondary).child(detail)),
                    )
                    .when(ready, |d| {
                        d.child(
                            div()
                                .id("sc-update-restart")
                                .px_3()
                                .py_1()
                                .rounded(theme::RADIUS_SM)
                                .bg(p.accent)
                                .text_color(p.accent_text)
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .cursor_pointer()
                                .hover(|s| s.opacity(0.9))
                                .child(t!("Yeniden başlat"))
                                .on_click(|_, _, _| {
                                    std::thread::spawn(crate::installer::restart_to_update);
                                }),
                        )
                    }),
            )
            .children(pct.map(|pct| {
                div().h(px(3.)).w_full().rounded_full().bg(p.pressed).child(div().h_full().rounded_full().bg(p.accent).w(relative((pct as f32 / 100.).clamp(0.02, 1.0))))
            }))
            .into_any_element(),
    )
}

/// The version whose "update ready" toast was closed (it doesn't come back for that version).
static TOAST_CLOSED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// A small card at the bottom of the window once an update is downloaded: the version, Restart
/// and Later. Slides in once; "Later" (or restarting) hides it for that version.
pub fn update_toast(p: &Palette, picker: bool) -> Option<AnyElement> {
    let crate::installer::State::Ready(v) = crate::installer::state() else { return None };
    // file dialogs of other apps don't get it
    if picker || TOAST_CLOSED.lock().unwrap().as_deref() == Some(v.as_str()) {
        return None;
    }
    let ver = v.clone();
    Some(
        div()
            .absolute()
            .bottom(px(44.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                h_flex()
                    .id("update-toast")
                    .gap_3()
                    .pl_4()
                    .pr_2()
                    .py_2()
                    .items_center()
                    .rounded(theme::RADIUS)
                    .bg(p.card_bg)
                    .border_1()
                    .border_color(p.accent)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(Icon::new(L::RefreshCw).size(px(16.)).text_color(p.accent))
                    .child(
                        v_flex()
                            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(t!("rustyfiles {v} hazır").replace("{v}", &v)))
                            .child(div().text_xs().text_color(p.text_secondary).child(t!("Yeniden başlatınca kurulur"))),
                    )
                    .child(
                        div()
                            .id("update-toast-restart")
                            .ml_2()
                            .px_3()
                            .py_1p5()
                            .rounded(theme::RADIUS_SM)
                            .bg(p.accent)
                            .text_color(p.accent_text)
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .cursor_pointer()
                            .hover(|s| s.opacity(0.9))
                            .child(t!("Yeniden başlat"))
                            .on_click(|_, _, _| {
                                std::thread::spawn(crate::installer::restart_to_update);
                            }),
                    )
                    .child(
                        div()
                            .id("update-toast-later")
                            .px_3()
                            .py_1p5()
                            .rounded(theme::RADIUS_SM)
                            .text_sm()
                            .text_color(p.text_secondary)
                            .cursor_pointer()
                            .hover(|s| s.bg(p.hover))
                            .child(t!("Sonra"))
                            .on_click(move |_, window, _| {
                                *TOAST_CLOSED.lock().unwrap() = Some(ver.clone());
                                window.refresh();
                            }),
                    )
                    .with_animation(("update-toast-in", v.len()), Animation::new(std::time::Duration::from_millis(260)).with_easing(ease_out_quint()), |el, t| {
                        el.opacity(t).mb(px(-12. * (1. - t)))
                    }),
            )
            .into_any_element(),
    )
}
