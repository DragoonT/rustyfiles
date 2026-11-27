//! The order of the command bar's buttons, the lines between them, and how each one is shown (issues 373, 374, 248, 258, 295).
//!
//! WHAT CAN BE ARRANGED: the buttons of the bar's left group (create, clipboard, item commands, "…", and in the Windows File Explorer
//! look Sort and View too), and lines ("sep") anywhere among them. The right group (filter, sort and view in the other look, the
//! preview pane) keeps its place, as Explorer's does. The user's own commands follow the left group.
//!
//! HOW IT IS KEPT (`Settings::toolbar_order`, `toolbar_look`): a list of names in order, with "sep" for a line; an empty list is the
//! default order. A name the list does not have stands in its default place at the end; a name the look does not use is skipped.
//! A button's look is "name=icon", "name=text" or "name=both"; a button not named there is drawn as it always was.
//!
//! Everything here is pure.

/// A line between two buttons, in the saved order.
pub const SEP: &str = "sep";

/// The left group in the ordinary look, as it always stood.
/// (the three selection commands, issue 406, and the View menu's switches, issue 405, are off unless asked for, as Undo and Redo)
pub const DEFAULT: [&str; 23] = [
    "new", "newfolder", SEP, "cut", "copy", "paste", "rename", "share", "delete", "props", "edit", "terminal", "hidden", "extensions", "dual", "shelf", "refresh",
    "undo", "redo", "selectall", "selectnone", "invertsel", "more",
];

/// The left group in the Windows File Explorer look (Sort and View stand in it there, Properties at its end).
pub const DEFAULT_EXPLORER: [&str; 26] = [
    "new", "newfolder", SEP, "cut", "copy", "paste", "rename", "share", "delete", "edit", "terminal", "hidden", "extensions", "dual", "shelf", "refresh", "undo",
    "redo", "selectall", "selectnone", "invertsel", SEP, "sort", "view", "more", "props",
];

fn default_of(explorer: bool) -> &'static [&'static str] {
    if explorer { &DEFAULT_EXPLORER } else { &DEFAULT }
}

/// The left group's order for this look: the saved names that the look has (each button once, lines as saved), then the buttons the
/// saved list does not name, in their default order. An empty list is the default order with its lines.
pub fn order(saved: &[String], explorer: bool) -> Vec<String> {
    let default = default_of(explorer);
    if saved.is_empty() {
        return default.iter().map(|s| s.to_string()).collect();
    }
    let mut out: Vec<String> = Vec::new();
    for name in saved {
        let known = name == SEP || default.contains(&name.as_str());
        if known && (name == SEP || !out.contains(name)) {
            out.push(name.clone());
        }
    }
    for name in default.iter().filter(|n| **n != SEP) {
        if !out.iter().any(|o| o == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// The order after the entry at `ix` moved one place up (towards the start) or down. Out of range: unchanged.
pub fn moved(order: &[String], ix: usize, up: bool) -> Vec<String> {
    let mut v = order.to_vec();
    let other = if up { ix.checked_sub(1) } else { Some(ix + 1) };
    if let Some(j) = other.filter(|&j| j < v.len() && ix < v.len()) {
        v.swap(ix, j);
    }
    v
}

/// The order with a line added at the end (the user moves it where they want it).
pub fn with_line(order: &[String]) -> Vec<String> {
    let mut v = order.to_vec();
    v.push(SEP.to_string());
    v
}

/// The order without the entry at `ix` (a line the user takes away).
pub fn without(order: &[String], ix: usize) -> Vec<String> {
    let mut v = order.to_vec();
    if ix < v.len() {
        v.remove(ix);
    }
    v
}

/// What is drawn, in order, from what each entry gives (`None` for a line): no line at either end and never two together, so a
/// line next to a hidden button does not stand alone.
pub fn tidy<T>(items: Vec<Option<T>>) -> Vec<Option<T>> {
    let mut out: Vec<Option<T>> = Vec::with_capacity(items.len());
    for it in items {
        if it.is_none() && out.last().is_none_or(|l: &Option<T>| l.is_none()) {
            continue;
        }
        out.push(it);
    }
    while out.last().is_some_and(|l| l.is_none()) {
        out.pop();
    }
    out
}

/// How a button is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Look {
    /// As it always was (most are a picture; New and, in a wide window of the Explorer look, Sort and View have their word too).
    Auto,
    Icon,
    Text,
    Both,
}

impl Look {
    fn word(self) -> Option<&'static str> {
        match self {
            Look::Auto => None,
            Look::Icon => Some("icon"),
            Look::Text => Some("text"),
            Look::Both => Some("both"),
        }
    }

    /// Whether the picture is drawn, given what the button did before (`auto_icon`).
    pub fn icon(self) -> bool {
        self != Look::Text
    }

    /// Whether the word is drawn, given whether the button had it before (`auto_word`).
    pub fn word_shown(self, auto_word: bool) -> bool {
        match self {
            Look::Auto => auto_word,
            Look::Icon => false,
            Look::Text | Look::Both => true,
        }
    }
}

/// The look saved for button `name`.
pub fn look_of(saved: &[String], name: &str) -> Look {
    saved
        .iter()
        .filter_map(|e| e.split_once('='))
        .find(|(n, _)| *n == name)
        .map(|(_, l)| match l {
            "icon" => Look::Icon,
            "text" => Look::Text,
            "both" => Look::Both,
            _ => Look::Auto,
        })
        .unwrap_or(Look::Auto)
}

/// The saved looks with button `name` set to `look` (Auto takes it out).
pub fn with_look(saved: &[String], name: &str, look: Look) -> Vec<String> {
    let mut v: Vec<String> = saved.iter().filter(|e| e.split_once('=').is_none_or(|(n, _)| n != name)).cloned().collect();
    if let Some(w) = look.word() {
        v.push(format!("{name}={w}"));
    }
    v
}

/// The bar's pictures, px: small, medium (as they always were) and large.
pub fn icon_px(size: crate::settings::ToolbarIconSize) -> f32 {
    match size {
        crate::settings::ToolbarIconSize::Small => 14.0,
        crate::settings::ToolbarIconSize::Medium => 16.0,
        crate::settings::ToolbarIconSize::Large => 20.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn an_empty_list_is_the_default_order_and_a_saved_one_keeps_every_button_once() {
        assert_eq!(order(&[], false), s(&DEFAULT));
        assert_eq!(order(&[], true), s(&DEFAULT_EXPLORER));
        let o = order(&s(&["delete", "sep", "cut", "cut", "nonsense", "sort"]), false);
        assert_eq!(&o[..3], &s(&["delete", "sep", "cut"])[..], "the saved part, each button once, unknown names and Sort (not in this look) out");
        for name in DEFAULT.iter().filter(|n| **n != SEP) {
            assert_eq!(o.iter().filter(|x| x == name).count(), 1, "{name} once");
        }
        assert!(order(&s(&["sort"]), true).starts_with(&s(&["sort"])), "Sort is the Explorer look's");
    }

    #[test]
    fn entries_move_one_place_and_lines_come_and_go() {
        let o = s(&["a", "b", "c"]);
        assert_eq!(moved(&o, 1, true), s(&["b", "a", "c"]));
        assert_eq!(moved(&o, 1, false), s(&["a", "c", "b"]));
        assert_eq!(moved(&o, 0, true), o, "the first one cannot go up");
        assert_eq!(moved(&o, 2, false), o, "the last one cannot go down");
        assert_eq!(with_line(&o), s(&["a", "b", "c", "sep"]));
        assert_eq!(without(&s(&["a", "sep", "b"]), 1), s(&["a", "b"]));
    }

    #[test]
    fn lines_never_stand_at_the_ends_or_together() {
        let got = tidy(vec![None, Some(1), None, None, Some(2), None]);
        assert_eq!(got, vec![Some(1), None, Some(2)]);
        assert_eq!(tidy::<u8>(vec![None, None]), vec![]);
    }

    #[test]
    fn a_buttons_look_is_saved_by_its_name() {
        let saved = with_look(&[], "cut", Look::Both);
        assert_eq!(look_of(&saved, "cut"), Look::Both);
        assert_eq!(look_of(&saved, "copy"), Look::Auto);
        let saved = with_look(&saved, "cut", Look::Text);
        assert_eq!(saved, s(&["cut=text"]), "one entry per button");
        assert_eq!(with_look(&saved, "cut", Look::Auto), Vec::<String>::new());
        assert!(!Look::Text.icon() && Look::Text.word_shown(false));
        assert!(Look::Auto.word_shown(true) && !Look::Icon.word_shown(true));
    }
}
