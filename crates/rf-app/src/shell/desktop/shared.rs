//! What the desktop views of one process share.
//!
//! The preview window has one view, which owns its layout store. Cover mode has one view PER MONITOR, and the layout file is one file
//! for all of them (a cell belongs to a monitor, but the sort, the icon size, "align to grid" and the removed-items memory are the
//! desktop's, and one file written by two owners would lose what the other wrote). So the store lives behind a handle that the views
//! hold together, and the views that share it know each other ([`Peers`]) so that a change made in one is drawn by all.
//!
//! Everything here is used on the UI thread only (the views are, and GPUI entities are not `Send`).

use std::cell::{Ref, RefCell, RefMut};
use std::rc::Rc;

use gpui_kit::*;
use rf_core::desktop::store::Store;

use super::view::DesktopView;

/// The layout store, shared. `get` and `edit` borrow for the length of the statement: never hold one while calling the other.
#[derive(Clone)]
pub struct SharedStore(Rc<RefCell<Store>>);

impl SharedStore {
    pub fn new(store: Store) -> SharedStore {
        SharedStore(Rc::new(RefCell::new(store)))
    }
    pub fn get(&self) -> Ref<'_, Store> {
        self.0.borrow()
    }
    pub fn edit(&self) -> RefMut<'_, Store> {
        self.0.borrow_mut()
    }
    /// A fresh store (the layout file was read).
    pub fn replace(&self, store: Store) {
        *self.0.borrow_mut() = store;
    }
}

/// The views that draw one desktop (all of them, this one included).
#[derive(Clone, Default)]
pub struct Peers(Rc<RefCell<Vec<WeakEntity<DesktopView>>>>);

impl Peers {
    pub fn add(&self, v: WeakEntity<DesktopView>) {
        // (a view whose window went is dropped by the toolkit a little later than the window: forget what is gone at every arrival too)
        self.prune();
        self.0.borrow_mut().push(v);
    }

    /// Forgets the views that are gone (called when the cover windows follow the monitors: each monitor that goes leaves a dead handle).
    pub fn prune(&self) {
        self.0.borrow_mut().retain(|w| w.upgrade().is_some());
    }

    pub fn all(&self) -> Vec<WeakEntity<DesktopView>> {
        self.0.borrow().clone()
    }

    /// Runs `f` on every view except `me`, which is being updated right now and must not be entered again.
    pub fn for_others(&self, me: EntityId, cx: &mut App, f: impl Fn(&mut DesktopView, &mut Context<DesktopView>)) {
        for w in self.all() {
            if w.entity_id() == me {
                continue;
            }
            let _ = w.update(cx, |v, cx| f(v, cx));
        }
    }
}

#[cfg(test)]
mod tests {
    // (explicit names: `use super::*` brings in the toolkit's own `test` attribute through its glob and breaks `#[test]`)
    use super::SharedStore;
    use rf_core::desktop::store::Store;

    #[test]
    fn views_that_hold_the_handle_see_each_others_changes() {
        let a = SharedStore::new(Store::default());
        let b = a.clone();
        a.edit().set_align_to_grid(false);
        assert!(!b.get().align_to_grid, "one store, two handles");
        b.edit().set_sort(rf_core::desktop::store::SortPref { key: rf_core::desktop::store::SortName::Size, descending: true });
        assert!(a.get().sort.descending);
        a.replace(Store::default());
        assert!(b.get().align_to_grid, "a fresh store replaces the one both hold");
    }
}
