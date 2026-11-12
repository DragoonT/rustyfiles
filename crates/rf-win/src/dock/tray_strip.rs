//! The tray strip in the right island and its overflow panel (shell plan increment D3): the arithmetic and the decisions that need
//! neither a window nor a tray host. Where the icons lie and how many fit, what the overflow panel looks like, where a dragged icon
//! lands, which click a pointer or a key means, and how the keyboard walks the strip and the panel. All of it pure and tested; the one
//! call into Windows at the end only asks for the foreground.
//!
//! THE STRIP is a row of cells, [`CELL`] logical px wide (the 16 px picture centred in it), at the left end of the right island's
//! content, then a chevron cell, then a thin separator, then the island's own symbols. As many cells as fit [`strip_limit`], the rest
//! (and the icons the user hid) are in the overflow panel the chevron opens. The island is anchored at the right edge of the screen,
//! so it grows to the LEFT by [`Layout::extra_width`] and everything that was there stays where it was.
//!
//! WHERE THE ORDER COMES FROM is `tray_order` (the user's list and the file); this module takes its [`Arrangement`](super::tray_order::Arrangement)
//! as plain indexes and flags.

use super::layout::{Rect, px};
use super::tray_order::Dest;

// ---------------------------------------------------------------- the strip

/// The picture, logical px (at 100 %).
pub const ICON: f32 = 16.0;
/// A cell of the strip: the picture and room around it.
pub const CELL: f32 = 24.0;
/// The chevron's cell.
pub const CHEVRON_W: f32 = 24.0;
/// The most room the strip block (cells and chevron) may take, logical px.
pub const MAX_STRIP_W: f32 = 240.0;
/// The island's padding before its first item (the same number as `rf-app`'s `plan::SIDE_PAD`; a test there keeps them equal).
pub const SIDE_PAD: f32 = 8.0;
/// Space on each side of the thin separator after the strip.
pub const SEP_GAP: f32 = 8.0;
pub const SEP_W: f32 = 1.0;

/// How wide the strip block may be on a screen `work_width` logical px wide: a quarter of the screen, at most [`MAX_STRIP_W`], at
/// least the chevron.
pub fn strip_limit(work_width: f32) -> f32 {
    MAX_STRIP_W.min(work_width * 0.25).max(CHEVRON_W)
}

/// How many cells fit a block at most `limit` logical px wide (the chevron takes its cell first).
pub fn capacity(limit: f32) -> usize {
    ((limit - CHEVRON_W) / CELL).floor().max(0.0) as usize
}

/// What the strip shows: which icons are in the strip, which are in the overflow panel, whether there is a chevron.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// Present icons shown as cells, left to right.
    pub visible: Vec<usize>,
    /// Present icons in the overflow panel: the ones the user hid and the ones that did not fit, in the user's order.
    pub overflow: Vec<usize>,
    /// There is a chevron whenever there is any icon at all: it is the place to drop an icon to hide it, and with nothing hidden the
    /// panel explains that.
    pub chevron: bool,
}

impl Layout {
    /// The strip block's width, logical px: the cells and the chevron.
    pub fn block_width(&self) -> f32 {
        self.visible.len() as f32 * CELL + if self.chevron { CHEVRON_W } else { 0.0 }
    }

    /// What the strip adds to the right island's width: the block and the separator after it. Nothing without icons.
    pub fn extra_width(&self) -> f32 {
        if self.chevron { self.block_width() + 2.0 * SEP_GAP + SEP_W } else { 0.0 }
    }
}

/// The strip for the present icons `all` (indexes, in the user's order), of which `hidden[i]` were hidden by the user, in a block at most
/// `limit` logical px wide. The icons that do not fit are the LAST of the strip.
pub fn layout(all: &[usize], hidden: &[bool], limit: f32) -> Layout {
    if all.is_empty() {
        return Layout::default();
    }
    let is_hidden = |i: usize| hidden.get(i).copied().unwrap_or(false);
    let cap = capacity(limit);
    let visible: Vec<usize> = all.iter().copied().filter(|&i| !is_hidden(i)).take(cap).collect();
    let overflow: Vec<usize> = all.iter().copied().filter(|&i| is_hidden(i) || !visible.contains(&i)).collect();
    Layout { visible, overflow, chevron: true }
}

// ---------------------------------------------------------------- where things are on the screen

/// Cell `j` of the strip, device px, screen coordinates, in an island whose visible glass is `glass`: centred in the glass's height.
pub fn cell_rect(glass: Rect, scale: f32, j: usize) -> Rect {
    let cell = px(CELL, scale);
    let left = glass.left + px(SIDE_PAD, scale) + j as i32 * cell;
    let top = glass.top + (glass.height() - cell) / 2;
    Rect::from_xywh(left, top, cell, cell)
}

/// The chevron's cell when `visible` cells stand before it.
pub fn chevron_rect(glass: Rect, scale: f32, visible: usize) -> Rect {
    let cell = cell_rect(glass, scale, visible);
    Rect::from_xywh(cell.left, cell.top, px(CHEVRON_W, scale), cell.height())
}

/// Where a dragged icon would land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DropTarget {
    /// In the strip, before cell `n` (`n` equal to the number of cells: at the end).
    Strip(usize),
    /// On the chevron: into the overflow.
    Chevron,
    /// On the open overflow panel: into the overflow.
    Overflow,
    /// Nowhere: the icon stays where it was.
    Nowhere,
}

/// What the drop test needs to know about the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Geometry {
    /// The right island's visible glass, device px.
    pub glass: Rect,
    pub scale: f32,
    /// How many cells the strip shows.
    pub visible: usize,
    pub chevron: bool,
    /// The open overflow panel's rectangle, device px.
    pub overflow: Option<Rect>,
}

/// The target under `cursor` (device px, screen coordinates). In the strip the nearer gap between two cells decides.
pub fn drop_target(g: &Geometry, cursor: (i32, i32)) -> DropTarget {
    let (x, y) = cursor;
    if g.overflow.is_some_and(|r| r.contains(x, y)) {
        return DropTarget::Overflow;
    }
    if y < g.glass.top || y >= g.glass.bottom {
        return DropTarget::Nowhere;
    }
    let cell = px(CELL, g.scale).max(1);
    let origin = g.glass.left + px(SIDE_PAD, g.scale);
    let end = origin + g.visible as i32 * cell;
    if x >= g.glass.left && x < end {
        let raw = ((x - origin) as f32 / cell as f32 + 0.5).floor().max(0.0) as usize;
        return DropTarget::Strip(raw.min(g.visible));
    }
    if g.chevron && x >= end && x < end + px(CHEVRON_W, g.scale) {
        return DropTarget::Chevron;
    }
    DropTarget::Nowhere
}

/// The place in the strip's list a dragged icon is inserted at, from the gap the pointer is at (`raw`, counted in the cells as they are
/// now) and the icon's own cell (`src`, `None` for an icon that comes from the overflow): the icon is taken out first, so the gaps after
/// it are one less.
pub fn reorder_index(src: Option<usize>, raw: usize) -> usize {
    match src {
        Some(s) if s < raw => raw - 1,
        _ => raw,
    }
}

/// What dropping present icon `icon` on `target` asks of the order: where to move it (the list and the place in it), or `None` for a drop
/// that moves nothing. `visible` are the strip's present icons, left to right, as they are on the screen now; `in_overflow` says the icon
/// is one of the overflow panel's (a drag that began there and ends over the panel again is a click that wandered, not a move). `cap` is
/// how many cells the strip has room for ([`capacity`]).
///
/// The order's strip list also holds the icons that did not fit (they follow the shown ones), so a place at or past `cap` is a place in the
/// overflow panel. An icon let go on the end gap of a FULL strip is meant to be the strip's last cell: it takes the place of the last
/// shown one, which is pushed out.
pub fn drop_move(visible: &[usize], icon: usize, in_overflow: bool, target: DropTarget, cap: usize) -> Option<(Dest, usize)> {
    match target {
        DropTarget::Strip(raw) => {
            let src = visible.iter().position(|&v| v == icon);
            let at = reorder_index(src, raw);
            let at = if src.is_none() && cap > 0 { at.min(cap - 1) } else { at };
            Some((Dest::Strip, at))
        }
        DropTarget::Overflow if in_overflow => None,
        DropTarget::Chevron | DropTarget::Overflow => Some((Dest::Hidden, usize::MAX)),
        DropTarget::Nowhere => None,
    }
}

// ---------------------------------------------------------------- the overflow panel

/// A cell of the panel and the picture in it, logical px.
pub const OVF_CELL: f32 = 40.0;
pub const OVF_ICON: f32 = 20.0;
/// Space between the panel's edge and its cells.
pub const OVF_PAD: f32 = 8.0;
/// Most cells in a row.
pub const OVF_COLS: usize = 6;
/// The panel's width and the height of its one line when nothing is in it.
pub const OVF_EMPTY_W: f32 = 232.0;
pub const OVF_EMPTY_H: f32 = 48.0;

/// Columns and rows for `count` icons: a row of up to [`OVF_COLS`], then more rows, evened out (7 icons are 4 and 3, not 6 and 1).
pub fn ovf_dims(count: usize) -> (usize, usize) {
    if count == 0 {
        return (0, 0);
    }
    let rows = count.div_ceil(OVF_COLS);
    (count.div_ceil(rows), rows)
}

/// The panel's size, logical px.
pub fn ovf_size(count: usize) -> (f32, f32) {
    if count == 0 {
        return (OVF_EMPTY_W, OVF_EMPTY_H + 2.0 * OVF_PAD);
    }
    let (cols, rows) = ovf_dims(count);
    (2.0 * OVF_PAD + cols as f32 * OVF_CELL, 2.0 * OVF_PAD + rows as f32 * OVF_CELL)
}

/// Cell `i` (x, y, w, h), logical px in the panel.
pub fn ovf_cell(count: usize, i: usize) -> [f32; 4] {
    let (cols, _) = ovf_dims(count);
    let cols = cols.max(1);
    [OVF_PAD + (i % cols) as f32 * OVF_CELL, OVF_PAD + (i / cols) as f32 * OVF_CELL, OVF_CELL, OVF_CELL]
}

/// The cell at (`x`, `y`), logical px in the panel.
pub fn ovf_hit(count: usize, x: f32, y: f32) -> Option<usize> {
    (0..count).find(|&i| {
        let [cx, cy, w, h] = ovf_cell(count, i);
        x >= cx && x < cx + w && y >= cy && y < cy + h
    })
}

// ---------------------------------------------------------------- what a click means

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerButton {
    Left,
    Right,
    Middle,
}

/// What the program is to be told. The tray host's click engine turns it into the program's messages (the layout of those depends on the
/// version the program asked for); a click from the keyboard carries the icon's top left corner as its place, and the host makes
/// `NIN_KEYSELECT` and `WM_CONTEXTMENU` of the two key kinds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickKind {
    Left,
    LeftDouble,
    Right,
    Middle,
    /// Enter or Space on the icon.
    KeySelect,
    /// The context-menu key or Shift+F10 on the icon.
    KeyContext,
}

/// The click a press of `button` is, given the toolkit's `click_count` for it (1 for the first press, 2 for the second of a double click,
/// 3 for the third, which starts a new one). Only the left button has a double click that a program hears about.
pub fn pointer_click(button: PointerButton, click_count: usize) -> ClickKind {
    match button {
        PointerButton::Left if click_count >= 2 && click_count.is_multiple_of(2) => ClickKind::LeftDouble,
        PointerButton::Left => ClickKind::Left,
        PointerButton::Right => ClickKind::Right,
        PointerButton::Middle => ClickKind::Middle,
    }
}

/// How far (logical px) the pointer must go with the button held before a press on an icon is a drag and not a click.
pub const DRAG_START: f32 = 4.0;

/// Whether the pointer has gone far enough from where it was pressed.
pub fn moved_enough(from: (i32, i32), to: (i32, i32), scale: f32) -> bool {
    let limit = px(DRAG_START, scale).max(1);
    (to.0 - from.0).abs() >= limit || (to.1 - from.1).abs() >= limit
}

/// A button held on an icon, from the press to the release.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Press {
    pub button: PointerButton,
    pub count: usize,
    /// Where it went down, device px, screen coordinates.
    pub at: (i32, i32),
    pub dragging: bool,
}

/// What one look at the held button found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Held, not moved enough yet.
    Idle,
    /// Moved enough: the icon is being dragged from now on.
    DragBegan,
    /// Held and dragging.
    DragMove,
    /// The button is up. `dragged`: it was a drag (a drop), otherwise a click.
    Released { dragged: bool },
}

/// One look at the pointer (`cursor`) and the buttons (`down`: any is held). Only the left button drags.
pub fn step(p: &mut Press, cursor: (i32, i32), down: bool, scale: f32) -> Step {
    if !down {
        return Step::Released { dragged: p.dragging };
    }
    if p.dragging {
        return Step::DragMove;
    }
    if p.button == PointerButton::Left && moved_enough(p.at, cursor, scale) {
        p.dragging = true;
        return Step::DragBegan;
    }
    Step::Idle
}

// ---------------------------------------------------------------- the keyboard

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    Tab,
    ShiftTab,
    /// Enter or Space: a left click.
    Activate,
    /// The context-menu key, or Shift+F10: a right click.
    Menu,
    Escape,
}

/// The key the toolkit names (`"left"`, `"enter"`, `"menu"`, `"f10"` ...) with its modifiers. `None` for anything the strip does not use,
/// and for any key with Control or Alt held (those belong to the program in front or to the system).
pub fn nav_key(key: &str, shift: bool, ctrl: bool, alt: bool) -> Option<NavKey> {
    if ctrl || alt {
        return None;
    }
    Some(match key {
        "left" => NavKey::Left,
        "right" => NavKey::Right,
        "up" => NavKey::Up,
        "down" => NavKey::Down,
        "home" => NavKey::Home,
        "end" => NavKey::End,
        "tab" if shift => NavKey::ShiftTab,
        "tab" => NavKey::Tab,
        "enter" | "space" => NavKey::Activate,
        "menu" => NavKey::Menu,
        "f10" if shift => NavKey::Menu,
        "escape" => NavKey::Escape,
        _ => return None,
    })
}

/// What a key does in the strip. The cursor walks the `visible` cells and then the chevron (if there is one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StripOutcome {
    Stay,
    /// The cursor moved to this stop.
    Moved(usize),
    /// A click on the icon in this cell, from the keyboard.
    Click(ClickKind, usize),
    /// The chevron was activated: the overflow panel opens or closes.
    ToggleOverflow,
    /// The keyboard leaves the strip (Escape, Tab past either end).
    Leave,
}

/// The number of stops: the cells and the chevron.
pub fn stops(visible: usize, chevron: bool) -> usize {
    visible + usize::from(chevron)
}

pub fn strip_nav(key: NavKey, cursor: usize, visible: usize, chevron: bool) -> StripOutcome {
    let n = stops(visible, chevron);
    if n == 0 {
        return if key == NavKey::Escape || key == NavKey::Tab || key == NavKey::ShiftTab { StripOutcome::Leave } else { StripOutcome::Stay };
    }
    let cursor = cursor.min(n - 1);
    let on_icon = cursor < visible;
    match key {
        NavKey::Left if cursor > 0 => StripOutcome::Moved(cursor - 1),
        NavKey::Right if cursor + 1 < n => StripOutcome::Moved(cursor + 1),
        NavKey::Home if cursor > 0 => StripOutcome::Moved(0),
        NavKey::End if cursor + 1 < n => StripOutcome::Moved(n - 1),
        NavKey::Tab if cursor + 1 < n => StripOutcome::Moved(cursor + 1),
        NavKey::ShiftTab if cursor > 0 => StripOutcome::Moved(cursor - 1),
        NavKey::Tab | NavKey::ShiftTab | NavKey::Escape => StripOutcome::Leave,
        NavKey::Activate if on_icon => StripOutcome::Click(ClickKind::KeySelect, cursor),
        NavKey::Menu if on_icon => StripOutcome::Click(ClickKind::KeyContext, cursor),
        NavKey::Activate | NavKey::Menu => StripOutcome::ToggleOverflow,
        _ => StripOutcome::Stay,
    }
}

/// What a key does in the overflow panel (a grid of `count` cells in `cols` columns). The cursor starts on nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridOutcome {
    Stay,
    Moved(usize),
    Click(ClickKind, usize),
    Close,
}

pub fn grid_nav(key: NavKey, cursor: Option<usize>, count: usize, cols: usize) -> GridOutcome {
    if key == NavKey::Escape {
        return GridOutcome::Close;
    }
    if count == 0 {
        return GridOutcome::Stay;
    }
    let cols = cols.max(1);
    let Some(c) = cursor.map(|c| c.min(count - 1)) else {
        // nothing chosen yet: the first key that moves goes to the first cell (the last for End and Shift+Tab)
        return match key {
            NavKey::End | NavKey::ShiftTab => GridOutcome::Moved(count - 1),
            NavKey::Left | NavKey::Right | NavKey::Up | NavKey::Down | NavKey::Home | NavKey::Tab => GridOutcome::Moved(0),
            _ => GridOutcome::Stay,
        };
    };
    match key {
        NavKey::Left if c > 0 => GridOutcome::Moved(c - 1),
        NavKey::Right if c + 1 < count => GridOutcome::Moved(c + 1),
        NavKey::Up if c >= cols => GridOutcome::Moved(c - cols),
        // down into a shorter last row lands on its last cell
        NavKey::Down if c + cols < count => GridOutcome::Moved(c + cols),
        NavKey::Down if c / cols + 1 < count.div_ceil(cols) => GridOutcome::Moved(count - 1),
        NavKey::Home => GridOutcome::Moved(0),
        NavKey::End => GridOutcome::Moved(count - 1),
        NavKey::Tab => GridOutcome::Moved((c + 1) % count),
        NavKey::ShiftTab => GridOutcome::Moved((c + count - 1) % count),
        NavKey::Activate => GridOutcome::Click(ClickKind::KeySelect, c),
        NavKey::Menu => GridOutcome::Click(ClickKind::KeyContext, c),
        _ => GridOutcome::Stay,
    }
}

// ---------------------------------------------------------------- pictures

/// A program's picture as the tray host hands it over (straight alpha, red first) to the order the toolkit's image wants (blue first).
/// A length that is not a whole number of pixels is cut to one.
pub fn rgba_to_bgra(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() - rgba.len() % 4);
    for p in rgba.as_chunks::<4>().0 {
        out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    out
}

// ---------------------------------------------------------------- the one call into Windows

/// Asks for the foreground for the island's window so that it gets the keys (an island is never activated by a click, but Windows
/// allows a program to bring a no-activate window forward when it has just had input). Unlike the panels' `bring_to_front_quietly` it
/// does nothing else if Windows says no: that function's last resort takes the window out of the topmost layer, which an island must
/// never leave. Returns whether the window has the foreground now.
pub fn take_keyboard(hwnd: isize) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};
    if hwnd == 0 {
        return false;
    }
    let h = HWND(hwnd as *mut _);
    unsafe {
        let _ = SetForegroundWindow(h);
        GetForegroundWindow() == h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    // ---- layout

    #[test]
    fn no_icons_no_strip() {
        let l = layout(&[], &[], 240.0);
        assert_eq!(l, Layout::default());
        assert_eq!(l.extra_width(), 0.0);
        assert_eq!(l.block_width(), 0.0);
    }

    #[test]
    fn a_few_icons_all_fit_and_the_chevron_is_there() {
        let l = layout(&ids(3), &[false; 3], 240.0);
        assert_eq!(l.visible, vec![0, 1, 2]);
        assert!(l.overflow.is_empty());
        assert!(l.chevron);
        assert_eq!(l.block_width(), 3.0 * 24.0 + 24.0);
        assert_eq!(l.extra_width(), 96.0 + 8.0 + 1.0 + 8.0);
    }

    #[test]
    fn what_does_not_fit_goes_to_the_overflow_from_the_end() {
        // 240 px: the chevron and nine cells
        let l = layout(&ids(12), &[false; 12], 240.0);
        assert_eq!(l.visible, (0..9).collect::<Vec<_>>());
        assert_eq!(l.overflow, vec![9, 10, 11]);
        assert!(l.block_width() <= 240.0);
        // exactly nine fit; ten is one over
        assert_eq!(layout(&ids(9), &[false; 9], 240.0).overflow, Vec::<usize>::new());
        assert_eq!(layout(&ids(10), &[false; 10], 240.0).overflow, vec![9]);
    }

    #[test]
    fn the_icons_the_user_hid_are_in_the_overflow_in_the_users_order() {
        let mut hidden = vec![false; 5];
        hidden[1] = true;
        hidden[3] = true;
        // the user's order: 4 3 2 1 0
        let all = vec![4, 3, 2, 1, 0];
        let l = layout(&all, &hidden, 240.0);
        assert_eq!(l.visible, vec![4, 2, 0]);
        assert_eq!(l.overflow, vec![3, 1]);
    }

    #[test]
    fn hidden_and_too_many_together() {
        let mut hidden = vec![false; 14];
        hidden[0] = true;
        let l = layout(&ids(14), &hidden, 240.0);
        assert_eq!(l.visible, (1..10).collect::<Vec<_>>());
        assert_eq!(l.overflow, vec![0, 10, 11, 12, 13], "in the user's order, hidden and cut alike");
    }

    #[test]
    fn a_narrow_limit_leaves_the_chevron_alone() {
        let l = layout(&ids(4), &[false; 4], 24.0);
        assert!(l.visible.is_empty());
        assert_eq!(l.overflow, vec![0, 1, 2, 3]);
        assert!(l.chevron);
        assert_eq!(l.block_width(), 24.0);
        // a limit below the chevron is not a negative number of cells
        assert!(layout(&ids(2), &[false; 2], 5.0).visible.is_empty());
    }

    #[test]
    fn the_limit_is_a_quarter_of_the_screen_but_never_more_than_the_cap() {
        assert_eq!(strip_limit(2560.0), MAX_STRIP_W);
        assert_eq!(strip_limit(1280.0), 240.0);
        assert_eq!(strip_limit(800.0), 200.0);
        assert_eq!(strip_limit(40.0), CHEVRON_W, "never below the chevron");
    }

    // ---- where things are

    fn glass() -> Rect {
        // a 400 x 48 right island on a 2560 x 1440 screen with the Windows taskbar: (2148, 1334) .. (2548, 1382)
        Rect::new(2148, 1334, 2548, 1382)
    }

    #[test]
    fn cells_stand_after_the_islands_padding_centred_in_its_height() {
        let r = cell_rect(glass(), 1.0, 0);
        assert_eq!(r, Rect::new(2156, 1346, 2180, 1370));
        assert_eq!(cell_rect(glass(), 1.0, 3).left, 2156 + 72);
        let c = chevron_rect(glass(), 1.0, 3);
        assert_eq!((c.left, c.width(), c.height()), (2156 + 72, 24, 24));
        // 150 %: 36 px cells, 12 px padding
        let g = Rect::new(3000, 2000, 3600, 2072);
        let r = cell_rect(g, 1.5, 2);
        assert_eq!((r.left, r.top, r.width()), (3000 + 12 + 72, 2000 + 18, 36));
    }

    fn geometry(visible: usize, overflow: Option<Rect>) -> Geometry {
        Geometry { glass: glass(), scale: 1.0, visible, chevron: true, overflow }
    }

    #[test]
    fn a_drop_in_the_strip_goes_to_the_nearer_gap() {
        let g = geometry(4, None);
        let origin = 2156;
        assert_eq!(drop_target(&g, (origin, 1350)), DropTarget::Strip(0));
        assert_eq!(drop_target(&g, (origin + 11, 1350)), DropTarget::Strip(0), "left half of the first cell");
        assert_eq!(drop_target(&g, (origin + 12, 1350)), DropTarget::Strip(1), "right half of the first cell");
        assert_eq!(drop_target(&g, (origin + 60, 1350)), DropTarget::Strip(3));
        assert_eq!(drop_target(&g, (origin + 95, 1350)), DropTarget::Strip(4), "the right half of the last cell: the end");
        // the island's own padding before the first cell is the first gap
        assert_eq!(drop_target(&g, (2150, 1350)), DropTarget::Strip(0));
    }

    #[test]
    fn the_chevron_takes_a_drop_and_the_rest_of_the_island_does_not() {
        let g = geometry(4, None);
        let end = 2156 + 96;
        assert_eq!(drop_target(&g, (end, 1350)), DropTarget::Chevron);
        assert_eq!(drop_target(&g, (end + 23, 1350)), DropTarget::Chevron);
        assert_eq!(drop_target(&g, (end + 24, 1350)), DropTarget::Nowhere, "the separator and the symbols");
        assert_eq!(drop_target(&g, (end + 200, 1350)), DropTarget::Nowhere);
        // above or below the island
        assert_eq!(drop_target(&g, (2160, 1300)), DropTarget::Nowhere);
        assert_eq!(drop_target(&g, (2160, 1382)), DropTarget::Nowhere);
        // left of the island
        assert_eq!(drop_target(&g, (2100, 1350)), DropTarget::Nowhere);
    }

    #[test]
    fn with_no_chevron_there_is_no_chevron_drop() {
        let g = Geometry { chevron: false, ..geometry(2, None) };
        assert_eq!(drop_target(&g, (2156 + 48, 1350)), DropTarget::Nowhere);
    }

    #[test]
    fn the_open_overflow_panel_takes_a_drop_into_the_overflow() {
        let panel = Rect::new(2300, 1100, 2548, 1326);
        let g = geometry(3, Some(panel));
        assert_eq!(drop_target(&g, (2400, 1200)), DropTarget::Overflow);
        assert_eq!(drop_target(&g, (2299, 1200)), DropTarget::Nowhere);
        // the strip still works with the panel open
        assert_eq!(drop_target(&g, (2160, 1350)), DropTarget::Strip(0));
    }

    #[test]
    fn dragging_an_icon_to_another_gap_is_a_move_of_the_right_size() {
        // [A B C D]: A dropped before C (gap 2) lands at place 1 of the list without A
        assert_eq!(reorder_index(Some(0), 2), 1);
        // D dropped before B (gap 1): place 1
        assert_eq!(reorder_index(Some(3), 1), 1);
        // dropped at its own gap or the one after it: no move
        assert_eq!(reorder_index(Some(1), 1), 1);
        assert_eq!(reorder_index(Some(1), 2), 1);
        // from the overflow nothing is taken out first
        assert_eq!(reorder_index(None, 2), 2);
        // at the end
        assert_eq!(reorder_index(Some(0), 4), 3);
    }

    #[test]
    fn a_drop_says_where_the_icon_goes() {
        // cells [5 7 9]: icon 5 dropped before the third cell goes to place 1 of the list without it; icon 9 before the first to place 0
        assert_eq!(drop_move(&[5, 7, 9], 5, false, DropTarget::Strip(2), 9), Some((Dest::Strip, 1)));
        assert_eq!(drop_move(&[5, 7, 9], 9, false, DropTarget::Strip(0), 9), Some((Dest::Strip, 0)));
        // an icon from the overflow (not among the cells) is put where the pointer is
        assert_eq!(drop_move(&[5, 7, 9], 2, true, DropTarget::Strip(1), 9), Some((Dest::Strip, 1)));
        // on the chevron or the panel: to the end of the hidden ones
        assert_eq!(drop_move(&[5, 7, 9], 7, false, DropTarget::Chevron, 9), Some((Dest::Hidden, usize::MAX)));
        assert_eq!(drop_move(&[5, 7, 9], 7, false, DropTarget::Overflow, 9), Some((Dest::Hidden, usize::MAX)));
        assert_eq!(drop_move(&[5, 7, 9], 7, false, DropTarget::Nowhere, 9), None);
        // an icon of the panel let go over the panel again: nothing moves (it is a click that wandered); on the chevron it is hidden
        assert_eq!(drop_move(&[5, 7, 9], 2, true, DropTarget::Overflow, 9), None);
        assert_eq!(drop_move(&[5, 7, 9], 2, true, DropTarget::Chevron, 9), Some((Dest::Hidden, usize::MAX)));
    }

    #[test]
    fn an_icon_let_go_on_the_end_gap_of_a_full_strip_becomes_its_last_cell() {
        // 9 cells fit (240 px) and 9 are shown: the end gap is gap 9
        let cap = capacity(MAX_STRIP_W);
        assert_eq!(cap, 9);
        let shown: Vec<usize> = (0..9).collect();
        // an icon from the overflow (10) let go there: place 8, the last cell, not place 9, which is past the shown ones
        assert_eq!(drop_move(&shown, 10, true, DropTarget::Strip(9), cap), Some((Dest::Strip, 8)));
        // without the capacity the place is 9, the first place of the overflow panel (what the drop did before)
        assert_eq!(drop_move(&shown, 10, true, DropTarget::Strip(9), usize::MAX), Some((Dest::Strip, 9)));
        // not full: the end gap is the end
        assert_eq!(drop_move(&shown[..4], 10, true, DropTarget::Strip(4), cap), Some((Dest::Strip, 4)));
        // an icon of the strip let go on the end gap of the full strip is already handled by taking it out first
        assert_eq!(drop_move(&shown, 2, false, DropTarget::Strip(9), cap), Some((Dest::Strip, 8)));
        // no room at all: the number is left alone
        assert_eq!(drop_move(&[], 3, true, DropTarget::Strip(0), 0), Some((Dest::Strip, 0)));
    }

    // ---- the overflow panel

    #[test]
    fn the_panel_wraps_into_even_rows() {
        assert_eq!(ovf_dims(0), (0, 0));
        assert_eq!(ovf_dims(1), (1, 1));
        assert_eq!(ovf_dims(6), (6, 1));
        assert_eq!(ovf_dims(7), (4, 2), "4 and 3, not 6 and 1");
        assert_eq!(ovf_dims(12), (6, 2));
        assert_eq!(ovf_dims(13), (5, 3));
        for n in 1..60 {
            let (c, r) = ovf_dims(n);
            assert!(c <= OVF_COLS && c * r >= n && (r - 1) * c < n, "{n}: {c} x {r}");
        }
    }

    #[test]
    fn the_panels_size_follows_its_cells() {
        assert_eq!(ovf_size(1), (2.0 * 8.0 + 40.0, 2.0 * 8.0 + 40.0));
        assert_eq!(ovf_size(7), (16.0 + 4.0 * 40.0, 16.0 + 80.0));
        let (w, h) = ovf_size(0);
        assert!(w > 100.0 && h > 40.0, "room for the one line that explains");
    }

    #[test]
    fn a_cell_of_the_panel_is_found_by_its_place() {
        assert_eq!(ovf_cell(7, 0), [8.0, 8.0, 40.0, 40.0]);
        assert_eq!(ovf_cell(7, 4), [8.0, 48.0, 40.0, 40.0], "the second row starts again at the left");
        assert_eq!(ovf_hit(7, 10.0, 10.0), Some(0));
        assert_eq!(ovf_hit(7, 8.0 + 3.0 * 40.0 + 5.0, 10.0), Some(3));
        assert_eq!(ovf_hit(7, 10.0, 55.0), Some(4));
        assert_eq!(ovf_hit(7, 8.0 + 3.0 * 40.0 + 5.0, 55.0), None, "the last row has three cells; this is past the 7th");
        assert_eq!(ovf_hit(7, 2.0, 2.0), None, "the panel's padding");
        assert_eq!(ovf_hit(0, 10.0, 10.0), None);
    }

    // ---- clicks

    #[test]
    fn the_pointers_buttons_are_the_programs_clicks() {
        assert_eq!(pointer_click(PointerButton::Left, 1), ClickKind::Left);
        assert_eq!(pointer_click(PointerButton::Left, 2), ClickKind::LeftDouble);
        assert_eq!(pointer_click(PointerButton::Left, 3), ClickKind::Left, "the third press starts a new click");
        assert_eq!(pointer_click(PointerButton::Left, 4), ClickKind::LeftDouble);
        assert_eq!(pointer_click(PointerButton::Left, 0), ClickKind::Left);
        assert_eq!(pointer_click(PointerButton::Right, 1), ClickKind::Right);
        assert_eq!(pointer_click(PointerButton::Right, 2), ClickKind::Right, "no double click for the other buttons");
        assert_eq!(pointer_click(PointerButton::Middle, 1), ClickKind::Middle);
    }

    #[test]
    fn a_press_becomes_a_drag_only_when_the_pointer_moves_far_enough_with_the_left_button() {
        let mut p = Press { button: PointerButton::Left, count: 1, at: (100, 100), dragging: false };
        assert_eq!(step(&mut p, (102, 101), true, 1.0), Step::Idle);
        assert_eq!(step(&mut p, (103, 100), true, 1.0), Step::Idle);
        assert_eq!(step(&mut p, (104, 100), true, 1.0), Step::DragBegan);
        assert!(p.dragging);
        // once dragging, it stays one, even back at the start
        assert_eq!(step(&mut p, (100, 100), true, 1.0), Step::DragMove);
        assert_eq!(step(&mut p, (300, 90), false, 1.0), Step::Released { dragged: true });
        // a click: down and up without moving
        let mut q = Press { button: PointerButton::Left, count: 1, at: (100, 100), dragging: false };
        assert_eq!(step(&mut q, (100, 100), true, 1.0), Step::Idle);
        assert_eq!(step(&mut q, (101, 100), false, 1.0), Step::Released { dragged: false });
        // the other buttons never drag
        let mut r = Press { button: PointerButton::Right, count: 1, at: (100, 100), dragging: false };
        assert_eq!(step(&mut r, (200, 200), true, 1.0), Step::Idle);
        assert_eq!(step(&mut r, (200, 200), false, 1.0), Step::Released { dragged: false });
        // at 200 % the distance is eight device px
        let mut s = Press { button: PointerButton::Left, count: 1, at: (0, 0), dragging: false };
        assert_eq!(step(&mut s, (7, 0), true, 2.0), Step::Idle);
        assert_eq!(step(&mut s, (8, 0), true, 2.0), Step::DragBegan);
    }

    // ---- keys

    #[test]
    fn the_keys_the_strip_uses() {
        let k = |key, shift| nav_key(key, shift, false, false);
        assert_eq!(k("left", false), Some(NavKey::Left));
        assert_eq!(k("tab", false), Some(NavKey::Tab));
        assert_eq!(k("tab", true), Some(NavKey::ShiftTab));
        assert_eq!(k("enter", false), Some(NavKey::Activate));
        assert_eq!(k("space", false), Some(NavKey::Activate));
        assert_eq!(k("menu", false), Some(NavKey::Menu));
        assert_eq!(k("f10", true), Some(NavKey::Menu), "Shift+F10");
        assert_eq!(k("f10", false), None, "F10 alone is not the menu key");
        assert_eq!(k("escape", false), Some(NavKey::Escape));
        assert_eq!(k("a", false), None);
        // Control and Alt are not ours
        assert_eq!(nav_key("left", false, true, false), None);
        assert_eq!(nav_key("enter", false, false, true), None);
    }

    #[test]
    fn the_cursor_walks_the_cells_and_the_chevron() {
        // 3 cells and a chevron: stops 0..=3
        assert_eq!(stops(3, true), 4);
        assert_eq!(strip_nav(NavKey::Right, 0, 3, true), StripOutcome::Moved(1));
        assert_eq!(strip_nav(NavKey::Right, 2, 3, true), StripOutcome::Moved(3), "onto the chevron");
        assert_eq!(strip_nav(NavKey::Right, 3, 3, true), StripOutcome::Stay, "no wrap");
        assert_eq!(strip_nav(NavKey::Left, 0, 3, true), StripOutcome::Stay);
        assert_eq!(strip_nav(NavKey::Left, 3, 3, true), StripOutcome::Moved(2));
        assert_eq!(strip_nav(NavKey::Home, 2, 3, true), StripOutcome::Moved(0));
        assert_eq!(strip_nav(NavKey::End, 0, 3, true), StripOutcome::Moved(3));
        assert_eq!(strip_nav(NavKey::Home, 0, 3, true), StripOutcome::Stay);
    }

    #[test]
    fn tab_walks_on_and_leaves_at_either_end() {
        assert_eq!(strip_nav(NavKey::Tab, 0, 3, true), StripOutcome::Moved(1));
        assert_eq!(strip_nav(NavKey::Tab, 3, 3, true), StripOutcome::Leave);
        assert_eq!(strip_nav(NavKey::ShiftTab, 2, 3, true), StripOutcome::Moved(1));
        assert_eq!(strip_nav(NavKey::ShiftTab, 0, 3, true), StripOutcome::Leave);
        assert_eq!(strip_nav(NavKey::Escape, 1, 3, true), StripOutcome::Leave);
    }

    #[test]
    fn enter_is_a_left_click_and_the_menu_key_a_right_click_on_an_icon_and_either_opens_the_panel_on_the_chevron() {
        assert_eq!(strip_nav(NavKey::Activate, 1, 3, true), StripOutcome::Click(ClickKind::KeySelect, 1));
        assert_eq!(strip_nav(NavKey::Menu, 2, 3, true), StripOutcome::Click(ClickKind::KeyContext, 2));
        assert_eq!(strip_nav(NavKey::Activate, 3, 3, true), StripOutcome::ToggleOverflow);
        assert_eq!(strip_nav(NavKey::Menu, 3, 3, true), StripOutcome::ToggleOverflow);
        // a cursor left past the end by a strip that shrank is pulled in
        assert_eq!(strip_nav(NavKey::Activate, 9, 3, true), StripOutcome::ToggleOverflow);
    }

    #[test]
    fn an_empty_strip_has_nothing_to_walk() {
        assert_eq!(strip_nav(NavKey::Right, 0, 0, false), StripOutcome::Stay);
        assert_eq!(strip_nav(NavKey::Activate, 0, 0, false), StripOutcome::Stay);
        assert_eq!(strip_nav(NavKey::Escape, 0, 0, false), StripOutcome::Leave);
        // only the chevron
        assert_eq!(strip_nav(NavKey::Activate, 0, 0, true), StripOutcome::ToggleOverflow);
    }

    #[test]
    fn the_panels_grid_is_walked_by_arrows() {
        // 7 cells in 4 columns: rows of 4 and 3
        let (n, cols) = (7, 4);
        assert_eq!(grid_nav(NavKey::Right, Some(0), n, cols), GridOutcome::Moved(1));
        assert_eq!(grid_nav(NavKey::Right, Some(6), n, cols), GridOutcome::Stay);
        assert_eq!(grid_nav(NavKey::Left, Some(0), n, cols), GridOutcome::Stay);
        assert_eq!(grid_nav(NavKey::Down, Some(1), n, cols), GridOutcome::Moved(5));
        assert_eq!(grid_nav(NavKey::Down, Some(3), n, cols), GridOutcome::Moved(6), "into a shorter row: its last cell");
        assert_eq!(grid_nav(NavKey::Down, Some(5), n, cols), GridOutcome::Stay, "already on the last row");
        assert_eq!(grid_nav(NavKey::Up, Some(5), n, cols), GridOutcome::Moved(1));
        assert_eq!(grid_nav(NavKey::Up, Some(2), n, cols), GridOutcome::Stay);
        assert_eq!(grid_nav(NavKey::Home, Some(5), n, cols), GridOutcome::Moved(0));
        assert_eq!(grid_nav(NavKey::End, Some(0), n, cols), GridOutcome::Moved(6));
    }

    #[test]
    fn tab_wraps_in_the_panel_and_the_first_key_starts_the_walk() {
        assert_eq!(grid_nav(NavKey::Tab, Some(6), 7, 4), GridOutcome::Moved(0));
        assert_eq!(grid_nav(NavKey::ShiftTab, Some(0), 7, 4), GridOutcome::Moved(6));
        assert_eq!(grid_nav(NavKey::Down, None, 7, 4), GridOutcome::Moved(0));
        assert_eq!(grid_nav(NavKey::Tab, None, 7, 4), GridOutcome::Moved(0));
        assert_eq!(grid_nav(NavKey::ShiftTab, None, 7, 4), GridOutcome::Moved(6));
        assert_eq!(grid_nav(NavKey::Activate, None, 7, 4), GridOutcome::Stay, "nothing under the cursor yet");
    }

    #[test]
    fn enter_and_the_menu_key_click_the_cell_in_the_panel_and_escape_closes_it() {
        assert_eq!(grid_nav(NavKey::Activate, Some(2), 7, 4), GridOutcome::Click(ClickKind::KeySelect, 2));
        assert_eq!(grid_nav(NavKey::Menu, Some(2), 7, 4), GridOutcome::Click(ClickKind::KeyContext, 2));
        assert_eq!(grid_nav(NavKey::Escape, Some(2), 7, 4), GridOutcome::Close);
        assert_eq!(grid_nav(NavKey::Escape, None, 0, 1), GridOutcome::Close);
        // an empty panel
        assert_eq!(grid_nav(NavKey::Down, None, 0, 1), GridOutcome::Stay);
        // a cursor past the end (the panel shrank) is pulled in
        assert_eq!(grid_nav(NavKey::Activate, Some(9), 3, 3), GridOutcome::Click(ClickKind::KeySelect, 2));
    }

    // ---- pictures

    #[test]
    fn red_and_blue_change_places_and_alpha_stays() {
        assert_eq!(rgba_to_bgra(&[1, 2, 3, 4, 10, 20, 30, 40]), vec![3, 2, 1, 4, 30, 20, 10, 40]);
        assert_eq!(rgba_to_bgra(&[1, 2, 3, 4, 9, 9]), vec![3, 2, 1, 4], "a part of a pixel is cut");
        assert!(rgba_to_bgra(&[]).is_empty());
    }

    #[test]
    fn asking_for_no_window_gives_no_keyboard() {
        assert!(!take_keyboard(0));
    }
}
