//! One opening of the switcher as a plain value: the windows shown, which card is selected, and what each effect of the chord does to
//! them. Pure: the app's view holds one while the switcher is open and draws it; the tests drive it with effects.

use super::SwitchItem;
use super::chord::{Effect, Scope};
use super::layout::Layout;
use super::select::{Selection, keep_scope};

/// What the view has to do after an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Nothing,
    /// The selection moved: draw again.
    Redraw,
    /// Close, and bring this window to the front.
    Choose(isize),
    /// Close, and leave things as they were.
    Cancel,
    /// This window (its handle and its process) is to be asked to close; its card is out of the session already and the switcher
    /// stays open. The host lays the remaining cards out anew ([`Session::fit`]), or cancels when none is left.
    Close(isize, u32),
}

#[derive(Clone, Debug)]
pub struct Session {
    items: Vec<SwitchItem>,
    selection: Selection,
    scope: Scope,
}

impl Session {
    /// The windows for `scope` (for `ThisApp` those of the app of the first window, which is the one in front), with the selection a
    /// first Tab (`backwards` false) or Shift+Tab gives.
    pub fn open(all: Vec<SwitchItem>, scope: Scope, backwards: bool) -> Session {
        Session::open_from(all, scope, backwards, true)
    }

    /// Like [`Session::open`]; `first_is_current` says whether the first window is the one in front. When the foreground is not a
    /// listed window (the desktop, our dock) it is not, and a first Tab selects the first window instead of skipping it.
    pub fn open_from(all: Vec<SwitchItem>, scope: Scope, backwards: bool, first_is_current: bool) -> Session {
        let items = keep_scope(all, scope, |i| i.group.clone());
        // (the columns are not known until the layout is: `fit` sets them)
        let selection = Selection::opened_from(items.len(), 1, backwards, first_is_current);
        Session { items, selection, scope }
    }

    /// The layout was worked out for these items: the columns for the arrow keys, and the cards that did not fit are dropped (the
    /// oldest windows go; the selection stays inside what is shown).
    pub fn fit(&mut self, layout: &Layout) {
        let selected = self.selection.index();
        self.items.truncate(layout.visible.max(1));
        self.selection = Selection::opened(self.items.len(), layout.cols, false);
        if let Some(i) = selected {
            self.selection.set(i.min(self.items.len().saturating_sub(1)));
        }
    }

    pub fn items(&self) -> &[SwitchItem] {
        &self.items
    }

    pub fn scope(&self) -> Scope {
        self.scope
    }

    pub fn selected(&self) -> Option<usize> {
        self.selection.index()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// An effect of the chord.
    pub fn apply(&mut self, effect: Effect) -> Step {
        let before = self.selection.index();
        match effect {
            // The opening is part of making the session (`open` has already selected the window a first Tab selects), so an Open
            // that reaches an open session, from a host that feeds the effect it opened on, must not step a second time. (The
            // chord itself emits Open only when it is closed.)
            Effect::Open { .. } => return Step::Nothing,
            Effect::Step { backwards } => self.selection.step(backwards),
            Effect::Move(dir) => self.selection.move_dir(dir),
            Effect::Choose => {
                return match self.selection.chosen() {
                    Some(i) => Step::Choose(self.items[i].hwnd),
                    None => Step::Cancel,
                };
            }
            Effect::Cancel => return Step::Cancel,
            Effect::Close => {
                return match self.selection.index().filter(|_| !self.items.is_empty()) {
                    Some(i) => self.close_at(i),
                    None => Step::Nothing,
                };
            }
        }
        if self.selection.index() != before { Step::Redraw } else { Step::Nothing }
    }

    /// The pointer moved over a card.
    pub fn hover(&mut self, i: usize) -> Step {
        if self.selection.index() == Some(i) || i >= self.items.len() {
            return Step::Nothing;
        }
        self.selection.set(i);
        Step::Redraw
    }

    /// The card at `i` goes out of the session: its window is the one to ask to close (the selected card with the close key, any
    /// card with the middle button). The selection keeps its PLACE, which is the next window now (the last one when the last card
    /// went), so that the key pressed again closes the next one, as in Windows' own switcher.
    pub fn close_at(&mut self, i: usize) -> Step {
        if i >= self.items.len() {
            return Step::Nothing;
        }
        let selected = self.selection.index().map(|s| if s > i { s - 1 } else { s });
        let gone = self.items.remove(i);
        // (the columns are the new layout's: `fit` sets them, as after `open`)
        self.selection = Selection::opened(self.items.len(), 1, false);
        if let Some(s) = selected.filter(|_| !self.items.is_empty()) {
            self.selection.set(s.min(self.items.len() - 1));
        }
        Step::Close(gone.hwnd, gone.pid)
    }

    /// A card was clicked.
    pub fn click(&mut self, i: usize) -> Step {
        match self.items.get(i) {
            Some(item) => Step::Choose(item.hwnd),
            None => Step::Nothing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::chord::Dir;
    use super::super::layout::plan;
    use super::*;
    use crate::dock::layout::Rect;

    fn item(hwnd: isize, group: &str) -> SwitchItem {
        SwitchItem {
            hwnd,
            title: format!("w{hwnd}"),
            pid: 1,
            exe: None,
            app_id: None,
            group: group.into(),
            minimized: false,
            suspended: false,
            other_desktop: false,
        }
    }

    fn four() -> Vec<SwitchItem> {
        vec![item(1, "a"), item(2, "b"), item(3, "a"), item(4, "c")]
    }

    #[test]
    fn it_opens_on_the_window_used_before_the_current_one() {
        let s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.selected(), Some(1));
    }

    #[test]
    fn an_open_effect_fed_to_an_open_session_does_not_step_again() {
        // the host opened the switcher for Alt+Tab and then hands the same effect to it: the selection stays on the second window
        let mut s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.apply(Effect::Open { scope: Scope::AllWindows, backwards: false }), Step::Nothing);
        assert_eq!(s.selected(), Some(1));
        assert_eq!(s.apply(Effect::Choose), Step::Choose(2), "a quick release still chooses the window used before");
        let mut back = Session::open(four(), Scope::AllWindows, true);
        assert_eq!(back.apply(Effect::Open { scope: Scope::AllWindows, backwards: true }), Step::Nothing);
        assert_eq!(back.selected(), Some(3));
    }

    #[test]
    fn with_the_foreground_not_in_the_list_the_first_tab_selects_the_first_window() {
        let s = Session::open_from(four(), Scope::AllWindows, false, false);
        assert_eq!(s.selected(), Some(0));
        let s = Session::open_from(four(), Scope::AllWindows, false, true);
        assert_eq!(s.selected(), Some(1));
    }

    #[test]
    fn tab_steps_and_enter_chooses_that_window() {
        let mut s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.apply(Effect::Step { backwards: false }), Step::Redraw);
        assert_eq!(s.selected(), Some(2));
        assert_eq!(s.apply(Effect::Choose), Step::Choose(3));
    }

    #[test]
    fn releasing_alt_straight_after_opening_chooses_the_previous_window() {
        let mut s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.apply(Effect::Choose), Step::Choose(2));
    }

    #[test]
    fn escape_cancels_and_chooses_nothing() {
        let mut s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.apply(Effect::Cancel), Step::Cancel);
    }

    #[test]
    fn this_app_opens_with_only_the_windows_of_the_app_in_front() {
        let s = Session::open(four(), Scope::ThisApp, false);
        let hwnds: Vec<isize> = s.items().iter().map(|i| i.hwnd).collect();
        assert_eq!(hwnds, vec![1, 3]);
        assert_eq!(s.selected(), Some(1), "the other window of the app");
    }

    #[test]
    fn shift_tab_opens_on_the_oldest() {
        let s = Session::open(four(), Scope::AllWindows, true);
        assert_eq!(s.selected(), Some(3));
    }

    #[test]
    fn an_empty_list_has_nothing_to_choose() {
        let mut s = Session::open(Vec::new(), Scope::AllWindows, false);
        assert!(s.is_empty());
        assert_eq!(s.apply(Effect::Choose), Step::Cancel);
        assert_eq!(s.apply(Effect::Step { backwards: false }), Step::Nothing);
    }

    #[test]
    fn a_move_that_changes_nothing_asks_for_no_redraw() {
        let mut s = Session::open(vec![item(1, "a")], Scope::AllWindows, false);
        assert_eq!(s.apply(Effect::Move(Dir::Down)), Step::Nothing);
        assert_eq!(s.apply(Effect::Step { backwards: false }), Step::Nothing, "one window: Tab stays on it");
    }

    #[test]
    fn the_pointer_selects_and_a_click_chooses() {
        let mut s = Session::open(four(), Scope::AllWindows, false);
        assert_eq!(s.hover(1), Step::Nothing, "already selected");
        assert_eq!(s.hover(3), Step::Redraw);
        assert_eq!(s.selected(), Some(3));
        assert_eq!(s.hover(99), Step::Nothing);
        assert_eq!(s.click(0), Step::Choose(1));
        assert_eq!(s.click(99), Step::Nothing);
    }

    #[test]
    fn fitting_to_the_layout_drops_what_did_not_fit_and_keeps_the_selection_inside() {
        let all: Vec<SwitchItem> = (1..=400).map(|i| item(i, "g")).collect();
        let mut s = Session::open(all, Scope::AllWindows, true); // selects the last of 400
        let layout = plan(400, Rect::new(0, 0, 1920, 1032), 1.0);
        assert!(layout.visible < 400);
        s.fit(&layout);
        assert_eq!(s.items().len(), layout.visible);
        assert_eq!(s.selected(), Some(layout.visible - 1));
    }

    #[test]
    fn after_fitting_the_arrows_use_the_layouts_columns() {
        let all: Vec<SwitchItem> = (1..=12).map(|i| item(i, "g")).collect();
        let mut s = Session::open(all, Scope::AllWindows, false);
        let layout = plan(12, Rect::new(0, 0, 1920, 1032), 1.0);
        assert_eq!(layout.cols, 6);
        s.fit(&layout);
        assert_eq!(s.selected(), Some(1));
        s.apply(Effect::Move(Dir::Down));
        assert_eq!(s.selected(), Some(7), "one row down is six cards on");
    }

    /// Closing from the switcher: the card goes, the selection stays at its place (the next window), the last card's close moves
    /// it back, and the last window of all leaves an empty session.
    #[test]
    fn closing_a_card_takes_it_out_and_keeps_the_selections_place() {
        let mut s = Session::open(vec![item(1, "a"), item(2, "b"), item(3, "c")], Scope::AllWindows, false);
        assert_eq!(s.selected(), Some(1), "a first Tab selects the window used before");
        assert_eq!(s.apply(Effect::Close), Step::Close(2, 1));
        assert_eq!(s.items().iter().map(|i| i.hwnd).collect::<Vec<_>>(), vec![1, 3]);
        assert_eq!(s.selected(), Some(1), "the same place: the next window");
        assert_eq!(s.apply(Effect::Close), Step::Close(3, 1));
        assert_eq!(s.selected(), Some(0), "the last card went: the one before it");
        // a middle click on a card before the selected one moves the selection with its window
        let mut m = Session::open(vec![item(1, "a"), item(2, "b"), item(3, "c")], Scope::AllWindows, true);
        assert_eq!(m.selected(), Some(2));
        assert_eq!(m.close_at(0), Step::Close(1, 1));
        assert_eq!((m.selected(), m.items()[1].hwnd), (Some(1), 3), "still on window 3");
        assert_eq!(m.close_at(9), Step::Nothing);
        // the last window of all
        assert_eq!(s.apply(Effect::Close), Step::Close(1, 1));
        assert!(s.is_empty());
        assert_eq!(s.apply(Effect::Close), Step::Nothing);
    }
}
