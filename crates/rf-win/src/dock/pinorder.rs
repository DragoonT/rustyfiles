//! The order of the dock's own pins, written from a drag of an icon (`dock/reorder.rs` in the app decides what a drop means; this is the
//! file's side of it). The pins the island shows are the file's pins whose program still exists, in the file's order; a pin whose
//! program is gone stays in the file and out of the island. A reorder must therefore never lose those hidden pins or move them about:
//! the places in the file that the shown pins held are filled again with the shown pins in their new order, and everything else stays
//! where it was.
//!
//! BY POSITION, NOT BY IDENTITY. The island's pinned items are one per pin of the list it read, in that order, so the n-th pinned item
//! is the n-th shown pin of the file. A drop is therefore told as positions ([`Placed::Existing`]), never as apps looked up again with
//! `PinFile::find`: two pins that start one program (two profiles of one browser, two shortcuts to one exe with other arguments)
//! are the same app to `find`, and a reorder that went by it would refuse to move either.
//!
//! Pure: [`PinFile::apply_order`] only edits the list it is given (the file is written by `ownpins::change`, which calls it).

use std::path::Path;

use super::model::DockApp;
use super::ownpins::{OwnPin, PinFile, pin_from_app};

/// One place of the pinned apps after a drop, left to right.
#[derive(Clone, Debug)]
pub enum Placed {
    /// The pin the island showed at this position (counted among the pinned apps, before the drop).
    Existing(usize),
    /// A running app that was dropped among the pinned ones: it is pinned here.
    New(Box<DockApp>),
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

impl PinFile {
    /// Puts the pins in the order `placed` lists them (the apps the island shows as pinned, left to right, after the change). A
    /// [`Placed::New`] app is pinned on the way and takes the place of the last shown pin's slot, which is the end of the file when it was
    /// appended. Pins that are not shown (their program is gone) keep their places. `before` are the names of the pins the island showed
    /// when the icon was picked up, in order: when the file's shown pins are not those (the list changed meanwhile, in the app menu or in
    /// another copy), nothing is written and the island reads the list again. False, and the list untouched, when nothing would change,
    /// when a position is used twice or is not there, or when an app that has to be pinned cannot be (it has no known way to start it).
    pub fn apply_order(&mut self, placed: &[Placed], before: &[String], exists: impl Fn(&Path) -> bool) -> bool {
        let alive = self.alive_indices(exists);
        if alive.len() != before.len() || alive.iter().zip(before).any(|(&i, name)| !same_name(&self.pins[i].name, name)) {
            return false;
        }
        let mut work = self.clone();
        // the file's index of every placed app; a new one is appended first
        let mut at: Vec<usize> = Vec::with_capacity(placed.len());
        for p in placed {
            let i = match p {
                Placed::Existing(n) => match alive.get(*n) {
                    Some(&i) => i,
                    None => return false,
                },
                Placed::New(app) => match pin_from_app(app) {
                    Some(pin) => {
                        work.pins.push(pin);
                        work.pins.len() - 1
                    }
                    None => return false,
                },
            };
            if at.contains(&i) {
                return false;
            }
            at.push(i);
        }
        // the places the shown pins held, left to right, are filled with them in the new order
        let mut slots = at.clone();
        slots.sort_unstable();
        let mut pins: Vec<OwnPin> = work.pins.clone();
        for (slot, &from) in slots.iter().zip(&at) {
            pins[*slot] = work.pins[from].clone();
        }
        if pins == self.pins {
            return false;
        }
        self.pins = pins;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::model::{IconSource, Launch};
    use std::path::PathBuf;

    fn app(name: &str, pinned: bool) -> DockApp {
        let exe = format!(r"C:\apps\{}.exe", name.to_lowercase());
        DockApp { key: exe.to_lowercase(), name: name.into(), pinned, launch: Some(Launch::Exe { path: PathBuf::from(&exe), args: String::new() }), icon: IconSource::File(PathBuf::from(exe)), windows: vec![], active: false }
    }

    fn file(names: &[&str]) -> PinFile {
        PinFile { version: 1, seeded: true, pins: names.iter().map(|n| pin_from_app(&app(n, true)).unwrap()).collect() }
    }

    fn names(f: &PinFile) -> Vec<String> {
        f.pins.iter().map(|p| p.name.clone()).collect()
    }

    fn every(_: &Path) -> bool {
        true
    }

    /// The names of the pins the island shows, which is what the island hands over as `before`.
    fn shown_names(f: &PinFile, exists: impl Fn(&Path) -> bool) -> Vec<String> {
        f.alive(exists).into_iter().map(|p| p.name).collect()
    }

    fn existing(order: &[usize]) -> Vec<Placed> {
        order.iter().map(|&i| Placed::Existing(i)).collect()
    }

    fn reorder(f: &mut PinFile, order: &[usize]) -> bool {
        let before = shown_names(f, every);
        f.apply_order(&existing(order), &before, every)
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_reorder_writes_the_shown_pins_in_their_new_order() {
        let mut f = file(&["A", "B", "C"]);
        assert!(reorder(&mut f, &[2, 0, 1]));
        assert_eq!(names(&f), ["C", "A", "B"]);
    }

    #[test]
    fn the_same_order_changes_nothing() {
        let mut f = file(&["A", "B", "C"]);
        assert!(!reorder(&mut f, &[0, 1, 2]));
        assert_eq!(names(&f), ["A", "B", "C"]);
    }

    #[test]
    fn a_pin_the_island_does_not_show_keeps_its_place() {
        // "H" is in the file but its program is gone: the island shows A, B, C
        let mut f = file(&["A", "H", "B", "C"]);
        let exists = |p: &Path| !p.ends_with("h.exe");
        let before = shown_names(&f, exists);
        assert_eq!(before, ["A", "B", "C"]);
        assert!(f.apply_order(&existing(&[2, 1, 0]), &before, exists));
        assert_eq!(names(&f), ["C", "H", "B", "A"], "the hidden pin stays second");
    }

    #[test]
    fn a_running_app_dropped_among_the_pinned_is_pinned_there() {
        let new = || Placed::New(Box::new(app("New", false)));
        let mut f = file(&["A", "B"]);
        let before = shown_names(&f, every);
        assert!(f.apply_order(&[Placed::Existing(0), new(), Placed::Existing(1)], &before, every));
        assert_eq!(names(&f), ["A", "New", "B"]);
        // and at the very front
        let mut f = file(&["A", "B"]);
        assert!(f.apply_order(&[new(), Placed::Existing(0), Placed::Existing(1)], &before, every));
        assert_eq!(names(&f), ["New", "A", "B"]);
    }

    #[test]
    fn a_new_pin_goes_among_the_shown_ones_and_leaves_the_hidden_ones_alone() {
        let mut f = file(&["H", "A", "B"]);
        let exists = |p: &Path| !p.ends_with("h.exe");
        let before = shown_names(&f, exists);
        assert!(f.apply_order(&[Placed::New(Box::new(app("New", false))), Placed::Existing(0), Placed::Existing(1)], &before, exists));
        assert_eq!(names(&f), ["H", "New", "A", "B"]);
    }

    #[test]
    fn an_app_that_cannot_be_started_is_not_pinned_and_nothing_changes() {
        let mut f = file(&["A", "B"]);
        let mut stuck = app("Stuck", false);
        stuck.launch = None;
        let before = shown_names(&f, every);
        assert!(!f.apply_order(&[Placed::New(Box::new(stuck)), Placed::Existing(0), Placed::Existing(1)], &before, every));
        assert_eq!(names(&f), ["A", "B"]);
    }

    #[test]
    fn a_position_used_twice_or_not_there_makes_no_change() {
        let mut f = file(&["A", "B"]);
        assert!(!reorder(&mut f, &[0, 0]));
        assert!(!reorder(&mut f, &[1, 5]));
        assert_eq!(names(&f), ["A", "B"]);
    }

    #[test]
    fn a_list_that_is_not_the_one_the_icon_was_picked_up_from_is_left_alone() {
        // the file changed meanwhile (another pin came, or the order was changed from the menu): the drop is not applied to it
        let mut f = file(&["A", "B", "C"]);
        assert!(!f.apply_order(&existing(&[2, 1, 0]), &strings(&["A", "C", "B"]), every));
        assert!(!f.apply_order(&existing(&[1, 0]), &strings(&["A", "B"]), every));
        assert_eq!(names(&f), ["A", "B", "C"]);
    }

    /// Two profiles of one browser: one program, other names and arguments. The island shows both and they can be put in either order
    /// (found in review: the first version went by `find`, which takes both for the first pin, and refused every reorder).
    #[test]
    fn two_pins_that_start_one_program_can_be_reordered() {
        let mk = |name: &str, args: &str| {
            let mut a = app("Chrome", true);
            a.name = name.into();
            a.launch = Some(Launch::Exe { path: PathBuf::from(r"C:\apps\chrome.exe"), args: args.into() });
            a
        };
        let (p, q) = (mk("Chrome Work", "--profile=w"), mk("Chrome Home", "--profile=h"));
        let mut f = PinFile { version: 1, seeded: true, pins: vec![pin_from_app(&p).unwrap(), pin_from_app(&q).unwrap(), pin_from_app(&app("Notes", true)).unwrap()] };
        assert!(reorder(&mut f, &[1, 0, 2]));
        assert_eq!(names(&f), ["Chrome Home", "Chrome Work", "Notes"]);
        assert!(reorder(&mut f, &[2, 0, 1]));
        assert_eq!(names(&f), ["Notes", "Chrome Home", "Chrome Work"]);
        // each keeps its own arguments
        assert!(matches!(&f.pins[1].launch, crate::dock::ownpins::LaunchRec::Exe { args, .. } if args == "--profile=h"));
        assert!(matches!(&f.pins[2].launch, crate::dock::ownpins::LaunchRec::Exe { args, .. } if args == "--profile=w"));
    }

    /// The whole way a drop takes: seeded from a made-up taskbar into a file in a temp folder of this test, reordered through
    /// `ownpins::change`, read back the way the island reads it, reordered again, and a change that changes nothing writes nothing.
    #[test]
    fn the_order_goes_through_the_pin_file_and_back() {
        use crate::dock::ownpins::{Changed, FILE, change, effective};
        let dir = std::env::temp_dir().join(format!("rf-pinorder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(FILE);
        let taskbar = || Some(["A", "B", "C"].iter().map(|n| pin_from_app(&app(n, true)).unwrap().to_pinned()).collect::<Vec<_>>());
        let order = |path: &std::path::Path| effective(path, |_| true).unwrap().into_iter().map(|p| p.name).collect::<Vec<_>>();

        // the first change seeds the file from the taskbar and puts C in front
        let first = existing(&[2, 0, 1]);
        assert_eq!(change(&path, taskbar, |p| p, |f| f.apply_order(&first, &strings(&["A", "B", "C"]), every)), Ok(Changed::Written));
        assert_eq!(order(&path), ["C", "A", "B"]);
        // a running app dropped between A and B is pinned there
        let second = vec![Placed::Existing(0), Placed::Existing(1), Placed::New(Box::new(app("New", false))), Placed::Existing(2)];
        assert_eq!(change(&path, taskbar, |p| p, |f| f.apply_order(&second, &strings(&["C", "A", "B"]), every)), Ok(Changed::Written));
        assert_eq!(order(&path), ["C", "A", "New", "B"]);
        // the same order again writes nothing
        let before = std::fs::read(&path).unwrap();
        let same = existing(&[0, 1, 2, 3]);
        assert_eq!(change(&path, taskbar, |p| p, |f| f.apply_order(&same, &strings(&["C", "A", "New", "B"]), every)), Ok(Changed::Same));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
