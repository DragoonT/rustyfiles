//! The order of the notification icons in the dock's tray strip, and which of them the user moved into the overflow panel (shell plan
//! increment D3). Pure, and the file form: `tray-order.json` in the config folder.
//!
//! WHAT AN ICON IS KNOWN BY. Never by its tip text (the tip changes with the program's state: "3 new messages"), never by a window handle
//! (it dies with the program). By its GUID when the program gave one (`NIF_GUID`), otherwise by the program's path and the icon's id
//! (`uID`) that the program chose. A packaged app gets a new GUID with every version (the research note counts 38 of 98 settings keys
//! being one app's old versions), so such an icon is a NEW icon after an update and appears at the end of the strip once: that is a
//! limit of the identity, not of this file.
//!
//! WHAT IS KEPT. One list, `order`: every icon the strip has ever shown, in the user's order, strip and overflow alike; and `hidden`: the
//! ones the user moved into the overflow. An icon that is not there at the moment (its program is not running) keeps its place, so
//! it comes back where it was. The list is cut at [`MAX_REMEMBERED`], the oldest absent ones first.
//!
//! The file is written only when the user changes something (never for reading), whole to a temporary file and then renamed over the
//! old one. A file that exists and cannot be read is moved aside, never written over (it may be the user's list, half written or from
//! a newer version).

use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file's name in the config folder.
pub const FILE: &str = "tray-order.json";
/// The file's version. A file of a newer version is not understood and is left alone.
pub const VERSION: u32 = 1;
/// How many icons the file remembers.
pub const MAX_REMEMBERED: usize = 256;

/// The identity an icon is remembered by: `guid:{...}` (lower case) or `exe:<lower case path>|<id>`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StableId(String);

fn is_guid_body(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36 && b.iter().enumerate().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { *c == b'-' } else { c.is_ascii_hexdigit() })
}

impl StableId {
    /// From a GUID as text, with or without braces, in any case. `None` for anything that is not a GUID and for the all-zero GUID
    /// (a program that set `NIF_GUID` with nothing in it did not give one).
    pub fn from_guid(guid: &str) -> Option<StableId> {
        let t = guid.trim();
        let body = t.strip_prefix('{').and_then(|r| r.strip_suffix('}')).unwrap_or(t);
        if !is_guid_body(body) || body.bytes().all(|c| c == b'0' || c == b'-') {
            return None;
        }
        Some(StableId(format!("guid:{{{}}}", body.to_lowercase())))
    }

    /// From the program's path and the icon's id: the path in lower case with backslashes (Windows paths are case blind), the id as the
    /// program gave it.
    pub fn from_program(exe: &str, id: u32) -> StableId {
        StableId(format!("exe:{}|{id}", exe.trim().replace('/', "\\").to_lowercase()))
    }

    /// The identity from a tray host's own facts: the GUID when there is a usable one, else the program and the id.
    pub fn of(guid: Option<&str>, exe: &str, id: u32) -> StableId {
        guid.and_then(StableId::from_guid).unwrap_or_else(|| StableId::from_program(exe, id))
    }

    /// An identity read back from the file; `None` for anything this module could not have written.
    pub fn from_stored(s: &str) -> Option<StableId> {
        if let Some(rest) = s.strip_prefix("guid:") {
            let id = StableId::from_guid(rest)?;
            return (id.0 == s).then_some(id);
        }
        let rest = s.strip_prefix("exe:")?;
        let (path, id) = rest.rsplit_once('|')?;
        let id_ok = !id.is_empty() && id.bytes().all(|c| c.is_ascii_digit()) && id.parse::<u32>().is_ok();
        (id_ok && path == path.to_lowercase() && !path.contains('/') && path.trim() == path).then(|| StableId(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where an icon is moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// The strip.
    Strip,
    /// The overflow panel (hidden by the user).
    Hidden,
}

/// The icons that are there now, arranged: all of them in the user's order, and which of them the user hid. Indexes are into the list the
/// caller gave (the snapshot's order).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Arrangement {
    /// Every present icon, in the user's order (new ones at the end, in the snapshot's order).
    pub all: Vec<usize>,
    /// `hidden[i]` is true when present icon `i` was moved into the overflow by the user.
    pub hidden: Vec<bool>,
}

impl Arrangement {
    /// The icons the user left in the strip, in order.
    pub fn strip(&self) -> Vec<usize> {
        self.all.iter().copied().filter(|&i| !self.hidden[i]).collect()
    }

    /// The icons the user moved into the overflow, in order.
    pub fn hidden_list(&self) -> Vec<usize> {
        self.all.iter().copied().filter(|&i| self.hidden[i]).collect()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TrayOrder {
    order: Vec<StableId>,
    hidden: Vec<StableId>,
}

impl TrayOrder {
    pub fn new() -> TrayOrder {
        TrayOrder::default()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty() && self.hidden.is_empty()
    }

    fn is_hidden(&self, id: &StableId) -> bool {
        self.hidden.contains(id)
    }

    /// The icons that are there now (`present`, in the order the tray host listed them), arranged by the user's list. An icon the list has
    /// never seen goes to the end; ties keep the host's order.
    pub fn arrange(&self, present: &[StableId]) -> Arrangement {
        let mut rank: HashMap<&StableId, usize> = HashMap::with_capacity(self.order.len());
        for (i, id) in self.order.iter().enumerate() {
            rank.entry(id).or_insert(i);
        }
        let mut all: Vec<usize> = (0..present.len()).collect();
        all.sort_by_key(|&i| (rank.get(&present[i]).copied().unwrap_or(usize::MAX), i));
        let hidden = present.iter().map(|id| self.is_hidden(id)).collect();
        Arrangement { all, hidden }
    }

    /// Moves present icon `who` to `dest`, at place `at` of that list (counted without the icon itself; past the end is the end).
    /// Moving into the strip shows an icon the user had hidden. Returns whether anything changed (the caller writes the file only then).
    pub fn reorder(&mut self, present: &[StableId], who: usize, dest: Dest, at: usize) -> bool {
        if who >= present.len() {
            return false;
        }
        let arr = self.arrange(present);
        let before = arr.clone();
        let mut strip: Vec<usize> = arr.strip();
        let mut hid: Vec<usize> = arr.hidden_list();
        strip.retain(|&i| i != who);
        hid.retain(|&i| i != who);
        let list = if dest == Dest::Strip { &mut strip } else { &mut hid };
        list.insert(at.min(list.len()), who);

        // the new order of the icons that are there: the strip, then the overflow
        let fresh: Vec<StableId> = strip.iter().chain(hid.iter()).map(|&i| present[i].clone()).collect();
        let present_set: HashSet<&StableId> = present.iter().collect();
        let mut next = fresh.iter();
        let mut order: Vec<StableId> = Vec::with_capacity(self.order.len().max(fresh.len()));
        let mut seen: HashSet<StableId> = HashSet::new();
        for id in &self.order {
            if present_set.contains(id) {
                // a present icon's place is taken by the next one of the new order
                for n in next.by_ref() {
                    if seen.insert(n.clone()) {
                        order.push(n.clone());
                        break;
                    }
                }
            } else if seen.insert(id.clone()) {
                order.push(id.clone());
            }
        }
        for n in next {
            if seen.insert(n.clone()) {
                order.push(n.clone());
            }
        }
        self.order = order;

        let id = &present[who];
        self.hidden.retain(|h| h != id);
        if dest == Dest::Hidden {
            self.hidden.push(id.clone());
        }
        self.trim(&present_set);
        // what counts as a change is what the user sees: the first move of a fresh list that puts everything where it was is none (and
        // writes no file)
        self.arrange(present) != before
    }

    /// Keeps the list at [`MAX_REMEMBERED`]: icons that are not there now go first, the oldest first.
    fn trim(&mut self, present: &HashSet<&StableId>) {
        let mut over = self.order.len().saturating_sub(MAX_REMEMBERED);
        if over > 0 {
            self.order.retain(|id| {
                if over > 0 && !present.contains(id) {
                    over -= 1;
                    false
                } else {
                    true
                }
            });
        }
        let kept: HashSet<&StableId> = self.order.iter().collect();
        self.hidden.retain(|h| kept.contains(h) || present.contains(h));
        self.hidden.truncate(MAX_REMEMBERED);
    }

    // ---- the file

    pub fn to_json(&self) -> String {
        let file = FileForm { version: VERSION, order: self.order.iter().map(|i| i.0.clone()).collect(), hidden: self.hidden.iter().map(|i| i.0.clone()).collect() };
        serde_json::to_string_pretty(&file).unwrap_or_else(|_| "{}".into()) + "\n"
    }

    /// Reads the file's text. `None`: not JSON of this shape, or a newer version than this code knows. Entries this module could not
    /// have written are dropped, repeats are dropped, a hidden icon that is not in the order is added to it.
    pub fn parse(text: &str) -> Option<TrayOrder> {
        let file: FileForm = serde_json::from_str(text).ok()?;
        if file.version > VERSION {
            return None;
        }
        let mut seen = HashSet::new();
        let mut order: Vec<StableId> = file.order.iter().filter_map(|s| StableId::from_stored(s)).filter(|i| seen.insert(i.clone())).collect();
        let mut seen_h = HashSet::new();
        let hidden: Vec<StableId> = file.hidden.iter().filter_map(|s| StableId::from_stored(s)).filter(|i| seen_h.insert(i.clone())).collect();
        for h in &hidden {
            if seen.insert(h.clone()) {
                order.push(h.clone());
            }
        }
        order.truncate(MAX_REMEMBERED);
        Some(TrayOrder { order, hidden })
    }
}

#[derive(Serialize, Deserialize)]
struct FileForm {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    order: Vec<String>,
    #[serde(default)]
    hidden: Vec<String>,
}

/// What the order file turned out to be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Loaded {
    /// There is none: the host's order stands.
    Absent,
    /// There is one and it could not be read (locked for a moment, damaged, a newer version): it is set aside before anything is written.
    Unreadable,
    Found(TrayOrder),
}

pub fn load_checked(path: &Path) -> Loaded {
    match std::fs::read_to_string(path) {
        Ok(text) => TrayOrder::parse(&text).map_or(Loaded::Unreadable, Loaded::Found),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Loaded::Absent,
        Err(_) => Loaded::Unreadable,
    }
}

/// Moves an unreadable file out of the way (`tray-order.json.broken`, then `.broken2` ... never over an earlier one).
pub fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    for n in 1..100 {
        let name = if n == 1 { "broken".to_string() } else { format!("broken{n}") };
        let to = path.with_extension(format!("json.{name}"));
        if !to.exists() {
            std::fs::rename(path, &to)?;
            return Ok(to);
        }
    }
    Err(std::io::Error::other("too many set-aside tray order files"))
}

/// Writes the file whole to a temporary file next to it and renames it over the old one, so a crash in the middle leaves the old list.
pub fn save(path: &Path, order: &TrayOrder) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(order.to_json().as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(n: u8) -> StableId {
        StableId::from_guid(&format!("{{{n:08x}-0000-4000-8000-000000000001}}")).unwrap()
    }
    fn p(name: &str, id: u32) -> StableId {
        StableId::from_program(&format!("C:\\Apps\\{name}.exe"), id)
    }

    #[test]
    fn a_guid_is_known_in_one_spelling() {
        let a = StableId::from_guid("{7820AE73-23E3-4229-82C1-E41CB67D5B9C}").unwrap();
        let b = StableId::from_guid("7820ae73-23e3-4229-82c1-e41cb67d5b9c").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "guid:{7820ae73-23e3-4229-82c1-e41cb67d5b9c}");
        // not a GUID, and the empty GUID
        assert!(StableId::from_guid("").is_none());
        assert!(StableId::from_guid("{1234}").is_none());
        assert!(StableId::from_guid("{zzzzzzzz-23e3-4229-82c1-e41cb67d5b9c}").is_none());
        assert!(StableId::from_guid("{00000000-0000-0000-0000-000000000000}").is_none());
    }

    #[test]
    fn a_program_and_its_id_are_known_in_one_spelling() {
        let a = StableId::from_program("C:\\Program Files\\Foo\\Foo.EXE", 7);
        let b = StableId::from_program(" c:/program files/foo/foo.exe ", 7);
        assert_eq!(a, b);
        assert_ne!(a, StableId::from_program("c:\\program files\\foo\\foo.exe", 8), "another icon of the same program");
        assert_eq!(a.as_str(), "exe:c:\\program files\\foo\\foo.exe|7");
    }

    #[test]
    fn the_guid_wins_over_the_program_when_there_is_a_usable_one() {
        let with = StableId::of(Some("{7820AE73-23E3-4229-82C1-E41CB67D5B9C}"), "c:\\a.exe", 1);
        assert!(with.as_str().starts_with("guid:"));
        let zero = StableId::of(Some("{00000000-0000-0000-0000-000000000000}"), "c:\\a.exe", 1);
        assert_eq!(zero, StableId::from_program("c:\\a.exe", 1));
        assert_eq!(StableId::of(None, "c:\\a.exe", 1), zero);
    }

    #[test]
    fn what_the_module_wrote_it_reads_back_and_nothing_else() {
        for id in [g(1), p("Foo", 3), StableId::from_program("", 0)] {
            assert_eq!(StableId::from_stored(id.as_str()), Some(id.clone()), "{id:?}");
        }
        for bad in ["", "guid:", "guid:{1234}", "GUID:{00000001-0000-4000-8000-000000000001}", "exe:c:\\a.exe", "exe:c:\\a.exe|x", "exe:C:\\A.exe|1", "exe:c:/a.exe|1", "tip:3 new messages", "exe:c:\\a.exe|99999999999"] {
            assert_eq!(StableId::from_stored(bad), None, "{bad:?}");
        }
        // a GUID in the wrong case is not what was written
        assert_eq!(StableId::from_stored("guid:{7820AE73-23E3-4229-82C1-E41CB67D5B9C}"), None);
    }

    /// Two icons with the same identity (a program started twice that gives both the same id) are one icon to the order: they are placed
    /// and hidden together, a drag of one over the other changes nothing (and so writes nothing), and nothing panics.
    #[test]
    fn icons_with_the_same_identity_share_one_place_and_one_hiding() {
        let present = vec![p("Twin", 1), p("Solo", 2), p("Twin", 1), g(3)];
        let mut o = TrayOrder::new();
        // moving the second twin to the front moves the identity: both twins go before the others
        assert!(o.reorder(&present, 2, Dest::Strip, 0));
        let arr = o.arrange(&present);
        assert_eq!(arr.all, vec![0, 2, 1, 3], "the twins stay together, in the host's order, in front");
        // moving one twin to the place the pair already has changes nothing
        assert!(!o.reorder(&present, 0, Dest::Strip, 0));
        assert!(!o.reorder(&present, 2, Dest::Strip, 1), "a drag of one twin over the other is no change");
        // hiding one hides both (they are the same icon to the file)
        assert!(o.reorder(&present, 0, Dest::Hidden, usize::MAX));
        assert_eq!(o.arrange(&present).hidden, vec![true, false, true, false]);
        // and the file holds the identity once
        let back = TrayOrder::parse(&o.to_json()).unwrap();
        assert_eq!(back.arrange(&present), o.arrange(&present));
    }

    /// An identity with no program path (`exe:|n`) is what a host that could not resolve the path makes; every such icon with the same id is
    /// one icon. It is a valid identity (it reads back), and it is the host's job not to produce it (see `tray_source.rs`).
    #[test]
    fn path_less_identities_collide_by_id_alone() {
        let a = StableId::from_program("", 4);
        let b = StableId::from_program("  ", 4);
        assert_eq!(a, b);
        assert_eq!(a.as_str(), "exe:|4");
        assert_eq!(StableId::from_stored(a.as_str()), Some(a.clone()));
        let present = vec![a.clone(), p("Other", 4), b];
        let mut o = TrayOrder::new();
        assert!(o.reorder(&present, 0, Dest::Hidden, usize::MAX));
        assert_eq!(o.arrange(&present).hidden, vec![true, false, true], "both path-less icons are hidden, the one with a path is not");
    }

    #[test]
    fn an_empty_list_leaves_the_hosts_order() {
        let order = TrayOrder::new();
        let present = vec![g(1), g(2), g(3)];
        let a = order.arrange(&present);
        assert_eq!(a.all, vec![0, 1, 2]);
        assert_eq!(a.strip(), vec![0, 1, 2]);
        assert!(a.hidden_list().is_empty());
    }

    #[test]
    fn the_users_order_wins_and_new_icons_go_to_the_end() {
        let mut order = TrayOrder::new();
        let present = vec![g(1), g(2), g(3)];
        assert!(order.reorder(&present, 2, Dest::Strip, 0));
        assert_eq!(order.arrange(&present).strip(), vec![2, 0, 1]);
        // a fourth icon appears: at the end
        let more = vec![g(1), g(2), g(3), g(4)];
        assert_eq!(order.arrange(&more).strip(), vec![2, 0, 1, 3]);
        // the host lists them in another order next time: the user's order still wins
        let shuffled = vec![g(3), g(4), g(1), g(2)];
        let a = order.arrange(&shuffled);
        assert_eq!(a.strip().iter().map(|&i| shuffled[i].clone()).collect::<Vec<_>>(), vec![g(3), g(1), g(2), g(4)]);
    }

    #[test]
    fn moving_to_the_place_it_already_has_changes_nothing() {
        let mut order = TrayOrder::new();
        let present = vec![g(1), g(2), g(3)];
        assert!(!order.reorder(&present, 1, Dest::Strip, 1), "the place it already has");
        assert_eq!(order.arrange(&present).strip(), vec![0, 1, 2]);
        // after one real change, the same move again is no change at all
        assert!(order.reorder(&present, 0, Dest::Strip, 2));
        assert!(!order.reorder(&present, 0, Dest::Strip, 2));
        assert!(!order.reorder(&present, 0, Dest::Strip, 99), "past the end is the end");
        assert!(!order.reorder(&present, 9, Dest::Strip, 0), "no such icon");
    }

    #[test]
    fn an_icon_moved_to_the_overflow_is_hidden_and_one_moved_out_is_shown() {
        let mut order = TrayOrder::new();
        let present = vec![g(1), g(2), g(3)];
        assert!(order.reorder(&present, 1, Dest::Hidden, usize::MAX));
        let a = order.arrange(&present);
        assert_eq!(a.strip(), vec![0, 2]);
        assert_eq!(a.hidden_list(), vec![1]);
        assert!(a.hidden[1]);
        // back into the strip, in front of the first
        assert!(order.reorder(&present, 1, Dest::Strip, 0));
        let a = order.arrange(&present);
        assert_eq!(a.strip(), vec![1, 0, 2]);
        assert!(a.hidden_list().is_empty());
        assert!(order.hidden.is_empty());
    }

    #[test]
    fn the_overflow_keeps_its_own_order() {
        let mut order = TrayOrder::new();
        let present = vec![g(1), g(2), g(3), g(4)];
        order.reorder(&present, 0, Dest::Hidden, usize::MAX);
        order.reorder(&present, 2, Dest::Hidden, usize::MAX);
        order.reorder(&present, 3, Dest::Hidden, 0);
        assert_eq!(order.arrange(&present).hidden_list(), vec![3, 0, 2]);
        assert_eq!(order.arrange(&present).strip(), vec![1]);
    }

    #[test]
    fn an_icon_that_is_not_there_keeps_its_place_and_its_hiding() {
        let mut order = TrayOrder::new();
        let all = vec![g(1), g(2), g(3)];
        order.reorder(&all, 1, Dest::Hidden, usize::MAX);
        order.reorder(&all, 2, Dest::Strip, 0);
        // the second program quits: the others are arranged without it
        let some = vec![g(1), g(3)];
        assert_eq!(order.arrange(&some).strip(), vec![1, 0]);
        // a change among the others does not forget it
        order.reorder(&some, 0, Dest::Strip, 0);
        assert!(order.order.contains(&g(2)));
        // and when it comes back it is still in the overflow
        let back = order.arrange(&all);
        assert!(back.hidden[1]);
        assert_eq!(back.hidden_list(), vec![1]);
        assert_eq!(back.strip(), vec![0, 2]);
    }

    #[test]
    fn the_tip_text_plays_no_part() {
        // two icons of one program with different ids, and the same icon with a changed tip: only the identity counts
        let mut order = TrayOrder::new();
        let present = vec![p("Mail", 1), p("Mail", 2)];
        order.reorder(&present, 1, Dest::Strip, 0);
        assert_eq!(order.arrange(&present).strip(), vec![1, 0]);
        // (the host says "3 new messages" instead of "1 new message": the same identity)
        let again = vec![p("Mail", 1), p("Mail", 2)];
        assert_eq!(order.arrange(&again).strip(), vec![1, 0]);
    }

    #[test]
    fn the_list_is_kept_to_its_size_and_the_icons_that_are_there_stay() {
        let mut order = TrayOrder::new();
        let many: Vec<StableId> = (0..(MAX_REMEMBERED as u32 + 40)).map(|i| p("App", i)).collect();
        // all of them present: nothing may be dropped that is there
        for i in 0..many.len() {
            order.reorder(&many, i, Dest::Strip, i);
        }
        assert!(order.order.len() >= MAX_REMEMBERED);
        // now only a few are present: the list shrinks to its size, from the absent ones
        let few: Vec<StableId> = many.iter().take(5).cloned().collect();
        order.reorder(&few, 0, Dest::Hidden, 0);
        assert!(order.order.len() <= MAX_REMEMBERED);
        for f in &few {
            assert!(order.order.contains(f), "an icon that is there is never trimmed");
        }
    }

    #[test]
    fn the_file_round_trips_and_is_tolerant() {
        let mut order = TrayOrder::new();
        let present = vec![g(1), p("Mail", 4), g(3)];
        order.reorder(&present, 2, Dest::Strip, 0);
        order.reorder(&present, 1, Dest::Hidden, usize::MAX);
        let text = order.to_json();
        assert!(text.contains("\"version\": 1"));
        assert_eq!(TrayOrder::parse(&text), Some(order.clone()));
        // junk entries and repeats are dropped, a hidden icon that is not in the order is added to it
        let messy = r#"{"version":1,"order":["guid:{00000003-0000-4000-8000-000000000001}","tip:hello","guid:{00000003-0000-4000-8000-000000000001}"],"hidden":["exe:c:\\a.exe|1","???"]}"#;
        let t = TrayOrder::parse(messy).unwrap();
        assert_eq!(t.order, vec![g(3), StableId::from_program("c:\\a.exe", 1)]);
        assert_eq!(t.hidden, vec![StableId::from_program("c:\\a.exe", 1)]);
        // missing fields are empty; a file of a newer version, or no JSON, is not understood
        assert_eq!(TrayOrder::parse("{}"), Some(TrayOrder::new()));
        assert_eq!(TrayOrder::parse("{\"version\":2,\"order\":[]}"), None);
        assert_eq!(TrayOrder::parse("not json"), None);
        assert_eq!(TrayOrder::parse(""), None);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rf-trayorder-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_file_is_written_whole_and_read_back_and_a_missing_one_is_absent() {
        let dir = tmp("save");
        let path = dir.join(FILE);
        assert_eq!(load_checked(&path), Loaded::Absent);
        let mut order = TrayOrder::new();
        let present = vec![g(1), g(2)];
        order.reorder(&present, 1, Dest::Strip, 0);
        save(&path, &order).unwrap();
        assert_eq!(load_checked(&path), Loaded::Found(order.clone()));
        // no temporary file is left
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec![FILE.to_string()]);
        // written again over the old one
        order.reorder(&present, 0, Dest::Hidden, 0);
        save(&path, &order).unwrap();
        assert_eq!(load_checked(&path), Loaded::Found(order));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_file_that_cannot_be_read_is_set_aside_and_never_written_over() {
        let dir = tmp("aside");
        let path = dir.join(FILE);
        std::fs::write(&path, "{ half written").unwrap();
        assert_eq!(load_checked(&path), Loaded::Unreadable);
        let moved = set_aside(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{ half written");
        // a second one goes next to the first, not over it
        std::fs::write(&path, "again").unwrap();
        let second = set_aside(&path).unwrap();
        assert_ne!(moved, second);
        assert_eq!(std::fs::read_to_string(&moved).unwrap(), "{ half written");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
