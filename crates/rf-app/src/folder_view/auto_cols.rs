//! Details columns that are as wide as what they show.
//!
//! WHY (the user, 2026-11-15, with a picture of the header of a list some 900 px wide: "burayı daha mantıklı dinamik olacak şekilde
//! yapalım. boşlukları vs. vs. rahatsız etmesin yani insanları."): every column had one width whatever stood in it, 150 px for a
//! date, 150 for the type, 130 for the tags, 100 for the size. So "Tür" was followed by a hole before the right-aligned size, the tag
//! column was a stretch of nothing in a folder without a tagged file, and the name, the one column that never has enough room, was
//! squeezed to pay for it.
//!
//! WHAT IS DONE. A column that nobody has dragged is AUTOMATIC: its width is its widest text in this listing (measured with the font
//! and size the list really uses), never less than its heading needs, plus the same air towards the next column for every column.
//! What the name column gets is everything that is left. And the tag column is shown only in a folder that has a tagged item, the
//! way the Git column is shown only inside a repository.
//!
//! WHICH COLUMN IS AUTOMATIC: one whose stored width is still the stock one ([`STOCK_W`], `ColumnPrefs::default()`'s). Dragging a
//! column's edge stores a width for THAT column alone and it stays that wide; a double click on the edge gives it back (the stock
//! width is stored again). No new setting and no new text: a settings file from an older version reads as "all automatic" unless
//! its owner had dragged something.
//!
//! WHAT IT COSTS: nothing per frame beyond comparing a key. The widths are measured again when the listing, the language, the date
//! or size style, the font or the repository's status changes. Dates and sizes are measured from the widest forms their formatters
//! can give, not from the entries; the type column looks at the extensions of at most [`SCAN`] entries; the tag column asks the tag
//! store for this folder's own items, and the Git column asks the status which states this folder's items have.

use std::collections::HashSet;
use std::sync::Arc;

use gpui_kit::*;
use rf_core::format;

use super::FolderView;
use crate::icons;
use crate::t;

/// The width of a column until somebody drags it: modified, created, type, size, tags, git, sync, in the order of
/// `FolderView::col_widths`. They are `ColumnPrefs::default()`'s (a test holds the two together).
pub(super) const STOCK_W: [f32; 7] = [150., 150., 150., 100., 130., 120., 150.];
/// Minimum automatic widths for the columns shown in the Explorer look's Details layout.
const EXPLORER_MIN: [f32; 4] = [168., 168., 132., 90.];
/// The columns measured here. The sync column (an icon and a state that few folders have) keeps its stored width.
const MEASURED: [bool; 7] = [true, true, true, true, true, true, false];

/// A cell's and a heading's own padding (`px_2` on both sides).
const PAD: f32 = 16.;
/// The air between a column's widest text and the next column. With the two paddings that meet there, 30 px from text to text.
const AIR: f32 = 14.;
/// A heading's sort arrow with its gap.
const ARROW: f32 = 16.;
const NARROWEST: f32 = 56.;
const WIDEST: f32 = 300.;
/// How many entries the type column looks at. Past that a rarer, longer type may be cut with an ellipsis; a folder that large is
/// not asked for one more pass over all of it.
const SCAN: usize = 20_000;
/// How many different extensions are measured one by one.
const KINDS: usize = 48;

/// The width of an automatic column: for its widest text and for its heading, whichever needs more. `air`: the room towards the
/// next column (none for a right-aligned column, whose text ends at the column's end). Pure.
pub(super) fn fit(content: f32, heading: f32, air: f32) -> f32 {
    ((content + air).max(heading + ARROW) + PAD).ceil().clamp(NARROWEST, WIDEST)
}

/// Whether the column at `ix` (of `FolderView::col_widths`) is automatic, from its stored width. Pure.
pub(super) fn automatic(ix: usize, stored: f32) -> bool {
    ix < STOCK_W.len() && MEASURED[ix] && stored == STOCK_W[ix]
}

/// The width to store for a column the user dragged to `width`: never the stock width itself, which means "automatic". Pure.
pub(super) fn kept(ix: usize, width: f32) -> u16 {
    let w = width.round();
    (if automatic(ix, w) { w + 1. } else { w }) as u16
}

/// What this listing's automatic columns measure.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) struct AutoCols {
    /// The measured width per column (`None`: not measured, the stored width holds).
    pub w: [Option<f32>; 7],
    /// Some item of this folder has a tag: the tag column has something to show.
    pub tags_any: bool,
}

impl AutoCols {
    /// The stored widths with every automatic column at its measured width. Pure.
    pub(super) fn apply(&self, mut stored: [f32; 11]) -> [f32; 11] {
        for ix in 0..STOCK_W.len() {
            if let (true, Some(w)) = (automatic(ix, stored[ix]), self.w[ix]) {
                stored[ix] = w;
            }
        }
        stored
    }
}

/// The kinds of items a listing has, as far as the type column's width goes.
#[derive(Debug, Default, PartialEq)]
pub(super) struct Kinds {
    pub dir: bool,
    /// The first [`KINDS`] different extensions ("" for a file without one).
    pub exts: Vec<Arc<str>>,
    /// The longest extension among those that came after: measured too, as the likeliest to be wider than the rest.
    pub longest_more: Option<Arc<str>>,
}

/// Looks at (is a folder, extension) of the entries. Pure.
pub(super) fn kinds<'a>(entries: impl Iterator<Item = (bool, &'a Arc<str>)>) -> Kinds {
    let mut k = Kinds::default();
    let mut seen: HashSet<&str> = HashSet::new();
    for (dir, ext) in entries {
        if dir {
            k.dir = true;
        } else if seen.contains(&**ext) {
        } else if seen.len() < KINDS {
            seen.insert(ext);
            k.exts.push(ext.clone());
        } else if k.longest_more.as_ref().is_none_or(|l| ext.chars().count() > l.chars().count()) {
            k.longest_more = Some(ext.clone());
        }
    }
    k
}

/// What the measurement depends on: it is done again when this changes.
#[derive(Clone, PartialEq)]
pub(super) struct Key {
    listing: u64,
    /// A date and a size as they are written now: they change with the language and with the two style settings.
    probes: [String; 2],
    /// The interface language (a translated text's address: another language is another table).
    language: usize,
    explorer_look: bool,
    /// "Calculate size" can stand in the size column.
    calc_link: bool,
    /// The repository's status (its address: a refreshed status is another one), 0 outside a repository.
    git: usize,
    font: SharedString,
    rem: u32,
    /// This folder's tagged items: how many and which tags (the tag store has no change counter).
    tags: (usize, u64),
}

const DAY: i64 = 86_400_000;

impl FolderView {
    /// Measures the automatic columns when something they depend on changed (see the module's text). Called at the start of a frame
    /// that draws the details list.
    pub(super) fn refresh_auto_cols(&mut self, window: &Window, cx: &App) {
        let prefs = cx.global::<crate::settings::Settings>();
        let explorer_look = prefs.windows_file_explorer_theme;
        let now = chrono::Local::now().timestamp_millis();
        let style = window.text_style();
        let rem = f32::from(window.rem_size());
        let dir = if self.is_search() { None } else { self.location().path().map(|p| p.to_path_buf()) };
        let store = &cx.global::<crate::tags::Tags>().0;
        let tagged: Vec<&rf_core::tags::Tagged> = match (&dir, prefs.columns.tags) {
            (Some(d), true) => store.tagged_in(d).collect(),
            _ => Vec::new(),
        };
        let tags_sig = tagged.iter().fold(0u64, |h, t| t.tags.iter().fold(h.rotate_left(7) ^ 0x9E37, |h, id| h.wrapping_mul(31).wrapping_add(u64::from(*id))));
        let key = Key {
            listing: self.entries_gen,
            probes: [format::datetime(now - 400 * DAY), format::size(1023)],
            language: t!("Boyut").as_ptr() as usize,
            explorer_look,
            calc_link: !prefs.calculate_folder_sizes,
            git: self.git.as_ref().map_or(0, |g| Arc::as_ptr(g) as usize),
            font: style.font_family.clone(),
            rem: rem.to_bits(),
            tags: (tagged.len(), tags_sig),
        };
        if self.auto_key.as_ref() == Some(&key) {
            return;
        }
        let (sm, xs) = (px(rem * 0.875), px(rem * 0.75));
        let font = style.font();
        let ts = window.text_system();
        let width = |s: &str, size: Pixels| -> f32 {
            if s.is_empty() {
                return 0.;
            }
            let run = TextRun { len: s.len(), font: font.clone(), color: Hsla::default(), background_color: None, underline: None, strikethrough: None };
            f32::from(ts.layout_line(s, size, &[run], None).width)
        };
        let widest = |texts: &[String], size: Pixels| texts.iter().map(|s| width(s, size)).fold(0., f32::max);
        let heading = |label: &str| width(label, xs);

        // dates: the widest forms the formatter gives in the style that is set (a full date; the longest "… ago" forms)
        let dates: Vec<String> = [400 * DAY, 59 * 60_000, 23 * 3_600_000, 6 * DAY].iter().map(|ago| format::datetime(now - ago)).collect();
        let date_w = widest(&dates, sm);
        // sizes: four digits, three with a unit, a decimal point; and the "calculate" link of a folder, which is drawn smaller
        let sizes: Vec<String> = [1023, 1023 << 10, 104_752_742, 10_726_932_152, 1023 << 30].iter().map(|b| format::size(*b)).collect();
        let size_w = widest(&sizes, sm).max(if key.calc_link { width(t!("Boyutu hesapla"), xs) } else { 0. });
        // types: what this listing has
        let k = kinds(self.entries.iter().take(SCAN).map(|e| (e.is_dir(), &e.ext)));
        let mut types: Vec<String> = k.exts.iter().chain(k.longest_more.iter()).map(|ext| icons::type_label_of(false, ext)).collect();
        if k.dir {
            types.push(icons::type_label_of(true, ""));
        }
        let type_w = widest(&types, sm);
        // tags: the dots (8 px and a gap each) and the first tag's name
        let tag_w = tagged
            .iter()
            .map(|t| {
                let name = t.tags.first().and_then(|id| store.def(*id)).map(|d| width(&crate::i18n::td(&d.name), sm)).unwrap_or(0.);
                t.tags.len() as f32 * 12. + name
            })
            .fold(0., f32::max);
        // git: the states this folder's items have now (a new state is a new status, and the column is measured again)
        let git_w = match (&self.git, &dir) {
            (Some(g), Some(d)) => rf_win::git::states_in(g, d).into_iter().map(|s| width(super::git_label(s), sm)).fold(0., f32::max),
            _ => 0.,
        };

        let fit_explorer = |width: f32, minimum: f32| if explorer_look { width.max(minimum) } else { width };
        self.auto_cols = AutoCols {
            w: [
                Some(fit_explorer(fit(date_w, heading(t!("Değiştirme tarihi")), AIR), EXPLORER_MIN[0])),
                Some(fit_explorer(fit(date_w, heading(t!("Oluşturma tarihi")), AIR), EXPLORER_MIN[1])),
                (!types.is_empty()).then(|| fit_explorer(fit(type_w, heading(t!("Tür")), AIR), EXPLORER_MIN[2])),
                Some(fit_explorer(fit(size_w, heading(t!("Boyut")), 0.), EXPLORER_MIN[3])),
                (!tagged.is_empty()).then(|| fit(tag_w, heading(t!("Dosya etiketi")), AIR)),
                (key.git != 0).then(|| fit(git_w, heading(t!("Git durumu")), AIR)),
                None,
            ],
            // (a place that is not a folder, and a search, keep the column as it was: its items are not "this folder's")
            tags_any: dir.is_none() || !tagged.is_empty(),
        };
        self.auto_key = Some(key);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{AIR, ARROW, AutoCols, KINDS, Kinds, NARROWEST, PAD, STOCK_W, WIDEST, automatic, fit, kept, kinds};

    #[test]
    fn the_stock_widths_are_the_settings_defaults() {
        let d = crate::settings::ColumnPrefs::default();
        let stored: Vec<f32> = d.widths.iter().chain(d.extra_widths.iter()).map(|w| f32::from(*w)).collect();
        assert_eq!(stored, STOCK_W, "a column is automatic while its stored width is the default one: the two lists must agree");
    }

    #[test]
    fn a_column_is_as_wide_as_its_text_or_its_heading() {
        // a date of 105 px under a heading of 96: the text decides, with the air and the padding
        assert_eq!(fit(105., 96., AIR), 105. + AIR + PAD);
        // a type of 40 px ("Dosya") under a wide heading: the heading and its arrow decide
        assert_eq!(fit(40., 90., AIR), 90. + ARROW + PAD);
        // the right-aligned size has no air of its own
        assert_eq!(fit(62., 34., 0.), 62. + PAD);
        // nothing to show: the narrowest; a text beyond all reason: the widest
        assert_eq!(fit(0., 0., AIR), NARROWEST);
        assert_eq!(fit(900., 40., AIR), WIDEST);
        assert_eq!(fit(100.2, 0., AIR), (100.2f32 + AIR + PAD).ceil(), "whole pixels");
    }

    #[test]
    fn a_dragged_column_keeps_its_width_and_only_the_stock_width_is_automatic() {
        let auto = AutoCols { w: [Some(135.), Some(135.), Some(112.), Some(98.), Some(90.), Some(110.), Some(77.)], tags_any: true };
        let mut stored = [150., 150., 150., 100., 130., 120., 150., 240., 130., 140., 90.];
        assert_eq!(auto.apply(stored), [135., 135., 112., 98., 90., 110., 150., 240., 130., 140., 90.], "all automatic but sync and the commit columns");
        stored[2] = 220.; // the user widened the type column
        assert_eq!(auto.apply(stored)[2], 220.);
        assert_eq!(auto.apply(stored)[0], 135., "the others stay automatic");
        // nothing measured (no listing yet): the stored widths
        assert_eq!(AutoCols::default().apply(stored), stored);
    }

    #[test]
    fn a_drag_that_ends_on_the_stock_width_is_still_a_width_of_the_users() {
        assert_eq!(kept(0, 150.), 151);
        assert_eq!(kept(3, 99.6), 101, "rounded first: 100 is the size column's stock width");
        assert_eq!(kept(0, 180.4), 180);
        assert_eq!(kept(6, 150.), 150, "the sync column is never automatic");
        assert_eq!(kept(7, 240.), 240, "nor are the commit columns");
        assert!(automatic(2, 150.) && !automatic(2, 151.) && !automatic(6, 150.) && !automatic(9, 140.));
    }

    #[test]
    fn the_kinds_of_a_listing() {
        let e = |s: &str| -> Arc<str> { Arc::from(s) };
        let items = [(true, e("")), (false, e("rs")), (false, e("rs")), (false, e("")), (false, e("toml")), (true, e("d"))];
        let k = kinds(items.iter().map(|(d, x)| (*d, x)));
        assert!(k.dir);
        assert_eq!(k.exts.iter().map(|x| &**x).collect::<Vec<_>>(), ["rs", "", "toml"], "each once, a folder's own extension not counted");
        assert_eq!(k.longest_more, None);
        assert_eq!(kinds(std::iter::empty()), Kinds::default());
    }

    #[test]
    fn past_the_measured_extensions_the_longest_is_kept() {
        let many: Vec<(bool, Arc<str>)> = (0..KINDS + 3).map(|i| (false, Arc::from(format!("e{i}{}", if i == KINDS + 1 { "-long-one" } else { "" })))).collect();
        let k = kinds(many.iter().map(|(d, x)| (*d, x)));
        assert_eq!(k.exts.len(), KINDS);
        assert_eq!(k.longest_more.as_deref(), Some(&*format!("e{}-long-one", KINDS + 1)));
        assert!(!k.dir);
    }
}
