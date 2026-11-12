//! The preview pane at the bottom of the window (setting `preview_position` = bottom; the default is the right).
//!
//! The rules (what fits, the limits, the splitter's arithmetic) are the pure functions of `rf_core::previewpane`; this file is the
//! window's side: it measures the row that holds the list and the pane, builds the pane's frame with its top-edge splitter, keeps the
//! pane's height (`preview_height`, written to the settings once the drag has settled) and tells the pane what box it has.
//!
//! In a split tab (two panes) the pane sits under both panes, as it sits at the right of both today: it shows the focused item of the
//! active pane. The shelf, when open, narrows the list(s) only; the preview runs under the shelf too. When the two panes are stacked,
//! each of them must keep its usable height, so the pane is allowed less room there (`list_min_h`).
//!
//! An Open/Save dialog window keeps the pane at the right, whatever the setting says: those windows are small, they keep their own
//! on/off state for the pane, and the setting is a choice for the file manager's windows.
//!
//! This is a child module of `workspace` (see the `#[path]` line there), so the window file only gets a few small hooks.
//! With the position at the right (the default) the cost per frame is one setting read; nothing here touches the pane.

use std::cell::Cell;
use std::rc::Rc;

use rf_core::previewpane as pp;

use super::*;

/// The row's height is not known before the first frame has been laid out: until then the window minus about this much of chrome
/// (title bar, toolbar, command bar, status bar) stands in for it.
const FALLBACK_CHROME_H: f32 = 190.;
/// The content card's right margin and its two borders: the area the pane has is that much narrower than the window minus the sidebar.
const CARD_SIDE: f32 = 10.;

/// Drag payload of the pane's top edge.
#[derive(Clone)]
struct ResizePreviewBottom;

/// What the window knows about this frame when it asks for the pane's frame.
#[derive(Clone, Copy)]
pub(super) struct BottomFrame {
    /// The position this frame was built for (read once by the caller): the bottom, or the right.
    pub bottom: bool,
    /// `preview_room` said the pane is shown at all.
    pub shown: bool,
    /// The tab shows two file panes one above the other.
    pub stacked: bool,
    pub side_w: f32,
    pub viewport_h: f32,
}

/// The window's state for the pane at the bottom.
pub struct Dock {
    /// The wanted height (the setting), clamped to the absolute range.
    height: f32,
    /// The height of the row that holds the list and the pane, as the last frame laid it out (0 = not measured yet).
    area_h: Rc<Cell<f32>>,
    /// Debounced settings write after a drag.
    save: Option<Task<()>>,
    /// The pane has been given a wide box and not yet told that it is back at the right (so the right position costs no call per frame).
    told_wide: bool,
}

impl Dock {
    pub fn new(stored_height: f32) -> Self {
        Dock { height: pp::stored_height(stored_height), area_h: Rc::new(Cell::new(0.)), save: None, told_wide: false }
    }

    fn area_h(&self, viewport_h: f32) -> f32 {
        let m = self.area_h.get();
        if m > 0. { m } else { (viewport_h - FALLBACK_CHROME_H).max(0.) }
    }

    /// The saved height changed somewhere else (another window dragged it, the file was reset): follow it. A drag in this window
    /// writes the setting from this value, so the two are equal while it runs.
    pub fn sync_height(&mut self, saved: f32) -> bool {
        let h = pp::stored_height(saved);
        if (self.height - h).abs() < 0.5 {
            return false;
        }
        self.height = h;
        true
    }
}

/// A thin invisible strip over the pane's top edge that starts a resize.
fn top_resize_handle<T: Clone + 'static>(id: &'static str, payload: T) -> impl IntoElement {
    div()
        .id(id)
        .absolute()
        .left_0()
        .w_full()
        .h(px(pp::SPLITTER))
        .top(px(-pp::SPLITTER / 2.))
        .cursor_row_resize()
        .on_drag(payload, |_, _, _, cx| cx.new(|_| ResizeGhost))
}

/// The two menu entries and the settings row both end here: the position is kept, and choosing one shows the pane.
pub(super) fn set_preview_position(cx: &mut App, pos: pp::PreviewPos) {
    crate::settings::update(cx, |s| {
        s.preview_position = pos;
        s.preview_open = true;
    });
}

impl Workspace {
    /// Whether this window puts the pane at the bottom: the setting says so, and it is not a file dialog's window.
    pub(super) fn preview_at_bottom(&self, cx: &App) -> bool {
        self.picker.is_none() && cx.global::<crate::settings::Settings>().preview_position == pp::PreviewPos::Bottom
    }

    /// The tab shows two file panes one above the other (the pane at the bottom then has two lists to leave room for).
    pub(super) fn preview_stacked(&self, cx: &App) -> bool {
        self.tabs[self.active].right.is_some() && !cx.global::<crate::settings::Settings>().dual_pane_vertical
    }

    /// The saved pane height changed: follow it (called from the settings observer).
    pub(super) fn sync_preview_height(&mut self, cx: &mut Context<Self>) {
        let saved = cx.global::<crate::settings::Settings>().preview_height;
        self.preview_dock.sync_height(saved);
    }

    /// Whether the pane has room in its place. At the right this is the rule the window always had (the list keeps its width).
    /// `viewport_h` is read only for the bottom position (0 is fine for the right).
    pub(super) fn preview_room(&self, bottom: bool, stacked: bool, side_w: f32, viewport_h: f32) -> bool {
        if bottom {
            pp::bottom_fits(self.viewport_w - side_w - CARD_SIDE, self.preview_dock.area_h(viewport_h), stacked)
        } else {
            pp::right_fits(self.viewport_w, side_w, self.preview_w)
        }
    }

    /// The pane's frame for the bottom position, or None. When the position is the right, the pane is told so once, and then nothing
    /// runs here. Called every frame, after `preview_room` said whether the pane is shown at all.
    pub(super) fn preview_bottom_pane(&mut self, f: BottomFrame, p: &Palette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let BottomFrame { bottom, shown, stacked, side_w, viewport_h } = f;
        if !bottom {
            if self.preview_dock.told_wide {
                self.preview_dock.told_wide = false;
                self.preview.update(cx, |pv, cx| pv.set_wide(None, cx));
            }
            return None;
        }
        if !shown {
            return None;
        }
        let area_w = self.viewport_w - side_w - CARD_SIDE;
        let h = pp::bottom_layout(area_w, self.preview_dock.area_h(viewport_h), self.preview_dock.height, stacked).pane_h?;
        let target = self.active_tab().read(cx).preview_target();
        self.preview_dock.told_wide = true;
        self.preview.update(cx, |pv, cx| {
            pv.set_target(target, cx);
            // (the top border takes one pixel of the frame)
            pv.set_wide(Some((area_w, h - 1.)), cx);
        });
        Some(
            div()
                .w_full()
                .h(px(h))
                .flex_shrink_0()
                .relative()
                .border_t_1()
                .border_color(p.divider)
                .child(self.preview.clone())
                .child(top_resize_handle("preview-resize-h", ResizePreviewBottom))
                .into_any_element(),
        )
    }

    /// The row under the command bar for the bottom position: the list (and the shelf) above, the pane under them across the whole width.
    /// It also measures its own height for the next frame, which is what the pane's limits are worked out from.
    pub(super) fn preview_bottom_row(&mut self, list: Div, shelf: Option<Div>, pane: Option<AnyElement>, cx: &mut Context<Self>) -> AnyElement {
        let cell = self.preview_dock.area_h.clone();
        let me = cx.entity().downgrade();
        v_flex()
            .id("preview-bottom-row")
            .flex_1()
            .min_h_0()
            .relative()
            .on_drag_move(cx.listener(|this, e: &DragMoveEvent<ResizePreviewBottom>, _, cx| {
                // the pane ends where the row ends: its height is the distance from the pointer to the row's bottom
                let stacked = this.preview_stacked(cx);
                let h = pp::height_from_drag(f32::from(e.bounds.bottom()), f32::from(e.event.position.y), f32::from(e.bounds.size.height), stacked);
                this.set_preview_height(h, cx);
            }))
            .child(h_flex().flex_1().min_h_0().child(list).children(shelf))
            .children(pane)
            .child(
                canvas(
                    move |bounds, _, cx| {
                        let h = f32::from(bounds.size.height);
                        // a changed height changes what fits: lay out again once with the measured number
                        if (cell.replace(h) - h).abs() > 0.5 {
                            let _ = me.update(cx, |_, cx| cx.notify());
                        }
                    },
                    |_, _, _, _| {},
                )
                .absolute()
                .size_full(),
            )
            .into_any_element()
    }

    /// Live resize of the pane at the bottom; the height is kept in the settings (written once the drag has settled).
    fn set_preview_height(&mut self, h: f32, cx: &mut Context<Self>) {
        let h = pp::stored_height(h);
        if (self.preview_dock.height - h).abs() < 0.5 {
            return;
        }
        self.preview_dock.height = h;
        cx.update_global::<crate::settings::Settings, _>(|s, _| s.preview_height = h);
        self.preview_dock.save = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(400)).await;
            cx.update(|cx| {
                let snapshot = cx.global::<crate::settings::Settings>().clone();
                std::thread::spawn(move || snapshot.save());
            });
        }));
        cx.notify();
    }
}
