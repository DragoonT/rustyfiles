//! A tray host's model, view and event queue with NO window and NO thread behind them, for the tests of whoever reads the view (the
//! interface's tray feed). Icons are added the way the host's handler adds them (`Shared::apply` with what a program would have
//! sent), owners are the stand-ins of `fakes.rs`, so a click reaches a recorder and nothing else. Nothing here touches Windows.

use std::sync::Arc;

use crossbeam_channel::Receiver;

use super::fakes::FakeOwners;
use super::model::{IconBitmap, ImageUpdate, Prepared, TrayEvent};
use super::shared::Shared;
use super::view::TrayView;
use crate::tray_wire::notify::{DataTier, Nim, NotifyIconData, NotifyIconRequest, nif, nis};
use crate::tray_wire::{IconVersion, WireGuid};

/// How a test names an icon after it was added: the window and id the program used, and the GUID when it gave one.
#[derive(Clone, Copy, Debug)]
pub struct IconRef {
    pub hwnd: u32,
    pub uid: u32,
    pub guid: Option<WireGuid>,
}

/// What a program sends with `NIM_ADD`.
#[derive(Clone, Debug)]
pub struct IconSpec {
    pub hwnd: u32,
    pub uid: u32,
    /// The owner's process.
    pub pid: u32,
    /// The owner's program file name.
    pub exe: String,
    pub tip: String,
    pub guid: Option<WireGuid>,
    /// `(width, height, bgra)` of the picture, straight alpha.
    pub picture: Option<(u32, u32, Vec<u8>)>,
    pub version: IconVersion,
}

impl IconSpec {
    pub fn new(hwnd: u32, uid: u32, pid: u32, exe: &str, tip: &str) -> IconSpec {
        IconSpec { hwnd, uid, pid, exe: exe.to_string(), tip: tip.to_string(), guid: None, picture: None, version: IconVersion::V0 }
    }

    pub fn as_ref(&self) -> IconRef {
        IconRef { hwnd: self.hwnd, uid: self.uid, guid: self.guid }
    }
}

pub struct FakeHost {
    pub shared: Arc<Shared>,
    pub owners: FakeOwners,
    pub view: TrayView,
    pub events: Receiver<TrayEvent>,
}

fn data(r: IconRef, flags: u32) -> NotifyIconData {
    let (flags, guid) = match r.guid {
        Some(g) => (flags | nif::GUID, g),
        None => (flags, WireGuid::default()),
    };
    NotifyIconData {
        cb_size: 956,
        hwnd: r.hwnd,
        uid: r.uid,
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
        guid,
        balloon_icon: 0,
        tier: DataTier::Full,
    }
}

fn request(m: Nim, d: NotifyIconData) -> NotifyIconRequest {
    NotifyIconRequest { message: m, data: d, trailing_len: 0, trailing_word: None }
}

fn bitmap(p: &(u32, u32, Vec<u8>)) -> Arc<IconBitmap> {
    Arc::new(IconBitmap::new(p.0, p.1, p.2.clone()).expect("a test picture within bounds"))
}

impl FakeHost {
    pub fn new(log_only: bool) -> FakeHost {
        let (shared, events) = Shared::new();
        let shared = Arc::new(shared);
        let owners = FakeOwners::new();
        let view = TrayView::new(shared.clone(), Arc::new(owners.clone()), log_only);
        FakeHost { shared, owners, view, events }
    }

    /// A program adds an icon (and its owner window exists, in its process).
    pub fn add(&self, spec: &IconSpec) {
        self.owners.add_window(spec.hwnd as isize, spec.pid);
        let mut flags = nif::MESSAGE | nif::TIP;
        if spec.picture.is_some() {
            flags |= nif::ICON;
        }
        let mut d = data(spec.as_ref(), flags);
        d.tip = spec.tip.clone();
        let image = spec.picture.as_ref().map_or(ImageUpdate::Keep, |p| ImageUpdate::Set(bitmap(p)));
        self.shared.apply(&request(Nim::Add, d), Prepared { image, owner: Some((spec.pid, spec.exe.clone())), ..Default::default() });
        if spec.version != IconVersion::V0 {
            let mut v = data(spec.as_ref(), 0);
            v.timeout_or_version = spec.version.raw();
            self.shared.apply(&request(Nim::SetVersion, v), Prepared::default());
        }
    }

    pub fn set_tip(&self, r: IconRef, tip: &str) {
        let mut d = data(r, nif::TIP);
        d.tip = tip.to_string();
        self.shared.apply(&request(Nim::Modify, d), Prepared::default());
    }

    pub fn set_picture(&self, r: IconRef, picture: &(u32, u32, Vec<u8>)) {
        self.shared.apply(&request(Nim::Modify, data(r, nif::ICON)), Prepared { image: ImageUpdate::Set(bitmap(picture)), ..Default::default() });
    }

    /// A balloon: the model stores it and publishes an event, but nothing the strip draws changes.
    pub fn balloon(&self, r: IconRef) {
        let mut d = data(r, nif::INFO);
        d.info = "x".into();
        d.info_title = "y".into();
        self.shared.apply(&request(Nim::Modify, d), Prepared::default());
    }

    /// `NIS_HIDDEN` on or off.
    pub fn set_hidden(&self, r: IconRef, hidden: bool) {
        let mut d = data(r, nif::STATE);
        d.state = if hidden { nis::HIDDEN } else { 0 };
        d.state_mask = nis::HIDDEN;
        self.shared.apply(&request(Nim::Modify, d), Prepared::default());
    }

    pub fn delete(&self, r: IconRef) {
        self.shared.apply(&request(Nim::Delete, data(r, 0)), Prepared::default());
    }

    /// The rectangles the interface has reported, as `(left, top, right, bottom)` (the host keeps those of icons it holds only).
    pub fn rects(&self) -> Vec<(i32, i32, i32, i32)> {
        self.shared.state.lock().rects.values().map(|r| (r.left, r.top, r.right, r.bottom)).collect()
    }

    /// `(window, message, wParam, lParam)` of every message a click sent to an owner.
    pub fn sent(&self) -> Vec<(isize, u32, usize, isize)> {
        self.owners.sent()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_added_icon_is_in_the_snapshot_with_its_owner_and_picture() {
        let h = FakeHost::new(false);
        let mut spec = IconSpec::new(0x10, 3, 77, "mail.exe", "Posta");
        spec.picture = Some((2, 1, vec![1, 2, 3, 4, 5, 6, 7, 8]));
        h.add(&spec);
        let snap = h.view.snapshot();
        assert_eq!(snap.icons.len(), 1);
        let i = &snap.icons[0];
        assert_eq!((i.hwnd, i.uid, i.owner_pid, i.owner_exe.as_str(), i.tip.as_str()), (0x10, 3, 77, "mail.exe", "Posta"));
        assert_eq!(i.image.as_ref().map(|b| (b.width, b.height)), Some((2, 1)));
        assert!(matches!(h.events.try_recv(), Ok(TrayEvent::Added(_))));
    }

    #[test]
    fn a_guid_icon_is_keyed_by_its_guid_and_can_be_hidden_and_deleted() {
        let h = FakeHost::new(false);
        let mut spec = IconSpec::new(0x10, 3, 77, "mail.exe", "Posta");
        spec.guid = Some(WireGuid::from_parts(1, 2, 3, [4; 8]));
        h.add(&spec);
        h.set_hidden(spec.as_ref(), true);
        assert!(h.view.snapshot().icons[0].hidden());
        h.set_hidden(spec.as_ref(), false);
        assert!(!h.view.snapshot().icons[0].hidden());
        h.delete(spec.as_ref());
        assert!(h.view.snapshot().icons.is_empty());
    }
}
