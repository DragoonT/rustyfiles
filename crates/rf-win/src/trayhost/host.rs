//! The tray host as the shell host process uses it: [`TrayHost::start`] makes the thread with the window (see `win.rs`), [`TrayHost`]
//! hands out a [`TrayView`] and the event hints, and [`TrayHost::stop`] / `Drop` end it in the order the lifecycle guarantees (window
//! destroyed first, then the `TaskbarCreated` broadcast).
//!
//! Nothing here runs unless the caller starts it, and the caller (rf-app `shell::trayhost`) does so only while the setting
//! `shell_tray_host` is on, in the shell host process.

use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crossbeam_channel::Receiver;

use super::handshake::Handshake;
use super::model::TrayEvent;
use super::shared::Shared;
use super::traits::OwnerSend;
use super::view::TrayView;
use super::win;
use crate::tray_wire::rect::RectPacking;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrayHostConfig {
    /// Log-only: the window exists and everything is passed on, but pictures are not read, no text is kept, nothing is clickable,
    /// and the interface gets counts only.
    pub log_only: bool,
    /// How `Shell_NotifyIconGetRect` is answered. `None`: not at all (Explorer answers). UNVERIFIED either way; see `docs/design/tray-host.md`.
    pub rect_packing: Option<RectPacking>,
    /// Ask programs to add their icons again when the host starts (they come to the model through the window). On by default; the
    /// broadcast at stop is always sent.
    pub broadcast_on_start: bool,
    /// Make the window visible (click-through, almost fully transparent) instead of hidden. A hidden window is what the research
    /// note's other implementations use by default; this is for the day a program turns out to look at the window's visibility.
    pub visible: bool,
}

impl Default for TrayHostConfig {
    fn default() -> Self {
        TrayHostConfig { log_only: false, rect_packing: None, broadcast_on_start: true, visible: false }
    }
}

/// How long `stop` waits for the thread to finish.
const STOP_WAIT: Duration = Duration::from_secs(3);

pub struct TrayHost {
    view: TrayView,
    events: Receiver<TrayEvent>,
    handshake: Arc<Handshake>,
    done: Receiver<()>,
    thread: Option<JoinHandle<()>>,
}

impl TrayHost {
    /// Creates the window of class `Shell_TrayWnd` and starts passing messages on. NOT RUN by any test.
    pub fn start(cfg: TrayHostConfig) -> Result<TrayHost, String> {
        let (shared, events) = Shared::new();
        let shared = Arc::new(shared);
        let handshake = Arc::new(Handshake::new());
        let (thread, done) = win::spawn(&cfg, shared.clone(), handshake.clone())?;
        let owners: Arc<dyn OwnerSend> = Arc::new(win::Win32Owners);
        Ok(TrayHost { view: TrayView::new(shared, owners, cfg.log_only), events, handshake, done, thread: Some(thread) })
    }

    pub fn view(&self) -> TrayView {
        self.view.clone()
    }

    /// The event hints (the model and its generation are the truth; a full queue drops hints).
    pub fn events(&self) -> &Receiver<TrayEvent> {
        &self.events
    }

    /// Destroys the window FIRST, then asks programs to add their icons again. Waits for the thread to end, at most 3 s. Safe to
    /// call twice.
    pub fn stop(&mut self) {
        let Some(thread) = self.thread.take() else { return };
        win::request_close(self.handshake.hwnd());
        match self.done.recv_timeout(STOP_WAIT) {
            Ok(()) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                // The thread is stuck (waiting for Explorer, most likely, for up to the forward limit). A window can only be destroyed by
                // the thread that owns it, so nothing else can remove ours: it goes when the thread gets back to its loop, or with the
                // process. Say so plainly.
                log::error!("tray host: the thread did not end within {} s; its window stays until it does or the process ends", STOP_WAIT.as_secs());
            }
        }
    }
}

impl Drop for TrayHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_configuration_is_the_careful_one() {
        let c = TrayHostConfig::default();
        assert!(!c.log_only && !c.visible);
        assert_eq!(c.rect_packing, None, "rectangle queries stay Explorer's until the packing is known");
        assert!(c.broadcast_on_start);
    }
}
