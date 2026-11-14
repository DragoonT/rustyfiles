//! The right island's own status buttons (safe removal, Windows Security, network, volume): their order and which of them are shown.
//! Pure: the saved lists in, the buttons to draw out.
//!
//! WHY (a user of 0.7.1, with a picture of the four symbols: "will be useful if we can swap icons here"). The order was fixed in the
//! island's code. The programs' own tray icons beside them can already be dragged into another order (`tray_order.rs`); these four are
//! the island's, so their order and their presence are two settings: `dock_status_order` and `dock_status_hidden`, lists of the keys
//! below.
//!
//! TWO WAYS TO CHANGE THE ORDER: the arrows of the list in Settings ([`moved`]), and a drag of the button itself on the island (the
//! user: "sıra değiştirme drag drop ile de olsun"), as the programs' tray icons beside them are dragged: [`drop_index`] says where the
//! pointer would put the button, [`dropped`] the order that results.
//!
//! A LIST FROM ANOTHER VERSION must not lose a button: a key the list does not name follows the named ones in the usual order, a key
//! this version does not know is skipped, and one that is named twice counts where it is first.

/// One of the island's own buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Status {
    /// Safe removal: there only while a removable drive holds a volume.
    Eject,
    /// Windows Security.
    Security,
    Network,
    Volume,
}

impl Status {
    /// The usual order (what the island had while the order was fixed).
    pub const ALL: [Status; 4] = [Status::Eject, Status::Security, Status::Network, Status::Volume];

    /// The name in the settings file.
    pub fn key(self) -> &'static str {
        match self {
            Status::Eject => "eject",
            Status::Security => "security",
            Status::Network => "network",
            Status::Volume => "volume",
        }
    }

    pub fn from_key(key: &str) -> Option<Status> {
        Status::ALL.into_iter().find(|s| s.key() == key.trim())
    }
}

/// All four buttons in the saved order, completed as the module says.
pub fn order(saved: &[String]) -> [Status; 4] {
    let mut out: Vec<Status> = Vec::with_capacity(4);
    for s in saved.iter().filter_map(|k| Status::from_key(k)).chain(Status::ALL) {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    [out[0], out[1], out[2], out[3]]
}

/// Whether the settings hide this button.
pub fn is_hidden(hidden: &[String], which: Status) -> bool {
    hidden.iter().any(|k| Status::from_key(k) == Some(which))
}

/// The buttons the island draws, in order: the saved order, less the hidden ones, less safe removal while there is nothing to remove
/// (`removable`: a removable drive holds a volume).
pub fn shown(saved: &[String], hidden: &[String], removable: bool) -> Vec<Status> {
    order(saved).into_iter().filter(|s| !is_hidden(hidden, *s) && (*s != Status::Eject || removable)).collect()
}

/// The order to save after `which` was moved one place earlier (`by` -1) or later (+1). At an end it stays where it is.
pub fn moved(saved: &[String], which: Status, by: i32) -> Vec<String> {
    let mut all = order(saved).to_vec();
    if let Some(i) = all.iter().position(|s| *s == which) {
        let j = (i as i32 + by.signum()).clamp(0, all.len() as i32 - 1) as usize;
        all.swap(i, j);
    }
    all.into_iter().map(|s| s.key().to_string()).collect()
}

/// The order to save after `which` was dragged onto the row of index `to` in the list of all four (Settings): it takes that place,
/// and the rows between move one place towards where it was. A drag onto its own row changes nothing.
pub fn placed(saved: &[String], which: Status, to: usize) -> Vec<String> {
    let mut all: Vec<Status> = order(saved).into_iter().filter(|s| *s != which).collect();
    all.insert(to.min(all.len()), which);
    all.into_iter().map(|s| s.key().to_string()).collect()
}

/// Where a dragged button would be put among the `n` buttons the island shows: before the button of this index, `n` for the end.
/// `first_left` is the first button's left edge, `step` the distance from one button's left edge to the next one's and `button` a
/// button's width (device px, screen coordinates); the pointer is at `x`. It is the number of buttons whose middle is left of the
/// pointer, so the place changes when the pointer crosses the middle of a button.
pub fn drop_index(n: usize, first_left: i32, step: i32, button: i32, x: i32) -> usize {
    (0..n).filter(|&i| x > first_left + i as i32 * step + button / 2).count()
}

/// Whether a drop before index `insert` moves the button that stands at index `at` (both count in the buttons that are shown): a drop
/// before itself or before its right neighbour leaves it where it is.
pub fn drop_moves(at: usize, insert: usize) -> bool {
    insert != at && insert != at + 1
}

/// The order to save after the button `which` was dropped before the shown button of index `insert` ([`drop_index`]). `shown` is what
/// the island draws now, in its order (`which` among them). `None` when nothing moves.
///
/// THE BUTTONS THAT ARE NOT SHOWN (the hidden ones, and safe removal while there is nothing to remove) keep their places beside
/// their neighbours: `which` is put directly before the shown button it was dropped before, or directly after the last shown one,
/// and nothing else changes its order.
pub fn dropped(saved: &[String], shown: &[Status], which: Status, insert: usize) -> Option<Vec<String>> {
    let at = shown.iter().position(|s| *s == which)?;
    if !drop_moves(at, insert) {
        return None;
    }
    let before = order(saved).to_vec();
    let mut all: Vec<Status> = before.iter().copied().filter(|s| *s != which).collect();
    let pos = match shown.get(insert) {
        Some(next) => all.iter().position(|s| s == next)?,
        None => {
            let last = shown.iter().rev().find(|s| **s != which)?;
            all.iter().position(|s| s == last)? + 1
        }
    };
    all.insert(pos, which);
    (all != before).then(|| all.into_iter().map(|s| s.key().to_string()).collect())
}

/// The hidden list to save after `which` was shown (`show`) or hidden: only known keys, each once, in the usual order.
pub fn with_hidden(hidden: &[String], which: Status, show: bool) -> Vec<String> {
    Status::ALL.into_iter().filter(|s| if *s == which { !show } else { is_hidden(hidden, *s) }).map(|s| s.key().to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn with_nothing_saved_the_buttons_are_the_ones_the_island_always_had_in_their_order() {
        assert_eq!(order(&[]), Status::ALL);
        assert_eq!(shown(&[], &[], true), Status::ALL.to_vec());
        // safe removal is there only while there is something to remove
        assert_eq!(shown(&[], &[], false), [Status::Security, Status::Network, Status::Volume]);
    }

    #[test]
    fn a_saved_order_is_followed_and_a_list_from_another_version_loses_no_button() {
        assert_eq!(order(&keys(&["volume", "network", "security", "eject"])), [Status::Volume, Status::Network, Status::Security, Status::Eject]);
        // a list that names only some: the rest follow in the usual order
        assert_eq!(order(&keys(&["volume"])), [Status::Volume, Status::Eject, Status::Security, Status::Network]);
        // an unknown key (a later version's), a key named twice, spaces around a key
        assert_eq!(order(&keys(&["bluetooth", "network", " volume ", "network"])), [Status::Network, Status::Volume, Status::Eject, Status::Security]);
    }

    #[test]
    fn hidden_buttons_are_left_out_and_the_others_keep_their_order() {
        let order_ = keys(&["volume", "security", "network", "eject"]);
        assert_eq!(shown(&order_, &keys(&["security"]), true), [Status::Volume, Status::Network, Status::Eject]);
        assert_eq!(shown(&order_, &keys(&["security", "nonsense", "volume"]), false), [Status::Network]);
        // all four hidden: none (the island then has its clock and its bell)
        assert!(shown(&order_, &keys(&["eject", "security", "network", "volume"]), true).is_empty());
        assert!(is_hidden(&keys(&["volume"]), Status::Volume) && !is_hidden(&keys(&["volume"]), Status::Network));
    }

    #[test]
    fn a_button_moves_one_place_and_stays_at_an_end() {
        assert_eq!(moved(&[], Status::Volume, -1), keys(&["eject", "security", "volume", "network"]));
        assert_eq!(moved(&[], Status::Eject, 1), keys(&["security", "eject", "network", "volume"]));
        assert_eq!(moved(&[], Status::Eject, -1), keys(&["eject", "security", "network", "volume"]), "the first stays first");
        assert_eq!(moved(&[], Status::Volume, 1), keys(&["eject", "security", "network", "volume"]), "the last stays last");
        // two moves bring the last to the second place; a big number is one step
        let once = moved(&[], Status::Volume, -5);
        assert_eq!(moved(&once, Status::Volume, -1), keys(&["eject", "volume", "security", "network"]));
        // a saved list with rubbish in it is written back clean and whole
        assert_eq!(moved(&keys(&["x", "volume"]), Status::Security, 1), keys(&["volume", "eject", "network", "security"]));
    }

    #[test]
    fn hiding_and_showing_write_a_clean_list() {
        assert_eq!(with_hidden(&[], Status::Network, false), keys(&["network"]));
        assert_eq!(with_hidden(&keys(&["network", "volume", "nonsense", "network"]), Status::Network, true), keys(&["volume"]));
        assert_eq!(with_hidden(&keys(&["volume"]), Status::Eject, false), keys(&["eject", "volume"]));
        assert!(with_hidden(&keys(&["volume"]), Status::Volume, true).is_empty());
        assert_eq!(with_hidden(&keys(&["volume"]), Status::Volume, false), keys(&["volume"]), "hidden twice is hidden once");
    }

    #[test]
    fn the_pointer_puts_a_dragged_button_before_the_first_button_whose_middle_it_has_not_passed() {
        // four buttons 32 px wide, 36 px from one to the next, the first at x = 100: middles at 116, 152, 188, 224
        let at = |x: i32| drop_index(4, 100, 36, 32, x);
        assert_eq!((at(0), at(100), at(116)), (0, 0, 0), "left of the first middle (and far left of the island): the first place");
        assert_eq!((at(117), at(152)), (1, 1));
        assert_eq!((at(153), at(188)), (2, 2));
        assert_eq!((at(189), at(224)), (3, 3));
        assert_eq!((at(225), at(5000)), (4, 4), "past the last middle: the end");
        assert_eq!(drop_index(0, 100, 36, 32, 150), 0, "no buttons: one place");
        // a drop before itself or before its right neighbour moves nothing
        assert!(!drop_moves(1, 1) && !drop_moves(1, 2));
        assert!(drop_moves(1, 0) && drop_moves(1, 3) && drop_moves(0, 2) && drop_moves(3, 0));
    }

    #[test]
    fn a_dropped_button_stands_where_it_was_dropped_and_the_others_keep_their_order() {
        let all = Status::ALL.to_vec(); // eject, security, network, volume
        // the last to the front, the first to the end, one step to the right
        assert_eq!(dropped(&[], &all, Status::Volume, 0), Some(keys(&["volume", "eject", "security", "network"])));
        assert_eq!(dropped(&[], &all, Status::Eject, 4), Some(keys(&["security", "network", "volume", "eject"])));
        assert_eq!(dropped(&[], &all, Status::Security, 3), Some(keys(&["eject", "network", "security", "volume"])));
        // on its own place: nothing to save
        assert_eq!(dropped(&[], &all, Status::Security, 1), None);
        assert_eq!(dropped(&[], &all, Status::Security, 2), None);
        // a button that is not shown cannot be dropped
        assert_eq!(dropped(&[], &[Status::Security, Status::Network, Status::Volume], Status::Eject, 0), None);
        // from a saved order
        let saved = keys(&["volume", "network", "security", "eject"]);
        let shown = [Status::Volume, Status::Network, Status::Security, Status::Eject];
        assert_eq!(dropped(&saved, &shown, Status::Eject, 1), Some(keys(&["volume", "eject", "network", "security"])));
    }

    #[test]
    fn buttons_that_are_not_shown_keep_their_places_when_another_is_dropped() {
        // no removable drive: the island shows security, network, volume; safe removal stays first in the saved order
        let shown = [Status::Security, Status::Network, Status::Volume];
        assert_eq!(dropped(&[], &shown, Status::Volume, 0), Some(keys(&["eject", "volume", "security", "network"])));
        assert_eq!(dropped(&[], &shown, Status::Security, 3), Some(keys(&["eject", "network", "volume", "security"])));
        // security is hidden and stands between the two that are shown: network dropped after volume goes directly after volume
        let shown = [Status::Network, Status::Volume];
        let saved = keys(&["network", "security", "volume", "eject"]);
        assert_eq!(dropped(&saved, &shown, Status::Network, 2), Some(keys(&["security", "volume", "network", "eject"])));
        assert_eq!(dropped(&saved, &shown, Status::Volume, 0), Some(keys(&["volume", "network", "security", "eject"])));
        // whatever is dropped where, the result names every button once
        for which in shown {
            for insert in 0..=shown.len() {
                if let Some(new) = dropped(&saved, &shown, which, insert) {
                    assert_eq!(order(&new).map(|s| s.key().to_string()).to_vec(), new, "whole and clean");
                    let now: Vec<Status> = super::shown(&new, &keys(&["security"]), false);
                    assert_eq!(now.len(), 2);
                    let at = now.iter().position(|s| *s == which).unwrap();
                    assert_eq!(at, if insert == 0 { 0 } else { 1 }, "{which:?} dropped at {insert} stands there");
                }
            }
        }
    }

    #[test]
    fn a_row_dragged_onto_another_takes_its_place() {
        // eject, security, network, volume
        assert_eq!(placed(&[], Status::Volume, 0), keys(&["volume", "eject", "security", "network"]));
        assert_eq!(placed(&[], Status::Eject, 3), keys(&["security", "network", "volume", "eject"]));
        assert_eq!(placed(&[], Status::Eject, 2), keys(&["security", "network", "eject", "volume"]), "down: it stands where the row it was dropped on stood");
        assert_eq!(placed(&[], Status::Volume, 1), keys(&["eject", "volume", "security", "network"]), "up: the same");
        assert_eq!(placed(&[], Status::Network, 2), keys(&["eject", "security", "network", "volume"]), "onto its own row: as it was");
        assert_eq!(placed(&[], Status::Network, 99), keys(&["eject", "security", "volume", "network"]), "past the end is the end");
        assert_eq!(placed(&keys(&["x", "volume"]), Status::Eject, 0), keys(&["eject", "volume", "security", "network"]), "written back whole and clean");
    }

    #[test]
    fn every_key_is_read_back_as_its_button() {
        for s in Status::ALL {
            assert_eq!(Status::from_key(s.key()), Some(s));
        }
        assert_eq!(Status::from_key("clock"), None);
    }
}
