//! Which icons show on the strip and which wait in the overflow, and in what order, from the USER'S OWN order list (row B9 of the
//! plan). Pure. The list is keyed by [`StableKey`] (a GUID, or program and id), never by window handle, and is the interface's file,
//! not Windows': `HKCU\Control Panel\NotifyIconSettings` is Explorer's own settings store, full of stale entries (38 of 98 on the
//! research machine belong to one packaged program's old versions) and is never read for the list, never written.

use std::collections::HashSet;

use super::model::{IconId, StableKey, TrayIcon};

/// The icons to show, in order, and the ones in the overflow, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arranged {
    pub visible: Vec<IconId>,
    pub overflow: Vec<IconId>,
}

/// `order` is the user's list (first = leftmost). Icons the list does not name come after the ones it names, in the order they
/// arrived. `in_overflow` names the icons the user sent to the overflow. Icons that ask to be hidden (`NIS_HIDDEN`) are in neither.
pub fn arrange(icons: &[TrayIcon], order: &[StableKey], in_overflow: &HashSet<StableKey>) -> Arranged {
    let mut shown: Vec<&TrayIcon> = icons.iter().filter(|i| !i.hidden()).collect();
    // (a stable sort by position in the list; unnamed icons share the last position and keep their arrival order)
    shown.sort_by_key(|i| {
        let k = i.stable_key();
        (order.iter().position(|o| *o == k).unwrap_or(usize::MAX), i.added_seq)
    });
    let mut out = Arranged::default();
    for i in shown {
        if in_overflow.contains(&i.stable_key()) {
            out.overflow.push(i.id);
        } else {
            out.visible.push(i.id);
        }
    }
    out
}

/// The list after the user moved icon `from` to the place of icon `to` (both by key); keys not in the list are added at the end first.
pub fn move_key(order: &[StableKey], all: &[StableKey], from: &StableKey, to: &StableKey) -> Vec<StableKey> {
    let mut list: Vec<StableKey> = order.to_vec();
    for k in all {
        if !list.contains(k) {
            list.push(k.clone());
        }
    }
    let (Some(a), Some(b)) = (list.iter().position(|k| k == from), list.iter().position(|k| k == to)) else { return list };
    let k = list.remove(a);
    list.insert(b, k);
    list
}

#[cfg(test)]
mod tests {
    use super::super::model::{Prepared, TrayModel, test_util::*};
    use super::*;
    use crate::tray_wire::notify::{Nim, nif};

    fn model_with(items: &[(u32, u32, &str, bool)]) -> TrayModel {
        let mut m = TrayModel::new();
        for (hwnd, uid, exe, hidden) in items {
            let mut d = data(*hwnd, *uid, nif::MESSAGE | nif::STATE);
            if *hidden {
                d.state = 1;
                d.state_mask = 1;
            }
            m.apply(&req(Nim::Add, d), Prepared { owner: Some((1, exe.to_string())), ..Default::default() });
        }
        m
    }

    fn key(exe: &str, uid: u32) -> StableKey {
        StableKey::ExeUid { exe: exe.into(), uid }
    }

    #[test]
    fn default_order_is_arrival_order() {
        let m = model_with(&[(1, 1, "a.exe", false), (2, 1, "b.exe", false), (3, 1, "c.exe", false)]);
        let a = arrange(m.icons(), &[], &HashSet::new());
        assert_eq!(a.visible, m.icons().iter().map(|i| i.id).collect::<Vec<_>>());
        assert!(a.overflow.is_empty());
    }

    #[test]
    fn the_users_order_comes_first_and_new_icons_follow() {
        let m = model_with(&[(1, 1, "a.exe", false), (2, 1, "b.exe", false), (3, 1, "c.exe", false)]);
        let ids: Vec<IconId> = m.icons().iter().map(|i| i.id).collect();
        let a = arrange(m.icons(), &[key("c.exe", 1), key("a.exe", 1)], &HashSet::new());
        assert_eq!(a.visible, vec![ids[2], ids[0], ids[1]]);
    }

    #[test]
    fn overflow_and_hidden() {
        let m = model_with(&[(1, 1, "a.exe", false), (2, 1, "b.exe", true), (3, 1, "c.exe", false)]);
        let ids: Vec<IconId> = m.icons().iter().map(|i| i.id).collect();
        let over: HashSet<StableKey> = [key("c.exe", 1)].into_iter().collect();
        let a = arrange(m.icons(), &[], &over);
        assert_eq!(a.visible, vec![ids[0]]);
        assert_eq!(a.overflow, vec![ids[2]]);
        // (the hidden one is in neither)
    }

    #[test]
    fn the_order_survives_a_program_restart() {
        // another window handle and process, the same program and id
        let order = [key("b.exe", 1), key("a.exe", 1)];
        let m = model_with(&[(900, 1, "a.exe", false), (901, 1, "B.EXE", false)]);
        let ids: Vec<IconId> = m.icons().iter().map(|i| i.id).collect();
        assert_eq!(arrange(m.icons(), &order, &HashSet::new()).visible, vec![ids[1], ids[0]]);
    }

    #[test]
    fn moving_an_icon_in_the_list() {
        let all = [key("a.exe", 1), key("b.exe", 1), key("c.exe", 1)];
        let l = move_key(&[], &all, &key("c.exe", 1), &key("a.exe", 1));
        assert_eq!(l, vec![key("c.exe", 1), key("a.exe", 1), key("b.exe", 1)]);
        let l = move_key(&l, &all, &key("c.exe", 1), &key("b.exe", 1));
        assert_eq!(l, vec![key("a.exe", 1), key("b.exe", 1), key("c.exe", 1)]);
        // an unknown key changes nothing
        assert_eq!(move_key(&l, &all, &key("zzz", 1), &key("a.exe", 1)), l);
    }
}
