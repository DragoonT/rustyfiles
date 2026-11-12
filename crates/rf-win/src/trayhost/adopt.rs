//! When a `NIM_MODIFY` for an icon the host never saw added may be taken as the add the host missed. Pure.
//!
//! THE GAP. The host knows only the icons that were added after it started, and those of programs that add theirs again when a taskbar
//! announces itself (`TaskbarCreated`, the start broadcast). Any other icon is in Explorer's list and not in the model: a program that
//! does not listen for the announcement (or whose hidden window cannot receive a broadcast), and an icon that Explorer's own parts add.
//! The first real run counted `modify_unknown 1` and `delete_unknown 6`: requests for icons the model had never held.
//!
//! WHAT A TASKBAR MAY ASK OF THE RUNNING PROGRAMS, and what it cannot have:
//! * it may broadcast `TaskbarCreated` (done at the host's start; repeating it is left out on purpose, see `docs/design/tray-host.md`);
//! * it may wait for the program to speak: every `Shell_NotifyIcon` call of a program reaches the first `Shell_TrayWnd`, i.e. us. A
//!   program that updates its icon (a new picture, a new tip) tells us what it needs, IF the call carries enough (this file);
//! * it cannot ask a program for its icons, and it cannot ask Explorer: there is no query for "the notification icons" (a supported
//!   object model does not exist; the legacy toolbar of Windows 10 is gone on this build). The only complete source is Explorer's own
//!   memory, which this program does not read. So an icon whose program is silent is unknown to the host until it speaks.
//!
//! WHEN A MODIFY IS AN ADD. All of these must hold (each is a field of the request or of what the handler read for it):
//! 1. Explorer ACCEPTED the same request. An icon Explorer does not have cannot be modified there: Explorer's refusal means the icon does
//!    not exist anywhere, and taking it as an add would invent a ghost. With no answer from Explorer (none running, or no reply in time)
//!    nothing is adopted: there is no proof the icon exists.
//! 2. The request names a callback message (`NIF_MESSAGE`): without it nothing can be sent to the owner, so there is nothing to click.
//! 3. The request carries a picture that was read (`NIF_ICON` with a handle that gave pixels): a cell with no picture is useless, and
//!    an icon that is only to be shown with a picture the host does not have would be a blank square.
//! 4. The owner window is a live window of a known process (the handler looked it up): the click engine drops an icon whose owner is
//!    gone or belongs to another process, so an owner-less icon would only be dropped again.
//! 5. The request does not state the icon hidden (`NIS_HIDDEN`): a program that hides its icon asked to be left out.
//!    KNOWN LIMIT of 5: only the CURRENT request can be read. A program that hid its icon before the host started, and keeps
//!    sending full modifies without `NIF_STATE`, has its icon adopted and shown although Explorer holds it hidden: the hidden bit
//!    lives in Explorer's memory like the version. There is no way to ask for it; the user can put such an icon away in the panel,
//!    and nothing is invented (Explorer accepted the request, so the icon exists).
//!
//! A modify with only a new tip or only a new picture (the common animation) fails 2 or 3 and stays ignored, as before. What an adopted
//! icon does not carry is its VERSION (Explorer holds it; nothing can ask for it): the model uses the version it saw in an earlier
//! `NIM_SETVERSION` for the same icon (`TrayModel::note_version`), else 0. A version 3 or 4 program then gets the packing of version 0
//! in its callbacks until it sends the version again (a program that re-adds does). That is the one thing adoption can get wrong, and
//! it is counted: `adopted` for every adoption, `adopted_version_guessed` for those that started at version 0 with no version seen.
//!
//! WHY VERSION 0 IS STILL THE GUESS (and not "no clicks until the version is known", which would show an icon that does nothing).
//! A version 0 packing (`wParam` = icon id, `lParam` = the mouse message) reaches a version 3 or 4 program with the mouse message in
//! the low word of `lParam`, where it reads the event: the click lands, only the anchor point in `wParam` is wrong (the program puts
//! its menu at the wrong place) and a program that also compares the icon id in the high word does not recognise it. The opposite
//! guess, version 4 packing sent to a version 0 program (the kind that never listens for the taskbar announcement, i.e. the likelier
//! owner of a missed icon), is read as no known message at all. So the wrong guess costs a misplaced menu at worst, and the other
//! costs the click.

use super::model::{ImageUpdate, Prepared};
use crate::tray_wire::notify::{Nim, NotifyIconRequest};

/// Why a modify is not an add. Only for tests and counters: nothing else reads the reason.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    NotAModify,
    ExplorerDidNotAccept,
    NoCallback,
    NoPicture,
    NoOwner,
    AskedToBeHidden,
}

/// Whether `req` (a request for an icon the model does not hold) may be taken as an add, given what the handler read for it.
pub fn modify_as_add(req: &NotifyIconRequest, prep: &Prepared) -> Result<(), Why> {
    if req.message != Nim::Modify {
        return Err(Why::NotAModify);
    }
    if prep.explorer_accepted != Some(true) {
        return Err(Why::ExplorerDidNotAccept);
    }
    if req.data.callback().is_none() {
        return Err(Why::NoCallback);
    }
    if !matches!(prep.image, ImageUpdate::Set(_)) {
        return Err(Why::NoPicture);
    }
    if !prep.owner.as_ref().is_some_and(|(pid, _)| *pid != 0) {
        return Err(Why::NoOwner);
    }
    if req.data.state_update().is_some_and(|s| s.hidden() == Some(true)) {
        return Err(Why::AskedToBeHidden);
    }
    Ok(())
}

/// Whether `req` has the pieces of an add (a modify with a callback and a picture handle). Reads only the request, so it is the cheap
/// test to make BEFORE the model's lock is taken to see whether the icon is known.
pub fn could_become_add(req: &NotifyIconRequest) -> bool {
    req.message == Nim::Modify && req.data.callback().is_some() && req.data.icon_handle().is_some_and(|h| h != 0)
}

/// Whether the handler should look up the owner of a MODIFY's window: only for a request that could become an add, and only for an
/// icon the model does not hold (a modify of a known icon never asks again; a stream of tip updates costs no process lookups).
pub fn wants_owner(req: &NotifyIconRequest, known: bool) -> bool {
    could_become_add(req) && !known
}

/// Whose icon a request is about, as far as it matters for finding out why an icon is missing. Never a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerKind {
    /// The owner window belongs to `explorer.exe`: the shell's own parts (hardware, volume and the like).
    Explorer,
    OtherProgram,
    /// The window is gone or its process could not be read.
    Unknown,
}

/// The kind of owner for a program file name (`None`: not known).
pub fn owner_kind(program_file: Option<&str>) -> OwnerKind {
    match program_file.map(str::trim) {
        Some(n) if n.eq_ignore_ascii_case("explorer.exe") => OwnerKind::Explorer,
        Some(n) if !n.is_empty() => OwnerKind::OtherProgram,
        _ => OwnerKind::Unknown,
    }
}

/// What kind of identity the icon gave. Only the dock's own three indicators are told apart; any other GUID is just "other".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuidKind {
    None,
    Volume,
    Network,
    Power,
    Other,
}

pub fn guid_kind(guid: Option<crate::tray_wire::WireGuid>) -> GuidKind {
    use super::own_icons::{SystemIcon, system_icon_of};
    match guid {
        None => GuidKind::None,
        Some(g) => match system_icon_of(&g) {
            Some(SystemIcon::Volume) => GuidKind::Volume,
            Some(SystemIcon::Network) => GuidKind::Network,
            Some(SystemIcon::Power) => GuidKind::Power,
            None => GuidKind::Other,
        },
    }
}

/// What is known of a request about an icon the model does not hold.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnknownFacts {
    pub message: Nim,
    pub owner: OwnerKind,
    pub guid: GuidKind,
    /// Explorer's answer to the same request (`None`: none).
    pub verdict: Option<bool>,
    pub callback: bool,
    pub picture: bool,
}

/// The one log line for such a request: kinds and yes/no only, no program name, no GUID, no text (a test checks the words).
pub fn unknown_line(f: &UnknownFacts) -> String {
    let what = match f.message {
        Nim::Modify => "modify",
        Nim::Delete => "delete",
        Nim::SetVersion => "set-version",
        _ => "request",
    };
    let owner = match f.owner {
        OwnerKind::Explorer => "Explorer",
        OwnerKind::OtherProgram => "another program",
        OwnerKind::Unknown => "unknown",
    };
    let guid = match f.guid {
        GuidKind::None => "none",
        GuidKind::Volume => "the volume indicator's",
        GuidKind::Network => "the network indicator's",
        GuidKind::Power => "the power indicator's",
        GuidKind::Other => "another",
    };
    let explorer = match f.verdict {
        Some(true) => "accepted",
        Some(false) => "refused",
        None => "no answer",
    };
    let yn = |b| if b { "yes" } else { "no" };
    format!(
        "{what} for an icon the host never saw added: owner {owner}, GUID {guid}, Explorer {explorer}, callback {}, picture {}",
        yn(f.callback),
        yn(f.picture)
    )
}

#[cfg(test)]
mod tests {
    use super::super::model::{IconBitmap, test_util::*};
    use super::*;
    use crate::tray_wire::notify::nif;
    use std::sync::Arc;

    fn full_modify() -> NotifyIconRequest {
        let mut d = data(7, 1, nif::MESSAGE | nif::ICON | nif::TIP);
        d.tip = "x".into();
        d.icon = 0x77;
        req(Nim::Modify, d)
    }

    fn ready() -> Prepared {
        Prepared {
            image: ImageUpdate::Set(Arc::new(IconBitmap::new(16, 16, vec![1; 16 * 16 * 4]).unwrap())),
            owner: Some((50, "a.exe".into())),
            explorer_accepted: Some(true),
            ..Prepared::default()
        }
    }

    #[test]
    fn a_modify_that_carries_what_an_add_carries_and_that_explorer_accepted_is_an_add() {
        assert_eq!(modify_as_add(&full_modify(), &ready()), Ok(()));
    }

    #[test]
    fn every_missing_piece_keeps_it_a_modify() {
        let ok = ready();
        let no_verdict = Prepared { explorer_accepted: None, ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &no_verdict), Err(Why::ExplorerDidNotAccept), "no answer is no proof the icon exists");
        let refused = Prepared { explorer_accepted: Some(false), ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &refused), Err(Why::ExplorerDidNotAccept), "an icon Explorer does not have is not one to invent");
        let no_pic = Prepared { image: ImageUpdate::Keep, ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &no_pic), Err(Why::NoPicture));
        let cleared = Prepared { image: ImageUpdate::Clear, ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &cleared), Err(Why::NoPicture));
        let no_owner = Prepared { owner: None, ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &no_owner), Err(Why::NoOwner));
        let pid0 = Prepared { owner: Some((0, String::new())), ..ok.clone() };
        assert_eq!(modify_as_add(&full_modify(), &pid0), Err(Why::NoOwner));
        let tip_only = req(Nim::Modify, data(7, 1, nif::ICON | nif::TIP));
        assert_eq!(modify_as_add(&tip_only, &ok), Err(Why::NoCallback), "no callback message: nothing could be clicked");
        let add = req(Nim::Add, data(7, 1, nif::MESSAGE | nif::ICON));
        assert_eq!(modify_as_add(&add, &ok), Err(Why::NotAModify));
    }

    #[test]
    fn an_icon_that_asks_to_be_hidden_is_left_out() {
        let mut d = data(7, 1, nif::MESSAGE | nif::ICON | nif::STATE);
        d.state = 1;
        d.state_mask = 1;
        assert_eq!(modify_as_add(&req(Nim::Modify, d), &ready()), Err(Why::AskedToBeHidden));
        // a state that unhides (mask without the hidden bit set in the value) is not a request to hide
        let mut d = data(7, 1, nif::MESSAGE | nif::ICON | nif::STATE);
        d.state = 0;
        d.state_mask = 1;
        assert_eq!(modify_as_add(&req(Nim::Modify, d), &ready()), Ok(()));
    }

    #[test]
    fn the_cheap_test_reads_only_the_request() {
        assert!(could_become_add(&full_modify()));
        assert!(!could_become_add(&req(Nim::Modify, data(7, 1, nif::ICON | nif::TIP))));
        assert!(!could_become_add(&req(Nim::Add, data(7, 1, nif::MESSAGE | nif::ICON))));
    }

    #[test]
    fn the_line_for_an_unknown_icon_says_kinds_and_never_a_name() {
        use super::super::own_icons::{SystemIcon, system_guid};
        let f = UnknownFacts { message: Nim::Delete, owner: owner_kind(Some("Explorer.EXE")), guid: GuidKind::Other, verdict: Some(true), callback: false, picture: false };
        assert_eq!(unknown_line(&f), "delete for an icon the host never saw added: owner Explorer, GUID another, Explorer accepted, callback no, picture no");
        assert_eq!(owner_kind(Some("vpn.exe")), OwnerKind::OtherProgram);
        assert_eq!(owner_kind(Some("  ")), OwnerKind::Unknown);
        assert_eq!(owner_kind(None), OwnerKind::Unknown);
        // the program's own file name and a GUID's digits never reach the line
        let line = unknown_line(&UnknownFacts { owner: owner_kind(Some("secretprogram.exe")), guid: guid_kind(Some(system_guid(SystemIcon::Volume))), ..f });
        assert!(!line.to_lowercase().contains("secretprogram") && !line.contains("7820"), "{line}");
        assert!(line.contains("the volume indicator's"));
        let g = crate::tray_wire::WireGuid::from_parts(0xBA82_E2DC, 1, 2, [3; 8]);
        assert_eq!(guid_kind(Some(g)), GuidKind::Other);
        assert_eq!(guid_kind(None), GuidKind::None);
    }

    #[test]
    fn the_owner_is_looked_up_only_for_a_modify_that_could_become_an_add() {
        assert!(wants_owner(&full_modify(), false));
        assert!(!wants_owner(&full_modify(), true), "a known icon: a stream of updates costs no lookups");
        assert!(!wants_owner(&req(Nim::Modify, data(7, 1, nif::ICON | nif::TIP)), false), "no callback");
        assert!(!wants_owner(&req(Nim::Modify, data(7, 1, nif::MESSAGE | nif::TIP)), false), "no picture handle");
        let mut cleared = data(7, 1, nif::MESSAGE | nif::ICON);
        cleared.icon = 0;
        assert!(!wants_owner(&req(Nim::Modify, cleared), false), "a null handle clears; there is no picture");
        let mut with_icon = data(7, 1, nif::MESSAGE | nif::ICON);
        with_icon.icon = 0x55;
        assert!(wants_owner(&req(Nim::Modify, with_icon.clone()), false));
        assert!(!wants_owner(&req(Nim::Add, with_icon), false), "an add always asks (that is the old rule, not this one)");
    }
}
