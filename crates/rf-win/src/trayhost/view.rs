//! What the interface holds: a cheap, cloneable view of the tray host's model, the click engine and the rectangle book. It has no
//! thread and no window of its own; it is built over the host's `Shared` state (and over fakes in tests).

use std::sync::Arc;

use super::clicks::{self, ClickOutcome, ClickRequest, OwnerRef};
use super::model::{IconId, Snapshot};
use super::shared::{Counts, Shared};
use super::traits::OwnerSend;
use crate::tray_wire::Rect;

#[derive(Clone)]
pub struct TrayView {
    shared: Arc<Shared>,
    owners: Arc<dyn OwnerSend>,
    log_only: bool,
}

impl TrayView {
    pub fn new(shared: Arc<Shared>, owners: Arc<dyn OwnerSend>, log_only: bool) -> TrayView {
        TrayView { shared, owners, log_only }
    }

    /// LOG-ONLY mode: the host keeps counts only; there is no model to read and nothing to click.
    pub fn log_only(&self) -> bool {
        self.log_only
    }

    /// The model's generation, without the lock: compare with the last one drawn.
    pub fn generation(&self) -> u64 {
        self.shared.generation()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.snapshot()
    }

    /// The snapshot if the model changed since `last`, else `None`.
    pub fn snapshot_if_changed(&self, last: u64) -> Option<Snapshot> {
        self.shared.snapshot_if_changed(last)
    }

    /// How many times the host thread has ticked (every 250 ms); for `stall::StallWatch`.
    pub fn heartbeat(&self) -> u64 {
        self.shared.heartbeat()
    }

    pub fn counts(&self) -> Counts {
        self.shared.counters.snapshot()
    }

    /// Icons in the model (in log-only mode: icons seen).
    pub fn icon_count(&self) -> usize {
        self.shared.icon_count(self.log_only)
    }

    /// Tells the host where each icon is on the screen now (screen coordinates), so it can answer `Shell_NotifyIconGetRect`.
    pub fn set_icon_rects(&self, rects: &[(IconId, Rect)]) {
        self.shared.set_icon_rects(rects);
    }

    /// A click, hover or key on one of our pictures. Call it from the thread that received the input (it hands the foreground to the
    /// icon's owner first, which only that thread may do).
    pub fn click(&self, req: &ClickRequest) -> ClickOutcome {
        if self.log_only {
            return ClickOutcome::UnknownIcon;
        }
        clicks::deliver(&self.shared, &*self.owners, req)
    }

    /// [`TrayView::click`] on a picture drawn when the icon had owner `expect`: refused (`OwnerChanged`) when the model's icon has been
    /// taken over by another window or process since.
    pub fn click_expecting(&self, req: &ClickRequest, expect: OwnerRef) -> ClickOutcome {
        if self.log_only {
            return ClickOutcome::UnknownIcon;
        }
        clicks::deliver_expecting(&self.shared, &*self.owners, req, Some(expect))
    }

    /// Drops the icons whose owner windows are gone (the host does this every 5 s; the interface may also ask at once).
    pub fn sweep(&self) -> usize {
        if self.log_only {
            return 0;
        }
        clicks::sweep(&self.shared, &*self.owners)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fakes::FakeOwners;
    use super::super::model::{Prepared, test_util::*};
    use super::*;
    use crate::tray_wire::Point;
    use crate::tray_wire::click::HostEvent;
    use crate::tray_wire::notify::{Nim, nif};

    #[test]
    fn the_view_reads_the_model_and_clicks_through_the_owners() {
        let (s, _rx) = Shared::new();
        let s = Arc::new(s);
        let owners = FakeOwners::new();
        owners.add_window(0x40, 9);
        let v = TrayView::new(s.clone(), Arc::new(owners.clone()), false);
        assert_eq!(v.generation(), 0);
        assert!(v.snapshot_if_changed(0).is_none());
        s.apply(&req(Nim::Add, data(0x40, 1, nif::MESSAGE)), Prepared { owner: Some((9, "x.exe".into())), ..Default::default() });
        let snap = v.snapshot_if_changed(0).expect("changed");
        assert_eq!(snap.icons.len(), 1);
        assert_eq!(v.icon_count(), 1);
        let id = snap.icons[0].id;
        let out = v.click(&ClickRequest { id, event: HostEvent::LeftClick, cursor: Point::default(), icon_top_left: Point::default(), opts: Default::default() });
        assert_eq!(out, ClickOutcome::Sent(2));
        owners.remove_window(0x40);
        assert_eq!(v.sweep(), 1);
        assert_eq!(v.icon_count(), 0);
        assert_eq!(v.counts().dead_dropped, 1);
    }

    #[test]
    fn a_log_only_view_has_nothing_to_click_or_sweep() {
        let (s, _rx) = Shared::new();
        let v = TrayView::new(Arc::new(s), Arc::new(FakeOwners::new()), true);
        assert!(v.log_only());
        assert_eq!(v.sweep(), 0);
        assert_eq!(v.click(&ClickRequest { id: IconId(1), event: HostEvent::LeftClick, cursor: Point::default(), icon_top_left: Point::default(), opts: Default::default() }), ClickOutcome::UnknownIcon);
    }
}
