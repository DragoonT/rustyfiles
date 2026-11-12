//! The right island's panels growing out of their island: the rules that are about WHICH panel, not about the motion (that is
//! `grow.rs`). Pure and tested; `rf-app`'s `dock/panel.rs` and `dock/grow_host.rs` carry them out.
//!
//! * [`press`]: what pressing a button of the island's row leads to, from the state the panels are in. The row STAYS while a panel is
//!   grown out of the island (`Stays::Row`), so its buttons can be pressed while a panel is open: the button of the open panel closes
//!   it, another button hands over to that button's panel.
//! * [`grown_if_clear`]: the rectangle of the grown glass of a panel, or why the panel must open the ordinary way instead.
//! * [`content_rect`]: where the panel's own content stands inside the grown glass.
//! * [`took_effect`]: whether a press did anything (the Windows-key tap needs to know), [`hands_keyboard_on`], [`CloseSeqs`].
//!
//! WHY A HAND-OVER IS "CLOSE, THEN OPEN" AND NOT ONE GLASS GOING FROM PANEL TO PANEL. Every panel is a window of its own with a glass
//! of its own (the window is made when the panel is first opened and kept), and a glass is a shape inside ITS window. Going straight
//! from one panel's size to the other's would need the second window's glass to be on the screen at the first one's rectangle while the
//! first still stands (two glasses over each other, the tint twice), and then the first one taken off in the very frame the second one
//! starts to move: the one hand-over `grow::Run` makes (from the island's glass to the panel's) happens where the two are the same
//! rectangle with nothing else on them, and `Spec` has the island's rectangle as its one starting place. Nothing of that could be seen
//! here, and a mistake would show as a flash on the user's screen. So the first panel shrinks into the island (its content first, then
//! the glass: 160 ms, the same motion as any closing) and the second grows out of it when the first has gone (220 ms). Each of the two
//! is a motion that already exists and that the user has seen.

use super::grow::{Corner, grown_rect, reaches_other_island};
use super::layout::Rect;
use super::panel::PanelId;

/// What the panels are doing when a button of the island's row is pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PressState {
    /// The panel that is open (not on its way out).
    pub open: Option<PanelId>,
    /// The open panel is grown out of its island (so it can shrink back into it; else it is put away at once).
    pub open_grown: bool,
    /// A panel that is shrinking back into its island, or is home and whose window has not gone yet.
    pub leaving: Option<PanelId>,
    /// The panel that is waiting for the one leaving to be gone.
    pub waiting: Option<PanelId>,
}

/// What a press leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Press {
    /// Open it now. Also the press on the panel that is shrinking back: the motion turns round (and anything waiting is dropped).
    OpenNow,
    /// It is the open panel: put it away.
    Close,
    /// Another panel is open and it is not grown: it is put away at once and this one opens, as it always did.
    SwitchNow,
    /// Another panel is open and grown: it shrinks into the island, and this one grows out of it when it has gone.
    CloseThenOpen,
    /// Another panel is shrinking already: this one grows out of the island when that one has gone.
    OpenWhenHome,
    /// This one was already waiting for its turn: the press takes it back (nothing opens).
    Withdraw,
}

/// What pressing the button of `asked` leads to.
pub fn press(s: &PressState, asked: PanelId) -> Press {
    if s.waiting == Some(asked) {
        return Press::Withdraw;
    }
    match s.open {
        Some(o) if o == asked => Press::Close,
        Some(_) if s.open_grown => Press::CloseThenOpen,
        Some(_) => Press::SwitchNow,
        None => match s.leaving {
            Some(l) if l != asked => Press::OpenWhenHome,
            _ => Press::OpenNow,
        },
    }
}

/// Why a panel does not grow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blocked {
    /// The panel had to be moved off the island's anchored edge to fit the screen, so the island's corner is not a corner of the glass.
    CornerMoves,
    /// The grown glass would lie under (or within the margin of) another island; the index is into the `others` given.
    ReachesIsland(usize),
}

/// The grown glass of a panel (`panel` is its content's rectangle, placed above `island` by `panel::place`), or why it must not grow:
/// the island's anchored `corner` must be a corner of it, and it must keep `margin` device px clear of every island in `others`
/// (empty rectangles are no island).
pub fn grown_if_clear(island: Rect, panel: Rect, corner: Corner, others: &[Rect], margin: i32) -> Result<Rect, Blocked> {
    let grown = grown_rect(island, panel);
    let fixed = match corner {
        Corner::BottomLeft => grown.left == island.left,
        Corner::BottomRight => grown.right == island.right,
    } && grown.bottom == island.bottom;
    if !fixed {
        return Err(Blocked::CornerMoves);
    }
    match others.iter().position(|o| reaches_other_island(grown, *o, margin)) {
        Some(i) => Err(Blocked::ReachesIsland(i)),
        None => Ok(grown),
    }
}

/// The panel's own rectangle inside the grown glass: its size, at its own top, lined up with the island's fixed `corner` side, which is
/// where the same panel stands when it opens the ordinary way (`panel::place` lines a right-hand panel up with the island's right edge).
/// A panel as wide as the glass (all but the narrow overflow panel, whose glass is as wide as the island) is where it always was. Device px.
pub fn content_rect(grown: Rect, panel: Rect, corner: Corner) -> Rect {
    let left = match corner {
        Corner::BottomLeft => grown.left,
        Corner::BottomRight => (grown.right - panel.width()).max(grown.left),
    };
    Rect::from_xywh(left, panel.top, panel.width(), panel.height())
}

/// [`content_rect`] for a panel whose content can take more room (every panel but the narrow overflow one, `fills`): a glass that is
/// only a LITTLE wider than the panel gives the panel its whole width. The island grows by a few tray icons, and a 360 px control
/// centre lined up with the corner of a 400 px glass left an empty band down its other side, which read as the content having
/// slipped (a user's report with a picture). "A little" is up to a quarter of the panel's width; beyond that the panel keeps its size
/// at the corner.
pub fn content_rect_in(grown: Rect, panel: Rect, corner: Corner, fills: bool) -> Rect {
    let spare = grown.width() - panel.width();
    if fills && spare > 0 && spare * 4 <= panel.width() {
        return Rect::from_xywh(grown.left, panel.top, grown.width(), panel.height());
    }
    content_rect(grown, panel, corner)
}

/// What the dock knows of the panels at one moment: the open one and the one waiting for its turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen {
    pub open: Option<PanelId>,
    pub waiting: Option<PanelId>,
}

/// Whether a press on the button of `asked` did something, from what was seen before and after it. A press that only QUEUES the panel
/// (it opens when the one leaving has gone) or takes a queued one back did something too: a caller that takes "nothing happened" to
/// mean "could not open" (the Windows key, which then also asks Windows for its own Start) must not count those as failures.
pub fn took_effect(before: Seen, after: Seen, asked: PanelId) -> bool {
    before.open == Some(asked) || before.waiting == Some(asked) || after.open == Some(asked) || after.waiting == Some(asked)
}

/// Whether the window that had the keyboard before the closing panel is NOT given it back when that panel is hidden: only when another
/// panel was waiting AND really opened (it takes the keyboard and gives it back when it is put away). A waiting panel that could not
/// open (a full-screen program came, there is no glass to sit above) leaves the keyboard with nobody, so the closing one gives it back.
pub fn hands_keyboard_on(waiting: bool, opened: bool) -> bool {
    waiting && opened
}

/// How many times each panel has been told to close. A closing task (it waits for the motion, then hides the window) belongs to the
/// close that made it; when the panel was opened again and closed again meanwhile, the earlier task must leave the hiding to the later
/// one, or the window vanishes in the middle of the second closing and the panel waiting for it opens early. (Not the global serial:
/// that is bumped by every opening too, and a panel that is easing out while ANOTHER panel opens still has to be hidden by its task.)
#[derive(Clone, Debug, Default)]
pub struct CloseSeqs(Vec<(PanelId, u64)>);

impl CloseSeqs {
    /// A close of `id` begins: the number its task keeps.
    pub fn bump(&mut self, id: PanelId) -> u64 {
        match self.0.iter_mut().find(|e| e.0 == id) {
            Some(e) => {
                e.1 += 1;
                e.1
            }
            None => {
                self.0.push((id, 1));
                1
            }
        }
    }

    /// Whether the task that kept `mine` is still the latest close of `id`.
    pub fn is_latest(&self, id: PanelId, mine: u64) -> bool {
        self.0.iter().find(|e| e.0 == id).is_some_and(|e| e.1 == mine)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_glass_a_little_wider_than_the_panel_gives_the_panel_its_whole_width() {
        // the control centre (360) in a glass as wide as an island that grew to 400: no empty band at the left
        let grown = Rect::from_xywh(2160, 900, 400, 540);
        let panel = Rect::from_xywh(2200, 900, 360, 470);
        assert_eq!(content_rect_in(grown, panel, Corner::BottomRight, true), Rect::from_xywh(2160, 900, 400, 470));
        // exactly a quarter wider still fills; the same glass and panel: nothing changes
        assert_eq!(content_rect_in(Rect::from_xywh(0, 0, 450, 500), Rect::from_xywh(90, 0, 360, 400), Corner::BottomRight, true).width(), 450);
        assert_eq!(content_rect_in(Rect::from_xywh(0, 0, 360, 500), Rect::from_xywh(0, 0, 360, 400), Corner::BottomRight, true), Rect::from_xywh(0, 0, 360, 400));
        // the overflow panel never fills, however little wider its glass is
        assert_eq!(content_rect_in(Rect::from_xywh(0, 0, 272, 300), Rect::from_xywh(40, 0, 232, 230), Corner::BottomRight, false), Rect::from_xywh(40, 0, 232, 230));
        // a narrow panel in a much wider glass keeps its size at the island's corner
        assert_eq!(content_rect_in(Rect::from_xywh(2160, 900, 400, 300), Rect::from_xywh(2400, 900, 160, 230), Corner::BottomRight, true), Rect::from_xywh(2400, 900, 160, 230));
        assert_eq!(content_rect_in(Rect::from_xywh(0, 900, 400, 300), Rect::from_xywh(0, 900, 160, 230), Corner::BottomLeft, true), Rect::from_xywh(0, 900, 160, 230));
    }

    use super::*;
    use crate::dock::grow::Spec;
    use crate::dock::layout::{self, Widths};
    use crate::dock::panel::{self, GAP, Side};
    use crate::dock::size::{self, DockSize};
    use crate::dock::tray_strip;

    const CC: PanelId = PanelId::CONTROL_CENTRE;
    const CAL: PanelId = PanelId("calendar");
    const NOTIF: PanelId = PanelId("notifications");
    const OVF: PanelId = PanelId("tray-overflow");

    fn st(open: Option<PanelId>, open_grown: bool, leaving: Option<PanelId>, waiting: Option<PanelId>) -> PressState {
        PressState { open, open_grown, leaving, waiting }
    }

    #[test]
    fn a_press_on_the_open_panels_button_closes_it_whether_it_is_grown_or_not() {
        assert_eq!(press(&st(Some(CC), true, None, None), CC), Press::Close);
        assert_eq!(press(&st(Some(CC), false, None, None), CC), Press::Close);
    }

    #[test]
    fn a_press_on_another_button_hands_over_to_that_panel() {
        // grown: the open one shrinks into the island first, then the pressed one grows out of it
        assert_eq!(press(&st(Some(CC), true, None, None), CAL), Press::CloseThenOpen);
        assert_eq!(press(&st(Some(OVF), true, None, None), NOTIF), Press::CloseThenOpen);
        // not grown (the setting is off, or the screen has no room for it): the old way, the first goes at once
        assert_eq!(press(&st(Some(CC), false, None, None), CAL), Press::SwitchNow);
    }

    #[test]
    fn with_nothing_open_a_press_opens() {
        assert_eq!(press(&st(None, false, None, None), CC), Press::OpenNow);
        assert_eq!(press(&st(None, false, None, None), OVF), Press::OpenNow);
    }

    #[test]
    fn a_press_while_a_panel_shrinks_waits_for_it_or_turns_it_round() {
        // CC is on its way back into the island: the calendar's button waits for it to be gone
        assert_eq!(press(&st(None, false, Some(CC), None), CAL), Press::OpenWhenHome);
        // the button of the one that is shrinking turns the motion round
        assert_eq!(press(&st(None, false, Some(CC), None), CC), Press::OpenNow);
        // a third button while one waits: it takes the waiting one's place
        assert_eq!(press(&st(None, false, Some(CC), Some(CAL)), NOTIF), Press::OpenWhenHome);
        // the waiting one's own button takes it back
        assert_eq!(press(&st(None, false, Some(CC), Some(CAL)), CAL), Press::Withdraw);
        // and the shrinking one's button, with another waiting, turns it round (the caller drops the waiting one)
        assert_eq!(press(&st(None, false, Some(CC), Some(CAL)), CC), Press::OpenNow);
    }

    #[test]
    fn a_hand_over_in_progress_withdraws_with_one_more_press_on_the_same_button() {
        // CC open and grown; the calendar's button: CloseThenOpen, the caller now records the calendar as waiting and CC is leaving
        let first = press(&st(Some(CC), true, None, None), CAL);
        assert_eq!(first, Press::CloseThenOpen);
        assert_eq!(press(&st(None, false, Some(CC), Some(CAL)), CAL), Press::Withdraw);
    }

    /// A 2560 x 1440 monitor with the taskbar's 48 px, at this scale and dock size: the three islands and the work area.
    fn screen(width: i32, height: i32, dpi: f32, factor: f32, right_w: f32) -> (Rect, layout::Islands, f32) {
        let work = Rect::new(0, 0, width, height - (48.0 * dpi).round() as i32);
        let unit = size::unit(dpi, factor);
        (work, layout::islands(work, unit, Widths { left: 330.0, middle: 600.0, right: right_w }), unit)
    }

    /// The four panels' sizes (logical px): the control centre, the calendar, the notifications and the overflow with 0, 3 and 14 icons;
    /// each with a short and a tall content.
    fn panel_sizes() -> Vec<(&'static str, (f32, f32))> {
        let mut v = vec![];
        for h in [120.0, 480.0, 900.0] {
            v.push(("control centre", (360.0, h)));
            v.push(("calendar", (340.0, h)));
            v.push(("notifications", (380.0, h)));
        }
        for n in [0, 1, 3, 6, 14] {
            v.push(("overflow", tray_strip::ovf_size(n)));
        }
        v
    }

    /// The right island is as wide as its row and its tray strip: 207 without a strip, wider with one.
    const RIGHT_WIDTHS: [f32; 4] = [207.0, 250.0, 320.0, 420.0];

    #[test]
    fn the_grown_rectangle_of_each_panel_holds_the_island_and_the_content_and_keeps_the_bottom_right_corner() {
        for dpi in [1.0, 1.25, 1.5] {
            for f in DockSize::ALL.map(DockSize::factor) {
                for rw in RIGHT_WIDTHS {
                    let (work, isl, unit) = screen(2560, 1440, dpi, f, rw);
                    for (name, size) in panel_sizes() {
                        let panel_rect = panel::place(isl.right, size, dpi, Side::Right, work);
                        let ctx = format!("{name} {size:?} at {dpi} {f} {rw}");
                        let grown = match grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], layout::px(GAP, dpi)) {
                            Ok(g) => g,
                            // (the largest dock at 150 % on this screen squeezes the middle island up to the right one: the panel
                            // opens the ordinary way there, which is the other test's business)
                            Err(Blocked::ReachesIsland(_)) => continue,
                            Err(e) => panic!("{ctx}: {e:?}"),
                        };
                        assert!(grown.covers(&isl.right) && grown.covers(&panel_rect), "{ctx}");
                        assert_eq!(grown.right, isl.right.right, "{ctx}: the right edge is the island's");
                        assert_eq!(grown.bottom, isl.right.bottom, "{ctx}: the bottom edge is the island's");
                        assert_eq!(grown.top, panel_rect.top, "{ctx}");
                        assert_eq!(grown.width(), panel_rect.width().max(isl.right.width()), "{ctx}");
                        assert_eq!(grown.height(), panel_rect.height() + layout::px(GAP, dpi) + isl.right.height(), "{ctx}: the gap is inside the glass");
                        // it grows to the left and up from the corner, so the spec's own check agrees
                        let spec = Spec::new(isl.right, panel_rect, Corner::BottomRight, layout::RADIUS * unit, 8.0 * dpi);
                        assert!(spec.corner_is_fixed(), "{ctx}");
                        assert_eq!(spec.grown, grown, "{ctx}");
                        // it never touches another island, with the margin the host keeps
                        assert!(!reaches_other_island(grown, isl.left, layout::px(GAP, dpi)) && !reaches_other_island(grown, isl.middle, layout::px(GAP, dpi)), "{ctx}");
                        // the content stands inside the glass, above the island's row and the gap, lined up with the island's right edge
                        let c = content_rect(grown, panel_rect, Corner::BottomRight);
                        assert!(grown.covers(&c) && c.bottom <= isl.right.top - layout::px(GAP, dpi), "{ctx}");
                        assert_eq!((c.width(), c.height(), c.top), (panel_rect.width(), panel_rect.height(), panel_rect.top), "{ctx}");
                        assert_eq!(c.right, grown.right, "{ctx}: lined up with the island's right edge, where the ordinary panel stands");
                        assert_eq!(c, panel_rect, "{ctx}: the same place as with the setting off");
                        if panel_rect.width() >= isl.right.width() {
                            assert_eq!(c, panel_rect, "{ctx}: a panel as wide as the glass is where it always was");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_growth_that_would_reach_the_middle_island_does_not_happen_and_never_falls_through() {
        // Whatever the screen: when the verdict is "grow" the grown glass keeps clear of the other islands, and when it is not, it says
        // which one it would have reached. A narrow or scaled screen is the common cause (a laptop at 125 % and 1920 px).
        let mut grown_count = 0;
        let mut blocked_count = 0;
        for (w, h) in [(1366, 768), (1600, 900), (1920, 1080), (2560, 1440), (3440, 1440), (3840, 2160)] {
            for dpi in [1.0, 1.25, 1.5] {
                for f in DockSize::ALL.map(DockSize::factor) {
                    for rw in RIGHT_WIDTHS {
                        let (work, isl, _) = screen(w, h, dpi, f, rw);
                        let margin = layout::px(GAP, dpi);
                        for (name, size) in panel_sizes() {
                            let panel_rect = panel::place(isl.right, size, dpi, Side::Right, work);
                            match grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], margin) {
                                Ok(grown) => {
                                    grown_count += 1;
                                    for o in [isl.left, isl.middle] {
                                        assert!(!reaches_other_island(grown, o, margin), "{name} {w}x{h} at {dpi} {f} {rw}");
                                    }
                                }
                                Err(Blocked::ReachesIsland(i)) => {
                                    blocked_count += 1;
                                    let other = [isl.left, isl.middle][i];
                                    let grown = grown_rect(isl.right, panel_rect);
                                    assert!(reaches_other_island(grown, other, margin), "{name} {w}x{h} at {dpi} {f} {rw}");
                                }
                                Err(Blocked::CornerMoves) => {
                                    blocked_count += 1;
                                    assert!(panel_rect.right != isl.right.right || panel_rect.bottom > isl.right.top, "{name} {w}x{h}: the corner moved for no reason");
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(grown_count > 0 && blocked_count > 0, "both verdicts happen across these screens: {grown_count} grow, {blocked_count} do not");
    }

    #[test]
    fn on_the_users_screen_every_panel_grows_at_100_percent_at_every_dock_size_and_tray_width() {
        // 2560 x 1440 at 100 %: the case the user looks at
        for f in DockSize::ALL.map(DockSize::factor) {
            for rw in RIGHT_WIDTHS {
                let (work, isl, _) = screen(2560, 1440, 1.0, f, rw);
                for (name, size) in panel_sizes() {
                    let panel_rect = panel::place(isl.right, size, 1.0, Side::Right, work);
                    assert!(grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], layout::px(GAP, 1.0)).is_ok(), "{name} {size:?} {f} {rw}");
                }
            }
        }
        // and at 125 % and 150 % with the dock at its default size or smaller, and the ordinary right island
        for dpi in [1.25, 1.5] {
            for f in [0.85, 1.0] {
                let (work, isl, _) = screen(2560, 1440, dpi, f, 207.0);
                for (name, size) in panel_sizes() {
                    let panel_rect = panel::place(isl.right, size, dpi, Side::Right, work);
                    assert!(grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], layout::px(GAP, dpi)).is_ok(), "{name} {size:?} {dpi} {f}");
                }
            }
        }
    }

    #[test]
    fn the_numbers_of_the_control_centre_at_100_percent_on_the_users_screen() {
        // 2560 x 1440, taskbar 48: the right island (2218, 1334, 2548, 1382) for a 330 wide row; the control centre is 360 wide
        let (work, isl, _) = screen(2560, 1440, 1.0, 1.0, 330.0);
        assert_eq!(isl.right, Rect::new(2218, 1334, 2548, 1382));
        let panel_rect = panel::place(isl.right, (360.0, 400.0), 1.0, Side::Right, work);
        assert_eq!(panel_rect, Rect::new(2188, 926, 2548, 1326));
        let grown = grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], 8).unwrap();
        assert_eq!(grown, Rect::new(2188, 926, 2548, 1382), "the panel's top to the island's bottom; the island's 330 wide row stands in its bottom right");
        let spec = Spec::new(isl.right, panel_rect, Corner::BottomRight, 24.0, 8.0);
        assert_eq!(spec.at(0.0).rect, [2218.0, 1334.0, 330.0, 48.0]);
        assert_eq!(spec.at(1.0).rect, [2188.0, 926.0, 360.0, 456.0]);
        // the corner stays where it is the whole way
        for i in 0..=100 {
            let [x, y, w, h] = spec.at(i as f32 / 100.0).rect;
            assert_eq!((x + w, y + h), (2548.0, 1382.0));
        }
    }

    /// A windowless read-only probe (run with `--ignored --nocapture`): the grown rectangle of each panel on a 2560 x 1440 monitor at
    /// 100, 125 and 150 % and every dock size, and the verdict. It opens nothing and touches nothing.
    #[test]
    #[ignore]
    fn probe_the_grown_rectangles_on_a_2560_by_1440_monitor() {
        for dpi in [1.0, 1.25, 1.5] {
            for (dname, f) in [("Kucuk", 0.85), ("Orta", 1.0), ("Buyuk", 1.2), ("CokBuyuk", 1.4)] {
                let (work, isl, _) = screen(2560, 1440, dpi, f, 207.0);
                let margin = layout::px(GAP, dpi);
                println!("-- {}% {dname}: right island {:?} (w {}), middle {:?}", (dpi * 100.0) as i32, isl.right, isl.right.width(), isl.middle);
                for (name, size) in [("control centre", (360.0, 420.0)), ("calendar", (340.0, 440.0)), ("notifications", (380.0, 600.0)), ("overflow x3", tray_strip::ovf_size(3))] {
                    let panel_rect = panel::place(isl.right, size, dpi, Side::Right, work);
                    match grown_if_clear(isl.right, panel_rect, Corner::BottomRight, &[isl.left, isl.middle], margin) {
                        Ok(g) => {
                            let c = content_rect(g, panel_rect, Corner::BottomRight);
                            println!("   {name:15} grows: glass {:?} ({}x{}), content {:?}, gap to the middle island {}", g, g.width(), g.height(), c, g.left - isl.middle.right);
                        }
                        Err(e) => println!("   {name:15} opens the ordinary way: {e:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn a_panel_pushed_off_the_islands_edge_does_not_grow() {
        // a screen so narrow that the panel is shifted right of where it lines up with: the island's corner is not a corner of the glass
        let island = Rect::new(60, 700, 300, 748);
        let panel_rect = Rect::new(0, 300, 360, 692);
        assert_eq!(grown_if_clear(island, panel_rect, Corner::BottomRight, &[], 8), Err(Blocked::CornerMoves));
        // the same panel lined up with the island's right edge grows
        let lined = Rect::new(-60, 300, 300, 692);
        assert!(grown_if_clear(island, lined, Corner::BottomRight, &[], 8).is_ok());
        // an empty rectangle is no island
        assert!(grown_if_clear(island, lined, Corner::BottomRight, &[Rect::default()], 8).is_ok());
    }

    #[test]
    fn the_narrow_overflow_panel_stays_where_it_stands_with_the_setting_off() {
        let grown = Rect::new(2218, 900, 2548, 1382);
        let panel_rect = Rect::new(2412, 900, 2548, 1326); // 136 wide, lined up with the island's right edge
        assert_eq!(content_rect(grown, panel_rect, Corner::BottomRight), panel_rect);
        // a panel as wide as the glass is where it was
        assert_eq!(content_rect(grown, Rect::new(2218, 900, 2548, 1326), Corner::BottomRight), Rect::new(2218, 900, 2548, 1326));
        // the left corner lines up with the island's left edge
        let left = Rect::new(12, 900, 342, 1382);
        assert_eq!(content_rect(left, Rect::new(12, 900, 148, 1326), Corner::BottomLeft), Rect::new(12, 900, 148, 1326));
    }

    const START: PanelId = PanelId::START;

    fn seen(open: Option<PanelId>, waiting: Option<PanelId>) -> Seen {
        Seen { open, waiting }
    }

    /// What the host leaves behind for each kind of press (as `panel::toggle` does), to follow a press to its end.
    fn apply(s: &PressState, asked: PanelId, opens: bool) -> Seen {
        match press(s, asked) {
            Press::OpenNow | Press::SwitchNow => seen(opens.then_some(asked), None),
            Press::Close => seen(None, None),
            Press::CloseThenOpen | Press::OpenWhenHome => seen(None, Some(asked)),
            Press::Withdraw => seen(s.open, None),
        }
    }

    #[test]
    fn a_press_that_only_queues_or_takes_back_a_queued_panel_counts_as_taking_effect() {
        // the Windows-key tap with the control centre grown: Start is queued behind the shrinking panel (reported as failure before)
        let s = st(Some(CC), true, None, None);
        assert!(took_effect(seen(Some(CC), None), apply(&s, START, true), START));
        // a tap while another panel is already shrinking
        let s = st(None, false, Some(CC), None);
        assert!(took_effect(seen(None, None), apply(&s, START, true), START));
        // a second tap while Start waits takes it back
        let s = st(None, false, Some(CC), Some(START));
        assert!(took_effect(seen(None, Some(START)), apply(&s, START, true), START));
        // Start open and pressed: closes
        let s = st(Some(START), true, None, None);
        assert!(took_effect(seen(Some(START), None), apply(&s, START, true), START));
        // the ordinary opening, and the one that could not open (a full-screen program): the only failure
        let s = st(None, false, None, None);
        assert!(took_effect(seen(None, None), apply(&s, START, true), START));
        assert!(!took_effect(seen(None, None), apply(&s, START, false), START));
        // a press on another panel's button is not Start's business
        assert!(!took_effect(seen(Some(CC), None), seen(Some(CC), None), START));
    }

    #[test]
    fn the_keyboard_is_handed_on_only_to_a_panel_that_really_opened() {
        assert!(hands_keyboard_on(true, true));
        assert!(!hands_keyboard_on(true, false), "the waiting panel could not open: the closing one gives the keyboard back");
        assert!(!hands_keyboard_on(false, false));
        assert!(!hands_keyboard_on(false, true));
    }

    #[test]
    fn an_earlier_closing_task_leaves_the_hiding_to_a_later_close_of_the_same_panel() {
        let mut c = CloseSeqs::default();
        // close, opened again, closed again: the first task must not hide the window in the middle of the second closing
        let first = c.bump(CC);
        assert!(c.is_latest(CC, first));
        let second = c.bump(CC);
        assert!(!c.is_latest(CC, first));
        assert!(c.is_latest(CC, second));
        // another panel's closing is no business of the first one's: a panel easing out while another opens is still hidden by its own task
        let other = c.bump(CAL);
        assert!(c.is_latest(CC, second) && c.is_latest(CAL, other));
        // a task of a panel that was never closed is not the latest
        assert!(!CloseSeqs::default().is_latest(OVF, 1));
    }

    #[test]
    fn a_middle_island_that_widens_while_the_panel_is_open_blocks_the_glass_on_the_next_check() {
        // 1920 x 1080 at 100 %: a middle island that gets wider (apps pinned or launched) comes to meet the glass of an open panel; the
        // same rules that let it open now say it must not stay grown (`panel::refit` asks them again at every layout)
        let (work, narrow, _) = screen(1920, 1080, 1.0, 1.0, 207.0);
        let panel_rect = panel::place(narrow.right, (340.0, 440.0), 1.0, Side::Right, work);
        let margin = layout::px(GAP, 1.0);
        assert!(grown_if_clear(narrow.right, panel_rect, Corner::BottomRight, &[narrow.left, narrow.middle], margin).is_ok());
        let wide = layout::islands(work, size::unit(1.0, 1.0), Widths { left: 330.0, middle: 1300.0, right: 207.0 });
        assert!(matches!(grown_if_clear(wide.right, panel_rect, Corner::BottomRight, &[wide.left, wide.middle], margin), Err(Blocked::ReachesIsland(1))));
    }
}
