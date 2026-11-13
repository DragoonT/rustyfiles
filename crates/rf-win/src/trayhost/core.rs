//! The handler: what the tray window does with each message. Pure over the traits of `traits.rs`, so every rule below has a test with
//! fakes.
//!
//! FOR EVERY `WM_COPYDATA`, in this order:
//! 1. only if the host is set to answer icon-rectangle queries and this is one: answer it from the interface's rectangle (no pass-on;
//!    a query we cannot answer goes on to step 2);
//! 2. pass the UNTOUCHED message to Explorer's tray window and take ITS answer. Nothing runs before this step except the test of step
//!    1, so nothing we do can keep a message from Explorer: not a parse failure, not a bug in the pixel code, not a full model;
//! 3. own handling: parse, read the icon's pixels, update the model. The sender is still blocked in its `SendMessage` until this
//!    function returns, so its `HICON` is still alive here (the sender can destroy it only after the answer). Explorer's answer is an
//!    input: a modify or version change that Explorer refused is not applied (see `model_step`);
//! 4. only when there is no tray window of Explorer's to pass it to, answer what a tray would (`model::ApplyOutcome::answer`; 0 for
//!    everything else).
//!
//! PANICS. Step 3 runs inside `catch_unwind`. That is a help in the test and development profiles only: the release profile of this
//! workspace sets `panic = "abort"`, where `catch_unwind` does nothing and a panic ends the process. What keeps a message from being
//! lost there is the ORDER above (Explorer has the message before any of our own code that could panic has run), and the handler
//! code is written not to panic (checked indexing and arithmetic in the model and the pixel code; the parser is fuzzed). A panic in a
//! release build still ends the dock process, and the window with it (see `docs/design/tray-host.md`).
//!
//! Every method takes `&self` and the window procedure may be entered again while it waits for Explorer in step 2: nothing here holds
//! a lock across the relay.
//!
//! LOG-ONLY mode (`CoreConfig::log_only`): steps 2 and 4 as always, but step 3 only counts: no pixel is read, no tip, balloon text
//! or name is kept, the model stays empty; the keys of the icons seen are counted so that "N icons" can be logged. The click engine
//! and the rectangle answers are off.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::clicks::{self, ClickOutcome, ClickRequest};
use super::model::{ApplyNote, ImageUpdate, Prepared};
use super::shared::{Shared, bump};
use super::traits::{Clock, IconPixels, OwnerSend, Raw, Relay, Relayed};
use crate::tray_wire::forward::{Method, Outcome, reply_for};
use crate::tray_wire::notify::{BalloonKind, Nim, NotifyIconRequest};
use crate::tray_wire::rect::{IconRectQuery, RectPacking, pack_reply};
use crate::tray_wire::appbar::AbMessage;
use crate::tray_wire::{DW_ICON_RECT, IconKey, TrayRequest, WireError, parse_copydata};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoreConfig {
    /// See the module docs.
    pub log_only: bool,
    /// How to pack the replies to `Shell_NotifyIconGetRect`. `None` (the default) means the host does not answer those queries: they
    /// are passed on to Explorer like everything else. The packing is UNVERIFIED (two sources disagree, probe P6 settles it), and a
    /// wrong answer would put a program's flyout in the wrong place, which is worse than Explorer's answer.
    pub rect_packing: Option<RectPacking>,
}

pub struct Core {
    pub shared: Arc<Shared>,
    cfg: CoreConfig,
    relay: Box<dyn Relay>,
    pixels: Box<dyn IconPixels>,
    owners: Arc<dyn OwnerSend>,
    clock: Box<dyn Clock>,
    /// Log lines still allowed for requests about icons the host never saw ([`UNKNOWN_LINES`] at the start).
    unknown_lines_left: AtomicU32,
}

/// How many requests about unknown icons are described in the log in one run: enough to name every owner of a first run (it counted
/// 7), few enough that a program that keeps deleting an icon it never added cannot fill the log.
pub const UNKNOWN_LINES: u32 = 40;

impl Core {
    pub fn new(shared: Arc<Shared>, cfg: CoreConfig, relay: Box<dyn Relay>, pixels: Box<dyn IconPixels>, owners: Arc<dyn OwnerSend>, clock: Box<dyn Clock>) -> Core {
        Core { shared, cfg, relay, pixels, owners, clock, unknown_lines_left: AtomicU32::new(UNKNOWN_LINES) }
    }

    pub fn config(&self) -> CoreConfig {
        self.cfg
    }

    /// One `WM_COPYDATA`. `dw_data` and `bytes` are the sender's `COPYDATASTRUCT` (valid for this call only), `raw` the message as it
    /// came, to be passed on untouched. The return value is the window procedure's result.
    pub fn on_copydata(&self, dw_data: usize, bytes: &[u8], raw: Raw) -> isize {
        let c = &self.shared.counters;
        bump(&c.messages);
        // Step 1: the one thing that may keep a message from Explorer, and only a rectangle query we are able to answer.
        let mut early_parse = None;
        if dw_data == DW_ICON_RECT && self.cfg.rect_packing.is_some() && !self.cfg.log_only {
            let parsed = parse_copydata(dw_data, bytes);
            if let Ok(TrayRequest::IconRect(q)) = &parsed
                && let Some(v) = self.guarded(|| self.rect_answer(q)).flatten()
            {
                bump(&c.rect_queries);
                return v;
            }
            early_parse = Some(parsed);
        }
        // Step 2: Explorer gets the message first, before any parse of ours, any picture read or model update.
        let relayed = self.relay.relay(raw, Method::Send);
        let verdict = match relayed {
            Relayed::Done(outcome) => {
                match outcome {
                    Outcome::Delivered(_) | Outcome::Posted => bump(&c.forwarded),
                    Outcome::TimedOut => bump(&c.forward_timed_out),
                    Outcome::TargetGone => bump(&c.forward_target_gone),
                }
                // (only an answer from Explorer is a verdict; a timeout or a vanished window says nothing about the request)
                match outcome {
                    Outcome::Delivered(v) => Some(v != 0),
                    _ => None,
                }
            }
            Relayed::NoTarget => {
                bump(&c.no_target);
                None
            }
        };
        // Step 3: our own handling. The sender is still waiting for our return, so its picture is still there.
        let own = self.guarded(|| self.own_step(early_parse.unwrap_or_else(|| parse_copydata(dw_data, bytes)), verdict)).flatten();
        match relayed {
            Relayed::Done(outcome) => reply_for(outcome),
            Relayed::NoTarget => isize::from(own.unwrap_or(false)),
        }
    }

    /// Runs `f`. In a build that unwinds, a panic is counted and logged and `None` comes back. In the release profile (`panic =
    /// "abort"`) this is just `Some(f())`: see the module docs.
    fn guarded<T>(&self, f: impl FnOnce() -> T) -> Option<T> {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(v) => Some(v),
            Err(_) => {
                bump(&self.shared.counters.handler_panics);
                log::error!("tray host: the handler panicked; the message was passed on before it ran");
                None
            }
        }
    }

    /// A message other than `WM_COPYDATA` that is meant for the real tray (`forward::method_for` said `Send` or `Post`).
    pub fn on_other(&self, raw: Raw, method: Method) -> isize {
        bump(&self.shared.counters.other_messages_passed_on);
        match self.relay.relay(raw, method) {
            Relayed::Done(o) => reply_for(o),
            Relayed::NoTarget => 0,
        }
    }

    /// `TaskbarCreated` reached our window: Explorer (re)created its taskbar. The model is kept (programs add their icons again and
    /// the model merges them); the relay looks for the new window at the next message.
    pub fn on_taskbar_created(&self) {
        self.relay.invalidate();
    }

    /// Explorer's tray window as of now, for the window code that mirrors its rectangle.
    pub fn explorer_tray(&self) -> Option<isize> {
        self.relay.target()
    }

    pub fn set_self(&self, hwnd: isize) {
        self.relay.set_self(hwnd);
    }

    /// A click on one of our pictures. Always from the caller's thread; see `clicks`.
    pub fn click(&self, req: &ClickRequest) -> ClickOutcome {
        if self.cfg.log_only {
            return ClickOutcome::UnknownIcon;
        }
        clicks::deliver(&self.shared, &*self.owners, req)
    }

    /// Drops the icons whose owners are gone. Returns how many.
    pub fn sweep(&self) -> usize {
        if self.cfg.log_only {
            return 0;
        }
        clicks::sweep(&self.shared, &*self.owners)
    }

    // ---------------------------------------------------------------- own handling

    /// Step 3. Returns the answer a tray would give (used only when there was nobody to pass the message to).
    fn own_step(&self, parsed: Result<TrayRequest, WireError>, verdict: Option<bool>) -> Option<bool> {
        let c = &self.shared.counters;
        match parsed {
            Err(WireError::UnknownKind(_)) => {
                bump(&c.unknown_kind);
                None
            }
            Err(_) => {
                bump(&c.unparsed);
                None
            }
            Ok(TrayRequest::Icon(req)) => {
                bump(&c.notify);
                if verdict == Some(false) {
                    bump(&c.explorer_refused);
                }
                Some(if self.cfg.log_only { self.count_step(&req) } else { self.model_step(&req, verdict) })
            }
            Ok(TrayRequest::IconRect(_)) => {
                // (not answered here: no packing is set, the icon is unknown to the interface, or the rectangle does not fit)
                bump(&c.rect_queries);
                None
            }
            Ok(TrayRequest::AppBar(req)) => {
                // The block passes through untouched (it was passed on before this). Two numbers say what became of the DOCK's own
                // toolbar messages, which land in this window while it is the first `Shell_TrayWnd`: how many came from our own
                // process, and how many registrations (`ABM_NEW`) Explorer answered with 0. `explorer_refused` counts icon requests
                // only and cannot show this. With Explorer's taskbar hidden by us the dock sends none (it does not register).
                bump(&c.appbar);
                if self.owners.owner_pid(req.hwnd_handle()) == Some(std::process::id()) {
                    bump(&c.appbar_own);
                }
                if req.message == AbMessage::New && verdict == Some(false) {
                    bump(&c.appbar_new_refused);
                }
                None
            }
            Ok(TrayRequest::ServiceObject(_)) => {
                bump(&c.service);
                None
            }
        }
    }

    fn count_nim(&self, nim: Nim, note: Option<ApplyNote>) {
        let c = &self.shared.counters;
        match (nim, note) {
            (_, Some(ApplyNote::Added)) | (Nim::Add, None) => bump(&c.adds),
            (_, Some(ApplyNote::ReAdded)) => bump(&c.re_adds),
            (_, Some(ApplyNote::Modified)) | (Nim::Modify, None) => bump(&c.modifies),
            (_, Some(ApplyNote::ModifyUnknown)) => bump(&c.modify_unknown),
            (_, Some(ApplyNote::Adopted)) => bump(&c.adopted),
            (_, Some(ApplyNote::AdoptedVersionGuessed)) => {
                bump(&c.adopted);
                bump(&c.adopted_version_guessed);
            }
            (_, Some(ApplyNote::Deleted)) | (Nim::Delete, None) => bump(&c.deletes),
            (_, Some(ApplyNote::DeleteUnknown)) => bump(&c.delete_unknown),
            (_, Some(ApplyNote::VersionSet)) | (Nim::SetVersion, None) => bump(&c.set_versions),
            (_, Some(ApplyNote::VersionRefused)) => bump(&c.version_refused),
            (_, Some(ApplyNote::Focus)) | (Nim::SetFocus, None) => bump(&c.set_focus),
            (_, Some(ApplyNote::Full)) => bump(&c.icons_full),
        }
    }

    /// LOG-ONLY: counts, and remembers the key of the icon, nothing else.
    fn count_step(&self, req: &NotifyIconRequest) -> bool {
        self.count_nim(req.message, None);
        if req.data.balloon().is_some_and(|b| !b.is_removal()) {
            bump(&self.shared.counters.balloons);
        }
        self.shared.note_key(req)
    }

    /// What Explorer said decides what the model keeps, because the model must not drift from the list Explorer holds:
    /// * `NIM_MODIFY` and `NIM_SETVERSION` that Explorer REFUSED are not applied (Explorer has no such icon, or refused the version);
    /// * `NIM_DELETE` is applied whatever Explorer said (the program no longer wants the icon; a ghost is worse than a missed one);
    /// * `NIM_ADD` is applied whatever Explorer said: Explorer refuses an add of an icon it already has, which is exactly what the
    ///   start broadcast makes programs send, and that is how the model fills. But a refused add of an icon the model already holds
    ///   keeps the icon's version (Explorer kept it too; see `TrayModel::apply`).
    ///
    /// With no verdict (nobody to ask, or no answer in time) every request is applied.
    fn model_step(&self, req: &NotifyIconRequest, verdict: Option<bool>) -> bool {
        if verdict == Some(false) && matches!(req.message, Nim::Modify | Nim::SetVersion) {
            return false;
        }
        let prep = self.prepare(req, verdict);
        let out = self.shared.apply(req, prep);
        self.count_nim(req.message, Some(out.note));
        if matches!(out.note, ApplyNote::ModifyUnknown | ApplyNote::DeleteUnknown | ApplyNote::VersionRefused) {
            self.say_unknown(req, verdict);
        }
        if out.events.iter().any(|e| matches!(e, super::model::TrayEvent::Balloon(_))) {
            bump(&self.shared.counters.balloons);
        }
        out.answer
    }

    /// One log line for a request about an icon the host never saw added, so that a run says WHOSE icons are missing (the counters
    /// only count them). It says kinds, never a name: who the owner is (Explorer, another program, unknown), whether a GUID came and
    /// whether it is one of the dock's own indicators, what Explorer answered and what the request carried. Only the first
    /// [`UNKNOWN_LINES`] of a run, and the owner is looked up only while lines are left.
    fn say_unknown(&self, req: &NotifyIconRequest, verdict: Option<bool>) {
        if self.unknown_lines_left.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_err() {
            return;
        }
        let d = &req.data;
        let owner_exe = self.owners.owner_pid(d.hwnd_handle()).and_then(|pid| self.owners.owner_exe(pid));
        let line = super::adopt::unknown_line(&super::adopt::UnknownFacts {
            message: req.message,
            owner: super::adopt::owner_kind(owner_exe.as_deref()),
            guid: super::adopt::guid_kind(match d.key() {
                IconKey::Guid(g) => Some(g),
                _ => None,
            }),
            verdict,
            callback: d.callback().is_some(),
            picture: d.icon_handle().is_some_and(|h| h != 0),
        });
        log::info!("tray host: {line}");
    }

    /// Reads, from the sender's process, what the model needs and the sender may take away: the pictures. Done before the model is
    /// locked, and (since Explorer has the message by now) while the sender is still waiting for our return.
    fn prepare(&self, req: &NotifyIconRequest, verdict: Option<bool>) -> Prepared {
        let mut prep = Prepared { now_ms: self.clock.now_ms(), explorer_accepted: verdict, ..Prepared::default() };
        if !matches!(req.message, Nim::Add | Nim::Modify) {
            return prep;
        }
        let d = &req.data;
        prep.image = match d.icon_handle() {
            None => ImageUpdate::Keep,
            Some(0) => ImageUpdate::Clear,
            Some(h) => match self.pixels.read(h) {
                Some(b) => ImageUpdate::Set(Arc::new(b)),
                None => {
                    bump(&self.shared.counters.pixel_failures);
                    ImageUpdate::Keep
                }
            },
        };
        if let Some(b) = d.balloon()
            && !b.is_removal()
            && b.kind == BalloonKind::User
            && let Some(h) = b.user_icon
        {
            prep.balloon_icon = self.pixels.read(h).map(Arc::new);
        }
        // (an add always; a modify only when it could become an add: the icon is not in the model and the request carries a callback and
        // a picture, see `adopt.rs`. A stream of updates for icons we hold costs no process lookup.)
        // (the cheap fields of the request first: the model's lock is taken only for a modify that has the pieces of an add)
        if req.message == Nim::Add || (super::adopt::could_become_add(req) && !self.shared.knows(req)) {
            let hwnd = d.hwnd_handle();
            prep.owner = self.owners.owner_pid(hwnd).map(|pid| (pid, self.owners.owner_exe(pid).unwrap_or_default()));
        }
        prep
    }

    /// The reply to an icon-rectangle query, when the interface knows the icon's place and the packing fits. `None`: Explorer answers.
    fn rect_answer(&self, q: &IconRectQuery) -> Option<isize> {
        let packing = self.cfg.rect_packing?;
        let part = q.part()?;
        let rect = {
            let st = self.shared.state.lock();
            st.model.find_query(&q.key()).and_then(|i| st.rects.get(&i.id).copied())
        };
        let v = pack_reply(packing, part, rect?)?;
        bump(&self.shared.counters.rect_answered);
        Some(v as isize)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fakes::{CallLog, FakeOwners, FakePixels, FakeRelay, ManualClock, new_log};
    use super::super::model::{IconId, test_util::*};
    use super::*;
    use crate::tray_wire::notify::{nif, tests_support::golden_full};
    use crate::tray_wire::{DW_APPBAR, DW_ICON_RECT, DW_NOTIFY_ICON, DW_SERVICE_OBJECT, Rect, WireGuid};

    struct Rig {
        core: Core,
        relay: Arc<FakeRelay>,
        pixels: FakePixels,
        owners: FakeOwners,
        log: CallLog,
    }

    fn rig(target: Option<isize>, cfg: CoreConfig) -> Rig {
        let log = new_log();
        let relay = Arc::new(FakeRelay::new(target, Outcome::Delivered(1)).with_log(log.clone()));
        let pixels = FakePixels::with_log(log.clone());
        let owners = FakeOwners::new();
        let (shared, _rx) = Shared::new();
        let core = Core::new(Arc::new(shared), cfg, Box::new(relay.clone()), Box::new(pixels.clone()), Arc::new(owners.clone()), Box::new(ManualClock::default()));
        Rig { core, relay, pixels, owners, log }
    }

    fn raw() -> Raw {
        Raw { msg: 0x4A, wparam: 0x77, lparam: 0xABCD }
    }

    /// A `dwData = 1` block built from a request.
    fn block(r: &NotifyIconRequest) -> Vec<u8> {
        r.encode()
    }

    fn icon_block(m: Nim, hwnd: u32, uid: u32, flags: u32, hicon: u32) -> Vec<u8> {
        let mut d = data(hwnd, uid, flags);
        d.icon = hicon;
        block(&req(m, d))
    }

    #[test]
    fn every_message_is_passed_on_untouched_and_explorers_answer_comes_back() {
        let r = rig(Some(200), CoreConfig::default());
        r.relay.set_outcome(Outcome::Delivered(42));
        let b = icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &b, raw()), 42);
        assert_eq!(r.relay.relayed(), vec![(raw(), Method::Send)], "the message as it came: same wParam, same lParam");
        // Explorer says no: the sender hears no
        r.relay.set_outcome(Outcome::Delivered(0));
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &b, raw()), 0);
        // Explorer does not answer
        r.relay.set_outcome(Outcome::TimedOut);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &b, raw()), 0);
        r.relay.set_outcome(Outcome::TargetGone);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &b, raw()), 0);
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.forwarded, c.forward_timed_out, c.forward_target_gone), (2, 1, 1));
    }

    #[test]
    fn appbar_service_object_and_unknown_kinds_are_passed_on_and_change_no_model() {
        let r = rig(Some(200), CoreConfig::default());
        for (dw, bytes) in [(DW_APPBAR, vec![0u8; 64]), (DW_SERVICE_OBJECT, vec![0u8; 20]), (7usize, vec![1u8; 10]), (DW_APPBAR, vec![])] {
            assert_eq!(r.core.on_copydata(dw, &bytes, raw()), 1);
        }
        assert_eq!(r.relay.relayed().len(), 4);
        assert_eq!(r.core.shared.generation(), 0);
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.appbar, c.service, c.unknown_kind, c.unparsed), (1, 1, 1, 1), "the empty appbar block is a parse failure");
        assert_eq!(c.messages, 4);
    }

    fn appbar_block(message: crate::tray_wire::appbar::AbMessage, hwnd: u32) -> Vec<u8> {
        use crate::tray_wire::appbar::{AbEdge, AppBarLayout, AppBarRequest};
        let _ = AbEdge::Bottom;
        AppBarRequest {
            layout: AppBarLayout::V3,
            abd_cb_size: 0x28,
            hwnd,
            callback_message: 0x8001,
            edge: 3,
            rect: Rect { left: 0, top: 1362, right: 2560, bottom: 1440 },
            lparam: 0,
            message,
            shared_memory: 0x1234,
            source_pid: 4242,
            trailing_len: 0,
        }
        .encode()
    }

    #[test]
    fn the_docks_own_toolbar_messages_are_passed_on_untouched_and_counted_by_what_became_of_them() {
        use crate::tray_wire::appbar::AbMessage;
        let r = rig(Some(200), CoreConfig::default());
        // the dock's window belongs to this very process; a program's window to another
        r.owners.add_window(0x77, std::process::id());
        r.owners.add_window(0x88, std::process::id() + 1);
        // Explorer accepts the registration and the positions
        r.relay.set_outcome(Outcome::Delivered(1));
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::New, 0x77), raw()), 1);
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::SetPos, 0x77), raw()), 1);
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::New, 0x88), raw()), 1);
        // Explorer refuses a registration: the sender hears 0, and the number says so (`explorer_refused` is for icons only)
        r.relay.set_outcome(Outcome::Delivered(0));
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::New, 0x77), raw()), 0);
        // a position that returns 0 is not a refused registration
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::QueryPos, 0x77), raw()), 0);
        // no answer at all (a timeout) says nothing about the registration
        r.relay.set_outcome(Outcome::TimedOut);
        assert_eq!(r.core.on_copydata(DW_APPBAR, &appbar_block(AbMessage::New, 0x77), raw()), 0);
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.appbar, c.appbar_own, c.appbar_new_refused, c.explorer_refused), (6, 5, 1, 0), "{c:?}");
        assert_eq!(r.relay.relayed().len(), 6, "every one of them went to Explorer's window");
    }

    #[test]
    fn a_block_that_does_not_parse_is_still_passed_on() {
        let r = rig(Some(200), CoreConfig::default());
        for bytes in [vec![], vec![1, 2, 3], vec![0u8; 964], golden_full()[..20].to_vec()] {
            assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &bytes, raw()), 1);
        }
        assert_eq!(r.relay.relayed().len(), 4);
        assert_eq!(r.core.shared.counters.snapshot().unparsed, 4);
        assert!(r.core.shared.snapshot().icons.is_empty());
    }

    #[test]
    fn explorer_has_the_message_before_any_pixel_or_owner_work_starts() {
        // Under `panic = "abort"` a bug in our own code ends the process, so nothing of ours may run before Explorer has the message.
        // The sender is still blocked in its SendMessage while we read the picture, so reading afterwards is safe.
        let r = rig(Some(200), CoreConfig::default());
        r.pixels.set(-256, 16, 16, 9); // 0xFFFFFF00 sign-extended
        let b = icon_block(Nim::Add, 1, 1, nif::MESSAGE | nif::ICON, 0xFFFF_FF00);
        r.core.on_copydata(DW_NOTIFY_ICON, &b, raw());
        let log = r.log.lock().clone();
        assert_eq!(log, vec!["relay".to_string(), "pixels 0xffffffffffffff00".to_string()], "Explorer first, then our own reading");
        let snap = r.core.shared.snapshot();
        assert_eq!(snap.icons.len(), 1);
        assert_eq!(snap.icons[0].image.as_ref().map(|i| (i.width, i.height)), Some((16, 16)));
    }

    #[test]
    fn nothing_of_ours_runs_before_explorer_even_for_a_block_that_does_not_parse() {
        // the parse itself is after the pass-on: a hook in the relay sees that no counter has moved yet beyond `messages`
        let r = rig(Some(200), CoreConfig::default());
        let seen = Arc::new(parking_lot::Mutex::new(None));
        let (s2, shared) = (seen.clone(), r.core.shared.clone());
        r.relay.set_hook(Arc::new(move || {
            let c = shared.counters.snapshot();
            *s2.lock() = Some((c.unparsed, c.notify, c.adds, c.unknown_kind));
        }));
        r.core.on_copydata(DW_NOTIFY_ICON, &[1, 2, 3], raw());
        assert_eq!(*seen.lock(), Some((0, 0, 0, 0)), "at the moment Explorer got the message nothing had been parsed or counted");
        assert_eq!(r.core.shared.counters.snapshot().unparsed, 1);
    }

    // ---- Explorer's verdict decides what the model keeps

    fn version_msg(version: u32) -> Vec<u8> {
        let mut v = data(1, 1, 0);
        v.timeout_or_version = version;
        block(&req(Nim::SetVersion, v))
    }

    #[test]
    fn a_refused_re_add_keeps_the_version_explorer_kept() {
        let r = rig(Some(200), CoreConfig::default());
        let add = icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0);
        r.core.on_copydata(DW_NOTIFY_ICON, &add, raw());
        r.core.on_copydata(DW_NOTIFY_ICON, &version_msg(4), raw());
        assert_eq!(r.core.shared.snapshot().icons[0].version.raw(), 4);
        // the start broadcast: the program adds again, Explorer (which has the icon) refuses
        r.relay.set_outcome(Outcome::Delivered(0));
        r.core.on_copydata(DW_NOTIFY_ICON, &add, raw());
        assert_eq!(r.core.shared.snapshot().icons[0].version.raw(), 4, "Explorer kept version 4, so does the model");
        // Explorer accepted the add (it had lost the icon): a new life, version 0
        r.relay.set_outcome(Outcome::Delivered(1));
        r.core.on_copydata(DW_NOTIFY_ICON, &add, raw());
        assert_eq!(r.core.shared.snapshot().icons[0].version.raw(), 0);
        assert_eq!(r.core.shared.counters.snapshot().explorer_refused, 1);
    }

    #[test]
    fn an_add_that_explorer_refused_still_fills_the_model() {
        // the point of the start broadcast: Explorer has the icon already and refuses the duplicate; the model must still learn it
        let r = rig(Some(200), CoreConfig::default());
        r.relay.set_outcome(Outcome::Delivered(0));
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        assert_eq!(r.core.shared.snapshot().icons.len(), 1);
    }

    #[test]
    fn a_modify_or_version_change_that_explorer_refused_is_not_applied_and_reads_no_pixels() {
        let r = rig(Some(200), CoreConfig::default());
        r.pixels.set(0x77, 16, 16, 1);
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        r.relay.set_outcome(Outcome::Delivered(0));
        let mut d = data(1, 1, nif::TIP | nif::ICON);
        d.tip = "changed".into();
        d.icon = 0x77;
        r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::Modify, d)), raw());
        r.core.on_copydata(DW_NOTIFY_ICON, &version_msg(4), raw());
        let i = &r.core.shared.snapshot().icons[0];
        assert_eq!((i.tip.as_str(), i.version.raw(), i.image.is_some()), ("", 0, false));
        assert!(r.pixels.reads().is_empty(), "no picture is read for a request that is not applied");
        assert_eq!(r.core.shared.counters.snapshot().explorer_refused, 2);
    }

    #[test]
    fn a_delete_is_applied_whatever_explorer_said() {
        let r = rig(Some(200), CoreConfig::default());
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        r.relay.set_outcome(Outcome::Delivered(0));
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Delete, 1, 1, 0, 0), raw());
        assert!(r.core.shared.snapshot().icons.is_empty(), "a ghost is worse than a missed icon");
    }

    #[test]
    fn a_timeout_or_a_vanished_window_is_no_verdict_and_the_request_is_applied() {
        for o in [Outcome::TimedOut, Outcome::TargetGone] {
            let r = rig(Some(200), CoreConfig::default());
            r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
            r.relay.set_outcome(o);
            let mut d = data(1, 1, nif::TIP);
            d.tip = "later".into();
            r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::Modify, d)), raw());
            assert_eq!(r.core.shared.snapshot().icons[0].tip, "later", "{o:?}");
        }
    }

    #[test]
    fn a_picture_that_cannot_be_read_is_counted_and_the_icon_is_kept() {
        let r = rig(Some(200), CoreConfig::default());
        let b = icon_block(Nim::Add, 1, 1, nif::MESSAGE | nif::ICON, 0x1234);
        r.core.on_copydata(DW_NOTIFY_ICON, &b, raw());
        assert_eq!(r.core.shared.counters.snapshot().pixel_failures, 1);
        assert_eq!(r.core.shared.snapshot().icons.len(), 1);
        assert!(r.core.shared.snapshot().icons[0].image.is_none());
        // a null handle clears, and nothing is read
        let before = r.pixels.reads().len();
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Modify, 1, 1, nif::ICON, 0), raw());
        assert_eq!(r.pixels.reads().len(), before);
    }

    #[test]
    fn the_owner_is_looked_up_at_add_only() {
        let r = rig(Some(200), CoreConfig::default());
        r.owners.add_window(1, 50);
        r.owners.set_exe(50, "tool.exe");
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        let i = &r.core.shared.snapshot().icons[0];
        assert_eq!((i.owner_pid, i.owner_exe.as_str()), (50, "tool.exe"));
        r.owners.remove_window(1);
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Modify, 1, 1, nif::TIP, 0), raw());
        assert_eq!(r.core.shared.snapshot().icons[0].owner_pid, 50, "a modify does not ask again");
    }

    fn late_modify(hwnd: u32, uid: u32) -> Vec<u8> {
        let mut d = data(hwnd, uid, nif::MESSAGE | nif::ICON | nif::TIP);
        d.icon = 0x77;
        d.tip = "late".into();
        block(&req(Nim::Modify, d))
    }

    #[test]
    fn a_full_modify_for_an_icon_the_host_never_saw_is_adopted_when_explorer_accepted_it() {
        let r = rig(Some(200), CoreConfig::default());
        r.pixels.set(0x77, 16, 16, 1);
        r.owners.add_window(9, 60);
        r.owners.set_exe(60, "late.exe");
        r.core.on_copydata(DW_NOTIFY_ICON, &late_modify(9, 4), raw());
        let snap = r.core.shared.snapshot();
        assert_eq!(snap.icons.len(), 1);
        assert_eq!((snap.icons[0].owner_pid, snap.icons[0].owner_exe.as_str()), (60, "late.exe"));
        assert!(snap.icons[0].image.is_some());
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.adopted, c.adopted_version_guessed, c.modify_unknown, c.modifies), (1, 1, 0, 0));
        assert!(c.summary().contains("adopted 1") && c.summary().contains("adopted_version_guessed 1"));
    }

    #[test]
    fn a_modify_that_explorer_refused_or_could_not_answer_is_not_adopted() {
        for outcome in [Outcome::Delivered(0), Outcome::TimedOut] {
            let r = rig(Some(200), CoreConfig::default());
            r.pixels.set(0x77, 16, 16, 1);
            r.owners.add_window(9, 60);
            r.relay.set_outcome(outcome);
            r.core.on_copydata(DW_NOTIFY_ICON, &late_modify(9, 4), raw());
            assert!(r.core.shared.snapshot().icons.is_empty(), "{outcome:?}");
        }
        // an owner that is not a window: nothing to click, nothing added (the counted note is the old one)
        let r = rig(Some(200), CoreConfig::default());
        r.pixels.set(0x77, 16, 16, 1);
        r.core.on_copydata(DW_NOTIFY_ICON, &late_modify(9, 4), raw());
        assert!(r.core.shared.snapshot().icons.is_empty());
        assert_eq!(r.core.shared.counters.snapshot().modify_unknown, 1);
        // no Explorer at all: the table says an unknown modify is false, and the model agrees
        let r = rig(None, CoreConfig::default());
        r.pixels.set(0x77, 16, 16, 1);
        r.owners.add_window(9, 60);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &late_modify(9, 4), raw()), 0);
        assert!(r.core.shared.snapshot().icons.is_empty());
    }

    #[test]
    fn a_version_the_program_set_before_it_was_adopted_is_kept() {
        let r = rig(Some(200), CoreConfig::default());
        r.pixels.set(0x77, 16, 16, 1);
        r.owners.add_window(9, 60);
        let mut v = data(9, 4, 0);
        v.timeout_or_version = 4;
        r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::SetVersion, v)), raw());
        r.core.on_copydata(DW_NOTIFY_ICON, &late_modify(9, 4), raw());
        assert_eq!(r.core.shared.snapshot().icons[0].version.raw(), 4);
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.adopted, c.adopted_version_guessed), (1, 0), "the version was seen, so nothing was guessed");
    }

    #[test]
    fn requests_about_unknown_icons_are_described_a_bounded_number_of_times() {
        let r = rig(Some(200), CoreConfig::default());
        assert_eq!(r.core.unknown_lines_left.load(Ordering::Relaxed), UNKNOWN_LINES);
        let del = |uid: u32| block(&req(Nim::Delete, data(9, uid, 0)));
        r.core.on_copydata(DW_NOTIFY_ICON, &del(1), raw());
        assert_eq!(r.core.unknown_lines_left.load(Ordering::Relaxed), UNKNOWN_LINES - 1, "an unknown delete is described once");
        for uid in 2..(UNKNOWN_LINES + 30) {
            r.core.on_copydata(DW_NOTIFY_ICON, &del(uid), raw());
        }
        assert_eq!(r.core.unknown_lines_left.load(Ordering::Relaxed), 0, "and then no more, however many come");
        assert_eq!(r.core.shared.counters.snapshot().delete_unknown as u32, UNKNOWN_LINES + 29, "the counter still counts every one");
        // an icon the host holds is not described when it is deleted
        let r = rig(Some(200), CoreConfig::default());
        r.owners.add_window(9, 60);
        r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::Add, data(9, 1, nif::MESSAGE))), raw());
        r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::Delete, data(9, 1, 0))), raw());
        assert_eq!(r.core.unknown_lines_left.load(Ordering::Relaxed), UNKNOWN_LINES);
    }

    #[test]
    fn with_no_tray_window_of_explorers_a_tray_answers_by_the_table() {
        let r = rig(None, CoreConfig::default());
        let add = icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &add, raw()), 1, "a new icon");
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &add, raw()), 0, "the same icon again");
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Modify, 1, 1, nif::TIP, 0), raw()), 1);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Modify, 9, 9, nif::TIP, 0), raw()), 0);
        let mut v = data(1, 1, 0);
        v.timeout_or_version = 4;
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::SetVersion, v.clone())), raw()), 1);
        v.timeout_or_version = 2;
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::SetVersion, v)), raw()), 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::SetFocus, 1, 1, 0, 0), raw()), 1);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Delete, 9, 9, 0, 0), raw()), 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Delete, 1, 1, 0, 0), raw()), 1);
        // everything that is not an icon: 0
        assert_eq!(r.core.on_copydata(DW_APPBAR, &[0; 64], raw()), 0);
        assert_eq!(r.core.on_copydata(DW_SERVICE_OBJECT, &[0; 20], raw()), 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &[1, 2, 3], raw()), 0);
        assert_eq!(r.core.on_other(raw(), Method::Send), 0);
        assert!(r.core.shared.counters.snapshot().no_target > 0);
    }

    #[test]
    fn explorer_that_comes_back_is_forwarded_to_again() {
        let r = rig(None, CoreConfig::default());
        let add = icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &add, raw()), 1);
        r.relay.set_target(Some(300));
        r.relay.set_outcome(Outcome::Delivered(5));
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Modify, 1, 1, nif::TIP, 0), raw()), 5);
    }

    #[test]
    fn taskbar_created_makes_the_relay_look_again_and_keeps_the_model() {
        let r = rig(Some(200), CoreConfig::default());
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        r.core.on_taskbar_created();
        assert_eq!(r.relay.invalidations(), 1);
        assert_eq!(r.core.shared.snapshot().icons.len(), 1);
        // the programs add again: merged, not doubled
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        assert_eq!(r.core.shared.snapshot().icons.len(), 1);
        assert_eq!(r.core.shared.counters.snapshot().re_adds, 1);
    }

    #[test]
    fn a_panic_in_the_own_handling_does_not_keep_the_message_from_explorer() {
        struct Boom;
        impl IconPixels for Boom {
            fn read(&self, _: isize) -> Option<super::super::model::IconBitmap> {
                panic!("a bug in the reader");
            }
        }
        let log = new_log();
        let relay = Arc::new(FakeRelay::new(Some(200), Outcome::Delivered(7)).with_log(log.clone()));
        let (shared, _rx) = Shared::new();
        let core = Core::new(Arc::new(shared), CoreConfig::default(), Box::new(relay.clone()), Box::new(Boom), Arc::new(FakeOwners::new()), Box::new(ManualClock::default()));
        let b = icon_block(Nim::Add, 1, 1, nif::MESSAGE | nif::ICON, 0x55);
        assert_eq!(core.on_copydata(DW_NOTIFY_ICON, &b, raw()), 7);
        assert_eq!(relay.relayed().len(), 1);
        assert_eq!(core.shared.counters.snapshot().handler_panics, 1);
        // and the host goes on working
        assert_eq!(core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 2, 1, nif::MESSAGE, 0), raw()), 7);
    }

    #[test]
    fn another_message_arriving_while_one_waits_for_explorer_is_handled_not_deadlocked() {
        // Windows delivers a second sender's WM_COPYDATA to this thread while it is inside SendMessageTimeout: the same Core is
        // entered again from within the relay
        let r = rig(Some(200), CoreConfig::default());
        let core = Arc::new(r.core);
        let inner = core.clone();
        let nested_answer = Arc::new(parking_lot::Mutex::new(None));
        let na = nested_answer.clone();
        r.relay.set_hook(Arc::new(move || {
            // (only the first entry triggers the nested one)
            if na.lock().is_none() {
                *na.lock() = Some(0);
                let v = inner.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 2, 2, nif::MESSAGE, 0), raw());
                *na.lock() = Some(v);
            }
        }));
        let v = core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0), raw());
        assert_eq!(v, 1);
        assert_eq!(*nested_answer.lock(), Some(1));
        assert_eq!(core.shared.snapshot().icons.len(), 2);
        assert_eq!(r.relay.relayed().len(), 2);
    }

    #[test]
    fn the_256_icon_bound_holds_and_the_message_still_goes_to_explorer() {
        let r = rig(Some(200), CoreConfig::default());
        for i in 0..300u32 {
            r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 1000 + i, 1, nif::MESSAGE, 0), raw());
        }
        assert_eq!(r.core.shared.snapshot().icons.len(), 256);
        assert_eq!(r.relay.relayed().len(), 300);
        assert_eq!(r.core.shared.counters.snapshot().icons_full, 44);
    }

    // ---- rectangles

    fn rect_query(hwnd: u32, uid: u32, message: i32) -> Vec<u8> {
        let q = IconRectQuery { magic: 0x3475_3423, message, cb_size: 40, hwnd, uid, guid: WireGuid::default(), trailing_len: 0 };
        q.encode()
    }

    fn with_icon_at(cfg: CoreConfig) -> Rig {
        let r = rig(Some(200), cfg);
        r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Add, 0x10, 3, nif::MESSAGE, 0), raw());
        let id = r.core.shared.snapshot().icons[0].id;
        r.core.shared.set_icon_rects(&[(id, Rect { left: 2319, top: 1392, right: 2335, bottom: 1424 })]);
        r
    }

    #[test]
    fn a_rectangle_query_is_answered_from_the_interfaces_rectangle_once_the_packing_is_known() {
        let r = with_icon_at(CoreConfig { rect_packing: Some(RectPacking::Corners), ..Default::default() });
        let before = r.relay.relayed().len();
        let first = r.core.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 1), raw());
        let second = r.core.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 2), raw());
        assert_eq!(r.relay.relayed().len(), before, "answered here, not passed on");
        let rc = crate::tray_wire::rect::unpack_replies(RectPacking::Corners, first as u32, second as u32);
        assert_eq!(rc, Rect { left: 2319, top: 1392, right: 2335, bottom: 1424 });
        assert_eq!(r.core.shared.counters.snapshot().rect_answered, 2);
    }

    #[test]
    fn without_a_known_packing_or_a_known_place_the_query_goes_to_explorer() {
        let r = with_icon_at(CoreConfig::default());
        let before = r.relay.relayed().len();
        r.core.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 1), raw());
        assert_eq!(r.relay.relayed().len(), before + 1, "packing unknown: Explorer answers");
        let r = with_icon_at(CoreConfig { rect_packing: Some(RectPacking::Axes), ..Default::default() });
        let before = r.relay.relayed().len();
        r.core.on_copydata(DW_ICON_RECT, &rect_query(0x99, 3, 1), raw());
        r.core.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 7), raw());
        assert_eq!(r.relay.relayed().len(), before + 2, "an unknown icon and an unknown part are Explorer's");
        // an icon with no rectangle reported
        let (shared, _rx) = Shared::new();
        let r2 = Core::new(Arc::new(shared), CoreConfig { rect_packing: Some(RectPacking::Axes), ..Default::default() }, Box::new(FakeRelay::new(None, Outcome::Posted)), Box::new(FakePixels::new()), Arc::new(FakeOwners::new()), Box::new(ManualClock::default()));
        assert_eq!(r2.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 1), raw()), 0, "no icon, nobody else: 0");
    }

    #[test]
    fn a_rectangle_beyond_sixteen_bits_is_left_to_explorer() {
        let r = with_icon_at(CoreConfig { rect_packing: Some(RectPacking::Axes), ..Default::default() });
        let id = r.core.shared.snapshot().icons[0].id;
        r.core.shared.set_icon_rects(&[(id, Rect { left: 40_000, top: 0, right: 40_016, bottom: 32 })]);
        let before = r.relay.relayed().len();
        r.core.on_copydata(DW_ICON_RECT, &rect_query(0x10, 3, 1), raw());
        assert_eq!(r.relay.relayed().len(), before + 1);
    }

    // ---- log-only

    #[test]
    fn log_only_reads_no_pixels_keeps_no_text_and_still_forwards() {
        let r = rig(Some(200), CoreConfig { log_only: true, rect_packing: Some(RectPacking::Axes) });
        r.pixels.set(0x77, 16, 16, 1);
        let mut d = data(1, 1, nif::MESSAGE | nif::ICON | nif::TIP | nif::INFO);
        d.icon = 0x77;
        d.tip = "a secret tooltip".into();
        d.info = "a secret balloon".into();
        d.info_title = "t".into();
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &block(&req(Nim::Add, d)), raw()), 1);
        assert!(r.pixels.reads().is_empty(), "no HICON is read");
        assert!(r.core.shared.snapshot().icons.is_empty(), "the model stays empty");
        assert_eq!(r.core.shared.icon_count(true), 1, "but the icons are counted");
        assert_eq!(r.relay.relayed().len(), 1);
        let c = r.core.shared.counters.snapshot();
        assert_eq!((c.adds, c.balloons), (1, 1));
        // no text anywhere in what could be logged
        assert!(!format!("{c:?}").contains("secret"));
        assert!(!c.summary().contains("secret"));
        // rectangle queries are not answered, clicks are off, the sweep does nothing
        r.core.on_copydata(DW_ICON_RECT, &rect_query(1, 1, 1), raw());
        assert_eq!(r.relay.relayed().len(), 2);
        assert_eq!(r.core.click(&ClickRequest { id: IconId(1), event: crate::tray_wire::click::HostEvent::LeftClick, cursor: Default::default(), icon_top_left: Default::default(), opts: Default::default() }), ClickOutcome::UnknownIcon);
        assert_eq!(r.core.sweep(), 0);
    }

    #[test]
    fn log_only_with_no_tray_window_answers_by_the_keys_seen() {
        let r = rig(None, CoreConfig { log_only: true, rect_packing: None });
        let add = icon_block(Nim::Add, 1, 1, nif::MESSAGE, 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &add, raw()), 1);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &add, raw()), 0);
        assert_eq!(r.core.on_copydata(DW_NOTIFY_ICON, &icon_block(Nim::Delete, 1, 1, 0, 0), raw()), 1);
        assert_eq!(r.core.shared.icon_count(true), 0);
    }

    // ---- the fuzz: nothing a program sends can keep a message from Explorer

    #[test]
    fn random_blocks_are_always_passed_on_exactly_once() {
        let mut rng = crate::tray_wire::XorShift(0xC0FF_EE00_1234_5678);
        let valid = golden_full();
        let r = rig(Some(200), CoreConfig::default());
        let mut sent = 0usize;
        for round in 0..6000u32 {
            let dw = [DW_NOTIFY_ICON, DW_NOTIFY_ICON, DW_APPBAR, DW_SERVICE_OBJECT, 9][rng.below(5) as usize];
            let bytes = match round % 3 {
                0 => {
                    let n = rng.below(1200) as usize;
                    rng.bytes(n)
                }
                1 => {
                    let mut v = valid.clone();
                    for _ in 0..1 + rng.below(6) {
                        let i = rng.below(v.len() as u64) as usize;
                        v[i] = rng.next() as u8;
                    }
                    v
                }
                _ => {
                    let n = rng.below(valid.len() as u64 + 1) as usize;
                    valid[..n].to_vec()
                }
            };
            r.core.on_copydata(dw, &bytes, raw());
            sent += 1;
            assert_eq!(r.relay.relayed().len(), sent, "round {round}: one pass-on per message");
        }
        assert!(r.core.shared.snapshot().icons.len() <= 256);
        assert_eq!(r.core.shared.counters.snapshot().handler_panics, 0);
    }
}
